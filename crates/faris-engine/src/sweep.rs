//! Allocation-sweep arithmetic: per-point transport and history summaries, the
//! resolution test between sampled values, and the plain-language findings.
//!
//! Everything here is pure so the desktop only draws what this module computes.
//! Transport statements are solver results with Monte Carlo sampling error only;
//! history statements are conditional on the selected authored assumptions.

use crate::{
    history::{
        EventKind, HistoryResult, JULIAN_YEAR_SECONDS, TransportDrivingRates,
        run_operating_history_cancellable,
    },
    jobs::Cancellation,
    reactor::ReactorRun,
};
use faris_model::{
    LoadedScenario, Variant,
    history::{ComponentClass, OperatingHistoryAssumptions},
};
use std::collections::BTreeMap;

const MWH_PER_TWH: f64 = 1.0e6;

/// A sampled value with its one-sigma Monte Carlo standard error.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Estimate {
    pub mean: f64,
    pub standard_error: f64,
}

impl Estimate {
    /// Two-sigma interval `(low, high)`; sampling error only.
    pub fn interval_2sigma(&self) -> (f64, f64) {
        (
            self.mean - 2.0 * self.standard_error,
            self.mean + 2.0 * self.standard_error,
        )
    }
}

/// True when the difference of two independent estimates exceeds two combined
/// standard errors: |Δ| > 2·sqrt(SE₁² + SE₂²).
pub fn difference_resolved(a: &Estimate, b: &Estimate) -> bool {
    let combined = (a.standard_error.powi(2) + b.standard_error.powi(2)).sqrt();
    (a.mean - b.mean).abs() > 2.0 * combined
}

/// One sweep allocation's transport results (independent run per point).
#[derive(Clone, Debug, PartialEq)]
pub struct TransportPoint {
    pub variant_id: String,
    pub label: String,
    pub blanket_m: f64,
    pub shield_m: f64,
    /// Total H3 produced anywhere in the model per source neutron.
    pub breeding: Estimate,
    /// Magnet-region mean energy-integrated neutron flux, n/m²/s at reference power.
    pub magnet_flux: Estimate,
    pub seed: u64,
    pub histories: u64,
}

fn layer_thickness(variant: &Variant, id: &str) -> Result<f64, String> {
    variant
        .layers
        .iter()
        .find(|l| l.id == id)
        .map(|l| l.thickness_m)
        .ok_or_else(|| format!("variant {} has no {id} layer", variant.id))
}

/// Extract the sweep quantities from a recorded run and its scenario variant.
pub fn transport_point(run: &ReactorRun, variant: &Variant) -> Result<TransportPoint, String> {
    let normalized = run
        .normalized
        .as_ref()
        .ok_or_else(|| format!("{} has no accepted transport result", run.variant_id))?;
    if run.variant_id != variant.id {
        return Err(format!(
            "run {} does not belong to variant {}",
            run.variant_id, variant.id
        ));
    }
    let find = |id: &str| {
        normalized
            .results
            .iter()
            .find(|r| r.response_id == id)
            .ok_or_else(|| format!("{} is missing response {id}", run.variant_id))
    };
    let tritium = find("total-tritium-production")?;
    let flux = find("magnets-flux")?;
    let rate = normalized.source_neutron_rate_per_s;
    if !(rate.is_finite() && rate > 0.0) {
        return Err("source neutron rate must be positive".into());
    }
    Ok(TransportPoint {
        variant_id: variant.id.clone(),
        label: variant.label.clone(),
        blanket_m: layer_thickness(variant, "blanket")?,
        shield_m: layer_thickness(variant, "shield")?,
        breeding: Estimate {
            mean: tritium.integrated_mean / rate,
            standard_error: tritium.integrated_standard_error / rate,
        },
        magnet_flux: Estimate {
            mean: flux.mean,
            standard_error: flux.standard_error,
        },
        seed: run.sampling.seed,
        histories: u64::from(run.sampling.batches) * u64::from(run.sampling.particles_per_batch),
    })
}

