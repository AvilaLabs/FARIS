// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! Workload-shaped memory lower bounds for PERF-041 admission.
//!
//! These byte counts describe storage implied by the request, not measured peak
//! RSS or a prediction of solver allocator, runtime, or native-library
//! overhead. Unknown terms stay explicit. The external-job address-space cap
//! remains the hard ceiling.

use crate::memory::{Admission, MemoryPolicy, admit};

/// Dimensions known before launching a solver or study job.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkloadFeatures {
    /// Number of mesh bins whose scalar response is retained.
    pub mesh_bins: u64,
    /// Number of ordinary (non-mesh) scalar response values in the request.
    pub responses: u64,
    /// Planned histories, recorded for context but not multiplied into RAM.
    pub histories: u64,
    /// Planned batch count, recorded for context only.
    pub batches: u64,
    /// Known CAD tessellation triangles, recorded for context only.
    pub triangles: u64,
    /// Serialized request/input bytes, recorded for context only.
    pub known_input_bytes: u64,
}

/// An auditable lower bound plus allocations not represented by those terms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projection {
    pub known_lower_bound_bytes: u64,
    pub admission_bytes: u64,
    pub unknown_overhead: bool,
    pub explanation: String,
}

impl WorkloadFeatures {
    /// Build a structural lower bound from explicit scalar result slots. The
    /// count excludes solver working storage, histories, batch accumulators,
    /// and native library allocations. It is not a peak RSS forecast.
    pub fn project(self, cap_bytes: u64) -> Projection {
        // Each requested mesh bin contributes one scalar tally value. Ordinary
        // responses contribute one scalar value each. Batch counts and histories
        // are workload context only; no per-batch retention is assumed.
        let scalar_results = self
            .mesh_bins
            .saturating_add(self.responses)
            .saturating_mul(8);
        // Triangle counts are retained as workload context only: meshes may
        // share vertices and native CAD layouts are implementation-dependent.
        let lower = scalar_results;
        // Solver and runtime allocations remain unmodeled, so the safe
        // admission reservation is still the cap. Keep the workload lower
        // bound for explanation; never use it to weaken the existing reserve
        // check as if it were a peak estimate.
        let admission_bytes = cap_bytes;
        Projection {
            known_lower_bound_bytes: lower,
            admission_bytes,
            unknown_overhead: true,
            explanation: format!(
                "known workload lower bound {} ({} mesh-bin scalars; {} ordinary response scalars; {} batches; {} triangles; {} request/input bytes recorded for context but not counted as resident); solver/runtime overhead is unknown, hard cap {}",
                gib(lower),
                self.mesh_bins,
                self.responses,
                self.batches,
                self.triangles,
                self.known_input_bytes,
                gib(cap_bytes)
            ),
        }
    }
}

/// Apply the existing free-memory and reserve rule to the workload-derived
/// admission figure. The hard cap is applied before this function.
pub fn admit_workload(
    features: WorkloadFeatures,
    cap_bytes: u64,
    policy: &MemoryPolicy,
) -> (Projection, Admission) {
    let projection = features.project(cap_bytes);
    let decision = admit(projection.admission_bytes, policy);
    (projection, decision)
}

/// Fallback for a job with no usable dimensions: preserve cap-based admission.
pub fn unknown_workload(cap_bytes: u64) -> Projection {
    Projection {
        known_lower_bound_bytes: 0,
        admission_bytes: cap_bytes,
        unknown_overhead: true,
        explanation: format!(
            "workload dimensions unavailable; using hard cap {}",
            gib(cap_bytes)
        ),
    }
}

fn gib(bytes: u64) -> String {
    format!("{:.2} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{GIB, MIB, decide};

    #[test]
    fn dimensions_account_only_for_explicit_storage_terms() {
        let projection = WorkloadFeatures {
            mesh_bins: 100,
            responses: 3,
            histories: 50_000_000,
            batches: 30,
            triangles: 1000,
            known_input_bytes: MIB,
        }
        .project(4 * GIB);
        assert_eq!(projection.known_lower_bound_bytes, 100 * 8 + 3 * 8);
        assert_eq!(projection.admission_bytes, 4 * GIB);
        assert!(projection.unknown_overhead);
        assert!(projection.explanation.contains("overhead is unknown"));
    }

    #[test]
    fn huge_known_workload_is_capped_and_refused_against_free_plus_margin() {
        let features = WorkloadFeatures {
            mesh_bins: u64::MAX,
            responses: 2,
            ..Default::default()
        };
        let policy = MemoryPolicy {
            cap_bytes: Some(4 * GIB),
            margin_bytes: GIB,
        };
        let projection = features.project(4 * GIB);
        let decision = decide(projection.admission_bytes, Ok(4 * GIB), policy.margin_bytes);
        assert_eq!(projection.admission_bytes, 4 * GIB);
        assert!(matches!(decision, Admission::Refused(_)));
    }

    #[test]
    fn missing_features_preserve_cap_fallback() {
        assert_eq!(unknown_workload(4 * GIB).admission_bytes, 4 * GIB);
    }
}
