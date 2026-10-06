//! Minimal reader for the USTAR+gzip evidence archives. It accepts regular
//! files only, with canonical relative names, within explicit bounds, and
//! creates each file exclusively. Anything else refuses the archive.
//!
//! [`extract_recorded_tree`] is the stricter reader for the recorded Core case
//! and workspace trees of a release package: it also checks every member
//! against a per-file manifest and the tar end markers.

use crate::StudyError;
use flate2::{bufread::GzDecoder as BufGzDecoder, read::GzDecoder};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    path::{Component, Path},
    sync::atomic::{AtomicBool, Ordering},
};

const MAX_FILES: usize = 4096;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 1536 * 1024 * 1024;
const MAX_NAME_BYTES: usize = 4096;
const MAX_DEPTH: usize = 64;

/// Bounds applied while unpacking. Production uses [`Limits::default`]; tests
/// lower them to exercise each bound with small archives.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub files: usize,
    pub file_bytes: u64,
    pub total_bytes: u64,
    pub depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            files: MAX_FILES,
            file_bytes: MAX_FILE_BYTES,
            total_bytes: MAX_TOTAL_BYTES,
            depth: MAX_DEPTH,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ExtractReport {
    pub files: usize,
    pub bytes: u64,
}

fn bad(message: impl Into<String>) -> StudyError {
    StudyError::Corrupt(format!("evidence archive: {}", message.into()))
}

fn field_text(field: &[u8]) -> &[u8] {
    &field[..field.iter().position(|b| *b == 0).unwrap_or(field.len())]
}

fn octal(field: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(field_text(field)).ok()?.trim();
    if text.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(text, 8).ok()
}

fn canonical_name(bytes: &[u8], max_depth: usize) -> Result<String, StudyError> {
    let name = std::str::from_utf8(bytes).map_err(|_| bad("member name is not UTF-8"))?;
    if name.is_empty()
        || name.len() > MAX_NAME_BYTES
        || name.contains('\\')
        || name.contains('\0')
        || name.starts_with('/')
    {
        return Err(bad(format!("noncanonical member name {name:?}")));
    }
    let parts: Vec<_> = Path::new(name).components().collect();
    if parts.len() > max_depth
        || parts.iter().any(|c| !matches!(c, Component::Normal(_)))
        || name
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err(bad(format!("noncanonical member name {name:?}")));
    }
    Ok(name.to_owned())
}

/// Extract `archive` (a single-member gzip of a USTAR tar of regular files)
/// into `destination`, which must exist.
pub fn extract_tar_gz(archive: &Path, destination: &Path) -> Result<ExtractReport, StudyError> {
    extract_with_limits(archive, destination, Limits::default())
}

pub(crate) fn extract_with_limits(
    archive: &Path,
    destination: &Path,
    limits: Limits,
) -> Result<ExtractReport, StudyError> {
    let file = File::open(archive)
        .map_err(|e| StudyError::io(format!("cannot open {}", archive.display()), e))?;
    let mut input = GzDecoder::new(BufReader::new(file));
    let mut report = ExtractReport::default();
    let mut header = [0u8; 512];
    let read_error = |e: std::io::Error| bad(format!("cannot decompress: {e}"));
    loop {
        input.read_exact(&mut header).map_err(read_error)?;
        if header.iter().all(|b| *b == 0) {
            break;
        }
        let stored = octal(&header[148..156]).ok_or_else(|| bad("unreadable header checksum"))?;
        let computed: u64 = header
            .iter()
            .enumerate()
            .map(|(i, b)| {
                if (148..156).contains(&i) {
                    32
                } else {
                    u64::from(*b)
                }
            })
            .sum();
        if stored != computed {
            return Err(bad("header checksum mismatch"));
        }
        if !matches!(header[156], b'0' | 0) {
            return Err(bad("only regular files are accepted"));
        }
        let mut name = field_text(&header[345..500]).to_vec();
        if &header[257..262] == b"ustar" && !name.is_empty() {
            name.push(b'/');
        } else {
            name.clear();
        }
        name.extend_from_slice(field_text(&header[0..100]));
        let name = canonical_name(&name, limits.depth)?;
        let size = octal(&header[124..136]).ok_or_else(|| bad("unreadable member size"))?;
        report.files += 1;
        report.bytes = report.bytes.saturating_add(size);
        if report.files > limits.files
            || size > limits.file_bytes
            || report.bytes > limits.total_bytes
        {
            return Err(bad("exceeds the file-count or size bounds"));
        }
        let path = destination.join(&name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| StudyError::io(format!("cannot create {}", parent.display()), e))?;
        }
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| StudyError::io(format!("cannot create {}", path.display()), e))?;
        let copied =
            std::io::copy(&mut (&mut input).take(size), &mut output).map_err(read_error)?;
        if copied != size {
            return Err(bad("truncated member"));
        }
        output
            .flush()
            .map_err(|e| StudyError::io(format!("cannot write {}", path.display()), e))?;
        let padding = (512 - size % 512) % 512;
        std::io::copy(&mut (&mut input).take(padding), &mut std::io::sink()).map_err(read_error)?;
    }
    Ok(report)
}

