#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Bounded OpenMC photon energy-closure control; does not qualify neutron heating.

The run mode uses the OpenMC 0.15.3 C API driver already audited in this FARIS
checkout, and preserves one statepoint per batch for same-batch covariance.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess
import sys

SOURCE_E_EV = 1.0e6
BATCHES = 40
PARTICLES = 5000
PROJECT_ROOT = Path(__file__).resolve().parents[1]


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def sample_stats(values):
    n = len(values)
    if n < 2:
        raise ValueError("at least two independent batch realizations are required")
    mean = math.fsum(values) / n
    variance = math.fsum((x - mean) ** 2 for x in values) / (n - 1)
    return {"mean": mean, "sample_sd": math.sqrt(variance), "standard_error": math.sqrt(variance / n)}


def batch_values(cumulative):
    if not cumulative:
        raise ValueError("no cumulative statepoint sums")
    out, previous = [], 0.0
    for total in cumulative:
        value = float(total) - previous
        if not math.isfinite(value):
            raise ValueError("non-finite tally result")
        out.append(value)
        previous = float(total)
    return out


def covariance_matrix(columns):
    names = list(columns)
    n = len(columns[names[0]])
    if n < 2 or any(len(columns[k]) != n for k in names):
        raise ValueError("covariance requires aligned batch vectors of length >=2")
    means = {k: math.fsum(columns[k]) / n for k in names}
    cov = {}
    for a in names:
        cov[a] = {}
        for b in names:
            cov[a][b] = math.fsum((x - means[a]) * (y - means[b])
                                  for x, y in zip(columns[a], columns[b])) / (n - 1)
    return cov


def make_openmc_model(openmc, batches=BATCHES, particles=PARTICLES):
    """Build pure-API model. Caller exports XML; this function never starts transport."""
    fe_inner = openmc.Material(name="Fe56 inner", material_id=901)
    fe_inner.add_nuclide("Fe56", 1.0)
    fe_inner.set_density("g/cm3", 7.874)
    fe_shell = openmc.Material(name="Fe56 shell", material_id=902)
    fe_shell.add_nuclide("Fe56", 1.0)
    fe_shell.set_density("g/cm3", 7.874)
    inner = openmc.Sphere(r=2.0)
    outer = openmc.Sphere(r=10.0, boundary_type="vacuum")
    inner_cell = openmc.Cell(name="inner Fe56", fill=fe_inner, region=-inner)
    shell_cell = openmc.Cell(name="shell Fe56", fill=fe_shell, region=+inner & -outer)
    geometry = openmc.Geometry([inner_cell, shell_cell])
    materials = openmc.Materials([fe_inner, fe_shell])

    settings = openmc.Settings()
    settings.run_mode = "fixed source"
    settings.particles = particles
    settings.batches = batches
    settings.photon_transport = True
    settings.electron_treatment = "led"
    # In the pinned OpenMC 0.15.3 API atomic relaxation is enabled by default
    # whenever photon transport is on; the Python toggle was added later.
    settings.source = openmc.IndependentSource(
        particle="photon",
        space=openmc.stats.Point((0.0, 0.0, 0.0)),
        angle=openmc.stats.Isotropic(),
        energy=openmc.stats.Discrete([SOURCE_E_EV], [1.0]),
        strength=1.0,
    )
    settings.statepoint = {"batches": list(range(1, batches + 1))}
    settings.output = {"summary": False}

    tallies = openmc.Tallies()
    whole = openmc.Tally(name="heating-whole-unfiltered")
    whole.scores = ["heating"]
    tallies.append(whole)

    parts = openmc.Tally(name="heating-by-material")
    parts.filters = [openmc.MaterialFilter([fe_inner, fe_shell])]
    parts.scores = ["heating"]
    tallies.append(parts)

    # Current is particle weight crossing the vacuum boundary. The energy
    # function multiplies it by incident E (eV), yielding escaping eV/source.
    escape = openmc.Tally(name="escaping-photon-energy")
    escape.filters = [
        openmc.SurfaceFilter([outer]),
        openmc.ParticleFilter(["photon"]),
        openmc.EnergyFunctionFilter([0.0, 2.0e6], [0.0, 2.0e6]),
    ]
    escape.scores = ["current"]
    tallies.append(escape)
    return openmc.Model(geometry=geometry, materials=materials, settings=settings, tallies=tallies)


