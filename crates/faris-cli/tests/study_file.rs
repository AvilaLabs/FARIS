//! `faris study-file` end to end: exit codes and the files it writes.

use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn faris(arguments: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_faris"))
        .args(arguments)
        .output()
        .expect("run faris")
}

fn run(arguments: &[&str]) -> Output {
    let owned: Vec<&std::ffi::OsStr> = arguments.iter().map(std::ffi::OsStr::new).collect();
    faris(&owned)
}

fn path_arg(path: &Path) -> &std::ffi::OsStr {
    path.as_os_str()
}

fn bundle_file(dir: &Path, name: &str, tag: &str) -> PathBuf {
    let mut files = std::collections::BTreeMap::new();
    for member in [
        "run.json",
        "input.json",
        "scenario.json",
        "audit.json",
        "reactor_transport.py",
        "solver/transport-artifact.json",
    ] {
        let text = if member == "scenario.json" {
            "{\"shared\":true}\n".to_owned()
        } else {
            format!("{{\"{member}\":\"{tag}\"}}\n")
        };
        files.insert(member.to_owned(), text);
    }
    let bundle = faris_engine::core_evidence::RecordedTransportBundle {
        schema_version: "faris-recorded-transport-bundle/v0.1".into(),
        files,
        notice: "test".into(),
    };
    let path = dir.join(format!("{name}.transport-bundle.json"));
    std::fs::write(&path, serde_json::to_vec_pretty(&bundle).unwrap()).unwrap();
    path
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

struct Fixture {
    dir: tempfile::TempDir,
    study: PathBuf,
    bundles: Vec<PathBuf>,
}

fn created() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let bundles = vec![
        bundle_file(dir.path(), "reference", "a"),
        bundle_file(dir.path(), "breeder-emphasis", "b"),
        bundle_file(dir.path(), "blanket-030cm", "c"),
    ];
    std::fs::write(dir.path().join("assumptions.json"), b"{\"x\":1}\n").unwrap();
    let study = dir.path().join("demo.faris");
    let output = faris(&[
        "study-file".as_ref(),
        "create".as_ref(),
        "--bundle".as_ref(),
        path_arg(&bundles[0]),
        "--bundle".as_ref(),
        path_arg(&bundles[1]),
        "--sweep-bundle".as_ref(),
        path_arg(&bundles[2]),
        "--assumptions".as_ref(),
        path_arg(&dir.path().join("assumptions.json")),
        "-o".as_ref(),
        path_arg(&study),
    ]);
    assert_eq!(code(&output), 0, "{}", text(&output));
    Fixture {
        dir,
        study,
        bundles,
    }
}

