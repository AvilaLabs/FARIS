//! Statistical health of Monte Carlo tallies: the MCNP statistical checks computed from batch values,
//! the variance of the variance, the figure of merit, the effective number of scoring histories, intervals
//! that stay valid at low counts, error from independent batches and seeds, and the error of a sum of
//! tally parts. Pure numerics; every function takes numbers and returns numbers or a stated reason.
//!
//! Method notes with sources: `docs/guide/tally-statistics.md`.

use crate::math;
use serde::Serialize;

/// Relative error guideline for a tally over a region (MCNP: R < 0.10).
pub const REL_ERR_LIMIT_VOLUME: f64 = 0.10;
/// Relative error guideline for a point detector (MCNP: R < 0.05).
pub const REL_ERR_LIMIT_POINT: f64 = 0.05;
/// Variance of the variance limit (MCNP: VOV < 0.1).
pub const VOV_LIMIT: f64 = 0.10;
/// Largest change of the figure of merit over the last half of the run (FARIS choice; MCNP asks for "constant").
pub const FOM_CHANGE_LIMIT: f64 = 0.20;
/// Smallest tally slope (MCNP: slope of the largest history scores greater than 3).
pub const SLOPE_MIN: f64 = 3.0;
/// Fewer effective scoring histories than this is flagged low-sample (FARIS choice).
pub const LOW_SAMPLE_HISTORIES: f64 = 1000.0;
/// Fewer effective scoring events than this gets a one-sided bound, not a two-sided interval (UNC-003).
pub const MIN_EVENTS_TWO_SIDED: f64 = 10.0;
/// Fewest batches the checks need; below this every check is NOT_EVALUATED.
pub const MIN_BATCHES: usize = 10;
/// Most checkpoints used over the last half of a run.
const CHECKPOINTS: usize = 10;

// ---------------------------------------------------------------------------------------------
// Batch statistics

/// Mean of the batch values and the standard error of that mean, `sqrt(sum (x - mean)^2 / (n (n - 1)))`.
/// None below two batches or when a value is not finite.
pub fn mean_and_standard_error(values: &[f64]) -> Option<(f64, f64)> {
    let n = values.len();
    if n < 2 || values.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let mean = values.iter().sum::<f64>() / n as f64;
    let second: f64 = values.iter().map(|x| (x - mean) * (x - mean)).sum();
    let variance = second / (n as f64 * (n as f64 - 1.0));
    Some((mean, variance.max(0.0).sqrt()))
}

/// Variance of the variance of the batch values: `sum (x - m)^4 / (sum (x - m)^2)^2 - 1/n`. Near 1 when one
/// batch holds most of the sum, about `2/n` for normal batches. None below three batches or with no spread.
pub fn variance_of_variance(values: &[f64]) -> Option<f64> {
    let n = values.len();
    if n < 3 || values.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let mean = values.iter().sum::<f64>() / n as f64;
    let second: f64 = values.iter().map(|x| math::powi(x - mean, 2)).sum();
    if second <= 0.0 {
        return None;
    }
    let fourth: f64 = values.iter().map(|x| math::powi(x - mean, 4)).sum();
    Some(fourth / (second * second) - 1.0 / n as f64)
}

/// Effective number of scoring histories, `1 / R^2` with `R` the relative error. A tally scored by N
/// unit-weight histories with scoring efficiency q has `R^2 = (1 - q) / (q N)`, so `1 / R^2` is the count a
/// Poisson tally of the same relative error would have; weighted scores give fewer than the true count.
pub fn effective_scoring_histories(mean: f64, standard_error: f64) -> Option<f64> {
    if !(mean.is_finite() && standard_error.is_finite()) || mean == 0.0 || standard_error <= 0.0 {
        return None;
    }
    let r = standard_error / mean.abs();
    Some(1.0 / (r * r))
}

/// Figure of merit `1 / (R^2 T)` for relative error `R` and cost `T` (seconds, or batches when no time is
/// recorded). None for a zero error or zero cost.
pub fn figure_of_merit(relative_error: f64, cost: f64) -> Option<f64> {
    if relative_error > 0.0 && cost > 0.0 && relative_error.is_finite() && cost.is_finite() {
        Some(1.0 / (relative_error * relative_error * cost))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------------------------
// The ten statistical checks

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CheckState {
    Pass,
    Fail,
    NotEvaluated,
}

/// What kind of tally the relative error guideline applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TallyKind {
    Volume,
    PointDetector,
}

impl TallyKind {
    pub fn relative_error_limit(self) -> f64 {
        match self {
            TallyKind::Volume => REL_ERR_LIMIT_VOLUME,
            TallyKind::PointDetector => REL_ERR_LIMIT_POINT,
        }
    }
}

/// What the figure of merit's cost is measured in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CostBasis {
    /// Cumulative wall seconds at the end of each batch, as recorded by the run.
    Seconds,
    /// Batches, with equal histories per batch and no timing recorded: `1 / (R^2 N)`, which equals the
    /// seconds-based figure only if every batch costs the same time.
    Batches,
}

