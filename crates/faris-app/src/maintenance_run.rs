//! Running the maintenance computation from the Maintenance window: input checks, the
//! worker thread, live progress and cancellation. The computation itself is
//! `faris_engine::maintenance::files::run_from_files`, the same function `faris maintenance
//! run` calls; nothing here owns physics. The worker never touches egui state: it reports
//! through a mutex-guarded progress record and a channel.

use crate::badge::Kind;
use eframe::egui;
use faris_engine::{
    jobs::Cancellation,
    maintenance::{
        MaintenanceResult,
        files::{ProgressFn, RunConfig, RunProgress, parse_designs, run_from_files},
    },
};
use faris_model::maintenance::{MaintenanceAssumptions, Threshold};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, TryRecvError},
    },
    time::{Duration, Instant},
};

pub const ACTINV_URL: &str = "https://actinv.avilalabs.org";
pub const MAX_WORKERS: u32 = 4;
const FIELD_WIDTH: f32 = 560.0;
const RESULT_NAME: &str = "maintenance-result.json";

/// The computation behind the Run button; replaced in tests.
pub type Runner = Arc<
    dyn Fn(&RunConfig, &Cancellation, ProgressFn) -> Result<MaintenanceResult, String>
        + Send
        + Sync,
>;

pub fn default_runner() -> Runner {
    Arc::new(run_from_files)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Designs,
    Assumptions,
    Actinv,
    DataDir,
    Builder,
    Python,
    Output,
}

/// An input that stops the run: why, and what to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub field: Field,
    pub why: String,
    pub next_step: String,
}

impl Problem {
    fn new(field: Field, why: impl Into<String>, next: impl Into<String>) -> Self {
        Self {
            field,
            why: why.into(),
            next_step: next.into(),
        }
    }

    pub fn text(&self) -> String {
        format!("{} Next step: {}", self.why, self.next_step)
    }
}

/// What the user typed or picked.
#[derive(Clone, Debug)]
pub struct RunInputs {
    pub designs: String,
    pub assumptions: String,
    pub actinv: String,
    pub data_dir: String,
    /// Empty: next to the designs file.
    pub output: String,
    pub workers: u32,
    pub python: String,
    pub builder: String,
}

impl Default for RunInputs {
    fn default() -> Self {
        Self {
            designs: String::new(),
            assumptions: String::new(),
            actinv: String::new(),
            data_dir: String::new(),
            output: String::new(),
            workers: 1,
            python: "python3".into(),
            builder: concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../scripts/build_activation_inputs.py"
            )
            .into(),
        }
    }
}

/// A checked request: what the worker will run and where the result goes.
#[derive(Clone, Debug)]
pub struct RunPlan {
    pub config: RunConfig,
    pub output: PathBuf,
}

fn trimmed(text: &str) -> Option<PathBuf> {
    let text = text.trim();
    (!text.is_empty()).then(|| PathBuf::from(text))
}

/// `path`, or `stem-2.ext`, `stem-3.ext`, ... when it exists: a result is never overwritten.
pub fn unused_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .map_or_else(|| "result".into(), |s| s.to_string_lossy().into_owned());
    let extension = path.extension().map(|e| e.to_string_lossy().into_owned());
    for n in 2u32.. {
        let name = match &extension {
            Some(e) => format!("{stem}-{n}.{e}"),
            None => format!("{stem}-{n}"),
        };
        let candidate = path.with_file_name(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

fn read_small(path: &Path) -> Result<Vec<u8>, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("it is not a file".into());
    }
    if meta.len() > 16 * 1024 * 1024 {
        return Err("it is larger than 16 MiB".into());
    }
    std::fs::read(path).map_err(|e| e.to_string())
}

