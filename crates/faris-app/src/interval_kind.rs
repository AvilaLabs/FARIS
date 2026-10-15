//! The kind label of an uncertainty interval (UNC-016).
//!
//! A number carries a kind (calculated, authored, literature, ...). An interval around a result
//! is only as firm as the least firm input that produced it, so its label is the weakest kind
//! among its inputs. The history ensembles re-run the operating history on rates drawn from the
//! recorded transport results (calculated) while the operating assumptions stay as the author
//! set them (authored), so their intervals are labelled authored and say why on hover.

use crate::badge::Kind;

/// One input of an interval, with the kind it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntervalInput {
    pub name: &'static str,
    pub kind: Kind,
}

/// The inputs of every operating-history interval and band.
pub const HISTORY_INPUTS: [IntervalInput; 2] = [
    IntervalInput {
        name: "the recorded transport rates, sampled from their means and covariance",
        kind: Kind::Calculated,
    },
    IntervalInput {
        name: "the operating assumptions (availability, service limits, tritium processing), held at their authored values",
        kind: Kind::Authored,
    },
];

/// The kind of an interval built from `inputs`: the weakest of theirs.
pub fn interval_kind(inputs: &[IntervalInput]) -> Option<Kind> {
    Kind::weakest(inputs.iter().map(|i| i.kind))
}

/// The label text of the kind badge on an interval.
pub fn interval_label(kind: Kind) -> String {
    format!("interval: {}", kind.default_label())
}

/// Hover text: the kind and why, naming each input with its kind.
pub fn interval_explanation(inputs: &[IntervalInput]) -> String {
    let Some(kind) = interval_kind(inputs) else {
        return String::new();
    };
    let list = inputs
        .iter()
        .map(|i| format!("{} ({})", i.name, i.kind.default_label()))
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "This interval is {} because an interval is only as firm as the least firm input it was built from. Inputs: {list}. The interval has the same unit as the value beside it.",
        kind.default_label()
    )
}

/// The kind badge for an operating-history interval; the hover text gives the inputs.
pub fn history_interval_badge(ui: &mut eframe::egui::Ui) {
    if let Some(kind) = interval_kind(&HISTORY_INPUTS) {
        crate::badge::badge(
            ui,
            kind,
            &interval_label(kind),
            &interval_explanation(&HISTORY_INPUTS),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faris_engine::{
        fixtures::{assumptions, rates_with_covariance},
        history::run_operating_history,
        history_ensemble::{EnsembleSettings, run_history_ensemble},
        history_uncertainty::history_rows,
        jobs::Cancellation,
    };

    const KINDS: [Kind; 8] = [
        Kind::Calculated,
        Kind::Checked,
        Kind::Authored,
        Kind::Literature,
        Kind::Conditional,
        Kind::Partial,
        Kind::NotEvaluated,
        Kind::Failed,
    ];

    // Verifies: UNC-016
    #[test]
    fn the_interval_kind_is_the_weakest_input_kind_for_every_combination() {
        for a in KINDS {
            for b in KINDS {
                for c in KINDS {
                    let inputs = [a, b, c].map(|kind| IntervalInput { name: "x", kind });
                    let got = interval_kind(&inputs).unwrap();
                    let floor = [a, b, c].iter().map(|k| k.firmness()).min().unwrap();
                    assert_eq!(got.firmness(), floor, "{a:?} {b:?} {c:?}");
                    assert!([a, b, c].contains(&got), "the kind is one of the inputs'");
                }
            }
        }
        assert_eq!(interval_kind(&[]), None);
    }

    // Verifies: UNC-016
    #[test]
    fn a_history_interval_is_authored_and_the_hover_names_every_input() {
        assert_eq!(interval_kind(&HISTORY_INPUTS), Some(Kind::Authored));
        let text = interval_explanation(&HISTORY_INPUTS);
        for input in HISTORY_INPUTS {
            assert!(text.contains(input.name), "{text}");
        }
        assert!(text.contains("same unit"));
        assert_eq!(interval_label(Kind::Authored), "interval: authored");
    }

    // Verifies: UNC-016
    #[test]
    fn every_continuous_range_carries_the_unit_of_its_nominal_value() {
        let history =
            run_operating_history(&assumptions(), &rates_with_covariance(0.06, 0.4, 'a')).unwrap();
        let settings = EnsembleSettings {
            samples: 8,
            seed: Some(7),
            threads: Some(1),
        };
        let ensemble = run_history_ensemble(
            &history_rates('a'),
            &assumptions(),
            &settings,
            &Cancellation::default(),
            &|_, _| {},
        )
        .unwrap();
        let rows = history_rows(&history, Some(&ensemble));
        let mut checked = 0;
        for row in &rows {
            let Some(result) = &row.result else { continue };
            let Some(unit) = row.nominal.split_whitespace().last() else {
                continue;
            };
            if row.nominal.split_whitespace().count() < 2
                || unit.chars().all(|c| c.is_ascii_digit())
            {
                continue; // a count, or text such as an outcome: no unit
            }
            assert!(
                result.text.contains(unit),
                "{}: range '{}' lacks the unit '{unit}' of '{}'",
                row.label,
                result.text,
                row.nominal
            );
            checked += 1;
        }
        assert!(checked >= 2, "the unit rows were checked ({checked})");
    }

    fn history_rates(artifact: char) -> faris_engine::history::TransportDrivingRates {
        rates_with_covariance(0.06, 0.4, artifact)
    }
}
