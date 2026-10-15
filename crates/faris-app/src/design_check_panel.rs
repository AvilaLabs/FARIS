//! The Design check window: write a draft design file from a STEP file, and run the nine import
//! checks on a design file, with live stage progress and Cancel.
//!
//! The runs and their input checks live in `design_check_run`, the wording of the results in
//! `design_check_result`, the shared tool rows and run state in `design_shared`, and the
//! computation in the engine; this file only draws them. The window has no plots, so scrolling it
//! never pans or zooms anything.

use crate::time::Duration;
use crate::{
    badge::{self, Kind},
    design_check_result::{
        CheckRow, CheckView, FindingRow, HANDBOOK_PAGE, InitView, Record, Verdict, check_view,
        flux_offer, init_view,
    },
    design_check_run::{
        CheckJob, CheckRunning, Field, InitJob, Inputs, NO_ESTIMATE, Problem, UnitChoice,
        check_failure_text, check_tool_rows, init_failure_text, init_tool_rows, validate_check,
        validate_init,
    },
    design_shared::{missing_count_text, needs_list, path_row},
    design_tools::{SharedTools, ToolsModel, Window},
    maintenance_run::elapsed_text,
    settings::Tool,
};
use eframe::egui;
use faris_engine::design::CheckStatus;
use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};

/// A finished check as the window keeps it.
struct CheckShown {
    view: CheckView,
    saved: Result<PathBuf, String>,
    design: PathBuf,
    work_dir: PathBuf,
}

/// Problems per action for the inputs they were last computed for.
struct Preview {
    inputs: Inputs,
    init: Vec<Problem>,
    check: Vec<Problem>,
}

pub struct DesignCheckPanel {
    pub open: bool,
    inputs: Inputs,
    tools: SharedTools,
    init_job: InitJob,
    check_job: CheckJob,
    pick: Option<(Field, Receiver<Option<PathBuf>>)>,
    /// Problems found when a button was pressed; cleared when an input changes.
    init_problems: Vec<Problem>,
    check_problems: Vec<Problem>,
    preview: Option<Preview>,
    init_result: Option<InitView>,
    check_result: Option<CheckShown>,
    /// The work folder of the check that last started, for the cancelled message.
    last_work_dir: Option<PathBuf>,
    /// Set by "Run the flux for this design"; taken by the app, which opens that window.
    flux_request: Option<(PathBuf, PathBuf)>,
}

impl Default for DesignCheckPanel {
    fn default() -> Self {
        Self::with_jobs(InitJob::default(), CheckJob::default(), Inputs::default())
    }
}

pub fn status_kind(status: CheckStatus) -> Kind {
    match status {
        CheckStatus::Pass => Kind::Checked,
        CheckStatus::Fail => Kind::Failed,
        CheckStatus::NotEvaluated => Kind::NotEvaluated,
    }
}

fn verdict_kind(verdict: Verdict) -> Kind {
    match verdict {
        Verdict::Complete => Kind::Checked,
        Verdict::Failed => Kind::Failed,
        Verdict::Incomplete => Kind::NotEvaluated,
    }
}

fn dialog_title(field: Field) -> &'static str {
    match field {
        Field::Step => "Choose the STEP file",
        Field::DesignOut => "Name the design file to write",
        Field::Design => "Choose the design file",
        Field::WorkDir => "Choose the work folder",
        Field::Report => "Name the check report to write",
        // The tools are chosen in the shared Tools section, not here.
        Field::CadPython | Field::OpenmcPython | Field::CrossSections | Field::PlasmaSource => {
            "Choose a file"
        }
    }
}

/// Problems to show beside their fields, and whether they are final (red) or a preview (grey).
fn shown_problems(attempted: &[Problem], preview: &[Problem]) -> (Vec<Problem>, Kind) {
    if attempted.is_empty() {
        (preview.to_vec(), Kind::NotEvaluated)
    } else {
        (attempted.to_vec(), Kind::Failed)
    }
}

