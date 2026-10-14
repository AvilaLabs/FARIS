//! Per-run time-step refinement flag for the operating history (VAL-017).
//!
//! The history is run at its authored `maximum_step_s` and again at half that step. If the
//! ledger totals at the horizon move by more than [`REFINEMENT_THRESHOLD`] (one percent), the
//! run is labelled not converged, naming the outputs that moved. A history whose halved run
//! cannot be made is NOT_EVALUATED with the reason, never converged by default. This is a
//! discretisation check on the ledger with fixed driving rates; it says nothing about the
//! sampling error of those rates or about the transport mesh.

use crate::history::{HistoryResult, TransportDrivingRates, run_operating_history};
use faris_model::history::OperatingHistoryAssumptions;
use serde::{Deserialize, Serialize};

/// Largest relative change of any total between the authored step and half of it.
pub const REFINEMENT_THRESHOLD: f64 = 0.01;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RefinementStatus {
    /// Every compared total moved by at most the threshold when the step was halved.
    Converged,
    /// At least one compared total moved by more than the threshold.
    NotConverged,
    /// The halved run could not be made or compared; see the reason.
    NotEvaluated,
}

/// One total compared between the two steps.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RefinementCheck {
    pub output: String,
    pub at_authored_step: f64,
    pub at_half_step: f64,
    /// |difference| over the larger magnitude; 0 when both are 0.
    pub relative_change: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct HistoryRefinement {
    pub status: RefinementStatus,
    pub threshold: f64,
    pub authored_step_s: f64,
    pub half_step_s: f64,
    pub checks: Vec<RefinementCheck>,
    /// Why the status is what it is, with the next step when it is not converged or not evaluated.
    pub reason: String,
}

fn relative_change(a: f64, b: f64) -> f64 {
    let scale = a.abs().max(b.abs());
    if scale == 0.0 {
        0.0
    } else {
        (a - b).abs() / scale
    }
}

fn not_evaluated(assumptions: &OperatingHistoryAssumptions, reason: String) -> HistoryRefinement {
    HistoryRefinement {
        status: RefinementStatus::NotEvaluated,
        threshold: REFINEMENT_THRESHOLD,
        authored_step_s: assumptions.maximum_step_s,
        half_step_s: assumptions.maximum_step_s / 2.0,
        checks: Vec::new(),
        reason,
    }
}

/// The totals at the end of a run that the refinement compares, by name.
fn totals(run: &HistoryResult) -> Option<Vec<(String, f64)>> {
    let last = run.snapshots.last()?;
    let mut out = vec![
        (
            "available_tritium_kg".to_string(),
            last.available_tritium_kg,
        ),
        ("in_process_tritium_kg".into(), last.in_process_tritium_kg),
        (
            "cumulative_production_kg".into(),
            last.cumulative_production_kg,
        ),
        ("cumulative_import_kg".into(), last.cumulative_import_kg),
        ("cumulative_burn_kg".into(), last.cumulative_burn_kg),
        (
            "cumulative_processing_loss_kg".into(),
            last.cumulative_processing_loss_kg,
        ),
        ("cumulative_decay_kg".into(), last.cumulative_decay_kg),
        (
            "cumulative_fusion_energy_mwh".into(),
            last.cumulative_fusion_energy_mwh,
        ),
    ];
    for (id, fluence) in &last.component_fluence_n_m2 {
        out.push((format!("fluence_n_m2:{id}"), *fluence));
    }
    for (id, count) in &last.component_replacements {
        out.push((format!("replacements:{id}"), f64::from(*count)));
    }
    Some(out)
}

/// Run the history at its step and at half of it and flag the run. Never panics; a history that
/// does not run, or whose halved run does not, is NOT_EVALUATED with the reason.
pub fn history_refinement(
    assumptions: &OperatingHistoryAssumptions,
    rates: &TransportDrivingRates,
) -> HistoryRefinement {
    let coarse = match run_operating_history(assumptions, rates) {
        Ok(run) => run,
        Err(error) => {
            return not_evaluated(
                assumptions,
                format!(
                    "The history does not run at its own step ({error}), so its step cannot be checked. Fix the history inputs first."
                ),
            );
        }
    };
    let mut half = assumptions.clone();
    half.maximum_step_s = assumptions.maximum_step_s / 2.0;
    let fine = match run_operating_history(&half, rates) {
        Ok(run) => run,
        Err(error) => {
            return not_evaluated(
                assumptions,
                format!(
                    "The history cannot be run at half its step ({error}). Raise the step so that half of it fits the segment budget, or shorten the horizon, and check again."
                ),
            );
        }
    };
    compare_runs(assumptions, &coarse, &fine)
}

