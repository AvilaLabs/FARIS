// Synthetic decay curve for a test input; not an evidence computation.
#![allow(clippy::disallowed_methods)]

//! `faris maintenance`: argument handling, input validation before any tool runs, and the
//! report on a result file.

use faris_engine::maintenance::MaintenanceResult;
use std::path::Path;
use std::process::{Command, Output};

fn faris(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_faris"))
        .args(arguments)
        .output()
        .expect("run faris")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn run_args<'a>(
    designs: &'a str,
    assumptions: &'a str,
    output: &'a str,
    extra: &[&'a str],
) -> Vec<&'a str> {
    let mut args = vec![
        "maintenance",
        "run",
        "--designs",
        designs,
        "--assumptions",
        assumptions,
        "--actinv",
        "/nonexistent/actinv",
        "--data-dir",
        "/nonexistent/data",
        "--output",
        output,
    ];
    args.extend_from_slice(extra);
    args
}

const ASSUMPTIONS: &str = r#"{
  "schema_version": "faris-maintenance-assumptions/v0.1",
  "governing_quantity": "heat",
  "classes": {"magnet": {"component_id": "magnets", "governing": ["magnets"], "work_s": 1.0,
                         "threshold": {"kind": "q_star", "q_star_w_per_m3": 1.0}}}
}"#;

#[test]
fn run_help_lists_every_option() {
    let out = faris(&["maintenance", "run", "--help"]);
    assert!(out.status.success());
    let help = text(&out.stdout);
    for option in [
        "--designs",
        "--assumptions",
        "--actinv",
        "--data-dir",
        "--output",
        "--cache",
        "--work-dir",
        "--workers",
        "--impurities",
        "--python",
        "--builder",
    ] {
        assert!(help.contains(option), "{option} missing from help");
    }
    // The default is described by its rule, never as a path on the build machine.
    assert!(help.contains("tools/build_activation_inputs.py"));
    assert!(!help.contains(env!("CARGO_MANIFEST_DIR")), "{help}");
}

#[test]
fn argument_errors_exit_with_the_usage_code() {
    // Missing required options and a bad worker count are usage errors (exit 64).
    assert_eq!(faris(&["maintenance", "run"]).status.code(), Some(64));
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("r.json");
    let args = run_args("d.json", "a.json", path(&out), &["--workers", "0"]);
    let result = faris(&args);
    assert_eq!(result.status.code(), Some(64));
    assert!(text(&result.stderr).contains("workers"));
    let args = run_args("d.json", "a.json", path(&out), &["--workers", "65"]);
    assert_eq!(faris(&args).status.code(), Some(64));
    assert_eq!(
        faris(&["maintenance", "report", "--format", "yaml", "x"])
            .status
            .code(),
        Some(64)
    );
    assert_eq!(
        faris(&["maintenance", "frobnicate"]).status.code(),
        Some(64)
    );
}

#[test]
fn run_validates_its_inputs_before_running_anything() {
    let dir = tempfile::tempdir().unwrap();
    let designs = dir.path().join("designs.json");
    let assumptions = dir.path().join("assumptions.json");
    let output = dir.path().join("result.json");
    std::fs::write(&assumptions, ASSUMPTIONS).unwrap();

    // An existing output is never overwritten.
    std::fs::write(&output, "keep").unwrap();
    std::fs::write(&designs, "{}").unwrap();
    let result = faris(&run_args(
        path(&designs),
        path(&assumptions),
        path(&output),
        &[],
    ));
    assert_eq!(result.status.code(), Some(2));
    assert!(text(&result.stderr).contains("refusing to overwrite"));
    assert_eq!(std::fs::read_to_string(&output).unwrap(), "keep");
    std::fs::remove_file(&output).unwrap();

    // Each bad input is named, and nothing is written.
    let cases: Vec<(&str, &str, &str)> = vec![
        ("{}", ASSUMPTIONS, "missing field"),
        (
            r#"{"schema_version": "faris-maintenance-designs/v0.1", "designs": {}}"#,
            ASSUMPTIONS,
            "names no design",
        ),
        (
            r#"{"schema_version": "faris-maintenance-designs/v0.2", "designs": {}}"#,
            ASSUMPTIONS,
            "schema_version must be faris-maintenance-designs/v0.1",
        ),
        (
            r#"{"schema_version": "faris-maintenance-designs/v0.1", "designs": {"a": {
                "scenario": "s.json", "physics": "p.json", "history_run": "r.json",
                "spectrum_run": "x.json", "history_assumptions": "h.json"}}}"#,
            ASSUMPTIONS,
            "design a: scenario",
        ),
        (
            r#"{"schema_version": "faris-maintenance-designs/v0.1", "designs": {"a": {
                "scenario": "s.json", "physics": "p.json", "history_run": "r.json",
                "spectrum_run": "x.json", "history_assumptions": "h.json"}}}"#,
            r#"{"schema_version": "faris-maintenance-assumptions/v0.1", "governing_quantity": "dose",
                "classes": {"m": {"component_id": "m", "governing": ["m"], "work_s": 1.0,
                                  "threshold": {"kind": "q_star", "q_star_w_per_m3": 1.0}}}}"#,
            "dose-governed durations are not supported in 0.2",
        ),
        (
            r#"{"schema_version": "faris-maintenance-designs/v0.1", "designs": {"a": {
                "scenario": "s.json", "physics": "p.json", "history_run": "r.json",
                "spectrum_run": "x.json", "history_assumptions": "h.json"}}}"#,
            r#"{"schema_version": "faris-maintenance-assumptions/v0.1", "governing_quantity": "heat",
                "classes": {"m": {"component_id": "m", "governing": ["m"], "work_s": 1.0,
                                  "threshold": {"kind": "calibrate", "design": "ghost", "target_cooldown_s": 5.0}}}}"#,
            "calibrates on design ghost",
        ),
    ];
    for (designs_text, assumptions_text, expected) in cases {
        std::fs::write(&designs, designs_text).unwrap();
        std::fs::write(&assumptions, assumptions_text).unwrap();
        // The design files exist for the cases that get past the file check.
        if expected.contains("dose") || expected.contains("ghost") {
            for f in ["s.json", "p.json", "r.json", "x.json", "h.json"] {
                std::fs::write(dir.path().join(f), "{}").unwrap();
            }
        }
        let result = faris(&run_args(
            path(&designs),
            path(&assumptions),
            path(&output),
            &[],
        ));
        assert_eq!(result.status.code(), Some(2), "{expected}");
        assert!(
            text(&result.stderr).contains(expected),
            "{expected}: {}",
            text(&result.stderr)
        );
        assert!(!output.exists());
    }
}

