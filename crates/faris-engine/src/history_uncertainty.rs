//! Presentation of the history ensemble: the identity that decides whether a
//! stored ensemble may be reused, an in-memory cache of finished ensembles, and
//! the plain-language text for ranges, distributions and paired comparisons.
//!
//! The desktop views, the study file and the exported brief all read these
//! functions, so the wording and the numbers cannot differ between them. Every
//! statement here carries transport Monte Carlo sampling uncertainty only.

use crate::brief::Arrangement;
use crate::history::{
    HISTORY_PROCESSING_MODEL_ID, HistoryResult, JULIAN_YEAR_SECONDS, TransportDrivingRates,
};
use crate::history_ensemble::{
    ContinuousSummary, DiscreteSummary, EnsembleComparison, EnsembleStatus, HistoryEnsemble,
    PairedOutput, Proportion, QuantileEstimate, SampleOutcome, derive_seed, nominal_sample,
};
use faris_model::history::OperatingHistoryAssumptions;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::sync::Arc;

/// One line, shown wherever a range is shown.
pub const SCOPE_LINE: &str = "Transport Monte Carlo sampling uncertainty only, not nuclear data, model or assumption uncertainty.";

/// What the line leaves out and what the range does cover.
pub const SCOPE_DETAIL: &str = "These ranges come from re-running the operating history on rates drawn from the recorded transport means and their covariance, so they show how far the sampling noise of the transport run moves each result. They do not include nuclear data, model form, tritium half-life or authored-assumption uncertainty, and no service life or net-electricity figure is qualified by them.";

/// Number of finished ensembles kept in memory.
const CACHE_LIMIT: usize = 24;

/// Everything a stored ensemble depends on. Two ensembles with equal keys are
/// the same calculation, so a stored one may be reused only on an exact match.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnsembleKey {
    /// Ensemble method identity.
    pub method: String,
    /// History ledger identity; a change of ledger changes every result.
    pub history_model: String,
    pub samples: u32,
    /// Written as text so no reader rounds a 64-bit value through a float.
    #[serde(with = "seed_text")]
    pub seed: u64,
    /// SHA-256 of the driving rates as JSON, covariance included.
    pub rates_sha256: String,
    /// SHA-256 of the operating assumptions as JSON.
    pub assumptions_sha256: String,
}

mod seed_text {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(seed: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&seed.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse()
            .map_err(|_| D::Error::custom("seed is not a 64-bit whole number"))
    }
}

fn sha256_json<T: Serialize>(value: &T) -> Result<String, String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

impl EnsembleKey {
    /// The key of the ensemble `run_history_ensemble` would run for these
    /// inputs with the default (derived) seed.
    pub fn new(
        rates: &TransportDrivingRates,
        assumptions: &OperatingHistoryAssumptions,
        samples: u32,
    ) -> Result<Self, String> {
        Ok(Self {
            method: crate::history_ensemble::ENSEMBLE_METHOD_ID.into(),
            history_model: HISTORY_PROCESSING_MODEL_ID.into(),
            samples,
            seed: derive_seed(&rates.transport_artifact_sha256, assumptions)?,
            rates_sha256: sha256_json(rates)?,
            assumptions_sha256: sha256_json(assumptions)?,
        })
    }

    /// Whether `ensemble` could have been produced under this key. This checks
    /// what the ensemble records about itself; the rates and assumptions are
    /// bound by the hashes only when the key was built from them.
    pub fn describes(&self, ensemble: &HistoryEnsemble) -> bool {
        ensemble.method == self.method
            && ensemble.seed == self.seed
            && ensemble.samples_requested == self.samples
    }
}

/// Finished ensembles in memory, found only by their exact key.
#[derive(Default)]
pub struct EnsembleCache {
    entries: Vec<(EnsembleKey, Arc<HistoryEnsemble>)>,
}

