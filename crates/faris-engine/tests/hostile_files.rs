//! Opening or checking a hostile file runs nothing (SEC-001). Fields hold shell
//! metacharacters, `../` climbs and paths to an executable that would leave a
//! marker if it ever ran; the tests assert that no child process starts and
//! that the marker never appears.

#![cfg(unix)]

#[path = "support/children.rs"]
mod children;

use faris_engine::{
    design::{
        DesignTransportOptions, GeometryOptions, ToolPaths, check_design_with_tools,
        design_transport,
    },
    jobs::Cancellation,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const RM_M: &str = include_str!("../../../scenarios/rm-m/rm-m.faris-design.json");

/// An executable script that records that it ran. If any hostile field makes
/// FARIS start it, `ran` appears next to it.
struct Sentinel {
    _dir: tempfile::TempDir,
    program: PathBuf,
    marker: PathBuf,
}

impl Sentinel {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        let program = dir.path().join("evil.sh");
        std::fs::write(
            &program,
            format!("#!/bin/sh\necho ran > '{}'\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            _dir: dir,
            program,
            marker,
        }
    }

    fn hostile_strings(&self) -> Vec<String> {
        let marker = self.marker.display();
        vec![
            format!("; touch {marker} #"),
            format!("$(touch {marker})"),
            format!("`touch {marker}`"),
            format!("| sh {}", self.program.display()),
            format!("&& {} &", self.program.display()),
            "../../../../../../../../bin/sh".into(),
            "-c touch".into(),
            "--output-dir=/tmp/x".into(),
            "\u{0}\u{202e}%n%s".into(),
            self.program.display().to_string(),
        ]
    }

    fn assert_never_ran(&self) {
        assert!(!self.marker.exists(), "a hostile field ran the sentinel");
    }
}

/// Replace every string under a free-text key with the next hostile string.
fn poison(value: &mut Value, hostile: &[String], next: &mut usize, only_free_text: bool) {
    const FREE_TEXT: [&str; 9] = [
        "label",
        "note",
        "name",
        "title",
        "description",
        "citation",
        "step_file",
        "path",
        "reason",
    ];
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if child.is_string() && (!only_free_text || FREE_TEXT.contains(&key.as_str())) {
                    *child = Value::String(hostile[*next % hostile.len()].clone());
                    *next += 1;
                } else {
                    poison(child, hostile, next, only_free_text);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                poison(item, hostile, next, only_free_text);
            }
        }
        _ => {}
    }
}

// Verifies: SEC-001
#[test]
fn the_process_check_can_see_a_process() {
    // Without this, "no process started" below could be a blind instrument.
    let (started, _) = children::starts_a_process(|| {
        // /bin/true on Linux, /usr/bin/true on macOS
        let program = ["/bin/true", "/usr/bin/true"]
            .into_iter()
            .find(|p| std::path::Path::new(p).exists())
            .unwrap();
        std::process::Command::new(program).status().unwrap();
    });
    assert!(started, "the child-process detector did not see a child");
}

// Verifies: SEC-001
#[test]
fn hostile_design_files_are_parsed_and_checked_without_starting_a_process() {
    let sentinel = Sentinel::new();
    let hostile = sentinel.hostile_strings();
    for only_free_text in [true, false] {
        let mut value: Value = serde_json::from_str(RM_M).unwrap();
        poison(&mut value, &hostile, &mut 0, only_free_text);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hostile.faris-design.json");
        std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        // The sentinel is configured as every tool: still, nothing in the file may start it
        // before a check reaches an external stage, and a design that does not parse reaches none.
        let tools = ToolPaths {
            cad_python: Some(sentinel.program.clone()),
            openmc_python: Some(sentinel.program.clone()),
            ..ToolPaths::default()
        };
        let bytes = std::fs::read(&path).unwrap();
        children::assert_no_process("parsing a hostile design", || {
            faris_model::design::parse_design(&bytes)
        });
        if only_free_text {
            continue;
        }
        // A design with every string replaced fails check 1 and stops there.
        children::assert_no_process("checking a design that does not parse", || {
            let _ = check_design_with_tools(&path, &tools, &GeometryOptions::default());
        });
        sentinel.assert_never_ran();
    }
    sentinel.assert_never_ran();
}

