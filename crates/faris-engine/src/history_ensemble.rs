//! Ensemble of operating histories on sampled, correlated transport rates.
//!
//! The transport Monte Carlo result gives each driving rate a mean and a
//! covariance (see [`DrivingCovariance`]). This module draws rate vectors from
//! a multivariate normal with those moments, runs the deterministic history on
//! each draw, and summarises the spread of the outputs. It carries only the
//! transport sampling uncertainty; nuclear data, model form, half-life and
//! authored-assumption uncertainty are not represented.
//!
//! Random numbers are generated in-house so results do not depend on external
//! crates or on the thread count:
//!
//! * SplitMix64 (Steele, Lea and Flood 2014) expands a 64-bit value into the
//!   state of the generator and mixes (seed, sample index) into a stream seed.
//! * xoshiro256** (Blackman and Vigna, "Scrambled linear pseudorandom number
//!   generators", ACM TOMS 47(4), 2021) is the generator.
//! * Standard normals use the Box-Muller transform (Box and Muller 1958), both
//!   members of each pair being used.
//!
//! Sample `i` owns a stream seeded from `(seed, i)` and nothing else; draws are
//! made serially before any history runs, so the sampled rates and therefore
//! all outputs are identical for any number of worker threads.

use crate::history::{
    HistoryOutcome, HistoryResult, TransportDrivingRates, factor_covariance, interpolate_snapshot,
    run_operating_history_cancellable,
};
use crate::jobs::Cancellation;
use faris_model::history::{ComponentClass, OperatingHistoryAssumptions};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub const ENSEMBLE_METHOD_ID: &str = "faris-history-ensemble/v1";
pub const DEFAULT_SAMPLES: u32 = 200;
pub const MAX_SAMPLES: u32 = 2000;
/// Points of the common time grid used for the series bands.
pub const SERIES_GRID_POINTS: usize = 361;
/// Rejected draws above this fraction of accepted samples fail the ensemble.
pub const MAX_REJECTION_FRACTION: f64 = 0.01;
/// Draw attempts for one sample before the ensemble gives up on it.
const MAX_ATTEMPTS_PER_SAMPLE: u32 = 1000;
const WILSON_Z_95: f64 = 1.959_963_984_540_054;

pub const NO_COVARIANCE_WHY: &str = "This transport record has standard errors but no covariance between its results, so correlated sampling is not possible";
pub const NO_COVARIANCE_NEXT: &str =
    "Rerun transport with this FARIS version to record batch-resolved results";
pub const REJECTION_NEXT: &str = "run more histories or use variance reduction";

