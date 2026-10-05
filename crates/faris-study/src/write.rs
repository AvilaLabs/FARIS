use crate::{
    ArchiveKind, ArrangementRecord, Arrangements, BlobRecord, BundleRecord, ENCODING_VERBATIM,
    ENSEMBLE_MEDIA_TYPE, EnsembleRecord, EvidenceArchive, EvidenceLayer, EvidenceMode, FORMAT,
    Layers, MIMETYPE, Manifest, StudyError, ViewState, is_safe_file_name, is_safe_relative_path,
    is_sha256_hex, read::sha256_file, sha256_hex,
};
use faris_engine::core_evidence::{RecordedTransportBundle, read_stage};
use faris_engine::history_ensemble::HistoryEnsemble;
use faris_engine::history_uncertainty::EnsembleKey;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

/// zstd level used for recorded text unless the caller picks another. Measured on
/// the 11-bundle demo study (154 MB of unique text): level 15 saves in about 6 s
/// to 6.1 MB; level 19 would reach 5.0 MB but takes over two minutes.
pub const DEFAULT_ZSTD_LEVEL: i64 = 15;
/// Largest single non-evidence input file (scenario, physics, assumptions).
const MAX_SMALL_FILE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub struct ArrangementDraft {
    /// The scenario file; optional because each bundle carries its own copy.
    pub scenario: Option<PathBuf>,
    pub physics: Vec<PathBuf>,
    /// Recorded-transport bundle JSON files.
    pub bundles: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct EvidenceDraft {
    pub archive: EvidenceArchive,
    /// Where the archive's bytes are, needed only when packing.
    pub path: Option<PathBuf>,
}

/// A finished history ensemble to store with the study, with the full key of
/// the inputs it was calculated from.
#[derive(Clone, Debug)]
pub struct EnsembleDraft {
    pub scenario_sha256: String,
    pub variant: String,
    pub key: EnsembleKey,
    pub ensemble: Arc<HistoryEnsemble>,
}

#[derive(Clone, Debug)]
pub struct StudyDraft {
    pub port: Option<ArrangementDraft>,
    pub control: Option<ArrangementDraft>,
    pub sweep: Vec<PathBuf>,
    pub assumptions: Option<PathBuf>,
    pub evidence: Vec<EvidenceDraft>,
    /// Calculated history ensembles, stored as derived blobs.
    pub ensembles: Vec<EnsembleDraft>,
    /// Store the evidence archives inside the file instead of by reference.
    pub pack_evidence: bool,
    pub view: ViewState,
    pub zstd_level: i64,
}

impl Default for StudyDraft {
    fn default() -> Self {
        Self {
            port: None,
            control: None,
            sweep: Vec::new(),
            assumptions: None,
            evidence: Vec::new(),
            ensembles: Vec::new(),
            pack_evidence: false,
            view: ViewState::default(),
            zstd_level: DEFAULT_ZSTD_LEVEL,
        }
    }
}

#[derive(Clone, Debug)]
pub struct WriteReport {
    /// Size of the written file.
    pub file_bytes: u64,
    pub blob_count: usize,
    /// Sum of the blobs' original sizes.
    pub original_bytes: u64,
    pub evidence: Option<EvidenceMode>,
}

/// Blobs in first-seen order; identical content is held and written once.
#[derive(Default)]
struct BlobSet {
    table: Vec<BlobRecord>,
    data: BTreeMap<String, Vec<u8>>,
    streamed: Vec<(String, PathBuf)>,
}

impl BlobSet {
    fn add(&mut self, bytes: Vec<u8>, media_type: &str) -> String {
        let sha256 = sha256_hex(&bytes);
        if !self.data.contains_key(&sha256) && !self.table.iter().any(|b| b.sha256 == sha256) {
            self.table.push(BlobRecord {
                sha256: sha256.clone(),
                bytes: bytes.len() as u64,
                media_type: media_type.into(),
                encoding: ENCODING_VERBATIM.into(),
            });
            self.data.insert(sha256.clone(), bytes);
        }
        sha256
    }

    fn add_streamed(&mut self, archive: &EvidenceArchive, path: &Path) {
        if !self.table.iter().any(|b| b.sha256 == archive.sha256) {
            self.table.push(BlobRecord {
                sha256: archive.sha256.clone(),
                bytes: archive.bytes,
                media_type: "application/gzip".into(),
                encoding: ENCODING_VERBATIM.into(),
            });
            self.streamed
                .push((archive.sha256.clone(), path.to_owned()));
        }
    }
}

fn read_small(path: &Path) -> Result<Vec<u8>, StudyError> {
    let context = || format!("cannot read {}", path.display());
    let metadata = std::fs::metadata(path).map_err(|e| StudyError::io(context(), e))?;
    if !metadata.is_file() {
        return Err(StudyError::Input(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| StudyError::io(context(), e))?
        .take(MAX_SMALL_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| StudyError::io(context(), e))?;
    if bytes.len() as u64 > MAX_SMALL_FILE_BYTES {
        return Err(StudyError::Input(format!(
            "{} exceeds the {} MiB limit for study inputs",
            path.display(),
            MAX_SMALL_FILE_BYTES / (1024 * 1024)
        )));
    }
    Ok(bytes)
}

/// "reference.transport-bundle.json" becomes "reference".
fn bundle_name(path: &Path, taken: &BTreeSet<String>) -> String {
    let file = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let stem = file
        .strip_suffix(".transport-bundle.json")
        .or_else(|| file.strip_suffix(".json"))
        .unwrap_or(&file);
    let mut base: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if base.is_empty() || base == "." || base == ".." {
        base = "bundle".into();
    }
    let mut name = base.clone();
    let mut n = 2;
    while taken.contains(&name) {
        name = format!("{base}-{n}");
        n += 1;
    }
    name
}

fn media_type_of(member: &str) -> &'static str {
    if member.ends_with(".py") {
        "text/x-python"
    } else {
        "application/json"
    }
}

fn add_bundles(blobs: &mut BlobSet, paths: &[PathBuf]) -> Result<Vec<BundleRecord>, StudyError> {
    let mut names = BTreeSet::new();
    let mut records = Vec::new();
    for path in paths {
        let bytes = read_stage(path)
            .map_err(|e| StudyError::Input(format!("cannot read {}: {e}", path.display())))?;
        let bundle: RecordedTransportBundle = serde_json::from_slice(&bytes).map_err(|e| {
            StudyError::Input(format!(
                "{} is not a recorded-transport bundle: {e}",
                path.display()
            ))
        })?;
        bundle.validate().map_err(|e| {
            StudyError::Input(format!("{} is not a valid bundle: {e}", path.display()))
        })?;
        let name = bundle_name(path, &names);
        names.insert(name.clone());
        let files = bundle
            .files
            .iter()
            .map(|(member, text)| {
                (
                    member.clone(),
                    blobs.add(text.as_bytes().to_vec(), media_type_of(member)),
                )
            })
            .collect();
        records.push(BundleRecord {
            name,
            schema_version: bundle.schema_version,
            notice: bundle.notice,
            files,
            trailing_newline: bytes.last() == Some(&b'\n'),
        });
    }
    Ok(records)
}

fn add_arrangement(
    blobs: &mut BlobSet,
    draft: &ArrangementDraft,
) -> Result<ArrangementRecord, StudyError> {
    Ok(ArrangementRecord {
        scenario: draft
            .scenario
            .as_deref()
            .map(|p| Ok::<_, StudyError>(blobs.add(read_small(p)?, "application/json")))
            .transpose()?,
        physics: draft
            .physics
            .iter()
            .map(|p| Ok(blobs.add(read_small(p)?, "application/json")))
            .collect::<Result<_, StudyError>>()?,
        bundles: add_bundles(blobs, &draft.bundles)?,
    })
}

fn check_evidence(draft: &StudyDraft, blobs: &mut BlobSet) -> Result<(), StudyError> {
    let mut names = BTreeSet::new();
    for item in &draft.evidence {
        let a = &item.archive;
        if !matches!(a.arrangement.as_str(), "port" | "control")
            || !is_safe_file_name(&a.allocation)
            || !is_safe_relative_path(&a.file_name)
            || !is_sha256_hex(&a.sha256)
            || !names.insert((a.arrangement.clone(), a.allocation.clone(), a.kind))
        {
            return Err(StudyError::Input(format!(
                "invalid or duplicate evidence record for {}",
                a.file_name
            )));
        }
        if draft.pack_evidence {
            let path = item.path.as_deref().ok_or_else(|| {
                StudyError::Input(format!(
                    "cannot include Core evidence: archive {} is not available (the receipts were saved by reference)",
                    a.file_name
                ))
            })?;
            let (sha, bytes) = sha256_file(path)
                .map_err(|e| StudyError::io(format!("cannot read {}", path.display()), e))?;
            if sha != a.sha256 || bytes != a.bytes {
                return Err(StudyError::Input(format!(
                    "evidence archive {} does not match its recorded hash and size",
                    path.display()
                )));
            }
            blobs.add_streamed(a, path);
        }
    }
    Ok(())
}

fn add_ensembles(
    blobs: &mut BlobSet,
    drafts: &[EnsembleDraft],
) -> Result<Vec<EnsembleRecord>, StudyError> {
    let mut records: Vec<EnsembleRecord> = Vec::new();
    for draft in drafts {
        if !draft.key.describes(&draft.ensemble) {
            return Err(StudyError::Input(format!(
                "the history ensemble for {} does not match the key it was given",
                draft.variant
            )));
        }
        if records.iter().any(|r| r.key == draft.key) {
            continue;
        }
        let bytes = serde_json::to_vec(&*draft.ensemble)?;
        records.push(EnsembleRecord {
            blob: blobs.add(bytes, ENSEMBLE_MEDIA_TYPE),
            scenario_sha256: draft.scenario_sha256.clone(),
            variant: draft.variant.clone(),
            key: draft.key.clone(),
        });
    }
    Ok(records)
}

fn zip_error(error: zip::result::ZipError) -> StudyError {
    StudyError::from(error)
}

pub fn write_study(path: &Path, draft: &StudyDraft) -> Result<WriteReport, StudyError> {
    let mut blobs = BlobSet::default();
    let arrangements = Arrangements {
        port: draft
            .port
            .as_ref()
            .map(|a| add_arrangement(&mut blobs, a))
            .transpose()?,
        control: draft
            .control
            .as_ref()
            .map(|a| add_arrangement(&mut blobs, a))
            .transpose()?,
    };
    let sweep = add_bundles(&mut blobs, &draft.sweep)?;
    let assumptions = draft
        .assumptions
        .as_deref()
        .map(|p| Ok::<_, StudyError>(blobs.add(read_small(p)?, "application/json")))
        .transpose()?;
    check_evidence(draft, &mut blobs)?;
    let ensembles = add_ensembles(&mut blobs, &draft.ensembles)?;
    let evidence = (!draft.evidence.is_empty()).then(|| EvidenceLayer {
        mode: if draft.pack_evidence {
            EvidenceMode::Packed
        } else {
            EvidenceMode::Referenced
        },
        archives: draft.evidence.iter().map(|e| e.archive.clone()).collect(),
    });
    let manifest = Manifest {
        format: FORMAT.into(),
        created_by: format!("FARIS {}", env!("CARGO_PKG_VERSION")),
        arrangements,
        sweep,
        assumptions,
        view: draft.view.clone(),
        layers: Layers {
            evidence: evidence.clone(),
        },
        ensembles,
        blobs: blobs.table.clone(),
    };
    let mut manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    manifest_bytes.push(b'\n');

    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temp = tempfile::Builder::new()
        .prefix(".faris-save-")
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|e| StudyError::io(format!("cannot create a file in {}", parent.display()), e))?;
    {
        let mut zip = ZipWriter::new(BufWriter::with_capacity(1 << 20, temp.as_file_mut()));
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let zstd = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Zstd)
            .compression_level(Some(draft.zstd_level));
        // The type string must be the first entry, uncompressed, so file(1) and
        // shared-mime-info can recognise it from the leading bytes.
        zip.start_file("mimetype", stored).map_err(zip_error)?;
        zip.write_all(MIMETYPE.as_bytes())
            .map_err(|e| StudyError::io("writing study file", e))?;
        zip.start_file("manifest.json", zstd).map_err(zip_error)?;
        zip.write_all(&manifest_bytes)
            .map_err(|e| StudyError::io("writing study file", e))?;
        for record in &blobs.table {
            let name = format!("blobs/{}", record.sha256);
            if let Some(bytes) = blobs.data.get(&record.sha256) {
                zip.start_file(name, zstd).map_err(zip_error)?;
                zip.write_all(bytes)
                    .map_err(|e| StudyError::io("writing study file", e))?;
            } else if let Some((_, source)) =
                blobs.streamed.iter().find(|(sha, _)| *sha == record.sha256)
            {
                // Already-compressed evidence archives are stored, not recompressed.
                zip.start_file(name, stored).map_err(zip_error)?;
                let mut input = File::open(source)
                    .map_err(|e| StudyError::io(format!("cannot read {}", source.display()), e))?;
                let copied = std::io::copy(&mut input, &mut zip)
                    .map_err(|e| StudyError::io("writing study file", e))?;
                if copied != record.bytes {
                    return Err(StudyError::Input(format!(
                        "{} changed while the study was being saved",
                        source.display()
                    )));
                }
            }
        }
        zip.finish()
            .map_err(zip_error)?
            .into_inner()
            .map_err(|e| StudyError::io("writing study file", e.into_error()))?;
    }
    temp.as_file()
        .sync_all()
        .map_err(|e| StudyError::io("syncing study file", e))?;
    set_permissions(temp.as_file(), path);
    let file_bytes = temp
        .as_file()
        .metadata()
        .map_err(|e| StudyError::io("measuring study file", e))?
        .len();
    temp.persist(path)
        .map_err(|e| StudyError::io(format!("cannot write {}", path.display()), e.error))?;
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(WriteReport {
        file_bytes,
        blob_count: blobs.table.len(),
        original_bytes: blobs.table.iter().map(|b| b.bytes).sum(),
        evidence: evidence.map(|e| e.mode),
    })
}

/// Temporary files are private; a saved study should look like any other
/// document: keep an overwritten file's mode, otherwise 0644.
fn set_permissions(file: &File, target: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(target)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0o644);
        let _ = file.set_permissions(std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = (file, target);
}

impl ArchiveKind {
    pub fn suffix(self) -> &'static str {
        match self {
            ArchiveKind::Case => "case",
            ArchiveKind::Workspace => "workspace",
        }
    }
}
