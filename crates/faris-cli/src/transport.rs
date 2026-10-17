use clap::Subcommand;
use faris_engine::transport::{TransportArtifact, normalize_transport_artifact};
use faris_model::{
    LoadedScenario,
    transport::{MAX_ARTIFACT_BYTES, TransportRequest},
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Subcommand)]
pub enum TransportCommand {
    /// Validate definitions against exact scenario bytes without running physics.
    ValidateRequest {
        #[arg(long)]
        scenario: PathBuf,
        #[arg(long)]
        request: PathBuf,
    },
    /// Convert solver-reported per-source scores to physical rates and densities.
    Normalize {
        #[arg(long)]
        scenario: PathBuf,
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        artifact: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Run the statistical checks on a run's per-batch tally values and show each tally with an
    /// interval that holds at low counts; a failing tally gets no interval and no verdict.
    Statistics {
        /// transport-batch-values.json written beside the artifact.
        #[arg(long)]
        batch_values: PathBuf,
        /// transport-artifact.json of the same run, to check its reported errors against the batches.
        #[arg(long)]
        artifact: Option<PathBuf>,
        /// Apply the point-detector relative error guideline (5 %) instead of the region one (10 %).
        #[arg(long)]
        point_detector: bool,
        /// Write the full report as JSON to this new file as well.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Package one completed run.json and its bound files into a portable bundle.
    Pack {
        /// A run.json beside its scenario, input, audit, adapter and tally files.
        #[arg(long)]
        run: PathBuf,
        /// New bundle path; an existing file is never overwritten.
        #[arg(long)]
        output: PathBuf,
    },
}

pub fn read_bounded(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if !std::fs::metadata(path)?.is_file() {
        return Err("input must be a regular file".into());
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take((MAX_ARTIFACT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_ARTIFACT_BYTES {
        return Err("input exceeds 16 MiB limit".into());
    }
    Ok(bytes)
}

pub fn write_new_json(
    path: &Path,
    value: &impl serde::Serialize,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    write_new_bytes(path, &bytes)
}

pub fn write_new_bytes(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(faris_engine::atomic_file::parent_directory(path))?;
    faris_engine::atomic_file::write_new(path, bytes)?;
    Ok(())
}

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn pack(run: &Path, output: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let bundle = faris_engine::core_evidence::pack_transport(run).map_err(|e| e.to_string())?;
    let (_, record) = bundle.verify().map_err(|e| e.to_string())?;
    let mut bytes = serde_json::to_vec_pretty(&bundle)?;
    bytes.push(b'\n');
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    faris_engine::core_evidence::write_new(output, &bytes).map_err(|e| e.to_string())?;
    println!(
        "Packed {} ({} files) to {}. Verified; scientific qualification: NOT_EVALUATED.",
        record.variant_id,
        bundle.files.len(),
        output.display()
    );
    Ok(())
}

/// The statistics report as text; writes the JSON report when `output` is given.
fn statistics(
    batch_values: &Path,
    artifact: Option<&Path>,
    point_detector: bool,
    output: Option<&Path>,
) -> Result<String, Box<dyn std::error::Error>> {
    use faris_engine::tally_health::{
        BatchValuesFile, assess_batch_values, check_lines, fom_basis_text, status_line,
    };
    use faris_model::tally_statistics::TallyKind;
    let file = BatchValuesFile::from_bytes(&read_bounded(batch_values)?)?;
    let mut reported = std::collections::BTreeMap::new();
    if let Some(path) = artifact {
        let artifact = TransportArtifact::from_bytes(&read_bounded(path)?)?;
        for tally in &artifact.tallies {
            reported.insert(tally.response_id.clone(), tally.standard_error);
        }
    }
    let kind = if point_detector {
        TallyKind::PointDetector
    } else {
        TallyKind::Volume
    };
    let reports = assess_batch_values(&file, &reported, kind);
    let mut text = String::new();
    for r in &reports {
        text.push_str(&format!("{}\n  value: {}\n", r.response_id, r.value_text));
        text.push_str(&format!("  status: {}\n", status_line(r.health.status)));
        text.push_str(&format!("  interval: {}\n", r.interval_text));
        if let Some(rel) = r.health.relative_error {
            text.push_str(&format!("  relative error: {:.2} %", 100.0 * rel));
            if let Some(v) = r.health.variance_of_variance {
                text.push_str(&format!(", variance of the variance: {v:.3}"));
            }
            text.push('\n');
        }
        if let Some(n) = r.health.effective_histories {
            let flag = if r.health.low_sample {
                " (LOW SAMPLE)"
            } else {
                ""
            };
            text.push_str(&format!("  effective scoring histories: {n:.0}{flag}\n"));
        }
        if let Some(f) = &r.health.fom {
            text.push_str(&format!(
                "  figure of merit: {:.4e} {}, largest change over the last half {:.1} %\n",
                f.final_value,
                fom_basis_text(f.basis),
                100.0 * f.max_change_last_half
            ));
        }
        if let Some(c) = &r.reported_error_check {
            text.push_str(&format!(
                "  reported standard error {:.3e} against batch standard error {:.3e}: z = {:.1}\n",
                c.reported, c.independent, c.z
            ));
        }
        for line in check_lines(r) {
            text.push_str(&format!("  {line}\n"));
        }
    }
    if let Some(c) = faris_engine::tally_health::mesh_reports_convergence(&reports) {
        text.push_str(&format!(
            "Mesh convergence: {}\n",
            faris_engine::tally_health::convergence_line_for(&c, "voxels")
        ));
    }
    if let Some(path) = output {
        write_new_json(path, &reports)?;
    }
    Ok(text)
}

pub fn run(command: TransportCommand) -> Result<(), Box<dyn std::error::Error>> {
    if let TransportCommand::Pack { run, output } = &command {
        return pack(run, output);
    }
    if let TransportCommand::Statistics {
        batch_values,
        artifact,
        point_detector,
        output,
    } = &command
    {
        let text = statistics(
            batch_values,
            artifact.as_deref(),
            *point_detector,
            output.as_deref(),
        )?;
        print!("{text}");
        return Ok(());
    }
    let (scenario_path, request_path, artifact_output) = match command {
        TransportCommand::ValidateRequest { scenario, request } => (scenario, request, None),
        TransportCommand::Normalize {
            scenario,
            request,
            artifact,
            output,
        } => (scenario, request, Some((artifact, output))),
        TransportCommand::Pack { .. } | TransportCommand::Statistics { .. } => {
            unreachable!("handled above")
        }
    };
    let scenario = LoadedScenario::from_bytes(&read_bounded(&scenario_path)?)?;
    let request_bytes = read_bounded(&request_path)?;
    let request: TransportRequest = serde_json::from_slice(&request_bytes)?;
    request.validate_against(&scenario)?;
    if let Some((artifact_path, output)) = artifact_output {
        let artifact_bytes = read_bounded(&artifact_path)?;
        let artifact = TransportArtifact::from_bytes(&artifact_bytes)?;
        let result = normalize_transport_artifact(&request, &artifact, &scenario)?;
        // Provenance and scientific qualification are deliberately separate.
        let record = serde_json::json!({
            "schema_version": "faris-transport-import/v0.1",
            "operation": "normalization_of_adapter_reported_tallies",
            "scientific_qualification": "NOT_EVALUATED",
            "request_sha256": sha256(&request_bytes),
            "raw_artifact_sha256": sha256(&artifact_bytes),
            "normalized": result,
            "scope": "Checked identities, response definitions, dimensions, and arithmetic. Solver/data identities and raw values are adapter assertions; this import does not authenticate their origin or establish physical validity."
        });
        write_new_json(&output, &record)?;
        println!(
            "Normalized {} responses to {}. Scientific qualification: NOT_EVALUATED.",
            result.results.len(),
            output.display()
        );
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "request_valid": true, "scenario_sha256": scenario.source_sha256,
                "request_sha256": sha256(&request_bytes),
                "responses": request.responses.len(), "solver_execution": "NOT_RUN",
                "scientific_qualification": "NOT_EVALUATED"
            }))?
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn pack_help_names_run_and_output() {
        let error = match crate::Arguments::try_parse_from(["faris", "transport", "pack", "--help"])
        {
            Ok(_) => panic!("--help should stop argument parsing"),
            Err(error) => error,
        };
        let help = error.to_string();
        for option in ["--run", "--output"] {
            assert!(help.contains(option), "missing {option} in help: {help}");
        }
    }

    // Verifies: UNC-005
    #[test]
    fn statistics_prints_each_tally_and_refuses_a_spiky_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transport-batch-values.json");
        let good: Vec<f64> = (0..100)
            .map(|i| 1.0 + 0.05 * (((i * 37) % 17) as f64 / 8.0 - 1.0))
            .collect();
        let mut spiky = good.clone();
        spiky[10] = 40.0;
        let doc = serde_json::json!({
            "schema_version": "faris-transport-batch-values/v0.1",
            "n_batches": 100,
            "response_ids": ["good", "spiky"],
            "values": {"good": good, "spiky": spiky},
        });
        std::fs::write(&path, doc.to_string()).unwrap();
        let out = dir.path().join("report.json");
        let text = statistics(&path, None, false, Some(&out)).unwrap();
        assert!(
            text.contains("good\n") && text.contains("spiky\n"),
            "{text}"
        );
        assert!(text.contains("(1 σ standard error)"), "{text}");
        assert!(text.contains("FAIL: a statistical check failed"), "{text}");
        assert!(text.contains("NOT_EVALUATED: interval refused"), "{text}");
        assert!(out.exists());
        assert!(statistics(&dir.path().join("none.json"), None, false, None).is_err());
        assert!(!text.contains("Mesh convergence"), "{text}");
    }

    // Verifies: NUC-049
    #[test]
    fn statistics_prints_the_mesh_convergence_sentence_for_a_flux_mesh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transport-batch-values.json");
        // Voxel 0 is tight (about 1 % error), voxel 1 is wide (about 50 %), voxel 2 scores nothing.
        let tight: Vec<f64> = (0..100)
            .map(|i| 1.0 + 0.1 * (((i * 37) % 17) as f64 / 8.0 - 1.0))
            .collect();
        let wide: Vec<f64> = (0..100)
            .map(|i| 1.0 + 5.0 * (((i * 37) % 17) as f64 / 8.0 - 1.0))
            .collect();
        let doc = serde_json::json!({
            "schema_version": "faris-transport-batch-values/v0.1",
            "n_batches": 100,
            "response_ids": ["mesh-flux-0", "mesh-flux-1", "mesh-flux-2"],
            "values": {"mesh-flux-0": tight, "mesh-flux-1": wide, "mesh-flux-2": vec![0.0; 100]},
        });
        std::fs::write(&path, doc.to_string()).unwrap();
        let text = statistics(&path, None, false, None).unwrap();
        assert!(
            text.contains("Mesh convergence: 1 of 3 voxels (33.3 %)"),
            "{text}"
        );
    }

    #[test]
    fn pack_refuses_a_missing_run_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("bundle.json");
        assert!(pack(&dir.path().join("run.json"), &output).is_err());
        assert!(!output.exists());
    }

    #[test]
    fn publishes_a_complete_record_without_overwriting_existing_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("record.json");
        let original = serde_json::json!({"state": "NOT_EVALUATED"});
        write_new_json(&path, &original).unwrap();
        assert!(write_new_json(&path, &serde_json::json!({"state": "PASS"})).is_err());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&read_bounded(&path).unwrap()).unwrap(),
            original
        );
    }
    // Verifies: SEC-003
    #[test]
    fn special_files_cannot_bypass_input_bounds() {
        #[cfg(unix)]
        assert!(read_bounded(Path::new("/dev/zero")).is_err());
    }
}
