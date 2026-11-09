#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Small live control for the optional MCNP imp:n=0 OpenMC patch.

Runs four one-batch, 1,000-history void controls with the exact selected CLI
and its linked shared library. This checks patch semantics only; it does not
qualify any FNS benchmark.
"""

from __future__ import annotations

import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "integrations/openmc"))
import reactor_transport as WORKER  # noqa: E402

HISTORIES = 1000
SEED = 271828
KILL_VERSION_SYMBOL = "faris_openmc_mcnp_imp0_patch_version"
STATEPOINT_FIELDS = (
    "faris_mcnp_imp0_patch_version",
    "faris_mcnp_imp0_kill_events",
    "faris_mcnp_imp0_birth_kill_events",
    "faris_mcnp_imp0_entry_kill_events",
    "faris_openmc_n_lost_particles",
)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def make_model(openmc, directory: Path, *, particle: str, source_in_zero_cell: bool,
               marked: bool) -> Path:
    inner = openmc.Sphere(r=1.0)
    middle = openmc.Sphere(r=1.5)
    outer = openmc.Sphere(r=2.0, boundary_type="vacuum")
    core = openmc.Cell(cell_id=1, name="live-source", region=-inner)
    live_shell = openmc.Cell(cell_id=2, name="live-shell", region=+inner & -middle)
    target = openmc.Cell(cell_id=3, name="optional-importance-zero", region=+middle & -outer)
    geometry = openmc.Geometry([core, live_shell, target])
    model = openmc.Model(geometry=geometry, materials=openmc.Materials())
    settings = model.settings
    settings.run_mode = "fixed source"
    settings.batches = 1
    settings.particles = HISTORIES
    settings.seed = SEED
    settings.statepoint = {"batches": [1]}
    settings.sourcepoint = {"write": False}
    settings.photon_transport = particle == "photon"
    settings.output = {"summary": False, "tallies": False}
    settings.max_lost_particles = 2_000_000_000
    settings.rel_max_lost_particles = 0.999999
    settings.max_write_lost_particles = 0
    point = (1.75, 0.0, 0.0) if source_in_zero_cell else (0.0, 0.0, 0.0)
    energy = 1.0e6 if particle == "photon" else 14.1e6
    settings.source = openmc.IndependentSource(
        space=openmc.stats.Point(point),
        angle=openmc.stats.Isotropic(),
        energy=openmc.stats.Discrete([energy], [1.0]),
        strength=1.0,
        particle=particle,
    )

    flux = openmc.Tally(name="cell-flux")
    flux.filters = [openmc.CellFilter([core, live_shell, target])]
    flux.scores = ["flux"]
    current = openmc.Tally(name="middle-surface-current")
    current.filters = [openmc.SurfaceFilter([middle])]
    current.scores = ["current"]
    model.tallies = openmc.Tallies([flux, current])
    model.export_to_xml(directory=str(directory))

    geometry_xml = directory / "geometry.xml"
    tree = ET.parse(geometry_xml)
    cells = [cell for cell in tree.getroot().findall("cell") if cell.get("id") == "3"]
    if marked:
        if len(cells) != 1 or cells[0].get("faris_neutron_importance") is not None:
            raise RuntimeError("could not bind the opt-in marker to exactly OpenMC cell 3")
        cells[0].set("faris_neutron_importance", "0")
        tree.write(geometry_xml, encoding="utf-8", xml_declaration=True)
    elif any(cell.get("faris_neutron_importance") is not None for cell in tree.getroot().findall("cell")):
        raise RuntimeError("unmarked control unexpectedly contains an importance marker")
    return geometry_xml


def run_case(openmc, executable: Path, root: Path, case: dict, env: dict[str, str]) -> dict:
    directory = root / case["name"]
    directory.mkdir()
    xs = directory / "cross_sections.xml"
    xs.write_text(
        '<?xml version="1.0"?>\n'
        '<cross_sections><library materials="H1" path="unused-stub.h5" type="neutron"/></cross_sections>\n',
        encoding="utf-8",
    )
    case_env = dict(env, OPENMC_CROSS_SECTIONS=str(xs))
    geometry_xml = make_model(
        openmc,
        directory,
        particle=case["particle"],
        source_in_zero_cell=case["source_in_zero_cell"],
        marked=case["marked"],
    )
    geometry_sha256 = sha256(geometry_xml)
    capture = subprocess.run(
        [str(executable)], cwd=directory, env=case_env, capture_output=True,
        timeout=300, check=False,
    )
    (directory / "openmc.stdout.log").write_bytes(capture.stdout)
    (directory / "openmc.stderr.log").write_bytes(capture.stderr)
    if capture.returncode != 0:
        raise RuntimeError(f"OpenMC control {case['name']} exited {capture.returncode}; inspect its logs")

    statepoints = list(directory.glob("statepoint.*.h5"))
    if len(statepoints) != 1:
        raise RuntimeError(f"{case['name']} produced {len(statepoints)} statepoints, expected one")
    statepoint_path = statepoints[0]
    with openmc.StatePoint(statepoint_path, autolink=False) as statepoint:
        if int(statepoint.current_batch) != 1 or int(statepoint.n_realizations) != HISTORIES:
            raise RuntimeError(f"{case['name']} statepoint is incomplete")
        flux_means = [float(value) for value in statepoint.get_tally(name="cell-flux").mean.ravel()]
        current_mean = float(statepoint.get_tally(name="middle-surface-current").mean.ravel()[0])
        diagnostics = {
            name: int(statepoint._f[name][()])
            for name in STATEPOINT_FIELDS
        }
        # Retain OpenMC's actual version in addition to CLI/library hashes.
        solver_version = str(statepoint._f.attrs["openmc_version"])

    if diagnostics["faris_mcnp_imp0_patch_version"] != 1:
        raise RuntimeError("statepoint was not written by the required FARIS imp:n=0 patch")
    if diagnostics["faris_mcnp_imp0_kill_events"] != (
        diagnostics["faris_mcnp_imp0_birth_kill_events"] +
        diagnostics["faris_mcnp_imp0_entry_kill_events"]
    ):
        raise RuntimeError(f"{case['name']} kill counter subcounts do not reconcile")
    if diagnostics["faris_openmc_n_lost_particles"] != 0:
        raise RuntimeError(f"{case['name']} recorded a real lost particle")

    expected = {
        "entry_neutron": {"kill": HISTORIES, "birth": 0, "entry": HISTORIES,
                          "flux": [1.0, 0.5, 0.0], "current": 1.0},
        "source_birth_neutron": {"kill": HISTORIES, "birth": HISTORIES, "entry": 0,
                                   "flux": [0.0, 0.0, 0.0], "current": 0.0},
        "marked_photon": {"kill": 0, "birth": 0, "entry": 0,
                          "flux": [1.0, 0.5, 0.5], "current": 1.0},
        "unmarked_neutron": {"kill": 0, "birth": 0, "entry": 0,
                             "flux": [1.0, 0.5, 0.5], "current": 1.0},
    }[case["name"]]
    tolerance = 1.0e-10
    if diagnostics["faris_mcnp_imp0_kill_events"] != expected["kill"]:
        raise RuntimeError(f"{case['name']} kill count {diagnostics} != expected {expected['kill']}")
    if diagnostics["faris_mcnp_imp0_birth_kill_events"] != expected["birth"]:
        raise RuntimeError(f"{case['name']} birth-kill count is unexpected: {diagnostics}")
    if diagnostics["faris_mcnp_imp0_entry_kill_events"] != expected["entry"]:
        raise RuntimeError(f"{case['name']} entry-kill count is unexpected: {diagnostics}")
    if any(abs(actual - target) > tolerance for actual, target in zip(flux_means, expected["flux"])):
        raise RuntimeError(f"{case['name']} track-length flux means {flux_means} != {expected['flux']}")
    if abs(current_mean - expected["current"]) > tolerance:
        raise RuntimeError(f"{case['name']} surface-current mean {current_mean} != {expected['current']}")

    return {
        "case": case,
        "solver_version": solver_version,
        "geometry_xml_sha256": geometry_sha256,
        "statepoint": str(statepoint_path),
        "statepoint_sha256": sha256(statepoint_path),
        "diagnostics": diagnostics,
        "cell_flux_means_per_source": flux_means,
        "middle_surface_current_mean_per_source": current_mean,
        "expected": expected,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--openmc-executable", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    executable = args.openmc_executable.resolve(strict=True)
    python = Path(sys.executable).resolve(strict=True)
    output_dir = args.output_dir.expanduser().resolve(strict=False)
    if output_dir.exists():
        raise ValueError("output directory must be new; refusing to overwrite prior controls")

    env = os.environ.copy()
    env["OMP_NUM_THREADS"] = "1"
    env.setdefault("OPENBLAS_NUM_THREADS", "1")
    library, library_sha256 = WORKER.selected_openmc_library(executable, python, env)
    dll = ctypes.CDLL(str(library))
    try:
        patch_version = ctypes.c_int.in_dll(dll, KILL_VERSION_SYMBOL).value
    except ValueError as error:
        raise RuntimeError("selected CLI-linked library lacks the required imp:n=0 patch capability") from error
    if patch_version != 1:
        raise RuntimeError(f"unsupported FARIS imp:n=0 patch version: {patch_version}")

    import openmc
    if openmc.__version__ != "0.15.3":
        raise RuntimeError(f"control requires OpenMC Python 0.15.3; found {openmc.__version__}")

    output_dir.mkdir(parents=True)
    cases = [
        {"name": "entry_neutron", "particle": "neutron", "source_in_zero_cell": False, "marked": True},
        {"name": "source_birth_neutron", "particle": "neutron", "source_in_zero_cell": True, "marked": True},
        {"name": "marked_photon", "particle": "photon", "source_in_zero_cell": False, "marked": True},
        {"name": "unmarked_neutron", "particle": "neutron", "source_in_zero_cell": False, "marked": False},
    ]
    results = [run_case(openmc, executable, output_dir, case, env) for case in cases]
    report = {
        "schema": "faris.openmc-mcnp-imp0-control/1.0.0",
        "status": "PASS",
        "scope": "explicit neutron imp:n=0 source-birth and cell-entry semantics, statepoint-bound counters, incoming surface current, live-cell path, photon and unmarked controls only",
        "histories_per_case": HISTORIES,
        "batches": 1,
        "seed": SEED,
        "runtime": {
            "selected_cli_path": str(executable),
            "selected_cli_sha256": sha256(executable),
            "selected_linked_library_path": str(library),
            "selected_linked_library_sha256": library_sha256,
            "faris_patch_version_symbol": patch_version,
            "python_path": str(python),
            "python_sha256": sha256(python),
            "openmc_python_version": openmc.__version__,
        },
        "cases": results,
    }
    report_path = output_dir / "control.json"
    report_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps({"status": "PASS", "report": str(report_path)}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
