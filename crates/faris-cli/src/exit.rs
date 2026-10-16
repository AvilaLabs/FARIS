//! The command line's exit codes (AUTO-004): one per failure class, so a script can tell
//! a bad argument from a rejected file from a solver that failed without reading text.
//! `docs/USER_GUIDE.md` lists them; `EXIT_CODES` is the single table both the help text
//! and the test read.

use faris_engine::jobs::{ExecutionStatus, JobError};

pub const OK: u8 = 0;
/// A file or evidence record was read and failed verification or compilation.
pub const REJECTED: u8 = 1;
/// A well-formed command whose input is missing, unreadable or invalid.
pub const VALIDATION: u8 = 2;
/// An external tool ran and did not succeed.
pub const SOLVER_FAILURE: u8 = 3;
/// The user (Ctrl-C or SIGTERM) or the caller stopped the job.
pub const CANCELLED: u8 = 4;
/// The memory check refused the job before it started.
pub const RESOURCE_REFUSED: u8 = 5;
/// A comparison ran and found the two sides different (`faris diff`, `faris config diff`).
pub const DIFFERENT: u8 = 6;
/// The command line itself did not parse (BSD `EX_USAGE`).
pub const USAGE: u8 = 64;

pub const EXIT_CODES: [(u8, &str); 8] = [
    (OK, "ok"),
    (
        REJECTED,
        "rejected evidence: a file failed verification or compilation",
    ),
    (
        VALIDATION,
        "validation: an input is missing, unreadable or invalid",
    ),
    (
        SOLVER_FAILURE,
        "solver failure: an external tool ran and did not succeed",
    ),
    (
        CANCELLED,
        "cancelled: the job was stopped by Ctrl-C or SIGTERM",
    ),
    (
        RESOURCE_REFUSED,
        "resource refused: the memory check refused the job",
    ),
    (
        DIFFERENT,
        "different: a comparison found the two sides differ",
    ),
    (USAGE, "usage: the command line did not parse"),
];

/// Printed at the end of `faris --help`.
pub fn help_table() -> String {
    let mut text = String::from("Exit codes:\n");
    for (code, meaning) in EXIT_CODES {
        text.push_str(&format!("  {code:>2}  {meaning}\n"));
    }
    text
}

/// An external job that did not succeed; carries the plain-language reason.
#[derive(Debug)]
pub struct JobFailed {
    pub status: ExecutionStatus,
    pub reason: String,
}

impl std::fmt::Display for JobFailed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl std::error::Error for JobFailed {}

/// The exit code for an error: typed errors anywhere in its source chain decide, anything
/// else is a validation failure.
pub fn classify(error: &(dyn std::error::Error + 'static)) -> u8 {
    let mut current = Some(error);
    while let Some(error) = current {
        if error
            .downcast_ref::<crate::study::CoreCompilationRejected>()
            .is_some()
            || error
                .downcast_ref::<crate::study_file::StudyFileRejected>()
                .is_some()
            || error
                .downcast_ref::<crate::design::DesignCheckFailed>()
                .is_some()
            || error.downcast_ref::<crate::cache::CacheDamaged>().is_some()
        {
            return REJECTED;
        }
        if error
            .downcast_ref::<crate::diff::DifferencesFound>()
            .is_some()
        {
            return DIFFERENT;
        }
        if let Some(failed) = error.downcast_ref::<JobFailed>() {
            return if failed.status == ExecutionStatus::Cancelled {
                CANCELLED
            } else if failed.status == ExecutionStatus::MemoryLimit {
                RESOURCE_REFUSED
            } else {
                SOLVER_FAILURE
            };
        }
        if matches!(
            error.downcast_ref::<JobError>(),
            Some(JobError::MemoryRefused(_))
        ) {
            return RESOURCE_REFUSED;
        }
        current = error.source();
    }
    VALIDATION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_failure_class_has_its_own_code() {
        // Verifies: AUTO-004
        let job = |status| JobFailed {
            status,
            reason: "x".into(),
        };
        assert_eq!(classify(&job(ExecutionStatus::Failed)), SOLVER_FAILURE);
        assert_eq!(classify(&job(ExecutionStatus::TimedOut)), SOLVER_FAILURE);
        assert_eq!(classify(&job(ExecutionStatus::Cancelled)), CANCELLED);
        assert_eq!(
            classify(&job(ExecutionStatus::MemoryLimit)),
            RESOURCE_REFUSED
        );
        assert_eq!(
            classify(&JobError::MemoryRefused("no".into())),
            RESOURCE_REFUSED
        );
        assert_eq!(
            classify(&crate::study_file::StudyFileRejected("bad".into())),
            REJECTED
        );
        assert_eq!(classify(&std::io::Error::other("missing")), VALIDATION);
        let codes: std::collections::BTreeSet<u8> = EXIT_CODES.iter().map(|c| c.0).collect();
        assert_eq!(codes.len(), EXIT_CODES.len(), "codes are distinct");
    }
}
