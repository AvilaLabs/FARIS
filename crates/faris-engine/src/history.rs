//! Deterministic, event-aware operating-history ledger.

use crate::jobs::Cancellation;
use crate::transport::{NormalizedTally, NormalizedTransportResult, PhysicalUnit};
use faris_model::history::{ComponentClass, OperatingHistoryAssumptions, ServiceLimit};
use faris_model::transport::{
    HeatingConvention, HeatingParticleScope, ProducedParticle, ResponseDomain, ScoreDefinition,
    ToroidalRegion,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

pub const TRITIUM_HALF_LIFE_YEARS: f64 = 12.32;
pub const JULIAN_YEAR_SECONDS: f64 = 365.25 * 86_400.0;
pub const TRITIUM_MOLAR_MASS_KG_PER_MOL: f64 = 3.016_049_277_9e-3;
pub const AVOGADRO_CONSTANT_PER_MOL: f64 = 6.022_140_76e23;
pub const ELEMENTARY_CHARGE_J_PER_EV: f64 = 1.602_176_634e-19;
pub const MASS_BALANCE_RELATIVE_TOLERANCE: f64 = 1e-10;
/// Service-limit metric tracking the energy-integrated component-average flux.
pub const ENERGY_INTEGRATED_FLUX_METRIC: &str = "energy_integrated_component_average_neutron_flux";
/// Service-limit metric tracking a fast-neutron flux averaged over a named
/// component region (or the whole component) from a flux-above response.
pub const FAST_FLUX_REGION_METRIC: &str = "fast_neutron_flux_region_average";
// The input horizon/step pair is separately bounded to <=4,000,000 nominal
// segments. Event-driven transitions can subdivide those intervals. A reviewed
// 6,000,000 runtime-segment ceiling bounds that additional work.
const MAX_HISTORY_SEGMENTS: usize = 6_000_000;
const MAX_HISTORY_EVENTS: usize = 20_000;
const MAX_HISTORY_PRODUCTION_INTERVALS: usize = 100_000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ScalarRate {
    pub mean: f64,
    pub standard_error: Option<f64>,
    pub unit: String,
    pub response_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TransportDrivingRates {
    pub reference_fusion_power_mw: f64,
    pub fusion_reaction_rate_per_s: f64,
    pub neutron_source_rate_per_s: f64,
    pub total_reaction_energy_ev: f64,
    pub primary_neutron_energy_ev: f64,
    /// H3 product particles per source neutron scored only in the breeder.
    pub breeder_h3_per_source_neutron: ScalarRate,
    /// Component-average, energy-integrated neutron flux at reference power.
    pub component_average_flux_n_m2_s: BTreeMap<String, ScalarRate>,
    /// Region-average fast-neutron flux responses by response ID, in
    /// neutrons/m²/s at reference power; absent in records made before regional
    /// fast-flux responses existed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub region_flux_n_m2_s: BTreeMap<String, RegionFluxRate>,
    /// Whole-model integrated nuclear heating power from the declared total-particle
    /// heating response (including its neutron, photon, electron, and positron scores).
    /// None means no total nuclear-heat or net-electricity result is available.
    #[serde(alias = "neutron_deposited_heat_w")]
    pub transport_deposited_heat_w: Option<ScalarRate>,
    pub scenario_sha256: String,
    pub transport_artifact_sha256: String,
    pub solver_digest: String,
    pub nuclear_data_digest: String,
    /// Monte Carlo covariance between the rates above, from batch-resolved
    /// transport tallies. None for records without it; correlated sampling is
    /// then not possible and must fail closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub covariance: Option<DrivingCovariance>,
}

/// A fast-neutron flux averaged over one named region of a component, or over
/// the whole component when `region` is absent. The local peak inside the
/// region is not resolved.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RegionFluxRate {
    pub component_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<ToroidalRegion>,
    /// Lower neutron energy bound of the flux, in eV.
    pub energy_min_ev: f64,
    pub rate: ScalarRate,
}

/// Sampling covariance between the driving rates, in the rates' own units.
///
/// Matrix order is fixed: the breeder H3 rate, each component flux in
/// component-ID order, each region flux in response-ID order, then the
/// whole-model heating power when present.
/// `monte_carlo` is the batch-means covariance of the transport tallies scaled
/// from integrated units to the driving units (H3 per source neutron: divide by
/// the source neutron rate; component flux: divide by the component volume;
/// heating: unchanged).
///
/// A component flux standard error in a normalized record also contains the
/// uncertainty of the stochastic volume estimate, which is independent of the
/// tallies (separate random streams). That part is kept out of the matrix in
/// `volume_variance` (same order as `rate_ids`, zero where a rate has no
/// volume term) and is sampled independently. For every rate,
/// `monte_carlo[i][i] + volume_variance[i]` equals `standard_error^2`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DrivingCovariance {
    /// Estimator identity copied from the transport record's covariance.
    pub method: String,
    pub batches: u32,
    pub rate_ids: Vec<String>,
    /// Row-major n x n Monte Carlo covariance in driving units.
    pub monte_carlo: Vec<f64>,
    /// Independent volume-estimate variance per rate, in driving units squared.
    pub volume_variance: Vec<f64>,
}

/// Relative tolerance for covariance diagonals against reported standard errors.
const COVARIANCE_VARIANCE_RTOL: f64 = 1e-6;
/// Symmetry tolerance relative to the geometric mean of the two variances.
const COVARIANCE_SYMMETRY_RTOL: f64 = 1e-9;
/// Pivots of the correlation matrix at or below this are rank deficiency.
const COVARIANCE_RANK_TOL: f64 = 1e-9;
/// Residual Schur-complement entries beyond this mean the matrix is not PSD.
const COVARIANCE_PSD_TOL: f64 = 1e-8;

/// Factor a symmetric positive semi-definite matrix `a` (row-major n x n) as
/// `a = L L^T` and return `L` (row-major n x n, trailing columns zero for a
/// rank-deficient matrix).
///
/// Pivoted Cholesky on the correlation matrix, so the tolerances do not depend
/// on the units of each variable: at each step the largest remaining diagonal
/// is the pivot; elimination stops when it is at or below 1e-9, and the matrix
/// is rejected as not positive semi-definite if any residual entry then
/// exceeds 1e-8 in magnitude. A variable with zero variance must have zero
/// covariance with every other variable.
pub fn factor_covariance(a: &[f64], n: usize) -> Result<Vec<f64>, String> {
    if a.len() != n * n {
        return Err("covariance matrix must be square".into());
    }
    if a.iter().any(|x| !x.is_finite()) {
        return Err("covariance entries must be finite".into());
    }
    let mut sd = Vec::with_capacity(n);
    for i in 0..n {
        let d = a[i * n + i];
        if d < 0.0 {
            return Err("covariance has a negative variance".into());
        }
        sd.push(d.sqrt());
    }
    let mut s = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..n {
            let (x, y) = (a[i * n + j], a[j * n + i]);
            if (x - y).abs() > COVARIANCE_SYMMETRY_RTOL * sd[i] * sd[j] + f64::MIN_POSITIVE {
                return Err("covariance matrix is not symmetric".into());
            }
            if sd[i] > 0.0 && sd[j] > 0.0 {
                s[i * n + j] = x / (sd[i] * sd[j]);
            } else if x != 0.0 {
                return Err("a zero-variance rate has nonzero covariance".into());
            }
        }
    }
    let mut l = vec![0.0; n * n];
    let mut used = vec![false; n];
    for k in 0..n {
        let Some(p) = (0..n)
            .filter(|i| !used[*i])
            .max_by(|a, b| s[a * n + a].total_cmp(&s[b * n + b]))
        else {
            break;
        };
        let pivot = s[p * n + p];
        if pivot <= COVARIANCE_RANK_TOL {
            break;
        }
        let root = pivot.sqrt();
        let column: Vec<f64> = (0..n).map(|i| s[i * n + p] / root).collect();
        for i in 0..n {
            for j in 0..n {
                s[i * n + j] -= column[i] * column[j];
            }
        }
        used[p] = true;
        for i in 0..n {
            s[p * n + i] = 0.0;
            s[i * n + p] = 0.0;
            l[i * n + k] = column[i] * sd[i];
        }
    }
    if s.iter().any(|x| x.abs() > COVARIANCE_PSD_TOL) {
        return Err("covariance matrix is not positive semi-definite".into());
    }
    Ok(l)
}

impl DrivingCovariance {
    pub fn dimension(&self) -> usize {
        self.rate_ids.len()
    }

    fn validate(&self, rates: &TransportDrivingRates) -> Result<(), String> {
        let entries = rates.covariance_entries();
        let n = entries.len();
        if self.rate_ids.len() != n
            || entries
                .iter()
                .zip(&self.rate_ids)
                .any(|(entry, id)| entry.0 != id)
        {
            return Err("covariance rate IDs must list the breeder rate, each component flux in component order, each region flux in response order, then heating".into());
        }
        if self.method.trim().is_empty() || self.batches < 2 {
            return Err("covariance needs a method identity and at least two batches".into());
        }
        if self.monte_carlo.len() != n * n || self.volume_variance.len() != n {
            return Err("covariance dimensions do not match the rate list".into());
        }
        if self
            .volume_variance
            .iter()
            .any(|v| !v.is_finite() || *v < 0.0)
        {
            return Err("volume variance terms must be finite and nonnegative".into());
        }
        for (i, (_, rate)) in entries.iter().enumerate() {
            let se = rate
                .standard_error
                .ok_or("a rate with a covariance must carry its standard error")?;
            let total = self.monte_carlo[i * n + i] + self.volume_variance[i];
            if (total - se * se).abs() > COVARIANCE_VARIANCE_RTOL * se * se + f64::MIN_POSITIVE {
                return Err(format!(
                    "covariance variance for {} does not equal its standard error squared",
                    rate.response_id
                ));
            }
        }
        factor_covariance(&self.monte_carlo, n)?;
        Ok(())
    }
}

impl TransportDrivingRates {
    /// Rates in covariance matrix order, with their response IDs.
    pub fn covariance_entries(&self) -> Vec<(&str, &ScalarRate)> {
        let mut entries = vec![(
            self.breeder_h3_per_source_neutron.response_id.as_str(),
            &self.breeder_h3_per_source_neutron,
        )];
        entries.extend(
            self.component_average_flux_n_m2_s
                .values()
                .map(|r| (r.response_id.as_str(), r)),
        );
        entries.extend(
            self.region_flux_n_m2_s
                .values()
                .map(|r| (r.rate.response_id.as_str(), &r.rate)),
        );
        if let Some(h) = &self.transport_deposited_heat_w {
            entries.push((h.response_id.as_str(), h));
        }
        entries
    }
}

