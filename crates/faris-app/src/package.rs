//! Package mode: opening the study a release package carries, with no launcher
//! script. The package index binds every file the app reads at launch; this
//! module checks that binding before anything is loaded, builds the inputs the
//! command-line flags would have passed, and (when the evidence part is
//! present) names the evidence store the saved Core studies are read from in
//! place.
//!
//! Full verification of every indexed file stays with `verify.sh`; the checks
//! here are the launch-relevant part of it. Nothing here interprets physics.

use faris_study::sha256_file;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub const INDEX: &str = "package-index.json";
pub const CHECKSUM: &str = "package-index.sha256";
const SCHEMA: &str = "faris-recorded-demo-package/v0.6";
const STATUS: &str = "IDENTITIES_REVALIDATED_CORE_EXECUTIONS_COMPLETED_PHYSICS_NOT_EVALUATED";
const MAX_INDEX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FILES: usize = 2048;
const SWEEP_BUNDLE_DIRECTORY: &str = "sweep/bundles";
/// The recorded maintenance result an app-part package may carry.
const MAINTENANCE_RESULT: &str = "maintenance/maintenance-result.json";
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

#[derive(Deserialize)]
struct StoreTreeRecord {
    name: String,
}

/// The evidence store the package carries: its folder, the SHA-256 of its
/// `store.json` (which holds every blob's digest) and the trees in it.
#[derive(Deserialize)]
struct EvidenceStoreRecord {
    path: String,
    store_json_sha256: String,
    trees: Vec<StoreTreeRecord>,
}

#[derive(Deserialize)]
struct Arrangement {
    variant_id: String,
    case_tree: String,
    workspace_tree: String,
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
    evidence_store: EvidenceStoreRecord,
    scenario_pairs: Vec<ScenarioPair>,
    #[serde(default)]
    sweep: Option<Sweep>,
}

/// One saved Core study: the case tree and the workspace tree that hold it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedStudy {
    pub pair: String,
    pub variant: String,
    pub case_tree: String,
    pub workspace_tree: String,
}

/// The evidence store holding the four saved studies, read in place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedStore {
    pub path: PathBuf,
    /// Lowercase hex SHA-256 of the store's `store.json`, as the index records it.
    pub index_sha256: String,
    pub studies: Vec<SavedStudy>,
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

/// A recorded maintenance result in the package's app part and the SHA-256 its index records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedMaintenance {
    pub path: PathBuf,
    /// `sha256:<hex>`, as in the index.
    pub sha256: String,
}

/// The bytes of `path` when they have the SHA-256 `expected` (`sha256:<hex>`); refuses any
/// other content, so a file changed after launch is never loaded.
pub fn read_verified(path: &Path, expected: &str) -> Result<Vec<u8>, String> {
    let name = path.display();
    let size = std::fs::metadata(path)
        .map_err(|e| format!("{name} cannot be read: {e}"))?
        .len();
    if size > MAX_FILE_BYTES {
        return Err(format!("{name} is larger than the package file limit"));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{name} cannot be read: {e}"))?;
    if format!("sha256:{:x}", Sha256::digest(&bytes)) != expected {
        return Err(format!(
            "{name} differs from the package index in SHA-256; download the package again"
        ));
    }
    Ok(bytes)
}

/// A package that passed its checks, ready to load.
pub struct Opened {
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
    /// The recorded maintenance result, if the package's app part carries one.
    pub maintenance_result: Option<RecordedMaintenance>,
    /// The saved studies, when the evidence part is present.
    pub saved_store: Option<SavedStore>,
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
    let app_relative = index
        .local_runtime
        .executables
        .get("faris-app")
        .map(|e| e.path.as_str())
        .ok_or_else(|| fail("the package index names no FARIS app executable"))?;
    in_app(app_relative)?;

    let maintenance_result = match app.get(MAINTENANCE_RESULT) {
        Some(entry) => Some(RecordedMaintenance {
            path: in_app(MAINTENANCE_RESULT)?,
            sha256: entry.sha256.clone(),
        }),
        None => None,
    };
    let saved_studies = saved_studies(&index)?;
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
    let saved_store = if evidence == Evidence::Present {
        Some(saved_store(&root, &index, saved_studies)?)
    } else {
        None
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
        maintenance_result,
        saved_store,
    })
}

/// The four saved studies and the trees the index says hold them.
fn saved_studies(index: &Index) -> Result<Vec<SavedStudy>, String> {
    let mut studies = Vec::new();
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
        for tree in [&arrangement.case_tree, &arrangement.workspace_tree] {
            if !index.evidence_store.trees.iter().any(|t| &t.name == tree) {
                return Err(fail(format!(
                    "the package index names the tree {tree} for {pair_id}/{variant}, but the \
                     evidence store record does not list it"
                )));
            }
        }
        studies.push(SavedStudy {
            pair: pair_id.into(),
            variant: variant.into(),
            case_tree: arrangement.case_tree.clone(),
            workspace_tree: arrangement.workspace_tree.clone(),
        });
    }
    Ok(studies)
}

