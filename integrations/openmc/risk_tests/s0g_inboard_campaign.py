#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Bounded NUC-050 campaign for fast-neutron flux in RM-M's inboard TF leg.

Protocol scope: neutron flux above 0.1 MeV, scored in TF coil material inside
|x|, |y| < 130 cm and |z| < 150 cm. The variance-reduction window file is made
by FARIS's current random-ray FW-CADIS generator. Its adjoint response uses the
homogenized TF-coil material as a whole (the current generator accepts cells or
materials, not a spatially clipped material); the production tally is restricted
to the exact inboard box. That distinction is recorded in every campaign.

Commands (run under the selected OpenMC Python environment):
  python s0g_inboard_campaign.py plan --model-dir MODEL_DIR --work-dir WORK
  timeout --signal=TERM --kill-after=10s 900s prlimit --as=6442450944 -- \
    python s0g_inboard_campaign.py prepare --model-dir MODEL_DIR --work-dir WORK
  python s0g_inboard_campaign.py run --model-dir MODEL_DIR --work-dir WORK
  python s0g_inboard_campaign.py analyze --work-dir WORK

`plan` inspects prerequisites and writes a frozen campaign-plan.json snapshot
when ready. Use a fresh work directory for each campaign. `prepare` generates windows. `run` executes bounded
analog/windowed replicates. No command silently builds or converts RM-M.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import importlib.metadata
import json
import math
import os
from pathlib import Path
import random
import resource
import re
import shutil
import signal
import subprocess
import sys
import time
import traceback

HERE = Path(__file__).resolve().parent
INTEGRATION = HERE.parent
REPO = INTEGRATION.parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(INTEGRATION))

import rm_m_openmc as RM  # noqa: E402
import rm_m_spec as SPEC  # noqa: E402
import window_generator as WG  # noqa: E402
import r3_dose_statistics as R3_STATS  # noqa: E402

SCHEMA = "faris-s0g-inboard-campaign/v1"
RUN_SCHEMA = "faris-s0g-inboard-run/v1"
ANALYSIS_SCHEMA = "faris-s0g-inboard-analysis/v1"
RESPONSE = {
    "name": "NUC-050 inboard TF fast flux",
    "quantity": "neutron flux",
    "energy_min_ev": 100_000.0,
    "energy_max_ev": 20_000_000.0,
    "region_cm": {"x": [-130.0, 130.0], "y": [-130.0, 130.0], "z": [-150.0, 150.0]},
    "solid_role": "toroidal-field coil",
}
DEFAULT_SEEDS = {
    "analog": [830050001, 830050002, 830050003, 830050004],
    "windows": [830050101, 830050102, 830050103, 830050104],
}
GENERATION_SEEDS = {WG.MGXS_SEED, WG.WINDOW_SEED}
DEFAULT_HISTORIES = {"analog": 27_000_000, "windows": 3_000_000}
DEFAULT_BATCHES = {"analog": 60, "windows": 60}
DEFAULT_THREADS = 4
DEFAULT_MEMORY_GB = 6.0
DEFAULT_PREP_BUDGET_S = 900.0
DEFAULT_TOTAL_BUDGET_S = 7_200.0
T_ONE_SIDED_975_DF3 = 3.182
BOOTSTRAP_SEED = 20261109
BOOTSTRAP_RESAMPLES = 2_000
NUC050_TARGET = 100.0
TALLY_ID = 1
WINDOW_SOURCE_CONFIGURATION = {"particle": "neutron", "energy_ev": WG.SOURCE_ENERGY_EV,
                               "spatial_distribution": "uniform volume in DAGMC plasma cell",
                               "strength": 1.0, "used_for": ["stochastic-slab MGXS", "random-ray FW-CADIS"]}
IMPLEMENTATION_FILES = (
    Path(__file__).resolve(), INTEGRATION / "window_generator.py", HERE / "rm_m_openmc.py",
    HERE / "rm_m_spec.py", HERE / "rm_m_checks.py", HERE / "r3_dose_statistics.py",
)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def canonical_hash(value: object) -> str:
    payload = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(payload).hexdigest()


def require_absent(path: Path, label: str) -> None:
    """Refuse to reuse any campaign output path; measurement artifacts are immutable."""
    path = Path(path)
    if os.path.lexists(path):
        raise FileExistsError(f"{label} already exists: {path}; start a fresh campaign work directory")


def write_json_new(path: Path, value: object) -> None:
    """Create a JSON artifact without replacing an earlier plan or measurement."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("x", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2)
        stream.write("\n")


def verify_implementation_hashes(plan: dict, repo_root: Path = REPO) -> None:
    """Require every imported FARIS implementation dependency to match the frozen plan."""
    expected = plan.get("implementation_hashes", {})
    required = {str(path.relative_to(REPO)) for path in IMPLEMENTATION_FILES}
    missing = sorted(required - set(expected))
    if missing:
        raise RuntimeError("frozen plan omits implementation hashes: " + ", ".join(missing))
    for relative in sorted(required):
        path = Path(repo_root) / relative
        if not path.is_file() or sha256(path) != expected[relative]:
            raise RuntimeError(f"campaign implementation changed after plan creation: {relative}")


def _scalar_batch_delta_collector():
    """Build callback state that differences the live C API's cumulative sums per batch."""
    batch_values = []
    previous_total = 0.0

    def on_batch(cumulative_sums, completed_batches):
        nonlocal previous_total
        if completed_batches != len(batch_values) + 1:
            raise ValueError("OpenMC C API batch callback skipped or repeated a completed batch")
        current_total = math.fsum(float(value) for value in cumulative_sums)
        if not math.isfinite(current_total):
            raise ValueError("OpenMC C API returned a non-finite cumulative tally")
        batch_values.append(current_total - previous_total)
        previous_total = current_total

    return batch_values, on_batch


def source_configuration() -> dict:
    return {"plasma_profile": RM.PLASMA_PROFILE, "major_radius_cm": SPEC.MAJOR_RADIUS_CM,
            "minor_radius_cm": SPEC.MINOR_RADIUS_CM, "elongation": SPEC.ELONGATION,
            "triangularity": SPEC.TRIANGULARITY, "source_strength_per_s": 1.0,
            "source_module": "openmc_plasma_source.tokamak_source via rm_m_openmc.tokamak_mesh_source"}


def _model_paths(model_dir: Path, stem: str) -> dict[str, Path]:
    return {
        "transport_h5m": model_dir / f"{stem}.h5m",
        "random_ray_h5m": model_dir / f"{stem}_rr.h5m",
        "roles": model_dir / f"{stem}_roles.json",
        "model_card": model_dir / f"{stem}_model_card.json",
    }


def _model_card_identity_reasons(card: dict | None, model_hashes: dict) -> list[str]:
    """Validate exact builder outputs and the required normal/RR geometry identity."""
    if not card:
        return []  # Missing card is reported by the ordinary prerequisite check.
    outputs = card.get("outputs", {})
    reasons = []
    for model_key, card_key, digest_key in (("transport_h5m", "h5m", "h5m_sha256"),
                                            ("random_ray_h5m", "h5m_random_ray", "h5m_random_ray_sha256")):
        expected = outputs.get(digest_key)
        observed = model_hashes.get(model_key)
        if not expected:
            reasons.append(f"model card is missing outputs.{digest_key}")
        elif observed and expected != observed:
            reasons.append(f"{model_key} does not match model-card {card_key} hash")
    if outputs.get("random_ray_geometry_identical") is not True:
        reasons.append("model card must assert outputs.random_ray_geometry_identical is true")
    return reasons


