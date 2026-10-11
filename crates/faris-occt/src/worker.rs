//! The requests the worker runs and the code that runs them inside the worker process.
//!
//! A request is a JSON file; the worker writes the response to a second JSON file. Large data
//! (facets, points, states) goes through files named in the request, never through the
//! response. The worker sets its own address-space and CPU limits from the request before it
//! touches OpenCASCADE.

use crate::inspect::{self, INTERNAL_UNIT};
use crate::native::{FaceState, StepModel};
use crate::tessellate::{self, Tag};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::PathBuf, time::Instant};

pub const REQUEST_SCHEMA: &str = "faris-occt-request/v1";
pub const RESPONSE_SCHEMA: &str = "faris-occt-response/v1";

/// What the worker limits itself to.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Limits {
    /// `RLIMIT_AS` in MiB.
    #[serde(default)]
    pub address_space_mb: Option<u64>,
    /// `RLIMIT_CPU` in seconds.
    #[serde(default)]
    pub cpu_seconds: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Batch {
    pub solid: usize,
    /// 1-based, explorer order within the solid.
    pub face: usize,
    /// Little-endian `f64` pairs `u0 v0 u1 v1 ...`.
    pub points: PathBuf,
    /// One byte per point: 0 in, 1 on, 2 out, 3 unknown.
    pub states: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Operation {
    /// Does nothing; reports the OpenCASCADE version.
    Ping,
    /// `faris-step-inspect/v1` of a STEP file.
    Inspect {
        step: PathBuf,
        #[serde(default)]
        assume_unit: Option<String>,
    },
    /// Per-solid meshes as `faris-facets/v1` (unimprinted).
    Tessellate {
        step: PathBuf,
        /// Output path without the `.facets.bin` / `.facets.json` suffixes.
        out_base: PathBuf,
        chord_cm: f64,
        angular: f64,
        shape_unit_m: f64,
        #[serde(default)]
        tags: Option<Vec<Tag>>,
    },
    /// Batches of (u, v) points classified against faces, one native call per batch.
    Classify {
        step: PathBuf,
        tolerance: f64,
        batches: Vec<Batch>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub schema: String,
    #[serde(default)]
    pub limits: Limits,
    #[serde(flatten)]
    pub operation: Operation,
}

impl Request {
    pub fn new(operation: Operation, limits: Limits) -> Self {
        Self {
            schema: REQUEST_SCHEMA.into(),
            limits,
            operation,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Ok { result: serde_json::Value },
    Error { message: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub schema: String,
    #[serde(flatten)]
    pub outcome: Outcome,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BatchResult {
    pub solid: usize,
    pub face: usize,
    pub points: usize,
    pub inside: usize,
    pub on: usize,
    pub outside: usize,
    pub unknown: usize,
    pub seconds: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClassifyResult {
    pub read_seconds: f64,
    pub batches: Vec<BatchResult>,
    pub total_points: usize,
    pub classify_seconds: f64,
    pub peak_rss_mb: f64,
}

/// Reads little-endian `f64` pairs and splits them into u and v.
pub fn read_points(path: &std::path::Path) -> Result<(Vec<f64>, Vec<f64>), String> {
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() % 16 != 0 {
        return Err(format!(
            "{} is not a whole number of (u, v) pairs",
            path.display()
        ));
    }
    let n = bytes.len() / 16;
    let (mut u, mut v) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for pair in bytes.as_chunks::<16>().0 {
        u.push(f64::from_le_bytes(
            pair[..8].try_into().expect("eight bytes"),
        ));
        v.push(f64::from_le_bytes(
            pair[8..].try_into().expect("eight bytes"),
        ));
    }
    Ok((u, v))
}

/// Writes (u, v) pairs in the batch file layout.
pub fn write_points(path: &std::path::Path, u: &[f64], v: &[f64]) -> Result<(), String> {
    let mut bytes = Vec::with_capacity(u.len() * 16);
    for (a, b) in u.iter().zip(v) {
        bytes.extend_from_slice(&a.to_le_bytes());
        bytes.extend_from_slice(&b.to_le_bytes());
    }
    fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Runs one request in this process and returns the result value.
pub fn execute(operation: &Operation) -> Result<serde_json::Value, String> {
    match operation {
        Operation::Ping => Ok(serde_json::json!({ "occt": crate::native::occt_version() })),
        Operation::Inspect { step, assume_unit } => {
            let report = inspect::inspect_step(step, assume_unit.as_deref())?;
            serde_json::to_value(report).map_err(|e| e.to_string())
        }
        Operation::Tessellate {
            step,
            out_base,
            chord_cm,
            angular,
            shape_unit_m,
            tags,
        } => {
            let report = tessellate::tessellate_step(
                step,
                out_base,
                &tessellate::Options {
                    chord_cm: *chord_cm,
                    angular: *angular,
                    shape_unit_m: *shape_unit_m,
                    tags: tags.clone(),
                },
            )?;
            serde_json::to_value(report).map_err(|e| e.to_string())
        }
        Operation::Classify {
            step,
            tolerance,
            batches,
        } => {
            let started = Instant::now();
            let model = StepModel::read(&step.display().to_string(), INTERNAL_UNIT)?;
            let read_seconds = started.elapsed().as_secs_f64();
            let mut results = Vec::new();
            let mut total = 0usize;
            let classify_started = Instant::now();
            for batch in batches {
                let (u, v) = read_points(&batch.points)?;
                let t = Instant::now();
                let states = model.classify(batch.solid, batch.face, *tolerance, &u, &v)?;
                let seconds = t.elapsed().as_secs_f64();
                let mut counts: BTreeMap<u8, usize> = BTreeMap::new();
                for s in &states {
                    *counts.entry(*s as u8).or_default() += 1;
                }
                let bytes: Vec<u8> = states.iter().map(|s| *s as u8).collect();
                fs::write(&batch.states, bytes)
                    .map_err(|e| format!("{}: {e}", batch.states.display()))?;
                total += u.len();
                results.push(BatchResult {
                    solid: batch.solid,
                    face: batch.face,
                    points: u.len(),
                    inside: counts.get(&(FaceState::In as u8)).copied().unwrap_or(0),
                    on: counts.get(&(FaceState::On as u8)).copied().unwrap_or(0),
                    outside: counts.get(&(FaceState::Out as u8)).copied().unwrap_or(0),
                    unknown: counts
                        .get(&(FaceState::Unknown as u8))
                        .copied()
                        .unwrap_or(0),
                    seconds,
                });
            }
            let result = ClassifyResult {
                read_seconds,
                batches: results,
                total_points: total,
                classify_seconds: classify_started.elapsed().as_secs_f64(),
                peak_rss_mb: tessellate::peak_rss_mb(),
            };
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
    }
}