/// Values read from one calculated operating history.
#[derive(Clone, Debug, PartialEq)]
pub struct HistorySummary {
    /// False when the assumptions class the magnets as permanent (no swaps modelled).
    pub magnets_replaceable: bool,
    pub magnet_replacements: u32,
    pub first_magnet_replacement_years: Option<f64>,
    /// First time a permanent magnet reaches its authored service limit.
    pub magnet_permanent_limit_years: Option<f64>,
    /// None when the history has no energy result.
    pub net_electricity_twh: Option<f64>,
    pub final_available_tritium_kg: f64,
}

pub fn summarize_history(history: &HistoryResult, component_id: &str) -> HistorySummary {
    let starts: Vec<f64> = history
        .events
        .iter()
        .filter(|e| {
            e.kind == EventKind::ReplacementStarted
                && e.component_id.as_deref() == Some(component_id)
        })
        .map(|e| e.time_s)
        .collect();
    let permanent = history
        .assumptions
        .service_limits
        .iter()
        .any(|l| l.component_id == component_id && l.class == ComponentClass::Permanent);
    let limit_years = permanent
        .then(|| {
            history
                .events
                .iter()
                .filter(|e| {
                    e.kind == EventKind::ServiceLimitReached
                        && e.component_id.as_deref() == Some(component_id)
                })
                .map(|e| e.time_s)
                .reduce(f64::min)
        })
        .flatten()
        .map(|t| t / JULIAN_YEAR_SECONDS);
    let last = history.snapshots.last();
    HistorySummary {
        magnets_replaceable: !permanent,
        magnet_permanent_limit_years: limit_years,
        magnet_replacements: starts.len() as u32,
        first_magnet_replacement_years: starts
            .iter()
            .copied()
            .reduce(f64::min)
            .map(|t| t / JULIAN_YEAR_SECONDS),
        net_electricity_twh: last
            .and_then(|s| s.cumulative_net_electricity_mwh)
            .map(|mwh| mwh / MWH_PER_TWH),
        final_available_tritium_kg: last.map_or(0.0, |s| s.available_tritium_kg),
    }
}

fn sorted(points: &[TransportPoint]) -> Vec<&TransportPoint> {
    let mut sorted: Vec<_> = points.iter().collect();
    sorted.sort_by(|a, b| a.blanket_m.total_cmp(&b.blanket_m));
    sorted
}

/// Transport statements (calculated, sampling error only).
pub fn transport_findings(points: &[TransportPoint]) -> Vec<String> {
    let points = sorted(points);
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return Vec::new();
    };
    if points.len() < 2 {
        return Vec::new();
    }
    let mut findings = Vec::new();
    let increments: Vec<f64> = points
        .windows(2)
        .map(|w| w[1].breeding.mean - w[0].breeding.mean)
        .collect();
    let direction = if last.breeding.mean >= first.breeding.mean {
        "rises"
    } else {
        "falls"
    };
    let resolved = difference_resolved(&first.breeding, &last.breeding);
    let mut text = format!(
        "Breeding {direction} from {:.3} to {:.3} across {:.2}\u{2013}{:.2} m of blanket",
        first.breeding.mean, last.breeding.mean, first.blanket_m, last.blanket_m
    );
    if !resolved {
        text.push_str(", a change not resolved at 2\u{3c3}");
    }
    text.push_str("; ");
    let half = increments.len() / 2;
    let early: f64 = increments[..half.max(1)].iter().sum();
    let late: f64 = increments[increments.len() - half.max(1)..].iter().sum();
    let diminishing = increments.windows(2).all(|w| w[1] <= w[0]);
    text.push_str(if increments.iter().all(|d| *d > 0.0) && diminishing {
        "each further step adds less than the previous one (saturating)."
    } else if early.abs() > late.abs() {
        "most of the change comes from the thinner-blanket steps; later steps add less."
    } else {
        "the steps do not show a clear saturation."
    });
    findings.push(text);
    let flux_resolved = difference_resolved(&first.magnet_flux, &last.magnet_flux);
    findings.push(if flux_resolved {
        format!(
            "Magnet flux differs between {:.2} m and {:.2} m of blanket beyond 2\u{3c3}: {:.2e} to {:.2e} n/m\u{b2}/s.",
            first.blanket_m, last.blanket_m, first.magnet_flux.mean, last.magnet_flux.mean
        )
    } else {
        "Magnet flux differences across the sweep are not resolved at 2\u{3c3} (port streaming dominates)."
            .into()
    });
    findings
}

