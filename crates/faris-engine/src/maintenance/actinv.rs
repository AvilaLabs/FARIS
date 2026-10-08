//! Decay curves from ACTINV: the Amendment 2 path of `scripts/maintenance_coupling_test.py`.
//!
//! For one design and iteration the source writes the history, has `build_activation_inputs.py`
//! write one continuation spec per (shutdown, governing component installation) with the
//! installation's irradiation history up to the shutdown and then the cooling grid, runs
//! `actinv run` on the specs not yet in the content-addressed points cache, and reads each
//! cooling part back as (time since shutdown, decay heat of the whole installation). The points
//! cache (`faris-mct-actinv-points/v0.1`) is shared with the Python driver. Every external
//! process runs through [`crate::jobs`], so it has a wall-time limit, bounded logs, a process
//! group and cancellation.

use super::{
    CurveRequest, DecaySource, InstallationCurve, NotEvaluated, history_end_s, not_evaluated,
};
use crate::history::{EventKind, HistoryResult};
use crate::jobs::{Cancellation, ExecutionStatus, JobSpec, ResourceLimits, run_job};
use serde::de::{Deserialize, Deserializer, Error as _, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize as DeriveDeserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const POINTS_SCHEMA: &str = "faris-mct-actinv-points/v0.1";
pub const BARE_VARIANT: &str = "bare_lower_bound";
pub const IMPURITIES_VARIANT: &str = "specification_maximum_impurities";

/// Tolerance of "the installation in place at the shutdown", seconds (the driver's `TOL_S`).
const INSTALLATION_TOL_S: f64 = 1.0;
const MAX_WORKERS: usize = 64;
/// Bytes of a failed tool's output kept in the error.
const ERROR_TAIL_CHARS: usize = 600;

// ------------------------------------------------------------ result reader --

/// One time step of an ACTINV result, reduced to what the maintenance loop uses.
#[derive(Clone, Debug, PartialEq)]
pub struct StepPoint {
    pub t_s: f64,
    pub heat_w_per_g: f64,
    /// Absent in the result: 0. `None` only for an explicit null.
    pub flux: Option<f64>,
    /// `photon_source.contact_gamma_air_dose_proxy_Gy_h`, when the step has one.
    pub dose_proxy_gy_h: Option<f64>,
}

impl Serialize for StepPoint {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        (self.t_s, self.heat_w_per_g, self.flux, self.dose_proxy_gy_h).serialize(s)
    }
}

struct TotalOf(f64);

impl<'de> Deserialize<'de> for TotalOf {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = TotalOf;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("the heat_W_per_g object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<TotalOf, A::Error> {
                let mut total = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "total" {
                        total = Some(map.next_value::<f64>()?);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                total
                    .map(TotalOf)
                    .ok_or_else(|| A::Error::missing_field("total"))
            }
        }
        d.deserialize_map(V)
    }
}

struct DoseOf(Option<f64>);

impl<'de> Deserialize<'de> for DoseOf {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = DoseOf;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("the photon_source object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<DoseOf, A::Error> {
                let mut dose = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "contact_gamma_air_dose_proxy_Gy_h" {
                        dose = map.next_value::<Option<f64>>()?;
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(DoseOf(dose))
            }
        }
        d.deserialize_map(V)
    }
}

impl<'de> Deserialize<'de> for StepPoint {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = StepPoint;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an ACTINV step object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<StepPoint, A::Error> {
                let (mut t_s, mut heat, mut flux, mut dose) = (None, None, Some(0.0), None);
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "t_s" => t_s = Some(map.next_value::<f64>()?),
                        "heat_W_per_g" => heat = Some(map.next_value::<TotalOf>()?.0),
                        "flux" => flux = map.next_value::<Option<f64>>()?,
                        "photon_source" => {
                            dose = map.next_value::<Option<DoseOf>>()?.and_then(|p| p.0)
                        }
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(StepPoint {
                    t_s: t_s.ok_or_else(|| A::Error::missing_field("t_s"))?,
                    heat_w_per_g: heat.ok_or_else(|| A::Error::missing_field("heat_W_per_g"))?,
                    flux,
                    dose_proxy_gy_h: dose,
                })
            }
        }
        d.deserialize_map(V)
    }
}

