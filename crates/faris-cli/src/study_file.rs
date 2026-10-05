use clap::Subcommand;
use faris_study::{
    ArrangementDraft, DEFAULT_ZSTD_LEVEL, EvidenceDraft, StudyDraft, StudyError, StudyReader,
    ViewState, evidence_from_descriptor, write_study,
};
use serde_json::json;
use std::path::{Path, PathBuf};

/// The file was read but failed verification; exits with status 1.
#[derive(Debug)]
pub struct StudyFileRejected(pub String);

impl std::fmt::Display for StudyFileRejected {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for StudyFileRejected {}

pub(crate) fn convert(error: StudyError) -> Box<dyn std::error::Error> {
    if error.is_verification_failure() {
        Box::new(StudyFileRejected(error.to_string()))
    } else {
        Box::new(error)
    }
}

// Parsed once per process; boxing the flag set would only obscure the derive.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub enum StudyFileCommand {
    /// Write a .faris study file from recorded transport bundles and assumptions.
    Create {
        /// New study file; an existing file is never overwritten.
        #[arg(short, long)]
        output: PathBuf,
        /// Scenario of the port arrangement (each bundle also carries its own copy).
        #[arg(long)]
        scenario: Option<PathBuf>,
        /// Physics input of the port arrangement; repeatable.
        #[arg(long)]
        physics: Vec<PathBuf>,
        /// Recorded-transport bundle of the port arrangement; repeatable.
        #[arg(long)]
        bundle: Vec<PathBuf>,
        #[arg(long)]
        control_scenario: Option<PathBuf>,
        #[arg(long)]
        control_physics: Vec<PathBuf>,
        #[arg(long)]
        control_bundle: Vec<PathBuf>,
        /// Operating assumptions file.
        #[arg(long)]
        assumptions: Option<PathBuf>,
        /// Recorded-transport bundle of the allocation sweep; repeatable.
        #[arg(long)]
        sweep_bundle: Vec<PathBuf>,
        /// Saved-study descriptor naming Core case and workspace archives; repeatable.
        #[arg(long)]
        evidence: Vec<PathBuf>,
        /// Store the Core evidence archives inside the file (default: by reference).
        #[arg(long, requires = "evidence")]
        pack_evidence: bool,
        /// JSON file with the view to record (any of: step, preset, what_if, year,
        /// field_view, history_tab, arrangement, allocation, sweep_blanket_m).
        #[arg(long)]
        view: Option<PathBuf>,
        /// zstd level for recorded text.
        #[arg(long, default_value_t = DEFAULT_ZSTD_LEVEL, value_parser = clap::value_parser!(i64).range(1..=22))]
        zstd_level: i64,
    },
    /// Summarize a study file without extracting it.
    Inspect { file: PathBuf },
    /// Rehash every blob and rebuild every bundle; exit 1 if anything fails.
    Verify { file: PathBuf },
    /// Write the study export folder (PDF, CSV, charts, manifest) from a study file.
    ///
    /// Histories are recalculated from the recorded assumptions and rates. The 3D
    /// view is not captured on the command line; export from the desktop for it.
    Export {
        file: PathBuf,
        /// Folder to create `<study name>-export` in; an existing export folder is
        /// never written into.
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Write a study's bundles and files back out as ordinary files.
    Unpack {
        file: PathBuf,
        /// New directory; an existing path is refused.
        directory: PathBuf,
    },
}

fn arrangement(
    scenario: Option<PathBuf>,
    physics: Vec<PathBuf>,
    bundles: Vec<PathBuf>,
) -> Option<ArrangementDraft> {
    (scenario.is_some() || !physics.is_empty() || !bundles.is_empty()).then_some(ArrangementDraft {
        scenario,
        physics,
        bundles,
    })
}

pub fn run(command: StudyFileCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        StudyFileCommand::Create {
            output,
            scenario,
            physics,
            bundle,
            control_scenario,
            control_physics,
            control_bundle,
            assumptions,
            sweep_bundle,
            evidence,
            pack_evidence,
            view,
            zstd_level,
        } => {
            if output.exists() {
                return Err(format!(
                    "{} already exists; it is never overwritten",
                    output.display()
                )
                .into());
            }
            let mut drafts: Vec<EvidenceDraft> = Vec::new();
            for descriptor in &evidence {
                drafts.extend(evidence_from_descriptor(descriptor).map_err(convert)?);
            }
            let draft = StudyDraft {
                port: arrangement(scenario, physics, bundle),
                control: arrangement(control_scenario, control_physics, control_bundle),
                sweep: sweep_bundle,
                assumptions,
                evidence: drafts,
                pack_evidence,
                zstd_level,
                view: match view {
                    Some(path) => serde_json::from_slice(&crate::transport::read_bounded(&path)?)
                        .map_err(|e| format!("{} is not a view: {e}", path.display()))?,
                    None => ViewState::default(),
                },
            };
            if draft.port.is_none() && draft.control.is_none() && draft.sweep.is_empty() {
                return Err("a study needs at least one bundle (--bundle, --control-bundle or --sweep-bundle)".into());
            }
            let report = write_study(&output, &draft).map_err(convert)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "file": output, "file_bytes": report.file_bytes, "blobs": report.blob_count,
                    "original_bytes": report.original_bytes,
                    "evidence": report.evidence.map(|m| format!("{m:?}").to_lowercase()),
                }))?
            );
        }
        StudyFileCommand::Inspect { file } => {
            let mut reader = StudyReader::open(&file).map_err(convert)?;
            let blobs = reader.blob_infos().map_err(convert)?;
            let m = &reader.manifest;
            let describe = |a: &Option<faris_study::ArrangementRecord>| {
                a.as_ref().map(|a| {
                    json!({
                        "scenario": a.scenario.is_some(), "physics_files": a.physics.len(),
                        "bundles": a.bundles.iter().map(|b| &b.name).collect::<Vec<_>>(),
                    })
                })
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "file": file, "file_bytes": reader.file_bytes,
                    "format": m.format, "created_by": m.created_by,
                    "arrangements": {"port": describe(&m.arrangements.port), "control": describe(&m.arrangements.control)},
                    "sweep": m.sweep.iter().map(|b| &b.name).collect::<Vec<_>>(),
                    "assumptions": m.assumptions.is_some(),
                    "view": m.view,
                    "evidence": m.layers.evidence,
                    "blobs": {
                        "count": blobs.len(),
                        "original_bytes": blobs.iter().map(|b| b.bytes).sum::<u64>(),
                        "stored_bytes": blobs.iter().map(|b| b.stored_bytes).sum::<u64>(),
                    },
                }))?
            );
        }
        StudyFileCommand::Verify { file } => {
            let mut reader = StudyReader::open(&file).map_err(convert)?;
            let count = reader.verify().map_err(convert)?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"file": file, "verified": true, "blobs": count})
                )?
            );
        }
        StudyFileCommand::Export { file, output } => {
            let summary = crate::study_export::export(&file, &output)?;
            println!("{}", serde_json::to_string_pretty(&summary)?);
        }
        StudyFileCommand::Unpack { file, directory } => {
            let mut reader = StudyReader::open(&file).map_err(convert)?;
            std::fs::create_dir(&directory).map_err(|e| {
                format!(
                    "cannot create {} (an existing directory is refused): {e}",
                    directory.display()
                )
            })?;
            let near = file
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let files = reader
                .materialize(&directory, Some(near))
                .map_err(convert)?;
            let mut manifest = serde_json::to_vec_pretty(&reader.manifest)?;
            manifest.push(b'\n');
            std::fs::write(directory.join("manifest.json"), manifest)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "directory": directory,
                    "port_bundles": files.port.as_ref().map_or(0, |a| a.bundles.len()),
                    "control_bundles": files.control.as_ref().map_or(0, |a| a.bundles.len()),
                    "sweep_bundles": files.sweep.len(),
                    "assumptions": files.assumptions.is_some(),
                    "evidence_mode": files.evidence.mode.map(|m| format!("{m:?}").to_lowercase()),
                    "evidence_available": files.evidence.available.len(),
                    "evidence_missing": files.evidence.missing.iter().map(|m| &m.archive.file_name).collect::<Vec<_>>(),
                }))?
            );
        }
    }
    Ok(())
}
