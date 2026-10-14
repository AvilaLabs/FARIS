//! `faris design transport`: the neutron flux in the solids a fluence limit of a checked
//! design applies to (the TF magnets, when the design has no such limit).
//!
//! The design must have passed `faris design check` with every check evaluated; the
//! check report names the conversion whose h5m is run. The OpenMC worker
//! (`integrations/openmc/design_transport.py`) runs as a bounded job and stops at a
//! target relative error or at its history or time limit. It answers per source
//! neutron; the source rate and the volumes are applied here.
//!
//! With an OpenMC 0.16 interpreter for the window step, two jobs run first: a
//! multigroup library on the design's materials and a random-ray adjoint of the
//! response, which gives weight windows. The run then uses them and, on an OpenMC
//! built with the aiming patch, aims at a streaming channel when it finds one.
//! The flux is then compared with the design's fluence limit over the linked
//! operating history (`life`): the mean over the group for a `group_average`
//! limit, the highest piece of any solid for a `peak_mesh` limit. A piece is a
//! ring of the limit's dr by dz about the machine axis, cut by the solid.

use super::life::{FluenceAssessment, LimitStatus, PowerHistory, assess_fluence, power_history};
use super::{
    AIM_COMMON_HELPER, AIM_TARGETS_HELPER, AIM_WINDOWS_HELPER, CAD_ADDRESS_SPACE,
    DESIGN_TRANSPORT_WORKER, DesignError, DesignIdentity, ImportReport, Launched, MAX_JSON_BYTES,
    STREAMING_AIM_HELPER, ToolPaths, io_error, launch_python_with_lines, resolve, sha256_file,
    sha256_hex,
};
use crate::jobs::{Cancellation, ResourceLimits};
use crate::transport::{D_T_ENERGY_PER_REACTION_EV, reaction_rate_per_s};
use faris_model::design::{self, Design, Metric, Role};
use faris_model::history::OperatingHistoryAssumptions;
use faris_model::transport::FAST_NEUTRON_ENERGY_MIN_EV;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fmt::Write as _,
    path::{Path, PathBuf},
    time::Duration,
};

pub const DESIGN_TRANSPORT_VERSION: &str = "faris-design-transport-report/v0.3";
const REQUEST_SCHEMA: &str = "faris-design-transport-request/v3";
const RESULT_SCHEMA: &str = "faris-design-transport/v3";
/// The fewest batches a run may stop at, as for the reactor run's sampling plan.
const MIN_BATCHES: u64 = 30;
/// Time the run gets beyond its sampling limit to load the geometry, look for a streaming
/// channel and write its result.
const STARTUP_ALLOWANCE: Duration = Duration::from_secs(30 * 60);
const MGXS_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const WINDOWS_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const VOLUMES_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Where a run is: a step that prepares it, or sampling with the error reached so far.
#[derive(Clone, Debug, PartialEq)]
pub enum TransportProgress {
    /// A step has started: `mgxs`, `windows`, `volumes` or `run`.
    Step(String),
    Sampling {
        histories: u64,
        /// `None` until two batches have scored.
        relative_error: Option<f64>,
        /// The error of the highest piece, when the limit asks for a peak and it is known.
        peak_relative_error: Option<f64>,
        elapsed_seconds: f64,
    },
}

/// Called from the thread that reads the worker's output.
pub type TransportProgressFn = std::sync::Arc<dyn Fn(TransportProgress) + Send + Sync>;

/// A worker output line as progress, if it is one.
fn parse_progress(line: &str) -> Option<TransportProgress> {
    #[derive(Deserialize)]
    struct Line {
        progress: Sampling,
    }
    #[derive(Deserialize)]
    struct Sampling {
        histories: u64,
        relative_error: Option<f64>,
        #[serde(default)]
        peak_relative_error: Option<f64>,
        elapsed_seconds: f64,
    }
    if !line.starts_with("{\"progress\"") {
        return None;
    }
    let parsed: Line = serde_json::from_str(line).ok()?;
    Some(TransportProgress::Sampling {
        histories: parsed.progress.histories,
        relative_error: parsed.progress.relative_error,
        peak_relative_error: parsed.progress.peak_relative_error,
        elapsed_seconds: parsed.progress.elapsed_seconds,
    })
}

#[derive(Clone)]
pub struct DesignTransportOptions {
    /// The JSON report `faris design check --report` wrote for this design.
    pub check_report: PathBuf,
    /// A new directory for the run.
    pub out_dir: PathBuf,
    pub target_relative_error: f64,
    pub particles_per_batch: u64,
    pub max_histories: u64,
    pub max_seconds: u64,
    pub seed: u64,
    pub threads: u32,
    /// A Python with OpenMC 0.16 or later, for the weight-window step. `None`: an analog run.
    pub window_python: Option<PathBuf>,
    /// A directory holding an `openmc` package to use in place of the interpreter's own, such
    /// as a build with the aiming patch.
    pub openmc_package_path: Option<PathBuf>,
    /// The replacement group whose fluence limit names the response. `None`: the only group
    /// with a fluence limit, or the TF magnets when the design has no such limit.
    pub group: Option<String>,
    /// Where weight windows and the channel search are kept between runs. `None`: nothing is kept.
    pub cache_dir: Option<PathBuf>,
    pub progress: Option<TransportProgressFn>,
    pub cancellation: Cancellation,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct WorkerValue {
    mean: f64,
    standard_error: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct WorkerSolid {
    solid_id: String,
    mean: f64,
    standard_error: Option<f64>,
    #[serde(default)]
    peak: Option<WorkerPeak>,
}

/// The piece of a solid with the highest flux: track length (cm per source neutron) in its volume.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct WorkerPeak {
    mean: f64,
    standard_error: Option<f64>,
    volume_cm3: f64,
    r_cm: [f64; 2],
    z_cm: [f64; 2],
    pieces_ranked: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct WorkerResult {
    schema: String,
    openmc: String,
    h5m_sha256: String,
    run_h5m_sha256: String,
    model_xml_sha256: String,
    seed: u64,
    threads: u32,
    particles_per_batch: u64,
    batches: u64,
    histories: u64,
    elapsed_seconds: f64,
    stop_reason: String,
    target_relative_error: f64,
    target_reached: bool,
    estimator: String,
    unit: String,
    energy_min_ev: f64,
    total: WorkerValue,
    solids: Vec<WorkerSolid>,
    variance_reduction: VarianceReduction,
    why: Option<String>,
    next_step: Option<String>,
}

/// How the run was sped up, with the reason for the mode and what the user can do.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct VarianceReduction {
    /// `analog`, `windows-only` or `aimed`.
    pub mode: String,
    pub reason: String,
    pub next_step: String,
    /// The streaming targets an aimed run found.
    #[serde(default)]
    pub targets: Vec<String>,
    pub windows_sha256: Option<String>,
    /// Seconds the run spent looking for a streaming channel before sampling.
    #[serde(default)]
    pub aim_seconds: f64,
    /// The channel search of an earlier run on the same model was used.
    #[serde(default)]
    pub aim_reused: bool,
}

/// One fluence limit of the design against the flux its averaging rule names.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct FluenceLimit {
    pub group_id: String,
    /// `group_average`: the mean over the group's solids. `peak_piece`: the highest piece of
    /// any solid of the group.
    pub basis: String,
    /// The solid that holds the highest piece; `None` for a group average.
    pub solid_id: Option<String>,
    pub assessment: FluenceAssessment,
}

