// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! Viewer for dose-governed maintenance results. The view reports the authored task location,
//! selected estimate method and D1S/R2S comparison state alongside the computed duration.

use eframe::egui;
use faris_engine::jobs::Cancellation;
use faris_engine::maintenance::{
    DoseMaintenanceResult, MAX_DOSE_RESULT_BYTES,
    files::{DoseRunConfig, default_event_dose_worker, run_dose_from_files},
    parse_dose_result, prepare_dose_cad,
    spatial_dose::ComparisonStatus,
};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver, TryRecvError},
    },
};

use crate::design_transport_panel::CadHistoryContext;
use faris_model::validation::{Quantity, note};

const MAX_RESULT_BYTES: u64 = MAX_DOSE_RESULT_BYTES as u64;

enum LoadState {
    Empty,
    Failed(String),
    Ready(DoseMaintenanceResult),
}

pub struct MaintenanceDosePanel {
    pub open: bool,
    state: LoadState,
    dialog: Option<(DoseField, Receiver<Option<PathBuf>>)>,
    dialog_error: Option<String>,
    loading: Option<Receiver<Result<DoseMaintenanceResult, String>>>,
    file_name: String,
    selected_design: Option<String>,
    inputs: DoseInputs,
    run: Option<DoseRunState>,
    prepare: Option<DosePrepareState>,
    prepare_config: String,
    prepare_message: Option<String>,
    cad_context: Option<CadHistoryContext>,
    cad_history_assumptions: String,
    cad_history_assumptions_note: Option<String>,
    cad_dose_physics: String,
    cad_design_file: String,
    cad_designs_inputs: Option<(String, String, String)>,
    last_run_inputs: Option<DoseInputs>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct DoseInputs {
    designs: String,
    assumptions: String,
    curves: String,
    output: String,
}

struct DoseRunState {
    cancel: Cancellation,
    receiver: Receiver<Result<(PathBuf, DoseMaintenanceResult), String>>,
    progress: Arc<std::sync::Mutex<Option<String>>>,
    inputs: DoseInputs,
}

struct DosePrepareState {
    cancel: Cancellation,
    receiver: Receiver<Result<PathBuf, String>>,
}

#[derive(Clone, Copy)]
enum DoseField {
    Designs,
    Assumptions,
    Curves,
    Output,
    PrepareConfig,
    Result,
    CadHistoryAssumptions,
    CadDosePhysics,
    CadDesignFile,
}

impl Default for MaintenanceDosePanel {
    fn default() -> Self {
        Self {
            open: false,
            state: LoadState::Empty,
            dialog: None,
            dialog_error: None,
            loading: None,
            file_name: String::new(),
            selected_design: None,
            inputs: DoseInputs::default(),
            run: None,
            prepare: None,
            prepare_config: String::new(),
            prepare_message: None,
            cad_context: None,
            cad_history_assumptions: String::new(),
            cad_history_assumptions_note: None,
            cad_dose_physics: String::new(),
            cad_design_file: String::new(),
            cad_designs_inputs: None,
            last_run_inputs: None,
        }
    }
}

pub fn parse_result(bytes: &[u8]) -> Result<DoseMaintenanceResult, String> {
    parse_dose_result(bytes).map_err(|e| format!("not a readable dose maintenance result: {e}"))
}

fn load(path: PathBuf) -> Result<DoseMaintenanceResult, String> {
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > MAX_RESULT_BYTES {
        return Err("dose maintenance result must be a regular file no larger than 16 MiB".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(MAX_RESULT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_RESULT_BYTES {
        return Err("dose maintenance result exceeds the 16 MiB input limit".into());
    }
    parse_result(&bytes)
}

fn run_inputs_changed(snapshot: Option<&DoseInputs>, current: &DoseInputs) -> bool {
    snapshot.is_some_and(|snapshot| snapshot != current)
}

impl MaintenanceDosePanel {
    pub fn prefill_cad_history(&mut self, context: CadHistoryContext) -> Result<(), String> {
        if self.is_pending() {
            return Err("Finish or cancel the current dose-physics preparation, run, or result load before switching CAD design.".into());
        }
        self.cad_history_assumptions.clear();
        self.cad_history_assumptions_note = Some(
            match faris_engine::design::resolve_cad_operating_context(&context.design, None) {
                Ok(operating) => format!(
                    "Design-linked assumptions: {} · SHA-256 {}",
                    operating.history_assumptions_path.display(),
                    operating.history_assumptions_sha256
                ),
                Err(error) => format!(
                    "The design has no usable linked assumptions ({error}). Choose a planning override to continue."
                ),
            },
        );
        self.cad_dose_physics.clear();
        self.prepare_message = None;
        self.inputs.designs.clear();
        self.cad_designs_inputs = None;
        self.last_run_inputs = None;
        self.file_name.clear();
        self.selected_design = None;
        self.state = LoadState::Empty;
        self.cad_design_file = crate::maintenance_run::unused_path(
            &context
                .history_rates
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("faris-maintenance-cad-dose-designs.json"),
        )
        .display()
        .to_string();
        self.cad_context = Some(context);
        self.open = true;
        Ok(())
    }

    pub fn is_pending(&self) -> bool {
        self.run.is_some()
            || self.prepare.is_some()
            || self.loading.is_some()
            || self.dialog.is_some()
    }

    pub fn is_running(&self) -> bool {
        self.run.is_some() || self.prepare.is_some()
    }

    /// Read-only state for bounded native interface checks. The snapshot exposes current CAD
    /// identities and real worker/error states without injecting dose results or launching work.
    pub fn interface_status(&self) -> serde_json::Value {
        let cad_designs_inputs_current = self.cad_designs_inputs.as_ref().is_some_and(|expected| {
            expected.0 == self.inputs.designs.trim()
                && expected.1 == self.cad_history_assumptions.trim()
                && expected.2 == self.cad_dose_physics.trim()
        });
        let (result_state, error) = match &self.state {
            LoadState::Empty => ("empty", None),
            LoadState::Failed(message) => ("failed", Some(message.as_str())),
            LoadState::Ready(_) => ("ready", None),
        };
        serde_json::json!({
            "open": self.open,
            "pending": self.is_pending(),
            "preparation_pending": self.prepare.is_some(),
            "dose_run_pending": self.run.is_some(),
            "result_load_pending": self.loading.is_some(),
            "preparation_cancel_requested": self.prepare.as_ref().is_some_and(|run| run.cancel.is_cancelled()),
            "dose_run_cancel_requested": self.run.as_ref().is_some_and(|run| run.cancel.is_cancelled()),
            "cad_history_context": self.cad_context.as_ref().map(|context| serde_json::json!({
                "design": context.design,
                "history_rates": context.history_rates,
                "design_id": context.design_id,
                "design_sha256": context.design_sha256,
                "fusion_power_mw": context.fusion_power_mw,
                "history_components": context.history_components,
            })),
            "cad_design_file_created": self.cad_designs_inputs.is_some(),
            "cad_design_inputs_current": cad_designs_inputs_current,
            "result_inputs_changed_since_run": run_inputs_changed(self.last_run_inputs.as_ref(), &self.inputs),
            "result_state": result_state,
            "error": error,
        })
    }

    fn cad_design_inputs_match(&self) -> bool {
        self.cad_context.is_some()
            && self.cad_designs_inputs.as_ref()
                == Some(&(
                    self.inputs.designs.trim().to_owned(),
                    self.cad_history_assumptions.trim().to_owned(),
                    self.cad_dose_physics.trim().to_owned(),
                ))
    }

    /// Add the CAD dose result to the app's shared Compare step with its authored identity.
    pub fn compare_section(&self, ui: &mut egui::Ui) {
        if !self.cad_design_inputs_match() {
            return;
        }
        let LoadState::Ready(result) = &self.state else {
            return;
        };
        egui::CollapsingHeader::new("CAD whole-life: computed versus fixed downtime")
            .default_open(true)
            .show(ui, |ui| {
                ui.label("This is the checked CAD design and its hash-bound transport history. Fixed durations are the authored maintenance baseline; computed durations use the selected event dose method. D1S/R2S comparison state is shown per event in the dose result window.");
                if !self.cad_history_assumptions.trim().is_empty() {
                    ui.label(format!(
                        "Planning override: {}",
                        self.cad_history_assumptions.trim()
                    ));
                } else if let Some(source) = &self.cad_history_assumptions_note {
                    ui.label(source);
                }
                for (name, design) in &result.designs {
                    ui.strong(name);
                    let prefix = format!("design/{name}/");
                    for input in ["design_file", "history_run", "history_assumptions", "dose_physics"] {
                        if let Some(hash) = result.inputs.get(&format!("{prefix}{input}")) {
                            ui.label(format!("{input} SHA-256 {}", &hash[..12.min(hash.len())]));
                        }
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!(
                            "Fixed-duration model downtime: {:.3} d",
                            design.fixed.total_replacement_downtime_s / 86_400.0
                        ));
                        ui.label(format!("Computed state: {:?}", design.computed.status));
                        if let Some(summary) = &design.computed.summary {
                            ui.label(format!(
                                "Computed downtime: {:.3} d",
                                summary.total_replacement_downtime_s / 86_400.0
                            ));
                            ui.label(format!(
                                "Availability fixed/computed: {:.2}% / {:.2}%",
                                100.0 * design.fixed.availability,
                                100.0 * summary.availability
                            ));
                            if let (Some(fixed), Some(computed)) = (
                                design.fixed.lifetime_net_electricity_mwh,
                                summary.lifetime_net_electricity_mwh,
                            ) {
                                ui.label(format!(
                                    "Net electricity fixed/computed: {:.3} / {:.3} TWh",
                                    fixed / 1.0e6,
                                    computed / 1.0e6
                                ));
                            }
                        } else if let Some(reason) = &design.computed.not_evaluated {
                            ui.label(format!("Why: {} Next step: {}", reason.reason, reason.next_step));
                        }
                    });
                    validation_detail(ui, Quantity::Availability, "Downtime and availability");
                    if design
                        .computed
                        .summary
                        .as_ref()
                        .is_some_and(|summary| summary.lifetime_net_electricity_mwh.is_some())
                    {
                        validation_detail(ui, Quantity::NetElectricPower, "Net electricity");
                    }
                }
            });
    }

    /// Expose the actual CAD-derived history on the app's Operate step.
    pub fn history_section(&self, ui: &mut egui::Ui) {
        if !self.cad_design_inputs_match() {
            return;
        }
        let LoadState::Ready(result) = &self.state else {
            return;
        };
        egui::CollapsingHeader::new("Checked CAD whole-life history")
            .default_open(true)
            .show(ui, |ui| {
                ui.label("These histories use the checked CAD transport rates, the bound whole-model heating response, and event-specific dose durations. The existing in-app arrangement history remains a separate scenario.");
                if !self.cad_history_assumptions.trim().is_empty() {
                    ui.label(format!("Planning override: {}", self.cad_history_assumptions.trim()));
                } else if let Some(source) = &self.cad_history_assumptions_note {
                    ui.label(source);
                }
                for (name, design) in &result.designs {
                    ui.strong(name);
                    let prefix = format!("design/{name}/");
                    for input in ["design_file", "history_run", "history_assumptions", "dose_physics"] {
                        if let Some(hash) = result.inputs.get(&format!("{prefix}{input}")) {
                            ui.label(format!("{input} SHA-256 {}", &hash[..12.min(hash.len())]));
                        }
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("Computed status: {:?}", design.computed.status));
                        if let Some(summary) = &design.computed.summary {
                            ui.label(format!(
                                "Replacement downtime: {:.3} d",
                                summary.total_replacement_downtime_s / 86_400.0
                            ));
                            ui.label(format!("Availability: {:.2}%", 100.0 * summary.availability));
                            if let Some(net) = summary.lifetime_net_electricity_mwh {
                                ui.label(format!("Net electricity: {:.3} TWh", net / 1.0e6));
                                validation_detail(ui, Quantity::NetElectricPower, "Net electricity");
                            }
                            validation_detail(ui, Quantity::Availability, "Downtime and availability");
                        } else if let Some(reason) = &design.computed.not_evaluated {
                            ui.label(format!("Why: {} Next step: {}", reason.reason, reason.next_step));
                        }
                    });
                }
            });
    }

