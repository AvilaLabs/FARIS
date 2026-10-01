//! Generated study declarations and genuine external Avila Core compilation.
//! Compilation, execution readiness and scientific assessment are separate.

use crate::{
    DemoManifest,
    jobs::{Cancellation, ExecutionStatus, JobResult, JobSpec, run_job},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StudySelection {
    pub breeding: bool,
    pub shielding: bool,
    pub fuel_history: bool,
    pub electricity: bool,
    /// Canonical exact-number text; Core reports malformed values itself.
    pub minimum_tbr: String,
}

impl Default for StudySelection {
    fn default() -> Self {
        Self {
            breeding: true,
            shielding: true,
            fuel_history: false,
            electricity: false,
            minimum_tbr: "1".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct GeneratedStudy {
    pub schema_version: String,
    pub scenario_sha256: String,
    pub variant_id: String,
    pub selection: StudySelection,
    pub stages: Vec<String>,
    pub contract: Value,
    pub registry: Value,
    pub notice: String,
}

fn reference(id: &str) -> Value {
    json!({"id": id, "major": 1})
}
fn role(id: &str, quantity: Option<(&str, &str)>) -> Value {
    let mut role = json!({
        "role": reference(id), "owner": "avila-labs.faris",
        "validator": format!("{id}.validate@1"),
        "accepted_media_types": ["application/json"],
        "permitted_claim_models": [{"model":"unquantified"}],
        "non_claims": ["Role declaration does not qualify its input or a scientific method."]
    });
    if let Some((kind, units)) = quantity {
        role["quantity_kind"] = json!(kind);
        role["unit_class"] = json!(units);
        role["permitted_claim_models"] = json!([{"model":"standard_uncertainty"},{"model":"coverage_interval"},{"model":"unquantified"}]);
    }
    role
}
fn input(slot: &str, role: &str) -> Value {
    json!({"slot_id":slot,"role":reference(role),"accepted_media_types":["application/json"]})
}
fn output(slot: &str, role: &str, quantified: bool) -> Value {
    json!({"slot_id":slot,"role":reference(role),"media_type":"application/json",
        "permitted_claim_models": if quantified {json!([{"model":"standard_uncertainty"},{"model":"coverage_interval"},{"model":"unquantified"}])} else {json!([{"model":"unquantified"}])}})
}
fn binding(slot: &str, source: Value) -> Value {
    json!({"input_slot":slot,"source":source})
}
fn authored(id: &str) -> Value {
    json!({"source":"contract_input","input_id":id})
}
fn produced(step: &str, slot: &str) -> Value {
    json!({"source":"step_output","step_id":step,"output_slot":slot})
}

/// Generate declared dependencies. Missing physical inputs are readiness issues,
/// while malformed declarations are delegated to the genuine Core compiler.
pub fn generate_study(
    manifest: &DemoManifest,
    variant_id: &str,
    selection: &StudySelection,
) -> Result<GeneratedStudy, String> {
    if !manifest.variants.iter().any(|v| v.id == variant_id) {
        return Err("unknown study arrangement".into());
    }
    if !selection.breeding
        && !selection.shielding
        && !selection.fuel_history
        && !selection.electricity
    {
        return Err("select at least one analysis".into());
    }
    let history = selection.fuel_history || selection.electricity;
    let mut stages = vec!["transport".to_owned(), "normalize".to_owned()];
    if history {
        stages.push("history".into());
    }
    if selection.electricity {
        stages.push("energy".into());
    }
    let mut roles = vec![
        role("faris.scenario", None),
        role("faris.physics", None),
        role("faris.nuclear-data", None),
        role("faris.raw-transport", None),
        role("faris.normalized-transport", None),
        role("faris.history", None),
        role("faris.energy", None),
        role(
            "faris.tbr",
            Some((
                "faris.tritium_breeding_ratio",
                "faris.tritium_breeding_ratio.units@1",
            )),
        ),
    ];
    if selection.shielding {
        roles.push(role(
            "faris.magnet-neutron-flux",
            Some(("faris.neutron_flux", "faris.neutron_flux.units@1")),
        ));
    }
    roles.sort_by_key(|r| r["role"]["id"].as_str().unwrap_or_default().to_owned());
    let mut normalize_outputs = vec![
        output("normalized", "faris.normalized-transport", false),
        output("tbr", "faris.tbr", true),
    ];
    if selection.shielding {
        normalize_outputs.push(output(
            "magnet-neutron-flux",
            "faris.magnet-neutron-flux",
            true,
        ));
    }
    let capabilities = vec![
        json!({"capability_type":reference("faris.openmc-transport"),"owner":"avila-labs.faris",
            "reproducibility":{"determinism":"seeded_stochastic"},
            "inputs":[input("scenario","faris.scenario"),input("physics","faris.physics"),input("nuclear-data","faris.nuclear-data")],
            "outputs":[output("raw","faris.raw-transport",false)]}),
        json!({"capability_type":reference("faris.normalize-transport"),"owner":"avila-labs.faris",
            "reproducibility":{"determinism":"deterministic"},
            "inputs":[input("raw","faris.raw-transport"),input("scenario","faris.scenario"),input("physics","faris.physics")],
            "outputs":normalize_outputs}),
        json!({"capability_type":reference("faris.operating-history"),"owner":"avila-labs.faris",
            "reproducibility":{"determinism":"deterministic"},
            "inputs":[input("rates","faris.normalized-transport"),input("scenario","faris.scenario")],
            "outputs":[output("history","faris.history",false)]}),
        json!({"capability_type":reference("faris.net-energy"),"owner":"avila-labs.faris",
            "reproducibility":{"determinism":"deterministic"},
            "inputs":[input("history","faris.history"),input("scenario","faris.scenario")],
            "outputs":[output("energy","faris.energy",false)]}),
    ];
    let mut kinds = vec![
        json!({"kind_id":"faris.tritium_breeding_ratio","canonical_unit":"1","unit_class":"faris.tritium_breeding_ratio.units@1",
        "owner":"avila-labs.faris","units":[{"symbol":"1","factor":"1"}]}),
    ];
    if selection.shielding {
        kinds.push(json!({"kind_id":"faris.neutron_flux","canonical_unit":"neutrons/m²/s","unit_class":"faris.neutron_flux.units@1",
            "owner":"avila-labs.faris","units":[{"symbol":"neutrons/m²/s","factor":"1"}]}));
    }
    let registry = json!({"schema_version":"avila.core/registry-snapshot/v0.2-draft",
        "semantic_profile":"avila.core/semantic/0.2-draft","registry_id":"faris.demo.registry","revision":1,
        "kinds":kinds,
        "purposes":[{"purpose":reference("faris.conditional-research-comparison"),"owner":"avila-labs.faris",
            "description":"Comparison conditional on authored geometry/material/source/history inputs; not a qualified reactor claim."}],
        "roles":roles,"capability_types":capabilities});
    let mut workflow = vec![
        json!({"step_id":"transport","capability_type":reference("faris.openmc-transport"),
            "bindings":[binding("scenario",authored("scenario")),binding("physics",authored("physics")),binding("nuclear-data",authored("nuclear-data"))],
            "reproducibility":{"seed":"123456789"}}),
        json!({"step_id":"normalize","capability_type":reference("faris.normalize-transport"),
            "bindings":[binding("raw",produced("transport","raw")),binding("scenario",authored("scenario")),binding("physics",authored("physics"))]}),
    ];
    if history {
        workflow.push(json!({"step_id":"history","capability_type":reference("faris.operating-history"),
        "bindings":[binding("rates",produced("normalize","normalized")),binding("scenario",authored("scenario"))]}));
    }
    if selection.electricity {
        workflow.push(json!({"step_id":"energy","capability_type":reference("faris.net-energy"),
        "bindings":[binding("history",produced("history","history")),binding("scenario",authored("scenario"))]}));
    }
    let requirements = if selection.breeding {
        vec![json!({
        "requirement_id":"FARIS-TBR","statement":"Model tritium production per primary D-T neutron meets the declared research criterion; model applicability requires separate qualification.",
        "purpose":reference("faris.conditional-research-comparison"),"metric":produced("normalize","tbr"),
        "comparison":"greater_than_or_equal","limit":{"kind":"faris.tritium_breeding_ratio","value":selection.minimum_tbr,"unit":"1"},
        "basis":{"kind":"bounded","coverage":"0.95"}})]
    } else {
        vec![]
    };
    let mut requested_responses = Vec::new();
    if selection.breeding {
        requested_responses.push("tritium breeding ratio".to_owned());
    }
    if selection.shielding {
        requested_responses
            .push("component-average neutron flux in the magnet (neutrons/m²/s)".to_owned());
    }
    if selection.fuel_history {
        requested_responses.push("declared operating-history response".to_owned());
    }
    if selection.electricity {
        requested_responses.push("net-energy response".to_owned());
    }
    let contract = json!({"schema_version":"avila.core/evidence-contract/v0.2-draft",
        "semantic_profile":"avila.core/semantic/0.2-draft","contract_id":format!("faris.{}.{}",manifest.scenario_id,variant_id),
        "revision":1,"status":"draft",
        "question":format!("How do the selected blanket/shield allocation and declared operating assumptions affect {}?", requested_responses.join(" and ")),
        "assumptions":manifest.assumptions,"execution_policy":{"require_qualification":true},
        "inputs":[{"input_id":"scenario","role":reference("faris.scenario"),"media_type":"application/json","claim_model":{"model":"unquantified"}},
            {"input_id":"physics","role":reference("faris.physics"),"media_type":"application/json","claim_model":{"model":"unquantified"}},
            {"input_id":"nuclear-data","role":reference("faris.nuclear-data"),"media_type":"application/json","claim_model":{"model":"unquantified"}}],
        "workflow":workflow,"requirements":requirements});
    Ok(GeneratedStudy {schema_version:"faris-generated-study/v0.1".into(),scenario_sha256:manifest.source_sha256.clone(),variant_id:variant_id.into(),
        selection:selection.clone(),stages,contract,registry,
        notice:"Generated declarations do not establish available inputs, executable stages, scientific qualification or requirement verdicts.".into()})
}

#[derive(Debug, Serialize)]
pub struct CoreCompilation {
    pub schema_version: String,
    pub executable_sha256: String,
    pub contract_sha256: String,
    pub registry_sha256: String,
    pub execution: JobResult,
    pub report: Value,
}

fn new_file(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut file = File::options().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn executable_digest(path: &Path) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let metadata = path.metadata()?;
    if !metadata.is_file() || metadata.len() > 512 * 1024 * 1024 {
        return Err("Core executable must be a regular file below 512 MiB".into());
    }
    let mut binary = File::open(path)?.take(512 * 1024 * 1024 + 1);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 65536];
    let mut length = 0_u64;
    loop {
        let count = binary.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        length += count as u64;
        if length > 512 * 1024 * 1024 {
            return Err("Core executable grew beyond limit".into());
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn replace_file(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let temporary = path.with_extension("json.tmp");
    let mut file = File::options()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

/// Call the actual external compiler in an owned worker workspace. A rejection
/// is a successfully obtained compiler report, not a transport or physics FAIL.
pub fn compile_study(
    study: &GeneratedStudy,
    executable: &Path,
    output: &Path,
    cancellation: &Cancellation,
) -> Result<CoreCompilation, Box<dyn std::error::Error + Send + Sync>> {
    let output = std::path::absolute(output)?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir(&output)?;
    let receipt = output.join("compilation.json");
    new_file(
        &receipt,
        &serde_json::to_vec_pretty(&json!({
            "schema_version":"faris-core-compilation-receipt/v0.1",
            "status":"not_available", "execution_status":"NOT_STARTED",
            "executable_path":executable, "binding_checked":false,
            "reason":"Compilation has not produced a validated Core report."
        }))?,
    )?;
    let result = compile_study_inner(study, executable, &output, cancellation);
    match result {
        Ok(result) => {
            if let Err(error) = replace_file(&receipt, &serde_json::to_vec_pretty(&result)?) {
                return Err(format!(
                    "compilation completed but its receipt could not be finalized: {error}"
                )
                .into());
            }
            Ok(result)
        }
        Err(error) => {
            let failure = json!({"schema_version":"faris-core-compilation-receipt/v0.1",
                "status":"not_available","execution_status":"NOT_AVAILABLE",
                "executable_path":executable,"binding_checked":false,
                "reason":error.to_string()});
            replace_file(&receipt, &serde_json::to_vec_pretty(&failure)?)?;
            Err(error)
        }
    }
}

fn compile_study_inner(
    study: &GeneratedStudy,
    executable: &Path,
    output: &Path,
    cancellation: &Cancellation,
) -> Result<CoreCompilation, Box<dyn std::error::Error + Send + Sync>> {
    let executable = executable.canonicalize()?;
    let executable_sha256 = executable_digest(&executable)?;
    let contract = serde_json::to_vec_pretty(&study.contract)?;
    let registry = serde_json::to_vec_pretty(&study.registry)?;
    let contract_path = output.join("contract.json");
    let registry_path = output.join("registry.json");
    new_file(&contract_path, &contract)?;
    new_file(&registry_path, &registry)?;
    new_file(
        &output.join("study.json"),
        &serde_json::to_vec_pretty(study)?,
    )?;
    let spec = JobSpec {
        program: executable.clone(),
        arguments: vec![
            "compile".into(),
            "--contract".into(),
            contract_path.into_os_string(),
            "--registry".into(),
            registry_path.into_os_string(),
        ],
        working_directory: output.to_path_buf(),
        environment: vec![],
        timeout: Duration::from_secs(30),
        capture_limit_bytes: 4 * 1024 * 1024,
    };
    let pre_execution_sha256 = executable_digest(&executable)?;
    if pre_execution_sha256 != executable_sha256 {
        return Err(
            "Core executable changed before execution; compilation was not launched".into(),
        );
    }
    let execution_result = run_job(&spec, cancellation);
    let post_execution_sha256 = executable_digest(&executable).ok();
    let identity_stable = post_execution_sha256.as_deref() == Some(executable_sha256.as_str());
    let execution = execution_result?;
    let mut report = if matches!(
        execution.execution_status,
        ExecutionStatus::Succeeded | ExecutionStatus::Failed
    ) && matches!(execution.exit_code, Some(0 | 1))
    {
        match serde_json::from_str::<Value>(&execution.stdout) {
            Ok(value)
                if value["schema_version"] == "avila.core/compile-report/v0.2-draft"
                    && matches!(
                        (execution.exit_code, value["status"].as_str()),
                        (Some(0), Some("compiled")) | (Some(1), Some("rejected"))
                    ) =>
            {
                value
            }
            Ok(_) => {
                json!({"status":"not_available","reason":"Core returned an unsupported or inconsistent compilation report."})
            }
            Err(error) => {
                json!({"status":"not_available","reason":format!("Core report could not be parsed: {error}")})
            }
        }
    } else {
        json!({"status":"not_available","reason":"Compiler execution did not produce a completed report."})
    };
    if identity_stable {
        if let Some(object) = report.as_object_mut() {
            object.insert("executable_identity_check".into(), json!({
                "initial_sha256":executable_sha256,
                "pre_execution_sha256":pre_execution_sha256,
                "post_execution_sha256":post_execution_sha256,
                "binding_checked":true,
                "scope":"Path-content consistency checks before and after execution; not a signed attestation or proof of loaded executable bytes."
            }));
        }
    } else {
        report = json!({"status":"not_available",
            "reason":"Core executable identity changed or became unavailable across execution; compiler report invalidated.",
            "executable_identity_check":{"initial_sha256":executable_sha256,
                "pre_execution_sha256":pre_execution_sha256,"post_execution_sha256":post_execution_sha256,
                "binding_checked":false,
                "scope":"Path-content consistency checks only; not a signed attestation or proof of loaded executable bytes."}});
    }
    let result = CoreCompilation {
        schema_version: "faris-core-compilation/v0.1".into(),
        executable_sha256,
        contract_sha256: digest(&contract),
        registry_sha256: digest(&registry),
        execution,
        report,
    };
    Ok(result)
}

pub fn local_core_executable() -> Option<PathBuf> {
    std::env::var_os("FARIS_CORE_EXECUTABLE")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("PATH").and_then(|paths| {
                std::env::split_paths(&paths)
                    .map(|p| p.join("avila-core"))
                    .find(|p| p.is_file())
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn manifest() -> DemoManifest {
        crate::build_manifest(
            &faris_model::LoadedScenario::from_bytes(include_bytes!(
                "../../../scenarios/arc-inspired/scenario.json"
            ))
            .unwrap(),
        )
        .unwrap()
    }
    #[test]
    fn electricity_adds_history_and_transport_dependencies() {
        let options = StudySelection {
            breeding: false,
            shielding: false,
            fuel_history: false,
            electricity: true,
            minimum_tbr: "1".into(),
        };
        let study = generate_study(&manifest(), "reference", &options).unwrap();
        assert_eq!(
            study.stages,
            ["transport", "normalize", "history", "energy"]
        );
        assert_eq!(
            study.contract["workflow"][2]["bindings"][0]["source"]["step_id"],
            "normalize"
        );
    }
    #[test]
    fn missing_data_never_becomes_a_compilation_or_physical_result() {
        let study = generate_study(&manifest(), "reference", &StudySelection::default()).unwrap();
        assert!(
            study.contract["execution_policy"]["require_qualification"]
                .as_bool()
                .unwrap()
        );
        assert!(study.contract.get("verdict").is_none());
        assert_eq!(study.scenario_sha256, manifest().source_sha256);
        assert!(generate_study(&manifest(), "missing", &StudySelection::default()).is_err());
    }

    #[test]
    fn shielding_selection_declares_magnet_flux_without_a_threshold() {
        let no_shield = StudySelection {
            shielding: false,
            ..StudySelection::default()
        };
        let without = generate_study(&manifest(), "reference", &no_shield).unwrap();
        let with = generate_study(
            &manifest(),
            "reference",
            &StudySelection {
                breeding: false,
                shielding: true,
                fuel_history: false,
                electricity: false,
                minimum_tbr: "1".into(),
            },
        )
        .unwrap();
        let has_flux = |study: &GeneratedStudy| {
            study.registry["capability_types"]
                .as_array()
                .unwrap()
                .iter()
                .find(|cap| cap["capability_type"]["id"] == "faris.normalize-transport")
                .unwrap()["outputs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|out| {
                    out["slot_id"] == "magnet-neutron-flux"
                        && out["role"]["id"] == "faris.magnet-neutron-flux"
                })
        };
        assert!(!has_flux(&without));
        assert!(has_flux(&with));
        assert!(
            !without.registry["kinds"]
                .as_array()
                .unwrap()
                .iter()
                .any(|kind| kind["kind_id"] == "faris.neutron_flux")
        );
        assert_ne!(without.contract, with.contract);
        assert!(
            with.contract["question"]
                .as_str()
                .unwrap()
                .contains("component-average neutron flux in the magnet (neutrons/m²/s)")
        );
        assert_eq!(with.contract["requirements"].as_array().unwrap().len(), 0);
        assert_eq!(
            with.registry["kinds"]
                .as_array()
                .unwrap()
                .iter()
                .find(|kind| kind["kind_id"] == "faris.neutron_flux")
                .unwrap()["canonical_unit"],
            "neutrons/m²/s"
        );
    }

    #[test]
    #[cfg(unix)]
    fn malformed_compiler_output_preserves_execution_without_inventing_compilation() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("invalid-test-compiler");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s' '{\"status\":\"compiled\"}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let study = generate_study(&manifest(), "reference", &StudySelection::default()).unwrap();
        let output = directory.path().join("evidence");
        let result = compile_study(&study, &executable, &output, &Cancellation::default()).unwrap();
        assert_eq!(
            result.execution.execution_status,
            ExecutionStatus::Succeeded
        );
        assert_eq!(result.report["status"], "not_available");
        assert!(output.join("compilation.json").is_file());
        assert!(compile_study(&study, &executable, &output, &Cancellation::default()).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn compilation_io_error_preserves_unavailable_receipt() {
        let directory = tempfile::tempdir().unwrap();
        let study = generate_study(&manifest(), "reference", &StudySelection::default()).unwrap();
        let output = directory.path().join("evidence");
        let missing = directory.path().join("no-such-core");
        assert!(compile_study(&study, &missing, &output, &Cancellation::default()).is_err());
        let receipt: Value =
            serde_json::from_slice(&std::fs::read(output.join("compilation.json")).unwrap())
                .unwrap();
        assert_eq!(receipt["status"], "not_available");
        assert_eq!(receipt["binding_checked"], false);
    }

    #[test]
    #[cfg(unix)]
    fn executable_change_invalidates_core_report() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("mutable-test-compiler");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s' '{\"schema_version\":\"avila.core/compile-report/v0.2-draft\",\"status\":\"compiled\"}'\nprintf '#!/bin/sh\\nexit 2\\n' > \"$0\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let study = generate_study(&manifest(), "reference", &StudySelection::default()).unwrap();
        let output = directory.path().join("evidence");
        let result = compile_study(&study, &executable, &output, &Cancellation::default()).unwrap();
        assert_eq!(
            result.execution.execution_status,
            ExecutionStatus::Succeeded
        );
        assert_eq!(result.report["status"], "not_available");
        assert_eq!(
            result.report["executable_identity_check"]["binding_checked"],
            false
        );
    }
}