/// Schema of the per-tree manifest that travels with each recorded archive.
const TREE_SCHEMA: &str = "faris-recorded-tree-archive/v0.1";
const TREE_MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
const TREE_MAX_MEMBER_BYTES: u64 = 64 * 1024 * 1024;
const TREE_MAX_BYTES: u64 = 512 * 1024 * 1024;
const TREE_MAX_FILES: usize = 2048;
const TREE_MAX_MEMBERS: usize = 4096;
const TREE_MAX_DIRECTORIES: usize = 1024;
const TREE_MAX_COMPONENTS: usize = 64;
/// Longest accepted run of zero bytes after the last member.
const TRAILER_LIMIT: u64 = TREE_MAX_MEMBERS as u64 * 1024 + 12288;
const COPY_CHUNK: usize = 1 << 20;

#[derive(Deserialize)]
struct TreeRecord {
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(Deserialize)]
struct TreeManifest {
    schema_version: String,
    archive_sha256: String,
    archive_bytes: u64,
    expanded_bytes: u64,
    file_count: u64,
    archive_member_count: u64,
    directory_count: u64,
    members: Vec<TreeRecord>,
}

/// The identities and totals a recorded tree archive declared and met.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedTreeManifest {
    /// `sha256:<hex>` of the compressed archive.
    pub archive_sha256: String,
    pub archive_bytes: u64,
    pub expanded_bytes: u64,
    pub file_count: u64,
    pub archive_member_count: u64,
    pub directory_count: u64,
}

fn tree_name(name: &str) -> Result<(), StudyError> {
    canonical_name(name.as_bytes(), TREE_MAX_COMPONENTS)?;
    if name.split('/').next().is_some_and(|c| c.contains(':')) {
        return Err(bad(format!("noncanonical member name {name:?}")));
    }
    Ok(())
}

/// Every proper ancestor directory of `name`.
fn ancestors(name: &str) -> impl Iterator<Item = &str> {
    name.match_indices('/').map(|(i, _)| &name[..i])
}

fn digest_text(text: &str) -> bool {
    text.strip_prefix("sha256:")
        .is_some_and(crate::is_sha256_hex)
}

fn read_tree_manifest(manifest: &Path) -> Result<TreeManifest, StudyError> {
    let length = std::fs::metadata(manifest)
        .map_err(|e| StudyError::io(format!("cannot read {}", manifest.display()), e))?
        .len();
    if length > TREE_MAX_ARCHIVE_BYTES {
        return Err(bad("manifest exceeds 64 MiB"));
    }
    let bytes = std::fs::read(manifest)
        .map_err(|e| StudyError::io(format!("cannot read {}", manifest.display()), e))?;
    serde_json::from_slice(&bytes).map_err(|e| bad(format!("manifest is not valid: {e}")))
}

