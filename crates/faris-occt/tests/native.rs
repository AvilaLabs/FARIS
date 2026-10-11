//! Tests against the repository's STEP corpus: parity with the recorded Python helper reports,
//! tessellation closure, the facets round trip, and the batch face classifier.

use faris_model::design::{StepInspection, load_facets};
use faris_occt::client::{Bounds, Worker, WorkerError};
use faris_occt::native::{FaceState, StepModel};
use faris_occt::worker::{Batch, Limits, Operation, write_points};
use std::path::{Path, PathBuf};

const STEPS: [&str; 4] = [
    "seeded-missing-material-boxes",
    "seeded-missing-material-port",
    "seeded-wrong-units-boxes",
    "seeded-wrong-units-tori",
];

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../faris-engine/tests/fixtures/corpus")
}

fn worker() -> Worker {
    Worker::new(env!("CARGO_BIN_EXE_faris-occt-worker"))
}

fn run(operation: Operation) -> serde_json::Value {
    worker()
        .run(operation, Bounds::default())
        .expect("the worker succeeds")
}

fn close(a: f64, b: f64, relative: f64, scale: f64) -> bool {
    (a - b).abs() <= relative * scale.max(f64::MIN_POSITIVE)
}

/// A small deterministic generator (xorshift64*), so the tests need no random-number crate.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let bits = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11;
        bits as f64 / (1u64 << 53) as f64
    }
}

#[test]
fn inspect_matches_the_recorded_python_reports() {
    for name in STEPS {
        let step = corpus().join(format!("{name}.step"));
        let recorded: StepInspection = serde_json::from_slice(
            &std::fs::read(corpus().join(format!("{name}.inspect.json"))).unwrap(),
        )
        .unwrap();
        let value = run(Operation::Inspect {
            step,
            assume_unit: None,
        });
        // The native report parses as the report FARIS already reads.
        let native = StepInspection::from_bytes(&serde_json::to_vec(&value).unwrap())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(native.step.sha256, recorded.step.sha256, "{name}");
        assert_eq!(native.unit, recorded.unit, "{name}: unit");
        assert_eq!(native.problems, recorded.problems, "{name}");
        assert_eq!(
            native.solids.len(),
            recorded.solids.len(),
            "{name}: solid count"
        );
        for (n, r) in native.solids.iter().zip(&recorded.solids) {
            assert_eq!(n.step_index, r.step_index, "{name}");
            assert_eq!(n.step_name, r.step_name, "{name} solid {}", r.step_index);
            assert_eq!(n.occt_valid, r.occt_valid, "{name} solid {}", r.step_index);
            let (v, w) = (n.cad_volume_m3.unwrap(), r.cad_volume_m3.unwrap());
            assert!(
                close(v, w, 1e-9, w.abs()),
                "{name} solid {}: volume {v} against {w}",
                r.step_index
            );
            let (lo, hi) = (r.bbox_min_m.unwrap(), r.bbox_max_m.unwrap());
            let diagonal = (0..3)
                .map(|i| (hi[i] - lo[i]) * (hi[i] - lo[i]))
                .sum::<f64>()
                .sqrt();
            for i in 0..3 {
                let (c, d) = (n.centroid_m.unwrap()[i], r.centroid_m.unwrap()[i]);
                assert!(
                    close(c, d, 1e-9, diagonal),
                    "{name} solid {}: centroid {c} against {d}",
                    r.step_index
                );
                for (a, b) in [
                    (n.bbox_min_m.unwrap()[i], lo[i]),
                    (n.bbox_max_m.unwrap()[i], hi[i]),
                ] {
                    assert!(
                        close(a, b, 1e-9, diagonal),
                        "{name} solid {}: bbox {a} against {b}",
                        r.step_index
                    );
                }
            }
        }
    }
}