impl EnsembleCache {
    pub fn get(&self, key: &EnsembleKey) -> Option<Arc<HistoryEnsemble>> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, e)| e.clone())
    }

    pub fn contains(&self, key: &EnsembleKey) -> bool {
        self.entries.iter().any(|(k, _)| k == key)
    }

    /// Store an ensemble; the oldest entry goes when the cache is full.
    pub fn insert(&mut self, key: EnsembleKey, ensemble: Arc<HistoryEnsemble>) {
        self.entries.retain(|(k, _)| *k != key);
        self.entries.push((key, ensemble));
        if self.entries.len() > CACHE_LIMIT {
            self.entries.remove(0);
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &(EnsembleKey, Arc<HistoryEnsemble>)> {
        self.entries.iter()
    }
}

// ---------------------------------------------------------------------------
// Number and range text

/// A fraction as whole percent text: "62 %", with "<1 %" and ">99 %" at the
/// ends so a rare outcome is never rounded to nothing or to certainty.
pub fn percent_text(fraction: f64) -> String {
    let p = fraction * 100.0;
    if p <= 0.0 {
        "0 %".into()
    } else if p < 1.0 {
        "<1 %".into()
    } else if p > 99.0 && p < 100.0 {
        ">99 %".into()
    } else {
        format!("{p:.0} %")
    }
}

fn percent_number(fraction: f64) -> String {
    let text = percent_text(fraction);
    text.trim_end_matches(" %").to_string()
}

/// "(95 % interval 55–69 %)" for one proportion.
pub fn interval_text(p: &Proportion) -> String {
    format!(
        "95 % interval {}–{} %",
        percent_number(p.wilson95_low),
        percent_number(p.wilson95_high)
    )
}

/// How a continuous output is shown: its unit, the factor from the engine's
/// unit to it, and the decimals.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Unit {
    pub symbol: &'static str,
    pub per_engine_unit: f64,
    pub decimals: usize,
}

pub const YEARS: Unit = Unit {
    symbol: "y",
    per_engine_unit: 1.0 / JULIAN_YEAR_SECONDS,
    decimals: 2,
};
pub const KILOGRAMS: Unit = Unit {
    symbol: "kg",
    per_engine_unit: 1.0,
    decimals: 3,
};
pub const TERAWATT_HOURS: Unit = Unit {
    symbol: "TWh",
    per_engine_unit: 1.0e-6,
    decimals: 3,
};

impl Unit {
    pub fn value(self, engine_value: f64) -> String {
        format!("{:.*}", self.decimals, engine_value * self.per_engine_unit)
    }

    pub fn with_symbol(self, engine_value: f64) -> String {
        format!("{} {}", self.value(engine_value), self.symbol)
    }
}

/// "16.80–18.01 y", or "-1.90 to 0.90 y" when a bound is negative.
pub fn range_text(unit: Unit, low: f64, high: f64) -> String {
    let (lo, hi) = (unit.value(low), unit.value(high));
    if lo.starts_with('-') || hi.starts_with('-') {
        format!("{lo} to {hi} {}", unit.symbol)
    } else {
        format!("{lo}–{hi} {}", unit.symbol)
    }
}

/// "median 17.42 y, P5–P95 16.80–18.01 y".
pub fn median_range_text(unit: Unit, s: &ContinuousSummary) -> String {
    format!(
        "median {}, P5–P95 {}",
        unit.with_symbol(s.p50.value),
        range_text(unit, s.p5.value, s.p95.value)
    )
}

fn quantile_detail(name: &str, unit: Unit, q: &QuantileEstimate) -> String {
    match (q.ci95_low, q.ci95_high) {
        (Some(lo), Some(hi)) => format!(
            "{name} {} (95 % interval {})",
            unit.with_symbol(q.value),
            range_text(unit, lo, hi)
        ),
        _ => format!(
            "{name} {} (too few samples for a 95 % interval)",
            unit.with_symbol(q.value)
        ),
    }
}

