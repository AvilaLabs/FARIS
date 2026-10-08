//! Maintenance view: opens a `faris-maintenance-result/v0.1` file and shows, per
//! design, the replacement outages computed from the decay heat of the components
//! around the replaced part beside the authored (fixed) ones, and why each
//! cooldown is what it is.
//!
//! Call from the application as `maintenance_panel.window(ctx)`; the panel is a
//! floating window opened from the top bar. The file is read and parsed on a
//! worker thread. Every number is read from the result file; the only arithmetic
//! here is unit conversion (seconds to days and years) and sorting. The view
//! makes no ranking claim: it reports downtime, availability and electricity
//! under both duration models.

use crate::{
    badge::{self, Kind},
    sweep_panel::nice_ticks,
};
use eframe::egui;
use faris_engine::{
    history::{HistoryOutcome, JULIAN_YEAR_SECONDS},
    maintenance::{
        ComputedResult, Contrast, DesignResult, EventRecord, HistorySummary,
        MAINTENANCE_RESULT_VERSION, MaintenanceResult, NotEvaluated, Status, ThresholdRecord, Why,
    },
};
use faris_model::maintenance::GoverningQuantity;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};

const DAY_S: f64 = 86_400.0;

/// What "computed" means, shown under the summary.
pub const COMPUTED_MEANING: &str = "Computed: each replacement's outage is the cooldown until the decay heat per volume of the governing components falls below the class threshold q*, plus the class's work time; the operating history is rerun with those outages until they stop changing. Fixed is the authored duration. The two models are compared on downtime, availability and lifetime electricity; the comparison makes no ranking claim.";

const NET_UNAVAILABLE: &str = "The operating history recorded no cumulative net electricity at its end (the history assumptions carry no energy assumptions, or no snapshot was recorded), so none is inferred here.";

struct Loaded {
    file_name: String,
    result: Box<MaintenanceResult>,
}

type LoadResult = Result<Loaded, String>;

enum State {
    Empty,
    Failed(String),
    Ready(Loaded),
}

pub struct MaintenancePanel {
    pub open: bool,
    state: State,
    selected: Option<String>,
    loading: Option<Receiver<LoadResult>>,
    dialog: Option<Receiver<Option<PathBuf>>>,
}

impl Default for MaintenancePanel {
    fn default() -> Self {
        Self {
            open: false,
            state: State::Empty,
            selected: None,
            loading: None,
            dialog: None,
        }
    }
}

/// Parse a result file's bytes; refuses any other schema.
pub fn parse_result(bytes: &[u8]) -> Result<MaintenanceResult, String> {
    let result: MaintenanceResult = serde_json::from_slice(bytes)
        .map_err(|e| format!("not a readable maintenance result: {e}"))?;
    if result.schema_version != MAINTENANCE_RESULT_VERSION {
        return Err(format!(
            "schema_version is {}, expected {MAINTENANCE_RESULT_VERSION}",
            result.schema_version
        ));
    }
    Ok(result)
}

fn read_result(path: &Path) -> LoadResult {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Ok(Loaded {
        file_name: path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into(),
        ),
        result: Box::new(parse_result(&bytes)?),
    })
}

impl MaintenancePanel {
    /// A panel already showing `result`, for tests and captures.
    #[cfg(test)]
    pub fn with_result(file_name: &str, result: MaintenanceResult) -> Self {
        let mut panel = Self::default();
        panel.install(Ok(Loaded {
            file_name: file_name.into(),
            result: Box::new(result),
        }));
        panel.open = true;
        panel
    }

    pub fn is_pending(&self) -> bool {
        self.loading.is_some() || self.dialog.is_some()
    }

    /// Read and parse `path` on a worker thread and open the window.
    pub fn load_in_background(&mut self, ctx: &egui::Context, path: PathBuf) {
        let (sender, receiver) = mpsc::channel();
        let context = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("faris-maintenance-load".into())
            .spawn(move || {
                let _ = sender.send(read_result(&path));
                context.request_repaint();
            });
        match spawned {
            Ok(_) => {
                self.loading = Some(receiver);
                self.open = true;
            }
            Err(error) => self.state = State::Failed(error.to_string()),
        }
    }

    fn pick_file(&mut self, ctx: &egui::Context) {
        if self.is_pending() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let context = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("faris-maintenance-dialog".into())
            .spawn(move || {
                let picked = rfd::FileDialog::new()
                    .add_filter("Maintenance result", &["json"])
                    .set_title("Open maintenance result")
                    .pick_file();
                let _ = sender.send(picked);
                context.request_repaint();
            });
        match spawned {
            Ok(_) => self.dialog = Some(receiver),
            Err(error) => self.state = State::Failed(format!("cannot open a file dialog: {error}")),
        }
    }

