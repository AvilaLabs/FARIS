//! Running `faris design init` and `faris design check` from the desktop: input checks, the
//! worker threads, live stage progress and cancellation. The computations are
//! `faris_engine::design::design_init_with_tools` and `check_design_with_tools`, the functions the
//! command line calls; nothing here owns physics. The run/cancel state machine and the tool rows
//! are the ones the Design flux window uses (`design_shared`).

use crate::{
    design_shared::{self, Run, RunningView, Sink, beside, trimmed},
    maintenance_run::unused_path,
};
use eframe::egui;
use faris_engine::{
    design::{
        DesignError, GeometryOptions, ImportReport, InitSummary, ToolPaths,
        check_design_with_tools, design_init_with_tools,
    },
    jobs::Cancellation,
};
use faris_model::design::LengthUnit;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Step,
    DesignOut,
    Design,
    WorkDir,
    Report,
    CadPython,
    OpenmcPython,
    CrossSections,
    PlasmaSource,
}

pub type ToolInfo = design_shared::ToolInfo<Field>;
pub type ToolRow = design_shared::ToolRow<Field>;
pub type Problem = design_shared::Problem<Field>;

/// The tool `design init` uses. Its absence is not an error here: the engine reads the STEP
/// file with its own worker when there is one, and says so when there is not.
pub static INIT_TOOLS: [ToolInfo; 1] = [ToolInfo {
    field: Field::CadPython,
    label: "CAD Python",
    purpose: "A Python with cadquery. It reads the STEP file.",
    without: "FARIS reads the STEP file with its own CAD worker (faris-occt-worker) when one sits beside the program. If there is none, the run stops and says so.",
    environment: Some("FARIS_CAD_PYTHON"),
    is_folder: false,
}];

/// The tools `design check` uses, with what each check loses without its tool.
pub static CHECK_TOOLS: [ToolInfo; 4] = [
    ToolInfo {
        field: Field::CadPython,
        label: "CAD Python",
        purpose: "A Python with cadquery, cad_to_dagmc and h5py. It converts the STEP model to the faceted model that checks 6 to 8 read.",
        without: "",
        environment: Some("FARIS_CAD_PYTHON"),
        is_folder: false,
    },
    ToolInfo {
        field: Field::OpenmcPython,
        label: "OpenMC Python",
        purpose: "A Python with openmc and h5py. It audits the materials' nuclides against the nuclear data (check 5) and samples the plasma source (check 9).",
        without: "check 5's nuclear-data audit and check 9 are NOT_EVALUATED, so the import cannot be complete and the flux cannot be run.",
        environment: Some("FARIS_OPENMC_PYTHON"),
        is_folder: false,
    },
    ToolInfo {
        field: Field::CrossSections,
        label: "Cross sections",
        purpose: "The cross_sections.xml of the nuclear-data library the materials are audited against.",
        without: "check 5's nuclear-data audit is NOT_EVALUATED, so the import cannot be complete and the flux cannot be run.",
        environment: Some("FARIS_CROSS_SECTIONS"),
        is_folder: false,
    },
    ToolInfo {
        field: Field::PlasmaSource,
        label: "Plasma source folder",
        purpose: "The folder holding the openmc_plasma_source package, which makes the source sites check 9 tests.",
        without: "check 9 is NOT_EVALUATED, so the import cannot be complete and the flux cannot be run.",
        environment: Some("FARIS_PLASMA_SOURCE_PATH"),
        is_folder: true,
    },
];

/// The unit of a STEP file that declares none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UnitChoice {
    /// The file declares it; if it does not, the run stops and asks.
    #[default]
    FromFile,
    M,
    Cm,
    Mm,
}

impl UnitChoice {
    pub const ALL: [UnitChoice; 4] = [Self::FromFile, Self::M, Self::Cm, Self::Mm];

    pub fn label(self) -> &'static str {
        match self {
            Self::FromFile => "as the file declares",
            Self::M => "m",
            Self::Cm => "cm",
            Self::Mm => "mm",
        }
    }

    pub fn unit(self) -> Option<LengthUnit> {
        match self {
            Self::FromFile => None,
            Self::M => Some(LengthUnit::M),
            Self::Cm => Some(LengthUnit::Cm),
            Self::Mm => Some(LengthUnit::Mm),
        }
    }
}