/// The manifest's own consistency: bounds, canonical unique names, no file that
/// is also a directory, and totals equal to the inventory. Returns the
/// directories its files imply.
fn validate_tree_manifest(manifest: &TreeManifest) -> Result<BTreeSet<String>, StudyError> {
    if manifest.schema_version != TREE_SCHEMA {
        return Err(bad("unsupported recorded tree archive manifest"));
    }
    if !digest_text(&manifest.archive_sha256) {
        return Err(bad("malformed archive digest in manifest"));
    }
    if manifest.members.is_empty() || manifest.members.len() > TREE_MAX_FILES {
        return Err(bad("member manifest is empty or over its bound"));
    }
    let mut files = BTreeSet::new();
    let mut directories = BTreeSet::new();
    let mut total = 0u64;
    for record in &manifest.members {
        tree_name(&record.path)?;
        if record.bytes > TREE_MAX_MEMBER_BYTES
            || !digest_text(&record.sha256)
            || !files.insert(record.path.clone())
        {
            return Err(bad(format!(
                "malformed or duplicate manifest member {}",
                record.path
            )));
        }
        directories.extend(ancestors(&record.path).map(str::to_owned));
        total += record.bytes;
    }
    if let Some(clash) = directories.iter().find(|d| files.contains(*d)) {
        return Err(bad(format!(
            "manifest has a file/directory prefix conflict at {clash}"
        )));
    }
    if total > TREE_MAX_BYTES
        || directories.len() > TREE_MAX_DIRECTORIES
        || manifest.directory_count != directories.len() as u64
        || manifest.expanded_bytes != total
        || manifest.file_count != files.len() as u64
        || manifest.archive_member_count < manifest.file_count
        || manifest.archive_member_count > TREE_MAX_MEMBERS as u64
    {
        return Err(bad("manifest totals do not match its bounded inventory"));
    }
    Ok(directories)
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), StudyError> {
    if cancel.load(Ordering::Relaxed) {
        Err(StudyError::Cancelled)
    } else {
        Ok(())
    }
}

fn private_directory(recursive: bool) -> std::fs::DirBuilder {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(recursive);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
}

fn make_directory(path: &Path) -> Result<(), StudyError> {
    private_directory(true)
        .create(path)
        .map_err(|e| StudyError::io(format!("cannot create {}", path.display()), e))
}

/// Extract a recorded Core case or workspace tree into the fresh `destination`.
///
/// `manifest` is the tree's `faris-recorded-tree-archive/v0.1` file. The archive
/// must match the manifest's hash and size, be exactly one gzip member with no
/// trailing bytes, and hold only USTAR regular files and zero-size directory
/// entries whose sizes and SHA-256 equal the manifest records. After the last
/// member there must be at least two zero blocks and nothing else, up to
/// `TRAILER_LIMIT` bytes. Files are created exclusively and read-only. The
/// verification and extraction share one pass; `destination` is removed on any
/// failure, including cancellation, which is checked between blocks.
pub fn extract_recorded_tree(
    archive: &Path,
    manifest: &Path,
    destination: &Path,
    cancel: &AtomicBool,
) -> Result<RecordedTreeManifest, StudyError> {
    check_cancel(cancel)?;
    if destination.exists() {
        return Err(StudyError::Input(format!(
            "archive extraction target must be new: {}",
            destination.display()
        )));
    }
    let tree = read_tree_manifest(manifest)?;
    let directories = validate_tree_manifest(&tree)?;
    let (found, size) = crate::sha256_file(archive)
        .map_err(|e| StudyError::io(format!("cannot read {}", archive.display()), e))?;
    if size > TREE_MAX_ARCHIVE_BYTES {
        return Err(bad("compressed archive exceeds 64 MiB"));
    }
    if tree.archive_sha256 != format!("sha256:{found}") || tree.archive_bytes != size {
        return Err(bad(
            "compressed archive hash or size differs from its manifest",
        ));
    }
    if let Some(parent) = destination.parent() {
        make_directory(parent)?;
    }
    private_directory(false)
        .create(destination)
        .map_err(|e| StudyError::io(format!("cannot create {}", destination.display()), e))?;
    match extract_verified(archive, &tree, &directories, destination, cancel) {
        Ok(()) => Ok(RecordedTreeManifest {
            archive_sha256: tree.archive_sha256,
            archive_bytes: tree.archive_bytes,
            expanded_bytes: tree.expanded_bytes,
            file_count: tree.file_count,
            archive_member_count: tree.archive_member_count,
            directory_count: tree.directory_count,
        }),
        Err(error) => {
            let _ = std::fs::remove_dir_all(destination);
            Err(error)
        }
    }
}

