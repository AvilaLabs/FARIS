//! Audited cold-data transport execution. Rust owns identity checks, requested
//! responses and normalization; Python only prepares and runs OpenMC.

use crate::{
    DemoManifest, build_manifest,
    jobs::{Cancellation, ExecutionStatus, JobResult, JobSpec, ResourceLimits, run_job},
    transport::{
        DomainVolume, MAX_TRANSPORT_ARTIFACT_BYTES, NormalizedEnergySpectrum, NormalizedTally,
        NormalizedTransportResult, PhysicalUnit, RawTally, RawTallyUnit, RawTransportSpectra,
        TallyEstimator, ToolIdentity, TransportArtifact, VolumeUnit, normalize_spectra,
        normalize_transport_artifact,
    },
};
use faris_model::math;
use faris_model::{
    LoadedScenario,
    physics::{
        MaterialRecipe, NuclearDataCapability, NuclearDataFile, NuclearDataSelection, PhysicsCase,
        ScientificScope,
    },
    transport::{
        DEFAULT_PORT_SECTOR_HALF_WIDTH_RAD, FAST_NEUTRON_ENERGY_MIN_EV, HeatingConvention,
        HeatingParticleScope, ProducedParticle, ResponseDefinition, ResponseDomain,
        ScoreDefinition, TRANSPORT_ARTIFACT_VERSION, TRANSPORT_REQUEST_LEGACY_VERSION,
        TRANSPORT_REQUEST_VERSION, ToroidalRegion, TransportRequest,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

mod windows;
use windows::Prepared;
pub use windows::{VarianceReductionRecord, WINDOW_CACHE_NAME, WeightWindowsPlan, WindowOptions};

pub type ReactorError = Box<dyn std::error::Error + Send + Sync>;
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_JSON_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FieldMesh {
    pub id: String,
    pub dimensions: [usize; 3],
    pub lower_left_m: [f64; 3],
    pub upper_right_m: [f64; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshPreset {
    Coarse,
    OutboardLocalCoarse,
    OutboardLocal,
    OutboardPortWindow,
}

impl MeshPreset {
    pub fn from_cli(value: &str) -> Result<Self, ReactorError> {
        match value {
            "coarse" => Ok(Self::Coarse),
            "outboard-local-coarse" => Ok(Self::OutboardLocalCoarse),
            "outboard-local" => Ok(Self::OutboardLocal),
            "outboard-port-window" => Ok(Self::OutboardPortWindow),
            _ => Err("mesh preset must be coarse, outboard-local-coarse, outboard-local, or outboard-port-window".into()),
        }
    }
}

impl FieldMesh {
    pub fn bins(&self) -> usize {
        self.dimensions.iter().product()
    }
    pub fn validate(&self, manifest: &DemoManifest) -> Result<(), ReactorError> {
        let count = self
            .dimensions
            .iter()
            .try_fold(1usize, |n, d| n.checked_mul(*d))
            .ok_or("mesh bin count overflows")?;
        if self.id.trim().is_empty()
            || self.dimensions.contains(&0)
            || count > 32_768
            || (0..3).any(|a| {
                !self.lower_left_m[a].is_finite()
                    || !self.upper_right_m[a].is_finite()
                    || self.lower_left_m[a] >= self.upper_right_m[a]
            })
        {
            return Err("mesh dimensions/bounds are invalid or exceed the 32,768-bin cap".into());
        }
        let extent = manifest.major_radius_m
            + manifest
                .variants
                .iter()
                .flat_map(|v| &v.components)
                .map(|c| c.outer_minor_radius_m)
                .fold(0.0_f64, f64::max);
        let outer = manifest
            .variants
            .iter()
            .flat_map(|v| &v.components)
            .map(|c| c.outer_minor_radius_m)
            .fold(0.0_f64, f64::max);
        if self.lower_left_m[0] < -extent
            || self.upper_right_m[0] > extent
            || self.lower_left_m[2] < -extent
            || self.upper_right_m[2] > extent
            || self.lower_left_m[1] < -outer
            || self.upper_right_m[1] > outer
        {
            return Err("mesh bounds exceed the model's rectangular torus envelope".into());
        }
        Ok(())
    }

    pub fn for_preset(manifest: &DemoManifest, preset: MeshPreset) -> Result<Self, ReactorError> {
        let outer = manifest
            .variants
            .iter()
            .flat_map(|v| &v.components)
            .map(|c| c.outer_minor_radius_m)
            .fold(0.0_f64, f64::max);
        let extent = manifest.major_radius_m + outer;
        let mesh = match preset {
            MeshPreset::Coarse => Self {
                id: "neutron-flux".into(),
                dimensions: [12, 8, 12],
                lower_left_m: [-extent, -outer, -extent],
                upper_right_m: [extent, outer, extent],
            },
            MeshPreset::OutboardLocalCoarse => Self {
                id: "neutron-flux-outboard-local-coarse".into(),
                dimensions: [12, 6, 12],
                lower_left_m: [manifest.major_radius_m + 0.7, -0.45, -0.45],
                upper_right_m: [extent, 0.45, 0.45],
            },
            MeshPreset::OutboardLocal => Self {
                id: "neutron-flux-outboard-local".into(),
                dimensions: [24, 12, 24],
                lower_left_m: [manifest.major_radius_m + 0.7, -0.45, -0.45],
                upper_right_m: [extent, 0.45, 0.45],
            },
            MeshPreset::OutboardPortWindow => Self {
                id: "neutron-flux-outboard-port-window".into(),
                dimensions: [1, 1, 1],
                lower_left_m: [manifest.major_radius_m + 1.04, -0.15, -0.15],
                upper_right_m: [extent, 0.15, 0.15],
            },
        };
        mesh.validate(manifest)?;
        Ok(mesh)
    }
    pub fn bin_volume_m3(&self) -> f64 {
        (0..3)
            .map(|axis| {
                (self.upper_right_m[axis] - self.lower_left_m[axis]) / self.dimensions[axis] as f64
            })
            .product()
    }

    /// OpenMC regular mesh convention: x varies fastest, then y, then z.
    pub fn bin_center_m(&self, bin: usize) -> Option<[f64; 3]> {
        if bin >= self.bins() {
            return None;
        }
        let indices = [
            bin % self.dimensions[0],
            (bin / self.dimensions[0]) % self.dimensions[1],
            bin / (self.dimensions[0] * self.dimensions[1]),
        ];
        Some(std::array::from_fn(|axis| {
            self.lower_left_m[axis]
                + (indices[axis] as f64 + 0.5)
                    * (self.upper_right_m[axis] - self.lower_left_m[axis])
                    / self.dimensions[axis] as f64
        }))
    }
}

fn mesh_payload_preflight(
    request: &TransportRequest,
    manifest: &DemoManifest,
    input_json_bytes: usize,
) -> Result<MeshPreflight, ReactorError> {
    mesh_payload_preflight_with_encoding(request, manifest, input_json_bytes, true, true)
}

fn mesh_payload_preflight_legacy_compact(
    request: &TransportRequest,
    manifest: &DemoManifest,
    input_json_bytes: usize,
) -> Result<MeshPreflight, ReactorError> {
    mesh_payload_preflight_with_encoding(request, manifest, input_json_bytes, false, false)
}

fn mesh_payload_preflight_with_encoding(
    request: &TransportRequest,
    manifest: &DemoManifest,
    input_json_bytes: usize,
    pretty_json: bool,
    spectra_reserves_request_copy: bool,
) -> Result<MeshPreflight, ReactorError> {
    let request_bytes = if pretty_json {
        serde_json::to_vec_pretty(request)?.len()
    } else {
        serde_json::to_vec(request)?.len()
    };
    if request_bytes > MAX_JSON_BYTES as usize {
        return Err("transport request exceeds 16 MiB JSON limit".into());
    }
    let mut volumes = Vec::new();
    let mut volume_domains = BTreeSet::new();
    let mut tallies = Vec::with_capacity(request.responses.len());
    let max = f64::MAX;
    for response in &request.responses {
        let domain_key = match &response.domain {
            ResponseDomain::WholeModel => (0_u8, String::new(), 0_u64),
            ResponseDomain::Component { component_id } => (1_u8, component_id.clone(), 0_u64),
            ResponseDomain::Mesh { mesh_id, bin } => (2_u8, mesh_id.clone(), *bin),
            // The region is part of the volume identity; its serialization
            // is unique per distinct region and width.
            ResponseDomain::ComponentRegion {
                component_id,
                region,
            } => (
                3_u8,
                format!("{component_id}/{}", serde_json::to_string(region)?),
                0_u64,
            ),
        };
        if volume_domains.insert(domain_key) {
            volumes.push(DomainVolume {
                domain: response.domain.clone(),
                value: max,
                standard_error: max,
                unit: VolumeUnit::CubicCentimetre,
            });
        }
        let (estimator, unit) = match &response.score {
            ScoreDefinition::Flux | ScoreDefinition::FluxAbove { .. } => {
                (TallyEstimator::Tracklength, RawTallyUnit::CmPerSource)
            }
            ScoreDefinition::ReactionRate { .. } => {
                (TallyEstimator::Tracklength, RawTallyUnit::EventsPerSource)
            }
            ScoreDefinition::ParticleProduction { .. } => (
                TallyEstimator::Tracklength,
                RawTallyUnit::ParticlesPerSource,
            ),
            ScoreDefinition::Heating { .. } => {
                (TallyEstimator::Collision, RawTallyUnit::EvPerSource)
            }
        };
        tallies.push(RawTally {
            response_id: response.id.clone(),
            estimator,
            unit,
            mean: max,
            standard_error: max,
        });
    }
    let artifact = TransportArtifact {
        schema_version: TRANSPORT_ARTIFACT_VERSION.into(),
        request: request.clone(),
        solver: ToolIdentity {
            name: "OpenMC".into(),
            version: "0.15.3".into(),
            digest: format!("sha256:{}", "0".repeat(64)),
        },
        nuclear_data: ToolIdentity {
            name: "bounded preflight".into(),
            version: "local".into(),
            digest: format!("sha256:{}", "0".repeat(64)),
        },
        histories: PREFLIGHT_HISTORIES,
        volumes,
        tallies,
        // Not counted in the pinned preflight sizes: the matrix adds only
        // n * n numbers for the n scalar responses.
        response_covariance: None,
    };
    let raw_bound = if pretty_json {
        serde_json::to_vec_pretty(&artifact)?.len()
    } else {
        serde_json::to_vec(&artifact)?.len()
    };
    if raw_bound > MAX_TRANSPORT_ARTIFACT_BYTES {
        return Err("requested tallies exceed the 16 MiB raw-artifact budget".into());
    }

    let variant = manifest
        .variants
        .iter()
        .find(|v| v.id == request.variant_id)
        .ok_or("mesh preflight variant missing")?;
    let mut normalized_results = Vec::with_capacity(request.responses.len());
    for response in &request.responses {
        let (unit, integrated_unit) = match &response.score {
            ScoreDefinition::Flux | ScoreDefinition::FluxAbove { .. } => (
                PhysicalUnit::NeutronsPerSquareMetreSecond,
                PhysicalUnit::NeutronMetresPerSecond,
            ),
            ScoreDefinition::ReactionRate { .. } => (
                PhysicalUnit::ReactionsPerCubicMetreSecond,
                PhysicalUnit::ReactionsPerSecond,
            ),
            ScoreDefinition::ParticleProduction { .. } => (
                PhysicalUnit::ParticlesPerCubicMetreSecond,
                PhysicalUnit::ParticlesPerSecond,
            ),
            ScoreDefinition::Heating { .. } => {
                (PhysicalUnit::WattsPerCubicMetre, PhysicalUnit::Watts)
            }
        };
        let domain_volume = match &response.domain {
            ResponseDomain::Mesh { .. } => 1.0,
            ResponseDomain::WholeModel => 1.0,
            ResponseDomain::Component { component_id }
            | ResponseDomain::ComponentRegion { component_id, .. } => variant
                .components
                .iter()
                .find(|c| &c.id == component_id)
                .map(|c| c.full_torus_volume_m3)
                .unwrap_or(1.0),
        };
        normalized_results.push(NormalizedTally {
            response_id: response.id.clone(),
            domain: response.domain.clone(),
            score: response.score.clone(),
            estimator: match &response.score {
                ScoreDefinition::Heating { .. } => TallyEstimator::Collision,
                _ => TallyEstimator::Tracklength,
            },
            mean: max,
            standard_error: max,
            unit,
            integrated_mean: max,
            integrated_standard_error: max,
            integrated_unit,
            volume_m3: domain_volume,
            volume_standard_error_m3: max,
        });
    }
    let normalized = NormalizedTransportResult {
        response_covariance: None,
        schema_version: "faris-normalized-transport/v0.1".into(),
        scenario_id: request.scenario_id.clone(),
        scenario_sha256: request.scenario_sha256.clone(),
        variant_id: request.variant_id.clone(),
        source: request.source.clone(),
        solver: artifact.solver.clone(),
        nuclear_data: artifact.nuclear_data.clone(),
        histories: artifact.histories,
        source_reaction_rate_per_s: max,
        source_neutron_rate_per_s: max,
        results: normalized_results,
    };
    let serialized_run = if pretty_json {
        serde_json::to_vec_pretty(&normalized)?.len()
    } else {
        serde_json::to_vec(&normalized)?.len()
    };
    let run_bound = serialized_run.saturating_add(64 * 1024);
    if run_bound > MAX_JSON_BYTES as usize {
        return Err("normalized run record exceeds the 16 MiB JSON budget".into());
    }
    let spectrum_bound = (if spectra_reserves_request_copy {
        request_bytes
    } else {
        0
    })
    .saturating_add(
        request
            .responses
            .iter()
            .filter(|r| {
                matches!(&r.score, ScoreDefinition::Flux)
                    && matches!(&r.domain, ResponseDomain::Component { .. })
            })
            .count()
            .saturating_mul(
                if request
                    .responses
                    .iter()
                    .any(|r| matches!(r.score, ScoreDefinition::Heating { .. }))
                {
                    2
                } else {
                    1
                },
            )
            .saturating_mul(2 * (11 * 48 + 1024)),
    );
    let package_bound = input_json_bytes
        .saturating_add(raw_bound)
        .saturating_add(run_bound)
        .saturating_add(spectrum_bound)
        .saturating_add(4 * 1024 * 1024);
    if package_bound > 32 * 1024 * 1024 {
        return Err("mesh request exceeds the 32 MiB evidence-package content budget".into());
    }
    Ok(MeshPreflight {
        serialization_method: if pretty_json {
            "pretty-json-v2".into()
        } else {
            "compact-json-v1".into()
        },
        request_json_bytes: request_bytes,
        raw_artifact_upper_bound_bytes: raw_bound,
        run_json_upper_bound_bytes: run_bound,
        package_content_upper_bound_bytes: package_bound,
    })
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SamplingPlan {
    /// Batches to run; the budget (most batches) when a target error is set.
    pub batches: u32,
    pub particles_per_batch: u32,
    pub seed: u64,
    pub threads: u32,
    /// Stop once the relative standard error of the magnet fast flux
    /// ([`STOP_RULE_RESPONSE_ID`]) reaches this, after at least
    /// [`STOP_RULE_MIN_BATCHES`] batches, or when `batches` is used up.
    /// Absent means a fixed run of exactly `batches`, as records made before
    /// the stop rule existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_relative_error: Option<f64>,
    /// The responses a target-error run watches, all of which must reach the target. Empty means the magnet
    /// fast flux alone. The analog pass of a windowed Production run watches the whole-model responses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub watch_responses: Vec<String>,
}
impl Default for SamplingPlan {
    fn default() -> Self {
        Self {
            batches: 100,
            particles_per_batch: 10_000,
            seed: 123_456_789,
            threads: 1,
            target_relative_error: None,
            watch_responses: Vec::new(),
        }
    }
}
/// Response the stop rule watches: the panel's headline magnet fast flux.
pub const STOP_RULE_RESPONSE_ID: &str = "magnets-fast-flux";
/// Fewest batches a target-error run may stop at; a relative error estimated
/// from fewer batches is not trustworthy.
pub const STOP_RULE_MIN_BATCHES: u32 = 30;
/// Seeds of the two weight-window steps (`window_generator.py`). A run may not use either, so it never shares a
/// random stream with the histories that made its windows (NUC-053).
pub const RESERVED_WINDOW_SEEDS: [u64; 2] = [811_500_991, 811_500_993];
/// Largest target relative error a plan may ask for.
pub const STOP_RULE_MAX_TARGET: f64 = 0.5;

/// Why a run stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// A fixed run did the batches it was asked for.
    FixedCount,
    TargetMet,
    /// The error reached the target, but the batches disagree (variance of the variance above
    /// [`VOV_LIMIT`]), so the error is not believed; only possible when the batch budget is used up.
    TargetUnsettled,
    BudgetExhausted,
}

/// A target is not accepted while the variance of the variance of the batch values is above this (MCNP's check).
/// The same limit as `batch_statistics.VOV_LIMIT`, which the worker applies; the worker's receipt carries it.
pub const VOV_LIMIT: f64 = 0.1;

/// What a run actually sampled, read from the hash-bound worker receipt.
#[derive(Clone, Debug, PartialEq)]
pub struct SamplingOutcome {
    pub batches_realized: u32,
    pub histories_realized: u64,
    pub target_relative_error: Option<f64>,
    /// Relative standard error of the watched response, when a target was set. With several watched responses,
    /// that of the least precise.
    pub achieved_relative_error: Option<f64>,
    /// The watched response that limited the run (the least precise one).
    pub limiting_response: Option<String>,
    /// Variance of the variance of the batch values of the watched responses (the worst), when known.
    pub variance_of_variance: Option<f64>,
    pub stop_reason: StopReason,
}

impl SamplingOutcome {
    /// What the limiting response is called in a sentence.
    fn limiting_label(&self) -> String {
        match self.limiting_response.as_deref() {
            None | Some(STOP_RULE_RESPONSE_ID) => "magnet fast flux".into(),
            Some(id) => format!("the least precise watched response ({id})"),
        }
    }

    /// One plain sentence for the result view.
    pub fn summary(&self) -> String {
        let histories = self.histories_realized;
        let what = self.limiting_label();
        match (
            self.stop_reason,
            self.target_relative_error,
            self.achieved_relative_error,
        ) {
            (StopReason::TargetMet, Some(target), Some(achieved)) => format!(
                "Stopped after {histories} histories because the target was met: {what} {:.1}% (target {:.1}%).",
                achieved * 100.0,
                target * 100.0
            ),
            (StopReason::TargetUnsettled, Some(target), Some(achieved)) => format!(
                "Target not trusted: {what} reached {:.1}% (target {:.1}%) after {histories} histories, but the batches do not agree (variance of the variance {:.2}, above {VOV_LIMIT}), so the error itself is not reliable. Run longer.",
                achieved * 100.0,
                target * 100.0,
                self.variance_of_variance.unwrap_or(f64::NAN)
            ),
            (StopReason::BudgetExhausted, Some(target), Some(achieved)) => format!(
                "Target not met: {what} {:.1}% after {histories} histories (target {:.1}%). The run used up its budget; run longer or use more threads.",
                achieved * 100.0,
                target * 100.0
            ),
            (StopReason::BudgetExhausted, Some(target), None) => format!(
                "Target not met: {what} scored nothing in {histories} histories (target {:.1}%). Run longer.",
                target * 100.0
            ),
            _ => format!("Ran the requested {histories} histories."),
        }
    }
}

/// Histories a successful run must have recorded: the fixed count, or the
/// realized count of a target-error run.
fn expected_histories(
    plan: &SamplingPlan,
    outcome: Option<&SamplingOutcome>,
) -> Result<u64, ReactorError> {
    let per_batch = u64::from(plan.particles_per_batch);
    match (plan.target_relative_error, outcome) {
        (None, _) => Ok(u64::from(plan.batches) * per_batch),
        (Some(_), Some(outcome)) => Ok(outcome.histories_realized),
        (Some(_), None) => {
            Err("target-error run has no sampling outcome in its worker receipt".into())
        }
    }
}

/// Read and check the worker's `sampling_outcome` against the plan. Receipts
/// from before the stop rule have none, which is only valid for a fixed plan.
pub fn sampling_outcome(
    worker: &Value,
    plan: &SamplingPlan,
) -> Result<Option<SamplingOutcome>, ReactorError> {
    let Some(raw) = worker.get("sampling_outcome") else {
        if plan.target_relative_error.is_some() {
            return Err("target-error run has no sampling outcome in its worker receipt".into());
        }
        return Ok(None);
    };
    let bad = |what: &str| -> ReactorError { format!("sampling outcome {what}").into() };
    let batches_realized = raw
        .get("batches_realized")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| bad("batches are missing"))?;
    let histories_realized = raw
        .get("histories_realized")
        .and_then(Value::as_u64)
        .ok_or_else(|| bad("histories are missing"))?;
    let target = raw.get("target_relative_error").and_then(Value::as_f64);
    let achieved = raw.get("achieved_relative_error").and_then(Value::as_f64);
    // Receipts from before the variance guard have neither field: no guard was applied to them.
    let vov = raw.get("variance_of_variance").and_then(Value::as_f64);
    let limiting_response = raw
        .get("target_response_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(limit) = raw
        .get("variance_of_variance_limit")
        .and_then(Value::as_f64)
        && limit != VOV_LIMIT
    {
        return Err(bad("was judged against another variance-of-variance limit"));
    }
    let reason = match raw.get("stop_reason").and_then(Value::as_str) {
        Some("fixed_count") => StopReason::FixedCount,
        Some("target_met") => StopReason::TargetMet,
        Some("target_unsettled") => StopReason::TargetUnsettled,
        Some("budget_exhausted") => StopReason::BudgetExhausted,
        _ => return Err(bad("stop reason is unknown")),
    };
    if histories_realized != u64::from(batches_realized) * u64::from(plan.particles_per_batch)
        || target != plan.target_relative_error
        || achieved.is_some_and(|v| !v.is_finite() || v < 0.0)
    {
        return Err(bad("disagrees with the sampling plan"));
    }
    let consistent = match plan.target_relative_error {
        None => reason == StopReason::FixedCount && batches_realized == plan.batches,
        Some(target) => {
            (STOP_RULE_MIN_BATCHES..=plan.batches).contains(&batches_realized)
                && match reason {
                    StopReason::TargetMet => {
                        achieved.is_some_and(|v| v <= target * (1.0 + 1e-9))
                            && vov.is_none_or(|v| v <= VOV_LIMIT)
                    }
                    StopReason::TargetUnsettled => {
                        batches_realized == plan.batches
                            && achieved.is_some_and(|v| v <= target * (1.0 + 1e-9))
                            && vov.is_some_and(|v| v > VOV_LIMIT)
                    }
                    StopReason::BudgetExhausted => {
                        batches_realized == plan.batches
                            && achieved.is_none_or(|v| v > target * (1.0 + 1e-9))
                    }
                    StopReason::FixedCount => false,
                }
        }
    };
    if !consistent {
        return Err(bad(
            "stop reason contradicts the batches and error recorded",
        ));
    }
    Ok(Some(SamplingOutcome {
        batches_realized,
        histories_realized,
        target_relative_error: target,
        achieved_relative_error: achieved,
        limiting_response,
        variance_of_variance: vov,
        stop_reason: reason,
    }))
}
/// Upper bound on histories in one fixed-source run. Wall time is bounded
/// separately by the job deadline; this bound keeps a typo from requesting a
/// run of days. Small regions (a magnet sector behind no port) need tens of
/// millions of histories for a usable relative error without variance reduction.
/// It must keep the digit count of `PREFLIGHT_HISTORIES`, which the recorded
/// preflight sizes of earlier runs were computed with.
pub const MAX_RUN_HISTORIES: u64 = 50_000_000;
const PREFLIGHT_HISTORIES: u64 = 10_000_000;
/// Longest wall time a reactor run may be given. A run at the history cap takes
/// about two hours on a 7-thread laptop, so the bound leaves room for slower
/// machines; a run can always be cancelled sooner.
pub const MAX_RUN_TIMEOUT_SECONDS: u64 = 14_400;

/// Whether a run timeout is within 1 second ..= `MAX_RUN_TIMEOUT_SECONDS`.
pub fn validate_run_timeout(timeout: Duration) -> Result<(), ReactorError> {
    if timeout.is_zero() || timeout > Duration::from_secs(MAX_RUN_TIMEOUT_SECONDS) {
        return Err(format!("reactor timeout must be 1..{MAX_RUN_TIMEOUT_SECONDS} seconds").into());
    }
    Ok(())
}

impl SamplingPlan {
    pub fn validate(&self) -> Result<(), ReactorError> {
        if self.batches < STOP_RULE_MIN_BATCHES {
            return Err(format!(
                "{} batches is too few for a meaningful standard error: the error is estimated from the spread between batches, so at least {STOP_RULE_MIN_BATCHES} are needed. Set the batch count to {STOP_RULE_MIN_BATCHES} or more (at most 1000)",
                self.batches
            )
            .into());
        }
        if self.batches > 1000
            || self.particles_per_batch == 0
            || u64::from(self.batches) * u64::from(self.particles_per_batch) > MAX_RUN_HISTORIES
            || self.seed == 0
            || self.seed > i64::MAX as u64
            || self.threads == 0
            || self.threads > 32
        {
            return Err("sampling requires 30..1000 batches, 1..50M total histories, positive signed-64-bit seed and 1..32 threads".into());
        }
        if RESERVED_WINDOW_SEEDS.contains(&self.seed) {
            return Err("this seed is reserved for the weight-window steps; use another".into());
        }
        if let Some(target) = self.target_relative_error
            && !(target.is_finite() && target > 0.0 && target <= STOP_RULE_MAX_TARGET)
        {
            return Err("target relative error must be above 0 and at most 0.5".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        if !self.watch_responses.is_empty()
            && (self.target_relative_error.is_none()
                || !self
                    .watch_responses
                    .iter()
                    .all(|id| !id.is_empty() && seen.insert(id)))
        {
            return Err(
                "watched responses need a target error and must be distinct response ids".into(),
            );
        }
        Ok(())
    }
}

#[derive(Serialize)]
pub struct ReactorInput {
    pub schema_version: String,
    pub manifest: DemoManifest,
    pub physics: PhysicsCase,
    pub request: TransportRequest,
    pub sampling: SamplingPlan,
    pub mesh: FieldMesh,
    pub cross_sections: PathBuf,
    pub openmc_executable: PathBuf,
    pub nuclear_data_digest: String,
    /// The weight windows the run reads; absent for an analog run, so earlier inputs keep their bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variance_reduction: Option<WeightWindowsPlan>,
}

/// The windowed pass of a two-pass run: where it is, which of its results the record reports, and its hash.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WindowedPass {
    /// Plain directory name inside the run directory; it holds a complete run (input.json, run.json, ...).
    pub directory: String,
    /// Hash of that pass's run.json.
    pub run_sha256: String,
    /// The responses reported from the windowed pass; every other one is reported from the analog pass.
    pub responses: Vec<String>,
}

/// Replace the results of `ids` in the analog pass's normalized result by those of the windowed pass. The
/// passes are independent random streams, so the sampling covariance between a replaced response and any other
/// is zero, and the replaced response's variance is that of its own pass.
fn merge_windowed(
    analog: Option<&NormalizedTransportResult>,
    windowed: Option<&NormalizedTransportResult>,
    ids: &[String],
) -> Result<NormalizedTransportResult, ReactorError> {
    let (Some(analog), Some(windowed)) = (analog, windowed) else {
        return Err("both passes need a normalized result to be merged".into());
    };
    let mut merged = analog.clone();
    for id in ids {
        let taken = windowed
            .results
            .iter()
            .find(|r| &r.response_id == id)
            .ok_or_else(|| format!("the windowed pass has no result for {id}"))?;
        let slot = merged
            .results
            .iter_mut()
            .find(|r| &r.response_id == id)
            .ok_or_else(|| format!("the analog pass has no result for {id}"))?;
        if slot.domain != taken.domain || slot.score != taken.score || slot.unit != taken.unit {
            return Err(format!("the two passes define {id} differently").into());
        }
        *slot = taken.clone();
        if let Some(covariance) = merged.response_covariance.as_mut()
            && let Some(at) = covariance.response_ids.iter().position(|r| r == id)
        {
            let n = covariance.response_ids.len();
            let own = windowed
                .response_covariance
                .as_ref()
                .and_then(|w| {
                    let j = w.response_ids.iter().position(|r| r == id)?;
                    w.integrated.get(j * w.response_ids.len() + j).copied()
                })
                .unwrap_or(math::powi(taken.integrated_standard_error, 2));
            for k in 0..n {
                covariance.integrated[at * n + k] = 0.0;
                covariance.integrated[k * n + at] = 0.0;
            }
            covariance.integrated[at * n + at] = own;
        }
    }
    Ok(merged)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReactorRun {
    pub schema_version: String,
    pub scenario_sha256: String,
    pub variant_id: String,
    pub physics_sha256: String,
    pub input_sha256: String,
    pub adapter_sha256: String,
    pub python_sha256: String,
    pub openmc_sha256: String,
    pub cross_sections_sha256: String,
    pub audit_sha256: String,
    pub scientific_scope: ScientificScope,
    pub sampling: SamplingPlan,
    pub mesh: FieldMesh,
    #[serde(default)]
    pub mesh_preflight: Option<MeshPreflight>,
    pub execution: Option<JobResult>,
    pub import_error: Option<String>,
    pub raw_artifact_sha256: Option<String>,
    pub normalized: Option<NormalizedTransportResult>,
    #[serde(default)]
    pub transport_spectra_sha256: Option<String>,
    /// Exact worker receipt bytes; absent only on historical runs before the
    /// geometry ownership audit was recorded.
    #[serde(default)]
    pub worker_result_sha256: Option<String>,
    #[serde(default)]
    pub normalized_spectra: Option<Vec<NormalizedEnergySpectrum>>,
    /// Recomputed from the SHA-bound worker receipt; intentionally omitted
    /// from run.json so established evidence bytes stay immutable.
    #[serde(skip)]
    pub sampling_precision_summary: Option<SamplingPrecisionSummary>,
    /// Batches and histories actually run, the achieved error and why the run
    /// stopped; recomputed from the same worker receipt, absent on records
    /// made before the stop rule.
    #[serde(skip)]
    pub sampling_outcome: Option<SamplingOutcome>,
    /// Whether the run used weight windows, and if not why not. Present on target-error runs made since the
    /// windows existed; the windows themselves are bound by the input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variance_reduction: Option<VarianceReductionRecord>,
    /// Set when the run is two passes (weight windows): the pass that sampled the responses the windows serve.
    /// Every other response in `normalized` comes from this run's own (analog) pass. Absent on every other run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windowed_pass: Option<WindowedPass>,
    /// The windowed pass as a run of its own, loaded from `windowed_pass`; not part of run.json.
    #[serde(skip)]
    pub windowed_run: Option<Box<ReactorRun>>,
    pub scientific_qualification: String,
    pub notice: String,
}

impl ReactorRun {
    /// Relative standard error of an integrated response, when the run has it and its mean is positive.
    pub fn relative_error(&self, response_id: &str) -> Option<f64> {
        let result = self
            .normalized
            .as_ref()?
            .results
            .iter()
            .find(|r| r.response_id == response_id)?;
        (result.integrated_mean > 0.0)
            .then(|| result.integrated_standard_error / result.integrated_mean)
    }

    /// Which pass a response of this run comes from: `"windowed"` for the responses the weight windows serve,
    /// `"analog"` for the rest. A run without windows is analog throughout.
    pub fn pass_of(&self, response_id: &str) -> &'static str {
        match &self.windowed_pass {
            Some(pass) if pass.responses.iter().any(|r| r == response_id) => "windowed",
            _ => "analog",
        }
    }

    /// One plain sentence naming the pass each kind of response comes from, with the histories each pass took;
    /// `None` for a run of one pass.
    pub fn passes_summary(&self) -> Option<String> {
        let pass = self.windowed_pass.as_ref()?;
        let windowed = self.windowed_run.as_deref()?;
        Some(format!(
            "Two passes. Analog pass ({} histories): every response except {}. Windowed pass ({} histories): {}.",
            self.histories_sampled(),
            pass.responses.join(", "),
            windowed.histories_sampled(),
            pass.responses.join(", ")
        ))
    }

    /// Histories of every pass of the run: its own and those of its windowed pass.
    pub fn histories_total(&self) -> u64 {
        self.histories_sampled()
            + self
                .windowed_run
                .as_deref()
                .map_or(0, ReactorRun::histories_sampled)
    }

    /// Histories the run actually sampled: the realized count of a
    /// target-error run, else the planned batches times particles.
    pub fn histories_sampled(&self) -> u64 {
        self.sampling_outcome.as_ref().map_or_else(
            || u64::from(self.sampling.batches) * u64::from(self.sampling.particles_per_batch),
            |outcome| outcome.histories_realized,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SamplingPrecisionSummary {
    pub plan_id: String,
    pub purpose: String,
    pub estimator: String,
    pub integrated_goal: f64,
    pub local_goal: f64,
    pub check_count: usize,
    pub checks_met: usize,
    pub checks_unmet: usize,
    pub all_goals_met: bool,
    pub whole_model_tbr_relative_standard_error: Option<f64>,
    pub whole_model_heating_relative_standard_error: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MeshPreflight {
    #[serde(default = "legacy_preflight_serialization_method")]
    pub serialization_method: String,
    pub request_json_bytes: usize,
    pub raw_artifact_upper_bound_bytes: usize,
    pub run_json_upper_bound_bytes: usize,
    pub package_content_upper_bound_bytes: usize,
}

fn legacy_preflight_serialization_method() -> String {
    "compact-json-v1".into()
}

#[derive(Clone)]
pub struct ReactorJob<'a> {
    pub scenario: &'a LoadedScenario,
    pub physics: &'a PhysicsCase,
    pub audit: &'a Path,
    pub cross_sections: &'a Path,
    pub python: &'a Path,
    pub openmc: &'a Path,
    pub output: &'a Path,
    pub sampling: SamplingPlan,
    pub mesh: Option<FieldMesh>,
    pub timeout: Duration,
    /// Opt-in extra per-component neutron spectra on a named group structure
    /// (`fispact-709`); `None` keeps the request exactly as before.
    pub activation_spectra: Option<String>,
    /// Embedded worker supplied by the client. Its exact bytes are preserved.
    pub adapter: &'a [u8],
    /// A Python with OpenMC 0.16 for weight windows. Used by target-error runs only; without it they are analog.
    pub windows: Option<WindowOptions<'a>>,
}

pub fn read_json_bytes(path: &Path) -> Result<Vec<u8>, ReactorError> {
    let bytes = crate::vfs::read_capped(path, MAX_JSON_BYTES)?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err("JSON input exceeds 16 MiB".into());
    }
    Ok(bytes)
}

pub fn load_physics_case(
    path: &Path,
    scenario: &LoadedScenario,
) -> Result<PhysicsCase, ReactorError> {
    let physics: PhysicsCase = serde_json::from_slice(&read_json_bytes(path)?)?;
    physics.validate_against(scenario)?;
    Ok(physics)
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
/// The browser build reads recorded files from memory.
#[cfg(target_arch = "wasm32")]
pub fn hash_file(path: &Path) -> Result<String, ReactorError> {
    let bytes = crate::vfs::read_capped(path, MAX_FILE_BYTES)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("identity input must be a regular file below 512 MiB".into());
    }
    Ok(digest(&bytes))
}
#[cfg(not(target_arch = "wasm32"))]
pub fn hash_file(path: &Path) -> Result<String, ReactorError> {
    let metadata = path.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return Err("identity input must be a regular file below 512 MiB".into());
    }
    let mut file = File::open(path)?.take(MAX_FILE_BYTES + 1);
    let mut hasher = Sha256::new();
    let mut total = 0;
    let mut buffer = [0_u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_FILE_BYTES {
            return Err("identity input grew beyond 512 MiB".into());
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn canonicalize_required_input(path: &Path, label: &str) -> Result<PathBuf, ReactorError> {
    path.canonicalize()
        .map_err(|error| format!("cannot locate {label} at {}: {error}", path.display()).into())
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), ReactorError> {
    crate::atomic_file::write_new(path, bytes)?;
    Ok(())
}

/// Bind only the case's required nuclides. The audit assertions are checked
/// against local file hashes; this does not authenticate upstream provenance.
pub fn bind_audited_library(
    physics: &PhysicsCase,
    scenario: &LoadedScenario,
    audit: &Value,
    cross_sections: &Path,
) -> Result<PhysicsCase, ReactorError> {
    physics.validate_against(scenario)?;
    if audit["schema"] != "faris.openmc-library-audit/1.0.0" {
        return Err("unsupported library audit".into());
    }
    let cross_sections = cross_sections.canonicalize()?;
    if audit["cross_sections_xml"]["sha256"].as_str() != Some(hash_file(&cross_sections)?.as_str())
    {
        return Err("cross_sections.xml differs from audited file".into());
    }
    let root = cross_sections
        .parent()
        .ok_or("cross-section file needs a parent")?;
    let needed: BTreeSet<_> = physics
        .materials
        .iter()
        .flat_map(|m| match &m.recipe {
            MaterialRecipe::NuclideMixture { nuclides, .. } => {
                nuclides.iter().map(|n| n.nuclide.clone()).collect()
            }
            MaterialRecipe::Void { .. } => Vec::new(),
        })
        .collect();
    let mut files = Vec::new();
    for nuclide in &needed {
        let entry = &audit["neutron_library"][&nuclide];
        if entry["readable_by_openmc_data_api"] != true
            || entry["nuclide_name_in_hdf5"].as_str() != Some(nuclide.as_str())
        {
            return Err(format!("missing or unreadable audited neutron data for {nuclide}").into());
        }
        if entry["reactions"]["301"]["present"] != true {
            return Err(format!("missing MT=301 heating coefficients for {nuclide}").into());
        }
        let relative_path = entry["relative_path"]
            .as_str()
            .ok_or("audit relative path missing")?;
        let relative = Path::new(relative_path);
        if relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err("data path must be safe and relative".into());
        }
        let path = root.join(relative).canonicalize()?;
        if !path.starts_with(root) {
            return Err("audited file escapes library root".into());
        }
        let size_bytes = entry["size_bytes"]
            .as_u64()
            .ok_or("audit file size missing")?;
        let sha256 = entry["sha256"]
            .as_str()
            .ok_or("audit file hash missing")?
            .to_owned();
        if path.metadata()?.len() != size_bytes || hash_file(&path)? != sha256 {
            return Err(format!("data file differs from audit: {nuclide}").into());
        }
        let temperatures_k = serde_json::from_value(entry["temperatures_k"].clone())?;
        files.push(NuclearDataFile {
            file_id: format!("neutron-{nuclide}"),
            relative_path: relative_path.into(),
            sha256: format!("sha256:{sha256}"),
            size_bytes,
            temperatures_k,
            nuclides: vec![nuclide.clone()],
            capabilities: vec![
                NuclearDataCapability::ContinuousEnergyNeutronTransport,
                NuclearDataCapability::Heating,
            ],
        });
    }
    let mut elements: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for nuclide in &needed {
        let element: String = nuclide
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect();
        if element.is_empty() {
            return Err(format!("cannot determine photon-data element for {nuclide}").into());
        }
        elements.entry(element).or_default().push(nuclide.clone());
    }
    for (element, isotopes) in elements {
        let entry = &audit["photon_atomic_library"][&element];
        if entry["library_entry_present"] != true || entry["readable_by_h5py"] != true {
            return Err(
                format!("missing or unreadable audited photon atomic data for {element}").into(),
            );
        }
        let relative_path = entry["relative_path"]
            .as_str()
            .ok_or("photon audit relative path missing")?;
        let relative = Path::new(relative_path);
        if relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err("photon data path must be safe and relative".into());
        }
        let path = root.join(relative).canonicalize()?;
        if !path.starts_with(root) {
            return Err("photon data path escapes library root".into());
        }
        let size_bytes = entry["size_bytes"]
            .as_u64()
            .ok_or("photon audit file size missing")?;
        let sha256 = entry["sha256"]
            .as_str()
            .ok_or("photon audit file hash missing")?
            .to_owned();
        if path.metadata()?.len() != size_bytes || hash_file(&path)? != sha256 {
            return Err(format!("photon data file differs from audit: {element}").into());
        }
        let mut photon_capabilities = vec![NuclearDataCapability::PhotonTransport];
        if entry["atomic_relaxation_populated"] == true {
            photon_capabilities.push(NuclearDataCapability::AtomicRelaxation);
        }
        files.push(NuclearDataFile {
            file_id: format!("photon-{element}"),
            relative_path: relative_path.into(),
            sha256: format!("sha256:{sha256}"),
            size_bytes,
            temperatures_k: Vec::new(),
            nuclides: isotopes,
            capabilities: photon_capabilities,
        });
    }
    let mut bound = physics.clone();
    bound.nuclear_data = NuclearDataSelection::Inventory {
        id: "local-hybrid-fendl32-endfbvii1".into(),
        name: "Local FENDL-3.2 neutron and ENDF/B-VII.1 photon inventory; provenance per component audit".into(),
        version: "FENDL-3.2 neutron + ENDF/B-VII.1 photon".into(),
        files,
    };
    let diagnostics = bound.readiness_diagnostics(scenario)?;
    if !diagnostics.is_empty() {
        return Err(format!(
            "physics readiness gaps: {}",
            serde_json::to_string(&diagnostics)?
        )
        .into());
    }
    Ok(bound)
}

pub fn request_for_case(
    scenario: &LoadedScenario,
    physics: &PhysicsCase,
) -> Result<(TransportRequest, FieldMesh), ReactorError> {
    let manifest = build_manifest(scenario)?;
    let mesh = FieldMesh::for_preset(&manifest, MeshPreset::Coarse)?;
    request_for_case_with_mesh(scenario, physics, mesh)
}

pub fn request_for_case_with_mesh(
    scenario: &LoadedScenario,
    physics: &PhysicsCase,
    mesh: FieldMesh,
) -> Result<(TransportRequest, FieldMesh), ReactorError> {
    request_for_case_with_regions(scenario, physics, mesh, DEFAULT_PORT_SECTOR_HALF_WIDTH_RAD)
}

/// Component whose fast-flux regions are requested for the magnet service check.
pub const MAGNET_COMPONENT_ID: &str = "magnets";

/// Fast-flux (E > 0.1 MeV) responses for the whole component and its named
/// regions: inboard half, outboard half outside the port sector, and the port
/// sector of the given half width about toroidal angle 0 (the +x axis). The
/// same set is requested with and without a penetration so a port arrangement
/// and its control are directly comparable.
pub fn fast_flux_region_responses(
    component_id: &str,
    port_sector_half_width_rad: f64,
) -> Vec<ResponseDefinition> {
    let fast = ScoreDefinition::FluxAbove {
        energy_min_ev: FAST_NEUTRON_ENERGY_MIN_EV,
    };
    let region = |suffix: &str, region: ToroidalRegion| ResponseDefinition {
        id: format!("{component_id}-{suffix}-fast-flux"),
        domain: ResponseDomain::ComponentRegion {
            component_id: component_id.into(),
            region,
        },
        score: fast.clone(),
    };
    vec![
        ResponseDefinition {
            id: format!("{component_id}-fast-flux"),
            domain: ResponseDomain::Component {
                component_id: component_id.into(),
            },
            score: fast.clone(),
        },
        region("inboard", ToroidalRegion::InboardHalf),
        region(
            "outboard",
            ToroidalRegion::OutboardHalf {
                excluding_sector_half_width_rad: Some(port_sector_half_width_rad),
            },
        ),
        region(
            "port-sector",
            ToroidalRegion::PortSector {
                half_width_rad: port_sector_half_width_rad,
            },
        ),
    ]
}

/// As [`request_for_case_with_mesh`] with a chosen port sector half width.
pub fn request_for_case_with_regions(
    scenario: &LoadedScenario,
    physics: &PhysicsCase,
    mesh: FieldMesh,
    port_sector_half_width_rad: f64,
) -> Result<(TransportRequest, FieldMesh), ReactorError> {
    physics.validate_against(scenario)?;
    let manifest = build_manifest(scenario)?;
    mesh.validate(&manifest)?;
    let variant = manifest
        .variants
        .iter()
        .find(|v| v.id == physics.variant_id)
        .ok_or("unknown variant")?;
    if variant.components.is_empty() {
        return Err("empty variant".into());
    }
    let mut responses = vec![ResponseDefinition {
        id: "total-tritium-production".into(),
        domain: ResponseDomain::WholeModel,
        score: ScoreDefinition::ParticleProduction {
            particle: ProducedParticle::Tritium,
            score: "H3-production".into(),
        },
    }];
    for (suffix, scope) in [
        ("total", HeatingParticleScope::Total),
        ("neutron", HeatingParticleScope::Neutron),
        ("photon", HeatingParticleScope::Photon),
        ("electron", HeatingParticleScope::Electron),
        ("positron", HeatingParticleScope::Positron),
    ] {
        responses.push(ResponseDefinition {
            id: format!("heating-{suffix}-whole-model"),
            domain: ResponseDomain::WholeModel,
            score: ScoreDefinition::Heating {
                convention: HeatingConvention::Heating,
                particle_scope: scope,
            },
        });
    }
    for component in &variant.components {
        let domain = ResponseDomain::Component {
            component_id: component.id.clone(),
        };
        responses.push(ResponseDefinition {
            id: format!("{}-flux", component.id),
            domain: domain.clone(),
            score: ScoreDefinition::Flux,
        });
        if component.id == MAGNET_COMPONENT_ID {
            responses.extend(fast_flux_region_responses(
                &component.id,
                port_sector_half_width_rad,
            ));
        }
        if component.material_id != "void" {
            responses.push(ResponseDefinition {
                id: format!("{}-tritium", component.id),
                domain: domain.clone(),
                score: ScoreDefinition::ParticleProduction {
                    particle: ProducedParticle::Tritium,
                    score: "H3-production".into(),
                },
            });
            for (suffix, scope) in [
                ("total", HeatingParticleScope::Total),
                ("neutron", HeatingParticleScope::Neutron),
                ("photon", HeatingParticleScope::Photon),
                ("electron", HeatingParticleScope::Electron),
                ("positron", HeatingParticleScope::Positron),
            ] {
                responses.push(ResponseDefinition {
                    id: format!("heating-{suffix}-{}", component.id),
                    domain: domain.clone(),
                    score: ScoreDefinition::Heating {
                        convention: HeatingConvention::Heating,
                        particle_scope: scope,
                    },
                });
            }
        }
    }
    for bin in 0..mesh.bins() {
        responses.push(ResponseDefinition {
            id: if mesh.id == "neutron-flux-outboard-port-window" && bin == 0 {
                "outboard-port-window-flux".into()
            } else {
                format!("mesh-flux-{bin}")
            },
            domain: ResponseDomain::Mesh {
                mesh_id: mesh.id.clone(),
                bin: bin as u64,
            },
            score: ScoreDefinition::Flux,
        });
    }
    let request = TransportRequest {
        schema_version: TRANSPORT_REQUEST_VERSION.into(),
        scenario_id: scenario.scenario.id.clone(),
        scenario_sha256: scenario.source_sha256.clone(),
        variant_id: physics.variant_id.clone(),
        fusion_power_mw: scenario.scenario.operating_plan.fusion_power_mw,
        source: physics.source.to_transport_source(),
        responses,
        activation_spectra: None,
    };
    request.validate_against(scenario)?;
    Ok((request, mesh))
}

fn legacy_request_for_case(
    scenario: &LoadedScenario,
    physics: &PhysicsCase,
) -> Result<(TransportRequest, FieldMesh), ReactorError> {
    let (mut request, mesh) = request_for_case(scenario, physics)?;
    request.schema_version = TRANSPORT_REQUEST_LEGACY_VERSION.into();
    request.responses.retain(|response| {
        !matches!(
            response.score,
            ScoreDefinition::Heating { .. } | ScoreDefinition::FluxAbove { .. }
        )
    });
    request.validate_against(scenario)?;
    Ok((request, mesh))
}

fn check_geometric_volumes(
    result: &NormalizedTransportResult,
    scenario: &LoadedScenario,
    mesh: &FieldMesh,
) -> Result<(), ReactorError> {
    let manifest = build_manifest(scenario)?;
    let variant = manifest
        .variants
        .iter()
        .find(|v| v.id == result.variant_id)
        .ok_or("unknown result variant")?;
    let outer = variant
        .components
        .last()
        .ok_or("empty variant")?
        .outer_minor_radius_m;
    let penetration = scenario.scenario.penetration.as_ref();
    let mut port_component_geometry = BTreeMap::new();
    if let Some(faris_model::Penetration::OutboardRectangularPrism { bounds_m, .. }) = penetration {
        for component in &variant.components {
            let intersection = crate::geometry::estimate_torus_shell_box_intersection(
                scenario.scenario.geometry.major_radius_m,
                component.inner_minor_radius_m,
                component.outer_minor_radius_m,
                &bounds_m.minimum_xyz_m,
                &bounds_m.maximum_xyz_m,
            );
            port_component_geometry.insert(
                component.id.as_str(),
                (
                    component.full_torus_volume_m3 - intersection.volume_m3,
                    intersection.refinement_delta_m3,
                ),
            );
        }
    }
    let affected: &[String] = match penetration {
        Some(faris_model::Penetration::OutboardRectangularPrism {
            affected_component_ids,
            ..
        }) => affected_component_ids,
        None => &[],
    };
    for response in &result.results {
        // Expected volume and, for volumes reduced by the port, the numerical
        // refinement difference of the independent port-intersection estimate.
        let (expected, refinement_delta) = match &response.domain {
            ResponseDomain::WholeModel => (
                2.0 * math::powi(std::f64::consts::PI, 2)
                    * manifest.major_radius_m
                    * math::powi(outer, 2),
                None,
            ),
            ResponseDomain::Component { component_id } => {
                let component = variant
                    .components
                    .iter()
                    .find(|c| &c.id == component_id)
                    .ok_or("unknown component volume")?;
                let entry = port_component_geometry.get(component_id.as_str());
                (
                    entry.map_or(component.full_torus_volume_m3, |entry| entry.0),
                    penetration
                        .map(|_| {
                            entry.map(|entry| entry.1).ok_or(
                                "missing port geometry volume estimate for affected component",
                            )
                        })
                        .transpose()?,
                )
            }
            ResponseDomain::ComponentRegion {
                component_id,
                region,
            } => {
                let component = variant
                    .components
                    .iter()
                    .find(|c| &c.id == component_id)
                    .ok_or("unknown component volume")?;
                let full = crate::geometry::torus_shell_region_volume_m3(
                    manifest.major_radius_m,
                    component.inner_minor_radius_m,
                    component.outer_minor_radius_m,
                    region,
                );
                match penetration {
                    Some(faris_model::Penetration::OutboardRectangularPrism {
                        bounds_m, ..
                    }) if affected.contains(component_id) => {
                        let removed =
                            crate::geometry::estimate_torus_shell_box_region_intersection(
                                manifest.major_radius_m,
                                component.inner_minor_radius_m,
                                component.outer_minor_radius_m,
                                region,
                                &bounds_m.minimum_xyz_m,
                                &bounds_m.maximum_xyz_m,
                            )
                            .ok_or("port region intersection needs a prism at positive x")?;
                        (full - removed.volume_m3, Some(removed.refinement_delta_m3))
                    }
                    _ => (full, None),
                }
            }
            ResponseDomain::Mesh { mesh_id, bin }
                if mesh_id == &mesh.id && *bin < mesh.bins() as u64 =>
            {
                (mesh.bin_volume_m3(), None)
            }
            _ => return Err("unknown field volume".into()),
        };
        let tolerance = match refinement_delta {
            Some(delta) => {
                (3.0 * math::hypot(response.volume_standard_error_m3, delta)).max(1.0e-8)
            }
            None => 1.0e-10 * expected,
        };
        if (response.volume_m3 - expected).abs() > tolerance {
            return Err(format!(
                "adapter volume differs from geometry for {}",
                response.response_id
            )
            .into());
        }
    }
    Ok(())
}

/// The batch statistics the worker imports (`batch_statistics.py`, shared with the design transport run), written
/// beside the worker; the worker's receipt carries the hash of the copy it used.
const BATCH_STATISTICS_HELPER: &str =
    include_str!("../../../integrations/openmc/batch_statistics.py");
const BATCH_STATISTICS_FILE: &str = "batch_statistics.py";

fn embedded_worker_sha256() -> String {
    digest(include_bytes!(
        "../../../integrations/openmc/reactor_transport.py"
    ))
}

/// Bind the worker's geometry-only OpenMC Geometry.find probes to the exact
/// input and expected scenario/material assignments. This is a software and
/// geometry control, not experimental or physical validation.
fn validate_geometry_ownership_receipt(
    worker: &Value,
    input_sha256: &str,
    scenario: &LoadedScenario,
    physics: &PhysicsCase,
) -> Result<(), ReactorError> {
    fn audit_str<'a>(obj: &'a Value, key: &str) -> Result<&'a str, ReactorError> {
        obj.get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("geometry ownership audit is missing {key}").into())
    }
    let audit = worker
        .get("geometry_ownership_audit")
        .ok_or("worker receipt is missing geometry_ownership_audit")?;
    let manifest = build_manifest(scenario)?;
    let variant = manifest
        .variants
        .iter()
        .find(|variant| variant.id == physics.variant_id)
        .ok_or("geometry ownership variant is absent from scenario")?;
    if audit_str(audit, "schema_version")? != "faris-openmc-geometry-ownership-audit/v0.1"
        || audit_str(audit, "status")? != "PASS"
        || audit_str(audit, "clearance_status")? != "PASS"
        || audit_str(audit, "scientific_qualification")? != "NOT_EVALUATED"
        || audit_str(audit, "scenario_sha256")? != scenario.source_sha256
        || audit_str(audit, "variant_id")? != physics.variant_id
        || audit_str(audit, "input_sha256")? != input_sha256
        || audit
            .get("checks_are_geometry_only")
            .and_then(Value::as_bool)
            != Some(true)
        || audit.get("probe_count").and_then(Value::as_u64) != Some(117)
        || audit.get("failed_probe_count").and_then(Value::as_u64) != Some(0)
        || audit.get("plasma_radius_m").and_then(Value::as_f64)
            != Some(manifest.plasma_minor_radius_m)
        || audit
            .get("declared_plasma_to_first_wall_clearance_m")
            .and_then(Value::as_f64)
            .is_none_or(|measured| {
                (measured - scenario.scenario.geometry.plasma_to_first_wall_gap_m).abs()
                    > 4.0
                        * f64::EPSILON
                        * scenario
                            .scenario
                            .geometry
                            .plasma_to_first_wall_gap_m
                            .abs()
                            .max(1.0)
            })
        || audit
            .get("first_wall_inner_radius_m")
            .and_then(Value::as_f64)
            != variant
                .components
                .first()
                .map(|component| component.inner_minor_radius_m)
    {
        return Err("worker geometry ownership audit identity/status/geometry differs".into());
    }

    let mut expected_ids = BTreeMap::<String, (String, Option<String>)>::new();
    let toroidal = ["1.57079633", "3.14159265", "4.71238898"];
    for phi in toroidal {
        expected_ids.insert(
            format!("plasma-interior-phi-{phi}"),
            ("plasma-source-domain".into(), None),
        );
        for label in ["near-plasma", "near-first-wall"] {
            expected_ids.insert(
                format!("clearance-{label}-phi-{phi}"),
                ("plasma-first-wall-clearance".into(), None),
            );
        }
    }
    let assignments: BTreeMap<_, _> = physics
        .component_assignments
        .iter()
        .map(|assignment| {
            (
                assignment.component_id.as_str(),
                assignment.material_id.as_str(),
            )
        })
        .collect();
    for component in &variant.components {
        let material_id = assignments
            .get(component.id.as_str())
            .ok_or("geometry ownership component has no material assignment")?;
        let expected_material = if matches!(
            physics
                .materials
                .iter()
                .find(|material| material.id == **material_id),
            Some(faris_model::physics::MaterialDefinition {
                recipe: MaterialRecipe::Void { .. },
                ..
            })
        ) {
            None
        } else {
            Some((**material_id).to_owned())
        };
        for phi in toroidal {
            for theta in ["0.00000000", "1.57079633", "3.14159265", "4.71238898"] {
                expected_ids.insert(
                    format!("{}-mid-phi-{phi}-theta-{theta}", component.id),
                    (component.id.clone(), expected_material.clone()),
                );
            }
            for edge in ["near-inner", "near-outer"] {
                expected_ids.insert(
                    format!("{}-{edge}-phi-{phi}", component.id),
                    (component.id.clone(), expected_material.clone()),
                );
            }
        }
    }
    let probes = audit
        .get("probes")
        .and_then(Value::as_array)
        .ok_or("geometry ownership audit probes are missing")?;
    if probes.len() != expected_ids.len() {
        return Err("geometry ownership audit probe count differs from required coverage".into());
    }
    let mut observed_ids = BTreeSet::new();
    for probe in probes {
        let probe_id = audit_str(probe, "probe_id")?;
        let (cell, material) = expected_ids
            .get(probe_id)
            .ok_or("geometry ownership audit contains an unexpected probe")?;
        let expected_material_id = material.as_ref().map_or("void", |id| id.as_str());
        let openmc_material_name = material.as_ref().map(|id| id.as_str());
        if !observed_ids.insert(probe_id)
            || audit_str(probe, "status")? != "PASS"
            || audit_str(probe, "expected_cell_name")? != cell
            || audit_str(probe, "observed_cell_name")? != cell
            || audit_str(probe, "expected_material_id")? != expected_material_id
            || probe
                .get("expected_openmc_material_name")
                .and_then(Value::as_str)
                != openmc_material_name
            || probe
                .get("observed_openmc_material_name")
                .and_then(Value::as_str)
                != openmc_material_name
            || probe.get("expected_cell_id").and_then(Value::as_u64)
                != probe.get("observed_cell_id").and_then(Value::as_u64)
            || probe
                .get("expected_openmc_material_id")
                .and_then(Value::as_u64)
                != probe
                    .get("observed_openmc_material_id")
                    .and_then(Value::as_u64)
        {
            return Err(format!("geometry ownership probe {probe_id} failed binding").into());
        }
    }
    if observed_ids.len() != expected_ids.len() {
        return Err("geometry ownership audit is missing required probes".into());
    }
    Ok(())
}

/// Extract only the bounded summary shown to the UI. The full per-bin check
/// list stays in the SHA-bound worker artifact; no new run.json fields are
/// written and these exploratory RSE goals never qualify physical accuracy.
fn sampling_precision_summary(worker: &Value) -> Result<SamplingPrecisionSummary, ReactorError> {
    let precision = worker
        .get("sampling_precision")
        .ok_or("worker receipt is missing sampling_precision")?;
    let text = |key: &str| -> Result<String, ReactorError> {
        let value = precision
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("sampling precision is missing {key}"))?;
        if value.len() > 512 {
            return Err(format!("sampling precision {key} exceeds its size bound").into());
        }
        Ok(value)
    };
    let integrated_goal = precision
        .get("integrated_goal")
        .and_then(Value::as_f64)
        .ok_or("sampling precision integrated goal is missing")?;
    let local_goal = precision
        .get("local_goal")
        .and_then(Value::as_f64)
        .ok_or("sampling precision local goal is missing")?;
    if text("plan_id")? != "faris-exploratory-precision-goals/v0.1"
        || integrated_goal != 0.05
        || local_goal != 0.1
    {
        return Err("sampling precision plan identity or goals differ".into());
    }
    let checks = precision
        .get("checks")
        .and_then(Value::as_array)
        .ok_or("sampling precision checks are missing")?;
    if checks.is_empty() || checks.len() > 32_832 {
        return Err("sampling precision check count is outside bounds".into());
    }
    let mut checks_met = 0;
    let mut tbr_rse = None;
    let mut heating_rse = None;
    for check in checks {
        let target = check
            .get("target_relative_standard_error")
            .and_then(Value::as_f64)
            .ok_or("sampling precision target is missing")?;
        let met = check
            .get("met")
            .and_then(Value::as_bool)
            .ok_or("sampling precision check state is missing")?;
        let observed = check
            .get("observed_relative_standard_error")
            .filter(|value| !value.is_null())
            .and_then(Value::as_f64);
        if target != integrated_goal && target != local_goal {
            return Err("sampling precision check uses an undeclared target".into());
        }
        if observed.is_some_and(|value| !value.is_finite() || value < 0.0)
            || met != observed.is_some_and(|value| value <= target)
        {
            return Err("sampling precision check state disagrees with its RSE".into());
        }
        checks_met += usize::from(met);
        match (
            check.get("response_id").and_then(Value::as_str),
            check.get("quantity").and_then(Value::as_str),
        ) {
            (Some("total-tritium-production"), Some("whole-model tritium production"))
                if target != integrated_goal =>
            {
                return Err("whole-model tritium precision check uses the wrong target".into());
            }
            (Some("total-tritium-production"), Some("whole-model tritium production"))
                if tbr_rse.replace(observed).is_some() =>
            {
                return Err("whole-model tritium precision check is ambiguous".into());
            }
            (Some("total-tritium-production"), Some("whole-model tritium production")) => {
                tbr_rse = Some(observed);
            }
            (Some("heating-total-whole-model"), Some("whole-model deposited heating"))
                if target != integrated_goal =>
            {
                return Err("whole-model heating precision check uses the wrong target".into());
            }
            (Some("heating-total-whole-model"), Some("whole-model deposited heating"))
                if heating_rse.replace(observed).is_some() =>
            {
                return Err("whole-model heating precision check is ambiguous".into());
            }
            (Some("heating-total-whole-model"), Some("whole-model deposited heating")) => {
                heating_rse = Some(observed);
            }
            _ => {}
        }
    }
    let declared_all = precision
        .get("all_goals_met")
        .and_then(Value::as_bool)
        .ok_or("sampling precision aggregate state is missing")?;
    let checks_unmet = checks.len() - checks_met;
    if declared_all != (checks_unmet == 0) || tbr_rse.is_none() || heating_rse.is_none() {
        return Err("sampling precision aggregate or whole-model checks are inconsistent".into());
    }
    Ok(SamplingPrecisionSummary {
        plan_id: text("plan_id")?,
        purpose: text("purpose")?,
        estimator: text("estimator")?,
        integrated_goal,
        local_goal,
        check_count: checks.len(),
        checks_met,
        checks_unmet,
        all_goals_met: declared_all,
        whole_model_tbr_relative_standard_error: tbr_rse.flatten(),
        whole_model_heating_relative_standard_error: heating_rse.flatten(),
    })
}

/// The precision goals of a two-pass run, each judged on the pass its response is reported from: the checks of
/// `ids` come from the windowed pass's receipt, every other check from the analog pass's. The aggregate is
/// recomputed from the checks taken.
fn merged_precision_summary(
    analog_worker: &Value,
    windowed_worker: &Value,
    ids: &[String],
) -> Result<SamplingPrecisionSummary, ReactorError> {
    let checks_of = |worker: &Value| -> Result<Vec<Value>, ReactorError> {
        worker["sampling_precision"]["checks"]
            .as_array()
            .cloned()
            .ok_or_else(|| "worker receipt is missing its precision checks".into())
    };
    let in_ids = |check: &Value| {
        check["response_id"]
            .as_str()
            .is_some_and(|id| ids.iter().any(|i| i == id))
    };
    let mut checks: Vec<Value> = checks_of(analog_worker)?
        .into_iter()
        .filter(|check| !in_ids(check))
        .collect();
    let taken: Vec<Value> = checks_of(windowed_worker)?
        .into_iter()
        .filter(|check| in_ids(check))
        .collect();
    if taken.is_empty() {
        return Err("the windowed pass has no precision check for its response".into());
    }
    checks.extend(taken);
    let mut merged = analog_worker.clone();
    merged["sampling_precision"]["all_goals_met"] = checks
        .iter()
        .all(|check| check["met"].as_bool() == Some(true))
        .into();
    merged["sampling_precision"]["checks"] = checks.into();
    sampling_precision_summary(&merged)
}

fn read_worker_result(run_directory: &Path) -> Result<Value, ReactorError> {
    Ok(serde_json::from_slice(&read_json_bytes(
        &run_directory.join("solver/worker-result.json"),
    )?)?)
}

/// What a pass needs before its input is written: the bound model, request, mesh and the identities of the tools.
struct PassInput {
    input: ReactorInput,
    audit_bytes: Vec<u8>,
    physics: PhysicsCase,
    request: TransportRequest,
    mesh: FieldMesh,
    python: PathBuf,
    openmc: PathBuf,
    python_sha256: String,
    openmc_sha256: String,
    cross_sections_sha256: String,
}

fn build_pass(job: &ReactorJob<'_>, sampling: &SamplingPlan) -> Result<PassInput, ReactorError> {
    sampling.validate()?;
    validate_run_timeout(job.timeout)?;
    let python = canonicalize_required_input(job.python, "Python interpreter")?;
    let openmc = canonicalize_required_input(job.openmc, "OpenMC executable")?;
    let python_sha256 = hash_file(&python)?;
    let openmc_sha256 = hash_file(&openmc)?;
    let cross_sections = canonicalize_required_input(job.cross_sections, "cross_sections.xml")?;
    let cross_sections_sha256 = hash_file(&cross_sections)?;
    let audit_bytes = read_json_bytes(job.audit).map_err(|error| {
        format!(
            "cannot read OpenMC library audit JSON at {}: {error}",
            job.audit.display()
        )
    })?;
    let physics = bind_audited_library(
        job.physics,
        job.scenario,
        &serde_json::from_slice(&audit_bytes)?,
        &cross_sections,
    )?;
    let default_mesh = FieldMesh::for_preset(&build_manifest(job.scenario)?, MeshPreset::Coarse)?;
    let requested_mesh = job.mesh.clone().unwrap_or(default_mesh);
    let (mut request, mesh) = request_for_case_with_mesh(job.scenario, &physics, requested_mesh)?;
    if job.activation_spectra.is_some() {
        request.activation_spectra = job.activation_spectra.clone();
        request.validate_against(job.scenario)?;
    }
    let nuclear_data_digest = format!(
        "sha256:{}",
        digest(&serde_json::to_vec(&physics.nuclear_data)?)
    );
    let input = ReactorInput {
        schema_version: "faris-openmc-input/v0.1".into(),
        manifest: build_manifest(job.scenario)?,
        physics: physics.clone(),
        request: request.clone(),
        sampling: sampling.clone(),
        mesh: mesh.clone(),
        cross_sections,
        openmc_executable: openmc.clone(),
        nuclear_data_digest,
        variance_reduction: None,
    };
    Ok(PassInput {
        input,
        audit_bytes,
        physics,
        request,
        mesh,
        python,
        openmc,
        python_sha256,
        openmc_sha256,
        cross_sections_sha256,
    })
}

/// Run one pass in the directory `output`: write its input and files, run the worker, import and check what it
/// produced. The record is returned unwritten (`write_run_record`), so a caller can still complete it.
fn execute_pass(
    job: &ReactorJob<'_>,
    sampling: &SamplingPlan,
    output: &Path,
    built: PassInput,
    prepared: Option<Prepared>,
    cancellation: &Cancellation,
) -> Result<ReactorRun, ReactorError> {
    let PassInput {
        mut input,
        audit_bytes,
        physics,
        request,
        mesh,
        python,
        openmc,
        python_sha256,
        openmc_sha256,
        cross_sections_sha256,
    } = built;
    input.variance_reduction = prepared.as_ref().and_then(|p| p.plan.clone());
    let variance_reduction = prepared.as_ref().map(|p| p.record.clone());
    let input_bytes = serde_json::to_vec_pretty(&input)?;
    if input_bytes.len() > MAX_JSON_BYTES as usize {
        return Err("serialized reactor input exceeds 16 MiB".into());
    }
    let mesh_preflight = match mesh_payload_preflight(&request, &input.manifest, input_bytes.len())
    {
        Ok(preflight) => preflight,
        Err(error) => {
            if let Some(prepared) = &prepared {
                prepared.discard();
            }
            return Err(error);
        }
    };
    let output = std::path::absolute(output)?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir(&output)?;
    if let Some(prepared) = prepared {
        prepared.install(&output)?;
    }
    write_new(&output.join("input.json"), &input_bytes)?;
    write_new(&output.join("scenario.json"), job.scenario.source_bytes())?;
    write_new(&output.join("audit.json"), &audit_bytes)?;
    let worker = output.join("reactor_transport.py");
    write_new(&worker, job.adapter)?;
    write_new(
        &output.join(BATCH_STATISTICS_FILE),
        BATCH_STATISTICS_HELPER.as_bytes(),
    )?;
    let mut environment = vec![
        (
            OsString::from("OMP_NUM_THREADS"),
            sampling.threads.to_string().into(),
        ),
        ("OPENBLAS_NUM_THREADS".into(), "1".into()),
        ("PYTHONDONTWRITEBYTECODE".into(), "1".into()),
    ];
    let path = std::env::join_paths([
        python.parent().ok_or("Python has no parent")?.to_path_buf(),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ])?;
    environment.push(("PATH".into(), path));
    if let Some(home) = std::env::var_os("HOME") {
        environment.push(("HOME".into(), home));
    }
    let spec = JobSpec {
        program: python,
        arguments: vec![
            worker.into_os_string(),
            "--input".into(),
            output.join("input.json").into_os_string(),
            "--output-dir".into(),
            output.join("solver").into_os_string(),
        ],
        working_directory: output.clone(),
        environment,
        timeout: job.timeout,
        capture_limit_bytes: 4 * 1024 * 1024,
        artifact_roots: vec![],
        resource_limits: ResourceLimits::default(),
    };
    let mut record=ReactorRun{schema_version:"faris-reactor-run/v0.1".into(),scenario_sha256:job.scenario.source_sha256.clone(),variant_id:physics.variant_id.clone(),physics_sha256:digest(&serde_json::to_vec(&physics)?),input_sha256:digest(&input_bytes),adapter_sha256:digest(job.adapter),python_sha256,openmc_sha256,cross_sections_sha256,audit_sha256:digest(&audit_bytes),scientific_scope:physics.scientific_scope.clone(),sampling:sampling.clone(),mesh:mesh.clone(),mesh_preflight:Some(mesh_preflight.clone()),execution:None,import_error:None,raw_artifact_sha256:None,normalized:None,transport_spectra_sha256:None,worker_result_sha256:None,normalized_spectra:None,sampling_precision_summary:None,sampling_outcome:None,variance_reduction,windowed_pass:None,windowed_run:None,scientific_qualification:"NOT_EVALUATED".into(),notice:"Coupled cold-data numerical surrogate with unqualified material/data/source applicability. Monte Carlo standard errors describe sampling only. Heating is transport energy deposition under OpenMC's local electron treatment; it is not a component thermal model, actual coil lifetime or operating-reactor prediction.".into()};
    match run_job(&spec, cancellation) {
        Ok(execution) => record.execution = Some(execution),
        Err(error) => {
            record.import_error = Some(format!("external execution unavailable: {error}"))
        }
    }
    if record
        .execution
        .as_ref()
        .is_some_and(|e| e.execution_status == ExecutionStatus::Succeeded)
    {
        let import = (|| -> Result<_, ReactorError> {
            let worker_bytes = read_json_bytes(&output.join("solver/worker-result.json"))?;
            let worker: Value = serde_json::from_slice(&worker_bytes)?;
            validate_geometry_ownership_receipt(
                &worker,
                &record.input_sha256,
                job.scenario,
                &physics,
            )?;
            let precision_summary = sampling_precision_summary(&worker)?;
            let outcome = sampling_outcome(&worker, sampling)?;
            let raw_bytes = read_json_bytes(&output.join("solver/transport-artifact.json"))?;
            let artifact = TransportArtifact::from_bytes(&raw_bytes)?;
            if artifact.histories != expected_histories(sampling, outcome.as_ref())?
                || artifact.solver.digest != format!("sha256:{}", record.openmc_sha256)
                || artifact.nuclear_data.digest != input.nuclear_data_digest
            {
                return Err("solver, data or histories differ from the requested execution".into());
            }
            // Recheck local executable identity after execution; altered files
            // invalidate binding. This is a local receipt, not signed attestation.
            if hash_file(&openmc)? != record.openmc_sha256
                || hash_file(&spec.program)? != record.python_sha256
            {
                return Err("executable changed during execution".into());
            }
            let normalized = normalize_transport_artifact(&request, &artifact, job.scenario)?;
            let spectra_bytes = read_json_bytes(&output.join("solver/transport-spectra.json"))?;
            let spectra: RawTransportSpectra = serde_json::from_slice(&spectra_bytes)?;
            let normalized_spectra = normalize_spectra(
                &spectra,
                &artifact,
                &normalized,
                &request,
                &digest(&input_bytes),
            )?;
            check_geometric_volumes(&normalized, job.scenario, &record.mesh)?;
            Ok((
                digest(&raw_bytes),
                digest(&worker_bytes),
                precision_summary,
                outcome,
                normalized,
                digest(&spectra_bytes),
                normalized_spectra,
            ))
        })();
        match import {
            Ok((
                digest,
                worker_digest,
                precision_summary,
                outcome,
                normalized,
                spectra_digest,
                normalized_spectra,
            )) => {
                record.raw_artifact_sha256 = Some(digest);
                record.worker_result_sha256 = Some(worker_digest);
                record.sampling_precision_summary = Some(precision_summary);
                record.sampling_outcome = outcome;
                record.normalized = Some(normalized);
                record.transport_spectra_sha256 = Some(spectra_digest);
                record.normalized_spectra = Some(normalized_spectra);
            }
            Err(error) => record.import_error = Some(error.to_string()),
        }
    }
    Ok(record)
}

pub fn run_reactor(
    job: &ReactorJob<'_>,
    cancellation: &Cancellation,
) -> Result<ReactorRun, ReactorError> {
    let built = build_pass(job, &job.sampling)?;
    let prepared = windows::prepare(
        job,
        &serde_json::to_value(&built.input)?,
        &built.cross_sections_sha256,
        cancellation,
    )?;
    let output = std::path::absolute(job.output)?;
    // Windows serve one response, so a windowed run is two passes (see `run_two_passes`); without windows, or
    // for a fixed-count Preview, it is one.
    if prepared.as_ref().is_some_and(|p| p.plan.is_some()) {
        let prepared = prepared.unwrap_or_else(|| unreachable!("checked above"));
        return run_two_passes(job, built, prepared, &output, cancellation);
    }
    let record = execute_pass(job, &job.sampling, &output, built, prepared, cancellation)?;
    write_run_record(&output, &record)?;
    Ok(record)
}

fn write_run_record(output: &Path, record: &ReactorRun) -> Result<(), ReactorError> {
    write_new(
        &output.join("run.json"),
        &serde_json::to_vec_pretty(record)?,
    )?;
    Ok(())
}

/// Directory of the windowed pass, inside the run directory of the analog pass.
pub const WINDOWED_PASS_DIRECTORY: &str = "windowed-pass";

/// The response ids the analog pass of a windowed run watches: every whole-model response (tritium and the
/// heating family), which an analog run of modest size resolves far below the target. The responses deep in the
/// blanket stack (the coil layer, the vessel) are not watched: an analog pass cannot reach the target on them,
/// and they are reported with the error the pass reached.
pub fn analog_watch(request: &TransportRequest) -> Vec<String> {
    request
        .responses
        .iter()
        .filter(|r| matches!(r.domain, faris_model::transport::ResponseDomain::WholeModel))
        .map(|r| r.id.clone())
        .collect()
}

/// Largest batch the analog pass of a windowed run uses, and its budget in batches; it stops at the 30th batch
/// when its watched responses are at the target by then.
const ANALOG_PASS_PARTICLES: u32 = 10_000;
const ANALOG_PASS_BATCHES: u32 = 100;

/// Sampling of the analog pass of a windowed Production run: the same target error, watching the whole-model
/// responses, in small batches. Its seed differs from the windowed pass's, so the passes are independent.
fn analog_pass_sampling(windowed: &SamplingPlan, watch: Vec<String>) -> SamplingPlan {
    SamplingPlan {
        batches: ANALOG_PASS_BATCHES,
        particles_per_batch: ANALOG_PASS_PARTICLES,
        seed: other_seed(windowed.seed),
        threads: windowed.threads,
        target_relative_error: windowed.target_relative_error,
        watch_responses: watch,
    }
}

/// A seed for the other pass: next to this one, and not a reserved one.
fn other_seed(seed: u64) -> u64 {
    let mut other = seed.wrapping_add(7_919) % (i64::MAX as u64);
    while other == 0 || other == seed || RESERVED_WINDOW_SEEDS.contains(&other) {
        other = other.wrapping_add(1) % (i64::MAX as u64);
    }
    other
}

impl<'a> ReactorJob<'a> {
    fn for_pass(&self, output: &'a Path, sampling: SamplingPlan) -> ReactorJob<'a> {
        ReactorJob {
            output,
            sampling,
            ..self.clone()
        }
    }
}

/// A Production run with weight windows. The windows are aimed at the magnet fast flux and are not shown to leave
/// the other responses unbiased at this precision (the demo's total tritium was far noisier with them), so each
/// response is taken from the pass that is valid for it:
///
/// - the analog pass (this run directory) samples the whole model until the whole-model responses are at the
///   target; every response but the magnet fast flux is reported from it;
/// - the windowed pass (`windowed-pass/`, a complete run of its own) samples the magnet fast flux with the
///   windows; that response is reported from it.
///
/// The record says which; loading it checks both passes and the merge.
fn run_two_passes(
    job: &ReactorJob<'_>,
    built: PassInput,
    prepared: Prepared,
    output: &Path,
    cancellation: &Cancellation,
) -> Result<ReactorRun, ReactorError> {
    let variance = prepared.record.clone();
    let watch = analog_watch(&built.request);
    if watch.is_empty() {
        prepared.discard();
        return Err("the request has no whole-model response for the analog pass to watch".into());
    }
    let analog_sampling = analog_pass_sampling(&job.sampling, watch);
    let analog_built = build_pass(job, &analog_sampling)?;
    let analog_job = job.for_pass(output, analog_sampling.clone());
    let mut record = match execute_pass(
        &analog_job,
        &analog_sampling,
        output,
        analog_built,
        None,
        cancellation,
    ) {
        Ok(record) => record,
        Err(error) => {
            prepared.discard();
            return Err(error);
        }
    };
    let windowed_dir = output.join(WINDOWED_PASS_DIRECTORY);
    let windowed_job = job.for_pass(&windowed_dir, job.sampling.clone());
    let windowed = if record.import_error.is_none() && record.normalized.is_some() {
        execute_pass(
            &windowed_job,
            &job.sampling,
            &windowed_dir,
            built,
            Some(prepared),
            cancellation,
        )
    } else {
        prepared.discard();
        Err("the analog pass did not produce a result".into())
    };
    match windowed {
        Ok(windowed) if windowed.import_error.is_none() && windowed.normalized.is_some() => {
            write_run_record(&windowed_dir, &windowed)?;
            let run_sha256 = hash_file(&windowed_dir.join("run.json"))?;
            let ids = vec![STOP_RULE_RESPONSE_ID.to_string()];
            let merged = merge_windowed(
                record.normalized.as_ref(),
                windowed.normalized.as_ref(),
                &ids,
            )?;
            record.normalized = Some(merged);
            record.sampling_precision_summary = Some(merged_precision_summary(
                &read_worker_result(output)?,
                &read_worker_result(&windowed_dir)?,
                &ids,
            )?);
            record.windowed_pass = Some(WindowedPass {
                directory: WINDOWED_PASS_DIRECTORY.into(),
                run_sha256,
                responses: ids,
            });
            record.variance_reduction = Some(variance);
            record.windowed_run = Some(Box::new(windowed));
        }
        Ok(windowed) => {
            // The windowed pass ran but its import failed: its record is kept for inspection, the run fails.
            write_run_record(&windowed_dir, &windowed)?;
            record.import_error = Some(format!(
                "the windowed pass of the magnet fast flux failed: {}. The analog pass is in this run directory; run again",
                windowed
                    .import_error
                    .as_deref()
                    .unwrap_or("it produced no result")
            ));
        }
        Err(error) => {
            record.import_error = Some(format!(
                "the windowed pass of the magnet fast flux failed: {error}. The analog pass is in this run directory; run again"
            ));
        }
    }
    write_run_record(output, &record)?;
    Ok(record)
}

/// Load and check the windowed pass of a two-pass run: its directory is a plain name, its run.json matches the
/// recorded hash, it is a complete run that verifies on its own, it used weight windows, and it describes the same
/// model, data and mesh as the analog pass that holds it.
fn load_windowed_pass(
    parent: &Path,
    pass: &WindowedPass,
    analog: &ReactorRun,
    scenario: &LoadedScenario,
) -> Result<ReactorRun, ReactorError> {
    if Path::new(&pass.directory)
        .file_name()
        .and_then(|n| n.to_str())
        != Some(pass.directory.as_str())
        || pass.directory != WINDOWED_PASS_DIRECTORY
        || pass.responses != [STOP_RULE_RESPONSE_ID]
    {
        return Err("the recorded windowed pass is not one this build knows".into());
    }
    let path = parent.join(&pass.directory).join("run.json");
    if hash_file(&path)? != pass.run_sha256 {
        return Err("the recorded windowed pass differs from its hash".into());
    }
    let windowed = load_reactor_run(&path, scenario)?;
    if !windowed
        .variance_reduction
        .as_ref()
        .is_some_and(VarianceReductionRecord::uses_windows)
        || windowed.windowed_pass.is_some()
        || windowed.variant_id != analog.variant_id
        || windowed.physics_sha256 != analog.physics_sha256
        || windowed.cross_sections_sha256 != analog.cross_sections_sha256
        || windowed.openmc_sha256 != analog.openmc_sha256
        || windowed.mesh != analog.mesh
        || windowed.sampling.target_relative_error != analog.sampling.target_relative_error
    {
        return Err("the windowed pass does not match the analog pass that holds it".into());
    }
    Ok(windowed)
}

/// Load locally recorded outputs and recheck exact request/normalization against
/// the original raw artifact. This authenticates no external scientific source.
pub fn load_reactor_run(
    path: &Path,
    scenario: &LoadedScenario,
) -> Result<ReactorRun, ReactorError> {
    let mut record: ReactorRun = serde_json::from_slice(&read_json_bytes(path)?)?;
    if record.schema_version != "faris-reactor-run/v0.1"
        || record.scenario_sha256 != scenario.source_sha256
        || record.scientific_qualification != "NOT_EVALUATED"
    {
        return Err("run record does not match scenario or supported scientific scope".into());
    }
    record.sampling.validate()?;
    let parent = path.parent().ok_or("run record needs a directory")?;
    let input_bytes = read_json_bytes(&parent.join("input.json"))?;
    if digest(&input_bytes) != record.input_sha256 {
        return Err("recorded input digest differs".into());
    }
    let input: Value = serde_json::from_slice(&input_bytes)?;
    let physics: PhysicsCase = serde_json::from_value(input["physics"].clone())?;
    let input_request: TransportRequest = serde_json::from_value(input["request"].clone())?;
    let (request, mesh) = if input_request.schema_version == TRANSPORT_REQUEST_LEGACY_VERSION {
        legacy_request_for_case(scenario, &physics)?
    } else {
        let mesh: FieldMesh = serde_json::from_value(input["mesh"].clone())?;
        let (mut request, mesh) = request_for_case_with_mesh(scenario, &physics, mesh)?;
        // The opt-in is recorded data, not derived from the scenario: carry it over and
        // let the request comparison below check everything else.
        if input_request.activation_spectra.is_some() {
            request.activation_spectra = input_request.activation_spectra.clone();
            request.validate_against(scenario)?;
        }
        (request, mesh)
    };
    let manifest = build_manifest(scenario)?;
    // Enforce current pretty-JSON budgets even when replaying a receipt created
    // by the earlier compact-size estimator.
    let current_preflight = mesh_payload_preflight(&request, &manifest, input_bytes.len())?;
    let saved_preflight_matches = if let Some(saved) = &record.mesh_preflight {
        let expected = match saved.serialization_method.as_str() {
            "pretty-json-v2" => current_preflight,
            "compact-json-v1" => {
                mesh_payload_preflight_legacy_compact(&request, &manifest, input_bytes.len())?
            }
            _ => return Err("unsupported mesh preflight method".into()),
        };
        saved == &expected
    } else {
        true
    };
    let checks = [
        (
            digest(&serde_json::to_vec(&physics)?) == record.physics_sha256,
            "physics input",
        ),
        (
            serde_json::from_value::<SamplingPlan>(input["sampling"].clone())? == record.sampling,
            "sampling plan",
        ),
        (
            record
                .execution
                .as_ref()
                .is_some_and(|e| e.execution_status == ExecutionStatus::Succeeded),
            "execution status (the run did not succeed)",
        ),
        (mesh == record.mesh, "field mesh"),
        (saved_preflight_matches, "mesh size preflight"),
        (physics.variant_id == record.variant_id, "arrangement"),
        (
            physics.scientific_scope == record.scientific_scope,
            "scientific scope",
        ),
        (
            input_request == request,
            "transport request (geometry, tallies or responses)",
        ),
    ];
    let differing: Vec<&str> = checks
        .iter()
        .filter(|(same, _)| !same)
        .map(|(_, what)| *what)
        .collect();
    if !differing.is_empty() {
        return Err(format!(
            "recorded case, mesh or response definitions differ from this FARIS build: {}. \
             The run was probably recorded by an earlier version; rerun the transport \
             case with this build to use it",
            differing.join(", ")
        )
        .into());
    }
    let raw_bytes = read_json_bytes(&parent.join("solver/transport-artifact.json"))?;
    if digest(&read_json_bytes(&parent.join("audit.json"))?) != record.audit_sha256
        || hash_file(&parent.join("reactor_transport.py"))? != record.adapter_sha256
    {
        return Err("recorded audit or adapter digest differs".into());
    }
    if record.raw_artifact_sha256.as_deref() != Some(digest(&raw_bytes).as_str()) {
        return Err("recorded raw artifact digest differs".into());
    }
    let artifact = TransportArtifact::from_bytes(&raw_bytes)?;
    // A target-error run's history count is only known from its bound receipt.
    let recorded_outcome = match record.worker_result_sha256.as_deref() {
        Some(_) => sampling_outcome(
            &serde_json::from_slice(&read_json_bytes(&parent.join("solver/worker-result.json"))?)?,
            &record.sampling,
        )?,
        None => None,
    };
    if artifact.solver.digest != format!("sha256:{}", record.openmc_sha256)
        || artifact.nuclear_data.digest.as_str()
            != input["nuclear_data_digest"]
                .as_str()
                .ok_or("missing recorded data digest")?
        || artifact.histories != expected_histories(&record.sampling, recorded_outcome.as_ref())?
    {
        return Err("recorded solver/data/sampling identity differs".into());
    }
    if let Some(expected_worker_digest) = record.worker_result_sha256.as_deref() {
        let worker_bytes = read_json_bytes(&parent.join("solver/worker-result.json"))?;
        if digest(&worker_bytes) != expected_worker_digest {
            return Err("recorded worker-result digest differs".into());
        }
        let worker: Value = serde_json::from_slice(&worker_bytes)?;
        validate_geometry_ownership_receipt(&worker, &record.input_sha256, scenario, &physics)?;
        record.sampling_precision_summary = Some(sampling_precision_summary(&worker)?);
        record.sampling_outcome = recorded_outcome;
        windows::verify_recorded(
            parent,
            &input,
            Some(&worker),
            record.variance_reduction.as_ref(),
            record.windowed_pass.is_some(),
        )?;
        // A worker that carries the shared batch statistics says which copy it used.
        if let Some(used) = worker
            .get("batch_statistics_sha256")
            .and_then(Value::as_str)
            && (used != digest(BATCH_STATISTICS_HELPER.as_bytes())
                || hash_file(&parent.join(BATCH_STATISTICS_FILE))? != used)
        {
            return Err("the recorded batch statistics differ from this build's".into());
        }
    } else if record.adapter_sha256 == embedded_worker_sha256() {
        return Err("current worker run is missing its bound geometry-ownership receipt".into());
    } else {
        windows::verify_recorded(
            parent,
            &input,
            None,
            record.variance_reduction.as_ref(),
            record.windowed_pass.is_some(),
        )?;
    }
    let normalized = normalize_transport_artifact(&request, &artifact, scenario)?;
    let normalized_spectra = if request.schema_version == TRANSPORT_REQUEST_VERSION {
        let spectra_bytes = read_json_bytes(&parent.join("solver/transport-spectra.json"))?;
        if record.transport_spectra_sha256.as_deref() != Some(digest(&spectra_bytes).as_str()) {
            return Err("recorded spectra digest differs".into());
        }
        let spectra: RawTransportSpectra = serde_json::from_slice(&spectra_bytes)?;
        Some(normalize_spectra(
            &spectra,
            &artifact,
            &normalized,
            &request,
            &record.input_sha256,
        )?)
    } else {
        None
    };
    check_geometric_volumes(&normalized, scenario, &mesh)?;
    // Two-pass run: the responses of the windowed pass come from its own recorded run, checked in full.
    let normalized = match record.windowed_pass.clone() {
        Some(pass) => {
            let windowed = load_windowed_pass(parent, &pass, &record, scenario)?;
            let merged = merge_windowed(
                Some(&normalized),
                windowed.normalized.as_ref(),
                &pass.responses,
            )?;
            record.sampling_precision_summary = Some(merged_precision_summary(
                &read_worker_result(parent)?,
                &read_worker_result(&parent.join(&pass.directory))?,
                &pass.responses,
            )?);
            record.windowed_run = Some(Box::new(windowed));
            merged
        }
        None => normalized,
    };
    if record.normalized.as_ref() != Some(&normalized) {
        let saved = serde_json::to_value(&record.normalized)?;
        let checked = serde_json::to_value(&normalized)?;
        let differing_fields: Vec<_> = saved
            .as_object()
            .into_iter()
            .flat_map(|object| object.keys())
            .filter(|key| saved[*key] != checked[*key])
            .cloned()
            .collect();
        let differing_results: Vec<_> = saved["results"]
            .as_array()
            .into_iter()
            .flatten()
            .zip(checked["results"].as_array().into_iter().flatten())
            .filter(|(a, b)| a != b)
            .take(3)
            .map(|(a, b)| {
                format!(
                    "{} saved_unit={} checked_unit={}",
                    a["response_id"].as_str().unwrap_or("?"),
                    a["unit"],
                    b["unit"]
                )
            })
            .collect();
        return Err(format!(
            "recorded normalization differs from checked arithmetic in fields: {}; first response mismatches: {}",
            differing_fields.join(", "),
            differing_results.join("; ")
        )
        .into());
    }
    if record.normalized_spectra != normalized_spectra {
        return Err("recorded spectra normalization differs from checked sidecar".into());
    }
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_solver_executables_fail_early_with_path_context() {
        let scenario = LoadedScenario::from_bytes(include_bytes!(
            "../../../scenarios/arc-inspired/cold-coupled-control.scenario.json"
        ))
        .unwrap();
        let physics = load_physics_case(
            Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../scenarios/arc-inspired/cold-coupled-control.reference.physics.json"
            )),
            &scenario,
        )
        .unwrap();
        let missing_python = std::env::temp_dir().join(format!(
            "faris-no-python-{}-not-installed",
            std::process::id()
        ));
        let missing_openmc = std::env::temp_dir().join(format!(
            "faris-no-openmc-{}-not-installed",
            std::process::id()
        ));
        let output =
            std::env::temp_dir().join(format!("faris-early-path-test-{}", std::process::id()));
        let audit = Path::new("unused-audit.json");
        let cross_sections = Path::new("unused-cross-sections.xml");
        let no_adapter: &[u8] = b"must not execute";

        let python_job = ReactorJob {
            scenario: &scenario,
            physics: &physics,
            audit,
            cross_sections,
            python: &missing_python,
            openmc: &missing_openmc,
            output: &output,
            sampling: SamplingPlan::default(),
            mesh: None,
            timeout: Duration::from_secs(10),
            activation_spectra: None,
            adapter: no_adapter,
            windows: None,
        };
        let error = run_reactor(&python_job, &Cancellation::default())
            .unwrap_err()
            .to_string();
        assert!(error.contains("Python interpreter"));
        assert!(error.contains(&missing_python.display().to_string()));

        let python = std::env::current_exe().unwrap();
        let openmc_job = ReactorJob {
            python: &python,
            ..python_job
        };
        let error = run_reactor(&openmc_job, &Cancellation::default())
            .unwrap_err()
            .to_string();
        assert!(error.contains("OpenMC executable"));
        assert!(error.contains(&missing_openmc.display().to_string()));
        assert!(
            !output.exists(),
            "preflight failures must not create a run directory"
        );
    }

    #[test]
    fn sampling_precision_summary_is_bounded_and_preserves_unresolved_local_checks() {
        let worker = serde_json::json!({
            "sampling_precision": {
                "plan_id": "faris-exploratory-precision-goals/v0.1",
                "purpose": "numerical sampling review only; not a physics or design acceptance test",
                "estimator": "one-standard-error relative to the response mean",
                "integrated_goal": 0.05,
                "local_goal": 0.1,
                "all_goals_met": false,
                "checks": [
                    {"response_id":"total-tritium-production", "quantity":"whole-model tritium production", "target_relative_standard_error":0.05, "observed_relative_standard_error":0.001, "met":true},
                    {"response_id":"heating-total-whole-model", "quantity":"whole-model deposited heating", "target_relative_standard_error":0.05, "observed_relative_standard_error":0.002, "met":true},
                    {"response_id":"mesh-flux-0", "quantity":"magnet or mesh neutron flux", "target_relative_standard_error":0.1, "observed_relative_standard_error":null, "met":false}
                ]
            }
        });
        let summary = sampling_precision_summary(&worker).unwrap();
        assert_eq!(summary.check_count, 3);
        assert_eq!(summary.checks_met, 2);
        assert_eq!(summary.checks_unmet, 1);
        assert!(!summary.all_goals_met);
        assert_eq!(summary.whole_model_tbr_relative_standard_error, Some(0.001));
        assert_eq!(
            summary.whole_model_heating_relative_standard_error,
            Some(0.002)
        );

        let mut inconsistent = worker;
        inconsistent["sampling_precision"]["checks"][0]["met"] = serde_json::json!(false);
        assert!(sampling_precision_summary(&inconsistent).is_err());
    }

    // Verifies: PERF-025
    #[test]
    fn precision_goals_are_judged_on_the_pass_each_response_is_reported_from() {
        let worker = |checks: serde_json::Value| {
            serde_json::json!({"sampling_precision": {
                "plan_id": "faris-exploratory-precision-goals/v0.1",
                "purpose": "p", "estimator": "e",
                "integrated_goal": 0.05, "local_goal": 0.1,
                "all_goals_met": false,
                "checks": checks,
            }})
        };
        let check = |id: &str, quantity: &str, target: f64, observed: f64| {
            serde_json::json!({"response_id": id, "quantity": quantity,
                "target_relative_standard_error": target,
                "observed_relative_standard_error": observed, "met": observed <= target})
        };
        let tritium = |observed| {
            check(
                "total-tritium-production",
                "whole-model tritium production",
                0.05,
                observed,
            )
        };
        let heating = |observed| {
            check(
                "heating-total-whole-model",
                "whole-model deposited heating",
                0.05,
                observed,
            )
        };
        let magnet = |observed| {
            check(
                STOP_RULE_RESPONSE_ID,
                "region-average fast neutron flux",
                0.1,
                observed,
            )
        };
        // the analog pass left the magnet flux far from its goal; the windowed pass met it and left its
        // own (unreported) tritium far off
        let analog = worker(serde_json::json!([
            tritium(0.002),
            heating(0.001),
            magnet(0.30)
        ]));
        let windowed = worker(serde_json::json!([
            tritium(0.60),
            heating(0.40),
            magnet(0.04)
        ]));
        let ids = vec![STOP_RULE_RESPONSE_ID.to_string()];
        // judged on the analog receipt alone, the run would say unmet
        assert_eq!(sampling_precision_summary(&analog).unwrap().checks_unmet, 1);
        let merged = merged_precision_summary(&analog, &windowed, &ids).unwrap();
        assert_eq!(merged.check_count, 3);
        assert_eq!(merged.checks_unmet, 0);
        assert!(merged.all_goals_met);
        // whole-model figures stay the analog pass's, not the windowed pass's
        assert_eq!(merged.whole_model_tbr_relative_standard_error, Some(0.002));
        assert_eq!(
            merged.whole_model_heating_relative_standard_error,
            Some(0.001)
        );
        // and a windowed response that missed its goal is unmet, whatever the analog pass saw
        let missed = worker(serde_json::json!([
            tritium(0.60),
            heating(0.40),
            magnet(0.2)
        ]));
        let merged = merged_precision_summary(&analog, &missed, &ids).unwrap();
        assert_eq!((merged.checks_unmet, merged.all_goals_met), (1, false));
        // a windowed receipt without the response's check cannot be merged
        let without = worker(serde_json::json!([tritium(0.6), heating(0.4)]));
        assert!(merged_precision_summary(&analog, &without, &ids).is_err());
    }

    #[test]
    fn mesh_uses_full_bin_volume_and_x_fastest_indexing() {
        let mesh = FieldMesh {
            id: "test".into(),
            dimensions: [12, 8, 12],
            lower_left_m: [-6.0, -2.0, -6.0],
            upper_right_m: [6.0, 2.0, 6.0],
        };
        assert_eq!(mesh.bins(), 1152);
        assert_eq!(mesh.bin_volume_m3(), 0.5);
        assert_eq!(mesh.bin_volume_m3() * mesh.bins() as f64, 576.0);
        assert_eq!(mesh.bin_center_m(0), Some([-5.5, -1.75, -5.5]));
        assert_eq!(mesh.bin_center_m(12), Some([-5.5, -1.25, -5.5]));
        assert_eq!(mesh.bin_center_m(96), Some([-5.5, -1.75, -4.5]));
        assert_eq!(mesh.bin_center_m(1152), None);
    }

    #[test]
    fn json_replay_preserves_float_bits_for_geometry_and_normalization() {
        // These magnitudes include the actual radial boundary and source-rate
        // scales that exposed lossy JSON parsing during record replay.
        let original: [f64; 4] = [
            2.280_000_000_000_000_2,
            293.594_308_480_163_36,
            1.861_813_786_415_852_5e20,
            1.0e-19,
        ];
        let encoded = serde_json::to_vec(&original).unwrap();
        let decoded: [f64; 4] = serde_json::from_slice(&encoded).unwrap();
        for (before, after) in original.into_iter().zip(decoded) {
            assert_eq!(before.to_bits(), after.to_bits());
        }
    }
    // Verifies: NUC-048
    #[test]
    fn run_timeout_admits_a_run_at_the_history_cap() {
        assert!(validate_run_timeout(Duration::from_secs(1)).is_ok());
        assert!(validate_run_timeout(Duration::from_secs(MAX_RUN_TIMEOUT_SECONDS)).is_ok());
        assert!(validate_run_timeout(Duration::ZERO).is_err());
        assert!(validate_run_timeout(Duration::from_secs(MAX_RUN_TIMEOUT_SECONDS + 1)).is_err());
        // 30M histories took about 68 min at 7 threads on the reference laptop;
        // 50M must fit with margin.
        const { assert!(MAX_RUN_TIMEOUT_SECONDS >= 2 * 68 * 60 * MAX_RUN_HISTORIES / 30_000_000) };
    }

    // Verifies: NUC-048
    #[test]
    fn fewer_than_thirty_batches_are_refused_with_the_reason_and_the_next_step() {
        for batches in [0, 1, 29] {
            let error = SamplingPlan {
                batches,
                ..Default::default()
            }
            .validate()
            .unwrap_err()
            .to_string();
            assert!(
                error.contains(&format!("{batches} batches is too few"))
                    && error.contains("standard error")
                    && error.contains("at least 30")
                    && error.contains("Set the batch count to 30 or more"),
                "{error}"
            );
        }
        assert!(
            SamplingPlan {
                batches: 30,
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn sampling_plan_rejects_unbounded_or_unusable_work() {
        assert_eq!(
            MAX_RUN_HISTORIES.to_string().len(),
            PREFLIGHT_HISTORIES.to_string().len()
        );
        assert!(SamplingPlan::default().validate().is_ok());
        assert!(
            SamplingPlan {
                batches: 29,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            SamplingPlan {
                batches: 500,
                particles_per_batch: 100_001,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            SamplingPlan {
                batches: 300,
                particles_per_batch: 100_000,
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
        assert!(
            SamplingPlan {
                seed: u64::MAX,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }

    // Verifies: PERF-025, NUC-043
    #[test]
    fn stop_rule_target_is_validated_and_optional() {
        let plan = |target| SamplingPlan {
            target_relative_error: target,
            ..Default::default()
        };
        assert!(plan(Some(0.05)).validate().is_ok());
        for bad in [0.0, -0.1, 0.51, f64::NAN, f64::INFINITY] {
            assert!(plan(Some(bad)).validate().is_err(), "{bad} accepted");
        }
        // The budget is `batches`, so the 30-batch minimum still applies.
        assert!(
            SamplingPlan {
                batches: 29,
                target_relative_error: Some(0.05),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }

    fn normalized_with(
        magnet: (f64, f64),
        tritium: (f64, f64),
        covariance: Vec<f64>,
    ) -> NormalizedTransportResult {
        let tally = |id: &str, domain: ResponseDomain, score: ScoreDefinition, mean: (f64, f64)| {
            NormalizedTally {
                response_id: id.into(),
                domain,
                score,
                estimator: TallyEstimator::Tracklength,
                mean: mean.0,
                standard_error: mean.1,
                unit: PhysicalUnit::NeutronsPerSquareMetreSecond,
                integrated_mean: mean.0,
                integrated_standard_error: mean.1,
                integrated_unit: PhysicalUnit::NeutronMetresPerSecond,
                volume_m3: 1.0,
                volume_standard_error_m3: 0.0,
            }
        };
        NormalizedTransportResult {
            schema_version: "faris-normalized-transport/v0.1".into(),
            scenario_id: "s".into(),
            scenario_sha256: "a".repeat(64),
            variant_id: "v".into(),
            source: faris_model::transport::DtSource {
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
            source_reaction_rate_per_s: 1.0,
            source_neutron_rate_per_s: 1.0,
            results: vec![
                tally(
                    STOP_RULE_RESPONSE_ID,
                    ResponseDomain::Component {
                        component_id: "magnets".into(),
                    },
                    ScoreDefinition::Flux,
                    magnet,
                ),
                tally(
                    "total-tritium-production",
                    ResponseDomain::WholeModel,
                    ScoreDefinition::Flux,
                    tritium,
                ),
            ],
            response_covariance: Some(crate::transport::ResponseCovariance {
                method: "batch-means-sample-covariance/v1".into(),
                batches: 30,
                response_ids: vec![
                    STOP_RULE_RESPONSE_ID.into(),
                    "total-tritium-production".into(),
                ],
                integrated: covariance,
            }),
        }
    }

    // Verifies: NUC-051, PERF-025
    #[test]
    fn a_merged_result_takes_each_response_from_the_pass_valid_for_it() {
        let analog = normalized_with(
            (1.0e15, 3.0e14),
            (2.4e20, 2.0e17),
            vec![9.0e28, 1.0e32, 1.0e32, 4.0e34],
        );
        let windowed = normalized_with(
            (1.37e15, 6.0e13),
            (6.0e20, 4.0e20),
            vec![3.6e27, 5.0e33, 5.0e33, 1.6e41],
        );
        let ids = vec![STOP_RULE_RESPONSE_ID.to_string()];
        let merged = merge_windowed(Some(&analog), Some(&windowed), &ids).unwrap();
        let by_id = |id: &str| merged.results.iter().find(|r| r.response_id == id).unwrap();
        // the magnet flux is the windowed pass's, the tritium stays the analog pass's
        assert_eq!(by_id(STOP_RULE_RESPONSE_ID).integrated_mean, 1.37e15);
        assert_eq!(by_id("total-tritium-production").integrated_mean, 2.4e20);
        assert_eq!(
            by_id("total-tritium-production").integrated_standard_error,
            2.0e17
        );
        // the passes are independent: no covariance across them, each response keeps its own variance
        let covariance = merged.response_covariance.as_ref().unwrap();
        assert_eq!(covariance.integrated, vec![3.6e27, 0.0, 0.0, 4.0e34]);
        // a pass that defines the response differently, or lacks it, cannot be merged
        let mut other = windowed.clone();
        other.results[0].unit = PhysicalUnit::Watts;
        assert!(merge_windowed(Some(&analog), Some(&other), &ids).is_err());
        assert!(merge_windowed(Some(&analog), None, &ids).is_err());
        assert!(merge_windowed(Some(&analog), Some(&windowed), &["nope".to_string()]).is_err());
    }

    #[test]
    fn the_analog_pass_watches_the_whole_model_responses_with_its_own_seed() {
        let scenario = LoadedScenario::from_bytes(include_bytes!(
            "../../../scenarios/arc-inspired/cold-coupled-control.scenario.json"
        ))
        .unwrap();
        let physics: PhysicsCase = serde_json::from_slice(include_bytes!(
            "../../../scenarios/arc-inspired/cold-coupled-control.reference.physics.json"
        ))
        .unwrap();
        let (request, _) = request_for_case_with_mesh(
            &scenario,
            &physics,
            FieldMesh::for_preset(&build_manifest(&scenario).unwrap(), MeshPreset::Coarse).unwrap(),
        )
        .unwrap();
        let watch = analog_watch(&request);
        assert!(watch.contains(&"total-tritium-production".to_string()));
        assert!(watch.contains(&"heating-total-whole-model".to_string()));
        assert!(!watch.contains(&STOP_RULE_RESPONSE_ID.to_string()));
        assert!(
            watch
                .iter()
                .all(|id| id.ends_with("whole-model") || id == "total-tritium-production")
        );
        let windowed = SamplingPlan {
            batches: 1000,
            particles_per_batch: 20_000,
            seed: 42,
            threads: 3,
            target_relative_error: Some(0.05),
            watch_responses: Vec::new(),
        };
        let analog = analog_pass_sampling(&windowed, watch.clone());
        analog.validate().unwrap();
        assert_ne!(analog.seed, windowed.seed);
        assert_eq!(analog.target_relative_error, Some(0.05));
        assert_eq!(analog.watch_responses, watch);
        for seed in [1, 42, RESERVED_WINDOW_SEEDS[0] - 7_919, i64::MAX as u64 - 3] {
            let other = other_seed(seed);
            assert!(other != seed && other > 0 && other <= i64::MAX as u64);
            assert!(!RESERVED_WINDOW_SEEDS.contains(&other));
        }
    }

    #[test]
    fn watched_responses_need_a_target_and_are_distinct() {
        let plan = |target, watch: &[&str]| SamplingPlan {
            target_relative_error: target,
            watch_responses: watch.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        assert!(plan(Some(0.05), &["a", "b"]).validate().is_ok());
        assert!(plan(None, &["a"]).validate().is_err());
        assert!(plan(Some(0.05), &["a", "a"]).validate().is_err());
        assert!(plan(Some(0.05), &[""]).validate().is_err());
        // plans without watched responses write exactly as before
        let json = serde_json::to_string(&plan(Some(0.05), &[])).unwrap();
        assert!(!json.contains("watch"));
    }

    // Verifies: NUC-043
    #[test]
    fn a_target_is_not_met_while_the_batches_disagree() {
        let plan = SamplingPlan {
            batches: 100,
            target_relative_error: Some(0.05),
            ..Default::default()
        };
        let with = |batches: u64, reason: &str, achieved: f64, vov: f64| {
            let mut value = outcome_json(batches, reason, Some(achieved));
            value["sampling_outcome"]["variance_of_variance"] = vov.into();
            value["sampling_outcome"]["variance_of_variance_limit"] = VOV_LIMIT.into();
            value["sampling_outcome"]["target_response_id"] = "total-tritium-production".into();
            value
        };
        let met = sampling_outcome(&with(40, "target_met", 0.04, 0.02), &plan)
            .unwrap()
            .unwrap();
        assert_eq!(
            met.limiting_response.as_deref(),
            Some("total-tritium-production")
        );
        assert!(met.summary().contains("total-tritium-production"));
        // met with a VOV above the limit may not stop early, and may not be called met
        assert!(sampling_outcome(&with(40, "target_met", 0.04, 0.3), &plan).is_err());
        assert!(sampling_outcome(&with(40, "target_unsettled", 0.04, 0.3), &plan).is_err());
        let unsettled = sampling_outcome(&with(100, "target_unsettled", 0.04, 0.3), &plan)
            .unwrap()
            .unwrap();
        assert_eq!(unsettled.stop_reason, StopReason::TargetUnsettled);
        assert!(unsettled.summary().contains("not reliable"));
        // unsettled needs a VOV above the limit and an error within the target
        assert!(sampling_outcome(&with(100, "target_unsettled", 0.04, 0.05), &plan).is_err());
        assert!(sampling_outcome(&with(100, "target_unsettled", 0.08, 0.3), &plan).is_err());
        // another limit than ours is refused
        let mut other = with(40, "target_met", 0.04, 0.02);
        other["sampling_outcome"]["variance_of_variance_limit"] = 0.5.into();
        assert!(sampling_outcome(&other, &plan).is_err());
    }

    // Verifies: NUC-053
    #[test]
    fn a_run_may_not_use_the_seeds_of_the_window_steps() {
        for seed in RESERVED_WINDOW_SEEDS {
            let plan = SamplingPlan {
                seed,
                ..Default::default()
            };
            assert!(
                plan.validate()
                    .unwrap_err()
                    .to_string()
                    .contains("reserved")
            );
        }
    }

    // Verifies: PERF-025
    #[test]
    fn plans_without_a_target_read_and_write_as_before() {
        let old = r#"{"batches":100,"particles_per_batch":10000,"seed":123456789,"threads":1}"#;
        let plan: SamplingPlan = serde_json::from_str(old).unwrap();
        assert_eq!(plan, SamplingPlan::default());
        assert_eq!(serde_json::to_string(&plan).unwrap(), old);
        let with: SamplingPlan = serde_json::from_str(
            r#"{"batches":100,"particles_per_batch":10000,"seed":1,"threads":2,"target_relative_error":0.05}"#,
        )
        .unwrap();
        assert_eq!(with.target_relative_error, Some(0.05));
        assert!(serde_json::to_string(&with).unwrap().contains("0.05"));
    }

    fn outcome_json(batches: u64, reason: &str, achieved: Option<f64>) -> Value {
        serde_json::json!({"sampling_outcome": {
            "batches_realized": batches,
            "histories_realized": batches * 10_000,
            "target_relative_error": 0.05,
            "achieved_relative_error": achieved,
            "stop_reason": reason,
        }})
    }

    // Verifies: PERF-025, NUC-043
    #[test]
    fn sampling_outcome_accepts_only_stops_the_rule_can_explain() {
        let plan = SamplingPlan {
            batches: 100,
            target_relative_error: Some(0.05),
            ..Default::default()
        };
        let met = sampling_outcome(&outcome_json(47, "target_met", Some(0.049)), &plan)
            .unwrap()
            .unwrap();
        assert_eq!(met.stop_reason, StopReason::TargetMet);
        assert_eq!(expected_histories(&plan, Some(&met)).unwrap(), 470_000);
        assert!(met.summary().contains("target was met"));
        let spent = sampling_outcome(&outcome_json(100, "budget_exhausted", Some(0.08)), &plan)
            .unwrap()
            .unwrap();
        assert!(spent.summary().contains("Target not met"));
        assert!(spent.summary().contains("run longer"));
        // Stopped early without meeting the target, below the minimum, or over budget.
        for (batches, reason, achieved) in [
            (47, "target_met", Some(0.06)),
            (47, "budget_exhausted", Some(0.06)),
            (29, "target_met", Some(0.01)),
            (101, "budget_exhausted", Some(0.06)),
            (100, "budget_exhausted", Some(0.04)),
            (100, "fixed_count", Some(0.04)),
        ] {
            assert!(
                sampling_outcome(&outcome_json(batches, reason, achieved), &plan).is_err(),
                "{batches} {reason} {achieved:?} accepted"
            );
        }
        // A target-error plan needs the receipt; a fixed plan may lack one.
        assert!(sampling_outcome(&serde_json::json!({}), &plan).is_err());
        assert!(
            sampling_outcome(&serde_json::json!({}), &SamplingPlan::default())
                .unwrap()
                .is_none()
        );
        assert_eq!(
            expected_histories(&SamplingPlan::default(), None).unwrap(),
            1_000_000
        );
    }

    // Verifies: GEO-046
    #[test]
    fn mesh_presets_are_bounded_and_local_pair_shares_exact_bounds() {
        let scenario = LoadedScenario::from_bytes(include_bytes!(
            "../../../scenarios/arc-inspired/cold-coupled-control.scenario.json"
        ))
        .unwrap();
        let manifest = build_manifest(&scenario).unwrap();
        let coarse = FieldMesh::for_preset(&manifest, MeshPreset::Coarse).unwrap();
        let local_coarse =
            FieldMesh::for_preset(&manifest, MeshPreset::OutboardLocalCoarse).unwrap();
        let local_fine = FieldMesh::for_preset(&manifest, MeshPreset::OutboardLocal).unwrap();
        let port_window = FieldMesh::for_preset(&manifest, MeshPreset::OutboardPortWindow).unwrap();
        assert_eq!(coarse.dimensions, [12, 8, 12]);
        assert_eq!(local_coarse.lower_left_m, local_fine.lower_left_m);
        assert_eq!(local_coarse.upper_right_m, local_fine.upper_right_m);
        assert_eq!(local_coarse.bins(), 864);
        assert_eq!(local_fine.bins(), 6912);
        assert_eq!(port_window.bins(), 1);
        assert!((port_window.bin_volume_m3() - 1.24 * 0.3 * 0.3).abs() < 1.0e-12);
        assert!(local_fine.validate(&manifest).is_ok());

        let invalid = FieldMesh {
            dimensions: [usize::MAX, 2, 2],
            ..local_fine.clone()
        };
        assert!(invalid.validate(&manifest).is_err());
        let too_many = FieldMesh {
            dimensions: [64, 64, 64],
            ..local_fine.clone()
        };
        assert!(too_many.validate(&manifest).is_err());
        let inverted = FieldMesh {
            upper_right_m: local_fine.lower_left_m,
            ..local_fine
        };
        assert!(inverted.validate(&manifest).is_err());
    }

    #[test]
    fn local_mesh_json_preflight_fits_explicit_artifact_and_package_budgets() {
        let scenario = LoadedScenario::from_bytes(include_bytes!(
            "../../../scenarios/arc-inspired/cold-coupled-control.scenario.json"
        ))
        .unwrap();
        let physics = load_physics_case(
            Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../scenarios/arc-inspired/cold-coupled-control.reference.physics.json"
            )),
            &scenario,
        )
        .unwrap();
        let manifest = build_manifest(&scenario).unwrap();
        let mesh = FieldMesh::for_preset(&manifest, MeshPreset::OutboardLocal).unwrap();
        let (request, _) = request_for_case_with_mesh(&scenario, &physics, mesh).unwrap();
        let preflight = mesh_payload_preflight(&request, &manifest, 128 * 1024).unwrap();
        let compact =
            mesh_payload_preflight_legacy_compact(&request, &manifest, 128 * 1024).unwrap();
        assert_eq!(preflight.serialization_method, "pretty-json-v2");
        assert_eq!(compact.serialization_method, "compact-json-v1");
        assert!(compact.request_json_bytes < preflight.request_json_bytes);
        assert!(preflight.request_json_bytes < MAX_JSON_BYTES as usize);
        assert!(preflight.raw_artifact_upper_bound_bytes < MAX_TRANSPORT_ARTIFACT_BYTES);
        assert!(preflight.run_json_upper_bound_bytes < MAX_JSON_BYTES as usize);
        assert!(preflight.package_content_upper_bound_bytes < 32 * 1024 * 1024);
        assert!(mesh_payload_preflight(&request, &manifest, 64 * 1024 * 1024).is_err());
        let window = FieldMesh::for_preset(&manifest, MeshPreset::OutboardPortWindow).unwrap();
        let (window_request, _) = request_for_case_with_mesh(&scenario, &physics, window).unwrap();
        assert!(
            window_request
                .responses
                .iter()
                .any(|r| r.id == "outboard-port-window-flux")
        );
        assert!(mesh_payload_preflight(&window_request, &manifest, 128 * 1024).is_ok());
    }

    #[test]
    fn compact_and_pretty_preflight_versions_keep_exact_recorded_sizes() {
        let scenario = LoadedScenario::from_bytes(include_bytes!(
            "../../../scenarios/arc-inspired/cold-coupled-control.scenario.json"
        ))
        .unwrap();
        let physics = load_physics_case(
            Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../scenarios/arc-inspired/cold-coupled-control.breeder-emphasis.physics.json"
            )),
            &scenario,
        )
        .unwrap();
        let manifest = build_manifest(&scenario).unwrap();
        let coarse = FieldMesh::for_preset(&manifest, MeshPreset::Coarse).unwrap();
        let (request, _) = request_for_case_with_mesh(&scenario, &physics, coarse).unwrap();
        let old = mesh_payload_preflight_legacy_compact(&request, &manifest, 296_682).unwrap();
        assert_eq!(
            old,
            MeshPreflight {
                serialization_method: "compact-json-v1".into(),
                request_json_bytes: 130_991,
                raw_artifact_upper_bound_bytes: 502_313,
                run_json_upper_bound_bytes: 622_940,
                package_content_upper_bound_bytes: 5_653_487,
            }
        );
        let current = mesh_payload_preflight(&request, &manifest, 296_682).unwrap();
        assert_eq!(
            current,
            MeshPreflight {
                serialization_method: "pretty-json-v2".into(),
                request_json_bytes: 236_831,
                raw_artifact_upper_bound_bytes: 786_389,
                run_json_upper_bound_bytes: 815_094,
                package_content_upper_bound_bytes: 6_366_548,
            }
        );
    }

    /// A tiny stand-in for a local library: one file per nuclide and per element, and the audit
    /// that describes them, with every datum the binding reads. The files are not real data.
    struct SyntheticLibrary {
        dir: tempfile::TempDir,
        audit: Value,
    }

    impl SyntheticLibrary {
        fn new(physics: &PhysicsCase) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let cross_sections = dir.path().join("cross_sections.xml");
            std::fs::write(&cross_sections, b"<cross_sections/>").unwrap();
            let mut neutron = serde_json::Map::new();
            let mut photon = serde_json::Map::new();
            for m in &physics.materials {
                let MaterialRecipe::NuclideMixture { nuclides, .. } = &m.recipe else {
                    continue;
                };
                for n in nuclides {
                    let name = &n.nuclide;
                    let file = format!("{name}.h5");
                    std::fs::write(dir.path().join(&file), name.as_bytes()).unwrap();
                    neutron.insert(
                        name.clone(),
                        serde_json::json!({
                            "readable_by_openmc_data_api": true,
                            "nuclide_name_in_hdf5": name,
                            "reactions": {"301": {"present": true}},
                            "relative_path": file,
                            "size_bytes": name.len(),
                            "sha256": digest(name.as_bytes()),
                            "temperatures_k": [293.6],
                        }),
                    );
                    let element: String = name
                        .chars()
                        .take_while(|c| c.is_ascii_alphabetic())
                        .collect();
                    let file = format!("{element}-photon.h5");
                    std::fs::write(dir.path().join(&file), element.as_bytes()).unwrap();
                    photon.insert(
                        element.clone(),
                        serde_json::json!({
                            "library_entry_present": true,
                            "readable_by_h5py": true,
                            "atomic_relaxation_populated": true,
                            "relative_path": file,
                            "size_bytes": element.len(),
                            "sha256": digest(element.as_bytes()),
                        }),
                    );
                }
            }
            let audit = serde_json::json!({
                "schema": "faris.openmc-library-audit/1.0.0",
                "cross_sections_xml": {"sha256": hash_file(&cross_sections).unwrap()},
                "neutron_library": neutron,
                "photon_atomic_library": photon,
            });
            Self { dir, audit }
        }

        fn cross_sections(&self) -> PathBuf {
            self.dir.path().join("cross_sections.xml")
        }
    }

    fn reference_case() -> (LoadedScenario, PhysicsCase) {
        let scenario = LoadedScenario::from_bytes(include_bytes!(
            "../../../scenarios/arc-inspired/cold-coupled-control.scenario.json"
        ))
        .unwrap();
        let physics = load_physics_case(
            Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../scenarios/arc-inspired/cold-coupled-control.reference.physics.json"
            )),
            &scenario,
        )
        .unwrap();
        (scenario, physics)
    }

    // Verifies: NUC-062
    #[test]
    fn a_missing_photon_element_refuses_the_run_and_names_the_element() {
        let (scenario, physics) = reference_case();
        let library = SyntheticLibrary::new(&physics);
        let cross_sections = library.cross_sections();
        let bound = bind_audited_library(&physics, &scenario, &library.audit, &cross_sections);
        assert!(bound.is_ok(), "{:?}", bound.err());
        let elements: Vec<String> = library.audit["photon_atomic_library"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        // Every element of the model is needed: removing any one refuses, naming that element.
        assert_eq!(elements.len(), 8);
        for element in &elements {
            let mut audit = library.audit.clone();
            audit["photon_atomic_library"]
                .as_object_mut()
                .unwrap()
                .remove(element);
            let error = bind_audited_library(&physics, &scenario, &audit, &cross_sections)
                .unwrap_err()
                .to_string();
            assert!(error.contains(element.as_str()), "{element}: {error}");
        }
        // An element present but without populated atomic relaxation is a readiness gap, not a silent run.
        let mut audit = library.audit.clone();
        audit["photon_atomic_library"]["Fe"]["atomic_relaxation_populated"] = false.into();
        let error = bind_audited_library(&physics, &scenario, &audit, &cross_sections)
            .unwrap_err()
            .to_string();
        assert!(error.contains("atomic-relaxation"), "{error}");
    }

    // Verifies: NUC-026
    #[test]
    fn a_nuclide_without_heating_or_a_readable_file_is_refused_by_name() {
        let (scenario, physics) = reference_case();
        let library = SyntheticLibrary::new(&physics);
        let cross_sections = library.cross_sections();
        let refused = |audit: &Value| {
            bind_audited_library(&physics, &scenario, audit, &cross_sections)
                .unwrap_err()
                .to_string()
        };
        let mut audit = library.audit.clone();
        audit["neutron_library"]["W184"]["reactions"]["301"]["present"] = false.into();
        let error = refused(&audit);
        assert!(
            error.contains("MT=301") && error.contains("W184"),
            "{error}"
        );
        let mut audit = library.audit.clone();
        audit["neutron_library"]
            .as_object_mut()
            .unwrap()
            .remove("Li6");
        let error = refused(&audit);
        assert!(error.contains("Li6"), "{error}");
        let mut audit = library.audit.clone();
        audit["neutron_library"]["Be9"]["sha256"] = "0".repeat(64).into();
        let error = refused(&audit);
        assert!(error.contains("Be9"), "{error}");
    }

    // Verifies: NUC-014, NUC-071
    #[test]
    fn every_heating_response_names_its_convention_and_particle_scope() {
        let (scenario, physics) = reference_case();
        let (request, _) = request_for_case(&scenario, &physics).unwrap();
        let value = serde_json::to_value(&request).unwrap();
        let heating: Vec<&Value> = value["responses"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["score"]["kind"] == "heating")
            .collect();
        assert!(heating.len() >= 5);
        // Every heating number is asked for with its convention and the particles that deposit it.
        let scopes: BTreeSet<&str> = heating
            .iter()
            .map(|r| {
                assert_eq!(r["score"]["convention"], "heating");
                r["score"]["particle_scope"].as_str().unwrap()
            })
            .collect();
        assert_eq!(
            scopes,
            BTreeSet::from(["total", "neutron", "photon", "electron", "positron"])
        );
        // The second convention exists in the request type but this inventory cannot provide it,
        // and asking for it is refused with the way out rather than answered with the first.
        let mut local = request.clone();
        for r in &mut local.responses {
            if let ScoreDefinition::Heating { convention, .. } = &mut r.score {
                *convention = HeatingConvention::HeatingLocal;
            }
        }
        let error = local.validate_against(&scenario).unwrap_err();
        assert!(
            error.contains("heating-local is unavailable") && error.contains("coupled heating"),
            "{error}"
        );
    }

    // Verifies: NUC-061
    #[test]
    fn the_bound_inventory_names_both_libraries_it_mixes() {
        let (scenario, physics) = reference_case();
        let library = SyntheticLibrary::new(&physics);
        let cross_sections = library.cross_sections();
        let bound =
            bind_audited_library(&physics, &scenario, &library.audit, &cross_sections).unwrap();
        let NuclearDataSelection::Inventory { name, version, .. } = &bound.nuclear_data else {
            panic!("the audited library binds as an inventory");
        };
        for label in [name, version] {
            assert!(
                label.contains("FENDL-3.2") && label.contains("ENDF/B-VII.1"),
                "{label}"
            );
        }
    }

    /// A job whose "Python" is a shell script, so the run, its exit code and its files can be faked.
    #[cfg(unix)]
    fn run_with_fake_worker(script: &str) -> (ReactorRun, tempfile::TempDir) {
        use std::os::unix::fs::PermissionsExt;
        let (scenario, physics) = reference_case();
        let library = SyntheticLibrary::new(&physics);
        let audit = library.dir.path().join("audit.json");
        std::fs::write(&audit, library.audit.to_string()).unwrap();
        let python = library.dir.path().join("python");
        std::fs::write(&python, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
        let openmc = library.dir.path().join("openmc");
        std::fs::write(&openmc, b"not a solver").unwrap();
        let output = library.dir.path().join("run");
        let cross_sections = library.cross_sections();
        let job = ReactorJob {
            scenario: &scenario,
            physics: &physics,
            audit: &audit,
            cross_sections: &cross_sections,
            python: &python,
            openmc: &openmc,
            output: &output,
            sampling: SamplingPlan::default(),
            mesh: None,
            timeout: Duration::from_secs(30),
            activation_spectra: None,
            adapter: b"fake worker",
            windows: None,
        };
        let record = run_reactor(&job, &Cancellation::default()).unwrap();
        (record, library.dir)
    }

    // Verifies: NUC-004
    #[cfg(unix)]
    #[test]
    fn injected_solver_faults_are_never_a_result_or_a_pass() {
        // Exit code: the failure is kept as the execution status and nothing is imported.
        let (record, _dir) = run_with_fake_worker("exit 3");
        let execution = record.execution.as_ref().unwrap();
        assert_eq!(execution.execution_status, ExecutionStatus::Failed);
        assert_eq!(execution.exit_code, Some(3));
        assert!(record.normalized.is_none() && record.raw_artifact_sha256.is_none());
        assert_eq!(record.scientific_qualification, "NOT_EVALUATED");
        // A clean exit with no artifacts, a truncated worker result and an empty one all fail the import.
        let faults = [
            "exit 0",
            "mkdir -p \"$5\"; printf '{\"execution_status\": \"COMPL' > \"$5/worker-result.json\"",
            "mkdir -p \"$5\"; printf '{}' > \"$5/worker-result.json\"",
        ];
        for script in faults {
            let (record, _dir) = run_with_fake_worker(script);
            let execution = record.execution.as_ref().unwrap();
            assert_eq!(execution.execution_status, ExecutionStatus::Succeeded);
            let reason = record.import_error.as_deref().unwrap_or_default();
            assert!(
                !reason.is_empty(),
                "{script}: an inconsistent run needs a reason"
            );
            assert!(record.normalized.is_none() && record.raw_artifact_sha256.is_none());
            assert_eq!(record.scientific_qualification, "NOT_EVALUATED");
        }
    }

    // Verifies: NUC-007
    #[cfg(unix)]
    #[test]
    fn a_run_receipt_carries_every_binding_and_a_missing_one_blocks_reuse() {
        let (scenario, _) = reference_case();
        let (record, dir) = run_with_fake_worker("exit 0");
        let receipt = serde_json::to_value(&record).unwrap();
        // Code (solver, interpreter, worker), data (library, audit, physics), geometry and source
        // (scenario, the whole input), seed and threads are all recorded; the variance-reduction record exists
        // only for a windowed run.
        let bindings = [
            "python_sha256",
            "openmc_sha256",
            "adapter_sha256",
            "cross_sections_sha256",
            "audit_sha256",
            "physics_sha256",
            "scenario_sha256",
            "input_sha256",
            "sampling",
        ];
        for field in bindings {
            assert!(receipt.get(field).is_some(), "receipt lacks {field}");
        }
        assert!(receipt["sampling"]["seed"].is_u64() && receipt["sampling"]["threads"].is_u64());
        for field in bindings {
            let mut damaged = receipt.clone();
            damaged.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<ReactorRun>(damaged).is_err(),
                "a receipt without {field} must not load"
            );
        }
        // A receipt whose input file was changed after the run does not load either.
        let run_json = dir.path().join("run/run.json");
        std::fs::write(dir.path().join("run/input.json"), b"{}").unwrap();
        let error = load_reactor_run(&run_json, &scenario)
            .unwrap_err()
            .to_string();
        assert!(error.contains("input digest differs"), "{error}");
    }
}
