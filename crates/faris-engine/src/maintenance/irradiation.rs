//! The irradiation schedule an activation solver is given, taken from the operating history.
//!
//! The history (`crate::history`) records when the source is on (`operation_started` /
//! `operation_stopped` events) and the power fraction at each snapshot. This module turns one
//! installation's part of that into piecewise-constant irradiation steps `(duration, flux
//! scale)`, the form ACTINV's schedule takes, and records which history it came from.
//!
//! Two layers, both stated in `docs/OPERATING_HISTORY.md`:
//!
//! 1. [`full_steps`] is the unlumped schedule: one step per snapshot span while operating and
//!    one zero-flux step per gap, adjacent equal scales merged (exact, no approximation).
//! 2. [`condense`] lumps runs of short pulses and short gaps into single steps that keep the
//!    run's start, end and full-power-equivalent time. It lumps only a run no longer than
//!    `T_min * ln(1 + tolerance) / ln 2`, so every nuclide with a half-life of at least `T_min`
//!    produced directly from a stable target ends up within `tolerance` (relative) of the
//!    unlumped inventory, at the end of irradiation and at every cooling time after it. A gap
//!    or a run longer than that is never averaged away.
//!
//! The receipt ([`IrradiationSchedule`]) carries the history's SHA-256, the window, the rule and
//! the steps, and an input hash over all of them, so a changed history changes the activation
//! input hash.

use super::actinv::installations;
use super::history_end_s;
use crate::history::{EventKind, HistoryResult};
use faris_model::math;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const SCHEDULE_SCHEMA: &str = "faris-irradiation-schedule/v0.1";

/// Largest number of unlumped steps accepted, a guard against a runaway history.
pub const MAX_STEPS: usize = 5_000_000;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IrradiationStep {
    pub duration_s: f64,
    /// Flux scale relative to the transport flux; 0 is a zero-flux gap.
    pub flux_scale: f64,
}

/// How the unlumped schedule is condensed. `tolerance == 0` lumps nothing.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CondensationRule {
    /// Relative inventory error allowed for nuclides at or above the half-life below.
    pub tolerance: f64,
    /// Shortest half-life, in seconds, the tolerance is guaranteed for.
    pub shortest_half_life_s: f64,
}

impl CondensationRule {
    pub const LOSSLESS: Self = Self {
        tolerance: 0.0,
        shortest_half_life_s: 1.0,
    };

    /// Longest run of steps that may become one step.
    pub fn max_block_s(&self) -> f64 {
        self.shortest_half_life_s * math::ln_1p(self.tolerance) / std::f64::consts::LN_2
    }