impl TransportDrivingRates {
    /// Bind only the designated breeder product tally, component-average fluxes,
    /// and the whole-model total heating response from one normalized transport run.
    pub fn from_normalized(
        normalized: &NormalizedTransportResult,
        reference_fusion_power_mw: f64,
        transport_artifact_sha256: &str,
    ) -> Result<Self, String> {
        let breeder = normalized
            .results
            .iter()
            .find(|r| r.response_id == "blanket-tritium")
            .ok_or("normalized transport is missing blanket-tritium")?;
        if breeder.domain
            != (ResponseDomain::Component {
                component_id: "blanket".into(),
            })
            || breeder.score
                != (ScoreDefinition::ParticleProduction {
                    particle: ProducedParticle::Tritium,
                    score: "H3-production".into(),
                })
            || breeder.integrated_unit != PhysicalUnit::ParticlesPerSecond
        {
            return Err("blanket-tritium must be the breeder component's H3-production response in particles/s".into());
        }
        if !normalized.source_neutron_rate_per_s.is_finite()
            || normalized.source_neutron_rate_per_s <= 0.0
        {
            return Err("normalized source neutron rate must be finite and positive".into());
        }
        let mut flux = BTreeMap::new();
        for response in &normalized.results {
            if let ResponseDomain::Component { component_id } = &response.domain
                && response.score == ScoreDefinition::Flux
            {
                if response.unit != PhysicalUnit::NeutronsPerSquareMetreSecond {
                    return Err(format!(
                        "component flux {} has an unexpected normalized unit",
                        response.response_id
                    ));
                }
                if flux
                    .insert(
                        component_id.clone(),
                        ScalarRate {
                            mean: response.mean,
                            standard_error: Some(response.standard_error),
                            unit: "neutrons/m²/s".into(),
                            response_id: response.response_id.clone(),
                        },
                    )
                    .is_some()
                {
                    return Err(format!(
                        "duplicate component flux response for {component_id}"
                    ));
                }
            }
        }
        let mut region_flux = BTreeMap::new();
        for response in &normalized.results {
            let ScoreDefinition::FluxAbove { energy_min_ev } = response.score else {
                continue;
            };
            let (component_id, region) = match &response.domain {
                ResponseDomain::Component { component_id } => (component_id, None),
                ResponseDomain::ComponentRegion {
                    component_id,
                    region,
                } => (component_id, Some(region.clone())),
                _ => continue,
            };
            if response.unit != PhysicalUnit::NeutronsPerSquareMetreSecond {
                return Err(format!(
                    "fast flux {} has an unexpected normalized unit",
                    response.response_id
                ));
            }
            region_flux.insert(
                response.response_id.clone(),
                RegionFluxRate {
                    component_id: component_id.clone(),
                    region,
                    energy_min_ev,
                    rate: ScalarRate {
                        mean: response.mean,
                        standard_error: Some(response.standard_error),
                        unit: "neutrons/m²/s".into(),
                        response_id: response.response_id.clone(),
                    },
                },
            );
        }
        let heat = normalized
            .results
            .iter()
            .find(|r| r.response_id == "heating-total-whole-model");
        let transport_deposited_heat_w = match heat {
            Some(r) if r.domain == ResponseDomain::WholeModel
                && r.score == (ScoreDefinition::Heating { convention: HeatingConvention::Heating, particle_scope: HeatingParticleScope::Total })
                && r.integrated_unit == PhysicalUnit::Watts => Some(ScalarRate {
                    mean: r.integrated_mean,
                    standard_error: Some(r.integrated_standard_error),
                    unit: "W".into(),
                    response_id: r.response_id.clone(),
                }),
            Some(_) => return Err("heating-total-whole-model must be a whole-model total-particle heating tally integrated in W".into()),
            None => None,
        };
        let result = Self {
            reference_fusion_power_mw,
            fusion_reaction_rate_per_s: normalized.source_reaction_rate_per_s,
            neutron_source_rate_per_s: normalized.source_neutron_rate_per_s,
            total_reaction_energy_ev: normalized.source.energy_per_reaction_ev,
            primary_neutron_energy_ev: normalized.source.neutron_energy_ev,
            breeder_h3_per_source_neutron: ScalarRate {
                mean: breeder.integrated_mean / normalized.source_neutron_rate_per_s,
                standard_error: Some(
                    breeder.integrated_standard_error / normalized.source_neutron_rate_per_s,
                ),
                unit: "particles/source_neutron".into(),
                response_id: breeder.response_id.clone(),
            },
            component_average_flux_n_m2_s: flux,
            region_flux_n_m2_s: region_flux,
            transport_deposited_heat_w,
            scenario_sha256: normalized.scenario_sha256.clone(),
            transport_artifact_sha256: transport_artifact_sha256.into(),
            solver_digest: normalized.solver.digest.clone(),
            nuclear_data_digest: normalized.nuclear_data.digest.clone(),
            covariance: None,
        };
        let covariance = Self::driving_covariance(normalized, &result, breeder, heat)?;
        let result = Self {
            covariance,
            ..result
        };
        result.validate()?;
        Ok(result)
    }

    /// Select and scale the transport covariance into driving units. None when
    /// the record has no covariance.
    fn driving_covariance(
        normalized: &NormalizedTransportResult,
        rates: &Self,
        breeder: &NormalizedTally,
        heat: Option<&NormalizedTally>,
    ) -> Result<Option<DrivingCovariance>, String> {
        let Some(rc) = &normalized.response_covariance else {
            return Ok(None);
        };
        let m = rc.response_ids.len();
        if m == 0 || rc.integrated.len() != m * m {
            return Err("transport response covariance has inconsistent dimensions".into());
        }
        // (tally, scale from integrated to driving units, independent volume variance)
        let mut picks: Vec<(&NormalizedTally, f64, f64)> =
            vec![(breeder, 1.0 / normalized.source_neutron_rate_per_s, 0.0)];
        for rate in rates.component_average_flux_n_m2_s.values() {
            let tally = normalized
                .results
                .iter()
                .find(|r| r.response_id == rate.response_id)
                .ok_or("flux response vanished from normalized transport")?;
            // mean = integrated_mean / volume; the stored standard error adds
            // (mean * volume_se / volume)^2 for the independent volume estimate.
            let volume_variance =
                (tally.mean * tally.volume_standard_error_m3 / tally.volume_m3).powi(2);
            picks.push((tally, 1.0 / tally.volume_m3, volume_variance));
        }
        for rate in rates.region_flux_n_m2_s.values() {
            let tally = normalized
                .results
                .iter()
                .find(|r| r.response_id == rate.rate.response_id)
                .ok_or("region flux response vanished from normalized transport")?;
            let volume_variance =
                (tally.mean * tally.volume_standard_error_m3 / tally.volume_m3).powi(2);
            picks.push((tally, 1.0 / tally.volume_m3, volume_variance));
        }
        if let Some(h) = heat {
            picks.push((h, 1.0, 0.0));
        }
        let n = picks.len();
        let mut index = Vec::with_capacity(n);
        for (tally, _, _) in &picks {
            let position = rc
                .response_ids
                .iter()
                .position(|id| *id == tally.response_id)
                .ok_or_else(|| {
                    format!(
                        "transport covariance has no entry for driving response {}",
                        tally.response_id
                    )
                })?;
            index.push(position);
        }
        let mut monte_carlo = vec![0.0; n * n];
        for i in 0..n {
            for j in 0..n {
                monte_carlo[i * n + j] =
                    rc.integrated[index[i] * m + index[j]] * picks[i].1 * picks[j].1;
            }
            let expected = (picks[i].0.integrated_standard_error * picks[i].1).powi(2);
            if (monte_carlo[i * n + i] - expected).abs()
                > COVARIANCE_VARIANCE_RTOL * expected + f64::MIN_POSITIVE
            {
                return Err(format!(
                    "covariance diagonal for {} differs from its tally standard error",
                    picks[i].0.response_id
                ));
            }
        }
        Ok(Some(DrivingCovariance {
            method: rc.method.clone(),
            batches: rc.batches,
            rate_ids: picks.iter().map(|p| p.0.response_id.clone()).collect(),
            monte_carlo,
            volume_variance: picks.iter().map(|p| p.2).collect(),
        }))
    }

    pub fn validate(&self) -> Result<(), String> {
        let positive = |x: f64| x.is_finite() && x > 0.0;
        for (x, name) in [
            (self.reference_fusion_power_mw, "reference_fusion_power_mw"),
            (
                self.fusion_reaction_rate_per_s,
                "fusion_reaction_rate_per_s",
            ),
            (self.neutron_source_rate_per_s, "neutron_source_rate_per_s"),
            (self.total_reaction_energy_ev, "total_reaction_energy_ev"),
            (self.primary_neutron_energy_ev, "primary_neutron_energy_ev"),
        ] {
            if !positive(x) {
                return Err(format!("{name} must be finite and positive"));
            }
        }
        let source_rel = (self.fusion_reaction_rate_per_s - self.neutron_source_rate_per_s).abs()
            / self.fusion_reaction_rate_per_s;
        if source_rel > 1e-10 {
            return Err("history requires one primary neutron per D-T reaction".into());
        }
        if self.primary_neutron_energy_ev >= self.total_reaction_energy_ev {
            return Err(
                "primary neutron energy must be below the total D-T reaction energy".into(),
            );
        }
        let q_rate = self.fusion_reaction_rate_per_s
            * self.total_reaction_energy_ev
            * ELEMENTARY_CHARGE_J_PER_EV;
        let declared_power = self.reference_fusion_power_mw * 1e6;
        if !q_rate.is_finite() || (q_rate - declared_power).abs() / declared_power > 1e-8 {
            return Err(
                "reaction-rate, D-T energy convention, and reference fusion power are inconsistent"
                    .into(),
            );
        }
        check_rate(
            &self.breeder_h3_per_source_neutron,
            "particles/source_neutron",
            false,
        )?;
        for (component, rate) in &self.component_average_flux_n_m2_s {
            if component.trim().is_empty() {
                return Err("flux response component ID is empty".into());
            }
            // A finite zero tally is valid for a sampled component with no
            // tracks; this is still only a sampled estimate and conveys no
            // upper bound on the true flux.
            check_rate(rate, "neutrons/m²/s", false)?;
        }
        for (id, region) in &self.region_flux_n_m2_s {
            if region.component_id.trim().is_empty()
                || *id != region.rate.response_id
                || !region.energy_min_ev.is_finite()
                || region.energy_min_ev <= 0.0
            {
                return Err(format!("region flux {id} is not a valid fast-flux rate"));
            }
            check_rate(&region.rate, "neutrons/m²/s", false)?;
        }
        if let Some(rate) = &self.transport_deposited_heat_w {
            check_rate(rate, "W", true)?;
        }
        validate_hash(&self.scenario_sha256, false, "scenario_sha256")?;
        validate_hash(
            &self.transport_artifact_sha256,
            false,
            "transport_artifact_sha256",
        )?;
        validate_hash(&self.solver_digest, true, "solver_digest")?;
        validate_hash(&self.nuclear_data_digest, true, "nuclear_data_digest")?;
        if let Some(covariance) = &self.covariance {
            covariance.validate(self)?;
        }
        Ok(())
    }
}

fn validate_hash(value: &str, prefixed: bool, name: &str) -> Result<(), String> {
    let hex = if prefixed {
        value.strip_prefix("sha256:")
    } else {
        Some(value)
    }
    .ok_or_else(|| format!("{name} must be a SHA-256 identity"))?;
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{name} must contain a 64-hex SHA-256 digest"));
    }
    Ok(())
}

