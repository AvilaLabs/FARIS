//! Bounded external execution, independent of scientific acceptance.
//!
//! Call from a worker, never the UI thread. The runner samples the working directory and any
//! owned artifact roots and stops the job when files or bytes pass the limits, whatever the
//! platform. The rest depends on it:
//!
//! - Linux: `prlimit` applies the address-space and single-file limits inside the kernel; the
//!   job owns a process group, and cancellation kills that group, solver children included.
//!   Memory is sampled from `/proc` for the peak.
//! - macOS: the job owns a process group and `sh` sets `ulimit -f` before it becomes the
//!   program. macOS does not enforce an address-space limit, so the runner sums the physical
//!   footprint of the group (through libproc) every [`MEMORY_SAMPLE_INTERVAL`] and stops the
//!   job with [`ExecutionStatus::MemoryLimit`] when it passes the cap. A spike shorter than
//!   the interval can be missed.
//! - Windows: the program is put in a job object that kills its processes when it closes.
//!   There is no kernel single-file limit, so the artifact scan alone enforces it. The safe
//!   job-object wrapper cannot set or read a job-wide memory limit, so the runner samples the
//!   committed memory of the program it started (not its descendants) and stops it with
//!   [`ExecutionStatus::MemoryLimit`] past the cap. The program runs briefly before it is
//!   assigned to the job; a process it starts in that moment is outside the job.
//!
//! This is a resource-bounded execution wrapper, not a sandbox for hostile programs.

use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    io::{Read, Result as IoResult},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub const MAX_CAPTURE_BYTES: usize = 4 * 1024 * 1024;
pub const ARTIFACT_SCAN_INTERVAL: Duration = Duration::from_millis(100);
/// How often a running job's memory is read. A spike shorter than this between two reads can
/// be missed; the kernel's own high-water mark for each process narrows that gap.
pub const MEMORY_SAMPLE_INTERVAL: Duration = Duration::from_millis(50);

/// Per-job ceilings. Callers may request lower limits than the defaults; only the address
/// space may be raised, up to [`ResourceLimits::ADDRESS_SPACE_CEILING`], because a CAD
/// conversion of a whole reactor model peaks above the 4 GiB default (RM-M: 4.6 GB resident).
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResourceLimits {
    pub address_space_bytes: u64,
    pub single_file_bytes: u64,
    /// Bytes the job may add to its artifact roots (what is there when it starts does not count).
    pub artifact_total_bytes: u64,
    /// Files the job may add to its artifact roots.
    pub artifact_file_count: u64,
}

impl ResourceLimits {
    pub const DEFAULT: Self = Self {
        address_space_bytes: 4 * 1024 * 1024 * 1024,
        single_file_bytes: 256 * 1024 * 1024,
        artifact_total_bytes: 512 * 1024 * 1024,
        artifact_file_count: 2048,
    };

    /// The most address space any job may ask for.
    pub const ADDRESS_SPACE_CEILING: u64 = 16 * 1024 * 1024 * 1024;

    /// The default limits with a different address space (bounded by the ceiling when run).
    pub const fn with_address_space(address_space_bytes: u64) -> Self {
        Self {
            address_space_bytes,
            ..Self::DEFAULT
        }
    }

    fn valid(self) -> bool {
        self.address_space_bytes > 0
            && self.address_space_bytes <= Self::ADDRESS_SPACE_CEILING
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
    /// The job's memory passed the cap and the runner stopped it (macOS and Windows; on
    /// Linux the kernel refuses the allocation and the job fails instead).
    MemoryLimit,
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
    /// Peak resident memory of the job and its descendants, sampled while it ran, where the
    /// platform gives it (Linux: `/proc`; macOS: libproc; Windows: the started program only). `None` means not measured, not zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_memory_bytes: Option<u64>,
    /// The memory check made before the job started, as a verdict line (PERF-041).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_check: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("invalid job: {0}")]
    Invalid(&'static str),
    #[error("external job I/O: {0}")]
    Io(#[from] std::io::Error),
    /// The memory check refused the job; the text says why and what to do.
    #[error("{0}")]
    MemoryRefused(String),
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

impl ArtifactUsage {
    /// Bytes added since `start`; files that shrank or were removed count as nothing added.
    fn grown_bytes(&self, start: &Self) -> u64 {
        self.bytes.saturating_sub(start.bytes)
    }

    fn grown_files(&self, start: &Self) -> u64 {
        self.files.saturating_sub(start.files)
    }
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
                // The totals are not a reason to stop counting: they cap growth from the
                // start of the job, so both scans must count everything (the entry bound
                // above keeps the scan finite).
                if usage.largest_file_bytes > limits.single_file_bytes {
                    return Ok(usage);
                }
            }
        }
    }
    Ok(usage)
}

/// Longest line handed to a line callback; the rest of a longer line is dropped.
pub const MAX_LINE_BYTES: usize = 64 * 1024;

type LineCallback<'a> = &'a (dyn Fn(&str) + Sync);

/// Splits a byte stream into lines for a callback, holding at most [`MAX_LINE_BYTES`].
struct LineSplitter<'a> {
    pending: Vec<u8>,
    callback: LineCallback<'a>,
}

