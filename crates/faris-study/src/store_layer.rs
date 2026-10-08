//! Core evidence as trees of a content-addressed store (`layers.evidence_store`).
//!
//! A study records, for each saved case and workspace, the tree's name and its
//! full file listing. Packed, the file carries a store holding exactly those
//! trees under `evidence-store/`; referenced, only the listing is recorded and
//! a tree is accepted from a store that lists exactly the same files. See
//! `docs/STUDY_FILE.md`.

use crate::{
    ArchiveKind, EvidenceMode, EvidenceStoreLayer, EvidenceStoreTree, StudyError,
    is_safe_file_name, is_sha256_hex, read::sha256_file,
};
use faris_engine::evidence_store::{EvidenceStore, StoreFile, StoreTree};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

/// Directory name of the store, inside a packed file and beside a study file.
pub const STORE_DIR: &str = "evidence-store";
pub(crate) const STORE_INDEX_ENTRY: &str = "evidence-store/store.json";
const BLOB_PREFIX: &str = "evidence-store/blobs/";
/// Largest `store.json` accepted from a file (the store format's own limit).
pub(crate) const MAX_STORE_INDEX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TREE_FILES: usize = 65_536;
const MAX_LISTED_FILES: usize = 262_144;

/// Zip entry name of one blob of the embedded store.
pub(crate) fn blob_entry_name(sha256: &str) -> String {
    format!("{BLOB_PREFIX}{}/{sha256}.xz", &sha256[..2])
}

/// The digest named by an embedded-store blob entry, if the name has exactly
/// the shape `evidence-store/blobs/<h0h1>/<h>.xz`.
pub(crate) fn blob_entry_digest(name: &str) -> Option<&str> {
    let rest = name.strip_prefix(BLOB_PREFIX)?;
    let (fanout, file) = rest.split_once('/')?;
    let digest = file.strip_suffix(".xz")?;
    (is_sha256_hex(digest) && fanout == &digest[..2]).then_some(digest)
}

/// The most a stored xz blob of `bytes` uncompressed bytes can occupy: its
/// content (incompressible data costs a few bytes per 64 KiB) plus framing.
/// An entry over this is refused before it is read.
pub(crate) fn max_blob_entry_bytes(bytes: u64) -> u64 {
    bytes.saturating_add(bytes >> 10).saturating_add(1024)
}

/// A tree a study records, and the store its bytes can be copied from when the
/// file is written packed.
#[derive(Clone, Debug)]
pub struct EvidenceStoreDraft {
    pub tree: EvidenceStoreTree,
    /// Directory of the store holding the tree; needed only when packing.
    pub store: Option<PathBuf>,
}

/// Where an accepted tree is read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreSource {
    /// The store packed inside the study file.
    Embedded,
    /// An `evidence-store/` folder beside the study file.
    BesideFile,
    /// The store of the package the running app was started from.
    Package,
}

/// One saved study (case and workspace tree) whose evidence is available,
/// and the store it is read from in place.
#[derive(Clone, Debug)]
pub struct StoreStudy {
    pub arrangement: String,
    pub allocation: String,
    pub case_tree: String,
    pub workspace_tree: String,
    pub store: PathBuf,
    /// SHA-256 (lowercase hex) of the store's `store.json` when it was resolved.
    pub store_json_sha256: String,
    pub source: StoreSource,
}

/// One saved study whose evidence is not available, with why.
#[derive(Clone, Debug)]
pub struct StoreMiss {
    pub arrangement: String,
    pub allocation: String,
    pub case_tree: String,
    pub workspace_tree: String,
    /// Which tree was not found or did not match, and where was searched.
    pub reason: String,
}

/// What the `evidence_store` layer of an opened study amounts to.
#[derive(Clone, Debug)]
pub struct StoreEvidenceState {
    pub mode: EvidenceMode,
    pub found: Vec<StoreStudy>,
    pub missing: Vec<StoreMiss>,
}

