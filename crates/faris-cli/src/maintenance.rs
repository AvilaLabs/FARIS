// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! Computed maintenance durations: run the coupling of operating history and ACTINV decay
//! heat for any number of designs, and report the result.
use crate::transport::{read_bounded, write_new_json};
use clap::{Subcommand, ValueEnum};
// What the tests below reach through `super::*`.
use faris_engine::maintenance::{
    ComputedResult, Contrast, DesignResult, DoseMaintenanceResult, EventRecord, HistorySummary,
    MAINTENANCE_DOSE_RESULT_VERSION, MAINTENANCE_RESULT_VERSION, MAX_DOSE_RESULT_BYTES,
    MaintenanceResult, Status,
    files::{
        BUILDER_NOT_FOUND, DoseRunConfig, RunConfig, default_builder, default_event_dose_worker,
        run_dose_from_files, run_from_files,
    },
    parse_dose_result, prepare_dose_cad,
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

use faris_model::{
    display::{DisplayPrefs, NumberFormat},
    units::{self, UnitDef},
};

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
        /// The activation-input builder script [default: tools/build_activation_inputs.py
        /// beside the folder that holds this program (a downloaded package); otherwise the
        /// script in the source repository this program was built from].
        #[arg(long)]
        builder: Option<PathBuf>,
    },
    /// Compute replacement outages from event-specific spatial photon dose-rate curves.
    /// D1S/R2S comparison is available when both methods can share the event's history basis;
    /// otherwise the result keeps D1S explicitly NOT_EVALUATED.
    DoseRun {
        /// faris-maintenance-designs/v0.1 inputs or faris-maintenance-cad-dose-designs/v0.2
        /// with the checked CAD design, CAD history-rate record, optional history-assumptions
        /// override, and dose_physics manifest. The scenario-bound CAD v0.1 format is also supported.
        #[arg(long)]
        designs: PathBuf,
        /// faris-maintenance-assumptions/v0.2 with dose-rate thresholds and location hashes.
        #[arg(long)]
        assumptions: PathBuf,
        /// Optional hash-verified faris-maintenance-spatial-dose-curves/v0.2 cache.
        #[arg(long)]
        spatial_dose_curves: Option<PathBuf>,
        /// Python interpreter with the validated OpenMC runtime.
        #[arg(long, default_value = "python3")]
        python: PathBuf,
        /// Event-specific OpenMC R2S worker script [default: installed worker].
        #[arg(long)]
        worker: Option<PathBuf>,
        /// Work directory for bounded OpenMC event jobs [default: OUTPUT.dose-work].
        #[arg(long)]
        work_dir: Option<PathBuf>,
        /// New result file (faris-maintenance-result/v0.2); existing paths are refused.
        #[arg(long)]
        output: PathBuf,
    },
    /// Clone checked CAD materials per solid and build the event-dose physics adapter.
    DosePrepare {
        /// faris-maintenance-dose-cad-config/v0.1 preparation request.
        #[arg(long)]
        config: PathBuf,
        /// Python interpreter with the validated OpenMC runtime and plasma-source package.
        #[arg(long, default_value = "python3")]
        python: PathBuf,
        /// Event-dose worker [default: installed worker].
        #[arg(long)]
        worker: Option<PathBuf>,
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
        MaintenanceCommand::DoseRun {
            designs,
            assumptions,
            spatial_dose_curves,
            python,
            worker,
            work_dir,
            output,
        } => run_dose_designs(
            designs,
            assumptions,
            spatial_dose_curves,
            python,
            worker,
            work_dir,
            output,
        ),
        MaintenanceCommand::DosePrepare {
            config,
            python,
            worker,
        } => run_dose_prepare(config, python, worker),
        MaintenanceCommand::Report { result, format } => report(&result, format),
    }
}

fn run_dose_prepare(
    config: PathBuf,
    python: PathBuf,
    worker: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let worker = worker
        .or_else(default_event_dose_worker)
        .ok_or("OpenMC event-dose worker script not found; pass --worker")?;
    let interrupts = crate::control::interrupt_cancellation()?;
    #[cfg(unix)]
    let cancellation = &interrupts.cancellation;
    #[cfg(not(unix))]
    let cancellation = &interrupts;
    let manifest = prepare_dose_cad(&config, &python, &worker, cancellation)?;
    println!(
        "Prepared event-dose physics manifest: {}",
        manifest.display()
    );
    Ok(())
}

