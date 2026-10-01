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
    } else {
        println!("Assumptions and transport driving rates are valid; no history was computed.");
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
    Ok(())
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
    ensure_new_outputs(&output, None)?;
    let scenario =
        faris_model::LoadedScenario::from_bytes(&crate::transport::read_bounded(&scenario_path)?)?;
    let assumptions: OperatingHistoryAssumptions =
        serde_json::from_slice(&crate::transport::read_bounded(&assumptions_path)?)?;
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
    println!("Paired history comparison recorded at {}", output.display());
    Ok(())
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