/// The piece of a solid with the highest flux: a ring about the machine axis, cut by the solid.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PeakFlux {
    pub r_m: [f64; 2],
    pub z_m: [f64; 2],
    pub volume_m3: f64,
    pub flux_n_m2_s: f64,
    pub relative_error: Option<f64>,
    /// How many pieces of the solid were compared.
    pub pieces_ranked: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SolidFlux {
    pub solid_id: String,
    pub volume_m3: f64,
    pub flux_n_m2_s: f64,
    /// `None`: fewer than two batches, or no score.
    pub relative_error: Option<f64>,
    /// `None`: the limit asks for no peak, or the solid has no piece to rank.
    pub peak: Option<PeakFlux>,
}

/// Something the run did not compute, with the reason and what to do.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct NotEvaluated {
    pub item: String,
    pub why: String,
    pub next_step: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DesignTransportReport {
    pub schema_version: String,
    pub design: DesignIdentity,
    pub check_report_sha256: String,
    pub h5m_sha256: String,
    pub cross_sections_sha256: String,
    pub openmc: String,
    pub run_directory: String,
    pub energy_min_ev: f64,
    /// The replacement group the response was taken from; `None`: the TF magnets, by role.
    pub response_group_id: Option<String>,
    /// Source neutrons per second, from the fusion power at 17.6 MeV per reaction.
    pub source_rate_per_s: f64,
    /// Energy per reaction the source rate was derived with (the convention, stored).
    #[serde(default = "default_energy_per_reaction_ev")]
    pub energy_per_reaction_ev: f64,
    pub fusion_power_mw: f64,
    pub histories: u64,
    pub elapsed_seconds: f64,
    pub seed: u64,
    pub threads: u32,
    pub target_relative_error: f64,
    pub target_reached: bool,
    pub stop_reason: String,
    /// Why the target was not reached and what to do; `None` when it was.
    pub why: Option<String>,
    pub next_step: Option<String>,
    /// Volume-averaged flux above the threshold over all TF magnet solids.
    pub mean_flux_n_m2_s: f64,
    pub mean_relative_error: Option<f64>,
    pub total_volume_m3: f64,
    pub solids: Vec<SolidFlux>,
    pub variance_reduction: VarianceReduction,
    /// Seconds spent before sampling on weight windows and the search for a streaming channel.
    pub setup_seconds: f64,
    /// The weight windows of an earlier run on the same model were used.
    pub windows_reused: bool,
    pub fluence_limits: Vec<FluenceLimit>,
    pub notices: Vec<String>,
    pub not_evaluated: Vec<NotEvaluated>,
}

fn default_energy_per_reaction_ev() -> f64 {
    D_T_ENERGY_PER_REACTION_EV
}

/// Flux in n/m²/s from a track length per source neutron (cm), a source rate and a volume.
pub fn flux_n_m2_s(cm_per_source: f64, source_rate_per_s: f64, volume_m3: f64) -> f64 {
    // cm / cm³ = 1/cm² = 1e4 /m²; the volume is 1e6 cm³ per m³.
    cm_per_source * source_rate_per_s / (volume_m3 * 1.0e6) * 1.0e4
}

fn source_rate_per_s(fusion_power_mw: f64) -> f64 {
    reaction_rate_per_s(fusion_power_mw, D_T_ENERGY_PER_REACTION_EV)
}

