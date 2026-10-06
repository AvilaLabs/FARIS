//! Package mode: opening the study a release package carries, with no launcher
//! script. The package index binds every file the app reads at launch; this
//! module checks that binding before anything is loaded, builds the inputs the
//! command-line flags would have passed, and (when the evidence part is
//! present) extracts the recorded Core trees on a background thread.
//!
//! Full verification of every indexed file stays with `verify.sh`; the checks
//! here are the launch-relevant part of it. Nothing here interprets physics.

use faris_study::{RecordedTreeManifest, StudyError, extract_recorded_tree, sha256_file};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Instant,
};

pub const INDEX: &str = "package-index.json";
pub const CHECKSUM: &str = "package-index.sha256";
const SCHEMA: &str = "faris-recorded-demo-package/v0.5";
const STATUS: &str = "IDENTITIES_REVALIDATED_CORE_EXECUTIONS_COMPLETED_PHYSICS_NOT_EVALUATED";
const MAX_INDEX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FILES: usize = 2048;
const MAX_EXPANDED_BYTES: u64 = 1536 * 1024 * 1024;
const MAX_EXPANDED_MEMBERS: u64 = 8192;
const MAX_EXPANDED_DIRECTORIES: u64 = 8192;
const SWEEP_BUNDLE_DIRECTORY: &str = "sweep/bundles";
const MARKER_SCHEMA: &str = "faris-recorded-materialization/v0.1";
/// The four saved studies the Evidence step reopens, in the order shown.
const SAVED_STUDIES: [(&str, &str); 4] = [
    ("port", "reference"),
    ("port", "breeder-emphasis"),
    ("control", "reference"),
    ("control", "breeder-emphasis"),
];

/// What a failed check tells the user to do about it.
const NEXT_STEP: &str = "Next step: download the package again and check it against the \
    SHA256SUMS file from the same release; if it still fails, report the message above.";

/// This build's platform, as the package index names it.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Platform {
    pub os: String,
    pub arch: String,
}

impl Platform {
    pub fn current() -> Self {
        Self {
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
        }
    }
}

#[derive(Deserialize)]
struct Executable {
    path: String,
}

#[derive(Deserialize)]
struct LocalRuntime {
    platform: Platform,
    executables: BTreeMap<String, Executable>,
}

#[derive(Deserialize)]
struct FileEntry {
    path: String,
    bytes: u64,
    sha256: String,
    #[serde(default)]
    part: Option<String>,
}

#[derive(Deserialize)]
struct EvidencePart {
    archive_name: String,
}

#[derive(Deserialize)]
struct Parts {
    evidence: EvidencePart,
}

#[derive(Clone, Deserialize)]
struct TreeArchive {
    path: String,
    sha256: String,
    bytes: u64,
    manifest_path: String,
    manifest_sha256: String,
    expanded_bytes: u64,
    file_count: u64,
    archive_member_count: u64,
    directory_count: u64,
}

#[derive(Deserialize)]
struct Arrangement {
    variant_id: String,
    case_archive: TreeArchive,
    workspace_archive: TreeArchive,
}

#[derive(Deserialize)]
struct ScenarioPair {
    scenario_path: String,
    arrangements: Vec<Arrangement>,
}

#[derive(Deserialize)]
struct SweepRun {
    transport_bundle: String,
    transport_bundle_sha256: String,
}

#[derive(Deserialize)]
struct Sweep {
    runs: Vec<SweepRun>,
}

#[derive(Deserialize)]
struct Index {
    schema_version: String,
    status: String,
    faris_app_sha256: String,
    local_runtime: LocalRuntime,
    files: Vec<FileEntry>,
    parts: Parts,
    expanded_case_workspace_bytes: u64,
    scenario_pairs: Vec<ScenarioPair>,
    #[serde(default)]
    sweep: Option<Sweep>,
}

/// One recorded Core tree to extract: where its files are and what the index
/// says they hold.
#[derive(Clone)]
pub struct TreeJob {
    pair: String,
    variant: String,
    kind: &'static str,
    expected: TreeArchive,
}

/// Whether the evidence part (the Core receipts) is in the package folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Evidence {
    Present,
    Absent {
        archive_name: String,
        total: usize,
    },
    Incomplete {
        archive_name: String,
        missing: usize,
        total: usize,
    },
}

/// A package that passed its checks, ready to load.
pub struct Opened {
    pub root: PathBuf,
    pub core: PathBuf,
    pub bundles: Vec<PathBuf>,
    pub control_scenario: PathBuf,
    pub control_bundles: Vec<PathBuf>,
    pub assumptions: PathBuf,
    pub sweep_bundles: Vec<PathBuf>,
    pub runs_directory: PathBuf,
    pub evidence: Evidence,
    /// `--package` was given and the running app is not the one the index pins.
    pub development_binary: bool,
    pub expanded_bytes: u64,
    trees: Vec<TreeJob>,
}

/// Failure text: why it failed, then what to do.
fn fail(why: impl std::fmt::Display) -> String {
    format!("Why: {why}.\n\n{NEXT_STEP}")
}

/// The folder of a package that sits beside this executable's `bin/` folder
/// (or, on macOS, in the bundle's `Resources`).
pub fn discover(exe: &Path) -> Option<PathBuf> {
    let beside_bin = exe.parent()?.parent()?;
    let mut candidates = vec![beside_bin.to_path_buf()];
    if cfg!(target_os = "macos") {
        candidates.push(beside_bin.join("Resources").join("package"));
    }
    candidates.into_iter().find(|dir| dir.join(INDEX).is_file())
}

