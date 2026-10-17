//! `faris activation`: activation inventory, decay heat and decay-photon source through ACTINV.
use crate::transport::{read_bounded, write_new_json};
use clap::Subcommand;
use faris_engine::activation::{
    ActivationConfig, ActivationData, ActivationRequest, PhotonOptions, PhotonResponse, SolverMode,
    ZoneSpec, decade_cooling_grid, limit_nuclides, log_photon_groups, parse_cooling_list,
    photon_source_file, run_activation, write_photon_source_file, zones_from_spectra,
};
use faris_engine::impurity_sensitivity::{SensitivityRequest, impurity_sensitivity};
use faris_engine::maintenance::irradiation::IrradiationSchedule;
use faris_engine::transport::NormalizedEnergySpectrum;
use std::path::PathBuf;

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum ModeArgument {
    Auto,
    Trace,
    Coupled,
}

#[derive(Subcommand)]
pub enum ActivationCommand {
    /// Run ACTINV on zones with group fluxes and an irradiation schedule; writes a JSON result
    /// with inventory, activity, decay heat, decay-photon source, pathways and data gaps.
    Run {
        /// A complete request (faris-activation-request/v0.1: zones with spectra, steps, cooling
        /// grid). Or give --zones, --spectra and --schedule instead.
        #[arg(long, conflicts_with_all = ["zones", "spectra", "schedule"])]
        request: Option<PathBuf>,
        /// JSON list of zones: id, component_id, mass_g, volume_m3, basis, composition.
        #[arg(long, requires_all = ["spectra", "schedule"])]
        zones: Option<PathBuf>,
        /// Normalized neutron spectra of the transport run (a list, or an object with
        /// `normalized_spectra`), 709 groups, n per m2 per s.
        #[arg(long)]
        spectra: Option<PathBuf>,
        /// An irradiation schedule file (faris-irradiation-schedule/v0.1).
        #[arg(long)]
        schedule: Option<PathBuf>,
        /// Cooling times since shutdown, `1s,1h,1d,1y`; 1 s to 1e9 s [default: one per decade].
        #[arg(long)]
        cooling: Option<String>,
        /// Number of decay-photon groups (log-spaced to 20 MeV); default is ACTINV's 24 groups.
        #[arg(long)]
        photon_groups: Option<usize>,
        /// An actinv-photon-response-1 file, for the contact dose proxy; needs its SHA-256.
        #[arg(long, requires = "photon_response_sha256")]
        photon_response: Option<PathBuf>,
        #[arg(long)]
        photon_response_sha256: Option<String>,
        /// ACTINV solution mode. `trace` (and `auto` when it picks trace) holds the starting
        /// material constant; use `coupled` when the material itself contains radionuclides.
        #[arg(long, value_enum)]
        mode: Option<ModeArgument>,
        /// The `actinv` executable.
        #[arg(long)]
        actinv: PathBuf,
        /// ACTINV data root (the folder above the catalogue version).
        #[arg(long)]
        data_dir: PathBuf,
        /// New result file (faris-activation-result/v0.1); existing paths are refused.
        #[arg(long)]
        output: PathBuf,
        /// Content-addressed zone results; an identical request is not run again.
        #[arg(long)]
        cache: Option<PathBuf>,
        /// Scratch folder for ACTINV problems [default: OUTPUT.work].
        #[arg(long)]
        work_dir: Option<PathBuf>,
        /// Keep this many nuclides per cooling point, most active first (totals stay complete).
        #[arg(long, default_value_t = 50)]
        top_nuclides: usize,
        /// Also write the decay-photon source (photons/s/cm3) of every zone and cooling time here.
        #[arg(long)]
        photon_source_dir: Option<PathBuf>,
    },
    /// Rank impurity elements by their effect on activity, decay heat and 10 CFR 61.55 waste
    /// class, with a finite-difference check of every sensitivity. One ACTINV request holds all
    /// the runs (matrix, full specification, one element alone, two steps per element).
    Impurities {
        /// A faris-impurity-sensitivity request: matrix, impurities (wt ppm with kind and
        /// source), mass, volume, 709-group spectrum, steps, cooling times.
        #[arg(long)]
        request: PathBuf,
        /// The `actinv` executable.
        #[arg(long)]
        actinv: PathBuf,
        /// ACTINV data root (the folder above the catalogue version).
        #[arg(long)]
        data_dir: PathBuf,
        /// New result file (faris-impurity-sensitivity/v0.1); existing paths are refused.
        #[arg(long)]
        output: PathBuf,
        /// Content-addressed zone results; an identical request is not run again.
        #[arg(long)]
        cache: Option<PathBuf>,
        /// Scratch folder [default: OUTPUT.work].
        #[arg(long)]
        work_dir: Option<PathBuf>,
    },
}

