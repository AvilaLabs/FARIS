//! Checked normalization for fixed-source transport artifacts.

use faris_model::math;
use faris_model::transport::{
    ACTIVATION_SPECTRA_FISPACT_709, ResponseDomain, ScoreDefinition,
    TRANSPORT_ARTIFACT_LEGACY_VERSION, TRANSPORT_ARTIFACT_VERSION,
    TRANSPORT_REQUEST_LEGACY_VERSION, TransportRequest,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const ELEMENTARY_CHARGE_C: f64 = 1.602_176_634e-19;
pub const MAX_TRANSPORT_ARTIFACT_BYTES: usize = faris_model::transport::MAX_ARTIFACT_BYTES;
pub const RESPONSE_COVARIANCE_METHOD: &str = "batch-means-sample-covariance/v1";
const COVARIANCE_SYMMETRY_TOLERANCE: f64 = 1.0e-12;
const COVARIANCE_DIAGONAL_TOLERANCE: f64 = 1.0e-6;
/// Slack on the unit-diagonal correlation matrix for rounding in the adapter's
/// covariance, applied to correlation bounds and Cholesky pivots alike.
const CORRELATION_TOLERANCE: f64 = 1.0e-9;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TransportArtifact {
    pub schema_version: String,
    /// Full request echoed by the adapter, binding all response definitions.
    pub request: TransportRequest,
    pub solver: ToolIdentity,
    pub nuclear_data: ToolIdentity,
    pub histories: u64,
    pub volumes: Vec<DomainVolume>,
    pub tallies: Vec<RawTally>,
    /// Batch-resolved sampling covariance between scalar responses; absent in
    /// artifacts written before the worker recorded batch values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_covariance: Option<RawResponseCovariance>,
}

/// Raw (per source neutron) covariance of scalar response means, as written by an adapter.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RawResponseCovariance {
    pub method: String,
    pub batches: u32,
    /// Response IDs in matrix order; each names one scalar (non-mesh) tally.
    pub response_ids: Vec<String>,
    /// Row-major n x n covariance of the raw tally means, in raw tally units
    /// per source neutron (product units off the diagonal).
    pub raw_per_source: Vec<f64>,
    /// Per-batch values this matrix was computed from, beside the artifact.
    pub batch_values_file: String,
    pub batch_values_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ToolIdentity {
    pub name: String,
    pub version: String,
    pub digest: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DomainVolume {
    pub domain: ResponseDomain,
    pub value: f64,
    /// One standard error for a stochastic volume estimate; zero for exact
    /// analytic torus/bin volume controls.
    #[serde(default)]
    pub standard_error: f64,
    pub unit: VolumeUnit,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VolumeUnit {
    CubicMetre,
    CubicCentimetre,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RawTallyUnit {
    /// OpenMC volume-integrated flux score, cm per source neutron.
    CmPerSource,
    /// Integrated reaction events per source neutron.
    EventsPerSource,
    /// Produced particles per source neutron (OpenMC particle-production score).
    ParticlesPerSource,
    /// Integrated heating score, eV per source neutron.
    EvPerSource,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RawTally {
    pub response_id: String,
    /// The OpenMC tally estimator used for this score.
    pub estimator: TallyEstimator,
    pub unit: RawTallyUnit,
    pub mean: f64,
    /// One standard error of the Monte Carlo estimator; not a confidence interval.
    pub standard_error: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TallyEstimator {
    Analog,
    Collision,
    Tracklength,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NormalizedTransportResult {
    pub schema_version: String,
    pub scenario_id: String,
    pub scenario_sha256: String,
    pub variant_id: String,
    pub source: faris_model::transport::DtSource,
    pub solver: ToolIdentity,
    pub nuclear_data: ToolIdentity,
    pub histories: u64,
    pub source_reaction_rate_per_s: f64,
    pub source_neutron_rate_per_s: f64,
    pub results: Vec<NormalizedTally>,
    /// Sampling covariance between scalar response means of this run, estimated
    /// from batch-resolved tallies. None for records made before batch-resolved
    /// tallies existed; consumers that need correlations must then fail closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_covariance: Option<ResponseCovariance>,
}

/// Monte Carlo sampling covariance of scalar response means from one transport run.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ResponseCovariance {
    /// Estimator identity, e.g. "batch-means-sample-covariance/v1": sample
    /// covariance of per-batch values divided by the number of batches.
    pub method: String,
    pub batches: u32,
    /// Response IDs in matrix order; each names a scalar entry of `results`.
    pub response_ids: Vec<String>,
    /// Row-major n x n covariance of `integrated_mean` values, in each
    /// response's `integrated_unit` (product units off the diagonal).
    /// Monte Carlo sampling only: volume-estimate uncertainty is not included.
    pub integrated: Vec<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NormalizedTally {
    pub response_id: String,
    pub domain: ResponseDomain,
    pub score: ScoreDefinition,
    pub estimator: TallyEstimator,
    pub mean: f64,
    pub standard_error: f64,
    pub unit: PhysicalUnit,
    pub integrated_mean: f64,
    pub integrated_standard_error: f64,
    pub integrated_unit: PhysicalUnit,
    pub volume_m3: f64,
    #[serde(default)]
    pub volume_standard_error_m3: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RawTransportSpectra {
    pub schema_version: String,
    pub request: TransportRequest,
    pub scenario_sha256: String,
    pub variant_id: String,
    pub input_sha256: String,
    pub solver: ToolIdentity,
    pub nuclear_data: ToolIdentity,
    pub histories: u64,
    pub spectra: Vec<RawEnergySpectrum>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RawEnergySpectrum {
    pub component_id: String,
    pub particle: String,
    /// Set only on the opt-in activation spectra (`fispact-709`); the full-energy
    /// spectra of every request leave it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_structure: Option<String>,
    pub estimator: TallyEstimator,
    pub unit: String,
    pub energy_edges_ev: Vec<f64>,
    pub mean_cm_per_source_per_bin: Vec<f64>,
    pub standard_error_cm_per_source_per_bin: Vec<f64>,
    pub volume_cm3: f64,
    pub volume_standard_error_cm3: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NormalizedEnergySpectrum {
    pub component_id: String,
    pub particle: String,
    pub estimator: TallyEstimator,
    pub energy_edges_ev: Vec<f64>,
    /// Per-energy-group particle flux; Monte Carlo uncertainty only.
    pub mean_per_square_metre_second: Vec<f64>,
    pub standard_error_per_square_metre_second: Vec<f64>,
    pub normalization_volume_m3: f64,
    pub volume_standard_error_m3: f64,
}

/// Number of boundaries of the `fispact-709` group structure.
const ACTIVATION_709_EDGES: usize = 710;

pub fn normalize_spectra(
    raw: &RawTransportSpectra,
    artifact: &TransportArtifact,
    normalized: &NormalizedTransportResult,
    expected_request: &TransportRequest,
    expected_input_sha256: &str,
) -> Result<Vec<NormalizedEnergySpectrum>, TransportError> {
    let fail = |s: &str| TransportError::Invalid(s.into());
    if raw.schema_version != "faris-transport-spectra/v0.1"
        || raw.request != *expected_request
        || raw.scenario_sha256 != expected_request.scenario_sha256
        || raw.variant_id != expected_request.variant_id
        || raw.input_sha256 != expected_input_sha256
        || raw.solver != normalized.solver
        || raw.nuclear_data != normalized.nuclear_data
        || raw.histories != normalized.histories
    {
        return Err(fail(
            "spectra identity differs from requested transport run",
        ));
    }
    let mut output = Vec::new();
    let mut seen = BTreeSet::new();
    let mut seen_activation = BTreeSet::new();
    for s in &raw.spectra {
        let activation = match s.group_structure.as_deref() {
            None => false,
            Some(ACTIVATION_SPECTRA_FISPACT_709)
                if expected_request.activation_spectra.as_deref()
                    == Some(ACTIVATION_SPECTRA_FISPACT_709)
                    && s.particle == "neutron" =>
            {
                true
            }
            Some(_) => return Err(fail("unrequested or unknown spectrum group structure")),
        };
        // Activation spectra start at the library's lowest boundary, not at zero; the
        // adapter checks the exact boundary values against its embedded constant.
        let edges_ok = if activation {
            s.energy_edges_ev.len() == ACTIVATION_709_EDGES
                && s.energy_edges_ev.first().is_some_and(|e| *e > 0.0)
        } else {
            s.energy_edges_ev.first() == Some(&0.0)
                && s.energy_edges_ev.last().is_some_and(|e| *e >= 1.0e9)
        };
        let fresh = if activation {
            seen_activation.insert(s.component_id.as_str())
        } else {
            seen.insert((s.component_id.as_str(), s.particle.as_str()))
        };
        if !fresh
            || !matches!(s.particle.as_str(), "neutron" | "photon")
            || s.unit != "cm_per_source_per_energy_bin"
            || s.estimator != TallyEstimator::Tracklength
            || s.energy_edges_ev.len() < 2
            || !edges_ok
            || s.energy_edges_ev.windows(2).any(|w| w[0] >= w[1])
            || s.mean_cm_per_source_per_bin.len() + 1 != s.energy_edges_ev.len()
            || s.standard_error_cm_per_source_per_bin.len() != s.mean_cm_per_source_per_bin.len()
            || !s.volume_cm3.is_finite()
            || s.volume_cm3 <= 0.0
            || !s.volume_standard_error_cm3.is_finite()
            || s.volume_standard_error_cm3 < 0.0
            || s.mean_cm_per_source_per_bin
                .iter()
                .any(|x| !x.is_finite() || *x < 0.0)
            || s.standard_error_cm_per_source_per_bin
                .iter()
                .any(|x| !x.is_finite() || *x < 0.0)
        {
            return Err(fail("invalid or incomplete full-energy spectrum"));
        }
        let response = normalized
            .results
            .iter()
            .find(|r| r.response_id == format!("{}-flux", s.component_id));
        let expected_flux =
            response.ok_or_else(|| fail("spectrum lacks component flux response"))?;
        let domain = ResponseDomain::Component {
            component_id: s.component_id.clone(),
        };
        let artifact_volume = artifact
            .volumes
            .iter()
            .find(|v| v.domain == domain)
            .ok_or_else(|| fail("spectrum lacks component volume"))?;
        let expected_volume_cm3 = match artifact_volume.unit {
            VolumeUnit::CubicCentimetre => artifact_volume.value,
            VolumeUnit::CubicMetre => artifact_volume.value * 1.0e6,
        };
        let expected_volume_se_cm3 = match artifact_volume.unit {
            VolumeUnit::CubicCentimetre => artifact_volume.standard_error,
            VolumeUnit::CubicMetre => artifact_volume.standard_error * 1.0e6,
        };
        if (s.volume_cm3 - expected_volume_cm3).abs() > 1.0e-10 * expected_volume_cm3.max(1.0)
            || (s.volume_standard_error_cm3 - expected_volume_se_cm3).abs()
                > 1.0e-10 * expected_volume_se_cm3.max(1.0)
        {
            return Err(fail(
                "spectrum normalization volume differs from transport artifact",
            ));
        }
        if s.particle == "neutron" {
            let flux_tally = artifact
                .tallies
                .iter()
                .find(|t| t.response_id == expected_flux.response_id)
                .ok_or_else(|| fail("spectrum lacks raw component flux tally"))?;
            let bin_sum: f64 = s.mean_cm_per_source_per_bin.iter().sum();
            let scale = flux_tally.mean.abs().max(1.0e-30);
            if activation {
                // The 709 groups cover only the library's energy range, so the sum may fall
                // short of the full-range flux but never exceed it.
                if bin_sum > flux_tally.mean + 1.0e-8 * scale {
                    return Err(fail(
                        "activation neutron spectrum exceeds integrated component flux",
                    ));
                }
            } else if (bin_sum - flux_tally.mean).abs() > 1.0e-8 * scale {
                return Err(fail(
                    "full-range neutron spectrum does not sum to integrated component flux",
                ));
            }
        }
        let volume_m3 = s.volume_cm3 * 1.0e-6;
        let factor = normalized.source_neutron_rate_per_s * 0.01 / volume_m3;
        output.push(NormalizedEnergySpectrum {
            component_id: s.component_id.clone(),
            particle: s.particle.clone(),
            estimator: s.estimator,
            energy_edges_ev: s.energy_edges_ev.clone(),
            mean_per_square_metre_second: s
                .mean_cm_per_source_per_bin
                .iter()
                .map(|x| x * factor)
                .collect(),
            standard_error_per_square_metre_second: s
                .standard_error_cm_per_source_per_bin
                .iter()
                .map(|x| x * factor)
                .collect(),
            normalization_volume_m3: volume_m3,
            volume_standard_error_m3: s.volume_standard_error_cm3 * 1.0e-6,
        });
    }
    let components: BTreeSet<_> = expected_request
        .responses
        .iter()
        .filter_map(|r| {
            if matches!(r.score, ScoreDefinition::Flux)
                && let ResponseDomain::Component { component_id } = &r.domain
            {
                return Some(component_id.as_str());
            }
            None
        })
        .collect();
    let photons_expected = expected_request
        .responses
        .iter()
        .any(|r| matches!(r.score, ScoreDefinition::Heating { .. }));
    let expected_pairs: BTreeSet<_> = components
        .iter()
        .flat_map(|c| {
            let mut v = vec![(*c, "neutron")];
            if photons_expected {
                v.push((*c, "photon"));
            }
            v
        })
        .collect();
    let activation_components: BTreeSet<_> = if expected_request.activation_spectra.is_some() {
        components.clone()
    } else {
        BTreeSet::new()
    };
    if seen_activation != activation_components {
        return Err(fail(
            "spectra sidecar does not contain exactly the requested activation spectra",
        ));
    }
    if seen != expected_pairs {
        return Err(fail(
            "spectra sidecar does not contain exactly the requested component/particle families",
        ));
    }
    Ok(output)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PhysicalUnit {
    NeutronMetresPerSecond,
    NeutronsPerSquareMetreSecond,
    ReactionsPerSecond,
    ReactionsPerCubicMetreSecond,
    ParticlesPerSecond,
    ParticlesPerCubicMetreSecond,
    Watts,
    WattsPerCubicMetre,
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("invalid transport contract: {0}")]
    Invalid(String),
    #[error("transport artifact JSON: {0}")]
    Json(#[from] serde_json::Error),
}

impl TransportArtifact {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TransportError> {
        if bytes.len() > MAX_TRANSPORT_ARTIFACT_BYTES {
            return Err(TransportError::Invalid(
                "artifact exceeds 16 MiB limit".into(),
            ));
        }
        Ok(serde_json::from_slice(bytes)?)
    }
}

pub fn normalize_transport_artifact(
    expected: &TransportRequest,
    artifact: &TransportArtifact,
    scenario: &faris_model::LoadedScenario,
) -> Result<NormalizedTransportResult, TransportError> {
    let fail = |s: &str| TransportError::Invalid(s.into());
    expected
        .validate_against(scenario)
        .map_err(TransportError::Invalid)?;
    let version_pair_matches = (artifact.schema_version == TRANSPORT_ARTIFACT_VERSION
        && expected.schema_version == faris_model::transport::TRANSPORT_REQUEST_VERSION)
        || (artifact.schema_version == TRANSPORT_ARTIFACT_LEGACY_VERSION
            && expected.schema_version == TRANSPORT_REQUEST_LEGACY_VERSION);
    if !version_pair_matches || &artifact.request != expected {
        return Err(fail(
            "artifact schema or echoed request identity/definitions do not match",
        ));
    }
    valid_identity(&artifact.solver)?;
    valid_identity(&artifact.nuclear_data)?;
    if artifact.histories == 0 {
        return Err(fail("histories must be positive"));
    }
    let power_w = expected.fusion_power_mw * 1.0e6;
    let reaction_energy_j = expected.source.energy_per_reaction_ev * ELEMENTARY_CHARGE_C;
    let reaction_rate = power_w / reaction_energy_j;
    let neutron_rate = reaction_rate * expected.source.neutrons_per_reaction;
    if !power_w.is_finite()
        || power_w <= 0.0
        || !reaction_energy_j.is_finite()
        || reaction_energy_j <= 0.0
        || !reaction_rate.is_finite()
        || reaction_rate <= 0.0
        || !neutron_rate.is_finite()
        || neutron_rate <= 0.0
    {
        return Err(fail("source normalization overflowed"));
    }

    let mut volumes = Vec::new();
    for v in &artifact.volumes {
        if !v.value.is_finite()
            || v.value <= 0.0
            || !v.standard_error.is_finite()
            || v.standard_error < 0.0
        {
            return Err(fail("all domain volumes must be finite and positive"));
        }
        if volumes
            .iter()
            .any(|(d, _, _): &(ResponseDomain, f64, f64)| d == &v.domain)
        {
            return Err(fail("duplicate domain volume"));
        }
        let (m3, se_m3) = match v.unit {
            VolumeUnit::CubicMetre => (v.value, v.standard_error),
            VolumeUnit::CubicCentimetre => (v.value * 1.0e-6, v.standard_error * 1.0e-6),
        };
        if !m3.is_finite() || m3 <= 0.0 || !se_m3.is_finite() {
            return Err(fail("volume conversion invalid"));
        }
        volumes.push((v.domain.clone(), m3, se_m3));
    }
    let definitions = &expected.responses;
    if artifact.tallies.len() != definitions.len() {
        return Err(fail(
            "artifact must contain exactly one tally per requested response",
        ));
    }
    if volumes.len() > definitions.len()
        || volumes
            .iter()
            .any(|(domain, _, _)| !definitions.iter().any(|d| &d.domain == domain))
    {
        return Err(fail("artifact contains missing or unused domain volumes"));
    }
    let mut ids = BTreeSet::new();
    let mut results = Vec::with_capacity(definitions.len());
    let mut scales: Vec<f64> = Vec::with_capacity(definitions.len());
    for def in definitions {
        if !ids.insert(&def.id) {
            return Err(fail("duplicate requested response id"));
        }
        let matches: Vec<_> = artifact
            .tallies
            .iter()
            .filter(|t| t.response_id == def.id)
            .collect();
        if matches.len() != 1 {
            return Err(fail("missing or duplicate response tally"));
        }
        let t = matches[0];
        let signed_score = matches!(def.score, ScoreDefinition::Heating { .. });
        if !t.mean.is_finite()
            || (!signed_score && t.mean < 0.0)
            || !t.standard_error.is_finite()
            || t.standard_error < 0.0
        {
            return Err(fail(
                "tally mean must be finite (heating may be signed) and standard error finite/nonnegative",
            ));
        }
        let (integrated_unit, average_unit, integrated_scale) = match (&def.score, t.unit) {
            (
                ScoreDefinition::Flux | ScoreDefinition::FluxAbove { .. },
                RawTallyUnit::CmPerSource,
            ) => (
                PhysicalUnit::NeutronMetresPerSecond,
                PhysicalUnit::NeutronsPerSquareMetreSecond,
                neutron_rate * 0.01,
            ),
            (ScoreDefinition::ReactionRate { .. }, RawTallyUnit::EventsPerSource) => (
                PhysicalUnit::ReactionsPerSecond,
                PhysicalUnit::ReactionsPerCubicMetreSecond,
                neutron_rate,
            ),
            (ScoreDefinition::ParticleProduction { .. }, RawTallyUnit::ParticlesPerSource) => (
                PhysicalUnit::ParticlesPerSecond,
                PhysicalUnit::ParticlesPerCubicMetreSecond,
                neutron_rate,
            ),
            (ScoreDefinition::Heating { .. }, RawTallyUnit::EvPerSource) => (
                PhysicalUnit::Watts,
                PhysicalUnit::WattsPerCubicMetre,
                neutron_rate * ELEMENTARY_CHARGE_C,
            ),
            _ => return Err(fail("raw tally unit is inconsistent with requested score")),
        };
        let (volume, volume_se) = volumes
            .iter()
            .find(|(d, _, _)| d == &def.domain)
            .map(|(_, v, se)| (*v, *se))
            .ok_or_else(|| fail("missing positive volume for response domain"))?;
        // OpenMC flux is volume integrated in cm/source; convert integrated cm to
        // metres, multiply by source/s, then divide by m³. Other supported scores
        // are volume integrated counts or energy and also require domain volume.
        let divisor = volume;
        let integrated_mean = t.mean * integrated_scale;
        let integrated_se = t.standard_error * integrated_scale;
        let mean = integrated_mean / divisor;
        // Geometry and transport estimates use separate random streams, so
        // their standard errors are propagated as independent quantities.
        let se = (math::powi(integrated_se / divisor, 2)
            + math::powi(integrated_mean * volume_se / math::powi(divisor, 2), 2))
        .sqrt();
        if !integrated_scale.is_finite()
            || integrated_scale <= 0.0
            || !integrated_mean.is_finite()
            || !integrated_se.is_finite()
            || !mean.is_finite()
            || !se.is_finite()
        {
            return Err(fail("normalized tally overflowed"));
        }
        scales.push(integrated_scale);
        results.push(NormalizedTally {
            response_id: def.id.clone(),
            domain: def.domain.clone(),
            score: def.score.clone(),
            estimator: t.estimator,
            mean,
            standard_error: se,
            unit: average_unit,
            integrated_mean,
            integrated_standard_error: integrated_se,
            integrated_unit,
            volume_m3: volume,
            volume_standard_error_m3: volume_se,
        });
    }
    let response_covariance = artifact
        .response_covariance
        .as_ref()
        .map(|raw| normalize_response_covariance(raw, &results, &scales))
        .transpose()?;
    Ok(NormalizedTransportResult {
        response_covariance,
        schema_version: "faris-normalized-transport/v0.1".into(),
        scenario_id: expected.scenario_id.clone(),
        scenario_sha256: expected.scenario_sha256.clone(),
        variant_id: expected.variant_id.clone(),
        source: expected.source.clone(),
        solver: artifact.solver.clone(),
        nuclear_data: artifact.nuclear_data.clone(),
        histories: artifact.histories,
        source_reaction_rate_per_s: reaction_rate,
        source_neutron_rate_per_s: neutron_rate,
        results,
    })
}

/// Convert and validate the raw batch-means covariance of scalar responses.
///
/// Entry (i, j) is scaled by the same per-response factors applied to the
/// integrated means (`s_i * s_j`). Every inconsistency rejects the artifact.
fn normalize_response_covariance(
    raw: &RawResponseCovariance,
    results: &[NormalizedTally],
    scales: &[f64],
) -> Result<ResponseCovariance, TransportError> {
    let fail = |s: &str| TransportError::Invalid(format!("response covariance: {s}"));
    if raw.method != RESPONSE_COVARIANCE_METHOD {
        return Err(fail("unsupported estimator method"));
    }
    if raw.batches < 2 {
        return Err(fail("at least 2 batches are required"));
    }
    let hex = &raw.batch_values_sha256;
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || raw.batch_values_file.is_empty()
        || raw.batch_values_file.contains(['/', '\\'])
    {
        return Err(fail("batch-values file name or sha256 is invalid"));
    }
    // Scalar responses are every non-mesh response; the matrix covers exactly those.
    let scalar: Vec<usize> = (0..results.len())
        .filter(|&i| !matches!(results[i].domain, ResponseDomain::Mesh { .. }))
        .collect();
    let n = raw.response_ids.len();
    let mut index = Vec::with_capacity(n);
    for id in &raw.response_ids {
        match scalar.iter().find(|&&i| &results[i].response_id == id) {
            Some(&i) if !index.contains(&i) => index.push(i),
            _ => {
                return Err(fail(
                    "response ids do not match the scalar results one-to-one",
                ));
            }
        }
    }
    if n != scalar.len() || n == 0 {
        return Err(fail(
            "response ids do not match the scalar results one-to-one",
        ));
    }
    if raw.raw_per_source.len() != n * n {
        return Err(fail("matrix must contain n * n entries"));
    }
    if raw.raw_per_source.iter().any(|v| !v.is_finite()) {
        return Err(fail("matrix contains a non-finite entry"));
    }
    let at = |i: usize, j: usize| raw.raw_per_source[i * n + j];
    for i in 0..n {
        for j in (i + 1)..n {
            let (a, b) = (at(i, j), at(j, i));
            if (a - b).abs() > COVARIANCE_SYMMETRY_TOLERANCE * a.abs().max(b.abs()) {
                return Err(fail("matrix is not symmetric"));
            }
        }
    }
    let mut integrated = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..n {
            integrated[i * n + j] = at(i, j) * (scales[index[i]] * scales[index[j]]);
        }
    }
    if integrated.iter().any(|v| !v.is_finite()) {
        return Err(fail("normalized matrix overflowed"));
    }
    for i in 0..n {
        let variance = integrated[i * n + i];
        let expected = math::powi(results[index[i]].integrated_standard_error, 2);
        if variance < 0.0
            || (variance - expected).abs() > COVARIANCE_DIAGONAL_TOLERANCE * variance.max(expected)
        {
            return Err(fail(
                "diagonal differs from the squared integrated standard error",
            ));
        }
    }
    check_positive_semidefinite(&integrated, n).map_err(fail)?;
    Ok(ResponseCovariance {
        method: raw.method.clone(),
        batches: raw.batches,
        response_ids: raw.response_ids.clone(),
        integrated,
    })
}

/// Positive-semidefinite check on the correlation matrix by diagonally pivoted
/// Cholesky (Schur-complement form), which stays stable for the rank-deficient
/// matrices that are normal when responses outnumber batches; an unpivoted
/// factorization amplifies rounding by dividing by near-zero pivots.
/// Zero-variance responses must be uncorrelated with all others. Every
/// remaining diagonal must stay above `-CORRELATION_TOLERANCE`; factorization
/// stops when the largest remaining diagonal is within `CORRELATION_TOLERANCE`
/// of zero, and the remaining block must then be within that tolerance too. A
/// correlation outside `[-1 - tol, 1 + tol]` also rejects.
fn check_positive_semidefinite(cov: &[f64], n: usize) -> Result<(), &'static str> {
    let mut active = Vec::new();
    for i in 0..n {
        if cov[i * n + i] > 0.0 {
            active.push(i);
        } else if (0..n).any(|j| j != i && cov[i * n + j] != 0.0) {
            return Err("zero-variance response has nonzero covariance");
        }
    }
    let m = active.len();
    let mut a = vec![0.0_f64; m * m];
    for (p, &i) in active.iter().enumerate() {
        for (q, &j) in active.iter().enumerate() {
            let r = cov[i * n + j] / (cov[i * n + i] * cov[j * n + j]).sqrt();
            if !r.is_finite() || r.abs() > 1.0 + CORRELATION_TOLERANCE {
                return Err("correlation coefficient is outside [-1, 1]");
            }
            a[p * m + q] = r;
        }
    }
    let mut done = vec![false; m];
    for _ in 0..m {
        let mut pivot: Option<usize> = None;
        for i in (0..m).filter(|&i| !done[i]) {
            if a[i * m + i] < -CORRELATION_TOLERANCE {
                return Err("matrix is not positive semidefinite");
            }
            if pivot.is_none_or(|k| a[i * m + i] > a[k * m + k]) {
                pivot = Some(i);
            }
        }
        let Some(k) = pivot else { break };
        let d = a[k * m + k];
        if d <= CORRELATION_TOLERANCE {
            // Rank exhausted: the remaining Schur complement must vanish.
            let leftover_small = (0..m).filter(|&i| !done[i]).all(|i| {
                (0..m)
                    .filter(|&j| !done[j])
                    .all(|j| a[i * m + j].abs() <= CORRELATION_TOLERANCE)
            });
            return if leftover_small {
                Ok(())
            } else {
                Err("matrix is not positive semidefinite")
            };
        }
        done[k] = true;
        for i in (0..m).filter(|&i| !done[i]) {
            let f = a[i * m + k] / d;
            for j in (0..m).filter(|&j| !done[j]) {
                a[i * m + j] -= f * a[k * m + j];
            }
        }
    }
    Ok(())
}

fn valid_identity(i: &ToolIdentity) -> Result<(), TransportError> {
    if i.name.trim().is_empty() || i.version.trim().is_empty() {
        return Err(TransportError::Invalid(
            "solver and nuclear data identities require name and version".into(),
        ));
    }
    let Some(hex) = i.digest.strip_prefix("sha256:") else {
        return Err(TransportError::Invalid(
            "identity digest must use sha256:<64 lowercase hex>".into(),
        ));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(TransportError::Invalid(
            "identity digest must use sha256:<64 lowercase hex>".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use faris_model::transport::*;

    fn fixture(
        volume: VolumeUnit,
    ) -> (
        TransportRequest,
        TransportArtifact,
        faris_model::LoadedScenario,
    ) {
        let scenario = faris_model::LoadedScenario::from_bytes(include_bytes!(
            "../../../scenarios/arc-inspired/scenario.json"
        ))
        .unwrap();
        let req = TransportRequest {
            schema_version: TRANSPORT_REQUEST_VERSION.into(),
            scenario_id: scenario.scenario.id.clone(),
            scenario_sha256: scenario.source_sha256.clone(),
            variant_id: scenario.scenario.variants[0].id.clone(),
            fusion_power_mw: scenario.scenario.operating_plan.fusion_power_mw,
            activation_spectra: None,
            source: DtSource {
                energy_per_reaction_ev: 17.6e6,
                neutron_energy_ev: 14.1e6,
                neutrons_per_reaction: 1.0,
                distribution_id: "dt-point-isotropic/v1".into(),
            },
            responses: vec![ResponseDefinition {
                id: "heat".into(),
                domain: ResponseDomain::Component {
                    component_id: scenario.scenario.variants[0].layers[1].id.clone(),
                },
                score: ScoreDefinition::Heating {
                    convention: HeatingConvention::Heating,
                    particle_scope: HeatingParticleScope::Total,
                },
            }],
        };
        let art = TransportArtifact {
            schema_version: TRANSPORT_ARTIFACT_VERSION.into(),
            request: req.clone(),
            solver: ToolIdentity {
                name: "OpenMC".into(),
                version: "0.15.3".into(),
                digest: format!("sha256:{}", "1".repeat(64)),
            },
            nuclear_data: ToolIdentity {
                name: "TEST-ONLY-nuclear-data".into(),
                version: "fixture-1".into(),
                digest: format!("sha256:{}", "2".repeat(64)),
            },
            histories: 100,
            response_covariance: None,
            volumes: vec![DomainVolume {
                domain: req.responses[0].domain.clone(),
                value: if volume == VolumeUnit::CubicMetre {
                    2.0
                } else {
                    2.0e6
                },
                standard_error: 0.0,
                unit: volume,
            }],
            tallies: vec![RawTally {
                response_id: "heat".into(),
                estimator: TallyEstimator::Collision,
                unit: RawTallyUnit::EvPerSource,
                mean: 2.0,
                standard_error: 0.1,
            }],
        };
        (req, art, scenario)
    }

    /// A flux-only request and artifact, with one full-energy neutron spectrum and
    /// optionally one 709-group activation spectrum that sums to `activation_sum`.
    fn spectra_fixture(
        activation: bool,
        activation_sum: f64,
    ) -> (
        TransportRequest,
        TransportArtifact,
        NormalizedTransportResult,
        RawTransportSpectra,
    ) {
        let (mut req, mut art, scenario) = fixture(VolumeUnit::CubicMetre);
        let component = scenario.scenario.variants[0].layers[1].id.clone();
        let domain = ResponseDomain::Component {
            component_id: component.clone(),
        };
        req.responses = vec![ResponseDefinition {
            id: format!("{component}-flux"),
            domain,
            score: ScoreDefinition::Flux,
        }];
        if activation {
            req.activation_spectra = Some("fispact-709".into());
        }
        art.request = req.clone();
        art.volumes[0].domain = req.responses[0].domain.clone();
        art.tallies = vec![RawTally {
            response_id: format!("{component}-flux"),
            estimator: TallyEstimator::Tracklength,
            unit: RawTallyUnit::CmPerSource,
            mean: 4.0,
            standard_error: 0.1,
        }];
        let normalized = normalize_transport_artifact(&req, &art, &scenario).unwrap();
        let raw = |structure: Option<&str>, edges: Vec<f64>, total: f64| {
            let bins = edges.len() - 1;
            RawEnergySpectrum {
                component_id: component.clone(),
                particle: "neutron".into(),
                group_structure: structure.map(str::to_string),
                estimator: TallyEstimator::Tracklength,
                unit: "cm_per_source_per_energy_bin".into(),
                energy_edges_ev: edges,
                mean_cm_per_source_per_bin: vec![total / bins as f64; bins],
                standard_error_cm_per_source_per_bin: vec![0.01; bins],
                volume_cm3: 2.0e6,
                volume_standard_error_cm3: 0.0,
            }
        };
        let mut spectra = vec![raw(None, vec![0.0, 1.0e3, 1.0e9], 4.0)];
        if activation {
            let edges: Vec<f64> = (0..=709)
                .map(|i| 1.0e-5 * math::powf(1.0e14_f64, i as f64 / 709.0))
                .collect();
            spectra.push(raw(Some("fispact-709"), edges, activation_sum));
        }
        let sidecar = RawTransportSpectra {
            schema_version: "faris-transport-spectra/v0.1".into(),
            request: req.clone(),
            scenario_sha256: req.scenario_sha256.clone(),
            variant_id: req.variant_id.clone(),
            input_sha256: "e".repeat(64),
            solver: normalized.solver.clone(),
            nuclear_data: normalized.nuclear_data.clone(),
            histories: normalized.histories,
            spectra,
        };
        (req, art, normalized, sidecar)
    }

    #[test]
    fn a_default_request_serializes_without_the_activation_field() {
        let (req, _, _, _) = spectra_fixture(false, 0.0);
        assert!(req.activation_spectra.is_none());
        assert!(
            !serde_json::to_string(&req)
                .unwrap()
                .contains("activation_spectra")
        );
        let opted_in = spectra_fixture(true, 3.0).0;
        assert!(
            serde_json::to_string(&opted_in)
                .unwrap()
                .contains("\"activation_spectra\":\"fispact-709\"")
        );
    }

    #[test]
    fn activation_spectra_are_carried_with_relative_errors_when_requested() {
        let (req, _, normalized, sidecar) = spectra_fixture(true, 3.0);
        let artifact = spectra_fixture(true, 3.0).1;
        let out =
            normalize_spectra(&sidecar, &artifact, &normalized, &req, &"e".repeat(64)).unwrap();
        assert_eq!(out.len(), 2);
        let groups = out.iter().find(|s| s.energy_edges_ev.len() == 710).unwrap();
        assert_eq!(groups.mean_per_square_metre_second.len(), 709);
        assert_eq!(groups.standard_error_per_square_metre_second.len(), 709);
        assert!(groups.mean_per_square_metre_second.iter().all(|v| *v > 0.0));
        assert_eq!(
            out.iter()
                .filter(|s| s.energy_edges_ev.first() == Some(&0.0))
                .count(),
            1
        );
    }

    #[test]
    fn activation_spectra_are_refused_unrequested_missing_or_above_the_total() {
        // Present in the sidecar but not requested.
        let (_, artifact, normalized, mut sidecar) = spectra_fixture(true, 3.0);
        let plain_request = spectra_fixture(false, 0.0).0;
        sidecar.request = plain_request.clone();
        let mut plain_normalized = normalized.clone();
        plain_normalized.solver = sidecar.solver.clone();
        assert!(
            normalize_spectra(
                &sidecar,
                &artifact,
                &plain_normalized,
                &plain_request,
                &"e".repeat(64)
            )
            .is_err()
        );
        // Requested but missing.
        let (req, artifact, normalized, mut sidecar) = spectra_fixture(true, 3.0);
        sidecar.spectra.truncate(1);
        assert!(
            normalize_spectra(&sidecar, &artifact, &normalized, &req, &"e".repeat(64)).is_err()
        );
        // Sum above the integrated flux.
        let (req, artifact, normalized, sidecar) = spectra_fixture(true, 4.5);
        assert!(
            normalize_spectra(&sidecar, &artifact, &normalized, &req, &"e".repeat(64)).is_err()
        );
    }

    // Verifies: NUC-003, SRC-001, PWR-012
    #[test]
    fn power_and_units_scale_exactly_and_cm3_equals_m3() {
        let (r, a, s) = fixture(VolumeUnit::CubicMetre);
        let out = normalize_transport_artifact(&r, &a, &s).unwrap();
        let expected_rr = 525.0e6 / (17.6e6 * ELEMENTARY_CHARGE_C);
        assert!((out.source_reaction_rate_per_s / expected_rr - 1.0).abs() < 1e-14);
        let (r, a, s) = fixture(VolumeUnit::CubicCentimetre);
        let cm = normalize_transport_artifact(&r, &a, &s).unwrap();
        assert_eq!(out.results[0].mean, cm.results[0].mean);
        assert_eq!(out.results[0].standard_error, cm.results[0].standard_error);
    }

    #[test]
    fn heating_response_preserves_signed_collision_energy_balance() {
        let (request, mut artifact, scenario) = fixture(VolumeUnit::CubicMetre);
        artifact.tallies[0].mean = -2.0;
        artifact.tallies[0].standard_error = 0.25;
        let normalized = normalize_transport_artifact(&request, &artifact, &scenario).unwrap();
        assert!(normalized.results[0].integrated_mean < 0.0);
        assert!(normalized.results[0].mean < 0.0);
        assert!(normalized.results[0].integrated_standard_error > 0.0);
    }

    // Verifies: NUC-003, FUEL-004, UNC-001
    #[test]
    fn flux_reaction_particle_production_and_heating_have_distinct_units() {
        let (mut request, mut artifact, scenario) = fixture(VolumeUnit::CubicMetre);
        let component = request.responses[0].domain.clone();
        let mesh = ResponseDomain::Mesh {
            mesh_id: "mesh-z0".into(),
            bin: 7,
        };
        request.responses = vec![
            ResponseDefinition {
                id: "flux".into(),
                domain: ResponseDomain::WholeModel,
                score: ScoreDefinition::Flux,
            },
            ResponseDefinition {
                id: "rate".into(),
                domain: component.clone(),
                score: ScoreDefinition::ReactionRate {
                    reaction: "elastic".into(),
                },
            },
            ResponseDefinition {
                id: "tritium".into(),
                domain: component.clone(),
                score: ScoreDefinition::ParticleProduction {
                    particle: ProducedParticle::Tritium,
                    score: "H3-production".into(),
                },
            },
            ResponseDefinition {
                id: "heat".into(),
                domain: mesh.clone(),
                score: ScoreDefinition::Heating {
                    convention: HeatingConvention::Heating,
                    particle_scope: HeatingParticleScope::Total,
                },
            },
        ];
        artifact.request = request.clone();
        artifact.volumes = vec![
            DomainVolume {
                domain: ResponseDomain::WholeModel,
                value: 2.0,
                standard_error: 0.0,
                unit: VolumeUnit::CubicMetre,
            },
            DomainVolume {
                domain: component,
                value: 2.0,
                standard_error: 0.0,
                unit: VolumeUnit::CubicMetre,
            },
            DomainVolume {
                domain: mesh,
                value: 2.0,
                standard_error: 0.0,
                unit: VolumeUnit::CubicMetre,
            },
        ];
        artifact.tallies = vec![
            RawTally {
                response_id: "flux".into(),
                estimator: TallyEstimator::Tracklength,
                unit: RawTallyUnit::CmPerSource,
                mean: 2.0,
                standard_error: 0.2,
            },
            RawTally {
                response_id: "rate".into(),
                estimator: TallyEstimator::Analog,
                unit: RawTallyUnit::EventsPerSource,
                mean: 3.0,
                standard_error: 0.3,
            },
            RawTally {
                response_id: "tritium".into(),
                estimator: TallyEstimator::Analog,
                unit: RawTallyUnit::ParticlesPerSource,
                mean: 3.5,
                standard_error: 0.35,
            },
            RawTally {
                response_id: "heat".into(),
                estimator: TallyEstimator::Collision,
                unit: RawTallyUnit::EvPerSource,
                mean: 4.0,
                standard_error: 0.4,
            },
        ];
        let out = normalize_transport_artifact(&request, &artifact, &scenario).unwrap();
        let neutron_rate = out.source_reaction_rate_per_s;
        assert_eq!(out.source_neutron_rate_per_s, neutron_rate);
        assert_eq!(
            out.results[0].unit,
            PhysicalUnit::NeutronsPerSquareMetreSecond
        );
        assert!(
            (out.results[0].mean - 2.0 * 0.01 * neutron_rate / 2.0).abs() / neutron_rate < 1e-14
        );
        assert_eq!(
            out.results[1].unit,
            PhysicalUnit::ReactionsPerCubicMetreSecond
        );
        assert!((out.results[1].mean - 3.0 * neutron_rate / 2.0).abs() / neutron_rate < 1e-14);
        assert_eq!(
            out.results[2].unit,
            PhysicalUnit::ParticlesPerCubicMetreSecond
        );
        assert_eq!(
            out.results[2].integrated_unit,
            PhysicalUnit::ParticlesPerSecond
        );
        assert!((out.results[2].integrated_mean / (3.5 * neutron_rate) - 1.0).abs() < 1e-14);
        assert_eq!(out.results[3].unit, PhysicalUnit::WattsPerCubicMetre);
        let expected_heat = 4.0 * neutron_rate * ELEMENTARY_CHARGE_C / 2.0;
        assert!((out.results[3].mean / expected_heat - 1.0).abs() < 1e-14);
        for result in &out.results {
            assert!((result.standard_error / result.mean - 0.1).abs() < 1e-14);
        }
        assert_eq!(out.source, request.source);
        assert_eq!(out.results[1].domain, request.responses[1].domain);
        assert_eq!(
            out.results[1].integrated_unit,
            PhysicalUnit::ReactionsPerSecond
        );
        assert!((out.results[1].integrated_mean / (3.0 * neutron_rate) - 1.0).abs() < 1e-14);
        assert_eq!(
            out.results[0].integrated_unit,
            PhysicalUnit::NeutronMetresPerSecond
        );
        assert!((out.results[0].integrated_mean / (2.0 * neutron_rate * 0.01) - 1.0).abs() < 1e-14);
        assert_eq!(out.results[3].integrated_unit, PhysicalUnit::Watts);
        assert!(
            (out.results[3].integrated_mean / (4.0 * neutron_rate * ELEMENTARY_CHARGE_C) - 1.0)
                .abs()
                < 1e-14
        );
    }

    // Verifies: NUC-004, FUEL-004
    #[test]
    fn validates_volume_domains_units_and_exact_echo() {
        let (r, mut a, s) = fixture(VolumeUnit::CubicMetre);
        a.volumes[0].value = 0.0;
        assert!(normalize_transport_artifact(&r, &a, &s).is_err());
        let (r, mut a, s) = fixture(VolumeUnit::CubicMetre);
        a.tallies[0].unit = RawTallyUnit::EventsPerSource;
        assert!(normalize_transport_artifact(&r, &a, &s).is_err());
        let (r, mut a, s) = fixture(VolumeUnit::CubicMetre);
        a.request.source.neutrons_per_reaction = 2.0;
        assert!(normalize_transport_artifact(&r, &a, &s).is_err());
        let (r, mut a, s) = fixture(VolumeUnit::CubicMetre);
        a.solver.digest = "sha256:solver".into();
        assert!(normalize_transport_artifact(&r, &a, &s).is_err());
        let (r, mut a, s) = fixture(VolumeUnit::CubicMetre);
        a.volumes.push(DomainVolume {
            domain: ResponseDomain::Mesh {
                mesh_id: "unused".into(),
                bin: 0,
            },
            value: 1.0,
            standard_error: 0.0,
            unit: VolumeUnit::CubicMetre,
        });
        assert!(normalize_transport_artifact(&r, &a, &s).is_err());
    }

    #[test]
    fn rejects_energy_conversion_underflow_and_nonfinite_rate() {
        let (mut r, a, s) = fixture(VolumeUnit::CubicMetre);
        r.source.energy_per_reaction_ev = f64::from_bits(1);
        r.source.neutron_energy_ev = f64::from_bits(1);
        let mut a = a;
        a.request = r.clone();
        assert!(normalize_transport_artifact(&r, &a, &s).is_err());
    }

    // Verifies: NUC-004
    #[test]
    fn component_volume_must_be_explicit_not_derived_or_guessed() {
        let (r, mut a, s) = fixture(VolumeUnit::CubicMetre);
        a.volumes.clear();
        assert!(normalize_transport_artifact(&r, &a, &s).is_err());
    }

    /// Scalar flux and reaction rate plus one mesh response, with a valid
    /// covariance whose standard errors are 0.2 and 0.3 and correlation 0.5.
    fn covariance_fixture() -> (
        TransportRequest,
        TransportArtifact,
        faris_model::LoadedScenario,
    ) {
        let (mut request, mut artifact, scenario) = fixture(VolumeUnit::CubicMetre);
        let component = request.responses[0].domain.clone();
        let mesh = ResponseDomain::Mesh {
            mesh_id: "mesh-z0".into(),
            bin: 0,
        };
        request.responses = vec![
            ResponseDefinition {
                id: "flux".into(),
                domain: ResponseDomain::WholeModel,
                score: ScoreDefinition::Flux,
            },
            ResponseDefinition {
                id: "rate".into(),
                domain: component.clone(),
                score: ScoreDefinition::ReactionRate {
                    reaction: "elastic".into(),
                },
            },
            ResponseDefinition {
                id: "mesh-flux".into(),
                domain: mesh.clone(),
                score: ScoreDefinition::Flux,
            },
        ];
        artifact.request = request.clone();
        artifact.volumes = [ResponseDomain::WholeModel, component, mesh]
            .into_iter()
            .map(|domain| DomainVolume {
                domain,
                value: 2.0,
                standard_error: 0.0,
                unit: VolumeUnit::CubicMetre,
            })
            .collect();
        let tally = |id: &str, unit, mean, standard_error| RawTally {
            response_id: id.into(),
            estimator: TallyEstimator::Tracklength,
            unit,
            mean,
            standard_error,
        };
        artifact.tallies = vec![
            tally("flux", RawTallyUnit::CmPerSource, 2.0, 0.2),
            tally("rate", RawTallyUnit::EventsPerSource, 3.0, 0.3),
            tally("mesh-flux", RawTallyUnit::CmPerSource, 1.0, 0.1),
        ];
        artifact.response_covariance = Some(RawResponseCovariance {
            method: RESPONSE_COVARIANCE_METHOD.into(),
            batches: 10,
            response_ids: vec!["flux".into(), "rate".into()],
            raw_per_source: vec![0.04, 0.03, 0.03, 0.09],
            batch_values_file: "transport-batch-values.json".into(),
            batch_values_sha256: "a".repeat(64),
        });
        (request, artifact, scenario)
    }

    fn covariance_error(mutate: impl FnOnce(&mut RawResponseCovariance)) -> String {
        let (request, mut artifact, scenario) = covariance_fixture();
        mutate(artifact.response_covariance.as_mut().unwrap());
        normalize_transport_artifact(&request, &artifact, &scenario)
            .unwrap_err()
            .to_string()
    }

    // Verifies: UNC-011
    #[test]
    fn covariance_is_scaled_like_integrated_means() {
        let (request, artifact, scenario) = covariance_fixture();
        let out = normalize_transport_artifact(&request, &artifact, &scenario).unwrap();
        let rate = out.source_neutron_rate_per_s;
        let (s_flux, s_rate) = (rate * 0.01, rate);
        let cov = out.response_covariance.unwrap();
        assert_eq!(cov.response_ids, ["flux", "rate"]);
        assert_eq!(cov.batches, 10);
        let close = |a: f64, b: f64| (a / b - 1.0).abs() < 1e-14;
        assert!(close(cov.integrated[0], 0.04 * s_flux * s_flux));
        assert!(close(cov.integrated[1], 0.03 * s_flux * s_rate));
        assert_eq!(cov.integrated[2], cov.integrated[1]);
        assert!(close(cov.integrated[3], 0.09 * s_rate * s_rate));
        let flux = &out.results[0];
        assert!(
            (cov.integrated[0] / math::powi(flux.integrated_standard_error, 2) - 1.0).abs() < 1e-12
        );
    }

    #[test]
    fn artifact_without_covariance_gives_none_and_old_json_parses() {
        let (request, mut artifact, scenario) = covariance_fixture();
        artifact.response_covariance = None;
        let text = serde_json::to_string(&artifact).unwrap();
        assert!(!text.contains("response_covariance"));
        let parsed = TransportArtifact::from_bytes(text.as_bytes()).unwrap();
        let out = normalize_transport_artifact(&request, &parsed, &scenario).unwrap();
        assert!(out.response_covariance.is_none());
    }

    #[test]
    fn covariance_rejects_unknown_fields() {
        let (_, artifact, _) = covariance_fixture();
        let mut value = serde_json::to_value(&artifact).unwrap();
        value["response_covariance"]["extra"] = 1.into();
        assert!(TransportArtifact::from_bytes(value.to_string().as_bytes()).is_err());
    }

    #[test]
    fn covariance_rejects_mismatched_ids() {
        assert!(
            covariance_error(|c| c.response_ids[1] = "mesh-flux".into()).contains("one-to-one")
        );
        assert!(covariance_error(|c| c.response_ids[1] = "flux".into()).contains("one-to-one"));
        assert!(
            covariance_error(|c| {
                c.response_ids.pop();
                c.raw_per_source = vec![0.04];
            })
            .contains("one-to-one")
        );
    }

    #[test]
    fn covariance_rejects_wrong_size_nonfinite_and_asymmetric() {
        assert!(covariance_error(|c| c.raw_per_source.push(0.0)).contains("n * n"));
        assert!(covariance_error(|c| c.raw_per_source[1] = f64::NAN).contains("non-finite"));
        assert!(covariance_error(|c| c.raw_per_source[1] = 0.0300001).contains("symmetric"));
        assert!(covariance_error(|c| c.batches = 1).contains("2 batches"));
        assert!(covariance_error(|c| c.method = "other/v1".into()).contains("method"));
        assert!(covariance_error(|c| c.batch_values_sha256 = "xyz".into()).contains("sha256"));
        assert!(
            covariance_error(|c| c.batch_values_file = "../x.json".into()).contains("file name")
        );
    }

    #[test]
    fn covariance_rejects_diagonal_mismatch() {
        assert!(covariance_error(|c| c.raw_per_source[0] = 0.0401).contains("diagonal differs"));
        assert!(covariance_error(|c| c.raw_per_source[3] = 0.0).contains("diagonal differs"));
    }

    #[test]
    fn covariance_rejects_impossible_correlation_and_non_psd() {
        // Correlation 1.1 exceeds one.
        assert!(
            covariance_error(|c| {
                c.raw_per_source[1] = 0.066;
                c.raw_per_source[2] = 0.066;
            })
            .contains("outside [-1, 1]")
        );
        // Perfect correlation is singular but still valid.
        let (request, mut artifact, scenario) = covariance_fixture();
        let c = artifact.response_covariance.as_mut().unwrap();
        c.raw_per_source = vec![0.04, 0.06, 0.06, 0.09];
        assert!(normalize_transport_artifact(&request, &artifact, &scenario).is_ok());
    }

    #[test]
    fn cholesky_rejects_indefinite_three_by_three() {
        // Pairwise correlations of -0.9 cannot all hold for three variables.
        let mut cov = vec![0.0; 9];
        for i in 0..3 {
            for j in 0..3 {
                cov[i * 3 + j] = if i == j { 1.0 } else { -0.9 };
            }
        }
        assert_eq!(
            check_positive_semidefinite(&cov, 3),
            Err("matrix is not positive semidefinite")
        );
        for i in 0..3 {
            for j in 0..3 {
                cov[i * 3 + j] = if i == j { 1.0 } else { 0.5 };
            }
        }
        assert!(check_positive_semidefinite(&cov, 3).is_ok());
    }

    #[test]
    fn zero_variance_response_must_be_uncorrelated() {
        assert!(check_positive_semidefinite(&[0.0, 0.0, 0.0, 1.0], 2).is_ok());
        assert_eq!(
            check_positive_semidefinite(&[0.0, 0.1, 0.1, 1.0], 2),
            Err("zero-variance response has nonzero covariance")
        );
    }

    #[test]
    fn rank_deficient_sample_covariance_is_accepted() {
        // 40 responses from 6 batches: a rank-5 matrix, as real runs produce
        // when responses outnumber batches. Unpivoted Cholesky rejects these.
        let (responses, batches) = (40, 6);
        let mut state = 12345_u64;
        let mut next = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 11) as f64 / (1_u64 << 53) as f64
        };
        let columns: Vec<Vec<f64>> = (0..responses)
            .map(|_| (0..batches).map(|_| next()).collect())
            .collect();
        let means: Vec<f64> = columns
            .iter()
            .map(|c| c.iter().sum::<f64>() / batches as f64)
            .collect();
        let mut cov = vec![0.0; responses * responses];
        for i in 0..responses {
            for j in 0..responses {
                cov[i * responses + j] = (0..batches)
                    .map(|b| (columns[i][b] - means[i]) * (columns[j][b] - means[j]))
                    .sum::<f64>()
                    / (batches - 1) as f64
                    / batches as f64;
            }
        }
        assert!(check_positive_semidefinite(&cov, responses).is_ok());
    }

    /// The magnet's whole-component and three regional fast-flux responses,
    /// each with its own volume (and volume standard error) and a covariance.
    fn region_fixture() -> (
        TransportRequest,
        TransportArtifact,
        faris_model::LoadedScenario,
    ) {
        let (mut request, mut artifact, scenario) = fixture(VolumeUnit::CubicMetre);
        let w = DEFAULT_PORT_SECTOR_HALF_WIDTH_RAD;
        let fast = ScoreDefinition::FluxAbove {
            energy_min_ev: FAST_NEUTRON_ENERGY_MIN_EV,
        };
        let region = |region| ResponseDomain::ComponentRegion {
            component_id: "magnets".into(),
            region,
        };
        let domains = [
            ResponseDomain::Component {
                component_id: "magnets".into(),
            },
            region(ToroidalRegion::InboardHalf),
            region(ToroidalRegion::OutboardHalf {
                excluding_sector_half_width_rad: Some(w),
            }),
            region(ToroidalRegion::PortSector { half_width_rad: w }),
        ];
        let ids = ["whole", "inboard", "outboard", "port"];
        request.responses = ids
            .iter()
            .zip(&domains)
            .map(|(id, domain)| ResponseDefinition {
                id: (*id).into(),
                domain: domain.clone(),
                score: fast.clone(),
            })
            .collect();
        // Volumes in m3 with standard errors only where the port removed some.
        let volumes = [(2.0, 0.0), (0.8, 0.0), (1.1, 0.0), (0.1, 0.002)];
        let means = [4.0, 1.0, 2.0, 8.0];
        let ses = [0.2, 0.05, 0.1, 0.5];
        artifact.request = request.clone();
        artifact.volumes = domains
            .iter()
            .zip(volumes)
            .map(|(domain, (value, se))| DomainVolume {
                domain: domain.clone(),
                value,
                standard_error: se,
                unit: VolumeUnit::CubicMetre,
            })
            .collect();
        artifact.tallies = ids
            .iter()
            .enumerate()
            .map(|(i, id)| RawTally {
                response_id: (*id).into(),
                estimator: TallyEstimator::Tracklength,
                unit: RawTallyUnit::CmPerSource,
                mean: means[i],
                standard_error: ses[i],
            })
            .collect();
        // Whole = inboard + outboard + port per batch, so the covariance is
        // that of a sum: correlated, and the whole's variance is the sum of all
        // entries among the parts.
        let parts = [1usize, 2, 3];
        let mut m = [[0.0_f64; 4]; 4];
        for &i in &parts {
            m[i][i] = ses[i] * ses[i];
        }
        m[1][2] = 0.2 * ses[1] * ses[2];
        m[2][1] = m[1][2];
        for &i in &parts {
            m[0][i] = parts.iter().map(|&j| m[i][j]).sum();
            m[i][0] = m[0][i];
        }
        m[0][0] = parts
            .iter()
            .map(|&i| parts.iter().map(|&j| m[i][j]).sum::<f64>())
            .sum();
        artifact.tallies[0].standard_error = m[0][0].sqrt();
        artifact.response_covariance = Some(RawResponseCovariance {
            method: RESPONSE_COVARIANCE_METHOD.into(),
            batches: 10,
            response_ids: ids.iter().map(|s| (*s).into()).collect(),
            raw_per_source: m.iter().flatten().copied().collect(),
            batch_values_file: "transport-batch-values.json".into(),
            batch_values_sha256: "a".repeat(64),
        });
        (request, artifact, scenario)
    }

    // Verifies: NUC-017, UNC-011
    #[test]
    fn regional_fast_flux_normalizes_per_region_volume_with_volume_error() {
        let (request, artifact, scenario) = region_fixture();
        let out = normalize_transport_artifact(&request, &artifact, &scenario).unwrap();
        let rate = out.source_neutron_rate_per_s;
        let volumes = [2.0, 0.8, 1.1, 0.1];
        for (i, r) in out.results.iter().enumerate() {
            let x = artifact.tallies[i].mean;
            let u = artifact.tallies[i].standard_error;
            let expected_integrated = x * rate * 0.01;
            assert!((r.integrated_mean / expected_integrated - 1.0).abs() < 1e-14);
            assert_eq!(r.volume_m3, volumes[i]);
            assert!((r.mean / (expected_integrated / volumes[i]) - 1.0).abs() < 1e-14);
            assert_eq!(r.unit, PhysicalUnit::NeutronsPerSquareMetreSecond);
            assert_eq!(r.integrated_unit, PhysicalUnit::NeutronMetresPerSecond);
            // Standard error: sampling and volume terms in quadrature.
            let volume_se = artifact.volumes[i].standard_error;
            let sampling = u * rate * 0.01 / volumes[i];
            let from_volume = expected_integrated * volume_se / math::powi(volumes[i], 2);
            let expected_se = math::hypot(sampling, from_volume);
            assert!((r.standard_error / expected_se - 1.0).abs() < 1e-12);
        }
        // Only the port sector has a stochastic volume.
        assert_eq!(out.results[1].volume_standard_error_m3, 0.0);
        assert!(out.results[3].volume_standard_error_m3 > 0.0);
        assert!(out.results[3].standard_error > out.results[3].integrated_standard_error / 0.1);
        // Covariance carried for every scalar response, including regions.
        let cov = out.response_covariance.unwrap();
        assert_eq!(cov.response_ids, ["whole", "inboard", "outboard", "port"]);
        let s = rate * 0.01;
        let raw = artifact.response_covariance.as_ref().unwrap();
        for i in 0..4 {
            for j in 0..4 {
                let want = raw.raw_per_source[i * 4 + j] * s * s;
                assert!((cov.integrated[i * 4 + j] - want).abs() <= 1e-12 * want.abs().max(1e-300));
            }
        }
        // The whole-component variance is the sum over its parts.
        assert!(cov.integrated[0] > cov.integrated[5] + cov.integrated[10] + cov.integrated[15]);
    }

    #[test]
    fn regional_volumes_are_matched_by_domain_not_by_component() {
        let (request, mut artifact, scenario) = region_fixture();
        // A region without its own volume is refused, as is a volume for an
        // unrequested region or a volume reused under a different width.
        artifact.volumes.remove(3);
        assert!(normalize_transport_artifact(&request, &artifact, &scenario).is_err());
        let (request, mut artifact, scenario) = region_fixture();
        artifact.volumes[3].domain = ResponseDomain::ComponentRegion {
            component_id: "magnets".into(),
            region: ToroidalRegion::PortSector {
                half_width_rad: 0.3,
            },
        };
        assert!(normalize_transport_artifact(&request, &artifact, &scenario).is_err());
        // The echoed request must carry the same region definitions.
        let (request, mut artifact, scenario) = region_fixture();
        artifact.request.responses[3].domain = ResponseDomain::ComponentRegion {
            component_id: "magnets".into(),
            region: ToroidalRegion::PortSector {
                half_width_rad: 0.3,
            },
        };
        assert!(normalize_transport_artifact(&request, &artifact, &scenario).is_err());
        // A flux-above tally must report cm per source, not events.
        let (request, mut artifact, scenario) = region_fixture();
        artifact.tallies[1].unit = RawTallyUnit::EventsPerSource;
        assert!(normalize_transport_artifact(&request, &artifact, &scenario).is_err());
    }
}
