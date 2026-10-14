//! Detects child processes started by the code under test (SEC-001). Shared by
//! test binaries with `#[path]`.
//!
//! A child that has been waited for leaves its resource use in
//! `getrusage(RUSAGE_CHILDREN)`; one still running appears under
//! `/proc/self/task/*/children`. Either changing across the closure means a
//! process was started. Call it only from a test binary whose other tests start
//! no processes, and through the lock, so tests running in parallel cannot
//! confuse each other.

use nix::sys::resource::{UsageWho, getrusage};
use std::sync::Mutex;

static LOCK: Mutex<()> = Mutex::new(());

fn reaped() -> (i64, i64, i64, i64) {
    let usage = getrusage(UsageWho::RUSAGE_CHILDREN).expect("getrusage");
    // The widths of these fields differ between targets.
    #[allow(clippy::unnecessary_cast)]
    let micros = |t: nix::sys::time::TimeVal| t.tv_sec() as i64 * 1_000_000 + t.tv_usec() as i64;
    (
        usage.max_rss() as i64,
        usage.minor_page_faults() as i64,
        usage.voluntary_context_switches() as i64,
        micros(usage.user_time()) + micros(usage.system_time()),
    )
}

fn live_children() -> usize {
    let mut count = 0;
    if let Ok(tasks) = std::fs::read_dir("/proc/self/task") {
        for task in tasks.flatten() {
            if let Ok(text) = std::fs::read_to_string(task.path().join("children")) {
                count += text.split_whitespace().count();
            }
        }
    }
    count
}

/// Runs `work`; true if it started a process.
pub fn starts_a_process<T>(work: impl FnOnce() -> T) -> (bool, T) {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let before = (reaped(), live_children());
    let value = work();
    let after = (reaped(), live_children());
    (before != after, value)
}

/// Runs `work` and fails the test if it started a process.
pub fn assert_no_process<T>(what: &str, work: impl FnOnce() -> T) -> T {
    let (started, value) = starts_a_process(work);
    assert!(!started, "{what} started a child process");
    value
}
