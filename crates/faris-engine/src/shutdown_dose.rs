// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! Import and compare already-run D1S and R2S shutdown-dose estimates.
//!
//! This module never runs a solver and never treats ACTINV contact-dose proxies as a transported
//! dose. Estimates are bound to their exact JSON record bytes and to hashed geometry, materials,
//! source, irradiation, response-coefficient, and run-record artifacts. Equal hashes establish
//! input identity, not scientific correctness.

use faris_model::math;
use faris_model::validation::{Output, Quantity, ValidationMarks, calculated, not_calculated};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const REQUEST_SCHEMA: &str = "faris-shutdown-dose-compare-request/v0.1";
pub const ESTIMATE_SCHEMA: &str = "faris-shutdown-dose-estimate/v0.1";
pub const REPORT_SCHEMA: &str = "faris-shutdown-dose-comparison/v0.1";
pub const RATIO_LOWER: f64 = 0.85;
pub const RATIO_UPPER: f64 = 1.15;

/// Numeric fields emitted by a D1S/R2S comparison report.
pub const SHUTDOWN_DOSE_OUTPUTS: &[Output] = &[
    calculated("value", Quantity::ShutdownDoseRate),
    calculated("standard_error", Quantity::ShutdownDoseRate),
    calculated("ratio_d1s_over_r2s", Quantity::ShutdownDoseRatio),
    calculated("ratio_standard_error", Quantity::ShutdownDoseRatio),
    not_calculated(
        "cooling_time_s",
        "authored time since shutdown identifying this comparison point",
    ),
    not_calculated(
        "ratio_band",
        "fixed comparison acceptance band, not a physical result",
    ),
];

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    D1s,
    R2s,
}

