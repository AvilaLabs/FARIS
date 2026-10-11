//! The requests the worker runs and the files they name: the protocol between FARIS and
//! `faris-occt-worker`, with no OpenCASCADE in it.
//!
//! A request is a JSON file; the worker writes the response to a second JSON file. Large data
//! (facets, points, states) goes through files named in the request, never through the
//! response. The worker sets its own address-space and CPU limits from the request before it
//! touches OpenCASCADE.

use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

pub const REQUEST_SCHEMA: &str = "faris-occt-request/v1";
pub const RESPONSE_SCHEMA: &str = "faris-occt-response/v1";

/// One material tag, as `tessellate` takes it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tag {
    pub step_index: usize,
    pub tag: String,
}

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

/// The answer to [`Operation::Ping`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pong {
    /// The OpenCASCADE version the worker links, for example `7.9.3`.
    pub occt: String,
    /// The release tag its source was taken from, for example `V7_9_3`.
    #[serde(default)]
    pub occt_tag: Option<String>,
    /// The worker's helper version (the `versions.helper` of its reports).
    #[serde(default)]
    pub helper: Option<String>,
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
    /// Does nothing; reports the OpenCASCADE version and the worker's identity ([`Pong`]).
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
