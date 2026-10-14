//! What the Design check and Design flux windows share: the tool list (what each tool is for
//! and what to do when it is missing), the input problems, the file rows and the run state
//! machine (start on a worker thread, live progress, cancel, collect). Nothing here owns
//! physics; the windows keep their own inputs and their own wording of results.

use crate::time::{Instant};
use crate::badge::Kind;
use eframe::egui;
use faris_engine::jobs::Cancellation;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, TryRecvError},
    },
};

/// What a tool path is, what it is for and what happens without it.
pub struct ToolInfo<F: 'static> {
    pub field: F,
    pub label: &'static str,
    pub purpose: &'static str,
    /// What the run lacks when an optional path is not set; empty for a required one.
    pub without: &'static str,
    pub environment: Option<&'static str>,
    pub is_folder: bool,
}

impl<F> ToolInfo<F> {
    pub fn required(&self) -> bool {
        self.without.is_empty()
    }
}

/// Where a tool path stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolState {
    Set,
    /// Nothing entered.
    Missing,
    /// Entered, but there is no such file or folder.
    NotFound(PathBuf),
}

/// One row of a tool list: what it is, what it is for and where it stands.
pub struct ToolRow<F: 'static> {
    pub info: &'static ToolInfo<F>,
    pub state: ToolState,
}

impl<F> ToolRow<F> {
    /// A plain statement of the state: which is missing, what it is for, what to do.
    pub fn statement(&self) -> String {
        let info = self.info;
        let kind = if info.is_folder { "folder" } else { "file" };
        match (&self.state, info.required()) {
            (ToolState::Set, _) => format!("Set. {}", info.purpose),
            (ToolState::Missing, true) => format!(
                "Missing. {} Next step: press Find tools in the Tools section, or choose the {kind}{}.",
                info.purpose,
                info.environment
                    .map(|e| format!(" or set {e} before starting FARIS"))
                    .unwrap_or_default()
            ),
            (ToolState::Missing, false) => {
                format!("Not set. {} Without it: {}", info.purpose, info.without)
            }
            (ToolState::NotFound(path), _) => format!(
                "{} is not a {kind} that exists. {} Next step: choose the {kind} again in the Tools section.",
                path.display(),
                info.purpose
            ),
        }
    }

    /// Does this row stop a run?
    pub fn blocks(&self) -> bool {
        match (&self.state, self.info.required()) {
            (ToolState::Set, _) => false,
            (ToolState::Missing, required) => required,
            (ToolState::NotFound(_), _) => true,
        }
    }
}

/// The rows of a tool list, each judged against the text its field holds.
pub fn tool_rows<'a, F: Copy>(
    tools: &'static [ToolInfo<F>],
    text: impl Fn(F) -> &'a str,
) -> Vec<ToolRow<F>> {
    tools
        .iter()
        .map(|info| {
            let state = match trimmed(text(info.field)) {
                None => ToolState::Missing,
                Some(path) if path.is_dir() == info.is_folder && path.exists() => ToolState::Set,
                Some(path) => ToolState::NotFound(path),
            };
            ToolRow { info, state }
        })
        .collect()
}

/// An input that stops a run: why, and what to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem<F> {
    /// The input at fault; `None` for the limits.
    pub field: Option<F>,
    pub why: String,
    pub next_step: String,
}

impl<F> Problem<F> {
    pub fn new(
        field: impl Into<Option<F>>,
        why: impl Into<String>,
        next: impl Into<String>,
    ) -> Self {
        Self {
            field: field.into(),
            why: why.into(),
            next_step: next.into(),
        }
    }

    pub fn text(&self) -> String {
        format!("{} Next step: {}", self.why, self.next_step)
    }
}

pub fn trimmed(text: &str) -> Option<PathBuf> {
    let text = text.trim();
    (!text.is_empty()).then(|| PathBuf::from(text))
}