def _audit_inputs(repo: Path, cross_sections: Path):
    audit_path = repo / "references/openmc-library-audit.json"
    photon_path = repo / "references/photon-library-provenance.json"
    audit = json.loads(audit_path.read_text())
    photon = json.loads(photon_path.read_text())
    index_hash = sha256(cross_sections)
    if index_hash != audit["cross_sections_xml"]["sha256"]:
        raise RuntimeError("cross_sections.xml differs from the selected audited combined index")
    photon_files = {item["relative_path"]: item for item in photon["photon_data"]["files"]}
    fe = photon_files.get("photon/Fe.h5")
    if not fe or sha256(cross_sections.parent / "photon/Fe.h5") != fe["sha256"]:
        raise RuntimeError("Fe photoatomic/relaxation data differs from the provenance audit")
    fe56 = audit["neutron_library"].get("Fe56")
    if not fe56 or sha256(cross_sections.parent / "neutron/Fe56.h5") != fe56["sha256"]:
        raise RuntimeError("Fe56 neutron HDF5 differs from the selected audited index")
    return {
        "cross_sections_xml": {"path": str(cross_sections), "sha256": index_hash},
        "index_audit": {"path": str(audit_path), "sha256": sha256(audit_path)},
        "photon_provenance": {"path": str(photon_path), "sha256": sha256(photon_path)},
        "data_files": {
            "photon/Fe.h5": {"sha256": fe["sha256"], "bytes": fe["bytes"]},
            "neutron/Fe56.h5": {"sha256": fe56["sha256"], "bytes": fe56["size_bytes"]},
        },
    }


def _verify_fe_relaxation_data(openmc, cross_sections: Path):
    """Read Fe photon HDF5 with the pinned OpenMC API and require shell transitions."""
    data = openmc.data.IncidentPhoton.from_hdf5(cross_sections.parent / "photon/Fe.h5")
    relaxation = data.atomic_relaxation
    if relaxation is None or not relaxation.transitions:
        raise RuntimeError("audited Fe photon data has no atomic-relaxation transition tables")
    transition_count = sum(len(table) for table in relaxation.transitions.values())
    if transition_count <= 0:
        raise RuntimeError("audited Fe photon data has zero atomic-relaxation transitions")
    return {
        "default_behavior": "OpenMC 0.15.3 enables atomic relaxation when photon transport is on; this pinned Python API has no toggle",
        "loaded_with_openmc_data_api": True,
        "binding_subshells": len(relaxation.binding_energy),
        "transition_subshells": len(relaxation.transitions),
        "transition_count": transition_count,
    }


def _linked_library(executable: Path, library: Path):
    if os.environ.get("LD_PRELOAD") or os.environ.get("LD_AUDIT"):
        raise RuntimeError("clear LD_PRELOAD and LD_AUDIT before an exact-library run")
    exe = executable.resolve(strict=True)
    lib = library.resolve(strict=True)
    if exe.parent.parent != lib.parent.parent:
        raise RuntimeError("OpenMC executable and libopenmc are not from the same prefix")
    readelf = subprocess.run(["readelf", "-d", str(exe)], capture_output=True, text=True, check=True, timeout=10).stdout
    if "Shared library: [libopenmc.so]" not in readelf or "Library runpath:" in readelf or "Library rpath: [$ORIGIN/../lib]" not in readelf:
        raise RuntimeError("cannot establish the executable's exact $ORIGIN/../lib resolution")
    expected = (exe.parent / "../lib/libopenmc.so").resolve(strict=True)
    if expected != lib:
        raise RuntimeError(f"selected --capi-library is not the executable-linked library: {expected}")
    return exe, lib


