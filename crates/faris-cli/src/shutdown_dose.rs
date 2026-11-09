// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! Import a hashed D1S/R2S shutdown-dose comparison request and write its report.

use crate::transport::{read_bounded, write_new_json};
use clap::Subcommand;
use faris_engine::shutdown_dose::{CompareRequest, CoolingResult, Status, compare, parse_request};
use faris_model::validation::Quantity;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum ShutdownDoseCommand {
    /// Compare already-run spatial D1S and R2S estimates; this command runs no solver.
    Compare {
        /// faris-shutdown-dose-compare-request/v0.1 JSON (record paths are relative to it).
        #[arg(long)]
        request: PathBuf,
        /// New faris-shutdown-dose-comparison/v0.1 report; existing paths are refused.
        #[arg(long)]
        output: PathBuf,
    },
}

pub fn run(command: ShutdownDoseCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        ShutdownDoseCommand::Compare { request, output } => {
            if output.exists() {
                return Err(format!("refusing to overwrite {}", output.display()).into());
            }
            let bytes = read_bounded(&request)?;
            let parsed: CompareRequest = parse_request(&bytes)?;
            let report = compare(&parsed, &request)?;
            write_new_json(&output, &report)?;
            for point in &report.points {
                println!("{}", summary_line(point));
            }
            println!("Comparison report recorded at {}", output.display());
            eprintln!("{}", faris_model::RESEARCH_SCREENING_STATEMENT);
            Ok(())
        }
    }
}

fn summary_line(point: &CoolingResult) -> String {
    let status = match point.status {
        Status::Pass => "PASS",
        Status::Fail => "FAIL",
        Status::Inconclusive => "INCONCLUSIVE",
    };
    let central_band = match point.ratio_in_band {
        Some(true) => "INSIDE",
        Some(false) => "OUTSIDE",
        None => "UNDEFINED",
    };
    let ratio = point
        .ratio_d1s_over_r2s
        .map_or_else(|| "undefined".into(), |v| format!("{v:.6e}"));
    let ratio_se = point
        .ratio_standard_error
        .map_or_else(|| "not reported".into(), |v| format!("{v:.6e}"));
    let validation = point.validation.tag("value", Quantity::ShutdownDoseRate);
    let ratio_validation = point
        .validation
        .tag("ratio_d1s_over_r2s", Quantity::ShutdownDoseRatio);
    let reason = point.reason.as_deref().unwrap_or("none");
    let next_step = point.next_step.as_deref().unwrap_or("none");
    format!(
        "{} s: central ratio {central_band} band, {ratio_validation}: {ratio} ± {ratio_se}, status {status}, covariance {}, dose {validation}; reason: {reason}; next step: {next_step}",
        point.cooling_time_s, point.covariance_assumption
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::path::Path;

    fn sha(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn record(root: &Path, method: &str, value: f64) -> (String, String) {
        let dir = root.join(method);
        std::fs::create_dir_all(&dir).unwrap();
        let mut hashes = serde_json::Map::new();
        for (role, bytes) in [
            ("geometry", b"geometry".as_slice()),
            ("materials", b"materials".as_slice()),
            ("source", b"source".as_slice()),
            ("irradiation", b"irradiation".as_slice()),
            ("response", b"response".as_slice()),
            ("location", b"location".as_slice()),
        ] {
            let path = root.join(format!("{role}.bin"));
            std::fs::write(&path, bytes).unwrap();
            hashes.insert(format!("{role}_sha256"), sha(bytes).into());
        }
        let err = value * 0.02;
        let streams = vec![format!("{method}-stream")];
        let response = serde_json::json!({
            "quantity": "icrp116_ap_effective_dose_rate",
            "unit": "uSv/h",
            "normalization": "whole scored volume"
        });
        let run = serde_json::json!({
            "method": method,
            "cooling_time_s": 86400.0,
            "random_streams": streams.clone(),
            "estimate": {"value": value, "standard_error": err},
            "response": response.clone(),
            "artifacts": hashes
        });
        let run_bytes = serde_json::to_vec(&run).unwrap();
        std::fs::write(dir.join("run.json"), &run_bytes).unwrap();
        let record = serde_json::json!({
            "schema": "faris-shutdown-dose-estimate/v0.1",
            "method": method,
            "cooling_time_s": 86400.0,
            "quantity": response["quantity"],
            "unit": response["unit"],
            "normalization": response["normalization"],
            "location": {"path": "../location.bin", "sha256": sha(b"location")},
            "estimate": {"value": value, "standard_error": err},
            "geometry": {"path": "../geometry.bin", "sha256": sha(b"geometry")},
            "materials": {"path": "../materials.bin", "sha256": sha(b"materials")},
            "source": {"path": "../source.bin", "sha256": sha(b"source")},
            "irradiation": {"path": "../irradiation.bin", "sha256": sha(b"irradiation")},
            "response": {"path": "../response.bin", "sha256": sha(b"response")},
            "run_record": {"path": "run.json", "sha256": sha(&run_bytes)},
            "random_streams": streams
        });
        let bytes = serde_json::to_vec(&record).unwrap();
        std::fs::write(dir.join("estimate.json"), &bytes).unwrap();
        (format!("{method}/estimate.json"), sha(&bytes))
    }

    #[test]
    fn compare_command_reads_hash_bound_records_and_writes_a_report() {
        let dir = tempfile::tempdir().unwrap();
        let (d1s, d1_hash) = record(dir.path(), "d1s", 1.20);
        let (r2s, r2_hash) = record(dir.path(), "r2s", 1.0);
        let request = serde_json::json!({
            "schema": "faris-shutdown-dose-compare-request/v0.1",
            "case_id": "cli-test",
            "points": [{
                "cooling_time_s": 86400.0,
                "d1s": {"path": d1s, "sha256": d1_hash},
                "r2s": {"path": r2s, "sha256": r2_hash},
                "covariance": {"kind": "unknown"}
            }]
        });
        let request_path = dir.path().join("request.json");
        std::fs::write(&request_path, serde_json::to_vec(&request).unwrap()).unwrap();
        let output = dir.path().join("report.json");
        run(ShutdownDoseCommand::Compare {
            request: request_path,
            output: output.clone(),
        })
        .unwrap();
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(report["schema"], "faris-shutdown-dose-comparison/v0.1");
        assert_eq!(report["points"][0]["status"], "INCONCLUSIVE");
        assert!(report["points"][0]["ratio_standard_error"].is_null());
        let typed: faris_engine::shutdown_dose::CompareReport =
            serde_json::from_value(report).unwrap();
        let line = summary_line(&typed.points[0]);
        assert!(line.contains("central ratio OUTSIDE band"));
        assert!(line.contains("INCONCLUSIVE"));
        assert!(line.contains("not validated"));
    }
}