fn sample_result_file(dir: &Path) -> std::path::PathBuf {
    use faris_engine::{
        fixtures,
        history::run_operating_history_cancellable,
        jobs::Cancellation,
        maintenance::{
            CurveRequest, DecaySource, DesignInput, InstallationCurve, NotEvaluated,
            run_maintenance,
        },
    };
    use faris_model::maintenance::MaintenanceAssumptions;
    struct Flat;
    impl DecaySource for Flat {
        fn curves(
            &mut self,
            _design: &str,
            _history: &faris_engine::history::HistoryResult,
            requests: &[CurveRequest],
            grid_s: &[f64],
            _cancel: &Cancellation,
        ) -> Result<Vec<Result<InstallationCurve, NotEvaluated>>, String> {
            Ok(requests
                .iter()
                .map(|_| {
                    Ok(InstallationCurve {
                        points: grid_s
                            .iter()
                            .map(|&t| (t, 40.0 * (-t / 8.0).exp()))
                            .collect(),
                        volume_m3: 2.0,
                        install_s: 0.0,
                    })
                })
                .collect())
        }
    }
    let assumptions: MaintenanceAssumptions = serde_json::from_str(
        r#"{"schema_version": "faris-maintenance-assumptions/v0.1", "governing_quantity": "heat",
            "classes": {"magnet": {"component_id": "magnets", "governing": ["magnets"], "work_s": 2.0,
                                   "threshold": {"kind": "q_star", "q_star_w_per_m3": 0.5}}},
            "cooling": {"min_s": 1.0, "max_s": 1000.0, "points": 40}, "convergence_s": 1e-6}"#,
    )
    .unwrap();
    let designs = vec![DesignInput {
        name: "ref".into(),
        history_assumptions: fixtures::assumptions(),
    }];
    let mut runner =
        |_: &str, a: &faris_model::history::OperatingHistoryAssumptions, c: &Cancellation| {
            run_operating_history_cancellable(a, &fixtures::rates_without_covariance(0.0, 'a'), c)
        };
    let result: MaintenanceResult = run_maintenance(
        &assumptions,
        &designs,
        &mut runner,
        &mut Flat,
        &Cancellation::default(),
        &mut |_, _, _| {},
    )
    .unwrap();
    let file = dir.join("result.json");
    std::fs::write(&file, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    file
}

#[test]
fn report_prints_markdown_and_json() {
    let dir = tempfile::tempdir().unwrap();
    let file = sample_result_file(dir.path());
    let md = faris(&["maintenance", "report", path(&file)]);
    assert!(md.status.success(), "{}", text(&md.stderr));
    let out = text(&md.stdout);
    assert!(out.starts_with("# Replacement downtime"));
    assert!(out.contains("| ref | EVALUATED |"));
    assert!(out.contains("## Replacements: ref"));
    assert!(out.contains("| magnets |"));
    // One design: no contrasts section.
    assert!(!out.contains("## Between designs"));
    let same = faris(&["maintenance", "report", path(&file), "--format", "markdown"]);
    assert_eq!(text(&same.stdout), out);

    let json = faris(&["maintenance", "report", path(&file), "--format", "json"]);
    assert!(json.status.success());
    let back: MaintenanceResult = serde_json::from_slice(&json.stdout).unwrap();
    let original: MaintenanceResult =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(back, original);
}

