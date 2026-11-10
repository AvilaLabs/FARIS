// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! `faris design transport`: the neutron flux in the solids a fluence limit of a checked
//! design applies to (the TF magnets, when the design has no such limit).
//!
//! The design must have passed `faris design check` with every check evaluated; the
//! check report names the conversion whose h5m is run. The OpenMC worker
//! (`integrations/openmc/design_transport.py`) runs as a bounded job and stops at a
//! target relative error or at its history or time limit. It answers per source
//! neutron; the source rate and the volumes are applied here.
//!
//! With an OpenMC 0.16 interpreter for the window step, two jobs run first: a
//! multigroup library on the design's materials and a random-ray adjoint of the
//! response, which gives weight windows. The run then uses them and, on an OpenMC
//! built with the aiming patch, aims at a streaming channel when it finds one.
//! The flux is then compared with the design's fluence limit over the linked
//! operating history (`life`): the mean over the group for a `group_average`
//! limit, the highest piece of any solid for a `peak_mesh` limit. A piece is a
//! ring of the limit's dr by dz about the machine axis, cut by the solid.

use super::life::{FluenceAssessment, LimitStatus, PowerHistory, assess_fluence, power_history};
use super::{
    AIM_COMMON_HELPER, AIM_TARGETS_HELPER, AIM_WINDOWS_HELPER, BATCH_STATISTICS_HELPER,
    CAD_ADDRESS_SPACE, DESIGN_TRANSPORT_WORKER, DesignError, DesignIdentity, ImportReport,
    Launched, MAX_JSON_BYTES, STREAMING_AIM_HELPER, ToolPaths, WINDOW_GENERATOR_HELPER, io_error,
    launch_python_with_lines, resolve, sha256_file, sha256_hex,
};
use crate::history::{ScalarRate, TransportDrivingRates};
use crate::jobs::{Cancellation, ResourceLimits};
use crate::result_cache::{self, CacheKey, CacheProvenance, Lookup, ResultCache};
use crate::transport::{
    D_T_ENERGY_PER_REACTION_EV, D_T_NEUTRON_ENERGY_EV, dt_source_power_summary, reaction_rate_per_s,
};
use crate::transport::{
    NormalizedTally, NormalizedTransportResult, PhysicalUnit, ResponseCovariance, TallyEstimator,
    ToolIdentity as TransportToolIdentity,
};
use faris_model::design::{self, Design, Metric, Role};
use faris_model::history::OperatingHistoryAssumptions;
use faris_model::transport::FAST_NEUTRON_ENERGY_MIN_EV;
use faris_model::validation::{
    Output, Quantity, ValidationMarks, by_score, calculated, not_calculated, notes_block,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fmt::Write as _,
    path::{Path, PathBuf},
    time::Duration,
};

pub const DESIGN_TRANSPORT_VERSION: &str = "faris-design-transport-report/v0.5";
const REQUEST_SCHEMA: &str = "faris-design-transport-request/v3";
const RESULT_SCHEMA: &str = "faris-design-transport/v3";
pub const DESIGN_HISTORY_RATES_VERSION: &str = "faris-cad-history-driving-rates/v0.1";
pub const DESIGN_HISTORY_RATES_CAD_VERSION: &str = "faris-cad-history-driving-rates/v0.2";
pub const DESIGN_HISTORY_RATES_OUTPUTS: &[Output] = &[
    by_score("mean"),
    by_score("standard_error"),
    by_score("integrated_mean"),
    by_score("integrated_standard_error"),
    not_calculated("neutron_source_rate_per_s", "D-T source normalization"),
    not_calculated("source_neutron_rate_per_s", "D-T source normalization"),
    not_calculated("source_reaction_rate_per_s", "D-T source normalization"),
    not_calculated("fusion_power_mw", "authored design source power"),
    not_calculated(
        "reference_fusion_power_mw",
        "source power used to normalize CAD transport responses",
    ),
    not_calculated("source_energy_per_reaction_ev", "D-T source convention"),
    not_calculated("source_neutron_energy_ev", "D-T source convention"),
    not_calculated("neutron_energy_ev", "D-T source convention"),
    not_calculated("energy_per_reaction_ev", "D-T source convention"),
    not_calculated("neutrons_per_reaction", "authored D-T source multiplicity"),
    not_calculated("total_reaction_energy_ev", "D-T source convention"),
    not_calculated("primary_neutron_energy_ev", "D-T source convention"),
    not_calculated("fusion_reaction_rate_per_s", "D-T source bookkeeping"),
    not_calculated(
        "flux_energy_min_ev",
        "authored all-energy response threshold",
    ),
    not_calculated("histories", "count of source neutron histories"),
    not_calculated("batches", "count of independent sampled batches"),
    not_calculated(
        "target_relative_error",
        "authored Monte Carlo stopping target for every required history response",
    ),
    not_calculated("volume_m3", "CAD solid volume used to volume-average flux"),
    not_calculated(
        "volume_standard_error_m3",
        "CAD volume uncertainty is not propagated",
    ),
    not_calculated(
        "response_covariance",
        "same-run batch covariance accompanying the tagged responses",
    ),
    not_calculated(
        "covariance",
        "same-run Monte Carlo covariance accompanying history rates",
    ),
    not_calculated(
        "integrated",
        "same-run sampling covariance in integrated response units",
    ),
    not_calculated(
        "monte_carlo",
        "same-run sampling covariance in history-driving units",
    ),
    not_calculated(
        "volume_variance",
        "CAD volume uncertainty is not propagated",
    ),
    not_calculated("variance_of_variance", "batch convergence diagnostic"),
];

/// Resolved operating context for a checked CAD design. Design/source identity is independent
/// of the operating-history planning assumptions, which may be explicitly overridden.
#[derive(Clone, Debug, PartialEq)]
pub struct CadOperatingContext {
    pub design_path: PathBuf,
    pub design_id: String,
    pub design_sha256: String,
    pub fusion_power_mw: f64,
    pub history_assumptions_path: PathBuf,
    pub history_assumptions_sha256: String,
    pub history_assumptions_overridden: bool,
}

/// Resolve source power from the CAD design and planning assumptions from either an explicit
/// override or the design's hash-bound `operating_scenario` link. No reactor CSG scenario is read.
pub fn resolve_cad_operating_context(
    design_path: &Path,
    assumptions_override: Option<&Path>,
) -> Result<CadOperatingContext, String> {
    let (design_path, _design_bytes, design, source_identity) =
        load_cad_source_identity(design_path)?;
    let fusion_power_mw = source_identity.fusion_power_mw;
    let design_sha256 = source_identity.design_sha256.clone();
    let (history_assumptions_path, history_assumptions_overridden, expected_hash) = if let Some(
        path,
    ) =
        assumptions_override
    {
        (path.to_path_buf(), true, None)
    } else {
        let link = &design.operating_scenario;
        let relative = link
                .path
                .as_deref()
                .ok_or("CAD design has no linked operating-history assumptions; pass an explicit history-assumptions override")?;
        let expected = link
            .sha256
            .as_deref()
            .ok_or("CAD design operating-history assumptions link has no SHA-256")?;
        (
            design_path
                .parent()
                .unwrap_or(Path::new("."))
                .join(relative),
            false,
            Some(expected.to_owned()),
        )
    };
    let history_assumptions_path = history_assumptions_path
        .canonicalize()
        .map_err(|e| format!("operating-history assumptions: {e}"))?;
    if !history_assumptions_path.is_file() {
        return Err("operating-history assumptions must be a regular file".into());
    }
    let assumptions_bytes = read_bounded(&history_assumptions_path).map_err(|e| e.to_string())?;
    let history_assumptions_sha256 = sha256_hex(&assumptions_bytes);
    if expected_hash
        .as_deref()
        .is_some_and(|expected| !expected.eq_ignore_ascii_case(&history_assumptions_sha256))
    {
        return Err("linked operating-history assumptions bytes do not match the CAD design's declared hash".into());
    }
    let assumptions: faris_model::history::OperatingHistoryAssumptions =
        serde_json::from_slice(&assumptions_bytes)
            .map_err(|e| format!("operating-history assumptions JSON: {e}"))?;
    assumptions.validate()?;
    Ok(CadOperatingContext {
        design_path,
        design_id: design.id,
        design_sha256,
        fusion_power_mw,
        history_assumptions_path,
        history_assumptions_sha256,
        history_assumptions_overridden,
    })
}

/// Resolve only the checked design/source identity. Unlike
/// [`resolve_cad_operating_context`], this does not read planning assumptions, so it is suitable
/// for validating a transport or heating response that may later be used with an explicit
/// operating-history assumptions override.
pub fn resolve_cad_source_identity(
    design_path: &Path,
) -> Result<faris_model::design::CadSourceIdentity, String> {
    let (_, _, _, identity) = load_cad_source_identity(design_path)?;
    Ok(identity)
}

fn load_cad_source_identity(
    design_path: &Path,
) -> Result<
    (
        PathBuf,
        Vec<u8>,
        faris_model::design::Design,
        faris_model::design::CadSourceIdentity,
    ),
    String,
> {
    let design_path = design_path.canonicalize().map_err(|e| e.to_string())?;
    if !design_path.is_file() {
        return Err("CAD design input must be a regular file".into());
    }
    let design_bytes = read_bounded(&design_path).map_err(|e| e.to_string())?;
    let design: faris_model::design::Design =
        serde_json::from_slice(&design_bytes).map_err(|e| format!("CAD design JSON: {e}"))?;
    if design.schema_version != faris_model::design::DESIGN_VERSION {
        return Err("CAD design schema version is unsupported".into());
    }
    let fusion_power_mw = design
        .plasma
        .fusion_power_mw
        .filter(|power| power.is_finite() && *power > 0.0)
        .ok_or("CAD design plasma source power must be finite and positive")?;
    let identity = faris_model::design::CadSourceIdentity {
        design_id: design.id.clone(),
        design_sha256: sha256_hex(&design_bytes),
        fusion_power_mw,
    };
    identity.validate()?;
    Ok((design_path, design_bytes, design, identity))
}
/// The fewest batches a run may stop at, as for the reactor run's sampling plan.
const MIN_BATCHES: u64 = 30;
/// Time the run gets beyond its sampling limit to load the geometry, look for a streaming
/// channel and write its result.
const STARTUP_ALLOWANCE: Duration = Duration::from_secs(30 * 60);
const MGXS_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const WINDOWS_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// Names the way weight windows, the channel search and piece volumes are made. Kept results are
/// reused only under the same name: change it whenever one of those steps changes what it writes.
const PREPARATION_RECIPE: &str = "2026-10-14c";
const VOLUMES_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Where a run is: a step that prepares it, or sampling with the error reached so far.
#[derive(Clone, Debug, PartialEq)]
pub enum TransportProgress {
    /// A step has started: `mgxs`, `windows`, `volumes` or `run`.
    Step(String),
    Sampling {
        histories: u64,
        /// `None` until two batches have scored.
        relative_error: Option<f64>,
        /// The error of the highest piece, when the limit asks for a peak and it is known.
        peak_relative_error: Option<f64>,
        elapsed_seconds: f64,
    },
}

/// Called from the thread that reads the worker's output.
pub type TransportProgressFn = std::sync::Arc<dyn Fn(TransportProgress) + Send + Sync>;

/// A worker output line as progress, if it is one.
fn parse_progress(line: &str) -> Option<TransportProgress> {
    #[derive(Deserialize)]
    struct Line {
        progress: Sampling,
    }
    #[derive(Deserialize)]
    struct Sampling {
        histories: u64,
        relative_error: Option<f64>,
        #[serde(default)]
        peak_relative_error: Option<f64>,
        elapsed_seconds: f64,
    }
    if !line.starts_with("{\"progress\"") {
        return None;
    }
    let parsed: Line = serde_json::from_str(line).ok()?;
    Some(TransportProgress::Sampling {
        histories: parsed.progress.histories,
        relative_error: parsed.progress.relative_error,
        peak_relative_error: parsed.progress.peak_relative_error,
        elapsed_seconds: parsed.progress.elapsed_seconds,
    })
}

