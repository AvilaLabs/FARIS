// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! Unit conversion for coupled design heating. The OpenMC score remains raw eV per source
//! neutron until this boundary; sampled volume uncertainty is recorded separately from MC SE.

use super::transport::read_bounded;
use super::{
    CAD_ADDRESS_SPACE, DesignError, DesignIdentity, ImportReport, Launched, MAX_JSON_BYTES,
    ToolPaths, io_error, launch_python_with_lines, sha256_file, sha256_hex,
};
use crate::history::{ScalarRate, TransportDrivingRates};
use crate::jobs::{Cancellation, ResourceLimits};
use crate::transport::{D_T_ENERGY_PER_REACTION_EV, reaction_rate_per_s};
use faris_model::design::{self, Role};
use faris_model::units::DAY_S;
use faris_model::validation::{Output, Quantity, ValidationMarks, calculated, not_calculated};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

pub const ELEMENTARY_CHARGE_C: f64 = crate::transport::ELEMENTARY_CHARGE_C;

const REQUEST_SCHEMA: &str = "faris-design-transport-request/v3";
const RESULT_SCHEMA: &str = "faris-design-heating/v1";
const MIN_BATCHES: u64 = 30;
const MAX_HEATING_BATCHES: u64 = 100_000;
const MAX_HEATING_HISTORIES: u64 = 2_000_000_000;
const MAX_HEATING_SECONDS: u64 = DAY_S as u64;
const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const VOLUME_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const STARTUP_ALLOWANCE: Duration = Duration::from_secs(30 * 60);
const MAX_RECEIPT_FILE_BYTES: u64 = 12 * 1024 * 1024 * 1024;

