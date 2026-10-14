//! The progress of `design_transport` in plain words, and an estimate of what is left.
//!
//! Shared by the command line and the desktop so that both word a run the same way. The
//! estimate follows from the Monte Carlo error falling as 1/sqrt(histories): at the error
//! reached after N histories, the target needs N times the square of (error / target).

use super::TransportProgress;

/// What stops the run first, going by the estimate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopsAt {
    Target,
    HistoryLimit,
    TimeLimit,
}

/// How far a sampling run has to go. An estimate, not a promise: the error of the first
/// batches is itself uncertain, and the speed of the run may change.
#[derive(Clone, Debug, PartialEq)]
pub struct SamplingEstimate {
    /// Histories in all that the target needs at the error reached so far.
    pub histories_needed: u64,
    /// Histories still to run for the target; 0 when the error is already within it.
    pub histories_remaining: u64,
    /// Seconds until the run stops, at the speed so far, for whichever stop comes first.
    pub seconds_remaining: f64,
    /// Elapsed share of the elapsed plus the remaining seconds, 0 to 1.
    pub fraction: f64,
    pub stops_at: StopsAt,
}

/// The step that has started, in the words the command line prints.
pub fn step_text(name: &str) -> &'static str {
    match name {
        "mgxs" => "preparing weight windows: condensing the nuclear data",
        "windows" => "preparing weight windows: solving for where neutrons matter",
        "volumes" => "measuring the pieces of each solid for the peak",
        _ => "starting the run",
    }
}

/// One progress update as a line of text.
pub fn progress_line(update: &TransportProgress) -> String {
    match update {
        TransportProgress::Step(name) => step_text(name).to_string(),
        TransportProgress::Sampling {
            histories,
            relative_error,
            peak_relative_error,
            elapsed_seconds,
        } => match (relative_error, peak_relative_error) {
            (Some(error), Some(peak)) => format!(
                "{histories} histories, {elapsed_seconds:.0} s: error {:.1} %, highest piece {:.1} %",
                100.0 * error,
                100.0 * peak
            ),
            (Some(error), None) => format!(
                "{histories} histories, {elapsed_seconds:.0} s: error {:.1} %",
                100.0 * error
            ),
            (None, _) => format!("{histories} histories, {elapsed_seconds:.0} s"),
        },
    }
}

/// Estimates what is left of a sampling run, or `None` while it cannot be told: no error yet
/// (fewer than two batches), nothing sampled yet, or an error of zero (nothing scored).
///
/// The run stops when the mean is within `target` and, when the limit asks for a peak, the
/// highest piece is within twice `target`. It also stops at `max_histories` or `max_seconds`
/// of sampling; the estimate names whichever the speed so far reaches first.
pub fn estimate_sampling(
    histories: u64,
    elapsed_seconds: f64,
    relative_error: Option<f64>,
    peak_relative_error: Option<f64>,
    target: f64,
    max_histories: u64,
    max_seconds: u64,
) -> Option<SamplingEstimate> {
    let error = relative_error?;
    let valid = |x: f64| x.is_finite() && x > 0.0;
    if histories == 0 || !valid(elapsed_seconds) || !valid(error) || !valid(target) {
        return None;
    }
    let mut ratio = error / target;
    if let Some(peak) = peak_relative_error.filter(|p| valid(*p)) {
        ratio = ratio.max(peak / (2.0 * target));
    }
    let done = histories as f64;
    let needed = if ratio > 1.0 {
        (done * ratio * ratio).ceil()
    } else {
        done
    };
    let histories_needed = needed.min(u64::MAX as f64) as u64;
    let histories_remaining = histories_needed.saturating_sub(histories);
    let rate = done / elapsed_seconds;
    let to_target = histories_remaining as f64 / rate;
    let to_history_limit = max_histories.saturating_sub(histories) as f64 / rate;
    let to_time_limit = (max_seconds as f64 - elapsed_seconds).max(0.0);
    let (seconds_remaining, stops_at) =
        if to_target <= to_history_limit && to_target <= to_time_limit {
            (to_target, StopsAt::Target)
        } else if to_history_limit <= to_time_limit {
            (to_history_limit, StopsAt::HistoryLimit)
        } else {
            (to_time_limit, StopsAt::TimeLimit)
        };
    Some(SamplingEstimate {
        histories_needed,
        histories_remaining,
        seconds_remaining,
        fraction: (elapsed_seconds / (elapsed_seconds + seconds_remaining)).clamp(0.0, 1.0),
        stops_at,
    })
}

/// Seconds as `42 s`, `7 min` or `2 h 05 min`, rounded the way a person reads a time left.
pub fn duration_text(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    if s < 90 {
        format!("{s} s")
    } else if s < 3600 {
        format!("{} min", (s + 30) / 60)
    } else {
        let minutes = (s + 30) / 60;
        format!("{} h {:02} min", minutes / 60, minutes % 60)
    }
}

/// Histories as `850`, `12.5 thousand`, `4.2 million`.
pub fn histories_text(histories: u64) -> String {
    let n = histories as f64;
    if histories < 10_000 {
        histories.to_string()
    } else if histories < 1_000_000 {
        format!("{:.1} thousand", n / 1.0e3)
    } else {
        format!("{:.1} million", n / 1.0e6)
    }
}

