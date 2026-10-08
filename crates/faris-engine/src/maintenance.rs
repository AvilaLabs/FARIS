//! Computed maintenance durations: the coupling loop of `scripts/maintenance_coupling_test.py`
//! in Rust. A replacement outage is the cooldown of the components around the replaced part
//! (until their decay heat per volume falls to q*) plus the work time; the operating history
//! is run again with those outages until no duration changes by more than the convergence
//! step. The module does no I/O: histories and decay curves come through [`HistoryRunner`]
//! and [`DecaySource`].

use crate::history::{HistoryOutcome, HistoryResult};
use crate::jobs::Cancellation;
use faris_model::history::{ComponentClass, OperatingHistoryAssumptions};
use faris_model::maintenance::{CoolingGrid, MaintenanceAssumptions, MaintenanceClass, Threshold};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAINTENANCE_RESULT_VERSION: &str = "faris-maintenance-result/v0.1";

const DAY_S: f64 = 86_400.0;
/// Contrasts whose fixed-duration difference is smaller than this give no ratio.
pub const CONTRAST_MIN_FIXED_S: f64 = 30.0 * DAY_S;
/// Tolerance of the calibration round trip, seconds (the reference's `TOL_S`).
const CALIBRATION_TOL_S: f64 = 1.0;

/// A physics or data gap. Never filled with a number.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct NotEvaluated {
    pub reason: String,
    pub next_step: String,
}

fn not_evaluated(reason: impl Into<String>, next_step: impl Into<String>) -> NotEvaluated {
    NotEvaluated {
        reason: reason.into(),
        next_step: next_step.into(),
    }
}

// ------------------------------------------------------------ cooling curves --

/// Log-spaced times from `min_s` to `max_s`, endpoints exact.
pub fn cooling_grid_s(grid: &CoolingGrid) -> Vec<f64> {
    let ratio = grid.max_s / grid.min_s;
    let last = grid.points - 1;
    let mut out: Vec<f64> = (0..grid.points)
        .map(|i| grid.min_s * ratio.powf(i as f64 / last as f64))
        .collect();
    out[0] = grid.min_s;
    out[last] = grid.max_s;
    out
}

/// Value at `t` between sorted (t, value) points: linear in ln t, and in ln value when both
/// ends are positive. Outside the range the end value holds. No points: NaN.
pub fn interpolate(points: &[(f64, f64)], t: f64) -> f64 {
    match points.len() {
        0 => return f64::NAN,
        1 => return points[0].1,
        _ => {}
    }
    let after = points.partition_point(|p| p.0 <= t);
    let i = after.saturating_sub(1).min(points.len() - 2);
    let ((t0, v0), (t1, v1)) = (points[i], points[i + 1]);
    if t <= t0 {
        return v0;
    }
    if t >= t1 {
        return v1;
    }
    let frac = (t.ln() - t0.ln()) / (t1.ln() - t0.ln());
    if v0 > 0.0 && v1 > 0.0 {
        return (v0.ln() + frac * (v1.ln() - v0.ln())).exp();
    }
    v0 + frac * (v1 - v0)
}

/// Curve of the governing set versus time since shutdown: the sum of the components' values
/// divided by their total volume, on the union of their sample times inside the common time
/// range, up to `max_s`. Each series is sorted (seconds > 0, value).
pub fn combined_curve(
    series: &[&[(f64, f64)]],
    volumes_m3: &[f64],
    max_s: f64,
) -> Result<Vec<(f64, f64)>, String> {
    if series.is_empty() || series.len() != volumes_m3.len() || series.iter().any(|s| s.is_empty())
    {
        return Err("no components".into());
    }
    let lo = series
        .iter()
        .map(|s| s[0].0)
        .fold(f64::NEG_INFINITY, f64::max);
    let hi = series
        .iter()
        .map(|s| s[s.len() - 1].0)
        .fold(f64::INFINITY, f64::min)
        .min(max_s * (1.0 + 1e-9));
    let mut times: Vec<f64> = series
        .iter()
        .flat_map(|s| s.iter().map(|p| p.0))
        .filter(|&t| lo * (1.0 - 1e-12) <= t && t <= hi * (1.0 + 1e-12))
        .collect();
    times.sort_by(f64::total_cmp);
    times.dedup();
    if times.is_empty() {
        return Err("components share no cooling-time range".into());
    }
    let total_volume: f64 = volumes_m3.iter().sum();
    Ok(times
        .into_iter()
        .map(|t| {
            let sum: f64 = series.iter().map(|s| interpolate(s, t)).sum();
            (t, sum / total_volume)
        })
        .collect())
}

