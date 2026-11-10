#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Coordinator-only analogue/weight-window control for a CSG void region.

This is a bounded NUC-056 control on the existing iron/hydrogen slab fixture.
It is not a whole-CAD void audit. It records exact runtime, inputs, window
coverage, statepoints and selected-library C API lost-particle receipts.
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
import time
from typing import Any

ROOT = Path(__file__).resolve().parents[3]
OPENMC_DIR = ROOT / "integrations/openmc"
RISK_TESTS_DIR = OPENMC_DIR / "risk_tests"
sys.path.insert(0, str(OPENMC_DIR))
sys.path.insert(0, str(RISK_TESTS_DIR))
import design_transport as DT  # noqa: E402
import openmc_ci  # noqa: E402
import reactor_transport as RT  # noqa: E402
import s0f_common  # noqa: E402
import test_windows_unbiased as U  # noqa: E402

SCHEMA = "faris.openmc-void-windows-control/1.0.0"
MAX_SECONDS_PER_RUN = 600
MAX_SPLIT_LIMIT = DT.MAX_SPLIT


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def canonical_receipt_digest(document: dict[str, Any]) -> str:
    body = {key: value for key, value in document.items() if key != "receipt_sha256"}
    canonical = json.dumps(body, sort_keys=True, separators=(",", ":"), allow_nan=False)
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


def audit_mesh_void_coverage(model, mesh) -> dict[str, Any]:
    """Prove each slab mesh voxel maps to a model cell and every explicit void is covered."""
    dimensions = tuple(int(value) for value in mesh.dimension)
    lower = tuple(float(value) for value in mesh.lower_left)
    upper = tuple(float(value) for value in mesh.upper_right)
    if dimensions != (65, 1, 1) or lower != (0.0, -50.0, -50.0) or upper != (65.0, 50.0, 50.0):
        raise ValueError("control mesh no longer exactly covers its declared 65 cm slab domain")
    cells = model.geometry.get_all_cells()
    void_ids = sorted(int(cell_id) for cell_id, cell in cells.items() if cell.fill is None)
    if void_ids != [12]:
        raise ValueError(f"control fixture expected exactly its declared detector void cell 12; found {void_ids}")
    surfaces = model.geometry.get_all_surfaces().values()
    x_surfaces = sorted(float(surface.x0) for surface in surfaces if hasattr(surface, "x0"))
    y_surfaces = sorted(float(surface.y0) for surface in surfaces if hasattr(surface, "y0"))
    z_surfaces = sorted(float(surface.z0) for surface in surfaces if hasattr(surface, "z0"))
    if x_surfaces != [0.0, 30.0, 60.0, 65.0] or y_surfaces != [-50.0, 50.0] or z_surfaces != [-50.0, 50.0]:
        raise ValueError("the control mesh no longer aligns with every slab and lateral geometry surface")
    for surface in surfaces:
        if hasattr(surface, "y0") or hasattr(surface, "z0"):
            if surface.boundary_type != "reflective":
                raise ValueError("the control slab side boundary is no longer reflective")
    expected_bins = {
        10: list(range(0, 30)),
        11: list(range(30, 60)),
        12: [60, 61, 62, 63, 64],
    }

    bins_by_cell: dict[int, set[int]] = {}
    sample_fractions = (0.125, 0.5, 0.875)
    for i in range(dimensions[0]):
        found: set[int] = set()
        for fx in sample_fractions:
            for fy in sample_fractions:
                for fz in sample_fractions:
                    point = (
                        lower[0] + (i + fx) * (upper[0] - lower[0]) / dimensions[0],
                        lower[1] + fy * (upper[1] - lower[1]),
                        lower[2] + fz * (upper[2] - lower[2]),
                    )
                    path = model.geometry.find(point)
                    if not path:
                        raise ValueError(f"mesh voxel {i} has an unowned sample point {point}")
                    found.add(int(path[-1].id))
        if len(found) != 1:
            raise ValueError(f"mesh voxel {i} crosses model cell boundaries: {sorted(found)}")
        bins_by_cell.setdefault(next(iter(found)), set()).add(i)
    actual_bins = {key: sorted(value) for key, value in bins_by_cell.items()}
    if actual_bins != expected_bins:
        raise ValueError(f"the coverage grid does not match all exact slab-cell spans: {actual_bins}")
    void_bins = {str(cell_id): sorted(bins_by_cell[cell_id]) for cell_id in void_ids}
    if void_bins != {"12": [60, 61, 62, 63, 64]}:
        raise ValueError(f"detector void mesh coverage changed unexpectedly: {void_bins}")
    return {
        "scope": "explicit CSG cells in the small slab fixture only",
        "mesh": {"dimension": list(dimensions), "lower_left_cm": list(lower), "upper_right_cm": list(upper)},
        "explicit_void_cell_ids": void_ids,
        "mesh_bins_by_cell_id": {str(key): value for key, value in sorted(actual_bins.items())},
        "void_mesh_bins": void_bins,
        "sample_points_per_voxel": len(sample_fractions) ** 3,
        "universe_implicit_complement_included": False,
    }


