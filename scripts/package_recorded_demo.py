#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Build identity-checked recorded transport + completed Core evidence cases.

Each scenario has reference and breeder-emphasis runs. Port runs additionally
require the independent volume reports bound to those exact run artifacts. The
package omits statepoints and nuclear-data files and cannot imply physical
qualification.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
from pathlib import PurePosixPath
import shutil
import subprocess
import sys
import tempfile
import re
from datetime import datetime, timezone
SCRIPT_DIR = str(Path(__file__).resolve().parent)
if SCRIPT_DIR not in sys.path:
    sys.path.insert(0, SCRIPT_DIR)
from port_geometry_contract import validate_ownership_audits
from recorded_bundle_contract import (validate_recorded_bundle, inspect_sweep_bundle,
                                      check_sweep_set, SWEEP_BUNDLE_DIRECTORY)
from verify_recorded_demo import launch_paths, part_for, rust_platform
from recorded_archives import (create_archive, MAX_PATH_COMPONENTS,
                               MAX_TREE_DIRECTORIES, MAX_EXPANDED_DIRECTORIES)

REQUIRED_RESPONSES = {"total-tritium-production", "heating-total-whole-model"}
FORBIDDEN_SUFFIXES = {".h5", ".hdf5", ".endf", ".zip"}
SWEEP_SCENARIO_RELATIVE = "scenarios/arc-inspired/allocation-sweep/scenario.json"
ANALYSES = "breeding,shielding,fuel-history,electricity"
MAX_PACKAGE_FILE_BYTES = 64 * 1024 * 1024
MAX_TREE_BYTES = 512 * 1024 * 1024
# Delivered package total; each expanded case/workspace tree keeps MAX_TREE_BYTES.
MAX_PACKAGE_BYTES = 1024 * 1024 * 1024
MAX_TREE_FILES = 2048
MAX_TREE_MEMBERS = 4096
MAX_EXPANDED_PACKAGE_BYTES = 1536 * 1024 * 1024
MAX_EXPANDED_PACKAGE_FILES = 8192
OUTAGE_DURATION_MULTIPLIERS = (0.5, 1.0, 2.0)
HISTORY_REFINEMENT_REPORT_NAME = "operating-history-primary-refinement-v4.json"
HISTORY_REFINEMENT_SCHEMA = "faris-operating-history-primary-refinement-v4"
SUPPORT_SOURCE_FILES = (
    "docs/COLD_REFERENCE.md",
    "docs/CORE_RUNTIME_DEPENDENCY_NOTICES.md",
    "docs/DEMO_INPUT_SPEC.md",
    "docs/LITHIUM_CAPTURE_CONTROL.md",
    "docs/NUMERICAL_CONTROLS.md",
    "docs/OPERATING_HISTORY.md",
    "docs/PHOTON_LIBRARY_ACQUISITION.md",
    "controls/check_history.py",
    "controls/check_lithium_capture.py",
    "controls/check_port_geometry.py",
    "controls/check_transport_arithmetic.py",
    "controls/test_lithium_capture.py",
    "scripts/collect_dependency_notices.py",
    "integrations/openmc/assemble_fendl_photon_overlay.py",
    "integrations/openmc/audit_library.py",
    "integrations/openmc/convert_endfbvii1_photon.py",
    "integrations/openmc/lithium_capture_control.py",
    "references/fendl-neutron-provenance-crosscheck.json",
    "references/lithium-capture-control-verification.json",
    "references/openmc-library-audit.json",
    "references/operating-history-continuous-processing-control.json",
    "references/operating-history-processing-control-phases-assumptions.json",
    "references/operating-history-processing-control-phases-history.json",
    "references/operating-history-processing-control-rates.json",
    "references/operating-history-processing-control-restart-assumptions.json",
    "references/operating-history-processing-control-restart-history.json",
    "references/photon-library-provenance.json",
)
LOCAL_PATH_KEYS = {"data_root", "source_root", "source_fendl_path",
                   "local_prior_environment_path"}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return "sha256:" + digest.hexdigest()


def invoke(command: list[str], timeout: int = 180) -> subprocess.CompletedProcess:
    result = subprocess.run(command, capture_output=True, text=True, timeout=timeout,
                            check=False)
    if result.returncode != 0:
        raise RuntimeError(f"command failed ({result.returncode}): {' '.join(command)}\n"
                           f"{result.stderr[-4000:]}")
    return result


def run_json(command: list[str]) -> dict:
    result = invoke(command)
    try:
        value = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise RuntimeError(f"command did not return JSON: {' '.join(command)}") from error
    if not isinstance(value, dict):
        raise RuntimeError(f"command returned a non-object JSON value: {' '.join(command)}")
    return value


def verify_run(faris: Path, scenario: Path, run_path: Path, expected_variant: str) -> dict:
    report = run_json([str(faris), "reactor", "inspect", "--scenario", str(scenario),
                       "--run", str(run_path)])
    if report.get("inspection_status") != "REVALIDATED_NORMALIZATION":
        raise RuntimeError(f"{run_path}: FARIS did not revalidate normalization")
    if report.get("variant_id") != expected_variant:
        raise RuntimeError(f"{run_path}: expected variant {expected_variant!r}")
    if str(report.get("scientific_qualification", "")).upper() != "NOT_EVALUATED":
        raise RuntimeError(f"{run_path}: qualification must remain NOT_EVALUATED")
    identities = {item.get("response_id") for item in report.get("non_mesh_responses", [])}
    if not REQUIRED_RESPONSES <= identities:
        raise RuntimeError(f"{run_path}: missing required integrated responses "
                           f"{sorted(REQUIRED_RESPONSES - identities)}")
    bins = report.get("mesh_nonzero_flux_bin_count")
    if not isinstance(bins, int) or bins <= 0:
        raise RuntimeError(f"{run_path}: revalidated run has no nonzero spatial flux bins")
    return report


def verify_port_volume_report(path: Path, scenario: Path, run_path: Path,
                              report: dict, expected_variant: str) -> dict:
    value = json.loads(path.read_text())
    run_record = json.loads(run_path.read_text())
    worker_path = run_path.parent / "solver" / "worker-result.json"
    input_path = run_path.parent / "input.json"
    scenario_sha = hashlib.sha256(scenario.read_bytes()).hexdigest()
    if value.get("schema_version") != "faris-independent-port-volume-check/v0.1":
        raise RuntimeError(f"{path}: unsupported port-volume report schema")
    if value.get("geometry_check") != "PASS" or value.get("transport_volume_check") != "PASS":
        raise RuntimeError(f"{path}: independent port-volume checks are not PASS")
    if value.get("scientific_qualification") != "NOT_EVALUATED":
        raise RuntimeError(f"{path}: geometry report must retain NOT_EVALUATED")
    if value.get("scenario_sha256") != scenario_sha or value.get("variant_id") != expected_variant:
        raise RuntimeError(f"{path}: wrong exact scenario or arrangement identity")
    if value.get("run_record_sha256") != sha256(run_path).removeprefix("sha256:"):
        raise RuntimeError(f"{path}: report is not bound to the exact run record")
    if value.get("transport_artifact_sha256") != run_record.get("raw_artifact_sha256"):
        raise RuntimeError(f"{path}: report is not bound to the run's raw transport artifact")
    if (not worker_path.is_file()
            or value.get("worker_result_sha256") != sha256(worker_path).removeprefix("sha256:")):
        raise RuntimeError(f"{path}: report is not bound to the adjacent worker-result record")
    if (not input_path.is_file()
            or value.get("input_sha256") != sha256(input_path).removeprefix("sha256:")):
        raise RuntimeError(f"{path}: report is not bound to the adjacent input snapshot")
    checks = value.get("component_volume_validation", [])
    if not checks or any(check.get("status") != "PASS" for check in checks):
        raise RuntimeError(f"{path}: one or more component-volume checks did not pass")
    if value.get("checks_are_geometry_only") is not True:
        raise RuntimeError(f"{path}: geometry-only limitation is missing")
    if report.get("scenario_sha256") != scenario_sha:
        raise RuntimeError(f"{path}: FARIS inspection and report scenario identities differ")
    return value


def verify_port_geometry_ownership(worker_path: Path, run_path: Path, scenario: Path,
                                   expected_variant: str) -> dict:
    """Require an independently executed, identity-bound geometry ownership audit."""
    worker = json.loads(worker_path.read_text(encoding="utf-8"))
    run_record = json.loads(run_path.read_text(encoding="utf-8"))
    input_path = run_path.parent / "input.json"
    if not input_path.is_file():
        raise RuntimeError(f"{run_path}: input snapshot is missing")
    input_sha = sha256(input_path).removeprefix("sha256:")
    scenario_sha = hashlib.sha256(scenario.read_bytes()).hexdigest()
    audit = worker.get("geometry_ownership_audit")
    if not isinstance(audit, dict):
        raise RuntimeError(f"{worker_path}: geometry ownership audit is missing")
    input_record = json.loads(input_path.read_text(encoding="utf-8"))
    physics = input_record.get("physics")
    assignments = physics.get("component_assignments") if isinstance(physics, dict) else None
    if not isinstance(assignments, list) or not assignments:
        raise RuntimeError(f"{input_path}: physics component assignments are missing")
    component_materials = {}
    for item in assignments:
        if not isinstance(item, dict):
            raise RuntimeError(f"{input_path}: invalid component assignment")
        component_id, material_id = item.get("component_id"), item.get("material_id")
        if (not isinstance(component_id, str) or not component_id
                or not isinstance(material_id, str) or not material_id
                or component_id in component_materials):
            raise RuntimeError(f"{input_path}: malformed or duplicate component assignment")
        component_materials[component_id] = material_id

    penetration = worker.get("penetration_volume_audit")
    try:
        validate_ownership_audits(
            audit, penetration, scenario_sha256=scenario_sha,
            variant_id=expected_variant, input_sha256=input_sha,
            component_materials=component_materials)
    except (TypeError, ValueError) as error:
        raise RuntimeError(f"{worker_path}: invalid geometry ownership audit: {error}") from error
    return audit