/// Where the evidence store is, once its `store.json` is an indexed evidence
/// file of the package. Its content is checked against the recorded digest
/// when the store is opened, and every file read from it against `store.json`.
fn saved_store(root: &Path, index: &Index, studies: Vec<SavedStudy>) -> Result<SavedStore, String> {
    let record = &index.evidence_store;
    let index_file = format!("{}/store.json", record.path);
    safe_file(root, &index_file).map_err(fail)?;
    let in_index = index
        .files
        .iter()
        .any(|f| f.path == index_file && f.part.as_deref() == Some("evidence"));
    let recorded = record.store_json_sha256.strip_prefix("sha256:");
    let Some(recorded) = recorded.filter(|hex| hex.len() == 64) else {
        return Err(fail(
            "the package index records no valid SHA-256 for store.json",
        ));
    };
    if !in_index {
        return Err(fail(format!(
            "the package index does not list {index_file} as an evidence file"
        )));
    }
    Ok(SavedStore {
        path: root.join(&record.path),
        index_sha256: recorded.to_owned(),
        studies,
    })
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
    use faris_engine::evidence_store::{EvidenceStore, pack_store};
    use serde_json::{Value, json};

    fn sha(bytes: &[u8]) -> String {
        format!("sha256:{}", faris_study::sha256_hex(bytes))
    }

    /// A v0.6 package in a temporary folder: app part, evidence part, index.
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
            // Eight small trees (a case and a workspace for each saved study) in
            // one store inside the package's evidence part.
            let sources = fixture.dir.path().join("trees");
            let mut trees = Vec::new();
            let mut pairs = Vec::new();
            for pair in ["port", "control"] {
                let mut arrangements = Vec::new();
                for variant in ["reference", "breeder-emphasis"] {
                    let mut tree = |kind: &str, file: &str| {
                        let name = format!("{pair}-{variant}-{kind}");
                        let directory = sources.join(&name);
                        std::fs::create_dir_all(&directory).unwrap();
                        std::fs::write(directory.join(file), format!("{pair} {variant} {kind}"))
                            .unwrap();
                        std::fs::write(directory.join("shared.json"), b"shared by every tree")
                            .unwrap();
                        trees.push((name.clone(), directory));
                        name
                    };
                    arrangements.push(json!({
                        "variant_id": variant,
                        "case_tree": tree("case", "execution-report.json"),
                        "workspace_tree": tree("workspace", "workspace.json"),
                    }));
                }
                pairs.push(json!({
                    "scenario_path": format!("{pair}/scenario.json"),
                    "arrangements": arrangements,
                }));
            }
            let store = root.join("evidence-store");
            pack_store(&store, &trees).unwrap();
            let mut store_files = vec!["store.json".to_owned()];
            for fanout in std::fs::read_dir(store.join("blobs")).unwrap() {
                let fanout = fanout.unwrap();
                for blob in std::fs::read_dir(fanout.path()).unwrap() {
                    store_files.push(format!(
                        "blobs/{}/{}",
                        fanout.file_name().to_string_lossy(),
                        blob.unwrap().file_name().to_string_lossy()
                    ));
                }
            }
            for relative in &store_files {
                let bytes = std::fs::read(store.join(relative)).unwrap();
                files.push(json!({
                    "path": format!("evidence-store/{relative}"), "bytes": bytes.len(),
                    "sha256": sha(&bytes), "part": "evidence",
                }));
            }
            let record = json!({
                "path": "evidence-store",
                "store_json_sha256": sha(&std::fs::read(store.join("store.json")).unwrap()),
                "trees": trees.iter().map(|(name, _)| json!({"name": name})).collect::<Vec<_>>(),
            });
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
                "evidence_store": record,
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

        /// Adds a recorded maintenance result to the app part and the index.
        fn add_recorded_maintenance(&self, bytes: &[u8]) {
            let path = self.root().join(MAINTENANCE_RESULT);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, bytes).unwrap();
            let mut index: Value =
                serde_json::from_slice(&std::fs::read(self.root().join(INDEX)).unwrap()).unwrap();
            index["files"].as_array_mut().unwrap().push(json!({
                "path": MAINTENANCE_RESULT, "bytes": bytes.len(), "sha256": sha(bytes),
                "part": "app",
            }));
            self.write_index(&index);
        }

        fn evidence_files(&self) -> Vec<PathBuf> {
            let mut found = vec![self.root().join("verify.sh")];
            let store = self.root().join("evidence-store");
            found.push(store.join("store.json"));
            for fanout in std::fs::read_dir(store.join("blobs")).unwrap() {
                for blob in std::fs::read_dir(fanout.unwrap().path()).unwrap() {
                    found.push(blob.unwrap().path());
                }
            }
            found
        }
    }

    #[test]
    fn opens_a_v06_package_and_builds_the_inputs() {
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
        let saved = opened.saved_store.expect("the evidence part is present");
        assert_eq!(saved.path, root.join("evidence-store"));
        assert_eq!(saved.studies.len(), 4);
        assert_eq!(
            saved.studies[0],
            SavedStudy {
                pair: "port".into(),
                variant: "reference".into(),
                case_tree: "port-reference-case".into(),
                workspace_tree: "port-reference-workspace".into(),
            }
        );
    }

    #[test]
    fn a_recorded_maintenance_result_is_found_and_hash_checked() {
        let plain = Fixture::new().open().unwrap();
        assert_eq!(plain.maintenance_result, None);

        let fixture = Fixture::new();
        fixture.add_recorded_maintenance(b"{\"recorded\": true}");
        let recorded = fixture.open().unwrap().maintenance_result.unwrap();
        assert_eq!(
            recorded.path,
            fixture
                .root()
                .canonicalize()
                .unwrap()
                .join(MAINTENANCE_RESULT)
        );
        assert_eq!(recorded.sha256, sha(b"{\"recorded\": true}"));
        assert_eq!(
            read_verified(&recorded.path, &recorded.sha256).unwrap(),
            b"{\"recorded\": true}"
        );
        // Changed after launch: refused, never loaded.
        std::fs::write(&recorded.path, b"{\"recorded\": false}").unwrap();
        let error = read_verified(&recorded.path, &recorded.sha256).unwrap_err();
        assert!(error.contains("differs from the package index"), "{error}");
        // A changed file also fails the launch checks of the app part.
        let error = fixture.open().err().unwrap();
        assert!(error.contains("maintenance-result.json differs"), "{error}");
    }

    #[test]
    fn the_saved_studies_are_read_from_the_store_in_place_with_nothing_expanded() {
        let fixture = Fixture::new();
        let opened = fixture.open().unwrap();
        let saved = opened.saved_store.unwrap();
        let store = EvidenceStore::open_expecting(&saved.path, &saved.index_sha256).unwrap();
        let study = &saved.studies[0];
        assert_eq!(
            store
                .read_file(&study.case_tree, "execution-report.json", 1 << 20)
                .unwrap(),
            b"port reference case"
        );
        assert_eq!(
            store
                .read_file(&study.workspace_tree, "workspace.json", 1 << 20)
                .unwrap(),
            b"port reference workspace"
        );
        // Opening the package and reading it wrote nothing into the package
        // and made no temporary expansion.
        assert!(!fixture.root().join("materialized").exists());
        assert!(store.verify().finding_count == 0);
    }

    #[test]
    fn a_changed_store_index_is_not_used() {
        let fixture = Fixture::new();
        let opened = fixture.open().unwrap();
        let saved = opened.saved_store.unwrap();
        let index_path = saved.path.join("store.json");
        let mut text = std::fs::read_to_string(&index_path).unwrap();
        text.push(' ');
        std::fs::write(&index_path, text).unwrap();
        let error = EvidenceStore::open_expecting(&saved.path, &saved.index_sha256)
            .unwrap_err()
            .to_string();
        assert!(error.contains("package index records"), "{error}");
    }

    #[test]
    fn a_tampered_blob_is_refused_when_its_file_is_read() {
        let fixture = Fixture::new();
        let opened = fixture.open().unwrap();
        let saved = opened.saved_store.unwrap();
        let store = EvidenceStore::open_expecting(&saved.path, &saved.index_sha256).unwrap();
        let entry = store
            .entry(&saved.studies[0].case_tree, "execution-report.json")
            .unwrap()
            .clone();
        let blob = saved
            .path
            .join("blobs")
            .join(&entry.sha256[..2])
            .join(format!("{}.xz", entry.sha256));
        let mut bytes = std::fs::read(&blob).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 1;
        std::fs::write(&blob, bytes).unwrap();
        assert!(
            store
                .read_file(
                    &saved.studies[0].case_tree,
                    "execution-report.json",
                    1 << 20
                )
                .is_err()
        );
    }

    #[test]
    fn the_index_must_list_the_trees_it_assigns_to_the_saved_studies() {
        let fixture = Fixture::with(|i| {
            i["evidence_store"]["trees"] = json!([{"name": "port-reference-case"}]);
        });
        let error = fixture.open().err().unwrap();
        assert!(error.contains("does not list it"), "{error}");
        let fixture = Fixture::with(|i| i["evidence_store"]["store_json_sha256"] = "x".into());
        let error = fixture.open().err().unwrap();
        assert!(error.contains("no valid SHA-256"), "{error}");
        let fixture = Fixture::with(|i| i["evidence_store"]["path"] = "../elsewhere".into());
        assert!(fixture.open().is_err());
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
        assert!(opened.saved_store.is_none());
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
        let Evidence::Incomplete { total, .. } = opened.evidence else {
            unreachable!()
        };
        let note = evidence_message(&opened.evidence).unwrap();
        assert!(
            note.text
                .contains(&format!("1 of the {} evidence files are missing", total))
        );
        assert!(opened.saved_store.is_none());
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
            Fixture::with(|i| i["schema_version"] = "faris-recorded-demo-package/v0.5".into()),
            "v0.5",
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
}
