// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! The Design flux window: run `faris design transport` on a checked design and watch it.
//!
//! The run, its checks and its progress words live in `design_transport_run`, the wording of
//! the result in `design_transport_result`, and the computation in the engine; this file only
//! draws them. The window has no plots, so scrolling it never pans or zooms anything.

use crate::time::Duration;
use crate::{
    badge::{self, Kind},
    design_shared::{missing_count_text, needs_list, path_row},
    design_tools::{SharedTools, ToolsModel, Window},
    design_transport_result::{LimitView, ResultView, Row, parse_report, result_view},
    design_transport_run::{
        Field, Job, MIN_HISTORIES, Problem, RunInputs, RunningView, failure_text, tool_rows,
        validate,
    },
    maintenance_run::elapsed_text,
    settings::Tool,
};
use eframe::egui;
use faris_engine::design::LimitStatus;
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};

/// A finished run as the window keeps it.
struct Shown {
    view: ResultView,
    report: faris_engine::design::DesignTransportReport,
    history_context: Option<CadHistoryContext>,
    opened_from: Option<PathBuf>,
    /// Where the report was saved, or why it was not.
    saved: Result<PathBuf, String>,
}

#[derive(Clone, Debug)]
pub struct CadHistoryContext {
    pub design: PathBuf,
    pub check_report: PathBuf,
    pub history_rates: PathBuf,
    pub cross_sections: PathBuf,
    pub cross_sections_sha256: String,
    pub history_components: std::collections::BTreeMap<String, Vec<String>>,
    pub design_id: String,
    pub design_sha256: String,
    pub fusion_power_mw: f64,
}

pub struct DesignTransportPanel {
    pub open: bool,
    inputs: RunInputs,
    tools: SharedTools,
    job: Job,
    pick: Option<(Field, Receiver<Option<PathBuf>>)>,
    /// Problems found when Run was pressed; cleared when an input changes.
    problems: Vec<Problem>,
    /// Inputs the idle-state problems were last computed for, and those problems.
    preview: Option<(RunInputs, Vec<Problem>)>,
    result: Option<Shown>,
    history_handoff: Option<CadHistoryContext>,
    running_history_context: Option<CadHistoryContext>,
    running_inputs: Option<RunInputs>,
    stale_run_message: Option<String>,
    report_pick: Option<Receiver<Option<PathBuf>>>,
    report_loading:
        Option<Receiver<Result<(PathBuf, faris_engine::design::DesignTransportReport), String>>>,
    history_solid_source: String,
    history_solid_summary: Vec<String>,
    history_solid_error: Option<String>,
}

impl Default for DesignTransportPanel {
    fn default() -> Self {
        Self::with_job(Job::default(), RunInputs::default())
    }
}

fn status_kind(status: LimitStatus) -> Kind {
    match status {
        LimitStatus::Within => Kind::Calculated,
        LimitStatus::Exceeded => Kind::Failed,
        LimitStatus::Inconclusive => Kind::Partial,
    }
}

fn dialog_title(field: Field) -> &'static str {
    match field {
        Field::Design => "Choose the design file",
        Field::CheckReport => "Choose the design check report",
        Field::Output => "Name a new folder for the run",
        // The tools are chosen in the shared Tools section, not here.
        Field::OpenmcPython
        | Field::CrossSections
        | Field::PlasmaSource
        | Field::WindowPython
        | Field::OpenmcPackage => "Choose a file",
    }
}

impl DesignTransportPanel {
    pub fn with_job(job: Job, inputs: RunInputs) -> Self {
        Self {
            open: false,
            inputs,
            tools: ToolsModel::in_memory().shared(),
            job,
            pick: None,
            problems: Vec::new(),
            preview: None,
            result: None,
            history_handoff: None,
            running_history_context: None,
            running_inputs: None,
            stale_run_message: None,
            report_pick: None,
            report_loading: None,
            history_solid_source: String::new(),
            history_solid_summary: Vec::new(),
            history_solid_error: None,
        }
    }

    /// Use the tool values the Design check window uses too.
    pub fn use_tools(&mut self, tools: SharedTools) {
        self.tools = tools;
        self.sync_tools();
    }