/// Time until a curve first reaches or falls below q*.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Cooldown {
    pub cooldown_s: f64,
    /// The curve ended inside the outage window without crossing: a lower bound.
    pub window_limited: bool,
}

/// First crossing of q* (log-linear between points). No crossing and the curve reaches
/// `max_s`: NOT_EVALUATED. No crossing and the curve ends earlier (the outage after the
/// shutdown is shorter than that): the window length, flagged window-limited.
pub fn cooldown(curve: &[(f64, f64)], q_star: f64, max_s: f64) -> Result<Cooldown, NotEvaluated> {
    let Some(first) = curve.first() else {
        return Err(not_evaluated(
            "no decay curve",
            "check that the decay source returned points after the shutdown",
        ));
    };
    if first.1 <= q_star {
        return Ok(Cooldown {
            cooldown_s: first.0,
            window_limited: false,
        });
    }
    for pair in curve.windows(2) {
        let ((t0, q0), (t1, q1)) = (pair[0], pair[1]);
        if q1 <= q_star {
            let t = if q0 > 0.0 && q1 > 0.0 && q_star > 0.0 {
                let frac = (q_star.ln() - q0.ln()) / (q1.ln() - q0.ln());
                (t0.ln() + frac * (t1.ln() - t0.ln())).exp()
            } else {
                t0 + (q0 - q_star) / (q0 - q1) * (t1 - t0)
            };
            return Ok(Cooldown {
                cooldown_s: t,
                window_limited: false,
            });
        }
    }
    let end = curve[curve.len() - 1].0;
    if end >= max_s * (1.0 - 1e-9) {
        return Err(not_evaluated(
            format!(
                "decay curve never falls below q* within {} days",
                max_s / DAY_S
            ),
            "raise q*, lengthen the cooling grid, or add a governing component's cooling time",
        ));
    }
    Ok(Cooldown {
        cooldown_s: end,
        window_limited: true,
    })
}

/// q* = the curve value at the target cooldown; the cooldown it gives must come back.
pub fn calibrate(
    curve: &[(f64, f64)],
    target_cooldown_s: f64,
    max_s: f64,
) -> Result<f64, NotEvaluated> {
    let (Some(first), Some(last)) = (curve.first(), curve.last()) else {
        return Err(not_evaluated(
            "no decay curve at the calibration event",
            "check that the calibration design has a replacement of this class",
        ));
    };
    if !(first.0 * (1.0 - 1e-9) <= target_cooldown_s && target_cooldown_s <= last.0 * (1.0 + 1e-9))
    {
        return Err(not_evaluated(
            format!(
                "calibration cooldown {:.3} d is outside the curve [{:.3}, {:.3}] d",
                target_cooldown_s / DAY_S,
                first.0 / DAY_S,
                last.0 / DAY_S
            ),
            "choose a target cooldown inside the cooling grid, or give q* explicitly",
        ));
    }
    let q_star = interpolate(curve, target_cooldown_s);
    let tol = CALIBRATION_TOL_S.max(1e-6 * target_cooldown_s);
    match cooldown(curve, q_star, max_s) {
        Ok(back) if (back.cooldown_s - target_cooldown_s).abs() <= tol => Ok(q_star),
        _ => Err(not_evaluated(
            format!(
                "decay curve is not monotone at the calibration event; q* ({q_star:.6e} W/m3) does not reproduce the target cooldown"
            ),
            "give q* explicitly, or calibrate on another design or class",
        )),
    }
}

// ------------------------------------------------------------------ history --

/// One replacement outage: counted per component (`k` from 1), `end_s` absent when the
/// horizon ends first.
#[derive(Clone, Debug, PartialEq)]
pub struct ReplacementEvent {
    pub class: String,
    pub component: String,
    pub k: u32,
    pub start_s: f64,
    pub end_s: Option<f64>,
}

pub fn history_end_s(history: &HistoryResult) -> f64 {
    if history.outcome == HistoryOutcome::HorizonCompleted {
        history.assumptions.horizon_s
    } else {
        history.snapshots.last().map_or(0.0, |s| s.time_s)
    }
}

