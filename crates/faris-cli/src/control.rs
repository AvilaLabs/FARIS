use crate::transport::{read_bounded, sha256, write_new_bytes, write_new_json};
use clap::Subcommand;
use faris_engine::jobs::{Cancellation, ExecutionStatus, JobSpec, run_job};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Subcommand)]
pub enum ControlCommand {
    /// Synthetic one-group pure absorber; mathematical control, not reactor physics.
    Absorber {
        /// Python interpreter from the chosen OpenMC environment.
        #[arg(long)]
        python: PathBuf,
        /// OpenMC executable from that same pinned environment.
        #[arg(long)]
        openmc: PathBuf,
        /// New evidence directory; existing paths are refused.
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(30..=100000))]
        batches: u32,
        #[arg(long, default_value_t = 10000, value_parser = clap::value_parser!(u32).range(1..=1000000))]
        particles: u32,
        #[arg(long, default_value_t = 123456789, value_parser = clap::value_parser!(u64).range(1..=i64::MAX as u64))]
        seed: u64,
        #[arg(long, default_value_t = 600, value_parser = clap::value_parser!(u64).range(1..=86400))]
        timeout_seconds: u64,
    },
}

fn hash_file(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    const MAX_EXECUTABLE_BYTES: u64 = 512 * 1024 * 1024;
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() > MAX_EXECUTABLE_BYTES {
        return Err("tool must be a regular file no larger than 512 MiB".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err("tool file must be executable".into());
        }
    }
    let mut file = File::open(path)?;
    let mut total = 0u64;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_EXECUTABLE_BYTES {
            return Err("tool exceeds the 512 MiB limit".into());
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(unix)]
pub(crate) struct InterruptGuard {
    pub(crate) cancellation: Cancellation,
    registrations: Vec<signal_hook::SigId>,
}

#[cfg(unix)]
impl Drop for InterruptGuard {
    fn drop(&mut self) {
        for registration in self.registrations.drain(..) {
            signal_hook::low_level::unregister(registration);
        }
    }
}

#[cfg(unix)]
pub(crate) fn interrupt_cancellation() -> Result<InterruptGuard, Box<dyn std::error::Error>> {
    use signal_hook::consts::{SIGINT, SIGTERM};
    let cancellation = Cancellation::default();
    let mut guard = InterruptGuard {
        cancellation,
        registrations: vec![],
    };
    for signal in [SIGINT, SIGTERM] {
        guard.registrations.push(signal_hook::flag::register(
            signal,
            guard.cancellation.shared_flag(),
        )?);
    }
    Ok(guard)
}

#[cfg(not(unix))]
fn interrupt_cancellation() -> Result<Cancellation, Box<dyn std::error::Error>> {
    Err("external controls require the Unix process-group adapter".into())
}

pub fn run(command: ControlCommand) -> Result<(), Box<dyn std::error::Error>> {
    let ControlCommand::Absorber {
        python,
        openmc,
        output,
        batches,
        particles,
        seed,
        timeout_seconds,
    } = command;
    let histories = u64::from(batches) * u64::from(particles);
    if histories > 10_000_000 {
        return Err("control histories exceed the 10 million work limit".into());
    }
    if output.exists() {
        return Err("control output already exists; choose a new evidence path".into());
    }
    let python = python.canonicalize()?;
    let openmc = openmc.canonicalize()?;
    // Execute the reviewed worker bundled with this Rust build. Copying it into
    // the run also preserves the exact source used by the control.
    const WORKER: &[u8] = include_bytes!("../../../controls/pure_absorber_sphere.py");
    let cwd = std::env::current_dir()?.canonicalize()?;
    let output = std::path::absolute(output)?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let solver_dir = output.join("solver");
    let script = output.join("pure_absorber_sphere.py");
    let script_sha256 = sha256(WORKER);
    let python_sha256 = hash_file(&python)?;
    let openmc_sha256 = hash_file(&openmc)?;
    let arguments: Vec<OsString> = vec![
        script.as_os_str().into(),
        "--run".into(),
        "--openmc-executable".into(),
        openmc.as_os_str().into(),
        "--out".into(),
        solver_dir.as_os_str().into(),
        "--batches".into(),
        batches.to_string().into(),
        "--particles".into(),
        particles.to_string().into(),
        "--seed".into(),
        seed.to_string().into(),
        "--threads".into(),
        "1".into(),
    ];
    let executable_dir = python
        .parent()
        .ok_or("Python executable has no parent directory")?;
    let path = std::env::join_paths([executable_dir, Path::new("/usr/bin"), Path::new("/bin")])?;
    let user_home =
        std::env::var_os("HOME").ok_or("OpenMPI requires an explicit user home directory")?;
    let spec = JobSpec {
        program: python.clone(),
        arguments,
        working_directory: cwd,
        environment: vec![
            ("PATH".into(), path),
            ("OMP_NUM_THREADS".into(), "1".into()),
            ("OPENBLAS_NUM_THREADS".into(), "1".into()),
            ("PYTHONDONTWRITEBYTECODE".into(), "1".into()),
            ("HOME".into(), user_home),
        ],
        timeout: Duration::from_secs(timeout_seconds),
        capture_limit_bytes: 1024 * 1024,
    };
    let started_unix_seconds = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let interrupts = interrupt_cancellation()?;
    // Preflight is complete. Reserve the root; Python creates its subdirectory.
    std::fs::create_dir(&output)?;
    write_new_bytes(&script, WORKER)?;
    write_new_json(
        &output.join("request.json"),
        &serde_json::json!({
            "schema_version": "faris-control-request/v0.1",
            "faris_version": env!("CARGO_PKG_VERSION"),
            "script_sha256": script_sha256, "python_sha256": python_sha256,
            "openmc_sha256": openmc_sha256,
            "batches": batches, "particles_per_batch": particles, "seed": seed,
            "threads": 1, "timeout_seconds": timeout_seconds,
            "scientific_scope": "Synthetic numerical control only"
        }),
    )?;
    eprintln!(
        "Running mathematical absorber control ({histories} histories, {timeout_seconds}s limit). Ctrl-C cancels the solver group."
    );
    #[cfg(unix)]
    let execution_attempt = run_job(&spec, &interrupts.cancellation);
    #[cfg(not(unix))]
    let execution_attempt = run_job(&spec, &interrupts);
    let execution = match execution_attempt {
        Ok(execution) => execution,
        Err(error) => {
            write_new_json(
                &output.join("execution.json"),
                &serde_json::json!({
                    "schema_version": "faris-control-execution/v0.1", "execution_status": "FAILED",
                    "error": error.to_string(), "script_sha256": script_sha256,
                    "python_sha256": python_sha256, "openmc_sha256": openmc_sha256,
                    "physical_qualification": "NOT_EVALUATED"
                }),
            )?;
            return Err(error.into());
        }
    };
    let result_path = solver_dir.join("control-result.json");
    let result_bytes = if result_path.is_file() {
        Some(read_bounded(&result_path))
    } else {
        None
    };
    let result_sha256 = result_bytes
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .map(|b| sha256(b));
    let (control, artifact_error) = match result_bytes {
        Some(Ok(bytes)) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(value) => (Some(value), None),
            Err(error) => (None, Some(error.to_string())),
        },
        Some(Err(error)) => (None, Some(error.to_string())),
        None => (None, Some("no control-result.json produced".into())),
    };
    let binding_verified = control.as_ref().is_some_and(|value| {
        value["script_identity"]["sha256"].as_str() == Some(script_sha256.as_str())
            && value["openmc_executable"]["sha256"].as_str() == Some(openmc_sha256.as_str())
            && value["solver_status"].as_str() == Some("COMPLETED")
            && value["execution"]["seed"].as_u64() == Some(seed)
            && value["execution"]["batches"].as_u64() == Some(u64::from(batches))
            && value["execution"]["particles_per_batch"].as_u64() == Some(u64::from(particles))
            && value["execution"]["threads"].as_u64() == Some(1)
    });
    let receipt = serde_json::json!({
        "schema_version": "faris-control-execution/v0.1",
        "control": "one_group_pure_absorber_sphere",
        "scientific_scope": "Synthetic mathematical numerical control; no reactor result or nuclear-data validation.",
        "started_unix_seconds": started_unix_seconds,
        "script": {"path": script, "sha256": script_sha256},
        "python": {"path": python, "sha256": python_sha256},
        "openmc": {"path": openmc, "sha256": openmc_sha256},
        "work": {"nominal_histories": histories, "batches": batches, "particles_per_batch": particles, "seed": seed,
            "timeout_seconds": timeout_seconds, "threads": 1, "per_stream_limit_bytes": spec.capture_limit_bytes},
        "execution": execution,
        "control_result": control,
        "control_result_sha256": result_sha256,
        "artifact_error": artifact_error,
        "completed_result_binding_verified": binding_verified,
        "physical_qualification": "NOT_EVALUATED"
    });
    write_new_json(&output.join("execution.json"), &receipt)?;
    println!(
        "Execution {:?}; evidence {}",
        execution.execution_status,
        output.display()
    );
    if execution.execution_status != ExecutionStatus::Succeeded {
        return Err("control did not execute successfully; see execution.json".into());
    }
    if control
        .as_ref()
        .and_then(|v| v.get("status"))
        .and_then(|v| v.as_str())
        != Some("PASS")
        || !binding_verified
    {
        return Err(
            "completed execution did not establish a numerical control PASS; see evidence".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_non_regular_executables_before_reading() {
        #[cfg(unix)]
        assert!(hash_file(Path::new("/dev/zero")).is_err());
    }
    #[test]
    #[cfg(unix)]
    fn interrupt_guard_shuts_down_without_waiting_for_a_signal() {
        let start = std::time::Instant::now();
        drop(interrupt_cancellation().unwrap());
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
