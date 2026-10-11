//! Runs the requests of [`faris_occt_client::protocol`] inside the worker process.
//!
//! A request is a JSON file; the worker writes the response to a second JSON file. Large data
//! (facets, points, states) goes through files named in the request, never through the
//! response. The worker sets its own address-space and CPU limits from the request before it
//! touches OpenCASCADE.

use crate::inspect::{self, INTERNAL_UNIT};
use crate::native::{FaceState, StepModel};
use crate::tessellate;
pub use faris_occt_client::protocol::*;
use std::{collections::BTreeMap, fs, time::Instant};

/// Runs one request in this process and returns the result value.
pub fn execute(operation: &Operation) -> Result<serde_json::Value, String> {
    match operation {
        Operation::Ping => serde_json::to_value(Pong {
            occt: crate::native::occt_version(),
            occt_tag: Some(crate::OCCT_TAG.into()),
            helper: Some(inspect::HELPER_VERSION.into()),
        })
        .map_err(|e| e.to_string()),
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