impl<'a> LineSplitter<'a> {
    fn new(callback: LineCallback<'a>) -> Self {
        Self {
            pending: Vec::new(),
            callback,
        }
    }

    fn push(&mut self, mut chunk: &[u8]) {
        while let Some(newline) = chunk.iter().position(|&byte| byte == b'\n') {
            self.extend(&chunk[..newline]);
            self.flush();
            chunk = &chunk[newline + 1..];
        }
        self.extend(chunk);
    }

    fn extend(&mut self, piece: &[u8]) {
        let room = MAX_LINE_BYTES - self.pending.len();
        self.pending
            .extend_from_slice(&piece[..piece.len().min(room)]);
    }

    fn flush(&mut self) {
        // Windows tools end lines with CR LF; the carriage return is not part of the line.
        if cfg!(windows) && self.pending.last() == Some(&b'\r') {
            self.pending.pop();
        }
        (self.callback)(&String::from_utf8_lossy(&self.pending));
        self.pending.clear();
    }

    /// Delivers a final unterminated line when the stream closes.
    fn finish(&mut self) {
        if !self.pending.is_empty() {
            self.flush();
        }
    }
}

fn capture(
    mut stream: impl Read,
    limit: usize,
    exceeded: Arc<AtomicBool>,
    on_line: Option<LineCallback<'_>>,
) -> IoResult<Capture> {
    let mut bytes = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0u8; 8192];
    let mut truncated = false;
    let mut lines = on_line.map(LineSplitter::new);
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = limit - bytes.len();
        bytes.extend_from_slice(&buffer[..count.min(remaining)]);
        // Lines are cut from the retained bytes only, so delivery stops at the capture limit.
        if let Some(lines) = lines.as_mut() {
            lines.push(&buffer[..count.min(remaining)]);
        }
        if count > remaining {
            truncated = true;
            exceeded.store(true, Ordering::Release);
        }
    }
    if let Some(lines) = lines.as_mut() {
        lines.finish();
    }
    Ok(Capture { bytes, truncated })
}

/// Execute synchronously on the calling worker with bounded wall time and logs.
pub fn run_job(spec: &JobSpec, cancellation: &Cancellation) -> Result<JobResult, JobError> {
    run_job_with_lines(spec, cancellation, &|_| {})
}

