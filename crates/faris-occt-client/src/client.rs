// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! Runs requests in `faris-occt-worker`, a separate process with a wall-clock limit enforced
//! here and address-space and CPU limits the worker sets on itself. OpenCASCADE can hang or
//! crash on a bad file; this is what keeps that out of FARIS.

use crate::protocol::{Limits, Operation, Outcome, Pong, Request, Response};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Limits on one worker run.
#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    /// Wall-clock seconds before the worker is killed.
    pub wall_seconds: f64,
    pub limits: Limits,
}

impl Default for Bounds {
    fn default() -> Self {
        Self {
            wall_seconds: 600.0,
            limits: Limits {
                address_space_mb: Some(8192),
                cpu_seconds: Some(3600),
            },
        }
    }
}

#[derive(Debug)]
pub enum WorkerError {
    /// The worker program could not be started.
    Spawn(String),
    /// Stopped because the caller cancelled.
    Cancelled,
    /// Killed after the wall-clock limit.
    TimedOut { seconds: f64 },
    /// Ended without writing a response: crash, signal, or exit from a limit.
    Crashed { status: String, stderr_tail: String },
    /// The worker ran and reported an error (unreadable file, OpenCASCADE exception).
    Failed(String),
    /// The response file was missing or malformed.
    BadResponse(String),
}

impl std::fmt::Display for WorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(m) => write!(f, "could not start the CAD worker: {m}"),
            Self::Cancelled => write!(f, "the CAD worker was stopped: the check was cancelled"),
            Self::TimedOut { seconds } => {
                write!(f, "the CAD worker was stopped after {seconds:.0} s")
            }
            Self::Crashed {
                status,
                stderr_tail,
            } => {
                write!(
                    f,
                    "the CAD worker ended abnormally ({status}): {stderr_tail}"
                )
            }
            Self::Failed(m) => write!(f, "{m}"),
            Self::BadResponse(m) => write!(f, "the CAD worker's response could not be read: {m}"),
        }
    }
}

impl std::error::Error for WorkerError {}

/// A worker program and where it keeps its request and response files.
#[derive(Clone, Debug)]
pub struct Worker {
    program: PathBuf,
}

