// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! Hash-bound spatial photon dose-rate curves used by remote-handling maintenance.
//!
//! The provider consumes externally produced D1S and R2S estimates. It does not run either
//! transport method. Each curve is keyed to the exact operating-history serialization and
//! replacement event; a curve from another irradiation or another component installation
//! cannot silently be reused.

use crate::history::HistoryResult;
use faris_model::maintenance::{DoseMethod, PHOTON_EFFECTIVE_DOSE_RATE_QUANTITY};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const CURVES_SCHEMA: &str = "faris-maintenance-spatial-dose-curves/v0.1";
pub const ESTIMATE_SCHEMA: &str = "faris-maintenance-spatial-dose-estimate/v0.1";
pub const RUN_RECORD_SCHEMA: &str = "faris-maintenance-spatial-dose-run-record/v0.1";
pub const MAX_CURVE_FILE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PHYSICAL_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_TOTAL_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_EVENT_COUNT: usize = 100_000;
pub const MAX_POINTS_PER_EVENT: usize = 512;
const D1S_R2S_RATIO_MIN: f64 = 0.85;
const D1S_R2S_RATIO_MAX: f64 = 1.15;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
    pub path: PathBuf,
    pub sha256: String,
}

/// Complete identity of one spatial task location. The hash is over the exact location
/// definition, such as the worker pose/volume and shielding context authored by the user.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskLocation {
    pub id: String,
    pub sha256: String,
}

