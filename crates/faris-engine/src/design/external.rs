//! The external half of checks 6 to 9: the CAD helper's conversion, tessellation,
//! boolean and deviation commands and the OpenMC source sampler, each run as a
//! bounded job. The helpers are embedded in the binary and written next to the
//! job's products, so what ran is what the report's hash says.

use super::geometry::{
    BooleanOutcome, BooleanRequest, ConversionReport, ConvertOutcome, ConvertRequest,
    DeviationOutcome, DeviationRequest, DeviationRow, SourceOutcome, SourceRequest,
    TessellateOutcome, TessellateRequest, ToolResult,
};
use super::overlap::BooleanResult;
use super::reader::{ActiveReader, NativeReader, ReaderStamp};
use super::{
    BOOLEAN_TIMEOUT, CAD_ADDRESS_SPACE, CAD_DEVIATION_HELPER, CAD_FACETS_HELPER, CAD_HELPER,
    CONVERT_TIMEOUT, DEVIATION_TIMEOUT, DesignError, ExternalBackends, Launched,
    SOURCE_SITES_HELPER, SOURCE_TIMEOUT, TESSELLATE_TIMEOUT, io_error, launch_python,
    read_json_bounded, sha256_file, sha256_hex, tail,
};
use crate::jobs::ResourceLimits;
use faris_model::design::{LengthUnit, load_facets};
use faris_occt_client::{
    client::{Bounds, WorkerError},
    protocol::{Limits, Operation, Tag},
};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn missing_python<T>(what: &str, kind: &str) -> ToolResult<T> {
    ToolResult::Unavailable {
        why: format!("no {kind} interpreter is configured, so {what} cannot run"),
        next_step: if kind == "CAD" {
            "Pass --cad-python PATH or set FARIS_CAD_PYTHON to a Python with cadquery, \
             cad_to_dagmc and h5py, then check again."
                .into()
        } else {
            "Pass --openmc-python PATH or set FARIS_OPENMC_PYTHON to a Python with openmc, \
             and --plasma-source-path to the directory holding openmc_plasma_source, then check again."
                .into()
        },
    }
}

fn cad_limits() -> ResourceLimits {
    ResourceLimits::with_address_space(CAD_ADDRESS_SPACE)
}

/// The bounds a native stage runs under: the stage's wall-clock timeout, and the address space
/// of the Python stage's job limits. The worker is one thread, so its CPU seconds cannot pass
/// the wall-clock seconds; the CPU limit is set to them.
fn native_bounds(timeout: Duration, limits: ResourceLimits) -> Bounds {
    Bounds {
        wall_seconds: timeout.as_secs_f64(),
        limits: Limits {
            address_space_mb: Some(limits.address_space_bytes >> 20),
            cpu_seconds: Some(timeout.as_secs()),
        },
    }
}

/// STEP inspect in the native worker: the same `faris-step-inspect/v1` report as the helper's.
pub(super) fn inspect_native(
    ext: &ExternalBackends,
    native: &NativeReader,
    step: &Path,
    assume: Option<LengthUnit>,
) -> Result<super::InspectOutcome, DesignError> {
    let step = step.canonicalize().map_err(|e| io_error(step, e))?;
    let started = Instant::now();
    let value = native
        .worker
        .run_cancellable(
            Operation::Inspect {
                step,
                assume_unit: assume.map(|u| u.label().to_string()),
            },
            native_bounds(super::CAD_TIMEOUT, ResourceLimits::default()),
            &|| ext.cancellation.is_cancelled(),
        )
        .map_err(|e| DesignError::Tool(format!("the native CAD reader: {e}")))?;
    let elapsed_seconds = started.elapsed().as_secs_f64();
    let bytes = serde_json::to_vec(&value).map_err(|e| DesignError::Tool(e.to_string()))?;
    let inspection =
        faris_model::design::StepInspection::from_bytes(&bytes).map_err(DesignError::Tool)?;
    Ok(super::InspectOutcome {
        inspection,
        helper_sha256: native.stamp.helper_sha256.clone(),
        reader: native.stamp.clone(),
        elapsed_seconds,
    })
}

/// A worker that failed to run a stage: a timeout, a crash or a failure it reported is a failed
/// stage with a next step; a worker that could not be started or answered unreadably, or a
/// cancel, ends the check.
fn worker_failure<T>(error: WorkerError, next_step: &str) -> Result<ToolResult<T>, DesignError> {
    match error {
        WorkerError::Cancelled | WorkerError::Spawn(_) | WorkerError::BadResponse(_) => {
            Err(DesignError::Tool(format!("the native CAD reader: {error}")))
        }
        WorkerError::TimedOut { .. } | WorkerError::Crashed { .. } | WorkerError::Failed(_) => {
            Ok(ToolResult::Failed {
                why: format!("the native CAD reader: {error}"),
                next_step: next_step.into(),
            })
        }
    }
}

fn write_text(path: &Path, text: &str) -> Result<(), DesignError> {
    std::fs::write(path, text).map_err(|e| io_error(path, e))
}

/// Write the embedded CAD helpers beside the job; returns the script to run.
fn write_cad_helpers(dir: &Path) -> Result<PathBuf, DesignError> {
    let helpers = dir.join("helpers");
    std::fs::create_dir_all(&helpers).map_err(|e| io_error(&helpers, e))?;
    write_text(&helpers.join("faris_facets.py"), CAD_FACETS_HELPER)?;
    write_text(&helpers.join("faris_deviation.py"), CAD_DEVIATION_HELPER)?;
    let script = helpers.join("faris_cad.py");
    write_text(&script, CAD_HELPER)?;
    Ok(script)
}

fn job_environment(dir: &Path) -> Result<Vec<(OsString, OsString)>, DesignError> {
    let tmp = dir.join("tmp");
    std::fs::create_dir_all(&tmp).map_err(|e| io_error(&tmp, e))?;
    Ok(vec![("TMPDIR".into(), tmp.into_os_string())])
}