fn on_path(program: &Path) -> bool {
    if program.components().count() > 1 {
        return program.is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
}

/// Checks every input before a run starts. `Ok` carries the plan; `Err` every problem.
pub fn validate(inputs: &RunInputs) -> Result<RunPlan, Vec<Problem>> {
    let mut problems = Vec::new();

    let designs_path = trimmed(&inputs.designs);
    let mut design_names = None;
    match &designs_path {
        None => problems.push(Problem::new(
            Field::Designs,
            "No designs file is chosen.",
            "choose the faris-maintenance-designs/v0.1 file that names each design's scenario, physics file, history run, 709-group spectrum run and history assumptions.",
        )),
        Some(path) => {
            let base = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            match read_small(path)
                .map_err(|e| format!("The designs file {} cannot be read: {e}.", path.display()))
                .and_then(|bytes| {
                    parse_designs(&bytes, base)
                        .map_err(|e| format!("The designs file is not usable: {e}."))
                }) {
                Ok(designs) => design_names = Some(designs.into_keys().collect::<Vec<_>>()),
                Err(why) => problems.push(Problem::new(
                    Field::Designs,
                    why,
                    "fix the file or choose another; every path in it is relative to the file's own folder.",
                )),
            }
        }
    }

    match trimmed(&inputs.assumptions) {
        None => problems.push(Problem::new(
            Field::Assumptions,
            "No maintenance assumptions file is chosen.",
            "choose the faris-maintenance-assumptions/v0.1 file with the component classes, thresholds and cooling grid.",
        )),
        Some(path) => {
            let parsed = read_small(&path)
                .map_err(|e| format!("The assumptions file {} cannot be read: {e}.", path.display()))
                .and_then(|bytes| {
                    serde_json::from_slice::<MaintenanceAssumptions>(&bytes)
                        .map_err(|e| format!("The assumptions file is not readable: {e}."))
                })
                .and_then(|a| {
                    a.validate()
                        .map_err(|e| format!("The assumptions are not valid: {e}."))?;
                    Ok(a)
                });
            match parsed {
                Err(why) => problems.push(Problem::new(
                    Field::Assumptions,
                    why,
                    "fix the file or choose another.",
                )),
                Ok(a) => {
                    if let Some(names) = &design_names {
                        for spec in a.classes.values() {
                            if let Threshold::Calibrate { design, .. } = &spec.threshold
                                && !names.contains(design)
                            {
                                problems.push(Problem::new(
                                    Field::Assumptions,
                                    format!(
                                        "A class calibrates its threshold on design {design}, which the designs file does not name."
                                    ),
                                    "add that design to the designs file or calibrate on one it names.",
                                ));
                            }
                        }
                    }
                }
            }
        }
    }

    match trimmed(&inputs.actinv) {
        None => problems.push(Problem::new(
            Field::Actinv,
            "ACTINV not set.",
            format!("choose the actinv binary (install ACTINV from {ACTINV_URL})."),
        )),
        Some(path) if !path.is_file() => problems.push(Problem::new(
            Field::Actinv,
            format!("The ACTINV binary {} is not a file.", path.display()),
            format!("choose the actinv executable (install ACTINV from {ACTINV_URL})."),
        )),
        Some(_) => {}
    }
    match trimmed(&inputs.data_dir) {
        None => problems.push(Problem::new(
            Field::DataDir,
            "The ACTINV data directory is not set.",
            "choose the ACTINV data root, the folder above the nuclear-data catalogue version (ACTINV's handbook explains how to obtain the data).",
        )),
        Some(path) if !path.is_dir() => problems.push(Problem::new(
            Field::DataDir,
            format!("The ACTINV data directory {} is not a folder.", path.display()),
            "choose the folder that holds the ACTINV nuclear-data catalogue.",
        )),
        Some(_) => {}
    }
    let builder = trimmed(&inputs.builder);
    match &builder {
        Some(path) if path.is_file() => {}
        other => problems.push(Problem::new(
            Field::Builder,
            format!(
                "The activation-input builder script {} was not found.",
                other.as_ref().map_or("(none)".into(), |p| p.display().to_string())
            ),
            "run FARIS from a source checkout that has scripts/build_activation_inputs.py, or choose the script under Advanced.",
        )),
    }
    let python = trimmed(&inputs.python).unwrap_or_else(|| "python3".into());
    if !on_path(&python) {
        problems.push(Problem::new(
            Field::Python,
            format!("The interpreter {} was not found.", python.display()),
            "install Python 3 or choose its path under Advanced; it only runs the activation-input builder.",
        ));
    }
    if !(1..=MAX_WORKERS).contains(&inputs.workers) {
        problems.push(Problem::new(
            Field::Output,
            format!("Workers must be 1 to {MAX_WORKERS}."),
            "set the number of concurrent ACTINV runs in that range.",
        ));
    }

    let output = match (&designs_path, trimmed(&inputs.output)) {
        (_, Some(path)) => Some(path),
        (Some(designs), None) => Some(
            designs
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
                .join(RESULT_NAME),
        ),
        (None, None) => None,
    };
    if let Some(path) = &output {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if !parent.is_dir() {
            problems.push(Problem::new(
                Field::Output,
                format!("The output folder {} does not exist.", parent.display()),
                "choose an existing folder for the result file.",
            ));
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let designs = designs_path.expect("checked above");
    let output = unused_path(&output.expect("checked above"));
    let work_dir = designs
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .join("maintenance-work");
    Ok(RunPlan {
        config: RunConfig {
            designs,
            assumptions: trimmed(&inputs.assumptions).expect("checked above"),
            actinv: trimmed(&inputs.actinv).expect("checked above"),
            data_dir: trimmed(&inputs.data_dir).expect("checked above"),
            cache: None,
            work_dir,
            workers: inputs.workers as usize,
            impurities: None,
            python,
            builder: builder.expect("checked above"),
        },
        output,
    })
}

// ------------------------------------------------------------------- state --

pub enum Outcome {
    Done {
        output: PathBuf,
        result: Box<MaintenanceResult>,
    },
    Failed(String),
    Cancelled,
}

enum State {
    Idle,
    Running {
        started: Instant,
        cancel: Cancellation,
        cancel_requested: bool,
        output: PathBuf,
        progress: Arc<Mutex<Option<RunProgress>>>,
        receiver: Receiver<Outcome>,
    },
    Done(PathBuf),
    Failed(String),
    Cancelled,
}

/// Where a run stands, for display and tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Running,
    Done,
    Failed,
    Cancelled,
}

pub struct RunPanel {
    pub inputs: RunInputs,
    state: State,
    runner: Runner,
    pick: Option<(Field, Receiver<Option<PathBuf>>)>,
    problems: Vec<Problem>,
    /// Inputs (as text) the idle-state problems were last computed for, and those problems.
    preview: (String, Vec<Problem>),
}

impl Default for RunPanel {
    fn default() -> Self {
        Self::with_runner(default_runner())
    }
}

/// Writes `result` to a path that must not exist yet; never replaces a file.
fn write_new(path: &Path, result: &MaintenanceResult) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(result).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    let mut tmp_name = path.as_os_str().to_os_string();
    tmp_name.push(".partial");
    let tmp = PathBuf::from(tmp_name);
    let write = || -> std::io::Result<()> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        // A hard link fails if the target exists, so an existing file is never replaced.
        std::fs::hard_link(&tmp, path)
    };
    let outcome = write();
    let _ = std::fs::remove_file(&tmp);
    outcome.map_err(|e| format!("cannot write {}: {e}", path.display()))
}