pub fn run(command: ActivationCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        ActivationCommand::Run {
            request,
            zones,
            spectra,
            schedule,
            cooling,
            photon_groups,
            photon_response,
            photon_response_sha256,
            mode,
            actinv,
            data_dir,
            output,
            cache,
            work_dir,
            top_nuclides,
            photon_source_dir,
        } => {
            if output.exists() {
                return Err(format!("refusing to overwrite {}", output.display()).into());
            }
            let photon = PhotonOptions {
                group_boundaries_ev: photon_groups.map(log_photon_groups).transpose()?,
                response: match (photon_response, photon_response_sha256) {
                    (Some(path), Some(sha256)) => Some(PhotonResponse { path, sha256 }),
                    _ => None,
                },
            };
            let cooling_override = cooling.map(|text| parse_cooling_list(&text)).transpose()?;
            let mut req = match (request, zones, spectra, schedule) {
                (Some(path), ..) => {
                    let mut r: ActivationRequest = serde_json::from_slice(&read_bounded(&path)?)?;
                    if let Some(grid) = cooling_override {
                        r.cooling_s = grid;
                    }
                    if photon != PhotonOptions::default() {
                        r.photon = photon.clone();
                    }
                    r
                }
                (None, Some(zones), Some(spectra), Some(schedule)) => {
                    let specs: Vec<ZoneSpec> = serde_json::from_slice(&read_bounded(&zones)?)?;
                    let doc: serde_json::Value = serde_json::from_slice(&read_bounded(&spectra)?)?;
                    let list = doc.get("normalized_spectra").cloned().unwrap_or(doc);
                    let spectra: Vec<NormalizedEnergySpectrum> = serde_json::from_value(list)?;
                    let schedule: IrradiationSchedule =
                        serde_json::from_slice(&read_bounded(&schedule)?)?;
                    ActivationRequest::from_schedule(
                        zones_from_spectra(&specs, &spectra)?,
                        &schedule,
                        cooling_override.unwrap_or_else(decade_cooling_grid),
                        photon,
                    )?
                }
                _ => return Err("give --request, or --zones with --spectra and --schedule".into()),
            };
            if let Some(mode) = mode {
                req.mode = match mode {
                    ModeArgument::Auto => SolverMode::Auto,
                    ModeArgument::Trace => SolverMode::Trace,
                    ModeArgument::Coupled => SolverMode::Coupled,
                };
            }
            req.validate()?;
            let work_dir = work_dir.unwrap_or_else(|| {
                let mut name = output.as_os_str().to_os_string();
                name.push(".work");
                PathBuf::from(name)
            });
            let mut config =
                ActivationConfig::new(actinv, ActivationData::catalog(&data_dir), work_dir);
            config.cache_dir = cache;
            let interrupts = crate::control::interrupt_cancellation()?;
            #[cfg(unix)]
            let cancellation = &interrupts.cancellation;
            #[cfg(not(unix))]
            let cancellation = &interrupts;
            let (mut result, stats) = run_activation(&req, &config, cancellation)?;
            if let Some(dir) = &photon_source_dir {
                std::fs::create_dir_all(dir)?;
                for zone in &result.zones {
                    for point in &zone.points {
                        let file = photon_source_file(&result, &zone.id, point.cooling_s)?;
                        let path = dir.join(format!("{}__{}s.json", zone.id, point.cooling_s));
                        let sha = write_photon_source_file(&path, &file)?;
                        println!("photon source {} sha256 {sha}", path.display());
                    }
                }
            }
            limit_nuclides(&mut result, top_nuclides);
            write_new_json(&output, &result)?;
            for zone in &result.zones {
                println!(
                    "{}: {} cooling times, data {:?}, {} data gaps",
                    zone.id,
                    zone.points.len(),
                    zone.completeness,
                    zone.gaps.len()
                );
            }
            println!(
                "Activation result recorded at {} ({} ACTINV runs, {} cached zones)",
                output.display(),
                stats.actinv_runs,
                stats.cache_hits
            );
            eprintln!("{}", faris_model::RESEARCH_SCREENING_STATEMENT);
            Ok(())
        }
        ActivationCommand::Impurities {
            request,
            actinv,
            data_dir,
            output,
            cache,
            work_dir,
        } => {
            if output.exists() {
                return Err(format!("refusing to overwrite {}", output.display()).into());
            }
            let req: SensitivityRequest = serde_json::from_slice(&read_bounded(&request)?)?;
            let work_dir = work_dir.unwrap_or_else(|| {
                let mut name = output.as_os_str().to_os_string();
                name.push(".work");
                PathBuf::from(name)
            });
            let mut config =
                ActivationConfig::new(actinv, ActivationData::catalog(&data_dir), work_dir);
            config.cache_dir = cache;
            let interrupts = crate::control::interrupt_cancellation()?;
            #[cfg(unix)]
            let cancellation = &interrupts.cancellation;
            #[cfg(not(unix))]
            let cancellation = &interrupts;
            let result = impurity_sensitivity(&req, &config, cancellation)?;
            write_new_json(&output, &result)?;
            for p in &result.points {
                let first = |r: &[faris_engine::impurity_sensitivity::RankLine]| {
                    r.first().map_or("none".to_string(), |l| l.element.clone())
                };
                println!(
                    "cooling {} s: largest effect on activity {}, decay heat {}, waste class {}; finite-difference check {}",
                    p.cooling_s,
                    first(&p.activity_ranking),
                    first(&p.decay_heat_ranking),
                    first(&p.waste_ranking),
                    if p.agreement.iter().all(|a| a.passed) {
                        "passed"
                    } else {
                        "FAILED"
                    }
                );
            }
            println!(
                "Impurity sensitivity recorded at {} ({} ACTINV runs)",
                output.display(),
                result.runs
            );
            eprintln!("{}", faris_model::RESEARCH_SCREENING_STATEMENT);
            Ok(())
        }
    }
}
