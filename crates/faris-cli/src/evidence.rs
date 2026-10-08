use crate::{control, transport::read_bounded};
use clap::{Subcommand, ValueEnum};
use faris_engine::{
    build_manifest,
    case_archive::{inspect_saved_case, inspect_saved_case_in_store},
    core_evidence::{self, RecordedTransportBundle},
    evidence_store::{EvidenceStore, StoreVerifyStatus, verify_store},
    study::{StudySelection, generate_study},
};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Stage {
    Transport,
    Normalize,
    History,
    Energy,
}

#[derive(Subcommand)]
pub enum EvidenceCommand {
    /// Generate a portable hash-bound Core case from actual recorded transport.
    Prepare {
        #[arg(long)]
        run: PathBuf,
        /// Generated study.json; if omitted, request breeding and shielding.
        #[arg(long)]
        study: Option<PathBuf>,
        #[arg(long)]
        assumptions: Option<PathBuf>,
        #[arg(long)]
        core: PathBuf,
        /// FARIS CLI executable to bind (defaults to this executable).
        #[arg(long)]
        faris: Option<PathBuf>,
        #[arg(long)]
        output: PathBuf,
    },
    /// Execute the real Core controlled runner against a generated case.
    Run {
        #[arg(long)]
        case: PathBuf,
        #[arg(long)]
        core: PathBuf,
        #[arg(long)]
        faris: Option<PathBuf>,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Reopen and revalidate a saved case and its completed Core workspace,
    /// from directories (--case, --report, --workspace) or in place from an
    /// evidence store (--store, --case-tree, --workspace-tree).
    Inspect {
        #[arg(long, requires_all = ["report", "workspace"], conflicts_with = "store")]
        case: Option<PathBuf>,
        #[arg(long)]
        report: Option<PathBuf>,
        #[arg(long)]
        workspace: Option<PathBuf>,
        /// Evidence store folder holding the case and workspace trees.
        #[arg(long, requires_all = ["case_tree", "workspace_tree"])]
        store: Option<PathBuf>,
        /// Tree of the store holding the prepared case and its execution-report.json.
        #[arg(long, requires = "store")]
        case_tree: Option<String>,
        /// Tree of the store holding the Core execution workspace.
        #[arg(long, requires = "store")]
        workspace_tree: Option<String>,
    },
    /// Check an evidence store: its index, every blob's length and SHA-256,
    /// and that the folder holds nothing else. Prints a JSON report; exits
    /// non-zero when the store fails.
    VerifyStore {
        #[arg(long)]
        store: PathBuf,
    },
    /// A deterministic FARIS stage invoked by a hash-bound Core descriptor.
    Stage {
        #[arg(value_enum)]
        stage: Stage,
        #[arg(long)]
        scenario: Option<PathBuf>,
        #[arg(long)]
        physics: Option<PathBuf>,
        #[arg(long)]
        data: Option<PathBuf>,
        #[arg(long)]
        recorded: Option<PathBuf>,
        #[arg(long)]
        upstream: Option<PathBuf>,
        #[arg(long)]
        assumptions: Option<PathBuf>,
        #[arg(long)]
        output: PathBuf,
    },
}

fn required(path: Option<PathBuf>, label: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let path = path.ok_or_else(|| format!("stage requires {label}"))?;
    read_bounded(&path)
}

fn required_stage(
    path: Option<PathBuf>,
    label: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let path = path.ok_or_else(|| format!("stage requires {label}"))?;
    core_evidence::read_stage(&path).map_err(|e| e as Box<dyn std::error::Error>)
}

pub fn run(command: EvidenceCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        EvidenceCommand::Prepare {
            run,
            study,
            assumptions,
            core,
            faris,
            output,
        } => {
            let bundle =
                core_evidence::pack_transport(&run).map_err(|e| e as Box<dyn std::error::Error>)?;
            let (scenario, record) = bundle
                .verify()
                .map_err(|e| e as Box<dyn std::error::Error>)?;
            let selection = if let Some(path) = study {
                let value: serde_json::Value = serde_json::from_slice(&read_bounded(&path)?)?;
                let selection: StudySelection = serde_json::from_value(value["selection"].clone())?;
                let generated =
                    generate_study(&build_manifest(&scenario)?, &record.variant_id, &selection)
                        .map_err(std::io::Error::other)?;
                if value != serde_json::to_value(&generated)? {
                    return Err("study differs from current scenario/template/selections".into());
                }
                selection
            } else {
                StudySelection::default()
            };
            let generated =
                generate_study(&build_manifest(&scenario)?, &record.variant_id, &selection)
                    .map_err(std::io::Error::other)?;
            let faris = faris.unwrap_or(std::env::current_exe()?);
            #[cfg(unix)]
            let interrupts = control::interrupt_cancellation()?;
            #[cfg(unix)]
            let cancellation = &interrupts.cancellation;
            #[cfg(not(unix))]
            let cancellation = &faris_engine::jobs::Cancellation::default();
            let assumptions: Option<faris_model::history::OperatingHistoryAssumptions> =
                assumptions
                    .map(|p| -> Result<_, Box<dyn std::error::Error>> {
                        Ok(serde_json::from_slice(&read_bounded(&p)?)?)
                    })
                    .transpose()?;
            core_evidence::prepare_case_with_assumptions(
                &generated,
                &run,
                assumptions.as_ref(),
                &core,
                &faris,
                &output,
                cancellation,
            )
            .map_err(|e| e as Box<dyn std::error::Error>)?;
            println!("Prepared bound Core case at {}", output.display());
        }
        EvidenceCommand::Run {
            case,
            core,
            faris,
            workspace,
            output,
        } => {
            if output.exists() {
                return Err("evidence output must be fresh".into());
            }
            let faris = faris.unwrap_or(std::env::current_exe()?);
            #[cfg(unix)]
            let interrupts = control::interrupt_cancellation()?;
            #[cfg(unix)]
            let cancellation = &interrupts.cancellation;
            #[cfg(not(unix))]
            let cancellation = &faris_engine::jobs::Cancellation::default();
            let result = core_evidence::run_case(&case, &core, &faris, &workspace, cancellation)
                .map_err(|e| e as Box<dyn std::error::Error>)?;
            core_evidence::write_new(&output, &serde_json::to_vec_pretty(&result)?)
                .map_err(|e| e as Box<dyn std::error::Error>)?;
            println!("Core evidence report saved to {}", output.display());
            if !result.completed() {
                return Err("Core execution did not complete; inspect the saved report".into());
            }
        }
        EvidenceCommand::Inspect {
            case,
            report,
            workspace,
            store,
            case_tree,
            workspace_tree,
        } => {
            let inspection = match (store, case, report, workspace, case_tree, workspace_tree) {
                (Some(store), None, None, None, Some(case_tree), Some(workspace_tree)) => {
                    let store = EvidenceStore::open(&store)?;
                    inspect_saved_case_in_store(&store, &case_tree, &workspace_tree)
                }
                (None, Some(case), Some(report), Some(workspace), None, None) => {
                    inspect_saved_case(&case, &report, &workspace)
                }
                _ => {
                    return Err(
                        "give either --case, --report and --workspace (directories), or \
                                --store, --case-tree and --workspace-tree (an evidence store)"
                            .into(),
                    );
                }
            }
            .map_err(|e| e as Box<dyn std::error::Error>)?;
            println!("{}", serde_json::to_string_pretty(&inspection)?);
        }
        EvidenceCommand::VerifyStore { store } => {
            let report = verify_store(&store);
            println!("{}", serde_json::to_string_pretty(&report)?);
            if report.status != StoreVerifyStatus::Verified {
                return Err(format!(
                    "evidence store failed verification ({} findings); see the report above",
                    report.finding_count
                )
                .into());
            }
        }
        EvidenceCommand::Stage {
            stage,
            scenario,
            physics,
            data,
            recorded,
            upstream,
            assumptions,
            output,
        } => {
            #[cfg(unix)]
            let interrupts = control::interrupt_cancellation()?;
            #[cfg(unix)]
            let cancellation = &interrupts.cancellation;
            #[cfg(not(unix))]
            let cancellation = &faris_engine::jobs::Cancellation::default();
            let result = match stage {
                Stage::Transport => {
                    let bundle: RecordedTransportBundle =
                        serde_json::from_slice(&required_stage(recorded, "--recorded")?)?;
                    core_evidence::transport_stage(
                        &required(scenario, "--scenario")?,
                        &required(physics, "--physics")?,
                        &required(data, "--data")?,
                        &bundle,
                    )
                }
                Stage::Normalize => {
                    let upstream =
                        serde_json::from_slice(&required_stage(upstream, "--upstream")?)?;
                    core_evidence::normalization_stage(
                        &upstream,
                        &required(scenario, "--scenario")?,
                    )
                }
                Stage::History => {
                    let upstream =
                        serde_json::from_slice(&required_stage(upstream, "--upstream")?)?;
                    let assumptions =
                        serde_json::from_slice(&required(assumptions, "--assumptions")?)?;
                    core_evidence::history_stage(
                        &upstream,
                        &required(scenario, "--scenario")?,
                        &assumptions,
                        cancellation,
                    )
                }
                Stage::Energy => {
                    let upstream =
                        serde_json::from_slice(&required_stage(upstream, "--upstream")?)?;
                    core_evidence::energy_stage(
                        &upstream,
                        &required(scenario, "--scenario")?,
                        cancellation,
                    )
                }
            }
            .map_err(|e| e as Box<dyn std::error::Error>)?;
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let bytes = serde_json::to_vec_pretty(&result)?;
            if bytes.len() > core_evidence::MAX_STAGE_BYTES {
                return Err("stage output exceeds 64 MiB".into());
            }
            core_evidence::write_new(&output, &bytes)
                .map_err(|e| e as Box<dyn std::error::Error>)?;
        }
    }
    Ok(())
}
