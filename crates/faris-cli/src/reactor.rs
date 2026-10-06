use crate::{control, transport::read_bounded};
use clap::Subcommand;
use faris_engine::{
    build_manifest,
    jobs::ExecutionStatus,
    reactor::{
        FieldMesh, MeshPreset, ReactorJob, SamplingPlan, load_physics_case, load_reactor_run,
        run_reactor,
    },
};
use faris_model::{LoadedScenario, transport::ResponseDomain};
use std::{path::PathBuf, time::Duration};

#[derive(Subcommand)]
pub enum ReactorCommand {
    /// Run one bounded fixed-source Monte Carlo calculation and retain its artifacts.
    Run {
        #[arg(long)]
        scenario: PathBuf,
        #[arg(long)]
        physics: PathBuf,
        #[arg(long)]
        audit: PathBuf,
        #[arg(long)]
        cross_sections: PathBuf,
        #[arg(long)]
        python: PathBuf,
        #[arg(long)]
        openmc: PathBuf,
        /// New exclusive evidence directory; existing paths are refused.
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(30..=1000))]
        batches: u32,
        #[arg(long, default_value_t = 10_000, value_parser = clap::value_parser!(u32).range(1..=1_000_000))]
        particles: u32,
        #[arg(long, default_value_t = 123_456_789, value_parser = clap::value_parser!(u64).range(1..=i64::MAX as u64))]
        seed: u64,
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=32))]
        threads: u32,
        #[arg(long, default_value_t = 3600, value_parser = clap::value_parser!(u64).range(1..=faris_engine::reactor::MAX_RUN_TIMEOUT_SECONDS))]
        timeout_seconds: u64,
        /// Spatial flux tally resolution; local variants use the same outboard bounds.
        #[arg(long, default_value = "coarse", value_parser = ["coarse", "outboard-local-coarse", "outboard-local", "outboard-port-window"])]
        mesh_preset: String,
        /// Also tally each component's neutron spectrum on the 709-group `fispact-709`
        /// boundaries of the TENDL-2025 activation library (for activation inputs).
        #[arg(long, value_parser = ["fispact-709"])]
        activation_spectra: Option<String>,
    },
    /// Revalidate a saved run's exact input, artifact, volumes, and normalization.
    Inspect {
        #[arg(long)]
        scenario: PathBuf,
        /// The run.json record inside a reactor evidence directory.
        #[arg(long)]
        run: PathBuf,
    },
}