impl Worker {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
        }
    }

    /// `FARIS_OCCT_WORKER`, else `faris-occt-worker` beside the running program.
    pub fn locate() -> Option<Self> {
        Self::locate_with(std::env::var_os("FARIS_OCCT_WORKER").map(PathBuf::from))
    }

    /// An explicit path wins; else `FARIS_OCCT_WORKER`; else `faris-occt-worker` beside the
    /// running program. An explicit path or variable is returned as given, even when no file
    /// is there, so the caller can say which path it tried.
    pub fn locate_with(explicit: Option<PathBuf>) -> Option<Self> {
        if let Some(path) = explicit {
            return Some(Self::new(path));
        }
        if let Some(path) = std::env::var_os("FARIS_OCCT_WORKER") {
            return Some(Self::new(path));
        }
        Self::beside_current_exe()
    }

    /// `faris-occt-worker` in the directory of the running program, when it is a file.
    pub fn beside_current_exe() -> Option<Self> {
        let beside = std::env::current_exe()
            .ok()?
            .parent()?
            .join("faris-occt-worker");
        beside.is_file().then(|| Self::new(beside))
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Asks the worker who it is. A worker that answers can be started, reads its request and
    /// writes a response.
    pub fn ping(&self) -> Result<Pong, WorkerError> {
        let value = self.run(
            Operation::Ping,
            Bounds {
                wall_seconds: 60.0,
                ..Bounds::default()
            },
        )?;
        serde_json::from_value(value).map_err(|e| WorkerError::BadResponse(format!("ping: {e}")))
    }

    /// Runs one operation to completion or to a limit.
    pub fn run(
        &self,
        operation: Operation,
        bounds: Bounds,
    ) -> Result<serde_json::Value, WorkerError> {
        self.run_cancellable(operation, bounds, &|| false)
    }

    /// As [`Worker::run`], and the worker is killed when `cancelled` returns true (polled every
    /// 10 ms while it runs).
    pub fn run_cancellable(
        &self,
        operation: Operation,
        bounds: Bounds,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<serde_json::Value, WorkerError> {
        let scratch = tempfile::tempdir().map_err(|e| WorkerError::Spawn(e.to_string()))?;
        let request_path = scratch.path().join("request.json");
        let response_path = scratch.path().join("response.json");
        let request = Request::new(operation, bounds.limits);
        fs::write(
            &request_path,
            serde_json::to_vec(&request).map_err(|e| WorkerError::Spawn(e.to_string()))?,
        )
        .map_err(|e| WorkerError::Spawn(e.to_string()))?;
        let stderr_path = scratch.path().join("stderr.txt");
        let stderr_file =
            fs::File::create(&stderr_path).map_err(|e| WorkerError::Spawn(e.to_string()))?;
        let mut command = Command::new(&self.program);
        command
            .arg(&request_path)
            .arg(&response_path)
            .stdin(Stdio::null())
            .stdout(Stdio::from(
                stderr_file
                    .try_clone()
                    .map_err(|e| WorkerError::Spawn(e.to_string()))?,
            ))
            .stderr(Stdio::from(stderr_file));
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // The native worker may launch helper processes. Give it an owned process group so
            // cancellation and timeout stop the complete operation, not just its leader.
            command.process_group(0);
        }
        let mut child = spawn_worker(command)
            .map_err(|e| WorkerError::Spawn(format!("{}: {e}", self.program.display())))?;
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(e) => {
                    terminate_worker(&mut child);
                    let _ = child.wait();
                    return Err(WorkerError::Spawn(e.to_string()));
                }
            }
            if cancelled() {
                terminate_worker(&mut child);
                let _ = child.wait();
                return Err(WorkerError::Cancelled);
            }
            if started.elapsed().as_secs_f64() > bounds.wall_seconds {
                terminate_worker(&mut child);
                let _ = child.wait();
                return Err(WorkerError::TimedOut {
                    seconds: bounds.wall_seconds,
                });
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let tail = tail_of(&stderr_path);
        let Ok(bytes) = fs::read(&response_path) else {
            return Err(WorkerError::Crashed {
                status: describe(&status),
                stderr_tail: tail,
            });
        };
        let response: Response =
            serde_json::from_slice(&bytes).map_err(|e| WorkerError::BadResponse(e.to_string()))?;
        match response.outcome {
            Outcome::Ok { result } => Ok(result),
            Outcome::Error { message } => Err(WorkerError::Failed(message)),
        }
    }
}

#[cfg(not(windows))]
type WorkerChild = std::process::Child;

#[cfg(windows)]
type WorkerChild = Box<dyn process_wrap::std::ChildWrapper>;

#[cfg(not(windows))]
fn spawn_worker(mut command: Command) -> std::io::Result<WorkerChild> {
    command.spawn()
}

#[cfg(windows)]
fn spawn_worker(command: Command) -> std::io::Result<WorkerChild> {
    use process_wrap::std::{CommandWrap, JobObject};
    let mut wrapped = CommandWrap::from(command);
    wrapped.wrap(JobObject).wrap(KillJobOnDrop);
    wrapped.spawn()
}

#[cfg(unix)]
fn terminate_worker(child: &mut WorkerChild) {
    use nix::{
        errno::Errno,
        sys::signal::{Signal, killpg},
        unistd::Pid,
    };
    if let Ok(pid) = i32::try_from(child.id()) {
        match killpg(Pid::from_raw(pid), Signal::SIGKILL) {
            Ok(()) | Err(Errno::ESRCH) => return,
            Err(_) => {}
        }
    }
    let _ = child.kill();
}

#[cfg(windows)]
fn terminate_worker(child: &mut WorkerChild) {
    let _ = child.start_kill();
}

#[cfg(not(any(unix, windows)))]
fn terminate_worker(child: &mut WorkerChild) {
    let _ = child.kill();
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug)]
struct KillJobOnDrop;