#[test]
fn tessellation_is_closed_oriented_and_loads_as_facets() {
    for name in STEPS {
        let step = corpus().join(format!("{name}.step"));
        let scratch = tempfile::tempdir().unwrap();
        let base = scratch.path().join("solids");
        let report = run(Operation::Tessellate {
            step: step.clone(),
            out_base: base.clone(),
            chord_cm: 0.1,
            angular: 0.1,
            shape_unit_m: 1.0e-3,
            tags: None,
        });
        assert_eq!(report["problems"], serde_json::json!([]), "{name}");
        let binary = std::fs::read(scratch.path().join("solids.facets.bin")).unwrap();
        let manifest = std::fs::read(scratch.path().join("solids.facets.json")).unwrap();
        let loaded = load_facets(&binary, &manifest).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(loaded.facets.unimprinted());
        let inspected = run(Operation::Inspect {
            step,
            assume_unit: None,
        });
        for (solid, result) in report["solids"].as_array().unwrap().iter().enumerate() {
            assert_eq!(result["closed"], true, "{name} solid {solid}");
            let faceted = result["faceted_volume_m3"].as_f64().unwrap();
            let exact = inspected["solids"][solid]["cad_volume_m3"]
                .as_f64()
                .unwrap();
            // A faceted solid is inside or outside its exact solid by the chord error.
            assert!(
                (faceted - exact).abs() <= 0.05 * exact,
                "{name} solid {solid}: {faceted} against {exact}"
            );
        }
    }
}

#[test]
fn batch_classifier_matches_per_point_calls_on_every_face() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut mixed_faces = 0;
    let mut faces_checked = 0;
    for name in [
        "seeded-wrong-units-tori",
        "seeded-missing-material-port",
        "seeded-wrong-units-boxes",
    ] {
        let step = corpus().join(format!("{name}.step"));
        let model = StepModel::read(step.to_str().unwrap(), "MM").unwrap();
        for solid in 0..model.solid_count().min(3) {
            for face in 1..=model.face_count(solid).unwrap() {
                let [u0, u1, v0, v1] = model.face_uv_bounds(solid, face).unwrap();
                let (du, dv) = (0.1 * (u1 - u0), 0.1 * (v1 - v0));
                let n = 2000;
                let u: Vec<f64> = (0..n)
                    .map(|_| u0 - du + (u1 - u0 + 2.0 * du) * rng.next())
                    .collect();
                let v: Vec<f64> = (0..n)
                    .map(|_| v0 - dv + (v1 - v0 + 2.0 * dv) * rng.next())
                    .collect();
                let batch = model.classify(solid, face, 1e-7, &u, &v).unwrap();
                let single = model.classify_per_point(solid, face, 1e-7, &u, &v).unwrap();
                assert_eq!(batch, single, "{name} solid {solid} face {face}");
                faces_checked += 1;
                let inside = batch.iter().filter(|s| **s == FaceState::In).count();
                if inside > 0 && inside < n {
                    mixed_faces += 1;
                }
                // The widened box has points beyond the face's UV box, which are out.
                assert!(
                    batch
                        .iter()
                        .any(|s| *s == FaceState::In || *s == FaceState::Out)
                );
            }
        }
    }
    assert!(
        faces_checked >= 30,
        "only {faces_checked} faces were checked"
    );
    assert!(
        mixed_faces >= 10,
        "only {mixed_faces} faces had both inside and outside points"
    );
}

#[test]
fn the_worker_classifies_batches_from_files() {
    let step = corpus().join("seeded-wrong-units-tori.step");
    let model = StepModel::read(step.to_str().unwrap(), "MM").unwrap();
    let [u0, u1, v0, v1] = model.face_uv_bounds(0, 1).unwrap();
    let mut rng = Rng(7);
    let u: Vec<f64> = (0..500).map(|_| u0 + (u1 - u0) * rng.next()).collect();
    let v: Vec<f64> = (0..500).map(|_| v0 + (v1 - v0) * rng.next()).collect();
    let expected: Vec<u8> = model
        .classify(0, 1, 1e-7, &u, &v)
        .unwrap()
        .into_iter()
        .map(|s| s as u8)
        .collect();
    let scratch = tempfile::tempdir().unwrap();
    let (points, states) = (scratch.path().join("p.bin"), scratch.path().join("s.bin"));
    write_points(&points, &u, &v).unwrap();
    let result = run(Operation::Classify {
        step,
        tolerance: 1e-7,
        batches: vec![Batch {
            solid: 0,
            face: 1,
            points,
            states: states.clone(),
        }],
    });
    assert_eq!(result["total_points"], 500);
    assert_eq!(std::fs::read(states).unwrap(), expected);
}

