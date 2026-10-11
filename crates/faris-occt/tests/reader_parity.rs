//! The two CAD readers of `faris design check`, the native worker and the Python helper, run on
//! the same STEP files through the engine's own backends: the STEP inspect and the per-solid
//! tessellation must agree (facets byte for byte, inspect fields to rounding).
//!
//! Needs the CAD interpreter (cadquery 2.8 / OCP 7.9.3.1) in `FARIS_CAD_PYTHON`; without it the
//! tests say so and pass, so a plain `cargo test -p faris-occt` needs no Python. RM-M, the large
//! model, runs with `-- --ignored` and `FARIS_RM_M_STEP` naming the STEP file. `make occt-check`
//! sets both.

use faris_engine::design::{
    Backends, CadReader, ExternalBackends, TagEntry, TessellateRequest, ToolPaths, ToolResult,
};
use faris_engine::jobs::Cancellation;
use serde_json::Value;
use std::path::{Path, PathBuf};

const CHORD_CM: f64 = 0.1;
const ANGULAR_RAD: f64 = 0.1;

const STEPS: [&str; 4] = [
    "seeded-missing-material-boxes",
    "seeded-missing-material-port",
    "seeded-wrong-units-boxes",
    "seeded-wrong-units-tori",
];

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../faris-engine/tests/fixtures/corpus")
}

fn python() -> Option<PathBuf> {
    let python = std::env::var_os("FARIS_CAD_PYTHON").map(PathBuf::from);
    if python.is_none() {
        eprintln!("FARIS_CAD_PYTHON is not set: the reader parity test is skipped");
    }
    python
}

fn tools(reader: CadReader, python: &Path) -> ToolPaths {
    ToolPaths {
        cad_python: Some(python.to_path_buf()),
        cad_reader: Some(reader),
        occt_worker: Some(PathBuf::from(env!("CARGO_BIN_EXE_faris-occt-worker"))),
        ..ToolPaths::default()
    }
}

/// Numbers agree to a relative 1e-12 (or 1e-15 absolute, metres); everything else exactly.
fn same(a: &Value, b: &Value, at: &str) {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
            assert!(
                (x - y).abs() <= 1e-12 * x.abs().max(y.abs()) + 1e-15,
                "{at}: {x} against {y}"
            );
        }
        (Value::Array(x), Value::Array(y)) => {
            assert_eq!(x.len(), y.len(), "{at}: length");
            for (i, (x, y)) in x.iter().zip(y).enumerate() {
                same(x, y, &format!("{at}[{i}]"));
            }
        }
        (Value::Object(x), Value::Object(y)) => {
            assert_eq!(
                x.keys().collect::<Vec<_>>(),
                y.keys().collect::<Vec<_>>(),
                "{at}: fields"
            );
            for (key, x) in x {
                same(x, &y[key], &format!("{at}.{key}"));
            }
        }
        _ => assert_eq!(a, b, "{at}"),
    }
}

fn parity(step: &Path, work: &Path, python: &Path) {
    let cancel = Cancellation::default();
    let (native_tools, python_tools) = (
        tools(CadReader::Native, python),
        tools(CadReader::Python, python),
    );
    let native = ExternalBackends::new(&native_tools, &cancel);
    let helper = ExternalBackends::new(&python_tools, &cancel);

    let ran_native = native.inspect_step(step, None).expect("native inspect");
    let ran_python = helper.inspect_step(step, None).expect("python inspect");
    assert_eq!(ran_native.reader.reader, "native");
    assert_eq!(ran_python.reader.reader, "python");
    let strip = |inspection: &faris_model::design::StepInspection| {
        let mut value = serde_json::to_value(inspection).unwrap();
        let fields = value.as_object_mut().unwrap();
        fields.remove("versions");
        fields.remove("timing_seconds");
        value
    };
    same(
        &strip(&ran_native.inspection),
        &strip(&ran_python.inspection),
        "inspect",
    );

    let inspection = &ran_native.inspection;
    let tags: Vec<TagEntry> = inspection
        .solids
        .iter()
        .map(|s| TagEntry {
            step_index: s.step_index,
            tag: format!("solid-{:03}", s.step_index + 1),
        })
        .collect();
    let shape_unit_m = if inspection.unit.declared.is_some() {
        1.0e-3
    } else {
        1.0
    };
    let tessellate = |backends: &ExternalBackends, name: &str| {
        let directory = work.join(name);
        let request = TessellateRequest {
            step,
            step_sha256: &inspection.step.sha256,
            shape_unit_m,
            tags: &tags,
            chord_cm: CHORD_CM,
            angular_rad: ANGULAR_RAD,
            directory: &directory,
            workers: 1,
            reuse: false,
            shape_cache: None,
        };
        let ToolResult::Done(outcome) = backends.tessellate(&request).expect("tessellate") else {
            panic!("{name}: the tessellation did not run");
        };
        (outcome, directory)
    };
    let (by_native, native_dir) = tessellate(&native, "native");
    let (by_python, python_dir) = tessellate(&helper, "python");
    assert!(by_native.reader.is_native() && !by_python.reader.is_native());
    assert_eq!(by_native.problems, by_python.problems, "problems");
    let bytes = |dir: &Path| std::fs::read(dir.join("solids.facets.bin")).unwrap();
    assert!(
        bytes(&native_dir) == bytes(&python_dir),
        "the facets are not byte-identical"
    );
    assert_eq!(
        by_native.manifest.binary_sha256,
        by_python.manifest.binary_sha256
    );
    // The manifest names the program that wrote it (`helper_version`); nothing else may differ.
    let manifest = |dir: &Path| -> Value {
        let mut value: Value =
            serde_json::from_slice(&std::fs::read(dir.join("solids.facets.json")).unwrap())
                .unwrap();
        value.as_object_mut().unwrap().remove("helper_version");
        value
    };
    assert_eq!(
        manifest(&native_dir),
        manifest(&python_dir),
        "facets manifest"
    );
    eprintln!(
        "{}: {} solids, {} triangles; inspect {:.1} s native against {:.1} s python; \
         tessellation {:.1} s against {:.1} s",
        step.display(),
        inspection.solids.len(),
        by_native.facets.triangles.len(),
        ran_native.elapsed_seconds,
        ran_python.elapsed_seconds,
        by_native.seconds,
        by_python.seconds
    );
}

#[test]
fn both_readers_agree_on_the_corpus() {
    let Some(python) = python() else { return };
    let work = tempfile::tempdir().unwrap();
    for name in STEPS {
        let dir = work.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        parity(&corpus().join(format!("{name}.step")), &dir, &python);
    }
}

#[test]
#[ignore = "RM-M takes minutes: run with --ignored and FARIS_RM_M_STEP and FARIS_CAD_PYTHON set"]
fn both_readers_agree_on_rm_m() {
    let Some(python) = python() else { return };
    let step = PathBuf::from(
        std::env::var_os("FARIS_RM_M_STEP").expect("FARIS_RM_M_STEP names the RM-M STEP file"),
    );
    let work = tempfile::tempdir().unwrap();
    parity(&step, work.path(), &python);
}