fn note_for(shown: &[Problem], field: Field) -> Option<String> {
    shown
        .iter()
        .find(|p| p.field == Some(field))
        .map(Problem::text)
}

impl DesignCheckPanel {
    pub fn with_jobs(init_job: InitJob, check_job: CheckJob, inputs: Inputs) -> Self {
        Self {
            open: false,
            inputs,
            tools: ToolsModel::in_memory().shared(),
            init_job,
            check_job,
            pick: None,
            init_problems: Vec::new(),
            check_problems: Vec::new(),
            preview: None,
            init_result: None,
            check_result: None,
            last_work_dir: None,
            flux_request: None,
        }
    }

    /// Use the tool values the Design flux window uses too.
    pub fn use_tools(&mut self, tools: SharedTools) {
        self.tools = tools;
        self.sync_tools();
    }

    /// Copies the shared tool values into the inputs the checks read.
    fn sync_tools(&mut self) {
        let tools = self.tools.borrow();
        self.inputs.cad_python = tools.value(Tool::CadPython);
        self.inputs.openmc_python = tools.value(Tool::OpenmcPython);
        self.inputs.cross_sections = tools.value(Tool::CrossSections);
        self.inputs.plasma_source = tools.value(Tool::PlasmaSource);
    }

    /// Start values for the two files (command-line options).
    pub fn prefill(&mut self, step: Option<&Path>, design: Option<&Path>) {
        if let Some(p) = step {
            self.inputs.step = p.display().to_string();
        }
        if let Some(p) = design {
            self.inputs.design = p.display().to_string();
        }
    }

    /// True while Find tools is looking.
    pub fn tools_busy(&self) -> bool {
        self.tools.borrow().is_finding()
    }

    pub fn is_running(&self) -> bool {
        self.init_job.is_running() || self.check_job.is_running()
    }

    /// The design file and report the user asked to run the flux with, once.
    pub fn take_flux_request(&mut self) -> Option<(PathBuf, PathBuf)> {
        self.flux_request.take()
    }