#[cfg(windows)]
impl process_wrap::std::CommandWrapper for KillJobOnDrop {
    fn wrap_child(
        &mut self,
        child: Box<dyn process_wrap::std::ChildWrapper>,
        _core: &process_wrap::std::CommandWrap,
    ) -> std::io::Result<Box<dyn process_wrap::std::ChildWrapper>> {
        Ok(Box::new(KillJobOnDropChild { inner: Some(child) }))
    }
}

#[cfg(windows)]
#[derive(Debug)]
struct KillJobOnDropChild {
    inner: Option<Box<dyn process_wrap::std::ChildWrapper>>,
}

#[cfg(windows)]
impl process_wrap::std::ChildWrapper for KillJobOnDropChild {
    fn inner(&self) -> &dyn process_wrap::std::ChildWrapper {
        self.inner.as_deref().expect("child wrapper is present")
    }

    fn inner_mut(&mut self) -> &mut dyn process_wrap::std::ChildWrapper {
        self.inner.as_deref_mut().expect("child wrapper is present")
    }

    fn into_inner(mut self: Box<Self>) -> Box<dyn process_wrap::std::ChildWrapper> {
        self.inner.take().expect("child wrapper is present")
    }

    fn process_handle(&self) -> Option<std::os::windows::io::BorrowedHandle<'_>> {
        self.inner.as_deref()?.process_handle()
    }
}

#[cfg(windows)]
impl Drop for KillJobOnDropChild {
    fn drop(&mut self) {
        if let Some(child) = self.inner.as_deref_mut() {
            let _ = child.kill();
        }
    }
}

fn describe(status: &std::process::ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("signal {signal}");
        }
    }
    status.to_string()
}

fn tail_of(path: &Path) -> String {
    let mut text = String::new();
    if let Ok(mut file) = fs::File::open(path) {
        let _ = file.by_ref().take(1 << 20).read_to_string(&mut text);
    }
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(8)..].join("\n")
}

// This client has a separate process-launch path from
// faris-engine's shared job runner, so exercise its cancellation boundary too.
#[cfg(all(test, target_os = "linux"))]
mod cancellation_tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    // Verifies: PERF-024
    #[test]
    fn cancelling_a_native_worker_stops_and_reaps_its_process() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("worker.pid");
        let script_path = dir.path().join("worker.sh");
        let quoted_pid_file = pid_file.to_string_lossy().replace('\'', "'\\''");
        std::fs::write(
            &script_path,
            format!("#!/bin/sh\nprintf '%s' \"$$\" > '{quoted_pid_file}'\nexec sleep 30\n"),
        )
        .unwrap();
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let worker = Worker::new(script_path);
        let running = std::thread::spawn(move || {
            worker.run_cancellable(Operation::Ping, Bounds::default(), &|| {
                worker_cancelled.load(Ordering::Acquire)
            })
        });

        let start_deadline = Instant::now() + Duration::from_secs(2);
        let pid: u32 = loop {
            if let Ok(contents) = fs::read_to_string(&pid_file)
                && let Ok(pid) = contents.trim().parse()
            {
                break pid;
            }
            assert!(Instant::now() < start_deadline, "CAD worker did not start");
            std::thread::sleep(Duration::from_millis(10));
        };
        let proc_stat = PathBuf::from(format!("/proc/{pid}/stat"));
        let cancelled_at = Instant::now();
        cancelled.store(true, Ordering::Release);
        assert!(matches!(
            running.join().unwrap(),
            Err(WorkerError::Cancelled)
        ));
        assert!(cancelled_at.elapsed() < Duration::from_secs(2));
        assert!(
            !proc_stat.exists(),
            "CAD worker process {pid} remained after cancellation and wait"
        );
    }

    // Verifies: PERF-024. The native CAD client bypasses faris-engine's job runner, so its
    // process-tree cancellation must be checked independently.
    #[test]
    fn cancelling_a_native_worker_kills_and_reaps_its_descendant() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("descendant.pid");
        let script_path = dir.path().join("worker.sh");
        let quoted_pid_file = pid_file.to_string_lossy().replace('\'', "'\\''");
        std::fs::write(
            &script_path,
            format!("#!/bin/sh\nsleep 30 &\nprintf '%s' \"$!\" > '{quoted_pid_file}'\nwait\n"),
        )
        .unwrap();
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let worker = Worker::new(script_path);
        let running = std::thread::spawn(move || {
            worker.run_cancellable(Operation::Ping, Bounds::default(), &|| {
                worker_cancelled.load(Ordering::Acquire)
            })
        });

        let start_deadline = Instant::now() + Duration::from_secs(2);
        let pid: u32 = loop {
            if let Ok(contents) = fs::read_to_string(&pid_file)
                && let Ok(pid) = contents.trim().parse()
            {
                break pid;
            }
            assert!(
                Instant::now() < start_deadline,
                "native worker descendant did not start"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        let proc_stat = PathBuf::from(format!("/proc/{pid}/stat"));
        let cancelled_at = Instant::now();
        cancelled.store(true, Ordering::Release);
        assert!(matches!(
            running.join().unwrap(),
            Err(WorkerError::Cancelled)
        ));
        assert!(cancelled_at.elapsed() < Duration::from_secs(2));

        let cleanup_deadline = Instant::now() + Duration::from_secs(2);
        while proc_stat.exists() && Instant::now() < cleanup_deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !proc_stat.exists(),
            "native worker descendant {pid} remained after cancellation and wait"
        );
    }
}

