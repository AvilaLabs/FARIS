//! Bounded external execution, independent of scientific acceptance.
//!
//! Call from a worker, never the UI thread. On Unix each job owns a process
//! group; cancellation terminates that group, including ordinary solver children.
//! Linux `prlimit` applies inherited address-space and single-file limits. The
//! runner samples owned artifact roots and terminates on aggregate limits. This
//! is a resource-bounded execution wrapper, not a sandbox for hostile programs.

use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    io::{Read, Result as IoResult},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub const MAX_CAPTURE_BYTES: usize = 4 * 1024 * 1024;
pub const ARTIFACT_SCAN_INTERVAL: Duration = Duration::from_millis(100);

/// Default per-job ceilings; callers may request lower limits, never higher.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResourceLimits {
    pub address_space_bytes: u64,
    pub single_file_bytes: u64,
    pub artifact_total_bytes: u64,
    pub artifact_file_count: u64,
}

impl ResourceLimits {
    pub const DEFAULT: Self = Self {
        address_space_bytes: 4 * 1024 * 1024 * 1024,
        single_file_bytes: 256 * 1024 * 1024,
        artifact_total_bytes: 512 * 1024 * 1024,
        artifact_file_count: 2048,
    };

    fn valid(self) -> bool {
        self.address_space_bytes > 0
            && self.address_space_bytes <= Self::DEFAULT.address_space_bytes
            && self.single_file_bytes > 0
            && self.single_file_bytes <= Self::DEFAULT.single_file_bytes
            && self.artifact_total_bytes > 0
            && self.artifact_total_bytes <= Self::DEFAULT.artifact_total_bytes
            && self.artifact_file_count > 0
            && self.artifact_file_count <= Self::DEFAULT.artifact_file_count
    }
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Clone, Debug)]
pub struct JobSpec {
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: PathBuf,
    /// The only environment variables passed to this job; no inherited secrets.
    pub environment: Vec<(OsString, OsString)>,
    pub timeout: Duration,
    /// Limit for each stream; exceeding either terminates the job.
    pub capture_limit_bytes: usize,
    /// Additional output locations outside the working directory.
    pub artifact_roots: Vec<PathBuf>,
    /// Requested ceilings, bounded above by the implementation defaults.
    pub resource_limits: ResourceLimits,
}