#[derive(Clone)]
pub struct DesignTransportOptions {
    /// The JSON report `faris design check --report` wrote for this design.
    pub check_report: PathBuf,
    /// A new directory for the run.
    pub out_dir: PathBuf,
    pub target_relative_error: f64,
    pub particles_per_batch: u64,
    pub max_histories: u64,
    pub max_seconds: u64,
    pub seed: u64,
    pub threads: u32,
    /// A Python with OpenMC 0.16 or later, for the weight-window step. `None`: an analog run.
    pub window_python: Option<PathBuf>,
    /// A directory holding an `openmc` package to use in place of the interpreter's own, such
    /// as a build with the aiming patch.
    pub openmc_package_path: Option<PathBuf>,
    /// The replacement group whose fluence limit names the response. `None`: the only group
    /// with a fluence limit, or the TF magnets when the design has no such limit.
    pub group: Option<String>,
    /// Optional explicit mapping from each history component ID to the CAD solid IDs it covers.
    /// When set, the transport run also scores all-energy flux and breeder H3 in the same batches.
    pub history_components: Option<BTreeMap<String, Vec<String>>>,
    /// The exact history scenario whose operating power will consume the CAD-derived rates.
    pub history_scenario: Option<PathBuf>,
    /// Component ID receiving the full-component H3-production tally (normally `blanket`).
    pub h3_component_id: Option<String>,
    /// Where weight windows and the channel search are kept between runs. `None`: nothing is kept.
    pub cache_dir: Option<PathBuf>,
    pub progress: Option<TransportProgressFn>,
    pub cancellation: Cancellation,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct WorkerValue {
    mean: f64,
    standard_error: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct WorkerSolid {
    solid_id: String,
    mean: f64,
    standard_error: Option<f64>,
    #[serde(default)]
    peak: Option<WorkerPeak>,
}

/// The piece of a solid with the highest flux: track length (cm per source neutron) in its volume.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct WorkerPeak {
    mean: f64,
    standard_error: Option<f64>,
    volume_cm3: f64,
    r_cm: [f64; 2],
    z_cm: [f64; 2],
    pieces_ranked: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct WorkerResult {
    schema: String,
    openmc: String,
    h5m_sha256: String,
    run_h5m_sha256: String,
    model_xml_sha256: String,
    model_xml_path: String,
    openmc_library_sha256: String,
    openmc_library_path: String,
    source_kernel_path: String,
    source_kernel_sha256: String,
    seed: u64,
    threads: u32,
    particles_per_batch: u64,
    batches: u64,
    histories: u64,
    elapsed_seconds: f64,
    stop_reason: String,
    target_relative_error: f64,
    target_reached: bool,
    /// Variance of the variance of the batch values; absent from workers before 2026-10-14.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    variance_of_variance: Option<f64>,
    estimator: String,
    unit: String,
    energy_min_ev: f64,
    total: WorkerValue,
    solids: Vec<WorkerSolid>,
    variance_reduction: VarianceReduction,
    why: Option<String>,
    next_step: Option<String>,
    /// `None`: the OpenMC build does not expose its count.
    #[serde(default)]
    lost_particles: Option<u64>,
    #[serde(default)]
    lost_positions_cm: Vec<[f64; 3]>,
    #[serde(default)]
    history_response: Option<WorkerHistoryResponse>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct WorkerHistoryResponse {
    schema: String,
    flux_energy_min_ev: f64,
    h3_score: String,
    h3_total_scope: String,
    estimator: String,
    covariance_method: String,
    batches: u32,
    target_relative_error: f64,
    response_ids: Vec<String>,
    responses: Vec<WorkerHistoryRate>,
    covariance_raw_per_source: Vec<f64>,
    covariance_unit_order: String,
    covariance_scope: String,
    target_reached: bool,
    nuclear_data_identity: HistoryNuclearDataIdentity,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct WorkerHistoryRate {
    response_id: String,
    solid_ids: Vec<String>,
    volume_m3: Option<f64>,
    mean_raw_per_source: f64,
    standard_error_raw_per_source: f64,
}

#[derive(Clone, Debug)]
enum HistorySourceContext {
    LegacyScenario {
        scenario: Box<faris_model::LoadedScenario>,
        path: PathBuf,
        sha256: String,
    },
    CadDesign(faris_model::design::CadSourceIdentity),
}

#[derive(Clone, Debug)]
struct HistorySetup {
    response: serde_json::Value,
    source: HistorySourceContext,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HistoryNuclearDataIdentity {
    pub schema: String,
    pub scope: String,
    pub cross_sections_xml_sha256: String,
    pub tables: Vec<HistoryNuclearDataTable>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HistoryNuclearDataTable {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub library_indices: Vec<usize>,
    pub nuclides: Vec<String>,
}

/// Hash-bound CAD neutron responses consumed by the existing operating-history model.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DesignHistoryRatesArtifact {
    pub schema_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cad_source_identity: Option<faris_model::design::CadSourceIdentity>,
    pub design_path: String,
    pub design_sha256: String,
    pub check_report_path: String,
    pub check_report_sha256: String,
    pub h5m_path: String,
    pub h5m_sha256: String,
    pub cross_sections_path: String,
    pub cross_sections_sha256: String,
    pub nuclear_data_identity: HistoryNuclearDataIdentity,
    pub worker_result_path: String,
    pub worker_result_sha256: String,
    pub transport_request_path: String,
    pub transport_request_sha256: String,
    pub model_xml_path: String,
    pub model_xml_sha256: String,
    pub openmc_library_path: String,
    pub openmc_library_sha256: String,
    pub source_definition_sha256: String,
    pub source_kernel_path: String,
    pub source_kernel_sha256: String,
    pub source_energy_per_reaction_ev: f64,
    pub source_neutron_energy_ev: f64,
    pub fusion_power_mw: f64,
    pub source_neutron_rate_per_s: f64,
    pub flux_energy_min_ev: f64,
    pub h3_score: String,
    pub component_map: BTreeMap<String, Vec<String>>,
    pub h3_component_id: String,
    pub h3_solid_ids: Vec<String>,
    pub component_map_sha256: String,
    pub response_definitions_sha256: String,
    pub histories: u64,
    pub batches: u32,
    pub target_relative_error: f64,
    pub covariance_method: String,
    pub covariance_scope: String,
    pub normalized_transport: NormalizedTransportResult,
    pub validation: ValidationMarks,
    pub total_h3_production_per_source_neutron: ScalarRate,
    /// Shared history engine input, derived from the exact same-batch CAD tallies.
    pub driving_rates: TransportDrivingRates,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct HistoryRatesArtifactRef {
    pub path: String,
    pub sha256: String,
}

/// How the run was sped up, with the reason for the mode and what the user can do.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct VarianceReduction {
    /// `analog`, `windows-only` or `aimed`.
    pub mode: String,
    pub reason: String,
    pub next_step: String,
    /// The streaming targets an aimed run found.
    #[serde(default)]
    pub targets: Vec<String>,
    pub windows_sha256: Option<String>,
    /// Seconds the run spent looking for a streaming channel before sampling.
    #[serde(default)]
    pub aim_seconds: f64,
    /// The channel search of an earlier run on the same model was used.
    #[serde(default)]
    pub aim_reused: bool,
}

/// One fluence limit of the design against the flux its averaging rule names.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct FluenceLimit {
    pub group_id: String,
    /// `group_average`: the mean over the group's solids. `peak_piece`: the highest piece of
    /// any solid of the group.
    pub basis: String,
    /// The solid that holds the highest piece; `None` for a group average.
    pub solid_id: Option<String>,
    pub assessment: FluenceAssessment,
}

/// The piece of a solid with the highest flux: a ring about the machine axis, cut by the solid.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PeakFlux {
    pub r_m: [f64; 2],
    pub z_m: [f64; 2],
    pub volume_m3: f64,
    pub flux_n_m2_s: f64,
    pub relative_error: Option<f64>,
    /// How many pieces of the solid were compared.
    pub pieces_ranked: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SolidFlux {
    pub solid_id: String,
    pub volume_m3: f64,
    pub flux_n_m2_s: f64,
    /// `None`: fewer than two batches, or no score.
    pub relative_error: Option<f64>,
    /// `None`: the limit asks for no peak, or the solid has no piece to rank.
    pub peak: Option<PeakFlux>,
}

/// Something the run did not compute, with the reason and what to do.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct NotEvaluated {
    pub item: String,
    pub why: String,
    pub next_step: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DesignTransportReport {
    pub schema_version: String,
    pub design: DesignIdentity,
    pub check_report_sha256: String,
    pub h5m_sha256: String,
    pub cross_sections_sha256: String,
    pub openmc: String,
    pub run_directory: String,
    pub energy_min_ev: f64,
    /// The replacement group the response was taken from; `None`: the TF magnets, by role.
    pub response_group_id: Option<String>,
    /// Source neutrons per second, from the fusion power at 17.6 MeV per reaction.
    pub source_rate_per_s: f64,
    /// Energy per reaction the source rate was derived with (the convention, stored).
    #[serde(default = "default_energy_per_reaction_ev")]
    pub energy_per_reaction_ev: f64,
    pub fusion_power_mw: f64,
    pub histories: u64,
    pub elapsed_seconds: f64,
    pub seed: u64,
    pub threads: u32,
    pub target_relative_error: f64,
    pub target_reached: bool,
    pub stop_reason: String,
    /// Why the target was not reached and what to do; `None` when it was.
    pub why: Option<String>,
    pub next_step: Option<String>,
    /// Volume-averaged flux above the threshold over all TF magnet solids.
    pub mean_flux_n_m2_s: f64,
    pub mean_relative_error: Option<f64>,
    /// Variance of the variance of the batch values of the mean (MCNP's check; a run is accepted only at or
    /// below 0.1). `None`: an earlier report, or too few batches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variance_of_variance: Option<f64>,
    pub total_volume_m3: f64,
    pub solids: Vec<SolidFlux>,
    pub variance_reduction: VarianceReduction,
    /// Seconds spent before sampling on weight windows and the search for a streaming channel.
    pub setup_seconds: f64,
    /// The weight windows of an earlier run on the same model were used.
    pub windows_reused: bool,
    /// When, by which version and under which key the reused windows were made (AUTO-039);
    /// absent when nothing was taken from the cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reused_from: Option<CacheProvenance>,
    pub fluence_limits: Vec<FluenceLimit>,
    /// Neutrons the geometry could not place; `None`: not counted by this OpenMC build.
    #[serde(default)]
    pub lost_particles: Option<u64>,
    /// OpenMC-recorded birth positions of the first lost neutrons (m); these are not the locations
    /// where the neutrons left the geometry.
    #[serde(default)]
    pub lost_positions_m: Vec<[f64; 3]>,
    pub notices: Vec<String>,
    pub not_evaluated: Vec<NotEvaluated>,
    /// Separate hash-bound all-energy/H3 rates for the existing operating-history engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_rates_artifact: Option<HistoryRatesArtifactRef>,
    /// The validation state of each calculated number above when the report was made (VAL-054).
    /// Empty in a report written before v0.4.
    #[serde(default, skip_serializing_if = "ValidationMarks::is_empty")]
    pub validation: ValidationMarks,
}

/// Every numeric field of the report, by JSON name: a calculated number names its quantity, any
/// other says why it is not one. A test fails on a number in neither list.
pub const DESIGN_TRANSPORT_OUTPUTS: &[Output] = &[
    calculated("mean_flux_n_m2_s", Quantity::Flux),
    calculated("mean_relative_error", Quantity::Flux),
    calculated("variance_of_variance", Quantity::Flux),
    calculated("flux_n_m2_s", Quantity::Flux),
    calculated("relative_error", Quantity::Flux),
    calculated("flux_relative_error", Quantity::Flux),
    calculated("history_fluence_n_m2", Quantity::Fluence),
    calculated("history_fluence_standard_error_n_m2", Quantity::Fluence),
    calculated("history_fluence_low_n_m2", Quantity::Fluence),
    calculated("history_fluence_high_n_m2", Quantity::Fluence),
    calculated("full_power_years_to_limit", Quantity::Fluence),
    calculated("calendar_years_to_limit", Quantity::Fluence),
    calculated("full_power_years_to_limit_earliest", Quantity::Fluence),
    calculated("full_power_years_to_limit_latest", Quantity::Fluence),
    calculated("calendar_years_to_limit_earliest", Quantity::Fluence),
    calculated("calendar_years_to_limit_latest", Quantity::Fluence),
    calculated("years_relative_error", Quantity::Fluence),
    not_calculated("energy_min_ev", "authored threshold of the response"),
    not_calculated(
        "source_rate_per_s",
        "conversion of the authored fusion power at the stored energy per reaction",
    ),
    not_calculated("energy_per_reaction_ev", "convention constant, stored"),
    not_calculated("fusion_power_mw", "authored design input"),
    not_calculated("histories", "count of source neutrons run"),
    not_calculated("elapsed_seconds", "wall-clock time of the run"),
    not_calculated("seed", "run setting"),
    not_calculated("threads", "run setting"),
    not_calculated("target_relative_error", "authored stopping target"),
    not_calculated(
        "total_volume_m3",
        "CAD volume of the design file, an input to the flux normalization",
    ),
    not_calculated(
        "volume_m3",
        "CAD volume of the design file, an input to the flux normalization",
    ),
    not_calculated(
        "r_m",
        "location of the highest piece, a position and not a transport quantity",
    ),
    not_calculated(
        "z_m",
        "location of the highest piece, a position and not a transport quantity",
    ),
    not_calculated("pieces_ranked", "count of pieces compared"),
    not_calculated("aim_seconds", "wall-clock time of the channel search"),
    not_calculated("setup_seconds", "wall-clock time of the preparation steps"),
    not_calculated("limit_n_m2", "authored fluence limit of the design"),
    not_calculated(
        "history_calendar_years",
        "arithmetic on the authored operating history only",
    ),
    not_calculated(
        "history_full_power_years",
        "arithmetic on the authored operating history only",
    ),
    not_calculated(
        "capacity_factor",
        "arithmetic on the authored operating history only",
    ),
    not_calculated(
        "lost_particles",
        "count of neutrons the geometry could not place, a run-integrity check",
    ),
    not_calculated(
        "lost_positions_m",
        "OpenMC-recorded birth positions of the first lost neutrons, not their loss locations",
    ),
];

fn default_energy_per_reaction_ev() -> f64 {
    D_T_ENERGY_PER_REACTION_EV
}

fn lost_position_provenance_notice(
    lost: Option<u64>,
    positions: &[[f64; 3]],
) -> Option<&'static str> {
    (lost.is_some_and(|count| count > 0) && !positions.is_empty()).then_some(
        "The listed lost-neutron positions are their recorded birth points; OpenMC does not \
         report where they left the geometry in this run format.",
    )
}

/// Flux in n/m²/s from a track length per source neutron (cm), a source rate and a volume.
pub fn flux_n_m2_s(cm_per_source: f64, source_rate_per_s: f64, volume_m3: f64) -> f64 {
    // cm / cm³ = 1/cm² = 1e4 /m²; the volume is 1e6 cm³ per m³.
    cm_per_source * source_rate_per_s / (volume_m3 * 1.0e6) * 1.0e4
}

fn source_rate_per_s(fusion_power_mw: f64) -> f64 {
    reaction_rate_per_s(fusion_power_mw, D_T_ENERGY_PER_REACTION_EV)
}

fn tool(message: impl Into<String>) -> DesignError {
    DesignError::Tool(message.into())
}

pub(super) fn read_bounded(path: &Path) -> Result<Vec<u8>, DesignError> {
    let metadata = std::fs::metadata(path).map_err(|e| io_error(path, e))?;
    if !metadata.is_file() || metadata.len() > MAX_JSON_BYTES {
        return Err(io_error(path, "not a regular file under 64 MiB"));
    }
    std::fs::read(path).map_err(|e| io_error(path, e))
}

/// One entry per design material in use: its nuclide vector from the check's audit and its density.
pub(super) fn material_entries(
    design: &Design,
    report: &ImportReport,
) -> Result<Vec<serde_json::Value>, DesignError> {
    let audited: BTreeMap<&str, &BTreeMap<String, f64>> = report
        .material_audit
        .iter()
        .flat_map(|a| &a.materials)
        .filter_map(|m| Some((m.id.as_str(), m.nuclides.as_ref()?)))
        .collect();
    let mut entries = Vec::new();
    for material in &design.materials {
        if !design
            .solids
            .iter()
            .any(|s| s.material_id.as_deref() == Some(material.id.as_str()))
        {
            continue;
        }
        let material_id = material.id.as_str();
        let nuclides = audited.get(material_id).ok_or_else(|| {
            tool(format!(
                "the check report holds no audited composition for material {material_id}; run \
                 `faris design check` with --openmc-python and --cross-sections and pass its report"
            ))
        })?;
        let density = material
            .density_kg_m3
            .or_else(|| {
                material
                    .catalog_id
                    .as_deref()
                    .and_then(design::catalog_material)
                    .map(|c| c.density_kg_m3)
            })
            .ok_or_else(|| tool(format!("material {material_id} has no density")))?;
        entries.push(serde_json::json!({
            "material_id": material_id,
            "nuclide_atom_fractions": nuclides,
            "density_kg_m3": density,
            "temperature_k": material.temperature_k,
        }));
    }
    Ok(entries)
}

/// Trusted numeric input for the separate maintenance-dose CAD preparation worker. The H5M and
/// materials are taken only from the same complete checked Design/ImportReport pair used by the
/// ordinary design-transport path.
pub(crate) fn maintenance_dose_cad_request(
    design_path: &Path,
    check_report_path: &Path,
    cross_sections: &Path,
    openmc_helper: &Path,
    solid_history_components: BTreeMap<String, String>,
) -> Result<serde_json::Value, DesignError> {
    let design_bytes = read_bounded(design_path)?;
    let design_sha256 = sha256_hex(&design_bytes);
    let design = design::parse_design(&design_bytes)
        .design
        .ok_or_else(|| tool("the design file does not parse"))?;
    let report_bytes = read_bounded(check_report_path)?;
    let report: ImportReport = serde_json::from_slice(&report_bytes)
        .map_err(|e| tool(format!("invalid Design ImportReport: {e}")))?;
    accept_check_report(&report, &design_sha256)?;
    let conversion = report
        .geometry
        .conversion
        .as_ref()
        .ok_or_else(|| tool("the ImportReport records no checked CAD conversion"))?;
    let h5m = converted_model(&conversion.directory)?;
    if !h5m.is_file() || !sha256_file(&h5m)?.eq_ignore_ascii_case(&conversion.h5m_sha256) {
        return Err(tool(
            "checked CAD H5M is missing or differs from the ImportReport",
        ));
    }
    let expected: std::collections::BTreeSet<String> = design
        .solids
        .iter()
        .filter(|s| s.material_id.as_deref().is_some_and(|m| m != "void"))
        .map(|s| s.id.clone())
        .collect();
    let supplied: std::collections::BTreeSet<String> =
        solid_history_components.keys().cloned().collect();
    if expected != supplied {
        return Err(tool(
            "solid-to-history-component mapping must cover every nonvoid checked solid exactly",
        ));
    }
    let cross_sections = cross_sections
        .canonicalize()
        .map_err(|e| io_error(cross_sections, e))?;
    let openmc_helper = openmc_helper
        .canonicalize()
        .map_err(|e| io_error(openmc_helper, e))?;
    Ok(serde_json::json!({
        "schema": "faris-maintenance-dose-cad-prepare/v0.1",
        "h5m": h5m,
        "h5m_sha256": conversion.h5m_sha256,
        "cross_sections": cross_sections,
        "solids": solid_entries(&design, &report)?,
        "materials": material_entries(&design, &report)?,
        "plasma": design.plasma,
        "solid_history_components": solid_history_components,
        "openmc_helper": openmc_helper,
    }))
}

/// Every solid with its DAGMC volume from the check's binding; `material_id` null for empty space.
pub(super) fn solid_entries(
    design: &Design,
    report: &ImportReport,
) -> Result<Vec<serde_json::Value>, DesignError> {
    let volumes: BTreeMap<&str, u32> = report
        .geometry
        .binding
        .iter()
        .filter_map(|row| Some((row.solid_id.as_deref()?, row.volume)))
        .collect();
    design
        .solids
        .iter()
        .map(|solid| {
            let volume = volumes.get(solid.id.as_str()).ok_or_else(|| {
                tool(format!(
                    "the check report binds no DAGMC volume to solid {}",
                    solid.id
                ))
            })?;
            let material = solid
                .material_id
                .as_deref()
                .ok_or_else(|| tool(format!("solid {} has no material", solid.id)))?;
            Ok(serde_json::json!({
                "solid_id": solid.id,
                "volume_id": volume,
                "material_id": (material != "void").then_some(material),
                "classification": solid.classification.map(|kind| match kind {
                    faris_model::design::SolidClassification::WindingPack => "winding_pack",
                }),
            }))
        })
        .collect()
}

fn validate_history_component_partition(
    active_solid_ids: &std::collections::BTreeSet<String>,
    components: &BTreeMap<String, Vec<String>>,
    breeder_component_id: &str,
) -> Result<(), String> {
    if components.is_empty() || components.len() > 254 {
        return Err("history component mapping must contain 1 to 254 components".into());
    }
    if breeder_component_id != "blanket" || !components.contains_key(breeder_component_id) {
        return Err(
            "the existing operating-history model requires a mapped breeder component ID `blanket`"
                .into(),
        );
    }
    let mut covered = std::collections::BTreeSet::new();
    for (component_id, solid_ids) in components {
        if !faris_model::design::valid_id(component_id) || solid_ids.is_empty() {
            return Err(
                "history component IDs must be valid and each map at least one solid".into(),
            );
        }
        let mut local = std::collections::BTreeSet::new();
        for solid_id in solid_ids {
            if !active_solid_ids.contains(solid_id)
                || !local.insert(solid_id)
                || !covered.insert(solid_id.clone())
            {
                return Err(format!(
                    "history solid {solid_id} is inactive, unknown, or mapped more than once"
                ));
            }
        }
    }
    if &covered != active_solid_ids {
        return Err(
            "history component map must cover every active non-void CAD solid exactly once".into(),
        );
    }
    Ok(())
}

fn same_source_power_binding(design_power_mw: f64, history_power_mw: f64) -> bool {
    design_power_mw.is_finite()
        && history_power_mw.is_finite()
        && design_power_mw.to_bits() == history_power_mw.to_bits()
}

fn validate_history_scenario_binding(
    artifact_scenario_id: &str,
    artifact_scenario_sha256: &str,
    artifact_power_mw: f64,
    rate_scenario_sha256: &str,
    rate_power_mw: f64,
    scenario: &faris_model::LoadedScenario,
) -> Result<(), String> {
    if artifact_scenario_id != scenario.scenario.id
        || artifact_scenario_sha256 != scenario.source_sha256
        || rate_scenario_sha256 != scenario.source_sha256
        || !same_source_power_binding(
            artifact_power_mw,
            scenario.scenario.operating_plan.fusion_power_mw,
        )
        || !same_source_power_binding(rate_power_mw, artifact_power_mw)
    {
        return Err("history rates scenario identity or fusion power differs from the loaded operating scenario".into());
    }
    Ok(())
}

fn history_response_request(
    design: &Design,
    report: &ImportReport,
    components: &BTreeMap<String, Vec<String>>,
    breeder_component_id: &str,
) -> Result<serde_json::Value, DesignError> {
    let active: BTreeMap<_, _> = design
        .solids
        .iter()
        .filter(|solid| solid.material_id.as_deref().is_some_and(|id| id != "void"))
        .map(|solid| (solid.id.as_str(), solid))
        .collect();
    let active_ids = active.keys().map(|id| (*id).to_string()).collect();
    validate_history_component_partition(&active_ids, components, breeder_component_id)
        .map_err(tool)?;
    let mut covered = std::collections::BTreeSet::new();
    let mut component_rows = Vec::with_capacity(components.len());
    for (component_id, solid_ids) in components {
        if !faris_model::design::valid_id(component_id) || solid_ids.is_empty() {
            return Err(tool(
                "history component IDs must be valid and each map at least one solid",
            ));
        }
        let mut unique_ids = std::collections::BTreeSet::new();
        let mut volume_m3 = 0.0;
        for solid_id in solid_ids {
            if !unique_ids.insert(solid_id.as_str()) || !covered.insert(solid_id.as_str()) {
                return Err(tool(format!(
                    "history solid {solid_id} is mapped more than once"
                )));
            }
            let solid = active.get(solid_id.as_str()).ok_or_else(|| {
                tool(format!(
                    "history map names inactive or unknown CAD solid {solid_id}"
                ))
            })?;
            volume_m3 += solid.fingerprint.cad_volume_m3;
        }
        if !volume_m3.is_finite() || volume_m3 <= 0.0 {
            return Err(tool(format!(
                "history component {component_id} has no positive CAD volume"
            )));
        }
        component_rows.push(serde_json::json!({
            "component_id": component_id,
            "solid_ids": solid_ids,
            "volume_m3": volume_m3,
        }));
    }
    if covered.len() != active.len() || active.keys().any(|id| !covered.contains(id)) {
        return Err(tool(
            "history component map must cover every active non-void CAD solid exactly once",
        ));
    }
    let breeder_solids = components
        .get(breeder_component_id)
        .ok_or_else(|| tool("history component map has no `blanket` breeder"))?;
    let material_by_id: BTreeMap<_, _> = material_entries(design, report)?
        .into_iter()
        .filter_map(|entry| Some((entry.get("material_id")?.as_str()?.to_string(), entry)))
        .collect();
    for solid_id in breeder_solids {
        let material_id = active[solid_id.as_str()]
            .material_id
            .as_deref()
            .ok_or_else(|| tool("the breeder solid has no audited material"))?;
        let material = material_by_id
            .get(material_id)
            .ok_or_else(|| tool(format!("the breeder material {material_id} is not audited")))?;
        let fractions = material
            .get("nuclide_atom_fractions")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| {
                tool(format!(
                    "the breeder material {material_id} has no audited nuclides"
                ))
            })?;
        if !fractions
            .get("Li6")
            .and_then(serde_json::Value::as_f64)
            .is_some_and(|v| v > 0.0)
        {
            return Err(tool(format!(
                "the breeder material {material_id} has no audited Li6; its H3-production response cannot be formed"
            )));
        }
    }
    Ok(serde_json::json!({
        "components": component_rows,
        "breeder_component_id": breeder_component_id,
        "h3_solid_ids": breeder_solids,
        "h3_total_scope": "whole_model",
        "flux_energy_min_ev": 0.0,
    }))
}

/// The power history the design links to; an error sentence when it cannot be used.
fn linked_history(
    design_path: &Path,
    design: &Design,
    fusion_power_mw: f64,
) -> Result<PowerHistory, String> {
    let relative = design
        .operating_scenario
        .path
        .as_deref()
        .ok_or("The design file links no operating history.")?;
    let path = resolve(design_path, relative);
    let bytes =
        read_bounded(&path).map_err(|e| format!("The operating history cannot be read: {e}."))?;
    if let Some(expected) = design.operating_scenario.sha256.as_deref()
        && !sha256_hex(&bytes).eq_ignore_ascii_case(expected)
    {
        return Err("The operating history file is not the one the design file records.".into());
    }
    let history: OperatingHistoryAssumptions = serde_json::from_slice(&bytes)
        .map_err(|e| format!("The operating history does not parse: {e}."))?;
    power_history(&history, fusion_power_mw)
}

/// The run's source is the tokamak formula for a D-T plasma. A design that asks for another
/// source is refused, so that it is never run with the wrong one.
pub(super) fn refuse_unsupported_source(design: &Design) -> Result<(), DesignError> {
    if design.plasma.emissivity_table.is_some() {
        return Err(tool(
            "the design gives an emissivity table, which this command cannot use yet: it would \
             run the tokamak profile instead. Remove emissivity_table to run with the profile",
        ));
    }
    let share = |species: &str| {
        design
            .plasma
            .fuel
            .as_ref()
            .and_then(|fuel| fuel.get(species).copied())
            .unwrap_or(0.0)
    };
    if !(share("D") > 0.0 && share("T") > 0.0) {
        return Err(tool(
            "the design's fuel is not a deuterium-tritium mix; this command's source rate and \
             neutron energy are for D-T only",
        ));
    }
    Ok(())
}

/// What the run scores: the flux above a threshold in the solids of one replacement group.
#[derive(Clone, Debug, PartialEq)]
struct ChosenResponse {
    /// `None`: the TF magnets by role, for a design with no fluence limit.
    group_id: Option<String>,
    energy_min_ev: f64,
    limit_n_m2: Option<f64>,
    /// `[dr, dz]` in metres of a `peak_mesh` limit; `None` in a place: the solid's full width.
    peak_voxel_m: Option<[Option<f64>; 2]>,
    /// The limit asks for pieces of a set toroidal width, which this command does not cut.
    peak_ignores_dphi: bool,
}

fn peak_voxel(limit: &design::Limit) -> (Option<[Option<f64>; 2]>, bool) {
    match &limit.averaging {
        design::Averaging::PeakMesh { voxel_m } if voxel_m.len() == 3 => {
            (Some([voxel_m[0], voxel_m[1]]), voxel_m[2].is_some())
        }
        _ => (None, false),
    }
}

/// The fluence limit a metric and threshold describe: the energy above which neutrons count.
fn fluence_threshold(limit: &design::Limit) -> Option<f64> {
    match limit.metric {
        Metric::FastNeutronFluence => Some(
            limit
                .energy_threshold_ev
                .unwrap_or(FAST_NEUTRON_ENERGY_MIN_EV),
        ),
        Metric::NeutronFluence => Some(0.0),
        _ => None,
    }
}

/// Pick the response from the design's fluence limits. With several groups the caller names one.
fn choose_response(design: &Design, group: Option<&str>) -> Result<ChosenResponse, DesignError> {
    let candidates: Vec<ChosenResponse> = design
        .replacement_groups
        .iter()
        .filter_map(|g| {
            let limit = g.limits.iter().find(|l| fluence_threshold(l).is_some())?;
            let (peak_voxel_m, peak_ignores_dphi) = peak_voxel(limit);
            Some(ChosenResponse {
                group_id: Some(g.id.clone()),
                energy_min_ev: fluence_threshold(limit)?,
                limit_n_m2: Some(limit.limit),
                peak_voxel_m,
                peak_ignores_dphi,
            })
        })
        .collect();
    let ids = || {
        candidates
            .iter()
            .filter_map(|c| c.group_id.as_deref())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match (group, candidates.len()) {
        (Some(wanted), _) => candidates
            .iter()
            .find(|c| c.group_id.as_deref() == Some(wanted))
            .cloned()
            .ok_or_else(|| {
                tool(format!(
                    "the replacement group {wanted} has no fluence limit; the groups with one are: {}",
                    ids()
                ))
            }),
        (None, 0) => Ok(ChosenResponse {
            group_id: None,
            energy_min_ev: FAST_NEUTRON_ENERGY_MIN_EV,
            limit_n_m2: None,
            peak_voxel_m: None,
            peak_ignores_dphi: false,
        }),
        (None, 1) => Ok(candidates[0].clone()),
        (None, _) => Err(tool(format!(
            "several replacement groups have a fluence limit; name one with --group: {}",
            ids()
        ))),
    }
}

/// One entry of the cache per model, materials, response, nuclear data and preparation recipe.
/// Built by the one key function (AUTO-031). The window interpreter enters as its identity
/// (`result_cache::interpreter_identity`), not its path.
fn transport_cache_key(
    request: &serde_json::Value,
    h5m_sha256: &str,
    cross_sections_sha256: &str,
    window_python: Option<&Path>,
) -> CacheKey {
    result_cache::cache_key(
        "design-transport",
        [
            ("recipe", serde_json::json!(PREPARATION_RECIPE)),
            ("h5m", serde_json::json!(h5m_sha256)),
            ("cross_sections", serde_json::json!(cross_sections_sha256)),
            ("solids", request["solids"].clone()),
            ("materials", request["materials"].clone()),
            ("chamber_solid_id", request["chamber_solid_id"].clone()),
            ("response", request["response"].clone()),
            (
                "window_python",
                serde_json::json!(window_python.map(result_cache::interpreter_identity)),
            ),
        ],
    )
}

fn copy_file(from: &Path, to: &Path) -> Result<(), DesignError> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
    }
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| io_error(from, e))
}