struct StepList(Vec<StepPoint>);

impl<'de> Deserialize<'de> for StepList {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = StepList;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("the steps array")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<StepList, A::Error> {
                let mut out = Vec::new();
                while let Some(p) = seq.next_element::<StepPoint>()? {
                    out.push(p);
                }
                Ok(StepList(out))
            }
        }
        d.deserialize_seq(V)
    }
}

#[derive(Default)]
struct Body {
    ms: Value,
    pruned_states: Value,
    total_states: Value,
    steps: Vec<StepPoint>,
}

impl<'de> Deserialize<'de> for Body {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Body;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an ACTINV result object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Body, A::Error> {
                let mut body = Body::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "steps" => body.steps.extend(map.next_value::<StepList>()?.0),
                        "ms" => body.ms = map.next_value()?,
                        "pruned_states" => body.pruned_states = map.next_value()?,
                        "total_states" => body.total_states = map.next_value()?,
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(body)
            }
        }
        d.deserialize_map(V)
    }
}

/// An ACTINV result reduced to its points, with the identity of the file it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct ScannedResult {
    pub ms: Value,
    pub pruned_states: Value,
    pub total_states: Value,
    pub steps: Vec<StepPoint>,
    pub sha256: String,
    pub bytes: u64,
}

struct Hashing<R> {
    inner: R,
    digest: Sha256,
    bytes: u64,
}

impl<R: Read> Read for Hashing<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.digest.update(&buf[..n]);
        self.bytes += n as u64;
        Ok(n)
    }
}

/// Reads an ACTINV result one step at a time: the result can run to gigabytes, and only `ms`,
/// `pruned_states`, `total_states` and four numbers per step are kept. Values are decoded
/// with correct rounding, so they equal Python's `json.loads` values.
pub fn scan_result(reader: impl Read) -> Result<ScannedResult, String> {
    let mut buffered = BufReader::with_capacity(
        1 << 20,
        Hashing {
            inner: reader,
            digest: Sha256::new(),
            bytes: 0,
        },
    );
    let body = {
        let mut de = serde_json::Deserializer::from_reader(&mut buffered);
        let body = Body::deserialize(&mut de).map_err(|e| format!("result: {e}"))?;
        de.end().map_err(|e| format!("result: {e}"))?;
        body
    };
    let mut rest = Vec::new();
    buffered
        .read_to_end(&mut rest)
        .map_err(|e| format!("result: {e}"))?;
    let hashing = buffered.into_inner();
    Ok(ScannedResult {
        ms: body.ms,
        pruned_states: body.pruned_states,
        total_states: body.total_states,
        steps: body.steps,
        sha256: format!("{:x}", hashing.digest.finalize()),
        bytes: hashing.bytes,
    })
}

/// The compact per-spec points file shared with the Python driver.
#[derive(Serialize, DeriveDeserialize)]
pub struct PointsFile {
    pub schema: String,
    #[serde(default)]
    pub result_sha256: String,
    #[serde(default)]
    pub result_bytes: u64,
    #[serde(default)]
    pub n_steps: u64,
    #[serde(default)]
    pub ms: Value,
    #[serde(default)]
    pub pruned_states: Value,
    #[serde(default)]
    pub total_states: Value,
    pub steps: Vec<(f64, f64, Option<f64>, Option<f64>)>,
}

/// Compacts an ACTINV result into a points file (atomically), then deletes the result.
pub fn write_points(result_path: &Path, points_path: &Path) -> Result<(), String> {
    let file =
        std::fs::File::open(result_path).map_err(|e| format!("{}: {e}", result_path.display()))?;
    let scanned = scan_result(file).map_err(|e| format!("{}: {e}", result_path.display()))?;
    let doc = PointsFile {
        schema: POINTS_SCHEMA.into(),
        result_sha256: scanned.sha256,
        result_bytes: scanned.bytes,
        n_steps: scanned.steps.len() as u64,
        ms: scanned.ms,
        pruned_states: scanned.pruned_states,
        total_states: scanned.total_states,
        steps: scanned
            .steps
            .iter()
            .map(|p| (p.t_s, p.heat_w_per_g, p.flux, p.dose_proxy_gy_h))
            .collect(),
    };
    if let Some(parent) = points_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let mut tmp_name = points_path.as_os_str().to_os_string();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    let write = || -> std::io::Result<()> {
        let mut out = BufWriter::new(std::fs::File::create(&tmp)?);
        serde_json::to_writer(&mut out, &doc).map_err(std::io::Error::from)?;
        out.write_all(b"\n")?;
        out.flush()?;
        out.get_ref().sync_all()
    };
    write().map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, points_path).map_err(|e| format!("{}: {e}", points_path.display()))?;
    std::fs::remove_file(result_path).map_err(|e| format!("{}: {e}", result_path.display()))
}

