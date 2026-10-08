//! Read-only, bounded identity revalidation for saved Core case evidence.
//!
//! This validates hashes and report consistency. The saved metadata is
//! unsigned: matching digests do not authenticate its author or qualify physics.

use crate::{
    core_evidence::{CoreEvidenceRun, RecordedTransportBundle},
    history::TransportDrivingRates,
    reactor::ReactorError,
};
use faris_model::history::OperatingHistoryAssumptions;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
};

const MAX_PACKAGE_FILES: usize = 2048;
const MAX_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_REPORT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_TREE_ENTRIES: usize = 16_384;

#[derive(Clone, Debug, Serialize)]
pub struct SavedRequirementVerdict {
    pub requirement_id: String,
    /// Preserved Core value: pass, fail, inconclusive, or not_evaluated.
    pub status: String,
    pub evidence_ids: Vec<String>,
    pub reason_codes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct VerifiedStepSummary {
    pub step_id: String,
    pub receipt_sha256: String,
    pub verified_input_count: usize,
    pub verified_output_count: usize,
    pub verified_log_count: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct SavedCaseInspection {
    pub schema_version: String,
    pub record_integrity: String,
    pub case_id: String,
    pub scenario_id: String,
    pub scenario_sha256: String,
    pub variant_id: String,
    pub compiled_snapshot_sha256: String,
    pub package_sha256: String,
    pub faris_executable_sha256: String,
    pub core_executable_sha256: String,
    pub compiler_id: String,
    pub semantic_profile: String,
    pub compiler_executable_sha256: String,
    pub compilation_status: String,
    pub execution_status: String,
    pub binding_status: String,
    pub campaign_status: String,
    pub requirement_verdicts: Vec<SavedRequirementVerdict>,
    pub steps: Vec<VerifiedStepSummary>,
    pub verified_package_document_count: usize,
    pub verified_package_artifact_count: usize,
    pub verified_receipt_count: usize,
    pub verified_stage_input_count: usize,
    pub verified_stage_output_count: usize,
    pub verified_log_count: usize,
    pub history_snapshot_count: Option<usize>,
    pub history_event_count: Option<usize>,
    pub history_result_sha256: Option<String>,
    pub history_assumptions_sha256: Option<String>,
    #[serde(skip_serializing)]
    pub history_assumptions: Option<OperatingHistoryAssumptions>,
    #[serde(skip_serializing)]
    pub history_result: Option<crate::history::HistoryResult>,
    pub limitations: Vec<String>,
    pub scope_notice: String,
}

struct LoadedHistory {
    assumptions: Option<OperatingHistoryAssumptions>,
    result: Option<crate::history::HistoryResult>,
    result_sha256: Option<String>,
    assumptions_sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct VerifiedCompilation {
    snapshot_sha256: String,
    compiler_id: String,
    semantic_profile: String,
    executable_sha256: String,
}

/// Verify a prepared Core case, the saved FARIS/Core run report, and its
/// read-only execution workspace. No file is created, removed, or modified.
pub fn inspect_saved_case(
    case_directory: &Path,
    execution_report_path: &Path,
    execution_workspace: &Path,
) -> Result<SavedCaseInspection, ReactorError> {
    let case_root = case_directory.canonicalize()?;
    if !case_root.is_dir() {
        return Err("saved case root must be a directory".into());
    }
    let workspace_root = execution_workspace.canonicalize()?;
    if !workspace_root.is_dir() {
        return Err("saved Core workspace must be a directory".into());
    }
    // Bound complete trees before following declarations that could otherwise
    // induce repeated reads of a large or excessive set of files.
    validate_tree_limits(&case_root)?;
    validate_tree_limits(&workspace_root)?;
    let report_bytes = read_explicit_file(execution_report_path, MAX_REPORT_BYTES)?;
    let saved: CoreEvidenceRun = serde_json::from_slice(&report_bytes)?;
    if saved.schema_version != "faris-core-evidence-run/v0.1" || !saved.completed() {
        return Err("saved Core execution report is unsupported or incomplete".into());
    }
    for digest in [
        &saved.core_sha256,
        &saved.faris_sha256,
        &saved.package_sha256,
    ] {
        validate_plain_sha256(digest)?;
    }
    validate_prefixed_sha256(&saved.compiled_snapshot_sha256)?;

    let package_bytes = read_case_file(&case_root, "package.json", MAX_REPORT_BYTES)?;
    let package_sha = hash_bytes(&package_bytes);
    if package_sha != saved.package_sha256 {
        return Err("saved Core report does not bind this exact case package".into());
    }
    let package: Value = serde_json::from_slice(&package_bytes)?;
    if package["schema_version"] != "avila.core/case-package/v0.1-draft" {
        return Err("unsupported saved Core case package schema".into());
    }
    let case_id = string_at(&package, "/case_id")?.to_owned();
    if case_id != saved.expected_case_id || case_id != saved.report["case_id"] {
        return Err("case identity differs across package and saved Core report".into());
    }

    let documents = package["documents"]
        .as_array()
        .ok_or("case package documents are missing")?;
    let artifacts = package["artifacts"]
        .as_array()
        .ok_or("case package artifacts are missing")?;
    if documents.is_empty()
        || artifacts.is_empty()
        || documents.len() + artifacts.len() > MAX_PACKAGE_FILES
    {
        return Err("saved case package has empty or excessive declarations".into());
    }
    let mut verified_paths = BTreeSet::new();
    let mut document_ids = BTreeSet::new();
    for document in documents {
        let id = string_at(document, "/document_id")?;
        if !document_ids.insert(id.to_owned()) {
            return Err("case package repeats a document identity".into());
        }
        verify_declared_file(
            &case_root,
            string_at(document, "/path")?,
            string_at(document, "/sha256")?,
            &mut verified_paths,
        )?;
    }
    let mut artifact_ids = BTreeSet::new();
    for artifact in artifacts {
        let id = string_at(artifact, "/artifact_id")?;
        if !artifact_ids.insert(id.to_owned()) {
            return Err("case package repeats an artifact identity".into());
        }
        if artifact["source_root"] != "case" {
            return Err(
                "saved case contains an artifact bound to an unsupported source root".into(),
            );
        }
        verify_declared_file(
            &case_root,
            string_at(artifact, "/path")?,
            string_at(artifact, "/sha256")?,
            &mut verified_paths,
        )?;
    }
    let scenario_bytes = read_case_file(&case_root, "inputs/scenario.json", MAX_REPORT_BYTES)?;
    let scenario: Value = serde_json::from_slice(&scenario_bytes)?;
    let scenario_id = string_at(&scenario, "/id")?.to_owned();
    let scenario_sha = hash_bytes(&scenario_bytes);
    let study_bytes = read_case_file(&case_root, "study.json", MAX_REPORT_BYTES)?;
    let study: Value = serde_json::from_slice(&study_bytes)?;
    let variant_id = string_at(&study, "/variant_id")?.to_owned();
    if string_at(&study, "/scenario_sha256")? != scenario_sha {
        return Err("saved study does not bind the exact scenario bytes".into());
    }
    if saved.expected_step_ids.is_empty() || saved.expected_step_ids.len() > 16 {
        return Err("saved report declares no steps or too many steps".into());
    }
    let package_steps = package["executions"]
        .as_array()
        .ok_or("case package execution declarations are missing")?;
    let package_step_ids: Vec<&str> = package_steps
        .iter()
        .map(|step| string_at(step, "/step_id"))
        .collect::<Result<_, _>>()?;
    if package_step_ids
        .iter()
        .copied()
        .ne(saved.expected_step_ids.iter().map(String::as_str))
    {
        return Err("saved execution step order differs from case package".into());
    }
    let contract_value: Value = serde_json::from_slice(&read_case_file(
        &case_root,
        "contract.json",
        MAX_REPORT_BYTES,
    )?)?;
    let registry_value: Value = serde_json::from_slice(&read_case_file(
        &case_root,
        "registry.json",
        MAX_REPORT_BYTES,
    )?)?;
    if study.get("contract") != Some(&contract_value)
        || study.get("registry") != Some(&registry_value)
    {
        return Err("saved study contract or registry differs from the case package".into());
    }
    let claims: Value = serde_json::from_slice(&read_case_file(
        &case_root,
        "claims.json",
        MAX_REPORT_BYTES,
    )?)?;
    if string_at(&claims, "/compiled_snapshot_sha256")? != saved.compiled_snapshot_sha256 {
        return Err("committed claims use a different compiled snapshot".into());
    }
    let compiled = verify_compilation(&case_root, &saved)?;

    // Revalidate the recorded transport embedded as an input. This verifies
    // its scenario/run/raw artifact/normalization identities, not the physics.
    let recorded_bytes = read_case_file(&case_root, "inputs/recorded.json", MAX_FILE_BYTES)?;
    let recorded: RecordedTransportBundle = serde_json::from_slice(&recorded_bytes)?;
    let (recorded_scenario, run) = recorded.verify()?;
    if recorded_scenario.source_sha256 != scenario_sha || run.variant_id != variant_id {
        return Err(
            "recorded transport does not match the package scenario and arrangement".into(),
        );
    }
    let physics_bytes = read_case_file(&case_root, "inputs/physics.json", MAX_REPORT_BYTES)?;
    let physics: Value = serde_json::from_slice(&physics_bytes)?;
    let transport_input: Value = serde_json::from_str(&recorded.files["input.json"])?;
    if physics != transport_input["physics"] {
        return Err("case physics input differs from the verified recorded transport".into());
    }
    let audit_bytes = read_case_file(&case_root, "inputs/nuclear-data.json", MAX_REPORT_BYTES)?;
    if audit_bytes != recorded.files["audit.json"].as_bytes() {
        return Err("case nuclear-data audit differs from the recorded transport".into());
    }

    let report = &saved.report;
    if report.pointer("/integrity/manifest_sha256") != Some(&json!(format!("sha256:{package_sha}")))
        || report.pointer("/compile/compiled/snapshot_sha256")
            != Some(&json!(saved.compiled_snapshot_sha256))
        || report.pointer("/campaign/compiled_snapshot_sha256")
            != Some(&json!(saved.compiled_snapshot_sha256))
    {
        return Err(
            "Core report identity fields do not match the saved case and compilation".into(),
        );
    }
    verify_core_integrity_inventory(report, &package, &case_root)?;

    let report_steps = report["execution"]["steps"]
        .as_array()
        .ok_or("Core execution step report is missing")?;
    let mut summaries = Vec::with_capacity(report_steps.len());
    let mut receipt_count = 0;
    let mut input_count = 0;
    let mut output_count = 0;
    let mut log_count = 0;
    let mut seen_step_ids = BTreeSet::new();
    for (expected_id, step) in saved.expected_step_ids.iter().zip(report_steps) {
        let step_id = string_at(step, "/step_id")?;
        if step_id != expected_id || !seen_step_ids.insert(step_id.to_owned()) {
            return Err("Core report contains an unexpected or duplicate step".into());
        }
        let receipt = &step["receipt"];
        let receipt_path = safe_workspace_path(string_at(receipt, "/workspace_path")?)?;
        let receipt_bytes = read_root_file(&workspace_root, &receipt_path, MAX_REPORT_BYTES)?;
        let receipt_hash = hash_bytes(&receipt_bytes);
        let expected_receipt_hash = string_at(receipt, "/sha256")?;
        verify_prefixed_digest(expected_receipt_hash, &receipt_hash)?;
        let receipt_json: Value = serde_json::from_slice(&receipt_bytes)?;
        if receipt_json["schema_version"] != "avila.core/execution-receipt/v0.1-draft"
            || receipt_json["status"] != "completed"
            || receipt_json["process"]["timed_out"] != false
            || receipt_json["step_id"] != step_id
            || receipt_json["case_id"] != saved.expected_case_id
            || receipt_json["compiled_snapshot_sha256"] != saved.compiled_snapshot_sha256
            || receipt_json["process"]["exit_status"] != receipt["exit_status"]
            || receipt_json["process"]["duration_ms"] != receipt["duration_ms"]
            || receipt_json["invocation_sha256"] != receipt["invocation_sha256"]
            || string_at(&receipt_json, "/capability/executable_sha256")?
                != format!("sha256:{}", saved.faris_sha256)
        {
            return Err(
                format!("saved receipt differs from the actual receipt for {step_id}").into(),
            );
        }
        receipt_count += 1;

        let step_dir = PathBuf::from(step_id);
        let receipt_inputs = receipt_json["inputs"]
            .as_array()
            .ok_or("Core receipt inputs are missing")?;
        if receipt_inputs.len() > 64 {
            return Err("Core receipt contains too many inputs".into());
        }
        let verification_files = step["verification"]["files"]
            .as_array()
            .ok_or("Core step verification files are missing")?;
        if verification_files.len() > 64 || receipt_inputs.len() > verification_files.len() {
            return Err("saved Core step has too many input files".into());
        }
        let input_files: Vec<_> = verification_files
            .iter()
            .filter(|entry| entry["role"] == "input")
            .collect();
        if input_files.len() != receipt_inputs.len() {
            return Err("Core receipt and verification input counts differ".into());
        }
        for (entry, receipt_input) in input_files.into_iter().zip(receipt_inputs) {
            if entry["state"] != "verified" || entry["actual_sha256"] != entry["expected_sha256"] {
                return Err(format!("Core did not verify a bound input for {step_id}").into());
            }
            if entry["workspace_path"] != receipt_input["workspace_path"]
                || entry["actual_sha256"] != receipt_input["sha256"]
            {
                return Err(format!("Core receipt and verification differ for {step_id}").into());
            }
            let relative = safe_workspace_path(string_at(entry, "/workspace_path")?)?;
            let path = step_dir.join(relative);
            let bytes = read_root_file(&workspace_root, &path, MAX_FILE_BYTES)?;
            verify_prefixed_digest(string_at(entry, "/actual_sha256")?, &hash_bytes(&bytes))?;
            input_count += 1;
        }

        let outputs = step["outputs"]
            .as_array()
            .ok_or("Core step outputs are missing")?;
        if outputs.is_empty() || outputs.len() > 64 {
            return Err("saved Core step has an empty or excessive output list".into());
        }
        let receipt_outputs = receipt_json["outputs"]
            .as_array()
            .ok_or("Core receipt outputs are missing")?;
        if receipt_outputs.len() != outputs.len() {
            return Err(format!("Core receipt output count differs for {step_id}").into());
        }
        for (output, receipt_output) in outputs.iter().zip(receipt_outputs) {
            if output["state"] != "collected" || output["reproduces_bound_artifact"] != true {
                return Err(format!("Core did not collect a bound output for {step_id}").into());
            }
            if output["output_id"] != receipt_output["output_id"]
                || output["workspace_path"] != receipt_output["workspace_path"]
                || output["sha256"] != receipt_output["sha256"]
            {
                return Err(
                    format!("Core receipt and output identity differ for {step_id}").into(),
                );
            }
            let relative = safe_workspace_path(string_at(output, "/workspace_path")?)?;
            let path = step_dir.join(relative);
            let bytes = read_root_file(&workspace_root, &path, MAX_FILE_BYTES)?;
            verify_prefixed_digest(string_at(output, "/sha256")?, &hash_bytes(&bytes))?;
            if output["bytes"].as_u64() != Some(bytes.len() as u64) {
                return Err(format!("Core output byte count differs for {step_id}").into());
            }
            output_count += 1;
        }
        let logs = receipt_json["logs"]
            .as_array()
            .ok_or("Core receipt logs are missing")?;
        if logs.len() > 8 {
            return Err("Core receipt has too many log files".into());
        }
        for log in logs {
            let relative = safe_workspace_path(string_at(log, "/workspace_path")?)?;
            let bytes = read_root_file(&workspace_root, &step_dir.join(relative), MAX_FILE_BYTES)?;
            verify_prefixed_digest(string_at(log, "/sha256")?, &hash_bytes(&bytes))?;
            if log["bytes"].as_u64() != Some(bytes.len() as u64) {
                return Err(format!("Core log byte count differs for {step_id}").into());
            }
            log_count += 1;
        }
        for entry in verification_files {
            if entry["state"] != "verified" || entry["actual_sha256"] != entry["expected_sha256"] {
                return Err(format!("Core did not verify a bound file for {step_id}").into());
            }
            let relative = safe_workspace_path(string_at(entry, "/workspace_path")?)?;
            let actual_role = match entry["role"].as_str() {
                Some("input") => continue,
                Some("log") => {
                    let path = relative.to_string_lossy();
                    logs.iter()
                        .find(|log| log["workspace_path"] == path.as_ref())
                        .ok_or("Core verification log is absent from its receipt")?
                }
                Some("output") => outputs
                    .iter()
                    .find(|output| output["workspace_path"] == entry["workspace_path"])
                    .ok_or("Core verification output is absent from its receipt")?,
                _ => {
                    return Err("Core report contains an unsupported verification file role".into());
                }
            };
            if entry["actual_sha256"] != actual_role["sha256"] {
                return Err("Core receipt differs from its verification file entry".into());
            }
        }
        summaries.push(VerifiedStepSummary {
            step_id: step_id.to_owned(),
            receipt_sha256: format!("sha256:{receipt_hash}"),
            verified_input_count: receipt_inputs.len(),
            verified_output_count: outputs.len(),
            verified_log_count: logs.len(),
        });
    }
    if summaries.len() != saved.expected_step_ids.len() {
        return Err("Core report omitted one or more committed execution steps".into());
    }
    validate_tree_limits(&case_root)?;
    validate_tree_limits(&workspace_root)?;

    let requirement_verdicts = parse_verdicts(report)?;
    let limitations = package["limitations"]
        .as_array()
        .ok_or("case package limitations are missing")?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or("invalid case limitation")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let scope_notice = report["notice"]
        .as_str()
        .ok_or("Core report scope notice is missing")?
        .to_owned();

    let history = load_history_result(&case_root, report, &scenario, &run)?;

    Ok(SavedCaseInspection {
        schema_version: "faris-saved-case-inspection/v0.2".into(),
        record_integrity: "UNSIGNED_IDENTITY_REVALIDATED".into(),
        case_id,
        scenario_id,
        scenario_sha256: format!("sha256:{scenario_sha}"),
        variant_id,
        compiled_snapshot_sha256: compiled.snapshot_sha256,
        package_sha256: format!("sha256:{package_sha}"),
        faris_executable_sha256: format!("sha256:{}", saved.faris_sha256),
        core_executable_sha256: format!("sha256:{}", saved.core_sha256),
        compiler_id: compiled.compiler_id,
        semantic_profile: compiled.semantic_profile,
        compiler_executable_sha256: format!("sha256:{}", compiled.executable_sha256),
        compilation_status: report["compile"]["status"]
            .as_str()
            .unwrap_or_default()
            .into(),
        execution_status: report["execution"]["status"]
            .as_str()
            .unwrap_or_default()
            .into(),
        binding_status: report["bindings"]["status"]
            .as_str()
            .unwrap_or_default()
            .into(),
        campaign_status: report["campaign"]["status"]
            .as_str()
            .unwrap_or_default()
            .into(),
        requirement_verdicts,
        steps: summaries,
        verified_package_document_count: documents.len(),
        verified_package_artifact_count: artifacts.len(),
        verified_receipt_count: receipt_count,
        verified_stage_input_count: input_count,
        verified_stage_output_count: output_count,
        verified_log_count: log_count,
        history_snapshot_count: history.result.as_ref().map(|value| value.snapshots.len()),
        history_event_count: history.result.as_ref().map(|value| value.events.len()),
        history_result_sha256: history.result_sha256,
        history_assumptions_sha256: history.assumptions_sha256,
        history_assumptions: history.assumptions,
        history_result: history.result,
        limitations,
        scope_notice,
    })
}

fn verify_compilation(
    root: &Path,
    saved: &CoreEvidenceRun,
) -> Result<VerifiedCompilation, ReactorError> {
    let bytes = read_case_file(root, "compile/compilation.json", MAX_REPORT_BYTES)?;
    let compilation: Value = serde_json::from_slice(&bytes)?;
    if compilation["schema_version"] != "faris-core-compilation/v0.1"
        || compilation["execution"]["execution_status"] != "SUCCEEDED"
        || compilation["execution"]["exit_code"] != 0
        || compilation["report"]["status"] != "compiled"
    {
        return Err("saved Core compilation is incomplete or unsupported".into());
    }
    let snapshot = string_at(&compilation, "/report/compiled/snapshot_sha256")?.to_owned();
    validate_prefixed_sha256(&snapshot)?;
    let contract: Value =
        serde_json::from_slice(&read_case_file(root, "contract.json", MAX_REPORT_BYTES)?)?;
    if snapshot != saved.compiled_snapshot_sha256
        || string_at(&compilation, "/report/compiled/contract_id")?
            != string_at(&contract, "/contract_id")?
    {
        return Err("compiled snapshot or contract identity differs from the run report".into());
    }
    let contract_hash = hash_bytes(&read_case_file(root, "contract.json", MAX_REPORT_BYTES)?);
    let registry_hash = hash_bytes(&read_case_file(root, "registry.json", MAX_REPORT_BYTES)?);
    if string_at(&compilation, "/contract_sha256")? != contract_hash
        || string_at(&compilation, "/registry_sha256")? != registry_hash
    {
        return Err("Core compilation does not bind the saved contract and registry".into());
    }
    let executable_sha256 = string_at(&compilation, "/executable_sha256")?;
    validate_plain_sha256(executable_sha256)?;
    if executable_sha256 != saved.core_sha256 {
        return Err(
            "saved Core compilation executable identity differs from the execution report".into(),
        );
    }
    let compiled = &compilation["report"]["compiled"];
    let compiler_id = checked_identity_label(string_at(compiled, "/compiler")?, "Core compiler")?;
    let semantic_profile = checked_identity_label(
        string_at(compiled, "/semantic_profile")?,
        "Core semantic profile",
    )?;
    Ok(VerifiedCompilation {
        snapshot_sha256: snapshot,
        compiler_id,
        semantic_profile,
        executable_sha256: executable_sha256.to_owned(),
    })
}

fn checked_identity_label(value: &str, label: &str) -> Result<String, ReactorError> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(
            format!("saved {label} identity is empty, excessive, or contains controls").into(),
        );
    }
    Ok(value.to_owned())
}

fn verify_core_integrity_inventory(
    report: &Value,
    package: &Value,
    root: &Path,
) -> Result<(), ReactorError> {
    for (package_key, report_key, id_key) in [
        ("documents", "documents", "document_id"),
        ("artifacts", "artifacts", "artifact_id"),
    ] {
        let declared = package[package_key]
            .as_array()
            .ok_or("case package integrity declarations are missing")?;
        let checked = report
            .pointer(&format!("/integrity/{report_key}"))
            .and_then(Value::as_array)
            .ok_or("Core integrity inventory is missing")?;
        if checked.len() != declared.len() {
            return Err("Core integrity inventory count differs from the case package".into());
        }
        for expected in declared {
            let id = string_at(expected, &format!("/{id_key}"))?;
            let actual = checked
                .iter()
                .find(|entry| entry[id_key] == id)
                .ok_or("Core integrity inventory omitted a package entry")?;
            if actual[id_key] != expected[id_key]
                || actual["state"] != "verified"
                || actual["path"] != expected["path"]
                || actual["expected_sha256"] != expected["sha256"]
                || actual["actual_sha256"] != expected["sha256"]
                || (package_key == "artifacts" && actual["source_root"] != expected["source_root"])
                || (package_key == "documents" && actual["role"] != expected["role"])
            {
                return Err("Core integrity inventory differs from the bound package".into());
            }
            let path = string_at(expected, "/path")?;
            let bytes = read_case_file(root, path, MAX_FILE_BYTES)?;
            verify_prefixed_digest(string_at(expected, "/sha256")?, &hash_bytes(&bytes))?;
        }
    }
    Ok(())
}

fn load_history_result(
    root: &Path,
    core_report: &Value,
    scenario: &Value,
    run: &crate::reactor::ReactorRun,
) -> Result<LoadedHistory, ReactorError> {
    let has_history_step = core_report["execution"]["steps"]
        .as_array()
        .is_some_and(|steps| steps.iter().any(|step| step["step_id"] == "history"));
    let history_path = root.join("expected/history.json");
    if !has_history_step {
        if history_path.exists() {
            return Err("case has a history artifact but no committed history execution".into());
        }
        return Ok(LoadedHistory {
            assumptions: None,
            result: None,
            result_sha256: None,
            assumptions_sha256: None,
        });
    }
    let stage_bytes = read_case_file(root, "expected/history.json", MAX_FILE_BYTES)?;
    let stage: Value = serde_json::from_slice(&stage_bytes)?;
    if !crate::core_evidence::is_supported_stage_schema(&stage["schema_version"])
        || stage["stage"] != "history"
        || stage["history"] != "history_calculated"
        || stage["scientific_qualification"] != "NOT_EVALUATED"
    {
        return Err("saved history stage has unsupported schema or scope".into());
    }
    let history_json = string_at(&stage, "/history_json")?;
    let history_digest = string_at(&stage, "/history_sha256")?;
    verify_prefixed_digest(history_digest, &hash_bytes(history_json.as_bytes()))?;
    let history: crate::history::HistoryResult = serde_json::from_str(history_json)?;
    if history.schema_version != "faris-history-result/v0.1" {
        return Err("saved history result schema is unsupported".into());
    }
    let assumptions_bytes = read_case_file(root, "inputs/assumptions.json", MAX_REPORT_BYTES)?;
    let assumptions: OperatingHistoryAssumptions = serde_json::from_slice(&assumptions_bytes)?;
    assumptions
        .validate()
        .map_err(|error| -> ReactorError { error.into() })?;
    if history.assumptions != assumptions {
        return Err("saved history assumptions differ from the bound case input".into());
    }
    let scenario_sha = hash_bytes(&read_case_file(
        root,
        "inputs/scenario.json",
        MAX_REPORT_BYTES,
    )?);
    let artifact_sha = run
        .raw_artifact_sha256
        .as_deref()
        .ok_or("saved transport result has no raw artifact identity")?;
    let normalized = run
        .normalized
        .as_ref()
        .ok_or("saved transport result has no normalized responses")?;
    let expected_rates = TransportDrivingRates::from_normalized(
        normalized,
        scenario["operating_plan"]["fusion_power_mw"]
            .as_f64()
            .ok_or("scenario has no finite fusion power")?,
        artifact_sha,
    )
    .map_err(|error| -> ReactorError { error.into() })?;
    if history.driving_rates != expected_rates
        || history.driving_rates.scenario_sha256 != scenario_sha
        || history.driving_rates.transport_artifact_sha256 != artifact_sha
    {
        return Err(
            "saved history rates are not bound to the exact verified transport result".into(),
        );
    }
    let steps = core_report["execution"]["steps"]
        .as_array()
        .ok_or("Core report execution steps are missing")?;
    let energy_step = steps.iter().find(|step| step["step_id"] == "energy");
    if let Some(energy) = energy_step {
        let energy_bytes = read_case_file(root, "expected/energy.json", MAX_FILE_BYTES)?;
        let energy_stage: Value = serde_json::from_slice(&energy_bytes)?;
        if energy_stage["history_sha256"] != history_digest
            || energy_stage["scientific_qualification"] != "NOT_EVALUATED"
        {
            return Err("saved energy stage is not bound to the exact history result".into());
        }
        // The stage output in the package must be declared and Core-receipted.
        if !energy["outputs"]
            .as_array()
            .is_some_and(|outputs| !outputs.is_empty())
        {
            return Err("Core did not receipt the energy output".into());
        }
    }
    Ok(LoadedHistory {
        assumptions: Some(assumptions),
        result: Some(history),
        result_sha256: Some(history_digest.to_owned()),
        assumptions_sha256: Some(format!("sha256:{}", hash_bytes(&assumptions_bytes))),
    })
}

fn parse_verdicts(report: &Value) -> Result<Vec<SavedRequirementVerdict>, ReactorError> {
    let verdicts = report["campaign"]["verdicts"]
        .as_array()
        .ok_or("Core campaign verdicts are missing")?;
    verdicts
        .iter()
        .map(|entry| {
            let status = string_at(entry, "/verdict/status")?;
            if !["pass", "fail", "inconclusive", "not_evaluated"].contains(&status) {
                return Err("Core report contains an unsupported verdict state".into());
            }
            let evidence_ids = entry["evidence_ids"]
                .as_array()
                .ok_or("verdict evidence IDs are missing")?
                .iter()
                .map(|id| id.as_str().map(str::to_owned).ok_or("invalid evidence ID"))
                .collect::<Result<_, _>>()?;
            let reasons = entry["verdict"]["reasons"]
                .as_array()
                .ok_or("verdict reasons are missing")?;
            let reason_codes = reasons
                .iter()
                .filter_map(|reason| reason["code"].as_str().map(str::to_owned))
                .collect();
            Ok(SavedRequirementVerdict {
                requirement_id: string_at(entry, "/requirement_id")?.to_owned(),
                status: status.to_owned(),
                evidence_ids,
                reason_codes,
            })
        })
        .collect()
}

fn verify_declared_file(
    root: &Path,
    relative: &str,
    digest: &str,
    seen: &mut BTreeSet<String>,
) -> Result<(), ReactorError> {
    validate_prefixed_sha256(digest)?;
    let relative_path = safe_relative_path(relative)?;
    let key = relative_path.to_string_lossy().into_owned();
    if !seen.insert(key.clone()) {
        return Err("case package declares a file more than once".into());
    }
    let bytes = read_root_file(root, &relative_path, MAX_FILE_BYTES)?;
    verify_prefixed_digest(digest, &hash_bytes(&bytes))?;
    let _ = key;
    Ok(())
}

fn read_case_file(root: &Path, relative: &str, max_bytes: u64) -> Result<Vec<u8>, ReactorError> {
    let relative = safe_relative_path(relative)?;
    read_root_file(root, &relative, max_bytes)
}

fn read_root_file(root: &Path, relative: &Path, max_bytes: u64) -> Result<Vec<u8>, ReactorError> {
    let candidate = root.join(relative);
    let metadata = fs::symlink_metadata(&candidate)?;
    if !metadata.file_type().is_file() || metadata.len() > max_bytes {
        return Err("saved evidence file is not regular or exceeds its read bound".into());
    }
    let canonical = candidate.canonicalize()?;
    if !canonical.starts_with(root) {
        return Err("saved evidence path escapes its declared root".into());
    }
    read_explicit_file(&canonical, max_bytes)
}

fn read_explicit_file(path: &Path, max_bytes: u64) -> Result<Vec<u8>, ReactorError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.len() > max_bytes {
        return Err("saved evidence file is not regular or exceeds its read bound".into());
    }
    let mut file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err("saved evidence path is not a regular file".into());
    }
    let mut bytes = Vec::with_capacity(metadata.len().min(max_bytes) as usize);
    file.by_ref().take(max_bytes + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err("saved evidence file exceeds its read bound".into());
    }
    Ok(bytes)
}

