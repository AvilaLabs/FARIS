//! Checks 6 to 9 of `faris design check`: conversion, binding, integrity and
//! source sites, run in that order. A failing stage stops the later ones.
//!
//! The external work (conversion, per-solid tessellation, OCCT booleans,
//! faceting deviation, source sampling) sits behind [`Backends`], so the
//! exact-arithmetic checks here run the same on recorded or generated models.

use super::binding::{self, BindingRow, VolumeGate};
use super::bvh::Bvh;
use super::exact::P3;
use super::integrity::{self, IntegrityRecord};
use super::mesh::Mesh;
use super::model::Model;
use super::overlap::{self, BooleanResult, PairRecord};
use super::par::{Cancelled, Ctl};
use super::rays::{Caster, Rng};
use super::source::{self, SourceClosure};
use super::{Backends, CheckOutcome, CheckStatus, DesignError, ImportReport, SubCheck, check_name};
use crate::jobs::Cancellation;
use faris_model::design::{
    Design, Facets, FacetsManifest, Finding, Notice, Plasma, Role, StepInspection, Waiver,
    apply_waivers, void_solids,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

/// Faceting tolerances when the design file gives none: 0.1 cm and 0.1 rad.
pub const DEFAULT_CHORD_M: f64 = 1.0e-3;
pub const DEFAULT_ANGULAR_RAD: f64 = 0.1;
/// OpenCASCADE projections per solid in the faceting deviation (GEO-032).
pub const DEFAULT_DEVIATION_POINTS: usize = 2000;
/// Threads the converter imprints with.
pub const DEFAULT_IMPRINT_THREADS: u32 = 4;

// ---------------------------------------------------------------------------
// What the backends take and give.

/// What a backend did when asked to run.
#[derive(Debug)]
pub enum ToolResult<T> {
    Done(T),
    /// The tool is not configured here; the check is not evaluated.
    Unavailable {
        why: String,
        next_step: String,
    },
    /// The tool ran and failed; the failure is a finding.
    Failed {
        why: String,
        next_step: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct TagEntry {
    pub step_index: usize,
    pub tag: String,
}

pub struct ConvertRequest<'a> {
    pub step: &'a Path,
    pub step_sha256: &'a str,
    /// Metres per unit of the shape OpenCASCADE holds.
    pub shape_unit_m: f64,
    pub tags: &'a [TagEntry],
    pub chord_cm: f64,
    pub angular_rad: f64,
    pub imprint_threads: u32,
    pub directory: &'a Path,
    /// Reuse a recorded conversion whose hashes and tolerances match.
    pub reuse: bool,
    /// Where the STEP solids are kept as BREP between helper commands; `None` reads the STEP file.
    pub shape_cache: Option<&'a Path>,
}

/// The CAD helper's `faris-conversion/v1` report.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ConversionReport {
    pub schema: String,
    pub step: ConversionStep,
    pub tolerances: ConversionTolerances,
    pub tags: Vec<TagEntry>,
    pub versions: serde_json::Value,
    pub timing_seconds: BTreeMap<String, f64>,
    pub peak_rss_mb: f64,
    pub h5m: ConversionFile,
    pub facets: ConversionFacets,
    pub counts: serde_json::Value,
    #[serde(default)]
    pub log_tail: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ConversionStep {
    pub file_name: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ConversionTolerances {
    pub chord_cm: f64,
    pub angular_rad: f64,
    pub scale_factor_to_cm: f64,
    pub imprint_threads: u32,
    pub implicit_complement_tag: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ConversionFile {
    pub file: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ConversionFacets {
    pub bin: String,
    pub json: String,
    pub bin_sha256: String,
}

pub struct ConvertOutcome {
    pub report: ConversionReport,
    pub facets: Facets,
    pub manifest: FacetsManifest,
    /// The `.facets.bin` of the imprinted model, for the deviation helper.
    pub facets_bin: PathBuf,
    pub reused: bool,
    pub helper_sha256: String,
}

pub struct TessellateRequest<'a> {
    pub step: &'a Path,
    pub step_sha256: &'a str,
    pub shape_unit_m: f64,
    pub tags: &'a [TagEntry],
    pub chord_cm: f64,
    pub angular_rad: f64,
    pub directory: &'a Path,
    pub workers: usize,
    /// Reuse a tessellation recorded under the same STEP hash, tolerances and helper version.
    pub reuse: bool,
    pub shape_cache: Option<&'a Path>,
}

pub struct TessellateOutcome {
    pub facets: Facets,
    pub manifest: FacetsManifest,
    /// Solids whose mesh is not closed or not oriented outward, and similar.
    pub problems: Vec<String>,
    pub seconds: f64,
    /// True when a recorded tessellation was reused; `seconds` is then the recorded time.
    pub reused: bool,
    /// The reader that made the tessellation, or whose identity the recorded one matched.
    pub reader: super::ReaderStamp,
}

pub struct BooleanRequest<'a> {
    pub step: &'a Path,
    pub step_sha256: &'a str,
    pub shape_unit_m: f64,
    /// Pairs of STEP solid indices.
    pub pairs: &'a [(usize, usize)],
    /// The tolerances the pairs were found with; they are part of the cache key.
    pub chord_cm: f64,
    pub angular_rad: f64,
    pub directory: &'a Path,
    pub workers: usize,
    pub reuse: bool,
    pub shape_cache: Option<&'a Path>,
}

pub struct BooleanOutcome {
    pub results: Vec<BooleanResult>,
    pub seconds: f64,
    pub reused: bool,
}

pub struct DeviationRequest<'a> {
    pub step: &'a Path,
    pub shape_unit_m: f64,
    pub facets_bin: &'a Path,
    /// `(volume, step_index)`.
    pub map: &'a [(u32, usize)],
    pub directory: &'a Path,
    pub workers: usize,
    /// Points per solid measured by OpenCASCADE projection, on an even stride; 0 for all.
    pub occt_points: usize,
    pub shape_cache: Option<&'a Path>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DeviationRow {
    pub volume: u32,
    pub step_index: usize,
    /// Points measured.
    pub points: u64,
    /// Points the solid's triangles give; more than `points` when the run took a stride.
    #[serde(default)]
    pub points_available: u64,
    /// Points measured in closed form on plane, cylinder, cone, sphere and torus faces.
    #[serde(default)]
    pub points_closed_form: u64,
    /// Points measured by OpenCASCADE projection.
    #[serde(default)]
    pub points_occt: u64,
    /// Points that needed OpenCASCADE (non-analytic faces, or a foot point outside the face).
    #[serde(default)]
    pub points_occt_needed: u64,
    pub faces: u64,
    pub max_distance_m: f64,
    #[serde(default)]
    pub at_m: Option<[f64; 3]>,
}

pub struct DeviationOutcome {
    pub rows: Vec<DeviationRow>,
    pub seconds: f64,
    pub method: String,
}

pub struct SourceRequest<'a> {
    pub plasma: &'a Plasma,
    pub count: usize,
    pub seed: u64,
    pub directory: &'a Path,
}

pub struct SourceOutcome {
    /// Sites in metres.
    pub points: Vec<[f64; 3]>,
    pub versions: serde_json::Value,
    pub bin_sha256: String,
    pub seconds: f64,
}

// ---------------------------------------------------------------------------
// Options.

pub type ProgressFn = Arc<dyn Fn(&str, usize, usize) + Send + Sync>;

/// How checks 6 to 9 run. Defaults give the numbers the format document names.
#[derive(Clone)]
pub struct GeometryOptions {
    /// Where conversion products are kept. `None`: a temporary directory, removed afterwards.
    pub work_dir: Option<PathBuf>,
    /// Reuse a conversion in `work_dir` whose recorded hashes and tolerances match.
    pub reuse: bool,
    pub threads: usize,
    pub seed: u64,
    pub samples: usize,
    pub chords: usize,
    pub source_sites: usize,
    /// Points per solid the faceting deviation lets OpenCASCADE project (those not on a plane,
    /// cylinder, cone, sphere or torus face, or whose foot point falls outside the face); 0 projects
    /// all of them. A positive number takes an even stride of them, which is recorded.
    pub deviation_points: usize,
    pub imprint_threads: u32,
    pub cancellation: Cancellation,
    pub progress: Option<ProgressFn>,
}

impl Default for GeometryOptions {
    fn default() -> Self {
        Self {
            work_dir: None,
            reuse: true,
            threads: super::par::default_threads(),
            seed: 1,
            samples: 1_000_000,
            chords: 100_000,
            source_sites: 100_000,
            deviation_points: DEFAULT_DEVIATION_POINTS,
            imprint_threads: DEFAULT_IMPRINT_THREADS,
            cancellation: Cancellation::default(),
            progress: None,
        }
    }
}

// ---------------------------------------------------------------------------
// What the report records.

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ConvertedVolume {
    pub volume: u32,
    pub global_id: i64,
    pub material_tag: Option<String>,
    pub triangles: u64,
    pub vertices: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ConversionRecord {
    /// True when a conversion recorded earlier was reused; the times are the recorded ones.
    pub reused: bool,
    pub directory: String,
    pub step_sha256: String,
    pub shape_unit_m: f64,
    pub chord_cm: f64,
    pub angular_rad: f64,
    pub scale_factor_to_cm: f64,
    pub imprint_threads: u32,
    pub implicit_complement_tag: String,
    pub versions: serde_json::Value,
    pub timing_seconds: BTreeMap<String, f64>,
    pub peak_rss_mb: f64,
    pub h5m_sha256: String,
    pub h5m_bytes: u64,
    pub facets_sha256: String,
    pub counts: serde_json::Value,
    pub helper_sha256: String,
    pub volumes: Vec<ConvertedVolume>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PreImprintRecord {
    pub tessellation_seconds: f64,
    /// True when a recorded tessellation or boolean result was reused; its time is the recorded one.
    #[serde(default)]
    pub tessellation_reused: bool,
    /// Which reader made the per-solid tessellation.
    #[serde(default = "super::ReaderStamp::python")]
    pub tessellation_reader: super::ReaderStamp,
    pub candidate_pairs: usize,
    pub boolean_seconds: Option<f64>,
    #[serde(default)]
    pub booleans_reused: bool,
    pub pairs: Vec<PairRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DeviationSolid {
    pub solid_id: String,
    pub max_distance_m: f64,
    pub at_m: Option<[f64; 3]>,
    pub points: u64,
    #[serde(default)]
    pub points_available: u64,
    #[serde(default)]
    pub points_closed_form: u64,
    #[serde(default)]
    pub points_occt: u64,
    #[serde(default)]
    pub points_occt_needed: u64,
    /// Every sample point was measured: the maximum is over all points, not a sampled maximum.
    #[serde(default)]
    pub complete: bool,
    pub above_tolerance: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DeviationRecord {
    pub method: String,
    pub chord_tolerance_m: f64,
    pub seconds: f64,
    /// What the maximum covers: all sample points, or a sample of them, per solid.
    pub statement: String,
    pub solids: Vec<DeviationSolid>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SourceRecord {
    pub sites: u64,
    pub seed: u64,
    pub plasma_chamber_solid: String,
    pub inside: u64,
    pub outside: u64,
    pub undecided: u64,
    /// Where the sites outside the chamber are instead, by solid.
    pub outside_in: BTreeMap<String, u64>,
    pub sites_sha256: String,
    pub versions: serde_json::Value,
    pub sampling_seconds: f64,
    pub test_seconds: f64,
    /// Solids with the role `first_wall`; the clearance is measured to these.
    #[serde(default)]
    pub first_wall_solids: Vec<String>,
    /// The smallest distance from a sampled site to a first-wall surface, m. `None`: no solid
    /// has the role `first_wall`, so nothing was measured.
    #[serde(default)]
    pub first_wall_clearance_m: Option<f64>,
    /// The site that has it.
    #[serde(default)]
    pub first_wall_clearance_at_m: Option<[f64; 3]>,
    /// Sites inside a first-wall solid.
    #[serde(default)]
    pub in_first_wall: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct WaiverUse {
    pub index: usize,
    pub check_id: String,
    pub author: String,
    pub date: String,
    pub reason: String,
    pub covered_findings: usize,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct GeometryRecords {
    pub conversion: Option<ConversionRecord>,
    pub pre_imprint: Option<PreImprintRecord>,
    pub binding: Vec<BindingRow>,
    /// Per-solid faceted volume against the CAD volume, and the tolerance it was converted at.
    #[serde(default)]
    pub volume_gate: Option<VolumeGate>,
    pub integrity: Option<IntegrityRecord>,
    pub deviation: Option<DeviationRecord>,
    pub sources: Option<SourceRecord>,
    /// The fusion power the plasma profiles imply, next to the declared one (SRC-010).
    #[serde(default)]
    pub source_closure: Option<SourceClosure>,
    pub waivers: Vec<WaiverUse>,
    /// Wall time and the process's peak memory so far, per stage.
    pub stages: Vec<StageClock>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct StageClock {
    pub stage: String,
    pub seconds: f64,
    pub peak_rss_bytes: Option<u64>,
}

// ---------------------------------------------------------------------------
// Assembling outcomes.

struct SubSpec {
    name: &'static str,
    /// Requirement ids whose findings belong to this sub-check; empty: every finding of the stage.
    ids: &'static [&'static str],
    not_evaluated: Option<(String, String)>,
    note: Option<String>,
}

fn sub(name: &'static str, ids: &'static [&'static str]) -> SubSpec {
    SubSpec {
        name,
        ids,
        not_evaluated: None,
        note: None,
    }
}

struct Waivers<'a> {
    list: &'a [Waiver],
    used: Vec<bool>,
    covered: Vec<usize>,
}

impl<'a> Waivers<'a> {
    fn new(list: &'a [Waiver]) -> Self {
        Self {
            list,
            used: vec![false; list.len()],
            covered: vec![0; list.len()],
        }
    }

    fn apply(&mut self, findings: &mut [Finding]) {
        let before: Vec<bool> = findings.iter().map(|f| f.waived.is_some()).collect();
        let used = apply_waivers(findings, self.list);
        for (a, b) in self.used.iter_mut().zip(used) {
            *a |= b;
        }
        // Count each finding once, however many times the stage's list is passed through.
        for (f, was) in findings.iter().zip(before) {
            if let (false, Some(w)) = (was, &f.waived) {
                self.covered[w.index] += 1;
            }
        }
    }
}

fn finish(
    check: u8,
    mut findings: Vec<Finding>,
    subs: Vec<SubSpec>,
    waivers: &mut Waivers,
) -> CheckOutcome {
    waivers.apply(&mut findings);
    let sub_checks: Vec<SubCheck> = subs
        .into_iter()
        .map(|spec| {
            let mine: Vec<&Finding> = findings
                .iter()
                .filter(|f| {
                    spec.ids.is_empty()
                        || f.requirement
                            .as_deref()
                            .is_some_and(|r| spec.ids.contains(&r))
                })
                .collect();
            let blocking = mine.iter().filter(|f| f.blocks()).count();
            let waived = mine.len() - blocking;
            if let Some((why, next)) = spec.not_evaluated {
                return SubCheck {
                    name: spec.name.into(),
                    status: CheckStatus::NotEvaluated,
                    why: Some(why),
                    next_step: Some(next),
                };
            }
            let mut why = spec.note;
            if waived > 0 {
                let w = format!("{waived} finding(s) waived; they are listed below");
                why = Some(why.map_or(w.clone(), |n| format!("{n}; {w}")));
            }
            SubCheck {
                name: spec.name.into(),
                status: if blocking > 0 {
                    CheckStatus::Fail
                } else {
                    CheckStatus::Pass
                },
                why,
                next_step: (blocking > 0).then(|| "See the findings below.".to_string()),
            }
        })
        .collect();
    let fails = findings.iter().any(Finding::blocks);
    let unevaluated = sub_checks
        .iter()
        .find(|s| s.status == CheckStatus::NotEvaluated);
    let (status, why, next_step) = if fails {
        (CheckStatus::Fail, None, None)
    } else if let Some(s) = unevaluated {
        (
            CheckStatus::NotEvaluated,
            s.why.clone(),
            s.next_step.clone(),
        )
    } else {
        (CheckStatus::Pass, None, None)
    };
    CheckOutcome {
        check,
        name: check_name(check).into(),
        status,
        findings,
        why,
        next_step,
        sub_checks,
    }
}

fn unavailable(check: u8, why: String, next_step: String) -> CheckOutcome {
    CheckOutcome {
        check,
        name: check_name(check).into(),
        status: CheckStatus::NotEvaluated,
        findings: Vec::new(),
        why: Some(why),
        next_step: Some(next_step),
        sub_checks: Vec::new(),
    }
}

fn not_run(why: &str, next: &str) -> Option<(String, String)> {
    Some((why.to_string(), next.to_string()))
}

fn cancelled(_: Cancelled) -> DesignError {
    DesignError::Tool("the check was cancelled".into())
}

fn tool_finding(check: u8, item: &str, why: String, next: String, requirement: &str) -> Finding {
    Finding::new(check, item, why, next).requirement(requirement)
}

// ---------------------------------------------------------------------------
// The stages.

pub(super) struct Inputs<'a> {
    pub design: &'a Design,
    pub inspection: &'a StepInspection,
    pub step: &'a Path,
    pub backends: &'a dyn Backends,
    pub options: &'a GeometryOptions,
}

/// Run checks 6 to 9, pushing their outcomes onto `report.checks` (checks 1 to 5 are there already).
pub(super) fn run(inputs: &Inputs, report: &mut ImportReport) -> Result<(), DesignError> {
    let Inputs {
        design,
        inspection,
        step,
        backends,
        options,
    } = *inputs;
    // The helpers run in a job directory, so the STEP path must not depend on the working directory.
    let step_abs = std::path::absolute(step).map_err(|e| super::io_error(step, e))?;
    let step = step_abs.as_path();
    let temporary;
    let work: PathBuf = match &options.work_dir {
        Some(dir) => {
            std::fs::create_dir_all(dir).map_err(|e| super::io_error(dir, e))?;
            std::path::absolute(dir).map_err(|e| super::io_error(dir, e))?
        }
        None => {
            temporary = crate::scratch::tempdir("faris-cad-")
                .map_err(|e| DesignError::Io(e.to_string()))?;
            temporary.path().to_path_buf()
        }
    };
    let progress_fn = |stage: &str, done: usize, total: usize| {
        if let Some(p) = &options.progress {
            p(stage, done, total);
        }
    };
    let ctl = Ctl::new(&options.cancellation, &progress_fn, options.threads);
    let mut records = GeometryRecords::default();
    // SRC-010: pure arithmetic on the plasma block, so it is reported whatever stops the checks.
    match source::closure(&design.plasma) {
        Ok(closure) => {
            report.notices.push(Notice::new(
                9,
                "plasma.fusion_power_mw",
                if closure.closes {
                    format!(
                        "the declared power closes with the profiles: {}",
                        closure.summary()
                    )
                } else {
                    format!(
                        "the declared power does not close with the profiles: {}. This blocks \
                         check 9 when it runs (SRC-010); its finding says what to do.",
                        closure.summary()
                    )
                },
            ));
            records.source_closure = Some(closure);
        }
        Err(why) => report.notices.push(Notice::new(
            9,
            "plasma.fusion_power_mw",
            format!(
                "the fusion power the profiles imply could not be computed, so the declared power \
                 is not compared with them: {why}. Correct that field in the plasma block."
            ),
        )),
    }
    let mut waivers = Waivers::new(&design.waivers);
    let chord_m = design.cad.faceting_tolerance_m.unwrap_or(DEFAULT_CHORD_M);
    let shape_unit_m = if inspection.unit.declared.is_some() {
        1.0e-3
    } else {
        design.cad.length_unit.metres()
    };
    let tags: Vec<TagEntry> = design
        .solids
        .iter()
        .map(|s| TagEntry {
            step_index: s.step_index,
            tag: s.id.clone(),
        })
        .collect();
    let labels_by_step: BTreeMap<usize, &str> = design
        .solids
        .iter()
        .map(|s| (s.step_index, s.id.as_str()))
        .collect();
    let started_all = Instant::now();
    let clock = |records: &mut GeometryRecords, stage: &str, started: Instant| {
        records.stages.push(StageClock {
            stage: stage.into(),
            seconds: started.elapsed().as_secs_f64(),
            peak_rss_bytes: integrity::peak_rss_bytes(),
        });
    };

    // ---- Check 6: overlaps before imprinting, then conversion.
    let started = Instant::now();
    // The STEP solids are kept as BREP beside the products, so only the first helper reads the
    // STEP file; reading RM-M's takes 52 s and a BREP 1 s. Not used when reuse is off.
    let reuse = options.reuse && options.work_dir.is_some();
    let shape_dir = work.join("shapes");
    let shape_cache: Option<&Path> = reuse.then_some(shape_dir.as_path());
    let mut findings6: Vec<Finding> = Vec::new();
    let mut subs6 = vec![
        sub("overlap before imprinting (GEO-021)", &["GEO-021"]),
        sub("conversion to DAGMC (GEO-012, GEO-013)", &["GEO-012"]),
    ];
    let mut convert_blocked = false;
    let tess_dir = work.join("tessellate");
    let tess_request = TessellateRequest {
        step,
        step_sha256: &design.cad.step_sha256,
        shape_unit_m,
        tags: &tags,
        chord_cm: chord_m * 100.0,
        angular_rad: DEFAULT_ANGULAR_RAD,
        directory: &tess_dir,
        workers: options.threads.clamp(1, 4),
        reuse,
        shape_cache,
    };
    let mut pre_unavailable: Option<(String, String)> = None;
    match backends.tessellate(&tess_request)? {
        ToolResult::Done(tess) => {
            for problem in &tess.problems {
                findings6.push(tool_finding(
                    6,
                    "per-solid tessellation",
                    problem.clone(),
                    "Repair the solid in the CAD tool and export again.".into(),
                    "GEO-012",
                ));
            }
            let model = Model::new(&tess.facets).map_err(|reason| {
                DesignError::Tool(format!("the per-solid meshes were refused: {reason}"))
            })?;
            let candidates =
                overlap::candidate_pairs(&model, chord_m, options.seed, &ctl).map_err(cancelled)?;
            let mut pairs: Vec<PairRecord> = Vec::new();
            let mut boolean_seconds = None;
            let mut booleans_reused = false;
            if !candidates.is_empty() {
                let pair_ids: Vec<(usize, usize)> = candidates.iter().map(|c| (c.a, c.b)).collect();
                let boolean_dir = work.join("booleans");
                match backends.overlap_booleans(&BooleanRequest {
                    step,
                    step_sha256: &design.cad.step_sha256,
                    shape_unit_m,
                    pairs: &pair_ids,
                    chord_cm: chord_m * 100.0,
                    angular_rad: DEFAULT_ANGULAR_RAD,
                    directory: &boolean_dir,
                    workers: options.threads.clamp(1, 4),
                    reuse,
                    shape_cache,
                })? {
                    ToolResult::Done(out) => {
                        boolean_seconds = Some(out.seconds);
                        booleans_reused = out.reused;
                        let n = design
                            .solids
                            .iter()
                            .map(|s| s.step_index + 1)
                            .max()
                            .unwrap_or(0);
                        let mut volumes = vec![0.0; n];
                        let mut labels = vec![String::new(); n];
                        for s in &design.solids {
                            volumes[s.step_index] = s.fingerprint.cad_volume_m3;
                            labels[s.step_index] = s.id.clone();
                        }
                        let (records6, found) =
                            overlap::boolean_findings(&candidates, &out.results, &volumes, &labels);
                        pairs = records6;
                        findings6.extend(found);
                    }
                    ToolResult::Unavailable { why, next_step } => {
                        pre_unavailable = Some((why, next_step));
                    }
                    ToolResult::Failed { why, next_step } => {
                        findings6.push(tool_finding(
                            6,
                            "overlap booleans",
                            why,
                            next_step,
                            "GEO-021",
                        ));
                    }
                }
            }
            records.pre_imprint = Some(PreImprintRecord {
                tessellation_seconds: tess.seconds,
                tessellation_reused: tess.reused,
                tessellation_reader: tess.reader.clone(),
                candidate_pairs: candidates.len(),
                boolean_seconds,
                booleans_reused,
                pairs,
            });
        }
        ToolResult::Unavailable { why, next_step } => pre_unavailable = Some((why, next_step)),
        ToolResult::Failed { why, next_step } => {
            findings6.push(tool_finding(
                6,
                "per-solid tessellation",
                why,
                next_step,
                "GEO-021",
            ));
        }
    }
    if let Some((why, next)) = pre_unavailable {
        subs6[0].not_evaluated = Some((why, next));
    }
    clock(&mut records, "check 6 pre-imprint", started);
    let conversion_started = Instant::now();
    // Waivers decide whether a pre-imprint finding stops the conversion.
    waivers.apply(&mut findings6);
    if findings6.iter().any(Finding::blocks) {
        convert_blocked = true;
    }

    let mut converted: Option<ConvertOutcome> = None;
    if convert_blocked {
        subs6[1].not_evaluated = not_run(
            "the findings above stop the checks before the conversion",
            "Fix them, then check again.",
        );
    } else {
        let convert_dir = work.join("convert");
        let request = ConvertRequest {
            step,
            step_sha256: &design.cad.step_sha256,
            shape_unit_m,
            tags: &tags,
            chord_cm: chord_m * 100.0,
            angular_rad: DEFAULT_ANGULAR_RAD,
            imprint_threads: options.imprint_threads,
            directory: &convert_dir,
            reuse,
            shape_cache,
        };
        match backends.convert(&request)? {
            ToolResult::Done(out) => converted = Some(out),
            ToolResult::Unavailable { why, next_step } => {
                subs6[1].not_evaluated = Some((why, next_step));
            }
            ToolResult::Failed { why, next_step } => {
                findings6.push(tool_finding(6, "conversion", why, next_step, "GEO-012"));
            }
        }
    }
    if let Some(out) = &converted {
        let t = &out.report.tolerances;
        records.conversion = Some(ConversionRecord {
            reused: out.reused,
            directory: work.join("convert").display().to_string(),
            step_sha256: out.report.step.sha256.clone(),
            shape_unit_m,
            chord_cm: t.chord_cm,
            angular_rad: t.angular_rad,
            scale_factor_to_cm: t.scale_factor_to_cm,
            imprint_threads: t.imprint_threads,
            implicit_complement_tag: t.implicit_complement_tag.clone(),
            versions: out.report.versions.clone(),
            timing_seconds: out.report.timing_seconds.clone(),
            peak_rss_mb: out.report.peak_rss_mb,
            h5m_sha256: out.report.h5m.sha256.clone(),
            h5m_bytes: out.report.h5m.bytes,
            facets_sha256: out.report.facets.bin_sha256.clone(),
            counts: out.report.counts.clone(),
            helper_sha256: out.helper_sha256.clone(),
            volumes: out
                .manifest
                .volumes
                .iter()
                .zip(out.facets.vertices_per_volume())
                .map(|(v, vertices)| ConvertedVolume {
                    volume: v.index,
                    global_id: v.global_id,
                    material_tag: v.material_tag.clone(),
                    triangles: v.triangles,
                    vertices,
                })
                .collect(),
        });
    }
    clock(&mut records, "check 6 conversion", conversion_started);
    clock(&mut records, "check 6", started);
    let six = finish(6, findings6, subs6, &mut waivers);
    let six_status = six.status;
    report.checks.push(six);
    let Some(converted) = converted.filter(|_| six_status == CheckStatus::Pass) else {
        finalize(report, records, waivers, design);
        return Ok(());
    };

    // ---- Check 7: binding.
    let started = Instant::now();
    let model = match Model::new(&converted.facets) {
        Ok(model) => model,
        Err(reason) => {
            let f = Finding::new(
                7,
                "the converted model",
                format!("FARIS refuses the model: {reason}"),
                "Check the length unit and the size of the model in the CAD tool.",
            )
            .requirement("GEO-020");
            report.checks.push(finish(
                7,
                vec![f],
                vec![sub("binding by volume and centroid", &[])],
                &mut waivers,
            ));
            finalize(report, records, waivers, design);
            return Ok(());
        }
    };
    let stats = binding::volume_stats(&model.mesh, &converted.facets, &converted.manifest);
    let diagonal = inspection
        .model_bbox_diagonal_m
        .unwrap_or(model.mesh.diagonal() * 1.0e-9);
    let mut bound = binding::bind(design, &stats, diagonal);
    records.binding = bound.rows.clone();
    let (gate, gate_findings) = binding::volume_gate(
        &bound.rows,
        chord_m,
        design.cad.faceting_tolerance_m.is_some(),
    );
    bound.findings.extend(gate_findings);
    let largest = bound
        .rows
        .iter()
        .filter_map(|r| r.volume_relative_difference)
        .map(f64::abs)
        .fold(0.0, f64::max);
    clock(&mut records, "check 7", started);
    let mut subs7 = vec![sub("binding by volume and centroid", &[])];
    subs7[0].note = Some(format!(
        "the largest volume difference is {:.4} % of the CAD volume; the limit is {:.1} % at the {} faceting tolerance of {chord_m:.3e} m",
        100.0 * largest,
        100.0 * gate.limit,
        gate.tolerance_source
    ));
    records.volume_gate = Some(gate.clone());
    let seven = finish(7, bound.findings, subs7, &mut waivers);
    let seven_pass = seven.status == CheckStatus::Pass;
    report.checks.push(seven);
    let Some(volume_solid) = bound.volume_solid.filter(|_| seven_pass) else {
        finalize(report, records, waivers, design);
        return Ok(());
    };
    drop(converted.facets);

    // ---- Check 8: integrity, and GEO-032.
    let started = Instant::now();
    let labels: Vec<String> = volume_solid
        .iter()
        .map(|&s| design.solids[s].id.clone())
        .collect();
    let config = integrity::Config {
        chord_m,
        seed: options.seed,
        samples: options.samples,
        chords: options.chords,
    };
    let result = integrity::check_integrity(&model, &labels, &config, &ctl).map_err(cancelled)?;
    let mut findings8 = result.findings;
    let mut integrity_record = result.record;
    clock(&mut records, "check 8 (integrity)", started);

    let started = Instant::now();
    let mut deviation_sub = sub("faceting deviation (GEO-032)", &["GEO-032"]);
    let map: Vec<(u32, usize)> = volume_solid
        .iter()
        .enumerate()
        .map(|(v, &s)| (v as u32 + 1, design.solids[s].step_index))
        .collect();
    let deviation_dir = work.join("deviation");
    let mut sliver_note: Option<String> = None;
    match backends.deviation(&DeviationRequest {
        step,
        shape_unit_m,
        facets_bin: &converted.facets_bin,
        map: &map,
        directory: &deviation_dir,
        workers: options.threads.clamp(1, 4),
        occt_points: options.deviation_points,
        shape_cache,
    })? {
        ToolResult::Done(out) => {
            let mut solids = Vec::new();
            for row in &out.rows {
                let label = labels_by_step
                    .get(&row.step_index)
                    .copied()
                    .unwrap_or("?")
                    .to_string();
                let above = row.max_distance_m > chord_m;
                let complete = row.points >= row.points_available;
                if above {
                    let mut f = Finding::new(
                        8,
                        format!("{label}: faceting deviation"),
                        format!(
                            "the facets deviate from the CAD surface by {:.3e} m ({}), \
                             above the declared faceting tolerance of {chord_m:.3e} m",
                            row.max_distance_m,
                            if complete {
                                "maximum over all points"
                            } else {
                                "sampled maximum"
                            }
                        ),
                        "Set cad.faceting_tolerance_m smaller so the facets follow the surfaces \
                         more closely, then convert again.",
                    )
                    .requirement("GEO-032")
                    .cells(vec![label.clone()])
                    .waivable();
                    if let Some(p) = row.at_m {
                        f = f.at(p);
                    }
                    findings8.push(f);
                }
                solids.push(DeviationSolid {
                    solid_id: label,
                    max_distance_m: row.max_distance_m,
                    at_m: row.at_m,
                    points: row.points,
                    points_available: row.points_available,
                    points_closed_form: row.points_closed_form,
                    points_occt: row.points_occt,
                    points_occt_needed: row.points_occt_needed,
                    complete,
                    above_tolerance: above,
                });
            }
            let sampled = solids.iter().filter(|s| !s.complete).count();
            // The sliver rule that compares the deviation with the thickness needs it per solid.
            let deviations: Vec<integrity::SolidDeviation> = solids
                .iter()
                .map(|s| integrity::SolidDeviation {
                    solid_id: &s.solid_id,
                    max_distance_m: s.max_distance_m,
                    at_m: s.at_m,
                    complete: s.complete,
                })
                .collect();
            findings8.extend(integrity::thickness_misrepresented(
                &integrity_record.volumes,
                &deviations,
            ));
            records.deviation = Some(DeviationRecord {
                method: out.method,
                chord_tolerance_m: chord_m,
                seconds: out.seconds,
                statement: if sampled == 0 {
                    "maximum over all points: every triangle centroid and edge midpoint of every \
                     solid was measured, in closed form on plane, cylinder, cone, sphere and torus \
                     faces and by OpenCASCADE projection elsewhere; it is not a bound over points \
                     between them"
                        .to_string()
                } else {
                    format!(
                        "sampled maximum for {sampled} of {} solid(s): their points that need \
                         OpenCASCADE projection (--deviation-points) are an even stride of at \
                         most {} per solid, the others are measured in closed form; the maximum \
                         is exact over the points measured, not a bound over every point",
                        solids.len(),
                        options.deviation_points
                    )
                },
                solids,
            });
        }
        ToolResult::Unavailable { why, next_step } => {
            sliver_note = Some(
                "only the rule that compares a solid's thickness with the faceting tolerance was \
                 evaluated; the thickness-misrepresented rule needs GEO-032, which did not run"
                    .to_string(),
            );
            deviation_sub.not_evaluated = Some((why, next_step));
        }
        ToolResult::Failed { why, next_step } => {
            sliver_note = Some(
                "only the rule that compares a solid's thickness with the faceting tolerance was \
                 evaluated; the thickness-misrepresented rule needs GEO-032, which failed"
                    .to_string(),
            );
            findings8.push(tool_finding(
                8,
                "faceting deviation",
                why,
                next_step,
                "GEO-032",
            ));
        }
    }
    clock(&mut records, "GEO-032 deviation", started);

    // The void list (GEO-039): never blocks, always reported.
    let voids = void_solids(design);
    let void_note = format!(
        "{} void solid(s) and {} enclosed region(s) of the implicit complement are listed in the report",
        voids.len(),
        integrity_record.cavities.len()
    );
    integrity_record.peak_rss_bytes = integrity::peak_rss_bytes();
    let mut void_sub = sub("void list (GEO-039)", &["GEO-039"]);
    void_sub.note = Some(void_note);
    let mut volume_sub = sub("volume against the CAD (GEO-030)", &["GEO-030"]);
    volume_sub.note = Some(format!(
        "checked in check 7 for every solid; the largest difference is {:.4} % (limit {:.1} % at the {} faceting tolerance)",
        100.0 * largest,
        100.0 * gate.limit,
        gate.tolerance_source
    ));
    let subs8 = vec![
        sub("watertight and orientation (GEO-020)", &["GEO-020"]),
        sub(
            "inverted normals, zero-volume cells, degenerate triangles",
            &["INVERTED-NORMALS", "ZERO-VOLUME", "DEGENERATE-TRIANGLE"],
        ),
        {
            let mut slivers = sub("slivers", &["SLIVER"]);
            slivers.note = sliver_note;
            slivers
        },
        sub("self-intersection", &["SELF-INTERSECTION"]),
        sub("overlap and containment (GEO-021)", &["GEO-021"]),
        sub("duplicate surfaces", &["DUPLICATE-SURFACE"]),
        sub("gaps (GEO-022)", &["GEO-022"]),
        sub("point sampling (GEO-024)", &["GEO-024"]),
        sub("ray consistency (GEO-025)", &["GEO-025"]),
        volume_sub,
        sub("partition (GEO-031)", &["GEO-031"]),
        deviation_sub,
        void_sub,
    ];
    let eight = finish(8, findings8, subs8, &mut waivers);
    records.integrity = Some(integrity_record);
    let eight_pass = eight.status != CheckStatus::Fail;
    report.checks.push(eight);
    if !eight_pass {
        finalize(report, records, waivers, design);
        return Ok(());
    }

    // ---- Check 9: source sites against the plasma chamber volume.
    let started = Instant::now();
    let chamber = design.plasma.chamber_solid_id.clone().unwrap_or_default();
    let chamber_volume = design
        .solids
        .iter()
        .position(|s| s.id == chamber)
        .and_then(|solid| volume_solid.iter().position(|&s| s == solid))
        .map(|v| v as u32 + 1);
    let source_dir = work.join("sources");
    let sampled = match chamber_volume {
        Some(_) => Some(backends.source_sites(&SourceRequest {
            plasma: &design.plasma,
            count: options.source_sites,
            seed: options.seed,
            directory: &source_dir,
        })?),
        None => None,
    };
    let nine = match (chamber_volume, sampled) {
        (None, _) | (Some(_), None) => finish(
            9,
            [Finding::new(
                9,
                "plasma.chamber_solid_id",
                format!("there is no volume for the plasma chamber solid {chamber:?}"),
                "Name the plasma chamber solid in plasma.chamber_solid_id.",
            )
            .requirement("SRC-018")]
            .into_iter()
            .chain(closure_finding(records.source_closure.as_ref(), &chamber))
            .collect(),
            vec![sub(
                "sites inside the plasma chamber and clear of the first wall (SRC-018)",
                &[],
            )],
            &mut waivers,
        ),
        (Some(_), Some(ToolResult::Unavailable { why, next_step })) => {
            unavailable(9, why, next_step)
        }
        (Some(_), Some(ToolResult::Failed { why, next_step })) => finish(
            9,
            [tool_finding(
                9,
                "source sampling",
                why,
                next_step,
                "SRC-018",
            )]
            .into_iter()
            .chain(closure_finding(records.source_closure.as_ref(), &chamber))
            .collect(),
            vec![sub(
                "sites inside the plasma chamber and clear of the first wall (SRC-018)",
                &[],
            )],
            &mut waivers,
        ),
        (Some(volume), Some(ToolResult::Done(sites))) => {
            let tested = Instant::now();
            let caster = Caster::new(&model.mesh, &model.bvh);
            let first_wall_solids: Vec<String> = design
                .solids
                .iter()
                .filter(|s| s.role == Some(Role::FirstWall))
                .map(|s| s.id.clone())
                .collect();
            let first_wall_volumes: Vec<u32> = volume_solid
                .iter()
                .enumerate()
                .filter(|&(_, &s)| design.solids[s].role == Some(Role::FirstWall))
                .map(|(v, _)| v as u32 + 1)
                .collect();
            let count = sites.points.len();
            let chunks = ctl
                .run("source sites", count, 2048, |range| {
                    let mut scratch = caster.scratch();
                    let mut out = SiteChunk::default();
                    for i in range {
                        let p = sites.points[i];
                        let g = [
                            super::mesh::to_grid(p[0]),
                            super::mesh::to_grid(p[1]),
                            super::mesh::to_grid(p[2]),
                        ];
                        let Some(point) = (match g {
                            [Some(x), Some(y), Some(z)] => Some([x, y, z]),
                            _ => None,
                        }) else {
                            out.outside += 1;
                            out.first_outside.get_or_insert(p);
                            *out.elsewhere.entry(0).or_default() += 1;
                            continue;
                        };
                        let seed = Rng::for_item(options.seed ^ 0x5C, i as u64).next_u64();
                        match caster.classify(point, seed, &mut scratch) {
                            Ok(class) if class.volumes.contains(&volume) => {
                                out.inside += 1;
                                if class.volumes.iter().any(|v| first_wall_volumes.contains(v)) {
                                    out.in_first_wall += 1;
                                    out.first_in_wall.get_or_insert(p);
                                }
                            }
                            Ok(class) => {
                                out.outside += 1;
                                out.first_outside.get_or_insert(p);
                                *out.elsewhere
                                    .entry(class.volumes.first().copied().unwrap_or(0))
                                    .or_default() += 1;
                            }
                            Err(_) => {
                                out.undecided += 1;
                                out.first_outside.get_or_insert(p);
                            }
                        }
                    }
                    out
                })
                .map_err(cancelled)?;
            let mut total = SiteChunk::default();
            for c in chunks {
                total.inside += c.inside;
                total.outside += c.outside;
                total.undecided += c.undecided;
                total.in_first_wall += c.in_first_wall;
                if total.first_outside.is_none() {
                    total.first_outside = c.first_outside;
                }
                if total.first_in_wall.is_none() {
                    total.first_in_wall = c.first_in_wall;
                }
                for (v, n) in c.elsewhere {
                    *total.elsewhere.entry(v).or_default() += n;
                }
            }
            let outside_in: BTreeMap<String, u64> = total
                .elsewhere
                .iter()
                .map(|(&v, &n)| {
                    (
                        if v == 0 {
                            "(outside every solid)".to_string()
                        } else {
                            labels[v as usize - 1].clone()
                        },
                        n,
                    )
                })
                .collect();
            let clearance = first_wall_clearance(&model, &first_wall_volumes, &sites.points, &ctl)
                .map_err(cancelled)?;
            records.sources = Some(SourceRecord {
                sites: count as u64,
                seed: options.seed,
                plasma_chamber_solid: chamber.clone(),
                inside: total.inside,
                outside: total.outside,
                undecided: total.undecided,
                outside_in,
                sites_sha256: sites.bin_sha256.clone(),
                versions: sites.versions.clone(),
                sampling_seconds: sites.seconds,
                test_seconds: tested.elapsed().as_secs_f64(),
                first_wall_solids: first_wall_solids.clone(),
                first_wall_clearance_m: clearance.map(|c| c.0),
                first_wall_clearance_at_m: clearance.map(|c| c.1),
                in_first_wall: total.in_first_wall,
            });
            let mut findings9 = Vec::new();
            if total.outside + total.undecided > 0 {
                let bad = total.outside + total.undecided;
                let mut f = Finding::new(
                    9,
                    format!("plasma sites outside {chamber}"),
                    format!(
                        "{bad} of {count} source sites ({:.3} %) are not inside the plasma chamber \
                         volume ({} outside it, {} undecided); a particle born there would start \
                         in another material or outside the model",
                        100.0 * bad as f64 / count.max(1) as f64,
                        total.outside,
                        total.undecided
                    ),
                    "Fit the plasma block (radii, elongation, triangularity, shift) inside the plasma \
                     chamber solid, or correct the chamber solid in the CAD tool.",
                )
                .requirement("SRC-018")
                .cells(vec![chamber.clone()]);
                if let Some(p) = total.first_outside {
                    f = f.at(p);
                }
                findings9.push(f);
            }
            // Clearance to the first wall. The row states no minimum, so the only limit is that a
            // site is not in or on a first-wall solid; the measured value is always reported.
            if total.in_first_wall > 0 {
                let mut f = Finding::new(
                    9,
                    format!("plasma sites inside {}", first_wall_solids.join(", ")),
                    format!(
                        "{} of {count} source sites are inside a first-wall solid (clearance 0 m); \
                         a neutron born there would start inside the wall",
                        total.in_first_wall
                    ),
                    "Shrink the plasma block (radii, elongation, shift) or move the first wall so \
                     the plasma region lies clear of it.",
                )
                .requirement("SRC-018")
                .cells(first_wall_solids.clone());
                if let Some(p) = total.first_in_wall {
                    f = f.at(p);
                }
                findings9.push(f);
            } else if let Some((distance, at)) = clearance.filter(|c| c.0 <= 0.0) {
                findings9.push(
                    Finding::new(
                        9,
                        format!("plasma sites touch {}", first_wall_solids.join(", ")),
                        format!(
                            "a source site lies {distance:.3e} m from a first-wall surface, which is \
                             not a positive clearance"
                        ),
                        "Shrink the plasma block or move the first wall so the plasma region lies \
                         clear of it.",
                    )
                    .requirement("SRC-018")
                    .cells(first_wall_solids.clone())
                    .at(at),
                );
            } else if let (Some(minimum), Some((distance, at))) =
                (design.plasma.min_first_wall_clearance_m, clearance)
                && distance < minimum
            {
                findings9.push(
                    Finding::new(
                        9,
                        format!("plasma sites too close to {}", first_wall_solids.join(", ")),
                        format!(
                            "the nearest source site is {distance:.4} m from a first-wall surface, \
                             below the minimum clearance of {minimum} m that \
                             plasma.min_first_wall_clearance_m sets"
                        ),
                        "Shrink the plasma block (radii, elongation, shift) or move the first wall \
                         so the plasma region is at least that far from it, or lower \
                         plasma.min_first_wall_clearance_m if the minimum was set too high.",
                    )
                    .requirement("SRC-018")
                    .cells(first_wall_solids.clone())
                    .at(at),
                );
            }
            findings9.extend(closure_finding(records.source_closure.as_ref(), &chamber));
            finish(
                9,
                findings9,
                vec![{
                    let mut s = sub(
                        "sites inside the plasma chamber and clear of the first wall (SRC-018)",
                        &[],
                    );
                    let minimum_note = match design.plasma.min_first_wall_clearance_m {
                        Some(m) => format!("the design file sets a minimum of {m} m"),
                        None => "the design file sets no minimum, so only a site in or on the \
                                 wall fails"
                            .to_string(),
                    };
                    let clearance_note = match clearance {
                        Some((d, at)) => format!(
                            "the nearest site to the first wall ({}) is {d:.4} m away, at \
                             ({:.3}, {:.3}, {:.3}) m; {minimum_note}",
                            first_wall_solids.join(", "),
                            at[0],
                            at[1],
                            at[2]
                        ),
                        None if design.plasma.min_first_wall_clearance_m.is_some() => {
                            s.not_evaluated = Some((
                                "plasma.min_first_wall_clearance_m is set, but no solid has the \
                                 role first_wall, so the clearance could not be measured"
                                    .to_string(),
                                "Give the first-wall solid the role first_wall, or set \
                                 plasma.min_first_wall_clearance_m to null."
                                    .to_string(),
                            ));
                            "clearance to the first wall was not measured".to_string()
                        }
                        None => {
                            "clearance to the first wall was not measured because no solid has \
                                 the role first_wall"
                                .to_string()
                        }
                    };
                    s.note = Some(format!(
                        "{} of {count} sites are inside; each is tested with FARIS's exact \
                         point-in-volume; {clearance_note}",
                        total.inside
                    ));
                    s
                }],
                &mut waivers,
            )
        }
    };
    clock(&mut records, "check 9", started);
    report.checks.push(nine);
    records.stages.push(StageClock {
        stage: "checks 6 to 9".into(),
        seconds: started_all.elapsed().as_secs_f64(),
        peak_rss_bytes: integrity::peak_rss_bytes(),
    });
    finalize(report, records, waivers, design);
    Ok(())
}

#[derive(Default)]
struct SiteChunk {
    in_first_wall: u64,
    first_in_wall: Option<[f64; 3]>,
    inside: u64,
    outside: u64,
    undecided: u64,
    first_outside: Option<[f64; 3]>,
    elsewhere: BTreeMap<u32, u64>,
}

/// The smallest distance from any of `sites` to a surface of the first-wall volumes, m, and the
/// site that has it. `None` when there is no first-wall volume or it has no triangle.
fn first_wall_clearance(
    model: &Model,
    volumes: &[u32],
    sites: &[[f64; 3]],
    ctl: &Ctl,
) -> Result<Option<(f64, [f64; 3])>, Cancelled> {
    let per_volume = model.mesh.volume_triangles();
    let triangles: Vec<u32> = volumes
        .iter()
        .flat_map(|&v| per_volume[v as usize - 1].iter().copied())
        .collect();
    if triangles.is_empty() {
        return Ok(None);
    }
    let mesh = &model.mesh;
    let walls = Bvh::build(mesh, triangles);
    let reach_limit = mesh.diagonal() + 1.0;
    let chunks = ctl.run("first-wall clearance", sites.len(), 2048, |range| {
        let mut best: Option<(f64, [f64; 3])> = None;
        for i in range {
            let p = sites[i];
            let (Some(x), Some(y), Some(z)) = (
                super::mesh::to_grid(p[0]),
                super::mesh::to_grid(p[1]),
                super::mesh::to_grid(p[2]),
            ) else {
                continue;
            };
            let bound = best.map_or(f64::INFINITY, |b| b.0);
            if let Some(d) = nearest_nm(mesh, &walls, [x, y, z], bound, reach_limit)
                && d < bound
            {
                best = Some((d, p));
            }
        }
        best
    })?;
    Ok(chunks
        .into_iter()
        .flatten()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(nm, at)| (nm / super::mesh::GRID_PER_M, at)))
}

/// The distance, nm, from `p` to the nearest triangle in `walls`, when it is below `bound`
/// (`INFINITY`: always). A triangle within `r` of `p` has a box that meets the box of half-width
/// `r` around `p`, so the search grows `r` until the nearest candidate is within it.
fn nearest_nm(mesh: &Mesh, walls: &Bvh, p: P3, bound: f64, reach_limit_nm: f64) -> Option<f64> {
    let mut r = if bound.is_finite() {
        bound.ceil() + 1.0
    } else {
        1.0e7
    };
    loop {
        let radius = r as i64;
        let lo = [p[0] - radius, p[1] - radius, p[2] - radius];
        let hi = [p[0] + radius, p[1] + radius, p[2] + radius];
        let mut nearest = f64::INFINITY;
        walls.query_box(lo, hi, &mut |t| {
            nearest = nearest.min(overlap::point_triangle_distance(p, mesh.tri_pts(t)));
        });
        if nearest <= r {
            return Some(nearest);
        }
        if bound.is_finite() || r > reach_limit_nm {
            return None;
        }
        r = if nearest.is_finite() {
            nearest.ceil() + 1.0
        } else {
            r * 4.0
        };
    }
}

/// The check 9 finding for a declared power the profiles do not close with (SRC-010). It names
/// the plasma chamber solid, so a waiver covers it with that solid in `solid_ids`.
fn closure_finding(closure: Option<&SourceClosure>, chamber: &str) -> Option<Finding> {
    let closure = closure.filter(|c| !c.closes)?;
    Some(
        Finding::new(
            9,
            "plasma.fusion_power_mw",
            closure.disagreement(),
            SourceClosure::next_step(),
        )
        .requirement("SRC-010")
        .cells(vec![chamber.to_string()])
        .waivable(),
    )
}

/// Put the records in the report, and say which waivers covered nothing.
fn finalize(
    report: &mut ImportReport,
    mut records: GeometryRecords,
    waivers: Waivers,
    design: &Design,
) {
    for (i, w) in waivers.list.iter().enumerate() {
        records.waivers.push(WaiverUse {
            index: i,
            check_id: w.check_id.clone(),
            author: w.author.clone(),
            date: w.date.clone(),
            reason: w.reason.clone(),
            covered_findings: waivers.covered[i],
        });
        if !waivers.used[i] {
            report.notices.push(Notice::new(
                8,
                format!("waivers[{i}]"),
                format!(
                    "this waiver for {} covered no finding in this run; it is kept in the design file and costs nothing",
                    w.check_id
                ),
            ));
        }
    }
    let _ = design;
    report.geometry = records;
}

#[cfg(test)]
mod tests;
