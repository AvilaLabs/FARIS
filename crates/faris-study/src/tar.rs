//! Minimal reader for the USTAR+gzip evidence archives. It accepts regular
//! files only, with canonical relative names, within explicit bounds, and
//! creates each file exclusively. Anything else refuses the archive.

use crate::StudyError;
use flate2::read::GzDecoder;
use std::{
    fs::{File, OpenOptions},
    io::{BufReader, Read, Write},
    path::{Component, Path},
};

const MAX_FILES: usize = 4096;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 1536 * 1024 * 1024;
const MAX_NAME_BYTES: usize = 4096;
const MAX_DEPTH: usize = 64;

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

fn canonical_name(bytes: &[u8]) -> Result<String, StudyError> {
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
    if parts.len() > MAX_DEPTH
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
        let name = canonical_name(&name)?;
        let size = octal(&header[124..136]).ok_or_else(|| bad("unreadable member size"))?;
        report.files += 1;
        report.bytes = report.bytes.saturating_add(size);
        if report.files > MAX_FILES || size > MAX_FILE_BYTES || report.bytes > MAX_TOTAL_BYTES {
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
