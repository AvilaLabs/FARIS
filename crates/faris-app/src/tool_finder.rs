//! "Find tools": look in the usual places for Python environments, ask each one a single short
//! question (what is installed, which OpenMC), and propose a value for each of the six tools.
//!
//! Looking is cheap: folder listings, then one bounded job per interpreter through the engine's
//! job runner (`faris_engine::jobs`), off the UI thread. Nothing is installed, changed or run
//! beyond that one probe. Choosing from the answers (`choose`) is a pure function, so it is
//! tested with injected answers and no real interpreters.

use crate::settings::Tool;
use crate::time::Duration;
use faris_engine::jobs::{Cancellation, ExecutionStatus, JobSpec, ResourceLimits, run_job};
use serde::Deserialize;
use std::{
    collections::HashSet,
    ffi::OsString,
    path::{Path, PathBuf},
};

/// The most interpreters asked, so a crowded machine cannot make the search long.
pub const MAX_CANDIDATES: usize = 24;
/// How long one interpreter has to answer.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(40);
/// Folder entries read in one extra search folder.
const EXTRA_ENTRY_BUDGET: usize = 4000;
/// How far below an extra search folder to look.
const EXTRA_DEPTH: usize = 2;
/// A library larger than this is not read for the aiming markers.
const MAX_LIBRARY_BYTES: u64 = 512 * 1024 * 1024;

/// What the engine's aiming build adds to the OpenMC library: the names of its variables, as in
/// `aiming_supported` of `integrations/openmc/streaming_aim.py`.
pub const AIMING_MARKERS: [&[u8]; 4] = [
    b"OPENMC_APERTURE_BIAS",
    b"OPENMC_APERTURE_ONE_SIDED",
    b"OPENMC_COLLISION_AIM",
    b"OPENMC_WW_SKIP_VOID",
];

/// The question every interpreter is asked. It imports nothing to find out what is installed
/// (`find_spec` and package metadata); only OpenMC is imported, once, to read its DAGMC flag and
/// its configured cross sections.
pub const PROBE_SCRIPT: &str = r#"
import json, os, sys
import importlib.util as U
import importlib.metadata as M

def spec(name):
    try:
        return U.find_spec(name)
    except Exception:
        return None

def version(name, *dists):
    if spec(name) is None:
        return None
    for dist in dists or (name,):
        try:
            return M.version(dist)
        except Exception:
            pass
    return ""

out = {
    "executable": sys.executable,
    "python_version": sys.version.split()[0],
    "cadquery": version("cadquery"),
    "cad_to_dagmc": version("cad_to_dagmc", "cad_to_dagmc", "cad-to-dagmc"),
    "h5py": version("h5py"),
    "openmc": version("openmc"),
    "openmc_dagmc": None,
    "openmc_package_dir": None,
    "openmc_library": None,
    "openmc_error": None,
    "plasma_source_dir": None,
    "cross_sections": os.environ.get("OPENMC_CROSS_SECTIONS") or None,
}
s = spec("openmc")
if s is not None and s.submodule_search_locations:
    package = list(s.submodule_search_locations)[0]
    out["openmc_package_dir"] = package
    lib = os.path.join(package, "lib")
    if os.path.isdir(lib):
        for name in sorted(os.listdir(lib)):
            if (name.startswith("libopenmc") or name.startswith("openmc.")) and not name.endswith(".py"):
                out["openmc_library"] = os.path.join(lib, name)
                break
    try:
        import openmc
        import openmc.lib
        out["openmc_dagmc"] = bool(openmc.lib._dagmc_enabled())
        configured = openmc.config.get("cross_sections")
        if configured:
            out["cross_sections"] = str(configured)
    except BaseException as error:
        out["openmc_error"] = (type(error).__name__ + ": " + str(error)).splitlines()[0][:200]
p = spec("openmc_plasma_source")
if p is not None and p.submodule_search_locations:
    out["plasma_source_dir"] = os.path.dirname(list(p.submodule_search_locations)[0])
print(json.dumps(out))
"#;

// ---------------------------------------------------------------- searching --

/// An interpreter to ask, and why it is on the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub python: PathBuf,
    pub origin: String,
}

/// A file or folder found by looking, not by asking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Located {
    pub path: PathBuf,
    pub origin: String,
}

/// Where to look. Built from the environment, or by hand in tests.
#[derive(Clone, Debug, Default)]
pub struct SearchRoots {
    pub home: Option<PathBuf>,
    pub path_dirs: Vec<PathBuf>,
    pub conda_prefix: Option<PathBuf>,
    pub mamba_root: Option<PathBuf>,
    pub extra: Vec<PathBuf>,
}

impl SearchRoots {
    pub fn from_environment(extra: &[String]) -> Self {
        let var = |n: &str| {
            std::env::var_os(n)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Self {
            home: var("HOME").or_else(|| var("USERPROFILE")),
            path_dirs: std::env::var_os("PATH")
                .map(|p| std::env::split_paths(&p).collect())
                .unwrap_or_default(),
            conda_prefix: var("CONDA_PREFIX"),
            mamba_root: var("MAMBA_ROOT_PREFIX"),
            extra: extra
                .iter()
                .map(|e| e.trim())
                .filter(|e| !e.is_empty())
                .map(|e| expand_home(e, var("HOME").or_else(|| var("USERPROFILE")).as_deref()))
                .collect(),
        }
    }
}

/// `~/x` -> `HOME/x`.
fn expand_home(text: &str, home: Option<&Path>) -> PathBuf {
    match (text.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(text),
    }
}

/// What looking found.
#[derive(Debug, Default)]
pub struct Scan {
    pub candidates: Vec<Candidate>,
    pub cross_sections: Vec<Located>,
    pub plasma_sources: Vec<Located>,
    /// Candidates left out because the list was full.
    pub skipped: usize,
}

/// The interpreter of an environment folder, if it has one.
pub fn env_python(dir: &Path) -> Option<PathBuf> {
    [
        "bin/python",
        "bin/python3",
        "python.exe",
        "Scripts/python.exe",
    ]
    .iter()
    .map(|rel| dir.join(rel))
    .find(|p| p.is_file())
}

/// The key two paths to one environment share: the canonical folder above `bin`.
fn env_key(python: &Path) -> PathBuf {
    let dir = python.parent().and_then(Path::parent).unwrap_or(python);
    dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf())
}

