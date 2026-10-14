//! Memory is budgeted, not discovered (PERF-041): a hard cap FARIS applies to
//! every external job, a check before the job starts that refuses one that
//! would not fit in the memory that is free, and a measured peak afterwards.
//!
//! The cap is an address-space limit (`RLIMIT_AS`) set on the job by the job
//! runner (on macOS and Windows the runner instead compares sampled memory to the same figure;
//! see `jobs`). Address space is never smaller than resident memory, so a job that
//! stays under the cap also stays under it in resident terms; the converse
//! does not hold, which is why a job can be stopped by the cap while its
//! resident size is still below it.
//!
//! FARIS has no model of how much memory a given job needs, so the check uses
//! the cap as the projection: it asks whether the job could grow to its cap in
//! the memory that is free. That refuses some jobs that would have fitted; the
//! message says how to lower the cap when the job is known to be small.

use std::sync::Mutex;

pub const MIB: u64 = 1024 * 1024;
pub const GIB: u64 = 1024 * MIB;

/// Memory kept free for the rest of the system when the check is made.
pub const DEFAULT_MARGIN_BYTES: u64 = GIB;

/// The cap and the margin. `cap_bytes` lowers the address space of every
/// external job; it never raises one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryPolicy {
    pub cap_bytes: Option<u64>,
    pub margin_bytes: u64,
}

impl Default for MemoryPolicy {
    fn default() -> Self {
        Self {
            cap_bytes: None,
            margin_bytes: DEFAULT_MARGIN_BYTES,
        }
    }
}

impl MemoryPolicy {
    /// From `FARIS_MEMORY_CAP_MB` and `FARIS_MEMORY_MARGIN_MB`; an unset or
    /// unreadable variable leaves the default.
    pub fn from_environment() -> Self {
        let megabytes = |name: &str| {
            std::env::var(name)
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok())
                .filter(|v| *v > 0)
                .map(|v| v.saturating_mul(MIB))
        };
        Self {
            cap_bytes: megabytes("FARIS_MEMORY_CAP_MB"),
            margin_bytes: megabytes("FARIS_MEMORY_MARGIN_MB").unwrap_or(DEFAULT_MARGIN_BYTES),
        }
    }
}

static CONFIGURED: Mutex<Option<MemoryPolicy>> = Mutex::new(None);

/// Set the policy for this process (a front end's setting). Without a call the
/// environment variables are read.
pub fn set_policy(policy: MemoryPolicy) {
    *CONFIGURED.lock().unwrap_or_else(|e| e.into_inner()) = Some(policy);
}

/// The policy in force: the one set by [`set_policy`], else the environment's.
pub fn policy() -> MemoryPolicy {
    CONFIGURED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .unwrap_or_else(MemoryPolicy::from_environment)
}

/// The address space a job is held to: what it asked for, lowered to the cap.
pub fn effective_address_space(requested: u64, policy: &MemoryPolicy) -> u64 {
    policy.cap_bytes.map_or(requested, |cap| requested.min(cap))
}

/// The outcome of the check before a job starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admission {
    Admitted {
        projected_bytes: u64,
        free_bytes: u64,
        margin_bytes: u64,
    },
    /// The job is not started; the text says why and what to do.
    Refused(String),
    /// Free memory could not be read here, so no decision was made.
    NotEvaluated(String),
}

fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / GIB as f64)
}

/// Decide from numbers alone. `free_bytes` is `Err(reason)` where the
/// platform gives no figure.
pub fn decide(
    projected_bytes: u64,
    free_bytes: Result<u64, String>,
    margin_bytes: u64,
) -> Admission {
    let free = match free_bytes {
        Ok(free) => free,
        Err(reason) => return Admission::NotEvaluated(reason),
    };
    if projected_bytes.saturating_add(margin_bytes) <= free {
        return Admission::Admitted {
            projected_bytes,
            free_bytes: free,
            margin_bytes,
        };
    }
    Admission::Refused(format!(
        "not started: this job may use up to {} (its memory cap; FARIS has no estimate for it), \
         but only {} of memory is free and {} is kept in reserve. Close other programs and run it \
         again, or, if the job is known to be small, lower the cap with FARIS_MEMORY_CAP_MB.",
        gib(projected_bytes),
        gib(free),
        gib(margin_bytes)
    ))
}

