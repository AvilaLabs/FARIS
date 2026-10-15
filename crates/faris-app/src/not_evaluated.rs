//! Reasons a value is NOT_EVALUATED (UNC-054). Every NOT_EVALUATED the window shows carries a
//! reason code, why it is so and the next step, as text and in the hover or tap explanation.
//!
//! The reasons the window states itself are written once here. Reasons that come from the engine
//! (an ensemble's why and next step, a design check's tool messages) are given a code with
//! [`Reason::engine`], so the three parts are always present.

/// Why a value is not evaluated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reason {
    /// A stable code to quote: `NE-` and a name.
    pub code: &'static str,
    /// Why the value is not evaluated.
    pub why: &'static str,
    /// What to do about it.
    pub next: &'static str,
}

/// The transport results are cold-data surrogate Monte Carlo runs; none is qualified.
pub const NOT_QUALIFIED: Reason = Reason {
    code: "NE-NO-BENCHMARK",
    why: "The transport results come from a cold-data surrogate Monte Carlo run and have not been compared with measurements or an independently qualified code, so no claim of scientific qualification is made either way.",
    next: "Use the numbers as calculated results of this model only. Qualifying them needs a comparison with an independent benchmark, which this version does not do; the Evidence step lists what was recorded.",
};

/// No transport result is loaded for the arrangement.
pub const NO_TRANSPORT_RESULT: Reason = Reason {
    code: "NE-NO-TRANSPORT",
    why: "No transport result is loaded for this arrangement, so there is nothing to evaluate.",
    next: "Press 2 for Simulate and run transport, or open a study file with recorded results.",
};

/// The sampling uncertainty of the operating history could not be evaluated.
pub const ENSEMBLE_UNEVALUATED: &str = "NE-ENSEMBLE";

impl Reason {
    /// The code and the three parts as one sentence run, for status lines and hover text.
    pub fn line(&self) -> String {
        format!(
            "NOT_EVALUATED ({}). Why: {} Next step: {}",
            self.code, self.why, self.next
        )
    }

    /// A reason from the engine, which supplies its own why and next step as text.
    pub fn engine(code: &str, why: &str, next: &str) -> String {
        format!(
            "NOT_EVALUATED ({code}). Why: {} Next step: {}",
            sentence(why),
            sentence(next)
        )
    }
}

/// The text ending in one full stop.
fn sentence(text: &str) -> String {
    let text = text.trim();
    if text.ends_with('.') {
        text.to_owned()
    } else {
        format!("{text}.")
    }
}

/// True when `text` states NOT_EVALUATED with a code, a why and a next step.
#[cfg(test)]
pub fn is_complete(text: &str) -> bool {
    text.contains("NOT_EVALUATED (NE-") && text.contains("Why:") && text.contains("Next step:")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: UNC-054
    #[test]
    fn every_reason_the_window_states_has_a_code_why_and_next_step() {
        for reason in [NOT_QUALIFIED, NO_TRANSPORT_RESULT] {
            assert!(reason.code.starts_with("NE-"), "{}", reason.code);
            assert!(reason.why.len() > 30 && reason.next.len() > 30);
            assert!(is_complete(&reason.line()), "{}", reason.line());
        }
        assert_ne!(NOT_QUALIFIED.code, NO_TRANSPORT_RESULT.code);
    }

    // Verifies: UNC-054
    #[test]
    fn an_engine_reason_gets_a_code_and_both_parts() {
        let text = Reason::engine(
            ENSEMBLE_UNEVALUATED,
            "no covariance was recorded",
            "rerun the transport case with covariance output",
        );
        assert!(is_complete(&text), "{text}");
        assert!(text.contains("no covariance was recorded."));
        assert!(!is_complete("NOT_EVALUATED"));
        assert!(!is_complete("Qualification NOT_EVALUATED."));
    }

    // Verifies: UNC-054
    #[test]
    fn the_status_texts_the_window_shows_are_complete() {
        // Hover and message texts built in the panels come from the same constants.
        for text in [NOT_QUALIFIED.line(), NO_TRANSPORT_RESULT.line()] {
            assert!(is_complete(&text));
            assert!(!text.contains("  "));
        }
    }
}
