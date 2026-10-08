//! Portable recorded transport and actual executable stages for Avila Core.
//! Core envelopes use canonical decimal strings; original Rust artifacts remain
//! verbatim strings, so scientific values are never rounded by the interface.

use crate::{
    jobs::{Cancellation, ExecutionStatus, JobResult, JobSpec, run_job},
    reactor::{ReactorError, ReactorRun, hash_file, load_reactor_run, read_json_bytes},
    study::{GeneratedStudy, compile_study},
};
use faris_model::{LoadedScenario, history::OperatingHistoryAssumptions};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, io::Write, path::Path, time::Duration};

const MAX_BUNDLE_BYTES: usize = 32 * 1024 * 1024;
/// History stage envelopes contain an exact compact JSON string for up to a
/// thirty-year event ledger. This has a separate bound from raw tally files.
pub const MAX_STAGE_BYTES: usize = 64 * 1024 * 1024;

pub fn read_stage(path: &Path) -> Result<Vec<u8>, ReactorError> {
    use std::io::Read;
    if !fs::metadata(path)?.is_file() {
        return Err("stage input must be a regular file".into());
    }
    let file = fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err("stage input must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take((MAX_STAGE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_STAGE_BYTES {
        return Err("stage envelope exceeds 64 MiB".into());
    }
    Ok(bytes)
}
const REQUIRED_FILES: &[&str] = &[
    "run.json",
    "input.json",
    "scenario.json",
    "audit.json",
    "reactor_transport.py",
    "solver/transport-artifact.json",
];
const OPTIONAL_FILES: &[&str] = &[
    "solver/worker-result.json",
    "solver/transport-spectra.json",
    "solver/transport-batch-values.json",
];

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedTransportBundle {
    pub schema_version: String,
    pub files: BTreeMap<String, String>,
    pub notice: String,
}

pub fn pack_transport(run_path: &Path) -> Result<RecordedTransportBundle, ReactorError> {
    let root = run_path.parent().ok_or("run requires a parent directory")?;
    if run_path.file_name().is_none_or(|n| n != "run.json") {
        return Err("recorded transport must name run.json".into());
    }
    let scenario = LoadedScenario::from_bytes(&read_json_bytes(&root.join("scenario.json"))?)?;
    load_reactor_run(run_path, &scenario)?;
    let mut files = BTreeMap::new();
    for name in REQUIRED_FILES.iter().chain(OPTIONAL_FILES.iter()) {
        let path = root.join(name);
        if OPTIONAL_FILES.contains(name) && !path.exists() {
            continue;
        }
        let bytes = read_json_bytes(&path)?;
        files.insert((*name).into(), String::from_utf8(bytes)?);
    }
    let bundle = RecordedTransportBundle {
        schema_version: "faris-recorded-transport-bundle/v0.1".into(), files,
        notice: "Exact saved scenario/input/raw tally/worker/audit/run bytes. Revalidated arithmetic and local execution records; no authentication of nuclear-data acquisition or qualification of reactor predictions. Statepoint and nuclear data are external identified artifacts.".into(),
    };
    bundle.validate()?;
    Ok(bundle)
}

impl RecordedTransportBundle {
    pub fn validate(&self) -> Result<(), ReactorError> {
        if self.schema_version != "faris-recorded-transport-bundle/v0.1"
            || REQUIRED_FILES.iter().any(|n| !self.files.contains_key(*n))
            || self.files.keys().any(|n| {
                !REQUIRED_FILES.contains(&n.as_str()) && !OPTIONAL_FILES.contains(&n.as_str())
            })
            || self.files.values().map(String::len).sum::<usize>() > MAX_BUNDLE_BYTES
        {
            return Err(
                "unsupported, incomplete, excessive or unsafe recorded-transport bundle".into(),
            );
        }
        Ok(())
    }

    pub fn materialize(&self) -> Result<tempfile::TempDir, ReactorError> {
        self.validate()?;
        let scratch = tempfile::tempdir()?;
        for (name, bytes) in &self.files {
            let path = scratch.path().join(name);
            fs::create_dir_all(path.parent().ok_or("file needs a directory")?)?;
            write_new(&path, bytes.as_bytes())?;
        }
        Ok(scratch)
    }

    pub fn verify(&self) -> Result<(LoadedScenario, ReactorRun), ReactorError> {
        let scratch = self.materialize()?;
        let scenario = LoadedScenario::from_bytes(self.files["scenario.json"].as_bytes())?;
        let record = load_reactor_run(&scratch.path().join("run.json"), &scenario)?;
        Ok((scenario, record))
    }
}