#[derive(Deserialize)]
struct WindowsRecord {
    filled_sha256: String,
}

/// What the design import reads back from the helpers and its kept entries, for the fuzz targets
/// (QA-033): each of these files is parsed from disk by code that is not otherwise reachable.
#[doc(hidden)]
pub fn fuzz_helper_outputs(bytes: &[u8]) {
    let _ = serde_json::from_slice::<WorkerResult>(bytes);
    let _ = serde_json::from_slice::<DesignHistoryRatesArtifact>(bytes);
    super::heating::fuzz_worker_outputs(bytes);
    let _ = serde_json::from_slice::<WindowsRecord>(bytes);
    let _ = serde_json::from_slice::<PiecesRecord>(bytes);
    if let Ok(text) = std::str::from_utf8(bytes) {
        let _ = parse_progress(text);
        for line in text.lines().take(64) {
            let _ = parse_progress(line);
        }
    }
    super::external::fuzz_cache_key(bytes);
}

/// Put a kept entry's weight windows and channel search into the run directory. True when the
/// windows were found and match their record; a damaged entry is ignored, not trusted.
fn restore_windows(entry: &Path, dir: &Path) -> Result<bool, DesignError> {
    let record_path = entry.join("windows").join("record.json");
    let filled = entry.join("windows").join("filled.h5");
    if !record_path.is_file() || !filled.is_file() {
        return Ok(false);
    }
    let Ok(record) = serde_json::from_slice::<WindowsRecord>(&read_bounded(&record_path)?) else {
        return Ok(false);
    };
    if !sha256_file(&filled)?.eq_ignore_ascii_case(&record.filled_sha256) {
        return Ok(false);
    }
    copy_file(&record_path, &dir.join("windows").join("record.json"))?;
    copy_file(&filled, &dir.join("windows").join("filled.h5"))?;
    let plan = entry.join("aim").join("plan.json");
    if plan.is_file() {
        copy_file(&plan, &dir.join("aim-kept").join("plan.json"))?;
        let capped = entry.join("aim").join("windows-capped.h5");
        if capped.is_file() {
            copy_file(&capped, &dir.join("aim-kept").join("windows-capped.h5"))?;
        }
    }
    Ok(true)
}

const PIECE_FILES: [&str; 2] = ["record.json", "volumes.npz"];

#[derive(Deserialize)]
struct PiecesRecord {
    volumes_sha256: String,
}

/// Put a kept entry's piece volumes into the run directory; a damaged entry is ignored.
fn restore_pieces(entry: &Path, dir: &Path) -> Result<bool, DesignError> {
    let record_path = entry.join("peak").join("record.json");
    let volumes = entry.join("peak").join("volumes.npz");
    if !record_path.is_file() || !volumes.is_file() {
        return Ok(false);
    }
    let Ok(record) = serde_json::from_slice::<PiecesRecord>(&read_bounded(&record_path)?) else {
        return Ok(false);
    };
    if !sha256_file(&volumes)?.eq_ignore_ascii_case(&record.volumes_sha256) {
        return Ok(false);
    }
    for name in PIECE_FILES {
        copy_file(&entry.join("peak").join(name), &dir.join("peak").join(name))?;
    }
    Ok(true)
}

/// Keep this run's weight windows and channel search for the next run on the same model.
fn keep_for_later(dir: &Path, entry: &Path, windows_reused: bool) -> Result<(), DesignError> {
    if !windows_reused {
        for name in ["record.json", "filled.h5"] {
            copy_file(
                &dir.join("windows").join(name),
                &entry.join("windows").join(name),
            )?;
        }
    }
    // The worker writes these only after a search of its own.
    let plan = dir.join("aim").join("plan.json");
    if plan.is_file() {
        let capped = dir.join("aim").join("windows-capped.h5");
        if capped.is_file() {
            copy_file(&capped, &entry.join("aim").join("windows-capped.h5"))?;
        } else {
            let _ = std::fs::remove_file(entry.join("aim").join("windows-capped.h5"));
        }
        copy_file(&plan, &entry.join("aim").join("plan.json"))?;
    }
    Ok(())
}

/// The check report must describe this design file and a complete, passing import.
pub(super) fn accept_check_report(
    report: &ImportReport,
    design_sha256: &str,
) -> Result<(), DesignError> {
    if !report.design.sha256.eq_ignore_ascii_case(design_sha256) {
        return Err(tool(
            "the check report was written for a different version of the design file; run \
             `faris design check --report` again and pass the new report",
        ));
    }
    if !report.import_complete {
        return Err(tool(format!(
            "the design has not passed all nine import checks ({}); transport runs only on a \
             design that imports. Resolve the findings of `faris design check` first",
            report.summary
        )));
    }
    Ok(())
}

/// The geometry file a check report names. The directory comes from the report's text, so it is
/// never a program or an argument to one (the OpenMC step receives it as data inside its request
/// file); it is still held to a path FARIS itself wrote: absolute, with no `..`, holding a plain
/// `model.h5m` that is not a link to somewhere else. SEC-001.
pub(super) fn converted_model(directory: &str) -> Result<PathBuf, DesignError> {
    let refuse = |why: &str| {
        tool(format!(
            "the check report names the conversion directory {directory:?}, which {why}; run \
             `faris design check --work-dir DIR --report FILE` again and use the new report"
        ))
    };
    let directory_path = Path::new(directory);
    if directory.is_empty() || directory.contains('\0') {
        return Err(refuse("is empty or holds a NUL character"));
    }
    if !directory_path.is_absolute() {
        return Err(refuse("is not an absolute path"));
    }
    if directory_path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(refuse("climbs out with `..`"));
    }
    let h5m = directory_path.join("model.h5m");
    if let Ok(metadata) = std::fs::symlink_metadata(&h5m)
        && metadata.file_type().is_symlink()
    {
        return Err(refuse("holds a model.h5m that is a link"));
    }
    Ok(h5m)
}

