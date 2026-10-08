//! Computed maintenance durations: run the coupling of operating history and ACTINV decay
//! heat for any number of designs, and report the result.
use crate::transport::{read_bounded, write_new_json};
use clap::{Subcommand, ValueEnum};
// What the tests below reach through `super::*`.
use faris_engine::{
    history::JULIAN_YEAR_SECONDS,
    maintenance::{
        ComputedResult, Contrast, DesignResult, EventRecord, HistorySummary,
        MAINTENANCE_RESULT_VERSION, MaintenanceResult, Status,
        files::{RunConfig, run_from_files},
    },
};
#[cfg(test)]
use faris_engine::{
    history::{HistoryResult, run_operating_history_cancellable},
    jobs::Cancellation,
    maintenance::{
        DecaySourceRecord, DesignInput,
        files::{DESIGNS_VERSION, parse_designs},
        run_maintenance,
    },
};
#[cfg(test)]
use faris_model::{
    history::OperatingHistoryAssumptions,
    maintenance::{MaintenanceAssumptions, Threshold},
};
#[cfg(test)]
use std::collections::BTreeMap;
use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::Arc,
};

const DAY_S: f64 = 86_400.0;

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum MaintenanceCommand {
    /// Compute replacement outages from the decay heat of the components around the replaced
    /// part, for every design, and compare them with the fixed durations. Runs ACTINV.
    Run {
        /// faris-maintenance-designs/v0.1: per design the scenario, physics file, history run,
        /// 709-group spectrum run and operating-history assumptions (paths relative to the file).
        #[arg(long)]
        designs: PathBuf,
        /// faris-maintenance-assumptions/v0.1: classes, thresholds, cooling grid, iteration limits.
        #[arg(long)]
        assumptions: PathBuf,
        /// The `actinv` executable.
        #[arg(long)]
        actinv: PathBuf,
        /// ACTINV data root (the folder above the catalogue version).
        #[arg(long)]
        data_dir: PathBuf,
        /// New result file (faris-maintenance-result/v0.1); existing paths are refused.
        #[arg(long)]
        output: PathBuf,
        /// Content-addressed ACTINV points cache, shared with the Python driver
        /// [default: DECAY-CACHE inside the work folder].
        #[arg(long)]
        cache: Option<PathBuf>,
        /// Per-design, per-iteration spec folders and scratch space [default: OUTPUT.work].
        #[arg(long)]
        work_dir: Option<PathBuf>,
        /// Concurrent ACTINV runs.
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=64))]
        workers: u32,
        /// Impurity specification (material -> impurities): use the specification-maximum
        /// variant instead of the bare lower bound.
        #[arg(long)]
        impurities: Option<PathBuf>,
        /// Interpreter that runs the activation-input builder.
        #[arg(long, default_value = "python3")]
        python: PathBuf,
        /// The activation-input builder script.
        #[arg(
            long,
            default_value = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/build_activation_inputs.py")
        )]
        builder: PathBuf,
    },
    /// Print a maintenance result: downtime, availability and electricity under both models,
    /// every replacement with its governing component, and the differences between designs.
    Report {
        result: PathBuf,
        #[arg(long, value_enum, default_value_t = ReportFormat::Markdown)]
        format: ReportFormat,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum ReportFormat {
    Markdown,
    Json,
}

pub fn run(command: MaintenanceCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        MaintenanceCommand::Run {
            designs,
            assumptions,
            actinv,
            data_dir,
            output,
            cache,
            work_dir,
            workers,
            impurities,
            python,
            builder,
        } => run_designs(RunRequest {
            designs,
            assumptions,
            actinv,
            data_dir,
            output,
            cache,
            work_dir,
            workers: workers as usize,
            impurities,
            python,
            builder,
        }),
        MaintenanceCommand::Report { result, format } => report(&result, format),
    }
}

// ---------------------------------------------------------------------- run --

struct RunRequest {
    designs: PathBuf,
    assumptions: PathBuf,
    actinv: PathBuf,
    data_dir: PathBuf,
    output: PathBuf,
    cache: Option<PathBuf>,
    work_dir: Option<PathBuf>,
    workers: usize,
    impurities: Option<PathBuf>,
    python: PathBuf,
    builder: PathBuf,
}

