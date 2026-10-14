//! Running `faris design transport` from the desktop: input checks, the worker thread, live
//! progress and cancellation. The run itself is `faris_engine::design::design_transport`, the
//! function the command line calls; nothing here owns physics. The worker never touches egui
//! state: it reports through a mutex-guarded snapshot and a channel, as the maintenance run does.

use crate::time::Instant;
use crate::{
    design_shared::{self, beside, trimmed},
    maintenance_run::unused_path,
};
use eframe::egui;
use faris_engine::{
    design::{
        DesignError, DesignTransportOptions, DesignTransportReport, SamplingEstimate, ToolPaths,
        TransportProgress, design_transport, estimate_sampling, estimate_text, step_text,
    },
    jobs::Cancellation,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, TryRecvError},
    },
};

/// Histories per batch, seed and threads: the command line's defaults.
const PARTICLES_PER_BATCH: u64 = 20_000;
const SEED: u64 = 1;
const THREADS: u32 = 4;
/// The fewest batches the engine allows a run to stop at.
const MIN_BATCHES: u64 = 30;
pub const MIN_HISTORIES: u64 = MIN_BATCHES * PARTICLES_PER_BATCH;
const CACHE_NAME: &str = "faris-transport-cache";
const RUN_NAME: &str = "faris-transport-run";
pub const REPORT_NAME: &str = "design-transport-report.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Design,
    CheckReport,
    Output,
    OpenmcPython,
    CrossSections,
    PlasmaSource,
    WindowPython,
    OpenmcPackage,
}

pub type ToolInfo = design_shared::ToolInfo<Field>;
pub type ToolRow = design_shared::ToolRow<Field>;
pub use design_shared::ToolState;
pub type Problem = design_shared::Problem<Field>;

pub static TOOLS: [ToolInfo; 5] = [
    ToolInfo {
        field: Field::OpenmcPython,
        label: "OpenMC Python",
        purpose: "Runs the transport: a Python with OpenMC 0.15.3 built with DAGMC.",
        without: "",
        environment: Some("FARIS_OPENMC_PYTHON"),
        is_folder: false,
    },
    ToolInfo {
        field: Field::CrossSections,
        label: "Cross sections",
        purpose: "The cross_sections.xml of the nuclear data the neutrons are tracked with.",
        without: "",
        environment: Some("FARIS_CROSS_SECTIONS"),
        is_folder: false,
    },
    ToolInfo {
        field: Field::PlasmaSource,
        label: "Plasma source folder",
        purpose: "The folder holding the openmc_plasma_source package, which makes the tokamak D-T neutron source.",
        without: "",
        environment: Some("FARIS_PLASMA_SOURCE_PATH"),
        is_folder: true,
    },
    ToolInfo {
        field: Field::WindowPython,
        label: "Window Python (optional)",
        purpose: "A Python with OpenMC 0.16 or later. It makes weight windows first, so the run needs far fewer histories.",
        without: "The run is analog: it needs many more histories for the same error.",
        environment: None,
        is_folder: false,
    },
    ToolInfo {
        field: Field::OpenmcPackage,
        label: "OpenMC package folder (optional)",
        purpose: "A folder holding an openmc package used in place of the interpreter's own, such as a build with the aiming patch.",
        without: "The run cannot aim at a duct that streams neutrons from the plasma to the magnets; results are the same, only slower.",
        environment: None,
        is_folder: true,
    },
];

/// What the user typed or picked.
#[derive(Clone, Debug, PartialEq)]
pub struct RunInputs {
    pub design: String,
    pub check_report: String,
    /// Empty: `faris-transport-run` beside the design file, numbered if it exists.
    pub output: String,
    pub openmc_python: String,
    pub cross_sections: String,
    pub plasma_source: String,
    pub window_python: String,
    pub openmc_package: String,
    /// Relative standard error of the mean flux at which the run stops, in per cent.
    pub target_percent: f64,
    pub max_histories: u64,
    pub max_seconds: u64,
    /// Keep weight windows and the channel search beside the design file between runs.
    pub keep_cache: bool,
}

