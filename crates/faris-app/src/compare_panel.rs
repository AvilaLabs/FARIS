//! Side-by-side comparison of the four recorded arrangements (two blanket/shield
//! allocations, each with the finite outboard port and as a no-port control).
//!
//! Call from the application as
//! `compare_panel::compare_view(ui, &self.history, &self.transport, self.paired.as_ref().map(|(_, p)| p))`
//! where `self.transport` is the panel holding the port scenario and the paired
//! panel is the no-port control (either may be missing, and its cells then read
//! "not recorded"). Scenario identities come from the transport records, so no
//! hashes are passed. The view scrolls itself. Every number is read from the
//! recorded transport results or from histories the history panel already holds;
//! the only arithmetic here is differences, ratios and the engine's resolution
//! and first-order uncertainty helpers.

use crate::{
    badge::{self, Kind},
    history_panel::{HistoryPanel, arrangement_color, arrangement_label},
    transport_panel::TransportPanel,
};
use eframe::egui;
use faris_engine::{
    comparison::{
        component_replacement_spans, difference_resolved_2sigma,
        first_crossing_relative_uncertainty,
    },
    history::JULIAN_YEAR_SECONDS,
};

/// Blanket thickness of the two recorded allocations, metres (the shield takes
/// the remainder of the same 0.9 m). Shown in headings and the takeaway only.
const REFERENCE_BLANKET_M: f64 = 0.45;
const BREEDER_BLANKET_M: f64 = 0.55;
const VARIANTS: [&str; 2] = ["reference", "breeder-emphasis"];

/// Numbers for one arrangement. None means the source was not recorded or not
/// calculated; nothing is filled in.
#[derive(Clone, Debug, Default, PartialEq)]
struct Cell {
    recorded: bool,
    breeding: Option<(f64, f64)>,
    breeding_is_total: bool,
    magnet_flux: Option<(f64, f64)>,
    swaps: Option<usize>,
    first_swap_y: Option<f64>,
    first_swap_relative_sampling: Option<f64>,
    net_twh: Option<f64>,
    final_tritium_kg: Option<f64>,
    horizon_years: f64,
}

fn build_cell(history: &HistoryPanel, panel: Option<&TransportPanel>, variant: &str) -> Cell {
    let Some((panel, record)) = panel.and_then(|p| p.record(variant).map(|r| (p, r))) else {
        return Cell::default();
    };
    let summary = panel.summary(variant);
    let mut cell = Cell {
        recorded: true,
        breeding: summary
            .total_h3_per_source
            .or(summary.breeder_h3_per_source),
        breeding_is_total: summary.total_h3_per_source.is_some(),
        magnet_flux: summary.magnet_flux,
        ..Cell::default()
    };
    if let Some(h) = history.result(&record.scenario_sha256, variant) {
        let horizon_s = h.assumptions.horizon_s;
        cell.horizon_years = horizon_s / JULIAN_YEAR_SECONDS;
        if h.assumptions
            .service_limits
            .iter()
            .any(|l| l.component_id == "magnets")
        {
            let spans = component_replacement_spans(&h.events, "magnets", horizon_s);
            cell.swaps = Some(spans.len());
            cell.first_swap_y = spans.first().map(|(s, _)| s / JULIAN_YEAR_SECONDS);
            cell.first_swap_relative_sampling = h
                .driving_rates
                .component_average_flux_n_m2_s
                .get("magnets")
                .and_then(|r| first_crossing_relative_uncertainty(r.mean, r.standard_error));
        }
        if let Some(last) = h.snapshots.last() {
            cell.net_twh = last.cumulative_net_electricity_mwh.map(|v| v / 1e6);
            cell.final_tritium_kg = Some(last.available_tritium_kg);
        }
    }
    cell
}

/// A change `to − from` for each compared quantity. Percentages are relative to
/// `from`; the flag says whether a transport difference exceeds 2σ.
#[derive(Clone, Debug, Default, PartialEq)]
struct Contrast {
    breeding_pct: Option<(f64, bool)>,
    flux_pct: Option<(f64, bool)>,
    swaps: Option<i64>,
    first_swap_y: Option<f64>,
    net_twh: Option<f64>,
}

fn percent_change(from: f64, to: f64) -> Option<f64> {
    (from != 0.0 && from.is_finite() && to.is_finite()).then(|| (to - from) / from * 100.0)
}

