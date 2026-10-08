//! Running the maintenance coupling from files: the designs file, the assumptions file and the
//! verified transport runs they name. `faris maintenance run` and the desktop both call
//! [`run_from_files`]; the module does its own bounded reads and no printing.

use super::actinv::{ActinvDecaySource, ActinvSourceConfig, DesignFiles, SourceCounters};
use super::{DecaySourceRecord, DesignInput, MaintenanceResult, ProducedBy, run_maintenance};
use crate::history::{HistoryResult, TransportDrivingRates, run_operating_history_cancellable};
use crate::jobs::Cancellation;
use crate::reactor::load_reactor_run;
use faris_model::{
    LoadedScenario,
    history::OperatingHistoryAssumptions,
    maintenance::{MaintenanceAssumptions, Threshold},
    transport::MAX_ARTIFACT_BYTES,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub const DESIGNS_VERSION: &str = "faris-maintenance-designs/v0.1";

// ------------------------------------------------------------------ builder --

/// File name of the activation-input builder script.
const BUILDER_FILE: &str = "build_activation_inputs.py";

/// What is reported when no builder script is found, and the next step.
pub const BUILDER_NOT_FOUND: &str = "activation-input builder not found. Next step: pass --builder \
    (or set the builder under Advanced) with the path of build_activation_inputs.py.";

/// The default builder script: `tools/build_activation_inputs.py` beside the running program's
/// folder (a downloaded package keeps programs in `bin/` and the script in `tools/`), otherwise
/// the script in the source repository this program was built from.
pub fn default_builder() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts")
        .join(BUILDER_FILE);
    builder_search(exe_dir.as_deref(), &repository)
}

/// [`default_builder`] with the program's folder and the repository script supplied.
pub fn builder_search(exe_dir: Option<&Path>, repository_script: &Path) -> Option<PathBuf> {
    exe_dir
        .map(|dir| dir.join("..").join("tools").join(BUILDER_FILE))
        .filter(|path| path.is_file())
        .or_else(|| {
            repository_script
                .is_file()
                .then(|| repository_script.to_path_buf())
        })
}