/// Where generated runs go when `--runs-directory` is not given: outside the
/// package, in the platform's per-user state folder.
pub fn default_runs_directory(
    os: &str,
    package_name: &str,
    env: &dyn Fn(&str) -> Option<PathBuf>,
) -> Result<PathBuf, String> {
    let absolute = |name: &str| env(name).filter(|p| p.is_absolute());
    let base = match os {
        "windows" => absolute("LOCALAPPDATA")
            .map(|p| p.join("FARIS"))
            .or_else(|| {
                absolute("USERPROFILE").map(|p| p.join("AppData").join("Local").join("FARIS"))
            }),
        "macos" => absolute("HOME").map(|p| p.join("Library/Application Support/FARIS")),
        _ => absolute("XDG_STATE_HOME")
            .map(|p| p.join("faris"))
            .or_else(|| absolute("HOME").map(|p| p.join(".local/state/faris"))),
    };
    base.map(|p| p.join("recorded-demo-runs").join(package_name))
        .ok_or_else(|| {
            "no per-user state folder could be found for generated runs; pass --runs-directory \
             with a folder outside the package"
                .into()
        })
}

/// `dir` made absolute and with any symlinks in its existing part resolved.
fn resolve_lenient(dir: &Path) -> std::io::Result<PathBuf> {
    let absolute = std::path::absolute(dir)?;
    let mut tail = Vec::new();
    let mut cursor = absolute.as_path();
    loop {
        match cursor.canonicalize() {
            Ok(mut resolved) => {
                resolved.extend(tail.iter().rev());
                return Ok(resolved);
            }
            Err(_) => match (cursor.file_name(), cursor.parent()) {
                (Some(name), Some(parent)) => {
                    tail.push(name.to_owned());
                    cursor = parent;
                }
                _ => return Ok(absolute),
            },
        }
    }
}

/// A runs directory that must lie outside the package folder.
pub fn runs_directory_outside(root: &Path, dir: &Path) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("cannot resolve the package folder: {e}"))?;
    let dir = resolve_lenient(dir)
        .map_err(|e| format!("cannot resolve the runs directory {}: {e}", dir.display()))?;
    if dir.starts_with(&root) {
        return Err(
            "--runs-directory must resolve outside the package folder, which is read-only".into(),
        );
    }
    Ok(dir)
}

/// A package-relative path made of plain components with no symlink on the way,
/// naming a regular file.
fn safe_file(root: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.is_empty()
        || relative.contains(['\\', ':', '\0'])
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(format!("unsafe package path {relative:?}"));
    }
    let mut cursor = root.to_path_buf();
    for part in relative.split('/') {
        cursor.push(part);
        let metadata = std::fs::symlink_metadata(&cursor)
            .map_err(|e| format!("{relative} cannot be read: {e}"))?;
        if metadata.file_type().is_symlink() {
            return Err(format!("{relative} is a symlink"));
        }
    }
    if !std::fs::symlink_metadata(&cursor).is_ok_and(|m| m.is_file()) {
        return Err(format!("{relative} is not a regular file"));
    }
    Ok(cursor)
}

fn sha256_text(path: &Path) -> Result<(String, u64), String> {
    sha256_file(path)
        .map(|(hex, size)| (format!("sha256:{hex}"), size))
        .map_err(|e| format!("{} cannot be read: {e}", path.display()))
}

fn read_index(root: &Path) -> Result<Index, String> {
    let index_path = safe_file(root, INDEX).map_err(fail)?;
    let checksum_path = safe_file(root, CHECKSUM).map_err(fail)?;
    let size = |path: &Path| std::fs::metadata(path).map(|m| m.len()).unwrap_or(u64::MAX);
    if size(&index_path) > MAX_INDEX_BYTES || size(&checksum_path) > 256 {
        return Err(fail(
            "the package index or its checksum is larger than allowed",
        ));
    }
    let bytes =
        std::fs::read(&index_path).map_err(|e| fail(format!("{INDEX} cannot be read: {e}")))?;
    let expected = format!("sha256:{:x}  {INDEX}\n", Sha256::digest(&bytes));
    let recorded = std::fs::read(&checksum_path)
        .map_err(|e| fail(format!("{CHECKSUM} cannot be read: {e}")))?;
    if recorded != expected.as_bytes() {
        return Err(fail(format!("{CHECKSUM} does not match {INDEX}")));
    }
    serde_json::from_slice(&bytes).map_err(|e| {
        fail(format!(
            "{INDEX} is not a valid package index for this FARIS ({e})"
        ))
    })
}

/// Check the package at `root` against its index and build what loading needs.
///
/// `exe` is the running executable and `explicit` whether the package was named
/// with `--package` (a differing executable is then labelled, not refused).
/// `runs_override` is `--runs-directory`, if given.
pub fn open(
    root: &Path,
    exe: &Path,
    explicit: bool,
    runs_override: Option<&Path>,
) -> Result<Opened, String> {
    open_for(root, exe, explicit, runs_override, &Platform::current())
}

