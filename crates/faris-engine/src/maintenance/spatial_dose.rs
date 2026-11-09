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

pub const CURVES_SCHEMA: &str = "faris-maintenance-spatial-dose-curves/v0.1";
pub const ESTIMATE_SCHEMA: &str = "faris-maintenance-spatial-dose-estimate/v0.1";
pub const MAX_CURVE_FILE_BYTES: usize = 16 * 1024 * 1024;
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
    pub standard_error_sv_h: Option<f64>,
    /// Hashes for geometry, materials, source, irradiation, response and location inputs.
    pub physical_input_sha256: BTreeMap<String, String>,
    /// The actual artifacts whose bytes produced those hashes. Hash strings without their
    /// corresponding input artifacts do not establish a checked physical basis.
    pub physical_input_artifacts: BTreeMap<String, ArtifactRef>,
    pub run_record: ArtifactRef,
    /// Covariance between compared point estimates, when supplied by the producer. Pointwise
    /// standard errors do not imply temporal covariance.
    pub comparison_covariance: Option<f64>,
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
    pub d1s_standard_error_sv_h: Option<f64>,
    pub r2s_standard_error_sv_h: Option<f64>,
    pub comparison_covariance: Option<f64>,
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

fn hash_file(path: &Path) -> Result<String, String> {
    let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_CURVE_FILE_BYTES as u64 {
        return Err(format!(
            "artifact {} is not a bounded regular file",
            path.display()
        ));
    }
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0usize;
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n);
        if total > MAX_CURVE_FILE_BYTES {
            return Err("artifact exceeds 16 MiB limit".into());
        }
        digest.update(&buffer[..n]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn verify_artifact(reference: &ArtifactRef, base: &Path) -> Result<(), String> {
    if !valid_sha(&reference.sha256) {
        return Err("artifact reference needs a SHA-256".into());
    }
    let path = base.join(&reference.path);
    let got = hash_file(&path)?;
    if got != reference.sha256 {
        return Err(format!("artifact {} SHA-256 mismatch", path.display()));
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
    method: DoseMethod,
    location: &TaskLocation,
    expected_event_sha256: &str,
    base: &Path,
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
    if e.standard_error_sv_h
        .is_some_and(|v| !v.is_finite() || v < 0.0)
    {
        return Err("standard error must be finite and nonnegative".into());
    }
    if e.comparison_covariance.is_some_and(|v| !v.is_finite()) {
        return Err("comparison covariance must be finite".into());
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
        verify_artifact(artifact, base)?;
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
    verify_artifact(&e.run_record, base)
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
    if pair.d1s.comparison_covariance.is_none()
        || pair.d1s.comparison_covariance != pair.r2s.comparison_covariance
        || pair.d1s.standard_error_sv_h.is_none()
        || pair.r2s.standard_error_sv_h.is_none()
    {
        return ComparisonStatus::InconclusiveCovariance;
    }
    ComparisonStatus::InBand
}

fn ratio_standard_error(pair: &DosePointPair) -> Option<f64> {
    let denominator = pair.r2s.value_sv_h;
    let sigma_d1s = pair.d1s.standard_error_sv_h?;
    let sigma_r2s = pair.r2s.standard_error_sv_h?;
    let covariance = pair.d1s.comparison_covariance?;
    if pair.r2s.comparison_covariance != Some(covariance) || denominator <= 0.0 {
        return None;
    }
    let numerator = pair.d1s.value_sv_h;
    let variance = sigma_d1s.powi(2) / denominator.powi(2)
        + numerator.powi(2) * sigma_r2s.powi(2) / denominator.powi(4)
        - 2.0 * numerator * covariance / denominator.powi(3);
    variance.is_finite().then_some(variance.max(0.0).sqrt())
}

impl CurveBundle {
    /// Load a bounded external curve file and verify every referenced run artifact.
    pub fn read(path: &Path) -> Result<Self, String> {
        let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.len() > MAX_CURVE_FILE_BYTES as u64 {
            return Err(
                "spatial dose curve file must be a regular file no larger than 16 MiB".into(),
            );
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|e| e.to_string())?
            .take((MAX_CURVE_FILE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > MAX_CURVE_FILE_BYTES {
            return Err("spatial dose curve file exceeds 16 MiB limit".into());
        }
        let mut bundle: Self = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        let base = path.parent().unwrap_or(Path::new("."));
        for curve in &mut bundle.curves {
            for point in &mut curve.points {
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
                    DoseMethod::D1s,
                    &curve.location,
                    &expected_event_sha256,
                    base,
                )?;
                validate_estimate(
                    &pair.r2s,
                    DoseMethod::R2s,
                    &curve.location,
                    &expected_event_sha256,
                    base,
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
                if pair.d1s.comparison_covariance != pair.r2s.comparison_covariance {
                    return Err("D1S and R2S records disagree on comparison covariance".into());
                }
                if let (Some(cov), Some(sx), Some(sy)) = (
                    pair.d1s.comparison_covariance,
                    pair.d1s.standard_error_sv_h,
                    pair.r2s.standard_error_sv_h,
                ) && cov.abs() > sx * sy * (1.0 + 1e-9)
                {
                    return Err(
                        "D1S/R2S covariance exceeds the pointwise standard-error bound".into(),
                    );
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
                comparison_covariance: pair.d1s.comparison_covariance,
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
        let mut prior_artifact_sha = None;
        for pair in &event.points {
            for estimate in [&pair.d1s, &pair.r2s] {
                let artifact = estimate
                    .physical_input_artifacts
                    .get("irradiation")
                    .ok_or("dose estimate omits the irradiation schedule artifact")?;
                if prior_artifact_sha
                    .as_ref()
                    .is_some_and(|sha| sha != &artifact.sha256)
                {
                    return Err(
                        "D1S/R2S points use different irradiation schedule artifacts".into(),
                    );
                }
                prior_artifact_sha = Some(artifact.sha256.clone());
                let path = artifact.path.clone();
                let metadata = std::fs::metadata(&path).map_err(|e| e.to_string())?;
                if !metadata.is_file() || metadata.len() > MAX_CURVE_FILE_BYTES as u64 {
                    return Err("irradiation schedule artifact must be a regular file no larger than 16 MiB".into());
                }
                let mut bytes = Vec::new();
                std::fs::File::open(&path)
                    .map_err(|e| e.to_string())?
                    .take((MAX_CURVE_FILE_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                if bytes.len() > MAX_CURVE_FILE_BYTES {
                    return Err("irradiation schedule artifact exceeds 16 MiB limit".into());
                }
                let receipt: super::irradiation::IrradiationSchedule =
                    serde_json::from_slice(&bytes)
                        .map_err(|e| format!("invalid irradiation schedule artifact: {e}"))?;
                if !receipt.verify() || receipt != expected {
                    return Err(format!(
                        "irradiation schedule artifact for {} replacement #{} does not match the exact history event",
                        event.component, event.replacement_k
                    ));
                }
            }
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
            standard_error_sv_h: Some(value * 0.01),
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
                path: "run.json".into(),
                sha256: run_sha.into(),
            },
            comparison_covariance: Some(0.0),
        }
    }

    #[test]
    fn only_exact_history_event_design_and_location_curve_is_selectable() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("run.json"), b"synthetic test artifact").unwrap();
        let run_sha = sha(b"synthetic test artifact");
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
        let mut d1s = estimate(DoseMethod::D1s, &location, &run_sha, 9.0e-5);
        let mut r2s = estimate(DoseMethod::R2s, &location, &run_sha, 1.0e-4);
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
            points: vec![DosePointPair { d1s, r2s }],
        };
        let event_sha = event_identity_sha256(&event).unwrap();
        event.points[0]
            .d1s
            .physical_input_sha256
            .insert("event".into(), event_sha.clone());
        event.points[0]
            .r2s
            .physical_input_sha256
            .insert("event".into(), event_sha);
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
        };
        assert_eq!(compare(&pair), ComparisonStatus::OutsideBand);
        let mut unknown = pair;
        unknown.d1s.value_sv_h = 0.9;
        unknown.d1s.comparison_covariance = None;
        assert_eq!(compare(&unknown), ComparisonStatus::InconclusiveCovariance);
    }
}
