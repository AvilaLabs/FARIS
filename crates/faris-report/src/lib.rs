//! Study export: a human-readable folder derived from a study. It holds a
//! two-page PDF summary, CSV tables, SVG and PNG charts and a manifest naming
//! every file by SHA-256. The `.faris` study file stays the source of truth;
//! the export names its hash when the study has been saved.
//!
//! All numbers come from `faris-engine`; this crate formats and draws them.

mod assemble;
mod charts;
mod fonts;
mod pdf;
mod svg;
mod tables;
mod uncertainty;

pub use assemble::{
    LoadedArrangement, NO_VIEW_ON_COMMAND_LINE, ReportContext, assemble_report_input,
    study_name_from_path,
};
pub use charts::{Band, BandSeries, ChartSvg, LimitLine, TimelineSeries};
pub use uncertainty::{Column, ColumnStatus, EnsembleInput, UncertaintyReport};

use faris_engine::{
    brief::{
        Arrangement, ArrangementSummary, AssumptionRow, Caveat, CaveatContext, StatusKind,
        TransportSummary, assumption_rows, caveats, compare_study, decimate, magnet_limit,
        summarize_arrangement,
    },
    comparison::component_replacement_spans,
    history::{HistoryResult, JULIAN_YEAR_SECONDS},
    sweep::{HistorySummary, TransportPoint, history_findings, transport_findings},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Upper bound on plotted points per series in the exported charts.
const MAX_CHART_POINTS: usize = 1500;

pub const FARIS_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("the export folder already exists: {0}")]
    FolderExists(PathBuf),
    #[error("the export input is not usable: {0}")]
    Invalid(String),
    #[error("could not render the export: {0}")]
    Render(String),
    #[error("could not write the export: {0}")]
    Io(#[from] std::io::Error),
}

/// Identity of the saved `.faris` study the export was derived from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StudyFileStamp {
    pub file_name: String,
    pub sha256: String,
}

impl StudyFileStamp {
    /// The stamp of the study file at `path`: its file name and the SHA-256
    /// of its bytes. None when the file cannot be read.
    pub fn from_path(path: &Path) -> Option<Self> {
        let bytes = fs::read(path).ok()?;
        Some(Self {
            file_name: path.file_name()?.to_string_lossy().into_owned(),
            sha256: sha256_hex(&bytes),
        })
    }
}

/// Transport sampling identity of one arrangement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sampling {
    pub seed: u64,
    pub histories: u64,
}

/// One arrangement's engine data. Missing parts stay missing.
#[derive(Clone, Debug)]
pub struct ArrangementInput {
    pub arrangement: Arrangement,
    pub transport: Option<TransportSummary>,
    pub sampling: Option<Sampling>,
    pub history: Option<HistoryResult>,
    /// The Monte Carlo ensemble of the history, when one was calculated.
    pub ensemble: EnsembleInput,
}

/// The allocation sweep: transport points and the history summaries aligned
/// with them (empty until the sweep histories are calculated).
#[derive(Clone, Debug, Default)]
pub struct SweepInput {
    pub points: Vec<TransportPoint>,
    pub summaries: Vec<Option<HistorySummary>>,
}

#[derive(Clone, Debug)]
pub struct ReportInput {
    pub study_name: String,
    pub arrangements: Vec<ArrangementInput>,
    pub sweep: Option<SweepInput>,
    /// Name of the operating-assumption preset the histories use.
    pub preset_label: String,
    /// The preset's own magnet limit, to tell an edited limit from the preset's.
    pub preset_magnet_limit: Option<f64>,
    pub fusion_power_mw: f64,
    /// True when the port volumes are estimates not independently validated.
    pub port_volume_unvalidated: bool,
    /// None for an unsaved study.
    pub study_file: Option<StudyFileStamp>,
    /// PNG bytes of the 3D view, when it could be captured.
    pub view_image: Option<Vec<u8>>,
    /// Why there is no view image; recorded in the manifest.
    pub view_image_note: Option<String>,
    /// Seconds since the Unix epoch, UTC.
    pub generated_unix_s: i64,
}

/// An arrangement with its engine summary.
pub(crate) struct ArrangementData {
    pub input: ArrangementInput,
    pub summary: ArrangementSummary,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileRecord {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug)]
pub struct ExportOutcome {
    pub folder: PathBuf,
    pub files: Vec<FileRecord>,
    pub view_image_included: bool,
}

#[derive(Serialize)]
struct ViewImageRecord {
    included: bool,
    note: Option<String>,
}