fn sampled_change(from: Option<(f64, f64)>, to: Option<(f64, f64)>) -> Option<(f64, bool)> {
    let ((a, sa), (b, sb)) = (from?, to?);
    Some((
        percent_change(a, b)?,
        difference_resolved_2sigma(a, sa, b, sb),
    ))
}

fn contrast(from: &Cell, to: &Cell) -> Contrast {
    Contrast {
        breeding_pct: sampled_change(from.breeding, to.breeding),
        flux_pct: sampled_change(from.magnet_flux, to.magnet_flux),
        swaps: from.swaps.zip(to.swaps).map(|(a, b)| b as i64 - a as i64),
        first_swap_y: from.first_swap_y.zip(to.first_swap_y).map(|(a, b)| b - a),
        net_twh: from.net_twh.zip(to.net_twh).map(|(a, b)| b - a),
    }
}

fn count_word(n: u64) -> String {
    match n {
        1 => "one".into(),
        2 => "two".into(),
        3 => "three".into(),
        4 => "four".into(),
        5 => "five".into(),
        other => other.to_string(),
    }
}

fn plural(n: u64, noun: &str) -> String {
    if n == 1 {
        format!("{} {noun}", count_word(n))
    } else {
        format!("{} {noun}s", count_word(n))
    }
}

/// One plain-language sentence for moving blanket thickness at the expense of
/// shield (breeder-heavy versus reference), built from the contrast numbers.
fn allocation_takeaway(c: &Contrast, shift_cm: f64, horizon_years: f64) -> Option<String> {
    let (breeding, resolved) = c.breeding_pct?;
    let breeding_clause = if resolved {
        let verb = if breeding >= 0.0 { "raises" } else { "lowers" };
        format!("{verb} breeding by {:.1} %", breeding.abs())
    } else {
        format!("changes breeding by {breeding:+.1} % (within sampling noise)")
    };
    let mut tail = Vec::new();
    let mut worse_first = false;
    if let Some(d) = c.first_swap_y
        && d.abs() >= 0.05
    {
        worse_first = d < 0.0;
        tail.push(format!(
            "{} the first magnet swap {:.1} years {}",
            if d < 0.0 { "brings" } else { "pushes" },
            d.abs(),
            if d < 0.0 { "earlier" } else { "later" }
        ));
    }
    if let Some(d) = c.swaps {
        tail.push(match d {
            0 => format!("leaves the swap count over {horizon_years:.0} years unchanged"),
            d if d > 0 => format!(
                "adds {} over {horizon_years:.0} years",
                plural(d.unsigned_abs(), "swap")
            ),
            d => format!(
                "removes {} over {horizon_years:.0} years",
                plural(d.unsigned_abs(), "swap")
            ),
        });
    }
    let joiner = if worse_first && breeding > 0.0 && resolved {
        "but"
    } else {
        "and"
    };
    let mut sentence =
        format!("Shifting {shift_cm:.0} cm from shield to blanket {breeding_clause}");
    if let Some((first, rest)) = tail.split_first() {
        sentence.push_str(&format!(" {joiner} {first}"));
        if let Some(last) = rest.first() {
            sentence.push_str(&format!(" and {last}"));
        }
    }
    sentence.push('.');
    Some(sentence)
}

/// One sentence for the finite outboard port in the reference allocation
/// (`c` is port minus no-port).
fn port_takeaway(c: &Contrast, horizon_years: f64) -> Option<String> {
    let swaps = c.swaps?;
    let net = c.net_twh?;
    let swap_clause = match swaps {
        0 => format!("leaves the magnet swap count over {horizon_years:.0} years unchanged"),
        d if d > 0 => format!(
            "adds {} over {horizon_years:.0} years",
            plural(d.unsigned_abs(), "magnet swap")
        ),
        d => format!(
            "removes {} over {horizon_years:.0} years",
            plural(d.unsigned_abs(), "magnet swap")
        ),
    };
    Some(format!(
        "Adding the finite outboard port {swap_clause} and {} {:.2} TWh of lifetime net electricity (reference allocation).",
        if net < 0.0 { "costs" } else { "adds" },
        net.abs()
    ))
}

fn pair(value: Option<(f64, f64)>, decimals: usize) -> String {
    value.map_or("—".into(), |(m, se)| {
        format!("{m:.decimals$} ± {se:.decimals$}")
    })
}

fn case_color(port: bool, breeder: bool) -> egui::Color32 {
    arrangement_color(port, breeder)
}

