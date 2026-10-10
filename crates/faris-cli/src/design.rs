//! `faris design init` and `faris design check`: the design file for a STEP model.

use clap::{Args, Subcommand};
use faris_engine::design::{
    GeometryOptions, ImportReport, ToolPaths, check_design_with_tools, design_init_with_tools,
    render_text,
};
use faris_model::design::LengthUnit;
use std::{
    fmt,
    path::{Path, PathBuf},
};

/// Where the external helpers are. Nothing is installed or searched for.
#[derive(Args, Clone)]
pub struct ToolArguments {
    /// Python with cadquery, used to read the STEP file [env: FARIS_CAD_PYTHON].
    #[arg(long, value_name = "PYTHON")]
    pub cad_python: Option<PathBuf>,
    /// Python with openmc and h5py, used for the nuclear-data audit [env: FARIS_OPENMC_PYTHON].
    #[arg(long, value_name = "PYTHON")]
    pub openmc_python: Option<PathBuf>,
    /// cross_sections.xml of the audited library [env: FARIS_CROSS_SECTIONS].
    #[arg(long, value_name = "XML")]
    pub cross_sections: Option<PathBuf>,
    /// Directory holding the openmc_plasma_source package, for the source sites of check 9
    /// [env: FARIS_PLASMA_SOURCE_PATH].
    #[arg(long, value_name = "DIR")]
    pub plasma_source_path: Option<PathBuf>,
}

impl ToolArguments {
    pub fn paths(&self) -> ToolPaths {
        ToolPaths::from_flags_and_environment(
            self.cad_python.clone(),
            self.openmc_python.clone(),
            self.cross_sections.clone(),
        )
        .with_plasma_source_path(self.plasma_source_path.clone())
    }
}

#[derive(Subcommand)]
pub enum DesignCommand {
    /// Read a STEP file and write a draft design file with every solid listed and every
    /// material and role left null for you to fill in.
    Init {
        step: PathBuf,
        /// Where to write the draft [default: beside the STEP file]. Never overwrites.
        #[arg(long)]
        out: Option<PathBuf>,
        /// The unit of a STEP file that declares none.
        #[arg(long, value_parser = ["m", "cm", "mm"])]
        length_unit: Option<String>,
        #[command(flatten)]
        tools: ToolArguments,
    },
    /// Run import checks 1 to 9 on a design file and print what passed and what did not.
    /// Exit 0: no check failed. Exit 1: a check failed. Exit 2: usage or I/O error.
    Check {
        design: PathBuf,
        /// Also write the report as JSON to this new file.
        #[arg(long)]
        report: Option<PathBuf>,
        /// Keep the conversion and the other products here, and reuse a conversion found
        /// here when its recorded hashes and tolerances match [default: a temporary
        /// directory, removed afterwards].
        #[arg(long, value_name = "DIR")]
        work_dir: Option<PathBuf>,
        /// Convert again even when the work directory holds a matching conversion.
        #[arg(long)]
        no_reuse: bool,
        /// Worker threads for the integrity checks [default: the available cores, at most 16].
        #[arg(long, value_name = "N")]
        threads: Option<usize>,
        /// Seed of the sampling and ray directions; the same seed gives the same report.
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Points sampled in the bounding box (GEO-024).
        #[arg(long, default_value_t = 1_000_000)]
        samples: usize,
        /// Random chords for the ray consistency check (GEO-025).
        #[arg(long, default_value_t = 100_000)]
        chords: usize,
        /// Source sites sampled from the plasma block (check 9).
        #[arg(long, default_value_t = 100_000)]
        source_sites: usize,
        /// Points per solid for the faceting deviation (GEO-032) [default: 0, every triangle
        /// centroid and edge midpoint]. A positive number takes an even stride, and the report
        /// says so.
        #[arg(long, default_value_t = 0)]
        deviation_points: usize,
        /// Threads the converter may use to imprint.
        #[arg(long, default_value_t = 1)]
        imprint_threads: u32,
        #[command(flatten)]
        tools: ToolArguments,
    },
}

/// A failed check: the report was already printed.
#[derive(Debug)]
pub struct DesignCheckFailed(pub String);

impl fmt::Display for DesignCheckFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DesignCheckFailed {}

pub fn parse_unit(label: &str) -> Option<LengthUnit> {
    match label {
        "m" => Some(LengthUnit::M),
        "cm" => Some(LengthUnit::Cm),
        "mm" => Some(LengthUnit::Mm),
        _ => None,
    }
}

pub fn run(command: DesignCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        DesignCommand::Init {
            step,
            out,
            length_unit,
            tools,
        } => {
            let interrupts = crate::control::interrupt_cancellation()?;
            #[cfg(unix)]
            let cancel = &interrupts.cancellation;
            #[cfg(not(unix))]
            let cancel = &interrupts;
            let summary = design_init_with_tools(
                &step,
                out.as_deref(),
                length_unit.as_deref().and_then(parse_unit),
                &tools.paths(),
                cancel,
            )?;
            println!("{}", summary.line());
            Ok(())
        }
        DesignCommand::Check {
            design,
            report,
            work_dir,
            no_reuse,
            deviation_points,
            threads,
            seed,
            samples,
            chords,
            source_sites,
            imprint_threads,
            tools,
        } => {
            let interrupts = crate::control::interrupt_cancellation()?;
            #[cfg(unix)]
            let cancel = interrupts.cancellation.clone();
            #[cfg(not(unix))]
            let cancel = interrupts.clone();
            let mut options = GeometryOptions {
                work_dir,
                reuse: !no_reuse,
                seed,
                samples,
                chords,
                source_sites,
                deviation_points,
                imprint_threads,
                cancellation: cancel,
                progress: Some(progress_printer()),
                ..GeometryOptions::default()
            };
            if let Some(n) = threads {
                options.threads = n.max(1);
            }
            let result = check_design_with_tools(&design, &tools.paths(), &options)?;
            print!("{}", render_text(&result));
            if let Some(path) = report {
                write_report(&path, &result)?;
                println!("Report written to {}", path.display());
            }
            if result.evaluated_checks_pass {
                Ok(())
            } else {
                Err(Box::new(DesignCheckFailed(result.summary)))
            }
        }
    }
}

/// Progress lines on standard error, at most one every two seconds per stage and one at the end.
fn progress_printer() -> faris_engine::design::ProgressFn {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    let last: Mutex<(String, Instant)> = Mutex::new((String::new(), Instant::now()));
    std::sync::Arc::new(move |stage: &str, done: usize, total: usize| {
        let Ok(mut state) = last.lock() else { return };
        let finished = done >= total;
        if state.0 != stage || finished || state.1.elapsed() >= Duration::from_secs(2) {
            eprintln!("  {stage}: {done} of {total}");
            *state = (stage.to_string(), Instant::now());
        }
    })
}

fn write_report(path: &Path, report: &ImportReport) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| {
            format!(
                "{}: {e} (an existing report is never overwritten)",
                path.display()
            )
        })?;
    file.write_all(&serde_json::to_vec_pretty(report)?)?;
    file.write_all(b"\n")?;
    Ok(())
}