/// The estimate as a sentence that says it is one.
pub fn estimate_text(estimate: &SamplingEstimate) -> String {
    let left = duration_text(estimate.seconds_remaining);
    match estimate.stops_at {
        StopsAt::Target if estimate.histories_remaining == 0 => {
            "Estimate: the error is already within the target; the run stops at the next check."
                .to_string()
        }
        StopsAt::Target => format!(
            "Estimate: about {} more histories to reach the target, about {left} at this speed.",
            histories_text(estimate.histories_remaining)
        ),
        StopsAt::HistoryLimit => format!(
            "Estimate: the target needs about {} histories in all, more than the history limit. \
             The run will stop at the limit in about {left}, short of the target.",
            histories_text(estimate.histories_needed)
        ),
        StopsAt::TimeLimit => format!(
            "Estimate: at this speed the time limit stops the run in about {left}, before the \
             target ({} histories in all) is reached.",
            histories_text(estimate.histories_needed)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn estimate(
        histories: u64,
        elapsed: f64,
        error: Option<f64>,
        peak: Option<f64>,
    ) -> Option<SamplingEstimate> {
        estimate_sampling(histories, elapsed, error, peak, 0.05, 50_000_000, 3600)
    }

    #[test]
    fn error_falling_as_one_over_root_n_gives_the_histories_needed() {
        // 10 % at 1e6 histories: 5 % needs four times as many.
        let e = estimate(1_000_000, 100.0, Some(0.10), None).unwrap();
        assert_eq!(e.histories_needed, 4_000_000);
        assert_eq!(e.histories_remaining, 3_000_000);
        // 1e4 histories/s: 300 s to go; 100 s of 400 s done.
        assert!((e.seconds_remaining - 300.0).abs() < 1e-9);
        assert!((e.fraction - 0.25).abs() < 1e-9);
        assert_eq!(e.stops_at, StopsAt::Target);
    }

    #[test]
    fn the_peak_is_held_to_twice_the_target() {
        // Mean 5 % is on target; the peak at 20 % is twice too high, so four times the histories.
        let e = estimate(1_000_000, 100.0, Some(0.05), Some(0.20)).unwrap();
        assert_eq!(e.histories_needed, 4_000_000);
        // A peak inside twice the target adds nothing.
        let e = estimate(1_000_000, 100.0, Some(0.10), Some(0.05)).unwrap();
        assert_eq!(e.histories_needed, 4_000_000);
    }

    #[test]
    fn a_run_already_on_target_has_nothing_remaining() {
        let e = estimate(2_000_000, 50.0, Some(0.04), None).unwrap();
        assert_eq!(e.histories_remaining, 0);
        assert_eq!(e.seconds_remaining, 0.0);
        assert_eq!(e.fraction, 1.0);
        assert!(estimate_text(&e).contains("already within the target"));
    }

    #[test]
    fn the_history_limit_is_named_when_it_comes_first() {
        let e =
            estimate_sampling(1_000_000, 100.0, Some(0.10), None, 0.05, 2_000_000, 3600).unwrap();
        assert_eq!(e.stops_at, StopsAt::HistoryLimit);
        assert!((e.seconds_remaining - 100.0).abs() < 1e-9);
        assert!(estimate_text(&e).contains("short of the target"));
    }

    #[test]
    fn the_time_limit_is_named_when_it_comes_first() {
        let e =
            estimate_sampling(1_000_000, 100.0, Some(0.10), None, 0.05, 50_000_000, 150).unwrap();
        assert_eq!(e.stops_at, StopsAt::TimeLimit);
        assert!((e.seconds_remaining - 50.0).abs() < 1e-9);
        assert!(estimate_text(&e).contains("time limit"));
    }

    #[test]
    fn nothing_is_estimated_before_the_error_is_known() {
        assert!(estimate(1_000_000, 100.0, None, Some(0.1)).is_none());
        assert!(estimate(0, 100.0, Some(0.1), None).is_none());
        assert!(estimate(1_000, 0.0, Some(0.1), None).is_none());
        assert!(estimate(1_000, 10.0, Some(0.0), None).is_none());
        assert!(estimate(1_000, 10.0, Some(f64::NAN), None).is_none());
    }

    #[test]
    fn progress_lines_match_the_command_line_wording() {
        assert_eq!(
            progress_line(&TransportProgress::Step("mgxs".into())),
            "preparing weight windows: condensing the nuclear data"
        );
        assert_eq!(
            progress_line(&TransportProgress::Step("run".into())),
            "starting the run"
        );
        let sampling = |error, peak| TransportProgress::Sampling {
            histories: 40_000,
            relative_error: error,
            peak_relative_error: peak,
            elapsed_seconds: 12.4,
        };
        assert_eq!(
            progress_line(&sampling(Some(0.123), Some(0.31))),
            "40000 histories, 12 s: error 12.3 %, highest piece 31.0 %"
        );
        assert_eq!(
            progress_line(&sampling(Some(0.123), None)),
            "40000 histories, 12 s: error 12.3 %"
        );
        assert_eq!(
            progress_line(&sampling(None, None)),
            "40000 histories, 12 s"
        );
    }

    #[test]
    fn durations_and_counts_read_naturally() {
        assert_eq!(duration_text(42.0), "42 s");
        assert_eq!(duration_text(420.0), "7 min");
        assert_eq!(duration_text(7500.0), "2 h 05 min");
        assert_eq!(histories_text(850), "850");
        assert_eq!(histories_text(12_500), "12.5 thousand");
        assert_eq!(histories_text(4_200_000), "4.2 million");
    }
}
