//! `run_from_files` end to end with a fake builder and a fake ACTINV. There is no cheap
//! verified transport run record, so the rate binding is injected (`run_from_files_with`);
//! everything else (designs file, assumptions file, input hashes, history, spec builder,
//! ACTINV runs, points cache, coupling loop) is the real code.

use super::*;
use crate::fixtures;
use faris_model::maintenance::{
    CoolingGrid, GoverningQuantity, MAINTENANCE_ASSUMPTIONS_VERSION, MaintenanceClass,
};
use std::os::unix::fs::PermissionsExt;
use std::sync::Mutex;

const SCENARIO: &[u8] = include_bytes!("../../../../../scenarios/arc-inspired/scenario.json");

/// Writes, for every continuation asked for, a spec, its provenance and the result the fake
/// ACTINV returns: one irradiation step and the cooling grid, heat falling as 1/(1 + t).
const BUILDER: &str = r#"
import json, os, sys
a = sys.argv[1:]
def arg(name): return a[a.index(name) + 1]
times = json.load(open(arg("--decay-continuations")))
out = arg("--output-dir")
grid = [float(x[:-1]) for x in arg("--cooling-grid").split(",")]
os.makedirs(out)
specs = []
for e in times:
    stem = "%s__inst%03d__cont%d__bare_lower_bound" % (e["component"], e["installation"], round(e["shutdown_s"]))
    spec = stem + ".spec.json"
    open(os.path.join(out, spec), "w").write(json.dumps({"fake_spec": stem}))
    json.dump({"spec_file": spec, "component": e["component"], "installation_index": e["installation"],
               "installation_interval_s": [0.0, e["shutdown_s"]], "mass_g": 1000.0, "volume_m3": 2.0,
               "continuation_of_shutdown_s": e["shutdown_s"]}, open(os.path.join(out, stem + ".provenance.json"), "w"))
    base = 1.0 if e["component"] == "magnets" else 0.5
    steps = [{"t_s": 10.0, "flux": 1.0e14, "heat_W_per_g": {"total": 99.0}}]
    for t in grid:
        steps.append({"t_s": 5000.0 + t, "flux": 0.0, "heat_W_per_g": {"total": base * e["installation"] / (1.0 + t)}})
    json.dump({"ms": 1, "pruned_states": 0, "total_states": 1, "steps": steps},
              open(os.path.join(out, stem + ".fake-result"), "w"))
    specs.append(spec)
json.dump({"specs": specs, "placeholder_spectrum_components": [],
           "validation": [{"spec": s, "ok": True} for s in specs]}, open(os.path.join(out, "manifest.json"), "w"))
"#;

struct Tree {
    dir: tempfile::TempDir,
}

impl Tree {
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn config(&self) -> RunConfig {
        RunConfig {
            designs: self.path("designs.json"),
            assumptions: self.path("assumptions.json"),
            actinv: self.path("actinv"),
            data_dir: self.path("data"),
            cache: None,
            work_dir: self.path("out.work"),
            workers: 1,
            impurities: None,
            python: "python3".into(),
            builder: self.path("builder.py"),
        }
    }
}

fn tree() -> Tree {
    let dir = tempfile::tempdir().unwrap();
    let t = Tree { dir };
    for name in ["physics.json", "spectrum.json", "run.json"] {
        std::fs::write(t.path(name), "{}").unwrap();
    }
    std::fs::write(t.path("scenario.json"), SCENARIO).unwrap();
    std::fs::write(
        t.path("history.json"),
        serde_json::to_vec(&fixtures::assumptions()).unwrap(),
    )
    .unwrap();
    let design = serde_json::json!({
        "scenario": "scenario.json", "physics": "physics.json", "history_run": "run.json",
        "spectrum_run": "spectrum.json", "history_assumptions": "history.json"
    });
    std::fs::write(
        t.path("designs.json"),
        serde_json::json!({"schema_version": DESIGNS_VERSION, "designs": {"ref": design}})
            .to_string(),
    )
    .unwrap();
    let mut classes = BTreeMap::new();
    classes.insert(
        "magnet".to_string(),
        MaintenanceClass {
            component_id: "magnets".into(),
            governing: vec!["blanket".into(), "magnets".into()],
            work_s: 2.0,
            threshold: Threshold::QStar {
                q_star_w_per_m3: 5.0,
            },
        },
    );
    let assumptions = MaintenanceAssumptions {
        schema_version: MAINTENANCE_ASSUMPTIONS_VERSION.into(),
        governing_quantity: GoverningQuantity::Heat,
        classes,
        cooling: CoolingGrid {
            min_s: 1.0,
            max_s: 1000.0,
            points: 8,
        },
        max_iterations: 10,
        convergence_s: 1e-6,
    };
    std::fs::write(
        t.path("assumptions.json"),
        serde_json::to_vec(&assumptions).unwrap(),
    )
    .unwrap();
    std::fs::write(t.path("builder.py"), BUILDER).unwrap();
    std::fs::create_dir_all(t.path("data")).unwrap();
    std::fs::write(
        t.path("actinv"),
        "#!/bin/sh\nstem=\"${2%.spec.json}\"\n/bin/cp \"$stem.fake-result\" \"$3\"\n",
    )
    .unwrap();
    std::fs::set_permissions(t.path("actinv"), std::fs::Permissions::from_mode(0o755)).unwrap();
    t
}