impl RunPanel {
    pub fn with_runner(runner: Runner) -> Self {
        Self {
            inputs: RunInputs::default(),
            state: State::Idle,
            runner,
            pick: None,
            problems: Vec::new(),
            preview: (String::new(), Vec::new()),
        }
    }

    pub fn phase(&self) -> Phase {
        match &self.state {
            State::Idle => Phase::Idle,
            State::Running { .. } => Phase::Running,
            State::Done(_) => Phase::Done,
            State::Failed(_) => Phase::Failed,
            State::Cancelled => Phase::Cancelled,
        }
    }

    pub fn is_running(&self) -> bool {
        self.phase() == Phase::Running
    }

    /// Validates and starts the worker. Returns the problems when nothing was started.
    pub fn start(&mut self, ctx: &egui::Context) -> Result<(), Vec<Problem>> {
        if self.is_running() {
            return Ok(());
        }
        let plan = match validate(&self.inputs) {
            Ok(plan) => plan,
            Err(problems) => {
                self.problems = problems.clone();
                return Err(problems);
            }
        };
        self.problems.clear();
        let (sender, receiver) = mpsc::channel();
        let cancel = Cancellation::default();
        let progress: Arc<Mutex<Option<RunProgress>>> = Arc::new(Mutex::new(None));
        let runner = self.runner.clone();
        let worker_cancel = cancel.clone();
        let worker_progress = progress.clone();
        let context = ctx.clone();
        let output = plan.output.clone();
        let spawned = std::thread::Builder::new()
            .name("faris-maintenance-run".into())
            .spawn(move || {
                let sink = context.clone();
                let report: ProgressFn = Arc::new(move |p| {
                    if let Ok(mut slot) = worker_progress.lock() {
                        *slot = Some(p);
                    }
                    sink.request_repaint();
                });
                let outcome = match runner(&plan.config, &worker_cancel, report) {
                    Ok(result) => match write_new(&plan.output, &result) {
                        Ok(()) => Outcome::Done {
                            output: plan.output.clone(),
                            result: Box::new(result),
                        },
                        Err(error) => Outcome::Failed(error),
                    },
                    Err(_) if worker_cancel.is_cancelled() => Outcome::Cancelled,
                    Err(error) => Outcome::Failed(error),
                };
                let _ = sender.send(outcome);
                context.request_repaint();
            });
        match spawned {
            Ok(_) => {
                self.state = State::Running {
                    started: Instant::now(),
                    cancel,
                    cancel_requested: false,
                    output,
                    progress,
                    receiver,
                };
                Ok(())
            }
            Err(error) => {
                self.state = State::Failed(format!("cannot start the worker thread: {error}"));
                Ok(())
            }
        }
    }