/// Run the flux response of a checked design to a target error.
pub fn design_transport(
    design_path: &Path,
    tools: &ToolPaths,
    options: &DesignTransportOptions,
) -> Result<DesignTransportReport, DesignError> {
    if !(options.target_relative_error > 0.0 && options.target_relative_error < 1.0) {
        return Err(tool("the target relative error must lie between 0 and 1"));
    }
    if options.particles_per_batch == 0
        || options.max_histories < MIN_BATCHES * options.particles_per_batch
    {
        return Err(tool(format!(
            "the history limit must allow at least {MIN_BATCHES} batches of {} histories",
            options.particles_per_batch
        )));
    }
    let python = tools.openmc_python.as_deref().ok_or_else(|| {
        tool("no OpenMC interpreter is configured; pass --openmc-python (or set FARIS_OPENMC_PYTHON)")
    })?;
    let cross_sections = tools.cross_sections.as_deref().ok_or_else(|| {
        tool("no nuclear data is configured; pass --cross-sections (or set FARIS_CROSS_SECTIONS)")
    })?;
    let plasma_source = tools.plasma_source_path.as_deref().ok_or_else(|| {
        tool(
            "no path to openmc_plasma_source is configured; pass --plasma-source-path (or set \
             FARIS_PLASMA_SOURCE_PATH)",
        )
    })?;

    let design_bytes = read_bounded(design_path)?;
    let design_sha256 = sha256_hex(&design_bytes);
    let design = design::parse_design(&design_bytes)
        .design
        .ok_or_else(|| tool("the design file does not parse; run `faris design check`"))?;
    let report_bytes = read_bounded(&options.check_report)?;
    let report: ImportReport = serde_json::from_slice(&report_bytes).map_err(|e| {
        tool(format!(
            "{} is not a design check report: {e}",
            options.check_report.display()
        ))
    })?;
    accept_check_report(&report, &design_sha256)?;
    let conversion = report
        .geometry
        .conversion
        .as_ref()
        .ok_or_else(|| tool("the check report records no conversion"))?;
    let h5m = converted_model(&conversion.directory)?;
    if !h5m.is_file() || !sha256_file(&h5m)?.eq_ignore_ascii_case(&conversion.h5m_sha256) {
        return Err(tool(format!(
            "{} is missing or is not the geometry the check report describes; run `faris design \
             check --work-dir DIR --report FILE` again and keep DIR",
            h5m.display()
        )));
    }

    let chosen = choose_response(&design, options.group.as_deref())?;
    let response: Vec<(&str, f64)> = design
        .solids
        .iter()
        .filter(|s| s.material_id.as_deref() != Some("void"))
        .filter(|s| match &chosen.group_id {
            Some(group) => s.replacement_group_id.as_deref() == Some(group.as_str()),
            None => s.role == Some(Role::TfMagnet),
        })
        .map(|s| (s.id.as_str(), s.fingerprint.cad_volume_m3))
        .collect();
    if response.is_empty() {
        return Err(tool(match &chosen.group_id {
            Some(group) => {
                format!("the replacement group {group} has no solid that is not empty space")
            }
            None => {
                "the design has no fluence limit and no solid with the role tf_magnet, so there \
                     is no response to compute; add a fluence limit to a replacement group"
                    .into()
            }
        }));
    }
    refuse_unsupported_source(&design)?;
    let fusion_power_mw = design
        .plasma
        .fusion_power_mw
        .ok_or_else(|| tool("the design's plasma block has no fusion power"))?;
    let history_setup = match (
        options.history_components.as_ref(),
        options.history_scenario.as_ref(),
        options.h3_component_id.as_deref(),
    ) {
        (None, None, None) => None,
        (Some(components), scenario_path, Some(h3_component)) => {
            if options.window_python.is_some() {
                return Err(tool(
                    "weight windows are prepared for the legacy fluence response only; history responses currently require analog transport",
                ));
            }
            let source = if let Some(scenario_path) = scenario_path {
                let scenario_path = scenario_path
                    .canonicalize()
                    .map_err(|e| io_error(scenario_path, e))?;
                let bytes = read_bounded(&scenario_path)?;
                let scenario = faris_model::LoadedScenario::from_bytes(&bytes).map_err(|e| {
                    tool(format!("history scenario {}: {e}", scenario_path.display()))
                })?;
                if !same_source_power_binding(
                    scenario.scenario.operating_plan.fusion_power_mw,
                    fusion_power_mw,
                ) {
                    return Err(tool(
                        "the CAD source fusion power does not exactly match the history scenario",
                    ));
                }
                HistorySourceContext::LegacyScenario {
                    scenario: Box::new(scenario),
                    path: scenario_path,
                    sha256: sha256_hex(&bytes),
                }
            } else {
                let design_identity = faris_model::design::CadSourceIdentity {
                    design_id: design.id.clone(),
                    design_sha256: design_sha256.clone(),
                    fusion_power_mw,
                };
                design_identity.validate().map_err(tool)?;
                HistorySourceContext::CadDesign(design_identity)
            };
            let response = history_response_request(&design, &report, components, h3_component)?;
            Some(HistorySetup { response, source })
        }
        _ => {
            return Err(tool(
                "CAD history responses require --history-component for every material solid and --h3-component; --history-scenario is optional and retained for the explicit legacy source-binding route",
            ));
        }
    };
    let chamber = design
        .plasma
        .chamber_solid_id
        .as_deref()
        .ok_or_else(|| tool("the design's plasma block names no chamber solid"))?;
    let history = linked_history(design_path, &design, fusion_power_mw);

    let dir = &options.out_dir;
    std::fs::create_dir(dir).map_err(|e| {
        io_error(
            dir,
            format!("{e} (the run directory must be new; nothing is overwritten)"),
        )
    })?;
    let dir = dir.canonicalize().map_err(|e| io_error(dir, e))?;
    let cross_sections = cross_sections
        .canonicalize()
        .map_err(|e| io_error(cross_sections, e))?;
    let cross_sections_sha256 = sha256_file(&cross_sections)?;
    let request = serde_json::json!({
        "schema": REQUEST_SCHEMA,
        "h5m": h5m,
        "cross_sections": cross_sections,
        "solids": solid_entries(&design, &report)?,
        "materials": material_entries(&design, &report)?,
        "plasma": design.plasma,
        "chamber_solid_id": chamber,
        "variance_reduction": if options.window_python.is_some() { "windows" } else { "none" },
        "response": {
            "solid_ids": response.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            "energy_min_ev": chosen.energy_min_ev,
            "peak_voxel_cm": chosen.peak_voxel_m.map(|v| v.map(|d| d.map(|m| m * 100.0))),
        },
        "history_response": history_setup.as_ref().map(|setup| &setup.response),
        "history_scenario": history_setup.as_ref().and_then(|setup| match &setup.source {
            HistorySourceContext::LegacyScenario { scenario, path, sha256 } => Some(serde_json::json!({
                "path": path.display().to_string(),
                "id": scenario.scenario.id,
                "sha256": sha256,
                "fusion_power_mw": scenario.scenario.operating_plan.fusion_power_mw,
            })),
            HistorySourceContext::CadDesign(_) => None,
        }),
        "history_cad_source": history_setup.as_ref().and_then(|setup| match &setup.source {
            HistorySourceContext::CadDesign(identity) => Some(identity),
            HistorySourceContext::LegacyScenario { .. } => None,
        }),
        "sampling": {
            "target_relative_error": options.target_relative_error,
            "particles_per_batch": options.particles_per_batch,
            "min_batches": MIN_BATCHES,
            "max_batches": options.max_histories / options.particles_per_batch,
            "max_seconds": options.max_seconds,
            "seed": options.seed,
            "threads": options.threads,
        },
    });
    let script = dir.join("design_transport.py");
    let helpers = dir.join("risk_tests");
    std::fs::create_dir(&helpers).map_err(|e| io_error(&helpers, e))?;
    for (path, text) in [
        (script.clone(), DESIGN_TRANSPORT_WORKER),
        (dir.join("streaming_aim.py"), STREAMING_AIM_HELPER),
        (dir.join("window_generator.py"), WINDOW_GENERATOR_HELPER),
        (dir.join("batch_statistics.py"), BATCH_STATISTICS_HELPER),
        (helpers.join("s0f_common.py"), AIM_COMMON_HELPER),
        (helpers.join("s0f_find_targets.py"), AIM_TARGETS_HELPER),
        (helpers.join("s0f_los_windows.py"), AIM_WINDOWS_HELPER),
    ] {
        std::fs::write(&path, text).map_err(|e| io_error(&path, e))?;
    }
    let request_path = dir.join("request.json");
    let request_bytes = serde_json::to_vec_pretty(&request).map_err(|e| tool(e.to_string()))?;
    std::fs::write(&request_path, &request_bytes).map_err(|e| io_error(&request_path, e))?;
    let solver = dir.join("solver");
    let tmp = dir.join("tmp");
    std::fs::create_dir_all(&tmp).map_err(|e| io_error(&tmp, e))?;
    let mut python_path = plasma_source.as_os_str().to_owned();
    if let Some(package) = &options.openmc_package_path {
        python_path.push(":");
        python_path.push(package);
    }
    let phase = |name: &str, interpreter: &Path, with_packages: bool, timeout: Duration| {
        let arguments: Vec<OsString> = vec![
            script.clone().into_os_string(),
            "--phase".into(),
            name.into(),
            "--request".into(),
            request_path.clone().into_os_string(),
            "--work-dir".into(),
            dir.clone().into_os_string(),
        ];
        let mut environment: Vec<(OsString, OsString)> = vec![
            ("TMPDIR".into(), tmp.clone().into_os_string()),
            // OpenMC initialises MPI, which refuses to start without a home directory.
            ("HOME".into(), dir.as_os_str().to_owned()),
            ("OMP_NUM_THREADS".into(), options.threads.to_string().into()),
        ];
        if with_packages {
            environment.push(("PYTHONPATH".into(), python_path.clone()));
        }
        if let Some(progress) = &options.progress {
            progress(TransportProgress::Step(name.to_string()));
        }
        match launch_python_with_lines(
            interpreter,
            arguments,
            &dir,
            timeout,
            &format!("the transport {name} step"),
            &options.cancellation,
            environment,
            ResourceLimits::with_address_space(CAD_ADDRESS_SPACE),
            &|line| {
                if let (Some(progress), Some(update)) = (&options.progress, parse_progress(line)) {
                    progress(update);
                }
            },
        )? {
            Launched::Succeeded(seconds) => Ok(seconds),
            Launched::Failed(text) => Err(tool(text)),
        }
    };
    let store = options.cache_dir.as_ref().map(ResultCache::open);
    let key = transport_cache_key(
        &request,
        &conversion.h5m_sha256,
        &cross_sections_sha256,
        options.window_python.as_deref(),
    );
    let entry = store.as_ref().map(|store| store.entry_dir(&key));
    // Every read of the kept entry is verified (AUTO-035); a refused one is reported and rebuilt.
    let uses_cache = options.window_python.is_some() || chosen.peak_voxel_m.is_some();
    let mut hit: Option<result_cache::Hit> = None;
    if let (Some(store), true) = (&store, uses_cache) {
        match store.lookup(&key) {
            Lookup::Hit(found) => hit = Some(found),
            Lookup::Miss => store.discard(&key),
            Lookup::Rejected(why) => {
                if let Some(progress) = &options.progress {
                    progress(TransportProgress::Step(why));
                }
                store.discard(&key);
            }
        }
    }
    let seal = |seconds: f64| -> Result<(), DesignError> {
        match &store {
            Some(store) => store
                .seal(&key, seconds)
                .map_err(|e| tool(format!("the transport cache: {e}"))),
            None => Ok(()),
        }
    };
    let mut setup_seconds = 0.0;
    let mut windows_reused = false;
    let mut cache_note: Option<String> = None;
    let mut reused_from: Option<CacheProvenance> = None;
    if let Some(window_python) = &options.window_python {
        windows_reused = match (&entry, &hit) {
            (Some(entry), Some(_)) => restore_windows(entry, &dir)?,
            _ => false,
        };
        if windows_reused {
            cache_note = hit.as_ref().map(|h| h.provenance.text());
            reused_from = hit.as_ref().map(|h| h.provenance.clone());
        } else {
            setup_seconds += phase("mgxs", python, true, MGXS_TIMEOUT)?;
            setup_seconds += phase("windows", window_python, false, WINDOWS_TIMEOUT)?;
            // Kept at once: a run that fails later does not cost the windows again.
            if let Some(entry) = &entry {
                keep_for_later(&dir, entry, false)?;
                seal(setup_seconds)?;
            }
        }
    }
    if chosen.peak_voxel_m.is_some() {
        let reused = match (&entry, &hit) {
            (Some(entry), Some(_)) => restore_pieces(entry, &dir)?,
            _ => false,
        };
        if !reused {
            setup_seconds += phase("volumes", python, true, VOLUMES_TIMEOUT)?;
            if let Some(entry) = &entry {
                for name in PIECE_FILES {
                    copy_file(&dir.join("peak").join(name), &entry.join("peak").join(name))?;
                }
                seal(0.0)?;
            }
        }
    }
    phase(
        "run",
        python,
        true,
        Duration::from_secs(options.max_seconds) + STARTUP_ALLOWANCE,
    )?;
    if let Some(entry) = &entry
        && options.window_python.is_some()
    {
        keep_for_later(&dir, entry, windows_reused)?;
        seal(0.0)?;
    }
    let result_path = solver.join("result.json");
    let worker_bytes = read_bounded(&result_path)?;
    let worker: WorkerResult = serde_json::from_slice(&worker_bytes)
        .map_err(|e| tool(format!("{}: {e}", result_path.display())))?;
    let history_rates_artifact = if let Some(setup) = &history_setup {
        let components = options
            .history_components
            .as_ref()
            .expect("paired history configuration");
        let h3_component_id = options
            .h3_component_id
            .as_deref()
            .expect("paired history configuration");
        let artifact = build_history_rates_artifact(
            &worker,
            &worker_bytes,
            &request,
            &request_bytes,
            components,
            h3_component_id,
            &setup.source,
            &design,
            &design_path
                .canonicalize()
                .map_err(|e| io_error(design_path, e))?,
            &options
                .check_report
                .canonicalize()
                .map_err(|e| io_error(&options.check_report, e))?,
            &h5m,
            &cross_sections,
            &dir,
            fusion_power_mw,
        )?;
        let mut bytes = serde_json::to_vec_pretty(&artifact).map_err(|e| tool(e.to_string()))?;
        bytes.push(b'\n');
        if bytes.len() > MAX_JSON_BYTES as usize {
            return Err(tool(format!(
                "CAD history-rate artifact exceeds the {} MiB JSON budget; reduce the number of mapped components",
                MAX_JSON_BYTES / (1024 * 1024)
            )));
        }
        let path = dir.join("history-driving-rates.json");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| io_error(&path, e))?;
        use std::io::Write as _;
        file.write_all(&bytes).map_err(|e| io_error(&path, e))?;
        Some(HistoryRatesArtifactRef {
            path: path.display().to_string(),
            sha256: sha256_hex(&bytes),
        })
    } else {
        if worker.history_response.is_some() {
            return Err(tool(
                "the OpenMC worker returned unrequested CAD history responses",
            ));
        }
        None
    };
    assemble(
        worker,
        Inputs {
            design_identity: DesignIdentity {
                path: design_path.display().to_string(),
                sha256: design_sha256,
                id: Some(design.id.clone()),
            },
            check_report_sha256: sha256_hex(&report_bytes),
            h5m_sha256: conversion.h5m_sha256.clone(),
            cross_sections_sha256,
            audit_cross_sections_sha256: report
                .material_audit
                .as_ref()
                .and_then(|a| a.cross_sections_sha256.clone()),
            run_directory: dir.display().to_string(),
            fusion_power_mw,
            response: &response,
            history_rates_artifact,
            design: &design,
            history,
            setup_seconds,
            windows_reused,
            cache_note,
            reused_from,
            chosen: &chosen,
        },
    )
}

struct Inputs<'a> {
    design_identity: DesignIdentity,
    check_report_sha256: String,
    h5m_sha256: String,
    cross_sections_sha256: String,
    audit_cross_sections_sha256: Option<String>,
    run_directory: String,
    fusion_power_mw: f64,
    response: &'a [(&'a str, f64)],
    history_rates_artifact: Option<HistoryRatesArtifactRef>,
    design: &'a Design,
    history: Result<PowerHistory, String>,
    setup_seconds: f64,
    windows_reused: bool,
    /// Where the reused windows came from (AUTO-039).
    cache_note: Option<String>,
    reused_from: Option<CacheProvenance>,
    chosen: &'a ChosenResponse,
}

