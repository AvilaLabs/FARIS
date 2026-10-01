//! The app side of `.faris` study files: the File menu, opening and saving on
//! worker threads, the window title, and the Evidence-step badge. Reading,
//! writing and verification live in `faris-study`; nothing here interprets
//! physics.

use crate::{
    Arguments, FarisApp, Session, SessionInputs, Step, badge::Kind, load_sweep, study_panel,
    sweep_panel, transport_panel::FieldView,
};
use clap::ValueEnum;
use eframe::egui;
use faris_study::{
    ArrangementDraft, EvidenceDraft, EvidenceMode, EvidenceState, MissReason, StudyDraft,
    StudyReader, ViewState, WriteReport, extract_tar_gz, write_study,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, TryRecvError},
    },
    time::Duration,
};

pub const EXTENSION: &str = "faris";

/// "FARIS — name.faris", with " •" when the view differs from the saved one.
pub fn window_title(path: Option<&Path>, dirty: bool) -> String {
    match path.and_then(|p| p.file_name()) {
        Some(name) => format!(
            "FARIS — {}{}",
            name.to_string_lossy(),
            if dirty { " •" } else { "" }
        ),
        None => "FARIS — unsaved study".into(),
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1e6)
}

/// How the evidence layer of an opened or saved file reads in the status bar.
pub fn evidence_summary(state: &EvidenceState) -> String {
    match state.mode {
        None => "no Core evidence recorded".into(),
        Some(EvidenceMode::Packed) => "Core evidence packed in the file".into(),
        Some(EvidenceMode::Referenced) if state.missing.is_empty() => {
            "Core evidence referenced, found beside the file".into()
        }
        Some(EvidenceMode::Referenced) => format!(
            "Core evidence referenced, {} of {} archives missing",
            state.missing.len(),
            state.missing.len() + state.available.len()
        ),
    }
}

pub fn status_line(path: &Path, verb: &str, bytes: u64, evidence: &str) -> String {
    format!(
        "{verb} {} · {} · {evidence}",
        path.file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        megabytes(bytes)
    )
}

pub struct EvidenceBadge {
    pub kind: Kind,
    pub label: &'static str,
    pub text: String,
}

/// The Evidence-step badge for a study whose receipts were saved by reference
/// and are not (all) beside the file. States why and the next step, and lists
/// the recorded hashes so anyone can check archives they are given later.
pub fn evidence_badge(state: &EvidenceState) -> Option<EvidenceBadge> {
    if state.mode != Some(EvidenceMode::Referenced) || state.missing.is_empty() {
        return None;
    }
    let (kind, label) = if state.available.is_empty() {
        (Kind::NotEvaluated, "Core receipts not included")
    } else {
        (Kind::Partial, "some Core receipts not included")
    };
    let mut text = String::from(
        "Why: this study file was saved with its Core evidence by reference, not included. \
         The transport, histories and assumptions are complete and shown, but no Core receipts \
         were re-checked on opening, so nothing here claims Core verification.\n\n",
    );
    for miss in &state.missing {
        if miss.reason == MissReason::HashMismatch {
            text.push_str(&format!(
                "{} was found beside the file but its SHA-256 differs from the recorded one, so it was not used.\n",
                miss.archive.file_name
            ));
        }
    }
    text.push_str(
        "Next step: put the archives below next to the .faris file, at the same relative paths, \
         then reopen it; or open the original study and save again with \"Include Core evidence \
         in saved files\" checked.\n\nRecorded archives:\n",
    );
    for miss in &state.missing {
        text.push_str(&format!(
            "  {} (not found)\n    sha256:{}\n    {} bytes\n",
            miss.archive.file_name, miss.archive.sha256, miss.archive.bytes
        ));
    }
    for found in &state.available {
        text.push_str(&format!(
            "  {} (found, hash matches)\n",
            found.archive.file_name
        ));
    }
    Some(EvidenceBadge { kind, label, text })
}

/// The study's inputs as files, plus whether this session can be saved.
#[derive(Clone, Default)]
pub struct StudyInputs {
    pub draft: StudyDraft,
    /// Why Save is unavailable (inputs a study file cannot hold), if so.
    pub unsaveable: Option<String>,
}

impl StudyInputs {
    pub fn from_arguments(args: &Arguments) -> Self {
        let arrangement = |scenario: &Option<PathBuf>, physics: &[PathBuf], bundles: &[PathBuf]| {
            (scenario.is_some() || !physics.is_empty() || !bundles.is_empty()).then(|| {
                ArrangementDraft {
                    scenario: scenario.clone(),
                    physics: physics.to_vec(),
                    bundles: bundles.to_vec(),
                }
            })
        };
        let draft = StudyDraft {
            port: arrangement(&args.scenario, &args.physics, &args.bundle),
            control: arrangement(
                &args.control_scenario,
                &args.control_physics,
                &args.control_bundle,
            ),
            sweep: args.sweep_bundle.clone(),
            assumptions: args.assumptions.clone(),
            ..StudyDraft::default()
        };
        let unsaveable = if !args.run.is_empty() || !args.control_run.is_empty() {
            Some("This session loaded local run records (--run); a study file holds recorded-transport bundles. Load them with --bundle to save.".into())
        } else if args.study.is_none()
            && draft.port.as_ref().is_none_or(|a| a.bundles.is_empty())
            && draft.control.as_ref().is_none_or(|a| a.bundles.is_empty())
            && draft.sweep.is_empty()
        {
            Some("Nothing to save: no recorded transport is loaded.".into())
        } else {
            None
        };
        Self { draft, unsaveable }
    }
}

