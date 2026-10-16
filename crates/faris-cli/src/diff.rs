//! `faris diff A B` (COL-020, COL-023) and `faris geometry summary` (GEO-069).
//!
//! The comparison itself is `faris_engine::semantic_diff` and `faris_study::diff`; this file reads
//! the two paths, picks the study or the model comparison, and prints the JSON or the text.

use clap::Subcommand;
use faris_engine::semantic_diff::{
    DiffReport, diff_models, geometry_summary, geometry_summary_csv, render_text,
};
use faris_study::{StudyDiffOptions, diff_study_files};
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};

/// The two sides differ; exits with status 6, so a script can tell "different" from "failed".
#[derive(Debug)]
pub struct DifferencesFound(pub String);

impl std::fmt::Display for DifferencesFound {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for DifferencesFound {}

fn is_study(path: &Path) -> bool {
    if path.extension().is_some_and(|e| e == "faris") {
        return true;
    }
    let mut magic = [0u8; 2];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .is_ok()
        && &magic == b"PK"
}

fn read_model(path: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    let bytes = crate::transport::read_bounded(path)?;
    serde_json::from_slice(&bytes).map_err(|e| {
        format!(
            "{} is neither a .faris study nor a JSON model file ({e}). Next step: give two .faris files, or two scenario, physics, assumptions or design files",
            path.display()
        )
        .into()
    })
}

/// Compare two paths: two studies, or two model documents.
pub fn compare(
    a: &Path,
    b: &Path,
    include_result_values: bool,
) -> Result<DiffReport, Box<dyn std::error::Error>> {
    match (is_study(a), is_study(b)) {
        (true, true) => diff_study_files(a, b, StudyDiffOptions { include_result_values })
            .map_err(crate::study_file::convert),
        (false, false) => Ok(diff_models(
            &a.display().to_string(),
            &read_model(a)?,
            &b.display().to_string(),
            &read_model(b)?,
        )),
        _ => Err(
            "one path is a .faris study and the other is a model file; a study can only be compared with a study. Next step: unpack one with `faris study-file unpack`, or give two of the same kind"
                .into(),
        ),
    }
}

pub fn run_diff(
    a: PathBuf,
    b: PathBuf,
    json: bool,
    values: bool,
    output: Option<PathBuf>,
    time: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let started = std::time::Instant::now();
    let report = compare(&a, &b, values)?;
    let text = if json {
        let mut text = serde_json::to_string_pretty(&report)?;
        text.push('\n');
        text
    } else {
        render_text(&report)
    };
    match output {
        Some(path) => crate::transport::write_new_bytes(&path, text.as_bytes())?,
        None => print!("{text}"),
    }
    if time {
        eprintln!("compared in {:.3} s", started.elapsed().as_secs_f64());
    }
    if report.identical {
        Ok(())
    } else {
        Err(Box::new(DifferencesFound(format!(
            "the two sides differ: {} inputs, {} evidence, {} results, {} presentation",
            report.summary.inputs_changed,
            report.summary.evidence_changed,
            report.summary.results_changed,
            report.summary.presentation_changed
        ))))
    }
}

#[derive(Subcommand)]
pub enum GeometryCommand {
    /// Write the table of cells of a scenario: name, id, material, volume, mass, representation and
    /// kind label, as CSV.
    Summary {
        #[arg(long)]
        scenario: PathBuf,
        /// A physics case for the scenario, to give each cell its density and mass. Without one the
        /// mass is NOT_EVALUATED.
        #[arg(long)]
        physics: Option<PathBuf>,
        /// The allocation to tabulate [default: the physics case's, or the first].
        #[arg(long)]
        variant: Option<String>,
        /// New CSV file; an existing file is never overwritten [default: print].
        #[arg(long)]
        output: Option<PathBuf>,
        /// JSON instead of CSV.
        #[arg(long)]
        json: bool,
    },
}

pub fn run_geometry(command: GeometryCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        GeometryCommand::Summary {
            scenario,
            physics,
            variant,
            output,
            json,
        } => {
            let loaded = faris_model::LoadedScenario::from_bytes(&crate::transport::read_bounded(
                &scenario,
            )?)?;
            let document: Value = serde_json::from_slice(loaded.source_bytes())?;
            let case = match &physics {
                Some(path) => Some(
                    faris_engine::reactor::load_physics_case(path, &loaded)
                        .map_err(|e| e.to_string())?,
                ),
                None => None,
            };
            let variant = variant
                .or_else(|| case.as_ref().map(|c| c.variant_id.clone()))
                .or_else(|| loaded.scenario.variants.first().map(|v| v.id.clone()))
                .ok_or("the scenario has no variants")?;
            let summary = geometry_summary(&document, &variant, case.as_ref())?;
            let text = if json {
                let mut text = serde_json::to_string_pretty(&summary)?;
                text.push('\n');
                text
            } else {
                geometry_summary_csv(&summary)
            };
            match output {
                Some(path) => {
                    crate::transport::write_new_bytes(&path, text.as_bytes())?;
                    println!(
                        "Wrote {} cells of {} to {}.",
                        summary.rows.len(),
                        variant,
                        path.display()
                    );
                }
                None => print!("{text}"),
            }
        }
    }
    Ok(())
}