#[allow(clippy::too_many_arguments)]
fn build_history_rates_artifact(
    worker: &WorkerResult,
    worker_bytes: &[u8],
    request: &serde_json::Value,
    request_bytes: &[u8],
    component_map: &BTreeMap<String, Vec<String>>,
    h3_component_id: &str,
    source_context: &HistorySourceContext,
    design: &Design,
    design_path: &Path,
    check_report_path: &Path,
    h5m_path: &Path,
    cross_sections_path: &Path,
    run_dir: &Path,
    fusion_power_mw: f64,
) -> Result<DesignHistoryRatesArtifact, DesignError> {
    let fail = |why: &str| tool(format!("CAD history response is not usable: {why}"));
    let expected = request
        .get("history_response")
        .filter(|value| !value.is_null())
        .ok_or_else(|| fail("transport request omitted the required response definitions"))?;
    let history = worker
        .history_response
        .as_ref()
        .ok_or_else(|| fail("worker omitted the required response"))?;
    validate_history_nuclear_data_identity(
        &history.nuclear_data_identity,
        &request["materials"],
        cross_sections_path,
    )
    .map_err(|why| fail(&why))?;
    if request.get("schema").and_then(serde_json::Value::as_str) != Some(REQUEST_SCHEMA)
        || request["sampling"]["target_relative_error"].as_f64()
            != Some(history.target_relative_error)
        || history.schema != "faris-cad-history-response/v0.1"
        || history.flux_energy_min_ev != 0.0
        || history.h3_score != "H3-production"
        || history.h3_total_scope != "whole_model"
        || history.estimator != "tracklength"
        || history.covariance_method != crate::transport::RESPONSE_COVARIANCE_METHOD
        || history.covariance_scope
            != "same-run sampling covariance across independent batch-integrated responses; no geometry-volume or nuclear-data uncertainty"
        || history.covariance_unit_order
            != "whole-model H3, blanket H3, then component flux cm/source_neutron"
        || u32::try_from(worker.batches).ok() != Some(history.batches)
        || history.batches < MIN_BATCHES as u32
        || history.target_relative_error.to_bits() != worker.target_relative_error.to_bits()
        || !history.target_reached
        || worker.batches.checked_mul(worker.particles_per_batch) != Some(worker.histories)
    {
        return Err(fail(
            "schema, all-energy response definition, batch history, or target completion does not match",
        ));
    }
    let lost = worker
        .lost_particles
        .ok_or_else(|| fail("OpenMC did not expose a final lost-particle count"))?;
    if worker.histories == 0 || lost as f64 / worker.histories as f64 > 1.0e-6 {
        return Err(fail(
            "lost-particle fraction exceeds the strict one-per-million limit",
        ));
    }
    let rows = expected["components"]
        .as_array()
        .ok_or_else(|| fail("request component rows are malformed"))?;
    let h3_ids = expected["h3_solid_ids"]
        .as_array()
        .ok_or_else(|| fail("request breeder solids are malformed"))?;
    let mut expected_ids = vec![
        "whole-model-tritium".to_string(),
        "blanket-tritium".to_string(),
    ];
    expected_ids.extend(rows.iter().map(|row| {
        format!(
            "history-flux:{}",
            row["component_id"].as_str().unwrap_or_default()
        )
    }));
    if history.response_ids != expected_ids || history.responses.len() != expected_ids.len() {
        return Err(fail(
            "response IDs or response count differ from the exact request",
        ));
    }
    let h3_solid_ids: Vec<String> = h3_ids
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    let expected_total_solids: Vec<String> = request["solids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|solid| !solid["material_id"].is_null())
        .filter_map(|solid| solid["solid_id"].as_str().map(str::to_string))
        .collect();
    if history.responses[0].response_id != "whole-model-tritium"
        || history.responses[0].solid_ids != expected_total_solids
        || history.responses[0].volume_m3.is_some()
        || history.responses[1].response_id != "blanket-tritium"
        || history.responses[1].solid_ids != h3_solid_ids
        || history.responses[1].volume_m3.is_some()
    {
        return Err(fail(
            "whole-model and breeder H3 responses do not bind their separate requested domains",
        ));
    }
    let n = history.responses.len();
    if history.covariance_raw_per_source.len() != n * n
        || history
            .covariance_raw_per_source
            .iter()
            .any(|value| !value.is_finite())
    {
        return Err(fail(
            "batch covariance is missing or has invalid dimensions/values",
        ));
    }
    for i in 0..n {
        let row = &history.responses[i];
        if !row.mean_raw_per_source.is_finite()
            || !row.standard_error_raw_per_source.is_finite()
            || row.standard_error_raw_per_source < 0.0
            || (i < 2 && row.mean_raw_per_source <= 0.0)
        {
            return Err(fail(
                "H3 or component score is absent, zero where required, or nonfinite",
            ));
        }
        let diagonal = history.covariance_raw_per_source[i * n + i];
        let se2 = faris_model::math::powi(row.standard_error_raw_per_source, 2);
        if (diagonal - se2).abs() > 1.0e-6 * diagonal.abs().max(se2.abs()).max(1.0e-300) {
            return Err(fail(
                "reported response standard error does not match batch covariance",
            ));
        }
        for j in 0..n {
            let a = history.covariance_raw_per_source[i * n + j];
            let b = history.covariance_raw_per_source[j * n + i];
            if (a - b).abs() > 1.0e-9 * a.abs().max(b.abs()).max(1.0e-300) {
                return Err(fail("batch covariance is asymmetric"));
            }
        }
    }
    let total_h3 = &history.responses[0];
    let breeder_h3 = &history.responses[1];
    if breeder_h3.mean_raw_per_source > total_h3.mean_raw_per_source * (1.0 + 1.0e-10) {
        return Err(fail("breeder H3 production exceeds whole-model production"));
    }
    for (index, (request_row, component_id)) in rows.iter().zip(component_map.keys()).enumerate() {
        let row = &history.responses[index + 2];
        let expected_volume = request_row["volume_m3"]
            .as_f64()
            .ok_or_else(|| fail("component volume is missing"))?;
        let expected_solids: Vec<String> = request_row["solid_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect();
        if request_row["component_id"].as_str() != Some(component_id.as_str())
            || row.response_id != format!("history-flux:{component_id}")
            || row.solid_ids != expected_solids
            || row.volume_m3.map(f64::to_bits) != Some(expected_volume.to_bits())
            || row.mean_raw_per_source <= 0.0
        {
            return Err(fail(
                "all-energy flux response domain or CAD volume differs from the request",
            ));
        }
    }
    let xml_path = Path::new(&worker.model_xml_path);
    if !xml_path.is_absolute()
        || !xml_path.starts_with(run_dir)
        || !xml_path.is_file()
        || !sha256_file(xml_path)?.eq_ignore_ascii_case(&worker.model_xml_sha256)
        || worker.openmc_library_sha256.len() != 64
        || !worker
            .openmc_library_sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        || !Path::new(&worker.openmc_library_path).is_absolute()
        || !sha256_file(Path::new(&worker.openmc_library_path))?
            .eq_ignore_ascii_case(&worker.openmc_library_sha256)
        || !Path::new(&worker.source_kernel_path).is_absolute()
        || !sha256_file(Path::new(&worker.source_kernel_path))?
            .eq_ignore_ascii_case(&worker.source_kernel_sha256)
    {
        return Err(fail(
            "run model XML or OpenMC library identity is missing or does not match its bytes",
        ));
    }
    let source_rate = source_rate_per_s(fusion_power_mw);
    let mut normalized_results = Vec::with_capacity(n);
    let total_volume_m3: f64 = rows
        .iter()
        .map(|row| row["volume_m3"].as_f64().unwrap_or(0.0))
        .sum();
    let breeder_volume = rows
        .iter()
        .find(|row| row["component_id"].as_str() == Some(h3_component_id))
        .and_then(|row| row["volume_m3"].as_f64())
        .ok_or_else(|| fail("breeder volume is missing from the component map"))?;
    let mut scales = vec![source_rate, source_rate];
    let total_h3_integrated = total_h3.mean_raw_per_source * source_rate;
    let total_h3_se = total_h3.standard_error_raw_per_source * source_rate;
    normalized_results.push(NormalizedTally {
        response_id: "whole-model-tritium".into(),
        domain: faris_model::transport::ResponseDomain::WholeModel,
        score: faris_model::transport::ScoreDefinition::ParticleProduction {
            particle: faris_model::transport::ProducedParticle::Tritium,
            score: "H3-production".into(),
        },
        estimator: TallyEstimator::Tracklength,
        mean: total_h3_integrated / total_volume_m3,
        standard_error: total_h3_se / total_volume_m3,
        unit: PhysicalUnit::ParticlesPerCubicMetreSecond,
        integrated_mean: total_h3_integrated,
        integrated_standard_error: total_h3_se,
        integrated_unit: PhysicalUnit::ParticlesPerSecond,
        volume_m3: total_volume_m3,
        volume_standard_error_m3: 0.0,
    });
    let h3 = breeder_h3;
    let h3_integrated = h3.mean_raw_per_source * source_rate;
    let h3_se = h3.standard_error_raw_per_source * source_rate;
    normalized_results.push(NormalizedTally {
        response_id: "blanket-tritium".into(),
        domain: faris_model::transport::ResponseDomain::Component {
            component_id: "blanket".into(),
        },
        score: faris_model::transport::ScoreDefinition::ParticleProduction {
            particle: faris_model::transport::ProducedParticle::Tritium,
            score: "H3-production".into(),
        },
        estimator: TallyEstimator::Tracklength,
        mean: h3_integrated / breeder_volume,
        standard_error: h3_se / breeder_volume,
        unit: PhysicalUnit::ParticlesPerCubicMetreSecond,
        integrated_mean: h3_integrated,
        integrated_standard_error: h3_se,
        integrated_unit: PhysicalUnit::ParticlesPerSecond,
        volume_m3: breeder_volume,
        volume_standard_error_m3: 0.0,
    });
    for (index, row) in rows.iter().enumerate() {
        let component_id = row["component_id"]
            .as_str()
            .ok_or_else(|| fail("component ID is missing"))?;
        let volume = row["volume_m3"]
            .as_f64()
            .ok_or_else(|| fail("component volume is invalid"))?;
        let sample = &history.responses[index + 2];
        let scale = source_rate * 0.01;
        scales.push(scale);
        let integrated = sample.mean_raw_per_source * scale;
        let integrated_se = sample.standard_error_raw_per_source * scale;
        normalized_results.push(NormalizedTally {
            response_id: sample.response_id.clone(),
            domain: faris_model::transport::ResponseDomain::Component {
                component_id: component_id.into(),
            },
            score: faris_model::transport::ScoreDefinition::Flux,
            estimator: TallyEstimator::Tracklength,
            mean: integrated / volume,
            standard_error: integrated_se / volume,
            unit: PhysicalUnit::NeutronsPerSquareMetreSecond,
            integrated_mean: integrated,
            integrated_standard_error: integrated_se,
            integrated_unit: PhysicalUnit::NeutronMetresPerSecond,
            volume_m3: volume,
            volume_standard_error_m3: 0.0,
        });
    }
    let integrated_covariance: Vec<f64> = history
        .covariance_raw_per_source
        .iter()
        .enumerate()
        .map(|(index, covariance)| covariance * scales[index / n] * scales[index % n])
        .collect();
    let plasma_bytes = serde_json::to_vec(&design.plasma).map_err(|e| tool(e.to_string()))?;
    let source_definition_sha256 = sha256_hex(&plasma_bytes);
    let component_map_bytes = serde_json::to_vec(component_map).map_err(|e| tool(e.to_string()))?;
    let response_definitions_bytes =
        serde_json::to_vec(expected).map_err(|e| tool(e.to_string()))?;
    let worker_result_sha256 = sha256_hex(worker_bytes);
    let (scenario_id, scenario_sha256, variant_id, source_identity) = match source_context {
        HistorySourceContext::LegacyScenario {
            scenario, sha256, ..
        } => (
            scenario.scenario.id.clone(),
            sha256.clone(),
            scenario
                .scenario
                .variants
                .first()
                .map_or_else(|| "cad-history".into(), |v| v.id.clone()),
            None,
        ),
        HistorySourceContext::CadDesign(identity) => (
            String::new(),
            String::new(),
            "cad-design-history".into(),
            Some(identity.clone()),
        ),
    };
    let (solver_identity, nuclear_data_identity) = history_tool_identities(
        &worker.openmc,
        &worker.openmc_library_sha256,
        cross_sections_path,
    )?;
    let normalized = NormalizedTransportResult {
        schema_version: "faris-normalized-transport/v0.1".into(),
        scenario_id,
        scenario_sha256,
        variant_id,
        source: faris_model::transport::DtSource {
            energy_per_reaction_ev: D_T_ENERGY_PER_REACTION_EV,
            neutron_energy_ev: D_T_NEUTRON_ENERGY_EV,
            neutrons_per_reaction: 1.0,
            distribution_id: format!(
                "openmc-plasma-source/tokamak-profile:{source_definition_sha256}"
            ),
        },
        solver: solver_identity,
        nuclear_data: nuclear_data_identity,
        histories: worker.histories,
        source_reaction_rate_per_s: reaction_rate_per_s(
            fusion_power_mw,
            D_T_ENERGY_PER_REACTION_EV,
        ),
        source_neutron_rate_per_s: source_rate,
        results: normalized_results,
        response_covariance: Some(ResponseCovariance {
            method: crate::transport::RESPONSE_COVARIANCE_METHOD.into(),
            batches: history.batches,
            response_ids: history.response_ids.clone(),
            integrated: integrated_covariance,
        }),
    };
    let normalized_validation = ValidationMarks(normalized.validation());
    let driving_rates = TransportDrivingRates::from_normalized_with_cad_identity(
        &normalized,
        fusion_power_mw,
        &worker_result_sha256,
        source_identity.clone(),
    )
    .map_err(|e| fail(&e))?;
    driving_rates.validate().map_err(|e| fail(&e))?;
    let rel_model = xml_path
        .strip_prefix(run_dir)
        .map_err(|_| fail("model XML escapes the run folder"))?;
    let (schema_version, scenario_path, scenario_id, scenario_sha256) = match source_context {
        HistorySourceContext::LegacyScenario {
            scenario,
            path,
            sha256,
        } => (
            DESIGN_HISTORY_RATES_VERSION,
            Some(path.display().to_string()),
            Some(scenario.scenario.id.clone()),
            Some(sha256.clone()),
        ),
        HistorySourceContext::CadDesign(_) => (DESIGN_HISTORY_RATES_CAD_VERSION, None, None, None),
    };
    Ok(DesignHistoryRatesArtifact {
        schema_version: schema_version.into(),
        scenario_path,
        scenario_id,
        scenario_sha256,
        cad_source_identity: source_identity,
        design_path: design_path.display().to_string(),
        design_sha256: sha256_file(design_path)?,
        check_report_path: check_report_path.display().to_string(),
        check_report_sha256: sha256_file(check_report_path)?,
        h5m_path: h5m_path.display().to_string(),
        h5m_sha256: sha256_file(h5m_path)?,
        cross_sections_path: cross_sections_path.display().to_string(),
        cross_sections_sha256: sha256_file(cross_sections_path)?,
        nuclear_data_identity: history.nuclear_data_identity.clone(),
        worker_result_path: "solver/result.json".into(),
        worker_result_sha256,
        transport_request_path: "request.json".into(),
        transport_request_sha256: sha256_hex(request_bytes),
        model_xml_path: rel_model.to_string_lossy().into_owned(),
        model_xml_sha256: worker.model_xml_sha256.clone(),
        openmc_library_path: worker.openmc_library_path.clone(),
        openmc_library_sha256: worker.openmc_library_sha256.clone(),
        source_definition_sha256,
        source_kernel_path: worker.source_kernel_path.clone(),
        source_kernel_sha256: worker.source_kernel_sha256.clone(),
        source_energy_per_reaction_ev: D_T_ENERGY_PER_REACTION_EV,
        source_neutron_energy_ev: D_T_NEUTRON_ENERGY_EV,
        fusion_power_mw,
        source_neutron_rate_per_s: source_rate,
        flux_energy_min_ev: 0.0,
        h3_score: "H3-production".into(),
        component_map: component_map.clone(),
        h3_component_id: h3_component_id.into(),
        h3_solid_ids: h3_solid_ids.clone(),
        component_map_sha256: sha256_hex(&component_map_bytes),
        response_definitions_sha256: sha256_hex(&response_definitions_bytes),
        histories: worker.histories,
        batches: history.batches,
        target_relative_error: history.target_relative_error,
        covariance_method: history.covariance_method.clone(),
        covariance_scope: history.covariance_scope.clone(),
        normalized_transport: normalized,
        validation: normalized_validation,
        total_h3_production_per_source_neutron: ScalarRate {
            mean: total_h3.mean_raw_per_source,
            standard_error: Some(total_h3.standard_error_raw_per_source),
            unit: "particles/source_neutron".into(),
            response_id: "whole-model-tritium".into(),
        },
        driving_rates,
    })
}

fn history_tool_identities(
    openmc_version: &str,
    openmc_library_sha256: &str,
    cross_sections_path: &Path,
) -> Result<(TransportToolIdentity, TransportToolIdentity), String> {
    Ok((
        TransportToolIdentity {
            name: "OpenMC C API".into(),
            version: openmc_version.into(),
            digest: sha256_identity(openmc_library_sha256, "OpenMC library")?,
        },
        TransportToolIdentity {
            name: "cross_sections.xml".into(),
            version: "sha256-pinned".into(),
            digest: sha256_identity(&sha256_file(cross_sections_path)?, "cross_sections.xml")?,
        },
    ))
}

/// Convert an already measured SHA-256 hex digest into the identity syntax required by
/// `TransportToolIdentity`. This only prefixes the recorded identity; it never derives a hash
/// from a display version or another descriptive string.
fn sha256_identity(hex: &str, label: &str) -> Result<String, String> {
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("{label} identity is not a 64-hex SHA-256 digest"));
    }
    Ok(format!("sha256:{}", hex.to_ascii_lowercase()))
}

fn validate_history_nuclear_data_identity(
    identity: &HistoryNuclearDataIdentity,
    materials: &serde_json::Value,
    cross_sections_path: &Path,
) -> Result<(), String> {
    let fail = |why: &str| format!("referenced nuclear-data identity {why}");
    let xml_metadata = cross_sections_path.metadata().map_err(|e| e.to_string())?;
    if identity.schema != "faris-cad-history-neutron-data/v0.1"
        || identity.scope
            != "material neutron nuclides; photon transport and thermal-scattering tables disabled"
        || identity.tables.is_empty()
        || identity.tables.len() > 4096
        || !xml_metadata.is_file()
        || xml_metadata.len() > 64 * 1024 * 1024
        || sha256_file(cross_sections_path).map_err(|e| e.to_string())?
            != identity.cross_sections_xml_sha256
    {
        return Err(fail(
            "is missing, unbounded, or not bound to cross_sections.xml",
        ));
    }
    let mut required = std::collections::BTreeSet::new();
    for material in materials
        .as_array()
        .ok_or_else(|| fail("has no material map"))?
    {
        if let Some(nuclides) = material["nuclide_atom_fractions"].as_object() {
            required.extend(nuclides.keys().cloned());
        }
    }
    let mut found = std::collections::BTreeSet::new();
    let mut paths = std::collections::BTreeSet::new();
    let mut total_bytes = 0u64;
    for table in &identity.tables {
        let path = Path::new(&table.path);
        let canonical = path.canonicalize().map_err(|e| e.to_string())?;
        let metadata = canonical.metadata().map_err(|e| e.to_string())?;
        if !path.is_absolute()
            || canonical != path
            || !metadata.is_file()
            || metadata.len() != table.bytes
            || table.bytes > 12 * 1024 * 1024 * 1024
            || table.sha256.len() != 64
            || !table
                .sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || !sha256_file(&canonical)
                .map_err(|e| e.to_string())?
                .eq_ignore_ascii_case(&table.sha256)
            || table.nuclides.is_empty()
            || table.nuclides.len() != table.library_indices.len()
            || !paths.insert(canonical)
        {
            return Err(fail(
                "contains a changed, malformed, or unbounded table reference",
            ));
        }
        total_bytes = total_bytes
            .checked_add(table.bytes)
            .ok_or_else(|| fail("byte count overflowed"))?;
        if total_bytes > 64 * 1024 * 1024 * 1024 {
            return Err(fail("exceeds the 64 GiB aggregate byte bound"));
        }
        for nuclide in &table.nuclides {
            if !found.insert(nuclide.clone()) {
                return Err(fail("maps a nuclide more than once"));
            }
        }
    }
    if found != required {
        return Err(fail("does not map exactly the model's material nuclides"));
    }
    Ok(())
}

/// Re-read and re-bind a CAD history response artifact before the maintenance history consumes it.
/// Hashes verify file identity; they do not constitute independent physical validation.
pub fn load_design_history_rates(
    artifact_path: &Path,
    scenario: &faris_model::LoadedScenario,
) -> Result<TransportDrivingRates, String> {
    load_design_history_rates_from_source(
        artifact_path,
        ExpectedHistorySource::LegacyScenario(Box::new(scenario.clone())),
    )
}

/// Re-read and re-bind CAD-native history rates to the exact CAD design. Operating-history
/// assumptions remain independent planning inputs and do not identify the neutron response.
pub fn load_cad_design_history_rates(
    artifact_path: &Path,
    design_path: &Path,
) -> Result<TransportDrivingRates, String> {
    load_design_history_rates_from_source(
        artifact_path,
        ExpectedHistorySource::CadDesign(design_path.to_path_buf()),
    )
}

#[derive(Clone, Debug)]
enum ExpectedHistorySource {
    LegacyScenario(Box<faris_model::LoadedScenario>),
    CadDesign(PathBuf),
}