/// A study unpacked into the workspace directory, ready to load.
struct OpenedStudy {
    path: PathBuf,
    file_bytes: u64,
    view: ViewState,
    inputs: StudyInputs,
    evidence: EvidenceState,
    sweep: Vec<PathBuf>,
    descriptors: Vec<PathBuf>,
    marker: Option<PathBuf>,
    workspace: tempfile::TempDir,
}

type OpenResult = Result<(Session, OpenedStudy), String>;

enum Task {
    Idle,
    Opening {
        name: String,
        progress: Arc<Mutex<String>>,
        receiver: Receiver<OpenResult>,
    },
    Saving {
        path: PathBuf,
        view: Box<ViewState>,
        receiver: Receiver<Result<WriteReport, String>>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Open,
    Save,
    SaveAs,
}

type DialogResult = (Action, Option<PathBuf>);

pub struct FileState {
    pub path: Option<PathBuf>,
    inputs: StudyInputs,
    saved_view: Option<ViewState>,
    pub include_evidence: bool,
    runs_directory: PathBuf,
    /// Keeps an opened study's unpacked files (and extracted evidence) alive.
    workspace: Option<tempfile::TempDir>,
    task: Task,
    dialog: Option<Receiver<DialogResult>>,
    auto_open: Option<PathBuf>,
    last_title: String,
    pub evidence: EvidenceState,
}

impl Default for FileState {
    fn default() -> Self {
        Self::new(StudyInputs::default(), PathBuf::from("runs"))
    }
}

impl FileState {
    pub fn new(inputs: StudyInputs, runs_directory: PathBuf) -> Self {
        Self {
            path: None,
            inputs,
            saved_view: None,
            include_evidence: false,
            runs_directory,
            workspace: None,
            task: Task::Idle,
            dialog: None,
            auto_open: None,
            last_title: String::new(),
            evidence: EvidenceState::default(),
        }
    }

    /// Open this file as soon as the first frame is up.
    pub fn request_open(&mut self, path: PathBuf) {
        self.auto_open = Some(path);
    }

    pub fn is_busy(&self) -> bool {
        !matches!(self.task, Task::Idle) || self.dialog.is_some() || self.auto_open.is_some()
    }

    fn is_saving(&self) -> bool {
        matches!(self.task, Task::Saving { .. })
    }

    fn is_opening(&self) -> bool {
        matches!(self.task, Task::Opening { .. })
    }

    fn dirty(&self, current: &ViewState) -> bool {
        self.saved_view
            .as_ref()
            .is_some_and(|saved| saved != current)
    }

    pub fn title(&self, current: &ViewState) -> String {
        window_title(self.path.as_deref(), self.dirty(current))
    }
}

fn open_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn set_progress(progress: &Mutex<String>, text: &str) {
    if let Ok(mut guard) = progress.lock() {
        *guard = text.to_owned();
    }
}

fn write_marker(path: &Path, status: &str, error: Option<String>) {
    let mut value = serde_json::json!({
        "schema_version": "faris-recorded-materialization/v0.1", "status": status,
    });
    if let Some(error) = error {
        let mut text: String = error.chars().take(512).collect();
        text.truncate(text.trim_end().len());
        value["error"] = text.into();
    }
    let staging = path.with_extension("tmp");
    if std::fs::write(&staging, serde_json::to_vec(&value).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&staging, path);
    }
}