def _extract_batch_tallies(out: Path, batches: int):
    import h5py
    cumulative = {"whole": [], "part_inner": [], "part_shell": [], "escape": []}
    for batch in range(1, batches + 1):
        matches = [p for p in out.glob("statepoint.*.h5") if p.name.split(".")[1].isdigit() and int(p.name.split(".")[1]) == batch]
        if len(matches) != 1:
            raise RuntimeError(f"expected exactly one statepoint for batch {batch}, found {len(matches)}")
        path = matches[0]
        with h5py.File(path, "r") as f:
            if int(f["current_batch"][()]) != batch or int(f["n_realizations"][()]) != batch:
                raise RuntimeError(f"unexpected statepoint batch counters in {path.name}")
            by_name = {}
            for key in f["tallies"]:
                group = f["tallies"][key]
                if key.startswith("tally "):
                    by_name[group["name"][()].decode()] = group
            whole = by_name["heating-whole-unfiltered"]["results"][()]
            parts = by_name["heating-by-material"]["results"][()]
            escape = by_name["escaping-photon-energy"]["results"][()]
            if whole.size != 2 or parts.size != 4 or escape.size != 2:
                raise RuntimeError("tally output shape changed; expected 1 whole, 2 material, 1 escape bins")
            cumulative["whole"].append(float(whole.ravel()[0]))
            # OpenMC material-filter bins retain the declared filter order.
            cumulative["part_inner"].append(float(parts.reshape(-1, 2)[0, 0]))
            cumulative["part_shell"].append(float(parts.reshape(-1, 2)[1, 0]))
            cumulative["escape"].append(float(escape.ravel()[0]))
    return {name: batch_values(vals) for name, vals in cumulative.items()}