fn check_rate(rate: &ScalarRate, unit: &str, positive: bool) -> Result<(), String> {
    if rate.response_id.trim().is_empty() || rate.unit != unit {
        return Err(format!("response identity/unit mismatch; expected {unit}"));
    }
    if !rate.mean.is_finite()
        || if positive {
            rate.mean <= 0.0
        } else {
            rate.mean < 0.0
        }
    {
        return Err("driving rate mean must be finite and nonnegative".into());
    }
    if let Some(se) = rate.standard_error
        && (!se.is_finite() || se < 0.0)
    {
        return Err("driving rate standard error must be finite and nonnegative".into());
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HistoryOutcome {
    HorizonCompleted,
    FuelLimitedAtHorizon,
    PermanentComponentLimit,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Imported,
    ProcessingReleased,
    OperationStarted,
    OperationStopped,
    FuelUnavailable,
    FuelAvailable,
    PlannedOutageStarted,
    PlannedOutageEnded,
    ServiceLimitReached,
    ReplacementStarted,
    ReplacementCompleted,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HistoryEvent {
    pub time_s: f64,
    pub order: u32,
    pub kind: EventKind,
    pub component_id: Option<String>,
    /// For a service-limit event, the response whose limit was reached, which
    /// names the triggering region of the component.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    pub mass_kg: Option<f64>,
    pub note: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HistorySnapshot {
    pub time_s: f64,
    pub operating: bool,
    pub power_fraction: f64,
    pub available_tritium_kg: f64,
    pub in_process_tritium_kg: f64,
    pub cumulative_production_kg: f64,
    pub cumulative_import_kg: f64,
    pub cumulative_burn_kg: f64,
    pub cumulative_processing_loss_kg: f64,
    pub cumulative_decay_kg: f64,
    pub cumulative_full_power_seconds: f64,
    pub cumulative_fusion_energy_mwh: f64,
    #[serde(alias = "instantaneous_neutron_recovered_heat_mw")]
    pub instantaneous_transport_recovered_heat_mw: Option<f64>,
    pub instantaneous_alpha_recovered_heat_mw: Option<f64>,
    pub instantaneous_gross_electricity_mw: Option<f64>,
    pub instantaneous_auxiliary_electricity_mw: Option<f64>,
    pub instantaneous_net_electricity_mw: Option<f64>,
    #[serde(alias = "cumulative_neutron_recovered_heat_mwh")]
    pub cumulative_transport_recovered_heat_mwh: Option<f64>,
    pub cumulative_alpha_recovered_heat_mwh: Option<f64>,
    pub cumulative_gross_electricity_mwh: Option<f64>,
    pub cumulative_auxiliary_electricity_mwh: Option<f64>,
    pub cumulative_net_electricity_mwh: Option<f64>,
    pub component_fluence_n_m2: BTreeMap<String, f64>,
    /// Fluence of each region-limit track by response ID (neutrons/m²); empty
    /// when no limit tracks a region response.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub limit_fluence_n_m2: BTreeMap<String, f64>,
    pub component_replacements: BTreeMap<String, u32>,
    pub mass_balance_residual_kg: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HistoryResult {
    pub schema_version: String,
    pub outcome: HistoryOutcome,
    pub assumptions: OperatingHistoryAssumptions,
    pub driving_rates: TransportDrivingRates,
    pub tritium_decay_constant_per_s: f64,
    pub tritium_atom_mass_kg: f64,
    pub mass_balance_tolerance_kg: f64,
    #[serde(default)]
    pub integration_segment_count: Option<usize>,
    #[serde(default)]
    pub integration_segment_limit: Option<usize>,
    /// Missing in legacy histories produced with midpoint production cohorts.
    #[serde(default)]
    pub processing_model: Option<String>,
    pub events: Vec<HistoryEvent>,
    pub snapshots: Vec<HistorySnapshot>,
    pub energy_unavailable_reason: Option<String>,
    pub notice: String,
}

#[derive(Clone)]
struct Cohort {
    mass_kg: f64,
    updated_s: f64,
    release_s: f64,
}

#[derive(Clone)]
struct ProductionInterval {
    start_s: f64,
    end_s: f64,
    rate_kg_s: f64,
}

pub const HISTORY_PROCESSING_MODEL_ID: &str = "continuous-delayed-release-v2";

fn production_rate_at(intervals: &VecDeque<ProductionInterval>, time_s: f64) -> f64 {
    intervals
        .front()
        .filter(|i| i.start_s <= time_s && time_s < i.end_s)
        .map_or(0.0, |i| i.rate_kg_s)
}

fn prune_production_intervals(
    intervals: &mut VecDeque<ProductionInterval>,
    time_s: f64,
    delay_s: f64,
) {
    let cutoff = time_s - delay_s;
    while intervals.front().is_some_and(|i| i.end_s <= cutoff + 1e-9) {
        intervals.pop_front();
    }
}

fn next_production_release_boundary(
    intervals: &VecDeque<ProductionInterval>,
    delay_s: f64,
    time_s: f64,
) -> Option<f64> {
    let interval = intervals.front()?;
    [interval.start_s + delay_s, interval.end_s + delay_s]
        .into_iter()
        .filter(|boundary| *boundary > time_s + 1e-9)
        .min_by(f64::total_cmp)
}

fn add_production_interval(
    intervals: &mut VecDeque<ProductionInterval>,
    start_s: f64,
    end_s: f64,
    rate_kg_s: f64,
) -> Result<(), String> {
    if end_s <= start_s || rate_kg_s <= 0.0 {
        return Ok(());
    }
    if let Some(last) = intervals.back_mut()
        && (last.end_s - start_s).abs() <= 1e-9
        && last.rate_kg_s == rate_kg_s
    {
        last.end_s = end_s;
    } else {
        if intervals
            .back()
            .is_some_and(|last| last.end_s > start_s + 1e-9)
        {
            return Err("production intervals must be chronological and non-overlapping".into());
        }
        if intervals.len() >= MAX_HISTORY_PRODUCTION_INTERVALS {
            return Err(format!(
                "history exceeded the {MAX_HISTORY_PRODUCTION_INTERVALS}-interval production-history bound"
            ));
        }
        intervals.push_back(ProductionInterval {
            start_s,
            end_s,
            rate_kg_s,
        });
    }
    Ok(())
}

#[derive(Clone)]
struct ComponentState {
    /// Energy-integrated component-average fluence (the legacy exposure track).
    fluence: f64,
    /// Fluence of every region-limit track on this component, by response ID.
    region_fluence: BTreeMap<String, f64>,
    down_until: f64,
    replacement_count: u32,
    tripped: bool,
}

/// Flux driving a limit's exposure track, at reference power. Limits were
/// checked against the rates before the run, so the lookups cannot miss.
fn limit_flux_mean(rates: &TransportDrivingRates, limit: &ServiceLimit) -> f64 {
    if limit.metric == FAST_FLUX_REGION_METRIC {
        rates.region_flux_n_m2_s[&limit.response_id].rate.mean
    } else {
        rates.component_average_flux_n_m2_s[&limit.component_id].mean
    }
}

fn limit_fluence(state: &ComponentState, limit: &ServiceLimit) -> f64 {
    if limit.metric == FAST_FLUX_REGION_METRIC {
        state.region_fluence[&limit.response_id]
    } else {
        state.fluence
    }
}

fn limit_fluence_mut<'a>(state: &'a mut ComponentState, limit: &ServiceLimit) -> &'a mut f64 {
    if limit.metric == FAST_FLUX_REGION_METRIC {
        state
            .region_fluence
            .get_mut(&limit.response_id)
            .expect("region-limit track exists")
    } else {
        &mut state.fluence
    }
}

fn decay_factor(lambda: f64, dt: f64) -> f64 {
    (-lambda * dt).exp()
}
fn flow_integral(lambda: f64, dt: f64) -> f64 {
    if lambda == 0.0 {
        dt
    } else {
        -(-lambda * dt).exp_m1() / lambda
    }
}
fn evolve_stock(stock: f64, inflow: f64, burn: f64, lambda: f64, dt: f64) -> f64 {
    stock * decay_factor(lambda, dt) + (inflow - burn) * flow_integral(lambda, dt)
}
fn stock_crossing_time(
    stock: f64,
    reserve: f64,
    inflow: f64,
    burn: f64,
    lambda: f64,
    dt: f64,
) -> Option<f64> {
    if stock <= reserve {
        return Some(0.0);
    }
    if evolve_stock(stock, inflow, burn, lambda, dt) >= reserve {
        return None;
    }
    let (mut lo, mut hi) = (0.0, dt);
    for _ in 0..80 {
        let mid = (lo + hi) * 0.5;
        if evolve_stock(stock, inflow, burn, lambda, mid) > reserve {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some(hi)
}

fn stock_rising_crossing_time(
    stock: f64,
    threshold: f64,
    inflow: f64,
    lambda: f64,
    dt: f64,
) -> Option<f64> {
    if stock >= threshold || evolve_stock(stock, inflow, 0.0, lambda, dt) < threshold {
        return None;
    }
    let (mut lo, mut hi) = (0.0, dt);
    for _ in 0..80 {
        let mid = (lo + hi) * 0.5;
        if evolve_stock(stock, inflow, 0.0, lambda, mid) < threshold {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some(hi)
}

fn requested_power_at(a: &OperatingHistoryAssumptions, time_s: f64) -> f64 {
    a.operation
        .iter()
        .find(|p| p.start_s <= time_s && time_s < p.end_s)
        .map_or(0.0, |p| p.power_fraction)
}
fn active_planned_outage(a: &OperatingHistoryAssumptions, time_s: f64) -> Option<&str> {
    a.planned_outages
        .iter()
        .find(|o| o.start_s <= time_s && time_s < o.end_s)
        .map(|o| o.reason.as_str())
}

fn current_in_process(cohorts: &[Cohort], time_s: f64, lambda: f64) -> f64 {
    cohorts
        .iter()
        .map(|c| c.mass_kg * decay_factor(lambda, (time_s - c.updated_s).max(0.0)))
        .sum()
}

fn push_event(
    events: &mut Vec<HistoryEvent>,
    time_s: f64,
    kind: EventKind,
    component_id: Option<String>,
    mass_kg: Option<f64>,
    note: impl Into<String>,
) -> Result<(), String> {
    if events.len() >= MAX_HISTORY_EVENTS {
        return Err("history exceeded the 20,000 discrete-event artifact bound".into());
    }
    let order = events
        .iter()
        .rev()
        .take_while(|e| (e.time_s - time_s).abs() <= 1e-9)
        .count() as u32;
    events.push(HistoryEvent {
        time_s,
        order,
        kind,
        component_id,
        response_id: None,
        mass_kg,
        note: note.into(),
    });
    Ok(())
}

pub fn run_operating_history(
    assumptions: &OperatingHistoryAssumptions,
    rates: &TransportDrivingRates,
) -> Result<HistoryResult, String> {
    run_operating_history_cancellable(assumptions, rates, &Cancellation::default())
}

pub fn run_operating_history_cancellable(
    assumptions: &OperatingHistoryAssumptions,
    rates: &TransportDrivingRates,
    cancellation: &Cancellation,
) -> Result<HistoryResult, String> {
    assumptions.validate()?;
    rates.validate()?;
    for limit in &assumptions.service_limits {
        if !rates
            .component_average_flux_n_m2_s
            .contains_key(&limit.component_id)
        {
            return Err(format!(
                "missing component-average flux response for service limit {}",
                limit.component_id
            ));
        }
        let rate = &rates.component_average_flux_n_m2_s[&limit.component_id];
        let compatible = limit.unit == "neutrons/m²"
            && match limit.metric.as_str() {
                ENERGY_INTEGRATED_FLUX_METRIC => rate.response_id == limit.response_id,
                FAST_FLUX_REGION_METRIC => rates
                    .region_flux_n_m2_s
                    .get(&limit.response_id)
                    .is_some_and(|r| r.component_id == limit.component_id),
                _ => false,
            };
        if !compatible {
            return Err(format!(
                "service limit for {} is incompatible with the supplied exposure response",
                limit.component_id
            ));
        }
    }
    let lambda = std::f64::consts::LN_2 / (TRITIUM_HALF_LIFE_YEARS * JULIAN_YEAR_SECONDS);
    let atom_mass = TRITIUM_MOLAR_MASS_KG_PER_MOL / AVOGADRO_CONSTANT_PER_MOL;
    let breeder_h3_rate =
        rates.breeder_h3_per_source_neutron.mean * rates.neutron_source_rate_per_s * atom_mass;
    let burn_rate = rates.fusion_reaction_rate_per_s
        * (rates.neutron_source_rate_per_s / rates.fusion_reaction_rate_per_s)
        * atom_mass;
    let energy_ready = assumptions.energy.alpha_deposition_fraction.is_some()
        && assumptions.energy.thermal_to_electric_efficiency.is_some()
        && assumptions
            .energy
            .transport_heat_recovery_fraction
            .is_some()
        && assumptions
            .energy
            .auxiliary_power_mw_while_operating
            .is_some()
        && assumptions.energy.auxiliary_power_mw_while_off.is_some()
        && rates.transport_deposited_heat_w.is_some();
    let energy_unavailable_reason = (!energy_ready).then(|| "net electricity requires explicit alpha deposition, the OpenMC whole-model heating response, thermal conversion efficiency, and operating/off auxiliary loads; unavailable terms remain absent".to_owned());
    let mut available = assumptions.initial_available_tritium_kg;
    let mut cohorts = Vec::new();
    let mut production_intervals: VecDeque<ProductionInterval> = VecDeque::new();
    let mut continuous_in_process = 0.0;
    if assumptions.initial_in_process_tritium_kg > 0.0 {
        cohorts.push(Cohort {
            mass_kg: assumptions.initial_in_process_tritium_kg,
            updated_s: 0.0,
            release_s: assumptions.initial_in_process_release_s,
        });
    }
    let mut components: BTreeMap<String, ComponentState> = rates
        .component_average_flux_n_m2_s
        .keys()
        .map(|id| {
            (
                id.clone(),
                ComponentState {
                    fluence: 0.0,
                    // One track per region limit on this component.
                    region_fluence: assumptions
                        .service_limits
                        .iter()
                        .filter(|l| l.component_id == *id && l.metric == FAST_FLUX_REGION_METRIC)
                        .map(|l| (l.response_id.clone(), 0.0))
                        .collect(),
                    down_until: 0.0,
                    replacement_count: 0,
                    tripped: false,
                },
            )
        })
        .collect();
    let mut imported = 0.0;
    let mut produced = 0.0;
    let mut burned = 0.0;
    let mut process_loss = 0.0;
    let mut decay = 0.0;
    let mut full_power_seconds = 0.0;
    let mut fusion_energy_mwh = 0.0;
    let mut transport_recovered_heat_mwh = energy_ready.then_some(0.0);
    let mut alpha_recovered_heat_mwh = energy_ready.then_some(0.0);
    let mut gross_electricity_mwh = energy_ready.then_some(0.0);
    let mut auxiliary_electricity_mwh = energy_ready.then_some(0.0);
    let mut import_index = 0usize;
    let mut time = 0.0;
    let mut previous_operating = false;
    let mut previous_outage = false;
    let mut fuel_unavailable = false;
    let mut permanent_limit = false;
    let fuel_limited_at_horizon;
    let mut events = Vec::new();
    let opening_site_inventory = available + assumptions.initial_in_process_tritium_kg;
    let mut snapshots = Vec::new();
    let mut next_snapshot_s = 0.0;
    let mut captured_event_count = 0usize;
    let mut segments = 0usize;

    loop {
        if cancellation.is_cancelled() {
            return Err("operating history canceled; partial history is not valid".into());
        }
        // Simultaneous event order: initial-inventory point releases; replacement completion;
        // external imports; then operation/outage state changes and limit trips.
        for cohort in &mut cohorts {
            let dt = (time - cohort.updated_s).max(0.0);
            let before = cohort.mass_kg;
            cohort.mass_kg *= decay_factor(lambda, dt);
            decay += before - cohort.mass_kg;
            cohort.updated_s = time;
        }
        let mut keep = Vec::with_capacity(cohorts.len());
        for cohort in cohorts.drain(..) {
            if cohort.release_s <= time + 1e-9 {
                let recoverable = cohort.mass_kg * assumptions.recovery_fraction;
                available += recoverable;
                process_loss += cohort.mass_kg - recoverable;
                // Routine cohort transfers remain visible through inventory and loss ledgers;
                // exporting every hourly release would dominate the history artifact.
            } else {
                keep.push(cohort);
            }
        }
        cohorts = keep;
        if assumptions.processing_delay_s > 0.0 {
            prune_production_intervals(
                &mut production_intervals,
                time,
                assumptions.processing_delay_s,
            );
        } else {
            production_intervals.clear();
            continuous_in_process = 0.0;
        }
        for (component_id, state) in &mut components {
            if state.down_until > 0.0 && state.down_until <= time + 1e-9 {
                // Replacement installs a new component: every exposure track
                // of it, whichever region limit tracks it, starts again at zero.
                state.fluence = 0.0;
                state.region_fluence.values_mut().for_each(|f| *f = 0.0);
                state.down_until = 0.0;
                state.replacement_count += 1;
                state.tripped = false;
                push_event(
                    &mut events,
                    time,
                    EventKind::ReplacementCompleted,
                    Some(component_id.clone()),
                    None,
                    "replaceable component local exposure reset; site fuel/permanent state retained",
                )?;
            }
        }
        while import_index < assumptions.imports.len()
            && assumptions.imports[import_index].at_s <= time + 1e-9
        {
            let event = &assumptions.imports[import_index];
            available += event.mass_kg;
            imported += event.mass_kg;
            push_event(
                &mut events,
                time,
                EventKind::Imported,
                None,
                Some(event.mass_kg),
                event.provenance.clone(),
            )?;
            import_index += 1;
        }
        let scheduled_power = requested_power_at(assumptions, time);
        let down = components.values().any(|c| c.down_until > time + 1e-9);
        let outage = active_planned_outage(assumptions, time);
        let mut requested = if permanent_limit || down || outage.is_some() {
            0.0
        } else {
            scheduled_power
        };
        let is_outage = outage.is_some() || down;
        if is_outage != previous_outage {
            push_event(
                &mut events,
                time,
                if is_outage {
                    EventKind::PlannedOutageStarted
                } else {
                    EventKind::PlannedOutageEnded
                },
                None,
                None,
                outage.unwrap_or("replacement outage"),
            )?;
            previous_outage = is_outage;
        }
        let restart_floor = if fuel_unavailable {
            assumptions.restart_inventory_kg
        } else {
            assumptions.startup_reserve_kg
        };
        if requested > 0.0 && available <= restart_floor + 1e-13 * restart_floor.max(1.0) {
            requested = 0.0;
            if !fuel_unavailable {
                push_event(
                    &mut events,
                    time,
                    EventKind::FuelUnavailable,
                    None,
                    None,
                    "usable inventory is at or below the declared startup reserve",
                )?;
                fuel_unavailable = true;
            }
        } else if requested > 0.0 && fuel_unavailable {
            push_event(
                &mut events,
                time,
                EventKind::FuelAvailable,
                None,
                None,
                "usable inventory again exceeds the declared startup reserve",
            )?;
            fuel_unavailable = false;
        }
        if (requested > 0.0) != previous_operating {
            push_event(
                &mut events,
                time,
                if requested > 0.0 {
                    EventKind::OperationStarted
                } else {
                    EventKind::OperationStopped
                },
                None,
                None,
                if requested > 0.0 {
                    "source on under declared calendar, inventory, outage, and limit state"
                } else {
                    "source off under declared calendar, inventory, outage, or limit state"
                },
            )?;
            previous_operating = requested > 0.0;
        }
        let in_process_at_time = current_in_process(&cohorts, time, lambda) + continuous_in_process;
        let site_at_time = available + in_process_at_time;
        let residual_at_time = site_at_time - opening_site_inventory - produced - imported
            + burned
            + process_loss
            + decay;
        let scale_at_time =
            opening_site_inventory + produced + imported + burned + process_loss + decay;
        let tolerance_at_time = MASS_BALANCE_RELATIVE_TOLERANCE * scale_at_time.max(1.0);
        if residual_at_time.abs() > tolerance_at_time {
            return Err(format!(
                "tritium mass balance residual {} kg exceeds tolerance {} kg",
                residual_at_time, tolerance_at_time
            ));
        }
        let event_snapshot = events.len() != captured_event_count;
        let routine_snapshot = time + 1e-9 >= next_snapshot_s;
        if event_snapshot || routine_snapshot || time >= assumptions.horizon_s {
            let (transport_heat_mw, alpha_heat_mw, gross_electric_mw, aux_mw, net_electric_mw) =
                if energy_ready {
                    let transport_heat = rates.transport_deposited_heat_w.as_ref().unwrap().mean
                        * assumptions.energy.transport_heat_recovery_fraction.unwrap()
                        * requested
                        / 1e6;
                    let alpha = rates.fusion_reaction_rate_per_s
                        * requested
                        * (rates.total_reaction_energy_ev - rates.primary_neutron_energy_ev)
                        * ELEMENTARY_CHARGE_J_PER_EV
                        / 1e6
                        * assumptions.energy.alpha_deposition_fraction.unwrap();
                    let gross = (transport_heat + alpha)
                        * assumptions.energy.thermal_to_electric_efficiency.unwrap();
                    let aux = if requested > 0.0 {
                        assumptions
                            .energy
                            .auxiliary_power_mw_while_operating
                            .unwrap()
                    } else {
                        assumptions.energy.auxiliary_power_mw_while_off.unwrap()
                    };
                    (
                        Some(transport_heat),
                        Some(alpha),
                        Some(gross),
                        Some(aux),
                        Some(gross - aux),
                    )
                } else {
                    (None, None, None, None, None)
                };
            snapshots.push(HistorySnapshot {
                time_s: time,
                operating: requested > 0.0,
                power_fraction: requested,
                available_tritium_kg: available,
                in_process_tritium_kg: in_process_at_time,
                cumulative_production_kg: produced,
                cumulative_import_kg: imported,
                cumulative_burn_kg: burned,
                cumulative_processing_loss_kg: process_loss,
                cumulative_decay_kg: decay,
                cumulative_full_power_seconds: full_power_seconds,
                cumulative_fusion_energy_mwh: fusion_energy_mwh,
                instantaneous_transport_recovered_heat_mw: transport_heat_mw,
                instantaneous_alpha_recovered_heat_mw: alpha_heat_mw,
                instantaneous_gross_electricity_mw: gross_electric_mw,
                instantaneous_auxiliary_electricity_mw: aux_mw,
                instantaneous_net_electricity_mw: net_electric_mw,
                cumulative_transport_recovered_heat_mwh: transport_recovered_heat_mwh,
                cumulative_alpha_recovered_heat_mwh: alpha_recovered_heat_mwh,
                cumulative_gross_electricity_mwh: gross_electricity_mwh,
                cumulative_auxiliary_electricity_mwh: auxiliary_electricity_mwh,
                cumulative_net_electricity_mwh: gross_electricity_mwh
                    .zip(auxiliary_electricity_mwh)
                    .map(|(gross, auxiliary)| gross - auxiliary),
                component_fluence_n_m2: components
                    .iter()
                    .map(|(id, s)| (id.clone(), s.fluence))
                    .collect(),
                limit_fluence_n_m2: components
                    .values()
                    .flat_map(|s| s.region_fluence.iter().map(|(id, f)| (id.clone(), *f)))
                    .collect(),
                component_replacements: components
                    .iter()
                    .map(|(id, s)| (id.clone(), s.replacement_count))
                    .collect(),
                mass_balance_residual_kg: residual_at_time,
            });
            captured_event_count = events.len();
            while next_snapshot_s <= time + 1e-9 {
                next_snapshot_s += assumptions.snapshot_interval_s;
            }
        }
        if time >= assumptions.horizon_s {
            fuel_limited_at_horizon = requested_power_at(assumptions, (time - 1e-9).max(0.0)) > 0.0
                && available
                    <= assumptions.startup_reserve_kg
                        + 1e-13 * assumptions.startup_reserve_kg.max(1.0);
            break;
        }
        segments += 1;
        if segments > MAX_HISTORY_SEGMENTS {
            return Err(format!(
                "history used {segments} segments; runtime segment limit is {MAX_HISTORY_SEGMENTS}"
            ));
        }

        let mut next = (time + assumptions.maximum_step_s).min(assumptions.horizon_s);
        next = next.min(next_snapshot_s);
        for period in &assumptions.operation {
            for boundary in [period.start_s, period.end_s] {
                if boundary > time + 1e-9 {
                    next = next.min(boundary);
                }
            }
        }
        for outage in &assumptions.planned_outages {
            for boundary in [outage.start_s, outage.end_s] {
                if boundary > time + 1e-9 {
                    next = next.min(boundary);
                }
            }
        }
        for cohort in &cohorts {
            if cohort.release_s > time + 1e-9 {
                next = next.min(cohort.release_s);
            }
        }
        if assumptions.processing_delay_s > 0.0
            && let Some(release_boundary) = next_production_release_boundary(
                &production_intervals,
                assumptions.processing_delay_s,
                time,
            )
        {
            next = next.min(release_boundary);
        }
        for import in assumptions.imports.iter().skip(import_index) {
            if import.at_s > time + 1e-9 {
                next = next.min(import.at_s);
            }
        }
        for state in components.values() {
            if state.down_until > time + 1e-9 {
                next = next.min(state.down_until);
            }
        }
        let mut dt = next - time;
        if dt <= 0.0 {
            return Err("history event queue did not advance".into());
        }

        // Bound the active segment by fuel reserve and every traceable exposure limit.
        let released_h3_rate = if assumptions.processing_delay_s == 0.0 {
            breeder_h3_rate * requested
        } else {
            production_rate_at(&production_intervals, time - assumptions.processing_delay_s)
                * decay_factor(lambda, assumptions.processing_delay_s)
        };
        let recovered_inflow = released_h3_rate * assumptions.recovery_fraction;
        if requested == 0.0
            && scheduled_power > 0.0
            && fuel_unavailable
            && !permanent_limit
            && !down
            && outage.is_none()
        {
            let restart_threshold = assumptions.restart_inventory_kg
                + 2.0e-13 * assumptions.restart_inventory_kg.max(1.0);
            if let Some(cross) = stock_rising_crossing_time(
                available,
                restart_threshold,
                recovered_inflow,
                lambda,
                dt,
            ) && cross > 1e-9
                && cross < dt - 1e-9
            {
                dt = cross;
                next = time + dt;
            }
        }
        let active_burn = burn_rate * requested;
        let mut active_duration = dt;
        let mut truncate_at_event = false;
        if requested > 0.0 {
            if let Some(cross) = stock_crossing_time(
                available,
                assumptions.startup_reserve_kg,
                recovered_inflow,
                active_burn,
                lambda,
                active_duration,
            ) && cross < active_duration - 1e-9
            {
                active_duration = cross;
                truncate_at_event = true;
            }
            for limit in &assumptions.service_limits {
                let state = &components[&limit.component_id];
                if state.tripped || state.down_until > time {
                    continue;
                }
                let flux = limit_flux_mean(rates, limit) * requested;
                let until = (limit.limit - limit_fluence(state, limit)).max(0.0) / flux;
                if flux > 0.0 && until < active_duration - 1e-9 {
                    active_duration = until;
                    truncate_at_event = true;
                }
            }
        } else {
            active_duration = 0.0;
        }
        let active_duration = active_duration.max(0.0).min(dt);
        if truncate_at_event {
            dt = active_duration;
        }
        if active_duration > 0.0 {
            let gross = breeder_h3_rate * requested * active_duration;
            produced += gross;
            burned += active_burn * active_duration;
            add_production_interval(
                &mut production_intervals,
                time,
                time + active_duration,
                breeder_h3_rate * requested,
            )?;
            full_power_seconds += requested * active_duration;
            fusion_energy_mwh +=
                rates.reference_fusion_power_mw * requested * active_duration / 3600.0;
            for (id, state) in &mut components {
                if let Some(flux) = rates.component_average_flux_n_m2_s.get(id) {
                    state.fluence += flux.mean * requested * active_duration;
                }
                for (response_id, fluence) in &mut state.region_fluence {
                    *fluence += rates.region_flux_n_m2_s[response_id].rate.mean
                        * requested
                        * active_duration;
                }
            }
            if energy_ready {
                let alpha_fraction = assumptions.energy.alpha_deposition_fraction.unwrap();
                let efficiency = assumptions.energy.thermal_to_electric_efficiency.unwrap();
                let transport_heat_mw = rates.transport_deposited_heat_w.as_ref().unwrap().mean
                    * assumptions.energy.transport_heat_recovery_fraction.unwrap()
                    * requested
                    / 1e6;
                let alpha_power_mw = rates.fusion_reaction_rate_per_s
                    * requested
                    * (rates.total_reaction_energy_ev - rates.primary_neutron_energy_ev)
                    * ELEMENTARY_CHARGE_J_PER_EV
                    / 1e6;
                if let Some(v) = transport_recovered_heat_mwh.as_mut() {
                    *v += transport_heat_mw * active_duration / 3600.0;
                }
                if let Some(v) = alpha_recovered_heat_mwh.as_mut() {
                    *v += alpha_power_mw * alpha_fraction * active_duration / 3600.0;
                }
                let gross_electric_mw =
                    (alpha_power_mw * alpha_fraction + transport_heat_mw) * efficiency;
                let aux_mw = assumptions
                    .energy
                    .auxiliary_power_mw_while_operating
                    .unwrap();
                if let Some(v) = gross_electricity_mwh.as_mut() {
                    *v += gross_electric_mw * active_duration / 3600.0;
                }
                if let Some(v) = auxiliary_electricity_mwh.as_mut() {
                    *v += aux_mw * active_duration / 3600.0;
                }
            }
        }
        // Delayed continuous release, recovery, scrap and burn are constant rates
        // over this event-bounded interval. Exponential inventory integration is exact.
        let available_start = available;
        let available_integral = flow_integral(lambda, dt);
        available = evolve_stock(available, recovered_inflow, active_burn, lambda, dt);
        let available_decay = (lambda * available_start * available_integral
            + (recovered_inflow - active_burn) * (dt - available_integral))
            .max(0.0);
        decay += available_decay;
        if assumptions.processing_delay_s == 0.0 {
            process_loss += released_h3_rate * (1.0 - assumptions.recovery_fraction) * dt;
        } else {
            process_loss += released_h3_rate * (1.0 - assumptions.recovery_fraction) * dt;
            let production_rate = breeder_h3_rate * requested;
            let in_process_start = continuous_in_process;
            let in_process_integral = flow_integral(lambda, dt);
            continuous_in_process = (in_process_start * decay_factor(lambda, dt)
                + (production_rate - released_h3_rate) * in_process_integral)
                .max(0.0);
            let process_decay = (lambda * in_process_start * in_process_integral
                + (production_rate - released_h3_rate) * (dt - in_process_integral))
                .max(0.0);
            decay += process_decay;
        }
        if energy_ready {
            let off_aux = assumptions.energy.auxiliary_power_mw_while_off.unwrap();
            let off_duration = dt - active_duration;
            if let Some(v) = auxiliary_electricity_mwh.as_mut() {
                *v += off_aux * off_duration / 3600.0;
            }
        }
        let mut ordered_limits: Vec<&ServiceLimit> = assumptions.service_limits.iter().collect();
        ordered_limits.sort_by(|a, b| {
            let rank = |c: ComponentClass| {
                if c == ComponentClass::Permanent {
                    0_u8
                } else {
                    1_u8
                }
            };
            rank(a.class)
                .cmp(&rank(b.class))
                .then(a.component_id.cmp(&b.component_id))
                .then(a.response_id.cmp(&b.response_id))
        });
        for limit in ordered_limits {
            if requested > 0.0 {
                let state = components.get_mut(&limit.component_id).unwrap();
                // The first limit of a component to be reached trips it; the
                // other limits of that component are then moot until replacement.
                if limit_fluence(state, limit) + 1e-10 * limit.limit >= limit.limit
                    && !state.tripped
                {
                    *limit_fluence_mut(state, limit) = limit.limit;
                    state.tripped = true;
                    push_event(
                        &mut events,
                        time + active_duration,
                        EventKind::ServiceLimitReached,
                        Some(limit.component_id.clone()),
                        None,
                        format!(
                            "{} {} reached on {}; applicability is conditional on authored limit",
                            limit.metric, limit.unit, limit.response_id
                        ),
                    )?;
                    if let Some(event) = events.last_mut() {
                        event.response_id = Some(limit.response_id.clone());
                    }
                    match limit.class {
                        ComponentClass::Permanent => {
                            permanent_limit = true;
                        }
                        ComponentClass::Replaceable => {
                            if !permanent_limit {
                                let duration = limit.replacement_duration_s.unwrap();
                                state.down_until = time + active_duration + duration;
                                push_event(
                                    &mut events,
                                    time + active_duration,
                                    EventKind::ReplacementStarted,
                                    Some(limit.component_id.clone()),
                                    None,
                                    format!("declared replacement outage for {duration} s"),
                                )?;
                            }
                        }
                    }
                }
            }
        }
        if available < 0.0 && available > -1e-12 {
            available = 0.0;
        }
        if available < 0.0 {
            return Err("fuel inventory became negative; limiting event handling failed".into());
        }
        time += dt;
    }
    let outcome = if permanent_limit {
        HistoryOutcome::PermanentComponentLimit
    } else if fuel_limited_at_horizon {
        HistoryOutcome::FuelLimitedAtHorizon
    } else {
        HistoryOutcome::HorizonCompleted
    };
    let scale = opening_site_inventory + produced + imported + burned + process_loss + decay;
    Ok(HistoryResult {
        schema_version: "faris-history-result/v0.1".into(), outcome, assumptions: assumptions.clone(), driving_rates: rates.clone(),
        tritium_decay_constant_per_s: lambda, tritium_atom_mass_kg: atom_mass,
        mass_balance_tolerance_kg: MASS_BALANCE_RELATIVE_TOLERANCE*scale.max(1.0), events, snapshots,
        integration_segment_count: Some(segments), integration_segment_limit: Some(MAX_HISTORY_SEGMENTS),
        processing_model: Some(HISTORY_PROCESSING_MODEL_ID.into()),
        energy_unavailable_reason,
        notice: "Conditional deterministic history from the exact recorded transport driving rates and authored assumptions. Monte Carlo standard errors and assumption/model uncertainty are retained as provenance but not propagated into a qualified bound. No service-life or net-electricity claim is qualified.".into(),
    })
}

/// Linear state interpolation is allowed only within a discrete-event-free interval.
pub fn interpolate_snapshot(run: &HistoryResult, time_s: f64) -> Option<HistorySnapshot> {
    if !time_s.is_finite() || run.snapshots.is_empty() {
        return None;
    }
    if let Some(exact) = run
        .snapshots
        .iter()
        .find(|s| (s.time_s - time_s).abs() <= 1e-9)
    {
        return Some(exact.clone());
    }
    let i = run.snapshots.partition_point(|s| s.time_s < time_s);
    if i == 0 || i >= run.snapshots.len() {
        return None;
    }
    let a = &run.snapshots[i - 1];
    let b = &run.snapshots[i];
    if run
        .events
        .iter()
        .any(|e| e.time_s > a.time_s + 1e-9 && e.time_s < b.time_s - 1e-9)
    {
        return None;
    }
    if a.operating != b.operating || a.power_fraction != b.power_fraction {
        return None;
    }
    let f = (time_s - a.time_s) / (b.time_s - a.time_s);
    let lerp = |x: f64, y: f64| x + (y - x) * f;
    let mut s = a.clone();
    s.time_s = time_s;
    s.available_tritium_kg = lerp(a.available_tritium_kg, b.available_tritium_kg);
    s.in_process_tritium_kg = lerp(a.in_process_tritium_kg, b.in_process_tritium_kg);
    s.cumulative_production_kg = lerp(a.cumulative_production_kg, b.cumulative_production_kg);
    s.cumulative_import_kg = lerp(a.cumulative_import_kg, b.cumulative_import_kg);
    s.cumulative_burn_kg = lerp(a.cumulative_burn_kg, b.cumulative_burn_kg);
    s.cumulative_processing_loss_kg = lerp(
        a.cumulative_processing_loss_kg,
        b.cumulative_processing_loss_kg,
    );
    s.cumulative_decay_kg = lerp(a.cumulative_decay_kg, b.cumulative_decay_kg);
    s.cumulative_full_power_seconds = lerp(
        a.cumulative_full_power_seconds,
        b.cumulative_full_power_seconds,
    );
    s.cumulative_fusion_energy_mwh = lerp(
        a.cumulative_fusion_energy_mwh,
        b.cumulative_fusion_energy_mwh,
    );
    s.cumulative_gross_electricity_mwh = zip_lerp(
        a.cumulative_gross_electricity_mwh,
        b.cumulative_gross_electricity_mwh,
        f,
    );
    s.cumulative_auxiliary_electricity_mwh = zip_lerp(
        a.cumulative_auxiliary_electricity_mwh,
        b.cumulative_auxiliary_electricity_mwh,
        f,
    );
    s.cumulative_net_electricity_mwh = zip_lerp(
        a.cumulative_net_electricity_mwh,
        b.cumulative_net_electricity_mwh,
        f,
    );
    for (id, v) in &mut s.component_fluence_n_m2 {
        *v = lerp(a.component_fluence_n_m2[id], b.component_fluence_n_m2[id]);
    }
    for (id, v) in &mut s.limit_fluence_n_m2 {
        *v = lerp(a.limit_fluence_n_m2[id], b.limit_fluence_n_m2[id]);
    }
    s.mass_balance_residual_kg = lerp(a.mass_balance_residual_kg, b.mass_balance_residual_kg);
    Some(s)
}

fn zip_lerp(a: Option<f64>, b: Option<f64>, f: f64) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x + (y - x) * f),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faris_model::history::{EnergyAssumptions, OperatingHistoryAssumptions};

    fn assumptions(horizon: f64) -> OperatingHistoryAssumptions {
        OperatingHistoryAssumptions {
            schema_version: faris_model::history::OPERATING_HISTORY_VERSION.into(),
            horizon_s: horizon,
            maximum_step_s: 3600.0,
            snapshot_interval_s: 86_400.0,
            initial_available_tritium_kg: 1.0,
            initial_in_process_tritium_kg: 0.0,
            initial_in_process_release_s: 0.0,
            startup_reserve_kg: 0.0,
            restart_inventory_kg: 0.1,
            recovery_fraction: 1.0,
            processing_delay_s: 0.0,
            operation: vec![],
            planned_outages: vec![],
            imports: vec![],
            service_limits: vec![],
            energy: EnergyAssumptions {
                alpha_deposition_fraction: None,
                thermal_to_electric_efficiency: None,
                transport_heat_recovery_fraction: None,
                auxiliary_power_mw_while_operating: None,
                auxiliary_power_mw_while_off: None,
                provenance: "not modeled in decay-only control".into(),
            },
            provenance: "analytic decay-only unit control".into(),
        }
    }

    fn rates() -> TransportDrivingRates {
        let q_ev = 17.6e6;
        let e = ELEMENTARY_CHARGE_J_PER_EV;
        let reactions = 1.0e6 / (q_ev * e);
        TransportDrivingRates {
            reference_fusion_power_mw: 1.0,
            fusion_reaction_rate_per_s: reactions,
            neutron_source_rate_per_s: reactions,
            total_reaction_energy_ev: q_ev,
            primary_neutron_energy_ev: 14.1e6,
            breeder_h3_per_source_neutron: ScalarRate {
                mean: 0.0,
                standard_error: Some(0.0),
                unit: "particles/source_neutron".into(),
                response_id: "blanket-tritium".into(),
            },
            component_average_flux_n_m2_s: BTreeMap::new(),
            region_flux_n_m2_s: BTreeMap::new(),
            transport_deposited_heat_w: None,
            scenario_sha256: "a".repeat(64),
            transport_artifact_sha256: "b".repeat(64),
            solver_digest: format!("sha256:{}", "c".repeat(64)),
            nuclear_data_digest: format!("sha256:{}", "d".repeat(64)),
            covariance: None,
        }
    }

    #[test]
    fn history_validation_allows_reviewed_two_million_step_budget_and_rejects_more() {
        let mut fine = assumptions(946_728_000.0);
        fine.maximum_step_s = 500.0;
        assert!(fine.validate().is_ok());

        fine.maximum_step_s = 250.0;
        assert!(fine.validate().is_ok());

        fine.maximum_step_s = 200.0;
        assert!(fine.validate().unwrap_err().contains("4,000,000-step"));
    }

    #[test]
    fn legacy_history_without_segment_counts_deserializes_as_unavailable() {
        let run = run_operating_history(&assumptions(86_400.0), &rates()).unwrap();
        let mut value = serde_json::to_value(run).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("integration_segment_count");
        value
            .as_object_mut()
            .unwrap()
            .remove("integration_segment_limit");
        value.as_object_mut().unwrap().remove("processing_model");
        let legacy: HistoryResult = serde_json::from_value(value).unwrap();
        assert_eq!(legacy.integration_segment_count, None);
        assert_eq!(legacy.integration_segment_limit, None);
        assert_eq!(legacy.processing_model, None);
    }

    #[test]
    fn decay_only_history_matches_half_life_and_is_daily_decimated() {
        let mut a = assumptions(TRITIUM_HALF_LIFE_YEARS * JULIAN_YEAR_SECONDS);
        a.snapshot_interval_s = 30.0 * 86_400.0;
        a.maximum_step_s = 86_400.0;
        let r = run_operating_history(&a, &rates()).unwrap();
        let last = r.snapshots.last().unwrap();
        assert!((last.available_tritium_kg - 0.5).abs() < 1e-12);
        assert!(last.mass_balance_residual_kg.abs() <= r.mass_balance_tolerance_kg);
        assert!(r.snapshots.len() <= 152);
    }

    #[test]
    fn rejects_unresolved_delay_steps_and_pre_cancelled_jobs() {
        let mut a = assumptions(100.0);
        a.processing_delay_s = 10.0;
        assert!(a.validate().is_err());
        a.maximum_step_s = 0.25;
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(run_operating_history_cancellable(&a, &rates(), &cancel).is_err());
    }

    #[test]
    fn rejects_invalid_transport_bindings_before_history_execution() {
        let mut driving = rates();
        driving.breeder_h3_per_source_neutron.unit = "particles/s".into();
        assert!(
            driving.validate().is_err(),
            "wrong normalized response unit"
        );

        let mut driving = rates();
        driving.neutron_source_rate_per_s *= 1.01;
        assert!(
            driving.validate().is_err(),
            "reaction/source identity mismatch"
        );

        let mut driving = rates();
        driving.component_average_flux_n_m2_s.insert(
            "blanket".into(),
            ScalarRate {
                mean: f64::NAN,
                standard_error: Some(0.0),
                unit: "neutrons/m²/s".into(),
                response_id: "blanket-flux".into(),
            },
        );
        assert!(driving.validate().is_err(), "non-finite flux binding");

        let mut driving = rates();
        driving.transport_artifact_sha256 = "not-a-digest".into();
        assert!(
            driving.validate().is_err(),
            "missing source artifact identity"
        );
    }

    #[test]
    fn zero_sampled_component_flux_is_valid_but_negative_flux_is_not() {
        let mut driving = rates();
        driving.component_average_flux_n_m2_s.insert(
            "magnets".into(),
            ScalarRate {
                mean: 0.0,
                standard_error: Some(0.0),
                unit: "neutrons/m²/s".into(),
                response_id: "magnets-flux".into(),
            },
        );
        assert!(driving.validate().is_ok());
        driving
            .component_average_flux_n_m2_s
            .get_mut("magnets")
            .unwrap()
            .mean = -1.0;
        assert!(driving.validate().is_err());
    }

    #[test]
    fn legacy_neutron_named_heat_fields_still_deserialize_through_aliases() {
        let mut a = assumptions(100.0);
        a.energy.alpha_deposition_fraction = Some(1.0);
        a.energy.transport_heat_recovery_fraction = Some(0.9);
        a.energy.thermal_to_electric_efficiency = Some(0.4);
        a.energy.auxiliary_power_mw_while_operating = Some(1.0);
        a.energy.auxiliary_power_mw_while_off = Some(1.0);
        let mut driving = rates();
        driving.transport_deposited_heat_w = Some(ScalarRate {
            mean: 10.0e6,
            standard_error: Some(100.0),
            unit: "W".into(),
            response_id: "heating-total-whole-model".into(),
        });
        let result = run_operating_history(&a, &driving).unwrap();
        let mut legacy = serde_json::to_value(result).unwrap();
        let energy = legacy["assumptions"]["energy"].as_object_mut().unwrap();
        let recovery = energy.remove("transport_heat_recovery_fraction").unwrap();
        energy.insert("neutron_heat_recovery_fraction".into(), recovery);
        let rates = legacy["driving_rates"].as_object_mut().unwrap();
        let heating = rates.remove("transport_deposited_heat_w").unwrap();
        rates.insert("neutron_deposited_heat_w".into(), heating);
        for snapshot in legacy["snapshots"].as_array_mut().unwrap() {
            let values = snapshot.as_object_mut().unwrap();
            let heat = values
                .remove("instantaneous_transport_recovered_heat_mw")
                .unwrap();
            values.insert("instantaneous_neutron_recovered_heat_mw".into(), heat);
            let heat = values
                .remove("cumulative_transport_recovered_heat_mwh")
                .unwrap();
            values.insert("cumulative_neutron_recovered_heat_mwh".into(), heat);
        }
        let restored: HistoryResult = serde_json::from_value(legacy).unwrap();
        assert!(restored.driving_rates.transport_deposited_heat_w.is_some());
        assert!(
            restored
                .snapshots
                .last()
                .unwrap()
                .cumulative_transport_recovered_heat_mwh
                .is_some()
        );
    }

    #[test]
    fn fuel_starvation_holds_off_instead_of_zero_inventory_restart_chatter() {
        let mut a = assumptions(10_000.0);
        a.maximum_step_s = 3600.0;
        a.snapshot_interval_s = 10_000.0;
        a.initial_available_tritium_kg = 1.0e-7;
        a.startup_reserve_kg = 5.0e-8;
        a.restart_inventory_kg = 6.0e-8;
        a.operation = vec![faris_model::history::PowerPeriod {
            start_s: 0.0,
            end_s: 10_000.0,
            power_fraction: 1.0,
        }];
        let result = run_operating_history(&a, &rates()).unwrap();
        assert_eq!(
            result
                .events
                .iter()
                .filter(|e| e.kind == EventKind::FuelUnavailable)
                .count(),
            1
        );
        assert_eq!(
            result
                .events
                .iter()
                .filter(|e| e.kind == EventKind::FuelAvailable)
                .count(),
            0
        );
        assert!(result.snapshots.last().unwrap().available_tritium_kg >= 0.0);
        assert!(
            result
                .snapshots
                .last()
                .unwrap()
                .mass_balance_residual_kg
                .abs()
                <= result.mass_balance_tolerance_kg
        );
    }

    #[test]
    fn delayed_recovery_restarts_only_after_release_threshold_is_reached() {
        let mut a = assumptions(90_000.0);
        a.maximum_step_s = 3600.0;
        a.snapshot_interval_s = 3600.0;
        a.initial_available_tritium_kg = 1.0e-7;
        a.startup_reserve_kg = 5.0e-8;
        a.restart_inventory_kg = 6.0e-8;
        a.recovery_fraction = 0.95;
        a.processing_delay_s = 86_400.0;
        a.operation = vec![faris_model::history::PowerPeriod {
            start_s: 0.0,
            end_s: 90_000.0,
            power_fraction: 1.0,
        }];
        let mut r = rates();
        r.breeder_h3_per_source_neutron.mean = 3.0;
        let result = run_operating_history(&a, &r).unwrap();
        let unavailable = result
            .events
            .iter()
            .find(|e| e.kind == EventKind::FuelUnavailable)
            .unwrap();
        let available = result
            .events
            .iter()
            .find(|e| e.kind == EventKind::FuelAvailable)
            .unwrap();
        assert!(unavailable.time_s < 100.0);
        assert!(available.time_s >= 86_400.0);
        assert!(available.time_s > unavailable.time_s);
        let mut finer = a.clone();
        finer.maximum_step_s = 1800.0;
        let finer_result = run_operating_history(&finer, &r).unwrap();
        let finer_available = finer_result
            .events
            .iter()
            .find(|e| e.kind == EventKind::FuelAvailable)
            .unwrap();
        assert!((available.time_s - finer_available.time_s).abs() < 1e-6);
        assert!((available.time_s % 3600.0) > 1e-6);
        assert!(available.time_s < unavailable.time_s + a.processing_delay_s - 1e-4);
        assert!(
            result
                .snapshots
                .iter()
                .all(|s| s.available_tritium_kg >= 0.0)
        );
        assert!(
            result
                .snapshots
                .iter()
                .all(|s| s.mass_balance_residual_kg.abs() <= result.mass_balance_tolerance_kg)
        );
    }

    #[test]
    fn delayed_constant_production_matches_continuous_release_solution() {
        let mut a = assumptions(200.0);
        a.maximum_step_s = 2.5;
        a.snapshot_interval_s = 10.0;
        a.initial_available_tritium_kg = 1.0;
        a.startup_reserve_kg = 0.0;
        a.restart_inventory_kg = 0.0;
        a.processing_delay_s = 60.0;
        a.operation = vec![faris_model::history::PowerPeriod {
            start_s: 0.0,
            end_s: 100.0,
            power_fraction: 1.0,
        }];
        let mut r = rates();
        let atom_mass = TRITIUM_MOLAR_MASS_KG_PER_MOL / AVOGADRO_CONSTANT_PER_MOL;
        let production_rate = 2.0e-9;
        r.breeder_h3_per_source_neutron.mean =
            production_rate / (r.neutron_source_rate_per_s * atom_mass);
        let result = run_operating_history(&a, &r).unwrap();
        assert_eq!(
            result.processing_model.as_deref(),
            Some(HISTORY_PROCESSING_MODEL_ID)
        );

        let lambda = result.tritium_decay_constant_per_s;
        let delayed_survival = decay_factor(lambda, a.processing_delay_s);
        let release_rate = production_rate * delayed_survival;
        let burn_rate = r.fusion_reaction_rate_per_s * atom_mass;
        let expected_available = evolve_stock(
            evolve_stock(
                evolve_stock(
                    evolve_stock(1.0, 0.0, burn_rate, lambda, 60.0),
                    release_rate,
                    burn_rate,
                    lambda,
                    40.0,
                ),
                release_rate,
                0.0,
                lambda,
                60.0,
            ),
            0.0,
            0.0,
            lambda,
            40.0,
        );
        let at = |time: f64| result.snapshots.iter().find(|s| s.time_s == time).unwrap();
        let expected_inventory = production_rate * flow_integral(lambda, 60.0);
        assert!((at(60.0).in_process_tritium_kg - expected_inventory).abs() < 1e-18);
        assert!((at(100.0).in_process_tritium_kg - expected_inventory).abs() < 1e-18);
        assert!(at(160.0).in_process_tritium_kg.abs() < 1e-18);
        assert!((at(200.0).available_tritium_kg - expected_available).abs() < 1e-12);
        assert!(at(200.0).mass_balance_residual_kg.abs() <= result.mass_balance_tolerance_kg);
    }

    #[test]
    fn production_interval_history_is_bounded_but_merges_contiguous_equal_rates() {
        let mut intervals = VecDeque::new();
        for index in 0..MAX_HISTORY_PRODUCTION_INTERVALS {
            intervals.push_back(ProductionInterval {
                start_s: index as f64 * 2.0,
                end_s: index as f64 * 2.0 + 1.0,
                rate_kg_s: index as f64 + 1.0,
            });
        }
        assert!(
            add_production_interval(
                &mut intervals,
                MAX_HISTORY_PRODUCTION_INTERVALS as f64 * 2.0 - 1.0,
                MAX_HISTORY_PRODUCTION_INTERVALS as f64 * 2.0,
                MAX_HISTORY_PRODUCTION_INTERVALS as f64,
            )
            .is_ok()
        );
        assert_eq!(intervals.len(), MAX_HISTORY_PRODUCTION_INTERVALS);
        assert!(
            add_production_interval(
                &mut intervals,
                MAX_HISTORY_PRODUCTION_INTERVALS as f64 * 2.0,
                MAX_HISTORY_PRODUCTION_INTERVALS as f64 * 2.0 + 1.0,
                f64::from(MAX_HISTORY_PRODUCTION_INTERVALS as u32) + 1.0,
            )
            .is_err()
        );
    }

    #[test]
    fn production_interval_queue_front_handles_gaps_and_future_release_boundaries() {
        let intervals = VecDeque::from([
            ProductionInterval {
                start_s: 0.0,
                end_s: 2.0,
                rate_kg_s: 1.0,
            },
            ProductionInterval {
                start_s: 5.0,
                end_s: 7.0,
                rate_kg_s: 2.0,
            },
        ]);
        assert_eq!(production_rate_at(&intervals, 1.0), 1.0);
        assert_eq!(production_rate_at(&intervals, 3.0), 0.0);
        assert_eq!(
            next_production_release_boundary(&intervals, 3.0, 0.0),
            Some(3.0)
        );
        assert_eq!(
            next_production_release_boundary(&intervals, 3.0, 3.0),
            Some(5.0)
        );
        let mut pruned = intervals.clone();
        prune_production_intervals(&mut pruned, 6.0, 3.0);
        assert_eq!(pruned.len(), 1);
        assert_eq!(production_rate_at(&pruned, 3.0), 0.0);
        assert_eq!(
            next_production_release_boundary(&pruned, 3.0, 6.0),
            Some(8.0)
        );
        assert!(add_production_interval(&mut pruned, 6.5, 8.0, 3.0).is_err());
    }

    #[test]
    fn energy_ledger_separates_recovered_heat_and_signed_net_power() {
        let mut a = assumptions(20.0);
        a.maximum_step_s = 10.0;
        a.snapshot_interval_s = 10.0;
        a.operation = vec![faris_model::history::PowerPeriod {
            start_s: 0.0,
            end_s: 10.0,
            power_fraction: 1.0,
        }];
        a.energy = faris_model::history::EnergyAssumptions {
            alpha_deposition_fraction: Some(1.0),
            thermal_to_electric_efficiency: Some(0.4),
            transport_heat_recovery_fraction: Some(0.9),
            auxiliary_power_mw_while_operating: Some(2.0),
            auxiliary_power_mw_while_off: Some(5.0),
            provenance: "analytic control inputs".into(),
        };
        let mut r = rates();
        r.transport_deposited_heat_w = Some(ScalarRate {
            mean: 10e6,
            standard_error: Some(0.0),
            unit: "W".into(),
            response_id: "heating-total-whole-model".into(),
        });
        let result = run_operating_history(&a, &r).unwrap();
        let on = result.snapshots.iter().find(|s| s.time_s == 0.0).unwrap();
        assert!((on.instantaneous_transport_recovered_heat_mw.unwrap() - 9.0).abs() < 1e-12);
        assert!(on.instantaneous_alpha_recovered_heat_mw.unwrap() > 0.19);
        assert!(on.instantaneous_net_electricity_mw.unwrap() > 1.0);
        let off = result.snapshots.iter().find(|s| s.time_s == 10.0).unwrap();
        assert_eq!(off.instantaneous_net_electricity_mw, Some(-5.0));
    }

    #[test]
    fn replaceable_trip_resets_locally_then_permanent_trip_stops() {
        let mut a = assumptions(100.0);
        a.maximum_step_s = 10.0;
        a.snapshot_interval_s = 10.0;
        a.operation = vec![faris_model::history::PowerPeriod {
            start_s: 0.0,
            end_s: 100.0,
            power_fraction: 1.0,
        }];
        a.service_limits = vec![
            ServiceLimit {
                component_id: "blanket".into(),
                class: ComponentClass::Replaceable,
                response_id: "flux-blanket".into(),
                metric: "energy_integrated_component_average_neutron_flux".into(),
                unit: "neutrons/m²".into(),
                limit: 1.0e11,
                replacement_duration_s: Some(10.0),
                provenance: "test".into(),
            },
            ServiceLimit {
                component_id: "magnet".into(),
                class: ComponentClass::Permanent,
                response_id: "flux-magnet".into(),
                metric: "energy_integrated_component_average_neutron_flux".into(),
                unit: "neutrons/m²".into(),
                limit: 3.0e11,
                replacement_duration_s: None,
                provenance: "test".into(),
            },
        ];
        let mut r = rates();
        r.component_average_flux_n_m2_s.insert(
            "blanket".into(),
            ScalarRate {
                mean: 1e10,
                standard_error: Some(0.0),
                unit: "neutrons/m²/s".into(),
                response_id: "flux-blanket".into(),
            },
        );
        r.component_average_flux_n_m2_s.insert(
            "magnet".into(),
            ScalarRate {
                mean: 1e10,
                standard_error: Some(0.0),
                unit: "neutrons/m²/s".into(),
                response_id: "flux-magnet".into(),
            },
        );
        let result = run_operating_history(&a, &r).unwrap();
        assert!(
            result
                .events
                .iter()
                .any(|e| e.kind == EventKind::ReplacementCompleted)
        );
        assert!(
            result
                .events
                .iter()
                .any(|e| e.kind == EventKind::ServiceLimitReached
                    && e.component_id.as_deref() == Some("magnet"))
        );
        assert_eq!(result.outcome, HistoryOutcome::PermanentComponentLimit);
        let last = result.snapshots.last().unwrap();
        assert!(last.component_replacements["blanket"] >= 1);
        assert!(last.mass_balance_residual_kg.abs() <= result.mass_balance_tolerance_kg);
    }

    #[test]
    fn simultaneous_import_outage_and_operation_stop_have_stable_order() {
        let mut a = assumptions(100.0);
        a.maximum_step_s = 10.0;
        a.snapshot_interval_s = 10.0;
        a.operation = vec![faris_model::history::PowerPeriod {
            start_s: 0.0,
            end_s: 100.0,
            power_fraction: 1.0,
        }];
        a.planned_outages = vec![faris_model::history::TimeInterval {
            start_s: 50.0,
            end_s: 60.0,
            reason: "same-time control event".into(),
        }];
        a.imports = vec![faris_model::history::TritiumImport {
            at_s: 50.0,
            mass_kg: 0.25,
            provenance: "test import".into(),
        }];
        let result = run_operating_history(&a, &rates()).unwrap();
        let at_time: Vec<_> = result
            .events
            .iter()
            .filter(|e| (e.time_s - 50.0).abs() < 1e-9)
            .map(|e| e.kind.clone())
            .collect();
        assert_eq!(at_time[0], EventKind::Imported);
        assert_eq!(at_time[1], EventKind::PlannedOutageStarted);
        assert_eq!(at_time[2], EventKind::OperationStopped);
    }

    #[test]
    fn daily_snapshot_outputs_converge_under_step_refinement() {
        let mut coarse = assumptions(3.0 * 86_400.0);
        coarse.processing_delay_s = 86_400.0;
        coarse.maximum_step_s = 3600.0;
        coarse.snapshot_interval_s = 86_400.0;
        coarse.operation = vec![faris_model::history::PowerPeriod {
            start_s: 0.0,
            end_s: coarse.horizon_s,
            power_fraction: 0.5,
        }];
        let coarse_result = run_operating_history(&coarse, &rates()).unwrap();
        let mut fine = coarse.clone();
        fine.maximum_step_s = 1800.0;
        let fine_result = run_operating_history(&fine, &rates()).unwrap();
        let c = coarse_result.snapshots.last().unwrap();
        let f = fine_result.snapshots.last().unwrap();
        assert!((c.cumulative_burn_kg - f.cumulative_burn_kg).abs() < 1e-14);
        assert!((c.cumulative_decay_kg - f.cumulative_decay_kg).abs() < 1e-12);
        assert!((c.mass_balance_residual_kg - f.mass_balance_residual_kg).abs() < 1e-12);
    }

    #[test]
    fn sensitivity_is_full_reruns_bounded_and_cancellable() {
        let a = assumptions(100.0);
        let grid = crate::comparison::HistorySensitivityGrid {
            recovery_fraction_levels: vec![0.95],
            delay_multipliers: vec![1.0],
            service_limit_multipliers: vec![1.0],
            rationale: "single-point API control; not a range claim".into(),
        };
        let result = crate::comparison::run_history_sensitivity(&a, &rates(), &grid).unwrap();
        assert_eq!(result.points.len(), 1);
        let cancellation = Cancellation::default();
        cancellation.cancel();
        assert!(
            crate::comparison::run_history_sensitivity_cancellable(
                &a,
                &rates(),
                &grid,
                &cancellation
            )
            .is_err()
        );
        let mut excessive = grid;
        excessive.recovery_fraction_levels = vec![0.1, 0.2, 0.4, 0.6, 0.8];
        excessive.delay_multipliers = vec![0.5, 1.0, 2.0, 4.0];
        excessive.service_limit_multipliers = vec![0.5, 1.0, 2.0, 4.0];
        assert!(crate::comparison::run_history_sensitivity(&a, &rates(), &excessive).is_err());
    }

    fn scalar(mean: f64, response_id: &str) -> ScalarRate {
        ScalarRate {
            mean,
            standard_error: Some(0.0),
            unit: "neutrons/m²/s".into(),
            response_id: response_id.into(),
        }
    }

    /// A magnet with three regional fast-flux limits of 3e11 n/m² and the given
    /// region fluxes (inboard, outboard, port sector) in n/m²/s.
    fn regional_case(fluxes: [f64; 3]) -> (OperatingHistoryAssumptions, TransportDrivingRates) {
        let mut a = assumptions(100.0);
        a.maximum_step_s = 10.0;
        a.snapshot_interval_s = 10.0;
        a.operation = vec![faris_model::history::PowerPeriod {
            start_s: 0.0,
            end_s: 100.0,
            power_fraction: 1.0,
        }];
        let names = ["r-inboard", "r-outboard", "r-port"];
        a.service_limits = names
            .iter()
            .map(|id| ServiceLimit {
                component_id: "magnet".into(),
                class: ComponentClass::Replaceable,
                response_id: (*id).into(),
                metric: FAST_FLUX_REGION_METRIC.into(),
                unit: "neutrons/m²".into(),
                limit: 3.0e11,
                replacement_duration_s: Some(10.0),
                provenance: "test".into(),
            })
            .collect();
        let mut r = rates();
        r.component_average_flux_n_m2_s
            .insert("magnet".into(), scalar(1.0e10, "flux-magnet"));
        for (id, flux) in names.iter().zip(fluxes) {
            r.region_flux_n_m2_s.insert(
                (*id).into(),
                RegionFluxRate {
                    component_id: "magnet".into(),
                    region: None,
                    energy_min_ev: 1.0e5,
                    rate: scalar(flux, id),
                },
            );
        }
        (a, r)
    }

    fn trips(run: &HistoryResult) -> Vec<(f64, String)> {
        run.events
            .iter()
            .filter(|e| e.kind == EventKind::ServiceLimitReached)
            .map(|e| (e.time_s, e.response_id.clone().unwrap()))
            .collect()
    }

    #[test]
    fn first_regional_limit_replaces_the_component_and_resets_every_track() {
        // The port sector reaches 3e11 after 10 s; the inboard half is at 2.5e11
        // by then and would trip 2 s into the next period if its track survived.
        let (a, r) = regional_case([2.5e10, 5.0e9, 3.0e10]);
        let run = run_operating_history(&a, &r).unwrap();
        let trips = trips(&run);
        assert_eq!(trips.len(), 5, "{trips:?}");
        assert!(trips.iter().all(|t| t.1 == "r-port"));
        assert!((trips[0].0 - 10.0).abs() < 1e-6 && (trips[1].0 - 30.0).abs() < 1e-6);
        // Replacement started at the first trip and finished 10 s later.
        let started: Vec<_> = run
            .events
            .iter()
            .filter(|e| e.kind == EventKind::ReplacementStarted)
            .collect();
        assert!((started[0].time_s - 10.0).abs() < 1e-6);
        let done = run
            .events
            .iter()
            .find(|e| e.kind == EventKind::ReplacementCompleted)
            .unwrap();
        assert!((done.time_s - 20.0).abs() < 1e-6);
        // Every track of the component restarted at zero.
        let reset = run
            .snapshots
            .iter()
            .find(|s| (s.time_s - 20.0).abs() < 1e-6)
            .unwrap();
        assert_eq!(reset.component_fluence_n_m2["magnet"], 0.0);
        assert!(reset.limit_fluence_n_m2.values().all(|f| *f == 0.0));
        assert_eq!(reset.limit_fluence_n_m2.len(), 3);
        // Just before the trip the inboard track held 2.5e11 and the port track its limit.
        let before = run
            .snapshots
            .iter()
            .find(|s| (s.time_s - 10.0).abs() < 1e-6)
            .unwrap();
        assert!((before.limit_fluence_n_m2["r-inboard"] - 2.5e11).abs() < 1.0);
        assert!((before.limit_fluence_n_m2["r-port"] - 3.0e11).abs() < 1.0);
        assert_eq!(
            run.snapshots.last().unwrap().component_replacements["magnet"],
            5
        );
        // Mass balance is untouched by which limit trips.
        let last = run.snapshots.last().unwrap();
        assert!(last.mass_balance_residual_kg.abs() <= run.mass_balance_tolerance_kg);
    }

    #[test]
    fn triggering_region_follows_the_fastest_track() {
        for (fluxes, expected) in [
            ([3.0e10, 5.0e9, 1.0e10], "r-inboard"),
            ([5.0e9, 3.0e10, 1.0e10], "r-outboard"),
            ([5.0e9, 1.0e10, 3.0e10], "r-port"),
        ] {
            let (a, r) = regional_case(fluxes);
            let run = run_operating_history(&a, &r).unwrap();
            let first = &trips(&run)[0];
            assert!((first.0 - 10.0).abs() < 1e-6, "{first:?}");
            assert_eq!(first.1, expected);
            let event = run
                .events
                .iter()
                .find(|e| e.kind == EventKind::ServiceLimitReached)
                .unwrap();
            assert!(event.note.contains(expected));
        }
    }

    #[test]
    fn region_limits_are_checked_against_the_supplied_responses() {
        let (a, mut r) = regional_case([1.0e10; 3]);
        run_operating_history(&a, &r).unwrap();
        r.region_flux_n_m2_s.remove("r-outboard");
        assert!(run_operating_history(&a, &r).is_err());
        let (a, mut r) = regional_case([1.0e10; 3]);
        r.region_flux_n_m2_s.get_mut("r-port").unwrap().component_id = "other".into();
        assert!(run_operating_history(&a, &r).is_err());
        let (mut a, r) = regional_case([1.0e10; 3]);
        a.service_limits[0].metric = "peak_neutron_flux".into();
        assert!(run_operating_history(&a, &r).is_err());
    }

    #[test]
    fn region_flux_adds_to_the_covariance_after_component_flux() {
        let (_, mut r) = regional_case([1.0e10; 3]);
        let ids: Vec<_> = r
            .covariance_entries()
            .iter()
            .map(|e| e.0.to_owned())
            .collect();
        assert_eq!(
            ids,
            [
                "blanket-tritium",
                "flux-magnet",
                "r-inboard",
                "r-outboard",
                "r-port"
            ]
        );
        r.region_flux_n_m2_s.clear();
        assert_eq!(r.covariance_entries().len(), 2);
    }
}