fn tool(message: impl Into<String>) -> DesignError {
    DesignError::Tool(message.into())
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, DesignError> {
    let metadata = std::fs::metadata(path).map_err(|e| io_error(path, e))?;
    if !metadata.is_file() || metadata.len() > MAX_JSON_BYTES {
        return Err(io_error(path, "not a regular file under 64 MiB"));
    }
    std::fs::read(path).map_err(|e| io_error(path, e))
}

/// One entry per design material in use: its nuclide vector from the check's audit and its density.
fn material_entries(
    design: &Design,
    report: &ImportReport,
) -> Result<Vec<serde_json::Value>, DesignError> {
    let audited: BTreeMap<&str, &BTreeMap<String, f64>> = report
        .material_audit
        .iter()
        .flat_map(|a| &a.materials)
        .filter_map(|m| Some((m.id.as_str(), m.nuclides.as_ref()?)))
        .collect();
    let mut entries = Vec::new();
    for material in &design.materials {
        if !design
            .solids
            .iter()
            .any(|s| s.material_id.as_deref() == Some(material.id.as_str()))
        {
            continue;
        }
        let material_id = material.id.as_str();
        let nuclides = audited.get(material_id).ok_or_else(|| {
            tool(format!(
                "the check report holds no audited composition for material {material_id}; run \
                 `faris design check` with --openmc-python and --cross-sections and pass its report"
            ))
        })?;
        let density = material
            .density_kg_m3
            .or_else(|| {
                material
                    .catalog_id
                    .as_deref()
                    .and_then(design::catalog_material)
                    .map(|c| c.density_kg_m3)
            })
            .ok_or_else(|| tool(format!("material {material_id} has no density")))?;
        entries.push(serde_json::json!({
            "material_id": material_id,
            "nuclide_atom_fractions": nuclides,
            "density_kg_m3": density,
            "temperature_k": material.temperature_k,
        }));
    }
    Ok(entries)
}

/// Every solid with its DAGMC volume from the check's binding; `material_id` null for empty space.
fn solid_entries(
    design: &Design,
    report: &ImportReport,
) -> Result<Vec<serde_json::Value>, DesignError> {
    let volumes: BTreeMap<&str, u32> = report
        .geometry
        .binding
        .iter()
        .filter_map(|row| Some((row.solid_id.as_deref()?, row.volume)))
        .collect();
    design
        .solids
        .iter()
        .map(|solid| {
            let volume = volumes.get(solid.id.as_str()).ok_or_else(|| {
                tool(format!(
                    "the check report binds no DAGMC volume to solid {}",
                    solid.id
                ))
            })?;
            let material = solid
                .material_id
                .as_deref()
                .ok_or_else(|| tool(format!("solid {} has no material", solid.id)))?;
            Ok(serde_json::json!({
                "solid_id": solid.id,
                "volume_id": volume,
                "material_id": (material != "void").then_some(material),
            }))
        })
        .collect()
}

/// The power history the design links to; an error sentence when it cannot be used.
fn linked_history(
    design_path: &Path,
    design: &Design,
    fusion_power_mw: f64,
) -> Result<PowerHistory, String> {
    let relative = design
        .operating_scenario
        .path
        .as_deref()
        .ok_or("The design file links no operating history.")?;
    let path = resolve(design_path, relative);
    let bytes =
        read_bounded(&path).map_err(|e| format!("The operating history cannot be read: {e}."))?;
    if let Some(expected) = design.operating_scenario.sha256.as_deref()
        && !sha256_hex(&bytes).eq_ignore_ascii_case(expected)
    {
        return Err("The operating history file is not the one the design file records.".into());
    }
    let history: OperatingHistoryAssumptions = serde_json::from_slice(&bytes)
        .map_err(|e| format!("The operating history does not parse: {e}."))?;
    power_history(&history, fusion_power_mw)
}

/// The run's source is the tokamak formula for a D-T plasma. A design that asks for another
/// source is refused, so that it is never run with the wrong one.
fn refuse_unsupported_source(design: &Design) -> Result<(), DesignError> {
    if design.plasma.emissivity_table.is_some() {
        return Err(tool(
            "the design gives an emissivity table, which this command cannot use yet: it would \
             run the tokamak profile instead. Remove emissivity_table to run with the profile",
        ));
    }
    let share = |species: &str| {
        design
            .plasma
            .fuel
            .as_ref()
            .and_then(|fuel| fuel.get(species).copied())
            .unwrap_or(0.0)
    };
    if !(share("D") > 0.0 && share("T") > 0.0) {
        return Err(tool(
            "the design's fuel is not a deuterium-tritium mix; this command's source rate and \
             neutron energy are for D-T only",
        ));
    }
    Ok(())
}

/// What the run scores: the flux above a threshold in the solids of one replacement group.
#[derive(Clone, Debug, PartialEq)]
struct ChosenResponse {
    /// `None`: the TF magnets by role, for a design with no fluence limit.
    group_id: Option<String>,
    energy_min_ev: f64,
    limit_n_m2: Option<f64>,
    /// `[dr, dz]` in metres of a `peak_mesh` limit; `None` in a place: the solid's full width.
    peak_voxel_m: Option<[Option<f64>; 2]>,
    /// The limit asks for pieces of a set toroidal width, which this command does not cut.
    peak_ignores_dphi: bool,
}

fn peak_voxel(limit: &design::Limit) -> (Option<[Option<f64>; 2]>, bool) {
    match &limit.averaging {
        design::Averaging::PeakMesh { voxel_m } if voxel_m.len() == 3 => {
            (Some([voxel_m[0], voxel_m[1]]), voxel_m[2].is_some())
        }
        _ => (None, false),
    }
}

/// The fluence limit a metric and threshold describe: the energy above which neutrons count.
fn fluence_threshold(limit: &design::Limit) -> Option<f64> {
    match limit.metric {
        Metric::FastNeutronFluence => Some(
            limit
                .energy_threshold_ev
                .unwrap_or(FAST_NEUTRON_ENERGY_MIN_EV),
        ),
        Metric::NeutronFluence => Some(0.0),
        _ => None,
    }
}

/// Pick the response from the design's fluence limits. With several groups the caller names one.
fn choose_response(design: &Design, group: Option<&str>) -> Result<ChosenResponse, DesignError> {
    let candidates: Vec<ChosenResponse> = design
        .replacement_groups
        .iter()
        .filter_map(|g| {
            let limit = g.limits.iter().find(|l| fluence_threshold(l).is_some())?;
            let (peak_voxel_m, peak_ignores_dphi) = peak_voxel(limit);
            Some(ChosenResponse {
                group_id: Some(g.id.clone()),
                energy_min_ev: fluence_threshold(limit)?,
                limit_n_m2: Some(limit.limit),
                peak_voxel_m,
                peak_ignores_dphi,
            })
        })
        .collect();
    let ids = || {
        candidates
            .iter()
            .filter_map(|c| c.group_id.as_deref())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match (group, candidates.len()) {
        (Some(wanted), _) => candidates
            .iter()
            .find(|c| c.group_id.as_deref() == Some(wanted))
            .cloned()
            .ok_or_else(|| {
                tool(format!(
                    "the replacement group {wanted} has no fluence limit; the groups with one are: {}",
                    ids()
                ))
            }),
        (None, 0) => Ok(ChosenResponse {
            group_id: None,
            energy_min_ev: FAST_NEUTRON_ENERGY_MIN_EV,
            limit_n_m2: None,
            peak_voxel_m: None,
            peak_ignores_dphi: false,
        }),
        (None, 1) => Ok(candidates[0].clone()),
        (None, _) => Err(tool(format!(
            "several replacement groups have a fluence limit; name one with --group: {}",
            ids()
        ))),
    }
}

/// One entry of the cache per model, materials, response, nuclear data and worker version.
fn cache_key(
    request: &serde_json::Value,
    h5m_sha256: &str,
    cross_sections_sha256: &str,
    window_python: Option<&Path>,
) -> String {
    let identity = serde_json::json!({
        "worker": sha256_hex(DESIGN_TRANSPORT_WORKER.as_bytes()),
        "h5m": h5m_sha256,
        "cross_sections": cross_sections_sha256,
        "solids": request["solids"],
        "materials": request["materials"],
        "chamber_solid_id": request["chamber_solid_id"],
        "response": request["response"],
        "window_python": window_python.map(|p| p.display().to_string()),
    });
    sha256_hex(identity.to_string().as_bytes())
}

fn copy_file(from: &Path, to: &Path) -> Result<(), DesignError> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
    }
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| io_error(from, e))
}

#[derive(Deserialize)]
struct WindowsRecord {
    filled_sha256: String,
}

/// Put a kept entry's weight windows and channel search into the run directory. True when the
/// windows were found and match their record; a damaged entry is ignored, not trusted.
fn restore_windows(entry: &Path, dir: &Path) -> Result<bool, DesignError> {
    let record_path = entry.join("windows").join("record.json");
    let filled = entry.join("windows").join("filled.h5");
    if !record_path.is_file() || !filled.is_file() {
        return Ok(false);
    }
    let Ok(record) = serde_json::from_slice::<WindowsRecord>(&read_bounded(&record_path)?) else {
        return Ok(false);
    };
    if !sha256_file(&filled)?.eq_ignore_ascii_case(&record.filled_sha256) {
        return Ok(false);
    }
    copy_file(&record_path, &dir.join("windows").join("record.json"))?;
    copy_file(&filled, &dir.join("windows").join("filled.h5"))?;
    let plan = entry.join("aim").join("plan.json");
    if plan.is_file() {
        copy_file(&plan, &dir.join("aim-kept").join("plan.json"))?;
        let block = entry.join("aim").join("windows-block.xml");
        if block.is_file() {
            copy_file(&block, &dir.join("aim-kept").join("windows-block.xml"))?;
        }
    }
    Ok(true)
}

const PIECE_FILES: [&str; 2] = ["record.json", "volumes.npz"];

#[derive(Deserialize)]
struct PiecesRecord {
    volumes_sha256: String,
}