/// Verify the file, unpack it under a fresh workspace and load it. Runs on a
/// worker thread; extraction of Core evidence continues on its own thread so
/// the study is usable before the (large) archives are on disk.
fn open_study(path: &Path, runs_directory: &Path, progress: &Mutex<String>) -> OpenResult {
    set_progress(progress, "Verifying the file…");
    let mut reader = StudyReader::open(path).map_err(|e| e.to_string())?;
    let file_bytes = reader.file_bytes;
    std::fs::create_dir_all(runs_directory)
        .map_err(|e| format!("cannot create {}: {e}", runs_directory.display()))?;
    let workspace = tempfile::Builder::new()
        .prefix("faris-study-")
        .tempdir_in(runs_directory)
        .map_err(|e| {
            format!(
                "cannot create a workspace in {}: {e}",
                runs_directory.display()
            )
        })?;
    set_progress(progress, "Unpacking recorded transport…");
    let near = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let files = reader
        .materialize(workspace.path(), Some(near))
        .map_err(|e| e.to_string())?;
    let view = reader.manifest.view.clone();

    let first = files.port.as_ref().or(files.control.as_ref());
    let second = files.port.as_ref().and(files.control.as_ref());
    let mut session_inputs = SessionInputs::empty(runs_directory.to_path_buf());
    if let Some(a) = first {
        session_inputs.scenario = a.scenario.clone();
        session_inputs.physics = a.physics.clone();
        session_inputs.bundle = a.bundles.clone();
    }
    if let Some(a) = second {
        session_inputs.control_scenario = a.scenario.clone();
        session_inputs.control_physics = a.physics.clone();
        session_inputs.control_bundle = a.bundles.clone();
    }
    session_inputs.assumptions = files.assumptions.clone();
    set_progress(progress, "Checking the recorded transport…");
    let session = crate::build_session(session_inputs)?;

    // Core evidence: extract complete case + workspace pairs where the
    // existing reopening code expects them, and say when they are ready.
    let mut descriptors = Vec::new();
    let mut jobs: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for file in &files.evidence.available {
        let key = (
            file.archive.arrangement.clone(),
            file.archive.allocation.clone(),
        );
        if !seen.insert(key.clone()) {
            continue;
        }
        let Some((case, workspace_archive)) = files.evidence.pair(&key.0, &key.1) else {
            continue;
        };
        let (arrangement, allocation) = key;
        let case_rel = format!("materialized/{arrangement}/cases/{allocation}");
        let workspace_rel = format!("materialized/{arrangement}/core-workspaces/{allocation}");
        let descriptor = workspace
            .path()
            .join(format!("saved-study-{arrangement}-{allocation}.json"));
        let body = serde_json::json!({
            "case_directory": case_rel,
            "execution_report": format!("{case_rel}/execution-report.json"),
            "execution_workspace": workspace_rel,
        });
        std::fs::write(
            &descriptor,
            serde_json::to_vec_pretty(&body).map_err(|e| e.to_string())?,
        )
        .map_err(|e| format!("cannot write {}: {e}", descriptor.display()))?;
        jobs.push((case.path.clone(), workspace.path().join(&case_rel)));
        jobs.push((
            workspace_archive.path.clone(),
            workspace.path().join(&workspace_rel),
        ));
        descriptors.push(descriptor);
    }
    let marker = (!jobs.is_empty()).then(|| workspace.path().join("materialization.json"));
    if let Some(marker) = marker.clone() {
        std::thread::Builder::new()
            .name("faris-extract-evidence".into())
            .spawn(move || {
                let result = jobs.iter().try_for_each(|(archive, destination)| {
                    std::fs::create_dir_all(destination)
                        .map_err(|e| format!("cannot create {}: {e}", destination.display()))?;
                    extract_tar_gz(archive, destination)
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                });
                match result {
                    Ok(()) => write_marker(&marker, "COMPLETE", None),
                    Err(error) => write_marker(&marker, "FAILED", Some(error)),
                }
            })
            .map_err(|e| format!("cannot start evidence extraction: {e}"))?;
    }

    let mut draft = StudyDraft {
        port: files.port.as_ref().map(|a| ArrangementDraft {
            scenario: a.scenario.clone(),
            physics: a.physics.clone(),
            bundles: a.bundles.clone(),
        }),
        control: files.control.as_ref().map(|a| ArrangementDraft {
            scenario: a.scenario.clone(),
            physics: a.physics.clone(),
            bundles: a.bundles.clone(),
        }),
        sweep: files.sweep.clone(),
        assumptions: files.assumptions.clone(),
        ..StudyDraft::default()
    };
    draft.evidence = files
        .evidence
        .available
        .iter()
        .map(|f| EvidenceDraft {
            archive: f.archive.clone(),
            path: Some(f.path.clone()),
        })
        .chain(files.evidence.missing.iter().map(|m| EvidenceDraft {
            archive: m.archive.clone(),
            path: None,
        }))
        .collect();
    Ok((
        session,
        OpenedStudy {
            path: path.to_path_buf(),
            file_bytes,
            view,
            inputs: StudyInputs {
                draft,
                unsaveable: None,
            },
            evidence: files.evidence,
            sweep: files.sweep,
            descriptors,
            marker,
            workspace,
        },
    ))
}

impl FarisApp {
    /// The view the author would want back: everything a study file records
    /// beyond its inputs.
    pub fn view_state(&self) -> ViewState {
        ViewState {
            step: enum_name(&self.step),
            preset: self.history.selected_preset().map(str::to_owned),
            what_if: self.history.what_if_values().cloned(),
            year: self.year,
            field_view: enum_name(&self.transport.view),
            history_tab: self.history.plot_name().to_owned(),
            arrangement: if self.manifest.penetration.is_some() {
                "port"
            } else {
                "control"
            }
            .into(),
            allocation: self.manifest.variants[self.variant].id.clone(),
            sweep_blanket_m: self.sweep.as_ref().and_then(|s| s.selected_blanket_m()),
        }
    }

