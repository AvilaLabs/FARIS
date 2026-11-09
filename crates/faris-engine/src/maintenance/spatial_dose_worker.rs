// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! Bounded OpenMC R2S curve production for exact operating-history replacement events.
//!
//! This adapter only accepts an explicit, hash-verified CAD/OpenMC context. It carries the exact
//! Rust history bytes and per-installation schedules into the Python worker; Python never
//! rebuilds or reserializes the operating history.

use super::irradiation::{CondensationRule, IrradiationSchedule, irradiation_schedule};
use super::spatial_dose::{
    ArtifactRef, CURVES_SCHEMA, DoseCovarianceAssessment, DoseEstimate, DosePointPair,
    ESTIMATE_SCHEMA, EventDoseCurve, RUN_RECORD_SCHEMA, TaskLocation,
};
use super::{DoseEventCurveProvider, ReplacementEvent, irradiation};
use crate::history::HistoryResult;
use crate::jobs::Cancellation;
use faris_model::maintenance::{DoseMethod, PHOTON_EFFECTIVE_DOSE_RATE_QUANTITY};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const DOSE_PHYSICS_SCHEMA: &str = "faris-maintenance-dose-physics/v0.1";
pub const COMPONENT_MAP_SCHEMA: &str = "faris-cad-component-material-map/v0.1";
pub const REQUEST_SCHEMA: &str = "faris-maintenance-r2s-event-request/v0.1";
pub const RECEIPT_SCHEMA: &str = "faris-maintenance-r2s-event-receipt/v0.1";
const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
const MAX_WORKER_REQUEST_BYTES: usize = 4 * 1024 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MeshBounds {
    pub lower_left_cm: [f64; 3],
    pub upper_right_cm: [f64; 3],
    pub dimension: [u32; 3],
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DoseSampling {
    pub neutron_particles: u32,
    pub neutron_batches: u32,
    pub photon_particles: u32,
    pub photon_batches: u32,
    pub seed: u32,
    pub threads: u32,
    pub mat_vol_samples: u32,
    pub activation_groups: String,
    pub run_budget_s: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DosePhysicsManifest {
    pub schema_version: String,
    pub model_xml: ArtifactRef,
    pub geometry_h5m: ArtifactRef,
    pub component_material_map: ArtifactRef,
    pub cross_sections_xml: ArtifactRef,
    pub chain_xml: ArtifactRef,
    pub activation_mesh: MeshBounds,
    /// Artifact paths are keyed by the location IDs used by the maintenance assumptions.
    pub response_locations: BTreeMap<String, ArtifactRef>,
    pub sampling: DoseSampling,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DoseCadPrepareConfig {
    pub schema_version: String,
    pub design_file: PathBuf,
    pub import_report: PathBuf,
    pub cross_sections_xml: ArtifactRef,
    pub chain_xml: ArtifactRef,
    pub solid_history_components: BTreeMap<String, String>,
    pub activation_mesh: MeshBounds,
    pub response_locations: BTreeMap<String, ArtifactRef>,
    pub sampling: DoseSampling,
    pub work_dir: PathBuf,
    pub output_manifest: PathBuf,
}

/// Prepare a complete, checked CAD design for event-specific spatial dose calculations. This
/// creates per-solid material clones and the unit-strength neutron kernel in a cancellable worker.
pub fn prepare_dose_cad(
    config_path: &Path,
    python: &Path,
    worker_script: &Path,
    cancel: &Cancellation,
) -> Result<PathBuf, String> {
    const CONFIG_SCHEMA: &str = "faris-maintenance-dose-cad-config/v0.1";
    const PREP_RECEIPT_SCHEMA: &str = "faris-maintenance-dose-cad-preparation-receipt/v0.1";
    let bytes = read_bounded(config_path, MAX_MANIFEST_BYTES)?;
    let mut config: DoseCadPrepareConfig =
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if config.schema_version != CONFIG_SCHEMA {
        return Err(format!(
            "dose CAD config schema_version must be {CONFIG_SCHEMA}"
        ));
    }
    let base = config_path.parent().unwrap_or(Path::new("."));
    for path in [
        &mut config.design_file,
        &mut config.import_report,
        &mut config.work_dir,
        &mut config.output_manifest,
    ] {
        if !path.is_absolute() {
            *path = base.join(&*path);
        }
    }
    for reference in [&mut config.cross_sections_xml, &mut config.chain_xml] {
        if !reference.path.is_absolute() {
            reference.path = base.join(&reference.path);
        }
        verify_ref(reference, "dose preparation nuclear data")?;
    }
    for reference in config.response_locations.values_mut() {
        if !reference.path.is_absolute() {
            reference.path = base.join(&reference.path);
        }
        verify_ref(reference, "dose preparation response location")?;
    }
    if config.design_file == config.import_report || config.output_manifest.exists() {
        return Err(
            "design and ImportReport must be separate files; output manifest must be new".into(),
        );
    }
    if config.response_locations.is_empty() {
        return Err(
            "dose CAD preparation needs at least one authored task response location".into(),
        );
    }
    validate_bounds(&config.activation_mesh, "activation mesh")?;
    validate_sampling(&config.sampling)?;
    let worker_script = worker_script.canonicalize().map_err(|e| e.to_string())?;
    let helper = worker_script.with_file_name("design_transport.py");
    if !helper.is_file() {
        return Err("trusted design_transport.py helper is missing beside the worker".into());
    }
    let request = crate::design::transport::maintenance_dose_cad_request(
        &config.design_file,
        &config.import_report,
        &config.cross_sections_xml.path,
        &helper,
        config.solid_history_components.clone(),
    )
    .map_err(|e| e.to_string())?;
    let work_dir = &config.work_dir;
    std::fs::create_dir(work_dir)
        .map_err(|e| format!("dose preparation work directory must be new: {e}"))?;
    let request_path = work_dir.join("dose-cad-prepare-request.json");
    write_exact(
        &request_path,
        &serde_json::to_vec_pretty(&request).map_err(|e| e.to_string())?,
    )?;
    let out = work_dir.join("prepared");
    let log = work_dir.join("preparation.log");
    let status = launch_bounded(
        python,
        &worker_script,
        &request_path,
        &out,
        &log,
        config.sampling.run_budget_s,
        cancel,
    )?;
    if !status.success() {
        let details = std::fs::read_to_string(&log).unwrap_or_default();
        return Err(format!(
            "dose CAD preparation failed ({status}): {}",
            details.chars().take(3000).collect::<String>()
        ));
    }
    let receipt_path = out.join("dose-cad-preparation-receipt.json");
    let receipt: Value = serde_json::from_slice(&read_bounded(&receipt_path, MAX_MANIFEST_BYTES)?)
        .map_err(|e| e.to_string())?;
    if receipt.get("schema").and_then(Value::as_str) != Some(PREP_RECEIPT_SCHEMA) {
        return Err("dose CAD preparation receipt has an unsupported schema".into());
    }
    let expected_request_sha = sha256(&read_bounded(&request_path, MAX_WORKER_REQUEST_BYTES)?);
    if receipt.get("request_sha256").and_then(Value::as_str) != Some(expected_request_sha.as_str())
    {
        return Err(
            "dose CAD preparation receipt does not bind the exact Rust request bytes".into(),
        );
    }
    let model_xml = path_from_receipt(&receipt, "model_xml")?;
    let geometry_h5m = path_from_receipt(&receipt, "geometry_h5m")?;
    let component_map = path_from_receipt(&receipt, "component_material_map")?;
    let prepared_root = out.canonicalize().map_err(|e| e.to_string())?;
    for path in [&model_xml, &geometry_h5m, &component_map] {
        let canonical = path.canonicalize().map_err(|e| e.to_string())?;
        if !canonical.starts_with(&prepared_root) || !canonical.is_file() {
            return Err(
                "dose CAD preparation receipt names an output outside its isolated work directory"
                    .into(),
            );
        }
    }
    let model_ref = make_ref(&model_xml)?;
    let geometry_ref = make_ref(&geometry_h5m)?;
    let map_ref = make_ref(&component_map)?;
    for (key, reference) in [
        ("model_xml_sha256", &model_ref),
        ("derived_h5m_sha256", &geometry_ref),
        ("component_material_map_sha256", &map_ref),
    ] {
        if receipt.get(key).and_then(Value::as_str) != Some(reference.sha256.as_str()) {
            return Err(format!(
                "dose CAD preparation receipt {key} does not match its artifact"
            ));
        }
    }
    let manifest = serde_json::json!({
        "schema_version": DOSE_PHYSICS_SCHEMA,
        "model_xml": model_ref,
        "geometry_h5m": geometry_ref,
        "component_material_map": map_ref,
        "cross_sections_xml": config.cross_sections_xml,
        "chain_xml": config.chain_xml,
        "activation_mesh": config.activation_mesh,
        "response_locations": config.response_locations,
        "sampling": config.sampling,
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
    write_exact(&config.output_manifest, &manifest_bytes)?;
    // Re-open through the exact validator used by DoseRun before declaring preparation complete.
    let _ = DosePhysicsManifest::read(&config.output_manifest)?;
    Ok(config.output_manifest)
}

fn path_from_receipt(receipt: &Value, key: &str) -> Result<PathBuf, String> {
    receipt
        .get(key)
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| format!("dose CAD preparation receipt omits {key}"))
}

impl DosePhysicsManifest {
    pub fn read(path: &Path) -> Result<(Self, BTreeMap<String, ArtifactRef>, String), String> {
        let bytes = read_bounded(path, MAX_MANIFEST_BYTES)?;
        let hash = sha256(&bytes);
        let mut manifest: Self = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if manifest.schema_version != DOSE_PHYSICS_SCHEMA {
            return Err(format!(
                "dose physics manifest schema_version must be {DOSE_PHYSICS_SCHEMA}"
            ));
        }
        let base = path.parent().unwrap_or(Path::new("."));
        let refs = [
            ("model_xml", &mut manifest.model_xml),
            ("geometry_h5m", &mut manifest.geometry_h5m),
            (
                "component_material_map",
                &mut manifest.component_material_map,
            ),
            ("cross_sections_xml", &mut manifest.cross_sections_xml),
            ("chain_xml", &mut manifest.chain_xml),
        ];
        let mut verified = BTreeMap::new();
        for (name, reference) in refs {
            if !reference.path.is_absolute() {
                reference.path = base.join(&reference.path);
            }
            verify_ref(reference, name)?;
            verified.insert(name.to_string(), reference.clone());
        }
        for (location_id, reference) in &mut manifest.response_locations {
            if location_id.trim().is_empty() {
                return Err("response location IDs must be nonempty".into());
            }
            if !reference.path.is_absolute() {
                reference.path = base.join(&reference.path);
            }
            verify_ref(reference, "response location")?;
            verified.insert(format!("location/{location_id}"), reference.clone());
        }
        validate_bounds(&manifest.activation_mesh, "activation mesh")?;
        validate_sampling(&manifest.sampling)?;
        let map_bytes = read_bounded(&manifest.component_material_map.path, MAX_MANIFEST_BYTES)?;
        let map: ComponentMaterialMap =
            serde_json::from_slice(&map_bytes).map_err(|e| e.to_string())?;
        if map.schema != COMPONENT_MAP_SCHEMA || map.materials.is_empty() {
            return Err(format!(
                "component material map schema must be {COMPONENT_MAP_SCHEMA} and name CAD components"
            ));
        }
        if !map
            .geometry_sha256
            .eq_ignore_ascii_case(&manifest.geometry_h5m.sha256)
        {
            return Err(
                "CAD component material map is not bound to the selected H5M geometry".into(),
            );
        }
        let mut material_ids = std::collections::BTreeSet::new();
        for (component, ids) in &map.materials {
            if component.trim().is_empty() || ids.is_empty() {
                return Err(
                    "every CAD component needs one or more unique OpenMC material IDs".into(),
                );
            }
            for id in ids {
                if *id == 0 || !material_ids.insert(*id) {
                    return Err(
                        "CAD component material IDs must be positive and unique across solids"
                            .into(),
                    );
                }
            }
        }
        Ok((manifest, verified, hash))
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ComponentMaterialMap {
    schema: String,
    geometry_sha256: String,
    materials: BTreeMap<String, Vec<u32>>,
}

#[derive(Clone)]
struct DesignContext {
    manifest: DosePhysicsManifest,
    manifest_artifact: ArtifactRef,
    manifest_sha256: String,
    neutron_rate_n_s: f64,
}

/// Provider state for one bounded invocation. Each generated run is placed in an isolated,
/// content-addressed work directory and kept as provenance evidence.
pub struct OpenMcDoseProvider {
    pub python: PathBuf,
    pub worker_script: PathBuf,
    pub work_dir: PathBuf,
    contexts: BTreeMap<String, DesignContext>,
}

impl OpenMcDoseProvider {
    pub fn new(
        python: PathBuf,
        worker_script: PathBuf,
        work_dir: PathBuf,
        contexts: BTreeMap<String, (PathBuf, f64)>,
    ) -> Result<Self, String> {
        let mut parsed = BTreeMap::new();
        for (design, (path, neutron_rate_n_s)) in contexts {
            if !neutron_rate_n_s.is_finite() || neutron_rate_n_s <= 0.0 {
                return Err(format!(
                    "design {design}: verified neutron source rate must be finite and positive"
                ));
            }
            let (manifest, _references, manifest_sha256) = DosePhysicsManifest::read(&path)
                .map_err(|e| format!("design {design} dose physics: {e}"))?;
            let manifest_artifact = ArtifactRef {
                path,
                sha256: manifest_sha256.clone(),
            };
            parsed.insert(
                design,
                DesignContext {
                    manifest,
                    manifest_artifact,
                    manifest_sha256,
                    neutron_rate_n_s,
                },
            );
        }
        Ok(Self {
            python,
            worker_script,
            work_dir,
            contexts: parsed,
        })
    }
}

impl DoseEventCurveProvider for OpenMcDoseProvider {
    fn produce(
        &mut self,
        design: &str,
        design_input_sha256: &BTreeMap<String, String>,
        exact_history_bytes: &[u8],
        history_sha256: &str,
        event: &ReplacementEvent,
        install_s: f64,
        location: &TaskLocation,
        _method: DoseMethod,
        cooling_grid_s: &[f64],
        cancel: &Cancellation,
    ) -> Result<Option<EventDoseCurve>, String> {
        let Some(context) = self.contexts.get(design) else {
            return Ok(None);
        };
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }
        let location_ref = context
            .manifest
            .response_locations
            .get(&location.id)
            .ok_or_else(|| {
                format!(
                    "design {design}: no physical response definition for task location {}",
                    location.id
                )
            })?;
        if !location_ref.sha256.eq_ignore_ascii_case(&location.sha256) {
            return Err(format!(
                "design {design}: response location {} hash differs from maintenance assumptions",
                location.id
            ));
        }
        let location_bytes = read_bounded(&location_ref.path, 64 * 1024)?;
        let location_value: Value =
            serde_json::from_slice(&location_bytes).map_err(|e| format!("task location: {e}"))?;
        let lower = value_vec3(&location_value, "lower_left_cm")?;
        let upper = value_vec3(&location_value, "upper_right_cm")?;
        let dimension: [u32; 3] = [1, 1, 1];
        if location_value.get("id").and_then(Value::as_str) != Some(location.id.as_str()) {
            return Err(
                "hashed task-location artifact ID differs from authored task location".into(),
            );
        }
        let response_volume: f64 = (0..3).map(|i| upper[i] - lower[i]).product();
        if !response_volume.is_finite() || response_volume <= 0.0 {
            return Err("task-location response volume must be finite and positive".into());
        }

        let history: HistoryResult = serde_json::from_slice(exact_history_bytes)
            .map_err(|e| format!("internal exact history bytes are not parseable: {e}"))?;
        if !sha256(exact_history_bytes).eq_ignore_ascii_case(history_sha256)
            || irradiation::history_sha256(&history)? != history_sha256
        {
            return Err("exact Rust history bytes and the history SHA-256 do not reconcile".into());
        }
        let rule = CondensationRule::LOSSLESS;
        let target_schedule = irradiation_schedule(
            &history,
            Some((&event.component, event.start_s)),
            &rule,
            &[event.start_s],
        )?;
        if (target_schedule.window_start_s - install_s).abs() > 1.0 {
            return Err(
                "replacement event installation time differs from its Rust irradiation schedule"
                    .into(),
            );
        }
        let mut schedules: BTreeMap<String, IrradiationSchedule> = BTreeMap::new();
        let material_map_bytes = read_bounded(
            &context.manifest.component_material_map.path,
            MAX_MANIFEST_BYTES,
        )?;
        let component_map: ComponentMaterialMap =
            serde_json::from_slice(&material_map_bytes).map_err(|e| e.to_string())?;
        let mut source_groups = Vec::new();
        for component in component_map.materials.keys() {
            let installations = super::actinv::installations(&history, component)?;
            let Some(index) = super::actinv::installation_at(&history, component, event.start_s)?
            else {
                continue;
            };
            let component_install_s = installations
                .get(index as usize - 1)
                .map(|x| x.0)
                .ok_or_else(|| {
                    format!("{component}: current installation is absent from event history")
                })?;
            let schedule = irradiation_schedule(
                &history,
                Some((component, event.start_s)),
                &rule,
                &[event.start_s],
            )?;
            let mut steps: Vec<Value> = schedule
                .steps
                .iter()
                .map(|step| {
                    json!({
                        "duration_s": step.duration_s,
                        "source_rate_n_s": context.neutron_rate_n_s * step.flux_scale,
                    })
                })
                .collect();
            let mut previous = 0.0;
            for cooling in cooling_grid_s {
                let duration = cooling - previous;
                if !duration.is_finite() || duration <= 0.0 {
                    return Err("cooling grid must be strictly ascending".into());
                }
                steps.push(json!({"duration_s": duration, "source_rate_n_s": 0.0}));
                previous = *cooling;
            }
            source_groups.push(json!({
                "component": component,
                "history_sha256": history_sha256,
                "installation_s": component_install_s,
                "schedule": steps,
            }));
            schedules.insert(component.clone(), schedule);
        }
        if !schedules.contains_key(&event.component) {
            return Err(format!(
                "event component {} is not installed at its replacement shutdown",
                event.component
            ));
        }
        if source_groups.is_empty() {
            return Err(
                "no currently installed CAD component has an irradiation history at this shutdown"
                    .into(),
            );
        }
        let schedule_bundle = json!({"schema": "faris-r2s-installation-schedules/v0.1", "history_sha256": history_sha256,
                                     "shutdown_s": event.start_s, "schedules": schedules});
        let schedule_bytes =
            serde_json::to_vec_pretty(&schedule_bundle).map_err(|e| e.to_string())?;

        let mut curve = EventDoseCurve {
            design: design.into(),
            design_input_sha256: design_input_sha256.clone(),
            history_sha256: history_sha256.into(),
            class: event.class.clone(),
            component: event.component.clone(),
            replacement_k: event.k,
            shutdown_s: event.start_s,
            install_s,
            irradiation_rule: rule,
            irradiation_schedule_sha256: target_schedule.input_sha256.clone(),
            location: location.clone(),
            points: Vec::new(),
        };
        let event_sha256 = super::spatial_dose::event_identity_sha256(&curve)?;
        let run_id = sha256(
            format!(
                "{design}\0{history_sha256}\0{}\0{}\0{}",
                event.component, event.k, event.start_s
            )
            .as_bytes(),
        );
        let run_dir = self.work_dir.join(&run_id);
        std::fs::create_dir_all(&run_dir).map_err(|e| e.to_string())?;
        let history_path = run_dir.join("history-rust-bytes.json");
        write_exact(&history_path, exact_history_bytes)?;
        let schedule_path = run_dir.join("installation-schedules.json");
        write_exact(&schedule_path, &schedule_bytes)?;
        let irradiation_path = run_dir.join("target-irradiation-schedule.json");
        write_exact(
            &irradiation_path,
            &serde_json::to_vec_pretty(&target_schedule).map_err(|e| e.to_string())?,
        )?;
        let history_ref = make_ref(&history_path)?;
        let schedule_ref = make_ref(&irradiation_path)?;
        let group_schedules_ref = make_ref(&schedule_path)?;
        let response_mesh = json!({"artifact": location_ref, "lower_left_cm": lower, "upper_right_cm": upper, "dimension": dimension});
        let sampling =
            serde_json::to_value(&context.manifest.sampling).map_err(|e| e.to_string())?;
        let request = json!({
            "schema": REQUEST_SCHEMA,
            "request_id": run_id,
            "history_sha256": history_sha256,
            "history_artifact": history_ref,
            "event": {
                "design": design, "class": event.class, "component": event.component, "replacement_k": event.k,
                "shutdown_s": event.start_s, "location_id": location.id, "location_sha256": location.sha256,
                "cooling_s": cooling_grid_s, "source_groups": source_groups,
            },
            "model": {"model_xml": context.manifest.model_xml.path, "sha256": context.manifest.model_xml.sha256},
            "geometry": context.manifest.geometry_h5m,
            "cross_sections": context.manifest.cross_sections_xml,
            "chain": context.manifest.chain_xml,
            "component_material_map": context.manifest.component_material_map,
            "activation_mesh": context.manifest.activation_mesh,
            "response_mesh": response_mesh,
            "sampling": sampling,
        });
        let request_bytes = serde_json::to_vec_pretty(&request).map_err(|e| e.to_string())?;
        if request_bytes.len() > MAX_WORKER_REQUEST_BYTES {
            return Err("R2S worker request exceeds 4 MiB".into());
        }
        let request_path = run_dir.join("r2s-event-request.json");
        write_exact(&request_path, &request_bytes)?;
        let request_ref = make_ref(&request_path)?;
        let log_path = run_dir.join("worker.log");
        let status = launch_bounded(
            &self.python,
            &self.worker_script,
            &request_path,
            &run_dir,
            &log_path,
            context.manifest.sampling.run_budget_s,
            cancel,
        )?;
        if !status.success() {
            let log = std::fs::read_to_string(&log_path).unwrap_or_default();
            return Err(format!(
                "OpenMC event R2S worker failed ({status}); see {}: {}",
                log_path.display(),
                log.chars().take(4000).collect::<String>()
            ));
        }
        let receipt_path = run_dir.join("r2s_event_receipt.json");
        let receipt_bytes = read_bounded(&receipt_path, MAX_MANIFEST_BYTES)?;
        let receipt: Value = serde_json::from_slice(&receipt_bytes).map_err(|e| e.to_string())?;
        if receipt.get("schema").and_then(Value::as_str) != Some(RECEIPT_SCHEMA)
            || receipt.get("status").and_then(Value::as_str) != Some("PASS")
            || receipt.get("request_sha256").and_then(Value::as_str)
                != Some(request_ref.sha256.as_str())
        {
            return Err(
                "OpenMC R2S receipt schema, status, or exact request hash does not match".into(),
            );
        }
        let run_record_ref = make_ref(&receipt_path)?;
        let input_refs = BTreeMap::from([
            (
                "geometry".to_string(),
                context.manifest.geometry_h5m.clone(),
            ),
            (
                "materials".to_string(),
                context.manifest.component_material_map.clone(),
            ),
            ("source".to_string(), context.manifest.model_xml.clone()),
            ("irradiation".to_string(), schedule_ref.clone()),
            ("response".to_string(), location_ref.clone()),
            ("location".to_string(), location_ref.clone()),
        ]);
        let mut input_sha: BTreeMap<String, String> = input_refs
            .iter()
            .map(|(k, v)| (k.clone(), v.sha256.clone()))
            .collect();
        input_sha.insert("event".into(), event_sha256.clone());
        let normalization = "Physical source rate in neutrons/s is applied to each exact Rust irradiation interval; OpenMC R2S MeshSource photon strengths are applied by the fixed-source transport; response is volume-averaged over the authored one-voxel task location.".to_string();
        let receipt_points = receipt
            .get("points")
            .and_then(Value::as_array)
            .ok_or("R2S receipt has no points")?;
        let d1s = receipt
            .get("d1s")
            .ok_or("R2S receipt omits common-history D1S status")?;
        let d1s_status = d1s
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("NOT_EVALUATED");
        let d1s_reason = d1s.get("reason").and_then(Value::as_str)
            .unwrap_or("D1S is not applicable to heterogeneous component installation histories; stock factors are nuclide-global.")
            .to_string();
        if receipt_points.len() != cooling_grid_s.len() {
            return Err("R2S receipt point count differs from the authored cooling grid".into());
        }
        let d1s_points = d1s.get("points").and_then(Value::as_array);
        if d1s_status == "PASS" && d1s_points.is_none_or(|p| p.len() != cooling_grid_s.len()) {
            return Err(
                "common-history D1S receipt point count differs from the authored cooling grid"
                    .into(),
            );
        }
        for (point_index, (cooling_s, r2s_point)) in
            cooling_grid_s.iter().zip(receipt_points).enumerate()
        {
            let r2s_value = value_f64(r2s_point, "mean_sv_h")?;
            let r2s_se = value_f64(r2s_point, "standard_error_sv_h")?;
            let mut r2s_hashes = input_sha.clone();
            r2s_hashes.insert(
                "cross_sections".into(),
                context.manifest.cross_sections_xml.sha256.clone(),
            );
            r2s_hashes.insert("chain".into(), context.manifest.chain_xml.sha256.clone());
            r2s_hashes.insert("history".into(), history_sha256.into());
            r2s_hashes.insert("request".into(), request_ref.sha256.clone());
            r2s_hashes.insert(
                "source_schedules".into(),
                group_schedules_ref.sha256.clone(),
            );
            r2s_hashes.insert(
                "dose_physics_manifest".into(),
                context.manifest_sha256.clone(),
            );
            let r2s_stream = receipt
                .get("runtime")
                .and_then(|r| r.get("openmc_python_version"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let r2s_seed = context.manifest.sampling.seed;
            let r2s_random = format!("OpenMC-R2S:{r2s_stream}:seed-{r2s_seed}");
            let mut r2s_artifact_hashes = artifact_hashes(&input_refs);
            r2s_artifact_hashes.extend([
                (
                    "cross_sections".into(),
                    context.manifest.cross_sections_xml.clone(),
                ),
                ("chain".into(), context.manifest.chain_xml.clone()),
                ("request".into(), request_ref.clone()),
                ("source_schedules".into(), group_schedules_ref.clone()),
                ("history".into(), history_ref.clone()),
                (
                    "dose_physics_manifest".into(),
                    context.manifest_artifact.clone(),
                ),
            ]);
            let r2s_run_record = write_run_record(
                &run_dir,
                &curve,
                &event_sha256,
                &input_sha,
                &r2s_hashes,
                &r2s_artifact_hashes,
                "r2s",
                *cooling_s,
                r2s_value,
                r2s_se,
                &normalization,
                &r2s_random,
                &run_record_ref,
                &request_ref,
                &schedule_ref,
            )?;
            let r2s_estimate = make_estimate(
                DoseMethod::R2s,
                *cooling_s,
                r2s_value,
                r2s_se,
                location,
                &normalization,
                r2s_random,
                r2s_hashes,
                r2s_artifact_hashes,
                r2s_run_record,
            )?;
            let d1s_estimate = if d1s_status == "PASS" {
                let d1s_point = &d1s_points.expect("checked D1S points")[point_index];
                let value = value_f64(d1s_point, "mean_sv_h")?;
                let se = value_f64(d1s_point, "standard_error_sv_h")?;
                let mut hashes = input_sha.clone();
                hashes.insert(
                    "cross_sections".into(),
                    context.manifest.cross_sections_xml.sha256.clone(),
                );
                hashes.insert("chain".into(), context.manifest.chain_xml.sha256.clone());
                hashes.insert("history".into(), history_sha256.into());
                hashes.insert("request".into(), request_ref.sha256.clone());
                hashes.insert(
                    "source_schedules".into(),
                    group_schedules_ref.sha256.clone(),
                );
                hashes.insert(
                    "dose_physics_manifest".into(),
                    context.manifest_sha256.clone(),
                );
                let version = d1s
                    .get("openmc_c_library_sha256")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let random = format!(
                    "OpenMC-D1S:{version}:seed-{}",
                    context.manifest.sampling.seed.saturating_add(100)
                );
                let mut artifact_refs = artifact_hashes(&input_refs);
                artifact_refs.extend([
                    (
                        "cross_sections".into(),
                        context.manifest.cross_sections_xml.clone(),
                    ),
                    ("chain".into(), context.manifest.chain_xml.clone()),
                    ("request".into(), request_ref.clone()),
                    ("source_schedules".into(), group_schedules_ref.clone()),
                    ("history".into(), history_ref.clone()),
                    (
                        "dose_physics_manifest".into(),
                        context.manifest_artifact.clone(),
                    ),
                ]);
                let run_ref = write_run_record(
                    &run_dir,
                    &curve,
                    &event_sha256,
                    &input_sha,
                    &hashes,
                    &artifact_refs,
                    "d1s",
                    *cooling_s,
                    value,
                    se,
                    &normalization,
                    &random,
                    &run_record_ref,
                    &request_ref,
                    &schedule_ref,
                )?;
                Some(make_estimate(
                    DoseMethod::D1s,
                    *cooling_s,
                    value,
                    se,
                    location,
                    &normalization,
                    random,
                    hashes,
                    artifact_refs,
                    run_ref,
                )?)
            } else {
                None
            };
            curve.points.push(DosePointPair {
                d1s: d1s_estimate,
                r2s: r2s_estimate?,
                d1s_not_evaluated: (d1s_status != "PASS").then(|| d1s_reason.clone()),
                covariance: DoseCovarianceAssessment::Unknown,
            });
            let _ = point_index;
        }
        // The event key is checked against the exact Rust history and task-location identities.
        if curve.history_sha256 != history_sha256
            || curve.irradiation_schedule_sha256 != target_schedule.input_sha256
        {
            return Err(
                "generated event curve no longer matches exact history/schedule identity".into(),
            );
        }
        let _ = request_ref;
        Ok(Some(curve))
    }
}

fn write_run_record(
    run_dir: &Path,
    curve: &EventDoseCurve,
    event_sha256: &str,
    _core_input_sha: &BTreeMap<String, String>,
    hashes: &BTreeMap<String, String>,
    _artifacts: &BTreeMap<String, ArtifactRef>,
    method: &str,
    cooling_s: f64,
    value: f64,
    standard_error: f64,
    normalization: &str,
    random_stream: &str,
    worker_receipt: &ArtifactRef,
    request: &ArtifactRef,
    schedule: &ArtifactRef,
) -> Result<ArtifactRef, String> {
    let record = json!({
        "schema": RUN_RECORD_SCHEMA, "method": method, "cooling_time_s": cooling_s,
        "estimate": {"value": value, "standard_error": standard_error},
        "response": {"quantity": PHOTON_EFFECTIVE_DOSE_RATE_QUANTITY, "unit": "Sv/h", "normalization": normalization},
        "artifacts": {
            "geometry_sha256": hashes["geometry"], "materials_sha256": hashes["materials"],
            "source_sha256": hashes["source"], "irradiation_sha256": hashes["irradiation"],
            "response_sha256": hashes["response"], "location_sha256": hashes["location"],
            "worker_receipt_sha256": worker_receipt.sha256, "request_sha256": request.sha256,
            "schedule_sha256": schedule.sha256,
        },
        "random_streams": [random_stream],
        "event": {
            "identity_sha256": event_sha256, "history_sha256": curve.history_sha256,
            "design": curve.design, "design_input_sha256": curve.design_input_sha256,
            "class": curve.class, "component": curve.component, "replacement_k": curve.replacement_k,
            "shutdown_s": curve.shutdown_s, "install_s": curve.install_s,
            "irradiation_schedule_sha256": curve.irradiation_schedule_sha256,
            "location": curve.location,
        },
    });
    let path = run_dir.join(format!("run-record-{method}-{:.0}.json", cooling_s));
    write_exact(
        &path,
        &serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?,
    )?;
    make_ref(&path)
}

#[allow(clippy::too_many_arguments)]
fn make_estimate(
    method: DoseMethod,
    cooling_s: f64,
    value: f64,
    standard_error: f64,
    location: &TaskLocation,
    normalization: &str,
    random_stream: String,
    hashes: BTreeMap<String, String>,
    artifacts: BTreeMap<String, ArtifactRef>,
    run_record: ArtifactRef,
) -> Result<DoseEstimate, String> {
    if !value.is_finite() || value < 0.0 || !standard_error.is_finite() || standard_error < 0.0 {
        return Err(
            "OpenMC worker returned a non-finite or negative dose or standard error".into(),
        );
    }
    Ok(DoseEstimate {
        schema: ESTIMATE_SCHEMA.into(),
        method,
        quantity: PHOTON_EFFECTIVE_DOSE_RATE_QUANTITY.into(),
        unit: "Sv/h".into(),
        normalization: normalization.into(),
        location: location.clone(),
        cooling_s,
        value_sv_h: value,
        standard_error_sv_h: standard_error,
        random_streams: vec![random_stream],
        physical_input_sha256: hashes,
        physical_input_artifacts: artifacts,
        run_record,
    })
}

fn artifact_hashes(refs: &BTreeMap<String, ArtifactRef>) -> BTreeMap<String, ArtifactRef> {
    refs.clone()
}

fn value_vec3(value: &Value, key: &str) -> Result<[f64; 3], String> {
    let values = value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("task location missing {key}"))?;
    if values.len() != 3 {
        return Err(format!("task location {key} must have three values"));
    }
    let mut out = [0.0; 3];
    for (i, value) in values.iter().enumerate() {
        out[i] = value
            .as_f64()
            .ok_or_else(|| format!("task location {key} values must be numeric"))?;
        if !out[i].is_finite() {
            return Err(format!("task location {key} values must be finite"));
        }
    }
    Ok(out)
}

fn validate_bounds(bounds: &MeshBounds, label: &str) -> Result<(), String> {
    if bounds.dimension.iter().any(|d| *d == 0)
        || bounds
            .dimension
            .iter()
            .fold(1_u64, |a, b| a.saturating_mul(u64::from(*b)))
            > 5_000_000
        || (0..3).any(|i| {
            !bounds.lower_left_cm[i].is_finite()
                || !bounds.upper_right_cm[i].is_finite()
                || bounds.lower_left_cm[i] >= bounds.upper_right_cm[i]
        })
    {
        return Err(format!(
            "{label} bounds/dimensions are invalid or exceed the worker cap"
        ));
    }
    Ok(())
}

fn validate_sampling(s: &DoseSampling) -> Result<(), String> {
    for (value, maximum, label) in [
        (s.neutron_particles, 1_000_000, "neutron particles"),
        (s.neutron_batches, 100_000, "neutron batches"),
        (s.photon_particles, 1_000_000, "photon particles"),
        (s.photon_batches, 100_000, "photon batches"),
        (s.threads, 64, "threads"),
        (s.mat_vol_samples, 10_000_000, "material-volume samples"),
    ] {
        if value == 0 || value > maximum {
            return Err(format!("{label} is outside the bounded worker range"));
        }
    }
    if s.seed == 0
        || !matches!(s.activation_groups.as_str(), "VITAMIN-J-42" | "CCFE-709")
        || !s.run_budget_s.is_finite()
        || !(1.0..=86_400.0).contains(&s.run_budget_s)
    {
        return Err("dose sampling needs a supported energy structure, positive seed and 1–86400 s run budget".into());
    }
    Ok(())
}

fn verify_ref(reference: &ArtifactRef, label: &str) -> Result<(), String> {
    if reference.sha256.len() != 64 || !reference.sha256.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("{label} artifact needs a valid SHA-256"));
    }
    let observed = sha256_file(&reference.path)?;
    if !observed.eq_ignore_ascii_case(&reference.sha256) {
        return Err(format!("{label} artifact SHA-256 mismatch"));
    }
    Ok(())
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err(format!(
            "{} exceeds the {limit} byte input limit",
            path.display()
        ));
    }
    let mut out = Vec::new();
    file.take((limit + 1) as u64)
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    if out.len() > limit {
        return Err(format!(
            "{} exceeds the {limit} byte input limit",
            path.display()
        ));
    }
    Ok(out)
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn sha256_file(path: &Path) -> Result<String, String> {
    Ok(sha256(&read_bounded(path, 256 * 1024 * 1024)?))
}
fn make_ref(path: &Path) -> Result<ArtifactRef, String> {
    Ok(ArtifactRef {
        path: path.to_path_buf(),
        sha256: sha256_file(path)?,
    })
}

fn write_exact(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    if path.exists() {
        let existing = read_bounded(path, bytes.len().max(1))?;
        return if existing == bytes {
            Ok(())
        } else {
            Err(format!(
                "refusing to replace changed worker artifact {}",
                path.display()
            ))
        };
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}

fn launch_bounded(
    python: &Path,
    worker: &Path,
    request: &Path,
    out: &Path,
    log: &Path,
    budget_s: f64,
    cancel: &Cancellation,
) -> Result<std::process::ExitStatus, String> {
    let log_file = std::fs::File::create(log).map_err(|e| e.to_string())?;
    let stderr = log_file.try_clone().map_err(|e| e.to_string())?;
    let python_bin = python.parent();
    let mut command = Command::new(python);
    command
        .arg(worker)
        .arg("--request")
        .arg(request)
        .arg("--out")
        .arg(out)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(stderr));
    if let Some(bin) = python_bin {
        let mut paths = vec![bin.to_path_buf()];
        if let Some(current) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&current));
        }
        let joined = std::env::join_paths(paths).map_err(|e| e.to_string())?;
        command.env("PATH", joined);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("could not start OpenMC worker {}: {e}", python.display()))?;
    let started = Instant::now();
    loop {
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("cancelled".into());
        }
        if started.elapsed().as_secs_f64() > budget_s {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "OpenMC worker exceeded its {:.0} second run budget",
                budget_s
            ));
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Ok(status);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}