def _runtime_identity(executable: Path, path_executable: str | None, openmc_version: str,
                      python_library: Path, cli_library: Path, cli_version_output: str) -> tuple[dict, list[str]]:
    """Capture both OpenMC runtimes and reject PATH/version ambiguity before transport."""
    executable = Path(executable).resolve()
    python_library = Path(python_library).resolve()
    cli_library = Path(cli_library).resolve()
    reasons = []
    resolved_path_executable = str(Path(path_executable).resolve()) if path_executable else None
    if resolved_path_executable and resolved_path_executable != str(executable):
        reasons.append(f"PATH resolves openmc to {resolved_path_executable}, but campaign uses {executable}")
    match = re.search(r"OpenMC version\s+(\S+)", cli_version_output)
    cli_version = match.group(1) if match else None
    if cli_version != openmc_version:
        reasons.append(f"OpenMC CLI version {cli_version or 'unreported'} differs from Python package {openmc_version}")
    for label, path in (("Python shared library", python_library), ("CLI shared library", cli_library)):
        if not path.is_file():
            reasons.append(f"{label} is missing: {path}")
    identity = {
        "openmc_version": openmc_version,
        "openmc_cli_version": cli_version,
        "openmc_executable": str(executable),
        "openmc_executable_sha256": sha256(executable) if executable.is_file() else None,
        "path_openmc_executable": resolved_path_executable,
        "openmc_python_shared_library": str(python_library),
        "openmc_python_shared_library_sha256": sha256(python_library) if python_library.is_file() else None,
        "openmc_cli_shared_library": str(cli_library),
        "openmc_cli_shared_library_sha256": sha256(cli_library) if cli_library.is_file() else None,
        "python_cli_library_same_file": python_library == cli_library,
        "python_cli_library_same_sha256": (sha256(python_library) == sha256(cli_library)
                                             if python_library.is_file() and cli_library.is_file() else False),
    }
    return identity, reasons


def _discover_runtime_identity(openmc) -> tuple[dict, list[str]]:
    executable = Path(sys.executable).resolve().parent / "openmc"
    if not executable.is_file():
        return {}, [f"missing OpenMC executable beside Python: {executable}"]
    path_executable = shutil.which("openmc")
    try:
        version_result = subprocess.run([str(executable), "--version"], check=False, capture_output=True,
                                        text=True, timeout=10)
        version_output = version_result.stdout + version_result.stderr
        if version_result.returncode:
            version_output = ""
    except (OSError, subprocess.TimeoutExpired):
        version_output = ""
    try:
        linked = subprocess.run(["ldd", str(executable)], check=False, capture_output=True,
                                text=True, timeout=10)
        match = re.search(r"libopenmc\.so\s+=>\s+(\S+)", linked.stdout)
        cli_library = Path(match.group(1)) if linked.returncode == 0 and match else Path("/nonexistent/libopenmc.so")
    except (OSError, subprocess.TimeoutExpired):
        cli_library = Path("/nonexistent/libopenmc.so")
    python_library = Path(openmc.lib._dll._name)
    return _runtime_identity(executable, path_executable, openmc.__version__, python_library,
                             cli_library, version_output)


def data_manifest(xml_path: Path) -> dict:
    """Hash the cross-section index plus each unique neutron library file it names."""
    import xml.etree.ElementTree as ET

    xml_path = Path(xml_path).resolve()
    root = ET.parse(xml_path).getroot()
    files: dict[str, dict] = {}
    for entry in root.findall("library"):
        if entry.get("type") != "neutron":
            continue
        raw = entry.get("path")
        if not raw:
            continue
        path = Path(raw)
        if not path.is_absolute():
            path = xml_path.parent / path
        path = path.resolve()
        if path.is_file():
            files[str(path)] = {"sha256": sha256(path), "bytes": path.stat().st_size}
        else:
            files[str(path)] = {"missing": True}
    return {"cross_sections_xml": str(xml_path), "cross_sections_xml_sha256": sha256(xml_path),
            "neutron_library_files": files,
            "all_files_present": all("missing" not in item for item in files.values())}


def prerequisites(model_dir: Path, work_dir: Path, stem: str = "rm_m", cross_sections: Path | None = None) -> dict:
    model_dir, work_dir = Path(model_dir).expanduser().resolve(), Path(work_dir).expanduser().resolve()
    paths = _model_paths(model_dir, stem)
    xs = Path(cross_sections or RM.FARIS_XS).expanduser().resolve()
    try:
        openmc_package_version = importlib.metadata.version("openmc")
        openmc_rr_version = tuple(int(part) for part in openmc_package_version.split(".")[:2]) >= (0, 16)
    except importlib.metadata.PackageNotFoundError:
        openmc_package_version, openmc_rr_version = None, False
    checks = {
        "transport_h5m": paths["transport_h5m"].is_file(),
        "random_ray_h5m": paths["random_ray_h5m"].is_file(),
        "roles": paths["roles"].is_file(),
        "model_card": paths["model_card"].is_file(),
        "cross_sections_xml": xs.is_file(),
        "openmc_cli": (Path(sys.executable).resolve().parent / "openmc").is_file(),
        "openmc_python_package": importlib.util.find_spec("openmc") is not None,
        "openmc_random_ray_version": openmc_rr_version,
        "openmc_plasma_source": importlib.util.find_spec("openmc_plasma_source") is not None,
        "h5py": importlib.util.find_spec("h5py") is not None,
        "numpy": importlib.util.find_spec("numpy") is not None,
    }
    reasons = [f"missing {key}: {value}" for key, value in paths.items() if key in ("transport_h5m", "random_ray_h5m", "roles", "model_card") and not value.is_file()]
    if not xs.is_file():
        reasons.append(f"missing audited cross_sections.xml: {xs}")
    dependency_messages = {
        "openmc_cli": f"missing OpenMC executable beside Python: {Path(sys.executable).resolve().parent / 'openmc'}",
        "openmc_python_package": f"OpenMC Python package is unavailable in {sys.executable}",
        "openmc_random_ray_version": f"random-ray FW-CADIS needs OpenMC >= 0.16; found {openmc_package_version or 'not installed'}",
        "openmc_plasma_source": f"openmc-plasma-source is unavailable in {sys.executable}",
        "h5py": f"h5py is unavailable in {sys.executable}",
        "numpy": f"numpy is unavailable in {sys.executable}",
    }
    reasons.extend(dependency_messages[key] for key, okay in checks.items()
                   if key in dependency_messages and not okay)
    model_hashes = {key: sha256(path) for key, path in paths.items() if path.is_file()}
    card = json.loads(paths["model_card"].read_text()) if paths["model_card"].is_file() else None
    reasons.extend(_model_card_identity_reasons(card, model_hashes))
    runtime = None
    if checks["openmc_python_package"] and checks["openmc_cli"]:
        try:
            runtime, runtime_reasons = _discover_runtime_identity(_openmc_modules())
            reasons.extend(runtime_reasons)
        except (RuntimeError, OSError, ImportError) as exc:
            reasons.append(f"cannot verify OpenMC Python/CLI runtime identity: {exc}")
    try:
        data = data_manifest(xs) if xs.is_file() else None
    except (OSError, ValueError) as exc:
        data = None
        reasons.append(f"cannot inspect cross-section index: {exc}")
    if data and not data["all_files_present"]:
        reasons.append("one or more neutron library files named by cross_sections.xml are missing")
    identity = None
    if xs.is_file():
        try:
            identity = RM.library_identity(xs)
        except (OSError, ValueError, KeyError) as exc:
            reasons.append(f"cannot validate cross-section audit: {exc}")
        if identity and not identity["usable"]:
            reasons.append("cross_sections.xml does not match the audited FARIS library or lacks needed nuclides")
    return {"schema": SCHEMA, "model_dir": str(model_dir), "work_dir": str(work_dir), "stem": stem,
            "cross_sections_xml": str(xs), "checks": checks, "model_hashes": model_hashes,
            "openmc_package_version": openmc_package_version,
            "runtime_identity": runtime, "data": data, "library_identity": identity, "model_card": card,
            "ready": not reasons, "prerequisites": reasons}