def verify_saved_case(faris: Path, case: Path, report: Path, workspace: Path,
                     expected_variant: str, scenario_sha: str) -> dict:
    inspection = run_json([str(faris), "evidence", "inspect", "--case", str(case),
                           "--report", str(report), "--workspace", str(workspace)])
    if (inspection.get("schema_version") != "faris-saved-case-inspection/v0.2"
            or inspection.get("record_integrity") != "UNSIGNED_IDENTITY_REVALIDATED"
            or inspection.get("variant_id") != expected_variant
            or inspection.get("scenario_sha256") != f"sha256:{scenario_sha}"
            or inspection.get("execution_status") != "executed"
            or inspection.get("binding_status") != "verified"):
        raise RuntimeError(f"saved Core evidence inspection failed for {case}")
    if (not inspection.get("compiler_id") or not inspection.get("semantic_profile")
            or inspection.get("compiler_executable_sha256") != inspection.get("core_executable_sha256")):
        raise RuntimeError(f"saved Core inspection lacks its bound compiler/profile identity for {case}")
    if not inspection.get("steps") or inspection.get("verified_receipt_count") != len(inspection["steps"]):
        raise RuntimeError(f"saved Core receipts were not all revalidated for {case}")
    verdicts = inspection.get("requirement_verdicts", [])
    if not verdicts or any(item.get("status") != "not_evaluated" for item in verdicts):
        raise RuntimeError(f"{case}: physical criteria must remain NOT_EVALUATED")
    return inspection


def bare_sha256(value: object, label: str) -> str:
    if not isinstance(value, str):
        raise RuntimeError(f"{label}: missing SHA-256 identity")
    result = value.removeprefix("sha256:")
    if len(result) != 64:
        raise RuntimeError(f"{label}: malformed SHA-256 identity")
    try:
        bytes.fromhex(result)
    except ValueError as error:
        raise RuntimeError(f"{label}: malformed SHA-256 identity") from error
    return result


def verify_finite_json(value: object, label: str) -> None:
    if isinstance(value, float) and not math.isfinite(value):
        raise RuntimeError(f"{label}: non-finite numeric value")
    if isinstance(value, dict):
        for child in value.values():
            verify_finite_json(child, label)
    elif isinstance(value, list):
        for child in value:
            verify_finite_json(child, label)


def write_bounded_json(path: Path, value: dict) -> None:
    encoded = (json.dumps(value, indent=2, sort_keys=True, allow_nan=False) + "\n").encode()
    if len(encoded) > MAX_PACKAGE_FILE_BYTES:
        raise RuntimeError(f"refusing to write oversized package artifact: {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        raise RuntimeError(f"refusing to overwrite package artifact: {path}")
    path.write_bytes(encoded)


def source_run_identity(run_path: Path, scenario_sha: str, variant_id: str) -> dict:
    run = json.loads(run_path.read_text(encoding="utf-8"))
    if (run.get("scenario_sha256") != scenario_sha
            or run.get("variant_id") != variant_id
            or run.get("execution", {}).get("execution_status") != "SUCCEEDED"):
        raise RuntimeError(f"{run_path}: source run identity/status differs from the selected case")
    return {
        "run_record_sha256": sha256(run_path),
        "raw_artifact_sha256": bare_sha256(run.get("raw_artifact_sha256"), str(run_path)),
        "input_sha256": bare_sha256(run.get("input_sha256"), str(run_path)),
        "scenario_sha256": scenario_sha,
        "variant_id": variant_id,
        "sampling": run.get("sampling"),
    }


def add_event_history(faris: Path, branch: Path, scenario: Path, run_path: Path,
                      assumptions: Path, scenario_sha: str, variant_id: str) -> dict:
    identity = source_run_identity(run_path, scenario_sha, variant_id)
    history_path = branch / "event-histories" / f"{variant_id}.json"
    rates_path = branch / "event-histories" / f"{variant_id}.rates.json"
    history_path.parent.mkdir(parents=True, exist_ok=True)
    invoke([str(faris), "history", "from-run", "--scenario", str(scenario),
            "--run", str(run_path), "--assumptions", str(assumptions),
            "--output", str(history_path), "--rates-output", str(rates_path)], timeout=600)
    history = json.loads(history_path.read_text(encoding="utf-8"))
    rates = json.loads(rates_path.read_text(encoding="utf-8"))
    assumptions_data = json.loads(assumptions.read_text(encoding="utf-8"))
    verify_finite_json(history, str(history_path))
    if (history.get("schema_version") != "faris-history-result/v0.1"
            or history.get("assumptions") != assumptions_data
            or bare_sha256(history.get("driving_rates", {}).get("scenario_sha256"), str(history_path)) != scenario_sha
            or bare_sha256(history.get("driving_rates", {}).get("transport_artifact_sha256"), str(history_path))
            != identity["raw_artifact_sha256"]
            or bare_sha256(rates.get("transport_artifact_sha256"), str(rates_path))
            != identity["raw_artifact_sha256"]
            or not history.get("snapshots") or not history.get("events")):
        raise RuntimeError(f"{history_path}: event history is incomplete or bound to another run")
    kinds = sorted({event.get("kind") for event in history["events"]})
    if not {"planned_outage_started", "planned_outage_ended"} <= set(kinds):
        raise RuntimeError(f"{history_path}: event history does not exercise planned outage transitions")
    provenance = {
        "schema_version": "faris-packaged-event-history-provenance/v0.1",
        "generator_faris_cli_sha256": sha256(faris),
        **identity,
        "assumptions_sha256": sha256(assumptions),
        "history_sha256": sha256(history_path),
        "rates_sha256": sha256(rates_path),
        "event_kinds": kinds,
        "outcome": history.get("outcome"),
        "event_count": len(history["events"]),
        "snapshot_count": len(history["snapshots"]),
        "scientific_scope": "Authored event-control history using the exact identified run rates; scenario thresholds are not physical material limits.",
    }
    provenance_path = branch / "event-histories" / f"{variant_id}.provenance.json"
    write_bounded_json(provenance_path, provenance)
    return {"history_path": history_path.relative_to(branch.parent).as_posix(),
            "history_sha256": sha256(history_path),
            "rates_path": rates_path.relative_to(branch.parent).as_posix(),
            "rates_sha256": sha256(rates_path),
            "provenance_path": provenance_path.relative_to(branch.parent).as_posix(),
            "provenance_sha256": sha256(provenance_path),
            "assumptions_sha256": sha256(assumptions)}


def add_sensitivity(faris: Path, branch: Path, scenario: Path, run_path: Path,
                    assumptions: Path, grid_path: Path, scenario_sha: str,
                    variant_id: str) -> dict:
    identity = source_run_identity(run_path, scenario_sha, variant_id)
    sensitivity_path = branch / "sensitivities" / f"{variant_id}.json"
    sensitivity_path.parent.mkdir(parents=True, exist_ok=True)
    invoke([str(faris), "history", "sensitivity", "--scenario", str(scenario),
            "--run", str(run_path), "--assumptions", str(assumptions),
            "--grid", str(grid_path), "--output", str(sensitivity_path)], timeout=1800)
    result = json.loads(sensitivity_path.read_text(encoding="utf-8"))
    verify_finite_json(result, str(sensitivity_path))
    grid = json.loads(grid_path.read_text(encoding="utf-8"))
    expected_points = (len(grid["recovery_fraction_levels"])
                      * len(grid["delay_multipliers"])
                      * len(grid["service_limit_multipliers"]))
    if (result.get("schema_version") != "faris-history-sensitivity/v0.1"
            or result.get("grid") != grid
            or result.get("base_assumptions") != json.loads(assumptions.read_text(encoding="utf-8"))
            or len(result.get("points", [])) != expected_points
            or not result.get("points")
            or bare_sha256(result.get("driving_rates", {}).get("scenario_sha256"), str(sensitivity_path)) != scenario_sha
            or bare_sha256(result.get("driving_rates", {}).get("transport_artifact_sha256"), str(sensitivity_path))
            != identity["raw_artifact_sha256"]):
        raise RuntimeError(f"{sensitivity_path}: sensitivity is incomplete or identity-mismatched")
    provenance = {
        "schema_version": "faris-packaged-history-sensitivity-provenance/v0.1",
        "generator_faris_cli_sha256": sha256(faris),
        **identity,
        "assumptions_sha256": sha256(assumptions),
        "grid_sha256": sha256(grid_path),
        "sensitivity_sha256": sha256(sensitivity_path),
        "point_count": len(result["points"]),
        "scope": "Full deterministic history reruns under an authored bounded grid; grid levels are not probability distributions or confidence intervals.",
    }
    provenance_path = branch / "sensitivities" / f"{variant_id}.provenance.json"
    write_bounded_json(provenance_path, provenance)
    return {"sensitivity_path": sensitivity_path.relative_to(branch.parent).as_posix(),
            "sensitivity_sha256": sha256(sensitivity_path),
            "provenance_path": provenance_path.relative_to(branch.parent).as_posix(),
            "provenance_sha256": sha256(provenance_path),
            "point_count": expected_points}


def scale_outage_durations(base: dict, multiplier: float) -> dict:
    """Copy an authored event fixture while changing only planned outage lengths."""
    if (isinstance(multiplier, bool) or not isinstance(multiplier, (int, float))
            or not math.isfinite(multiplier) or multiplier not in OUTAGE_DURATION_MULTIPLIERS):
        raise ValueError("outage multiplier is outside the frozen three-level axis")
    if not isinstance(base, dict):
        raise ValueError("event assumptions must be an object")
    adjusted = json.loads(json.dumps(base))
    outages = adjusted.get("planned_outages")
    horizon = adjusted.get("horizon_s")
    if (not isinstance(outages, list) or not outages
            or not isinstance(horizon, (int, float)) or isinstance(horizon, bool)
            or not math.isfinite(horizon) or horizon <= 0):
        raise ValueError("outage-duration study requires authored planned outages")
    previous_end = -math.inf
    previous_start = None
    expected_base_duration = 30 * 86400.0
    annual_spacing = 365.25 * 86400.0
    for outage in outages:
        start, end = outage.get("start_s"), outage.get("end_s")
        if (not isinstance(start, (int, float)) or isinstance(start, bool)
                or not isinstance(end, (int, float)) or isinstance(end, bool)
                or not math.isfinite(start) or not math.isfinite(end) or end <= start
                or start < previous_end):
            raise ValueError("planned outage intervals must be finite, ordered, and non-overlapping")
        duration = (end - start) * multiplier
        base_duration = end - start
        if not math.isclose(base_duration, expected_base_duration, rel_tol=0.0, abs_tol=1e-6):
            raise ValueError("frozen outage axis requires exactly 30-day baseline outages")
        if previous_start is not None:
            if not math.isclose(start - previous_start, annual_spacing, rel_tol=0.0, abs_tol=1e-6):
                raise ValueError("frozen outage axis requires exactly annual 365.25-day spacing")
        if not math.isfinite(duration) or duration <= 0:
            raise ValueError("scaled planned outage duration must be finite and positive")
        outage["end_s"] = start + duration
        if outage["end_s"] > horizon:
            raise ValueError("scaled planned outage exceeds the authored history horizon")
        previous_end = outage["end_s"]
        previous_start = start
    return adjusted


def add_outage_duration_study(faris: Path, staging: Path, pair_id: str,
                              scenario: Path, run_path: Path,
                              base_assumptions: Path, scenario_sha: str,
                              variant_id: str) -> list[dict]:
    identity = source_run_identity(run_path, scenario_sha, variant_id)
    base = json.loads(base_assumptions.read_text(encoding="utf-8"))
    baseline_report = Path(__file__).resolve().parents[1] / "references" / HISTORY_REFINEMENT_REPORT_NAME
    if not baseline_report.is_file():
        raise RuntimeError("the independent 600-second baseline refinement report is required for outage-axis anchoring")
    baseline_report_sha = sha256(baseline_report)
    baseline = json.loads(baseline_report.read_text(encoding="utf-8"))
    driver_id = f"{pair_id}-{('reference' if variant_id == 'reference' else 'breeder')}"
    baseline_output = baseline.get("primary_drivers", {}).get(driver_id, {}).get("history_outputs", {}).get("600")
    if (baseline.get("schema_version") != HISTORY_REFINEMENT_SCHEMA
            or not isinstance(baseline_output, dict)
            or baseline_output.get("assumptions_sha256")
            != sha256(base_assumptions).removeprefix("sha256:")):
        raise RuntimeError(f"baseline refinement report is not bound to the frozen assumptions/driver {driver_id}")
    records = []
    for multiplier in OUTAGE_DURATION_MULTIPLIERS:
        adjusted = scale_outage_durations(base, multiplier)
        factor = str(multiplier).replace(".", "p")
        directory = staging / "outage-duration-sensitivity" / pair_id / variant_id / f"multiplier-{factor}"
        directory.mkdir(parents=True)
        assumptions_path = directory / "assumptions.json"
        write_bounded_json(assumptions_path, adjusted)
        history_path = directory / "history.json"
        rates_path = directory / "rates.json"
        invoke([str(faris), "history", "from-run", "--scenario", str(scenario),
                "--run", str(run_path), "--assumptions", str(assumptions_path),
                "--output", str(history_path), "--rates-output", str(rates_path)], timeout=600)
        invoke([str(faris), "history", "validate", "--assumptions", str(assumptions_path),
                "--rates", str(rates_path)], timeout=180)
        history = json.loads(history_path.read_text(encoding="utf-8"))
        rates = json.loads(rates_path.read_text(encoding="utf-8"))
        verify_finite_json(history, str(history_path))
        if (history.get("schema_version") != "faris-history-result/v0.1"
                or history.get("assumptions") != adjusted
                or bare_sha256(history.get("driving_rates", {}).get("scenario_sha256"), str(history_path)) != scenario_sha
                or bare_sha256(history.get("driving_rates", {}).get("transport_artifact_sha256"), str(history_path))
                != identity["raw_artifact_sha256"]
                or bare_sha256(rates.get("transport_artifact_sha256"), str(rates_path))
                != identity["raw_artifact_sha256"]
                or not history.get("snapshots")):
            raise RuntimeError(f"outage history is incomplete or bound to another run: {history_path}")
        if multiplier == 1.0 and (
                sha256(history_path).removeprefix("sha256:") != baseline_output.get("history_sha256")
                or sha256(rates_path).removeprefix("sha256:") != baseline_output.get("rates_sha256")):
            raise RuntimeError(f"1.0 outage probe differs from audited 600-second baseline for {driver_id}")
        provenance = {
            "schema_version": "faris-outage-duration-provenance/v0.1",
            "pair_id": pair_id,
            "variant_id": variant_id,
            "duration_multiplier": multiplier,
            "outage_durations_days": sorted({(item["end_s"] - item["start_s"]) / 86400.0
                                              for item in adjusted["planned_outages"]}),
            **identity,
            "base_operating_assumptions_sha256": sha256(base_assumptions),
            "baseline_refinement_report_sha256": baseline_report_sha,
            "adjusted_assumptions_sha256": sha256(assumptions_path),
            "history_sha256": sha256(history_path),
            "rates_sha256": sha256(rates_path),
            "event_count": len(history.get("events", [])),
            "snapshot_count": len(history["snapshots"]),
            "outcome": history.get("outcome"),
            "scope": "Authored one-factor scenario probe only; levels are not probability distributions, physical uncertainty ranges, maintenance forecasts, or availability estimates.",
            "interpretation": "AUTHORED_SCENARIO_PROBE",
            "not_probability_distribution": True,
            "not_physical_uncertainty": True,
            "not_availability_estimate": True,
            "baseline_anchor_history_sha256": baseline_output.get("history_sha256"),
            "baseline_anchor_rates_sha256": baseline_output.get("rates_sha256"),
        }
        provenance_path = directory / "provenance.json"
        write_bounded_json(provenance_path, provenance)
        records.append({
            "pair_id": pair_id, "variant_id": variant_id,
            "duration_multiplier": multiplier,
            "assumptions_path": assumptions_path.relative_to(staging).as_posix(),
            "assumptions_sha256": sha256(assumptions_path),
            "history_path": history_path.relative_to(staging).as_posix(),
            "history_sha256": sha256(history_path),
            "rates_path": rates_path.relative_to(staging).as_posix(),
            "rates_sha256": sha256(rates_path),
            "provenance_path": provenance_path.relative_to(staging).as_posix(),
            "provenance_sha256": sha256(provenance_path),
        })
    return records


def archive_tree(staging: Path, branch: Path, variant_id: str,
                 kind: str, source: Path) -> dict[str, object]:
    archives = branch / "archives"
    archives.mkdir(exist_ok=True)
    stem = f"{variant_id}-{kind}"
    archive_path = archives / f"{stem}.tar.gz"
    manifest_path = archives / f"{stem}.manifest.json"
    manifest = create_archive(source, archive_path, manifest_path)
    return {
        "path": archive_path.relative_to(staging).as_posix(),
        "sha256": sha256(archive_path),
        "bytes": archive_path.stat().st_size,
        "manifest_path": manifest_path.relative_to(staging).as_posix(),
        "manifest_sha256": sha256(manifest_path),
        "expanded_bytes": manifest["expanded_bytes"],
        "file_count": manifest["file_count"],
        "archive_member_count": manifest["archive_member_count"],
        "directory_count": manifest["directory_count"],
    }


def copy_port_volume_report(staging: Path, branch: Path, variant_id: str,
                            source: Path) -> dict[str, str]:
    """Copy a bound port-volume report; both variants share the geometry directory."""
    target = branch / "geometry" / f"{variant_id}-volume-check.json"
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, target)
    return {"path": target.relative_to(staging).as_posix(), "sha256": sha256(target)}


