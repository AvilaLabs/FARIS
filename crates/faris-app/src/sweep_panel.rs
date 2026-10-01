//! Allocation-sweep view: blanket/shield thickness against real transport runs.
//!
//! The panel only draws and schedules. Point extraction, history summaries and
//! the plain-language findings are computed by `faris_engine::sweep`; histories
//! run in one background thread and are recomputed (debounced, cancellable)
//! when the selected operating assumptions change.

use crate::{
    badge::{self, Kind},
    transport_panel::TransportPanel,
};
use eframe::egui;
use faris_engine::{
    DemoManifest, build_manifest,
    history::{TransportDrivingRates, run_operating_history_cancellable},
    jobs::Cancellation,
    sweep::{
        Estimate, HistorySummary, TransportPoint, history_findings, summarize_history,
        transport_findings, transport_point,
    },
};
use faris_model::{LoadedScenario, history::OperatingHistoryAssumptions};
use std::{
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};

const DEBOUNCE: Duration = Duration::from_millis(400);
const MAGNET_COMPONENT: &str = "magnets";
/// Named arrangements of the demo, matched by blanket thickness.
const NAMED: [(f64, &str); 2] = [(0.45, "Reference"), (0.55, "Breeder-heavy")];

struct Pending {
    cancellation: Cancellation,
    receiver: Receiver<Result<Vec<HistorySummary>, String>>,
    key: String,
}

type Loaded = Result<(TransportPanel, LoadedScenario), String>;

pub struct SweepPanel {
    /// Bundles are validated off the UI thread; `Some` until the load ends.
    loading: Option<Receiver<Loaded>>,
    /// Keeps the materialized replay directories alive for the session.
    _transport: Option<TransportPanel>,
    manifest: Option<DemoManifest>,
    load_error: Option<String>,
    points: Vec<TransportPoint>,
    rates: Vec<TransportDrivingRates>,
    /// Aligned with `points`; empty while histories are not current.
    summaries: Vec<Option<HistorySummary>>,
    computed_key: Option<String>,
    seen_key: Option<(String, Instant)>,
    pending: Option<Pending>,
    history_error: Option<String>,
    selected: usize,
    /// Saved selection (blanket thickness, m), applied when the points load.
    wanted_blanket_m: Option<f64>,
}

/// Axis mapping a data range to a 0..1 fraction (linear or base-10 log).
#[derive(Clone, Copy, Debug)]
struct Axis {
    lo: f64,
    hi: f64,
    log: bool,
}

impl Axis {
    fn frac(&self, v: f64) -> f32 {
        let f = if self.log {
            (v.max(self.lo).log10() - self.lo.log10()) / (self.hi.log10() - self.lo.log10())
        } else {
            (v - self.lo) / (self.hi - self.lo)
        };
        f.clamp(0.0, 1.0) as f32
    }
}

/// Round tick values covering `lo..=hi` with about `target` divisions.
fn nice_ticks(lo: f64, hi: f64, target: usize) -> (Vec<f64>, f64) {
    let raw = (hi - lo) / target.max(1) as f64;
    if !(raw.is_finite() && raw > 0.0) {
        return (vec![lo], 1.0);
    }
    let magnitude = 10f64.powf(raw.log10().floor());
    let norm = raw / magnitude;
    let step = magnitude
        * if norm < 1.5 {
            1.0
        } else if norm < 3.0 {
            2.0
        } else if norm < 7.0 {
            5.0
        } else {
            10.0
        };
    let mut ticks = Vec::new();
    let mut v = (lo / step).ceil() * step;
    while v <= hi + step * 1e-9 {
        ticks.push(v);
        v += step;
    }
    (ticks, step)
}

/// 1-2-5 ticks inside a log range; denser when the range spans under a decade.
fn log_ticks(lo: f64, hi: f64) -> Vec<f64> {
    let collect = |mantissas: &[f64]| {
        let mut ticks = Vec::new();
        for k in lo.log10().floor() as i32..=hi.log10().ceil() as i32 {
            for m in mantissas {
                let v = m * 10f64.powi(k);
                if v >= lo * (1.0 - 1e-9) && v <= hi * (1.0 + 1e-9) {
                    ticks.push(v);
                }
            }
        }
        ticks
    };
    let ticks = collect(&[1.0, 2.0, 5.0]);
    if ticks.len() >= 3 {
        ticks
    } else {
        collect(&[1.0, 1.5, 2.0, 3.0, 4.0, 5.0, 7.0])
    }
}