/// Replacements per class, in time order.
pub fn replacement_events(
    history: &HistoryResult,
    classes: &BTreeMap<String, MaintenanceClass>,
) -> Vec<ReplacementEvent> {
    use crate::history::EventKind::{ReplacementCompleted, ReplacementStarted};
    let mut out: Vec<ReplacementEvent> = Vec::new();
    for (class, spec) in classes {
        let mut events: Vec<_> = history
            .events
            .iter()
            .filter(|e| {
                e.component_id.as_deref() == Some(spec.component_id.as_str())
                    && matches!(e.kind, ReplacementStarted | ReplacementCompleted)
            })
            .collect();
        events.sort_by(|a, b| a.time_s.total_cmp(&b.time_s).then(a.order.cmp(&b.order)));
        let (mut k, mut open) = (0u32, None::<usize>);
        for e in events {
            if e.kind == ReplacementStarted {
                k += 1;
                out.push(ReplacementEvent {
                    class: class.clone(),
                    component: spec.component_id.clone(),
                    k,
                    start_s: e.time_s,
                    end_s: None,
                });
                open = Some(out.len() - 1);
            } else if let Some(i) = open.take() {
                out[i].end_s = Some(e.time_s);
            }
        }
    }
    out.sort_by(|a, b| {
        a.start_s
            .total_cmp(&b.start_s)
            .then_with(|| a.component.cmp(&b.component))
    });
    out
}

pub fn total_downtime_s(
    history: &HistoryResult,
    classes: &BTreeMap<String, MaintenanceClass>,
) -> f64 {
    let end = history_end_s(history);
    replacement_events(history, classes)
        .iter()
        .map(|e| e.end_s.unwrap_or(end) - e.start_s)
        .sum()
}

// ------------------------------------------------------------------- inputs --

/// A decay curve is requested for the installation of `component` in place at `shutdown_s`.
#[derive(Clone, Debug, PartialEq)]
pub struct CurveRequest {
    pub component: String,
    pub shutdown_s: f64,
}

/// Decay heat of one installation after a shutdown.
#[derive(Clone, Debug, PartialEq)]
pub struct InstallationCurve {
    /// (seconds since shutdown > 0, ascending; decay heat in W of the whole installation).
    pub points: Vec<(f64, f64)>,
    pub volume_m3: f64,
    /// When this installation went into the machine, seconds.
    pub install_s: f64,
}

pub trait DecaySource {
    /// For each request, the decay curve of the installation of `component` in place at
    /// `shutdown_s` in this design's history. `grid_s` is the cooling grid the curves should
    /// resolve. One call per design per iteration, so a source can batch its runs. `Err` is a
    /// tool failure; a per-request `Err` is a gap in the physics and becomes NOT_EVALUATED.
    fn curves(
        &mut self,
        design: &str,
        history: &HistoryResult,
        requests: &[CurveRequest],
        grid_s: &[f64],
        cancel: &Cancellation,
    ) -> Result<Vec<Result<InstallationCurve, NotEvaluated>>, String>;
}

pub trait HistoryRunner {
    /// The operating history of `design` under `assumptions` (its rates are the runner's).
    fn run(
        &mut self,
        design: &str,
        assumptions: &OperatingHistoryAssumptions,
        cancel: &Cancellation,
    ) -> Result<HistoryResult, String>;
}

impl<F> HistoryRunner for F
where
    F: FnMut(&str, &OperatingHistoryAssumptions, &Cancellation) -> Result<HistoryResult, String>,
{
    fn run(
        &mut self,
        design: &str,
        assumptions: &OperatingHistoryAssumptions,
        cancel: &Cancellation,
    ) -> Result<HistoryResult, String> {
        self(design, assumptions, cancel)
    }
}

#[derive(Clone, Debug)]
pub struct DesignInput {
    pub name: String,
    /// Its replacement durations are the design's fixed durations.
    pub history_assumptions: OperatingHistoryAssumptions,
}

