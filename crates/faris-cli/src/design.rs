// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! `faris design init`, `check` and `transport`: the design file for a STEP model.

use clap::{Args, Subcommand};
use faris_engine::design::{
    CadReader, DesignHeatingOptions, DesignHistoryRatesArtifact, DesignTransportOptions,
    GeometryOptions, ImportReport, ToolPaths, check_design_with_tools,
    combine_history_energy_rates, design_heating, design_heating_with_history_rates,
    design_init_with_tools, design_transport, render_design_heating_text, render_text,
    render_transport_text,
};
use faris_model::design::LengthUnit;
use std::{
    collections::BTreeMap,
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
    /// Which reader runs the STEP inspect and the per-solid tessellation: `native` (the
    /// OpenCASCADE worker), `python` (the helper), or `auto`, the worker when one is found and
    /// answers and the helper otherwise [default: auto; env: FARIS_CAD_READER].
    #[arg(long, value_name = "READER", value_parser = ["auto", "native", "python"])]
    pub cad_reader: Option<String>,
    /// The native CAD worker, faris-occt-worker [default: the one next to the faris program;
    /// env: FARIS_OCCT_WORKER].
    #[arg(long, value_name = "WORKER")]
    pub occt_worker: Option<PathBuf>,
}

impl ToolArguments {
    pub fn paths(&self) -> ToolPaths {
        ToolPaths::from_flags_and_environment(
            self.cad_python.clone(),
            self.openmc_python.clone(),
            self.cross_sections.clone(),
        )
        .with_plasma_source_path(self.plasma_source_path.clone())
        .with_cad_reader(self.cad_reader.as_deref().and_then(CadReader::from_label))
        .with_occt_worker(self.occt_worker.clone())
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
        /// Keep the conversion, the per-solid tessellation, the boolean verdicts and the other
        /// products here, and reuse what is found here when its recorded STEP hash, tolerances
        /// and helper version match [default: a temporary directory, removed afterwards].
        #[arg(long, value_name = "DIR")]
        work_dir: Option<PathBuf>,
        /// Do everything again even when the work directory holds matching products.
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
        /// Points per solid the faceting deviation (GEO-032) lets OpenCASCADE project. Faces on a
        /// plane, cylinder, cone, sphere or torus are measured in closed form at every point;
        /// the points on other surfaces take an even stride of at most this many [0: all of
        /// them]. These diagnostics are separate from the global triangle bound.
        #[arg(long, default_value_t = 2000)]
        deviation_points: usize,
        /// Bounded CAD distance queries per solid for the GEO-032 global proof.
        #[arg(long, default_value_t = 100_000)]
        deviation_bound_queries: usize,
        /// Maximum recursive subdivision depth for the GEO-032 global proof.
        #[arg(long, default_value_t = 18)]
        deviation_bound_depth: usize,
        /// Stochastic OpenMC volume samples per DAGMC cell for GEO-030.
        #[arg(long, default_value_t = 100_000)]
        stochastic_volume_samples: usize,
        /// Threads the converter may use to imprint.
        #[arg(long, default_value_t = 4)]
        imprint_threads: u32,
        #[command(flatten)]
        tools: ToolArguments,
    },
    /// Compute the neutron flux in the solids a fluence limit of the design applies to (the TF
    /// magnets, above 0.1 MeV, when it has none), for a design that passed `design check`,
    /// running until the mean over those solids reaches the target error.
    /// Exit 0: target reached. Exit 1: a limit stopped the run first. Exit 2: usage or I/O error.
    Transport {
        design: PathBuf,
        /// The report `design check --report` wrote for this design; its work directory must
        /// still hold the conversion.
        #[arg(long, value_name = "JSON")]
        check_report: PathBuf,
        /// A new directory for the run. Never overwrites.
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
        /// Also write the result as JSON to this new file.
        #[arg(long)]
        report: Option<PathBuf>,
        /// Relative standard error of the mean flux at which the run stops.
        #[arg(long, default_value_t = 0.05)]
        target_error: f64,
        /// Stop after this many histories even when the target is not reached.
        #[arg(long, default_value_t = 50_000_000)]
        max_histories: u64,
        /// Stop after this many seconds of sampling even when the target is not reached.
        #[arg(long, default_value_t = 3600)]
        max_seconds: u64,
        /// Histories per batch; the error is checked after every batch.
        #[arg(long, default_value_t = 20_000)]
        particles: u64,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u32).range(1..=32))]
        threads: u32,
        /// A Python with OpenMC 0.16 or later. With it, weight windows are made for the magnets
        /// before the run, which then needs far fewer histories.
        #[arg(long, value_name = "PYTHON")]
        window_python: Option<PathBuf>,
        /// A directory holding an `openmc` package to use in place of --openmc-python's own, such
        /// as a build with the aiming patch.
        #[arg(long, value_name = "DIR")]
        openmc_package_path: Option<PathBuf>,
        /// The replacement group whose fluence limit names the solids and the energy threshold
        /// to score [default: the only group with a fluence limit].
        #[arg(long, value_name = "ID")]
        group: Option<String>,
        /// Add all-energy component flux and breeder H3 responses for operating history.
        /// Repeat as `--history-component component_id=solid_id[,solid_id]` for every active solid.
        #[arg(long = "history-component", value_name = "COMPONENT=SOLID[,SOLID]", action = clap::ArgAction::Append)]
        history_component: Vec<String>,
        /// Exact scenario whose authored operating power is bound to the CAD history rates.
        #[arg(long, value_name = "SCENARIO.json", requires_all = ["history_component", "h3_component"])]
        history_scenario: Option<PathBuf>,
        /// Component receiving the H3-production tally; requires an explicit Li6-bearing CAD domain.
        #[arg(long, value_name = "ID", requires_all = ["history_component", "history_scenario"])]
        h3_component: Option<String>,
        /// Where weight windows and the channel search are kept, so that a later run on the same
        /// model starts sampling at once [default: `faris-transport-cache` beside the design file].
        #[arg(long, value_name = "DIR")]
        cache_dir: Option<PathBuf>,
        /// Keep nothing between runs and reuse nothing.
        #[arg(long, conflicts_with = "cache_dir")]
        no_cache: bool,
        #[command(flatten)]
        tools: ToolArguments,
    },
    /// Run separate all-energy coupled neutron/photon/electron heating in explicitly authored
    /// winding-pack solids, then score the highest sampled ring voxel. With --history-rates,
    /// also score unfiltered whole-model heating for the exact CAD history component partition.
    Heating {
        design: PathBuf,
        #[arg(long, value_name = "JSON")]
        check_report: PathBuf,
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
        #[arg(long)]
        report: Option<PathBuf>,
        /// Bind a previously validated CAD history-rate artifact and emit
        /// `history-energy-rates.json` in the new run directory on success.
        #[arg(long, value_name = "CAD_HISTORY_RATES.json")]
        history_rates: Option<PathBuf>,
        /// Radial and vertical peak voxel widths, in metres: --peak-voxel-m 0.05,0.05.
        #[arg(long, value_delimiter = ',', num_args = 2, default_values_t = [0.05, 0.05])]
        peak_voxel_m: Vec<f64>,
        #[arg(long, default_value_t = 50_000_000)]
        max_histories: u64,
        #[arg(long, default_value_t = 3600)]
        max_seconds: u64,
        /// Relative standard error target for aggregate coupled heating (resolved peak target is twice this).
        #[arg(long, default_value_t = 0.05)]
        target_error: f64,
        #[arg(long, default_value_t = 20_000)]
        particles: u64,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u32).range(1..=32))]
        threads: u32,
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