#[test]
fn reference_classifier_agrees_with_the_batch_classifier_off_the_boundary() {
    let step = corpus().join("seeded-wrong-units-boxes.step");
    let model = StepModel::read(step.to_str().unwrap(), "MM").unwrap();
    let mut rng = Rng(11);
    let mut compared = 0;
    for face in 1..=model.face_count(0).unwrap() {
        let [u0, u1, v0, v1] = model.face_uv_bounds(0, face).unwrap();
        let u: Vec<f64> = (0..500)
            .map(|_| u0 - 0.2 * (u1 - u0) + 1.4 * (u1 - u0) * rng.next())
            .collect();
        let v: Vec<f64> = (0..500)
            .map(|_| v0 - 0.2 * (v1 - v0) + 1.4 * (v1 - v0) * rng.next())
            .collect();
        let batch = model.classify(0, face, 1e-7, &u, &v).unwrap();
        let reference = model.classify_reference(0, face, 1e-7, &u, &v).unwrap();
        for (a, b) in batch.iter().zip(&reference) {
            assert_eq!(a, b, "face {face}");
            compared += 1;
        }
    }
    assert!(compared > 0);
}

#[test]
fn a_missing_file_is_a_reported_failure_not_a_crash() {
    let error = worker()
        .run(
            Operation::Inspect {
                step: PathBuf::from("/nonexistent/model.step"),
                assume_unit: None,
            },
            Bounds::default(),
        )
        .unwrap_err();
    assert!(matches!(error, WorkerError::Failed(_)), "{error}");
}

#[test]
fn a_garbage_file_is_reported_not_fatal() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("garbage.step");
    std::fs::write(&path, b"ISO-10303-21;\nHEADER;\nthis is not a model\n").unwrap();
    let value = worker().run(
        Operation::Inspect {
            step: path,
            assume_unit: None,
        },
        Bounds::default(),
    );
    match value {
        // Either the reader refuses it (a reported problem) or the worker reports a failure.
        Ok(report) => assert!(
            !report["problems"].as_array().unwrap().is_empty()
                || report["solids"] == serde_json::json!([])
        ),
        Err(error) => assert!(
            matches!(error, WorkerError::Failed(_) | WorkerError::Crashed { .. }),
            "{error}"
        ),
    }
}

#[test]
fn a_hung_worker_is_stopped_at_the_wall_limit() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = tempfile::tempdir().unwrap();
    let script = scratch.path().join("hang");
    std::fs::write(&script, "#!/bin/sh\nexec sleep 60\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let started = std::time::Instant::now();
    let error = Worker::new(script)
        .run(
            Operation::Ping,
            Bounds {
                wall_seconds: 0.5,
                limits: Limits::default(),
            },
        )
        .unwrap_err();
    assert!(matches!(error, WorkerError::TimedOut { .. }), "{error}");
    assert!(started.elapsed().as_secs_f64() < 10.0);
}

#[test]
fn a_crashing_worker_is_reported() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = tempfile::tempdir().unwrap();
    let script = scratch.path().join("crash");
    std::fs::write(&script, "#!/bin/sh\nkill -SEGV $$\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let error = Worker::new(script)
        .run(Operation::Ping, Bounds::default())
        .unwrap_err();
    assert!(matches!(error, WorkerError::Crashed { .. }), "{error}");
}

#[test]
fn the_memory_limit_stops_a_worker_that_needs_more() {
    // The worker's own mappings already exceed this, so reading a model cannot allocate.
    let limits = Limits {
        address_space_mb: Some(16),
        cpu_seconds: Some(60),
    };
    let error = worker()
        .run(
            Operation::Inspect {
                step: corpus().join("seeded-wrong-units-tori.step"),
                assume_unit: None,
            },
            Bounds {
                wall_seconds: 30.0,
                limits,
            },
        )
        .unwrap_err();
    assert!(
        matches!(error, WorkerError::Crashed { .. } | WorkerError::Failed(_)),
        "{error}"
    );
}

#[test]
fn the_ping_reports_the_pinned_release() {
    let value = run(Operation::Ping);
    let version = value["occt"].as_str().unwrap();
    assert!(version.starts_with("7.9.3"), "{version}");
    assert_eq!(faris_occt::OCCT_TAG, "V7_9_3");
}