/// Hover text of a continuous output: each quantile with its own sampling
/// interval, the mean and the number of samples.
pub fn continuous_detail(unit: Unit, s: &ContinuousSummary, total: u32) -> String {
    let mut text = format!(
        "{}\n{}\n{}\nmean {}\n{} of {total} samples have this value.",
        quantile_detail("P5", unit, &s.p5),
        quantile_detail("median", unit, &s.p50),
        quantile_detail("P95", unit, &s.p95),
        unit.with_symbol(s.mean),
        s.n
    );
    if s.n < total {
        text.push_str(
            " The others never reach it in the horizon, so the range is for the samples that do.",
        );
    }
    text
}

// ---------------------------------------------------------------------------
// Distributions of discrete outputs

/// Categories of a discrete output with a non-zero count, most frequent
/// first; ties keep the category order.
fn ranked(summary: &DiscreteSummary) -> Vec<(&String, &Proportion)> {
    let mut list: Vec<_> = summary
        .categories
        .iter()
        .filter(|(_, p)| p.count > 0)
        .collect();
    list.sort_by_key(|a| std::cmp::Reverse(a.1.count));
    list
}

/// "Magnet swaps: 4 in 62 % of samples, 5 in 31 %, 3 in 7 %".
pub fn count_distribution_sentence(label: &str, summary: &DiscreteSummary) -> String {
    category_sentence(label, summary, &|value| value.to_string())
}

fn category_sentence(
    label: &str,
    summary: &DiscreteSummary,
    name: &dyn Fn(&str) -> String,
) -> String {
    let parts: Vec<String> = ranked(summary)
        .iter()
        .enumerate()
        .map(|(i, (value, p))| {
            let value = name(value);
            if i == 0 {
                format!("{value} in {} of samples", percent_text(p.fraction))
            } else {
                format!("{value} in {}", percent_text(p.fraction))
            }
        })
        .collect();
    format!("{label}: {}", parts.join(", "))
}

/// One line per category with its Wilson interval, for the hover.
pub fn count_distribution_detail(summary: &DiscreteSummary) -> String {
    category_detail(summary, &|value| value.to_string())
}

fn category_detail(summary: &DiscreteSummary, name: &dyn Fn(&str) -> String) -> String {
    let mut lines: Vec<String> = ranked(summary)
        .iter()
        .map(|(value, p)| {
            format!(
                "{}: {} of samples ({}/{}; {})",
                name(value),
                percent_text(p.fraction),
                p.count,
                summary.n,
                interval_text(p)
            )
        })
        .collect();
    lines.push("Intervals are Wilson 95 % intervals for the share of samples.".into());
    lines.join("\n")
}

/// The region a service-limit response names, in words: "port sector" for
/// `magnets-port-sector-fast-flux`; "no swap" for the samples that never
/// reached a limit.
pub fn trigger_region_text(component: &str, response_id: &str) -> String {
    if response_id == "none" {
        return "no swap".into();
    }
    let trimmed = response_id
        .strip_prefix(&format!("{component}-"))
        .unwrap_or(response_id);
    let trimmed = trimmed.strip_suffix("-fast-flux").unwrap_or(trimmed);
    trimmed.replace('-', " ")
}

/// "Magnet swap triggered first by: port sector in 97 % of samples, inboard
/// in 3 %".
pub fn trigger_distribution_sentence(
    component: &str,
    label: &str,
    summary: &DiscreteSummary,
) -> String {
    category_sentence(label, summary, &|v| trigger_region_text(component, v))
}

pub fn trigger_distribution_detail(component: &str, summary: &DiscreteSummary) -> String {
    category_detail(summary, &|v| trigger_region_text(component, v))
}

fn status_phrase(category: &str) -> &'static str {
    match category {
        "horizon_completed" => "runs to the end of the horizon",
        "fuel_limited_at_horizon" => "ends the horizon short of fuel",
        "permanent_component_limit" => "stops at a permanent component limit",
        _ => "has an unlisted outcome",
    }
}