pub fn read_points(path: &Path) -> Result<PointsFile, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let points: PointsFile = serde_json::from_reader(BufReader::new(file))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if points.schema != POINTS_SCHEMA {
        return Err(format!("{}: schema is not {POINTS_SCHEMA}", path.display()));
    }
    Ok(points)
}

// ------------------------------------------------------------ installations --

/// (install_s, remove_s) of each installation of a component: from 0 or a replacement's
/// completion to the next replacement's start or the end (`build_activation_inputs.installations`).
pub fn installations(history: &HistoryResult, component: &str) -> Result<Vec<(f64, f64)>, String> {
    let end = history_end_s(history);
    let mut events: Vec<_> = history
        .events
        .iter()
        .filter(|e| {
            e.component_id.as_deref() == Some(component)
                && matches!(
                    e.kind,
                    EventKind::ReplacementStarted | EventKind::ReplacementCompleted
                )
        })
        .collect();
    events.sort_by(|a, b| a.time_s.total_cmp(&b.time_s).then(a.order.cmp(&b.order)));
    let (mut out, mut start) = (Vec::new(), Some(0.0));
    for e in events {
        if e.kind == EventKind::ReplacementStarted {
            let from = start.take().ok_or_else(|| {
                format!(
                    "{component}: replacement_started at {} while removed",
                    e.time_s
                )
            })?;
            out.push((from, e.time_s));
        } else {
            if start.is_some() {
                return Err(format!(
                    "{component}: replacement_completed at {} while installed",
                    e.time_s
                ));
            }
            start = Some(e.time_s);
        }
    }
    if let Some(from) = start {
        out.push((from, end));
    }
    Ok(out)
}

/// 1-based index of the installation of `component` in place at `shutdown_s`: the last one that
/// started before it and ends no earlier (the driver's rule, tolerance 1 s).
pub fn installation_at(
    history: &HistoryResult,
    component: &str,
    shutdown_s: f64,
) -> Result<Option<u32>, String> {
    Ok(installations(history, component)?
        .into_iter()
        .enumerate()
        .filter(|(_, (a, b))| {
            *a < shutdown_s - INSTALLATION_TOL_S + 1e-9 && shutdown_s <= *b + INSTALLATION_TOL_S
        })
        .map(|(i, _)| i as u32 + 1)
        .next_back())
}

// ------------------------------------------------------------------- source --

/// The scenario-side files of one design that `build_activation_inputs.py` reads.
#[derive(Clone, Debug)]
pub struct DesignFiles {
    pub scenario: PathBuf,
    pub physics: PathBuf,
    /// Transport run record: sets each component's flux scale and volume.
    pub history_run: PathBuf,
    /// 709-group run whose spectrum shapes are used.
    pub spectrum_run: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ActinvSourceConfig {
    /// Interpreter that runs the builder; a bare name is looked up on PATH.
    pub python: PathBuf,
    pub builder: PathBuf,
    pub actinv: PathBuf,
    /// ACTINV data root (the folder above the catalogue version).
    pub data_dir: PathBuf,
    /// Content-addressed `<sha>.points.json` files, shared across runs and with the driver.
    pub cache_dir: PathBuf,
    /// Per-(design, iteration) folders and scratch space.
    pub work_dir: PathBuf,
    /// Concurrent ACTINV runs, 1 to 64.
    pub workers: usize,
    /// Impurity specification: selects the specification-maximum variant instead of the bare one.
    pub impurities: Option<PathBuf>,
    pub builder_timeout: Duration,
    pub actinv_timeout: Duration,
    pub resource_limits: ResourceLimits,
}

impl ActinvSourceConfig {
    pub fn new(
        builder: PathBuf,
        actinv: PathBuf,
        data_dir: PathBuf,
        cache_dir: PathBuf,
        work_dir: PathBuf,
    ) -> Self {
        Self {
            python: PathBuf::from("python3"),
            builder,
            actinv,
            data_dir,
            cache_dir,
            work_dir,
            workers: 1,
            impurities: None,
            builder_timeout: Duration::from_secs(6 * 3600),
            actinv_timeout: Duration::from_secs(24 * 3600),
            resource_limits: ResourceLimits::default(),
        }
    }