    /// The floating window; takes a finished run every frame, window open or not.
    pub fn window(&mut self, ctx: &egui::Context) {
        if let Some(done) = self.init_job.poll() {
            // The next action starts from the file just written.
            self.inputs.design = done.summary.path.display().to_string();
            self.init_result = Some(init_view(&done));
        }
        if let Some(done) = self.check_job.poll() {
            self.check_result = Some(CheckShown {
                view: check_view(&done.report),
                saved: done.saved,
                design: done.design,
                work_dir: done.work_dir,
            });
        }
        if !self.open {
            return;
        }
        let mut open = self.open;
        egui::Window::new("Design check")
            .open(&mut open)
            .default_size([840.0, 800.0])
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
        let start = self.inputs.clone();
        let folder = self.tools.borrow().last_folder(Window::DesignCheck);
        let spawned = crate::thread::Builder::new()
            .name("faris-design-check-dialog".into())
            .spawn(move || {
                let mut dialog = crate::dialog::FileDialog::new().set_title(dialog_title(field));
                if let Some(folder) = folder {
                    dialog = dialog.set_directory(folder);
                }
                let picked = match field {
                    Field::Step => dialog.add_filter("STEP", &["step", "stp"]).pick_file(),
                    Field::Design => dialog.add_filter("JSON", &["json"]).pick_file(),
                    Field::DesignOut => dialog
                        .set_file_name(
                            crate::design_check_run::default_design_path(Path::new(
                                start.step.trim(),
                            ))
                            .file_name()
                            .map_or_else(
                                || "machine.faris-design.json".to_string(),
                                |n| n.to_string_lossy().into_owned(),
                            ),
                        )
                        .save_file(),
                    Field::Report => dialog.set_file_name("design-check-report.json").save_file(),
                    Field::WorkDir | Field::PlasmaSource => dialog.pick_folder(),
                    Field::CadPython | Field::OpenmcPython | Field::CrossSections => {
                        dialog.pick_file()
                    }
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
                        .remember_folder(Window::DesignCheck, &path);
                    *self.inputs.text_mut(field) = path.display().to_string();
                    self.init_problems.clear();
                    self.check_problems.clear();
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.pick = None,
        }
    }

    fn view(&mut self, ui: &mut egui::Ui) {
        self.poll_pick();
        let ctx = ui.ctx().clone();
        let running = self.is_running();
        ui.weak(
            "Go from the STEP file of your reactor to a checked design. First write a draft design \
             file from the STEP file and fill it in; then check it. The check runs the nine import \
             checks and keeps its work folder, which the flux step reads.",
        );
        // The tools are set once for both windows, in the shared section.
        self.sync_tools();
        let tools_before = self.inputs.clone();
        let attention = init_tool_rows(&self.inputs)
            .iter()
            .chain(&check_tool_rows(&self.inputs))
            .any(|row| row.blocks());
        let tools = self.tools.clone();
        ui.add_enabled_ui(!running, |ui| tools.borrow_mut().ui(ui, attention));
        self.sync_tools();
        if self.inputs != tools_before {
            self.init_problems.clear();
            self.check_problems.clear();
        }
        // Show what is missing before the first click, too; checked again only when an input
        // changed, so the disk is not asked every frame.
        if !running
            && self
                .preview
                .as_ref()
                .is_none_or(|p| p.inputs != self.inputs)
        {
            self.preview = Some(Preview {
                inputs: self.inputs.clone(),
                init: validate_init(&self.inputs).err().unwrap_or_default(),
                check: validate_check(&self.inputs).err().unwrap_or_default(),
            });
        }
        let (init_shown, init_kind) = shown_problems(
            &self.init_problems,
            self.preview.as_ref().map_or(&[][..], |p| p.init.as_slice()),
        );
        let (check_shown, check_kind) = shown_problems(
            &self.check_problems,
            self.preview
                .as_ref()
                .map_or(&[][..], |p| p.check.as_slice()),
        );
        let before = self.inputs.clone();
        let mut pick = None;
        ui.add_space(4.0);
        egui::CollapsingHeader::new(egui::RichText::new("1. Start from a STEP file").heading())
            .default_open(true)
            .show(ui, |ui| {
                self.init_section(ui, &ctx, &mut pick, &init_shown, init_kind, running);
            });
        ui.add_space(6.0);
        egui::CollapsingHeader::new(egui::RichText::new("2. Check a design").heading())
            .default_open(true)
            .show(ui, |ui| {
                self.check_section(ui, &ctx, &mut pick, &check_shown, check_kind, running);
            });
        if self.inputs != before {
            self.init_problems.clear();
            self.check_problems.clear();
        }
        if let Some(field) = pick {
            self.pick_path(&ctx, field);
        }
    }

    fn init_section(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        pick: &mut Option<Field>,
        shown: &[Problem],
        kind: Kind,
        running: bool,
    ) {
        ui.weak(
            "Reads the STEP file and writes a draft design file with every solid listed and every \
             material and role empty. Same as `faris design init`. It never overwrites a file.",
        );
        ui.add_enabled_ui(!running, |ui| {
            egui::Grid::new("design-check-init-files")
                .num_columns(3)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    for (label, hint, field) in [
                        (
                            "STEP file",
                            "the .step or .stp file your CAD tool exported",
                            Field::Step,
                        ),
                        (
                            "Design file to write",
                            "empty: NAME.faris-design.json beside the STEP file",
                            Field::DesignOut,
                        ),
                    ] {
                        let note = note_for(shown, field);
                        let note = note.as_deref().map(|text| (kind, text));
                        if path_row(ui, label, hint, self.inputs.text_mut(field), note) {
                            *pick = Some(field);
                        }
                    }
                    ui.label("Length unit").on_hover_text(
                        "Only used when the STEP file declares none. A file that declares a unit and a different choice here is refused.",
                    );
                    egui::ComboBox::from_id_salt("design-check-unit")
                        .selected_text(self.inputs.length_unit.label())
                        .show_ui(ui, |ui| {
                            for choice in UnitChoice::ALL {
                                ui.selectable_value(
                                    &mut self.inputs.length_unit,
                                    choice,
                                    choice.label(),
                                );
                            }
                        });
                    ui.end_row();
                });
            ui.add_space(4.0);
            ui.strong("Tool this action uses");
            needs_list(ui, &init_tool_rows(&self.inputs));
        });
        ui.add_space(4.0);
        if let Some((wall, cancelling)) = self.init_job.running() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!(
                    "{} since you pressed Read",
                    elapsed_text(Duration::from_secs_f64(wall))
                ));
                if cancelling {
                    ui.weak("Cancelling\u{2026}");
                }
            });
            ui.weak("Reading the STEP file reports no count; the time depends on the model.");
            if ui
                .button("Cancel")
                .on_hover_text("Stops the reader. No design file is written.")
                .clicked()
            {
                self.init_job.cancel();
            }
            ctx.request_repaint_after(Duration::from_millis(250));
        } else {
            if !shown.is_empty() {
                ui.colored_label(kind.color(), missing_count_text(shown.len()));
            }
            if ui
                .add_enabled(!running, egui::Button::new("Read the STEP file"))
                .on_hover_text(
                    "Reads the STEP file and writes the draft design file. Takes seconds to minutes, depending on the model.",
                )
                .clicked()
            {
                match validate_init(&self.inputs) {
                    Ok(plan) => {
                        self.init_problems.clear();
                        self.init_result = None;
                        self.init_job.start(plan, ctx);
                    }
                    Err(problems) => self.init_problems = problems,
                }
            }
        }
        if let Some(error) = self.init_job.failure() {
            ui.colored_label(Kind::Failed.color(), init_failure_text(error));
        } else if self.init_job.was_cancelled() {
            ui.colored_label(
                Kind::NotEvaluated.color(),
                "Cancelled. No design file was written. Next step: press Read the STEP file to start again.",
            );
        }
        if let Some(view) = &self.init_result {
            ui.add_space(4.0);
            init_result_ui(ui, view);
        }
    }

    fn check_section(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        pick: &mut Option<Field>,
        shown: &[Problem],
        kind: Kind,
        running: bool,
    ) {
        ui.weak(
            "Runs the nine import checks on a design file. Same as `faris design check`. The work \
             folder is kept, not removed afterwards: the flux step reads the conversion from it, and a \
             second check reuses what matches.",
        );
        ui.add_enabled_ui(!running, |ui| {
            egui::Grid::new("design-check-files")
                .num_columns(3)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    for (label, hint, field) in [
                        (
                            "Design file",
                            "the .faris-design.json to check; filled in after \"Read the STEP file\"",
                            Field::Design,
                        ),
                        (
                            "Work folder",
                            "empty: NAME-check-work beside the design file; kept for the flux step",
                            Field::WorkDir,
                        ),
                        (
                            "Report to write",
                            "empty: NAME-check-report.json beside the design file, numbered if it exists",
                            Field::Report,
                        ),
                    ] {
                        let note = note_for(shown, field);
                        let note = note.as_deref().map(|text| (kind, text));
                        if path_row(ui, label, hint, self.inputs.text_mut(field), note) {
                            *pick = Some(field);
                        }
                    }
                });
            ui.checkbox(&mut self.inputs.no_reuse, "Do everything again")
                .on_hover_text(
                    "Redo the conversion and every product even when the work folder holds matching ones. Slower.",
                );
            ui.add_space(4.0);
            ui.strong("Tools the check needs");
            needs_list(ui, &check_tool_rows(&self.inputs));
        });
        ui.add_space(4.0);
        if let Some(view) = self.check_job.running_view() {
            check_running_ui(ui, &view);
            if ui
                .button("Cancel")
                .on_hover_text(
                    "Stops the check and the programs it started. No report is written. The work folder is left as it is.",
                )
                .clicked()
            {
                self.check_job.cancel();
            }
            ctx.request_repaint_after(Duration::from_millis(250));
        } else {
            if !shown.is_empty() {
                ui.colored_label(kind.color(), missing_count_text(shown.len()));
            }
            if ui
                .add_enabled(!running, egui::Button::new("Check"))
                .on_hover_text(
                    "Runs checks 1 to 9. A check of a real model can take many minutes: the conversion and the overlap tests dominate.",
                )
                .clicked()
            {
                match validate_check(&self.inputs) {
                    Ok(plan) => {
                        self.check_problems.clear();
                        self.check_result = None;
                        self.last_work_dir = Some(plan.work_dir.clone());
                        self.check_job.start(plan, ctx);
                    }
                    Err(problems) => self.check_problems = problems,
                }
            }
        }
        if let Some(error) = self.check_job.failure() {
            ui.colored_label(Kind::Failed.color(), check_failure_text(error));
        } else if self.check_job.was_cancelled() {
            let folder = self
                .last_work_dir
                .as_ref()
                .map_or_else(String::new, |p| format!(", {},", p.display()));
            ui.colored_label(
                Kind::NotEvaluated.color(),
                format!(
                    "Cancelled. No report was written. Next step: press Check to start again; the work folder{folder} keeps what was finished, and a new check reuses what matches."
                ),
            );
        }
        if let Some(shown) = &self.check_result {
            ui.separator();
            if check_result_ui(ui, shown) {
                self.flux_request =
                    flux_offer(shown.view.verdict, &shown.design, &shown.saved).ok();
            }
        }
    }
}

