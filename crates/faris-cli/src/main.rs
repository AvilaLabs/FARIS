use clap::{Parser, Subcommand};
use faris_engine::build_manifest;
use faris_model::LoadedScenario;
use std::path::PathBuf;

mod control;
mod evidence;
mod history;
mod maintenance;
mod reactor;
mod study;
mod study_export;
mod study_file;
mod transport;

#[derive(Parser)]
#[command(
    name = "faris",
    version,
    about = "Fusion Analysis and Reactor Integration Simulator"
)]
struct Arguments {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Calculate or validate the shared Rust operating-history ledger.
    History {
        #[command(subcommand)]
        command: history::HistoryCommand,
    },
    /// Compute replacement outages from activation decay heat and compare them with fixed ones.
    Maintenance {
        #[command(subcommand)]
        command: maintenance::MaintenanceCommand,
    },
    /// Package and execute identified simulation evidence with real Avila Core.
    Evidence {
        #[command(subcommand)]
        command: evidence::EvidenceCommand,
    },
    /// Validate a scenario without running physics.
    Validate { scenario: PathBuf },
    /// Export geometry metadata and explicit evaluation status.
    Export {
        #[arg(long)]
        scenario: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Inspect optional tool availability without executing tools.
    Doctor,
    /// Execute independently specified mathematical controls through the job runner.
    Control {
        #[command(subcommand)]
        command: control::ControlCommand,
    },
    /// Check and normalize raw fixed-source transport artifacts; no solver invocation.
    Transport {
        #[command(subcommand)]
        command: transport::TransportCommand,
    },
    /// Generate and semantically compile a declared Core study; no solver runs.
    Study {
        #[command(subcommand)]
        command: study::StudyCommand,
    },
    /// Create, inspect, verify and unpack .faris study files.
    StudyFile {
        #[command(subcommand)]
        command: study_file::StudyFileCommand,
    },
    /// Execute a fixed-source cold-data numerical reference through OpenMC.
    Reactor {
        #[command(subcommand)]
        command: reactor::ReactorCommand,
    },
}

fn main() -> std::process::ExitCode {
    match run(Arguments::parse()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("faris: {error}");
            std::process::ExitCode::from(
                if error
                    .downcast_ref::<study::CoreCompilationRejected>()
                    .is_some()
                    || error
                        .downcast_ref::<study_file::StudyFileRejected>()
                        .is_some()
                {
                    1
                } else {
                    2
                },
            )
        }
    }
}

fn run(arguments: Arguments) -> Result<(), Box<dyn std::error::Error>> {
    match arguments.command {
        Command::History { command } => history::run(command)?,
        Command::Maintenance { command } => maintenance::run(command)?,
        Command::Evidence { command } => evidence::run(command)?,
        Command::Control { command } => control::run(command)?,
        Command::Reactor { command } => reactor::run(command)?,
        Command::Study { command } => study::run(command)?,
        Command::StudyFile { command } => study_file::run(command)?,
        Command::Transport { command } => transport::run(command)?,
        Command::Validate { scenario } => {
            let loaded = LoadedScenario::load(&scenario)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "scenario_id": loaded.scenario.id, "valid": true,
                    "variants": loaded.scenario.variants.len(), "source_sha256": loaded.source_sha256,
                }))?
            );
        }
        Command::Export { scenario, output } => {
            let manifest = build_manifest(&LoadedScenario::load(&scenario)?)?;
            transport::write_new_json(&output, &manifest)?;
            println!("Exported {} to {}", manifest.scenario_id, output.display());
        }
        Command::Doctor => {
            let tools: serde_json::Map<String, serde_json::Value> = [
                "openmc",
                "actinv",
                "avila-core",
            ]
            .into_iter()
            .map(|name| {
                let detected = std::env::var_os("PATH").is_some_and(|paths| {
                    std::env::split_paths(&paths).any(|directory| {
                        let path = directory.join(name);
                        path.is_file()
                            || cfg!(windows) && directory.join(format!("{name}.exe")).is_file()
                    })
                });
                (
                    name.into(),
                    serde_json::json!({"detected": detected, "adapter_status": "NOT_EVALUATED"}),
                )
            })
            .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "faris_version": env!("CARGO_PKG_VERSION"), "optional_tools": tools,
                    "note": "PATH presence only; no solver executed. The absorber numerical control accepts explicit tool paths. Reactor transport and Core execution are not implemented.",
                }))?
            );
        }
    }
    Ok(())
}
