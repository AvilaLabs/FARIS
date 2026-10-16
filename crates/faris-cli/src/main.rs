use clap::{Parser, Subcommand};
use faris_engine::build_manifest;
use faris_model::LoadedScenario;
use std::path::PathBuf;

mod activation;
mod adapters;
mod cache;
mod config;
mod control;
mod design;
mod diff;
mod evidence;
mod exit;
mod history;
mod maintenance;
mod materials;
mod reactor;
mod study;
mod study_export;
mod study_file;
mod transport;
mod waste;

#[derive(Parser)]
#[command(
    name = "faris",
    version,
    about = "Fusion Analysis and Reactor Integration Simulator",
    after_help = exit::help_table()
)]
struct Arguments {
    #[command(subcommand)]
    command: Command,
    /// Set one setting for this run, over the environment and your settings file. Repeatable.
    /// `faris config list` shows the keys.
    #[arg(long = "setting", value_name = "KEY=VALUE", global = true)]
    settings: Vec<String>,
    /// Unit system for printed quantities: si (the default) or fusion (MW, keV, cm, GWd, dpa).
    /// The same as `--setting units.system=...`; stored files are never changed by it.
    #[arg(long, value_name = "SYSTEM", global = true)]
    units: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Calculate or validate the shared Rust operating-history ledger.
    History {
        #[command(subcommand)]
        command: history::HistoryCommand,
    },
    /// Activation inventory, decay heat and decay-photon source of zones through ACTINV.
    Activation {
        #[command(subcommand)]
        command: activation::ActivationCommand,
    },
    /// Compute replacement outages from activation decay heat and compare them with fixed ones.
    Maintenance {
        #[command(subcommand)]
        command: maintenance::MaintenanceCommand,
    },
    /// Waste classification (10 CFR 61.55), IAEA clearance index and recycling index of activated components.
    Waste {
        #[command(subcommand)]
        command: waste::WasteCommand,
    },
    /// The cited materials and limits database: compositions, impurity limits, damage and gas limits.
    Materials {
        #[command(subcommand)]
        command: materials::MaterialsCommand,
    },
    /// Package and execute identified simulation evidence with real Avila Core.
    Evidence {
        #[command(subcommand)]
        command: evidence::EvidenceCommand,
    },
    /// Write and check the design file that says what a STEP model's solids are.
    Design {
        #[command(subcommand)]
        command: design::DesignCommand,
    },
    /// Compare two .faris studies, or two scenario, physics, assumptions or design files: every
    /// changed parameter with old and new value and unit, causes apart from effects, and which
    /// results the change invalidates. Exits 0 when identical, 6 when they differ.
    Diff {
        a: PathBuf,
        b: PathBuf,
        /// Stable, versioned JSON (`faris-diff/v1`) instead of text.
        #[arg(long)]
        json: bool,
        /// Also list the values that changed inside changed result files (slower, longer).
        #[arg(long)]
        values: bool,
        /// Write the report to a new file instead of printing it.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Print how long the comparison took on standard error.
        #[arg(long)]
        time: bool,
    },
    /// Tables about a model's geometry.
    Geometry {
        #[command(subcommand)]
        command: diff::GeometryCommand,
    },
    /// Look at, check and trim the result cache.
    Cache {
        /// The cache folder [default: `faris-transport-cache` in the current folder].
        #[arg(long, global = true)]
        dir: Option<PathBuf>,
        #[command(subcommand)]
        command: cache::CacheCommand,
    },
    /// Show and change your settings, and which layer supplies each value.
    Config {
        #[command(subcommand)]
        command: config::ConfigCommand,
    },
    /// Validate a scenario without running physics.
    Validate { scenario: PathBuf },
    /// Export geometry metadata (--scenario), or a study's PDF brief, CSV and charts (--study).
    Export {
        /// Scenario to write the geometry manifest for.
        #[arg(long, required_unless_present = "study", conflicts_with = "study")]
        scenario: Option<PathBuf>,
        /// A .faris study file: writes the same export folder as the desktop's export
        /// (the 3D view is not captured on the command line).
        #[arg(long)]
        study: Option<PathBuf>,
        /// The manifest file for --scenario; the folder to create the export in for --study.
        #[arg(long)]
        output: PathBuf,
    },
    /// Inspect optional tool availability and file hashes without executing tools.
    Doctor,
    /// List the external tools FARIS can drive with version, hash and health.
    Adapters {
        #[command(subcommand)]
        command: adapters::AdaptersCommand,
    },
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
    let arguments = match Arguments::try_parse() {
        Ok(arguments) => arguments,
        Err(error) => {
            // Help and version go to stdout and succeed; anything else did not parse.
            let parsed = !error.use_stderr();
            let _ = error.print();
            return std::process::ExitCode::from(if parsed { exit::OK } else { exit::USAGE });
        }
    };
    match run(arguments) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("faris: {error}");
            std::process::ExitCode::from(exit::classify(error.as_ref()))
        }
    }
}