fn run_dose_designs(
    designs: PathBuf,
    assumptions: PathBuf,
    spatial_dose_curves: Option<PathBuf>,
    python: PathBuf,
    worker: Option<PathBuf>,
    work_dir: Option<PathBuf>,
    output: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    if output.exists() {
        return Err(format!("refusing to overwrite {}", output.display()).into());
    }
    let worker_script = worker
        .or_else(default_event_dose_worker)
        .ok_or("OpenMC event-dose worker script not found; pass --worker")?;
    let config = DoseRunConfig {
        designs,
        assumptions,
        spatial_dose_curves,
        python,
        worker_script,
        work_dir: work_dir.unwrap_or_else(|| output.with_extension("dose-work")),
    };
    let interrupts = crate::control::interrupt_cancellation()?;
    #[cfg(unix)]
    let cancellation = &interrupts.cancellation;
    #[cfg(not(unix))]
    let cancellation = &interrupts;
    let result: DoseMaintenanceResult = run_dose_from_files(
        &config,
        cancellation,
        Arc::new(|progress| eprintln!("maintenance: {}", progress.line())),
    )?;
    let bytes = serde_json::to_vec_pretty(&result)?;
    if bytes.len() >= MAX_DOSE_RESULT_BYTES {
        return Err("dose maintenance result exceeds the 16 MiB output limit".into());
    }
    write_new_json(&output, &result)?;
    for (name, design) in &result.designs {
        let fixed = design.fixed.total_replacement_downtime_s / faris_model::units::DAY_S;
        let computed = design
            .computed
            .summary
            .as_ref()
            .map(|s| s.total_replacement_downtime_s / faris_model::units::DAY_S);
        println!(
            "{name}: {:.3} days fixed, {} computed; validation {:?}",
            fixed,
            computed.map_or_else(|| "NOT_EVALUATED".into(), |v| format!("{v:.3} days")),
            design.computed.validation
        );
    }
    println!(
        "Dose-governed result ({MAINTENANCE_DOSE_RESULT_VERSION}) recorded at {}",
        output.display()
    );
    eprintln!("{}", faris_model::RESEARCH_SCREENING_STATEMENT);
    Ok(())
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
    builder: Option<PathBuf>,
}