    /// The saved file the export may name: only when the current view matches
    /// what was last saved or opened, so the hash describes what is exported.
    pub fn study_file_stamp(&self) -> Option<faris_report::StudyFileStamp> {
        let path = self.file.path.as_ref()?;
        if self.file.saved_view.is_none() || self.file.dirty(&self.view_state()) {
            return None;
        }
        let bytes = std::fs::read(path).ok()?;
        Some(faris_report::StudyFileStamp {
            file_name: path.file_name()?.to_string_lossy().into_owned(),
            sha256: format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(&bytes)),
        })
    }

    pub fn apply_view(&mut self, view: &ViewState) {
        let want_port = view.arrangement != "control";
        if self.paired.is_some() && self.manifest.penetration.is_some() != want_port {
            self.swap_arrangement();
        }
        if let Ok(field) = FieldView::from_str(&view.field_view, true) {
            self.transport.view = field;
            if let Some((_, panel)) = &mut self.paired {
                panel.view = field;
            }
        }
        if let Some(index) = self
            .manifest
            .variants
            .iter()
            .position(|v| v.id == view.allocation)
        {
            self.variant = index;
        }
        if let Ok(step) = Step::from_str(&view.step, true) {
            self.step = step;
        }
        self.year = if view.year.is_finite() {
            view.year.clamp(0.0, self.manifest.horizon_years)
        } else {
            0.0
        };
        self.history.restore_view(
            view.preset.as_deref(),
            view.what_if.as_ref(),
            &view.history_tab,
        );
        if let (Some(sweep), Some(blanket)) = (&mut self.sweep, view.sweep_blanket_m) {
            sweep.select_blanket_m(blanket);
        }
        if self.transport.view == FieldView::FluxSlice
            && let Some(record) = self
                .transport
                .record(&self.manifest.variants[self.variant].id)
        {
            self.camera.frame_bounds(
                record.mesh.lower_left_m.map(|x| x as f32),
                record.mesh.upper_right_m.map(|x| x as f32),
            );
        }
        self.rebuild()
            .unwrap_or_else(|error| self.message = error.to_string());
    }

    fn install_opened(&mut self, ctx: &egui::Context, session: Session, opened: OpenedStudy) {
        // Replace the panels that depend on the old workspace before it goes.
        let core = self.study.core_path();
        self.sweep = None;
        self.study = study_panel::StudyPanel::new(core, self.file.runs_directory.clone());
        self.manifest = session.manifest;
        self.paired = session.control;
        self.transport = session.transport;
        self.history = session.history;
        self.file.workspace = Some(opened.workspace);
        if !opened.sweep.is_empty() {
            let (paths, runs) = (opened.sweep.clone(), self.file.runs_directory.clone());
            self.sweep = Some(sweep_panel::SweepPanel::load_in_background(
                ctx,
                move || load_sweep(&paths, &runs),
            ));
        }
        if let Err(error) = self.reset_for_session() {
            self.message = error.to_string();
        }
        self.apply_view(&opened.view);
        if !opened.descriptors.is_empty()
            && let Err(error) = self
                .study
                .archive
                .queue_descriptors(opened.descriptors, opened.marker)
        {
            self.message = error;
        }
        let summary = evidence_summary(&opened.evidence);
        self.message = status_line(&opened.path, "Opened", opened.file_bytes, &summary);
        self.file.path = Some(opened.path);
        self.file.inputs = opened.inputs;
        self.file.evidence = opened.evidence;
        self.file.saved_view = Some(self.view_state());
    }

    fn begin_open(&mut self, ctx: &egui::Context, path: PathBuf) {
        if self.file.is_busy() && self.file.auto_open.is_none() {
            return;
        }
        self.file.auto_open = None;
        let progress = Arc::new(Mutex::new("Starting…".to_owned()));
        let (sender, receiver) = mpsc::channel();
        let (worker_progress, runs, worker_path, context) = (
            progress.clone(),
            self.file.runs_directory.clone(),
            path.clone(),
            ctx.clone(),
        );
        let spawned = std::thread::Builder::new()
            .name("faris-open-study".into())
            .spawn(move || {
                let _ = sender.send(open_study(&worker_path, &runs, &worker_progress));
                context.request_repaint();
            });
        match spawned {
            Ok(_) => {
                self.file.task = Task::Opening {
                    name: open_name(&path),
                    progress,
                    receiver,
                }
            }
            Err(error) => {
                self.message = format!("Cannot start opening {}: {error}", path.display())
            }
        }
    }

    fn begin_save(&mut self, ctx: &egui::Context, path: PathBuf) {
        if self.file.is_busy() {
            return;
        }
        if let Some(reason) = &self.file.inputs.unsaveable {
            self.message = reason.clone();
            return;
        }
        let view = self.view_state();
        let mut draft = self.file.inputs.draft.clone();
        draft.view = view.clone();
        draft.pack_evidence = self.file.include_evidence && !draft.evidence.is_empty();
        let (sender, receiver) = mpsc::channel();
        let (target, context) = (path.clone(), ctx.clone());
        let spawned = std::thread::Builder::new()
            .name("faris-save-study".into())
            .spawn(move || {
                let _ = sender.send(write_study(&target, &draft).map_err(|e| e.to_string()));
                context.request_repaint();
            });
        match spawned {
            Ok(_) => {
                self.message = format!("Saving {}…", open_name(&path));
                self.file.task = Task::Saving {
                    path,
                    view: Box::new(view),
                    receiver,
                };
            }
            Err(error) => self.message = format!("Cannot start saving: {error}"),
        }
    }

    fn start_dialog(&mut self, ctx: &egui::Context, action: Action) {
        if self.file.is_busy() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let context = ctx.clone();
        let current = self.file.path.clone();
        let spawned = std::thread::Builder::new()
            .name("faris-file-dialog".into())
            .spawn(move || {
                let dialog = rfd::FileDialog::new().add_filter("FARIS study", &[EXTENSION]);
                let picked = match action {
                    Action::Open => dialog.set_title("Open study").pick_file(),
                    _ => {
                        let mut dialog = dialog.set_title("Save study as");
                        if let Some(directory) = current.as_deref().and_then(Path::parent) {
                            dialog = dialog.set_directory(directory);
                        }
                        dialog
                            .set_file_name(
                                current.as_deref().and_then(Path::file_name).map_or_else(
                                    || "study.faris".into(),
                                    |n| n.to_string_lossy().into_owned(),
                                ),
                            )
                            .save_file()
                    }
                };
                let _ = sender.send((action, picked));
                context.request_repaint();
            });
        match spawned {
            Ok(_) => self.file.dialog = Some(receiver),
            Err(error) => self.message = format!("Cannot open a file dialog: {error}"),
        }
    }

    /// Run a File-menu action (menu, shortcut or dialog result).
    pub fn file_action(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::Open => self.start_dialog(ctx, Action::Open),
            Action::SaveAs => self.start_dialog(ctx, Action::SaveAs),
            Action::Save => match self.file.path.clone() {
                Some(path) => self.begin_save(ctx, path),
                None => self.start_dialog(ctx, Action::SaveAs),
            },
        }
    }

    /// Poll workers and dialogs, handle shortcuts and dropped files, and keep
    /// the window title current. Called once per frame.
    pub fn poll_file(&mut self, ctx: &egui::Context) {
        if self.frames >= 1
            && let Some(path) = self.file.auto_open.clone()
            && !self.file.is_opening()
        {
            self.begin_open(ctx, path);
        }
        if let Some(receiver) = &self.file.dialog {
            match receiver.try_recv() {
                Ok((action, picked)) => {
                    self.file.dialog = None;
                    if let Some(mut path) = picked {
                        match action {
                            Action::Open => self.begin_open(ctx, path),
                            _ => {
                                if path.extension().is_none_or(|e| e != EXTENSION) {
                                    let mut name = path.clone().into_os_string();
                                    name.push(format!(".{EXTENSION}"));
                                    path = PathBuf::from(name);
                                }
                                self.begin_save(ctx, path);
                            }
                        }
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.file.dialog = None,
            }
        }
        let finished = match &self.file.task {
            Task::Opening { receiver, .. } => match receiver.try_recv() {
                Ok(result) => Some(Ok(result)),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    Some(Err("The open worker ended without a result.".to_owned()))
                }
            },
            _ => None,
        };
        if let Some(result) = finished {
            let name = match &self.file.task {
                Task::Opening { name, .. } => name.clone(),
                _ => String::new(),
            };
            self.file.task = Task::Idle;
            match result.and_then(|r| r) {
                Ok((session, opened)) => self.install_opened(ctx, session, opened),
                Err(error) => self.message = format!("Cannot open {name}: {error}"),
            }
        }
        let saved = match &self.file.task {
            Task::Saving { receiver, .. } => match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    Some(Err("The save worker ended without a result.".into()))
                }
            },
            _ => None,
        };
        if let Some(result) = saved
            && let Task::Saving { path, view, .. } =
                std::mem::replace(&mut self.file.task, Task::Idle)
        {
            match result {
                Ok(report) => {
                    let mode = match report.evidence {
                        None => "no Core evidence recorded".to_owned(),
                        Some(EvidenceMode::Packed) => "Core evidence packed in the file".to_owned(),
                        Some(EvidenceMode::Referenced) => {
                            "Core evidence saved by reference".to_owned()
                        }
                    };
                    self.message = status_line(&path, "Saved", report.file_bytes, &mode);
                    self.file.path = Some(path);
                    self.file.saved_view = Some(*view);
                }
                Err(error) => self.message = format!("Cannot save {}: {error}", open_name(&path)),
            }
        }
        if !matches!(self.file.task, Task::Idle) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        // Shortcuts and dropped files.
        use egui::{Key, KeyboardShortcut, Modifiers};
        let (save_as, save, open, dropped) = ctx.input_mut(|input| {
            (
                input.consume_shortcut(&KeyboardShortcut::new(
                    Modifiers::COMMAND | Modifiers::SHIFT,
                    Key::S,
                )),
                input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::S)),
                input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::O)),
                input
                    .raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path().to_path_buf())
                    .find(|p| p.extension().is_some_and(|e| e == EXTENSION)),
            )
        });
        if let Some(path) = dropped {
            self.begin_open(ctx, path);
        } else if save_as {
            self.file_action(ctx, Action::SaveAs);
        } else if save {
            self.file_action(ctx, Action::Save);
        } else if open {
            self.file_action(ctx, Action::Open);
        }

        let current = self.view_state();
        if self.file.saved_view.is_none() && !self.file.is_busy() {
            self.file.saved_view = Some(current.clone());
        }
        let title = self.file.title(&current);
        if title != self.file.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.file.last_title = title;
        }
    }

    /// The File menu button in the top bar.
    pub fn file_menu(&mut self, ui: &mut egui::Ui) {
        let busy = self.file.is_busy();
        let mut action = None;
        ui.menu_button("File", |ui| {
            if ui
                .add_enabled(!busy, egui::Button::new("Open…").shortcut_text("Ctrl+O"))
                .clicked()
            {
                action = Some(Action::Open);
                ui.close();
            }
            let reason = self.file.inputs.unsaveable.clone();
            let save = ui.add_enabled(
                !busy && reason.is_none(),
                egui::Button::new("Save").shortcut_text("Ctrl+S"),
            );
            let save_as = ui.add_enabled(
                !busy && reason.is_none(),
                egui::Button::new("Save as…").shortcut_text("Ctrl+Shift+S"),
            );
            if let Some(reason) = &reason {
                save.clone().on_disabled_hover_text(reason);
                save_as.clone().on_disabled_hover_text(reason);
            }
            if save.clicked() {
                action = Some(Action::Save);
                ui.close();
            }
            if save_as.clicked() {
                action = Some(Action::SaveAs);
                ui.close();
            }
            ui.separator();
            ui.checkbox(
                &mut self.file.include_evidence,
                "Include Core evidence in saved files (about +50 MB)",
            )
            .on_hover_text(
                "Off: the file records the Core evidence archives' names and SHA-256 hashes, and opens fully without them. On: the archives are stored inside, so the receipts can be re-checked from this one file. Applies when the study has saved Core receipts.",
            );
        })
        .response
        .on_hover_text("Open and save .faris study files");
        if self.file.is_saving() {
            ui.spinner();
            ui.small("Saving…");
        }
        if let Some(action) = action {
            let ctx = ui.ctx().clone();
            self.file_action(&ctx, action);
        }
    }

    /// Blocks the window with a progress card while a study opens.
    pub fn opening_overlay(&self, ctx: &egui::Context) {
        let Task::Opening { name, progress, .. } = &self.file.task else {
            return;
        };
        let text = progress.lock().map(|p| p.clone()).unwrap_or_default();
        egui::Modal::new(egui::Id::new("opening-study")).show(ctx, |ui| {
            ui.set_min_width(320.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.strong(format!("Opening {name}"));
            });
            ui.add_space(6.0);
            ui.label(text);
            ui.weak("Recorded transport is checked against its hashes; histories recalculate afterwards.");
        });
    }
}

