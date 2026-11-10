// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! Maintenance view: opens a `faris-maintenance-result/v0.1` file and shows, per
//! design, the replacement outages computed from the decay heat of the components
//! around the replaced part beside the authored (fixed) ones, and why each
//! cooldown is what it is.
//!
//! Call from the application as `maintenance_panel.window(ctx)`; the panel is a
//! floating window opened from the top bar. The file is read and parsed on a
//! worker thread. The loaded result view reads its numbers from that file; its
//! arithmetic is unit conversion (seconds to days and years) and sorting. A
//! separate what-if section calls the shared history engine on a worker thread
//! using the active history snapshot. The view makes no ranking claim: it
//! reports downtime, availability and electricity under the duration cases.

use crate::{
    badge::{self, Kind},
    maintenance_run::RunPanel,
    package::{self, RecordedMaintenance},
    sweep_panel::nice_ticks,
};
use eframe::egui;
use faris_engine::{
    duration_sensitivity::{
        DurationRange, DurationTornado, base_replacement_duration_s, duration_tornado,
        with_replacement_duration_s,
    },
    history::{HistoryOutcome, JULIAN_YEAR_SECONDS},
    maintenance::{
        ComputedResult, Contrast, DesignResult, EventRecord, HistorySummary,
        MAINTENANCE_RESULT_VERSION, MaintenanceResult, NotEvaluated, Status, ThresholdRecord, Why,
    },
};
use faris_model::{
    history::{ComponentClass, OperatingHistoryAssumptions},
    maintenance::GoverningQuantity,
    remote_handling::{InputLabel, MaintenancePreset, maintenance_presets},
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};

const DAY_S: f64 = faris_model::units::DAY_S;

/// Current, identified what-if inputs supplied by the history panel.
#[derive(Clone, Debug, PartialEq)]
pub struct DurationSnapshot {
    pub assumptions: OperatingHistoryAssumptions,
    pub rates: faris_engine::history::TransportDrivingRates,
}

type TornadoReceiver = Receiver<(u64, Result<DurationTornado, String>)>;

/// What "computed" means, shown under the summary.
pub const COMPUTED_MEANING: &str = "Computed: each replacement's outage is the cooldown until the decay heat per volume of the governing components falls below the class threshold q*, plus the class's work time; the operating history is rerun with those outages until they stop changing. Fixed is the authored duration. The two models are compared on downtime, availability and lifetime electricity; the comparison makes no ranking claim.";

/// Shown under the button that opens the package's recorded result.
pub const RECORDED_NOTE: &str = "Recorded with ACTINV for the four packaged arrangements; to recompute, use Run\u{2026} with ACTINV and your own transport runs.";

const NET_UNAVAILABLE: &str = "The operating history recorded no cumulative net electricity at its end (the history assumptions carry no energy assumptions, or no snapshot was recorded), so none is inferred here.";

/// The typed maintenance result records point estimates without propagated uncertainty fields.
const MAINTENANCE_NO_INTERVAL: &str = "No propagated interval is recorded. Reason: the typed maintenance result stores point estimates only; it has no propagated transport, activation or operating-schedule uncertainty fields. Next step: propagate those uncertainties through the maintenance calculation and record the resulting intervals.";

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
    pub run: RunPanel,
    /// The recorded result of the opened package, if it carries one.
    recorded: Option<RecordedMaintenance>,
    /// The first opening of the window in this session has been handled.
    recorded_offered: bool,
    duration_snapshot: Option<DurationSnapshot>,
    duration_preset_index: usize,
    duration_component: String,
    pending_history_edit: Option<OperatingHistoryAssumptions>,
    tornado: Option<DurationTornado>,
    tornado_receiver: Option<TornadoReceiver>,
    tornado_error: Option<String>,
    duration_revision: u64,
}