fn cell_ui(ui: &mut egui::Ui, id: &str, cell: &Cell, port: bool, breeder: bool, width: f32) {
    let color = case_color(port, breeder);
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.5, color.gamma_multiply(0.8)))
        .fill(color.gamma_multiply(0.07))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_min_width(width - 18.0);
            if !cell.recorded {
                ui.weak("not recorded")
                    .on_hover_text("No transport record is loaded for this arrangement.");
                return;
            }
            egui::Grid::new(("compare-cell", id))
                .num_columns(2)
                .spacing([12.0, 3.0])
                .show(ui, |ui| {
                    ui.label(if cell.breeding_is_total {
                        "Tritium breeding ratio"
                    } else {
                        "Breeder H3 / source"
                    })
                    .on_hover_text("Tritium atoms produced per source neutron, ± one Monte Carlo standard error. Cold-data surrogate; scientific qualification NOT_EVALUATED.");
                    ui.monospace(pair(cell.breeding, 4));
                    ui.end_row();
                    ui.label("Magnet mean flux")
                        .on_hover_text("Component-average neutron flux in the magnet envelope at reference power, ± one sampling standard error. Volume and model/data uncertainty are separate.");
                    ui.monospace(match cell.magnet_flux {
                        Some((m, se)) => format!("{} ± {} n/m²/s", fmt_sig(m), fmt_sig(se)),
                        None => "—".into(),
                    });
                    ui.end_row();
                    let horizon = if cell.horizon_years > 0.0 { cell.horizon_years } else { 30.0 };
                    ui.label(format!("Magnet swaps in {horizon:.0} y"));
                    ui.horizontal(|ui| match cell.swaps {
                        Some(n) => {
                            ui.monospace(n.to_string());
                            badge::kind_badge(ui, Kind::Conditional, "Follows from the authored magnet service limit applied to transport-driven fluence; not a qualified lifetime.");
                        }
                        None => {
                            ui.weak("no limit declared").on_hover_text("The selected assumptions declare no magnet service limit, so no swap count is inferred.");
                        }
                    });
                    ui.end_row();
                    ui.label("First swap");
                    ui.horizontal(|ui| match (cell.first_swap_y, cell.first_swap_relative_sampling) {
                        (Some(y), Some(rel)) => {
                            ui.monospace(format!("{y:.1} ± {:.1} y", y * rel));
                            badge::badge(ui, Kind::Partial, "sampling, first order", "Relative sampling error of the magnet flux carried onto the crossing time (fluence = flux × time). Ignores outage-timing nonlinearity, the volume average, and all model and data uncertainty, so it is a lower bound on the true uncertainty.");
                        }
                        (Some(y), None) => {
                            ui.monospace(format!("{y:.1} y"));
                        }
                        (None, _) => {
                            ui.weak(if cell.swaps == Some(0) { "none in horizon" } else { "—" });
                        }
                    });
                    ui.end_row();
                    ui.label("Lifetime net electricity");
                    ui.horizontal(|ui| match cell.net_twh {
                        Some(v) => {
                            ui.monospace(format!("{v:.2} TWh"));
                            badge::kind_badge(ui, Kind::Conditional, "Signed net of gross output and auxiliary load under authored energy assumptions; not a plant estimate.");
                        }
                        None => {
                            ui.weak("—");
                        }
                    });
                    ui.end_row();
                    ui.label("Final usable tritium");
                    ui.monospace(
                        cell.final_tritium_kg
                            .map_or("—".into(), |v| format!("{v:.2} kg")),
                    );
                    ui.end_row();
                });
        });
}

/// Compact scientific notation for fluxes (1.64e14).
fn fmt_sig(v: f64) -> String {
    format!("{v:.2e}")
}

fn delta_cell(
    ui: &mut egui::Ui,
    text: Option<String>,
    badge_kind: Option<(Kind, &str, &str)>,
    none_hint: &str,
) {
    ui.horizontal(|ui| match text {
        Some(t) => {
            ui.monospace(t);
            if let Some((kind, label, why)) = badge_kind {
                badge::badge(ui, kind, label, why);
            }
        }
        None => {
            ui.weak("—").on_hover_text(none_hint);
        }
    });
}