fn run_designs(request: RunRequest) -> Result<(), Box<dyn std::error::Error>> {
    if request.output.exists() {
        return Err(format!("refusing to overwrite {}", request.output.display()).into());
    }
    let work_dir = request
        .work_dir
        .unwrap_or_else(|| append_suffix(&request.output, ".work"));
    let config = RunConfig {
        designs: request.designs,
        assumptions: request.assumptions,
        actinv: request.actinv,
        data_dir: request.data_dir,
        cache: request.cache,
        work_dir,
        workers: request.workers,
        impurities: request.impurities,
        python: request.python,
        builder: request.builder,
    };
    let interrupts = crate::control::interrupt_cancellation()?;
    #[cfg(unix)]
    let cancellation = &interrupts.cancellation;
    #[cfg(not(unix))]
    let cancellation = &interrupts;
    let result = run_from_files(
        &config,
        cancellation,
        Arc::new(|progress| eprintln!("maintenance: {}", progress.line())),
    )?;
    write_new_json(&request.output, &result)?;
    for (name, design) in &result.designs {
        println!("{name}: {}", summary_line(design));
    }
    let (actinv_runs, cache_hits) = result
        .decay_source
        .as_ref()
        .map_or((0, 0), |s| (s.actinv_runs, s.cache_hits));
    println!(
        "Maintenance result recorded at {} ({actinv_runs} ACTINV runs, {cache_hits} cached curves)",
        request.output.display()
    );
    eprintln!("{}", faris_model::RESEARCH_SCREENING_STATEMENT);
    Ok(())
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn summary_line(design: &DesignResult) -> String {
    let fixed = design.fixed.total_replacement_downtime_s / DAY_S;
    match (&design.computed.summary, &design.computed.not_evaluated) {
        (Some(c), _) => format!(
            "replacement downtime {fixed:.1} d fixed, {:.1} d computed",
            c.total_replacement_downtime_s / DAY_S
        ),
        (None, Some(ne)) => format!(
            "replacement downtime {fixed:.1} d fixed; computed NOT_EVALUATED: {} Next step: {}",
            ne.reason, ne.next_step
        ),
        (None, None) => format!("replacement downtime {fixed:.1} d fixed"),
    }
}

// ------------------------------------------------------------------- report --

fn report(path: &Path, format: ReportFormat) -> Result<(), Box<dyn std::error::Error>> {
    let result: MaintenanceResult = serde_json::from_slice(&read_bounded(path)?)?;
    if result.schema_version != MAINTENANCE_RESULT_VERSION {
        return Err(format!("result schema_version must be {MAINTENANCE_RESULT_VERSION}").into());
    }
    match format {
        ReportFormat::Markdown => print!("{}", markdown(&result)),
        ReportFormat::Json => println!("{}", serde_json::to_string_pretty(&result)?),
    }
    Ok(())
}

fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

fn days(seconds: f64) -> String {
    format!("{:.1}", seconds / DAY_S)
}

fn years(seconds: f64) -> String {
    format!("{:.2}", seconds / JULIAN_YEAR_SECONDS)
}

fn availability(summary: &HistorySummary) -> String {
    format!("{:.2} %", summary.availability * 100.0)
}

fn electricity(summary: &HistorySummary) -> String {
    summary
        .lifetime_net_electricity_mwh
        .map_or_else(|| "not available".into(), |mwh| format!("{mwh:.0}"))
}

/// The iteration whose durations the converged summary used, or the last one.
fn final_events(computed: &ComputedResult) -> &[EventRecord] {
    let wanted = computed.converged_at_iteration;
    computed
        .iterations
        .iter()
        .find(|i| Some(i.iteration) == wanted)
        .or(computed.iterations.last())
        .map_or(&[], |i| i.events.as_slice())
}

fn markdown(result: &MaintenanceResult) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Replacement downtime: fixed and computed durations\n"
    );
    let _ = writeln!(
        out,
        "A computed outage is the cooldown (until the decay heat per volume of the governing \
         components falls to q*) plus the work time. Fixed durations are the authored ones. \
         This report ranks nothing: it shows downtime, availability and lifetime net \
         electricity under both models.\n"
    );
    if let Some(source) = &result.decay_source {
        let _ = writeln!(
            out,
            "Decay source: {} ({} ACTINV runs, {} cached curves).\n",
            source.kind, source.actinv_runs, source.cache_hits
        );
    }

    let _ = writeln!(out, "## Per design\n");
    let _ = writeln!(
        out,
        "| Design | Status | Downtime fixed (d) | Downtime computed (d) | Availability fixed | \
         Availability computed | Net electricity fixed (MWh) | Net electricity computed (MWh) |"
    );
    let _ = writeln!(out, "|---|---|---:|---:|---:|---:|---:|---:|");
    for (name, design) in &result.designs {
        let fixed = &design.fixed;
        match (&design.computed.status, &design.computed.summary) {
            (Status::Evaluated, Some(c)) => {
                let _ = writeln!(
                    out,
                    "| {} | EVALUATED | {} | {} | {} | {} | {} | {} |",
                    cell(name),
                    days(fixed.total_replacement_downtime_s),
                    days(c.total_replacement_downtime_s),
                    availability(fixed),
                    availability(c),
                    electricity(fixed),
                    electricity(c)
                );
            }
            _ => {
                let _ = writeln!(
                    out,
                    "| {} | NOT_EVALUATED | {} | NOT_EVALUATED | {} | NOT_EVALUATED | {} | NOT_EVALUATED |",
                    cell(name),
                    days(fixed.total_replacement_downtime_s),
                    availability(fixed),
                    electricity(fixed)
                );
            }
        }
    }
    out.push('\n');
    for (name, design) in &result.designs {
        if let Some(ne) = &design.computed.not_evaluated {
            let _ = writeln!(
                out,
                "- {name}: NOT_EVALUATED. {} Next step: {}",
                ne.reason, ne.next_step
            );
        }
    }

    if let Some(design) = result.designs.values().next() {
        let _ = writeln!(out, "\n## Cooldown thresholds\n");
        let _ = writeln!(out, "| Class | q* (W/m3) | Source | Note |");
        let _ = writeln!(out, "|---|---:|---|---|");
        for (class, th) in &design.thresholds {
            let (q, note) = match (&th.q_star_w_per_m3, &th.not_evaluated) {
                (Some(q), _) => (format!("{q:.4e}"), String::new()),
                (None, Some(ne)) => (
                    "NOT_EVALUATED".into(),
                    format!("{} Next step: {}", ne.reason, ne.next_step),
                ),
                (None, None) => ("NOT_EVALUATED".into(), String::new()),
            };
            let source = match &th.calibration_design {
                Some(d) => format!("{} on {d}", th.source),
                None => th.source.clone(),
            };
            let _ = writeln!(
                out,
                "| {} | {q} | {} | {} |",
                cell(class),
                cell(&source),
                cell(&note)
            );
        }
    }

    for (name, design) in &result.designs {
        if design.computed.status != Status::Evaluated {
            continue;
        }
        let _ = writeln!(out, "\n## Replacements: {name}\n");
        let _ = writeln!(
            out,
            "| Component | k | Start (y) | Fixed (d) | Computed (d) | Governing component | Share | In service (y) | Note |"
        );
        let _ = writeln!(out, "|---|---:|---:|---:|---:|---|---:|---:|---|");
        for e in final_events(&design.computed) {
            let governing = e.why.as_ref().and_then(|w| {
                w.governing
                    .iter()
                    .reduce(|best, g| if g.share > best.share { g } else { best })
            });
            let note = match (&e.not_evaluated, e.window_limited) {
                (Some(ne), _) => {
                    format!("NOT_EVALUATED: {} Next step: {}", ne.reason, ne.next_step)
                }
                (None, true) => "cooldown limited by the outage window".into(),
                (None, false) => String::new(),
            };
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                cell(&e.component),
                e.k,
                years(e.start_s),
                days(e.fixed_duration_s),
                e.duration_computed_s.map_or_else(|| "n/a".into(), days),
                governing.map_or_else(|| "n/a".into(), |g| cell(&g.component)),
                governing.map_or_else(|| "n/a".into(), |g| format!("{:.0} %", g.share * 100.0)),
                governing.map_or_else(|| "n/a".into(), |g| years(g.in_service_s)),
                cell(&note)
            );
        }
    }

    if !result.contrasts.is_empty() {
        let _ = writeln!(out, "\n## Between designs\n");
        let _ = writeln!(
            out,
            "Downtime of A minus downtime of B, under each model.\n"
        );
        let _ = writeln!(
            out,
            "| A | B | Fixed difference (d) | Computed difference (d) | Computed / fixed | Note |"
        );
        let _ = writeln!(out, "|---|---|---:|---:|---:|---|");
        for c in &result.contrasts {
            let _ = writeln!(out, "{}", contrast_row(c));
        }
    }
    out
}

fn contrast_row(c: &Contrast) -> String {
    let note = c
        .not_evaluated
        .as_ref()
        .map(|ne| format!("{} Next step: {}", ne.reason, ne.next_step))
        .unwrap_or_default();
    format!(
        "| {} | {} | {} | {} | {} | {} |",
        cell(&c.a),
        cell(&c.b),
        days(c.fixed_difference_s),
        c.computed_difference_s.map_or_else(|| "n/a".into(), days),
        c.ratio_computed_over_fixed
            .map_or_else(|| "n/a".into(), |r| format!("{r:.2}")),
        cell(&note)
    )
}

#[cfg(test)]
mod tests;