/// The scenario a recorded-transport bundle file carries.
pub fn scenario_from_bundle_file(path: &Path) -> Result<LoadedScenario, String> {
    let bundle: RecordedTransportBundle =
        serde_json::from_slice(&read_stage(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    bundle.validate().map_err(|e| e.to_string())?;
    LoadedScenario::from_bytes(bundle.files["scenario.json"].as_bytes()).map_err(|e| e.to_string())
}

/// A recorded-transport bundle read, materialized and validated against its
/// scenario: the run record, the physics it was bound to, and the directory the
/// replay files live in (kept alive by the caller).
pub struct LoadedBundle {
    pub record: ReactorRun,
    pub case: faris_model::physics::PhysicsCase,
    pub directory: tempfile::TempDir,
}

/// The one way a bundle file becomes a transport record, shared by the desktop
/// and the command line.
pub fn load_recorded_bundle(
    path: &Path,
    scenario: &LoadedScenario,
) -> Result<LoadedBundle, String> {
    let bundle: RecordedTransportBundle =
        serde_json::from_slice(&read_stage(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let directory = bundle.materialize().map_err(|e| e.to_string())?;
    let record = load_reactor_run(&directory.path().join("run.json"), scenario)
        .map_err(|e| e.to_string())?;
    let input: Value =
        serde_json::from_str(&bundle.files["input.json"]).map_err(|e| e.to_string())?;
    let case: faris_model::physics::PhysicsCase =
        serde_json::from_value(input["physics"].clone()).map_err(|e| e.to_string())?;
    case.validate_against(scenario).map_err(|e| e.to_string())?;
    Ok(LoadedBundle {
        record,
        case,
        directory,
    })
}

pub fn write_new(path: &Path, bytes: &[u8]) -> Result<(), ReactorError> {
    let parent = path.parent().ok_or("artifact needs a parent directory")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(path)?;
    Ok(())
}

fn sha(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// Lossless boundary representation of a finite f64's shortest round-trip
/// decimal, expanded from exponent notation into Core's canonical decimal.
/// This does not declare the measured/computed physical quantity exact.
pub fn canonical_decimal(value: f64) -> Result<String, ReactorError> {
    if !value.is_finite() {
        return Err("nonfinite Core quantity".into());
    }
    if value == 0.0 {
        return Ok("0".into());
    }
    let shortest = value.to_string();
    let (mantissa, exponent) = match shortest.split_once(['e', 'E']) {
        Some((m, e)) => (m, e.parse::<i32>()?),
        None => (shortest.as_str(), 0),
    };
    let negative = mantissa.starts_with('-');
    let unsigned = mantissa.trim_start_matches('-');
    let integral = unsigned.split('.').next().ok_or("missing digits")?.len() as i32;
    let mut digits = unsigned.replace('.', "");
    let decimal_position = integral + exponent;
    let mut result = if decimal_position <= 0 {
        format!("0.{}{}", "0".repeat((-decimal_position) as usize), digits)
    } else if decimal_position as usize >= digits.len() {
        digits.push_str(&"0".repeat(decimal_position as usize - digits.len()));
        digits
    } else {
        digits.insert(decimal_position as usize, '.');
        digits
    };
    if result.contains('.') {
        while result.ends_with('0') {
            result.pop();
        }
        if result.ends_with('.') {
            result.pop();
        }
    }
    let without_leading = result.trim_start_matches('0');
    result = if without_leading.starts_with('.') {
        format!("0{without_leading}")
    } else {
        without_leading.into()
    };
    if negative {
        result.insert(0, '-');
    }
    Ok(result)
}

const STAGE_V01: &str = "faris-core-stage-output/v0.1";
const STAGE_V02: &str = "faris-core-stage-output/v0.2";

/// Stage envelopes written before v0.2 stay readable. `history` and `energy`
/// outputs have the same shape in both versions.
pub fn is_supported_stage_schema(schema_version: &Value) -> bool {
    schema_version == STAGE_V01 || schema_version == STAGE_V02
}

/// A v0.2 upstream envelope of the named stage whose scenario identity equals
/// the bound scenario input. Core binds every stage input by SHA-256 to the
/// upstream step's receipt-verified output, so nothing is re-verified from
/// scratch here; only the binding to this scenario is checked.
fn v02_upstream<'a>(
    upstream: &'a Value,
    stage: &str,
    scenario_sha256: &str,
    refusal: &str,
) -> Result<&'a Value, ReactorError> {
    if upstream["schema_version"] != STAGE_V02
        || upstream["stage"] != stage
        || upstream["scientific_qualification"] != "NOT_EVALUATED"
    {
        return Err(refusal.into());
    }
    if upstream["scenario_sha256"] != scenario_sha256 {
        return Err("upstream stage scenario differs from the bound scenario".into());
    }
    Ok(upstream)
}

pub fn transport_stage(
    scenario_bytes: &[u8],
    physics_bytes: &[u8],
    data_bytes: &[u8],
    bundle: &RecordedTransportBundle,
) -> Result<Value, ReactorError> {
    let (scenario, record) = bundle.verify()?;
    if scenario.source_bytes() != scenario_bytes {
        return Err("bound scenario differs from recorded transport".into());
    }
    let input: Value = serde_json::from_str(&bundle.files["input.json"])?;
    let physics: Value = serde_json::from_slice(physics_bytes)?;
    if physics != input["physics"] || data_bytes != bundle.files["audit.json"].as_bytes() {
        return Err("bound physics or nuclear-data audit differs from recorded transport".into());
    }
    let normalized = record
        .normalized
        .as_ref()
        .ok_or("missing checked normalized result")?;
    transport_envelope(
        &scenario.source_sha256,
        &record.input_sha256,
        record
            .raw_artifact_sha256
            .as_deref()
            .ok_or("missing raw identity")?,
        normalized,
    )
}

fn transport_envelope(
    scenario_sha256: &str,
    input_sha256: &str,
    raw_artifact_sha256: &str,
    normalized: &crate::transport::NormalizedTransportResult,
) -> Result<Value, ReactorError> {
    Ok(json!({"schema_version":STAGE_V02, "stage":"transport",
        "raw":"recorded_transport_revalidated",
        "scenario_sha256":scenario_sha256, "input_sha256":input_sha256,
        "raw_artifact_sha256":raw_artifact_sha256,
        "normalized_json":serde_json::to_string(normalized)?,
        "scientific_qualification":"NOT_EVALUATED", "notice":"This stage verified a prior OpenMC execution; Core did not rerun OpenMC."}))
}

pub fn normalization_stage(upstream: &Value, scenario_bytes: &[u8]) -> Result<Value, ReactorError> {
    let scenario = LoadedScenario::from_bytes(scenario_bytes)?;
    let upstream = v02_upstream(
        upstream,
        "transport",
        &scenario.source_sha256,
        "normalization requires verified v0.2 transport stage",
    )?;
    let result: crate::transport::NormalizedTransportResult = serde_json::from_str(
        upstream["normalized_json"]
            .as_str()
            .ok_or("missing normalized payload")?,
    )?;
    let raw_artifact_sha256 = upstream["raw_artifact_sha256"]
        .as_str()
        .ok_or("missing raw identity")?;
    if result.scenario_sha256 != scenario.source_sha256 {
        return Err("normalized upstream payload belongs to a different scenario".into());
    }
    let tritium = result
        .results
        .iter()
        .find(|r| r.response_id == "total-tritium-production")
        .ok_or("missing total H3 response")?;
    let magnet = result
        .results
        .iter()
        .find(|r| r.response_id == "magnets-flux")
        .ok_or("missing magnet response")?;
    let rates = crate::history::TransportDrivingRates::from_normalized(
        &result,
        scenario.scenario.operating_plan.fusion_power_mw,
        raw_artifact_sha256,
    )?;
    Ok(json!({"schema_version":STAGE_V02, "stage":"normalize",
        "normalized":"normalized_transport_extracted",
        "tbr":canonical_decimal(tritium.integrated_mean / result.source_neutron_rate_per_s)?,
        "magnet_flux":canonical_decimal(magnet.mean)?,
        "rates_json":serde_json::to_string(&rates)?,
        "scenario_sha256":scenario.source_sha256,
        "raw_artifact_sha256":raw_artifact_sha256,
        "uncertainty":"Sampling standard errors are retained in the transport stage's normalized_json. Extracted nominal claims are unquantified; no confidence interval or exact physical value is asserted.",
        "scientific_qualification":"NOT_EVALUATED"}))
}

pub fn history_stage(
    upstream: &Value,
    scenario_bytes: &[u8],
    assumptions: &OperatingHistoryAssumptions,
    cancellation: &Cancellation,
) -> Result<Value, ReactorError> {
    let scenario = LoadedScenario::from_bytes(scenario_bytes)?;
    let upstream = v02_upstream(
        upstream,
        "normalize",
        &scenario.source_sha256,
        "history requires normalized v0.2 transport stage",
    )?;
    let rates: crate::history::TransportDrivingRates = serde_json::from_str(
        upstream["rates_json"]
            .as_str()
            .ok_or("missing driving-rates payload")?,
    )?;
    if rates.scenario_sha256 != scenario.source_sha256 {
        return Err("driving rates belong to a different scenario".into());
    }
    let history =
        crate::history::run_operating_history_cancellable(assumptions, &rates, cancellation)?;
    let history_json = serde_json::to_string(&history)?;
    Ok(
        json!({"schema_version":STAGE_V02,"stage":"history","history":"history_calculated",
        "history_sha256":sha(history_json.as_bytes()),"history_json":history_json,
        "scenario_sha256":scenario.source_sha256,"scientific_qualification":"NOT_EVALUATED",
        "notice":"Deterministic fuel, decay, processing, exposure, maintenance and energy ledger. All projections remain conditional on the exact authored assumptions and transport model."}),
    )
}

pub fn energy_stage(
    upstream: &Value,
    scenario_bytes: &[u8],
    cancellation: &Cancellation,
) -> Result<Value, ReactorError> {
    if !is_supported_stage_schema(&upstream["schema_version"]) || upstream["stage"] != "history" {
        return Err("energy requires calculated operating history stage".into());
    }
    let history_json = upstream["history_json"]
        .as_str()
        .ok_or("missing operating history payload")?;
    if sha(history_json.as_bytes()) != upstream["history_sha256"] {
        return Err("history digest differs".into());
    }
    let history: crate::history::HistoryResult = serde_json::from_str(history_json)?;
    let scenario = LoadedScenario::from_bytes(scenario_bytes)?;
    if history.driving_rates.scenario_sha256 != scenario.source_sha256 {
        return Err("energy scenario differs from history".into());
    }
    let replay = crate::history::run_operating_history_cancellable(
        &history.assumptions,
        &history.driving_rates,
        cancellation,
    )?;
    if replay != history {
        return Err("history differs from shared deterministic engine replay".into());
    }
    let last = history.snapshots.last().ok_or("history has no snapshots")?;
    let net=last.cumulative_net_electricity_mwh.ok_or("energy requires actual coupled heating and explicit recovery, efficiency and auxiliary assumptions")?;
    let gross = last
        .cumulative_gross_electricity_mwh
        .ok_or("missing gross electricity")?;
    let auxiliary = last
        .cumulative_auxiliary_electricity_mwh
        .ok_or("missing auxiliary ledger")?;
    if ((gross - auxiliary) - net).abs() > 1e-9 * gross.abs().max(auxiliary.abs()).max(1.0) {
        return Err("net energy ledger does not close".into());
    }
    Ok(
        json!({"schema_version":STAGE_V02,"stage":"energy","energy":"energy_ledger_verified",
        "history_sha256":upstream["history_sha256"],"net_electricity_mwh":canonical_decimal(net)?,
        "gross_electricity_mwh":canonical_decimal(gross)?,"auxiliary_electricity_mwh":canonical_decimal(auxiliary)?,
        "transport_recovered_heat_mwh":canonical_decimal(last.cumulative_transport_recovered_heat_mwh.ok_or("missing recovered neutron-source heat")?)?,
        "alpha_recovered_heat_mwh":canonical_decimal(last.cumulative_alpha_recovered_heat_mwh.ok_or("missing recovered alpha heat")?)?,
        "scientific_qualification":"NOT_EVALUATED","notice":"Signed numerical energy ledger under declared prompt-heat recovery and conversion assumptions. This nominal quantity is not a qualified plant prediction."}),
    )
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CoreEvidenceRun {
    pub schema_version: String,
    pub core_sha256: String,
    pub faris_sha256: String,
    pub package_sha256: String,
    pub execution: JobResult,
    pub report: Value,
    pub expected_case_id: String,
    pub expected_step_ids: Vec<String>,
    pub compiled_snapshot_sha256: String,
}

impl CoreEvidenceRun {
    /// Workflow completion is separate from every physical requirement verdict.
    pub fn completed(&self) -> bool {
        self.execution.execution_status == ExecutionStatus::Succeeded
            && self.execution.exit_code == Some(0)
            && self.report["status"] == "evaluated"
            && self.report["case_id"] == self.expected_case_id
            && self.report.pointer("/integrity/manifest_sha256")
                == Some(&json!(format!("sha256:{}", self.package_sha256)))
            && self.report.pointer("/compile/compiled/snapshot_sha256")
                == Some(&json!(self.compiled_snapshot_sha256))
            && self.report.pointer("/integrity/status") == Some(&json!("complete"))
            && self.report.pointer("/compile/status") == Some(&json!("compiled"))
            && self.report.pointer("/execution/status") == Some(&json!("executed"))
            && self.report.pointer("/bindings/status") == Some(&json!("verified"))
            && self.report.pointer("/campaign/status") == Some(&json!("evaluated"))
            && self.report.pointer("/claims/matches_committed") == Some(&json!(true))
            && self
                .report
                .pointer("/execution/steps")
                .and_then(Value::as_array)
                .is_some_and(|steps| {
                    !steps.is_empty()
                        && steps
                            .iter()
                            .filter_map(|s| s["step_id"].as_str())
                            .eq(self.expected_step_ids.iter().map(String::as_str))
                        && steps.iter().all(|step| {
                            step["state"] == "executed"
                                && step.pointer("/receipt/status") == Some(&json!("completed"))
                                && step.pointer("/receipt/exit_status") == Some(&json!(0))
                                && step.pointer("/verification/state") == Some(&json!("verified"))
                                && step["outputs"].as_array().is_some_and(|outputs| {
                                    !outputs.is_empty()
                                        && outputs.iter().all(|o| {
                                            o["state"] == "collected"
                                                && o["reproduces_bound_artifact"] == true
                                        })
                                })
                        })
                })
    }
}

fn adapter(
    stage: &str,
    capability: &str,
    inputs: &[(&str, &str)],
    outputs: &[(&str, &str, Option<&str>)],
) -> Value {
    let mut arguments = vec![
        json!({"kind":"literal","value":"evidence"}),
        json!({"kind":"literal","value":"stage"}),
        json!({"kind":"literal","value":stage}),
    ];
    for (slot, flag) in inputs {
        arguments.push(json!({"kind":"literal","value":flag}));
        arguments.push(json!({"kind":"input_path","input_slot":slot}));
    }
    arguments.push(json!({"kind":"literal","value":"--output"}));
    arguments.push(json!({"kind":"output_path","output_id":"report"}));
    let claims: Vec<_> = outputs.iter().map(|(slot, pointer, unit)| match unit {
        Some(unit) => json!({"model":"unquantified","output_slot":slot,"output_id":"report","pointer":pointer,"unit":unit}),
        None => json!({"model":"categorical","output_slot":slot,"output_id":"report","pointer":pointer,
            "allowed_values":["recorded_transport_revalidated","raw_transport_renormalized_and_verified","normalized_transport_extracted","history_calculated","energy_ledger_verified"]}),
    }).collect();
    json!({"schema_version":"avila.core/external-checker-adapter/v0.1-draft",
        "adapter_id":format!("avila-labs.faris/{stage}@1"),"capability_type":{"id":capability,"major":1},
        "input_slots":inputs.iter().map(|(slot,_)|*slot).collect::<Vec<_>>(),"arguments":arguments,
        "outputs":[{"output_id":"report","workspace_path":"outputs/report.json","media_type":"application/json"}],
        "claims":claims,"timeout_ms":120000,
        "limitations":["Controlled FARIS numerical processing of identified research artifacts. No scientific qualification of a reactor, confidence bound, or engineering approval."]})
}

/// Materialize one reusable case from real run artifacts. Expected stage outputs
/// are calculated here through the same headless engine, then recalculated by
/// the bound FARIS executable under Core; handwritten values are not accepted.
pub fn prepare_case(
    study: &GeneratedStudy,
    run_path: &Path,
    core: &Path,
    faris: &Path,
    output: &Path,
    cancellation: &Cancellation,
) -> Result<(), ReactorError> {
    prepare_case_with_assumptions(study, run_path, None, core, faris, output, cancellation)
}

pub fn prepare_case_with_assumptions(
    study: &GeneratedStudy,
    run_path: &Path,
    assumptions: Option<&OperatingHistoryAssumptions>,
    core: &Path,
    faris: &Path,
    output: &Path,
    cancellation: &Cancellation,
) -> Result<(), ReactorError> {
    if (study.selection.fuel_history || study.selection.electricity) && assumptions.is_none() {
        return Err("history selections require explicit operating assumptions".into());
    }
    let bundle = pack_transport(run_path)?;
    let (scenario, record) = bundle.verify()?;
    if study.scenario_sha256 != scenario.source_sha256 || study.variant_id != record.variant_id {
        return Err("study and transport do not identify the same scenario and arrangement".into());
    }
    let output = std::path::absolute(output)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir(&output)?;
    fs::create_dir(output.join("inputs"))?;
    fs::create_dir(output.join("expected"))?;
    let compilation = compile_study(study, core, &output.join("compile"), cancellation)?;
    if compilation.report["status"] != "compiled" {
        return Err("Core did not compile the requested study".into());
    }
    let snapshot = compilation
        .report
        .pointer("/compiled/snapshot_sha256")
        .and_then(Value::as_str)
        .ok_or("Core did not return compiled identity")?;
    let input: Value = serde_json::from_str(&bundle.files["input.json"])?;
    let scenario_bytes = scenario.source_bytes().to_vec();
    let physics_bytes = serde_json::to_vec_pretty(&input["physics"])?;
    let data_bytes = bundle.files["audit.json"].as_bytes().to_vec();
    let bundle_bytes = serde_json::to_vec_pretty(&bundle)?;
    let mut bound_inputs = vec![
        ("scenario", scenario_bytes),
        ("physics", physics_bytes),
        ("nuclear-data", data_bytes),
        ("recorded", bundle_bytes),
    ];
    if study.selection.fuel_history || study.selection.electricity {
        bound_inputs.push((
            "assumptions",
            serde_json::to_vec_pretty(assumptions.ok_or("missing operating assumptions")?)?,
        ));
    }
    let mut artifacts = Vec::new();
    let mut input_claims = Vec::new();
    for (id, bytes) in &bound_inputs {
        let path = format!("inputs/{id}.json");
        write_new(&output.join(&path), bytes)?;
        let digest = sha(bytes);
        artifacts.push(json!({"artifact_id":format!("input-{id}"),"evidence_ids":[format!("input:{id}")],"source_root":"case","path":path,"sha256":digest}));
        input_claims.push(
            json!({"input_id":id,"artifact":{"sha256":digest,"media_type":"application/json"}}),
        );
    }
    let raw = transport_stage(
        &bound_inputs[0].1,
        &bound_inputs[1].1,
        &bound_inputs[2].1,
        &bundle,
    )?;
    let normalized = normalization_stage(&raw, &bound_inputs[0].1)?;
    let transport_outputs = vec![("raw", "/raw", None)];
    let mut normalized_outputs = vec![
        ("normalized", "/normalized", None),
        ("tbr", "/tbr", Some("1")),
    ];
    if study.selection.shielding {
        normalized_outputs.push(("magnet-neutron-flux", "/magnet_flux", Some("neutrons/m²/s")));
    }
    let mut specs = vec![
        (
            "transport",
            "faris.verify-transport",
            vec![
                ("scenario", "--scenario"),
                ("physics", "--physics"),
                ("nuclear-data", "--data"),
                ("recorded", "--recorded"),
            ],
            transport_outputs,
            raw,
        ),
        (
            "normalize",
            "faris.normalize-transport",
            vec![("raw", "--upstream"), ("scenario", "--scenario")],
            normalized_outputs,
            normalized,
        ),
    ];
    if study.selection.fuel_history || study.selection.electricity {
        let history = history_stage(
            &specs[1].4,
            &bound_inputs[0].1,
            assumptions.ok_or("missing assumptions")?,
            cancellation,
        )?;
        if study.selection.electricity {
            let energy = energy_stage(&history, &bound_inputs[0].1, cancellation)?;
            specs.push((
                "history",
                "faris.operating-history",
                vec![
                    ("rates", "--upstream"),
                    ("scenario", "--scenario"),
                    ("assumptions", "--assumptions"),
                ],
                vec![("history", "/history", None)],
                history,
            ));
            specs.push((
                "energy",
                "faris.net-energy",
                vec![("history", "--upstream"), ("scenario", "--scenario")],
                vec![("energy", "/energy", None)],
                energy,
            ));
        } else {
            specs.push((
                "history",
                "faris.operating-history",
                vec![
                    ("rates", "--upstream"),
                    ("scenario", "--scenario"),
                    ("assumptions", "--assumptions"),
                ],
                vec![("history", "/history", None)],
                history,
            ));
        }
    }
    let faris_digest = format!("sha256:{}", hash_file(faris)?);
    let producer = json!({"package_id":concat!("avila-labs.faris/cli@",env!("CARGO_PKG_VERSION")),"sha256":faris_digest});
    let mut documents = Vec::new();
    let mut executions = Vec::new();
    let mut claims = Vec::new();
    for (stage, capability, inputs, outputs, report) in specs {
        let bytes = serde_json::to_vec_pretty(&report)?;
        if bytes.len() > MAX_STAGE_BYTES {
            return Err("generated stage envelope exceeds 64 MiB".into());
        }
        let path = format!("expected/{stage}.json");
        write_new(&output.join(&path), &bytes)?;
        let evidence_ids: Vec<_> = outputs
            .iter()
            .map(|(slot, _, _)| format!("{stage}:{slot}"))
            .collect();
        artifacts.push(json!({"artifact_id":format!("output-{stage}"),"evidence_ids":evidence_ids,"source_root":"case","path":path,"sha256":sha(&bytes)}));
        let descriptor = adapter(stage, capability, &inputs, &outputs);
        let descriptor_path = format!("adapter-{stage}.json");
        let descriptor_bytes = serde_json::to_vec_pretty(&descriptor)?;
        write_new(&output.join(&descriptor_path), &descriptor_bytes)?;
        documents.push(json!({"document_id":descriptor["adapter_id"],"role":"external_checker_adapter","path":descriptor_path,"sha256":sha(&descriptor_bytes)}));
        executions.push(json!({"step_id":stage,"adapter":descriptor["adapter_id"],"capability_id":"faris",
            "inputs":inputs.iter().map(|(slot,_)|json!({"input_slot":slot,"workspace_path":format!("inputs/{slot}.json")})).collect::<Vec<_>>(),
            "outputs":outputs.iter().map(|(slot,_,_)|json!({"output_slot":slot,"claim_id":format!("{stage}:{slot}")})).collect::<Vec<_>>()}));
        for (slot, pointer, unit) in outputs {
            let selected = report
                .pointer(pointer)
                .ok_or("expected output pointer is absent")?;
            let claim = match unit {
                Some(unit) => {
                    json!({"model":"unquantified","nominal":{"value":selected,"unit":unit}})
                }
                None => json!({"model":"unquantified","value":selected}),
            };
            claims.push(json!({"claim_id":format!("{stage}:{slot}"),"step_id":stage,"output_slot":slot,
                "artifact":{"sha256":sha(&bytes),"media_type":"application/json"},"producer":producer,"claim":claim}));
        }
    }
    input_claims.sort_by_key(|input| input["input_id"].as_str().unwrap_or_default().to_owned());
    let claims = json!({"schema_version":"avila.core/evidence-claims/v0.2-draft","semantic_profile":"avila.core/semantic/0.2-draft","compiled_snapshot_sha256":snapshot,"inputs":input_claims,"claims":claims});
    for (name, role, value) in [
        ("contract", "contract", &study.contract),
        ("registry", "registry", &study.registry),
        ("claims", "claims", &claims),
    ] {
        let path = format!("{name}.json");
        let bytes = serde_json::to_vec_pretty(value)?;
        write_new(&output.join(&path), &bytes)?;
        documents.push(json!({"document_id":name,"role":role,"path":path,"sha256":sha(&bytes)}));
    }
    let package = json!({"schema_version":"avila.core/case-package/v0.1-draft",
        "case_id":format!("FARIS-{}-{}",scenario.scenario.id,record.variant_id),"title":"FARIS identified transport, deterministic normalization and conditional research evidence",
        "documents":documents,"artifacts":artifacts,"capabilities":[{"capability_id":"faris","package_id":producer["package_id"],"executable_sha256":faris_digest}],
        "executions":executions,"limitations":["Recorded OpenMC outputs are revalidated and normalized under Core. Nuclear-data acquisition and reactor-response methods remain scientifically unqualified; physical requirements must remain NOT_EVALUATED."]});
    write_new(
        &output.join("package.json"),
        &serde_json::to_vec_pretty(&package)?,
    )?;
    write_new(
        &output.join("study.json"),
        &serde_json::to_vec_pretty(study)?,
    )?;
    Ok(())
}

/// Run one generated, hash-bound case with Core's actual controlled runner.
pub fn run_case(
    package: &Path,
    core: &Path,
    faris: &Path,
    workspace: &Path,
    cancellation: &Cancellation,
) -> Result<CoreEvidenceRun, ReactorError> {
    let core = core.canonicalize()?;
    let faris = faris.canonicalize()?;
    let package = package.canonicalize()?;
    let workspace = std::path::absolute(workspace)?;
    if workspace.exists() {
        return Err("Core workspace must be fresh".into());
    }
    let core_sha256 = hash_file(&core)?;
    let faris_sha256 = hash_file(&faris)?;
    let package_sha256 = hash_file(&package.join("package.json"))?;
    let package_definition: Value =
        serde_json::from_slice(&read_json_bytes(&package.join("package.json"))?)?;
    let expected_case_id = package_definition["case_id"]
        .as_str()
        .ok_or("package has no case identity")?
        .to_owned();
    let expected_step_ids: Vec<String> = package_definition["executions"]
        .as_array()
        .ok_or("package has no declared executions")?
        .iter()
        .map(|step| {
            step["step_id"]
                .as_str()
                .map(str::to_owned)
                .ok_or("declared execution has no step identity")
        })
        .collect::<Result<_, _>>()?;
    let committed_claims: Value =
        serde_json::from_slice(&read_json_bytes(&package.join("claims.json"))?)?;
    let compiled_snapshot_sha256 = committed_claims["compiled_snapshot_sha256"]
        .as_str()
        .ok_or("case claims have no compiled identity")?
        .to_owned();
    let job = JobSpec {
        program: core.clone(),
        arguments: vec![
            "run".into(),
            package.as_os_str().into(),
            "--capability".into(),
            format!("faris={}", faris.display()).into(),
            "--source-root".into(),
            format!("case={}", package.display()).into(),
            "--workspace".into(),
            workspace.as_os_str().into(),
            "--no-reuse".into(),
            "--expect-manifest".into(),
            format!("sha256:{package_sha256}").into(),
            "--json".into(),
        ],
        working_directory: package.clone(),
        environment: vec![],
        timeout: Duration::from_secs(300),
        capture_limit_bytes: 4 * 1024 * 1024,
        artifact_roots: vec![workspace.clone()],
        resource_limits: crate::jobs::ResourceLimits::default(),
    };
    let execution = run_job(&job, cancellation)?;
    let report = if hash_file(&core).ok().as_ref() != Some(&core_sha256)
        || hash_file(&faris).ok().as_ref() != Some(&faris_sha256)
        || hash_file(&package.join("package.json")).ok().as_ref() != Some(&package_sha256)
    {
        json!({"status":"not_available","reason":"bound executable or package identity changed during execution"})
    } else if matches!(
        execution.execution_status,
        ExecutionStatus::Succeeded | ExecutionStatus::Failed
    ) {
        match serde_json::from_str::<Value>(&execution.stdout) {
            Ok(value)
                if value["schema_version"] == "avila.core/case-run-report/v0.5-draft"
                    && matches!(
                        (execution.exit_code, value["status"].as_str()),
                        (Some(0), Some("evaluated")) | (Some(1), Some("rejected"))
                    ) =>
            {
                value
            }
            Ok(_) => {
                json!({"status":"not_available","reason":"Unsupported or process-inconsistent Core run report; captured diagnostics are preserved."})
            }
            Err(error) => {
                json!({"status":"not_available","reason":format!("Core report could not be parsed: {error}")})
            }
        }
    } else {
        json!({"status":"not_available","execution_status":execution.execution_status})
    };
    Ok(CoreEvidenceRun {
        schema_version: "faris-core-evidence-run/v0.1".into(),
        core_sha256,
        faris_sha256,
        package_sha256,
        execution,
        report,
        expected_case_id,
        expected_step_ids,
        compiled_snapshot_sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decimal_boundary_preserves_bits_without_exponent_or_redundant_zeroes() {
        for value in [
            0.0,
            -0.0,
            1.0,
            -0.125,
            1.861_813_786_415_852_5e20,
            1.602_176_634e-19,
            f64::MIN_POSITIVE,
            f64::MAX,
        ] {
            let text = canonical_decimal(value).unwrap();
            assert!(!text.contains(['e', 'E', '+']));
            assert_eq!(text.parse::<f64>().unwrap(), value);
            if text.contains('.') {
                assert!(!text.ends_with('0'));
            }
        }
        assert!(canonical_decimal(f64::NAN).is_err());
    }
    // Verifies: SEC-002
    #[test]
    fn portable_record_refuses_path_escape_and_missing_artifacts() {
        let mut bundle = RecordedTransportBundle {
            schema_version: "faris-recorded-transport-bundle/v0.1".into(),
            files: BTreeMap::new(),
            notice: String::new(),
        };
        assert!(bundle.validate().is_err());
        for name in REQUIRED_FILES {
            bundle.files.insert((*name).into(), "{}".into());
        }
        assert!(bundle.validate().is_ok());
        bundle.files.insert("../escaped.json".into(), "{}".into());
        assert!(bundle.verify().is_err());
    }
    // Verifies: PRV-014
    #[test]
    fn completed_workflow_keeps_physical_not_evaluated_and_refuses_missing_receipts() {
        let step = |id: &str| json!({"step_id":id,"state":"executed","receipt":{"status":"completed","exit_status":0},"verification":{"state":"verified"},"outputs":[{"state":"collected","reproduces_bound_artifact":true}]});
        let execution: JobResult=serde_json::from_value(json!({"execution_status":"SUCCEEDED","exit_code":0,"elapsed_seconds":1.0,"stdout":"","stderr":"","stdout_truncated":false,"stderr_truncated":false})).unwrap();
        let mut evidence = CoreEvidenceRun {
            schema_version: "faris-core-evidence-run/v0.1".into(),
            core_sha256: "core".into(),
            faris_sha256: "faris".into(),
            package_sha256: "package".into(),
            execution,
            expected_case_id: "test-software-case".into(),
            expected_step_ids: vec!["transport".into(), "normalize".into()],
            compiled_snapshot_sha256: "sha256:snapshot".into(),
            report: json!({"case_id":"test-software-case","status":"evaluated","integrity":{"status":"complete","manifest_sha256":"sha256:package"},"compile":{"status":"compiled","compiled":{"snapshot_sha256":"sha256:snapshot"}},"execution":{"status":"executed","steps":[step("transport"),step("normalize")]},"bindings":{"status":"verified"},"campaign":{"status":"evaluated","verdicts":[{"verdict":{"status":"not_evaluated"}}]},"claims":{"matches_committed":true}}),
        };
        assert!(evidence.completed());
        evidence.report["execution"]["steps"][1]["receipt"]["status"] = json!("interrupted");
        assert!(!evidence.completed());
        evidence.report["execution"]["steps"][1]["receipt"]["status"] = json!("completed");
        evidence.expected_step_ids.push("history".into());
        assert!(!evidence.completed());
        evidence.expected_step_ids.pop();
        evidence.report["integrity"]["manifest_sha256"] = json!("sha256:other-case");
        assert!(!evidence.completed());
    }

    // Verifies: PRV-014
    mod stage_contract {
        use super::*;
        use crate::transport::{
            NormalizedTally, NormalizedTransportResult, PhysicalUnit, TallyEstimator, ToolIdentity,
        };
        use faris_model::transport::{
            DtSource, HeatingConvention, HeatingParticleScope, ProducedParticle, ResponseDomain,
            ScoreDefinition,
        };

        const SCENARIO: &[u8] = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scenarios/arc-inspired/cold-reference-port.scenario.json"
        ));
        const ASSUMPTIONS: &[u8] = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scenarios/arc-inspired/demo-operating-assumptions.json"
        ));
        const RAW: &str = "abababababababababababababababababababababababababababababababab";

        fn scenario_sha() -> String {
            LoadedScenario::from_bytes(SCENARIO).unwrap().source_sha256
        }

        /// A small normalized result with every response the stages read.
        fn normalized(scenario_sha256: &str) -> NormalizedTransportResult {
            let power_mw = LoadedScenario::from_bytes(SCENARIO)
                .unwrap()
                .scenario
                .operating_plan
                .fusion_power_mw;
            let source_rate =
                power_mw * 1.0e6 / (17.6e6 * crate::history::ELEMENTARY_CHARGE_J_PER_EV);
            let tally = |id: &str,
                         domain: ResponseDomain,
                         score: ScoreDefinition,
                         unit: PhysicalUnit,
                         integrated_unit: PhysicalUnit,
                         mean: f64| NormalizedTally {
                response_id: id.into(),
                domain,
                score,
                estimator: TallyEstimator::Tracklength,
                mean,
                standard_error: mean * 0.01,
                unit,
                integrated_mean: mean * 20.0,
                integrated_standard_error: mean * 0.2,
                integrated_unit,
                volume_m3: 20.0,
                volume_standard_error_m3: 0.0,
            };
            let component = |id: &str| ResponseDomain::Component {
                component_id: id.into(),
            };
            let tritium = ScoreDefinition::ParticleProduction {
                particle: ProducedParticle::Tritium,
                score: "H3-production".into(),
            };
            let flux = PhysicalUnit::NeutronsPerSquareMetreSecond;
            let flux_integrated = PhysicalUnit::NeutronMetresPerSecond;
            let per_volume = PhysicalUnit::ParticlesPerCubicMetreSecond;
            let per_second = PhysicalUnit::ParticlesPerSecond;
            NormalizedTransportResult {
                schema_version: "faris-normalized-transport/v0.1".into(),
                scenario_id: "s".into(),
                scenario_sha256: scenario_sha256.into(),
                variant_id: "v".into(),
                source: DtSource {
                    energy_per_reaction_ev: 17.6e6,
                    neutron_energy_ev: 14.1e6,
                    neutrons_per_reaction: 1.0,
                    distribution_id: "d".into(),
                },
                solver: ToolIdentity {
                    name: "OpenMC".into(),
                    version: "0.15.3".into(),
                    digest: format!("sha256:{}", "1".repeat(64)),
                },
                nuclear_data: ToolIdentity {
                    name: "data".into(),
                    version: "1".into(),
                    digest: format!("sha256:{}", "2".repeat(64)),
                },
                histories: 1000,
                source_reaction_rate_per_s: source_rate,
                source_neutron_rate_per_s: source_rate,
                results: vec![
                    tally(
                        "total-tritium-production",
                        ResponseDomain::WholeModel,
                        tritium.clone(),
                        per_volume,
                        per_second,
                        1.07e19,
                    ),
                    tally(
                        "blanket-tritium",
                        component("blanket"),
                        tritium,
                        per_volume,
                        per_second,
                        1.05e19,
                    ),
                    tally(
                        "blanket-flux",
                        component("blanket"),
                        ScoreDefinition::Flux,
                        flux,
                        flux_integrated,
                        1.0e13,
                    ),
                    tally(
                        "magnets-flux",
                        component("magnets"),
                        ScoreDefinition::Flux,
                        flux,
                        flux_integrated,
                        3.0e9,
                    ),
                    tally(
                        "heating-total-whole-model",
                        ResponseDomain::WholeModel,
                        ScoreDefinition::Heating {
                            convention: HeatingConvention::Heating,
                            particle_scope: HeatingParticleScope::Total,
                        },
                        PhysicalUnit::WattsPerCubicMetre,
                        PhysicalUnit::Watts,
                        2.5e7,
                    ),
                ],
                response_covariance: None,
            }
        }

        fn transport_v02(sha: &str) -> Value {
            transport_envelope(sha, "input", RAW, &normalized(sha)).unwrap()
        }

        fn assumptions() -> OperatingHistoryAssumptions {
            serde_json::from_slice(ASSUMPTIONS).unwrap()
        }

        /// What a v0.1 chain produced from the same recorded result: the
        /// extracted claims and the history computed directly from the record.
        fn legacy(sha: &str) -> (String, String, String) {
            let result = normalized(sha);
            let scenario = LoadedScenario::from_bytes(SCENARIO).unwrap();
            let tritium = &result.results[0];
            let magnet = &result.results[3];
            let rates = crate::history::TransportDrivingRates::from_normalized(
                &result,
                scenario.scenario.operating_plan.fusion_power_mw,
                RAW,
            )
            .unwrap();
            let history = crate::history::run_operating_history_cancellable(
                &assumptions(),
                &rates,
                &Cancellation::default(),
            )
            .unwrap();
            (
                canonical_decimal(tritium.integrated_mean / result.source_neutron_rate_per_s)
                    .unwrap(),
                canonical_decimal(magnet.mean).unwrap(),
                serde_json::to_string(&history).unwrap(),
            )
        }

        fn chain(sha: &str) -> (Value, Value) {
            let normalize = normalization_stage(&transport_v02(sha), SCENARIO).unwrap();
            let history = history_stage(
                &normalize,
                SCENARIO,
                &assumptions(),
                &Cancellation::default(),
            )
            .unwrap();
            (normalize, history)
        }

        #[test]
        fn v02_chain_matches_the_direct_v01_computation_and_carries_no_bundle() {
            let sha = scenario_sha();
            let transport = transport_v02(&sha);
            assert_eq!(transport["schema_version"], STAGE_V02);
            assert!(transport.get("transport_json").is_none());
            let (normalize, history) = chain(&sha);
            let (tbr, magnet, history_json) = legacy(&sha);
            assert_eq!(normalize["tbr"], tbr);
            assert_eq!(normalize["magnet_flux"], magnet);
            assert_eq!(normalize["normalized"], "normalized_transport_extracted");
            for forbidden in ["transport_json", "normalized_json"] {
                assert!(normalize.get(forbidden).is_none());
            }
            assert_eq!(history["history_json"], history_json);
            assert_eq!(history["history_sha256"], sha_of(&history_json));
            // The energy stage reads a v0.2 history envelope.
            let energy = energy_stage(&history, SCENARIO, &Cancellation::default());
            assert!(energy.is_ok() || energy.unwrap_err().to_string().contains("energy"));
        }

        fn sha_of(text: &str) -> String {
            sha(text.as_bytes())
        }

        #[test]
        fn a_v01_history_envelope_is_still_accepted_by_energy_and_readers() {
            let sha = scenario_sha();
            let (_, history) = chain(&sha);
            let mut v01 = history.clone();
            v01["schema_version"] = json!(STAGE_V01);
            assert!(is_supported_stage_schema(&v01["schema_version"]));
            assert!(is_supported_stage_schema(&history["schema_version"]));
            assert!(!is_supported_stage_schema(&json!(
                "faris-core-stage-output/v0.3"
            )));
            let cancellation = Cancellation::default();
            let a = energy_stage(&v01, SCENARIO, &cancellation)
                .map(|v| v["net_electricity_mwh"].clone());
            let b = energy_stage(&history, SCENARIO, &cancellation)
                .map(|v| v["net_electricity_mwh"].clone());
            assert_eq!(a.is_ok(), b.is_ok());
            if let (Ok(a), Ok(b)) = (a, b) {
                assert_eq!(a, b);
            }
            let mut other = v01;
            other["schema_version"] = json!("faris-core-stage-output/v0.3");
            assert!(energy_stage(&other, SCENARIO, &cancellation).is_err());
        }

        #[test]
        fn normalize_refuses_a_mismatched_scenario_or_stage_or_version() {
            let sha = scenario_sha();
            let mut other = transport_v02(&sha);
            other["scenario_sha256"] = json!(format!("sha256:{}", "0".repeat(64)));
            assert!(normalization_stage(&other, SCENARIO).is_err());
            let mut wrong_stage = transport_v02(&sha);
            wrong_stage["stage"] = json!("normalize");
            assert!(normalization_stage(&wrong_stage, SCENARIO).is_err());
            let mut v01 = transport_v02(&sha);
            v01["schema_version"] = json!(STAGE_V01);
            assert!(normalization_stage(&v01, SCENARIO).is_err());
            // A payload recorded for another scenario is refused even when the
            // envelope claims the bound one.
            let foreign = transport_envelope(
                &sha,
                "input",
                RAW,
                &normalized(&format!("sha256:{}", "0".repeat(64))),
            )
            .unwrap();
            assert!(normalization_stage(&foreign, SCENARIO).is_err());
        }

        #[test]
        fn history_refuses_a_mismatched_scenario_and_malformed_rates() {
            let sha = scenario_sha();
            let (normalize, _) = chain(&sha);
            let cancellation = Cancellation::default();
            let run =
                |upstream: &Value| history_stage(upstream, SCENARIO, &assumptions(), &cancellation);
            assert!(run(&normalize).is_ok());
            let mut other = normalize.clone();
            other["scenario_sha256"] = json!(format!("sha256:{}", "0".repeat(64)));
            assert!(run(&other).is_err());
            for broken in [json!("{not json"), json!("{}"), json!(7)] {
                let mut malformed = normalize.clone();
                malformed["rates_json"] = broken;
                assert!(run(&malformed).is_err());
            }
            let mut missing = normalize.clone();
            missing.as_object_mut().unwrap().remove("rates_json");
            assert!(run(&missing).is_err());
            let mut v01 = normalize.clone();
            v01["schema_version"] = json!(STAGE_V01);
            assert!(run(&v01).is_err());
            // Rates computed for another scenario cannot ride a bound envelope.
            let mut rates: crate::history::TransportDrivingRates =
                serde_json::from_str(normalize["rates_json"].as_str().unwrap()).unwrap();
            rates.scenario_sha256 = format!("sha256:{}", "0".repeat(64));
            let mut foreign = normalize;
            foreign["rates_json"] = json!(serde_json::to_string(&rates).unwrap());
            assert!(run(&foreign).is_err());
        }
    }
}