fn decimals(step: f64) -> usize {
    (-step.log10().floor()).clamp(0.0, 6.0) as usize
}

impl SweepPanel {
    /// `loaded` carries the sweep's transport records and the scenario they
    /// were validated against; any error is shown in the view, never fatal.
    pub fn new(loaded: Result<(TransportPanel, LoadedScenario), String>) -> Self {
        let mut panel = Self {
            loading: None,
            _transport: None,
            manifest: None,
            load_error: None,
            points: Vec::new(),
            rates: Vec::new(),
            summaries: Vec::new(),
            computed_key: None,
            seen_key: None,
            pending: None,
            history_error: None,
            selected: 0,
            wanted_blanket_m: None,
        };
        panel.install(loaded);
        panel
    }

    /// Validate the sweep's bundles on a worker thread; the view shows progress.
    pub fn load_in_background(
        ctx: &egui::Context,
        loader: impl FnOnce() -> Loaded + Send + 'static,
    ) -> Self {
        let mut panel = Self::new(Err(String::new()));
        panel.load_error = None;
        let (sender, receiver) = mpsc::channel();
        let context = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("faris-sweep-load".into())
            .spawn(move || {
                let _ = sender.send(loader());
                context.request_repaint();
            });
        match spawned {
            Ok(_) => panel.loading = Some(receiver),
            Err(error) => panel.load_error = Some(error.to_string()),
        }
        panel
    }

    fn install(&mut self, loaded: Loaded) {
        let panel = self;
        match loaded.and_then(|(transport, scenario)| {
            let manifest = build_manifest(&scenario).map_err(|e| e.to_string())?;
            let (points, rates) = collect_points(&transport, &scenario, &manifest)?;
            Ok((transport, manifest, points, rates))
        }) {
            Ok((transport, manifest, points, rates)) => {
                let wanted = panel.wanted_blanket_m.unwrap_or(NAMED[0].0);
                panel.selected = points
                    .iter()
                    .position(|p| (p.blanket_m - wanted).abs() < 1e-9)
                    .or_else(|| {
                        points
                            .iter()
                            .position(|p| (p.blanket_m - NAMED[0].0).abs() < 1e-9)
                    })
                    .unwrap_or(0);
                panel._transport = Some(transport);
                panel.manifest = Some(manifest);
                panel.points = points;
                panel.rates = rates;
            }
            Err(error) => panel.load_error = Some(error),
        }
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some() || self.loading.is_some()
    }

    /// Blanket thickness (m) of the selected allocation, once the points load;
    /// before that, the saved selection still waiting to be applied.
    pub fn selected_blanket_m(&self) -> Option<f64> {
        self.points
            .get(self.selected)
            .map(|p| p.blanket_m)
            .or(self.wanted_blanket_m)
    }

    /// Select the allocation with this blanket thickness, now or when loaded.
    pub fn select_blanket_m(&mut self, blanket_m: f64) {
        self.wanted_blanket_m = Some(blanket_m);
        if let Some(index) = self
            .points
            .iter()
            .position(|p| (p.blanket_m - blanket_m).abs() < 1e-9)
        {
            self.selected = index;
        }
    }