#[derive(Serialize)]
struct Manifest<'a> {
    schema: &'static str,
    faris_version: &'static str,
    research_screening: &'static str,
    generated_utc: String,
    study_name: &'a str,
    study_file: Option<&'a StudyFileStamp>,
    view_image: ViewImageRecord,
    /// One entry per arrangement with a history: method, seed (as text),
    /// samples, rejections and status of its ensemble.
    history_ensembles: Vec<uncertainty::EnsembleRecord>,
    /// One entry per contrast of the compare view: compared, or why not.
    history_ensemble_comparisons: Vec<uncertainty::ComparisonRecord>,
    files: &'a [FileRecord],
}

/// `<study name>-export`, with characters unsafe in a folder name replaced.
pub fn folder_name(study_name: &str) -> String {
    let cleaned: String = study_name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.' | '(' | ')') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches(|c| c == '.' || c == ' ' || c == '-');
    let base = if cleaned.is_empty() { "study" } else { cleaned };
    format!("{base}-export")
}

/// `(year, month, day, hour, minute, second)` in UTC.
fn civil(unix_s: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = unix_s.div_euclid(86_400);
    let secs = unix_s.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (
        if m <= 2 { y + 1 } else { y },
        m,
        d,
        (secs / 3600) as u32,
        (secs % 3600 / 60) as u32,
        (secs % 60) as u32,
    )
}