/// What the user typed or picked, for both actions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inputs {
    pub step: String,
    /// Empty: `NAME.faris-design.json` beside the STEP file, as the command line does.
    pub design_out: String,
    pub length_unit: UnitChoice,
    pub design: String,
    /// Empty: `NAME-check-work` beside the design file, kept.
    pub work_dir: String,
    /// Empty: `NAME-check-report.json` beside the design file, numbered if it exists.
    pub report: String,
    /// Do everything again even when the work folder holds matching products.
    pub no_reuse: bool,
    pub cad_python: String,
    pub openmc_python: String,
    pub cross_sections: String,
    pub plasma_source: String,
}

impl Inputs {
    pub fn text_mut(&mut self, field: Field) -> &mut String {
        match field {
            Field::Step => &mut self.step,
            Field::DesignOut => &mut self.design_out,
            Field::Design => &mut self.design,
            Field::WorkDir => &mut self.work_dir,
            Field::Report => &mut self.report,
            Field::CadPython => &mut self.cad_python,
            Field::OpenmcPython => &mut self.openmc_python,
            Field::CrossSections => &mut self.cross_sections,
            Field::PlasmaSource => &mut self.plasma_source,
        }
    }

    fn text(&self, field: Field) -> &str {
        match field {
            Field::Step => &self.step,
            Field::DesignOut => &self.design_out,
            Field::Design => &self.design,
            Field::WorkDir => &self.work_dir,
            Field::Report => &self.report,
            Field::CadPython => &self.cad_python,
            Field::OpenmcPython => &self.openmc_python,
            Field::CrossSections => &self.cross_sections,
            Field::PlasmaSource => &self.plasma_source,
        }
    }
}

pub fn init_tool_rows(inputs: &Inputs) -> Vec<ToolRow> {
    design_shared::tool_rows(&INIT_TOOLS, |field| inputs.text(field))
}

pub fn check_tool_rows(inputs: &Inputs) -> Vec<ToolRow> {
    design_shared::tool_rows(&CHECK_TOOLS, |field| inputs.text(field))
}

/// The problems the rows of a tool list make: a missing required tool, or any wrong path.
fn tool_problems(rows: Vec<ToolRow>) -> Vec<Problem> {
    rows.into_iter()
        .filter(ToolRow::blocks)
        .map(|row| {
            let info = row.info;
            match row.state {
                design_shared::ToolState::NotFound(path) => Problem::new(
                    info.field,
                    format!("{} {} does not exist.", info.label, path.display()),
                    "choose it again in the Tools section.",
                ),
                _ => Problem::new(
                    info.field,
                    format!("{} is not set.", info.label),
                    "press Find tools in the Tools section, or set it there; its row says what it is for.",
                ),
            }
        })
        .collect()
}

fn missing_file(field: Field, what: &str, next: &str) -> Problem {
    Problem::new(field, format!("No {what} is chosen."), next)
}

/// A new file: it must not exist and its folder must.
fn new_file_problem(field: Field, what: &str, path: &Path) -> Option<Problem> {
    if path.exists() {
        Some(Problem::new(
            field,
            format!("The {what} {} already exists.", path.display()),
            "choose a new name, or leave it empty; FARIS never overwrites a file.",
        ))
    } else if !beside(path).is_dir() {
        Some(Problem::new(
            field,
            format!("The folder {} does not exist.", beside(path).display()),
            format!("choose a new {what} inside an existing folder."),
        ))
    } else {
        None
    }
}

// -------------------------------------------------------------------- init --

pub const DESIGN_SUFFIX: &str = ".faris-design.json";

/// Where `design init` writes by default: beside the STEP file, as the command line does.
pub fn default_design_path(step: &Path) -> PathBuf {
    let stem = step
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "design".into());
    step.with_file_name(format!("{stem}{DESIGN_SUFFIX}"))
}

/// A checked request to read a STEP file.
#[derive(Clone)]
pub struct InitPlan {
    pub step: PathBuf,
    pub out: PathBuf,
    pub unit: Option<LengthUnit>,
    pub tools: ToolPaths,
}

