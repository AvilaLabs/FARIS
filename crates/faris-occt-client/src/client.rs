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
        let mut child = Command::new(&self.program)
            .arg(&request_path)
            .arg(&response_path)
            .stdin(Stdio::null())
            .stdout(Stdio::from(
                stderr_file
                    .try_clone()
                    .map_err(|e| WorkerError::Spawn(e.to_string()))?,
            ))
            .stderr(Stdio::from(stderr_file))
            .spawn()
            .map_err(|e| WorkerError::Spawn(format!("{}: {e}", self.program.display())))?;
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(e) => return Err(WorkerError::Spawn(e.to_string())),
            }
            if cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                return Err(WorkerError::Cancelled);
            }
            if started.elapsed().as_secs_f64() > bounds.wall_seconds {
                let _ = child.kill();
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