pub fn format_date(unix_s: i64) -> String {
    let (y, m, d, ..) = civil(unix_s);
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn format_timestamp(unix_s: i64) -> String {
    let (y, m, d, hh, mm, ss) = civil(unix_s);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Seconds since the Unix epoch now.
pub fn now_unix_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Rasterise a chart SVG to PNG at `scale` times its natural size.
pub fn rasterize_png(svg_text: &str, scale: f32) -> Result<Vec<u8>, String> {
    let options = usvg::Options {
        fontdb: fonts::font_database(),
        ..Default::default()
    };
    let tree = usvg::Tree::from_str(svg_text, &options).map_err(|e| e.to_string())?;
    let size = tree.size();
    let (w, h) = (
        (size.width() * scale).ceil() as u32,
        (size.height() * scale).ceil() as u32,
    );
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).ok_or("chart size is not drawable")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().map_err(|e| e.to_string())
}

fn question(data: &[&ArrangementData], has_control: bool) -> String {
    let any_swaps = data.iter().any(|d| d.summary.swaps.is_some());
    let any_net = data.iter().any(|d| d.summary.net_twh.is_some());
    let outcomes = match (any_swaps, any_net) {
        (true, true) => "magnet replacements and lifetime net electricity",
        (true, false) => "magnet replacements",
        (false, true) => "lifetime net electricity",
        (false, false) => "tritium breeding and magnet-region flux",
    };
    if has_control {
        format!(
            "How does an outboard maintenance port and the blanket/shield allocation change {outcomes}?"
        )
    } else {
        format!(
            "How does the blanket/shield allocation change {outcomes}? (Only one port setting is loaded.)"
        )
    }
}

struct Prepared {
    data: Vec<ArrangementData>,
    study: faris_engine::brief::StudyComparison,
    timeline: ChartSvg,
    timeline_series: Vec<TimelineSeries>,
    timeline_limit: Option<LimitLine>,
    horizon_years: f64,
    sweep_charts: Option<[ChartSvg; 3]>,
    sweep_findings: Vec<(StatusKind, String)>,
    sweep_note: String,
    assumptions: Vec<AssumptionRow>,
    caveats: Vec<Caveat>,
    view_image: Option<Vec<u8>>,
    view_note: Option<String>,
    uncertainty: Option<UncertaintyReport>,
    /// Paired comparison of each contrast; empty with no uncertainty section.
    comparisons: Vec<uncertainty::ContrastComparison>,
    /// Tritium and net-electricity charts with their bands.
    band_charts: Vec<ChartSvg>,
}

fn prepare(input: &ReportInput) -> Result<Prepared, ExportError> {
    if input.study_name.trim().is_empty() {
        return Err(ExportError::Invalid("the study needs a name".into()));
    }
    let mut seen = Vec::new();
    for a in &input.arrangements {
        if seen.contains(&a.arrangement.id()) {
            return Err(ExportError::Invalid(format!(
                "arrangement {} appears twice",
                a.arrangement.id()
            )));
        }
        seen.push(a.arrangement.id());
    }
    let data: Vec<ArrangementData> = Arrangement::ORDER
        .iter()
        .filter_map(|arrangement| {
            input
                .arrangements
                .iter()
                .find(|a| a.arrangement == *arrangement)
        })
        .map(|a| ArrangementData {
            summary: summarize_arrangement(a.transport.as_ref(), a.history.as_ref()),
            input: a.clone(),
        })
        .collect();
    let cell = |port: bool, breeder: bool| {
        data.iter()
            .find(|d| d.input.arrangement == Arrangement { port, breeder })
            .map(|d| d.summary.clone())
            .unwrap_or_default()
    };
    let cells = [
        [cell(true, false), cell(true, true)],
        [cell(false, false), cell(false, true)],
    ];
    let study = compare_study(&cells);

    let with_history: Vec<&ArrangementData> =
        data.iter().filter(|d| d.input.history.is_some()).collect();
    let horizon_years = with_history
        .iter()
        .filter_map(|d| d.input.history.as_ref())
        .map(|h| h.assumptions.horizon_s / JULIAN_YEAR_SECONDS)
        .fold(0.0, f64::max);
    let series: Vec<TimelineSeries> = with_history
        .iter()
        .filter_map(|d| {
            let h = d.input.history.as_ref()?;
            let raw: Vec<[f64; 2]> = h
                .snapshots
                .iter()
                .filter_map(|s| {
                    faris_engine::history::limit_exposure_n_m2(&h.assumptions, "magnets", s)
                        .map(|v| [s.time_s / JULIAN_YEAR_SECONDS, v])
                })
                .collect();
            Some(TimelineSeries {
                arrangement: d.input.arrangement,
                points: decimate(raw, MAX_CHART_POINTS),
                swap_spans_years: component_replacement_spans(
                    &h.events,
                    "magnets",
                    h.assumptions.horizon_s,
                )
                .into_iter()
                .map(|(a, b)| (a / JULIAN_YEAR_SECONDS, b / JULIAN_YEAR_SECONDS))
                .collect(),
                band: uncertainty::fluence_band(d),
            })
        })
        .collect();
    let limit = with_history
        .iter()
        .filter_map(|d| d.input.history.as_ref())
        .find_map(|h| magnet_limit(h, input.preset_magnet_limit))
        .map(|(value, literature)| LimitLine { value, literature });
    let horizon_years = horizon_years.max(1.0);
    let timeline = charts::timeline_chart(&series, limit, horizon_years, charts::TIMELINE_SIZE.1);

    let (sweep_charts, sweep_findings, sweep_note, sweep_points, sweep_ready) = match input
        .sweep
        .as_ref()
        .filter(|s| !s.points.is_empty())
    {
        Some(sweep) => {
            let ready = sweep.summaries.len() == sweep.points.len();
            let charts = [
                charts::sweep_breeding_chart(&sweep.points),
                charts::sweep_flux_chart(&sweep.points),
                charts::sweep_history_chart(&sweep.points, &sweep.summaries),
            ];
            let mut findings: Vec<(StatusKind, String)> = transport_findings(&sweep.points)
                .into_iter()
                .map(|t| (StatusKind::Calculated, t))
                .collect();
            if ready {
                findings.extend(
                    history_findings(&sweep.points, &sweep.summaries)
                        .into_iter()
                        .map(|t| (StatusKind::Conditional, t)),
                );
            }
            let note = format!(
                "{} real transport runs, one per blanket thickness; the shield takes the rest of {} m",
                sweep.points.len(),
                faris_engine::brief::BLANKET_PLUS_SHIELD_M
            );
            (Some(charts), findings, note, sweep.points.len(), ready)
        }
        None => (None, Vec::new(), String::new(), 0, false),
    };

    let assumptions = with_history
        .first()
        .and_then(|d| d.input.history.as_ref())
        .map(|h| assumption_rows(h, input.fusion_power_mw, input.preset_magnet_limit))
        .unwrap_or_default();

    // The view image must actually decode as a PNG to count as included.
    let (view_image, view_note) = match &input.view_image {
        Some(bytes) if krilla::image::Image::from_png(bytes.clone().into(), true).is_ok() => {
            (Some(bytes.clone()), None)
        }
        Some(_) => (
            None,
            Some("The supplied 3D view image is not a readable PNG.".to_string()),
        ),
        None => (
            None,
            Some(
                input
                    .view_image_note
                    .clone()
                    .unwrap_or_else(|| "No 3D view image was supplied.".into()),
            ),
        ),
    };

    let flat: Vec<(Arrangement, ArrangementSummary)> = Arrangement::ORDER
        .iter()
        .map(|a| {
            (
                *a,
                data.iter()
                    .find(|d| d.input.arrangement == *a)
                    .map(|d| d.summary.clone())
                    .unwrap_or_default(),
            )
        })
        .collect();
    let mut list = caveats(&CaveatContext {
        arrangements: &flat,
        sweep_points,
        sweep_histories_ready: sweep_ready,
        view_image_included: view_image.is_some(),
        port_volume_unvalidated: input.port_volume_unvalidated,
    });
    if with_history.is_empty() {
        list.insert(
            0,
            Caveat {
                kind: StatusKind::NotEvaluated,
                item: "Operating histories".into(),
                why: "No calculated operating history was available, so swaps, net electricity, the fluence chart and the assumptions table are empty.".into(),
                settle: "Wait for the histories to finish calculating, then export again.".into(),
            },
        );
    }
    let uncertainty_report = uncertainty::build(&data);
    let (tritium_bands, electricity_bands) = uncertainty::band_series(&data);
    let mut band_charts = Vec::new();
    if !tritium_bands.is_empty() {
        band_charts.push(charts::band_chart(
            "history-tritium-bands",
            &format!("Usable tritium over {horizon_years:.0} years of operation"),
            "Usable tritium (kg)",
            &tritium_bands,
            horizon_years,
            charts::BAND_CHART_HEIGHT,
        ));
    }
    if !electricity_bands.is_empty() {
        band_charts.push(charts::band_chart(
            "history-net-electricity-bands",
            &format!("Cumulative signed net electricity over {horizon_years:.0} years"),
            "Net electricity (TWh)",
            &electricity_bands,
            horizon_years,
            charts::BAND_CHART_HEIGHT,
        ));
    }
    Ok(Prepared {
        comparisons: uncertainty::contrast_comparisons(&data),
        data,
        study,
        timeline,
        timeline_series: series,
        timeline_limit: limit,
        horizon_years,
        sweep_charts,
        sweep_findings,
        sweep_note,
        assumptions,
        caveats: list,
        view_image,
        view_note,
        uncertainty: uncertainty_report,
        band_charts,
    })
}

fn footer(input: &ReportInput) -> String {
    let stamp = match &input.study_file {
        Some(s) => format!("Study file: {} sha256:{}", s.file_name, s.sha256),
        None => "Unsaved study — no study file hash".into(),
    };
    format!(
        "FARIS {FARIS_VERSION} · {} · {stamp}",
        format_date(input.generated_unix_s)
    )
}

/// The timeline chart at a requested height, for the PDF page that decides it.
fn timeline_fn(p: &Prepared) -> impl Fn(f32) -> ChartSvg + '_ {
    move |height| {
        charts::timeline_chart(
            &p.timeline_series,
            p.timeline_limit,
            p.horizon_years,
            height,
        )
    }
}

