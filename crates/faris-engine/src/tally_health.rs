//! Statistical health of recorded transport tallies: reads the worker's per-batch values, runs the
//! statistical checks of `faris_model::tally_statistics`, sets the interval and the digits shown, and
//! refuses an interval and a verdict from a failing tally. Also sums tally parts with the recorded
//! covariance and summarises the convergence of a spectrum or mesh tally.
//!
//! Method notes with sources: `docs/guide/tally-statistics.md`.

use crate::transport::{NormalizedTransportResult, RawEnergySpectrum, ResponseCovariance};
use faris_model::interval_display::{IntervalConvention, display_bounds, format_with_uncertainty};
use faris_model::tally_statistics::{
    CostBasis, ErrorAgreement, MeshConvergence, TallyHealth, TallyInterval, TallyKind, TallyStatus,
    assess_tally, batch_error_agreement, combine_linear, mesh_convergence, tally_interval,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const BATCH_VALUES_SCHEMA: &str = "faris-transport-batch-values/v0.1";
/// Most batches a values file may hold; the reactor worker accepts 30 to 1000.
pub const MAX_BATCHES: usize = 10_000;
/// Confidence level of the interval shown beside every tally.
pub const INTERVAL_LEVEL: f64 = 0.95;

/// The per-batch values file written beside a transport artifact. Fields the worker may add later
/// (`batch_end_seconds`) are read when present; unknown fields are ignored so older readers keep working.
#[derive(Clone, Debug, Deserialize)]
pub struct BatchValuesFile {
    pub schema_version: String,
    pub n_batches: usize,
    pub response_ids: Vec<String>,
    /// Raw tally values per source neutron for each batch, by response id.
    pub values: BTreeMap<String, Vec<f64>>,
    /// Wall seconds at the end of each batch (cumulative), shared by all responses of the run. Absent in
    /// files written before the worker recorded time; the figure of merit is then per batch.
    #[serde(default)]
    pub batch_end_seconds: Option<Vec<f64>>,
}

impl BatchValuesFile {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > faris_model::transport::MAX_ARTIFACT_BYTES {
            return Err("batch-values file exceeds the 16 MiB limit".into());
        }
        let file: BatchValuesFile = serde_json::from_slice(bytes)
            .map_err(|e| format!("batch-values file is not valid: {e}"))?;
        if file.schema_version != BATCH_VALUES_SCHEMA {
            return Err(format!(
                "batch-values schema is {:?}, expected {BATCH_VALUES_SCHEMA:?}; rerun the transport with this FARIS version",
                file.schema_version
            ));
        }
        if file.n_batches == 0 || file.n_batches > MAX_BATCHES {
            return Err(format!(
                "batch count {} is outside 1 to {MAX_BATCHES}",
                file.n_batches
            ));
        }
        for id in &file.response_ids {
            match file.values.get(id) {
                Some(v) if v.len() == file.n_batches => {}
                Some(v) => {
                    return Err(format!(
                        "response {id} has {} batch values but the file declares {}",
                        v.len(),
                        file.n_batches
                    ));
                }
                None => return Err(format!("response {id} is listed but has no batch values")),
            }
        }
        if let Some(t) = &file.batch_end_seconds
            && t.len() != file.n_batches
        {
            return Err(format!(
                "batch_end_seconds has {} entries for {} batches",
                t.len(),
                file.n_batches
            ));
        }
        Ok(file)
    }
}

/// Everything shown about one tally's statistics.
#[derive(Clone, Debug, Serialize)]
pub struct TallyReport {
    pub response_id: String,
    pub health: TallyHealth,
    /// The interval, or the reason there is none. A failing tally is refused here.
    pub interval: TallyInterval,
    /// Mean and 1 σ standard error written with the digits the error supports, convention stated.
    pub value_text: String,
    /// The interval written out with its convention; the reason when there is none.
    pub interval_text: String,
    /// Reported standard error against the standard error of the batch values.
    pub reported_error_check: Option<ErrorAgreement>,
    /// Whether a verdict may rest on this tally.
    pub allows_verdict: bool,
    /// What the statistics do not cover.
    pub excludes: Vec<String>,
}

