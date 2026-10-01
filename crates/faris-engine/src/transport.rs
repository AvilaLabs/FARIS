//! Checked normalization for fixed-source transport artifacts.

use faris_model::transport::{
    ResponseDomain, ScoreDefinition, TRANSPORT_ARTIFACT_VERSION, TransportRequest,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const ELEMENTARY_CHARGE_C: f64 = 1.602_176_634e-19;
pub const MAX_TRANSPORT_ARTIFACT_BYTES: usize = faris_model::transport::MAX_ARTIFACT_BYTES;

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
    if artifact.schema_version != TRANSPORT_ARTIFACT_VERSION || &artifact.request != expected {
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
        if !v.value.is_finite() || v.value <= 0.0 {
            return Err(fail("all domain volumes must be finite and positive"));
        }
        if volumes
            .iter()
            .any(|(d, _): &(ResponseDomain, f64)| d == &v.domain)
        {
            return Err(fail("duplicate domain volume"));
        }
        let m3 = match v.unit {
            VolumeUnit::CubicMetre => v.value,
            VolumeUnit::CubicCentimetre => v.value * 1.0e-6,
        };
        if !m3.is_finite() || m3 <= 0.0 {
            return Err(fail("volume conversion invalid"));
        }
        volumes.push((v.domain.clone(), m3));
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
            .any(|(domain, _)| !definitions.iter().any(|d| &d.domain == domain))
    {
        return Err(fail("artifact contains missing or unused domain volumes"));
    }
    let mut ids = BTreeSet::new();
    let mut results = Vec::with_capacity(definitions.len());
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
        if !t.mean.is_finite()
            || t.mean < 0.0
            || !t.standard_error.is_finite()
            || t.standard_error < 0.0
        {
            return Err(fail(
                "tally mean and standard error must be finite and nonnegative",
            ));
        }
        let (integrated_unit, average_unit, integrated_scale) = match (&def.score, t.unit) {
            (ScoreDefinition::Flux, RawTallyUnit::CmPerSource) => (
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
        let volume = volumes
            .iter()
            .find(|(d, _)| d == &def.domain)
            .map(|(_, v)| *v)
            .ok_or_else(|| fail("missing positive volume for response domain"))?;
        // OpenMC flux is volume integrated in cm/source; convert integrated cm to
        // metres, multiply by source/s, then divide by m³. Other supported scores
        // are volume integrated counts or energy and also require domain volume.
        let divisor = volume;
        let integrated_mean = t.mean * integrated_scale;
        let integrated_se = t.standard_error * integrated_scale;
        let mean = integrated_mean / divisor;
        let se = integrated_se / divisor;
        if !integrated_scale.is_finite()
            || integrated_scale <= 0.0
            || !integrated_mean.is_finite()
            || !integrated_se.is_finite()
            || !mean.is_finite()
            || !se.is_finite()
        {
            return Err(fail("normalized tally overflowed"));
        }
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
        });
    }
    Ok(NormalizedTransportResult {
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
                    convention: HeatingConvention::HeatingLocal,
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
            volumes: vec![DomainVolume {
                domain: req.responses[0].domain.clone(),
                value: if volume == VolumeUnit::CubicMetre {
                    2.0
                } else {
                    2.0e6
                },
                unit: volume,
            }],
            tallies: vec![RawTally {
                response_id: "heat".into(),
                estimator: TallyEstimator::Tracklength,
                unit: RawTallyUnit::EvPerSource,
                mean: 2.0,
                standard_error: 0.1,
            }],
        };
        (req, art, scenario)
    }

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
                },
            },
        ];
        artifact.request = request.clone();
        artifact.volumes = vec![
            DomainVolume {
                domain: ResponseDomain::WholeModel,
                value: 2.0,
                unit: VolumeUnit::CubicMetre,
            },
            DomainVolume {
                domain: component,
                value: 2.0,
                unit: VolumeUnit::CubicMetre,
            },
            DomainVolume {
                domain: mesh,
                value: 2.0,
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
                estimator: TallyEstimator::Tracklength,
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

    #[test]
    fn component_volume_must_be_explicit_not_derived_or_guessed() {
        let (r, mut a, s) = fixture(VolumeUnit::CubicMetre);
        a.volumes.clear();
        assert!(normalize_transport_artifact(&r, &a, &s).is_err());
    }
}