#[test]
fn create_inspect_verify_unpack_round_trip() {
    let f = created();
    let study = f.study.to_str().unwrap();
    let inspected = run(&["study-file", "inspect", study]);
    assert_eq!(code(&inspected), 0, "{}", text(&inspected));
    let summary: serde_json::Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(summary["format"], "faris-study/1");
    assert_eq!(
        summary["arrangements"]["port"]["bundles"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(summary["sweep"][0], "blanket-030cm");
    // Three bundles share scenario.json: 3 * 5 distinct members + 1 shared + assumptions.
    assert_eq!(summary["blobs"]["count"], 3 * 5 + 1 + 1);
    assert!(summary["blobs"]["original_bytes"].as_u64().unwrap() > 0);
    assert!(summary["blobs"]["stored_bytes"].as_u64().is_some());

    let verified = run(&["study-file", "verify", study]);
    assert_eq!(code(&verified), 0, "{}", text(&verified));

    let out = f.dir.path().join("unpacked");
    let unpacked = run(&["study-file", "unpack", study, out.to_str().unwrap()]);
    assert_eq!(code(&unpacked), 0, "{}", text(&unpacked));
    for (written, original) in [
        (
            out.join("port/bundles/reference.transport-bundle.json"),
            &f.bundles[0],
        ),
        (
            out.join("port/bundles/breeder-emphasis.transport-bundle.json"),
            &f.bundles[1],
        ),
        (
            out.join("sweep/blanket-030cm.transport-bundle.json"),
            &f.bundles[2],
        ),
    ] {
        let a: serde_json::Value =
            serde_json::from_slice(&std::fs::read(written).unwrap()).unwrap();
        let b: serde_json::Value =
            serde_json::from_slice(&std::fs::read(original).unwrap()).unwrap();
        assert_eq!(a, b);
    }
    assert_eq!(
        std::fs::read(out.join("operating-assumptions.json")).unwrap(),
        b"{\"x\":1}\n"
    );
    assert!(out.join("manifest.json").is_file());

    // An existing directory is refused, and nothing in it is touched.
    let again = run(&["study-file", "unpack", study, out.to_str().unwrap()]);
    assert_eq!(code(&again), 2, "{}", text(&again));
}

// Verifies: INT-004, REL-020, AUTO-004
#[test]
fn a_damaged_file_fails_verification_with_status_one() {
    let f = created();
    let mut bytes = std::fs::read(&f.study).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xff;
    let damaged = f.dir.path().join("damaged.faris");
    std::fs::write(&damaged, bytes).unwrap();
    let output = run(&["study-file", "verify", damaged.to_str().unwrap()]);
    assert_eq!(code(&output), 1, "{}", text(&output));
}

// Verifies: INT-004, AUTO-004
#[test]
fn a_file_that_is_not_a_study_is_a_verification_failure_and_a_missing_one_an_input_error() {
    let dir = tempfile::tempdir().unwrap();
    let junk = dir.path().join("junk.faris");
    std::fs::write(&junk, b"definitely not a zip file").unwrap();
    assert_eq!(
        code(&run(&["study-file", "verify", junk.to_str().unwrap()])),
        1
    );
    let missing = dir.path().join("missing.faris");
    assert_eq!(
        code(&run(&["study-file", "verify", missing.to_str().unwrap()])),
        2
    );
    assert_eq!(
        code(&run(&["study-file", "inspect", missing.to_str().unwrap()])),
        2
    );
}

#[test]
fn create_never_overwrites_and_needs_a_bundle() {
    let f = created();
    let again = faris(&[
        "study-file".as_ref(),
        "create".as_ref(),
        "--bundle".as_ref(),
        path_arg(&f.bundles[0]),
        "-o".as_ref(),
        path_arg(&f.study),
    ]);
    assert_eq!(code(&again), 2, "{}", text(&again));
    let none = run(&[
        "study-file",
        "create",
        "-o",
        f.dir.path().join("none.faris").to_str().unwrap(),
    ]);
    assert_eq!(code(&none), 2, "{}", text(&none));
    let bad = faris(&[
        "study-file".as_ref(),
        "create".as_ref(),
        "--bundle".as_ref(),
        path_arg(&f.dir.path().join("assumptions.json")),
        "-o".as_ref(),
        path_arg(&f.dir.path().join("bad.faris")),
    ]);
    assert_eq!(code(&bad), 2, "{}", text(&bad));
    assert!(!f.dir.path().join("bad.faris").exists());
}

fn sha(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// A saved-study descriptor and its two archives, laid out like the package.
fn evidence(dir: &Path) -> PathBuf {
    let archives = dir.join("port/archives");
    std::fs::create_dir_all(&archives).unwrap();
    let case = b"case archive bytes";
    let workspace = b"workspace archive bytes";
    std::fs::write(archives.join("reference-case.tar.gz"), case).unwrap();
    std::fs::write(archives.join("reference-workspace.tar.gz"), workspace).unwrap();
    let descriptor = dir.join("saved-study-port-reference.json");
    std::fs::write(
        &descriptor,
        serde_json::to_vec(&serde_json::json!({
            "schema_version": "faris-saved-study-archive/v0.1",
            "case_archive": {"path": "port/archives/reference-case.tar.gz", "sha256": sha(case), "bytes": case.len(), "manifest_path": "x"},
            "workspace_archive": {"path": "port/archives/reference-workspace.tar.gz", "sha256": sha(workspace), "bytes": workspace.len()},
            "execution_report_member": "execution-report.json",
        }))
        .unwrap(),
    )
    .unwrap();
    descriptor
}

// Verifies: PRV-040
#[test]
fn evidence_is_referenced_by_default_and_packed_on_request() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = bundle_file(dir.path(), "reference", "a");
    let descriptor = evidence(dir.path());
    let make = |name: &str, pack: bool| {
        let study = dir.path().join(name);
        let mut arguments: Vec<&std::ffi::OsStr> = vec![
            "study-file".as_ref(),
            "create".as_ref(),
            "--bundle".as_ref(),
            path_arg(&bundle),
            "--evidence".as_ref(),
            path_arg(&descriptor),
            "-o".as_ref(),
            path_arg(&study),
        ];
        if pack {
            arguments.push("--pack-evidence".as_ref());
        }
        let output = faris(&arguments);
        assert_eq!(code(&output), 0, "{}", text(&output));
        study
    };
    let referenced = make("referenced.faris", false);
    let packed = make("packed.faris", true);
    let mode = |study: &Path| -> serde_json::Value {
        let output = run(&["study-file", "inspect", study.to_str().unwrap()]);
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let r = mode(&referenced);
    assert_eq!(r["evidence"]["mode"], "referenced");
    assert_eq!(r["evidence"]["archives"].as_array().unwrap().len(), 2);
    assert_eq!(
        r["evidence"]["archives"][0]["file_name"],
        "port/archives/reference-case.tar.gz"
    );
    assert_eq!(mode(&packed)["evidence"]["mode"], "packed");
    assert!(
        std::fs::metadata(&packed).unwrap().len() > std::fs::metadata(&referenced).unwrap().len()
    );
    // Unpacking a packed study writes the archives back out.
    let out = dir.path().join("out");
    let unpacked = run(&[
        "study-file",
        "unpack",
        packed.to_str().unwrap(),
        out.to_str().unwrap(),
    ]);
    assert_eq!(code(&unpacked), 0, "{}", text(&unpacked));
    assert_eq!(
        std::fs::read(out.join("evidence/port/archives/reference-case.tar.gz")).unwrap(),
        b"case archive bytes"
    );
    // The referenced one finds the archives beside it (the descriptor's layout)
    // and reports none missing.
    let beside = run(&[
        "study-file",
        "unpack",
        referenced.to_str().unwrap(),
        dir.path().join("out2").to_str().unwrap(),
    ]);
    let report: serde_json::Value = serde_json::from_slice(&beside.stdout).unwrap();
    assert_eq!(report["evidence_available"], 2);

    // A wrong hash in the descriptor is an input error.
    std::fs::write(
        dir.path().join("port/archives/reference-case.tar.gz"),
        b"tampered",
    )
    .unwrap();
    let output = faris(&[
        "study-file".as_ref(),
        "create".as_ref(),
        "--bundle".as_ref(),
        path_arg(&bundle),
        "--evidence".as_ref(),
        path_arg(&descriptor),
        "-o".as_ref(),
        path_arg(&dir.path().join("third.faris")),
    ]);
    assert_eq!(code(&output), 2, "{}", text(&output));
}

#[test]
fn a_view_file_is_recorded_and_a_bad_one_refused() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = bundle_file(dir.path(), "reference", "a");
    let view = dir.path().join("view.json");
    std::fs::write(
        &view,
        br#"{"step":"compare","year":12.5,"arrangement":"control"}"#,
    )
    .unwrap();
    let study = dir.path().join("v.faris");
    let make = |view: &Path, study: &Path| {
        faris(&[
            "study-file".as_ref(),
            "create".as_ref(),
            "--bundle".as_ref(),
            path_arg(&bundle),
            "--view".as_ref(),
            path_arg(view),
            "-o".as_ref(),
            path_arg(study),
        ])
    };
    let output = make(&view, &study);
    assert_eq!(code(&output), 0, "{}", text(&output));
    let inspected = run(&["study-file", "inspect", study.to_str().unwrap()]);
    let summary: serde_json::Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(summary["view"]["step"], "compare");
    assert_eq!(summary["view"]["year"], 12.5);
    assert_eq!(summary["view"]["arrangement"], "control");
    // Fields left out keep their defaults.
    assert_eq!(summary["view"]["field_view"], "materials");
    std::fs::write(&view, b"[1,2]").unwrap();
    assert_eq!(code(&make(&view, &dir.path().join("w.faris"))), 2);
}

#[test]
fn export_refuses_recorded_files_that_are_not_real_transport_and_writes_nothing() {
    // The fixture's bundles carry placeholder members, not a transport run, so
    // the desktop's validation (shared by the export) must refuse them.
    let f = created();
    let out = f.dir.path().join("exports");
    std::fs::create_dir(&out).unwrap();
    let output = run(&[
        "study-file",
        "export",
        f.study.to_str().unwrap(),
        "--output",
        out.to_str().unwrap(),
    ]);
    assert_ne!(code(&output), 0, "{}", text(&output));
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0);
}

// Verifies: INT-004
#[test]
fn export_of_a_damaged_study_file_exits_one() {
    let dir = tempfile::tempdir().unwrap();
    let study = dir.path().join("bad.faris");
    std::fs::write(&study, b"PK not really").unwrap();
    let output = run(&[
        "study-file",
        "export",
        study.to_str().unwrap(),
        "--output",
        dir.path().to_str().unwrap(),
    ]);
    assert_eq!(code(&output), 1, "{}", text(&output));
}
