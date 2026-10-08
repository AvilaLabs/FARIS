//! Core evidence named by a packaged saved-study descriptor: archives
//! (`faris-saved-study-archive/v0.1`, older packages) or two trees of an
//! evidence store (`faris-saved-study-store/v0.1`).

use crate::{
    ArchiveKind, EvidenceArchive, EvidenceDraft, EvidenceStoreDraft, EvidenceStoreTree, StudyError,
    is_safe_relative_path, is_sha256_hex, read::sha256_file,
};
use faris_engine::evidence_store::EvidenceStore;
use serde::Deserialize;
use std::path::{Component, Path};

const ARCHIVE_SCHEMA: &str = "faris-saved-study-archive/v0.1";
const STORE_SCHEMA: &str = "faris-saved-study-store/v0.1";

/// What a descriptor names.
#[derive(Clone, Debug)]
pub enum DescriptorEvidence {
    Archives(Vec<EvidenceDraft>),
    Store(Vec<EvidenceStoreDraft>),
}

#[derive(Deserialize)]
struct Descriptor {
    schema_version: String,
    case_archive: ArchiveField,
    workspace_archive: ArchiveField,
}

#[derive(Deserialize)]
struct Probe {
    schema_version: Option<String>,
}

#[derive(Deserialize)]
struct StoreDescriptor {
    /// The store folder, relative to the descriptor's directory.
    store: String,
    case_tree: String,
    workspace_tree: String,
}

#[derive(Deserialize)]
struct ArchiveField {
    path: String,
    sha256: String,
    bytes: u64,
}

fn read_descriptor(descriptor: &Path) -> Result<Vec<u8>, StudyError> {
    let context = || format!("cannot read {}", descriptor.display());
    let metadata = std::fs::metadata(descriptor).map_err(|e| StudyError::io(context(), e))?;
    if !metadata.is_file() || metadata.len() > 64 * 1024 {
        return Err(StudyError::Input(format!(
            "{} must be a regular JSON file no larger than 64 KiB",
            descriptor.display()
        )));
    }
    std::fs::read(descriptor).map_err(|e| StudyError::io(context(), e))
}

fn schema_of(bytes: &[u8]) -> Option<String> {
    serde_json::from_slice::<Probe>(bytes)
        .ok()
        .and_then(|p| p.schema_version)
}

/// The evidence a descriptor names, whichever kind it is.
pub fn descriptor_evidence(descriptor: &Path) -> Result<DescriptorEvidence, StudyError> {
    let bytes = read_descriptor(descriptor)?;
    if schema_of(&bytes).as_deref() == Some(STORE_SCHEMA) {
        Ok(DescriptorEvidence::Store(store_trees(descriptor, &bytes)?))
    } else {
        Ok(DescriptorEvidence::Archives(archives(descriptor, &bytes)?))
    }
}

/// `<arrangement>-<allocation>` from a tree named `<arrangement>-<allocation>-<suffix>`.
fn role_of<'a>(tree: &'a str, suffix: &str) -> Option<(&'a str, &'a str)> {
    let stem = tree.strip_suffix(suffix)?;
    let (arrangement, allocation) = stem.split_once('-')?;
    (matches!(arrangement, "port" | "control") && !allocation.is_empty())
        .then_some((arrangement, allocation))
}

/// The case and workspace trees a store descriptor names, with the listing of
/// each read from the store's index (nothing is decompressed). The store is
/// resolved relative to the descriptor, as the package does. The arrangement
/// and allocation come from the tree names, `<arrangement>-<allocation>-case`
/// and `-workspace`.
fn store_trees(descriptor: &Path, bytes: &[u8]) -> Result<Vec<EvidenceStoreDraft>, StudyError> {
    let parsed: StoreDescriptor = serde_json::from_slice(bytes).map_err(|e| {
        StudyError::Input(format!(
            "{} is not a saved-study descriptor: {e}",
            descriptor.display()
        ))
    })?;
    if !is_safe_relative_path(&parsed.store) {
        return Err(StudyError::Input(format!(
            "store path \"{}\" must be relative and without traversal",
            parsed.store.escape_debug()
        )));
    }
    let root = descriptor
        .parent()
        .unwrap_or(Path::new("."))
        .join(&parsed.store);
    let store = EvidenceStore::open(&root).map_err(|e| {
        StudyError::Input(format!(
            "cannot open the evidence store {} named by {}: {e}",
            root.display(),
            descriptor.display()
        ))
    })?;
    let mut roles = Vec::new();
    let mut drafts = Vec::new();
    for (kind, name, suffix) in [
        (ArchiveKind::Case, &parsed.case_tree, "-case"),
        (ArchiveKind::Workspace, &parsed.workspace_tree, "-workspace"),
    ] {
        let (arrangement, allocation) = role_of(name, suffix).ok_or_else(|| {
            StudyError::Input(format!(
                "tree \"{}\" is not named <port|control>-<allocation>{suffix}",
                name.escape_debug()
            ))
        })?;
        roles.push((arrangement, allocation));
        let files = crate::store_layer::tree_files(&store, name)?;
        drafts.push(EvidenceStoreDraft {
            tree: EvidenceStoreTree {
                arrangement: arrangement.to_owned(),
                allocation: allocation.to_owned(),
                kind,
                tree: name.clone(),
                files,
            },
            store: Some(root.clone()),
        });
    }
    if roles[0] != roles[1] {
        return Err(StudyError::Input(format!(
            "{} names a case tree and a workspace tree of different studies",
            descriptor.display()
        )));
    }
    Ok(drafts)
}