    /// Copies the shared tool values into the inputs the run reads.
    fn sync_tools(&mut self) {
        let tools = self.tools.borrow();
        self.inputs.openmc_python = tools.value(Tool::OpenmcPython);
        self.inputs.cross_sections = tools.value(Tool::CrossSections);
        self.inputs.plasma_source = tools.value(Tool::PlasmaSource);
        self.inputs.window_python = tools.value(Tool::WindowPython);
        self.inputs.openmc_package = tools.value(Tool::OpenmcPackage);
    }

    /// Start values for the two files (command-line options).
    pub fn prefill(&mut self, design: Option<&Path>, check_report: Option<&Path>) {
        if let Some(p) = design {
            self.inputs.design = p.display().to_string();
        }
        if let Some(p) = check_report {
            self.inputs.check_report = p.display().to_string();
        }
    }

    pub fn is_running(&self) -> bool {
        self.job.is_running()
    }

    /// Read-only state for bounded native interface checks. This reports the panel's actual
    /// CAD handoff and validation state; it does not start or synthesize a transport result.
    pub fn interface_status(&self) -> serde_json::Value {
        let preview_problems = self
            .preview
            .as_ref()
            .map(|(_, problems)| problems.as_slice())
            .unwrap_or(&[]);
        let problems = if self.problems.is_empty() {
            preview_problems
        } else {
            &self.problems
        };
        let shown = self.result.as_ref();
        let context = shown.and_then(|result| result.history_context.as_ref());
        serde_json::json!({
            "open": self.open,
            "job_phase": format!("{:?}", self.job.phase()),
            "pending": self.is_running() || self.report_loading.is_some() || self.report_pick.is_some(),
            "cancel_requested": self.job.running_view().is_some_and(|view| view.cancelling),
            "input_problem_count": problems.len(),
            "input_problems": problems.iter().map(|problem| problem.text()).collect::<Vec<_>>(),
            "history_mapping_enabled": !self.inputs.history_components_json.trim().is_empty(),
            "history_mapping_json_valid": self.inputs.history_components_json.trim().is_empty()
                || serde_json::from_str::<std::collections::BTreeMap<String, Vec<String>>>(
                    &self.inputs.history_components_json,
                ).is_ok(),
            "result_visible": shown.is_some(),
            "saved_report_view_only": shown.is_some_and(|result| result.opened_from.is_some()),
            "result_target_reached": shown.map(|result| result.report.target_reached),
            "result_stop_reason": shown.map(|result| result.report.stop_reason.as_str()),
            "history_rates_path": shown.and_then(|result| result.report.history_rates_artifact.as_ref().map(|reference| reference.path.as_str())),
            "history_context": context.map(|value| serde_json::json!({
                "design": value.design,
                "check_report": value.check_report,
                "history_rates": value.history_rates,
                "design_id": value.design_id,
                "design_sha256": value.design_sha256,
                "fusion_power_mw": value.fusion_power_mw,
                "history_components": value.history_components,
            })),
            "continue_available": shown.is_some_and(|result| {
                result.history_context.is_some() && result.report.history_rates_artifact.is_some()
            }),
            "handoff_pending": self.history_handoff.is_some(),
        })
    }

    pub fn take_history_handoff(&mut self) -> Option<CadHistoryContext> {
        self.history_handoff.take()
    }

