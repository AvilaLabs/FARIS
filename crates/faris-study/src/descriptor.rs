//! Evidence archives named by a packaged saved-study descriptor
//! (`faris-saved-study-archive/v0.1`). A store descriptor
//! (`faris-saved-study-store/v0.1`) is refused with a next step.

use crate::{
    ArchiveKind, EvidenceArchive, EvidenceDraft, StudyError, is_sha256_hex, read::sha256_file,
};
use serde::Deserialize;
use std::path::{Component, Path};

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
struct ArchiveField {
    path: String,
    sha256: String,
    bytes: u64,
}

/// The case and workspace archives a descriptor names, checked against the
/// recorded hashes and sizes. Paths resolve relative to the descriptor's
/// directory; the arrangement is the path's first component (`port` or
/// `control`) and the allocation is the file name without `-case.tar.gz` or
/// `-workspace.tar.gz`.
pub fn evidence_from_descriptor(descriptor: &Path) -> Result<Vec<EvidenceDraft>, StudyError> {
    let context = || format!("cannot read {}", descriptor.display());
    let metadata = std::fs::metadata(descriptor).map_err(|e| StudyError::io(context(), e))?;
    if !metadata.is_file() || metadata.len() > 64 * 1024 {
        return Err(StudyError::Input(format!(
            "{} must be a regular JSON file no larger than 64 KiB",
            descriptor.display()
        )));
    }
    let bytes = std::fs::read(descriptor).map_err(|e| StudyError::io(context(), e))?;
    if let Ok(Probe {
        schema_version: Some(schema),
    }) = serde_json::from_slice(&bytes)
        && schema == "faris-saved-study-store/v0.1"
    {
        return Err(StudyError::Input(format!(
            "{} names trees in an evidence store; evidence records for store trees come in a later change. \
             Use a descriptor from an older package that names archives, or create the study file without --evidence.",
            descriptor.display()
        )));
    }
    let parsed: Descriptor = serde_json::from_slice(&bytes).map_err(|e| {
        StudyError::Input(format!(
            "{} is not a saved-study descriptor: {e}",
            descriptor.display()
        ))
    })?;
    if parsed.schema_version != "faris-saved-study-archive/v0.1" {
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