    /// Asks the running job to stop; the engine stops ACTINV and cleans up.
    pub fn cancel(&mut self) {
        if let State::Running {
            cancel,
            cancel_requested,
            ..
        } = &mut self.state
        {
            cancel.cancel();
            *cancel_requested = true;
        }
    }

    /// Collects a finished run. A successful one is returned once, with its file name.
    pub fn poll(&mut self) -> Option<(String, MaintenanceResult)> {
        let State::Running { receiver, .. } = &self.state else {
            return None;
        };
        let outcome = match receiver.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                Outcome::Failed("The worker ended unexpectedly without a result.".into())
            }
        };
        match outcome {
            Outcome::Done { output, result } => {
                let name = output.file_name().map_or_else(
                    || output.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                );
                self.state = State::Done(output);
                Some((name, *result))
            }
            Outcome::Failed(error) => {
                self.state = State::Failed(error);
                None
            }
            Outcome::Cancelled => {
                self.state = State::Cancelled;
                None
            }
        }
    }

    fn pick_path(&mut self, ctx: &egui::Context, field: Field) {
        if self.pick.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let context = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("faris-maintenance-run-dialog".into())
            .spawn(move || {
                let dialog = rfd::FileDialog::new().set_title(match field {
                    Field::Designs => "Choose the maintenance designs file",
                    Field::Assumptions => "Choose the maintenance assumptions file",
                    Field::Actinv => "Choose the actinv binary",
                    Field::DataDir => "Choose the ACTINV data directory",
                    Field::Output => "Choose the result file",
                    Field::Builder => "Choose the activation-input builder script",
                    Field::Python => "Choose the Python interpreter",
                });
                let picked = match field {
                    Field::DataDir => dialog.pick_folder(),
                    Field::Output => dialog.set_file_name(RESULT_NAME).save_file(),
                    Field::Designs | Field::Assumptions => {
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
                    let text = path.display().to_string();
                    match field {
                        Field::Designs => self.inputs.designs = text,
                        Field::Assumptions => self.inputs.assumptions = text,
                        Field::Actinv => self.inputs.actinv = text,
                        Field::DataDir => self.inputs.data_dir = text,
                        Field::Output => self.inputs.output = text,
                        Field::Builder => self.inputs.builder = text,
                        Field::Python => self.inputs.python = text,
                    }
                    self.problems.clear();
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.pick = None,
        }
    }

    // ------------------------------------------------------------------ view --

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.poll_pick();
        let ctx = ui.ctx().clone();
        let running = self.is_running();
        let mut pick = None;
        ui.add_enabled_ui(!running, |ui| {
            egui::Grid::new("maintenance-run-inputs")
                .num_columns(3)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    let mut row = |ui: &mut egui::Ui,
                                   label: &str,
                                   hint: &str,
                                   field: Field,
                                   text: &mut String,
                                   problems: &[Problem]| {
                        ui.label(label).on_hover_text(hint);
                        let mut edit = ui.add_sized(
                            [FIELD_WIDTH, ui.spacing().interact_size.y],
                            egui::TextEdit::singleline(text).hint_text(hint),
                        );
                        if let Some(p) = problems.iter().find(|p| p.field == field) {
                            edit = edit.on_hover_text(p.text());
                        }
                        let _ = edit;
                        if ui.button("Browse\u{2026}").clicked() {
                            pick = Some(field);
                        }
                        ui.end_row();
                    };
                    let problems = self.problems.clone();
                    row(
                        ui,
                        "Designs file",
                        "faris-maintenance-designs/v0.1",
                        Field::Designs,
                        &mut self.inputs.designs,
                        &problems,
                    );
                    row(
                        ui,
                        "Assumptions file",
                        "faris-maintenance-assumptions/v0.1",
                        Field::Assumptions,
                        &mut self.inputs.assumptions,
                        &problems,
                    );
                    row(
                        ui,
                        "ACTINV binary",
                        "the actinv executable",
                        Field::Actinv,
                        &mut self.inputs.actinv,
                        &problems,
                    );
                    row(
                        ui,
                        "ACTINV data directory",
                        "the folder above the catalogue version",
                        Field::DataDir,
                        &mut self.inputs.data_dir,
                        &problems,
                    );
                    row(
                        ui,
                        "Result file",
                        "empty: maintenance-result.json next to the designs file; an existing name gets a number",
                        Field::Output,
                        &mut self.inputs.output,
                        &problems,
                    );
                });
            ui.horizontal(|ui| {
                ui.label("Workers").on_hover_text(
                    "Concurrent ACTINV runs. Each run holds one ACTINV process; start with 1 on a laptop.",
                );
                ui.add(egui::DragValue::new(&mut self.inputs.workers).range(1..=MAX_WORKERS));
            });
            egui::CollapsingHeader::new("Advanced")
                .id_salt("maintenance-run-advanced")
                .show(ui, |ui| {
                    egui::Grid::new("maintenance-run-advanced-grid")
                        .num_columns(3)
                        .show(ui, |ui| {
                            ui.label("Builder script");
                            ui.add_sized(
                                [FIELD_WIDTH, ui.spacing().interact_size.y],
                                egui::TextEdit::singleline(&mut self.inputs.builder),
                            );
                            if ui.button("Browse\u{2026}").clicked() {
                                pick = Some(Field::Builder);
                            }
                            ui.end_row();
                            ui.label("Python");
                            ui.add_sized(
                                [FIELD_WIDTH, ui.spacing().interact_size.y],
                                egui::TextEdit::singleline(&mut self.inputs.python),
                            );
                            ui.end_row();
                        });
                });
        });
        if let Some(field) = pick {
            self.pick_path(&ctx, field);
        }

        for problem in &self.problems {
            ui.colored_label(Kind::Failed.color(), problem.text())
                .on_hover_text(problem.text());
        }
        // Show what is wrong before the first click, too; checked again only when an
        // input changed, so the files are not read every frame.
        if self.problems.is_empty() && matches!(self.state, State::Idle) {
            let key = format!("{:?}", self.inputs);
            if self.preview.0 != key {
                self.preview = (key, validate(&self.inputs).err().unwrap_or_default());
            }
            for problem in &self.preview.1 {
                ui.colored_label(Kind::NotEvaluated.color(), problem.text())
                    .on_hover_text(problem.text());
            }
        }

        ui.horizontal(|ui| {
            if running {
                if ui
                    .button("Cancel")
                    .on_hover_text(
                        "Stops the running ACTINV processes and discards this run; no result file is written.",
                    )
                    .clicked()
                {
                    self.cancel();
                }
            } else if ui
                .button("Run")
                .on_hover_text(
                    "Computes replacement outages for every design from ACTINV decay heat; this can take minutes to hours. Curves already computed are reused from the cache.",
                )
                .clicked()
            {
                let _ = self.start(&ctx);
            }
        });
        self.status(ui);
    }

    fn status(&self, ui: &mut egui::Ui) {
        match &self.state {
            State::Idle => {}
            State::Running {
                started,
                cancel_requested,
                output,
                progress,
                ..
            } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("{} elapsed", elapsed_text(started.elapsed())));
                    if *cancel_requested {
                        ui.weak("Cancelling: stopping ACTINV and cleaning up\u{2026}");
                    }
                });
                let current = progress.lock().ok().and_then(|p| p.clone());
                match current {
                    Some(p) => {
                        ui.label(progress_text(&p));
                        ui.weak(format!(
                            "ACTINV runs {}, cached curves {}",
                            p.actinv_runs, p.cache_hits
                        ));
                    }
                    None => {
                        ui.weak("Starting\u{2026}");
                    }
                }
                ui.weak(format!("Result will be written to {}", output.display()));
                ui.ctx().request_repaint_after(Duration::from_millis(250));
            }
            State::Done(path) => {
                ui.colored_label(
                    Kind::Calculated.color(),
                    format!(
                        "Finished. Result written to {} and shown below.",
                        path.display()
                    ),
                );
            }
            State::Failed(error) => {
                let text = failure_text(error);
                ui.colored_label(Kind::Failed.color(), &text)
                    .on_hover_text(&text);
            }
            State::Cancelled => {
                ui.colored_label(
                    Kind::NotEvaluated.color(),
                    "Cancelled. No result file was written. Next step: press Run to start again; ACTINV curves already computed are reused from the cache.",
                );
            }
        }
    }
}