fn sorted_children(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    children.sort();
    children
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

struct Collector {
    scan: Scan,
    seen: HashSet<PathBuf>,
}

impl Collector {
    fn add(&mut self, python: PathBuf, origin: String) {
        if !self.seen.insert(env_key(&python)) {
            return;
        }
        if self.scan.candidates.len() >= MAX_CANDIDATES {
            self.scan.skipped += 1;
            return;
        }
        self.scan.candidates.push(Candidate { python, origin });
    }
}

/// Looks in the usual places; runs nothing. The order is the order of preference when two
/// interpreters are equally good: the user's own folders, the active environment, `PATH`, then
/// the environment managers' folders.
pub fn scan(roots: &SearchRoots) -> Scan {
    let mut c = Collector {
        scan: Scan::default(),
        seen: HashSet::new(),
    };
    for extra in &roots.extra {
        scan_extra(&mut c, extra);
    }
    if let Some(prefix) = &roots.conda_prefix
        && let Some(python) = env_python(prefix)
    {
        c.add(
            python,
            format!("the active conda environment ({})", prefix.display()),
        );
    }
    for name in ["python3", "python", "python3.exe", "python.exe"] {
        for dir in &roots.path_dirs {
            let python = dir.join(name);
            if python.is_file() {
                c.add(python, format!("{name} on PATH"));
                break;
            }
        }
    }
    if let Some(home) = &roots.home {
        let mut managers: Vec<(String, PathBuf)> = [
            "micromamba/envs",
            "miniforge3/envs",
            "miniconda3/envs",
            "anaconda3/envs",
            ".conda/envs",
        ]
        .iter()
        .map(|rel| (format!("~/{rel}"), home.join(rel)))
        .collect();
        if let Some(root) = &roots.mamba_root {
            managers.push((
                format!("{}", root.join("envs").display()),
                root.join("envs"),
            ));
        }
        for (shown, folder) in managers {
            for env in sorted_children(&folder) {
                if let Some(python) = env_python(&env) {
                    c.add(
                        python,
                        format!("environment {} in {shown}", file_name(&env)),
                    );
                }
            }
        }
        for base in ["miniforge3", "miniconda3", "anaconda3"] {
            if let Some(python) = env_python(&home.join(base)) {
                c.add(python, format!("the base environment of ~/{base}"));
            }
        }
        for env in sorted_children(&home.join(".venvs")) {
            if let Some(python) = env_python(&env) {
                c.add(
                    python,
                    format!("virtualenv {} in ~/.venvs", file_name(&env)),
                );
            }
        }
    }
    c.scan
}

/// Looks in one folder the user named, two levels down: environments, a `cross_sections.xml`,
/// a folder holding `openmc_plasma_source`.
fn scan_extra(c: &mut Collector, root: &Path) {
    let mut budget = EXTRA_ENTRY_BUDGET;
    let mut level = vec![root.to_path_buf()];
    for depth in 0..=EXTRA_DEPTH {
        let mut next = Vec::new();
        for dir in level {
            if !dir.is_dir() {
                continue;
            }
            let origin = format!("the search folder {}", root.display());
            if let Some(python) = env_python(&dir) {
                c.add(python, format!("{} (in {origin})", file_name(&dir)));
                continue;
            }
            let xml = dir.join("cross_sections.xml");
            if xml.is_file() {
                c.scan.cross_sections.push(Located {
                    path: xml,
                    origin: origin.clone(),
                });
            }
            if dir
                .join("openmc_plasma_source")
                .join("__init__.py")
                .is_file()
            {
                c.scan.plasma_sources.push(Located {
                    path: dir.clone(),
                    origin: origin.clone(),
                });
            }
            if depth == EXTRA_DEPTH {
                continue;
            }
            for child in sorted_children(&dir) {
                if budget == 0 {
                    return;
                }
                budget -= 1;
                let name = file_name(&child);
                let skip = name.starts_with('.')
                    || matches!(
                        name.as_str(),
                        "target" | "node_modules" | "site-packages" | "__pycache__"
                    );
                if !skip && child.is_dir() {
                    next.push(child);
                }
            }
        }
        level = next;
    }
}

// ------------------------------------------------------------------ asking --

/// What one interpreter said.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ProbeReport {
    pub executable: Option<String>,
    pub python_version: Option<String>,
    pub cadquery: Option<String>,
    pub cad_to_dagmc: Option<String>,
    pub h5py: Option<String>,
    pub openmc: Option<String>,
    pub openmc_dagmc: Option<bool>,
    pub openmc_package_dir: Option<String>,
    pub openmc_library: Option<String>,
    pub openmc_error: Option<String>,
    pub plasma_source_dir: Option<String>,
    pub cross_sections: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probed {
    Answered(Box<ProbeReport>),
    /// The interpreter did not answer: why.
    Broken(String),
}

/// A candidate with its answer, and whether its OpenMC library carries the aiming markers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub candidate: Candidate,
    pub probe: Probed,
    /// `None`: no OpenMC library to read, or it could not be read.
    pub aiming: Option<bool>,
}