pub fn validate_init(inputs: &Inputs) -> Result<InitPlan, Vec<Problem>> {
    let mut problems = Vec::new();
    let step = trimmed(&inputs.step);
    match &step {
        None => problems.push(missing_file(
            Field::Step,
            "STEP file",
            "choose the STEP file (.step or .stp) your CAD tool exported.",
        )),
        Some(path) if !path.is_file() => problems.push(Problem::new(
            Field::Step,
            format!("The STEP file {} does not exist.", path.display()),
            "choose the STEP file again.",
        )),
        Some(_) => {}
    }
    let out = match (&step, trimmed(&inputs.design_out)) {
        (_, Some(path)) => Some(path),
        (Some(step), None) => Some(default_design_path(step)),
        (None, None) => None,
    };
    if let Some(path) = &out {
        problems.extend(new_file_problem(Field::DesignOut, "design file", path));
    }
    problems.extend(tool_problems(init_tool_rows(inputs)));
    if !problems.is_empty() {
        return Err(problems);
    }
    Ok(InitPlan {
        step: step.expect("checked above"),
        out: out.expect("checked above"),
        unit: inputs.length_unit.unit(),
        tools: ToolPaths {
            cad_python: trimmed(&inputs.cad_python),
            ..ToolPaths::default()
        },
    })
}

/// What `design init` made and the fields it left for the user.
pub struct InitDone {
    pub summary: InitSummary,
    /// The path of every `null` the draft still holds, or why the file could not be read back.
    pub null_paths: Result<Vec<String>, String>,
}

/// Paths of the nulls in a design file.
pub fn read_null_paths(path: &Path) -> Result<Vec<String>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("{} is not valid JSON: {e}", path.display()))?;
    Ok(faris_model::design::nullable_paths(&value))
}

/// The computation behind the Read button; replaced in tests.
pub type InitRunner = Arc<
    dyn Fn(
            &Path,
            &Path,
            Option<LengthUnit>,
            &ToolPaths,
            &Cancellation,
        ) -> Result<InitSummary, String>
        + Send
        + Sync,
>;

pub fn default_init_runner() -> InitRunner {
    Arc::new(|step, out, unit, tools, cancel| {
        design_init_with_tools(step, Some(out), unit, tools, cancel)
            .map_err(|e: DesignError| e.to_string())
    })
}

pub struct InitJob {
    run: Run<(), InitDone>,
    runner: InitRunner,
}

impl Default for InitJob {
    fn default() -> Self {
        Self::with_runner(default_init_runner())
    }
}

impl InitJob {
    pub fn with_runner(runner: InitRunner) -> Self {
        Self {
            run: Run::default(),
            runner,
        }
    }

    pub fn is_running(&self) -> bool {
        self.run.is_running()
    }

    pub fn failure(&self) -> Option<&str> {
        self.run.failure()
    }

    pub fn was_cancelled(&self) -> bool {
        self.run.was_cancelled()
    }

    pub fn start(&mut self, plan: InitPlan, ctx: &egui::Context) {
        let runner = self.runner.clone();
        self.run.start("faris-design-init", ctx, move |cancel, _| {
            let summary = runner(&plan.step, &plan.out, plan.unit, &plan.tools, &cancel)?;
            let null_paths = read_null_paths(&summary.path);
            Ok(InitDone {
                summary,
                null_paths,
            })
        });
    }

    pub fn cancel(&mut self) {
        self.run.cancel();
    }

    pub fn poll(&mut self) -> Option<InitDone> {
        self.run.poll()
    }

    /// Seconds since the job started and whether Cancel was pressed; `None` unless it runs.
    pub fn running(&self) -> Option<(f64, bool)> {
        self.run
            .running_view()
            .map(|v: RunningView<()>| (v.wall_seconds, v.cancelling))
    }
}

pub fn init_failure_text(error: &str) -> String {
    let error = error.trim().trim_end_matches('.');
    format!(
        "No design file was written: {error}. Next step: fix what the message names and press Read again."
    )
}

// ------------------------------------------------------------------- check --

/// A checked request: what the worker runs.
#[derive(Clone)]
pub struct CheckPlan {
    pub design: PathBuf,
    pub tools: ToolPaths,
    pub options: GeometryOptions,
    pub report_path: PathBuf,
    pub work_dir: PathBuf,
}

/// The design file's name without its `.faris-design.json` (or other) ending.
fn design_stem(design: &Path) -> String {
    let name = design
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "design".into());
    name.strip_suffix(DESIGN_SUFFIX)
        .or_else(|| name.strip_suffix(".json"))
        .unwrap_or(&name)
        .to_string()
}

