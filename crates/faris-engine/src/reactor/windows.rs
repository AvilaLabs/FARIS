//! Weight windows for a target-error reactor run (the Production tier).
//!
//! The magnet fast flux is the hardest response of the study to sample: a few neutrons in a million reach the
//! outer coil layer above 0.1 MeV. A helper run with OpenMC 0.16 (`reactor_windows.py`, on the generator that
//! `faris design transport` also uses) solves the adjoint problem of that response by random ray and writes
//! weight windows. The run then splits and roulettes on them; the windows change the variance, never the
//! mean. They are made once per model, data and response and kept in the transport cache.
//!
//! Without an OpenMC 0.16 Python, or when the helper fails, the run goes on analog and its record says so
//! and why. A preview (no target error) is always analog and records nothing about windows.

use super::{ReactorError, ReactorJob, digest, hash_file, write_new};
use crate::jobs::{Cancellation, ExecutionStatus, JobSpec, ResourceLimits, run_job};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};

const REACTOR_WINDOWS: &str = include_str!("../../../../integrations/openmc/reactor_windows.py");
const BATCH_STATISTICS: &str = include_str!("../../../../integrations/openmc/batch_statistics.py");
const WINDOW_GENERATOR: &str = include_str!("../../../../integrations/openmc/window_generator.py");
const WINDOW_FILL_HELPER: &str =
    include_str!("../../../../integrations/openmc/risk_tests/s0f_common.py");
/// Names the recipe in the cache key; change it when the helper's method changes.
const RECIPE: &str = "reactor-windows/v1";
/// Longest the helper may take: the multigroup step and the random-ray step are each a few minutes.
const WINDOWS_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// The helper loads OpenMC and runs the multigroup condensation, which peaks above the runner's 4 GiB default.
const WINDOWS_ADDRESS_SPACE: u64 = 12 * 1024 * 1024 * 1024;
/// The folder windows are kept in, beside the runs (the design transport keeps its windows in the same one).
pub const WINDOW_CACHE_NAME: &str = "faris-transport-cache";
/// File names inside a run directory.
pub const WINDOWS_FILE: &str = "windows.h5";
pub const WINDOWS_RECORD_FILE: &str = "windows-record.json";
const KIND: &str = "weight_windows";

/// What a run needs to make or reuse windows.
#[derive(Clone, Copy, Debug)]
pub struct WindowOptions<'a> {
    /// A Python with OpenMC 0.16 or later.
    pub python: &'a Path,
    /// Where windows are kept between runs (`faris-transport-cache`); `None` makes them for this run only.
    pub cache_dir: Option<&'a Path>,
}

/// The windows a run reads: part of the input, so the hashes bind the run to them.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WeightWindowsPlan {
    pub kind: String,
    /// The response the windows are aimed at (the one the stop rule watches).
    pub response_id: String,
    /// Plain file name in the run directory.
    pub windows_file: String,
    pub windows_sha256: String,
    /// The generator's own record, kept beside the windows.
    pub record_file: String,
    pub record_sha256: String,
}

/// What the run record says about variance reduction.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VarianceReductionRecord {
    /// `weight_windows` or `analog`.
    pub mode: String,
    /// In words: how the windows came about, or why the run is analog and what to do.
    pub reason: String,
    /// Whether the windows came from the cache rather than being made for this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows_reused: Option<bool>,
    /// Seconds spent making the windows for this run; 0 when they were reused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_seconds: Option<f64>,
}

impl VarianceReductionRecord {
    pub fn uses_windows(&self) -> bool {
        self.mode == KIND
    }

    /// One plain sentence for the result view.
    pub fn summary(&self) -> String {
        self.reason.clone()
    }
}

/// Where the windows of a run stand before its input is written.
pub struct Prepared {
    pub plan: Option<WeightWindowsPlan>,
    pub record: VarianceReductionRecord,
    /// The windows and the generator record to place in the run directory.
    source: Option<(PathBuf, PathBuf)>,
    /// Scratch the helper worked in, removed once the files are placed.
    scratch: Option<PathBuf>,
}

fn analog(reason: String) -> Prepared {
    Prepared {
        plan: None,
        record: VarianceReductionRecord {
            mode: "analog".into(),
            reason,
            windows_reused: None,
            generation_seconds: None,
        },
        source: None,
        scratch: None,
    }
}

