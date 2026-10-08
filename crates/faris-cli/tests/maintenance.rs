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
    assert!(help.contains("scripts/build_activation_inputs.py"));
}

#[test]
fn argument_errors_exit_two() {
    // Missing required options and a bad worker count are usage errors.
    assert_eq!(faris(&["maintenance", "run"]).status.code(), Some(2));
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("r.json");
    let args = run_args("d.json", "a.json", path(&out), &["--workers", "0"]);
    let result = faris(&args);
    assert_eq!(result.status.code(), Some(2));
    assert!(text(&result.stderr).contains("workers"));
    let args = run_args("d.json", "a.json", path(&out), &["--workers", "65"]);
    assert_eq!(faris(&args).status.code(), Some(2));
    assert_eq!(
        faris(&["maintenance", "report", "--format", "yaml", "x"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(faris(&["maintenance", "frobnicate"]).status.code(), Some(2));
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