    fn install(&mut self, loaded: LoadResult) {
        match loaded {
            Ok(loaded) => {
                self.selected = loaded.result.designs.keys().next().cloned();
                self.state = State::Ready(loaded);
            }
            Err(error) => self.state = State::Failed(error),
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = &self.dialog {
            match receiver.try_recv() {
                Ok(picked) => {
                    self.dialog = None;
                    if let Some(path) = picked {
                        self.load_in_background(ctx, path);
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.dialog = None,
            }
        }
        if let Some(receiver) = &self.loading {
            match receiver.try_recv() {
                Ok(loaded) => {
                    self.loading = None;
                    self.install(loaded);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.loading = None;
                    self.state = State::Failed("The loader ended unexpectedly.".into());
                }
            }
        }
    }

    /// The floating window; does nothing while closed.
    pub fn window(&mut self, ctx: &egui::Context) {
        self.poll(ctx);
        if !self.open {
            return;
        }
        let mut open = self.open;
        egui::Window::new("Maintenance")
            .open(&mut open)
            .default_size([1180.0, 900.0])
            .vscroll(false)
            .show(ctx, |ui| self.view(ui));
        self.open = open;
    }

    /// The window's content.
    pub fn view(&mut self, ui: &mut egui::Ui) {
        let mut open_dialog = false;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.is_pending(), egui::Button::new("Open result\u{2026}"))
                .on_hover_text(
                    "Open a faris-maintenance-result/v0.1 file written by the maintenance command.",
                )
                .clicked()
            {
                open_dialog = true;
            }
            if self.loading.is_some() {
                ui.spinner();
                ui.weak("Reading the result\u{2026}");
            }
        });
        if open_dialog {
            self.pick_file(&ui.ctx().clone());
        }
        ui.separator();
        let selected = &mut self.selected;
        match &self.state {
            State::Empty => {
                ui.weak("No maintenance result is open.");
                ui.weak("The result file is written by the maintenance command and holds fixed and computed replacement durations for each design.");
            }
            State::Failed(error) => {
                ui.colored_label(
                    Kind::Failed.color(),
                    format!("Maintenance result not loaded: {error}"),
                );
            }
            State::Ready(loaded) => {
                egui::ScrollArea::both()
                    .id_salt("maintenance-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| body(ui, loaded, selected));
            }
        }
    }
}

// ----------------------------------------------------------------- formatting --

fn days(seconds: f64) -> f64 {
    seconds / DAY_S
}

fn years(seconds: f64) -> f64 {
    seconds / JULIAN_YEAR_SECONDS
}

/// Reason, then next step: the text every NOT_EVALUATED shows.
pub fn not_evaluated_text(ne: &NotEvaluated) -> String {
    format!(
        "{}. Next step: {}.",
        ne.reason.trim_end_matches('.'),
        ne.next_step.trim_end_matches('.')
    )
}

/// The governing components of a cooldown, largest share first.
pub fn why_text(why: &Why) -> String {
    let mut parts: Vec<_> = why.governing.iter().collect();
    parts.sort_by(|a, b| b.share.total_cmp(&a.share));
    parts
        .iter()
        .map(|g| {
            format!(
                "{} {:.0} % ({:.1} y in service)",
                g.component,
                g.share * 100.0,
                years(g.in_service_s)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn threshold_source(record: &ThresholdRecord) -> String {
    match (
        record.source.as_str(),
        &record.calibration_design,
        record.target_cooldown_s,
    ) {
        ("calibrated", Some(design), Some(target)) => {
            format!(
                "calibrated on {design} to a target of {:.1} days",
                days(target)
            )
        }
        (source, _, _) => source.to_string(),
    }
}

fn outcome_label(outcome: &HistoryOutcome) -> &'static str {
    match outcome {
        HistoryOutcome::HorizonCompleted => "horizon completed",
        HistoryOutcome::FuelLimitedAtHorizon => "fuel limited at horizon",
        HistoryOutcome::PermanentComponentLimit => "permanent component limit",
    }
}

fn quantity_label(quantity: GoverningQuantity) -> &'static str {
    match quantity {
        GoverningQuantity::Heat => "heat (decay heat per volume of the governing components)",
        GoverningQuantity::Dose => "dose",
    }
}

fn availability_text(summary: &HistorySummary) -> String {
    format!("{:.2}", summary.availability * 100.0)
}

/// Whole number with thin-space thousands grouping (1 200 000).
pub fn group_thousands(value: f64) -> String {
    let digits = format!("{:.0}", value.abs());
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push('\u{2009}');
        }
        grouped.push(c);
    }
    if value < 0.0 && digits != "0" {
        grouped.insert(0, '-');
    }
    grouped
}

fn net_text(summary: &HistorySummary) -> Option<String> {
    summary.lifetime_net_electricity_mwh.map(group_thousands)
}

/// Everything known about one event, for hover and the iteration list.
pub fn event_detail(event: &EventRecord) -> String {
    let mut lines = vec![
        format!(
            "{} \u{b7} {} \u{b7} replacement {} at {:.2} y",
            event.class,
            event.component,
            event.k,
            years(event.start_s)
        ),
        format!("fixed {:.1} d", days(event.fixed_duration_s)),
    ];
    match (event.duration_computed_s, event.cooldown_s) {
        (Some(total), Some(cool)) => lines.push(format!(
            "computed {:.1} d = cooldown {:.1} d + work {:.1} d",
            days(total),
            days(cool),
            days(event.work_s)
        )),
        _ => lines.push("computed: not evaluated".into()),
    }
    if event.window_limited {
        lines.push("Window-limited: the decay curve ended before reaching q*, so the cooldown is a lower bound.".into());
    }
    if let Some(why) = &event.why {
        lines.push(format!("Why: {}", why_text(why)));
    }
    if let Some(ne) = &event.not_evaluated {
        lines.push(format!("NOT_EVALUATED: {}", not_evaluated_text(ne)));
    }
    lines.join("\n")
}

// ----------------------------------------------------------------------- body --

fn final_events(design: &DesignResult) -> &[EventRecord] {
    design
        .computed
        .iterations
        .last()
        .map_or(&[], |i| i.events.as_slice())
}

fn ne_label(ui: &mut egui::Ui, ne: &NotEvaluated) {
    badge::kind_badge(ui, Kind::NotEvaluated, &not_evaluated_text(ne));
}

fn body(ui: &mut egui::Ui, loaded: &Loaded, selected: &mut Option<String>) {
    let result = &loaded.result;
    header(ui, loaded);
    ui.add_space(8.0);
    summary_table(ui, result);
    ui.add_space(4.0);
    ui.weak(COMPUTED_MEANING);
    for (name, design) in &result.designs {
        if let Some(ne) = &design.computed.not_evaluated {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ne_label(ui, ne);
                ui.label(format!("{name}: {}", not_evaluated_text(ne)));
            });
        }
    }
    ui.add_space(10.0);
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        ui.strong("Design");
        for name in result.designs.keys() {
            ui.selectable_value(selected, Some(name.clone()), name);
        }
    });
    if let Some((name, design)) = selected
        .as_ref()
        .and_then(|s| result.designs.get_key_value(s))
    {
        ui.add_space(6.0);
        outage_chart(ui, name, design);
        ui.add_space(8.0);
        event_table(ui, name, design);
    }
    ui.add_space(10.0);
    ui.separator();
    contrast_table(ui, &result.contrasts);
}