/// What the batch-based statistics leave out; shown beside every interval.
pub fn tally_statistics_exclusions() -> Vec<String> {
    vec![
        "nuclear data".into(),
        "geometry".into(),
        "model form".into(),
        "heavy tails not yet sampled".into(),
    ]
}

/// Assess every response in a batch-values file. `reported_errors` holds the standard error the
/// artifact reported per response (raw units), checked against the batch-based error.
pub fn assess_batch_values(
    file: &BatchValuesFile,
    reported_errors: &BTreeMap<String, f64>,
    kind: TallyKind,
) -> Vec<TallyReport> {
    file.response_ids
        .iter()
        .map(|id| {
            let values = &file.values[id];
            let health = assess_tally(values, file.batch_end_seconds.as_deref(), kind);
            let excludes = tally_statistics_exclusions();
            let reported_error_check = reported_errors
                .get(id)
                .and_then(|r| batch_error_agreement(*r, values));
            let allows_verdict = health.status.allows_verdict()
                && reported_error_check.as_ref().is_none_or(|c| c.agrees);
            let interval = if allows_verdict {
                tally_interval(
                    health.mean,
                    health.standard_error,
                    health.batches,
                    INTERVAL_LEVEL,
                )
            } else {
                TallyInterval::NotEvaluated {
                    why: refusal_reason(&health, reported_error_check.as_ref()),
                    next_step: "run more histories or use variance reduction, then check again"
                        .into(),
                }
            };
            let value_text = format!(
                "{} ({})",
                format_with_uncertainty(health.mean, health.standard_error).text,
                IntervalConvention::OneSigma.label()
            );
            let interval_text = interval_text(health.mean, &interval, &excludes);
            TallyReport {
                response_id: id.clone(),
                health,
                interval,
                value_text,
                interval_text,
                reported_error_check,
                allows_verdict,
                excludes,
            }
        })
        .collect()
}

fn refusal_reason(health: &TallyHealth, check: Option<&ErrorAgreement>) -> String {
    if !health.status.allows_verdict() {
        return format!("interval refused: {}", health.summary);
    }
    match check {
        Some(c) => format!(
            "interval refused: the reported standard error {:.3e} and the standard error of the batch values {:.3e} disagree (z = {:.1}); the reported error cannot be trusted",
            c.reported, c.independent, c.z
        ),
        None => "interval refused".into(),
    }
}

/// An interval written with its convention and what it excludes, or the reason there is none.
pub fn interval_text(mean: f64, interval: &TallyInterval, excludes: &[String]) -> String {
    match interval {
        TallyInterval::TwoSided {
            lower,
            upper,
            level,
            method,
        } => {
            let text = display_bounds(
                mean,
                *lower,
                *upper,
                IntervalConvention::Confidence { level: *level },
                excludes,
            );
            format!("{text}; {method}")
        }
        TallyInterval::UpperBound { upper, method, .. } => {
            // Two significant digits: an upper bound from a handful of events supports no more.
            let shown = format_with_uncertainty(*upper, *upper * 0.1).value;
            let mut text = format!("below {shown} ({method})");
            if !excludes.is_empty() {
                text.push_str(&format!("; excludes: {}", excludes.join(", ")));
            }
            text
        }
        TallyInterval::NotEvaluated { why, next_step } => {
            format!("NOT_EVALUATED: {why}. Next: {next_step}")
        }
    }
}

/// One line per failing or unevaluated check, for a report. Empty for a passing tally.
pub fn check_lines(report: &TallyReport) -> Vec<String> {
    use faris_model::tally_statistics::CheckState;
    report
        .health
        .checks
        .iter()
        .filter(|c| c.state != CheckState::Pass)
        .map(|c| {
            let state = match c.state {
                CheckState::Fail => "FAIL",
                _ => "NOT_EVALUATED",
            };
            format!("check {} ({}): {state}: {}", c.id, c.name, c.detail)
        })
        .collect()
}