fn check_running_ui(ui: &mut egui::Ui, view: &CheckRunning) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label(format!(
            "{} since you pressed Check",
            elapsed_text(Duration::from_secs_f64(view.wall_seconds))
        ));
        if view.cancelling {
            ui.weak("Cancelling: stopping the programs\u{2026}");
        }
    });
    ui.label(format!("Now: {}", view.progress.line));
    if let Some(fraction) = view.progress.fraction {
        ui.add(
            egui::ProgressBar::new(fraction)
                .show_percentage()
                .desired_width(360.0),
        );
        ui.weak("The bar is this stage only, not the whole check.");
    }
    if let Some(remaining) = &view.progress.stage_remaining {
        ui.label(remaining);
    }
    ui.weak(&view.progress.stages_seen);
    ui.weak(NO_ESTIMATE);
}

fn init_result_ui(ui: &mut egui::Ui, view: &InitView) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.colored_label(Kind::Checked.color(), &view.summary);
        ui.label(format!("Written to {}", view.path));
        ui.add_space(2.0);
        if let Some(why) = &view.unreadable {
            ui.colored_label(
                Kind::Partial.color(),
                format!("The list of fields to fill in is not available: {why}. Next step: open the file and replace every null."),
            );
        } else {
            let total: usize = view.groups.iter().map(|g| g.count).sum();
            ui.strong(format!("{total} fields are empty (null) and must be filled in:"));
            for group in &view.groups {
                ui.label(format!("\u{2022} {} \u{2013} {} field(s)", group.what, group.count))
                    .on_hover_text(group.paths.join("\n"));
            }
        }
        ui.strong("Also add:");
        for line in &view.also {
            ui.label(format!("\u{2022} {line}"));
        }
        ui.add_space(2.0);
        ui.label(&view.edit_note);
        ui.weak(format!("Handbook: {HANDBOOK_PAGE}."));
    });
}