// ------------------------------------------------------------------ designs --

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DesignsFile {
    schema_version: String,
    designs: BTreeMap<String, DesignEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DesignEntry {
    scenario: PathBuf,
    physics: PathBuf,
    history_run: PathBuf,
    spectrum_run: PathBuf,
    history_assumptions: PathBuf,
}

/// One design with its five files resolved against the designs file's folder.
#[derive(Debug, PartialEq)]
pub struct ResolvedDesign {
    pub scenario: PathBuf,
    pub physics: PathBuf,
    pub history_run: PathBuf,
    pub spectrum_run: PathBuf,
    pub history_assumptions: PathBuf,
}

impl ResolvedDesign {
    pub fn files(&self) -> [(&'static str, &Path); 5] {
        [
            ("scenario", &self.scenario),
            ("physics", &self.physics),
            ("history_run", &self.history_run),
            ("spectrum_run", &self.spectrum_run),
            ("history_assumptions", &self.history_assumptions),
        ]
    }
}

/// Parses and checks a designs file; every named file must exist.
pub fn parse_designs(
    bytes: &[u8],
    base: &Path,
) -> Result<BTreeMap<String, ResolvedDesign>, String> {
    let file: DesignsFile = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if file.schema_version != DESIGNS_VERSION {
        return Err(format!(
            "designs file schema_version must be {DESIGNS_VERSION}"
        ));
    }
    if file.designs.is_empty() {
        return Err("designs file names no design".into());
    }
    let mut out = BTreeMap::new();
    for (name, entry) in file.designs {
        if name.trim().is_empty() {
            return Err("design names must be nonempty".into());
        }
        let resolved = ResolvedDesign {
            scenario: base.join(entry.scenario),
            physics: base.join(entry.physics),
            history_run: base.join(entry.history_run),
            spectrum_run: base.join(entry.spectrum_run),
            history_assumptions: base.join(entry.history_assumptions),
        };
        for (what, path) in resolved.files() {
            if !path.is_file() {
                return Err(format!(
                    "design {name}: {what} {} is not a file",
                    path.display()
                ));
            }
        }
        out.insert(name, resolved);
    }
    Ok(out)
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let io = |e: std::io::Error| e.to_string();
    if !std::fs::metadata(path).map_err(io)?.is_file() {
        return Err("input must be a regular file".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(io)?
        .take((MAX_ARTIFACT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() > MAX_ARTIFACT_BYTES {
        return Err("input exceeds 16 MiB limit".into());
    }
    Ok(bytes)
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let io = |e: std::io::Error| e.to_string();
    let mut file = std::fs::File::open(path).map_err(io)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buffer).map_err(io)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn resolved(program: &Path) -> Result<PathBuf, String> {
    if program.components().count() > 1 || program.is_absolute() {
        return std::path::absolute(program).map_err(|e| e.to_string());
    }
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|d| d.join(program))
                .find(|p| p.is_file())
        })
        .ok_or_else(|| format!("executable {} not found", program.display()))
}

// -------------------------------------------------------------------- rates --

/// Transport-driving rates bound from a revalidated successful fixed-source run record.
pub fn rates_from_run(
    scenario: &LoadedScenario,
    run_path: &Path,
) -> Result<TransportDrivingRates, String> {
    let run = load_reactor_run(run_path, scenario).map_err(|e| e.to_string())?;
    let normalized = run
        .normalized
        .as_ref()
        .ok_or("verified run has no normalized result")?;
    TransportDrivingRates::from_normalized(
        normalized,
        scenario.scenario.operating_plan.fusion_power_mw,
        run.raw_artifact_sha256
            .as_deref()
            .ok_or("verified run lacks raw artifact identity")?,
    )
    .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------- run --

/// Everything a run reads and where it works. Paths of the designs file's designs are
/// relative to that file.
#[derive(Clone, Debug)]
pub struct RunConfig {
    pub designs: PathBuf,
    pub assumptions: PathBuf,
    pub actinv: PathBuf,
    pub data_dir: PathBuf,
    /// Content-addressed ACTINV points cache [default: `decay-cache` inside the work folder].
    pub cache: Option<PathBuf>,
    /// Per-design, per-iteration spec folders and scratch space.
    pub work_dir: PathBuf,
    pub workers: usize,
    pub impurities: Option<PathBuf>,
    pub python: PathBuf,
    pub builder: PathBuf,
}

/// One progress report. [`RunProgress::line`] is the text `faris maintenance run` prints.
#[derive(Clone, Debug, PartialEq)]
pub struct RunProgress {
    pub design: Option<String>,
    /// 0 outside an iteration (fixed-duration history, binding rates, calibration).
    pub iteration: u32,
    pub message: String,
    /// Running totals of the decay source.
    pub actinv_runs: u64,
    pub cache_hits: u64,
}

impl RunProgress {
    pub fn line(&self) -> String {
        match (&self.design, self.iteration) {
            (None, _) => self.message.clone(),
            (Some(design), 0) => format!("{design}: {}", self.message),
            (Some(design), n) => format!("{design}: iteration {n}: {}", self.message),
        }
    }
}

pub type ProgressFn = Arc<dyn Fn(RunProgress) + Send + Sync>;

/// Runs the fixed and the coupled model for every design named by the files. `Err` is bad
/// input, a tool failure or cancellation (the message is "cancelled"); nothing is written.
pub fn run_from_files(
    config: &RunConfig,
    cancel: &Cancellation,
    progress: ProgressFn,
) -> Result<MaintenanceResult, String> {
    run_from_files_with(config, &rates_from_run, cancel, progress)
}

/// [`run_from_files`] with the transport-rate binding supplied.
pub fn run_from_files_with(
    config: &RunConfig,
    rates_for: &dyn Fn(&LoadedScenario, &Path) -> Result<TransportDrivingRates, String>,
    cancel: &Cancellation,
    progress: ProgressFn,
) -> Result<MaintenanceResult, String> {
    let counters = Arc::new(SourceCounters::default());
    let report = {
        let counters = counters.clone();
        move |design: Option<&str>, iteration: u32, message: &str| -> RunProgress {
            RunProgress {
                design: design.map(str::to_owned),
                iteration,
                message: message.to_owned(),
                actinv_runs: counters.actinv_runs.load(Ordering::Relaxed),
                cache_hits: counters.cache_hits.load(Ordering::Relaxed),
            }
        }
    };

    let assumptions: MaintenanceAssumptions =
        serde_json::from_slice(&read_bounded(&config.assumptions)?).map_err(|e| e.to_string())?;
    assumptions.validate()?;
    let designs_base = config
        .designs
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let designs = parse_designs(&read_bounded(&config.designs)?, designs_base)?;
    for spec in assumptions.classes.values() {
        if let Threshold::Calibrate { design, .. } = &spec.threshold
            && !designs.contains_key(design)
        {
            return Err(format!(
                "a class calibrates on design {design}, which the designs file does not name"
            ));
        }
    }

    // Rates from the verified history runs, and the fixed-duration assumptions.
    let mut rates: BTreeMap<String, TransportDrivingRates> = BTreeMap::new();
    let mut inputs_for_run = Vec::new();
    let mut files: BTreeMap<String, DesignFiles> = BTreeMap::new();
    for (name, design) in &designs {
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }
        progress(report(
            Some(name),
            0,
            "binding transport rates from the history run",
        ));
        let scenario = LoadedScenario::from_bytes(&read_bounded(&design.scenario)?)
            .map_err(|e| e.to_string())?;
        rates.insert(name.clone(), rates_for(&scenario, &design.history_run)?);
        let history_assumptions: OperatingHistoryAssumptions =
            serde_json::from_slice(&read_bounded(&design.history_assumptions)?)
                .map_err(|e| e.to_string())?;
        history_assumptions
            .validate()
            .map_err(|e| format!("design {name}: {e}"))?;
        inputs_for_run.push(DesignInput {
            name: name.clone(),
            history_assumptions,
        });
        files.insert(
            name.clone(),
            DesignFiles {
                scenario: design.scenario.clone(),
                physics: design.physics.clone(),
                history_run: design.history_run.clone(),
                spectrum_run: design.spectrum_run.clone(),
            },
        );
    }

    // Input identity: what the result depends on, by SHA-256.
    let mut inputs = BTreeMap::new();
    inputs.insert("assumptions".to_string(), sha256_file(&config.assumptions)?);
    inputs.insert("designs".to_string(), sha256_file(&config.designs)?);
    for (name, design) in &designs {
        for (what, path) in design.files() {
            inputs.insert(format!("design/{name}/{what}"), sha256_file(path)?);
        }
    }

    let cache = config
        .cache
        .clone()
        .unwrap_or_else(|| config.work_dir.join("decay-cache"));
    let mut source_config = ActinvSourceConfig::new(
        config.builder.clone(),
        config.actinv.clone(),
        config.data_dir.clone(),
        cache,
        config.work_dir.clone(),
    );
    source_config.python = config.python.clone();
    source_config.workers = config.workers;
    source_config.impurities = config.impurities.clone();
    let log_report = report.clone();
    let log_progress = progress.clone();
    let mut source = ActinvDecaySource::new(source_config, files)?
        .with_counters(counters.clone())
        .with_log(Box::new(move |line| {
            log_progress(log_report(None, 0, line));
        }));
    // The tools are hashed after they are found, so a bad path is reported first.
    inputs.insert(
        "actinv".to_string(),
        sha256_file(&resolved(&config.actinv)?)?,
    );
    inputs.insert("builder".to_string(), sha256_file(&config.builder)?);
    if let Some(path) = &config.impurities {
        inputs.insert("impurities".to_string(), sha256_file(path)?);
    }

    let mut runner = |design: &str,
                      assumptions: &OperatingHistoryAssumptions,
                      cancel: &Cancellation|
     -> Result<HistoryResult, String> {
        let rates = rates
            .get(design)
            .ok_or_else(|| format!("no transport rates for design {design}"))?;
        run_operating_history_cancellable(assumptions, rates, cancel)
    };
    let mut result = run_maintenance(
        &assumptions,
        &inputs_for_run,
        &mut runner,
        &mut source,
        cancel,
        &mut |design, iteration, message| progress(report(Some(design), iteration, message)),
    )?;
    let (actinv_runs, cache_hits) = source.stats();
    result.inputs = inputs;
    result.produced_by = Some(ProducedBy {
        faris_version: env!("CARGO_PKG_VERSION").into(),
    });
    result.decay_source = Some(DecaySourceRecord {
        kind: "actinv-continuations".into(),
        actinv_runs,
        cache_hits,
    });
    Ok(result)
}

#[cfg(test)]
mod tests;