    pub fn variant(&self) -> &'static str {
        if self.impurities.is_some() {
            IMPURITIES_VARIANT
        } else {
            BARE_VARIANT
        }
    }
}

/// Running totals of a source, readable from another thread while it works.
#[derive(Debug, Default)]
pub struct SourceCounters {
    pub actinv_runs: AtomicU64,
    pub cache_hits: AtomicU64,
}

/// Receives progress lines.
pub type LogFn = Box<dyn Fn(&str) + Send + Sync>;

pub struct ActinvDecaySource {
    config: ActinvSourceConfig,
    python: PathBuf,
    data_dir: PathBuf,
    designs: BTreeMap<String, DesignFiles>,
    /// Hash of everything but the history that the specs of a design depend on.
    design_keys: BTreeMap<String, String>,
    actinv_runs: u64,
    cache_hits: u64,
    counters: Option<Arc<SourceCounters>>,
    log: Option<LogFn>,
}

fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

/// Absolute path of an executable: a bare name is searched on PATH.
fn resolve_program(program: &Path) -> Result<PathBuf, String> {
    let direct = program.is_absolute() || program.components().count() > 1;
    let found = if direct {
        std::path::absolute(program).ok().filter(|p| p.is_file())
    } else {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|d| d.join(program))
                .find(|p| p.is_file())
        })
    };
    found.ok_or_else(|| format!("executable {} not found", program.display()))
}

fn existing_file(path: &Path, what: &str) -> Result<PathBuf, String> {
    let absolute = std::path::absolute(path).map_err(|e| format!("{what}: {e}"))?;
    if !absolute.is_file() {
        return Err(format!("{what} {} is not a file", absolute.display()));
    }
    Ok(absolute)
}

fn tail_of(text: &str) -> String {
    let chars: Vec<char> = text.trim().chars().collect();
    chars[chars.len().saturating_sub(ERROR_TAIL_CHARS)..]
        .iter()
        .collect()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut tmp_name = path.as_os_str().to_os_string();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// `build_activation_inputs.py` writes manifest.json last; a folder without it, or whose
/// validation did not all pass, is an interrupted build.
pub fn complete_spec_dir(spec_dir: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(spec_dir.join("manifest.json")) else {
        return false;
    };
    let Ok(record) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    let validated = record
        .get("validation")
        .and_then(Value::as_array)
        .is_none_or(|all| {
            all.iter()
                .all(|r| r.get("ok").and_then(Value::as_bool) == Some(true))
        });
    let placeholders = match record.get("placeholder_spectrum_components") {
        None | Some(Value::Null) => false,
        Some(Value::Array(a)) => !a.is_empty(),
        Some(_) => true,
    };
    validated && !placeholders
}

#[derive(serde::Serialize)]
struct TimesEntry<'a> {
    component: &'a str,
    installation: u32,
    shutdown_s: f64,
}

#[derive(DeriveDeserialize)]
struct Provenance {
    spec_file: String,
    component: String,
    installation_index: u32,
    installation_interval_s: (f64, f64),
    mass_g: f64,
    volume_m3: f64,
    #[serde(default)]
    continuation_of_shutdown_s: Option<f64>,
}

type CurveKey = (String, u32, i64);