#[cfg(all(test, windows))]
mod windows_cancellation_tests {
    use super::*;

    // Isolated helper launched by the regression below with an exact test filter.
    #[test]
    fn grandchild_helper() {
        if std::env::var_os("FARIS_OCCT_WINDOWS_HELPER").as_deref()
            != Some(std::ffi::OsStr::new("1"))
        {
            return;
        }
        let pid_file = PathBuf::from(
            std::env::var_os("FARIS_OCCT_WINDOWS_PID_FILE")
                .expect("helper PID path is set by its parent"),
        );
        let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        let ping = PathBuf::from(system_root).join("System32").join("ping.exe");
        let mut child = Command::new(ping)
            .args(["-n", "30", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start ping grandchild");
        std::fs::write(&pid_file, child.id().to_string()).expect("publish grandchild PID");
        let _ = child.wait();
    }

    // Verifies: PERF-024
    #[test]
    fn cancelling_the_windows_native_worker_kills_its_descendant() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("grandchild.pid");
        let test_binary = std::env::current_exe().unwrap();
        let mut command = Command::new(test_binary);
        command
            .args([
                "--exact",
                "client::windows_cancellation_tests::grandchild_helper",
                "--nocapture",
            ])
            .env("FARIS_OCCT_WINDOWS_HELPER", "1")
            .env("FARIS_OCCT_WINDOWS_PID_FILE", &pid_file)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = spawn_worker(command).expect("start isolated helper in a job object");

        let startup_deadline = Instant::now() + Duration::from_secs(20);
        let pid: u32 = loop {
            if let Ok(contents) = fs::read_to_string(&pid_file)
                && let Ok(pid) = contents.trim().parse()
            {
                break pid;
            }
            assert!(
                Instant::now() < startup_deadline,
                "grandchild did not start"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        let cancelled_at = Instant::now();
        terminate_worker(&mut child);
        let _ = child.wait();
        assert!(cancelled_at.elapsed() < Duration::from_secs(2));

        let pid_text = pid.to_string();
        let cleanup_deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let listing = Command::new("tasklist.exe")
                .args(["/FI", &format!("PID eq {pid_text}"), "/FO", "CSV", "/NH"])
                .output()
                .expect("query grandchild process");
            let output = String::from_utf8_lossy(&listing.stdout);
            if listing.status.success() && !output.contains(&format!(r#""{pid_text}""#)) {
                break;
            }
            assert!(
                Instant::now() < cleanup_deadline,
                "grandchild {pid} remained after job cancellation: {output}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
