//! The uncertainty state every history and comparison output carries: an interval with the basis
//! it comes from, or the reason there is none and what to do next. A number is never left bare.

use serde::{Deserialize, Serialize};

/// Half-width of the band a time, count or margin is shown with, in standard errors.
pub use crate::comparison::MARGIN_BAND_SIGMAS;

/// Why a single history carries no interval on a plant output.
pub const DETERMINISTIC_REASON: &str = "deterministic function of the authored inputs and the recorded transport means; the transport and input uncertainty is not propagated through a single history";
/// Basis text of a replacement-count range.
pub const COUNT_BAND_BASIS: &str = "2 sigma of the flux that drives each limit, to first order; the mean run's schedule, outages and other replacements are held fixed, so operating time gained or lost near the horizon is not counted; no nuclear data, model or authored-input uncertainty";
/// What to do about it.
pub const ENSEMBLE_NEXT_STEP: &str = "run the history ensemble for this arrangement to get a range from the transport sampling error";

/// What is known about the uncertainty of one output number.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OutputUncertainty {
    /// An interval in the quantity's own unit. `upper` is `None` when the interval has no upper
    /// edge (for example the flux band reaches zero, so the limit may never be reached).
    Interval {
        lower: f64,
        upper: Option<f64>,
        /// Where the interval comes from, including what it leaves out.
        basis: String,
    },
    /// No interval, and why; `next_step` says what would give one.
    NoInterval { why: String, next_step: String },
}

impl OutputUncertainty {
    pub fn interval(lower: f64, upper: Option<f64>, basis: impl Into<String>) -> Self {
        OutputUncertainty::Interval {
            lower,
            upper,
            basis: basis.into(),
        }
    }

    pub fn none(why: impl Into<String>, next_step: impl Into<String>) -> Self {
        OutputUncertainty::NoInterval {
            why: why.into(),
            next_step: next_step.into(),
        }
    }

    /// The same state in another unit: both edges multiplied by `factor`.
    pub fn scaled(&self, factor: f64) -> Self {
        match self {
            OutputUncertainty::Interval {
                lower,
                upper,
                basis,
            } => OutputUncertainty::Interval {
                lower: lower * factor,
                upper: upper.map(|u| u * factor),
                basis: basis.clone(),
            },
            other => other.clone(),
        }
    }

    /// The state of a number that has no value in this result.
    pub fn absent(what: &str) -> Self {
        Self::none(
            format!("{what} has no value here, so there is nothing to qualify"),
            "nothing to do unless the value should exist; check the assumptions that govern it",
        )
    }

    /// The reason every plant output of a single history has no interval.
    pub fn deterministic() -> Self {
        Self::none(DETERMINISTIC_REASON, ENSEMBLE_NEXT_STEP)
    }

    pub fn has_interval(&self) -> bool {
        matches!(self, OutputUncertainty::Interval { .. })
    }

    /// One line for a table cell or a brief: the interval and its basis, or the reason and the
    /// next step. `decimals` applies to the interval edges.
    pub fn text(&self, decimals: usize) -> String {
        match self {
            OutputUncertainty::Interval {
                lower,
                upper: Some(upper),
                basis,
            } => format!("{lower:.decimals$} to {upper:.decimals$} ({basis})"),
            OutputUncertainty::Interval {
                lower,
                upper: None,
                basis,
            } => format!("from {lower:.decimals$}, no upper edge ({basis})"),
            OutputUncertainty::NoInterval { why, next_step } => {
                format!("no interval, because {why} (next step: {next_step})")
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn text_names_the_basis_or_the_reason_and_next_step() {
        let band = OutputUncertainty::interval(1.0, Some(2.0), "2 sigma");
        assert_eq!(band.text(1), "1.0 to 2.0 (2 sigma)");
        assert!(band.has_interval());
        let open = OutputUncertainty::interval(1.0, None, "2 sigma");
        assert!(open.text(1).contains("no upper edge"));
        let none = OutputUncertainty::deterministic();
        let text = none.text(1);
        assert!(text.contains("because") && text.contains("next step"));
        assert!(!none.has_interval());
    }

    #[test]
    fn serialised_form_is_tagged_and_round_trips() {
        for value in [
            OutputUncertainty::interval(1.0, None, "b"),
            OutputUncertainty::deterministic(),
        ] {
            let json = serde_json::to_string(&value).unwrap();
            assert!(json.contains("\"state\""));
            assert_eq!(
                serde_json::from_str::<OutputUncertainty>(&json).unwrap(),
                value
            );
        }
    }

    /// A number-bearing JSON value: a number, or an array or object holding one.
    fn bears_a_number(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Number(_) => true,
            serde_json::Value::Array(items) => items.iter().any(bears_a_number),
            serde_json::Value::Object(map) => map.values().any(bears_a_number),
            _ => false,
        }
    }

    /// Fails if a numeric field of the serialised result has no uncertainty state, if a state
    /// names no field, or if a state is empty (an interval with a non-finite edge, or a reason
    /// or next step with no words). `skip` names the state table's own field.
    pub(crate) fn assert_every_number_has_a_state(
        result: &serde_json::Value,
        states: &std::collections::BTreeMap<String, OutputUncertainty>,
        skip: &[&str],
    ) {
        let fields = result
            .as_object()
            .expect("a result serialises to an object");
        let mut numeric = 0;
        for (name, value) in fields {
            if skip.contains(&name.as_str()) || !bears_a_number(value) {
                continue;
            }
            numeric += 1;
            assert!(
                states.contains_key(name),
                "numeric field `{name}` has neither an interval nor a reason it has none"
            );
        }
        assert!(
            numeric > 0,
            "the fixture has no numeric field, so the audit proves nothing"
        );
        for (name, state) in states {
            assert!(fields.contains_key(name), "state `{name}` names no field");
            match state {
                OutputUncertainty::Interval {
                    lower,
                    upper,
                    basis,
                } => {
                    assert!(
                        lower.is_finite() && upper.is_none_or(|u| u.is_finite() && u >= *lower)
                    );
                    assert!(!basis.trim().is_empty(), "`{name}` interval has no basis");
                }
                OutputUncertainty::NoInterval { why, next_step } => assert!(
                    !why.trim().is_empty() && !next_step.trim().is_empty(),
                    "`{name}` says no interval without a reason and a next step"
                ),
            }
        }
    }
}