/// Draws the result; true when the flux button was pressed.
fn check_result_ui(ui: &mut egui::Ui, shown: &CheckShown) -> bool {
    let view = &shown.view;
    ui.horizontal_wrapped(|ui| {
        badge::badge(
            ui,
            verdict_kind(view.verdict),
            &format!("Import: {}", view.verdict.word()),
            &view.summary,
        );
        ui.strong(&view.summary);
    });
    ui.label(&view.next_step);
    ui.weak(&view.design_line);
    if let Some(step) = &view.step_line {
        ui.weak(step);
    }
    ui.add_space(4.0);
    ui.strong("The nine checks");
    for row in &view.checks {
        check_row_ui(ui, row);
    }
    ui.add_space(4.0);
    ui.strong("What was measured");
    for record in &view.records {
        record_ui(ui, record);
    }
    if !view.notices.is_empty() {
        ui.add_space(4.0);
        ui.strong("Notices");
        for notice in &view.notices {
            ui.label(notice);
        }
    }
    ui.add_space(4.0);
    ui.weak(format!(
        "Work folder (kept; the flux step reads the conversion from it): {}",
        shown.work_dir.display()
    ));
    match &shown.saved {
        Ok(path) => {
            ui.weak(format!("Report saved to {}", path.display()));
        }
        Err(error) => {
            ui.colored_label(
                Kind::Partial.color(),
                format!("The report file was not saved: {error}. Next step: the result above is complete; copy it before closing, or check again with a report path in a writable folder."),
            );
        }
    }
    ui.weak(&view.scope);
    ui.add_space(4.0);
    let offer = flux_offer(view.verdict, &shown.design, &shown.saved);
    let clicked = ui
        .add_enabled(
            offer.is_ok(),
            egui::Button::new("Run the flux for this design"),
        )
        .on_hover_text(
            "Opens the Design flux window with this design file and this report filled in.",
        )
        .clicked();
    if let Err(reason) = &offer {
        ui.weak(reason);
    }
    clicked
}