def audit_window_coverage(path: Path, coverage: dict[str, Any]) -> dict[str, Any]:
    import h5py
    import numpy as np

    with h5py.File(path, "r") as stream:
        weight_group = stream["weight_windows"][next(iter(stream["weight_windows"]))]
        lower = np.asarray(weight_group["lower_ww_bounds"][()], dtype=np.float64)
        upper = np.asarray(weight_group["upper_ww_bounds"][()], dtype=np.float64)
        max_split = int(weight_group["max_split"][()])
    if lower.shape != (2, 65) or upper.shape != lower.shape:
        raise ValueError(f"weight-window file has unexpected bound shape: {lower.shape}, {upper.shape}")
    if not np.isfinite(lower).all() or not np.isfinite(upper).all():
        raise ValueError("window bounds contain a non-finite value")
    active_groups = []
    inactive_groups = []
    for group in range(lower.shape[0]):
        active = lower[group] > 0.0
        if active.any():
            if not active.all() or np.any(upper[group] < lower[group]):
                raise ValueError(f"energy group {group} has an invalid or partially uncovered window map")
            active_groups.append(group)
        else:
            if np.any(upper[group] > 0.0):
                raise ValueError(f"inactive energy group {group} has positive upper bounds without lower bounds")
            inactive_groups.append(group)
    if not active_groups:
        raise ValueError("no energy group has usable windows")
    if max_split != MAX_SPLIT_LIMIT:
        raise ValueError(f"window split cap {max_split} differs from the configured cap {MAX_SPLIT_LIMIT}")
    active_ratio = upper[active_groups] / lower[active_groups]
    per_void = {}
    for cell_id, bins in coverage["void_mesh_bins"].items():
        indexes = np.asarray(bins, dtype=int)
        void_lower = lower[active_groups][:, indexes]
        void_upper = upper[active_groups][:, indexes]
        if not np.isfinite(void_lower).all() or not np.isfinite(void_upper).all():
            raise ValueError(f"void cell {cell_id} has non-finite windows")
        if np.any(void_lower <= 0.0) or np.any(void_upper < void_lower):
            raise ValueError(f"void cell {cell_id} has a missing or invalid window")
        per_void[cell_id] = {
            "mesh_bins": bins,
            "positive_finite_bounds_by_energy_group": True,
            "minimum_lower_bound": float(void_lower.min()),
            "maximum_upper_bound": float(void_upper.max()),
            "maximum_upper_lower_ratio": float((void_upper / void_lower).max()),
        }
    return {
        "sha256": sha256(path),
        "path": str(path),
        "dimensions": [65, 1, 1],
        "energy_groups": int(lower.shape[0]),
        "active_energy_group_indexes": active_groups,
        "unwindowed_energy_group_indexes": inactive_groups,
        "all_active_bins_positive_finite": True,
        "max_split": max_split,
        "maximum_upper_lower_ratio": float(active_ratio.max()),
        "void_cells": per_void,
    }