impl ActinvDecaySource {
    pub fn new(
        mut config: ActinvSourceConfig,
        designs: BTreeMap<String, DesignFiles>,
    ) -> Result<Self, String> {
        if !(1..=MAX_WORKERS).contains(&config.workers) {
            return Err(format!("workers must be 1 to {MAX_WORKERS}"));
        }
        let python = resolve_program(&config.python)?;
        config.builder = existing_file(&config.builder, "builder script")?;
        config.actinv = resolve_program(&config.actinv)?;
        if let Some(path) = &config.impurities {
            config.impurities = Some(existing_file(path, "impurities file")?);
        }
        let data_dir = std::fs::canonicalize(&config.data_dir)
            .map_err(|e| format!("data dir {}: {e}", config.data_dir.display()))?;
        config.cache_dir = std::path::absolute(&config.cache_dir)
            .map_err(|e| format!("cache dir {}: {e}", config.cache_dir.display()))?;
        config.work_dir = std::path::absolute(&config.work_dir)
            .map_err(|e| format!("work dir {}: {e}", config.work_dir.display()))?;
        let builder_hash = hash_file(&config.builder)?;
        let impurities_hash = match &config.impurities {
            Some(path) => hash_file(path)?,
            None => String::new(),
        };
        let mut checked = BTreeMap::new();
        let mut keys = BTreeMap::new();
        for (name, files) in designs {
            let files = DesignFiles {
                scenario: existing_file(&files.scenario, &format!("design {name} scenario"))?,
                physics: existing_file(&files.physics, &format!("design {name} physics"))?,
                history_run: existing_file(
                    &files.history_run,
                    &format!("design {name} history run"),
                )?,
                spectrum_run: existing_file(
                    &files.spectrum_run,
                    &format!("design {name} spectrum run"),
                )?,
            };
            let mut key = Sha256::new();
            for part in [
                name.as_str(),
                &builder_hash,
                &impurities_hash,
                &hash_file(&files.scenario)?,
                &hash_file(&files.physics)?,
                &hash_file(&files.history_run)?,
                &hash_file(&files.spectrum_run)?,
            ] {
                key.update(part.as_bytes());
                key.update([0]);
            }
            keys.insert(name.clone(), format!("{:x}", key.finalize()));
            checked.insert(name, files);
        }
        Ok(Self {
            config,
            python,
            data_dir,
            designs: checked,
            design_keys: keys,
            actinv_runs: 0,
            cache_hits: 0,
            counters: None,
            log: None,
        })
    }

    /// Receives progress lines (design, ACTINV runs, cache hits).
    pub fn with_log(mut self, log: LogFn) -> Self {
        self.log = Some(log);
        self
    }

    /// Mirrors the running totals into `counters` as they change.
    pub fn with_counters(mut self, counters: Arc<SourceCounters>) -> Self {
        self.counters = Some(counters);
        self
    }

    /// (ACTINV runs started, curves found in the cache) so far.
    pub fn stats(&self) -> (u64, u64) {
        (self.actinv_runs, self.cache_hits)
    }

    fn say(&self, line: &str) {
        if let Some(log) = &self.log {
            log(line);
        }
    }

    fn environment(&self) -> Vec<(OsString, OsString)> {
        let mut env: Vec<(OsString, OsString)> = ["PATH", "HOME", "LANG", "LC_ALL", "TMPDIR"]
            .into_iter()
            .filter_map(|k| std::env::var_os(k).map(|v| (k.into(), v)))
            .collect();
        env.push((
            "ACTINV_DATA_DIR".into(),
            self.data_dir.clone().into_os_string(),
        ));
        env
    }

    /// Runs one tool through the job boundary; `Err("cancelled")` on cancellation.
    fn run_tool(
        &self,
        program: &Path,
        arguments: Vec<OsString>,
        scratch: &Path,
        timeout: Duration,
        what: &str,
        cancel: &Cancellation,
    ) -> Result<(), String> {
        std::fs::create_dir_all(scratch).map_err(|e| format!("{}: {e}", scratch.display()))?;
        let spec = JobSpec {
            program: program.to_owned(),
            arguments,
            working_directory: scratch.to_owned(),
            environment: self.environment(),
            timeout,
            capture_limit_bytes: 1 << 20,
            artifact_roots: Vec::new(),
            resource_limits: self.config.resource_limits,
        };
        let result = run_job(&spec, cancel).map_err(|e| format!("{what}: {e}"))?;
        let hint = match result.execution_status {
            ExecutionStatus::Succeeded => return Ok(()),
            ExecutionStatus::Cancelled => return Err("cancelled".into()),
            ExecutionStatus::FileSizeLimit | ExecutionStatus::ArtifactLimit => {
                " (the job boundary's file or artifact size limit; see jobs::ResourceLimits)"
            }
            _ => "",
        };
        Err(format!(
            "{what} failed ({:?}, exit {:?}){hint}: {}",
            result.execution_status,
            result.exit_code,
            tail_of(&format!("{}\n{}", result.stdout, result.stderr))
        ))
    }