/// Compare the totals of the authored-step run and the half-step run and flag the history.
fn compare_runs(
    assumptions: &OperatingHistoryAssumptions,
    coarse: &HistoryResult,
    fine: &HistoryResult,
) -> HistoryRefinement {
    let half_step_s = assumptions.maximum_step_s / 2.0;
    let (Some(coarse_totals), Some(fine_totals)) = (totals(coarse), totals(fine)) else {
        return not_evaluated(
            assumptions,
            "A run produced no snapshot to compare. Set a snapshot interval no longer than the horizon.".into(),
        );
    };
    if coarse.outcome != fine.outcome {
        return HistoryRefinement {
            status: RefinementStatus::NotConverged,
            threshold: REFINEMENT_THRESHOLD,
            authored_step_s: assumptions.maximum_step_s,
            half_step_s,
            checks: Vec::new(),
            reason: format!(
                "Not converged: the history ends {:?} at the authored step and {:?} at half of it. Lower the maximum step until the outcome stops changing.",
                coarse.outcome, fine.outcome
            ),
        };
    }
    if coarse_totals.len() != fine_totals.len()
        || coarse_totals
            .iter()
            .zip(&fine_totals)
            .any(|(a, b)| a.0 != b.0)
    {
        return not_evaluated(
            assumptions,
            "The two runs track different components, so their totals cannot be compared. Report this as a defect.".into(),
        );
    }
    let checks: Vec<RefinementCheck> = coarse_totals
        .into_iter()
        .zip(fine_totals)
        .map(|((output, a), (_, b))| RefinementCheck {
            output,
            at_authored_step: a,
            at_half_step: b,
            relative_change: relative_change(a, b),
        })
        .collect();
    let moved: Vec<&str> = checks
        .iter()
        .filter(|c| c.relative_change > REFINEMENT_THRESHOLD)
        .map(|c| c.output.as_str())
        .collect();
    let (status, reason) = if moved.is_empty() {
        (
            RefinementStatus::Converged,
            format!(
                "Converged: no total moved by more than {:.0} % when the step was halved.",
                REFINEMENT_THRESHOLD * 100.0
            ),
        )
    } else {
        (
            RefinementStatus::NotConverged,
            format!(
                "Not converged: {} moved by more than {:.0} % when the step was halved. Lower the maximum step and check again.",
                moved.join(", "),
                REFINEMENT_THRESHOLD * 100.0
            ),
        )
    };
    HistoryRefinement {
        status,
        threshold: REFINEMENT_THRESHOLD,
        authored_step_s: assumptions.maximum_step_s,
        half_step_s,
        checks,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference() -> (OperatingHistoryAssumptions, TransportDrivingRates) {
        let assumptions: OperatingHistoryAssumptions = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scenarios/arc-inspired/demountable-magnet-assumptions.json"
        )))
        .unwrap();
        let rates: TransportDrivingRates = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/history-release-rounding-rates.json"
        )))
        .unwrap();
        (assumptions, rates)
    }

    // Verifies: VAL-017
    #[test]
    fn the_reference_history_is_flagged_converged_at_its_authored_step() {
        let (assumptions, rates) = reference();
        let flag = history_refinement(&assumptions, &rates);
        assert_eq!(flag.status, RefinementStatus::Converged, "{flag:?}");
        assert!(!flag.checks.is_empty());
        assert!(
            flag.checks
                .iter()
                .all(|c| c.relative_change <= REFINEMENT_THRESHOLD)
        );
    }

    // Verifies: VAL-017
    #[test]
    fn a_run_whose_totals_move_when_the_step_is_halved_is_flagged_not_converged() {
        // The ledger integrates each segment in closed form, so a real history does not move with
        // its step. A half-step run that differs is therefore seeded: 2 % on the burn total, and
        // a replacement that appears only at the finer step.
        let (assumptions, rates) = reference();
        let coarse = run_operating_history(&assumptions, &rates).unwrap();
        let mut seeded = coarse.clone();
        let last = seeded.snapshots.last_mut().unwrap();
        last.cumulative_burn_kg *= 1.02;
        let flag = compare_runs(&assumptions, &coarse, &seeded);
        assert_eq!(flag.status, RefinementStatus::NotConverged);
        assert!(
            flag.reason.contains("cumulative_burn_kg"),
            "{}",
            flag.reason
        );
        assert!(flag.reason.contains("Lower the maximum step"));
        let mut seeded = coarse.clone();
        let last = seeded.snapshots.last_mut().unwrap();
        *last.component_replacements.values_mut().next().unwrap() += 1;
        let flag = compare_runs(&assumptions, &coarse, &seeded);
        assert_eq!(flag.status, RefinementStatus::NotConverged);
        assert!(flag.reason.contains("replacements:"), "{}", flag.reason);
        // A change at the threshold is still converged; just above it is not.
        let mut within = coarse.clone();
        within.snapshots.last_mut().unwrap().cumulative_burn_kg *= 1.0 + 0.009;
        assert_eq!(
            compare_runs(&assumptions, &coarse, &within).status,
            RefinementStatus::Converged
        );
    }

    #[test]
    fn a_history_that_cannot_run_is_not_evaluated_with_the_reason() {
        let (mut assumptions, rates) = reference();
        assumptions.maximum_step_s = 1.0e7;
        let flag = history_refinement(&assumptions, &rates);
        assert_eq!(flag.status, RefinementStatus::NotEvaluated);
        assert!(flag.checks.is_empty());
        assert!(flag.reason.contains("does not run") && flag.reason.contains("Fix"));
    }

    #[test]
    fn relative_change_is_symmetric_and_zero_for_two_zeros() {
        assert_eq!(relative_change(0.0, 0.0), 0.0);
        assert_eq!(relative_change(1.0, 1.0), 0.0);
        assert!((relative_change(1.0, 1.02) - relative_change(1.02, 1.0)).abs() < 1e-15);
        assert!((relative_change(100.0, 101.0) - 1.0 / 101.0).abs() < 1e-15);
    }
}