/// [`run_job`] that also reports the child's stdout line by line while it runs.
///
/// `on_stdout_line` is called once per complete line (no trailing newline, lossy UTF-8),
/// in order, as soon as the line is read; a final unterminated line is delivered when the
/// stream closes. It runs on the thread that reads stdout, not the caller's thread, so it
/// must be quick and must not panic. A line over [`MAX_LINE_BYTES`] is delivered truncated.
/// Delivery stops at the stdout capture limit: only retained bytes are split into lines.
/// The returned `stdout` and every status are identical to [`run_job`].
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub fn run_job_with_lines(
    spec: &JobSpec,
    cancellation: &Cancellation,
    on_stdout_line: &(dyn Fn(&str) + Sync),
) -> Result<JobResult, JobError> {
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
    // The cap FARIS applies to every external job, then the check that the job fits.
    let memory_policy = crate::memory::policy();
    let mut limits = spec.resource_limits;
    limits.address_space_bytes =
        crate::memory::effective_address_space(limits.address_space_bytes, &memory_policy);
    let admission = crate::memory::admit(limits.address_space_bytes, &memory_policy);
    if let crate::memory::Admission::Refused(why) = admission {
        return Err(JobError::MemoryRefused(why));
    }
    let memory_check = Some(admission.record());
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
    let initial_usage = artifact_usage(&artifact_roots, limits)?;
    // The totals cap what the job writes, not what earlier steps left in its roots (a transport
    // run starts beside the geometry and windows its preparation made: over 200 MB on a large
    // machine), so only the scan bound and the single-file limit apply to what is there now.
    if initial_usage.exceeded_scan_bound
        || initial_usage.largest_file_bytes > limits.single_file_bytes
    {
        return Err(JobError::Invalid(
            "existing artifact roots already exceed configured limits",
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
            resource_limits: limits,
            artifact_files_observed: initial_usage.files,
            artifact_bytes_observed: initial_usage.bytes,
            largest_artifact_file_bytes_observed: initial_usage.largest_file_bytes,
            peak_memory_bytes: None,
            memory_check: memory_check.clone(),
        });
    }
    let mut running = platform::Running::start(spec, &limits)?;
    let exceeded = Arc::new(AtomicBool::new(false));
    let stdout = running.child.stdout.take().expect("piped child stdout");
    let stderr = running.child.stderr.take().expect("piped child stderr");
    let limit = spec.capture_limit_bytes;
    let out_flag = exceeded.clone();
    let err_flag = exceeded.clone();
    let (outcome, cleanup, waited, out, err, peak_memory) = thread::scope(|scope| {
        let out_reader =
            scope.spawn(move || capture(stdout, limit, out_flag, Some(on_stdout_line)));
        let err_reader = scope.spawn(move || capture(stderr, limit, err_flag, None));

        let mut observed_usage: ArtifactUsage;
        let mut next_artifact_scan = Instant::now();
        let mut peak = crate::memory::sampler::Peak::default();
        let mut next_memory_sample = Instant::now();
        let outcome: Result<_, JobError> = loop {
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
                observed_usage = match artifact_usage(&artifact_roots, limits) {
                    Ok(usage) => usage,
                    Err(error) => break Err(error),
                };
                if observed_usage.largest_file_bytes > limits.single_file_bytes {
                    break Ok((ExecutionStatus::FileSizeLimit, None));
                }
                if observed_usage.exceeded_scan_bound
                    || observed_usage.grown_files(&initial_usage) > limits.artifact_file_count
                    || observed_usage.grown_bytes(&initial_usage) > limits.artifact_total_bytes
                {
                    break Ok((ExecutionStatus::ArtifactLimit, None));
                }
                next_artifact_scan = Instant::now() + ARTIFACT_SCAN_INTERVAL;
            }
            // Read before try_wait: once the child is reaped its /proc entries are gone.
            if Instant::now() >= next_memory_sample {
                let sample = running.sample_memory();
                peak.observe(sample);
                // Where the kernel does not hold the job to its memory cap (macOS, and
                // Windows, whose job-object cap the safe wrapper cannot set), the runner does.
                if platform::MEMORY_CAP_BY_SAMPLING
                    && sample.is_some_and(|(now, _)| now > limits.address_space_bytes)
                {
                    break Ok((ExecutionStatus::MemoryLimit, None));
                }
                next_memory_sample = Instant::now() + MEMORY_SAMPLE_INTERVAL;
            }
            match running.child.try_wait() {
                Ok(Some(status)) => {
                    break Ok((platform::classify(&status), status.code()));
                }
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(error) => break Err(JobError::Io(error)),
            }
        };
        // Close pipes held by descendant processes even when the direct child exits.
        let cleanup = running.terminate();
        let waited = running.child.wait();
        let out = out_reader.join().map_err(|_| JobError::ReaderPanic)??;
        let err = err_reader.join().map_err(|_| JobError::ReaderPanic)??;
        Ok::<_, JobError>((outcome, cleanup, waited, out, err, peak.bytes()))
    })?;
    cleanup?;
    let (mut execution_status, exit_code) = outcome?;
    let waited = waited?;
    // Catch short-lived bursts that may finish between watchdog polls. A
    // completed job that crossed either aggregate ceiling is never succeeded.
    let observed_usage = artifact_usage(&artifact_roots, limits)?;
    if observed_usage.largest_file_bytes > limits.single_file_bytes {
        execution_status = ExecutionStatus::FileSizeLimit;
    } else if observed_usage.exceeded_scan_bound
        || observed_usage.grown_files(&initial_usage) > limits.artifact_file_count
        || observed_usage.grown_bytes(&initial_usage) > limits.artifact_total_bytes
        || observed_usage.largest_file_bytes > limits.single_file_bytes
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
        resource_limits: limits,
        artifact_files_observed: observed_usage.files,
        artifact_bytes_observed: observed_usage.bytes,
        largest_artifact_file_bytes_observed: observed_usage.largest_file_bytes,
        peak_memory_bytes: peak_memory,
        memory_check,
    })
}

/// The platform half of the runner: how the program is launched under its limits, how a running
/// job's memory is read, and how the job and everything it started is stopped.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
mod platform {
    use super::{ExecutionStatus, IoResult, JobError, JobSpec, ResourceLimits};
    use std::process::{Child, Command, ExitStatus, Stdio};

    /// A launched job. On Windows, dropping it closes the job object, which kills what is left.
    pub struct Running {
        pub child: Child,
        #[cfg(unix)]
        group: nix::unistd::Pid,
        #[cfg(windows)]
        job: Option<win32job::Job>,
    }

