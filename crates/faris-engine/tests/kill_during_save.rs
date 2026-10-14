//! Kill-during-save trials of the shared atomic writer (REL-010). The test
//! binary starts itself as the child with `FARIS_KILL_CHILD` set.

use faris_engine::atomic_file::{
    campaign::{Config, Found, Tally, announce_go, run, trials_from_env},
    write_atomic,
};
use std::{path::Path, process::Command};

const CHILD_ENV: &str = "FARIS_KILL_CHILD";
const PAYLOAD_BYTES: usize = 1 << 20;

/// FNV-1a over 8-byte words: cheap even in a debug build, and any partial or
/// mixed file fails it.
fn checksum(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for chunk in bytes.chunks(8) {
        let mut word = [0u8; 8];
        word[..chunk.len()].copy_from_slice(chunk);
        hash = (hash ^ u64::from_le_bytes(word)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash ^ bytes.len() as u64
}

/// A file that checks itself: a header with the version, filler, then a
/// checksum of everything before the last line.
fn versioned(version: u8) -> Vec<u8> {
    let mut body = format!("version {version}\n").into_bytes();
    let mut state = u64::from(version) + 1;
    while body.len() < PAYLOAD_BYTES {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        body.extend_from_slice(&state.to_le_bytes());
    }
    let sum = checksum(&body);
    body.extend_from_slice(format!("sum {sum:016x}\n").as_bytes());
    body
}

fn classify(path: &Path) -> Result<Found, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("unreadable: {e}"))?;
    let cut = bytes
        .windows(4)
        .rposition(|w| w == b"sum ")
        .ok_or("no checksum line (partial file)")?;
    let (body, tail) = bytes.split_at(cut);
    let expected = format!("sum {:016x}\n", checksum(body));
    if tail != expected.as_bytes() {
        return Err("checksum does not match (damaged file)".into());
    }
    match body.get(..10) {
        Some(b"version 1\n") => Ok(Found::Previous),
        Some(b"version 2\n") => Ok(Found::New),
        _ => Err("unknown version header".into()),
    }
}

/// The child process: saves version 2 over version 1. Does nothing in a
/// normal test run.
#[test]
fn kill_child_entry() {
    let Ok(target) = std::env::var(CHILD_ENV) else {
        return;
    };
    let bytes = versioned(2);
    announce_go();
    write_atomic(Path::new(&target), &bytes).unwrap();
}

fn campaign(trials: usize) -> Tally {
    // FARIS_KILL_DIR picks the filesystem under test; /tmp is often memory-backed.
    let dir = match std::env::var_os("FARIS_KILL_DIR") {
        Some(base) => tempfile::tempdir_in(base).unwrap(),
        None => tempfile::tempdir().unwrap(),
    };
    let target = dir.path().join("saved.dat");
    let started = std::time::Instant::now();
    write_atomic(&target, &versioned(2)).unwrap();
    let save_time = started.elapsed();
    let exe = std::env::current_exe().unwrap();
    run(
        Config {
            trials,
            seed: 20_261_013,
            max_delay: save_time * 3 / 2,
        },
        dir.path(),
        &|_| write_atomic(&target, &versioned(1)).unwrap(),
        &|| {
            let mut command = Command::new(&exe);
            command
                .args(["--exact", "kill_child_entry"])
                .args(["--nocapture", "--test-threads=1"])
                .env(CHILD_ENV, &target);
            command
        },
        &|| classify(&target),
    )
}

fn check(tally: &Tally, trials: usize) {
    eprintln!(
        "kill campaign: {} trials, previous {}, new {}, damaged {}, not run {}, leftover temporaries {}",
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
fn reduced_kill_during_save_campaign_finds_no_damaged_file() {
    if std::env::var(CHILD_ENV).is_ok() {
        return;
    }
    let trials = 300;
    check(&campaign(trials), trials);
}

/// The full campaign: `FARIS_KILL_TRIALS` trials (default 10 000).
// Verifies: REL-010
#[cfg(unix)]
#[test]
#[ignore = "full kill-during-save campaign; run with --ignored"]
fn full_kill_during_save_campaign_finds_no_damaged_file() {
    let trials = trials_from_env(10_000);
    check(&campaign(trials), trials);
}