/// Whether the figure of merit of a tally rests on recorded times or on batch counts, in words.
pub fn fom_basis_text(basis: CostBasis) -> &'static str {
    match basis {
        CostBasis::Seconds => "per wall second",
        CostBasis::Batches => "per batch (batch times are not recorded, equal batches assumed)",
    }
}

// ---------------------------------------------------------------------------------------------
// Sum of tally parts

#[derive(Clone, Debug, Serialize)]
pub struct PartShare {
    pub response_id: String,
    pub share_of_sum: f64,
    pub relative_error: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct PartsSum {
    /// Sum of the parts in the responses' integrated unit.
    pub value: f64,
    /// Standard error of the sum from the full recorded covariance.
    pub standard_error: f64,
    /// Standard error if the parts were wrongly taken as independent, to show what the covariance changes.
    pub independent_standard_error: f64,
    pub parts: Vec<PartShare>,
    /// Zones above 5 % of the sum all have a relative error of at most 2 %.
    pub large_zones_within_2_percent: bool,
    /// The sum against the global tally, when one was named.
    pub against_total: Option<TotalCheck>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TotalCheck {
    pub total_id: String,
    pub total: f64,
    /// Sum of parts minus the total.
    pub difference: f64,
    /// Error of that difference with the covariance between sum and total.
    pub difference_standard_error: f64,
    pub z: f64,
    /// `|z| <= 3`.
    pub within_3_sigma: bool,
}

/// Sum of tally parts with the combined error from the run's sampling covariance. The covariance is
/// required: parts of one run share batches, and independence is never assumed. Fails with the reason
/// when a response is missing from the covariance or the record has none.
pub fn sum_of_parts(
    result: &NormalizedTransportResult,
    part_ids: &[&str],
    total_id: Option<&str>,
) -> Result<PartsSum, String> {
    let cov = result.response_covariance.as_ref().ok_or(
        "this transport record has no covariance between its tallies, so the error of a sum cannot be set; independence is never assumed. Rerun the transport with this FARIS version to record the batch-resolved results",
    )?;
    sum_with_covariance(
        cov,
        &|id| {
            result
                .results
                .iter()
                .find(|r| r.response_id == id)
                .map(|r| r.integrated_mean)
        },
        part_ids,
        total_id,
    )
}

/// [`sum_of_parts`] on a covariance and a lookup of the integrated means, so the arithmetic is tested
/// without building a whole normalized record.
pub fn sum_with_covariance(
    cov: &ResponseCovariance,
    integrated_mean: &dyn Fn(&str) -> Option<f64>,
    part_ids: &[&str],
    total_id: Option<&str>,
) -> Result<PartsSum, String> {
    let mut ids: Vec<&str> = part_ids.to_vec();
    if let Some(t) = total_id {
        ids.push(t);
    }
    if part_ids.is_empty() {
        return Err("no parts were named".into());
    }
    let n = cov.response_ids.len();
    let mut index = Vec::with_capacity(ids.len());
    let mut means = Vec::with_capacity(ids.len());
    for id in &ids {
        let at = cov
            .response_ids
            .iter()
            .position(|r| r == id)
            .ok_or_else(|| {
                format!("response {id} is not in the recorded covariance (it must be a scalar response of the same run)")
            })?;
        let mean =
            integrated_mean(id).ok_or_else(|| format!("response {id} is not in the record"))?;
        index.push(at);
        means.push(mean);
    }
    let m = ids.len();
    let mut sub = vec![0.0; m * m];
    for (a, &i) in index.iter().enumerate() {
        for (b, &j) in index.iter().enumerate() {
            sub[a * m + b] = cov.integrated[i * n + j];
        }
    }
    let k = part_ids.len();
    let weights: Vec<f64> = (0..m).map(|i| if i < k { 1.0 } else { 0.0 }).collect();
    let (value, standard_error) =
        combine_linear(&weights, &means, &sub).map_err(|e| e.to_string())?;
    let independent_standard_error = (0..k).map(|i| sub[i * m + i]).sum::<f64>().sqrt();
    let parts: Vec<PartShare> = (0..k)
        .map(|i| PartShare {
            response_id: ids[i].to_string(),
            share_of_sum: if value != 0.0 { means[i] / value } else { 0.0 },
            relative_error: if means[i] != 0.0 {
                sub[i * m + i].sqrt() / means[i].abs()
            } else {
                f64::INFINITY
            },
        })
        .collect();
    let large_zones_within_2_percent = parts
        .iter()
        .filter(|p| p.share_of_sum.abs() > 0.05)
        .all(|p| p.relative_error <= 0.02);
    let against_total = match total_id {
        None => None,
        Some(t) => {
            let mut w = weights.clone();
            w[k] = -1.0;
            let (difference, se) = combine_linear(&w, &means, &sub).map_err(|e| e.to_string())?;
            let z = if se > 0.0 {
                difference / se
            } else if difference == 0.0 {
                0.0
            } else {
                f64::INFINITY
            };
            Some(TotalCheck {
                total_id: t.to_string(),
                total: means[k],
                difference,
                difference_standard_error: se,
                z,
                within_3_sigma: z.abs() <= 3.0,
            })
        }
    };
    Ok(PartsSum {
        value,
        standard_error,
        independent_standard_error,
        parts,
        large_zones_within_2_percent,
        against_total,
    })
}

/// The whole-model tritium production tally, the global tally the zone tallies are summed against.
pub const TRITIUM_TOTAL_ID: &str = "total-tritium-production";

/// What the sum of the per-zone tritium tallies shows, per source neutron (UNC-013).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ZoneSum {
    pub zones: usize,
    /// Sum of the zone tallies and its standard error from the full recorded covariance.
    pub sum: f64,
    pub standard_error: f64,
    /// The global tally the sum is checked against.
    pub total: f64,
    /// Sum minus global tally, its error with the covariance between them, and the ratio.
    pub difference: f64,
    pub difference_standard_error: f64,
    pub z: f64,
    /// `|z| <= 3`.
    pub within_3_sigma: bool,
    /// Zones above 5 % of the sum all have a relative error of at most 2 %.
    pub large_zones_within_2_percent: bool,
}

/// Why a record has no zone sum, shown where the sum would be.
pub const ZONE_SUM_UNAVAILABLE: &str = "NOT_EVALUATED: this record has no covariance between its zone tritium tallies and the whole-model tally, so the error of their sum cannot be set; independence is never assumed. Next: rerun the transport with this FARIS version to record the batch-resolved results";

/// The sum of the `*-tritium` zone tallies of a run against its whole-model tritium tally, scaled to
/// per source neutron. The zones are those the recorded covariance holds; fewer than two, or no
/// whole-model tally in the covariance, is an error with the reason.
pub fn tritium_zone_sum(
    cov: &ResponseCovariance,
    integrated_mean: &dyn Fn(&str) -> Option<f64>,
    source_neutron_rate_per_s: f64,
) -> Result<ZoneSum, String> {
    if !cov.response_ids.iter().any(|id| id == TRITIUM_TOTAL_ID) {
        return Err(format!(
            "{TRITIUM_TOTAL_ID} is not in the recorded covariance"
        ));
    }
    let zones: Vec<&str> = cov
        .response_ids
        .iter()
        .map(String::as_str)
        .filter(|id| id.ends_with("-tritium") && *id != TRITIUM_TOTAL_ID)
        .collect();
    if zones.len() < 2 {
        return Err(format!(
            "{} zone tritium tallies are in the recorded covariance; a sum needs at least two",
            zones.len()
        ));
    }
    if !(source_neutron_rate_per_s.is_finite() && source_neutron_rate_per_s > 0.0) {
        return Err("the source neutron rate is not a positive number".into());
    }
    let sum = sum_with_covariance(cov, integrated_mean, &zones, Some(TRITIUM_TOTAL_ID))?;
    let check = sum
        .against_total
        .ok_or("the whole-model tally was not compared")?;
    let per_source = 1.0 / source_neutron_rate_per_s;
    Ok(ZoneSum {
        zones: zones.len(),
        sum: sum.value * per_source,
        standard_error: sum.standard_error * per_source,
        total: check.total * per_source,
        difference: check.difference * per_source,
        difference_standard_error: check.difference_standard_error * per_source,
        z: check.z,
        within_3_sigma: check.within_3_sigma,
        large_zones_within_2_percent: sum.large_zones_within_2_percent,
    })
}

/// [`tritium_zone_sum`] on a normalized record.
pub fn tritium_zone_sum_of(result: &NormalizedTransportResult) -> Result<ZoneSum, String> {
    let cov = result
        .response_covariance
        .as_ref()
        .ok_or(ZONE_SUM_UNAVAILABLE)?;
    tritium_zone_sum(
        cov,
        &|id| {
            result
                .results
                .iter()
                .find(|r| r.response_id == id)
                .map(|r| r.integrated_mean)
        },
        result.source_neutron_rate_per_s,
    )
}

impl ZoneSum {
    /// The sum with its combined error, one line for the app and the report.
    pub fn sum_line(&self) -> String {
        format!(
            "Sum of {} zone tallies: {} H3/source neutron (error from the recorded covariance, 1 σ)",
            self.zones,
            format_with_uncertainty(self.sum, self.standard_error).text
        )
    }