const NOISE_NOTE: &str = "Compared with twice the combined Monte Carlo standard error, 2·√(SE₁²+SE₂²). Covariance between runs is not modelled and the runs use distinct seeds, so this is a screening flag, not a significance test.";
const CONDITIONAL_NOTE: &str = "Conditional on authored assumptions. Difference of two histories driven by the same authored assumptions. Sampling and model uncertainty are not propagated; not a qualified result.";

fn sampled_badge(resolved: bool) -> (Kind, &'static str, &'static str) {
    if resolved {
        (Kind::Checked, "resolved (>2σ)", NOISE_NOTE)
    } else {
        (Kind::Partial, "within sampling noise", NOISE_NOTE)
    }
}

fn contrast_row(ui: &mut egui::Ui, title: &str, c: &Contrast) {
    ui.label(title);
    delta_cell(
        ui,
        c.breeding_pct.map(|(v, _)| format!("{v:+.2} %")),
        c.breeding_pct.map(|(_, r)| sampled_badge(r)),
        "Needs a recorded breeding ratio for both arrangements.",
    );
    delta_cell(
        ui,
        c.flux_pct.map(|(v, _)| format!("{v:+.1} %")),
        c.flux_pct.map(|(_, r)| sampled_badge(r)),
        "Needs a recorded magnet flux for both arrangements.",
    );
    let conditional =
        |present: bool| present.then_some((Kind::Conditional, "conditional", CONDITIONAL_NOTE));
    delta_cell(
        ui,
        c.swaps.map(|v| format!("{v:+}")),
        conditional(c.swaps.is_some()),
        "Needs a magnet service limit in the history assumptions.",
    );
    delta_cell(
        ui,
        c.first_swap_y.map(|v| format!("{v:+.1} y")),
        conditional(c.first_swap_y.is_some()),
        "Needs a first magnet swap in both arrangements; one of them has none within the horizon.",
    );
    delta_cell(
        ui,
        c.net_twh.map(|v| format!("{v:+.2} TWh")),
        conditional(c.net_twh.is_some()),
        "Needs a net-electricity ledger for both arrangements.",
    );
    ui.end_row();
}

/// Horizontal bars, one row per arrangement, grouped in port / no-port pairs.
fn paired_bars(
    ui: &mut egui::Ui,
    title: &str,
    unit: &str,
    rows: &[(bool, bool, Option<f64>)],
    decimals: usize,
) {
    ui.strong(title);
    let row_h = 20.0;
    let gap = 8.0;
    let label_w = 150.0;
    let height = rows.len() as f32 * row_h + (rows.len() / 2) as f32 * gap + 4.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(260.0), height),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    let max = rows
        .iter()
        .filter_map(|r| r.2)
        .fold(f64::MIN_POSITIVE, f64::max);
    let bar_left = rect.left() + label_w;
    let bar_w = (rect.width() - label_w - 78.0).max(40.0);
    let mut y = rect.top() + 2.0;
    for (index, (port, breeder, value)) in rows.iter().enumerate() {
        if index > 0 && index % 2 == 0 {
            y += gap;
        }
        let color = case_color(*port, *breeder);
        painter.text(
            egui::pos2(bar_left - 8.0, y + row_h / 2.0),
            egui::Align2::RIGHT_CENTER,
            arrangement_label(*port, *breeder),
            egui::FontId::proportional(12.0),
            color,
        );
        match value {
            Some(v) => {
                let w = (v.max(0.0) / max) as f32 * bar_w;
                let bar = egui::Rect::from_min_size(
                    egui::pos2(bar_left, y + 3.0),
                    egui::vec2(w.max(1.5), row_h - 6.0),
                );
                painter.rect_filled(
                    bar,
                    2.0,
                    color.gamma_multiply(if *port { 1.0 } else { 0.7 }),
                );
                painter.text(
                    egui::pos2(bar.right() + 6.0, y + row_h / 2.0),
                    egui::Align2::LEFT_CENTER,
                    format!("{v:.decimals$} {unit}"),
                    egui::FontId::monospace(12.0),
                    egui::Color32::from_gray(225),
                );
            }
            None => {
                painter.text(
                    egui::pos2(bar_left + 6.0, y + row_h / 2.0),
                    egui::Align2::LEFT_CENTER,
                    "—",
                    egui::FontId::proportional(12.0),
                    egui::Color32::GRAY,
                );
            }
        }
        y += row_h;
    }
    painter.line_segment(
        [
            egui::pos2(bar_left, rect.top()),
            egui::pos2(bar_left, rect.bottom()),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_gray(110)),
    );
}