fn run_designs(request: RunRequest) -> Result<(), Box<dyn std::error::Error>> {
    if request.output.exists() {
        return Err(format!("refusing to overwrite {}", request.output.display()).into());
    }
    let builder = request
        .builder
        .or_else(default_builder)
        .ok_or(BUILDER_NOT_FOUND)?;
    let work_dir = request
        .work_dir
        .unwrap_or_else(|| append_suffix(&request.output, ".work"));
    let config = RunConfig {
        designs: request.designs,
        assumptions: request.assumptions,
        actinv: request.actinv,
        data_dir: request.data_dir,
        prepared_cache_dir: None,
        cache: request.cache,
        work_dir,
        workers: request.workers,
        impurities: request.impurities,
        python: request.python,
        builder,
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
    let show = Show::new(&crate::config::display());
    for (name, design) in &result.designs {
        println!("{name}: {}", summary_line(design, &show));
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

fn summary_line(design: &DesignResult, show: &Show) -> String {
    let unit = show.duration.symbol;
    let fixed = show.days(design.fixed.total_replacement_downtime_s);
    match (&design.computed.summary, &design.computed.not_evaluated) {
        (Some(c), _) => format!(
            "replacement downtime {fixed} {unit} fixed, {} {unit} computed",
            show.days(c.total_replacement_downtime_s)
        ),
        (None, Some(ne)) => format!(
            "replacement downtime {fixed} {unit} fixed; computed NOT_EVALUATED: {} Next step: {}",
            ne.reason, ne.next_step
        ),
        (None, None) => format!("replacement downtime {fixed} {unit} fixed"),
    }
}

/// The units and number format this report prints in: the user's choice, else the units the
/// report has always used (days, years, MWh, W/m3), so a default run prints what it always did.
struct Show {
    duration: &'static UnitDef,
    service: &'static UnitDef,
    energy: &'static UnitDef,
    density: &'static UnitDef,
    number: NumberFormat,
}

impl Show {
    fn new(prefs: &DisplayPrefs) -> Self {
        Self {
            duration: prefs.unit_or("time", "d"),
            service: prefs.unit_or("service_time", "yr"),
            energy: prefs.unit_or("energy_plant", "MWh"),
            density: prefs.unit_or("power_density", "W/m3"),
            number: prefs.number,
        }
    }

    /// Four significant digits for a unit the report does not habitually use, the report's own
    /// fixed decimals for the habitual one.
    fn digits(&self, value: f64, habit_decimals: usize, habitual: bool) -> String {
        if habitual {
            self.number.fixed(value, habit_decimals)
        } else {
            self.number.significant(value, 4)
        }
    }

    fn days(&self, seconds: f64) -> String {
        self.digits(
            self.duration.from_si(seconds),
            1,
            self.duration.symbol == "d",
        )
    }

    fn years(&self, seconds: f64) -> String {
        self.digits(
            self.service.from_si(seconds),
            2,
            self.service.symbol == "yr",
        )
    }
}

// ------------------------------------------------------------------- report --

fn report(path: &Path, format: ReportFormat) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = read_bounded(path)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    if value
        .get("schema_version")
        .and_then(serde_json::Value::as_str)
        == Some(MAINTENANCE_DOSE_RESULT_VERSION)
    {
        let result: DoseMaintenanceResult = parse_dose_result(&bytes)?;
        match format {
            ReportFormat::Json => println!("{}", serde_json::to_string_pretty(&value)?),
            ReportFormat::Markdown => print!("{}", dose_markdown(&result)),
        }
        return Ok(());
    }
    let result: MaintenanceResult = serde_json::from_value(value)?;
    if result.schema_version != MAINTENANCE_RESULT_VERSION {
        return Err(format!("result schema_version must be {MAINTENANCE_RESULT_VERSION}").into());
    }
    match format {
        ReportFormat::Markdown => print!(
            "{}",
            markdown_with(&result, &Show::new(&crate::config::display()))
        ),
        ReportFormat::Json => println!("{}", serde_json::to_string_pretty(&result)?),
    }
    Ok(())
}

fn dose_markdown(result: &DoseMaintenanceResult) -> String {
    let mut out = String::from(
        "# Dose-governed replacement durations\n\nCriterion: ICRP-116 AP photon effective dose rate at the authored task location (Sv/h). External D1S/R2S estimates; not a product-generated solver result.\n\n",
    );
    for (name, design) in &result.designs {
        let _ = writeln!(
            out,
            "## {}\n\nFixed downtime: {:.3} days.\n",
            cell(name),
            design.fixed.total_replacement_downtime_s / faris_model::units::DAY_S
        );
        if let Some(summary) = &design.computed.summary {
            let _ = writeln!(
                out,
                "Computed downtime: {:.3} days.\n",
                summary.total_replacement_downtime_s / faris_model::units::DAY_S
            );
        }
        let _ = writeln!(
            out,
            "Computed status: **{:?}**. Curve comparison state: **{:?}**.\n",
            design.computed.status, design.computed.validation
        );
        if let Some(ne) = &design.computed.not_evaluated {
            let _ = writeln!(
                out,
                "NOT_EVALUATED: {} Next step: {}\n",
                cell(&ne.reason),
                cell(&ne.next_step)
            );
        }
        out.push_str("| Event | Task location | Limit source | Limit (Sv/h) | Method | Duration used (days) | D1S/R2S | Crossing uncertainty |\n|---|---|---|---:|---|---:|---:|---|\n");
        for iteration in &design.computed.iterations {
            for event in &iteration.events {
                let point = event.curve.as_ref().and_then(|c| c.points.first());
                let method = point.map_or("—".into(), |p| format!("{:?}", p.selected_method));
                let ratio = point
                    .and_then(|p| p.d1s_over_r2s)
                    .map_or("—".into(), |v| format!("{v:.4}"));
                let uncertainty =
                    point.map_or_else(|| "NOT_EVALUATED".into(), |p| cell(&p.crossing_uncertainty));
                let _ = writeln!(
                    out,
                    "| {} / {} #{} | {} | {} | {:.4e} | {} | {:.3} | {} | {} |",
                    cell(&event.class),
                    cell(&event.component),
                    event.replacement_k,
                    cell(&event.location_id),
                    cell(&event.dose_limit_source),
                    event.dose_limit_sv_h,
                    method,
                    event.duration_used_s / faris_model::units::DAY_S,
                    ratio,
                    uncertainty
                );
            }
        }
        out.push('\n');
    }
    out
}

fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
fn days(seconds: f64) -> String {
    Show::new(&DisplayPrefs::default()).days(seconds)
}

fn availability(summary: &HistorySummary, show: &Show) -> String {
    format!("{} %", show.number.fixed(summary.availability * 100.0, 2))
}

fn electricity(summary: &HistorySummary, show: &Show) -> String {
    summary.lifetime_net_electricity_mwh.map_or_else(
        || "not available".into(),
        |mwh| {
            // The result file holds MWh; the unit registry holds the factor to the shown unit.
            let mwh_unit = units::find_in("energy_plant", "MWh").unwrap_or(show.energy);
            show.digits(
                units::convert(mwh, mwh_unit, show.energy).unwrap_or(mwh),
                0,
                show.energy.symbol == "MWh",
            )
        },
    )
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

#[cfg(test)]
fn markdown(result: &MaintenanceResult) -> String {
    markdown_with(result, &Show::new(&DisplayPrefs::default()))
}

fn markdown_with(result: &MaintenanceResult, show: &Show) -> String {
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

    if let Some(produced) = &result.produced_by {
        let _ = writeln!(out, "Produced by FARIS {}.\n", produced.faris_version);
    }

    let _ = writeln!(out, "## Per design\n");
    let (d, e) = (show.duration.symbol, show.energy.symbol);
    let _ = writeln!(
        out,
        "| Design | Status | Downtime fixed ({d}) | Downtime computed ({d}) | Availability fixed | \
         Availability computed | Net electricity fixed ({e}) | Net electricity computed ({e}) |"
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
                    show.days(fixed.total_replacement_downtime_s),
                    show.days(c.total_replacement_downtime_s),
                    availability(fixed, show),
                    availability(c, show),
                    electricity(fixed, show),
                    electricity(c, show)
                );
            }
            _ => {
                let _ = writeln!(
                    out,
                    "| {} | NOT_EVALUATED | {} | NOT_EVALUATED | {} | NOT_EVALUATED | {} | NOT_EVALUATED |",
                    cell(name),
                    show.days(fixed.total_replacement_downtime_s),
                    availability(fixed, show),
                    electricity(fixed, show)
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
        let _ = writeln!(
            out,
            "| Class | q* ({}) | Source | Note |",
            show.density.symbol
        );
        let _ = writeln!(out, "|---|---:|---|---|");
        for (class, th) in &design.thresholds {
            let (q, note) = match (&th.q_star_w_per_m3, &th.not_evaluated) {
                (Some(q), _) => (
                    show.number.scientific(show.density.from_si(*q), 4),
                    String::new(),
                ),
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
        let (y, d) = (show.service.symbol, show.duration.symbol);
        let _ = writeln!(
            out,
            "| Component | k | Start ({y}) | Fixed ({d}) | Computed ({d}) | Governing component | Share | In service ({y}) | Note |"
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
                show.years(e.start_s),
                show.days(e.fixed_duration_s),
                e.duration_computed_s
                    .map_or_else(|| "n/a".into(), |s| show.days(s)),
                governing.map_or_else(|| "n/a".into(), |g| cell(&g.component)),
                governing.map_or_else(|| "n/a".into(), |g| format!("{:.0} %", g.share * 100.0)),
                governing.map_or_else(|| "n/a".into(), |g| show.years(g.in_service_s)),
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
            "| A | B | Fixed difference ({d}) | Computed difference ({d}) | Computed / fixed | Note |",
            d = show.duration.symbol
        );
        let _ = writeln!(out, "|---|---|---:|---:|---:|---|");
        for c in &result.contrasts {
            let _ = writeln!(out, "{}", contrast_row_with(c, show));
        }
    }
    out
}

#[cfg(test)]
fn contrast_row(c: &Contrast) -> String {
    contrast_row_with(c, &Show::new(&DisplayPrefs::default()))
}

fn contrast_row_with(c: &Contrast, show: &Show) -> String {
    let note = c
        .not_evaluated
        .as_ref()
        .map(|ne| format!("{} Next step: {}", ne.reason, ne.next_step))
        .unwrap_or_default();
    format!(
        "| {} | {} | {} | {} | {} | {} |",
        cell(&c.a),
        cell(&c.b),
        show.days(c.fixed_difference_s),
        c.computed_difference_s
            .map_or_else(|| "n/a".into(), |s| show.days(s)),
        c.ratio_computed_over_fixed
            .map_or_else(|| "n/a".into(), |r| show.number.fixed(r, 2)),
        cell(&note)
    )
}

#[cfg(test)]
mod tests;
