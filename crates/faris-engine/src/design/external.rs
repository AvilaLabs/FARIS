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
use super::{
    BOOLEAN_TIMEOUT, CAD_ADDRESS_SPACE, CAD_FACETS_HELPER, CAD_HELPER, CONVERT_TIMEOUT,
    DEVIATION_TIMEOUT, DesignError, ExternalBackends, Launched, SOURCE_SITES_HELPER,
    SOURCE_TIMEOUT, TESSELLATE_TIMEOUT, io_error, launch_python, read_json_bounded, sha256_file,
    sha256_hex, tail,
};
use crate::jobs::ResourceLimits;
use faris_model::design::load_facets;
use serde::Deserialize;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
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

fn write_text(path: &Path, text: &str) -> Result<(), DesignError> {
    std::fs::write(path, text).map_err(|e| io_error(path, e))
}

/// Write the embedded CAD helpers beside the job; returns the script to run.
fn write_cad_helpers(dir: &Path) -> Result<PathBuf, DesignError> {
    let helpers = dir.join("helpers");
    std::fs::create_dir_all(&helpers).map_err(|e| io_error(&helpers, e))?;
    write_text(&helpers.join("faris_facets.py"), CAD_FACETS_HELPER)?;
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
    let arguments: Vec<OsString> = vec![
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

pub(super) fn tessellate(
    ext: &ExternalBackends,
    request: &TessellateRequest,
) -> Result<ToolResult<TessellateOutcome>, DesignError> {
    let Some(python) = ext.tools.cad_python.as_deref() else {
        return Ok(missing_python("the per-solid tessellation", "CAD"));
    };
    let dir = request.directory;
    std::fs::create_dir_all(dir).map_err(|e| io_error(dir, e))?;
    let script = write_cad_helpers(dir)?;
    let tags = tags_json(dir, request.tags)?;
    let report_path = dir.join("tessellation.json");
    let arguments: Vec<OsString> = vec![
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
        "--report".into(),
        report_path.clone().into_os_string(),
    ];
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
                next_step: "A solid could not be meshed on its own; repair it in the CAD tool \
                            and check again."
                    .into(),
            });
        }
    }
    let report: TessellationReport = read_json(&report_path, "the tessellation report")?;
    let (facets, manifest) = load_pair(dir, &report.facets.bin, &report.facets.json)?;
    Ok(ToolResult::Done(TessellateOutcome {
        facets,
        manifest,
        problems: report.problems,
        seconds: report.timing_seconds.get("total").copied().unwrap_or(0.0),
    }))
}

#[derive(Deserialize)]
struct BooleanReport {
    pairs: Vec<BooleanResult>,
    timing_seconds: std::collections::BTreeMap<String, f64>,
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
    let script = write_cad_helpers(dir)?;
    let pairs_path = dir.join("pairs.json");
    let pairs: Vec<[usize; 2]> = request.pairs.iter().map(|&(a, b)| [a, b]).collect();
    write_text(
        &pairs_path,
        &serde_json::to_string(&pairs).map_err(|e| DesignError::Tool(e.to_string()))?,
    )?;
    let report_path = dir.join("booleans.json");
    let arguments: Vec<OsString> = vec![
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
    let report: BooleanReport = read_json(&report_path, "the boolean report")?;
    Ok(ToolResult::Done(BooleanOutcome {
        results: report.pairs,
        seconds: report.timing_seconds.get("total").copied().unwrap_or(0.0),
    }))
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
    let arguments: Vec<OsString> = vec![
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
        "--report".into(),
        report_path.clone().into_os_string(),
    ];
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

    #[test]
    fn a_tool_that_is_not_configured_says_what_to_pass() {
        let tools = super::super::ToolPaths::default();
        let cancel = crate::jobs::Cancellation::default();
        let ext = ExternalBackends {
            tools: &tools,
            cancellation: &cancel,
        };
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