fn load_design_history_rates_from_source(
    artifact_path: &Path,
    expected_source: ExpectedHistorySource,
) -> Result<TransportDrivingRates, String> {
    let bytes = read_bounded(artifact_path).map_err(|e| e.to_string())?;
    let artifact: DesignHistoryRatesArtifact = serde_json::from_slice(&bytes)
        .map_err(|e| format!("CAD history rates artifact JSON: {e}"))?;
    let fail = |why: &str| format!("CAD history rates artifact is not bound to this run: {why}");
    let source_context = match expected_source {
        ExpectedHistorySource::LegacyScenario(scenario) => {
            if artifact.schema_version != DESIGN_HISTORY_RATES_VERSION
                || artifact.cad_source_identity.is_some()
            {
                return Err(fail(
                    "legacy route requires a v0.1 scenario-bound CAD record",
                ));
            }
            validate_history_scenario_binding(
                artifact
                    .scenario_id
                    .as_deref()
                    .ok_or_else(|| fail("legacy history artifact has no scenario ID"))?,
                artifact
                    .scenario_sha256
                    .as_deref()
                    .ok_or_else(|| fail("legacy history artifact has no scenario hash"))?,
                artifact.fusion_power_mw,
                &artifact.driving_rates.scenario_sha256,
                artifact.driving_rates.reference_fusion_power_mw,
                &scenario,
            )
            .map_err(|e| fail(&e))?;
            let path = PathBuf::from(
                artifact
                    .scenario_path
                    .as_deref()
                    .ok_or_else(|| fail("legacy history artifact has no scenario path"))?,
            );
            if !path.is_absolute()
                || !sha256_file(&path)
                    .map_err(|e| e.to_string())?
                    .eq_ignore_ascii_case(artifact.scenario_sha256.as_deref().unwrap_or_default())
            {
                return Err(fail(
                    "history scenario file does not match its recorded source identity",
                ));
            }
            HistorySourceContext::LegacyScenario {
                scenario,
                path,
                sha256: artifact.scenario_sha256.clone().unwrap_or_default(),
            }
        }
        ExpectedHistorySource::CadDesign(expected_path) => {
            if artifact.schema_version != DESIGN_HISTORY_RATES_CAD_VERSION
                || artifact.scenario_path.is_some()
                || artifact.scenario_id.is_some()
                || artifact.scenario_sha256.is_some()
            {
                return Err(fail(
                    "CAD-native route requires a v0.2 design-bound record without scenario identity",
                ));
            }
            let expected_path = expected_path.canonicalize().map_err(|e| e.to_string())?;
            let artifact_design_path = Path::new(&artifact.design_path)
                .canonicalize()
                .map_err(|e| e.to_string())?;
            if expected_path != artifact_design_path {
                return Err(fail("CAD design path differs from the history artifact"));
            }
            let design_bytes = read_bounded(&expected_path).map_err(|e| e.to_string())?;
            if sha256_hex(&design_bytes) != artifact.design_sha256 {
                return Err(fail("CAD design bytes changed after the transport run"));
            }
            let design = design::parse_design(&design_bytes)
                .design
                .ok_or_else(|| fail("CAD design does not parse"))?;
            let identity = faris_model::design::CadSourceIdentity {
                design_id: design.id,
                design_sha256: artifact.design_sha256.clone(),
                fusion_power_mw: design
                    .plasma
                    .fusion_power_mw
                    .ok_or_else(|| fail("CAD design has no fusion source power"))?,
            };
            identity.validate().map_err(|e| fail(&e))?;
            if artifact.cad_source_identity.as_ref() != Some(&identity)
                || artifact.driving_rates.cad_source_identity.as_ref() != Some(&identity)
                || !artifact.driving_rates.scenario_sha256.is_empty()
                || artifact.fusion_power_mw.to_bits() != identity.fusion_power_mw.to_bits()
                || artifact.driving_rates.reference_fusion_power_mw.to_bits()
                    != identity.fusion_power_mw.to_bits()
            {
                return Err(fail(
                    "CAD source identity or source power differs from the history rates",
                ));
            }
            HistorySourceContext::CadDesign(identity)
        }
    };
    if artifact.driving_rates.transport_artifact_sha256 != artifact.worker_result_sha256
        || artifact.flux_energy_min_ev != 0.0
        || artifact.h3_score != "H3-production"
        || artifact.h3_component_id != "blanket"
        || artifact.driving_rates.transport_deposited_heat_w.is_some()
    {
        return Err(fail(
            "scenario, power, response definition, or sampling provenance differs",
        ));
    }
    let base = artifact_path
        .parent()
        .ok_or_else(|| fail("artifact has no parent folder"))?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let relative_file = |name: &str| -> Result<PathBuf, String> {
        let path = Path::new(name);
        if path.is_absolute()
            || path
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(fail(
                "run artifact references must be normalized relative paths",
            ));
        }
        let resolved = base.join(path).canonicalize().map_err(|e| e.to_string())?;
        if !resolved.starts_with(&base) || !resolved.is_file() {
            return Err(fail(
                "run artifact reference escapes its run folder or is not a regular file",
            ));
        }
        Ok(resolved)
    };
    let worker_path = relative_file(&artifact.worker_result_path)?;
    let worker_bytes = read_bounded(&worker_path).map_err(|e| e.to_string())?;
    if sha256_hex(&worker_bytes) != artifact.worker_result_sha256 {
        return Err(fail(
            "worker result bytes do not match their recorded digest",
        ));
    }
    let request_path = relative_file(&artifact.transport_request_path)?;
    let request_bytes = read_bounded(&request_path).map_err(|e| e.to_string())?;
    if sha256_hex(&request_bytes) != artifact.transport_request_sha256 {
        return Err(fail(
            "transport request bytes do not match their recorded digest",
        ));
    }
    let transport_request: serde_json::Value =
        serde_json::from_slice(&request_bytes).map_err(|e| e.to_string())?;
    let design_path = Path::new(&artifact.design_path);
    let check_path = Path::new(&artifact.check_report_path);
    let h5m_path = Path::new(&artifact.h5m_path);
    let xs_path = Path::new(&artifact.cross_sections_path);
    for (path, digest, label) in [
        (design_path, &artifact.design_sha256, "design"),
        (check_path, &artifact.check_report_sha256, "check report"),
        (h5m_path, &artifact.h5m_sha256, "H5M"),
        (xs_path, &artifact.cross_sections_sha256, "cross sections"),
    ] {
        if !path.is_absolute()
            || !sha256_file(path)
                .map_err(|e| e.to_string())?
                .eq_ignore_ascii_case(digest)
        {
            return Err(fail(&format!(
                "{label} bytes do not match their recorded path and digest"
            )));
        }
    }
    let design_bytes = read_bounded(design_path).map_err(|e| e.to_string())?;
    if sha256_hex(&design_bytes) != artifact.design_sha256 {
        return Err(fail("design bytes changed after the digest check"));
    }
    let design = design::parse_design(&design_bytes)
        .design
        .ok_or_else(|| fail("bound CAD design does not parse"))?;
    let check_bytes = read_bounded(check_path).map_err(|e| e.to_string())?;
    let check: ImportReport = serde_json::from_slice(&check_bytes).map_err(|e| e.to_string())?;
    accept_check_report(&check, &artifact.design_sha256).map_err(|e| e.to_string())?;
    let checked_h5m = converted_model(
        &check
            .geometry
            .conversion
            .as_ref()
            .ok_or_else(|| fail("check report has no conversion"))?
            .directory,
    )
    .map_err(|e| e.to_string())?
    .canonicalize()
    .map_err(|e| e.to_string())?;
    if checked_h5m != h5m_path.canonicalize().map_err(|e| e.to_string())? {
        return Err(fail(
            "H5M reference differs from the CAD check report conversion",
        ));
    }
    let request = history_response_request(
        &design,
        &check,
        &artifact.component_map,
        &artifact.h3_component_id,
    )
    .map_err(|e| e.to_string())?;
    let (expected_scenario, expected_cad_source) = match &source_context {
        HistorySourceContext::LegacyScenario {
            scenario,
            path,
            sha256,
        } => (
            serde_json::json!({
                "path": path.display().to_string(),
                "id": scenario.scenario.id,
                "sha256": sha256,
                "fusion_power_mw": scenario.scenario.operating_plan.fusion_power_mw,
            }),
            serde_json::Value::Null,
        ),
        HistorySourceContext::CadDesign(identity) => (
            serde_json::Value::Null,
            serde_json::to_value(identity).map_err(|e| e.to_string())?,
        ),
    };
    let expected_plasma = serde_json::to_value(&design.plasma).map_err(|e| e.to_string())?;
    let expected_solids = solid_entries(&design, &check).map_err(|e| e.to_string())?;
    let expected_materials = material_entries(&design, &check).map_err(|e| e.to_string())?;
    if transport_request.get("history_scenario") != Some(&expected_scenario)
        || transport_request.get("history_cad_source") != Some(&expected_cad_source)
        || transport_request.get("history_response") != Some(&request)
        || transport_request.get("plasma") != Some(&expected_plasma)
        || transport_request.get("solids") != Some(&serde_json::Value::Array(expected_solids))
        || transport_request.get("materials") != Some(&serde_json::Value::Array(expected_materials))
        || transport_request["sampling"]["target_relative_error"].as_f64()
            != Some(artifact.target_relative_error)
        || transport_request["response"]["energy_min_ev"]
            .as_f64()
            .is_none_or(|threshold| !threshold.is_finite())
        || Path::new(transport_request["h5m"].as_str().unwrap_or_default())
            .canonicalize()
            .ok()
            != h5m_path.canonicalize().ok()
        || Path::new(
            transport_request["cross_sections"]
                .as_str()
                .unwrap_or_default(),
        )
        .canonicalize()
        .ok()
            != xs_path.canonicalize().ok()
    {
        return Err(fail(
            "transport request source, material, component, or response definitions differ from bound CAD inputs",
        ));
    }
    let worker: WorkerResult = serde_json::from_slice(&worker_bytes).map_err(|e| e.to_string())?;
    let rebuilt = build_history_rates_artifact(
        &worker,
        &worker_bytes,
        &transport_request,
        &request_bytes,
        &artifact.component_map,
        &artifact.h3_component_id,
        &source_context,
        &design,
        design_path,
        check_path,
        h5m_path,
        xs_path,
        &base,
        artifact.fusion_power_mw,
    )
    .map_err(|e| e.to_string())?;
    if rebuilt != artifact {
        return Err(fail(
            "recomputed response values or CAD/source/run provenance differ from the artifact",
        ));
    }
    Ok(rebuilt.driving_rates)
}

/// Check the worker's answer against the request and convert it to fluxes.
fn assemble(worker: WorkerResult, inputs: Inputs) -> Result<DesignTransportReport, DesignError> {
    if worker.schema != RESULT_SCHEMA || worker.unit != "cm_per_source" {
        return Err(tool(
            "the transport worker's result has an unknown schema or unit",
        ));
    }
    if !worker.h5m_sha256.eq_ignore_ascii_case(&inputs.h5m_sha256) {
        return Err(tool(
            "the transport worker ran a different geometry than the checked one",
        ));
    }
    if worker.histories != worker.batches * worker.particles_per_batch
        || worker.solids.len() != inputs.response.len()
        || worker
            .solids
            .iter()
            .zip(inputs.response)
            .any(|(w, (id, _))| w.solid_id != *id)
    {
        return Err(tool(
            "the transport worker's result does not match its request",
        ));
    }
    let finite = |v: f64| v.is_finite() && v >= 0.0;
    if !finite(worker.total.mean)
        || worker.solids.iter().any(|s| !finite(s.mean))
        || worker
            .solids
            .iter()
            .filter_map(|s| s.peak.as_ref())
            .any(|p| !finite(p.mean) || !(p.volume_cm3.is_finite() && p.volume_cm3 > 0.0))
    {
        return Err(tool(
            "the transport worker returned a value that is not a finite score",
        ));
    }
    let rate = source_rate_per_s(inputs.fusion_power_mw);
    let relative = |mean: f64, se: Option<f64>| se.filter(|_| mean > 0.0).map(|se| se / mean);
    let total_volume_m3: f64 = inputs.response.iter().map(|(_, v)| v).sum();
    let solids: Vec<SolidFlux> = worker
        .solids
        .iter()
        .zip(inputs.response)
        .map(|(w, (id, volume))| SolidFlux {
            solid_id: (*id).to_string(),
            volume_m3: *volume,
            flux_n_m2_s: flux_n_m2_s(w.mean, rate, *volume),
            relative_error: relative(w.mean, w.standard_error),
            peak: w.peak.as_ref().map(|p| PeakFlux {
                r_m: p.r_cm.map(|v| v / 100.0),
                z_m: p.z_cm.map(|v| v / 100.0),
                volume_m3: p.volume_cm3 / 1.0e6,
                flux_n_m2_s: flux_n_m2_s(p.mean, rate, p.volume_cm3 / 1.0e6),
                relative_error: relative(p.mean, p.standard_error),
                pieces_ranked: p.pieces_ranked,
            }),
        })
        .collect();
    let mut notices =
        vec!["Volumes are the CAD volumes of the design file's fingerprints.".to_string()];
    if let Some(audited) = &inputs.audit_cross_sections_sha256
        && !audited.eq_ignore_ascii_case(&inputs.cross_sections_sha256)
    {
        notices.push(
            "The nuclear data of this run is not the library the design check audited.".into(),
        );
    }
    let mean_flux = flux_n_m2_s(worker.total.mean, rate, total_volume_m3);
    let mean_relative_error = relative(worker.total.mean, worker.total.standard_error);
    if inputs.chosen.peak_voxel_m.is_some() {
        notices.push(
            "The highest piece is the highest the run has resolved (its flux less two standard \
             errors); a piece with few scores could be higher."
                .into(),
        );
    }
    if inputs.chosen.peak_ignores_dphi {
        notices.push(
            "The limit's toroidal voxel width is not applied: each piece spans the solid's full \
             toroidal width."
                .into(),
        );
    }
    match worker.lost_particles {
        Some(0) => {}
        Some(lost) => notices.push(format!(
            "{lost} of {} neutrons could not be placed in the geometry (a run fails above one in a \
             million); the first positions are in the report.",
            worker.histories
        )),
        None => {}
    }
    if let Some(notice) =
        lost_position_provenance_notice(worker.lost_particles, &worker.lost_positions_cm)
    {
        notices.push(notice.into());
    }
    let mut fluence_limits = Vec::new();
    let mut not_evaluated = Vec::new();
    if worker.lost_particles.is_none() {
        not_evaluated.push(NotEvaluated {
            item: "Lost neutrons".into(),
            why:
                "This OpenMC build does not report how many neutrons the geometry could not place."
                    .into(),
            next_step: "Run with OpenMC 0.15.3 or later built as a shared library.".into(),
        });
    }
    if inputs.windows_reused {
        notices.push(match &inputs.cache_note {
            Some(note) => {
                format!("The weight windows of an earlier run on this model were used: {note}.")
            }
            None => "The weight windows of an earlier run on this model were used.".into(),
        });
    }
    if worker.variance_reduction.aim_reused {
        notices.push("The channel search of an earlier run on this model was used.".into());
    }
    for group in &inputs.design.replacement_groups {
        for limit in &group.limits {
            let item = format!(
                "{}: {} against the limit of {:e}",
                group.id,
                limit.metric.label(),
                limit.limit
            );
            let scored = inputs.chosen.group_id.as_deref() == Some(group.id.as_str())
                && fluence_threshold(limit) == Some(worker.energy_min_ev);
            if !scored {
                let (why, next_step) = if fluence_threshold(limit).is_some() {
                    (
                        "This run scored another group or another energy threshold.",
                        format!("Run again with --group {}.", group.id),
                    )
                } else {
                    (
                        "This command computes neutron fluence limits only.",
                        "Not available from this command yet.".to_string(),
                    )
                };
                not_evaluated.push(NotEvaluated {
                    item,
                    why: why.into(),
                    next_step,
                });
                continue;
            }
            // (basis, solid, flux, relative error) of the value the limit's averaging names.
            let judged = if inputs.chosen.peak_voxel_m.is_some() {
                leading_peak(&solids).map(|(s, p)| {
                    (
                        "peak_piece",
                        Some(s.solid_id.clone()),
                        p.flux_n_m2_s,
                        p.relative_error,
                    )
                })
            } else {
                Some(("group_average", None, mean_flux, mean_relative_error))
            };
            match (judged, &inputs.history) {
                (Some((basis, solid_id, flux, error)), Ok(history)) => {
                    fluence_limits.push(FluenceLimit {
                        group_id: group.id.clone(),
                        basis: basis.into(),
                        solid_id,
                        assessment: assess_fluence(flux, error, limit.limit, history),
                    })
                }
                (None, _) => not_evaluated.push(NotEvaluated {
                    item,
                    why: "No piece of any solid of the group could be ranked.".into(),
                    next_step: "Use a larger voxel in the limit's averaging.".into(),
                }),
                (_, Err(problem)) => not_evaluated.push(NotEvaluated {
                    item,
                    why: problem.clone(),
                    next_step: "Link a valid operating history in the design file and run again."
                        .into(),
                }),
            }
        }
    }
    if worker.stop_reason == "lost_particles" {
        // Too many neutrons were lost for the fluxes to be trusted: no limit is judged on them.
        for limit in fluence_limits.drain(..) {
            not_evaluated.push(NotEvaluated {
                item: format!("{}: fluence limit", limit.group_id),
                why: worker.why.clone().unwrap_or_default(),
                next_step: worker.next_step.clone().unwrap_or_default(),
            });
        }
    }
    Ok(DesignTransportReport {
        schema_version: DESIGN_TRANSPORT_VERSION.into(),
        design: inputs.design_identity,
        check_report_sha256: inputs.check_report_sha256,
        h5m_sha256: inputs.h5m_sha256,
        cross_sections_sha256: inputs.cross_sections_sha256,
        openmc: worker.openmc,
        run_directory: inputs.run_directory,
        energy_min_ev: worker.energy_min_ev,
        response_group_id: inputs.chosen.group_id.clone(),
        source_rate_per_s: rate,
        energy_per_reaction_ev: D_T_ENERGY_PER_REACTION_EV,
        fusion_power_mw: inputs.fusion_power_mw,
        histories: worker.histories,
        elapsed_seconds: worker.elapsed_seconds,
        seed: worker.seed,
        threads: worker.threads,
        target_relative_error: worker.target_relative_error,
        target_reached: worker.target_reached,
        stop_reason: worker.stop_reason,
        why: worker.why,
        next_step: worker.next_step,
        mean_flux_n_m2_s: mean_flux,
        mean_relative_error,
        variance_of_variance: worker.variance_of_variance,
        total_volume_m3,
        solids,
        setup_seconds: inputs.setup_seconds + worker.variance_reduction.aim_seconds,
        windows_reused: inputs.windows_reused,
        reused_from: inputs.reused_from.clone(),
        variance_reduction: worker.variance_reduction,
        fluence_limits,
        lost_particles: worker.lost_particles,
        lost_positions_m: worker
            .lost_positions_cm
            .iter()
            .map(|p| p.map(|v| v / 100.0))
            .collect(),
        notices,
        not_evaluated,
        history_rates_artifact: inputs.history_rates_artifact,
        validation: ValidationMarks::for_catalogue(DESIGN_TRANSPORT_OUTPUTS),
    })
}

fn percent(relative: Option<f64>) -> String {
    relative.map_or_else(
        || "error unknown".into(),
        |r| format!("± {:.1} %", 100.0 * r),
    )
}

/// The solid whose leading piece is the highest the run has resolved: the highest flux after two
/// standard errors are taken off, as the worker ranks pieces within a solid. With nothing
/// resolved above zero, the highest flux.
pub fn leading_peak(solids: &[SolidFlux]) -> Option<(&SolidFlux, &PeakFlux)> {
    let resolved = |p: &PeakFlux| p.flux_n_m2_s * (1.0 - 2.0 * p.relative_error.unwrap_or(1.0));
    let peaks = || solids.iter().filter_map(|s| Some((s, s.peak.as_ref()?)));
    let best = peaks().max_by(|a, b| resolved(a.1).total_cmp(&resolved(b.1)))?;
    if resolved(best.1) > 0.0 {
        Some(best)
    } else {
        peaks().max_by(|a, b| a.1.flux_n_m2_s.total_cmp(&b.1.flux_n_m2_s))
    }
}

