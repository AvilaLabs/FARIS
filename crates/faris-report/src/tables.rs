//! CSV tables of the exported data. Values are written at full precision
//! (shortest round-trip representation); units are in the headers; a kind
//! column accompanies every value the app shows with a status label.

use crate::{ArrangementData, uncertainty::EnsembleInput};
use faris_engine::{
    brief::{
        AssumptionRow, BLANKET_PLUS_SHIELD_M, BREEDER_BLANKET_M, Caveat, Contrast,
        REFERENCE_BLANKET_M, StatusKind, StudyComparison,
    },
    comparison::{OperatingState, classify_operating_state},
    history::{JULIAN_YEAR_SECONDS, limit_exposure_n_m2},
    history_ensemble::{EnsembleStatus, HistoryEnsemble, SampleOutcome, nominal_sample},
    history_uncertainty::{SCOPE_LINE, outcome_category},
    sweep::{HistorySummary, TransportPoint, difference_resolved},
};
use std::collections::BTreeSet;

const MWH_PER_TWH: f64 = 1.0e6;

/// RFC 4180 quoting where needed.
pub fn field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

pub fn number(value: f64) -> String {
    if value.is_finite() {
        format!("{value}")
    } else {
        String::new()
    }
}

pub fn optional(value: Option<f64>) -> String {
    value.map_or_else(String::new, number)
}

fn row(cells: &[String]) -> String {
    let mut line = cells.iter().map(|c| field(c)).collect::<Vec<_>>().join(",");
    line.push('\n');
    line
}

fn state_name(state: OperatingState) -> &'static str {
    match state {
        OperatingState::Operating => "operating",
        OperatingState::MagnetReplacement => "magnet_replacement",
        OperatingState::OtherReplacement => "other_replacement",
        OperatingState::PlannedOutage => "planned_outage",
        OperatingState::FuelLimited => "fuel_limited",
        OperatingState::Stopped => "stopped",
    }
}

pub fn histories_csv(arrangements: &[&ArrangementData]) -> String {
    let mut out = row(&[
        "arrangement".into(),
        "calendar_year".into(),
        "state".into(),
        "magnet_fluence_n_m2".into(),
        "magnet_limit_fluence_n_m2".into(),
        "blanket_fluence_n_m2".into(),
        "usable_tritium_kg".into(),
        "net_electricity_twh".into(),
        "magnet_swaps".into(),
    ]);
    for data in arrangements {
        let Some(history) = &data.input.history else {
            continue;
        };
        let horizon_s = history.assumptions.horizon_s;
        for s in &history.snapshots {
            let state = classify_operating_state(
                &history.events,
                &history.assumptions.planned_outages,
                s.operating,
                s.time_s,
                horizon_s,
            );
            out.push_str(&row(&[
                data.input.arrangement.id().into(),
                number(s.time_s / JULIAN_YEAR_SECONDS),
                state_name(state).into(),
                optional(s.component_fluence_n_m2.get("magnets").copied()),
                optional(limit_exposure_n_m2(&history.assumptions, "magnets", s)),
                optional(s.component_fluence_n_m2.get("blanket").copied()),
                number(s.available_tritium_kg),
                optional(s.cumulative_net_electricity_mwh.map(|v| v / MWH_PER_TWH)),
                s.component_replacements
                    .get("magnets")
                    .map_or_else(String::new, |n| n.to_string()),
            ]));
        }
    }
    out
}

fn kind_or(value_present: bool, kind: StatusKind) -> String {
    if value_present {
        kind.label().into()
    } else {
        StatusKind::NotEvaluated.label().into()
    }
}