impl Method {
    fn as_str(self) -> &'static str {
        match self {
            Self::D1s => "d1s",
            Self::R2s => "r2s",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
    /// Relative to the estimate record containing this reference, or absolute.
    pub path: PathBuf,
    /// SHA-256 of the exact referenced file bytes.
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Estimate {
    pub value: f64,
    /// One-standard-error estimate in the same unit as `value`.
    pub standard_error: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DoseEstimateRecord {
    pub schema: String,
    pub method: Method,
    pub cooling_time_s: f64,
    /// Stable physical response identifier, for example `icrp116_ap_effective_dose_rate`.
    pub quantity: String,
    pub unit: String,
    /// Exact scoring and normalization description; it must match between methods.
    pub normalization: String,
    /// Hashed spatial region/volume definition used by the tally; must match between methods.
    pub location: ArtifactRef,
    pub estimate: Estimate,
    /// Actual input artifacts used by the run. The tool verifies each referenced file hash.
    pub geometry: ArtifactRef,
    pub materials: ArtifactRef,
    pub source: ArtifactRef,
    pub irradiation: ArtifactRef,
    /// Dose-response coefficient data or other response-defining input.
    pub response: ArtifactRef,
    /// Solver run record. Its method, cooling time, and random stream identities are checked.
    pub run_record: ArtifactRef,
    /// Random-stream IDs copied from the run record, not invented by the comparison request.
    pub random_streams: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RecordInput {
    pub path: PathBuf,
    /// Expected SHA-256 supplied by the author of the comparison request.
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CoolingPair {
    pub cooling_time_s: f64,
    pub d1s: RecordInput,
    pub r2s: RecordInput,
    #[serde(default)]
    pub covariance: CovarianceAssessment,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CovarianceAssessment {
    /// Fail-closed default: no ratio uncertainty is reported.
    #[default]
    Unknown,
    /// A user-authored independence assumption with evidence explaining its basis.
    /// Different random-stream names alone do not establish independence.
    IndependentAssumed {
        rationale: String,
        evidence: ArtifactRef,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CompareRequest {
    pub schema: String,
    pub case_id: String,
    pub points: Vec<CoolingPair>,
}

/// Parses a user-supplied comparison request. Exposed so the CLI and fuzz entry share this parser.
pub fn parse_request(bytes: &[u8]) -> Result<CompareRequest, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("shutdown-dose request JSON: {e}"))
}

/// Parses a method estimate record. Exposed so the CLI and fuzz entry share this parser.
pub fn parse_estimate_record(bytes: &[u8]) -> Result<DoseEstimateRecord, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("shutdown-dose estimate JSON: {e}"))
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Status {
    Pass,
    Fail,
    Inconclusive,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MethodResult {
    pub method: Method,
    pub record_sha256: String,
    pub value: f64,
    pub standard_error: f64,
    pub random_streams: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CoolingResult {
    pub cooling_time_s: f64,
    pub quantity: String,
    pub unit: String,
    pub normalization: String,
    pub location_sha256: String,
    pub ratio_d1s_over_r2s: Option<f64>,
    /// Present only under an explicit, evidence-bound independence assumption.
    pub ratio_standard_error: Option<f64>,
    pub ratio_in_band: Option<bool>,
    pub status: Status,
    pub reason: Option<String>,
    pub next_step: Option<String>,
    pub covariance_assumption: String,
    pub covariance_evidence_sha256: Option<String>,
    pub d1s: MethodResult,
    pub r2s: MethodResult,
    pub basis_sha256: BasisHashes,
    pub validation: ValidationMarks,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BasisHashes {
    pub geometry: String,
    pub materials: String,
    pub source: String,
    pub irradiation: String,
    pub response: String,
    pub location: String,
    pub run_record_d1s: String,
    pub run_record_r2s: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CompareReport {
    pub schema: String,
    pub case_id: String,
    pub method: String,
    pub ratio_band: [f64; 2],
    pub interpretation: String,
    pub points: Vec<CoolingResult>,
}

#[derive(Clone, Debug)]
struct LoadedRecord {
    record: DoseEstimateRecord,
    record_sha256: String,
}

pub fn compare(request: &CompareRequest, request_path: &Path) -> Result<CompareReport, String> {
    if request.schema != REQUEST_SCHEMA {
        return Err(format!("schema must be {REQUEST_SCHEMA}"));
    }
    if request.case_id.trim().is_empty() || request.case_id.len() > 256 || request.points.is_empty()
    {
        return Err("case_id and at least one cooling point are required".into());
    }
    if request.points.len() > 100 {
        return Err("a comparison request may contain at most 100 cooling points".into());
    }
    let request_dir = request_path.parent().unwrap_or_else(|| Path::new("."));
    let mut out = Vec::with_capacity(request.points.len());
    let mut seen_times = Vec::new();
    for pair in &request.points {
        if !pair.cooling_time_s.is_finite() || pair.cooling_time_s <= 0.0 {
            return Err("cooling_time_s must be finite and positive".into());
        }
        if seen_times.contains(&pair.cooling_time_s) {
            return Err(format!("duplicate cooling time {} s", pair.cooling_time_s));
        }
        seen_times.push(pair.cooling_time_s);
        let d1s = load_record(request_dir, &pair.d1s, Method::D1s, pair.cooling_time_s)?;
        let r2s = load_record(request_dir, &pair.r2s, Method::R2s, pair.cooling_time_s)?;
        let a = &d1s.record;
        let b = &r2s.record;
        for (name, lhs, rhs) in [
            ("quantity", a.quantity.as_str(), b.quantity.as_str()),
            ("unit", a.unit.as_str(), b.unit.as_str()),
            (
                "normalization",
                a.normalization.as_str(),
                b.normalization.as_str(),
            ),
        ] {
            if lhs.trim().is_empty() || lhs != rhs {
                return Err(format!("D1S and R2S {name} do not match"));
            }
        }
        let mut basis = compare_basis(a, b)?;
        basis.run_record_d1s = a.run_record.sha256.to_ascii_lowercase();
        basis.run_record_r2s = b.run_record.sha256.to_ascii_lowercase();
        let ratio = if b.estimate.value > 0.0 {
            Some(a.estimate.value / b.estimate.value)
        } else {
            None
        };
        let (covariance_assumption, covariance_evidence_sha256, independent) =
            match &pair.covariance {
                CovarianceAssessment::Unknown => ("unknown".to_owned(), None, false),
                CovarianceAssessment::IndependentAssumed {
                    rationale,
                    evidence,
                } => {
                    if rationale.trim().is_empty() || rationale.len() > 2048 {
                        return Err(
                        "independent covariance assumption needs a rationale of at most 2048 bytes"
                            .into(),
                    );
                    }
                    verify_artifact(&request_dir.join("request.json"), evidence)?;
                    (
                        format!("independent_assumed: {rationale}"),
                        Some(hash_file(&resolve(request_dir, &evidence.path))?),
                        true,
                    )
                }
            };
        let ratio_se = if independent {
            ratio
                .map(|_| {
                    let first = a.estimate.standard_error / b.estimate.value;
                    let second =
                        ratio.unwrap_or(f64::NAN) * b.estimate.standard_error / b.estimate.value;
                    math::hypot(first, second)
                })
                .filter(|x| x.is_finite())
        } else {
            None
        };
        let ratio = ratio.filter(|x| x.is_finite());
        let ratio_in_band = ratio.map(|r| (RATIO_LOWER..=RATIO_UPPER).contains(&r));
        let (status, reason, next_step) = match (ratio, independent, ratio_se) {
            (None, _, _) if b.estimate.value == 0.0 => (
                Status::Inconclusive,
                Some("R2S estimate is zero; ratio is undefined".into()),
                Some("check the source tally and its normalization; do not infer a ratio".into()),
            ),
            (None, _, _) => (
                Status::Inconclusive,
                Some("ratio or propagated arithmetic is non-finite".into()),
                Some("check the estimate magnitudes and units in both imported records".into()),
            ),
            (Some(_), false, _) => (
                Status::Inconclusive,
                Some("covariance is unknown; no ratio uncertainty is reported".into()),
                Some(
                    "supply a supported covariance assessment or retain the inconclusive result"
                        .into(),
                ),
            ),
            (Some(_), true, None) => (
                Status::Inconclusive,
                Some("independence was assumed but propagated uncertainty is non-finite".into()),
                Some("check estimate scales and uncertainty inputs".into()),
            ),
            (Some(_), true, Some(_)) if ratio_in_band == Some(true) => (Status::Pass, None, None),
            (Some(_), true, Some(_)) => (
                Status::Fail,
                Some("central D1S/R2S ratio is outside 0.85–1.15".into()),
                Some(
                    "review the matched inputs and method differences; do not adjust the band"
                        .into(),
                ),
            ),
        };
        out.push(CoolingResult {
            cooling_time_s: pair.cooling_time_s,
            quantity: a.quantity.clone(),
            unit: a.unit.clone(),
            normalization: a.normalization.clone(),
            location_sha256: a.location.sha256.to_ascii_lowercase(),
            ratio_d1s_over_r2s: ratio,
            ratio_standard_error: ratio_se,
            ratio_in_band,
            status,
            reason,
            next_step,
            covariance_assumption,
            covariance_evidence_sha256,
            d1s: MethodResult {
                method: Method::D1s,
                record_sha256: d1s.record_sha256,
                value: a.estimate.value,
                standard_error: a.estimate.standard_error,
                random_streams: a.random_streams.clone(),
            },
            r2s: MethodResult {
                method: Method::R2s,
                record_sha256: r2s.record_sha256,
                value: b.estimate.value,
                standard_error: b.estimate.standard_error,
                random_streams: b.random_streams.clone(),
            },
            basis_sha256: basis,
            validation: ValidationMarks::for_catalogue(SHUTDOWN_DOSE_OUTPUTS),
        });
    }
    Ok(CompareReport {
        schema: REPORT_SCHEMA.into(),
        case_id: request.case_id.clone(),
        method: "imported estimates; no solver executed; hashes verify byte identity, not scientific correctness".into(),
        ratio_band: [RATIO_LOWER, RATIO_UPPER],
        interpretation: "D1S/R2S ratio of the same spatially integrated response, location, units and normalization. Standard error is propagated only under an explicit evidence-bound independence assumption; this assumption is not established by different seed or stream labels. Unknown covariance yields INCONCLUSIVE.".into(),
        points: out,
    })
}

fn load_record(
    dir: &Path,
    input: &RecordInput,
    method: Method,
    time_s: f64,
) -> Result<LoadedRecord, String> {
    validate_hash(&input.sha256, "expected record SHA-256")?;
    let path = resolve(dir, &input.path);
    let bytes = read_record_bytes(&path)?;
    let digest = sha256(&bytes);
    if !digest.eq_ignore_ascii_case(&input.sha256) {
        return Err(format!(
            "{}: SHA-256 does not match the request",
            path.display()
        ));
    }
    let record = parse_estimate_record(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if record.schema != ESTIMATE_SCHEMA || record.method != method {
        return Err(format!(
            "{}: expected {ESTIMATE_SCHEMA} with method {}",
            path.display(),
            method.as_str()
        ));
    }
    if !close_time(record.cooling_time_s, time_s) {
        return Err(format!(
            "{}: cooling time does not match the request",
            path.display()
        ));
    }
    if record.quantity.trim().is_empty()
        || record.unit.trim().is_empty()
        || record.normalization.trim().is_empty()
        || record.quantity.len() > 256
        || record.unit.len() > 64
        || record.normalization.len() > 2048
    {
        return Err(format!(
            "{}: quantity, unit and normalization are required",
            path.display()
        ));
    }
    if !record.estimate.value.is_finite()
        || record.estimate.value < 0.0
        || !record.estimate.standard_error.is_finite()
        || record.estimate.standard_error < 0.0
    {
        return Err(format!(
            "{}: estimate and standard error must be finite and nonnegative",
            path.display()
        ));
    }
    if record.random_streams.is_empty()
        || record.random_streams.len() > 128
        || record
            .random_streams
            .iter()
            .any(|s| s.trim().is_empty() || s.len() > 256)
    {
        return Err(format!(
            "{}: run record must identify at least one random stream",
            path.display()
        ));
    }
    let mut streams = record.random_streams.clone();
    streams.sort();
    streams.dedup();
    if streams.len() != record.random_streams.len() {
        return Err(format!(
            "{}: random stream IDs must be unique",
            path.display()
        ));
    }
    for artifact in [
        &record.geometry,
        &record.materials,
        &record.source,
        &record.irradiation,
        &record.response,
        &record.location,
        &record.run_record,
    ] {
        verify_artifact(&path, artifact)?;
    }
    let run_path = resolve(
        path.parent().unwrap_or_else(|| Path::new(".")),
        &record.run_record.path,
    );
    let run_bytes = read_record_bytes(&run_path)?;
    let run: serde_json::Value = serde_json::from_slice(&run_bytes)
        .map_err(|e| format!("{}: run record JSON: {e}", run_path.display()))?;
    let run_time = run.get("cooling_time_s").and_then(|v| v.as_f64());
    let run_estimate = run.get("estimate");
    let estimate_matches = run_estimate
        .and_then(|v| v.get("value"))
        .and_then(|v| v.as_f64())
        == Some(record.estimate.value)
        && run_estimate
            .and_then(|v| v.get("standard_error"))
            .and_then(|v| v.as_f64())
            == Some(record.estimate.standard_error);
    let response_matches = [
        ("quantity", &record.quantity),
        ("unit", &record.unit),
        ("normalization", &record.normalization),
    ]
    .iter()
    .all(|(key, expected)| {
        run.get("response")
            .and_then(|a| a.get(key))
            .and_then(|v| v.as_str())
            == Some(expected.as_str())
    });
    let run_streams = run
        .get("random_streams")
        .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok());
    let basis_matches = [
        ("geometry_sha256", &record.geometry.sha256),
        ("materials_sha256", &record.materials.sha256),
        ("source_sha256", &record.source.sha256),
        ("irradiation_sha256", &record.irradiation.sha256),
        ("response_sha256", &record.response.sha256),
        ("location_sha256", &record.location.sha256),
    ]
    .iter()
    .all(|(key, expected)| {
        run.get("artifacts")
            .and_then(|a| a.get(key))
            .and_then(|v| v.as_str())
            .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
    });
    if run.get("method").and_then(|v| v.as_str()) != Some(method.as_str())
        || run_time.is_none_or(|v| !close_time(v, time_s))
        || run_streams.as_ref() != Some(&record.random_streams)
        || !basis_matches
        || !estimate_matches
        || !response_matches
    {
        return Err(format!(
            "{}: run record method, cooling time, estimate, response, artifact hashes, or random streams do not match the estimate",
            run_path.display()
        ));
    }
    Ok(LoadedRecord {
        record,
        record_sha256: digest,
    })
}

fn compare_basis(a: &DoseEstimateRecord, b: &DoseEstimateRecord) -> Result<BasisHashes, String> {
    let pairs = [
        ("geometry", &a.geometry.sha256, &b.geometry.sha256),
        ("materials", &a.materials.sha256, &b.materials.sha256),
        ("source", &a.source.sha256, &b.source.sha256),
        ("irradiation", &a.irradiation.sha256, &b.irradiation.sha256),
        ("response", &a.response.sha256, &b.response.sha256),
        ("location", &a.location.sha256, &b.location.sha256),
    ];
    for (name, x, y) in pairs {
        if !x.eq_ignore_ascii_case(y) {
            return Err(format!("D1S and R2S {name} artifact hashes differ"));
        }
    }
    Ok(BasisHashes {
        geometry: a.geometry.sha256.to_ascii_lowercase(),
        materials: a.materials.sha256.to_ascii_lowercase(),
        source: a.source.sha256.to_ascii_lowercase(),
        irradiation: a.irradiation.sha256.to_ascii_lowercase(),
        response: a.response.sha256.to_ascii_lowercase(),
        location: a.location.sha256.to_ascii_lowercase(),
        run_record_d1s: String::new(),
        run_record_r2s: String::new(),
    })
}

fn verify_artifact(record_path: &Path, artifact: &ArtifactRef) -> Result<(), String> {
    validate_hash(&artifact.sha256, "artifact SHA-256")?;
    let path = resolve(
        record_path.parent().unwrap_or_else(|| Path::new(".")),
        &artifact.path,
    );
    let digest = hash_file(&path)?;
    if !digest.eq_ignore_ascii_case(&artifact.sha256) {
        return Err(format!("{}: artifact SHA-256 mismatch", path.display()));
    }
    Ok(())
}

fn resolve(dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        dir.join(path)
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn open_regular_file(path: &Path) -> Result<std::fs::File, String> {
    let metadata = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "{}: expected a regular file; special files and directories are refused",
            path.display()
        ));
    }
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !file
        .metadata()
        .map_err(|e| format!("{}: {e}", path.display()))?
        .is_file()
    {
        return Err(format!(
            "{}: opened object is not a regular file; special files and directories are refused",
            path.display()
        ));
    }
    Ok(file)
}

fn hash_file(path: &Path) -> Result<String, String> {
    let file = open_regular_file(path)?;
    let mut input = std::io::BufReader::new(file);
    let mut digest = Sha256::new();
    let mut chunk = [0; 64 * 1024];
    loop {
        let n = input
            .read(&mut chunk)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        digest.update(&chunk[..n]);
    }
    let digest = digest.finalize();
    Ok(format!("{digest:x}"))
}

fn read_record_bytes(path: &Path) -> Result<Vec<u8>, String> {
    const MAX_RECORD_BYTES: u64 = 16 * 1024 * 1024;
    let file = open_regular_file(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(format!("{}: record exceeds 16 MiB limit", path.display()));
    }
    Ok(bytes)
}

fn validate_hash(hash: &str, label: &str) -> Result<(), String> {
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{label} must be 64 hexadecimal characters"));
    }
    Ok(())
}

fn close_time(a: f64, b: f64) -> bool {
    a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(path: &Path) -> String {
        sha256(&std::fs::read(path).unwrap())
    }

    fn artifact(path: &str, bytes: &[u8]) -> ArtifactRef {
        ArtifactRef {
            path: path.into(),
            sha256: sha256(bytes),
        }
    }

    fn setup(
        root: &Path,
        d1_value: f64,
        r2_value: f64,
        shared_stream: bool,
        mismatch_material: bool,
    ) -> CompareRequest {
        std::fs::create_dir_all(root).unwrap();
        let geometry = b"geometry";
        let materials = b"materials";
        let other_materials = b"different materials";
        let source = b"source";
        let irradiation = b"irradiation";
        let response = b"response";
        let location_bytes = b"scored region";
        let other_location = b"other scored region";
        for (name, bytes) in [
            ("geometry.bin", geometry.as_slice()),
            ("materials.bin", materials.as_slice()),
            ("other-materials.bin", other_materials.as_slice()),
            ("source.bin", source.as_slice()),
            ("irradiation.bin", irradiation.as_slice()),
            ("response.bin", response.as_slice()),
            ("location.bin", location_bytes.as_slice()),
            ("other-location.bin", other_location.as_slice()),
        ] {
            std::fs::write(root.join(name), bytes).unwrap();
        }
        let streams_d1 = vec!["d1s-seed-11".to_owned()];
        let streams_r2 = vec![
            if shared_stream {
                "d1s-seed-11"
            } else {
                "r2s-seed-22"
            }
            .to_owned(),
        ];
        let make = |method: Method, value: f64, streams: Vec<String>, change_material: bool| {
            let (subdir, file) = match method {
                Method::D1s => ("d1s", "d1s.json"),
                Method::R2s => ("r2s", "r2s.json"),
            };
            let dir = root.join(subdir);
            std::fs::create_dir_all(&dir).unwrap();
            let material_ref = if change_material {
                artifact("../other-materials.bin", other_materials)
            } else {
                artifact("../materials.bin", materials)
            };
            let estimate = Estimate {
                value,
                standard_error: value * 0.02,
            };
            let normalization = "integrated over the same scored region";
            let quantity = "icrp116_ap_effective_dose_rate";
            let unit = "uSv/h";
            let run = serde_json::json!({
                "method": method.as_str(), "cooling_time_s": 86_400.0,
                "random_streams": streams,
                "estimate": estimate,
                "response": {"quantity": quantity, "unit": unit, "normalization": normalization},
                "artifacts": {
                    "geometry_sha256": sha256(geometry),
                    "materials_sha256": material_ref.sha256,
                    "source_sha256": sha256(source),
                    "irradiation_sha256": sha256(irradiation),
                    "response_sha256": sha256(response),
                    "location_sha256": if change_material { sha256(other_location) } else { sha256(location_bytes) }
                }
            });
            let run_bytes = serde_json::to_vec(&run).unwrap();
            std::fs::write(dir.join("run.json"), &run_bytes).unwrap();
            let record = DoseEstimateRecord {
                schema: ESTIMATE_SCHEMA.into(),
                method,
                cooling_time_s: 86_400.0,
                quantity: quantity.into(),
                unit: unit.into(),
                normalization: normalization.into(),
                location: if change_material {
                    artifact("../other-location.bin", other_location)
                } else {
                    artifact("../location.bin", location_bytes)
                },
                estimate,
                geometry: artifact("../geometry.bin", geometry),
                materials: material_ref,
                source: artifact("../source.bin", source),
                irradiation: artifact("../irradiation.bin", irradiation),
                response: artifact("../response.bin", response),
                run_record: artifact("run.json", &run_bytes),
                random_streams: streams,
            };
            let bytes = serde_json::to_vec(&record).unwrap();
            std::fs::write(dir.join(file), bytes).unwrap();
            format!("{subdir}/{file}")
        };
        let d1_path = make(Method::D1s, d1_value, streams_d1, false);
        let r2_path = make(Method::R2s, r2_value, streams_r2, mismatch_material);
        CompareRequest {
            schema: REQUEST_SCHEMA.into(),
            case_id: "fixture".into(),
            points: vec![CoolingPair {
                cooling_time_s: 86_400.0,
                d1s: RecordInput {
                    path: d1_path.clone().into(),
                    sha256: hash(&root.join(d1_path)),
                },
                r2s: RecordInput {
                    path: r2_path.clone().into(),
                    sha256: hash(&root.join(r2_path)),
                },
                covariance: CovarianceAssessment::Unknown,
            }],
        }
    }

    #[test]
    fn reports_ratio_and_propagates_error_only_under_explicit_assumption() {
        let dir = tempfile::tempdir().unwrap();
        let mut req = setup(dir.path(), 1.10, 1.0, false, false);
        std::fs::write(
            dir.path().join("covariance-evidence.md"),
            b"Reviewed independence basis",
        )
        .unwrap();
        req.points[0].covariance = CovarianceAssessment::IndependentAssumed {
            rationale: "independent event sampling was verified for these run records".into(),
            evidence: artifact("covariance-evidence.md", b"Reviewed independence basis"),
        };
        let report = compare(&req, &dir.path().join("request.json")).unwrap();
        let point = &report.points[0];
        assert_eq!(point.status, Status::Pass);
        assert_eq!(point.ratio_d1s_over_r2s, Some(1.10));
        assert!(point.ratio_standard_error.unwrap() > 0.0);
        assert_eq!(point.basis_sha256.materials, sha256(b"materials"));
        assert_eq!(point.basis_sha256.location, sha256(b"scored region"));
        assert!(point.covariance_evidence_sha256.is_some());
    }

    #[test]
    fn every_numeric_report_field_is_in_the_output_catalogue() {
        let dir = tempfile::tempdir().unwrap();
        let request = setup(dir.path(), 1.10, 1.0, false, false);
        let report = compare(&request, &dir.path().join("request.json")).unwrap();
        let json = serde_json::to_value(report).unwrap();
        assert_eq!(
            faris_model::validation::unclassified_numbers(&json, SHUTDOWN_DOSE_OUTPUTS),
            Vec::<String>::new()
        );
    }

    #[test]
    fn covariance_unknown_is_inconclusive_even_when_seed_labels_differ() {
        let dir = tempfile::tempdir().unwrap();
        let req = setup(dir.path(), 1.20, 1.0, true, false);
        let report = compare(&req, &dir.path().join("request.json")).unwrap();
        assert_eq!(report.points[0].status, Status::Inconclusive);
        assert_eq!(report.points[0].ratio_standard_error, None);
        assert_eq!(report.points[0].ratio_in_band, Some(false));
    }

    #[test]
    fn out_of_band_ratio_fails_and_mismatched_physics_artifact_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut req = setup(dir.path(), 1.20, 1.0, false, false);
        std::fs::write(
            dir.path().join("covariance-evidence.md"),
            b"Reviewed independence basis",
        )
        .unwrap();
        req.points[0].covariance = CovarianceAssessment::IndependentAssumed {
            rationale: "verified independent samples".into(),
            evidence: artifact("covariance-evidence.md", b"Reviewed independence basis"),
        };
        let report = compare(&req, &dir.path().join("request.json")).unwrap();
        assert_eq!(report.points[0].status, Status::Fail);
        let dir = tempfile::tempdir().unwrap();
        let req = setup(dir.path(), 1.0, 1.0, false, true);
        assert!(
            compare(&req, &dir.path().join("request.json"))
                .unwrap_err()
                .contains("materials artifact hashes differ")
        );
    }

    #[test]
    fn record_bytes_are_bound_to_the_expected_hash() {
        let dir = tempfile::tempdir().unwrap();
        let mut req = setup(dir.path(), 1.0, 1.0, false, false);
        req.points[0].d1s.sha256 = "0".repeat(64);
        assert!(
            compare(&req, &dir.path().join("request.json"))
                .unwrap_err()
                .contains("SHA-256 does not match")
        );
    }

    #[cfg(unix)]
    #[test]
    fn special_files_are_rejected_before_streaming_reads() {
        let special = Path::new("/dev/null");
        if !special.exists() {
            return;
        }
        assert!(hash_file(special).unwrap_err().contains("regular file"));
        assert!(
            read_record_bytes(special)
                .unwrap_err()
                .contains("regular file")
        );
    }

    fn rewrite_d1_record(
        root: &Path,
        edit_record: impl FnOnce(&mut serde_json::Value),
    ) -> CompareRequest {
        let mut request = setup(root, 1.0, 1.0, false, false);
        let path = root.join("d1s/d1s.json");
        let bytes = std::fs::read(&path).unwrap();
        let mut record: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        edit_record(&mut record);
        std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        request.points[0].d1s.sha256 = hash(&path);
        request
    }

    #[test]
    fn estimate_value_tampering_is_rejected_even_when_outer_hash_is_updated() {
        let dir = tempfile::tempdir().unwrap();
        let request = rewrite_d1_record(dir.path(), |record| {
            record["estimate"]["value"] = 3.0.into();
        });
        assert!(
            compare(&request, &dir.path().join("request.json"))
                .unwrap_err()
                .contains("estimate")
        );
    }

    #[test]
    fn unit_and_spatial_location_tampering_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let request = rewrite_d1_record(dir.path(), |record| {
            record["unit"] = "Gy/h".into();
        });
        assert!(
            compare(&request, &dir.path().join("request.json"))
                .unwrap_err()
                .contains("run record")
        );

        let dir = tempfile::tempdir().unwrap();
        let request = rewrite_d1_record(dir.path(), |record| {
            record["location"]["path"] = "../other-location.bin".into();
            record["location"]["sha256"] = sha256(b"other scored region").into();
        });
        assert!(
            compare(&request, &dir.path().join("request.json"))
                .unwrap_err()
                .contains("run record")
        );
    }

    #[test]
    fn zero_denominator_and_overflow_are_inconclusive() {
        let dir = tempfile::tempdir().unwrap();
        let request = setup(dir.path(), 1.0, 0.0, false, false);
        let report = compare(&request, &dir.path().join("request.json")).unwrap();
        assert_eq!(report.points[0].status, Status::Inconclusive);
        assert_eq!(report.points[0].ratio_d1s_over_r2s, None);

        let dir = tempfile::tempdir().unwrap();
        let request = setup(dir.path(), 1.0e308, 1.0e-308, false, false);
        let report = compare(&request, &dir.path().join("request.json")).unwrap();
        assert_eq!(report.points[0].status, Status::Inconclusive);
        assert_eq!(report.points[0].ratio_d1s_over_r2s, None);
    }
}
