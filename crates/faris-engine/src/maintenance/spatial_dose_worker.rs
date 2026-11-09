// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! Bounded OpenMC R2S curve production for exact operating-history replacement events.
//!
//! This adapter only accepts an explicit, hash-verified CAD/OpenMC context. It carries the exact
//! Rust history bytes and per-installation schedules into the Python worker; Python never
//! rebuilds or reserializes the operating history.

use super::irradiation::{CondensationRule, IrradiationSchedule, irradiation_schedule};
use super::spatial_dose::{
    ArtifactRef, DoseCovarianceAssessment, DoseEstimate, DosePointPair, ESTIMATE_SCHEMA,
    EventDoseCurve, RUN_RECORD_SCHEMA, TaskLocation,
};
use super::{DoseEventCurveProvider, ReplacementEvent, irradiation};
use crate::history::HistoryResult;
use crate::jobs::{Cancellation, ExecutionStatus, JobResult, JobSpec, ResourceLimits, run_job};
use faris_model::maintenance::{DoseMethod, PHOTON_EFFECTIVE_DOSE_RATE_QUANTITY};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const DOSE_PHYSICS_SCHEMA: &str = "faris-maintenance-dose-physics/v0.2";
pub const DOSE_PHYSICS_SCHEMA_V1: &str = "faris-maintenance-dose-physics/v0.1";
pub const COMPONENT_MAP_SCHEMA: &str = "faris-cad-component-material-map/v0.1";
pub const REQUEST_SCHEMA: &str = "faris-maintenance-r2s-event-request/v0.1";
pub const RECEIPT_SCHEMA: &str = "faris-maintenance-r2s-event-receipt/v0.1";
pub const PREP_RECEIPT_SCHEMA: &str = "faris-maintenance-dose-cad-preparation-receipt/v0.1";
const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
const MAX_WORKER_REQUEST_BYTES: usize = 4 * 1024 * 1024;
const MAX_ACTINV_BATCH_BYTES: usize = 256 * 1024 * 1024;
const MAX_ACTINV_ZONE_COUNT: usize = 5_000;
const MAX_ACTINV_EMITTER_RECORDS: usize = 50_000;
const MAX_ACTINV_EMITTER_GROUPS: usize = 2_000_000;
const WORKER_LOG_LIMIT_BYTES: usize = 1 << 20;

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
    #[serde(default)]
    pub chain_xml: Option<ArtifactRef>,
    #[serde(default)]
    pub activation_backend: Option<DoseActinvBackend>,
    pub activation_mesh: MeshBounds,
    /// Artifact paths are keyed by the location IDs used by the maintenance assumptions.
    pub response_locations: BTreeMap<String, ArtifactRef>,
    pub sampling: DoseSampling,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DoseActinvBackend {
    pub kind: String,
    pub executable: ArtifactRef,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub library_npz: ArtifactRef,
    pub library_index: ArtifactRef,
    pub decay_primary: ArtifactRef,
    pub decay_fallback: Option<ArtifactRef>,
    pub photon_group_boundaries_ev: Vec<f64>,
    pub timeout_s: f64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DoseActinvPrepareConfig {
    pub executable: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub photon_groups: usize,
    pub timeout_s: f64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DoseCadPrepareConfig {
    pub schema_version: String,
    pub design_file: PathBuf,
    pub import_report: PathBuf,
    pub cross_sections_xml: ArtifactRef,
    #[serde(default)]
    pub chain_xml: Option<ArtifactRef>,
    #[serde(default)]
    pub actinv: Option<DoseActinvPrepareConfig>,
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
    const CONFIG_SCHEMA: &str = "faris-maintenance-dose-cad-config/v0.2";
    let bytes = read_bounded(config_path, MAX_MANIFEST_BYTES)?;
    let mut config: DoseCadPrepareConfig =
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if config.schema_version != CONFIG_SCHEMA {
        return Err(format!(
            "dose CAD config schema_version must be {CONFIG_SCHEMA}"
        ));
    }
    let actinv_prepare = config.actinv.as_mut().ok_or(
        "dose CAD preparation config must include explicit ACTINV executable, data, cache, and photon grouping settings",
    )?;
    let base = config_path.parent().unwrap_or(Path::new("."));
    for path in [
        &mut config.design_file,
        &mut config.import_report,
        &mut config.work_dir,
        &mut config.output_manifest,
        &mut actinv_prepare.executable,
        &mut actinv_prepare.data_dir,
        &mut actinv_prepare.cache_dir,
    ] {
        if !path.is_absolute() {
            *path = base.join(&*path);
        }
    }
    {
        let reference = &mut config.cross_sections_xml;
        if !reference.path.is_absolute() {
            reference.path = base.join(&reference.path);
        }
        verify_ref(reference, "dose preparation nuclear data")?;
    }
    if let Some(reference) = &mut config.chain_xml {
        if !reference.path.is_absolute() {
            reference.path = base.join(&reference.path);
        }
        verify_ref(reference, "dose preparation depletion chain")?;
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
    if !actinv_prepare.timeout_s.is_finite()
        || !(1.0..=86_400.0).contains(&actinv_prepare.timeout_s)
    {
        return Err("ACTINV timeout_s must be finite and in 1..=86400".into());
    }
    let actinv_executable = actinv_prepare
        .executable
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let data_dir = actinv_prepare
        .data_dir
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !data_dir.is_dir() {
        return Err("ACTINV data_dir must be an existing directory".into());
    }
    let cache_dir = if actinv_prepare.cache_dir.is_absolute() {
        actinv_prepare.cache_dir.clone()
    } else {
        base.join(&actinv_prepare.cache_dir)
    };
    let cache_dir = cache_dir.canonicalize().unwrap_or(cache_dir);
    let activation_data = crate::activation::ActivationData::catalog(&data_dir);
    let library_npz = make_ref(&activation_data.library)?;
    let library_index_path = activation_data
        .library
        .with_file_name("tendl-2025-neutron-709g_index.json");
    let library_index = make_ref(&library_index_path)?;
    let decay_primary = make_ref(&activation_data.decay_primary)?;
    let decay_fallback = activation_data
        .decay_fallback
        .as_ref()
        .map(|(path, _)| make_ref(path))
        .transpose()?;
    let activation_backend = DoseActinvBackend {
        kind: "actinv_tendl_709".into(),
        executable: make_ref(&actinv_executable)?,
        data_dir,
        cache_dir,
        library_npz,
        library_index,
        decay_primary,
        decay_fallback,
        photon_group_boundaries_ev: crate::activation::log_photon_groups(
            actinv_prepare.photon_groups,
        )?,
        timeout_s: actinv_prepare.timeout_s,
    };
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
    let result = launch_bounded(
        python,
        &worker_script,
        &request_path,
        &out,
        &config.work_dir,
        &log,
        config.sampling.run_budget_s,
        cancel,
    )?;
    if result.execution_status != ExecutionStatus::Succeeded {
        if result.execution_status == ExecutionStatus::Cancelled {
            return Err("cancelled".into());
        }
        if result.execution_status == ExecutionStatus::TimedOut {
            return Err(format!(
                "OpenMC worker exceeded its {:.0} second run budget",
                config.sampling.run_budget_s
            ));
        }
        return Err(format!(
            "dose CAD preparation failed ({:?}, exit {:?}): {}",
            result.execution_status,
            result.exit_code,
            tail_job_output(&result)
        ));
    }
    let receipt_path = out.join("dose-cad-preparation-receipt.json");
    let expected_request_sha = sha256(&read_bounded(&request_path, MAX_WORKER_REQUEST_BYTES)?);
    let receipt = parse_preparation_receipt(
        &read_bounded(&receipt_path, MAX_MANIFEST_BYTES)?,
        &expected_request_sha,
    )?;
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
        "activation_backend": activation_backend,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActinvEventBatchFile {
    schema: String,
    uncertainty_scope: String,
    spatial_approximation: String,
    source_rate_normalization: String,
    provenance: Value,
    batches: Vec<ActinvEventBatch>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActinvEventBatch {
    component: String,
    zone_metadata: BTreeMap<String, ActinvZoneMetadata>,
    activation_request: crate::activation::ActivationRequest,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActinvZoneMetadata {
    mesh_element: u32,
    material_id: u32,
    material_volume_cm3: f64,
}

struct ActinvEventOutputs {
    activation_results: Vec<Value>,
    emitters_by_cooling: Vec<Value>,
    receipt_ref: ArtifactRef,
    source_artifact_refs: BTreeMap<String, ArtifactRef>,
    total_photons_by_cooling: Vec<f64>,
}

fn run_actinv_event_batches(
    manifest: &DosePhysicsManifest,
    batch_ref: &ArtifactRef,
    out_dir: &Path,
    cooling_s: &[f64],
    mesh: &MeshBounds,
    neutron_request: &Value,
    cancel: &Cancellation,
) -> Result<ActinvEventOutputs, String> {
    let backend = manifest
        .activation_backend
        .as_ref()
        .ok_or("ACTINV backend is not configured")?;
    if backend.kind != "actinv_tendl_709" {
        return Err("unsupported event activation backend".into());
    }
    verify_ref(batch_ref, "709-group ACTINV batch payload")?;
    let batch_bytes = read_bounded(&batch_ref.path, MAX_ACTINV_BATCH_BYTES)?;
    let batches: ActinvEventBatchFile =
        serde_json::from_slice(&batch_bytes).map_err(|e| e.to_string())?;
    if batches.schema != "faris-maintenance-actinv-event-batches/v0.1"
        || batches.batches.is_empty()
        || batches.uncertainty_scope.trim().is_empty()
        || batches.spatial_approximation.trim().is_empty()
        || batches.source_rate_normalization.trim().is_empty()
        || !batches.provenance.is_object()
    {
        return Err("709 response helper returned no supported ACTINV event batches".into());
    }
    let zone_total: usize = batches
        .batches
        .iter()
        .map(|b| b.activation_request.zones.len())
        .sum();
    if zone_total == 0 || zone_total > MAX_ACTINV_ZONE_COUNT {
        return Err(format!(
            "activation request has {zone_total} zones; the event cap is {MAX_ACTINV_ZONE_COUNT}"
        ));
    }
    let expected_schedules = expected_activation_schedules(neutron_request)?;
    if expected_schedules.len() != batches.batches.len() {
        return Err(
            "ACTINV batches do not cover exactly the currently installed component schedules"
                .into(),
        );
    }
    let map_bytes = read_bounded(&manifest.component_material_map.path, MAX_MANIFEST_BYTES)?;
    let component_map: ComponentMaterialMap =
        serde_json::from_slice(&map_bytes).map_err(|e| e.to_string())?;
    let voxel_volume = (0..3)
        .map(|i| (mesh.upper_right_cm[i] - mesh.lower_left_cm[i]) / f64::from(mesh.dimension[i]))
        .product::<f64>();
    let activation_data = crate::activation::ActivationData::catalog(&backend.data_dir);
    if activation_data
        .library
        .canonicalize()
        .map_err(|e| e.to_string())?
        != backend.library_npz.path
        || activation_data
            .decay_primary
            .canonicalize()
            .map_err(|e| e.to_string())?
            != backend.decay_primary.path
        || backend.decay_fallback.as_ref().is_some_and(|fallback| {
            activation_data
                .decay_fallback
                .as_ref()
                .is_none_or(|(path, _)| path.canonicalize().ok().as_ref() != Some(&fallback.path))
        })
    {
        return Err("ACTINV data paths differ from the hash-pinned catalogue files".into());
    }
    verify_ref(&backend.executable, "ACTINV executable")?;
    let actinv_executable_sha256 = crate::maintenance::actinv::hash_file(&backend.executable.path)?;
    let library_index: Value = serde_json::from_slice(&read_bounded(
        &backend.library_index.path,
        MAX_MANIFEST_BYTES,
    )?)
    .map_err(|e| format!("ACTINV library index: {e}"))?;
    let group_boundary_sha256 = library_index
        .get("group_boundary_sha256")
        .and_then(Value::as_str)
        .ok_or("ACTINV library index omits its 709-group boundary hash")?;
    let mut seen_components = std::collections::BTreeSet::new();
    let mut seen_zones = std::collections::BTreeSet::new();
    let mut activation_results = Vec::new();
    let mut source_artifact_refs = BTreeMap::new();
    let mut emitters_by_cooling: Vec<Vec<Value>> = cooling_s.iter().map(|_| Vec::new()).collect();
    let mut total_photons_by_cooling = vec![0.0; cooling_s.len()];
    let mut emitter_records = 0usize;
    let mut emitter_groups = 0usize;
    let mut material_volume_by_element: BTreeMap<u32, f64> = BTreeMap::new();
    let mut bins_seen: std::collections::BTreeSet<(u32, u32)> = std::collections::BTreeSet::new();
    let activation_deadline = Instant::now() + Duration::from_secs_f64(backend.timeout_s);
    for (batch_index, batch) in batches.batches.into_iter().enumerate() {
        if Instant::now() >= activation_deadline {
            return Err(format!(
                "ACTINV event activation exceeded its {:.0} second budget",
                backend.timeout_s
            ));
        }
        if batch.component.trim().is_empty() || !seen_components.insert(batch.component.clone()) {
            return Err("ACTINV event batches have an empty or repeated history component".into());
        }
        batch.activation_request.validate()?;
        if batch.activation_request.cooling_s != cooling_s {
            return Err(format!(
                "ACTINV component {} cooling grid differs from the authored event",
                batch.component
            ));
        }
        let expected_steps = expected_schedules.get(&batch.component).ok_or_else(|| {
            format!(
                "ACTINV component {} has no exact Rust schedule",
                batch.component
            )
        })?;
        if batch.activation_request.schedule_input_sha256.is_some()
            || batch.activation_request.steps.len() != expected_steps.len()
            || batch
                .activation_request
                .steps
                .iter()
                .zip(expected_steps)
                .any(|(actual, expected)| {
                    !close(actual.duration_s, expected.duration_s, 1e-12, 1e-7)
                        || !close(actual.flux_scale, expected.flux_scale, 1e-12, 1e-12)
                })
            || batch.activation_request.photon.group_boundaries_ev.as_ref()
                != Some(&backend.photon_group_boundaries_ev)
            || batch.activation_request.mode != crate::activation::SolverMode::Auto
        {
            return Err(format!(
                "ACTINV component {} schedule or photon grouping differs from the exact Rust event inputs",
                batch.component
            ));
        }
        let zone_ids: std::collections::BTreeSet<_> = batch
            .activation_request
            .zones
            .iter()
            .map(|z| z.id.as_str())
            .collect();
        if zone_ids.len() != batch.activation_request.zones.len()
            || zone_ids != batch.zone_metadata.keys().map(String::as_str).collect()
        {
            return Err(format!(
                "ACTINV component {} zone metadata does not exactly bind all activation zones",
                batch.component
            ));
        }
        if batch
            .activation_request
            .zones
            .iter()
            .any(|zone| !seen_zones.insert(zone.id.clone()))
        {
            return Err("ACTINV zone IDs repeat across component schedules".into());
        }
        for zone in &batch.activation_request.zones {
            let meta = &batch.zone_metadata[&zone.id];
            if meta.material_id == 0
                || !meta.material_volume_cm3.is_finite()
                || meta.material_volume_cm3 <= 0.0
                || (zone.volume_m3 - meta.material_volume_cm3 * 1.0e-6).abs()
                    > zone.volume_m3.max(1e-30) * 1e-9
                || zone.id != format!("e{}_m{}", meta.mesh_element, meta.material_id)
            {
                return Err(format!(
                    "ACTINV zone {} volume or mesh/material identity does not reconcile",
                    zone.id
                ));
            }
            let ids = component_map
                .materials
                .get(&batch.component)
                .ok_or_else(|| {
                    format!(
                        "ACTINV component {} is absent from the checked CAD material map",
                        batch.component
                    )
                })?;
            if !ids.contains(&meta.material_id)
                || !bins_seen.insert((meta.mesh_element, meta.material_id))
            {
                return Err(format!(
                    "ACTINV zone {} is unmapped or duplicates a mesh/material bin",
                    zone.id
                ));
            }
            *material_volume_by_element
                .entry(meta.mesh_element)
                .or_default() += meta.material_volume_cm3;
        }
        let observed_materials: std::collections::BTreeSet<u32> = batch
            .activation_request
            .zones
            .iter()
            .map(|zone| batch.zone_metadata[&zone.id].material_id)
            .collect();
        let expected_materials: std::collections::BTreeSet<u32> = component_map.materials
            [&batch.component]
            .iter()
            .copied()
            .collect();
        if observed_materials != expected_materials {
            return Err(format!(
                "ACTINV component {} lacks mesh activation bins for one or more cloned CAD materials",
                batch.component
            ));
        }
        let work_dir = out_dir
            .join("activation")
            .join(format!("batch_{batch_index:05}"));
        std::fs::create_dir_all(&work_dir).map_err(|e| e.to_string())?;
        let mut config = crate::activation::ActivationConfig::new(
            backend.executable.path.clone(),
            activation_data.clone(),
            work_dir.clone(),
        );
        config.timeout = Duration::from_secs_f64(backend.timeout_s);
        config.cache_dir = Some(backend.cache_dir.clone());
        let activation_cancel = Cancellation::default();
        let worker_cancel = activation_cancel.clone();
        let user_cancel = cancel.clone();
        let deadline = activation_deadline;
        let finished = Arc::new(AtomicBool::new(false));
        let timed_out = Arc::new(AtomicBool::new(false));
        let watchdog_finished = finished.clone();
        let watchdog_timed_out = timed_out.clone();
        let watchdog_cancel = worker_cancel.clone();
        let watchdog = std::thread::spawn(move || {
            while !watchdog_finished.load(Ordering::Acquire) {
                if user_cancel.is_cancelled() {
                    watchdog_cancel.cancel();
                    return;
                }
                if Instant::now() >= deadline {
                    watchdog_timed_out.store(true, Ordering::Release);
                    watchdog_cancel.cancel();
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let activation_result = crate::activation::run_activation(
            &batch.activation_request,
            &config,
            &activation_cancel,
        );
        if !cancel.is_cancelled() && Instant::now() >= deadline {
            timed_out.store(true, Ordering::Release);
        }
        finished.store(true, Ordering::Release);
        let _ = watchdog.join();
        if timed_out.load(Ordering::Acquire) {
            return Err(format!(
                "ACTINV event activation exceeded its {:.0} second budget",
                backend.timeout_s
            ));
        }
        let (result, stats) = activation_result?;
        if result.zones.len() != batch.activation_request.zones.len()
            || result.cooling_s != cooling_s
            || result.zones.iter().any(|z| {
                z.completeness != crate::activation::Completeness::Complete || !z.gaps.is_empty()
            })
        {
            return Err(format!(
                "ACTINV component {} returned incomplete zones or data gaps; refusing a whole-plant dose source",
                batch.component
            ));
        }
        let result_path = out_dir
            .join("activation")
            .join(format!("component_{batch_index:05}_result.json"));
        let result_bytes = serde_json::to_vec_pretty(&result).map_err(|e| e.to_string())?;
        write_exact(&result_path, &result_bytes)?;
        let result_ref = make_ref(&result_path)?;
        source_artifact_refs.insert(
            format!("activation_result/{}", batch.component),
            result_ref.clone(),
        );
        activation_results.push(json!({"component": batch.component, "artifact": result_ref, "actinv_runs": stats.actinv_runs,
                                       "cache_hits": stats.cache_hits, "schema": result.schema,
                                       "input_sha256": result.input_sha256, "data": result.data}));
        for zone in &result.zones {
            let meta = &batch.zone_metadata[&zone.id];
            for (point_index, point) in zone.points.iter().enumerate() {
                if point.cooling_s != cooling_s[point_index] {
                    return Err(format!(
                        "ACTINV zone {} result cooling grid is out of order",
                        zone.id
                    ));
                }
                let photons = point.photon_source.total_photons_per_s;
                let outside = point.photon_source.photons_outside_groups_per_s;
                if !photons.is_finite()
                    || photons < 0.0
                    || !outside.is_finite()
                    || outside > (photons * 1e-12).max(1e-12)
                {
                    return Err(format!(
                        "ACTINV zone {} has non-finite photons or an unbounded out-of-group rate",
                        zone.id
                    ));
                }
                let grouped: f64 = point
                    .photon_source
                    .groups
                    .iter()
                    .map(|g| g.photons_per_s)
                    .sum();
                if !grouped.is_finite() || (grouped - photons).abs() > (photons * 1e-8).max(1e-12) {
                    return Err(format!(
                        "ACTINV zone {} photon groups do not conserve its total rate",
                        zone.id
                    ));
                }
                if photons == 0.0 {
                    continue;
                }
                emitter_records = emitter_records.saturating_add(1);
                emitter_groups = emitter_groups.saturating_add(
                    point
                        .photon_source
                        .groups
                        .iter()
                        .filter(|g| g.photons_per_s > 0.0)
                        .count(),
                );
                if emitter_records > MAX_ACTINV_EMITTER_RECORDS
                    || emitter_groups > MAX_ACTINV_EMITTER_GROUPS
                {
                    return Err(
                        "ACTINV emitter count exceeds the bounded event artifact budget".into(),
                    );
                }
                let source_file =
                    crate::activation::photon_source_file(&result, &zone.id, point.cooling_s)?;
                let source_path = out_dir.join("activation").join(format!(
                    "{}__{}s.photon-source.json",
                    zone.id, point.cooling_s
                ));
                let source_hash =
                    crate::activation::write_photon_source_file(&source_path, &source_file)?;
                let source_ref = ArtifactRef {
                    path: source_path,
                    sha256: source_hash,
                };
                source_artifact_refs.insert(
                    format!("photon_source/{}/{}", zone.id, point.cooling_s),
                    source_ref.clone(),
                );
                let (lower, upper) = voxel_bounds(mesh, meta.mesh_element)?;
                emitters_by_cooling[point_index].push(json!({
                    "component": batch.component, "zone_id": zone.id,
                    "mesh_element": meta.mesh_element, "material_id": meta.material_id,
                    "material_volume_cm3": meta.material_volume_cm3,
                    "lower_left_cm": lower, "upper_right_cm": upper,
                    "photons_per_s": photons,
                    "energy_groups": point.photon_source.groups.iter().filter(|g| g.photons_per_s > 0.0).map(|g| json!({
                        "energy_ev": g.centroid_ev, "energy_low_ev": g.low_ev,
                        "energy_high_ev": g.high_ev, "photons_per_s": g.photons_per_s
                    })).collect::<Vec<_>>(),
                    "photon_source_artifact": source_ref.clone()
                }));
                total_photons_by_cooling[point_index] += photons;
            }
        }
    }
    if material_volume_by_element
        .values()
        .any(|volume| !volume.is_finite() || *volume > voxel_volume * (1.0 + 1e-6))
    {
        return Err(
            "ACTINV mesh/material volumes exceed the authored activation voxel volume".into(),
        );
    }
    if seen_components.is_empty()
        || total_photons_by_cooling
            .iter()
            .any(|x| !x.is_finite() || *x <= 0.0)
    {
        return Err(
            "ACTINV emitted no positive whole-plant photon source at one or more cooling times"
                .into(),
        );
    }
    let emitters_by_cooling: Vec<Value> = cooling_s
        .iter()
        .enumerate()
        .map(|(i, cooling)| {
            json!({
                "cooling_s": cooling, "source_strength_photons_s": total_photons_by_cooling[i],
                "emitters": emitters_by_cooling[i]
            })
        })
        .collect();
    let receipt_path = out_dir.join("activation/actinv-event-receipt.json");
    let receipt = json!({
        "schema": "faris-maintenance-actinv-event-receipt/v0.1", "status": "PASS",
        "batch_payload": batch_ref, "activation_results": activation_results.clone(),
        "points": cooling_s.iter().enumerate().map(|(i,t)| json!({"cooling_s": t, "source_strength_photons_s": total_photons_by_cooling[i]})).collect::<Vec<_>>(),
        "group_boundaries_ev": &backend.photon_group_boundaries_ev,
        "group_boundary_sha256": group_boundary_sha256,
        "actinv_executable_sha256": actinv_executable_sha256,
        "actinv_library_npz_sha256": backend.library_npz.sha256,
        "actinv_library_index_sha256": backend.library_index.sha256,
        "actinv_decay_primary_sha256": backend.decay_primary.sha256,
        "actinv_decay_fallback_sha256": backend.decay_fallback.as_ref().map(|r| r.sha256.clone()),
        "source_normalization": "ACTINV photons/s per mesh-material zone are absolute rates. They are combined once as source strengths; no fusion/neutron source rate is applied again.",
        "spatial_source_approximation": "Each source is uniform within the corresponding mesh voxel and constrained to its exact cloned DAGMC material ID; ACTINV mesh/material volumes and composition are matched by zone ID.",
        "limits": {"zones": MAX_ACTINV_ZONE_COUNT, "emitter_records": MAX_ACTINV_EMITTER_RECORDS,
                   "emitter_group_entries": MAX_ACTINV_EMITTER_GROUPS, "artifact_bytes": MAX_ACTINV_BATCH_BYTES},
        "uncertainty_scope": "ACTINV source/inventory is conditional on neutron 709 response, composition, material-volume sampling, activation library and decay data. It excludes neutron-response, nuclear-data, geometry, composition, and source-model uncertainty. Photon transport batch uncertainty is recorded separately."
    });
    write_exact(
        &receipt_path,
        &serde_json::to_vec_pretty(&receipt).map_err(|e| e.to_string())?,
    )?;
    Ok(ActinvEventOutputs {
        activation_results,
        emitters_by_cooling,
        receipt_ref: make_ref(&receipt_path)?,
        source_artifact_refs,
        total_photons_by_cooling,
    })
}

fn voxel_bounds(mesh: &MeshBounds, element: u32) -> Result<([f64; 3], [f64; 3]), String> {
    let count = mesh
        .dimension
        .iter()
        .map(|x| u64::from(*x))
        .product::<u64>();
    if u64::from(element) >= count {
        return Err("activation mesh element index is out of bounds".into());
    }
    let nx = u64::from(mesh.dimension[0]);
    let ny = u64::from(mesh.dimension[1]);
    let i = u64::from(element) % nx;
    let j = (u64::from(element) / nx) % ny;
    let k = u64::from(element) / (nx * ny);
    let widths = std::array::from_fn::<_, 3, _>(|a| {
        (mesh.upper_right_cm[a] - mesh.lower_left_cm[a]) / f64::from(mesh.dimension[a])
    });
    let index = [i, j, k];
    let lower = std::array::from_fn(|a| mesh.lower_left_cm[a] + index[a] as f64 * widths[a]);
    let upper = std::array::from_fn(|a| lower[a] + widths[a]);
    Ok((lower, upper))
}

fn worker_artifact(
    receipt: &Value,
    key: &str,
    root: &Path,
    label: &str,
) -> Result<ArtifactRef, String> {
    let mut reference: ArtifactRef = serde_json::from_value(
        receipt
            .get(key)
            .cloned()
            .ok_or_else(|| format!("worker receipt omits {key}"))?,
    )
    .map_err(|e| format!("worker receipt {key}: {e}"))?;
    if !reference.path.is_absolute() {
        reference.path = root.join(&reference.path);
    }
    let canonical = reference
        .path
        .canonicalize()
        .map_err(|e| format!("{label}: {e}"))?;
    let canonical_root = root.canonicalize().map_err(|e| e.to_string())?;
    if !canonical.starts_with(&canonical_root) || !canonical.is_file() {
        return Err(format!(
            "{label} must be a regular file under its isolated phase output"
        ));
    }
    reference.path = canonical;
    verify_ref(&reference, label)?;
    Ok(reference)
}

fn parse_stage_receipt(
    bytes: &[u8],
    schema: &str,
    expected_request_sha: &str,
) -> Result<Value, String> {
    let receipt: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if receipt.get("schema").and_then(Value::as_str) != Some(schema)
        || receipt.get("status").and_then(Value::as_str) != Some("PASS")
        || receipt.get("request_sha256").and_then(Value::as_str) != Some(expected_request_sha)
    {
        return Err(format!(
            "worker receipt must be PASS {schema} bound to the exact request hash"
        ));
    }
    Ok(receipt)
}

struct StagedActinvDose {
    receipt: Value,
    request_ref: ArtifactRef,
    record_ref: ArtifactRef,
    artifacts: BTreeMap<String, ArtifactRef>,
}

#[allow(clippy::too_many_arguments)]
fn run_staged_actinv_dose(
    python: &Path,
    worker: &Path,
    run_dir: &Path,
    neutron_request: &Value,
    manifest: &DosePhysicsManifest,
    cooling_s: &[f64],
    response_mesh: &Value,
    event: &Value,
    sampling: &Value,
    cancel: &Cancellation,
) -> Result<StagedActinvDose, String> {
    let backend = manifest
        .activation_backend
        .as_ref()
        .ok_or("ACTINV backend is missing")?;
    let neutron_dir = run_dir.join("neutron");
    std::fs::create_dir_all(&neutron_dir).map_err(|e| e.to_string())?;
    let neutron_request_path = run_dir.join("709-neutron-request.json");
    let neutron_bytes = serde_json::to_vec_pretty(neutron_request).map_err(|e| e.to_string())?;
    if neutron_bytes.len() > MAX_WORKER_REQUEST_BYTES {
        return Err("709 neutron request exceeds 4 MiB".into());
    }
    write_exact(&neutron_request_path, &neutron_bytes)?;
    let neutron_request_ref = make_ref(&neutron_request_path)?;
    let neutron_log = run_dir.join("709-neutron.log");
    let result = launch_bounded(
        python,
        worker,
        &neutron_request_path,
        &neutron_dir,
        run_dir,
        &neutron_log,
        manifest.sampling.run_budget_s,
        cancel,
    )?;
    check_job_success(
        &result,
        &neutron_log,
        "OpenMC 709 neutron response",
        manifest.sampling.run_budget_s,
    )?;
    let neutron_job_record = run_dir.join("709-neutron-job.json");
    if !neutron_job_record.is_file() {
        return Err("bounded neutron worker omitted its job metadata artifact".into());
    }
    let neutron_receipt_path = neutron_dir.join("709_neutron_receipt.json");
    let neutron_receipt = parse_stage_receipt(
        &read_bounded(&neutron_receipt_path, MAX_MANIFEST_BYTES)?,
        "faris-maintenance-709-neutron-response-receipt/v0.1",
        &neutron_request_ref.sha256,
    )?;
    let index: Value = serde_json::from_slice(&read_bounded(
        &backend.library_index.path,
        MAX_MANIFEST_BYTES,
    )?)
    .map_err(|e| format!("ACTINV library index: {e}"))?;
    let expected_group_hash = index
        .get("group_boundary_sha256")
        .and_then(Value::as_str)
        .ok_or("ACTINV library index omits the 709 group-boundary hash")?;
    if neutron_receipt
        .get("group_boundary_sha256")
        .and_then(Value::as_str)
        != Some(expected_group_hash)
    {
        return Err(
            "709 response receipt group boundaries differ from the exact ACTINV library".into(),
        );
    }
    if neutron_receipt
        .get("history_sha256")
        .and_then(Value::as_str)
        != neutron_request
            .get("history_sha256")
            .and_then(Value::as_str)
        || neutron_receipt.get("event") != neutron_request.get("event")
    {
        return Err(
            "709 response receipt does not echo the exact history and event identity".into(),
        );
    }
    for key in ["history_artifact", "source_schedules"] {
        let expected = neutron_request
            .get(key)
            .and_then(|v| v.get("sha256"))
            .and_then(Value::as_str);
        let observed = neutron_receipt
            .get(key)
            .and_then(|v| v.get("sha256"))
            .and_then(Value::as_str);
        if expected.is_none() || expected != observed {
            return Err(format!(
                "709 response receipt {key} hash differs from the event request"
            ));
        }
    }
    let response_npz = worker_artifact(
        &neutron_receipt,
        "response_npz",
        &neutron_dir,
        "709 neutron response NPZ",
    )?;
    let response_record = worker_artifact(
        &neutron_receipt,
        "neutron_response_record",
        &neutron_dir,
        "709 neutron response execution record",
    )?;
    let response_record_value: Value =
        serde_json::from_slice(&read_bounded(&response_record.path, MAX_MANIFEST_BYTES)?)
            .map_err(|e| format!("709 neutron response execution record: {e}"))?;
    if response_record_value.get("schema").and_then(Value::as_str)
        != Some("faris-maintenance-709-neutron-response/v0.1")
        || response_record_value.get("status").and_then(Value::as_str) != Some("PASS")
        || response_record_value
            .get("request_sha256")
            .and_then(Value::as_str)
            != Some(neutron_request_ref.sha256.as_str())
    {
        return Err(
            "709 neutron internal response record is not a PASS record for this request".into(),
        );
    }
    for (key, expected) in [
        ("response_npz_sha256", response_npz.sha256.as_str()),
        ("model_sha256", manifest.model_xml.sha256.as_str()),
        ("geometry_sha256", manifest.geometry_h5m.sha256.as_str()),
        (
            "cross_sections_sha256",
            manifest.cross_sections_xml.sha256.as_str(),
        ),
        ("group_boundary_sha256", expected_group_hash),
    ] {
        if response_record_value.get(key).and_then(Value::as_str) != Some(expected) {
            return Err(format!(
                "internal neutron response record {key} differs from checked inputs"
            ));
        }
    }
    if response_record_value.get("activation_mesh") != neutron_receipt.get("activation_mesh")
        || response_record_value.get("source") != neutron_receipt.get("source")
    {
        return Err("internal and external neutron response execution records disagree".into());
    }
    for key in [
        "openmc_python_version",
        "openmc_c_library",
        "openmc_c_library_sha256",
    ] {
        if response_record_value.pointer(&format!("/runtime/{key}"))
            != neutron_receipt.pointer(&format!("/runtime/{key}"))
        {
            return Err(format!(
                "internal and external neutron response runtime field {key} disagrees"
            ));
        }
    }
    let neutron_solver_model = worker_artifact(
        &neutron_receipt,
        "solver_model",
        &neutron_dir,
        "actual OpenMC neutron solver model XML",
    )?;
    for key in [
        "particles",
        "batches",
        "histories",
        "lost_particles",
        "lost_particle_fraction_limit",
        "solver_status",
        "stop_reason",
    ] {
        if response_record_value.pointer(&format!("/solver/{key}"))
            != neutron_receipt.pointer(&format!("/solver/{key}"))
        {
            return Err(format!(
                "internal and external neutron response solver field {key} disagrees"
            ));
        }
    }
    let activation_batches = worker_artifact(
        &neutron_receipt,
        "activation_batches",
        &neutron_dir,
        "ACTINV event batches",
    )?;
    for (key, expected) in [
        ("model", &manifest.model_xml),
        ("geometry", &manifest.geometry_h5m),
        ("cross_sections", &manifest.cross_sections_xml),
        ("component_material_map", &manifest.component_material_map),
        ("activation_library_npz", &backend.library_npz),
        ("activation_library_index", &backend.library_index),
    ] {
        let observed: ArtifactRef = serde_json::from_value(
            neutron_receipt
                .get(key)
                .cloned()
                .ok_or_else(|| format!("709 response receipt omits {key}"))?,
        )
        .map_err(|e| e.to_string())?;
        if observed.sha256 != expected.sha256 {
            return Err(format!(
                "709 response receipt {key} identity differs from prepared physics"
            ));
        }
    }
    let batch_file: Value = serde_json::from_slice(&read_bounded(
        &activation_batches.path,
        MAX_ACTINV_BATCH_BYTES,
    )?)
    .map_err(|e| format!("ACTINV batch artifact: {e}"))?;
    if batch_file.get("schema").and_then(Value::as_str)
        != Some("faris-maintenance-actinv-event-batches/v0.1")
    {
        return Err("ACTINV batch artifact has an unsupported schema".into());
    }
    let provenance = batch_file
        .get("provenance")
        .ok_or("ACTINV batch artifact omits provenance")?;
    for (key, expected) in [
        ("model_sha256", &manifest.model_xml.sha256),
        ("geometry_sha256", &manifest.geometry_h5m.sha256),
        (
            "component_material_map_sha256",
            &manifest.component_material_map.sha256,
        ),
        ("neutron_response_sha256", &response_npz.sha256),
        ("actinv_library_npz_sha256", &backend.library_npz.sha256),
        ("actinv_library_index_sha256", &backend.library_index.sha256),
    ] {
        if provenance.get(key).and_then(Value::as_str) != Some(expected.as_str()) {
            return Err(format!(
                "ACTINV batch provenance {key} differs from the checked phase artifacts"
            ));
        }
    }
    for (key, expected) in [
        (
            "history_sha256",
            neutron_request
                .get("history_sha256")
                .and_then(Value::as_str),
        ),
        (
            "source_schedules_sha256",
            neutron_request
                .get("source_schedules")
                .and_then(|v| v.get("sha256"))
                .and_then(Value::as_str),
        ),
        ("group_boundary_sha256", Some(expected_group_hash)),
    ] {
        if provenance.get(key).and_then(Value::as_str) != expected {
            return Err(format!(
                "ACTINV batch provenance {key} differs from the exact event request"
            ));
        }
    }
    if neutron_receipt
        .get("activation_mesh")
        .and_then(Value::as_object)
        != serde_json::to_value(&manifest.activation_mesh)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .as_ref()
    {
        return Err(
            "709 response receipt activation mesh differs from the prepared manifest".into(),
        );
    }
    let vertex_bounds = neutron_receipt
        .get("h5m_vertex_bounds")
        .ok_or("709 response receipt omits checked H5M vertex bounds")?;
    let vertex_nodes = vertex_bounds
        .get("node_count")
        .and_then(Value::as_u64)
        .ok_or("709 response receipt omits H5M vertex count")?;
    let vertex_bytes = vertex_bounds
        .get("coordinate_data_bytes")
        .and_then(Value::as_u64)
        .ok_or("709 response receipt omits H5M coordinate byte count")?;
    if vertex_nodes == 0 || vertex_nodes > 20_000_000 || vertex_bytes > 256 * 1024 * 1024 {
        return Err("H5M geometry coverage evidence exceeds the bounded vertex limits".into());
    }
    let geometry_lower = value_vec3(vertex_bounds, "lower_left_cm")?;
    let geometry_upper = value_vec3(vertex_bounds, "upper_right_cm")?;
    if (0..3).any(|axis| {
        manifest.activation_mesh.lower_left_cm[axis] > geometry_lower[axis]
            || manifest.activation_mesh.upper_right_cm[axis] < geometry_upper[axis]
    }) {
        return Err("activation mesh does not cover the full checked H5M vertex bounds".into());
    }
    let source = neutron_receipt
        .get("source")
        .ok_or("709 response receipt omits source normalization")?;
    let expected_base_rate = source_groups_base_rate(neutron_request)?;
    if source.get("unit_strength").and_then(Value::as_f64) != Some(1.0)
        || source
            .get("base_source_rate_n_s")
            .and_then(Value::as_f64)
            .is_none_or(|x| (x - expected_base_rate).abs() > x.abs().max(1.0) * 1e-12)
    {
        return Err(
            "709 response receipt source normalization differs from the event request".into(),
        );
    }
    if !is_sha256(source.get("kernel_sha256").and_then(Value::as_str)) {
        return Err("709 response receipt has no valid source-kernel SHA-256".into());
    }
    let solver = neutron_receipt
        .get("solver")
        .ok_or("709 response receipt omits solver evidence")?;
    let particles = solver
        .get("particles")
        .and_then(Value::as_u64)
        .ok_or("709 response receipt has no particle count")?;
    let batches = solver
        .get("batches")
        .and_then(Value::as_u64)
        .ok_or("709 response receipt has no batch count")?;
    let lost = solver
        .get("lost_particles")
        .and_then(Value::as_u64)
        .ok_or("709 response receipt has no lost-particle count")?;
    let histories = solver
        .get("histories")
        .and_then(Value::as_u64)
        .ok_or("709 response receipt has no completed history count")?;
    let loss_limit = solver
        .get("lost_particle_fraction_limit")
        .and_then(Value::as_f64)
        .ok_or("709 response receipt has no declared lost-particle fraction limit")?;
    let requested_particles = neutron_request
        .pointer("/sampling/neutron_particles")
        .and_then(Value::as_u64)
        .ok_or("709 request has no neutron particle count")?;
    let requested_batches = neutron_request
        .pointer("/sampling/neutron_batches")
        .and_then(Value::as_u64)
        .ok_or("709 request has no neutron batch count")?;
    if batches != requested_batches
        || particles != requested_particles
        || solver.get("solver_status").and_then(Value::as_i64) != Some(1)
        || solver.get("stop_reason").and_then(Value::as_str) != Some("batch_limit")
        || batches < 2
        || particles == 0
        || histories != particles.saturating_mul(batches)
        || !loss_limit.is_finite()
        || loss_limit > 1e-6
        || lost as f64 > 1e-6 * (particles as f64) * (batches as f64)
    {
        return Err(
            "709 neutron response did not complete the requested batches or exceeds the lost-particle limit".into(),
        );
    }
    let runtime = neutron_receipt
        .get("runtime")
        .ok_or("709 response receipt omits runtime identity")?;
    if runtime
        .get("openmc_python_version")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
        || runtime
            .get("openmc_c_library")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        || !is_sha256(
            runtime
                .get("openmc_c_library_sha256")
                .and_then(Value::as_str),
        )
        || solver
            .get("stop_reason")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err("709 response receipt omits actual OpenMC runtime identity".into());
    }
    let neutron_openmc_library = ArtifactRef {
        path: PathBuf::from(
            runtime
                .get("openmc_c_library")
                .and_then(Value::as_str)
                .unwrap(),
        ),
        sha256: runtime
            .get("openmc_c_library_sha256")
            .and_then(Value::as_str)
            .unwrap()
            .to_string(),
    };
    verify_ref(&neutron_openmc_library, "loaded OpenMC C API library")?;
    let openmc_python_module = make_ref(Path::new(
        runtime
            .get("openmc_python_module")
            .and_then(Value::as_str)
            .ok_or("709 response receipt omits the loaded OpenMC Python module path")?,
    ))?;

    let activation = run_actinv_event_batches(
        manifest,
        &activation_batches,
        run_dir,
        cooling_s,
        &manifest.activation_mesh,
        neutron_request,
        cancel,
    )?;
    let activation_results = activation.activation_results.clone();
    let activation_artifacts: Vec<Value> = activation_results.iter().map(|item| {
        let reference = item.get("artifact").ok_or("native ACTINV result lacks its artifact ref")?;
        let mut value = serde_json::from_value::<ArtifactRef>(reference.clone()).map_err(|e| e.to_string())?;
        if !value.path.is_absolute() { value.path = run_dir.join(&value.path); }
        verify_ref(&value, "native ACTINV result")?;
        Ok(json!({"component": item.get("component").and_then(Value::as_str).ok_or("native ACTINV result lacks component")?,
                  "path": value.path, "sha256": value.sha256}))
    }).collect::<Result<_, String>>()?;
    let emitters_path = run_dir.join("activation/emitters-by-cooling.json");
    let emitters_payload = json!({"schema":"faris-maintenance-actinv-photon-emitters/v0.1",
                                  "emitters_by_cooling": activation.emitters_by_cooling});
    let emitters_bytes = serde_json::to_vec(&emitters_payload).map_err(|e| e.to_string())?;
    if emitters_bytes.len() > MAX_ACTINV_BATCH_BYTES {
        return Err("ACTINV emitter source artifact exceeds 256 MiB".into());
    }
    write_exact(&emitters_path, &emitters_bytes)?;
    let emitters_ref = make_ref(&emitters_path)?;
    let neutron_receipt_ref = make_ref(&neutron_receipt_path)?;
    let photon_request = json!({
        "schema": "faris-maintenance-709-photon-request/v0.1", "request_id": neutron_request["request_id"], "stage": "photon",
        "model": manifest.model_xml, "geometry": manifest.geometry_h5m,
        "cross_sections": manifest.cross_sections_xml, "component_material_map": manifest.component_material_map,
        "neutron_response_receipt": neutron_receipt_ref,
        "activation_mesh": manifest.activation_mesh, "response_mesh": response_mesh,
        "event": event, "emitters_by_cooling": emitters_ref,
        "activation_results": activation_artifacts,
        "sampling": sampling,
    });
    let photon_dir = run_dir.join("photon");
    std::fs::create_dir_all(&photon_dir).map_err(|e| e.to_string())?;
    let photon_request_path = run_dir.join("709-photon-request.json");
    let photon_bytes = serde_json::to_vec_pretty(&photon_request).map_err(|e| e.to_string())?;
    if photon_bytes.len() > MAX_WORKER_REQUEST_BYTES {
        return Err("709 photon request exceeds 4 MiB".into());
    }
    write_exact(&photon_request_path, &photon_bytes)?;
    let photon_request_ref = make_ref(&photon_request_path)?;
    let photon_log = run_dir.join("709-photon.log");
    let result = launch_bounded(
        python,
        worker,
        &photon_request_path,
        &photon_dir,
        run_dir,
        &photon_log,
        manifest.sampling.run_budget_s,
        cancel,
    )?;
    check_job_success(
        &result,
        &photon_log,
        "OpenMC full-geometry photon response",
        manifest.sampling.run_budget_s,
    )?;
    let photon_job_record = run_dir.join("709-photon-job.json");
    if !photon_job_record.is_file() {
        return Err("bounded photon worker omitted its job metadata artifact".into());
    }
    let photon_receipt_path = photon_dir.join("709_photon_receipt.json");
    let mut photon_receipt = parse_stage_receipt(
        &read_bounded(&photon_receipt_path, MAX_MANIFEST_BYTES)?,
        "faris-maintenance-709-photon-receipt/v0.1",
        &photon_request_ref.sha256,
    )?;
    if photon_receipt
        .get("activation_results")
        .and_then(Value::as_array)
        != Some(&activation_artifacts)
    {
        return Err(
            "photon receipt activation result references differ from the native ACTINV outputs"
                .into(),
        );
    }
    for (key, expected) in [
        ("neutron_response_receipt", &neutron_receipt_ref),
        ("emitters_by_cooling", &emitters_ref),
    ] {
        let observed: ArtifactRef = serde_json::from_value(
            photon_receipt
                .get(key)
                .cloned()
                .ok_or_else(|| format!("photon receipt omits {key}"))?,
        )
        .map_err(|e| format!("photon receipt {key}: {e}"))?;
        if observed.sha256 != expected.sha256 || observed.path != expected.path {
            return Err(format!(
                "photon receipt {key} differs from the exact stage input"
            ));
        }
    }
    let photon_points = photon_receipt
        .get("points")
        .and_then(Value::as_array)
        .ok_or("photon receipt omits points")?;
    if photon_points.len() != cooling_s.len() {
        return Err("photon response point count differs from the cooling grid".into());
    }
    let mut photon_solver_models = Vec::with_capacity(photon_points.len());
    let mut photon_openmc_library: Option<ArtifactRef> = None;
    for (index, point) in photon_points.iter().enumerate() {
        let batches = point
            .get("batches")
            .and_then(Value::as_u64)
            .ok_or("photon point omits completed batch count")?;
        let particles = point
            .get("particles_per_batch")
            .and_then(Value::as_u64)
            .ok_or("photon point omits particles per batch")?;
        let histories = point
            .get("histories")
            .and_then(Value::as_u64)
            .ok_or("photon point omits completed history count")?;
        let lost = point
            .get("lost_particles")
            .and_then(Value::as_u64)
            .ok_or("photon point omits lost-particle count")?;
        let loss_limit = point
            .get("lost_particle_fraction_limit")
            .and_then(Value::as_f64)
            .ok_or("photon point omits lost-particle fraction limit")?;
        let requested_batches = sampling
            .get("photon_batches")
            .and_then(Value::as_u64)
            .ok_or("photon request has no photon batch count")?;
        let requested_particles = sampling
            .get("photon_particles")
            .and_then(Value::as_u64)
            .ok_or("photon request has no photon particle count")?;
        if batches != requested_batches
            || particles != requested_particles
            || histories != batches.saturating_mul(particles)
            || batches < 2
            || point.get("solver_status").and_then(Value::as_i64) != Some(1)
            || point.get("stop_reason").and_then(Value::as_str) != Some("batch_limit")
            || point
                .get("seed")
                .and_then(Value::as_u64)
                .is_none_or(|seed| seed == 0)
            || !loss_limit.is_finite()
            || loss_limit > 1e-6
            || lost as f64 > 1e-6 * histories as f64
        {
            return Err(
                "photon point failed requested-batch, solver-status, seed, or particle-loss checks"
                    .into(),
            );
        }
        if value_f64(point, "standard_error_sv_h")? < 0.0 {
            return Err("photon point standard error is negative".into());
        }
        let model_holder = json!({"solver_model": point.get("model_xml")
            .ok_or("photon point omits its actual solver model XML artifact")?});
        let solver_model = worker_artifact(
            &model_holder,
            "solver_model",
            &photon_dir,
            "actual OpenMC photon solver model XML",
        )?;
        let library: ArtifactRef = serde_json::from_value(
            point
                .get("c_api_library")
                .cloned()
                .ok_or("photon point omits its loaded OpenMC C API library")?,
        )
        .map_err(|e| format!("photon C API library reference: {e}"))?;
        verify_ref(&library, "loaded OpenMC photon C API library")?;
        if library.sha256 != neutron_openmc_library.sha256
            || library.path != neutron_openmc_library.path
        {
            return Err(
                "photon transport loaded a different OpenMC C API library than neutron transport"
                    .into(),
            );
        }
        if let Some(previous) = &photon_openmc_library {
            if previous != &library {
                return Err(
                    "OpenMC C API library identity changed between photon cooling points".into(),
                );
            }
        } else {
            photon_openmc_library = Some(library);
        }
        photon_solver_models.push(solver_model);
        if value_f64(point, "cooling_s")? != cooling_s[index]
            || (value_f64(point, "source_strength_photons_s")?
                - activation.total_photons_by_cooling[index])
                .abs()
                > activation.total_photons_by_cooling[index].abs().max(1.0) * 1e-8
        {
            return Err(
                "photon response cooling/source strength does not reconcile with ACTINV results"
                    .into(),
            );
        }
    }
    if let Some(d1s) = neutron_receipt.get("d1s") {
        photon_receipt["d1s"] = d1s.clone();
    } else {
        photon_receipt["d1s"] = json!({"status":"NOT_EVALUATED", "reason":"ACTINV 709 backend does not perform compatible CE D1S", "points": []});
    }
    photon_receipt["runtime"] = neutron_receipt["runtime"].clone();
    photon_receipt["activation_receipt"] = json!(activation.receipt_ref);
    let final_receipt_path = run_dir.join("709-dose-receipt.json");
    write_exact(
        &final_receipt_path,
        &serde_json::to_vec_pretty(&photon_receipt).map_err(|e| e.to_string())?,
    )?;
    let mut artifacts = activation.source_artifact_refs;
    artifacts.insert("cross_sections".into(), manifest.cross_sections_xml.clone());
    artifacts.insert("actinv_executable".into(), backend.executable.clone());
    artifacts.insert("actinv_library_npz".into(), backend.library_npz.clone());
    artifacts.insert("actinv_library_index".into(), backend.library_index.clone());
    artifacts.insert("actinv_decay_primary".into(), backend.decay_primary.clone());
    if let Some(fallback) = &backend.decay_fallback {
        artifacts.insert("actinv_decay_fallback".into(), fallback.clone());
    }
    artifacts.insert("709_neutron_request".into(), neutron_request_ref);
    artifacts.insert("709_neutron_response".into(), response_npz);
    artifacts.insert("709_neutron_response_record".into(), response_record);
    artifacts.insert("709_neutron_solver_model".into(), neutron_solver_model);
    artifacts.insert("openmc_c_api_library".into(), neutron_openmc_library);
    artifacts.insert("openmc_python_module".into(), openmc_python_module);
    for (index, reference) in photon_solver_models.into_iter().enumerate() {
        artifacts.insert(format!("709_photon_solver_model_{index:03}"), reference);
    }
    artifacts.insert("709_neutron_receipt".into(), neutron_receipt_ref);
    artifacts.insert("actinv_batches".into(), activation_batches);
    artifacts.insert("actinv_receipt".into(), activation.receipt_ref);
    artifacts.insert("actinv_emitters".into(), emitters_ref);
    artifacts.insert("709_photon_request".into(), photon_request_ref.clone());
    artifacts.insert("709_photon_receipt".into(), make_ref(&photon_receipt_path)?);
    artifacts.insert("709_neutron_worker_log".into(), make_ref(&neutron_log)?);
    artifacts.insert(
        "709_neutron_worker_job".into(),
        make_ref(&neutron_job_record)?,
    );
    artifacts.insert("709_photon_worker_log".into(), make_ref(&photon_log)?);
    artifacts.insert(
        "709_photon_worker_job".into(),
        make_ref(&photon_job_record)?,
    );
    Ok(StagedActinvDose {
        receipt: photon_receipt,
        request_ref: photon_request_ref,
        record_ref: make_ref(&final_receipt_path)?,
        artifacts,
    })
}

fn source_groups_base_rate(request: &Value) -> Result<f64, String> {
    let value = request
        .get("base_source_rate_n_s")
        .and_then(Value::as_f64)
        .ok_or("709 neutron request lacks base_source_rate_n_s")?;
    if !value.is_finite() || value <= 0.0 {
        return Err("base neutron rate is invalid".into());
    }
    Ok(value)
}

fn expected_activation_schedules(
    request: &Value,
) -> Result<BTreeMap<String, Vec<irradiation::IrradiationStep>>, String> {
    let base_rate = source_groups_base_rate(request)?;
    let history_sha = request
        .get("history_sha256")
        .and_then(Value::as_str)
        .ok_or("709 neutron request lacks the exact history SHA-256")?;
    let shutdown = request
        .pointer("/event/shutdown_s")
        .and_then(Value::as_f64)
        .ok_or("709 neutron request lacks shutdown time")?;
    if !shutdown.is_finite() || shutdown <= 0.0 {
        return Err("709 neutron request shutdown time must be finite and positive".into());
    }
    let source_groups = request
        .get("source_groups")
        .and_then(Value::as_array)
        .ok_or("709 neutron request lacks component source schedules")?;
    let mut schedules = BTreeMap::new();
    for group in source_groups {
        let component = group
            .get("component")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .ok_or("source schedule has no component")?;
        if group.get("history_sha256").and_then(Value::as_str) != Some(history_sha) {
            return Err(format!(
                "{component}: source schedule is not bound to the exact Rust history"
            ));
        }
        let installation = group
            .get("installation_s")
            .and_then(Value::as_f64)
            .ok_or_else(|| format!("{component}: source schedule has no installation time"))?;
        let target = shutdown - installation;
        if !installation.is_finite() || installation < 0.0 || !target.is_finite() || target <= 0.0 {
            return Err(format!(
                "{component}: installation time is invalid for this shutdown"
            ));
        }
        let raw_steps = group
            .get("schedule")
            .and_then(Value::as_array)
            .filter(|steps| !steps.is_empty())
            .ok_or_else(|| format!("{component}: source schedule is empty"))?;
        let mut elapsed = 0.0;
        let mut steps = Vec::new();
        for raw in raw_steps {
            let duration = raw
                .get("duration_s")
                .and_then(Value::as_f64)
                .ok_or_else(|| format!("{component}: source interval lacks duration"))?;
            let rate = raw
                .get("source_rate_n_s")
                .and_then(Value::as_f64)
                .ok_or_else(|| format!("{component}: source interval lacks neutron rate"))?;
            if !duration.is_finite() || duration <= 0.0 || !rate.is_finite() || rate < 0.0 {
                return Err(format!(
                    "{component}: source interval duration/rate is invalid"
                ));
            }
            if elapsed < target - 1e-6 {
                let portion = duration.min(target - elapsed);
                steps.push(irradiation::IrradiationStep {
                    duration_s: portion,
                    flux_scale: rate / base_rate,
                });
                elapsed += portion;
                if portion < duration && rate != 0.0 {
                    return Err(format!(
                        "{component}: a nonzero source interval crosses shutdown"
                    ));
                }
            } else if rate != 0.0 {
                return Err(format!(
                    "{component}: post-shutdown source interval is nonzero"
                ));
            }
        }
        if (elapsed - target).abs() > target.abs().max(1.0) * 1e-10 {
            return Err(format!(
                "{component}: exact source schedule does not end at shutdown"
            ));
        }
        if schedules.insert(component.to_string(), steps).is_some() {
            return Err(format!("{component}: duplicate component source schedule"));
        }
    }
    if schedules.is_empty() {
        return Err("709 request has no active installed component source schedules".into());
    }
    Ok(schedules)
}

fn close(left: f64, right: f64, relative: f64, absolute: f64) -> bool {
    (left - right).abs() <= absolute.max(relative * left.abs().max(right.abs()))
}

fn is_sha256(value: Option<&str>) -> bool {
    value.is_some_and(|text| text.len() == 64 && text.bytes().all(|c| c.is_ascii_hexdigit()))
}

fn check_job_success(
    result: &JobResult,
    log: &Path,
    what: &str,
    budget_s: f64,
) -> Result<(), String> {
    if result.execution_status == ExecutionStatus::Succeeded {
        return Ok(());
    }
    if result.execution_status == ExecutionStatus::Cancelled {
        return Err("cancelled".into());
    }
    if result.execution_status == ExecutionStatus::TimedOut {
        return Err(format!(
            "{what} exceeded its {:.0} second run budget",
            budget_s
        ));
    }
    Err(format!(
        "{what} failed ({:?}, exit {:?}); see {}: {}",
        result.execution_status,
        result.exit_code,
        log.display(),
        tail_job_output(result)
    ))
}

fn parse_preparation_receipt(bytes: &[u8], expected_request_sha: &str) -> Result<Value, String> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err("dose CAD preparation receipt exceeds the bounded JSON limit".into());
    }
    let receipt: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if receipt.get("schema").and_then(Value::as_str) != Some(PREP_RECEIPT_SCHEMA) {
        return Err("dose CAD preparation receipt has an unsupported schema".into());
    }
    if receipt.get("request_sha256").and_then(Value::as_str) != Some(expected_request_sha) {
        return Err(
            "dose CAD preparation receipt does not bind the exact Rust request bytes".into(),
        );
    }
    Ok(receipt)
}

fn parse_r2s_receipt(bytes: &[u8], expected_request_sha: &str) -> Result<Value, String> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err("OpenMC R2S receipt exceeds the bounded JSON limit".into());
    }
    let receipt: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if receipt.get("schema").and_then(Value::as_str) != Some(RECEIPT_SCHEMA)
        || receipt.get("status").and_then(Value::as_str) != Some("PASS")
        || receipt.get("request_sha256").and_then(Value::as_str) != Some(expected_request_sha)
    {
        return Err(
            "OpenMC R2S receipt schema, status, or exact request hash does not match".into(),
        );
    }
    Ok(receipt)
}

/// Exercise the external worker receipt parsers from the fuzz target without running OpenMC.
/// The fixtures are synthetic and this function never treats their hashes or values as evidence.
#[doc(hidden)]
pub fn fuzz_receipt_parsers(bytes: &[u8]) {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return;
    }
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return;
    };
    let expected = value
        .get("request_sha256")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Ok(receipt) = parse_preparation_receipt(bytes, expected) {
        for key in ["model_xml", "geometry_h5m", "component_material_map"] {
            let _ = path_from_receipt(&receipt, key);
        }
    }
    if let Ok(receipt) = parse_r2s_receipt(bytes, expected) {
        if let Some(points) = receipt.get("points").and_then(Value::as_array) {
            for point in points {
                let _ = value_f64(point, "mean_sv_h");
                let _ = value_f64(point, "standard_error_sv_h");
            }
        }
        if let Some(points) = receipt
            .get("d1s")
            .and_then(|d1s| d1s.get("points"))
            .and_then(Value::as_array)
        {
            for point in points {
                let _ = value_f64(point, "mean_sv_h");
                let _ = value_f64(point, "standard_error_sv_h");
            }
        }
    }
}

impl DosePhysicsManifest {
    pub fn read(path: &Path) -> Result<(Self, BTreeMap<String, ArtifactRef>, String), String> {
        let bytes = read_bounded(path, MAX_MANIFEST_BYTES)?;
        let hash = sha256(&bytes);
        let mut manifest: Self = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        let base = path.parent().unwrap_or(Path::new("."));
        if manifest.schema_version != DOSE_PHYSICS_SCHEMA
            && manifest.schema_version != DOSE_PHYSICS_SCHEMA_V1
        {
            return Err(format!(
                "dose physics manifest schema_version must be {DOSE_PHYSICS_SCHEMA} or {DOSE_PHYSICS_SCHEMA_V1}"
            ));
        }
        if manifest.schema_version == DOSE_PHYSICS_SCHEMA && manifest.activation_backend.is_none() {
            return Err("v0.2 dose physics manifest needs an explicit activation backend".into());
        }
        if manifest.schema_version == DOSE_PHYSICS_SCHEMA_V1 && manifest.chain_xml.is_none() {
            return Err("v0.1 OpenMC dose physics manifest needs its depletion chain".into());
        }
        let refs = [
            ("model_xml", &mut manifest.model_xml),
            ("geometry_h5m", &mut manifest.geometry_h5m),
            (
                "component_material_map",
                &mut manifest.component_material_map,
            ),
            ("cross_sections_xml", &mut manifest.cross_sections_xml),
        ];
        let mut verified = BTreeMap::new();
        for (name, reference) in refs {
            if !reference.path.is_absolute() {
                reference.path = base.join(&reference.path);
            }
            verify_ref(reference, name)?;
            verified.insert(name.to_string(), reference.clone());
        }
        if let Some(reference) = &mut manifest.chain_xml {
            if !reference.path.is_absolute() {
                reference.path = base.join(&reference.path);
            }
            verify_ref(reference, "chain XML")?;
            verified.insert("chain_xml".into(), reference.clone());
        }
        if let Some(backend) = &mut manifest.activation_backend {
            if backend.kind != "actinv_tendl_709" {
                return Err("unsupported activation backend in dose physics manifest".into());
            }
            for path in [&mut backend.data_dir, &mut backend.cache_dir] {
                if !path.is_absolute() {
                    *path = base.join(&*path);
                }
            }
            backend.data_dir = backend
                .data_dir
                .canonicalize()
                .map_err(|e| format!("ACTINV data directory: {e}"))?;
            if !backend.data_dir.is_dir() {
                return Err("ACTINV data_dir must be an existing directory".into());
            }
            if !backend.timeout_s.is_finite() || !(1.0..=86_400.0).contains(&backend.timeout_s) {
                return Err("ACTINV timeout_s must lie in 1..=86400".into());
            }
            for (name, reference) in [
                ("ACTINV executable", &mut backend.executable),
                ("ACTINV TENDL library", &mut backend.library_npz),
                ("ACTINV library index", &mut backend.library_index),
                ("ACTINV primary decay data", &mut backend.decay_primary),
            ] {
                if !reference.path.is_absolute() {
                    reference.path = base.join(&reference.path);
                }
                verify_ref(reference, name)?;
                verified.insert(name.to_string(), reference.clone());
            }
            let library_index: Value = serde_json::from_slice(&read_bounded(
                &backend.library_index.path,
                MAX_MANIFEST_BYTES,
            )?)
            .map_err(|e| format!("ACTINV library index: {e}"))?;
            if library_index.get("schema").and_then(Value::as_str) != Some("actinv-library-index-1")
                || library_index.get("groups").and_then(Value::as_str) != Some("fispact-709")
                || library_index.get("sha256_npz").and_then(Value::as_str)
                    != Some(backend.library_npz.sha256.as_str())
            {
                return Err("ACTINV library index must bind the selected FISPACT-709 NPZ".into());
            }
            if let Some(reference) = &mut backend.decay_fallback {
                if !reference.path.is_absolute() {
                    reference.path = base.join(&reference.path);
                }
                verify_ref(reference, "ACTINV fallback decay data")?;
                verified.insert("ACTINV fallback decay data".into(), reference.clone());
            }
            if backend.photon_group_boundaries_ev.len() < 3
                || backend
                    .photon_group_boundaries_ev
                    .windows(2)
                    .any(|w| w[1] <= w[0])
                || backend
                    .photon_group_boundaries_ev
                    .iter()
                    .any(|x| !x.is_finite())
            {
                return Err(
                    "ACTINV photon group boundaries must be finite and strictly ascending".into(),
                );
            }
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
        let actinv_backend = context.manifest.activation_backend.as_ref();
        let chain_ref = if actinv_backend.is_none() {
            Some(context.manifest.chain_xml.as_ref().ok_or_else(|| {
                format!("design {design}: the legacy OpenMC depletion route requires chain_xml")
            })?)
        } else {
            None
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
        let run_dir = run_dir.canonicalize().map_err(|e| e.to_string())?;
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
        let request = if let Some(backend) = actinv_backend {
            json!({
                "schema": "faris-maintenance-709-neutron-request/v0.1",
                "request_id": run_id, "stage": "neutron",
                "history_sha256": history_sha256,
                "history_artifact": history_ref,
                "source_schedules": group_schedules_ref,
                "model": context.manifest.model_xml,
                "geometry": context.manifest.geometry_h5m,
                "cross_sections": context.manifest.cross_sections_xml,
                "component_material_map": context.manifest.component_material_map,
                "activation_library_npz": backend.library_npz,
                "activation_library_index": backend.library_index,
                "activation_mesh": context.manifest.activation_mesh,
                "source_groups": source_groups,
                "event": {"design": design, "class": event.class, "component": event.component,
                          "replacement_k": event.k, "shutdown_s": event.start_s,
                          "cooling_s": cooling_grid_s, "location_id": location.id,
                          "location_sha256": location.sha256},
                "base_source_rate_n_s": context.neutron_rate_n_s,
                "photon_energy_edges_ev": backend.photon_group_boundaries_ev,
                "response_location_artifacts": {location.id.clone(): location_ref},
                "sampling": sampling,
            })
        } else {
            json!({
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
                "chain": chain_ref,
                "component_material_map": context.manifest.component_material_map,
                "activation_mesh": context.manifest.activation_mesh,
                "response_mesh": response_mesh,
                "sampling": sampling,
            })
        };
        let request_bytes = serde_json::to_vec_pretty(&request).map_err(|e| e.to_string())?;
        if request_bytes.len() > MAX_WORKER_REQUEST_BYTES {
            return Err("event-dose worker request exceeds 4 MiB".into());
        }
        let (receipt, request_ref, run_record_ref, additional_artifacts) = if actinv_backend
            .is_some()
        {
            let event_for_photon = json!({"design": design, "class": event.class, "component": event.component,
                "replacement_k": event.k, "shutdown_s": event.start_s, "location_id": location.id,
                "location_sha256": location.sha256, "cooling_s": cooling_grid_s});
            let staged = run_staged_actinv_dose(
                &self.python,
                &self.worker_script,
                &run_dir,
                &request,
                &context.manifest,
                cooling_grid_s,
                &response_mesh,
                &event_for_photon,
                &sampling,
                cancel,
            )?;
            (
                staged.receipt,
                staged.request_ref,
                staged.record_ref,
                staged.artifacts,
            )
        } else {
            let request_path = run_dir.join("r2s-event-request.json");
            write_exact(&request_path, &request_bytes)?;
            let request_ref = make_ref(&request_path)?;
            let log_path = run_dir.join("worker.log");
            let result = launch_bounded(
                &self.python,
                &self.worker_script,
                &request_path,
                &run_dir,
                &run_dir,
                &log_path,
                context.manifest.sampling.run_budget_s,
                cancel,
            )?;
            check_job_success(
                &result,
                &log_path,
                "OpenMC event R2S worker",
                context.manifest.sampling.run_budget_s,
            )?;
            let receipt_path = run_dir.join("r2s_event_receipt.json");
            let receipt = parse_r2s_receipt(
                &read_bounded(&receipt_path, MAX_MANIFEST_BYTES)?,
                &request_ref.sha256,
            )?;
            (
                receipt,
                request_ref,
                make_ref(&receipt_path)?,
                BTreeMap::new(),
            )
        };
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
            ("history".to_string(), history_ref.clone()),
            ("source_schedules".to_string(), group_schedules_ref.clone()),
            (
                "dose_physics_manifest".to_string(),
                context.manifest_artifact.clone(),
            ),
            (
                "cross_sections".to_string(),
                context.manifest.cross_sections_xml.clone(),
            ),
        ]);
        let mut input_refs = input_refs;
        input_refs.extend(additional_artifacts.clone());
        let mut input_sha: BTreeMap<String, String> = input_refs
            .iter()
            .map(|(k, v)| (k.clone(), v.sha256.clone()))
            .collect();
        input_sha.insert("event".into(), event_sha256.clone());
        let normalization = if actinv_backend.is_some() {
            "OpenMC mesh/material track-length response per source is divided by its sampled material-in-voxel volume and scaled by the authored base neutron source rate. Exact Rust interval source-rate ratios drive native ACTINV. ACTINV photons/s are absolute; OpenMC samples the combined emitter mixture and applies the summed ACTINV photon rate once to the per-source response. The task-location response is volume-averaged.".to_string()
        } else {
            "Physical source rate in neutrons/s is applied to each exact Rust irradiation interval; OpenMC R2S MeshSource photon strengths are applied by the fixed-source transport; response is volume-averaged over the authored one-voxel task location.".to_string()
        };
        let receipt_points = receipt
            .get("points")
            .and_then(Value::as_array)
            .ok_or("R2S receipt has no points")?;
        let d1s = receipt
            .get("d1s")
            .ok_or("dose receipt omits D1S comparison state")?;
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
            if r2s_value < 0.0 || r2s_se < 0.0 {
                return Err(
                    "dose estimate and Monte Carlo standard error must be nonnegative".into(),
                );
            }
            let mut r2s_hashes = input_sha.clone();
            r2s_hashes.insert(
                "cross_sections".into(),
                context.manifest.cross_sections_xml.sha256.clone(),
            );
            if let Some(chain_ref) = chain_ref {
                r2s_hashes.insert("chain".into(), chain_ref.sha256.clone());
            }
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
            let r2s_seed = r2s_point
                .get("seed")
                .and_then(Value::as_u64)
                .ok_or("photon receipt omits the actual seed used for this cooling point")?;
            let openmc_c_library_sha = r2s_point
                .pointer("/c_api_library/sha256")
                .and_then(Value::as_str)
                .ok_or("photon point omits loaded C API library hash")?;
            let r2s_random =
                format!("OpenMC-R2S:{r2s_stream}:{openmc_c_library_sha}:seed-{r2s_seed}");
            let mut r2s_artifact_hashes = artifact_hashes(&input_refs);
            r2s_artifact_hashes.extend([
                (
                    "cross_sections".into(),
                    context.manifest.cross_sections_xml.clone(),
                ),
                ("request".into(), request_ref.clone()),
                ("source_schedules".into(), group_schedules_ref.clone()),
                ("history".into(), history_ref.clone()),
                (
                    "dose_physics_manifest".into(),
                    context.manifest_artifact.clone(),
                ),
            ]);
            if let Some(chain_ref) = chain_ref {
                r2s_artifact_hashes.insert("chain".into(), chain_ref.clone());
            }
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
                if let Some(chain_ref) = chain_ref {
                    hashes.insert("chain".into(), chain_ref.sha256.clone());
                }
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
                    ("request".into(), request_ref.clone()),
                    ("source_schedules".into(), group_schedules_ref.clone()),
                    ("history".into(), history_ref.clone()),
                    (
                        "dose_physics_manifest".into(),
                        context.manifest_artifact.clone(),
                    ),
                ]);
                if let Some(chain_ref) = chain_ref {
                    artifact_refs.insert("chain".into(), chain_ref.clone());
                }
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
                r2s: r2s_estimate,
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

#[allow(clippy::too_many_arguments)]
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

fn value_f64(value: &Value, key: &str) -> Result<f64, String> {
    value
        .get(key)
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite() && *number >= 0.0)
        .ok_or_else(|| format!("worker field {key:?} must be a finite non-negative number"))
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

#[cfg(test)]
mod receipt_tests {
    use super::*;

    #[test]
    fn preparation_and_r2s_receipt_headers_bind_schema_status_and_request() {
        let preparation = br#"{"schema":"faris-maintenance-dose-cad-preparation-receipt/v0.1","request_sha256":"request-hash","model_xml":"model.xml","geometry_h5m":"geometry.h5m","component_material_map":"map.json"}"#;
        assert!(parse_preparation_receipt(preparation, "request-hash").is_ok());
        assert!(parse_preparation_receipt(preparation, "different-request").is_err());

        let r2s = br#"{"schema":"faris-maintenance-r2s-event-receipt/v0.1","status":"PASS","request_sha256":"request-hash","points":[{"mean_sv_h":0.01,"standard_error_sv_h":0.001}],"d1s":{"status":"NOT_EVALUATED","points":[]}}"#;
        assert!(parse_r2s_receipt(r2s, "request-hash").is_ok());
        assert!(parse_r2s_receipt(r2s, "different-request").is_err());
        let mut status = serde_json::from_slice::<Value>(r2s).unwrap();
        status["status"] = "PASS_WITH_WARNINGS".into();
        assert!(parse_r2s_receipt(&serde_json::to_vec(&status).unwrap(), "request-hash").is_err());
    }
}

fn validate_bounds(bounds: &MeshBounds, label: &str) -> Result<(), String> {
    if bounds.dimension.contains(&0)
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
        || !matches!(
            s.activation_groups.as_str(),
            "VITAMIN-J-42" | "CCFE-709" | "FISPACT-709"
        )
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

#[allow(clippy::too_many_arguments)]
fn launch_bounded(
    python: &Path,
    worker: &Path,
    request: &Path,
    out: &Path,
    working_directory: &Path,
    log: &Path,
    budget_s: f64,
    cancel: &Cancellation,
) -> Result<JobResult, String> {
    let python = resolve_python(python)?;
    let worker = worker.canonicalize().map_err(|e| e.to_string())?;
    let request = request.canonicalize().map_err(|e| e.to_string())?;
    let working_directory = working_directory
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !working_directory.is_dir() {
        return Err(format!(
            "worker directory is missing: {}",
            working_directory.display()
        ));
    }
    let out = if out.exists() {
        out.canonicalize().map_err(|e| e.to_string())?
    } else {
        let parent = out
            .parent()
            .ok_or("worker output has no parent directory")?;
        let name = out.file_name().ok_or("worker output has no file name")?;
        parent.canonicalize().map_err(|e| e.to_string())?.join(name)
    };
    if !out.starts_with(&working_directory) {
        return Err("worker output must stay inside its bounded working directory".into());
    }
    let log_name = log.file_name().ok_or("worker log has no file name")?;
    let log = log
        .parent()
        .ok_or("worker log has no parent directory")?
        .canonicalize()
        .map_err(|e| e.to_string())?
        .join(log_name);
    if !log.starts_with(&working_directory) {
        return Err("worker log must stay inside its bounded working directory".into());
    }
    // FARIS_PLASMA_SOURCE_PATH is the documented, explicit directory containing
    // openmc_plasma_source. No other inherited variables reach the scientific process.
    let plasma_path = std::env::var_os("FARIS_PLASMA_SOURCE_PATH")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty());
    let environment =
        worker_environment(&python, plasma_path.as_deref(), std::env::var_os("PATH"))?;
    let path = environment
        .iter()
        .find(|(name, _)| name == "PATH")
        .unwrap()
        .1
        .clone();
    let python_path = environment
        .iter()
        .find(|(name, _)| name == "PYTHONPATH")
        .unwrap()
        .1
        .clone();
    let spec = JobSpec {
        program: python.clone(),
        arguments: vec![
            worker.clone().into_os_string(),
            "--request".into(),
            request.clone().into_os_string(),
            "--out".into(),
            out.clone().into_os_string(),
        ],
        working_directory: working_directory.clone(),
        environment,
        timeout: Duration::from_secs_f64(budget_s),
        capture_limit_bytes: WORKER_LOG_LIMIT_BYTES,
        artifact_roots: Vec::new(),
        resource_limits: ResourceLimits::DEFAULT,
    };
    let result = run_job(&spec, cancel).map_err(|error| {
        format!(
            "could not run OpenMC worker {}: {error}",
            spec.program.display()
        )
    })?;
    let mut log_bytes = result.stdout.as_bytes().to_vec();
    if !log_bytes.is_empty() && !result.stderr.is_empty() {
        log_bytes.extend_from_slice(b"\n--- stderr ---\n");
    }
    log_bytes.extend_from_slice(result.stderr.as_bytes());
    write_exact(&log, &log_bytes)?;
    let metadata = serde_json::to_vec_pretty(&json!({
        "schema": "faris-maintenance-dose-worker-job/v1",
        "python": &python,
        "worker": &worker,
        "request": &request,
        "output_directory": &out,
        "working_directory": &working_directory,
        "environment": {
            "PATH": &path,
            "PYTHONPATH": &python_path,
            "PYTHONDONTWRITEBYTECODE": "1",
        },
        "job": &result,
    }))
    .map_err(|e| e.to_string())?;
    let job_stem = log
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or("worker log must have a UTF-8 file stem")?;
    write_exact(
        &log.with_file_name(format!("{job_stem}-job.json")),
        &metadata,
    )?;
    Ok(result)
}

fn resolve_python(selected: &Path) -> Result<PathBuf, String> {
    let has_directory = selected.components().count() > 1 || selected.is_absolute();
    let candidates = if has_directory {
        vec![std::path::absolute(selected).map_err(|e| e.to_string())?]
    } else {
        std::env::var_os("PATH")
            .map(|path| {
                std::env::split_paths(&path)
                    .map(|directory| directory.join(selected))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    for candidate in candidates {
        let candidate = std::path::absolute(candidate).map_err(|e| e.to_string())?;
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "selected Python {} is not a file on PATH",
        selected.display()
    ))
}

fn worker_environment(
    python: &Path,
    plasma_source_path: Option<&Path>,
    inherited_path: Option<OsString>,
) -> Result<Vec<(OsString, OsString)>, String> {
    if let Some(path) = plasma_source_path
        && (!path.is_absolute() || !path.join("openmc_plasma_source").is_dir())
    {
        return Err(format!(
            "FARIS_PLASMA_SOURCE_PATH must be an absolute directory holding openmc_plasma_source: {}",
            path.display()
        ));
    }
    let mut path_entries = Vec::new();
    if let Some(parent) = python.parent() {
        path_entries.push(parent.to_path_buf());
    }
    if let Some(inherited_path) = inherited_path {
        path_entries.extend(std::env::split_paths(&inherited_path));
    }
    let path = std::env::join_paths(path_entries).map_err(|e| e.to_string())?;
    let python_path =
        plasma_source_path.map_or_else(OsString::new, |path| path.as_os_str().to_owned());
    Ok(vec![
        ("PATH".into(), path),
        ("PYTHONPATH".into(), python_path),
        ("PYTHONDONTWRITEBYTECODE".into(), "1".into()),
    ])
}

fn tail_job_output(result: &JobResult) -> String {
    let mut text = format!("{}\n{}", result.stdout, result.stderr);
    if result.stdout_truncated || result.stderr_truncated {
        text.push_str("\n[captured output reached its configured size limit]");
    }
    text.chars().take(4000).collect()
}

#[cfg(test)]
mod bounded_worker_tests {
    use super::worker_environment;
    use std::{
        collections::BTreeMap,
        ffi::{OsStr, OsString},
        path::Path,
    };

    #[test]
    fn worker_environment_is_an_explicit_allowlist_with_selected_python_and_helper_paths() {
        let helper_root = tempfile::tempdir().unwrap();
        std::fs::create_dir(helper_root.path().join("openmc_plasma_source")).unwrap();
        let selected_python = Path::new("/selected/env/bin/python");
        let env = worker_environment(
            selected_python,
            Some(helper_root.path()),
            Some(OsString::from("/usr/bin:/bin")),
        )
        .unwrap();
        let values: BTreeMap<_, _> = env.into_iter().collect();
        assert_eq!(values.len(), 3);
        assert_eq!(
            values.get(OsStr::new("PYTHONPATH")).unwrap().as_os_str(),
            helper_root.path().as_os_str()
        );
        assert_eq!(
            values
                .get(OsStr::new("PYTHONDONTWRITEBYTECODE"))
                .unwrap()
                .as_os_str(),
            OsStr::new("1")
        );
        let paths =
            std::env::split_paths(values.get(OsStr::new("PATH")).unwrap()).collect::<Vec<_>>();
        assert_eq!(paths[0], Path::new("/selected/env/bin").to_path_buf());
        assert_eq!(
            paths[1..],
            [
                Path::new("/usr/bin").to_path_buf(),
                Path::new("/bin").to_path_buf()
            ]
        );
        assert!(!values.contains_key(OsStr::new("HOME")));
        assert!(!values.contains_key(OsStr::new("OPENMC_CROSS_SECTIONS")));
    }

    #[test]
    fn worker_environment_rejects_an_unconfigured_or_wrong_plasma_package_root() {
        let helper_root = tempfile::tempdir().unwrap();
        let error = worker_environment(
            Path::new("/selected/env/bin/python"),
            Some(helper_root.path()),
            None,
        )
        .unwrap_err();
        assert!(error.contains("holding openmc_plasma_source"));
    }
}