pub fn run(command: ReactorCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        ReactorCommand::Run {
            scenario,
            physics,
            audit,
            cross_sections,
            python,
            openmc,
            output,
            batches,
            particles,
            seed,
            threads,
            timeout_seconds,
            mesh_preset,
            activation_spectra,
        } => run_case(
            scenario,
            physics,
            audit,
            cross_sections,
            python,
            openmc,
            output,
            batches,
            particles,
            seed,
            threads,
            timeout_seconds,
            mesh_preset,
            activation_spectra,
        ),
        ReactorCommand::Inspect { scenario, run } => inspect(scenario, run),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_case(
    scenario: PathBuf,
    physics: PathBuf,
    audit: PathBuf,
    cross_sections: PathBuf,
    python: PathBuf,
    openmc: PathBuf,
    output: PathBuf,
    batches: u32,
    particles: u32,
    seed: u64,
    threads: u32,
    timeout_seconds: u64,
    mesh_preset: String,
    activation_spectra: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let scenario = LoadedScenario::from_bytes(&read_bounded(&scenario)?)?;
    let physics = load_physics_case(&physics, &scenario)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let sampling = SamplingPlan {
        batches,
        particles_per_batch: particles,
        seed,
        threads,
    };
    let preset = MeshPreset::from_cli(&mesh_preset)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let manifest = build_manifest(&scenario)?;
    let mesh = FieldMesh::for_preset(&manifest, preset)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let interrupts = control::interrupt_cancellation()?;
    #[cfg(unix)]
    let cancellation = &interrupts.cancellation;
    #[cfg(not(unix))]
    let cancellation = &interrupts;
    let output_absolute = std::path::absolute(&output)?;
    const ADAPTER: &[u8] = include_bytes!("../../../integrations/openmc/reactor_transport.py");
    eprintln!(
        "Running fixed-source OpenMC job ({} batches × {} particles; {} thread(s), {}s limit). Ctrl-C cancels the process group.",
        batches, particles, threads, timeout_seconds
    );
    let job = ReactorJob {
        scenario: &scenario,
        physics: &physics,
        audit: &audit,
        cross_sections: &cross_sections,
        python: &python,
        openmc: &openmc,
        output: &output_absolute,
        sampling,
        mesh: Some(mesh),
        timeout: Duration::from_secs(timeout_seconds),
        activation_spectra,
        adapter: ADAPTER,
    };
    let run = run_reactor(&job, cancellation)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let execution = run.execution.as_ref().map(|result| result.execution_status);
    let response_count = run
        .normalized
        .as_ref()
        .map_or(0, |normalized| normalized.results.len());
    println!(
        "Execution: {:?}; normalized responses: {}; scientific qualification: NOT_EVALUATED; evidence {}",
        execution,
        response_count,
        output_absolute.display()
    );
    if execution != Some(ExecutionStatus::Succeeded) {
        let reason = run
            .import_error
            .as_deref()
            .or_else(|| run.execution.as_ref().map(|result| result.stderr.as_str()))
            .filter(|message| !message.is_empty())
            .unwrap_or("external execution did not succeed");
        return Err(format!("reactor execution failed: {reason}; see run.json").into());
    }
    if run.normalized.is_none() {
        let reason = run
            .import_error
            .as_deref()
            .unwrap_or("normalized result was not produced");
        return Err(format!("transport artifact was not accepted: {reason}; see run.json").into());
    }
    Ok(())
}

fn inspect(scenario: PathBuf, path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let scenario = LoadedScenario::from_bytes(&read_bounded(&scenario)?)?;
    let run = load_reactor_run(&path, &scenario)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let normalized = run
        .normalized
        .as_ref()
        .ok_or("run has no validated normalized responses")?;
    let summaries: Vec<_> = normalized
        .results
        .iter()
        .filter(|result| !matches!(result.domain, ResponseDomain::Mesh { .. }))
        .map(|result| {
            serde_json::json!({
                "response_id": result.response_id,
                "domain": result.domain,
                "score": result.score,
                "mean": result.mean,
                "standard_error": result.standard_error,
                "unit": result.unit,
                "integrated_mean": result.integrated_mean,
                "integrated_standard_error": result.integrated_standard_error,
                "integrated_unit": result.integrated_unit,
                "volume_m3": result.volume_m3,
            })
        })
        .collect();
    let mesh_nonzero_bins = normalized
        .results
        .iter()
        .filter(|result| matches!(result.domain, ResponseDomain::Mesh { .. }) && result.mean > 0.0)
        .count();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "inspection_status": "REVALIDATED_NORMALIZATION",
            "scenario_id": scenario.scenario.id,
            "scenario_sha256": run.scenario_sha256,
            "variant_id": run.variant_id,
            "scientific_scope": run.scientific_scope,
            "sampling": run.sampling,
            "execution_status": run.execution.as_ref().map(|e| e.execution_status),
            "response_count": normalized.results.len(),
            "mesh_bin_count": normalized.results.len() - summaries.len(),
            "mesh_nonzero_flux_bin_count": mesh_nonzero_bins,
            "non_mesh_responses": summaries,
            "scientific_qualification": run.scientific_qualification,
            "notice": run.notice,
            "record": path,
        }))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn reactor_help_exposes_required_identity_inputs_and_bounded_sampling() {
        let error = match crate::Arguments::try_parse_from(["faris", "reactor", "run", "--help"]) {
            Ok(_) => panic!("--help should stop argument parsing"),
            Err(error) => error,
        };
        let help = error.to_string();
        for option in [
            "--scenario",
            "--physics",
            "--audit",
            "--cross-sections",
            "--python",
            "--openmc",
            "--output",
            "--batches",
            "--particles",
            "--seed",
            "--threads",
            "--timeout-seconds",
            "--mesh-preset",
            "--activation-spectra",
        ] {
            assert!(help.contains(option), "missing {option} in help: {help}");
        }
    }

    #[test]
    fn inspect_help_requires_scenario_bound_run_record() {
        let error =
            match crate::Arguments::try_parse_from(["faris", "reactor", "inspect", "--help"]) {
                Ok(_) => panic!("--help should stop argument parsing"),
                Err(error) => error,
            };
        let help = error.to_string();
        assert!(help.contains("--scenario"));
        assert!(help.contains("--run"));
    }

    #[test]
    fn default_sampling_is_engine_default() {
        let sampling = SamplingPlan::default();
        assert_eq!(sampling.batches, 100);
        assert_eq!(sampling.particles_per_batch, 10_000);
        assert_eq!(sampling.seed, 123_456_789);
        assert_eq!(sampling.threads, 1);
    }
}