#[test]
fn report_refuses_other_files() {
    let dir = tempfile::tempdir().unwrap();
    let file = sample_result_file(dir.path());
    let text_in = std::fs::read_to_string(&file).unwrap();
    let wrong = dir.path().join("wrong.json");
    std::fs::write(
        &wrong,
        text_in.replace(
            "faris-maintenance-result/v0.1",
            "faris-maintenance-result/v9",
        ),
    )
    .unwrap();
    let out = faris(&["maintenance", "report", path(&wrong)]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("schema_version must be faris-maintenance-result/v0.1"));
    let missing = faris(&["maintenance", "report", "/nonexistent/result.json"]);
    assert_eq!(missing.status.code(), Some(2));
    let garbage = dir.path().join("g.json");
    std::fs::write(&garbage, "not json").unwrap();
    assert_eq!(
        faris(&["maintenance", "report", path(&garbage)])
            .status
            .code(),
        Some(2)
    );
}

/// `faris` in its own configuration folder, so the machine's settings cannot change the output.
fn faris_in(config: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_faris"))
        .args(arguments)
        .env("FARIS_CONFIG_DIR", config)
        .env_remove("FARIS_UNITS")
        .output()
        .expect("run faris")
}

/// The first q* number in a report's threshold table.
fn threshold_value(report: &str) -> f64 {
    let section = report.split("## Cooldown thresholds").nth(1).unwrap();
    let row = section
        .lines()
        .find(|l| l.starts_with("| magnet |"))
        .unwrap();
    row.split('|')
        .nth(2)
        .unwrap()
        .trim()
        .replace(',', ".")
        .parse()
        .unwrap()
}

// Verifies: L10N-023, UX-066, CFG-040, CFG-037
#[test]
fn report_units_are_display_only_and_the_stored_file_never_changes() {
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config");
    let file = sample_result_file(dir.path());
    let before = Sha256::digest(std::fs::read(&file).unwrap());
    let report = |extra: &[&str]| {
        let mut args = vec!["maintenance", "report", path(&file)];
        args.extend_from_slice(extra);
        let out = faris_in(&config, &args);
        assert!(out.status.success(), "{}", text(&out.stderr));
        text(&out.stdout)
    };
    let si = report(&[]);
    // The default keeps the report's habitual days, years, MWh and W/m3.
    for want in [
        "Downtime fixed (d)",
        "Start (yr)",
        "Net electricity fixed (MWh)",
        "q* (W/m3)",
    ] {
        assert!(si.contains(want), "{want} in {si}");
    }
    let fusion = report(&["--units", "fusion"]);
    assert!(fusion.contains("Net electricity fixed (GWd)"), "{fusion}");
    assert!(fusion.contains("q* (W/cm3)"), "{fusion}");
    assert!(fusion.contains("Start (yr)"), "{fusion}");
    assert_ne!(si, fusion);
    // A per-quantity override beats the system, and the hours replace the days.
    let hours = report(&["--units", "fusion", "--setting", "units.time=h"]);
    assert!(hours.contains("Downtime fixed (h)"), "{hours}");
    assert!(hours.contains("Net electricity fixed (GWd)"), "{hours}");
    // The same stored number in two units: 1 W/cm3 is 1e6 W/m3.
    let (q_si, q_fusion) = (threshold_value(&si), threshold_value(&fusion));
    assert!(
        (q_si / q_fusion / 1e6 - 1.0).abs() < 1e-3,
        "{q_si} {q_fusion}"
    );
    // The number format is display only too.
    let comma = report(&["--setting", "format.decimal=comma"]);
    assert!(comma.contains("5,0000e-1"), "{comma}");
    assert!(si.contains("5.0000e-1"), "{si}");
    // The JSON report and the file are byte for byte the same under every choice.
    let json = |extra: &[&str]| {
        let mut args = vec!["maintenance", "report", path(&file), "--format", "json"];
        args.extend_from_slice(extra);
        faris_in(&config, &args).stdout
    };
    let plain = json(&[]);
    assert_eq!(plain, json(&["--units", "fusion"]));
    assert_eq!(
        plain,
        json(&[
            "--setting",
            "format.decimal=comma",
            "--setting",
            "units.time=h"
        ])
    );
    assert_eq!(before, Sha256::digest(std::fs::read(&file).unwrap()));
}

// Verifies: L10N-021, CFG-040
#[test]
fn a_unit_of_the_wrong_quantity_in_a_setting_is_refused_with_the_allowed_ones() {
    let dir = tempfile::tempdir().unwrap();
    let out = faris_in(
        dir.path(),
        &["--setting", "units.time=kg", "config", "get", "units.time"],
    );
    assert_eq!(out.status.code(), Some(2));
    let message = text(&out.stderr);
    assert!(
        message.contains("units.time") && message.contains("kg"),
        "{message}"
    );
    assert!(
        message.contains("min") && message.contains("Next step"),
        "{message}"
    );
    let bad_system = faris_in(dir.path(), &["--units", "imperial", "config", "list"]);
    assert_eq!(bad_system.status.code(), Some(2));
    assert!(text(&bad_system.stderr).contains("units.system"));
}