/// The case and workspace archives an archive descriptor names, checked
/// against the recorded hashes and sizes. A descriptor that names store trees
/// is refused here; use [`descriptor_evidence`]. Paths resolve relative to the
/// descriptor's directory; the arrangement is the path's first component
/// (`port` or `control`) and the allocation is the file name without
/// `-case.tar.gz` or `-workspace.tar.gz`.
pub fn evidence_from_descriptor(descriptor: &Path) -> Result<Vec<EvidenceDraft>, StudyError> {
    let bytes = read_descriptor(descriptor)?;
    if schema_of(&bytes).as_deref() == Some(STORE_SCHEMA) {
        return Err(StudyError::Input(format!(
            "{} names trees in an evidence store, not archives; read it with descriptor_evidence",
            descriptor.display()
        )));
    }
    archives(descriptor, &bytes)
}

fn archives(descriptor: &Path, bytes: &[u8]) -> Result<Vec<EvidenceDraft>, StudyError> {
    let parsed: Descriptor = serde_json::from_slice(bytes).map_err(|e| {
        StudyError::Input(format!(
            "{} is not a saved-study descriptor: {e}",
            descriptor.display()
        ))
    })?;
    if parsed.schema_version != ARCHIVE_SCHEMA {
        return Err(StudyError::Input(format!(
            "{} has unsupported schema \"{}\"",
            descriptor.display(),
            parsed.schema_version
        )));
    }
    let root = descriptor.parent().unwrap_or(Path::new("."));
    let mut drafts = Vec::new();
    for (kind, field) in [
        (ArchiveKind::Case, parsed.case_archive),
        (ArchiveKind::Workspace, parsed.workspace_archive),
    ] {
        let relative = Path::new(&field.path);
        let parts: Vec<&str> = field.path.split('/').collect();
        if relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
            || parts.len() < 2
        {
            return Err(StudyError::Input(format!(
                "archive path \"{}\" must be relative, without traversal, and start with port/ or control/",
                field.path
            )));
        }
        let arrangement = parts[0];
        let suffix = format!(
            "-{}.tar.gz",
            match kind {
                ArchiveKind::Case => "case",
                ArchiveKind::Workspace => "workspace",
            }
        );
        let allocation = parts[parts.len() - 1]
            .strip_suffix(&suffix)
            .ok_or_else(|| {
                StudyError::Input(format!(
                    "archive \"{}\" is not named <allocation>{suffix}",
                    field.path
                ))
            })?;
        let sha256 = field
            .sha256
            .strip_prefix("sha256:")
            .unwrap_or(&field.sha256)
            .to_owned();
        if !matches!(arrangement, "port" | "control") || !is_sha256_hex(&sha256) {
            return Err(StudyError::Input(format!(
                "archive \"{}\" has an unknown arrangement or an invalid hash",
                field.path
            )));
        }
        let path = root.join(relative);
        let (found, bytes) = sha256_file(&path)
            .map_err(|e| StudyError::io(format!("cannot read {}", path.display()), e))?;
        if found != sha256 || bytes != field.bytes {
            return Err(StudyError::Input(format!(
                "{} does not match the hash and size recorded in {}",
                path.display(),
                descriptor.display()
            )));
        }
        drafts.push(EvidenceDraft {
            archive: EvidenceArchive {
                arrangement: arrangement.to_owned(),
                allocation: allocation.to_owned(),
                kind,
                file_name: field.path.clone(),
                sha256,
                bytes,
            },
            path: Some(path),
        });
    }
    Ok(drafts)
}
