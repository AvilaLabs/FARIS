//! Reader, verifier and writer for the content-addressed evidence store.
//!
//! The format is `avila.core/evidence-store/v0.1` (Avila Core ADR-0028): a
//! directory holding `store.json` and one `blobs/<h0h1>/<h>.xz` per distinct
//! file content, where `<h>` is the SHA-256 of the content before compression.
//! Several named file trees share those blobs. This is FARIS's own
//! implementation, as `scripts/evidence_store.py` is; neither shares code with
//! Core.
//!
//! Nothing read from a store is believed. The index is checked against the
//! format rules before use, each blob is decompressed with its output bounded
//! to the indexed length plus one byte, and content is returned only when its
//! length and SHA-256 equal the index entry. A blob that is a symlink or not a
//! regular file, or has data after the end of its xz stream, is refused. A
//! store is not a security boundary: anyone can write one, and integrity comes
//! from checking digests against the records that name them.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
use thiserror::Error;
use xz2::{
    stream::{Action, Status, Stream},
    write::XzEncoder,
};

pub const SCHEMA_VERSION: &str = "avila.core/evidence-store/v0.1";
pub const CODEC: &str = "xz";

const XZ_PRESET: u32 = 9;
/// Decoder memory ceiling. Preset 9 needs about 65 MiB; the limit only stops a
/// hostile stream from declaring an enormous dictionary.
const DECODER_MEMORY_LIMIT: u64 = 1 << 30;
const MAX_INDEX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TREES: usize = 1_024;
const MAX_FILES_PER_TREE: usize = 65_536;
const MAX_FILES_TOTAL: usize = 262_144;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_TREE_NAME_CHARS: usize = 128;
const MAX_PATH_COMPONENTS: usize = 64;
const MAX_REPORTED_FINDINGS: usize = 1_000;
const INDEX_FILE: &str = "store.json";
const BLOBS_DIR: &str = "blobs";
const CHUNK: usize = 64 * 1024;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("evidence store I/O error at `{path}`: {source}")]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("invalid evidence store index: {0}")]
    InvalidIndex(String),
    #[error("cannot pack an evidence store: {0}")]
    InvalidInput(String),
    #[error("evidence store has no tree `{0}`")]
    UnknownTree(String),
    #[error("evidence store tree `{tree}` has no file `{path}`")]
    UnknownFile { tree: String, path: String },
    #[error("evidence store blob {sha256} is not valid: {detail}")]
    Blob { sha256: String, detail: String },
    #[error("evidence store layout is not valid: {0}")]
    Layout(String),
}

fn io_error(path: &Path, source: io::Error) -> StoreError {
    StoreError::Io {
        path: path.display().to_string(),
        source,
    }
}

/// One file in a tree: where it lives and the identity of its content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreFile {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreTree {
    pub name: String,
    pub files: Vec<StoreFile>,
}

impl StoreTree {
    /// Total uncompressed bytes of the tree's files.
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).sum()
    }

    /// Directories implied by the file paths (the tree root is not counted).
    pub fn directory_count(&self) -> usize {
        let mut directories = BTreeSet::new();
        for file in &self.files {
            let mut end = 0;
            let parts: Vec<&str> = file.path.split('/').collect();
            for part in &parts[..parts.len() - 1] {
                end += part.len();
                directories.insert(&file.path[..end]);
                end += 1;
            }
        }
        directories.len()
    }
}

/// The parsed `store.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreIndex {
    pub schema_version: String,
    pub codec: String,
    pub trees: Vec<StoreTree>,
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
}

fn validate_tree_name(name: &str) -> Result<(), String> {
    let count = name.chars().count();
    if count == 0 || count > MAX_TREE_NAME_CHARS {
        return Err(format!(
            "tree name `{name}` must be 1 to {MAX_TREE_NAME_CHARS} characters"
        ));
    }
    if name == "." || name == ".." {
        return Err(format!("tree name `{name}` is not allowed"));
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(format!(
            "tree name `{name}` may contain only A-Z a-z 0-9 . _ -"
        ));
    }
    Ok(())
}

fn validate_store_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("path is empty".into());
    }
    if path.starts_with('/') {
        return Err(format!("path `{path}` is absolute"));
    }
    if path.chars().any(|c| c == '\\' || c.is_control()) {
        return Err(format!(
            "path `{}` contains a backslash or control character",
            path.escape_debug()
        ));
    }
    let components: Vec<&str> = path.split('/').collect();
    if components.len() > MAX_PATH_COMPONENTS {
        return Err(format!(
            "path `{path}` has more than {MAX_PATH_COMPONENTS} components"
        ));
    }
    if components
        .iter()
        .any(|c| c.is_empty() || *c == "." || *c == "..")
    {
        return Err(format!("path `{path}` has an empty, `.` or `..` component"));
    }
    Ok(())
}

