use crate::{
    ArchiveKind, ArrangementRecord, BundleRecord, ENCODING_VERBATIM, EvidenceArchive,
    EvidenceLayer, EvidenceMode, MIMETYPE, Manifest, StudyError, is_safe_file_name,
    is_safe_relative_path, is_sha256_hex,
    preview::{MAX_PREVIEW_BYTES_USED, MAX_PREVIEW_SIDE, Preview, PreviewStatus, png_dimensions},
    sha256_hex,
};
use faris_engine::core_evidence::RecordedTransportBundle;
use faris_engine::history_ensemble::HistoryEnsemble;
use faris_engine::history_uncertainty::EnsembleKey;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};
use zip::{CompressionMethod, ZipArchive};

/// Total declared decompressed bytes accepted in one file.
const MAX_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Largest single blob accepted (a recorded bundle is at most tens of MiB).
const MAX_BLOB_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PREVIEW_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;

/// Entry names straight from the central directory. The zip library keeps only
/// the last of two entries with one name, so duplicates must be found here.
fn central_directory_names(file: &mut File, file_bytes: u64) -> Result<Vec<Vec<u8>>, StudyError> {
    use std::io::{Seek, SeekFrom};
    let damaged = || StudyError::corrupt("the study file's central directory is damaged");
    let tail_len = file_bytes.min(65_557);
    file.seek(SeekFrom::Start(file_bytes - tail_len))
        .map_err(|e| StudyError::io("reading study file", e))?;
    let mut tail = vec![0u8; tail_len as usize];
    file.read_exact(&mut tail)
        .map_err(|e| StudyError::io("reading study file", e))?;
    let eocd = tail
        .windows(4)
        .rposition(|w| w == b"PK\x05\x06")
        .ok_or_else(damaged)?;
    let record = &tail[eocd..];
    if record.len() < 22 {
        return Err(damaged());
    }
    let u16_at = |i: usize| u16::from_le_bytes([record[i], record[i + 1]]) as usize;
    let u32_at = |i: usize| u32::from_le_bytes(record[i..i + 4].try_into().expect("4 bytes"));
    let (count, size, offset) = (u16_at(10), u32_at(12) as u64, u32_at(16) as u64);
    if count == 0xFFFF || size == 0xFFFF_FFFF || offset == 0xFFFF_FFFF {
        return Err(StudyError::corrupt(
            "zip64 containers are not part of study-file format 1",
        ));
    }
    if size > MAX_MANIFEST_BYTES || offset.checked_add(size).is_none_or(|e| e > file_bytes) {
        return Err(damaged());
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| StudyError::io("reading study file", e))?;
    let mut directory = vec![0u8; size as usize];
    file.read_exact(&mut directory)
        .map_err(|e| StudyError::io("reading study file", e))?;
    let mut names = Vec::with_capacity(count);
    let mut at = 0usize;
    while at < directory.len() {
        let header = directory.get(at..at + 46).ok_or_else(damaged)?;
        if &header[..4] != b"PK\x01\x02" {
            return Err(damaged());
        }
        let field = |i: usize| u16::from_le_bytes([header[i], header[i + 1]]) as usize;
        let (name, extra, comment) = (field(28), field(30), field(32));
        names.push(
            directory
                .get(at + 46..at + 46 + name)
                .ok_or_else(damaged)?
                .to_vec(),
        );
        at += 46 + name + extra + comment;
    }
    if names.len() != count {
        return Err(damaged());
    }
    Ok(names)
}

/// SHA-256 (lowercase hex) and length of a file, streamed.
pub fn sha256_file(path: &Path) -> std::io::Result<(String, u64)> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
        total += n as u64;
    }
    Ok((format!("{:x}", digest.finalize()), total))
}

/// A history ensemble read back from a study file with the key it was stored
/// under.
#[derive(Clone, Debug)]
pub struct StoredEnsemble {
    pub scenario_sha256: String,
    pub variant: String,
    pub key: EnsembleKey,
    pub ensemble: HistoryEnsemble,
}

#[derive(Clone, Debug)]
pub struct BlobInfo {
    pub sha256: String,
    pub bytes: u64,
    pub media_type: String,
    /// Size inside the container.
    pub stored_bytes: u64,
    pub compressed: bool,
}

pub struct StudyReader {
    archive: ZipArchive<File>,
    pub manifest: Manifest,
    /// Blob digest to (zip entry index, recorded size).
    entries: BTreeMap<String, (usize, u64)>,
    pub file_bytes: u64,
    /// Zip entry index of `preview.png`, if the file has one.
    preview: Option<usize>,
}