pub fn compare_view(
    ui: &mut egui::Ui,
    history: &HistoryPanel,
    port: &TransportPanel,
    control: Option<&TransportPanel>,
    after: &mut dyn FnMut(&mut egui::Ui),
) {
    // cells[port 0 / control 1][reference 0 / breeder 1]
    let cells: Vec<Vec<Cell>> = [Some(port), control]
        .iter()
        .map(|panel| {
            VARIANTS
                .iter()
                .map(|v| build_cell(history, *panel, v))
                .collect()
        })
        .collect();
    egui::ScrollArea::both()
        .id_salt("compare-view")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_max_width((ui.clip_rect().width() - 24.0).max(600.0));
            ui.horizontal_wrapped(|ui| {
                ui.strong("Compare the four recorded arrangements");
                badge::kind_badge(ui, Kind::Conditional, "Transport rows are cold-data surrogate Monte Carlo results (qualification NOT_EVALUATED). History rows are conditional on the authored operating assumptions currently selected.");
                if history.is_stale() {
                    ui.colored_label(egui::Color32::YELLOW, "Histories belong to earlier inputs; recalculating.");
                }
            });
            ui.add_space(4.0);
            let shift_cm = ((BREEDER_BLANKET_M - REFERENCE_BLANKET_M) * 100.0).round();
            let heading = |blanket: f64| {
                format!("{:.2} / {:.2} m blanket / shield", blanket, 0.9 - blanket)
            };
            let label_w = 80.0;
            let column_w = ((ui.available_width() - label_w - 24.0) / 2.0).clamp(300.0, 560.0);
            let column = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
                ui.allocate_ui_with_layout(
                    egui::vec2(column_w, 10.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_width(column_w);
                        add(ui)
                    },
                );
            };
            ui.horizontal_top(|ui| {
                ui.add_space(label_w + ui.spacing().item_spacing.x);
                column(ui, &mut |ui| {
                    ui.strong("Reference");
                    ui.small(heading(REFERENCE_BLANKET_M));
                });
                column(ui, &mut |ui| {
                    ui.strong("Breeder-heavy");
                    ui.small(heading(BREEDER_BLANKET_M));
                });
            });
            for (row, name) in ["With port", "No port"].iter().enumerate() {
                ui.horizontal_top(|ui| {
                    ui.allocate_ui(egui::vec2(label_w, 10.0), |ui| {
                        ui.set_min_width(label_w);
                        ui.strong(*name)
                    });
                    for (index, cell) in cells[row].iter().enumerate() {
                        column(ui, &mut |ui| {
                            cell_ui(
                                ui,
                                &format!("{row}-{index}"),
                                cell,
                                row == 0,
                                index == 1,
                                column_w,
                            );
                        });
                    }
                });
            }

            ui.add_space(10.0);
            ui.strong("What changes");
            ui.small("Each row is the second arrangement minus the first. Transport differences carry a screening flag; history differences are conditional on the authored assumptions.");
            let port_alloc = contrast(&cells[0][0], &cells[0][1]);
            let control_alloc = contrast(&cells[1][0], &cells[1][1]);
            let ref_port = contrast(&cells[1][0], &cells[0][0]);
            let breeder_port = contrast(&cells[1][1], &cells[0][1]);
            egui::Grid::new("compare-deltas")
                .num_columns(6)
                .striped(true)
                .spacing([18.0, 6.0])
                .show(ui, |ui| {
                    for heading in ["", "Δ breeding", "Δ magnet flux", "Δ swaps", "Δ first swap", "Δ net electricity"] {
                        ui.strong(heading);
                    }
                    ui.end_row();
                    contrast_row(ui, "Breeder-heavy − Reference · with port", &port_alloc);
                    contrast_row(ui, "Breeder-heavy − Reference · no port", &control_alloc);
                    contrast_row(ui, "Port − No port · reference", &ref_port);
                    contrast_row(ui, "Port − No port · breeder-heavy", &breeder_port);
                });

            ui.add_space(10.0);
            let horizon = cells[0][0].horizon_years.max(cells[0][1].horizon_years);
            let horizon = if horizon > 0.0 { horizon } else { 30.0 };
            let sentences: Vec<String> = [
                allocation_takeaway(&port_alloc, shift_cm, horizon),
                port_takeaway(&ref_port, horizon),
            ]
            .into_iter()
            .flatten()
            .collect();
            if !sentences.is_empty() {
                egui::Frame::new()
                    .fill(egui::Color32::from_white_alpha(8))
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new(sentences.join(" ")).size(15.0));
                            badge::kind_badge(ui, Kind::Conditional, "Generated from the numbers above. Conditional on the authored service limit and replacement duration; sampling noise on the transport inputs is not propagated into the swap years.");
                        });
                    });
            }

            ui.add_space(12.0);
            let order = [(true, false), (false, false), (true, true), (false, true)];
            let pick = |f: &dyn Fn(&Cell) -> Option<f64>| -> Vec<(bool, bool, Option<f64>)> {
                order
                    .iter()
                    .map(|(is_port, breeder)| {
                        let cell = &cells[usize::from(!*is_port)][usize::from(*breeder)];
                        (*is_port, *breeder, if cell.recorded { f(cell) } else { None })
                    })
                    .collect()
            };
            ui.columns(2, |columns| {
                paired_bars(&mut columns[0], "Lifetime net electricity", "TWh", &pick(&|c| c.net_twh), 2);
                paired_bars(&mut columns[1], "Magnet swaps over the horizon", "swaps", &pick(&|c| c.swaps.map(|n| n as f64)), 0);
            });
            ui.small("Bars start at zero. Net electricity differs by a few percent between arrangements; the swap count is the large effect. Both are conditional on authored assumptions.");
            ui.add_space(8.0);
            ui.separator();
            after(ui);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(breeding: f64, flux: f64, swaps: usize, first: Option<f64>, net: f64) -> Cell {
        Cell {
            recorded: true,
            breeding: Some((breeding, 0.0014)),
            magnet_flux: Some((flux, flux * 0.14)),
            swaps: Some(swaps),
            first_swap_y: first,
            net_twh: Some(net),
            horizon_years: 30.0,
            ..Cell::default()
        }
    }

    #[test]
    fn contrast_is_second_minus_first() {
        let a = cell(1.295, 1.6e14, 4, Some(6.8), 34.13);
        let b = cell(1.312, 2.1e14, 5, Some(5.2), 33.72);
        let c = contrast(&a, &b);
        assert_eq!(c.swaps, Some(1));
        assert!((c.first_swap_y.unwrap() + 1.6).abs() < 1e-9);
        assert!((c.net_twh.unwrap() + 0.41).abs() < 1e-9);
        let (breeding, resolved) = c.breeding_pct.unwrap();
        assert!((breeding - 1.3127).abs() < 1e-3);
        assert!(resolved);
        // A 31 % flux change on 14 % relative errors is within sampling noise.
        assert!(!c.flux_pct.unwrap().1);
    }

    #[test]
    fn missing_swaps_leave_the_first_swap_delta_unset() {
        let none = cell(1.3, 1e14, 0, None, 35.4);
        let some = cell(1.3, 1e14, 4, Some(6.8), 34.1);
        let c = contrast(&none, &some);
        assert_eq!(c.swaps, Some(4));
        assert_eq!(c.first_swap_y, None);
        assert_eq!(contrast(&Cell::default(), &some).swaps, None);
    }

    #[test]
    fn takeaway_follows_the_numbers() {
        let c = Contrast {
            breeding_pct: Some((1.3, true)),
            swaps: Some(1),
            first_swap_y: Some(-1.6),
            ..Contrast::default()
        };
        assert_eq!(
            allocation_takeaway(&c, 10.0, 30.0).unwrap(),
            "Shifting 10 cm from shield to blanket raises breeding by 1.3 % but brings the first magnet swap 1.6 years earlier and adds one swap over 30 years."
        );
        let flat = Contrast {
            breeding_pct: Some((0.4, false)),
            swaps: Some(0),
            ..Contrast::default()
        };
        assert_eq!(
            allocation_takeaway(&flat, 10.0, 30.0).unwrap(),
            "Shifting 10 cm from shield to blanket changes breeding by +0.4 % (within sampling noise) and leaves the swap count over 30 years unchanged."
        );
        assert!(allocation_takeaway(&Contrast::default(), 10.0, 30.0).is_none());
        let port = Contrast {
            swaps: Some(4),
            net_twh: Some(-1.31),
            ..Contrast::default()
        };
        assert_eq!(
            port_takeaway(&port, 30.0).unwrap(),
            "Adding the finite outboard port adds four magnet swaps over 30 years and costs 1.31 TWh of lifetime net electricity (reference allocation)."
        );
    }
}