/// Put a kept entry's piece volumes into the run directory; a damaged entry is ignored.
fn restore_pieces(entry: &Path, dir: &Path) -> Result<bool, DesignError> {
    let record_path = entry.join("peak").join("record.json");
    let volumes = entry.join("peak").join("volumes.npz");
    if !record_path.is_file() || !volumes.is_file() {
        return Ok(false);
    }
    let Ok(record) = serde_json::from_slice::<PiecesRecord>(&read_bounded(&record_path)?) else {
        return Ok(false);
    };
    if !sha256_file(&volumes)?.eq_ignore_ascii_case(&record.volumes_sha256) {
        return Ok(false);
    }
    for name in PIECE_FILES {
        copy_file(&entry.join("peak").join(name), &dir.join("peak").join(name))?;
    }
    Ok(true)
}

/// Keep this run's weight windows and channel search for the next run on the same model.
fn keep_for_later(dir: &Path, entry: &Path, windows_reused: bool) -> Result<(), DesignError> {
    if !windows_reused {
        for name in ["record.json", "filled.h5"] {
            copy_file(
                &dir.join("windows").join(name),
                &entry.join("windows").join(name),
            )?;
        }
    }
    // The worker writes these only after a search of its own.
    let plan = dir.join("aim").join("plan.json");
    if plan.is_file() {
        let block = dir.join("aim").join("windows-block.xml");
        if block.is_file() {
            copy_file(&block, &entry.join("aim").join("windows-block.xml"))?;
        } else {
            let _ = std::fs::remove_file(entry.join("aim").join("windows-block.xml"));
        }
        copy_file(&plan, &entry.join("aim").join("plan.json"))?;
    }
    Ok(())
}

/// The check report must describe this design file and a complete, passing import.
fn accept_check_report(report: &ImportReport, design_sha256: &str) -> Result<(), DesignError> {
    if !report.design.sha256.eq_ignore_ascii_case(design_sha256) {
        return Err(tool(
            "the check report was written for a different version of the design file; run \
             `faris design check --report` again and pass the new report",
        ));
    }
    if !report.import_complete {
        return Err(tool(format!(
            "the design has not passed all nine import checks ({}); transport runs only on a \
             design that imports. Resolve the findings of `faris design check` first",
            report.summary
        )));
    }
    Ok(())
}