fn enum_name<T: ValueEnum>(value: &T) -> String {
    value
        .to_possible_value()
        .map_or_else(String::new, |v| v.get_name().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use faris_study::{ArchiveKind, EvidenceArchive, EvidenceFile, EvidenceMiss};

    const ASSUMPTIONS: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scenarios/arc-inspired/demo-operating-assumptions.json"
    );

    fn app() -> FarisApp {
        let session = crate::build_session(SessionInputs {
            assumptions: Some(PathBuf::from(ASSUMPTIONS)),
            ..SessionInputs::empty(PathBuf::from("runs"))
        })
        .unwrap();
        FarisApp::new(
            session.manifest,
            None,
            None,
            PathBuf::from("runs"),
            session.transport,
            session.control,
            session.history,
        )
        .unwrap()
    }

    #[test]
    fn title_names_the_file_marks_changes_and_says_unsaved() {
        let path = Path::new("/home/x/ports/study.faris");
        assert_eq!(window_title(Some(path), false), "FARIS — study.faris");
        assert_eq!(window_title(Some(path), true), "FARIS — study.faris •");
        assert_eq!(window_title(None, false), "FARIS — unsaved study");
        // Before a first save there is no file to be different from.
        assert_eq!(window_title(None, true), "FARIS — unsaved study");
    }

    #[test]
    fn a_changed_view_marks_the_title_until_it_is_saved_again() {
        let mut state = FileState::default();
        let mut view = ViewState::default();
        // Nothing recorded yet: never dirty.
        assert!(!state.dirty(&view));
        state.path = Some(PathBuf::from("a.faris"));
        state.saved_view = Some(view.clone());
        assert_eq!(state.title(&view), "FARIS — a.faris");
        view.year = 7.5;
        assert_eq!(state.title(&view), "FARIS — a.faris •");
        state.saved_view = Some(view.clone());
        assert_eq!(state.title(&view), "FARIS — a.faris");
        view.what_if = None;
        view.step = "compare".into();
        assert!(state.dirty(&view));
    }

    #[test]
    fn the_view_round_trips_through_a_fresh_app() {
        let mut first = app();
        first.step = Step::Compare;
        first.year = 12.25;
        first.variant = first.manifest.variants.len() - 1;
        first.transport.view = FieldView::ComponentFlux;
        let mut what_if = first.history.what_if_values().cloned().unwrap();
        what_if.recovery_fraction = 0.8;
        first.history.restore_view(
            Some("Baseline authored scenario"),
            Some(&what_if),
            "tritium",
        );
        let saved = first.view_state();
        assert_eq!(saved.step, "compare");
        assert_eq!(saved.field_view, "component-flux");
        assert_eq!(saved.history_tab, "tritium");
        assert_eq!(saved.preset.as_deref(), Some("Baseline authored scenario"));
        assert_eq!(saved.what_if.as_ref().unwrap().recovery_fraction, 0.8);

        let json = serde_json::to_string(&saved).unwrap();
        let mut second = app();
        assert_ne!(second.view_state(), saved);
        second.apply_view(&serde_json::from_str(&json).unwrap());
        assert_eq!(second.view_state(), saved);
        assert_eq!(second.step, Step::Compare);
        assert_eq!(second.year, 12.25);
    }

    #[test]
    fn a_saved_view_with_unknown_names_falls_back_to_defaults() {
        let mut app = app();
        let before = app.view_state();
        app.apply_view(&ViewState {
            step: "nonsense".into(),
            field_view: "nonsense".into(),
            history_tab: "nonsense".into(),
            allocation: "no-such-allocation".into(),
            preset: Some("No such preset".into()),
            year: f64::NAN,
            ..ViewState::default()
        });
        let after = app.view_state();
        assert_eq!(after.step, before.step);
        assert_eq!(after.field_view, before.field_view);
        assert_eq!(after.allocation, before.allocation);
        assert_eq!(after.year, 0.0);
    }

    #[test]
    fn a_study_file_conflicts_with_the_other_input_flags() {
        assert!(Arguments::try_parse_from(["faris-app", "study.faris"]).is_ok());
        assert!(Arguments::try_parse_from(["faris-app", "study.faris", "--core", "x"]).is_ok());
        for flag in [
            "--bundle",
            "--sweep-bundle",
            "--control-bundle",
            "--assumptions",
            "--scenario",
            "--saved-study",
        ] {
            assert!(
                Arguments::try_parse_from(["faris-app", "study.faris", flag, "x.json"]).is_err(),
                "{flag}"
            );
        }
        assert!(
            Arguments::try_parse_from(["faris-app", "study.faris", "--step", "compare"]).is_err()
        );
    }

    #[test]
    fn inputs_a_study_file_cannot_hold_disable_save_with_a_reason() {
        let parse = |args: &[&str]| {
            let mut all = vec!["faris-app"];
            all.extend_from_slice(args);
            StudyInputs::from_arguments(&Arguments::try_parse_from(all).unwrap())
        };
        assert!(
            parse(&["--run", "r.json"])
                .unsaveable
                .unwrap()
                .contains("--bundle")
        );
        assert!(parse(&[]).unsaveable.unwrap().contains("Nothing to save"));
        let ok = parse(&[
            "--bundle",
            "a.json",
            "--sweep-bundle",
            "s.json",
            "--assumptions",
            "x.json",
        ]);
        assert!(ok.unsaveable.is_none());
        assert_eq!(
            ok.draft.port.unwrap().bundles,
            vec![PathBuf::from("a.json")]
        );
        assert_eq!(ok.draft.sweep.len(), 1);
    }

    fn archive(name: &str, kind: ArchiveKind) -> EvidenceArchive {
        EvidenceArchive {
            arrangement: "port".into(),
            allocation: "reference".into(),
            kind,
            file_name: name.into(),
            sha256: "ab".repeat(32),
            bytes: 1234,
        }
    }

    #[test]
    fn missing_referenced_evidence_explains_why_and_the_next_step() {
        let missing = |name: &str, kind| EvidenceMiss {
            archive: archive(name, kind),
            reason: MissReason::NotFound,
        };
        let mut state = EvidenceState {
            mode: Some(EvidenceMode::Referenced),
            available: vec![],
            missing: vec![
                missing("port/archives/reference-case.tar.gz", ArchiveKind::Case),
                missing(
                    "port/archives/reference-workspace.tar.gz",
                    ArchiveKind::Workspace,
                ),
            ],
        };
        let note = evidence_badge(&state).unwrap();
        assert_eq!(note.kind, Kind::NotEvaluated);
        assert!(note.text.contains("Why:") && note.text.contains("by reference, not included"));
        assert!(note.text.contains("Next step:") && note.text.contains("next to the .faris"));
        assert!(note.text.contains("Include Core evidence"));
        assert!(note.text.contains("port/archives/reference-case.tar.gz"));
        assert!(note.text.contains(&"ab".repeat(32)));
        assert!(note.text.contains("nothing here claims Core verification"));

        // One archive found: partial, and the found one is named as such.
        let found = EvidenceFile {
            archive: archive("port/archives/other.tar.gz", ArchiveKind::Case),
            path: PathBuf::from("/x"),
        };
        state.available.push(found);
        let note = evidence_badge(&state).unwrap();
        assert_eq!(note.kind, Kind::Partial);
        assert!(note.text.contains("other.tar.gz (found, hash matches)"));

        // A hash mismatch is called out as such.
        state.missing[0].reason = MissReason::HashMismatch;
        assert!(
            evidence_badge(&state)
                .unwrap()
                .text
                .contains("differs from the recorded one")
        );

        // Nothing to say when everything is present, packed, or not recorded.
        state.missing.clear();
        assert!(evidence_badge(&state).is_none());
        assert!(evidence_badge(&EvidenceState::default()).is_none());
        state.mode = Some(EvidenceMode::Packed);
        assert!(evidence_badge(&state).is_none());
    }

    #[test]
    fn the_status_line_names_file_size_and_evidence() {
        let line = status_line(
            Path::new("/a/b/study.faris"),
            "Opened",
            5_200_000,
            "Core evidence packed in the file",
        );
        assert_eq!(
            line,
            "Opened study.faris · 5.2 MB · Core evidence packed in the file"
        );
        let state = EvidenceState {
            mode: Some(EvidenceMode::Referenced),
            available: vec![],
            missing: vec![EvidenceMiss {
                archive: archive("a.tar.gz", ArchiveKind::Case),
                reason: MissReason::NotFound,
            }],
        };
        assert_eq!(
            evidence_summary(&state),
            "Core evidence referenced, 1 of 1 archives missing"
        );
        assert_eq!(
            evidence_summary(&EvidenceState::default()),
            "no Core evidence recorded"
        );
    }

    /// Writes the real review-demo study, opens it the way the app does, and
    /// restores its view. Needs the packaged demo and the sweep bundles; run
    /// with `--release -- --ignored --nocapture` (checking seven sweep bundles
    /// plus four arrangements is slow in debug builds).
    #[test]
    #[ignore = "needs the packaged demo inputs and the sweep bundles on disk"]
    fn the_real_demo_study_saves_opens_and_restores_its_view() {
        let demo = PathBuf::from(
            "/home/connoravila/Documents/Avila-Labs/project-faris/dist/FARIS-demo-2026-10-01",
        );
        let sweep_dir = PathBuf::from(
            "/home/connoravila/Documents/Avila-Labs/project-faris/runs/allocation-sweep/bundles",
        );
        let bundles = |role: &str| -> Vec<PathBuf> {
            ["reference", "breeder-emphasis"]
                .iter()
                .map(|n| {
                    demo.join(role)
                        .join(format!("bundles/{n}.transport-bundle.json"))
                })
                .collect()
        };
        let mut sweep: Vec<PathBuf> = std::fs::read_dir(&sweep_dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        sweep.sort();
        let mut evidence = Vec::new();
        for arrangement in ["port", "control"] {
            for allocation in ["reference", "breeder-emphasis"] {
                evidence.extend(
                    faris_study::evidence_from_descriptor(
                        &demo.join(format!("saved-study-{arrangement}-{allocation}.json")),
                    )
                    .unwrap(),
                );
            }
        }
        let view = ViewState {
            step: "operate".into(),
            preset: Some("Loaded assumptions".into()),
            year: 11.0,
            field_view: "component-flux".into(),
            history_tab: "tritium".into(),
            arrangement: "control".into(),
            allocation: "breeder-emphasis".into(),
            sweep_blanket_m: Some(0.55),
            what_if: None,
        };
        let dir = tempfile::tempdir().unwrap();
        let mut draft = StudyDraft {
            port: Some(ArrangementDraft {
                scenario: Some(demo.join("port/scenario.json")),
                physics: vec![],
                bundles: bundles("port"),
            }),
            control: Some(ArrangementDraft {
                scenario: Some(demo.join("control/scenario.json")),
                physics: vec![],
                bundles: bundles("control"),
            }),
            sweep,
            assumptions: Some(demo.join("operating-assumptions.json")),
            evidence,
            view: view.clone(),
            ..StudyDraft::default()
        };
        // Referenced and not beside the file: opens, evidence reported missing.
        let referenced = dir.path().join("referenced.faris");
        write_study(&referenced, &draft).unwrap();
        let progress = Mutex::new(String::new());
        let started = std::time::Instant::now();
        let (session, opened) =
            open_study(&referenced, &dir.path().join("runs"), &progress).unwrap();
        let seconds = started.elapsed().as_secs_f64();
        assert!(session.control.is_some());
        assert_eq!(opened.sweep.len(), 7);
        assert_eq!(opened.evidence.missing.len(), 8);
        assert!(opened.descriptors.is_empty() && opened.marker.is_none());
        assert!(evidence_badge(&opened.evidence).is_some());
        assert_eq!(opened.view, view);
        println!("referenced study opened in {seconds:.1} s");

        // Packed: the archives come out of the file and extract for reopening.
        draft.pack_evidence = true;
        let packed = dir.path().join("packed.faris");
        write_study(&packed, &draft).unwrap();
        let (_, opened) = open_study(&packed, &dir.path().join("runs"), &progress).unwrap();
        assert_eq!(opened.evidence.available.len(), 8);
        assert_eq!(opened.descriptors.len(), 4);
        let marker = opened.marker.clone().unwrap();
        let started = std::time::Instant::now();
        while !marker.exists() {
            assert!(
                started.elapsed().as_secs() < 120,
                "evidence extraction did not finish"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        println!(
            "evidence extracted in {:.1} s",
            started.elapsed().as_secs_f64()
        );
        let status: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&marker).unwrap()).unwrap();
        assert_eq!(status["status"], "COMPLETE", "{status}");
        for descriptor in &opened.descriptors {
            let body: serde_json::Value =
                serde_json::from_slice(&std::fs::read(descriptor).unwrap()).unwrap();
            let report = descriptor
                .parent()
                .unwrap()
                .join(body["execution_report"].as_str().unwrap());
            assert!(report.is_file(), "{}", report.display());
        }
        // Restoring the saved view onto the opened session reproduces it.
        let mut restored = FarisApp::new(
            session.manifest,
            None,
            None,
            dir.path().join("runs"),
            session.transport,
            session.control,
            session.history,
        )
        .unwrap();
        restored.apply_view(&view);
        assert_eq!(restored.view_state().arrangement, "control");
        assert_eq!(restored.view_state().allocation, "breeder-emphasis");
        assert_eq!(restored.view_state().step, "operate");
    }
}
