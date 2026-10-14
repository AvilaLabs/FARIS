//! The words and numbers of a finished design flux run, built from the engine's report.
//!
//! No window and no physics: the report is the engine's, and this turns it into rows of text
//! for the panel to draw, so the wording can be tested. Every number carries its error or the
//! reason it has none; nothing is built before a report exists.

use faris_engine::design::{
    DesignTransportReport, FluenceLimit, LimitStatus, NotEvaluated, SolidFlux, span_text,
};

/// A quantity with its caption: the value, and its error or why there is none.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub caption: String,
    pub value: String,
    pub error: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TargetView {
    pub reached: bool,
    pub text: String,
    /// Why the target was not reached and what to do; empty when it was.
    pub why: Option<String>,
    pub next_step: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpeedUpView {
    pub mode: String,
    pub text: String,
    /// What to do to speed up the next run; `None` for an aimed run.
    pub next_step: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LimitView {
    pub title: String,
    pub status: LimitStatus,
    pub status_word: &'static str,
    /// The flux the limit was judged on.
    pub flux: Row,
    /// The fluence over the linked history, with its error.
    pub fluence: Row,
    /// Years to the limit at full power, with their error, or why there are none.
    pub years: Row,
    /// The same over the history's own capacity factor, when it is known.
    pub calendar_years: Option<Row>,
    pub why: String,
    pub next_step: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResultView {
    pub heading: String,
    pub mean: Row,
    pub highest_solid: Option<Row>,
    pub highest_piece: Option<Row>,
    /// Where the highest piece is and how big it is.
    pub highest_piece_place: Option<String>,
    pub target: TargetView,
    pub run_line: String,
    pub speed_up: SpeedUpView,
    pub limits: Vec<LimitView>,
    pub notices: Vec<String>,
    pub not_evaluated: Vec<NotEvaluated>,
    pub run_directory: String,
}

/// `± 2.1 %`, or why the error is unknown.
pub fn error_text(relative: Option<f64>) -> String {
    relative.map_or_else(
        || {
            "error unknown: fewer than two batches scored or nothing scored here; \
             run more histories"
                .to_string()
        },
        |r| format!("± {:.1} %", 100.0 * r),
    )
}

fn flux_text(flux: f64) -> String {
    format!("{flux:.3e} n/m²/s")
}

pub fn status_word(status: LimitStatus) -> &'static str {
    match status {
        LimitStatus::Within => "WITHIN",
        LimitStatus::Exceeded => "EXCEEDED",
        LimitStatus::Inconclusive => "INCONCLUSIVE",
    }
}

fn solid_row(caption: String, solid: &SolidFlux) -> Row {
    Row {
        caption,
        value: flux_text(solid.flux_n_m2_s),
        error: error_text(solid.relative_error),
    }
}

fn limit_view(limit: &FluenceLimit) -> LimitView {
    let a = &limit.assessment;
    let judged = match &limit.solid_id {
        Some(solid) => format!("highest piece, in {solid}"),
        None => "mean over the group".to_string(),
    };
    // Margins carry the two-standard-error band the verdict is judged on.
    let fluence_error = match (a.history_fluence_low_n_m2, a.history_fluence_high_n_m2) {
        (Some(low), Some(high)) => format!("2σ band {low:.2e} to {high:.2e} n/m²"),
        _ => error_text(None),
    };
    let years_error = match (
        a.full_power_years_to_limit_earliest,
        a.full_power_years_to_limit_latest,
    ) {
        (Some(early), Some(late)) => format!("2σ band {} to {}", span_text(early), span_text(late)),
        (Some(early), None) => format!("2σ band: {} at the earliest, no latest", span_text(early)),
        _ => error_text(a.years_relative_error),
    };
    let years = match a.full_power_years_to_limit {
        Some(years) => Row {
            caption: "Years to the limit at full power".into(),
            value: span_text(years),
            error: years_error.clone(),
        },
        None => Row {
            caption: "Years to the limit at full power".into(),
            value: "none".into(),
            error: "the flux is zero or not valid, so no time to the limit follows".into(),
        },
    };
    let calendar_years = a.calendar_years_to_limit.map(|years| Row {
        caption: format!(
            "Years to the limit over this history (capacity factor {:.2})",
            a.capacity_factor
        ),
        value: span_text(years),
        error: years_error,
    });
    LimitView {
        title: format!(
            "Fluence limit of {} ({:e} n/m²), {judged}",
            limit.group_id, a.limit_n_m2
        ),
        status: a.status,
        status_word: status_word(a.status),
        flux: Row {
            caption: "Flux judged".into(),
            value: flux_text(a.flux_n_m2_s),
            error: error_text(a.flux_relative_error),
        },
        fluence: Row {
            caption: format!(
                "Fluence over the {:.1}-year history ({:.1} years at full power)",
                a.history_calendar_years, a.history_full_power_years
            ),
            value: format!("{:.3e} n/m²", a.history_fluence_n_m2),
            error: fluence_error,
        },
        years,
        calendar_years,
        why: a.why.clone(),
        next_step: a.next_step.clone(),
    }
}

/// The view of a finished run.
pub fn result_view(report: &DesignTransportReport) -> ResultView {
    let what = if report.energy_min_ev > 0.0 {
        format!("Neutron flux above {} MeV", report.energy_min_ev / 1.0e6)
    } else {
        "Neutron flux at all energies".to_string()
    };
    let place = match &report.response_group_id {
        Some(group) => format!("the group {group}"),
        None => "the TF magnets".to_string(),
    };
    let heading = format!(
        "{what} in {place} of {}",
        report.design.id.as_deref().unwrap_or("the design")
    );
    let highest = report
        .solids
        .iter()
        .max_by(|a, b| a.flux_n_m2_s.total_cmp(&b.flux_n_m2_s));
    let piece = faris_engine::design::leading_peak(&report.solids);
    let has_peak = report.solids.iter().any(|s| s.peak.is_some());
    let target = 100.0 * report.target_relative_error;
    let on = if has_peak {
        format!("the mean and {} % on the highest piece", 2.0 * target)
    } else {
        "the mean".to_string()
    };
    let reduction = &report.variance_reduction;
    ResultView {
        heading,
        mean: Row {
            caption: format!("Mean over {} solids", report.solids.len()),
            value: flux_text(report.mean_flux_n_m2_s),
            error: error_text(report.mean_relative_error),
        },
        highest_solid: highest.map(|s| solid_row(format!("Highest solid: {}", s.solid_id), s)),
        highest_piece: piece.map(|(s, p)| Row {
            caption: format!("Highest piece, in {}", s.solid_id),
            value: flux_text(p.flux_n_m2_s),
            error: error_text(p.relative_error),
        }),
        highest_piece_place: piece.map(|(_, p)| {
            format!(
                "r {:.2}–{:.2} m, z {:.2}–{:.2} m, {:.1} litres, the highest of {} pieces",
                p.r_m[0],
                p.r_m[1],
                p.z_m[0],
                p.z_m[1],
                p.volume_m3 * 1.0e3,
                p.pieces_ranked
            )
        }),
        target: TargetView {
            reached: report.target_reached,
            text: if report.target_reached {
                format!("Target of {target} % on {on}: reached.")
            } else {
                format!("Target of {target} % on {on}: NOT reached.")
            },
            why: report.why.clone().filter(|_| !report.target_reached),
            next_step: report.next_step.clone().filter(|_| !report.target_reached),
        },
        run_line: format!(
            "{} histories in {:.0} s on {} thread(s); source {:.3e} n/s from {} MW of fusion power at {} MeV per reaction. {}",
            report.histories,
            report.elapsed_seconds,
            report.threads,
            report.source_rate_per_s,
            report.fusion_power_mw,
            report.energy_per_reaction_ev / 1.0e6,
            match report.lost_particles {
                Some(lost) =>
                    format!("Lost neutrons: {lost} (a run fails above one in a million)."),
                None => "Lost neutrons were not counted by this OpenMC build.".to_string(),
            }
        ),
        speed_up: SpeedUpView {
            mode: reduction.mode.clone(),
            text: format!(
                "{} ({:.0} s of preparation{}). {}",
                reduction.mode,
                report.setup_seconds,
                if report.windows_reused {
                    ", weight windows reused from an earlier run"
                } else {
                    ""
                },
                reduction.reason
            ),
            next_step: (reduction.mode != "aimed").then(|| reduction.next_step.clone()),
        },
        limits: report.fluence_limits.iter().map(limit_view).collect(),
        notices: report.notices.clone(),
        not_evaluated: report.not_evaluated.clone(),
        run_directory: report.run_directory.clone(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use faris_engine::design::{DesignIdentity, FluenceAssessment, PeakFlux, VarianceReduction};

    /// A hand-built report for tests of the wording; the numbers are arbitrary and say nothing
    /// about any design.
    pub fn sample_report() -> DesignTransportReport {
        let assessment = |status, flux_error: Option<f64>| FluenceAssessment {
            limit_n_m2: 3.0e22,
            flux_n_m2_s: 2.0e14,
            flux_relative_error: flux_error,
            history_fluence_n_m2: 1.5e22,
            history_fluence_standard_error_n_m2: flux_error.map(|e| 1.5e22 * e),
            history_fluence_low_n_m2: flux_error.map(|e| 1.5e22 * (1.0 - 2.0 * e)),
            history_fluence_high_n_m2: flux_error.map(|e| 1.5e22 * (1.0 + 2.0 * e)),
            history_calendar_years: 30.0,
            history_full_power_years: 24.0,
            capacity_factor: 0.8,
            full_power_years_to_limit: Some(47.5),
            calendar_years_to_limit: Some(59.4),
            full_power_years_to_limit_earliest: flux_error.map(|e| 47.5 / (1.0 + 2.0 * e)),
            full_power_years_to_limit_latest: flux_error.map(|e| 47.5 / (1.0 - 2.0 * e)),
            calendar_years_to_limit_earliest: flux_error.map(|e| 59.4 / (1.0 + 2.0 * e)),
            calendar_years_to_limit_latest: flux_error.map(|e| 59.4 / (1.0 - 2.0 * e)),
            years_relative_error: flux_error,
            status,
            why: "The fluence is below the limit.".into(),
            next_step: "Keep the margin in mind.".into(),
        };
        DesignTransportReport {
            schema_version: "faris-design-transport-report/v0.3".into(),
            design: DesignIdentity {
                path: "d.json".into(),
                sha256: "0".into(),
                id: Some("test-design".into()),
            },
            check_report_sha256: "0".into(),
            h5m_sha256: "0".into(),
            cross_sections_sha256: "0".into(),
            openmc: "0.15.3".into(),
            run_directory: "/runs/one".into(),
            energy_min_ev: 1.0e5,
            response_group_id: None,
            source_rate_per_s: 1.0e20,
            fusion_power_mw: 500.0,
            energy_per_reaction_ev: 17.6e6,
            lost_particles: Some(0),
            lost_positions_m: Vec::new(),
            histories: 3_000_000,
            elapsed_seconds: 640.0,
            seed: 1,
            threads: 4,
            target_relative_error: 0.05,
            target_reached: true,
            stop_reason: "target".into(),
            why: None,
            next_step: None,
            mean_flux_n_m2_s: 2.0e14,
            mean_relative_error: Some(0.043),
            total_volume_m3: 12.0,
            solids: vec![
                SolidFlux {
                    solid_id: "coil-a".into(),
                    volume_m3: 5.0,
                    flux_n_m2_s: 1.5e14,
                    relative_error: Some(0.06),
                    peak: None,
                },
                SolidFlux {
                    solid_id: "coil-b".into(),
                    volume_m3: 7.0,
                    flux_n_m2_s: 2.4e14,
                    relative_error: None,
                    peak: Some(PeakFlux {
                        r_m: [4.0, 4.2],
                        z_m: [0.5, 0.7],
                        volume_m3: 0.012,
                        flux_n_m2_s: 6.0e14,
                        relative_error: Some(0.09),
                        pieces_ranked: 40,
                    }),
                },
            ],
            variance_reduction: VarianceReduction {
                mode: "windows-only".into(),
                reason: "No streaming channel was found.".into(),
                next_step: "Build OpenMC with the aiming patch.".into(),
                targets: Vec::new(),
                windows_sha256: None,
                aim_seconds: 0.0,
                aim_reused: false,
            },
            setup_seconds: 95.0,
            windows_reused: false,
            fluence_limits: vec![FluenceLimit {
                group_id: "tf".into(),
                basis: "peak_piece".into(),
                solid_id: Some("coil-b".into()),
                assessment: assessment(LimitStatus::Within, Some(0.09)),
            }],
            notices: vec!["A notice.".into()],
            not_evaluated: vec![NotEvaluated {
                item: "Gamma heating".into(),
                why: "Not asked for.".into(),
                next_step: "Run the heating study.".into(),
            }],
        }
    }

    #[test]
    fn every_number_carries_its_error_or_the_reason_it_has_none() {
        let view = result_view(&sample_report());
        assert_eq!(view.mean.error, "± 4.3 %");
        assert_eq!(view.mean.value, "2.000e14 n/m²/s");
        let solid = view.highest_solid.unwrap();
        assert_eq!(solid.caption, "Highest solid: coil-b");
        assert!(
            solid
                .error
                .starts_with("error unknown: fewer than two batches")
        );
        assert_eq!(view.highest_piece.unwrap().error, "± 9.0 %");
        assert_eq!(
            view.highest_piece_place.unwrap(),
            "r 4.00–4.20 m, z 0.50–0.70 m, 12.0 litres, the highest of 40 pieces"
        );
    }

    #[test]
    fn a_limit_shows_status_fluence_years_and_the_why_and_next_text() {
        let view = result_view(&sample_report());
        let limit = &view.limits[0];
        assert_eq!(limit.status_word, "WITHIN");
        assert!(limit.title.contains("highest piece, in coil-b"));
        assert_eq!(limit.fluence.value, "1.500e22 n/m²");
        // Verifies: MAG-004
        assert_eq!(limit.fluence.error, "2σ band 1.23e22 to 1.77e22 n/m²");
        assert_eq!(limit.years.value, "47.5 years");
        assert_eq!(limit.years.error, "2σ band 40.3 years to 57.9 years");
        assert!(
            limit
                .calendar_years
                .as_ref()
                .unwrap()
                .caption
                .contains("0.80")
        );
        assert_eq!(limit.why, "The fluence is below the limit.");
        assert_eq!(limit.next_step, "Keep the margin in mind.");
    }

    #[test]
    fn a_limit_without_years_says_why() {
        let mut report = sample_report();
        let a = &mut report.fluence_limits[0].assessment;
        a.full_power_years_to_limit = None;
        a.calendar_years_to_limit = None;
        a.flux_relative_error = None;
        a.history_fluence_standard_error_n_m2 = None;
        a.history_fluence_low_n_m2 = None;
        a.history_fluence_high_n_m2 = None;
        a.full_power_years_to_limit_earliest = None;
        a.full_power_years_to_limit_latest = None;
        a.status = LimitStatus::Inconclusive;
        let view = result_view(&report);
        let limit = &view.limits[0];
        assert_eq!(limit.status_word, "INCONCLUSIVE");
        assert_eq!(limit.years.value, "none");
        assert!(limit.years.error.contains("zero or not valid"));
        assert!(limit.calendar_years.is_none());
        assert!(limit.fluence.error.starts_with("error unknown"));
    }

    #[test]
    fn a_run_that_missed_its_target_says_why_and_what_next() {
        let mut report = sample_report();
        report.target_reached = false;
        report.why = Some("The history limit came first.".into());
        report.next_step = Some("Raise the history limit.".into());
        let view = result_view(&report);
        assert!(!view.target.reached);
        assert!(view.target.text.ends_with("NOT reached."));
        assert!(view.target.text.contains("10 % on the highest piece"));
        assert_eq!(
            view.target.why.as_deref(),
            Some("The history limit came first.")
        );
        assert_eq!(
            view.target.next_step.as_deref(),
            Some("Raise the history limit.")
        );
        let reached = result_view(&sample_report());
        assert!(reached.target.text.ends_with("reached."));
        assert!(reached.target.why.is_none());
    }

    #[test]
    fn the_speed_up_mode_and_every_notice_and_gap_are_listed() {
        let view = result_view(&sample_report());
        assert!(
            view.speed_up
                .text
                .starts_with("windows-only (95 s of preparation).")
        );
        assert_eq!(
            view.speed_up.next_step.as_deref(),
            Some("Build OpenMC with the aiming patch.")
        );
        assert_eq!(view.notices, vec!["A notice.".to_string()]);
        assert_eq!(view.not_evaluated[0].item, "Gamma heating");
        let mut aimed = sample_report();
        aimed.variance_reduction.mode = "aimed".into();
        assert!(result_view(&aimed).speed_up.next_step.is_none());
    }

    #[test]
    fn a_design_without_a_peak_shows_no_highest_piece() {
        let mut report = sample_report();
        for solid in &mut report.solids {
            solid.peak = None;
        }
        let view = result_view(&report);
        assert!(view.highest_piece.is_none());
        assert!(view.highest_piece_place.is_none());
        assert!(view.target.text.contains("on the mean: reached"));
    }
}