/// The trees of a layer, paired into saved studies:
/// `(arrangement, allocation) -> (case tree, workspace tree)`.
pub(crate) fn pairs(
    layer: &EvidenceStoreLayer,
) -> BTreeMap<(&str, &str), (&EvidenceStoreTree, &EvidenceStoreTree)> {
    let mut cases = BTreeMap::new();
    let mut workspaces = BTreeMap::new();
    for tree in &layer.trees {
        let key = (tree.arrangement.as_str(), tree.allocation.as_str());
        match tree.kind {
            ArchiveKind::Case => cases.insert(key, tree),
            ArchiveKind::Workspace => workspaces.insert(key, tree),
        };
    }
    cases
        .into_iter()
        .filter_map(|(key, case)| Some((key, (case, *workspaces.get(&key)?))))
        .collect()
}

/// Structural rules for a recorded layer: known arrangements, safe names,
/// every saved study complete (one case and one workspace tree), listings
/// sorted and bounded. Used on both sides, so a writer cannot record what a
/// reader would refuse.
pub(crate) fn check_layer(layer: &EvidenceStoreLayer) -> Result<(), String> {
    let mut roles = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut listed = 0usize;
    let mut sizes: BTreeMap<&str, u64> = BTreeMap::new();
    for tree in &layer.trees {
        let invalid = |why: &str| {
            Err(format!(
                "evidence tree \"{}\" is invalid: {why}",
                tree.tree.escape_debug()
            ))
        };
        if !matches!(tree.arrangement.as_str(), "port" | "control")
            || !is_safe_file_name(&tree.allocation)
            || !is_safe_file_name(&tree.tree)
        {
            return invalid("unknown arrangement, or an unsafe allocation or tree name");
        }
        if !roles.insert((&tree.arrangement, &tree.allocation, tree.kind))
            || !names.insert(&tree.tree)
        {
            return invalid("repeated role or tree name");
        }
        if tree.files.len() > MAX_TREE_FILES {
            return invalid("lists too many files");
        }
        listed += tree.files.len();
        if listed > MAX_LISTED_FILES {
            return Err("the evidence trees list too many files".into());
        }
        for pair in tree.files.windows(2) {
            if pair[0].path.as_bytes() >= pair[1].path.as_bytes() {
                return invalid("files are not sorted by path, or a path is repeated");
            }
        }
        for file in &tree.files {
            if file.path.is_empty() || !is_sha256_hex(&file.sha256) {
                return invalid("a file has an empty path or an invalid digest");
            }
            if sizes
                .insert(&file.sha256, file.bytes)
                .is_some_and(|b| b != file.bytes)
            {
                return invalid("one digest is listed with two sizes");
            }
        }
    }
    let complete = pairs(layer).len() * 2;
    if complete != layer.trees.len() {
        return Err(
            "an evidence tree has no partner: every saved study needs one case and one workspace tree"
                .into(),
        );
    }
    Ok(())
}

/// Distinct file digests of the layer with their uncompressed sizes.
pub(crate) fn distinct_blobs(layer: &EvidenceStoreLayer) -> BTreeMap<&str, u64> {
    layer
        .trees
        .iter()
        .flat_map(|t| &t.files)
        .map(|f| (f.sha256.as_str(), f.bytes))
        .collect()
}

/// The layer's trees as store trees, sorted by name, as `store.json` lists them.
pub(crate) fn store_trees(layer: &EvidenceStoreLayer) -> Vec<StoreTree> {
    let mut trees: Vec<StoreTree> = layer
        .trees
        .iter()
        .map(|t| StoreTree {
            name: t.tree.clone(),
            files: t.files.clone(),
        })
        .collect();
    trees.sort_by(|a, b| a.name.cmp(&b.name));
    trees
}

/// The bytes the saved cases would add to a packed file: the distinct blobs of
/// the draft trees as they are stored (compressed) in their source stores.
/// Trees without a source store count nothing.
pub fn packed_store_bytes(drafts: &[EvidenceStoreDraft]) -> u64 {
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    for draft in drafts {
        let Some(store) = &draft.store else { continue };
        for file in &draft.tree.files {
            if !is_sha256_hex(&file.sha256) || !seen.insert(file.sha256.clone()) {
                continue;
            }
            let blob = store.join(faris_engine::evidence_store::blob_path_in_store(
                &file.sha256,
            ));
            total += std::fs::metadata(blob).map_or(0, |m| m.len());
        }
    }
    total
}