fn validate_tree_limits(root: &Path) -> Result<(), ReactorError> {
    let mut todo = vec![root.to_owned()];
    let mut files = 0usize;
    let mut directories = 0usize;
    let mut entries = 0usize;
    let mut bytes = 0u64;
    while let Some(directory) = todo.pop() {
        for entry in fs::read_dir(directory)? {
            entries += 1;
            if entries > MAX_TREE_ENTRIES {
                return Err("saved evidence tree exceeds directory-entry bound".into());
            }
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            let kind = metadata.file_type();
            if kind.is_symlink() {
                return Err("saved evidence trees must not contain symlinks".into());
            }
            if kind.is_dir() {
                directories += 1;
                if directories > MAX_PACKAGE_FILES {
                    return Err("saved evidence tree exceeds directory-count bound".into());
                }
                todo.push(path);
            } else if kind.is_file() {
                files += 1;
                bytes = bytes.saturating_add(metadata.len());
                if files > MAX_PACKAGE_FILES || bytes > MAX_PACKAGE_BYTES {
                    return Err(
                        "saved evidence tree exceeds file-count or total-size bounds".into(),
                    );
                }
            }
        }
    }
    Ok(())
}

fn safe_workspace_path(value: &str) -> Result<PathBuf, ReactorError> {
    safe_relative_path(value)
}

