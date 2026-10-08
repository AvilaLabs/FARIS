use super::*;
use crate::fixtures;
use crate::history::run_operating_history_cancellable;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

// ------------------------------------------------------------ result reader --

const RESULT: &[u8] = include_bytes!("../../../tests/fixtures/actinv_result_synthetic.json");
const EXPECTED: &str = include_str!("../../../tests/fixtures/actinv_scan_expected.json");

#[test]
fn reader_reproduces_the_reference_scan_exactly() {
    let want: Value = serde_json::from_str(EXPECTED).unwrap();
    let got = scan_result(RESULT).unwrap();
    assert_eq!(got.sha256, want["sha256"].as_str().unwrap());
    assert_eq!(got.bytes, want["bytes"].as_u64().unwrap());
    assert_eq!(got.ms, want["kept"]["ms"]);
    assert_eq!(got.pruned_states, want["kept"]["pruned_states"]);
    assert_eq!(got.total_states, want["kept"]["total_states"]);
    let points = want["points"].as_array().unwrap();
    assert_eq!(got.steps.len(), points.len());
    assert_eq!(points.len() as u64, want["n_steps"].as_u64().unwrap());
    for (i, (p, w)) in got.steps.iter().zip(points).enumerate() {
        assert_eq!(p.t_s, w[0].as_f64().unwrap(), "t_s[{i}]");
        assert_eq!(p.heat_w_per_g, w[1].as_f64().unwrap(), "heat[{i}]");
        assert_eq!(p.flux, w[2].as_f64(), "flux[{i}]");
        assert_eq!(p.dose_proxy_gy_h, w[3].as_f64(), "dose[{i}]");
    }
    // The cases the fixture is built for are all present.
    assert!(got.steps.iter().any(|p| p.dose_proxy_gy_h.is_some()));
    assert!(got.steps.iter().any(|p| p.dose_proxy_gy_h.is_none()));
    assert!(got.steps.iter().any(|p| p.heat_w_per_g == 4987.0));
    assert!(got.steps.iter().any(|p| p.heat_w_per_g == 5e-324));
}