impl Default for RunInputs {
    fn default() -> Self {
        Self {
            design: String::new(),
            check_report: String::new(),
            output: String::new(),
            openmc_python: String::new(),
            cross_sections: String::new(),
            plasma_source: String::new(),
            window_python: String::new(),
            openmc_package: String::new(),
            target_percent: 5.0,
            max_histories: 50_000_000,
            max_seconds: 3600,
            keep_cache: true,
        }
    }
}

impl RunInputs {
    pub fn text_mut(&mut self, field: Field) -> &mut String {
        match field {
            Field::Design => &mut self.design,
            Field::CheckReport => &mut self.check_report,
            Field::Output => &mut self.output,
            Field::OpenmcPython => &mut self.openmc_python,
            Field::CrossSections => &mut self.cross_sections,
            Field::PlasmaSource => &mut self.plasma_source,
            Field::WindowPython => &mut self.window_python,
            Field::OpenmcPackage => &mut self.openmc_package,
        }
    }

    fn text(&self, field: Field) -> &str {
        match field {
            Field::Design => &self.design,
            Field::CheckReport => &self.check_report,
            Field::Output => &self.output,
            Field::OpenmcPython => &self.openmc_python,
            Field::CrossSections => &self.cross_sections,
            Field::PlasmaSource => &self.plasma_source,
            Field::WindowPython => &self.window_python,
            Field::OpenmcPackage => &self.openmc_package,
        }
    }
}

pub fn tool_rows(inputs: &RunInputs) -> Vec<ToolRow> {
    design_shared::tool_rows(&TOOLS, |field| inputs.text(field))
}

/// A checked request: what the worker runs.
#[derive(Clone)]
pub struct RunPlan {
    pub design: PathBuf,
    pub tools: ToolPaths,
    pub options: DesignTransportOptions,
}

fn missing_file(field: Field, what: &str, next: &str) -> Problem {
    Problem::new(field, format!("No {what} is chosen."), next)
}