impl Prepared {
    /// Put the windows and their record in the run directory `output` (hard links when the file system allows,
    /// copies otherwise) and clear the scratch.
    pub fn install(self, output: &Path) -> Result<(), ReactorError> {
        if let Some((windows, record)) = &self.source {
            link_or_copy(windows, &output.join(WINDOWS_FILE))?;
            link_or_copy(record, &output.join(WINDOWS_RECORD_FILE))?;
        }
        if let Some(scratch) = &self.scratch {
            let _ = std::fs::remove_dir_all(scratch);
        }
        Ok(())
    }

    /// Clear the scratch of windows that were not used.
    pub fn discard(&self) {
        if let Some(scratch) = &self.scratch {
            let _ = std::fs::remove_dir_all(scratch);
        }
    }
}

fn link_or_copy(from: &Path, to: &Path) -> Result<(), ReactorError> {
    if std::fs::hard_link(from, to).is_err() {
        std::fs::copy(from, to).map_err(|error| {
            format!(
                "cannot place {} in the run directory: {error}",
                from.display()
            )
        })?;
    }
    Ok(())
}

/// The request without the field-mesh responses: the windows do not depend on the mesh preset.
fn solid_request(request: &Value) -> Value {
    let mut request = request.clone();
    if let Some(responses) = request["responses"].as_array_mut() {
        responses.retain(|r| r["domain"]["kind"] != "mesh");
    }
    request
}

/// Identity of a set of windows: the model, materials, response, data and helper code they were made from.
fn cache_key(
    base_input: &Value,
    cross_sections_sha256: &str,
    python: &Path,
    adapter: &[u8],
) -> String {
    let identity = serde_json::json!({
        "recipe": RECIPE,
        "manifest": base_input["manifest"],
        "physics": base_input["physics"],
        "request": solid_request(&base_input["request"]),
        "cross_sections": cross_sections_sha256,
        "window_python": python.display().to_string(),
        "helpers": [
            digest(REACTOR_WINDOWS.as_bytes()),
            digest(WINDOW_GENERATOR.as_bytes()),
            digest(WINDOW_FILL_HELPER.as_bytes()),
            digest(adapter),
        ],
    });
    digest(identity.to_string().as_bytes())
}

/// Windows kept by an earlier run, if the entry is whole and its file matches its record.
fn cached(entry: &Path) -> Option<(PathBuf, PathBuf)> {
    let windows = entry.join("filled.h5");
    let record = entry.join("record.json");
    let stated = serde_json::from_slice::<Value>(&std::fs::read(&record).ok()?).ok()?;
    let expected = stated.get("filled_sha256")?.as_str()?;
    (hash_file(&windows).ok()?.as_str() == expected).then_some((windows, record))
}

fn failure_reason(text: &str) -> String {
    let last = text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("no message");
    let last: String = last.chars().take(300).collect();
    format!(
        "The helper that makes the windows failed ({last}). The run is analog; it samples the magnet fast flux far more slowly. Check the OpenMC 0.16 Python and run again"
    )
}