fn log_tail(path: &Path) -> String {
    std::fs::read_to_string(path)
        .map(|t| format!("\nlog tail:\n{}", tail(&t)))
        .unwrap_or_default()
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path, what: &str) -> Result<T, DesignError> {
    serde_json::from_slice(&read_json_bounded(path)?).map_err(|e| {
        DesignError::Tool(format!(
            "{what} ({}) could not be read: {e}",
            path.display()
        ))
    })
}

fn tags_json(dir: &Path, tags: &[super::geometry::TagEntry]) -> Result<PathBuf, DesignError> {
    let path = dir.join("tags.json");
    let body = serde_json::to_string(tags).map_err(|e| DesignError::Tool(e.to_string()))?;
    write_text(&path, &body)?;
    Ok(path)
}

fn load_pair(
    dir: &Path,
    bin: &str,
    json: &str,
) -> Result<
    (
        faris_model::design::Facets,
        faris_model::design::FacetsManifest,
    ),
    DesignError,
> {
    let bin_path = dir.join(bin);
    let json_path = dir.join(json);
    let binary = std::fs::read(&bin_path).map_err(|e| io_error(&bin_path, e))?;
    let manifest = read_json_bounded(&json_path)?;
    let loaded = load_facets(&binary, &manifest)
        .map_err(|e| DesignError::Tool(format!("{}: {e}", bin_path.display())))?;
    Ok((loaded.facets, loaded.manifest))
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-12 * a.abs().max(b.abs()).max(1e-300)
}

/// The helper's version, read from the embedded script so there is one place that says it.
pub(super) fn helper_version() -> String {
    CAD_HELPER
        .lines()
        .find_map(|l| l.strip_prefix("HELPER_VERSION = \""))
        .and_then(|rest| rest.split('"').next())
        .unwrap_or("?")
        .to_string()
}

/// What decides a tessellation or a set of boolean verdicts: the STEP hash, the tolerances, the
/// helper's version, and (for booleans) the pairs. A recorded result is reused only when the key
/// matches what the run would use.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct CacheKey {
    command: String,
    step_sha256: String,
    chord_cm: f64,
    angular_rad: f64,
    shape_unit_m: f64,
    helper: String,
    #[serde(default)]
    tags: Vec<super::geometry::TagEntry>,
    #[serde(default)]
    pairs: Vec<[usize; 2]>,
}

impl CacheKey {
    fn same(&self, other: &Self) -> bool {
        self.command == other.command
            && self.step_sha256.eq_ignore_ascii_case(&other.step_sha256)
            && close(self.chord_cm, other.chord_cm)
            && close(self.angular_rad, other.angular_rad)
            && close(self.shape_unit_m, other.shape_unit_m)
            && self.helper == other.helper
            && self.tags == other.tags
            && self.pairs == other.pairs
    }

    fn path(dir: &Path) -> PathBuf {
        dir.join("cache-key.json")
    }

    fn recorded(dir: &Path) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(Self::path(dir)).ok()?).ok()
    }

    fn record(&self, dir: &Path) -> Result<(), DesignError> {
        let body =
            serde_json::to_string_pretty(self).map_err(|e| DesignError::Tool(e.to_string()))?;
        write_text(&Self::path(dir), &body)
    }
}

fn shape_cache_arguments(cache: Option<&Path>) -> Vec<OsString> {
    cache.map_or_else(Vec::new, |dir| {
        vec!["--shape-cache".into(), dir.as_os_str().to_owned()]
    })
}

/// A recorded conversion is reused only when everything that decides its result
/// matches: the STEP hash, the tags, the tolerances and scale, and the hashes of the
/// h5m and the facets it produced.
fn reuse(request: &ConvertRequest) -> Result<Option<ConvertOutcome>, DesignError> {
    let dir = request.directory;
    let report_path = dir.join("conversion.json");
    if !report_path.is_file() {
        return Ok(None);
    }
    let Ok(report) = read_json::<ConversionReport>(&report_path, "the recorded conversion") else {
        return Ok(None);
    };
    let t = &report.tolerances;
    let mut tags = request.tags.to_vec();
    tags.sort_by_key(|e| e.step_index);
    let same = report.schema == "faris-conversion/v1"
        && report.step.sha256.eq_ignore_ascii_case(request.step_sha256)
        && close(t.chord_cm, request.chord_cm)
        && close(t.angular_rad, request.angular_rad)
        && close(t.scale_factor_to_cm, request.shape_unit_m * 100.0)
        && t.implicit_complement_tag == "void"
        && report.tags == tags;
    if !same {
        return Ok(None);
    }
    let h5m = dir.join(&report.h5m.file);
    if !h5m.is_file() || !sha256_file(&h5m)?.eq_ignore_ascii_case(&report.h5m.sha256) {
        return Ok(None);
    }
    let Ok((facets, manifest)) = load_pair(dir, &report.facets.bin, &report.facets.json) else {
        return Ok(None);
    };
    if !manifest
        .binary_sha256
        .eq_ignore_ascii_case(&report.facets.bin_sha256)
        || !manifest
            .source
            .sha256
            .eq_ignore_ascii_case(&report.h5m.sha256)
    {
        return Ok(None);
    }
    let facets_bin = dir.join(&report.facets.bin);
    Ok(Some(ConvertOutcome {
        report,
        facets,
        manifest,
        facets_bin,
        reused: true,
        helper_sha256: sha256_hex(CAD_HELPER.as_bytes()),
    }))
}