fn rates(_: &LoadedScenario, _: &Path) -> Result<TransportDrivingRates, String> {
    Ok(fixtures::rates_without_covariance(0.0, 'a'))
}

fn collector() -> (ProgressFn, Arc<Mutex<Vec<RunProgress>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    (Arc::new(move |p| sink.lock().unwrap().push(p)), seen)
}

#[test]
fn run_from_files_runs_the_real_wiring_end_to_end() {
    let t = tree();
    let (progress, seen) = collector();
    let result =
        run_from_files_with(&t.config(), &rates, &Cancellation::default(), progress).unwrap();
    let design = &result.designs["ref"];
    assert!(design.computed.summary.is_some(), "{:?}", design.computed);
    assert!(design.computed.converged_at_iteration.is_some());
    // Input identity: both files, the design's five files, both tools.
    let keys: Vec<&str> = result.inputs.keys().map(String::as_str).collect();
    for expected in [
        "assumptions",
        "designs",
        "actinv",
        "builder",
        "design/ref/scenario",
        "design/ref/history_assumptions",
    ] {
        assert!(keys.contains(&expected), "{expected} missing from {keys:?}");
    }
    assert_eq!(
        result.inputs["designs"],
        sha256_file(&t.path("designs.json")).unwrap()
    );
    let source = result.decay_source.as_ref().unwrap();
    assert_eq!(source.kind, "actinv-continuations");
    assert!(source.actinv_runs > 0);
    // Progress: binding, iterations, source lines, and the running counts reach the totals.
    let seen = seen.lock().unwrap();
    assert!(
        seen[0]
            .line()
            .ends_with("ref: binding transport rates from the history run")
    );
    assert!(
        seen.iter()
            .any(|p| p.iteration > 0 && p.line().contains("iteration"))
    );
    assert!(
        seen.iter()
            .any(|p| p.design.is_none() && p.message.contains("decay curves"))
    );
    assert!(seen.iter().any(|p| p.actinv_runs > 0));
    assert_eq!(seen.last().unwrap().actinv_runs, source.actinv_runs);
}

#[test]
fn a_second_run_is_served_from_the_points_cache() {
    let t = tree();
    let (progress, _) = collector();
    let first = run_from_files_with(
        &t.config(),
        &rates,
        &Cancellation::default(),
        progress.clone(),
    )
    .unwrap();
    let second =
        run_from_files_with(&t.config(), &rates, &Cancellation::default(), progress).unwrap();
    let again = second.decay_source.unwrap();
    assert_eq!(again.actinv_runs, 0);
    assert_eq!(again.cache_hits, {
        let f = first.decay_source.unwrap();
        f.actinv_runs + f.cache_hits
    });
}

#[test]
fn bad_input_and_cancellation_are_errors_with_a_reason() {
    let t = tree();
    let (progress, _) = collector();
    let mut config = t.config();
    config.actinv = t.path("no-such-actinv");
    let error = run_from_files_with(&config, &rates, &Cancellation::default(), progress.clone())
        .unwrap_err();
    assert!(error.contains("not found"), "{error}");

    let mut config = t.config();
    config.designs = t.path("missing.json");
    assert!(
        run_from_files_with(&config, &rates, &Cancellation::default(), progress.clone()).is_err()
    );

    let cancel = Cancellation::default();
    cancel.cancel();
    let error = run_from_files_with(&t.config(), &rates, &cancel, progress).unwrap_err();
    assert_eq!(error, "cancelled");
}

#[test]
fn progress_lines_match_what_the_cli_prints() {
    let at = |design: Option<&str>, iteration: u32, message: &str| RunProgress {
        design: design.map(str::to_owned),
        iteration,
        message: message.into(),
        actinv_runs: 0,
        cache_hits: 0,
    };
    assert_eq!(
        at(None, 0, "ref: 3 decay curves").line(),
        "ref: 3 decay curves"
    );
    assert_eq!(
        at(Some("ref"), 0, "fixed-duration history").line(),
        "ref: fixed-duration history"
    );
    assert_eq!(
        at(Some("ref"), 2, "running").line(),
        "ref: iteration 2: running"
    );
}
