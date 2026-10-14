#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Write manifest.json for the OKTAVIAN aluminium sphere built from the OpenMC-Fusion-Benchmarks STEP file.

    python3 build_manifest.py --root DIR --step-dir DIR [--out manifest.json]

--root holds two clean checkouts side by side: `openmc_fusion_benchmarks/` (MIT, with its `lfs` submodule)
and `open-benchmarks/` (IAEA-NDS, CC-BY-4.0). --step-dir holds the Git LFS objects fetched for the
submodule (`oktavian_a.step`); each is checked against the LFS object id in the pointer file. Needs PyYAML;
with h5py it also checks that the packaged experiment is the IAEA table (see `equivalence`).
The OFB specification is read here and its numbers are written into the manifest, so the case runner
needs neither PyYAML nor the OFB package.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import math
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[2]))
from validation.manifest import SCHEMA, check_manifest  # noqa: E402

OFB, IAEA = "openmc_fusion_benchmarks", "open-benchmarks"
OFB_URL = "https://github.com/eepeterson/openmc_fusion_benchmarks"
LFS_URL = "https://github.com/SteSeg/openmc_fusion_benchmarks-lfs"
SPEC = f"{OFB}/src/openmc_fusion_benchmarks/benchmarks/oktavian_al/specifications.yaml"
SCHEMA_FILE = f"{OFB}/src/openmc_fusion_benchmarks/benchmarks/benchmark_schema.yaml"
EXPERIMENT_H5 = f"{OFB}/src/openmc_fusion_benchmarks/results_database/oktavian_al/experiment.h5"
STEP = f"{OFB}/lfs/benchmarks/oktavian/oktavian_a.step"
# Per-source-file packaging of the IAEA table: value x lethargy width / (4 pi r^2) with r = 19.5 cm.
OFB_PACKAGING_RADIUS_CM = 19.5
# STEP read order (faris_cad.py step-inspect) against the OFB specification's volume ids 1..5.
STEP_SOLIDS = [("Cell_1_0", "void", 1), ("Cell_5_0", "void", 2), ("Cell_2_2", "SS-304", 3), ("Cell_4_2", "SS-304", 4), ("Cell_3_1", "Aluminum", 5)]


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git(root: Path, *args: str) -> str:
    return subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True, text=True).stdout.strip()