pub fn comparison_csv(arrangements: &[&ArrangementData]) -> String {
    let mut out = row(&[
        "arrangement".into(),
        "port".into(),
        "allocation".into(),
        "blanket_m".into(),
        "shield_m".into(),
        "transport_recorded".into(),
        "breeding_ratio_h3_per_source_neutron".into(),
        "breeding_ratio_standard_error".into(),
        "breeding_ratio_scope".into(),
        "breeding_ratio_kind".into(),
        "magnet_flux_n_m2_s".into(),
        "magnet_flux_standard_error_n_m2_s".into(),
        "magnet_flux_kind".into(),
        "magnet_swaps".into(),
        "magnet_swaps_kind".into(),
        "first_swap_year".into(),
        "first_swap_sampling_standard_error_y".into(),
        "first_swap_kind".into(),
        "net_electricity_twh".into(),
        "net_electricity_kind".into(),
        "final_usable_tritium_kg".into(),
        "horizon_years".into(),
        "transport_seed".into(),
        "transport_source_histories".into(),
    ]);
    for data in arrangements {
        let a = data.input.arrangement;
        let s = &data.summary;
        let blanket = if a.breeder {
            BREEDER_BLANKET_M
        } else {
            REFERENCE_BLANKET_M
        };
        let first_kind = match (s.first_swap_y, s.first_swap_relative_sampling) {
            (Some(_), Some(_)) => StatusKind::Partial.label().to_string(),
            (Some(_), None) => StatusKind::Conditional.label().to_string(),
            (None, _) if s.swaps == Some(0) => StatusKind::Conditional.label().to_string(),
            (None, _) => StatusKind::NotEvaluated.label().to_string(),
        };
        out.push_str(&row(&[
            a.id().into(),
            if a.port { "with-port" } else { "no-port" }.into(),
            if a.breeder {
                "breeder-heavy"
            } else {
                "reference"
            }
            .into(),
            number(blanket),
            number(BLANKET_PLUS_SHIELD_M - blanket),
            s.recorded.to_string(),
            optional(s.breeding.map(|b| b.0)),
            optional(s.breeding.map(|b| b.1)),
            if s.breeding.is_none() {
                String::new()
            } else if s.breeding_is_total {
                "whole-model H3 production".into()
            } else {
                "breeder-only H3 production".into()
            },
            kind_or(s.breeding.is_some(), StatusKind::Calculated),
            optional(s.magnet_flux.map(|b| b.0)),
            optional(s.magnet_flux.map(|b| b.1)),
            kind_or(s.magnet_flux.is_some(), StatusKind::Calculated),
            s.swaps.map_or_else(String::new, |n| n.to_string()),
            kind_or(s.swaps.is_some(), StatusKind::Conditional),
            optional(s.first_swap_y),
            optional(
                s.first_swap_y
                    .zip(s.first_swap_relative_sampling)
                    .map(|(y, r)| y * r),
            ),
            first_kind,
            optional(s.net_twh),
            kind_or(s.net_twh.is_some(), StatusKind::Conditional),
            optional(s.final_tritium_kg),
            if s.horizon_years > 0.0 {
                number(s.horizon_years)
            } else {
                String::new()
            },
            data.input
                .sampling
                .map_or_else(String::new, |x| x.seed.to_string()),
            data.input
                .sampling
                .map_or_else(String::new, |x| x.histories.to_string()),
        ]));
    }
    out
}

fn flag(value: Option<(f64, bool)>) -> (String, String) {
    match value {
        Some((v, true)) => (number(v), "resolved (>2 sigma)".into()),
        Some((v, false)) => (number(v), "within sampling noise".into()),
        None => (String::new(), String::new()),
    }
}

pub fn differences_csv(study: &StudyComparison) -> String {
    let mut out = row(&[
        "comparison".into(),
        "breeding_change_percent".into(),
        "breeding_screening_flag".into(),
        "magnet_flux_change_percent".into(),
        "magnet_flux_screening_flag".into(),
        "magnet_swaps_change".into(),
        "first_swap_change_y".into(),
        "net_electricity_change_twh".into(),
        "history_kind".into(),
    ]);
    let rows: [(&str, &Contrast); 4] = [
        (
            "breeder-heavy minus reference, with port",
            &study.allocation_with_port,
        ),
        (
            "breeder-heavy minus reference, no port",
            &study.allocation_no_port,
        ),
        ("port minus no port, reference", &study.port_reference),
        ("port minus no port, breeder-heavy", &study.port_breeder),
    ];
    for (name, c) in rows {
        let (b, bf) = flag(c.breeding_pct);
        let (x, xf) = flag(c.flux_pct);
        out.push_str(&row(&[
            name.into(),
            b,
            bf,
            x,
            xf,
            c.swaps.map_or_else(String::new, |n| n.to_string()),
            optional(c.first_swap_y),
            optional(c.net_twh),
            StatusKind::Conditional.label().into(),
        ]));
    }
    out
}