fn pdf_content<'a>(
    input: &'a ReportInput,
    p: &'a Prepared,
    order: &'a [&'a ArrangementData],
    timeline: &'a dyn Fn(f32) -> ChartSvg,
) -> pdf::PdfContent<'a> {
    let has_control = order
        .iter()
        .any(|d| !d.input.arrangement.port && d.summary.recorded);
    let has_port = order
        .iter()
        .any(|d| d.input.arrangement.port && d.summary.recorded);
    pdf::PdfContent {
        title: &input.study_name,
        subtitle: format!(
            "Operating assumptions: {} · fusion power {} MW · four arrangements, real transport results",
            input.preset_label, input.fusion_power_mw
        ),
        question: question(order, has_control && has_port),
        takeaways: &p.study.takeaways,
        arrangements: order,
        study: &p.study,
        timeline,
        sweep_charts: p.sweep_charts.as_ref().map(|[a, b, c]| [a, b, c]),
        sweep_findings: p.sweep_findings.clone(),
        sweep_note: p.sweep_note.clone(),
        assumptions: &p.assumptions,
        caveats: &p.caveats,
        footer: footer(input),
        view_image: p.view_image.as_deref(),
        uncertainty: p.uncertainty.as_ref(),
        band_charts: p.band_charts.iter().collect(),
    }
}