    fn build_specs(
        &self,
        files: &DesignFiles,
        folder: &Path,
        components: &BTreeSet<String>,
        grid_s: &[f64],
        design: &str,
        cancel: &Cancellation,
    ) -> Result<(), String> {
        let spec_dir = folder.join("decay");
        let grid: Vec<String> = grid_s.iter().map(|t| format!("{t:?}s")).collect();
        let mut arguments: Vec<OsString> = vec![
            self.config.builder.clone().into(),
            "--run".into(),
            files.history_run.clone().into(),
            "--spectrum-run".into(),
            files.spectrum_run.clone().into(),
            "--scenario".into(),
            files.scenario.clone().into(),
            "--physics".into(),
            files.physics.clone().into(),
            "--history".into(),
            folder.join("history.json").into(),
            "--data-dir".into(),
            self.data_dir.clone().into(),
            "--cooling-grid".into(),
            grid.join(",").into(),
            "--actinv-outputs".into(),
            "heat".into(),
            "--actinv".into(),
            self.config.actinv.clone().into(),
            "--output-dir".into(),
            spec_dir.into(),
            "--decay-continuations".into(),
            folder.join("decay-times.json").into(),
            "--continuations-only".into(),
        ];
        for component in components {
            arguments.push("--component".into());
            arguments.push(component.into());
        }
        if let Some(path) = &self.config.impurities {
            arguments.push("--impurities".into());
            arguments.push(path.clone().into());
        }
        let scratch = folder.join("builder-cwd");
        let outcome = self.run_tool(
            &self.python,
            arguments,
            &scratch,
            self.config.builder_timeout,
            &format!("build_activation_inputs ({design})"),
            cancel,
        );
        let _ = std::fs::remove_dir_all(&scratch);
        outcome
    }

    /// One ACTINV run: the result stays in a scratch folder, only the points file is kept.
    fn run_spec(&self, sha: &str, spec: &Path, cancel: &Cancellation) -> Result<(), String> {
        let scratch = self.config.work_dir.join("tmp").join(&sha[..16]);
        let _ = std::fs::remove_dir_all(&scratch);
        let result = scratch.join("result.json");
        let name = spec
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let outcome = self
            .run_tool(
                &self.config.actinv,
                vec!["run".into(), spec.into(), (&result).into()],
                &scratch,
                self.config.actinv_timeout,
                &format!("actinv run {name}"),
                cancel,
            )
            .and_then(|()| {
                write_points(&result, &self.points_path(sha))
                    .map_err(|e| format!("actinv run {name}: {e}"))
            });
        let _ = std::fs::remove_dir_all(&scratch);
        outcome
    }

    fn points_path(&self, sha: &str) -> PathBuf {
        self.config.cache_dir.join(format!("{sha}.points.json"))
    }