    /// The command with its working directory, environment (only the spec's) and pipes set.
    fn configure<'a>(command: &'a mut Command, spec: &JobSpec) -> &'a mut Command {
        command
            .current_dir(&spec.working_directory)
            .env_clear()
            .envs(spec.environment.iter().cloned())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub use unix::*;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    mod unix {
        use super::*;
        use nix::{
            errno::Errno,
            sys::signal::{Signal, killpg},
            unistd::Pid,
        };
        use std::os::unix::process::{CommandExt, ExitStatusExt};

        /// Linux holds the job to its address-space cap in the kernel.
        #[cfg(target_os = "linux")]
        pub const MEMORY_CAP_BY_SAMPLING: bool = false;
        /// macOS does not enforce `RLIMIT_AS`, so the runner compares samples to the cap.
        #[cfg(target_os = "macos")]
        pub const MEMORY_CAP_BY_SAMPLING: bool = true;

        impl Running {
            /// Starts the program as the leader of a new process group.
            pub fn start(spec: &JobSpec, limits: &ResourceLimits) -> Result<Self, JobError> {
                let mut command = limited_command(spec, limits)?;
                let child = configure(&mut command, spec).process_group(0).spawn()?;
                let group = Pid::from_raw(
                    i32::try_from(child.id())
                        .map_err(|_| JobError::Invalid("child PID out of range"))?,
                );
                Ok(Self { child, group })
            }

            /// `(resident now, kernel high-water mark)` summed over the job's process group.
            pub fn sample_memory(&self) -> Option<(u64, u64)> {
                #[cfg(target_os = "linux")]
                return crate::memory::sampler::sample(
                    self.group.as_raw(),
                    std::path::Path::new("/proc"),
                );
                #[cfg(target_os = "macos")]
                return crate::memory::sampler::sample(self.group.as_raw());
            }

            /// SIGKILL to the whole group. ESRCH means the owned group has already exited; macOS
            /// answers EPERM instead when the group holds only exited, unreaped processes.
            pub fn terminate(&mut self) -> IoResult<()> {
                match killpg(self.group, Signal::SIGKILL) {
                    Ok(()) | Err(Errno::ESRCH) => Ok(()),
                    #[cfg(target_os = "macos")]
                    Err(Errno::EPERM) => Ok(()),
                    Err(error) => Err(std::io::Error::from_raw_os_error(error as i32)),
                }
            }
        }

        pub fn classify(status: &ExitStatus) -> ExecutionStatus {
            if status.signal() == Some(Signal::SIGXFSZ as i32) {
                ExecutionStatus::FileSizeLimit
            } else if status.success() {
                ExecutionStatus::Succeeded
            } else {
                ExecutionStatus::Failed
            }
        }

        /// Linux: `prlimit` applies the address-space and file-size limits to the program.
        #[cfg(target_os = "linux")]
        fn limited_command(spec: &JobSpec, limits: &ResourceLimits) -> Result<Command, JobError> {
            use std::path::PathBuf;
            let prlimit = [
                PathBuf::from("/usr/bin/prlimit"),
                PathBuf::from("/bin/prlimit"),
            ]
            .into_iter()
            .find(|candidate| candidate.is_file())
            .ok_or(JobError::Invalid(
                "Linux prlimit utility is required for bounded execution",
            ))?;
            let mut command = Command::new(prlimit);
            command
                .arg(format!("--as={}", limits.address_space_bytes))
                .arg(format!("--fsize={}", limits.single_file_bytes))
                .arg("--")
                .arg(&spec.program)
                .args(&spec.arguments);
            Ok(command)
        }

        /// macOS: a shell sets the file-size limit, then replaces itself with the program.
        #[cfg(target_os = "macos")]
        fn limited_command(spec: &JobSpec, limits: &ResourceLimits) -> Result<Command, JobError> {
            let mut command = Command::new("/bin/sh");
            command
                .arg("-c")
                .arg(crate::jobs::macos_shell_script(limits.single_file_bytes))
                .arg(&spec.program)
                .args(&spec.arguments);
            Ok(command)
        }
    }

    #[cfg(windows)]
    pub const MEMORY_CAP_BY_SAMPLING: bool = true;

    #[cfg(windows)]
    impl Running {
        /// Starts the program, then puts it in a job object that kills everything in it when
        /// the job is closed. The program runs for a moment before it is assigned, so a
        /// grandchild it creates in that moment is outside the job and escapes termination.
        pub fn start(spec: &JobSpec, _limits: &ResourceLimits) -> Result<Self, JobError> {
            use std::os::windows::io::AsRawHandle;
            let mut limit_info = win32job::ExtendedLimitInfo::new();
            limit_info.limit_kill_on_job_close();
            let job = win32job::Job::create_with_limit_info(&limit_info).map_err(job_error)?;
            let mut command = Command::new(&spec.program);
            command.args(&spec.arguments);
            let mut child = configure(&mut command, spec).spawn()?;
            if let Err(error) = job.assign_process(child.as_raw_handle() as isize) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(job_error(error));
            }
            Ok(Self {
                child,
                job: Some(job),
            })
        }

        /// `(committed now, peak committed)` of the program the job started. The safe job-object
        /// wrapper reads no job-wide figure, so processes it started are not counted.
        pub fn sample_memory(&self) -> Option<(u64, u64)> {
            use std::os::windows::io::AsRawHandle;
            let counters =
                win32job::utils::get_process_memory_info(self.child.as_raw_handle() as isize)
                    .ok()?;
            Some((
                u64::try_from(counters.private_usage).ok()?,
                u64::try_from(counters.peak_pagefile_usage).ok()?,
            ))
        }

        /// Closing the job object terminates every process in it.
        pub fn terminate(&mut self) -> IoResult<()> {
            drop(self.job.take());
            let _ = self.child.kill();
            Ok(())
        }
    }

    #[cfg(windows)]
    fn job_error(error: win32job::JobError) -> JobError {
        JobError::Io(std::io::Error::other(error.to_string()))
    }

    #[cfg(windows)]
    pub fn classify(status: &ExitStatus) -> ExecutionStatus {
        if status.success() {
            ExecutionStatus::Succeeded
        } else {
            ExecutionStatus::Failed
        }
    }
}