def validate_pass_receipt(receipt_path: Path, *, library: Path, library_sha256: str,
                          expected_batches: int) -> dict[str, Any]:
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    expected = {
        "schema_version": "faris-openmc-loss-pass/v0.2",
        "method": "openmc-c-api-n_lost_particles-delta/v1",
        "library_path": str(library.resolve()),
        "library_sha256": library_sha256,
        "start_batch": 0,
        "end_batch": expected_batches,
        "start_lost_particle_events": 0,
        "end_lost_particle_events": 0,
        "lost_particle_events": 0,
    }
    if receipt != expected:
        raise ValueError(f"solver pass receipt differs from the required exact run identity/counts: {receipt}")
    return receipt


def evaluate_comparison(analog_mean: float, analog_se: float, windowed_mean: float,
                        windowed_se: float) -> dict[str, Any]:
    values = (analog_mean, analog_se, windowed_mean, windowed_se)
    if any(not math.isfinite(value) or value <= 0.0 for value in values):
        raise ValueError("means and standard errors must be finite and positive")
    z = (windowed_mean - analog_mean) / math.hypot(analog_se, windowed_se)
    analog_rse = analog_se / analog_mean
    windowed_rse = windowed_se / windowed_mean
    return {
        "absolute_z_max": U.Z_LIMIT,
        "relative_standard_error_max": U.MAX_RELATIVE_ERROR,
        "analog_rse": analog_rse,
        "windowed_rse": windowed_rse,
        "z": z,
        "pass": abs(z) < U.Z_LIMIT and analog_rse < U.MAX_RELATIVE_ERROR and windowed_rse < U.MAX_RELATIVE_ERROR,
    }


def validate_seed_plan() -> dict[str, Any]:
    generation = int(DT.WINDOW_SEED)
    reserved_mgxs = int(DT.MGXS_SEED)
    analog = int(U.ANALOG_SEED)
    windowed = int(U.WINDOWED_SEED)
    if analog == windowed or analog in (generation, reserved_mgxs) or windowed in (generation, reserved_mgxs):
        raise ValueError("production seeds must differ from one another and all reserved window-generation seeds")
    if generation == reserved_mgxs:
        raise ValueError("window-generation seed reservations collide")
    return {
        "generation_seed": generation,
        "reserved_mgxs_seed": reserved_mgxs,
        "analog_seed": analog,
        "windowed_seed": windowed,
        "all_seed_labels_distinct": len({generation, reserved_mgxs, analog, windowed}) == 4,
        "stream_nonoverlap": "NOT_PROVEN",
        "sourcepoint_restart_inputs": "none; distinct fresh run directories",
    }