    /// How the sum compares with the whole-model tally.
    pub fn check_line(&self) -> String {
        let zones = if self.large_zones_within_2_percent {
            "every zone above 5 % of the sum is within 2 % relative error"
        } else {
            "a zone above 5 % of the sum has a relative error above 2 %"
        };
        format!(
            "Whole-model tally {}; difference {} = {:.1} σ, {} 3 σ; {zones}",
            format_with_uncertainty(self.total, self.difference_standard_error).text,
            format_with_uncertainty(self.difference, self.difference_standard_error).text,
            self.z,
            if self.within_3_sigma {
                "within"
            } else {
                "OUTSIDE"
            },
        )
    }
}

// ---------------------------------------------------------------------------------------------
// Convergence across a spectrum or mesh

/// Convergence summary of an energy-binned tally: fractions of bins at 10 % and 5 % relative error,
/// unscored bins counted as not converged.
pub fn spectrum_convergence(spectrum: &RawEnergySpectrum) -> Option<MeshConvergence> {
    mesh_convergence(
        &spectrum.mean_cm_per_source_per_bin,
        &spectrum.standard_error_cm_per_source_per_bin,
    )
}

/// The convergence summary as one line for a report.
pub fn convergence_line(c: &MeshConvergence) -> String {
    format!(
        "{} of {} bins ({:.1} %) have relative error of at most 10 %, {} ({:.1} %) at most 5 %; {} unscored, {} scored with fewer than 1000 effective scoring histories",
        c.within_10_percent,
        c.voxels,
        100.0 * c.fraction_within_10_percent,
        c.within_5_percent,
        100.0 * c.fraction_within_5_percent,
        c.unscored,
        c.low_sample
    )
}

/// Whether a status refuses a verdict, in words for a report.
pub fn status_line(status: TallyStatus) -> &'static str {
    match status {
        TallyStatus::Passing => "PASS: every statistical check passed",
        TallyStatus::PassingWithGaps => {
            "PASS with gaps: no check failed, some could not be evaluated"
        }
        TallyStatus::Failing => {
            "FAIL: a statistical check failed; no verdict may rest on this tally"
        }
        TallyStatus::NotEvaluated => "NOT_EVALUATED: the checks could not run",
    }
}

#[cfg(test)]
mod tests;