    /// Runs the specs given as (sha, spec path), `workers` at a time. The first failure in spec
    /// order is returned; specs not yet started are then skipped.
    fn run_missing(
        &self,
        missing: &[(String, PathBuf)],
        design: &str,
        cancel: &Cancellation,
    ) -> Result<(), String> {
        std::fs::create_dir_all(&self.config.cache_dir)
            .map_err(|e| format!("{}: {e}", self.config.cache_dir.display()))?;
        let next = AtomicUsize::new(0);
        let done = AtomicUsize::new(0);
        let failed = Arc::new(AtomicBool::new(false));
        let errors: Mutex<Vec<(usize, String)>> = Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for _ in 0..self.config.workers.min(missing.len()) {
                scope.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::SeqCst);
                        if i >= missing.len()
                            || failed.load(Ordering::SeqCst)
                            || cancel.is_cancelled()
                        {
                            break;
                        }
                        let (sha, spec) = &missing[i];
                        match self.run_spec(sha, spec, cancel) {
                            Ok(()) => {
                                let n = done.fetch_add(1, Ordering::SeqCst) + 1;
                                self.say(&format!(
                                    "{design}: ACTINV run {n}/{} done",
                                    missing.len()
                                ));
                            }
                            Err(e) => {
                                failed.store(true, Ordering::SeqCst);
                                errors.lock().expect("error list").push((i, e));
                            }
                        }
                    }
                });
            }
        });
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }
        let mut errors = errors.into_inner().expect("error list");
        errors.sort();
        match errors.into_iter().next() {
            Some((_, e)) => Err(e),
            None => Ok(()),
        }
    }

    /// Curves of the built specs, keyed by (component, installation, rounded shutdown).
    fn read_curves(
        &mut self,
        spec_dir: &Path,
        grid_s: &[f64],
        design: &str,
        cancel: &Cancellation,
    ) -> Result<BTreeMap<CurveKey, InstallationCurve>, String> {
        let suffix = format!("__{}.provenance.json", self.config.variant());
        let mut names: Vec<String> = std::fs::read_dir(spec_dir)
            .map_err(|e| format!("{}: {e}", spec_dir.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(&suffix))
            .collect();
        names.sort();
        let mut provs = Vec::new();
        for name in &names {
            let path = spec_dir.join(name);
            let prov: Provenance = serde_json::from_slice(
                &std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?,
            )
            .map_err(|e| format!("{}: {e}", path.display()))?;
            if prov.continuation_of_shutdown_s.is_some() {
                let spec = spec_dir.join(&prov.spec_file);
                let sha = hex_sha256(
                    &std::fs::read(&spec).map_err(|e| format!("{}: {e}", spec.display()))?,
                );
                provs.push((prov, spec, sha));
            }
        }
        let mut missing: BTreeMap<String, PathBuf> = BTreeMap::new();
        let mut hits = 0u64;
        for (_, spec, sha) in &provs {
            if self.points_path(sha).exists() || missing.contains_key(sha) {
                hits += 1;
            } else {
                missing.insert(sha.clone(), spec.clone());
            }
        }
        let missing: Vec<(String, PathBuf)> = missing.into_iter().collect();
        self.say(&format!(
            "{design}: {} decay curves, {} to run, {hits} cached",
            provs.len(),
            missing.len()
        ));
        self.cache_hits += hits;
        if let Some(counters) = &self.counters {
            counters.cache_hits.fetch_add(hits, Ordering::Relaxed);
        }
        self.run_missing(&missing, design, cancel)?;
        self.actinv_runs += missing.len() as u64;
        if let Some(counters) = &self.counters {
            counters
                .actinv_runs
                .fetch_add(missing.len() as u64, Ordering::Relaxed);
        }
        let mut out = BTreeMap::new();
        for (prov, spec, sha) in &provs {
            let name = spec
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let stored = read_points(&self.points_path(sha))?;
            let tail = &stored.steps[stored.steps.len().saturating_sub(grid_s.len())..];
            let ok = tail.len() == grid_s.len()
                && tail.iter().all(|s| s.2 == Some(0.0))
                && (tail[tail.len() - 1].0 - tail[0].0 - (grid_s[grid_s.len() - 1] - grid_s[0]))
                    .abs()
                    <= 1e-6 * grid_s[grid_s.len() - 1];
            if !ok {
                return Err(format!(
                    "{name}: the last {} steps are not the cooling grid",
                    grid_s.len()
                ));
            }
            let shutdown = prov
                .continuation_of_shutdown_s
                .expect("continuations are filtered above");
            out.insert(
                (
                    prov.component.clone(),
                    prov.installation_index,
                    shutdown.round_ties_even() as i64,
                ),
                InstallationCurve {
                    points: grid_s
                        .iter()
                        .zip(tail)
                        .map(|(tau, s)| (*tau, s.1 * prov.mass_g))
                        .collect(),
                    volume_m3: prov.volume_m3,
                    install_s: prov.installation_interval_s.0,
                },
            );
        }
        Ok(out)
    }
}