/// Checks every input before a run starts. `Ok` carries the plan; `Err` every problem.
pub fn validate(inputs: &RunInputs) -> Result<RunPlan, Vec<Problem>> {
    let mut problems = Vec::new();

    let design = trimmed(&inputs.design);
    match &design {
        None => problems.push(missing_file(
            Field::Design,
            "design file",
            "choose the .faris-design.json file that passed `design check`.",
        )),
        Some(path) if !path.is_file() => problems.push(Problem::new(
            Field::Design,
            format!("The design file {} does not exist.", path.display()),
            "choose the design file again.",
        )),
        Some(_) => {}
    }
    match trimmed(&inputs.check_report) {
        None => problems.push(missing_file(
            Field::CheckReport,
            "check report",
            "choose the JSON report `faris design check --report` wrote for this design; its work folder must still hold the conversion.",
        )),
        Some(path) if !path.is_file() => problems.push(Problem::new(
            Field::CheckReport,
            format!("The check report {} does not exist.", path.display()),
            "choose the check report again.",
        )),
        Some(_) => {}
    }
    for row in tool_rows(inputs) {
        let info = row.info;
        match row.state {
            ToolState::Set => {}
            ToolState::Missing if !info.required() => {}
            ToolState::NotFound(path) => problems.push(Problem::new(
                info.field,
                format!("{} {} does not exist.", info.label, path.display()),
                "choose it again in the Tools section.",
            )),
            ToolState::Missing => problems.push(Problem::new(
                info.field,
                format!("{} is not set.", info.label),
                "press Find tools in the Tools section, or set it there; its row says what it is for.",
            )),
        }
    }
    let target = inputs.target_percent / 100.0;
    if !(target > 0.0 && target < 1.0) {
        problems.push(Problem::new(
            None,
            "The target error must lie between 0 and 100 %.",
            "enter a target such as 5 %.",
        ));
    }
    if inputs.max_histories < MIN_HISTORIES {
        problems.push(Problem::new(
            None,
            format!(
                "The history limit must allow at least {MIN_BATCHES} batches of {PARTICLES_PER_BATCH} histories ({MIN_HISTORIES})."
            ),
            "raise the history limit.",
        ));
    }
    if inputs.max_seconds == 0 {
        problems.push(Problem::new(
            None,
            "The time limit is zero.",
            "enter the seconds of sampling the run may take.",
        ));
    }
    let out_dir = match (&design, trimmed(&inputs.output)) {
        (_, Some(path)) => Some(path),
        (Some(design), None) => Some(unused_path(&beside(design).join(RUN_NAME))),
        (None, None) => None,
    };
    if let Some(path) = &out_dir {
        if path.exists() {
            problems.push(Problem::new(
                Field::Output,
                format!("The run folder {} already exists.", path.display()),
                "choose a new folder name, or leave it empty; a run never overwrites.",
            ));
        } else if !beside(path).is_dir() {
            problems.push(Problem::new(
                Field::Output,
                format!("The folder {} does not exist.", beside(path).display()),
                "choose a new run folder inside an existing folder.",
            ));
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let design = design.expect("checked above");
    let tools = ToolPaths {
        openmc_python: trimmed(&inputs.openmc_python),
        cross_sections: trimmed(&inputs.cross_sections),
        plasma_source_path: trimmed(&inputs.plasma_source),
        ..ToolPaths::default()
    };
    let options = DesignTransportOptions {
        check_report: trimmed(&inputs.check_report).expect("checked above"),
        out_dir: out_dir.expect("checked above"),
        target_relative_error: target,
        particles_per_batch: PARTICLES_PER_BATCH,
        max_histories: inputs.max_histories,
        max_seconds: inputs.max_seconds,
        seed: SEED,
        threads: THREADS,
        window_python: trimmed(&inputs.window_python),
        openmc_package_path: trimmed(&inputs.openmc_package),
        group: None,
        cache_dir: inputs.keep_cache.then(|| beside(&design).join(CACHE_NAME)),
        progress: None,
        cancellation: Cancellation::default(),
    };
    Ok(RunPlan {
        design,
        tools,
        options,
    })
}

// ---------------------------------------------------------------- progress --

/// The newest sampling update.
#[derive(Clone, Debug, PartialEq)]
pub struct Sampling {
    pub histories: u64,
    pub relative_error: Option<f64>,
    pub peak_relative_error: Option<f64>,
    pub elapsed_seconds: f64,
}

/// Where the run is, as the worker reported it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub step: Option<String>,
    pub sampling: Option<Sampling>,
}

impl Snapshot {
    pub fn apply(&mut self, update: TransportProgress) {
        match update {
            TransportProgress::Step(name) => self.step = Some(name),
            TransportProgress::Sampling {
                histories,
                relative_error,
                peak_relative_error,
                elapsed_seconds,
            } => {
                self.sampling = Some(Sampling {
                    histories,
                    relative_error,
                    peak_relative_error,
                    elapsed_seconds,
                });
            }
        }
    }
}

/// The limits a run was started with, for the estimate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub target: f64,
    pub max_histories: u64,
    pub max_seconds: u64,
}

/// What the panel shows while the run goes.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgressView {
    /// The current step in plain words.
    pub step: String,
    pub sampling: Option<SamplingView>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SamplingView {
    pub histories: String,
    pub elapsed_seconds: f64,
    pub mean_error: String,
    pub peak_error: Option<String>,
    /// `None` until an estimate can be made; then the share done and the sentence that says
    /// it is an estimate.
    pub estimate: Option<(f64, String)>,
    /// Why there is no estimate yet, when there is none.
    pub no_estimate_reason: Option<String>,
}

