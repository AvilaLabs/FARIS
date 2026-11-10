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
import hmac
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "integrations/openmc"))
from imp0_cross_sections import validate_cross_sections_index  # noqa: E402

HISTORIES = 1000
BATCHES = 1
SEED = 271828
OPENMC_VERSION = (0, 15, 3)
KILL_VERSION_SYMBOL = "faris_openmc_mcnp_imp0_patch_version"
STATEPOINT_FIELDS = (
    "faris_mcnp_imp0_patch_version",
    "faris_mcnp_imp0_kill_events",
    "faris_mcnp_imp0_birth_kill_events",
    "faris_mcnp_imp0_entry_kill_events",
    "faris_openmc_n_lost_particles",
)


def validate_statepoint_metadata(*, current_batch: int, n_batches: int,
                                 n_particles: int, n_realizations: int,
                                 version: tuple[int, ...]) -> None:
    """Require the pinned solver's complete one-batch control statepoint."""
    if tuple(version) != OPENMC_VERSION:
        raise RuntimeError(
            f"statepoint OpenMC version {tuple(version)} != required {OPENMC_VERSION}"
        )
    if current_batch != BATCHES or n_batches != BATCHES:
        raise RuntimeError(
            f"statepoint is incomplete: current_batch={current_batch}, "
            f"n_batches={n_batches}, expected {BATCHES}"
        )
    if n_particles != HISTORIES:
        raise RuntimeError(
            f"statepoint n_particles={n_particles}, expected {HISTORIES} histories per batch"
        )
    # In fixed-source mode OpenMC's global n_realizations counts completed
    # batches (see tally.cpp), not individual source particles.
    if n_realizations != BATCHES:
        raise RuntimeError(
            f"statepoint n_realizations={n_realizations}, expected {BATCHES} completed batch"
        )