pub fn failure_text(error: &str) -> String {
    let error = error.trim().trim_end_matches('.');
    format!(
        "The run failed: {error}. Next step: fix what the message names and press Run again; ACTINV curves already computed are reused from the cache, and the log of the failed tool is in the message."
    )
}

pub fn progress_text(p: &RunProgress) -> String {
    p.line()
}

pub fn elapsed_text(elapsed: Duration) -> String {
    let s = elapsed.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faris_engine::maintenance::MAINTENANCE_RESULT_VERSION;
    use faris_model::maintenance::{
        CoolingGrid, GoverningQuantity, MAINTENANCE_ASSUMPTIONS_VERSION, MaintenanceClass,
    };
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn assumptions() -> MaintenanceAssumptions {
        MaintenanceAssumptions {
            schema_version: MAINTENANCE_ASSUMPTIONS_VERSION.into(),
            governing_quantity: GoverningQuantity::Heat,
            classes: BTreeMap::new(),
            cooling: CoolingGrid::default(),
            max_iterations: 10,
            convergence_s: 1.0,
        }
    }

    fn with_threshold(threshold: Threshold) -> MaintenanceAssumptions {
        let mut a = assumptions();
        a.classes.insert(
            "magnet".into(),
            MaintenanceClass {
                component_id: "magnets".into(),
                governing: vec!["magnets".into()],
                work_s: 1.0,
                threshold,
            },
        );
        a
    }

    fn empty_result() -> MaintenanceResult {
        MaintenanceResult {
            schema_version: MAINTENANCE_RESULT_VERSION.into(),
            inputs: BTreeMap::new(),
            assumptions: assumptions(),
            designs: BTreeMap::new(),
            contrasts: Vec::new(),
            decay_source: None,
        }
    }

    /// A folder with every input present; the files only have to exist and parse.
    fn tree() -> (tempfile::TempDir, RunInputs) {
        let dir = tempfile::tempdir().unwrap();
        let p = |n: &str| dir.path().join(n);
        for f in [
            "s.json",
            "p.json",
            "run.json",
            "spec.json",
            "h.json",
            "actinv",
            "build.py",
        ] {
            std::fs::write(p(f), "{}").unwrap();
        }
        std::fs::create_dir(p("data")).unwrap();
        std::fs::write(
            p("designs.json"),
            serde_json::json!({
                "schema_version": "faris-maintenance-designs/v0.1",
                "designs": {"a": {"scenario": "s.json", "physics": "p.json",
                    "history_run": "run.json", "spectrum_run": "spec.json",
                    "history_assumptions": "h.json"}}
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            p("assumptions.json"),
            serde_json::to_vec(&with_threshold(Threshold::QStar {
                q_star_w_per_m3: 5.0,
            }))
            .unwrap(),
        )
        .unwrap();
        let inputs = RunInputs {
            designs: p("designs.json").display().to_string(),
            assumptions: p("assumptions.json").display().to_string(),
            actinv: p("actinv").display().to_string(),
            data_dir: p("data").display().to_string(),
            output: String::new(),
            workers: 1,
            python: "sh".into(),
            builder: p("build.py").display().to_string(),
        };
        (dir, inputs)
    }

    fn wait(panel: &mut RunPanel) -> Option<(String, MaintenanceResult)> {
        let started = Instant::now();
        while panel.is_running() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "run did not end"
            );
            if let Some(done) = panel.poll() {
                return Some(done);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    #[test]
    fn empty_inputs_name_each_missing_input_with_why_and_next_step() {
        let problems = validate(&RunInputs {
            builder: "/nonexistent/build.py".into(),
            python: "no-such-python-3".into(),
            ..RunInputs::default()
        })
        .unwrap_err();
        let fields: Vec<Field> = problems.iter().map(|p| p.field).collect();
        for field in [
            Field::Designs,
            Field::Assumptions,
            Field::Actinv,
            Field::DataDir,
            Field::Builder,
            Field::Python,
        ] {
            assert!(fields.contains(&field), "{field:?} missing in {fields:?}");
        }
        let actinv = problems.iter().find(|p| p.field == Field::Actinv).unwrap();
        assert_eq!(
            actinv.text(),
            "ACTINV not set. Next step: choose the actinv binary (install ACTINV from https://actinv.avilalabs.org)."
        );
        assert!(problems.iter().all(|p| p.text().contains("Next step:")));
    }

    #[test]
    fn bad_files_are_refused_before_a_run_starts() {
        let (dir, mut inputs) = tree();
        std::fs::write(dir.path().join("designs.json"), "not json").unwrap();
        std::fs::remove_file(dir.path().join("actinv")).unwrap();
        inputs.workers = 9;
        let problems = validate(&inputs).unwrap_err();
        let by = |f: Field| problems.iter().find(|p| p.field == f).map(Problem::text);
        assert!(by(Field::Designs).unwrap().contains("not usable"));
        assert!(by(Field::Actinv).unwrap().contains("is not a file"));
        assert!(
            by(Field::Output)
                .unwrap()
                .contains("Workers must be 1 to 4")
        );
    }

    #[test]
    fn a_calibration_design_the_designs_file_lacks_is_a_problem() {
        let (dir, inputs) = tree();
        let a = with_threshold(Threshold::Calibrate {
            design: "zzz".into(),
            target_cooldown_s: 5.0,
        });
        std::fs::write(
            dir.path().join("assumptions.json"),
            serde_json::to_vec(&a).unwrap(),
        )
        .unwrap();
        let problems = validate(&inputs).unwrap_err();
        assert!(
            problems.iter().any(|p| p.why.contains("calibrates")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_complete_set_of_inputs_gives_a_plan_that_never_overwrites() {
        let (dir, inputs) = tree();
        let plan = validate(&inputs).unwrap();
        assert_eq!(plan.output, dir.path().join("maintenance-result.json"));
        assert_eq!(plan.config.workers, 1);
        std::fs::write(&plan.output, "x").unwrap();
        assert_eq!(
            validate(&inputs).unwrap().output,
            dir.path().join("maintenance-result-2.json")
        );
        std::fs::write(dir.path().join("maintenance-result-2.json"), "x").unwrap();
        assert_eq!(
            validate(&inputs).unwrap().output,
            dir.path().join("maintenance-result-3.json")
        );
        let mut missing_folder = inputs;
        missing_folder.output = dir.path().join("no/such/r.json").display().to_string();
        assert!(
            validate(&missing_folder).unwrap_err()[0]
                .why
                .contains("does not exist")
        );
    }

    #[test]
    fn a_finished_run_writes_the_file_and_hands_over_the_result() {
        let (dir, inputs) = tree();
        let seen = Arc::new(Mutex::new(None));
        let sink = seen.clone();
        let mut panel = RunPanel::with_runner(Arc::new(move |config, _, progress| {
            *sink.lock().unwrap() = Some(config.clone());
            progress(RunProgress {
                design: Some("a".into()),
                iteration: 1,
                message: "running".into(),
                actinv_runs: 2,
                cache_hits: 3,
            });
            Ok(empty_result())
        }));
        panel.inputs = inputs;
        assert_eq!(panel.phase(), Phase::Idle);
        panel.start(&egui::Context::default()).unwrap();
        let (name, result) = wait(&mut panel).expect("a result");
        assert_eq!(name, "maintenance-result.json");
        assert_eq!(result.schema_version, MAINTENANCE_RESULT_VERSION);
        assert_eq!(panel.phase(), Phase::Done);
        let text = std::fs::read_to_string(dir.path().join(&name)).unwrap();
        assert!(crate::maintenance_panel::parse_result(text.as_bytes()).is_ok());
        assert_eq!(seen.lock().unwrap().as_ref().unwrap().workers, 1);
        // Run again: the first result is kept, the second gets a number.
        panel.start(&egui::Context::default()).unwrap();
        let (second, _) = wait(&mut panel).unwrap();
        assert_eq!(second, "maintenance-result-2.json");
    }

    #[test]
    fn an_invalid_start_runs_nothing_and_keeps_the_problems() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let mut panel = RunPanel::with_runner(Arc::new(move |_, _, _| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(empty_result())
        }));
        assert!(panel.start(&egui::Context::default()).is_err());
        assert_eq!(panel.phase(), Phase::Idle);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_failed_run_keeps_the_message_and_writes_nothing() {
        let (dir, inputs) = tree();
        let mut panel =
            RunPanel::with_runner(Arc::new(|_, _, _| Err("actinv run x: exit 2".into())));
        panel.inputs = inputs;
        panel.start(&egui::Context::default()).unwrap();
        assert!(wait(&mut panel).is_none());
        assert_eq!(panel.phase(), Phase::Failed);
        let State::Failed(message) = &panel.state else {
            panic!()
        };
        assert!(failure_text(message).contains("Next step:"));
        assert!(!dir.path().join("maintenance-result.json").exists());
    }

    #[test]
    fn cancelling_stops_the_job_and_leaves_no_result_file() {
        let (dir, inputs) = tree();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let mut panel = RunPanel::with_runner(Arc::new(move |_, cancel, _| {
            counter.fetch_add(1, Ordering::SeqCst);
            while !cancel.is_cancelled() {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err("cancelled".into())
        }));
        panel.inputs = inputs;
        let ctx = egui::Context::default();
        panel.start(&ctx).unwrap();
        // One run at a time: a second start while running does nothing.
        panel.start(&ctx).unwrap();
        assert!(panel.is_running());
        panel.cancel();
        assert!(wait(&mut panel).is_none());
        assert_eq!(panel.phase(), Phase::Cancelled);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("maintenance-result"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn the_run_section_renders_idle_running_and_finished_without_panic() {
        let ctx = egui::Context::default();
        let mut panel = RunPanel::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 900.0),
            )),
            ..Default::default()
        };
        for _ in 0..2 {
            ctx.run_ui(input.clone(), |ui| panel.ui(ui))
                .drop_without_applying_deltas();
        }
        assert_eq!(elapsed_text(Duration::from_secs(75)), "1:15");
        assert_eq!(elapsed_text(Duration::from_secs(3725)), "1:02:05");
    }

    #[test]
    fn a_finished_run_appears_in_the_maintenance_view() {
        let mut panel = crate::maintenance_panel::MaintenancePanel::default();
        let (_dir, inputs) = tree();
        panel.run = RunPanel::with_runner(Arc::new(|_, _, _| Ok(empty_result())));
        panel.run.inputs = inputs;
        panel.run.start(&egui::Context::default()).unwrap();
        let started = Instant::now();
        while panel.is_pending() {
            assert!(started.elapsed() < Duration::from_secs(10));
            panel.poll_run_for_tests();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(panel.shows_result_for_tests());
    }
}