/// Why `tree` is not acceptable from `store`, or `None` when the store lists
/// exactly the recorded files for it.
fn mismatch(store: &EvidenceStore, tree: &EvidenceStoreTree) -> Option<String> {
    match store.tree(&tree.tree) {
        None => Some(format!("it has no tree {}", tree.tree)),
        Some(found) if found.files != tree.files => Some(format!(
            "its tree {} lists different files than the study recorded ({} files there, {} recorded)",
            tree.tree,
            found.files.len(),
            tree.files.len()
        )),
        Some(_) => None,
    }
}

/// Resolve each saved study of a referenced layer, in order, against a store
/// beside the study file and then the running package's store. A tree is
/// accepted only where the store's index lists exactly the recorded files;
/// every read from the store is then checked against that index. Nothing is
/// decompressed here.
pub(crate) fn resolve_referenced(
    layer: &EvidenceStoreLayer,
    near: Option<&Path>,
    package_store: Option<&Path>,
) -> StoreEvidenceState {
    struct Candidate {
        source: StoreSource,
        place: &'static str,
        path: PathBuf,
        store: Result<EvidenceStore, String>,
    }
    let beside = near.map(|dir| dir.join(STORE_DIR));
    let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    let mut candidates = Vec::new();
    for (source, place, path) in [
        (StoreSource::BesideFile, "beside the file", beside),
        (
            StoreSource::Package,
            "in the package the app was started from",
            package_store
                .filter(|p| !beside_is(near, p, &same))
                .map(Path::to_path_buf),
        ),
    ] {
        let Some(path) = path else { continue };
        let store = if path.is_dir() {
            EvidenceStore::open(&path).map_err(|e| e.to_string())
        } else {
            Err(format!("no {STORE_DIR} folder at {}", path.display()))
        };
        candidates.push(Candidate {
            source,
            place,
            path,
            store,
        });
    }
    let mut state = StoreEvidenceState {
        mode: EvidenceMode::Referenced,
        found: Vec::new(),
        missing: Vec::new(),
    };
    for ((arrangement, allocation), (case, workspace)) in pairs(layer) {
        let mut reasons = Vec::new();
        let mut accepted = None;
        for candidate in &candidates {
            match &candidate.store {
                Err(error) => reasons.push(format!("{}: {error}", candidate.place)),
                Ok(store) => match [case, workspace].iter().find_map(|t| mismatch(store, t)) {
                    None => {
                        accepted = Some(candidate);
                        break;
                    }
                    Some(why) => reasons.push(format!("{}: {why}", candidate.place)),
                },
            }
        }
        match accepted {
            Some(candidate) => {
                let index = candidate.path.join("store.json");
                match sha256_file(&index) {
                    Ok((digest, _)) => state.found.push(StoreStudy {
                        arrangement: arrangement.into(),
                        allocation: allocation.into(),
                        case_tree: case.tree.clone(),
                        workspace_tree: workspace.tree.clone(),
                        store: candidate.path.clone(),
                        store_json_sha256: digest,
                        source: candidate.source,
                    }),
                    Err(error) => state.missing.push(StoreMiss {
                        arrangement: arrangement.into(),
                        allocation: allocation.into(),
                        case_tree: case.tree.clone(),
                        workspace_tree: workspace.tree.clone(),
                        reason: format!("{} cannot be read: {error}", index.display()),
                    }),
                }
            }
            None => {
                if package_store.is_none() {
                    reasons.push("the app was not started from a package".into());
                }
                if near.is_none() {
                    reasons.push("the file's folder is unknown".into());
                }
                state.missing.push(StoreMiss {
                    arrangement: arrangement.into(),
                    allocation: allocation.into(),
                    case_tree: case.tree.clone(),
                    workspace_tree: workspace.tree.clone(),
                    reason: reasons.join("; "),
                });
            }
        }
    }
    state
}

fn beside_is(near: Option<&Path>, package: &Path, same: &dyn Fn(&Path, &Path) -> bool) -> bool {
    near.is_some_and(|dir| same(&dir.join(STORE_DIR), package))
}

/// The files of one tree as store files, for building a draft from a store.
pub fn tree_files(store: &EvidenceStore, tree: &str) -> Result<Vec<StoreFile>, StudyError> {
    store
        .tree(tree)
        .map(|t| t.files.clone())
        .ok_or_else(|| StudyError::Input(format!("the evidence store has no tree {tree}")))
}