fn run(arguments: Arguments) -> Result<(), Box<dyn std::error::Error>> {
    // `--units` is a flag-layer setting that an explicit `--setting units.system=` overrides.
    let settings: Vec<String> = arguments
        .units
        .iter()
        .map(|system| format!("units.system={system}"))
        .chain(arguments.settings.iter().cloned())
        .collect();
    // `faris config` reports a damaged settings stack itself, so it can be used to repair it.
    if !matches!(arguments.command, Command::Config { .. }) {
        config::startup(&settings)?;
    }
    match arguments.command {
        Command::Adapters { command } => adapters::run(command)?,
        Command::Config { command } => config::run(command, &settings)?,
        Command::History { command } => history::run(command)?,
        Command::Activation { command } => activation::run(command)?,
        Command::Maintenance { command } => maintenance::run(command)?,
        Command::Waste { command } => waste::run(command)?,
        Command::Materials { command } => materials::run(command)?,
        Command::Evidence { command } => evidence::run(command)?,
        Command::Design { command } => design::run(command)?,
        Command::Diff {
            a,
            b,
            json,
            values,
            output,
            time,
        } => diff::run_diff(a, b, json, values, output, time)?,
        Command::Geometry { command } => diff::run_geometry(command)?,
        Command::Cache { dir, command } => cache::run(command, dir)?,
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
        Command::Export {
            scenario,
            study,
            output,
        } => {
            if let Some(study) = study {
                let summary = study_export::export(&study, &output)?;
                println!("{}", serde_json::to_string_pretty(&summary)?);
            } else if let Some(scenario) = scenario {
                let manifest = build_manifest(&LoadedScenario::load(&scenario)?)?;
                transport::write_new_json(&output, &manifest)?;
                println!("Exported {} to {}", manifest.scenario_id, output.display());
            }
        }
        Command::Doctor => {
            // Path and hash of what is found; nothing is executed (`faris adapters list` runs
            // the version check).
            let listing = adapters::list(std::env::var_os("PATH").as_deref(), false);
            let tools: serde_json::Map<String, serde_json::Value> = listing["tools"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|tool| {
                    (
                        tool["name"].as_str().unwrap_or_default().to_owned(),
                        serde_json::json!({
                            "detected": tool["detected"], "path": tool["path"],
                            "sha256": tool["sha256"], "adapter_status": "NOT_EVALUATED",
                        }),
                    )
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "faris_version": env!("CARGO_PKG_VERSION"), "optional_tools": tools,
                    "bundled_adapters": listing["bundled_adapters"],
                    "note": "PATH presence and file hashes only; no tool executed (faris adapters list also reports versions and health). Commands that run a tool take its path as a flag (faris reactor run, faris design check and transport, faris evidence run, faris control absorber); the desktop app's Find tools searches environments as well as PATH.",
                }))?
            );
        }
    }
    Ok(())
}