pub(super) fn convert(
    ext: &ExternalBackends,
    request: &ConvertRequest,
) -> Result<ToolResult<ConvertOutcome>, DesignError> {
    let Some(python) = ext.tools.cad_python.as_deref() else {
        return Ok(missing_python("the conversion", "CAD"));
    };
    let dir = request.directory;
    std::fs::create_dir_all(dir).map_err(|e| io_error(dir, e))?;
    if request.reuse
        && let Some(outcome) = reuse(request)?
    {
        return Ok(ToolResult::Done(outcome));
    }
    // A stale report must never describe a run that fails half way.
    let report_path = dir.join("conversion.json");
    let _ = std::fs::remove_file(&report_path);
    let script = write_cad_helpers(dir)?;
    let tags = tags_json(dir, request.tags)?;
    let mut arguments: Vec<OsString> = vec![
        script.into_os_string(),
        "convert".into(),
        request.step.as_os_str().to_owned(),
        "--shape-unit-m".into(),
        format!("{:e}", request.shape_unit_m).into(),
        "--tags".into(),
        tags.into_os_string(),
        "--out-dir".into(),
        dir.as_os_str().to_owned(),
        "--chord-cm".into(),
        format!("{:e}", request.chord_cm).into(),
        "--angular".into(),
        format!("{:e}", request.angular_rad).into(),
        "--imprint-threads".into(),
        request.imprint_threads.to_string().into(),
        "--report".into(),
        report_path.clone().into_os_string(),
    ];
    arguments.extend(shape_cache_arguments(request.shape_cache));
    match launch_python(
        python,
        arguments,
        dir,
        CONVERT_TIMEOUT,
        "the conversion",
        ext.cancellation,
        job_environment(dir)?,
        cad_limits(),
    )? {
        Launched::Succeeded(_) => {}
        Launched::Failed(text) => {
            return Ok(ToolResult::Failed {
                why: format!("{text}{}", log_tail(&dir.join("convert.log"))),
                next_step: "Read the conversion log in the work directory. Overlapping solids, \
                            faces that cannot be meshed and out-of-memory are the usual causes; \
                            fix the model in the CAD tool and check again."
                    .into(),
            });
        }
    }
    let report: ConversionReport = read_json(&report_path, "the conversion report")?;
    let (facets, manifest) = load_pair(dir, &report.facets.bin, &report.facets.json)?;
    let facets_bin = dir.join(&report.facets.bin);
    Ok(ToolResult::Done(ConvertOutcome {
        report,
        facets,
        manifest,
        facets_bin,
        reused: false,
        helper_sha256: sha256_hex(CAD_HELPER.as_bytes()),
    }))
}

#[derive(Deserialize)]
struct TessellationReport {
    #[serde(default)]
    problems: Vec<String>,
    timing_seconds: std::collections::BTreeMap<String, f64>,
    facets: super::geometry::ConversionFacets,
}

fn tessellation_key(request: &TessellateRequest, reader: &ReaderStamp) -> CacheKey {
    let mut tags = request.tags.to_vec();
    tags.sort_by_key(|e| e.step_index);
    CacheKey {
        command: "tessellate".into(),
        step_sha256: request.step_sha256.into(),
        chord_cm: request.chord_cm,
        angular_rad: request.angular_rad,
        shape_unit_m: request.shape_unit_m,
        helper: reader.identity(),
        tags,
        pairs: Vec::new(),
    }
}

fn load_tessellation(
    dir: &Path,
    reused: bool,
    reader: &ReaderStamp,
) -> Result<Option<TessellateOutcome>, DesignError> {
    let report_path = dir.join("tessellation.json");
    let Ok(report) = read_json::<TessellationReport>(&report_path, "the tessellation report")
    else {
        return Ok(None);
    };
    let Ok((facets, manifest)) = load_pair(dir, &report.facets.bin, &report.facets.json) else {
        return Ok(None);
    };
    if !manifest
        .binary_sha256
        .eq_ignore_ascii_case(&report.facets.bin_sha256)
    {
        return Ok(None);
    }
    Ok(Some(TessellateOutcome {
        facets,
        manifest,
        problems: report.problems,
        seconds: report.timing_seconds.get("total").copied().unwrap_or(0.0),
        reused,
        reader: reader.clone(),
    }))
}

pub(super) fn tessellate(
    ext: &ExternalBackends,
    request: &TessellateRequest,
) -> Result<ToolResult<TessellateOutcome>, DesignError> {
    let reader = ext.reader()?;
    let python = match reader {
        ActiveReader::Python(_) => {
            let Some(python) = ext.tools.cad_python.as_deref() else {
                return Ok(missing_python("the per-solid tessellation", "CAD"));
            };
            Some(python)
        }
        ActiveReader::Native(_) => None,
    };
    let stamp = reader.stamp();
    let dir = request.directory;
    std::fs::create_dir_all(dir).map_err(|e| io_error(dir, e))?;
    let key = tessellation_key(request, stamp);
    if request.reuse
        && CacheKey::recorded(dir).is_some_and(|k| k.same(&key))
        && let Some(outcome) = load_tessellation(dir, true, stamp)?
    {
        return Ok(ToolResult::Done(outcome));
    }
    // A stale key must never describe a run that fails half way.
    let _ = std::fs::remove_file(CacheKey::path(dir));
    let report_path = dir.join("tessellation.json");
    let _ = std::fs::remove_file(&report_path);
    match (reader, python) {
        (ActiveReader::Native(native), _) => {
            let mut tags: Vec<Tag> = request
                .tags
                .iter()
                .map(|e| Tag {
                    step_index: e.step_index,
                    tag: e.tag.clone(),
                })
                .collect();
            tags.sort_by_key(|t| t.step_index);
            let operation = Operation::Tessellate {
                step: request.step.to_path_buf(),
                out_base: dir.join("solids"),
                chord_cm: request.chord_cm,
                angular: request.angular_rad,
                shape_unit_m: request.shape_unit_m,
                tags: Some(tags),
            };
            let value = match native.worker.run_cancellable(
                operation,
                native_bounds(TESSELLATE_TIMEOUT, cad_limits()),
                &|| ext.cancellation.is_cancelled(),
            ) {
                Ok(value) => value,
                Err(error) => {
                    return worker_failure(
                        error,
                        "A solid could not be meshed on its own; repair it in the CAD tool \
                         and check again, or check with --cad-reader python.",
                    );
                }
            };
            let body =
                serde_json::to_vec_pretty(&value).map_err(|e| DesignError::Tool(e.to_string()))?;
            std::fs::write(&report_path, body).map_err(|e| io_error(&report_path, e))?;
        }
        (ActiveReader::Python(_), Some(python)) => {
            let script = write_cad_helpers(dir)?;
            let tags = tags_json(dir, request.tags)?;
            let mut arguments: Vec<OsString> = vec![
                script.into_os_string(),
                "tessellate".into(),
                request.step.as_os_str().to_owned(),
                "--shape-unit-m".into(),
                format!("{:e}", request.shape_unit_m).into(),
                "--tags".into(),
                tags.into_os_string(),
                "--out-dir".into(),
                dir.as_os_str().to_owned(),
                "--chord-cm".into(),
                format!("{:e}", request.chord_cm).into(),
                "--angular".into(),
                format!("{:e}", request.angular_rad).into(),
                "--workers".into(),
                request.workers.max(1).to_string().into(),
                "--report".into(),
                report_path.clone().into_os_string(),
            ];
            arguments.extend(shape_cache_arguments(request.shape_cache));
            match launch_python(
                python,
                arguments,
                dir,
                TESSELLATE_TIMEOUT,
                "the per-solid tessellation",
                ext.cancellation,
                job_environment(dir)?,
                cad_limits(),
            )? {
                Launched::Succeeded(_) => {}
                Launched::Failed(text) => {
                    return Ok(ToolResult::Failed {
                        why: format!("{text}{}", log_tail(&dir.join("tessellate.log"))),
                        next_step: "A solid could not be meshed on its own; repair it in the \
                                    CAD tool and check again."
                            .into(),
                    });
                }
            }
        }
        (ActiveReader::Python(_), None) => {
            return Ok(missing_python("the per-solid tessellation", "CAD"));
        }
    }
    let Some(outcome) = load_tessellation(dir, false, stamp)? else {
        return Err(DesignError::Tool(format!(
            "the tessellation report in {} does not match its facets",
            dir.display()
        )));
    };
    key.record(dir)?;
    Ok(ToolResult::Done(outcome))
}