    /// Poll the history worker and schedule a recomputation when the selected
    /// assumptions changed (debounced; an in-flight run is cancelled).
    pub fn update(
        &mut self,
        ctx: &egui::Context,
        assumptions: Option<&OperatingHistoryAssumptions>,
    ) {
        if let Some(receiver) = &self.loading {
            match receiver.try_recv() {
                Ok(loaded) => {
                    self.loading = None;
                    self.install(loaded);
                    ctx.request_repaint();
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.loading = None;
                    self.load_error = Some("The sweep loader ended unexpectedly.".into());
                }
            }
        }
        self.poll(ctx);
        let Some(assumptions) = assumptions else {
            return;
        };
        if self.points.is_empty() {
            return;
        }
        let Ok(key) = serde_json::to_string(assumptions) else {
            return;
        };
        if self.computed_key.as_deref() == Some(&key)
            || self.pending.as_ref().is_some_and(|p| p.key == key)
        {
            self.seen_key = None;
            return;
        }
        let now = Instant::now();
        let first_run = self.computed_key.is_none() && self.pending.is_none();
        let since = match &self.seen_key {
            Some((seen, at)) if *seen == key => *at,
            _ => {
                self.seen_key = Some((key.clone(), now));
                now
            }
        };
        if !first_run && now.duration_since(since) < DEBOUNCE {
            ctx.request_repaint_after(DEBOUNCE);
            return;
        }
        self.seen_key = None;
        if let Some(old) = self.pending.take() {
            old.cancellation.cancel();
        }
        self.summaries.clear();
        self.history_error = None;
        let cancellation = Cancellation::default();
        let worker_cancellation = cancellation.clone();
        let (sender, receiver) = mpsc::channel();
        let context = ctx.clone();
        let assumptions = assumptions.clone();
        let rates = self.rates.clone();
        let spawn = std::thread::Builder::new()
            .name("faris-sweep-histories".into())
            .spawn(move || {
                let result = rates
                    .iter()
                    .map(|rates| {
                        run_operating_history_cancellable(&assumptions, rates, &worker_cancellation)
                            .map(|history| summarize_history(&history, MAGNET_COMPONENT))
                    })
                    .collect();
                let _ = sender.send(result);
                context.request_repaint();
            });
        match spawn {
            Ok(_) => {
                self.pending = Some(Pending {
                    cancellation,
                    receiver,
                    key,
                })
            }
            Err(error) => self.history_error = Some(error.to_string()),
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.pending else {
            return;
        };
        let result = match pending.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("History worker ended unexpectedly.".into()),
        };
        let pending = self.pending.take().expect("worker exists");
        match result {
            Ok(summaries) => {
                self.summaries = summaries.into_iter().map(Some).collect();
                self.computed_key = Some(pending.key);
            }
            Err(error) => self.history_error = Some(error),
        }
        ctx.request_repaint();
    }

    fn histories_ready(&self) -> bool {
        self.summaries.len() == self.points.len() && !self.summaries.is_empty()
    }

    /// `preset_label` names the operating assumptions the history statements
    /// are conditional on.
    pub fn view(&mut self, ui: &mut egui::Ui, preset_label: &str) {
        if self.loading.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("Loading allocation sweep bundles\u{2026}");
            });
            return;
        }
        let title = format!(
            "Allocation sweep \u{b7} {} real transport runs",
            self.points.len()
        );
        egui::CollapsingHeader::new(egui::RichText::new(title).strong())
            .id_salt("allocation-sweep")
            .default_open(true)
            .show(ui, |ui| self.body(ui, preset_label));
    }

    fn body(&mut self, ui: &mut egui::Ui, preset_label: &str) {
        if let Some(error) = &self.load_error {
            ui.colored_label(
                Kind::Failed.color(),
                format!("Allocation sweep not loaded: {error}"),
            );
            return;
        }
        ui.horizontal_wrapped(|ui| {
            badge::kind_badge(
                ui,
                Kind::Calculated,
                "Breeding and magnet flux are recorded OpenMC transport results (cold-data numerical reference). Bars are sampling error only; geometry, data and model uncertainty are not included. Each allocation is an independent run with its own seed.",
            );
            ui.weak("Same radial envelope; thickness moves from shield to blanket. Independent runs, sampling error only.");
        });
        let n = self.points.len();
        ui.horizontal(|ui| {
            ui.label("Allocation");
            ui.add(
                egui::Slider::new(&mut self.selected, 0..=n - 1)
                    .show_value(false)
                    .smart_aim(false),
            );
            let point = &self.points[self.selected];
            ui.monospace(format!(
                "blanket {:.2} m / shield {:.2} m",
                point.blanket_m, point.shield_m
            ));
            if let Some(name) = named(point.blanket_m) {
                ui.strong(name);
            }
        });
        self.radial_bar(ui);
        ui.add_space(4.0);
        let charts_state = ChartData {
            points: &self.points,
            summaries: if self.histories_ready() {
                Some(&self.summaries)
            } else {
                None
            },
            selected: self.selected,
            pending: self.pending.is_some(),
            preset_label,
        };
        let spacing = ui.spacing().item_spacing.x;
        let width = ((ui.available_width() - 2.0 * spacing) / 3.0).max(220.0);
        let mut clicked = None;
        ui.horizontal_top(|ui| {
            for chart in [Chart::Breeding, Chart::MagnetFlux, Chart::Histories] {
                ui.vertical(|ui| {
                    ui.set_width(width);
                    if let Some(i) = draw_chart(ui, chart, &charts_state, width, 230.0) {
                        clicked = Some(i);
                    }
                });
            }
        });
        if let Some(i) = clicked {
            self.selected = i;
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            badge::badge(
                ui,
                Kind::Conditional,
                &format!("conditional on {preset_label}"),
                "Replacements and electricity come from the Rust operating history driven by each run's component fluxes under the currently selected authored operating assumptions (service limits, outages, efficiency). They are not qualified predictions and the transport uncertainty is not propagated.",
            );
            if self.pending.is_some() {
                ui.spinner();
                ui.weak("Calculating histories\u{2026}");
            }
        });
        if let Some(error) = &self.history_error {
            ui.colored_label(Kind::Failed.color(), error);
        }
        ui.add_space(4.0);
        ui.strong("What the sweep shows");
        for text in transport_findings(&self.points) {
            ui.horizontal_wrapped(|ui| {
                badge::kind_badge(ui, Kind::Calculated, "Statement about recorded transport results with Monte Carlo sampling error only.");
                ui.label(text);
            });
        }
        if self.histories_ready() {
            for text in history_findings(&self.points, &self.summaries) {
                ui.horizontal_wrapped(|ui| {
                    badge::kind_badge(ui, Kind::Conditional, &format!("Conditional on {preset_label}; authored assumptions, transport uncertainty not propagated."));
                    ui.label(text);
                });
            }
        } else if self.pending.is_none() && self.history_error.is_none() {
            ui.weak("Load operating assumptions (--assumptions) to calculate replacements and electricity for each allocation.");
        }
    }

    fn radial_bar(&self, ui: &mut egui::Ui) {
        let Some(manifest) = &self.manifest else {
            return;
        };
        let point = &self.points[self.selected];
        let Some(variant) = manifest.variants.iter().find(|v| v.id == point.variant_id) else {
            return;
        };
        let total: f64 = variant.components.iter().map(|c| c.thickness_m).sum();
        let width = ui.available_width().min(720.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 24.0), egui::Sense::hover());
        let mut x = rect.left();
        for component in &variant.components {
            let w = (component.thickness_m / total) as f32 * width;
            let segment = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(w, 24.0));
            x += w;
            let fill = crate::hex_color(&component.color);
            ui.painter().rect_filled(segment, 0.0, fill);
            ui.painter().rect_stroke(
                segment,
                0.0,
                egui::Stroke::new(1.0, egui::Color32::from_black_alpha(90)),
                egui::StrokeKind::Inside,
            );
            let luminance =
                0.299 * fill.r() as f32 + 0.587 * fill.g() as f32 + 0.114 * fill.b() as f32;
            if w >= 34.0 {
                ui.painter().text(
                    segment.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("{:.2}", component.thickness_m),
                    egui::FontId::proportional(11.0),
                    if luminance > 140.0 {
                        egui::Color32::BLACK
                    } else {
                        egui::Color32::WHITE
                    },
                );
            }
            ui.interact(
                segment,
                ui.id().with(("sweep-build", &component.id)),
                egui::Sense::hover(),
            )
            .on_hover_text(format!(
                "{} \u{b7} {:.3} m \u{b7} {}",
                component.label, component.thickness_m, component.material_id
            ));
        }
    }
}