    fn validate(&self) -> Result<(), String> {
        if !self.tolerance.is_finite() || self.tolerance < 0.0 {
            return Err("condensation tolerance must be finite and nonnegative".into());
        }
        if !self.shortest_half_life_s.is_finite() || self.shortest_half_life_s <= 0.0 {
            return Err("condensation shortest half-life must be finite and positive".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CondensationReport {
    pub unlumped_step_count: usize,
    pub condensed_step_count: usize,
    pub lumped_block_count: usize,
    pub longest_block_s: f64,
    /// Zero-flux gaps longer than the block limit, all kept as written.
    pub outages_kept: usize,
    /// Full-power-equivalent seconds, `sum(duration * flux_scale)`, before and after.
    pub full_power_equivalent_s_unlumped: f64,
    pub full_power_equivalent_s_condensed: f64,
    /// Upper bound of the relative inventory error for a nuclide of at least
    /// `shortest_half_life_s`, produced directly from a stable target (`e^(lambda L) - 1`).
    pub relative_error_bound: f64,
    /// What the bound does not cover.
    pub not_covered: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IrradiationSchedule {
    pub schema: String,
    pub history_sha256: String,
    pub component: Option<String>,
    pub window_start_s: f64,
    pub window_end_s: f64,
    pub rule: CondensationRule,
    pub steps: Vec<IrradiationStep>,
    pub report: CondensationReport,
    /// SHA-256 over the schema, history hash, component, window, rule and steps.
    pub input_sha256: String,
}

const NOT_COVERED: &str = "daughters of a decay chain, targets burned up during irradiation, and nuclides with a half-life below the stated shortest; the bound assumes the flux scale multiplies a fixed production rate";

/// SHA-256 of the history as serialised, the hash an activation receipt names.
pub fn history_sha256(history: &HistoryResult) -> Result<String, String> {
    let bytes = serde_json::to_vec(history).map_err(|e| format!("history: {e}"))?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

/// Source-on intervals from the operation events; empty ones dropped.
fn operating_intervals(history: &HistoryResult) -> Result<Vec<(f64, f64)>, String> {
    let mut events: Vec<_> = history
        .events
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                EventKind::OperationStarted | EventKind::OperationStopped
            )
        })
        .collect();
    events.sort_by(|a, b| a.time_s.total_cmp(&b.time_s).then(a.order.cmp(&b.order)));
    let end = history_end_s(history);
    let (mut out, mut start) = (Vec::new(), None);
    for e in events {
        if e.kind == EventKind::OperationStarted {
            if start.is_some() {
                return Err(format!(
                    "operation_started at {} while already operating",
                    e.time_s
                ));
            }
            start = Some(e.time_s);
        } else {
            let from = start
                .take()
                .ok_or_else(|| format!("operation_stopped at {} while not operating", e.time_s))?;
            if e.time_s > from {
                out.push((from, e.time_s));
            }
        }
    }
    if let Some(from) = start
        && end > from
    {
        out.push((from, end));
    }
    Ok(out)
}

/// `(start, end, flux scale)` pieces of one source-on interval: each operating snapshot's power
/// fraction holds until the next snapshot; before the first one inside the interval the first
/// applies. With no operating snapshot inside, the authored operation window covering the
/// interval start is used.
fn interval_pieces(
    history: &HistoryResult,
    (a, b): (f64, f64),
) -> Result<Vec<(f64, f64, f64)>, String> {
    let snaps = &history.snapshots;
    let lo = snaps.partition_point(|s| s.time_s < a);
    let hi = snaps.partition_point(|s| s.time_s < b);
    let mut ops = snaps[lo..hi].iter().filter(|s| s.operating);
    let Some(first) = ops.next() else {
        return history
            .assumptions
            .operation
            .iter()
            .find(|w| w.start_s <= a && a < w.end_s)
            .map(|w| vec![(a, b, w.power_fraction)])
            .ok_or_else(|| {
                format!(
                    "no power fraction for the operating span starting at {a} s: no operating \
                     snapshot and no authored operation window; run the history with snapshots on"
                )
            });
    };
    let (mut pieces, mut cursor, mut power) = (Vec::new(), a, first.power_fraction);
    for s in std::iter::once(first).chain(ops) {
        if s.time_s > cursor {
            pieces.push((cursor, s.time_s, power));
            cursor = s.time_s;
        }
        power = s.power_fraction;
    }
    pieces.push((cursor, b, power));
    Ok(pieces)
}

fn cuts_between(times: impl Iterator<Item = f64>, a: f64, b: f64) -> Vec<f64> {
    let mut cuts: Vec<f64> = times.filter(|t| *t > a && *t < b).collect();
    cuts.sort_by(f64::total_cmp);
    cuts.dedup();
    cuts
}

/// The unlumped steps of `window`, cut also at each of `cut_times_s` so that a downstream
/// shutdown time stays a step boundary. Adjacent equal scales are merged exactly.
pub fn full_steps(
    history: &HistoryResult,
    window: (f64, f64),
    cut_times_s: &[f64],
) -> Result<Vec<IrradiationStep>, String> {
    let (w0, w1) = window;
    if !(w0.is_finite() && w1.is_finite()) || w1 < w0 {
        return Err(format!("irradiation window [{w0}, {w1}] is not valid"));
    }
    let mut raw: Vec<(f64, f64)> = Vec::new();
    let mut push = |dt: f64, m: f64| {
        if dt > 0.0 {
            match raw.last_mut() {
                Some(last) if last.1 == m => last.0 += dt,
                _ => raw.push((dt, m)),
            }
        }
    };
    let mut cursor = w0;
    for interval in operating_intervals(history)? {
        let (a, b) = (interval.0.max(w0), interval.1.min(w1));
        if b <= a {
            continue;
        }
        push(a - cursor, 0.0);
        for (from, to, power) in interval_pieces(history, interval)? {
            push(to.min(b) - from.max(a), power);
        }
        cursor = b;
    }
    push(w1 - cursor, 0.0);
    if raw.len() > MAX_STEPS {
        return Err(format!(
            "more than {MAX_STEPS} irradiation steps; lengthen the snapshot interval of the history"
        ));
    }
    // Cut at the requested times too (gaps included), after merging, so each is a boundary.
    let mut steps = Vec::with_capacity(raw.len());
    let mut t = w0;
    for (dt, m) in raw {
        let end = t + dt;
        let mut from = t;
        for to in cuts_between(cut_times_s.iter().copied(), t, end)
            .into_iter()
            .chain(std::iter::once(end))
        {
            steps.push(IrradiationStep {
                duration_s: to - from,
                flux_scale: m,
            });
            from = to;
        }
        t = end;
    }
    Ok(steps)
}

fn fpe(steps: &[IrradiationStep]) -> f64 {
    steps.iter().map(|s| s.duration_s * s.flux_scale).sum()
}

/// Lump runs of steps. A run starts and ends on a flux step, holds only whole steps, never
/// crosses a time in `cut_offsets_s` (seconds from the first step's start), and is no longer
/// than the rule's block limit; a run of one step is left alone. A lumped run keeps its start,
/// end and full-power-equivalent time, so its flux scale is their ratio.
pub fn condense(
    steps: &[IrradiationStep],
    rule: &CondensationRule,
    cut_offsets_s: &[f64],
) -> Result<(Vec<IrradiationStep>, CondensationReport), String> {
    rule.validate()?;
    let limit = rule.max_block_s();
    let mut starts = Vec::with_capacity(steps.len() + 1);
    let mut t = 0.0;
    for s in steps {
        starts.push(t);
        t += s.duration_s;
    }
    starts.push(t);
    let crosses = |a: f64, b: f64| cut_offsets_s.iter().any(|c| *c > a && *c < b);
    let (mut out, mut blocks, mut longest, mut kept) = (Vec::new(), 0usize, 0.0f64, 0usize);
    let mut i = 0;
    while i < steps.len() {
        let s = steps[i];
        if s.flux_scale == 0.0 {
            if s.duration_s > limit {
                kept += 1;
            }
            out.push(s);
            i += 1;
            continue;
        }
        // Extend over (gap, pulse) pairs while the run stays within the limit.
        let mut last = i;
        let mut j = i + 2;
        while j < steps.len()
            && steps[j].flux_scale != 0.0
            && steps[j - 1].flux_scale == 0.0
            && starts[j + 1] - starts[i] <= limit
            && !crosses(starts[i], starts[j + 1])
        {
            last = j;
            j += 2;
        }
        if last == i {
            out.push(s);
            i += 1;
            continue;
        }
        let length = starts[last + 1] - starts[i];
        out.push(IrradiationStep {
            duration_s: length,
            flux_scale: fpe(&steps[i..=last]) / length,
        });
        blocks += 1;
        longest = longest.max(length);
        i = last + 1;
    }
    let lambda_max = std::f64::consts::LN_2 / rule.shortest_half_life_s;
    let report = CondensationReport {
        unlumped_step_count: steps.len(),
        condensed_step_count: out.len(),
        lumped_block_count: blocks,
        longest_block_s: longest,
        outages_kept: kept,
        full_power_equivalent_s_unlumped: fpe(steps),
        full_power_equivalent_s_condensed: fpe(&out),
        relative_error_bound: math::exp_m1(lambda_max * longest),
        not_covered: NOT_COVERED.into(),
    };
    Ok((out, report))
}

/// The receipt-carrying schedule of `component`'s installation in place at the given shutdown
/// time (or of the whole history when `None`), condensed by `rule`. `shutdown_times_s` stay
/// step boundaries.
pub fn irradiation_schedule(
    history: &HistoryResult,
    component: Option<(&str, f64)>,
    rule: &CondensationRule,
    shutdown_times_s: &[f64],
) -> Result<IrradiationSchedule, String> {
    let window = match component {
        None => (0.0, history_end_s(history)),
        Some((id, shutdown_s)) => installations(history, id)?
            .into_iter()
            .find(|(a, b)| *a < shutdown_s && shutdown_s <= *b)
            .ok_or_else(|| format!("{id}: no installation in place at {shutdown_s} s"))?,
    };
    let steps = full_steps(history, window, shutdown_times_s)?;
    let offsets: Vec<f64> = shutdown_times_s.iter().map(|t| t - window.0).collect();
    let (condensed, report) = condense(&steps, rule, &offsets)?;
    let mut schedule = IrradiationSchedule {
        schema: SCHEDULE_SCHEMA.into(),
        history_sha256: history_sha256(history)?,
        component: component.map(|(id, _)| id.to_string()),
        window_start_s: window.0,
        window_end_s: window.1,
        rule: *rule,
        steps: condensed,
        report,
        input_sha256: String::new(),
    };
    schedule.input_sha256 = schedule.compute_input_sha256()?;
    Ok(schedule)
}

impl IrradiationSchedule {
    fn compute_input_sha256(&self) -> Result<String, String> {
        let body = serde_json::json!({
            "schema": self.schema,
            "history_sha256": self.history_sha256,
            "component": self.component,
            "window": [self.window_start_s, self.window_end_s],
            "rule": self.rule,
            "steps": self.steps,
        });
        let bytes = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
        Ok(format!("{:x}", Sha256::digest(&bytes)))
    }

    /// True when the stored input hash matches the content.
    pub fn verify(&self) -> bool {
        self.compute_input_sha256()
            .is_ok_and(|h| h == self.input_sha256)
    }
}

/// Inventory of a nuclide of decay constant `lambda` (1/s) produced at unit rate times the flux
/// scale from a stable, undepleted target, at the end of `steps`.
pub fn direct_inventory(steps: &[IrradiationStep], lambda: f64) -> f64 {
    steps.iter().fold(0.0, |n, s| {
        let decay = math::exp(-lambda * s.duration_s);
        n * decay - s.flux_scale * math::exp_m1(-lambda * s.duration_s) / lambda
    })
}

#[cfg(test)]
mod tests;