impl Found {
    pub fn report(&self) -> Option<&ProbeReport> {
        match &self.probe {
            Probed::Answered(r) => Some(r),
            Probed::Broken(_) => None,
        }
    }

    /// One plain line of what the interpreter has.
    pub fn summary(&self) -> String {
        let Some(r) = self.report() else {
            let Probed::Broken(why) = &self.probe else {
                unreachable!()
            };
            return format!("did not answer: {why}");
        };
        let mut parts = Vec::new();
        let name = |label: &str, v: &Option<String>| {
            v.as_ref().map(|v| {
                if v.is_empty() {
                    label.to_string()
                } else {
                    format!("{label} {v}")
                }
            })
        };
        parts.extend(name("cadquery", &r.cadquery));
        parts.extend(name("cad_to_dagmc", &r.cad_to_dagmc));
        parts.extend(name("h5py", &r.h5py));
        if let Some(v) = &r.openmc {
            let dagmc = match r.openmc_dagmc {
                Some(true) => " with DAGMC",
                Some(false) => " without DAGMC",
                None => "",
            };
            let aim = if self.aiming == Some(true) {
                ", aiming build"
            } else {
                ""
            };
            parts.push(format!("openmc {v}{dagmc}{aim}"));
        }
        if r.plasma_source_dir.is_some() {
            parts.push("openmc_plasma_source".into());
        }
        let version = r
            .python_version
            .as_deref()
            .map(|v| format!("Python {v}: "))
            .unwrap_or_default();
        if parts.is_empty() {
            format!("{version}none of the packages FARIS uses")
        } else {
            format!("{version}{}", parts.join(", "))
        }
    }
}

/// The last line of `text` that looks like a JSON object.
fn last_json_line(text: &str) -> Option<&str> {
    text.lines().rev().find(|l| l.trim_start().starts_with('{'))
}

/// Turns what a probe printed into an answer.
pub fn parse_probe(stdout: &str) -> Probed {
    match last_json_line(stdout) {
        None => Probed::Broken("it printed no answer".into()),
        Some(line) => match serde_json::from_str::<ProbeReport>(line) {
            Ok(report) => Probed::Answered(Box::new(report)),
            Err(e) => Probed::Broken(format!("its answer could not be read ({e})")),
        },
    }
}

/// Asks one interpreter, under the engine's job runner: bounded time, output and memory. A
/// broken interpreter is an answer (`Broken`), not an error.
pub fn probe(python: &Path, scratch: &Path, cancel: &Cancellation) -> Probed {
    let mut environment: Vec<(OsString, OsString)> = vec![
        ("PYTHONDONTWRITEBYTECODE".into(), "1".into()),
        ("OMP_NUM_THREADS".into(), "1".into()),
        ("OPENBLAS_NUM_THREADS".into(), "1".into()),
        ("HOME".into(), scratch.as_os_str().to_owned()),
    ];
    if let Some(value) = std::env::var_os("OPENMC_CROSS_SECTIONS") {
        environment.push(("OPENMC_CROSS_SECTIONS".into(), value));
    }
    let program = std::path::absolute(python).unwrap_or_else(|_| python.to_path_buf());
    let spec = JobSpec {
        program,
        arguments: vec!["-I".into(), "-c".into(), PROBE_SCRIPT.into()],
        working_directory: scratch.to_path_buf(),
        environment,
        timeout: PROBE_TIMEOUT,
        capture_limit_bytes: 64 * 1024,
        artifact_roots: Vec::new(),
        resource_limits: ResourceLimits::DEFAULT,
    };
    match run_job(&spec, cancel) {
        Err(e) => Probed::Broken(e.to_string()),
        Ok(result) => match result.execution_status {
            ExecutionStatus::Succeeded => parse_probe(&result.stdout),
            ExecutionStatus::TimedOut => Probed::Broken(format!(
                "it did not answer within {} s",
                PROBE_TIMEOUT.as_secs()
            )),
            ExecutionStatus::Cancelled => Probed::Broken("the search was cancelled".into()),
            _ => {
                let why = result
                    .stderr
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("it exited with an error and no message");
                Probed::Broken(why.chars().take(200).collect())
            }
        },
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    let Some(&first) = needle.first() else {
        return true;
    };
    let mut from = 0;
    while let Some(offset) = haystack[from..].iter().position(|&b| b == first) {
        let at = from + offset;
        if haystack[at..].starts_with(needle) {
            return true;
        }
        from = at + 1;
    }
    false
}

/// True when the library holds every aiming marker. The library is read as bytes, never loaded.
/// `None` when it cannot be read.
pub fn library_has_aiming(library: &Path) -> Option<bool> {
    let meta = std::fs::metadata(library).ok()?;
    if !meta.is_file() || meta.len() > MAX_LIBRARY_BYTES {
        return None;
    }
    let data = std::fs::read(library).ok()?;
    Some(AIMING_MARKERS.iter().all(|m| contains(&data, m)))
}

// ----------------------------------------------------------------- choosing --

/// One value offered for a tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub value: String,
    /// Where it was found.
    pub origin: String,
    /// What it has, in plain words.
    pub detail: String,
}

/// What was found for a tool. With several equal offers (`ambiguous`) the user chooses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    pub tool: Tool,
    pub offers: Vec<Offer>,
    pub ambiguous: bool,
}