impl DecaySource for ActinvDecaySource {
    fn curves(
        &mut self,
        design: &str,
        history: &HistoryResult,
        requests: &[CurveRequest],
        grid_s: &[f64],
        cancel: &Cancellation,
    ) -> Result<Vec<Result<InstallationCurve, NotEvaluated>>, String> {
        let files = self
            .designs
            .get(design)
            .ok_or_else(|| format!("no ACTINV inputs for design {design}"))?
            .clone();
        // Which installation each request refers to.
        let mut wanted: Vec<Option<(String, u32, i64)>> = Vec::new();
        let mut entries: BTreeMap<(u64, String), (f64, u32)> = BTreeMap::new();
        for r in requests {
            let found = installation_at(history, &r.component, r.shutdown_s)
                .map_err(|e| format!("design {design}: {e}"))?;
            wanted.push(found.map(|k| {
                entries.insert(
                    (r.shutdown_s.to_bits(), r.component.clone()),
                    (r.shutdown_s, k),
                );
                (
                    r.component.clone(),
                    k,
                    r.shutdown_s.round_ties_even() as i64,
                )
            }));
        }
        let mut sorted: Vec<(&String, f64, u32)> =
            entries.iter().map(|((_, c), (s, k))| (c, *s, *k)).collect();
        sorted.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(b.0)));
        let gap = |r: &CurveRequest| {
            not_evaluated(
                format!(
                    "{} has no installation in place at the shutdown at {:.0} s",
                    r.component, r.shutdown_s
                ),
                "the component is out of the machine at that shutdown, so it has no decay heat to wait for; check the replacement schedule or remove it from the class's governing components",
            )
        };
        if sorted.is_empty() {
            return Ok(requests.iter().map(|r| Err(gap(r))).collect());
        }
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }

        // The per-(design, iteration) folder is named by everything the specs depend on.
        let history_bytes = {
            let mut bytes = serde_json::to_vec_pretty(history).map_err(|e| e.to_string())?;
            bytes.push(b'\n');
            bytes
        };
        let grid_text: Vec<String> = grid_s.iter().map(|t| format!("{t:?}")).collect();
        let mut key = Sha256::new();
        for part in [
            self.design_keys[design].as_bytes(),
            grid_text.join(",").as_bytes(),
            &history_bytes,
        ] {
            key.update(part);
            key.update([0]);
        }
        let key = format!("{:x}", key.finalize());
        let slug: String = design
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let folder = self
            .config
            .work_dir
            .join("designs")
            .join(slug)
            .join(&key[..16]);
        std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
        let history_path = folder.join("history.json");
        if std::fs::read(&history_path).ok().as_deref() != Some(&history_bytes) {
            write_atomic(&history_path, &history_bytes)?;
        }

        let times: Vec<TimesEntry> = sorted
            .iter()
            .map(|(component, shutdown_s, installation)| TimesEntry {
                component,
                installation: *installation,
                shutdown_s: *shutdown_s,
            })
            .collect();
        let mut times_text = serde_json::to_string_pretty(&times).map_err(|e| e.to_string())?;
        times_text.push('\n');
        let times_path = folder.join("decay-times.json");
        let spec_dir = folder.join("decay");
        if spec_dir.exists()
            && !(complete_spec_dir(&spec_dir)
                && std::fs::read_to_string(&times_path).ok().as_deref() == Some(&times_text))
        {
            std::fs::remove_dir_all(&spec_dir)
                .map_err(|e| format!("{}: {e}", spec_dir.display()))?;
        }
        if !spec_dir.exists() {
            write_atomic(&times_path, times_text.as_bytes())?;
            let components: BTreeSet<String> =
                sorted.iter().map(|(c, _, _)| (*c).clone()).collect();
            self.say(&format!(
                "{design}: building {} continuation specs",
                sorted.len()
            ));
            self.build_specs(&files, &folder, &components, grid_s, design, cancel)?;
        }
        let by_key = self.read_curves(&spec_dir, grid_s, design, cancel)?;
        requests
            .iter()
            .zip(wanted)
            .map(|(r, want)| match want {
                None => Ok(Err(gap(r))),
                Some(key) => by_key.get(&key).cloned().map(Ok).ok_or_else(|| {
                    format!(
                        "the builder wrote no {} continuation for {} installation {} at {:.0} s",
                        self.config.variant(),
                        key.0,
                        key.1,
                        r.shutdown_s
                    )
                }),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