// Verifies: SEC-001
#[test]
fn a_hostile_free_text_design_with_no_tools_configured_starts_nothing() {
    let sentinel = Sentinel::new();
    let mut value: Value = serde_json::from_str(RM_M).unwrap();
    poison(&mut value, &sentinel.hostile_strings(), &mut 0, true);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hostile.faris-design.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    // No interpreter is configured, so any process would have to come from the file's text.
    children::assert_no_process("checking a hostile design with no tools", || {
        let _ = check_design_with_tools(&path, &ToolPaths::default(), &GeometryOptions::default());
    });
    sentinel.assert_never_ran();
}

fn options(report: &Path, out_dir: &Path) -> DesignTransportOptions {
    DesignTransportOptions {
        check_report: report.to_path_buf(),
        out_dir: out_dir.to_path_buf(),
        target_relative_error: 0.05,
        particles_per_batch: 1000,
        max_histories: 100_000,
        max_seconds: 60,
        seed: 1,
        threads: 1,
        window_python: None,
        openmc_package_path: None,
        group: None,
        cache_dir: None,
        progress: None,
        cancellation: Cancellation::default(),
    }
}

fn conversion_in(directory: &str) -> Value {
    json!({
        "reused": false, "directory": directory, "step_sha256": "0".repeat(64),
        "shape_unit_m": 0.001, "chord_cm": 0.1, "angular_rad": 0.1,
        "scale_factor_to_cm": 0.1, "imprint_threads": 1, "implicit_complement_tag": "void",
        "versions": {}, "timing_seconds": {}, "peak_rss_mb": 0.0,
        "h5m_sha256": "0".repeat(64), "h5m_bytes": 0, "facets_sha256": "0".repeat(64),
        "counts": {}, "helper_sha256": "0".repeat(64), "volumes": [],
    })
}

// Verifies: SEC-001
#[test]
fn a_hostile_check_report_cannot_make_transport_start_or_read_elsewhere() {
    let sentinel = Sentinel::new();
    let dir = tempfile::tempdir().unwrap();
    let design = dir.path().join("rm-m.faris-design.json");
    std::fs::write(&design, RM_M).unwrap();
    let report =
        check_design_with_tools(&design, &ToolPaths::default(), &GeometryOptions::default())
            .expect("a check report without tools");
    let mut base = serde_json::to_value(&report).unwrap();
    base["import_complete"] = json!(true);
    // The OpenMC interpreter is a setting, and here it is the sentinel: a report whose text
    // reaches the run step would start it.
    let tools = ToolPaths {
        openmc_python: Some(sentinel.program.clone()),
        cross_sections: Some(sentinel.program.clone()),
        plasma_source_path: Some(dir.path().to_path_buf()),
        ..ToolPaths::default()
    };
    // Directories a report must not be able to use: relative, climbing, shell text, and a
    // real directory (the sentinel's own) that holds no model of the recorded hash.
    let mut directories = sentinel.hostile_strings();
    directories.push(sentinel.program.parent().unwrap().display().to_string());
    directories.push(format!("{}/../..", dir.path().display()));
    for (index, directory) in directories.iter().enumerate() {
        let mut value = base.clone();
        value["geometry"]["conversion"] = conversion_in(directory);
        let path = dir.path().join(format!("report-{index}.json"));
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let out = dir.path().join(format!("run-{index}"));
        let outcome = children::assert_no_process("transport on a hostile report", || {
            design_transport(&design, &tools, &options(&path, &out))
        });
        let error = outcome
            .expect_err("a hostile report must be refused")
            .to_string();
        assert!(
            error.contains("--work-dir"),
            "{directory:?} gave an unexpected error: {error}"
        );
        assert!(
            !out.exists(),
            "no run directory may be made for {directory:?}"
        );
        sentinel.assert_never_ran();
    }
}