fn named(blanket_m: f64) -> Option<&'static str> {
    NAMED
        .iter()
        .find(|(m, _)| (m - blanket_m).abs() < 1e-9)
        .map(|(_, name)| *name)
}

/// Extract every completed point (ascending blanket thickness) and its
/// history driving rates from the loaded sweep records.
fn collect_points(
    transport: &TransportPanel,
    scenario: &LoadedScenario,
    manifest: &DemoManifest,
) -> Result<(Vec<TransportPoint>, Vec<TransportDrivingRates>), String> {
    let mut rows = Vec::new();
    for variant in &scenario.scenario.variants {
        let Some(run) = transport.record(&variant.id) else {
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
        let rates =
            TransportDrivingRates::from_normalized(normalized, manifest.fusion_power_mw, raw)?;
        rows.push((point, rates));
    }
    if rows.is_empty() {
        return Err("no completed transport records in the sweep bundles".into());
    }
    rows.sort_by(|a, b| a.0.blanket_m.total_cmp(&b.0.blanket_m));
    Ok(rows.into_iter().unzip())
}

#[derive(Clone, Copy, PartialEq)]
enum Chart {
    Breeding,
    MagnetFlux,
    Histories,
}

struct ChartData<'a> {
    points: &'a [TransportPoint],
    summaries: Option<&'a Vec<Option<HistorySummary>>>,
    selected: usize,
    pending: bool,
    preset_label: &'a str,
}

fn text(
    painter: &egui::Painter,
    pos: egui::Pos2,
    align: egui::Align2,
    value: impl ToString,
    size: f32,
    color: egui::Color32,
) {
    painter.text(
        pos,
        align,
        value.to_string(),
        egui::FontId::proportional(size),
        color,
    );
}

