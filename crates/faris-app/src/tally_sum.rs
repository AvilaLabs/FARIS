//! The sum of a run's zone tritium tallies with its combined error, shown under the tallies
//! (UNC-013). The arithmetic is `faris_engine::tally_health`; this only draws its sentences.

use crate::badge::{self, Kind};
use eframe::egui;
use faris_engine::{
    tally_health::{ZoneSum, tritium_zone_sum_of},
    transport::NormalizedTransportResult,
};

/// The badge kind and the lines for a zone sum, or the reason there is none.
pub fn zone_sum_lines(sum: &Result<ZoneSum, String>) -> (Kind, Vec<String>) {
    match sum {
        Ok(z) => (
            if z.within_3_sigma && z.large_zones_within_2_percent {
                Kind::Calculated
            } else {
                Kind::Partial
            },
            vec![z.sum_line(), z.check_line()],
        ),
        Err(why) => (Kind::NotEvaluated, vec![why.clone()]),
    }
}

/// Draw the sum for one normalized transport result.
pub fn zone_sum_ui(ui: &mut egui::Ui, normalized: &NormalizedTransportResult) {
    let (kind, lines) = zone_sum_lines(&tritium_zone_sum_of(normalized));
    ui.horizontal_wrapped(|ui| {
        badge::badge(
            ui,
            kind,
            "zone sum",
            "The zone tritium tallies added with the covariance recorded between them, checked against the whole-model tally. Sampling error only; model and data uncertainty are separate.",
        );
        ui.label(&lines[0]);
    });
    for line in &lines[1..] {
        ui.small(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sum(z: f64, large: bool) -> ZoneSum {
        ZoneSum {
            zones: 4,
            sum: 1.3,
            standard_error: 0.003,
            total: 1.3,
            difference: z * 0.001,
            difference_standard_error: 0.001,
            z,
            within_3_sigma: z.abs() <= 3.0,
            large_zones_within_2_percent: large,
        }
    }

    // Verifies: UNC-013
    #[test]
    fn the_zone_sum_is_shown_with_its_error_and_the_check_against_the_total() {
        let (kind, lines) = zone_sum_lines(&Ok(sum(0.5, true)));
        assert_eq!(kind, Kind::Calculated);
        assert!(lines[0].starts_with("Sum of 4 zone tallies:"), "{lines:?}");
        assert!(lines[1].contains("within 3 σ"), "{lines:?}");
        let (kind, lines) = zone_sum_lines(&Ok(sum(5.0, true)));
        assert_eq!(kind, Kind::Partial);
        assert!(lines[1].contains("OUTSIDE"), "{lines:?}");
        let (kind, lines) = zone_sum_lines(&Ok(sum(0.5, false)));
        assert_eq!(kind, Kind::Partial);
        assert!(lines[1].contains("above 2 %"), "{lines:?}");
        let (kind, lines) =
            zone_sum_lines(&Err(faris_engine::tally_health::ZONE_SUM_UNAVAILABLE.into()));
        assert_eq!(kind, Kind::NotEvaluated);
        assert!(lines[0].starts_with("NOT_EVALUATED"));
    }
}