/// Run the flux response of a checked design to a target error.
pub fn design_transport(
    design_path: &Path,
    tools: &ToolPaths,
    options: &DesignTransportOptions,
) -> Result<DesignTransportReport, DesignError> {
    if !(options.target_relative_error > 0.0 && options.target_relative_error < 1.0) {
        return Err(tool("the target relative error must lie between 0 and 1"));
    }
    if options.particles_per_batch == 0
        || options.max_histories < MIN_BATCHES * options.particles_per_batch
    {
        return Err(tool(format!(
            "the history limit must allow at least {MIN_BATCHES} batches of {} histories",
            options.particles_per_batch
        )));
    }
    let python = tools.openmc_python.as_deref().ok_or_else(|| {
        tool("no OpenMC interpreter is configured; pass --openmc-python (or set FARIS_OPENMC_PYTHON)")
    })?;
    let cross_sections = tools.cross_sections.as_deref().ok_or_else(|| {
        tool("no nuclear data is configured; pass --cross-sections (or set FARIS_CROSS_SECTIONS)")
    })?;
    let plasma_source = tools.plasma_source_path.as_deref().ok_or_else(|| {
        tool(
            "no path to openmc_plasma_source is configured; pass --plasma-source-path (or set \
             FARIS_PLASMA_SOURCE_PATH)",
        )
    })?;

    let design_bytes = read_bounded(design_path)?;
    let design_sha256 = sha256_hex(&design_bytes);
    let design = design::parse_design(&design_bytes)
        .design
        .ok_or_else(|| tool("the design file does not parse; run `faris design check`"))?;
    let report_bytes = read_bounded(&options.check_report)?;
    let report: ImportReport = serde_json::from_slice(&report_bytes).map_err(|e| {
        tool(format!(
            "{} is not a design check report: {e}",
            options.check_report.display()
        ))
    })?;
    accept_check_report(&report, &design_sha256)?;
    let conversion = report
        .geometry
        .conversion
        .as_ref()
        .ok_or_else(|| tool("the check report records no conversion"))?;
    let h5m = Path::new(&conversion.directory).join("model.h5m");
    if !h5m.is_file() || !sha256_file(&h5m)?.eq_ignore_ascii_case(&conversion.h5m_sha256) {
        return Err(tool(format!(
            "{} is missing or is not the geometry the check report describes; run `faris design \
             check --work-dir DIR --report FILE` again and keep DIR",
            h5m.display()
        )));
    }

    let chosen = choose_response(&design, options.group.as_deref())?;
    let response: Vec<(&str, f64)> = design
        .solids
        .iter()
        .filter(|s| s.material_id.as_deref() != Some("void"))
        .filter(|s| match &chosen.group_id {
            Some(group) => s.replacement_group_id.as_deref() == Some(group.as_str()),
            None => s.role == Some(Role::TfMagnet),
        })
        .map(|s| (s.id.as_str(), s.fingerprint.cad_volume_m3))
        .collect();
    if response.is_empty() {
        return Err(tool(match &chosen.group_id {
            Some(group) => {
                format!("the replacement group {group} has no solid that is not empty space")
            }
            None => {
                "the design has no fluence limit and no solid with the role tf_magnet, so there \
                     is no response to compute; add a fluence limit to a replacement group"
                    .into()
            }
        }));
    }
    refuse_unsupported_source(&design)?;
    let fusion_power_mw = design
        .plasma
        .fusion_power_mw
        .ok_or_else(|| tool("the design's plasma block has no fusion power"))?;
    let chamber = design
        .plasma
        .chamber_solid_id
        .as_deref()
        .ok_or_else(|| tool("the design's plasma block names no chamber solid"))?;
    let history = linked_history(design_path, &design, fusion_power_mw);

    let dir = &options.out_dir;
    std::fs::create_dir(dir).map_err(|e| {
        io_error(
            dir,
            format!("{e} (the run directory must be new; nothing is overwritten)"),
        )
    })?;
    let dir = dir.canonicalize().map_err(|e| io_error(dir, e))?;
    let cross_sections = cross_sections
        .canonicalize()
        .map_err(|e| io_error(cross_sections, e))?;
    let cross_sections_sha256 = sha256_file(&cross_sections)?;
    let request = serde_json::json!({
        "schema": REQUEST_SCHEMA,
        "h5m": h5m,
        "cross_sections": cross_sections,
        "solids": solid_entries(&design, &report)?,
        "materials": material_entries(&design, &report)?,
        "plasma": design.plasma,
        "chamber_solid_id": chamber,
        "variance_reduction": if options.window_python.is_some() { "windows" } else { "none" },
        "response": {
            "solid_ids": response.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            "energy_min_ev": chosen.energy_min_ev,
            "peak_voxel_cm": chosen.peak_voxel_m.map(|v| v.map(|d| d.map(|m| m * 100.0))),
        },
        "sampling": {
            "target_relative_error": options.target_relative_error,
            "particles_per_batch": options.particles_per_batch,
            "min_batches": MIN_BATCHES,
            "max_batches": options.max_histories / options.particles_per_batch,
            "max_seconds": options.max_seconds,
            "seed": options.seed,
            "threads": options.threads,
        },
    });
    let script = dir.join("design_transport.py");
    let helpers = dir.join("risk_tests");
    std::fs::create_dir(&helpers).map_err(|e| io_error(&helpers, e))?;
    for (path, text) in [
        (script.clone(), DESIGN_TRANSPORT_WORKER),
        (dir.join("streaming_aim.py"), STREAMING_AIM_HELPER),
        (helpers.join("s0f_common.py"), AIM_COMMON_HELPER),
        (helpers.join("s0f_find_targets.py"), AIM_TARGETS_HELPER),
        (helpers.join("s0f_los_windows.py"), AIM_WINDOWS_HELPER),
    ] {
        std::fs::write(&path, text).map_err(|e| io_error(&path, e))?;
    }
    let request_path = dir.join("request.json");
    std::fs::write(
        &request_path,
        serde_json::to_vec_pretty(&request).unwrap_or_default(),
    )
    .map_err(|e| io_error(&request_path, e))?;
    let solver = dir.join("solver");
    let tmp = dir.join("tmp");
    std::fs::create_dir_all(&tmp).map_err(|e| io_error(&tmp, e))?;
    let mut python_path = plasma_source.as_os_str().to_owned();
    if let Some(package) = &options.openmc_package_path {
        python_path.push(":");
        python_path.push(package);
    }
    let phase = |name: &str, interpreter: &Path, with_packages: bool, timeout: Duration| {
        let arguments: Vec<OsString> = vec![
            script.clone().into_os_string(),
            "--phase".into(),
            name.into(),
            "--request".into(),
            request_path.clone().into_os_string(),
            "--work-dir".into(),
            dir.clone().into_os_string(),
        ];
        let mut environment: Vec<(OsString, OsString)> = vec![
            ("TMPDIR".into(), tmp.clone().into_os_string()),
            // OpenMC initialises MPI, which refuses to start without a home directory.
            ("HOME".into(), dir.as_os_str().to_owned()),
            ("OMP_NUM_THREADS".into(), options.threads.to_string().into()),
        ];
        if with_packages {
            environment.push(("PYTHONPATH".into(), python_path.clone()));
        }
        if let Some(progress) = &options.progress {
            progress(TransportProgress::Step(name.to_string()));
        }
        match launch_python_with_lines(
            interpreter,
            arguments,
            &dir,
            timeout,
            &format!("the transport {name} step"),
            &options.cancellation,
            environment,
            ResourceLimits::with_address_space(CAD_ADDRESS_SPACE),
            &|line| {
                if let (Some(progress), Some(update)) = (&options.progress, parse_progress(line)) {
                    progress(update);
                }
            },
        )? {
            Launched::Succeeded(seconds) => Ok(seconds),
            Launched::Failed(text) => Err(tool(text)),
        }
    };
    let entry = options.cache_dir.as_ref().map(|cache| {
        cache.join(cache_key(
            &request,
            &conversion.h5m_sha256,
            &cross_sections_sha256,
            options.window_python.as_deref(),
        ))
    });
    let mut setup_seconds = 0.0;
    let mut windows_reused = false;
    if let Some(window_python) = &options.window_python {
        windows_reused = match &entry {
            Some(entry) => restore_windows(entry, &dir)?,
            None => false,
        };
        if !windows_reused {
            setup_seconds += phase("mgxs", python, true, MGXS_TIMEOUT)?;
            setup_seconds += phase("windows", window_python, false, WINDOWS_TIMEOUT)?;
        }
    }
    if chosen.peak_voxel_m.is_some() {
        let reused = match &entry {
            Some(entry) => restore_pieces(entry, &dir)?,
            None => false,
        };
        if !reused {
            setup_seconds += phase("volumes", python, true, VOLUMES_TIMEOUT)?;
            if let Some(entry) = &entry {
                for name in PIECE_FILES {
                    copy_file(&dir.join("peak").join(name), &entry.join("peak").join(name))?;
                }
            }
        }
    }
    phase(
        "run",
        python,
        true,
        Duration::from_secs(options.max_seconds) + STARTUP_ALLOWANCE,
    )?;
    if let Some(entry) = &entry
        && options.window_python.is_some()
    {
        keep_for_later(&dir, entry, windows_reused)?;
    }
    let result_path = solver.join("result.json");
    let worker: WorkerResult = serde_json::from_slice(&read_bounded(&result_path)?)
        .map_err(|e| tool(format!("{}: {e}", result_path.display())))?;
    assemble(
        worker,
        Inputs {
            design_identity: DesignIdentity {
                path: design_path.display().to_string(),
                sha256: design_sha256,
                id: Some(design.id.clone()),
            },
            check_report_sha256: sha256_hex(&report_bytes),
            h5m_sha256: conversion.h5m_sha256.clone(),
            cross_sections_sha256,
            audit_cross_sections_sha256: report
                .material_audit
                .as_ref()
                .and_then(|a| a.cross_sections_sha256.clone()),
            run_directory: dir.display().to_string(),
            fusion_power_mw,
            response: &response,
            design: &design,
            history,
            setup_seconds,
            windows_reused,
            chosen: &chosen,
        },
    )
}

struct Inputs<'a> {
    design_identity: DesignIdentity,
    check_report_sha256: String,
    h5m_sha256: String,
    cross_sections_sha256: String,
    audit_cross_sections_sha256: Option<String>,
    run_directory: String,
    fusion_power_mw: f64,
    response: &'a [(&'a str, f64)],
    design: &'a Design,
    history: Result<PowerHistory, String>,
    setup_seconds: f64,
    windows_reused: bool,
    chosen: &'a ChosenResponse,
}