/// A tool nothing was found for: why, and what to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotFound {
    pub tool: Tool,
    pub why: String,
    pub next_step: String,
}

/// Everything a search produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub looked_at: Vec<Found>,
    pub proposals: Vec<Proposal>,
    pub not_found: Vec<NotFound>,
    pub skipped: usize,
}

/// `0.16.0`, `0.15.3.dev12+gabc` -> `(0, 16, 0)`, `(0, 15, 3)`.
pub fn version_tuple(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.trim().split('.').map(|p| {
        let digits: String = p.chars().take_while(char::is_ascii_digit).collect();
        digits.parse::<u32>().ok()
    });
    let major = parts.next()??;
    let minor = parts.next().flatten().unwrap_or(0);
    let patch = parts.next().flatten().unwrap_or(0);
    Some((major, minor, patch))
}

fn has(v: &Option<String>) -> bool {
    v.is_some()
}

fn exists_file(text: &str) -> bool {
    Path::new(text).is_file()
}

/// Best first by `rank`; the earliest candidate wins a tie. Rank 0 is unusable.
fn best(found: &[Found], rank: impl Fn(&Found, &ProbeReport) -> u32) -> Option<&Found> {
    let mut chosen: Option<(u32, &Found)> = None;
    for f in found {
        let Some(r) = f.report() else { continue };
        let score = rank(f, r);
        if score > 0 && chosen.is_none_or(|(s, _)| score > s) {
            chosen = Some((score, f));
        }
    }
    chosen.map(|(_, f)| f)
}

fn offer(f: &Found, value: String, detail: String) -> Offer {
    Offer {
        value,
        origin: f.candidate.origin.clone(),
        detail,
    }
}