impl Default for MaintenancePanel {
    fn default() -> Self {
        Self {
            open: false,
            state: State::Empty,
            selected: None,
            loading: None,
            dialog: None,
            run: RunPanel::default(),
            recorded: None,
            recorded_offered: false,
            duration_snapshot: None,
            duration_preset_index: 0,
            duration_component: String::new(),
            pending_history_edit: None,
            tornado: None,
            tornado_receiver: None,
            tornado_error: None,
            duration_revision: 0,
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

/// Reads a package's recorded result after checking its SHA-256 against the index.
fn read_recorded(recorded: &RecordedMaintenance) -> LoadResult {
    let bytes = package::read_verified(&recorded.path, &recorded.sha256)?;
    Ok(Loaded {
        file_name: recorded.path.file_name().map_or_else(
            || recorded.path.display().to_string(),
            |n| n.to_string_lossy().into(),
        ),
        result: Box::new(parse_result(&bytes)?),
    })
}

impl MaintenancePanel {
    /// Supply the active history values for the maintenance what-if section. This is a
    /// snapshot only; edits are returned to the coordinator for its undo-safe history path.
    pub fn set_duration_snapshot(&mut self, snapshot: Option<DurationSnapshot>) {
        if self.duration_snapshot != snapshot {
            self.duration_revision = self.duration_revision.wrapping_add(1);
            self.duration_snapshot = snapshot;
            self.tornado = None;
            self.tornado_error = None;
            self.duration_component.clear();
        }
    }

    /// Retrieve a literature-duration edit requested by the UI. The caller applies it through
    /// HistoryPanel's normal validated, undo-aware edit path.
    pub fn take_history_edit(&mut self) -> Option<OperatingHistoryAssumptions> {
        self.pending_history_edit.take()
    }

    fn duration_what_if(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Replacement-duration what-if")
            .id_salt("maintenance-duration-what-if")
            .default_open(false)
            .show(ui, |ui| {
                let Some(snapshot) = self.duration_snapshot.clone() else {
                    ui.weak("Load and calculate an identified operating history to compare a replacement duration.");
                    return;
                };
                let components: Vec<String> = snapshot
                    .assumptions
                    .service_limits
                    .iter()
                    .filter(|limit| limit.class == ComponentClass::Replaceable)
                    .map(|limit| limit.component_id.clone())
                    .fold(Vec::new(), |mut ids, id| {
                        if !ids.contains(&id) {
                            ids.push(id);
                        }
                        ids
                    });
                if components.is_empty() {
                    ui.weak("This history has no replaceable components with duration inputs.");
                    return;
                }
                if self.duration_component.is_empty()
                    || !components.iter().any(|id| id == &self.duration_component)
                {
                    self.duration_component = components[0].clone();
                }
                let presets = maintenance_presets();
                self.duration_preset_index = self.duration_preset_index.min(presets.len() - 1);
                let previous_choice =
                    (self.duration_component.clone(), self.duration_preset_index);
                ui.horizontal(|ui| {
                    ui.label("Replaceable component");
                    egui::ComboBox::from_id_salt("maintenance-duration-component")
                        .selected_text(&self.duration_component)
                        .show_ui(ui, |ui| {
                            for component in &components {
                                ui.selectable_value(
                                    &mut self.duration_component,
                                    component.clone(),
                                    component,
                                );
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.label("Literature campaign");
                    egui::ComboBox::from_id_salt("maintenance-duration-preset")
                        .selected_text(presets[self.duration_preset_index].title)
                        .show_ui(ui, |ui| {
                            for (index, preset) in presets.iter().enumerate() {
                                ui.selectable_value(
                                    &mut self.duration_preset_index,
                                    index,
                                    preset.title,
                                );
                            }
                        });
                });
                if previous_choice
                    != (self.duration_component.clone(), self.duration_preset_index)
                {
                    self.duration_revision = self.duration_revision.wrapping_add(1);
                    self.tornado = None;
                    self.tornado_error = None;
                }
                let preset = presets[self.duration_preset_index].clone();
                ui.small(format!(
                    "{}: {:.1} months whole-campaign outage ({:.1} days). Source: {}",
                    preset.label,
                    preset.duration_months,
                    days(preset.duration_s()),
                    preset.citation
                ));
                ui.small(preset.conditions);
                ui.hyperlink_to(
                    "Open source",
                    preset.source_link.value.unwrap_or("https://arxiv.org/pdf/1412.4008"),
                );
                ui.colored_label(
                    Kind::Conditional.color(),
                    "The source duration is for an EU-DEMO blanket and divertor campaign. Applying it to one selected component creates an authored scenario what-if; the paper does not report a per-component duration.",
                );
                let current = base_replacement_duration_s(
                    &snapshot.assumptions,
                    &self.duration_component,
                );
                match current {
                    Err(error) => {
                        ui.colored_label(Kind::Failed.color(), error);
                    }
                    Ok(base_s) => {
                        ui.small(format!(
                            "Current duration: {}. Campaign value: {}.",
                            days_text(base_s, 1),
                            days_text(preset.duration_s(), 1)
                        ));
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    self.tornado_receiver.is_none(),
                                    egui::Button::new("Calculate duration tornado"),
                                )
                                .on_hover_text("Runs the current history and the low/high duration cases on a worker thread. The history's recorded assumptions and rates are the inputs.")
                                .clicked()
                            {
                                self.start_tornado(snapshot.clone(), &preset, base_s, ui.ctx());
                            }
                            if ui
                                .add_enabled(
                                    self.pending_history_edit.is_none(),
                                    egui::Button::new("Apply campaign duration to this component"),
                                )
                                .clicked()
                            {
                                match assumptions_with_campaign_duration(
                                    &snapshot.assumptions,
                                    &self.duration_component,
                                    &preset,
                                ) {
                                    Ok(edited) => self.pending_history_edit = Some(edited),
                                    Err(error) => self.tornado_error = Some(error),
                                }
                            }
                        });
                    }
                }
                if self.tornado_receiver.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.weak("Calculating three bounded histories…");
                    });
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
                }
                if let Some(error) = &self.tornado_error {
                    ui.colored_label(Kind::Failed.color(), error);
                }
                if let Some(tornado) = &self.tornado {
                    ui.separator();
                    let availability_validation =
                        faris_model::validation::ValidationMark::current(
                            faris_model::validation::Quantity::Availability,
                        )
                        .tag();
                    let electricity_validation =
                        faris_model::validation::ValidationMark::current(
                            faris_model::validation::Quantity::NetElectricPower,
                        )
                        .tag();
                    let availability_note = validation_hover_text(
                        faris_model::validation::Quantity::Availability,
                    );
                    let electricity_note = validation_hover_text(
                        faris_model::validation::Quantity::NetElectricPower,
                    );
                    let lit_endpoint = tornado.bars.first().map_or("no endpoint", |bar| {
                        let is_low = preset.duration_s() == bar.low_s;
                        let is_high = preset.duration_s() == bar.high_s;
                        match (is_low, is_high) {
                            (true, true) => "both endpoints (the durations are equal)",
                            (true, false) => "low-duration endpoint",
                            (false, true) => "high-duration endpoint",
                            (false, false) => "no endpoint",
                        }
                    });
                    ui.label(format!(
                        "These are deterministic scenario sensitivities, not uncertainty bands. No model uncertainty is propagated. Current matrix validation: availability {availability_validation}; net electricity {electricity_validation}. The literature campaign value applies only at the {lit_endpoint}; the other endpoint is the current authored duration."
                    ))
                    .on_hover_text(format!(
                        "Availability — {}\nNet electric power — {}",
                        availability_note,
                        electricity_note,
                    ));
                    ui.small(format!(
                        "Campaign endpoint source: {} · {} · {}",
                        preset.id,
                        preset.citation,
                        preset.source_link.value.unwrap_or("source link not stated")
                    ));
                    ui.label(format!(
                        "Base (conditional deterministic case): availability {:.2} %{}",
                        tornado.base.availability * 100.0,
                        tornado.base.net_electricity_mwh.map_or_else(
                            || "; lifetime net electricity not available".to_owned(),
                            |mwh| format!("; lifetime net electricity {} MWh", group_thousands(mwh))
                        )
                    ));
                    for bar in &tornado.bars {
                        ui.label(format!(
                            "{} · authored low-duration endpoint {}: availability {:.2} %{}; authored high-duration endpoint {}: {:.2} %{}",
                            bar.component_id,
                            days_text(bar.low_s, 1),
                            bar.at_low.availability * 100.0,
                            bar.at_low.net_electricity_mwh.map_or_else(
                                || "".to_owned(),
                                |mwh| format!("; {} MWh", group_thousands(mwh))
                            ),
                            days_text(bar.high_s, 1),
                            bar.at_high.availability * 100.0,
                            bar.at_high.net_electricity_mwh.map_or_else(
                                || "".to_owned(),
                                |mwh| format!("; {} MWh", group_thousands(mwh))
                            )
                        ));
                        ui.small(format!(
                            "Duration bounds: {}. The literature source applies only to the endpoint identified above.",
                            input_label_text(&bar.label)
                        ));
                    }
                }
                if self.pending_history_edit.is_some() {
                    ui.colored_label(
                        Kind::Authored.color(),
                        "Literature duration prepared as an authored scenario assumption. The application will validate and apply it through the history editor; use Undo to restore the prior assumptions.",
                    );
                }
            });
    }