impl Admission {
    /// The line recorded with a finished job.
    pub fn record(&self) -> String {
        match self {
            Self::Admitted {
                projected_bytes,
                free_bytes,
                margin_bytes,
            } => format!(
                "PASS: projected {} (the cap), free {}, reserve {}",
                gib(*projected_bytes),
                gib(*free_bytes),
                gib(*margin_bytes)
            ),
            Self::Refused(why) => format!("REFUSED: {why}"),
            Self::NotEvaluated(why) => format!("NOT_EVALUATED: {why}"),
        }
    }
}

/// `MemAvailable` from the text of `/proc/meminfo`, in bytes.
pub fn parse_meminfo_available(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    kib.checked_mul(1024)
}

/// Memory the system reports as available for new work, in bytes. Unit tests
/// of this crate see an injected figure (a large one unless a test sets one)
/// so that they do not depend on the machine they run on.
pub fn available_memory() -> Result<u64, String> {
    #[cfg(test)]
    return Ok(tests::injected().unwrap_or(1024 * GIB));
    #[cfg(not(test))]
    read_available_memory()
}

fn read_available_memory() -> Result<u64, String> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/meminfo")
            .map_err(|e| format!("cannot read /proc/meminfo: {e}"))?;
        parse_meminfo_available(&text)
            .ok_or_else(|| "/proc/meminfo has no MemAvailable line".to_string())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(
            "free memory is read from /proc/meminfo, which this platform does not have; \
             the cap still applies"
                .into(),
        )
    }
}

/// Check a job projected to use `projected_bytes` against the memory free now.
pub fn admit(projected_bytes: u64, policy: &MemoryPolicy) -> Admission {
    decide(projected_bytes, available_memory(), policy.margin_bytes)
}

