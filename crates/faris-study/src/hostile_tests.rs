//! Opening a study whose text is hostile runs nothing (SEC-001): the view and
//! the recorded files carry shell metacharacters, `../` climbs, script text and
//! the path of an executable; reading, verifying and unpacking the study must
//! start no process and leave no marker.

#![cfg(unix)]

#[path = "../../faris-engine/tests/support/children.rs"]
mod children;

use crate::{StudyReader, tests::draft, write_study};
use std::os::unix::fs::PermissionsExt;

// Verifies: SEC-001
#[test]
fn a_study_full_of_hostile_text_opens_verifies_and_unpacks_without_a_process() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran");
    let program = dir.path().join("evil.sh");
    std::fs::write(
        &program,
        format!("#!/bin/sh\necho ran > '{}'\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let hostile = [
        format!("; touch {} #", marker.display()),
        format!("$(touch {})", marker.display()),
        format!("`{}`", program.display()),
        "../../../../../../bin/sh".to_string(),
        program.display().to_string(),
    ];

    let mut d = draft(dir.path());
    d.view.step = hostile[0].clone();
    d.view.preset = Some(hostile[1].clone());
    d.view.field_view = hostile[2].clone();
    d.view.history_tab = hostile[3].clone();
    d.view.arrangement = hostile[4].clone();
    d.view.allocation = hostile[0].clone();
    // Script text as the recorded assumptions: stored and hashed, never run.
    std::fs::write(
        dir.path().join("assumptions.json"),
        format!(
            "{{\"note\":\"#!/bin/sh\\ntouch {m}\",\"macro\":\"=cmd|' /C calc'!A0\",\"p\":\"{p}\"}}\n",
            m = marker.display(),
            p = program.display()
        ),
    )
    .unwrap();
    let target = dir.path().join("hostile.faris");
    // A hostile view may be refused by the writer; the rest only matters for what it writes.
    if write_study(&target, &d).is_err() {
        d.view = draft(dir.path()).view;
        write_study(&target, &d).expect("the study without hostile view text must save");
    }

    children::assert_no_process("opening a hostile study", || {
        let mut reader = StudyReader::open(&target).expect("opens");
        reader.verify().expect("verifies");
        let out = dir.path().join("out");
        std::fs::create_dir(&out).unwrap();
        reader.materialize(&out, None).expect("unpacks");
    });
    assert!(!marker.exists(), "a hostile field ran the sentinel");
}