/// History statements (conditional on the selected authored assumptions).
/// `summaries` is aligned with `points`.
pub fn history_findings(
    points: &[TransportPoint],
    summaries: &[Option<HistorySummary>],
) -> Vec<String> {
    let rows: Vec<(&TransportPoint, &HistorySummary)> = points
        .iter()
        .zip(summaries)
        .filter_map(|(p, s)| s.as_ref().map(|s| (p, s)))
        .collect();
    if rows.len() < 2 {
        return Vec::new();
    }
    let mut findings = Vec::new();
    if rows.iter().any(|(_, s)| !s.magnets_replaceable) {
        let years: Vec<f64> = rows
            .iter()
            .filter_map(|(_, s)| s.magnet_permanent_limit_years)
            .collect();
        let lo = years.iter().copied().reduce(f64::min);
        let hi = years.iter().copied().reduce(f64::max);
        findings.push(match (lo, hi) {
            (Some(lo), Some(hi)) => format!(
                "The magnets are permanent under these assumptions (no replacement is modelled); they reach the authored service limit after {lo:.1} to {hi:.1} y depending on the allocation."
            ),
            _ => "The magnets are permanent under these assumptions (no replacement is modelled) and do not reach the authored service limit within the horizon at any allocation.".into(),
        });
    } else {
        let min = rows
            .iter()
            .map(|(_, s)| s.magnet_replacements)
            .min()
            .unwrap();
        let max = rows
            .iter()
            .map(|(_, s)| s.magnet_replacements)
            .max()
            .unwrap();
        findings.push(if min == max {
            format!("Magnet replacements are {min} at every allocation in the horizon.")
        } else {
            let flux_resolved = rows.iter().any(|(a, _)| {
                rows.iter()
                    .any(|(b, _)| difference_resolved(&a.magnet_flux, &b.magnet_flux))
            });
            format!(
                "Magnet replacements range from {min} to {max} across the sweep{}",
                if flux_resolved {
                    "."
                } else {
                    "; the flux differences driving them are not resolved at 2\u{3c3}, so the ordering is not a design signal."
                }
            )
        });
    }
    let energy: Vec<_> = rows
        .iter()
        .filter_map(|(p, s)| s.net_electricity_twh.map(|e| (*p, e)))
        .collect();
    if let (Some(low), Some(high)) = (
        energy.iter().min_by(|a, b| a.1.total_cmp(&b.1)),
        energy.iter().max_by(|a, b| a.1.total_cmp(&b.1)),
    ) {
        findings.push(format!(
            "Lifetime net electricity ranges from {:.2} TWh ({:.2} m blanket) to {:.2} TWh ({:.2} m blanket).",
            low.1, low.0.blanket_m, high.1, high.0.blanket_m
        ));
    }
    findings
}

/// The component whose service life the sweep summaries follow.
pub const MAGNET_COMPONENT: &str = "magnets";

/// Extract every completed point (ascending blanket thickness) and its
/// history driving rates from the loaded sweep records.
pub fn collect_points(
    records: &BTreeMap<String, ReactorRun>,
    scenario: &LoadedScenario,
    fusion_power_mw: f64,
) -> Result<(Vec<TransportPoint>, Vec<TransportDrivingRates>), String> {
    let mut rows = Vec::new();
    for variant in &scenario.scenario.variants {
        let Some(run) = records.get(&variant.id) else {
            continue;
        };
        let Some(normalized) = &run.normalized else {
            continue;
        };
        let point = transport_point(run, variant)?;
        let raw = run
            .raw_artifact_sha256
            .as_deref()
            .ok_or("A sweep record has no raw identity.")?;
        let rates = TransportDrivingRates::from_normalized(normalized, fusion_power_mw, raw)?;
        rows.push((point, rates));
    }
    if rows.is_empty() {
        return Err("no completed transport records in the sweep bundles".into());
    }
    rows.sort_by(|a, b| a.0.blanket_m.total_cmp(&b.0.blanket_m));
    Ok(rows.into_iter().unzip())
}