/// Make or find the windows of a target-error run, or decide it goes on analog. `base_input` is the run's input
/// without windows. Never fails on a missing or broken helper: that is an analog run with a reason. It fails
/// only on cancellation and on files it cannot handle.
pub fn prepare(
    job: &ReactorJob<'_>,
    base_input: &Value,
    cross_sections_sha256: &str,
    cancellation: &Cancellation,
) -> Result<Option<Prepared>, ReactorError> {
    if job.sampling.target_relative_error.is_none() {
        return Ok(None);
    }
    let Some(options) = job.windows else {
        return Ok(Some(analog(
            "No OpenMC 0.16 Python was given, so no weight windows were made: the run is analog. To speed it up give one (faris reactor run --window-python, or the Window Python tool in the app)".into(),
        )));
    };
    let python = match options.python.canonicalize() {
        Ok(path) => path,
        Err(error) => {
            return Ok(Some(analog(format!(
                "The window Python {} cannot be used ({error}), so no weight windows were made: the run is analog. Give the path of a Python with OpenMC 0.16",
                options.python.display()
            ))));
        }
    };
    let key = cache_key(base_input, cross_sections_sha256, &python, job.adapter);
    let entry = options
        .cache_dir
        .map(|cache| cache.join(format!("reactor-windows-{key}")));
    if let Some((windows, record)) = entry.as_deref().and_then(cached) {
        return Ok(Some(found(windows, record, true, 0.0, None)?));
    }
    // Made in a directory beside the cache (or beside the run), moved into place whole.
    let parent = match options.cache_dir {
        Some(cache) => cache.to_path_buf(),
        None => std::path::absolute(job.output)?
            .parent()
            .ok_or("run directory has no parent")?
            .to_path_buf(),
    };
    std::fs::create_dir_all(&parent)?;
    let work = parent.join(format!(
        ".reactor-windows-{}-{}",
        &key[..16],
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir(&work)?;
    let result = generate(job, base_input, &python, &work, cancellation);
    let (seconds, kept) = match result {
        Ok(pair) => pair,
        Err(Generation::Cancelled) => {
            let _ = std::fs::remove_dir_all(&work);
            return Err("the run was cancelled while its weight windows were made".into());
        }
        Err(Generation::Failed(reason)) => {
            let _ = std::fs::remove_dir_all(&work);
            return Ok(Some(analog(reason)));
        }
        Err(Generation::Unusable(error)) => {
            let _ = std::fs::remove_dir_all(&work);
            return Err(error);
        }
    };
    let (windows, record) = (kept.join("filled.h5"), kept.join("record.json"));
    match entry {
        Some(entry) => {
            // Kept at once: a run that fails later does not cost the windows again.
            let _ = std::fs::remove_dir_all(&entry);
            std::fs::rename(&kept, &entry)?;
            let _ = std::fs::remove_dir_all(&work);
            let (windows, record) = (entry.join("filled.h5"), entry.join("record.json"));
            Ok(Some(found(windows, record, false, seconds, None)?))
        }
        None => Ok(Some(found(windows, record, false, seconds, Some(work))?)),
    }
}

fn found(
    windows: PathBuf,
    record: PathBuf,
    reused: bool,
    seconds: f64,
    scratch: Option<PathBuf>,
) -> Result<Prepared, ReactorError> {
    let plan = WeightWindowsPlan {
        kind: KIND.into(),
        response_id: super::STOP_RULE_RESPONSE_ID.into(),
        windows_file: WINDOWS_FILE.into(),
        windows_sha256: hash_file(&windows)?,
        record_file: WINDOWS_RECORD_FILE.into(),
        record_sha256: hash_file(&record)?,
    };
    let reason = if reused {
        "Weight windows aimed at the magnet fast flux, reused from an earlier run on the same model, data and response.".to_string()
    } else {
        format!(
            "Weight windows aimed at the magnet fast flux, made for this run in {seconds:.0} s."
        )
    };
    Ok(Prepared {
        plan: Some(plan),
        record: VarianceReductionRecord {
            mode: KIND.into(),
            reason,
            windows_reused: Some(reused),
            generation_seconds: Some(if reused { 0.0 } else { seconds }),
        },
        source: Some((windows, record)),
        scratch,
    })
}

enum Generation {
    Cancelled,
    /// The helper ran and did not deliver; the run goes on analog.
    Failed(String),
    /// A file the engine itself needs could not be written.
    Unusable(ReactorError),
}

/// Run the helper in `work`. On success returns the seconds it took and the directory that holds `filled.h5`
/// and `record.json`.
fn generate(
    job: &ReactorJob<'_>,
    base_input: &Value,
    python: &Path,
    work: &Path,
    cancellation: &Cancellation,
) -> Result<(f64, PathBuf), Generation> {
    let unusable = |error: std::io::Error| Generation::Unusable(error.into());
    let helpers = work.join("risk_tests");
    std::fs::create_dir(&helpers).map_err(unusable)?;
    for (path, text) in [
        (work.join("reactor_windows.py"), REACTOR_WINDOWS.as_bytes()),
        (
            work.join("window_generator.py"),
            WINDOW_GENERATOR.as_bytes(),
        ),
        (
            work.join("batch_statistics.py"),
            BATCH_STATISTICS.as_bytes(),
        ),
        (helpers.join("s0f_common.py"), WINDOW_FILL_HELPER.as_bytes()),
        (work.join("reactor_transport.py"), job.adapter),
    ] {
        write_new(&path, text).map_err(Generation::Unusable)?;
    }
    let input = work.join("input.json");
    let bytes = serde_json::to_vec(base_input).map_err(|e| Generation::Unusable(e.into()))?;
    write_new(&input, &bytes).map_err(Generation::Unusable)?;
    let tmp = work.join("tmp");
    std::fs::create_dir(&tmp).map_err(unusable)?;
    let threads = job.sampling.threads.to_string();
    let path = std::env::join_paths([
        python
            .parent()
            .unwrap_or(Path::new("/usr/bin"))
            .to_path_buf(),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ])
    .map_err(|e| Generation::Unusable(e.into()))?;
    let spec = JobSpec {
        program: python.to_path_buf(),
        arguments: vec![
            work.join("reactor_windows.py").into_os_string(),
            "--input".into(),
            input.into_os_string(),
            "--work-dir".into(),
            work.join("out").into_os_string(),
            "--threads".into(),
            OsString::from(&threads),
        ],
        working_directory: work.to_path_buf(),
        environment: vec![
            ("OMP_NUM_THREADS".into(), threads.into()),
            ("OPENBLAS_NUM_THREADS".into(), "1".into()),
            ("PYTHONDONTWRITEBYTECODE".into(), "1".into()),
            ("TMPDIR".into(), tmp.into_os_string()),
            // OpenMC initialises MPI, which refuses to start without a home directory.
            ("HOME".into(), work.as_os_str().to_owned()),
            ("PATH".into(), path),
        ],
        timeout: WINDOWS_TIMEOUT,
        capture_limit_bytes: 4 * 1024 * 1024,
        artifact_roots: vec![],
        resource_limits: ResourceLimits::with_address_space(WINDOWS_ADDRESS_SPACE),
    };
    let result = match run_job(&spec, cancellation) {
        Ok(result) => result,
        Err(error) => return Err(Generation::Failed(failure_reason(&error.to_string()))),
    };
    match result.execution_status {
        ExecutionStatus::Succeeded => {}
        ExecutionStatus::Cancelled => return Err(Generation::Cancelled),
        ExecutionStatus::TimedOut => {
            return Err(Generation::Failed(
                "The helper that makes the windows ran out of time (30 minutes). The run is analog. Try a machine with more cores, or run again".into(),
            ));
        }
        _ => return Err(Generation::Failed(failure_reason(&result.stderr))),
    }
    let made = work.join("out").join("windows");
    let seconds = std::fs::read(made.join("record.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|record| record["elapsed_seconds"].as_f64());
    let kept = work.join("kept");
    std::fs::create_dir(&kept).map_err(unusable)?;
    for name in ["filled.h5", "record.json"] {
        std::fs::rename(made.join(name), kept.join(name)).map_err(|error| {
            Generation::Failed(format!(
                "The helper that makes the windows left no {name} ({error}). The run is analog. Run again"
            ))
        })?;
    }
    // The helper's total (both steps) is in the record under its own key.
    let total = std::fs::read(kept.join("record.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|record| record["elapsed_seconds"].as_f64())
        .or(seconds)
        .unwrap_or(result.elapsed_seconds);
    Ok((total, kept))
}

/// Check a recorded run's window evidence: the plan in its input must match the files in the run directory, and
/// the worker's receipt must say the run read those windows.
pub fn verify_recorded(
    directory: &Path,
    input: &Value,
    worker: Option<&Value>,
    record: Option<&VarianceReductionRecord>,
    holds_windowed_pass: bool,
) -> Result<Option<WeightWindowsPlan>, ReactorError> {
    let plan = match input.get("variance_reduction") {
        None | Some(Value::Null) => None,
        Some(value) => Some(serde_json::from_value::<WeightWindowsPlan>(value.clone())?),
    };
    // The analog pass of a two-pass run reads no windows itself, but its record says the run used them.
    match (
        &plan,
        record.map(VarianceReductionRecord::uses_windows),
        holds_windowed_pass,
    ) {
        (Some(_), Some(true), false)
        | (None, None | Some(false), false)
        | (None, Some(true), true) => {}
        _ => return Err("the run record and its input disagree about weight windows".into()),
    }
    let Some(plan) = plan else {
        return Ok(None);
    };
    if plan.kind != KIND
        || plan.windows_file != WINDOWS_FILE
        || plan.record_file != WINDOWS_RECORD_FILE
        || plan.response_id != super::STOP_RULE_RESPONSE_ID
    {
        return Err("the recorded weight-window plan is not one this build knows".into());
    }
    if hash_file(&directory.join(&plan.windows_file))? != plan.windows_sha256
        || hash_file(&directory.join(&plan.record_file))? != plan.record_sha256
    {
        return Err("the recorded weight-window files differ from the hashes in the input".into());
    }
    if let Some(worker) = worker {
        let receipt = &worker["variance_reduction"];
        if receipt["mode"] != KIND || receipt["windows_sha256"] != plan.windows_sha256.as_str() {
            return Err("the worker did not read the recorded weight windows".into());
        }
    }
    Ok(Some(plan))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Value {
        serde_json::json!({"manifest": {"a": 1}, "physics": {"b": 2}, "request": {"c": 3}})
    }

    #[test]
    fn cache_key_follows_model_data_helpers_and_python_only() {
        let key = |input: &Value, data: &str, python: &str, adapter: &[u8]| {
            cache_key(input, data, Path::new(python), adapter)
        };
        let reference = key(&base(), "d1", "/p", b"w");
        assert_eq!(reference, key(&base(), "d1", "/p", b"w"));
        let mut other = base();
        other["request"]["c"] = 4.into();
        assert_ne!(reference, key(&other, "d1", "/p", b"w"));
        assert_ne!(reference, key(&base(), "d2", "/p", b"w"));
        assert_ne!(reference, key(&base(), "d1", "/q", b"w"));
        assert_ne!(reference, key(&base(), "d1", "/p", b"x"));
        // Sampling (seed, threads, budget) does not change the model the windows are made for.
        let mut sampled = base();
        sampled["sampling"] = serde_json::json!({"seed": 5});
        assert_eq!(reference, key(&sampled, "d1", "/p", b"w"));
    }

    /// Every value of the model the windows are made for, as the reference case writes it.
    fn reference_input() -> Value {
        let scenario = faris_model::LoadedScenario::from_bytes(include_bytes!(
            "../../../../scenarios/arc-inspired/cold-coupled-control.scenario.json"
        ))
        .unwrap();
        let physics = super::super::load_physics_case(
            Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../scenarios/arc-inspired/cold-coupled-control.reference.physics.json"
            )),
            &scenario,
        )
        .unwrap();
        let (request, _) = super::super::request_for_case(&scenario, &physics).unwrap();
        serde_json::json!({
            "manifest": crate::build_manifest(&scenario).unwrap(),
            "physics": physics,
            "request": request,
        })
    }

    /// Paths (key or index lists) of every number, string and boolean in a JSON value.
    fn leaf_paths(value: &Value, path: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
        match value {
            Value::Object(map) => {
                for (key, v) in map {
                    path.push(key.clone());
                    leaf_paths(v, path, out);
                    path.pop();
                }
            }
            Value::Array(items) => {
                for (i, v) in items.iter().enumerate() {
                    path.push(i.to_string());
                    leaf_paths(v, path, out);
                    path.pop();
                }
            }
            Value::Null => {}
            _ => out.push(path.clone()),
        }
    }

    fn at_path<'a>(value: &'a mut Value, path: &[String]) -> &'a mut Value {
        path.iter().fold(value, |v, step| match v {
            Value::Array(items) => &mut items[step.parse::<usize>().unwrap()],
            _ => &mut v[step.as_str()],
        })
    }

    // Verifies: NUC-057
    #[test]
    fn changing_any_geometry_material_or_source_value_changes_the_cache_key() {
        let base = reference_input();
        let key = |input: &Value| cache_key(input, "data", Path::new("/p"), b"worker");
        let reference = key(&base);
        let mut paths = Vec::new();
        leaf_paths(&base, &mut Vec::new(), &mut paths);
        // The windows do not depend on the field-mesh responses (see `solid_request`); every other
        // value must reach the key.
        let mesh_response = |path: &[String]| {
            path.first().map(String::as_str) == Some("request")
                && path.get(1).map(String::as_str) == Some("responses")
                && base["request"]["responses"][path[2].parse::<usize>().unwrap()]["domain"]["kind"]
                    == "mesh"
        };
        let (mut tested, mut false_reuse) = (0, Vec::new());
        for path in paths.iter().filter(|p| !mesh_response(p)) {
            let mut mutated = base.clone();
            let leaf = at_path(&mut mutated, path);
            *leaf = match leaf.clone() {
                Value::Bool(b) => Value::Bool(!b),
                Value::String(s) => Value::String(format!("{s}-changed")),
                number => serde_json::json!(number.as_f64().unwrap() * 1.0000001 + 1.0e-9),
            };
            tested += 1;
            if key(&mutated) == reference {
                false_reuse.push(path.join("/"));
            }
        }
        assert!(tested > 200, "only {tested} values reached the key test");
        assert!(
            false_reuse.is_empty(),
            "stale windows reused after a change of {false_reuse:?}"
        );
        // The three kinds of change the requirement names, spelled out on named values.
        let mut geometry = base.clone();
        geometry["manifest"]["major_radius_m"] =
            (base["manifest"]["major_radius_m"].as_f64().unwrap() * 1.001).into();
        let mut material = base.clone();
        material["physics"]["materials"][0]["recipe"]["density_kg_m3"] = 1.0.into();
        let mut source = base.clone();
        source["request"]["fusion_power_mw"] = 1.0.into();
        for changed in [&geometry, &material, &source] {
            assert_ne!(key(changed), reference);
        }
    }

    #[test]
    fn a_cache_entry_counts_only_when_its_file_matches_its_record() {
        let dir = tempfile::tempdir().unwrap();
        assert!(cached(dir.path()).is_none());
        std::fs::write(dir.path().join("filled.h5"), b"windows").unwrap();
        let hash = digest(b"windows");
        std::fs::write(
            dir.path().join("record.json"),
            serde_json::json!({"filled_sha256": hash}).to_string(),
        )
        .unwrap();
        assert!(cached(dir.path()).is_some());
        std::fs::write(dir.path().join("filled.h5"), b"damaged").unwrap();
        assert!(cached(dir.path()).is_none());
    }

    #[test]
    fn verify_recorded_binds_input_files_and_receipt() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(WINDOWS_FILE), b"w").unwrap();
        std::fs::write(dir.path().join(WINDOWS_RECORD_FILE), b"r").unwrap();
        let plan = WeightWindowsPlan {
            kind: KIND.into(),
            response_id: super::super::STOP_RULE_RESPONSE_ID.into(),
            windows_file: WINDOWS_FILE.into(),
            windows_sha256: digest(b"w"),
            record_file: WINDOWS_RECORD_FILE.into(),
            record_sha256: digest(b"r"),
        };
        let input = serde_json::json!({"variance_reduction": plan});
        let record = VarianceReductionRecord {
            mode: KIND.into(),
            reason: String::new(),
            windows_reused: Some(false),
            generation_seconds: Some(1.0),
        };
        let worker = serde_json::json!({"variance_reduction": {"mode": KIND, "windows_sha256": digest(b"w")}});
        assert!(
            verify_recorded(dir.path(), &input, Some(&worker), Some(&record), false)
                .unwrap()
                .is_some()
        );
        // A worker that read other windows, a changed file, a record that claims analog: each is refused.
        let wrong = serde_json::json!({"variance_reduction": {"mode": "analog"}});
        assert!(verify_recorded(dir.path(), &input, Some(&wrong), Some(&record), false).is_err());
        let analog_record = VarianceReductionRecord {
            mode: "analog".into(),
            ..record.clone()
        };
        assert!(
            verify_recorded(
                dir.path(),
                &input,
                Some(&worker),
                Some(&analog_record),
                false
            )
            .is_err()
        );
        std::fs::write(dir.path().join(WINDOWS_FILE), b"changed").unwrap();
        assert!(verify_recorded(dir.path(), &input, Some(&worker), Some(&record), false).is_err());
        // Records before windows existed have neither.
        assert_eq!(
            verify_recorded(dir.path(), &serde_json::json!({}), None, None, false).unwrap(),
            None
        );
    }

    // Verifies: PERF-025
    #[test]
    fn the_analog_pass_of_a_two_pass_run_reads_no_windows_but_its_record_says_the_run_used_them() {
        let dir = tempfile::tempdir().unwrap();
        let record = VarianceReductionRecord {
            mode: KIND.into(),
            reason: String::new(),
            windows_reused: Some(true),
            generation_seconds: Some(0.0),
        };
        let input = serde_json::json!({});
        assert!(verify_recorded(dir.path(), &input, None, Some(&record), true).is_ok());
        // without the windowed pass beside it, the same record contradicts its input
        assert!(verify_recorded(dir.path(), &input, None, Some(&record), false).is_err());
        // and a holder that claims a windowed pass but says analog is refused
        let analog = VarianceReductionRecord {
            mode: "analog".into(),
            ..record
        };
        assert!(verify_recorded(dir.path(), &input, None, Some(&analog), true).is_err());
    }

    #[test]
    fn a_preview_never_mentions_windows_and_a_production_run_without_python_says_why() {
        let reason = analog("x".into());
        assert!(!reason.record.uses_windows());
        assert!(reason.plan.is_none());
        assert!(failure_reason("a\nboom\n").contains("boom"));
    }
}