fn parse_history_components(values: &[String]) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut components = BTreeMap::new();
    for value in values {
        let (component, solids) = value.split_once('=').ok_or_else(|| {
            format!(
                "invalid --history-component {value:?}; expected component_id=solid_id[,solid_id]"
            )
        })?;
        let component = component.trim();
        let solid_ids: Vec<String> = solids
            .split(',')
            .map(str::trim)
            .map(str::to_string)
            .collect();
        if component.is_empty()
            || solid_ids.is_empty()
            || solid_ids.iter().any(String::is_empty)
            || components
                .insert(component.to_string(), solid_ids)
                .is_some()
        {
            return Err(format!(
                "invalid or duplicate --history-component {value:?}"
            ));
        }
    }
    Ok(components)
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
            deviation_bound_queries,
            deviation_bound_depth,
            stochastic_volume_samples,
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
                deviation_bound_queries,
                deviation_bound_depth,
                stochastic_volume_samples,
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
        DesignCommand::Transport {
            design,
            check_report,
            out,
            report,
            history_rates,
            target_error,
            max_histories,
            max_seconds,
            particles,
            seed,
            threads,
            window_python,
            openmc_package_path,
            group,
            history_component,
            history_scenario,
            h3_component,
            cache_dir,
            no_cache,
            tools,
        } => {
            let interrupts = crate::control::interrupt_cancellation()?;
            #[cfg(unix)]
            let cancellation = interrupts.cancellation.clone();
            #[cfg(not(unix))]
            let cancellation = interrupts.clone();
            let history_components = parse_history_components(&history_component)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
            let options = DesignTransportOptions {
                check_report,
                out_dir: out,
                target_relative_error: target_error,
                particles_per_batch: particles,
                max_histories,
                max_seconds,
                seed,
                threads,
                window_python,
                openmc_package_path,
                group,
                history_components: (!history_components.is_empty()).then_some(history_components),
                history_scenario,
                h3_component_id: h3_component,
                cache_dir: (!no_cache).then(|| {
                    cache_dir.unwrap_or_else(|| {
                        design
                            .parent()
                            .unwrap_or(Path::new("."))
                            .join("faris-transport-cache")
                    })
                }),
                progress: Some(transport_progress_printer()),
                cancellation,
            };
            let result = design_transport(&design, &tools.paths(), &options)?;
            print!("{}", render_transport_text(&result));
            if let Some(path) = report {
                write_json(&path, &result)?;
                println!("Report written to {}", path.display());
            }
            if result.target_reached {
                Ok(())
            } else {
                Err(Box::new(DesignCheckFailed(
                    "the transport run stopped before its target error".into(),
                )))
            }
        }
        DesignCommand::Heating {
            design,
            check_report,
            out,
            report,
            peak_voxel_m,
            max_histories,
            max_seconds,
            target_error,
            particles,
            seed,
            threads,
            tools,
        } => {
            let interrupts = crate::control::interrupt_cancellation()?;
            #[cfg(unix)]
            let cancellation = interrupts.cancellation.clone();
            #[cfg(not(unix))]
            let cancellation = interrupts.clone();
            let run_dir = out.clone();
            if history_rates.is_some()
                && report
                    .as_deref()
                    .is_some_and(|path| path == run_dir.join("history-energy-rates.json"))
            {
                return Err(
                    "--report cannot overwrite the generated history-energy-rates.json sidecar"
                        .into(),
                );
            }
            let options = DesignHeatingOptions {
                check_report,
                out_dir: out,
                peak_voxel_m: [peak_voxel_m[0], peak_voxel_m[1]],
                particles_per_batch: particles,
                max_histories,
                max_seconds,
                target_relative_error: target_error,
                progress: Some(std::sync::Arc::new(|update| {
                    let mean = update
                        .mean_relative_error
                        .map_or_else(|| "n/a".into(), |v| format!("{:.1}%", 100.0 * v));
                    let peak = update
                        .peak_relative_error
                        .map_or_else(|| "n/a".into(), |v| format!("{:.1}%", 100.0 * v));
                    eprintln!(
                        "Heating: {} histories, mean error {mean}, peak error {peak}, VOV {}",
                        update.histories,
                        update
                            .variance_of_variance
                            .map_or_else(|| "n/a".into(), |v| format!("{v:.3}"))
                    );
                })),
                seed,
                threads,
                cancellation,
            };
            let tool_paths = tools.paths();
            let result = if let Some(base_path) = history_rates.as_deref() {
                design_heating_with_history_rates(&design, &tool_paths, &options, base_path)?
            } else {
                design_heating(&design, &tool_paths, &options)?
            };
            if history_rates.is_some()
                && let Some(path) = report.as_deref()
                && let Some(name) = path.file_name()
                && path
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."))
                    .canonicalize()
                    .ok()
                    .is_some_and(|parent| {
                        run_dir.canonicalize().ok().is_some_and(|run| {
                            parent.join(name) == run.join("history-energy-rates.json")
                        })
                    })
            {
                return Err(
                    "--report cannot overwrite the generated history-energy-rates.json sidecar"
                        .into(),
                );
            }
            print!("{}", render_design_heating_text(&result));
            if let Some(base_path) = history_rates.as_deref()
                && result
                    .history_energy_response
                    .as_ref()
                    .is_some_and(|response| {
                        response.status == "CALCULATED" && response.target_reached
                    })
            {
                let base_bytes = std::fs::read(base_path)?;
                let base: DesignHistoryRatesArtifact = serde_json::from_slice(&base_bytes)?;
                let scenario = faris_model::LoadedScenario::load(Path::new(&base.scenario_path))?;
                let energy_path = run_dir.join("history-energy-rates.json");
                combine_history_energy_rates(
                    base_path,
                    &run_dir.join("report.json"),
                    &scenario,
                    &energy_path,
                )?;
                println!(
                    "Whole-model history-energy rates written to {}",
                    energy_path.display()
                );
            }
            if let Some(path) = report {
                write_json(&path, &result)?;
                println!("Report written to {}", path.display());
            }
            if result.status == "CALCULATED" {
                Ok(())
            } else {
                Err(Box::new(DesignCheckFailed(result.why.unwrap_or_else(
                    || "coupled heating was not evaluated".into(),
                ))))
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

/// Progress lines on standard error: each step as it starts, and the error reached while sampling.
fn transport_progress_printer() -> faris_engine::design::TransportProgressFn {
    std::sync::Arc::new(|update| eprintln!("  {}", faris_engine::design::progress_line(&update)))
}

fn write_report(path: &Path, report: &ImportReport) -> Result<(), Box<dyn std::error::Error>> {
    write_json(path, report)
}

fn write_json<T: serde::Serialize>(
    path: &Path,
    report: &T,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = serde_json::to_vec_pretty(report)?;
    bytes.push(b'\n');
    faris_engine::atomic_file::write_new(path, &bytes).map_err(|e| {
        format!(
            "{}: {e} (an existing report is never overwritten)",
            path.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_component_cli_values_make_an_explicit_stable_map() {
        let map =
            parse_history_components(&["blanket=blanket".into(), "tf-coil=coil-a,coil-b".into()])
                .unwrap();
        assert_eq!(map["blanket"], vec!["blanket".to_string()]);
        assert_eq!(
            map["tf-coil"],
            vec!["coil-a".to_string(), "coil-b".to_string()]
        );
        assert!(parse_history_components(&["bad".into()]).is_err());
        assert!(parse_history_components(&["blanket=a".into(), "blanket=b".into()]).is_err());
        assert!(parse_history_components(&["wall=a,,b".into()]).is_err());
    }
}