    /// The floating window; takes a finished run every frame, window open or not.
    pub fn window(&mut self, ctx: &egui::Context) {
        if let Some((report, saved)) = self.job.poll() {
            let inputs_current = self.running_inputs.as_ref() == Some(&self.inputs);
            self.running_inputs = None;
            let mut history_context = self.running_history_context.take();
            if report.history_rates_artifact.is_none() {
                history_context = None;
            }
            if let (Some(context), Some(reference)) = (
                history_context.as_mut(),
                report.history_rates_artifact.as_ref(),
            ) {
                context.history_rates = PathBuf::from(&reference.path);
                context.cross_sections_sha256 = report.cross_sections_sha256.clone();
                context.design_id = report
                    .design
                    .id
                    .clone()
                    .unwrap_or_else(|| "CAD design".into());
                context.design_sha256 = report.design.sha256.clone();
                context.fusion_power_mw = report.fusion_power_mw;
            }
            self.result = inputs_current.then(|| Shown {
                view: result_view(&report),
                report,
                history_context,
                opened_from: None,
                saved: saved.clone(),
            });
            if !inputs_current {
                self.stale_run_message = Some(match saved {
                    Ok(path) => format!(
                        "The run finished after its inputs changed. Its report is saved at {}; rerun with the current inputs before continuing the CAD life study.",
                        path.display()
                    ),
                    Err(error) => format!(
                        "The run finished after its inputs changed and the report was not saved: {error}. Rerun with the current inputs before continuing the CAD life study."
                    ),
                });
            }
        }
        if let Some(receiver) = &self.report_pick {
            match receiver.try_recv() {
                Ok(path) => {
                    self.report_pick = None;
                    if let Some(path) = path {
                        let (tx, rx) = mpsc::channel();
                        let _ = crate::thread::Builder::new()
                            .name("faris-design-report-reader".into())
                            .spawn(move || {
                                let result = load_saved_report(&path).map(|report| (path, report));
                                let _ = tx.send(result);
                            });
                        self.report_loading = Some(rx);
                    }
                }
                Err(TryRecvError::Disconnected) => self.report_pick = None,
                Err(TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(100));
                }
            }
        }
        if let Some(receiver) = &self.report_loading {
            match receiver.try_recv() {
                Ok(Ok((path, report))) => {
                    self.result = Some(Shown {
                        view: result_view(&report),
                        report,
                        history_context: None,
                        opened_from: Some(path.clone()),
                        saved: Ok(path),
                    });
                    self.report_loading = None;
                    self.history_handoff = None;
                }
                Ok(Err(error)) => {
                    self.stale_run_message = Some(error);
                    self.report_loading = None;
                }
                Err(TryRecvError::Disconnected) => {
                    self.stale_run_message =
                        Some("The saved report reader stopped without a result.".into());
                    self.report_loading = None;
                }
                Err(TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(100));
                }
            }
        }
        if !self.open {
            return;
        }
        let mut open = self.open;
        egui::Window::new("Design flux")
            .open(&mut open)
            .default_size([820.0, 780.0])
            .vscroll(true)
            .show(ctx, |ui| self.view(ui));
        self.open = open;
    }

    fn pick_path(&mut self, ctx: &egui::Context, field: Field) {
        if self.pick.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let context = ctx.clone();
        let folder = self.tools.borrow().last_folder(Window::DesignFlux);
        let spawned = crate::thread::Builder::new()
            .name("faris-design-transport-dialog".into())
            .spawn(move || {
                let mut dialog = crate::dialog::FileDialog::new().set_title(dialog_title(field));
                if let Some(folder) = folder {
                    dialog = dialog.set_directory(folder);
                }
                let picked = match field {
                    Field::Output => dialog.set_file_name("faris-transport-run").save_file(),
                    Field::Design | Field::CheckReport => {
                        dialog.add_filter("JSON", &["json"]).pick_file()
                    }
                    _ => dialog.pick_file(),
                };
                let _ = sender.send(picked);
                context.request_repaint();
            });
        if spawned.is_ok() {
            self.pick = Some((field, receiver));
        }
    }

    fn poll_pick(&mut self) {
        let Some((field, receiver)) = &self.pick else {
            return;
        };
        let field = *field;
        match receiver.try_recv() {
            Ok(picked) => {
                self.pick = None;
                if let Some(path) = picked {
                    self.tools
                        .borrow_mut()
                        .remember_folder(Window::DesignFlux, &path);
                    *self.inputs.text_mut(field) = path.display().to_string();
                    self.problems.clear();
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.pick = None,
        }
    }

    fn view(&mut self, ui: &mut egui::Ui) {
        self.poll_pick();
        self.refresh_history_solid_summary();
        let ctx = ui.ctx().clone();
        let running = self.job.is_running();
        ui.horizontal(|ui| {
            if !running
                && self.report_loading.is_none()
                && self.report_pick.is_none()
                && ui.button("Open saved transport report…").clicked()
            {
                let (tx, rx) = mpsc::channel();
                self.report_pick = Some(rx);
                let _ = crate::thread::Builder::new()
                    .name("faris-design-report-dialog".into())
                    .spawn(move || {
                        let path = rfd::FileDialog::new()
                            .add_filter("JSON", &["json"])
                            .pick_file();
                        let _ = tx.send(path);
                    });
            }
            if self.report_loading.is_some() {
                ui.spinner();
                ui.label("Reading saved report");
            }
        });
        if let Some(message) = &self.stale_run_message {
            ui.colored_label(Kind::NotEvaluated.color(), message);
        }
        ui.weak(
            "The neutron flux in the solids a fluence limit of a checked design applies to \
             (the TF magnets above 0.1 MeV when it has none), compared with that limit over the \
             design's operating history. Check the design first (the Design check window, or \
             `faris design check --report`) and keep its work folder.",
        );
        // The tools are set once for both windows, in the shared section.
        self.sync_tools();
        let tools_before = self.inputs.clone();
        let attention = tool_rows(&self.inputs).iter().any(|row| row.blocks());
        let tools = self.tools.clone();
        ui.add_enabled_ui(!running, |ui| tools.borrow_mut().ui(ui, attention));
        self.sync_tools();
        if self.inputs != tools_before {
            self.problems.clear();
            self.result = None;
            self.history_handoff = None;
        }
        // Show what is missing before the first click, too; checked again only when an input
        // changed, so the disk is not asked every frame.
        if !running
            && self
                .preview
                .as_ref()
                .is_none_or(|(inputs, _)| *inputs != self.inputs)
        {
            let problems = validate(&self.inputs).err().unwrap_or_default();
            self.preview = Some((self.inputs.clone(), problems));
        }
        // Each problem is said once, beside its field: red after Run was pressed, grey before.
        let (shown, kind) = if self.problems.is_empty() {
            (
                self.preview
                    .as_ref()
                    .map(|(_, p)| p.clone())
                    .unwrap_or_default(),
                Kind::NotEvaluated,
            )
        } else {
            (self.problems.clone(), Kind::Failed)
        };
        let mut pick = None;
        let before = self.inputs.clone();
        ui.add_enabled_ui(!running, |ui| {
            self.files_and_tools(ui, &mut pick, &shown, kind);
            self.limits(ui, &shown, kind);
        });
        if self.inputs != before {
            self.problems.clear();
            // Never offer a previously completed CAD result as belonging to newly edited
            // design/source or component-mapping inputs.
            self.result = None;
            self.history_handoff = None;
        }
        if let Some(field) = pick {
            self.pick_path(&ctx, field);
        }

        if !running && !shown.is_empty() {
            ui.colored_label(kind.color(), missing_count_text(shown.len()));
        }

        ui.add_space(4.0);
        if running {
            if ui
                .button("Cancel")
                .on_hover_text(
                    "Stops OpenMC and any weight-window job and writes no result. The run folder is left as it is.",
                )
                .clicked()
            {
                self.job.cancel();
            }
        } else if ui
            .button("Run")
            .on_hover_text(
                "Runs OpenMC on the design's geometry until the error on the mean flux is within the target or a limit is reached. With a peak limit it also needs the highest piece within twice the target. This can take many minutes.",
            )
            .clicked()
        {
            match validate(&self.inputs) {
                Ok(plan) => {
                    self.problems.clear();
                    self.result = None;
                    self.stale_run_message = None;
                    self.running_history_context = plan
                        .options
                        .history_components
                        .as_ref()
                        .map(|history_components| CadHistoryContext {
                            design: plan.design.clone(),
                            check_report: plan.options.check_report.clone(),
                            history_rates: PathBuf::new(),
                            cross_sections: PathBuf::from(self.inputs.cross_sections.trim()),
                            cross_sections_sha256: String::new(),
                            history_components: history_components.clone(),
                            design_id: "CAD design".into(),
                            design_sha256: String::new(),
                            fusion_power_mw: 0.0,
                        });
                    self.running_inputs = Some(self.inputs.clone());
                    self.job.start(plan, &ctx);
                }
                Err(problems) => self.problems = problems,
            }
        }
        self.status(ui);
        if let Some(message) = &self.stale_run_message {
            ui.colored_label(Kind::NotEvaluated.color(), message);
        }
        if let Some(shown) = &self.result {
            ui.separator();
            if show_result(ui, shown)
                && let Some(context) = &shown.history_context
            {
                self.history_handoff = Some(context.clone());
            }
        }
    }

    fn refresh_history_solid_summary(&mut self) {
        let design_path = self.inputs.design.trim();
        if design_path == self.history_solid_source {
            return;
        }
        self.history_solid_source = design_path.to_owned();
        self.history_solid_summary.clear();
        self.history_solid_error = None;
        if design_path.is_empty() {
            return;
        }
        let path = PathBuf::from(design_path);
        let bytes = match faris_engine::reactor::read_json_bytes(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.history_solid_error = Some(error.to_string());
                return;
            }
        };
        let Some(design) = faris_model::design::parse_design(&bytes).design else {
            self.history_solid_error =
                Some("The design file does not parse as a FARIS design.".into());
            return;
        };
        self.history_solid_summary = design
            .solids
            .into_iter()
            .filter(|solid| solid.material_id.as_deref().is_some_and(|id| id != "void"))
            .map(|solid| {
                format!(
                    "{} · {} · replacement group {} · material {}",
                    solid.id,
                    solid.step_name.as_deref().unwrap_or("unnamed solid"),
                    solid.replacement_group_id.as_deref().unwrap_or("none"),
                    solid.material_id.as_deref().unwrap_or("missing")
                )
            })
            .collect();
    }

    fn files_and_tools(
        &mut self,
        ui: &mut egui::Ui,
        pick: &mut Option<Field>,
        shown: &[Problem],
        kind: Kind,
    ) {
        ui.add_space(4.0);
        egui::Grid::new("design-transport-files")
            .num_columns(3)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                for (label, hint, field) in [
                    (
                        "Design file",
                        "the .faris-design.json that passed `design check`",
                        Field::Design,
                    ),
                    (
                        "Check report",
                        "the JSON `design check --report` wrote for it",
                        Field::CheckReport,
                    ),
                    (
                        "Run folder",
                        "empty: faris-transport-run beside the design file, numbered if it exists",
                        Field::Output,
                    ),
                ] {
                    let note = shown
                        .iter()
                        .find(|p| p.field == Some(field))
                        .map(Problem::text);
                    let note = note.as_deref().map(|text| (kind, text));
                    if path_row(ui, label, hint, self.inputs.text_mut(field), note) {
                        *pick = Some(field);
                    }
                }
            });
        ui.add_space(6.0);
        ui.collapsing("Whole-life history rates (optional)", |ui| {
            ui.weak("Enable this to score CAD-derived history responses in the same OpenMC batches. Source power and identity come from the checked CAD design. FARIS does not infer component mappings or service limits.");
            if let Some(error) = &self.history_solid_error {
                ui.weak(format!("CAD solid list unavailable: {error}"));
            } else if !self.history_solid_summary.is_empty() {
                ui.strong("Active material solids to map exactly once:");
                egui::ScrollArea::vertical()
                    .max_height(100.0)
                    .show(ui, |ui| {
                        for solid in &self.history_solid_summary {
                            ui.label(solid);
                        }
                    });
            }
            ui.weak("The design's linked OperatingHistoryAssumptions set the default operating plan later in the life study. A planning override can be chosen in the dose-governed maintenance step.");
            ui.label("Component ID → CAD solid IDs (JSON). Map every non-void solid exactly once; use the CAD replacement groups as the life components where appropriate.");
            ui.add(
                egui::TextEdit::multiline(&mut self.inputs.history_components_json)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .hint_text(r#"{"blanket":["blanket-solid"],"magnets":["tf-solid"]}"#),
            );
            ui.horizontal(|ui| {
                ui.label("H3 component");
                ui.text_edit_singleline(&mut self.inputs.h3_component_id);
                ui.weak("Current history model: blanket");
            });
            if !self.inputs.history_components_json.trim().is_empty()
                && serde_json::from_str::<std::collections::BTreeMap<String, Vec<String>>>(
                    &self.inputs.history_components_json,
                )
                .is_err()
            {
                ui.colored_label(kind.color(), "Map must be a JSON object of component IDs to arrays of solid IDs.");
            }
        });
        ui.add_space(6.0);
        ui.strong("Tools the run needs");
        needs_list(ui, &tool_rows(&self.inputs));
    }

    fn limits(&mut self, ui: &mut egui::Ui, shown: &[Problem], kind: Kind) {
        ui.add_space(6.0);
        ui.strong("Stop the run at");
        ui.horizontal_wrapped(|ui| {
            ui.label("Target error").on_hover_text(
                "Relative standard error of the mean flux at which the run stops. Command line default: 5 %.",
            );
            ui.add(
                egui::DragValue::new(&mut self.inputs.target_percent)
                    .range(0.1..=50.0)
                    .speed(0.1)
                    .suffix(" %"),
            );
            ui.label("History limit").on_hover_text(
                "Stop after this many histories even when the target is not reached. Command line default: 50 000 000.",
            );
            ui.add(
                egui::DragValue::new(&mut self.inputs.max_histories)
                    .range(MIN_HISTORIES..=10_000_000_000)
                    .speed(100_000.0),
            );
            ui.label("Time limit").on_hover_text(
                "Stop after this many seconds of sampling even when the target is not reached. Command line default: 3600.",
            );
            let mut limit_s = self.inputs.max_seconds as f64;
            if crate::units_menu::quantity_field(
                ui,
                "transport-time-limit",
                "time",
                "s",
                &mut limit_s,
                1.0..=2_592_000.0,
            ) {
                self.inputs.max_seconds = limit_s.round() as u64;
            }
        });
        ui.checkbox(&mut self.inputs.keep_cache, "Keep weight windows between runs")
            .on_hover_text(
                "Keeps weight windows and the channel search in faris-transport-cache beside the design file, so a second run on the same model starts sampling at once.",
            );
        // The limits have no field of their own, so their problems are said here.
        for problem in shown.iter().filter(|p| p.field.is_none()) {
            ui.colored_label(kind.color(), problem.text());
        }
    }

    fn status(&self, ui: &mut egui::Ui) {
        if let Some(view) = self.job.running_view() {
            running_status(ui, &view);
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        } else if let Some(error) = self.job.failure() {
            ui.colored_label(Kind::Failed.color(), failure_text(error));
        } else if let Some(folder) = self.job.cancelled_folder() {
            ui.colored_label(
                Kind::NotEvaluated.color(),
                format!(
                    "Cancelled. No result was written. Next step: press Run to start again; the stopped run's folder, {}, is left as it is and the next run uses a new one.",
                    folder.display()
                ),
            );
        }
    }
}