pub fn progress_view(snapshot: &Snapshot, limits: Limits) -> ProgressView {
    let step = match (&snapshot.step, &snapshot.sampling) {
        (_, Some(_)) => "sampling the histories".to_string(),
        (Some(name), None) => step_text(name).to_string(),
        (None, None) => "starting".to_string(),
    };
    let sampling = snapshot.sampling.as_ref().map(|s| {
        let percent = |r: f64| format!("{:.1} %", 100.0 * r);
        let estimate: Option<SamplingEstimate> = estimate_sampling(
            s.histories,
            s.elapsed_seconds,
            s.relative_error,
            s.peak_relative_error,
            limits.target,
            limits.max_histories,
            limits.max_seconds,
        );
        SamplingView {
            histories: crate::transport_panel::grouped(s.histories as usize),
            elapsed_seconds: s.elapsed_seconds,
            mean_error: match s.relative_error {
                Some(e) => format!(
                    "Error on the mean: {} (target {:.1} %)",
                    percent(e),
                    100.0 * limits.target
                ),
                None => "Error on the mean: not known yet; two batches must finish first".into(),
            },
            peak_error: s.peak_relative_error.map(|e| {
                format!(
                    "Error on the highest piece: {} (target {:.1} %)",
                    percent(e),
                    200.0 * limits.target
                )
            }),
            no_estimate_reason: estimate.is_none().then(|| {
                "No time estimate yet: it needs the error on the mean, which is known after two batches.".to_string()
            }),
            estimate: estimate.map(|e| (e.fraction, estimate_text(&e))),
        }
    });
    ProgressView { step, sampling }
}

// ------------------------------------------------------------------- state --

/// The computation behind the Run button; replaced in tests.
pub type Runner = Arc<
    dyn Fn(&Path, &ToolPaths, &DesignTransportOptions) -> Result<DesignTransportReport, String>
        + Send
        + Sync,
>;

pub fn default_runner() -> Runner {
    Arc::new(|design, tools, options| {
        design_transport(design, tools, options).map_err(|e: DesignError| e.to_string())
    })
}

pub enum Outcome {
    Done {
        report: Box<DesignTransportReport>,
        /// Where the report was saved, or why it could not be.
        saved: Result<PathBuf, String>,
    },
    Failed(String),
    Cancelled,
}

enum State {
    Idle,
    Running {
        started: Instant,
        cancel: Cancellation,
        cancel_requested: Option<Instant>,
        out_dir: PathBuf,
        limits: Limits,
        progress: Arc<Mutex<Snapshot>>,
        receiver: Receiver<Outcome>,
    },
    Done,
    Failed(String),
    Cancelled(PathBuf),
}

/// Where a run stands, for display and tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Running,
    Done,
    Failed,
    Cancelled,
}

/// What a running job shows.
pub struct RunningView {
    pub wall_seconds: f64,
    pub cancelling: bool,
    pub out_dir: PathBuf,
    pub progress: ProgressView,
}

pub struct Job {
    state: State,
    runner: Runner,
}