def plan_record(preflight: dict, *, seeds: dict[str, list[int]] | None = None, histories: dict | None = None,
                batches: dict | None = None, threads: int = DEFAULT_THREADS,
                memory_gb: float = DEFAULT_MEMORY_GB, total_budget_s: float = DEFAULT_TOTAL_BUDGET_S) -> dict:
    seeds = {key: list(value) for key, value in (seeds if seeds is not None else DEFAULT_SEEDS).items()}
    histories = dict(histories if histories is not None else DEFAULT_HISTORIES)
    batches = dict(batches if batches is not None else DEFAULT_BATCHES)
    if set(seeds) != {"analog", "windows"} or any(len(seeds[c]) != 4 for c in ("analog", "windows")):
        raise ValueError("protocol requires four replicate seeds for each configuration")
    all_seeds = seeds["analog"] + seeds["windows"]
    if len(set(all_seeds)) != 8:
        raise ValueError("analog and windowed runs must use eight distinct production seeds")
    if set(all_seeds) & GENERATION_SEEDS:
        raise ValueError("replicate seeds must be distinct from the current generator's reserved seeds")
    if threads < 1 or memory_gb <= 0 or total_budget_s <= 0:
        raise ValueError("threads, memory budget and time budget must be positive")
    for config in ("analog", "windows"):
        if config not in histories or config not in batches or histories[config] <= 0 or batches[config] < 2:
            raise ValueError(f"invalid {config} history or batch count")
        if histories[config] % batches[config]:
            raise ValueError(f"{config} histories must divide evenly into batches")
    planned = {
        "analog": {"histories_per_seed": histories["analog"], "batches_per_seed": batches["analog"],
                   "histories_total": histories["analog"] * 4, "seeds": seeds["analog"]},
        "windows": {"histories_per_seed": histories["windows"], "batches_per_seed": batches["windows"],
                    "histories_total": histories["windows"] * 4, "seeds": seeds["windows"]},
    }
    # Estimates use S0g's one-seed 3M-history timings; deliberately conservative
    # linear extrapolation, plus 10% for startup and statepoint overhead.
    solver_estimate = (histories["analog"] / 3_000_000 * 114.0 + histories["windows"] / 3_000_000 * 92.0) * 4 * 1.1
    campaign_estimate = solver_estimate + DEFAULT_PREP_BUDGET_S
    response_hash = canonical_hash(RESPONSE)
    source_config = source_configuration()
    record = {"schema": SCHEMA, "protocol": "NUC-050 RM-M inboard fast-flux v1",
              "preflight": preflight, "response": RESPONSE, "response_sha256": response_hash,
              "source_configuration": source_config,
              "source_configuration_sha256": canonical_hash(source_config),
              "window_source_configuration": WINDOW_SOURCE_CONFIGURATION,
              "window_source_configuration_sha256": canonical_hash(WINDOW_SOURCE_CONFIGURATION),
              "window_generation": {"method": "current FARIS random-ray FW-CADIS", "energy_min_ev": RESPONSE["energy_min_ev"],
                  "mesh_cell_cm": WG.WINDOW_CELL_CM, "energy_group_edges_ev": WG.group_edges(RESPONSE["energy_min_ev"]),
                  "mgxs": WG.MGXS, "random_ray": WG.RANDOM_RAY,
                  "adjoint_domains": "all homogenized TF-coil material; broader than the exact scored inboard box because the current generator accepts material/cell domains",
                  "nucloses": ["the final tally is restricted to TF coil material intersected with the declared box and E>0.1 MeV",
                               "the adjoint response is broader than the final tally; recorded explicitly, not represented as an exact spatial adjoint"]},
              "implementation_hashes": {str(path.relative_to(REPO)): sha256(path)
                                         for path in IMPLEMENTATION_FILES},
              "sampling": {"seeds_by_configuration": seeds, "generation_seeds_reserved": sorted(GENERATION_SEEDS),
                  "all_production_seeds_disjoint": True,
                  "nucloses": ["seed values are disjoint across generation, analog and windowed runs",
                               "different seeds do not establish stream-level non-overlap and are not presented as a NUC-053 proof"],
                  "configurations": planned, "threads": threads, "memory_limit_bytes": int(memory_gb * 1024**3),
                  "memory_limit_enforced_by_child_rlimit_as": True, "total_job_budget_s": total_budget_s,
                  "prepare_budget_s": DEFAULT_PREP_BUDGET_S,
                  "prepare_budget_enforcement": "run prepare under the documented GNU timeout + prlimit wrapper; random-ray/MGXS helpers are shared code without internal timeouts",
                  "estimated_solver_seconds": solver_estimate, "estimated_total_seconds": campaign_estimate,
                  "estimate_basis": "S0g pilot: analog 3M/114s; windowed 3M/92s; 10% transport overhead; plus a 15-minute MGXS/FW-CADIS preparation cap",
                  "estimated_campaign_fits_budget": campaign_estimate <= total_budget_s,
                  "fom_definition": "1 / (relative_error^2 * wall_seconds)",
              "confidence_rule": "minimum of one-sided 97.5% Student-t lower bound over four paired per-seed FOM ratios and deterministic 2.5th-percentile batch bootstrap; Bonferroni allocation gives an overall one-sided 95% bound under the stated assumptions",
                  "bias_rule": "distinct analog/windowed production seeds; pooled configuration means agree within 3 standard errors combined in quadrature (independent-run assumption)",
                  "vov_diagnostic": "report VOV per replicate, pooled per configuration, and maximum; VOV > 0.1 makes the result inconclusive"},
              "status": "ready" if preflight.get("ready") and campaign_estimate <= total_budget_s else "prerequisites-or-budget-blocked"}
    return record


def _openmc_modules():
    try:
        import openmc
        import openmc.lib
        import openmc.mgxs  # noqa: F401
    except ImportError as exc:
        raise RuntimeError(f"OpenMC Python package is unavailable in {sys.executable}: {exc}") from exc
    return openmc


