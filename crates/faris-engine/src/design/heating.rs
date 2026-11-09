// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! Unit conversion for coupled design heating. The OpenMC score remains raw eV per source
//! neutron until this boundary; sampled volume uncertainty is recorded separately from MC SE.

use super::transport::read_bounded;
use super::{
    CAD_ADDRESS_SPACE, DesignError, DesignIdentity, ImportReport, Launched, MAX_JSON_BYTES,
    ToolPaths, io_error, launch_python_with_lines, sha256_file, sha256_hex,
};
use crate::jobs::{Cancellation, ResourceLimits};
use crate::transport::{D_T_ENERGY_PER_REACTION_EV, reaction_rate_per_s};
use faris_model::design::{self, Role};
use faris_model::validation::{Output, Quantity, ValidationMarks, calculated, not_calculated};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};

pub const ELEMENTARY_CHARGE_C: f64 = crate::transport::ELEMENTARY_CHARGE_C;

const REQUEST_SCHEMA: &str = "faris-design-transport-request/v3";
const RESULT_SCHEMA: &str = "faris-design-heating/v1";
const MIN_BATCHES: u64 = 30;
const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const VOLUME_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const STARTUP_ALLOWANCE: Duration = Duration::from_secs(30 * 60);

#[derive(Clone)]
pub struct DesignHeatingOptions {
    pub check_report: PathBuf,
    pub out_dir: PathBuf,
    pub peak_voxel_m: [f64; 2],
    pub particles_per_batch: u64,
    pub max_histories: u64,
    pub max_seconds: u64,
    pub seed: u64,
    pub threads: u32,
    pub cancellation: Cancellation,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct HeatingScore {
    pub mean_ev_per_source: f64,
    pub standard_error_ev_per_source: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct HeatingPeak {
    pub energy_ev_per_source: f64,
    pub sampling_se_ev_per_source: Option<f64>,
    pub volume_m3: f64,
    pub r_m: [f64; 2],
    pub z_m: [f64; 2],
    pub pieces_ranked: u64,
    pub volume_estimator: VolumeEstimator,
    pub closure_residual: HeatingScore,
    pub power_density_kw_m3: f64,
    pub sampling_se_kw_m3: Option<f64>,
    pub volume_sampling_relative_error: Option<f64>,
    pub component_scores: BTreeMap<String, HeatingScore>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct VolumeEstimator {
    pub method: String,
    pub rays: u64,
    /// `None` means the OpenMC volume adapter did not provide an estimator SE.
    pub sampling_relative_error: Option<f64>,
    pub sampling_uncertainty_reported: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SolidHeating {
    pub solid_id: String,
    pub neutron: HeatingScore,
    pub photon: HeatingScore,
    pub electron: HeatingScore,
    pub positron: HeatingScore,
    pub total: HeatingScore,
    pub closure_residual: HeatingScore,
    pub volume_estimator: VolumeEstimator,
    pub peak: Option<HeatingPeak>,
    pub voxels: Vec<VoxelHeating>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct VoxelHeating {
    pub r_m: [f64; 2],
    pub z_m: [f64; 2],
    pub volume_m3: f64,
    /// Raw eV per source neutron for each particle scope, with the batch SE beside it.
    pub component_scores: BTreeMap<String, HeatingScore>,
    pub closure_residual: HeatingScore,
    pub is_peak: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DesignHeatingReport {
    pub schema_version: String,
    pub status: String,
    pub design: DesignIdentity,
    pub check_report_sha256: String,
    pub h5m_sha256: String,
    pub cross_sections_sha256: String,
    pub run_directory: String,
    pub openmc: Option<String>,
    pub source_rate_per_s: f64,
    pub energy_per_reaction_ev: f64,
    pub fusion_power_mw: f64,
    pub histories: u64,
    pub c_api_terminal_status: Option<i32>,
    pub lost_particles: Option<u64>,
    pub stop_reason: String,
    pub elapsed_seconds: f64,
    pub seed: u64,
    pub threads: u32,
    pub data_identity: Option<serde_json::Value>,
    pub validation_scope: String,
    pub solids: Vec<SolidHeating>,
    pub why: Option<String>,
    pub next_step: Option<String>,
    pub notices: Vec<String>,
    #[serde(default, skip_serializing_if = "ValidationMarks::is_empty")]
    pub validation: ValidationMarks,
}

pub const DESIGN_HEATING_OUTPUTS: &[Output] = &[
    calculated("energy_ev_per_source", Quantity::Heating),
    calculated("sampling_se_ev_per_source", Quantity::Heating),
    calculated("mean_ev_per_source", Quantity::Heating),
    calculated("standard_error_ev_per_source", Quantity::Heating),
    calculated("power_density_kw_m3", Quantity::Heating),
    calculated("sampling_se_kw_m3", Quantity::Heating),
    calculated("peak_neutron_ev_per_source", Quantity::Heating),
    calculated("peak_photon_ev_per_source", Quantity::Heating),
    calculated("peak_electron_ev_per_source", Quantity::Heating),
    calculated("peak_positron_ev_per_source", Quantity::Heating),
    calculated("peak_total_ev_per_source", Quantity::Heating),
    calculated("neutron", Quantity::Heating),
    calculated("photon", Quantity::Heating),
    calculated("electron", Quantity::Heating),
    calculated("positron", Quantity::Heating),
    calculated("total", Quantity::Heating),
    calculated("closure_residual", Quantity::Heating),
    not_calculated(
        "volume_m3",
        "A sampled geometric averaging volume, not nuclear heating.",
    ),
    not_calculated(
        "volume_sampling_relative_error",
        "A geometric volume-estimator uncertainty, separate from heating Monte Carlo error.",
    ),
    not_calculated(
        "r_m",
        "A sampled geometric voxel coordinate, not nuclear heating.",
    ),
    not_calculated(
        "z_m",
        "A sampled geometric voxel coordinate, not nuclear heating.",
    ),
    not_calculated(
        "pieces_ranked",
        "A geometric estimator count, not nuclear heating.",
    ),
    not_calculated(
        "rays",
        "A geometric volume-estimator sample count, not nuclear heating.",
    ),
    not_calculated(
        "shell_count",
        "A nuclear-data identity count, not a heating response.",
    ),
    not_calculated(
        "source_rate_per_s",
        "A source-rate conversion from authored fusion power, not a heating response.",
    ),
    not_calculated(
        "fusion_power_mw",
        "An authored design input, not a heating response.",
    ),
    not_calculated(
        "energy_per_reaction_ev",
        "The D-T source normalization convention, not a heating response.",
    ),
    not_calculated("histories", "A run count, not a heating response."),
    not_calculated(
        "c_api_terminal_status",
        "An OpenMC execution terminal code, not a heating response.",
    ),
    not_calculated(
        "lost_particles",
        "A transport quality count, not a heating response.",
    ),
    not_calculated(
        "stop_reason",
        "A sampling budget endpoint, not a heating response.",
    ),
    not_calculated("seed", "A sampling input, not a heating response."),
    not_calculated("threads", "A run configuration, not a heating response."),
    not_calculated("elapsed_seconds", "A run duration, not a heating response."),
];

/// A heating score normalized once to operating power density.
#[derive(Clone, Debug, PartialEq)]
pub struct HeatingEstimate {
    pub energy_ev_per_source: f64,
    pub sampling_se_ev_per_source: Option<f64>,
    pub source_rate_per_s: f64,
    pub volume_m3: f64,
    pub power_density_kw_m3: f64,
    pub sampling_se_kw_m3: Option<f64>,
    /// Relative uncertainty of the independent geometry volume estimator, kept separate from
    /// the Monte Carlo tally error. It is `None` when the volume was authored rather than sampled.
    pub volume_sampling_relative_error: Option<f64>,
}

#[derive(Clone, Debug, Deserialize)]
struct WorkerScore {
    mean_ev_per_source: f64,
    standard_error_ev_per_source: f64,
    #[serde(default)]
    within_3se: Option<bool>,
    #[serde(default)]
    roundoff_tolerance_ev_per_source: Option<f64>,
}

#[derive(Clone, Debug, Deserialize)]
struct WorkerPeak {
    mean: f64,
    standard_error: Option<f64>,
    volume_cm3: f64,
    r_cm: [f64; 2],
    z_cm: [f64; 2],
    pieces_ranked: u64,
    volume_estimator: VolumeEstimator,
    closure_residual: WorkerScore,
    components: BTreeMap<String, WorkerScore>,
}

#[derive(Clone, Debug, Deserialize)]
struct WorkerSolid {
    solid_id: String,
    neutron: WorkerScore,
    photon: WorkerScore,
    electron: WorkerScore,
    positron: WorkerScore,
    total: WorkerScore,
    closure_residual: WorkerScore,
    peak: Option<WorkerPeak>,
    voxel_scores: Vec<WorkerVoxel>,
}

#[derive(Clone, Debug, Deserialize)]
struct WorkerVoxel {
    r_cm: [f64; 2],
    z_cm: [f64; 2],
    volume_cm3: f64,
    components: BTreeMap<String, WorkerScore>,
    closure_residual: WorkerScore,
    is_peak: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct WorkerResult {
    schema: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    why: Option<String>,
    #[serde(default)]
    next_step: Option<String>,
    #[serde(default)]
    openmc: Option<String>,
    #[serde(default)]
    h5m_sha256: Option<String>,
    #[serde(default)]
    cross_sections_sha256: Option<String>,
    #[serde(default)]
    histories: u64,
    #[serde(default)]
    c_api_terminal_status: Option<i32>,
    #[serde(default)]
    lost_particles: Option<u64>,
    #[serde(default)]
    stop_reason: String,
    #[serde(default)]
    solids: Vec<WorkerSolid>,
    #[serde(default)]
    data_identity: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize)]
struct WorkerPreflight {
    schema: String,
    status: String,
    #[serde(default)]
    why: Option<String>,
    #[serde(default)]
    next_step: Option<String>,
    h5m_sha256: String,
    cross_sections_sha256: String,
    #[serde(default)]
    data_identity: Option<serde_json::Value>,
}

/// Fuzz the private heating worker and preflight JSON readers from the shared design helper entry.
#[doc(hidden)]
pub(super) fn fuzz_worker_outputs(bytes: &[u8]) {
    let _ = serde_json::from_slice::<WorkerResult>(bytes);
    let _ = serde_json::from_slice::<WorkerPreflight>(bytes);
}

/// Convert eV/source to kW/m³. The source rate and volume are SI; callers must not normalize again.
pub fn power_density_kw_m3(
    energy_ev_per_source: f64,
    source_rate_per_s: f64,
    volume_m3: f64,
) -> Result<f64, DesignError> {
    if !energy_ev_per_source.is_finite() || energy_ev_per_source < 0.0 {
        return Err(DesignError::Tool(
            "heating score must be finite and non-negative".into(),
        ));
    }
    if !source_rate_per_s.is_finite() || source_rate_per_s <= 0.0 {
        return Err(DesignError::Tool(
            "heating source rate must be finite and positive".into(),
        ));
    }
    if !volume_m3.is_finite() || volume_m3 <= 0.0 {
        return Err(DesignError::Tool(
            "heating averaging volume must be finite and positive".into(),
        ));
    }
    let result =
        energy_ev_per_source * source_rate_per_s * ELEMENTARY_CHARGE_C / volume_m3 / 1000.0;
    if !result.is_finite() {
        return Err(DesignError::Tool(
            "heating power-density conversion overflowed or became non-finite".into(),
        ));
    }
    Ok(result)
}

/// Normalize a score and its MC sampling SE; volume uncertainty is attached as its own field.
pub fn normalize_heating_ev_per_source(
    energy_ev_per_source: f64,
    sampling_se_ev_per_source: Option<f64>,
    source_rate_per_s: f64,
    volume_m3: f64,
    volume_sampling_relative_error: Option<f64>,
) -> Result<HeatingEstimate, DesignError> {
    if sampling_se_ev_per_source.is_some_and(|se| !se.is_finite() || se < 0.0) {
        return Err(DesignError::Tool(
            "heating sampling error must be finite and non-negative".into(),
        ));
    }
    if volume_sampling_relative_error.is_some_and(|se| !se.is_finite() || se < 0.0) {
        return Err(DesignError::Tool(
            "volume sampling error must be finite and non-negative".into(),
        ));
    }
    let density_kw_m3 = power_density_kw_m3(energy_ev_per_source, source_rate_per_s, volume_m3)?;
    let sampling_se_kw_m3 = sampling_se_ev_per_source
        .map(|se| power_density_kw_m3(se, source_rate_per_s, volume_m3))
        .transpose()?;
    Ok(HeatingEstimate {
        energy_ev_per_source,
        sampling_se_ev_per_source,
        source_rate_per_s,
        volume_m3,
        power_density_kw_m3: density_kw_m3,
        sampling_se_kw_m3,
        volume_sampling_relative_error,
    })
}

/// Run an independent coupled heating calculation on a checked CAD model. This operation has its
/// own request and worker phase; it never adds heating to the neutron-flux cutoff run.
pub fn design_heating(
    design_path: &Path,
    tools: &ToolPaths,
    options: &DesignHeatingOptions,
) -> Result<DesignHeatingReport, DesignError> {
    if options.particles_per_batch == 0
        || options.max_histories / options.particles_per_batch.max(1) < MIN_BATCHES
        || options.max_seconds == 0
        || options.threads == 0
        || options.threads > 32
        || options
            .peak_voxel_m
            .iter()
            .any(|d| !d.is_finite() || *d < 0.01)
    {
        return Err(DesignError::Tool(
            "heating needs at least 30 batches, a positive time limit, 1 to 32 threads, and peak voxels at least 0.01 m".into(),
        ));
    }
    let design_bytes = read_bounded(design_path)?;
    let design_sha256 = sha256_hex(&design_bytes);
    let design = design::parse_design(&design_bytes).design.ok_or_else(|| {
        DesignError::Tool("the design file does not parse; run `faris design check`".into())
    })?;
    let report_bytes = read_bounded(&options.check_report)?;
    let check_report: ImportReport = serde_json::from_slice(&report_bytes).map_err(|e| {
        DesignError::Tool(format!(
            "{} is not a design check report: {e}",
            options.check_report.display()
        ))
    })?;
    super::transport::accept_check_report(&check_report, &design_sha256)?;
    let conversion =
        check_report.geometry.conversion.as_ref().ok_or_else(|| {
            DesignError::Tool("the design check report records no conversion".into())
        })?;
    let h5m = super::transport::converted_model(&conversion.directory)?;
    if !h5m.is_file() || !sha256_file(&h5m)?.eq_ignore_ascii_case(&conversion.h5m_sha256) {
        return Err(DesignError::Tool(format!(
            "{} is missing or differs from the checked conversion; rerun `faris design check` and keep its work directory",
            h5m.display()
        )));
    }
    super::transport::refuse_unsupported_source(&design)?;
    let run_dir = &options.out_dir;
    std::fs::create_dir(run_dir).map_err(|e| {
        io_error(
            run_dir,
            format!("{e} (the heating run directory must be new; nothing is overwritten)"),
        )
    })?;
    let run_dir = run_dir.canonicalize().map_err(|e| io_error(run_dir, e))?;
    let fusion_power_mw = match design.plasma.fusion_power_mw {
        Some(power) if power.is_finite() && power > 0.0 => power,
        _ => {
            return Err(DesignError::Tool(
                "the design's plasma block has no positive fusion power".into(),
            ));
        }
    };
    let source_rate = reaction_rate_per_s(fusion_power_mw, D_T_ENERGY_PER_REACTION_EV);
    if !source_rate.is_finite() || source_rate <= 0.0 {
        return Err(DesignError::Tool(
            "the fusion power does not yield a finite positive D-T source rate".into(),
        ));
    }
    let design_identity = DesignIdentity {
        path: design_path.display().to_string(),
        sha256: design_sha256,
        id: Some(design.id.clone()),
    };
    let mut report = DesignHeatingReport {
        schema_version: RESULT_SCHEMA.into(),
        status: "NOT_EVALUATED".into(),
        design: design_identity,
        check_report_sha256: sha256_hex(&report_bytes),
        h5m_sha256: conversion.h5m_sha256.clone(),
        cross_sections_sha256: String::new(),
        run_directory: run_dir.display().to_string(),
        openmc: None,
        source_rate_per_s: source_rate,
        energy_per_reaction_ev: D_T_ENERGY_PER_REACTION_EV,
        fusion_power_mw,
        histories: 0,
        c_api_terminal_status: None,
        lost_particles: None,
        stop_reason: "not_started".into(),
        elapsed_seconds: 0.0,
        seed: options.seed,
        threads: options.threads,
        data_identity: None,
        validation_scope: "No physical validation claim. Calculated values are Monte Carlo estimates scoped to the authored winding-pack solids and the checked CAD conversion.".into(),
        solids: Vec::new(),
        why: None,
        next_step: None,
        notices: vec!["Heating currently stops at its configured history or time budget; target-relative-error stopping is not implemented yet (NUC-043 remains open).".into()],
        validation: ValidationMarks::default(),
    };
    let record_not_evaluated = |mut report: DesignHeatingReport,
                                why: String,
                                next_step: String|
     -> Result<DesignHeatingReport, DesignError> {
        report.why = Some(why);
        report.next_step = Some(next_step);
        write_report(&run_dir, &report)?;
        Ok(report)
    };
    let response: Vec<_> = design
        .solids
        .iter()
        .filter(|solid| solid.role == Some(Role::TfMagnet))
        .filter(|solid| solid.classification == Some(design::SolidClassification::WindingPack))
        .filter(|solid| solid.material_id.as_deref().is_some_and(|id| id != "void"))
        .map(|solid| (solid.id.as_str(), solid.fingerprint.cad_volume_m3))
        .collect();
    if response.is_empty() {
        return record_not_evaluated(
            report,
            "No solid has an explicit winding-pack classification and a non-void material.".into(),
            "Set classification: winding_pack only on CAD solids that represent the winding pack, then rerun design check.".into(),
        );
    }
    if response.len() > 64 {
        return record_not_evaluated(
            report,
            format!("{} authored winding-pack solids exceed the 64-material sampled-volume estimator bound.", response.len()),
            "Partition the CAD design or explicitly narrow the winding-pack assignments, then rerun heating.".into(),
        );
    }
    let chamber = match design.plasma.chamber_solid_id.as_deref() {
        Some(chamber) => chamber,
        None => {
            return record_not_evaluated(
                report,
                "The design's plasma block names no chamber solid.".into(),
                "Author the chamber solid identity in the plasma block and rerun design check."
                    .into(),
            );
        }
    };
    let python = match tools.openmc_python.as_deref() {
        Some(path) => path,
        None => {
            return record_not_evaluated(
                report,
                "No OpenMC interpreter is configured.".into(),
                "Pass --openmc-python with an OpenMC 0.15.3 or later environment.".into(),
            );
        }
    };
    let cross_sections = match tools.cross_sections.as_deref() {
        Some(path) => path,
        None => {
            return record_not_evaluated(
                report,
                "No nuclear data library is configured.".into(),
                "Pass --cross-sections naming the checked cross_sections.xml.".into(),
            );
        }
    };
    let plasma_source = match tools.plasma_source_path.as_deref() {
        Some(path) => path,
        None => {
            return record_not_evaluated(
                report,
                "No openmc_plasma_source package path is configured.".into(),
                "Set FARIS_PLASMA_SOURCE_PATH or pass --plasma-source-path.".into(),
            );
        }
    };
    let cross_sections = cross_sections
        .canonicalize()
        .map_err(|e| io_error(cross_sections, e))?;
    let cross_sections_sha256 = sha256_file(&cross_sections)?;
    report.cross_sections_sha256 = cross_sections_sha256;
    if check_report
        .material_audit
        .as_ref()
        .and_then(|audit| audit.cross_sections_sha256.as_deref())
        .is_some_and(|audited| !audited.eq_ignore_ascii_case(&report.cross_sections_sha256))
    {
        report.notices.push(
            "This heating run used cross-section data different from the material audit recorded by design check.".into(),
        );
    }
    let cross_sections = cross_sections.display().to_string();
    let request = serde_json::json!({
        "schema": REQUEST_SCHEMA,
        "heating_mode": true,
        "h5m": h5m,
        "cross_sections": cross_sections,
        "solids": super::transport::solid_entries(&design, &check_report)?,
        "materials": super::transport::material_entries(&design, &check_report)?,
        "plasma": design.plasma,
        "chamber_solid_id": chamber,
        "response": {
            "solid_ids": response.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            "energy_min_ev": 0.0,
            "peak_voxel_cm": options.peak_voxel_m.map(|v| v * 100.0),
        },
        "sampling": {
            "particles_per_batch": options.particles_per_batch,
            "min_batches": MIN_BATCHES,
            "max_batches": options.max_histories / options.particles_per_batch,
            "max_seconds": options.max_seconds,
            "seed": options.seed,
            "threads": options.threads,
        },
    });
    let script = run_dir.join("design_transport.py");
    let request_path = run_dir.join("request.json");
    let tmp = run_dir.join("tmp");
    std::fs::create_dir_all(&tmp).map_err(|e| io_error(&tmp, e))?;
    for (path, text) in [
        (script.clone(), super::DESIGN_TRANSPORT_WORKER),
        (
            run_dir.join("window_generator.py"),
            super::WINDOW_GENERATOR_HELPER,
        ),
        (
            run_dir.join("batch_statistics.py"),
            super::BATCH_STATISTICS_HELPER,
        ),
    ] {
        std::fs::write(&path, text).map_err(|e| io_error(&path, e))?;
    }
    std::fs::write(
        &request_path,
        serde_json::to_vec_pretty(&request).unwrap_or_default(),
    )
    .map_err(|e| io_error(&request_path, e))?;
    let plasma_source = plasma_source
        .canonicalize()
        .map_err(|e| io_error(plasma_source, e))?;
    let python_path = std::env::join_paths([plasma_source.as_os_str()])
        .map_err(|e| DesignError::Tool(format!("invalid plasma source search path: {e}")))?;
    let phase = |name: &str, timeout: Duration| -> Result<f64, DesignError> {
        let arguments: Vec<OsString> = vec![
            script.clone().into_os_string(),
            "--phase".into(),
            name.into(),
            "--request".into(),
            request_path.clone().into_os_string(),
            "--work-dir".into(),
            run_dir.clone().into_os_string(),
        ];
        let environment = vec![
            ("TMPDIR".into(), tmp.clone().into_os_string()),
            ("HOME".into(), run_dir.as_os_str().to_owned()),
            ("OMP_NUM_THREADS".into(), options.threads.to_string().into()),
            ("PYTHONPATH".into(), python_path.clone()),
        ];
        match launch_python_with_lines(
            python,
            arguments,
            &run_dir,
            timeout,
            &format!("design heating {name} step"),
            &options.cancellation,
            environment,
            ResourceLimits::with_address_space(CAD_ADDRESS_SPACE),
            &|_| {},
        )? {
            Launched::Succeeded(seconds) => Ok(seconds),
            Launched::Failed(text) => Err(DesignError::Tool(text)),
        }
    };
    let mut elapsed = phase("heating_preflight", PREFLIGHT_TIMEOUT)?;
    let preflight_path = run_dir.join("solver/heating-preflight.json");
    let preflight_bytes = super::transport::read_bounded(&preflight_path)?;
    let preflight: WorkerPreflight = serde_json::from_slice(&preflight_bytes).map_err(|e| {
        DesignError::Tool(format!(
            "{}: invalid heating data preflight: {e}",
            preflight_path.display()
        ))
    })?;
    if preflight.schema != "faris-design-heating-preflight/v1"
        || !preflight
            .h5m_sha256
            .eq_ignore_ascii_case(&report.h5m_sha256)
        || !preflight
            .cross_sections_sha256
            .eq_ignore_ascii_case(&report.cross_sections_sha256)
    {
        return Err(DesignError::Tool(
            "the coupled-heating preflight does not match the checked geometry or nuclear-data identity".into(),
        ));
    }
    report.data_identity = preflight.data_identity;
    if preflight.status == "NOT_EVALUATED" {
        report.elapsed_seconds = elapsed;
        report.why = preflight.why;
        report.next_step = preflight.next_step;
        return write_report(&run_dir, &report).map(|()| report);
    }
    if preflight.status != "READY" {
        return Err(DesignError::Tool(format!(
            "the coupled-heating preflight returned unknown status {:?}",
            preflight.status
        )));
    }
    elapsed += phase("volumes", VOLUME_TIMEOUT)?;
    elapsed += phase(
        "heating",
        Duration::from_secs(options.max_seconds) + STARTUP_ALLOWANCE,
    )?;
    let result_path = run_dir.join("solver/heating-result.json");
    let worker_bytes = super::transport::read_bounded(&result_path)?;
    let worker: WorkerResult = serde_json::from_slice(&worker_bytes).map_err(|e| {
        DesignError::Tool(format!(
            "{}: invalid heating worker report: {e}",
            result_path.display()
        ))
    })?;
    if worker.schema != RESULT_SCHEMA {
        return Err(DesignError::Tool(format!(
            "the heating worker report has schema {:?}, expected {RESULT_SCHEMA}",
            worker.schema
        )));
    }
    if worker.status.as_deref() == Some("NOT_EVALUATED") {
        report.elapsed_seconds = elapsed;
        report.histories = worker.histories;
        report.c_api_terminal_status = worker.c_api_terminal_status;
        report.lost_particles = worker.lost_particles;
        report.why = worker.why;
        report.next_step = worker.next_step;
        return write_report(&run_dir, &report).map(|()| report);
    }
    if !matches!(
        worker.status.as_deref(),
        Some("CALCULATED" | "INCONCLUSIVE")
    ) || !matches!(worker.stop_reason.as_str(), "history_limit" | "time_limit")
        || worker
            .h5m_sha256
            .as_deref()
            .is_none_or(|hash| !hash.eq_ignore_ascii_case(&report.h5m_sha256))
        || worker
            .cross_sections_sha256
            .as_deref()
            .is_none_or(|hash| !hash.eq_ignore_ascii_case(&report.cross_sections_sha256))
        || worker.histories < MIN_BATCHES * options.particles_per_batch
        || worker.histories > options.max_histories
        || !worker.histories.is_multiple_of(options.particles_per_batch)
        || worker.c_api_terminal_status.is_none()
        || worker.lost_particles.is_none()
        || !valid_terminal_status(
            &worker,
            options.max_histories / options.particles_per_batch,
            options.particles_per_batch,
        )
        || (worker.status.as_deref() == Some("CALCULATED")
            && worker
                .lost_particles
                .is_some_and(|lost| lost as f64 / worker.histories as f64 > 1.0e-6))
        || (worker.status.as_deref() == Some("INCONCLUSIVE") && worker.why.is_none())
        || worker.solids.len() != response.len()
        || worker
            .solids
            .iter()
            .zip(&response)
            .any(|(got, (expected, _))| got.solid_id != *expected)
    {
        return Err(DesignError::Tool(
            "the heating worker result does not match the checked request or input identities"
                .into(),
        ));
    }
    if worker.solids.iter().any(|solid| solid.peak.is_none()) {
        return record_not_evaluated(
            report,
            "The worker could not resolve a positive leading winding-pack voxel from the sampled heating scores.".into(),
            "Increase the history limit or use a coarser peak voxel, then rerun the separate heating calculation.".into(),
        );
    }
    let finite_score = |score: &WorkerScore| {
        score.mean_ev_per_source.is_finite()
            && score.mean_ev_per_source >= 0.0
            && score.standard_error_ev_per_source.is_finite()
            && score.standard_error_ev_per_source >= 0.0
    };
    let finite_closure = |score: &WorkerScore| {
        score.mean_ev_per_source.is_finite()
            && score.standard_error_ev_per_source.is_finite()
            && score.standard_error_ev_per_source >= 0.0
            && score.within_3se.is_some()
            && score
                .roundoff_tolerance_ev_per_source
                .is_some_and(|v| v.is_finite() && v >= 0.0)
    };
    if worker.solids.iter().any(|solid| {
        !finite_score(&solid.neutron)
            || !finite_score(&solid.photon)
            || !finite_score(&solid.electron)
            || !finite_score(&solid.positron)
            || !finite_score(&solid.total)
            || !worker_component_maps_valid(std::slice::from_ref(solid))
            || finite_abs_sum(&[
                solid.neutron.mean_ev_per_source,
                solid.photon.mean_ev_per_source,
                solid.electron.mean_ev_per_source,
                solid.positron.mean_ev_per_source,
                solid.total.mean_ev_per_source,
            ])
            .is_none()
            || !finite_closure(&solid.closure_residual)
            || !closure_mean_matches(
                solid.closure_residual.mean_ev_per_source,
                solid.total.mean_ev_per_source,
                &[
                    solid.neutron.mean_ev_per_source,
                    solid.photon.mean_ev_per_source,
                    solid.electron.mean_ev_per_source,
                    solid.positron.mean_ev_per_source,
                ],
            )
            || solid.voxel_scores.is_empty()
            || solid
                .voxel_scores
                .iter()
                .filter(|voxel| voxel.is_peak)
                .count()
                != 1
            || solid.voxel_scores.iter().any(|voxel| {
                !voxel.volume_cm3.is_finite()
                    || voxel.volume_cm3 <= 0.0
                    || voxel
                        .r_cm
                        .iter()
                        .chain(voxel.z_cm.iter())
                        .any(|v| !v.is_finite())
                    || !finite_closure(&voxel.closure_residual)
                    || !closure_mean_matches(
                        voxel.closure_residual.mean_ev_per_source,
                        voxel
                            .components
                            .get("total")
                            .expect("required score is checked")
                            .mean_ev_per_source,
                        &["neutron", "photon", "electron", "positron"].map(|name| {
                            voxel
                                .components
                                .get(name)
                                .expect("required score is checked")
                                .mean_ev_per_source
                        }),
                    )
            })
            || solid.peak.as_ref().is_none_or(|peak| {
                !peak.mean.is_finite()
                    || peak.mean < 0.0
                    || peak
                        .standard_error
                        .is_some_and(|se| !se.is_finite() || se < 0.0)
                    || !peak.volume_cm3.is_finite()
                    || peak.volume_cm3 <= 0.0
                    || peak.volume_estimator.rays == 0
                    || peak.volume_estimator.sampling_uncertainty_reported
                        != peak.volume_estimator.sampling_relative_error.is_some()
                    || !finite_closure(&peak.closure_residual)
                    || !closure_mean_matches(
                        peak.closure_residual.mean_ev_per_source,
                        peak.components
                            .get("total")
                            .expect("required score is checked")
                            .mean_ev_per_source,
                        &["neutron", "photon", "electron", "positron"].map(|name| {
                            peak.components
                                .get(name)
                                .expect("required score is checked")
                                .mean_ev_per_source
                        }),
                    )
            })
    }) {
        return Err(DesignError::Tool(
            "the heating worker returned a non-finite score or no peak volume".into(),
        ));
    }
    let closure_failed = worker.solids.iter().any(|solid| {
        let solid_scale = finite_abs_sum(&[
            solid.total.mean_ev_per_source,
            solid.neutron.mean_ev_per_source,
            solid.photon.mean_ev_per_source,
            solid.electron.mean_ev_per_source,
            solid.positron.mean_ev_per_source,
        ])
        .expect("finite top-level score scale was validated");
        !closure_within_3se(&solid.closure_residual, solid_scale)
            || solid.voxel_scores.iter().any(|voxel| {
                let scale = component_score_scale(&voxel.components)
                    .expect("exact finite voxel component set was validated");
                !closure_within_3se(&voxel.closure_residual, scale)
            })
            || solid.peak.as_ref().is_some_and(|peak| {
                let scale = component_score_scale(&peak.components)
                    .expect("exact finite peak component set was validated");
                !closure_within_3se(&peak.closure_residual, scale)
            })
    });
    let peak_unconverged = worker.solids.iter().any(|solid| {
        solid
            .peak
            .as_ref()
            .is_none_or(|peak| !heating_peak_is_converged(peak.mean, peak.standard_error))
    });
    if (worker.status.as_deref() == Some("CALCULATED") && (closure_failed || peak_unconverged))
        || (worker.status.as_deref() == Some("INCONCLUSIVE") && worker.why.is_none())
    {
        return Err(DesignError::Tool(
            "the heating worker status does not agree with its preserved closure and peak-uncertainty diagnostics"
                .into(),
        ));
    }
    let mut solids = Vec::new();
    for worker_solid in worker.solids {
        let peak = worker_solid.peak.expect("checked above");
        let peak_voxel = worker_solid
            .voxel_scores
            .iter()
            .find(|voxel| voxel.is_peak)
            .expect("checked above");
        let peak_total = peak_voxel.components.get("total").expect("checked above");
        if !close_enough(peak.mean, peak_total.mean_ev_per_source)
            || !close_enough(
                peak.standard_error.unwrap_or(0.0),
                peak_total.standard_error_ev_per_source,
            )
            || !close_enough(peak.volume_cm3, peak_voxel.volume_cm3)
            || peak
                .r_cm
                .iter()
                .zip(peak_voxel.r_cm)
                .chain(peak.z_cm.iter().zip(peak_voxel.z_cm))
                .any(|(left, right)| !close_enough(*left, right))
            || peak.components.iter().any(|(name, score)| {
                peak_voxel.components.get(name).is_none_or(|voxel_score| {
                    !close_enough(score.mean_ev_per_source, voxel_score.mean_ev_per_source)
                        || !close_enough(
                            score.standard_error_ev_per_source,
                            voxel_score.standard_error_ev_per_source,
                        )
                })
            })
        {
            return Err(DesignError::Tool(
                "the worker's selected peak does not match its returned voxel scores".into(),
            ));
        }
        let estimate = normalize_heating_ev_per_source(
            peak.mean,
            peak.standard_error,
            source_rate,
            peak.volume_cm3 / 1.0e6,
            peak.volume_estimator.sampling_relative_error,
        )?;
        let volume_estimator = peak.volume_estimator.clone();
        let voxels = worker_solid
            .voxel_scores
            .into_iter()
            .map(|voxel| VoxelHeating {
                r_m: voxel.r_cm.map(|v| v / 100.0),
                z_m: voxel.z_cm.map(|v| v / 100.0),
                volume_m3: voxel.volume_cm3 / 1.0e6,
                component_scores: worker_scores(voxel.components),
                closure_residual: worker_score(voxel.closure_residual),
                is_peak: voxel.is_peak,
            })
            .collect();
        solids.push(SolidHeating {
            solid_id: worker_solid.solid_id,
            neutron: worker_score(worker_solid.neutron),
            photon: worker_score(worker_solid.photon),
            electron: worker_score(worker_solid.electron),
            positron: worker_score(worker_solid.positron),
            total: worker_score(worker_solid.total),
            closure_residual: worker_score(worker_solid.closure_residual),
            volume_estimator,
            peak: Some(HeatingPeak {
                energy_ev_per_source: estimate.energy_ev_per_source,
                sampling_se_ev_per_source: estimate.sampling_se_ev_per_source,
                volume_m3: estimate.volume_m3,
                r_m: peak.r_cm.map(|v| v / 100.0),
                z_m: peak.z_cm.map(|v| v / 100.0),
                pieces_ranked: peak.pieces_ranked,
                volume_estimator: peak.volume_estimator,
                closure_residual: worker_score(peak.closure_residual),
                power_density_kw_m3: estimate.power_density_kw_m3,
                sampling_se_kw_m3: estimate.sampling_se_kw_m3,
                volume_sampling_relative_error: estimate.volume_sampling_relative_error,
                component_scores: peak
                    .components
                    .into_iter()
                    .map(|(name, score)| (name, worker_score(score)))
                    .collect(),
            }),
            voxels,
        });
    }
    report.status = worker
        .status
        .clone()
        .unwrap_or_else(|| "NOT_EVALUATED".into());
    if report.status == "INCONCLUSIVE" {
        report.why = worker.why.clone();
        report.next_step = worker.next_step.clone();
    }
    report.openmc = worker.openmc;
    report.histories = worker.histories;
    report.c_api_terminal_status = worker.c_api_terminal_status;
    report.lost_particles = worker.lost_particles;
    report.stop_reason = worker.stop_reason;
    report.elapsed_seconds = elapsed;
    report.data_identity = worker.data_identity;
    report.solids = solids;
    if !report.solids.is_empty() {
        report.validation = ValidationMarks::for_catalogue(DESIGN_HEATING_OUTPUTS);
    }
    report.notices.push("The reported peak is the highest sampled ring voxel resolved by this tally; unsampled sub-voxel peaks may be higher.".into());
    report.notices.push("The voxel volume estimator uses ray sampling. This adapter records ray count but does not provide its volume standard error; that geometric uncertainty is not included in the Monte Carlo heating error.".into());
    write_report(&run_dir, &report)?;
    Ok(report)
}

fn write_report(run_dir: &Path, report: &DesignHeatingReport) -> Result<(), DesignError> {
    let path = run_dir.join("report.json");
    let bytes = serde_json::to_vec_pretty(report)
        .map_err(|e| DesignError::Tool(format!("serialize heating report: {e}")))?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err(DesignError::Tool(
            "the heating report exceeds the JSON size bound".into(),
        ));
    }
    std::fs::write(&path, bytes).map_err(|e| io_error(&path, e))
}

fn worker_score(score: WorkerScore) -> HeatingScore {
    HeatingScore {
        mean_ev_per_source: score.mean_ev_per_source,
        standard_error_ev_per_source: score.standard_error_ev_per_source,
    }
}

fn worker_scores(scores: BTreeMap<String, WorkerScore>) -> BTreeMap<String, HeatingScore> {
    scores
        .into_iter()
        .map(|(name, score)| (name, worker_score(score)))
        .collect()
}

fn close_enough(left: f64, right: f64) -> bool {
    let scale = left.abs().max(right.abs()).max(1.0e-300);
    (left - right).abs() <= 1.0e-8 * scale
}

fn finite_abs_sum(values: &[f64]) -> Option<f64> {
    values.iter().try_fold(0.0, |sum, value| {
        if !value.is_finite() {
            return None;
        }
        let next = sum + value.abs();
        next.is_finite().then_some(next)
    })
}

fn component_score_scale(scores: &BTreeMap<String, WorkerScore>) -> Option<f64> {
    const COMPONENTS: [&str; 5] = ["neutron", "photon", "electron", "positron", "total"];
    if scores.len() != COMPONENTS.len()
        || COMPONENTS.iter().any(|name| {
            scores.get(*name).is_none_or(|score| {
                !score.mean_ev_per_source.is_finite()
                    || score.mean_ev_per_source < 0.0
                    || !score.standard_error_ev_per_source.is_finite()
                    || score.standard_error_ev_per_source < 0.0
            })
        })
    {
        return None;
    }
    finite_abs_sum(&COMPONENTS.map(|name| {
        scores
            .get(name)
            .expect("exact component names were checked")
            .mean_ev_per_source
    }))
}

fn worker_component_maps_valid(solids: &[WorkerSolid]) -> bool {
    solids.iter().all(|solid| {
        solid
            .voxel_scores
            .iter()
            .all(|voxel| component_score_scale(&voxel.components).is_some())
            && solid
                .peak
                .as_ref()
                .is_none_or(|peak| component_score_scale(&peak.components).is_some())
    })
}

fn closure_mean_matches(residual: f64, total: f64, components: &[f64]) -> bool {
    if !residual.is_finite() || !total.is_finite() || components.iter().any(|v| !v.is_finite()) {
        return false;
    }
    let Some(component_sum) = finite_abs_sum(components) else {
        return false;
    };
    let expected = total - components.iter().sum::<f64>();
    let Some(scale) = finite_abs_sum(&[total, component_sum]) else {
        return false;
    };
    let difference = residual - expected;
    difference.is_finite() && difference.abs() <= 1.0e-8 * scale.max(1.0e-300)
}

fn closure_within_3se(score: &WorkerScore, scale: f64) -> bool {
    if !scale.is_finite()
        || !score.mean_ev_per_source.is_finite()
        || !score.standard_error_ev_per_source.is_finite()
    {
        return false;
    }
    let expected_tolerance = 64.0
        * f64::EPSILON
        * scale
            .abs()
            .max(score.mean_ev_per_source.abs())
            .max(1.0e-300);
    let gate = 3.0 * score.standard_error_ev_per_source + expected_tolerance;
    if !expected_tolerance.is_finite() || !gate.is_finite() {
        return false;
    }
    score.within_3se == Some(score.mean_ev_per_source.abs() <= gate)
        && score.roundoff_tolerance_ev_per_source.is_some_and(|value| {
            (value - expected_tolerance).abs() <= expected_tolerance.max(1.0e-300) * 1.0e-8
        })
}

fn valid_terminal_status(
    worker: &WorkerResult,
    planned_batches: u64,
    particles_per_batch: u64,
) -> bool {
    let batches = worker.histories / particles_per_batch;
    match worker.c_api_terminal_status {
        Some(1) => batches == planned_batches && worker.stop_reason == "history_limit",
        Some(0) => {
            worker.stop_reason == "time_limit"
                && batches >= MIN_BATCHES
                && batches < planned_batches
        }
        _ => false,
    }
}

fn heating_peak_is_converged(mean: f64, standard_error: Option<f64>) -> bool {
    mean.is_finite()
        && mean > 0.0
        && standard_error.is_some_and(|se| se.is_finite() && se >= 0.0 && se / mean <= 0.10)
}

pub fn render_design_heating_text(report: &DesignHeatingReport) -> String {
    use std::fmt::Write as _;
    let mut text = format!("Winding-pack heating: {}\n", report.status);
    let _ = writeln!(
        text,
        "  Design: {}",
        report.design.id.as_deref().unwrap_or("(no authored id)")
    );
    let _ = writeln!(text, "  Input path: {}", report.design.path);
    let _ = writeln!(text, "  Input SHA-256: {}", report.design.sha256);
    if let Some(why) = &report.why {
        let label = if report.status == "INCONCLUSIVE" {
            "  Reason"
        } else {
            "  Not evaluated"
        };
        let _ = writeln!(text, "{label}: {why}");
    }
    if let Some(next) = &report.next_step {
        let _ = writeln!(text, "  Next: {next}");
    }
    if report.histories > 0 {
        let _ = writeln!(
            text,
            "  Histories: {}; stopped at {}",
            report.histories, report.stop_reason
        );
    }
    if let Some(status) = report.c_api_terminal_status {
        let _ = writeln!(text, "  OpenMC C API terminal status: {status}");
    }
    if let Some(lost) = report.lost_particles {
        let fraction = if report.histories > 0 {
            format!(
                " ({:.3e} of histories)",
                lost as f64 / report.histories as f64
            )
        } else {
            String::new()
        };
        let _ = writeln!(text, "  Lost particles: {lost}{fraction}");
    }
    if report.solids.is_empty() {
        let _ = writeln!(
            text,
            "  Peak power density: not available (no evaluated peak result)"
        );
    }
    for solid in &report.solids {
        if let Some(peak) = &solid.peak {
            if peak.energy_ev_per_source > 0.0 {
                let _ = writeln!(
                    text,
                    "  {} peak power density: {:.6e} kW/m³",
                    solid.solid_id, peak.power_density_kw_m3
                );
            } else {
                let _ = writeln!(
                    text,
                    "  {} peak power density: not available (zero sampled response; relative heating uncertainty is unavailable)",
                    solid.solid_id
                );
            }
            let _ = writeln!(
                text,
                "    sampled voxel volume: {:.6e} m³; heating MC SE: {}",
                peak.volume_m3,
                peak.sampling_se_kw_m3.map_or_else(
                    || "not available".into(),
                    |se| {
                        if peak.power_density_kw_m3 > 0.0 {
                            format!("{:.3}%", 100.0 * se / peak.power_density_kw_m3)
                        } else {
                            "not available (zero mean)".into()
                        }
                    }
                )
            );
            let _ = writeln!(
                text,
                "    peak raw total: {:.6e} eV/source neutron; component batch closure residual: {:.6e} ± {:.6e} eV/source",
                peak.energy_ev_per_source,
                peak.closure_residual.mean_ev_per_source,
                peak.closure_residual.standard_error_ev_per_source
            );
            for name in ["neutron", "photon", "electron", "positron", "total"] {
                if let Some(score) = peak.component_scores.get(name) {
                    let _ = writeln!(
                        text,
                        "    {name} peak score: {:.6e} ± {:.6e} eV/source neutron",
                        score.mean_ev_per_source, score.standard_error_ev_per_source
                    );
                }
            }
            let _ = writeln!(text, "    scored sampled voxels: {}", solid.voxels.len());
            let _ = writeln!(
                text,
                "    volume estimator: {} ({} rays; relative SE: {})",
                peak.volume_estimator.method,
                peak.volume_estimator.rays,
                peak.volume_estimator
                    .sampling_relative_error
                    .map_or_else(|| "not reported".into(), |se| format!("{:.3}%", 100.0 * se))
            );
        } else {
            let _ = writeln!(
                text,
                "  {} peak power density: not available (no resolved sampled voxel)",
                solid.solid_id
            );
        }
    }
    let _ = writeln!(text, "  Validation scope: {}", report.validation_scope);
    if !report.validation.is_empty() {
        let _ = writeln!(
            text,
            "  {}",
            report
                .validation
                .tag("power_density_kw_m3", Quantity::Heating)
        );
    }
    for notice in &report.notices {
        let _ = writeln!(text, "  Note: {notice}");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_parser_preserves_failed_correlated_closure_and_terminal_controls() {
        let score = |mean: f64, se: f64| {
            serde_json::json!({
                "mean_ev_per_source": mean,
                "standard_error_ev_per_source": se,
                "within_3se": false,
                "roundoff_tolerance_ev_per_source": 2.1316282072803006e-13
            })
        };
        let raw = serde_json::json!({
            "schema": RESULT_SCHEMA,
            "status": "INCONCLUSIVE",
            "why": "closure outside three SE",
            "c_api_terminal_status": 1,
            "lost_particles": 0,
            "histories": 100_000,
            "stop_reason": "history_limit",
            "h5m_sha256": "h",
            "cross_sections_sha256": "x",
            "solids": [{
                "solid_id": "winding",
                "neutron": {"mean_ev_per_source": 5.0, "standard_error_ev_per_source": 1.0},
                "photon": {"mean_ev_per_source": 1.0, "standard_error_ev_per_source": 1.0},
                "electron": {"mean_ev_per_source": 1.0, "standard_error_ev_per_source": 1.0},
                "positron": {"mean_ev_per_source": 0.0, "standard_error_ev_per_source": 0.0},
                "total": {"mean_ev_per_source": 8.0, "standard_error_ev_per_source": 1.0},
                "closure_residual": score(1.0, 0.0),
                "peak": {
                    "mean": 0.0,
                    "standard_error": null,
                    "volume_cm3": 1000.0,
                    "r_cm": [100.0, 105.0],
                    "z_cm": [-5.0, 0.0],
                    "pieces_ranked": 1,
                    "volume_estimator": {"method": "fixture", "rays": 1000,
                                         "sampling_relative_error": null,
                                         "sampling_uncertainty_reported": false},
                    "closure_residual": score(0.0, 0.0),
                    "components": {
                        "neutron": score(0.0, 0.0), "photon": score(0.0, 0.0),
                        "electron": score(0.0, 0.0), "positron": score(0.0, 0.0),
                        "total": score(0.0, 0.0)
                    }
                },
                "voxel_scores": []
            }]
        });
        let worker: WorkerResult = serde_json::from_value(raw).unwrap();
        assert_eq!(worker.status.as_deref(), Some("INCONCLUSIVE"));
        assert_eq!(worker.c_api_terminal_status, Some(1));
        assert_eq!(worker.lost_particles, Some(0));
        let residual = &worker.solids[0].closure_residual;
        assert_eq!(residual.mean_ev_per_source, 1.0);
        assert!(!closure_within_3se(residual, 15.0));
        let peak = worker.solids[0].peak.as_ref().unwrap();
        assert!(!heating_peak_is_converged(peak.mean, peak.standard_error));
        assert_eq!(peak.mean, 0.0);
        assert_eq!(peak.standard_error, None);
        assert!(valid_terminal_status(&worker, 100, 1_000));
        assert!(!valid_terminal_status(&worker, 101, 1_000));
    }

    #[test]
    fn peak_sampling_quality_requires_positive_mean_and_at_most_ten_percent_error() {
        assert!(!heating_peak_is_converged(0.0, Some(0.0)));
        assert!(!heating_peak_is_converged(1.0, None));
        assert!(heating_peak_is_converged(1.0, Some(0.10)));
        assert!(!heating_peak_is_converged(1.0, Some(0.100_001)));
    }

    #[test]
    fn two_volumes_scale_density_and_sampling_error_independently() {
        let a =
            normalize_heating_ev_per_source(1000.0, Some(100.0), 1.0e18, 0.01, Some(0.02)).unwrap();
        let b =
            normalize_heating_ev_per_source(1000.0, Some(100.0), 1.0e18, 0.02, Some(0.04)).unwrap();
        assert!((a.power_density_kw_m3 - 16.021_766_34).abs() < 1e-12);
        assert!((b.power_density_kw_m3 - 8.010_883_17).abs() < 1e-12);
        assert!((a.sampling_se_kw_m3.unwrap() - 1.602_176_634).abs() < 1e-12);
        assert!((b.sampling_se_kw_m3.unwrap() - 0.801_088_317).abs() < 1e-12);
        assert_eq!(a.volume_sampling_relative_error, Some(0.02));
        assert_eq!(b.volume_sampling_relative_error, Some(0.04));
    }

    #[test]
    fn invalid_dimensions_and_errors_are_rejected() {
        assert!(power_density_kw_m3(1.0, 1.0, 0.0).is_err());
        assert!(normalize_heating_ev_per_source(1.0, Some(f64::NAN), 1.0, 1.0, None).is_err());
        assert!(power_density_kw_m3(f64::MAX, 1.0e308, 1.0e-300).is_err());
        assert!(
            normalize_heating_ev_per_source(1.0, Some(f64::MAX), 1.0e308, 1.0e-300, None).is_err()
        );
    }

    #[test]
    fn parsed_voxels_reject_extra_or_overflowing_component_scales() {
        let score = |mean: f64| {
            serde_json::json!({
                "mean_ev_per_source": mean,
                "standard_error_ev_per_source": 0.0
            })
        };
        let voxel = |components| {
            serde_json::json!({
                "r_cm": [1.0, 2.0],
                "z_cm": [3.0, 4.0],
                "volume_cm3": 1.0,
                "components": components,
                "closure_residual": score(0.0),
                "is_peak": true
            })
        };
        let extra_components = serde_json::json!({
            "neutron": score(1.0), "photon": score(1.0), "electron": score(1.0),
            "positron": score(1.0), "total": score(4.0), "diagnostic_scale": score(1.0e308)
        });
        let extra: WorkerVoxel = serde_json::from_value(voxel(extra_components.clone())).unwrap();
        assert!(component_score_scale(&extra.components).is_none());
        let solid: WorkerSolid = serde_json::from_value(serde_json::json!({
            "solid_id": "bad",
            "neutron": score(1.0), "photon": score(1.0), "electron": score(1.0),
            "positron": score(1.0), "total": score(4.0), "closure_residual": score(0.0),
            "peak": null,
            "voxel_scores": [voxel(extra_components)]
        }))
        .unwrap();
        assert!(!worker_component_maps_valid(&[solid]));
        let overflow: WorkerVoxel = serde_json::from_value(voxel(serde_json::json!({
            "neutron": score(1.0e308), "photon": score(1.0e308), "electron": score(1.0e308),
            "positron": score(1.0e308), "total": score(1.0e308)
        })))
        .unwrap();
        assert!(
            overflow
                .components
                .values()
                .all(|s| s.mean_ev_per_source.is_finite())
        );
        assert!(component_score_scale(&overflow.components).is_none());
        assert!(finite_abs_sum(&[1.0e308, 1.0e308]).is_none());
        assert!(!closure_mean_matches(1.0e308, 1.0e308, &[1.0e308, 1.0e308]));
    }

    #[test]
    fn paired_winding_pack_report_numbers_are_classified() {
        let score = HeatingScore {
            mean_ev_per_source: 1000.0,
            standard_error_ev_per_source: 100.0,
        };
        let estimator = VolumeEstimator {
            method: "OpenMC sampled ring volume".into(),
            rays: 2_000_000,
            sampling_relative_error: None,
            sampling_uncertainty_reported: false,
        };
        let components: BTreeMap<String, HeatingScore> =
            ["neutron", "photon", "electron", "positron", "total"]
                .into_iter()
                .map(|name| (name.into(), score.clone()))
                .collect();
        let peak = HeatingPeak {
            energy_ev_per_source: 1000.0,
            sampling_se_ev_per_source: Some(100.0),
            volume_m3: 0.01,
            r_m: [1.0, 1.05],
            z_m: [-0.05, 0.0],
            pieces_ranked: 20,
            volume_estimator: estimator.clone(),
            closure_residual: HeatingScore {
                mean_ev_per_source: 0.0,
                standard_error_ev_per_source: 5.0,
            },
            power_density_kw_m3: 16.021_766_34,
            sampling_se_kw_m3: Some(1.602_176_634),
            volume_sampling_relative_error: None,
            component_scores: components.clone(),
        };
        let report = DesignHeatingReport {
            schema_version: RESULT_SCHEMA.into(),
            status: "CALCULATED".into(),
            design: DesignIdentity {
                path: "design.json".into(),
                sha256: "d".repeat(64),
                id: Some("case".into()),
            },
            check_report_sha256: "c".repeat(64),
            h5m_sha256: "h".repeat(64),
            cross_sections_sha256: "x".repeat(64),
            run_directory: "run".into(),
            openmc: Some("0.15.3".into()),
            source_rate_per_s: 1.0e18,
            energy_per_reaction_ev: D_T_ENERGY_PER_REACTION_EV,
            fusion_power_mw: 283.0,
            histories: 600_000,
            c_api_terminal_status: Some(1),
            lost_particles: Some(0),
            stop_reason: "history_limit".into(),
            elapsed_seconds: 30.0,
            seed: 7,
            threads: 2,
            data_identity: Some(serde_json::json!({"photon": {"Fe": {"shell_count": 7}}})),
            validation_scope: "No physical validation claim.".into(),
            solids: vec![SolidHeating {
                solid_id: "tf-winding-01".into(),
                neutron: score.clone(),
                photon: score.clone(),
                electron: score.clone(),
                positron: score.clone(),
                total: score.clone(),
                closure_residual: HeatingScore {
                    mean_ev_per_source: 0.0,
                    standard_error_ev_per_source: 5.0,
                },
                volume_estimator: estimator,
                peak: Some(peak),
                voxels: vec![VoxelHeating {
                    r_m: [1.0, 1.05],
                    z_m: [-0.05, 0.0],
                    volume_m3: 0.01,
                    component_scores: components,
                    closure_residual: HeatingScore {
                        mean_ev_per_source: 0.0,
                        standard_error_ev_per_source: 5.0,
                    },
                    is_peak: true,
                }],
            }],
            why: None,
            next_step: None,
            notices: vec![],
            validation: ValidationMarks::for_catalogue(DESIGN_HEATING_OUTPUTS),
        };
        let json = serde_json::to_value(&report).unwrap();
        assert!(
            faris_model::validation::unclassified_numbers(&json, DESIGN_HEATING_OUTPUTS).is_empty()
        );
        assert!(
            (report.solids[0].peak.as_ref().unwrap().power_density_kw_m3 - 16.021_766_34).abs()
                < 1e-12
        );
        assert_eq!(report.solids[0].voxels[0].volume_m3, 0.01);
        let rendered = render_design_heating_text(&report);
        assert!(rendered.contains("Design: case"));
        assert!(rendered.contains("Input path: design.json"));
        assert!(rendered.contains(&format!("Input SHA-256: {}", "d".repeat(64))));
        let mut missing_peak = report.clone();
        missing_peak.solids[0].peak = None;
        missing_peak.status = "NOT_EVALUATED".into();
        let missing_text = render_design_heating_text(&missing_peak);
        assert!(
            missing_text.contains("peak power density: not available (no resolved sampled voxel)")
        );
        assert!(!missing_text.contains("peak power density: 0.000000e0"));
        missing_peak.solids.clear();
        let no_result = render_design_heating_text(&missing_peak);
        assert!(no_result.contains("Peak power density: not available (no evaluated peak result)"));
        let mut zero_peak = report.clone();
        zero_peak.status = "INCONCLUSIVE".into();
        let zero = zero_peak.solids[0].peak.as_mut().unwrap();
        zero.energy_ev_per_source = 0.0;
        zero.sampling_se_ev_per_source = None;
        zero.power_density_kw_m3 = 0.0;
        zero.sampling_se_kw_m3 = None;
        let zero_text = render_design_heating_text(&zero_peak);
        assert!(
            zero_text
                .contains("zero sampled response; relative heating uncertainty is unavailable")
        );
        assert!(zero_text.contains("peak raw total: 0.000000e0 eV/source neutron"));
        assert!(!zero_text.contains("peak power density: 0.000000e0"));
        for output in DESIGN_HEATING_OUTPUTS {
            if let faris_model::validation::Class::Calculated(quantity) = output.class {
                assert_eq!(report.validation.0[output.name].quantity, quantity);
            }
        }
    }
}