fn extract_verified(
    archive: &Path,
    tree: &TreeManifest,
    directories: &BTreeSet<String>,
    destination: &Path,
    cancel: &AtomicBool,
) -> Result<(), StudyError> {
    let expected: BTreeMap<&str, &TreeRecord> =
        tree.members.iter().map(|r| (r.path.as_str(), r)).collect();
    let file = File::open(archive)
        .map_err(|e| StudyError::io(format!("cannot open {}", archive.display()), e))?;
    // The bufread decoder stops after the first gzip member; whatever follows
    // it stays in the BufReader, where it is checked below.
    let mut input = BufGzDecoder::new(BufReader::new(file));
    let read_error = |e: std::io::Error| bad(format!("cannot decompress: {e}"));
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut members = 0u64;
    let mut expanded = 0u64;
    let mut header = [0u8; 512];
    let mut chunk = vec![0u8; COPY_CHUNK];
    loop {
        check_cancel(cancel)?;
        input
            .read_exact(&mut header)
            .map_err(|e| bad(format!("tar archive ends without end markers: {e}")))?;
        if header.iter().all(|b| *b == 0) {
            break;
        }
        let stored = octal(&header[148..156]).ok_or_else(|| bad("unreadable header checksum"))?;
        let computed: u64 = header
            .iter()
            .enumerate()
            .map(|(i, b)| {
                if (148..156).contains(&i) {
                    32
                } else {
                    u64::from(*b)
                }
            })
            .sum();
        if stored != computed {
            return Err(bad("header checksum mismatch"));
        }
        if &header[257..262] != b"ustar" {
            return Err(bad("member is not a USTAR header"));
        }
        let is_directory = match header[156] {
            b'0' | 0 => false,
            b'5' => true,
            _ => return Err(bad("only regular files and directories are accepted")),
        };
        let mut name = field_text(&header[345..500]).to_vec();
        if !name.is_empty() {
            name.push(b'/');
        }
        name.extend_from_slice(field_text(&header[0..100]));
        let name = std::str::from_utf8(&name).map_err(|_| bad("member name is not UTF-8"))?;
        // Directory entries conventionally end in a slash; no other name may.
        let name = if is_directory {
            name.strip_suffix('/').unwrap_or(name)
        } else {
            name
        }
        .to_owned();
        tree_name(&name)?;
        let size = octal(&header[124..136]).ok_or_else(|| bad("unreadable member size"))?;
        members += 1;
        if members > tree.archive_member_count {
            return Err(bad("archive has more members than its manifest declares"));
        }
        if !seen.insert(name.clone()) {
            return Err(bad(format!("archive repeats a path: {name}")));
        }
        let path = destination.join(&name);
        if is_directory {
            if size != 0 {
                return Err(bad(format!("archive directory has a nonzero size: {name}")));
            }
            if !directories.contains(&name) {
                return Err(bad(format!(
                    "archive has an unneeded directory entry: {name}"
                )));
            }
            make_directory(&path)?;
            continue;
        }
        let record = expected
            .get(name.as_str())
            .ok_or_else(|| bad(format!("archive member is not in its manifest: {name}")))?;
        if size != record.bytes {
            return Err(bad(format!(
                "archive member size differs from manifest: {name}"
            )));
        }
        expanded += size;
        if expanded > tree.expanded_bytes {
            return Err(bad("archive members exceed the declared expanded size"));
        }
        if let Some(parent) = path.parent() {
            make_directory(parent)?;
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o444);
        let mut output = options
            .open(&path)
            .map_err(|e| StudyError::io(format!("cannot create {}", path.display()), e))?;
        let mut digest = Sha256::new();
        let mut remaining = size;
        while remaining > 0 {
            check_cancel(cancel)?;
            let want = remaining.min(COPY_CHUNK as u64) as usize;
            input.read_exact(&mut chunk[..want]).map_err(read_error)?;
            digest.update(&chunk[..want]);
            output
                .write_all(&chunk[..want])
                .map_err(|e| StudyError::io(format!("cannot write {}", path.display()), e))?;
            remaining -= want as u64;
        }
        output
            .flush()
            .map_err(|e| StudyError::io(format!("cannot write {}", path.display()), e))?;
        drop(output);
        #[cfg(not(unix))]
        {
            let mut permissions = std::fs::metadata(&path)
                .map_err(|e| StudyError::io(format!("cannot read {}", path.display()), e))?
                .permissions();
            permissions.set_readonly(true);
            std::fs::set_permissions(&path, permissions)
                .map_err(|e| StudyError::io(format!("cannot protect {}", path.display()), e))?;
        }
        if format!("sha256:{:x}", digest.finalize()) != record.sha256 {
            return Err(bad(format!("archive member hash mismatch: {name}")));
        }
        let padding = (512 - size % 512) % 512;
        std::io::copy(&mut (&mut input).take(padding), &mut std::io::sink()).map_err(read_error)?;
    }
    // The zero block just read is the first end marker. Everything after the
    // last member's padded data must be zero blocks, at least two in all.
    let mut trailer = 512u64;
    loop {
        check_cancel(cancel)?;
        let n = input.read(&mut chunk).map_err(read_error)?;
        if n == 0 {
            break;
        }
        trailer += n as u64;
        if trailer > TRAILER_LIMIT {
            return Err(bad("tar archive has excessive trailing padding"));
        }
        if chunk[..n].iter().any(|b| *b != 0) {
            return Err(bad(
                "tar archive has nonzero data after its first end marker",
            ));
        }
    }
    if trailer < 1024 || !trailer.is_multiple_of(512) {
        return Err(bad("tar archive has invalid end markers"));
    }
    let rest = input
        .get_mut()
        .fill_buf()
        .map_err(|e| bad(format!("cannot read past the gzip member: {e}")))?;
    if !rest.is_empty() {
        return Err(bad(
            "archive has concatenated gzip members or trailing bytes",
        ));
    }
    if members != tree.archive_member_count
        || expected.keys().any(|name| !seen.contains(*name))
        || expanded != tree.expanded_bytes
    {
        return Err(bad(
            "archive extraction did not match the declared inventory",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod recorded_tree_tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use std::sync::atomic::AtomicBool;

    fn header(name: &str, size: usize, kind: u8) -> [u8; 512] {
        let mut block = [0u8; 512];
        block[..name.len()].copy_from_slice(name.as_bytes());
        block[100..107].copy_from_slice(b"0000444");
        block[124..135].copy_from_slice(format!("{size:011o}").as_bytes());
        block[136..147].copy_from_slice(b"00000000000");
        block[148..156].copy_from_slice(b"        ");
        block[156] = kind;
        block[257..263].copy_from_slice(b"ustar\0");
        block[263..265].copy_from_slice(b"00");
        let sum: u64 = block.iter().map(|b| u64::from(*b)).sum();
        block[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
        block[155] = b' ';
        block
    }

    /// Tar bytes for `entries` (a `/`-ending name is a directory), without end markers.
    fn tar_body(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, data) in entries {
            let directory = name.ends_with('/');
            out.extend_from_slice(&header(
                name,
                data.len(),
                if directory { b'5' } else { b'0' },
            ));
            out.extend_from_slice(data);
            out.resize(out.len().div_ceil(512) * 512, 0);
        }
        out
    }

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    const FILES: [(&str, &[u8]); 3] = [
        ("a.txt", b"alpha"),
        ("sub/b.bin", &[7u8; 1500]),
        ("sub/deep/c.txt", b""),
    ];

    struct Fixture {
        dir: tempfile::TempDir,
    }

    impl Fixture {
        /// Write `gz` as the archive and a manifest for `FILES`; `edit` may
        /// change the manifest before it is written.
        fn new(gz: &[u8], edit: impl FnOnce(&mut serde_json::Value)) -> Self {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("t.tar.gz"), gz).unwrap();
            let members: Vec<_> = FILES
                .iter()
                .map(|(path, data)| {
                    serde_json::json!({
                        "path": path,
                        "bytes": data.len(),
                        "sha256": format!("sha256:{}", crate::sha256_hex(data)),
                    })
                })
                .collect();
            let mut manifest = serde_json::json!({
                "schema_version": TREE_SCHEMA,
                "archive_sha256": format!("sha256:{}", crate::sha256_hex(gz)),
                "archive_bytes": gz.len(),
                "expanded_bytes": FILES.iter().map(|f| f.1.len()).sum::<usize>(),
                "file_count": FILES.len(),
                "archive_member_count": FILES.len(),
                "directory_count": 2,
                "members": members,
            });
            edit(&mut manifest);
            std::fs::write(dir.path().join("t.manifest.json"), manifest.to_string()).unwrap();
            Self { dir }
        }

        fn extract(&self, cancel: &AtomicBool) -> Result<RecordedTreeManifest, StudyError> {
            extract_recorded_tree(
                &self.dir.path().join("t.tar.gz"),
                &self.dir.path().join("t.manifest.json"),
                &self.destination(),
                cancel,
            )
        }

        fn destination(&self) -> std::path::PathBuf {
            self.dir.path().join("out").join("tree")
        }
    }

    fn valid_tar() -> Vec<u8> {
        let mut tar = tar_body(&FILES);
        tar.extend_from_slice(&[0u8; 1024]);
        tar
    }

    fn refused(fixture: &Fixture, needle: &str) {
        let error = fixture.extract(&AtomicBool::new(false)).unwrap_err();
        assert!(error.to_string().contains(needle), "{error}");
        assert!(!fixture.destination().exists(), "destination kept: {error}");
    }

    #[test]
    fn extracts_a_valid_archive_read_only() {
        let fixture = Fixture::new(&gzip(&valid_tar()), |_| {});
        let report = fixture.extract(&AtomicBool::new(false)).unwrap();
        assert_eq!(report.file_count, 3);
        assert_eq!(report.expanded_bytes, 1505);
        let root = fixture.destination();
        assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"alpha");
        assert_eq!(std::fs::read(root.join("sub/b.bin")).unwrap().len(), 1500);
        assert!(root.join("sub/deep/c.txt").is_file());
        assert!(
            std::fs::metadata(root.join("a.txt"))
                .unwrap()
                .permissions()
                .readonly()
        );
    }

    #[test]
    fn accepts_directory_entries_and_longer_zero_trailers() {
        let mut tar = tar_body(&[
            ("sub/", b""),
            ("a.txt", b"alpha"),
            ("sub/b.bin", &[7u8; 1500]),
            ("sub/deep/c.txt", b""),
        ]);
        tar.extend_from_slice(&vec![0u8; 512 * 20]);
        let fixture = Fixture::new(&gzip(&tar), |m| m["archive_member_count"] = 4.into());
        assert!(fixture.extract(&AtomicBool::new(false)).is_ok());
    }

    #[test]
    fn refuses_a_garbage_block_before_the_end_markers() {
        let mut tar = tar_body(&FILES);
        tar.extend_from_slice(&[0x41u8; 512]);
        tar.extend_from_slice(&[0u8; 1024]);
        refused(&Fixture::new(&gzip(&tar), |_| {}), "checksum");
    }

    #[test]
    fn refuses_nonzero_data_after_the_first_end_marker() {
        let mut tar = tar_body(&FILES);
        tar.extend_from_slice(&[0u8; 512]);
        tar.extend_from_slice(&[0x41u8; 512]);
        refused(&Fixture::new(&gzip(&tar), |_| {}), "nonzero");
    }

    #[test]
    fn refuses_a_single_zero_block() {
        let mut tar = tar_body(&FILES);
        tar.extend_from_slice(&[0u8; 512]);
        refused(&Fixture::new(&gzip(&tar), |_| {}), "end markers");
    }

    #[test]
    fn refuses_a_trailer_that_is_not_whole_blocks() {
        let mut tar = tar_body(&FILES);
        tar.extend_from_slice(&[0u8; 1024 + 100]);
        refused(&Fixture::new(&gzip(&tar), |_| {}), "end markers");
    }

    #[test]
    fn refuses_an_excessive_zero_trailer() {
        let mut tar = tar_body(&FILES);
        tar.resize(tar.len() + TRAILER_LIMIT as usize + 512, 0);
        refused(&Fixture::new(&gzip(&tar), |_| {}), "excessive");
    }

    #[test]
    fn refuses_a_second_gzip_member() {
        let mut gz = gzip(&valid_tar());
        gz.extend_from_slice(&gzip(b"more"));
        refused(&Fixture::new(&gz, |_| {}), "trailing");
    }

    #[test]
    fn refuses_trailing_bytes_after_the_gzip_member() {
        let mut gz = gzip(&valid_tar());
        gz.extend_from_slice(b"junk");
        refused(&Fixture::new(&gz, |_| {}), "trailing");
    }

    #[test]
    fn refuses_a_member_hash_mismatch() {
        let fixture = Fixture::new(&gzip(&valid_tar()), |m| {
            m["members"][0]["sha256"] = format!("sha256:{}", crate::sha256_hex(b"other")).into();
        });
        refused(&fixture, "hash mismatch");
    }

    #[test]
    fn refuses_a_member_size_mismatch() {
        let fixture = Fixture::new(&gzip(&valid_tar()), |m| {
            m["members"][0]["bytes"] = 6.into();
            m["expanded_bytes"] = 1506.into();
        });
        refused(&fixture, "size differs");
    }

    #[test]
    fn refuses_a_member_the_manifest_does_not_list() {
        let mut tar = tar_body(&FILES);
        tar.extend_from_slice(&tar_body(&[("extra.txt", b"x")]));
        tar.extend_from_slice(&[0u8; 1024]);
        let fixture = Fixture::new(&gzip(&tar), |m| m["archive_member_count"] = 4.into());
        refused(&fixture, "not in its manifest");
    }

    #[test]
    fn refuses_a_listed_member_that_is_missing() {
        let mut tar = tar_body(&FILES[..2]);
        tar.extend_from_slice(&[0u8; 1024]);
        refused(&Fixture::new(&gzip(&tar), |_| {}), "inventory");
    }

    #[test]
    fn refuses_a_repeated_path() {
        let mut tar = tar_body(&FILES);
        tar.extend_from_slice(&tar_body(&FILES[..1]));
        tar.extend_from_slice(&[0u8; 1024]);
        let fixture = Fixture::new(&gzip(&tar), |m| m["archive_member_count"] = 4.into());
        refused(&fixture, "repeats");
    }

    #[test]
    fn refuses_links_and_noncanonical_names() {
        let mut tar = Vec::new();
        tar.extend_from_slice(&header("a.txt", 0, b'2'));
        tar.extend_from_slice(&[0u8; 1024]);
        refused(&Fixture::new(&gzip(&tar), |_| {}), "regular files");
        let mut tar = tar_body(&[("../a.txt", b"alpha")]);
        tar.extend_from_slice(&[0u8; 1024]);
        refused(&Fixture::new(&gzip(&tar), |_| {}), "noncanonical");
    }

    #[test]
    fn refuses_a_wrong_archive_hash_or_manifest_schema() {
        let gz = gzip(&valid_tar());
        let fixture = Fixture::new(&gz, |m| {
            m["archive_sha256"] = format!("sha256:{}", crate::sha256_hex(b"x")).into();
        });
        refused(&fixture, "hash or size");
        let fixture = Fixture::new(&gz, |m| m["schema_version"] = "other/v1".into());
        refused(&fixture, "unsupported");
    }

    #[test]
    fn refuses_a_destination_that_already_exists() {
        let fixture = Fixture::new(&gzip(&valid_tar()), |_| {});
        std::fs::create_dir_all(fixture.destination()).unwrap();
        let error = fixture.extract(&AtomicBool::new(false)).unwrap_err();
        assert!(error.to_string().contains("must be new"));
        assert!(fixture.destination().exists());
    }

    #[test]
    fn cancellation_stops_and_removes_the_destination() {
        let fixture = Fixture::new(&gzip(&valid_tar()), |_| {});
        let error = fixture.extract(&AtomicBool::new(true)).unwrap_err();
        assert!(matches!(error, StudyError::Cancelled));
        assert!(!fixture.destination().exists());
        // Cancelled after the destination exists: the loop checks between blocks.
        let late = Fixture::new(&gzip(&valid_tar()), |_| {});
        let cancel = AtomicBool::new(false);
        let archive = late.dir.path().join("t.tar.gz");
        let manifest = late.dir.path().join("t.manifest.json");
        let tree = read_tree_manifest(&manifest).unwrap();
        let directories = validate_tree_manifest(&tree).unwrap();
        std::fs::create_dir_all(late.destination()).unwrap();
        cancel.store(true, Ordering::Relaxed);
        let error = extract_verified(&archive, &tree, &directories, &late.destination(), &cancel)
            .unwrap_err();
        assert!(matches!(error, StudyError::Cancelled));
    }

    #[test]
    fn removes_the_destination_after_a_late_failure() {
        // The first file extracts, then the second fails its hash.
        let fixture = Fixture::new(&gzip(&valid_tar()), |m| {
            m["members"][1]["sha256"] = format!("sha256:{}", crate::sha256_hex(b"z")).into();
        });
        refused(&fixture, "hash mismatch");
    }
}
