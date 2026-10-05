use clap::Subcommand;
use faris_engine::transport::{TransportArtifact, normalize_transport_artifact};
use faris_model::{
    LoadedScenario,
    transport::{MAX_ARTIFACT_BYTES, TransportRequest},
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
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
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(path)?;
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

pub fn run(command: TransportCommand) -> Result<(), Box<dyn std::error::Error>> {
    if let TransportCommand::Pack { run, output } = &command {
        return pack(run, output);
    }
    let (scenario_path, request_path, artifact_output) = match command {
        TransportCommand::ValidateRequest { scenario, request } => (scenario, request, None),
        TransportCommand::Normalize {
            scenario,
            request,
            artifact,
            output,
        } => (scenario, request, Some((artifact, output))),
        TransportCommand::Pack { .. } => unreachable!("handled above"),
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