fn blob_entry_name(name: &str) -> Option<&str> {
    name.strip_prefix("blobs/").filter(|h| is_sha256_hex(h))
}

/// Major version of "faris-study/<major>[.<minor>...]", if the text has that shape.
fn major_version(format: &str) -> Option<&str> {
    let version = format.strip_prefix("faris-study/")?;
    Some(version.split('.').next().unwrap_or(version))
}

impl StudyReader {
    /// Open and structurally check a study file. Blob contents are verified
    /// when read (`read_blob`) or all at once (`verify`).
    pub fn open(path: &Path) -> Result<Self, StudyError> {
        let context = || format!("cannot open {}", path.display());
        let file = File::open(path).map_err(|e| StudyError::io(context(), e))?;
        let file_bytes = file
            .metadata()
            .map_err(|e| StudyError::io(context(), e))?
            .len();
        let mut file = file;
        let directory_names = central_directory_names(&mut file, file_bytes)?;
        let mut archive = ZipArchive::new(file)?;
        if archive.len() > MAX_ENTRIES || directory_names.len() > MAX_ENTRIES {
            return Err(StudyError::corrupt("study file has too many entries"));
        }
        let distinct: BTreeSet<_> = directory_names.iter().collect();
        if distinct.len() != directory_names.len() || archive.len() != directory_names.len() {
            return Err(StudyError::corrupt("the study file repeats an entry name"));
        }

        // Entry names: only the expected ones, each once, mimetype first.
        let mut names = BTreeSet::new();
        let mut blob_indices: BTreeMap<String, usize> = BTreeMap::new();
        let mut manifest_index = None;
        let mut preview_index = None;
        for index in 0..archive.len() {
            let entry = archive.by_index_raw(index)?;
            let name = entry.name().to_owned();
            if !names.insert(name.clone()) {
                return Err(StudyError::corrupt(format!(
                    "duplicate entry \"{name}\" in the study file"
                )));
            }
            if index == 0 {
                if name != "mimetype" {
                    return Err(StudyError::corrupt(
                        "the first entry of a study file must be \"mimetype\"",
                    ));
                }
                if entry.compression() != CompressionMethod::Stored {
                    return Err(StudyError::corrupt("the mimetype entry must be stored"));
                }
            } else if name == "manifest.json" {
                manifest_index = Some(index);
            } else if name == "preview.png" {
                if entry.size() > MAX_PREVIEW_BYTES {
                    return Err(StudyError::corrupt("preview.png is too large"));
                }
                preview_index = Some(index);
            } else if let Some(hash) = blob_entry_name(&name) {
                blob_indices.insert(hash.to_owned(), index);
            } else {
                return Err(StudyError::corrupt(format!(
                    "unexpected entry name \"{}\" in the study file",
                    name.escape_debug()
                )));
            }
        }
        {
            let mut entry = archive.by_index(0)?;
            let mut text = Vec::new();
            (&mut entry)
                .take(MIMETYPE.len() as u64 + 1)
                .read_to_end(&mut text)
                .map_err(|e| StudyError::corrupt(format!("cannot read mimetype: {e}")))?;
            if text != MIMETYPE.as_bytes() {
                return Err(StudyError::corrupt(
                    "this is not a FARIS study file (wrong mimetype)",
                ));
            }
        }
        let manifest_index =
            manifest_index.ok_or_else(|| StudyError::corrupt("the study file has no manifest"))?;
        let manifest_bytes = {
            let mut entry = archive.by_index(manifest_index)?;
            let mut bytes = Vec::new();
            (&mut entry)
                .take(MAX_MANIFEST_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| StudyError::corrupt(format!("cannot read manifest: {e}")))?;
            if bytes.len() as u64 > MAX_MANIFEST_BYTES {
                return Err(StudyError::corrupt("manifest is too large"));
            }
            bytes
        };
        let value: serde_json::Value = serde_json::from_slice(&manifest_bytes)?;
        let format = value
            .get("format")
            .and_then(|f| f.as_str())
            .ok_or_else(|| StudyError::corrupt("manifest names no format version"))?;
        if major_version(format) != Some("1") {
            return Err(StudyError::UnsupportedVersion(format.to_owned()));
        }
        let manifest: Manifest = serde_json::from_value(value)?;

        // Blob table: shape, encodings, bounds, and agreement with the entries.
        let mut entries = BTreeMap::new();
        let mut total = 0u64;
        for record in &manifest.blobs {
            if !is_sha256_hex(&record.sha256) {
                return Err(StudyError::corrupt(format!(
                    "blob table holds an invalid digest \"{}\"",
                    record.sha256.escape_debug()
                )));
            }
            if record.encoding != ENCODING_VERBATIM {
                return Err(StudyError::UnsupportedEncoding {
                    blob: record.sha256.clone(),
                    encoding: record.encoding.clone(),
                });
            }
            if record.bytes > MAX_BLOB_BYTES {
                return Err(StudyError::corrupt(format!(
                    "blob {} declares an excessive size",
                    record.sha256
                )));
            }
            total = total
                .checked_add(record.bytes)
                .filter(|t| *t <= MAX_TOTAL_BYTES)
                .ok_or_else(|| StudyError::corrupt("study file declares too many bytes"))?;
            let index = blob_indices.get(&record.sha256).ok_or_else(|| {
                StudyError::corrupt(format!(
                    "blob {} is listed in the manifest but missing from the file",
                    record.sha256
                ))
            })?;
            if entries
                .insert(record.sha256.clone(), (*index, record.bytes))
                .is_some()
            {
                return Err(StudyError::corrupt(format!(
                    "blob {} is listed twice",
                    record.sha256
                )));
            }
        }
        if let Some(extra) = blob_indices.keys().find(|h| !entries.contains_key(*h)) {
            return Err(StudyError::corrupt(format!(
                "blob {extra} is in the file but not listed in the manifest"
            )));
        }
        let reader = Self {
            archive,
            manifest,
            entries,
            file_bytes,
            preview: preview_index,
        };
        reader.check_references()?;
        Ok(reader)
    }

