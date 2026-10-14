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
    design_transport_result::{LimitView, ResultView, Row, result_view},
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
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};

/// A finished run as the window keeps it.
struct Shown {
    view: ResultView,
    /// Where the report was saved, or why it was not.
    saved: Result<PathBuf, String>,
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

    /// The floating window; takes a finished run every frame, window open or not.
    pub fn window(&mut self, ctx: &egui::Context) {
        if let Some((report, saved)) = self.job.poll() {
            self.result = Some(Shown {
                view: result_view(&report),
                saved,
            });
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
        let ctx = ui.ctx().clone();
        let running = self.job.is_running();
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
                    self.job.start(plan, &ctx);
                }
                Err(problems) => self.problems = problems,
            }
        }
        self.status(ui);
        if let Some(shown) = &self.result {
            ui.separator();
            show_result(ui, shown);
        }
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
            ui.add(
                egui::DragValue::new(&mut self.inputs.max_seconds)
                    .range(1..=2_592_000)
                    .speed(60.0)
                    .suffix(" s"),
            );
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

fn row_ui(ui: &mut egui::Ui, row: &Row) {
    ui.horizontal_wrapped(|ui| {
        ui.weak(&row.caption);
        ui.label(egui::RichText::new(&row.value).strong());
        ui.label(&row.error);
    });
}

fn show_result(ui: &mut egui::Ui, shown: &Shown) {
    let view = &shown.view;
    ui.heading(&view.heading);
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
        panel.result = Some(Shown {
            view: result_view(&sample_report()),
            saved: Err("no folder".into()),
        });
        frame(&mut panel);
        panel.result = Some(Shown {
            view: result_view(&sample_report()),
            saved: Ok(PathBuf::from("/runs/one/design-transport-report.json")),
        });
        frame(&mut panel);
    }
}
