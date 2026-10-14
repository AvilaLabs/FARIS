//! Kill-during-save trials of the real study writer (REL-010). The test binary
//! starts itself as the child with `FARIS_KILL_CHILD` set to a work directory;
//! the child saves year 2 over a saved year 1, the parent kills it with SIGKILL
//! at a random point and opens whatever is on disk.

use crate::{StudyReader, tests::draft, write_study};
use faris_engine::atomic_file::campaign::{
    Config, Found, Tally, announce_go, run, trials_from_env,
};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

const CHILD_ENV: &str = "FARIS_KILL_CHILD";

/// Bundles plus a few hundred KiB of assumptions, so the save takes
/// long enough to be killed part-way through.
fn versioned_draft(dir: &Path, year: f64) -> crate::StudyDraft {
    let mut d = draft(dir);
    let mut pad = String::from("{\"pad\":\"");
    let mut state = 0x2545_F491_4F6C_DD1D_u64;
    while pad.len() < (384 << 10) {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        pad.push_str(&format!("{state:016x}"));
    }
    pad.push_str("\"}\n");
    std::fs::write(dir.join("assumptions.json"), pad).unwrap();
    d.view.year = year;
    d
}

/// The child process. Does nothing in a normal test run.
#[test]
fn kill_child_entry() {
    let Ok(work) = std::env::var(CHILD_ENV) else {
        return;
    };
    let work = PathBuf::from(work);
    let d = versioned_draft(&work, 2.0);
    announce_go();
    write_study(&work.join("saved.faris"), &d).unwrap();
}

fn classify(path: &Path) -> Result<Found, String> {
    let mut reader = StudyReader::open(path).map_err(|e| format!("cannot open: {e}"))?;
    reader
        .verify()
        .map_err(|e| format!("blob verification failed: {e}"))?;
    let year = reader.manifest.view.year;
    if year == 1.0 {
        Ok(Found::Previous)
    } else if year == 2.0 {
        Ok(Found::New)
    } else {
        Err(format!("unexpected view year {year}"))
    }
}

fn campaign(trials: usize) -> Tally {
    // FARIS_KILL_DIR picks the filesystem under test; /tmp is often memory-backed.
    let dir = match std::env::var_os("FARIS_KILL_DIR") {
        Some(base) => tempfile::tempdir_in(base).unwrap(),
        None => tempfile::tempdir().unwrap(),
    };
    let work = dir.path().to_path_buf();
    let target = work.join("saved.faris");
    let draft_two = versioned_draft(&work, 2.0);
    let started = std::time::Instant::now();
    write_study(&target, &draft_two).unwrap();
    let save_time = started.elapsed();
    let exe = std::env::current_exe().unwrap();
    run(
        Config {
            trials,
            seed: 20_261_014,
            max_delay: save_time * 3 / 2,
        },
        &work,
        &|_| {
            write_study(&target, &versioned_draft(&work, 1.0)).unwrap();
        },
        &|| {
            let mut command = Command::new(&exe);
            command
                .args(["--exact", "kill_tests::kill_child_entry"])
                .args(["--nocapture", "--test-threads=1"])
                .env(CHILD_ENV, &work);
            command
        },
        &|| classify(&target),
    )
}

fn check(tally: &Tally, trials: usize) {
    eprintln!(
        "study kill campaign: {} trials, previous {}, new {}, damaged {}, not run {}, leftover temporaries {}",
        tally.trials,
        tally.previous,
        tally.new,
        tally.damaged.len(),
        tally.not_run.len(),
        tally.leftover_temporaries
    );
    assert!(
        tally.not_run.is_empty(),
        "trials not run: {:?}",
        tally.not_run
    );
    assert_eq!(tally.trials, trials);
    let shown = &tally.damaged[..tally.damaged.len().min(5)];
    assert!(tally.damaged.is_empty(), "{shown:?}");
    assert_eq!(tally.previous + tally.new, trials);
}

// Verifies: REL-010
#[cfg(unix)]
#[test]
fn reduced_study_kill_during_save_campaign_finds_no_damaged_file() {
    if std::env::var(CHILD_ENV).is_ok() {
        return;
    }
    let trials = 200;
    check(&campaign(trials), trials);
}

/// The full campaign: `FARIS_KILL_TRIALS` trials (default 10 000).
// Verifies: REL-010
#[cfg(unix)]
#[test]
#[ignore = "full kill-during-save campaign; run with --ignored"]
fn full_study_kill_during_save_campaign_finds_no_damaged_file() {
    let trials = trials_from_env(10_000);
    check(&campaign(trials), trials);
}