#[derive(Deserialize)]
struct BooleanReport {
    pairs: Vec<BooleanResult>,
    timing_seconds: std::collections::BTreeMap<String, f64>,
}

fn boolean_key(request: &BooleanRequest) -> CacheKey {
    CacheKey {
        command: "overlap-booleans".into(),
        step_sha256: request.step_sha256.into(),
        chord_cm: request.chord_cm,
        angular_rad: request.angular_rad,
        shape_unit_m: request.shape_unit_m,
        helper: helper_version(),
        tags: Vec::new(),
        pairs: request.pairs.iter().map(|&(a, b)| [a, b]).collect(),
    }
}

fn load_booleans(dir: &Path, reused: bool) -> Result<Option<BooleanOutcome>, DesignError> {
    let Ok(report) = read_json::<BooleanReport>(&dir.join("booleans.json"), "the boolean report")
    else {
        return Ok(None);
    };
    Ok(Some(BooleanOutcome {
        results: report.pairs,
        seconds: report.timing_seconds.get("total").copied().unwrap_or(0.0),
        reused,
    }))
}

pub(super) fn overlap_booleans(
    ext: &ExternalBackends,
    request: &BooleanRequest,
) -> Result<ToolResult<BooleanOutcome>, DesignError> {
    let Some(python) = ext.tools.cad_python.as_deref() else {
        return Ok(missing_python("the overlap booleans", "CAD"));
    };
    let dir = request.directory;
    std::fs::create_dir_all(dir).map_err(|e| io_error(dir, e))?;
    let key = boolean_key(request);
    if request.reuse
        && CacheKey::recorded(dir).is_some_and(|k| k.same(&key))
        && let Some(outcome) = load_booleans(dir, true)?
    {
        return Ok(ToolResult::Done(outcome));
    }
    let _ = std::fs::remove_file(CacheKey::path(dir));
    let script = write_cad_helpers(dir)?;
    let pairs_path = dir.join("pairs.json");
    write_text(
        &pairs_path,
        &serde_json::to_string(&key.pairs).map_err(|e| DesignError::Tool(e.to_string()))?,
    )?;
    let report_path = dir.join("booleans.json");
    let _ = std::fs::remove_file(&report_path);
    let mut arguments: Vec<OsString> = vec![
        script.into_os_string(),
        "overlap-booleans".into(),
        request.step.as_os_str().to_owned(),
        "--shape-unit-m".into(),
        format!("{:e}", request.shape_unit_m).into(),
        "--pairs".into(),
        pairs_path.into_os_string(),
        "--workers".into(),
        request.workers.to_string().into(),
        "--report".into(),
        report_path.clone().into_os_string(),
    ];
    arguments.extend(shape_cache_arguments(request.shape_cache));
    match launch_python(
        python,
        arguments,
        dir,
        BOOLEAN_TIMEOUT,
        "the overlap booleans",
        ext.cancellation,
        job_environment(dir)?,
        cad_limits(),
    )? {
        Launched::Succeeded(_) => {}
        Launched::Failed(text) => {
            return Ok(ToolResult::Failed {
                why: text,
                next_step: "Check the pairs of solids that touch or lie close together in the \
                            CAD tool, then check again."
                    .into(),
            });
        }
    }
    let Some(outcome) = load_booleans(dir, false)? else {
        return Err(DesignError::Tool(format!(
            "the boolean report in {} could not be read",
            dir.display()
        )));
    };
    // A failed or timed-out boolean may succeed on another run, so only decided results are kept.
    if outcome.results.iter().all(|r| r.status == "decided") {
        key.record(dir)?;
    }
    Ok(ToolResult::Done(outcome))
}

#[derive(Deserialize)]
struct DeviationReport {
    method: String,
    solids: Vec<DeviationRow>,
    timing_seconds: std::collections::BTreeMap<String, f64>,
}

