//! Converts a full-power fast-neutron flux into years until a fluence limit,
//! using the power history linked to the design. Pure arithmetic: no I/O and
//! no solver. The flux is an average over the solid it was scored in, so the
//! result says nothing about the peak inside that solid.

use faris_model::history::OperatingHistoryAssumptions;
use serde::{Deserialize, Serialize};

/// Length of a year used for all conversions: 365.25 days.
pub const SECONDS_PER_YEAR: f64 = 365.25 * 86_400.0;

/// Width of the status band, in standard errors of the history fluence.
const BAND_SIGMAS: f64 = 2.0;

/// How the plant's history scales a full-power flux into fluence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PowerHistory {
    /// Seconds covered by the history's power periods.
    pub calendar_seconds: f64,
    /// Integral over the history of (fusion power in the period / design fusion power) dt, in seconds.
    pub full_power_seconds: f64,
}

/// Builds the power history from `operation` (periods of `start_s`, `end_s`
/// and `power_fraction`). The history gives power as a FRACTION of the design
/// fusion power, so it is used directly and `design_fusion_power_mw` is only
/// checked, not used. Planned outages (`planned_outages`) are times of zero
/// power: their overlap with a period is removed from the full-power seconds
/// but still counts as calendar time.
pub fn power_history(
    history: &OperatingHistoryAssumptions,
    design_fusion_power_mw: f64,
) -> Result<PowerHistory, String> {
    if !design_fusion_power_mw.is_finite() || design_fusion_power_mw <= 0.0 {
        return Err("The design fusion power is not a positive number.".into());
    }
    if history.operation.is_empty() {
        return Err("The operating history has no power periods.".into());
    }
    let mut outages: Vec<(f64, f64)> = Vec::new();
    for o in &history.planned_outages {
        if !o.start_s.is_finite() || !o.end_s.is_finite() || o.end_s < o.start_s {
            return Err("The operating history has an invalid planned outage.".into());
        }
        outages.push((o.start_s, o.end_s));
    }
    outages.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (mut calendar, mut full) = (0.0_f64, 0.0_f64);
    let mut previous_end = f64::NEG_INFINITY;
    for p in &history.operation {
        if !p.start_s.is_finite()
            || !p.end_s.is_finite()
            || !p.power_fraction.is_finite()
            || p.power_fraction < 0.0
            || p.end_s <= p.start_s
        {
            return Err(
                "The operating history has a power period that is not a valid positive-length, non-negative value."
                    .into(),
            );
        }
        if p.start_s < previous_end {
            return Err("The operating history has overlapping power periods.".into());
        }
        previous_end = p.end_s;
        let duration = p.end_s - p.start_s;
        let mut off = 0.0;
        let mut last_end = f64::NEG_INFINITY;
        for &(s, e) in &outages {
            // Merge overlapping outages so the same time is not removed twice.
            let s = s.max(last_end);
            if e <= s {
                continue;
            }
            last_end = e;
            off += (e.min(p.end_s) - s.max(p.start_s)).max(0.0);
        }
        calendar += duration;
        full += (duration - off.min(duration)) * p.power_fraction;
    }
    if calendar.is_nan() || calendar <= 0.0 || !calendar.is_finite() || !full.is_finite() {
        return Err("The operating history has no usable total duration.".into());
    }
    Ok(PowerHistory {
        calendar_seconds: calendar,
        full_power_seconds: full,
    })
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LimitStatus {
    Within,
    Exceeded,
    Inconclusive,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct FluenceAssessment {
    pub limit_n_m2: f64,
    pub flux_n_m2_s: f64,
    pub flux_relative_error: Option<f64>,
    /// Fluence over the whole history at this flux, and its standard error.
    pub history_fluence_n_m2: f64,
    pub history_fluence_standard_error_n_m2: Option<f64>,
    pub history_calendar_years: f64,
    pub history_full_power_years: f64,
    /// full_power_seconds / calendar_seconds
    pub capacity_factor: f64,
    /// limit / flux, in years at full power; None when the flux is zero.
    pub full_power_years_to_limit: Option<f64>,
    /// The same at the history's capacity factor; None when flux or capacity factor is zero.
    pub calendar_years_to_limit: Option<f64>,
    /// Relative error of both "years to limit" values (equals the flux's relative error to first order).
    pub years_relative_error: Option<f64>,
    pub status: LimitStatus,
    /// One plain sentence explaining the status.
    pub why: String,
    /// One plain sentence: what the user should do.
    pub next_step: String,
}

/// Assesses a full-power flux against a fluence limit over the history.
/// Never panics; invalid numbers give `Inconclusive`.
pub fn assess_fluence(
    flux_n_m2_s: f64,
    flux_relative_error: Option<f64>,
    limit_n_m2: f64,
    history: &PowerHistory,
) -> FluenceAssessment {
    let capacity_factor = if history.calendar_seconds > 0.0 {
        history.full_power_seconds / history.calendar_seconds
    } else {
        0.0
    };
    let valid = flux_n_m2_s.is_finite()
        && flux_n_m2_s >= 0.0
        && limit_n_m2.is_finite()
        && limit_n_m2 >= 0.0
        && flux_relative_error.is_none_or(|e| e.is_finite() && e >= 0.0);
    let fluence = flux_n_m2_s * history.full_power_seconds;
    let se = flux_relative_error.map(|e| fluence * e);
    let full_power_years =
        (valid && flux_n_m2_s > 0.0).then(|| limit_n_m2 / flux_n_m2_s / SECONDS_PER_YEAR);
    let calendar_years = match full_power_years {
        Some(y) if capacity_factor > 0.0 => Some(y / capacity_factor),
        _ => None,
    };
    let years_relative_error = if full_power_years.is_some() {
        flux_relative_error
    } else {
        None
    };

    let inconclusive_next = "Run again with a smaller target error.";
    let (status, why, next_step) = if !valid {
        (
            LimitStatus::Inconclusive,
            "The flux, its error or the limit is not a valid number.".to_string(),
            "Check the transport result and the limit in the design file.".to_string(),
        )
    } else if let Some(se) = se {
        let band = BAND_SIGMAS * se;
        if fluence - band > limit_n_m2 {
            (
                LimitStatus::Exceeded,
                "The fluence over the history is above the limit by more than two standard errors; this is an average over the solid the flux was scored in, not the peak inside it.".to_string(),
                "Shorten the service interval of this group, or add shielding.".to_string(),
            )
        } else if fluence + band < limit_n_m2 {
            (
                LimitStatus::Within,
                "The fluence over the history is below the limit by more than two standard errors; this is an average over the solid the flux was scored in, not the peak inside it.".to_string(),
                "No action is needed for this limit.".to_string(),
            )
        } else {
            (
                LimitStatus::Inconclusive,
                "The fluence over the history is within two standard errors of the limit."
                    .to_string(),
                inconclusive_next.to_string(),
            )
        }
    } else {
        (
            LimitStatus::Inconclusive,
            "The flux has no error estimate.".to_string(),
            inconclusive_next.to_string(),
        )
    };

    FluenceAssessment {
        limit_n_m2,
        flux_n_m2_s,
        flux_relative_error,
        history_fluence_n_m2: fluence,
        history_fluence_standard_error_n_m2: se,
        history_calendar_years: history.calendar_seconds / SECONDS_PER_YEAR,
        history_full_power_years: history.full_power_seconds / SECONDS_PER_YEAR,
        capacity_factor,
        full_power_years_to_limit: full_power_years,
        calendar_years_to_limit: calendar_years,
        years_relative_error,
        status,
        why,
        next_step,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faris_model::history::{PowerPeriod, TimeInterval};

    const RM_M: &str = include_str!("../../../../scenarios/rm-m/rm-m.operating-history.json");

    fn rm_m() -> OperatingHistoryAssumptions {
        serde_json::from_str(RM_M).unwrap()
    }

    fn unit() -> PowerHistory {
        PowerHistory {
            calendar_seconds: 1.0,
            full_power_seconds: 1.0,
        }
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * b.abs()
    }

    #[test]
    fn textbook_full_power_years() {
        let h = PowerHistory {
            calendar_seconds: 1e9,
            full_power_seconds: 1e9,
        };
        let a = assess_fluence(1e14, Some(0.01), 3e22, &h);
        // 3e22 / 1e14 = 3e8 s
        assert!(approx(
            a.full_power_years_to_limit.unwrap(),
            3e8 / SECONDS_PER_YEAR
        ));
        assert!((a.full_power_years_to_limit.unwrap() - 9.5066).abs() < 1e-3);
        assert_eq!(a.capacity_factor, 1.0);
        assert!(approx(
            a.calendar_years_to_limit.unwrap(),
            a.full_power_years_to_limit.unwrap()
        ));
        assert_eq!(a.years_relative_error, Some(0.01));
        assert!(approx(a.history_fluence_n_m2, 1e23));
        assert!(approx(a.history_fluence_standard_error_n_m2.unwrap(), 1e21));
    }

    #[test]
    fn half_capacity_factor_doubles_calendar_years() {
        let h = PowerHistory {
            calendar_seconds: 2e8,
            full_power_seconds: 1e8,
        };
        let a = assess_fluence(1e14, Some(0.01), 3e22, &h);
        assert_eq!(a.capacity_factor, 0.5);
        assert!(approx(
            a.calendar_years_to_limit.unwrap(),
            2.0 * a.full_power_years_to_limit.unwrap()
        ));
        assert!(approx(a.history_calendar_years, 2e8 / SECONDS_PER_YEAR));
        assert!(approx(a.history_full_power_years, 1e8 / SECONDS_PER_YEAR));
    }

    #[test]
    fn status_branches_and_exact_band_edges() {
        // fluence 8, standard error 2, band 4: limits 4 and 12 are the edges.
        let s = |limit| assess_fluence(8.0, Some(0.25), limit, &unit()).status;
        assert_eq!(s(3.999), LimitStatus::Exceeded);
        assert_eq!(s(4.0), LimitStatus::Inconclusive);
        assert_eq!(s(8.0), LimitStatus::Inconclusive);
        assert_eq!(s(12.0), LimitStatus::Inconclusive);
        assert_eq!(s(12.001), LimitStatus::Within);
        let ex = assess_fluence(8.0, Some(0.25), 1.0, &unit());
        assert!(ex.why.contains("not the peak"));
        assert_eq!(
            ex.next_step,
            "Shorten the service interval of this group, or add shielding."
        );
        let wi = assess_fluence(8.0, Some(0.25), 100.0, &unit());
        assert!(wi.why.contains("not the peak"));
        assert_eq!(wi.next_step, "No action is needed for this limit.");
        let inc = assess_fluence(8.0, Some(0.25), 8.0, &unit());
        assert_eq!(
            inc.why,
            "The fluence over the history is within two standard errors of the limit."
        );
        assert_eq!(inc.next_step, "Run again with a smaller target error.");
    }

    #[test]
    fn zero_flux_is_within_with_no_years() {
        let a = assess_fluence(0.0, Some(0.1), 1e22, &unit());
        assert_eq!(a.status, LimitStatus::Within);
        assert_eq!(a.full_power_years_to_limit, None);
        assert_eq!(a.calendar_years_to_limit, None);
        assert_eq!(a.years_relative_error, None);
    }

    #[test]
    fn missing_error_is_inconclusive() {
        let a = assess_fluence(1e20, None, 1.0, &unit());
        assert_eq!(a.status, LimitStatus::Inconclusive);
        assert!(a.why.contains("no error estimate"));
        assert!(a.full_power_years_to_limit.is_some());
        assert_eq!(a.history_fluence_standard_error_n_m2, None);
    }

    #[test]
    fn invalid_numbers_are_inconclusive_without_panic() {
        for (f, e, l) in [
            (f64::NAN, Some(0.1), 1.0),
            (-1.0, Some(0.1), 1.0),
            (1.0, Some(0.1), f64::INFINITY),
            (1.0, Some(0.1), -1.0),
            (f64::INFINITY, Some(0.1), 1.0),
            (1.0, Some(f64::NAN), 1.0),
            (1.0, Some(-0.1), 1.0),
        ] {
            let a = assess_fluence(f, e, l, &unit());
            assert_eq!(a.status, LimitStatus::Inconclusive);
            assert!(a.why.contains("not a valid number"));
            assert_eq!(a.full_power_years_to_limit, None);
        }
    }

    #[test]
    fn zero_capacity_factor_has_no_calendar_years() {
        let h = PowerHistory {
            calendar_seconds: 1e8,
            full_power_seconds: 0.0,
        };
        let a = assess_fluence(1e14, Some(0.1), 3e22, &h);
        assert!(a.full_power_years_to_limit.is_some());
        assert_eq!(a.calendar_years_to_limit, None);
    }

    #[test]
    fn status_serializes_screaming_snake_case() {
        assert_eq!(
            serde_json::to_string(&LimitStatus::Within).unwrap(),
            "\"WITHIN\""
        );
        assert_eq!(
            serde_json::to_string(&LimitStatus::Inconclusive).unwrap(),
            "\"INCONCLUSIVE\""
        );
    }

    fn period(start_s: f64, end_s: f64, power_fraction: f64) -> PowerPeriod {
        PowerPeriod {
            start_s,
            end_s,
            power_fraction,
        }
    }

    #[test]
    fn hand_built_history() {
        let mut h = rm_m();
        h.planned_outages = vec![TimeInterval {
            start_s: 50.0,
            end_s: 150.0,
            reason: "test".into(),
        }];
        h.operation = vec![period(0.0, 100.0, 1.0), period(100.0, 300.0, 0.5)];
        let p = power_history(&h, 500.0).unwrap();
        assert_eq!(p.calendar_seconds, 300.0);
        // 50 s at 1.0 before the outage, 150 s at 0.5 after it ends at 150: 50 + 75
        assert!(approx(p.full_power_seconds, 125.0));
    }

    #[test]
    fn bad_histories_are_errors() {
        let mut h = rm_m();
        assert!(power_history(&h, 0.0).is_err());
        let ok = h.operation.clone();
        h.operation = vec![];
        assert!(power_history(&h, 500.0).is_err());
        h.operation = vec![period(5.0, 5.0, 1.0)];
        assert!(power_history(&h, 500.0).is_err());
        for bad in [-0.1, f64::NAN, f64::INFINITY] {
            h.operation = vec![period(0.0, 5.0, bad)];
            assert!(power_history(&h, 500.0).is_err());
        }
        h.operation = vec![period(0.0, 5.0, 1.0), period(4.0, 8.0, 1.0)];
        assert!(power_history(&h, 500.0).is_err());
        h.operation = ok;
        assert!(power_history(&h, 500.0).is_ok());
    }

    #[test]
    fn rm_m_history_capacity_factor() {
        let h = rm_m();
        h.validate().unwrap();
        let p = power_history(&h, 500.0).unwrap();
        let cf = p.full_power_seconds / p.calendar_seconds;
        assert!(cf > 0.0 && cf <= 1.0, "{cf}");
        assert!(p.calendar_seconds > 0.0);
    }
}