/// A time to a limit in words: years, or days when it is under a tenth of a year.
pub fn span_text(years: f64) -> String {
    if years >= 0.1 {
        format!("{years:.1} years")
    } else {
        format!(
            "{:.0} days",
            years * (faris_model::units::JULIAN_YEAR_S / faris_model::units::DAY_S)
        )
    }
}

/// The fluence's 2-sigma band; the verdict never says WITHIN unless the upper edge is inside.
fn fluence_band(a: &FluenceAssessment) -> String {
    match (a.history_fluence_low_n_m2, a.history_fluence_high_n_m2) {
        (Some(low), Some(high)) => format!(
            "(2-sigma band {low:.3e} to {high:.3e}; the verdict uses the unfavourable edge)"
        ),
        _ => "(no error band: the flux has no error estimate)".into(),
    }
}

/// The years to the limit with their 2-sigma band; the earliest edge is the one the verdict uses.
fn years_band(earliest: Option<f64>, latest: Option<f64>) -> String {
    match (earliest, latest) {
        (Some(e), Some(l)) => format!("(2-sigma band {} to {})", span_text(e), span_text(l)),
        (Some(e), None) => format!("(2-sigma band from {}, no upper edge)", span_text(e)),
        _ => "(no error band: the flux has no error estimate)".into(),
    }
}

