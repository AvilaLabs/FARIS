//! The `.faris` study file: one zip container holding a study's inputs, its
//! recorded transport, its assumptions, the view the author left open and,
//! optionally, the Core evidence archives. See `docs/STUDY_FILE.md`.
//!
//! Recorded files are stored byte for byte, so every hash a Core receipt binds
//! still matches after a round trip. Reading is fail-closed: anything the
//! reader cannot interpret or verify refuses the file.

mod descriptor;
mod error;
mod manifest;
mod preview;
mod read;
mod tar;
mod write;

#[cfg(test)]
mod tests;

pub use descriptor::evidence_from_descriptor;
pub use error::StudyError;
pub use manifest::{
    ArchiveKind, ArrangementRecord, Arrangements, BlobRecord, BundleRecord, ENCODING_VERBATIM,
    ENSEMBLE_MEDIA_TYPE, EnsembleRecord, EvidenceArchive, EvidenceLayer, EvidenceMode, FORMAT,
    Layers, MIMETYPE, Manifest, ViewState,
};
pub use preview::{MAX_PREVIEW_SIDE, Preview, PreviewStatus, png_dimensions};
pub use read::{
    ArrangementFiles, BlobInfo, EvidenceFile, EvidenceMiss, EvidenceState, Materialized,
    MissReason, StoredEnsemble, StudyReader, sha256_file,
};
pub use tar::{ExtractReport, RecordedTreeManifest, extract_recorded_tree, extract_tar_gz};
pub use write::{
    ArrangementDraft, DEFAULT_ZSTD_LEVEL, EnsembleDraft, EvidenceDraft, StudyDraft, WriteReport,
    write_study,
};

use sha2::{Digest, Sha256};

/// Lowercase hexadecimal SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// True for a 64-character lowercase hexadecimal digest.
pub(crate) fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A file name safe to create under a directory we choose: no separators, no
/// traversal, bounded length, a conservative character set.
pub(crate) fn is_safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A relative path of one to four safe components joined by `/`: where an
/// evidence archive is looked for, relative to the study file.
pub(crate) fn is_safe_relative_path(path: &str) -> bool {
    let parts: Vec<_> = path.split('/').collect();
    (1..=4).contains(&parts.len()) && parts.iter().all(|p| is_safe_file_name(p))
}