/// Picks a value for each tool from the answers. Pure: the files it is shown need not exist
/// except where a path must be a file (cross sections), which it checks on disk.
pub fn choose(found: &[Found], cross: &[Located], plasma: &[Located]) -> Outcome {
    let mut proposals = Vec::new();
    let mut not_found = Vec::new();
    let asked = found.len();
    let python_of = |f: &Found| f.candidate.python.display().to_string();

    // CAD Python: cadquery is needed to read the STEP file; cad_to_dagmc and h5py as well to
    // check the design.
    let cad = best(found, |_, r| {
        if !has(&r.cadquery) {
            0
        } else {
            1 + u32::from(has(&r.cad_to_dagmc)) + u32::from(has(&r.h5py))
        }
    });
    match cad {
        Some(f) => {
            let r = f.report().expect("chosen from answers");
            let complete = has(&r.cad_to_dagmc) && has(&r.h5py);
            let detail = if complete {
                f.summary()
            } else {
                format!(
                    "{}. It can read a STEP file; checking a design also needs cad_to_dagmc and h5py in the same Python",
                    f.summary()
                )
            };
            proposals.push(Proposal {
                tool: Tool::CadPython,
                offers: vec![offer(f, python_of(f), detail)],
                ambiguous: false,
            });
        }
        None => not_found.push(NotFound {
            tool: Tool::CadPython,
            why: format!("None of the {asked} Python(s) looked at has cadquery."),
            next_step: "install cadquery and cad_to_dagmc in a Python environment, or add the folder holding that environment under \"Also search\"."
                .into(),
        }),
    }

    // OpenMC Python: the transport needs OpenMC 0.15.x with DAGMC; the checks need any OpenMC
    // with h5py.
    let openmc = best(found, |_, r| {
        let Some(v) = r.openmc.as_deref() else {
            return 0;
        };
        let tier = match (r.openmc_dagmc, version_tuple(v)) {
            (Some(true), Some((0, 15, _))) => 3,
            (Some(true), _) => 2,
            _ => 1,
        };
        tier * 2 + u32::from(has(&r.h5py))
    });
    match openmc {
        Some(f) => {
            let r = f.report().expect("chosen from answers");
            let ready = r.openmc_dagmc == Some(true)
                && r.openmc.as_deref().and_then(version_tuple).map(|v| (v.0, v.1)) == Some((0, 15));
            let detail = if ready {
                f.summary()
            } else {
                format!(
                    "{}. The flux run needs OpenMC 0.15 built with DAGMC; this one may only serve the import checks",
                    f.summary()
                )
            };
            proposals.push(Proposal {
                tool: Tool::OpenmcPython,
                offers: vec![offer(f, python_of(f), detail)],
                ambiguous: false,
            });
        }
        None => not_found.push(NotFound {
            tool: Tool::OpenmcPython,
            why: format!("None of the {asked} Python(s) looked at has openmc."),
            next_step: "install OpenMC 0.15 with DAGMC in a Python environment (for example with conda or micromamba), or add the folder holding that environment under \"Also search\"."
                .into(),
        }),
    }

    // Cross sections: the chosen OpenMC's own configuration is authoritative; otherwise every
    // library found is offered and the user chooses (the library changes the answer).
    let mut cross_offers: Vec<Offer> = Vec::new();
    let own = openmc
        .and_then(|f| {
            f.report()
                .and_then(|r| r.cross_sections.clone())
                .map(|v| (f, v))
        })
        .filter(|(_, v)| exists_file(v));
    if let Some((f, v)) = own {
        cross_offers.push(offer(
            f,
            v,
            "the library that OpenMC is configured with (OPENMC_CROSS_SECTIONS)".into(),
        ));
    } else {
        let mut seen = HashSet::new();
        for f in found {
            if let Some(v) = f.report().and_then(|r| r.cross_sections.clone())
                && exists_file(&v)
                && seen.insert(v.clone())
            {
                cross_offers.push(offer(
                    f,
                    v,
                    "the library this Python is configured with".into(),
                ));
            }
        }
        for l in cross {
            let v = l.path.display().to_string();
            if seen.insert(v.clone()) {
                cross_offers.push(Offer {
                    value: v,
                    origin: l.origin.clone(),
                    detail: "a nuclear-data library".into(),
                });
            }
        }
    }
    match cross_offers.len() {
        0 => not_found.push(NotFound {
            tool: Tool::CrossSections,
            why: "No cross_sections.xml was found: none of the Pythons is configured with one (OPENMC_CROSS_SECTIONS), and no search folder holds one.".into(),
            next_step: "download a nuclear-data library for OpenMC and choose its cross_sections.xml, or add the folder that holds it under \"Also search\"."
                .into(),
        }),
        n => proposals.push(Proposal {
            tool: Tool::CrossSections,
            offers: cross_offers,
            ambiguous: n > 1,
        }),
    }

    // Plasma source folder: the chosen OpenMC's own, else every folder found.
    let mut plasma_offers: Vec<Offer> = Vec::new();
    let own = openmc.and_then(|f| {
        f.report()
            .and_then(|r| r.plasma_source_dir.clone())
            .map(|v| (f, v))
    });
    if let Some((f, v)) = own {
        plasma_offers.push(offer(
            f,
            v,
            "openmc_plasma_source is installed in this Python".into(),
        ));
    } else {
        let mut seen = HashSet::new();
        for f in found {
            if let Some(v) = f.report().and_then(|r| r.plasma_source_dir.clone())
                && seen.insert(v.clone())
            {
                plasma_offers.push(offer(
                    f,
                    v,
                    "openmc_plasma_source is installed in this Python".into(),
                ));
            }
        }
        for l in plasma {
            let v = l.path.display().to_string();
            if seen.insert(v.clone()) {
                plasma_offers.push(Offer {
                    value: v,
                    origin: l.origin.clone(),
                    detail: "holds openmc_plasma_source".into(),
                });
            }
        }
    }
    match plasma_offers.len() {
        0 => not_found.push(NotFound {
            tool: Tool::PlasmaSource,
            why: "No folder holding openmc_plasma_source was found.".into(),
            next_step: "install openmc_plasma_source (or place its folder somewhere) and add that folder under \"Also search\", or choose it yourself."
                .into(),
        }),
        n => proposals.push(Proposal {
            tool: Tool::PlasmaSource,
            offers: plasma_offers,
            ambiguous: n > 1,
        }),
    }

    // Window Python: OpenMC 0.16 or later, DAGMC preferred.
    let window = best(found, |_, r| {
        match r.openmc.as_deref().and_then(version_tuple) {
            Some(v) if v >= (0, 16, 0) => 1 + u32::from(r.openmc_dagmc == Some(true)),
            _ => 0,
        }
    });
    match window {
        Some(f) => proposals.push(Proposal {
            tool: Tool::WindowPython,
            offers: vec![offer(f, python_of(f), f.summary())],
            ambiguous: false,
        }),
        None => not_found.push(NotFound {
            tool: Tool::WindowPython,
            why: "No Python with OpenMC 0.16 or later was found.".into(),
            next_step: "none needed; without it the flux run is analog and needs many more histories. Install OpenMC 0.16 in a separate environment to speed it up."
                .into(),
        }),
    }

    // OpenMC package folder: a build carrying the aiming markers.
    let aiming = found.iter().find(|f| {
        f.aiming == Some(true) && f.report().is_some_and(|r| r.openmc_package_dir.is_some())
    });
    match aiming {
        Some(f) => {
            let package = f
                .report()
                .and_then(|r| r.openmc_package_dir.as_deref())
                .expect("checked above");
            let folder = Path::new(package)
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| package.to_string());
            proposals.push(Proposal {
                tool: Tool::OpenmcPackage,
                offers: vec![offer(
                    f,
                    folder,
                    "an OpenMC whose library carries the aiming patch".into(),
                )],
                ambiguous: false,
            });
        }
        None => not_found.push(NotFound {
            tool: Tool::OpenmcPackage,
            why: "No OpenMC build with the aiming patch was found.".into(),
            next_step: "none needed; without it the run cannot aim at a duct that streams neutrons to the magnets, so those runs are slower with the same results."
                .into(),
        }),
    }

    // Keep the tool order the windows use.
    proposals.sort_by_key(|p| p.tool);
    not_found.sort_by_key(|n| n.tool);
    Outcome {
        looked_at: found.to_vec(),
        proposals,
        not_found,
        skipped: 0,
    }
}

