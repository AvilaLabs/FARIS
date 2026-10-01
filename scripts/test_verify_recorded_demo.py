import importlib.util
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("verify_recorded_demo.py")
SPEC = importlib.util.spec_from_file_location("verify_recorded_demo", SCRIPT)
VERIFY = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(VERIFY)
sys.path.insert(0, str(SCRIPT.parent))
from port_geometry_contract import validate_ownership_audits
from recorded_bundle_contract import validate_recorded_bundle
import package_recorded_demo as PACKAGE


def write(path: Path, value: bytes | str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(value.encode() if isinstance(value, str) else value)


def make_bundle(root: Path, pair: str, variant: str, scenario_bytes: bytes) -> tuple[str, str, str, dict]:
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
    artifact_bytes = (json.dumps({"schema_version": "faris-transport-artifact/v0.1",
                                  "tallies": [{"response_id": "mesh-bin-0"}]},
                                 sort_keys=True) + "\n").encode()
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
    worker_bytes = (json.dumps({"geometry_ownership_audit": geometry_audit},
                               sort_keys=True) + "\n").encode()
    run_value = {
        "schema_version": "faris-reactor-run/v0.1", "scenario_sha256": scenario_sha,
        "variant_id": variant, "execution": {"execution_status": "SUCCEEDED"},
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
            {"response_id": "mesh-bin-0", "domain": {"kind": "mesh", "mesh_id": "mesh", "bin": 0}},
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
        history = {
            "schema_version": "faris-history-result/v0.1",
            "assumptions": event_assumptions,
            "driving_rates": {"scenario_sha256": scenario_sha,
                              "transport_artifact_sha256": identity["raw_artifact_sha256"]},
            "events": [{"kind": "planned_outage_started"}, {"kind": "planned_outage_ended"}],
            "snapshots": [{"time_s": 1.0}],
        }
        history_rel = f"{pair}/event-histories/{variant}.json"
        rates_rel = f"{pair}/event-histories/{variant}.rates.json"
        write(root / history_rel, json.dumps(history, sort_keys=True) + "\n")
        write(root / rates_rel, json.dumps({"transport_artifact_sha256": identity["raw_artifact_sha256"]}) + "\n")
        history_sha, rates_sha = VERIFY.digest(root / history_rel), VERIFY.digest(root / rates_rel)
        event_provenance = {
            "schema_version": "faris-packaged-event-history-provenance/v0.1",
            **identity, "assumptions_sha256": VERIFY.digest(root / "inputs/event-assumptions.json"),
            "history_sha256": history_sha, "rates_sha256": rates_sha,
        }
        event_prov_rel = f"{pair}/event-histories/{variant}.provenance.json"
        write(root / event_prov_rel, json.dumps(event_provenance, sort_keys=True) + "\n")
        events_index.append({"history_path": history_rel, "history_sha256": history_sha,
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
    write(root / "operating-assumptions.json", json.dumps(assumptions) + "\n")
    write(root / "inputs/event-assumptions.json", json.dumps(event_assumptions) + "\n")
    write(root / "inputs/sensitivity-grid.json", json.dumps(grid) + "\n")
    pairs = []
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
            case = root / pair / "cases" / variant
            workspace = root / pair / "core-workspaces" / variant
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
            write(root / export_report_rel, json.dumps(export_report) + "\n")
            case_archive = PACKAGE.archive_tree(root, root / pair, variant, "case", case)
            workspace_archive = PACKAGE.archive_tree(root, root / pair, variant, "workspace", workspace)
            descriptor_rel = f"saved-study-{pair}-{variant}.json"
            descriptor = {"schema_version": "faris-saved-study-archive/v0.1",
                          "case_archive": case_archive, "workspace_archive": workspace_archive,
                          "execution_report_member": "execution-report.json"}
            descriptor_path = root / descriptor_rel
            write(descriptor_path, json.dumps(descriptor, indent=2) + "\n")
            shutil.rmtree(case)
            shutil.rmtree(workspace)
            arrangement = {
                "variant_id": variant,
                "scenario_sha256": scenario_sha,
                "core_execution_report_member": "execution-report.json",
                "core_execution_report_sha256": "sha256:" + hashlib.sha256(b"report material\n").hexdigest(),
                "case_archive": case_archive,
                "workspace_archive": workspace_archive,
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
                history = {
                    "schema_version": "faris-history-result/v0.1",
                    "assumptions": adjusted,
                    "driving_rates": {"scenario_sha256": pair["scenario_sha256"],
                                      "transport_artifact_sha256": arrangement["raw_artifact_sha256"]},
                    "events": [{"kind": "planned_outage_started"}],
                    "snapshots": [{"time_s": 1.0}],
                }
                history_path, rates_path = directory / "history.json", directory / "rates.json"
                write(history_path, json.dumps(history) + "\n")
                write(rates_path, json.dumps({"transport_artifact_sha256": arrangement["raw_artifact_sha256"]}) + "\n")
                provenance = {
                    "schema_version": "faris-outage-duration-provenance/v0.1",
                    "pair_id": pair_id, "variant_id": arrangement["variant_id"],
                    "duration_multiplier": multiplier,
                    "run_record_sha256": arrangement["run_record_sha256"],
                    "raw_artifact_sha256": arrangement["raw_artifact_sha256"],
                    "input_sha256": arrangement["input_sha256"],
                    "sampling": arrangement["sampling"],
                    "scenario_sha256": pair["scenario_sha256"],
                    "adjusted_assumptions_sha256": VERIFY.digest(assumptions_path),
                    "history_sha256": VERIFY.digest(history_path),
                    "rates_sha256": VERIFY.digest(rates_path),
                    "base_operating_assumptions_sha256": VERIFY.digest(root / "operating-assumptions.json"),
                    "baseline_refinement_report_sha256": VERIFY.digest(
                        Path(__file__).resolve().parents[1] / "references/operating-history-primary-refinement-v3.json"),
                    "baseline_anchor_history_sha256": VERIFY.digest(history_path),
                    "baseline_anchor_rates_sha256": VERIFY.digest(rates_path),
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
                    "history_path": history_path.relative_to(root).as_posix(),
                    "history_sha256": VERIFY.digest(history_path),
                    "rates_path": rates_path.relative_to(root).as_posix(),
                    "rates_sha256": VERIFY.digest(rates_path),
                    "provenance_path": provenance_path.relative_to(root).as_posix(),
                    "provenance_sha256": VERIFY.digest(provenance_path),
                })
    outage_summary = {
        "schema_version": "faris-outage-duration-study/v0.1",
        "status": "COMPLETED_AUTHORED_SCENARIO_PROBES_NOT_PHYSICAL_UNCERTAINTY",
        "baseline_refinement_report_sha256": VERIFY.digest(
            Path(__file__).resolve().parents[1] / "references/operating-history-primary-refinement-v3.json"),
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
    write(app, "#!/usr/bin/env python3\nimport json,os,sys,time\nfrom pathlib import Path\n"
               "if '--version' in sys.argv: print('faris-app 0.0.1'); raise SystemExit(0)\n"
               "if os.environ.get('FARIS_TEST_EXIT_EARLY'): raise SystemExit(int(os.environ['FARIS_TEST_EXIT_EARLY']))\n"
               "args=sys.argv[1:]; ds=[]\n"
               "marker=Path(args[args.index('--saved-study-ready-marker')+1]); deadline=time.monotonic()+10\n"
               "while not marker.exists() and time.monotonic()<deadline: time.sleep(.01)\n"
               "assert marker.is_file(); readiness=json.loads(marker.read_text())\n"
               "assert readiness['schema_version']=='faris-recorded-materialization/v0.1'\n"
               "if os.environ.get('FARIS_TEST_EXPECT_FAILED'):\n"
               " assert readiness['status']=='FAILED' and len(readiness.get('error','').encode())<=1024; raise SystemExit(9)\n"
               "assert readiness['status']=='COMPLETE'\n"
               "for i,item in enumerate(args[:-1]):\n"
               " if item=='--saved-study':\n"
               "  p=Path(args[i+1]); d=json.loads(p.read_text()); base=p.parent; c=(base/d['case_directory']).resolve(); w=(base/d['execution_workspace']).resolve()\n"
               "  assert (c/'case.marker').is_file() and (w/'receipt.json').is_file()\n"
               "  ds.append({'descriptor':str(p),'exists_during_launch':True})\n"
               "runs=Path(args[args.index('--runs-directory')+1]); runs.mkdir(parents=True,exist_ok=True); (runs/'fake-app-output.json').write_text('{\\\"created\\\":true}\\n')\n"
               "log=os.environ.get('FARIS_TEST_ARGS'); Path(log).write_text(json.dumps({'saved':ds,'args':args,'core':args[args.index('--core')+1],'runs':str(runs)})) if log else None\n")
    app.chmod(0o755)
    runtime = PACKAGE.install_local_runtime(
        root, faris, app, core, core_source_repo, core_source_revision,
        require_clean_faris_source=False)
    campaign_source = root.parent / "campaign-fixture.json"
    write(campaign_source, json.dumps({"status": "software_fixture",
                                      "raw_path": "/tmp/private/run.json"}) + "\n")
    support = PACKAGE.install_support(root, [
        ("fixture-campaign", campaign_source),
        ("history-refinement", Path(__file__).resolve().parents[1]
         / "references/operating-history-primary-refinement-v3.json"),
    ])
    (root / "README.md").write_text("Fixture demo package.\n")
    expanded_records = [arrangement[key]
                        for pair in pairs for arrangement in pair["arrangements"]
                        for key in ("case_archive", "workspace_archive")]
    index = {"schema_version": "faris-recorded-demo-package/v0.4",
             "status": "IDENTITIES_REVALIDATED_CORE_EXECUTIONS_COMPLETED_PHYSICS_NOT_EVALUATED",
             "faris_cli_sha256": VERIFY.digest(faris),
             "faris_app_sha256": VERIFY.digest(app),
             "core_executable_sha256": VERIFY.digest(core),
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
             "expanded_case_workspace_bytes": sum(item["expanded_bytes"] for item in expanded_records),
             "expanded_case_workspace_file_count": sum(item["file_count"] for item in expanded_records),
             "expanded_case_workspace_member_count": sum(item["archive_member_count"] for item in expanded_records),
             "expanded_case_workspace_directory_count": sum(item["directory_count"] for item in expanded_records),
             "compressed_case_workspace_archive_bytes": sum(item["bytes"] for item in expanded_records),
             "expanded_size_cap_bytes": 1536 * 1024 * 1024,
             "expanded_file_count_cap": 8192,
             "expanded_archive_member_count_cap": 8192,
             "expanded_directory_count_cap": 8192,
             "per_tree_expanded_size_cap_bytes": 512 * 1024 * 1024,
             "per_tree_file_count_cap": 2048,
             "per_tree_archive_member_count_cap": 4096,
             "per_tree_directory_count_cap": 1024,
             "archive_path_component_count_cap": 64,
             "scenario_pairs": pairs}
    inventory = []
    for path in sorted(root.rglob("*")):
        if path.is_file():
            inventory.append({"path": path.relative_to(root).as_posix(),
                              "bytes": path.stat().st_size, "sha256": VERIFY.digest(path)})
    index["files"] = inventory
    index["package_file_count"] = len(inventory)
    index["package_bytes"] = sum(item["bytes"] for item in inventory)
    index_path = root / "package-index.json"
    write(index_path, json.dumps(index, indent=2) + "\n")
    write(root / "package-index.sha256", f"{VERIFY.digest(index_path)}  package-index.json\n")


class RecordedDemoPackageVerificationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.package = self.root / "package"
        self.package.mkdir()
        self.faris = self.root / "faris-test"
        write(self.faris, "#!/usr/bin/env python3\nimport hashlib,json,sys\nfrom pathlib import Path\nif '--version' in sys.argv: print('faris 0.0.1'); raise SystemExit(0)\ncase=Path(sys.argv[sys.argv.index('--case')+1])\npair=case.parents[1].name\nscenario=case.parents[1]/'scenario.json'\nh=hashlib.sha256(scenario.read_bytes()).hexdigest()\nvariant=case.name\nprint(json.dumps({'schema_version':'faris-saved-case-inspection/v0.2','record_integrity':'UNSIGNED_IDENTITY_REVALIDATED','scenario_sha256':'sha256:'+h,'variant_id':variant,'case_id':pair+'-'+variant+'-case','execution_status':'executed','binding_status':'verified','compiler_id':'avila.core/compiler-rust@0.1.0','semantic_profile':'avila.core/semantic/0.2-draft','compiler_executable_sha256':'sha256:'+'b'*64,'core_executable_sha256':'sha256:'+'b'*64,'requirement_verdicts':[{'status':'not_evaluated'}],'steps':[{'step_id':'transport'}],'verified_receipt_count':1}))\n")
        self.faris.chmod(0o755)
        self.core = self.root / "core-test"
        write(self.core, "#!/usr/bin/env python3\nimport hashlib,json,sys\nfrom pathlib import Path\n"
                         "if '--version' in sys.argv: print('avila-core 0.1.0'); raise SystemExit(0)\n"
                         "case=Path(sys.argv[2]); out=Path(sys.argv[sys.argv.index('--out')+1])\n"
                         "pair=case.parents[1].name; variant=case.name\n"
                         "report={'schema_version':'avila.core/export-report/v0.1-draft',"
                         "'status':'exported','case_id':pair+'-'+variant+'-case',"
                         "'export_sha256':'sha256:'+hashlib.sha256((case/'case.marker').read_bytes()).hexdigest()}\n"
                         "out.mkdir(parents=True); (out/'export-report.json').write_text(json.dumps(report)+'\\n')\n"
                         "print(json.dumps(report))\n")
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
        for script in ("launch.sh", "verify.sh"):
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
        VERIFY.verify_outage_duration_study(self.package, index)
        summary_path = self.package / index["outage_duration_sensitivity"]["path"]
        summary = json.loads(summary_path.read_text())
        target_record = summary["records"][0]
        assumptions_path = self.package / target_record["assumptions_path"]
        assumptions = json.loads(assumptions_path.read_text())
        assumptions["recovery_fraction"] = 0.1
        write(assumptions_path, json.dumps(assumptions) + "\n")
        with self.assertRaises(ValueError):
            VERIFY.verify_outage_duration_study(self.package, index)

    def test_launcher_keeps_private_materialization_alive_for_app_and_cleans_it(self):
        log = self.root / "app-arguments.json"
        state_home = self.root / "state-home"
        environment = dict(os.environ, FARIS_TEST_ARGS=str(log), XDG_STATE_HOME=str(state_home))
        index_bytes = (self.package / "package-index.json").read_bytes()
        result = subprocess.run(
            [str(self.package / "launch.sh")], env=environment, text=True, capture_output=True, check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        data = json.loads(log.read_text())
        self.assertEqual(len(data["saved"]), 4)
        self.assertTrue(data["core"].endswith("bin/avila-core"))
        self.assertIn("--saved-study-ready-marker", data["args"])
        self.assertIn("--runs-directory", data["args"])
        self.assertFalse(Path(data["runs"]).is_relative_to(self.package.resolve()))
        self.assertTrue((Path(data["runs"]) / "fake-app-output.json").is_file())
        for item in data["saved"]:
            self.assertTrue(item["exists_during_launch"])
            self.assertFalse(Path(item["descriptor"]).exists())
        self.assertIn("materialized", result.stderr)
        self.assertEqual((self.package / "package-index.json").read_bytes(), index_bytes)
        self.assertFalse(any(self.package.rglob("__pycache__")))
        index = json.loads(index_bytes)
        for item in index["files"]:
            self.assertEqual(VERIFY.digest(self.package / item["path"]), item["sha256"])

    def test_launcher_cancels_materialization_and_cleans_when_app_exits(self):
        environment = dict(os.environ, FARIS_TEST_EXIT_EARLY="7")
        before = set(Path(tempfile.gettempdir()).glob("faris-recorded-demo-*"))
        result = subprocess.run(
            [sys.executable, str(SCRIPT.with_name("launch_recorded_demo.py")),
             str(self.package)], env=environment, text=True, capture_output=True, check=False,
        )
        self.assertEqual(result.returncode, 7, result.stderr)
        after = set(Path(tempfile.gettempdir()).glob("faris-recorded-demo-*"))
        self.assertEqual(after, before)

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

    def test_launcher_publishes_failed_marker_and_cleans_on_bad_archive(self):
        bad_package = self.root / "bad-archive-package"
        shutil.copytree(self.package, bad_package)
        index_path = bad_package / "package-index.json"
        index = json.loads(index_path.read_text())
        arrangement = index["scenario_pairs"][0]["arrangements"][0]
        archive = arrangement["case_archive"]
        archive_path = bad_package / archive["path"]
        archive_path.write_bytes(b"not a gzip archive")
        archive["bytes"] = archive_path.stat().st_size
        archive["sha256"] = VERIFY.digest(archive_path)
        manifest_path = bad_package / archive["manifest_path"]
        manifest = json.loads(manifest_path.read_text())
        manifest["archive_bytes"] = archive["bytes"]
        manifest["archive_sha256"] = archive["sha256"]
        write(manifest_path, json.dumps(manifest, sort_keys=True) + "\n")
        archive["manifest_sha256"] = VERIFY.digest(manifest_path)
        descriptor_path = bad_package / arrangement["saved_study_descriptor"]
        saved_descriptor = json.loads(descriptor_path.read_text())
        saved_descriptor["case_archive"] = archive
        write(descriptor_path, json.dumps(saved_descriptor, indent=2, sort_keys=True) + "\n")
        arrangement["saved_study_descriptor_sha256"] = VERIFY.digest(descriptor_path)
        for item in index["files"]:
            path = bad_package / item["path"]
            item["bytes"] = path.stat().st_size
            item["sha256"] = VERIFY.digest(path)
        index["package_bytes"] = sum(item["bytes"] for item in index["files"])
        index["compressed_case_workspace_archive_bytes"] = sum(
            item[f"{kind}_archive"]["bytes"]
            for pair in index["scenario_pairs"] for item in pair["arrangements"]
            for kind in ("case", "workspace"))
        write(index_path, json.dumps(index, indent=2, sort_keys=True) + "\n")
        write(bad_package / "package-index.sha256",
              f"{VERIFY.digest(index_path)}  package-index.json\n")
        environment = dict(os.environ, FARIS_TEST_EXPECT_FAILED="1")
        before = set(Path(tempfile.gettempdir()).glob("faris-recorded-demo-*"))
        result = subprocess.run(
            [sys.executable, str(SCRIPT.with_name("launch_recorded_demo.py")),
             str(bad_package)], env=environment, text=True, capture_output=True, check=False,
        )
        self.assertEqual(result.returncode, 9, result.stderr)
        self.assertIn("materialization failed", result.stderr)
        after = set(Path(tempfile.gettempdir()).glob("faris-recorded-demo-*"))
        self.assertEqual(after, before)

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

    def test_launcher_rejects_runs_directory_inside_read_only_package(self):
        environment = dict(os.environ, XDG_STATE_HOME=str(self.root / "state-home"))
        result = subprocess.run(
            [sys.executable, str(SCRIPT.with_name("launch_recorded_demo.py")),
             str(self.package), "--runs-directory", str(self.package / "user-runs")],
            env=environment, text=True, capture_output=True, check=False,
        )
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn("outside the read-only distribution", result.stderr)

    def test_index_rejects_extra_unindexed_file_and_path_traversal(self):
        extra = self.package / "unexpected.txt"
        write(extra, "unindexed\n")
        with self.assertRaises(ValueError):
            VERIFY.verify_package(self.package, self.faris, self.core)
        with self.assertRaises(ValueError):
            VERIFY.safe_package_path(self.package.resolve(), "../outside")

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