/// Where the work folder goes when none is named: kept, so that the flux run can read the
/// conversion from it and a second check reuses what matches.
pub fn default_work_dir(design: &Path) -> PathBuf {
    beside(design).join(format!("{}-check-work", design_stem(design)))
}

/// Where the report goes when none is named: beside the design file, never an existing file.
pub fn default_report_path(design: &Path) -> PathBuf {
    unused_path(&beside(design).join(format!("{}-check-report.json", design_stem(design))))
}

pub fn validate_check(inputs: &Inputs) -> Result<CheckPlan, Vec<Problem>> {
    let mut problems = Vec::new();
    let design = trimmed(&inputs.design);
    match &design {
        None => problems.push(missing_file(
            Field::Design,
            "design file",
            "choose the .faris-design.json file, or write one first with \"Start from a STEP file\".",
        )),
        Some(path) if !path.is_file() => problems.push(Problem::new(
            Field::Design,
            format!("The design file {} does not exist.", path.display()),
            "choose the design file again.",
        )),
        Some(_) => {}
    }
    let work_dir = match (&design, trimmed(&inputs.work_dir)) {
        (_, Some(path)) => Some(path),
        (Some(design), None) => Some(default_work_dir(design)),
        (None, None) => None,
    };
    if let Some(path) = &work_dir {
        if path.exists() && !path.is_dir() {
            problems.push(Problem::new(
                Field::WorkDir,
                format!("The work folder {} is a file.", path.display()),
                "choose a folder, or leave it empty.",
            ));
        } else if !path.exists() && !beside(path).is_dir() {
            problems.push(Problem::new(
                Field::WorkDir,
                format!("The folder {} does not exist.", beside(path).display()),
                "choose a work folder inside an existing folder.",
            ));
        }
    }
    let report_path = match (&design, trimmed(&inputs.report)) {
        (_, Some(path)) => Some(path),
        (Some(design), None) => Some(default_report_path(design)),
        (None, None) => None,
    };
    if let Some(path) = &report_path {
        problems.extend(new_file_problem(Field::Report, "report", path));
    }
    problems.extend(tool_problems(check_tool_rows(inputs)));
    if !problems.is_empty() {
        return Err(problems);
    }
    let work_dir = work_dir.expect("checked above");
    let options = GeometryOptions {
        work_dir: Some(work_dir.clone()),
        reuse: !inputs.no_reuse,
        ..GeometryOptions::default()
    };
    Ok(CheckPlan {
        design: design.expect("checked above"),
        tools: ToolPaths {
            cad_python: trimmed(&inputs.cad_python),
            openmc_python: trimmed(&inputs.openmc_python),
            cross_sections: trimmed(&inputs.cross_sections),
            plasma_source_path: trimmed(&inputs.plasma_source),
            ..ToolPaths::default()
        },
        options,
        report_path: report_path.expect("checked above"),
        work_dir,
    })
}

/// Where the check is, as the engine's progress callback reported it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// The newest stage line: stage, done, total.
    pub stage: Option<(String, usize, usize)>,
    /// How many different stages have reported so far.
    pub stages_seen: usize,
}

impl Snapshot {
    pub fn apply(&mut self, stage: &str, done: usize, total: usize) {
        if self.stage.as_ref().is_none_or(|(name, _, _)| name != stage) {
            self.stages_seen += 1;
        }
        self.stage = Some((stage.to_string(), done, total));
    }
}

/// What the panel shows while the check goes.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgressView {
    /// `stage: done of total`, the command line's stage line; or why there is none yet.
    pub line: String,
    /// How far the stage in progress is, when it has a count.
    pub fraction: Option<f32>,
    pub stages_seen: String,
}

/// Said under every progress line, because no honest total can be given.
pub const NO_ESTIMATE: &str = "No time estimate for the whole check: the stages differ in length and their number depends on the model, so only the stage in progress shows how far it is. Reading the STEP file, the conversion and the material audit run as separate programs and show no count while they work; the line stays where it is until they finish.";

