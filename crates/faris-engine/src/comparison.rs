//! Descriptive paired operating-history comparisons with no uncertainty shortcut.
use crate::history::{HistoryResult, TransportDrivingRates};
use crate::jobs::Cancellation;
use faris_model::history::OperatingHistoryAssumptions;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HistorySensitivityGrid {
    pub recovery_fraction_levels: Vec<f64>,
    pub delay_multipliers: Vec<f64>,
    pub service_limit_multipliers: Vec<f64>,
    pub rationale: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HistorySensitivityPoint {
    pub recovery_fraction: f64,
    pub delay_multiplier: f64,
    pub service_limit_multiplier: f64,
    pub outcome: crate::history::HistoryOutcome,
    pub final_snapshot: crate::history::HistorySnapshot,
    pub event_count: usize,
    pub maximum_absolute_mass_balance_residual_kg: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HistorySensitivityResult {
    pub schema_version: String,
    pub rationale: String,
    pub base_assumptions: OperatingHistoryAssumptions,
    pub driving_rates: TransportDrivingRates,
    pub grid: HistorySensitivityGrid,
    pub points: Vec<HistorySensitivityPoint>,
    pub note: String,
}

/// Bounded full-history parameter reruns. These are authored scenario probes,
/// never a probability distribution or uncertainty interval.
pub fn run_history_sensitivity(
    assumptions: &OperatingHistoryAssumptions,
    rates: &TransportDrivingRates,
    grid: &HistorySensitivityGrid,
) -> Result<HistorySensitivityResult, String> {
    run_history_sensitivity_cancellable(assumptions, rates, grid, &Cancellation::default())
}

pub fn run_history_sensitivity_cancellable(
    assumptions: &OperatingHistoryAssumptions,
    rates: &TransportDrivingRates,
    grid: &HistorySensitivityGrid,
    cancellation: &Cancellation,
) -> Result<HistorySensitivityResult, String> {
    assumptions.validate()?;
    rates.validate()?;
    let valid_levels = |values: &[f64], min: f64, max: f64| {
        !values.is_empty()
            && values.len() <= 4
            && values
                .iter()
                .all(|v| v.is_finite() && *v >= min && *v <= max)
            && values
                .iter()
                .enumerate()
                .all(|(i, v)| !values[..i].contains(v))
    };
    if !valid_levels(&grid.recovery_fraction_levels, 0.0, 1.0)
        || !valid_levels(&grid.delay_multipliers, 0.25, 4.0)
        || !valid_levels(&grid.service_limit_multipliers, 0.25, 4.0)
        || grid.rationale.trim().is_empty()
    {
        return Err(
            "sensitivity requires distinct bounded levels (1..4 each) and a rationale".into(),
        );
    }
    if assumptions.service_limits.is_empty()
        && grid.service_limit_multipliers.iter().any(|m| *m != 1.0)
    {
        return Err("service-limit sensitivity requires declared service limits".into());
    }
    let count = grid.recovery_fraction_levels.len()
        * grid.delay_multipliers.len()
        * grid.service_limit_multipliers.len();
    if count > 64 {
        return Err("sensitivity grid exceeds the 64-rerun bound".into());
    }
    let mut points = Vec::with_capacity(count);
    for recovery in &grid.recovery_fraction_levels {
        for delay_factor in &grid.delay_multipliers {
            for limit_factor in &grid.service_limit_multipliers {
                if cancellation.is_cancelled() {
                    return Err("history sensitivity canceled; partial grid is not valid".into());
                }
                let mut case = assumptions.clone();
                case.recovery_fraction = *recovery;
                case.processing_delay_s *= delay_factor;
                if case.processing_delay_s > 0.0 {
                    case.maximum_step_s = case.maximum_step_s.min(case.processing_delay_s / 24.0);
                }
                for limit in &mut case.service_limits {
                    limit.limit *= limit_factor;
                }
                case.validate()?;
                let run =
                    crate::history::run_operating_history_cancellable(&case, rates, cancellation)?;
                let final_snapshot = run
                    .snapshots
                    .last()
                    .cloned()
                    .ok_or("history has no snapshots")?;
                let maximum_absolute_mass_balance_residual_kg = run
                    .snapshots
                    .iter()
                    .map(|s| s.mass_balance_residual_kg.abs())
                    .fold(0.0, f64::max);
                points.push(HistorySensitivityPoint {
                    recovery_fraction: *recovery,
                    delay_multiplier: *delay_factor,
                    service_limit_multiplier: *limit_factor,
                    outcome: run.outcome,
                    final_snapshot,
                    event_count: run.events.len(),
                    maximum_absolute_mass_balance_residual_kg,
                });
            }
        }
    }
    Ok(HistorySensitivityResult {
        schema_version: "faris-history-sensitivity/v0.1".into(),
        rationale: grid.rationale.clone(),
        base_assumptions: assumptions.clone(),
        driving_rates: rates.clone(),
        grid: grid.clone(),
        points,
        note: "Every point reruns the complete deterministic history. Levels are conditional user-authored scenarios, not random samples or uncertainty bounds; no transport covariance is inferred.".into(),
    })
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PairedHistoryComparison {
    pub schema_version: String,
    pub left_label: String,
    pub right_label: String,
    pub controlled_difference: String,
    pub dependence_note: String,
    pub left: HistoryRunSummary,
    pub right: HistoryRunSummary,
    pub differences: HistoryDifferences,
    pub interpretation: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HistoryRunSummary {
    pub outcome: crate::history::HistoryOutcome,
    pub assumptions: OperatingHistoryAssumptions,
    pub driving_rates: TransportDrivingRates,
    pub events: Vec<crate::history::HistoryEvent>,
    pub final_snapshot: crate::history::HistorySnapshot,
    pub energy_unavailable_reason: Option<String>,
    pub notice: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HistoryDifferences {
    pub produced_tritium_kg_right_minus_left: f64,
    pub burned_tritium_kg_right_minus_left: f64,
    pub full_power_seconds_right_minus_left: f64,
    pub net_electricity_mwh_right_minus_left: Option<f64>,
}

pub fn compare_histories(
    assumptions: &OperatingHistoryAssumptions,
    left_label: &str,
    left_rates: &TransportDrivingRates,
    right_label: &str,
    right_rates: &TransportDrivingRates,
    controlled_difference: &str,
) -> Result<PairedHistoryComparison, String> {
    compare_histories_cancellable(
        assumptions,
        left_label,
        left_rates,
        right_label,
        right_rates,
        controlled_difference,
        &Cancellation::default(),
    )
}

pub fn compare_histories_cancellable(
    assumptions: &OperatingHistoryAssumptions,
    left_label: &str,
    left_rates: &TransportDrivingRates,
    right_label: &str,
    right_rates: &TransportDrivingRates,
    controlled_difference: &str,
    cancellation: &Cancellation,
) -> Result<PairedHistoryComparison, String> {
    assumptions.validate()?;
    if left_label.trim().is_empty() || right_label.trim().is_empty() || left_label == right_label {
        return Err("comparison requires distinct nonempty variant labels".into());
    }
    if controlled_difference.trim().is_empty() {
        return Err("controlled_difference must identify the authored contrast".into());
    }
    left_rates.validate()?;
    right_rates.validate()?;
    if left_rates.reference_fusion_power_mw != right_rates.reference_fusion_power_mw
        || left_rates.total_reaction_energy_ev != right_rates.total_reaction_energy_ev
        || left_rates.primary_neutron_energy_ev != right_rates.primary_neutron_energy_ev
    {
        return Err(
            "paired comparison requires identical reference power and reaction convention".into(),
        );
    }
    let left =
        crate::history::run_operating_history_cancellable(assumptions, left_rates, cancellation)?;
    let right =
        crate::history::run_operating_history_cancellable(assumptions, right_rates, cancellation)?;
    let l = left
        .snapshots
        .last()
        .cloned()
        .ok_or("left history has no snapshots")?;
    let r = right
        .snapshots
        .last()
        .cloned()
        .ok_or("right history has no snapshots")?;
    let differences = HistoryDifferences {
        produced_tritium_kg_right_minus_left: r.cumulative_production_kg
            - l.cumulative_production_kg,
        burned_tritium_kg_right_minus_left: r.cumulative_burn_kg - l.cumulative_burn_kg,
        full_power_seconds_right_minus_left: r.cumulative_full_power_seconds
            - l.cumulative_full_power_seconds,
        net_electricity_mwh_right_minus_left: r
            .cumulative_net_electricity_mwh
            .zip(l.cumulative_net_electricity_mwh)
            .map(|(a, b)| a - b),
    };
    let summarize =
        |run: HistoryResult, final_snapshot: &crate::history::HistorySnapshot| HistoryRunSummary {
            outcome: run.outcome,
            assumptions: run.assumptions,
            driving_rates: run.driving_rates,
            events: run.events,
            final_snapshot: final_snapshot.clone(),
            energy_unavailable_reason: run.energy_unavailable_reason,
            notice: run.notice,
        };
    Ok(PairedHistoryComparison {
        schema_version: "faris-history-comparison/v0.1".into(),
        left_label: left_label.into(), right_label: right_label.into(),
        controlled_difference: controlled_difference.into(),
        dependence_note: "Both deterministic histories use the same authored assumptions. Joint Monte Carlo uncertainty of production, exposure and heat responses is not propagated through these histories. Differences are descriptive point estimates, not uncertainty-qualified effects.".into(),
        left: summarize(left, &l), right: summarize(right, &r), differences,
        interpretation: "Conditional paired code-to-code/model comparison only. No empirical qualification, service-life claim, or inferential significance is implied.".into(),
    })
}

/// True when two independent sampled estimates differ by more than twice the
/// combined standard error, `|b - a| > 2 sqrt(se_a² + se_b²)`. Covariance
/// between the runs is not modelled (the runs use distinct seeds), and any
/// non-finite or negative input is never reported as resolved.
pub fn difference_resolved_2sigma(a: f64, se_a: f64, b: f64, se_b: f64) -> bool {
    if ![a, se_a, b, se_b].iter().all(|v| v.is_finite()) || se_a < 0.0 || se_b < 0.0 {
        return false;
    }
    (b - a).abs() > 2.0 * (se_a * se_a + se_b * se_b).sqrt()
}

/// First-order relative sampling uncertainty of the time at which accumulated
/// fluence crosses a fixed limit. Fluence is flux times operating time, so the
/// crossing time scales as 1 / flux and its relative uncertainty equals the
/// relative standard error of the driving flux. Ignores outage-timing
/// nonlinearity and all model or data uncertainty. None for a non-positive
/// mean or a missing/invalid standard error.
pub fn first_crossing_relative_uncertainty(flux_mean: f64, flux_se: Option<f64>) -> Option<f64> {
    let se = flux_se?;
    (flux_mean.is_finite() && flux_mean > 0.0 && se.is_finite() && se >= 0.0)
        .then(|| se / flux_mean)
}

/// Replacement outages of one component as `(start_s, end_s)` pairs, taken
/// from `ReplacementStarted` and the following `ReplacementCompleted` event.
/// A replacement still open at the end of the history ends at `horizon_s`.
pub fn component_replacement_spans(
    events: &[crate::history::HistoryEvent],
    component_id: &str,
    horizon_s: f64,
) -> Vec<(f64, f64)> {
    use crate::history::EventKind;
    let mut spans = Vec::new();
    let mut open: Option<f64> = None;
    for event in events
        .iter()
        .filter(|e| e.component_id.as_deref() == Some(component_id))
    {
        match event.kind {
            EventKind::ReplacementStarted => open = Some(event.time_s),
            EventKind::ReplacementCompleted => {
                if let Some(start) = open.take() {
                    spans.push((start, event.time_s));
                }
            }
            _ => {}
        }
    }
    if let Some(start) = open {
        spans.push((start, horizon_s.max(start)));
    }
    spans
}

/// Why the plant is or is not producing at one instant, from recorded events
/// and authored outages. A presentation classification, not a new calculation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperatingState {
    Operating,
    MagnetReplacement,
    OtherReplacement,
    PlannedOutage,
    FuelLimited,
    Stopped,
}

pub fn classify_operating_state(
    events: &[crate::history::HistoryEvent],
    planned_outages: &[faris_model::history::TimeInterval],
    operating: bool,
    time_s: f64,
    horizon_s: f64,
) -> OperatingState {
    use crate::history::EventKind;
    if operating {
        return OperatingState::Operating;
    }
    let inside = |spans: Vec<(f64, f64)>| spans.iter().any(|(s, e)| *s <= time_s && time_s < *e);
    if inside(component_replacement_spans(events, "magnets", horizon_s)) {
        return OperatingState::MagnetReplacement;
    }
    let mut others: Vec<&str> = events
        .iter()
        .filter(|e| e.kind == EventKind::ReplacementStarted)
        .filter_map(|e| e.component_id.as_deref())
        .collect();
    others.sort_unstable();
    others.dedup();
    if others
        .into_iter()
        .filter(|c| *c != "magnets")
        .any(|c| inside(component_replacement_spans(events, c, horizon_s)))
    {
        return OperatingState::OtherReplacement;
    }
    if planned_outages
        .iter()
        .any(|o| o.start_s <= time_s && time_s < o.end_s)
    {
        return OperatingState::PlannedOutage;
    }
    let last_fuel = events.iter().filter(|e| e.time_s <= time_s).rfind(|e| {
        matches!(
            e.kind,
            EventKind::FuelUnavailable | EventKind::FuelAvailable
        )
    });
    if last_fuel.is_some_and(|e| e.kind == EventKind::FuelUnavailable) {
        OperatingState::FuelLimited
    } else {
        OperatingState::Stopped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{EventKind, HistoryEvent};

    fn event(time_s: f64, kind: EventKind, component: Option<&str>) -> HistoryEvent {
        HistoryEvent {
            time_s,
            order: 0,
            kind,
            component_id: component.map(Into::into),
            response_id: None,
            mass_kg: None,
            note: String::new(),
        }
    }

    // Verifies: NUC-046, UNC-084
    #[test]
    fn two_sigma_resolution_uses_combined_standard_error() {
        // Combined SE = 5, threshold 10.
        assert!(difference_resolved_2sigma(0.0, 3.0, 10.5, 4.0));
        assert!(!difference_resolved_2sigma(0.0, 3.0, 9.5, 4.0));
        assert!(difference_resolved_2sigma(10.5, 3.0, 0.0, 4.0));
        assert!(!difference_resolved_2sigma(1.0, 0.1, 1.0, 0.1));
        assert!(!difference_resolved_2sigma(f64::NAN, 1.0, 5.0, 1.0));
        assert!(!difference_resolved_2sigma(0.0, -1.0, 5.0, 1.0));
    }

    #[test]
    fn first_crossing_uncertainty_is_relative_flux_error() {
        let rel = first_crossing_relative_uncertainty(1.6e14, Some(0.23e14)).unwrap();
        assert!((rel - 0.14375).abs() < 1e-12);
        assert_eq!(first_crossing_relative_uncertainty(0.0, Some(1.0)), None);
        assert_eq!(first_crossing_relative_uncertainty(1.0, None), None);
        assert_eq!(first_crossing_relative_uncertainty(1.0, Some(-1.0)), None);
        assert_eq!(
            first_crossing_relative_uncertainty(1.0, Some(0.0)),
            Some(0.0)
        );
    }

    #[test]
    fn replacement_spans_pair_start_and_completion_per_component() {
        let events = vec![
            event(10.0, EventKind::ReplacementStarted, Some("magnets")),
            event(12.0, EventKind::ReplacementStarted, Some("blanket")),
            event(14.0, EventKind::ReplacementCompleted, Some("blanket")),
            event(20.0, EventKind::ReplacementCompleted, Some("magnets")),
            event(50.0, EventKind::ReplacementStarted, Some("magnets")),
        ];
        assert_eq!(
            component_replacement_spans(&events, "magnets", 60.0),
            vec![(10.0, 20.0), (50.0, 60.0)]
        );
        assert_eq!(
            component_replacement_spans(&events, "blanket", 60.0),
            vec![(12.0, 14.0)]
        );
        assert!(component_replacement_spans(&events, "shield", 60.0).is_empty());
    }

    #[test]
    fn operating_state_prefers_replacement_then_outage_then_fuel() {
        let events = vec![
            event(10.0, EventKind::ReplacementStarted, Some("magnets")),
            event(20.0, EventKind::ReplacementCompleted, Some("magnets")),
            event(30.0, EventKind::ReplacementStarted, Some("blanket")),
            event(33.0, EventKind::ReplacementCompleted, Some("blanket")),
            event(40.0, EventKind::FuelUnavailable, None),
            event(45.0, EventKind::FuelAvailable, None),
        ];
        let outages = vec![faris_model::history::TimeInterval {
            start_s: 100.0,
            end_s: 110.0,
            reason: "test".into(),
        }];
        let state = |operating, t| classify_operating_state(&events, &outages, operating, t, 200.0);
        assert_eq!(state(true, 15.0), OperatingState::Operating);
        assert_eq!(state(false, 15.0), OperatingState::MagnetReplacement);
        assert_eq!(state(false, 31.0), OperatingState::OtherReplacement);
        assert_eq!(state(false, 42.0), OperatingState::FuelLimited);
        assert_eq!(state(false, 47.0), OperatingState::Stopped);
        assert_eq!(state(false, 105.0), OperatingState::PlannedOutage);
    }
}