pub fn sweep_csv(points: &[TransportPoint], summaries: &[Option<HistorySummary>]) -> String {
    let mut out = row(&[
        "blanket_m".into(),
        "shield_m".into(),
        "variant_id".into(),
        "breeding_ratio_h3_per_source_neutron".into(),
        "breeding_ratio_standard_error".into(),
        "breeding_ratio_kind".into(),
        "magnet_flux_n_m2_s".into(),
        "magnet_flux_standard_error_n_m2_s".into(),
        "magnet_flux_kind".into(),
        "breeding_change_vs_previous_resolved_2sigma".into(),
        "transport_seed".into(),
        "transport_source_histories".into(),
        "magnets_replaceable".into(),
        "magnet_swaps".into(),
        "first_swap_year".into(),
        "permanent_limit_year".into(),
        "net_electricity_twh".into(),
        "final_usable_tritium_kg".into(),
        "history_kind".into(),
    ]);
    let ready = summaries.len() == points.len();
    for (i, p) in points.iter().enumerate() {
        let s = if ready { summaries[i].as_ref() } else { None };
        let resolved = i
            .checked_sub(1)
            .map(|j| difference_resolved(&points[j].breeding, &p.breeding).to_string())
            .unwrap_or_default();
        out.push_str(&row(&[
            number(p.blanket_m),
            number(p.shield_m),
            p.variant_id.clone(),
            number(p.breeding.mean),
            number(p.breeding.standard_error),
            StatusKind::Calculated.label().into(),
            number(p.magnet_flux.mean),
            number(p.magnet_flux.standard_error),
            StatusKind::Calculated.label().into(),
            resolved,
            p.seed.to_string(),
            p.histories.to_string(),
            s.map_or_else(String::new, |s| s.magnets_replaceable.to_string()),
            s.map_or_else(String::new, |s| s.magnet_replacements.to_string()),
            optional(s.and_then(|s| s.first_magnet_replacement_years)),
            optional(s.and_then(|s| s.magnet_permanent_limit_years)),
            optional(s.and_then(|s| s.net_electricity_twh)),
            optional(s.map(|s| s.final_available_tritium_kg)),
            kind_or(s.is_some(), StatusKind::Conditional),
        ]));
    }
    out
}

/// The evaluated ensemble of an arrangement, if it has one.
fn evaluated(data: &ArrangementData) -> Option<&HistoryEnsemble> {
    match &data.input.ensemble {
        EnsembleInput::Ready(e) if e.status == EnsembleStatus::Evaluated => Some(e.as_ref()),
        _ => None,
    }
}