/// Writes the report to a path that must not exist yet.
fn save_report(dir: &Path, report: &DesignTransportReport) -> Result<PathBuf, String> {
    use std::io::Write;
    let path = dir.join(REPORT_NAME);
    let mut bytes = serde_json::to_vec_pretty(report).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    file.write_all(&bytes)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

impl Default for Job {
    fn default() -> Self {
        Self::with_runner(default_runner())
    }
}

impl Job {
    pub fn with_runner(runner: Runner) -> Self {
        Self {
            state: State::Idle,
            runner,
        }
    }

    pub fn phase(&self) -> Phase {
        match &self.state {
            State::Idle => Phase::Idle,
            State::Running { .. } => Phase::Running,
            State::Done => Phase::Done,
            State::Failed(_) => Phase::Failed,
            State::Cancelled(_) => Phase::Cancelled,
        }
    }

    pub fn is_running(&self) -> bool {
        self.phase() == Phase::Running
    }

    pub fn failure(&self) -> Option<&str> {
        match &self.state {
            State::Failed(error) => Some(error),
            _ => None,
        }
    }

    /// The folder of a cancelled run, which holds what it had made.
    pub fn cancelled_folder(&self) -> Option<&Path> {
        match &self.state {
            State::Cancelled(dir) => Some(dir),
            _ => None,
        }
    }

    /// Starts the worker on a checked plan.
    pub fn start(&mut self, mut plan: RunPlan, ctx: &egui::Context) {
        if self.is_running() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let cancel = Cancellation::default();
        let progress = Arc::new(Mutex::new(Snapshot::default()));
        let sink = progress.clone();
        let repaint = ctx.clone();
        plan.options.cancellation = cancel.clone();
        plan.options.progress = Some(Arc::new(move |update| {
            if let Ok(mut slot) = sink.lock() {
                slot.apply(update);
            }
            repaint.request_repaint();
        }));
        let limits = Limits {
            target: plan.options.target_relative_error,
            max_histories: plan.options.max_histories,
            max_seconds: plan.options.max_seconds,
        };
        let out_dir = plan.options.out_dir.clone();
        let runner = self.runner.clone();
        let worker_cancel = cancel.clone();
        let context = ctx.clone();
        let worker_dir = out_dir.clone();
        let spawned = crate::thread::Builder::new()
            .name("faris-design-transport".into())
            .spawn(move || {
                let outcome = match runner(&plan.design, &plan.tools, &plan.options) {
                    Ok(report) => {
                        let saved = save_report(&worker_dir, &report);
                        Outcome::Done {
                            report: Box::new(report),
                            saved,
                        }
                    }
                    Err(_) if worker_cancel.is_cancelled() => Outcome::Cancelled,
                    Err(error) => Outcome::Failed(error),
                };
                let _ = sender.send(outcome);
                context.request_repaint();
            });
        self.state = match spawned {
            Ok(_) => State::Running {
                started: Instant::now(),
                cancel,
                cancel_requested: None,
                out_dir,
                limits,
                progress,
                receiver,
            },
            Err(error) => State::Failed(format!("cannot start the worker thread: {error}")),
        };
    }

    /// Asks the run to stop. The engine stops OpenMC and returns; the panel says so at once.
    pub fn cancel(&mut self) {
        if let State::Running {
            cancel,
            cancel_requested,
            ..
        } = &mut self.state
        {
            cancel.cancel();
            cancel_requested.get_or_insert_with(Instant::now);
        }
    }

    /// Collects a finished run. A successful one is returned once.
    pub fn poll(&mut self) -> Option<(DesignTransportReport, Result<PathBuf, String>)> {
        let State::Running {
            receiver, out_dir, ..
        } = &self.state
        else {
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
            Outcome::Done { report, saved } => {
                self.state = State::Done;
                Some((*report, saved))
            }
            Outcome::Failed(error) => {
                self.state = State::Failed(error);
                None
            }
            Outcome::Cancelled => {
                self.state = State::Cancelled(out_dir.clone());
                None
            }
        }
    }

    /// What to draw while the run goes; `None` otherwise.
    pub fn running_view(&self) -> Option<RunningView> {
        let State::Running {
            started,
            cancel_requested,
            out_dir,
            limits,
            progress,
            ..
        } = &self.state
        else {
            return None;
        };
        let snapshot = progress.lock().map(|s| s.clone()).unwrap_or_default();
        Some(RunningView {
            wall_seconds: started.elapsed().as_secs_f64(),
            cancelling: cancel_requested.is_some(),
            out_dir: out_dir.clone(),
            progress: progress_view(&snapshot, *limits),
        })
    }

    /// Time since cancel was pressed, to test how quickly the panel answers.
    #[cfg(test)]
    pub fn cancel_acknowledged_after(&self) -> Option<crate::time::Duration> {
        match &self.state {
            State::Running {
                cancel_requested: Some(at),
                ..
            } => Some(at.elapsed()),
            _ => None,
        }
    }
}

/// The failure as a sentence with its next step.
pub fn failure_text(error: &str) -> String {
    let error = error.trim().trim_end_matches('.');
    format!(
        "The run failed: {error}. Next step: fix what the message names and press Run again. A failed run leaves its folder in place; the next run uses a new one."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design_transport_result::tests::sample_report;
    use crate::time::Duration;

    /// Files and folders every required input points at.
    fn tree() -> (tempfile::TempDir, RunInputs) {
        let dir = tempfile::tempdir().unwrap();
        let p = |n: &str| dir.path().join(n);
        for f in ["d.faris-design.json", "check.json", "python", "xs.xml"] {
            std::fs::write(p(f), "{}").unwrap();
        }
        std::fs::create_dir(p("source")).unwrap();
        let inputs = RunInputs {
            design: p("d.faris-design.json").display().to_string(),
            check_report: p("check.json").display().to_string(),
            openmc_python: p("python").display().to_string(),
            cross_sections: p("xs.xml").display().to_string(),
            plasma_source: p("source").display().to_string(),
            ..RunInputs::default()
        };
        (dir, inputs)
    }

    fn refused(inputs: &RunInputs) -> Vec<Problem> {
        match validate(inputs) {
            Err(problems) => problems,
            Ok(_) => panic!("the request was accepted"),
        }
    }

    fn fields(problems: &[Problem]) -> Vec<Field> {
        problems.iter().filter_map(|p| p.field).collect()
    }

    #[test]
    fn defaults_are_the_command_lines() {
        let inputs = RunInputs::default();
        assert_eq!(inputs.target_percent, 5.0);
        assert_eq!(inputs.max_histories, 50_000_000);
        assert_eq!(inputs.max_seconds, 3600);
        assert!(inputs.keep_cache);
    }

    #[test]
    fn empty_inputs_name_each_missing_input_with_why_and_next_step() {
        let problems = refused(&RunInputs::default());
        assert_eq!(
            fields(&problems),
            [
                Field::Design,
                Field::CheckReport,
                Field::OpenmcPython,
                Field::CrossSections,
                Field::PlasmaSource
            ]
        );
        assert!(problems.iter().all(|p| p.text().contains("Next step")));
    }

    #[test]
    fn the_tool_list_says_which_is_missing_and_what_each_is_for() {
        let rows = tool_rows(&RunInputs::default());
        assert_eq!(rows.len(), 5);
        let python = rows[0].statement();
        assert!(python.starts_with("Missing."), "{python}");
        assert!(python.contains("OpenMC 0.15.3"));
        assert!(python.contains("FARIS_OPENMC_PYTHON"));
        let windows = rows[3].statement();
        assert!(windows.starts_with("Not set."), "{windows}");
        assert!(windows.contains("Without it: The run is analog"));
        let package = rows[4].statement();
        assert!(package.contains("cannot aim"));
        let (_dir, mut inputs) = tree();
        inputs.cross_sections = "/no/such/xs.xml".into();
        let rows = tool_rows(&inputs);
        assert_eq!(rows[0].state, ToolState::Set);
        assert!(
            rows[1]
                .statement()
                .contains("/no/such/xs.xml is not a file that exists")
        );
        assert_eq!(rows[2].state, ToolState::Set);
    }

    #[test]
    fn optional_tools_may_stay_empty_but_a_wrong_path_stops_the_run() {
        let (_dir, mut inputs) = tree();
        assert!(validate(&inputs).is_ok());
        inputs.window_python = "/no/such/python".into();
        let problems = refused(&inputs);
        assert_eq!(fields(&problems), [Field::WindowPython]);
        inputs.window_python.clear();
        inputs.openmc_package = inputs.design.clone();
        assert_eq!(
            fields(&refused(&inputs)),
            [Field::OpenmcPackage],
            "a file is not a folder"
        );
    }

    #[test]
    fn a_valid_request_builds_the_command_lines_options() {
        let (dir, inputs) = tree();
        let plan = validate(&inputs).unwrap();
        let o = &plan.options;
        assert_eq!(o.target_relative_error, 0.05);
        assert_eq!(o.max_histories, 50_000_000);
        assert_eq!(o.max_seconds, 3600);
        assert_eq!(o.particles_per_batch, 20_000);
        assert_eq!(o.threads, 4);
        assert_eq!(o.cache_dir, Some(dir.path().join("faris-transport-cache")));
        assert_eq!(o.out_dir, dir.path().join("faris-transport-run"));
        assert!(plan.tools.cad_python.is_none());
        assert!(plan.tools.openmc_python.is_some());
        // An existing run folder is never reused.
        std::fs::create_dir(dir.path().join("faris-transport-run")).unwrap();
        let plan = validate(&inputs).unwrap();
        assert_eq!(
            plan.options.out_dir,
            dir.path().join("faris-transport-run-2")
        );
        let mut no_cache = inputs.clone();
        no_cache.keep_cache = false;
        assert!(validate(&no_cache).unwrap().options.cache_dir.is_none());
    }

    #[test]
    fn limits_are_checked_as_the_engine_checks_them() {
        let (_dir, mut inputs) = tree();
        inputs.target_percent = 0.0;
        assert!(validate(&inputs).is_err());
        inputs.target_percent = 100.0;
        assert!(validate(&inputs).is_err());
        inputs.target_percent = 5.0;
        inputs.max_histories = MIN_HISTORIES - 1;
        assert!(validate(&inputs).is_err());
        inputs.max_histories = MIN_HISTORIES;
        inputs.max_seconds = 0;
        assert!(validate(&inputs).is_err());
        inputs.max_seconds = 1;
        assert!(validate(&inputs).is_ok());
    }

    #[test]
    fn an_explicit_run_folder_must_be_new_and_inside_an_existing_folder() {
        let (dir, mut inputs) = tree();
        inputs.output = dir.path().display().to_string();
        assert_eq!(fields(&refused(&inputs)), [Field::Output]);
        inputs.output = dir.path().join("missing/run").display().to_string();
        assert_eq!(fields(&refused(&inputs)), [Field::Output]);
        inputs.output = dir.path().join("mine").display().to_string();
        assert!(validate(&inputs).is_ok());
    }

    fn limits() -> Limits {
        Limits {
            target: 0.05,
            max_histories: 50_000_000,
            max_seconds: 3600,
        }
    }

    #[test]
    fn progress_is_worded_by_step_and_then_by_sampling() {
        let mut snapshot = Snapshot::default();
        assert_eq!(progress_view(&snapshot, limits()).step, "starting");
        snapshot.apply(TransportProgress::Step("mgxs".into()));
        let view = progress_view(&snapshot, limits());
        assert_eq!(
            view.step,
            "preparing weight windows: condensing the nuclear data"
        );
        assert!(view.sampling.is_none());
        snapshot.apply(TransportProgress::Step("run".into()));
        snapshot.apply(TransportProgress::Sampling {
            histories: 1_000_000,
            relative_error: None,
            peak_relative_error: None,
            elapsed_seconds: 20.0,
        });
        let view = progress_view(&snapshot, limits());
        assert_eq!(view.step, "sampling the histories");
        let s = view.sampling.unwrap();
        assert_eq!(s.histories, "1,000,000");
        assert!(s.mean_error.contains("not known yet"));
        assert!(s.estimate.is_none());
        assert!(s.no_estimate_reason.is_some());
        assert!(s.peak_error.is_none());
    }

    #[test]
    fn sampling_shows_errors_against_targets_and_a_labelled_estimate() {
        let mut snapshot = Snapshot::default();
        snapshot.apply(TransportProgress::Sampling {
            histories: 1_000_000,
            relative_error: Some(0.10),
            peak_relative_error: Some(0.15),
            elapsed_seconds: 100.0,
        });
        let s = progress_view(&snapshot, limits()).sampling.unwrap();
        assert_eq!(s.mean_error, "Error on the mean: 10.0 % (target 5.0 %)");
        assert_eq!(
            s.peak_error.as_deref(),
            Some("Error on the highest piece: 15.0 % (target 10.0 %)")
        );
        let (fraction, text) = s.estimate.unwrap();
        assert!((fraction - 0.25).abs() < 1e-9);
        assert!(text.starts_with("Estimate:"));
        assert!(s.no_estimate_reason.is_none());
    }

    #[test]
    fn the_failure_text_has_a_next_step() {
        let text = failure_text("the h5m is missing.");
        assert!(text.contains("The run failed: the h5m is missing. Next step"));
    }

    fn start(job: &mut Job, plan: RunPlan) {
        job.start(plan, &egui::Context::default());
    }

    fn wait(job: &mut Job) -> Option<(DesignTransportReport, Result<PathBuf, String>)> {
        let started = Instant::now();
        while job.is_running() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "run did not end"
            );
            if let Some(done) = job.poll() {
                return Some(done);
            }
            crate::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    #[test]
    fn a_run_reports_progress_and_saves_a_new_report() {
        let (_dir, inputs) = tree();
        let plan = validate(&inputs).unwrap();
        std::fs::create_dir(&plan.options.out_dir).unwrap();
        let out = plan.options.out_dir.clone();
        let mut job = Job::with_runner(Arc::new(|_, _, options| {
            let progress = options.progress.as_ref().unwrap();
            progress(TransportProgress::Step("run".into()));
            progress(TransportProgress::Sampling {
                histories: 600_000,
                relative_error: Some(0.2),
                peak_relative_error: None,
                elapsed_seconds: 5.0,
            });
            Ok(sample_report())
        }));
        start(&mut job, plan);
        assert_eq!(job.phase(), Phase::Running);
        let (report, saved) = wait(&mut job).expect("a report");
        assert_eq!(report.histories, 3_000_000);
        let path = saved.unwrap();
        assert_eq!(path, out.join(REPORT_NAME));
        let back: DesignTransportReport =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(back, report);
        assert_eq!(job.phase(), Phase::Done);
    }

    #[test]
    fn cancel_is_acknowledged_at_once_and_reaches_the_engine() {
        let (_dir, inputs) = tree();
        let plan = validate(&inputs).unwrap();
        let mut job = Job::with_runner(Arc::new(|_, _, options| {
            let started = Instant::now();
            while !options.cancellation.is_cancelled() {
                assert!(started.elapsed() < Duration::from_secs(10));
                crate::thread::sleep(Duration::from_millis(2));
            }
            Err("the run was cancelled".into())
        }));
        let out = plan.options.out_dir.clone();
        start(&mut job, plan);
        let view = job.running_view().unwrap();
        assert!(!view.cancelling);
        job.cancel();
        // The panel says "cancelling" in the same call, with no wait for the engine.
        assert!(job.running_view().unwrap().cancelling);
        assert!(job.cancel_acknowledged_after().unwrap() < Duration::from_millis(100));
        assert!(wait(&mut job).is_none());
        assert_eq!(job.phase(), Phase::Cancelled);
        assert_eq!(job.cancelled_folder(), Some(out.as_path()));
    }

    #[test]
    fn a_failed_run_keeps_its_message() {
        let (_dir, inputs) = tree();
        let plan = validate(&inputs).unwrap();
        let mut job = Job::with_runner(Arc::new(|_, _, _| Err("OpenMC exited with 1".into())));
        start(&mut job, plan);
        assert!(wait(&mut job).is_none());
        assert_eq!(job.phase(), Phase::Failed);
        assert_eq!(job.failure(), Some("OpenMC exited with 1"));
    }
}