#[derive(Clone, Debug, Serialize)]
pub struct StatCheck {
    /// MCNP check number, 1 to 10.
    pub id: u8,
    pub name: &'static str,
    pub state: CheckState,
    /// The number the check looked at, when it has one.
    pub value: Option<f64>,
    /// The pass condition in words.
    pub limit: String,
    /// Why it passed, failed or was not evaluated, and what to do next when it did not pass.
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TallyStatus {
    /// Every check passed.
    Passing,
    /// No check failed, but some could not be evaluated (listed on the checks).
    PassingWithGaps,
    /// At least one check failed: no verdict may be drawn from this tally.
    Failing,
    /// Too few batches, or non-finite values: nothing can be said.
    NotEvaluated,
}

impl TallyStatus {
    /// Whether a verdict may rest on the tally. A failing or unevaluated tally refuses.
    pub fn allows_verdict(self) -> bool {
        matches!(self, TallyStatus::Passing | TallyStatus::PassingWithGaps)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct FomSummary {
    pub basis: CostBasis,
    /// Figure of merit of the whole run.
    pub final_value: f64,
    /// (batches, figure of merit) at the checkpoints of the last half.
    pub trend: Vec<(usize, f64)>,
    /// Largest `|FOM_k / FOM_final - 1|` over the checkpoints.
    pub max_change_last_half: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct TallyHealth {
    pub batches: usize,
    pub mean: f64,
    pub standard_error: f64,
    pub relative_error: Option<f64>,
    pub variance_of_variance: Option<f64>,
    pub effective_histories: Option<f64>,
    /// Fewer than `LOW_SAMPLE_HISTORIES` effective scoring histories.
    pub low_sample: bool,
    pub fom: Option<FomSummary>,
    pub checks: Vec<StatCheck>,
    pub status: TallyStatus,
    /// One line saying why the status is what it is and what to do next.
    pub summary: String,
}

/// Batch counts at which the last half of the run is sampled: up to ten, evenly spaced, ending at `n`.
fn checkpoints(n: usize) -> Vec<usize> {
    let start = n.div_ceil(2).max(3);
    let span = n - start;
    let count = (span + 1).min(CHECKPOINTS);
    let mut points: Vec<usize> = if count < 2 {
        vec![n]
    } else {
        (0..count).map(|i| start + span * i / (count - 1)).collect()
    };
    points.dedup();
    points
}

/// Relative standard deviation of an error estimated from `k` batches (normal batches: `1 / sqrt(2 (k - 1))`).
fn relative_error_noise(k: usize) -> f64 {
    1.0 / (2.0 * (k as f64 - 1.0)).sqrt()
}

/// Relative standard deviation of the variance of the variance estimated from `k` batches: the standard
/// deviation of a sample kurtosis is about `sqrt(24 / k)` against a value of about 2, so `sqrt(6 / k)`.
fn vov_noise(k: usize) -> f64 {
    (6.0 / k as f64).sqrt()
}

/// Three standard deviations of the difference of two estimates with relative noise `s`, in log units.
fn log_tolerance(s: f64) -> f64 {
    3.0 * std::f64::consts::SQRT_2 * s
}

fn strictly_monotone(series: &[f64]) -> bool {
    series.len() >= 3
        && (series.windows(2).all(|w| w[1] > w[0]) || series.windows(2).all(|w| w[1] < w[0]))
}

fn check(
    id: u8,
    name: &'static str,
    state: CheckState,
    value: Option<f64>,
    limit: &str,
    detail: String,
) -> StatCheck {
    StatCheck {
        id,
        name,
        state,
        value,
        limit: limit.to_string(),
        detail,
    }
}

fn not_evaluated(id: u8, name: &'static str, limit: &str, why: &str) -> StatCheck {
    check(
        id,
        name,
        CheckState::NotEvaluated,
        None,
        limit,
        why.to_string(),
    )
}

fn pass_fail(ok: bool) -> CheckState {
    if ok {
        CheckState::Pass
    } else {
        CheckState::Fail
    }
}

const TALLY_NAMES: [&str; 10] = [
    "mean has no trend",
    "relative error size",
    "relative error decreases",
    "relative error falls as 1/sqrt(N)",
    "variance of the variance size",
    "variance of the variance decreases",
    "variance of the variance falls as 1/N",
    "figure of merit constant",
    "figure of merit has no trend",
    "tally slope",
];

const LIMITS: [&str; 10] = [
    "no monotonic drift larger than one standard error over the last half",
    "relative error below the guideline",
    "relative error does not rise over the last half beyond sampling noise",
    "relative error falls as 1/sqrt(N) over the last half, within sampling noise",
    "variance of the variance below 0.1",
    "variance of the variance does not rise over the last half beyond sampling noise",
    "variance of the variance falls as 1/N over the last half, within sampling noise",
    "figure of merit within 20 % of its final value over the last half, widened by sampling noise",
    "no steady figure of merit change beyond the same limit over the last half",
    "tally slope above 3",
];

fn unevaluated_all(why: &str) -> Vec<StatCheck> {
    (0..10)
        .map(|i| not_evaluated(i as u8 + 1, TALLY_NAMES[i], LIMITS[i], why))
        .collect()
}

/// Run the ten statistical checks on the per-batch values of one tally.
///
/// `cumulative_seconds` is the wall time at the end of each batch when the run recorded it; without it the
/// figure of merit is per batch (equal batches assumed) and says so. The tally slope (check 10) needs the
/// largest per-history scores, which batch values do not hold, so it is always NOT_EVALUATED here.
pub fn assess_tally(
    values: &[f64],
    cumulative_seconds: Option<&[f64]>,
    kind: TallyKind,
) -> TallyHealth {
    let n = values.len();
    let blank = |why: String, mean: f64, se: f64| TallyHealth {
        batches: n,
        mean,
        standard_error: se,
        relative_error: None,
        variance_of_variance: None,
        effective_histories: None,
        low_sample: false,
        fom: None,
        checks: unevaluated_all(&why),
        status: TallyStatus::NotEvaluated,
        summary: format!("not evaluated: {why}"),
    };
    if values.iter().any(|v| !v.is_finite()) {
        return blank(
            "a batch value is not a finite number; rerun the transport and check the tally output"
                .into(),
            f64::NAN,
            f64::NAN,
        );
    }
    if n < MIN_BATCHES {
        return blank(
            format!(
                "{n} batches are fewer than the {MIN_BATCHES} the checks need; run with more batches (at least 30 is the reactor default)"
            ),
            f64::NAN,
            f64::NAN,
        );
    }
    if let Some(t) = cumulative_seconds
        && (t.len() != n
            || t.iter().any(|v| !v.is_finite() || *v <= 0.0)
            || t.windows(2).any(|w| w[1] <= w[0]))
    {
        return blank(
            "the recorded batch times are not one increasing positive time per batch; rerun so the worker records them".into(),
            f64::NAN,
            f64::NAN,
        );
    }
    let (mean, se) = mean_and_standard_error(values).unwrap_or((f64::NAN, f64::NAN));
    let points = checkpoints(n);
    // Prefix statistics at each checkpoint.
    let prefix: Vec<(usize, f64, f64, Option<f64>)> = points
        .iter()
        .map(|&k| {
            let (m, s) = mean_and_standard_error(&values[..k]).unwrap_or((f64::NAN, f64::NAN));
            (k, m, s, variance_of_variance(&values[..k]))
        })
        .collect();
    let r_of = |m: f64, s: f64| if m != 0.0 { Some(s / m.abs()) } else { None };
    let relative_error = r_of(mean, se);
    let vov = variance_of_variance(values);
    let effective = effective_scoring_histories(mean, se);
    let mut checks = Vec::with_capacity(10);

    // 1. The mean does not drift in the last half.
    {
        let means: Vec<f64> = prefix.iter().map(|p| p.1).collect();
        let change = means.last().unwrap_or(&mean) - means.first().unwrap_or(&mean);
        let drifting = strictly_monotone(&means) && change.abs() > se;
        let detail = if drifting {
            format!(
                "the running mean moved steadily by {:.3e} over the last half, more than its standard error {:.3e}; the mean has not settled, run more batches",
                change.abs(),
                se
            )
        } else {
            "the running mean shows no steady drift in the last half".to_string()
        };
        checks.push(check(
            1,
            TALLY_NAMES[0],
            pass_fail(!drifting),
            Some(change),
            LIMITS[0],
            detail,
        ));
    }

    // 2. Size of the relative error.
    let limit = kind.relative_error_limit();
    match relative_error {
        None => checks.push(check(
            2,
            TALLY_NAMES[1],
            CheckState::Fail,
            None,
            LIMITS[1],
            "the tally mean is zero: no history scored; run more histories or use variance reduction".into(),
        )),
        Some(r) => checks.push(check(
            2,
            TALLY_NAMES[1],
            pass_fail(r < limit),
            Some(r),
            &format!("relative error below {limit}"),
            if r < limit {
                format!("relative error {r:.4} is below {limit}")
            } else {
                format!("relative error {r:.4} is not below {limit}; run more histories or use variance reduction")
            },
        )),
    }

    // 3 and 4. The relative error decreases, and at the 1/sqrt(N) rate. Both compare the half-way point of
    // the run with its end and allow for the sampling noise of an error estimated from `k` batches.
    let k_mid = points[0];
    let k_end = n;
    let steps = math::ln(k_end as f64 / k_mid as f64);
    let r_pair = r_of(prefix[0].1, prefix[0].2)
        .zip(relative_error)
        .filter(|(a, b)| *a > 0.0 && *b > 0.0);
    match r_pair {
        Some((r_mid, r_end)) => {
            let tol = log_tolerance(relative_error_noise(k_mid));
            let change = math::ln(r_end / r_mid);
            let limit3 = format!(
                "relative error at the end of the run not above its value at half the run by more than the sampling noise of the estimate (a factor {:.2})",
                math::exp(tol)
            );
            checks.push(check(
                3,
                TALLY_NAMES[2],
                pass_fail(change <= tol),
                Some(r_end / r_mid),
                &limit3,
                if change <= tol {
                    format!("relative error went from {r_mid:.4} to {r_end:.4} over the last half")
                } else {
                    format!("the relative error rose from {r_mid:.4} to {r_end:.4} over the last half; a rare large score is still arriving, run more histories")
                },
            ));
            let deviation = change + 0.5 * steps;
            let limit4 = format!(
                "relative error falls from half the run to the end as 1/sqrt(N), within the sampling noise (log-log slope at most {:.2}, expected -0.5)",
                -0.5 + tol / steps
            );
            checks.push(check(
                4,
                TALLY_NAMES[3],
                pass_fail(deviation <= tol),
                Some(change / steps),
                &limit4,
                if deviation <= tol {
                    format!("log-log slope {:.2} over the last half", change / steps)
                } else {
                    format!("log-log slope {:.2} over the last half is flatter than 1/sqrt(N) allows: the error is not shrinking as fast as more histories should make it", change / steps)
                },
            ));
        }
        None => {
            let why = "the relative error is not positive at half the run and at its end (a zero running mean or no spread)";
            checks.push(not_evaluated(3, TALLY_NAMES[2], LIMITS[2], why));
            checks.push(not_evaluated(4, TALLY_NAMES[3], LIMITS[3], why));
        }
    }

    // 5, 6 and 7. Variance of the variance: size, decrease, rate.
    match vov {
        None => {
            let why = "the batch values have no spread or fewer than 3 batches";
            for i in 4..7 {
                checks.push(not_evaluated(i as u8 + 1, TALLY_NAMES[i], LIMITS[i], why));
            }
        }
        Some(v) => {
            checks.push(check(
                5,
                TALLY_NAMES[4],
                pass_fail(v < VOV_LIMIT),
                Some(v),
                LIMITS[4],
                if v < VOV_LIMIT {
                    format!("variance of the variance {v:.4}")
                } else {
                    format!("variance of the variance {v:.3} is not below {VOV_LIMIT}: a few batches hold much of the sum, so the error itself is not reliable; run more histories")
                },
            ));
            match prefix[0]
                .3
                .filter(|m| *m > 0.0)
                .zip(Some(v).filter(|e| *e > 0.0))
            {
                Some((v_mid, v_end)) => {
                    let tol = log_tolerance(vov_noise(k_mid));
                    let change = math::ln(v_end / v_mid);
                    let limit6 = format!(
                        "variance of the variance at the end of the run not above its value at half the run by more than the sampling noise of the estimate (a factor {:.1})",
                        math::exp(tol)
                    );
                    checks.push(check(
                        6,
                        TALLY_NAMES[5],
                        pass_fail(change <= tol),
                        Some(v_end / v_mid),
                        &limit6,
                        if change <= tol {
                            format!("variance of the variance went from {v_mid:.4} to {v_end:.4} over the last half")
                        } else {
                            format!("the variance of the variance rose from {v_mid:.4} to {v_end:.4} over the last half; a heavy batch arrived late, run more histories")
                        },
                    ));
                    let deviation = change + steps;
                    let limit7 = format!(
                        "variance of the variance falls from half the run to the end as 1/N, within the sampling noise (log-log slope at most {:.2}, expected -1)",
                        -1.0 + tol / steps
                    );
                    checks.push(check(
                        7,
                        TALLY_NAMES[6],
                        pass_fail(deviation <= tol),
                        Some(change / steps),
                        &limit7,
                        if deviation <= tol {
                            format!("log-log slope {:.2} over the last half", change / steps)
                        } else {
                            format!("log-log slope {:.2} over the last half is flatter than 1/N allows: the heavy tail is not averaging out; run more histories", change / steps)
                        },
                    ));
                }
                None => {
                    let why = "the variance of the variance is not positive at half the run";
                    checks.push(not_evaluated(6, TALLY_NAMES[5], LIMITS[5], why));
                    checks.push(not_evaluated(7, TALLY_NAMES[6], LIMITS[6], why));
                }
            }
        }
    }

    // 8 and 9. Figure of merit. It varies with the sampling noise of the variance estimate, so the 20 %
    // limit is widened by three standard deviations of that noise at the half-way point.
    let (basis, cost_at): (CostBasis, Box<dyn Fn(usize) -> f64>) = match cumulative_seconds {
        Some(t) => {
            let t = t.to_vec();
            (CostBasis::Seconds, Box::new(move |k| t[k - 1]))
        }
        None => (CostBasis::Batches, Box::new(|k| k as f64)),
    };
    let fom_series: Vec<(usize, f64)> = prefix
        .iter()
        .filter_map(|p| {
            r_of(p.1, p.2)
                .and_then(|r| figure_of_merit(r, cost_at(p.0)))
                .map(|f| (p.0, f))
        })
        .collect();
    let fom = if fom_series.len() == prefix.len() && fom_series.len() >= 3 {
        let final_value = fom_series[fom_series.len() - 1].1;
        let max_change = fom_series
            .iter()
            .map(|p| (p.1 / final_value - 1.0).abs())
            .fold(0.0, f64::max);
        Some(FomSummary {
            basis,
            final_value,
            trend: fom_series.clone(),
            max_change_last_half: max_change,
        })
    } else {
        None
    };
    match &fom {
        Some(f) => {
            let allowed = FOM_CHANGE_LIMIT + 3.0 * (2.0 / (k_mid as f64 - 1.0)).sqrt();
            let basis_text = match f.basis {
                CostBasis::Seconds => "per second",
                CostBasis::Batches => "per batch, batch times not recorded",
            };
            let limit8 = format!(
                "figure of merit within {:.0} % of its final value over the last half (20 % plus three standard deviations of the sampling noise of the variance estimate at half the run)",
                100.0 * allowed
            );
            checks.push(check(
                8,
                TALLY_NAMES[7],
                pass_fail(f.max_change_last_half < allowed),
                Some(f.max_change_last_half),
                &limit8,
                if f.max_change_last_half < allowed {
                    format!("figure of merit ({basis_text}) changed by {:.1} % at most", 100.0 * f.max_change_last_half)
                } else {
                    format!("figure of merit ({basis_text}) changed by {:.1} % over the last half, so the error is not shrinking as expected for this cost; run more histories", 100.0 * f.max_change_last_half)
                },
            ));
            let values_only: Vec<f64> = f.trend.iter().map(|p| p.1).collect();
            let ratio = values_only[values_only.len() - 1] / values_only[0];
            // Symmetric in rising and falling: a halving counts like a doubling.
            let change = if ratio >= 1.0 {
                ratio - 1.0
            } else {
                1.0 / ratio - 1.0
            };
            let trending = strictly_monotone(&values_only) && change.abs() > allowed;
            let limit9 = format!(
                "no strictly monotonic figure of merit change larger than {:.0} % over the last half",
                100.0 * allowed
            );
            checks.push(check(
                9,
                TALLY_NAMES[8],
                pass_fail(!trending),
                Some(ratio - 1.0),
                &limit9,
                if trending {
                    format!("the figure of merit moved steadily by {:.1} % over the last half; the run has not reached its stable regime", 100.0 * change)
                } else {
                    "the figure of merit shows no steady trend".to_string()
                },
            ));
        }
        None => {
            let why = "the figure of merit needs a positive relative error at every checkpoint";
            checks.push(not_evaluated(8, TALLY_NAMES[7], LIMITS[7], why));
            checks.push(not_evaluated(9, TALLY_NAMES[8], LIMITS[8], why));
        }
    }

    // 10. Tally slope.
    checks.push(not_evaluated(
        10,
        TALLY_NAMES[9],
        LIMITS[9],
        "the slope of the largest history scores needs per-history scores, which batch values do not hold; the run must record the largest scores per tally (OpenMC higher moments give the third and fourth moments, not the largest scores)",
    ));

    let failed: Vec<u8> = checks
        .iter()
        .filter(|c| c.state == CheckState::Fail)
        .map(|c| c.id)
        .collect();
    let gaps = checks
        .iter()
        .filter(|c| c.state == CheckState::NotEvaluated)
        .count();
    let (status, summary) = if !failed.is_empty() {
        let ids: Vec<String> = failed.iter().map(|i| i.to_string()).collect();
        (
            TallyStatus::Failing,
            format!(
                "failing statistical check{} {}; no verdict may rest on this tally. Run more histories or use variance reduction",
                if failed.len() > 1 { "s" } else { "" },
                ids.join(", ")
            ),
        )
    } else if gaps > 0 {
        (
            TallyStatus::PassingWithGaps,
            format!(
                "no statistical check failed; {gaps} could not be evaluated (see the checks for why)"
            ),
        )
    } else {
        (TallyStatus::Passing, "all statistical checks passed".into())
    };
    TallyHealth {
        batches: n,
        mean,
        standard_error: se,
        relative_error,
        variance_of_variance: vov,
        effective_histories: effective,
        low_sample: effective.is_none_or(|e| e < LOW_SAMPLE_HISTORIES),
        fom,
        checks,
        status,
        summary,
    }
}

// ---------------------------------------------------------------------------------------------
// Special functions for the intervals

/// Regularised lower incomplete gamma `P(a, x)`.
fn gamma_p(a: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    let prefactor = math::exp(-x + a * math::ln(x) - math::lgamma(a));
    if x < a + 1.0 {
        let mut term = 1.0 / a;
        let mut sum = term;
        let mut ap = a;
        for _ in 0..1000 {
            ap += 1.0;
            term *= x / ap;
            sum += term;
            if term.abs() < sum.abs() * 1.0e-16 {
                break;
            }
        }
        sum * prefactor
    } else {
        // Continued fraction for Q(a, x) (modified Lentz).
        let tiny = 1.0e-300;
        let mut b = x + 1.0 - a;
        let mut c = 1.0 / tiny;
        let mut d = 1.0 / b;
        let mut h = d;
        for i in 1..1000 {
            let an = -(i as f64) * (i as f64 - a);
            b += 2.0;
            d = an * d + b;
            if d.abs() < tiny {
                d = tiny;
            }
            c = b + an / c;
            if c.abs() < tiny {
                c = tiny;
            }
            d = 1.0 / d;
            let delta = d * c;
            h *= delta;
            if (delta - 1.0).abs() < 1.0e-16 {
                break;
            }
        }
        1.0 - prefactor * h
    }
}

/// Quantile of the gamma distribution with shape `a` and unit scale.
fn gamma_quantile(p: f64, a: f64) -> f64 {
    let mut hi = a.max(1.0);
    while gamma_p(a, hi) < p {
        hi *= 2.0;
    }
    let mut lo = 0.0;
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if gamma_p(a, mid) < p {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Regularised incomplete beta `I_x(a, b)`.
fn beta_inc(a: f64, b: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let front = math::exp(
        math::lgamma(a + b) - math::lgamma(a) - math::lgamma(b)
            + a * math::ln(x)
            + b * math::ln_1p(-x),
    );
    let fraction = |a: f64, b: f64, x: f64| {
        let tiny = 1.0e-300;
        let (qab, qap, qam) = (a + b, a + 1.0, a - 1.0);
        let mut c = 1.0;
        let mut d = 1.0 - qab * x / qap;
        if d.abs() < tiny {
            d = tiny;
        }
        d = 1.0 / d;
        let mut h = d;
        for m in 1..1000 {
            let m = m as f64;
            let m2 = 2.0 * m;
            let aa = m * (b - m) * x / ((qam + m2) * (a + m2));
            d = 1.0 + aa * d;
            if d.abs() < tiny {
                d = tiny;
            }
            c = 1.0 + aa / c;
            if c.abs() < tiny {
                c = tiny;
            }
            d = 1.0 / d;
            h *= d * c;
            let aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2));
            d = 1.0 + aa * d;
            if d.abs() < tiny {
                d = tiny;
            }
            c = 1.0 + aa / c;
            if c.abs() < tiny {
                c = tiny;
            }
            d = 1.0 / d;
            let delta = d * c;
            h *= delta;
            if (delta - 1.0).abs() < 1.0e-16 {
                break;
            }
        }
        h
    };
    if x < (a + 1.0) / (a + b + 2.0) {
        front * fraction(a, b, x) / a
    } else {
        1.0 - front * fraction(b, a, 1.0 - x) / b
    }
}

/// Student t quantile for probability `p` in (0.5, 1) and `dof` degrees of freedom.
pub fn student_t_quantile(p: f64, dof: f64) -> f64 {
    let cdf = |t: f64| 1.0 - 0.5 * beta_inc(dof / 2.0, 0.5, dof / (dof + t * t));
    let mut hi = 1.0;
    while cdf(hi) < p {
        hi *= 2.0;
    }
    let mut lo = 0.0;
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if cdf(mid) < p {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Standard normal quantile for probability `p` in (0, 1), by bisection on the error function.
pub fn normal_quantile(p: f64) -> f64 {
    let cdf = |z: f64| 0.5 * (1.0 + libm::erf(z / std::f64::consts::SQRT_2));
    let (mut lo, mut hi) = (-40.0, 40.0);
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if cdf(mid) < p {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

// ---------------------------------------------------------------------------------------------
// Intervals that hold at low counts

/// Exact (Garwood) two-sided interval for the mean of a Poisson count `k`, at confidence `level`.
/// Its coverage is at least `level` for every mean, which is the point at low counts.
pub fn garwood_poisson(k: u64, level: f64) -> (f64, f64) {
    garwood_real(k as f64, level)
}

fn garwood_real(k: f64, level: f64) -> (f64, f64) {
    let alpha = 1.0 - level;
    let lower = if k > 0.0 {
        gamma_quantile(alpha / 2.0, k)
    } else {
        0.0
    };
    (lower, gamma_quantile(1.0 - alpha / 2.0, k + 1.0))
}

/// One-sided upper confidence bound for the mean of a Poisson count `k` (Garwood).
pub fn poisson_upper_bound(k: u64, level: f64) -> f64 {
    gamma_quantile(level, k as f64 + 1.0)
}

/// Wilson score interval for a proportion of `k` successes in `n` trials, e.g. the share of histories
/// that scored.
pub fn wilson_interval(k: u64, n: u64, level: f64) -> Option<(f64, f64)> {
    if n == 0 || k > n {
        return None;
    }
    let z = normal_quantile(0.5 + level / 2.0);
    let (nf, p) = (n as f64, k as f64 / n as f64);
    let denom = 1.0 + z * z / nf;
    let centre = (p + z * z / (2.0 * nf)) / denom;
    let half = z * (p * (1.0 - p) / nf + z * z / (4.0 * nf * nf)).sqrt() / denom;
    Some(((centre - half).max(0.0), (centre + half).min(1.0)))
}

/// What a tally can honestly show as an interval.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TallyInterval {
    TwoSided {
        lower: f64,
        upper: f64,
        level: f64,
        /// Words for the convention and method; shown beside the interval.
        method: String,
    },
    /// Fewer than 10 effective scoring events: only an upper bound is supported.
    UpperBound {
        upper: f64,
        level: f64,
        method: String,
    },
    NotEvaluated {
        why: String,
        next_step: String,
    },
}

/// Interval for a tally mean from its mean, standard error and number of batches, valid at low counts.
///
/// With `n_eff = (mean / se)^2` effective scoring events: below 10 only a one-sided scaled-Garwood upper
/// bound is given. Otherwise the interval is the wider of the Student t interval on the batch means
/// (`batches - 1` degrees of freedom) and the scaled Garwood interval, so it is neither too narrow when
/// few batches exist nor when few events scored. It cannot see a heavy tail that has not yet been
/// sampled; the health checks exist for that, and a failing tally gets no interval from the callers.
pub fn tally_interval(mean: f64, standard_error: f64, batches: usize, level: f64) -> TallyInterval {
    if !(mean.is_finite() && standard_error.is_finite()) || mean <= 0.0 || standard_error <= 0.0 {
        return TallyInterval::NotEvaluated {
            why: "a positive mean and a positive standard error are needed to set the interval; no scoring event, or a zero or negative mean, gives neither".into(),
            next_step: "run more histories until the tally scores, or use variance reduction".into(),
        };
    }
    if batches < 2 {
        return TallyInterval::NotEvaluated {
            why: "the Student t interval needs at least 2 batches".into(),
            next_step: "run with at least 30 batches".into(),
        };
    }
    let n_eff = math::powi(mean / standard_error, 2);
    let scale = mean / n_eff;
    if n_eff < MIN_EVENTS_TWO_SIDED {
        return TallyInterval::UpperBound {
            upper: scale * gamma_quantile(level, n_eff + 1.0),
            level,
            method: format!(
                "one-sided {:.0} % upper bound, scaled Garwood Poisson bound on {n_eff:.1} effective scoring events (fewer than {MIN_EVENTS_TWO_SIDED:.0}: no lower bound is supported)",
                100.0 * level
            ),
        };
    }
    let (g_lo, g_hi) = garwood_real(n_eff, level);
    let t = student_t_quantile(0.5 + level / 2.0, batches as f64 - 1.0);
    let lower = (scale * g_lo).min(mean - t * standard_error);
    let upper = (scale * g_hi).max(mean + t * standard_error);
    TallyInterval::TwoSided {
        lower: lower.max(0.0),
        upper,
        level,
        method: format!(
            "{:.0} % interval, wider of the Student t interval ({} degrees of freedom) and the scaled Garwood Poisson interval on {n_eff:.0} effective scoring events",
            100.0 * level,
            batches - 1
        ),
    }
}

/// Two-sided or one-sided interval for a raw scoring-event count with known per-event weight: the
/// exact-count form of [`tally_interval`], used where the count itself is recorded.
pub fn counts_interval(events: u64, weight: f64, level: f64) -> TallyInterval {
    if !(weight.is_finite() && weight > 0.0) {
        return TallyInterval::NotEvaluated {
            why: "the weight of one scoring event is not a positive number, so the count has no scale".into(),
            next_step: "pass the per-event weight of the tally".into(),
        };
    }
    if (events as f64) < MIN_EVENTS_TWO_SIDED {
        return TallyInterval::UpperBound {
            upper: weight * poisson_upper_bound(events, level),
            level,
            method: format!(
                "one-sided {:.0} % upper bound, Garwood Poisson bound on {events} scoring events",
                100.0 * level
            ),
        };
    }
    let (lo, hi) = garwood_poisson(events, level);
    TallyInterval::TwoSided {
        lower: weight * lo,
        upper: weight * hi,
        level,
        method: format!(
            "{:.0} % interval, Garwood Poisson interval on {events} scoring events",
            100.0 * level
        ),
    }
}

// ---------------------------------------------------------------------------------------------
// Independent checks of the reported error

/// Result of comparing the reported standard error with an independent estimate of it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ErrorAgreement {
    pub reported: f64,
    pub independent: f64,
    /// (independent - reported) in units of the sampling error of the independent estimate.
    pub z: f64,
    /// Whether `|z| < 3`.
    pub agrees: bool,
}

/// Error from independent batches against the reported error. The standard error estimated from `n`
/// normal batches has a relative sampling error of about `1 / sqrt(2 (n - 1))`.
pub fn batch_error_agreement(reported: f64, values: &[f64]) -> Option<ErrorAgreement> {
    let (_, independent) = mean_and_standard_error(values)?;
    if !(reported > 0.0 && reported.is_finite()) {
        return None;
    }
    let n = values.len() as f64;
    let sampling = independent / (2.0 * (n - 1.0)).sqrt();
    let z = (independent - reported) / sampling.max(f64::MIN_POSITIVE);
    Some(ErrorAgreement {
        reported,
        independent,
        z,
        agrees: z.abs() < 3.0,
    })
}

/// Seed-to-seed spread against the reported errors: `runs` holds (mean, reported standard error) from
/// independent seeds of the same problem. The spread of the means should match the root-mean-square
/// reported error; the sampling error of a standard deviation from `K` runs is `s / sqrt(2 (K - 1))`.
pub fn seed_spread_agreement(runs: &[(f64, f64)]) -> Option<ErrorAgreement> {
    let k = runs.len();
    if k < 3
        || runs
            .iter()
            .any(|r| !(r.0.is_finite() && r.1.is_finite() && r.1 > 0.0))
    {
        return None;
    }
    let kf = k as f64;
    let mean = runs.iter().map(|r| r.0).sum::<f64>() / kf;
    let spread = (runs
        .iter()
        .map(|r| (r.0 - mean) * (r.0 - mean))
        .sum::<f64>()
        / (kf - 1.0))
        .sqrt();
    let reported = (runs.iter().map(|r| r.1 * r.1).sum::<f64>() / kf).sqrt();
    let sampling = spread / (2.0 * (kf - 1.0)).sqrt();
    let z = (spread - reported) / sampling.max(f64::MIN_POSITIVE);
    Some(ErrorAgreement {
        reported,
        independent: spread,
        z,
        agrees: z.abs() < 3.0,
    })
}

// ---------------------------------------------------------------------------------------------
// Sums of tally parts

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum CombineError {
    #[error(
        "{0} weights, {1} means and a covariance of {2} entries do not describe one square matrix; pass the n x n covariance of the same n parts"
    )]
    Shape(usize, usize, usize),
    #[error(
        "the combined variance is negative ({0:e}) beyond rounding: the covariance is not a valid sampling covariance; recompute it from the batch values"
    )]
    NegativeVariance(f64),
    #[error("a value is not a finite number")]
    NotFinite,
}

/// Weighted sum `sum w_i x_i` and its standard error `sqrt(w' C w)` with the full covariance `C`
/// (row-major, n x n). Parts that share batches are correlated; dropping the off-diagonal entries gives
/// the wrong error, which is why the covariance is required here and never assumed zero.
pub fn combine_linear(
    weights: &[f64],
    means: &[f64],
    covariance: &[f64],
) -> Result<(f64, f64), CombineError> {
    let n = means.len();
    if weights.len() != n || covariance.len() != n * n {
        return Err(CombineError::Shape(weights.len(), n, covariance.len()));
    }
    if weights
        .iter()
        .chain(means)
        .chain(covariance)
        .any(|v| !v.is_finite())
    {
        return Err(CombineError::NotFinite);
    }
    let value: f64 = weights.iter().zip(means).map(|(w, m)| w * m).sum();
    let mut variance = 0.0;
    let mut scale: f64 = 0.0;
    for i in 0..n {
        for j in 0..n {
            let term = weights[i] * weights[j] * covariance[i * n + j];
            variance += term;
            scale = scale.max(term.abs());
        }
    }
    if variance < -1.0e-9 * scale {
        return Err(CombineError::NegativeVariance(variance));
    }
    Ok((value, variance.max(0.0).sqrt()))
}

/// Sample covariance of the means of batch columns, `cov(x_i, x_j) / n`, the estimator the transport
/// worker records. `columns` holds one slice of batch values per part, all of the same length.
pub fn batch_mean_covariance(columns: &[&[f64]]) -> Option<Vec<f64>> {
    let p = columns.len();
    let n = columns.first()?.len();
    if n < 2
        || columns
            .iter()
            .any(|c| c.len() != n || c.iter().any(|v| !v.is_finite()))
    {
        return None;
    }
    let nf = n as f64;
    let means: Vec<f64> = columns.iter().map(|c| c.iter().sum::<f64>() / nf).collect();
    let mut out = vec![0.0; p * p];
    for i in 0..p {
        for j in 0..p {
            let s: f64 = (0..n)
                .map(|b| (columns[i][b] - means[i]) * (columns[j][b] - means[j]))
                .sum();
            out[i * p + j] = s / (nf - 1.0) / nf;
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------------------------
// Convergence across a mesh

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MeshConvergence {
    pub voxels: usize,
    /// Voxels with a positive mean and standard error: those that have a relative error.
    pub scored: usize,
    /// Voxels with no score; they count as not converged, never as zero error.
    pub unscored: usize,
    pub within_10_percent: usize,
    pub within_5_percent: usize,
    /// Scored voxels with fewer than `LOW_SAMPLE_HISTORIES` effective scoring histories.
    pub low_sample: usize,
    /// Fractions of all voxels.
    pub fraction_within_10_percent: f64,
    pub fraction_within_5_percent: f64,
    /// Largest relative error among scored voxels.
    pub worst_relative_error: Option<f64>,
}

/// Convergence summary of a mesh (or spectrum) tally from per-voxel means and standard errors.
/// Fractions are over all voxels, so unscored voxels pull them down.
pub fn mesh_convergence(means: &[f64], standard_errors: &[f64]) -> Option<MeshConvergence> {
    if means.len() != standard_errors.len() || means.is_empty() {
        return None;
    }
    let mut c = MeshConvergence {
        voxels: means.len(),
        scored: 0,
        unscored: 0,
        within_10_percent: 0,
        within_5_percent: 0,
        low_sample: 0,
        fraction_within_10_percent: 0.0,
        fraction_within_5_percent: 0.0,
        worst_relative_error: None,
    };
    for (&m, &s) in means.iter().zip(standard_errors) {
        if m.is_finite() && s.is_finite() && m > 0.0 && s > 0.0 {
            c.scored += 1;
            let r = s / m;
            if r <= 0.10 {
                c.within_10_percent += 1;
            }
            if r <= 0.05 {
                c.within_5_percent += 1;
            }
            if 1.0 / (r * r) < LOW_SAMPLE_HISTORIES {
                c.low_sample += 1;
            }
            c.worst_relative_error = Some(c.worst_relative_error.map_or(r, |w: f64| w.max(r)));
        } else {
            c.unscored += 1;
        }
    }
    let total = c.voxels as f64;
    c.fraction_within_10_percent = c.within_10_percent as f64 / total;
    c.fraction_within_5_percent = c.within_5_percent as f64 / total;
    Some(c)
}

#[cfg(test)]
mod tests;
