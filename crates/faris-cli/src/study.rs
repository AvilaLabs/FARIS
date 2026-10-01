use crate::{
    control,
    transport::{read_bounded, write_new_json},
};
use clap::{Subcommand, ValueEnum};
use faris_engine::{
    build_manifest,
    study::{self, GeneratedStudy, StudySelection},
};
use faris_model::LoadedScenario;
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Debug)]
pub struct CoreCompilationRejected;

impl std::fmt::Display for CoreCompilationRejected {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Core rejected the study contract; see compilation.json")
    }
}

impl std::error::Error for CoreCompilationRejected {}

#[derive(Clone, Copy, Debug, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum Analysis {
    Breeding,
    Shielding,
    FuelHistory,
    Electricity,
}

#[derive(Subcommand)]
pub enum StudyCommand {
    /// Generate an immutable study directory containing study, contract, and registry JSON.
    Generate {
        #[arg(long)]
        scenario: PathBuf,
        #[arg(long)]
        variant: String,
        /// Select analyses, comma-separated or repeated (default: breeding,shielding).
        #[arg(
            long,
            value_enum,
            value_delimiter = ',',
            default_value = "breeding,shielding"
        )]
        analysis: Vec<Analysis>,
        #[arg(long, default_value = "1")]
        minimum_tbr: String,
        /// New exclusive evidence directory; existing paths are refused.
        #[arg(long)]
        output: PathBuf,
    },
    /// Compile a generated study with the explicitly selected Avila Core executable.
    Compile {
        #[arg(long)]
        scenario: PathBuf,
        #[arg(long)]
        study: PathBuf,
        #[arg(long)]
        core: PathBuf,
        /// New exclusive evidence directory; existing paths are refused.
        #[arg(long)]
        output: PathBuf,
    },
}

fn selection(analyses: &[Analysis], minimum_tbr: String) -> StudySelection {
    let selected: BTreeSet<_> = analyses.iter().map(|a| *a as u8).collect();
    StudySelection {
        breeding: selected.contains(&(Analysis::Breeding as u8)),
        shielding: selected.contains(&(Analysis::Shielding as u8)),
        fuel_history: selected.contains(&(Analysis::FuelHistory as u8)),
        electricity: selected.contains(&(Analysis::Electricity as u8)),
        minimum_tbr,
    }
}

fn new_directory(path: PathBuf) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = std::path::absolute(path)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir(&path)?;
    Ok(path)
}

fn write_generated(
    output: &std::path::Path,
    generated: &GeneratedStudy,
) -> Result<(), Box<dyn std::error::Error>> {
    write_new_json(&output.join("study.json"), generated)?;
    write_new_json(&output.join("contract.json"), &generated.contract)?;
    write_new_json(&output.join("registry.json"), &generated.registry)?;
    Ok(())
}

pub fn run(command: StudyCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        StudyCommand::Generate {
            scenario,
            variant,
            analysis,
            minimum_tbr,
            output,
        } => {
            let loaded = LoadedScenario::load(&scenario)?;
            let manifest = build_manifest(&loaded)?;
            let generated =
                study::generate_study(&manifest, &variant, &selection(&analysis, minimum_tbr))
                    .map_err(std::io::Error::other)?;
            let output = new_directory(output)?;
            write_generated(&output, &generated)?;
            println!(
                "Generated study {} for {} (Core compilation and scientific evaluation NOT_RUN); evidence {}",
                generated.contract["contract_id"]
                    .as_str()
                    .unwrap_or("<unknown>"),
                generated.variant_id,
                output.display()
            );
        }
        StudyCommand::Compile {
            scenario,
            study: study_path,
            core,
            output,
        } => {
            let loaded = LoadedScenario::load(&scenario)?;
            let manifest = build_manifest(&loaded)?;
            let supplied: serde_json::Value = serde_json::from_slice(&read_bounded(&study_path)?)?;
            let variant = supplied
                .get("variant_id")
                .and_then(serde_json::Value::as_str)
                .ok_or("study.json must contain a string variant_id")?;
            let selection: StudySelection = serde_json::from_value(
                supplied
                    .get("selection")
                    .cloned()
                    .ok_or("study.json must contain a selection object")?,
            )?;
            let generated = study::generate_study(&manifest, variant, &selection)
                .map_err(std::io::Error::other)?;
            if supplied != serde_json::to_value(&generated)? {
                return Err(
                    "study.json is not the exact generated study for these scenario bytes".into(),
                );
            }
            let interrupts = control::interrupt_cancellation()?;
            #[cfg(unix)]
            let cancellation = &interrupts.cancellation;
            #[cfg(not(unix))]
            let cancellation = &interrupts;
            let output = std::path::absolute(output)?;
            let result = study::compile_study(&generated, &core, &output, cancellation)
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            let compiler_status = result.report["status"].as_str().unwrap_or("not_available");
            println!(
                "Core compilation: {compiler_status}; execution: {:?}; scientific evaluation: NOT_EVALUATED; evidence {}",
                result.execution.execution_status,
                output.display()
            );
            match compiler_status {
                "compiled" => {}
                "rejected" => return Err(Box::new(CoreCompilationRejected)),
                _ => return Err("Core compilation did not produce a supported report".into()),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn analysis_selection_deduplicates_flags_and_defaults_are_visible_in_help() {
        let parsed = selection(
            &[
                Analysis::Breeding,
                Analysis::Breeding,
                Analysis::Electricity,
            ],
            "1".into(),
        );
        assert!(parsed.breeding && !parsed.shielding && parsed.electricity && !parsed.fuel_history);
        let error = match crate::Arguments::try_parse_from(["faris", "study", "generate", "--help"])
        {
            Ok(_) => panic!("--help should stop argument parsing"),
            Err(error) => error,
        };
        let help = error.to_string();
        assert!(help.contains("--analysis"));
        assert!(help.contains("--output"));
    }
}