/// Build every file of the export in memory: `(relative path, bytes)`.
fn build_files(input: &ReportInput, p: &Prepared) -> Result<Vec<(String, Vec<u8>)>, ExportError> {
    let order: Vec<&ArrangementData> = p.data.iter().collect();
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let draw = timeline_fn(p);
    let content = pdf_content(input, p, &order, &draw);
    files.push((
        "summary.pdf".into(),
        pdf::render_pdf(&content).map_err(ExportError::Render)?,
    ));

    files.push((
        "data/histories.csv".into(),
        tables::with_statement(tables::histories_csv(&order)).into_bytes(),
    ));
    files.push((
        "data/comparison.csv".into(),
        tables::with_statement(tables::comparison_csv(&order)).into_bytes(),
    ));
    files.push((
        "data/differences.csv".into(),
        tables::with_statement(tables::differences_csv(&p.study)).into_bytes(),
    ));
    let sweep_csv = input.sweep.as_ref().map_or_else(
        || tables::sweep_csv(&[], &[]),
        |s| tables::sweep_csv(&s.points, &s.summaries),
    );
    files.push((
        "data/sweep.csv".into(),
        tables::with_statement(sweep_csv).into_bytes(),
    ));
    if p.uncertainty.is_some() {
        files.push((
            "data/history-ensemble-samples.csv".into(),
            tables::with_statement(tables::ensemble_samples_csv(&order)).into_bytes(),
        ));
        files.push((
            "data/history-ensemble-summary.csv".into(),
            tables::with_statement(tables::ensemble_summary_csv(&order)).into_bytes(),
        ));
        files.push((
            "data/history-ensemble-comparison.csv".into(),
            tables::with_statement(tables::ensemble_comparison_csv(&p.comparisons)).into_bytes(),
        ));
    }
    files.push((
        "data/assumptions.csv".into(),
        tables::with_statement(tables::assumptions_csv(&p.assumptions)).into_bytes(),
    ));
    files.push((
        "data/caveats.csv".into(),
        tables::with_statement(tables::caveats_csv(&p.caveats)).into_bytes(),
    ));

    let mut all_charts: Vec<(&ChartSvg, f32)> = vec![(&p.timeline, 3.0)];
    if let Some(sweep) = &p.sweep_charts {
        all_charts.extend(sweep.iter().map(|c| (c, 4.0)));
    }
    all_charts.extend(p.band_charts.iter().map(|c| (c, 3.0)));
    for (chart, scale) in all_charts {
        let svg = chart.export_svg();
        files.push((
            format!("charts/{}.svg", chart.name),
            svg.clone().into_bytes(),
        ));
        files.push((
            format!("charts/{}.png", chart.name),
            rasterize_png(&svg, scale).map_err(ExportError::Render)?,
        ));
    }
    if let Some(view) = &p.view_image {
        files.push(("charts/3d-view.png".into(), view.clone()));
    }
    Ok(files)
}

/// Write the export into `<parent>/<study name>-export/`. Refuses to write
/// into an existing folder; the files go to a temporary sibling first and the
/// folder appears under its final name only when complete.
pub fn export_study(input: &ReportInput, parent: &Path) -> Result<ExportOutcome, ExportError> {
    let final_dir = parent.join(folder_name(&input.study_name));
    if final_dir.exists() {
        return Err(ExportError::FolderExists(final_dir));
    }
    let prepared = prepare(input)?;
    let files = build_files(input, &prepared)?;

    let staging = tempfile::Builder::new()
        .prefix(".faris-export-")
        .tempdir_in(parent)?;
    let mut records = Vec::with_capacity(files.len() + 1);
    for (path, bytes) in &files {
        let target = staging.path().join(path);
        if let Some(dir) = target.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&target, bytes)?;
        records.push(FileRecord {
            path: path.clone(),
            sha256: sha256_hex(bytes),
            bytes: bytes.len() as u64,
        });
    }
    records.sort_by(|a, b| a.path.cmp(&b.path));
    let manifest = Manifest {
        schema: "faris-export/1",
        faris_version: FARIS_VERSION,
        research_screening: faris_model::RESEARCH_SCREENING_STATEMENT,
        generated_utc: format_timestamp(input.generated_unix_s),
        study_name: &input.study_name,
        study_file: input.study_file.as_ref(),
        view_image: ViewImageRecord {
            included: prepared.view_image.is_some(),
            note: prepared.view_note.clone(),
        },
        history_ensembles: uncertainty::manifest_records(&prepared.data),
        history_ensemble_comparisons: uncertainty::comparison_records(&prepared.comparisons),
        files: &records,
    };
    let mut text =
        serde_json::to_string_pretty(&manifest).map_err(|e| ExportError::Render(e.to_string()))?;
    text.push('\n');
    fs::write(staging.path().join("export-manifest.json"), text)?;

    // The check above can race with another writer; rename onto an existing
    // directory would silently replace an empty one, so look again.
    if final_dir.exists() {
        return Err(ExportError::FolderExists(final_dir));
    }
    let staged = staging.keep();
    fs::rename(&staged, &final_dir).inspect_err(|_| {
        let _ = fs::remove_dir_all(&staged);
    })?;
    Ok(ExportOutcome {
        folder: final_dir,
        files: records,
        view_image_included: prepared.view_image.is_some(),
    })
}

#[cfg(test)]
mod tests;
