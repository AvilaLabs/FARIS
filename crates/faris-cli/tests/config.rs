//! `faris config` end to end, each run in its own configuration folder.

use std::{path::Path, process::Command};

fn faris(config_home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_faris"))
        .args(args)
        .env("XDG_CONFIG_HOME", config_home)
        .env_remove("FARIS_MEMORY_CAP_MB")
        .env_remove("FARIS_MEMORY_MARGIN_MB")
        .env_remove("FARIS_CAD_READER")
        .output()
        .expect("run faris")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn set_then_get_shows_the_user_file_as_the_origin() {
    // Verifies: CFG-002
    let home = tempfile::tempdir().unwrap();
    let set = faris(home.path(), &["config", "set", "memory.cap_mb", "2048"]);
    assert_eq!(set.status.code(), Some(0), "{}", text(&set.stderr));
    let file = home.path().join("faris").join("settings.json");
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(written["schema"], 1);
    assert_eq!(written["values"]["memory.cap_mb"], 2048);

    let got = faris(
        home.path(),
        &["config", "get", "memory.cap_mb", "--show-origin"],
    );
    let out = text(&got.stdout);
    assert!(out.contains("2048"), "{out}");
    assert!(out.contains("user file"), "{out}");
    assert!(out.contains(file.to_str().unwrap()), "{out}");

    let path = faris(home.path(), &["config", "path"]);
    assert_eq!(text(&path.stdout).trim(), file.to_str().unwrap());
}

#[test]
fn list_names_a_layer_for_every_setting_and_a_flag_wins() {
    // Verifies: CFG-002
    // Verifies: CFG-080
    let home = tempfile::tempdir().unwrap();
    faris(home.path(), &["config", "set", "ui.theme", "dark"]);
    let listed = faris(
        home.path(),
        &["--setting", "ui.scale=2", "config", "list", "--json"],
    );
    assert_eq!(listed.status.code(), Some(0), "{}", text(&listed.stderr));
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&listed.stdout).unwrap();
    assert!(rows.len() >= 10);
    for row in &rows {
        let layer = row["origin"].as_str().unwrap();
        assert!(
            ["default", "user file", "environment", "flag"].contains(&layer),
            "{row}"
        );
        assert!(!row["origin_detail"].as_str().unwrap().is_empty());
    }
    let layer = |key: &str| {
        rows.iter().find(|r| r["key"] == key).unwrap()["origin"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(layer("ui.theme"), "user file");
    assert_eq!(layer("ui.scale"), "flag");
    assert_eq!(layer("memory.margin_mb"), "default");
}

#[test]
fn environment_wins_over_the_file_and_set_says_so() {
    // Verifies: CFG-002
    let home = tempfile::tempdir().unwrap();
    faris(home.path(), &["config", "set", "memory.cap_mb", "100"]);
    let got = Command::new(env!("CARGO_BIN_EXE_faris"))
        .args(["config", "get", "memory.cap_mb", "--show-origin"])
        .env("XDG_CONFIG_HOME", home.path())
        .env("FARIS_MEMORY_CAP_MB", "300")
        .output()
        .unwrap();
    let out = text(&got.stdout);
    assert!(
        out.contains("300") && out.contains("FARIS_MEMORY_CAP_MB"),
        "{out}"
    );
}

#[test]
fn five_bad_settings_are_reported_at_once_and_nothing_starts() {
    // Verifies: CFG-017
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("faris");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("settings.json"),
        r#"{"schema":1,"tools":{"cad_reader":"fast"},
            "values":{"memory.cap_mb":0,"ui.scale":9.5,"ui.theme":"neon","memory.margin_mb":"x"}}"#,
    )
    .unwrap();
    let started = std::time::Instant::now();
    let out = faris(home.path(), &["doctor"]);
    assert!(started.elapsed().as_secs_f64() <= 1.0, "took too long");
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty(), "doctor must not have run");
    let message = text(&out.stderr);
    assert!(message.contains("5 problems"), "{message}");
    for key in [
        "tools.cad_reader",
        "memory.cap_mb",
        "ui.scale",
        "ui.theme",
        "memory.margin_mb",
    ] {
        assert!(message.contains(key), "{key} missing:\n{message}");
    }
}

#[test]
fn unknown_keys_and_newer_files_are_refused_and_left_alone() {
    // Verifies: CFG-070
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("faris");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("settings.json");
    let newer = r#"{"schema":2,"values":{}}"#;
    std::fs::write(&file, newer).unwrap();
    let out = faris(home.path(), &["config", "set", "ui.theme", "dark"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("newer FARIS"));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), newer);

    std::fs::write(&file, r#"{"schema":1,"values":{"ui.thme":"dark"}}"#).unwrap();
    let out = faris(home.path(), &["config", "list"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("Unknown setting ui.thme"));
}

#[test]
fn set_refuses_a_bad_value_and_unset_and_reset_restore_defaults() {
    // Verifies: UX-060
    let home = tempfile::tempdir().unwrap();
    let bad = faris(home.path(), &["config", "set", "ui.scale", "100"]);
    assert_eq!(bad.status.code(), Some(2));
    assert!(!home.path().join("faris").join("settings.json").exists());

    faris(home.path(), &["config", "set", "ui.theme", "light"]);
    faris(home.path(), &["config", "unset", "ui.theme"]);
    let got = faris(home.path(), &["config", "get", "ui.theme"]);
    assert_eq!(text(&got.stdout).trim(), "system");

    faris(home.path(), &["config", "set", "ui.theme", "dark"]);
    let refused = faris(home.path(), &["config", "reset"]);
    assert_eq!(refused.status.code(), Some(2));
    let done = faris(home.path(), &["config", "reset", "--yes"]);
    assert_eq!(done.status.code(), Some(0));
    let got = faris(home.path(), &["config", "get", "ui.theme"]);
    assert_eq!(text(&got.stdout).trim(), "system");
}

#[test]
fn the_same_scenario_gives_identical_hashes_under_different_user_settings() {
    // Verifies: CFG-006
    let scenario =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios/arc-inspired/scenario.json");
    let scenario = scenario.to_str().unwrap();
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    faris(a.path(), &["config", "set", "memory.cap_mb", "512"]);
    faris(a.path(), &["config", "set", "ui.theme", "dark"]);
    faris(b.path(), &["config", "set", "memory.cap_mb", "65536"]);
    faris(b.path(), &["config", "set", "ui.scale", "2"]);
    let hash = |home: &Path| {
        let out = faris(home, &["validate", scenario]);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        text(&out.stdout)
    };
    let (first, second) = (hash(a.path()), hash(b.path()));
    assert!(first.contains("source_sha256"), "{first}");
    assert_eq!(first, second);

    let export = |home: &Path, name: &str| {
        let output = home.join(name);
        let out = faris(
            home,
            &[
                "export",
                "--scenario",
                scenario,
                "--output",
                output.to_str().unwrap(),
            ],
        );
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        std::fs::read(&output).unwrap()
    };
    assert_eq!(export(a.path(), "out.json"), export(b.path(), "out.json"));
}
