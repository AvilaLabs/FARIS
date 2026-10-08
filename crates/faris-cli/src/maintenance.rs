//! Computed maintenance durations: run the coupling of operating history and ACTINV decay
//! heat for any number of designs, and report the result.
use crate::transport::{read_bounded, write_new_json};
use clap::{Subcommand, ValueEnum};
use faris_engine::{
    history::{
        HistoryResult, JULIAN_YEAR_SECONDS, TransportDrivingRates,
        run_operating_history_cancellable,
    },
    jobs::Cancellation,
    maintenance::{
        ComputedResult, Contrast, DecaySourceRecord, DesignInput, DesignResult, EventRecord,
        HistorySummary, MAINTENANCE_RESULT_VERSION, MaintenanceResult, Status,
        actinv::{ActinvDecaySource, ActinvSourceConfig, DesignFiles},
        run_maintenance,
    },
};
use faris_model::{
    LoadedScenario,
    history::OperatingHistoryAssumptions,
    maintenance::{MaintenanceAssumptions, Threshold},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    io::Read,
    path::{Path, PathBuf},
};

pub const DESIGNS_VERSION: &str = "faris-maintenance-designs/v0.1";
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
struct ResolvedDesign {
    scenario: PathBuf,
    physics: PathBuf,
    history_run: PathBuf,
    spectrum_run: PathBuf,
    history_assumptions: PathBuf,
}

impl ResolvedDesign {
    fn files(&self) -> [(&'static str, &Path); 5] {
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
fn parse_designs(
    bytes: &[u8],
    base: &Path,
) -> Result<BTreeMap<String, ResolvedDesign>, Box<dyn std::error::Error>> {
    let file: DesignsFile = serde_json::from_slice(bytes)?;
    if file.schema_version != DESIGNS_VERSION {
        return Err(format!("designs file schema_version must be {DESIGNS_VERSION}").into());
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
                return Err(
                    format!("design {name}: {what} {} is not a file", path.display()).into(),
                );
            }
        }
        out.insert(name, resolved);
    }
    Ok(out)
}

fn sha256_file(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut file = std::fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(format!("{:x}", digest.finalize()))
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
    let assumptions: MaintenanceAssumptions =
        serde_json::from_slice(&read_bounded(&request.assumptions)?)?;
    assumptions.validate().map_err(std::io::Error::other)?;
    let designs_base = request
        .designs
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let designs = parse_designs(&read_bounded(&request.designs)?, designs_base)?;
    for spec in assumptions.classes.values() {
        if let Threshold::Calibrate { design, .. } = &spec.threshold
            && !designs.contains_key(design)
        {
            return Err(format!(
                "a class calibrates on design {design}, which the designs file does not name"
            )
            .into());
        }
    }

    // Rates from the verified history runs, and the fixed-duration assumptions.
    let mut rates: BTreeMap<String, TransportDrivingRates> = BTreeMap::new();
    let mut inputs_for_run = Vec::new();
    let mut files: BTreeMap<String, DesignFiles> = BTreeMap::new();
    for (name, design) in &designs {
        eprintln!("maintenance: {name}: binding transport rates from the history run");
        let scenario = LoadedScenario::from_bytes(&read_bounded(&design.scenario)?)?;
        rates.insert(
            name.clone(),
            crate::history::rates_from_run(&scenario, &design.history_run)?,
        );
        let history_assumptions: OperatingHistoryAssumptions =
            serde_json::from_slice(&read_bounded(&design.history_assumptions)?)?;
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
    inputs.insert(
        "assumptions".to_string(),
        sha256_file(&request.assumptions)?,
    );
    inputs.insert("designs".to_string(), sha256_file(&request.designs)?);
    for (name, design) in &designs {
        for (what, path) in design.files() {
            inputs.insert(format!("design/{name}/{what}"), sha256_file(path)?);
        }
    }

    let work_dir = request
        .work_dir
        .unwrap_or_else(|| append_suffix(&request.output, ".work"));
    let cache = request
        .cache
        .unwrap_or_else(|| work_dir.join("decay-cache"));
    let mut config = ActinvSourceConfig::new(
        request.builder.clone(),
        request.actinv.clone(),
        request.data_dir,
        cache,
        work_dir,
    );
    config.python = request.python;
    config.workers = request.workers;
    config.impurities = request.impurities.clone();
    let mut source = ActinvDecaySource::new(config, files)?.with_log(Box::new(|line| {
        eprintln!("maintenance: {line}");
    }));
    // The tools are hashed after they are found, so a bad path is reported first.
    inputs.insert(
        "actinv".to_string(),
        sha256_file(&resolved(&request.actinv)?)?,
    );
    inputs.insert("builder".to_string(), sha256_file(&request.builder)?);
    if let Some(path) = &request.impurities {
        inputs.insert("impurities".to_string(), sha256_file(path)?);
    }

    let interrupts = crate::control::interrupt_cancellation()?;
    #[cfg(unix)]
    let cancellation = &interrupts.cancellation;
    #[cfg(not(unix))]
    let cancellation = &interrupts;
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
        cancellation,
        &mut |design, iteration, message| {
            if iteration == 0 {
                eprintln!("maintenance: {design}: {message}");
            } else {
                eprintln!("maintenance: {design}: iteration {iteration}: {message}");
            }
        },
    )?;
    let (actinv_runs, cache_hits) = source.stats();
    result.inputs = inputs;
    result.decay_source = Some(DecaySourceRecord {
        kind: "actinv-continuations".into(),
        actinv_runs,
        cache_hits,
    });
    write_new_json(&request.output, &result)?;
    for (name, design) in &result.designs {
        println!("{name}: {}", summary_line(design));
    }
    println!(
        "Maintenance result recorded at {} ({actinv_runs} ACTINV runs, {cache_hits} cached curves)",
        request.output.display()
    );
    eprintln!("{}", faris_model::RESEARCH_SCREENING_STATEMENT);
    Ok(())
}

fn resolved(program: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if program.components().count() > 1 || program.is_absolute() {
        return Ok(std::path::absolute(program)?);
    }
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|d| d.join(program))
                .find(|p| p.is_file())
        })
        .ok_or_else(|| format!("executable {} not found", program.display()).into())
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