/// Rules 1 to 3 and 6.
fn validate_index(index: &StoreIndex) -> Result<(), StoreError> {
    let invalid = |message: String| Err(StoreError::InvalidIndex(message));
    if index.schema_version != SCHEMA_VERSION {
        return invalid(format!(
            "schema_version must be `{SCHEMA_VERSION}`, found `{}`",
            index.schema_version.escape_debug()
        ));
    }
    if index.codec != CODEC {
        return invalid(format!(
            "codec must be `{CODEC}`, found `{}`",
            index.codec.escape_debug()
        ));
    }
    if index.trees.len() > MAX_TREES {
        return invalid(format!("more than {MAX_TREES} trees"));
    }
    let mut total_files = 0usize;
    let mut lengths: BTreeMap<&str, u64> = BTreeMap::new();
    let mut previous_name: Option<&str> = None;
    for tree in &index.trees {
        if let Err(message) = validate_tree_name(&tree.name) {
            return invalid(message);
        }
        if let Some(previous) = previous_name {
            if previous == tree.name {
                return invalid(format!("duplicate tree name `{}`", tree.name));
            }
            if previous > tree.name.as_str() {
                return invalid(format!(
                    "trees are not sorted by name (`{}` after `{previous}`)",
                    tree.name
                ));
            }
        }
        previous_name = Some(&tree.name);
        if tree.files.len() > MAX_FILES_PER_TREE {
            return invalid(format!(
                "tree `{}` has more than {MAX_FILES_PER_TREE} files",
                tree.name
            ));
        }
        total_files += tree.files.len();
        if total_files > MAX_FILES_TOTAL {
            return invalid(format!("more than {MAX_FILES_TOTAL} files in total"));
        }
        let mut paths: BTreeSet<&str> = BTreeSet::new();
        let mut previous_path: Option<&str> = None;
        for file in &tree.files {
            if let Err(message) = validate_store_path(&file.path) {
                return invalid(format!("tree `{}`: {message}", tree.name));
            }
            if let Some(previous) = previous_path {
                if previous == file.path {
                    return invalid(format!(
                        "tree `{}` lists `{}` more than once",
                        tree.name, file.path
                    ));
                }
                if previous.as_bytes() > file.path.as_bytes() {
                    return invalid(format!(
                        "tree `{}` files are not sorted by path (`{}` after `{previous}`)",
                        tree.name, file.path
                    ));
                }
            }
            previous_path = Some(&file.path);
            paths.insert(&file.path);
            if !is_lower_hex_sha256(&file.sha256) {
                return invalid(format!(
                    "tree `{}` file `{}`: sha256 must be 64 lowercase hexadecimal characters",
                    tree.name, file.path
                ));
            }
            if file.bytes > MAX_FILE_BYTES {
                return invalid(format!(
                    "tree `{}` file `{}`: {} bytes exceeds the {MAX_FILE_BYTES}-byte limit",
                    tree.name, file.path, file.bytes
                ));
            }
            match lengths.get(file.sha256.as_str()) {
                Some(known) if *known != file.bytes => {
                    return invalid(format!(
                        "digest {} is recorded with two different lengths ({known} and {})",
                        file.sha256, file.bytes
                    ));
                }
                Some(_) => {}
                None => {
                    lengths.insert(&file.sha256, file.bytes);
                }
            }
        }
        // Sorted order does not make a file and a directory of the same name
        // adjacent ("a", "a-x", "a/b"), so look up every proper ancestor.
        for path in &paths {
            let mut end = 0;
            for component in path.split('/') {
                end += component.len();
                if end >= path.len() {
                    break;
                }
                if paths.contains(&path[..end]) {
                    return invalid(format!(
                        "tree `{}`: `{}` is a file and also a directory prefix of `{path}`",
                        tree.name,
                        &path[..end]
                    ));
                }
                end += 1;
            }
        }
    }
    Ok(())
}

fn blob_relative(sha256: &str) -> PathBuf {
    Path::new(BLOBS_DIR)
        .join(&sha256[..2])
        .join(format!("{sha256}.xz"))
}

fn blob_error(sha256: &str, detail: impl Into<String>) -> StoreError {
    StoreError::Blob {
        sha256: sha256.into(),
        detail: detail.into(),
    }
}