#[derive(Clone, Default, Debug)]
pub struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    /// Shared atomic flag for safe platform interrupt handlers.
    pub fn shared_flag(&self) -> Arc<AtomicBool> {
        self.0.clone()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExecutionStatus {
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    OutputLimit,
    ArtifactLimit,
    FileSizeLimit,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JobResult {
    pub execution_status: ExecutionStatus,
    pub exit_code: Option<i32>,
    pub elapsed_seconds: f64,
    pub stdout: String,
    pub stderr: String,
    /// Captured text is lossy UTF-8; original solver artifacts remain separate.
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    /// Defaults on older serialized records that predate resource reporting.
    #[serde(default)]
    pub resource_limits: ResourceLimits,
    #[serde(default)]
    pub artifact_files_observed: u64,
    #[serde(default)]
    pub artifact_bytes_observed: u64,
    #[serde(default)]
    pub largest_artifact_file_bytes_observed: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("invalid job: {0}")]
    Invalid(&'static str),
    #[error("external job I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("job output reader panicked")]
    ReaderPanic,
}

#[derive(Debug)]
struct Capture {
    bytes: Vec<u8>,
    truncated: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct ArtifactUsage {
    files: u64,
    bytes: u64,
    largest_file_bytes: u64,
    exceeded_scan_bound: bool,
}

fn artifact_usage(roots: &[PathBuf], limits: ResourceLimits) -> Result<ArtifactUsage, JobError> {
    use std::{collections::HashSet, fs};
    let mut seen_roots = HashSet::new();
    let mut pending = Vec::new();
    for root in roots {
        if root.exists() {
            let canonical = root.canonicalize()?;
            if !seen_roots
                .iter()
                .any(|outer: &PathBuf| canonical.starts_with(outer))
            {
                seen_roots.insert(canonical.clone());
                pending.push(canonical);
            }
        }
    }
    let mut usage = ArtifactUsage::default();
    let mut entries_scanned = 0_u64;
    let maximum_entries = limits.artifact_file_count.saturating_mul(8).max(4096);
    // The worker deletes files while it runs (intermediate statepoints), so an
    // entry listed by read_dir may be gone before it is examined. A vanished
    // file or directory holds no space; skip it rather than fail the job.
    let vanished = |error: &std::io::Error| error.kind() == std::io::ErrorKind::NotFound;
    while let Some(directory) = pending.pop() {
        let listing = match fs::read_dir(directory) {
            Err(error) if vanished(&error) => continue,
            other => other?,
        };
        for entry in listing {
            entries_scanned = entries_scanned.saturating_add(1);
            if entries_scanned > maximum_entries {
                usage.exceeded_scan_bound = true;
                return Ok(usage);
            }
            let entry = entry?;
            let path = entry.path();
            let metadata = match fs::symlink_metadata(&path) {
                Err(error) if vanished(&error) => continue,
                other => other?,
            };
            let kind = metadata.file_type();
            if kind.is_symlink() {
                // Exclude external nuclear-data links and never recurse through
                // symlinks into arbitrary user directories.
                continue;
            }
            if kind.is_dir() {
                pending.push(path);
                if pending.len() as u64 > limits.artifact_file_count {
                    usage.exceeded_scan_bound = true;
                    return Ok(usage);
                }
            } else if kind.is_file() {
                usage.files = usage.files.saturating_add(1);
                usage.bytes = usage.bytes.saturating_add(metadata.len());
                usage.largest_file_bytes = usage.largest_file_bytes.max(metadata.len());
                if usage.files > limits.artifact_file_count
                    || usage.bytes > limits.artifact_total_bytes
                    || usage.largest_file_bytes > limits.single_file_bytes
                {
                    return Ok(usage);
                }
            }
        }
    }
    Ok(usage)
}

fn capture(mut stream: impl Read, limit: usize, exceeded: Arc<AtomicBool>) -> IoResult<Capture> {
    let mut bytes = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0u8; 8192];
    let mut truncated = false;
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = limit - bytes.len();
        bytes.extend_from_slice(&buffer[..count.min(remaining)]);
        if count > remaining {
            truncated = true;
            exceeded.store(true, Ordering::Release);
        }
    }
    Ok(Capture { bytes, truncated })
}

/// Execute synchronously on the calling worker with bounded wall time and logs.
#[cfg(target_os = "linux")]
pub fn run_job(spec: &JobSpec, cancellation: &Cancellation) -> Result<JobResult, JobError> {
    use nix::{
        errno::Errno,
        sys::signal::{Signal, killpg},
        unistd::Pid,
    };
    use std::os::unix::process::{CommandExt, ExitStatusExt};

    if !spec.program.is_absolute() || !spec.program.is_file() {
        return Err(JobError::Invalid(
            "program must be an explicit absolute file path",
        ));
    }
    if !spec.working_directory.is_absolute() || !spec.working_directory.is_dir() {
        return Err(JobError::Invalid(
            "working directory must be an existing absolute directory",
        ));
    }
    if spec.timeout.is_zero() || spec.timeout > Duration::from_secs(86400) {
        return Err(JobError::Invalid("timeout must be in (0, 86400] seconds"));
    }
    if spec.capture_limit_bytes == 0 || spec.capture_limit_bytes > MAX_CAPTURE_BYTES {
        return Err(JobError::Invalid(
            "each log limit must be in 1..=4194304 bytes",
        ));
    }
    if !spec.resource_limits.valid() {
        return Err(JobError::Invalid(
            "resource limits must be positive and no greater than the runner defaults",
        ));
    }
    if spec.artifact_roots.len() > 16 {
        return Err(JobError::Invalid(
            "at most 16 artifact roots may be configured",
        ));
    }
    let mut artifact_roots = vec![spec.working_directory.clone()];
    for root in &spec.artifact_roots {
        if !root.is_absolute() {
            return Err(JobError::Invalid("artifact roots must be absolute paths"));
        }
        std::fs::create_dir_all(root)?;
        if !root.is_dir() {
            return Err(JobError::Invalid("artifact roots must be directories"));
        }
        artifact_roots.push(root.clone());
    }
    let initial_usage = artifact_usage(&artifact_roots, spec.resource_limits)?;
    if initial_usage.exceeded_scan_bound
        || initial_usage.files > spec.resource_limits.artifact_file_count
        || initial_usage.bytes > spec.resource_limits.artifact_total_bytes
        || initial_usage.largest_file_bytes > spec.resource_limits.single_file_bytes
    {
        return Err(JobError::Invalid(
            "existing artifact roots already exceed configured limits",
        ));
    }
    let prlimit = [
        PathBuf::from("/usr/bin/prlimit"),
        PathBuf::from("/bin/prlimit"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
    .ok_or(JobError::Invalid(
        "Linux prlimit utility is required for bounded execution",
    ))?;
    let start = Instant::now();
    if cancellation.is_cancelled() {
        return Ok(JobResult {
            execution_status: ExecutionStatus::Cancelled,
            exit_code: None,
            elapsed_seconds: 0.0,
            stdout: String::new(),
            stderr: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            resource_limits: spec.resource_limits,
            artifact_files_observed: initial_usage.files,
            artifact_bytes_observed: initial_usage.bytes,
            largest_artifact_file_bytes_observed: initial_usage.largest_file_bytes,
        });
    }
    let mut child = Command::new(prlimit)
        .arg(format!("--as={}", spec.resource_limits.address_space_bytes))
        .arg(format!(
            "--fsize={}",
            spec.resource_limits.single_file_bytes
        ))
        .arg("--")
        .arg(&spec.program)
        .args(&spec.arguments)
        .current_dir(&spec.working_directory)
        .env_clear()
        .envs(spec.environment.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    let pid = Pid::from_raw(
        i32::try_from(child.id()).map_err(|_| JobError::Invalid("child PID out of range"))?,
    );
    let exceeded = Arc::new(AtomicBool::new(false));
    let stdout = child.stdout.take().expect("piped child stdout");
    let stderr = child.stderr.take().expect("piped child stderr");
    let limit = spec.capture_limit_bytes;
    let out_flag = exceeded.clone();
    let err_flag = exceeded.clone();
    let out_reader = thread::spawn(move || capture(stdout, limit, out_flag));
    let err_reader = thread::spawn(move || capture(stderr, limit, err_flag));

    let mut observed_usage: ArtifactUsage;
    let mut next_artifact_scan = Instant::now();
    let outcome = loop {
        if cancellation.is_cancelled() {
            break Ok((ExecutionStatus::Cancelled, None));
        }
        if exceeded.load(Ordering::Acquire) {
            break Ok((ExecutionStatus::OutputLimit, None));
        }
        if start.elapsed() >= spec.timeout {
            break Ok((ExecutionStatus::TimedOut, None));
        }
        if Instant::now() >= next_artifact_scan {
            observed_usage = match artifact_usage(&artifact_roots, spec.resource_limits) {
                Ok(usage) => usage,
                Err(error) => break Err(error),
            };
            if observed_usage.largest_file_bytes > spec.resource_limits.single_file_bytes {
                break Ok((ExecutionStatus::FileSizeLimit, None));
            }
            if observed_usage.exceeded_scan_bound
                || observed_usage.files > spec.resource_limits.artifact_file_count
                || observed_usage.bytes > spec.resource_limits.artifact_total_bytes
            {
                break Ok((ExecutionStatus::ArtifactLimit, None));
            }
            next_artifact_scan = Instant::now() + ARTIFACT_SCAN_INTERVAL;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                break Ok((
                    if status.signal() == Some(Signal::SIGXFSZ as i32) {
                        ExecutionStatus::FileSizeLimit
                    } else if status.success() {
                        ExecutionStatus::Succeeded
                    } else {
                        ExecutionStatus::Failed
                    },
                    status.code(),
                ));
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => break Err(JobError::Io(error)),
        }
    };
    // Close pipes held by descendant processes even when the direct child exits.
    // ESRCH means the owned group has already exited.
    let cleanup = match killpg(pid, Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(error) => Err(std::io::Error::from_raw_os_error(error as i32)),
    };
    let waited = child.wait();
    let out = out_reader.join().map_err(|_| JobError::ReaderPanic)??;
    let err = err_reader.join().map_err(|_| JobError::ReaderPanic)??;
    cleanup?;
    let (mut execution_status, exit_code) = outcome?;
    let waited = waited?;
    // Catch short-lived bursts that may finish between watchdog polls. A
    // completed job that crossed either aggregate ceiling is never succeeded.
    observed_usage = artifact_usage(&artifact_roots, spec.resource_limits)?;
    if observed_usage.largest_file_bytes > spec.resource_limits.single_file_bytes {
        execution_status = ExecutionStatus::FileSizeLimit;
    } else if observed_usage.exceeded_scan_bound
        || observed_usage.files > spec.resource_limits.artifact_file_count
        || observed_usage.bytes > spec.resource_limits.artifact_total_bytes
        || observed_usage.largest_file_bytes > spec.resource_limits.single_file_bytes
    {
        execution_status = ExecutionStatus::ArtifactLimit;
    }
    // A short-lived process may exit before the reader detects oversized output.
    if matches!(
        execution_status,
        ExecutionStatus::Succeeded | ExecutionStatus::Failed
    ) && (out.truncated || err.truncated)
    {
        execution_status = ExecutionStatus::OutputLimit;
    }
    Ok(JobResult {
        execution_status,
        exit_code: exit_code.or(waited.code()),
        elapsed_seconds: start.elapsed().as_secs_f64(),
        stdout: String::from_utf8_lossy(&out.bytes).into_owned(),
        stderr: String::from_utf8_lossy(&err.bytes).into_owned(),
        stdout_truncated: out.truncated,
        stderr_truncated: err.truncated,
        resource_limits: spec.resource_limits,
        artifact_files_observed: observed_usage.files,
        artifact_bytes_observed: observed_usage.bytes,
        largest_artifact_file_bytes_observed: observed_usage.largest_file_bytes,
    })
}

/// Resource-enforced execution currently requires Linux's trusted `prlimit`.
#[cfg(not(target_os = "linux"))]
pub fn run_job(_spec: &JobSpec, _cancellation: &Cancellation) -> Result<JobResult, JobError> {
    Err(JobError::Invalid(
        "bounded external job execution currently requires Linux prlimit",
    ))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    fn shell(script: &str) -> (tempfile::TempDir, JobSpec) {
        let dir = tempfile::tempdir().unwrap();
        let spec = JobSpec {
            program: PathBuf::from("/bin/sh"),
            arguments: vec!["-c".into(), script.into()],
            working_directory: dir.path().to_owned(),
            environment: vec![],
            timeout: Duration::from_secs(3),
            capture_limit_bytes: 1024,
            artifact_roots: vec![],
            resource_limits: ResourceLimits::default(),
        };
        (dir, spec)
    }
    // Verifies: NUC-004
    #[test]
    fn preserves_exit_code_and_does_not_infer_a_scientific_verdict() {
        let (_dir, spec) = shell("printf 'raw tally'; printf 'solver note' >&2; exit 7");
        let result = run_job(&spec, &Cancellation::default()).unwrap();
        assert_eq!(result.execution_status, ExecutionStatus::Failed);
        assert_eq!(result.exit_code, Some(7));
        assert_eq!(result.stdout, "raw tally");
        assert_eq!(result.stderr, "solver note");
    }
    // Verifies: AUTO-040, REL-006
    #[test]
    fn timeout_cleans_up_descendants_holding_the_output_pipe() {
        let (_dir, mut spec) = shell("/bin/sleep 30 & wait");
        spec.timeout = Duration::from_millis(80);
        let result = run_job(&spec, &Cancellation::default()).unwrap();
        assert_eq!(result.execution_status, ExecutionStatus::TimedOut);
        assert!(result.elapsed_seconds < 3.0);
    }
    // Verifies: AUTO-040
    #[test]
    fn cancellation_terminates_a_running_job() {
        let (_dir, spec) = shell("/bin/sleep 30 & wait");
        let cancellation = Cancellation::default();
        let remote = cancellation.clone();
        let worker = thread::spawn(move || run_job(&spec, &remote).unwrap());
        thread::sleep(Duration::from_millis(80));
        cancellation.cancel();
        assert_eq!(
            worker.join().unwrap().execution_status,
            ExecutionStatus::Cancelled
        );
    }
    // Verifies: AUTO-040, SEC-063
    #[test]
    fn oversized_output_is_bounded_even_when_a_process_exits_quickly() {
        let (_dir, mut spec) = shell("/usr/bin/head -c 4096 /dev/zero");
        spec.capture_limit_bytes = 64;
        let result = run_job(&spec, &Cancellation::default()).unwrap();
        assert_eq!(result.execution_status, ExecutionStatus::OutputLimit);
        assert_eq!(result.stdout.len(), 64);
        assert!(result.stdout_truncated);
    }
    #[test]
    fn files_deleted_while_the_output_is_measured_do_not_fail_the_job() {
        // The OpenMC worker deletes per-batch statepoints while the runner
        // measures the output tree; a file listed and then removed must not
        // abort the job.
        let (dir, mut spec) = shell("true");
        spec.program = PathBuf::from("/usr/bin/python3");
        spec.arguments = vec![
            "-c".into(),
            "import os, shutil, time\n\
             end = time.time() + 2.5\n\
             while time.time() < end:\n\
             \x20   for d in range(8):\n\
             \x20       os.makedirs(f'churn/{d}', exist_ok=True)\n\
             \x20       for i in range(40): open(f'churn/{d}/statepoint.{i:03}.h5', 'wb').close()\n\
             \x20   for d in range(8):\n\
             \x20       for i in range(40): os.remove(f'churn/{d}/statepoint.{i:03}.h5')\n\
             \x20       shutil.rmtree(f'churn/{d}')\n"
                .into(),
        ];
        spec.timeout = Duration::from_secs(10);
        spec.artifact_roots = vec![dir.path().to_owned()];
        let result = run_job(&spec, &Cancellation::default()).unwrap();
        assert_eq!(
            result.execution_status,
            ExecutionStatus::Succeeded,
            "{}",
            result.stderr
        );
    }

    #[test]
    fn pre_cancelled_job_never_launches() {
        let (dir, spec) = shell("touch should-not-exist");
        let cancellation = Cancellation::default();
        cancellation.cancel();
        assert_eq!(
            run_job(&spec, &cancellation).unwrap().execution_status,
            ExecutionStatus::Cancelled
        );
        assert!(!dir.path().join("should-not-exist").exists());
    }

    // Verifies: AUTO-040, SEC-061
    #[test]
    fn inherited_address_space_limit_bounds_child_allocation() {
        let dir = tempfile::tempdir().unwrap();
        let mut spec = shell("true").1;
        spec.program = PathBuf::from("/usr/bin/python3");
        spec.arguments = vec!["-c".into(), "bytearray(5 * 1024**3)".into()];
        spec.working_directory = dir.path().to_owned();
        spec.timeout = Duration::from_secs(10);
        let result = run_job(&spec, &Cancellation::default()).unwrap();
        assert_eq!(result.execution_status, ExecutionStatus::Failed);
        assert!(result.exit_code.is_some_and(|code| code != 0));
        assert_eq!(
            result.resource_limits.address_space_bytes,
            4 * 1024 * 1024 * 1024
        );
    }

    // Verifies: AUTO-040, SEC-061
    #[test]
    fn oversized_single_file_is_killed_by_inherited_file_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let mut spec = shell("true").1;
        spec.program = PathBuf::from("/usr/bin/dd");
        spec.arguments = vec![
            "if=/dev/zero".into(),
            "of=oversized.bin".into(),
            "bs=4096".into(),
            "count=32".into(),
            "status=none".into(),
        ];
        spec.working_directory = dir.path().to_owned();
        spec.resource_limits.single_file_bytes = 64 * 1024;
        let result = run_job(&spec, &Cancellation::default()).unwrap();
        assert_eq!(result.execution_status, ExecutionStatus::FileSizeLimit);
        assert_eq!(
            std::fs::metadata(dir.path().join("oversized.bin"))
                .unwrap()
                .len(),
            64 * 1024
        );
        assert_eq!(result.artifact_files_observed, 1);
        assert_eq!(result.artifact_bytes_observed, 64 * 1024);
    }

    // Verifies: AUTO-040
    #[test]
    fn aggregate_artifact_bytes_are_checked_even_for_short_jobs() {
        let (dir, mut spec) = shell(
            "dd if=/dev/zero of=a bs=4096 count=8 status=none; dd if=/dev/zero of=b bs=4096 count=8 status=none",
        );
        spec.resource_limits.artifact_total_bytes = 48 * 1024;
        let result = run_job(&spec, &Cancellation::default()).unwrap();
        assert_eq!(result.execution_status, ExecutionStatus::ArtifactLimit);
        assert!(result.artifact_bytes_observed > result.resource_limits.artifact_total_bytes);
        assert_eq!(result.artifact_files_observed, 2);
        assert!(dir.path().join("a").exists());
        assert!(dir.path().join("b").exists());
    }

    #[test]
    fn aggregate_artifact_file_count_includes_explicit_external_roots() {
        let (dir, mut spec) = shell("mkdir -p extra; touch a b; touch extra/c extra/d");
        let extra = dir.path().join("extra");
        spec.artifact_roots = vec![extra];
        spec.resource_limits.artifact_file_count = 3;
        let result = run_job(&spec, &Cancellation::default()).unwrap();
        assert_eq!(result.execution_status, ExecutionStatus::ArtifactLimit);
        assert_eq!(result.artifact_files_observed, 4);
    }

    #[test]
    fn artifact_scanner_does_not_follow_external_symlinks() {
        let (dir, mut spec) = shell("touch local; ln -s /tmp external-link");
        spec.artifact_roots = vec![dir.path().to_owned()];
        let result = run_job(&spec, &Cancellation::default()).unwrap();
        assert_eq!(result.execution_status, ExecutionStatus::Succeeded);
        assert_eq!(result.artifact_files_observed, 1);
    }

    #[test]
    fn legacy_job_results_default_resource_metadata() {
        let result: JobResult = serde_json::from_str(
            r#"{"execution_status":"SUCCEEDED","exit_code":0,"elapsed_seconds":1.0,"stdout":"","stderr":"","stdout_truncated":false,"stderr_truncated":false}"#,
        )
        .unwrap();
        assert_eq!(result.resource_limits, ResourceLimits::default());
        assert_eq!(result.artifact_files_observed, 0);
        assert_eq!(result.artifact_bytes_observed, 0);
    }
}