fn open_for(
    root: &Path,
    exe: &Path,
    explicit: bool,
    runs_override: Option<&Path>,
    platform: &Platform,
) -> Result<Opened, String> {
    let root = root.canonicalize().map_err(|e| {
        fail(format!(
            "the package folder {} cannot be read: {e}",
            root.display()
        ))
    })?;
    let index = read_index(&root)?;
    if index.schema_version != SCHEMA {
        return Err(fail(format!(
            "the package index is schema {:?}, and this FARIS opens {SCHEMA:?}",
            index.schema_version
        )));
    }
    if index.status != STATUS {
        return Err(fail(
            "the package status does not preserve the required NOT_EVALUATED scope",
        ));
    }
    if &index.local_runtime.platform != platform {
        return Err(fail(format!(
            "this package was built for {}/{}, and this FARIS runs on {}/{}",
            index.local_runtime.platform.os,
            index.local_runtime.platform.arch,
            platform.os,
            platform.arch
        )));
    }
    let (running, _) = sha256_text(exe).map_err(fail)?;
    let development_binary = running != index.faris_app_sha256;
    if development_binary && !explicit {
        return Err(fail(
            "the running FARIS application is not the one this package's index pins",
        ));
    }
    if development_binary {
        eprintln!(
            "FARIS: development app binary: not covered by the package index ({})",
            exe.display()
        );
    }

    if index.files.is_empty() || index.files.len() > MAX_FILES {
        return Err(fail(
            "the package index has an empty or excessive file list",
        ));
    }
    let mut app: BTreeMap<&str, &FileEntry> = BTreeMap::new();
    let mut evidence_files: Vec<&FileEntry> = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in &index.files {
        if !seen.insert(entry.path.as_str()) {
            return Err(fail(format!("the package index repeats {}", entry.path)));
        }
        match entry.part.as_deref() {
            Some("app") => {
                app.insert(&entry.path, entry);
            }
            Some("evidence") => evidence_files.push(entry),
            _ => {
                return Err(fail(format!(
                    "the package index does not say which part {} belongs to",
                    entry.path
                )));
            }
        }
    }
    for entry in app.values() {
        let path = safe_file(&root, &entry.path).map_err(fail)?;
        let (digest, size) = sha256_text(&path).map_err(fail)?;
        if size > MAX_FILE_BYTES || size != entry.bytes || digest != entry.sha256 {
            return Err(fail(format!(
                "{} differs from the package index in size or SHA-256",
                entry.path
            )));
        }
    }

    // Everything the app opens must be an app-part file the loop above checked.
    let in_app = |relative: &str| -> Result<PathBuf, String> {
        if !app.contains_key(relative) {
            return Err(fail(format!(
                "{relative} is needed to open the study but is not in the package's app part"
            )));
        }
        safe_file(&root, relative).map_err(fail)
    };
    let core_relative = index
        .local_runtime
        .executables
        .get("avila-core")
        .map(|e| e.path.as_str())
        .ok_or_else(|| fail("the package index names no Avila Core executable"))?;
    let core = in_app(core_relative)?;
    let bundles = ["reference", "breeder-emphasis"]
        .map(|variant| in_app(&format!("port/bundles/{variant}.transport-bundle.json")));
    let control_bundles = ["reference", "breeder-emphasis"]
        .map(|variant| in_app(&format!("control/bundles/{variant}.transport-bundle.json")));
    let control_scenario = in_app("control/scenario.json")?;
    let assumptions = in_app("operating-assumptions.json")?;
    let mut sweep_bundles = Vec::new();
    if let Some(sweep) = &index.sweep {
        if sweep.runs.is_empty() {
            return Err(fail("the package's allocation sweep lists no runs"));
        }
        for run in &sweep.runs {
            let parent = run.transport_bundle.rsplit_once('/').map(|(p, _)| p);
            if parent != Some(SWEEP_BUNDLE_DIRECTORY) {
                return Err(fail(format!(
                    "sweep bundle {} is outside {SWEEP_BUNDLE_DIRECTORY}",
                    run.transport_bundle
                )));
            }
            let path = in_app(&run.transport_bundle)?;
            if app[run.transport_bundle.as_str()].sha256 != run.transport_bundle_sha256 {
                return Err(fail(format!(
                    "sweep bundle {} differs from the sweep's recorded SHA-256",
                    run.transport_bundle
                )));
            }
            sweep_bundles.push(path);
        }
    }
    in_app("bin/faris-app")?;

    let trees = tree_jobs(&index)?;
    let evidence = {
        let total = evidence_files.len();
        let missing = evidence_files
            .iter()
            .filter(|e| safe_file(&root, &e.path).is_err())
            .count();
        let archive_name = index.parts.evidence.archive_name.clone();
        if missing == 0 {
            Evidence::Present
        } else if missing == total {
            Evidence::Absent {
                archive_name,
                total,
            }
        } else {
            Evidence::Incomplete {
                archive_name,
                missing,
                total,
            }
        }
    };
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "package".into());
    let runs_directory = match runs_override {
        Some(dir) => runs_directory_outside(&root, dir)?,
        None => {
            let found = default_runs_directory(&platform.os, &name, &|key| {
                std::env::var_os(key).map(PathBuf::from)
            })?;
            runs_directory_outside(&root, &found)?
        }
    };
    let [bundle_reference, bundle_breeder] = bundles;
    let [control_reference, control_breeder] = control_bundles;
    Ok(Opened {
        core,
        bundles: vec![bundle_reference?, bundle_breeder?],
        control_scenario,
        control_bundles: vec![control_reference?, control_breeder?],
        assumptions,
        sweep_bundles,
        runs_directory,
        evidence,
        development_binary,
        expanded_bytes: index.expanded_case_workspace_bytes,
        trees,
        root,
    })
}

/// The eight recorded trees and what the index says each holds. Their
/// files are checked and read during extraction.
fn tree_jobs(index: &Index) -> Result<Vec<TreeJob>, String> {
    let mut jobs = Vec::new();
    for (pair_id, variant) in SAVED_STUDIES {
        let arrangement = index
            .scenario_pairs
            .iter()
            .filter(|pair| pair.scenario_path.split('/').next() == Some(pair_id))
            .flat_map(|pair| &pair.arrangements)
            .find(|a| a.variant_id == variant)
            .ok_or_else(|| {
                fail(format!(
                    "the package index has no saved case {pair_id}/{variant}"
                ))
            })?;
        for (kind, tree) in [
            ("case", &arrangement.case_archive),
            ("workspace", &arrangement.workspace_archive),
        ] {
            jobs.push(TreeJob {
                pair: pair_id.into(),
                variant: variant.into(),
                kind,
                expected: tree.clone(),
            });
        }
    }
    Ok(jobs)
}