/// Looks, asks and chooses. `progress` is told what is being asked; `cancel` stops it between
/// (and during) questions.
pub fn find_tools(
    roots: &SearchRoots,
    cancel: &Cancellation,
    progress: &dyn Fn(String),
) -> Result<Outcome, String> {
    progress("Looking for Python environments\u{2026}".into());
    let scanned = scan(roots);
    let scratch = tempfile::Builder::new()
        .prefix("faris-find-tools-")
        .tempdir()
        .map_err(|e| format!("cannot make a scratch folder: {e}"))?;
    let total = scanned.candidates.len();
    let mut found = Vec::with_capacity(total);
    for (i, candidate) in scanned.candidates.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }
        progress(format!(
            "Asking Python {} of {total}: {}",
            i + 1,
            candidate.python.display()
        ));
        let probed = probe(&candidate.python, scratch.path(), cancel);
        let aiming = match &probed {
            Probed::Answered(r) => r
                .openmc_library
                .as_deref()
                .and_then(|lib| library_has_aiming(Path::new(lib))),
            Probed::Broken(_) => None,
        };
        found.push(Found {
            candidate: candidate.clone(),
            probe: probed,
            aiming,
        });
    }
    if cancel.is_cancelled() {
        return Err("cancelled".into());
    }
    let mut outcome = choose(&found, &scanned.cross_sections, &scanned.plasma_sources);
    outcome.skipped = scanned.skipped;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(f: impl FnOnce(&mut ProbeReport)) -> Probed {
        let mut r = ProbeReport {
            python_version: Some("3.13.1".into()),
            ..ProbeReport::default()
        };
        f(&mut r);
        Probed::Answered(Box::new(r))
    }

    fn found(name: &str, probe: Probed) -> Found {
        Found {
            candidate: Candidate {
                python: PathBuf::from(format!("/envs/{name}/bin/python")),
                origin: format!("environment {name}"),
            },
            probe,
            aiming: None,
        }
    }

    fn value_of(outcome: &Outcome, tool: Tool) -> Option<String> {
        outcome
            .proposals
            .iter()
            .find(|p| p.tool == tool)
            .map(|p| p.offers[0].value.clone())
    }

    #[test]
    fn each_tool_gets_the_best_matching_environment() {
        let cad = found(
            "cad",
            report(|r| {
                r.cadquery = Some("2.5".into());
                r.cad_to_dagmc = Some("0.9".into());
                r.h5py = Some("3.11".into());
            }),
        );
        let cad_only = found("cadlite", report(|r| r.cadquery = Some("2.4".into())));
        let omc15 = found(
            "omc15",
            report(|r| {
                r.openmc = Some("0.15.3".into());
                r.openmc_dagmc = Some(true);
                r.h5py = Some("3.11".into());
            }),
        );
        let omc16 = found(
            "omc16",
            report(|r| {
                r.openmc = Some("0.16.0".into());
                r.openmc_dagmc = Some(true);
                r.h5py = Some("3.11".into());
            }),
        );
        let broken = found("broken", Probed::Broken("no such file".into()));
        let outcome = choose(&[cad_only, broken, omc16, cad, omc15], &[], &[]);
        assert_eq!(
            value_of(&outcome, Tool::CadPython).as_deref(),
            Some("/envs/cad/bin/python"),
            "cadquery with cad_to_dagmc and h5py beats cadquery alone"
        );
        assert_eq!(
            value_of(&outcome, Tool::OpenmcPython).as_deref(),
            Some("/envs/omc15/bin/python"),
            "the transport wants 0.15 with DAGMC, not the newest"
        );
        assert_eq!(
            value_of(&outcome, Tool::WindowPython).as_deref(),
            Some("/envs/omc16/bin/python")
        );
        assert_eq!(outcome.looked_at.len(), 5);
    }

    #[test]
    fn the_earlier_candidate_wins_a_tie() {
        let make = |name: &str| {
            found(
                name,
                report(|r| {
                    r.cadquery = Some("2".into());
                    r.cad_to_dagmc = Some("1".into());
                    r.h5py = Some("3".into());
                }),
            )
        };
        let outcome = choose(&[make("first"), make("second")], &[], &[]);
        assert_eq!(
            value_of(&outcome, Tool::CadPython).as_deref(),
            Some("/envs/first/bin/python")
        );
    }

    #[test]
    fn a_python_with_only_part_of_the_cad_stack_says_what_it_lacks() {
        let outcome = choose(
            &[found(
                "cadlite",
                report(|r| r.cadquery = Some("2.4".into())),
            )],
            &[],
            &[],
        );
        let offer = &outcome
            .proposals
            .iter()
            .find(|p| p.tool == Tool::CadPython)
            .unwrap()
            .offers[0];
        assert!(
            offer.detail.contains("cad_to_dagmc and h5py"),
            "{}",
            offer.detail
        );
    }

    #[test]
    fn nothing_found_says_why_and_what_to_do() {
        let outcome = choose(&[found("bare", report(|_| {}))], &[], &[]);
        assert!(outcome.proposals.is_empty());
        assert_eq!(outcome.not_found.len(), 6);
        for n in &outcome.not_found {
            assert!(!n.why.is_empty() && !n.next_step.is_empty(), "{:?}", n.tool);
        }
        let optional = outcome
            .not_found
            .iter()
            .find(|n| n.tool == Tool::WindowPython)
            .unwrap();
        assert!(optional.next_step.contains("none needed"));
    }

    #[test]
    fn openmc_below_0_16_is_not_a_window_python() {
        let old = found(
            "old",
            report(|r| {
                r.openmc = Some("0.15.3".into());
                r.openmc_dagmc = Some(true);
            }),
        );
        let outcome = choose(&[old], &[], &[]);
        assert_eq!(value_of(&outcome, Tool::WindowPython), None);
        assert!(value_of(&outcome, Tool::OpenmcPython).is_some());
    }

    #[test]
    fn openmc_without_dagmc_is_offered_with_a_warning() {
        let plain = found(
            "plain",
            report(|r| {
                r.openmc = Some("0.15.3".into());
                r.openmc_dagmc = Some(false);
            }),
        );
        let outcome = choose(&[plain], &[], &[]);
        let offer = &outcome.proposals[0].offers[0];
        assert!(offer.detail.contains("DAGMC"), "{}", offer.detail);
        assert!(offer.detail.contains("without DAGMC"), "{}", offer.detail);
    }

    #[test]
    fn the_aiming_package_is_the_folder_above_a_marked_library() {
        let mut marked = found(
            "aim",
            report(|r| {
                r.openmc = Some("0.15.3".into());
                r.openmc_package_dir = Some("/envs/aim/lib/python3.13/site-packages/openmc".into());
                r.openmc_library = Some("/x/libopenmc.so".into());
            }),
        );
        marked.aiming = Some(true);
        let mut unmarked = found(
            "stock",
            report(|r| {
                r.openmc = Some("0.15.3".into());
                r.openmc_package_dir = Some("/envs/stock/lib/site-packages/openmc".into());
            }),
        );
        unmarked.aiming = Some(false);
        let outcome = choose(&[unmarked.clone(), marked], &[], &[]);
        assert_eq!(
            value_of(&outcome, Tool::OpenmcPackage).as_deref(),
            Some("/envs/aim/lib/python3.13/site-packages")
        );
        let outcome = choose(&[unmarked], &[], &[]);
        assert_eq!(value_of(&outcome, Tool::OpenmcPackage), None);
    }

    #[test]
    fn cross_sections_from_the_chosen_openmc_win_and_others_are_left_to_the_user() {
        let dir = tempfile::tempdir().unwrap();
        let own = dir.path().join("own.xml");
        let other = dir.path().join("other.xml");
        std::fs::write(&own, "").unwrap();
        std::fs::write(&other, "").unwrap();
        let with_own = found(
            "omc",
            report(|r| {
                r.openmc = Some("0.15.3".into());
                r.openmc_dagmc = Some(true);
                r.cross_sections = Some(own.display().to_string());
            }),
        );
        let located = [Located {
            path: other.clone(),
            origin: "a folder".into(),
        }];
        let outcome = choose(std::slice::from_ref(&with_own), &located, &[]);
        let cross = outcome
            .proposals
            .iter()
            .find(|p| p.tool == Tool::CrossSections)
            .unwrap();
        assert_eq!(cross.offers.len(), 1);
        assert!(!cross.ambiguous);
        assert_eq!(cross.offers[0].value, own.display().to_string());

        // Without a configured library, two found libraries are the user's call.
        let bare = found("bare", report(|r| r.openmc = Some("0.15.3".into())));
        let two = [
            Located {
                path: other,
                origin: "a".into(),
            },
            Located {
                path: own,
                origin: "b".into(),
            },
        ];
        let outcome = choose(&[bare], &two, &[]);
        let cross = outcome
            .proposals
            .iter()
            .find(|p| p.tool == Tool::CrossSections)
            .unwrap();
        assert!(cross.ambiguous);
        assert_eq!(cross.offers.len(), 2);

        // A configured library that no longer exists is not offered.
        let stale = found(
            "stale",
            report(|r| {
                r.openmc = Some("0.15.3".into());
                r.cross_sections = Some("/no/such/cross_sections.xml".into());
            }),
        );
        let outcome = choose(&[stale], &[], &[]);
        assert!(
            outcome
                .not_found
                .iter()
                .any(|n| n.tool == Tool::CrossSections)
        );
    }

    #[test]
    fn plasma_source_prefers_the_chosen_pythons_own_install() {
        let omc = found(
            "omc",
            report(|r| {
                r.openmc = Some("0.15.3".into());
                r.openmc_dagmc = Some(true);
                r.plasma_source_dir = Some("/envs/omc/site-packages".into());
            }),
        );
        let located = [Located {
            path: PathBuf::from("/elsewhere"),
            origin: "a folder".into(),
        }];
        let outcome = choose(&[omc], &[], &located);
        let plasma = outcome
            .proposals
            .iter()
            .find(|p| p.tool == Tool::PlasmaSource)
            .unwrap();
        assert!(!plasma.ambiguous);
        assert_eq!(plasma.offers[0].value, "/envs/omc/site-packages");
    }

    #[test]
    fn versions_parse_with_suffixes() {
        assert_eq!(version_tuple("0.16.0"), Some((0, 16, 0)));
        assert_eq!(version_tuple("0.15.3.dev12+gabc"), Some((0, 15, 3)));
        assert_eq!(version_tuple("0.15"), Some((0, 15, 0)));
        assert_eq!(version_tuple("dev"), None);
        assert_eq!(version_tuple(""), None);
    }

    #[test]
    fn probe_output_is_read_from_its_last_json_line() {
        let out =
            "warning: something\n{\"cadquery\": \"2.5\", \"openmc\": null, \"unknown_key\": 1}\n";
        let Probed::Answered(r) = parse_probe(out) else {
            panic!("should parse");
        };
        assert_eq!(r.cadquery.as_deref(), Some("2.5"));
        assert_eq!(r.openmc, None);
        assert!(matches!(parse_probe("nothing here"), Probed::Broken(_)));
        assert!(matches!(parse_probe("{ broken"), Probed::Broken(_)));
    }

    #[test]
    fn the_aiming_markers_are_read_as_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let mut data = vec![0u8; 1000];
        for m in AIMING_MARKERS {
            data.extend_from_slice(b"\0junk OPENMC_ ");
            data.extend_from_slice(m);
        }
        let full = dir.path().join("libfull.so");
        std::fs::write(&full, &data).unwrap();
        assert_eq!(library_has_aiming(&full), Some(true));
        let partial = dir.path().join("libpartial.so");
        std::fs::write(&partial, b"OPENMC_APERTURE_BIAS only").unwrap();
        assert_eq!(library_has_aiming(&partial), Some(false));
        assert_eq!(library_has_aiming(&dir.path().join("missing.so")), None);
    }

    /// The marker list must stay the one the aiming script checks.
    #[test]
    fn the_markers_match_the_aiming_script() {
        let script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../integrations/openmc/streaming_aim.py");
        let Ok(text) = std::fs::read_to_string(&script) else {
            return;
        };
        for m in AIMING_MARKERS {
            let name = std::str::from_utf8(m).unwrap();
            assert!(text.contains(name), "{name} is not in streaming_aim.py");
        }
    }

    fn make_env(dir: &Path) {
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("python"), "").unwrap();
    }

    #[test]
    fn the_search_lists_environments_in_the_usual_places() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        make_env(&h.join("micromamba/envs/openmc016"));
        make_env(&h.join("miniforge3/envs/cad"));
        make_env(&h.join(".venvs/thermal"));
        make_env(&h.join("miniconda3"));
        std::fs::create_dir_all(h.join("micromamba/envs/not-an-env/lib")).unwrap();
        let on_path = tempfile::tempdir().unwrap();
        std::fs::write(on_path.path().join("python3"), "").unwrap();
        let roots = SearchRoots {
            home: Some(h.to_path_buf()),
            path_dirs: vec![on_path.path().to_path_buf(), h.join("missing")],
            conda_prefix: Some(h.join("miniforge3/envs/cad")),
            ..SearchRoots::default()
        };
        let scan = scan(&roots);
        let origins: Vec<&str> = scan.candidates.iter().map(|c| c.origin.as_str()).collect();
        assert!(
            origins[0].contains("active conda environment"),
            "{origins:?}"
        );
        assert!(origins.iter().any(|o| o.contains("python3 on PATH")));
        assert!(origins.iter().any(|o| o.contains("environment openmc016")));
        assert!(origins.iter().any(|o| o.contains("virtualenv thermal")));
        assert!(
            origins
                .iter()
                .any(|o| o.contains("base environment of ~/miniconda3"))
        );
        assert!(!origins.iter().any(|o| o.contains("not-an-env")));
        // The active environment is also under miniforge3/envs: listed once.
        assert_eq!(
            scan.candidates
                .iter()
                .filter(|c| c.python.ends_with("cad/bin/python"))
                .count(),
            1
        );
    }

    #[test]
    fn extra_folders_are_searched_two_levels_down() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        make_env(&r.join("study/cadvenv"));
        std::fs::create_dir_all(r.join("data/endf")).unwrap();
        std::fs::write(r.join("data/endf/cross_sections.xml"), "").unwrap();
        std::fs::create_dir_all(r.join("lib/openmc_plasma_source")).unwrap();
        std::fs::write(r.join("lib/openmc_plasma_source/__init__.py"), "").unwrap();
        std::fs::create_dir_all(r.join("a/b/c/d")).unwrap();
        make_env(&r.join("a/b/c/d"));
        let roots = SearchRoots {
            extra: vec![r.to_path_buf()],
            ..SearchRoots::default()
        };
        let scan = scan(&roots);
        assert_eq!(scan.candidates.len(), 1, "{:?}", scan.candidates);
        assert!(scan.candidates[0].python.ends_with("cadvenv/bin/python"));
        assert_eq!(scan.cross_sections.len(), 1);
        assert!(
            scan.cross_sections[0]
                .path
                .ends_with("endf/cross_sections.xml")
        );
        assert_eq!(scan.plasma_sources.len(), 1);
        assert!(scan.plasma_sources[0].path.ends_with("lib"));
    }

    #[test]
    fn the_candidate_list_is_bounded() {
        let home = tempfile::tempdir().unwrap();
        for i in 0..(MAX_CANDIDATES + 5) {
            make_env(&home.path().join(format!(".venvs/v{i:03}")));
        }
        let roots = SearchRoots {
            home: Some(home.path().to_path_buf()),
            ..SearchRoots::default()
        };
        let scan = scan(&roots);
        assert_eq!(scan.candidates.len(), MAX_CANDIDATES);
        assert_eq!(scan.skipped, 5);
    }

    #[test]
    fn home_is_expanded_in_search_folders() {
        assert_eq!(
            expand_home("~/envs", Some(Path::new("/home/u"))),
            PathBuf::from("/home/u/envs")
        );
        assert_eq!(
            expand_home("/abs", Some(Path::new("/home/u"))),
            PathBuf::from("/abs")
        );
    }

    /// The probe script is valid Python (skipped without a `python3`).
    #[test]
    fn the_probe_script_compiles() {
        let Ok(dir) = tempfile::tempdir() else { return };
        let script = dir.path().join("probe.py");
        std::fs::write(&script, PROBE_SCRIPT).unwrap();
        let ran = std::process::Command::new("python3")
            .args([
                "-I",
                "-c",
                "import ast,sys; ast.parse(open(sys.argv[1]).read())",
            ])
            .arg(&script)
            .output();
        match ran {
            Err(_) => eprintln!("python3 is not installed; the probe script was not compiled"),
            Ok(out) => assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            ),
        }
    }

    /// With a real `python3` the probe answers and the answer parses; a missing interpreter and
    /// a non-Python program are answers, not errors. Runs only a trivial import-free question.
    #[test]
    fn a_broken_interpreter_is_an_answer_not_a_hang() {
        let scratch = tempfile::tempdir().unwrap();
        let cancel = Cancellation::default();
        let missing = probe(Path::new("/no/such/python"), scratch.path(), &cancel);
        assert!(matches!(missing, Probed::Broken(_)), "{missing:?}");
        // A program that is not an interpreter and exits with an error.
        if Path::new("/bin/false").is_file() {
            let false_program = probe(Path::new("/bin/false"), scratch.path(), &cancel);
            assert!(
                matches!(false_program, Probed::Broken(_)),
                "{false_program:?}"
            );
        }
    }
}