pub(super) fn deviation(
    ext: &ExternalBackends,
    request: &DeviationRequest,
) -> Result<ToolResult<DeviationOutcome>, DesignError> {
    let Some(python) = ext.tools.cad_python.as_deref() else {
        return Ok(missing_python("the faceting deviation", "CAD"));
    };
    let dir = request.directory;
    std::fs::create_dir_all(dir).map_err(|e| io_error(dir, e))?;
    let script = write_cad_helpers(dir)?;
    let map_path = dir.join("map.json");
    let map: Vec<serde_json::Value> = request
        .map
        .iter()
        .map(
            |&(volume, step_index)| serde_json::json!({"volume": volume, "step_index": step_index}),
        )
        .collect();
    write_text(
        &map_path,
        &serde_json::to_string(&map).map_err(|e| DesignError::Tool(e.to_string()))?,
    )?;
    let report_path = dir.join("deviation.json");
    let mut arguments: Vec<OsString> = vec![
        script.into_os_string(),
        "deviation".into(),
        request.step.as_os_str().to_owned(),
        "--shape-unit-m".into(),
        format!("{:e}", request.shape_unit_m).into(),
        "--facets".into(),
        request.facets_bin.as_os_str().to_owned(),
        "--map".into(),
        map_path.into_os_string(),
        "--workers".into(),
        request.workers.to_string().into(),
        "--occt-points".into(),
        request.occt_points.to_string().into(),
        "--report".into(),
        report_path.clone().into_os_string(),
    ];
    arguments.extend(shape_cache_arguments(request.shape_cache));
    match launch_python(
        python,
        arguments,
        dir,
        DEVIATION_TIMEOUT,
        "the faceting deviation",
        ext.cancellation,
        job_environment(dir)?,
        cad_limits(),
    )? {
        Launched::Succeeded(_) => {}
        Launched::Failed(text) => {
            return Ok(ToolResult::Failed {
                why: text,
                next_step: "Check that the STEP file and the converted model belong together, \
                            then check again."
                    .into(),
            });
        }
    }
    let report: DeviationReport = read_json(&report_path, "the deviation report")?;
    Ok(ToolResult::Done(DeviationOutcome {
        rows: report.solids,
        seconds: report.timing_seconds.get("total").copied().unwrap_or(0.0),
        method: report.method,
    }))
}

#[derive(Deserialize)]
struct SitesReport {
    count: usize,
    bin_sha256: String,
    versions: serde_json::Value,
    #[serde(default)]
    timing_seconds: std::collections::BTreeMap<String, f64>,
}

pub(super) fn source_sites(
    ext: &ExternalBackends,
    request: &SourceRequest,
) -> Result<ToolResult<SourceOutcome>, DesignError> {
    let Some(python) = ext.tools.openmc_python.as_deref() else {
        return Ok(missing_python("the source sampler", "OpenMC"));
    };
    let Some(package_path) = ext.tools.plasma_source_path.as_deref() else {
        return Ok(ToolResult::Unavailable {
            why: "no path to openmc_plasma_source is configured, so source sites cannot be sampled"
                .into(),
            next_step: "Pass --plasma-source-path DIR (or set FARIS_PLASMA_SOURCE_PATH) to the \
                        directory holding the openmc_plasma_source package, then check again."
                .into(),
        });
    };
    let dir = request.directory;
    std::fs::create_dir_all(dir).map_err(|e| io_error(dir, e))?;
    let script = dir.join("source_sites.py");
    write_text(&script, SOURCE_SITES_HELPER)?;
    let body = serde_json::json!({
        "plasma": request.plasma,
        "count": request.count,
        "seed": request.seed,
    });
    let request_path = dir.join("request.json");
    write_text(&request_path, &body.to_string())?;
    let out_base = dir.join("sites");
    let arguments: Vec<OsString> = vec![
        script.into_os_string(),
        "--request".into(),
        request_path.into_os_string(),
        "--out-base".into(),
        out_base.clone().into_os_string(),
    ];
    let mut env = job_environment(dir)?;
    // OpenMC initialises MPI, which refuses to start without a home directory.
    env.push(("HOME".into(), dir.as_os_str().to_owned()));
    env.push(("PYTHONPATH".into(), package_path.as_os_str().to_owned()));
    match launch_python(
        python,
        arguments,
        dir,
        SOURCE_TIMEOUT.max(Duration::from_secs(60)),
        "the source sampler",
        ext.cancellation,
        env,
        ResourceLimits::default(),
    )? {
        Launched::Succeeded(_) => {}
        Launched::Failed(text) => {
            return Ok(ToolResult::Failed {
                why: text,
                next_step: "Check that --openmc-python has openmc and that --plasma-source-path \
                            holds openmc_plasma_source, and that the plasma block is valid."
                    .into(),
            });
        }
    }
    let report: SitesReport = read_json(&dir.join("sites.json"), "the source sites report")?;
    let bin_path = dir.join("sites.bin");
    let bytes = std::fs::read(&bin_path).map_err(|e| io_error(&bin_path, e))?;
    if !sha256_hex(&bytes).eq_ignore_ascii_case(&report.bin_sha256)
        || bytes.len() != 24 * report.count
    {
        return Err(DesignError::Tool(format!(
            "{} does not match its report (hash or size)",
            bin_path.display()
        )));
    }
    let points: Vec<[f64; 3]> = bytes
        .as_chunks::<24>()
        .0
        .iter()
        .map(|c| {
            let f = |k: usize| f64::from_le_bytes(c[8 * k..8 * k + 8].try_into().unwrap());
            [f(0), f(1), f(2)]
        })
        .collect();
    Ok(ToolResult::Done(SourceOutcome {
        points,
        versions: report.versions,
        bin_sha256: report.bin_sha256,
        seconds: report.timing_seconds.values().sum(),
    }))
}

#[cfg(test)]
mod tests {
    use super::super::CadReader;
    use super::super::geometry::{
        ConversionFacets, ConversionFile, ConversionStep, ConversionTolerances, TagEntry,
    };
    use super::super::mesh::fixtures::shell_and_core;
    use super::super::testutil::manifest_of;
    use super::*;