def add_history_comparison(faris: Path, branch: Path, scenario: Path,
                           left_run: Path, right_run: Path, assumptions: Path,
                           scenario_sha: str) -> dict:
    left_identity = source_run_identity(left_run, scenario_sha, "reference")
    right_identity = source_run_identity(right_run, scenario_sha, "breeder-emphasis")
    controlled_difference = (
        "Reference and breeder-emphasis blanket/shield geometry allocations differ (blanket +0.10 m, shield −0.10 m), "
        "with material compositions, source and data held fixed under the same scenario and authored assumptions; "
        "transport seeds are distinct."
    )
    comparison_path = branch / "comparisons" / "reference-vs-breeder-emphasis.json"
    comparison_path.parent.mkdir(parents=True, exist_ok=True)
    invoke([str(faris), "history", "compare-runs", "--scenario", str(scenario),
            "--left-run", str(left_run), "--left-label", "reference",
            "--right-run", str(right_run), "--right-label", "breeder-emphasis",
            "--assumptions", str(assumptions), "--controlled-difference", controlled_difference,
            "--output", str(comparison_path)], timeout=1200)
    cli_provenance_path = Path(str(comparison_path) + ".provenance.json")
    comparison = json.loads(comparison_path.read_text(encoding="utf-8"))
    cli_provenance = json.loads(cli_provenance_path.read_text(encoding="utf-8"))
    verify_finite_json(comparison, str(comparison_path))
    if (comparison.get("schema_version") != "faris-history-comparison/v0.1"
            or comparison.get("left_label") != "reference"
            or comparison.get("right_label") != "breeder-emphasis"
            or comparison.get("controlled_difference") != controlled_difference
            or not comparison.get("left", {}).get("events")
            or not comparison.get("right", {}).get("events")):
        raise RuntimeError(f"{comparison_path}: comparison report is incomplete")
    if bare_sha256(cli_provenance.get("comparison_sha256"), str(cli_provenance_path)) != sha256(comparison_path).removeprefix("sha256:"):
        raise RuntimeError(f"{cli_provenance_path}: comparison artifact digest mismatch")
    for side, identity in (("left_run", left_identity), ("right_run", right_identity)):
        source = cli_provenance.get(side, {})
        if (bare_sha256(source.get("run_record_sha256"), str(cli_provenance_path))
                != bare_sha256(identity["run_record_sha256"], str(left_run))
                or bare_sha256(source.get("raw_artifact_sha256"), str(cli_provenance_path))
                != identity["raw_artifact_sha256"]
                or bare_sha256(source.get("scenario_sha256"), str(cli_provenance_path)) != scenario_sha):
            raise RuntimeError(f"{cli_provenance_path}: source identity mismatch for {side}")
    for side, identity in (("left", left_identity), ("right", right_identity)):
        rates = comparison.get(side, {}).get("driving_rates", {})
        if (bare_sha256(rates.get("scenario_sha256"), str(comparison_path)) != scenario_sha
                or bare_sha256(rates.get("transport_artifact_sha256"), str(comparison_path))
                != identity["raw_artifact_sha256"]):
            raise RuntimeError(f"{comparison_path}: comparison rates differ from the selected source run")
    if (bare_sha256(cli_provenance.get("assumptions_sha256"), str(cli_provenance_path))
            != bare_sha256(sha256(assumptions), str(assumptions))
            or bare_sha256(cli_provenance.get("scenario_sha256"), str(cli_provenance_path)) != scenario_sha):
        raise RuntimeError(f"{cli_provenance_path}: assumptions or scenario identity mismatch")
    provenance_path = branch / "comparisons" / "reference-vs-breeder-emphasis.provenance.json"
    provenance = {
        "schema_version": "faris-packaged-history-comparison-provenance/v0.1",
        "generator_faris_cli_sha256": sha256(faris),
        "comparison_sha256": sha256(comparison_path),
        "assumptions_sha256": sha256(assumptions),
        "scenario_sha256": scenario_sha,
        "left": left_identity,
        "right": right_identity,
        "dependence_note": comparison.get("dependence_note"),
        "controlled_difference": controlled_difference,
        "scope": "Descriptive deterministic comparison; independent seeds do not estimate correlated transport uncertainty.",
    }
    cli_provenance_path.unlink()
    write_bounded_json(provenance_path, provenance)
    return {"comparison_path": comparison_path.relative_to(branch.parent).as_posix(),
            "comparison_sha256": sha256(comparison_path),
            "provenance_path": provenance_path.relative_to(branch.parent).as_posix(),
            "provenance_sha256": sha256(provenance_path)}