def prepare_windows(preflight: dict, work_dir: Path, threads: int) -> dict:
    """Generate the campaign windows with the project random-ray FW-CADIS adapter."""
    target = Path(work_dir) / "windows"
    require_absent(target, "campaign windows directory")
    if not preflight["ready"]:
        raise RuntimeError("campaign prerequisites are missing: " + "; ".join(preflight["prerequisites"]))
    openmc = _openmc_modules()
    runtime_identity, runtime_reasons = _discover_runtime_identity(openmc)
    if runtime_reasons:
        raise RuntimeError("OpenMC runtime identity is invalid: " + "; ".join(runtime_reasons))
    version = tuple(int(part) for part in openmc.__version__.split(".")[:2])
    if version < (0, 16):
        raise RuntimeError(f"FARIS random-ray FW-CADIS preparation requires OpenMC >= 0.16; found {openmc.__version__}")
    model_dir = Path(preflight["model_dir"])
    stem = preflight["stem"]
    roles = RM.load_roles(model_dir, stem)
    role_ids = RM.role_ids(roles)
    rm_xs = Path(preflight["cross_sections_xml"])
    identity = RM.library_identity(rm_xs)
    if not identity["usable"]:
        raise RuntimeError("cross-section library failed FARIS audit: " + json.dumps(identity, sort_keys=True))
    RM.FARIS_XS = rm_xs
    preparation_started = time.monotonic()
    rr_path = _model_paths(model_dir, stem)["random_ray_h5m"]
    model, universe = RM.build_model(openmc, rr_path, roles, rr=True)
    tf_material = next((m for m in model.materials if m.name == "tf_coil"), None)
    if tf_material is None:
        raise RuntimeError("RM-M roles/material mapping contains no tf_coil material for FW-CADIS")
    plasma_cell = openmc.Cell(cell_id=role_ids["plasma"])
    source = WG.domain_source(openmc, plasma_cell)
    bounds = universe.bounding_box
    mesh_bounds = WG.window_grid(list(bounds.lower_left), list(bounds.upper_right), WG.WINDOW_CELL_CM)
    mg = WG.condense_multigroup(openmc, model, WG.group_edges(RESPONSE["energy_min_ev"]), source,
                                target / "mgxs", threads)
    # The MG model is mutated by condensation; rebuild from original materials for the adjoint solve.
    model, universe = RM.build_model(openmc, rr_path, roles, rr=True)
    tf_material = next(m for m in model.materials if m.name == "tf_coil")
    source = WG.domain_source(openmc, openmc.Cell(cell_id=role_ids["plasma"]))
    win = WG.solve_windows(openmc, model, universe, source, [tf_material], RESPONSE["energy_min_ev"], mesh_bounds,
                           Path(target / "mgxs" / "mgxs.h5"), target / "solve", threads)
    filled = Path(target / "solve" / "filled.h5")
    final = target / "filled.h5"
    final.parent.mkdir(parents=True, exist_ok=True)
    import shutil
    shutil.copyfile(filled, final)
    preparation_elapsed = time.monotonic() - preparation_started
    if preparation_elapsed > DEFAULT_PREP_BUDGET_S:
        raise TimeoutError(f"MGXS/FW-CADIS preparation exceeded {DEFAULT_PREP_BUDGET_S:.0f}s budget")
    record = {"schema": SCHEMA, "kind": "s0g-windows", "method": "FARIS random-ray FW-CADIS",
              "elapsed_seconds": preparation_elapsed, "budget_seconds": DEFAULT_PREP_BUDGET_S,
              "response": RESPONSE, "response_sha256": canonical_hash(RESPONSE),
              "adjoint_domain": "all tf_coil material", "model_hashes": preflight["model_hashes"],
              "cross_sections": preflight["data"], "library": identity, "openmc_version": openmc.__version__,
              "openmc_shared_library": str(Path(openmc.lib._dll._name).resolve()),
              "openmc_shared_library_sha256": sha256(Path(openmc.lib._dll._name)),
              "openmc_executable": str(Path(sys.executable).resolve().parent / "openmc"),
              "openmc_executable_sha256": sha256(Path(sys.executable).resolve().parent / "openmc"),
              "runtime_identity": runtime_identity,
              "source_configuration_sha256": canonical_hash(source_configuration()),
              "window_source_configuration": WINDOW_SOURCE_CONFIGURATION,
              "window_source_configuration_sha256": canonical_hash(WINDOW_SOURCE_CONFIGURATION),
              "generator_module_sha256": sha256(INTEGRATION / "window_generator.py"),
              "mgxs": mg, "windows": win, "weight_windows_path": str(final),
              "weight_windows_sha256": sha256(final), "window_bytes": final.stat().st_size}
    write_json_new(target / "campaign-windows.json", record)
    return record


