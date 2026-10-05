//! Explicit operating-history execution from reviewed JSON inputs.
use clap::Subcommand;
use faris_engine::{
    comparison::{
        HistorySensitivityGrid, compare_histories_cancellable, run_history_sensitivity_cancellable,
    },
    history::{TransportDrivingRates, run_operating_history_cancellable},
    reactor::load_reactor_run,
};
use faris_model::history::OperatingHistoryAssumptions;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum HistoryCommand {
    Run {
        #[arg(long)]
        assumptions: PathBuf,
        #[arg(long)]
        rates: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Validate {
        #[arg(long)]
        assumptions: PathBuf,
        #[arg(long)]
        rates: PathBuf,
    },
    /// Bind rates from a revalidated successful fixed-source run and calculate history.
    FromRun {
        #[arg(long)]
        scenario: PathBuf,
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        assumptions: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        rates_output: Option<PathBuf>,
    },
    /// Ensemble of histories on transport rates sampled from their recorded
    /// covariance; carries transport Monte Carlo uncertainty only.
    Ensemble {
        #[arg(long)]
        assumptions: PathBuf,
        #[arg(long)]
        rates: PathBuf,
        /// Number of sampled histories (1 to 2000).
        #[arg(long, default_value_t = faris_engine::history_ensemble::DEFAULT_SAMPLES)]
        samples: u32,
        /// Random seed; default derives from the transport artifact and assumptions.
        #[arg(long)]
        seed: Option<u64>,
        /// Worker threads; default is the available parallelism minus one.
        #[arg(long)]
        threads: Option<usize>,
        #[arg(long)]
        output: PathBuf,
    },
    CompareRuns {
        #[arg(long)]
        scenario: PathBuf,
        #[arg(long)]
        left_run: PathBuf,
        #[arg(long)]
        left_label: String,
        #[arg(long)]
        right_run: PathBuf,
        #[arg(long)]
        right_label: String,
        #[arg(long)]
        assumptions: PathBuf,
        #[arg(long)]
        controlled_difference: String,
        #[arg(long)]
        output: PathBuf,
    },
    Sensitivity {
        #[arg(long)]
        scenario: PathBuf,
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        assumptions: PathBuf,
        #[arg(long)]
        grid: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
}

pub fn run(command: HistoryCommand) -> Result<(), Box<dyn std::error::Error>> {
    let (assumptions_path, rates_path, output) = match command {
        HistoryCommand::Run {
            assumptions,
            rates,
            output,
        } => (assumptions, rates, Some(output)),
        HistoryCommand::Validate { assumptions, rates } => (assumptions, rates, None),
        HistoryCommand::FromRun {
            scenario,
            run,
            assumptions,
            output,
            rates_output,
        } => {
            return from_run(scenario, run, assumptions, output, rates_output);
        }
        HistoryCommand::Ensemble {
            assumptions,
            rates,
            samples,
            seed,
            threads,
            output,
        } => {
            return ensemble(
                assumptions,
                rates,
                faris_engine::history_ensemble::EnsembleSettings {
                    samples,
                    seed,
                    threads,
                },
                output,
            );
        }
        HistoryCommand::CompareRuns {
            scenario,
            left_run,
            left_label,
            right_run,
            right_label,
            assumptions,
            controlled_difference,
            output,
        } => {
            return compare_runs(CompareRunRequest {
                scenario,
                left_run,
                left_label,
                right_run,
                right_label,
                assumptions,
                controlled_difference,
                output,
            });
        }
        HistoryCommand::Sensitivity {
            scenario,
            run,
            assumptions,
            grid,
            output,
        } => {
            return sensitivity(scenario, run, assumptions, grid, output);
        }
    };
    let assumptions: OperatingHistoryAssumptions =
        serde_json::from_slice(&crate::transport::read_bounded(&assumptions_path)?)?;
    assumptions.validate().map_err(std::io::Error::other)?;
    let rates: TransportDrivingRates =
        serde_json::from_slice(&crate::transport::read_bounded(&rates_path)?)?;
    rates.validate().map_err(std::io::Error::other)?;
    if let Some(output) = output {
        ensure_new_outputs(&output, None)?;
        let interrupts = crate::control::interrupt_cancellation()?;
        #[cfg(unix)]
        let cancellation = &interrupts.cancellation;
        #[cfg(not(unix))]
        let cancellation = &interrupts;
        let result = run_operating_history_cancellable(&assumptions, &rates, cancellation)
            .map_err(std::io::Error::other)?;
        write_new_json(&output, &result)?;
        println!(
            "History recorded at {} ({:?})",
            output.display(),
            result.outcome
        );
        announce_research_screening();
    } else {
        println!("Assumptions and transport driving rates are valid; no history was computed.");
    }
    Ok(())
}

fn ensemble(
    assumptions_path: PathBuf,
    rates_path: PathBuf,
    settings: faris_engine::history_ensemble::EnsembleSettings,
    output: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    use faris_engine::history_ensemble::{EnsembleStatus, run_history_ensemble};
    ensure_new_outputs(&output, None)?;
    let assumptions: OperatingHistoryAssumptions =
        serde_json::from_slice(&crate::transport::read_bounded(&assumptions_path)?)?;
    assumptions.validate().map_err(std::io::Error::other)?;
    let rates: TransportDrivingRates =
        serde_json::from_slice(&crate::transport::read_bounded(&rates_path)?)?;
    rates.validate().map_err(std::io::Error::other)?;
    let interrupts = crate::control::interrupt_cancellation()?;
    #[cfg(unix)]
    let cancellation = &interrupts.cancellation;
    #[cfg(not(unix))]
    let cancellation = &interrupts;
    let progress = |done: usize, total: usize| {
        if done == total || done.is_multiple_of((total / 10).max(1)) {
            eprintln!("ensemble: {done}/{total} histories");
        }
    };
    let result = run_history_ensemble(&rates, &assumptions, &settings, cancellation, &progress)
        .map_err(std::io::Error::other)?;
    write_new_json(&output, &result)?;
    announce_research_screening();
    match &result.status {
        EnsembleStatus::Evaluated => println!(
            "History ensemble recorded at {} ({} samples, seed {}, {} rejected draws)",
            output.display(),
            result.samples_accepted,
            result.seed,
            result.rejections
        ),
        EnsembleStatus::NotEvaluated { why, next_step } => println!(
            "History ensemble NOT EVALUATED, recorded at {}: {why}. Next step: {next_step}.",
            output.display()
        ),
    }
    Ok(())
}

fn from_run(
    scenario_path: PathBuf,
    run_path: PathBuf,
    assumptions_path: PathBuf,
    output: PathBuf,
    rates_output: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    ensure_new_outputs(&output, rates_output.as_deref())?;
    let scenario =
        faris_model::LoadedScenario::from_bytes(&crate::transport::read_bounded(&scenario_path)?)?;
    let run = load_reactor_run(&run_path, &scenario).map_err(std::io::Error::other)?;
    let normalized = run
        .normalized
        .as_ref()
        .ok_or("verified run has no normalized transport result")?;
    let rates = TransportDrivingRates::from_normalized(
        normalized,
        scenario.scenario.operating_plan.fusion_power_mw,
        run.raw_artifact_sha256
            .as_deref()
            .ok_or("verified run lacks raw transport artifact identity")?,
    )
    .map_err(std::io::Error::other)?;
    let assumptions: OperatingHistoryAssumptions =
        serde_json::from_slice(&crate::transport::read_bounded(&assumptions_path)?)?;
    assumptions.validate().map_err(std::io::Error::other)?;
    let interrupts = crate::control::interrupt_cancellation()?;
    #[cfg(unix)]
    let cancellation = &interrupts.cancellation;
    #[cfg(not(unix))]
    let cancellation = &interrupts;
    let result = run_operating_history_cancellable(&assumptions, &rates, cancellation)
        .map_err(std::io::Error::other)?;
    write_new_json(&output, &result)?;
    if let Some(path) = rates_output {
        write_new_json(&path, &rates)?;
    }
    println!(
        "History recorded at {} ({:?}); source run {}",
        output.display(),
        result.outcome,
        run_path.display()
    );
    announce_research_screening();
    Ok(())
}

/// The research-screening statement on stderr after a result file is written.
/// The result files are not changed: `HistoryResult.notice` is part of
/// receipt-bound bytes, and a comparison's own bytes are hashed into its
/// provenance record.
fn announce_research_screening() {
    eprintln!("{}", faris_model::RESEARCH_SCREENING_STATEMENT);
}

fn write_new_json<T: serde::Serialize>(
    path: &std::path::Path,
    value: &T,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    faris_engine::core_evidence::write_new(path, &bytes).map_err(std::io::Error::other)?;
    Ok(())
}

fn rates_from_run(
    scenario: &faris_model::LoadedScenario,
    run_path: &std::path::Path,
) -> Result<TransportDrivingRates, Box<dyn std::error::Error>> {
    let run = load_reactor_run(run_path, scenario).map_err(std::io::Error::other)?;
    let normalized = run
        .normalized
        .as_ref()
        .ok_or("verified run has no normalized result")?;
    Ok(TransportDrivingRates::from_normalized(
        normalized,
        scenario.scenario.operating_plan.fusion_power_mw,
        run.raw_artifact_sha256
            .as_deref()
            .ok_or("verified run lacks raw artifact identity")?,
    )
    .map_err(std::io::Error::other)?)
}

struct CompareRunRequest {
    scenario: PathBuf,
    left_run: PathBuf,
    left_label: String,
    right_run: PathBuf,
    right_label: String,
    assumptions: PathBuf,
    controlled_difference: String,
    output: PathBuf,
}

fn compare_runs(request: CompareRunRequest) -> Result<(), Box<dyn std::error::Error>> {
    let CompareRunRequest {
        scenario: scenario_path,
        left_run,
        left_label,
        right_run,
        right_label,
        assumptions: assumptions_path,
        controlled_difference,
        output,
    } = request;
    let provenance_output = append_suffix(&output, ".provenance.json");
    ensure_new_outputs(&output, Some(&provenance_output))?;
    let scenario =
        faris_model::LoadedScenario::from_bytes(&crate::transport::read_bounded(&scenario_path)?)?;
    let assumptions: OperatingHistoryAssumptions =
        serde_json::from_slice(&crate::transport::read_bounded(&assumptions_path)?)?;
    let left_provenance = comparison_run_provenance(&left_run)?;
    let right_provenance = comparison_run_provenance(&right_run)?;
    let left = rates_from_run(&scenario, &left_run)?;
    let right = rates_from_run(&scenario, &right_run)?;
    let interrupts = crate::control::interrupt_cancellation()?;
    #[cfg(unix)]
    let cancellation = &interrupts.cancellation;
    #[cfg(not(unix))]
    let cancellation = &interrupts;
    let paired = compare_histories_cancellable(
        &assumptions,
        &left_label,
        &left,
        &right_label,
        &right,
        &controlled_difference,
        cancellation,
    )
    .map_err(std::io::Error::other)?;
    write_new_json(&output, &paired)?;
    let mut comparison_bytes = serde_json::to_vec_pretty(&paired)?;
    comparison_bytes.push(b'\n');
    let provenance = serde_json::json!({
        "schema_version": "faris-history-comparison-provenance/v0.1",
        "comparison_path": output,
        "comparison_sha256": crate::transport::sha256(&comparison_bytes),
        "assumptions_sha256": crate::transport::sha256(&crate::transport::read_bounded(&assumptions_path)?),
        "scenario_sha256": scenario.source_sha256,
        "left_run": left_provenance,
        "right_run": right_provenance,
        "scope": "Exact run-record byte hashes and recorded sampler settings bind the paired deterministic comparison to its two transport records. Same-seed runs are not independent replicates; no covariance is estimated."
    });
    write_new_json(&provenance_output, &provenance)?;
    println!("Paired history comparison recorded at {}", output.display());
    announce_research_screening();
    Ok(())
}

fn append_suffix(path: &std::path::Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn comparison_run_provenance(
    path: &std::path::Path,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let bytes = crate::transport::read_bounded(path)?;
    let record: serde_json::Value = serde_json::from_slice(&bytes)?;
    let migration_path = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."))
        .join("preflight-migration.json");
    let migration = if migration_path.is_file() {
        let migration_bytes = crate::transport::read_bounded(&migration_path)?;
        Some(serde_json::json!({
            "path": migration_path,
            "sha256": crate::transport::sha256(&migration_bytes)
        }))
    } else {
        None
    };
    let sampling = record
        .get("sampling")
        .cloned()
        .ok_or("run record lacks recorded sampling plan")?;
    let raw_artifact_sha256 = record
        .get("raw_artifact_sha256")
        .and_then(serde_json::Value::as_str)
        .ok_or("run record lacks raw artifact identity")?;
    Ok(serde_json::json!({
        "path": path,
        "run_record_sha256": crate::transport::sha256(&bytes),
        "raw_artifact_sha256": raw_artifact_sha256,
        "sampling": sampling,
        "scenario_sha256": record.get("scenario_sha256"),
        "variant_id": record.get("variant_id"),
        "input_sha256": record.get("input_sha256"),
        "physics_sha256": record.get("physics_sha256"),
        "adapter_sha256": record.get("adapter_sha256"),
        "openmc_sha256": record.get("openmc_sha256"),
        "cross_sections_sha256": record.get("cross_sections_sha256"),
        "execution_status": record.pointer("/execution/execution_status"),
        "preflight_migration_receipt": migration
    }))
}

fn sensitivity(
    scenario_path: PathBuf,
    run_path: PathBuf,
    assumptions_path: PathBuf,
    grid_path: PathBuf,
    output: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    ensure_new_outputs(&output, None)?;
    let scenario =
        faris_model::LoadedScenario::from_bytes(&crate::transport::read_bounded(&scenario_path)?)?;
    let rates = rates_from_run(&scenario, &run_path)?;
    let assumptions: OperatingHistoryAssumptions =
        serde_json::from_slice(&crate::transport::read_bounded(&assumptions_path)?)?;
    let grid: HistorySensitivityGrid =
        serde_json::from_slice(&crate::transport::read_bounded(&grid_path)?)?;
    let interrupts = crate::control::interrupt_cancellation()?;
    #[cfg(unix)]
    let cancellation = &interrupts.cancellation;
    #[cfg(not(unix))]
    let cancellation = &interrupts;
    let result = run_history_sensitivity_cancellable(&assumptions, &rates, &grid, cancellation)
        .map_err(std::io::Error::other)?;
    write_new_json(&output, &result)?;
    println!(
        "{} full-history sensitivity points recorded at {}",
        result.points.len(),
        output.display()
    );
    announce_research_screening();
    Ok(())
}

fn ensure_new_outputs(
    primary: &std::path::Path,
    secondary: Option<&std::path::Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    if primary.exists() {
        return Err(format!("refusing to overwrite {}", primary.display()).into());
    }
    if let Some(other) = secondary {
        if other.exists() {
            return Err(format!("refusing to overwrite {}", other.display()).into());
        }
        if std::path::absolute(primary)? == std::path::absolute(other)? {
            return Err("history output and rates output must be distinct paths".into());
        }
    }
    Ok(())
}