fn running_status(ui: &mut egui::Ui, view: &RunningView) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label(format!(
            "{} since you pressed Run",
            elapsed_text(Duration::from_secs_f64(view.wall_seconds))
        ));
        if view.cancelling {
            ui.weak("Cancelling: stopping OpenMC\u{2026}");
        }
    });
    ui.label(format!("Now: {}", view.progress.step));
    match &view.progress.sampling {
        None => {
            ui.weak("No time estimate for this step: its length depends on the model.");
        }
        Some(s) => {
            ui.label(format!(
                "{} histories in {} of sampling",
                s.histories,
                elapsed_text(Duration::from_secs_f64(s.elapsed_seconds))
            ));
            ui.label(&s.mean_error);
            if let Some(peak) = &s.peak_error {
                ui.label(peak);
            }
            match &s.estimate {
                Some((fraction, text)) => {
                    ui.add(
                        egui::ProgressBar::new(*fraction as f32)
                            .show_percentage()
                            .desired_width(360.0),
                    );
                    ui.weak(text);
                }
                None => {
                    if let Some(reason) = &s.no_estimate_reason {
                        ui.weak(reason);
                    }
                }
            }
            ui.weak("These errors are provisional: they change as histories are added.");
        }
    }
    ui.weak(format!("Run folder: {}", view.out_dir.display()));
}