    pub fn window(&mut self, ctx: &egui::Context, actinv_executable: Option<&Path>) {
        if !self.open {
            return;
        }
        if let Some((field, receiver)) = &self.dialog {
            match receiver.try_recv() {
                Ok(path) => {
                    let field = *field;
                    self.dialog = None;
                    if let Some(path) = path {
                        match field {
                            DoseField::Result => {
                                self.cad_context = None;
                                self.cad_designs_inputs = None;
                                self.last_run_inputs = None;
                                let name = path.file_name().map_or_else(
                                    || path.display().to_string(),
                                    |n| n.to_string_lossy().into_owned(),
                                );
                                let (tx, rx) = mpsc::channel();
                                match crate::thread::Builder::new()
                                    .name("faris-dose-result-reader".into())
                                    .spawn(move || {
                                        let _ = tx.send(load(path));
                                    }) {
                                    Ok(_) => {
                                        self.file_name = name;
                                        self.loading = Some(rx);
                                    }
                                    Err(error) => {
                                        self.state = LoadState::Failed(format!(
                                            "Could not start result reader: {error}"
                                        ));
                                    }
                                }
                            }
                            other => {
                                let value = path.display().to_string();
                                match other {
                                    DoseField::Designs => self.inputs.designs = value,
                                    DoseField::Assumptions => self.inputs.assumptions = value,
                                    DoseField::Curves => self.inputs.curves = value,
                                    DoseField::Output => self.inputs.output = value,
                                    DoseField::PrepareConfig => self.prepare_config = value,
                                    DoseField::CadHistoryAssumptions => {
                                        self.cad_history_assumptions = value
                                    }
                                    DoseField::CadDosePhysics => self.cad_dose_physics = value,
                                    DoseField::CadDesignFile => self.cad_design_file = value,
                                    DoseField::Result => unreachable!(),
                                }
                            }
                        }
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    self.dialog = None;
                    self.dialog_error = Some("File dialog stopped before returning a path.".into());
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(receiver) = &self.loading {
            match receiver.try_recv() {
                Ok(Ok(result)) => {
                    self.selected_design = result.designs.keys().next().cloned();
                    self.state = LoadState::Ready(result);
                    self.loading = None;
                }
                Ok(Err(error)) => {
                    self.state = LoadState::Failed(error);
                    self.loading = None;
                }
                Err(TryRecvError::Disconnected) => {
                    self.state = LoadState::Failed("dose result reader stopped".into());
                    self.loading = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(run) = &self.run {
            match run.receiver.try_recv() {
                Ok(Ok((output, result))) => {
                    self.file_name = output.display().to_string();
                    self.selected_design = result.designs.keys().next().cloned();
                    self.last_run_inputs = self.run.as_ref().map(|run| run.inputs.clone());
                    self.state = LoadState::Ready(result);
                    self.run = None;
                }
                Ok(Err(error)) => {
                    self.state = LoadState::Failed(error);
                    self.run = None;
                }
                Err(TryRecvError::Disconnected) => {
                    self.state = LoadState::Failed("dose maintenance worker stopped".into());
                    self.run = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(prepare) = &self.prepare {
            match prepare.receiver.try_recv() {
                Ok(Ok(path)) => {
                    self.prepare_message =
                        Some(format!("Prepared physics manifest: {}", path.display()));
                    self.cad_dose_physics = path.display().to_string();
                    self.prepare = None;
                }
                Ok(Err(error)) => {
                    self.state = LoadState::Failed(error);
                    self.prepare = None;
                }
                Err(TryRecvError::Disconnected) => {
                    self.state = LoadState::Failed("CAD dose preparation worker stopped".into());
                    self.prepare = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        let mut open = self.open;
        egui::Window::new("Dose-governed maintenance result").open(&mut open).default_width(780.0).show(ctx, |ui| {
            if let Some(error) = &self.dialog_error {
                ui.colored_label(egui::Color32::from_rgb(180, 45, 45), error);
            }
            if !crate::browser::IS_BROWSER {
                ui.collapsing("Run dose-governed maintenance", |ui| {
                    ui.label("Prepare dose physics from a passing checked CAD Design and ImportReport. The preparation config authors the solid-to-history-component map and spatial inputs; FARIS clones materials per solid and builds the full-geometry model.");
                    ui.add_enabled_ui(!self.is_pending(), |ui| {
                        path_row(ui, "CAD dose preparation config", &mut self.prepare_config, DoseField::PrepareConfig, &mut self.dialog, &mut self.dialog_error);
                    });
                    ui.horizontal(|ui| {
                        if !self.is_pending() && ui.button("Prepare CAD dose physics").clicked() {
                            match start_dose_prepare(&self.prepare_config, self.cad_context.clone(), actinv_executable) {
                                Ok(state) => { self.prepare = Some(state); self.prepare_message = None; }
                                Err(error) => self.state = LoadState::Failed(error),
                            }
                        }
                        if let Some(prepare) = &self.prepare {
                            if ui.button("Cancel preparation").clicked() { prepare.cancel.cancel(); }
                            ui.spinner();
                        }
                    });
                    if let Some(message) = &self.prepare_message { ui.label(message); }
                    if let Some(context) = &self.cad_context {
                        ui.separator();
                        ui.strong(format!("Checked CAD design: {}", context.design_id));
                        ui.label(format!("Source power: {:.3} MW", context.fusion_power_mw));
                        ui.label(format!("Design SHA-256: {}", context.design_sha256));
                        ui.label(format!("Verified same-batch history rates: {}", context.history_rates.display()));
                        ui.weak("The design-linked OperatingHistoryAssumptions are used by default. An explicit override changes planning assumptions only; source power and transport identity remain bound to this CAD design.");
                        ui.label("The 3D study view behind this window is separate; dose geometry comes from this checked CAD design.");
                        if let Some(note) = &self.cad_history_assumptions_note {
                            ui.weak(note);
                        }
                        ui.weak("Dose preparation must use the same solid-to-component map and cross-section index as the transport run.");
                        for (component, solids) in &context.history_components {
                            ui.label(format!("{component}: {}", solids.join(", ")));
                        }
                        ui.add_enabled_ui(!self.is_pending(), |ui| {
                        path_row(ui, "History assumptions override (optional)", &mut self.cad_history_assumptions, DoseField::CadHistoryAssumptions, &mut self.dialog, &mut self.dialog_error);
                        path_row(ui, "Prepared dose-physics manifest", &mut self.cad_dose_physics, DoseField::CadDosePhysics, &mut self.dialog, &mut self.dialog_error);
                        path_row(ui, "New CAD designs file", &mut self.cad_design_file, DoseField::CadDesignFile, &mut self.dialog, &mut self.dialog_error);
                        if ui.button("Create hash-bound CAD dose design input").clicked() {
                            match write_cad_designs(
                                context,
                                &self.cad_history_assumptions,
                                &self.cad_dose_physics,
                                &self.cad_design_file,
                            ) {
                                Ok(path) => {
                                    let value = path.display().to_string();
                                    self.inputs.designs = value.clone();
                                    self.cad_designs_inputs = Some((
                                        value,
                                        self.cad_history_assumptions.trim().to_owned(),
                                        self.cad_dose_physics.trim().to_owned(),
                                    ));
                                }
                                Err(error) => self.state = LoadState::Failed(error),
                            }
                        }
                        });
                    }
                    ui.label("Provide event-specific D1S/R2S spatial curves. FARIS computes the history coupling and duration; this workflow does not run a general D1S/R2S transport solver.");
                    ui.add_enabled_ui(!self.is_pending(), |ui| {
                        path_row(ui, "Designs", &mut self.inputs.designs, DoseField::Designs, &mut self.dialog, &mut self.dialog_error);
                        path_row(ui, "Dose limits and calculation settings", &mut self.inputs.assumptions, DoseField::Assumptions, &mut self.dialog, &mut self.dialog_error);
                        path_row(ui, "Optional spatial dose curve cache", &mut self.inputs.curves, DoseField::Curves, &mut self.dialog, &mut self.dialog_error);
                        path_row(ui, "New result file", &mut self.inputs.output, DoseField::Output, &mut self.dialog, &mut self.dialog_error);
                    });
                    ui.horizontal(|ui| {
                        if !self.is_pending() && ui.button("Run bounded dose coupling").clicked() {
                            let cad_designs_match = self.cad_context.is_none()
                                || self.cad_designs_inputs.as_ref()
                                    == Some(&(
                                        self.inputs.designs.trim().to_owned(),
                                        self.cad_history_assumptions.trim().to_owned(),
                                        self.cad_dose_physics.trim().to_owned(),
                                    ));
                            let started = if cad_designs_match {
                                start_dose_run(&self.inputs, actinv_executable)
                            } else {
                                Err("The CAD design inputs changed after the guided setup. Recreate the hash-bound CAD dose design file before running.".into())
                            };
                            match started {
                                Ok(run) => {
                                    self.run = Some(run);
                                    self.last_run_inputs = None;
                                    self.state = LoadState::Empty;
                                }
                                Err(error) => self.state = LoadState::Failed(error),
                            }
                        }
                        if let Some(run) = &self.run { if ui.button("Cancel").clicked() { run.cancel.cancel(); } ui.spinner(); ui.label(run.progress.lock().ok().and_then(|p| p.clone()).unwrap_or_else(|| "Starting dose coupling".into())); }
                    });
                });
                ui.separator();
            }
            ui.horizontal(|ui| {
                if !self.is_pending() && ui.button("Open result…").clicked() {
                    let (tx, rx) = mpsc::channel(); self.dialog = Some((DoseField::Result, rx));
                    match crate::thread::Builder::new()
                        .name("faris-dose-result-dialog".into())
                        .spawn(move || {
                            let path = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file();
                            let _ = tx.send(path);
                        })
                    {
                        Ok(_) => self.dialog_error = None,
                        Err(error) => {
                            self.dialog = None;
                            self.dialog_error = Some(format!("Could not start result file dialog: {error}"));
                        }
                    }
                }
                if !self.file_name.is_empty() { ui.label(&self.file_name); }
                if self.loading.is_some() { ui.spinner(); ui.label("Reading result"); }
            });
            match &self.state {
                LoadState::Empty => { ui.label("Open a faris-maintenance-result/v0.2 file to inspect authored spatial-dose scenarios."); },
                LoadState::Failed(error) => { ui.colored_label(egui::Color32::from_rgb(180, 45, 45), error); },
                LoadState::Ready(result) => {
                    if run_inputs_changed(self.last_run_inputs.as_ref(), &self.inputs) {
                        ui.colored_label(
                            egui::Color32::from_rgb(170, 95, 0),
                            "This result was calculated from the input snapshot captured when the run started. Current input fields have since changed.",
                        );
                    }
                    show_result(ui, result, &mut self.selected_design)
                }
            }
        });
        self.open = open;
    }
}

fn validation_detail(ui: &mut egui::Ui, quantity: Quantity, label: &str) {
    let mark = faris_model::validation::ValidationMark::current(quantity);
    let note = note(quantity);
    ui.label(format!(
        "{label} validation: {} · scope: {} · why: {} · next: {}",
        mark.state.label(),
        note.scope,
        note.why,
        note.next_step
    ));
}

fn write_cad_designs(
    context: &CadHistoryContext,
    history_assumptions: &str,
    dose_physics: &str,
    output: &str,
) -> Result<PathBuf, String> {
    let required = |label: &str, text: &str| -> Result<PathBuf, String> {
        let path = PathBuf::from(text.trim());
        if !path.is_file() {
            return Err(format!("choose an existing {label}"));
        }
        path.canonicalize().map_err(|e| format!("{label}: {e}"))
    };
    let history_assumptions = if history_assumptions.trim().is_empty() {
        None
    } else {
        Some(required(
            "operating history assumptions override",
            history_assumptions,
        )?)
    };
    let operating = faris_engine::design::resolve_cad_operating_context(
        &context.design,
        history_assumptions.as_deref(),
    )?;
    if operating.design_id != context.design_id
        || operating.design_sha256 != context.design_sha256
        || operating.fusion_power_mw.to_bits() != context.fusion_power_mw.to_bits()
    {
        return Err("the checked CAD source changed after its transport history run".into());
    }
    let dose_physics = required("prepared CAD dose-physics manifest", dose_physics)?;
    let output = PathBuf::from(output.trim());
    if output.as_os_str().is_empty() || output.exists() {
        return Err(
            "choose a new CAD designs file path; existing files are never overwritten".into(),
        );
    }
    let path_value = |path: &Path| -> Result<String, String> {
        path.canonicalize()
            .map(|p| p.display().to_string())
            .map_err(|e| e.to_string())
    };
    let value = serde_json::json!({
        "schema_version": faris_engine::maintenance::files::CAD_DOSE_DESIGNS_VERSION,
        "designs": {
            (context.design_id.clone()): {
                "design_file": path_value(&context.design)?,
                "history_run": path_value(&context.history_rates)?,
                "history_assumptions": history_assumptions.as_ref().map(|path| path.display().to_string()),
                "dose_physics": dose_physics.display().to_string(),
            }
        }
    });
    let mut bytes = serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    faris_engine::atomic_file::write_new(&output, &bytes)
        .map_err(|e| format!("cannot write {}: {e}", output.display()))?;
    Ok(output)
}

fn path_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    field: DoseField,
    pending: &mut Option<(DoseField, Receiver<Option<PathBuf>>)>,
    error_message: &mut Option<String>,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::TextEdit::singleline(value).desired_width(420.0));
        if ui.button("Browse…").clicked() && pending.is_none() {
            let (tx, rx) = mpsc::channel();
            match crate::thread::Builder::new()
                .name("faris-dose-input-dialog".into())
                .spawn(move || {
                    let dialog = rfd::FileDialog::new().add_filter("JSON", &["json"]);
                    let path = if matches!(field, DoseField::Output | DoseField::CadDesignFile) {
                        dialog.save_file()
                    } else {
                        dialog.pick_file()
                    };
                    let _ = tx.send(path);
                }) {
                Ok(_) => {
                    *pending = Some((field, rx));
                    *error_message = None;
                }
                Err(error) => {
                    *error_message = Some(format!("Could not start file dialog: {error}"));
                }
            }
        }
    });
}

fn start_dose_run(
    inputs: &DoseInputs,
    actinv_executable: Option<&Path>,
) -> Result<DoseRunState, String> {
    let actinv_executable = required_actinv_executable(actinv_executable)?;
    let designs = required_path("designs", &inputs.designs)?;
    let assumptions = required_path("assumptions", &inputs.assumptions)?;
    let curves = optional_path(&inputs.curves);
    let output = required_path("new result file", &inputs.output)?;
    if output.exists() {
        return Err(format!("refusing to overwrite {}", output.display()));
    }
    let worker_script = default_event_dose_worker().ok_or(
        "OpenMC event-dose worker script was not found in the application package or source tree",
    )?;
    let config = DoseRunConfig {
        designs,
        assumptions,
        spatial_dose_curves: curves,
        python: PathBuf::from("python3"),
        worker_script,
        work_dir: output.with_extension("dose-work"),
        actinv_executable: Some(actinv_executable),
    };
    let cancel = Cancellation::default();
    let worker_cancel = cancel.clone();
    let progress = Arc::new(std::sync::Mutex::new(None));
    let worker_progress = progress.clone();
    let (tx, receiver) = mpsc::channel();
    crate::thread::Builder::new()
        .name("faris-dose-history-run".into())
        .spawn(move || {
            let result = run_dose_from_files(
                &config,
                &worker_cancel,
                Arc::new(move |p| {
                    if let Ok(mut current) = worker_progress.lock() {
                        *current = Some(p.line());
                    }
                }),
            )
            .and_then(|result| {
                let mut bytes = serde_json::to_vec_pretty(&result).map_err(|e| e.to_string())?;
                if bytes.len() >= MAX_DOSE_RESULT_BYTES {
                    return Err("dose maintenance result exceeds the 16 MiB output limit".into());
                }
                bytes.push(b'\n');
                faris_engine::atomic_file::write_new(&output, &bytes)
                    .map_err(|e| format!("cannot write {}: {e}", output.display()))?;
                Ok((output, result))
            });
            let _ = tx.send(result);
        })
        .map_err(|error| format!("could not start dose history worker: {error}"))?;
    Ok(DoseRunState {
        cancel,
        receiver,
        progress,
        inputs: inputs.clone(),
    })
}

fn start_dose_prepare(
    config: &str,
    cad_context: Option<CadHistoryContext>,
    actinv_executable: Option<&Path>,
) -> Result<DosePrepareState, String> {
    let actinv_executable = required_actinv_executable(actinv_executable)?;
    let config = required_path("dose CAD preparation config", config)?;
    let worker = default_event_dose_worker().ok_or(
        "OpenMC event-dose worker script was not found in the application package or source tree",
    )?;
    let cancel = Cancellation::default();
    let worker_cancel = cancel.clone();
    let python = PathBuf::from("python3");
    let (tx, receiver) = mpsc::channel();
    crate::thread::Builder::new()
        .name("faris-dose-cad-preparation".into())
        .spawn(move || {
            let result = cad_context
                .as_ref()
                .map_or(Ok(()), |context| {
                    verify_cad_prepare_binding(&config, context)
                })
                .and_then(|()| {
                    prepare_dose_cad(
                        &config,
                        &python,
                        &worker,
                        Some(&actinv_executable),
                        &worker_cancel,
                    )
                });
            let _ = tx.send(result);
        })
        .map_err(|error| format!("could not start CAD dose preparation worker: {error}"))?;
    Ok(DosePrepareState { cancel, receiver })
}

fn required_actinv_executable(path: Option<&Path>) -> Result<PathBuf, String> {
    let path = path.ok_or_else(|| {
        "Set the ACTINV executable in Tools settings before starting this operation.".to_string()
    })?;
    if !path.is_file() {
        return Err(format!(
            "The ACTINV executable in Tools settings is not a file: {}",
            path.display()
        ));
    }
    path.canonicalize().map_err(|error| {
        format!(
            "Cannot resolve ACTINV executable {}: {error}",
            path.display()
        )
    })
}

fn verify_cad_prepare_binding(
    config_path: &Path,
    context: &CadHistoryContext,
) -> Result<(), String> {
    let bytes = faris_engine::reactor::read_json_bytes(config_path)
        .map_err(|e| format!("CAD dose preparation config: {e}"))?;
    let config: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("CAD dose preparation config: {e}"))?;
    if config["schema_version"] != "faris-maintenance-dose-cad-config/v0.2" {
        return Err(
            "CAD dose preparation config must use faris-maintenance-dose-cad-config/v0.2".into(),
        );
    }
    let base = config_path.parent().unwrap_or_else(|| Path::new("."));
    let (design_hash, _) = faris_study::sha256_file(&context.design)
        .map_err(|e| format!("cannot recheck CAD design: {e}"))?;
    if !design_hash.eq_ignore_ascii_case(&context.design_sha256) {
        return Err("CAD design changed after the transport history run; rerun transport before dose preparation".into());
    }
    let bound_path = |value: &serde_json::Value, label: &str| -> Result<PathBuf, String> {
        let text = value
            .as_str()
            .ok_or_else(|| format!("CAD dose config has no {label} path"))?;
        let path = PathBuf::from(text);
        let path = if path.is_absolute() {
            path
        } else {
            base.join(path)
        };
        path.canonicalize()
            .map_err(|e| format!("CAD dose {label} path: {e}"))
    };
    for (key, expected, label) in [
        ("design_file", &context.design, "design"),
        ("import_report", &context.check_report, "check report"),
    ] {
        if bound_path(&config[key], label)? != expected.canonicalize().map_err(|e| e.to_string())? {
            return Err(format!(
                "CAD dose config {label} does not match the checked transport run"
            ));
        }
    }
    let xs = bound_path(&config["cross_sections_xml"]["path"], "cross-section index")?;
    if xs
        != context
            .cross_sections
            .canonicalize()
            .map_err(|e| e.to_string())?
    {
        return Err(
            "CAD dose config cross-section index does not match the transport history".into(),
        );
    }
    let configured_hash = config["cross_sections_xml"]["sha256"]
        .as_str()
        .ok_or("CAD dose config has no cross-section index SHA-256")?;
    if !configured_hash.eq_ignore_ascii_case(&context.cross_sections_sha256) {
        return Err(
            "CAD dose config cross-section hash does not match the transport history".into(),
        );
    }
    let (current_hash, _) = faris_study::sha256_file(&xs)
        .map_err(|e| format!("cannot recheck cross-section index: {e}"))?;
    if !current_hash.eq_ignore_ascii_case(&context.cross_sections_sha256) {
        return Err(
            "cross-section index changed after CAD transport; rerun the history-rate calculation"
                .into(),
        );
    }
    let actual: std::collections::BTreeMap<String, String> =
        serde_json::from_value(config["solid_history_components"].clone())
            .map_err(|e| format!("CAD dose solid-to-component map: {e}"))?;
    let mut expected = std::collections::BTreeMap::new();
    for (component, solids) in &context.history_components {
        for solid in solids {
            if expected.insert(solid.clone(), component.clone()).is_some() {
                return Err(format!(
                    "CAD history map assigns solid {solid} more than once"
                ));
            }
        }
    }
    if actual != expected {
        return Err(
            "CAD dose solid-to-component map differs from the transport history map".into(),
        );
    }
    Ok(())
}

fn required_path(label: &str, value: &str) -> Result<PathBuf, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(format!("choose {label}"))
    } else {
        Ok(Path::new(trimmed).to_path_buf())
    }
}

fn optional_path(value: &str) -> Option<PathBuf> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| Path::new(trimmed).to_path_buf())
}

fn show_result(ui: &mut egui::Ui, result: &DoseMaintenanceResult, selected: &mut Option<String>) {
    ui.label("Criterion: ICRP-116 AP photon effective dose rate at the authored task location (Sv/h). Missing curves are computed with the bounded OpenMC event worker when a design supplies a dose-physics adapter; heterogeneous histories use R2S with D1S marked NOT_EVALUATED.");
    let names: Vec<_> = result.designs.keys().cloned().collect();
    if selected
        .as_ref()
        .is_none_or(|s| !result.designs.contains_key(s))
    {
        *selected = names.first().cloned();
    }
    if names.len() > 1 {
        egui::ComboBox::from_label("Design")
            .selected_text(selected.as_deref().unwrap_or("Choose design"))
            .show_ui(ui, |ui| {
                for name in names {
                    ui.selectable_value(selected, Some(name.clone()), name);
                }
            });
    }
    let Some(name) = selected.as_ref() else {
        ui.label("No designs in this result.");
        return;
    };
    let Some(design) = result.designs.get(name) else {
        return;
    };
    ui.heading(name);
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "Fixed downtime: {:.3} days",
            design.fixed.total_replacement_downtime_s / 86_400.0
        ));
        ui.label(format!("Computed status: {:?}", design.computed.status));
        if let Some(summary) = &design.computed.summary {
            ui.label(format!(
                "Computed: {:.3} days",
                summary.total_replacement_downtime_s / 86_400.0
            ));
        }
        ui.label(format!(
            "Curve comparison: {:?}",
            design.computed.validation
        ));
    });
    if let Some(ne) = &design.computed.not_evaluated {
        ui.colored_label(
            egui::Color32::from_rgb(165, 90, 20),
            format!("NOT_EVALUATED: {} Next step: {}", ne.reason, ne.next_step),
        );
    }
    validation_detail(
        ui,
        Quantity::ShutdownDoseRate,
        "Computed outage and shutdown-dose rates",
    );
    validation_detail(ui, Quantity::ShutdownDoseRatio, "D1S/R2S comparison ratios");
    validation_detail(
        ui,
        Quantity::Availability,
        "Fixed/computed downtime and availability",
    );
    if design
        .computed
        .summary
        .as_ref()
        .is_some_and(|summary| summary.lifetime_net_electricity_mwh.is_some())
    {
        validation_detail(ui, Quantity::NetElectricPower, "Net electricity");
    }
    egui::ScrollArea::both().max_height(460.0).show(ui, |ui| {
        egui::Grid::new("dose-maintenance-events")
            .striped(true)
            .show(ui, |ui| {
                for heading in [
                    "Class / replacement",
                    "Task location",
                    "Limit source",
                    "Selected method",
                    "Dose limit (Sv/h)",
                    "Cooldown / work",
                    "Duration used",
                    "D1S / R2S central ratio",
                    "Comparison",
                    "Crossing uncertainty",
                ] {
                    ui.strong(heading);
                }
                ui.end_row();
                for iteration in &design.computed.iterations {
                    for event in &iteration.events {
                        ui.label(format!(
                            "{} / {} #{}",
                            event.class, event.component, event.replacement_k
                        ));
                        ui.label(format!(
                            "{} · {}…",
                            event.location_id,
                            &event.location_sha256[..8.min(event.location_sha256.len())]
                        ));
                        ui.label(&event.dose_limit_source);
                        let (method, ratio, comparison) = event
                            .curve
                            .as_ref()
                            .and_then(|curve| {
                                curve.points.first().map(|point| {
                                    (
                                        format!("{:?}", point.selected_method),
                                        point
                                            .d1s_over_r2s
                                            .map_or("undefined".into(), |v| format!("{v:.4}")),
                                        format!("{:?}", curve.comparison),
                                    )
                                })
                            })
                            .unwrap_or_else(|| ("—".into(), "—".into(), "NOT_EVALUATED".into()));
                        ui.label(method);
                        ui.label(format!("{:.3e}", event.dose_limit_sv_h));
                        ui.label(format!(
                            "{} + {:.3} d",
                            event
                                .cooldown_s
                                .map_or("—".into(), |s| format!("{:.3}", s / 86_400.0)),
                            event.work_s / 86_400.0
                        ));
                        ui.label(format!("{:.3} d", event.duration_used_s / 86_400.0));
                        ui.label(ratio);
                        let color = if comparison == "INCONCLUSIVE_COVARIANCE" {
                            egui::Color32::from_rgb(165, 90, 20)
                        } else if comparison == "OUTSIDE_BAND" || event.not_evaluated.is_some() {
                            egui::Color32::from_rgb(180, 45, 45)
                        } else {
                            ui.visuals().text_color()
                        };
                        ui.colored_label(color, comparison);
                        ui.label(
                            event
                                .curve
                                .as_ref()
                                .and_then(|c| c.points.first())
                                .map_or_else(
                                    || "NOT_EVALUATED".into(),
                                    |p| p.crossing_uncertainty.clone(),
                                ),
                        );
                        ui.end_row();
                    }
                }
            });
    });
    for iteration in &design.computed.iterations {
        for event in &iteration.events {
            if let Some(curve) = &event.curve {
                ui.collapsing(
                    format!(
                        "{} #{} spatial dose points",
                        event.component, event.replacement_k
                    ),
                    |ui| {
                        ui.label(&curve.validation_scope);
                        validation_detail(ui, Quantity::ShutdownDoseRate, "Spatial dose rates");
                        validation_detail(ui, Quantity::ShutdownDoseRatio, "D1S/R2S ratios");
                        ui.label(format!(
                            "Normalization: {}",
                            curve
                                .points
                                .first()
                                .map_or("not recorded", |p| p.source_normalization.as_str())
                        ));
                        ui.label(format!(
                            "History SHA-256: {} · irradiation schedule SHA-256: {}",
                            curve.history_sha256, curve.irradiation_schedule_sha256
                        ));
                        ui.label(format!(
                            "Design input SHA-256 values: {}",
                            curve
                                .design_input_sha256
                                .iter()
                                .map(|(k, v)| format!("{k}={v}"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                        egui::Grid::new(format!(
                            "dose-points-{}-{}-{}",
                            name, event.component, event.replacement_k
                        ))
                        .striped(true)
                        .show(ui, |ui| {
                            for heading in [
                                "Cooling time (days)",
                                "D1S (Sv/h)",
                                "R2S (Sv/h)",
                                "D1S/R2S",
                                "D1S SE",
                                "R2S SE",
                                "Ratio SE",
                                "Comparison",
                                "Crossing uncertainty",
                            ] {
                                ui.strong(heading);
                            }
                            ui.end_row();
                            for point in &curve.points {
                                ui.label(format!("{:.4}", point.cooling_s / 86_400.0));
                                ui.label(
                                    point
                                        .d1s_sv_h
                                        .map_or("NOT_EVALUATED".into(), |v| format!("{v:.6e}")),
                                );
                                ui.label(format!("{:.6e}", point.r2s_sv_h));
                                ui.label(
                                    point
                                        .d1s_over_r2s
                                        .map_or("undefined".into(), |r| format!("{r:.5}")),
                                );
                                ui.label(
                                    point
                                        .d1s_standard_error_sv_h
                                        .map_or("not available".into(), |se| format!("{se:.3e}")),
                                );
                                ui.label(format!("{:.3e}", point.r2s_standard_error_sv_h));
                                ui.label(
                                    point
                                        .ratio_standard_error
                                        .map_or("not available".into(), |se| format!("{se:.3e}")),
                                );
                                let color = if point.comparison
                                    == ComparisonStatus::InconclusiveCovariance
                                {
                                    egui::Color32::from_rgb(165, 90, 20)
                                } else if point.comparison == ComparisonStatus::OutsideBand {
                                    egui::Color32::from_rgb(180, 45, 45)
                                } else {
                                    ui.visuals().text_color()
                                };
                                ui.colored_label(color, format!("{:?}", point.comparison));
                                ui.label(&point.crossing_uncertainty);
                                ui.end_row();
                                if let Some(reason) = &point.d1s_not_evaluated {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(165, 90, 20),
                                        format!("D1S NOT_EVALUATED: {reason}"),
                                    );
                                    ui.end_row();
                                }
                            }
                        });
                    },
                );
            }
        }
    }
    if design.computed.validation == ComparisonStatus::InconclusiveCovariance {
        ui.colored_label(egui::Color32::from_rgb(165, 90, 20), "D1S/R2S central values are in band, but comparison covariance is unknown. Physical validation remains inconclusive; pointwise standard errors are not cooling-time uncertainty.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_worker_result_is_tied_to_its_input_snapshot() {
        let result = parse_result(include_bytes!(
            "../../../fuzz/entries/tests/fixtures/dose-maintenance-result-v02.json"
        ))
        .unwrap();
        let snapshot = DoseInputs {
            designs: "designs.json".into(),
            assumptions: "assumptions.json".into(),
            curves: "curves.json".into(),
            output: "result.json".into(),
        };
        let output = PathBuf::from("result.json");
        let (sender, receiver) = mpsc::channel();
        sender.send(Ok((output.clone(), result.clone()))).unwrap();
        let mut panel = MaintenanceDosePanel {
            open: true,
            inputs: snapshot.clone(),
            run: Some(DoseRunState {
                cancel: Cancellation::default(),
                receiver,
                progress: Arc::default(),
                inputs: snapshot.clone(),
            }),
            ..MaintenanceDosePanel::default()
        };

        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0));
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| panel.window(ui.ctx()),
        );
        output.drop_without_applying_deltas();

        assert_eq!(panel.interface_status()["result_state"], "ready");
        assert_eq!(
            panel.interface_status()["result_inputs_changed_since_run"],
            false
        );
        assert_eq!(panel.file_name, output.display().to_string());
        let LoadState::Ready(loaded) = &panel.state else {
            panic!("worker completion should retain its result")
        };
        assert_eq!(loaded, &result);

        panel.inputs.assumptions = "other-assumptions.json".into();
        assert_eq!(
            panel.interface_status()["result_inputs_changed_since_run"],
            true
        );
        assert!(matches!(panel.state, LoadState::Ready(_)));

        panel.inputs = snapshot;
        assert_eq!(
            panel.interface_status()["result_inputs_changed_since_run"],
            false
        );
    }

    #[test]
    fn cad_design_input_binds_cad_source_rates_and_optional_assumptions_override() {
        let dir = tempfile::tempdir().unwrap();
        let design_file = dir.path().join("rm-m.faris-design.json");
        let linked_assumptions = dir.path().join("rm-m.operating-history.json");
        let rates = dir.path().join("history-driving-rates.json");
        let dose_manifest = dir.path().join("dose-physics.json");
        for path in [&rates, &dose_manifest] {
            std::fs::write(path, "{}").unwrap();
        }
        std::fs::write(
            &design_file,
            include_bytes!("../../../scenarios/rm-m/rm-m.faris-design.json"),
        )
        .unwrap();
        std::fs::write(
            &linked_assumptions,
            include_bytes!("../../../scenarios/rm-m/rm-m.operating-history.json"),
        )
        .unwrap();
        let output = dir.path().join("cad-designs.json");
        let context = CadHistoryContext {
            design: design_file.clone(),
            check_report: design_file.clone(),
            history_rates: rates.clone(),
            cross_sections: design_file.clone(),
            cross_sections_sha256: faris_study::sha256_file(&design_file).unwrap().0,
            history_components: std::collections::BTreeMap::from([(
                "blanket".into(),
                vec!["blanket-solid".into()],
            )]),
            design_id: "rm-m".into(),
            design_sha256: faris_study::sha256_file(&design_file).unwrap().0,
            fusion_power_mw: 525.0,
        };
        let written = write_cad_designs(
            &context,
            "",
            dose_manifest.to_str().unwrap(),
            output.to_str().unwrap(),
        )
        .unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(written).unwrap()).unwrap();
        assert_eq!(
            value["schema_version"],
            faris_engine::maintenance::files::CAD_DOSE_DESIGNS_VERSION
        );
        let entry = &value["designs"]["rm-m"];
        assert_eq!(
            entry["design_file"],
            design_file.canonicalize().unwrap().display().to_string()
        );
        assert_eq!(
            entry["history_run"],
            rates.canonicalize().unwrap().display().to_string()
        );
        assert!(entry.get("scenario").is_none());
        assert!(
            entry
                .get("history_assumptions")
                .is_none_or(serde_json::Value::is_null)
        );
        assert!(entry.get("physics").is_none());
        assert!(entry.get("spectrum_run").is_none());
        let override_output = dir.path().join("cad-designs-override.json");
        let override_file = write_cad_designs(
            &context,
            linked_assumptions.to_str().unwrap(),
            dose_manifest.to_str().unwrap(),
            override_output.to_str().unwrap(),
        )
        .unwrap();
        let override_value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(override_file).unwrap()).unwrap();
        assert_eq!(
            override_value["designs"]["rm-m"]["history_assumptions"],
            linked_assumptions
                .canonicalize()
                .unwrap()
                .display()
                .to_string()
        );
        assert!(
            write_cad_designs(
                &context,
                "",
                dose_manifest.to_str().unwrap(),
                output.to_str().unwrap(),
            )
            .is_err()
        );
    }

    #[test]
    fn interface_status_starts_empty_without_pending_or_result() {
        let panel = MaintenanceDosePanel::default();
        let state = panel.interface_status();
        assert_eq!(state["open"], false);
        assert_eq!(state["pending"], false);
        assert_eq!(state["preparation_pending"], false);
        assert_eq!(state["dose_run_pending"], false);
        assert_eq!(state["result_state"], "empty");
        assert_eq!(state["cad_history_context"], serde_json::Value::Null);
    }

    #[test]
    fn cad_prepare_rejects_mapping_or_cross_section_drift_from_transport() {
        let dir = tempfile::tempdir().unwrap();
        let design = dir.path().join("design.json");
        let report = dir.path().join("import-report.json");
        let rates = dir.path().join("rates.json");
        let xs = dir.path().join("cross_sections.xml");
        for path in [&design, &report, &rates, &xs] {
            std::fs::write(path, "{}").unwrap();
        }
        let context = CadHistoryContext {
            design: design.clone(),
            check_report: report.clone(),
            history_rates: rates,
            cross_sections: xs.clone(),
            cross_sections_sha256: faris_study::sha256_file(&xs).unwrap().0,
            history_components: std::collections::BTreeMap::from([(
                "blanket".into(),
                vec!["blanket-solid".into()],
            )]),
            design_id: "device-cad".into(),
            design_sha256: faris_study::sha256_file(&design).unwrap().0,
            fusion_power_mw: 0.0,
        };
        let config = dir.path().join("prepare.json");
        let xs_sha256 = faris_study::sha256_file(&xs).unwrap().0;
        let value = |solid_map: serde_json::Value, xs_path: &Path| {
            serde_json::json!({
                "schema_version": "faris-maintenance-dose-cad-config/v0.2",
                "design_file": design,
                "import_report": report,
                "cross_sections_xml": {"path": xs_path, "sha256": xs_sha256},
                "solid_history_components": solid_map,
            })
        };
        let correct = value(serde_json::json!({"blanket-solid":"blanket"}), &xs);
        std::fs::write(&config, serde_json::to_vec(&correct).unwrap()).unwrap();
        verify_cad_prepare_binding(&config, &context).unwrap();
        let wrong_map = value(serde_json::json!({"blanket-solid":"divertor"}), &xs);
        std::fs::write(&config, serde_json::to_vec(&wrong_map).unwrap()).unwrap();
        assert!(
            verify_cad_prepare_binding(&config, &context)
                .unwrap_err()
                .contains("map differs")
        );
        let other_xs = dir.path().join("other-cross-sections.xml");
        std::fs::write(&other_xs, "{}").unwrap();
        let wrong_xs = value(serde_json::json!({"blanket-solid":"blanket"}), &other_xs);
        std::fs::write(&config, serde_json::to_vec(&wrong_xs).unwrap()).unwrap();
        assert!(
            verify_cad_prepare_binding(&config, &context)
                .unwrap_err()
                .contains("index does not match")
        );
        std::fs::write(&design, b"{ \"changed\": true }").unwrap();
        assert!(
            verify_cad_prepare_binding(&config, &context)
                .unwrap_err()
                .contains("CAD design changed")
        );
    }
}