def run_pass(*, label: str, model, directory: Path, executable: Path, library: Path,
             library_sha256: str, env: dict[str, str], threads: int, expected_batches: int,
             expected_seed: int, window_mesh=None) -> dict[str, Any]:
    import openmc

    directory.mkdir()
    before = sorted(path.name for path in directory.iterdir())
    if before:
        raise ValueError(f"{label} run directory is not empty")
    model_xml = directory / "model.xml"
    DT.export_model(model, model_xml, window_mesh)
    xml_identity = {path.name: sha256(path) for path in sorted(directory.glob("*.xml"))}
    if not xml_identity:
        raise RuntimeError(f"{label} wrote no OpenMC model XML")
    run_env = dict(env)
    run_env["OMP_NUM_THREADS"] = str(threads)
    run_env["OPENMC_CROSS_SECTIONS"] = str(model.materials.cross_sections)
    receipt_path = directory / "loss-pass.json"
    if list(directory.glob("sourcepoint*")) or list(directory.glob("statepoint*")):
        raise ValueError(f"{label} has an unexpected sourcepoint or restart statepoint input")
    command = [
        sys.executable, str(OPENMC_DIR / "reactor_transport.py"), "--capi-runner",
        "--library", str(library), "--library-sha256", library_sha256,
        "--executable", str(executable), "--receipt", str(receipt_path),
    ]
    started = time.monotonic()
    try:
        completed = subprocess.run(
            command, cwd=directory, env=run_env, capture_output=True,
            timeout=MAX_SECONDS_PER_RUN, check=False,
        )
    except subprocess.TimeoutExpired as error:
        (directory / "solver.stdout.log").write_bytes(error.stdout or b"")
        (directory / "solver.stderr.log").write_bytes(error.stderr or b"")
        raise RuntimeError(f"{label} exceeded the {MAX_SECONDS_PER_RUN} second run bound") from error
    (directory / "solver.stdout.log").write_bytes(completed.stdout)
    (directory / "solver.stderr.log").write_bytes(completed.stderr)
    if completed.returncode != 0:
        raise RuntimeError(f"{label} OpenMC run exited {completed.returncode}; see its saved logs")
    if not receipt_path.is_file():
        raise RuntimeError(f"{label} OpenMC C API runner wrote no loss-count receipt")
    loss = validate_pass_receipt(receipt_path, library=library, library_sha256=library_sha256,
                                 expected_batches=expected_batches)
    statepoints = sorted(directory.glob("statepoint.*.h5"))
    if len(statepoints) != 1:
        raise RuntimeError(f"{label} wrote {len(statepoints)} statepoints; expected one final statepoint")
    with openmc.StatePoint(statepoints[0], autolink=False) as statepoint:
        if (int(statepoint.current_batch) != expected_batches or
                int(statepoint.n_batches) != expected_batches or
                int(statepoint.n_particles) != model.settings.particles or
                int(statepoint.n_realizations) != expected_batches or
                int(statepoint.seed) != expected_seed or
                tuple(statepoint.version) != (0, 15, 3) or statepoint.run_mode != "fixed source"):
            raise RuntimeError(f"{label} statepoint does not match the complete requested fixed-source run")
        tally = statepoint.get_tally(name="fast-flux-behind-the-slab")
        mean = float(tally.mean.ravel()[0])
        standard_error = float(tally.std_dev.ravel()[0])
        if not math.isfinite(mean) or not math.isfinite(standard_error) or mean <= 0.0 or standard_error <= 0.0:
            raise RuntimeError(f"{label} void flux mean/error is non-finite or non-positive")
        statepoint_record = {
            "path": str(statepoints[0]), "sha256": sha256(statepoints[0]),
            "current_batch": int(statepoint.current_batch), "n_batches": int(statepoint.n_batches),
            "n_particles": int(statepoint.n_particles), "n_realizations": int(statepoint.n_realizations),
            "seed": int(statepoint.seed), "version": [int(value) for value in statepoint.version],
            "run_mode": statepoint.run_mode,
            "void_flux_mean_per_source_neutron": mean,
            "void_flux_standard_error_per_source_neutron": standard_error,
        }
    if sorted(path.name for path in directory.glob("sourcepoint*")):
        raise RuntimeError(f"{label} unexpectedly wrote a sourcepoint file")
    return {
        "label": label,
        "sampling": {
            "batches": expected_batches,
            "particles_per_batch": int(model.settings.particles),
            "seed": expected_seed,
            "threads": threads,
            "source_strength": 1.0,
            "particle": "neutron",
        },
        "statepoint": statepoint_record,
        "statepoint_sha256": statepoint_record["sha256"],
        "model_xml_sha256": xml_identity,
        "loss_pass": loss,
        "loss_receipt_sha256": sha256(receipt_path),
        "solver_log_sha256": {
            "stdout": sha256(directory / "solver.stdout.log"),
            "stderr": sha256(directory / "solver.stderr.log"),
        },
        "run_seconds": time.monotonic() - started,
        "sourcepoint_or_restart_input": None,
    }


def create_case(openmc_module, library_path: Path, run: dict[str, int], seed: int):
    model = U.build_model(openmc_module, library_path, run, seed)
    model.settings.sourcepoint = {"write": False}
    return model


