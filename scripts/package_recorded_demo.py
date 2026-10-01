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
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
SCRIPT_DIR = str(Path(__file__).resolve().parent)
if SCRIPT_DIR not in sys.path:
    sys.path.insert(0, SCRIPT_DIR)
from port_geometry_contract import validate_ownership_audits

REQUIRED_RESPONSES = {"total-tritium-production", "heating-total-whole-model"}
FORBIDDEN_SUFFIXES = {".h5", ".hdf5", ".endf", ".zip"}
ANALYSES = "breeding,shielding,fuel-history,electricity"
MAX_PACKAGE_FILE_BYTES = 64 * 1024 * 1024


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
        if len(records) > 2048 or total_bytes > 512 * 1024 * 1024:
            raise RuntimeError("recorded demo package exceeds 512 MiB or 2,048 files")
    return records


def add_pair(staging: Path, pair_id: str, faris: Path, core: Path,
             scenario_path: Path, reference_run: Path, breeder_run: Path,
             assumptions: Path, port_reports: tuple[Path, Path] | None) -> dict:
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
            volume_target = branch / "geometry" / f"{variant_id}-volume-check.json"
            volume_target.parent.mkdir()
            shutil.copyfile(port_reports[index], volume_target)
            volume_identity = {"path": volume_target.relative_to(staging).as_posix(),
                               "sha256": sha256(volume_target)}
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
        export_dir = branch / "core-exports" / variant_id
        export_result = run_json([str(core), "export", str(case), "--source-root",
                                  f"case={case}", "--out", str(export_dir)])
        if (export_result.get("schema_version") != "avila.core/export-report/v0.1-draft"
                or export_result.get("status") != "exported"
                or export_result.get("case_id") != evidence.get("expected_case_id")):
            raise RuntimeError(f"Core export did not verify the completed case {pair_id}/{variant_id}")
        exported_report = export_dir / "export-report.json"
        if not exported_report.is_file() or json.loads(exported_report.read_text()) != export_result:
            raise RuntimeError(f"Core export report did not match its published copy: {exported_report}")
        recorded_bundle = case / "inputs" / "recorded.json"
        if not recorded_bundle.is_file():
            raise RuntimeError(f"prepared Core case lacks portable transport bundle: {recorded_bundle}")
        bundle_path = branch / "bundles" / f"{variant_id}.transport-bundle.json"
        bundle_path.parent.mkdir(exist_ok=True)
        shutil.copyfile(recorded_bundle, bundle_path)
        descriptor_path = staging / f"saved-study-{pair_id}-{variant_id}.json"
        descriptor = {
            "case_directory": case.relative_to(staging).as_posix(),
            "execution_report": execution_report.relative_to(staging).as_posix(),
            "execution_workspace": workspace.relative_to(staging).as_posix(),
        }
        descriptor_path.write_text(json.dumps(descriptor, indent=2) + "\n", encoding="utf-8")
        run_summaries.append({
            "variant_id": variant_id,
            "run_record_sha256": sha256(run_path),
            "scenario_sha256": inspected["scenario_sha256"],
            "scientific_qualification": inspected["scientific_qualification"],
            "normalized_response_count": inspected["response_count"],
            "mesh_nonzero_flux_bin_count": inspected.get("mesh_nonzero_flux_bin_count"),
            "source_record_name": "run.json",
            "core_execution_report": execution_report.relative_to(staging).as_posix(),
            "core_execution_report_sha256": sha256(execution_report),
            "saved_case_inspection": inspection_path.relative_to(staging).as_posix(),
            "saved_case_inspection_sha256": sha256(inspection_path),
            "core_requirement_verdicts": verdict_states,
            "core_export_directory": export_dir.relative_to(staging).as_posix(),
            "core_export_sha256": export_result.get("export_sha256"),
            "core_export_report_sha256": sha256(exported_report),
            "saved_study_descriptor": descriptor_path.relative_to(staging).as_posix(),
            "saved_study_descriptor_sha256": sha256(descriptor_path),
            "transport_bundle": bundle_path.relative_to(staging).as_posix(),
            "transport_bundle_sha256": sha256(bundle_path),
            "port_volume_report": volume_identity,
            "port_geometry_ownership_report": ownership_identity,
        })
    return {
        "scenario_id": scenario_data.get("id"),
        "scenario_path": (branch / "scenario.json").relative_to(staging).as_posix(),
        "scenario_sha256": scenario_sha,
        "arrangements": run_summaries,
        "feature": "finite_port" if port_reports else "feature_free_control",
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--faris", required=True, type=Path)
    parser.add_argument("--core", required=True, type=Path)
    parser.add_argument("--control-scenario", required=True, type=Path)
    parser.add_argument("--control-reference-run", required=True, type=Path)
    parser.add_argument("--control-breeder-run", required=True, type=Path)
    parser.add_argument("--port-scenario", required=True, type=Path)
    parser.add_argument("--port-reference-run", required=True, type=Path)
    parser.add_argument("--port-breeder-run", required=True, type=Path)
    parser.add_argument("--port-reference-volume-report", required=True, type=Path)
    parser.add_argument("--port-breeder-volume-report", required=True, type=Path)
    parser.add_argument("--assumptions", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    files = [args.faris, args.core, args.control_scenario, args.control_reference_run,
             args.control_breeder_run, args.port_scenario, args.port_reference_run,
             args.port_breeder_run, args.port_reference_volume_report,
             args.port_breeder_volume_report, args.assumptions]
    for path in files:
        if not path.is_file():
            raise SystemExit(f"required file is missing: {path}")
    output = args.output.absolute()
    if output.exists():
        raise SystemExit(f"output already exists: {output}")
    faris, core = args.faris.resolve(), args.core.resolve()
    assumptions = args.assumptions.resolve()
    try:
        assumption_data = json.loads(assumptions.read_text())
        if not isinstance(assumption_data, dict):
            raise ValueError("operating assumptions must be a JSON object")
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
        branches = []
        for pair_id, (scenario_path, reference_run, breeder_run, volume_reports) in zip(
                ("control", "port"), pairs, strict=True):
            branches.append(add_pair(staging, pair_id, faris, core, scenario_path,
                                     reference_run, breeder_run, assumptions, volume_reports))
        (staging / "README.txt").write_text(
            "Recorded FARIS results and actual Core evidence. Inspect package-index.json. "
            "This bundle establishes artifact identity and workflow completion only. "
            "Scientific qualification remains NOT_EVALUATED. Nuclear data and statepoints "
            "are intentionally excluded; see the source repository's acquisition guide.\n")
        index = {
            "schema_version": "faris-recorded-demo-package/v0.3",
            "status": "IDENTITIES_REVALIDATED_CORE_EXECUTIONS_COMPLETED_PHYSICS_NOT_EVALUATED",
            "faris_cli_sha256": sha256(faris),
            "core_executable_sha256": sha256(core),
            "operating_assumptions_sha256": sha256(staging / "operating-assumptions.json"),
            "scenario_pairs": branches,
            "files": scan_package(staging),
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