/// Name of a terminal status as the history engine records it.
pub fn outcome_category(outcome: &crate::history::HistoryOutcome) -> &'static str {
    match outcome {
        crate::history::HistoryOutcome::HorizonCompleted => "horizon_completed",
        crate::history::HistoryOutcome::FuelLimitedAtHorizon => "fuel_limited_at_horizon",
        crate::history::HistoryOutcome::PermanentComponentLimit => "permanent_component_limit",
    }
}

/// Nominal terminal status in words: "runs to the end of the horizon".
pub fn outcome_phrase(outcome: &crate::history::HistoryOutcome) -> &'static str {
    status_phrase(outcome_category(outcome))
}

/// "Outcome: runs to the end of the horizon in 98 % of samples, stops at a
/// permanent component limit in 2 %".
pub fn status_distribution_sentence(label: &str, summary: &DiscreteSummary) -> String {
    let parts: Vec<String> = ranked(summary)
        .iter()
        .enumerate()
        .map(|(i, (category, p))| {
            if i == 0 {
                format!(
                    "{} in {} of samples",
                    status_phrase(category),
                    percent_text(p.fraction)
                )
            } else {
                format!(
                    "{} in {}",
                    status_phrase(category),
                    percent_text(p.fraction)
                )
            }
        })
        .collect();
    format!("{label}: {}", parts.join(", "))
}

