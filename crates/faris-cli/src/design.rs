//! `faris design init` and `faris design check`: the design file for a STEP model.

use clap::{Args, Subcommand};
use faris_engine::design::{
    ImportReport, ToolPaths, check_design_with_tools, design_init_with_tools, render_text,
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
}

impl ToolArguments {
    pub fn paths(&self) -> ToolPaths {
        ToolPaths::from_flags_and_environment(
            self.cad_python.clone(),
            self.openmc_python.clone(),
            self.cross_sections.clone(),
        )
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
    /// Run import checks 1 to 5 on a design file and print what passed and what did not.
    /// Exit 0: no check failed. Exit 1: a check failed. Exit 2: usage or I/O error.
    Check {
        design: PathBuf,
        /// Also write the report as JSON to this new file.
        #[arg(long)]
        report: Option<PathBuf>,
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
            tools,
        } => {
            let interrupts = crate::control::interrupt_cancellation()?;
            #[cfg(unix)]
            let cancel = &interrupts.cancellation;
            #[cfg(not(unix))]
            let cancel = &interrupts;
            let result = check_design_with_tools(&design, &tools.paths(), cancel)?;
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