/// One physical D1S or R2S point estimate at a cooling time after a specific shutdown.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DoseEstimate {
    pub schema: String,
    pub method: DoseMethod,
    pub quantity: String,
    pub unit: String,
    /// Explicit source normalization, for example particles/source-neutron and physical
    /// source rate. A bare tally or an implicit normalization is rejected.
    pub normalization: String,
    pub location: TaskLocation,
    pub cooling_s: f64,
    pub value_sv_h: f64,
    pub standard_error_sv_h: f64,
    /// IDs copied from the run record. Different labels alone do not establish independence.
    pub random_streams: Vec<String>,
    /// Hashes for geometry, materials, source, irradiation, response and location inputs.
    pub physical_input_sha256: BTreeMap<String, String>,
    /// The actual artifacts whose bytes produced those hashes. Hash strings without their
    /// corresponding input artifacts do not establish a checked physical basis.
    pub physical_input_artifacts: BTreeMap<String, ArtifactRef>,
    pub run_record: ArtifactRef,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EventDoseCurve {
    pub design: String,
    pub design_input_sha256: BTreeMap<String, String>,
    pub history_sha256: String,
    pub class: String,
    pub component: String,
    pub replacement_k: u32,
    pub shutdown_s: f64,
    pub install_s: f64,
    pub irradiation_rule: super::irradiation::CondensationRule,
    pub irradiation_schedule_sha256: String,
    pub location: TaskLocation,
    pub points: Vec<DosePointPair>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DosePointPair {
    pub d1s: DoseEstimate,
    pub r2s: DoseEstimate,
    #[serde(default)]
    pub covariance: DoseCovarianceAssessment,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DoseCovarianceAssessment {
    /// Fail-closed: no comparison uncertainty is reported.
    #[default]
    Unknown,
    /// Authored independence assumption with a hash-verified evidence file.
    IndependentAssumed {
        rationale: String,
        evidence: ArtifactRef,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CurveBundle {
    pub schema: String,
    pub curves: Vec<EventDoseCurve>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ComparisonStatus {
    InBand,
    OutsideBand,
    InconclusiveCovariance,
    NotEvaluated,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ValidatedDosePoint {
    pub cooling_s: f64,
    pub selected_sv_h: f64,
    pub d1s_sv_h: f64,
    pub r2s_sv_h: f64,
    pub d1s_standard_error_sv_h: f64,
    pub r2s_standard_error_sv_h: f64,
    pub covariance_assumption: String,
    pub covariance_evidence_sha256: Option<String>,
    pub source_normalization: String,
    pub physical_input_sha256: BTreeMap<String, String>,
    pub run_record_d1s_sha256: String,
    pub run_record_r2s_sha256: String,
    pub d1s_over_r2s: Option<f64>,
    /// Delta-method uncertainty of the central method ratio at this point only.
    pub ratio_standard_error: Option<f64>,
    pub comparison: ComparisonStatus,
    pub selected_method: DoseMethod,
    pub crossing_uncertainty: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ValidatedDoseCurve {
    pub design: String,
    pub design_input_sha256: BTreeMap<String, String>,
    pub history_sha256: String,
    pub class: String,
    pub component: String,
    pub replacement_k: u32,
    pub shutdown_s: f64,
    pub install_s: f64,
    pub irradiation_schedule_sha256: String,
    pub location: TaskLocation,
    pub points: Vec<ValidatedDosePoint>,
    pub comparison: ComparisonStatus,
    pub validation_scope: String,
}

fn valid_sha(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileFingerprint {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    #[cfg(unix)]
    ctime: i64,
    #[cfg(unix)]
    ctime_nsec: i64,
}

impl FileFingerprint {
    fn from_metadata(metadata: &std::fs::Metadata) -> Result<Self, String> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            dev: metadata.dev(),
            #[cfg(unix)]
            ino: metadata.ino(),
            #[cfg(unix)]
            ctime: metadata.ctime(),
            #[cfg(unix)]
            ctime_nsec: metadata.ctime_nsec(),
        })
    }

    #[cfg(unix)]
    fn reusable_for_cache(&self) -> bool {
        true
    }

    #[cfg(not(unix))]
    fn reusable_for_cache(&self) -> bool {
        false
    }
}

fn hash_file(path: &Path) -> Result<(String, u64, FileFingerprint), String> {
    let file = open_regular_file(path)?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    let before = FileFingerprint::from_metadata(&metadata)?;
    let path_before =
        FileFingerprint::from_metadata(&std::fs::metadata(path).map_err(|e| e.to_string())?)?;
    if before != path_before {
        return Err(format!(
            "artifact {} changed while it was being opened",
            path.display()
        ));
    }
    if !metadata.is_file() || before.len > MAX_PHYSICAL_ARTIFACT_BYTES {
        return Err(format!(
            "artifact {} is not a regular file no larger than 256 MiB",
            path.display()
        ));
    }
    let mut file = file;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n as u64);
        if total > MAX_PHYSICAL_ARTIFACT_BYTES {
            return Err("physical artifact exceeds 256 MiB limit".into());
        }
        digest.update(&buffer[..n]);
    }
    let after = FileFingerprint::from_metadata(&file.metadata().map_err(|e| e.to_string())?)?;
    let path_after =
        FileFingerprint::from_metadata(&std::fs::metadata(path).map_err(|e| e.to_string())?)?;
    if before != after || after != path_after || total != before.len {
        return Err(format!(
            "artifact {} changed while it was being verified",
            path.display()
        ));
    }
    Ok((format!("{:x}", digest.finalize()), total, after))
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
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err(format!(
            "{}: opened object is not a regular file",
            path.display()
        ));
    }
    Ok(file)
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    open_regular_file(path)?
        .take((MAX_CURVE_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() > MAX_CURVE_FILE_BYTES {
        return Err(format!("{}: file exceeds 16 MiB limit", path.display()));
    }
    Ok(bytes)
}

#[derive(Default)]
struct ArtifactVerifier {
    json_records: BTreeMap<PathBuf, (String, FileFingerprint, serde_json::Value)>,
    verified_artifacts: BTreeMap<PathBuf, (String, FileFingerprint)>,
    total_bytes_read: u64,
}

impl ArtifactVerifier {
    fn verify(&mut self, reference: &ArtifactRef, base: &Path) -> Result<PathBuf, String> {
        if !valid_sha(&reference.sha256) {
            return Err("artifact reference needs a SHA-256".into());
        }
        let path = std::fs::canonicalize(base.join(&reference.path))
            .map_err(|e| format!("{}: {e}", base.join(&reference.path).display()))?;
        let metadata = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let fingerprint = FileFingerprint::from_metadata(&metadata)?;
        if !metadata.is_file() || fingerprint.len > MAX_PHYSICAL_ARTIFACT_BYTES {
            return Err(format!(
                "artifact {} is not a regular file no larger than 256 MiB",
                path.display()
            ));
        }
        if let Some((cached_sha, cached_fingerprint)) = self.verified_artifacts.get(&path) {
            let opened = open_regular_file(&path)?;
            let opened_fingerprint =
                FileFingerprint::from_metadata(&opened.metadata().map_err(|e| e.to_string())?)?;
            if fingerprint.reusable_for_cache()
                && cached_fingerprint == &fingerprint
                && cached_fingerprint == &opened_fingerprint
                && cached_sha.eq_ignore_ascii_case(&reference.sha256)
            {
                return Ok(path);
            }
        }
        let (got, bytes_read, hashed_fingerprint) = hash_file(&path)?;
        self.total_bytes_read = self.total_bytes_read.saturating_add(bytes_read);
        if self.total_bytes_read > MAX_TOTAL_ARTIFACT_BYTES {
            return Err(
                "artifact hashing exceeds the 512 MiB aggregate verification budget".into(),
            );
        }
        if !got.eq_ignore_ascii_case(&reference.sha256) {
            return Err(format!("artifact {} SHA-256 mismatch", path.display()));
        }
        self.verified_artifacts
            .insert(path.clone(), (got, hashed_fingerprint));
        Ok(path)
    }

    fn json_record(
        &mut self,
        reference: &ArtifactRef,
        base: &Path,
    ) -> Result<(PathBuf, serde_json::Value), String> {
        let path = std::fs::canonicalize(base.join(&reference.path))
            .map_err(|e| format!("{}: {e}", base.join(&reference.path).display()))?;
        let metadata = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !metadata.is_file() || metadata.len() > MAX_CURVE_FILE_BYTES as u64 {
            return Err(format!(
                "JSON artifact {} exceeds the 16 MiB regular-file limit",
                path.display()
            ));
        }
        let before = FileFingerprint::from_metadata(&metadata)?;
        if let Some((cached_sha, cached_fingerprint, record)) = self.json_records.get(&path) {
            let opened = open_regular_file(&path)?;
            let opened_fingerprint =
                FileFingerprint::from_metadata(&opened.metadata().map_err(|e| e.to_string())?)?;
            if before.reusable_for_cache()
                && cached_fingerprint == &before
                && cached_fingerprint == &opened_fingerprint
                && cached_sha.eq_ignore_ascii_case(&reference.sha256)
            {
                return Ok((path, record.clone()));
            }
        }
        let bytes = read_bounded(&path)?;
        let after =
            FileFingerprint::from_metadata(&std::fs::metadata(&path).map_err(|e| e.to_string())?)?;
        if before != after || bytes.len() as u64 != after.len {
            return Err(format!(
                "JSON artifact {} changed while it was being verified",
                path.display()
            ));
        }
        self.total_bytes_read = self.total_bytes_read.saturating_add(bytes.len() as u64);
        if self.total_bytes_read > MAX_TOTAL_ARTIFACT_BYTES {
            return Err("artifact reads exceed the 512 MiB aggregate verification budget".into());
        }
        let digest = format!("{:x}", Sha256::digest(&bytes));
        if !digest.eq_ignore_ascii_case(&reference.sha256) {
            return Err(format!("artifact {} SHA-256 mismatch", path.display()));
        }
        let record: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("{}: JSON: {e}", path.display()))?;
        self.json_records
            .insert(path.clone(), (digest, after, record.clone()));
        Ok((path, record))
    }
}

fn verify_artifact(
    reference: &ArtifactRef,
    base: &Path,
    verifier: &mut ArtifactVerifier,
) -> Result<(), String> {
    verifier.verify(reference, base)?;
    Ok(())
}

fn close_time(a: f64, b: f64) -> bool {
    a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

fn validate_run_record(
    e: &DoseEstimate,
    event: &EventDoseCurve,
    event_sha256: &str,
    base: &Path,
    verifier: &mut ArtifactVerifier,
) -> Result<(), String> {
    let (path, run) = verifier.json_record(&e.run_record, base)?;
    let method = match e.method {
        DoseMethod::D1s => "d1s",
        DoseMethod::R2s => "r2s",
    };
    let estimate = run.get("estimate");
    let response = run.get("response");
    let artifacts = run.get("artifacts");
    let run_streams = run
        .get("random_streams")
        .and_then(|value| serde_json::from_value::<Vec<String>>(value.clone()).ok());
    let event_record = run.get("event");
    let design_hashes = serde_json::to_value(&event.design_input_sha256).ok();
    let event_matches = event_record.is_some_and(|actual| {
        actual.get("identity_sha256").and_then(|v| v.as_str()) == Some(event_sha256)
            && actual.get("history_sha256").and_then(|v| v.as_str())
                == Some(event.history_sha256.as_str())
            && actual.get("design").and_then(|v| v.as_str()) == Some(event.design.as_str())
            && actual.get("design_input_sha256") == design_hashes.as_ref()
            && actual.get("class").and_then(|v| v.as_str()) == Some(event.class.as_str())
            && actual.get("component").and_then(|v| v.as_str()) == Some(event.component.as_str())
            && actual.get("replacement_k").and_then(|v| v.as_u64())
                == Some(u64::from(event.replacement_k))
            && actual
                .get("shutdown_s")
                .and_then(|v| v.as_f64())
                .is_some_and(|v| close_time(v, event.shutdown_s))
            && actual
                .get("install_s")
                .and_then(|v| v.as_f64())
                .is_some_and(|v| close_time(v, event.install_s))
            && actual
                .get("irradiation_schedule_sha256")
                .and_then(|v| v.as_str())
                == Some(event.irradiation_schedule_sha256.as_str())
            && actual
                .get("location")
                .and_then(|v| v.get("id"))
                .and_then(|v| v.as_str())
                == Some(event.location.id.as_str())
            && actual
                .get("location")
                .and_then(|v| v.get("sha256"))
                .and_then(|v| v.as_str())
                .is_some_and(|v| v.eq_ignore_ascii_case(&event.location.sha256))
    });
    let estimate_matches = estimate.is_some_and(|actual| {
        actual.get("value").and_then(|v| v.as_f64()) == Some(e.value_sv_h)
            && actual.get("standard_error").and_then(|v| v.as_f64()) == Some(e.standard_error_sv_h)
    });
    let response_matches = [
        ("quantity", e.quantity.as_str()),
        ("unit", e.unit.as_str()),
        ("normalization", e.normalization.as_str()),
    ]
    .iter()
    .all(|(key, expected)| {
        response.and_then(|r| r.get(key)).and_then(|v| v.as_str()) == Some(*expected)
    });
    let basis_keys = [
        ("geometry_sha256", "geometry"),
        ("materials_sha256", "materials"),
        ("source_sha256", "source"),
        ("irradiation_sha256", "irradiation"),
        ("response_sha256", "response"),
        ("location_sha256", "location"),
    ];
    let basis_matches = basis_keys.iter().all(|(record_key, input_key)| {
        artifacts
            .and_then(|a| a.get(*record_key))
            .and_then(|v| v.as_str())
            .is_some_and(|v| v.eq_ignore_ascii_case(&e.physical_input_sha256[*input_key]))
    });
    if run.get("schema").and_then(|v| v.as_str()) != Some(RUN_RECORD_SCHEMA)
        || run.get("method").and_then(|v| v.as_str()) != Some(method)
        || run
            .get("cooling_time_s")
            .and_then(|v| v.as_f64())
            .is_none_or(|v| !close_time(v, e.cooling_s))
        || run_streams.as_ref() != Some(&e.random_streams)
        || !event_matches
        || !estimate_matches
        || !response_matches
        || !basis_matches
    {
        return Err(format!(
            "{}: run record method, cooling time, estimate, response, event identity, input hashes or random streams do not match the estimate",
            path.display()
        ));
    }
    Ok(())
}

fn validate_location(location: &TaskLocation) -> Result<(), String> {
    if location.id.trim().is_empty() || location.id.len() > 256 || !valid_sha(&location.sha256) {
        return Err("task location needs a nonempty ID and a 64-character SHA-256".into());
    }
    Ok(())
}

fn validate_estimate(
    e: &DoseEstimate,
    event: &EventDoseCurve,
    method: DoseMethod,
    location: &TaskLocation,
    expected_event_sha256: &str,
    base: &Path,
    verifier: &mut ArtifactVerifier,
) -> Result<(), String> {
    if e.schema != ESTIMATE_SCHEMA {
        return Err(format!("estimate schema must be {ESTIMATE_SCHEMA}"));
    }
    if e.method != method {
        return Err("dose estimate method does not match its D1S/R2S slot".into());
    }
    if e.quantity != PHOTON_EFFECTIVE_DOSE_RATE_QUANTITY {
        return Err("dose estimate quantity must be ICRP-116 AP photon effective dose rate; Sv/h alone does not establish interchangeability".into());
    }
    if e.unit != "Sv/h" {
        return Err("dose estimate unit must be Sv/h".into());
    }
    if e.normalization.trim().is_empty() || e.normalization.len() > 2048 {
        return Err("dose estimate requires explicit source normalization".into());
    }
    validate_location(&e.location)?;
    if &e.location != location {
        return Err("estimate location identity differs from event task location".into());
    }
    if !(e.cooling_s.is_finite()
        && e.cooling_s > 0.0
        && e.value_sv_h.is_finite()
        && e.value_sv_h >= 0.0)
    {
        return Err("cooling time and dose rate must be finite, and cooling time positive".into());
    }
    if !e.standard_error_sv_h.is_finite() || e.standard_error_sv_h < 0.0 {
        return Err("standard error must be finite and nonnegative".into());
    }
    for required in [
        "geometry",
        "materials",
        "source",
        "irradiation",
        "response",
        "location",
    ] {
        let sha = e
            .physical_input_sha256
            .get(required)
            .ok_or_else(|| format!("estimate omits {required} physical input hash"))?;
        if !valid_sha(sha) {
            return Err(format!("estimate {required} input hash is not SHA-256"));
        }
        let artifact = e
            .physical_input_artifacts
            .get(required)
            .ok_or_else(|| format!("estimate omits the {required} physical input artifact"))?;
        if &artifact.sha256 != sha {
            return Err(format!(
                "estimate {required} artifact hash differs from its declared input hash"
            ));
        }
        verify_artifact(artifact, base, verifier)?;
    }
    if e.physical_input_sha256["location"] != location.sha256 {
        return Err("location input hash differs from event location hash".into());
    }
    if e.physical_input_sha256["event"] != expected_event_sha256 {
        return Err(
            "dose point event input hash does not match the exact replacement event".into(),
        );
    }
    if !valid_sha(&e.physical_input_sha256["event"]) {
        return Err("event input hash is not SHA-256".into());
    }
    if e.random_streams.is_empty()
        || e.random_streams.len() > 128
        || e.random_streams
            .iter()
            .any(|s| s.trim().is_empty() || s.len() > 256)
    {
        return Err(
            "run record must name 1–128 nonempty random streams, each at most 256 bytes".into(),
        );
    }
    let mut streams = e.random_streams.clone();
    streams.sort();
    streams.dedup();
    if streams.len() != e.random_streams.len() {
        return Err("random stream IDs must be unique".into());
    }
    validate_run_record(e, event, expected_event_sha256, base, verifier)
}

/// Hash of the exact event key, with fixed field order and serde's compact JSON encoding.
pub fn event_identity_sha256(curve: &EventDoseCurve) -> Result<String, String> {
    let bytes = serde_json::to_vec(&(
        CURVES_SCHEMA,
        &curve.design,
        &curve.design_input_sha256,
        &curve.history_sha256,
        &curve.class,
        &curve.component,
        curve.replacement_k,
        curve.shutdown_s,
        curve.install_s,
        curve.irradiation_rule,
        &curve.irradiation_schedule_sha256,
        &curve.location,
    ))
    .map_err(|e| e.to_string())?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

fn compare(pair: &DosePointPair) -> ComparisonStatus {
    if pair.r2s.value_sv_h == 0.0 {
        return if pair.d1s.value_sv_h > 0.0 {
            ComparisonStatus::OutsideBand
        } else {
            ComparisonStatus::InconclusiveCovariance
        };
    }
    let ratio = pair.d1s.value_sv_h / pair.r2s.value_sv_h;
    if !(D1S_R2S_RATIO_MIN..=D1S_R2S_RATIO_MAX).contains(&ratio) {
        return ComparisonStatus::OutsideBand;
    }
    if matches!(pair.covariance, DoseCovarianceAssessment::Unknown) {
        return ComparisonStatus::InconclusiveCovariance;
    }
    ComparisonStatus::InBand
}

fn ratio_standard_error(pair: &DosePointPair) -> Option<f64> {
    let denominator = pair.r2s.value_sv_h;
    if !matches!(
        pair.covariance,
        DoseCovarianceAssessment::IndependentAssumed { .. }
    ) || denominator <= 0.0
    {
        return None;
    }
    let numerator = pair.d1s.value_sv_h;
    let first = pair.d1s.standard_error_sv_h / denominator;
    let second = numerator / denominator * pair.r2s.standard_error_sv_h / denominator;
    let result = first.hypot(second);
    result.is_finite().then_some(result)
}

impl CurveBundle {
    /// Bounded, side-effect-free JSON parser shared by the CLI, native UI and fuzz harness.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_CURVE_FILE_BYTES {
            return Err("spatial dose curve file exceeds 16 MiB limit".into());
        }
        let bundle: Self =
            serde_json::from_slice(bytes).map_err(|e| format!("spatial dose curves JSON: {e}"))?;
        if bundle.schema != CURVES_SCHEMA {
            return Err(format!("curve schema must be {CURVES_SCHEMA}"));
        }
        if bundle.curves.len() > MAX_EVENT_COUNT
            || bundle
                .curves
                .iter()
                .any(|c| c.points.len() > MAX_POINTS_PER_EVENT)
        {
            return Err("spatial dose curve bundle exceeds its event or point bound".into());
        }
        Ok(bundle)
    }

    /// Load a bounded external curve file and verify every referenced run artifact.
    pub fn read(path: &Path) -> Result<Self, String> {
        let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.len() > MAX_CURVE_FILE_BYTES as u64 {
            return Err(
                "spatial dose curve file must be a regular file no larger than 16 MiB".into(),
            );
        }
        let mut bytes = Vec::new();
        open_regular_file(path)?
            .take((MAX_CURVE_FILE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > MAX_CURVE_FILE_BYTES {
            return Err("spatial dose curve file exceeds 16 MiB limit".into());
        }
        let mut bundle = Self::parse(&bytes)?;
        let base = path.parent().unwrap_or(Path::new("."));
        for curve in &mut bundle.curves {
            for point in &mut curve.points {
                if let DoseCovarianceAssessment::IndependentAssumed { evidence, .. } =
                    &mut point.covariance
                {
                    if !evidence.path.is_absolute() {
                        evidence.path = base.join(&evidence.path);
                    }
                }
                for estimate in [&mut point.d1s, &mut point.r2s] {
                    if !estimate.run_record.path.is_absolute() {
                        estimate.run_record.path = base.join(&estimate.run_record.path);
                    }
                    for artifact in estimate.physical_input_artifacts.values_mut() {
                        if !artifact.path.is_absolute() {
                            artifact.path = base.join(&artifact.path);
                        }
                    }
                }
            }
        }
        bundle.validate(Path::new("."))?;
        Ok(bundle)
    }

    pub fn validate(&self, base: &Path) -> Result<(), String> {
        if self.schema != CURVES_SCHEMA {
            return Err(format!("curve schema must be {CURVES_SCHEMA}"));
        }
        if self.curves.len() > MAX_EVENT_COUNT {
            return Err("too many event dose curves".into());
        }
        let mut keys = BTreeSet::new();
        let mut verifier = ArtifactVerifier::default();
        for curve in &self.curves {
            validate_location(&curve.location)?;
            if curve.design.trim().is_empty()
                || curve.class.trim().is_empty()
                || curve.component.trim().is_empty()
                || curve.replacement_k == 0
                || !(curve.shutdown_s.is_finite()
                    && curve.shutdown_s >= 0.0
                    && curve.install_s.is_finite()
                    && curve.install_s <= curve.shutdown_s)
                || !valid_sha(&curve.history_sha256)
            {
                return Err(
                    "event curve has incomplete design/history/replacement identity".into(),
                );
            }
            if curve.design_input_sha256.is_empty()
                || curve
                    .design_input_sha256
                    .iter()
                    .any(|(k, v)| k.trim().is_empty() || !valid_sha(v))
            {
                return Err("event curve needs named, valid design input hashes".into());
            }
            let expected_design_inputs: BTreeSet<&str> = [
                "scenario",
                "physics",
                "history_run",
                "spectrum_run",
                "history_assumptions",
            ]
            .into_iter()
            .collect();
            if curve
                .design_input_sha256
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                != expected_design_inputs
            {
                return Err("event curve must bind all five named design input hashes".into());
            }
            let key = (
                &curve.design,
                &curve.history_sha256,
                &curve.class,
                &curve.component,
                curve.replacement_k,
                curve.shutdown_s.to_bits(),
            );
            if !keys.insert(key) {
                return Err("duplicate event dose curve identity".into());
            }
            if curve.points.is_empty() || curve.points.len() > MAX_POINTS_PER_EVENT {
                return Err("event dose curve has no points or exceeds the 512 point limit".into());
            }
            let expected_event_sha256 = event_identity_sha256(curve)?;
            if !valid_sha(&curve.irradiation_schedule_sha256) {
                return Err("event curve needs the irradiation schedule SHA-256".into());
            }
            let mut prior = 0.0;
            let mut reference_basis: Option<BTreeMap<String, String>> = None;
            let mut reference_normalization: Option<&str> = None;
            for pair in &curve.points {
                validate_estimate(
                    &pair.d1s,
                    curve,
                    DoseMethod::D1s,
                    &curve.location,
                    &expected_event_sha256,
                    base,
                    &mut verifier,
                )?;
                validate_estimate(
                    &pair.r2s,
                    curve,
                    DoseMethod::R2s,
                    &curve.location,
                    &expected_event_sha256,
                    base,
                    &mut verifier,
                )?;
                if pair.d1s.cooling_s != pair.r2s.cooling_s || pair.d1s.cooling_s <= prior {
                    return Err(
                        "D1S/R2S points must have identical strictly ascending cooling times"
                            .into(),
                    );
                }
                if pair.d1s.physical_input_sha256 != pair.r2s.physical_input_sha256 {
                    return Err("D1S/R2S physical input hashes differ".into());
                }
                if pair.d1s.normalization != pair.r2s.normalization {
                    return Err("D1S/R2S source normalization differs".into());
                }
                if reference_basis
                    .as_ref()
                    .is_some_and(|basis| basis != &pair.d1s.physical_input_sha256)
                    || reference_normalization.is_some_and(|norm| norm != pair.d1s.normalization)
                {
                    return Err("physical input basis and normalization must be stable across cooling points for one event".into());
                }
                reference_basis.get_or_insert_with(|| pair.d1s.physical_input_sha256.clone());
                reference_normalization.get_or_insert(&pair.d1s.normalization);
                if let DoseCovarianceAssessment::IndependentAssumed {
                    rationale,
                    evidence,
                } = &pair.covariance
                {
                    if rationale.trim().is_empty() || rationale.len() > 2048 {
                        return Err("independent covariance assumption needs a rationale of at most 2048 bytes".into());
                    }
                    verify_artifact(evidence, base, &mut verifier)?;
                }
                prior = pair.d1s.cooling_s;
            }
        }
        Ok(())
    }

    /// Find only a curve matching the exact history and event identity. Returning `None` is a
    /// physics/data gap; callers must report NOT_EVALUATED instead of reusing another event.
    pub fn event_curve(
        &self,
        design: &str,
        design_input_sha256: &BTreeMap<String, String>,
        history_sha256: &str,
        class: &str,
        component: &str,
        replacement_k: u32,
        shutdown_s: f64,
        location: &TaskLocation,
        method: DoseMethod,
    ) -> Result<Option<ValidatedDoseCurve>, String> {
        let Some(curve) = self.curves.iter().find(|c| {
            c.design == design
                && c.history_sha256 == history_sha256
                && c.class == class
                && c.component == component
                && c.replacement_k == replacement_k
                && c.shutdown_s.to_bits() == shutdown_s.to_bits()
        }) else {
            return Ok(None);
        };
        if &curve.design_input_sha256 != design_input_sha256 {
            return Err(format!(
                "dose curve for {design}/{component} uses stale design input hashes"
            ));
        }
        if &curve.location != location {
            return Err(format!(
                "dose curve for {design}/{component} uses a different task location"
            ));
        }
        let mut overall = ComparisonStatus::InBand;
        let points: Vec<_> = curve.points.iter().map(|pair| {
            let status = compare(pair);
            if status == ComparisonStatus::OutsideBand { overall = ComparisonStatus::OutsideBand; }
            else if status == ComparisonStatus::InconclusiveCovariance && overall == ComparisonStatus::InBand { overall = status.clone(); }
            ValidatedDosePoint {
                cooling_s: pair.d1s.cooling_s,
                selected_sv_h: match method { DoseMethod::D1s => pair.d1s.value_sv_h, DoseMethod::R2s => pair.r2s.value_sv_h },
                d1s_sv_h: pair.d1s.value_sv_h,
                r2s_sv_h: pair.r2s.value_sv_h,
                d1s_standard_error_sv_h: pair.d1s.standard_error_sv_h,
                r2s_standard_error_sv_h: pair.r2s.standard_error_sv_h,
                covariance_assumption: match &pair.covariance {
                    DoseCovarianceAssessment::Unknown => "unknown".into(),
                    DoseCovarianceAssessment::IndependentAssumed { rationale, .. } => format!("independent_assumed: {rationale}"),
                },
                covariance_evidence_sha256: match &pair.covariance {
                    DoseCovarianceAssessment::Unknown => None,
                    DoseCovarianceAssessment::IndependentAssumed { evidence, .. } => Some(evidence.sha256.clone()),
                },
                source_normalization: pair.d1s.normalization.clone(),
                physical_input_sha256: pair.d1s.physical_input_sha256.clone(),
                run_record_d1s_sha256: pair.d1s.run_record.sha256.clone(),
                run_record_r2s_sha256: pair.r2s.run_record.sha256.clone(),
                d1s_over_r2s: (pair.r2s.value_sv_h > 0.0).then_some(pair.d1s.value_sv_h / pair.r2s.value_sv_h),
                ratio_standard_error: ratio_standard_error(pair),
                comparison: status,
                selected_method: method,
                crossing_uncertainty: "NOT_EVALUATED: pointwise standard errors do not provide cooling-time covariance".into(),
            }
        }).collect();
        Ok(Some(ValidatedDoseCurve {
            design: curve.design.clone(), design_input_sha256: curve.design_input_sha256.clone(), history_sha256: curve.history_sha256.clone(), class: curve.class.clone(), component: curve.component.clone(), replacement_k: curve.replacement_k, shutdown_s: curve.shutdown_s, install_s: curve.install_s, irradiation_schedule_sha256: curve.irradiation_schedule_sha256.clone(), location: curve.location.clone(), points,
            comparison: overall,
            validation_scope: "Authored external spatial estimate; D1S/R2S comparison only. Not a product-generated transport solve or a complete physical validation.".into(),
        }))
    }

    /// Verify the receipt file from the estimate's irradiation artifact against a schedule
    /// rebuilt from this exact event history. The artifact byte hash and the schedule's
    /// semantic input hash are separate identities; both are checked.
    pub fn verify_irradiation_schedule(
        &self,
        event: &EventDoseCurve,
        history: &HistoryResult,
    ) -> Result<(), String> {
        let expected = super::irradiation::irradiation_schedule(
            history,
            Some((&event.component, event.shutdown_s)),
            &event.irradiation_rule,
            &[event.shutdown_s],
        )?;
        if expected.input_sha256 != event.irradiation_schedule_sha256 {
            return Err(format!(
                "dose curve for {} replacement #{} has a stale irradiation schedule hash",
                event.component, event.replacement_k
            ));
        }
        let pair = event
            .points
            .first()
            .ok_or("event dose curve has no points")?;
        let reference = pair
            .d1s
            .physical_input_artifacts
            .get("irradiation")
            .ok_or("dose estimate omits the irradiation schedule artifact")?;
        let expected_sha = &pair.d1s.physical_input_sha256["irradiation"];
        if !reference.sha256.eq_ignore_ascii_case(expected_sha)
            || !pair.r2s.physical_input_sha256["irradiation"].eq_ignore_ascii_case(expected_sha)
        {
            return Err(
                "D1S/R2S event points do not share the exact irradiation artifact hash".into(),
            );
        }
        let bytes = read_bounded(&reference.path)?;
        let actual_sha = format!("{:x}", Sha256::digest(&bytes));
        if !actual_sha.eq_ignore_ascii_case(expected_sha) {
            return Err("irradiation schedule artifact SHA-256 mismatch".into());
        }
        let receipt: super::irradiation::IrradiationSchedule = serde_json::from_slice(&bytes)
            .map_err(|e| format!("invalid irradiation schedule artifact: {e}"))?;
        if !receipt.verify() || receipt != expected {
            return Err(format!(
                "irradiation schedule artifact for {} replacement #{} does not match the exact history event",
                event.component, event.replacement_k
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    #[test]
    fn large_shared_physical_artifact_is_supported_and_hashed_once() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("rm-m-geometry.h5");
        let bytes = vec![0_u8; 64 * 1024 * 1024];
        std::fs::write(&path, &bytes).unwrap();
        let reference = ArtifactRef {
            path: "rm-m-geometry.h5".into(),
            sha256: sha(&bytes),
        };
        let mut verifier = ArtifactVerifier::default();
        for _ in 0..6 {
            verifier.verify(&reference, temp.path()).unwrap();
        }
        assert_eq!(verifier.total_bytes_read, bytes.len() as u64);
        assert!(bytes.len() as u64 > MAX_CURVE_FILE_BYTES as u64);
        assert!(bytes.len() as u64 <= MAX_PHYSICAL_ARTIFACT_BYTES);
    }

    #[test]
    fn changed_physical_artifact_cannot_reuse_a_cached_hash() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("geometry.h5");
        let original = b"original geometry bytes";
        let changed = b"modified geometry bytes";
        assert_eq!(original.len(), changed.len());
        std::fs::write(&path, original).unwrap();
        let reference = ArtifactRef {
            path: "geometry.h5".into(),
            sha256: sha(original),
        };
        let mut verifier = ArtifactVerifier::default();
        verifier.verify(&reference, temp.path()).unwrap();
        std::fs::write(&path, changed).unwrap();
        let error = verifier.verify(&reference, temp.path()).unwrap_err();
        assert!(error.contains("SHA-256 mismatch"), "{error}");
    }

    fn estimate(
        method: DoseMethod,
        location: &TaskLocation,
        run_sha: &str,
        value: f64,
    ) -> DoseEstimate {
        DoseEstimate {
            schema: ESTIMATE_SCHEMA.into(),
            method,
            quantity: PHOTON_EFFECTIVE_DOSE_RATE_QUANTITY.into(),
            unit: "Sv/h".into(),
            normalization: "test-only normalized source".into(),
            location: location.clone(),
            cooling_s: 10.0,
            value_sv_h: value,
            standard_error_sv_h: value * 0.01,
            random_streams: vec![
                match method {
                    DoseMethod::D1s => "d1s-stream",
                    DoseMethod::R2s => "r2s-stream",
                }
                .into(),
            ],
            physical_input_sha256: [
                "geometry",
                "materials",
                "source",
                "irradiation",
                "event",
                "response",
                "location",
            ]
            .into_iter()
            .map(|name| {
                (
                    name.to_string(),
                    if name == "location" {
                        location.sha256.clone()
                    } else if name == "event" {
                        "a".repeat(64)
                    } else if name == "irradiation" {
                        "e".repeat(64)
                    } else {
                        "b".repeat(64)
                    },
                )
            })
            .collect(),
            physical_input_artifacts: BTreeMap::new(),
            run_record: ArtifactRef {
                path: match method {
                    DoseMethod::D1s => "run-d1s.json",
                    DoseMethod::R2s => "run-r2s.json",
                }
                .into(),
                sha256: run_sha.into(),
            },
        }
    }

    #[test]
    fn only_exact_history_event_design_and_location_curve_is_selectable() {
        let temp = tempfile::tempdir().unwrap();
        let mut artifact_refs = BTreeMap::new();
        for name in [
            "geometry",
            "materials",
            "source",
            "irradiation",
            "response",
            "location",
        ] {
            let bytes = format!("synthetic {name} artifact");
            let file = format!("{name}.json");
            std::fs::write(temp.path().join(&file), bytes.as_bytes()).unwrap();
            artifact_refs.insert(
                name.to_owned(),
                ArtifactRef {
                    path: file.into(),
                    sha256: sha(bytes.as_bytes()),
                },
            );
        }
        let location = TaskLocation {
            id: "worker-pose-1".into(),
            sha256: artifact_refs["location"].sha256.clone(),
        };
        let hashes = [
            "scenario",
            "physics",
            "history_run",
            "spectrum_run",
            "history_assumptions",
        ]
        .into_iter()
        .map(|n| (n.to_string(), "c".repeat(64)))
        .collect::<BTreeMap<_, _>>();
        let mut d1s = estimate(DoseMethod::D1s, &location, &"0".repeat(64), 9.0e-5);
        let mut r2s = estimate(DoseMethod::R2s, &location, &"0".repeat(64), 1.0e-4);
        for e in [&mut d1s, &mut r2s] {
            e.physical_input_sha256 = artifact_refs
                .iter()
                .map(|(k, v)| (k.clone(), v.sha256.clone()))
                .collect();
            e.physical_input_artifacts = artifact_refs.clone();
        }
        let mut event = EventDoseCurve {
            design: "design-a".into(),
            design_input_sha256: hashes.clone(),
            history_sha256: "d".repeat(64),
            class: "magnet".into(),
            component: "magnet".into(),
            replacement_k: 1,
            shutdown_s: 100.0,
            install_s: 0.0,
            irradiation_rule: super::super::irradiation::CondensationRule::LOSSLESS,
            irradiation_schedule_sha256: "f".repeat(64),
            location: location.clone(),
            points: vec![DosePointPair {
                d1s,
                r2s,
                covariance: DoseCovarianceAssessment::Unknown,
            }],
        };
        let event_sha = event_identity_sha256(&event).unwrap();
        event.points[0]
            .d1s
            .physical_input_sha256
            .insert("event".into(), event_sha.clone());
        event.points[0]
            .r2s
            .physical_input_sha256
            .insert("event".into(), event_sha.clone());
        let covariance_evidence =
            b"synthetic format-test evidence for the authored independence assumption";
        std::fs::write(temp.path().join("covariance.json"), covariance_evidence).unwrap();
        event.points[0].covariance = DoseCovarianceAssessment::IndependentAssumed {
            rationale: "synthetic fixture only".into(),
            evidence: ArtifactRef {
                path: "covariance.json".into(),
                sha256: sha(covariance_evidence),
            },
        };
        for estimate in [&mut event.points[0].d1s, &mut event.points[0].r2s] {
            let method = match estimate.method {
                DoseMethod::D1s => "d1s",
                DoseMethod::R2s => "r2s",
            };
            let run = serde_json::json!({
                "schema": RUN_RECORD_SCHEMA,
                "method": method,
                "cooling_time_s": estimate.cooling_s,
                "estimate": { "value": estimate.value_sv_h, "standard_error": estimate.standard_error_sv_h },
                "response": { "quantity": estimate.quantity, "unit": estimate.unit, "normalization": estimate.normalization },
                "artifacts": {
                    "geometry_sha256": estimate.physical_input_sha256["geometry"],
                    "materials_sha256": estimate.physical_input_sha256["materials"],
                    "source_sha256": estimate.physical_input_sha256["source"],
                    "irradiation_sha256": estimate.physical_input_sha256["irradiation"],
                    "response_sha256": estimate.physical_input_sha256["response"],
                    "location_sha256": estimate.physical_input_sha256["location"]
                },
                "random_streams": estimate.random_streams,
                "event": {
                    "identity_sha256": event_sha,
                    "history_sha256": event.history_sha256,
                    "design": event.design,
                    "design_input_sha256": event.design_input_sha256,
                    "class": event.class,
                    "component": event.component,
                    "replacement_k": event.replacement_k,
                    "shutdown_s": event.shutdown_s,
                    "install_s": event.install_s,
                    "irradiation_schedule_sha256": event.irradiation_schedule_sha256,
                    "location": event.location
                }
            });
            let bytes = serde_json::to_vec(&run).unwrap();
            std::fs::write(temp.path().join(&estimate.run_record.path), &bytes).unwrap();
            estimate.run_record.sha256 = sha(&bytes);
        }
        let bundle = CurveBundle {
            schema: CURVES_SCHEMA.into(),
            curves: vec![event],
        };
        bundle.validate(temp.path()).unwrap();
        let selected = bundle
            .event_curve(
                "design-a",
                &hashes,
                &"d".repeat(64),
                "magnet",
                "magnet",
                1,
                100.0,
                &location,
                DoseMethod::D1s,
            )
            .unwrap()
            .unwrap();
        assert_eq!(selected.comparison, ComparisonStatus::InBand);
        assert_eq!(selected.points[0].d1s_over_r2s, Some(0.9));
        assert!(selected.points[0].ratio_standard_error.is_some());
        assert!(
            bundle
                .event_curve(
                    "design-a",
                    &hashes,
                    &"e".repeat(64),
                    "magnet",
                    "magnet",
                    1,
                    100.0,
                    &location,
                    DoseMethod::D1s
                )
                .unwrap()
                .is_none()
        );
        let mut stale = hashes;
        stale.insert("physics".into(), "f".repeat(64));
        assert!(
            bundle
                .event_curve(
                    "design-a",
                    &stale,
                    &"d".repeat(64),
                    "magnet",
                    "magnet",
                    1,
                    100.0,
                    &location,
                    DoseMethod::D1s
                )
                .is_err()
        );
        let mut relabeled = bundle.clone();
        relabeled.curves[0].history_sha256 = "e".repeat(64);
        let new_event_sha = event_identity_sha256(&relabeled.curves[0]).unwrap();
        for estimate in [
            &mut relabeled.curves[0].points[0].d1s,
            &mut relabeled.curves[0].points[0].r2s,
        ] {
            estimate
                .physical_input_sha256
                .insert("event".into(), new_event_sha.clone());
        }
        let error = relabeled.validate(temp.path()).unwrap_err();
        assert!(error.contains("run record method"), "{error}");
    }

    #[test]
    fn central_ratio_outside_band_never_passes_validation() {
        let pair = DosePointPair {
            d1s: DoseEstimate {
                value_sv_h: 2.0,
                cooling_s: 10.0,
                ..estimate(
                    DoseMethod::D1s,
                    &TaskLocation {
                        id: "x".into(),
                        sha256: "a".repeat(64),
                    },
                    &"b".repeat(64),
                    1.0,
                )
            },
            r2s: DoseEstimate {
                value_sv_h: 1.0,
                cooling_s: 10.0,
                ..estimate(
                    DoseMethod::R2s,
                    &TaskLocation {
                        id: "x".into(),
                        sha256: "a".repeat(64),
                    },
                    &"b".repeat(64),
                    1.0,
                )
            },
            covariance: DoseCovarianceAssessment::Unknown,
        };
        assert_eq!(compare(&pair), ComparisonStatus::OutsideBand);
        let mut unknown = pair;
        unknown.d1s.value_sv_h = 0.9;
        unknown.covariance = DoseCovarianceAssessment::Unknown;
        assert_eq!(compare(&unknown), ComparisonStatus::InconclusiveCovariance);
    }
}
