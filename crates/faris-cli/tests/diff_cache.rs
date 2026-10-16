//! `faris diff`, `faris config diff`, `faris geometry summary` and `faris cache`, end to end.

use faris_engine::result_cache::{ResultCache, cache_key};
use serde_json::{Value, json};
use std::{path::Path, process::Command};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../faris-study/tests/fixtures");
const SCENARIO: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scenarios/arc-inspired/cold-reference.scenario.json"
);
const PHYSICS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scenarios/arc-inspired/cold-reference.reference.physics.json"
);

fn faris(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_faris"))
        .args(args)
        .output()
        .expect("run faris")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn code(output: &std::process::Output) -> i32 {
    output.status.code().expect("exited normally")
}

fn fixture(name: &str) -> String {
    format!("{FIXTURES}/{name}")
}

// Verifies: COL-023
#[test]
fn diff_json_is_versioned_and_the_exit_code_says_whether_the_studies_differ() {
    let a = fixture("24-everything-but-store-evidence.faris");
    let b = fixture("22-history-ensembles.faris");
    let same = faris(&["diff", &a, &a, "--json"]);
    assert_eq!(code(&same), 0, "{}", text(&same.stderr));
    let report: Value = serde_json::from_slice(&same.stdout).unwrap();
    assert_eq!(report["schema"], "faris-diff/v1");
    assert_eq!(report["identical"], true);

    let different = faris(&["diff", &a, &b, "--json"]);
    assert_eq!(code(&different), 6, "{}", text(&different.stderr));
    let report: Value = serde_json::from_slice(&different.stdout).unwrap();
    assert_eq!(report["schema"], "faris-diff/v1");
    assert_eq!(report["identical"], false);
    for key in [
        "a",
        "b",
        "compared",
        "summary",
        "inputs",
        "evidence",
        "results",
        "presentation",
        "components",
        "impact",
    ] {
        assert!(report.get(key).is_some(), "{key} missing");
    }
    // The text form says the same thing.
    let readable = faris(&["diff", &a, &b]);
    assert_eq!(code(&readable), 6);
    let out = text(&readable.stdout);
    for heading in [
        "INPUTS CHANGED",
        "EVIDENCE CHANGED",
        "RESULTS CHANGED",
        "PRESENTATION ONLY",
    ] {
        assert!(out.contains(heading), "{heading} missing:\n{out}");
    }
    assert!(text(&readable.stderr).contains("the two sides differ"));
    // Two runs give the same bytes (stable output).
    let again = faris(&["diff", &a, &b, "--json"]);
    assert_eq!(again.stdout, different.stdout);
}

// Verifies: COL-023, GEO-067
#[test]
fn diff_compares_two_model_files_and_refuses_a_mixed_pair() {
    let dir = tempfile::tempdir().unwrap();
    let mut scenario: Value = serde_json::from_slice(&std::fs::read(SCENARIO).unwrap()).unwrap();
    let a = dir.path().join("a.json");
    std::fs::write(&a, serde_json::to_vec(&scenario).unwrap()).unwrap();
    scenario["variants"][0]["layers"][1]["thickness_m"] = json!(0.6);
    let b = dir.path().join("b.json");
    std::fs::write(&b, serde_json::to_vec_pretty(&scenario).unwrap()).unwrap();
    let out = faris(&["diff", a.to_str().unwrap(), b.to_str().unwrap(), "--json"]);
    assert_eq!(code(&out), 6);
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        report["inputs"][0]["path"],
        "variants[id=reference].layers[id=blanket].thickness_m"
    );
    assert_eq!(report["inputs"][0]["unit"], "m");
    assert_eq!(report["inputs"][0]["cost"], "transport_run");
    assert!(!report["components"].as_array().unwrap().is_empty());
    // Pretty-printed and compact copies of the same file are not a difference.
    let compact = dir.path().join("c.json");
    std::fs::write(&compact, serde_json::to_vec(&scenario).unwrap()).unwrap();
    assert_eq!(
        code(&faris(&[
            "diff",
            b.to_str().unwrap(),
            compact.to_str().unwrap()
        ])),
        0
    );
    let mixed = faris(&[
        "diff",
        a.to_str().unwrap(),
        &fixture("22-history-ensembles.faris"),
    ]);
    assert_eq!(code(&mixed), 2);
    assert!(text(&mixed.stderr).contains("Next step"));
}