/// The report as text for the terminal.
pub fn render_transport_text(report: &DesignTransportReport) -> String {
    let mut out = String::new();
    let source_power =
        dt_source_power_summary(report.fusion_power_mw, report.energy_per_reaction_ev);
    let what = if report.energy_min_ev > 0.0 {
        format!("Neutron flux above {} MeV", report.energy_min_ev / 1.0e6)
    } else {
        "Neutron flux at all energies".to_string()
    };
    let place = match &report.response_group_id {
        Some(group) => format!("the group {group}"),
        None => "the TF magnets".to_string(),
    };
    let _ = writeln!(
        out,
        "{what} in {place} of {}",
        report.design.id.as_deref().unwrap_or("the design")
    );
    let flux_tag = report.validation.tag("flux_n_m2_s", Quantity::Flux);
    let fluence_tag = report
        .validation
        .tag("history_fluence_n_m2", Quantity::Fluence);
    let _ = writeln!(
        out,
        "  Mean over {} solids: {:.3e} n/m²/s {} {}",
        report.solids.len(),
        report.mean_flux_n_m2_s,
        percent(report.mean_relative_error),
        report.validation.tag("mean_flux_n_m2_s", Quantity::Flux)
    );
    if let Some(highest) = report
        .solids
        .iter()
        .max_by(|a, b| a.flux_n_m2_s.total_cmp(&b.flux_n_m2_s))
    {
        let _ = writeln!(
            out,
            "  Highest solid: {} at {:.3e} n/m²/s {} {flux_tag}",
            highest.solid_id,
            highest.flux_n_m2_s,
            percent(highest.relative_error)
        );
    }
    if let Some((solid, peak)) = leading_peak(&report.solids) {
        let _ = writeln!(
            out,
            "  Highest piece: {:.3e} n/m²/s {} in {} at r {:.2}–{:.2} m, z {:.2}–{:.2} m ({:.1} litres) {flux_tag}",
            peak.flux_n_m2_s,
            percent(peak.relative_error),
            solid.solid_id,
            peak.r_m[0],
            peak.r_m[1],
            peak.z_m[0],
            peak.z_m[1],
            peak.volume_m3 * 1.0e3
        );
    }
    let target = 100.0 * report.target_relative_error;
    let on = if report.solids.iter().any(|s| s.peak.is_some()) {
        format!("the mean and {} % on the highest piece", 2.0 * target)
    } else {
        "the mean".to_string()
    };
    if report.target_reached {
        let _ = writeln!(out, "  Target of {target} % on {on}: reached.");
        if let Some(vov) = report.variance_of_variance {
            let _ = writeln!(
                out,
                "  Batches agree: variance of the variance {vov:.3} (accepted at or below 0.1)."
            );
        }
    } else {
        let _ = writeln!(out, "  Target of {target} % on {on}: NOT reached.");
        if let (Some(why), Some(next)) = (&report.why, &report.next_step) {
            let _ = writeln!(out, "    Why: {why}");
            let _ = writeln!(out, "    Next: {next}");
        }
    }
    let _ = writeln!(
        out,
        "  {} histories in {:.0} s on {} thread(s); source {:.3e} n/s from {:.1} MW fusion at {:.1} MeV per reaction. Nominal D-T neutron power: {:.1} MW ({:.1}/{:.1} MeV = {:.1}% of fusion power); source convention, not a transport heating tally or energy-closure result.",
        report.histories,
        report.elapsed_seconds,
        report.threads,
        report.source_rate_per_s,
        report.fusion_power_mw,
        report.energy_per_reaction_ev / 1.0e6,
        source_power.nominal_neutron_power_mw,
        D_T_NEUTRON_ENERGY_EV / 1.0e6,
        report.energy_per_reaction_ev / 1.0e6,
        source_power.neutron_power_fraction * 100.0
    );
    if let Some(lost) = report.lost_particles {
        let _ = writeln!(
            out,
            "  Lost neutrons: {lost} of {} (a run fails above one in a million).",
            report.histories
        );
    }
    let reduction = &report.variance_reduction;
    let _ = writeln!(
        out,
        "  Speed-up: {} ({:.0} s of preparation). {}",
        reduction.mode, report.setup_seconds, reduction.reason
    );
    if reduction.mode != "aimed" {
        let _ = writeln!(out, "    Next: {}", reduction.next_step);
    }
    for limit in &report.fluence_limits {
        let a = &limit.assessment;
        let judged = match &limit.solid_id {
            Some(solid) => format!("highest piece, in {solid}"),
            None => "mean over the group".to_string(),
        };
        let _ = writeln!(
            out,
            "  Fluence limit of {} ({:e} n/m²), {judged}: {}",
            limit.group_id,
            a.limit_n_m2,
            match a.status {
                LimitStatus::Within => "WITHIN",
                LimitStatus::Exceeded => "EXCEEDED",
                LimitStatus::Inconclusive => "INCONCLUSIVE",
            }
        );
        let _ = writeln!(
            out,
            "    Over the {:.1}-year history ({:.1} years at full power): {:.3e} n/m² {} {fluence_tag}",
            a.history_calendar_years,
            a.history_full_power_years,
            a.history_fluence_n_m2,
            fluence_band(a)
        );
        if let (Some(full), Some(calendar)) =
            (a.full_power_years_to_limit, a.calendar_years_to_limit)
        {
            let _ = writeln!(
                out,
                "    Reaches the limit after {} at full power ({} of this history) {} {}",
                span_text(full),
                span_text(calendar),
                years_band(
                    a.full_power_years_to_limit_earliest,
                    a.full_power_years_to_limit_latest
                ),
                report
                    .validation
                    .tag("full_power_years_to_limit", Quantity::Fluence)
            );
        }
        let _ = writeln!(out, "    Why: {}", a.why);
        let _ = writeln!(out, "    Next: {}", a.next_step);
    }
    for notice in &report.notices {
        let _ = writeln!(out, "  Note: {notice}");
    }
    for item in &report.not_evaluated {
        let _ = writeln!(out, "  Not evaluated: {}", item.item);
        let _ = writeln!(out, "    Why: {}", item.why);
        let _ = writeln!(out, "    Next: {}", item.next_step);
    }
    if let Some(artifact) = &report.history_rates_artifact {
        let _ = writeln!(
            out,
            "  CAD operating-history rates: {} (hash-verified inputs; transport sampling covariance only, physical dose/material response qualification remains unvalidated).",
            artifact.path
        );
    }
    let mut shown = vec![Quantity::Flux];
    if !report.fluence_limits.is_empty() {
        shown.push(Quantity::Fluence);
    }
    for line in notes_block(&shown, &report.validation).lines() {
        let _ = writeln!(out, "  {line}");
    }
    let _ = writeln!(out, "  Run directory: {}", report.run_directory);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_tool_identities_pass_the_driving_rate_sha256_validation_boundary() {
        // This is the OpenMC C library SHA-256 recorded by the completed tiny CAD history run;
        // the display version (`0.15.3`) is deliberately not the identity.
        let solver_library_sha256 =
            "ba09ae9d19c4c789d8230abe62984de00d2326723b306c440506f099c2e46216";
        let dir = tempfile::tempdir().unwrap();
        let cross_sections = dir.path().join("cross_sections.xml");
        std::fs::write(&cross_sections, b"<cross_sections/>").unwrap();
        let cross_sections_sha256 = sha256_file(&cross_sections).unwrap();
        let (solver, nuclear_data) =
            history_tool_identities("0.15.3", solver_library_sha256, &cross_sections).unwrap();
        assert_eq!(solver.digest, format!("sha256:{solver_library_sha256}"));
        assert_eq!(
            nuclear_data.digest,
            format!("sha256:{cross_sections_sha256}")
        );
        assert!(history_tool_identities("0.15.3", "0.15.3", &cross_sections).is_err());
        let mut rates = crate::fixtures::rates_without_covariance(0.01, 'c');
        rates.solver_digest = solver_library_sha256.into();
        rates.nuclear_data_digest = cross_sections_sha256.clone();
        assert!(
            rates
                .validate()
                .unwrap_err()
                .contains("solver_digest must be a SHA-256 identity")
        );
        rates.solver_digest = solver.digest;
        rates.nuclear_data_digest = nuclear_data.digest;
        // Exercise the same strict SHA-256 identity validator called by the artifact builder.
        rates.validate().unwrap();
    }

    #[test]
    fn cad_operating_context_uses_design_identity_and_verified_linked_assumptions() {
        let design_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scenarios/rm-m/rm-m.faris-design.json");
        let context = resolve_cad_operating_context(&design_path, None).unwrap();
        assert_eq!(context.design_id, "rm-m");
        assert_eq!(context.design_sha256, sha256_file(&design_path).unwrap());
        assert_eq!(context.fusion_power_mw, 525.0);
        assert!(!context.history_assumptions_overridden);
        assert_eq!(
            context.history_assumptions_path.file_name().unwrap(),
            "rm-m.operating-history.json"
        );

        let override_context =
            resolve_cad_operating_context(&design_path, Some(&context.history_assumptions_path))
                .unwrap();
        assert!(override_context.history_assumptions_overridden);
        assert_eq!(override_context.design_sha256, context.design_sha256);
        assert_eq!(
            override_context.history_assumptions_sha256,
            context.history_assumptions_sha256
        );
    }

    #[test]
    fn cad_source_identity_does_not_require_planning_assumptions() {
        let dir = tempfile::tempdir().unwrap();
        let design_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scenarios/rm-m/rm-m.faris-design.json");
        let mut design: serde_json::Value =
            serde_json::from_slice(&read_bounded(&design_path).unwrap()).unwrap();
        design["operating_scenario"] = serde_json::json!({"path": null, "sha256": null});
        let unlinked_path = dir.path().join("unlinked-design.json");
        std::fs::write(&unlinked_path, serde_json::to_vec(&design).unwrap()).unwrap();

        let identity = resolve_cad_source_identity(&unlinked_path).unwrap();
        assert_eq!(identity.design_id, "rm-m");
        assert_eq!(identity.fusion_power_mw, 525.0);
        assert_eq!(identity.design_sha256, sha256_file(&unlinked_path).unwrap());

        let planning_error = resolve_cad_operating_context(&unlinked_path, None).unwrap_err();
        assert!(planning_error.contains("no linked operating-history assumptions"));

        let assumptions_path = dir.path().join("planning.json");
        let assumptions_fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scenarios/rm-m/rm-m.operating-history.json");
        std::fs::copy(assumptions_fixture, &assumptions_path).unwrap();
        let context =
            resolve_cad_operating_context(&unlinked_path, Some(&assumptions_path)).unwrap();
        assert!(context.history_assumptions_overridden);
        assert_eq!(context.design_sha256, identity.design_sha256);
    }

    #[test]
    fn history_nuclear_data_identity_rejects_same_path_table_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let xml = dir.path().join("cross_sections.xml");
        let table_path = dir.path().join("Li6.h5");
        std::fs::write(&xml, b"<cross_sections/>").unwrap();
        std::fs::write(&table_path, b"table-v1").unwrap();
        let identity = HistoryNuclearDataIdentity {
            schema: "faris-cad-history-neutron-data/v0.1".into(),
            scope:
                "material neutron nuclides; photon transport and thermal-scattering tables disabled"
                    .into(),
            cross_sections_xml_sha256: sha256_file(&xml).unwrap(),
            tables: vec![HistoryNuclearDataTable {
                path: table_path.canonicalize().unwrap().display().to_string(),
                bytes: 8,
                sha256: sha256_file(&table_path).unwrap(),
                library_indices: vec![0],
                nuclides: vec!["Li6".into()],
            }],
        };
        let materials = serde_json::json!([{
            "nuclide_atom_fractions": {"Li6": 1.0}
        }]);
        assert!(validate_history_nuclear_data_identity(&identity, &materials, &xml).is_ok());
        std::fs::write(&table_path, b"table-v2").unwrap();
        let error =
            validate_history_nuclear_data_identity(&identity, &materials, &xml).unwrap_err();
        assert!(error.contains("changed"), "{error}");
    }

    #[test]
    fn history_component_map_is_a_complete_unique_partition_with_blanket_breeder() {
        let active = ["blanket", "first-wall", "coil"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let map = BTreeMap::from([
            ("blanket".into(), vec!["blanket".into()]),
            ("first-wall".into(), vec!["first-wall".into()]),
            ("tf-coil".into(), vec!["coil".into()]),
        ]);
        validate_history_component_partition(&active, &map, "blanket").unwrap();
        let changed = BTreeMap::from([
            ("blanket".into(), vec!["blanket".into()]),
            ("first-wall".into(), vec!["coil".into()]),
            ("tf-coil".into(), vec!["coil".into()]),
        ]);
        assert!(validate_history_component_partition(&active, &changed, "blanket").is_err());
        assert!(validate_history_component_partition(&active, &map, "missing").is_err());
        let incomplete = BTreeMap::from([
            ("blanket".into(), vec!["blanket".into()]),
            ("first-wall".into(), vec!["first-wall".into()]),
        ]);
        assert!(validate_history_component_partition(&active, &incomplete, "blanket").is_err());
    }

    #[test]
    fn history_rate_binding_requires_exact_fusion_power_and_rejects_nonfinite_values() {
        assert!(same_source_power_binding(1.25, 1.25));
        assert!(!same_source_power_binding(1.25, 1.250_000_000_1));
        assert!(!same_source_power_binding(f64::NAN, f64::NAN));
    }

    #[test]
    fn loaded_cad_history_rates_must_match_scenario_hash_id_and_power() {
        let scenario = faris_model::LoadedScenario::from_bytes(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scenarios/arc-inspired/scenario.json"
        )))
        .unwrap();
        let power = scenario.scenario.operating_plan.fusion_power_mw;
        assert!(
            validate_history_scenario_binding(
                &scenario.scenario.id,
                &scenario.source_sha256,
                power,
                &scenario.source_sha256,
                power,
                &scenario,
            )
            .is_ok()
        );
        assert!(
            validate_history_scenario_binding(
                &scenario.scenario.id,
                &"f".repeat(64),
                power,
                &scenario.source_sha256,
                power,
                &scenario,
            )
            .is_err()
        );
        assert!(
            validate_history_scenario_binding(
                &scenario.scenario.id,
                &scenario.source_sha256,
                power + 1.0,
                &scenario.source_sha256,
                power + 1.0,
                &scenario,
            )
            .is_err()
        );
        assert!(
            validate_history_scenario_binding(
                "other-scenario",
                &scenario.source_sha256,
                power,
                &scenario.source_sha256,
                power,
                &scenario,
            )
            .is_err()
        );
    }

    use crate::design::life::SECONDS_PER_YEAR;

    // Verifies: SEC-001
    #[test]
    fn a_report_cannot_point_the_model_at_a_relative_climbing_or_linked_path() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().to_str().unwrap();
        assert_eq!(converted_model(good).unwrap(), dir.path().join("model.h5m"));
        for hostile in [
            "",
            "relative/dir",
            "-rf",
            "$(touch /tmp/x)",
            "/tmp/../etc",
            "/tmp/a\0b",
        ] {
            let error = converted_model(hostile).unwrap_err().to_string();
            assert!(error.contains("--work-dir"), "{hostile:?}: {error}");
        }
        #[cfg(unix)]
        {
            let elsewhere = dir.path().join("elsewhere.h5m");
            std::fs::write(&elsewhere, b"x").unwrap();
            std::os::unix::fs::symlink(&elsewhere, dir.path().join("model.h5m")).unwrap();
            assert!(
                converted_model(good)
                    .unwrap_err()
                    .to_string()
                    .contains("a link")
            );
        }
    }

    #[test]
    fn flux_conversion_has_the_right_units() {
        // 1 cm of track per source neutron in 1 m³ at 1e6 n/s: 1e6 cm/s in 1e6 cm³ = 1 /cm²/s.
        assert!((flux_n_m2_s(1.0, 1.0e6, 1.0) - 1.0e4).abs() < 1.0e-9);
    }

    fn design_with_groups(limits: &[(&str, &str, Option<f64>)]) -> Design {
        let groups: Vec<serde_json::Value> = limits
            .iter()
            .map(|(id, metric, threshold)| {
                serde_json::json!({
                    "id": id, "label": id,
                    "limits": [{
                        "metric": metric, "energy_threshold_ev": threshold, "limit": 3.0e22,
                        "averaging": {"kind": "group_average"},
                        "provenance": {"label": "authored", "note": "test"},
                    }],
                    "replacement_duration": {"kind": "computed"},
                })
            })
            .collect();
        let text = include_str!("../../../../scenarios/rm-m/rm-m.faris-design.json");
        let mut value: serde_json::Value = serde_json::from_str(text).unwrap();
        value["replacement_groups"] = serde_json::Value::Array(groups);
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn other_sources_are_refused_not_replaced() {
        let mut design = design_with_groups(&[("coils", "fast_neutron_fluence", Some(1.0e5))]);
        assert!(refuse_unsupported_source(&design).is_ok());
        design.plasma.fuel = Some([("D".to_string(), 1.0)].into_iter().collect());
        assert!(
            refuse_unsupported_source(&design)
                .unwrap_err()
                .to_string()
                .contains("D-T")
        );
        design.plasma.fuel = Some(
            [("D".to_string(), 0.5), ("T".to_string(), 0.5)]
                .into_iter()
                .collect(),
        );
        design.plasma.emissivity_table = Some(design::EmissivityTable {
            path: "table.csv".into(),
            sha256: "0".repeat(64),
        });
        assert!(
            refuse_unsupported_source(&design)
                .unwrap_err()
                .to_string()
                .contains("emissivity")
        );
    }

    #[test]
    fn the_only_fluence_limit_names_the_response() {
        let design = design_with_groups(&[("coils", "fast_neutron_fluence", Some(1.0e5))]);
        let chosen = choose_response(&design, None).unwrap();
        assert_eq!(chosen.group_id.as_deref(), Some("coils"));
        assert_eq!(chosen.energy_min_ev, 1.0e5);
    }

    #[test]
    fn a_peak_limit_asks_for_pieces_of_its_voxel() {
        let mut design = design_with_groups(&[("coils", "fast_neutron_fluence", Some(1.0e5))]);
        assert_eq!(choose_response(&design, None).unwrap().peak_voxel_m, None);
        design.replacement_groups[0].limits[0].averaging = design::Averaging::PeakMesh {
            voxel_m: vec![Some(0.05), Some(0.10), None],
        };
        let chosen = choose_response(&design, None).unwrap();
        assert_eq!(chosen.peak_voxel_m, Some([Some(0.05), Some(0.10)]));
        assert!(!chosen.peak_ignores_dphi);
        design.replacement_groups[0].limits[0].averaging = design::Averaging::PeakMesh {
            voxel_m: vec![Some(0.05), None, Some(0.2)],
        };
        let chosen = choose_response(&design, None).unwrap();
        assert_eq!(chosen.peak_voxel_m, Some([Some(0.05), None]));
        assert!(chosen.peak_ignores_dphi);
    }

    #[test]
    fn a_total_fluence_limit_scores_all_energies() {
        let design = design_with_groups(&[("vessel", "neutron_fluence", None)]);
        assert_eq!(choose_response(&design, None).unwrap().energy_min_ev, 0.0);
    }

    #[test]
    fn several_limits_need_a_named_group() {
        let design = design_with_groups(&[
            ("coils", "fast_neutron_fluence", Some(1.0e5)),
            ("vessel", "neutron_fluence", None),
        ]);
        let error = choose_response(&design, None).unwrap_err().to_string();
        assert!(
            error.contains("--group") && error.contains("coils, vessel"),
            "{error}"
        );
        let chosen = choose_response(&design, Some("vessel")).unwrap();
        assert_eq!(chosen.group_id.as_deref(), Some("vessel"));
        assert!(choose_response(&design, Some("missing")).is_err());
    }

    #[test]
    fn a_design_without_fluence_limits_falls_back_to_the_tf_magnets() {
        let design = design_with_groups(&[("coils", "helium_appm", None)]);
        let chosen = choose_response(&design, None).unwrap();
        assert_eq!(chosen.group_id, None);
        assert_eq!(chosen.energy_min_ev, FAST_NEUTRON_ENERGY_MIN_EV);
    }

    // Verifies: NUC-008
    #[test]
    fn the_cache_key_follows_the_library_and_the_window_interpreter() {
        let request = serde_json::json!({"solids": [1], "materials": [2], "chamber_solid_id": "p",
            "response": {"solid_ids": ["a"], "energy_min_ev": 1.0e5}});
        let key = |library: &str, python: Option<&str>| {
            transport_cache_key(&request, "h", library, python.map(Path::new))
                .digest()
                .to_string()
        };
        let reference = key("lib1", Some("/py/a"));
        assert_eq!(reference, key("lib1", Some("/py/a")));
        // A changed library hash or interpreter never finds the old entry.
        assert_ne!(reference, key("lib2", Some("/py/a")));
        assert_ne!(reference, key("lib1", Some("/py/b")));
        assert_ne!(reference, key("lib1", None));
    }

    #[test]
    fn the_cache_key_follows_the_model_and_the_response() {
        let request = serde_json::json!({"solids": [1], "materials": [2], "chamber_solid_id": "p",
            "response": {"solid_ids": ["a"], "energy_min_ev": 1.0e5}, "sampling": {"seed": 1}});
        let mut other_seed = request.clone();
        other_seed["sampling"]["seed"] = 2.into();
        let mut other_response = request.clone();
        other_response["response"]["energy_min_ev"] = 0.0.into();
        let key = |r: &serde_json::Value, h: &str| {
            transport_cache_key(r, h, "x", None).digest().to_string()
        };
        assert_eq!(key(&request, "h"), key(&other_seed, "h"));
        assert_ne!(key(&request, "h"), key(&other_response, "h"));
        assert_ne!(key(&request, "h"), key(&request, "h2"));
    }

    #[test]
    fn progress_lines_are_recognised_and_other_output_is_ignored() {
        let line = r#"{"progress": {"batches": 12, "histories": 240000, "relative_error": 0.21, "elapsed_seconds": 31.2}}"#;
        assert_eq!(
            parse_progress(line),
            Some(TransportProgress::Sampling {
                histories: 240_000,
                relative_error: Some(0.21),
                peak_relative_error: None,
                elapsed_seconds: 31.2
            })
        );
        let first = r#"{"progress": {"batches": 1, "histories": 20000, "relative_error": null, "elapsed_seconds": 6.8}}"#;
        assert!(matches!(
            parse_progress(first),
            Some(TransportProgress::Sampling {
                relative_error: None,
                ..
            })
        ));
        assert_eq!(parse_progress(" Simulating batch 3"), None);
        assert_eq!(parse_progress("{\"progress\": broken"), None);
    }

    #[test]
    fn source_rate_follows_the_fusion_power() {
        // 525 MW at 17.6 MeV per reaction.
        let rate = source_rate_per_s(525.0);
        assert!((rate / 1.8618e20 - 1.0).abs() < 1.0e-3, "{rate:e}");
    }

    // Verifies: MAG-004
    #[test]
    fn a_fluence_margin_is_shown_with_its_band_and_judged_on_the_unfavourable_edge() {
        let history = PowerHistory {
            calendar_seconds: 1.0,
            full_power_seconds: 1.0,
        };
        // Mean fluence 8 against a limit of 10 looks WITHIN, but a 25 % error puts the upper
        // edge (12) over the limit.
        let a = assess_fluence(8.0, Some(0.25), 10.0, &history);
        assert_eq!(a.status, LimitStatus::Inconclusive);
        let text = format!(
            "{} | {}",
            fluence_band(&a),
            years_band(
                a.full_power_years_to_limit_earliest,
                a.full_power_years_to_limit_latest
            )
        );
        assert!(text.contains("2-sigma band 4.000e0 to 1.200e1"), "{text}");
        assert!(text.contains("unfavourable edge"), "{text}");
        assert_eq!(span_text(1.04), "1.0 years");
        assert_eq!(span_text(0.002), "1 days");
        // With no error estimate the display says so rather than showing a bare margin.
        let bare = assess_fluence(8.0, None, 10.0, &history);
        assert!(fluence_band(&bare).contains("no error band"));
        assert!(years_band(None, None).contains("no error band"));
    }

    #[test]
    fn the_leading_piece_is_the_one_the_run_tracks_not_the_noisiest() {
        let solid = |id: &str, flux: f64, error: f64| SolidFlux {
            solid_id: id.into(),
            volume_m3: 1.0,
            flux_n_m2_s: flux,
            relative_error: Some(0.01),
            peak: Some(PeakFlux {
                r_m: [1.0, 1.1],
                z_m: [0.0, 0.1],
                volume_m3: 0.004,
                flux_n_m2_s: flux,
                relative_error: Some(error),
                pieces_ranked: 5,
            }),
        };
        // A piece with a higher flux but a 60 % error is not resolved above the steadier one.
        let solids = [solid("noisy", 10.0, 0.6), solid("steady", 8.0, 0.05)];
        assert_eq!(leading_peak(&solids).unwrap().0.solid_id, "steady");
        // With nothing resolved above zero, the highest flux leads.
        let solids = [solid("noisy", 10.0, 0.6), solid("noisier", 8.0, 0.7)];
        assert_eq!(leading_peak(&solids).unwrap().0.solid_id, "noisy");
        assert!(leading_peak(&[]).is_none());
    }

    fn sample_report() -> DesignTransportReport {
        let history = PowerHistory {
            calendar_seconds: 20.0 * SECONDS_PER_YEAR,
            full_power_seconds: 8.0 * SECONDS_PER_YEAR,
        };
        let solid = |id: &str, flux: f64| SolidFlux {
            solid_id: id.into(),
            volume_m3: 2.0,
            flux_n_m2_s: flux,
            relative_error: Some(0.012),
            peak: Some(PeakFlux {
                r_m: [1.0, 1.1],
                z_m: [0.0, 0.1],
                volume_m3: 0.004,
                flux_n_m2_s: 2.0 * flux,
                relative_error: Some(0.04),
                pieces_ranked: 12,
            }),
        };
        DesignTransportReport {
            schema_version: DESIGN_TRANSPORT_VERSION.into(),
            design: DesignIdentity {
                path: "rm-m.faris-design.json".into(),
                sha256: "0".repeat(64),
                id: Some("rm-m".into()),
            },
            check_report_sha256: "1".repeat(64),
            h5m_sha256: "2".repeat(64),
            cross_sections_sha256: "3".repeat(64),
            openmc: "0.15.3".into(),
            run_directory: "runs/t1".into(),
            energy_min_ev: 1.0e5,
            response_group_id: Some("coils".into()),
            source_rate_per_s: 1.8618e20,
            energy_per_reaction_ev: D_T_ENERGY_PER_REACTION_EV,
            fusion_power_mw: 525.0,
            histories: 2_000_000,
            elapsed_seconds: 310.0,
            seed: 7,
            threads: 4,
            target_relative_error: 0.05,
            target_reached: true,
            stop_reason: "target".into(),
            why: None,
            next_step: None,
            mean_flux_n_m2_s: 3.1e13,
            mean_relative_error: Some(0.012),
            variance_of_variance: Some(0.02),
            total_volume_m3: 4.0,
            solids: vec![solid("tf-1", 3.0e13), solid("tf-2", 3.2e13)],
            variance_reduction: VarianceReduction {
                mode: "analog".into(),
                reason: "no window interpreter".into(),
                next_step: "give --window-python".into(),
                targets: vec![],
                windows_sha256: None,
                aim_seconds: 0.0,
                aim_reused: false,
            },
            setup_seconds: 0.0,
            windows_reused: false,
            reused_from: None,
            fluence_limits: vec![FluenceLimit {
                group_id: "coils".into(),
                basis: "peak_piece".into(),
                solid_id: Some("tf-2".into()),
                assessment: assess_fluence(6.4e13, Some(0.04), 3.0e22, &history),
            }],
            lost_particles: Some(0),
            lost_positions_m: vec![],
            notices: vec![],
            not_evaluated: vec![],
            history_rates_artifact: None,
            validation: ValidationMarks::for_catalogue(DESIGN_TRANSPORT_OUTPUTS),
        }
    }

    // Verifies: GEO-025
    #[test]
    fn worker_lost_positions_keep_birth_point_provenance_through_assembly_and_cli_rendering() {
        let design = design_with_groups(&[("coils", "fast_neutron_fluence", Some(1.0e5))]);
        let chosen = choose_response(&design, None).unwrap();
        let response = [("tf-1", 1.0)];
        let worker = WorkerResult {
            schema: RESULT_SCHEMA.into(),
            openmc: "0.15.3".into(),
            h5m_sha256: "2".repeat(64),
            run_h5m_sha256: "2".repeat(64),
            model_xml_sha256: "3".repeat(64),
            model_xml_path: "/run/solver/model.xml".into(),
            openmc_library_sha256: "4".repeat(64),
            openmc_library_path: "/run/libopenmc.so".into(),
            source_kernel_path: "/run/openmc_plasma_source.py".into(),
            source_kernel_sha256: "5".repeat(64),
            seed: 7,
            threads: 1,
            particles_per_batch: 100,
            batches: 10,
            histories: 1_000,
            elapsed_seconds: 1.0,
            stop_reason: "lost_particles".into(),
            target_relative_error: 0.05,
            target_reached: false,
            variance_of_variance: None,
            estimator: "tracklength".into(),
            unit: "cm_per_source".into(),
            energy_min_ev: chosen.energy_min_ev,
            total: WorkerValue {
                mean: 1.0,
                standard_error: Some(0.1),
            },
            solids: vec![WorkerSolid {
                solid_id: "tf-1".into(),
                mean: 1.0,
                standard_error: Some(0.1),
                peak: None,
            }],
            variance_reduction: VarianceReduction {
                mode: "analog".into(),
                reason: "test run".into(),
                next_step: "none".into(),
                targets: Vec::new(),
                windows_sha256: None,
                aim_seconds: 0.0,
                aim_reused: false,
            },
            why: Some("lost-particle limit exceeded".into()),
            next_step: Some("repair the geometry".into()),
            lost_particles: Some(1),
            lost_positions_cm: vec![[12.5, -25.0, 50.0]],
            history_response: None,
        };
        let report = assemble(
            worker,
            Inputs {
                design_identity: DesignIdentity {
                    path: "test.faris-design.json".into(),
                    sha256: "0".repeat(64),
                    id: Some("test".into()),
                },
                check_report_sha256: "1".repeat(64),
                h5m_sha256: "2".repeat(64),
                cross_sections_sha256: "4".repeat(64),
                audit_cross_sections_sha256: None,
                run_directory: "test-run".into(),
                fusion_power_mw: 500.0,
                response: &response,
                history_rates_artifact: None,
                design: &design,
                history: Err("test history unavailable".into()),
                setup_seconds: 0.0,
                windows_reused: false,
                cache_note: None,
                reused_from: None,
                chosen: &chosen,
            },
        )
        .unwrap();
        assert_eq!(report.lost_positions_m, vec![[0.125, -0.25, 0.5]]);
        assert!(report.notices.iter().any(|notice| {
            notice.contains("birth points") && notice.contains("does not report where they left")
        }));
        assert!(
            report.fluence_limits.is_empty(),
            "lost particles invalidate scores"
        );
        let text = render_transport_text(&report);
        assert!(text.contains("Lost neutrons: 1 of 1000"), "{text}");
        assert!(text.contains("birth points"), "{text}");
        assert!(!text.contains("lost at ("), "{text}");
    }

    // Verifies: VAL-054
    #[test]
    fn every_number_of_the_transport_report_is_classed_and_every_calculated_one_has_a_state() {
        let report = sample_report();
        let json = serde_json::to_value(&report).unwrap();
        let loose = faris_model::validation::unclassified_numbers(&json, DESIGN_TRANSPORT_OUTPUTS);
        assert!(
            loose.is_empty(),
            "add these to DESIGN_TRANSPORT_OUTPUTS as calculated (with a quantity) or not: {loose:?}"
        );
        for output in DESIGN_TRANSPORT_OUTPUTS {
            if let faris_model::validation::Class::Calculated(q) = output.class {
                assert_eq!(
                    report.validation.0[output.name].quantity, q,
                    "{}",
                    output.name
                );
            }
        }
        // A number added without a class is found.
        let mut extra = json.clone();
        extra["new_number"] = 1.0.into();
        assert_eq!(
            faris_model::validation::unclassified_numbers(&extra, DESIGN_TRANSPORT_OUTPUTS),
            vec!["$.new_number"]
        );
    }

    // Verifies: VAL-054
    #[test]
    fn a_report_without_marks_still_reads_and_shows_the_current_state() {
        let mut json = serde_json::to_value(sample_report()).unwrap();
        json.as_object_mut().unwrap().remove("validation");
        let old: DesignTransportReport = serde_json::from_value(json).unwrap();
        assert!(old.validation.is_empty());
        let text = render_transport_text(&old);
        assert!(text.contains("[not validated: flux]"), "{text}");
    }

    // Verifies: SRC-002
    #[test]
    fn cli_source_summary_uses_neutron_energy_and_labels_the_source_convention() {
        let text = render_transport_text(&sample_report());
        assert!(text.contains("525.0 MW fusion"), "{text}");
        assert!(
            text.contains("Nominal D-T neutron power: 420.6 MW (14.1/17.6 MeV = 80.1%"),
            "{text}"
        );
        assert!(text.contains("not a transport heating tally or energy-closure result"));
    }

    // Verifies: VAL-054
    #[test]
    fn the_text_tags_each_calculated_number_and_explains_each_quantity_once() {
        let text = render_transport_text(&sample_report());
        for line in text.lines().filter(|l| {
            l.contains("Mean over")
                || l.contains("Highest solid")
                || l.contains("Highest piece")
                || l.contains("Over the")
                || l.contains("Reaches the limit")
        }) {
            assert!(line.contains("[not validated: "), "{line}");
        }
        assert_eq!(text.matches("[not validated: flux]").count(), 3);
        assert_eq!(text.matches("[not validated: fluence]").count(), 2);
        assert_eq!(
            text.matches("Validation status, once per quantity:")
                .count(),
            1
        );
        assert_eq!(
            text.matches("neutron flux: not validated. Why: ").count(),
            1
        );
        assert_eq!(text.matches("fluence: not validated. Why: ").count(), 1);
        assert!(text.contains(" Next: "), "{text}");
    }

    // Verifies: SRC-001
    #[test]
    fn design_source_rate_matches_the_reactor_path_per_megawatt() {
        assert_eq!(format!("{:.3e}", source_rate_per_s(1.0)), "3.546e17");
        assert_eq!(
            source_rate_per_s(525.0),
            crate::transport::reaction_rate_per_s(525.0, 17.6e6)
        );
    }
}