    fn start_tornado(
        &mut self,
        snapshot: DurationSnapshot,
        preset: &MaintenancePreset,
        base_s: f64,
        ctx: &egui::Context,
    ) {
        let (low_s, high_s) = if base_s <= preset.duration_s() {
            (base_s, preset.duration_s())
        } else {
            (preset.duration_s(), base_s)
        };
        let range = DurationRange {
            component_id: self.duration_component.clone(),
            low_s,
            high_s,
            label: InputLabel::Authored,
        };
        let (sender, receiver) = mpsc::channel();
        let context = ctx.clone();
        let revision = self.duration_revision;
        let spawned = crate::thread::Builder::new()
            .name("faris-duration-tornado".into())
            .spawn(move || {
                let result = duration_tornado(&snapshot.assumptions, &snapshot.rates, &[range]);
                let _ = sender.send((revision, result));
                context.request_repaint();
            });
        match spawned {
            Ok(_) => {
                self.tornado = None;
                self.tornado_error = None;
                self.tornado_receiver = Some(receiver);
            }
            Err(error) => {
                self.tornado_error = Some(format!("Cannot start tornado worker: {error}"))
            }
        }
    }

    fn poll_tornado(&mut self, ctx: &egui::Context) {
        let Some(receiver) = &self.tornado_receiver else {
            return;
        };
        match receiver.try_recv() {
            Ok((revision, Ok(result))) => {
                self.tornado_receiver = None;
                if revision == self.duration_revision {
                    self.tornado = Some(result);
                }
            }
            Ok((revision, Err(error))) => {
                self.tornado_receiver = None;
                if revision == self.duration_revision {
                    self.tornado_error = Some(error);
                }
            }
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(200))
            }
            Err(TryRecvError::Disconnected) => {
                self.tornado_receiver = None;
                self.tornado_error = Some("The tornado worker ended unexpectedly.".into());
            }
        }
    }

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
        self.loading.is_some()
            || self.dialog.is_some()
            || self.run.is_running()
            || self.tornado_receiver.is_some()
    }

    #[cfg(test)]
    pub fn poll_run_for_tests(&mut self) {
        self.poll_run();
    }

    #[cfg(test)]
    pub fn shows_result_for_tests(&self) -> bool {
        matches!(self.state, State::Ready(_))
    }

    /// Start values for the Run section's inputs (command-line options).
    pub fn prefill_run(
        &mut self,
        designs: Option<&Path>,
        assumptions: Option<&Path>,
        actinv: Option<&Path>,
        data_dir: Option<&Path>,
    ) {
        let text = |p: &Path| p.display().to_string();
        if let Some(p) = designs {
            self.run.inputs.designs = text(p);
        }
        if let Some(p) = assumptions {
            self.run.inputs.assumptions = text(p);
        }
        if let Some(p) = actinv {
            self.run.inputs.actinv = text(p);
        }
        if let Some(p) = data_dir {
            self.run.inputs.data_dir = text(p);
        }
    }

    /// Takes a finished run's result into the view; called every frame, window open or not.
    fn poll_run(&mut self) {
        if let Some((file_name, result)) = self.run.poll() {
            self.install(Ok(Loaded {
                file_name,
                result: Box::new(result),
            }));
        }
    }

    /// The package's recorded maintenance result, offered in the window.
    pub fn set_recorded(&mut self, recorded: RecordedMaintenance) {
        self.recorded = Some(recorded);
    }

    /// Read and parse `path` on a worker thread and open the window.
    pub fn load_in_background(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.spawn_load(ctx, move || read_result(&path));
    }

    /// Verify and load the package's recorded result on a worker thread.
    fn load_recorded_in_background(&mut self, ctx: &egui::Context) {
        if let Some(recorded) = self.recorded.clone() {
            self.spawn_load(ctx, move || read_recorded(&recorded));
        }
    }

    /// The first time the window is opened in a session, open the recorded result unless
    /// something else is open or loading.
    fn offer_recorded(&mut self, ctx: &egui::Context) {
        if self.recorded_offered {
            return;
        }
        self.recorded_offered = true;
        if self.recorded.is_some() && matches!(self.state, State::Empty) && !self.is_pending() {
            self.load_recorded_in_background(ctx);
        }
    }

    fn spawn_load(
        &mut self,
        ctx: &egui::Context,
        job: impl FnOnce() -> LoadResult + Send + 'static,
    ) {
        let (sender, receiver) = mpsc::channel();
        let context = ctx.clone();
        let spawned = crate::thread::Builder::new()
            .name("faris-maintenance-load".into())
            .spawn(move || {
                let _ = sender.send(job());
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
        let spawned = crate::thread::Builder::new()
            .name("faris-maintenance-dialog".into())
            .spawn(move || {
                let picked = crate::dialog::FileDialog::new()
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
        self.poll_run();
        self.poll_tornado(ctx);
        if !self.open {
            return;
        }
        self.offer_recorded(ctx);
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
        let mut open_recorded = false;
        ui.horizontal(|ui| {
            if !crate::browser::IS_BROWSER
                && ui
                .add_enabled(!self.is_pending(), egui::Button::new("Open result\u{2026}"))
                .on_hover_text(
                    "Open a faris-maintenance-result/v0.1 file written by the maintenance command.",
                )
                .clicked()
            {
                open_dialog = true;
            }
            if self.recorded.is_some()
                && ui
                    .add_enabled(
                        !self.is_pending(),
                        egui::Button::new("Open the recorded maintenance result"),
                    )
                    .on_hover_text(
                        "Open the result recorded in this package; its SHA-256 is checked against the package index first.",
                    )
                    .clicked()
            {
                open_recorded = true;
            }
            if self.loading.is_some() {
                ui.spinner();
                ui.weak("Reading the result\u{2026}");
            }
        });
        if self.recorded.is_some() {
            ui.weak(if crate::browser::IS_BROWSER {
                "Recorded with ACTINV for the four packaged arrangements."
            } else {
                RECORDED_NOTE
            });
        }
        if open_recorded {
            self.load_recorded_in_background(&ui.ctx().clone());
        }
        if open_dialog {
            self.pick_file(&ui.ctx().clone());
        }
        ui.separator();
        if crate::browser::IS_BROWSER {
            crate::browser::desktop_only_note(ui, "Running the maintenance calculation (ACTINV)");
        } else {
            let running = self.run.is_running();
            egui::CollapsingHeader::new(if running {
                "Run (in progress)"
            } else {
                "Run\u{2026}"
            })
            .id_salt("maintenance-run-section")
            .default_open(matches!(self.state, State::Empty) || running)
            .show(ui, |ui| self.run.ui(ui));
        }
        self.duration_what_if(ui);
        ui.separator();
        self.poll_run();
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
                // Vertical only: each wide table scrolls horizontally inside the window's
                // width, so its scroll bar is where the table is and no column is cut off.
                egui::ScrollArea::vertical()
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

fn assumptions_with_campaign_duration(
    assumptions: &OperatingHistoryAssumptions,
    component_id: &str,
    preset: &MaintenancePreset,
) -> Result<OperatingHistoryAssumptions, String> {
    let mut edited = with_replacement_duration_s(assumptions, component_id, preset.duration_s())?;
    for limit in edited.service_limits.iter_mut().filter(|limit| {
        limit.component_id == component_id && limit.class == ComponentClass::Replaceable
    }) {
        limit.provenance = format!(
            "authored per-component scenario what-if using literature campaign {} ({}, {}); source duration is for a whole EU-DEMO blanket/divertor campaign, not a per-component measurement",
            preset.id,
            preset.citation,
            preset.source_link.value.unwrap_or("no source link stated")
        );
    }
    edited.provenance = format!(
        "{}; authored per-component what-if: {} replacement duration set to literature campaign {} ({})",
        assumptions.provenance, component_id, preset.id, preset.citation
    );
    edited.validate()?;
    Ok(edited)
}

fn input_label_text(label: &InputLabel) -> String {
    match label {
        InputLabel::Authored => "authored range".into(),
        InputLabel::Literature { preset_id, link } => format!(
            "literature endpoint from preset {preset_id}{}",
            link.as_deref()
                .map_or_else(String::new, |url| format!(" ({url})"))
        ),
    }
}

/// A duration in the unit the user chose for durations (days until they choose), unit attached.
fn days_text(seconds: f64, decimals: usize) -> String {
    crate::quantity_text::quantity("time", "d", seconds, decimals)
}

fn validation_hover_text(quantity: faris_model::validation::Quantity) -> String {
    let note = faris_model::validation::note(quantity);
    format!(
        "Scope: {}. Why: {}. Next step: {}",
        note.scope, note.why, note.next_step
    )
}

fn validation_tag(quantity: faris_model::validation::Quantity) -> String {
    faris_model::validation::ValidationMark::current(quantity).tag()
}

fn validated_value(
    ui: &mut egui::Ui,
    value: impl Into<String>,
    quantity: faris_model::validation::Quantity,
) {
    ui.monospace(format!("{} {}", value.into(), validation_tag(quantity)))
        .on_hover_text(validation_hover_text(quantity));
}

/// Show validation state and the explicit uncertainty status for a calculated maintenance value.
/// Authored durations/work/thresholds/counts deliberately continue to use plain labels.
fn maintenance_calculated_value(
    ui: &mut egui::Ui,
    value: impl Into<String>,
    quantity: faris_model::validation::Quantity,
) {
    ui.monospace(format!(
        "{} {} · no interval",
        value.into(),
        validation_tag(quantity)
    ))
    .on_hover_text(format!(
        "{}. {}",
        validation_hover_text(quantity),
        MAINTENANCE_NO_INTERVAL
    ));
}

fn maintenance_uncertainty_detail() -> &'static str {
    MAINTENANCE_NO_INTERVAL
}

/// A time in service in the unit the user chose (years until they choose), unit attached.
fn years_text(seconds: f64, decimals: usize) -> String {
    crate::quantity_text::quantity("service_time", "yr", seconds, decimals)
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
                "{} {:.0} % ({} in service)",
                g.component,
                g.share * 100.0,
                years_text(g.in_service_s, 1)
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
                "calibrated on {design} to a target of {}",
                days_text(target, 1)
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
            "{} \u{b7} {} \u{b7} replacement {} at {}",
            event.class,
            event.component,
            event.k,
            years_text(event.start_s, 2)
        ),
        format!("fixed {}", days_text(event.fixed_duration_s, 1)),
    ];
    match (event.duration_computed_s, event.cooldown_s) {
        (Some(total), Some(cool)) => lines.push(format!(
            "computed {} = cooldown {} + work {}",
            days_text(total, 1),
            days_text(cool, 1),
            days_text(event.work_s, 1)
        )),
        _ => lines.push("computed: not evaluated".into()),
    }
    if event.duration_computed_s.is_some() {
        lines.push(format!("Uncertainty: {}", maintenance_uncertainty_detail()));
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
        if let Some(produced) = &result.produced_by {
            ui.weak(format!("Produced by FARIS {}", produced.faris_version));
        }
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
                ui.label(spec.map_or("\u{2014}".into(), |c| days_text(c.work_s, 1)));
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
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .show(ui, |ui| {
            egui::Grid::new("maintenance-summary")
                .striped(true)
                .spacing([14.0, 3.0])
                .show(ui, |ui| {
                    for caption in [
                        "Design",
                        "History",
                        "Downtime\nfixed (d)",
                        "Downtime\ncomputed (d)",
                        "Availability\nfixed (%)",
                        "Availability\ncomputed (%)",
                        "Net electricity\nfixed (MWh)",
                        "Net electricity\ncomputed (MWh)",
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
            maintenance_calculated_value(
                ui,
                text,
                faris_model::validation::Quantity::NetElectricPower,
            );
        }
        None => {
            ui.weak("unavailable").on_hover_text(format!(
                "{NET_UNAVAILABLE}\n{}",
                maintenance_uncertainty_detail()
            ));
        }
    }
}

fn summary_row(ui: &mut egui::Ui, name: &str, design: &DesignResult) {
    let fixed = &design.fixed;
    ui.strong(name);
    ui.label(years_text(fixed.history_end_s, 1))
        .on_hover_text(outcome_label(&fixed.outcome));
    maintenance_calculated_value(
        ui,
        format!("{:.1}", days(fixed.total_replacement_downtime_s)),
        faris_model::validation::Quantity::Availability,
    );
    let computed = design.computed.summary.as_ref();
    match (design.computed.status, computed) {
        (Status::Evaluated, Some(c)) => {
            maintenance_calculated_value(
                ui,
                format!("{:.1}", days(c.total_replacement_downtime_s)),
                faris_model::validation::Quantity::Availability,
            );
            maintenance_calculated_value(
                ui,
                availability_text(fixed),
                faris_model::validation::Quantity::Availability,
            );
            maintenance_calculated_value(
                ui,
                availability_text(c),
                faris_model::validation::Quantity::Availability,
            );
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
            validated_value(
                ui,
                availability_text(fixed),
                faris_model::validation::Quantity::Availability,
            );
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

const FIXED_COLOR: egui::Color32 = crate::theme::SERIES_FIXED;
const COOLDOWN_COLOR: egui::Color32 = crate::theme::SERIES_COOLDOWN;
const WORK_COLOR: egui::Color32 = crate::theme::SERIES_WORK;

fn legend_swatch(ui: &mut egui::Ui, color: egui::Color32, label: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 1.0, color);
    ui.painter().rect_stroke(
        rect,
        1.0,
        egui::Stroke::new(1.0, crate::theme::palette().chart_axis),
        egui::StrokeKind::Outside,
    );
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
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
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
                maintenance_calculated_value(ui, format!("{:.1}", days(total)), faris_model::validation::Quantity::DecayHeat);
                if event.window_limited {
                    badge::badge(
                        ui,
                        Kind::Partial,
                        "window-limited",
                        "The decay curve ended before reaching q*, so this cooldown is a lower bound.",
                    );
                }
            });
            maintenance_calculated_value(
                ui,
                format!("{:.1}", days(cool)),
                faris_model::validation::Quantity::DecayHeat,
            );
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
                validated_value(
                    ui,
                    format!("{:.2}", days(iteration.max_change_s)),
                    faris_model::validation::Quantity::DecayHeat,
                );
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
                ui.label(text).on_hover_text(validation_hover_text(
                    faris_model::validation::Quantity::DecayHeat,
                ));
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
    egui::ScrollArea::horizontal()
        .id_salt("maintenance-contrasts-scroll")
        .show(ui, |ui| {
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
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use faris_engine::{
        history::HistoryOutcome,
        maintenance::{GoverningShare, IterationRecord, ReplacementSummary},
    };
    use faris_model::history::{
        ComponentClass, EnergyAssumptions, OPERATING_HISTORY_VERSION, OperatingHistoryAssumptions,
        ServiceLimit,
    };
    use faris_model::maintenance::{
        CoolingGrid, MAINTENANCE_ASSUMPTIONS_VERSION, MaintenanceAssumptions, MaintenanceClass,
        Threshold,
    };

    #[test]
    fn duration_tornado_validation_hover_explains_both_engine_quantities() {
        for quantity in [
            faris_model::validation::Quantity::Availability,
            faris_model::validation::Quantity::NetElectricPower,
            faris_model::validation::Quantity::DecayHeat,
        ] {
            let note = faris_model::validation::note(quantity);
            let detail = validation_hover_text(quantity);
            assert!(detail.contains(&note.scope));
            assert!(detail.contains(&note.why));
            assert!(detail.contains(&note.next_step));
            assert!(validation_tag(quantity).contains(note.state.label()));
        }
    }

    #[test]
    fn event_row_renders_uncertainty_for_computed_but_not_authored_only_duration() {
        fn texts(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        texts(shape, out);
                    }
                }
                _ => {}
            }
        }
        fn rendered_event(event: &EventRecord) -> Vec<String> {
            let ctx = egui::Context::default();
            let mut output = ctx.run_ui(Default::default(), |ui| {
                egui::Grid::new("maintenance-event-uncertainty-test")
                    .show(ui, |ui| event_row(ui, event));
            });
            let mut rendered = Vec::new();
            for clipped in &output.shapes {
                texts(&clipped.shape, &mut rendered);
            }
            output.textures_delta.clear();
            rendered
        }

        let calculated = rendered_event(&event(1, Some(42.0), false));
        let calculated_value = calculated.iter().find(|text| text.contains("no interval"));
        assert!(calculated_value.is_some_and(|text| {
            text.contains(&validation_tag(
                faris_model::validation::Quantity::DecayHeat,
            ))
        }));
        let calculated_detail = event_detail(&event(1, Some(42.0), false));
        assert!(
            calculated_detail
                .contains("Reason: the typed maintenance result stores point estimates only")
        );
        assert!(calculated_detail.contains("Next step:"));

        let authored_only = rendered_event(&event(1, None, false));
        assert!(authored_only.iter().any(|text| text == "30.0"));
        assert!(
            !authored_only
                .iter()
                .any(|text| text.contains("no interval"))
        );
    }

    const Y: f64 = JULIAN_YEAR_SECONDS;

    fn duration_assumptions() -> OperatingHistoryAssumptions {
        let limit = |component: &str, response: &str, duration_s: f64, events: Option<Vec<f64>>| {
            ServiceLimit {
                component_id: component.into(),
                class: ComponentClass::Replaceable,
                response_id: response.into(),
                metric: "fluence".into(),
                unit: "n/m2".into(),
                limit: 1.0e20,
                replacement_duration_s: Some(duration_s),
                replacement_durations_s: events,
                provenance: format!("authored source for {component}"),
            }
        };
        OperatingHistoryAssumptions {
            schema_version: OPERATING_HISTORY_VERSION.into(),
            horizon_s: Y,
            maximum_step_s: DAY_S,
            snapshot_interval_s: 30.0 * DAY_S,
            initial_available_tritium_kg: 0.0,
            initial_in_process_tritium_kg: 0.0,
            initial_in_process_release_s: 0.0,
            startup_reserve_kg: 0.0,
            restart_inventory_kg: 0.0,
            recovery_fraction: 0.95,
            processing_delay_s: 0.0,
            operation: Vec::new(),
            planned_outages: Vec::new(),
            imports: Vec::new(),
            service_limits: vec![
                limit(
                    "magnets",
                    "magnet-inner",
                    20.0 * DAY_S,
                    Some(vec![10.0, 15.0]),
                ),
                limit(
                    "magnets",
                    "magnet-outer",
                    20.0 * DAY_S,
                    Some(vec![10.0, 15.0]),
                ),
                limit("blankets", "blanket", 40.0 * DAY_S, None),
                ServiceLimit {
                    component_id: "vacuum-vessel".into(),
                    class: ComponentClass::Permanent,
                    response_id: "vessel-fluence".into(),
                    metric: "fluence".into(),
                    unit: "n/m2".into(),
                    limit: 1.0e20,
                    replacement_duration_s: None,
                    replacement_durations_s: None,
                    provenance: "authored permanent component limit".into(),
                },
            ],
            energy: EnergyAssumptions {
                alpha_deposition_fraction: None,
                thermal_to_electric_efficiency: None,
                transport_heat_recovery_fraction: None,
                auxiliary_power_mw_while_operating: None,
                auxiliary_power_mw_while_off: None,
                provenance: "no energy assumptions".into(),
            },
            provenance: "authored history".into(),
        }
    }

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
            decay_source: None,
            produced_by: None,
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
            "first-wall 82 % (7.0 yr in service), blanket 18 % (3.5 yr in service)"
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
            "calibrated on X to a target of 20.0 d"
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

    fn wait_until_loaded(panel: &mut MaintenancePanel, ctx: &egui::Context) {
        for _ in 0..500 {
            panel.poll(ctx);
            if !panel.is_pending() {
                return;
            }
            crate::thread::sleep(crate::time::Duration::from_millis(10));
        }
        panic!("the load did not finish");
    }

    fn recorded_in(dir: &Path, bytes: &[u8]) -> RecordedMaintenance {
        use sha2::{Digest, Sha256};
        let path = dir.join("maintenance-result.json");
        std::fs::write(&path, bytes).unwrap();
        RecordedMaintenance {
            path,
            sha256: format!("sha256:{:x}", Sha256::digest(bytes)),
        }
    }

    #[test]
    fn the_recorded_result_opens_by_itself_the_first_time_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut result = fixture();
        result.produced_by = Some(faris_engine::maintenance::ProducedBy {
            faris_version: "0.2.0".into(),
        });
        let recorded = recorded_in(dir.path(), &serde_json::to_vec(&result).unwrap());
        let ctx = egui::Context::default();
        let mut panel = MaintenancePanel::default();
        panel.set_recorded(recorded);
        assert!(!panel.shows_result_for_tests());
        panel.offer_recorded(&ctx);
        wait_until_loaded(&mut panel, &ctx);
        assert!(panel.shows_result_for_tests());
        match &panel.state {
            State::Ready(loaded) => {
                assert_eq!(loaded.file_name, "maintenance-result.json");
                assert_eq!(loaded.result.produced_by, result.produced_by);
            }
            _ => panic!("recorded result not shown"),
        }
        // Closing and reopening the window does not reload it over the user's choice.
        panel.state = State::Empty;
        panel.offer_recorded(&ctx);
        assert!(!panel.is_pending());
        assert!(!panel.shows_result_for_tests());
        // The button still works, and the view renders the button and the note.
        panel.load_recorded_in_background(&ctx);
        wait_until_loaded(&mut panel, &ctx);
        assert!(panel.shows_result_for_tests());
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 900.0),
            )),
            ..Default::default()
        };
        ctx.run_ui(input, |ui| panel.view(ui))
            .drop_without_applying_deltas();
    }

    #[test]
    fn a_recorded_result_that_differs_from_the_index_is_not_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorded = recorded_in(dir.path(), &serde_json::to_vec(&fixture()).unwrap());
        recorded.sha256 = format!("sha256:{}", "0".repeat(64));
        let ctx = egui::Context::default();
        let mut panel = MaintenancePanel::default();
        panel.set_recorded(recorded);
        panel.offer_recorded(&ctx);
        wait_until_loaded(&mut panel, &ctx);
        match &panel.state {
            State::Failed(error) => assert!(error.contains("differs from the package index")),
            _ => panic!("a changed file must not load"),
        }
    }

    #[test]
    fn without_a_package_result_nothing_is_offered() {
        let ctx = egui::Context::default();
        let mut panel = MaintenancePanel::default();
        panel.offer_recorded(&ctx);
        assert!(!panel.is_pending());
        assert!(matches!(panel.state, State::Empty));
        assert!(RECORDED_NOTE.contains("to recompute, use Run"));
    }

    #[test]
    fn explanatory_text_makes_no_ranking_claim() {
        let lowered = COMPUTED_MEANING.to_lowercase();
        assert!(lowered.contains("no ranking claim"));
        for word in ["winner", "best", "rank higher"] {
            assert!(!lowered.contains(word), "{word}");
        }
    }

    #[test]
    fn literature_campaign_application_updates_only_the_selected_component_with_authored_provenance()
     {
        let original = duration_assumptions();
        original.validate().unwrap();
        let preset =
            faris_model::remote_handling::maintenance_preset("demo-blanket-divertor-4").unwrap();
        let edited = assumptions_with_campaign_duration(&original, "magnets", &preset).unwrap();

        for limit in edited
            .service_limits
            .iter()
            .filter(|limit| limit.component_id == "magnets")
        {
            assert_eq!(limit.replacement_duration_s, Some(preset.duration_s()));
            assert_eq!(limit.replacement_durations_s, None);
            assert!(
                limit
                    .provenance
                    .contains("authored per-component scenario what-if")
            );
            assert!(limit.provenance.contains(preset.id));
            assert!(limit.provenance.contains("not a per-component measurement"));
        }
        assert_eq!(edited.service_limits[2], original.service_limits[2]);
        assert_eq!(edited.operation, original.operation);
        assert_eq!(edited.planned_outages, original.planned_outages);
        assert_eq!(edited.imports, original.imports);
        assert_eq!(edited.energy, original.energy);
        assert_eq!(edited.horizon_s, original.horizon_s);
        assert!(edited.provenance.contains("authored per-component what-if"));
        assert!(edited.provenance.contains(preset.citation));
        edited.validate().unwrap();
    }

    #[test]
    fn literature_campaign_application_rejects_an_unknown_or_nonreplaceable_component() {
        let original = duration_assumptions();
        let preset = faris_model::remote_handling::maintenance_presets()
            .into_iter()
            .next()
            .unwrap();
        let error = assumptions_with_campaign_duration(&original, "missing", &preset).unwrap_err();
        assert!(error.contains("not a replaceable component"));
        let error =
            assumptions_with_campaign_duration(&original, "vacuum-vessel", &preset).unwrap_err();
        assert!(error.contains("not a replaceable component"));
    }

    #[test]
    fn stale_tornado_result_is_discarded_by_the_real_poll_path() {
        let (sender, receiver) = mpsc::channel();
        let mut panel = MaintenancePanel {
            duration_revision: 2,
            tornado_receiver: Some(receiver),
            ..Default::default()
        };
        sender
            .send((
                1,
                Ok(DurationTornado {
                    base: faris_engine::duration_sensitivity::TornadoOutputs {
                        net_electricity_mwh: None,
                        availability: 0.75,
                    },
                    bars: Vec::new(),
                }),
            ))
            .unwrap();
        panel.poll_tornado(&egui::Context::default());
        assert!(panel.tornado.is_none());
        assert!(panel.tornado_receiver.is_none());
        assert!(panel.tornado_error.is_none());
    }
}