/// One operating history per sweep point, summarised for the magnets.
pub fn summarize_sweep(
    rates: &[TransportDrivingRates],
    assumptions: &OperatingHistoryAssumptions,
    cancellation: &Cancellation,
) -> Result<Vec<HistorySummary>, String> {
    rates
        .iter()
        .map(|rates| {
            run_operating_history_cancellable(assumptions, rates, cancellation)
                .map(|history| summarize_history(&history, MAGNET_COMPONENT))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn estimate(mean: f64, se: f64) -> Estimate {
        Estimate {
            mean,
            standard_error: se,
        }
    }

    fn point(blanket: f64, tbr: f64, flux: f64, flux_se: f64) -> TransportPoint {
        TransportPoint {
            variant_id: format!("b{blanket}"),
            label: String::new(),
            blanket_m: blanket,
            shield_m: 0.9 - blanket,
            breeding: estimate(tbr, 0.001),
            magnet_flux: estimate(flux, flux_se),
            seed: 1,
            histories: 1_000_000,
        }
    }

    #[test]
    fn interval_is_two_standard_errors_each_side() {
        assert_eq!(estimate(10.0, 1.5).interval_2sigma(), (7.0, 13.0));
    }

    // Verifies: NUC-046, UNC-084, DSN-083
    #[test]
    fn resolution_uses_combined_standard_error() {
        // Combined SE = 5; threshold 10.
        assert!(!difference_resolved(
            &estimate(0.0, 3.0),
            &estimate(10.0, 4.0)
        ));
        assert!(difference_resolved(
            &estimate(0.0, 3.0),
            &estimate(10.1, 4.0)
        ));
        assert!(!difference_resolved(
            &estimate(5.0, 0.0),
            &estimate(5.0, 0.0)
        ));
    }

    // Verifies: DSN-083
    #[test]
    fn saturating_breeding_is_described_from_the_increments() {
        let points = [
            point(0.30, 1.20, 1.0e14, 2.0e13),
            point(0.35, 1.26, 1.1e14, 2.0e13),
            point(0.40, 1.29, 1.2e14, 2.0e13),
            point(0.45, 1.30, 1.0e14, 2.0e13),
        ];
        let findings = transport_findings(&points);
        assert!(findings[0].contains("rises from 1.200 to 1.300"));
        assert!(findings[0].contains("0.30\u{2013}0.45"));
        assert!(findings[0].contains("saturating"));
        assert!(findings[1].contains("not resolved"));
    }

    #[test]
    fn unsorted_input_and_nonmonotone_breeding_are_not_called_saturating() {
        let points = [
            point(0.45, 1.30, 1.0e14, 1.0e12),
            point(0.30, 1.20, 5.0e14, 1.0e12),
            point(0.35, 1.30, 1.0e14, 1.0e12),
            point(0.40, 1.25, 1.0e14, 1.0e12),
        ];
        let findings = transport_findings(&points);
        assert!(findings[0].contains("rises from 1.200 to 1.300"));
        assert!(!findings[0].contains("saturating"));
        assert!(findings[1].contains("beyond 2"));
    }

    #[test]
    fn permanent_magnets_report_the_limit_year_not_replacements() {
        use crate::history::run_operating_history;
        let assumptions: faris_model::history::OperatingHistoryAssumptions =
            serde_json::from_slice(include_bytes!(
                "../../../scenarios/arc-inspired/demo-event-assumptions.json"
            ))
            .unwrap();
        let run = run_operating_history(&assumptions, &test_rates(&assumptions)).unwrap();
        let summary = summarize_history(&run, "magnets");
        assert!(!summary.magnets_replaceable);
        assert_eq!(summary.magnet_replacements, 0);
        let year = summary.magnet_permanent_limit_years.expect("limit reached");
        assert!(year > 0.0 && year < 30.0, "{year}");
        let points = [
            point(0.3, 1.2, 1.0e14, 2.0e13),
            point(0.4, 1.3, 1.2e14, 2.0e13),
        ];
        let findings = history_findings(&points, &[Some(summary.clone()), Some(summary)]);
        assert!(findings[0].contains("permanent"), "{}", findings[0]);
    }

    #[test]
    fn single_point_makes_no_trend_claim() {
        assert!(transport_findings(&[point(0.3, 1.2, 1.0, 0.1)]).is_empty());
        assert!(transport_findings(&[]).is_empty());
    }

    // Verifies: DSN-083
    #[test]
    fn history_findings_flag_unresolved_flux_behind_replacement_differences() {
        let points = [
            point(0.3, 1.2, 1.0e14, 2.0e13),
            point(0.4, 1.3, 1.2e14, 2.0e13),
        ];
        let summary = |n, twh| {
            Some(HistorySummary {
                magnets_replaceable: true,
                magnet_permanent_limit_years: None,
                magnet_replacements: n,
                first_magnet_replacement_years: Some(3.0),
                net_electricity_twh: twh,
                final_available_tritium_kg: 1.0,
            })
        };
        let findings = history_findings(&points, &[summary(2, Some(30.0)), summary(3, Some(28.5))]);
        assert!(findings[0].contains("range from 2 to 3"));
        assert!(findings[0].contains("not resolved"));
        assert!(findings[1].contains("30.00 TWh (0.30 m"));
        assert!(findings[1].contains("28.50 TWh (0.40 m"));
        let same = history_findings(&points, &[summary(0, None), summary(0, None)]);
        assert!(same[0].contains("are 0 at every"));
        assert_eq!(same.len(), 1);
        assert!(history_findings(&points, &[summary(0, None), None]).is_empty());
    }

    #[test]
    fn history_summary_counts_only_magnet_replacements() {
        use crate::history::HistoryEvent;
        use faris_model::history::OperatingHistoryAssumptions;
        let mut assumptions: OperatingHistoryAssumptions = serde_json::from_slice(include_bytes!(
            "../../../scenarios/arc-inspired/demo-event-assumptions.json"
        ))
        .unwrap();
        // The shipped preset makes magnets permanent; make them swappable here.
        for limit in &mut assumptions.service_limits {
            if limit.component_id == "magnets" {
                limit.class = ComponentClass::Replaceable;
                limit.replacement_duration_s = Some(5_184_000.0);
            }
        }
        // Count directly from the events of a real history to cross-check.
        let run = test_history(&assumptions);
        let summary = summarize_history(&run, "magnets");
        let expected = run
            .events
            .iter()
            .filter(|e: &&HistoryEvent| {
                e.kind == EventKind::ReplacementStarted
                    && e.component_id.as_deref() == Some("magnets")
            })
            .count() as u32;
        assert!(expected > 0, "the event preset must replace magnets");
        assert_eq!(summary.magnet_replacements, expected);
        assert!(summary.magnets_replaceable && summary.magnet_permanent_limit_years.is_none());
        let none = summarize_history(&run, "no-such-component");
        assert_eq!(none.magnet_replacements, 0);
        assert!(none.first_magnet_replacement_years.is_none());
    }

    fn test_history(
        assumptions: &faris_model::history::OperatingHistoryAssumptions,
    ) -> HistoryResult {
        crate::history::run_operating_history(assumptions, &test_rates(assumptions))
            .expect("history")
    }

    fn test_rates(
        assumptions: &faris_model::history::OperatingHistoryAssumptions,
    ) -> crate::history::TransportDrivingRates {
        use crate::history::{ScalarRate, TransportDrivingRates};
        let rate = |mean: f64, id: &str| ScalarRate {
            mean,
            standard_error: Some(0.0),
            unit: "neutrons/m\u{b2}/s".into(),
            response_id: id.into(),
        };
        let mut flux = std::collections::BTreeMap::new();
        for limit in &assumptions.service_limits {
            flux.insert(limit.component_id.clone(), rate(1.0e15, &limit.response_id));
        }
        TransportDrivingRates {
            reference_fusion_power_mw: 500.0,
            fusion_reaction_rate_per_s: 500.0e6 / (17.6e6 * 1.602_176_634e-19),
            neutron_source_rate_per_s: 500.0e6 / (17.6e6 * 1.602_176_634e-19),
            total_reaction_energy_ev: 17.6e6,
            primary_neutron_energy_ev: 14.06e6,
            breeder_h3_per_source_neutron: ScalarRate {
                mean: 1.2,
                standard_error: Some(0.001),
                unit: "particles/source_neutron".into(),
                response_id: "blanket-tritium".into(),
            },
            component_average_flux_n_m2_s: flux,
            region_flux_n_m2_s: Default::default(),
            transport_deposited_heat_w: None,
            scenario_sha256: "0".repeat(64),
            transport_artifact_sha256: "0".repeat(64),
            solver_digest: format!("sha256:{}", "1".repeat(64)),
            nuclear_data_digest: format!("sha256:{}", "2".repeat(64)),
            covariance: None,
        }
    }
}
