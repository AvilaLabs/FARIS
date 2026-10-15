//! `faris adapters list` and the tool section of `faris doctor` (AUTO-066): each external
//! tool FARIS can drive, with where it was found, the SHA-256 of that file, the version it
//! reports and whether it answers. The health check runs only `<tool> --version` through
//! the bounded job runner, never physics, and each probe is limited to five seconds.

use crate::transport::sha256;
use clap::Subcommand;
use faris_engine::jobs::{Cancellation, ExecutionStatus, JobSpec, ResourceLimits, run_job};
use serde_json::{Value, json};
use std::{
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

/// External tools FARIS can drive, by the name looked up on PATH.
pub const TOOLS: [&str; 3] = ["openmc", "actinv", "avila-core"];
/// The longest a health check may take.
pub const PROBE_SECONDS: u64 = 5;
const MAX_HASHED_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Subcommand)]
pub enum AdaptersCommand {
    /// List the tools FARIS can drive with path, SHA-256, version and health. Output is JSON.
    List {
        /// Do not run `<tool> --version`; health is then NOT_EVALUATED and the version unknown.
        #[arg(long)]
        no_probe: bool,
    },
}

/// The first regular file called `name` on `path_variable` (with `.exe` on Windows).
pub fn find_on_path(name: &str, path_variable: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    std::env::split_paths(path_variable?).find_map(|directory| {
        let plain = directory.join(name);
        if plain.is_file() {
            return Some(plain);
        }
        let exe = directory.join(format!("{name}.exe"));
        (cfg!(windows) && exe.is_file()).then_some(exe)
    })
}

fn hash_bounded(path: &Path) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    if length > MAX_HASHED_BYTES {
        return Err("larger than the 512 MiB hashing limit".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_HASHED_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(sha256(&bytes))
}

/// The first line of the tool's `--version` output, its health, and why when not OK.
fn probe(program: &Path) -> (Option<String>, &'static str, Option<String>) {
    let Ok(program) = program.canonicalize() else {
        return (None, "FAILED", Some("the file cannot be resolved".into()));
    };
    let Ok(work) = tempfile::tempdir() else {
        return (
            None,
            "NOT_EVALUATED",
            Some("no scratch folder could be made for the check".into()),
        );
    };
    let spec = JobSpec {
        program,
        arguments: vec!["--version".into()],
        working_directory: work.path().to_path_buf(),
        environment: std::env::var_os("PATH")
            .map(|path| vec![("PATH".into(), path)])
            .unwrap_or_default(),
        timeout: Duration::from_secs(PROBE_SECONDS),
        capture_limit_bytes: 64 * 1024,
        artifact_roots: Vec::new(),
        resource_limits: ResourceLimits::default(),
    };
    match run_job(&spec, &Cancellation::default()) {
        Ok(result) if result.execution_status == ExecutionStatus::Succeeded => {
            let version = result
                .stdout
                .lines()
                .chain(result.stderr.lines())
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(|line| line.chars().take(200).collect());
            (version, "OK", None)
        }
        Ok(result) => (
            None,
            "FAILED",
            Some(format!(
                "`--version` ended as {:?}; run the tool by hand to see why",
                result.execution_status
            )),
        ),
        Err(error) => (None, "FAILED", Some(error.to_string())),
    }
}

/// One entry per tool, in `TOOLS` order.
pub fn list(path_variable: Option<&std::ffi::OsStr>, probing: bool) -> Value {
    let tools: Vec<Value> = TOOLS
        .into_iter()
        .map(|name| match find_on_path(name, path_variable) {
            None => json!({
                "name": name, "detected": false, "path": null, "sha256": null,
                "version": null, "health": "NOT_FOUND",
                "next_step": format!("install {name} or pass its path to the command that needs it"),
            }),
            Some(path) => {
                let hash = hash_bounded(&path);
                let (version, health, why) = if probing {
                    probe(&path)
                } else {
                    (
                        None,
                        "NOT_EVALUATED",
                        Some("run without --no-probe to check".to_string()),
                    )
                };
                json!({
                    "name": name, "detected": true, "path": path,
                    "sha256": hash.as_ref().ok(), "sha256_error": hash.as_ref().err(),
                    "version": version, "health": health, "health_detail": why,
                })
            }
        })
        .collect();
    json!({
        "faris_version": env!("CARGO_PKG_VERSION"),
        "check": format!("only `<tool> --version`, at most {PROBE_SECONDS} s each; no physics"),
        "tools": tools,
        "bundled_adapters": bundled(),
    })
}

/// The scripts FARIS carries and runs under a tool, by hash.
fn bundled() -> Value {
    const SCRIPTS: [(&str, &[u8]); 2] = [
        (
            "openmc reactor transport",
            include_bytes!("../../../integrations/openmc/reactor_transport.py"),
        ),
        (
            "absorber control",
            include_bytes!("../../../controls/pure_absorber_sphere.py"),
        ),
    ];
    SCRIPTS
        .iter()
        .map(|(name, bytes)| json!({"name": name, "sha256": sha256(bytes)}))
        .collect()
}

pub fn run(command: AdaptersCommand) -> Result<(), Box<dyn std::error::Error>> {
    let AdaptersCommand::List { no_probe } = command;
    let path = std::env::var_os("PATH");
    println!(
        "{}",
        serde_json::to_string_pretty(&list(path.as_deref(), !no_probe))?
    );
    Ok(())
}