#[test]
fn reader_refuses_malformed_results() {
    for (label, text) in [
        (
            "truncated",
            r#"{"steps": [{"t_s": 1.0, "heat_W_per_g": {"total": 1"#,
        ),
        ("trailing text", r#"{"steps": []} x"#),
        ("not an object", "[1, 2]"),
        ("steps not a list", r#"{"steps": 3}"#),
        (
            "step without t_s",
            r#"{"steps": [{"heat_W_per_g": {"total": 1}}]}"#,
        ),
        ("step without heat", r#"{"steps": [{"t_s": 1}]}"#),
        (
            "heat without total",
            r#"{"steps": [{"t_s": 1, "heat_W_per_g": {"gamma": 1}}]}"#,
        ),
        ("empty", ""),
    ] {
        assert!(scan_result(text.as_bytes()).is_err(), "{label} should fail");
    }
    // A result with no steps is valid; the tail check refuses it later.
    assert!(scan_result(&b"{}"[..]).unwrap().steps.is_empty());
}

#[test]
fn points_file_round_trips_and_the_result_is_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let result = dir.path().join("r.result.json");
    let points = dir.path().join("cache").join("abc.points.json");
    std::fs::write(&result, RESULT).unwrap();
    write_points(&result, &points).unwrap();
    assert!(!result.exists());
    assert!(!dir.path().join("cache/abc.points.json.tmp").exists());
    let stored = read_points(&points).unwrap();
    let want: Value = serde_json::from_str(EXPECTED).unwrap();
    assert_eq!(stored.schema, POINTS_SCHEMA);
    assert_eq!(stored.result_sha256, want["sha256"].as_str().unwrap());
    assert_eq!(stored.n_steps, 10);
    assert_eq!(stored.ms, json!(1234));
    assert_eq!(stored.steps.len(), 10);
    assert_eq!(stored.steps[2], (7200.5, 4987.0, Some(0.0), Some(0.0625)));
    // Another schema is refused.
    let text = std::fs::read_to_string(&points).unwrap();
    std::fs::write(&points, text.replace(POINTS_SCHEMA, "other/v1")).unwrap();
    assert!(read_points(&points).is_err());
}

// ------------------------------------------------------------ installations --

#[test]
fn installation_in_place_follows_the_driver_rule() {
    let h = history();
    let starts = magnet_starts(&h);
    assert!(starts.len() >= 3, "{starts:?}");
    assert_eq!(installations(&h, "blanket").unwrap().len(), 1);
    assert_eq!(installations(&h, "nothing").unwrap().len(), 1);
    let inst = installations(&h, "magnets").unwrap();
    assert_eq!(inst.len(), starts.len() + 1);
    for (k, &t) in starts.iter().enumerate() {
        // At the start of its own replacement outage the installation being removed is in place.
        assert_eq!(
            installation_at(&h, "magnets", t).unwrap(),
            Some(k as u32 + 1)
        );
        // During the outage nothing is in place.
        assert_eq!(installation_at(&h, "magnets", t + 2.0).unwrap(), None);
    }
    assert_eq!(installation_at(&h, "blanket", starts[0]).unwrap(), Some(1));
}

// ----------------------------------------------------------------- the rig --

const GRID: [f64; 4] = [1.0, 10.0, 100.0, 1000.0];
const MASS_G: f64 = 1000.0;

fn history() -> HistoryResult {
    run_operating_history_cancellable(
        &fixtures::assumptions(),
        &fixtures::rates_without_covariance(0.0, 'a'),
        &Cancellation::default(),
    )
    .unwrap()
}

fn magnet_starts(h: &HistoryResult) -> Vec<f64> {
    h.events
        .iter()
        .filter(|e| {
            e.component_id.as_deref() == Some("magnets") && e.kind == EventKind::ReplacementStarted
        })
        .map(|e| e.time_s)
        .collect()
}

fn script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// W per g of the fake installation, by variant.
fn fake_heat(component: &str, k: u32, variant: &str, tau: f64) -> f64 {
    let base = if component == "magnets" { 1.0 } else { 0.5 };
    base * f64::from(k) / (1.0 + tau) * if variant == BARE_VARIANT { 1.0 } else { 2.0 }
}

struct Rig {
    dir: tempfile::TempDir,
    history: HistoryResult,
    requests: Vec<CurveRequest>,
    /// (component, installation, shutdown) behind each request.
    wanted: Vec<(String, u32, f64)>,
}

impl Rig {
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn log(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.path("logs").join(name))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

/// A fake builder template for every request, written the way `build_activation_inputs.py`
/// writes a continuations-only folder: spec, provenance and (for the fake ACTINV) the result
/// to return, for both variants, and a manifest last.
fn rig_with(actinv_extra: &str) -> Rig {
    let history = history();
    let starts = magnet_starts(&history);
    let mut requests = Vec::new();
    let mut wanted = Vec::new();
    for (k, &t) in starts.iter().enumerate() {
        requests.push(CurveRequest {
            component: "magnets".into(),
            shutdown_s: t,
        });
        wanted.push(("magnets".to_string(), k as u32 + 1, t));
        requests.push(CurveRequest {
            component: "blanket".into(),
            shutdown_s: t,
        });
        wanted.push(("blanket".to_string(), 1, t));
    }
    let dir = tempfile::tempdir().unwrap();
    let template = dir.path().join("template");
    std::fs::create_dir_all(&template).unwrap();
    std::fs::create_dir_all(dir.path().join("logs")).unwrap();
    let mut specs = Vec::new();
    for (component, k, t) in &wanted {
        for variant in [BARE_VARIANT, IMPURITIES_VARIANT] {
            let stem = format!("{component}__inst{k:03}__cont{}__{variant}", *t as i64);
            let spec = format!("{stem}.spec.json");
            std::fs::write(template.join(&spec), json!({"fake_spec": stem}).to_string()).unwrap();
            let interval = if component == "magnets" {
                let starts = &starts;
                let from = if *k == 1 {
                    0.0
                } else {
                    starts[*k as usize - 2] + 5.0
                };
                [from, *t]
            } else {
                [0.0, 100.0]
            };
            std::fs::write(
                template.join(format!("{stem}.provenance.json")),
                json!({
                    "spec_file": spec, "component": component, "installation_index": k,
                    "installation_interval_s": interval, "mass_g": MASS_G, "volume_m3": 2.0,
                    "continuation_of_shutdown_s": t, "label": variant,
                })
                .to_string(),
            )
            .unwrap();
            let mut steps = vec![json!({
                "t_s": 10.0, "flux": 1.0e14, "heat_W_per_g": {"total": 99.0},
                "nuclides": {"Fe-55": [1, 2, 3]},
            })];
            for tau in GRID {
                steps.push(json!({
                    "t_s": 5000.0 + tau, "flux": 0.0,
                    "heat_W_per_g": {"total": fake_heat(component, *k, variant, tau)},
                }));
            }
            std::fs::write(
                template.join(format!("{stem}.fake-result")),
                json!({"ms": 5, "pruned_states": 1, "total_states": 2, "steps": steps}).to_string(),
            )
            .unwrap();
            specs.push(spec);
        }
    }
    std::fs::write(
        template.join("manifest.json"),
        json!({"specs": specs, "placeholder_spectrum_components": [],
               "validation": specs.iter().map(|s| json!({"spec": s, "ok": true})).collect::<Vec<_>>()})
        .to_string(),
    )
    .unwrap();
    let logs = dir.path().join("logs");
    script(
        &dir.path().join("builder.sh"),
        &format!(
            "echo \"$@\" >> {logs}/builder.log\nout=\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = --output-dir ]; then out=\"$2\"; fi\n  shift\ndone\n/bin/cp -r {template} \"$out\"",
            logs = logs.display(),
            template = template.display()
        ),
    );
    let rig = Rig {
        dir,
        history,
        requests,
        wanted,
    };
    script(
        &rig.path("actinv"),
        &format!(
            "echo \"$ACTINV_DATA_DIR $2\" >> {logs}/actinv.log\n{actinv_extra}\nstem=\"${{2%.spec.json}}\"\n/bin/cp \"$stem.fake-result\" \"$3\"",
            logs = rig.path("logs").display()
        ),
    );
    std::fs::create_dir_all(rig.path("data")).unwrap();
    rig
}

fn rig() -> Rig {
    rig_with("")
}

fn source(
    rig: &Rig,
    work: &str,
    cache: &str,
    workers: usize,
    impurities: bool,
) -> ActinvDecaySource {
    let mut files = BTreeMap::new();
    for name in ["scenario", "physics", "history_run", "spectrum_run"] {
        std::fs::write(rig.path(name), format!("{{\"fake\": \"{name}\"}}")).unwrap();
    }
    files.insert(
        "d1".to_string(),
        DesignFiles {
            scenario: rig.path("scenario"),
            physics: rig.path("physics"),
            history_run: rig.path("history_run"),
            spectrum_run: rig.path("spectrum_run"),
        },
    );
    let mut config = ActinvSourceConfig::new(
        rig.path("builder.sh"),
        rig.path("actinv"),
        rig.path("data"),
        rig.path(cache),
        rig.path(work),
    );
    config.python = "/bin/sh".into();
    config.workers = workers;
    if impurities {
        std::fs::write(rig.path("impurities.json"), "{}").unwrap();
        config.impurities = Some(rig.path("impurities.json"));
    }
    ActinvDecaySource::new(config, files).unwrap()
}

type Curves = Vec<Result<InstallationCurve, NotEvaluated>>;

fn curves(rig: &Rig, source: &mut ActinvDecaySource) -> Result<Curves, String> {
    source.curves(
        "d1",
        &rig.history,
        &rig.requests,
        &GRID,
        &Cancellation::default(),
    )
}

fn check(rig: &Rig, got: &Curves, variant: &str) {
    assert_eq!(got.len(), rig.wanted.len());
    let starts = magnet_starts(&rig.history);
    for (curve, (component, k, _)) in got.iter().zip(&rig.wanted) {
        let curve = curve.as_ref().unwrap();
        let want: Vec<(f64, f64)> = GRID
            .iter()
            .map(|&tau| (tau, fake_heat(component, *k, variant, tau) * MASS_G))
            .collect();
        assert_eq!(curve.points, want);
        assert_eq!(curve.volume_m3, 2.0);
        let install = if component == "magnets" && *k > 1 {
            starts[*k as usize - 2] + 5.0
        } else {
            0.0
        };
        assert_eq!(curve.install_s, install);
    }
}

// ------------------------------------------------------------------ source --

#[test]
fn cache_miss_then_hit_and_curves_are_scaled_by_mass() {
    let rig = rig();
    let n = rig.wanted.len() as u64;
    let mut src = source(&rig, "work", "cache", 1, false);
    let got = curves(&rig, &mut src).unwrap();
    check(&rig, &got, BARE_VARIANT);
    assert_eq!(src.stats(), (n, 0));
    assert_eq!(rig.log("actinv.log").len() as u64, n);
    assert_eq!(rig.log("builder.log").len(), 1);
    // ACTINV gets the data dir in its environment, resolved.
    let data = std::fs::canonicalize(rig.path("data")).unwrap();
    assert!(rig.log("actinv.log")[0].starts_with(data.to_str().unwrap()));
    // Results are compacted to points files and removed; the scratch space is empty.
    let cached: Vec<_> = std::fs::read_dir(rig.path("cache")).unwrap().collect();
    assert_eq!(cached.len() as u64, n);
    assert!(
        std::fs::read_dir(rig.path("work/tmp"))
            .map(|mut d| d.next().is_none())
            .unwrap_or(true)
    );

    // Same iteration again: the folder is complete, nothing is built or run.
    let again = curves(&rig, &mut src).unwrap();
    assert_eq!(again, got);
    assert_eq!(src.stats(), (n, n));
    assert_eq!(rig.log("actinv.log").len() as u64, n);
    assert_eq!(rig.log("builder.log").len(), 1);

    // A fresh work folder with the same cache rebuilds the specs but runs nothing.
    let mut other = source(&rig, "work2", "cache", 1, false);
    assert_eq!(curves(&rig, &mut other).unwrap(), got);
    assert_eq!(other.stats(), (0, n));
    assert_eq!(rig.log("actinv.log").len() as u64, n);
    assert_eq!(rig.log("builder.log").len(), 2);
}

#[test]
fn builder_arguments_follow_the_driver() {
    let rig = rig();
    let mut src = source(&rig, "work", "cache", 1, false);
    curves(&rig, &mut src).unwrap();
    let line = &rig.log("builder.log")[0];
    for expected in [
        "--run ",
        "history_run --spectrum-run ",
        "--scenario ",
        "--physics ",
        "--history ",
        "history.json --data-dir ",
        "--cooling-grid 1.0s,10.0s,100.0s,1000.0s --actinv-outputs heat --actinv ",
        "--output-dir ",
        "decay --decay-continuations ",
        "decay-times.json --continuations-only --component blanket --component magnets",
    ] {
        assert!(line.contains(expected), "missing {expected:?} in {line}");
    }
    assert!(!line.contains("--impurities"));
    // The times file: sorted by shutdown then component, with 1-based installation indices.
    let times_path = std::fs::read_dir(rig.path("work/designs/d1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("decay-times.json");
    let times: Vec<Value> = serde_json::from_slice(&std::fs::read(times_path).unwrap()).unwrap();
    assert_eq!(times.len(), rig.wanted.len());
    let keys: Vec<(f64, String)> = times
        .iter()
        .map(|t| {
            (
                t["shutdown_s"].as_f64().unwrap(),
                t["component"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let mut sorted = keys.clone();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    assert_eq!(keys, sorted);
    assert!(times.iter().any(|t| t["installation"] == 3));
}

#[test]
fn impurities_select_the_specification_maximum_variant() {
    let rig = rig();
    let mut src = source(&rig, "work", "cache", 1, true);
    let got = curves(&rig, &mut src).unwrap();
    check(&rig, &got, IMPURITIES_VARIANT);
    assert!(rig.log("builder.log")[0].contains("--impurities "));
    // Only the chosen variant's specs were run.
    assert_eq!(rig.log("actinv.log").len(), rig.wanted.len());
    assert!(
        rig.log("actinv.log")
            .iter()
            .all(|l| l.contains(IMPURITIES_VARIANT))
    );
}

#[test]
fn an_interrupted_build_is_rebuilt() {
    let rig = rig();
    let mut src = source(&rig, "work", "cache", 1, false);
    let first = curves(&rig, &mut src).unwrap();
    let folder = std::fs::read_dir(rig.path("work/designs/d1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::remove_file(folder.join("decay/manifest.json")).unwrap();
    assert!(!complete_spec_dir(&folder.join("decay")));
    assert_eq!(curves(&rig, &mut src).unwrap(), first);
    assert_eq!(rig.log("builder.log").len(), 2);
}

#[test]
fn complete_spec_dir_needs_a_clean_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let write = |v: Value| std::fs::write(dir.path().join("manifest.json"), v.to_string()).unwrap();
    assert!(!complete_spec_dir(dir.path()));
    write(json!({"validation": [{"ok": true}], "placeholder_spectrum_components": []}));
    assert!(complete_spec_dir(dir.path()));
    write(json!({"validation": [{"ok": true}, {"ok": false}]}));
    assert!(!complete_spec_dir(dir.path()));
    write(json!({"validation": [], "placeholder_spectrum_components": ["blanket"]}));
    assert!(!complete_spec_dir(dir.path()));
    std::fs::write(dir.path().join("manifest.json"), "{").unwrap();
    assert!(!complete_spec_dir(dir.path()));
}

#[test]
fn a_tail_that_is_not_the_cooling_grid_is_a_tool_error() {
    let rig = rig();
    // The first magnet continuation ends with irradiation flux, not zero flux.
    let stem = rig.path("template").join(format!(
        "magnets__inst001__cont{}__{BARE_VARIANT}.fake-result",
        rig.wanted[0].2 as i64
    ));
    let mut doc: Value = serde_json::from_slice(&std::fs::read(&stem).unwrap()).unwrap();
    doc["steps"][4]["flux"] = json!(1.0);
    std::fs::write(&stem, doc.to_string()).unwrap();
    let mut src = source(&rig, "work", "cache", 1, false);
    let err = curves(&rig, &mut src).unwrap_err();
    assert!(err.contains("are not the cooling grid"), "{err}");

    // Too few steps fails the same way.
    let rig = rig_with("");
    let stem = rig.path("template").join(format!(
        "blanket__inst001__cont{}__{BARE_VARIANT}.fake-result",
        rig.wanted[1].2 as i64
    ));
    let mut doc: Value = serde_json::from_slice(&std::fs::read(&stem).unwrap()).unwrap();
    doc["steps"].as_array_mut().unwrap().truncate(3);
    std::fs::write(&stem, doc.to_string()).unwrap();
    let mut src = source(&rig, "work", "cache", 1, false);
    assert!(curves(&rig, &mut src).unwrap_err().contains("cooling grid"));
}

#[test]
fn a_request_with_no_installation_in_place_is_not_evaluated() {
    let mut rig = rig();
    let t = rig.wanted[0].2 + 2.0; // inside the first magnet outage
    rig.requests.push(CurveRequest {
        component: "magnets".into(),
        shutdown_s: t,
    });
    let mut src = source(&rig, "work", "cache", 1, false);
    let got = curves(&rig, &mut src).unwrap();
    let (last, rest) = got.split_last().unwrap();
    let ne = last.as_ref().unwrap_err();
    assert!(ne.reason.contains("no installation in place"), "{ne:?}");
    assert!(!ne.next_step.is_empty());
    assert!(rest.iter().all(Result::is_ok));
    // Only gaps: nothing is built or run.
    let mut rig = rig;
    rig.requests = vec![CurveRequest {
        component: "magnets".into(),
        shutdown_s: t,
    }];
    let before = rig.log("builder.log").len();
    let mut src = source(&rig, "work3", "cache3", 1, false);
    let got = curves(&rig, &mut src).unwrap();
    assert!(got[0].is_err());
    assert_eq!(rig.log("builder.log").len(), before);
    assert_eq!(src.stats(), (0, 0));
}

#[test]
fn a_failing_builder_reports_its_output() {
    let rig = rig();
    script(
        &rig.path("builder.sh"),
        "echo 'error: component x is void' >&2\nexit 2",
    );
    let mut src = source(&rig, "work", "cache", 1, false);
    let err = curves(&rig, &mut src).unwrap_err();
    assert!(err.contains("build_activation_inputs"), "{err}");
    assert!(err.contains("component x is void"), "{err}");
}

#[test]
fn a_failing_actinv_reports_its_output_and_leaves_no_points_file() {
    let rig = rig_with("echo 'actinv: bad spec' >&2\nexit 3");
    let mut src = source(&rig, "work", "cache", 1, false);
    let err = curves(&rig, &mut src).unwrap_err();
    assert!(err.contains("actinv run"), "{err}");
    assert!(err.contains("bad spec"), "{err}");
    assert!(
        std::fs::read_dir(rig.path("cache"))
            .map(|d| d.count() == 0)
            .unwrap_or(true)
    );
}

#[test]
fn cancellation_stops_a_running_actinv() {
    let rig = rig_with("exec /bin/sleep 30");
    let mut src = source(&rig, "work", "cache", 2, false);
    let cancel = Cancellation::default();
    let remote = cancel.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(700));
        remote.cancel();
    });
    let started = std::time::Instant::now();
    let err = src
        .curves("d1", &rig.history, &rig.requests, &GRID, &cancel)
        .unwrap_err();
    canceller.join().unwrap();
    assert_eq!(err, "cancelled");
    assert!(started.elapsed() < Duration::from_secs(20));
    assert!(
        std::fs::read_dir(rig.path("cache"))
            .map(|d| d.count() == 0)
            .unwrap_or(true)
    );
    // Cancelled before the call: nothing runs.
    let before = rig.log("builder.log").len();
    let cancelled = Cancellation::default();
    cancelled.cancel();
    let mut fresh = source(&rig, "work9", "cache9", 1, false);
    let err = fresh
        .curves("d1", &rig.history, &rig.requests, &GRID, &cancelled)
        .unwrap_err();
    assert_eq!(err, "cancelled");
    assert_eq!(rig.log("builder.log").len(), before);
}

#[test]
fn several_workers_give_the_same_curves_as_one() {
    let rig = rig();
    let mut one = source(&rig, "w1", "c1", 1, false);
    let a = curves(&rig, &mut one).unwrap();
    let mut three = source(&rig, "w3", "c3", 3, false);
    let b = curves(&rig, &mut three).unwrap();
    assert_eq!(a, b);
    assert_eq!(three.stats(), one.stats());
    assert_eq!(rig.log("actinv.log").len(), 2 * rig.wanted.len());
}

#[test]
fn bad_configuration_is_refused() {
    let rig = rig();
    let src = source(&rig, "work", "cache", 1, false);
    drop(src);
    let make = |edit: &dyn Fn(&mut ActinvSourceConfig)| {
        let mut config = ActinvSourceConfig::new(
            rig.path("builder.sh"),
            rig.path("actinv"),
            rig.path("data"),
            rig.path("cache"),
            rig.path("work"),
        );
        config.python = "/bin/sh".into();
        edit(&mut config);
        ActinvDecaySource::new(config, BTreeMap::new()).err()
    };
    assert!(make(&|_| {}).is_none());
    assert!(make(&|c| c.workers = 0).is_some());
    assert!(make(&|c| c.workers = 65).is_some());
    assert!(make(&|c| c.builder = rig.path("missing.py")).is_some());
    assert!(make(&|c| c.actinv = "/no/such/actinv".into()).is_some());
    assert!(make(&|c| c.data_dir = rig.path("missing")).is_some());
    assert!(make(&|c| c.python = "no-such-interpreter-xyz".into()).is_some());
    let mut src = source(&rig, "work", "cache", 1, false);
    let err = src
        .curves(
            "unknown",
            &rig.history,
            &rig.requests,
            &GRID,
            &Cancellation::default(),
        )
        .unwrap_err();
    assert!(err.contains("no ACTINV inputs for design unknown"));
}