def analyze_batches(columns):
    cov = covariance_matrix(columns)
    closure = [h + e - SOURCE_E_EV for h, e in zip(columns["whole"], columns["escape"])]
    component = [h - a - b for h, a, b in zip(columns["whole"], columns["part_inner"], columns["part_shell"])]
    delta_stats = sample_stats(closure)
    component_stats = sample_stats(component)
    # Independent calculation from batch covariance verifies same-batch terms
    # are retained in Var(H + L), rather than adding marginal variances alone.
    predicted_closure_variance = (cov["whole"]["whole"] + cov["escape"]["escape"]
                                  + 2.0 * cov["whole"]["escape"])
    observed_closure_variance = delta_stats["sample_sd"] ** 2
    if not math.isclose(predicted_closure_variance, observed_closure_variance, rel_tol=1e-10, abs_tol=1e-10):
        raise RuntimeError("same-batch covariance identity failed")
    max_component_residual = max(abs(x) for x in component)
    closure_limit = 5.0 * delta_stats["standard_error"] + 5_000.0
    passed = (abs(delta_stats["mean"]) <= closure_limit
              and delta_stats["standard_error"] <= 20_000.0
              and abs(delta_stats["mean"]) <= 25_000.0
              and max_component_residual <= 1.0e-5)
    return {
        "batch_count": len(closure),
        "tally_batch_means_eV_per_source": columns,
        "tally_mean_and_se_eV_per_source": {name: sample_stats(values) for name, values in columns.items()},
        "sample_covariance_matrix_batch_realizations": cov,
        "covariance_matrix_of_batch_means": {
            a: {b: cov[a][b] / len(closure) for b in columns} for a in columns
        },
        "closure_batch_values_eV_per_source": closure,
        "closure_mean_and_se_eV_per_source": delta_stats,
        "component_closure_batch_values_eV_per_source": component,
        "component_closure_mean_and_se_eV_per_source": component_stats,
        "component_closure_max_abs_batch_residual_eV_per_source": max_component_residual,
        "closure_variance_from_covariance_eV2": predicted_closure_variance,
        "closure_variance_from_batch_differences_eV2": observed_closure_variance,
        "predeclared_pass_rules": {
            "energy_closure": "abs(mean(H + escaping photon energy - 1 MeV)) <= 5*SE_same_batch + 5000 eV; SE <= 20000 eV; absolute mean residual <= 25000 eV",
            "whole_vs_material_partition": "max absolute batch residual (whole - inner - shell) <= 1e-5 eV/source",
            "covariance": "sample variance of H+escape equals Var(H)+Var(escape)+2 Cov(H,escape), rel 1e-10 / abs 1e-10",
        },
        "passed": passed,
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--repo-root", type=Path, default=PROJECT_ROOT)
    ap.add_argument("--cross-sections", type=Path,
                    default=PROJECT_ROOT / "data/raw/combined-fendl32-endfbvii1/cross_sections.xml")
    ap.add_argument("--output-dir", type=Path, required=True)
    ap.add_argument("--openmc-executable", type=Path)
    ap.add_argument("--capi-library", type=Path)
    ap.add_argument("--prepare-only", action="store_true", help="export XML and preflight; never invoke OpenMC")
    ap.add_argument("--batches", type=int, default=BATCHES)
    ap.add_argument("--particles", type=int, default=PARTICLES)
    args = ap.parse_args()
    if not (2 <= args.batches <= 40 and 1 <= args.particles <= 5000):
        raise SystemExit("bounded control permits 2..40 batches and 1..5000 histories/batch")
    repo = args.repo_root.resolve(strict=True)
    xslib = args.cross_sections.resolve(strict=True)
    out = args.output_dir.resolve()
    if out.exists():
        raise SystemExit("output directory must not already exist")
    if args.prepare_only:
        out.mkdir(parents=True)
    else:
        out.mkdir(parents=True)
    data_identity = _audit_inputs(repo, xslib)
    try:
        import openmc
    except ImportError as e:
        raise SystemExit("run with the selected OpenMC 0.15.3 Python environment") from e
    if openmc.__version__ != "0.15.3":
        raise SystemExit(f"requires OpenMC Python 0.15.3, found {openmc.__version__}")
    relaxation_identity = _verify_fe_relaxation_data(openmc, xslib)
    os.environ["OPENMC_CROSS_SECTIONS"] = str(xslib)
    openmc.config["cross_sections"] = str(xslib)
    model = make_openmc_model(openmc, args.batches, args.particles)
    model.export_to_xml(directory=str(out))
    receipt = {
        "schema": "faris-openmc-photon-energy-closure/v0.1",
        "status": "PREPARED_ONLY" if args.prepare_only else "RUNNING",
        "source": {"particle": "photon", "energy_eV": SOURCE_E_EV, "isotropic": True, "strength": 1.0},
        "geometry": "Fe56 homogeneous sphere, r=10 cm vacuum; inner r=2 cm material partition with identical Fe56 composition",
        "transport": {"photon_transport": True, "electron_treatment": "led", "atomic_relaxation": relaxation_identity},
        "histories_per_batch": args.particles,
        "batches": args.batches,
        "openmc_python": {"version": openmc.__version__, "executable": sys.executable, "sha256": sha256(Path(sys.executable).resolve())},
        "data_identity": data_identity,
        "script_sha256": sha256(Path(__file__).resolve()),
    }
    (out / "control-input.json").write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
    if args.prepare_only:
        print(json.dumps({"status": "PREPARED_ONLY", "output_dir": str(out)}, indent=2))
        return 0
    if args.openmc_executable is None or args.capi_library is None:
        raise SystemExit("--openmc-executable and --capi-library are required for a run")
    exe, lib = _linked_library(args.openmc_executable, args.capi_library)
    lib_hash = sha256(lib)
    driver = repo / "integrations/openmc/reactor_transport.py"
    env = dict(os.environ)
    env["OPENMC_CROSS_SECTIONS"] = str(xslib)
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    if env.get("OMP_NUM_THREADS") is None:
        env["OMP_NUM_THREADS"] = "1"
    cmd = [sys.executable, str(driver), "--capi-runner", "--library", str(lib),
           "--library-sha256", lib_hash, "--executable", str(exe), "--receipt", str(out / "loss-receipt.json")]
    proc = subprocess.run(cmd, cwd=out, env=env, text=True, capture_output=True, timeout=300)
    (out / "capi.stdout.log").write_text(proc.stdout)
    (out / "capi.stderr.log").write_text(proc.stderr)
    if proc.returncode:
        raise RuntimeError(f"C API driver exited {proc.returncode}; inspect capi logs")
    loss = json.loads((out / "loss-receipt.json").read_text())
    if loss["library_sha256"] != lib_hash or loss["lost_particle_events"] != 0 or loss["end_batch"] != args.batches:
        raise RuntimeError("OpenMC C API loss receipt does not show the exact requested zero-loss run")
    columns = _extract_batch_tallies(out, args.batches)
    result = analyze_batches(columns)
    receipt.update({
        "status": "PASS" if result["passed"] else "FAIL",
        "capi_runtime": {"executable": str(exe), "library": str(lib), "library_sha256": lib_hash, "loss_receipt": loss},
        "analysis": result,
    })
    (out / "control-result.json").write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"status": receipt["status"], "output_dir": str(out),
                      "closure_eV_per_source": result["closure_mean_and_se_eV_per_source"],
                      "component_max_residual_eV_per_source": result["component_closure_max_abs_batch_residual_eV_per_source"]}, indent=2))
    return 0 if result["passed"] else 2

if __name__ == "__main__":
    raise SystemExit(main())
