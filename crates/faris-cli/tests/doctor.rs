//! `faris doctor` end to end: what it reports and what it says about tools.

use std::process::Command;

#[test]
fn the_note_names_the_commands_that_run_tools_and_no_longer_says_not_implemented() {
    let output = Command::new(env!("CARGO_BIN_EXE_faris"))
        .arg("doctor")
        .output()
        .expect("run faris");
    assert_eq!(output.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let note = report["note"].as_str().unwrap();
    assert!(!note.contains("not implemented"), "{note}");
    for command in [
        "faris reactor run",
        "faris design check",
        "faris evidence run",
    ] {
        assert!(note.contains(command), "{note}");
    }
}