pub fn status_distribution_detail(summary: &DiscreteSummary) -> String {
    let mut lines: Vec<String> = ranked(summary)
        .iter()
        .map(|(category, p)| {
            format!(
                "{}: {} of samples ({}/{}; {})",
                status_phrase(category),
                percent_text(p.fraction),
                p.count,
                summary.n,
                interval_text(p)
            )
        })
        .collect();
    lines.push("Intervals are Wilson 95 % intervals for the share of samples.".into());
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Rows: every end-of-history output beside its nominal value

/// One history output: the nominal value and, when an ensemble ran, its
/// uncertainty text. `result` is None when the ensemble was not evaluated.
#[derive(Clone, Debug, PartialEq)]
pub struct UncertaintyRow {
    /// Stable name: the engine's output name, or `terminal_status`.
    pub name: String,
    pub label: String,
    /// The nominal value with its unit, as text.
    pub nominal: String,
    pub result: Option<RowResult>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RowResult {
    /// The range or distribution sentence.
    pub text: String,
    /// Longer text for the hover.
    pub detail: String,
}

/// "Magnet swaps" for the replaceable magnet envelope; other components by id.
pub fn replacement_label(component: &str) -> String {
    match component {
        "magnets" => "Magnet swaps".into(),
        "blanket" => "Blanket replacements".into(),
        other => format!("{other} replacements"),
    }
}

fn trigger_label(component: &str) -> String {
    match component {
        "magnets" => "Magnet swap triggered first by".into(),
        "blanket" => "Blanket replacement triggered first by".into(),
        other => format!("{other} replacement triggered first by"),
    }
}

fn first_label(component: &str) -> String {
    match component {
        "magnets" => "First magnet swap".into(),
        "blanket" => "First blanket replacement".into(),
        other => format!("First {other} replacement"),
    }
}

/// The rows for one arrangement. `nominal` is the history's own outcome
/// (`history_ensemble::nominal_sample`); `ensemble` is None while nothing has
/// been calculated and a not-evaluated ensemble gives rows without results.
pub fn uncertainty_rows(
    nominal: &SampleOutcome,
    ensemble: Option<&HistoryEnsemble>,
) -> Vec<UncertaintyRow> {
    let evaluated = ensemble
        .filter(|e| e.status == EnsembleStatus::Evaluated)
        .and_then(|e| e.summary.as_ref().map(|s| (e, s)));
    let samples = evaluated.map_or(0, |(e, _)| e.samples.len() as u32);
    let continuous = |name: &str, unit: Unit| -> Option<RowResult> {
        let (_, summary) = evaluated?;
        let s = summary.continuous.iter().find(|c| c.name == name)?;
        let mut text = median_range_text(unit, s);
        if s.n < samples {
            text.push_str(&format!(" ({} of {samples} samples)", s.n));
        }
        Some(RowResult {
            text,
            detail: continuous_detail(unit, s, samples),
        })
    };
    let mut rows = Vec::new();
    rows.push(UncertaintyRow {
        name: "full_power_time_s".into(),
        label: "Full-power time".into(),
        nominal: YEARS.with_symbol(nominal.full_power_time_s),
        result: continuous("full_power_time_s", YEARS),
    });
    rows.push(UncertaintyRow {
        name: "final_available_tritium_kg".into(),
        label: "Usable tritium at the end".into(),
        nominal: KILOGRAMS.with_symbol(nominal.final_available_tritium_kg),
        result: continuous("final_available_tritium_kg", KILOGRAMS),
    });
    if let Some(net) = nominal.cumulative_net_electricity_mwh {
        rows.push(UncertaintyRow {
            name: "cumulative_net_electricity_mwh".into(),
            label: "Net electricity over the horizon".into(),
            nominal: TERAWATT_HOURS.with_symbol(net),
            result: continuous("cumulative_net_electricity_mwh", TERAWATT_HOURS),
        });
    }
    for (id, first) in &nominal.first_replacement_time_s {
        rows.push(UncertaintyRow {
            name: format!("first_replacement_time_s:{id}"),
            label: first_label(id),
            nominal: first.map_or("none in the horizon".into(), |t| YEARS.with_symbol(t)),
            result: continuous(&format!("first_replacement_time_s:{id}"), YEARS).or_else(|| {
                // No sample replaced it: say so rather than show nothing.
                evaluated.map(|_| RowResult {
                    text: format!("none in the horizon in any of {samples} samples"),
                    detail: "No sample reaches this component's service limit within the horizon."
                        .into(),
                })
            }),
        });
    }
    for (id, count) in &nominal.replacements {
        let name = format!("replacements:{id}");
        let label = replacement_label(id);
        let result = evaluated.and_then(|(_, s)| {
            let d = s.discrete.iter().find(|d| d.name == name)?;
            Some(RowResult {
                text: count_distribution_sentence(&label, d),
                detail: count_distribution_detail(d),
            })
        });
        rows.push(UncertaintyRow {
            name,
            label,
            nominal: count.to_string(),
            result,
        });
    }
    let status = evaluated.and_then(|(_, s)| {
        let d = s.discrete.iter().find(|d| d.name == "terminal_status")?;
        Some(RowResult {
            text: status_distribution_sentence("Outcome", d),
            detail: status_distribution_detail(d),
        })
    });
    rows.push(UncertaintyRow {
        name: "terminal_status".into(),
        label: "Outcome at the horizon".into(),
        nominal: outcome_phrase(&nominal.outcome).into(),
        result: status,
    });
    rows
}

/// The rows for a calculated history: [`uncertainty_rows`] plus, for each
/// component with several region service limits, which region reached its
/// limit first. They sit before the outcome row.
pub fn history_rows(
    history: &HistoryResult,
    ensemble: Option<&HistoryEnsemble>,
) -> Vec<UncertaintyRow> {
    let Some(nominal) = nominal_sample(history) else {
        return Vec::new();
    };
    let mut rows = uncertainty_rows(&nominal, ensemble);
    let regional: Vec<&str> = history
        .assumptions
        .service_limits
        .iter()
        .filter(|l| l.metric == crate::history::FAST_FLUX_REGION_METRIC)
        .map(|l| l.component_id.as_str())
        .collect();
    let at = rows.len().saturating_sub(1);
    rows.splice(at..at, trigger_rows(&nominal, ensemble, &regional));
    rows
}

/// Rows naming the region that reached its limit first, for the components in
/// `regional`.
pub fn trigger_rows(
    nominal: &SampleOutcome,
    ensemble: Option<&HistoryEnsemble>,
    regional: &[&str],
) -> Vec<UncertaintyRow> {
    let evaluated = ensemble
        .filter(|e| e.status == EnsembleStatus::Evaluated)
        .and_then(|e| e.summary.as_ref());
    let mut components: Vec<&str> = Vec::new();
    for id in regional {
        if !components.contains(id) {
            components.push(id);
        }
    }
    components
        .into_iter()
        .map(|id| {
            let name = format!("first_trigger:{id}");
            let label = trigger_label(id);
            let result = evaluated.and_then(|s| {
                let d = s.discrete.iter().find(|d| d.name == name)?;
                Some(RowResult {
                    text: trigger_distribution_sentence(id, &label, d),
                    detail: trigger_distribution_detail(id, d),
                })
            });
            UncertaintyRow {
                name,
                label,
                nominal: nominal
                    .first_trigger_response
                    .get(id)
                    .map_or("none in the horizon".into(), |r| trigger_region_text(id, r)),
                result,
            }
        })
        .collect()
}

/// The ensemble's why and next step when it was not evaluated.
pub fn not_evaluated_text(ensemble: &HistoryEnsemble) -> Option<(&str, &str)> {
    match &ensemble.status {
        EnsembleStatus::NotEvaluated { why, next_step } => Some((why, next_step)),
        EnsembleStatus::Evaluated => None,
    }
}

/// "Uncertainty: 120 of 200 samples".
pub fn progress_text(done: usize, total: usize) -> String {
    format!("Uncertainty: {done} of {total} samples")
}

/// The three plotted series that carry a band, with the engine's series name.
pub const BAND_FLUENCE_MAGNETS: &str = "fluence_n_m2:magnets";
/// The magnet exposure toward its fast-flux region limits, when it has them.
pub const BAND_LIMIT_FLUENCE_MAGNETS: &str = "limit_fluence_n_m2:magnets";
pub const BAND_TRITIUM: &str = "available_tritium_kg";
pub const BAND_NET_ELECTRICITY: &str = "cumulative_net_electricity_mwh";

/// Hover text for a band point: how many samples have a value there. Empty
/// when every sample does.
pub fn band_coverage_note(n_at_point: u32, samples: u32) -> Option<String> {
    (n_at_point < samples).then(|| {
        format!(
            "Band from {n_at_point} of {samples} samples here: the rest have no value at this time (after a permanent stop, or across a swap)."
        )
    })
}

// ---------------------------------------------------------------------------
// Paired comparison

/// One of the four contrasts of the comparison: the second arrangement minus
/// the first, with names that read in a sentence.
#[derive(Clone, Copy, Debug)]
pub struct HistoryContrast {
    pub title: &'static str,
    pub a: Arrangement,
    pub b: Arrangement,
    pub a_name: &'static str,
    pub b_name: &'static str,
}

const fn arrangement(port: bool, breeder: bool) -> Arrangement {
    Arrangement { port, breeder }
}

/// The contrasts of the comparison view and the exported brief, in their order.
pub const CONTRASTS: [HistoryContrast; 4] = [
    HistoryContrast {
        title: "Breeder-heavy − Reference · with port",
        a: arrangement(true, false),
        b: arrangement(true, true),
        a_name: "Reference with port",
        b_name: "Breeder-heavy with port",
    },
    HistoryContrast {
        title: "Breeder-heavy − Reference · no port",
        a: arrangement(false, false),
        b: arrangement(false, true),
        a_name: "Reference without port",
        b_name: "Breeder-heavy without port",
    },
    HistoryContrast {
        title: "Port − No port · reference",
        a: arrangement(false, false),
        b: arrangement(true, false),
        a_name: "Reference without port",
        b_name: "Reference with port",
    },
    HistoryContrast {
        title: "Port − No port · breeder-heavy",
        a: arrangement(false, true),
        b: arrangement(true, true),
        a_name: "Breeder-heavy without port",
        b_name: "Breeder-heavy with port",
    },
];

/// One line of the history comparison between two arrangements.
#[derive(Clone, Debug, PartialEq)]
pub struct ComparisonLine {
    pub name: String,
    pub label: String,
    /// "median 0.31 TWh, P5–P95 -0.20 to 0.80 TWh" for B minus A.
    pub difference: String,
    /// For counts: "A has fewer … than B in 38 % of paired samples (…)".
    pub sentence: Option<String>,
    pub detail: String,
}

fn paired_unit(name: &str) -> Option<(Unit, String)> {
    match name {
        "full_power_time_s" => Some((YEARS, "Full-power time".into())),
        "final_available_tritium_kg" => Some((KILOGRAMS, "Usable tritium at the end".into())),
        "cumulative_net_electricity_mwh" => {
            Some((TERAWATT_HOURS, "Net electricity over the horizon".into()))
        }
        _ => name
            .strip_prefix("first_replacement_time_s:")
            .map(|id| (YEARS, first_label(id))),
    }
}

fn paired_detail(p: &PairedOutput, unit: Option<Unit>) -> String {
    let mut lines = vec![format!("{} paired samples.", p.n_pairs)];
    if let Some(unit) = unit {
        lines.push(quantile_detail("P5", unit, &p.difference_p5));
        lines.push(quantile_detail("median", unit, &p.difference_p50));
        lines.push(quantile_detail("P95", unit, &p.difference_p95));
    }
    for (what, share) in [
        ("A < B", &p.a_less_than_b),
        ("A = B", &p.a_equal_to_b),
        ("A > B", &p.a_greater_than_b),
    ] {
        lines.push(format!(
            "{what}: {} ({})",
            percent_text(share.fraction),
            interval_text(share)
        ));
    }
    lines.push("Differences are second arrangement minus first, sample by sample.".into());
    lines.join("\n")
}

/// "Reference has fewer magnet swaps than Breeder-heavy in 38 % of paired
/// samples (95 % interval 32–45 %); the same number in 50 %."
pub fn fewer_swaps_sentence(a: &str, b: &str, noun: &str, p: &PairedOutput) -> String {
    format!(
        "{a} has fewer {noun} than {b} in {} of paired samples ({}); the same number in {}.",
        percent_text(p.a_less_than_b.fraction),
        interval_text(&p.a_less_than_b),
        percent_text(p.a_equal_to_b.fraction)
    )
}

/// The history lines of a paired comparison, with the arrangements named.
pub fn comparison_lines(a: &str, b: &str, comparison: &EnsembleComparison) -> Vec<ComparisonLine> {
    let mut lines = Vec::new();
    let mut counts = Vec::new();
    for p in &comparison.outputs {
        if let Some(id) = p.name.strip_prefix("replacements:") {
            let noun = match id {
                "magnets" => "magnet swaps".to_string(),
                "blanket" => "blanket replacements".to_string(),
                other => format!("{other} replacements"),
            };
            counts.push(ComparisonLine {
                name: p.name.clone(),
                label: replacement_label(id),
                difference: format!(
                    "median {:+.0}, P5–P95 {:+.0} to {:+.0}",
                    p.difference_p50.value, p.difference_p5.value, p.difference_p95.value
                ),
                sentence: Some(fewer_swaps_sentence(a, b, &noun, p)),
                detail: paired_detail(p, None),
            });
            continue;
        }
        let Some((unit, label)) = paired_unit(&p.name) else {
            continue;
        };
        lines.push(ComparisonLine {
            name: p.name.clone(),
            label,
            difference: format!(
                "median {}, P5–P95 {}",
                signed_with_symbol(unit, p.difference_p50.value),
                range_text(unit, p.difference_p5.value, p.difference_p95.value)
            ),
            sentence: None,
            detail: paired_detail(p, Some(unit)),
        });
    }
    counts.extend(lines);
    counts
}

fn signed_with_symbol(unit: Unit, engine_value: f64) -> String {
    format!(
        "{:+.*} {}",
        unit.decimals,
        engine_value * unit.per_engine_unit,
        unit.symbol
    )
}

/// Names of the outputs a paired comparison reports, for tests and tables.
pub fn comparison_output_names(comparison: &EnsembleComparison) -> BTreeSet<&str> {
    comparison.outputs.iter().map(|o| o.name.as_str()).collect()
}

#[cfg(test)]
mod tests;