fn load_saved_report(path: &Path) -> Result<faris_engine::design::DesignTransportReport, String> {
    const MAX_REPORT_BYTES: u64 = 16 * 1024 * 1024;
    let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_REPORT_BYTES {
        return Err(
            "saved design transport report must be a regular file no larger than 16 MiB".into(),
        );
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(MAX_REPORT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_REPORT_BYTES {
        return Err("saved design transport report exceeds the 16 MiB limit".into());
    }
    parse_report(&bytes)
}

fn row_ui(ui: &mut egui::Ui, row: &Row) {
    ui.horizontal_wrapped(|ui| {
        ui.weak(&row.caption);
        ui.label(egui::RichText::new(&row.value).strong());
        ui.label(&row.error);
    });
}

fn show_result(ui: &mut egui::Ui, shown: &Shown) -> bool {
    let view = &shown.view;
    ui.heading(&view.heading);
    if let Some(reused) = &view.reused {
        ui.horizontal_wrapped(|ui| {
            badge::badge(
                ui,
                Kind::Conditional,
                "reused from cache",
                "The weight windows came from the result cache, not from this run. The flux was sampled in this run.",
            );
            ui.label(reused);
        });
    }
    row_ui(ui, &view.mean);
    if let Some(row) = &view.highest_solid {
        row_ui(ui, row);
    }
    if let Some(row) = &view.highest_piece {
        row_ui(ui, row);
        if let Some(place) = &view.highest_piece_place {
            ui.weak(format!("    at {place}"));
        }
    }
    let target_kind = if view.target.reached {
        Kind::Calculated
    } else {
        Kind::Partial
    };
    ui.colored_label(target_kind.color(), &view.target.text);
    if let Some(why) = &view.target.why {
        ui.label(format!("Why: {why}"));
    }
    if let Some(next) = &view.target.next_step {
        ui.label(format!("Next: {next}"));
    }
    ui.weak(&view.run_line);
    ui.add_space(4.0);
    ui.strong("Speed-up");
    ui.label(&view.speed_up.text);
    if let Some(next) = &view.speed_up.next_step {
        ui.label(format!("Next: {next}"));
    }
    for limit in &view.limits {
        ui.add_space(4.0);
        limit_ui(ui, limit);
    }
    for notice in &view.notices {
        ui.label(format!("Note: {notice}"));
    }
    for item in &view.not_evaluated {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            badge::kind_badge(ui, Kind::NotEvaluated, &item.why);
            ui.strong(format!("Not evaluated: {}", item.item));
        });
        ui.label(format!("Why: {}", item.why));
        ui.label(format!("Next: {}", item.next_step));
    }
    ui.add_space(4.0);
    ui.weak(format!("Run folder: {}", view.run_directory));
    let mut continue_to_dose = false;
    if let Some(reference) = &shown.report.history_rates_artifact {
        if let Some(context) = &shown.history_context {
            ui.label(format!(
                "CAD source: {} · {:.3} MW · design SHA-256 {}",
                context.design_id, context.fusion_power_mw, context.design_sha256
            ));
        }
        ui.label(format!(
            "CAD history rates: {} · SHA-256 {}",
            reference.path, reference.sha256
        ));
        if shown.history_context.is_some() {
            continue_to_dose = ui
                .button("Continue CAD life study with coupled heating")
                .clicked();
        }
    }
    if let Some(path) = &shown.opened_from {
        ui.weak(format!(
            "Opened saved report from {}. This report is view-only.",
            path.display()
        ));
    } else {
        match &shown.saved {
            Ok(path) => {
                ui.weak(format!("Report saved to {}", path.display()));
            }
            Err(error) => {
                ui.colored_label(
                Kind::Partial.color(),
                format!("The report file was not saved: {error}. Next step: the result above is complete; copy it before closing, or run again with a writable run folder."),
            );
            }
        }
    }
    continue_to_dose
}