/// Decompress one blob, passing its bytes to `sink` as they arrive. Output is
/// bounded to `bytes` + 1, and the length, the SHA-256 and the absence of
/// trailing data are checked at the end: what `sink` received is provisional
/// until this returns `Ok`.
fn decode_blob(
    path: &Path,
    sha256: &str,
    bytes: u64,
    mut sink: impl FnMut(&[u8]),
) -> Result<(), StoreError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            blob_error(sha256, "the blob file is missing")
        } else {
            io_error(path, error)
        }
    })?;
    if !metadata.is_file() {
        return Err(blob_error(sha256, "the blob is not a regular file"));
    }
    let mut file = File::open(path).map_err(|e| io_error(path, e))?;
    let mut stream = Stream::new_stream_decoder(DECODER_MEMORY_LIMIT, 0)
        .map_err(|e| blob_error(sha256, format!("cannot start the xz decoder: {e}")))?;
    let mut input = vec![0u8; CHUNK];
    let mut output = vec![0u8; CHUNK];
    let (mut position, mut length, mut eof) = (0usize, 0usize, false);
    let mut hasher = Sha256::new();
    let mut produced = 0u64;
    loop {
        if position == length && !eof {
            length = file.read(&mut input).map_err(|e| io_error(path, e))?;
            position = 0;
            eof = length == 0;
        }
        // At most one byte past the indexed length is ever produced.
        let allowance = (bytes + 1 - produced).min(output.len() as u64) as usize;
        let (before_in, before_out) = (stream.total_in(), stream.total_out());
        let status = stream
            .process(
                &input[position..length],
                &mut output[..allowance],
                Action::Run,
            )
            .map_err(|e| blob_error(sha256, format!("xz decoding failed: {e}")))?;
        let consumed = (stream.total_in() - before_in) as usize;
        let written = (stream.total_out() - before_out) as usize;
        position += consumed;
        if written > 0 {
            produced += written as u64;
            if produced > bytes {
                return Err(blob_error(
                    sha256,
                    format!("decompresses to more than the {bytes} bytes the index records"),
                ));
            }
            hasher.update(&output[..written]);
            sink(&output[..written]);
        }
        match status {
            Status::StreamEnd => break,
            Status::MemNeeded => {
                return Err(blob_error(sha256, "the xz stream exceeds the memory limit"));
            }
            Status::Ok | Status::GetCheck => {}
        }
        if written == 0 && consumed == 0 {
            if eof {
                return Err(blob_error(sha256, "the xz stream is truncated"));
            }
            if position < length {
                return Err(blob_error(sha256, "the xz decoder made no progress"));
            }
        }
    }
    let mut probe = [0u8; 1];
    if position < length || (!eof && file.read(&mut probe).map_err(|e| io_error(path, e))? != 0) {
        return Err(blob_error(sha256, "data follows the end of the xz stream"));
    }
    if produced != bytes {
        return Err(blob_error(
            sha256,
            format!("decompressed to {produced} bytes, index records {bytes}"),
        ));
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual != sha256 {
        return Err(blob_error(
            sha256,
            format!("decompressed content hashes to {actual}"),
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreVerifyStatus {
    Verified,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StoreFinding {
    pub kind: &'static str,
    pub path: String,
    pub detail: String,
}

/// The result of `EvidenceStore::verify`, in the same shape as the Python
/// verifier's report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StoreVerifyReport {
    pub store: String,
    pub status: StoreVerifyStatus,
    pub trees: usize,
    pub files: usize,
    pub distinct_blobs: usize,
    pub uncompressed_bytes: u64,
    pub stored_bytes: u64,
    pub finding_count: usize,
    pub findings: Vec<StoreFinding>,
}

#[derive(Default)]
struct Findings {
    count: usize,
    items: Vec<StoreFinding>,
}

impl Findings {
    fn push(&mut self, kind: &'static str, path: impl AsRef<Path>, detail: String) {
        self.count += 1;
        if self.items.len() < MAX_REPORTED_FINDINGS {
            self.items.push(StoreFinding {
                kind,
                path: path.as_ref().display().to_string(),
                detail,
            });
        }
    }
}

/// A store whose index has been read and validated. Opening touches no blob.
#[derive(Debug)]
pub struct EvidenceStore {
    root: PathBuf,
    index: StoreIndex,
}

impl EvidenceStore {
    /// Read and validate `store.json`. A store whose index breaks the format
    /// rules does not open.
    pub fn open(root: &Path) -> Result<Self, StoreError> {
        Self::open_checked(root, None)
    }

    /// [`open`](Self::open), also requiring `store.json` to have the SHA-256
    /// (64 lowercase hex characters) a package index records for it. Every
    /// blob digest is in `store.json`, so this binds the whole store.
    pub fn open_expecting(root: &Path, index_sha256: &str) -> Result<Self, StoreError> {
        Self::open_checked(root, Some(index_sha256))
    }

    fn open_checked(root: &Path, expected_sha256: Option<&str>) -> Result<Self, StoreError> {
        let root_metadata = fs::metadata(root).map_err(|e| io_error(root, e))?;
        if !root_metadata.is_dir() {
            return Err(StoreError::Layout(format!(
                "`{}` is not a directory",
                root.display()
            )));
        }
        let index_path = root.join(INDEX_FILE);
        let metadata = fs::symlink_metadata(&index_path).map_err(|e| io_error(&index_path, e))?;
        if !metadata.is_file() {
            return Err(StoreError::Layout(format!(
                "`{}` is not a regular file",
                index_path.display()
            )));
        }
        let mut raw = Vec::new();
        File::open(&index_path)
            .and_then(|f| f.take(MAX_INDEX_BYTES + 1).read_to_end(&mut raw))
            .map_err(|e| io_error(&index_path, e))?;
        if raw.len() as u64 > MAX_INDEX_BYTES {
            return Err(StoreError::InvalidIndex(format!(
                "{INDEX_FILE} is larger than {MAX_INDEX_BYTES} bytes"
            )));
        }
        if let Some(expected) = expected_sha256 {
            let actual = format!("{:x}", Sha256::digest(&raw));
            if actual != expected {
                return Err(StoreError::InvalidIndex(format!(
                    "{INDEX_FILE} has SHA-256 {actual}, and the package index records {expected}"
                )));
            }
        }
        let index: StoreIndex =
            serde_json::from_slice(&raw).map_err(|e| StoreError::InvalidIndex(e.to_string()))?;
        validate_index(&index)?;
        Ok(Self {
            root: root.to_path_buf(),
            index,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn index(&self) -> &StoreIndex {
        &self.index
    }

    pub fn tree(&self, name: &str) -> Option<&StoreTree> {
        self.index
            .trees
            .binary_search_by(|tree| tree.name.as_str().cmp(name))
            .ok()
            .map(|position| &self.index.trees[position])
    }

    pub fn entry(&self, tree: &str, path: &str) -> Option<&StoreFile> {
        let tree = self.tree(tree)?;
        tree.files
            .binary_search_by(|file| file.path.as_bytes().cmp(path.as_bytes()))
            .ok()
            .map(|position| &tree.files[position])
    }

    /// The blob's path, after checking that `blobs/` and its fan-out directory
    /// are real directories, so a symlinked directory cannot lead a read
    /// outside the store.
    fn blob_path(&self, sha256: &str) -> Result<PathBuf, StoreError> {
        let blobs = self.root.join(BLOBS_DIR);
        let fanout = blobs.join(&sha256[..2]);
        for directory in [&blobs, &fanout] {
            match fs::symlink_metadata(directory) {
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => {
                    return Err(blob_error(
                        sha256,
                        format!("`{}` is not a directory", directory.display()),
                    ));
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    return Err(blob_error(sha256, "the blob file is missing"));
                }
                Err(e) => return Err(io_error(directory, e)),
            }
        }
        Ok(self.root.join(blob_relative(sha256)))
    }

    /// One whole file, returned only after its length and digest match the
    /// index. Refused before any decompression when the indexed length is over
    /// `max_bytes`.
    pub fn read_file(&self, tree: &str, path: &str, max_bytes: u64) -> Result<Vec<u8>, StoreError> {
        if self.tree(tree).is_none() {
            return Err(StoreError::UnknownTree(tree.into()));
        }
        let entry = self
            .entry(tree, path)
            .ok_or_else(|| StoreError::UnknownFile {
                tree: tree.into(),
                path: path.into(),
            })?;
        if entry.bytes > max_bytes {
            return Err(StoreError::Blob {
                sha256: entry.sha256.clone(),
                detail: format!(
                    "`{path}` is {} bytes, over its read bound of {max_bytes}",
                    entry.bytes
                ),
            });
        }
        let blob = self.blob_path(&entry.sha256)?;
        let mut content = Vec::with_capacity(entry.bytes.min(64 * 1024 * 1024) as usize);
        decode_blob(&blob, &entry.sha256, entry.bytes, |chunk| {
            content.extend_from_slice(chunk)
        })?;
        Ok(content)
    }

    /// Verify the whole store: the index is well formed, every referenced blob
    /// exists and decodes to its digest and length, and `blobs/` holds nothing
    /// else.
    pub fn verify(&self) -> StoreVerifyReport {
        let mut findings = Findings::default();
        let mut referenced: BTreeMap<&str, u64> = BTreeMap::new();
        let (mut files, mut uncompressed_bytes) = (0usize, 0u64);
        for tree in &self.index.trees {
            files += tree.files.len();
            for file in &tree.files {
                referenced.insert(&file.sha256, file.bytes);
                uncompressed_bytes += file.bytes;
            }
        }
        self.check_layout(&referenced, &mut findings);
        let mut stored_bytes = 0u64;
        for (sha256, bytes) in &referenced {
            let outcome = self
                .blob_path(sha256)
                .and_then(|blob| decode_blob(&blob, sha256, *bytes, |_| {}));
            match outcome {
                Ok(()) => {
                    if let Ok(metadata) = fs::metadata(self.root.join(blob_relative(sha256))) {
                        stored_bytes += metadata.len();
                    }
                }
                Err(StoreError::Blob { detail, .. }) if detail.contains("is missing") => {
                    findings.push("missing_blob", blob_relative(sha256), detail);
                }
                Err(error) => findings.push("bad_blob", blob_relative(sha256), error.to_string()),
            }
        }
        StoreVerifyReport {
            store: self.root.display().to_string(),
            status: if findings.count == 0 {
                StoreVerifyStatus::Verified
            } else {
                StoreVerifyStatus::Failed
            },
            trees: self.index.trees.len(),
            files,
            distinct_blobs: referenced.len(),
            uncompressed_bytes,
            stored_bytes,
            finding_count: findings.count,
            findings: findings.items,
        }
    }

    fn check_layout(&self, referenced: &BTreeMap<&str, u64>, findings: &mut Findings) {
        let names = |directory: &Path| -> io::Result<Vec<String>> {
            let mut names = Vec::new();
            for entry in fs::read_dir(directory)? {
                names.push(entry?.file_name().to_string_lossy().into_owned());
            }
            names.sort();
            Ok(names)
        };
        match names(&self.root) {
            Ok(top) => {
                for name in top {
                    if name != INDEX_FILE && name != BLOBS_DIR {
                        findings.push(
                            "unexpected_entry",
                            &name,
                            "only store.json and blobs/ may appear in a store".into(),
                        );
                    }
                }
            }
            Err(error) => {
                findings.push("layout", "", error.to_string());
                return;
            }
        }
        let blobs = self.root.join(BLOBS_DIR);
        match fs::symlink_metadata(&blobs) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                findings.push("layout", BLOBS_DIR, "blobs is not a directory".into());
                return;
            }
            // A missing blobs directory is reported through the missing blobs.
            Err(error) if error.kind() == io::ErrorKind::NotFound => return,
            Err(error) => {
                findings.push("layout", BLOBS_DIR, error.to_string());
                return;
            }
        }
        let fanouts = match names(&blobs) {
            Ok(fanouts) => fanouts,
            Err(error) => {
                findings.push("layout", BLOBS_DIR, error.to_string());
                return;
            }
        };
        for fanout in fanouts {
            let relative = Path::new(BLOBS_DIR).join(&fanout);
            let fanout_path = blobs.join(&fanout);
            let is_fanout_name = fanout.len() == 2
                && fanout
                    .bytes()
                    .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'));
            let is_directory = fs::symlink_metadata(&fanout_path).is_ok_and(|m| m.is_dir());
            if !is_directory || !is_fanout_name {
                findings.push(
                    "misnamed_blob",
                    &relative,
                    "blobs/ may contain only two-character lowercase hex directories".into(),
                );
                continue;
            }
            let blob_names = match names(&fanout_path) {
                Ok(blob_names) => blob_names,
                Err(error) => {
                    findings.push("layout", &relative, error.to_string());
                    continue;
                }
            };
            for name in blob_names {
                let blob_relative_path = relative.join(&name);
                let digest = name
                    .strip_suffix(".xz")
                    .filter(|d| is_lower_hex_sha256(d) && d[..2] == fanout);
                let Some(digest) = digest else {
                    findings.push(
                        "misnamed_blob",
                        &blob_relative_path,
                        "a blob must be named <h0h1>/<sha256>.xz".into(),
                    );
                    continue;
                };
                if !referenced.contains_key(digest) {
                    findings.push(
                        "unreferenced_blob",
                        &blob_relative_path,
                        "no entry in the index references this blob".into(),
                    );
                } else if !fs::symlink_metadata(fanout_path.join(&name)).is_ok_and(|m| m.is_file())
                {
                    findings.push(
                        "not_regular_blob",
                        &blob_relative_path,
                        "the blob is not a regular file".into(),
                    );
                }
            }
        }
    }
}

/// Verify a store from its path; an index that does not open is a failed
/// verification, not an error.
pub fn verify_store(root: &Path) -> StoreVerifyReport {
    match EvidenceStore::open(root) {
        Ok(store) => store.verify(),
        Err(error) => StoreVerifyReport {
            store: root.display().to_string(),
            status: StoreVerifyStatus::Failed,
            trees: 0,
            files: 0,
            distinct_blobs: 0,
            uncompressed_bytes: 0,
            stored_bytes: 0,
            finding_count: 1,
            findings: vec![StoreFinding {
                kind: "invalid_index",
                path: INDEX_FILE.into(),
                detail: error.to_string(),
            }],
        },
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StorePackReport {
    pub files: usize,
    pub distinct_blobs: usize,
    pub uncompressed_bytes: u64,
    pub distinct_bytes: u64,
    pub stored_bytes: u64,
}

fn hash_file(path: &Path) -> Result<(String, u64), StoreError> {
    let mut file = File::open(path).map_err(|e| io_error(path, e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK];
    let mut length = 0u64;
    loop {
        let count = file.read(&mut buffer).map_err(|e| io_error(path, e))?;
        if count == 0 {
            return Ok((format!("{:x}", hasher.finalize()), length));
        }
        hasher.update(&buffer[..count]);
        length += count as u64;
    }
}

/// The regular files under `root` with their identities, sorted by path.
/// Symlinks, special files and names that are not store paths are refused.
fn scan_tree(tree: &str, root: &Path) -> Result<Vec<(PathBuf, StoreFile)>, StoreError> {
    let refuse = |path: &Path, why: &str| {
        Err(StoreError::InvalidInput(format!(
            "tree `{tree}`: `{}` {why}",
            path.display()
        )))
    };
    let mut found = Vec::new();
    let mut pending = vec![(root.to_path_buf(), Vec::<String>::new())];
    while let Some((directory, prefix)) = pending.pop() {
        for entry in fs::read_dir(&directory).map_err(|e| io_error(&directory, e))? {
            let entry = entry.map_err(|e| io_error(&directory, e))?;
            let path = entry.path();
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                return refuse(&path, "has a name that is not valid UTF-8");
            };
            let metadata = fs::symlink_metadata(&path).map_err(|e| io_error(&path, e))?;
            let kind = metadata.file_type();
            if kind.is_symlink() {
                return refuse(&path, "is a symbolic link");
            }
            let mut components = prefix.clone();
            components.push(name);
            if kind.is_dir() {
                if components.len() >= MAX_PATH_COMPONENTS {
                    return refuse(&path, "is nested deeper than the format allows");
                }
                pending.push((path, components));
            } else if kind.is_file() {
                let relative = components.join("/");
                if let Err(message) = validate_store_path(&relative) {
                    return refuse(&path, &format!("is not a valid store path: {message}"));
                }
                let (sha256, bytes) = hash_file(&path)?;
                if bytes != metadata.len() {
                    return refuse(&path, "changed size while it was being read");
                }
                found.push((
                    path,
                    StoreFile {
                        path: relative,
                        sha256,
                        bytes,
                    },
                ));
            } else {
                return refuse(&path, "is not a regular file or directory");
            }
        }
    }
    found.sort_by(|a, b| a.1.path.as_bytes().cmp(b.1.path.as_bytes()));
    Ok(found)
}

fn write_blob(source: &Path, entry: &StoreFile, destination: &Path) -> Result<u64, StoreError> {
    let mut input = File::open(source).map_err(|e| io_error(source, e))?;
    let output = File::create_new(destination).map_err(|e| io_error(destination, e))?;
    let mut encoder = XzEncoder::new(io::BufWriter::new(output), XZ_PRESET);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK];
    let mut length = 0u64;
    loop {
        let count = input.read(&mut buffer).map_err(|e| io_error(source, e))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        length += count as u64;
        encoder
            .write_all(&buffer[..count])
            .map_err(|e| io_error(destination, e))?;
    }
    let mut writer = encoder.finish().map_err(|e| io_error(destination, e))?;
    writer.flush().map_err(|e| io_error(destination, e))?;
    if length != entry.bytes || format!("{:x}", hasher.finalize()) != entry.sha256 {
        return Err(StoreError::InvalidInput(format!(
            "`{}` changed while the store was being written",
            source.display()
        )));
    }
    Ok(fs::metadata(destination)
        .map_err(|e| io_error(destination, e))?
        .len())
}

/// Create a new store at `out` from named directory trees. Blobs are written
/// first and `store.json` last, so a directory without an index is an
/// unfinished store, never a valid one. On failure `out` is removed.
pub fn pack_store(out: &Path, trees: &[(String, PathBuf)]) -> Result<StorePackReport, StoreError> {
    let mut sources: Vec<&(String, PathBuf)> = trees.iter().collect();
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    for pair in sources.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(StoreError::InvalidInput(format!(
                "tree name `{}` is given more than once",
                pair[0].0
            )));
        }
    }
    for (name, _) in &sources {
        validate_tree_name(name).map_err(StoreError::InvalidInput)?;
    }
    if fs::symlink_metadata(out).is_ok() {
        return Err(StoreError::InvalidInput(format!(
            "`{}` already exists; a store is only written to a new location",
            out.display()
        )));
    }
    let mut index_trees = Vec::new();
    let mut scanned = Vec::new();
    for (name, directory) in sources {
        if !fs::metadata(directory)
            .map_err(|e| io_error(directory, e))?
            .is_dir()
        {
            return Err(StoreError::InvalidInput(format!(
                "tree `{name}`: `{}` is not a directory",
                directory.display()
            )));
        }
        let files = scan_tree(name, directory)?;
        index_trees.push(StoreTree {
            name: name.clone(),
            files: files.iter().map(|(_, entry)| entry.clone()).collect(),
        });
        scanned.push(files);
    }
    let index = StoreIndex {
        schema_version: SCHEMA_VERSION.into(),
        codec: CODEC.into(),
        trees: index_trees,
    };
    validate_index(&index).map_err(|e| StoreError::InvalidInput(e.to_string()))?;
    fs::create_dir_all(out.join(BLOBS_DIR)).map_err(|e| io_error(out, e))?;
    match write_store(out, &index, &scanned) {
        Ok(report) => Ok(report),
        Err(error) => {
            let _ = fs::remove_dir_all(out);
            Err(error)
        }
    }
}

fn write_store(
    out: &Path,
    index: &StoreIndex,
    scanned: &[Vec<(PathBuf, StoreFile)>],
) -> Result<StorePackReport, StoreError> {
    let mut done: BTreeSet<&str> = BTreeSet::new();
    let (mut files, mut uncompressed_bytes, mut distinct_bytes, mut stored_bytes) =
        (0usize, 0u64, 0u64, 0u64);
    for (source, entry) in scanned.iter().flatten() {
        files += 1;
        uncompressed_bytes += entry.bytes;
        if !done.insert(&entry.sha256) {
            continue;
        }
        let destination = out.join(blob_relative(&entry.sha256));
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
        }
        stored_bytes += write_blob(source, entry, &destination)?;
        distinct_bytes += entry.bytes;
    }
    let index_path = out.join(INDEX_FILE);
    let mut bytes =
        serde_json::to_vec_pretty(index).map_err(|e| StoreError::InvalidIndex(e.to_string()))?;
    bytes.push(b'\n');
    let mut index_file = File::create_new(&index_path).map_err(|e| io_error(&index_path, e))?;
    index_file
        .write_all(&bytes)
        .and_then(|()| index_file.sync_all())
        .map_err(|e| io_error(&index_path, e))?;
    Ok(StorePackReport {
        files,
        distinct_blobs: done.len(),
        uncompressed_bytes,
        distinct_bytes,
        stored_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn sha(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn xz(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = XzEncoder::new(Vec::new(), 1);
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn blob_file(store: &Path, sha256: &str) -> PathBuf {
        store.join(blob_relative(sha256))
    }

    const SHARED: &[u8] = b"shared content shared content shared content";

    /// Two trees sharing one file; one tree repeats a file under two paths.
    fn sources(dir: &Path) -> Vec<(String, PathBuf)> {
        let case = dir.join("src-case");
        write(&case.join("package.json"), b"{\"package\":true}\n");
        write(&case.join("expected/history.json"), SHARED);
        write(&case.join("nested/deep/file.txt"), b"deep");
        write(&case.join("empty.bin"), b"");
        let work = dir.join("src-work");
        write(&work.join("history/outputs/history.json"), SHARED);
        write(&work.join("history/inputs/history.json"), SHARED);
        write(&work.join("log.txt"), b"log line\n");
        vec![("workspace".into(), work), ("case".into(), case)]
    }

    fn packed(dir: &Path) -> PathBuf {
        let out = dir.join("store");
        pack_store(&out, &sources(dir)).unwrap();
        out
    }

    fn read_index(store: &Path) -> Value {
        serde_json::from_slice(&fs::read(store.join("store.json")).unwrap()).unwrap()
    }

    fn write_index(store: &Path, index: &Value) {
        fs::write(
            store.join("store.json"),
            serde_json::to_vec_pretty(index).unwrap(),
        )
        .unwrap();
    }

    fn index_with(trees: Value) -> Value {
        json!({"schema_version": SCHEMA_VERSION, "codec": "xz", "trees": trees})
    }

    fn file(path: &str, content: &[u8]) -> Value {
        json!({"path": path, "sha256": sha(content), "bytes": content.len()})
    }

    fn open_error(index: &Value) -> String {
        let dir = tempfile::tempdir().unwrap();
        write_index_new(dir.path(), index);
        EvidenceStore::open(dir.path()).unwrap_err().to_string()
    }

    fn write_index_new(store: &Path, index: &Value) {
        fs::create_dir_all(store).unwrap();
        write_index(store, index);
    }

    fn finding_kinds(report: &StoreVerifyReport) -> Vec<&'static str> {
        report.findings.iter().map(|f| f.kind).collect()
    }

    // Verifies: SEC-002
    #[test]
    fn pack_then_read_round_trips_every_file_and_shares_blobs() {
        let dir = tempfile::tempdir().unwrap();
        let store_dir = dir.path().join("store");
        let report = pack_store(&store_dir, &sources(dir.path())).unwrap();
        assert_eq!(report.files, 7);
        // package.json, history, deep, empty, log: five distinct contents.
        assert_eq!(report.distinct_blobs, 5);
        assert!(report.distinct_bytes < report.uncompressed_bytes);
        let store = EvidenceStore::open(&store_dir).unwrap();
        assert_eq!(store.verify().status, StoreVerifyStatus::Verified);
        assert_eq!(
            store
                .read_file("case", "expected/history.json", 1 << 20)
                .unwrap(),
            SHARED
        );
        assert_eq!(
            store
                .read_file("workspace", "history/inputs/history.json", 1 << 20)
                .unwrap(),
            SHARED
        );
        assert_eq!(store.read_file("case", "empty.bin", 1).unwrap(), b"");
        let case = store.tree("case").unwrap();
        assert_eq!(case.directory_count(), 3);
        assert_eq!(case.total_bytes(), 17 + SHARED.len() as u64 + 4);
        assert_eq!(
            store.tree("workspace").unwrap().files[0].path,
            "history/inputs/history.json"
        );
    }

    #[test]
    fn reads_refuse_unknown_trees_files_and_oversize() {
        let dir = tempfile::tempdir().unwrap();
        let store = EvidenceStore::open(&packed(dir.path())).unwrap();
        assert!(matches!(
            store.read_file("nope", "package.json", 100),
            Err(StoreError::UnknownTree(_))
        ));
        assert!(matches!(
            store.read_file("case", "nope.json", 100),
            Err(StoreError::UnknownFile { .. })
        ));
        let error = store
            .read_file("case", "expected/history.json", 4)
            .unwrap_err();
        assert!(error.to_string().contains("over its read bound"), "{error}");
    }

    #[test]
    fn the_index_must_follow_the_format_rules() {
        let a = file("a.txt", b"a");
        let cases: Vec<(Value, &str)> = vec![
            (
                index_with(json!([{"name": "t", "files": [file("/abs", b"a")]}])),
                "absolute",
            ),
            (
                index_with(json!([{"name": "t", "files": [file("a/../b", b"a")]}])),
                "component",
            ),
            (
                index_with(json!([{"name": "t", "files": [file("a//b", b"a")]}])),
                "component",
            ),
            (
                index_with(json!([{"name": "t", "files": [file("a\\b", b"a")]}])),
                "backslash",
            ),
            (
                index_with(json!([{"name": "t", "files": [file("", b"a")]}])),
                "empty",
            ),
            (
                index_with(json!([{"name": "t", "files": [file(&"d/".repeat(65), b"a")]}])),
                "components",
            ),
            (
                index_with(json!([{"name": "t", "files": [file("b", b"b"), a.clone()]}])),
                "not sorted by path",
            ),
            (
                index_with(json!([{"name": "t", "files": [a.clone(), a.clone()]}])),
                "more than once",
            ),
            (
                index_with(json!([{"name": "b", "files": []}, {"name": "a", "files": []}])),
                "not sorted by name",
            ),
            (
                index_with(json!([{"name": "a", "files": []}, {"name": "a", "files": []}])),
                "duplicate tree",
            ),
            (
                index_with(json!([{"name": "a b", "files": []}])),
                "tree name",
            ),
            (
                index_with(json!([{"name": "..", "files": []}])),
                "not allowed",
            ),
            (
                index_with(json!([{"name": "t", "files": [
                    file("a", b"a"), file("a/b", b"a")]}])),
                "also a directory prefix",
            ),
            (
                index_with(json!([{"name": "t", "files": [
                    {"path": "a", "sha256": "ABCD", "bytes": 1}]}])),
                "lowercase",
            ),
            (
                index_with(json!([{"name": "t", "files": [
                    {"path": "a", "sha256": sha(b"a"), "bytes": 1u64 << 33}]}])),
                "exceeds",
            ),
            (
                index_with(json!([
                    {"name": "t", "files": [{"path": "a", "sha256": sha(b"a"), "bytes": 1}]},
                    {"name": "u", "files": [{"path": "a", "sha256": sha(b"a"), "bytes": 2}]}])),
                "two different lengths",
            ),
            (
                json!({"schema_version": "x", "codec": "xz", "trees": []}),
                "schema_version",
            ),
            (
                json!({"schema_version": SCHEMA_VERSION, "codec": "zstd", "trees": []}),
                "codec",
            ),
            (
                json!({"schema_version": SCHEMA_VERSION, "codec": "xz", "trees": [], "extra": 1}),
                "unknown field",
            ),
        ];
        for (index, fragment) in cases {
            let message = open_error(&index);
            assert!(message.contains(fragment), "{fragment}: {message}");
        }
    }

    #[test]
    fn an_index_that_differs_from_the_recorded_digest_does_not_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = packed(dir.path());
        let recorded = sha(&fs::read(path.join("store.json")).unwrap());
        assert!(EvidenceStore::open_expecting(&path, &recorded).is_ok());
        let error = EvidenceStore::open_expecting(&path, &"0".repeat(64)).unwrap_err();
        assert!(
            error.to_string().contains("package index records"),
            "{error}"
        );
        let mut index = read_index(&path);
        index["trees"][0]["files"][0]["bytes"] = json!(1);
        write_index(&path, &index);
        assert!(EvidenceStore::open_expecting(&path, &recorded).is_err());
    }

    #[test]
    fn an_oversized_index_does_not_open() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("store.json"),
            vec![b' '; MAX_INDEX_BYTES as usize + 1],
        )
        .unwrap();
        let error = EvidenceStore::open(dir.path()).unwrap_err().to_string();
        assert!(error.contains("larger than"), "{error}");
    }

    // Verifies: SEC-002
    #[test]
    fn a_tampered_blob_is_detected_and_never_returned() {
        let dir = tempfile::tempdir().unwrap();
        let path = packed(dir.path());
        let victim = blob_file(&path, &sha(SHARED));
        let mut bytes = fs::read(&victim).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 1;
        fs::write(&victim, bytes).unwrap();
        let store = EvidenceStore::open(&path).unwrap();
        assert!(
            store
                .read_file("case", "expected/history.json", 1 << 20)
                .is_err()
        );
        let report = store.verify();
        assert_eq!(report.status, StoreVerifyStatus::Failed);
        assert_eq!(finding_kinds(&report), ["bad_blob"]);
    }

    #[test]
    fn content_that_decodes_cleanly_but_differs_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let path = packed(dir.path());
        fs::write(blob_file(&path, &sha(b"deep")), xz(b"deeq")).unwrap();
        let store = EvidenceStore::open(&path).unwrap();
        let error = store
            .read_file("case", "nested/deep/file.txt", 100)
            .unwrap_err();
        assert!(error.to_string().contains("hashes to"), "{error}");
    }

    // Verifies: SEC-002
    #[test]
    fn a_decompression_bomb_is_bounded_by_the_indexed_length() {
        let dir = tempfile::tempdir().unwrap();
        let path = packed(dir.path());
        fs::write(
            blob_file(&path, &sha(b"deep")),
            xz(&vec![0u8; 32 * 1024 * 1024]),
        )
        .unwrap();
        let store = EvidenceStore::open(&path).unwrap();
        let error = store
            .read_file("case", "nested/deep/file.txt", 100)
            .unwrap_err();
        assert!(
            error.to_string().contains("more than the 4 bytes"),
            "{error}"
        );
    }

    #[test]
    fn short_truncated_and_trailing_blobs_are_detected() {
        let dir = tempfile::tempdir().unwrap();
        let path = packed(dir.path());
        let store = EvidenceStore::open(&path).unwrap();
        let victim = blob_file(&path, &sha(b"deep"));
        let read = || {
            store
                .read_file("case", "nested/deep/file.txt", 100)
                .unwrap_err()
                .to_string()
        };

        fs::write(&victim, xz(b"de")).unwrap();
        assert!(read().contains("decompressed to 2 bytes"), "short");

        let good = xz(b"deep");
        fs::write(&victim, &good[..good.len() - 6]).unwrap();
        assert!(
            read().contains("truncated") || read().contains("decoding failed"),
            "truncated"
        );

        let mut trailing = good.clone();
        trailing.extend_from_slice(b"junk");
        fs::write(&victim, &trailing).unwrap();
        assert!(read().contains("data follows"), "trailing bytes");

        let mut second = good.clone();
        second.extend_from_slice(&xz(b""));
        fs::write(&victim, &second).unwrap();
        assert!(read().contains("data follows"), "second stream");

        fs::write(&victim, b"").unwrap();
        assert!(read().contains("truncated"), "empty");
    }

    // Verifies: SEC-002
    #[cfg(unix)]
    #[test]
    fn a_symlink_blob_or_blob_directory_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = packed(dir.path());
        let store = EvidenceStore::open(&path).unwrap();
        let victim = blob_file(&path, &sha(b"deep"));
        let outside = dir.path().join("outside.xz");
        fs::copy(&victim, &outside).unwrap();
        fs::remove_file(&victim).unwrap();
        std::os::unix::fs::symlink(&outside, &victim).unwrap();
        let error = store
            .read_file("case", "nested/deep/file.txt", 100)
            .unwrap_err();
        assert!(error.to_string().contains("not a regular file"), "{error}");
        assert_eq!(
            finding_kinds(&store.verify()),
            ["not_regular_blob", "bad_blob"]
        );

        let moved = dir.path().join("moved-blobs");
        fs::rename(path.join("blobs"), &moved).unwrap();
        std::os::unix::fs::symlink(&moved, path.join("blobs")).unwrap();
        let error = store.read_file("case", "package.json", 100).unwrap_err();
        assert!(error.to_string().contains("not a directory"), "{error}");
    }

    #[test]
    fn missing_unreferenced_misnamed_and_extra_entries_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = packed(dir.path());
        let store = EvidenceStore::open(&path).unwrap();
        assert_eq!(store.verify().status, StoreVerifyStatus::Verified);

        fs::remove_file(blob_file(&path, &sha(b"deep"))).unwrap();
        assert_eq!(finding_kinds(&store.verify()), ["missing_blob"]);
        assert!(
            store
                .read_file("case", "nested/deep/file.txt", 100)
                .unwrap_err()
                .to_string()
                .contains("is missing")
        );

        let stray = "ab".to_string() + &"0".repeat(62);
        write(&path.join(format!("blobs/ab/{stray}.xz")), &xz(b"x"));
        write(&path.join("blobs/ab/readme.txt"), b"hi");
        write(&path.join("notes.txt"), b"hi");
        let report = store.verify();
        let mut kinds = finding_kinds(&report);
        kinds.sort_unstable();
        assert_eq!(
            kinds,
            [
                "misnamed_blob",
                "missing_blob",
                "unexpected_entry",
                "unreferenced_blob"
            ]
        );
        assert_eq!(report.finding_count, 4);
    }

    #[test]
    fn verify_store_reports_an_invalid_index_as_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        write_index_new(dir.path(), &json!({"schema_version": "x"}));
        let report = verify_store(dir.path());
        assert_eq!(report.status, StoreVerifyStatus::Failed);
        assert_eq!(finding_kinds(&report), ["invalid_index"]);
    }

    #[cfg(unix)]
    #[test]
    fn pack_refuses_symlinks_special_names_existing_output_and_duplicate_trees() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("t");
        write(&tree.join("real"), b"x");
        std::os::unix::fs::symlink(tree.join("real"), tree.join("alias")).unwrap();
        let error = pack_store(&dir.path().join("s1"), &[("t".into(), tree.clone())]).unwrap_err();
        assert!(error.to_string().contains("symbolic link"), "{error}");
        assert!(!dir.path().join("s1").exists());

        let ok = dir.path().join("ok");
        write(&ok.join("a"), b"x");
        let existing = dir.path().join("existing");
        fs::create_dir(&existing).unwrap();
        assert!(pack_store(&existing, &[("t".into(), ok.clone())]).is_err());
        assert!(
            pack_store(
                &dir.path().join("s2"),
                &[("t".into(), ok.clone()), ("t".into(), ok.clone())]
            )
            .is_err()
        );
        assert!(pack_store(&dir.path().join("s3"), &[("bad name".into(), ok.clone())]).is_err());
        let backslash = dir.path().join("bs");
        write(&backslash.join("a\\b"), b"x");
        assert!(pack_store(&dir.path().join("s4"), &[("t".into(), backslash)]).is_err());
    }

    #[test]
    fn the_written_index_is_sorted_and_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let first = packed(dir.path());
        let second = dir.path().join("again");
        pack_store(&second, &sources(dir.path())).unwrap();
        assert_eq!(
            fs::read(first.join("store.json")).unwrap(),
            fs::read(second.join("store.json")).unwrap()
        );
        let index = read_index(&first);
        assert_eq!(index["trees"][0]["name"], "case");
        assert_eq!(index["trees"][1]["name"], "workspace");
    }
}