/// Check the worker's answer against the request and convert it to fluxes.
fn assemble(worker: WorkerResult, inputs: Inputs) -> Result<DesignTransportReport, DesignError> {
    if worker.schema != RESULT_SCHEMA || worker.unit != "cm_per_source" {
        return Err(tool(
            "the transport worker's result has an unknown schema or unit",
        ));
    }
    if !worker.h5m_sha256.eq_ignore_ascii_case(&inputs.h5m_sha256) {
        return Err(tool(
            "the transport worker ran a different geometry than the checked one",
        ));
    }
    if worker.histories != worker.batches * worker.particles_per_batch
        || worker.solids.len() != inputs.response.len()
        || worker
            .solids
            .iter()
            .zip(inputs.response)
            .any(|(w, (id, _))| w.solid_id != *id)
    {
        return Err(tool(
            "the transport worker's result does not match its request",
        ));
    }
    let finite = |v: f64| v.is_finite() && v >= 0.0;
    if !finite(worker.total.mean)
        || worker.solids.iter().any(|s| !finite(s.mean))
        || worker
            .solids
            .iter()
            .filter_map(|s| s.peak.as_ref())
            .any(|p| !finite(p.mean) || !(p.volume_cm3.is_finite() && p.volume_cm3 > 0.0))
    {
        return Err(tool(
            "the transport worker returned a value that is not a finite score",
        ));
    }
    let rate = source_rate_per_s(inputs.fusion_power_mw);
    let relative = |mean: f64, se: Option<f64>| se.filter(|_| mean > 0.0).map(|se| se / mean);
    let total_volume_m3: f64 = inputs.response.iter().map(|(_, v)| v).sum();
    let solids: Vec<SolidFlux> = worker
        .solids
        .iter()
        .zip(inputs.response)
        .map(|(w, (id, volume))| SolidFlux {
            solid_id: (*id).to_string(),
            volume_m3: *volume,
            flux_n_m2_s: flux_n_m2_s(w.mean, rate, *volume),
            relative_error: relative(w.mean, w.standard_error),
            peak: w.peak.as_ref().map(|p| PeakFlux {
                r_m: p.r_cm.map(|v| v / 100.0),
                z_m: p.z_cm.map(|v| v / 100.0),
                volume_m3: p.volume_cm3 / 1.0e6,
                flux_n_m2_s: flux_n_m2_s(p.mean, rate, p.volume_cm3 / 1.0e6),
                relative_error: relative(p.mean, p.standard_error),
                pieces_ranked: p.pieces_ranked,
            }),
        })
        .collect();
    let mut notices =
        vec!["Volumes are the CAD volumes of the design file's fingerprints.".to_string()];
    if let Some(audited) = &inputs.audit_cross_sections_sha256
        && !audited.eq_ignore_ascii_case(&inputs.cross_sections_sha256)
    {
        notices.push(
            "The nuclear data of this run is not the library the design check audited.".into(),
        );
    }
    let mean_flux = flux_n_m2_s(worker.total.mean, rate, total_volume_m3);
    let mean_relative_error = relative(worker.total.mean, worker.total.standard_error);
    if inputs.chosen.peak_voxel_m.is_some() {
        notices.push(
            "The highest piece is the highest the run has resolved (its flux less two standard \
             errors); a piece with few scores could be higher."
                .into(),
        );
    }
    if inputs.chosen.peak_ignores_dphi {
        notices.push(
            "The limit's toroidal voxel width is not applied: each piece spans the solid's full \
             toroidal width."
                .into(),
        );
    }
    let mut fluence_limits = Vec::new();
    let mut not_evaluated = Vec::new();
    if inputs.windows_reused {
        notices.push("The weight windows of an earlier run on this model were used.".into());
    }
    if worker.variance_reduction.aim_reused {
        notices.push("The channel search of an earlier run on this model was used.".into());
    }
    for group in &inputs.design.replacement_groups {
        for limit in &group.limits {
            let item = format!(
                "{}: {} against the limit of {:e}",
                group.id,
                limit.metric.label(),
                limit.limit
            );
            let scored = inputs.chosen.group_id.as_deref() == Some(group.id.as_str())
                && fluence_threshold(limit) == Some(worker.energy_min_ev);
            if !scored {
                let (why, next_step) = if fluence_threshold(limit).is_some() {
                    (
                        "This run scored another group or another energy threshold.",
                        format!("Run again with --group {}.", group.id),
                    )
                } else {
                    (
                        "This command computes neutron fluence limits only.",
                        "Not available from this command yet.".to_string(),
                    )
                };
                not_evaluated.push(NotEvaluated {
                    item,
                    why: why.into(),
                    next_step,
                });
                continue;
            }
            // (basis, solid, flux, relative error) of the value the limit's averaging names.
            let judged = if inputs.chosen.peak_voxel_m.is_some() {
                solids
                    .iter()
                    .filter_map(|s| Some((s, s.peak.as_ref()?)))
                    .max_by(|a, b| a.1.flux_n_m2_s.total_cmp(&b.1.flux_n_m2_s))
                    .map(|(s, p)| {
                        (
                            "peak_piece",
                            Some(s.solid_id.clone()),
                            p.flux_n_m2_s,
                            p.relative_error,
                        )
                    })
            } else {
                Some(("group_average", None, mean_flux, mean_relative_error))
            };
            match (judged, &inputs.history) {
                (Some((basis, solid_id, flux, error)), Ok(history)) => {
                    fluence_limits.push(FluenceLimit {
                        group_id: group.id.clone(),
                        basis: basis.into(),
                        solid_id,
                        assessment: assess_fluence(flux, error, limit.limit, history),
                    })
                }
                (None, _) => not_evaluated.push(NotEvaluated {
                    item,
                    why: "No piece of any solid of the group could be ranked.".into(),
                    next_step: "Use a larger voxel in the limit's averaging.".into(),
                }),
                (_, Err(problem)) => not_evaluated.push(NotEvaluated {
                    item,
                    why: problem.clone(),
                    next_step: "Link a valid operating history in the design file and run again."
                        .into(),
                }),
            }
        }
    }
    Ok(DesignTransportReport {
        schema_version: DESIGN_TRANSPORT_VERSION.into(),
        design: inputs.design_identity,
        check_report_sha256: inputs.check_report_sha256,
        h5m_sha256: inputs.h5m_sha256,
        cross_sections_sha256: inputs.cross_sections_sha256,
        openmc: worker.openmc,
        run_directory: inputs.run_directory,
        energy_min_ev: worker.energy_min_ev,
        response_group_id: inputs.chosen.group_id.clone(),
        source_rate_per_s: rate,
        energy_per_reaction_ev: D_T_ENERGY_PER_REACTION_EV,
        fusion_power_mw: inputs.fusion_power_mw,
        histories: worker.histories,
        elapsed_seconds: worker.elapsed_seconds,
        seed: worker.seed,
        threads: worker.threads,
        target_relative_error: worker.target_relative_error,
        target_reached: worker.target_reached,
        stop_reason: worker.stop_reason,
        why: worker.why,
        next_step: worker.next_step,
        mean_flux_n_m2_s: mean_flux,
        mean_relative_error,
        total_volume_m3,
        solids,
        setup_seconds: inputs.setup_seconds + worker.variance_reduction.aim_seconds,
        windows_reused: inputs.windows_reused,
        variance_reduction: worker.variance_reduction,
        fluence_limits,
        notices,
        not_evaluated,
    })
}

fn percent(relative: Option<f64>) -> String {
    relative.map_or_else(
        || "error unknown".into(),
        |r| format!("± {:.1} %", 100.0 * r),
    )
}

/// The fluence's 2-sigma band; the verdict never says WITHIN unless the upper edge is inside.
fn fluence_band(a: &FluenceAssessment) -> String {
    match (a.history_fluence_low_n_m2, a.history_fluence_high_n_m2) {
        (Some(low), Some(high)) => format!(
            "(2-sigma band {low:.3e} to {high:.3e}; WITHIN needs the upper edge below the limit, {})",
            percent(a.flux_relative_error)
        ),
        _ => "(no error band: the flux has no error estimate)".into(),
    }
}