/// The folder a file sits in; `.` for a bare name.
pub fn beside(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

/// How many things stop a run, said once under the Run button; each is marked beside its field.
pub fn missing_count_text(count: usize) -> String {
    match count {
        0 => String::new(),
        1 => "1 thing needs fixing before this can run; it is marked beside its field.".into(),
        n => {
            format!("{n} things need fixing before this can run; each is marked beside its field.")
        }
    }
}

// ---------------------------------------------------------------------- rows --

pub const FIELD_WIDTH: f32 = 520.0;

/// One grid row: label, text field, Browse. A note (a problem, say) goes on its own line under
/// the field. True when Browse was pressed.
pub fn path_row(
    ui: &mut egui::Ui,
    label: &str,
    hint: &str,
    text: &mut String,
    note: Option<(Kind, &str)>,
) -> bool {
    path_row_enabled(ui, label, hint, text, note, true)
}

/// [`path_row`] that can be shown read-only (a value the environment supplies).
pub fn path_row_enabled(
    ui: &mut egui::Ui,
    label: &str,
    hint: &str,
    text: &mut String,
    note: Option<(Kind, &str)>,
    enabled: bool,
) -> bool {
    ui.label(label).on_hover_text(hint);
    ui.add_sized(
        [FIELD_WIDTH, ui.spacing().interact_size.y],
        egui::TextEdit::singleline(text)
            .hint_text(hint)
            .interactive(enabled),
    );
    let clicked = ui
        .add_enabled(enabled, egui::Button::new("Browse\u{2026}"))
        .clicked();
    ui.end_row();
    if let Some((kind, note)) = note {
        ui.label("");
        ui.add(egui::Label::new(egui::RichText::new(note).small().color(kind.color())).wrap());
        ui.end_row();
    }
    clicked
}

/// The tools a window (or one of its actions) needs, each with the statement of where it
/// stands. The values themselves are set once, in the shared Tools section.
pub fn needs_list<F: Copy>(ui: &mut egui::Ui, rows: &[ToolRow<F>]) {
    for tool in rows {
        let info = tool.info;
        let kind = match (&tool.state, info.required()) {
            (ToolState::Set, _) => Kind::Calculated,
            (_, true) => Kind::Failed,
            (ToolState::Missing, false) => Kind::NotEvaluated,
            (ToolState::NotFound(_), false) => Kind::Failed,
        };
        ui.horizontal_wrapped(|ui| {
            ui.strong(info.label);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(tool.statement())
                        .small()
                        .color(kind.color()),
                )
                .wrap(),
            );
        });
    }
}

// ------------------------------------------------------------------ run state --

/// What a worker reports into: a mutex-guarded snapshot and a repaint request.
pub struct Sink<S> {
    slot: Arc<Mutex<S>>,
    context: egui::Context,
}

impl<S> Clone for Sink<S> {
    fn clone(&self) -> Self {
        Self {
            slot: self.slot.clone(),
            context: self.context.clone(),
        }
    }
}

impl<S> Sink<S> {
    pub fn update(&self, change: impl FnOnce(&mut S)) {
        if let Ok(mut slot) = self.slot.lock() {
            change(&mut slot);
        }
        self.context.request_repaint();
    }
}

enum Outcome<R> {
    Done(R),
    Failed(String),
    Cancelled,
}