def _run_one(openmc, model_dir: Path, run_dir: Path, stem: str, windows: Path | None,
             histories: int, batches: int, seed: int, replicate_index: int, threads: int, memory_limit_bytes: int,
             remaining_budget_s: float) -> dict:
    run_dir = Path(run_dir)
    runtime_identity, runtime_reasons = _discover_runtime_identity(openmc)
    if runtime_reasons:
        raise RuntimeError("OpenMC runtime identity is invalid: " + "; ".join(runtime_reasons))
    require_absent(run_dir / "model.xml", "solver model XML")
    require_absent(run_dir / "solver-record.json", "solver measurement record")
    roles = RM.load_roles(model_dir, stem)
    ids = RM.role_ids(roles)
    model, _ = RM.build_model(openmc, model_dir / f"{stem}.h5m", roles)
    source, unscaled = RM.tokamak_mesh_source(openmc, ids["plasma"], rate=1.0)
    settings = model.settings
    settings.run_mode = "fixed source"
    settings.batches = batches
    settings.particles = histories // batches
    settings.seed = seed
    settings.source = [source]
    settings.cutoff = {"energy_neutron": RESPONSE["energy_min_ev"]}
    settings.photon_transport = False
    # The C API live-batch observer applies the campaign's strict 1e-6 limit after every batch.
    # Keep OpenMC's coarse abort thresholds above this campaign bound so the observer can receipt it.
    settings.max_lost_particles = 2_000_000_000
    settings.rel_max_lost_particles = 0.5
    settings.statepoint = {"batches": []}
    settings.temperature = {"default": RM.DATA_TEMPERATURE_K, "method": "nearest", "tolerance": 1.0}
    settings.output = {"summary": False, "tallies": False}
    mesh = openmc.RegularMesh(mesh_id=9999)
    mesh.lower_left = [RESPONSE["region_cm"][a][0] for a in ("x", "y", "z")]
    mesh.upper_right = [RESPONSE["region_cm"][a][1] for a in ("x", "y", "z")]
    mesh.dimension = [1, 1, 1]
    tally = openmc.Tally(tally_id=TALLY_ID, name="nuc050_inboard_fast_flux")
    tally.filters = [openmc.CellFilter(ids["tf"]), openmc.MeshFilter(mesh),
                     openmc.EnergyFilter([RESPONSE["energy_min_ev"], RESPONSE["energy_max_ev"]])]
    tally.scores = ["flux"]
    tally.estimator = "tracklength"
    model.tallies = openmc.Tallies([tally])
    window_hash = None
    model_xml = run_dir / "model.xml"
    if windows is not None:
        if not Path(windows).is_file():
            raise RuntimeError(f"missing weight windows: {windows}")
        window_hash = sha256(Path(windows))
        window_mesh = WG.apply_windows(openmc, model, Path(windows), run_dir)
        WG.export_model(model, model_xml, window_mesh)
    else:
        model.export_to_model_xml(str(model_xml))
    model_xml_sha256 = sha256(model_xml)
    per_batch, on_batch = _scalar_batch_delta_collector()

    run_info, _, _ = R3_STATS.run_live_tally(openmc, model_xml, TALLY_ID, threads, batches,
                                             remaining_budget_s, on_batch, histories // batches)
    elapsed = float(run_info["elapsed_s"])
    actual_histories = int(run_info["histories"])
    actual_batches = int(run_info["batches"])
    terminal_status = run_info.get("solver_status")
    solver_complete = (type(terminal_status) is int and terminal_status == 1
                       and actual_batches == batches and actual_histories == histories
                       and run_info.get("stop_reason") == "batch_limit")
    solver_status = "COMPLETED" if solver_complete else "INCOMPLETE"
    if sha256(model_xml) != model_xml_sha256:
        raise RuntimeError("solver model XML changed during OpenMC C API execution")
    solver_model = run_info["integrity"].get("solver_model", {})
    if solver_model.get("path") != str(model_xml.resolve()) or solver_model.get("sha256") != model_xml_sha256:
        raise RuntimeError("OpenMC C API did not run the exact exported model XML")
    actual_library = run_info["integrity"].get("c_api_library", {})
    if (actual_library.get("path") != runtime_identity.get("openmc_python_shared_library")
            or actual_library.get("sha256") != runtime_identity.get("openmc_python_shared_library_sha256")):
        raise RuntimeError("OpenMC solver loaded a C API library different from the frozen Python library")
    loss = run_info["integrity"].get("lost_particle_assessment", {})
    if loss.get("status") == "PASS":
        solver_status = "COMPLETED" if solver_status == "COMPLETED" else solver_status
    elif loss.get("status") == "REJECTED":
        solver_status = "REJECTED_LOST_PARTICLES"
    else:
        solver_status = "NOT_EVALUATED_LOST_PARTICLES"
    if loss.get("status") == "REJECTED":
        # The helper stopped at the declared 1e-6 threshold; preserve the record for diagnosis.
        pass
    stats = RM.batch_statistics(per_batch) if len(per_batch) >= 2 else {
        "batches": len(per_batch), "mean": None, "std_error": None, "relative_error": None}
    deviations = [value - stats["mean"] for value in per_batch] if per_batch and stats["mean"] is not None else []
    sum2 = math.fsum(value * value for value in deviations)
    stats["vov"] = (math.fsum(value ** 4 for value in deviations) / (sum2 * sum2) - 1.0 / len(per_batch)
                     if sum2 else None)
    if stats["mean"] not in (None, 0.0):
        stats["variance_per_history"] = (sum((x - stats["mean"]) ** 2 for x in per_batch) / (len(per_batch) - 1)
                                         * (histories // batches) / (stats["mean"] ** 2)) if len(per_batch) > 1 else None
    relative_error = stats["relative_error"]
    fom = 1.0 / (relative_error ** 2 * elapsed) if relative_error and relative_error > 0 and elapsed > 0 else None
    model_inputs = {key: sha256(path) for key, path in _model_paths(model_dir, stem).items() if path.is_file()}
    exe = Path(sys.executable).resolve().parent / "openmc"
    return {"schema": RUN_SCHEMA, "configuration": "windows" if windows else "analog", "seed": seed,
            "replicate_index": replicate_index,
            "histories": actual_histories, "batches": actual_batches, "threads": threads, "elapsed_seconds": elapsed,
            "peak_child_rss_kb": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
            "memory_limit_bytes": memory_limit_bytes, "run_directory": str(run_dir.resolve()),
            "model_dir": str(model_dir), "model_stem": stem,
            "model_sha256": sha256(model_dir / f"{stem}.h5m"), "roles_sha256": sha256(model_dir / f"{stem}_roles.json"),
            "model_input_hashes": model_inputs,
            "solver_status": solver_status, "solver_model_xml": solver_model,
            "solver_terminal_status": terminal_status,
            "solver_model_xml_sha256": model_xml_sha256, "solver_integrity": run_info["integrity"],
            "solver_completed_batches": actual_batches, "solver_histories": actual_histories,
            "source_configuration_sha256": canonical_hash(source_configuration()),
            "response": RESPONSE, "response_sha256": canonical_hash(RESPONSE), "energy_min_ev": RESPONSE["energy_min_ev"],
            "source_strength_per_s": 1.0, "source_unscaled_strength_per_s": unscaled,
            "weight_windows": str(windows) if windows else None, "weight_windows_sha256": window_hash,
            "openmc_version": openmc.__version__, "openmc_executable": str(exe),
            "openmc_executable_sha256": runtime_identity.get("openmc_executable_sha256"),
            "openmc_shared_library": str(Path(openmc.lib._dll._name).resolve()),
            "openmc_shared_library_sha256": sha256(Path(openmc.lib._dll._name)),
            "runtime_identity": runtime_identity,
            "solver_c_api_library": run_info["integrity"]["c_api_library"],
            "solver_mode": "OpenMC C API in exact selected Python environment",
            "stats": stats, "fom": fom, "batch_values": per_batch,
            "solver_stop_reason": run_info.get("stop_reason"), "lost_particle_assessment": loss,
            "statepoint_retained": None}


def _execute_transport_child(request_path: Path) -> int:
    request = json.loads(Path(request_path).read_text())
    RM.FARIS_XS = Path(request["cross_sections_xml"])
    openmc = _openmc_modules()
    record = _run_one(openmc, Path(request["model_dir"]), Path(request["run_dir"]), request["stem"],
                      Path(request["windows_path"]) if request.get("windows_path") else None,
                      int(request["histories"]), int(request["batches"]), int(request["seed"]),
                      int(request["replicate_index"]), int(request["threads"]),
                      int(request["memory_limit_bytes"]), float(request["time_budget_s"]))
    target = Path(request["run_dir"]) / "solver-record.json"
    write_json_new(target, record)
    return 0


def _launch_transport_child(request: dict, run_dir: Path, timeout_s: float) -> dict:
    run_dir.mkdir(parents=True, exist_ok=False)
    request_path = run_dir / "solver-request.json"
    request_path.write_text(json.dumps(request, indent=2) + "\n")
    log_path = run_dir / "openmc-capi.log"

    def limit_memory():
        resource.setrlimit(resource.RLIMIT_AS, (int(request["memory_limit_bytes"]),
                                                int(request["memory_limit_bytes"])))

    command = [sys.executable, str(Path(__file__).resolve()), "transport-child", "--request", str(request_path)]
    env = dict(os.environ, OMP_NUM_THREADS=str(request["threads"]))
    with log_path.open("wb") as log:
        proc = subprocess.Popen(command, cwd=run_dir, env=env, stdout=log, stderr=subprocess.STDOUT,
                                start_new_session=True, preexec_fn=limit_memory)
        try:
            code = proc.wait(timeout=max(0.01, timeout_s))
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGKILL)
            proc.wait()
            raise TimeoutError(f"run {run_dir.name} exceeded remaining campaign time budget")
    if code != 0:
        tail = log_path.read_text(errors="replace")[-2500:]
        raise RuntimeError(f"OpenMC C API child exited {code} for {run_dir.name}:\n{tail}")
    result_path = run_dir / "solver-record.json"
    if not result_path.is_file():
        raise RuntimeError(f"OpenMC C API child wrote no result record for {run_dir.name}")
    return json.loads(result_path.read_text())


def run_campaign(plan: dict, work_dir: Path, model_dir: Path, stem: str) -> list[dict]:
    if plan.get("status") != "ready":
        raise RuntimeError("plan is blocked; resolve prerequisites and time budget first")
    runs_root = Path(work_dir) / "runs"
    require_absent(runs_root, "campaign runs directory")
    sample = plan["sampling"]
    for config in ("analog", "windows"):
        for seed in sample["seeds_by_configuration"][config]:
            require_absent(runs_root / f"{config}-seed-{seed}", "campaign run directory")
    windows_dir = Path(work_dir) / "windows"
    win_record_path = windows_dir / "campaign-windows.json"
    windows_path = windows_dir / "filled.h5"
    if not win_record_path.is_file() or not windows_path.is_file():
        raise RuntimeError("run requires prepare step: missing windows/campaign-windows.json or windows/filled.h5")
    win_record = json.loads(win_record_path.read_text())
    if sha256(windows_path) != win_record.get("weight_windows_sha256"):
        raise RuntimeError("weight windows do not match campaign-windows.json")
    if win_record.get("response_sha256") != plan.get("response_sha256"):
        raise RuntimeError("weight windows were generated for a different response")
    if win_record.get("model_hashes") != plan["preflight"]["model_hashes"]:
        raise RuntimeError("weight windows were generated from different RM-M inputs")
    if win_record.get("cross_sections") != plan["preflight"]["data"]:
        raise RuntimeError("weight windows were generated from different cross-section data")
    if win_record.get("source_configuration_sha256") != plan.get("source_configuration_sha256"):
        raise RuntimeError("weight windows were generated with a different plasma-source definition")
    if win_record.get("window_source_configuration_sha256") != plan.get("window_source_configuration_sha256"):
        raise RuntimeError("weight windows were generated with a different MGXS/adjoint source")
    verify_implementation_hashes(plan)
    expected_implementations = plan["implementation_hashes"]
    if win_record.get("generator_module_sha256") != expected_implementations.get("integrations/openmc/window_generator.py"):
        raise RuntimeError("weight windows were generated by a different window-generator implementation")
    if win_record.get("mgxs", {}).get("parameters") != plan["window_generation"]["mgxs"]:
        raise RuntimeError("multigroup-generation settings differ from the frozen plan")
    if win_record.get("windows", {}).get("parameters") != plan["window_generation"]["random_ray"]:
        raise RuntimeError("random-ray FW-CADIS settings differ from the frozen plan")
    openmc = _openmc_modules()
    runtime_identity, runtime_reasons = _discover_runtime_identity(openmc)
    if runtime_reasons:
        raise RuntimeError("OpenMC runtime identity is invalid: " + "; ".join(runtime_reasons))
    if runtime_identity != plan["preflight"].get("runtime_identity"):
        raise RuntimeError("transport OpenMC executable/shared-library identity differs from the frozen plan")
    if runtime_identity != win_record.get("runtime_identity"):
        raise RuntimeError("transport OpenMC executable/shared-library identity differs from the window-generation runtime")
    sample = plan["sampling"]
    prepare_seconds = float(win_record.get("elapsed_seconds", math.inf))
    if prepare_seconds > sample["prepare_budget_s"]:
        raise RuntimeError("window preparation exceeded the campaign's declared preparation budget")
    solver_budget = sample["total_job_budget_s"] - prepare_seconds
    if solver_budget <= 0:
        raise RuntimeError("window preparation exhausted the overall campaign time budget")
    deadline = time.monotonic() + solver_budget
    results = []
    for replicate_index in range(4):
        for config in ("analog", "windows"):
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("campaign time budget exhausted before all replicates completed")
            cfg = sample["configurations"][config]
            seed = sample["seeds_by_configuration"][config][replicate_index]
            path = Path(work_dir) / "runs" / f"{config}-seed-{seed}"
            record = _launch_transport_child({
                "model_dir": str(Path(model_dir).resolve()), "run_dir": str(path.resolve()), "stem": stem,
                "windows_path": str(windows_path.resolve()) if config == "windows" else None,
                "histories": cfg["histories_per_seed"], "batches": cfg["batches_per_seed"],
                "seed": seed, "replicate_index": replicate_index, "threads": sample["threads"],
                "memory_limit_bytes": sample["memory_limit_bytes"], "time_budget_s": remaining,
                "cross_sections_xml": plan["preflight"]["cross_sections_xml"],
            }, path, remaining)
            record["campaign_plan_sha256"] = canonical_hash(plan)
            record["windows_record_sha256"] = sha256(win_record_path)
            record["cross_sections"] = plan["preflight"]["data"]
            record["data_manifest_sha256"] = canonical_hash(record["cross_sections"])
            write_json_new(path / "record.json", record)
            results.append(record)
    return results


def analyze_records(records: list[dict], plan: dict) -> dict:
    verify_implementation_hashes(plan)
    by_config = {k: [] for k in ("analog", "windows")}
    seen_seeds = set()
    seen_replicates = {"analog": set(), "windows": set()}
    model_hashes = set()
    role_hashes = set()
    data_hashes = set()
    window_hashes = set()
    window_record_hashes = set()
    runtime_hashes = set()
    source_hashes = set()
    expected_plan_sha256 = canonical_hash(plan)
    expected_data_sha256 = canonical_hash(plan["preflight"]["data"])
    expected_model_identity = plan["preflight"]["model_hashes"]

    def close(observed, expected, label):
        if observed is None or expected is None:
            if observed is not expected:
                raise ValueError(f"record {label} contradicts recomputed batch statistics")
            return
        if not isinstance(observed, (int, float)) or not math.isfinite(float(observed)) or not math.isclose(
                float(observed), float(expected), rel_tol=1.0e-9, abs_tol=1.0e-12):
            raise ValueError(f"record {label} contradicts recomputed batch statistics")

    for record in records:
        if record.get("schema") != RUN_SCHEMA:
            raise ValueError("record has an unsupported S0g run schema")
        cfg, seed = record.get("configuration"), record.get("seed")
        replicate_index = record.get("replicate_index")
        if cfg not in by_config or seed in seen_seeds:
            raise ValueError("duplicate or invalid configuration/seed in S0g records")
        if cfg not in plan["sampling"]["configurations"]:
            raise ValueError("run configuration is absent from the frozen sampling plan")
        if replicate_index not in range(4) or replicate_index in seen_replicates.get(cfg, set()):
            raise ValueError("each configuration needs exactly one run for replicate indices 0 through 3")
        config_plan = plan["sampling"]["configurations"][cfg]
        histories = int(config_plan["histories_per_seed"])
        batches = int(config_plan["batches_per_seed"])
        if (record.get("histories") != histories or record.get("batches") != batches
                or record.get("threads") != plan["sampling"]["threads"]):
            raise ValueError(f"{cfg} run history/batch/thread counts differ from the frozen plan")
        elapsed = record.get("elapsed_seconds")
        if not isinstance(elapsed, (int, float)) or not math.isfinite(float(elapsed)) or elapsed <= 0:
            raise ValueError("run elapsed_seconds must be finite and positive")
        terminal_status = record.get("solver_terminal_status")
        if type(terminal_status) is not int or terminal_status != 1:
            raise ValueError("run terminal OpenMC status must be exactly 1 for a completed final batch")
        if (record.get("solver_status") != "COMPLETED" or record.get("solver_stop_reason") != "batch_limit"
                or record.get("solver_mode") != "OpenMC C API in exact selected Python environment"):
            raise ValueError("run lacks a successful complete OpenMC C API solver status")
        if record.get("solver_completed_batches") != batches or record.get("solver_histories") != histories:
            raise ValueError("run solver completion counts differ from the frozen plan")
        model_xml = record.get("solver_model_xml")
        model_xml_hash = record.get("solver_model_xml_sha256")
        if not isinstance(model_xml, dict) or not isinstance(model_xml.get("path"), str):
            raise ValueError("run is missing the exact OpenMC solver model XML identity")
        model_xml_path = Path(model_xml["path"]).resolve()
        expected_xml_path = (Path(record.get("run_directory", "")) / "model.xml").resolve()
        if model_xml_path != expected_xml_path or not model_xml_path.is_file():
            raise ValueError("solver model XML path differs from the run's retained model.xml")
        observed_xml_hash = sha256(model_xml_path)
        if (not isinstance(model_xml_hash, str) or model_xml.get("sha256") != model_xml_hash
                or observed_xml_hash != model_xml_hash):
            raise ValueError("solver model XML hash differs from the retained model.xml")
        integrity = record.get("solver_integrity", {})
        if integrity.get("solver_model") != model_xml:
            raise ValueError("solver integrity receipt does not bind the exact model XML")
        loss = record.get("lost_particle_assessment", {})
        lost = loss.get("lost_particles")
        if (loss.get("status") != "PASS" or not isinstance(lost, int) or lost < 0
                or loss.get("histories") != histories or loss.get("fraction_limit") != R3_STATS.LOST_FRACTION_LIMIT
                or lost > R3_STATS.LOST_FRACTION_LIMIT * histories):
            raise ValueError("NUC-050 requires a measured lost-particle count at or below 1e-6 of histories")
        if integrity.get("lost_particle_assessment") != loss:
            raise ValueError("solver integrity receipt does not bind the final lost-particle count")
        c_api_library = record.get("solver_c_api_library")
        expected_lib = {"path": plan["preflight"]["runtime_identity"].get("openmc_python_shared_library"),
                        "sha256": plan["preflight"]["runtime_identity"].get("openmc_python_shared_library_sha256")}
        if c_api_library != expected_lib or integrity.get("c_api_library") != c_api_library:
            raise ValueError("run solver shared-library identity differs from the frozen OpenMC Python C API library")

        batch_values = record.get("batch_values")
        if (not isinstance(batch_values, list) or len(batch_values) != batches
                or not all(isinstance(value, (int, float)) and math.isfinite(float(value)) for value in batch_values)):
            raise ValueError("run batch_values must contain one finite value for every planned completed batch")
        derived = RM.batch_statistics([float(value) for value in batch_values])
        deviations = [float(value) - derived["mean"] for value in batch_values]
        squared = math.fsum(value * value for value in deviations)
        vov = (math.fsum(value ** 4 for value in deviations) / (squared * squared) - 1.0 / batches
               if squared else None)
        derived["vov"] = vov
        derived_fom = (1.0 / (derived["relative_error"] ** 2 * float(elapsed))
                       if derived["relative_error"] is not None and derived["relative_error"] > 0 else None)
        supplied = record.get("stats")
        if not isinstance(supplied, dict):
            raise ValueError("run record is missing reported statistics")
        for key in ("mean", "std_error", "relative_error", "vov"):
            close(supplied.get(key), derived.get(key), f"stats.{key}")
        if supplied.get("batches") != batches:
            raise ValueError("record stats.batches contradicts the frozen batch count")
        close(record.get("fom"), derived_fom, "fom")
        if derived_fom is None or not math.isfinite(derived_fom) or derived_fom <= 0:
            raise ValueError("recomputed run FOM is not finite and positive")

        if record.get("response_sha256") != plan.get("response_sha256"):
            raise ValueError("run response differs from campaign plan")
        if record.get("source_configuration_sha256") != plan.get("source_configuration_sha256"):
            raise ValueError("run plasma-source identity differs from campaign plan")
        if record.get("campaign_plan_sha256") != expected_plan_sha256:
            raise ValueError("run record belongs to a different campaign plan")
        if record.get("model_input_hashes") != expected_model_identity:
            raise ValueError("run model inputs differ from the frozen, fully hashed RM-M plan")
        model_hashes.add(canonical_hash(record["model_input_hashes"]))
        role_hashes.add(record.get("roles_sha256"))
        if record.get("data_manifest_sha256") != expected_data_sha256:
            raise ValueError("run cross-section data differs from the frozen plan")
        data_hashes.add(record["data_manifest_sha256"])
        if cfg == "windows":
            window_hashes.add(record.get("weight_windows_sha256"))
            window_record_hashes.add(record.get("windows_record_sha256"))
        source_hashes.add(record.get("source_configuration_sha256"))
        runtime_identity = record.get("runtime_identity")
        if not isinstance(runtime_identity, dict) or not runtime_identity.get("openmc_executable_sha256"):
            raise ValueError("run record is missing its complete OpenMC executable/library identity")
        if runtime_identity != plan["preflight"].get("runtime_identity"):
            raise ValueError("run OpenMC runtime differs from the frozen plan")
        runtime_hashes.add(canonical_hash(runtime_identity))
        verified = dict(record)
        verified["_derived_stats"] = derived
        verified["_derived_fom"] = derived_fom
        by_config[cfg].append(verified)
        seen_seeds.add(seed)
        seen_replicates[cfg].add(replicate_index)
    if len(model_hashes) != 1 or canonical_hash(expected_model_identity) not in model_hashes or len(role_hashes) != 1 or None in role_hashes:
        raise ValueError("runs do not share one hashed RM-M geometry/role identity")
    if data_hashes and (len(data_hashes) != 1 or None in data_hashes):
        raise ValueError("runs do not share one hashed cross-section data identity")
    if len(window_hashes) != 1 or None in window_hashes:
        raise ValueError("windowed runs do not share the same hashed weight-window file")
    if len(window_record_hashes) != 1 or None in window_record_hashes:
        raise ValueError("windowed runs do not share the same hashed preparation record")
    if source_hashes != {plan["source_configuration_sha256"]}:
        raise ValueError("runs do not share the planned source identity")
    if len(runtime_hashes) != 1:
        raise ValueError("runs do not share one complete OpenMC runtime identity")
    if next(iter(runtime_hashes)) != canonical_hash(plan["preflight"].get("runtime_identity")):
        raise ValueError("run records do not match the frozen OpenMC runtime identity")
    expected_by_config = {cfg: set(seeds) for cfg, seeds in plan["sampling"]["seeds_by_configuration"].items()}
    for cfg, rows in by_config.items():
        if {row["seed"] for row in rows} != expected_by_config[cfg] or len(rows) != 4:
            raise ValueError(f"{cfg} records must contain exactly its four planned, disjoint seeds")
    if expected_by_config["analog"] & expected_by_config["windows"]:
        raise ValueError("analog and windowed configurations reuse a production seed")
    analog = {r["replicate_index"]: r for r in by_config["analog"]}
    windows = {r["replicate_index"]: r for r in by_config["windows"]}
    gains = []
    replicate_pairs = []
    for replicate_index in range(4):
        arow, wrow = analog[replicate_index], windows[replicate_index]
        fa, fw = arow["_derived_fom"], wrow["_derived_fom"]
        gains.append(fw / fa)
        replicate_pairs.append({"replicate_index": replicate_index, "analog_seed": arow["seed"],
                                "windowed_seed": wrow["seed"], "fom_gain": fw / fa})
    mean = sum(gains) / len(gains)
    variance = sum((g - mean) ** 2 for g in gains) / (len(gains) - 1)
    seed_lower = mean - T_ONE_SIDED_975_DF3 * math.sqrt(variance / len(gains))

    def pooled(rows):
        values = [float(value) for row in rows for value in row["batch_values"]]
        stats = RM.batch_statistics(values)
        total_weight = sum(r["histories"] for r in rows)
        seconds = sum(r["elapsed_seconds"] for r in rows)
        pooled_fom = 1.0 / (stats["relative_error"] ** 2 * seconds)
        deviations = [value - stats["mean"] for value in values]
        squared = sum(value * value for value in deviations)
        vov = sum(value ** 4 for value in deviations) / (squared * squared) - 1.0 / len(values) if squared else None
        vov_by_seed = {str(r["seed"]): r["_derived_stats"]["vov"] for r in rows}
        vov_max = max((v if v is not None else math.inf) for v in vov_by_seed.values())
        return {"mean": stats["mean"], "std_error": stats["std_error"], "histories": total_weight,
                "batches": len(values), "seconds": seconds,
                "vov_by_seed": vov_by_seed,
                "vov_max": vov_max, "vov_pooled": vov,
                "fom": pooled_fom}

    pa, pw = pooled(by_config["analog"]), pooled(by_config["windows"])
    combined_se = math.hypot(pa["std_error"], pw["std_error"])
    z = (pw["mean"] - pa["mean"]) / combined_se if combined_se else (0.0 if pw["mean"] == pa["mean"] else math.inf)
    unbiased = abs(z) <= 3.0
    tail_resolved = (pa["vov_pooled"] is not None and pw["vov_pooled"] is not None
                     and pa["vov_pooled"] <= 0.1 and pw["vov_pooled"] <= 0.1
                     and pa["vov_max"] <= 0.1 and pw["vov_max"] <= 0.1)
    gain = pw["fom"] / pa["fom"]
    rng = random.Random(BOOTSTRAP_SEED)
    analog_values = [value for row in by_config["analog"] for value in row["batch_values"]]
    window_values = [value for row in by_config["windows"] for value in row["batch_values"]]

    def bootstrap_fom(values, seconds):
        draw = [values[rng.randrange(len(values))] for _ in values]
        stats = RM.batch_statistics(draw)
        return (1.0 / (stats["relative_error"] ** 2 * seconds)
                if stats["relative_error"] and stats["relative_error"] > 0 else math.inf)

    boot_gains = []
    for _ in range(BOOTSTRAP_RESAMPLES):
        fa = bootstrap_fom(analog_values, pa["seconds"])
        fw = bootstrap_fom(window_values, pw["seconds"])
        if math.isfinite(fa) and fa > 0.0 and math.isfinite(fw):
            boot_gains.append(fw / fa)
    if len(boot_gains) < int(0.95 * BOOTSTRAP_RESAMPLES):
        raise ValueError("too many degenerate batch-bootstrap samples to report a stable lower bound")
    boot_gains.sort()
    bootstrap_lower = boot_gains[max(0, math.ceil(0.025 * len(boot_gains)) - 1)]
    lower = min(seed_lower, bootstrap_lower)
    met = lower >= NUC050_TARGET and unbiased and tail_resolved
    if not unbiased:
        outcome = "FAIL: windowed and analog means are biased by more than 3 combined standard errors"
    elif not tail_resolved:
        outcome = "INCONCLUSIVE: VOV exceeds 0.1 in at least one replicate or pooled configuration"
    elif lower < NUC050_TARGET:
        outcome = "FAIL: 95% lower confidence bound is below 100x"
    else:
        outcome = "PASS"
    return {"schema": ANALYSIS_SCHEMA, "response": RESPONSE, "response_sha256": canonical_hash(RESPONSE),
            "seeds_by_configuration": plan["sampling"]["seeds_by_configuration"],
            "replicate_pairs_by_index": replicate_pairs, "paired_fom_gain_by_replicate_index": gains,
            "gain_mean": mean, "gain_seed_lower97_5_one_sided_t_df3": seed_lower,
            "gain_bootstrap_lower97_5_2_5th_percentile": bootstrap_lower, "gain_lower95_bonferroni": lower,
            "bootstrap": {"method": "independent resampling of batch means within each configuration; pooled FOM uses total elapsed seconds",
                          "resamples": BOOTSTRAP_RESAMPLES, "seed": BOOTSTRAP_SEED},
            "pooled_fom_gain": gain,
            "analog": pa, "windows": pw, "bias_z": z, "unbiased_within_3_sigma": unbiased,
            "vov_tail_resolved_diagnostic": tail_resolved,
            "vov_note": "VOV > 0.1 is reported as an unresolved-tail diagnostic; it is not silently discarded.",
            "lower_bound_assumption": "Bonferroni minimum of one-sided 97.5% Student-t bound over four replicate-indexed ratios using disjoint analog/windowed seeds (df=3; assumes independent, approximately normal seed blocks) and 2.5th percentile of deterministic bootstrap over pooled batch means (assumes exchangeable independent batch means within each configuration); four replicates limit tail inference",
            "nuc050_target": NUC050_TARGET, "tail_resolved_required_for_pass": True, "nuc050_met": met,
            "nuc050_outcome": outcome,
            "limitations": ["adjoint target uses the whole homogenized TF-coil material; final reported tally is the exact inboard-box response",
                            "FOM confidence bound is based on four seed-specific ratios; it is a small-sample Student-t interval",
                            "all tallies are neutron-only flux above 0.1 MeV, not nuclear heating"]}


def _load_plan(work: Path) -> dict:
    path = Path(work) / "campaign-plan.json"
    if not path.is_file():
        raise FileNotFoundError(f"missing {path}; run the plan subcommand first")
    return json.loads(path.read_text())


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    child = sub.add_parser("transport-child", help=argparse.SUPPRESS)
    child.add_argument("--request", required=True, type=Path)
    for name in ("plan", "prepare", "run", "analyze"):
        p = sub.add_parser(name)
        p.add_argument("--work-dir", required=True, type=Path)
        if name != "analyze":
            p.add_argument("--model-dir", required=True, type=Path)
            p.add_argument("--stem", default="rm_m")
            p.add_argument("--cross-sections", type=Path)
        if name in ("plan", "prepare"):
            p.add_argument("--threads", type=int, default=DEFAULT_THREADS)
            p.add_argument("--memory-gb", type=float, default=DEFAULT_MEMORY_GB)
            p.add_argument("--time-budget-s", type=float, default=DEFAULT_TOTAL_BUDGET_S)
    args = parser.parse_args(argv)
    if args.command == "transport-child":
        return _execute_transport_child(args.request)
    work = args.work_dir.expanduser().resolve()
    if args.command == "plan":
        require_absent(work / "campaign-plan.json", "frozen campaign plan")
        require_absent(work / "windows", "campaign windows directory")
        require_absent(work / "runs", "campaign runs directory")
        pre = prerequisites(args.model_dir, work, args.stem, getattr(args, "cross_sections", None))
        record = plan_record(pre, threads=args.threads, memory_gb=args.memory_gb, total_budget_s=args.time_budget_s)
        print(json.dumps(record, indent=2))
        if record["status"] != "ready":
            return 2
        work.mkdir(parents=True, exist_ok=True)
        write_json_new(work / "campaign-plan.json", record)
        return 0
    if args.command == "prepare":
        plan = _load_plan(work)
        if plan.get("status") != "ready":
            raise RuntimeError("frozen campaign plan is blocked; recreate it after resolving all prerequisites and budget")
        verify_implementation_hashes(plan)
        pre = prerequisites(args.model_dir, work, args.stem, getattr(args, "cross_sections", None))
        if not pre["ready"]:
            print(json.dumps({"status": "blocked", "prerequisites": pre["prerequisites"]}, indent=2))
            return 2
        if pre["model_hashes"] != plan["preflight"]["model_hashes"]:
            raise RuntimeError("RM-M inputs differ from the read-only plan; create a fresh plan")
        if pre["data"] != plan["preflight"]["data"]:
            raise RuntimeError("cross-section files differ from the read-only plan; create a fresh plan")
        if args.threads != plan["sampling"]["threads"]:
            raise RuntimeError("window generation thread count differs from campaign plan")
        if args.memory_gb * 1024**3 != plan["sampling"]["memory_limit_bytes"]:
            raise RuntimeError("prepare memory limit differs from campaign plan; use the documented prlimit wrapper")
        print(json.dumps(prepare_windows(pre, work, args.threads), indent=2))
        return 0
    if args.command == "run":
        plan = _load_plan(work)
        verify_implementation_hashes(plan)
        pre = prerequisites(args.model_dir, work, args.stem, getattr(args, "cross_sections", None))
        if not pre["ready"]:
            print(json.dumps({"status": "blocked", "prerequisites": pre["prerequisites"]}, indent=2))
            return 2
        if pre["model_hashes"] != plan["preflight"]["model_hashes"]:
            raise RuntimeError("RM-M inputs differ from the read-only plan; create a fresh plan")
        if pre["data"] != plan["preflight"]["data"]:
            raise RuntimeError("cross-section files differ from the read-only plan; create a fresh plan")
        RM.FARIS_XS = Path(pre["cross_sections_xml"])
        records = run_campaign(plan, work, args.model_dir, args.stem)
        print(json.dumps({"completed_runs": len(records), "run_records": [r["configuration"] + ":" + str(r["seed"]) for r in records]}, indent=2))
        return 0
    plan = _load_plan(work)
    records = []
    for path in sorted((work / "runs").glob("*/record.json")):
        records.append(json.loads(path.read_text()))
    result = analyze_records(records, plan)
    target = work / "analysis.json"
    target.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))
    return 0 if result["nuc050_met"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