def assign_parts(records: list[dict], index_fields: dict) -> dict:
    """Mark each file record app or evidence, return the `parts` totals, and assert
    that every path the app opens at launch is in the app part."""
    launched = launch_paths(index_fields)
    by_path = {record["path"]: record for record in records}
    for record in records:
        record["part"] = part_for(record["path"], launched)
    for relative in sorted(launched):
        if relative not in by_path or by_path[relative]["part"] != "app":
            raise RuntimeError(f"app launch path is not in the app part: {relative}")
    for relative in ("bin/faris-app", "bin/avila-core"):
        if by_path.get(relative, {}).get("part") != "app":
            raise RuntimeError(f"app executable is not in the app part: {relative}")
    totals = {}
    for part in ("app", "evidence"):
        members = [record for record in records if record["part"] == part]
        totals[part] = {"file_count": len(members), "bytes": sum(item["bytes"] for item in members)}
    return totals


def prune_empty_directories(root: Path) -> None:
    """Remove directories left empty by staging, so the package holds only indexed files
    and both release archives unpack to exactly the same tree."""
    for path in sorted((p for p in root.rglob("*") if p.is_dir() and not p.is_symlink()),
                       key=lambda p: len(p.parts), reverse=True):
        if not any(path.iterdir()):
            path.rmdir()


def scan_package(root: Path) -> list[dict]:
    records = []
    total_bytes = 0
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            raise RuntimeError(f"refusing package symlink: {path}")
        if not path.is_file():
            continue
        if path.suffix.lower() in FORBIDDEN_SUFFIXES:
            raise RuntimeError(f"refusing to package external scientific data: {path}")
        if path.stat().st_size > MAX_PACKAGE_FILE_BYTES:
            raise RuntimeError(f"package file exceeds the 64 MiB bound: {path}")
        records.append({"path": path.relative_to(root).as_posix(),
                        "bytes": path.stat().st_size, "sha256": sha256(path)})
        total_bytes += path.stat().st_size
        if len(records) > MAX_TREE_FILES or total_bytes > MAX_PACKAGE_BYTES:
            raise RuntimeError("recorded demo package exceeds 1 GiB or 2,048 files")
    return records


def git_output(repository: Path, *arguments: str) -> str:
    result = subprocess.run(["git", "-C", str(repository), *arguments],
                            capture_output=True, text=True, check=False)
    if result.returncode != 0:
        raise RuntimeError(f"cannot read source provenance from {repository}: {result.stderr.strip()}")
    return result.stdout.strip()


def executable_version(path: Path) -> str:
    result = subprocess.run([str(path), "--version"], capture_output=True, text=True,
                            check=False, timeout=10)
    if result.returncode != 0 or not result.stdout.strip():
        raise RuntimeError(f"cannot obtain version from packaged executable: {path}")
    return result.stdout.strip()


def install_local_runtime(staging: Path, faris: Path, app: Path, core: Path,
                          core_source_repo: Path, core_source_revision: str,
                          *, require_clean_faris_source: bool = True) -> dict:
    if sys.platform != "linux":
        raise RuntimeError("the recorded package builder currently targets Linux only")
    bin_dir = staging / "bin"
    bin_dir.mkdir()
    installed = {}
    for name, source in (("faris", faris), ("faris-app", app), ("avila-core", core)):
        target = bin_dir / name
        shutil.copyfile(source, target)
        target.chmod(0o555)
        installed[name] = {"path": target.relative_to(staging).as_posix(),
                           "sha256": sha256(target), "bytes": target.stat().st_size,
                           "version": executable_version(target)}

    scripts_dir = staging / "scripts"
    scripts_dir.mkdir()
    for name in ("verify_recorded_demo.py", "recorded_bundle_contract.py",
                 "port_geometry_contract.py", "verify_binary_manifest.py",
                 "recorded_archives.py"):
        source = Path(__file__).with_name(name)
        shutil.copyfile(source, scripts_dir / name)

    faris_repo = Path(__file__).resolve().parents[1]
    core_license_root = core_source_repo.resolve(strict=True)
    licenses_dir = staging / "licenses"
    licenses_dir.mkdir()
    core_license_files = ["LICENSE", "THIRD_PARTY_NOTICES.md"]
    license_tree = git_output(core_license_root, "ls-tree", "-r", "--name-only",
                              core_source_revision, "--", "LICENSES")
    core_license_files.extend(path for path in license_tree.splitlines() if path)
    for relative in core_license_files:
        path = PurePosixPath(relative)
        allowed_root_file = relative in {"LICENSE", "THIRD_PARTY_NOTICES.md"}
        allowed_license_file = relative.startswith("LICENSES/")
        if path.is_absolute() or ".." in path.parts or not (allowed_root_file or allowed_license_file):
            raise RuntimeError(f"unsafe Core license path at source revision: {relative}")
        output_name = ({"LICENSE": "core-LICENSE",
                        "THIRD_PARTY_NOTICES.md": "core-THIRD_PARTY_NOTICES.md"}.get(relative)
                       or str(PurePosixPath("core-LICENSES") / path.relative_to("LICENSES")))
        source = subprocess.run(["git", "-C", str(core_license_root), "show",
                                 f"{core_source_revision}:{relative}"],
                                capture_output=True, check=False)
        if source.returncode != 0:
            raise RuntimeError(f"Core source revision lacks license material: {relative}")
        target = licenses_dir / output_name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(source.stdout)
    shutil.copyfile(faris_repo / "LICENSE", licenses_dir / "faris-LICENSE")
    faris_third_party_notices = faris_repo / "docs" / "THIRD_PARTY_NOTICES.md"
    if not faris_third_party_notices.is_file() or faris_third_party_notices.stat().st_size > MAX_PACKAGE_FILE_BYTES:
        raise RuntimeError("FARIS third-party dependency notices are missing or oversized")
    shutil.copyfile(faris_third_party_notices,
                    licenses_dir / "faris-THIRD_PARTY_NOTICES.md")
    core_runtime_notices = faris_repo / "docs" / "CORE_RUNTIME_DEPENDENCY_NOTICES.md"
    rust_library_notice = faris_repo / "licenses" / "rust-1.98.1-COPYRIGHT-library.html"
    for source in (core_runtime_notices, rust_library_notice):
        if not source.is_file() or source.stat().st_size > MAX_PACKAGE_FILE_BYTES:
            raise RuntimeError(f"required runtime license/notice is missing or oversized: {source}")
    shutil.copyfile(core_runtime_notices,
                    licenses_dir / "core-RUNTIME_DEPENDENCY_NOTICES.md")
    shutil.copyfile(rust_library_notice,
                    licenses_dir / "rust-1.98.1-COPYRIGHT-library.html")

    faris_revision = git_output(faris_repo, "rev-parse", "HEAD")
    core_revision_resolved = git_output(core_license_root, "rev-parse", f"{core_source_revision}^{{commit}}")
    if core_revision_resolved != core_source_revision:
        raise RuntimeError("Core source revision must be a full canonical commit SHA")
    core_remote = git_output(core_license_root, "remote", "get-url", "origin")
    faris_remote = git_output(faris_repo, "remote", "get-url", "origin")
    faris_clean = not bool(git_output(faris_repo, "status", "--porcelain", "--untracked-files=all"))
    if require_clean_faris_source and not faris_clean:
        raise RuntimeError("refusing to distribute binaries from a dirty FARIS source tree")
    source_record = {
        "faris": {"repository": faris_remote, "commit": faris_revision,
                  "working_tree_clean": faris_clean},
        "core": {"repository": core_remote, "commit": core_revision_resolved,
                 "checkout_head": git_output(core_license_root, "rev-parse", "HEAD"),
                 "binary_profile": "debug",
                 "working_tree_clean_at_packaging": not bool(git_output(
                     core_license_root, "status", "--porcelain", "--untracked-files=all"))},
        "rebuild": [
            "FARIS: check out the recorded commit, then run cargo build --release --locked -p faris-cli -p faris-app --bins -j 1.",
            "Packaged Core: check out the recorded commit, run cargo build --locked --bin avila-core, copy target/debug/avila-core to the distribution, then run strip --strip-debug on that copy.",
            "Optional Core release build: cargo build --release --locked --bin avila-core; it is compatible but has a different binary hash.",
            "These instructions identify the source and toolchain command; they do not claim bit-for-bit reproducibility.",
        ],
    }
    (staging / "SOURCE_PROVENANCE.md").write_text(
        "# Local runtime provenance\n\n"
        f"- FARIS repository: `{faris_remote}` at `{faris_revision}`.\n"
        f"- Avila Core repository: `{core_remote}` at `{core_revision_resolved}`.\n"
        f"- Packaged FARIS CLI reports: `{installed['faris']['version']}`.\n"
        f"- Packaged native app reports: `{installed['faris-app']['version']}`.\n"
        f"- Core executable reports: `{installed['avila-core']['version']}`.\n"
        "- Rebuild FARIS with `cargo build --release --locked -p faris-cli -p faris-app --bins -j 1`.\n"
        "- The packaged Core executable is from the debug profile: build with `cargo build --locked --bin avila-core`, "
        "copy `target/debug/avila-core`, then apply `strip --strip-debug` to that copy.\n"
        "- A release-profile Core rebuild is also compatible, using `cargo build --release --locked --bin avila-core`; "
        "it will have a different hash and is not byte-identical to this package.\n"
        "- These commands document the build route; no bit-for-bit reproducibility claim is made.\n"
        "- License texts and Core third-party notices are included under `licenses/`.\n"
        "- Binary SHA-256 values in the package index identify bytes only; they are unsigned.\n",
        encoding="utf-8")

    verify = """#!/bin/sh
set -eu
export PYTHONDONTWRITEBYTECODE=1
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT HUP INT TERM
python3 "$root/scripts/verify_recorded_demo.py" \\
  --package "$root" \\
  --relocated-copy "$temporary/relocated-demo" \\
  --faris "$root/bin/faris" \\
  --core "$root/bin/avila-core"
"""
    verify_path = staging / "verify.sh"
    verify_path.write_text(verify, encoding="utf-8")
    verify_path.chmod(0o555)
    license_files = {
        path.relative_to(staging).as_posix(): sha256(path)
        for path in sorted(licenses_dir.rglob("*")) if path.is_file()
    }
    return {"schema_version": "faris-local-runtime/v0.1",
            "platform": rust_platform(),
            "executables": installed,
            "license_files": license_files,
            "source_provenance": source_record,
            "launcher": {"kind": "native", "executable": "faris-app"},
            "verifier": {"path": "verify.sh", "sha256": sha256(verify_path)}}