impl Opened {
    /// Start extracting the evidence trees, if the evidence part is present.
    pub fn start_materializer(&self) -> Result<Option<Materializer>, String> {
        if self.evidence != Evidence::Present {
            return Ok(None);
        }
        Materializer::start(&self.root, self.trees.clone(), self.expanded_bytes).map(Some)
    }
}

/// The background extraction of the recorded Core trees into a private
/// temporary folder that lives as long as this value. Dropping it cancels the
/// thread, waits for it, and removes the folder.
pub struct Materializer {
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    pub descriptors: Vec<PathBuf>,
    pub marker: PathBuf,
    // Dropped after `drop` has joined the thread.
    _folder: tempfile::TempDir,
}

impl Materializer {
    fn start(root: &Path, trees: Vec<TreeJob>, expanded_bytes: u64) -> Result<Self, String> {
        let folder = tempfile::Builder::new()
            .prefix("faris-recorded-demo-")
            .tempdir()
            .map_err(|e| format!("cannot create a private temporary folder: {e}"))?;
        let base = folder.path().to_path_buf();
        let mut descriptors = Vec::new();
        for (pair, variant) in SAVED_STUDIES {
            let case = format!("materialized/{pair}/cases/{variant}");
            let workspace = format!("materialized/{pair}/core-workspaces/{variant}");
            let text = serde_json::to_string_pretty(&serde_json::json!({
                "case_directory": case,
                "execution_report": format!("{case}/execution-report.json"),
                "execution_workspace": workspace,
            }))
            .map_err(|e| e.to_string())?;
            let path = base.join(format!("saved-study-{pair}-{variant}.json"));
            std::fs::write(&path, text + "\n")
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            descriptors.push(path);
        }
        let marker = base.join("materialization.json");
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = {
            let (cancel, base, root) = (cancel.clone(), base.clone(), root.to_path_buf());
            move || {
                let started = Instant::now();
                let outcome = extract_all(&root, &base, &trees, &cancel);
                let status = match outcome {
                    Ok(()) => {
                        eprintln!(
                            "FARIS: materialized {expanded_bytes} bytes of Core evidence in {:.3} s; \
                             the private temporary copy remains until the app exits.",
                            started.elapsed().as_secs_f64()
                        );
                        Ok(())
                    }
                    Err(error @ StudyError::Io { .. }) => Err(format!(
                        "{error}; extracting the saved Core evidence needs about {} MB of free temporary space",
                        expanded_bytes / 1_000_000 + 1
                    )),
                    Err(error) => Err(error.to_string()),
                };
                if let Err(error) = publish_marker(&base, &status) {
                    eprintln!("FARIS: cannot publish the materialization marker: {error}");
                }
            }
        };
        let handle = std::thread::Builder::new()
            .name("faris-materialization".into())
            .spawn(worker)
            .map_err(|e| format!("cannot start the evidence extraction thread: {e}"))?;
        Ok(Self {
            cancel,
            handle: Some(handle),
            descriptors,
            marker,
            _folder: folder,
        })
    }
}