#[derive(Clone)]
pub struct DesignHeatingOptions {
    pub check_report: PathBuf,
    pub out_dir: PathBuf,
    pub peak_voxel_m: [f64; 2],
    pub particles_per_batch: u64,
    pub max_histories: u64,
    pub max_seconds: u64,
    pub target_relative_error: f64,
    pub progress: Option<HeatingProgressFn>,
    pub seed: u64,
    pub threads: u32,
    pub cancellation: Cancellation,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HeatingProgress {
    pub batches: u64,
    pub histories: u64,
    pub mean_relative_error: Option<f64>,
    pub peak_relative_error: Option<f64>,
    pub variance_of_variance: Option<f64>,
    pub elapsed_seconds: f64,
}

pub type HeatingProgressFn = std::sync::Arc<dyn Fn(HeatingProgress) + Send + Sync>;

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
pub struct HeatingFileIdentity {
    pub path: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct HeatingRuntimeIdentity {
    pub python: HeatingFileIdentity,
    pub openmc_version: String,
    pub openmc_c_api_library: HeatingFileIdentity,
    pub worker_helper: HeatingFileIdentity,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct HeatingExecutionIdentity {
    pub run_h5m: HeatingFileIdentity,
    pub model_xml: HeatingFileIdentity,
}

/// Hashes of the exact native request and worker output retained in the heating run folder.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HeatingWorkerReceipt {
    pub request_sha256: String,
    pub result_sha256: String,
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
    #[serde(default)]
    pub batches: u64,
    #[serde(default)]
    pub target_relative_error: Option<f64>,
    #[serde(default)]
    pub peak_target_relative_error: Option<f64>,
    #[serde(default)]
    pub target_reached: bool,
    #[serde(default)]
    pub mean_relative_error: Option<f64>,
    #[serde(default)]
    pub peak_relative_error: Option<f64>,
    #[serde(default)]
    pub variance_of_variance: Option<f64>,
    pub c_api_terminal_status: Option<i32>,
    pub lost_particles: Option<u64>,
    pub stop_reason: String,
    pub elapsed_seconds: f64,
    pub seed: u64,
    pub threads: u32,
    pub data_identity: Option<serde_json::Value>,
    #[serde(default)]
    pub runtime_identity: Option<HeatingRuntimeIdentity>,
    #[serde(default)]
    pub execution_identity: Option<HeatingExecutionIdentity>,
    pub validation_scope: String,
    pub solids: Vec<SolidHeating>,
    /// Full-model energy response coupled to the exact CAD history component map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_energy_response: Option<HistoryEnergyResponse>,
    /// Present when the full-model history-energy response was requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_energy_worker_receipt: Option<HeatingWorkerReceipt>,
    pub why: Option<String>,
    pub next_step: Option<String>,
    pub notices: Vec<String>,
    #[serde(default, skip_serializing_if = "ValidationMarks::is_empty")]
    pub validation: ValidationMarks,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HistoryEnergyResponse {
    pub status: String,
    pub source_definition_sha256: String,
    pub source_kernel_path: String,
    pub source_kernel_sha256: String,
    pub component_map: BTreeMap<String, Vec<String>>,
    pub component_map_sha256: String,
    pub domain: String,
    pub normalization: String,
    pub source_strength_normalized_to: String,
    pub histories: u64,
    pub batches: u64,
    pub target_relative_error: f64,
    pub target_reached: bool,
    pub variance_of_variance: Option<f64>,
    pub whole_model: HistoryEnergyScore,
    pub components: BTreeMap<String, HistoryEnergyScore>,
    pub response_ids: Vec<String>,
    pub covariance_of_mean_ev2: Vec<f64>,
    pub covariance_method: String,
    pub closure_residual: HistoryEnergyClosure,
    pub cross_run_covariance: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HistoryEnergyScore {
    pub mean_ev_per_source: f64,
    pub standard_error_ev_per_source: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HistoryEnergyClosure {
    pub mean: f64,
    pub standard_error: f64,
    pub within_3se: bool,
    pub roundoff_tolerance: f64,
}

pub const HISTORY_ENERGY_RATES_VERSION: &str = "faris-cad-history-energy-rates/v0.1";
/// CAD-native energy-rate receipt bound to a checked design rather than a CSG scenario.
pub const HISTORY_ENERGY_RATES_CAD_VERSION: &str = "faris-cad-history-energy-rates/v0.2";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HistoryEnergyRatesArtifact {
    pub schema_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cad_source_identity: Option<faris_model::design::CadSourceIdentity>,
    pub base_rates_path: String,
    pub base_rates_sha256: String,
    pub heating_report_path: String,
    pub heating_report_sha256: String,
    pub design_sha256: String,
    pub check_report_sha256: String,
    pub h5m_path: String,
    pub h5m_sha256: String,
    pub cross_sections_path: String,
    pub cross_sections_sha256: String,
    pub source_definition_sha256: String,
    pub heating_data_identity: serde_json::Value,
    pub covariance_scope: String,
    pub driving_rates: TransportDrivingRates,
    #[serde(default, skip_serializing_if = "ValidationMarks::is_empty")]
    pub validation: ValidationMarks,
}

/// Numeric outputs published by a combined CAD history/whole-model heating receipt.
pub const HISTORY_ENERGY_RATES_OUTPUTS: &[Output] = &[
    calculated("breeder_h3_per_source_neutron", Quantity::Tbr),
    calculated("component_average_flux_n_m2_s", Quantity::Flux),
    calculated("region_flux_n_m2_s", Quantity::Flux),
    calculated("transport_deposited_heat_w", Quantity::Heating),
    not_calculated(
        "covariance",
        "base-run covariance; separate heating-run covariance is unknown",
    ),
    not_calculated("response_id", "response identity metadata"),
    not_calculated(
        "mean",
        "response estimate value is classified by its enclosing response",
    ),
    not_calculated(
        "standard_error",
        "sampling diagnostic, not an additional response",
    ),
    not_calculated("reference_fusion_power_mw", "authored operating input"),
    not_calculated(
        "fusion_reaction_rate_per_s",
        "source normalization conversion",
    ),
    not_calculated(
        "neutron_source_rate_per_s",
        "source normalization conversion",
    ),
    not_calculated("total_reaction_energy_ev", "source convention constant"),
    not_calculated("primary_neutron_energy_ev", "source convention constant"),
    not_calculated("fusion_power_mw", "authored CAD operating input"),
    not_calculated("source_neutron_rate_per_s", "source normalization input"),
    not_calculated("bytes", "nuclear-data file size metadata"),
    not_calculated("shell_count", "photon data shell-count metadata"),
    not_calculated("histories", "transport sample count"),
    not_calculated("batches", "independent transport batch count"),
];

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
    calculated("covariance_of_mean_ev2", Quantity::Heating),
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
        "batches",
        "An independent batch count, not a heating response.",
    ),
    not_calculated(
        "target_relative_error",
        "An authored mean sampling target, not a heating response.",
    ),
    not_calculated(
        "peak_target_relative_error",
        "The authored resolved-peak sampling target, not a heating response.",
    ),
    not_calculated(
        "target_reached",
        "A sampling-stop condition, not a heating response.",
    ),
    not_calculated(
        "mean_relative_error",
        "A batch sampling diagnostic, not a heating response.",
    ),
    not_calculated(
        "peak_relative_error",
        "A resolved-peak batch sampling diagnostic, not a heating response.",
    ),
    not_calculated(
        "variance_of_variance",
        "A batch reliability diagnostic, not a heating response.",
    ),
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
    batches: u64,
    #[serde(default)]
    target_relative_error: Option<f64>,
    #[serde(default)]
    peak_target_relative_error: Option<f64>,
    #[serde(default)]
    target_reached: bool,
    #[serde(default)]
    mean_relative_error: Option<f64>,
    #[serde(default)]
    peak_relative_error: Option<f64>,
    #[serde(default)]
    variance_of_variance: Option<f64>,
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
    #[serde(default)]
    runtime_identity: Option<HeatingRuntimeIdentity>,
    #[serde(default)]
    execution_identity: Option<HeatingExecutionIdentity>,
    #[serde(default)]
    history_energy_response: Option<HistoryEnergyResponse>,
}

fn parse_heating_progress(line: &str) -> Option<HeatingProgress> {
    #[derive(Deserialize)]
    struct Line {
        progress: Progress,
    }
    #[derive(Deserialize)]
    struct Progress {
        batches: u64,
        histories: u64,
        relative_error: Option<f64>,
        peak_relative_error: Option<f64>,
        variance_of_variance: Option<f64>,
        elapsed_seconds: f64,
    }
    if !line.starts_with("{\"progress\"") {
        return None;
    }
    let progress: Line = serde_json::from_str(line).ok()?;
    Some(HeatingProgress {
        batches: progress.progress.batches,
        histories: progress.progress.histories,
        mean_relative_error: progress.progress.relative_error,
        peak_relative_error: progress.progress.peak_relative_error,
        variance_of_variance: progress.progress.variance_of_variance,
        elapsed_seconds: progress.progress.elapsed_seconds,
    })
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
    #[serde(default)]
    runtime_identity: Option<HeatingRuntimeIdentity>,
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
    design_heating_impl(design_path, tools, options, None)
}

/// Run coupled heating and add the exact component partition from a verified CAD history-rate
/// artifact to the same OpenMC sample. The optional response never substitutes winding-pack heat.
pub fn design_heating_with_history_rates(
    design_path: &Path,
    tools: &ToolPaths,
    options: &DesignHeatingOptions,
    base_history_rates_path: &Path,
) -> Result<DesignHeatingReport, DesignError> {
    let bytes = read_bounded(base_history_rates_path)?;
    let base: super::transport::DesignHistoryRatesArtifact = serde_json::from_slice(&bytes)
        .map_err(|e| DesignError::Tool(format!("CAD history-rate artifact: {e}")))?;
    match base.schema_version.as_str() {
        super::transport::DESIGN_HISTORY_RATES_VERSION => {
            let scenario_path = base.scenario_path.as_deref().ok_or_else(|| {
                DesignError::Tool("legacy CAD history rates lack a scenario path".into())
            })?;
            let scenario_bytes = read_bounded(Path::new(scenario_path))?;
            let scenario = faris_model::LoadedScenario::from_bytes(&scenario_bytes)
                .map_err(|e| DesignError::Tool(format!("recorded history scenario: {e}")))?;
            super::transport::load_design_history_rates(base_history_rates_path, &scenario)
                .map_err(DesignError::Tool)?;
        }
        super::transport::DESIGN_HISTORY_RATES_CAD_VERSION => {
            super::transport::load_cad_design_history_rates(base_history_rates_path, design_path)
                .map_err(DesignError::Tool)?;
        }
        _ => {
            return Err(DesignError::Tool(
                "unsupported CAD history-rate schema".into(),
            ));
        }
    }
    let base_sha256 = sha256_hex(&bytes);
    if !base_sha256.eq_ignore_ascii_case(&sha256_file(base_history_rates_path)?) {
        return Err(DesignError::Tool(
            "CAD history-rate artifact changed while it was being checked".into(),
        ));
    }
    let context = HistoryEnergyRequest {
        component_map: base.component_map,
        source_definition_sha256: base.source_definition_sha256,
        design_sha256: base.design_sha256,
        check_report_sha256: base.check_report_sha256,
        h5m_sha256: base.h5m_sha256,
        h5m_path: base.h5m_path,
        cross_sections_sha256: base.cross_sections_sha256,
        cross_sections_path: base.cross_sections_path,
        source_kernel_path: base.source_kernel_path,
        source_kernel_sha256: base.source_kernel_sha256,
    };
    design_heating_impl(design_path, tools, options, Some(context))
}

#[derive(Clone)]
struct HistoryEnergyRequest {
    component_map: BTreeMap<String, Vec<String>>,
    source_definition_sha256: String,
    design_sha256: String,
    check_report_sha256: String,
    h5m_sha256: String,
    h5m_path: String,
    cross_sections_sha256: String,
    cross_sections_path: String,
    source_kernel_path: String,
    source_kernel_sha256: String,
}

fn design_heating_impl(
    design_path: &Path,
    tools: &ToolPaths,
    options: &DesignHeatingOptions,
    history_energy_request: Option<HistoryEnergyRequest>,
) -> Result<DesignHeatingReport, DesignError> {
    if options.particles_per_batch == 0
        || options.max_histories / options.particles_per_batch.max(1) < MIN_BATCHES
        || options.max_histories > MAX_HEATING_HISTORIES
        || options.max_histories / options.particles_per_batch.max(1) > MAX_HEATING_BATCHES
        || options.max_seconds == 0
        || options.max_seconds > MAX_HEATING_SECONDS
        || !options.target_relative_error.is_finite()
        || !(0.0..1.0).contains(&options.target_relative_error)
        || options.threads == 0
        || options.threads > 32
        || options
            .peak_voxel_m
            .iter()
            .any(|d| !d.is_finite() || *d < 0.01)
    {
        return Err(DesignError::Tool(
            "heating needs at least 30 batches, a target error between 0 and 1, a positive time limit, 1 to 32 threads, and peak voxels at least 0.01 m".into(),
        ));
    }
    let design_bytes = read_bounded(design_path)?;
    let design_sha256 = sha256_hex(&design_bytes);
    let design = design::parse_design(&design_bytes).design.ok_or_else(|| {
        DesignError::Tool("the design file does not parse; run `faris design check`".into())
    })?;
    if history_energy_request.as_ref().is_some_and(|context| {
        serde_json::to_vec(&design.plasma)
            .ok()
            .map(|bytes| sha256_hex(&bytes))
            .as_deref()
            != Some(context.source_definition_sha256.as_str())
    }) {
        return Err(DesignError::Tool(
            "history rates were generated from a different plasma source definition".into(),
        ));
    }
    let report_bytes = read_bounded(&options.check_report)?;
    if history_energy_request
        .as_ref()
        .is_some_and(|context| context.design_sha256 != design_sha256)
    {
        return Err(DesignError::Tool(
            "history rates were generated for a different CAD design".into(),
        ));
    }
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
    if history_energy_request.as_ref().is_some_and(|context| {
        context.check_report_sha256 != sha256_hex(&report_bytes)
            || !context
                .h5m_sha256
                .eq_ignore_ascii_case(&conversion.h5m_sha256)
            || h5m.canonicalize().ok().as_deref() != Some(Path::new(&context.h5m_path))
    }) {
        return Err(DesignError::Tool(
            "history rates are bound to a different design check report or H5M".into(),
        ));
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
        batches: 0,
        target_relative_error: Some(options.target_relative_error),
        peak_target_relative_error: Some(2.0 * options.target_relative_error),
        target_reached: false,
        mean_relative_error: None,
        peak_relative_error: None,
        variance_of_variance: None,
        c_api_terminal_status: None,
        lost_particles: None,
        stop_reason: "not_started".into(),
        elapsed_seconds: 0.0,
        seed: options.seed,
        threads: options.threads,
        data_identity: None,
        runtime_identity: None,
        execution_identity: None,
        validation_scope: "No physical validation claim. Calculated values are Monte Carlo estimates scoped to the authored winding-pack solids and the checked CAD conversion.".into(),
        solids: Vec::new(),
        history_energy_response: None,
        history_energy_worker_receipt: None,
        why: None,
        next_step: None,
        notices: vec!["Target stopping uses aggregate per-batch heating error, the resolved peak in each authored solid, and batch variance-of-variance. Geometry volume uncertainty is separate.".into()],
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
    if history_energy_request.as_ref().is_some_and(|context| {
        !context
            .cross_sections_sha256
            .eq_ignore_ascii_case(&cross_sections_sha256)
            || context.cross_sections_path != cross_sections.display().to_string()
    }) {
        return Err(DesignError::Tool(
            "history rates were generated with different cross-section data".into(),
        ));
    }
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
        "history_energy_response": history_energy_request.as_ref().map(|context| serde_json::json!({
            "component_map": &context.component_map,
            "source_definition_sha256": &context.source_definition_sha256,
            "source_kernel_path": &context.source_kernel_path,
            "source_kernel_sha256": &context.source_kernel_sha256,
        })),
        "sampling": {
            "particles_per_batch": options.particles_per_batch,
            "min_batches": MIN_BATCHES,
            "max_batches": options.max_histories / options.particles_per_batch,
            "max_seconds": options.max_seconds,
            "target_relative_error": options.target_relative_error,
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
    let request_bytes = serde_json::to_vec_pretty(&request)
        .map_err(|e| DesignError::Tool(format!("could not serialize heating request: {e}")))?;
    std::fs::write(&request_path, &request_bytes).map_err(|e| io_error(&request_path, e))?;
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
            &|line| {
                if let (Some(progress), Some(update)) =
                    (&options.progress, parse_heating_progress(line))
                {
                    progress(update);
                }
            },
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
    report.data_identity = preflight.data_identity.clone();
    report.runtime_identity = preflight.runtime_identity.clone();
    if let Some(runtime) = &preflight.runtime_identity
        && !verify_runtime_identity(runtime, &run_dir)
    {
        return Err(DesignError::Tool(
                "the heating preflight runtime receipt does not match the selected Python, loaded OpenMC C API library, or staged worker helper".into(),
            ));
    }
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
    if preflight.data_identity.is_none() || preflight.runtime_identity.is_none() {
        return Err(DesignError::Tool(
            "a READY heating preflight must include nuclear-data and runtime identities".into(),
        ));
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
    if worker
        .data_identity
        .as_ref()
        .is_some_and(|identity| Some(identity) != preflight.data_identity.as_ref())
        || worker
            .runtime_identity
            .as_ref()
            .is_some_and(|identity| Some(identity) != preflight.runtime_identity.as_ref())
    {
        return Err(DesignError::Tool(
            "the heating run runtime or nuclear-data identity differs from READY preflight".into(),
        ));
    }
    if let Some(runtime) = &worker.runtime_identity
        && !verify_runtime_identity(runtime, &run_dir)
    {
        return Err(DesignError::Tool(
                "the heating run runtime receipt does not match the selected Python, loaded OpenMC C API library, or staged worker helper".into(),
            ));
    }
    if let Some(execution) = &worker.execution_identity {
        if !verify_execution_identity(execution, &run_dir) {
            return Err(DesignError::Tool(
                "the heating C API input receipt does not match the retagged H5M or exported model XML".into(),
            ));
        }
        report.execution_identity = Some(execution.clone());
    }
    if worker.status.as_deref() == Some("NOT_EVALUATED") {
        if worker.histories > 0
            && (!worker.histories.is_multiple_of(options.particles_per_batch)
                || worker.batches != worker.histories / options.particles_per_batch
                || worker.histories > options.max_histories
                || worker.target_relative_error != Some(options.target_relative_error)
                || worker.peak_target_relative_error != Some(2.0 * options.target_relative_error)
                || worker.target_reached != (worker.stop_reason == "target_reached"))
        {
            return Err(DesignError::Tool(
                "the unevaluated heating worker count or target metadata does not match the checked request".into(),
            ));
        }
        report.elapsed_seconds = elapsed;
        report.histories = worker.histories;
        report.batches = worker.batches;
        report.target_reached = worker.target_reached;
        report.mean_relative_error = worker.mean_relative_error;
        report.peak_relative_error = worker.peak_relative_error;
        report.variance_of_variance = worker.variance_of_variance;
        report.stop_reason = worker.stop_reason;
        report.target_relative_error = worker
            .target_relative_error
            .or(report.target_relative_error);
        report.peak_target_relative_error = worker
            .peak_target_relative_error
            .or(report.peak_target_relative_error);
        report.c_api_terminal_status = worker.c_api_terminal_status;
        report.lost_particles = worker.lost_particles;
        report.why = worker.why;
        report.next_step = worker.next_step;
        return write_report(&run_dir, &report).map(|()| report);
    }
    if !matches!(
        worker.status.as_deref(),
        Some("CALCULATED" | "INCONCLUSIVE")
    ) || !matches!(
        worker.stop_reason.as_str(),
        "history_limit" | "time_limit" | "target_reached"
    ) || worker
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
        || worker.batches != worker.histories / options.particles_per_batch
        || worker.target_relative_error != Some(options.target_relative_error)
        || worker.peak_target_relative_error != Some(2.0 * options.target_relative_error)
        || worker.target_reached != (worker.stop_reason == "target_reached")
        || worker
            .mean_relative_error
            .is_some_and(|v| !v.is_finite() || v < 0.0)
        || worker
            .peak_relative_error
            .is_some_and(|v| !v.is_finite() || v < 0.0)
        || worker
            .variance_of_variance
            .is_some_and(|v| !v.is_finite() || v < 0.0)
        || (worker.target_reached
            && (worker
                .mean_relative_error
                .is_none_or(|v| v > options.target_relative_error)
                || worker
                    .peak_relative_error
                    .is_none_or(|v| v > 2.0 * options.target_relative_error)
                || worker.variance_of_variance.is_none_or(|v| v > 0.1)))
        || (worker.status.as_deref() == Some("CALCULATED") && !worker.target_reached)
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
        || worker.data_identity != preflight.data_identity
        || worker.runtime_identity != preflight.runtime_identity
        || worker.execution_identity.is_none()
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
    match (&history_energy_request, &worker.history_energy_response) {
        (Some(request), Some(response))
            if history_energy_response_valid(response, request, worker.histories, worker.batches,
                                             options.target_relative_error) => {}
        (None, None) => {}
        (Some(_), Some(_)) => return Err(DesignError::Tool(
            "the whole-model history-energy response failed its source, partition, covariance, uncertainty, or closure checks".into(),
        )),
        (Some(_), None) => return Err(DesignError::Tool(
            "the coupled-heating worker omitted the requested whole-model history-energy response".into(),
        )),
        (None, Some(_)) => return Err(DesignError::Tool(
            "the coupled-heating worker returned an unrequested history-energy response".into(),
        )),
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
    let solids = worker
        .solids
        .into_iter()
        .map(|solid| worker_solid_to_heating(solid, source_rate))
        .collect::<Result<Vec<_>, _>>()?;
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
    report.batches = worker.batches;
    report.target_relative_error = worker.target_relative_error;
    report.peak_target_relative_error = worker.peak_target_relative_error;
    report.target_reached = worker.target_reached;
    report.mean_relative_error = worker.mean_relative_error;
    report.peak_relative_error = worker.peak_relative_error;
    report.variance_of_variance = worker.variance_of_variance;
    report.c_api_terminal_status = worker.c_api_terminal_status;
    report.lost_particles = worker.lost_particles;
    report.stop_reason = worker.stop_reason;
    report.elapsed_seconds = elapsed;
    report.data_identity = worker.data_identity;
    report.solids = solids;
    if history_energy_request.is_some() {
        let retained_request = super::transport::read_bounded(&request_path)?;
        let retained_result = super::transport::read_bounded(&result_path)?;
        if retained_request != request_bytes || retained_result != worker_bytes {
            return Err(DesignError::Tool(
                "heating request or raw worker result changed before report creation".into(),
            ));
        }
        report.history_energy_worker_receipt = Some(HeatingWorkerReceipt {
            request_sha256: sha256_hex(&request_bytes),
            result_sha256: sha256_hex(&worker_bytes),
        });
    }
    report.history_energy_response = worker.history_energy_response;
    if !report.solids.is_empty() {
        report.validation = ValidationMarks::for_catalogue(DESIGN_HEATING_OUTPUTS);
    }
    report.notices.push("The reported peak is the highest sampled ring voxel resolved by this tally; unsampled sub-voxel peaks may be higher.".into());
    report.notices.push("The voxel volume estimator uses ray sampling. This adapter records ray count but does not provide its volume standard error; that geometric uncertainty is not included in the Monte Carlo heating error.".into());
    write_report(&run_dir, &report)?;
    Ok(report)
}

/// Convert one validated worker solid into its report form, preserving the worker voxel volume
/// as the averaging volume for both the peak density and its Monte Carlo standard error.
fn worker_solid_to_heating(
    worker_solid: WorkerSolid,
    source_rate: f64,
) -> Result<SolidHeating, DesignError> {
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
    Ok(SolidHeating {
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
    })
}

/// Bind a completed whole-model heat report to one previously validated CAD history-rate
/// artifact. The separate-run heat/H3/flux covariance is recorded as unknown.
pub fn combine_history_energy_rates(
    base_history_rates_path: &Path,
    heating_report_path: &Path,
    scenario: &faris_model::LoadedScenario,
    output_path: &Path,
) -> Result<HistoryEnergyRatesArtifact, String> {
    let artifact = build_history_energy_rates(
        base_history_rates_path,
        heating_report_path,
        EnergySourceContext::LegacyScenario(scenario),
    )?;
    write_history_energy_rates(&artifact, output_path)?;
    Ok(artifact)
}

/// Bind whole-model heating to CAD history rates using the checked CAD design identity.
/// Operating-history assumptions do not enter the transport response identity.
pub fn combine_cad_history_energy_rates(
    base_history_rates_path: &Path,
    heating_report_path: &Path,
    design_path: &Path,
    output_path: &Path,
) -> Result<HistoryEnergyRatesArtifact, String> {
    let identity = super::transport::resolve_cad_source_identity(design_path)?;
    let artifact = build_history_energy_rates(
        base_history_rates_path,
        heating_report_path,
        EnergySourceContext::CadDesign {
            identity: &identity,
            design_path,
        },
    )?;
    write_history_energy_rates(&artifact, output_path)?;
    Ok(artifact)
}

fn write_history_energy_rates(
    artifact: &HistoryEnergyRatesArtifact,
    output_path: &Path,
) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(artifact).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err("combined history-energy receipt exceeds the JSON size limit".into());
    }
    crate::atomic_file::write_new(output_path, &bytes)
        .map_err(|e| format!("{}: {e}", output_path.display()))?;
    Ok(())
}

/// Revalidate the base rates, heating report and both their physical input identities before
/// returning rates for the shared operating-history ledger.
pub fn load_history_energy_rates(
    path: &Path,
    scenario: &faris_model::LoadedScenario,
) -> Result<TransportDrivingRates, String> {
    load_history_energy_rates_impl(path, EnergySourceContext::LegacyScenario(scenario))
}

/// Load and revalidate a CAD-native heating receipt against its checked design.
pub fn load_cad_history_energy_rates(
    path: &Path,
    design_path: &Path,
) -> Result<TransportDrivingRates, String> {
    let identity = super::transport::resolve_cad_source_identity(design_path)?;
    load_history_energy_rates_impl(
        path,
        EnergySourceContext::CadDesign {
            identity: &identity,
            design_path,
        },
    )
}

enum EnergySourceContext<'a> {
    LegacyScenario(&'a faris_model::LoadedScenario),
    CadDesign {
        identity: &'a faris_model::design::CadSourceIdentity,
        design_path: &'a Path,
    },
}

fn load_history_energy_rates_impl(
    path: &Path,
    context: EnergySourceContext<'_>,
) -> Result<TransportDrivingRates, String> {
    let bytes = read_bounded(path).map_err(|e| e.to_string())?;
    let artifact: HistoryEnergyRatesArtifact = serde_json::from_slice(&bytes)
        .map_err(|e| format!("CAD history-energy artifact JSON: {e}"))?;
    let expected = build_history_energy_rates(
        Path::new(&artifact.base_rates_path),
        Path::new(&artifact.heating_report_path),
        context,
    )?;
    if expected != artifact {
        return Err("CAD history-energy artifact differs from its hash-bound source runs".into());
    }
    Ok(artifact.driving_rates)
}

fn build_history_energy_rates(
    base_history_rates_path: &Path,
    heating_report_path: &Path,
    context: EnergySourceContext<'_>,
) -> Result<HistoryEnergyRatesArtifact, String> {
    let base_path = base_history_rates_path
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let report_path = heating_report_path
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let base_bytes = read_bounded(&base_path).map_err(|e| e.to_string())?;
    let base_sha = sha256_hex(&base_bytes);
    let base: super::transport::DesignHistoryRatesArtifact = serde_json::from_slice(&base_bytes)
        .map_err(|e| format!("CAD history-rate artifact JSON: {e}"))?;
    let (rates, schema_version, scenario_id, scenario_sha256, cad_source_identity) = match context {
        EnergySourceContext::LegacyScenario(scenario) => {
            let rates = super::transport::load_design_history_rates(&base_path, scenario)?;
            if base.schema_version != super::transport::DESIGN_HISTORY_RATES_VERSION {
                return Err("legacy history-energy receipt requires CAD history rates v0.1".into());
            }
            if Some(scenario.source_sha256.as_str()) != base.scenario_sha256.as_deref()
                || base.scenario_id.as_deref() != Some(scenario.scenario.id.as_str())
            {
                return Err("legacy history scenario identity differs from its CAD rates".into());
            }
            (
                rates,
                HISTORY_ENERGY_RATES_VERSION,
                Some(scenario.scenario.id.clone()),
                Some(scenario.source_sha256.clone()),
                None,
            )
        }
        EnergySourceContext::CadDesign {
            identity: expected_identity,
            design_path,
        } => {
            let rates = super::transport::load_cad_design_history_rates(&base_path, design_path)?;
            let identity = base
                .cad_source_identity
                .as_ref()
                .ok_or("CAD history-energy receipt requires typed CAD source identity")?;
            if base.schema_version != super::transport::DESIGN_HISTORY_RATES_CAD_VERSION
                || identity != expected_identity
            {
                return Err("CAD design source identity differs from its history rates".into());
            }
            (
                rates,
                HISTORY_ENERGY_RATES_CAD_VERSION,
                None,
                None,
                Some(identity.clone()),
            )
        }
    };
    let report_bytes = read_bounded(&report_path).map_err(|e| e.to_string())?;
    let report_sha = sha256_hex(&report_bytes);
    let report: DesignHeatingReport = serde_json::from_slice(&report_bytes)
        .map_err(|e| format!("CAD heating report JSON: {e}"))?;
    let response = report
        .history_energy_response
        .as_ref()
        .ok_or("heating report has no whole-model history-energy response")?;
    let (worker_request_path, worker_request_bytes, worker_result_path, worker_result_bytes) =
        verify_history_energy_worker_run(&report, response, &base, &base_path)?;
    let worker_request: serde_json::Value = serde_json::from_slice(&worker_request_bytes)
        .map_err(|e| format!("heating request JSON: {e}"))?;
    let heating_run_dir = Path::new(&report.run_directory);
    let heating_runtime_valid = report
        .runtime_identity
        .as_ref()
        .is_some_and(|identity| verify_runtime_identity(identity, heating_run_dir));
    let heating_execution_valid = report
        .execution_identity
        .as_ref()
        .is_some_and(|identity| verify_execution_identity(identity, heating_run_dir));
    if report.schema_version != RESULT_SCHEMA
        || response.status != "CALCULATED"
        || !response.target_reached
        || !response.closure_residual.within_3se
        || response.component_map != base.component_map
        || response.source_definition_sha256 != base.source_definition_sha256
        || response.source_kernel_path != base.source_kernel_path
        || !response
            .source_kernel_sha256
            .eq_ignore_ascii_case(&base.source_kernel_sha256)
        || report.design.sha256 != base.design_sha256
        || report.check_report_sha256 != base.check_report_sha256
        || report.h5m_sha256 != base.h5m_sha256
        || report.cross_sections_sha256 != base.cross_sections_sha256
        || report.runtime_identity.as_ref().is_none_or(|runtime| {
            runtime.openmc_c_api_library.path != base.openmc_library_path
                || !runtime
                    .openmc_c_api_library
                    .sha256
                    .eq_ignore_ascii_case(&base.openmc_library_sha256)
        })
        || !heating_runtime_valid
        || !heating_execution_valid
        || !report.source_rate_per_s.is_finite()
        || report.source_rate_per_s != base.source_neutron_rate_per_s
        || report.data_identity.is_none()
        || rates.transport_deposited_heat_w.is_some()
    {
        return Err(
            "heating result is not a converged whole-model response for this exact CAD history run"
                .into(),
        );
    }
    let data_identity = report.data_identity.clone().expect("checked above");
    if !heating_neutron_data_matches(&base, &data_identity) {
        return Err(
            "heating neutron MT=301 HDF hashes differ from the CAD history-rate run".into(),
        );
    }
    let runtime = report
        .runtime_identity
        .as_ref()
        .expect("runtime checked above");
    let execution = report
        .execution_identity
        .as_ref()
        .expect("execution checked above");
    let nuclear_data_files = supplemental_nuclear_data_files(&data_identity)?;
    let heat = ScalarRate {
        mean: response.whole_model.mean_ev_per_source
            * report.source_rate_per_s
            * ELEMENTARY_CHARGE_C,
        standard_error: Some(
            response.whole_model.standard_error_ev_per_source
                * report.source_rate_per_s
                * ELEMENTARY_CHARGE_C,
        ),
        unit: "W".into(),
        response_id: "heating-total-whole-model".into(),
    };
    if !heat.mean.is_finite()
        || heat.mean < 0.0
        || heat
            .standard_error
            .is_none_or(|se| !se.is_finite() || se < 0.0)
    {
        return Err("whole-model heating conversion to watts is non-finite or negative".into());
    }
    let mut driving_rates = rates;
    driving_rates.transport_deposited_heat_w = Some(heat);
    driving_rates.supplemental_heating = Some(crate::history::SupplementalHeatingProvenance {
        heating_report_path: report_path.display().to_string(),
        heating_report_sha256: report_sha.clone(),
        worker_request_path: worker_request_path.display().to_string(),
        worker_request_sha256: sha256_hex(&worker_request_bytes),
        worker_result_path: worker_result_path.display().to_string(),
        worker_result_sha256: sha256_hex(&worker_result_bytes),
        worker_helper_path: runtime.worker_helper.path.clone(),
        worker_helper_sha256: runtime.worker_helper.sha256.clone(),
        source_kernel_path: response.source_kernel_path.clone(),
        source_kernel_sha256: response.source_kernel_sha256.clone(),
        openmc_version: runtime.openmc_version.clone(),
        openmc_library_path: runtime.openmc_c_api_library.path.clone(),
        openmc_library_sha256: runtime.openmc_c_api_library.sha256.clone(),
        h5m_path: execution.run_h5m.path.clone(),
        h5m_sha256: execution.run_h5m.sha256.clone(),
        model_xml_path: execution.model_xml.path.clone(),
        model_xml_sha256: execution.model_xml.sha256.clone(),
        cross_sections_path: worker_request["cross_sections"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        cross_sections_sha256: report.cross_sections_sha256.clone(),
        source_definition_sha256: response.source_definition_sha256.clone(),
        component_map_sha256: response.component_map_sha256.clone(),
        response_id: "heating-total-whole-model".into(),
        domain: response.domain.clone(),
        normalization: response.normalization.clone(),
        source_neutron_rate_per_s: report.source_rate_per_s,
        unit: "W".into(),
        nuclear_data_files,
    });
    if let Some(covariance) = driving_rates.covariance.as_mut() {
        covariance
            .unknown_rate_ids
            .push("heating-total-whole-model".into());
        covariance.method = format!(
            "{}; separate heating run covariance unknown",
            covariance.method
        );
    }
    driving_rates.validate()?;
    let canonical_file = |path: &Path| -> Result<String, String> {
        if !path.is_file() {
            return Err(format!("{} is not a regular file", path.display()));
        }
        Ok(path
            .canonicalize()
            .map_err(|e| e.to_string())?
            .display()
            .to_string())
    };
    if sha256_file(&base_path).map_err(|e| e.to_string())? != base_sha
        || sha256_file(&report_path).map_err(|e| e.to_string())? != report_sha
        || sha256_file(&worker_request_path).map_err(|e| e.to_string())?
            != sha256_hex(&worker_request_bytes)
        || sha256_file(&worker_result_path).map_err(|e| e.to_string())?
            != sha256_hex(&worker_result_bytes)
    {
        return Err(
            "a source report changed while the combined history-energy receipt was being checked"
                .into(),
        );
    }
    Ok(HistoryEnergyRatesArtifact {
        schema_version: schema_version.into(),
        scenario_id,
        scenario_sha256,
        cad_source_identity,
        base_rates_path: canonical_file(&base_path)?,
        base_rates_sha256: base_sha,
        heating_report_path: canonical_file(&report_path)?,
        heating_report_sha256: report_sha,
        design_sha256: base.design_sha256,
        check_report_sha256: base.check_report_sha256,
        h5m_path: base.h5m_path,
        h5m_sha256: base.h5m_sha256,
        cross_sections_path: base.cross_sections_path,
        cross_sections_sha256: base.cross_sections_sha256,
        source_definition_sha256: base.source_definition_sha256,
        heating_data_identity: data_identity,
        covariance_scope: "base flux/H3 same-run covariance preserved; separate whole-model heating covariance to flux and H3 is unknown and ensemble propagation refuses this artifact".into(),
        driving_rates,
        validation: ValidationMarks::for_catalogue(HISTORY_ENERGY_RATES_OUTPUTS),
    })
}

fn verify_history_energy_worker_run(
    report: &DesignHeatingReport,
    response: &HistoryEnergyResponse,
    base: &super::transport::DesignHistoryRatesArtifact,
    base_path: &Path,
) -> Result<(PathBuf, Vec<u8>, PathBuf, Vec<u8>), String> {
    let receipt = report
        .history_energy_worker_receipt
        .as_ref()
        .ok_or("heating report lacks hashes for its retained worker request/result")?;
    let run_dir = Path::new(&report.run_directory)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let resolve = |name: &str| -> Result<PathBuf, String> {
        let relative = Path::new(name);
        if relative.is_absolute()
            || relative
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err("heating report has a non-normalized worker artifact path".into());
        }
        let path = run_dir
            .join(relative)
            .canonicalize()
            .map_err(|e| e.to_string())?;
        if !path.starts_with(&run_dir) || !path.is_file() {
            return Err(
                "heating worker artifact escapes its run directory or is not a regular file".into(),
            );
        }
        Ok(path)
    };
    let request_path = resolve("request.json")?;
    let result_path = resolve("solver/heating-result.json")?;
    let request_bytes = read_bounded(&request_path).map_err(|e| e.to_string())?;
    let result_bytes = read_bounded(&result_path).map_err(|e| e.to_string())?;
    if sha256_hex(&request_bytes) != receipt.request_sha256
        || sha256_hex(&result_bytes) != receipt.result_sha256
    {
        return Err("raw heating worker request/result differs from its report receipt".into());
    }
    let request: serde_json::Value =
        serde_json::from_slice(&request_bytes).map_err(|e| e.to_string())?;
    let result: WorkerResult = serde_json::from_slice(&result_bytes).map_err(|e| e.to_string())?;
    let base_request_path = checked_base_artifact_path(base_path, &base.transport_request_path)?;
    let base_request_bytes = read_bounded(&base_request_path).map_err(|e| e.to_string())?;
    if sha256_hex(&base_request_bytes) != base.transport_request_sha256 {
        return Err("base CAD transport request hash changed".into());
    }
    let base_request: serde_json::Value =
        serde_json::from_slice(&base_request_bytes).map_err(|e| e.to_string())?;
    let target = request["sampling"]["target_relative_error"]
        .as_f64()
        .ok_or("heating request omits its sampling target")?;
    let max_batches = request["sampling"]["max_batches"]
        .as_u64()
        .ok_or("heating request omits its batch bound")?;
    let particles_per_batch = request["sampling"]["particles_per_batch"]
        .as_u64()
        .ok_or("heating request omits batch size")?;
    let max_seconds = request["sampling"]["max_seconds"]
        .as_u64()
        .ok_or("heating request omits time bound")?;
    let threads = request["sampling"]["threads"]
        .as_u64()
        .ok_or("heating request omits thread bound")?;
    let _seed = request["sampling"]["seed"]
        .as_u64()
        .ok_or("heating request omits its random seed")?;
    let Some(history_request) = request["history_energy_response"].as_object() else {
        return Err("heating worker request omitted the history energy request".into());
    };
    if request["schema"].as_str() != Some(REQUEST_SCHEMA)
        || request["heating_mode"].as_bool() != Some(true)
        || request["h5m"] != base_request["h5m"]
        || request["cross_sections"] != base_request["cross_sections"]
        || request["plasma"] != base_request["plasma"]
        || request["solids"] != base_request["solids"]
        || request["materials"] != base_request["materials"]
        || request["response"]["energy_min_ev"].as_f64() != Some(0.0)
        || request["sampling"]["min_batches"].as_u64() != Some(MIN_BATCHES)
        || particles_per_batch == 0
        || particles_per_batch
            .checked_mul(max_batches)
            .is_none_or(|n| n > MAX_HEATING_HISTORIES)
        || max_seconds == 0
        || max_seconds > MAX_HEATING_SECONDS
        || threads == 0
        || threads > 32
        || !(0.0..1.0).contains(&target)
        || !(MIN_BATCHES..=MAX_HEATING_BATCHES).contains(&max_batches)
        || history_request.get("component_map")
            != Some(&serde_json::to_value(&base.component_map).map_err(|e| e.to_string())?)
        || history_request
            .get("source_definition_sha256")
            .and_then(serde_json::Value::as_str)
            != Some(base.source_definition_sha256.as_str())
        || history_request
            .get("source_kernel_path")
            .and_then(serde_json::Value::as_str)
            != Some(base.source_kernel_path.as_str())
        || history_request
            .get("source_kernel_sha256")
            .and_then(serde_json::Value::as_str)
            != Some(base.source_kernel_sha256.as_str())
        || !history_energy_result_response_matches(&result, response)
        || result.status.as_deref() != Some("CALCULATED")
        || result.histories != response.histories
        || result.batches != response.batches
        || result.histories != particles_per_batch.saturating_mul(result.batches)
        || result.batches > max_batches
        || result.target_relative_error != Some(target)
        || !result.target_reached
        || result.stop_reason != "target_reached"
        || result.data_identity != report.data_identity
        || result.runtime_identity != report.runtime_identity
        || result.execution_identity != report.execution_identity
        || !history_energy_response_valid(
            response,
            &HistoryEnergyRequest {
                component_map: base.component_map.clone(),
                source_definition_sha256: base.source_definition_sha256.clone(),
                design_sha256: base.design_sha256.clone(),
                check_report_sha256: base.check_report_sha256.clone(),
                h5m_sha256: base.h5m_sha256.clone(),
                h5m_path: base.h5m_path.clone(),
                cross_sections_sha256: base.cross_sections_sha256.clone(),
                cross_sections_path: base.cross_sections_path.clone(),
                source_kernel_path: base.source_kernel_path.clone(),
                source_kernel_sha256: base.source_kernel_sha256.clone(),
            },
            result.histories,
            result.batches,
            target,
        )
    {
        return Err("raw heating worker request/result does not reproduce the report's validated CAD whole-model response".into());
    }
    Ok((request_path, request_bytes, result_path, result_bytes))
}

fn history_energy_result_response_matches(
    worker: &WorkerResult,
    report_response: &HistoryEnergyResponse,
) -> bool {
    worker.history_energy_response.as_ref() == Some(report_response)
}

fn checked_base_artifact_path(base_path: &Path, relative: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    if path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("base CAD run has a non-normalized request path".into());
    }
    let parent = base_path
        .parent()
        .ok_or("base CAD artifact has no parent")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let resolved = parent
        .join(path)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !resolved.starts_with(&parent) || !resolved.is_file() {
        return Err("base CAD request escapes its run directory or is not a regular file".into());
    }
    Ok(resolved)
}

fn supplemental_nuclear_data_files(
    identity: &serde_json::Value,
) -> Result<Vec<crate::history::SupplementalNuclearDataFile>, String> {
    let mut files = Vec::new();
    for (scope, map_key, names) in [
        ("neutron", "neutron_mt301", "nuclides"),
        ("photon", "photon", "elements"),
    ] {
        let Some(entries) = identity.get(map_key).and_then(serde_json::Value::as_object) else {
            return Err(format!(
                "heating nuclear-data receipt omits {scope} identities"
            ));
        };
        for (name, entry) in entries {
            let path = entry
                .get("path")
                .and_then(serde_json::Value::as_str)
                .ok_or("nuclear-data path missing")?;
            let sha256 = entry
                .get("sha256")
                .and_then(serde_json::Value::as_str)
                .ok_or("nuclear-data hash missing")?;
            let canonical = Path::new(path).canonicalize().map_err(|e| e.to_string())?;
            let before = canonical.metadata().map_err(|e| e.to_string())?;
            if !before.is_file() || before.len() == 0 || before.len() > MAX_RECEIPT_FILE_BYTES {
                return Err(
                    "heating nuclear-data file size is outside its bounded receipt range".into(),
                );
            }
            let actual = bounded_file_sha256(&canonical, MAX_RECEIPT_FILE_BYTES)
                .map_err(|e| e.to_string())?;
            let after = canonical.metadata().map_err(|e| e.to_string())?;
            if canonical.display().to_string() != path
                || before.len() != after.len()
                || before.modified().ok() != after.modified().ok()
                || !actual.eq_ignore_ascii_case(sha256)
            {
                return Err("heating nuclear-data file identity changed".into());
            }
            files.push(crate::history::SupplementalNuclearDataFile {
                scope: scope.into(),
                path: path.into(),
                bytes: after.len(),
                sha256: sha256.into(),
                nuclides: if names == "nuclides" {
                    vec![name.clone()]
                } else {
                    Vec::new()
                },
                elements: if names == "elements" {
                    vec![name.clone()]
                } else {
                    Vec::new()
                },
            });
        }
    }
    Ok(files)
}

fn heating_neutron_data_matches(
    base: &super::transport::DesignHistoryRatesArtifact,
    heating_identity: &serde_json::Value,
) -> bool {
    if heating_identity
        .get("atomic_relaxation")
        .and_then(serde_json::Value::as_str)
        != Some("enabled")
        || heating_identity
            .get("electron_treatment")
            .and_then(serde_json::Value::as_str)
            != Some("led")
    {
        return false;
    }
    let Some(heating_tables) = heating_identity
        .get("neutron_mt301")
        .and_then(serde_json::Value::as_object)
    else {
        return false;
    };
    let base_nuclides = base
        .nuclear_data_identity
        .tables
        .iter()
        .flat_map(|table| {
            table
                .nuclides
                .iter()
                .map(move |nuclide| (nuclide, (table.path.as_str(), table.sha256.as_str())))
        })
        .collect::<BTreeMap<_, _>>();
    if base_nuclides.len() != heating_tables.len() {
        return false;
    }
    let neutron_matches = base_nuclides.iter().all(|(nuclide, (path, digest))| {
        heating_tables.get(*nuclide).is_some_and(|entry| {
            entry.get("path").and_then(serde_json::Value::as_str) == Some(*path)
                && entry.get("mt301").and_then(serde_json::Value::as_bool) == Some(true)
                && entry
                    .get("sha256")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|actual| actual.eq_ignore_ascii_case(digest))
        })
    });
    if !neutron_matches {
        return false;
    }
    let Some(photons) = heating_identity
        .get("photon")
        .and_then(serde_json::Value::as_object)
    else {
        return false;
    };
    let expected_elements = base_nuclides
        .keys()
        .map(|nuclide| {
            nuclide
                .chars()
                .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_lowercase())
                .collect::<String>()
        })
        .collect::<std::collections::BTreeSet<_>>();
    if photons
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        != expected_elements
    {
        return false;
    }
    let mut total_bytes = 0_u64;
    for entry in photons.values() {
        let Some(path_text) = entry.get("path").and_then(serde_json::Value::as_str) else {
            return false;
        };
        let Some(expected_sha) = entry.get("sha256").and_then(serde_json::Value::as_str) else {
            return false;
        };
        let Some(shell_count) = entry.get("shell_count").and_then(serde_json::Value::as_u64) else {
            return false;
        };
        let path = Path::new(path_text);
        let Ok(canonical) = path.canonicalize() else {
            return false;
        };
        let Ok(metadata) = canonical.metadata() else {
            return false;
        };
        if !path.is_absolute()
            || canonical != path
            || !metadata.is_file()
            || metadata.len() == 0
            || metadata.len() > 12 * 1024 * 1024 * 1024
            || shell_count == 0
        {
            return false;
        }
        let Ok(before) = canonical.metadata() else {
            return false;
        };
        let Ok(actual_sha) = bounded_file_sha256(&canonical, 12 * 1024 * 1024 * 1024) else {
            return false;
        };
        let Ok(after) = canonical.metadata() else {
            return false;
        };
        if before.len() != after.len()
            || before.modified().ok() != after.modified().ok()
            || !actual_sha.eq_ignore_ascii_case(expected_sha)
        {
            return false;
        }
        total_bytes = total_bytes.saturating_add(metadata.len());
        if total_bytes > 64 * 1024 * 1024 * 1024 {
            return false;
        }
    }
    true
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
    score.mean_ev_per_source.abs() <= gate
        && score.within_3se == Some(true)
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
        Some(1) => {
            batches == planned_batches
                && matches!(
                    worker.stop_reason.as_str(),
                    "history_limit" | "target_reached"
                )
        }
        Some(0) => {
            (worker.stop_reason == "time_limit" || worker.stop_reason == "target_reached")
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

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn history_energy_response_valid(
    response: &HistoryEnergyResponse,
    request: &HistoryEnergyRequest,
    histories: u64,
    batches: u64,
    target: f64,
) -> bool {
    let expected_map_hash = serde_json::to_vec(&request.component_map)
        .ok()
        .map(|bytes| sha256_hex(&bytes));
    let dimension = 1 + request.component_map.len();
    let scores = std::iter::once(&response.whole_model)
        .chain(
            request
                .component_map
                .keys()
                .filter_map(|key| response.components.get(key)),
        )
        .collect::<Vec<_>>();
    if response.source_definition_sha256 != request.source_definition_sha256
        || response.source_kernel_path != request.source_kernel_path
        || !response
            .source_kernel_sha256
            .eq_ignore_ascii_case(&request.source_kernel_sha256)
        || response.component_map != request.component_map
        || expected_map_hash.as_deref() != Some(response.component_map_sha256.as_str())
        || response.domain != "unfiltered_whole_model_all_material_bearing_solids"
        || response.normalization != "eV_per_unit_source_neutron"
        || response.source_strength_normalized_to != "one_source_neutron"
        || !matches!(response.status.as_str(), "CALCULATED" | "INCONCLUSIVE")
        || (response.status == "CALCULATED"
            && (!response.target_reached || !response.closure_residual.within_3se))
        || response.histories != histories
        || response.batches != batches
        || response.batches < MIN_BATCHES
        || response.target_relative_error != target
        || response
            .variance_of_variance
            .is_some_and(|v| !v.is_finite() || v < 0.0)
        || response.components.len() != request.component_map.len()
        || scores.len() != dimension
        || response.response_ids
            != std::iter::once("whole-model-heating".to_string())
                .chain(
                    request
                        .component_map
                        .keys()
                        .map(|id| format!("component-heating:{id}")),
                )
                .collect::<Vec<_>>()
        || response.covariance_of_mean_ev2.len() != dimension * dimension
        || response.covariance_method != "same-run-independent-batch-covariance-of-mean"
        || response.cross_run_covariance != "unknown_to_flux_and_breeder_H3"
        || scores.iter().any(|score| {
            !score.mean_ev_per_source.is_finite()
                || score.mean_ev_per_source < 0.0
                || !score.standard_error_ev_per_source.is_finite()
                || score.standard_error_ev_per_source < 0.0
        })
        || !response.closure_residual.mean.is_finite()
        || !response.closure_residual.standard_error.is_finite()
        || response.closure_residual.standard_error < 0.0
        || !response.closure_residual.roundoff_tolerance.is_finite()
        || response.closure_residual.roundoff_tolerance < 0.0
    {
        return false;
    }
    let covariance = &response.covariance_of_mean_ev2;
    if crate::history::factor_covariance(covariance, dimension).is_err() {
        return false;
    }
    for (index, score) in scores.iter().enumerate() {
        let variance = covariance[index * dimension + index];
        let expected = faris_model::math::powi(score.standard_error_ev_per_source, 2);
        if (variance - expected).abs() > 1.0e-6 * expected + f64::MIN_POSITIVE {
            return false;
        }
    }
    let contrast = std::iter::once(1.0)
        .chain(std::iter::repeat_n(-1.0, dimension - 1))
        .collect::<Vec<_>>();
    let Some((closure_variance, closure_abs_terms)) =
        covariance_contrast_variance(covariance, &contrast)
    else {
        return false;
    };
    let closure_expected_variance =
        faris_model::math::powi(response.closure_residual.standard_error, 2);
    let dot_operations = (dimension * dimension + 1) as f64;
    let roundoff_bound = 64.0 * f64::EPSILON * dot_operations * closure_abs_terms;
    let comparison_tolerance = 1.0e-6 * closure_expected_variance.abs().max(closure_variance.abs())
        + roundoff_bound
        + f64::MIN_POSITIVE;
    if (closure_variance - closure_expected_variance).abs() > comparison_tolerance {
        return false;
    }
    let component_sum = response
        .components
        .values()
        .map(|s| s.mean_ev_per_source)
        .sum::<f64>();
    let expected_residual = response.whole_model.mean_ev_per_source - component_sum;
    let scale = response.whole_model.mean_ev_per_source.abs()
        + response
            .components
            .values()
            .map(|s| s.mean_ev_per_source.abs())
            .sum::<f64>();
    let roundoff = 64.0 * f64::EPSILON * scale.max(1.0e-300);
    let closure_limit = 3.0 * response.closure_residual.standard_error + roundoff;
    response.closure_residual.within_3se == (response.closure_residual.mean.abs() <= closure_limit)
        && (response.closure_residual.roundoff_tolerance - roundoff).abs()
            <= roundoff.max(1.0e-300) * 1.0e-8
        && (response.closure_residual.mean - expected_residual).abs()
            <= 1.0e-8 * scale.max(1.0e-300)
        && (!response.target_reached
            || (response.status == "CALCULATED"
                && scores.iter().all(|score| {
                    score.mean_ev_per_source > 0.0
                        && score.standard_error_ev_per_source / score.mean_ev_per_source <= target
                })
                && response.variance_of_variance.is_some_and(|v| v <= 0.1)))
        && (!response.target_reached || response.closure_residual.within_3se)
}

/// Contract a covariance-of-mean matrix with a contrast vector using compensated summation.
/// The second return value bounds floating-point cancellation from the absolute term sum.
fn covariance_contrast_variance(covariance: &[f64], contrast: &[f64]) -> Option<(f64, f64)> {
    let dimension = contrast.len();
    if covariance.len() != dimension.checked_mul(dimension)?
        || contrast.iter().any(|value| !value.is_finite())
    {
        return None;
    }
    let mut sum = 0.0;
    let mut correction = 0.0;
    let mut absolute_terms = 0.0;
    for i in 0..dimension {
        for j in 0..dimension {
            let term = contrast[i] * covariance[i * dimension + j] * contrast[j];
            if !term.is_finite() {
                return None;
            }
            absolute_terms += term.abs();
            let adjusted = term - correction;
            let next = sum + adjusted;
            correction = (next - sum) - adjusted;
            sum = next;
        }
    }
    (sum.is_finite() && absolute_terms.is_finite()).then_some((sum, absolute_terms))
}

fn verify_file_identity(receipt: &HeatingFileIdentity, expected: Option<&Path>) -> bool {
    if !valid_sha256(&receipt.sha256) {
        return false;
    }
    let path = Path::new(&receipt.path);
    if !path.is_absolute() {
        return false;
    }
    let Ok(canonical) = path.canonicalize() else {
        return false;
    };
    if canonical != path
        || expected.is_some_and(|expected| {
            expected
                .canonicalize()
                .map_or(true, |expected| expected != canonical)
        })
    {
        return false;
    }
    bounded_file_sha256(&canonical, MAX_RECEIPT_FILE_BYTES)
        .is_ok_and(|digest| digest.eq_ignore_ascii_case(&receipt.sha256))
}

fn bounded_file_sha256(path: &Path, byte_limit: u64) -> Result<String, DesignError> {
    let mut file = std::fs::File::open(path).map_err(|e| io_error(path, e))?;
    if !file.metadata().map_err(|e| io_error(path, e))?.is_file() {
        return Err(DesignError::Tool(format!(
            "{} is not a regular receipt file",
            path.display()
        )));
    }
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; 1 << 20];
    loop {
        let count = file.read(&mut buffer).map_err(|e| io_error(path, e))?;
        if count == 0 {
            break;
        }
        total = total.saturating_add(count as u64);
        if total > byte_limit {
            return Err(DesignError::Tool(format!(
                "{} exceeds the bounded receipt hash limit",
                path.display()
            )));
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn verify_runtime_identity(identity: &HeatingRuntimeIdentity, run_dir: &Path) -> bool {
    !identity.openmc_version.trim().is_empty()
        && verify_file_identity(&identity.python, None)
        && verify_file_identity(&identity.openmc_c_api_library, None)
        && verify_file_identity(
            &identity.worker_helper,
            Some(&run_dir.join("design_transport.py")),
        )
}

fn verify_execution_identity(identity: &HeatingExecutionIdentity, run_dir: &Path) -> bool {
    verify_file_identity(
        &identity.run_h5m,
        Some(&run_dir.join("geometry/model-heating.h5m")),
    ) && verify_file_identity(
        &identity.model_xml,
        Some(&run_dir.join("solver/heating-model.xml")),
    )
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
        let _ = writeln!(text, "  Batches: {}", report.batches);
        if let (Some(mean_target), Some(peak_target)) = (
            report.target_relative_error,
            report.peak_target_relative_error,
        ) {
            let _ = writeln!(
                text,
                "  Error targets: mean {:.2}%; resolved per-solid peak {:.2}%",
                100.0 * mean_target,
                100.0 * peak_target
            );
        }
        if let (Some(mean), Some(peak), Some(vov)) = (
            report.mean_relative_error,
            report.peak_relative_error,
            report.variance_of_variance,
        ) {
            let _ = writeln!(
                text,
                "  Relative error: mean {:.2}%; resolved per-solid peak {:.2}%",
                100.0 * mean,
                100.0 * peak
            );
            let _ = writeln!(text, "  Batch variance-of-variance: {vov:.4} (limit 0.1)");
        }
    }
    if let Some(status) = report.c_api_terminal_status {
        let _ = writeln!(text, "  OpenMC C API terminal status: {status}");
    }
    if let Some(runtime) = &report.runtime_identity {
        let _ = writeln!(text, "  OpenMC Python package: {}", runtime.openmc_version);
        let _ = writeln!(
            text,
            "  Python runtime: {} (SHA-256 {})",
            runtime.python.path, runtime.python.sha256
        );
        let _ = writeln!(
            text,
            "  OpenMC C API library: {} (SHA-256 {})",
            runtime.openmc_c_api_library.path, runtime.openmc_c_api_library.sha256
        );
        let _ = writeln!(
            text,
            "  Worker helper: {} (SHA-256 {})",
            runtime.worker_helper.path, runtime.worker_helper.sha256
        );
    }
    if let Some(execution) = &report.execution_identity {
        let _ = writeln!(
            text,
            "  C API retagged H5M: {} (SHA-256 {})",
            execution.run_h5m.path, execution.run_h5m.sha256
        );
        let _ = writeln!(
            text,
            "  C API exported model XML: {} (SHA-256 {})",
            execution.model_xml.path, execution.model_xml.sha256
        );
    }
    if let Some(scope) = report
        .data_identity
        .as_ref()
        .and_then(|identity| identity.get("electron_treatment_scope"))
        .and_then(serde_json::Value::as_str)
    {
        let _ = writeln!(text, "  Electron treatment scope: {scope}");
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
    if let Some(energy) = &report.history_energy_response {
        let heat_w =
            energy.whole_model.mean_ev_per_source * report.source_rate_per_s * ELEMENTARY_CHARGE_C;
        let heat_se_w = energy.whole_model.standard_error_ev_per_source
            * report.source_rate_per_s
            * ELEMENTARY_CHARGE_C;
        let _ = writeln!(
            text,
            "  Whole-model history energy response: {}",
            energy.status
        );
        let _ = writeln!(
            text,
            "    all material-bearing CAD solids, unfiltered; {heat_w:.6e} ± {heat_se_w:.6e} W",
        );
        let _ = writeln!(
            text,
            "    source: one-neutron normalized; eV-to-W conversion applied once using {:.6e} source neutrons/s",
            report.source_rate_per_s
        );
        let _ = writeln!(
            text,
            "    whole-model batch VOV: {}",
            energy
                .variance_of_variance
                .map_or_else(|| "not available".into(), |v| format!("{v:.3}"))
        );
        let _ = writeln!(
            text,
            "    component partition closure: {:.6e} ± {:.6e} eV/source neutron (within 3 SE: {})",
            energy.closure_residual.mean,
            energy.closure_residual.standard_error,
            energy.closure_residual.within_3se,
        );
        let _ = writeln!(
            text,
            "    cross-run covariance with flux and breeder H3: unknown; history ensembles refuse this rate set",
        );
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
    fn heating_worker_accepts_flat_row_major_four_response_covariance() {
        let response: HistoryEnergyResponse = serde_json::from_str(include_str!(
            "../../tests/fixtures/heating-history-response.json"
        ))
        .unwrap();
        assert_eq!(response.status, "INCONCLUSIVE");
        assert!(!response.target_reached);
        assert_eq!(response.covariance_of_mean_ev2.len(), 16);
        assert_ne!(response.covariance_of_mean_ev2[1], 0.0);
        assert_ne!(response.covariance_of_mean_ev2[4], 0.0);
        let request = HistoryEnergyRequest {
            component_map: response.component_map.clone(),
            source_definition_sha256: response.source_definition_sha256.clone(),
            design_sha256: "b".repeat(64),
            check_report_sha256: "c".repeat(64),
            h5m_sha256: "d".repeat(64),
            h5m_path: "/data/model.h5m".into(),
            cross_sections_sha256: "e".repeat(64),
            cross_sections_path: "/data/cross_sections.xml".into(),
            source_kernel_path: response.source_kernel_path.clone(),
            source_kernel_sha256: response.source_kernel_sha256.clone(),
        };
        assert!(history_energy_response_valid(
            &response,
            &request,
            response.histories,
            response.batches,
            response.target_relative_error
        ));
    }

    #[test]
    fn whole_model_energy_response_requires_same_batch_partition_covariance() {
        let component_map = BTreeMap::from([("blanket".into(), vec!["blanket-cell".into()])]);
        let context = HistoryEnergyRequest {
            component_map: component_map.clone(),
            source_definition_sha256: "a".repeat(64),
            design_sha256: "b".repeat(64),
            check_report_sha256: "c".repeat(64),
            h5m_sha256: "d".repeat(64),
            h5m_path: "/data/model.h5m".into(),
            cross_sections_sha256: "e".repeat(64),
            cross_sections_path: "/data/cross_sections.xml".into(),
            source_kernel_path: "/runtime/openmc_plasma_source.py".into(),
            source_kernel_sha256: "f".repeat(64),
        };
        let map_hash = sha256_hex(&serde_json::to_vec(&component_map).unwrap());
        let response = HistoryEnergyResponse {
            status: "CALCULATED".into(),
            source_definition_sha256: context.source_definition_sha256.clone(),
            source_kernel_path: context.source_kernel_path.clone(),
            source_kernel_sha256: context.source_kernel_sha256.clone(),
            component_map,
            component_map_sha256: map_hash,
            domain: "unfiltered_whole_model_all_material_bearing_solids".into(),
            normalization: "eV_per_unit_source_neutron".into(),
            source_strength_normalized_to: "one_source_neutron".into(),
            histories: 30_000,
            batches: 30,
            target_relative_error: 0.05,
            target_reached: true,
            variance_of_variance: Some(0.02),
            whole_model: HistoryEnergyScore {
                mean_ev_per_source: 100.0,
                standard_error_ev_per_source: 1.0,
            },
            components: BTreeMap::from([(
                "blanket".into(),
                HistoryEnergyScore {
                    mean_ev_per_source: 100.0,
                    standard_error_ev_per_source: 1.0,
                },
            )]),
            response_ids: vec![
                "whole-model-heating".into(),
                "component-heating:blanket".into(),
            ],
            covariance_of_mean_ev2: vec![1.0, 1.0, 1.0, 1.0],
            covariance_method: "same-run-independent-batch-covariance-of-mean".into(),
            closure_residual: HistoryEnergyClosure {
                mean: 0.0,
                standard_error: 0.0,
                within_3se: true,
                roundoff_tolerance: 64.0 * f64::EPSILON * 200.0,
            },
            cross_run_covariance: "unknown_to_flux_and_breeder_H3".into(),
        };
        assert!(history_energy_response_valid(
            &response, &context, 30_000, 30, 0.05
        ));
        assert!(
            faris_model::validation::unclassified_numbers(
                &serde_json::to_value(&response).unwrap(),
                DESIGN_HEATING_OUTPUTS,
            )
            .is_empty()
        );
        let mut invalid = response.clone();
        invalid.covariance_of_mean_ev2 = vec![1.0, 0.0, 0.0, 1.0];
        assert!(!history_energy_response_valid(
            &invalid, &context, 30_000, 30, 0.05
        ));
        let mut wrong_partition = response;
        wrong_partition
            .components
            .get_mut("blanket")
            .unwrap()
            .mean_ev_per_source = 99.0;
        assert!(!history_energy_response_valid(
            &wrong_partition,
            &context,
            30_000,
            30,
            0.05
        ));
    }

    #[test]
    fn rank_deficient_large_covariance_allows_roundoff_but_rejects_material_closure_variance() {
        let component_map = BTreeMap::from([
            ("first".into(), vec!["cell-a".into()]),
            ("second".into(), vec!["cell-b".into()]),
        ]);
        let context = HistoryEnergyRequest {
            component_map: component_map.clone(),
            source_definition_sha256: "a".repeat(64),
            design_sha256: "b".repeat(64),
            check_report_sha256: "c".repeat(64),
            h5m_sha256: "d".repeat(64),
            h5m_path: "/run/model.h5m".into(),
            cross_sections_sha256: "e".repeat(64),
            cross_sections_path: "/data/cross_sections.xml".into(),
            source_kernel_path: "/runtime/openmc_plasma_source.py".into(),
            source_kernel_sha256: "f".repeat(64),
        };
        let map_hash = sha256_hex(&serde_json::to_vec(&component_map).unwrap());
        let a: f64 = 3.0e100;
        let b: f64 = 2.0e100;
        let c: f64 = -0.5e100;
        // The whole-model response is exactly the sum of the two component
        // responses. Component responses are anticorrelated, so the covariance
        // matrix is rank deficient with both positive and negative off-diagonals.
        let covariance = vec![a + b + 2.0 * c, a + c, b + c, a + c, a, c, b + c, c, b];
        let response = HistoryEnergyResponse {
            status: "CALCULATED".into(),
            source_definition_sha256: context.source_definition_sha256.clone(),
            source_kernel_path: context.source_kernel_path.clone(),
            source_kernel_sha256: context.source_kernel_sha256.clone(),
            component_map: component_map.clone(),
            component_map_sha256: map_hash,
            domain: "unfiltered_whole_model_all_material_bearing_solids".into(),
            normalization: "eV_per_unit_source_neutron".into(),
            source_strength_normalized_to: "one_source_neutron".into(),
            histories: 30_000,
            batches: 30,
            target_relative_error: 0.05,
            target_reached: true,
            variance_of_variance: Some(0.02),
            whole_model: HistoryEnergyScore {
                mean_ev_per_source: 3.0e52,
                standard_error_ev_per_source: (a + b + 2.0 * c).sqrt(),
            },
            components: BTreeMap::from([
                (
                    "first".into(),
                    HistoryEnergyScore {
                        mean_ev_per_source: 1.0e52,
                        standard_error_ev_per_source: a.sqrt(),
                    },
                ),
                (
                    "second".into(),
                    HistoryEnergyScore {
                        mean_ev_per_source: 2.0e52,
                        standard_error_ev_per_source: b.sqrt(),
                    },
                ),
            ]),
            response_ids: vec![
                "whole-model-heating".into(),
                "component-heating:first".into(),
                "component-heating:second".into(),
            ],
            covariance_of_mean_ev2: covariance.clone(),
            covariance_method: "same-run-independent-batch-covariance-of-mean".into(),
            closure_residual: HistoryEnergyClosure {
                mean: 0.0,
                standard_error: 0.0,
                within_3se: true,
                roundoff_tolerance: 64.0 * f64::EPSILON * 6.0e52,
            },
            cross_run_covariance: "unknown_to_flux_and_breeder_H3".into(),
        };
        assert!(history_energy_response_valid(
            &response, &context, 30_000, 30, 0.05
        ));

        let mut wrong = response;
        let wrong_total_variance = a + b + 2.0 * c + 1.0e96;
        wrong.covariance_of_mean_ev2[0] = wrong_total_variance;
        wrong.whole_model.standard_error_ev_per_source = wrong_total_variance.sqrt();
        assert!(!history_energy_response_valid(
            &wrong, &context, 30_000, 30, 0.05
        ));
    }

    #[test]
    fn report_cannot_replace_worker_response_and_history_energy_numbers_are_classified() {
        let response = HistoryEnergyResponse {
            status: "CALCULATED".into(),
            source_definition_sha256: "a".repeat(64),
            source_kernel_path: "/run/kernel.py".into(),
            source_kernel_sha256: "b".repeat(64),
            component_map: BTreeMap::from([("blanket".into(), vec!["blanket-solid".into()])]),
            component_map_sha256: "c".repeat(64),
            domain: "unfiltered_whole_model_all_material_bearing_solids".into(),
            normalization: "eV_per_unit_source_neutron".into(),
            source_strength_normalized_to: "one_source_neutron".into(),
            histories: 30_000,
            batches: 30,
            target_relative_error: 0.05,
            target_reached: true,
            variance_of_variance: Some(0.02),
            whole_model: HistoryEnergyScore {
                mean_ev_per_source: 100.0,
                standard_error_ev_per_source: 1.0,
            },
            components: BTreeMap::from([(
                "blanket".into(),
                HistoryEnergyScore {
                    mean_ev_per_source: 70.0,
                    standard_error_ev_per_source: 1.0,
                },
            )]),
            response_ids: vec!["whole".into(), "blanket".into()],
            covariance_of_mean_ev2: vec![1.0, 1.0, 1.0, 1.0],
            covariance_method: "same-run-independent-batch-covariance-of-mean".into(),
            closure_residual: HistoryEnergyClosure {
                mean: 30.0,
                standard_error: 0.0,
                within_3se: true,
                roundoff_tolerance: 1.0e-10,
            },
            cross_run_covariance: "unknown_to_flux_and_breeder_H3".into(),
        };
        let worker: WorkerResult = serde_json::from_value(serde_json::json!({
            "schema": RESULT_SCHEMA,
            "history_energy_response": response,
        }))
        .unwrap();
        assert!(history_energy_result_response_matches(
            &worker,
            worker.history_energy_response.as_ref().unwrap()
        ));
        let mut report_tamper = worker.history_energy_response.clone().unwrap();
        report_tamper.whole_model.mean_ev_per_source += 1.0;
        assert!(!history_energy_result_response_matches(
            &worker,
            &report_tamper
        ));

        use faris_model::validation::{Class, Quantity};
        let outputs = HISTORY_ENERGY_RATES_OUTPUTS;
        assert!(matches!(
            outputs
                .iter()
                .find(|o| o.name == "transport_deposited_heat_w")
                .unwrap()
                .class,
            Class::Calculated(Quantity::Heating)
        ));
        assert!(matches!(
            outputs
                .iter()
                .find(|o| o.name == "component_average_flux_n_m2_s")
                .unwrap()
                .class,
            Class::Calculated(Quantity::Flux)
        ));
        assert!(matches!(
            outputs
                .iter()
                .find(|o| o.name == "breeder_h3_per_source_neutron")
                .unwrap()
                .class,
            Class::Calculated(Quantity::Tbr)
        ));
        let marks = ValidationMarks::for_catalogue(outputs);
        assert!(marks.0.contains_key("transport_deposited_heat_w"));
        assert!(marks.0.contains_key("component_average_flux_n_m2_s"));
        assert!(marks.0.contains_key("breeder_h3_per_source_neutron"));
        let example = serde_json::json!({
            "driving_rates": {
                "transport_deposited_heat_w": {"mean": 42.0, "standard_error": 2.0},
                "component_average_flux_n_m2_s": {"blanket": {"mean": 3.0, "standard_error": 0.1}},
                "breeder_h3_per_source_neutron": {"mean": 0.9, "standard_error": 0.01},
                "covariance": {"matrix": [1.0]},
                "source_neutron_rate_per_s": 1.0e18,
                "supplemental_heating": {"source_neutron_rate_per_s": 1.0e18, "nuclear_data_files": [{"bytes": 100}]}
            }
        });
        assert!(faris_model::validation::unclassified_numbers(&example, outputs).is_empty());
    }

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
        let solid = serde_json::json!({
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
        });
        let raw = serde_json::json!({
            "schema": RESULT_SCHEMA,
            "status": "INCONCLUSIVE",
            "why": "closure outside three SE",
            "data_identity": {"electron_treatment_scope": "LED local energy deposition approximation"},
            "runtime_identity": {
                "python": {"path": "/runtime/bin/python", "sha256": "p".repeat(64)},
                "openmc_version": "0.16.0",
                "openmc_c_api_library": {"path": "/runtime/lib/libopenmc.so", "sha256": "l".repeat(64)},
                "worker_helper": {"path": "/run/design_transport.py", "sha256": "w".repeat(64)}
            },
            "execution_identity": {
                "run_h5m": {"path": "/run/geometry/model-heating.h5m", "sha256": "g".repeat(64)},
                "model_xml": {"path": "/run/solver/heating-model.xml", "sha256": "m".repeat(64)}
            },
            "c_api_terminal_status": 1,
            "lost_particles": 0,
            "histories": 100_000,
            "batches": 100,
            "target_relative_error": 0.05,
            "peak_target_relative_error": 0.1,
            "target_reached": false,
            "mean_relative_error": 0.2,
            "peak_relative_error": null,
            "variance_of_variance": 0.03,
            "stop_reason": "history_limit",
            "h5m_sha256": "h",
            "cross_sections_sha256": "x",
            "solids": [solid]
        });
        let worker: WorkerResult = serde_json::from_value(raw).unwrap();
        assert_eq!(worker.status.as_deref(), Some("INCONCLUSIVE"));
        assert_eq!(worker.c_api_terminal_status, Some(1));
        assert_eq!(worker.lost_particles, Some(0));
        assert_eq!(worker.batches, 100);
        assert_eq!(worker.target_relative_error, Some(0.05));
        assert!(!worker.target_reached);
        assert_eq!(
            worker.runtime_identity.as_ref().unwrap().openmc_version,
            "0.16.0"
        );
        assert_eq!(
            worker.execution_identity.as_ref().unwrap().model_xml.sha256,
            "m".repeat(64)
        );
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
    fn heating_progress_parser_requires_and_preserves_batch_metrics() {
        let progress = parse_heating_progress(
            r#"{"progress":{"batches":30,"histories":600000,"relative_error":0.04,"peak_relative_error":0.08,"variance_of_variance":0.02,"elapsed_seconds":12.5}}"#,
        )
        .unwrap();
        assert_eq!(progress.batches, 30);
        assert_eq!(progress.histories, 600_000);
        assert_eq!(progress.mean_relative_error, Some(0.04));
        assert_eq!(progress.peak_relative_error, Some(0.08));
        assert_eq!(progress.variance_of_variance, Some(0.02));
        assert!(parse_heating_progress(r#"{"progress":{"batches":30}}"#).is_none());
    }

    #[test]
    fn peak_sampling_quality_requires_positive_mean_and_at_most_ten_percent_error() {
        assert!(!heating_peak_is_converged(0.0, Some(0.0)));
        assert!(!heating_peak_is_converged(1.0, None));
        assert!(heating_peak_is_converged(1.0, Some(0.10)));
        assert!(!heating_peak_is_converged(1.0, Some(0.100_001)));
    }

    #[test]
    fn preflight_parser_retains_selected_runtime_and_led_scope() {
        let preflight: WorkerPreflight = serde_json::from_value(serde_json::json!({
            "schema": "faris-design-heating-preflight/v1",
            "status": "READY",
            "h5m_sha256": "a".repeat(64),
            "cross_sections_sha256": "b".repeat(64),
            "data_identity": {"electron_treatment": "led",
                               "electron_treatment_scope": "LED local energy deposition approximation"},
            "runtime_identity": {
                "python": {"path": "/runtime/bin/python", "sha256": "p".repeat(64)},
                "openmc_version": "0.15.3",
                "openmc_c_api_library": {"path": "/runtime/lib/libopenmc.so", "sha256": "l".repeat(64)},
                "worker_helper": {"path": "/run/design_transport.py", "sha256": "w".repeat(64)}
            }
        })).unwrap();
        assert_eq!(
            preflight.runtime_identity.as_ref().unwrap().openmc_version,
            "0.15.3"
        );
        assert_eq!(
            preflight.data_identity.as_ref().unwrap()["electron_treatment"],
            "led"
        );
    }

    #[test]
    fn runtime_and_execution_receipts_rehash_the_files_actually_named() {
        let root =
            std::env::temp_dir().join(format!("faris-heating-receipt-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let run_dir = root.join("run");
        let python = root.join("env/bin/python");
        let library = root.join("env/lib/libopenmc.so");
        let helper = run_dir.join("design_transport.py");
        let h5m = run_dir.join("geometry/model-heating.h5m");
        let xml = run_dir.join("solver/heating-model.xml");
        for (path, body) in [
            (&python, b"selected python".as_slice()),
            (&library, b"loaded C API".as_slice()),
            (&helper, b"staged worker".as_slice()),
            (&h5m, b"retagged geometry".as_slice()),
            (&xml, b"exported model".as_slice()),
        ] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        let receipt = |path: &Path| {
            let canonical = path.canonicalize().unwrap();
            HeatingFileIdentity {
                path: canonical.display().to_string(),
                sha256: sha256_file(&canonical).unwrap(),
            }
        };
        let runtime = HeatingRuntimeIdentity {
            python: receipt(&python),
            openmc_version: "0.16.0".into(),
            openmc_c_api_library: receipt(&library),
            worker_helper: receipt(&helper),
        };
        let execution = HeatingExecutionIdentity {
            run_h5m: receipt(&h5m),
            model_xml: receipt(&xml),
        };
        assert!(verify_runtime_identity(&runtime, &run_dir));
        assert!(verify_execution_identity(&execution, &run_dir));
        #[cfg(unix)]
        {
            // macOS temp paths can resolve through aliases such as /var -> /private/var.
            // Model that by checking the same receipt through a symlinked expected run dir.
            let run_alias = root.join("run-alias");
            std::os::unix::fs::symlink(&run_dir, &run_alias).unwrap();
            assert!(verify_runtime_identity(&runtime, &run_alias));
            assert!(verify_execution_identity(&execution, &run_alias));
        }
        std::fs::write(&xml, b"changed model").unwrap();
        assert!(!verify_execution_identity(&execution, &run_dir));
        let _ = std::fs::remove_dir_all(root);
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
            batches: 30,
            target_relative_error: Some(0.05),
            peak_target_relative_error: Some(0.10),
            target_reached: false,
            mean_relative_error: None,
            peak_relative_error: None,
            variance_of_variance: None,
            c_api_terminal_status: Some(1),
            lost_particles: Some(0),
            stop_reason: "history_limit".into(),
            elapsed_seconds: 30.0,
            seed: 7,
            threads: 2,
            data_identity: Some(serde_json::json!({"photon": {"Fe": {"shell_count": 7}}})),
            runtime_identity: Some(HeatingRuntimeIdentity {
                python: HeatingFileIdentity {
                    path: "/runtime/bin/python".into(),
                    sha256: "p".repeat(64),
                },
                openmc_version: "0.16.0".into(),
                openmc_c_api_library: HeatingFileIdentity {
                    path: "/runtime/lib/libopenmc.so".into(),
                    sha256: "l".repeat(64),
                },
                worker_helper: HeatingFileIdentity {
                    path: "/run/design_transport.py".into(),
                    sha256: "w".repeat(64),
                },
            }),
            execution_identity: Some(HeatingExecutionIdentity {
                run_h5m: HeatingFileIdentity {
                    path: "/run/geometry/model-heating.h5m".into(),
                    sha256: "g".repeat(64),
                },
                model_xml: HeatingFileIdentity {
                    path: "/run/solver/heating-model.xml".into(),
                    sha256: "m".repeat(64),
                },
            }),
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
            history_energy_response: None,
            history_energy_worker_receipt: None,
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

        // Two worker voxel responses exercise the same conversion used by design_heating_impl.
        let worker_solid = |volume_cm3: f64| {
            let score = |mean, se| WorkerScore {
                mean_ev_per_source: mean,
                standard_error_ev_per_source: se,
                within_3se: Some(true),
                roundoff_tolerance_ev_per_source: Some(1e-12),
            };
            let components: BTreeMap<String, WorkerScore> = [
                ("neutron".into(), score(1000.0, 100.0)),
                ("photon".into(), score(0.0, 0.0)),
                ("electron".into(), score(0.0, 0.0)),
                ("positron".into(), score(0.0, 0.0)),
                ("total".into(), score(1000.0, 100.0)),
            ]
            .into_iter()
            .collect();
            let estimator = VolumeEstimator {
                method: "fixture voxel volume".into(),
                rays: 1000,
                sampling_relative_error: None,
                sampling_uncertainty_reported: false,
            };
            let voxel = WorkerVoxel {
                r_cm: [100.0, 105.0],
                z_cm: [-5.0, 0.0],
                volume_cm3,
                components: components.clone(),
                closure_residual: score(0.0, 0.0),
                is_peak: true,
            };
            WorkerSolid {
                solid_id: "tf-winding-01".into(),
                neutron: score(1000.0, 100.0),
                photon: score(0.0, 0.0),
                electron: score(0.0, 0.0),
                positron: score(0.0, 0.0),
                total: score(1000.0, 100.0),
                closure_residual: score(0.0, 0.0),
                peak: Some(WorkerPeak {
                    mean: 1000.0,
                    standard_error: Some(100.0),
                    volume_cm3,
                    r_cm: voxel.r_cm,
                    z_cm: voxel.z_cm,
                    pieces_ranked: 1,
                    volume_estimator: estimator,
                    closure_residual: score(0.0, 0.0),
                    components,
                }),
                voxel_scores: vec![voxel],
            }
        };
        let mut paired_report = report.clone();
        paired_report.solids = [10_000.0, 20_000.0]
            .into_iter()
            .map(|volume| worker_solid_to_heating(worker_solid(volume), 1.0e18).unwrap())
            .collect();
        let peaks: Vec<_> = paired_report
            .solids
            .iter()
            .map(|solid| solid.peak.as_ref().unwrap())
            .collect();
        assert_eq!(peaks[0].volume_m3, 0.01);
        assert_eq!(peaks[1].volume_m3, 0.02);
        assert!((peaks[0].power_density_kw_m3 - 16.021_766_34).abs() < 1e-12);
        assert!((peaks[1].power_density_kw_m3 - 8.010_883_17).abs() < 1e-12);
        assert!((peaks[0].sampling_se_kw_m3.unwrap() - 1.602_176_634).abs() < 1e-12);
        assert!((peaks[1].sampling_se_kw_m3.unwrap() - 0.801_088_317).abs() < 1e-12);
        assert_eq!(
            paired_report.solids[0].voxels[0].volume_m3,
            peaks[0].volume_m3
        );
        assert_eq!(
            paired_report.solids[1].voxels[0].volume_m3,
            peaks[1].volume_m3
        );
        let paired_text = render_design_heating_text(&paired_report);
        assert!(paired_text.contains("1.602177e1 kW/m³"));
        assert!(paired_text.contains("8.010883e0 kW/m³"));
        assert!(paired_text.contains("sampled voxel volume: 1.000000e-2 m³"));
        assert!(paired_text.contains("sampled voxel volume: 2.000000e-2 m³"));

        let rendered = render_design_heating_text(&report);
        assert!(rendered.contains("Design: case"));
        assert!(rendered.contains("Input path: design.json"));
        assert!(rendered.contains(&format!("Input SHA-256: {}", "d".repeat(64))));
        assert!(rendered.contains("OpenMC Python package: 0.16.0"));
        assert!(rendered.contains("C API retagged H5M: /run/geometry/model-heating.h5m"));
        assert!(rendered.contains("C API exported model XML: /run/solver/heating-model.xml"));
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