fn safe_relative_path(value: &str) -> Result<PathBuf, ReactorError> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("saved evidence contains an unsafe relative path".into());
    }
    Ok(path.to_owned())
}

fn string_at<'a>(value: &'a Value, pointer: &str) -> Result<&'a str, ReactorError> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("saved evidence missing string at {pointer}").into())
}

fn validate_plain_sha256(value: &str) -> Result<(), ReactorError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("saved evidence contains an invalid SHA-256 identity".into());
    }
    Ok(())
}

fn validate_prefixed_sha256(value: &str) -> Result<(), ReactorError> {
    let digest = value
        .strip_prefix("sha256:")
        .ok_or("saved evidence digest must use sha256:")?;
    validate_plain_sha256(digest)
}

fn verify_prefixed_digest(expected: &str, actual: &str) -> Result<(), ReactorError> {
    validate_prefixed_sha256(expected)?;
    if expected != format!("sha256:{actual}") {
        return Err("saved evidence file digest does not match its bound identity".into());
    }
    Ok(())
}

fn hash_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: SEC-002
    #[test]
    fn archive_paths_and_digests_are_strict() {
        assert!(safe_relative_path("inputs/scenario.json").is_ok());
        for unsafe_path in ["", "/etc/passwd", "../outside", "inputs/../scenario.json"] {
            assert!(safe_relative_path(unsafe_path).is_err(), "{unsafe_path}");
        }
        assert!(validate_prefixed_sha256(&format!("sha256:{}", "a".repeat(64))).is_ok());
        assert!(validate_prefixed_sha256("sha256:not-a-digest").is_err());
        assert!(validate_plain_sha256(&"g".repeat(64)).is_err());
    }

    // Verifies: SEC-002
    #[test]
    fn archive_file_reader_rejects_symlink_and_oversize() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        fs::write(root.join("small"), b"ok").unwrap();
        assert_eq!(read_root_file(&root, Path::new("small"), 2).unwrap(), b"ok");
        assert!(read_root_file(&root, Path::new("small"), 1).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("small"), root.join("link")).unwrap();
            assert!(read_root_file(&root, Path::new("link"), 2).is_err());
        }
    }

    #[test]
    fn compiler_and_semantic_profile_must_be_present_bounded_identities() {
        assert_eq!(
            checked_identity_label("avila.core/compiler-rust@0.1.0", "Core compiler").unwrap(),
            "avila.core/compiler-rust@0.1.0"
        );
        assert!(
            checked_identity_label("avila.core/semantic/0.2-draft", "Core semantic profile")
                .is_ok()
        );
        for invalid in ["", "bad\nlabel", &"x".repeat(257)] {
            assert!(checked_identity_label(invalid, "Core identity").is_err());
        }
    }
}