/// Peak memory of a process group, sampled while it runs.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod sampler {
    #[cfg(target_os = "linux")]
    use std::path::Path;

    #[cfg(target_os = "linux")]
    /// The process group id in the text of `/proc/<pid>/stat`. The command
    /// name is in parentheses and may itself contain spaces or parentheses,
    /// so fields are counted from the last `)`.
    pub fn parse_stat_group(text: &str) -> Option<i32> {
        let rest = &text[text.rfind(')')? + 1..];
        // state ppid pgrp ...
        rest.split_whitespace().nth(2)?.parse().ok()
    }

    #[cfg(target_os = "linux")]
    /// A `kB` field from the text of `/proc/<pid>/status`, in bytes.
    pub fn parse_status_kib(text: &str, field: &str) -> Option<u64> {
        let line = text.lines().find(|l| l.starts_with(field))?;
        let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        kib.checked_mul(1024)
    }

    #[cfg(target_os = "linux")]
    /// Resident bytes now (summed over the group) and the largest
    /// kernel-kept high-water mark of any one process in it.
    pub fn sample(group: i32, proc_root: &Path) -> Option<(u64, u64)> {
        let mut resident = 0_u64;
        let mut high_water = 0_u64;
        let mut found = false;
        for entry in std::fs::read_dir(proc_root).ok()?.flatten() {
            let name = entry.file_name();
            if !name.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                continue;
            };
            if parse_stat_group(&stat) != Some(group) {
                continue;
            }
            let Ok(status) = std::fs::read_to_string(entry.path().join("status")) else {
                continue;
            };
            found = true;
            resident += parse_status_kib(&status, "VmRSS:").unwrap_or(0);
            high_water = high_water.max(parse_status_kib(&status, "VmHWM:").unwrap_or(0));
        }
        found.then_some((resident, high_water))
    }

    /// Resident bytes now (summed over the process group, as the physical footprint, which
    /// counts memory the system has compressed) and a high-water mark the kernel does not
    /// keep here, so the second figure is zero.
    #[cfg(target_os = "macos")]
    pub fn sample(group: i32) -> Option<(u64, u64)> {
        use libproc::{
            pid_rusage::{RUsageInfoV2, pidrusage},
            processes::{ProcFilter, pids_by_type},
        };
        let group = u32::try_from(group).ok()?;
        let pids = pids_by_type(ProcFilter::ByProgramGroup { pgrpid: group }).ok()?;
        let mut footprint = 0_u64;
        let mut found = false;
        for pid in pids {
            let Ok(pid) = i32::try_from(pid) else {
                continue;
            };
            // A process that exited between the listing and the read is simply not counted.
            let Ok(usage) = pidrusage::<RUsageInfoV2>(pid) else {
                continue;
            };
            found = true;
            footprint = footprint.saturating_add(usage.ri_phys_footprint);
        }
        found.then_some((footprint, 0))
    }

    /// Keeps the largest figure seen.
    #[derive(Default)]
    pub struct Peak(Option<u64>);

    impl Peak {
        pub fn observe(&mut self, sample: Option<(u64, u64)>) {
            if let Some((resident, high_water)) = sample {
                self.0 = Some(self.0.unwrap_or(0).max(resident).max(high_water));
            }
        }
        pub fn bytes(&self) -> Option<u64> {
            self.0
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::Cell;

    thread_local! {
        static FREE: Cell<Option<u64>> = const { Cell::new(None) };
    }

    /// Free memory injected for the calling thread, when a test set one.
    pub(crate) fn injected() -> Option<u64> {
        FREE.with(Cell::get)
    }

    pub(crate) fn inject_free(bytes: Option<u64>) {
        FREE.with(|f| f.set(bytes));
    }

    // Verifies: PERF-041
    #[test]
    fn a_job_that_fits_is_admitted_and_one_that_does_not_is_refused_with_a_reason() {
        let admitted = decide(4 * GIB, Ok(8 * GIB), GIB);
        assert!(matches!(admitted, Admission::Admitted { .. }));
        assert!(admitted.record().starts_with("PASS: "));
        // Exactly projected + margin free is still admitted.
        assert!(matches!(
            decide(4 * GIB, Ok(5 * GIB), GIB),
            Admission::Admitted { .. }
        ));
        let Admission::Refused(why) = decide(4 * GIB, Ok(5 * GIB - 1), GIB) else {
            panic!("a job one byte short must be refused");
        };
        assert!(
            why.contains("4.0 GiB") && why.contains("only 5.0 GiB"),
            "{why}"
        );
        assert!(why.contains("FARIS_MEMORY_CAP_MB"), "{why}");
    }

    // Verifies: PERF-041
    #[test]
    fn an_unreadable_free_figure_is_not_evaluated_not_guessed() {
        let decision = decide(4 * GIB, Err("no /proc here".into()), GIB);
        assert_eq!(decision, Admission::NotEvaluated("no /proc here".into()));
        assert_eq!(decision.record(), "NOT_EVALUATED: no /proc here");
    }

    // Verifies: PERF-041
    #[test]
    fn the_cap_lowers_but_never_raises_a_jobs_address_space() {
        let capped = MemoryPolicy {
            cap_bytes: Some(2 * GIB),
            margin_bytes: GIB,
        };
        assert_eq!(effective_address_space(4 * GIB, &capped), 2 * GIB);
        assert_eq!(effective_address_space(GIB, &capped), GIB);
        assert_eq!(
            effective_address_space(4 * GIB, &MemoryPolicy::default()),
            4 * GIB
        );
    }

    // Verifies: PERF-041
    #[cfg(target_os = "linux")]
    #[test]
    fn this_machines_free_memory_is_readable() {
        assert!(read_available_memory().unwrap() > 0);
    }

    // Verifies: PERF-041
    #[test]
    fn a_refusal_comes_from_the_injected_free_figure() {
        inject_free(Some(GIB));
        let decision = admit(4 * GIB, &MemoryPolicy::default());
        inject_free(None);
        assert!(matches!(decision, Admission::Refused(_)));
    }

    // Verifies: PERF-041
    #[test]
    fn meminfo_and_proc_text_parse() {
        let meminfo =
            "MemTotal:       32000000 kB\nMemFree:  100 kB\nMemAvailable:   20971520 kB\n";
        assert_eq!(parse_meminfo_available(meminfo), Some(20 * GIB));
        assert_eq!(parse_meminfo_available("MemTotal: 1 kB\n"), None);
        #[cfg(target_os = "linux")]
        {
            use sampler::*;
            // A command name with spaces and a parenthesis must not shift fields.
            assert_eq!(
                parse_stat_group("42 (a b) c) S 1 4242 4242 0 -1"),
                Some(4242)
            );
            let status = "Name:\tx\nVmHWM:\t    2048 kB\nVmRSS:\t    1024 kB\n";
            assert_eq!(parse_status_kib(status, "VmRSS:"), Some(MIB));
            assert_eq!(parse_status_kib(status, "VmHWM:"), Some(2 * MIB));
            let mut peak = Peak::default();
            assert_eq!(peak.bytes(), None);
            peak.observe(Some((3, 2)));
            peak.observe(None);
            peak.observe(Some((1, 5)));
            assert_eq!(peak.bytes(), Some(5));
        }
    }
}