/// The shell script that sets the single-file limit on macOS and then becomes the program
/// (`$0`, with the arguments after it). macOS `/bin/sh` counts `ulimit -f` in 1024-byte blocks
/// (measured on the macOS 14 and 15 runners: 128 blocks let a file reach 128 KiB); the count is
/// rounded down so a file never grows past the limit, and is at least one block (the artifact
/// scan still stops a file that outgrows a limit under 1024 bytes).
#[cfg(any(target_os = "macos", test))]
fn macos_shell_script(single_file_bytes: u64) -> String {
    let blocks = (single_file_bytes / 1024).max(1);
    format!("ulimit -f {blocks}; exec \"$0\" \"$@\"")
}

/// Bounded execution needs a process model this target lacks (the browser build).
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn run_job_with_lines(
    _spec: &JobSpec,
    _cancellation: &Cancellation,
    _on_stdout_line: &(dyn Fn(&str) + Sync),
) -> Result<JobResult, JobError> {
    Err(JobError::Invalid(
        "bounded external job execution is not available on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Runs `script` in the platform's shell: `sh -c` on Unix, `cmd /C` on Windows.
    fn shell(script: &str) -> (tempfile::TempDir, JobSpec) {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        let (program, flag, environment) = (PathBuf::from("/bin/sh"), "-c", vec![]);
        // Windows programs need SystemRoot to start; the runner passes nothing it was not given.
        #[cfg(windows)]
        let (program, flag, environment) = (
            PathBuf::from(r"C:\Windows\System32\cmd.exe"),
            "/C",
            vec![(
                OsString::from("SystemRoot"),
                std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into()),
            )],
        );
        let spec = JobSpec {
            program,
            arguments: vec![flag.into(), script.into()],
            working_directory: dir.path().to_owned(),
            environment,
            timeout: Duration::from_secs(3),
            capture_limit_bytes: 1024,
            artifact_roots: vec![],
            resource_limits: ResourceLimits::default(),
        };
        (dir, spec)
    }
    /// A script that runs for about 30 seconds, with a child of its own holding the pipes.
    #[cfg(unix)]
    const LONG_SCRIPT: &str = "/bin/sleep 30 & wait";
    #[cfg(windows)]
    const LONG_SCRIPT: &str = r"C:\Windows\System32\PING.EXE -n 30 127.0.0.1 >nul";
    /// A script that runs for about a third of a second.
    #[cfg(unix)]
    const BRIEF_SCRIPT: &str = "sleep 0.3";
    #[cfg(windows)]
    const BRIEF_SCRIPT: &str = r"C:\Windows\System32\PING.EXE -n 2 127.0.0.1 >nul";

    // Verifies: NUC-004, PLAT-004, PLAT-005
    #[test]
    fn preserves_exit_code_and_does_not_infer_a_scientific_verdict() {
        #[cfg(unix)]
        let script = "printf 'raw tally'; printf 'solver note' >&2; exit 7";
        #[cfg(windows)]
        let script = "echo raw tally& echo solver note 1>&2& exit 7";
        let (_dir, spec) = shell(script);
        let result = run_job(&spec, &Cancellation::default()).unwrap();
        assert_eq!(result.execution_status, ExecutionStatus::Failed);
        assert_eq!(result.exit_code, Some(7));
        assert_eq!(result.stdout.trim_end(), "raw tally");
        assert_eq!(result.stderr.trim_end(), "solver note");
    }
    // Verifies: AUTO-040, REL-006, PLAT-004, PLAT-005
    #[test]
    fn timeout_cleans_up_descendants_holding_the_output_pipe() {
        let (_dir, mut spec) = shell(LONG_SCRIPT);
        spec.timeout = Duration::from_millis(80);
        let result = run_job(&spec, &Cancellation::default()).unwrap();
        assert_eq!(result.execution_status, ExecutionStatus::TimedOut);
        assert!(result.elapsed_seconds < 3.0);
    }
    // Verifies: AUTO-040, PLAT-004, PLAT-005
    #[test]
    fn cancellation_terminates_a_running_job() {
        let (_dir, spec) = shell(LONG_SCRIPT);
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
    // Verifies: AUTO-040, SEC-063, PLAT-004, PLAT-005
    #[test]
    fn oversized_output_is_bounded_even_when_a_process_exits_quickly() {
        #[cfg(unix)]
        let script = "/usr/bin/head -c 4096 /dev/zero".to_string();
        #[cfg(windows)]
        let script = format!("echo {}", "x".repeat(300));
        let (_dir, mut spec) = shell(&script);
        spec.capture_limit_bytes = 64;
        let result = run_job(&spec, &Cancellation::default()).unwrap();
        assert_eq!(result.execution_status, ExecutionStatus::OutputLimit);
        assert_eq!(result.stdout.len(), 64);
        assert!(result.stdout_truncated);
    }
    #[cfg(target_os = "linux")]
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
        let (dir, spec) = shell("echo x > should-not-exist");
        let cancellation = Cancellation::default();
        cancellation.cancel();
        assert_eq!(
            run_job(&spec, &cancellation).unwrap().execution_status,
            ExecutionStatus::Cancelled
        );
        assert!(!dir.path().join("should-not-exist").exists());
    }

    // Verifies: AUTO-040, SEC-061
    #[cfg(target_os = "linux")]
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
    fn only_the_address_space_may_be_raised_and_only_to_the_ceiling() {
        assert!(ResourceLimits::DEFAULT.valid());
        assert!(ResourceLimits::with_address_space(12 << 30).valid());
        assert!(ResourceLimits::with_address_space(ResourceLimits::ADDRESS_SPACE_CEILING).valid());
        assert!(
            !ResourceLimits::with_address_space(ResourceLimits::ADDRESS_SPACE_CEILING + 1).valid()
        );
        let mut more_files = ResourceLimits::DEFAULT;
        more_files.artifact_file_count += 1;
        assert!(!more_files.valid());
    }

    // Verifies: AUTO-040, SEC-061
    #[cfg(unix)]
    #[test]
    fn oversized_single_file_is_killed_by_inherited_file_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let mut spec = shell("true").1;
        #[cfg(target_os = "linux")]
        let (program, quiet) = ("/usr/bin/dd", Some("status=none"));
        #[cfg(target_os = "macos")]
        let (program, quiet) = ("/bin/dd", None);
        spec.program = PathBuf::from(program);
        spec.arguments = ["if=/dev/zero", "of=oversized.bin", "bs=4096", "count=32"]
            .into_iter()
            .chain(quiet)
            .map(OsString::from)
            .collect();
        spec.working_directory = dir.path().to_owned();
        spec.resource_limits.single_file_bytes = 64 * 1024;
        let result = run_job(&spec, &Cancellation::default()).unwrap();
        assert_eq!(result.execution_status, ExecutionStatus::FileSizeLimit);
        // The kernel stops the write at the limit; the macOS limit is whole 1024-byte blocks,
        // rounded down, so it is at most the limit and here (a multiple of 1024) exactly it.
        let written = std::fs::metadata(dir.path().join("oversized.bin"))
            .unwrap()
            .len();
        assert_eq!(written, 64 * 1024);
        assert_eq!(result.artifact_files_observed, 1);
        assert_eq!(result.artifact_bytes_observed, 64 * 1024);
    }

    // Verifies: AUTO-040, PLAT-004
    #[cfg(windows)]
    #[test]
    fn oversized_single_file_is_caught_by_the_artifact_scan() {
        // Windows has no kernel file-size limit: the scan stops the job after the write.
        let (dir, mut spec) = shell(&format!(
            "for /L %i in (1,1,300) do @echo {}>>big.txt",
            "x".repeat(100)
        ));
        spec.resource_limits.single_file_bytes = 8 * 1024;
        let result = run_job(&spec, &Cancellation::default()).unwrap();
        assert_eq!(result.execution_status, ExecutionStatus::FileSizeLimit);
        assert!(dir.path().join("big.txt").is_file());
    }

    #[test]
    fn the_macos_file_size_limit_is_whole_blocks_rounded_down() {
        assert_eq!(
            macos_shell_script(64 * 1024),
            "ulimit -f 64; exec \"$0\" \"$@\""
        );
        assert_eq!(
            macos_shell_script(64 * 1024 + 1023),
            "ulimit -f 64; exec \"$0\" \"$@\""
        );
        assert_eq!(
            macos_shell_script(10),
            "ulimit -f 1; exec \"$0\" \"$@\"",
            "never zero blocks, which would forbid every write"
        );
    }

    #[test]
    fn the_memory_limit_status_has_a_stable_name() {
        assert_eq!(
            serde_json::to_string(&ExecutionStatus::MemoryLimit).unwrap(),
            "\"MEMORY_LIMIT\""
        );
    }

    // Verifies: AUTO-040
    #[cfg(unix)]
    #[test]
    fn aggregate_artifact_bytes_are_checked_even_for_short_jobs() {
        let (dir, mut spec) = shell(
            "dd if=/dev/zero of=a bs=4096 count=8 2>/dev/null; dd if=/dev/zero of=b bs=4096 count=8 2>/dev/null",
        );
        spec.resource_limits.artifact_total_bytes = 48 * 1024;
        let result = run_job(&spec, &Cancellation::default()).unwrap();
        assert_eq!(result.execution_status, ExecutionStatus::ArtifactLimit);
        assert!(result.artifact_bytes_observed > result.resource_limits.artifact_total_bytes);
        assert_eq!(result.artifact_files_observed, 2);
        assert!(dir.path().join("a").exists());
        assert!(dir.path().join("b").exists());
    }

    // Verifies: AUTO-040
    #[cfg(unix)]
    #[test]
    fn files_already_in_the_roots_do_not_count_against_the_total() {
        // 64 KiB left by an earlier step, over a 48 KiB cap: the job may still add 32 KiB, and
        // is stopped when it adds more than the cap.
        let (dir, mut spec) = shell("dd if=/dev/zero of=new bs=4096 count=8 2>/dev/null");
        std::fs::write(dir.path().join("input"), vec![0_u8; 64 * 1024]).unwrap();
        spec.resource_limits.artifact_total_bytes = 48 * 1024;
        let result = run_job(&spec, &Cancellation::default()).unwrap();
        assert_eq!(result.execution_status, ExecutionStatus::Succeeded);
        let (dir, mut spec) = shell("dd if=/dev/zero of=new bs=4096 count=16 2>/dev/null");
        std::fs::write(dir.path().join("input"), vec![0_u8; 64 * 1024]).unwrap();
        spec.resource_limits.artifact_total_bytes = 48 * 1024;
        let result = run_job(&spec, &Cancellation::default()).unwrap();
        assert_eq!(result.execution_status, ExecutionStatus::ArtifactLimit);
    }

    // Verifies: PLAT-004, PLAT-005
    #[test]
    fn aggregate_artifact_file_count_includes_explicit_external_roots() {
        // The runner creates the explicit root `extra` before the job starts.
        #[cfg(unix)]
        let script = "touch a b; touch extra/c extra/d";
        #[cfg(windows)]
        let script = "echo.>a& echo.>b& echo.>extra\\c& echo.>extra\\d";
        let (dir, mut spec) = shell(script);
        let extra = dir.path().join("extra");
        spec.artifact_roots = vec![extra];
        spec.resource_limits.artifact_file_count = 3;
        let result = run_job(&spec, &Cancellation::default()).unwrap();
        assert_eq!(result.execution_status, ExecutionStatus::ArtifactLimit);
        assert_eq!(result.artifact_files_observed, 4);
    }

    #[cfg(unix)]
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

    fn collect_lines(spec: &JobSpec, cancellation: &Cancellation) -> (JobResult, Vec<String>) {
        let lines = std::sync::Mutex::new(Vec::new());
        let result = run_job_with_lines(spec, cancellation, &|line| {
            lines.lock().unwrap().push(line.to_owned())
        })
        .unwrap();
        (result, lines.into_inner().unwrap())
    }

    #[cfg(unix)]
    #[test]
    fn stdout_lines_arrive_in_order_before_the_job_ends() {
        let (_dir, spec) = shell("echo first; sleep 1; echo second");
        let seen = std::sync::Mutex::new(Vec::new());
        let result = run_job_with_lines(&spec, &Cancellation::default(), &|line| {
            seen.lock().unwrap().push((line.to_owned(), Instant::now()));
        })
        .unwrap();
        let finished = Instant::now();
        assert_eq!(result.execution_status, ExecutionStatus::Succeeded);
        let seen = seen.into_inner().unwrap();
        let names: Vec<_> = seen.iter().map(|(line, _)| line.as_str()).collect();
        assert_eq!(names, ["first", "second"]);
        assert!(finished.duration_since(seen[0].1) >= Duration::from_millis(500));
    }

    #[cfg(unix)]
    #[test]
    fn unterminated_final_line_is_delivered() {
        let (_dir, spec) = shell("printf 'one\\ntwo'");
        let (result, lines) = collect_lines(&spec, &Cancellation::default());
        assert_eq!(result.stdout, "one\ntwo");
        assert_eq!(lines, ["one", "two"]);
    }

    #[cfg(unix)]
    #[test]
    fn stdout_is_identical_with_and_without_a_callback() {
        let (_dir, spec) = shell("printf 'a\\n\\nb\\nc'; printf 'warn' >&2");
        let plain = run_job(&spec, &Cancellation::default()).unwrap();
        let (with, lines) = collect_lines(&spec, &Cancellation::default());
        assert_eq!(plain.stdout, with.stdout);
        assert_eq!(plain.stderr, with.stderr);
        assert_eq!(plain.execution_status, with.execution_status);
        assert_eq!(lines, ["a", "", "b", "c"]);
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_works_while_lines_are_flowing() {
        let (_dir, mut spec) = shell("while true; do echo tick; sleep 0.02; done");
        spec.capture_limit_bytes = MAX_CAPTURE_BYTES;
        let cancellation = Cancellation::default();
        let remote = cancellation.clone();
        let worker = thread::spawn(move || collect_lines(&spec, &remote));
        thread::sleep(Duration::from_millis(300));
        cancellation.cancel();
        let (result, lines) = worker.join().unwrap();
        assert_eq!(result.execution_status, ExecutionStatus::Cancelled);
        assert!(lines.len() >= 2 && lines.iter().all(|line| line == "tick"));
    }

    #[cfg(unix)]
    #[test]
    fn over_long_line_is_truncated_for_the_callback() {
        let (_dir, mut spec) = shell("head -c 100000 /dev/zero | tr '\\0' x; echo; echo after");
        spec.capture_limit_bytes = 200 * 1024;
        let (result, lines) = collect_lines(&spec, &Cancellation::default());
        assert_eq!(result.execution_status, ExecutionStatus::Succeeded);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), MAX_LINE_BYTES);
        assert_eq!(lines[1], "after");
    }

    // Verifies: PERF-041, PLAT-004, PLAT-005
    #[test]
    fn a_finished_job_records_its_peak_memory_and_the_check_made_before_it() {
        // A third of a second spans several 50 ms samples of the running job.
        let (_dir, spec) = shell(BRIEF_SCRIPT);
        let result = run_job(&spec, &Cancellation::default()).unwrap();
        assert_eq!(result.execution_status, ExecutionStatus::Succeeded);
        let peak = result
            .peak_memory_bytes
            .expect("peak memory is measured on Linux, macOS and Windows");
        assert!(peak > 0 && peak < 1 << 30, "implausible peak {peak}");
        assert!(result.memory_check.unwrap().starts_with("PASS: "));
    }

    // Verifies: PERF-041
    #[test]
    fn a_job_projected_beyond_free_memory_is_refused_before_it_starts() {
        let (dir, spec) = shell("echo started > ran");
        crate::memory::tests::inject_free(Some(crate::memory::GIB));
        let outcome = run_job(&spec, &Cancellation::default());
        crate::memory::tests::inject_free(None);
        let Err(JobError::MemoryRefused(why)) = outcome else {
            panic!("expected a refusal, got {outcome:?}");
        };
        assert!(
            why.starts_with("not started: ") && why.contains("4.0 GiB"),
            "{why}"
        );
        assert!(
            !dir.path().join("ran").exists(),
            "the job must not have started"
        );
    }

    // Verifies: PERF-041, PLAT-005
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_stops_a_job_whose_memory_passes_the_cap() {
        // awk builds a 128 MiB string, then sleeps while still holding it.
        let (_dir, mut spec) = shell("true");
        spec.program = PathBuf::from("/usr/bin/awk");
        spec.arguments = vec![
            "BEGIN { s = \"x\"; for (i = 0; i < 27; i++) s = s s; system(\"sleep 20\") }".into(),
        ];
        spec.timeout = Duration::from_secs(15);
        spec.resource_limits = ResourceLimits::with_address_space(64 << 20);
        let result = run_job(&spec, &Cancellation::default()).unwrap();
        assert_eq!(result.execution_status, ExecutionStatus::MemoryLimit);
        assert!(result.elapsed_seconds < 10.0);
        assert!(result.peak_memory_bytes.is_some_and(|peak| peak > 64 << 20));
    }

    // Verifies: PLAT-004
    #[cfg(windows)]
    #[test]
    fn windows_lines_arrive_without_the_carriage_return() {
        let (_dir, spec) = shell("echo one& echo two");
        let (result, lines) = collect_lines(&spec, &Cancellation::default());
        assert_eq!(result.execution_status, ExecutionStatus::Succeeded);
        assert_eq!(lines, ["one", "two"]);
    }
}