    const STEP_SHA: &str = "ab";

    struct Recorded {
        dir: tempfile::TempDir,
        tags: Vec<TagEntry>,
    }

    impl Recorded {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let f = shell_and_core(10.0, 2.0);
            let mut manifest = manifest_of(&f, &["mat:shell", "mat:core"]);
            std::fs::write(dir.path().join("model.h5m"), b"stand-in h5m").unwrap();
            manifest.source.sha256 = sha256_hex(b"stand-in h5m");
            std::fs::write(dir.path().join("model.facets.bin"), f.to_bytes()).unwrap();
            std::fs::write(
                dir.path().join("model.facets.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            let tags = vec![
                TagEntry {
                    step_index: 0,
                    tag: "shell".into(),
                },
                TagEntry {
                    step_index: 1,
                    tag: "core".into(),
                },
            ];
            let report = ConversionReport {
                schema: "faris-conversion/v1".into(),
                step: ConversionStep {
                    file_name: "m.step".into(),
                    sha256: STEP_SHA.into(),
                },
                tolerances: ConversionTolerances {
                    chord_cm: 0.1,
                    angular_rad: 0.1,
                    scale_factor_to_cm: 0.1,
                    imprint_threads: 1,
                    implicit_complement_tag: "void".into(),
                },
                tags: tags.clone(),
                versions: serde_json::json!({}),
                timing_seconds: Default::default(),
                peak_rss_mb: 1.0,
                h5m: ConversionFile {
                    file: "model.h5m".into(),
                    sha256: sha256_hex(b"stand-in h5m"),
                    bytes: 12,
                },
                facets: ConversionFacets {
                    bin: "model.facets.bin".into(),
                    json: "model.facets.json".into(),
                    bin_sha256: manifest.binary_sha256.clone(),
                },
                counts: serde_json::json!({}),
                log_tail: Vec::new(),
            };
            std::fs::write(
                dir.path().join("conversion.json"),
                serde_json::to_vec(&report).unwrap(),
            )
            .unwrap();
            Self { dir, tags }
        }

        fn request<'a>(&'a self, chord_cm: f64, step_sha: &'a str) -> ConvertRequest<'a> {
            ConvertRequest {
                step: Path::new("m.step"),
                step_sha256: step_sha,
                shape_unit_m: 1.0e-3,
                tags: &self.tags,
                chord_cm,
                angular_rad: 0.1,
                imprint_threads: 1,
                directory: self.dir.path(),
                reuse: true,
                shape_cache: None,
            }
        }
    }

    // Verifies: GEO-013
    #[test]
    fn a_recorded_conversion_is_reused_only_when_hashes_and_tolerances_match() {
        let r = Recorded::new();
        assert!(reuse(&r.request(0.1, STEP_SHA)).unwrap().unwrap().reused);
        assert!(
            reuse(&r.request(0.1, "AB")).unwrap().is_some(),
            "hex case does not matter"
        );
        assert!(
            reuse(&r.request(0.05, STEP_SHA)).unwrap().is_none(),
            "other chord"
        );
        assert!(
            reuse(&r.request(0.1, "cd")).unwrap().is_none(),
            "other STEP file"
        );
        let mut other_tags = r.request(0.1, STEP_SHA);
        let swapped = vec![
            TagEntry {
                step_index: 0,
                tag: "core".into(),
            },
            TagEntry {
                step_index: 1,
                tag: "shell".into(),
            },
        ];
        other_tags.tags = &swapped;
        assert!(reuse(&other_tags).unwrap().is_none(), "other tags");
        let mut other_unit = r.request(0.1, STEP_SHA);
        other_unit.shape_unit_m = 1.0e-2;
        assert!(reuse(&other_unit).unwrap().is_none(), "other scale");
    }

    // Verifies: GEO-013
    #[test]
    fn a_changed_product_is_never_reused() {
        let r = Recorded::new();
        std::fs::write(r.dir.path().join("model.h5m"), b"edited").unwrap();
        assert!(
            reuse(&r.request(0.1, STEP_SHA)).unwrap().is_none(),
            "h5m edited"
        );
        let r = Recorded::new();
        let mut bytes = std::fs::read(r.dir.path().join("model.facets.bin")).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        std::fs::write(r.dir.path().join("model.facets.bin"), bytes).unwrap();
        assert!(
            reuse(&r.request(0.1, STEP_SHA)).unwrap().is_none(),
            "facets edited"
        );
        let r = Recorded::new();
        std::fs::remove_file(r.dir.path().join("conversion.json")).unwrap();
        assert!(
            reuse(&r.request(0.1, STEP_SHA)).unwrap().is_none(),
            "no record"
        );
    }

    fn fake_tools() -> super::super::ToolPaths {
        // An interpreter that does not exist: a request that is not served from the record fails.
        super::super::ToolPaths {
            cad_python: Some(PathBuf::from("/nonexistent/faris-test-python")),
            cad_reader: Some(CadReader::Python),
            ..Default::default()
        }
    }