fn check_row_ui(ui: &mut egui::Ui, row: &CheckRow) {
    ui.horizontal_wrapped(|ui| {
        badge::badge(
            ui,
            status_kind(row.status),
            row.status_word,
            row.why.as_deref().unwrap_or(""),
        );
        ui.strong(format!("Check {}", row.number));
        ui.label(&row.name);
    });
    if row.status != CheckStatus::Pass {
        if let Some(why) = &row.why {
            ui.label(format!("    Why: {why}"));
        }
        if let Some(next) = &row.next_step {
            ui.label(format!("    Next step: {next}"));
        }
    }
    for sub in row
        .sub_checks
        .iter()
        .filter(|s| s.status != CheckStatus::Pass)
    {
        ui.label(format!("    {}: {}", sub.name, sub.status.label()));
        if let Some(why) = &sub.why {
            ui.label(format!("        Why: {why}"));
        }
        if let Some(next) = &sub.next_step {
            ui.label(format!("        Next step: {next}"));
        }
    }
    if !row.findings.is_empty() {
        // A long list scrolls inside the window instead of pushing the rest away.
        egui::ScrollArea::vertical()
            .id_salt(("design-check-findings", row.number))
            .max_height(260.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for finding in &row.findings {
                    finding_ui(ui, finding);
                }
            });
    }
}

fn finding_ui(ui: &mut egui::Ui, finding: &FindingRow) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.strong(&finding.item);
        if let Some(waived) = &finding.waived {
            ui.colored_label(Kind::Partial.color(), waived);
        }
        if let Some(requirement) = &finding.requirement {
            ui.weak(format!("Requirement: {requirement}"));
        }
        if let Some(solids) = &finding.solids {
            ui.weak(format!("Solids: {solids}"));
        }
        if let Some(at) = &finding.location {
            ui.weak(format!("At: {at}"));
        }
        ui.label(format!("Why: {}", finding.why));
        ui.label(format!("Next step: {}", finding.next_step));
    });
}