#[derive(Debug, thiserror::Error)]
pub enum EnsembleError {
    #[error("{0}")]
    Invalid(String),
    #[error("history ensemble cancelled; partial ensemble is not valid")]
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnsembleSettings {
    pub samples: u32,
    /// None derives the seed from the transport artifact and the assumptions.
    pub seed: Option<u64>,
    /// None uses the available parallelism minus one, at least one.
    pub threads: Option<usize>,
}

impl Default for EnsembleSettings {
    fn default() -> Self {
        Self {
            samples: DEFAULT_SAMPLES,
            seed: None,
            threads: None,
        }
    }
}

/// First 8 bytes (big-endian) of SHA-256 over the method ID, the transport
/// artifact SHA-256 text and the compact JSON of the assumptions, concatenated.
pub fn derive_seed(
    transport_artifact_sha256: &str,
    assumptions: &OperatingHistoryAssumptions,
) -> Result<u64, String> {
    let canonical = serde_json::to_vec(assumptions).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    hasher.update(ENSEMBLE_METHOD_ID.as_bytes());
    hasher.update(transport_artifact_sha256.as_bytes());
    hasher.update(&canonical);
    let digest = hasher.finalize();
    Ok(u64::from_be_bytes(digest[..8].try_into().unwrap()))
}

// ---------------------------------------------------------------------------
// Random numbers

fn splitmix64_step(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// xoshiro256** with a cached second Box-Muller normal.
#[derive(Clone, Debug)]
struct Rng {
    s: [u64; 4],
    spare: Option<f64>,
}

impl Rng {
    /// Stream for sample `index`: the (seed, index) pair is hashed twice so
    /// neighbouring indices do not give overlapping SplitMix64 sequences.
    fn stream(seed: u64, index: u64) -> Self {
        let mut a = index ^ 0xD1B5_4A32_D192_ED03;
        let mixed_index = splitmix64_step(&mut a);
        let mut b = seed ^ mixed_index;
        let mut state = splitmix64_step(&mut b);
        let mut s = [0_u64; 4];
        for word in &mut s {
            *word = splitmix64_step(&mut state);
        }
        Self { s, spare: None }
    }

    fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    fn next_normal(&mut self) -> f64 {
        if let Some(z) = self.spare.take() {
            return z;
        }
        // u1 in (0, 1], u2 in [0, 1): 53-bit mantissas.
        let u1 = ((self.next_u64() >> 11) + 1) as f64 * (1.0 / 9_007_199_254_740_992.0);
        let u2 = (self.next_u64() >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0);
        let radius = (-2.0 * u1.ln()).sqrt();
        let angle = std::f64::consts::TAU * u2;
        self.spare = Some(radius * angle.sin());
        radius * angle.cos()
    }
}

// ---------------------------------------------------------------------------
// Sampling the rates

/// Gaussian sampler for the driving-rate vector of one transport record.
struct Sampler {
    means: Vec<f64>,
    /// Row-major n x n lower factor, `monte_carlo = L L^T`.
    factor: Vec<f64>,
    volume_sd: Vec<f64>,
    /// Heating must be strictly positive; the other rates may be zero.
    strict: Vec<bool>,
}

struct Draw {
    values: Vec<f64>,
    rejections: u32,
    /// False when the attempt cap was reached without an acceptable draw.
    accepted: bool,
}

impl Sampler {
    fn new(rates: &TransportDrivingRates) -> Result<Self, String> {
        let covariance = rates
            .covariance
            .as_ref()
            .ok_or("transport driving rates carry no covariance")?;
        let entries = rates.covariance_entries();
        let n = entries.len();
        let factor = factor_covariance(&covariance.monte_carlo, n)?;
        let heat_id = rates
            .transport_deposited_heat_w
            .as_ref()
            .map(|h| h.response_id.as_str());
        Ok(Self {
            means: entries.iter().map(|e| e.1.mean).collect(),
            factor,
            volume_sd: covariance
                .volume_variance
                .iter()
                .map(|v| v.sqrt())
                .collect(),
            strict: entries.iter().map(|e| Some(e.0) == heat_id).collect(),
        })
    }

    fn acceptable(&self, x: &[f64]) -> bool {
        x.iter().enumerate().all(|(i, v)| {
            v.is_finite()
                && if self.strict[i] {
                    *v > 0.0
                } else {
                    // Exactly zero is allowed only for a rate whose mean is zero
                    // (a sampled component with no tracks and no variance).
                    *v > 0.0 || (*v == 0.0 && self.means[i] == 0.0)
                }
        })
    }

    /// Draw x = mu + L z + independent volume term; redraw from the same
    /// stream until every rate is physical.
    fn draw(&self, seed: u64, index: u64) -> Draw {
        let n = self.means.len();
        let mut rng = Rng::stream(seed, index);
        let mut rejections = 0;
        for _ in 0..MAX_ATTEMPTS_PER_SAMPLE {
            let z: Vec<f64> = (0..n).map(|_| rng.next_normal()).collect();
            let values: Vec<f64> = (0..n)
                .map(|i| {
                    let correlated: f64 = (0..n).map(|k| self.factor[i * n + k] * z[k]).sum();
                    let independent = if self.volume_sd[i] > 0.0 {
                        self.volume_sd[i] * rng.next_normal()
                    } else {
                        0.0
                    };
                    self.means[i] + correlated + independent
                })
                .collect();
            if self.acceptable(&values) {
                return Draw {
                    values,
                    rejections,
                    accepted: true,
                };
            }
            rejections += 1;
        }
        Draw {
            values: self.means.clone(),
            rejections,
            accepted: false,
        }
    }
}

fn with_sampled_means(base: &TransportDrivingRates, values: &[f64]) -> TransportDrivingRates {
    let mut rates = base.clone();
    rates.covariance = None;
    rates.breeder_h3_per_source_neutron.mean = values[0];
    let flux_count = rates.component_average_flux_n_m2_s.len();
    for (rate, v) in rates
        .component_average_flux_n_m2_s
        .values_mut()
        .zip(&values[1..])
    {
        rate.mean = *v;
    }
    let region_count = rates.region_flux_n_m2_s.len();
    for (region, v) in rates
        .region_flux_n_m2_s
        .values_mut()
        .zip(&values[1 + flux_count..])
    {
        region.rate.mean = *v;
    }
    if let Some(h) = &mut rates.transport_deposited_heat_w {
        h.mean = values[1 + flux_count + region_count];
    }
    rates
}

// ---------------------------------------------------------------------------
// Result types

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EnsembleStatus {
    Evaluated,
    NotEvaluated { why: String, next_step: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SampledRates {
    pub breeder_h3_per_source_neutron: f64,
    pub component_average_flux_n_m2_s: BTreeMap<String, f64>,
    /// Region fast-flux responses by response ID; empty without regional limits.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub region_flux_n_m2_s: BTreeMap<String, f64>,
    pub transport_deposited_heat_w: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SampleOutcome {
    /// Sample index (0 for the nominal history).
    pub index: u32,
    pub rates: SampledRates,
    /// Draws rejected before this sample's rates were accepted.
    pub rejected_draws: u32,
    pub outcome: HistoryOutcome,
    /// Time of the permanent component trip if one occurred, else the horizon.
    pub terminal_time_s: f64,
    pub replacements: BTreeMap<String, u32>,
    /// Start of the first replacement outage per component; None if none.
    pub first_replacement_time_s: BTreeMap<String, Option<f64>>,
    /// Response (region) whose limit tripped each component first, for
    /// components that tripped; absent for legacy single-limit records.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub first_trigger_response: BTreeMap<String, String>,
    pub full_power_time_s: f64,
    pub final_available_tritium_kg: f64,
    pub final_in_process_tritium_kg: f64,
    pub cumulative_gross_electricity_mwh: Option<f64>,
    pub cumulative_auxiliary_electricity_mwh: Option<f64>,
    pub cumulative_net_electricity_mwh: Option<f64>,
}

impl SampleOutcome {
    /// Every numeric output by name; replacement counts are `replacements:<id>`.
    fn scalar_outputs(&self) -> BTreeMap<String, f64> {
        let mut out = BTreeMap::new();
        out.insert("full_power_time_s".into(), self.full_power_time_s);
        out.insert("terminal_time_s".into(), self.terminal_time_s);
        out.insert(
            "final_available_tritium_kg".into(),
            self.final_available_tritium_kg,
        );
        out.insert(
            "final_in_process_tritium_kg".into(),
            self.final_in_process_tritium_kg,
        );
        for (name, v) in [
            (
                "cumulative_gross_electricity_mwh",
                self.cumulative_gross_electricity_mwh,
            ),
            (
                "cumulative_auxiliary_electricity_mwh",
                self.cumulative_auxiliary_electricity_mwh,
            ),
            (
                "cumulative_net_electricity_mwh",
                self.cumulative_net_electricity_mwh,
            ),
        ] {
            if let Some(v) = v {
                out.insert(name.into(), v);
            }
        }
        for (id, count) in &self.replacements {
            out.insert(format!("replacements:{id}"), f64::from(*count));
        }
        for (id, t) in &self.first_replacement_time_s {
            if let Some(t) = t {
                out.insert(format!("first_replacement_time_s:{id}"), *t);
            }
        }
        out
    }
}

/// A quantile estimate with a distribution-free 95 % confidence interval from
/// order statistics. A bound is None when the sample is too small to give one.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct QuantileEstimate {
    pub value: f64,
    pub ci95_low: Option<f64>,
    pub ci95_high: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ContinuousSummary {
    pub name: String,
    /// Samples that have this output (first-replacement times exist only for
    /// samples that replaced the component).
    pub n: u32,
    pub mean: f64,
    pub p5: QuantileEstimate,
    pub p50: QuantileEstimate,
    pub p95: QuantileEstimate,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Proportion {
    pub count: u32,
    pub fraction: f64,
    pub wilson95_low: f64,
    pub wilson95_high: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DiscreteSummary {
    pub name: String,
    pub n: u32,
    pub categories: BTreeMap<String, Proportion>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SeriesBand {
    pub name: String,
    /// Samples with a value at each grid point; interpolation is withheld across
    /// discrete events and after the terminal time.
    pub n: Vec<u32>,
    pub p5: Vec<Option<f64>>,
    pub p50: Vec<Option<f64>>,
    pub p95: Vec<Option<f64>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EnsembleSummary {
    pub continuous: Vec<ContinuousSummary>,
    pub discrete: Vec<DiscreteSummary>,
    /// Common grid of `SERIES_GRID_POINTS` points over the horizon.
    pub time_grid_s: Vec<f64>,
    pub series_bands: Vec<SeriesBand>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HistoryEnsemble {
    pub method: String,
    pub status: EnsembleStatus,
    pub seed: u64,
    pub samples_requested: u32,
    pub samples_accepted: u32,
    pub rejections: u64,
    pub scenario_sha256: String,
    pub transport_artifact_sha256: String,
    /// Covariance estimator identity from the transport record.
    pub covariance_method: Option<String>,
    /// History at the mean rates (index 0, no draws).
    pub nominal: Option<SampleOutcome>,
    pub samples: Vec<SampleOutcome>,
    pub summary: Option<EnsembleSummary>,
    pub scope: String,
}

const SCOPE_NOTICE: &str = "Spread of the history outputs from transport Monte Carlo sampling uncertainty only, assuming the transport means are jointly normal with the recorded covariance. Nuclear data, model form, tritium half-life and authored-assumption uncertainty are not included; no service-life or net-electricity claim is qualified.";

// ---------------------------------------------------------------------------
// Statistics

/// Linear-interpolation quantile (Hyndman and Fan type 7) of a sorted slice.
fn quantile_sorted(sorted: &[f64], p: f64) -> f64 {
    let h = (sorted.len() - 1) as f64 * p;
    let lo = h.floor() as usize;
    let hi = (lo + 1).min(sorted.len() - 1);
    sorted[lo] + (h - lo as f64) * (sorted[hi] - sorted[lo])
}

/// 1-based ranks (l, u) of order statistics whose interval covers the p-quantile
/// with probability at least 95 %: l is the largest rank with
/// P(B <= l-1) <= 2.5 %, u the smallest with P(B <= u-1) >= 97.5 %, B ~ Bin(n, p).
fn quantile_ci_ranks(n: usize, p: f64) -> (Option<usize>, Option<usize>) {
    let mut cdf = Vec::with_capacity(n + 1);
    let ratio = (p / (1.0 - p)).ln();
    let mut log_pmf = n as f64 * (1.0 - p).ln();
    let mut total = 0.0;
    for k in 0..=n {
        total += log_pmf.exp();
        cdf.push(total);
        if k < n {
            log_pmf += ((n - k) as f64 / (k + 1) as f64).ln() + ratio;
        }
    }
    let lower = (1..=n).rev().find(|l| cdf[l - 1] <= 0.025);
    let upper = (1..=n).find(|u| cdf[u - 1] >= 0.975);
    (lower, upper)
}

fn quantile_estimate(sorted: &[f64], p: f64) -> QuantileEstimate {
    let (l, u) = quantile_ci_ranks(sorted.len(), p);
    QuantileEstimate {
        value: quantile_sorted(sorted, p),
        ci95_low: l.map(|l| sorted[l - 1]),
        ci95_high: u.map(|u| sorted[u - 1]),
    }
}

/// Wilson 95 % score interval for `k` successes in `n` trials.
pub fn wilson_interval(k: u32, n: u32) -> (f64, f64) {
    if n == 0 {
        return (0.0, 1.0);
    }
    let (k, n) = (f64::from(k), f64::from(n));
    let p = k / n;
    let z2 = WILSON_Z_95 * WILSON_Z_95;
    let denom = 1.0 + z2 / n;
    let centre = (p + z2 / (2.0 * n)) / denom;
    let half = WILSON_Z_95 * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt() / denom;
    let lower = if k == 0.0 {
        0.0
    } else {
        (centre - half).max(0.0)
    };
    let upper = if k == n {
        1.0
    } else {
        (centre + half).min(1.0)
    };
    (lower, upper)
}

fn proportion(count: u32, n: u32) -> Proportion {
    let (lo, hi) = wilson_interval(count, n);
    Proportion {
        count,
        fraction: if n == 0 {
            0.0
        } else {
            f64::from(count) / f64::from(n)
        },
        wilson95_low: lo,
        wilson95_high: hi,
    }
}

fn sorted_finite(values: impl Iterator<Item = f64>) -> Vec<f64> {
    let mut v: Vec<f64> = values.collect();
    v.sort_by(f64::total_cmp);
    v
}

fn continuous_summary(name: &str, values: Vec<f64>) -> Option<ContinuousSummary> {
    if values.is_empty() {
        return None;
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let sorted = sorted_finite(values.into_iter());
    Some(ContinuousSummary {
        name: name.into(),
        n: sorted.len() as u32,
        mean,
        p5: quantile_estimate(&sorted, 0.05),
        p50: quantile_estimate(&sorted, 0.50),
        p95: quantile_estimate(&sorted, 0.95),
    })
}

// ---------------------------------------------------------------------------
// Running

type Series = BTreeMap<String, Vec<Option<f64>>>;
type SampleResult = (usize, Result<(SampleOutcome, Series), String>);

fn outcome_of(
    index: u32,
    rates: &[f64],
    base: &TransportDrivingRates,
    rejected: u32,
    run: &HistoryResult,
) -> SampleOutcome {
    let sampled = with_sampled_means(base, rates);
    let last = run
        .snapshots
        .last()
        .expect("a completed history has snapshots");
    let mut first = BTreeMap::new();
    for id in last.component_replacements.keys() {
        let t = run
            .events
            .iter()
            .filter(|e| {
                e.kind == crate::history::EventKind::ReplacementStarted
                    && e.component_id.as_deref() == Some(id.as_str())
            })
            .map(|e| e.time_s)
            .fold(None, |acc: Option<f64>, t| {
                Some(acc.map_or(t, |a| a.min(t)))
            });
        first.insert(id.clone(), t);
    }
    // The earliest service-limit event of each component names the region that
    // tripped it first (events are in time then order sequence).
    let mut first_trigger_response = BTreeMap::new();
    for event in &run.events {
        if event.kind == crate::history::EventKind::ServiceLimitReached
            && let (Some(component), Some(response)) = (&event.component_id, &event.response_id)
        {
            first_trigger_response
                .entry(component.clone())
                .or_insert_with(|| response.clone());
        }
    }
    // A permanent trip ends operation but the run continues to the horizon, so
    // the terminal time is the trip time when there is one.
    let permanent_trip_s = run
        .events
        .iter()
        .filter(|e| {
            e.kind == crate::history::EventKind::ServiceLimitReached
                && run.assumptions.service_limits.iter().any(|l| {
                    l.class == ComponentClass::Permanent
                        && e.component_id.as_deref() == Some(l.component_id.as_str())
                })
        })
        .map(|e| e.time_s)
        .fold(None, |acc: Option<f64>, t| {
            Some(acc.map_or(t, |a| a.min(t)))
        });
    SampleOutcome {
        index,
        rates: SampledRates {
            breeder_h3_per_source_neutron: sampled.breeder_h3_per_source_neutron.mean,
            component_average_flux_n_m2_s: sampled
                .component_average_flux_n_m2_s
                .iter()
                .map(|(k, v)| (k.clone(), v.mean))
                .collect(),
            region_flux_n_m2_s: sampled
                .region_flux_n_m2_s
                .iter()
                .map(|(k, v)| (k.clone(), v.rate.mean))
                .collect(),
            transport_deposited_heat_w: sampled.transport_deposited_heat_w.map(|h| h.mean),
        },
        rejected_draws: rejected,
        outcome: run.outcome.clone(),
        terminal_time_s: permanent_trip_s.unwrap_or(last.time_s),
        replacements: last.component_replacements.clone(),
        first_replacement_time_s: first,
        first_trigger_response,
        full_power_time_s: last.cumulative_full_power_seconds,
        final_available_tritium_kg: last.available_tritium_kg,
        final_in_process_tritium_kg: last.in_process_tritium_kg,
        cumulative_gross_electricity_mwh: last.cumulative_gross_electricity_mwh,
        cumulative_auxiliary_electricity_mwh: last.cumulative_auxiliary_electricity_mwh,
        cumulative_net_electricity_mwh: last.cumulative_net_electricity_mwh,
    }
}

fn time_grid(horizon_s: f64) -> Vec<f64> {
    let last = (SERIES_GRID_POINTS - 1) as f64;
    (0..SERIES_GRID_POINTS)
        .map(|k| horizon_s * k as f64 / last)
        .collect()
}

fn series_of(run: &HistoryResult, grid: &[f64]) -> Series {
    let snapshots: Vec<Option<_>> = grid.iter().map(|t| interpolate_snapshot(run, *t)).collect();
    let mut series = Series::new();
    let components: Vec<&String> = run
        .snapshots
        .last()
        .map(|s| s.component_fluence_n_m2.keys().collect())
        .unwrap_or_default();
    for id in components {
        series.insert(
            format!("fluence_n_m2:{id}"),
            snapshots
                .iter()
                .map(|s| s.as_ref().map(|s| s.component_fluence_n_m2[id]))
                .collect(),
        );
    }
    series.insert(
        "available_tritium_kg".into(),
        snapshots
            .iter()
            .map(|s| s.as_ref().map(|s| s.available_tritium_kg))
            .collect(),
    );
    if run
        .snapshots
        .last()
        .is_some_and(|s| s.cumulative_net_electricity_mwh.is_some())
    {
        series.insert(
            "cumulative_net_electricity_mwh".into(),
            snapshots
                .iter()
                .map(|s| s.as_ref().and_then(|s| s.cumulative_net_electricity_mwh))
                .collect(),
        );
    }
    series
}

fn summarise(samples: &[SampleOutcome], series: &[Series], grid: Vec<f64>) -> EnsembleSummary {
    let outputs: Vec<BTreeMap<String, f64>> = samples.iter().map(|s| s.scalar_outputs()).collect();
    let names: BTreeSet<&String> = outputs.iter().flat_map(|o| o.keys()).collect();
    let mut continuous = Vec::new();
    for name in names {
        if name.starts_with("replacements:") {
            continue;
        }
        let values: Vec<f64> = outputs
            .iter()
            .filter_map(|o| o.get(name).copied())
            .collect();
        continuous.extend(continuous_summary(name, values));
    }
    let n = samples.len() as u32;
    let mut discrete = Vec::new();
    let mut statuses: BTreeMap<String, u32> = [
        "horizon_completed",
        "fuel_limited_at_horizon",
        "permanent_component_limit",
    ]
    .iter()
    .map(|s| (s.to_string(), 0))
    .collect();
    for s in samples {
        let label = match s.outcome {
            HistoryOutcome::HorizonCompleted => "horizon_completed",
            HistoryOutcome::FuelLimitedAtHorizon => "fuel_limited_at_horizon",
            HistoryOutcome::PermanentComponentLimit => "permanent_component_limit",
        };
        *statuses.get_mut(label).unwrap() += 1;
    }
    discrete.push(DiscreteSummary {
        name: "terminal_status".into(),
        n,
        categories: statuses
            .into_iter()
            .map(|(k, c)| (k, proportion(c, n)))
            .collect(),
    });
    let components: BTreeSet<&String> =
        samples.iter().flat_map(|s| s.replacements.keys()).collect();
    for id in components {
        let counts: Vec<u32> = samples
            .iter()
            .map(|s| s.replacements.get(id).copied().unwrap_or(0))
            .collect();
        let max = counts.iter().copied().max().unwrap_or(0);
        discrete.push(DiscreteSummary {
            name: format!("replacements:{id}"),
            n,
            categories: (0..=max)
                .map(|v| {
                    (
                        v.to_string(),
                        proportion(counts.iter().filter(|c| **c == v).count() as u32, n),
                    )
                })
                .collect(),
        });
    }
    // Which response (region) tripped each component first, over all samples.
    // Samples whose component never tripped count under "none".
    let triggered: BTreeSet<&String> = samples
        .iter()
        .flat_map(|s| s.first_trigger_response.keys())
        .collect();
    for component in triggered {
        let mut counts: BTreeMap<String, u32> = BTreeMap::new();
        for s in samples {
            let key = s
                .first_trigger_response
                .get(component)
                .cloned()
                .unwrap_or_else(|| "none".into());
            *counts.entry(key).or_insert(0) += 1;
        }
        counts.entry("none".into()).or_insert(0);
        discrete.push(DiscreteSummary {
            name: format!("first_trigger:{component}"),
            n,
            categories: counts
                .into_iter()
                .map(|(k, c)| (k, proportion(c, n)))
                .collect(),
        });
    }
    let series_names: BTreeSet<&String> = series.iter().flat_map(|s| s.keys()).collect();
    let mut series_bands = Vec::new();
    for name in series_names {
        let mut band = SeriesBand {
            name: name.clone(),
            n: Vec::new(),
            p5: Vec::new(),
            p50: Vec::new(),
            p95: Vec::new(),
        };
        for k in 0..grid.len() {
            let sorted = sorted_finite(
                series
                    .iter()
                    .filter_map(|s| s.get(name)?.get(k).copied().flatten()),
            );
            band.n.push(sorted.len() as u32);
            if sorted.is_empty() {
                band.p5.push(None);
                band.p50.push(None);
                band.p95.push(None);
            } else {
                band.p5.push(Some(quantile_sorted(&sorted, 0.05)));
                band.p50.push(Some(quantile_sorted(&sorted, 0.50)));
                band.p95.push(Some(quantile_sorted(&sorted, 0.95)));
            }
        }
        series_bands.push(band);
    }
    EnsembleSummary {
        continuous,
        discrete,
        time_grid_s: grid,
        series_bands,
    }
}

fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1))
        .unwrap_or(1)
        .max(1)
}

fn not_evaluated(
    rates: &TransportDrivingRates,
    seed: u64,
    settings: &EnsembleSettings,
    accepted: u32,
    rejections: u64,
    why: String,
    next_step: &str,
) -> HistoryEnsemble {
    HistoryEnsemble {
        method: ENSEMBLE_METHOD_ID.into(),
        status: EnsembleStatus::NotEvaluated {
            why,
            next_step: next_step.into(),
        },
        seed,
        samples_requested: settings.samples,
        samples_accepted: accepted,
        rejections,
        scenario_sha256: rates.scenario_sha256.clone(),
        transport_artifact_sha256: rates.transport_artifact_sha256.clone(),
        covariance_method: rates.covariance.as_ref().map(|c| c.method.clone()),
        nominal: None,
        samples: Vec::new(),
        summary: None,
        scope: SCOPE_NOTICE.into(),
    }
}

/// Run the ensemble. `progress` receives (completed, total) after each sample
/// from any worker thread. Cancellation is checked between samples and inside
/// each history; a cancelled ensemble returns an error, never partial results.
pub fn run_history_ensemble(
    rates: &TransportDrivingRates,
    assumptions: &OperatingHistoryAssumptions,
    settings: &EnsembleSettings,
    cancellation: &Cancellation,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<HistoryEnsemble, EnsembleError> {
    let invalid = EnsembleError::Invalid;
    if settings.samples == 0 || settings.samples > MAX_SAMPLES {
        return Err(invalid(format!(
            "samples must be between 1 and {MAX_SAMPLES}"
        )));
    }
    if settings.threads == Some(0) {
        return Err(invalid("threads must be at least 1".into()));
    }
    assumptions.validate().map_err(invalid)?;
    rates.validate().map_err(invalid)?;
    let seed = match settings.seed {
        Some(seed) => seed,
        None => derive_seed(&rates.transport_artifact_sha256, assumptions).map_err(invalid)?,
    };
    if rates.covariance.is_none() {
        return Ok(not_evaluated(
            rates,
            seed,
            settings,
            0,
            0,
            NO_COVARIANCE_WHY.into(),
            NO_COVARIANCE_NEXT,
        ));
    }
    let sampler = Sampler::new(rates).map_err(invalid)?;
    let total = settings.samples as usize;
    let draws: Vec<Draw> = (0..total).map(|i| sampler.draw(seed, i as u64)).collect();
    let rejections: u64 = draws.iter().map(|d| u64::from(d.rejections)).sum();
    let accepted = draws.iter().filter(|d| d.accepted).count();
    if accepted < total || rejections as f64 > MAX_REJECTION_FRACTION * accepted as f64 {
        let percent = 100.0 * rejections as f64 / (rejections as usize + accepted) as f64;
        return Ok(not_evaluated(
            rates,
            seed,
            settings,
            accepted as u32,
            rejections,
            format!(
                "Gaussian sampling of the transport means gave non-physical rates in {percent:.2} % of draws; the relative errors are too large for this approximation"
            ),
            REJECTION_NEXT,
        ));
    }

    let nominal_run =
        run_operating_history_cancellable(assumptions, rates, cancellation).map_err(|e| {
            if cancellation.is_cancelled() {
                EnsembleError::Cancelled
            } else {
                EnsembleError::Invalid(e)
            }
        })?;
    let nominal_values: Vec<f64> = rates
        .covariance_entries()
        .iter()
        .map(|e| e.1.mean)
        .collect();
    let nominal = outcome_of(0, &nominal_values, rates, 0, &nominal_run);

    let grid = time_grid(assumptions.horizon_s);
    let threads = settings
        .threads
        .unwrap_or_else(default_threads)
        .clamp(1, total);
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let results: Mutex<Vec<SampleResult>> = Mutex::new(Vec::with_capacity(total));
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    if cancellation.is_cancelled() || failed.load(Ordering::Acquire) {
                        break;
                    }
                    let i = next.fetch_add(1, Ordering::AcqRel);
                    if i >= total {
                        break;
                    }
                    let draw = &draws[i];
                    let sampled = with_sampled_means(rates, &draw.values);
                    let result = run_operating_history_cancellable(
                        assumptions,
                        &sampled,
                        cancellation,
                    )
                    .map(|run| {
                        (
                            outcome_of(i as u32 + 1, &draw.values, rates, draw.rejections, &run),
                            series_of(&run, &grid),
                        )
                    });
                    match &result {
                        Ok(_) => {
                            let completed = done.fetch_add(1, Ordering::AcqRel) + 1;
                            progress(completed, total);
                        }
                        Err(_) => failed.store(true, Ordering::Release),
                    }
                    results.lock().unwrap().push((i, result));
                }
            });
        }
    });
    let mut collected = results.into_inner().unwrap();
    if cancellation.is_cancelled() {
        return Err(EnsembleError::Cancelled);
    }
    collected.sort_by_key(|(i, _)| *i);
    let mut samples = Vec::with_capacity(total);
    let mut series = Vec::with_capacity(total);
    for (_, result) in collected {
        let (outcome, s) = result.map_err(invalid)?;
        samples.push(outcome);
        series.push(s);
    }
    if samples.len() != total {
        return Err(invalid(
            "history ensemble ended before all samples ran".into(),
        ));
    }
    let summary = summarise(&samples, &series, grid);
    Ok(HistoryEnsemble {
        method: ENSEMBLE_METHOD_ID.into(),
        status: EnsembleStatus::Evaluated,
        seed,
        samples_requested: settings.samples,
        samples_accepted: accepted as u32,
        rejections,
        scenario_sha256: rates.scenario_sha256.clone(),
        transport_artifact_sha256: rates.transport_artifact_sha256.clone(),
        covariance_method: rates.covariance.as_ref().map(|c| c.method.clone()),
        nominal: Some(nominal),
        samples,
        summary: Some(summary),
        scope: SCOPE_NOTICE.into(),
    })
}

// ---------------------------------------------------------------------------
// Comparison of two ensembles

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PairedOutput {
    pub name: String,
    pub n_pairs: u32,
    /// B minus A, per pair.
    pub difference_mean: f64,
    pub difference_p5: QuantileEstimate,
    pub difference_p50: QuantileEstimate,
    pub difference_p95: QuantileEstimate,
    pub a_less_than_b: Proportion,
    pub a_equal_to_b: Proportion,
    pub a_greater_than_b: Proportion,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EnsembleComparison {
    pub method: String,
    pub pairs: u32,
    pub outputs: Vec<PairedOutput>,
    pub scope: String,
}

/// Pair sample i of A with sample i of B. Valid only for ensembles from
/// independent transport runs (their streams are independent); equal seeds or
/// the same transport artifact are refused because the pairs would then be
/// correlated or identical.
pub fn compare_ensembles(
    a: &HistoryEnsemble,
    b: &HistoryEnsemble,
) -> Result<EnsembleComparison, String> {
    if a.status != EnsembleStatus::Evaluated || b.status != EnsembleStatus::Evaluated {
        return Err("both ensembles must be evaluated to be compared".into());
    }
    if a.samples.len() != b.samples.len() || a.samples.is_empty() {
        return Err("ensembles must have the same, nonzero number of samples".into());
    }
    if a.seed == b.seed || a.transport_artifact_sha256 == b.transport_artifact_sha256 {
        return Err(
            "paired comparison needs independent transport runs with different seeds".into(),
        );
    }
    let out_a: Vec<_> = a.samples.iter().map(|s| s.scalar_outputs()).collect();
    let out_b: Vec<_> = b.samples.iter().map(|s| s.scalar_outputs()).collect();
    let names: BTreeSet<&String> = out_a[0]
        .keys()
        .filter(|k| out_b[0].contains_key(*k))
        .collect();
    let mut outputs = Vec::new();
    for name in names {
        let pairs: Vec<(f64, f64)> = out_a
            .iter()
            .zip(&out_b)
            .filter_map(|(x, y)| Some((*x.get(name)?, *y.get(name)?)))
            .collect();
        if pairs.is_empty() {
            continue;
        }
        let n = pairs.len() as u32;
        let diffs = sorted_finite(pairs.iter().map(|(x, y)| y - x));
        let count =
            |f: fn(f64, f64) -> bool| pairs.iter().filter(|(x, y)| f(*x, *y)).count() as u32;
        outputs.push(PairedOutput {
            name: name.clone(),
            n_pairs: n,
            difference_mean: diffs.iter().sum::<f64>() / f64::from(n),
            difference_p5: quantile_estimate(&diffs, 0.05),
            difference_p50: quantile_estimate(&diffs, 0.50),
            difference_p95: quantile_estimate(&diffs, 0.95),
            a_less_than_b: proportion(count(|x, y| x < y), n),
            a_equal_to_b: proportion(count(|x, y| x == y), n),
            a_greater_than_b: proportion(count(|x, y| x > y), n),
        });
    }
    Ok(EnsembleComparison {
        method: ENSEMBLE_METHOD_ID.into(),
        pairs: a.samples.len() as u32,
        outputs,
        scope: SCOPE_NOTICE.into(),
    })
}

#[cfg(test)]
mod tests;