/// Each class's threshold record in every design that has one.
fn class_thresholds(result: &MaintenanceResult) -> Vec<(String, Vec<(&str, &ThresholdRecord)>)> {
    let mut classes: BTreeMap<String, Vec<(&str, &ThresholdRecord)>> = BTreeMap::new();
    for (name, design) in &result.designs {
        for (class, record) in &design.thresholds {
            classes
                .entry(class.clone())
                .or_default()
                .push((name.as_str(), record));
        }
    }
    classes.into_iter().collect()
}

fn thresholds_differ(records: &[(&str, &ThresholdRecord)]) -> bool {
    records.iter().any(|(_, r)| *r != records[0].1)
}

fn header(ui: &mut egui::Ui, loaded: &Loaded) {
    let result = &loaded.result;
    ui.horizontal_wrapped(|ui| {
        ui.strong(&loaded.file_name);
        ui.weak(&result.schema_version);
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Governing quantity:");
        ui.strong(quantity_label(result.assumptions.governing_quantity));
    });
    egui::Grid::new("maintenance-thresholds")
        .striped(true)
        .spacing([14.0, 3.0])
        .show(ui, |ui| {
            for caption in ["Class", "Replaced", "Work", "Threshold q*", "Source", ""] {
                ui.strong(caption);
            }
            ui.end_row();
            for (class, records) in class_thresholds(result) {
                let record = records[0].1;
                ui.label(&class);
                let spec = result.assumptions.classes.get(&class);
                ui.label(spec.map_or("\u{2014}".into(), |c| c.component_id.clone()))
                    .on_hover_text(spec.map_or(String::new(), |c| {
                        format!("Governing components: {}", c.governing.join(", "))
                    }));
                ui.label(spec.map_or("\u{2014}".into(), |c| format!("{:.1} d", days(c.work_s))));
                match (
                    &record.status,
                    record.q_star_w_per_m3,
                    &record.not_evaluated,
                ) {
                    (Status::Evaluated, Some(q), _) => {
                        ui.monospace(format!("{q:.3e} W/m\u{b3}"));
                    }
                    (_, _, Some(ne)) => ne_label(ui, ne),
                    _ => {
                        ui.weak("\u{2014}");
                    }
                }
                ui.label(threshold_source(record));
                if thresholds_differ(&records) {
                    let detail = records
                        .iter()
                        .map(|(design, r)| {
                            format!(
                                "{design}: {}",
                                r.q_star_w_per_m3
                                    .map_or("not evaluated".into(), |q| format!("{q:.3e} W/m\u{b3}"))
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    badge::badge(
                        ui,
                        Kind::Partial,
                        "designs differ",
                        &format!(
                            "The designs carry different thresholds for this class; a real result uses one threshold per class, so check the file. The first design's record is shown.\n{detail}"
                        ),
                    );
                }
                ui.end_row();
            }
        });
    egui::CollapsingHeader::new("Input hashes (SHA-256)")
        .id_salt("maintenance-hashes")
        .show(ui, |ui| {
            if result.inputs.is_empty() {
                ui.weak("The file records no input hashes.");
            }
            egui::Grid::new("maintenance-inputs").show(ui, |ui| {
                for (name, hash) in &result.inputs {
                    ui.label(name);
                    ui.monospace(hash);
                    ui.end_row();
                }
            });
        });
}

fn summary_table(ui: &mut egui::Ui, result: &MaintenanceResult) {
    ui.strong("Designs");
    egui::ScrollArea::horizontal()
        .id_salt("maintenance-summary-scroll")
        .show(ui, |ui| {
            egui::Grid::new("maintenance-summary")
                .striped(true)
                .spacing([14.0, 3.0])
                .show(ui, |ui| {
                    for caption in [
                        "Design",
                        "History",
                        "Downtime fixed (d)",
                        "Downtime computed (d)",
                        "Availability fixed (%)",
                        "Availability computed (%)",
                        "Net electricity fixed (MWh)",
                        "Net electricity computed (MWh)",
                        "Computed status",
                    ] {
                        ui.strong(caption);
                    }
                    ui.end_row();
                    for (name, design) in &result.designs {
                        summary_row(ui, name, design);
                        ui.end_row();
                    }
                });
        });
}

fn net_cell(ui: &mut egui::Ui, summary: &HistorySummary) {
    match net_text(summary) {
        Some(text) => {
            ui.monospace(text);
        }
        None => {
            ui.weak("unavailable").on_hover_text(NET_UNAVAILABLE);
        }
    }
}

fn summary_row(ui: &mut egui::Ui, name: &str, design: &DesignResult) {
    let fixed = &design.fixed;
    ui.strong(name);
    ui.label(format!("{:.1} y", years(fixed.history_end_s)))
        .on_hover_text(outcome_label(&fixed.outcome));
    ui.monospace(format!("{:.1}", days(fixed.total_replacement_downtime_s)));
    let computed = design.computed.summary.as_ref();
    match (design.computed.status, computed) {
        (Status::Evaluated, Some(c)) => {
            ui.monospace(format!("{:.1}", days(c.total_replacement_downtime_s)));
            ui.monospace(availability_text(fixed));
            ui.monospace(availability_text(c));
            net_cell(ui, fixed);
            net_cell(ui, c);
            badge::badge(
                ui,
                Kind::Calculated,
                "EVALUATED",
                &format!(
                    "Converged{} with the class thresholds recorded in this file.",
                    design
                        .computed
                        .converged_at_iteration
                        .map_or(String::new(), |i| format!(" at iteration {i}"))
                ),
            );
        }
        _ => {
            ui.weak("\u{2014}");
            ui.monospace(availability_text(fixed));
            ui.weak("\u{2014}");
            net_cell(ui, fixed);
            ui.weak("\u{2014}");
            match &design.computed.not_evaluated {
                Some(ne) => {
                    badge::badge(
                        ui,
                        Kind::NotEvaluated,
                        "NOT_EVALUATED",
                        &not_evaluated_text(ne),
                    );
                }
                None => {
                    badge::badge(
                        ui,
                        Kind::NotEvaluated,
                        "NOT_EVALUATED",
                        "The file gives no reason; rerun the maintenance command and check its log.",
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------- chart --

const FIXED_COLOR: egui::Color32 = egui::Color32::from_rgb(140, 152, 176);
const COOLDOWN_COLOR: egui::Color32 = egui::Color32::from_rgb(224, 140, 60);
const WORK_COLOR: egui::Color32 = egui::Color32::from_rgb(86, 156, 214);

fn legend_swatch(ui: &mut egui::Ui, color: egui::Color32, label: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 1.0, color);
    ui.weak(label);
}

/// Index of the event whose bar pair is nearest `x` within `reach` pixels.
fn nearest_event(xs: &[f32], x: f32, reach: f32) -> Option<usize> {
    xs.iter()
        .enumerate()
        .map(|(i, ex)| (i, (ex - x).abs()))
        .filter(|(_, d)| *d <= reach)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

fn outage_chart(ui: &mut egui::Ui, name: &str, design: &DesignResult) {
    let events = final_events(design);
    ui.strong(format!("Replacement outages \u{b7} {name}"));
    ui.horizontal_wrapped(|ui| {
        legend_swatch(ui, FIXED_COLOR, "fixed");
        legend_swatch(ui, COOLDOWN_COLOR, "computed: cooldown");
        legend_swatch(ui, WORK_COLOR, "computed: work");
        legend_swatch(ui, Kind::Partial.color(), "window-limited (lower bound)");
        legend_swatch(ui, Kind::NotEvaluated.color(), "not evaluated");
    });
    if events.is_empty() {
        ui.weak("This design's final iteration records no replacement events.");
        return;
    }
    let width = ui.available_width().clamp(480.0, 1400.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 220.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let weak = ui.visuals().weak_text_color();
    let grid = egui::Color32::from_white_alpha(18);
    let plot = egui::Rect::from_min_max(
        rect.min + egui::vec2(56.0, 14.0),
        rect.max - egui::vec2(14.0, 34.0),
    );
    let end_years = years(
        design
            .computed
            .summary
            .as_ref()
            .map_or(design.fixed.history_end_s, |s| s.history_end_s)
            .max(design.fixed.history_end_s),
    )
    .max(years(events.iter().map(|e| e.start_s).fold(0.0, f64::max)) * 1.02)
    .max(1e-9);
    let top_days = events
        .iter()
        .flat_map(|e| {
            [
                days(e.fixed_duration_s),
                e.duration_computed_s.map_or(0.0, days),
            ]
        })
        .fold(1.0, f64::max);
    let (ticks, _) = nice_ticks(0.0, top_days * 1.08, 5);
    let y_hi = ticks
        .last()
        .copied()
        .unwrap_or(top_days)
        .max(top_days * 1.02);
    let px = |t_years: f64| plot.left() + (t_years / end_years) as f32 * plot.width();
    let py = |d: f64| plot.bottom() - (d / y_hi) as f32 * plot.height();
    let font = egui::FontId::proportional(11.0);
    for tick in &ticks {
        let y = py(*tick);
        painter.line_segment(
            [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            egui::Stroke::new(1.0, grid),
        );
        painter.text(
            egui::pos2(plot.left() - 5.0, y),
            egui::Align2::RIGHT_CENTER,
            format!("{tick:.0}"),
            font.clone(),
            weak,
        );
    }
    let (xticks, _) = nice_ticks(0.0, end_years, 8);
    for tick in &xticks {
        let x = px(*tick);
        painter.line_segment(
            [
                egui::pos2(x, plot.bottom()),
                egui::pos2(x, plot.bottom() + 4.0),
            ],
            egui::Stroke::new(1.0, weak),
        );
        painter.text(
            egui::pos2(x, plot.bottom() + 6.0),
            egui::Align2::CENTER_TOP,
            format!("{tick:.0}"),
            font.clone(),
            weak,
        );
    }
    painter.text(
        egui::pos2(plot.center().x, rect.bottom() - 2.0),
        egui::Align2::CENTER_BOTTOM,
        "plant time at the start of the replacement (years)",
        font.clone(),
        weak,
    );
    painter.text(
        egui::pos2(rect.left() + 2.0, plot.top() - 2.0),
        egui::Align2::LEFT_BOTTOM,
        "outage (days)",
        font.clone(),
        weak,
    );
    painter.rect_stroke(
        plot,
        0.0,
        egui::Stroke::new(1.0, grid),
        egui::StrokeKind::Inside,
    );
    let bar = (plot.width() / (events.len() as f32 * 2.6)).clamp(3.0, 9.0);
    let mut xs = Vec::with_capacity(events.len());
    for event in events {
        let x = px(years(event.start_s));
        xs.push(x);
        let fixed = egui::Rect::from_min_max(
            egui::pos2(x - bar, py(days(event.fixed_duration_s))),
            egui::pos2(x, plot.bottom()),
        );
        painter.rect_filled(fixed, 0.0, FIXED_COLOR);
        if let (Some(total), Some(_)) = (event.duration_computed_s, event.cooldown_s) {
            let work_top = py(days(event.work_s));
            let total_top = py(days(total));
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x, work_top),
                    egui::pos2(x + bar, plot.bottom()),
                ),
                0.0,
                WORK_COLOR,
            );
            painter.rect_filled(
                egui::Rect::from_min_max(egui::pos2(x, total_top), egui::pos2(x + bar, work_top)),
                0.0,
                COOLDOWN_COLOR,
            );
            if event.window_limited {
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        egui::pos2(x + bar * 0.5, total_top - 8.0),
                        egui::pos2(x + bar * 0.5 + 4.0, total_top - 2.0),
                        egui::pos2(x + bar * 0.5 - 4.0, total_top - 2.0),
                    ],
                    Kind::Partial.color(),
                    egui::Stroke::NONE,
                ));
            }
        }
        if event.not_evaluated.is_some() {
            let color = Kind::NotEvaluated.color();
            painter.line_segment(
                [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
                egui::Stroke::new(1.5, color),
            );
            painter.text(
                egui::pos2(x - 4.0, plot.top() + 2.0),
                if x > plot.center().x {
                    egui::Align2::RIGHT_TOP
                } else {
                    egui::Align2::LEFT_TOP
                },
                "NOT_EVALUATED",
                font.clone(),
                color,
            );
        }
    }
    if let Some(pos) = response.hover_pos()
        && plot.contains(pos)
        && let Some(i) = nearest_event(&xs, pos.x, bar * 2.0 + 6.0)
    {
        response.on_hover_ui_at_pointer(|ui| {
            ui.set_max_width(420.0);
            ui.label(event_detail(&events[i]));
        });
    }
}

// ------------------------------------------------------------- event table --

fn event_table(ui: &mut egui::Ui, name: &str, design: &DesignResult) {
    let events = final_events(design);
    ui.strong(format!("Replacements \u{b7} {name}"));
    let computed = &design.computed;
    ui.weak(match (computed.status, computed.converged_at_iteration) {
        (Status::Evaluated, Some(i)) => format!(
            "Final iteration of {}; the durations stopped changing at iteration {i}.",
            computed.iterations.len()
        ),
        _ => format!(
            "Last of {} iteration(s); the computation did not complete, see the reason above.",
            computed.iterations.len()
        ),
    });
    egui::ScrollArea::horizontal()
        .id_salt("maintenance-events-scroll")
        .show(ui, |ui| {
            egui::Grid::new(("maintenance-events", name))
                .striped(true)
                .spacing([12.0, 3.0])
                .show(ui, |ui| {
                    for caption in [
                        "Class",
                        "Component",
                        "k",
                        "Start (y)",
                        "Fixed (d)",
                        "Computed (d)",
                        "Cooldown (d)",
                        "Work (d)",
                        "Why",
                    ] {
                        ui.strong(caption);
                    }
                    ui.end_row();
                    for event in events {
                        event_row(ui, event);
                        ui.end_row();
                    }
                });
        });
    if !computed.iterations.is_empty() {
        egui::CollapsingHeader::new(format!("All iterations ({})", computed.iterations.len()))
            .id_salt(("maintenance-iterations", name))
            .show(ui, |ui| iteration_list(ui, computed));
    }
}

fn event_row(ui: &mut egui::Ui, event: &EventRecord) {
    ui.label(&event.class);
    ui.label(&event.component);
    ui.monospace(event.k.to_string());
    ui.monospace(format!("{:.2}", years(event.start_s)));
    ui.monospace(format!("{:.1}", days(event.fixed_duration_s)));
    match (
        &event.not_evaluated,
        event.duration_computed_s,
        event.cooldown_s,
    ) {
        (Some(ne), _, _) => {
            badge::badge(
                ui,
                Kind::NotEvaluated,
                "NOT_EVALUATED",
                &not_evaluated_text(ne),
            );
            ui.weak("\u{2014}");
            ui.monospace(format!("{:.1}", days(event.work_s)));
            ui.horizontal_wrapped(|ui| ui.label(not_evaluated_text(ne)));
        }
        (None, Some(total), Some(cool)) => {
            ui.horizontal(|ui| {
                ui.monospace(format!("{:.1}", days(total)));
                if event.window_limited {
                    badge::badge(
                        ui,
                        Kind::Partial,
                        "window-limited",
                        "The decay curve ended before reaching q*, so this cooldown is a lower bound.",
                    );
                }
            });
            ui.monospace(format!("{:.1}", days(cool)));
            ui.monospace(format!("{:.1}", days(event.work_s)));
            match &event.why {
                Some(why) => {
                    ui.label(why_text(why)).on_hover_text(format!(
                        "Shares of the governing components' decay heat at the end of the cooldown; the curve started at {:.2e} W/m\u{b3}.",
                        why.curve_start_w_per_m3
                    ));
                }
                None => {
                    ui.weak("\u{2014}");
                }
            }
        }
        _ => {
            ui.weak("\u{2014}");
            ui.weak("\u{2014}");
            ui.monospace(format!("{:.1}", days(event.work_s)));
            ui.weak("no breakdown recorded");
        }
    }
}

fn iteration_list(ui: &mut egui::Ui, computed: &ComputedResult) {
    egui::Grid::new("maintenance-iteration-grid")
        .striped(true)
        .spacing([12.0, 3.0])
        .show(ui, |ui| {
            for caption in [
                "Iteration",
                "Max change (d)",
                "Window-limited",
                "Durations in the next iteration (d), by class",
            ] {
                ui.strong(caption);
            }
            ui.end_row();
            for iteration in &computed.iterations {
                ui.monospace(iteration.iteration.to_string());
                ui.monospace(format!("{:.2}", days(iteration.max_change_s)));
                ui.label(if iteration.window_limited {
                    "yes"
                } else {
                    "no"
                });
                let text = iteration
                    .durations_out_s
                    .iter()
                    .map(|(class, values)| {
                        format!(
                            "{class}: {}",
                            values
                                .iter()
                                .map(|s| format!("{:.1}", days(*s)))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                ui.label(text);
                ui.end_row();
            }
        });
}

// -------------------------------------------------------------- contrasts --

fn contrast_table(ui: &mut egui::Ui, contrasts: &[Contrast]) {
    ui.strong("Contrasts between designs");
    ui.weak("Difference in total replacement downtime, first design minus second, under each duration model.");
    if contrasts.is_empty() {
        ui.weak("The file holds one design, so there is nothing to contrast.");
        return;
    }
    egui::Grid::new("maintenance-contrasts")
        .striped(true)
        .spacing([14.0, 3.0])
        .show(ui, |ui| {
            for caption in [
                "Designs",
                "Fixed difference (d)",
                "Computed difference (d)",
                "Ratio computed / fixed",
            ] {
                ui.strong(caption);
            }
            ui.end_row();
            for contrast in contrasts {
                ui.label(format!("{} \u{2212} {}", contrast.a, contrast.b));
                ui.monospace(format!("{:+.1}", days(contrast.fixed_difference_s)));
                match (
                    contrast.computed_difference_s,
                    contrast.ratio_computed_over_fixed,
                ) {
                    (Some(diff), ratio) => {
                        ui.monospace(format!("{:+.1}", days(diff)));
                        match ratio {
                            Some(r) => {
                                ui.monospace(format!("{r:.2}\u{d7}"));
                            }
                            None => {
                                ui.weak("\u{2014}").on_hover_text(
                                    "The ratio is undefined when the fixed difference is zero.",
                                );
                            }
                        }
                    }
                    (None, _) => {
                        ui.weak("\u{2014}");
                        match &contrast.not_evaluated {
                            Some(ne) => {
                                ui.horizontal_wrapped(|ui| {
                                    ne_label(ui, ne);
                                    ui.label(not_evaluated_text(ne));
                                });
                            }
                            None => {
                                ui.weak("\u{2014}");
                            }
                        }
                    }
                }
                ui.end_row();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use faris_engine::{
        history::HistoryOutcome,
        maintenance::{GoverningShare, IterationRecord, ReplacementSummary},
    };
    use faris_model::maintenance::{
        CoolingGrid, MAINTENANCE_ASSUMPTIONS_VERSION, MaintenanceAssumptions, MaintenanceClass,
        Threshold,
    };

    const Y: f64 = JULIAN_YEAR_SECONDS;

    fn summary(downtime_d: f64, end_y: f64, net: Option<f64>) -> HistorySummary {
        HistorySummary {
            outcome: HistoryOutcome::HorizonCompleted,
            history_end_s: end_y * Y,
            lifetime_net_electricity_mwh: net,
            total_replacement_downtime_s: downtime_d * DAY_S,
            availability: 1.0 - downtime_d * DAY_S / (end_y * Y),
            replacements: (1..=3)
                .map(|k| ReplacementSummary {
                    class: "blanket".into(),
                    component: "blanket".into(),
                    k,
                    start_s: f64::from(k) * 3.5 * Y,
                    end_s: Some(f64::from(k) * 3.5 * Y + 30.0 * DAY_S),
                })
                .collect(),
        }
    }

    fn event(k: u32, computed_d: Option<f64>, window_limited: bool) -> EventRecord {
        let start = f64::from(k) * 3.5 * Y;
        // The first wall is never replaced, so it has been in service since the start; the
        // blanket has run one interval (or since the start, at the first replacement).
        let first_wall_share = 0.60 + 0.11 * f64::from(k);
        EventRecord {
            class: "blanket".into(),
            component: "blanket".into(),
            k,
            start_s: start,
            end_s: Some(start + 30.0 * DAY_S),
            fixed_duration_s: 30.0 * DAY_S,
            duration_used_s: 30.0 * DAY_S,
            cooldown_s: computed_d.map(|d| (d - 10.0) * DAY_S),
            work_s: 10.0 * DAY_S,
            duration_computed_s: computed_d.map(|d| d * DAY_S),
            window_limited,
            q_star: 1.5e3,
            why: computed_d.map(|_| Why {
                governing: vec![
                    GoverningShare {
                        component: "blanket".into(),
                        share: 1.0 - first_wall_share,
                        in_service_s: 3.5 * Y,
                    },
                    GoverningShare {
                        component: "first-wall".into(),
                        share: first_wall_share,
                        in_service_s: start,
                    },
                ],
                curve_start_s: 3600.0,
                curve_start_w_per_m3: 4.0e4,
            }),
            not_evaluated: computed_d.is_none().then(|| NotEvaluated {
                reason: "decay curve never falls below q* within 365 days".into(),
                next_step: "raise q*, lengthen the cooling grid".into(),
            }),
        }
    }

    fn iteration(n: u32, events: Vec<EventRecord>) -> IterationRecord {
        let out: Vec<f64> = events
            .iter()
            .map(|e| e.duration_computed_s.unwrap_or(e.fixed_duration_s))
            .collect();
        IterationRecord {
            iteration: n,
            durations_in_s: BTreeMap::from([("blanket".into(), vec![30.0 * DAY_S; out.len()])]),
            durations_out_s: BTreeMap::from([("blanket".into(), out)]),
            max_change_s: 12.0 * DAY_S / f64::from(n),
            window_limited: events.iter().any(|e| e.window_limited),
            events,
        }
    }

    fn threshold(source: &str) -> ThresholdRecord {
        ThresholdRecord {
            status: Status::Evaluated,
            q_star_w_per_m3: Some(1.5e3),
            source: source.into(),
            calibration_design: (source == "calibrated").then(|| "X".to_string()),
            target_cooldown_s: (source == "calibrated").then_some(20.0 * DAY_S),
            calibration_event_start_s: None,
            not_evaluated: None,
        }
    }

    fn fixture() -> MaintenanceResult {
        let evaluated = DesignResult {
            fixed: summary(90.0, 30.0, Some(1.2e6)),
            computed: ComputedResult {
                status: Status::Evaluated,
                not_evaluated: None,
                summary: Some(summary(210.0, 30.0, Some(1.05e6))),
                converged_at_iteration: Some(2),
                iterations: vec![
                    iteration(
                        1,
                        vec![
                            event(1, Some(35.0), false),
                            event(2, Some(55.0), false),
                            event(3, Some(95.0), true),
                        ],
                    ),
                    iteration(
                        2,
                        vec![
                            event(1, Some(36.0), false),
                            event(2, Some(58.0), false),
                            event(3, Some(96.0), false),
                        ],
                    ),
                ],
            },
            thresholds: BTreeMap::from([("blanket".into(), threshold("calibrated"))]),
        };
        let failing = DesignResult {
            fixed: summary(90.0, 30.0, Some(1.1e6)),
            computed: ComputedResult {
                status: Status::NotEvaluated,
                not_evaluated: Some(NotEvaluated {
                    reason: "decay curve never falls below q* within 365 days".into(),
                    next_step: "raise q*, lengthen the cooling grid".into(),
                }),
                summary: None,
                converged_at_iteration: None,
                iterations: vec![iteration(
                    1,
                    vec![event(1, Some(40.0), false), event(2, None, false)],
                )],
            },
            thresholds: BTreeMap::from([("blanket".into(), threshold("calibrated"))]),
        };
        MaintenanceResult {
            schema_version: MAINTENANCE_RESULT_VERSION.into(),
            inputs: BTreeMap::from([("assumptions".into(), "ab".repeat(32))]),
            assumptions: MaintenanceAssumptions {
                schema_version: MAINTENANCE_ASSUMPTIONS_VERSION.into(),
                governing_quantity: GoverningQuantity::Heat,
                classes: BTreeMap::from([(
                    "blanket".into(),
                    MaintenanceClass {
                        component_id: "blanket".into(),
                        governing: vec!["first-wall".into(), "blanket".into()],
                        work_s: 10.0 * DAY_S,
                        threshold: Threshold::QStar {
                            q_star_w_per_m3: 1.5e3,
                        },
                    },
                )]),
                cooling: CoolingGrid::default(),
                max_iterations: 10,
                convergence_s: DAY_S,
            },
            designs: BTreeMap::from([("X".into(), evaluated), ("Y".into(), failing)]),
            contrasts: vec![
                Contrast {
                    a: "X".into(),
                    b: "Y".into(),
                    fixed_difference_s: 0.0,
                    computed_difference_s: Some(40.0 * DAY_S),
                    ratio_computed_over_fixed: None,
                    not_evaluated: None,
                },
                Contrast {
                    a: "Y".into(),
                    b: "X".into(),
                    fixed_difference_s: 10.0 * DAY_S,
                    computed_difference_s: None,
                    ratio_computed_over_fixed: None,
                    not_evaluated: Some(NotEvaluated {
                        reason: "design Y has no computed downtime".into(),
                        next_step: "resolve the design Y reason above".into(),
                    }),
                },
                Contrast {
                    a: "X".into(),
                    b: "Y".into(),
                    fixed_difference_s: 20.0 * DAY_S,
                    computed_difference_s: Some(60.0 * DAY_S),
                    ratio_computed_over_fixed: Some(3.0),
                    not_evaluated: None,
                },
            ],
        }
    }

    /// Writes the fixture as a result file when `FARIS_MAINTENANCE_FIXTURE_OUT`
    /// names a path, so the desktop can open it for a screenshot:
    /// `--maintenance <that file> --capture maintenance.png`.
    #[test]
    fn writes_the_fixture_file_on_request() {
        let Some(path) = std::env::var_os("FARIS_MAINTENANCE_FIXTURE_OUT") else {
            return;
        };
        let json = serde_json::to_vec_pretty(&fixture()).unwrap();
        std::fs::write(path, json).unwrap();
    }

    #[test]
    fn a_result_round_trips_through_json_and_parses() {
        let original = fixture();
        let bytes = serde_json::to_vec(&original).unwrap();
        assert_eq!(parse_result(&bytes).unwrap(), original);
    }

    #[test]
    fn another_schema_or_garbage_is_refused() {
        let mut other = fixture();
        other.schema_version = "faris-maintenance-result/v9".into();
        let bytes = serde_json::to_vec(&other).unwrap();
        assert!(parse_result(&bytes).unwrap_err().contains("expected"));
        assert!(parse_result(b"{}").is_err());
    }

    #[test]
    fn why_lists_components_by_share_with_years_in_service() {
        let result = fixture();
        let events = final_events(&result.designs["X"]);
        assert_eq!(
            why_text(events[1].why.as_ref().unwrap()),
            "first-wall 82 % (7.0 y in service), blanket 18 % (3.5 y in service)"
        );
        // The first wall's share and years in service grow from event to event.
        let first_wall = |i: usize| {
            events[i]
                .why
                .as_ref()
                .unwrap()
                .governing
                .iter()
                .find(|g| g.component == "first-wall")
                .map(|g| (g.share, g.in_service_s))
                .unwrap()
        };
        assert!(first_wall(0).0 < first_wall(1).0 && first_wall(1).0 < first_wall(2).0);
        assert!(first_wall(0).1 < first_wall(1).1 && first_wall(1).1 < first_wall(2).1);
    }

    #[test]
    fn not_evaluated_text_has_reason_and_next_step() {
        let result = fixture();
        let design = &result.designs["Y"];
        let text = not_evaluated_text(design.computed.not_evaluated.as_ref().unwrap());
        assert!(text.contains("never falls below q*"), "{text}");
        assert!(text.contains("Next step: raise q*"), "{text}");
        let failing = final_events(design).last().unwrap();
        let detail = event_detail(failing);
        assert!(
            detail.contains("NOT_EVALUATED") && detail.contains("Next step"),
            "{detail}"
        );
    }

    #[test]
    fn event_detail_flags_window_limited_cooldowns() {
        let result = fixture();
        let first = &result.designs["X"].computed.iterations[0];
        let detail = event_detail(&first.events[2]);
        assert!(
            detail.contains("Window-limited") && detail.contains("cooldown"),
            "{detail}"
        );
    }

    #[test]
    fn an_evaluated_fixture_has_no_window_limited_final_event_and_equal_thresholds() {
        let result = fixture();
        let x = &result.designs["X"];
        assert!(final_events(x).iter().all(|e| !e.window_limited));
        assert!(
            x.computed.iterations[0]
                .events
                .iter()
                .any(|e| e.window_limited)
        );
        assert!(
            x.computed
                .summary
                .as_ref()
                .unwrap()
                .lifetime_net_electricity_mwh
                .is_some()
        );
        let classes = class_thresholds(&result);
        assert_eq!(classes.len(), 1);
        assert!(!thresholds_differ(&classes[0].1));
        let mut other = result.clone();
        other
            .designs
            .get_mut("Y")
            .unwrap()
            .thresholds
            .get_mut("blanket")
            .unwrap()
            .q_star_w_per_m3 = Some(9.0);
        assert!(thresholds_differ(&class_thresholds(&other)[0].1));
    }

    #[test]
    fn large_numbers_group_in_thousands() {
        assert_eq!(group_thousands(1_200_000.0), "1\u{2009}200\u{2009}000");
        assert_eq!(group_thousands(999.4), "999");
        assert_eq!(group_thousands(-12_345.0), "-12\u{2009}345");
        assert_eq!(group_thousands(0.0), "0");
    }

    #[test]
    fn nearest_event_picks_the_closest_within_reach() {
        let xs = [10.0, 30.0, 33.0];
        assert_eq!(nearest_event(&xs, 31.0, 5.0), Some(1));
        assert_eq!(nearest_event(&xs, 32.0, 5.0), Some(2));
        assert_eq!(nearest_event(&xs, 100.0, 5.0), None);
    }

    #[test]
    fn threshold_source_names_the_calibration() {
        assert_eq!(
            threshold_source(&threshold("calibrated")),
            "calibrated on X to a target of 20.0 days"
        );
        assert_eq!(threshold_source(&threshold("explicit")), "explicit");
    }

    #[test]
    fn the_panel_renders_both_designs_without_panic() {
        let ctx = egui::Context::default();
        let mut panel = MaintenancePanel::with_result("fixture.json", fixture());
        for design in ["X", "Y"] {
            panel.selected = Some(design.into());
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 900.0),
                )),
                ..Default::default()
            };
            for _ in 0..2 {
                ctx.run_ui(input.clone(), |ui| panel.view(ui))
                    .drop_without_applying_deltas();
            }
        }
        let empty = MaintenancePanel::default();
        assert!(!empty.is_pending());
    }

    #[test]
    fn explanatory_text_makes_no_ranking_claim() {
        let lowered = COMPUTED_MEANING.to_lowercase();
        assert!(lowered.contains("no ranking claim"));
        for word in ["winner", "best", "rank higher"] {
            assert!(!lowered.contains(word), "{word}");
        }
    }
}