    fn tessellation_request<'a>(
        dir: &'a Path,
        tags: &'a [TagEntry],
        chord_cm: f64,
        sha: &'a str,
        reuse: bool,
    ) -> TessellateRequest<'a> {
        TessellateRequest {
            step: Path::new("m.step"),
            step_sha256: sha,
            shape_unit_m: 1.0e-3,
            tags,
            chord_cm,
            angular_rad: 0.1,
            directory: dir,
            workers: 2,
            reuse,
            shape_cache: None,
        }
    }

    /// A tessellation as the helper leaves it, with the key `tessellate` writes after a run.
    fn recorded_tessellation(r: &Recorded, helper: &str) {
        let dir = r.dir.path();
        let bytes = std::fs::read(dir.join("model.facets.bin")).unwrap();
        let manifest = std::fs::read(dir.join("model.facets.json")).unwrap();
        std::fs::write(dir.join("solids.facets.bin"), bytes).unwrap();
        std::fs::write(dir.join("solids.facets.json"), &manifest).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
        std::fs::write(
            dir.join("tessellation.json"),
            serde_json::to_vec(&serde_json::json!({
                "problems": ["solid 1 (core): the tessellation is not closed: 3 edge(s)"],
                "timing_seconds": {"total": 12.5},
                "facets": {
                    "bin": "solids.facets.bin",
                    "json": "solids.facets.json",
                    "bin_sha256": parsed["binary_sha256"],
                },
            }))
            .unwrap(),
        )
        .unwrap();
        let key = CacheKey {
            command: "tessellate".into(),
            step_sha256: STEP_SHA.into(),
            chord_cm: 0.1,
            angular_rad: 0.1,
            shape_unit_m: 1.0e-3,
            helper: helper.into(),
            tags: r.tags.clone(),
            pairs: Vec::new(),
        };
        key.record(dir).unwrap();
    }

    // Verifies: GEO-013
    #[test]
    fn a_recorded_tessellation_is_reused_only_under_the_same_key() {
        let tools = fake_tools();
        let cancel = crate::jobs::Cancellation::default();
        let ext = ExternalBackends::new(&tools, &cancel);
        let r = Recorded::new();
        recorded_tessellation(&r, &helper_version());
        let dir = r.dir.path();
        let ToolResult::Done(out) = tessellate(
            &ext,
            &tessellation_request(dir, &r.tags, 0.1, STEP_SHA, true),
        )
        .unwrap() else {
            panic!("expected the recorded tessellation");
        };
        assert!(out.reused);
        assert_eq!(out.seconds, 12.5, "the recorded time is reported");
        assert_eq!(
            out.problems.len(),
            1,
            "a recorded problem is reported again"
        );
        // anything else in the key, or no reuse, runs the helper: here that fails for want of Python
        let served = |request: TessellateRequest| {
            matches!(tessellate(&ext, &request), Ok(ToolResult::Done(_)))
        };
        assert!(
            !served(tessellation_request(dir, &r.tags, 0.05, STEP_SHA, true)),
            "other chord"
        );
        assert!(
            !served(tessellation_request(dir, &r.tags, 0.1, "cd", true)),
            "other STEP file"
        );
        assert!(
            !served(tessellation_request(dir, &r.tags, 0.1, STEP_SHA, false)),
            "reuse off"
        );
    }

    // Verifies: GEO-013
    #[test]
    fn a_tessellation_of_another_helper_version_is_not_reused() {
        let tools = fake_tools();
        let cancel = crate::jobs::Cancellation::default();
        let ext = ExternalBackends::new(&tools, &cancel);
        let r = Recorded::new();
        recorded_tessellation(&r, "0");
        let request = tessellation_request(r.dir.path(), &r.tags, 0.1, STEP_SHA, true);
        assert!(!matches!(
            tessellate(&ext, &request),
            Ok(ToolResult::Done(_))
        ));
    }

    #[cfg(unix)]
    fn native_tools(worker: &faris_occt_client::client::Worker) -> super::super::ToolPaths {
        super::super::ToolPaths {
            cad_reader: Some(CadReader::Native),
            occt_worker: Some(worker.program().to_path_buf()),
            ..Default::default()
        }
    }

    #[cfg(unix)]
    fn native_identity(worker: &faris_occt_client::client::Worker) -> String {
        format!("native:{}:V7_9_3", sha256_file(worker.program()).unwrap())
    }

    #[cfg(unix)]
    #[test]
    fn a_native_tessellation_is_never_reused_for_python_or_for_another_worker() {
        use super::super::testutil::stub_worker;
        let cancel = crate::jobs::Cancellation::default();
        let bins = tempfile::tempdir().unwrap();
        // Workers that answer a ping and fail every other request, so a request that is not
        // served from the record fails.
        let worker_a = stub_worker(bins.path(), "a", "V7_9_3", "exit 5");
        let worker_b = stub_worker(bins.path(), "b", "V7_9_3", "exit 6");
        let worker_a_other_occt = stub_worker(bins.path(), "a2", "V7_9_4", "exit 5");
        let python = fake_tools();
        let (tools_a, tools_b, tools_a2) = (
            native_tools(&worker_a),
            native_tools(&worker_b),
            native_tools(&worker_a_other_occt),
        );
        let served = |tools: &super::super::ToolPaths, r: &Recorded| {
            let ext = ExternalBackends::new(tools, &cancel);
            let request = tessellation_request(r.dir.path(), &r.tags, 0.1, STEP_SHA, true);
            match tessellate(&ext, &request) {
                Ok(ToolResult::Done(out)) => Some(out),
                _ => None,
            }
        };

        let recorded_by_a = || {
            let r = Recorded::new();
            recorded_tessellation(&r, &native_identity(&worker_a));
            r
        };
        let out = served(&tools_a, &recorded_by_a()).expect("the same worker reuses its result");
        assert!(out.reused && out.reader.is_native());
        assert_eq!(out.reader.occt_tag.as_deref(), Some("V7_9_3"));
        // a run that does not match clears the record, so each question gets its own
        assert!(
            served(&python, &recorded_by_a()).is_none(),
            "python after native"
        );
        assert!(
            served(&tools_b, &recorded_by_a()).is_none(),
            "another worker binary"
        );
        assert!(
            served(&tools_a2, &recorded_by_a()).is_none(),
            "another OpenCASCADE tag"
        );

        let helper = Recorded::new();
        recorded_tessellation(&helper, &helper_version());
        let out = served(&python, &helper).expect("python reuses its own result");
        assert!(out.reused && !out.reader.is_native());
        let helper = Recorded::new();
        recorded_tessellation(&helper, &helper_version());
        assert!(served(&tools_a, &helper).is_none(), "native after python");
    }

    #[cfg(unix)]
    #[test]
    fn the_native_worker_tessellates_and_the_result_is_recorded_under_its_identity() {
        use super::super::testutil::stub_worker;
        let source = Recorded::new();
        recorded_tessellation(&source, "unused");
        let src = source.dir.path().display();
        let bins = tempfile::tempdir().unwrap();
        let worker = stub_worker(
            bins.path(),
            "w",
            "V7_9_3",
            &format!(
                "grep -q '\"tag\":\"core\"' \"$1\" || exit 4; \
                 out=$(sed 's/.*\"out_base\":\"\\([^\"]*\\)\".*/\\1/' \"$1\"); \
                 cp {src}/solids.facets.bin \"$out.facets.bin\"; \
                 cp {src}/solids.facets.json \"$out.facets.json\"; \
                 printf '{{\"schema\":\"faris-occt-response/v1\",\"status\":\"ok\",\"result\":' > \"$2\"; \
                 cat {src}/tessellation.json >> \"$2\"; printf '}}' >> \"$2\""
            ),
        );
        let tools = native_tools(&worker);
        let cancel = crate::jobs::Cancellation::default();
        let ext = ExternalBackends::new(&tools, &cancel);
        let run = tempfile::tempdir().unwrap();
        let request = |reuse| tessellation_request(run.path(), &source.tags, 0.1, STEP_SHA, reuse);
        let ToolResult::Done(first) = tessellate(&ext, &request(false)).unwrap() else {
            panic!("the worker's tessellation was not accepted");
        };
        assert!(!first.reused && first.reader.is_native());
        assert_eq!(
            first.problems.len(),
            1,
            "the worker's problems are reported"
        );
        assert_eq!(
            CacheKey::recorded(run.path()).unwrap().helper,
            native_identity(&worker)
        );
        let ToolResult::Done(again) = tessellate(&ext, &request(true)).unwrap() else {
            panic!("the recorded tessellation was not reused");
        };
        assert!(again.reused && again.reader == first.reader);
    }

    #[cfg(unix)]
    #[test]
    fn a_native_worker_that_fails_a_tessellation_leaves_a_failed_stage_and_no_key() {
        use super::super::testutil::stub_worker;
        let bins = tempfile::tempdir().unwrap();
        let worker = stub_worker(
            bins.path(),
            "w",
            "V7_9_3",
            "printf '{\"schema\":\"faris-occt-response/v1\",\"status\":\"error\",\"message\":\"solid 1: no faces\"}' > \"$2\"; exit 1",
        );
        let tools = native_tools(&worker);
        let cancel = crate::jobs::Cancellation::default();
        let ext = ExternalBackends::new(&tools, &cancel);
        let r = Recorded::new();
        recorded_tessellation(&r, &native_identity(&worker));
        let request = tessellation_request(r.dir.path(), &r.tags, 0.1, STEP_SHA, false);
        let ToolResult::Failed { why, next_step } = tessellate(&ext, &request).unwrap() else {
            panic!("expected a failed stage");
        };
        assert!(why.contains("solid 1: no faces"), "{why}");
        assert!(next_step.contains("--cad-reader python"), "{next_step}");
        assert!(CacheKey::recorded(r.dir.path()).is_none());
    }

    #[test]
    fn the_helper_version_is_read_from_the_helper() {
        let v = helper_version();
        assert!(v.parse::<u32>().is_ok(), "{v}");
    }

    // Verifies: GEO-013
    #[test]
    fn recorded_boolean_verdicts_are_reused_only_for_the_same_pairs_and_key() {
        let tools = fake_tools();
        let cancel = crate::jobs::Cancellation::default();
        let ext = ExternalBackends::new(&tools, &cancel);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("booleans.json"),
            serde_json::to_vec(&serde_json::json!({
                "pairs": [{"a": 0, "b": 1, "status": "decided", "common_volume_m3": 0.0}],
                "timing_seconds": {"total": 7.0},
            }))
            .unwrap(),
        )
        .unwrap();
        let pairs = [(0usize, 1usize)];
        let request = |pairs: &'static [(usize, usize)], chord_cm: f64, reuse: bool| {
            let dir_path: &'static Path = Box::leak(dir.path().to_path_buf().into_boxed_path());
            BooleanRequest {
                step: Path::new("m.step"),
                step_sha256: STEP_SHA,
                shape_unit_m: 1.0e-3,
                pairs,
                chord_cm,
                angular_rad: 0.1,
                directory: dir_path,
                workers: 2,
                reuse,
                shape_cache: None,
            }
        };
        let pairs: &'static [(usize, usize)] = Box::leak(Box::new(pairs));
        boolean_key(&request(pairs, 0.1, true))
            .record(dir.path())
            .unwrap();
        let ToolResult::Done(out) = overlap_booleans(&ext, &request(pairs, 0.1, true)).unwrap()
        else {
            panic!("expected the recorded verdicts");
        };
        assert!(out.reused);
        assert_eq!(out.seconds, 7.0);
        assert_eq!(out.results.len(), 1);
        let served =
            |r: BooleanRequest| matches!(overlap_booleans(&ext, &r), Ok(ToolResult::Done(_)));
        let other_pairs: &'static [(usize, usize)] = Box::leak(Box::new([(0usize, 2usize)]));
        assert!(!served(request(other_pairs, 0.1, true)), "other pairs");
        assert!(!served(request(pairs, 0.2, true)), "other tolerance");
        assert!(!served(request(pairs, 0.1, false)), "reuse off");
    }

    #[test]
    fn a_tool_that_is_not_configured_says_what_to_pass() {
        let tools = super::super::ToolPaths {
            cad_reader: Some(CadReader::Python),
            ..Default::default()
        };
        let cancel = crate::jobs::Cancellation::default();
        let ext = ExternalBackends::new(&tools, &cancel);
        let r = Recorded::new();
        let ToolResult::Unavailable { why, next_step } =
            convert(&ext, &r.request(0.1, STEP_SHA)).unwrap()
        else {
            panic!("expected Unavailable");
        };
        assert!(why.contains("no CAD interpreter"));
        assert!(next_step.contains("--cad-python"));
    }
}
