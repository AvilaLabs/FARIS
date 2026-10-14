//! Kill-during-save campaign driver (REL-010). A trial starts a child process
//! that saves a new version over a known previous version, kills it with
//! SIGKILL after a random delay, and asks the caller to classify what is on
//! disk. Only the kill-campaign tests use it.
//!
//! SIGKILL stops the process at an arbitrary instruction but the operating
//! system still holds its completed writes, so this exercises the writer's
//! ordering against process death. It does not drop data from the page cache;
//! see the power-loss model in the parent module for what is checked there.

use std::{
    io::{BufRead, BufReader},
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

/// The line a child prints on stdout immediately before it starts saving.
pub const GO_MARKER: &str = "FARIS_KILL_GO";

/// What a trial found on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    Previous,
    New,
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub trials: usize,
    pub seed: u64,
    /// Kill delays are uniform in `[0, max_delay]`, measured from the child's
    /// go marker reaching the parent.
    pub max_delay: Duration,
}

#[derive(Debug, Default)]
pub struct Tally {
    pub trials: usize,
    pub previous: usize,
    pub new: usize,
    /// One message per trial that found a damaged or missing file.
    pub damaged: Vec<String>,
    /// Trials that could not be run (the child would not start, or never reached the save).
    /// These say nothing about the file, so they are not counted as damage.
    pub not_run: Vec<String>,
    /// Temporary files left behind by killed saves (the destination is intact).
    pub leftover_temporaries: usize,
}

/// Number of trials from `FARIS_KILL_TRIALS`, or `default`.
pub fn trials_from_env(default: usize) -> usize {
    std::env::var("FARIS_KILL_TRIALS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Called by the child just before it saves.
pub fn announce_go() {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{GO_MARKER}");
    let _ = out.flush();
}

fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Run the campaign. `directory` is where the saved file and its temporary
/// files live. `prepare(trial)` must leave the previous version in place;
/// `child()` builds the command that saves the new one; `classify()` decides
/// whether the file on disk is the previous version, the new one, or damaged.
pub fn run(
    config: Config,
    directory: &Path,
    prepare: &dyn Fn(usize),
    child: &dyn Fn() -> Command,
    classify: &dyn Fn() -> Result<Found, String>,
) -> Tally {
    let mut tally = Tally::default();
    let mut rng = config.seed;
    let max_micros = config.max_delay.as_micros().max(1) as u64;
    for trial in 0..config.trials {
        remove_temporaries(directory);
        prepare(trial);
        // One trial in ten kills at the first instant, to cover the start.
        let delay = if trial % 10 == 0 {
            Duration::ZERO
        } else {
            Duration::from_micros(splitmix(&mut rng) % (max_micros + 1))
        };
        let mut command = child();
        command.stdout(Stdio::piped()).stderr(Stdio::null());
        tally.trials += 1;
        let mut process = match command.spawn() {
            Ok(p) => p,
            Err(e) => {
                tally
                    .not_run
                    .push(format!("trial {trial}: cannot start child: {e}"));
                continue;
            }
        };
        let mut reader = BufReader::new(process.stdout.take().expect("piped stdout"));
        let mut reached = false;
        let mut line = String::new();
        while let Ok(count) = reader.read_line(&mut line) {
            if count == 0 {
                break;
            }
            // libtest prints "test name ... " on the same line before the child writes.
            if line.contains(GO_MARKER) {
                reached = true;
                break;
            }
            line.clear();
        }
        if reached {
            // Spin-wait: thread::sleep is too coarse for microsecond delays.
            let start = std::time::Instant::now();
            while start.elapsed() < delay {
                std::hint::spin_loop();
            }
        }
        let _ = process.kill();
        let _ = process.wait();
        if !reached {
            tally
                .not_run
                .push(format!("trial {trial}: child never reached the save"));
            continue;
        }
        tally.leftover_temporaries += count_temporaries(directory);
        match classify() {
            Ok(Found::Previous) => tally.previous += 1,
            Ok(Found::New) => tally.new += 1,
            Err(why) => tally.damaged.push(format!("trial {trial}: {why}")),
        }
    }
    tally
}

fn is_temporary(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with(".faris-save-")
}

fn count_temporaries(directory: &Path) -> usize {
    std::fs::read_dir(directory)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| is_temporary(&e.file_name()))
                .count()
        })
        .unwrap_or(0)
}

fn remove_temporaries(directory: &Path) {
    if let Ok(entries) = std::fs::read_dir(directory) {
        for entry in entries.flatten() {
            if is_temporary(&entry.file_name()) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}