/// One row per sample per arrangement with an evaluated ensemble: the sampled
/// driving rates and every per-sample outcome. Sample 0, the nominal history,
/// is in the summary table; numbering here is the engine's (1 to N).
pub fn ensemble_samples_csv(arrangements: &[&ArrangementData]) -> String {
    let ensembles: Vec<(&ArrangementData, &HistoryEnsemble)> = arrangements
        .iter()
        .filter_map(|d| Some((*d, evaluated(d)?)))
        .collect();
    let mut fluxes = BTreeSet::new();
    let mut replaced = BTreeSet::new();
    let mut triggered = BTreeSet::new();
    for (_, e) in &ensembles {
        for s in &e.samples {
            triggered.extend(s.first_trigger_response.keys().cloned());
            fluxes.extend(s.rates.component_average_flux_n_m2_s.keys().cloned());
            replaced.extend(s.replacements.keys().cloned());
        }
    }
    let mut header: Vec<String> = vec![
        "arrangement".into(),
        "sample".into(),
        "rejected_draws".into(),
        "breeder_h3_per_source_neutron".into(),
    ];
    header.extend(fluxes.iter().map(|id| format!("flux_{id}_n_m2_s")));
    header.extend(
        [
            "heating_w",
            "outcome",
            "terminal_time_years",
            "full_power_time_years",
            "final_usable_tritium_kg",
            "final_in_process_tritium_kg",
            "gross_electricity_twh",
            "auxiliary_electricity_twh",
            "net_electricity_twh",
        ]
        .map(String::from),
    );
    header.extend(replaced.iter().map(|id| format!("replacements_{id}")));
    header.extend(
        replaced
            .iter()
            .map(|id| format!("first_replacement_year_{id}")),
    );
    header.extend(triggered.iter().map(|id| format!("first_trigger_{id}")));
    let mut out = row(&header);
    for (data, e) in ensembles {
        for s in &e.samples {
            let mut cells: Vec<String> = vec![
                data.input.arrangement.id().into(),
                s.index.to_string(),
                s.rejected_draws.to_string(),
                number(s.rates.breeder_h3_per_source_neutron),
            ];
            cells.extend(
                fluxes
                    .iter()
                    .map(|id| optional(s.rates.component_average_flux_n_m2_s.get(id).copied())),
            );
            cells.push(optional(s.rates.transport_deposited_heat_w));
            cells.push(outcome_category(&s.outcome).into());
            cells.push(number(s.terminal_time_s / JULIAN_YEAR_SECONDS));
            cells.push(number(s.full_power_time_s / JULIAN_YEAR_SECONDS));
            cells.push(number(s.final_available_tritium_kg));
            cells.push(number(s.final_in_process_tritium_kg));
            for v in [
                s.cumulative_gross_electricity_mwh,
                s.cumulative_auxiliary_electricity_mwh,
                s.cumulative_net_electricity_mwh,
            ] {
                cells.push(optional(v.map(|x| x / MWH_PER_TWH)));
            }
            cells.extend(replaced.iter().map(|id| {
                s.replacements
                    .get(id)
                    .map_or_else(String::new, u32::to_string)
            }));
            cells.extend(replaced.iter().map(|id| {
                optional(
                    s.first_replacement_time_s
                        .get(id)
                        .copied()
                        .flatten()
                        .map(|t| t / JULIAN_YEAR_SECONDS),
                )
            }));
            // The response whose limit tripped the component first; empty when
            // it never tripped.
            cells.extend(triggered.iter().map(|id| {
                s.first_trigger_response
                    .get(id)
                    .cloned()
                    .unwrap_or_default()
            }));
            out.push_str(&row(&cells));
        }
    }
    out
}

/// Output name, unit and factor from the engine's unit, as the summary table
/// writes them.
fn summary_unit(name: &str) -> (String, &'static str, f64) {
    let year = 1.0 / JULIAN_YEAR_SECONDS;
    if let Some(base) = name.strip_suffix("_time_s") {
        return (format!("{base}_time"), "years", year);
    }
    if let Some((base, id)) = name.split_once(":")
        && let Some(stem) = base.strip_suffix("_time_s")
    {
        return (format!("{stem}_time:{id}"), "years", year);
    }
    if let Some(base) = name.strip_suffix("_mwh") {
        return (base.to_owned(), "TWh", 1.0 / MWH_PER_TWH);
    }
    if let Some(base) = name.strip_suffix("_kg") {
        return (base.to_owned(), "kg", 1.0);
    }
    if name.starts_with("replacements:") {
        return (name.to_owned(), "count", 1.0);
    }
    (name.to_owned(), "", 1.0)
}

fn nominal_outputs(
    history: Option<&faris_engine::history::HistoryResult>,
) -> Option<SampleOutcome> {
    history.and_then(nominal_sample)
}

