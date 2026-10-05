//! Strict contracts for fixed-source D-T transport inputs and solver artifacts.
//!
//! Tallies are defined per source neutron. The adapter must preserve these
//! definitions and report the values as supplied by the transport solver.

use serde::{Deserialize, Serialize};

pub const TRANSPORT_REQUEST_VERSION: &str = "faris-transport-request/v0.2";
pub const TRANSPORT_REQUEST_LEGACY_VERSION: &str = "faris-transport-request/v0.1";
pub const TRANSPORT_ARTIFACT_VERSION: &str = "faris-transport-artifact/v0.2";
pub const TRANSPORT_ARTIFACT_LEGACY_VERSION: &str = "faris-transport-artifact/v0.1";
pub const MAX_TRANSPORT_RESPONSES: usize = 8192;
pub const MAX_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;
/// Lower energy bound of the fast-neutron flux screening responses, in eV.
pub const FAST_NEUTRON_ENERGY_MIN_EV: f64 = 1.0e5;
/// Highest lower energy bound a flux-above response may declare, in eV. The
/// worker's energy filters end at 1 GeV, far above any D-T source energy.
pub const MAX_FLUX_ABOVE_ENERGY_EV: f64 = 2.0e7;
/// Default half width of the port toroidal sector (10 degrees), in radians.
pub const DEFAULT_PORT_SECTOR_HALF_WIDTH_RAD: f64 = 0.1745;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TransportRequest {
    pub schema_version: String,
    pub scenario_id: String,
    /// SHA-256 of the exact scenario bytes, including formatting.
    pub scenario_sha256: String,
    pub variant_id: String,
    pub fusion_power_mw: f64,
    pub source: DtSource,
    pub responses: Vec<ResponseDefinition>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DtSource {
    /// Total energy released per D-T reaction, in eV. State the chosen Q value.
    pub energy_per_reaction_ev: f64,
    /// Emitted neutron energy for the primary fixed-source particle, in eV.
    pub neutron_energy_ev: f64,
    /// Neutrons emitted per D-T reaction (normally one, but explicitly stated).
    pub neutrons_per_reaction: f64,
    /// Stable identity for the authored spatial/angular/energy source distribution.
    pub distribution_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ResponseDefinition {
    pub id: String,
    pub domain: ResponseDomain,
    pub score: ScoreDefinition,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResponseDomain {
    WholeModel,
    Component {
        component_id: String,
    },
    /// A named region of a toroidal-shell component, defined by the major
    /// radius and the toroidal angle about the torus axis. Angle zero is the
    /// +x axis, the centre of the outboard penetration.
    ComponentRegion {
        component_id: String,
        region: ToroidalRegion,
    },
    Mesh {
        mesh_id: String,
        bin: u64,
    },
}

// Region widths are validated finite, so equality is reflexive.
impl Eq for ResponseDomain {}

/// Region of a toroidal shell. With major radius `R0`, cylindrical radius `R`
/// about the torus axis and toroidal angle `phi` measured from the +x axis:
/// the inboard half is `R < R0`, the outboard half is `R >= R0`, and the port
/// sector is `R >= R0` with `|phi| <= half width`. Regions are cut at the
/// shell's own cross-section, so they partition the component (inboard,
/// port sector, and outboard excluding the port sector sum to the whole).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToroidalRegion {
    InboardHalf,
    OutboardHalf {
        /// When present, the outboard half without the port sector of this
        /// half width; absent means the whole outboard half.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        excluding_sector_half_width_rad: Option<f64>,
    },
    PortSector {
        half_width_rad: f64,
    },
}

impl Eq for ToroidalRegion {}

impl ToroidalRegion {
    /// Half width of the toroidal sector this region is cut by, if any.
    pub fn sector_half_width_rad(&self) -> Option<f64> {
        match self {
            Self::InboardHalf => None,
            Self::OutboardHalf {
                excluding_sector_half_width_rad,
            } => *excluding_sector_half_width_rad,
            Self::PortSector { half_width_rad } => Some(*half_width_rad),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScoreDefinition {
    Flux,
    /// Neutron flux integrated over energies above `energy_min_ev`.
    FluxAbove {
        energy_min_ev: f64,
    },
    ReactionRate {
        reaction: String,
    },
    ParticleProduction {
        particle: ProducedParticle,
        score: String,
    },
    Heating {
        convention: HeatingConvention,
        #[serde(default = "default_heating_particle_scope")]
        particle_scope: HeatingParticleScope,
    },
}

// Energy bounds are validated finite, so equality is reflexive.
impl Eq for ScoreDefinition {}

/// Which incident particle histories contribute to an OpenMC `heating` tally.
/// `Total` is directly tallied without a particle filter so it retains the
/// estimator's actual standard error and any within-history covariance.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HeatingParticleScope {
    Total,
    Neutron,
    Photon,
    Electron,
    Positron,
}

fn default_heating_particle_scope() -> HeatingParticleScope {
    HeatingParticleScope::Total
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProducedParticle {
    Hydrogen1,
    Deuterium,
    Tritium,
    Helium3,
    Helium4,
}

impl ProducedParticle {
    fn score(self) -> &'static str {
        match self {
            Self::Hydrogen1 => "H1-production",
            Self::Deuterium => "H2-production",
            Self::Tritium => "H3-production",
            Self::Helium3 => "He3-production",
            Self::Helium4 => "He4-production",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HeatingConvention {
    Heating,
    HeatingLocal,
}

impl TransportRequest {
    pub fn validate_against(&self, scenario: &crate::LoadedScenario) -> Result<(), String> {
        let s = &scenario.scenario;
        if self.schema_version != TRANSPORT_REQUEST_VERSION
            && self.schema_version != TRANSPORT_REQUEST_LEGACY_VERSION
        {
            return Err("unsupported transport request schema_version".into());
        }
        if self.scenario_id != s.id || self.scenario_sha256 != scenario.source_sha256 {
            return Err("transport request does not identify these exact scenario bytes".into());
        }
        if !s.variants.iter().any(|v| v.id == self.variant_id) {
            return Err("transport request variant_id is absent from scenario".into());
        }
        if !self.fusion_power_mw.is_finite()
            || self.fusion_power_mw <= 0.0
            || self.fusion_power_mw != s.operating_plan.fusion_power_mw
        {
            return Err("fusion_power_mw must be positive and match scenario exactly".into());
        }
        positive(self.source.energy_per_reaction_ev, "energy_per_reaction_ev")?;
        positive(self.source.neutron_energy_ev, "neutron_energy_ev")?;
        if !self.source.neutrons_per_reaction.is_finite()
            || self.source.neutrons_per_reaction != 1.0
        {
            return Err("a D-T reaction must produce exactly one source neutron".into());
        }
        if self.source.neutron_energy_ev > self.source.energy_per_reaction_ev {
            return Err("source neutron energy cannot exceed total D-T reaction energy".into());
        }
        nonempty(&self.source.distribution_id, "source distribution_id")?;
        if self.responses.is_empty() || self.responses.len() > MAX_TRANSPORT_RESPONSES {
            return Err("responses must contain 1 to 8192 definitions".into());
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut sector_by_component = std::collections::BTreeMap::new();
        for r in &self.responses {
            nonempty(&r.id, "response id")?;
            if !ids.insert(&r.id) {
                return Err("duplicate response id".into());
            }
            match &r.domain {
                ResponseDomain::WholeModel => (),
                ResponseDomain::Component { component_id } => {
                    nonempty(component_id, "component_id")?;
                    if !s.variants.iter().any(|v| {
                        v.id == self.variant_id && v.layers.iter().any(|l| l.id == *component_id)
                    }) {
                        return Err(format!("unknown component_id: {component_id}"));
                    }
                }
                ResponseDomain::ComponentRegion {
                    component_id,
                    region,
                } => {
                    nonempty(component_id, "component_id")?;
                    if !s.variants.iter().any(|v| {
                        v.id == self.variant_id && v.layers.iter().any(|l| l.id == *component_id)
                    }) {
                        return Err(format!("unknown component_id: {component_id}"));
                    }
                    if self.schema_version == TRANSPORT_REQUEST_LEGACY_VERSION {
                        return Err("legacy v0.1 requests cannot declare component regions".into());
                    }
                    if !matches!(r.score, ScoreDefinition::FluxAbove { .. }) {
                        return Err("component regions support only flux-above scores".into());
                    }
                    if let Some(w) = region.sector_half_width_rad() {
                        if !w.is_finite() || w <= 0.0 || w >= std::f64::consts::FRAC_PI_2 {
                            return Err(
                                "region sector half width must be in (0, pi/2) radians".into()
                            );
                        }
                        if sector_by_component
                            .insert(component_id.as_str(), w)
                            .is_some_and(|previous| previous != w)
                        {
                            return Err(format!(
                                "component {component_id} declares two different region sector half widths"
                            ));
                        }
                    }
                }
                ResponseDomain::Mesh { mesh_id, .. } => nonempty(mesh_id, "mesh_id")?,
            }
            match &r.score {
                ScoreDefinition::FluxAbove { energy_min_ev } => {
                    if self.schema_version == TRANSPORT_REQUEST_LEGACY_VERSION {
                        return Err("legacy v0.1 requests cannot declare flux-above scores".into());
                    }
                    if !energy_min_ev.is_finite()
                        || *energy_min_ev <= 0.0
                        || *energy_min_ev >= MAX_FLUX_ABOVE_ENERGY_EV
                    {
                        return Err(
                            "flux-above energy_min_ev must be finite, positive and below 20 MeV"
                                .into(),
                        );
                    }
                    if matches!(r.domain, ResponseDomain::Mesh { .. }) {
                        return Err("flux-above scores are not defined on mesh bins".into());
                    }
                }
                ScoreDefinition::Heating { .. }
                    if self.schema_version == TRANSPORT_REQUEST_LEGACY_VERSION =>
                {
                    return Err("legacy v0.1 requests cannot declare coupled heating".into());
                }
                ScoreDefinition::ReactionRate { reaction } => {
                    if !is_openmc_reaction_score(reaction) {
                        return Err(format!(
                            "unsupported or non-reaction OpenMC score: {reaction}"
                        ));
                    }
                }
                ScoreDefinition::ParticleProduction { particle, score }
                    if particle.score() != score =>
                {
                    return Err("particle and OpenMC particle-production score disagree".into());
                }
                ScoreDefinition::Heating {
                    convention: HeatingConvention::HeatingLocal,
                    ..
                } => return Err(
                    "heating-local is unavailable for this data inventory; request coupled heating"
                        .into(),
                ),
                _ => (),
            }
        }
        Ok(())
    }
}

fn positive(x: f64, name: &str) -> Result<(), String> {
    if x.is_finite() && x > 0.0 {
        Ok(())
    } else {
        Err(format!("{name} must be finite and positive"))
    }
}

/// OpenMC 0.15.3 reaction scores from its standard tally score table, plus
/// positive ENDF MT numbers and level-specific reaction spellings. The
/// separate particle-production and miscellaneous score families must never
/// pass through the reaction-rate normalization path.
fn is_openmc_reaction_score(score: &str) -> bool {
    const NAMED: &[&str] = &[
        "absorption",
        "elastic",
        "fission",
        "scatter",
        "total",
        "(n,2nd)",
        "(n,2n)",
        "(n,3n)",
        "(n,na)",
        "(n,n3a)",
        "(n,2na)",
        "(n,3na)",
        "(n,np)",
        "(n,n2a)",
        "(n,2n2a)",
        "(n,nd)",
        "(n,nt)",
        "(n,n3He)",
        "(n,nd2a)",
        "(n,nt2a)",
        "(n,4n)",
        "(n,2np)",
        "(n,3np)",
        "(n,n2p)",
        "(n,Xn)",
        "(n,Xgamma)",
        "(n,gamma)",
        "(n,p)",
        "(n,d)",
        "(n,t)",
        "(n,3He)",
        "(n,a)",
        "(n,2a)",
        "(n,3a)",
        "(n,2p)",
        "(n,pa)",
        "(n,t2a)",
        "(n,d2a)",
        "(n,pd)",
        "(n,pt)",
        "(n,da)",
        "coherent-scatter",
        "incoherent-scatter",
        "photoelectric",
        "pair-production",
    ];
    if NAMED.contains(&score)
        || score
            .parse::<i32>()
            .is_ok_and(|mt| mt > 0 && mt.to_string() == score)
    {
        return true;
    }
    // OpenMC names level-specific reaction scores such as (n,n3), (n,p2),
    // (n,3He1), and (n,a0); these are reaction-count scores as well.
    for (prefix, max_level) in [
        ("(n,n", 40),
        ("(n,p", 48),
        ("(n,d", 48),
        ("(n,t", 48),
        ("(n,3He", 48),
        ("(n,a", 48),
        ("(n,2n", 15),
    ] {
        if let Some(level) = score.strip_prefix(prefix).and_then(|s| s.strip_suffix(')'))
            && !level.is_empty()
            && level.bytes().all(|b| b.is_ascii_digit())
            && level.parse::<u8>().is_ok_and(|n| n <= max_level)
        {
            return true;
        }
    }
    false
}
fn nonempty(x: &str, name: &str) -> Result<(), String> {
    if x.trim().is_empty() {
        Err(format!("{name} must be nonempty"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const CASE: &[u8] = include_bytes!("../../../scenarios/arc-inspired/scenario.json");
    fn request() -> (TransportRequest, crate::LoadedScenario) {
        let scenario = crate::LoadedScenario::from_bytes(CASE).unwrap();
        let request = TransportRequest {
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
                id: "blanket-heating".into(),
                domain: ResponseDomain::Component {
                    component_id: scenario.scenario.variants[0].layers[1].id.clone(),
                },
                score: ScoreDefinition::Heating {
                    convention: HeatingConvention::HeatingLocal,
                    particle_scope: HeatingParticleScope::Total,
                },
            }],
        };
        (request, scenario)
    }
    #[test]
    fn exact_scenario_binding_and_domains_are_enforced() {
        let (mut r, s) = request();
        assert!(r.validate_against(&s).is_err());
        r.responses[0].score = ScoreDefinition::Heating {
            convention: HeatingConvention::Heating,
            particle_scope: HeatingParticleScope::Total,
        };
        r.validate_against(&s).unwrap();
        r.scenario_sha256.push('0');
        assert!(r.validate_against(&s).is_err());
        let (mut r, s) = request();
        r.responses[0].domain = ResponseDomain::Component {
            component_id: "missing".into(),
        };
        assert!(r.validate_against(&s).is_err());
    }
    #[test]
    fn strict_json_rejects_unknown_contract_fields() {
        let (r, _) = request();
        let mut v = serde_json::to_value(r).unwrap();
        v["unrecorded_units"] = serde_json::json!("MW");
        assert!(serde_json::from_value::<TransportRequest>(v).is_err());
    }

    // Verifies: SRC-003
    #[test]
    fn source_requires_physical_dt_energy_and_single_neutron_yield() {
        let (mut r, s) = request();
        r.source.neutrons_per_reaction = 2.0;
        assert!(r.validate_against(&s).is_err());
        let (mut r, s) = request();
        r.source.neutron_energy_ev = 17.7e6;
        assert!(r.validate_against(&s).is_err());
    }

    #[test]
    fn particle_score_is_distinct_and_consistent_with_particle_identity() {
        let (mut r, s) = request();
        r.responses[0].score = ScoreDefinition::ParticleProduction {
            particle: ProducedParticle::Tritium,
            score: "H3-production".into(),
        };
        r.validate_against(&s).unwrap();
        if let ScoreDefinition::ParticleProduction { score, .. } = &mut r.responses[0].score {
            *score = "elastic".into();
        }
        assert!(r.validate_against(&s).is_err());
    }

    #[test]
    fn reaction_score_rejects_openmc_particle_miscellaneous_and_flux_scores() {
        let (mut r, s) = request();
        for score in [
            "H3-production",
            "nu-fission",
            "heating",
            "heating-local",
            "flux",
            "events",
            "current",
        ] {
            r.responses[0].score = ScoreDefinition::ReactionRate {
                reaction: score.into(),
            };
            assert!(
                r.validate_against(&s).is_err(),
                "accepted non-reaction score {score}"
            );
        }
        for score in ["elastic", "(n,gamma)", "102", "(n,n3)"] {
            r.responses[0].score = ScoreDefinition::ReactionRate {
                reaction: score.into(),
            };
            r.validate_against(&s).unwrap();
        }
    }

    fn region_response(id: &str, region: ToroidalRegion) -> ResponseDefinition {
        ResponseDefinition {
            id: id.into(),
            domain: ResponseDomain::ComponentRegion {
                component_id: "magnets".into(),
                region,
            },
            score: ScoreDefinition::FluxAbove {
                energy_min_ev: FAST_NEUTRON_ENERGY_MIN_EV,
            },
        }
    }

    fn region_request() -> (TransportRequest, crate::LoadedScenario) {
        let (mut r, s) = request();
        r.responses = vec![
            ResponseDefinition {
                id: "magnets-fast-flux".into(),
                domain: ResponseDomain::Component {
                    component_id: "magnets".into(),
                },
                score: ScoreDefinition::FluxAbove {
                    energy_min_ev: FAST_NEUTRON_ENERGY_MIN_EV,
                },
            },
            region_response("magnets-inboard-fast-flux", ToroidalRegion::InboardHalf),
            region_response(
                "magnets-outboard-fast-flux",
                ToroidalRegion::OutboardHalf {
                    excluding_sector_half_width_rad: Some(DEFAULT_PORT_SECTOR_HALF_WIDTH_RAD),
                },
            ),
            region_response(
                "magnets-port-sector-fast-flux",
                ToroidalRegion::PortSector {
                    half_width_rad: DEFAULT_PORT_SECTOR_HALF_WIDTH_RAD,
                },
            ),
        ];
        (r, s)
    }

    #[test]
    fn region_and_flux_above_responses_round_trip_and_validate() {
        let (r, s) = region_request();
        r.validate_against(&s).unwrap();
        let text = serde_json::to_string(&r).unwrap();
        let back: TransportRequest = serde_json::from_str(&text).unwrap();
        assert_eq!(back, r);
        assert!(text.contains(r#""kind":"component_region""#));
        assert!(text.contains(r#""kind":"flux_above""#));
        assert!(text.contains(r#""kind":"inboard_half""#));
        // The whole outboard half carries no exclusion field at all.
        let whole = serde_json::to_string(&ToroidalRegion::OutboardHalf {
            excluding_sector_half_width_rad: None,
        })
        .unwrap();
        assert_eq!(whole, r#"{"kind":"outboard_half"}"#);
        let parsed: ToroidalRegion = serde_json::from_str(&whole).unwrap();
        assert_eq!(parsed.sector_half_width_rad(), None);
    }

    #[test]
    fn region_contract_is_strict() {
        for bad in [r#"{"kind":"port_sector"}"#, r#"{"kind":"middle_third"}"#] {
            assert!(
                serde_json::from_str::<ToroidalRegion>(bad).is_err(),
                "{bad}"
            );
        }
        assert!(serde_json::from_str::<ScoreDefinition>(r#"{"kind":"flux_above"}"#).is_err());
        assert!(
            serde_json::from_str::<ScoreDefinition>(
                r#"{"kind":"flux_above","energy_min_ev":1e5,"extra":0}"#
            )
            .is_err()
        );
    }

    #[test]
    fn existing_domains_and_scores_still_parse_unchanged() {
        let domain: ResponseDomain =
            serde_json::from_str(r#"{"kind":"component","component_id":"blanket"}"#).unwrap();
        assert_eq!(
            domain,
            ResponseDomain::Component {
                component_id: "blanket".into()
            }
        );
        let score: ScoreDefinition = serde_json::from_str(r#"{"kind":"flux"}"#).unwrap();
        assert_eq!(score, ScoreDefinition::Flux);
    }

    #[test]
    fn region_validation_rejects_unsound_definitions() {
        let (r0, s) = region_request();
        let mutate = |f: &dyn Fn(&mut TransportRequest)| {
            let mut r = r0.clone();
            f(&mut r);
            r.validate_against(&s)
        };
        assert!(mutate(&|r| r.schema_version = TRANSPORT_REQUEST_LEGACY_VERSION.into()).is_err());
        for w in [
            0.0,
            -0.1,
            f64::NAN,
            f64::INFINITY,
            std::f64::consts::FRAC_PI_2,
            2.0,
        ] {
            assert!(
                mutate(&|r| {
                    r.responses[3].domain = ResponseDomain::ComponentRegion {
                        component_id: "magnets".into(),
                        region: ToroidalRegion::PortSector { half_width_rad: w },
                    }
                })
                .is_err(),
                "accepted half width {w}"
            );
        }
        // One sector per component: a different exclusion width is refused.
        assert!(
            mutate(&|r| {
                r.responses[2].domain = ResponseDomain::ComponentRegion {
                    component_id: "magnets".into(),
                    region: ToroidalRegion::OutboardHalf {
                        excluding_sector_half_width_rad: Some(0.3),
                    },
                }
            })
            .is_err()
        );
        assert!(
            mutate(&|r| {
                r.responses[1].domain = ResponseDomain::ComponentRegion {
                    component_id: "missing".into(),
                    region: ToroidalRegion::InboardHalf,
                }
            })
            .is_err()
        );
        // Regions are only defined for the fast-flux score.
        assert!(mutate(&|r| r.responses[1].score = ScoreDefinition::Flux).is_err());
        for e in [0.0, -1.0, f64::NAN, 2.0e7, 1.0e9] {
            assert!(
                mutate(&|r| r.responses[0].score = ScoreDefinition::FluxAbove { energy_min_ev: e })
                    .is_err(),
                "accepted energy {e}"
            );
        }
        assert!(
            mutate(&|r| {
                r.responses[0].domain = ResponseDomain::Mesh {
                    mesh_id: "m".into(),
                    bin: 0,
                }
            })
            .is_err()
        );
    }

    #[test]
    fn region_domains_with_different_widths_are_distinct_volume_keys() {
        let a = ResponseDomain::ComponentRegion {
            component_id: "magnets".into(),
            region: ToroidalRegion::PortSector {
                half_width_rad: 0.1,
            },
        };
        let b = ResponseDomain::ComponentRegion {
            component_id: "magnets".into(),
            region: ToroidalRegion::PortSector {
                half_width_rad: 0.2,
            },
        };
        assert_ne!(a, b);
        assert_eq!(a, a.clone());
    }
}
