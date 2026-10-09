import importlib.util
import hashlib
import json
import os
from pathlib import Path
import shutil
import errno
import stat
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

HISTORY_REFINEMENT_REPORT_NAME = "operating-history-primary-refinement-v4.json"
SCRIPT = Path(__file__).with_name("verify_recorded_demo.py")
SPEC = importlib.util.spec_from_file_location("verify_recorded_demo", SCRIPT)
VERIFY = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(VERIFY)
sys.path.insert(0, str(SCRIPT.parent))
from port_geometry_contract import validate_ownership_audits
from recorded_bundle_contract import validate_recorded_bundle, inspect_sweep_bundle
import package_recorded_demo as PACKAGE


def write(path: Path, value: bytes | str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(value.encode() if isinstance(value, str) else value)


def fake_history_bytes(assumptions: dict, rates: dict) -> bytes:
    """What the fixture `faris history run` writes: the history the package does not ship."""
    return (json.dumps({
        "schema_version": "faris-history-result/v0.1", "assumptions": assumptions,
        "driving_rates": {"scenario_sha256": rates["scenario_sha256"],
                          "transport_artifact_sha256": rates["transport_artifact_sha256"]},
        "events": [{"kind": "planned_outage_started"}, {"kind": "planned_outage_ended"}],
        "snapshots": [{"time_s": 1.0}]}, sort_keys=True) + "\n").encode()


def apply_parts(index: dict) -> None:
    """Assign every inventoried file to its part and write the `parts` totals."""
    launched = VERIFY.launch_paths(index)
    for item in index["files"]:
        item["part"] = VERIFY.part_for(item["path"], launched)
    index["parts"] = {}
    for part in VERIFY.PARTS:
        members = [item for item in index["files"] if item["part"] == part]
        index["parts"][part] = {"file_count": len(members),
                                "bytes": sum(item["bytes"] for item in members)}
    index["parts"]["evidence"]["archive_name"] = "FARIS-0.0.1-evidence.tar.gz"


def reindex_package(root: Path) -> None:
    """Refresh a fixture inventory after a deliberate semantic mutation."""
    index_path = root / "package-index.json"
    index = json.loads(index_path.read_text())
    inventory = []
    for path in sorted(root.rglob("*")):
        if path.is_file() and path.name not in {"package-index.json", "package-index.sha256"}:
            inventory.append({"path": path.relative_to(root).as_posix(),
                              "bytes": path.stat().st_size, "sha256": VERIFY.digest(path)})
    index["files"] = inventory
    index["package_file_count"] = len(inventory)
    index["package_bytes"] = sum(item["bytes"] for item in inventory)
    apply_parts(index)
    write(index_path, json.dumps(index, indent=2) + "\n")
    write(root / "package-index.sha256", f"{VERIFY.digest(index_path)}  package-index.json\n")


def make_bundle(root: Path, pair: str, variant: str, scenario_bytes: bytes,
                seed: int | None = None,
                batch_values: bool = False) -> tuple[str, str, str, dict]:
    scenario_sha = VERIFY.digest(root / pair / "scenario.json").removeprefix("sha256:")
    input_value = {
        "schema_version": "faris-reactor-input/v0.1", "scenario_sha256": scenario_sha,
        "request": {"variant_id": variant, "scenario_sha256": scenario_sha},
        "physics": {"component_assignments": [{"component_id": "first-wall",
                                                   "material_id": "tungsten-natural"}]},
    }
    input_bytes = (json.dumps(input_value, sort_keys=True) + "\n").encode()
    audit_bytes = b"{}\n"
    adapter_bytes = b"test adapter bytes\n"
    batch_bytes = b'{"batches": 2, "values": [[1.0], [2.0]]}\n'
    batch_sha = hashlib.sha256(batch_bytes).hexdigest()
    artifact_value = {"schema_version": "faris-transport-artifact/v0.1",
                      "tallies": [{"response_id": "mesh-bin-0"}]}
    if batch_values:
        artifact_value["response_covariance"] = {"batch_values_sha256": batch_sha}
    artifact_bytes = (json.dumps(artifact_value, sort_keys=True) + "\n").encode()
    spectrum_items = [{"component_id": "first-wall", "particle": particle,
                       "energy_edges_ev": [0.0, 1.0e9], "mean_cm_per_source_per_bin": [1.0],
                       "standard_error_cm_per_source_per_bin": [0.1]}
                      for particle in ("neutron", "photon")]
    spectra_bytes = (json.dumps({"schema_version": "faris-transport-spectra/v0.1",
                                 "scenario_sha256": scenario_sha, "variant_id": variant,
                                 "input_sha256": hashlib.sha256(input_bytes).hexdigest(),
                                 "spectra": spectrum_items}, sort_keys=True) + "\n").encode()
    phis = [1.5707963267948966, 3.141592653589793, 4.71238898038469]
    thetas = [0.0, 1.5707963267948966, 3.141592653589793, 4.71238898038469]
    probes = []
    def probe(probe_id, cell, material, cell_id):
        probes.append({"probe_id": probe_id, "status": "PASS",
                       "expected_cell_name": cell, "observed_cell_name": cell,
                       "expected_cell_id": cell_id, "observed_cell_id": cell_id,
                       "expected_material_id": material,
                       "expected_openmc_material_name": None if material == "void" else material,
                       "observed_openmc_material_name": None if material == "void" else material,
                       "expected_openmc_material_id": None if material == "void" else 1,
                       "observed_openmc_material_id": None if material == "void" else 1,
                       "point_xyz_cm": [1.0, 2.0, 3.0]})
    for phi in phis:
        p = f"{phi:.8f}"
        probe(f"plasma-interior-phi-{p}", "plasma-source-domain", "void", 1)
        probe(f"clearance-near-plasma-phi-{p}", "plasma-first-wall-clearance", "void", 2)
        probe(f"clearance-near-first-wall-phi-{p}", "plasma-first-wall-clearance", "void", 2)
        probe(f"first-wall-near-inner-phi-{p}", "first-wall", "tungsten-natural", 3)
        probe(f"first-wall-near-outer-phi-{p}", "first-wall", "tungsten-natural", 3)
        for theta in thetas:
            probe(f"first-wall-mid-phi-{p}-theta-{theta:.8f}", "first-wall", "tungsten-natural", 3)
    geometry_audit = {
        "schema_version": "faris-openmc-geometry-ownership-audit/v0.1",
        "status": "PASS", "checks_are_geometry_only": True,
        "scientific_qualification": "NOT_EVALUATED",
        "scenario_sha256": scenario_sha, "variant_id": variant,
        "input_sha256": hashlib.sha256(input_bytes).hexdigest(),
        "probe_count": len(probes), "failed_probe_count": 0, "probes": probes,
        "clearance_status": "PASS", "plasma_radius_m": 1.0,
        "declared_plasma_to_first_wall_clearance_m": 0.08,
        "first_wall_inner_radius_m": 1.08,
        "toroidal_probe_directions_rad": phis,
        "cross_section_probe_directions_rad": thetas,
    }
    worker_value = {"geometry_ownership_audit": geometry_audit}
    if batch_values:
        worker_value["transport_batch_values_sha256"] = batch_sha
    worker_bytes = (json.dumps(worker_value, sort_keys=True) + "\n").encode()
    run_value = {
        "schema_version": "faris-reactor-run/v0.1", "scenario_sha256": scenario_sha,
        "variant_id": variant,
        "execution": {"execution_status": "SUCCEEDED"},
        **({} if seed is None else {"sampling": {"seed": seed}}),
        "input_sha256": hashlib.sha256(input_bytes).hexdigest(),
        "raw_artifact_sha256": hashlib.sha256(artifact_bytes).hexdigest(),
        "audit_sha256": hashlib.sha256(audit_bytes).hexdigest(),
        "adapter_sha256": hashlib.sha256(adapter_bytes).hexdigest(),
        "worker_result_sha256": hashlib.sha256(worker_bytes).hexdigest(),
        "transport_spectra_sha256": hashlib.sha256(spectra_bytes).hexdigest(),
        "normalized_spectra": [{"component_id": "first-wall", "particle": item["particle"]}
                               for item in spectrum_items],
        "mesh": {"id": "mesh", "dimensions": [1, 1, 1]},
        "normalized": {"results": [
            {"response_id": "total-tritium-production"},
            {"response_id": "heating-total-whole-model"},
            {"response_id": "mesh-bin-0", "mean": 1.0,
             "domain": {"kind": "mesh", "mesh_id": "mesh", "bin": 0}},
        ]},
    }
    run_bytes = (json.dumps(run_value, sort_keys=True) + "\n").encode()
    bundle = {"schema_version": "faris-recorded-transport-bundle/v0.1", "files": {
        "run.json": run_bytes.decode(), "input.json": input_bytes.decode(),
        "scenario.json": scenario_bytes.decode(), "audit.json": audit_bytes.decode(),
        "reactor_transport.py": adapter_bytes.decode(),
        "solver/transport-artifact.json": artifact_bytes.decode(),
        "solver/worker-result.json": worker_bytes.decode(),
        "solver/transport-spectra.json": spectra_bytes.decode(),
        **({"solver/transport-batch-values.json": batch_bytes.decode()} if batch_values else {}),
    }}
    bundle_rel = f"{pair}/bundles/{variant}.transport-bundle.json"
    write(root / bundle_rel, json.dumps(bundle, sort_keys=True) + "\n")
    summary = validate_recorded_bundle(
        bundle, scenario_sha256=scenario_sha, variant_id=variant,
        expected_run_sha256=hashlib.sha256(run_bytes).hexdigest(),
        expected_raw_artifact_sha256=hashlib.sha256(artifact_bytes).hexdigest(),
        mesh_nonzero_flux_bin_count=1)
    return (bundle_rel, "sha256:" + hashlib.sha256(run_bytes).hexdigest(),
            hashlib.sha256(artifact_bytes).hexdigest(), summary)


def make_operating_artifacts(root: Path, pair: str, scenario_sha: str,
                             arrangements: list[dict], assumptions: dict,
                             event_assumptions: dict, grid: dict) -> tuple[dict, list[dict], list[dict]]:
    events_index, sensitivity_index = [], []
    for arrangement in arrangements:
        variant = arrangement["variant_id"]
        identity = {"run_record_sha256": arrangement["run_record_sha256"],
                    "raw_artifact_sha256": arrangement["raw_artifact_sha256"],
                    "scenario_sha256": scenario_sha, "variant_id": variant}
        rates = {"scenario_sha256": scenario_sha,
                 "transport_artifact_sha256": identity["raw_artifact_sha256"]}
        history = fake_history_bytes(event_assumptions, rates)
        history_sha, history_bytes = "sha256:" + hashlib.sha256(history).hexdigest(), len(history)
        rates_rel = f"{pair}/event-histories/{variant}.rates.json"
        write(root / rates_rel, json.dumps(rates) + "\n")
        rates_sha = VERIFY.digest(root / rates_rel)
        event_provenance = {
            "schema_version": "faris-packaged-event-history-provenance/v0.2",
            **identity, "assumptions_sha256": VERIFY.digest(root / "inputs/event-assumptions.json"),
            "history_sha256": history_sha, "history_bytes": history_bytes, "rates_sha256": rates_sha,
        }
        event_prov_rel = f"{pair}/event-histories/{variant}.provenance.json"
        write(root / event_prov_rel, json.dumps(event_provenance, sort_keys=True) + "\n")
        events_index.append({"history_sha256": history_sha, "history_bytes": history_bytes,
                             "rates_path": rates_rel, "rates_sha256": rates_sha,
                             "provenance_path": event_prov_rel,
                             "provenance_sha256": VERIFY.digest(root / event_prov_rel),
                             "assumptions_sha256": event_provenance["assumptions_sha256"]})

        sensitivity = {
            "schema_version": "faris-history-sensitivity/v0.1", "grid": grid,
            "base_assumptions": assumptions,
            "driving_rates": {"scenario_sha256": scenario_sha,
                              "transport_artifact_sha256": identity["raw_artifact_sha256"]},
            "points": [{} for _ in range(27)],
        }
        sens_rel = f"{pair}/sensitivities/{variant}.json"
        write(root / sens_rel, json.dumps(sensitivity, sort_keys=True) + "\n")
        sens_sha = VERIFY.digest(root / sens_rel)
        sens_prov = {
            "schema_version": "faris-packaged-history-sensitivity-provenance/v0.1",
            **identity,
            "assumptions_sha256": VERIFY.digest(root / "operating-assumptions.json"),
            "grid_sha256": VERIFY.digest(root / "inputs/sensitivity-grid.json"),
            "sensitivity_sha256": sens_sha,
        }
        sens_prov_rel = f"{pair}/sensitivities/{variant}.provenance.json"
        write(root / sens_prov_rel, json.dumps(sens_prov, sort_keys=True) + "\n")
        sensitivity_index.append({"sensitivity_path": sens_rel, "sensitivity_sha256": sens_sha,
                                  "provenance_path": sens_prov_rel,
                                  "provenance_sha256": VERIFY.digest(root / sens_prov_rel),
                                  "point_count": 27})

    comparison = {
        "schema_version": "faris-history-comparison/v0.1",
        "left_label": "reference", "right_label": "breeder-emphasis",
        "left": {"driving_rates": {"scenario_sha256": scenario_sha,
                                     "transport_artifact_sha256": arrangements[0]["raw_artifact_sha256"]}},
        "right": {"driving_rates": {"scenario_sha256": scenario_sha,
                                      "transport_artifact_sha256": arrangements[1]["raw_artifact_sha256"]}},
    }
    comp_rel = f"{pair}/comparisons/reference-vs-breeder-emphasis.json"
    write(root / comp_rel, json.dumps(comparison, sort_keys=True) + "\n")
    comp_sha = VERIFY.digest(root / comp_rel)
    comp_prov = {
        "schema_version": "faris-packaged-history-comparison-provenance/v0.1",
        "comparison_sha256": comp_sha,
        "assumptions_sha256": VERIFY.digest(root / "operating-assumptions.json"),
        "scenario_sha256": scenario_sha,
        "left": {k: arrangements[0][k] for k in ("run_record_sha256", "raw_artifact_sha256")}
                | {"scenario_sha256": scenario_sha, "variant_id": "reference"},
        "right": {k: arrangements[1][k] for k in ("run_record_sha256", "raw_artifact_sha256")}
                 | {"scenario_sha256": scenario_sha, "variant_id": "breeder-emphasis"},
    }
    comp_prov_rel = f"{pair}/comparisons/reference-vs-breeder-emphasis.provenance.json"
    write(root / comp_prov_rel, json.dumps(comp_prov, sort_keys=True) + "\n")
    return ({"comparison_path": comp_rel, "comparison_sha256": comp_sha,
             "provenance_path": comp_prov_rel,
             "provenance_sha256": VERIFY.digest(root / comp_prov_rel)}, events_index, sensitivity_index)


def make_package(root: Path, faris: Path, core: Path,
                 core_source_repo: Path, core_source_revision: str) -> None:
    assumptions = {"horizon_s": 30 * 365.25 * 86400,
                   "planned_outages": [
                       {"start_s": 0, "end_s": 30 * 86400, "reason": "fixture"},
                       {"start_s": 365.25 * 86400,
                        "end_s": 395.25 * 86400, "reason": "fixture"},
                   ], "operating_days": 10, "service_limit": 0.9}
    event_assumptions = {"horizon_s": 30 * 365 * 86400, "planned_outages": [
        {"start_s": 0, "end_s": 30 * 86400, "reason": "fixture"},
        {"start_s": 365.25 * 86400, "end_s": 395.25 * 86400, "reason": "fixture"},
    ], "recovery_fraction": 0.5}
    grid = {"recovery_fraction_levels": [0.2, 0.5, 0.8],
            "delay_multipliers": [0.5, 1.0, 2.0],
            "service_limit_multipliers": [0.5, 1.0, 1.5], "rationale": "fixture grid"}
    # The release report is generated from release runs; the fixture only needs the same file name.
    refinement_report = root.parent / HISTORY_REFINEMENT_REPORT_NAME
    write(refinement_report, json.dumps({"schema_version": "faris-operating-history-primary-refinement-v4",
                                         "status": "software_fixture"}) + "\n")
    write(root / "operating-assumptions.json", json.dumps(assumptions) + "\n")
    write(root / "inputs/event-assumptions.json", json.dumps(event_assumptions) + "\n")
    write(root / "inputs/sensitivity-grid.json", json.dumps(grid) + "\n")
    pairs = []
    tree_sources = {}
    for pair in ("control", "port"):
        scenario = root / pair / "scenario.json"
        scenario_data = {"id": pair, "pair": pair}
        if pair == "port":
            scenario_data["penetration"] = {"kind": "outboard_rectangular_prism"}
        write(scenario, json.dumps(scenario_data) + "\n")
        scenario_sha = VERIFY.digest(scenario).removeprefix("sha256:")
        arrangements = []
        for variant in ("reference", "breeder-emphasis"):
            bundle_rel, run_record_hash, artifact_sha, bundle_summary = make_bundle(
                root, pair, variant, scenario.read_bytes())
            case = root.parent / "trees" / f"{pair}-{variant}-case"
            workspace = root.parent / "trees" / f"{pair}-{variant}-workspace"
            tree_sources[case.name] = case
            tree_sources[workspace.name] = workspace
            report = case / "execution-report.json"
            write(case / "case.marker", "case material\n")
            write(workspace / "receipt.json", "receipt material\n")
            write(report, "report material\n")
            inspection = {
                "schema_version": "faris-saved-case-inspection/v0.2",
                "record_integrity": "UNSIGNED_IDENTITY_REVALIDATED",
                "scenario_sha256": f"sha256:{scenario_sha}",
                "variant_id": variant,
                "case_id": f"{pair}-{variant}-case",
                "execution_status": "executed",
                "binding_status": "verified",
                "compiler_id": "avila.core/compiler-rust@0.1.0",
                "semantic_profile": "avila.core/semantic/0.2-draft",
                "compiler_executable_sha256": "sha256:" + "b" * 64,
                "core_executable_sha256": "sha256:" + "b" * 64,
                "requirement_verdicts": [{"status": "not_evaluated"}],
                "steps": [{"step_id": "transport"}],
                "verified_receipt_count": 1,
            }
            inspection_rel = f"{pair}/inspections/{variant}.json"
            write(root / inspection_rel, json.dumps(inspection, indent=2, sort_keys=True) + "\n")
            export_report_rel = f"{pair}/core-exports/{variant}/export-report.json"
            export_report = {
                "schema_version": "avila.core/export-report/v0.1-draft",
                "status": "exported", "case_id": f"{pair}-{variant}-case",
                "export_sha256": "sha256:" + hashlib.sha256(
                    (case / "case.marker").read_bytes()).hexdigest(),
            }
            write(root / export_report_rel, json.dumps(export_report))
            descriptor_rel = f"saved-study-{pair}-{variant}.json"
            descriptor = {"schema_version": "faris-saved-study-store/v0.1",
                          "store": "evidence-store",
                          "case_tree": case.name, "workspace_tree": workspace.name,
                          "execution_report_member": "execution-report.json"}
            descriptor_path = root / descriptor_rel
            write(descriptor_path, json.dumps(descriptor, indent=2) + "\n")
            arrangement = {
                "variant_id": variant,
                "scenario_sha256": scenario_sha,
                "core_execution_report_member": "execution-report.json",
                "core_execution_report_sha256": "sha256:" + hashlib.sha256(b"report material\n").hexdigest(),
                "case_tree": case.name,
                "workspace_tree": workspace.name,
                "run_record_sha256": run_record_hash,
                "raw_artifact_sha256": artifact_sha,
                "input_sha256": bundle_summary["input_sha256"],
                "sampling": {"batches": 10},
                "transport_bundle": bundle_rel,
                "transport_bundle_sha256": VERIFY.digest(root / bundle_rel),
                "mesh_nonzero_flux_bin_count": 1,
                "offline_field_and_spectrum_identity": bundle_summary,
                "scientific_qualification": "NOT_EVALUATED",
                "core_requirement_verdicts": ["not_evaluated"],
                "saved_study_descriptor": descriptor_rel,
                "saved_study_descriptor_sha256": VERIFY.digest(descriptor_path),
                "saved_case_inspection": inspection_rel,
                "saved_case_inspection_sha256": VERIFY.digest(root / inspection_rel),
                "core_export_report": export_report_rel,
                "core_export_report_sha256": VERIFY.digest(root / export_report_rel),
                "core_export_sha256": export_report["export_sha256"],
            }
            if pair == "port":
                scenario_sha_raw = scenario_sha
                input_sha = bundle_summary["input_sha256"]
                worker_sha = "sha256:" + bundle_summary["worker_result_sha256"]
                volume = {"schema_version": "faris-independent-port-volume-check/v0.1",
                          "geometry_check": "PASS", "transport_volume_check": "PASS",
                          "scientific_qualification": "NOT_EVALUATED", "variant_id": variant,
                          "scenario_sha256": scenario_sha,
                          "run_record_sha256": run_record_hash.removeprefix("sha256:"),
                          "transport_artifact_sha256": artifact_sha,
                          "worker_result_sha256": worker_sha.removeprefix("sha256:"),
                          "input_sha256": input_sha}
                volume_rel = f"{pair}/geometry/{variant}-volume-check.json"
                write(root / volume_rel, json.dumps(volume) + "\n")
                arrangement["port_volume_report"] = {
                    "path": volume_rel, "sha256": VERIFY.digest(root / volume_rel)}
                phis = [1.5707963267948966, 3.141592653589793, 4.71238898038469]
                thetas = [0.0, 1.5707963267948966, 3.141592653589793, 4.71238898038469]
                probes = []
                def add_probe(probe_id, cell, material):
                    probes.append({
                        "probe_id": probe_id, "status": "PASS",
                        "expected_cell_name": cell, "observed_cell_name": cell,
                        "expected_cell_id": 1 if cell == "plasma-source-domain" else 2,
                        "observed_cell_id": 1 if cell == "plasma-source-domain" else 2,
                        "expected_material_id": material,
                        "expected_openmc_material_name": None if material == "void" else material,
                        "observed_openmc_material_name": None if material == "void" else material,
                        "expected_openmc_material_id": None if material == "void" else 1,
                        "observed_openmc_material_id": None if material == "void" else 1,
                        "point_xyz_cm": [1.0, 2.0, 3.0],
                    })
                for phi in phis:
                    p = f"{phi:.8f}"
                    add_probe(f"plasma-interior-phi-{p}", "plasma-source-domain", "void")
                    add_probe(f"clearance-near-plasma-phi-{p}", "plasma-first-wall-clearance", "void")
                    add_probe(f"clearance-near-first-wall-phi-{p}", "plasma-first-wall-clearance", "void")
                    add_probe(f"first-wall-near-inner-phi-{p}", "first-wall", "tungsten-natural")
                    add_probe(f"first-wall-near-outer-phi-{p}", "first-wall", "tungsten-natural")
                    for theta in thetas:
                        add_probe(f"first-wall-mid-phi-{p}-theta-{theta:.8f}",
                                  "first-wall", "tungsten-natural")
                ownership_audit = {
                    "schema_version": "faris-openmc-geometry-ownership-audit/v0.1",
                    "status": "PASS", "checks_are_geometry_only": True,
                    "scientific_qualification": "NOT_EVALUATED",
                    "method": "OpenMC Geometry.find actual cell/material ownership at analytic probes; zero transport histories",
                    "scenario_sha256": scenario_sha_raw, "variant_id": variant,
                    "input_sha256": input_sha, "plasma_radius_m": 1.0,
                    "declared_plasma_to_first_wall_clearance_m": 0.08,
                    "first_wall_inner_radius_m": 1.08, "clearance_status": "PASS",
                    "toroidal_probe_directions_rad": phis,
                    "cross_section_probe_directions_rad": thetas,
                    "probe_count": len(probes), "failed_probe_count": 0,
                    "probes": probes,
                }
                penetration = {
                    "scenario_sha256": scenario_sha_raw, "variant_id": variant,
                    "cell_counts": {"first-wall": 2},
                    "final_port_void_confirmation_counts_by_component": {"first-wall": 2},
                    "not_a_physical_validation": True,
                    "fractional_volume_standard_errors_are_binomial": True,
                    "independent_of_Rust_midpoint_quadrature": True,
                    "samples": 100,
                }
                own_rel = f"{pair}/geometry/{variant}-ownership-audit.json"
                ownership = {
                    "schema_version": "faris-packaged-port-geometry-ownership/v0.1",
                    "worker_result_sha256": worker_sha,
                    "run_record_sha256": run_record_hash,
                    "transport_artifact_sha256": artifact_sha,
                    "input_sha256": input_sha,
                    "scenario_sha256": scenario_sha_raw,
                    "variant_id": variant,
                    "component_materials": {"first-wall": "tungsten-natural"},
                    "geometry_ownership_audit": ownership_audit,
                    "penetration_volume_audit": penetration,
                }
                write(root / own_rel, json.dumps(ownership) + "\n")
                arrangement["port_geometry_ownership_report"] = {
                    "path": own_rel, "sha256": VERIFY.digest(root / own_rel),
                    "worker_result_sha256": worker_sha}
            else:
                arrangement["port_volume_report"] = None
                arrangement["port_geometry_ownership_report"] = None
            arrangements.append(arrangement)
        comparison, events, sensitivities = make_operating_artifacts(
            root, pair, scenario_sha, arrangements, assumptions, event_assumptions, grid)
        for arrangement, event, sensitivity in zip(arrangements, events, sensitivities, strict=True):
            arrangement["event_history"] = event
            arrangement["sensitivity_study"] = sensitivity
        pairs.append({"scenario_id": pair, "scenario_path": f"{pair}/scenario.json",
                      "scenario_sha256": scenario_sha,
                      "feature": "finite_port" if pair == "port" else "feature_free_control",
                      "arrangements": arrangements,
                      "paired_history_comparison": comparison,
                      "event_histories": events,
                      "sensitivity_studies": sensitivities})
    outage_records = []
    base_event_path = root / "operating-assumptions.json"
    base_event = json.loads(base_event_path.read_text())
    for pair in pairs:
        pair_id = pair["scenario_path"].split("/", 1)[0]
        for arrangement in pair["arrangements"]:
            for multiplier in PACKAGE.OUTAGE_DURATION_MULTIPLIERS:
                adjusted = PACKAGE.scale_outage_durations(base_event, multiplier)
                factor = str(multiplier).replace(".", "p")
                directory = root / "outage-duration-sensitivity" / pair_id / arrangement["variant_id"] / f"multiplier-{factor}"
                assumptions_path = directory / "assumptions.json"
                write(assumptions_path, json.dumps(adjusted) + "\n")
                rates = {"scenario_sha256": pair["scenario_sha256"],
                         "transport_artifact_sha256": arrangement["raw_artifact_sha256"]}
                history = fake_history_bytes(adjusted, rates)
                history_sha = "sha256:" + hashlib.sha256(history).hexdigest()
                rates_path = directory / "rates.json"
                write(rates_path, json.dumps(rates) + "\n")
                provenance = {
                    "schema_version": "faris-outage-duration-provenance/v0.2",
                    "pair_id": pair_id, "variant_id": arrangement["variant_id"],
                    "duration_multiplier": multiplier,
                    "run_record_sha256": arrangement["run_record_sha256"],
                    "raw_artifact_sha256": arrangement["raw_artifact_sha256"],
                    "input_sha256": arrangement["input_sha256"],
                    "sampling": arrangement["sampling"],
                    "scenario_sha256": pair["scenario_sha256"],
                    "adjusted_assumptions_sha256": VERIFY.digest(assumptions_path),
                    "history_sha256": history_sha, "history_bytes": len(history),
                    "rates_sha256": VERIFY.digest(rates_path),
                    "base_operating_assumptions_sha256": VERIFY.digest(root / "operating-assumptions.json"),
                    "baseline_refinement_report_sha256": VERIFY.digest(refinement_report),
                    "baseline_anchor_history_sha256": history_sha.removeprefix("sha256:"),
                    "baseline_anchor_rates_sha256": VERIFY.digest(rates_path).removeprefix("sha256:"),
                    "interpretation": "AUTHORED_SCENARIO_PROBE",
                    "not_probability_distribution": True,
                    "not_physical_uncertainty": True,
                    "not_availability_estimate": True,
                }
                provenance_path = directory / "provenance.json"
                write(provenance_path, json.dumps(provenance) + "\n")
                outage_records.append({
                    "pair_id": pair_id, "variant_id": arrangement["variant_id"],
                    "duration_multiplier": multiplier,
                    "assumptions_path": assumptions_path.relative_to(root).as_posix(),
                    "assumptions_sha256": VERIFY.digest(assumptions_path),
                    "history_sha256": history_sha, "history_bytes": len(history),
                    "rates_path": rates_path.relative_to(root).as_posix(),
                    "rates_sha256": VERIFY.digest(rates_path),
                    "provenance_path": provenance_path.relative_to(root).as_posix(),
                    "provenance_sha256": VERIFY.digest(provenance_path),
                })
    outage_summary = {
        "schema_version": "faris-outage-duration-study/v0.2",
        "status": "COMPLETED_AUTHORED_SCENARIO_PROBES_NOT_PHYSICAL_UNCERTAINTY",
        "baseline_refinement_report_sha256": VERIFY.digest(refinement_report),
        "interpretation": "AUTHORED_SCENARIO_PROBE",
        "not_probability_distribution": True,
        "not_physical_uncertainty": True,
        "not_availability_estimate": True,
        "axis": {"multipliers": [0.5, 1.0, 2.0], "resulting_duration_days": [15, 30, 60],
                 "base_outage_duration_days": 30,
                 "fixed": ["outage start times", "outage spacing", "all non-duration assumptions",
                           "transport source rates", "scenario", "variant", "operating history horizon"],
                 "maximum_outage_below_annual_spacing": True},
        "scope": "Authored one-factor scenario probes; levels are not probability distributions.",
        "records": outage_records,
    }
    outage_summary_path = root / "references/outage-duration-sensitivity-summary.json"
    write(outage_summary_path, json.dumps(outage_summary) + "\n")
    app = root.parent / "faris-app-test"
    write(app, "#!/usr/bin/env python3\nimport sys\n"
               "if '--version' in sys.argv: print('faris-app 0.0.1'); raise SystemExit(0)\n")
    app.chmod(0o755)
    runtime = PACKAGE.install_local_runtime(
        root, faris, app, core, core_source_repo, core_source_revision,
        require_clean_faris_source=False)
    campaign_source = root.parent / "campaign-fixture.json"
    write(campaign_source, json.dumps({"status": "software_fixture",
                                      "raw_path": "/tmp/private/run.json"}) + "\n")
    support = PACKAGE.install_support(root, [
        ("fixture-campaign", campaign_source),
        ("history-refinement", refinement_report),
    ])
    (root / "README.md").write_text("Fixture demo package.\n")
    store_record = PACKAGE.pack_evidence_store(root, tree_sources)
    index = {"schema_version": "faris-recorded-demo-package/v0.7",
             "status": "IDENTITIES_REVALIDATED_CORE_EXECUTIONS_COMPLETED_PHYSICS_NOT_EVALUATED",
             "faris_cli_sha256": VERIFY.digest(faris),
             "faris_app_sha256": VERIFY.digest(app),
             "core_executable_sha256": VERIFY.digest(core),
             "evidence_recorded_with": {"platform": runtime["platform"],
                                        "faris_cli_sha256": VERIFY.digest(faris),
                                        "core_executable_sha256": VERIFY.digest(core)},
             "local_runtime": runtime,
             "support": support,
             "operating_assumptions_sha256": VERIFY.digest(root / "operating-assumptions.json"),
             "event_assumptions": {"path": "inputs/event-assumptions.json",
                                   "sha256": VERIFY.digest(root / "inputs/event-assumptions.json")},
             "sensitivity_grid": {"path": "inputs/sensitivity-grid.json",
                                  "sha256": VERIFY.digest(root / "inputs/sensitivity-grid.json"),
                                  "points_per_run": 27},
             "outage_duration_sensitivity": {
                 "path": outage_summary_path.relative_to(root).as_posix(),
                 "sha256": VERIFY.digest(outage_summary_path), "case_count": 12,
                 "multipliers": [0.5, 1.0, 2.0], "duration_days": [15, 30, 60]},
             "evidence_store": store_record,
             "scenario_pairs": pairs}
    inventory = []
    for path in sorted(root.rglob("*")):
        if path.is_file():
            inventory.append({"path": path.relative_to(root).as_posix(),
                              "bytes": path.stat().st_size, "sha256": VERIFY.digest(path)})
    index["files"] = inventory
    index["package_file_count"] = len(inventory)
    index["package_bytes"] = sum(item["bytes"] for item in inventory)
    apply_parts(index)
    index_path = root / "package-index.json"
    write(index_path, json.dumps(index, indent=2) + "\n")
    write(root / "package-index.sha256", f"{VERIFY.digest(index_path)}  package-index.json\n")


def maintenance_fixture(assumptions: bytes, history: bytes, version: str | None = "0.2.0",
                        status: str = "EVALUATED") -> dict:
    """A minimal faris-maintenance-result/v0.1 holding what the packager and verifier read."""
    def bare(data: bytes) -> str:
        return hashlib.sha256(data).hexdigest()
    result = {"schema_version": "faris-maintenance-result/v0.1",
              "inputs": {"assumptions": bare(assumptions), "designs": "0" * 64,
                         "design/a/history_assumptions": bare(history),
                         "design/b/history_assumptions": bare(history)},
              "designs": {"a": {"computed": {"status": status}},
                          "b": {"computed": {"status": "EVALUATED"}}}}
    if version is not None:
        result["produced_by"] = {"faris_version": version}
    return result


def add_maintenance(root: Path, result: dict | None = None, assumptions: bytes = b'{"m": 1}\n',
                    history: bytes = b'{"h": 1}\n', record: dict | None = None) -> None:
    """Add the builder and a recorded maintenance result to a fixture package and index them."""
    result = result if result is not None else maintenance_fixture(assumptions, history)
    write(root / "tools/build_activation_inputs.py", "# builder\n")
    write(root / "maintenance/maintenance-result.json", json.dumps(result) + "\n")
    write(root / "maintenance/maintenance-assumptions.json", assumptions)
    write(root / "maintenance/operating-assumptions.json", history)
    index_path = root / "package-index.json"
    index = json.loads(index_path.read_text())
    produced = result.get("produced_by")
    index["maintenance"] = record if record is not None else {
        "result": "maintenance/maintenance-result.json",
        "faris_version": produced["faris_version"] if produced else None}
    write(index_path, json.dumps(index, indent=2) + "\n")
    reindex_package(root)


def add_sweep(root: Path, variants: list[tuple[str, float]], seeds: list[int] | None = None) -> None:
    """Add a synthetic allocation sweep (variant, blanket m) and index it."""
    layers = lambda blanket: [{"id": "blanket", "thickness_m": blanket},
                              {"id": "shield", "thickness_m": round(0.9 - blanket, 6)}]
    scenario = {"id": "allocation-sweep", "variants": [
        {"id": name, "layers": layers(blanket)} for name, blanket in variants]}
    scenario_bytes = (json.dumps(scenario, sort_keys=True) + "\n").encode()
    write(root / "sweep/scenario.json", scenario_bytes)
    runs = []
    for position, (name, blanket) in enumerate(variants):
        seed = (seeds or list(range(100, 100 + len(variants))))[position]
        rel, run_sha, raw_sha, summary = make_bundle(root, "sweep", name, scenario_bytes, seed)
        runs.append({"variant_id": name, "blanket_thickness_m": blanket,
                     "shield_thickness_m": round(0.9 - blanket, 6), "seed": seed,
                     "run_record_sha256": run_sha, "raw_artifact_sha256": raw_sha,
                     "offline_field_and_spectrum_identity": summary,
                     "transport_bundle": rel, "transport_bundle_sha256": VERIFY.digest(root / rel)})
    index_path = root / "package-index.json"
    index = json.loads(index_path.read_text())
    index["sweep"] = {"scenario_id": "allocation-sweep", "scenario_path": "sweep/scenario.json",
                      "scenario_sha256": VERIFY.digest(root / "sweep/scenario.json").removeprefix("sha256:"),
                      "runs": runs}
    write(index_path, json.dumps(index, indent=2) + "\n")
    reindex_package(root)


class RecordedDemoPackageVerificationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.package = self.root / "package"
        self.package.mkdir()
        self.faris = self.root / "faris-test"
        write(self.faris, "#!/usr/bin/env python3\nimport hashlib,json,sys\nfrom pathlib import Path\n"
                          "if '--version' in sys.argv: print('faris 0.0.1'); raise SystemExit(0)\n"
                          "if sys.argv[1:3]==['history','run']:\n"
                          " arg=lambda n: sys.argv[sys.argv.index(n)+1]\n"
                          " a=json.loads(Path(arg('--assumptions')).read_text()); r=json.loads(Path(arg('--rates')).read_text())\n"
                          " Path(arg('--output')).write_text(json.dumps({'schema_version':'faris-history-result/v0.1','assumptions':a,'driving_rates':{'scenario_sha256':r['scenario_sha256'],'transport_artifact_sha256':r['transport_artifact_sha256']},'events':[{'kind':'planned_outage_started'},{'kind':'planned_outage_ended'}],'snapshots':[{'time_s':1.0}]},sort_keys=True)+'\\n'); raise SystemExit(0)\n"
                          "assert sys.argv[1:3]==['evidence','inspect'] and '--case' not in sys.argv\n"
                          "store=Path(sys.argv[sys.argv.index('--store')+1]); tree=sys.argv[sys.argv.index('--case-tree')+1]\n"
                          "assert (store/'store.json').is_file() and sys.argv[sys.argv.index('--workspace-tree')+1]==tree[:-4]+'workspace'\n"
                          "pair=tree.split('-')[0]; variant=tree[len(pair)+1:-len('-case')]\n"
                          "h=hashlib.sha256((store.parent/pair/'scenario.json').read_bytes()).hexdigest()\n"
                          "print(json.dumps({'schema_version':'faris-saved-case-inspection/v0.2','record_integrity':'UNSIGNED_IDENTITY_REVALIDATED','scenario_sha256':'sha256:'+h,'variant_id':variant,'case_id':tree,'execution_status':'executed','binding_status':'verified','compiler_id':'avila.core/compiler-rust@0.1.0','semantic_profile':'avila.core/semantic/0.2-draft','compiler_executable_sha256':'sha256:'+'b'*64,'core_executable_sha256':'sha256:'+'b'*64,'requirement_verdicts':[{'status':'not_evaluated'}],'steps':[{'step_id':'transport'}],'verified_receipt_count':1}))\n")
        self.faris.chmod(0o755)
        self.core = self.root / "core-test"
        write(self.core, "#!/usr/bin/env python3\nimport hashlib,json,sys\nfrom pathlib import Path\n"
                         "if '--version' in sys.argv: print('avila-core 0.1.0'); raise SystemExit(0)\n"
                         f"sys.path.insert(0,{str(Path(__file__).resolve().parent)!r}); import evidence_store\n"
                         "assert sys.argv[1:3]==['export','--report-only'] and '--out' not in sys.argv\n"
                         "store,tree=sys.argv[3].removeprefix('store:').rsplit('#',1)\n"
                         "assert sys.argv[sys.argv.index('--source-root')+1]=='case='+sys.argv[3]\n"
                         "marker=evidence_store.read_file(store,evidence_store.load_index(store),tree,'case.marker')\n"
                         "report={'schema_version':'avila.core/export-report/v0.1-draft',"
                         "'status':'exported','case_id':tree,"
                         "'export_sha256':'sha256:'+hashlib.sha256(marker).hexdigest()}\n"
                         "sys.stdout.write(json.dumps(report))\n")
        self.core.chmod(0o755)
        self.core_source_repo = self.root / "core-source"
        self.core_source_repo.mkdir()
        write(self.core_source_repo / "LICENSE", "AGPL fixture license\n")
        write(self.core_source_repo / "THIRD_PARTY_NOTICES.md", "fixture notices\n")
        write(self.core_source_repo / "LICENSES/NOTICE.txt", "third-party fixture notice\n")
        subprocess.run(["git", "init", "-q", str(self.core_source_repo)], check=True)
        subprocess.run(["git", "-C", str(self.core_source_repo), "config", "user.email",
                        "test@example.invalid"], check=True)
        subprocess.run(["git", "-C", str(self.core_source_repo), "config", "user.name",
                        "FARIS test"], check=True)
        subprocess.run(["git", "-C", str(self.core_source_repo), "remote", "add", "origin",
                        "https://example.invalid/avila-core.git"], check=True)
        subprocess.run(["git", "-C", str(self.core_source_repo), "add", "."], check=True)
        subprocess.run(["git", "-C", str(self.core_source_repo), "commit", "-qm", "fixture"], check=True)
        self.core_source_revision = subprocess.run(
            ["git", "-C", str(self.core_source_repo), "rev-parse", "HEAD"],
            capture_output=True, text=True, check=True).stdout.strip()
        make_package(self.package, self.faris, self.core,
                     self.core_source_repo, self.core_source_revision)

    def tearDown(self):
        self.temporary.cleanup()

    def test_relocated_copy_rehashes_and_reopens_four_cases(self):
        relocated = self.root / "relocated" / "demo"
        shutil.copytree(self.package, relocated)
        manifest = subprocess.run(
            [sys.executable, str(relocated / "scripts/verify_binary_manifest.py"), str(relocated)],
            text=True, capture_output=True, check=False,
        )
        self.assertEqual(manifest.returncode, 0, manifest.stderr)
        for script in ("verify.sh",):
            syntax = subprocess.run(["sh", "-n", str(relocated / script)],
                                    text=True, capture_output=True, check=False)
            self.assertEqual(syntax.returncode, 0, syntax.stderr)
        support_manifest = json.loads((relocated / "support/manifest.json").read_text())
        self.assertFalse(support_manifest["demo_acceptance_snapshot_included"])
        self.assertTrue(any(item["redacted_locations"] for item in support_manifest["files"]))
        campaign = json.loads((relocated / "support/campaigns/fixture-campaign.json").read_text())
        self.assertNotIn("/tmp/private/run.json", json.dumps(campaign))
        result = VERIFY.verify_package(relocated, self.faris, self.core)
        self.assertEqual(result["inspected_saved_case_count"], 4)

    def test_package_without_sweep_verifies(self):
        result = VERIFY.verify_package(self.package, self.faris, self.core)
        self.assertEqual(result["verified_sweep_bundle_count"], 0)
        self.assertFalse((self.package / "launch.sh").exists())
        self.assertFalse((self.package / "scripts/launch_recorded_demo.py").exists())

    def test_sweep_bundles_are_indexed_verified_and_in_the_app_part(self):
        add_sweep(self.package, [("blanket-030cm", 0.30), ("blanket-040cm", 0.40)])
        result = VERIFY.verify_package(self.package, self.faris, self.core)
        self.assertEqual(result["verified_sweep_bundle_count"], 2)
        index = json.loads((self.package / "package-index.json").read_text())
        indexed = {item["path"] for item in index["files"]}
        self.assertIn("sweep/bundles/blanket-030cm.transport-bundle.json", indexed)
        self.assertIn("sweep/scenario.json", indexed)
        parts = {item["path"]: item["part"] for item in index["files"]}
        self.assertEqual(parts["sweep/bundles/blanket-030cm.transport-bundle.json"], "app")
        self.assertEqual(parts["sweep/bundles/blanket-040cm.transport-bundle.json"], "app")
        self.assertEqual(parts["sweep/scenario.json"], "evidence")

    def test_tampered_sweep_bundle_is_refused_by_verifier(self):
        add_sweep(self.package, [("blanket-030cm", 0.30), ("blanket-040cm", 0.40)])
        victim = self.package / "sweep/bundles/blanket-040cm.transport-bundle.json"
        victim.chmod(0o644)
        content = bytearray(victim.read_bytes())
        content[len(content) // 2] ^= 0x01
        victim.write_bytes(content)
        with self.assertRaises(ValueError):
            VERIFY.verify_package(self.package, self.faris, self.core)

    def test_sweep_scenario_identity_is_bare_and_still_checked(self):
        add_sweep(self.package, [("blanket-030cm", 0.30)])
        index = json.loads((self.package / "package-index.json").read_text())
        self.assertFalse(index["sweep"]["scenario_sha256"].startswith("sha256:"))
        self.assertEqual(VERIFY.verify_sweep(self.package, index), 1)
        index["sweep"]["scenario_sha256"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "sweep scenario digest mismatch"):
            VERIFY.verify_sweep(self.package, index)

    def test_tamper_negative_control_flips_a_store_blob_and_both_layers_refuse_it(self):
        result = VERIFY.mutate_copy_for_negative_control(self.package, self.faris, self.core)
        self.assertTrue(result["tampered_copy_path"].startswith("evidence-store/blobs/"))
        # The package index's file digest catches the flipped byte; with the index
        # rewritten to match, the evidence store's own check still does.
        self.assertEqual(result["tamper_control"], "EXPECTED_REJECTION")
        self.assertEqual(result["store_tamper_control"], "EXPECTED_REJECTION")

    def test_reindexed_sweep_with_wrong_identity_is_refused(self):
        add_sweep(self.package, [("blanket-030cm", 0.30), ("blanket-040cm", 0.40)],
                  seeds=[7, 7])
        with self.assertRaisesRegex(ValueError, "seed"):
            VERIFY.verify_package(self.package, self.faris, self.core)
        shutil.rmtree(self.package / "sweep")
        add_sweep(self.package, [("blanket-030cm", 0.30)])
        index = json.loads((self.package / "package-index.json").read_text())
        index["sweep"]["runs"][0]["blanket_thickness_m"] = 0.31
        write(self.package / "package-index.json", json.dumps(index, indent=2) + "\n")
        reindex_package(self.package)
        with self.assertRaisesRegex(ValueError, "differs from its index entry"):
            VERIFY.verify_package(self.package, self.faris, self.core)

    def test_bundle_with_matching_batch_values_is_accepted_and_altered_values_are_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            scenario_bytes = b'{"id": "s"}\n'
            write(root / "p" / "scenario.json", scenario_bytes.decode())
            bundle_rel, _, _, _ = make_bundle(root, "p", "reference", scenario_bytes,
                                              batch_values=True)
            bundle = json.loads((root / bundle_rel).read_text())
            scenario_sha = VERIFY.digest(root / "p" / "scenario.json").removeprefix("sha256:")
            validate_recorded_bundle(bundle, scenario_sha256=scenario_sha, variant_id="reference",
                                     mesh_nonzero_flux_bin_count=1)
            bundle["files"]["solver/transport-batch-values.json"] = '{"batches": 2}\n'
            with self.assertRaisesRegex(ValueError, "per-batch values"):
                validate_recorded_bundle(bundle, scenario_sha256=scenario_sha,
                                         variant_id="reference", mesh_nonzero_flux_bin_count=1)

    def test_sweep_variant_must_match_pattern_and_scenario(self):
        scenario = {"variants": [{"id": "blanket-030cm", "layers": [
            {"id": "blanket", "thickness_m": 0.3}, {"id": "shield", "thickness_m": 0.6}]}]}
        scenario_bytes = (json.dumps(scenario) + "\n").encode()
        write(self.root / "x/scenario.json", scenario_bytes)
        sha = VERIFY.digest(self.root / "x/scenario.json").removeprefix("sha256:")
        for variant in ("reference", "blanket-070cm"):
            rel, *_ = make_bundle(self.root, "x", variant, scenario_bytes, 1)
            bundle = json.loads((self.root / rel).read_text())
            with self.assertRaises(ValueError, msg=variant):
                inspect_sweep_bundle(bundle, scenario_sha256=sha, scenario=scenario)

    def test_packager_copies_validates_and_tabulates_sweep_bundles(self):
        real = Path(__file__).resolve().parents[1] / PACKAGE.SWEEP_SCENARIO_RELATIVE
        real_bytes = real.read_bytes()
        write(self.root / "real/scenario.json", real_bytes)
        sources = []
        for position, variant in enumerate(("blanket-035cm", "blanket-030cm")):
            rel, *_ = make_bundle(self.root, "real", variant, real_bytes, 500 + position)
            sources.append(self.root / rel)
        staging = self.root / "staging-sweep"
        staging.mkdir()
        sweep = PACKAGE.add_sweep(staging, sources)
        self.assertEqual([run["variant_id"] for run in sweep["runs"]],
                         ["blanket-030cm", "blanket-035cm"])
        self.assertEqual(sweep["runs"][0]["blanket_thickness_m"], 0.3)
        self.assertEqual(sweep["runs"][0]["shield_thickness_m"], 0.6)
        self.assertTrue((staging / "sweep/bundles/blanket-035cm.transport-bundle.json").is_file())
        store = {"schema_version": "avila.core/evidence-store/v0.1", "uncompressed_bytes": 900,
                 "distinct_bytes": 300, "stored_bytes": 40, "blob_count": 7,
                 "trees": [{"name": "control-reference-case", "file_count": 4, "bytes": 500,
                            "directory_count": 1}]}
        PACKAGE.write_package_readme(staging, [], {}, sweep, "0.1.1", store)
        readme = (staging / "README.md").read_text()
        self.assertIn("evidence store", readme)
        self.assertIn("Nothing is expanded", readme)
        self.assertNotIn("archives", readme.split("## The two downloads")[1])
        self.assertIn("bin/faris-app", readme)
        self.assertIn("FARIS-0.1.1-evidence.tar.gz", readme)
        self.assertIn("unpack it into the same folder", readme)
        self.assertNotIn("launch.sh", readme)
        self.assertIn("| blanket-030cm | 0.3 | 0.6 |", readme)
        self.assertIn(sweep["runs"][1]["raw_artifact_sha256"], readme)
        self.assertIsNone(PACKAGE.add_sweep(self.root / "unused", []))
        with self.assertRaisesRegex(ValueError, "repeat a variant"):
            PACKAGE.check_sweep_set([{"variant_id": "a", "seed": 1}, {"variant_id": "a", "seed": 2}])
        # a bundle built for a different scenario is refused
        other = self.root / "other-staging"
        other.mkdir()
        add_sweep(self.package, [("blanket-030cm", 0.30)])
        with self.assertRaises(ValueError):
            PACKAGE.add_sweep(other, [self.package / "sweep/bundles/blanket-030cm.transport-bundle.json"])

    def test_outage_duration_axis_changes_only_interval_lengths(self):
        base = json.loads((self.package / "operating-assumptions.json").read_text())
        adjusted = PACKAGE.scale_outage_durations(base, 2.0)
        for original, changed in zip(base["planned_outages"], adjusted["planned_outages"], strict=True):
            self.assertEqual(changed["start_s"], original["start_s"])
            self.assertEqual(changed["end_s"] - changed["start_s"],
                             2 * (original["end_s"] - original["start_s"]))
        base["planned_outages"][1]["start_s"] = 20 * 86400
        with self.assertRaisesRegex(ValueError, "ordered, and non-overlapping"):
            PACKAGE.scale_outage_durations(base, 2.0)

    def test_outage_duration_replay_rejects_tampered_input_or_missing_driver(self):
        index = json.loads((self.package / "package-index.json").read_text())
        VERIFY.verify_outage_duration_study(self.package, index, self.faris)
        summary_path = self.package / index["outage_duration_sensitivity"]["path"]
        summary = json.loads(summary_path.read_text())
        target_record = summary["records"][0]
        assumptions_path = self.package / target_record["assumptions_path"]
        assumptions = json.loads(assumptions_path.read_text())
        assumptions["recovery_fraction"] = 0.1
        write(assumptions_path, json.dumps(assumptions) + "\n")
        with self.assertRaises(ValueError):
            VERIFY.verify_outage_duration_study(self.package, index, self.faris)

    def test_outage_duration_replay_rejects_wrong_anchor_after_valid_reindex(self):
        index = json.loads((self.package / "package-index.json").read_text())
        summary_path = self.package / index["outage_duration_sensitivity"]["path"]
        summary = json.loads(summary_path.read_text())
        record = next(item for item in summary["records"] if item["duration_multiplier"] == 1.0)
        provenance_path = self.package / record["provenance_path"]
        provenance = json.loads(provenance_path.read_text())
        provenance["baseline_anchor_history_sha256"] = "0" * 64
        write(provenance_path, json.dumps(provenance) + "\n")
        record["provenance_sha256"] = VERIFY.digest(provenance_path)
        write(summary_path, json.dumps(summary) + "\n")
        index["outage_duration_sensitivity"]["sha256"] = VERIFY.digest(summary_path)
        write(self.package / "package-index.json", json.dumps(index, indent=2) + "\n")
        reindex_package(self.package)
        # Establish that the rejection is semantic: the full file inventory is valid.
        valid_index, _ = VERIFY.verify_index(self.package, self.faris, self.core)
        with self.assertRaisesRegex(ValueError, "1.0 outage probe is not byte-bound"):
            VERIFY.verify_outage_duration_study(self.package, valid_index, self.faris)

    def reseal_outage_summary(self, index, summary):
        summary_path = self.package / index["outage_duration_sensitivity"]["path"]
        write(summary_path, json.dumps(summary) + "\n")
        index["outage_duration_sensitivity"]["sha256"] = VERIFY.digest(summary_path)
        write(self.package / "package-index.json", json.dumps(index, indent=2) + "\n")
        reindex_package(self.package)
        return VERIFY.verify_index(self.package, self.faris, self.core)[0]

    def test_outage_history_is_recomputed_not_shipped(self):
        index = json.loads((self.package / "package-index.json").read_text())
        self.assertFalse(list(self.package.rglob("history.json")))
        shipped = [path.name for path in self.package.glob("*/event-histories/*.json")
                   if not path.name.endswith((".rates.json", ".provenance.json"))]
        self.assertEqual(shipped, [])
        VERIFY.verify_outage_duration_study(self.package, index, self.faris)

    def test_outage_history_that_recomputes_differently_is_refused(self):
        index = json.loads((self.package / "package-index.json").read_text())
        summary_path = self.package / index["outage_duration_sensitivity"]["path"]
        summary = json.loads(summary_path.read_text())
        summary["records"][0]["history_sha256"] = "sha256:" + "0" * 64
        valid_index = self.reseal_outage_summary(index, summary)
        with self.assertRaisesRegex(ValueError, "recomputed to"):
            VERIFY.verify_outage_duration_study(self.package, valid_index, self.faris)

    def test_outage_history_size_must_match_the_recomputed_size(self):
        index = json.loads((self.package / "package-index.json").read_text())
        summary_path = self.package / index["outage_duration_sensitivity"]["path"]
        summary = json.loads(summary_path.read_text())
        summary["records"][0]["history_bytes"] += 1
        valid_index = self.reseal_outage_summary(index, summary)
        with self.assertRaisesRegex(ValueError, "recomputed to"):
            VERIFY.verify_outage_duration_study(self.package, valid_index, self.faris)

    def test_two_port_volume_reports_share_geometry_directory(self):
        staging = self.root / "staging"
        branch = staging / "port"
        first = self.root / "reference-volume.json"
        second = self.root / "breeder-volume.json"
        write(first, json.dumps({"variant_id": "reference", "transport_volume_check": "PASS"}) + "\n")
        write(second, json.dumps({"variant_id": "breeder-emphasis", "transport_volume_check": "PASS"}) + "\n")
        one = PACKAGE.copy_port_volume_report(staging, branch, "reference", first)
        two = PACKAGE.copy_port_volume_report(staging, branch, "breeder-emphasis", second)
        self.assertEqual((staging / one["path"]).read_bytes(), first.read_bytes())
        self.assertEqual((staging / two["path"]).read_bytes(), second.read_bytes())
        self.assertNotEqual(one["sha256"], two["sha256"])

    def test_parts_follow_the_assignment_rule(self):
        index = json.loads((self.package / "package-index.json").read_text())
        parts = {item["path"]: item["part"] for item in index["files"]}
        for relative in VERIFY.LAUNCH_PATHS + ("README.md", "SOURCE_PROVENANCE.md", "bin/faris-app",
                                               "bin/avila-core", "licenses/faris-LICENSE"):
            self.assertEqual(parts[relative], "app", relative)
        for relative in ("verify.sh", "scripts/verify_recorded_demo.py", "inputs/event-assumptions.json"):
            self.assertEqual(parts[relative], "evidence", relative)
        self.assertEqual(parts["evidence-store/store.json"], "evidence")
        self.assertTrue(any(path.startswith("evidence-store/blobs/") and part == "evidence"
                            for path, part in parts.items()))
        self.assertFalse(any(path.endswith(".tar.gz") for path in parts))
        self.assertEqual(index["local_runtime"]["launcher"], {"kind": "native", "executable": "faris-app"})
        self.assertEqual(index["local_runtime"]["platform"], VERIFY.rust_platform())
        self.assertEqual(index["parts"]["evidence"]["archive_name"], "FARIS-0.0.1-evidence.tar.gz")

    def test_packager_asserts_launch_paths_are_in_the_app_part(self):
        index = json.loads((self.package / "package-index.json").read_text())
        records = [{"path": item["path"], "bytes": item["bytes"]} for item in index["files"]]
        records.append({"path": "tools/build_activation_inputs.py", "bytes": 1})
        totals = PACKAGE.assign_parts(records, {"sweep": None})
        self.assertEqual(totals["app"]["file_count"] + totals["evidence"]["file_count"], len(records))
        for missing in ("control/scenario.json", "bin/faris-app", "tools/build_activation_inputs.py"):
            with self.assertRaisesRegex(RuntimeError, "not in the app part"):
                PACKAGE.assign_parts([item for item in records if item["path"] != missing], {"sweep": None})
        maintenance = {"maintenance": {"result": "maintenance/maintenance-result.json"}}
        with self.assertRaisesRegex(RuntimeError, "maintenance/maintenance-result.json"):
            PACKAGE.assign_parts(records, {"sweep": None, **maintenance})
        records.append({"path": "maintenance/maintenance-result.json", "bytes": 1})
        PACKAGE.assign_parts(records, {"sweep": None, **maintenance})
        self.assertEqual(records[-1]["part"], "app")

    def test_maintenance_record_is_in_the_app_part_and_verifies(self):
        add_maintenance(self.package)
        index, inventory = VERIFY.verify_index(self.package, self.faris, self.core)
        for path in ("maintenance/maintenance-result.json", "maintenance/maintenance-assumptions.json",
                     "maintenance/operating-assumptions.json", "tools/build_activation_inputs.py"):
            self.assertEqual(inventory[path]["part"], "app", path)
        self.assertEqual(index["maintenance"]["faris_version"], "0.2.0")
        self.assertTrue(VERIFY.verify_maintenance(self.package, index, inventory))
        result = VERIFY.verify_package(self.package, self.faris, self.core)
        self.assertTrue(result["maintenance_result_recorded"])

    def test_package_without_maintenance_still_verifies(self):
        index, inventory = VERIFY.verify_index(self.package, self.faris, self.core)
        self.assertNotIn("maintenance", index)
        self.assertFalse(VERIFY.verify_maintenance(self.package, index, inventory))

    def test_old_maintenance_result_without_produced_by_verifies(self):
        add_maintenance(self.package, maintenance_fixture(b'{"m": 1}\n', b'{"h": 1}\n', version=None))
        index, inventory = VERIFY.verify_index(self.package, self.faris, self.core)
        self.assertIsNone(index["maintenance"]["faris_version"])
        self.assertTrue(VERIFY.verify_maintenance(self.package, index, inventory))

    def test_verifier_refuses_inconsistent_maintenance_records(self):
        good_a, good_h = b'{"m": 1}\n', b'{"h": 1}\n'
        cases = {
            "assumptions hash": (maintenance_fixture(b"other", good_h), "assumptions hash differs"),
            "history hash": (maintenance_fixture(good_a, b"other"), "history_assumptions differs"),
            "not evaluated": (maintenance_fixture(good_a, good_h, status="NOT_EVALUATED"), "not EVALUATED"),
        }
        wrong_schema = maintenance_fixture(good_a, good_h)
        wrong_schema["schema_version"] = "faris-maintenance-result/v9"
        cases["schema"] = (wrong_schema, "schema")
        for label, (result, message) in cases.items():
            with self.subTest(label):
                package = self.root / f"bad-{label.replace(' ', '-')}"
                shutil.copytree(self.package, package)
                add_maintenance(package, result)
                index, inventory = VERIFY.verify_index(package, self.faris, self.core)
                with self.assertRaisesRegex(ValueError, message):
                    VERIFY.verify_maintenance(package, index, inventory)

    def test_verifier_refuses_a_mismatched_version_record_and_unindexed_maintenance_files(self):
        add_maintenance(self.package, record={"result": "maintenance/maintenance-result.json",
                                             "faris_version": "0.1.1"})
        index, inventory = VERIFY.verify_index(self.package, self.faris, self.core)
        with self.assertRaisesRegex(ValueError, "different FARIS version"):
            VERIFY.verify_maintenance(self.package, index, inventory)
        plain = self.root / "plain"
        shutil.copytree(self.package, plain)
        index_path = plain / "package-index.json"
        data = json.loads(index_path.read_text())
        del data["maintenance"]
        write(index_path, json.dumps(data, indent=2) + "\n")
        reindex_package(plain)
        index, inventory = VERIFY.verify_index(plain, self.faris, self.core)
        with self.assertRaisesRegex(ValueError, "declares no maintenance record"):
            VERIFY.verify_maintenance(plain, index, inventory)

    def test_packager_checks_and_installs_maintenance_inputs(self):
        good_a, good_h = b'{"m": 1}\n', b'{"h": 1}\n'
        files = {}
        for name, data in (("assumptions", good_a), ("history", good_h)):
            files[name] = self.root / f"{name}.json"
            write(files[name], data)

        def result_file(result: dict) -> Path:
            path = self.root / "result.json"
            write(path, json.dumps(result))
            return path
        good = result_file(maintenance_fixture(good_a, good_h))
        self.assertEqual(PACKAGE.check_maintenance_inputs(good, files["assumptions"], files["history"]), "0.2.0")
        refusals = [
            (maintenance_fixture(b"x", good_h), "assumptions hash"),
            (maintenance_fixture(good_a, b"x"), "history_assumptions"),
            (maintenance_fixture(good_a, good_h, status="NOT_EVALUATED"), "not EVALUATED"),
        ]
        for result, message in refusals:
            with self.assertRaisesRegex(SystemExit, message):
                PACKAGE.check_maintenance_inputs(result_file(result), files["assumptions"], files["history"])
        staging = self.root / "staging"
        staging.mkdir()
        record = PACKAGE.install_maintenance(staging, good, files["assumptions"], files["history"], "0.2.0")
        self.assertEqual(record, {"result": "maintenance/maintenance-result.json", "faris_version": "0.2.0"})
        self.assertEqual((staging / "maintenance/maintenance-assumptions.json").read_bytes(), good_a)
        self.assertEqual((staging / "maintenance/operating-assumptions.json").read_bytes(), good_h)
        self.assertTrue((staging / "tools/build_activation_inputs.py").is_file())
        bare = self.root / "bare"
        bare.mkdir()
        self.assertIsNone(PACKAGE.install_maintenance(bare, None, None, None, None))
        self.assertTrue((bare / "tools/build_activation_inputs.py").is_file())
        self.assertFalse((bare / "maintenance").exists())

    def test_packager_requires_all_three_maintenance_arguments(self):
        completed = subprocess.run(
            [sys.executable, str(SCRIPT.with_name("package_recorded_demo.py")),
             "--faris", "a", "--faris-app", "a", "--core", "a", "--core-source-repo", "a",
             "--core-source-revision", "a", "--control-scenario", "a", "--control-reference-run", "a",
             "--control-breeder-run", "a", "--port-scenario", "a", "--port-reference-run", "a",
             "--port-breeder-run", "a", "--port-reference-volume-report", "a",
             "--port-breeder-volume-report", "a", "--assumptions", "a", "--event-assumptions", "a",
             "--sensitivity-grid", "a", "--version", "0.2.0", "--output", str(self.root / "out"),
             "--maintenance-result", "a"],
            capture_output=True, text=True, check=False)
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("must be given together", completed.stderr)

    def test_empty_staging_directories_are_pruned(self):
        staging = self.root / "prune"
        (staging / "a/b/c").mkdir(parents=True)
        (staging / "a/keep").mkdir()
        write(staging / "a/keep/file.txt", "x\n")
        PACKAGE.prune_empty_directories(staging)
        self.assertEqual(sorted(p.relative_to(staging).as_posix() for p in staging.rglob("*")),
                         ["a", "a/keep", "a/keep/file.txt"])

    def test_platform_names_follow_rust(self):
        saved = VERIFY.sys.platform, VERIFY.platform.machine
        try:
            for system, machine, expected in (("linux", "x86_64", ("linux", "x86_64")),
                                              ("darwin", "arm64", ("macos", "aarch64")),
                                              ("win32", "AMD64", ("windows", "x86_64")),
                                              ("linux", "aarch64", ("linux", "aarch64"))):
                VERIFY.sys.platform = system
                VERIFY.platform.machine = lambda machine=machine: machine
                self.assertEqual(tuple(VERIFY.rust_platform().values()), expected)
            VERIFY.sys.platform = "freebsd"
            with self.assertRaises(ValueError):
                VERIFY.rust_platform()
        finally:
            VERIFY.sys.platform, VERIFY.platform.machine = saved

    def test_missing_evidence_part_names_the_archive_to_download(self):
        index = json.loads((self.package / "package-index.json").read_text())
        for item in index["files"]:
            if item["part"] == "evidence":
                path = self.package / item["path"]
                path.parent.chmod(0o755)
                path.unlink()
        with self.assertRaisesRegex(ValueError, "download FARIS-0.0.1-evidence.tar.gz"):
            VERIFY.verify_index(self.package, self.faris, self.core)

    def test_partly_missing_evidence_part_reports_the_count(self):
        index = json.loads((self.package / "package-index.json").read_text())
        evidence = [item for item in index["files"] if item["part"] == "evidence"]
        for item in evidence[:2]:
            (self.package / item["path"]).unlink()
        with self.assertRaisesRegex(ValueError, f"incomplete: 2 of {len(evidence)} evidence files"):
            VERIFY.verify_index(self.package, self.faris, self.core)

    def test_wrong_part_or_totals_are_refused(self):
        index_path = self.package / "package-index.json"
        original = index_path.read_text()
        index = json.loads(original)
        index["files"][0]["part"] = "app" if index["files"][0]["part"] == "evidence" else "evidence"
        write(index_path, json.dumps(index, indent=2) + "\n")
        write(self.package / "package-index.sha256", f"{VERIFY.digest(index_path)}  package-index.json\n")
        with self.assertRaisesRegex(ValueError, "wrong part"):
            VERIFY.verify_index(self.package, self.faris, self.core)
        index = json.loads(original)
        index["parts"]["app"]["bytes"] += 1
        write(index_path, json.dumps(index, indent=2) + "\n")
        write(self.package / "package-index.sha256", f"{VERIFY.digest(index_path)}  package-index.json\n")
        with self.assertRaisesRegex(ValueError, "app part totals"):
            VERIFY.verify_index(self.package, self.faris, self.core)

    def test_evidence_recorded_with_is_required_and_matches_the_pins_on_linux(self):
        index_path = self.package / "package-index.json"
        original = json.loads(index_path.read_text())
        self.assertEqual(original["evidence_recorded_with"],
                         {"platform": original["local_runtime"]["platform"],
                          "faris_cli_sha256": original["faris_cli_sha256"],
                          "core_executable_sha256": original["core_executable_sha256"]})
        for mutate in (lambda index: index.pop("evidence_recorded_with"),
                       lambda index: index["evidence_recorded_with"].update(faris_cli_sha256="sha256:" + "0" * 64),
                       lambda index: index["evidence_recorded_with"].update(
                           platform={"os": "linux", "arch": "aarch64"})):
            index = json.loads(json.dumps(original))
            mutate(index)
            write(index_path, json.dumps(index, indent=2) + "\n")
            write(self.package / "package-index.sha256", f"{VERIFY.digest(index_path)}  package-index.json\n")
            with self.assertRaisesRegex(ValueError, "evidence_recorded_with"):
                VERIFY.verify_index(self.package, self.faris, self.core)

    def make_ci_built(self, mutate=None):
        """Mark the fixture package as a retargeted Linux package: its pins are the CI build's
        programs, and evidence_recorded_with names other (laptop) programs."""
        index_path = self.package / "package-index.json"
        index = json.loads(index_path.read_text())
        index["desktop_build"] = {"schema_version": "faris-desktop-build/v0.1", "platform": "linux-x86_64",
                                  "executables": json.loads(json.dumps(index["local_runtime"]["executables"])),
                                  "faris_commit": index["local_runtime"]["source_provenance"]["faris"]["commit"],
                                  "core_commit": index["local_runtime"]["source_provenance"]["core"]["commit"]}
        index["evidence_recorded_with"] = {"platform": {"os": "linux", "arch": "x86_64"},
                                           "faris_cli_sha256": "sha256:" + "1" * 64,
                                           "core_executable_sha256": "sha256:" + "2" * 64}
        if mutate:
            mutate(index)
        write(index_path, json.dumps(index, indent=2) + "\n")
        write(self.package / "package-index.sha256", f"{VERIFY.digest(index_path)}  package-index.json\n")

    def test_plain_package_report_names_the_recording_programs(self):
        result = VERIFY.verify_package(self.package, self.faris, self.core)
        self.assertEqual(result["evidence_programs"], "PACKAGED_PROGRAMS_PRODUCED_THE_EVIDENCE")

    def test_ci_built_linux_package_may_differ_from_evidence_recorded_with(self):
        self.make_ci_built()
        index, _ = VERIFY.verify_index(self.package, self.faris, self.core)
        self.assertNotEqual(index["evidence_recorded_with"]["faris_cli_sha256"], index["faris_cli_sha256"])
        result = VERIFY.verify_package(self.package, self.faris, self.core)
        self.assertEqual(result["evidence_programs"], "CI_BUILD_OF_RECORDED_COMMITS_CHECKED_BY_REPRODUCTION")
        self.assertEqual(result["inspected_saved_case_count"], 4)

    def test_ci_built_linux_package_is_refused_unless_pins_equal_the_build(self):
        def other(name):
            return lambda index: index["desktop_build"]["executables"][name].update(sha256="sha256:" + "3" * 64)
        cases = {
            "faris pin differs": (other("faris"), "pins differ from the desktop_build"),
            "app pin differs": (other("faris-app"), "pins differ from the desktop_build"),
            "core pin differs": (other("avila-core"), "pins differ from the desktop_build"),
            "build missing a program": (lambda index: index["desktop_build"]["executables"].pop("faris"),
                                        "pins differ from the desktop_build"),
            "build is not a mapping": (lambda index: index.update(desktop_build="x"), "malformed"),
            "build for another platform": (lambda index: index["desktop_build"].update(platform="windows-x86_64"),
                                           "malformed"),
            "recorded_with missing": (lambda index: index.pop("evidence_recorded_with"), "evidence_recorded_with"),
            "build from another FARIS commit": (lambda index: index["desktop_build"].update(faris_commit="0" * 40),
                                                "recorded FARIS and Core commits"),
            "build from another Core commit": (lambda index: index["desktop_build"].update(core_commit="0" * 40),
                                               "recorded FARIS and Core commits"),
            "recorded_with other platform": (lambda index: index["evidence_recorded_with"].update(
                platform={"os": "linux", "arch": "aarch64"}), "malformed"),
            "recorded_with hash malformed": (lambda index: index["evidence_recorded_with"].update(
                faris_cli_sha256=7), "malformed"),
        }
        for label, (mutate, message) in cases.items():
            with self.subTest(label):
                self.make_ci_built(mutate)
                with self.assertRaisesRegex(ValueError, message):
                    VERIFY.verify_index(self.package, self.faris, self.core)

    def test_ci_built_linux_package_still_refuses_programs_that_are_not_its_pins(self):
        self.make_ci_built()
        stranger = self.package.parent / "stranger-faris"
        write(stranger, "#!/bin/sh\nexit 0\n")
        stranger.chmod(0o755)
        with self.assertRaisesRegex(ValueError, "differs from the package pin"):
            VERIFY.verify_index(self.package, stranger, self.core)

    # Verifies: PRV-005
    def test_tampered_copy_is_rejected_without_changing_source(self):
        original_hash = VERIFY.digest(self.package / "package-index.json")
        original_file = VERIFY.digest(self.package / "control" / "scenario.json")
        result = VERIFY.mutate_copy_for_negative_control(self.package, self.faris, self.core)
        self.assertEqual(result["tamper_control"], "EXPECTED_REJECTION")
        self.assertTrue(result["original_preserved"])
        self.assertEqual(VERIFY.digest(self.package / "package-index.json"), original_hash)
        self.assertEqual(VERIFY.digest(self.package / "control" / "scenario.json"), original_file)

    def test_cli_relocates_reopens_and_runs_tamper_negative_control(self):
        relocated = self.root / "relocated-cli" / "demo"
        original = VERIFY.digest(self.package / "control" / "scenario.json")
        result = subprocess.run(
            [str(self.package / "verify.sh")],
            text=True, capture_output=True, check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        output = json.loads(result.stdout)
        self.assertEqual(output["inspected_saved_case_count"], 4)
        self.assertEqual(output["tamper_control"], "EXPECTED_REJECTION")
        self.assertTrue(output["original_preserved"])
        self.assertFalse(any(self.package.rglob("__pycache__")))
        self.assertEqual(VERIFY.digest(self.package / "control" / "scenario.json"), original)

    # Verifies: SEC-002
    def test_index_rejects_extra_unindexed_file_and_path_traversal(self):
        extra = self.package / "unexpected.txt"
        write(extra, "unindexed\n")
        with self.assertRaises(ValueError):
            VERIFY.verify_package(self.package, self.faris, self.core)
        with self.assertRaises(ValueError):
            VERIFY.safe_package_path(self.package.resolve(), "../outside")

    def edit_index(self, edit, reindex=False):
        index_path = self.package / "package-index.json"
        index = json.loads(index_path.read_text())
        edit(index)
        write(index_path, json.dumps(index, indent=2) + "\n")
        if reindex:
            reindex_package(self.package)
        else:
            write(self.package / "package-index.sha256",
                  f"{VERIFY.digest(index_path)}  package-index.json\n")

    def test_the_package_index_records_the_store_and_the_trees_of_each_arrangement(self):
        index = json.loads((self.package / "package-index.json").read_text())
        record = index["evidence_store"]
        self.assertEqual(record["path"], "evidence-store")
        self.assertEqual(len(record["trees"]), 8)
        self.assertEqual(record["store_json_sha256"],
                         VERIFY.digest(self.package / "evidence-store/store.json"))
        names = {tree["name"] for tree in record["trees"]}
        for pair in index["scenario_pairs"]:
            for arrangement in pair["arrangements"]:
                self.assertIn(arrangement["case_tree"], names)
                self.assertIn(arrangement["workspace_tree"], names)
                self.assertNotIn("case_archive", arrangement)
        for key in ("expanded_case_workspace_bytes", "per_tree_archive_member_count_cap"):
            self.assertNotIn(key, index)
        self.assertFalse(list(self.package.rglob("*.tar.gz")))
        # Identical files across the eight trees share one blob.
        blobs = list((self.package / "evidence-store/blobs").rglob("*.xz"))
        self.assertEqual(len(blobs), record["blob_count"])
        self.assertLess(record["distinct_bytes"], record["uncompressed_bytes"])

    def test_verify_package_reports_the_store_it_checked(self):
        result = VERIFY.verify_package(self.package, self.faris, self.core)
        self.assertEqual(result["evidence_store_trees"], 8)
        self.assertEqual(result["archive_integrity_status"],
                         "STORE_BLOBS_AND_CORE_RECEIPTS_REVALIDATED")
        self.assertEqual([case["pair"] + "/" + case["variant"] for case in result["saved_cases"]],
                         ["control/reference", "control/breeder-emphasis",
                          "port/reference", "port/breeder-emphasis"])

    def test_scratch_space_covers_the_largest_recomputed_history_not_the_exports(self):
        index = json.loads((self.package / "package-index.json").read_text())
        needed = VERIFY.scratch_needed(self.package, index)
        summary = json.loads((self.package / index["outage_duration_sensitivity"]["path"]).read_text())
        largest = max([record["history_bytes"] for record in summary["records"]]
                      + [arrangement["event_history"]["history_bytes"]
                         for pair in index["scenario_pairs"] for arrangement in pair["arrangements"]])
        largest_case = max(tree["bytes"] for tree in index["evidence_store"]["trees"]
                           if tree["name"].endswith("-case"))
        self.assertGreaterEqual(needed, largest + 64 * 1024 * 1024)
        self.assertLess(needed, largest + 70 * 1024 * 1024 + 4096 * 64)
        self.assertLess(needed - largest, largest_case + 64 * 1024 * 1024 + 4096 * 64)

    def snapshot(self, root):
        """Every file's inode, mode and digest, so a write through a link shows up."""
        return {path.relative_to(root).as_posix(): (path.lstat().st_ino, path.lstat().st_mode,
                                                    VERIFY.digest(path))
                for path in sorted(root.rglob("*")) if path.is_file() and not path.is_symlink()}

    def test_link_tree_links_files_and_recreates_symlinks(self):
        source = self.root / "link-source"
        write(source / "a/file.txt", "payload\n")
        os.symlink("file.txt", source / "a/alias")
        target = self.root / "link-target"
        counts = VERIFY.link_tree(source, target)
        self.assertEqual(counts, {"linked": 1, "copied": 0})
        self.assertEqual((source / "a/file.txt").stat().st_ino, (target / "a/file.txt").stat().st_ino)
        self.assertTrue((target / "a/alias").is_symlink())
        self.assertEqual(os.readlink(target / "a/alias"), "file.txt")

    def test_link_tree_copies_when_links_are_refused_and_counts_them(self):
        source = self.root / "link-source"
        write(source / "one.txt", "1\n")
        write(source / "d/two.txt", "2\n")
        for code in (errno.EXDEV, errno.EPERM, errno.ENOTSUP):
            target = self.root / f"copy-target-{code}"
            with mock.patch.object(VERIFY.os, "link", side_effect=OSError(code, "refused")):
                counts = VERIFY.link_tree(source, target)
            self.assertEqual(counts, {"linked": 0, "copied": 2})
            self.assertEqual((target / "d/two.txt").read_text(), "2\n")
            self.assertNotEqual((source / "d/two.txt").stat().st_ino, (target / "d/two.txt").stat().st_ino)
        with mock.patch.object(VERIFY.os, "link", side_effect=OSError(errno.ENOENT, "gone")):
            with self.assertRaises(OSError):
                VERIFY.link_tree(source, self.root / "copy-target-other")

    def test_replace_file_breaks_the_link_and_leaves_the_source_alone(self):
        source = self.root / "replace-source"
        write(source / "blob", "original\n")
        (source / "blob").chmod(0o444)
        target = self.root / "replace-target"
        VERIFY.link_tree(source, target)
        VERIFY.replace_file(target / "blob", b"changed\n")
        self.assertEqual((source / "blob").read_text(), "original\n")
        self.assertEqual(stat.S_IMODE((source / "blob").stat().st_mode), 0o444)
        self.assertEqual((target / "blob").read_text(), "changed\n")
        self.assertNotEqual((source / "blob").stat().st_ino, (target / "blob").stat().st_ino)
        self.assertEqual([path.name for path in target.iterdir()], ["blob"])

    def test_tamper_controls_with_linked_copies_leave_the_source_untouched(self):
        index = json.loads((self.package / "package-index.json").read_text())
        blob = next(item["path"] for item in sorted(index["files"], key=lambda item: item["path"])
                    if item["path"].startswith("evidence-store/blobs/"))
        (self.package / blob).chmod(0o444)  # read-only, as shipped: the hardest case for a link
        before = self.snapshot(self.package)
        result = VERIFY.mutate_copy_for_negative_control(self.package, self.faris, self.core, self.root)
        self.assertEqual(before, self.snapshot(self.package))
        self.assertEqual(stat.S_IMODE((self.package / blob).stat().st_mode), 0o444)
        self.assertEqual(result["store_tamper_control"], "EXPECTED_REJECTION")
        self.assertGreater(result["tamper_copies"]["linked"], 0)
        self.assertEqual(result["tamper_copies"]["copied"], 0)
        self.assertEqual([path.name for path in self.root.iterdir() if path.name.startswith(".faris-")], [])

    def test_a_write_through_a_link_would_be_caught_by_the_source_guard(self):
        real = VERIFY.replace_file
        def writes_through(path, data):
            path.write_bytes(data)  # the bug the helper exists to prevent
        with mock.patch.object(VERIFY, "replace_file", writes_through):
            with self.assertRaisesRegex(ValueError, "modified the source package"):
                VERIFY.mutate_copy_for_negative_control(self.package, self.faris, self.core, self.root)
        self.assertIs(VERIFY.replace_file, real)

    def test_scratch_requirement_is_small_on_the_same_file_system_and_full_across_file_systems(self):
        index = json.loads((self.package / "package-index.json").read_text())
        linked = VERIFY.scratch_requirement(self.package, index, self.root)
        self.assertLess(linked, VERIFY.scratch_needed(self.package, index) + 64 * 1024 * 1024)
        with mock.patch.object(VERIFY, "same_file_system", return_value=False):
            across = VERIFY.scratch_requirement(self.package, index, self.root)
        self.assertEqual(across - linked, index["package_bytes"] - VERIFY.copy_allowance(self.package, index))

    def test_verify_sh_scratch_is_beside_the_package_and_falls_back_when_not_writable(self):
        script = (self.package / "verify.sh").read_text()
        self.assertIn('mktemp -d "$(dirname -- "$root")/.faris-verify.XXXXXX"', script)
        result = subprocess.run([str(self.package / "verify.sh")], text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        output = json.loads(result.stdout)
        self.assertGreater(output["relocation"]["linked"], 0)
        self.assertEqual(output["relocation"]["copied"], 0)
        self.assertEqual([path.name for path in self.root.iterdir() if path.name.startswith(".faris-")], [])
        if os.geteuid() == 0:
            self.skipTest("a read-only parent folder is still writable for root")
        # Package one level down so its parent can be made read-only.
        parent = self.root / "readonly-parent"
        parent.mkdir()
        shutil.copytree(self.package, parent / "package")
        parent.chmod(0o555)
        try:
            result = subprocess.run([str(parent / "package/verify.sh")], text=True, capture_output=True)
        finally:
            parent.chmod(0o755)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("not writable; using the temporary folder", result.stderr)
        self.assertEqual(json.loads(result.stdout)["tamper_control"], "EXPECTED_REJECTION")

    def test_low_free_space_refuses_with_the_amount_and_the_next_step(self):
        free = shutil.disk_usage(tempfile.gettempdir()).free
        original = VERIFY.shutil.disk_usage
        VERIFY.shutil.disk_usage = lambda path: type("U", (), {"free": 1024})()
        try:
            with self.assertRaisesRegex(ValueError, "bytes free.*TMPDIR"):
                VERIFY.verify_package(self.package, self.faris, self.core)
        finally:
            VERIFY.shutil.disk_usage = original
        self.assertGreater(free, 1024)

    def test_an_unindexed_extra_blob_or_a_missing_blob_is_refused(self):
        extra = self.package / "evidence-store/blobs/ab"
        write(extra / ("ab" + "0" * 62 + ".xz"), b"stray")
        with self.assertRaises(ValueError):
            VERIFY.verify_package(self.package, self.faris, self.core)
        (extra / ("ab" + "0" * 62 + ".xz")).unlink()
        extra.rmdir()
        victim = next((self.package / "evidence-store/blobs").rglob("*.xz"))
        victim.chmod(0o644)
        victim.unlink()
        reindex_package(self.package)
        with self.assertRaisesRegex(ValueError, "evidence store files differ"):
            VERIFY.verify_package(self.package, self.faris, self.core)

    def test_store_totals_trees_and_descriptors_must_match_the_index(self):
        def totals(index):
            index["evidence_store"]["distinct_bytes"] += 1
        def trees(index):
            index["evidence_store"]["trees"].pop()
        def digest(index):
            index["evidence_store"]["store_json_sha256"] = "sha256:" + "0" * 64
        def names(index):
            index["scenario_pairs"][0]["arrangements"][0]["case_tree"] = "other-case"
        def schema(index):
            index["evidence_store"]["schema_version"] = "avila.core/evidence-store/v9"
        for edit, message in ((totals, "totals differ"), (trees, "totals differ"),
                              (digest, "digest differs"), (names, "tree name differs"),
                              (schema, "lacks its evidence store record")):
            with self.subTest(edit.__name__):
                package = self.root / f"copy-{edit.__name__}"
                shutil.copytree(self.package, package)
                index_path = package / "package-index.json"
                index = json.loads(index_path.read_text())
                edit(index)
                write(index_path, json.dumps(index, indent=2) + "\n")
                write(package / "package-index.sha256",
                      f"{VERIFY.digest(index_path)}  package-index.json\n")
                with self.assertRaisesRegex(ValueError, message):
                    VERIFY.verify_package(package, self.faris, self.core)
        descriptor = self.package / "saved-study-control-reference.json"
        value = json.loads(descriptor.read_text())
        value["workspace_tree"] = "control-breeder-emphasis-workspace"
        descriptor.chmod(0o644)
        write(descriptor, json.dumps(value, indent=2) + "\n")
        reindex_package(self.package)
        self.edit_index(lambda i: i["scenario_pairs"][0]["arrangements"][0].__setitem__(
            "saved_study_descriptor_sha256", VERIFY.digest(descriptor)))
        with self.assertRaisesRegex(ValueError, "descriptor"):
            VERIFY.verify_package(self.package, self.faris, self.core)

    def test_a_changed_stored_report_is_refused_even_when_the_store_is_consistent(self):
        index = json.loads((self.package / "package-index.json").read_text())
        index["scenario_pairs"][0]["arrangements"][0]["core_execution_report_sha256"] = "sha256:" + "1" * 64
        write(self.package / "package-index.json", json.dumps(index, indent=2) + "\n")
        reindex_package(self.package)
        with self.assertRaisesRegex(ValueError, "execution report changed"):
            VERIFY.verify_package(self.package, self.faris, self.core)

    def test_exports_are_replayed_from_the_store_without_unpacking_any_tree(self):
        unpacked = []
        original = VERIFY.evidence_store.unpack_store
        VERIFY.evidence_store.unpack_store = lambda *args, **kwargs: unpacked.append(args)
        try:
            VERIFY.verify_package(self.package, self.faris, self.core)
        finally:
            VERIFY.evidence_store.unpack_store = original
        self.assertEqual(unpacked, [])

    def test_a_stored_export_report_that_differs_by_one_byte_is_refused(self):
        index = json.loads((self.package / "package-index.json").read_text())
        arrangement = index["scenario_pairs"][0]["arrangements"][0]
        report_path = self.package / arrangement["core_export_report"]
        report_path.chmod(0o644)
        # Same JSON value, different bytes: only the exact comparison can refuse it.
        write(report_path, report_path.read_text() + "\n")
        arrangement["core_export_report_sha256"] = VERIFY.digest(report_path)
        write(self.package / "package-index.json", json.dumps(index, indent=2) + "\n")
        reindex_package(self.package)
        with self.assertRaisesRegex(ValueError, "no longer reconstructs"):
            VERIFY.verify_package(self.package, self.faris, self.core)

    # Verifies: GEO-022
    def test_port_geometry_contract_rejects_material_in_clearance(self):
        ownership_path = self.package / "port" / "geometry" / "reference-ownership-audit.json"
        ownership = json.loads(ownership_path.read_text())
        kwargs = {
            "scenario_sha256": ownership["scenario_sha256"],
            "variant_id": ownership["variant_id"],
            "input_sha256": ownership["input_sha256"],
            "component_materials": ownership["component_materials"],
        }
        validate_ownership_audits(ownership["geometry_ownership_audit"],
                                  ownership["penetration_volume_audit"], **kwargs)
        clearance = next(probe for probe in ownership["geometry_ownership_audit"]["probes"]
                         if probe["probe_id"].startswith("clearance-near-plasma-"))
        clearance["observed_openmc_material_name"] = "tungsten-natural"
        clearance["observed_openmc_material_id"] = 1
        with self.assertRaises(ValueError):
            validate_ownership_audits(ownership["geometry_ownership_audit"],
                                      ownership["penetration_volume_audit"], **kwargs)

    # Verifies: GEO-046
    def test_bundle_rejects_oversized_mesh_before_range_allocation(self):
        bundle_path = self.package / "control" / "bundles" / "reference.transport-bundle.json"
        bundle = json.loads(bundle_path.read_text())
        run = json.loads(bundle["files"]["run.json"])
        run["mesh"]["dimensions"] = [32_769, 1, 1]
        bundle["files"]["run.json"] = json.dumps(run, sort_keys=True) + "\n"
        with self.assertRaisesRegex(ValueError, "32,768-bin"):
            validate_recorded_bundle(
                bundle, scenario_sha256=run["scenario_sha256"], variant_id=run["variant_id"],
                expected_run_sha256=hashlib.sha256(
                    bundle["files"]["run.json"].encode()).hexdigest(),
                expected_raw_artifact_sha256=run["raw_artifact_sha256"],
                mesh_nonzero_flux_bin_count=1)


if __name__ == "__main__":
    unittest.main()