def main() -> int:
    import openmc

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--openmc-executable", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    if openmc.__version__ != "0.15.3":
        raise RuntimeError(f"requires OpenMC 0.15.3, found {openmc.__version__}")
    rng = validate_seed_plan()
    executable = args.openmc_executable.resolve(strict=True)
    python = Path(sys.executable).resolve(strict=True)
    expected_cli = (python.parent / "openmc").resolve(strict=True)
    if executable != expected_cli:
        raise RuntimeError("selected OpenMC CLI must be the executable beside this selected Python")
    output = args.output_dir.expanduser().resolve(strict=False)
    continuation_root = (ROOT / "runs/continuation").resolve()
    if not output.is_relative_to(continuation_root):
        raise ValueError("control output must remain under runs/continuation")
    if output.exists():
        raise ValueError(f"output directory must be new; refusing overwrite: {output}")
    output.parent.mkdir(parents=True, exist_ok=True)
    output.mkdir()

    env = os.environ.copy()
    env.pop("LD_PRELOAD", None)
    env.pop("LD_AUDIT", None)
    env["OMP_NUM_THREADS"] = "1"
    env.setdefault("OPENBLAS_NUM_THREADS", "1")
    library, library_sha256 = RT.selected_openmc_library(executable, python, env)
    data = openmc_ci.neutron_data_dir()
    data_hashes_before = {name: sha256(data / name) for name in openmc_ci.DATA_SHA256}
    xs_path = openmc_ci.cross_sections_xml(data, output)
    xs_sha256 = sha256(xs_path)
    env["OPENMC_CROSS_SECTIONS"] = str(xs_path)
    cli_sha256 = sha256(executable)
    python_sha256 = sha256(python)
    openmc_module = Path(openmc.__file__).resolve(strict=True)
    openmc_module_sha256 = sha256(openmc_module)
    source_paths = (
        Path(__file__).resolve(), Path(U.__file__).resolve(), Path(DT.__file__).resolve(),
        Path(RT.__file__).resolve(), Path(s0f_common.__file__).resolve(),
        Path(openmc_ci.__file__).resolve(),
    )
    source_files = {str(path): sha256(path) for path in source_paths}
    windows_dir = output / "window-generation"
    windows_dir.mkdir()
    filler_model = create_case(openmc, xs_path, U.GENERATION_RUN, DT.WINDOW_SEED)
    window_mesh = U.window_mesh(openmc)
    coverage = audit_mesh_void_coverage(filler_model, window_mesh)
    filler_model.tallies = openmc.Tallies()
    filler_model.settings.weight_window_generators = [openmc.WeightWindowGenerator(
        mesh=window_mesh,
        energy_bounds=U.ENERGY_BOUNDS,
        particle_type="neutron",
        method="magic",
        max_realizations=U.GENERATION_RUN["batches"],
        update_interval=1,
        on_the_fly=True,
    )]
    # Generation histories only produce this window file. Production runs use
    # fresh models/directories and have no restart/sourcepoint input.
    export_path = windows_dir / "model.xml"
    filler_model.export_to_model_xml(str(export_path))
    original_xml = {path.name: sha256(path) for path in windows_dir.glob("*.xml")}
    gen_receipt_path = windows_dir / "loss-pass.json"
    if list(windows_dir.glob("sourcepoint*")) or list(windows_dir.glob("statepoint*")):
        raise RuntimeError("window-generation directory has unexpected restart/sourcepoint input")
    started = time.monotonic()
    gen_command = [
        str(python), str(OPENMC_DIR / "reactor_transport.py"), "--capi-runner",
        "--library", str(library), "--library-sha256", library_sha256,
        "--executable", str(executable), "--receipt", str(gen_receipt_path),
    ]
    gen_env = dict(env, OPENMC_CROSS_SECTIONS=str(xs_path))
    try:
        generation = subprocess.run(
            gen_command, cwd=windows_dir, env=gen_env, capture_output=True,
            timeout=MAX_SECONDS_PER_RUN, check=False,
        )
    except subprocess.TimeoutExpired as error:
        (windows_dir / "solver.stdout.log").write_bytes(error.stdout or b"")
        (windows_dir / "solver.stderr.log").write_bytes(error.stderr or b"")
        raise RuntimeError(f"window generation exceeded {MAX_SECONDS_PER_RUN} seconds") from error
    (windows_dir / "solver.stdout.log").write_bytes(generation.stdout)
    (windows_dir / "solver.stderr.log").write_bytes(generation.stderr)
    if generation.returncode != 0:
        raise RuntimeError(f"window generation exited {generation.returncode}; see saved logs")
    generation_seconds = time.monotonic() - started
    generation_loss = validate_pass_receipt(
        gen_receipt_path, library=library, library_sha256=library_sha256,
        expected_batches=U.GENERATION_RUN["batches"],
    )
    generator_statepoints = sorted(windows_dir.glob("statepoint.*.h5"))
    generated_windows = sorted(windows_dir.glob("weight_windows*.h5"))
    if len(generator_statepoints) != 1 or len(generated_windows) != 1:
        raise RuntimeError("generation must produce exactly one final statepoint and one window file")
    generated_window = generated_windows[0]
    with openmc.StatePoint(generator_statepoints[0], autolink=False) as statepoint:
        if (int(statepoint.current_batch) != U.GENERATION_RUN["batches"] or
                int(statepoint.n_batches) != U.GENERATION_RUN["batches"] or
                int(statepoint.n_particles) != U.GENERATION_RUN["particles"] or
                int(statepoint.n_realizations) != U.GENERATION_RUN["batches"] or
                int(statepoint.seed) != DT.WINDOW_SEED or
                tuple(statepoint.version) != (0, 15, 3) or statepoint.run_mode != "fixed source"):
            raise RuntimeError("window-generation statepoint does not match the complete requested run")
        generation_statepoint_record = {
            "path": str(generator_statepoints[0]),
            "sha256": sha256(generator_statepoints[0]),
            "current_batch": int(statepoint.current_batch),
            "n_batches": int(statepoint.n_batches),
            "n_particles": int(statepoint.n_particles),
            "n_realizations": int(statepoint.n_realizations),
            "seed": int(statepoint.seed),
            "version": list(statepoint.version),
            "run_mode": statepoint.run_mode,
        }
    if list(windows_dir.glob("sourcepoint*")):
        raise RuntimeError("window generation unexpectedly wrote a sourcepoint file")
    filled_window = output / "filled.h5"
    fill_summary = s0f_common.fill_file(generated_window, filled_window)
    DT.label_windows(filled_window)
    window_audit = audit_window_coverage(filled_window, coverage)
    if not window_audit["all_active_bins_positive_finite"]:
        raise RuntimeError("filled window map does not cover its complete mesh")

    analog_model = create_case(openmc, xs_path, U.ANALOG_RUN, U.ANALOG_SEED)
    analog_record = run_pass(
        label="analogue", model=analog_model, directory=output / "analogue",
        executable=executable, library=library, library_sha256=library_sha256,
        env=env, threads=U.THREADS, expected_batches=U.ANALOG_RUN["batches"],
        expected_seed=U.ANALOG_SEED,
    )
    windowed_model = create_case(openmc, xs_path, U.WINDOWED_RUN, U.WINDOWED_SEED)
    applied_mesh = DT.apply_windows(openmc, windowed_model, filled_window, output / "windowed")
    windowed_record = run_pass(
        label="windowed", model=windowed_model, directory=output / "windowed",
        executable=executable, library=library, library_sha256=library_sha256,
        env=env, threads=U.THREADS, expected_batches=U.WINDOWED_RUN["batches"],
        expected_seed=U.WINDOWED_SEED,
        window_mesh=applied_mesh,
    )
    criteria = evaluate_comparison(
        analog_record["statepoint"]["void_flux_mean_per_source_neutron"],
        analog_record["statepoint"]["void_flux_standard_error_per_source_neutron"],
        windowed_record["statepoint"]["void_flux_mean_per_source_neutron"],
        windowed_record["statepoint"]["void_flux_standard_error_per_source_neutron"],
    )
    if not criteria["pass"]:
        raise RuntimeError(f"predeclared void-response comparison criteria failed: {criteria}")
    if (sha256(executable) != cli_sha256 or sha256(python) != python_sha256 or
            sha256(library) != library_sha256 or sha256(xs_path) != xs_sha256 or
            {name: sha256(data / name) for name in openmc_ci.DATA_SHA256} != data_hashes_before or
            sha256(openmc_module) != openmc_module_sha256 or
            {str(path): sha256(path) for path in source_paths} != source_files):
        raise RuntimeError("selected runtime, model code or input nuclear data changed during the control")
    report = {
        "schema": SCHEMA,
        "status": "PASS",
        "scope": "one explicit CSG detector void cell in the small slab fixture; not an actual CAD/DAGMC void qualification",
        "requirement_assessment": {
            "NUC-056": "supporting-control-pass-only; actual CAD/DAGMC void inventory remains unqualified",
            "NUC-053": "remains partial; RNG stream-level non-overlap is not proven",
        },
        "limitations": [
            "Seed disjointness and separate run directories show no sourcepoint/restart reuse; OpenMC RNG stream sequence non-overlap is not proven.",
            "This fixture does not enumerate CAD void solids, sampled cavities, DAGMC material-void cells, or DAGMC's implicit complement.",
            "A PASS applies only to this model, response, window map, runtime and inputs; it cannot set NUC-056 Met for full CAD models.",
        ],
        "next_step_for_full_NUC_056": "On the actual checked CAD import, bind GEO-039 void solids and GEO-022 cavity records to DAGMC volumes; include every DAGMC cell including any implicit-complement cell, audit its intersection with every applied-window mesh and record per-void coverage/results in the run receipt.",
        "criteria": criteria,
        "comparison_assumption": "Combined standard error is calculated for the separate analog and windowed production runs. No cross-run covariance or generator-map uncertainty is estimated.",
        "window_generation": {
            "method": "OpenMC MAGIC WeightWindowGenerator",
            "seed": DT.WINDOW_SEED,
            "sampling": U.GENERATION_RUN,
            "threads": 1,
            "generated_window_path": str(generated_window),
            "generated_window_sha256": sha256(generated_window),
            "filled_window_path": str(filled_window),
            "fill_summary": fill_summary,
            "audit": window_audit,
            "generation_statepoint": generation_statepoint_record,
            "generation_loss_pass": generation_loss,
            "generation_loss_receipt_sha256": sha256(gen_receipt_path),
            "generation_model_xml_sha256": original_xml,
            "generation_seconds": generation_seconds,
        },
        "void_inventory": coverage,
        "runs": {
            "analogue": analog_record,
            "windowed": windowed_record,
        },
        "rng": {
            **rng,
        },
        "runtime": {
            "openmc_version": openmc.__version__,
            "python_path": str(python),
            "python_sha256": python_sha256,
            "openmc_python_module_path": str(openmc_module),
            "openmc_python_module_sha256": openmc_module_sha256,
            "cli_path": str(executable),
            "cli_sha256": cli_sha256,
            "linked_library_path": str(library),
            "linked_library_sha256": library_sha256,
            "openmc_run_env": {key: env[key] for key in ("OMP_NUM_THREADS", "OPENBLAS_NUM_THREADS", "OPENMC_CROSS_SECTIONS") if key in env},
        },
        "nuclear_data": {
            "cross_sections_path": str(xs_path),
            "cross_sections_sha256": xs_sha256,
            "files": {name: {"path": str(data / name), "sha256": sha256(data / name)} for name in openmc_ci.DATA_SHA256},
        },
        "source_files": source_files,
        "input_parameters": {
            "energy_bounds_ev": U.ENERGY_BOUNDS,
            "mesh_dimension": [65, 1, 1],
            "response": "tracklength neutron flux above 0.1 MeV in explicit void cell 12",
            "source": {"position_cm": [0.001, 0.0, 0.0], "direction": [1.0, 0.0, 0.0], "energy_ev": 14.1e6, "strength": 1.0},
            "max_seconds_per_run": MAX_SECONDS_PER_RUN,
            "max_split": MAX_SPLIT_LIMIT,
        },
    }
    report["receipt_sha256"] = canonical_receipt_digest(report)
    path = output / "control.json"
    temporary = output / ".control.json.tmp"
    temporary.write_text(json.dumps(report, indent=2, sort_keys=True, allow_nan=False) + "\n", encoding="utf-8")
    temporary.replace(path)
    print(json.dumps({"status": report["status"], "report": str(path), "receipt_sha256": report["receipt_sha256"]}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