    fn check_references(&self) -> Result<(), StudyError> {
        let known = |hash: &str| -> Result<(), StudyError> {
            if self.entries.contains_key(hash) {
                Ok(())
            } else {
                Err(StudyError::corrupt(format!(
                    "the manifest refers to blob {hash}, which the file does not hold"
                )))
            }
        };
        let m = &self.manifest;
        let mut bundle_sets: Vec<&[BundleRecord]> = vec![&m.sweep];
        for a in [&m.arrangements.port, &m.arrangements.control]
            .into_iter()
            .flatten()
        {
            if let Some(s) = &a.scenario {
                known(s)?;
            }
            for p in &a.physics {
                known(p)?;
            }
            bundle_sets.push(&a.bundles);
        }
        for set in bundle_sets {
            let mut names = BTreeSet::new();
            for bundle in set {
                if !is_safe_file_name(&bundle.name) || !names.insert(&bundle.name) {
                    return Err(StudyError::corrupt(format!(
                        "bundle name \"{}\" is unsafe or repeated",
                        bundle.name.escape_debug()
                    )));
                }
                for hash in bundle.files.values() {
                    known(hash)?;
                }
            }
        }
        if let Some(a) = &m.assumptions {
            known(a)?;
        }
        let mut keys = BTreeSet::new();
        for record in &m.ensembles {
            known(&record.blob)?;
            if !keys.insert(&record.key) {
                return Err(StudyError::corrupt(
                    "the manifest lists one history ensemble key twice",
                ));
            }
        }
        if let Some(EvidenceLayer { mode, archives }) = &m.layers.evidence {
            let mut roles = BTreeSet::new();
            for archive in archives {
                if !matches!(archive.arrangement.as_str(), "port" | "control")
                    || !is_safe_file_name(&archive.allocation)
                    || !is_safe_relative_path(&archive.file_name)
                    || !is_sha256_hex(&archive.sha256)
                    || !roles.insert((&archive.arrangement, &archive.allocation, archive.kind))
                {
                    return Err(StudyError::corrupt(format!(
                        "evidence record for \"{}\" is invalid or repeated",
                        archive.file_name.escape_debug()
                    )));
                }
                if *mode == EvidenceMode::Packed {
                    known(&archive.sha256)?;
                    if self.entries[&archive.sha256].1 != archive.bytes {
                        return Err(StudyError::corrupt(format!(
                            "evidence archive {} disagrees with its blob about size",
                            archive.file_name
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// The thumbnail, if the file has one the reader can use. It is optional
    /// and carries no evidence: bytes that are damaged (the container's CRC
    /// fails), are not a PNG, or are larger than a writer would store are
    /// ignored with a reason, and never refuse the study.
    pub fn preview_status(&mut self) -> PreviewStatus {
        let Some(index) = self.preview else {
            return PreviewStatus::Absent;
        };
        let ignored = |bytes: u64, reason: &str| PreviewStatus::Ignored {
            bytes,
            reason: reason.to_owned(),
        };
        let mut entry = match self.archive.by_index(index) {
            Ok(entry) => entry,
            Err(_) => return ignored(0, "the entry cannot be opened"),
        };
        let declared = entry.size();
        if declared > MAX_PREVIEW_BYTES_USED {
            return ignored(declared, "larger than 512 KiB");
        }
        let mut png = Vec::new();
        if (&mut entry)
            .take(MAX_PREVIEW_BYTES_USED + 1)
            .read_to_end(&mut png)
            .is_err()
        {
            return ignored(declared, "the entry is damaged");
        }
        let bytes = png.len() as u64;
        match png_dimensions(&png) {
            Some((width, height))
                if (1..=MAX_PREVIEW_SIDE).contains(&width)
                    && (1..=MAX_PREVIEW_SIDE).contains(&height) =>
            {
                PreviewStatus::Usable(Preview { png, width, height })
            }
            Some(_) => ignored(bytes, "larger than 512 pixels on a side"),
            None => ignored(bytes, "not a PNG"),
        }
    }

    /// The usable thumbnail, if any.
    pub fn preview(&mut self) -> Option<Preview> {
        match self.preview_status() {
            PreviewStatus::Usable(preview) => Some(preview),
            _ => None,
        }
    }

    pub fn blob_infos(&mut self) -> Result<Vec<BlobInfo>, StudyError> {
        let mut infos = Vec::new();
        for record in self.manifest.blobs.clone() {
            let index = self.entries[&record.sha256].0;
            let entry = self.archive.by_index_raw(index)?;
            infos.push(BlobInfo {
                sha256: record.sha256,
                bytes: record.bytes,
                media_type: record.media_type,
                stored_bytes: entry.compressed_size(),
                compressed: entry.compression() != CompressionMethod::Stored,
            });
        }
        Ok(infos)
    }

    /// Read one blob, bounded by its recorded size and checked against its
    /// recorded SHA-256 before it is returned.
    pub fn read_blob(&mut self, sha256: &str) -> Result<Vec<u8>, StudyError> {
        let &(index, bytes) = self.entries.get(sha256).ok_or_else(|| {
            StudyError::corrupt(format!("blob {sha256} is not recorded in the manifest"))
        })?;
        let mut entry = self.archive.by_index(index)?;
        if entry.size() != bytes {
            return Err(StudyError::SizeMismatch {
                blob: sha256.to_owned(),
                recorded: bytes,
                found: format!("{} declared by the container", entry.size()),
            });
        }
        let mut data = Vec::with_capacity(bytes.min(64 * 1024 * 1024) as usize);
        (&mut entry)
            .take(bytes + 1)
            .read_to_end(&mut data)
            .map_err(|e| StudyError::corrupt(format!("blob {sha256} is damaged: {e}")))?;
        if data.len() as u64 != bytes {
            return Err(StudyError::SizeMismatch {
                blob: sha256.to_owned(),
                recorded: bytes,
                found: if data.len() as u64 > bytes {
                    "more than recorded".into()
                } else {
                    data.len().to_string()
                },
            });
        }
        let found = sha256_hex(&data);
        if found != sha256 {
            return Err(StudyError::HashMismatch {
                blob: sha256.to_owned(),
                found,
            });
        }
        Ok(data)
    }

    /// Rehash every blob. Returns the number checked.
    pub fn verify(&mut self) -> Result<usize, StudyError> {
        let hashes: Vec<String> = self.entries.keys().cloned().collect();
        for hash in &hashes {
            self.read_blob(hash)?;
        }
        self.verify_structure()?;
        Ok(hashes.len())
    }

    /// Every stored history ensemble, each read through the hash check and
    /// parsed strictly. A blob that does not parse as an ensemble, or whose
    /// own method, seed or sample count disagrees with its recorded key,
    /// refuses the file: the reader never skips data it cannot interpret.
    pub fn ensembles(&mut self) -> Result<Vec<StoredEnsemble>, StudyError> {
        let mut stored = Vec::new();
        for record in self.manifest.ensembles.clone() {
            let bytes = self.read_blob(&record.blob)?;
            let ensemble: HistoryEnsemble = serde_json::from_slice(&bytes).map_err(|e| {
                StudyError::corrupt(format!(
                    "history ensemble blob {} is not a valid ensemble: {e}",
                    record.blob
                ))
            })?;
            if !record.key.describes(&ensemble) {
                return Err(StudyError::corrupt(format!(
                    "history ensemble blob {} disagrees with the key recorded for it",
                    record.blob
                )));
            }
            stored.push(StoredEnsemble {
                scenario_sha256: record.scenario_sha256,
                variant: record.variant,
                key: record.key,
                ensemble,
            });
        }
        Ok(stored)
    }

    /// The stored ensemble for exactly this key, if there is one. A key that
    /// differs in any part (rates, covariance, assumptions, samples, seed or
    /// method) finds nothing, and the caller recomputes.
    pub fn ensemble_for(
        &mut self,
        key: &EnsembleKey,
    ) -> Result<Option<HistoryEnsemble>, StudyError> {
        Ok(self
            .ensembles()?
            .into_iter()
            .find(|stored| stored.key == *key)
            .map(|stored| stored.ensemble))
    }

    /// Reconstruct every bundle; catches members a bundle must not hold.
    fn verify_structure(&mut self) -> Result<(), StudyError> {
        self.ensembles()?;
        let mut bundles: Vec<BundleRecord> = self.manifest.sweep.clone();
        for a in [
            &self.manifest.arrangements.port,
            &self.manifest.arrangements.control,
        ]
        .into_iter()
        .flatten()
        {
            bundles.extend(a.bundles.iter().cloned());
        }
        for record in &bundles {
            self.bundle(record)?;
        }
        Ok(())
    }

    /// The exact recorded-transport bundle a record describes.
    pub fn bundle(&mut self, record: &BundleRecord) -> Result<RecordedTransportBundle, StudyError> {
        let mut files = BTreeMap::new();
        for (member, hash) in &record.files {
            let text = String::from_utf8(self.read_blob(hash)?).map_err(|_| {
                StudyError::corrupt(format!("bundle member {member} is not UTF-8 text"))
            })?;
            files.insert(member.clone(), text);
        }
        let bundle = RecordedTransportBundle {
            schema_version: record.schema_version.clone(),
            files,
            notice: record.notice.clone(),
        };
        bundle.validate().map_err(|e| {
            StudyError::corrupt(format!("bundle \"{}\" is not valid: {e}", record.name))
        })?;
        Ok(bundle)
    }

    /// Write every input out as ordinary files under an existing directory
    /// and report where the evidence archives are. With `near`, referenced
    /// evidence is looked for beside the study file by recorded name and used
    /// only when its hash and size match.
    pub fn materialize(
        &mut self,
        destination: &Path,
        near: Option<&Path>,
    ) -> Result<Materialized, StudyError> {
        let write = |path: &Path, bytes: &[u8]| -> Result<(), StudyError> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    StudyError::io(format!("cannot create {}", parent.display()), e)
                })?;
            }
            std::fs::write(path, bytes)
                .map_err(|e| StudyError::io(format!("cannot write {}", path.display()), e))
        };
        let manifest = self.manifest.clone();
        let arrangement = |reader: &mut Self,
                           role: &str,
                           record: &ArrangementRecord|
         -> Result<ArrangementFiles, StudyError> {
            let base = destination.join(role);
            let mut files = ArrangementFiles::default();
            if let Some(hash) = &record.scenario {
                let path = base.join("scenario.json");
                write(&path, &reader.read_blob(hash)?)?;
                files.scenario = Some(path);
            }
            for (index, hash) in record.physics.iter().enumerate() {
                let path = base.join("physics").join(format!("physics-{index}.json"));
                write(&path, &reader.read_blob(hash)?)?;
                files.physics.push(path);
            }
            for bundle in &record.bundles {
                files
                    .bundles
                    .push(reader.write_bundle(&base.join("bundles"), bundle)?);
            }
            Ok(files)
        };
        let port = manifest
            .arrangements
            .port
            .as_ref()
            .map(|r| arrangement(self, "port", r))
            .transpose()?;
        let control = manifest
            .arrangements
            .control
            .as_ref()
            .map(|r| arrangement(self, "control", r))
            .transpose()?;
        let mut sweep = Vec::new();
        for bundle in &manifest.sweep {
            sweep.push(self.write_bundle(&destination.join("sweep"), bundle)?);
        }
        let assumptions = match &manifest.assumptions {
            Some(hash) => {
                let path = destination.join("operating-assumptions.json");
                write(&path, &self.read_blob(hash)?)?;
                Some(path)
            }
            None => None,
        };
        let evidence = self.materialize_evidence(destination, near)?;
        Ok(Materialized {
            port,
            control,
            sweep,
            assumptions,
            evidence,
        })
    }

    fn write_bundle(
        &mut self,
        directory: &Path,
        record: &BundleRecord,
    ) -> Result<PathBuf, StudyError> {
        let bundle = self.bundle(record)?;
        let mut json = serde_json::to_vec_pretty(&bundle)?;
        if record.trailing_newline {
            json.push(b'\n');
        }
        std::fs::create_dir_all(directory)
            .map_err(|e| StudyError::io(format!("cannot create {}", directory.display()), e))?;
        let path = directory.join(format!("{}.transport-bundle.json", record.name));
        std::fs::write(&path, json)
            .map_err(|e| StudyError::io(format!("cannot write {}", path.display()), e))?;
        Ok(path)
    }

    fn materialize_evidence(
        &mut self,
        destination: &Path,
        near: Option<&Path>,
    ) -> Result<EvidenceState, StudyError> {
        let Some(layer) = self.manifest.layers.evidence.clone() else {
            return Ok(EvidenceState::default());
        };
        let mut state = EvidenceState {
            mode: Some(layer.mode),
            ..EvidenceState::default()
        };
        for archive in layer.archives {
            match layer.mode {
                EvidenceMode::Packed => {
                    let path = destination.join("evidence").join(&archive.file_name);
                    let bytes = self.read_blob(&archive.sha256)?;
                    std::fs::create_dir_all(path.parent().expect("has parent"))
                        .map_err(|e| StudyError::io("cannot create the evidence directory", e))?;
                    std::fs::write(&path, bytes).map_err(|e| {
                        StudyError::io(format!("cannot write {}", path.display()), e)
                    })?;
                    state.available.push(EvidenceFile { archive, path });
                }
                EvidenceMode::Referenced => {
                    let candidate = near.map(|d| d.join(&archive.file_name));
                    match candidate {
                        None => state.missing.push(EvidenceMiss {
                            archive,
                            reason: MissReason::NotFound,
                        }),
                        Some(path) => match sha256_file(&path) {
                            Err(_) => state.missing.push(EvidenceMiss {
                                archive,
                                reason: MissReason::NotFound,
                            }),
                            Ok((sha, bytes)) if sha == archive.sha256 && bytes == archive.bytes => {
                                state.available.push(EvidenceFile { archive, path })
                            }
                            Ok(_) => state.missing.push(EvidenceMiss {
                                archive,
                                reason: MissReason::HashMismatch,
                            }),
                        },
                    }
                }
            }
        }
        Ok(state)
    }
}

#[derive(Clone, Debug, Default)]
pub struct ArrangementFiles {
    pub scenario: Option<PathBuf>,
    pub physics: Vec<PathBuf>,
    pub bundles: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct EvidenceFile {
    pub archive: EvidenceArchive,
    pub path: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissReason {
    NotFound,
    HashMismatch,
}

#[derive(Clone, Debug)]
pub struct EvidenceMiss {
    pub archive: EvidenceArchive,
    pub reason: MissReason,
}

/// What the evidence layer of an opened study amounts to.
#[derive(Clone, Debug, Default)]
pub struct EvidenceState {
    /// `None` when the study records no evidence.
    pub mode: Option<EvidenceMode>,
    pub available: Vec<EvidenceFile>,
    pub missing: Vec<EvidenceMiss>,
}

impl EvidenceState {
    /// Archives of one arrangement and allocation, when both case and
    /// workspace are available.
    pub fn pair(
        &self,
        arrangement: &str,
        allocation: &str,
    ) -> Option<(&EvidenceFile, &EvidenceFile)> {
        let find = |kind: ArchiveKind| {
            self.available.iter().find(|f| {
                f.archive.arrangement == arrangement
                    && f.archive.allocation == allocation
                    && f.archive.kind == kind
            })
        };
        Some((find(ArchiveKind::Case)?, find(ArchiveKind::Workspace)?))
    }
}

#[derive(Clone, Debug)]
pub struct Materialized {
    pub port: Option<ArrangementFiles>,
    pub control: Option<ArrangementFiles>,
    pub sweep: Vec<PathBuf>,
    pub assumptions: Option<PathBuf>,
    pub evidence: EvidenceState,
}