/// The summary of each arrangement's ensemble in long format: the nominal
/// value, and for an evaluated ensemble the mean, the P5, median and P95 with
/// their 95 % sampling intervals, and the share of samples in each category of
/// a discrete output with its Wilson interval. A not-evaluated ensemble keeps
/// the nominal values and carries its reason and next step in `note`.
pub fn ensemble_summary_csv(arrangements: &[&ArrangementData]) -> String {
    let mut out = row(&[
        "arrangement".into(),
        "ensemble_status".into(),
        "output".into(),
        "unit".into(),
        "statistic".into(),
        "category".into(),
        "value".into(),
        "ci95_low".into(),
        "ci95_high".into(),
        "n".into(),
        "note".into(),
    ]);
    for data in arrangements {
        let id = data.input.arrangement.id();
        let nominal = nominal_outputs(data.input.history.as_ref());
        let (status, note, ensemble): (&str, String, Option<&HistoryEnsemble>) =
            match &data.input.ensemble {
                EnsembleInput::Ready(e) => match &e.status {
                    EnsembleStatus::Evaluated => ("evaluated", SCOPE_LINE.into(), Some(e.as_ref())),
                    EnsembleStatus::NotEvaluated { why, next_step } => (
                        "not_evaluated",
                        format!("no uncertainty range: {why}. Next step: {next_step}."),
                        None,
                    ),
                },
                EnsembleInput::Failed(why) => (
                    "failed",
                    format!("no uncertainty range: the ensemble could not be calculated: {why}"),
                    None,
                ),
                EnsembleInput::None if nominal.is_some() => (
                    "not_calculated",
                    "no uncertainty range: the ensemble was not calculated before this export"
                        .into(),
                    None,
                ),
                EnsembleInput::None => continue,
            };
        let cell = |output: &str,
                    unit: &str,
                    statistic: &str,
                    category: &str,
                    value: String,
                    ci: (String, String),
                    n: String| {
            row(&[
                id.into(),
                status.into(),
                output.into(),
                unit.into(),
                statistic.into(),
                category.into(),
                value,
                ci.0,
                ci.1,
                n,
                note.clone(),
            ])
        };
        let no_ci = || (String::new(), String::new());
        if let Some(n) = &nominal {
            for (name, value) in n.scalar_outputs() {
                let (output, unit, factor) = summary_unit(&name);
                out.push_str(&cell(
                    &output,
                    unit,
                    "nominal",
                    "",
                    number(value * factor),
                    no_ci(),
                    String::new(),
                ));
            }
            out.push_str(&cell(
                "terminal_status",
                "",
                "nominal",
                outcome_category(&n.outcome),
                String::new(),
                no_ci(),
                String::new(),
            ));
            for (component, response) in &n.first_trigger_response {
                out.push_str(&cell(
                    &format!("first_trigger:{component}"),
                    "",
                    "nominal",
                    response,
                    String::new(),
                    no_ci(),
                    String::new(),
                ));
            }
        }
        let Some(summary) = ensemble.and_then(|e| e.summary.as_ref()) else {
            continue;
        };
        for c in &summary.continuous {
            let (output, unit, factor) = summary_unit(&c.name);
            let n = c.n.to_string();
            out.push_str(&cell(
                &output,
                unit,
                "mean",
                "",
                number(c.mean * factor),
                no_ci(),
                n.clone(),
            ));
            for (stat, q) in [("p5", &c.p5), ("p50", &c.p50), ("p95", &c.p95)] {
                out.push_str(&cell(
                    &output,
                    unit,
                    stat,
                    "",
                    number(q.value * factor),
                    (
                        optional(q.ci95_low.map(|v| v * factor)),
                        optional(q.ci95_high.map(|v| v * factor)),
                    ),
                    n.clone(),
                ));
            }
        }
        for d in &summary.discrete {
            let unit = if d.name.starts_with("replacements:") {
                "count"
            } else {
                ""
            };
            for (category, p) in &d.categories {
                out.push_str(&cell(
                    &d.name,
                    unit,
                    "share",
                    category,
                    number(p.fraction),
                    (number(p.wilson95_low), number(p.wilson95_high)),
                    d.n.to_string(),
                ));
            }
        }
    }
    out
}

pub fn assumptions_csv(rows: &[AssumptionRow]) -> String {
    let mut out = row(&[
        "assumption".into(),
        "value".into(),
        "value_number".into(),
        "unit".into(),
        "kind".into(),
        "provenance".into(),
    ]);
    for r in rows {
        out.push_str(&row(&[
            r.name.clone(),
            r.value.clone(),
            optional(r.value_number),
            r.unit.clone(),
            r.kind.label().into(),
            r.provenance.clone(),
        ]));
    }
    out
}

pub fn caveats_csv(caveats: &[Caveat]) -> String {
    let mut out = row(&[
        "item".into(),
        "kind".into(),
        "why".into(),
        "what_would_settle_it".into(),
    ]);
    for c in caveats {
        out.push_str(&row(&[
            c.item.clone(),
            c.kind.label().into(),
            c.why.clone(),
            c.settle.clone(),
        ]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_are_quoted_only_when_needed() {
        assert_eq!(field("plain"), "plain");
        assert_eq!(field("a,b"), "a,b".replace("a,b", "\"a,b\""));
        assert_eq!(field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(number(f64::NAN), "");
        assert_eq!(number(0.1 + 0.2), "0.30000000000000004");
    }
}
