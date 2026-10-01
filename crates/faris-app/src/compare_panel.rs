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
use faris_engine::brief::{
    ArrangementSummary as Cell, BLANKET_PLUS_SHIELD_M, BREEDER_BLANKET_M, Contrast,
    REFERENCE_BLANKET_M, compare_study, summarize_arrangement,
};

pub const VARIANTS: [&str; 2] = ["reference", "breeder-emphasis"];

fn build_cell(history: &HistoryPanel, panel: Option<&TransportPanel>, variant: &str) -> Cell {
    let Some((panel, record)) = panel.and_then(|p| p.record(variant).map(|r| (p, r))) else {
        return Cell::default();
    };
    summarize_arrangement(
        Some(&panel.summary(variant)),
        history.result(&record.scenario_sha256, variant),
    )
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
    let build = |panel: Option<&TransportPanel>| {
        [
            build_cell(history, panel, VARIANTS[0]),
            build_cell(history, panel, VARIANTS[1]),
        ]
    };
    let cells: [[Cell; 2]; 2] = [build(Some(port)), build(control)];
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
            let heading = |blanket: f64| {
                format!(
                    "{:.2} / {:.2} m blanket / shield",
                    blanket,
                    BLANKET_PLUS_SHIELD_M - blanket
                )
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
            let study = compare_study(&cells);
            egui::Grid::new("compare-deltas")
                .num_columns(6)
                .striped(true)
                .spacing([18.0, 6.0])
                .show(ui, |ui| {
                    for heading in ["", "Δ breeding", "Δ magnet flux", "Δ swaps", "Δ first swap", "Δ net electricity"] {
                        ui.strong(heading);
                    }
                    ui.end_row();
                    contrast_row(ui, "Breeder-heavy − Reference · with port", &study.allocation_with_port);
                    contrast_row(ui, "Breeder-heavy − Reference · no port", &study.allocation_no_port);
                    contrast_row(ui, "Port − No port · reference", &study.port_reference);
                    contrast_row(ui, "Port − No port · breeder-heavy", &study.port_breeder);
                });

            ui.add_space(10.0);
            let sentences = &study.takeaways;
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
