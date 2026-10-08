//! The app side of `.faris` study files: the File menu, opening and saving on
//! worker threads, the window title, and the Evidence-step badge. Reading,
//! writing and verification live in `faris-study`; nothing here interprets
//! physics.

use crate::{
    Arguments, FarisApp, Session, SessionInputs, Step,
    badge::Kind,
    load_sweep,
    package::{SavedStore, SavedStudy, StoreOrigin},
    study_panel, sweep_panel, thumbnail,
    transport_panel::FieldView,
};
use clap::ValueEnum;
use eframe::egui;
use faris_engine::evidence_store::EvidenceStore;
use faris_study::{
    ArchiveKind, ArrangementDraft, EvidenceDraft, EvidenceMode, EvidenceState, EvidenceStoreDraft,
    EvidenceStoreTree, MissReason, StoreEvidenceState, StoreSource, StudyDraft, StudyReader,
    ViewState, WriteReport, extract_tar_gz, packed_store_bytes, write_study,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, TryRecvError},
    },
    time::{Duration, Instant},
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

/// Label of the File menu option that packs the Core evidence archives into
/// saved files: the size is the actual total of the open study's archives, and
/// is left out when they are not known.
fn evidence_option_label(evidence: &[EvidenceDraft], trees: &[EvidenceStoreDraft]) -> String {
    let total: u64 =
        evidence.iter().map(|e| e.archive.bytes).sum::<u64>() + packed_store_bytes(trees);
    if (evidence.is_empty() && trees.is_empty()) || total == 0 {
        "Include Core evidence in saved files".into()
    } else {
        format!(
            "Include Core evidence in saved files (about +{})",
            megabytes(total)
        )
    }
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

/// How the evidence-store layer of an opened file reads in the status bar.
pub fn store_evidence_summary(state: &StoreEvidenceState) -> String {
    match state.mode {
        EvidenceMode::Packed => "Core evidence packed in the file".into(),
        EvidenceMode::Referenced if state.missing.is_empty() => {
            let package = state.found.iter().any(|s| s.source == StoreSource::Package);
            let beside = state
                .found
                .iter()
                .any(|s| s.source == StoreSource::BesideFile);
            match (beside, package) {
                (true, true) => {
                    "Core evidence referenced, found beside the file and in the package"
                }
                (false, true) => "Core evidence referenced, found in the package",
                _ => "Core evidence referenced, found beside the file",
            }
            .into()
        }
        EvidenceMode::Referenced => format!(
            "Core evidence referenced, {} of {} saved studies not included",
            state.missing.len(),
            state.missing.len() + state.found.len()
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

/// The Evidence-step badge for a study whose evidence-store trees were saved by
/// reference and were not found (or did not match) beside the file or in the
/// package. Names each tree and why, the next step, and the recorded listing
/// sizes so anyone can check a store they are given later.
pub fn store_evidence_badge(
    state: &StoreEvidenceState,
    trees: &[EvidenceStoreTree],
) -> Option<EvidenceBadge> {
    if state.mode != EvidenceMode::Referenced || state.missing.is_empty() {
        return None;
    }
    let (kind, label) = if state.found.is_empty() {
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
        text.push_str(&format!(
            "{}/{} ({} and {}) was not used: {}.\n",
            miss.arrangement, miss.allocation, miss.case_tree, miss.workspace_tree, miss.reason
        ));
    }
    text.push_str(
        "\nNext step: save or move the .faris file next to the evidence-store folder of the \
         package it came from (the folder that holds store.json), or open it with FARIS \
         started from that package, then reopen the file; or open the original study and \
         save again with \"Include Core evidence in saved files\" checked, which packs the \
         evidence into the file.\n\nRecorded trees:\n",
    );
    for tree in trees {
        let missing = state
            .missing
            .iter()
            .any(|m| m.case_tree == tree.tree || m.workspace_tree == tree.tree);
        text.push_str(&format!(
            "  {} ({}): {} files, {} bytes\n",
            tree.tree,
            if missing {
                "not found"
            } else {
                "found, listing matches"
            },
            tree.files.len(),
            tree.files.iter().map(|f| f.bytes).sum::<u64>()
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

    /// Record the package's saved studies as store trees, so a session started
    /// from a package can save its Core evidence (by reference or packed). The
    /// listings come from the store's index; an unreadable store adds none.
    pub fn with_saved_store(mut self, saved: &SavedStore) -> Self {
        let Ok(store) = EvidenceStore::open_expecting(&saved.path, &saved.index_sha256) else {
            return self;
        };
        for study in &saved.studies {
            let trees = [
                (ArchiveKind::Case, &study.case_tree),
                (ArchiveKind::Workspace, &study.workspace_tree),
            ]
            .map(|(kind, name)| {
                store.tree(name).map(|tree| EvidenceStoreDraft {
                    tree: EvidenceStoreTree {
                        arrangement: study.pair.clone(),
                        allocation: study.variant.clone(),
                        kind,
                        tree: name.clone(),
                        files: tree.files.clone(),
                    },
                    store: Some(saved.path.clone()),
                })
            });
            if let [Some(case), Some(workspace)] = trees {
                self.draft.evidence_store.extend([case, workspace]);
            }
        }
        self
    }
}

/// A study unpacked into the workspace directory, ready to load.
struct OpenedStudy {
    path: PathBuf,
    file_bytes: u64,
    view: ViewState,
    inputs: StudyInputs,
    evidence: EvidenceState,
    /// Evidence recorded as store trees, and the stores to reopen it from.
    evidence_store: Option<StoreEvidenceState>,
    stores: Vec<SavedStore>,
    sweep: Vec<PathBuf>,
    descriptors: Vec<PathBuf>,
    marker: Option<PathBuf>,
    /// History ensembles stored in the file, offered to the history panel;
    /// each is used only where its key equals the key of the loaded inputs.
    ensembles: Vec<faris_study::StoredEnsemble>,
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
    /// Save requested; waiting briefly for the window capture that becomes the
    /// thumbnail. The study is written without one if it does not arrive.
    Capturing {
        path: PathBuf,
        draft: Box<StudyDraft>,
        view: Box<ViewState>,
        viewport: Option<egui::Rect>,
        since: Instant,
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
    /// The evidence store of the package the app was started from, where an
    /// opened file's referenced trees are looked for after the folder beside it.
    pub package_store: Option<PathBuf>,
    /// Keeps an opened study's unpacked files (and extracted evidence) alive.
    workspace: Option<tempfile::TempDir>,
    task: Task,
    dialog: Option<Receiver<DialogResult>>,
    auto_open: Option<PathBuf>,
    last_title: String,
    pub evidence: EvidenceState,
    /// The trees the opened file recorded in its evidence-store layer.
    pub evidence_store: Option<StoreEvidenceState>,
    pub evidence_trees: Vec<EvidenceStoreTree>,
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
            package_store: None,
            workspace: None,
            task: Task::Idle,
            dialog: None,
            auto_open: None,
            last_title: String::new(),
            evidence: EvidenceState::default(),
            evidence_store: None,
            evidence_trees: Vec::new(),
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
        matches!(self.task, Task::Saving { .. } | Task::Capturing { .. })
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
fn open_study(
    path: &Path,
    runs_directory: &Path,
    package_store: Option<&Path>,
    progress: &Mutex<String>,
) -> OpenResult {
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
        .materialize_with(workspace.path(), Some(near), package_store)
        .map_err(|e| e.to_string())?;
    let view = reader.manifest.view.clone();
    // Read through the hash check and parsed strictly: a stored ensemble the
    // reader cannot interpret refuses the file like any other blob.
    let ensembles = reader.ensembles().map_err(|e| e.to_string())?;

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

    // Evidence recorded as store trees is read in place, store by store.
    let mut stores: Vec<SavedStore> = Vec::new();
    if let Some(state) = &files.evidence_store {
        for found in &state.found {
            let study = SavedStudy {
                pair: found.arrangement.clone(),
                variant: found.allocation.clone(),
                case_tree: found.case_tree.clone(),
                workspace_tree: found.workspace_tree.clone(),
            };
            match stores
                .iter_mut()
                .find(|s| s.path == found.store && s.index_sha256 == found.store_json_sha256)
            {
                Some(store) => store.studies.push(study),
                None => stores.push(SavedStore {
                    origin: if found.source == StoreSource::Package {
                        StoreOrigin::Package
                    } else {
                        StoreOrigin::StudyFile
                    },
                    path: found.store.clone(),
                    index_sha256: found.store_json_sha256.clone(),
                    studies: vec![study],
                }),
            }
        }
    }
    let evidence_trees = reader
        .manifest
        .layers
        .evidence_store
        .as_ref()
        .map_or_else(Vec::new, |layer| layer.trees.clone());
    let store_drafts = evidence_trees
        .iter()
        .map(|tree| EvidenceStoreDraft {
            tree: tree.clone(),
            store: files.evidence_store.as_ref().and_then(|state| {
                state
                    .found
                    .iter()
                    .find(|s| s.arrangement == tree.arrangement && s.allocation == tree.allocation)
                    .map(|s| s.store.clone())
            }),
        })
        .collect();

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
        evidence_store: store_drafts,
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
            evidence_store: files.evidence_store,
            stores,
            sweep: files.sweep,
            descriptors,
            marker,
            ensembles,
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
        faris_report::StudyFileStamp::from_path(path)
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
        self.history.restore_ensembles(opened.ensembles);
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
        for store in opened.stores {
            self.study.archive.queue_store(store);
        }
        let summary = match &opened.evidence_store {
            Some(state) => store_evidence_summary(state),
            None => evidence_summary(&opened.evidence),
        };
        // A file opened with its evidence packed saves it packed again unless
        // the box is cleared.
        self.file.include_evidence = opened.evidence.mode == Some(EvidenceMode::Packed)
            || opened
                .evidence_store
                .as_ref()
                .is_some_and(|s| s.mode == EvidenceMode::Packed);
        self.file.evidence_trees = opened
            .inputs
            .draft
            .evidence_store
            .iter()
            .map(|d| d.tree.clone())
            .collect();
        self.file.evidence_store = opened.evidence_store;
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
        let package_store = self.file.package_store.clone();
        let spawned = std::thread::Builder::new()
            .name("faris-open-study".into())
            .spawn(move || {
                let _ = sender.send(open_study(
                    &worker_path,
                    &runs,
                    package_store.as_deref(),
                    &worker_progress,
                ));
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
        draft.pack_evidence = self.file.include_evidence
            && !(draft.evidence.is_empty() && draft.evidence_store.is_empty());
        // Calculated ensembles go in as derived blobs under their input keys.
        draft.ensembles = self.history.ensemble_drafts();
        if self.viewport_rect.is_some() {
            // The thumbnail is the 3D viewport; ask for the window capture and
            // write when it arrives, or without it after a short wait.
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
                thumbnail::SCREENSHOT_TAG,
            )));
            ctx.request_repaint();
            self.message = format!("Saving {}…", open_name(&path));
            self.file.task = Task::Capturing {
                path,
                draft: Box::new(draft),
                view: Box::new(view),
                viewport: self.viewport_rect,
                since: Instant::now(),
            };
            return;
        }
        self.start_write(ctx, path, draft, view, None);
    }

    /// Write the study on a worker thread. `capture` is the window screenshot,
    /// the viewport rectangle and the pixel scale; the thumbnail is made from it
    /// on the worker, and a failure there only means no thumbnail.
    fn start_write(
        &mut self,
        ctx: &egui::Context,
        path: PathBuf,
        mut draft: StudyDraft,
        view: ViewState,
        capture: Option<(Arc<egui::ColorImage>, Option<egui::Rect>, f32)>,
    ) {
        let (sender, receiver) = mpsc::channel();
        let (target, context) = (path.clone(), ctx.clone());
        let spawned = std::thread::Builder::new()
            .name("faris-save-study".into())
            .spawn(move || {
                draft.preview_png = capture
                    .and_then(|(shot, rect, scale)| thumbnail::from_screenshot(&shot, rect, scale));
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
        if matches!(self.file.task, Task::Capturing { .. }) {
            let arrived = ctx.input(|i| {
                i.events.iter().find_map(|event| match event {
                    egui::Event::Screenshot {
                        image, user_data, ..
                    } if thumbnail::is_ours(user_data) => Some(image.clone()),
                    _ => None,
                })
            });
            let timed_out = matches!(
                &self.file.task,
                Task::Capturing { since, .. } if since.elapsed() >= thumbnail::CAPTURE_TIMEOUT
            );
            if (arrived.is_some() || timed_out)
                && let Task::Capturing {
                    path,
                    draft,
                    view,
                    viewport,
                    ..
                } = std::mem::replace(&mut self.file.task, Task::Idle)
            {
                let capture = arrived.map(|image| (image, viewport, ctx.pixels_per_point()));
                self.start_write(ctx, path, *draft, *view, capture);
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
            let evidence_label = evidence_option_label(
                &self.file.inputs.draft.evidence,
                &self.file.inputs.draft.evidence_store,
            );
            ui.checkbox(&mut self.file.include_evidence, evidence_label)
            .on_hover_text(
                "Off: the file records the Core evidence's names, file lists and SHA-256 hashes, and opens fully without it. On: the evidence is stored inside, so the receipts can be re-checked from this one file. Applies when the study has saved Core receipts.",
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
    fn the_evidence_option_shows_the_actual_size_or_none() {
        let archive = |bytes| EvidenceDraft {
            archive: EvidenceArchive {
                arrangement: "port".into(),
                allocation: "reference".into(),
                kind: ArchiveKind::Case,
                file_name: "a.tar.gz".into(),
                sha256: "0".repeat(64),
                bytes,
            },
            path: None,
        };
        assert_eq!(
            evidence_option_label(&[], &[]),
            "Include Core evidence in saved files"
        );
        assert_eq!(
            evidence_option_label(&[archive(12_345_678), archive(2_000_000)], &[]),
            "Include Core evidence in saved files (about +14.3 MB)"
        );
    }

    #[test]
    fn a_file_saved_with_an_old_preset_name_still_restores_that_preset() {
        let mut app = app();
        for (old, id) in [
            ("Baseline authored scenario", "authored-baseline"),
            (
                "Demountable magnets · REBCO fluence limit",
                "demountable-magnets",
            ),
            ("Permanent-trip test (numerical control)", "permanent-trip"),
            ("Loaded assumptions", "loaded"),
        ] {
            app.history
                .restore_view(Some("authored-baseline"), None, "tritium");
            app.apply_view(&ViewState {
                preset: Some(old.into()),
                ..ViewState::default()
            });
            assert_eq!(app.history.selected_preset(), Some(id), "{old}");
        }
    }

    // Verifies: UX-027
    #[test]
    fn title_names_the_file_marks_changes_and_says_unsaved() {
        let path = Path::new("/home/x/ports/study.faris");
        assert_eq!(window_title(Some(path), false), "FARIS — study.faris");
        assert_eq!(window_title(Some(path), true), "FARIS — study.faris •");
        assert_eq!(window_title(None, false), "FARIS — unsaved study");
        // Before a first save there is no file to be different from.
        assert_eq!(window_title(None, true), "FARIS — unsaved study");
    }

    // Verifies: UX-027
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

    // Verifies: VIS-074, CFG-034
    #[test]
    fn the_view_round_trips_through_a_fresh_app() {
        let mut first = app();
        first.step = Step::Compare;
        first.year = 12.25;
        first.variant = first.manifest.variants.len() - 1;
        first.transport.view = FieldView::ComponentFlux;
        let mut what_if = first.history.what_if_values().cloned().unwrap();
        what_if.recovery_fraction = 0.8;
        first
            .history
            .restore_view(Some("authored-baseline"), Some(&what_if), "tritium");
        let saved = first.view_state();
        assert_eq!(saved.step, "compare");
        assert_eq!(saved.field_view, "component-flux");
        assert_eq!(saved.history_tab, "tritium");
        assert_eq!(saved.preset.as_deref(), Some("authored-baseline"));
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

    /// A store with the saved study `port/reference`, and the package-style
    /// record of it.
    fn store_fixture(dir: &Path) -> SavedStore {
        let src = dir.join("src");
        for tree in ["port-reference-case", "port-reference-workspace"] {
            let root = src.join(tree);
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("a.json"), tree.as_bytes()).unwrap();
        }
        let store = dir.join("evidence-store");
        faris_engine::evidence_store::pack_store(
            &store,
            &[
                (
                    "port-reference-case".into(),
                    src.join("port-reference-case"),
                ),
                (
                    "port-reference-workspace".into(),
                    src.join("port-reference-workspace"),
                ),
            ],
        )
        .unwrap();
        let index = std::fs::read(store.join("store.json")).unwrap();
        SavedStore {
            origin: StoreOrigin::Package,
            path: store,
            index_sha256: faris_study::sha256_hex(&index),
            studies: vec![SavedStudy {
                pair: "port".into(),
                variant: "reference".into(),
                case_tree: "port-reference-case".into(),
                workspace_tree: "port-reference-workspace".into(),
            }],
        }
    }

    // Verifies: PRV-040
    #[test]
    fn a_package_session_can_record_its_saved_studies_as_store_trees() {
        let dir = tempfile::tempdir().unwrap();
        let saved = store_fixture(dir.path());
        let inputs = StudyInputs::default().with_saved_store(&saved);
        let trees = &inputs.draft.evidence_store;
        assert_eq!(trees.len(), 2);
        assert_eq!(trees[0].tree.kind, ArchiveKind::Case);
        assert_eq!(trees[1].tree.kind, ArchiveKind::Workspace);
        assert_eq!(trees[0].store.as_deref(), Some(saved.path.as_path()));
        let store = EvidenceStore::open(&saved.path).unwrap();
        assert_eq!(
            trees[0].tree.files,
            store.tree("port-reference-case").unwrap().files
        );
        // The File-menu option names the size packing would add.
        let label = evidence_option_label(&[], trees);
        assert!(label.contains("about +"), "{label}");
        // A store that does not match its recorded digest adds nothing.
        let mut changed = saved.clone();
        changed.index_sha256 = "0".repeat(64);
        assert!(
            StudyInputs::default()
                .with_saved_store(&changed)
                .draft
                .evidence_store
                .is_empty()
        );
    }

    // Verifies: PRV-041, PRV-042, PRV-012
    #[test]
    fn missing_store_trees_explain_why_and_the_next_step() {
        let dir = tempfile::tempdir().unwrap();
        let saved = store_fixture(dir.path());
        let inputs = StudyInputs::default().with_saved_store(&saved);
        let trees: Vec<EvidenceStoreTree> = inputs
            .draft
            .evidence_store
            .iter()
            .map(|d| d.tree.clone())
            .collect();
        let mut state = StoreEvidenceState {
            mode: EvidenceMode::Referenced,
            found: vec![],
            missing: vec![faris_study::StoreMiss {
                arrangement: "port".into(),
                allocation: "reference".into(),
                case_tree: "port-reference-case".into(),
                workspace_tree: "port-reference-workspace".into(),
                reason: "beside the file: no evidence-store folder at x; the app was not started from a package".into(),
            }],
        };
        let note = store_evidence_badge(&state, &trees).unwrap();
        assert_eq!(note.kind, Kind::NotEvaluated);
        assert_eq!(note.label, "Core receipts not included");
        assert!(note.text.contains("Why:") && note.text.contains("by reference, not included"));
        assert!(note.text.contains("port-reference-case"));
        assert!(note.text.contains("not started from a package"));
        assert!(note.text.contains("Next step:") && note.text.contains("evidence-store folder"));
        assert!(note.text.contains("Include Core evidence"));
        assert!(note.text.contains("nothing here claims Core verification"));
        assert!(note.text.contains("(not found)"));
        assert_eq!(
            store_evidence_summary(&state),
            "Core evidence referenced, 1 of 1 saved studies not included"
        );

        // Found beside the file or in the package, or packed: nothing to explain.
        let found = |source| faris_study::StoreStudy {
            arrangement: "port".into(),
            allocation: "reference".into(),
            case_tree: "port-reference-case".into(),
            workspace_tree: "port-reference-workspace".into(),
            store: saved.path.clone(),
            store_json_sha256: saved.index_sha256.clone(),
            source,
        };
        state.missing.clear();
        state.found = vec![found(StoreSource::Package)];
        assert!(store_evidence_badge(&state, &trees).is_none());
        assert_eq!(
            store_evidence_summary(&state),
            "Core evidence referenced, found in the package"
        );
        state.found = vec![found(StoreSource::BesideFile)];
        assert_eq!(
            store_evidence_summary(&state),
            "Core evidence referenced, found beside the file"
        );
        state.mode = EvidenceMode::Packed;
        state.found = vec![found(StoreSource::Embedded)];
        assert!(store_evidence_badge(&state, &trees).is_none());
        assert_eq!(
            store_evidence_summary(&state),
            "Core evidence packed in the file"
        );
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

    // Verifies: PRV-041, PRV-042, PRV-012
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
    /// with FARIS_DEMO_DIR and FARIS_SWEEP_DIR set and `--release -- --ignored
    /// --nocapture` (checking seven sweep bundles
    /// plus four arrangements is slow in debug builds).
    #[test]
    #[ignore = "needs the packaged demo inputs and the sweep bundles on disk"]
    fn the_real_demo_study_saves_opens_and_restores_its_view() {
        let (Some(demo), Some(sweep_dir)) = (
            std::env::var_os("FARIS_DEMO_DIR").map(PathBuf::from),
            std::env::var_os("FARIS_SWEEP_DIR").map(PathBuf::from),
        ) else {
            eprintln!(
                "skipped: set FARIS_DEMO_DIR (packaged demo) and FARIS_SWEEP_DIR (sweep bundles)"
            );
            return;
        };
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
        let mut trees = Vec::new();
        for arrangement in ["port", "control"] {
            for allocation in ["reference", "breeder-emphasis"] {
                let found = faris_study::descriptor_evidence(
                    &demo.join(format!("saved-study-{arrangement}-{allocation}.json")),
                )
                .unwrap();
                let faris_study::DescriptorEvidence::Store(found) = found else {
                    panic!("the packaged demo names evidence-store trees");
                };
                trees.extend(found);
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
            evidence_store: trees,
            view: view.clone(),
            ..StudyDraft::default()
        };
        let runs = dir.path().join("runs");
        let progress = Mutex::new(String::new());
        let reopen = |opened: &OpenedStudy| -> Vec<String> {
            opened
                .stores
                .iter()
                .flat_map(crate::archive_panel::check_saved_studies_in_store)
                .map(|r| r.unwrap())
                .collect()
        };
        // Referenced and not beside the file, no package: opens, evidence not included.
        let referenced = dir.path().join("referenced.faris");
        write_study(&referenced, &draft).unwrap();
        let started = std::time::Instant::now();
        let (session, opened) = open_study(&referenced, &runs, None, &progress).unwrap();
        let seconds = started.elapsed().as_secs_f64();
        assert!(session.control.is_some());
        assert_eq!(opened.sweep.len(), 7);
        let state = opened.evidence_store.clone().unwrap();
        assert_eq!(state.missing.len(), 4);
        assert!(opened.stores.is_empty() && opened.marker.is_none());
        assert!(
            store_evidence_badge(
                &state,
                &opened
                    .inputs
                    .draft
                    .evidence_store
                    .iter()
                    .map(|d| d.tree.clone())
                    .collect::<Vec<_>>()
            )
            .is_some()
        );
        assert_eq!(opened.view, view);
        println!("referenced study opened in {seconds:.1} s");

        // Referenced and launched from the package: its store is used in place.
        let (_, opened) = open_study(
            &referenced,
            &runs,
            Some(&demo.join("evidence-store")),
            &progress,
        )
        .unwrap();
        assert!(opened.evidence_store.as_ref().unwrap().missing.is_empty());
        let started = std::time::Instant::now();
        let cases = reopen(&opened);
        assert_eq!(cases.len(), 4);
        println!(
            "package store reopened in {:.1} s: {cases:?}",
            started.elapsed().as_secs_f64()
        );

        // Packed: the store comes out of the file and is read in place.
        draft.pack_evidence = true;
        let packed = dir.path().join("packed.faris");
        write_study(&packed, &draft).unwrap();
        let (_, opened) = open_study(&packed, &runs, None, &progress).unwrap();
        assert_eq!(opened.evidence_store.as_ref().unwrap().found.len(), 4);
        assert!(opened.descriptors.is_empty() && opened.marker.is_none());
        let started = std::time::Instant::now();
        let from_file = reopen(&opened);
        println!(
            "packed store reopened in {:.1} s",
            started.elapsed().as_secs_f64()
        );
        assert_eq!(from_file, cases);
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