impl Drop for Materializer {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn extract_all(
    root: &Path,
    base: &Path,
    trees: &[TreeJob],
    cancel: &AtomicBool,
) -> Result<(), StudyError> {
    let corrupt = |message: String| StudyError::Corrupt(message);
    let (mut bytes, mut members, mut directories) = (0u64, 0u64, 0u64);
    for job in trees {
        let label = format!("{}/{} {}", job.pair, job.variant, job.kind);
        let archive = safe_file(root, &job.expected.path).map_err(&corrupt)?;
        let manifest = safe_file(root, &job.expected.manifest_path).map_err(&corrupt)?;
        let (manifest_digest, _) = sha256_text(&manifest).map_err(&corrupt)?;
        if manifest_digest != job.expected.manifest_sha256 {
            return Err(corrupt(format!(
                "{label} archive manifest differs from the package index"
            )));
        }
        let destination = base
            .join("materialized")
            .join(&job.pair)
            .join(match job.kind {
                "case" => "cases",
                _ => "core-workspaces",
            });
        let tree =
            extract_recorded_tree(&archive, &manifest, &destination.join(&job.variant), cancel)
                .map_err(|e| match e {
                    StudyError::Corrupt(message) => corrupt(format!("{label}: {message}")),
                    other => other,
                })?;
        if !matches_index(&tree, &job.expected) {
            return Err(corrupt(format!(
                "{label} archive totals differ from the package index"
            )));
        }
        bytes += tree.expanded_bytes;
        members += tree.archive_member_count;
        directories += tree.directory_count;
        if bytes > MAX_EXPANDED_BYTES
            || members > MAX_EXPANDED_MEMBERS
            || directories > MAX_EXPANDED_DIRECTORIES
        {
            return Err(corrupt(
                "combined case and workspace expansion exceeds the package limits".into(),
            ));
        }
    }
    Ok(())
}

fn matches_index(tree: &RecordedTreeManifest, index: &TreeArchive) -> bool {
    tree.archive_sha256 == index.sha256
        && tree.archive_bytes == index.bytes
        && tree.expanded_bytes == index.expanded_bytes
        && tree.file_count == index.file_count
        && tree.archive_member_count == index.archive_member_count
        && tree.directory_count == index.directory_count
}

/// Write the completion marker the saved-study worker waits for: staged
/// beside it, synced, then renamed into place.
fn publish_marker(base: &Path, outcome: &Result<(), String>) -> std::io::Result<()> {
    let mut value = serde_json::json!({ "schema_version": MARKER_SCHEMA });
    match outcome {
        Ok(()) => value["status"] = "COMPLETE".into(),
        Err(error) => {
            let mut end = error.len().min(1024);
            while !error.is_char_boundary(end) {
                end -= 1;
            }
            value["status"] = "FAILED".into();
            value["error"] = if end == 0 {
                "extraction failed".into()
            } else {
                error[..end].into()
            };
        }
    }
    let staging = base.join(".materialization.json.tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)?;
    file.write_all((value.to_string() + "\n").as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&staging, base.join("materialization.json"))
}

/// The Evidence-step message for a package without all its Core receipts, or
/// `None` when they are present.
pub fn evidence_message(evidence: &Evidence) -> Option<crate::study_file::EvidenceBadge> {
    let (archive_name, scope) = match evidence {
        Evidence::Present => return None,
        Evidence::Absent { archive_name, .. } => (
            archive_name,
            "this download does not include the evidence part".to_owned(),
        ),
        Evidence::Incomplete {
            archive_name,
            missing,
            total,
        } => (
            archive_name,
            format!(
                "{missing} of the {total} evidence files are missing from this folder, so the evidence part is incomplete"
            ),
        ),
    };
    let text = format!(
        "Why: {scope}. The transport, histories and assumptions are complete and shown, but no \
         Core receipts were re-checked, so nothing here claims Core verification.\n\n\
         Next step: download {archive_name} from the same release, unpack it into the package \
         folder, and reopen FARIS."
    );
    Some(crate::study_file::EvidenceBadge {
        kind: crate::badge::Kind::NotEvaluated,
        label: "Core receipts not included",
        text,
    })
}

/// The message shown when the package checks fail and nothing was loaded.
pub fn failure_message(text: &str) -> crate::study_file::EvidenceBadge {
    crate::study_file::EvidenceBadge {
        kind: crate::badge::Kind::Failed,
        label: "package check failed",
        text: format!("The study in this package was not opened. {text}"),
    }
}

/// The label for a package opened with a development app binary.
pub fn development_binary_message() -> crate::study_file::EvidenceBadge {
    crate::study_file::EvidenceBadge {
        kind: crate::badge::Kind::Partial,
        label: "development app binary",
        text: "Why: this app is not the executable the package index pins, so the index does not \
               cover it; the package files themselves were checked.\n\nNext step: use the \
               bin/faris-app that came in the package to run the released build."
            .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use serde_json::{Value, json};

    fn sha(bytes: &[u8]) -> String {
        format!("sha256:{}", faris_study::sha256_hex(bytes))
    }

    /// A one-file USTAR archive, gzipped, plus its tree manifest.
    fn tree(file: &str, data: &[u8]) -> (Vec<u8>, Value) {
        let mut block = [0u8; 512];
        block[..file.len()].copy_from_slice(file.as_bytes());
        block[100..107].copy_from_slice(b"0000444");
        block[124..135].copy_from_slice(format!("{:011o}", data.len()).as_bytes());
        block[148..156].copy_from_slice(b"        ");
        block[156] = b'0';
        block[257..263].copy_from_slice(b"ustar\0");
        block[263..265].copy_from_slice(b"00");
        let sum: u64 = block.iter().map(|b| u64::from(*b)).sum();
        block[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
        let mut tar = block.to_vec();
        tar.extend_from_slice(data);
        tar.resize(tar.len().div_ceil(512) * 512 + 1024, 0);
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&tar).unwrap();
        let gz = encoder.finish().unwrap();
        let manifest = json!({
            "schema_version": "faris-recorded-tree-archive/v0.1",
            "archive_sha256": sha(&gz),
            "archive_bytes": gz.len(),
            "expanded_bytes": data.len(),
            "file_count": 1,
            "archive_member_count": 1,
            "directory_count": 0,
            "members": [{"path": file, "bytes": data.len(), "sha256": sha(data)}],
        });
        (gz, manifest)
    }

    /// A v0.5 package in a temporary folder: app part, evidence part, index.
    struct Fixture {
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn root(&self) -> PathBuf {
            self.dir.path().join("FARIS-test")
        }

        fn exe(&self) -> PathBuf {
            self.root().join("bin/faris-app")
        }

        fn new() -> Self {
            Self::with(|_| {})
        }

        fn with(edit: impl FnOnce(&mut Value)) -> Self {
            let fixture = Self {
                dir: tempfile::tempdir().unwrap(),
            };
            let root = fixture.root();
            let mut files: Vec<Value> = Vec::new();
            let mut put = |relative: &str, bytes: &[u8], part: &str| {
                let path = root.join(relative);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, bytes).unwrap();
                files.push(json!({
                    "path": relative, "bytes": bytes.len(), "sha256": sha(bytes), "part": part,
                }));
            };
            put("bin/faris-app", b"the app binary", "app");
            put("bin/avila-core", b"the core binary", "app");
            put("README.md", b"readme", "app");
            for variant in ["reference", "breeder-emphasis"] {
                put(
                    &format!("port/bundles/{variant}.transport-bundle.json"),
                    format!("port {variant}").as_bytes(),
                    "app",
                );
                put(
                    &format!("control/bundles/{variant}.transport-bundle.json"),
                    format!("control {variant}").as_bytes(),
                    "app",
                );
            }
            put("control/scenario.json", b"scenario", "app");
            put("operating-assumptions.json", b"assumptions", "app");
            put("sweep/bundles/a.transport-bundle.json", b"sweep a", "app");
            put("verify.sh", b"#!/bin/sh\n", "evidence");
            let mut pairs = Vec::new();
            let mut expanded = 0u64;
            for pair in ["port", "control"] {
                let mut arrangements = Vec::new();
                for variant in ["reference", "breeder-emphasis"] {
                    let mut descriptor = |kind: &str, file: &str| {
                        let data = format!("{pair} {variant} {kind}").into_bytes();
                        let (gz, manifest) = tree(file, &data);
                        let archive = format!("{pair}/archives/{variant}-{kind}.tar.gz");
                        let manifest_path =
                            format!("{pair}/archives/{variant}-{kind}.manifest.json");
                        let manifest_bytes = manifest.to_string().into_bytes();
                        put(&archive, &gz, "evidence");
                        put(&manifest_path, &manifest_bytes, "evidence");
                        expanded += data.len() as u64;
                        json!({
                            "path": archive, "sha256": sha(&gz), "bytes": gz.len(),
                            "manifest_path": manifest_path, "manifest_sha256": sha(&manifest_bytes),
                            "expanded_bytes": data.len(), "file_count": 1,
                            "archive_member_count": 1, "directory_count": 0,
                        })
                    };
                    arrangements.push(json!({
                        "variant_id": variant,
                        "case_archive": descriptor("case", "execution-report.json"),
                        "workspace_archive": descriptor("workspace", "workspace.json"),
                    }));
                }
                pairs.push(json!({
                    "scenario_path": format!("{pair}/scenario.json"),
                    "arrangements": arrangements,
                }));
            }
            let platform = Platform::current();
            let mut index = json!({
                "schema_version": SCHEMA,
                "status": STATUS,
                "faris_app_sha256": sha(b"the app binary"),
                "local_runtime": {
                    "platform": {"os": platform.os, "arch": platform.arch},
                    "executables": {
                        "faris-app": {"path": "bin/faris-app"},
                        "avila-core": {"path": "bin/avila-core"},
                    },
                    "launcher": {"kind": "native", "executable": "faris-app"},
                },
                "files": files,
                "parts": {
                    "app": {"file_count": 0, "bytes": 0},
                    "evidence": {"file_count": 0, "bytes": 0, "archive_name": "FARIS-test-evidence.tar.gz"},
                },
                "expanded_case_workspace_bytes": expanded,
                "scenario_pairs": pairs,
                "sweep": {"runs": [{
                    "transport_bundle": "sweep/bundles/a.transport-bundle.json",
                    "transport_bundle_sha256": sha(b"sweep a"),
                }]},
            });
            edit(&mut index);
            fixture.write_index(&index);
            fixture
        }

        fn write_index(&self, index: &Value) {
            let bytes = index.to_string().into_bytes();
            std::fs::write(self.root().join(INDEX), &bytes).unwrap();
            std::fs::write(
                self.root().join(CHECKSUM),
                format!("{}  {INDEX}\n", sha(&bytes)),
            )
            .unwrap();
        }

        fn open(&self) -> Result<Opened, String> {
            open(
                &self.root(),
                &self.exe(),
                true,
                Some(&self.dir.path().join("runs")),
            )
        }

        fn evidence_files(&self) -> Vec<PathBuf> {
            let mut found = Vec::new();
            for dir in ["port/archives", "control/archives"] {
                for entry in std::fs::read_dir(self.root().join(dir)).unwrap() {
                    found.push(entry.unwrap().path());
                }
            }
            found.push(self.root().join("verify.sh"));
            found
        }
    }

    fn wait_for_marker(materializer: &Materializer) -> Value {
        for _ in 0..6000 {
            if let Ok(bytes) = std::fs::read(&materializer.marker) {
                return serde_json::from_slice(&bytes).unwrap();
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("no materialization marker");
    }

    #[test]
    fn opens_a_v05_package_and_builds_the_inputs() {
        let fixture = Fixture::new();
        let opened = fixture.open().unwrap();
        let root = fixture.root().canonicalize().unwrap();
        assert_eq!(opened.core, root.join("bin/avila-core"));
        assert_eq!(
            opened.bundles,
            vec![
                root.join("port/bundles/reference.transport-bundle.json"),
                root.join("port/bundles/breeder-emphasis.transport-bundle.json"),
            ]
        );
        assert_eq!(opened.control_bundles.len(), 2);
        assert_eq!(opened.control_scenario, root.join("control/scenario.json"));
        assert_eq!(opened.assumptions, root.join("operating-assumptions.json"));
        assert_eq!(
            opened.sweep_bundles,
            vec![root.join("sweep/bundles/a.transport-bundle.json")]
        );
        assert_eq!(opened.evidence, Evidence::Present);
        assert!(!opened.development_binary);
        assert!(!opened.runs_directory.starts_with(&root));
        assert_eq!(opened.trees.len(), 8);
    }

    #[test]
    fn extracts_the_evidence_trees_and_removes_them_on_exit() {
        let fixture = Fixture::new();
        let opened = fixture.open().unwrap();
        let materializer = opened.start_materializer().unwrap().unwrap();
        assert_eq!(materializer.descriptors.len(), 4);
        let marker = wait_for_marker(&materializer);
        assert_eq!(marker["status"], "COMPLETE", "{marker}");
        assert_eq!(marker["schema_version"], MARKER_SCHEMA);
        let descriptor = &materializer.descriptors[0];
        assert!(descriptor.ends_with("saved-study-port-reference.json"));
        let value: Value = serde_json::from_slice(&std::fs::read(descriptor).unwrap()).unwrap();
        let base = descriptor.parent().unwrap().to_path_buf();
        let report = base.join(value["execution_report"].as_str().unwrap());
        assert_eq!(std::fs::read(report).unwrap(), b"port reference case");
        assert!(
            base.join(value["execution_workspace"].as_str().unwrap())
                .join("workspace.json")
                .is_file()
        );
        drop(materializer);
        assert!(!base.exists());
    }

    #[test]
    fn a_failed_extraction_publishes_a_failed_marker() {
        let fixture = Fixture::new();
        let archive = fixture.root().join("port/archives/reference-case.tar.gz");
        // The index still records the original hash, so the index mismatch is
        // what the extraction reports.
        let mut bytes = std::fs::read(&archive).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        std::fs::write(&archive, bytes).unwrap();
        let opened = fixture.open().unwrap();
        let materializer = opened.start_materializer().unwrap().unwrap();
        let marker = wait_for_marker(&materializer);
        assert_eq!(marker["status"], "FAILED");
        assert!(
            marker["error"]
                .as_str()
                .is_some_and(|e| e.contains("port/reference case")),
            "{marker}"
        );
    }

    #[test]
    fn dropping_the_materializer_cancels_and_joins_the_thread() {
        let fixture = Fixture::new();
        let opened = fixture.open().unwrap();
        let materializer = opened.start_materializer().unwrap().unwrap();
        let base = materializer.marker.parent().unwrap().to_path_buf();
        drop(materializer);
        assert!(!base.exists());
    }

    #[test]
    fn absent_evidence_says_why_and_what_to_download() {
        let fixture = Fixture::new();
        for path in fixture.evidence_files() {
            std::fs::remove_file(path).unwrap();
        }
        let opened = fixture.open().unwrap();
        assert!(
            matches!(&opened.evidence, Evidence::Absent { archive_name, .. }
            if archive_name == "FARIS-test-evidence.tar.gz")
        );
        assert!(opened.start_materializer().unwrap().is_none());
        let note = evidence_message(&opened.evidence).unwrap();
        assert_eq!(note.label, "Core receipts not included");
        assert_eq!(note.kind, crate::badge::Kind::NotEvaluated);
        assert!(note.text.contains("does not include the evidence part"));
        assert!(note.text.contains("download FARIS-test-evidence.tar.gz"));
    }

    #[test]
    fn incomplete_evidence_counts_the_missing_files() {
        let fixture = Fixture::new();
        std::fs::remove_file(fixture.root().join("verify.sh")).unwrap();
        let opened = fixture.open().unwrap();
        assert!(matches!(
            opened.evidence,
            Evidence::Incomplete { missing: 1, .. }
        ));
        let note = evidence_message(&opened.evidence).unwrap();
        assert!(note.text.contains("1 of the 17 evidence files are missing"));
        assert!(opened.start_materializer().unwrap().is_none());
        assert!(evidence_message(&Evidence::Present).is_none());
    }

    #[test]
    fn refuses_other_schemas_platforms_and_statuses() {
        let refused = |fixture: Fixture, needle: &str| {
            let error = fixture.open().err().expect("opened");
            assert!(error.contains(needle), "{error}");
            assert!(
                error.contains("Why:") && error.contains("SHA256SUMS"),
                "{error}"
            );
        };
        refused(
            Fixture::with(|i| i["schema_version"] = "faris-recorded-demo-package/v0.4".into()),
            "v0.4",
        );
        refused(
            Fixture::with(|i| i["local_runtime"]["platform"]["os"] = "plan9".into()),
            "plan9",
        );
        refused(
            Fixture::with(|i| i["local_runtime"]["platform"] = json!({"sys_platform": "linux"})),
            "not a valid package index",
        );
        refused(
            Fixture::with(|i| i["status"] = "PASS".into()),
            "NOT_EVALUATED",
        );
        refused(
            Fixture::with(|i| i["files"][0]["part"] = Value::Null),
            "which part",
        );
    }

    #[test]
    fn refuses_a_checksum_that_does_not_match_the_index() {
        let fixture = Fixture::new();
        std::fs::write(
            fixture.root().join(CHECKSUM),
            format!("sha256:{:064}  {INDEX}\n", 0),
        )
        .unwrap();
        assert!(fixture.open().err().unwrap().contains("does not match"));
        std::fs::write(fixture.root().join(CHECKSUM), "").unwrap();
        assert!(fixture.open().is_err());
    }

    #[test]
    fn refuses_a_changed_app_part_file() {
        let fixture = Fixture::new();
        std::fs::write(
            fixture.root().join("operating-assumptions.json"),
            b"tampered",
        )
        .unwrap();
        let error = fixture.open().err().unwrap();
        assert!(error.contains("operating-assumptions.json"), "{error}");
        let fixture = Fixture::new();
        std::fs::remove_file(fixture.root().join("control/scenario.json")).unwrap();
        assert!(fixture.open().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlinked_app_part_file() {
        let fixture = Fixture::new();
        let path = fixture.root().join("operating-assumptions.json");
        let copy = fixture.dir.path().join("elsewhere.json");
        std::fs::rename(&path, &copy).unwrap();
        std::os::unix::fs::symlink(&copy, &path).unwrap();
        let error = fixture.open().err().unwrap();
        assert!(error.contains("symlink"), "{error}");
    }

    #[test]
    fn refuses_unsafe_paths_in_the_index() {
        for bad in ["../outside", "/etc/passwd", "a//b", "a\\b", "c:/x"] {
            assert!(safe_file(Path::new("/nonexistent"), bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_app_part_file_must_be_in_the_app_part() {
        let fixture = Fixture::with(|i| {
            let files = i["files"].as_array_mut().unwrap();
            for file in files.iter_mut() {
                if file["path"] == "operating-assumptions.json" {
                    file["part"] = "evidence".into();
                }
            }
        });
        let error = fixture.open().err().unwrap();
        assert!(error.contains("not in the package's app part"), "{error}");
    }

    #[test]
    fn a_sweep_bundle_must_match_its_recorded_hash_and_folder() {
        let fixture = Fixture::with(|i| {
            i["sweep"]["runs"][0]["transport_bundle_sha256"] = sha(b"other").into();
        });
        assert!(fixture.open().err().unwrap().contains("recorded SHA-256"));
        let fixture = Fixture::with(|i| {
            i["sweep"]["runs"][0]["transport_bundle"] =
                "port/bundles/reference.transport-bundle.json".into();
        });
        assert!(
            fixture
                .open()
                .err()
                .unwrap()
                .contains("outside sweep/bundles")
        );
    }

    #[test]
    fn a_differing_executable_is_labelled_when_explicit_and_refused_when_found() {
        let fixture = Fixture::new();
        let other = fixture.dir.path().join("dev-faris-app");
        std::fs::write(&other, b"a development build").unwrap();
        let runs = fixture.dir.path().join("runs");
        let opened = open(&fixture.root(), &other, true, Some(&runs)).unwrap();
        assert!(opened.development_binary);
        let note = development_binary_message();
        assert!(
            note.text
                .contains("not the executable the package index pins")
        );
        let error = open(&fixture.root(), &other, false, Some(&runs))
            .err()
            .unwrap();
        assert!(
            error.contains("not the one this package's index pins"),
            "{error}"
        );
    }

    #[test]
    fn the_runs_directory_must_be_outside_the_package() {
        let fixture = Fixture::new();
        let inside = fixture.root().join("runs");
        let error = open(&fixture.root(), &fixture.exe(), true, Some(&inside))
            .err()
            .unwrap();
        assert!(error.contains("outside the package"), "{error}");
        let inside = fixture.root().join("bin/../runs");
        assert!(open(&fixture.root(), &fixture.exe(), true, Some(&inside)).is_err());
    }

    #[test]
    fn default_runs_directories_follow_each_platforms_convention() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                vars.iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| PathBuf::from(v))
            }
        };
        let linux = |vars| default_runs_directory("linux", "FARIS-1", &env(vars));
        assert_eq!(
            linux(&[("XDG_STATE_HOME", "/state"), ("HOME", "/home/u")]).unwrap(),
            PathBuf::from("/state/faris/recorded-demo-runs/FARIS-1")
        );
        assert_eq!(
            linux(&[("HOME", "/home/u")]).unwrap(),
            PathBuf::from("/home/u/.local/state/faris/recorded-demo-runs/FARIS-1")
        );
        // A relative XDG_STATE_HOME is not valid and is ignored.
        assert_eq!(
            linux(&[("XDG_STATE_HOME", "rel"), ("HOME", "/home/u")]).unwrap(),
            PathBuf::from("/home/u/.local/state/faris/recorded-demo-runs/FARIS-1")
        );
        assert!(linux(&[]).unwrap_err().contains("--runs-directory"));
        assert_eq!(
            default_runs_directory("macos", "FARIS-1", &env(&[("HOME", "/Users/u")])).unwrap(),
            PathBuf::from("/Users/u/Library/Application Support/FARIS/recorded-demo-runs/FARIS-1")
        );
        assert_eq!(
            default_runs_directory("windows", "FARIS-1", &env(&[("LOCALAPPDATA", "/L")])).unwrap(),
            PathBuf::from("/L/FARIS/recorded-demo-runs/FARIS-1")
        );
    }

    #[test]
    fn discovery_finds_a_package_beside_the_bin_folder() {
        let fixture = Fixture::new();
        assert_eq!(discover(&fixture.exe()), Some(fixture.root()));
        let elsewhere = fixture.dir.path().join("other/bin/faris-app");
        assert_eq!(discover(&elsewhere), None);
    }

    #[test]
    fn a_failure_message_is_a_failed_badge_with_the_text() {
        let note = failure_message("Why: x.\n\nNext step: y.");
        assert_eq!(note.kind, crate::badge::Kind::Failed);
        assert!(note.text.contains("not opened") && note.text.contains("Next step"));
    }

    #[test]
    fn the_package_flag_conflicts_with_the_study_and_data_flags() {
        use clap::Parser;
        let parse = |extra: &[&str]| {
            let mut all = vec!["faris-app", "--package", "p"];
            all.extend_from_slice(extra);
            crate::Arguments::try_parse_from(all)
        };
        assert!(parse(&[]).is_ok());
        assert!(parse(&["--core", "c", "--runs-directory", "r", "--step", "evidence"]).is_ok());
        for flag in [
            "--scenario",
            "--physics",
            "--run",
            "--bundle",
            "--sweep-bundle",
            "--control-scenario",
            "--control-physics",
            "--control-run",
            "--control-bundle",
            "--assumptions",
            "--saved-study",
        ] {
            assert!(parse(&[flag, "x"]).is_err(), "{flag}");
        }
        assert!(
            crate::Arguments::try_parse_from(["faris-app", "--package", "p", "s.faris"]).is_err()
        );
    }

    #[test]
    fn only_study_inputs_turn_off_finding_a_package() {
        use clap::Parser;
        let supplies = |args: &[&str]| {
            let mut all = vec!["faris-app"];
            all.extend_from_slice(args);
            crate::Arguments::try_parse_from(all)
                .unwrap()
                .supplies_study()
        };
        assert!(!supplies(&[]));
        assert!(!supplies(&[
            "--capture",
            "x.png",
            "--core",
            "c",
            "--step",
            "evidence"
        ]));
        assert!(supplies(&["s.faris"]));
        assert!(supplies(&["--bundle", "b.json"]));
        assert!(supplies(&["--assumptions", "a.json"]));
        assert!(supplies(&["--saved-study", "d.json"]));
    }

    /// Against a real 0.1.0 package (schema v0.4, no part tags): extract all
    /// eight recorded trees. Set FARIS_REAL_PACKAGE to the package folder.
    #[test]
    #[ignore = "needs a real package folder"]
    fn extracts_the_real_package_trees() {
        let root = PathBuf::from(std::env::var("FARIS_REAL_PACKAGE").unwrap());
        let mut value: Value =
            serde_json::from_slice(&std::fs::read(root.join(INDEX)).unwrap()).unwrap();
        let platform = Platform::current();
        value["local_runtime"]["platform"] = json!({"os": platform.os, "arch": platform.arch});
        value["parts"] = json!({"evidence": {"archive_name": "x"}});
        let index: Index = serde_json::from_value(value).unwrap();
        let trees = tree_jobs(&index).unwrap();
        let materializer =
            Materializer::start(&root, trees, index.expanded_case_workspace_bytes).unwrap();
        let marker = wait_for_marker(&materializer);
        assert_eq!(marker["status"], "COMPLETE", "{marker}");
    }
}