// ------------------------------------------------------------------- result --

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Status {
    Evaluated,
    NotEvaluated,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ReplacementSummary {
    pub class: String,
    pub component: String,
    pub k: u32,
    pub start_s: f64,
    pub end_s: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct HistorySummary {
    pub outcome: HistoryOutcome,
    pub history_end_s: f64,
    pub lifetime_net_electricity_mwh: Option<f64>,
    pub total_replacement_downtime_s: f64,
    /// 1 - downtime / history end.
    pub availability: f64,
    pub replacements: Vec<ReplacementSummary>,
}

fn summarize(
    history: &HistoryResult,
    classes: &BTreeMap<String, MaintenanceClass>,
) -> HistorySummary {
    let end = history_end_s(history);
    let downtime = total_downtime_s(history, classes);
    HistorySummary {
        outcome: history.outcome.clone(),
        history_end_s: end,
        lifetime_net_electricity_mwh: history
            .snapshots
            .last()
            .and_then(|s| s.cumulative_net_electricity_mwh),
        total_replacement_downtime_s: downtime,
        availability: if end > 0.0 { 1.0 - downtime / end } else { 1.0 },
        replacements: replacement_events(history, classes)
            .into_iter()
            .map(|e| ReplacementSummary {
                class: e.class,
                component: e.component,
                k: e.k,
                start_s: e.start_s,
                end_s: e.end_s,
            })
            .collect(),
    }
}

/// One governing component's part in a cooldown.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct GoverningShare {
    pub component: String,
    /// Its share of the combined curve value at the cooldown time; the shares sum to 1.
    pub share: f64,
    /// Shutdown time minus the install time of the installation in place.
    pub in_service_s: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Why {
    pub governing: Vec<GoverningShare>,
    /// First sample of the combined curve (time since shutdown, W/m3).
    pub curve_start_s: f64,
    pub curve_start_w_per_m3: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct EventRecord {
    pub class: String,
    pub component: String,
    pub k: u32,
    pub start_s: f64,
    pub end_s: Option<f64>,
    pub fixed_duration_s: f64,
    /// Duration this iteration's history was run with.
    pub duration_used_s: f64,
    pub cooldown_s: Option<f64>,
    pub work_s: f64,
    /// Work plus cooldown: the duration the next iteration uses.
    pub duration_computed_s: Option<f64>,
    pub window_limited: bool,
    pub q_star: f64,
    pub why: Option<Why>,
    pub not_evaluated: Option<NotEvaluated>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct IterationRecord {
    pub iteration: u32,
    pub durations_in_s: BTreeMap<String, Vec<f64>>,
    pub durations_out_s: BTreeMap<String, Vec<f64>>,
    pub max_change_s: f64,
    pub window_limited: bool,
    pub events: Vec<EventRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ComputedResult {
    pub status: Status,
    pub not_evaluated: Option<NotEvaluated>,
    /// The history run with the converged durations.
    pub summary: Option<HistorySummary>,
    pub converged_at_iteration: Option<u32>,
    pub iterations: Vec<IterationRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ThresholdRecord {
    pub status: Status,
    pub q_star_w_per_m3: Option<f64>,
    /// "explicit" or "calibrated".
    pub source: String,
    pub calibration_design: Option<String>,
    pub target_cooldown_s: Option<f64>,
    pub calibration_event_start_s: Option<f64>,
    pub not_evaluated: Option<NotEvaluated>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DesignResult {
    pub fixed: HistorySummary,
    pub computed: ComputedResult,
    pub thresholds: BTreeMap<String, ThresholdRecord>,
}

/// Downtime difference between two designs (a minus b, names in order) under both models.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Contrast {
    pub a: String,
    pub b: String,
    pub fixed_difference_s: f64,
    pub computed_difference_s: Option<f64>,
    pub ratio_computed_over_fixed: Option<f64>,
    /// Why the computed difference or the ratio is absent.
    pub not_evaluated: Option<NotEvaluated>,
}

/// What the decay source did, filled by the caller after the run.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DecaySourceRecord {
    /// For example "actinv-continuations".
    pub kind: String,
    /// ACTINV runs started in this run.
    pub actinv_runs: u64,
    /// Decay curves found in the points cache instead of being run.
    pub cache_hits: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct MaintenanceResult {
    pub schema_version: String,
    /// SHA-256 of each input file, filled by the caller.
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
    pub assumptions: MaintenanceAssumptions,
    pub designs: BTreeMap<String, DesignResult>,
    pub contrasts: Vec<Contrast>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decay_source: Option<DecaySourceRecord>,
}

// --------------------------------------------------------------------- loop --

fn cancelled() -> String {
    "cancelled".into()
}

/// The authored duration of a class in a design: its component's replaceable limits agree on one.
fn fixed_duration_s(
    assumptions: &OperatingHistoryAssumptions,
    component: &str,
) -> Result<f64, String> {
    let mut values = BTreeSet::new();
    for limit in &assumptions.service_limits {
        if limit.component_id == component && limit.class == ComponentClass::Replaceable {
            values.insert(limit.replacement_duration_s.map(f64::to_bits));
        }
    }
    match (values.len(), values.first()) {
        (1, Some(Some(bits))) => Ok(f64::from_bits(*bits)),
        _ => Err(format!(
            "component {component} needs one replacement_duration_s over its replaceable limits"
        )),
    }
}

/// Requests for the governing components of the given events, de-duplicated, with an index
/// from (component, shutdown) to its position.
fn requests_for(
    events: &[ReplacementEvent],
    classes: &BTreeMap<String, MaintenanceClass>,
) -> (Vec<CurveRequest>, BTreeMap<(String, u64), usize>) {
    let mut requests = Vec::new();
    let mut index = BTreeMap::new();
    for e in events {
        for component in &classes[&e.class].governing {
            index
                .entry((component.clone(), e.start_s.to_bits()))
                .or_insert_with(|| {
                    requests.push(CurveRequest {
                        component: component.clone(),
                        shutdown_s: e.start_s,
                    });
                    requests.len() - 1
                });
        }
    }
    (requests, index)
}

/// Curves in request order, and where each (component, shutdown) request sits in them.
type Fetched = (
    Vec<Result<InstallationCurve, NotEvaluated>>,
    BTreeMap<(String, u64), usize>,
);

fn fetch_curves(
    source: &mut dyn DecaySource,
    design: &str,
    history: &HistoryResult,
    events: &[ReplacementEvent],
    classes: &BTreeMap<String, MaintenanceClass>,
    grid_s: &[f64],
    cancel: &Cancellation,
) -> Result<Fetched, String> {
    let (requests, index) = requests_for(events, classes);
    if requests.is_empty() {
        return Ok((Vec::new(), index));
    }
    if cancel.is_cancelled() {
        return Err(cancelled());
    }
    let got = source.curves(design, history, &requests, grid_s, cancel)?;
    if got.len() != requests.len() {
        return Err(format!(
            "decay source returned {} curves for {} requests",
            got.len(),
            requests.len()
        ));
    }
    Ok((got, index))
}

/// The combined governing curve of one event, with what is needed to explain it.
struct EventCurve {
    curve: Vec<(f64, f64)>,
    series: Vec<Vec<(f64, f64)>>,
    in_service_s: Vec<f64>,
}

fn event_curve(
    event: &ReplacementEvent,
    spec: &MaintenanceClass,
    curves: &[Result<InstallationCurve, NotEvaluated>],
    index: &BTreeMap<(String, u64), usize>,
    max_s: f64,
) -> Result<EventCurve, NotEvaluated> {
    let mut series = Vec::new();
    let mut volumes = Vec::new();
    let mut in_service_s = Vec::new();
    for component in &spec.governing {
        let i = index[&(component.clone(), event.start_s.to_bits())];
        let c = curves[i].as_ref().map_err(Clone::clone)?;
        let sorted = c.points.windows(2).all(|w| w[0].0 < w[1].0)
            && c.points
                .iter()
                .all(|p| p.0 > 0.0 && p.0.is_finite() && p.1.is_finite());
        if c.points.is_empty() || !sorted || !(c.volume_m3.is_finite() && c.volume_m3 > 0.0) {
            return Err(not_evaluated(
                format!(
                    "{component} has no usable decay points after the shutdown at {:.0} s",
                    event.start_s
                ),
                "check the decay source: points must be finite, ascending, after the shutdown, with a positive volume",
            ));
        }
        series.push(c.points.clone());
        volumes.push(c.volume_m3);
        in_service_s.push(event.start_s - c.install_s);
    }
    let refs: Vec<&[(f64, f64)]> = series.iter().map(Vec::as_slice).collect();
    let curve = combined_curve(&refs, &volumes, max_s).map_err(|e| {
        not_evaluated(
            e,
            "check that the governing components' decay curves overlap in cooling time",
        )
    })?;
    Ok(EventCurve {
        curve,
        series,
        in_service_s,
    })
}

fn why(spec: &MaintenanceClass, ec: &EventCurve, cooldown_s: f64) -> Why {
    let values: Vec<f64> = ec
        .series
        .iter()
        .map(|s| interpolate(s, cooldown_s))
        .collect();
    let total: f64 = values.iter().sum();
    let n = values.len() as f64;
    // An all-zero set has no shares; split evenly rather than divide by zero.
    let governing = spec
        .governing
        .iter()
        .enumerate()
        .map(|(i, component)| GoverningShare {
            component: component.clone(),
            share: if total > 0.0 {
                values[i] / total
            } else {
                1.0 / n
            },
            in_service_s: ec.in_service_s[i],
        })
        .collect();
    Why {
        governing,
        curve_start_s: ec.curve[0].0,
        curve_start_w_per_m3: ec.curve[0].1,
    }
}

fn explicit_or_pending(threshold: &Threshold) -> Option<f64> {
    match threshold {
        Threshold::QStar { q_star_w_per_m3 } => Some(*q_star_w_per_m3),
        Threshold::Calibrate { .. } => None,
    }
}

struct Fixed<'a> {
    input: &'a DesignInput,
    history: HistoryResult,
    /// Fixed duration per class.
    durations_s: BTreeMap<String, f64>,
}

#[allow(clippy::too_many_arguments)]
fn calibrate_thresholds(
    assumptions: &MaintenanceAssumptions,
    fixed: &[Fixed],
    source: &mut dyn DecaySource,
    grid_s: &[f64],
    cancel: &Cancellation,
    progress: &mut dyn FnMut(&str, u32, &str),
) -> Result<BTreeMap<String, ThresholdRecord>, String> {
    let mut out = BTreeMap::new();
    for (name, spec) in &assumptions.classes {
        if let Some(q) = explicit_or_pending(&spec.threshold) {
            out.insert(
                name.clone(),
                ThresholdRecord {
                    status: Status::Evaluated,
                    q_star_w_per_m3: Some(q),
                    source: "explicit".into(),
                    calibration_design: None,
                    target_cooldown_s: None,
                    calibration_event_start_s: None,
                    not_evaluated: None,
                },
            );
            continue;
        }
        let Threshold::Calibrate {
            design,
            target_cooldown_s,
        } = &spec.threshold
        else {
            unreachable!("explicit thresholds are handled above");
        };
        let mut record = ThresholdRecord {
            status: Status::NotEvaluated,
            q_star_w_per_m3: None,
            source: "calibrated".into(),
            calibration_design: Some(design.clone()),
            target_cooldown_s: Some(*target_cooldown_s),
            calibration_event_start_s: None,
            not_evaluated: None,
        };
        let Some(cal) = fixed.iter().find(|f| &f.input.name == design) else {
            return Err(format!(
                "class {name} calibrates on design {design}, which is not among the designs"
            ));
        };
        let events: Vec<_> = replacement_events(&cal.history, &assumptions.classes)
            .into_iter()
            .filter(|e| &e.class == name)
            .take(1)
            .collect();
        let Some(first) = events.first() else {
            record.not_evaluated = Some(not_evaluated(
                format!("no {name} replacement in {design} at the fixed durations"),
                "lengthen the calibration design's horizon or give q* explicitly",
            ));
            out.insert(name.clone(), record);
            continue;
        };
        record.calibration_event_start_s = Some(first.start_s);
        progress(design, 0, &format!("calibrating q* for class {name}"));
        let (curves, index) = fetch_curves(
            source,
            design,
            &cal.history,
            &events,
            &assumptions.classes,
            grid_s,
            cancel,
        )?;
        let result = event_curve(first, spec, &curves, &index, assumptions.cooling.max_s)
            .and_then(|ec| calibrate(&ec.curve, *target_cooldown_s, assumptions.cooling.max_s));
        match result {
            Ok(q) => {
                record.status = Status::Evaluated;
                record.q_star_w_per_m3 = Some(q);
            }
            Err(ne) => record.not_evaluated = Some(ne),
        }
        out.insert(name.clone(), record);
    }
    Ok(out)
}

fn failed(ne: NotEvaluated, iterations: Vec<IterationRecord>) -> ComputedResult {
    ComputedResult {
        status: Status::NotEvaluated,
        not_evaluated: Some(ne),
        summary: None,
        converged_at_iteration: None,
        iterations,
    }
}

/// Iterates history -> decay curves -> durations for one design.
#[allow(clippy::too_many_arguments)]
fn coupled_case(
    assumptions: &MaintenanceAssumptions,
    fixed: &Fixed,
    thresholds: &BTreeMap<String, ThresholdRecord>,
    runner: &mut dyn HistoryRunner,
    source: &mut dyn DecaySource,
    grid_s: &[f64],
    cancel: &Cancellation,
    progress: &mut dyn FnMut(&str, u32, &str),
) -> Result<ComputedResult, String> {
    let name = fixed.input.name.as_str();
    let classes = &assumptions.classes;
    for (class, th) in thresholds {
        if let Some(ne) = &th.not_evaluated {
            return Ok(failed(
                not_evaluated(
                    format!("calibration unavailable for {class}: {}", ne.reason),
                    ne.next_step.clone(),
                ),
                Vec::new(),
            ));
        }
    }
    let mut used: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut iterations: Vec<IterationRecord> = Vec::new();
    for number in 1..=assumptions.max_iterations {
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        // Iteration 1 runs at the fixed durations: the history is already in hand.
        let history = if used.is_empty() {
            fixed.history.clone()
        } else {
            progress(name, number, "operating history");
            let mut derived = fixed.input.history_assumptions.clone();
            for limit in &mut derived.service_limits {
                if let Some(durations) = used.get(&limit.component_id) {
                    limit.replacement_durations_s = Some(durations.clone());
                }
            }
            runner.run(name, &derived, cancel)?
        };
        let horizon = history_end_s(&history);
        let events = replacement_events(&history, classes);
        progress(name, number, "decay curves");
        let (curves, index) =
            fetch_curves(source, name, &history, &events, classes, grid_s, cancel)?;
        let mut out: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        let mut records = Vec::new();
        let (mut change, mut window_limited) = (0.0_f64, false);
        for e in &events {
            let spec = &classes[&e.class];
            let q_star = thresholds[&e.class]
                .q_star_w_per_m3
                .expect("evaluated thresholds carry q*");
            let fixed_s = fixed.durations_s[&e.class];
            let old = used
                .get(&e.component)
                .and_then(|d| d.get(e.k as usize - 1))
                .copied()
                .unwrap_or(fixed_s);
            let mut record = EventRecord {
                class: e.class.clone(),
                component: e.component.clone(),
                k: e.k,
                start_s: e.start_s,
                end_s: e.end_s,
                fixed_duration_s: fixed_s,
                duration_used_s: old,
                cooldown_s: None,
                work_s: spec.work_s,
                duration_computed_s: None,
                window_limited: false,
                q_star,
                why: None,
                not_evaluated: None,
            };
            let ec = event_curve(e, spec, &curves, &index, assumptions.cooling.max_s);
            let cd = ec.and_then(|ec| {
                cooldown(&ec.curve, q_star, assumptions.cooling.max_s).map(|cd| (ec, cd))
            });
            let (ec, cd) = match cd {
                Ok(v) => v,
                Err(ne) => {
                    let reason = format!(
                        "iteration {number}: {} replacement {} at {:.0} s: {}",
                        e.component, e.k, e.start_s, ne.reason
                    );
                    record.not_evaluated = Some(not_evaluated(&reason, ne.next_step.clone()));
                    records.push(record);
                    iterations.push(IterationRecord {
                        iteration: number,
                        durations_in_s: used,
                        durations_out_s: out,
                        max_change_s: change,
                        window_limited,
                        events: records,
                    });
                    return Ok(failed(not_evaluated(reason, ne.next_step), iterations));
                }
            };
            let new = spec.work_s + cd.cooldown_s;
            let remaining = horizon - e.start_s;
            change = change.max((new.min(remaining) - old.min(remaining)).abs());
            window_limited |= cd.window_limited;
            out.entry(e.component.clone()).or_default().push(new);
            record.cooldown_s = Some(cd.cooldown_s);
            record.duration_computed_s = Some(new);
            record.window_limited = cd.window_limited;
            record.why = Some(why(spec, &ec, cd.cooldown_s));
            records.push(record);
        }
        progress(name, number, &format!("max change {:.3} d", change / DAY_S));
        iterations.push(IterationRecord {
            iteration: number,
            durations_in_s: used.clone(),
            durations_out_s: out.clone(),
            max_change_s: change,
            window_limited,
            events: records,
        });
        if change <= assumptions.convergence_s && !window_limited {
            return Ok(ComputedResult {
                status: Status::Evaluated,
                not_evaluated: None,
                summary: Some(summarize(&history, classes)),
                converged_at_iteration: Some(number),
                iterations,
            });
        }
        used = out;
    }
    let last = iterations.last();
    let limited = last.is_some_and(|i| i.window_limited);
    let reason = format!(
        "no convergence in {} iterations (last change {:.3} d{})",
        assumptions.max_iterations,
        last.map_or(0.0, |i| i.max_change_s) / DAY_S,
        if limited {
            ", a cooldown still limited by its window"
        } else {
            ""
        }
    );
    Ok(failed(
        not_evaluated(
            reason,
            "raise max_iterations or convergence_s, or inspect the last two iterations for durations that alternate",
        ),
        iterations,
    ))
}

fn contrasts(designs: &BTreeMap<String, DesignResult>) -> Vec<Contrast> {
    let names: Vec<&String> = designs.keys().collect();
    let mut out = Vec::new();
    for (i, a) in names.iter().enumerate() {
        for b in &names[i + 1..] {
            let (da, db) = (&designs[*a], &designs[*b]);
            let fixed_difference_s =
                da.fixed.total_replacement_downtime_s - db.fixed.total_replacement_downtime_s;
            let computed = match (&da.computed.summary, &db.computed.summary) {
                (Some(x), Some(y)) => {
                    Some(x.total_replacement_downtime_s - y.total_replacement_downtime_s)
                }
                _ => None,
            };
            let mut ratio = None;
            let mut ne = None;
            match computed {
                None => {
                    ne = Some(not_evaluated(
                        "a design has no computed downtime",
                        "see that design's computed NOT_EVALUATED reason",
                    ));
                }
                Some(c) if fixed_difference_s.abs() < CONTRAST_MIN_FIXED_S => {
                    ne = Some(not_evaluated(
                        "the fixed downtime difference is under 30 days, so the ratio of computed to fixed is not meaningful",
                        format!("compare the absolute differences ({c:.0} s computed)"),
                    ));
                }
                Some(c) => ratio = Some(c / fixed_difference_s),
            }
            out.push(Contrast {
                a: (*a).clone(),
                b: (*b).clone(),
                fixed_difference_s,
                computed_difference_s: computed,
                ratio_computed_over_fixed: ratio,
                not_evaluated: ne,
            });
        }
    }
    out
}

/// Runs the fixed and the coupled model for every design. `Err` is bad input, a tool
/// failure or cancellation; a physics gap is NOT_EVALUATED inside the result and does not
/// stop the other designs. `progress` gets (design, iteration, message).
pub fn run_maintenance(
    assumptions: &MaintenanceAssumptions,
    designs: &[DesignInput],
    runner: &mut dyn HistoryRunner,
    source: &mut dyn DecaySource,
    cancel: &Cancellation,
    progress: &mut dyn FnMut(&str, u32, &str),
) -> Result<MaintenanceResult, String> {
    assumptions.validate()?;
    if designs.is_empty() {
        return Err("at least one design is required".into());
    }
    let mut names = BTreeSet::new();
    for d in designs {
        if d.name.trim().is_empty() || !names.insert(d.name.as_str()) {
            return Err("design names must be nonempty and unique".into());
        }
        d.history_assumptions
            .validate()
            .map_err(|e| format!("design {}: {e}", d.name))?;
    }
    for spec in assumptions.classes.values() {
        if let Threshold::Calibrate { design, .. } = &spec.threshold
            && !names.contains(design.as_str())
        {
            return Err(format!(
                "calibration design {design} is not among the designs"
            ));
        }
    }
    let grid_s = cooling_grid_s(&assumptions.cooling);

    let mut fixed = Vec::new();
    for input in designs {
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        let mut durations_s = BTreeMap::new();
        for (class, spec) in &assumptions.classes {
            durations_s.insert(
                class.clone(),
                fixed_duration_s(&input.history_assumptions, &spec.component_id)
                    .map_err(|e| format!("design {}: {e}", input.name))?,
            );
        }
        progress(&input.name, 0, "fixed-duration history");
        let history = runner.run(&input.name, &input.history_assumptions, cancel)?;
        fixed.push(Fixed {
            input,
            history,
            durations_s,
        });
    }

    let thresholds = calibrate_thresholds(assumptions, &fixed, source, &grid_s, cancel, progress)?;

    let mut results = BTreeMap::new();
    for f in &fixed {
        let computed = coupled_case(
            assumptions,
            f,
            &thresholds,
            runner,
            source,
            &grid_s,
            cancel,
            progress,
        )?;
        results.insert(
            f.input.name.clone(),
            DesignResult {
                fixed: summarize(&f.history, &assumptions.classes),
                computed,
                thresholds: thresholds.clone(),
            },
        );
    }
    let contrasts = contrasts(&results);
    Ok(MaintenanceResult {
        schema_version: MAINTENANCE_RESULT_VERSION.into(),
        inputs: BTreeMap::new(),
        assumptions: assumptions.clone(),
        designs: results,
        contrasts,
        decay_source: None,
    })
}

pub mod actinv;

#[cfg(test)]
mod tests;