pub fn progress_view(snapshot: &Snapshot) -> ProgressView {
    match &snapshot.stage {
        None => ProgressView {
            line: "Starting: the design file and the STEP file are read first, and that reports no count.".into(),
            fraction: None,
            stages_seen: "No stage has reported yet.".into(),
        },
        Some((stage, done, total)) => ProgressView {
            line: format!("{stage}: {done} of {total}"),
            fraction: (*total > 0).then(|| (*done as f32 / *total as f32).clamp(0.0, 1.0)),
            stages_seen: format!(
                "{} counted stage(s) have reported so far.",
                snapshot.stages_seen
            ),
        },
    }
}

/// What a finished check hands the window.
pub struct CheckDone {
    pub report: Box<ImportReport>,
    /// Where the report was saved, or why it could not be.
    pub saved: Result<PathBuf, String>,
    pub design: PathBuf,
    pub work_dir: PathBuf,
}

/// The computation behind the Check button; replaced in tests.
pub type CheckRunner =
    Arc<dyn Fn(&Path, &ToolPaths, &GeometryOptions) -> Result<ImportReport, String> + Send + Sync>;

pub fn default_check_runner() -> CheckRunner {
    Arc::new(|design, tools, options| {
        check_design_with_tools(design, tools, options).map_err(|e: DesignError| e.to_string())
    })
}

/// Writes the report to a path that must not exist yet, as the command line does.
fn save_report(path: &Path, report: &ImportReport) -> Result<PathBuf, String> {
    use std::io::Write;
    let mut bytes = serde_json::to_vec_pretty(report).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    file.write_all(&bytes)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path.to_path_buf())
}

/// What a running check shows.
pub struct CheckRunning {
    pub wall_seconds: f64,
    pub cancelling: bool,
    pub progress: ProgressView,
}

pub struct CheckJob {
    run: Run<Snapshot, CheckDone>,
    runner: CheckRunner,
}

impl Default for CheckJob {
    fn default() -> Self {
        Self::with_runner(default_check_runner())
    }
}

impl CheckJob {
    pub fn with_runner(runner: CheckRunner) -> Self {
        Self {
            run: Run::default(),
            runner,
        }
    }

    pub fn is_running(&self) -> bool {
        self.run.is_running()
    }

    pub fn failure(&self) -> Option<&str> {
        self.run.failure()
    }

    pub fn was_cancelled(&self) -> bool {
        self.run.was_cancelled()
    }

    pub fn start(&mut self, plan: CheckPlan, ctx: &egui::Context) {
        let runner = self.runner.clone();
        self.run.start(
            "faris-design-check",
            ctx,
            move |cancel, sink: Sink<Snapshot>| {
                let mut options = plan.options;
                options.cancellation = cancel;
                options.progress = Some(Arc::new(move |stage: &str, done, total| {
                    sink.update(|s| s.apply(stage, done, total));
                }));
                let report = runner(&plan.design, &plan.tools, &options)?;
                let saved = save_report(&plan.report_path, &report);
                Ok(CheckDone {
                    report: Box::new(report),
                    saved,
                    design: plan.design,
                    work_dir: plan.work_dir,
                })
            },
        );
    }

    pub fn cancel(&mut self) {
        self.run.cancel();
    }

    pub fn poll(&mut self) -> Option<CheckDone> {
        self.run.poll()
    }

    pub fn running_view(&self) -> Option<CheckRunning> {
        self.run.running_view().map(|v| CheckRunning {
            wall_seconds: v.wall_seconds,
            cancelling: v.cancelling,
            progress: progress_view(&v.progress),
        })
    }

    #[cfg(test)]
    pub fn cancel_acknowledged_after(&self) -> Option<crate::time::Duration> {
        self.run.cancel_acknowledged_after()
    }
}