def redact_local_paths(value: object, redactions: list[str], location: str = "$") -> object:
    if isinstance(value, dict):
        result = {}
        for key, child in value.items():
            child_location = f"{location}.{key}"
            if key in LOCAL_PATH_KEYS and isinstance(child, str):
                result[key] = "<local filesystem path omitted>"
                redactions.append(child_location)
            else:
                result[key] = redact_local_paths(child, redactions, child_location)
        return result
    if isinstance(value, list):
        return [redact_local_paths(child, redactions, f"{location}[{index}]")
                for index, child in enumerate(value)]
    if isinstance(value, str) and (value.startswith("/") or value.startswith("runs/")):
        redactions.append(location)
        return "<local artifact path omitted>"
    return value


def install_support(staging: Path, campaign_reports: list[tuple[str, Path]]) -> dict:
    repository = Path(__file__).resolve().parents[1]
    support_root = staging / "support"
    records = []
    for relative in SUPPORT_SOURCE_FILES:
        source = repository / relative
        if (not source.is_file() or source.stat().st_size > MAX_PACKAGE_FILE_BYTES):
            raise RuntimeError(f"required bounded demo support source is unavailable: {source}")
        target_relative = PurePosixPath("support") / PurePosixPath(relative)
        target = staging / target_relative
        target.parent.mkdir(parents=True, exist_ok=True)
        original_digest = sha256(source)
        redactions: list[str] = []
        if source.suffix.lower() == ".json":
            try:
                source_json = json.loads(source.read_text(encoding="utf-8"))
                sanitized = redact_local_paths(source_json, redactions)
                target.write_text(json.dumps(sanitized, indent=2, sort_keys=True, allow_nan=False) + "\n",
                                  encoding="utf-8")
            except (UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
                raise RuntimeError(f"support JSON is invalid: {source}") from error
        else:
            shutil.copyfile(source, target)
        if target.stat().st_size > MAX_PACKAGE_FILE_BYTES:
            raise RuntimeError(f"packaged support file exceeds the 64 MiB bound: {target}")
        records.append({"source_path": relative, "source_sha256": original_digest,
                        "package_path": target_relative.as_posix(),
                        "package_sha256": sha256(target), "redacted_locations": redactions})

    seen_labels: set[str] = set()
    for label, source in campaign_reports:
        if (not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,63}", label)
                or label in seen_labels or not source.is_file()
                or source.stat().st_size > MAX_PACKAGE_FILE_BYTES):
            raise RuntimeError(f"invalid, duplicate, or oversized campaign support report: {label}")
        seen_labels.add(label)
        try:
            source_json = json.loads(source.read_text(encoding="utf-8"))
            redactions = []
            sanitized = redact_local_paths(source_json, redactions)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise RuntimeError(f"campaign support report is not valid JSON: {source}") from error
        relative = f"support/campaigns/{label}.json"
        target = staging / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(json.dumps(sanitized, indent=2, sort_keys=True, allow_nan=False) + "\n",
                          encoding="utf-8")
        records.append({"source_name": source.name, "source_sha256": sha256(source),
                        "package_path": relative, "package_sha256": sha256(target),
                        "source_location_omitted": True,
                        "redacted_locations": redactions})

    readme = support_root / "README.md"
    readme.write_text(
        "# Scientific scope and support materials\n\n"
        "This directory contains bounded documentation, independent checking scripts, and metadata. "
        "It contains no nuclear-data library, ENDF archive, OpenMC statepoint, or Li-control raw statepoint. "
        "Use the four recorded transport bundles as the exact run inputs and identities. The included cold "
        "reference and history documents explain model assumptions and limits; their older run summaries "
        "are not substituted for the run records in this package. Any bundled campaign report is an "
        "identity-bearing audit artifact with path fields redacted for portability. All physical reactor "
        "qualification remains NOT_EVALUATED.\n",
        encoding="utf-8")
    manifest_path = support_root / "manifest.json"
    manifest_path.write_text(json.dumps({
        "schema_version": "faris-recorded-demo-support/v0.1",
        "files": records,
        "support_readme_sha256": sha256(readme),
        "demo_acceptance_snapshot_included": False,
    }, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return {"path": "support/manifest.json", "sha256": sha256(manifest_path),
            "file_count": len(records), "demo_acceptance_snapshot_included": False}


def add_sweep(staging: Path, bundle_paths: list[Path]) -> dict | None:
    """Copy identity-checked allocation-sweep bundles under sweep/ and describe them."""
    if not bundle_paths:
        return None
    scenario_path = Path(__file__).resolve().parents[1] / SWEEP_SCENARIO_RELATIVE
    scenario_bytes = scenario_path.read_bytes()
    scenario = json.loads(scenario_bytes)
    scenario_sha = hashlib.sha256(scenario_bytes).hexdigest()
    sweep_dir = staging / "sweep"
    (sweep_dir / "bundles").mkdir(parents=True)
    shutil.copyfile(scenario_path, sweep_dir / "scenario.json")
    runs = []
    for path in bundle_paths:
        record = inspect_sweep_bundle(json.loads(path.read_text(encoding="utf-8")),
                                      scenario_sha256=scenario_sha, scenario=scenario)
        target = staging / SWEEP_BUNDLE_DIRECTORY / f"{record['variant_id']}.transport-bundle.json"
        if target.exists():
            raise RuntimeError(f"duplicate sweep variant: {record['variant_id']}")
        shutil.copyfile(path, target)
        record["transport_bundle"] = target.relative_to(staging).as_posix()
        record["transport_bundle_sha256"] = sha256(target)
        runs.append(record)
    check_sweep_set(runs)
    runs.sort(key=lambda item: item["blanket_thickness_m"])
    return {"scenario_id": scenario.get("id"), "scenario_path": "sweep/scenario.json",
            "scenario_sha256": scenario_sha, "runs": runs}


def evidence_archive_name(version: str) -> str:
    return f"FARIS-{version}-evidence.tar.gz"


def write_package_readme(staging: Path, pairs: list[dict], support: dict,
                         sweep: dict | None, version: str) -> None:
    expanded_bytes = sum(int(arrangement[key]["expanded_bytes"])
                         for pair in pairs for arrangement in pair["arrangements"]
                         for key in ("case_archive", "workspace_archive"))
    expanded_files = sum(int(arrangement[key]["file_count"])
                         for pair in pairs for arrangement in pair["arrangements"]
                         for key in ("case_archive", "workspace_archive"))
    compressed_archive_bytes = sum(int(arrangement[key]["bytes"])
                                   for pair in pairs for arrangement in pair["arrangements"]
                                   for key in ("case_archive", "workspace_archive"))
    lines = [
        "# FARIS recorded coupled-transport demo",
        "",
        "This portable package contains four identity-checked fixed-source transport records and four completed Core evidence cases.",
        "Core execution and receipt revalidation establish workflow completion; every physical qualification status remains NOT_EVALUATED.",
        "It is a cold-data numerical surrogate, not ARC, a materials qualification, an experimental benchmark, or an engineering prediction.",
        "",
        "## Recorded case matrix",
        "",
        "| Scenario | Variant | Scenario SHA-256 | Run SHA-256 | Raw artifact SHA-256 | Responses | Nonzero mesh flux bins |",
        "|---|---|---|---|---|---:|---:|",
    ]
    for pair in pairs:
        for arrangement in pair["arrangements"]:
            lines.append(
                f"| {pair['scenario_id']} | {arrangement['variant_id']} | `{pair['scenario_sha256']}` "
                f"| `{arrangement['run_record_sha256'].removeprefix('sha256:')}` "
                f"| `{arrangement['raw_artifact_sha256']}` "
                f"| {arrangement['normalized_response_count']} "
                f"| {arrangement['mesh_nonzero_flux_bin_count']} |"
            )
    if sweep:
        lines.extend([
            "",
            f"## Blanket/shield allocation sweep ({len(sweep['runs'])} recorded runs)",
            "",
            f"Scenario `{sweep['scenario_id']}` (SHA-256 `{sweep['scenario_sha256']}`) holds the total thickness fixed and moves it between the breeding blanket and the neutron shield. Each run is a recorded transport bundle under `sweep/bundles/`; the app opens them at launch. They carry transport identity only (no Core evidence cases) and the same NOT_EVALUATED scope.",
            "",
            "| Variant | Blanket (m) | Shield (m) | Run SHA-256 | Raw artifact SHA-256 |",
            "|---|---:|---:|---|---|",
        ])
        for run in sweep["runs"]:
            lines.append(
                f"| {run['variant_id']} | {run['blanket_thickness_m']:g} | {run['shield_thickness_m']:g} "
                f"| `{run['run_record_sha256'].removeprefix('sha256:')}` | `{run['raw_artifact_sha256']}` |")
    else:
        lines.extend(["", "This package contains no blanket/shield allocation sweep."])
    lines.extend([
        "",
        "Each recorded bundle contains exact run/input/scenario/audit/adapter/raw artifact/worker/spectra bytes.",
        "It exposes normalized integrated tritium production and all-particle heating, 12 neutron/photon component spectra, and the complete local mesh field.",
        "The port cases additionally retain independent OpenMC point-ownership/clearance and geometry-volume audit reports.",
        "",
        "The package includes two descriptive paired-history comparisons, four event-control histories, four 27-point sensitivity results, and a separate 12-case planned-outage duration axis (15/30/60 days for each of four exact transport drivers). The outage axis changes duration only and is an authored scenario probe, not a physical uncertainty range or availability estimate.",
        "Their deterministic results are conditional on the authored ledger model; grid points are not probabilities, confidence limits, material allowables, or lifetime predictions.",
        "See `support/` for the bounded scientific background, independent checker scripts, data acquisition route, license notes, and included verification metadata.",
        "The old DEMO_ACCEPTANCE snapshot is intentionally omitted because release acceptance is determined by the final package index and fresh verifier run.",
        "",
        "## Opening the study",
        "",
        "The app finds this package by itself, checks the indexed bytes of the files it opens, and shows the four recorded cases (and the allocation sweep, when the package contains one). Nothing is written inside this folder; the app keeps its own runs outside it.",
        "- Linux (glibc 2.35 or newer: Ubuntu 22.04 or later, Debian 12 or later): run `bin/faris-app` (or double-click it).",
        "- Windows: double-click `bin\\faris-app.exe`. Windows SmartScreen may warn because the programs are not signed; choose More info, then Run anyway.",
        "- macOS: the programs are not signed or notarized. After unpacking, run `xattr -dr com.apple.quarantine <folder>` once in Terminal (with this folder in place of `<folder>`), or open `bin/faris-app` and allow it under System Settings, Privacy & Security, Open Anyway.",
        "The bundled executables are hash-pinned and not signed. Their hashes establish byte identity, not authenticity.",
        "",
        "## The two downloads",
        "",
        f"The program, the transport bundles, the operating assumptions, the licenses and the package index are the platform download (`FARIS-{version}-<os>-<arch>.tar.gz`). That alone opens and runs the whole study.",
        f"The evidence download (`{evidence_archive_name(version)}`) adds the Core receipts shown in the Evidence step and the files `verify.sh` checks: the eight Core case/workspace archives, the saved-study descriptors, inspections, exports, comparisons, event histories, sensitivities, outage-duration probes, support files, `inputs/`, `verify.sh` and the verifier scripts. To install it, unpack it into the same folder as the platform download on every platform (both unpack into `FARIS-{version}/`). Without it the Evidence step says the Core receipts are not included, and the rest works.",
        f"With the evidence part present, the app expands the eight Core case/workspace archives into a private temporary folder while it runs and removes it when it exits. The temporary space for that is the indexed expanded total, {expanded_bytes} bytes in {expanded_files} files, plus one filesystem block per indexed implicit directory and 64 MiB.",
        "",
        "## Verification",
        "",
        "`./verify.sh` needs Linux, `python3` and the evidence part; the Windows and macOS downloads are checked by the app itself at launch. It relocates a copy of the compressed package, expands the archives, revalidates saved Core evidence and export reports, and checks rejection of a separate tampered copy. It needs temporary space for the relocated compressed package (`package_bytes` in `package-index.json`) plus the expanded bytes above, one largest one-case export copy, directory blocks and 64 MiB; the verifier checks this. A later tamper negative control needs a third compressed copy only after expanded scratch is released. No files are expanded inside the read-only distribution.",
        "",
        "No OpenMC statepoint, neutron/photon nuclear-data file, ENDF input, or data archive is included. Follow `support/docs/PHOTON_LIBRARY_ACQUISITION.md` for local fresh-run data setup; redistribution terms for the evaluated libraries remain unresolved.",
        f"The eight Core case/workspace archives occupy {compressed_archive_bytes} compressed bytes and expand to {expanded_bytes} bytes across {expanded_files} files. Expansion reproduces the original files byte-for-byte. The full package's indexed compressed total is `package_bytes` in `package-index.json`; outer caps are 64 MiB per indexed file, 2,048 files, and 1 GiB total. Each expanded case or workspace is capped at 512 MiB, 2,048 files, 4,096 archive members, and 1,024 implicit directories; aggregate expansion is capped at 1.5 GiB, 8,192 files, 8,192 archive members, and 8,192 implicit directories. Paths are limited to 64 components. Each expanded file remains capped at 64 MiB.",
        "",
    ])
    destination = staging / "README.md"
    destination.write_text("\n".join(lines), encoding="utf-8")
    if destination.stat().st_size > MAX_PACKAGE_FILE_BYTES:
        raise RuntimeError("generated package README exceeds the 64 MiB bound")


def add_pair(staging: Path, pair_id: str, faris: Path, core: Path,
             scenario_path: Path, reference_run: Path, breeder_run: Path,
             assumptions: Path, event_assumptions: Path, sensitivity_grid: Path,
             port_reports: tuple[Path, Path] | None) -> dict:
    scenario_data = json.loads(scenario_path.read_text())
    scenario_sha = hashlib.sha256(scenario_path.read_bytes()).hexdigest()
    branch = staging / pair_id
    branch.mkdir()
    shutil.copyfile(scenario_path, branch / "scenario.json")
    variants = (
        ("reference", reference_run),
        ("breeder-emphasis", breeder_run),
    )
    run_summaries = []
    event_summaries = []
    sensitivity_summaries = []
    outage_summaries = []
    for index, (variant_id, run_path) in enumerate(variants):
        inspected = verify_run(faris, scenario_path, run_path, variant_id)
        volume_identity = None
        ownership_identity = None
        if port_reports:
            volume_report = verify_port_volume_report(
                port_reports[index], scenario_path, run_path, inspected, variant_id)
            worker_path = run_path.parent / "solver" / "worker-result.json"
            ownership = verify_port_geometry_ownership(
                worker_path, run_path, scenario_path, variant_id)
            volume_identity = copy_port_volume_report(
                staging, branch, variant_id, port_reports[index])
            input_data = json.loads((run_path.parent / "input.json").read_text(encoding="utf-8"))
            assignments = input_data["physics"]["component_assignments"]
            geometry_evidence = {
                "schema_version": "faris-packaged-port-geometry-ownership/v0.1",
                "worker_result_sha256": sha256(worker_path),
                "run_record_sha256": sha256(run_path),
                "transport_artifact_sha256": json.loads(run_path.read_text(encoding="utf-8"))["raw_artifact_sha256"],
                "input_sha256": json.loads(run_path.read_text(encoding="utf-8"))["input_sha256"],
                "scenario_sha256": inspected["scenario_sha256"],
                "variant_id": variant_id,
                "component_materials": {item["component_id"]: item["material_id"]
                                         for item in assignments},
                "geometry_ownership_audit": ownership,
                "penetration_volume_audit": json.loads(worker_path.read_text(encoding="utf-8"))["penetration_volume_audit"],
            }
            ownership_target = branch / "geometry" / f"{variant_id}-ownership-audit.json"
            ownership_target.write_text(json.dumps(geometry_evidence, indent=2, sort_keys=True) + "\n",
                                        encoding="utf-8")
            ownership_identity = {"path": ownership_target.relative_to(staging).as_posix(),
                                 "sha256": sha256(ownership_target),
                                 "worker_result_sha256": sha256(worker_path)}

        case = branch / "cases" / variant_id
        study = branch / "studies" / variant_id
        invoke([str(faris), "study", "generate", "--scenario", str(scenario_path),
                "--variant", variant_id, "--analysis", ANALYSES,
                "--output", str(study)])
        invoke([str(faris), "evidence", "prepare", "--run", str(run_path),
                "--study", str(study / "study.json"), "--assumptions", str(assumptions),
                "--core", str(core), "--faris", str(faris), "--output", str(case)])
        workspace = branch / "core-workspaces" / variant_id
        execution_report = case / "execution-report.json"
        invoke([str(faris), "evidence", "run", "--case", str(case), "--core", str(core),
                "--faris", str(faris), "--workspace", str(workspace),
                "--output", str(execution_report)], timeout=360)
        evidence = json.loads(execution_report.read_text())
        if (evidence.get("execution", {}).get("execution_status") != "SUCCEEDED"
                or evidence.get("execution", {}).get("exit_code") != 0
                or evidence.get("report", {}).get("status") != "evaluated"):
            raise RuntimeError(f"Core evidence execution incomplete for {pair_id}/{variant_id}")
        verdicts = evidence.get("report", {}).get("campaign", {}).get("verdicts", [])
        verdict_states = [item.get("verdict", {}).get("status") for item in verdicts]
        if not verdict_states or any(state != "not_evaluated" for state in verdict_states):
            raise RuntimeError(f"{pair_id}/{variant_id}: physical criteria must remain NOT_EVALUATED")
        saved_inspection = verify_saved_case(
            faris, case, execution_report, workspace, variant_id, scenario_sha)
        inspection_path = branch / "inspections" / f"{variant_id}.saved-case-inspection.json"
        inspection_path.parent.mkdir(exist_ok=True)
        inspection_path.write_text(json.dumps(saved_inspection, indent=2, sort_keys=True) + "\n")
        # Core export duplicates the complete case tree byte-for-byte. Validate
        # the export in a temporary directory, but retain only its report: the
        # canonical case below is already the portable export payload.
        with tempfile.TemporaryDirectory(prefix="faris-core-export-") as temporary:
            export_dir = Path(temporary) / "export"
            export_result = run_json([str(core), "export", str(case), "--source-root",
                                      f"case={case}", "--out", str(export_dir)])
            if (export_result.get("schema_version") != "avila.core/export-report/v0.1-draft"
                    or export_result.get("status") != "exported"
                    or export_result.get("case_id") != evidence.get("expected_case_id")):
                raise RuntimeError(f"Core export did not verify the completed case {pair_id}/{variant_id}")
            temporary_report = export_dir / "export-report.json"
            if (not temporary_report.is_file()
                    or json.loads(temporary_report.read_text()) != export_result):
                raise RuntimeError(f"Core export report did not match its published copy: {temporary_report}")
            export_report_data = temporary_report.read_bytes()
        export_report = branch / "core-exports" / variant_id / "export-report.json"
        export_report.parent.mkdir(parents=True, exist_ok=True)
        export_report.write_bytes(export_report_data)
        recorded_bundle = case / "inputs" / "recorded.json"
        if not recorded_bundle.is_file():
            raise RuntimeError(f"prepared Core case lacks portable transport bundle: {recorded_bundle}")
        run_record_data = json.loads(run_path.read_text(encoding="utf-8"))
        bundle_summary = validate_recorded_bundle(
            json.loads(recorded_bundle.read_text(encoding="utf-8")),
            scenario_sha256=scenario_sha, variant_id=variant_id,
            expected_run_sha256=sha256(run_path),
            expected_raw_artifact_sha256=run_record_data.get("raw_artifact_sha256"),
            mesh_nonzero_flux_bin_count=inspected.get("mesh_nonzero_flux_bin_count"))
        bundle_path = branch / "bundles" / f"{variant_id}.transport-bundle.json"
        bundle_path.parent.mkdir(exist_ok=True)
        shutil.copyfile(recorded_bundle, bundle_path)
        event_identity = add_event_history(
            faris, branch, scenario_path, run_path, event_assumptions,
            scenario_sha, variant_id)
        event_summaries.append(event_identity)
        sensitivity_identity = add_sensitivity(
            faris, branch, scenario_path, run_path, assumptions,
            sensitivity_grid, scenario_sha, variant_id)
        sensitivity_summaries.append(sensitivity_identity)
        outage_summaries.extend(add_outage_duration_study(
            faris, staging, branch.name, scenario_path, run_path,
            assumptions, scenario_sha, variant_id))
        case_report_sha = sha256(execution_report)
        case_archive = archive_tree(staging, branch, variant_id, "case", case)
        workspace_archive = archive_tree(staging, branch, variant_id, "workspace", workspace)
        descriptor_path = staging / f"saved-study-{pair_id}-{variant_id}.json"
        descriptor = {
            "schema_version": "faris-saved-study-archive/v0.1",
            "case_archive": case_archive,
            "workspace_archive": workspace_archive,
            "execution_report_member": "execution-report.json",
        }
        descriptor_path.write_text(json.dumps(descriptor, indent=2) + "\n", encoding="utf-8")
        shutil.rmtree(case)
        shutil.rmtree(workspace)
        run_summaries.append({
            "variant_id": variant_id,
            "run_record_sha256": sha256(run_path),
            "raw_artifact_sha256": source_run_identity(run_path, scenario_sha, variant_id)["raw_artifact_sha256"],
            "input_sha256": source_run_identity(run_path, scenario_sha, variant_id)["input_sha256"],
            "sampling": source_run_identity(run_path, scenario_sha, variant_id)["sampling"],
            "scenario_sha256": inspected["scenario_sha256"],
            "scientific_qualification": inspected["scientific_qualification"],
            "normalized_response_count": inspected["response_count"],
            "mesh_nonzero_flux_bin_count": inspected.get("mesh_nonzero_flux_bin_count"),
            "offline_field_and_spectrum_identity": bundle_summary,
            "source_record_name": "run.json",
            "core_execution_report_member": "execution-report.json",
            "core_execution_report_sha256": case_report_sha,
            "saved_case_inspection": inspection_path.relative_to(staging).as_posix(),
            "saved_case_inspection_sha256": sha256(inspection_path),
            "core_requirement_verdicts": verdict_states,
            "core_export_report": export_report.relative_to(staging).as_posix(),
            "core_export_sha256": export_result.get("export_sha256"),
            "core_export_report_sha256": sha256(export_report),
            "saved_study_descriptor": descriptor_path.relative_to(staging).as_posix(),
            "saved_study_descriptor_sha256": sha256(descriptor_path),
            "case_archive": case_archive,
            "workspace_archive": workspace_archive,
            "transport_bundle": bundle_path.relative_to(staging).as_posix(),
            "transport_bundle_sha256": sha256(bundle_path),
            "port_volume_report": volume_identity,
            "port_geometry_ownership_report": ownership_identity,
            "event_history": event_identity,
            "sensitivity_study": sensitivity_identity,
        })
    comparison_summary = add_history_comparison(
        faris, branch, scenario_path, reference_run, breeder_run,
        assumptions, scenario_sha)
    return {
        "scenario_id": scenario_data.get("id"),
        "scenario_path": (branch / "scenario.json").relative_to(staging).as_posix(),
        "scenario_sha256": scenario_sha,
        "arrangements": run_summaries,
        "paired_history_comparison": comparison_summary,
        "event_histories": event_summaries,
        "sensitivity_studies": sensitivity_summaries,
        "outage_duration_studies": outage_summaries,
        "feature": "finite_port" if port_reports else "feature_free_control",
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--faris", required=True, type=Path)
    parser.add_argument("--faris-app", required=True, type=Path)
    parser.add_argument("--core", required=True, type=Path)
    parser.add_argument("--core-source-repo", required=True, type=Path)
    parser.add_argument("--core-source-revision", required=True)
    parser.add_argument("--control-scenario", required=True, type=Path)
    parser.add_argument("--control-reference-run", required=True, type=Path)
    parser.add_argument("--control-breeder-run", required=True, type=Path)
    parser.add_argument("--port-scenario", required=True, type=Path)
    parser.add_argument("--port-reference-run", required=True, type=Path)
    parser.add_argument("--port-breeder-run", required=True, type=Path)
    parser.add_argument("--port-reference-volume-report", required=True, type=Path)
    parser.add_argument("--port-breeder-volume-report", required=True, type=Path)
    parser.add_argument("--assumptions", required=True, type=Path)
    parser.add_argument("--event-assumptions", required=True, type=Path)
    parser.add_argument("--sensitivity-grid", required=True, type=Path)
    parser.add_argument("--support-report", action="append", default=[], metavar="LABEL=JSON_PATH",
                        help="additional bounded campaign audit report to retain under support/campaigns")
    parser.add_argument("--sweep-bundle", action="append", default=[], type=Path, metavar="PATH",
                        help="portable allocation-sweep RecordedTransportBundle (from `faris transport pack`); repeatable")
    parser.add_argument("--version", required=True,
                        help="release version; names the evidence archive FARIS-<version>-evidence.tar.gz")
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if not re.fullmatch(r"\d+\.\d+\.\d+", args.version):
        raise SystemExit("--version must look like 0.1.1")
    files = [args.faris, args.faris_app, args.core, args.control_scenario, args.control_reference_run,
             args.control_breeder_run, args.port_scenario, args.port_reference_run,
             args.port_breeder_run, args.port_reference_volume_report,
             args.port_breeder_volume_report, args.assumptions,
             args.event_assumptions, args.sensitivity_grid]
    support_reports = []
    for item in args.support_report:
        label, separator, raw_path = item.partition("=")
        if not separator or not label or not raw_path:
            raise SystemExit("--support-report must use LABEL=JSON_PATH")
        report_path = Path(raw_path)
        if not report_path.is_file() or report_path.stat().st_size > MAX_PACKAGE_FILE_BYTES:
            raise SystemExit(f"support report is missing or oversized: {report_path}")
        support_reports.append((label, report_path.resolve()))
    for bundle_path in args.sweep_bundle:
        if not bundle_path.is_file() or bundle_path.stat().st_size > MAX_PACKAGE_FILE_BYTES:
            raise SystemExit(f"sweep bundle is missing or oversized: {bundle_path}")
    if not args.sweep_bundle:
        print("NOTE: no --sweep-bundle given; the package will contain no allocation sweep.",
              file=sys.stderr)
    baseline_report_source = Path(__file__).resolve().parents[1] / "references" / HISTORY_REFINEMENT_REPORT_NAME
    if not any(label == "history-refinement" and path == baseline_report_source.resolve()
               for label, path in support_reports):
        raise SystemExit("--support-report history-refinement=<exact v4 baseline refinement report> is required")
    for path in files:
        if not path.is_file():
            raise SystemExit(f"required file is missing: {path}")
        if path.stat().st_size > MAX_PACKAGE_FILE_BYTES:
            raise SystemExit(f"required input exceeds the 64 MiB bound: {path}")
    output = args.output.absolute()
    if output.exists():
        raise SystemExit(f"output already exists: {output}")
    faris, app, core = args.faris.resolve(), args.faris_app.resolve(), args.core.resolve()
    core_source_repo = args.core_source_repo.resolve(strict=True)
    assumptions = args.assumptions.resolve()
    event_assumptions = args.event_assumptions.resolve()
    sensitivity_grid = args.sensitivity_grid.resolve()
    try:
        assumption_data = json.loads(assumptions.read_text())
        if not isinstance(assumption_data, dict):
            raise ValueError("operating assumptions must be a JSON object")
        event_assumption_data = json.loads(event_assumptions.read_text())
        if not isinstance(event_assumption_data, dict):
            raise ValueError("event-control assumptions must be a JSON object")
        grid_data = json.loads(sensitivity_grid.read_text())
        if not isinstance(grid_data, dict):
            raise ValueError("sensitivity grid must be a JSON object")
        levels = [grid_data.get("recovery_fraction_levels"),
                  grid_data.get("delay_multipliers"),
                  grid_data.get("service_limit_multipliers")]
        if any(not isinstance(values, list) or len(values) != 3 for values in levels):
            raise ValueError("final sensitivity artifact must declare three levels per parameter (27 points)")
        bounds = [(0.0, 1.0), (0.25, 4.0), (0.25, 4.0)]
        for values, (lower, upper) in zip(levels, bounds, strict=True):
            if (any(not isinstance(value, (int, float)) or isinstance(value, bool)
                    or not math.isfinite(value) or value < lower or value > upper
                    for value in values)
                    or len(set(values)) != len(values)):
                raise ValueError("sensitivity levels must be finite, distinct, and inside model bounds")
        if not isinstance(grid_data.get("rationale"), str) or not grid_data["rationale"].strip():
            raise ValueError("sensitivity grid requires an authored rationale")
        pairs = [
            (args.control_scenario.resolve(), args.control_reference_run.resolve(),
             args.control_breeder_run.resolve(), None),
            (args.port_scenario.resolve(), args.port_reference_run.resolve(),
             args.port_breeder_run.resolve(),
             (args.port_reference_volume_report.resolve(), args.port_breeder_volume_report.resolve())),
        ]
        scenarios = [json.loads(item[0].read_text()) for item in pairs]
        if any(not isinstance(item, dict) for item in scenarios):
            raise ValueError("each scenario must be a JSON object")
        control_data, port_data = scenarios
        if control_data.get("penetration") is not None:
            raise ValueError("control scenario must be feature-free")
        if (not isinstance(port_data.get("penetration"), dict)
                or port_data["penetration"].get("kind") != "outboard_rectangular_prism"):
            raise ValueError("port scenario must contain the finite rectangular port")
        for field in ("schema_version", "geometry", "operating_plan", "variants",
                      "references", "assumptions"):
            if control_data.get(field) != port_data.get(field):
                raise ValueError(f"control and port scenarios differ outside the feature: {field}")
    except (OSError, json.JSONDecodeError, ValueError) as error:
        raise SystemExit(f"invalid JSON input: {error}") from error

    output.parent.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
    try:
        shutil.copyfile(assumptions, staging / "operating-assumptions.json")
        runtime_manifest = install_local_runtime(
            staging, faris, app, core, core_source_repo, args.core_source_revision)
        support_manifest = install_support(staging, support_reports)
        inputs_dir = staging / "inputs"
        inputs_dir.mkdir()
        shutil.copyfile(event_assumptions, inputs_dir / "event-assumptions.json")
        shutil.copyfile(sensitivity_grid, inputs_dir / "sensitivity-grid.json")
        branches = []
        for pair_id, (scenario_path, reference_run, breeder_run, volume_reports) in zip(
                ("control", "port"), pairs, strict=True):
            branches.append(add_pair(staging, pair_id, faris, core, scenario_path,
                                     reference_run, breeder_run, assumptions,
                                     event_assumptions, sensitivity_grid, volume_reports))
        outage_records = [record for pair in branches for record in pair["outage_duration_studies"]]
        if len(outage_records) != 12:
            raise RuntimeError(f"outage-duration study produced {len(outage_records)} of 12 required runs")
        outage_summary = {
            "schema_version": "faris-outage-duration-study/v0.1",
            "status": "COMPLETED_AUTHORED_SCENARIO_PROBES_NOT_PHYSICAL_UNCERTAINTY",
            "axis": {"base_outage_duration_days": 30, "multipliers": list(OUTAGE_DURATION_MULTIPLIERS),
                     "resulting_duration_days": [15, 30, 60],
                     "fixed": ["outage start times", "outage spacing", "all non-duration assumptions",
                               "transport source rates", "scenario", "variant", "operating history horizon"],
                     "maximum_outage_below_annual_spacing": True},
            "scope": "One-factor authored scenario probes; levels are not probability distributions, physical uncertainty ranges, maintenance forecasts, or availability claims.",
            "baseline_refinement_report_sha256": sha256(
                Path(__file__).resolve().parents[1] / "references" / HISTORY_REFINEMENT_REPORT_NAME),
            "interpretation": "AUTHORED_SCENARIO_PROBE",
            "not_probability_distribution": True,
            "not_physical_uncertainty": True,
            "not_availability_estimate": True,
            "records": outage_records,
        }
        outage_summary_path = staging / "references" / "outage-duration-sensitivity-summary.json"
        write_bounded_json(outage_summary_path, outage_summary)
        sweep_manifest = add_sweep(staging, [path.resolve() for path in args.sweep_bundle])
        write_package_readme(staging, branches, support_manifest, sweep_manifest, args.version)
        prune_empty_directories(staging)
        indexed_files = scan_package(staging)
        part_totals = assign_parts(indexed_files, {"sweep": sweep_manifest})
        part_totals["evidence"]["archive_name"] = evidence_archive_name(args.version)
        expanded_total = sum(
            int(arrangement[key]["expanded_bytes"])
            for pair in branches for arrangement in pair["arrangements"]
            for key in ("case_archive", "workspace_archive"))
        expanded_file_count = sum(
            int(arrangement[key]["file_count"])
            for pair in branches for arrangement in pair["arrangements"]
            for key in ("case_archive", "workspace_archive"))
        expanded_directory_count = sum(
            int(arrangement[key]["directory_count"])
            for pair in branches for arrangement in pair["arrangements"]
            for key in ("case_archive", "workspace_archive"))
        if (expanded_total > MAX_EXPANDED_PACKAGE_BYTES
                or expanded_file_count > MAX_EXPANDED_PACKAGE_FILES
                or expanded_directory_count > MAX_EXPANDED_DIRECTORIES):
            raise RuntimeError("case/workspace archives exceed the expanded bytes/files/directories bounds")
        index = {
            "schema_version": "faris-recorded-demo-package/v0.5",
            "status": "IDENTITIES_REVALIDATED_CORE_EXECUTIONS_COMPLETED_PHYSICS_NOT_EVALUATED",
            "faris_cli_sha256": sha256(faris),
            "faris_app_sha256": sha256(app),
            "core_executable_sha256": sha256(core),
            "evidence_recorded_with": {
                "platform": runtime_manifest["platform"],
                "faris_cli_sha256": sha256(faris),
                "core_executable_sha256": sha256(core),
            },
            "local_runtime": runtime_manifest,
            "support": support_manifest,
            "operating_assumptions_sha256": sha256(staging / "operating-assumptions.json"),
            "event_assumptions": {
                "path": "inputs/event-assumptions.json",
                "sha256": sha256(inputs_dir / "event-assumptions.json"),
            },
            "sensitivity_grid": {
                "path": "inputs/sensitivity-grid.json",
                "sha256": sha256(inputs_dir / "sensitivity-grid.json"),
                "points_per_run": 27,
            },
            "outage_duration_sensitivity": {
                "path": outage_summary_path.relative_to(staging).as_posix(),
                "sha256": sha256(outage_summary_path),
                "case_count": len(outage_records),
                "multipliers": list(OUTAGE_DURATION_MULTIPLIERS),
                "duration_days": [15, 30, 60],
            },
            "package_file_count": len(indexed_files),
            "package_bytes": sum(item["bytes"] for item in indexed_files),
            "parts": part_totals,
            "expanded_case_workspace_bytes": expanded_total,
            "expanded_case_workspace_file_count": expanded_file_count,
            "expanded_case_workspace_member_count": sum(
                int(arrangement[key]["archive_member_count"])
                for pair in branches for arrangement in pair["arrangements"]
                for key in ("case_archive", "workspace_archive")),
            "expanded_case_workspace_directory_count": expanded_directory_count,
            "compressed_case_workspace_archive_bytes": sum(
                int(arrangement[key]["bytes"])
                for pair in branches for arrangement in pair["arrangements"]
                for key in ("case_archive", "workspace_archive")),
            "expanded_size_cap_bytes": MAX_EXPANDED_PACKAGE_BYTES,
            "expanded_file_count_cap": MAX_EXPANDED_PACKAGE_FILES,
            "expanded_archive_member_count_cap": MAX_EXPANDED_PACKAGE_FILES,
            "expanded_directory_count_cap": MAX_EXPANDED_DIRECTORIES,
            "per_tree_expanded_size_cap_bytes": MAX_TREE_BYTES,
            "per_tree_file_count_cap": MAX_TREE_FILES,
            "per_tree_archive_member_count_cap": MAX_TREE_MEMBERS,
            "per_tree_directory_count_cap": MAX_TREE_DIRECTORIES,
            "archive_path_component_count_cap": MAX_PATH_COMPONENTS,
            "scenario_pairs": branches,
            "sweep": sweep_manifest,
            "files": indexed_files,
            "index_digest_scope": "package-index.json and package-index.sha256 are excluded from files to avoid a self-referential digest.",
            "external_requirements": [
                "Compatible OpenMC/Python and nuclear data are external and needed for fresh transport.",
                "No OpenMC statepoint, HDF5, ENDF, ZIP, or nuclear-data file is bundled.",
                "Nuclear-data redistribution terms are unresolved; use the documented acquisition route and audit local files.",
            ],
            "scope_notice": "FARIS revalidation and Core receipts establish identity and workflow execution. All physical interpretations remain NOT_EVALUATED.",
        }
        index_path = staging / "package-index.json"
        index_path.write_text(json.dumps(index, indent=2) + "\n")
        (staging / "package-index.sha256").write_text(sha256(index_path) + "  package-index.json\n")
        for path in staging.rglob("*"):
            if path.is_file():
                with path.open("rb") as stream:
                    os.fsync(stream.fileno())
        staging.rename(output)
    except Exception:
        incomplete = output.with_name(
            f"{output.name}.incomplete-{datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ')}-{os.getpid()}"
        )
        staging.rename(incomplete)
        print(f"Incomplete evidence retained for review at {incomplete}")
        raise
    print(f"Created identity-checked four-run demo package: {output}")


if __name__ == "__main__":
    main()