fn error_bar(painter: &egui::Painter, x: f32, lo: f32, hi: f32, mid: f32, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.4, color);
    painter.line_segment([egui::pos2(x, lo), egui::pos2(x, hi)], stroke);
    for y in [lo, hi] {
        painter.line_segment([egui::pos2(x - 4.0, y), egui::pos2(x + 4.0, y)], stroke);
    }
    painter.circle_filled(egui::pos2(x, mid), 3.5, color);
}

/// Draw one chart; returns a clicked point index.
fn draw_chart(
    ui: &mut egui::Ui,
    chart: Chart,
    data: &ChartData,
    width: f32,
    height: f32,
) -> Option<usize> {
    let points = data.points;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let painter = ui.painter_at(rect);
    let weak = ui.visuals().weak_text_color();
    let strong = ui.visuals().strong_text_color();
    let grid = egui::Color32::from_white_alpha(18);
    let right_margin = if chart == Chart::Histories {
        48.0
    } else {
        12.0
    };
    let plot = egui::Rect::from_min_max(
        rect.min + egui::vec2(56.0, 40.0),
        rect.max - egui::vec2(right_margin, 48.0),
    );
    let (title, subtitle) = match chart {
        Chart::Breeding => (
            "Tritium breeding ratio",
            "total H3 per source neutron \u{b7} \u{b1}2 SE \u{b7} axis not from zero",
        ),
        Chart::MagnetFlux => (
            "Magnet-region mean neutron flux",
            "n/m\u{b2}/s, log scale \u{b7} \u{b1}2 SE",
        ),
        Chart::Histories => (
            "Magnet replacements and electricity",
            if data
                .summaries
                .is_some_and(|s| s.iter().flatten().any(|s| !s.magnets_replaceable))
            {
                "bars: swaps (magnets permanent: none) \u{b7} red: limit year \u{b7} line: net TWh"
            } else {
                "bars: swaps in horizon \u{b7} line: net TWh (right axis)"
            },
        ),
    };
    text(
        &painter,
        rect.left_top() + egui::vec2(2.0, 2.0),
        egui::Align2::LEFT_TOP,
        title,
        13.0,
        strong,
    );
    text(
        &painter,
        rect.left_top() + egui::vec2(2.0, 19.0),
        egui::Align2::LEFT_TOP,
        subtitle,
        10.0,
        weak,
    );

    // Shared x axis.
    let first = points.first()?.blanket_m;
    let last = points.last()?.blanket_m;
    let pad = ((last - first) / (points.len().max(2) - 1) as f64) * 0.6;
    let x_axis = Axis {
        lo: first - pad,
        hi: last + pad,
        log: false,
    };
    let px = |m: f64| plot.left() + x_axis.frac(m) * plot.width();
    let step_px = if points.len() > 1 {
        px(points[1].blanket_m) - px(points[0].blanket_m)
    } else {
        plot.width()
    };

    // Y axes per chart.
    let breeding_bounds = |p: &TransportPoint| p.breeding.interval_2sigma();
    let flux_bounds = |p: &TransportPoint| p.magnet_flux.interval_2sigma();
    let y_axis = match chart {
        Chart::Breeding => {
            let lo = points
                .iter()
                .map(|p| breeding_bounds(p).0)
                .fold(f64::INFINITY, f64::min);
            let hi = points
                .iter()
                .map(|p| breeding_bounds(p).1)
                .fold(f64::NEG_INFINITY, f64::max);
            let span = (hi - lo).max(1e-6);
            Axis {
                lo: lo - 0.06 * span,
                hi: hi + 0.18 * span,
                log: false,
            }
        }
        Chart::MagnetFlux => {
            let lo = points
                .iter()
                .map(|p| {
                    let low = flux_bounds(p).0;
                    if low > 0.0 {
                        low
                    } else {
                        p.magnet_flux.mean / 3.0
                    }
                })
                .fold(f64::INFINITY, f64::min)
                .max(1e-300);
            let hi = points
                .iter()
                .map(|p| flux_bounds(p).1)
                .fold(f64::NEG_INFINITY, f64::max);
            Axis {
                lo: lo / 1.15,
                hi: hi * 1.6,
                log: true,
            }
        }
        Chart::Histories => {
            let max = data
                .summaries
                .map(|s| {
                    s.iter()
                        .flatten()
                        .map(|s| s.magnet_replacements)
                        .max()
                        .unwrap_or(0)
                })
                .unwrap_or(0);
            Axis {
                lo: 0.0,
                hi: (f64::from(max) * 1.25).max(f64::from(max) + 1.0),
                log: false,
            }
        }
    };

    // Selected-point band.
    if let Some(p) = points.get(data.selected) {
        let x = px(p.blanket_m);
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x - step_px * 0.5, plot.top()),
                egui::pos2(x + step_px * 0.5, plot.bottom()),
            ),
            0.0,
            egui::Color32::from_white_alpha(14),
        );
    }

    // Y grid and labels.
    let py = |axis: &Axis, v: f64| plot.bottom() - axis.frac(v) * plot.height();
    let (y_ticks, y_step): (Vec<f64>, f64) = if y_axis.log {
        (log_ticks(y_axis.lo, y_axis.hi), 1.0)
    } else if chart == Chart::Histories {
        // Whole-number replacement counts only.
        let step = nice_ticks(0.0, y_axis.hi, 4).1.ceil().max(1.0);
        let ticks = (0..)
            .map(|i| f64::from(i) * step)
            .take_while(|v| *v <= y_axis.hi)
            .collect();
        (ticks, step)
    } else {
        nice_ticks(y_axis.lo, y_axis.hi, 5)
    };
    for v in &y_ticks {
        let y = py(&y_axis, *v);
        painter.line_segment(
            [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            egui::Stroke::new(1.0, grid),
        );
        let label = if y_axis.log {
            format!("{v:e}")
        } else {
            format!("{:.*}", decimals(y_step), v)
        };
        text(
            &painter,
            egui::pos2(plot.left() - 5.0, y),
            egui::Align2::RIGHT_CENTER,
            label,
            10.0,
            weak,
        );
    }
    painter.rect_stroke(
        plot,
        0.0,
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(60)),
        egui::StrokeKind::Inside,
    );

    // X ticks: blanket over shield thickness.
    let stride = if step_px < 58.0 { 2 } else { 1 };
    for (i, p) in points.iter().enumerate() {
        let x = px(p.blanket_m);
        painter.line_segment(
            [
                egui::pos2(x, plot.bottom()),
                egui::pos2(x, plot.bottom() + 4.0),
            ],
            egui::Stroke::new(1.0, weak),
        );
        if i % stride == 0 {
            text(
                &painter,
                egui::pos2(x, plot.bottom() + 6.0),
                egui::Align2::CENTER_TOP,
                format!("{:.2} / {:.2}", p.blanket_m, p.shield_m),
                10.0,
                if i == data.selected { strong } else { weak },
            );
        }
    }
    text(
        &painter,
        egui::pos2(plot.center().x, rect.bottom() - 2.0),
        egui::Align2::CENTER_BOTTOM,
        "Blanket thickness (m) / shield thickness (m)",
        10.5,
        weak,
    );

    let calc = Kind::Calculated.color();
    let mut secondary: Option<(Axis, Vec<(f32, f64)>)> = None;
    match chart {
        Chart::Breeding | Chart::MagnetFlux => {
            for p in points {
                let (est, bounds) = if chart == Chart::Breeding {
                    (p.breeding, breeding_bounds(p))
                } else {
                    (p.magnet_flux, flux_bounds(p))
                };
                error_bar(
                    &painter,
                    px(p.blanket_m),
                    py(&y_axis, bounds.0),
                    py(&y_axis, bounds.1),
                    py(&y_axis, est.mean),
                    calc,
                );
            }
        }
        Chart::Histories => {
            if let Some(summaries) = data.summaries {
                let bar = egui::Color32::from_rgb(120, 140, 175);
                for (p, s) in points.iter().zip(summaries) {
                    let Some(s) = s else { continue };
                    let x = px(p.blanket_m);
                    let top = py(&y_axis, f64::from(s.magnet_replacements));
                    let half = (step_px * 0.28).clamp(3.0, 18.0);
                    painter.rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(x - half, top),
                            egui::pos2(x + half, plot.bottom()),
                        ),
                        1.0,
                        bar.gamma_multiply(0.75),
                    );
                    text(
                        &painter,
                        egui::pos2(x, top - 1.0),
                        egui::Align2::CENTER_BOTTOM,
                        s.magnet_replacements,
                        10.0,
                        strong,
                    );
                    if let Some(year) = s.magnet_permanent_limit_years {
                        // Permanent magnets cannot be swapped: mark the limit year.
                        text(
                            &painter,
                            egui::pos2(x, plot.bottom() - 4.0),
                            egui::Align2::CENTER_BOTTOM,
                            format!("limit {year:.1} y"),
                            9.5,
                            Kind::Failed.color(),
                        );
                    }
                }
                let energies: Vec<(usize, f64)> = summaries
                    .iter()
                    .enumerate()
                    .filter_map(|(i, s)| Some((i, s.as_ref()?.net_electricity_twh?)))
                    .collect();
                if let (Some(lo), Some(hi)) = (
                    energies.iter().map(|e| e.1).reduce(f64::min),
                    energies.iter().map(|e| e.1).reduce(f64::max),
                ) {
                    let span = (hi - lo).max(hi.abs() * 0.02).max(1e-9);
                    let axis = Axis {
                        lo: lo - 0.15 * span,
                        hi: hi + 0.3 * span,
                        log: false,
                    };
                    secondary = Some((
                        axis,
                        energies
                            .iter()
                            .map(|(i, e)| (px(points[*i].blanket_m), *e))
                            .collect(),
                    ));
                }
            }
        }
    }
    if let Some((axis, line)) = &secondary {
        let color = Kind::Partial.color();
        let (ticks, step) = nice_ticks(axis.lo, axis.hi, 4);
        for v in ticks {
            let y = plot.bottom() - axis.frac(v) * plot.height();
            text(
                &painter,
                egui::pos2(plot.right() + 5.0, y),
                egui::Align2::LEFT_CENTER,
                format!("{:.*}", decimals(step), v),
                10.0,
                color,
            );
        }
        let positions: Vec<egui::Pos2> = line
            .iter()
            .map(|(x, e)| egui::pos2(*x, plot.bottom() - axis.frac(*e) * plot.height()))
            .collect();
        painter.add(egui::Shape::line(
            positions.clone(),
            egui::Stroke::new(2.0, color),
        ));
        for pos in positions {
            painter.circle_filled(pos, 3.0, color);
        }
        text(
            &painter,
            egui::pos2(rect.right() - 2.0, plot.top() - 8.0),
            egui::Align2::RIGHT_BOTTOM,
            "TWh",
            10.0,
            color,
        );
    }
    if chart == Chart::Histories && data.summaries.is_none() {
        let note = if data.pending {
            "Calculating histories\u{2026}"
        } else {
            "No history: load operating assumptions"
        };
        text(
            &painter,
            plot.center(),
            egui::Align2::CENTER_CENTER,
            note,
            11.0,
            weak,
        );
        if data.pending {
            let spinner = egui::Rect::from_center_size(
                plot.center() + egui::vec2(0.0, 22.0),
                egui::vec2(18.0, 18.0),
            );
            egui::Spinner::new().paint_at(ui, spinner);
        }
    }

    // Named arrangements: ring markers and labels on every chart.
    for p in points {
        let Some(name) = named(p.blanket_m) else {
            continue;
        };
        let x = px(p.blanket_m);
        let y = match chart {
            Chart::Breeding => py(&y_axis, p.breeding.mean),
            Chart::MagnetFlux => py(&y_axis, p.magnet_flux.mean),
            Chart::Histories => match data
                .summaries
                .and_then(|s| s.get(points.iter().position(|q| q.variant_id == p.variant_id)?))
                .and_then(|s| s.as_ref())
            {
                Some(s) => py(&y_axis, f64::from(s.magnet_replacements)),
                None => plot.bottom(),
            },
        };
        painter.circle_stroke(
            egui::pos2(x, y),
            8.0,
            egui::Stroke::new(1.6, egui::Color32::WHITE),
        );
        text(
            &painter,
            egui::pos2(x, plot.top() + 3.0),
            egui::Align2::CENTER_TOP,
            name,
            10.5,
            egui::Color32::WHITE,
        );
    }

    // Hover and click: nearest point by x.
    let mut clicked = None;
    if let Some(pointer) = response
        .hover_pos()
        .filter(|p| plot.expand2(egui::vec2(12.0, 0.0)).contains(*p))
    {
        let nearest = (0..points.len())
            .min_by(|a, b| {
                (px(points[*a].blanket_m) - pointer.x)
                    .abs()
                    .total_cmp(&(px(points[*b].blanket_m) - pointer.x).abs())
            })
            .unwrap_or(0);
        let x = px(points[nearest].blanket_m);
        painter.line_segment(
            [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(70)),
        );
        if response.clicked() {
            clicked = Some(nearest);
        }
        let summary = data
            .summaries
            .and_then(|s| s.get(nearest))
            .and_then(|s| s.as_ref());
        response.clone().on_hover_ui_at_pointer(|ui| {
            tooltip(ui, &points[nearest], summary, data.preset_label);
        });
    }
    clicked
}