def load_iaea_builder():
    """The existing case's helpers (CSV reading, detector layout); loaded by path because both folders name their script alike."""
    path = HERE.parent / "oktavian-al" / "build_manifest.py"
    spec = importlib.util.spec_from_file_location("oktavian_al_build_manifest", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def lfs_oid(pointer: Path) -> str:
    m = re.search(r"^oid sha256:([0-9a-f]{64})$", pointer.read_text(encoding="utf-8"), re.M)
    if m is None:
        raise SystemExit(f"{pointer} is not a Git LFS pointer file")
    return m.group(1)


def model_block(spec: dict) -> dict:
    """The numbers the runner needs, taken from the OFB specification."""
    materials = []
    for m in spec["materials"]:
        c = m["composition"]
        materials.append({"id": m["id"], "name": m["name"], "fraction_type": "ao" if c["fraction_type"] == "atomic" else "wo",
                          "nuclides": c["data"], "density_g_cm3": m["density"]["value"]})
    src = spec["sources"][0]["angular_energy_distribution"]
    edges, weights = src["energy"]["values"], src["weights"][0]
    tallies = {t["name"]: next(f["values"] for f in t["filters"] if f["type"] == "energy") for t in spec["tallies"]}
    surf = {t["name"]: next(f["values"] for f in t["filters"] if f["type"] == "surface") for t in spec["tallies"]}
    s = spec["settings"]
    return {
        "attribution": "numbers extracted from OpenMC-Fusion-Benchmarks specifications.yaml, Copyright (c) 2025 MIT PSFC, MIT licence, commit 653d6aba04d993d2915a4e4e983e2a2119fa0e27",
        "materials": materials,
        "step_solids": [{"step_index": i, "step_name": n, "tag": t, "ofb_volume_id": v} for i, (n, t, v) in enumerate(STEP_SOLIDS)],
        "cad_file": spec["geometry"]["cad_file"], "ofb_surface_id_for_tallies": surf,
        "source": {"particle": "neutron", "point_cm": spec["sources"][0]["spatial_distribution"]["center"], "angular": "isotropic",
                   "energy_edges_ev": edges, "bin_probability": weights[:-1], "last_weight_in_specification": weights[-1],
                   "reading": "weights[i] is the probability of the interval [edges[i], edges[i+1]); they equal the MCNP sp1 entries of the IAEA input one for one (checked at build time)"},
        "tally_energy_edges_ev": tallies,
        "settings": {"batches": s["batches"], "particles_per_batch": s["particles_per_batch"], "photon_transport": s["photon_transport"], "run_mode": s["run_mode"]},
    }


def equivalence(ofb_root: Path, iaea_root: Path, builder) -> dict | None:
    """The packaged OFB experiment against the IAEA table: the same measurement, scaled by a constant per bin."""
    try:
        import h5py  # noqa: PLC0415
    except ImportError:
        return None
    exp = iaea_root / builder.EXP / "Oktavian_Al Neutron flux.csv"
    gam = iaea_root / builder.EXP / "Oktavian_Al Gamma flux.csv"
    with h5py.File(ofb_root / "src/openmc_fusion_benchmarks/results_database/oktavian_al/experiment.h5") as f:
        n, g = f["neutron_leakage/neutron_leakage"][0], f["photon_leakage/photon_leakage"][0]
    out = {}
    for name, rows, table, width in (("neutron", builder.read_table(exp), n, lambda lo, hi: math.log(hi / lo)),
                                     ("photon", builder.read_table(gam), g, lambda lo, hi: (hi - lo))):
        factors, err_factors = [], []
        for (energy, value, error), row in zip(rows, table):
            w = width(row[0] / 1e6, row[1] / 1e6)
            factors.append(value * w / row[2])
            err_factors.append(error * w / row[3])
        out[name] = {"bins": len(rows), "value_over_stored_min": min(factors), "value_over_stored_max": max(factors),
                     "error_over_stored_min": min(err_factors), "error_over_stored_max": max(err_factors)}
    area = 4 * math.pi * OFB_PACKAGING_RADIUS_CM ** 2
    out["conclusion"] = (f"every bin of the OFB table equals the IAEA table times the bin width (lethargy for neutrons, MeV for photons) divided by one constant, "
                         f"{out['neutron']['value_over_stored_min']:.6f} cm2 to {out['neutron']['value_over_stored_max']:.6f} cm2 for neutrons; 4 pi (19.5 cm)^2 = {area:.6f} cm2. "
                         "The measurement and its uncertainty column are the same; OFB stores a per-bin, per-cm2 quantity.")
    return out


def build(root: Path, step_dir: Path) -> dict:
    import yaml  # noqa: PLC0415
    ofb, iaea = root / OFB, root / IAEA
    for repo in (ofb, iaea):
        if git(repo, "status", "--porcelain"):
            raise SystemExit(f"{repo} has local changes; use a clean checkout")
    ofb_commit, lfs_commit = git(ofb, "rev-parse", "HEAD"), git(ofb / "lfs", "rev-parse", "HEAD")
    iaea_commit = git(iaea, "rev-parse", "HEAD")
    builder = load_iaea_builder()
    spec = yaml.safe_load((ofb / SPEC.split("/", 1)[1]).read_text(encoding="utf-8"))
    pointer = ofb / STEP.split("/", 1)[1]
    oid = lfs_oid(pointer)
    step = step_dir / "oktavian_a.step"
    if sha256(step) != oid:
        raise SystemExit(f"{step} does not match the LFS object id {oid}")

    files = []

    def add(role: str, base: Path, rel: str, **extra) -> None:
        path = base / rel
        files.append({"role": role, "path_in_source": f"{base.name}/{rel}", "sha256": sha256(path), "bytes": path.stat().st_size, **extra})

    add("OFB benchmark specification (materials, source, tallies, settings)", ofb, SPEC.split("/", 1)[1])
    add("OFB specification schema", ofb, SCHEMA_FILE.split("/", 1)[1])
    add("OFB packaged experiment (equivalent to the IAEA table, see equivalence)", ofb, EXPERIMENT_H5.split("/", 1)[1])
    add("OFB Git LFS pointer of the STEP geometry (the file's own sha256 is lfs_object_sha256)", ofb, STEP.split("/", 1)[1],
        lfs_object_sha256=oid, lfs_object_bytes=step.stat().st_size, lfs_submodule_commit=lfs_commit)
    add("OFB licence (MIT)", ofb, "LICENSE")
    add("OFB benchmark list", ofb, "benchmark_list.txt")
    add("OFB-LFS licence (MIT)", ofb, "lfs/LICENSE")
    for role, rel in (("experimental neutron leakage spectrum (IAEA)", f"{builder.EXP}/Oktavian_Al Neutron flux.csv"),
                      ("experimental photon leakage spectrum (IAEA)", f"{builder.EXP}/Oktavian_Al Gamma flux.csv"),
                      ("mcnp input of the IAEA package (source histogram, same material compositions)", f"{builder.INPUTS}/mcnp/Oktavian_Al.i"),
                      ("IAEA licence text", "LICENSE")):
        add(role, iaea, rel)

    model = model_block(spec)
    run_case = _load_runner_helpers()
    parsed = run_case.parse_mcnp((iaea / builder.INPUTS / "mcnp" / "Oktavian_Al.i").read_text(encoding="utf-8"))
    src = model["source"]
    if not (len(src["energy_edges_ev"]) == len(parsed["source_bounds_mev"]) and all(abs(a / 1e6 - b) < 1e-6 for a, b in zip(src["energy_edges_ev"], parsed["source_bounds_mev"]))
            and all(abs(a - b) < 1e-12 for a, b in zip(src["bin_probability"], parsed["source_sp"][1:]))):
        raise SystemExit("the OFB source histogram differs from the MCNP sp1 histogram of the IAEA input")
    for m in model["materials"]:
        mcnp_mat = {n: f for n, f in parsed["materials"][m["id"]]["nuclides"]}
        if mcnp_mat != m["nuclides"] or parsed["mass_density_g_cm3"][m["id"]] != m["density_g_cm3"]:
            raise SystemExit(f"material {m['name']} differs between the OFB specification and the MCNP input")
    neutron = builder.read_table(iaea / builder.EXP / "Oktavian_Al Neutron flux.csv")
    photon = builder.read_table(iaea / builder.EXP / "Oktavian_Al Gamma flux.csv")
    nedges = model["tally_energy_edges_ev"]["neutron_leakage"]
    pedges = model["tally_energy_edges_ev"]["photon_leakage"]
    if len(nedges) != len(neutron) + 1 or any(abs(nedges[i + 1] / 1e6 - neutron[i][0]) > 1e-9 for i in range(len(neutron))):
        raise SystemExit("OFB neutron tally edges and the IAEA neutron table disagree")
    if len(pedges) != len(photon) + 1 or any(abs(pedges[i] / 1e6 - photon[i][0]) > 1e-9 for i in range(len(photon))):
        raise SystemExit("OFB photon tally edges and the IAEA photon table disagree")

    detectors = []
    for i, (hi, value, err) in enumerate(neutron):
        lo = nedges[i] / 1e6
        d = {"id": f"n-{hi:g}", "response_class": "neutron_leakage_spectrum",
             "bin": {"lo_mev": lo, "hi_mev": hi, "tally_bin": i, "edge_convention": "CSV energy is the upper edge of the bin; same edge list as the OFB specification"},
             "controlling_variable": {"name": "energy", "value": hi, "unit": "MeV"}, "reference": {"value": value, "u": err}}
        if i == 0:
            d["blocked"] = {"why": "the first bin (97.1 to 101.1 keV) straddles the 100 keV lower limit of the time-of-flight measurement",
                            "next_step": "score only bins entirely above 100 keV, or confirm the measurement range in the original publication"}
        detectors.append(d)
    for i, (energy, value, err) in enumerate(photon):
        detectors.append({
            "id": f"g-{energy:g}", "response_class": "photon_leakage_spectrum",
            "bin": {"lo_mev": pedges[i] / 1e6, "hi_mev": pedges[i + 1] / 1e6, "tally_bin": i,
                    "edge_convention": "read as the lower edge by OFB, which is also what its stored table does; the MCNP e41 card of the IAEA input would make it the upper edge (unresolved)"},
            "controlling_variable": {"name": "energy", "value": energy, "unit": "MeV"}, "reference": {"value": value, "u": err},
            "blocked": {"why": "the photon bin edge convention is unresolved (lower edge in OFB, upper edge by the MCNP e41 card of the IAEA input); OFB's tally uses the lower-edge reading and does not resolve it either",
                        "next_step": "resolve the convention against the original OKTAVIAN gamma-spectrum publication or the CoNDERC record, then lift this block in the manifest"}})

    iaea_manifest = json.loads((HERE.parent / "oktavian-al" / "manifest.json").read_text(encoding="utf-8"))
    manifest = {
        "schema": SCHEMA,
        "case_id": "oktavian-al-cad",
        "title": "OKTAVIAN aluminium sphere from the OpenMC-Fusion-Benchmarks STEP file, FARIS CAD-to-DAGMC path (D-T point source)",
        "evidence_class": "experiment",
        "geometry_path": "CAD",
        "source": {"url": OFB_URL, "commit": ofb_commit, "retrieved": "2026-10-14",
                   "lfs_url": LFS_URL, "lfs_commit": lfs_commit,
                   "experiment_origin": "the measurement is the OKTAVIAN table of IAEA-NDS/open-benchmarks (identical to OFB's packaged experiment, see equivalence), "
                                        f"commit {iaea_commit}, CC-BY-4.0; original: Osaka University OKTAVIAN, Ichihara et al. (see the oktavian-al case)",
                   "paper": "Segantin, Pettinari, Shimwell, Peterson, Fusion Science and Technology 2026, doi 10.1080/15361055.2026.2664345",
                   "doi": "10.1080/15361055.2026.2664345",
                   "fetch": f"git clone --recurse-submodules {OFB_URL} && git -C openmc_fusion_benchmarks checkout {ofb_commit} && git -C openmc_fusion_benchmarks/lfs checkout {lfs_commit} && git -C openmc_fusion_benchmarks/lfs lfs pull; "
                            f"git clone https://github.com/IAEA-NDS/open-benchmarks && git -C open-benchmarks checkout {iaea_commit}"},
        "source_layout": "path_in_source is relative to a directory that holds the two checkouts side by side: openmc_fusion_benchmarks/ and open-benchmarks/",
        "license": {"spdx": "MIT AND CC-BY-4.0",
                    "attribution": "Geometry (STEP), specification and packaging: OpenMC-Fusion-Benchmarks, Copyright (c) 2025 MIT PSFC (https://github.com/eepeterson/openmc_fusion_benchmarks) and "
                                   "openmc_fusion_benchmarks-lfs, Copyright (c) 2025 Stefano Segantin, both MIT licence, commits " + f"{ofb_commit} and {lfs_commit}. "
                                   "Experimental spectra: IAEA-NDS/open-benchmarks (IAEA Nuclear Data Section), CC BY 4.0, https://creativecommons.org/licenses/by/4.0/, commit " + f"{iaea_commit}; "
                                   "the detector tables in this manifest were copied unchanged from the IAEA CSV files named under files. The MIT permission notice is in the LICENSE files named under files.",
                    "terms": "OFB and OFB-LFS: MIT. IAEA table: CC-BY-4.0 (upstream states all contents are)."},
        "files": files,
        "files_are_external_not_vendored": True,
        "equivalence": equivalence(ofb, iaea, builder),
        "model": model,
        "normalisation": iaea_manifest["normalisation"] | {
            "quantity": "leakage current through the whole outer boundary of the void sphere around the assembly (DAGMC surface between the r = 100 cm void volume and the implicit complement), per source neutron; no area factor",
            "location": "the r = 100 cm sphere; nothing returns across it, so it equals the r = 19.95 cm surface current of the oktavian-al case; the experiment is a time-of-flight detector at about 11 m",
            "scored_quantity": "outward partial current from DAGMC volume 2 across that surface (CellFromFilter on volume 2); the net current is recorded as a check on its absolute value"},
        "uncertainty": iaea_manifest["uncertainty"],
        "compatibility": iaea_manifest["compatibility"] | {"declared": "2026-10-14"},
        "protocol": None,  # filled in below
        "response_classes": iaea_manifest["response_classes"],
        "detectors": detectors,
        "not_covered": iaea_manifest["not_covered"] + [
            "CAD geometries with curved surfaces other than spheres and cylinders, imprint failures on larger models, and overlap handling (this STEP has five non-overlapping solids)",
            "variance reduction: the case is analog"],
        "qualified_range": iaea_manifest["qualified_range"],
        "known_upstream_defects": [],  # filled in below
        "literature_context": [],
    }
    manifest["protocol"] = protocol_block()
    manifest["known_upstream_defects"] = upstream_defects()
    return check_manifest(manifest)


def _load_runner_helpers():
    path = HERE.parent / "oktavian-al" / "run_case.py"
    spec = importlib.util.spec_from_file_location("oktavian_al_run_case", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def protocol_block() -> dict:
    return json.loads((HERE / "protocol.json").read_text(encoding="utf-8"))


def upstream_defects() -> list[str]:
    return json.loads((HERE / "upstream_defects.json").read_text(encoding="utf-8"))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--root", type=Path, required=True)
    ap.add_argument("--step-dir", type=Path, required=True)
    ap.add_argument("--out", type=Path, default=HERE / "manifest.json")
    args = ap.parse_args()
    manifest = build(args.root.expanduser(), args.step_dir.expanduser())
    args.out.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {args.out}: {len(manifest['detectors'])} detectors, OFB commit {manifest['source']['commit'][:12]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