fn limit_ui(ui: &mut egui::Ui, limit: &LimitView) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            badge::badge(ui, status_kind(limit.status), limit.status_word, &limit.why);
            ui.strong(&limit.title);
        });
        row_ui(ui, &limit.flux);
        row_ui(ui, &limit.fluence);
        row_ui(ui, &limit.years);
        if let Some(row) = &limit.calendar_years {
            row_ui(ui, row);
        }
        ui.label(format!("Why: {}", limit.why));
        ui.label(format!("Next: {}", limit.next_step));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design_transport_result::tests::sample_report;

    #[test]
    fn a_new_panel_shows_no_result() {
        let panel = DesignTransportPanel::with_job(Job::default(), RunInputs::default());
        assert!(panel.result.is_none());
        assert!(!panel.is_running());
        assert!(!panel.open);
    }

    #[test]
    fn interface_status_reports_empty_and_invalid_input_without_starting_transport() {
        let mut panel = DesignTransportPanel::with_job(Job::default(), RunInputs::default());
        panel.inputs.history_components_json = "{invalid".into();
        panel.preview = Some((
            panel.inputs.clone(),
            vec![Problem::new(None, "bad map", "edit it")],
        ));
        let state = panel.interface_status();
        assert_eq!(state["open"], false);
        assert_eq!(state["job_phase"], "Idle");
        assert_eq!(state["pending"], false);
        assert_eq!(state["input_problem_count"], 1);
        assert_eq!(state["history_mapping_json_valid"], false);
        assert_eq!(state["result_visible"], false);
    }

    #[test]
    fn prefill_sets_only_the_two_files() {
        let mut panel = DesignTransportPanel::with_job(Job::default(), RunInputs::default());
        panel.prefill(Some(Path::new("/d/x.json")), None);
        assert_eq!(panel.inputs.design, "/d/x.json");
        assert!(panel.inputs.check_report.is_empty());
    }

    #[test]
    fn statuses_map_to_distinct_kinds() {
        assert_eq!(status_kind(LimitStatus::Within), Kind::Calculated);
        assert_eq!(status_kind(LimitStatus::Exceeded), Kind::Failed);
        assert_eq!(status_kind(LimitStatus::Inconclusive), Kind::Partial);
    }

    /// Draws the empty window and then a window holding a hand-built result, without a
    /// display: the frames must not panic, and the result appears only once one exists.
    #[test]
    fn the_window_draws_empty_and_with_a_result() {
        let ctx = egui::Context::default();
        let mut panel = DesignTransportPanel::with_job(Job::default(), RunInputs::default());
        panel.open = true;
        let frame = |panel: &mut DesignTransportPanel| {
            ctx.run_ui(egui::RawInput::default(), |ui| panel.view(ui))
                .drop_without_applying_deltas();
        };
        frame(&mut panel);
        assert!(panel.result.is_none());
        let report = sample_report();
        panel.result = Some(Shown {
            view: result_view(&report),
            report,
            history_context: None,
            opened_from: None,
            saved: Err("no folder".into()),
        });
        frame(&mut panel);
        let report = sample_report();
        panel.result = Some(Shown {
            view: result_view(&report),
            report,
            history_context: None,
            opened_from: None,
            saved: Ok(PathBuf::from("/runs/one/design-transport-report.json")),
        });
        frame(&mut panel);
    }

    #[test]
    fn v04_saved_report_is_view_only_and_cannot_offer_a_cad_handoff() {
        let ctx = egui::Context::default();
        let mut value = serde_json::to_value(sample_report()).unwrap();
        value["schema_version"] = "faris-design-transport-report/v0.4".into();
        value
            .as_object_mut()
            .unwrap()
            .remove("history_rates_artifact");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.json");
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let report = load_saved_report(&path).unwrap();
        let shown = Shown {
            view: result_view(&report),
            report,
            history_context: None,
            opened_from: Some(path.clone()),
            saved: Ok(path),
        };
        let text = drawn_text(&ctx, &shown);
        assert!(text.contains("view-only"));
        assert!(!text.contains("Continue CAD life study"));
    }

    /// The text a frame draws, to check what a person would read.
    fn drawn_text(ctx: &egui::Context, shown: &Shown) -> String {
        fn collect(shape: &egui::Shape, out: &mut String) {
            match shape {
                egui::Shape::Text(text) => {
                    out.push_str(text.galley.text());
                    out.push('\n');
                }
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, out)),
                _ => {}
            }
        }
        let mut out = String::new();
        let full = ctx.run_ui(egui::RawInput::default(), |ui| {
            show_result(ui, shown);
        });
        for clipped in &full.shapes {
            collect(&clipped.shape, &mut out);
        }
        full.drop_without_applying_deltas();
        out
    }

    // Verifies: AUTO-039
    #[test]
    fn the_window_says_reused_from_cache_and_the_date_only_for_a_reused_result() {
        use faris_engine::result_cache::CacheProvenance;
        let ctx = egui::Context::default();
        let shown = |report| Shown {
            view: result_view(&report),
            report,
            history_context: None,
            opened_from: None,
            saved: Err("no folder".into()),
        };
        // Two frames: the first lays out, the second draws the text.
        let fresh = shown(sample_report());
        drawn_text(&ctx, &fresh);
        assert!(!drawn_text(&ctx, &fresh).contains("reused from cache"));
        let mut report = sample_report();
        report.windows_reused = true;
        report.reused_from = Some(CacheProvenance {
            kind: "design-transport".into(),
            key: "0123456789abcdef0123".into(),
            created_utc: "2026-09-14T08:30:00Z".into(),
            faris_version: "1.0.0".into(),
            compute_seconds: 95.0,
        });
        let reused = shown(report);
        drawn_text(&ctx, &reused);
        let text = drawn_text(&ctx, &reused);
        assert!(text.contains("reused from cache"), "{text}");
        assert!(text.contains("computed on 2026-09-14"), "{text}");
        assert!(text.contains("0123456789ab"), "{text}");
    }
}