fn estimate_text(e: &Estimate, scientific: bool) -> String {
    let relative = if e.mean != 0.0 {
        format!(" ({:.1}% rel. SE)", 100.0 * e.standard_error / e.mean.abs())
    } else {
        String::new()
    };
    if scientific {
        format!("{:.3e} \u{b1} {:.2e}{relative}", e.mean, e.standard_error)
    } else {
        format!("{:.4} \u{b1} {:.4}{relative}", e.mean, e.standard_error)
    }
}

fn tooltip(ui: &mut egui::Ui, p: &TransportPoint, summary: Option<&HistorySummary>, preset: &str) {
    ui.set_max_width(380.0);
    ui.strong(format!(
        "Blanket {:.2} m / shield {:.2} m{}",
        p.blanket_m,
        p.shield_m,
        named(p.blanket_m)
            .map(|n| format!(" \u{b7} {n}"))
            .unwrap_or_default()
    ));
    egui::Grid::new("sweep-tip").num_columns(2).show(ui, |ui| {
        ui.weak("Breeding (H3/source)");
        ui.monospace(estimate_text(&p.breeding, false));
        ui.end_row();
        ui.weak("Magnet flux (n/m\u{b2}/s)");
        ui.monospace(estimate_text(&p.magnet_flux, true));
        ui.end_row();
        ui.weak("Run");
        ui.monospace(format!(
            "seed {} \u{b7} {} histories",
            p.seed,
            crate::transport_panel::grouped(p.histories as usize)
        ));
        ui.end_row();
        if let Some(s) = summary {
            ui.weak("Magnet replacements");
            ui.monospace(format!(
                "{}{}",
                s.magnet_replacements,
                s.first_magnet_replacement_years
                    .map(|y| format!(" (first at {y:.1} y)"))
                    .unwrap_or_default()
            ));
            ui.end_row();
            ui.weak("Net electricity");
            ui.monospace(
                s.net_electricity_twh
                    .map_or("unavailable".into(), |e| format!("{e:.2} TWh")),
            );
            ui.end_row();
            ui.weak("Usable tritium at end");
            ui.monospace(format!("{:.2} kg", s.final_available_tritium_kg));
            ui.end_row();
        }
    });
    if summary.is_some() {
        ui.weak(format!(
            "Replacements and electricity: conditional on {preset}."
        ));
    }
    ui.weak("Sweep points are independent runs with different seeds, not the same records; differences below ~2\u{3c3} are sampling noise.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_ticks_cover_the_range_with_round_steps() {
        let (ticks, step) = nice_ticks(1.23, 1.31, 5);
        assert!((step - 0.02).abs() < 1e-12, "{step}");
        assert!(ticks.first().unwrap() >= &1.23 && ticks.last().unwrap() <= &1.31001);
        assert!(ticks.len() >= 4);
        assert_eq!(nice_ticks(0.0, 4.4, 4).1, 1.0);
    }

    #[test]
    fn log_ticks_use_one_two_five_and_densify_narrow_ranges() {
        assert_eq!(log_ticks(0.9e14, 6e14), vec![1e14, 2e14, 5e14]);
        let narrow = log_ticks(0.9e14, 2.2e14);
        assert!(narrow.len() >= 3, "{narrow:?}");
    }

    #[test]
    fn axis_maps_linear_and_log_and_clamps() {
        let linear = Axis {
            lo: 0.0,
            hi: 10.0,
            log: false,
        };
        assert_eq!(linear.frac(5.0), 0.5);
        assert_eq!(linear.frac(20.0), 1.0);
        let log = Axis {
            lo: 1.0,
            hi: 100.0,
            log: true,
        };
        assert!((log.frac(10.0) - 0.5).abs() < 1e-6);
        assert_eq!(log.frac(-3.0), 0.0);
    }

    #[test]
    fn named_arrangements_match_by_thickness() {
        assert_eq!(named(0.45), Some("Reference"));
        assert_eq!(named(0.55), Some("Breeder-heavy"));
        assert_eq!(named(0.5), None);
    }

    #[test]
    fn an_unloaded_sweep_reports_its_error_instead_of_points() {
        let panel = SweepPanel::new(Err("bundle missing".into()));
        assert_eq!(panel.load_error.as_deref(), Some("bundle missing"));
        assert!(panel.points.is_empty() && !panel.is_pending());
    }
}
