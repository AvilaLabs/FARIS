#!/usr/bin/env python3
"""Independent analytic/OpenMC control for a one-group absorber sphere.

This is a mathematical transport/normalization control. Its synthetic cross
section is not evaluated nuclear data and has no physical material meaning.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import shutil
import sys
from pathlib import Path
from typing import Any

DEFAULT_RADIUS_CM = 10.0
DEFAULT_SIGMA_A_PER_CM = 0.3
DEFAULT_SEED = 123456789
DEFAULT_BATCHES = 100
DEFAULT_PARTICLES = 10000
DEFAULT_THREADS = 8
MAX_HISTORIES = 10_000_000
MAX_THREADS = 32
RULE = "|estimate-reference| <= 3*reported_standard_deviation + 1e-12 in native units; heuristic only"


def analytic(radius_cm: float, sigma_a_per_cm: float) -> dict[str, float]:
    if not math.isfinite(radius_cm) or radius_cm <= 0:
        raise ValueError("radius_cm must be finite and > 0")
    if not math.isfinite(sigma_a_per_cm) or sigma_a_per_cm <= 0:
        raise ValueError("sigma_a_per_cm must be finite and > 0")
    escape = math.exp(-sigma_a_per_cm * radius_cm)
    absorbed = -math.expm1(-sigma_a_per_cm * radius_cm)
    track_length_cm_per_source = absorbed / sigma_a_per_cm
    return {
        "escape_probability": escape,
        "absorption_probability": absorbed,
        "integrated_track_length_cm_per_source": track_length_cm_per_source,
    }


def write_record(path: Path, obj: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("x", encoding="utf-8") as stream:
        stream.write(json.dumps(obj, indent=2, sort_keys=True) + "\n")


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def build_openmc_inputs(out: Path, radius_cm: float, sigma_a_per_cm: float,
                        seed: int, batches: int, particles: int) -> dict[str, Any]:
    try:
        import numpy as np
        import openmc
    except ImportError as exc:
        raise RuntimeError("OpenMC Python API and NumPy are required to generate solver inputs") from exc

    groups = openmc.mgxs.EnergyGroups(group_edges=[0.0, 20.0e6])
    xs = openmc.XSdata("synthetic_absorber", groups, temperatures=[294.0])
    # Set expansion order before any setters inspect the cross-section shapes.
    xs.order = 0
    xs.set_total(np.array([sigma_a_per_cm]), temperature=294.0)
    xs.set_absorption(np.array([sigma_a_per_cm]), temperature=294.0)
    # Explicit P0 zero-scatter matrix sets the order and excludes implicit
    # scattering behavior from the synthetic material definition.
    xs.set_scatter_matrix(np.zeros((1, 1, 1)), temperature=294.0)
    library = openmc.MGXSLibrary(groups)
    library.add_xsdata(xs)
    mgxs_path = (out / "synthetic_mgxs.h5").resolve()
    library.export_to_hdf5(str(mgxs_path))

    material = openmc.Material(name="synthetic pure absorber")
    material.add_macroscopic("synthetic_absorber")
    materials = openmc.Materials([material])
    materials.cross_sections = str(mgxs_path)

    boundary = openmc.Sphere(r=radius_cm, boundary_type="vacuum")
    cell = openmc.Cell(name="absorbing sphere", fill=material, region=-boundary)
    geometry = openmc.Geometry(openmc.Universe(cells=[cell]))

    source = openmc.IndependentSource(
        space=openmc.stats.Point((0.0, 0.0, 0.0)),
        angle=openmc.stats.Isotropic(),
        energy=openmc.stats.Discrete([14.0e6], [1.0]),
        strength=1.0,
        particle="neutron",
    )
    settings = openmc.Settings()
    settings.run_mode = "fixed source"
    settings.energy_mode = "multi-group"
    settings.batches = batches
    settings.particles = particles
    settings.seed = seed
    settings.source = source
    settings.statepoint = {"batches": [batches]}

    cell_filter = openmc.CellFilter(cell)
    absorption_tally = openmc.Tally(name="analog absorption per source particle")
    absorption_tally.filters = [cell_filter]
    absorption_tally.scores = ["absorption"]
    absorption_tally.estimator = "analog"
    flux_tally = openmc.Tally(name="tracklength flux per source particle")
    flux_tally.filters = [cell_filter]
    flux_tally.scores = ["flux"]
    flux_tally.estimator = "tracklength"
    tallies = openmc.Tallies([absorption_tally, flux_tally])

    materials.export_to_xml(path=str(out / "materials.xml"))
    geometry.export_to_xml(path=str(out / "geometry.xml"))
    settings.export_to_xml(path=str(out / "settings.xml"))
    tallies.export_to_xml(path=str(out / "tallies.xml"))
    # The required MG library is explicitly identified by the Materials XML.
    inputs = [out / name for name in ("materials.xml", "geometry.xml", "settings.xml", "tallies.xml", "synthetic_mgxs.h5")]
    digest = hashlib.sha256()
    for item in inputs:
        digest.update(item.name.encode("utf-8") + b"\0" + item.read_bytes())
    return {
        "input_sha256": digest.hexdigest(),
        "mgxs_path": str(mgxs_path),
        "tallies": {
            "absorption": {"name": absorption_tally.name, "estimator": absorption_tally.estimator},
            "integrated_track_length": {"name": flux_tally.name, "estimator": flux_tally.estimator},
        },
        "cell_id": cell.id,
        "openmc_python_version": openmc.__version__,
    }


def run_openmc(out: Path, radius_cm: float, sigma_a_per_cm: float,
               seed: int, batches: int, particles: int,
               threads: int, openmc_executable: Path | None) -> dict[str, Any]:
    script_identity = {"path": str(Path(__file__).resolve()), "sha256": file_sha256(Path(__file__))}
    exact = analytic(radius_cm, sigma_a_per_cm)
    failure_base = {
        "status": "NOT_EVALUATED",
        "script_identity": script_identity,
        "exact": exact,
        "seed": seed,
        "batches": batches,
        "particles_per_batch": particles,
        "threads": threads,
    }
    try:
        import openmc
    except ImportError as exc:
        return {**failure_base, "solver_status": "NOT_AVAILABLE",
                "failure": f"OpenMC Python API unavailable: {type(exc).__name__}: {exc}"}

    if openmc_executable is not None:
        executable = str(openmc_executable.expanduser().resolve())
        if not Path(executable).is_file() or not os.access(executable, os.X_OK):
            return {**failure_base, "solver_status": "NOT_AVAILABLE",
                    "python_api_version": openmc.__version__,
                    "failure": f"OpenMC executable is not an executable file: {executable}"}
    else:
        executable = shutil.which("openmc")
        if executable is None:
            return {**failure_base, "solver_status": "NOT_AVAILABLE",
                    "python_api_version": openmc.__version__,
                    "failure": "OpenMC executable is unavailable"}
    executable_identity = {"path": executable, "sha256": file_sha256(Path(executable))}
    try:
        input_identity = build_openmc_inputs(out, radius_cm, sigma_a_per_cm, seed, batches, particles)
    except Exception as exc:
        return {**failure_base, "solver_status": "NOT_STARTED",
                "python_api_version": openmc.__version__,
                "openmc_executable": executable_identity,
                "failure": f"Input generation failed: {type(exc).__name__}: {exc}"}

    statepoint_path = out / f"statepoint.{batches}.h5"
    try:
        run_completed = False
        solver_version = None
        openmc.run(cwd=str(out), threads=threads, output=True, openmc_exec=executable,
                   path_input=str(out.resolve()))
        run_completed = True
        if not statepoint_path.is_file():
            raise RuntimeError(f"OpenMC exited without the expected statepoint: {statepoint_path.name}")
        with openmc.StatePoint(statepoint_path) as sp:
            solver_version = ".".join(str(int(part)) for part in sp.version)
            if solver_version != openmc.__version__:
                raise RuntimeError(f"OpenMC Python API {openmc.__version__} does not match statepoint solver {solver_version}")
            if sp.current_batch != batches:
                raise RuntimeError(f"Statepoint contains {sp.current_batch} batches; expected {batches}")
            absorption_tally = sp.get_tally(name="analog absorption per source particle")
            flux_tally = sp.get_tally(name="tracklength flux per source particle")
            leakage_bins = sp.global_tallies[sp.global_tallies["name"] == b"leakage"]
            if len(leakage_bins) != 1:
                raise RuntimeError("Expected one OpenMC global leakage tally")
            leakage = leakage_bins[0]
            if absorption_tally.mean.size != 1 or absorption_tally.std_dev.size != 1:
                raise RuntimeError("Unexpected analog absorption tally shape")
            if flux_tally.mean.size != 1 or flux_tally.std_dev.size != 1:
                raise RuntimeError("Unexpected track-length flux tally shape")
            abs_mean = float(absorption_tally.mean.ravel()[0])
            abs_sd = float(absorption_tally.std_dev.ravel()[0])
            flux_mean = float(flux_tally.mean.ravel()[0])
            flux_sd = float(flux_tally.std_dev.ravel()[0])
            leak_mean = float(leakage["mean"])
            leak_sd = float(leakage["std_dev"])
            estimate = {
                "absorption_probability": {"mean": abs_mean, "sd": abs_sd,
                    "unit": "reactions/source particle", "estimator": "analog"},
                "integrated_track_length_cm_per_source": {"mean": flux_mean, "sd": flux_sd,
                    "unit": "particle-cm/source particle", "estimator": "tracklength"},
                "escape_probability": {"mean": leak_mean, "sd": leak_sd,
                    "unit": "leaked source particles/source particle", "estimator": "global leakage"},
            }
            for name, response in estimate.items():
                if not math.isfinite(response["mean"]) or response["mean"] < 0:
                    raise RuntimeError(f"{name} mean must be finite and nonnegative")
                if not math.isfinite(response["sd"]) or response["sd"] < 0:
                    raise RuntimeError(f"{name} standard deviation must be finite and nonnegative")
            if not all(math.isfinite(value) for value in exact.values()):
                raise RuntimeError("analytic reference contains a non-finite value")
    except Exception as exc:
        record = {
            **failure_base,
            "solver_status": "COMPLETED" if run_completed else "FAILED",
            "failure": f"{type(exc).__name__}: {exc}",
            "input_identity": input_identity,
            "python_api_version": openmc.__version__,
            "statepoint_solver_version": solver_version,
            "openmc_executable": executable_identity,
        }
        if statepoint_path.is_file():
            record["partial_statepoint"] = {
                "path": str(statepoint_path.resolve()),
                "sha256": file_sha256(statepoint_path),
            }
        return record
    comparisons = {}
    for key, value in estimate.items():
        ref = exact[key]
        delta = abs(value["mean"] - ref)
        limit = 3.0 * value["sd"] + 1e-12
        comparisons[key] = {"reference": ref, "absolute_difference": delta,
                            "acceptance_limit": limit, "within_heuristic": delta <= limit}
    balance_residual = (estimate["absorption_probability"]["mean"]
                        + estimate["escape_probability"]["mean"] - 1.0)
    particle_balance = {
        "absorption_plus_escape_per_source_particle":
            estimate["absorption_probability"]["mean"] + estimate["escape_probability"]["mean"],
        "residual_from_one": balance_residual,
        "within_roundoff": abs(balance_residual) <= 1e-12,
        "uncertainty": "Not applicable to this exact source-by-source balance identity; no covariance estimate is implied.",
    }
    passed = all(v["within_heuristic"] for v in comparisons.values()) and particle_balance["within_roundoff"]
    return {
        "status": "PASS" if passed else "FAIL",
        "solver_status": "COMPLETED",
        "solver": "OpenMC",
        "openmc_executable": executable_identity,
        "openmc_version": solver_version,
        "openmc_python_api_version": openmc.__version__,
        "data_class": "synthetic mathematical control; not physical nuclear data",
        "input_identity": input_identity,
        "script_identity": script_identity,
        "analytic_reference": exact,
        "geometry": {"type": "centered point source in homogeneous vacuum-bounded sphere", "radius_cm": radius_cm},
        "cross_sections": {"absorption_per_cm": sigma_a_per_cm, "scatter_per_cm": 0.0, "groups": 1},
        "source": {"position_cm": [0, 0, 0], "direction": "isotropic", "energy_eV": 14.0e6, "strength": 1.0, "normalization": "per source particle; strength does not scale per-history tallies"},
        "execution": {"seed": seed, "batches": batches, "particles_per_batch": particles,
                      "nominal_histories": batches * particles, "threads": threads,
                      "statepoint": str(statepoint_path.resolve()),
                      "statepoint_sha256": file_sha256(statepoint_path)},
        "estimates": estimate,
        "comparisons": comparisons,
        "particle_balance": particle_balance,
        "acceptance_rule": RULE,
        "covariance": "Not exported or used. Absorption and flux use different estimators, and leakage is a separate global response; all share histories and may be correlated. Each response is judged separately; no combined uncertainty or exact confidence guarantee is claimed.",
    }


def positive_int(value: str) -> int:
    result = int(value)
    if result <= 0:
        raise argparse.ArgumentTypeError("must be a positive integer")
    return result


def batch_count(value: str) -> int:
    result = positive_int(value)
    if result < 30:
        raise argparse.ArgumentTypeError("must be at least 30 to estimate batch uncertainty")
    return result


def thread_count(value: str) -> int:
    result = positive_int(value)
    if result > MAX_THREADS:
        raise argparse.ArgumentTypeError(f"must not exceed {MAX_THREADS}")
    return result


def validate_history_budget(batches: int, particles: int) -> None:
    if batches * particles > MAX_HISTORIES:
        raise ValueError(f"batches × particles may not exceed {MAX_HISTORIES:,} histories")


def positive_float(value: str) -> float:
    result = float(value)
    if not math.isfinite(result) or result <= 0:
        raise argparse.ArgumentTypeError("must be a finite positive number")
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--radius-cm", type=positive_float, default=DEFAULT_RADIUS_CM)
    parser.add_argument("--sigma-a-per-cm", type=positive_float, default=DEFAULT_SIGMA_A_PER_CM)
    parser.add_argument("--seed", type=positive_int, default=DEFAULT_SEED)
    parser.add_argument("--batches", type=batch_count, default=DEFAULT_BATCHES)
    parser.add_argument("--particles", type=positive_int, default=DEFAULT_PARTICLES, help="particles per batch")
    parser.add_argument("--threads", type=thread_count, default=DEFAULT_THREADS,
                        help=f"OpenMP threads (maximum {MAX_THREADS})")
    parser.add_argument("--out", type=Path, default=Path("pure-absorber-control"))
    parser.add_argument("--openmc-executable", type=Path,
                        help="explicit OpenMC binary path (the Python interpreter is selected when invoking this script)")
    parser.add_argument("--run", action="store_true", help="generate inputs and invoke OpenMC")
    args = parser.parse_args()
    try:
        validate_history_budget(args.batches, args.particles)
    except ValueError as exc:
        parser.error(str(exc))
    if args.out.exists():
        parser.error(f"output path already exists; refusing to overwrite: {args.out}")
    if args.run:
        try:
            args.out.mkdir(parents=True, exist_ok=False)
        except FileExistsError:
            parser.error(f"output path appeared during preflight; refusing to overwrite: {args.out}")
    exact = {
        "status": "NOT_EVALUATED",
        "control_kind": "independent analytic reference",
        "solver_status": "NOT_RUN",
        "scientific_scope": "synthetic one-group absorber sphere; mathematical solver/normalization test only",
        "geometry": {"type": "centered point source in homogeneous vacuum-bounded sphere", "radius_cm": args.radius_cm},
        "cross_sections": {"absorption_per_cm": args.sigma_a_per_cm, "scatter_per_cm": 0.0, "groups": 1},
        "source": {"position_cm": [0, 0, 0], "direction": "isotropic", "energy_eV": 14.0e6,
                   "strength": 1.0, "normalization": "per source particle"},
        "exact": analytic(args.radius_cm, args.sigma_a_per_cm),
        "identity": {"script_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest()},
    }
    if args.run:
        record = run_openmc(args.out, args.radius_cm, args.sigma_a_per_cm,
                            args.seed, args.batches, args.particles, args.threads,
                            args.openmc_executable)
    else:
        record = exact
    write_record(args.out / "control-result.json", record)
    print(json.dumps(record, indent=2, sort_keys=True))
    if args.run:
        if record.get("status") == "PASS":
            return 0
        if record.get("status") == "FAIL":
            return 1
        return 2
    return 0 if record.get("status") in ("NOT_EVALUATED", "PASS") else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (ValueError, RuntimeError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        raise SystemExit(2)