fn record_ui(ui: &mut egui::Ui, record: &Record) {
    ui.horizontal_wrapped(|ui| {
        ui.weak(&record.caption);
        ui.label(egui::RichText::new(&record.value).strong());
    });
    if !record.note.is_empty() {
        ui.add(
            egui::Label::new(egui::RichText::new(format!("    {}", record.note)).small()).wrap(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design_check_result::tests::{failing_report, passing_report};

    fn panel() -> DesignCheckPanel {
        DesignCheckPanel::with_jobs(InitJob::default(), CheckJob::default(), Inputs::default())
    }

    fn shown(
        report: &faris_engine::design::ImportReport,
        saved: Result<PathBuf, String>,
    ) -> CheckShown {
        CheckShown {
            view: check_view(report),
            saved,
            design: PathBuf::from("/d/machine.faris-design.json"),
            work_dir: PathBuf::from("/d/machine-check-work"),
        }
    }

    #[test]
    fn a_new_panel_shows_no_result() {
        let panel = panel();
        assert!(panel.check_result.is_none());
        assert!(panel.init_result.is_none());
        assert!(!panel.is_running());
        assert!(!panel.open);
    }

    #[test]
    fn prefill_sets_the_step_and_design_files() {
        let mut panel = panel();
        panel.prefill(Some(Path::new("/d/x.step")), None);
        assert_eq!(panel.inputs.step, "/d/x.step");
        assert!(panel.inputs.design.is_empty());
        panel.prefill(None, Some(Path::new("/d/x.faris-design.json")));
        assert_eq!(panel.inputs.design, "/d/x.faris-design.json");
    }

    #[test]
    fn statuses_map_to_distinct_kinds() {
        assert_eq!(status_kind(CheckStatus::Pass), Kind::Checked);
        assert_eq!(status_kind(CheckStatus::Fail), Kind::Failed);
        assert_eq!(status_kind(CheckStatus::NotEvaluated), Kind::NotEvaluated);
        assert_ne!(
            verdict_kind(Verdict::Complete),
            verdict_kind(Verdict::Failed)
        );
    }

    #[test]
    fn each_problem_is_said_once_beside_its_field() {
        let problems = vec![
            Problem::new(Field::Design, "No design file is chosen.", "choose it."),
            Problem::new(Field::CadPython, "CAD Python is not set.", "set it."),
        ];
        let (shown, kind) = shown_problems(&[], &problems);
        assert_eq!(kind, Kind::NotEvaluated);
        assert_eq!(
            note_for(&shown, Field::Design).unwrap(),
            "No design file is chosen. Next step: choose it."
        );
        assert!(note_for(&shown, Field::Report).is_none());
        let (_, kind) = shown_problems(&problems, &[]);
        assert_eq!(kind, Kind::Failed);
        assert!(missing_count_text(shown.len()).starts_with("2 things need fixing"));
    }

    /// Draws the empty window and then windows holding hand-built results, without a display:
    /// the frames must not panic, and results appear only once they exist.
    #[test]
    fn the_window_draws_empty_and_with_results() {
        let ctx = egui::Context::default();
        let mut panel = panel();
        panel.open = true;
        let frame = |panel: &mut DesignCheckPanel| {
            ctx.run_ui(egui::RawInput::default(), |ui| panel.view(ui))
                .drop_without_applying_deltas();
        };
        frame(&mut panel);
        assert!(panel.check_result.is_none());
        panel.check_result = Some(shown(&passing_report(), Ok(PathBuf::from("/d/r.json"))));
        frame(&mut panel);
        panel.check_result = Some(shown(&failing_report(), Err("no folder".into())));
        frame(&mut panel);
        let done = crate::design_check_run::InitDone {
            summary: faris_engine::design::InitSummary {
                path: PathBuf::from("/d/machine.faris-design.json"),
                solids: 2,
                length_unit: faris_model::design::LengthUnit::Mm,
                unit_declared_by_file: true,
                step_sha256: "0".into(),
                nulls_to_fill: 2,
            },
            null_paths: Ok(vec![
                "solids[0].role".into(),
                "plasma.major_radius_m".into(),
            ]),
        };
        panel.init_result = Some(init_view(&done));
        frame(&mut panel);
    }

    #[test]
    fn the_flux_request_is_taken_once() {
        let mut panel = panel();
        panel.flux_request = Some((PathBuf::from("/d/a.json"), PathBuf::from("/d/r.json")));
        assert_eq!(
            panel.take_flux_request(),
            Some((PathBuf::from("/d/a.json"), PathBuf::from("/d/r.json")))
        );
        assert_eq!(panel.take_flux_request(), None);
    }
}