enum State<S, R> {
    Idle,
    Running {
        started: Instant,
        cancel: Cancellation,
        cancel_requested: Option<Instant>,
        progress: Arc<Mutex<S>>,
        receiver: Receiver<Outcome<R>>,
    },
    Done,
    Failed(String),
    Cancelled,
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
pub struct RunningView<S> {
    pub wall_seconds: f64,
    pub cancelling: bool,
    pub progress: S,
}

/// One job on a worker thread: progress of type `S` while it goes, a result of type `R`.
pub struct Run<S, R> {
    state: State<S, R>,
}

impl<S, R> Default for Run<S, R> {
    fn default() -> Self {
        Self { state: State::Idle }
    }
}

impl<S: Default + Clone + Send + 'static, R: Send + 'static> Run<S, R> {
    pub fn phase(&self) -> Phase {
        match &self.state {
            State::Idle => Phase::Idle,
            State::Running { .. } => Phase::Running,
            State::Done => Phase::Done,
            State::Failed(_) => Phase::Failed,
            State::Cancelled => Phase::Cancelled,
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

    pub fn was_cancelled(&self) -> bool {
        self.phase() == Phase::Cancelled
    }

    /// Starts the worker. `work` gets the cancellation and the sink it reports progress to.
    pub fn start(
        &mut self,
        thread_name: &str,
        ctx: &egui::Context,
        work: impl FnOnce(Cancellation, Sink<S>) -> Result<R, String> + Send + 'static,
    ) {
        if self.is_running() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let cancel = Cancellation::default();
        let progress = Arc::new(Mutex::new(S::default()));
        let sink = Sink {
            slot: progress.clone(),
            context: ctx.clone(),
        };
        let worker_cancel = cancel.clone();
        let context = ctx.clone();
        let spawned = crate::thread::Builder::new()
            .name(thread_name.into())
            .spawn(move || {
                let outcome = match work(worker_cancel.clone(), sink) {
                    Ok(result) => Outcome::Done(result),
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
                progress,
                receiver,
            },
            Err(error) => State::Failed(format!("cannot start the worker thread: {error}")),
        };
    }

    /// Asks the job to stop; the panel says so at once.
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

    /// Collects a finished job. A successful result is returned once.
    pub fn poll(&mut self) -> Option<R> {
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
            Outcome::Done(result) => {
                self.state = State::Done;
                Some(result)
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

    /// What to draw while the job goes; `None` otherwise.
    pub fn running_view(&self) -> Option<RunningView<S>> {
        let State::Running {
            started,
            cancel_requested,
            progress,
            ..
        } = &self.state
        else {
            return None;
        };
        Some(RunningView {
            wall_seconds: started.elapsed().as_secs_f64(),
            cancelling: cancel_requested.is_some(),
            progress: progress.lock().map(|s| s.clone()).unwrap_or_default(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::Duration;

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum F {
        Tool,
        Folder,
        Optional,
    }

    static TOOLS: [ToolInfo<F>; 3] = [
        ToolInfo {
            field: F::Tool,
            label: "Tool",
            purpose: "Does the work.",
            without: "",
            environment: Some("TOOL_ENV"),
            is_folder: false,
        },
        ToolInfo {
            field: F::Folder,
            label: "Folder",
            purpose: "Holds the package.",
            without: "",
            environment: None,
            is_folder: true,
        },
        ToolInfo {
            field: F::Optional,
            label: "Extra",
            purpose: "Speeds it up.",
            without: "It is slower.",
            environment: None,
            is_folder: false,
        },
    ];

    #[test]
    fn a_missing_tool_says_what_it_is_for_and_what_to_do() {
        let rows = tool_rows(&TOOLS, |_| "");
        assert_eq!(
            rows[0].statement(),
            "Missing. Does the work. Next step: press Find tools in the Tools section, or choose the file or set TOOL_ENV before starting FARIS."
        );
        assert!(rows[0].blocks());
        assert_eq!(
            rows[1].statement(),
            "Missing. Holds the package. Next step: press Find tools in the Tools section, or choose the folder."
        );
        assert_eq!(
            rows[2].statement(),
            "Not set. Speeds it up. Without it: It is slower."
        );
        assert!(!rows[2].blocks());
    }

    #[test]
    fn a_wrong_path_blocks_even_an_optional_tool() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("python");
        std::fs::write(&file, "").unwrap();
        let file_text = file.display().to_string();
        let folder_text = dir.path().display().to_string();
        let rows = tool_rows(&TOOLS, |f| match f {
            F::Tool => file_text.as_str(),
            F::Folder => file_text.as_str(),
            F::Optional => "/no/such/file",
        });
        assert_eq!(rows[0].state, ToolState::Set);
        assert!(
            matches!(rows[1].state, ToolState::NotFound(_)),
            "a file is not a folder"
        );
        assert!(rows[1].statement().contains("is not a folder that exists"));
        assert!(rows[2].blocks());
        let rows = tool_rows(&TOOLS, |f| match f {
            F::Folder => folder_text.as_str(),
            _ => "",
        });
        assert_eq!(rows[1].state, ToolState::Set);
    }

    #[test]
    fn the_count_is_said_once_and_in_the_singular_when_it_is_one() {
        assert_eq!(missing_count_text(0), "");
        assert!(missing_count_text(1).starts_with("1 thing needs fixing"));
        assert!(missing_count_text(4).starts_with("4 things need fixing"));
    }

    fn wait<S: Default + Clone + Send + 'static, R: Send + 'static>(
        run: &mut Run<S, R>,
    ) -> Option<R> {
        let started = Instant::now();
        while run.is_running() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "job did not end"
            );
            if let Some(done) = run.poll() {
                return Some(done);
            }
            crate::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    #[test]
    fn a_job_reports_progress_and_hands_over_its_result_once() {
        let mut run: Run<u32, &'static str> = Run::default();
        run.start("test-job", &egui::Context::default(), |_, sink| {
            sink.update(|n| *n = 7);
            Ok("done")
        });
        assert_eq!(run.phase(), Phase::Running);
        assert_eq!(wait(&mut run), Some("done"));
        assert_eq!(run.phase(), Phase::Done);
        assert_eq!(run.poll(), None);
    }

    #[test]
    fn cancel_is_acknowledged_at_once_and_reaches_the_worker() {
        let mut run: Run<(), ()> = Run::default();
        run.start("test-job", &egui::Context::default(), |cancel, _| {
            let started = Instant::now();
            while !cancel.is_cancelled() {
                assert!(started.elapsed() < Duration::from_secs(10));
                crate::thread::sleep(Duration::from_millis(2));
            }
            Err("cancelled".into())
        });
        assert!(!run.running_view().unwrap().cancelling);
        run.cancel();
        assert!(run.running_view().unwrap().cancelling);
        assert!(run.cancel_acknowledged_after().unwrap() < Duration::from_millis(100));
        assert_eq!(wait(&mut run), None);
        assert!(run.was_cancelled());
    }

    #[test]
    fn a_failed_job_keeps_its_message() {
        let mut run: Run<(), ()> = Run::default();
        run.start("test-job", &egui::Context::default(), |_, _| {
            Err("no tool".into())
        });
        assert_eq!(wait(&mut run), None);
        assert_eq!(run.phase(), Phase::Failed);
        assert_eq!(run.failure(), Some("no tool"));
    }
}