/// The years to the limit with their 2-sigma band; the earliest edge is the one the verdict uses.
fn years_band(earliest: Option<f64>, latest: Option<f64>) -> String {
    match (earliest, latest) {
        (Some(e), Some(l)) => format!("(2-sigma band {e:.1} to {l:.1} years; earliest edge used)"),
        (Some(e), None) => {
            format!("(2-sigma band from {e:.1} years, no upper edge; earliest edge used)")
        }
        _ => "(no error band: the flux has no error estimate)".into(),
    }
}

/// The report as text for the terminal.
pub fn render_transport_text(report: &DesignTransportReport) -> String {
    let mut out = String::new();
    let what = if report.energy_min_ev > 0.0 {
        format!("Neutron flux above {} MeV", report.energy_min_ev / 1.0e6)
    } else {
        "Neutron flux at all energies".to_string()
    };
    let place = match &report.response_group_id {
        Some(group) => format!("the group {group}"),
        None => "the TF magnets".to_string(),
    };
    let _ = writeln!(
        out,
        "{what} in {place} of {}",
        report.design.id.as_deref().unwrap_or("the design")
    );
    let _ = writeln!(
        out,
        "  Mean over {} solids: {:.3e} n/m²/s {}",
        report.solids.len(),
        report.mean_flux_n_m2_s,
        percent(report.mean_relative_error)
    );
    if let Some(highest) = report
        .solids
        .iter()
        .max_by(|a, b| a.flux_n_m2_s.total_cmp(&b.flux_n_m2_s))
    {
        let _ = writeln!(
            out,
            "  Highest solid: {} at {:.3e} n/m²/s {}",
            highest.solid_id,
            highest.flux_n_m2_s,
            percent(highest.relative_error)
        );
    }
    if let Some((solid, peak)) = report
        .solids
        .iter()
        .filter_map(|s| Some((s, s.peak.as_ref()?)))
        .max_by(|a, b| a.1.flux_n_m2_s.total_cmp(&b.1.flux_n_m2_s))
    {
        let _ = writeln!(
            out,
            "  Highest piece: {:.3e} n/m²/s {} in {} at r {:.2}–{:.2} m, z {:.2}–{:.2} m ({:.1} litres)",
            peak.flux_n_m2_s,
            percent(peak.relative_error),
            solid.solid_id,
            peak.r_m[0],
            peak.r_m[1],
            peak.z_m[0],
            peak.z_m[1],
            peak.volume_m3 * 1.0e3
        );
    }
    let target = 100.0 * report.target_relative_error;
    let on = if report.solids.iter().any(|s| s.peak.is_some()) {
        format!("the mean and {} % on the highest piece", 2.0 * target)
    } else {
        "the mean".to_string()
    };
    if report.target_reached {
        let _ = writeln!(out, "  Target of {target} % on {on}: reached.");
    } else {
        let _ = writeln!(out, "  Target of {target} % on {on}: NOT reached.");
        if let (Some(why), Some(next)) = (&report.why, &report.next_step) {
            let _ = writeln!(out, "    Why: {why}");
            let _ = writeln!(out, "    Next: {next}");
        }
    }
    let _ = writeln!(
        out,
        "  {} histories in {:.0} s on {} thread(s); source {:.3e} n/s from {} MW of fusion power at {} MeV per reaction.",
        report.histories,
        report.elapsed_seconds,
        report.threads,
        report.source_rate_per_s,
        report.fusion_power_mw,
        report.energy_per_reaction_ev / 1.0e6
    );
    let reduction = &report.variance_reduction;
    let _ = writeln!(
        out,
        "  Speed-up: {} ({:.0} s of preparation). {}",
        reduction.mode, report.setup_seconds, reduction.reason
    );
    if reduction.mode != "aimed" {
        let _ = writeln!(out, "    Next: {}", reduction.next_step);
    }
    for limit in &report.fluence_limits {
        let a = &limit.assessment;
        let judged = match &limit.solid_id {
            Some(solid) => format!("highest piece, in {solid}"),
            None => "mean over the group".to_string(),
        };
        let _ = writeln!(
            out,
            "  Fluence limit of {} ({:e} n/m²), {judged}: {}",
            limit.group_id,
            a.limit_n_m2,
            match a.status {
                LimitStatus::Within => "WITHIN",
                LimitStatus::Exceeded => "EXCEEDED",
                LimitStatus::Inconclusive => "INCONCLUSIVE",
            }
        );
        let _ = writeln!(
            out,
            "    Over the {:.1}-year history ({:.1} years at full power): {:.3e} n/m² {}",
            a.history_calendar_years,
            a.history_full_power_years,
            a.history_fluence_n_m2,
            fluence_band(a)
        );
        if let (Some(full), Some(calendar)) =
            (a.full_power_years_to_limit, a.calendar_years_to_limit)
        {
            let _ = writeln!(
                out,
                "    Reaches the limit after {full:.1} years at full power ({calendar:.1} years of this history) {}",
                years_band(
                    a.full_power_years_to_limit_earliest,
                    a.full_power_years_to_limit_latest
                )
            );
        }
        let _ = writeln!(out, "    Why: {}", a.why);
        let _ = writeln!(out, "    Next: {}", a.next_step);
    }
    for notice in &report.notices {
        let _ = writeln!(out, "  Note: {notice}");
    }
    for item in &report.not_evaluated {
        let _ = writeln!(out, "  Not evaluated: {}", item.item);
        let _ = writeln!(out, "    Why: {}", item.why);
        let _ = writeln!(out, "    Next: {}", item.next_step);
    }
    let _ = writeln!(out, "  Run directory: {}", report.run_directory);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flux_conversion_has_the_right_units() {
        // 1 cm of track per source neutron in 1 m³ at 1e6 n/s: 1e6 cm/s in 1e6 cm³ = 1 /cm²/s.
        assert!((flux_n_m2_s(1.0, 1.0e6, 1.0) - 1.0e4).abs() < 1.0e-9);
    }

    fn design_with_groups(limits: &[(&str, &str, Option<f64>)]) -> Design {
        let groups: Vec<serde_json::Value> = limits
            .iter()
            .map(|(id, metric, threshold)| {
                serde_json::json!({
                    "id": id, "label": id,
                    "limits": [{
                        "metric": metric, "energy_threshold_ev": threshold, "limit": 3.0e22,
                        "averaging": {"kind": "group_average"},
                        "provenance": {"label": "authored", "note": "test"},
                    }],
                    "replacement_duration": {"kind": "computed"},
                })
            })
            .collect();
        let text = include_str!("../../../../scenarios/rm-m/rm-m.faris-design.json");
        let mut value: serde_json::Value = serde_json::from_str(text).unwrap();
        value["replacement_groups"] = serde_json::Value::Array(groups);
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn other_sources_are_refused_not_replaced() {
        let mut design = design_with_groups(&[("coils", "fast_neutron_fluence", Some(1.0e5))]);
        assert!(refuse_unsupported_source(&design).is_ok());
        design.plasma.fuel = Some([("D".to_string(), 1.0)].into_iter().collect());
        assert!(
            refuse_unsupported_source(&design)
                .unwrap_err()
                .to_string()
                .contains("D-T")
        );
        design.plasma.fuel = Some(
            [("D".to_string(), 0.5), ("T".to_string(), 0.5)]
                .into_iter()
                .collect(),
        );
        design.plasma.emissivity_table = Some(design::EmissivityTable {
            path: "table.csv".into(),
            sha256: "0".repeat(64),
        });
        assert!(
            refuse_unsupported_source(&design)
                .unwrap_err()
                .to_string()
                .contains("emissivity")
        );
    }

    #[test]
    fn the_only_fluence_limit_names_the_response() {
        let design = design_with_groups(&[("coils", "fast_neutron_fluence", Some(1.0e5))]);
        let chosen = choose_response(&design, None).unwrap();
        assert_eq!(chosen.group_id.as_deref(), Some("coils"));
        assert_eq!(chosen.energy_min_ev, 1.0e5);
    }

    #[test]
    fn a_peak_limit_asks_for_pieces_of_its_voxel() {
        let mut design = design_with_groups(&[("coils", "fast_neutron_fluence", Some(1.0e5))]);
        assert_eq!(choose_response(&design, None).unwrap().peak_voxel_m, None);
        design.replacement_groups[0].limits[0].averaging = design::Averaging::PeakMesh {
            voxel_m: vec![Some(0.05), Some(0.10), None],
        };
        let chosen = choose_response(&design, None).unwrap();
        assert_eq!(chosen.peak_voxel_m, Some([Some(0.05), Some(0.10)]));
        assert!(!chosen.peak_ignores_dphi);
        design.replacement_groups[0].limits[0].averaging = design::Averaging::PeakMesh {
            voxel_m: vec![Some(0.05), None, Some(0.2)],
        };
        let chosen = choose_response(&design, None).unwrap();
        assert_eq!(chosen.peak_voxel_m, Some([Some(0.05), None]));
        assert!(chosen.peak_ignores_dphi);
    }

    #[test]
    fn a_total_fluence_limit_scores_all_energies() {
        let design = design_with_groups(&[("vessel", "neutron_fluence", None)]);
        assert_eq!(choose_response(&design, None).unwrap().energy_min_ev, 0.0);
    }

    #[test]
    fn several_limits_need_a_named_group() {
        let design = design_with_groups(&[
            ("coils", "fast_neutron_fluence", Some(1.0e5)),
            ("vessel", "neutron_fluence", None),
        ]);
        let error = choose_response(&design, None).unwrap_err().to_string();
        assert!(
            error.contains("--group") && error.contains("coils, vessel"),
            "{error}"
        );
        let chosen = choose_response(&design, Some("vessel")).unwrap();
        assert_eq!(chosen.group_id.as_deref(), Some("vessel"));
        assert!(choose_response(&design, Some("missing")).is_err());
    }

    #[test]
    fn a_design_without_fluence_limits_falls_back_to_the_tf_magnets() {
        let design = design_with_groups(&[("coils", "helium_appm", None)]);
        let chosen = choose_response(&design, None).unwrap();
        assert_eq!(chosen.group_id, None);
        assert_eq!(chosen.energy_min_ev, FAST_NEUTRON_ENERGY_MIN_EV);
    }

    #[test]
    fn the_cache_key_follows_the_model_and_the_response() {
        let request = serde_json::json!({"solids": [1], "materials": [2], "chamber_solid_id": "p",
            "response": {"solid_ids": ["a"], "energy_min_ev": 1.0e5}, "sampling": {"seed": 1}});
        let mut other_seed = request.clone();
        other_seed["sampling"]["seed"] = 2.into();
        let mut other_response = request.clone();
        other_response["response"]["energy_min_ev"] = 0.0.into();
        let key = |r: &serde_json::Value, h: &str| cache_key(r, h, "x", None);
        assert_eq!(key(&request, "h"), key(&other_seed, "h"));
        assert_ne!(key(&request, "h"), key(&other_response, "h"));
        assert_ne!(key(&request, "h"), key(&request, "h2"));
    }

    #[test]
    fn progress_lines_are_recognised_and_other_output_is_ignored() {
        let line = r#"{"progress": {"batches": 12, "histories": 240000, "relative_error": 0.21, "elapsed_seconds": 31.2}}"#;
        assert_eq!(
            parse_progress(line),
            Some(TransportProgress::Sampling {
                histories: 240_000,
                relative_error: Some(0.21),
                peak_relative_error: None,
                elapsed_seconds: 31.2
            })
        );
        let first = r#"{"progress": {"batches": 1, "histories": 20000, "relative_error": null, "elapsed_seconds": 6.8}}"#;
        assert!(matches!(
            parse_progress(first),
            Some(TransportProgress::Sampling {
                relative_error: None,
                ..
            })
        ));
        assert_eq!(parse_progress(" Simulating batch 3"), None);
        assert_eq!(parse_progress("{\"progress\": broken"), None);
    }

    #[test]
    fn source_rate_follows_the_fusion_power() {
        // 525 MW at 17.6 MeV per reaction.
        let rate = source_rate_per_s(525.0);
        assert!((rate / 1.8618e20 - 1.0).abs() < 1.0e-3, "{rate:e}");
    }

    // Verifies: MAG-004
    #[test]
    fn a_fluence_margin_is_shown_with_its_band_and_judged_on_the_unfavourable_edge() {
        let history = PowerHistory {
            calendar_seconds: 1.0,
            full_power_seconds: 1.0,
        };
        // Mean fluence 8 against a limit of 10 looks WITHIN, but a 25 % error puts the upper
        // edge (12) over the limit.
        let a = assess_fluence(8.0, Some(0.25), 10.0, &history);
        assert_eq!(a.status, LimitStatus::Inconclusive);
        let text = format!(
            "{} | {}",
            fluence_band(&a),
            years_band(
                a.full_power_years_to_limit_earliest,
                a.full_power_years_to_limit_latest
            )
        );
        assert!(text.contains("2-sigma band 4.000e0 to 1.200e1"), "{text}");
        assert!(text.contains("earliest edge used"), "{text}");
        // With no error estimate the display says so rather than showing a bare margin.
        let bare = assess_fluence(8.0, None, 10.0, &history);
        assert!(fluence_band(&bare).contains("no error band"));
        assert!(years_band(None, None).contains("no error band"));
    }

    // Verifies: SRC-001
    #[test]
    fn design_source_rate_matches_the_reactor_path_per_megawatt() {
        assert_eq!(format!("{:.3e}", source_rate_per_s(1.0)), "3.546e17");
        assert_eq!(
            source_rate_per_s(525.0),
            crate::transport::reaction_rate_per_s(525.0, 17.6e6)
        );
    }
}
