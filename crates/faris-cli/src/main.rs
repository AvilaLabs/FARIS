use clap::{Parser, Subcommand};
use faris_engine::build_manifest;
use faris_model::LoadedScenario;
use std::{fs::OpenOptions, io::Write, path::PathBuf};

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
}

fn main() -> std::process::ExitCode {
    match run(Arguments::parse()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("faris: {error}");
            std::process::ExitCode::from(2)
        }
    }
}

fn run(arguments: Arguments) -> Result<(), Box<dyn std::error::Error>> {
    match arguments.command {
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
            let mut bytes = serde_json::to_vec_pretty(&manifest)?;
            bytes.push(b'\n');
            if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output)?;
            file.write_all(&bytes)?;
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
                    "note": "PATH presence only. No adapter is implemented and no solver was executed.",
                }))?
            );
        }
    }
    Ok(())
}