def canonical_receipt_digest(document: dict) -> str:
    """Hash the canonical control record with its own digest field excluded."""
    body = {key: value for key, value in document.items() if key != "receipt_sha256"}
    encoded = json.dumps(
        body, sort_keys=True, separators=(",", ":"), allow_nan=False
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def verify_receipt_digest(document: dict) -> bool:
    supplied = document.get("receipt_sha256")
    return isinstance(supplied, str) and hmac.compare_digest(
        supplied, canonical_receipt_digest(document)
    )


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def verify_cli_library_binding(executable: Path, library: Path, env: dict[str, str]) -> str:
    """Require the explicit library to be the one this CLI resolves at runtime."""
    result = subprocess.run(
        ["ldd", str(executable)], env=env, check=False, text=True,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"cannot inspect selected OpenMC executable dependencies: {result.stdout[-2000:]}"
        )
    rows = [
        line for line in result.stdout.splitlines()
        if re.match(r"^\s*libopenmc\.so\s+=>", line)
    ]
    if len(rows) != 1:
        raise RuntimeError("selected OpenMC executable does not resolve exactly one libopenmc.so")
    match = re.fullmatch(r"\s*libopenmc\.so\s+=>\s+(\S+)\s+\(0x[0-9a-fA-F]+\)\s*", rows[0])
    if match is None or match.group(1) == "not":
        raise RuntimeError(f"selected OpenMC executable has an unresolved libopenmc.so: {rows[0]}")
    try:
        resolved = Path(match.group(1)).resolve(strict=True)
    except OSError as error:
        raise RuntimeError(
            f"selected OpenMC library path does not exist: {match.group(1)}"
        ) from error
    if resolved != library.resolve(strict=True):
        raise RuntimeError(
            f"explicit OpenMC library {library} differs from selected CLI dependency {resolved}"
        )
    return rows[0].strip()


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
    case_env = dict(env)
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
        validate_statepoint_metadata(
            current_batch=int(statepoint.current_batch),
            n_batches=int(statepoint.n_batches),
            n_particles=int(statepoint.n_particles),
            n_realizations=int(statepoint.n_realizations),
            version=tuple(statepoint.version),
        )
        statepoint_metadata = {
            "statepoint_current_batch": int(statepoint.current_batch),
            "statepoint_n_batches": int(statepoint.n_batches),
            "statepoint_n_particles": int(statepoint.n_particles),
            "statepoint_n_realizations": int(statepoint.n_realizations),
            "solver_version": ".".join(str(part) for part in statepoint.version),
        }
        flux_means = [float(value) for value in statepoint.get_tally(name="cell-flux").mean.ravel()]
        current_mean = float(statepoint.get_tally(name="middle-surface-current").mean.ravel()[0])
        diagnostics = {
            name: int(statepoint._f[name][()])
            for name in STATEPOINT_FIELDS
        }

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
        **statepoint_metadata,
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
    parser.add_argument("--openmc-library", type=Path, required=True,
                        help="exact shared library resolved by the selected CLI (checked with ldd)")
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--cross-sections", type=Path, required=True,
                        help="existing OpenMC 0.15.3 cross_sections.xml used by the control")
    args = parser.parse_args()
    executable = args.openmc_executable.resolve(strict=True)
    library = args.openmc_library.resolve(strict=True)
    python = Path(sys.executable).resolve(strict=True)
    output_dir = args.output_dir.expanduser().resolve(strict=False)
    cross_sections_identity = validate_cross_sections_index(args.cross_sections)
    cross_sections = Path(str(cross_sections_identity["path"]))
    cross_sections_sha256 = str(cross_sections_identity["sha256"])
    if output_dir.exists():
        raise ValueError("output directory must be new; refusing to overwrite prior controls")

    env = os.environ.copy()
    for name in ("LD_PRELOAD", "LD_AUDIT"):
        env.pop(name, None)
    loader_paths = [
        Path(item).resolve()
        for item in env.get("LD_LIBRARY_PATH", "").split(os.pathsep)
        if item
    ]
    if not loader_paths or loader_paths[0] != library.parent:
        raise RuntimeError(
            "sanitized LD_LIBRARY_PATH must select the explicit CLI-linked library directory first"
        )
    env["OMP_NUM_THREADS"] = "1"
    env.setdefault("OPENBLAS_NUM_THREADS", "1")
    cli_library_binding = verify_cli_library_binding(executable, library, env)
    library_sha256 = sha256(library)
    cli_sha256 = sha256(executable)
    python_sha256 = sha256(python)
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
    cross_sections_identity_before = cross_sections_identity.copy()
    env["OPENMC_CROSS_SECTIONS"] = str(cross_sections)
    results = [run_case(openmc, executable, output_dir, case, env) for case in cases]
    cross_sections_identity_after = validate_cross_sections_index(cross_sections)
    if (sha256(executable) != cli_sha256 or sha256(python) != python_sha256 or
            cross_sections_identity_after != cross_sections_identity_before):
        raise RuntimeError("selected CLI, Python, or cross-section index changed during the control")
    cli_library_binding_after = verify_cli_library_binding(executable, library, env)
    if (cli_library_binding_after != cli_library_binding or
            sha256(library) != library_sha256):
        raise RuntimeError("selected CLI-linked OpenMC library changed during the control")
    report = {
        "schema": "faris.openmc-mcnp-imp0-control/1.0.0",
        "status": "PASS",
        "scope": "explicit neutron imp:n=0 source-birth and cell-entry semantics, statepoint-bound counters, incoming surface current, live-cell path, photon and unmarked controls only",
        "histories_per_case": HISTORIES,
        "batches": BATCHES,
        "seed": SEED,
        "runtime": {
            "selected_cli_path": str(executable),
            "selected_cli_sha256": cli_sha256,
            "selected_linked_library_path": str(library),
            "selected_linked_library_sha256": library_sha256,
            "selected_cli_ldd_binding": cli_library_binding,
            "faris_patch_version_symbol": patch_version,
            "python_path": str(python),
            "python_sha256": python_sha256,
            "openmc_python_version": openmc.__version__,
            "openmc_python_module_path": str(Path(openmc.__file__).resolve(strict=True)),
            "openmc_python_module_sha256": sha256(Path(openmc.__file__).resolve(strict=True)),
            "cross_sections_path": str(cross_sections),
            "cross_sections_sha256": cross_sections_sha256,
            "cross_sections_catalogue": cross_sections_identity,
        },
        "cases": results,
    }
    report["receipt_sha256"] = canonical_receipt_digest(report)
    if not verify_receipt_digest(report):
        raise RuntimeError("internal error: control receipt digest did not self-verify")
    report_path = output_dir / "control.json"
    temporary_path = output_dir / ".control.json.tmp"
    temporary_path.write_text(
        json.dumps(report, indent=2, sort_keys=True, allow_nan=False) + "\n",
        encoding="utf-8",
    )
    temporary_path.replace(report_path)
    print(json.dumps({
        "status": "PASS",
        "report": str(report_path),
        "receipt_sha256": report["receipt_sha256"],
    }, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