// Verifies: GEO-069
#[test]
fn geometry_summary_writes_the_csv_table() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("cells.csv");
    let out = faris(&[
        "geometry",
        "summary",
        "--scenario",
        SCENARIO,
        "--physics",
        PHYSICS,
        "--output",
        output.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    let csv = std::fs::read_to_string(&output).unwrap();
    assert!(csv.starts_with("cell_name,cell_id,variant,material_id,volume_m3,density_kg_m3,mass_kg,representation,kind_label,note"));
    assert_eq!(csv.lines().count(), 8);
    assert!(csv.contains("Breeding blanket,blanket,reference,flibe-cold-solid"));
    // An existing file is never overwritten.
    let again = faris(&[
        "geometry",
        "summary",
        "--scenario",
        SCENARIO,
        "--output",
        output.to_str().unwrap(),
    ]);
    assert_eq!(code(&again), 2);
}

// Verifies: CFG-081, CFG-082
#[test]
fn config_diff_lists_keys_with_unit_layer_and_effect_and_exits_6_when_different() {
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, body: Value| {
        let path = dir.path().join(name);
        std::fs::write(&path, serde_json::to_vec_pretty(&body).unwrap()).unwrap();
        path
    };
    let a = write(
        "a.json",
        json!({"schema": 1, "tools": {}, "values": {"memory.cap_mb": 4096, "ui.scale": 1}}),
    );
    let b = write(
        "b.json",
        json!({"schema": 1, "tools": {"cross_sections": "/data/xs.xml"}, "values": {"ui.scale": 1.5, "memory.cap_mb": 4096}}),
    );
    let resaved = write(
        "c.json",
        json!({"values": {"ui.scale": 1.0, "memory.cap_mb": 4096}, "tools": {}, "schema": 1}),
    );
    let (a, b, resaved) = (
        a.to_str().unwrap(),
        b.to_str().unwrap(),
        resaved.to_str().unwrap(),
    );
    assert_eq!(code(&faris(&["config", "diff", a, resaved])), 0);
    let json_out = faris(&["config", "diff", a, b, "--json"]);
    assert_eq!(code(&json_out), 6, "{}", text(&json_out.stderr));
    let report: Value = serde_json::from_slice(&json_out.stdout).unwrap();
    assert_eq!(report["schema"], "faris-config-diff/v1");
    assert_eq!(report["result_changing"], 1);
    let keys: Vec<_> = report["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["tools.cross_sections", "ui.scale"]);
    assert_eq!(report["changes"][1]["effect"], "appearance");
    assert_eq!(report["changes"][1]["old"], "1");
    let readable = text(&faris(&["config", "diff", a, b]).stdout);
    assert!(
        readable.contains("2 differences, 1 result-changing"),
        "{readable}"
    );
}

fn populate(root: &Path) -> Vec<String> {
    let cache = ResultCache::open(root);
    let mut names = Vec::new();
    for i in 0..3 {
        let key = cache_key("test-kind", [("input", json!({"n": i, "radius_m": 3.3}))]);
        cache
            .put_bytes(
                &key,
                "result.json",
                format!("{{\"n\":{i}}}").as_bytes(),
                4.0,
            )
            .unwrap();
        names.push(key.dir_name());
    }
    names
}

// Verifies: AUTO-032, AUTO-035, AUTO-036, AUTO-038
#[test]
fn cache_list_explain_verify_stats_gc_pin_and_clear() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("faris-transport-cache");
    let names = populate(&root);
    let dir_arg = root.to_str().unwrap();
    let cache = |extra: &[&str]| {
        let mut args = vec!["cache", "--dir", dir_arg];
        args.extend_from_slice(extra);
        faris(&args)
    };

    let list = cache(&["list", "--json"]);
    assert_eq!(code(&list), 0, "{}", text(&list.stderr));
    let entries: Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(entries.as_array().unwrap().len(), 3);
    let key = entries[0]["key"].as_str().unwrap().to_string();

    let explain = text(&cache(&["explain", &key[..10]]).stdout);
    assert!(explain.contains("radius_m"), "{explain}");
    assert!(explain.contains("faris_version"), "{explain}");
    assert!(explain.contains("result.json"), "{explain}");

    assert_eq!(code(&cache(&["verify"])), 0);
    // Damage one entry: verify exits 1 and names it.
    let victim = root.join(&names[1]).join("result.json");
    std::fs::write(&victim, b"{\"n\":99}").unwrap();
    let verify = cache(&["verify"]);
    assert_eq!(code(&verify), 1);
    assert!(
        text(&verify.stdout).contains("damaged"),
        "{}",
        text(&verify.stdout)
    );
    assert!(text(&verify.stderr).contains("Next step"));

    // Counters: a lookup that finds the damaged entry is counted as refused, not as a hit.
    let stats: Value = serde_json::from_slice(&cache(&["stats", "--json"]).stdout).unwrap();
    assert_eq!(stats["false_hits"], 0);
    assert!(stats["hit_rate"].is_null() || stats["hit_rate"].is_number());

    // gc keeps pinned entries.
    let pinned = names[0].rsplit('-').next().unwrap()[..8].to_string();
    assert_eq!(code(&cache(&["pin", &pinned])), 0);
    let gc = cache(&["gc", "--max-size", "0"]);
    assert_eq!(code(&gc), 0, "{}", text(&gc.stderr));
    assert!(root.join(&names[0]).is_dir(), "the pinned entry stays");
    assert!(!root.join(&names[2]).exists());
    // clear needs a confirmation.
    assert_eq!(code(&cache(&["clear"])), 2);
    assert_eq!(code(&cache(&["clear", "--yes", "--include-pinned"])), 0);
    assert!(!root.join(&names[0]).exists());
    // A folder that is not there is an error with a next step.
    let missing = faris(&[
        "cache",
        "--dir",
        dir.path().join("none").to_str().unwrap(),
        "list",
    ]);
    assert_eq!(code(&missing), 2);
    assert!(text(&missing.stderr).contains("Next step"));
}