/// The failure as a sentence with its next step.
pub fn check_failure_text(error: &str) -> String {
    let error = error.trim().trim_end_matches('.');
    format!(
        "The check could not run, so the design was not judged: {error}. Next step: fix what the message names and press Check again. The work folder is kept, so what was already converted is reused."
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::design_check_result::tests::passing_report;
    use crate::design_shared::ToolState;
    use crate::time::{Duration, Instant};

    /// Files and folders every required input points at.
    fn tree() -> (tempfile::TempDir, Inputs) {
        let dir = tempfile::tempdir().unwrap();
        let p = |n: &str| dir.path().join(n);
        for f in [
            "machine.step",
            "machine.faris-design.json",
            "python",
            "xs.xml",
        ] {
            std::fs::write(p(f), "{}").unwrap();
        }
        std::fs::create_dir(p("source")).unwrap();
        let inputs = Inputs {
            step: p("machine.step").display().to_string(),
            design: p("machine.faris-design.json").display().to_string(),
            cad_python: p("python").display().to_string(),
            openmc_python: p("python").display().to_string(),
            cross_sections: p("xs.xml").display().to_string(),
            plasma_source: p("source").display().to_string(),
            ..Inputs::default()
        };
        (dir, inputs)
    }

    fn fields(problems: &[Problem]) -> Vec<Field> {
        problems.iter().filter_map(|p| p.field).collect()
    }

    fn refused_check(inputs: &Inputs) -> Vec<Problem> {
        match validate_check(inputs) {
            Err(p) => p,
            Ok(_) => panic!("the request was accepted"),
        }
    }

    fn refused_init(inputs: &Inputs) -> Vec<Problem> {
        match validate_init(inputs) {
            Err(p) => p,
            Ok(_) => panic!("the request was accepted"),
        }
    }

    #[test]
    fn defaults_are_the_command_lines() {
        let plan = validate_check(&tree().1).unwrap();
        let o = &plan.options;
        assert_eq!(o.seed, 1);
        assert_eq!(o.samples, 1_000_000);
        assert_eq!(o.chords, 100_000);
        assert_eq!(o.source_sites, 100_000);
        assert_eq!(o.deviation_points, 2000);
        assert_eq!(o.imprint_threads, 4);
        assert!(o.reuse);
    }

    #[test]
    fn the_default_design_file_sits_beside_the_step_file() {
        assert_eq!(
            default_design_path(Path::new("/cad/machine.step")),
            PathBuf::from("/cad/machine.faris-design.json")
        );
        assert_eq!(
            default_design_path(Path::new("tokamak.stp")),
            PathBuf::from("tokamak.faris-design.json")
        );
    }

    #[test]
    fn a_valid_init_request_builds_the_command_lines_arguments() {
        let (dir, mut inputs) = tree();
        inputs.length_unit = UnitChoice::Mm;
        inputs.design_out = dir
            .path()
            .join("new.faris-design.json")
            .display()
            .to_string();
        let plan = validate_init(&inputs).unwrap();
        assert_eq!(plan.out, dir.path().join("new.faris-design.json"));
        assert_eq!(plan.unit, Some(LengthUnit::Mm));
        assert!(plan.tools.cad_python.is_some());
    }

    #[test]
    fn init_names_what_is_wrong_and_never_overwrites() {
        let problems = refused_init(&Inputs::default());
        assert_eq!(fields(&problems), [Field::Step]);
        assert!(problems[0].text().contains("Next step"));
        let (dir, mut inputs) = tree();
        // The default design file name is taken by the design file in the tree.
        inputs.step = dir.path().join("machine.step").display().to_string();
        let problems = refused_init(&inputs);
        assert_eq!(fields(&problems), [Field::DesignOut]);
        assert!(problems[0].why.contains("already exists"));
        inputs.design_out = dir.path().join("missing/x.json").display().to_string();
        assert_eq!(fields(&refused_init(&inputs)), [Field::DesignOut]);
        inputs.design_out = dir
            .path()
            .join("new.faris-design.json")
            .display()
            .to_string();
        assert!(validate_init(&inputs).is_ok());
    }

    #[test]
    fn init_does_not_need_the_cad_python_but_a_wrong_path_stops_it() {
        let (dir, mut inputs) = tree();
        inputs.design_out = dir
            .path()
            .join("new.faris-design.json")
            .display()
            .to_string();
        inputs.cad_python.clear();
        assert!(validate_init(&inputs).is_ok());
        let rows = init_tool_rows(&inputs);
        assert!(rows[0].statement().starts_with("Not set."));
        assert!(rows[0].statement().contains("faris-occt-worker"));
        inputs.cad_python = "/no/such/python".into();
        assert_eq!(fields(&refused_init(&inputs)), [Field::CadPython]);
    }

    #[test]
    fn check_names_each_missing_input_with_why_and_next_step() {
        let problems = refused_check(&Inputs::default());
        assert_eq!(fields(&problems), [Field::Design, Field::CadPython]);
        assert!(problems.iter().all(|p| p.text().contains("Next step")));
    }

    #[test]
    fn the_check_tool_list_says_which_is_missing_and_what_each_is_for() {
        let rows = check_tool_rows(&Inputs::default());
        assert_eq!(rows.len(), 4);
        let cad = rows[0].statement();
        assert!(cad.starts_with("Missing."), "{cad}");
        assert!(cad.contains("cadquery") && cad.contains("FARIS_CAD_PYTHON"));
        for row in &rows[1..] {
            let text = row.statement();
            assert!(text.starts_with("Not set."), "{text}");
            assert!(text.contains("NOT_EVALUATED"), "{text}");
            assert!(text.contains("flux cannot be run"), "{text}");
        }
        let (_dir, mut inputs) = tree();
        inputs.cross_sections = "/no/such/xs.xml".into();
        let rows = check_tool_rows(&inputs);
        assert_eq!(rows[0].state, ToolState::Set);
        assert!(
            rows[2]
                .statement()
                .contains("/no/such/xs.xml is not a file that exists")
        );
    }

    #[test]
    fn the_optional_check_tools_may_stay_empty() {
        let (_dir, mut inputs) = tree();
        inputs.openmc_python.clear();
        inputs.cross_sections.clear();
        inputs.plasma_source.clear();
        let plan = validate_check(&inputs).unwrap();
        assert!(plan.tools.openmc_python.is_none());
        inputs.plasma_source = inputs.design.clone();
        assert_eq!(fields(&refused_check(&inputs)), [Field::PlasmaSource]);
    }

    #[test]
    fn work_folder_and_report_default_beside_the_design_file_and_are_kept() {
        let (dir, mut inputs) = tree();
        let plan = validate_check(&inputs).unwrap();
        assert_eq!(plan.work_dir, dir.path().join("machine-check-work"));
        assert_eq!(plan.options.work_dir, Some(plan.work_dir.clone()));
        assert_eq!(
            plan.report_path,
            dir.path().join("machine-check-report.json")
        );
        // An existing report is never reused; an existing work folder is (that is its purpose).
        std::fs::write(&plan.report_path, "{}").unwrap();
        std::fs::create_dir(&plan.work_dir).unwrap();
        let again = validate_check(&inputs).unwrap();
        assert_eq!(
            again.report_path,
            dir.path().join("machine-check-report-2.json")
        );
        assert_eq!(again.work_dir, plan.work_dir);
        inputs.no_reuse = true;
        assert!(!validate_check(&inputs).unwrap().options.reuse);
    }

    #[test]
    fn an_explicit_report_must_be_new_and_a_work_folder_must_be_a_folder() {
        let (dir, mut inputs) = tree();
        inputs.report = dir.path().join("python").display().to_string();
        assert_eq!(fields(&refused_check(&inputs)), [Field::Report]);
        inputs.report = dir.path().join("missing/r.json").display().to_string();
        assert_eq!(fields(&refused_check(&inputs)), [Field::Report]);
        inputs.report.clear();
        inputs.work_dir = dir.path().join("python").display().to_string();
        assert_eq!(fields(&refused_check(&inputs)), [Field::WorkDir]);
        inputs.work_dir = dir.path().join("missing/work").display().to_string();
        assert_eq!(fields(&refused_check(&inputs)), [Field::WorkDir]);
        inputs.work_dir = dir.path().join("work").display().to_string();
        assert!(validate_check(&inputs).is_ok());
    }

    #[test]
    fn progress_is_the_stage_line_and_never_a_made_up_total() {
        let mut snapshot = Snapshot::default();
        let start = progress_view(&snapshot);
        assert!(start.line.starts_with("Starting"));
        assert!(start.fraction.is_none());
        snapshot.apply("sampling points", 250, 1000);
        snapshot.apply("sampling points", 500, 1000);
        let view = progress_view(&snapshot);
        assert_eq!(view.line, "sampling points: 500 of 1000");
        assert_eq!(view.fraction, Some(0.5));
        assert!(view.stages_seen.starts_with("1 counted"));
        snapshot.apply("random chords", 0, 0);
        let view = progress_view(&snapshot);
        assert_eq!(view.line, "random chords: 0 of 0");
        assert_eq!(view.fraction, None, "no count, no bar");
        assert!(view.stages_seen.starts_with("2 counted"));
        assert!(NO_ESTIMATE.contains("No time estimate"));
    }

    fn wait(job: &mut CheckJob) -> Option<CheckDone> {
        let started = Instant::now();
        while job.is_running() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "check did not end"
            );
            if let Some(done) = job.poll() {
                return Some(done);
            }
            crate::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    #[test]
    fn a_check_reports_progress_and_saves_a_new_report() {
        let (dir, inputs) = tree();
        let plan = validate_check(&inputs).unwrap();
        let report_path = plan.report_path.clone();
        let mut job = CheckJob::with_runner(Arc::new(|_, _, options| {
            let progress = options.progress.as_ref().unwrap();
            progress("sampling points", 10, 20);
            Ok(passing_report())
        }));
        job.start(plan, &egui::Context::default());
        assert!(job.is_running());
        let done = wait(&mut job).expect("a result");
        assert_eq!(done.saved.unwrap(), report_path);
        assert_eq!(done.work_dir, dir.path().join("machine-check-work"));
        let back: ImportReport =
            serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
        assert_eq!(back, *done.report);
        // A second save onto the same path is refused, never overwritten.
        assert!(save_report(&report_path, &back).is_err());
    }

    #[test]
    fn cancel_is_acknowledged_at_once_and_reaches_the_engine() {
        let (_dir, inputs) = tree();
        let plan = validate_check(&inputs).unwrap();
        let mut job = CheckJob::with_runner(Arc::new(|_, _, options| {
            let started = Instant::now();
            while !options.cancellation.is_cancelled() {
                assert!(started.elapsed() < Duration::from_secs(10));
                crate::thread::sleep(Duration::from_millis(2));
            }
            Err("the check was cancelled".into())
        }));
        job.start(plan, &egui::Context::default());
        assert!(!job.running_view().unwrap().cancelling);
        job.cancel();
        assert!(job.running_view().unwrap().cancelling);
        assert!(job.cancel_acknowledged_after().unwrap() < Duration::from_millis(100));
        assert!(wait(&mut job).is_none());
        assert!(job.was_cancelled());
    }

    #[test]
    fn a_failed_check_keeps_its_message_with_a_next_step() {
        let (_dir, inputs) = tree();
        let plan = validate_check(&inputs).unwrap();
        let mut job = CheckJob::with_runner(Arc::new(|_, _, _| Err("python exited with 1".into())));
        job.start(plan, &egui::Context::default());
        assert!(wait(&mut job).is_none());
        assert_eq!(job.failure(), Some("python exited with 1"));
        let text = check_failure_text(job.failure().unwrap());
        assert!(text.contains("the design was not judged: python exited with 1. Next step"));
    }

    #[test]
    fn init_hands_back_the_summary_and_the_nulls_it_left() {
        let (dir, mut inputs) = tree();
        let out = dir.path().join("new.faris-design.json");
        inputs.design_out = out.display().to_string();
        let plan = validate_init(&inputs).unwrap();
        let written = out.clone();
        let mut job = InitJob::with_runner(Arc::new(move |_, out, unit, _, _| {
            assert_eq!(unit, None);
            std::fs::write(
                out,
                r#"{"solids":[{"id":"a","material_id":null},{"id":"b","material_id":null}],"plasma":{"major_radius_m":null}}"#,
            )
            .unwrap();
            Ok(InitSummary {
                path: written.clone(),
                solids: 2,
                length_unit: LengthUnit::Mm,
                unit_declared_by_file: true,
                step_sha256: "0".into(),
                nulls_to_fill: 3,
            })
        }));
        job.start(plan, &egui::Context::default());
        let started = Instant::now();
        let done = loop {
            assert!(started.elapsed() < Duration::from_secs(10));
            if let Some(done) = job.poll() {
                break done;
            }
            crate::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(done.summary.solids, 2);
        let mut nulls = done.null_paths.unwrap();
        nulls.sort();
        assert_eq!(
            nulls,
            [
                "plasma.major_radius_m",
                "solids[0].material_id",
                "solids[1].material_id"
            ]
        );
        assert!(init_failure_text("the file holds no solids.").contains("Next step"));
    }

    #[test]
    fn the_nulls_of_a_file_that_is_not_json_say_so() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.json");
        std::fs::write(&path, "not json").unwrap();
        assert!(
            read_null_paths(&path)
                .unwrap_err()
                .contains("not valid JSON")
        );
        assert!(read_null_paths(&dir.path().join("missing.json")).is_err());
    }
}
