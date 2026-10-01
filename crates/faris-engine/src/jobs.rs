//! Bounded external execution, independent of scientific acceptance.
//!
//! Call from a worker, never the UI thread. On Unix each job owns a process
//! group; cancellation terminates that group, including ordinary solver children.
//! This is not a sandbox for hostile executables and does not limit RAM or disk.

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
#[cfg(unix)]
pub fn run_job(spec: &JobSpec, cancellation: &Cancellation) -> Result<JobResult, JobError> {
    use nix::{
        errno::Errno,
        sys::signal::{Signal, killpg},
        unistd::Pid,
    };
    use std::os::unix::process::CommandExt;

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
        });
    }
    let mut child = Command::new(&spec.program)
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
        match child.try_wait() {
            Ok(Some(status)) => {
                break Ok((
                    if status.success() {
                        ExecutionStatus::Succeeded
                    } else {
                        ExecutionStatus::Failed
                    },
                    status.code(),
                ));
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => break Err(error),
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
    })
}

/// Full descendant cancellation is currently implemented for Unix only.
#[cfg(not(unix))]
pub fn run_job(_spec: &JobSpec, _cancellation: &Cancellation) -> Result<JobResult, JobError> {
    Err(JobError::Invalid(
        "external job execution requires the Unix process-group adapter",
    ))
}

#[cfg(all(test, unix))]
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
        };
        (dir, spec)
    }
    #[test]
    fn preserves_exit_code_and_does_not_infer_a_scientific_verdict() {
        let (_dir, spec) = shell("printf 'raw tally'; printf 'solver note' >&2; exit 7");
        let result = run_job(&spec, &Cancellation::default()).unwrap();
        assert_eq!(result.execution_status, ExecutionStatus::Failed);
        assert_eq!(result.exit_code, Some(7));
        assert_eq!(result.stdout, "raw tally");
        assert_eq!(result.stderr, "solver note");
    }
    #[test]
    fn timeout_cleans_up_descendants_holding_the_output_pipe() {
        let (_dir, mut spec) = shell("/bin/sleep 30 & wait");
        spec.timeout = Duration::from_millis(80);
        let result = run_job(&spec, &Cancellation::default()).unwrap();
        assert_eq!(result.execution_status, ExecutionStatus::TimedOut);
        assert!(result.elapsed_seconds < 3.0);
    }
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
}
