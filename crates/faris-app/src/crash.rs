//! Crash records and the diagnostics bundle (REL-004, REL-040 to REL-043, LEG-027).
//!
//! When the UI thread panics, a small record is written to the `crashes` folder
//! of the per-user configuration folder and the newest recovery state is saved
//! (see `recovery`). The record is built from an allow-list: the FARIS version,
//! the operating system and architecture, which kind of thread panicked, an
//! error code, the source location inside FARIS or a library, and the function
//! names of the stack. It never holds the panic message (which can quote a file
//! name or a value from a study), a path, a study name or any geometry. Nothing
//! is sent anywhere: there is no upload code in this program, and the records
//! are deleted after `crash.keep_days` days (30 by default) or when more than
//! [`MAX_RECORDS`] pile up, or at once with Clear now.
//!
//! The diagnostics bundle is one JSON file the user can read before deciding
//! what to do with it: what it holds is listed first, and a scan removes
//! anything that looks like a path, an address, a key or the user's own names.

use crate::recovery::{self, EmergencyOutcome};
use faris_engine::atomic_file;
use serde::{Deserialize, Serialize};
use std::{
    cell::Cell,
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread::ThreadId,
};

pub const SCHEMA: &str = "faris-crash-record/1";
pub const DEFAULT_KEEP_DAYS: u64 = 30;
/// The most records kept, whatever their age.
pub const MAX_RECORDS: usize = 20;
const MAX_STACK_FRAMES: usize = 48;
const MAX_SYMBOL_CHARS: usize = 160;
/// The most a diagnostics bundle may hold.
pub const BUNDLE_LIMIT_BYTES: usize = 20 * 1024 * 1024;

pub const UI_PANIC_CODE: &str = "FARIS-CRASH-UI";
pub const WORKER_PANIC_CODE: &str = "FARIS-CRASH-WORKER";

pub fn dir_in(config: &Path) -> PathBuf {
    config.join("crashes")
}

pub fn diagnostics_dir_in(config: &Path) -> PathBuf {
    config.join("diagnostics")
}

/// Everything a crash record may hold.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrashRecord {
    pub schema: String,
    pub id: String,
    pub written_unix: u64,
    pub faris_version: String,
    pub os: String,
    pub arch: String,
    /// `ui-panic` or `worker-panic`.
    pub kind: String,
    pub error_code: String,
    /// Where in FARIS or a library the panic happened, relative to its source.
    pub location: String,
    /// Function names only, innermost first.
    pub stack: Vec<String>,
    /// `written`, `not needed` or `failed`.
    pub emergency_autosave: String,
}

/// What the hook learns about a panic. The message is deliberately not here.
#[derive(Clone, Debug, Default)]
pub struct PanicFacts {
    pub ui_thread: bool,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub backtrace_text: String,
}

fn looks_like_path(text: &str) -> bool {
    text.starts_with('/')
        || text.starts_with('~')
        || text.starts_with("\\\\")
        || text.as_bytes().get(1) == Some(&b':')
}

fn clean_symbol(symbol: &str) -> Option<String> {
    let symbol = symbol.trim();
    if symbol.is_empty() || symbol.contains('/') || symbol.contains('\\') || looks_like_path(symbol)
    {
        return None;
    }
    let ok = |c: char| c.is_ascii_alphanumeric() || " _:<>&*,[]()'{}.+-#$=;".contains(c);
    let kept: String = symbol
        .chars()
        .filter(|c| ok(*c))
        .take(MAX_SYMBOL_CHARS)
        .collect();
    (!kept.is_empty()).then_some(kept)
}

/// The function names in a printed backtrace, dropping every `at file:line` line.
pub fn stack_from_backtrace(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let (number, symbol) = line.trim_start().split_once(':')?;
            number.trim().parse::<u32>().ok()?;
            clean_symbol(symbol)
        })
        .take(MAX_STACK_FRAMES)
        .collect()
}

/// A source location with the machine's own folders removed: workspace sources
/// as `crates/...`, libraries as `crate-version/src/...`, the standard library as `std ...`.
pub fn location_text(file: Option<&str>, line: Option<u32>) -> String {
    let Some(file) = file else {
        return "unknown".into();
    };
    let file = file.replace('\\', "/");
    let relative = if let Some((_, rest)) = file.split_once("/registry/src/") {
        // Skip the registry's own folder name.
        rest.split_once('/')
            .map(|(_, dep)| format!("dependency {dep}"))
    } else if let Some((_, rest)) = file.split_once("/library/") {
        file.contains("/rustc/")
            .then(|| format!("std library/{rest}"))
    } else if file.starts_with("crates/") {
        Some(file.clone())
    } else {
        file.split_once("/crates/")
            .map(|(_, rest)| format!("crates/{rest}"))
    };
    match relative {
        Some(text)
            if !looks_like_path(&text)
                && text
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || " _./-+".contains(c)) =>
        {
            match line {
                Some(line) => format!("{text}:{line}"),
                None => text,
            }
        }
        _ => "unknown".into(),
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Build the record for a panic. Only allow-listed fields are filled.
pub fn build_record(facts: &PanicFacts, emergency: EmergencyOutcome, now_unix: u64) -> CrashRecord {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    CrashRecord {
        schema: SCHEMA.into(),
        id: format!("{now_unix}-{n}"),
        written_unix: now_unix,
        faris_version: env!("CARGO_PKG_VERSION").into(),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        kind: if facts.ui_thread {
            "ui-panic"
        } else {
            "worker-panic"
        }
        .into(),
        error_code: if facts.ui_thread {
            UI_PANIC_CODE
        } else {
            WORKER_PANIC_CODE
        }
        .into(),
        location: location_text(facts.file.as_deref(), facts.line),
        stack: stack_from_backtrace(&facts.backtrace_text),
        emergency_autosave: emergency.word().into(),
    }
}

pub fn write_record(dir: &Path, record: &CrashRecord) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("cannot create the crash record folder: {e}"))?;
    let path = dir.join(format!("crash-{}.json", record.id));
    let bytes = serde_json::to_vec_pretty(record).map_err(|e| e.to_string())?;
    atomic_file::write_atomic(&path, &bytes)
        .map_err(|e| format!("cannot write the crash record: {e}"))?;
    Ok(path)
}

/// What the hook does for a panic: save the newest recovery state if the UI
/// thread panicked, then write the record. Returns the record's path.
pub fn handle_panic(facts: &PanicFacts, config: &Path, now_unix: u64) -> Result<PathBuf, String> {
    let emergency = if facts.ui_thread {
        recovery::emergency_write()
    } else {
        EmergencyOutcome::NotNeeded
    };
    write_record(&dir_in(config), &build_record(facts, emergency, now_unix))
}

static UI_THREAD: Mutex<Option<ThreadId>> = Mutex::new(None);
static CONFIG: Mutex<Option<PathBuf>> = Mutex::new(None);

thread_local! {
    static IN_HOOK: Cell<bool> = const { Cell::new(false) };
}

/// Install the panic hook. The calling thread is the UI thread. The hook writes a
/// record, saves the newest recovery state when the UI thread is the one that
/// panicked, then runs the hook that was there before (which prints the message
/// to the terminal as usual). It never sends anything.
pub fn install_panic_hook(config: PathBuf) {
    if let Ok(mut ui) = UI_THREAD.lock() {
        *ui = Some(std::thread::current().id());
    }
    if let Ok(mut slot) = CONFIG.lock() {
        *slot = Some(config);
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if !IN_HOOK.with(|flag| flag.replace(true)) {
            let ui_thread = UI_THREAD
                .try_lock()
                .ok()
                .and_then(|slot| *slot)
                .is_some_and(|id| id == std::thread::current().id());
            let config = CONFIG.try_lock().ok().and_then(|slot| slot.clone());
            if let Some(config) = config {
                let location = info.location();
                let facts = PanicFacts {
                    ui_thread,
                    file: location.map(|l| l.file().to_owned()),
                    line: location.map(|l| l.line()),
                    backtrace_text: std::backtrace::Backtrace::force_capture().to_string(),
                };
                let now = recovery_now();
                let _ = std::panic::catch_unwind(|| handle_panic(&facts, &config, now));
            }
            IN_HOOK.with(|flag| flag.set(false));
        }
        previous(info);
    }));
}

fn recovery_now() -> u64 {
    crate::time::SystemTime::now()
        .duration_since(crate::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The age in unix seconds named in a file called `crash-<unix>-<n>.json`.
fn age_from_name(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    name.strip_prefix("crash-")?.split('-').next()?.parse().ok()
}

fn age_from_modified(path: &Path) -> Option<u64> {
    path.metadata()
        .and_then(|m| m.modified())
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

/// The files in `dir`, newest first, with the time each was made.
fn dated_files(dir: &Path, time_of: &dyn Fn(&Path) -> Option<u64>) -> Vec<(PathBuf, u64)> {
    let mut files: Vec<(PathBuf, u64)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| {
            let path = e.path();
            let time = time_of(&path)?;
            Some((path, time))
        })
        .collect();
    files.sort_by_key(|file| std::cmp::Reverse(file.1));
    files
}

/// Delete crash records older than `keep_days` and all but the newest `max_count`.
/// Returns how many were deleted.
pub fn purge_expired(dir: &Path, now_unix: u64, keep_days: u64, max_count: usize) -> usize {
    purge_by(dir, now_unix, keep_days, max_count, &age_from_name)
}

/// The same for files dated by their modification time (recovery files, bundles).
pub fn purge_expired_by_modified(
    dir: &Path,
    now_unix: u64,
    keep_days: u64,
    max_count: usize,
) -> usize {
    purge_by(dir, now_unix, keep_days, max_count, &age_from_modified)
}

fn purge_by(
    dir: &Path,
    now_unix: u64,
    keep_days: u64,
    max_count: usize,
    time_of: &dyn Fn(&Path) -> Option<u64>,
) -> usize {
    let limit = keep_days.saturating_mul(86_400);
    let mut deleted = 0;
    for (index, (path, time)) in dated_files(dir, time_of).into_iter().enumerate() {
        if (index >= max_count || now_unix.saturating_sub(time) > limit)
            && std::fs::remove_file(&path).is_ok()
        {
            deleted += 1;
        }
    }
    deleted
}

/// Delete every crash record (the Clear now button). Returns how many.
pub fn clear_all(dir: &Path) -> usize {
    let mut deleted = 0;
    for (path, _) in dated_files(dir, &|p| age_from_name(p).or(Some(0))) {
        if std::fs::remove_file(path).is_ok() {
            deleted += 1;
        }
    }
    deleted
}

/// Every record in `dir` that this version can read, newest first.
pub fn list_records(dir: &Path) -> Vec<(PathBuf, CrashRecord)> {
    dated_files(dir, &age_from_name)
        .into_iter()
        .filter_map(|(path, _)| {
            let record: CrashRecord = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
            (record.schema == SCHEMA).then_some((path, record))
        })
        .collect()
}

/// Apply the expiry rule to every folder FARIS keeps its own records in (LEG-027).
/// Returns how many files were deleted.
pub fn purge_all(config: &Path, now_unix: u64, keep_days: u64) -> usize {
    purge_expired(&dir_in(config), now_unix, keep_days, MAX_RECORDS)
        + purge_expired_by_modified(
            &diagnostics_dir_in(config),
            now_unix,
            keep_days,
            MAX_RECORDS,
        )
        + purge_expired_by_modified(&recovery::dir_in(config), now_unix, keep_days, MAX_RECORDS)
}

// ---------------------------------------------------------------------------
// The leak scan, shared by the bundle and the tests.

/// One thing the scan does not want in text that leaves a study.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub kind: &'static str,
    pub word: String,
}

const DELIMITERS: &str = " \t\r\n\",[]{}()<>;'`|";

fn word_leak(word: &str, secrets: &[String]) -> Option<&'static str> {
    let lower = word.to_lowercase();
    if secrets
        .iter()
        .any(|s| !s.is_empty() && lower.contains(&s.to_lowercase()))
    {
        return Some("a name or string from the study or the user");
    }
    let bytes = word.as_bytes();
    if word.starts_with('/')
        && bytes
            .get(1)
            .is_some_and(|b| b.is_ascii_alphanumeric() || b"._~-".contains(b))
    {
        return Some("a file path");
    }
    if word == "~" || word.starts_with("~/") {
        return Some("a file path");
    }
    if (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/'))
        || word.starts_with("\\\\")
    {
        return Some("a file path");
    }
    if let Some(at) = word.find('@')
        && at > 0
        && word[at + 1..].contains('.')
    {
        return Some("an e-mail address");
    }
    for key in [
        "password",
        "secret",
        "token",
        "apikey",
        "api_key",
        "authorization",
        "bearer",
    ] {
        if lower.contains(key) {
            return Some("a secret or key");
        }
    }
    if word.len() >= 40
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+/=_-".contains(c))
        && word.chars().any(|c| c.is_ascii_digit())
    {
        return Some("a long key or hash");
    }
    None
}

fn words(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut start = None;
    let mut out = Vec::new();
    for (index, c) in text.char_indices() {
        if DELIMITERS.contains(c) {
            if let Some(s) = start.take() {
                out.push((s, &text[s..index]));
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(s) = start {
        out.push((s, &text[s..]));
    }
    out.into_iter()
}

/// The leaks in `text`. `secrets` are strings that must never appear: the home
/// folder, the user name, study and file names.
#[cfg(test)]
pub fn scan(text: &str, secrets: &[String]) -> Vec<Finding> {
    words(text)
        .filter_map(|(_, word)| {
            let trimmed = word.trim_start_matches([':', '=']);
            word_leak(trimmed, secrets).map(|kind| Finding {
                kind,
                word: trimmed.to_owned(),
            })
        })
        .collect()
}

/// `text` with every leak replaced by `[redacted]`, and how many were replaced.
pub fn redact(text: &str, secrets: &[String]) -> (String, usize) {
    let mut out = text.to_owned();
    let mut count = 0;
    for _ in 0..8 {
        let mut ranges = Vec::new();
        for (start, word) in words(&out) {
            let trimmed = word.trim_start_matches([':', '=']);
            if word_leak(trimmed, secrets).is_some() {
                let begin = start + (word.len() - trimmed.len());
                ranges.push((begin, start + word.len()));
            }
        }
        if ranges.is_empty() {
            break;
        }
        count += ranges.len();
        for (begin, end) in ranges.into_iter().rev() {
            out.replace_range(begin..end, "[redacted]");
        }
    }
    (out, count)
}

// ---------------------------------------------------------------------------
// The diagnostics bundle.

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ManifestEntry {
    pub section: String,
    pub contents: String,
    pub bytes: usize,
}

pub struct Bundle {
    pub json: String,
    pub manifest: Vec<ManifestEntry>,
    pub redactions: usize,
    pub records_included: usize,
}

pub struct BundleInputs<'a> {
    pub config: &'a Path,
    /// Settings that are set, as (key, text); path settings are reduced to "set".
    pub settings: Vec<(String, String)>,
    /// Strings that must not appear in the bundle.
    pub secrets: Vec<String>,
    pub now_unix: u64,
}

/// Build the bundle: the program and system, the settings that are set (paths
/// reduced to "set"), the crash records, and whether a recovery copy exists.
pub fn build_bundle(inputs: &BundleInputs) -> Bundle {
    let mut manifest = Vec::new();
    let mut add = |section: &str, contents: &str, value: &serde_json::Value| {
        manifest.push(ManifestEntry {
            section: section.into(),
            contents: contents.into(),
            bytes: value.to_string().len(),
        });
    };
    let app = serde_json::json!({
        "faris_version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "build": if cfg!(debug_assertions) { "debug" } else { "release" },
        "logical_cpus": crate::thread::available_parallelism().map_or(0, |n| n.get()),
    });
    add(
        "app",
        "FARIS version, operating system, processor type, build kind, processor count",
        &app,
    );
    let settings: serde_json::Map<String, serde_json::Value> = inputs
        .settings
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();
    let settings = serde_json::Value::Object(settings);
    add(
        "settings",
        "the settings that are set; a path is shown only as set",
        &settings,
    );
    let recovery_present = std::fs::read_dir(recovery::dir_in(inputs.config))
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| e.path().extension().is_some_and(|x| x == "json"));
    let recovery_value = serde_json::json!({ "a_recovery_copy_exists": recovery_present });
    add(
        "recovery",
        "whether a recovery copy of unsaved work exists (not its contents)",
        &recovery_value,
    );
    let mut records: Vec<serde_json::Value> = Vec::new();
    let mut size = 0usize;
    for (_, record) in list_records(&dir_in(inputs.config)) {
        let value = serde_json::to_value(&record).unwrap_or_default();
        size += value.to_string().len();
        if size > BUNDLE_LIMIT_BYTES / 2 {
            break;
        }
        records.push(value);
    }
    let included = records.len();
    let records = serde_json::Value::Array(records);
    add(
        "crash_records",
        "the crash records on this computer: version, system, error code, source location, function names",
        &records,
    );
    let mut doc = serde_json::json!({
        "schema": "faris-diagnostics/1",
        "made_unix": inputs.now_unix,
        "note": "Made on this computer and sent nowhere. Read it before you share it. It holds no study content, file names, paths or geometry.",
        "manifest": [],
        "app": app,
        "settings": settings,
        "recovery": recovery_value,
        "crash_records": records,
    });
    doc["manifest"] = serde_json::to_value(&manifest).unwrap_or_default();
    let text = serde_json::to_string_pretty(&doc).unwrap_or_default();
    let (json, redactions) = redact(&text, &inputs.secrets);
    Bundle {
        json,
        manifest,
        redactions,
        records_included: included,
    }
}

/// Write the bundle to the diagnostics folder and return its path.
pub fn save_bundle(config: &Path, bundle: &Bundle, now_unix: u64) -> Result<PathBuf, String> {
    let dir = diagnostics_dir_in(config);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("cannot create the diagnostics folder: {e}"))?;
    let path = dir.join(format!("faris-diagnostics-{now_unix}.json"));
    atomic_file::write_atomic(&path, bundle.json.as_bytes())
        .map_err(|e| format!("cannot write the diagnostics bundle: {e}"))?;
    Ok(path)
}

/// The settings that are set in the user's file, for the bundle: a path is reduced to "set".
pub fn settings_for_bundle() -> Vec<(String, String)> {
    let Some(path) = crate::settings::default_path() else {
        return Vec::new();
    };
    let loaded = crate::settings::load(&path).settings;
    let mut out = Vec::new();
    for tool in crate::settings::Tool::ALL {
        if loaded.tool(tool).is_some() {
            out.push((format!("tools.{}", tool.key()), "set".to_owned()));
        }
    }
    for def in faris_model::settings::REGISTRY {
        if def.key.starts_with("tools.") {
            continue;
        }
        if let Some(value) = loaded.value(def.key) {
            out.push((def.key.to_owned(), value));
        }
    }
    out
}

/// Strings that identify the person and machine, for the scan: the home folder
/// and the user name from the environment.
pub fn personal_strings() -> Vec<String> {
    let mut out = BTreeSet::new();
    for var in ["HOME", "USERPROFILE", "USER", "USERNAME", "LOGNAME"] {
        if let Some(value) = std::env::var_os(var).map(|v| v.to_string_lossy().into_owned())
            && value.len() >= 3
            && value != "/"
        {
            out.insert(value);
        }
    }
    out.into_iter().collect()
}

// ---------------------------------------------------------------------------
// The window.

/// The crash records and diagnostics window.
#[derive(Default)]
pub struct Panel {
    pub open: bool,
    /// The configuration folder; `None` in the browser and in scripted runs.
    pub config: Option<PathBuf>,
    pub keep_days: Option<u64>,
    bundle: Option<(Bundle, Result<PathBuf, String>)>,
    notice: Option<String>,
}

impl Panel {
    fn days(&self) -> u64 {
        self.keep_days.unwrap_or(DEFAULT_KEEP_DAYS)
    }

    /// The stated retention, in words.
    pub fn retention_text(&self) -> String {
        format!(
            "Records are deleted after {} days, and only the newest {MAX_RECORDS} are kept.",
            self.days()
        )
    }

    /// The Clear now button: delete every crash record.
    pub fn clear_now(&mut self) -> usize {
        let n = self
            .config
            .as_ref()
            .map_or(0, |config| clear_all(&dir_in(config)));
        self.notice = Some(format!(
            "Deleted {n} crash record{}.",
            if n == 1 { "" } else { "s" }
        ));
        n
    }

    /// The Make a diagnostics bundle button: build, redact, write and keep the result to show.
    pub fn make_bundle(&mut self, settings: Vec<(String, String)>, secrets: Vec<String>) {
        let Some(config) = self.config.clone() else {
            return;
        };
        let now = recovery_now();
        let bundle = build_bundle(&BundleInputs {
            config: &config,
            settings,
            secrets,
            now_unix: now,
        });
        let saved = save_bundle(&config, &bundle, now);
        self.bundle = Some((bundle, saved));
    }

    /// The bundle last made, and where it was saved.
    #[cfg(test)]
    pub fn last_bundle(&self) -> Option<&(Bundle, Result<PathBuf, String>)> {
        self.bundle.as_ref()
    }

    pub fn show(
        &mut self,
        ctx: &eframe::egui::Context,
        settings: &dyn Fn() -> Vec<(String, String)>,
    ) {
        use eframe::egui;
        if !self.open {
            return;
        }
        let mut open = true;
        egui::Window::new("Crash records and diagnostics")
            .open(&mut open)
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.label("If FARIS crashes, it writes a short record on this computer. Nothing is sent anywhere, ever, unless you choose to attach a file yourself.");
                ui.weak("A record holds the FARIS version, your operating system and processor type, an error code, the place in the program code, and function names. It does not hold study content, file names, folders or geometry.");
                ui.weak(self.retention_text());
                ui.separator();
                let Some(config) = self.config.clone() else {
                    ui.label("Crash records are not kept in this version of FARIS (the browser version keeps nothing on your computer).");
                    return;
                };
                let records = list_records(&dir_in(&config));
                if records.is_empty() {
                    ui.label("No crash records. FARIS has not crashed since the records were last cleared.");
                }
                for (_, record) in &records {
                    ui.label(format!(
                        "{} · {} · {} · recovery copy {}",
                        crate::undo::clock(record.written_unix as f64),
                        record.error_code,
                        record.location,
                        record.emergency_autosave
                    ));
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!records.is_empty(), egui::Button::new("Clear now"))
                        .on_hover_text("Deletes every crash record on this computer.")
                        .clicked()
                    {
                        self.clear_now();
                    }
                    if ui
                        .button("Make a diagnostics bundle")
                        .on_hover_text("Writes one readable file with the details above. Nothing is sent.")
                        .clicked()
                    {
                        self.make_bundle(settings(), personal_strings());
                    }
                });
                if let Some(notice) = &self.notice {
                    ui.label(notice);
                }
                if let Some((bundle, saved)) = &self.bundle {
                    ui.separator();
                    match saved {
                        Ok(path) => {
                            ui.label(format!(
                                "Saved as {}. Nothing has been sent. Read it below; attach it to a bug report only if you choose to.",
                                path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())
                            ));
                        }
                        Err(why) => {
                            ui.colored_label(crate::badge::Kind::Failed.color(), format!("The bundle could not be saved: {why} Check that the configuration folder is writable, then try again."));
                        }
                    }
                    ui.strong("What the bundle holds");
                    for entry in &bundle.manifest {
                        ui.label(format!("{}: {} ({} bytes)", entry.section, entry.contents, entry.bytes));
                    }
                    ui.weak(format!(
                        "{} record{} included; {} item{} looking like a path, address or key {} removed.",
                        bundle.records_included,
                        if bundle.records_included == 1 { "" } else { "s" },
                        bundle.redactions,
                        if bundle.redactions == 1 { "" } else { "s" },
                        if bundle.redactions == 1 { "was" } else { "were" }
                    ));
                    egui::ScrollArea::vertical().max_height(220.0).id_salt("bundle-preview").show(ui, |ui| {
                        ui.add(egui::Label::new(egui::RichText::new(&bundle.json).monospace().small()).wrap());
                    });
                }
            });
        self.open = open;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STUDY_STRINGS: &[&str] = &["Tokamak-Bravo-7", "plant-secret-geometry", "ACME Fusion"];

    fn facts(message_seed: u64, ui: bool) -> PanicFacts {
        // Backtraces as the standard library prints them: symbol lines, then `at path:line`.
        let home = format!("/home/user{message_seed}/work/ACME Fusion/Tokamak-Bravo-7");
        PanicFacts {
            ui_thread: ui,
            file: Some(format!("{home}/crates/faris-app/src/lib.rs")),
            line: Some(10 + message_seed as u32),
            backtrace_text: format!(
                "   0: faris_app::FarisApp::ui\n             at {home}/crates/faris-app/src/lib.rs:{message_seed}\n   1: <faris_app::X as core::ops::Drop>::drop\n   2: C:\\Users\\Alice\\{home}\n   3: eframe::native::run::run_wgpu\n             at /home/u/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/eframe-0.36.2/src/native/run.rs:99\n"
            ),
        }
    }

    // Verifies: REL-041
    #[test]
    fn a_thousand_injected_crashes_leave_no_path_or_study_string_in_the_record() {
        let dir = tempfile::tempdir().unwrap();
        let mut secrets: Vec<String> = STUDY_STRINGS.iter().map(|s| (*s).to_owned()).collect();
        secrets.extend(["alice".into(), "user1".into()]);
        for n in 0..1000u64 {
            let record = build_record(
                &facts(n, n % 2 == 0),
                EmergencyOutcome::Written,
                1_700_000_000 + n,
            );
            let path = write_record(dir.path(), &record).unwrap();
            let text = std::fs::read_to_string(&path).unwrap();
            let found = scan(&text, &secrets);
            assert!(found.is_empty(), "record {n} leaks {found:?}:\n{text}");
            // The allow-list: the record parses back to exactly the fields it declares.
            let back: CrashRecord = serde_json::from_str(&text).unwrap();
            assert_eq!(back, record);
            assert!(!text.contains("/home/"), "a home folder is in the record");
            std::fs::remove_file(path).unwrap();
        }
        // The fields a record can have are exactly these.
        let record = build_record(&facts(1, true), EmergencyOutcome::NotNeeded, 5);
        let value = serde_json::to_value(&record).unwrap();
        let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "arch",
                "emergency_autosave",
                "error_code",
                "faris_version",
                "id",
                "kind",
                "location",
                "os",
                "schema",
                "stack",
                "written_unix"
            ]
        );
    }

    // Verifies: REL-041
    #[test]
    fn locations_keep_the_source_place_and_drop_the_machine() {
        assert_eq!(
            location_text(Some("crates/faris-app/src/lib.rs"), Some(12)),
            "crates/faris-app/src/lib.rs:12"
        );
        assert_eq!(
            location_text(
                Some("/home/u/src/faris/crates/faris-app/src/lib.rs"),
                Some(3)
            ),
            "crates/faris-app/src/lib.rs:3"
        );
        assert_eq!(
            location_text(
                Some(
                    "/home/u/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/eframe-0.36.2/src/x.rs"
                ),
                Some(9)
            ),
            "dependency eframe-0.36.2/src/x.rs:9"
        );
        assert_eq!(
            location_text(
                Some("/rustc/abcdef0123/library/core/src/option.rs"),
                Some(1)
            ),
            "std library/core/src/option.rs:1"
        );
        assert_eq!(
            location_text(Some("/home/u/secret/plant.rs"), Some(1)),
            "unknown"
        );
        assert_eq!(location_text(Some("C:\\Users\\a\\x.rs"), None), "unknown");
        assert_eq!(location_text(None, None), "unknown");
    }

    // Verifies: REL-004, REL-041
    #[test]
    fn a_hundred_injected_ui_panics_each_leave_a_record_and_a_recoverable_autosave() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path();
        for n in 0..100u64 {
            // The app keeps the newest recovery state where the hook can reach it.
            let mut state = crate::undo::EditState::placeholder();
            state.view.year = n as f64;
            let file = recovery::RecoveryFile::new(state.clone(), None, n as f64);
            let path = recovery::file_for(&recovery::dir_in(config), 7000 + n as u32);
            recovery::set_emergency(path.clone(), file.encode());
            let record = handle_panic(&facts(n, true), config, 1_700_000_000 + n).unwrap();
            assert!(record.is_file(), "no record for panic {n}");
            let back = recovery::read(&path).unwrap();
            assert_eq!(back.state, state, "autosave {n} did not restore the state");
            let saved: CrashRecord =
                serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
            assert_eq!(saved.emergency_autosave, "written");
        }
        recovery::clear_emergency();
        assert_eq!(list_records(&dir_in(config)).len(), 100);
        // A worker-thread panic writes a record and touches no recovery file.
        recovery::set_emergency(
            recovery::file_for(&recovery::dir_in(config), 1),
            b"x".to_vec(),
        );
        let record = handle_panic(&facts(5, false), config, 1_700_000_500).unwrap();
        assert!(!recovery::file_for(&recovery::dir_in(config), 1).exists());
        let saved: CrashRecord = serde_json::from_slice(&std::fs::read(record).unwrap()).unwrap();
        assert_eq!(
            (saved.kind.as_str(), saved.emergency_autosave.as_str()),
            ("worker-panic", "not needed")
        );
        recovery::clear_emergency();
    }

    // Verifies: REL-004
    #[test]
    fn a_real_panic_on_the_ui_thread_goes_through_the_hook_and_leaves_a_record() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().to_path_buf();
        let path = recovery::file_for(&recovery::dir_in(&config), 31337);
        let file = recovery::RecoveryFile::new(crate::undo::EditState::placeholder(), None, 1.0);
        recovery::set_emergency(path.clone(), file.encode());
        let before = std::panic::take_hook();
        std::panic::set_hook(before);
        install_panic_hook(config.clone());
        // The message quotes a study file and a folder; neither may reach the record.
        let caught = std::panic::catch_unwind(|| {
            panic!("cannot read /home/alice/ACME Fusion/Tokamak-Bravo-7.faris");
        });
        let _ = std::panic::take_hook(); // drops this test's hook (and the chain it holds)
        std::panic::set_hook(Box::new(|_| {}));
        let _ = std::panic::take_hook();
        assert!(caught.is_err());
        let records = list_records(&dir_in(&config));
        assert_eq!(records.len(), 1, "one panic, one record");
        let text = std::fs::read_to_string(&records[0].0).unwrap();
        assert!(
            scan(&text, &["Tokamak-Bravo-7".into(), "alice".into()]).is_empty(),
            "{text}"
        );
        assert_eq!(records[0].1.kind, "ui-panic");
        assert_eq!(records[0].1.emergency_autosave, "written");
        assert!(
            recovery::read(&path).is_ok(),
            "the emergency autosave was written"
        );
        recovery::clear_emergency();
    }

    // Verifies: REL-042, LEG-027
    #[test]
    fn records_are_deleted_after_thirty_days_and_beyond_the_count_and_clear_now_empties() {
        let dir = tempfile::tempdir().unwrap();
        let now = 100 * 86_400u64;
        let make = |age_days: u64, n: usize| {
            let when = now - age_days * 86_400;
            let record = build_record(&facts(n as u64, true), EmergencyOutcome::NotNeeded, when);
            write_record(dir.path(), &record).unwrap()
        };
        let fresh = make(1, 1);
        let edge = make(29, 2);
        let old = make(31, 3);
        let older = make(90, 4);
        assert_eq!(
            purge_expired(dir.path(), now, DEFAULT_KEEP_DAYS, MAX_RECORDS),
            2
        );
        assert!(fresh.is_file() && edge.is_file());
        assert!(!old.exists() && !older.exists());
        // More than the cap: the newest are kept.
        for n in 0..30 {
            make(0, 100 + n);
        }
        purge_expired(dir.path(), now, DEFAULT_KEEP_DAYS, MAX_RECORDS);
        assert_eq!(list_records(dir.path()).len(), MAX_RECORDS);
        // The retention can be changed.
        assert_eq!(
            purge_expired(dir.path(), now, 1, MAX_RECORDS),
            0,
            "all but two were made today"
        );
        // Clear now leaves none.
        let cleared = clear_all(dir.path());
        assert!(cleared >= 1);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    // Verifies: LEG-027
    #[test]
    fn recovery_files_and_bundles_expire_with_the_same_rule() {
        let config = tempfile::tempdir().unwrap();
        let recovery_dir = recovery::dir_in(config.path());
        let diagnostics = diagnostics_dir_in(config.path());
        std::fs::create_dir_all(&recovery_dir).unwrap();
        std::fs::create_dir_all(&diagnostics).unwrap();
        let old_recovery = recovery::file_for(&recovery_dir, 1);
        let new_recovery = recovery::file_for(&recovery_dir, 2);
        let old_bundle = diagnostics.join("faris-diagnostics-1.json");
        for f in [&old_recovery, &new_recovery, &old_bundle] {
            std::fs::write(f, b"{}").unwrap();
        }
        let now = crate::time::SystemTime::now();
        let forty_days = std::time::Duration::from_secs(40 * 86_400);
        for f in [&old_recovery, &old_bundle] {
            std::fs::File::options()
                .write(true)
                .open(f)
                .unwrap()
                .set_modified(now - forty_days)
                .unwrap();
        }
        let now_unix = now
            .duration_since(crate::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(purge_all(config.path(), now_unix, 30), 2);
        assert!(new_recovery.exists() && !old_recovery.exists() && !old_bundle.exists());
        // A missing configuration folder is not an error.
        assert_eq!(purge_all(&config.path().join("nowhere"), now_unix, 30), 0);
    }

    // Verifies: REL-043
    #[test]
    fn the_bundle_is_small_listed_fast_and_has_no_path_or_secret() {
        let config = tempfile::tempdir().unwrap();
        for n in 0..MAX_RECORDS as u64 {
            write_record(
                &dir_in(config.path()),
                &build_record(
                    &facts(n, n % 2 == 0),
                    EmergencyOutcome::Failed,
                    1_700_000_000 + n,
                ),
            )
            .unwrap();
        }
        let secrets = vec!["Tokamak-Bravo-7".to_owned(), "alice".to_owned()];
        let started = std::time::Instant::now();
        let bundle = build_bundle(&BundleInputs {
            config: config.path(),
            settings: vec![
                ("tools.openmc_python".into(), "set".into()),
                ("backups.keep".into(), "3".into()),
                // A setting whose text names the person: the scan takes it out.
                ("ui.note".into(), "/home/alice/ACME Fusion/openmc".into()),
            ],
            secrets: secrets.clone(),
            now_unix: 1_700_001_000,
        });
        let path = save_bundle(config.path(), &bundle, 1_700_001_000).unwrap();
        assert!(
            started.elapsed().as_secs_f64() <= 10.0,
            "took {:?}",
            started.elapsed()
        );
        assert!(bundle.json.len() <= BUNDLE_LIMIT_BYTES);
        assert_eq!(bundle.records_included, MAX_RECORDS);
        assert!(bundle.redactions >= 1, "the planted path was removed");
        assert!(scan(&bundle.json, &secrets).is_empty(), "{}", bundle.json);
        // The manifest names every section and its size.
        let names: Vec<_> = bundle.manifest.iter().map(|m| m.section.as_str()).collect();
        assert_eq!(names, ["app", "settings", "recovery", "crash_records"]);
        assert!(
            bundle
                .manifest
                .iter()
                .all(|m| m.bytes > 0 && !m.contents.is_empty())
        );
        assert!(path.starts_with(diagnostics_dir_in(config.path())));
        assert_eq!(std::fs::read_to_string(path).unwrap(), bundle.json);
        let parsed: serde_json::Value = serde_json::from_str(&bundle.json).unwrap();
        assert_eq!(parsed["manifest"].as_array().unwrap().len(), 4);
    }

    // Verifies: REL-042, REL-043
    #[test]
    fn the_buttons_clear_the_records_and_make_a_reviewable_bundle() {
        let config = tempfile::tempdir().unwrap();
        for n in 0..5u64 {
            write_record(
                &dir_in(config.path()),
                &build_record(
                    &facts(n, true),
                    EmergencyOutcome::NotNeeded,
                    1_700_000_000 + n,
                ),
            )
            .unwrap();
        }
        let mut panel = Panel {
            config: Some(config.path().to_path_buf()),
            ..Panel::default()
        };
        assert!(
            panel.retention_text().contains("30 days"),
            "{}",
            panel.retention_text()
        );
        assert!(panel.retention_text().contains("newest 20"));
        panel.make_bundle(vec![("backups.keep".into(), "3".into())], vec![]);
        let (bundle, saved) = panel.last_bundle().unwrap();
        assert_eq!(bundle.records_included, 5);
        assert!(saved.is_ok());
        assert_eq!(panel.clear_now(), 5);
        assert_eq!(
            list_records(&dir_in(config.path())).len(),
            0,
            "0 left after clearing"
        );
        // The window draws with and without a configuration folder, and says what the records hold.
        for config in [Some(config.path().to_path_buf()), None] {
            let mut panel = Panel {
                open: true,
                config,
                ..Panel::default()
            };
            let ctx = eframe::egui::Context::default();
            ctx.run_ui(eframe::egui::RawInput::default(), |ui| {
                panel.show(&ui.ctx().clone(), &|| Vec::new());
            })
            .drop_without_applying_deltas();
            assert!(panel.open);
        }
    }

    // Verifies: REL-043
    #[test]
    fn the_scan_finds_paths_addresses_keys_and_names_and_the_redaction_removes_them() {
        let secrets = vec!["Bravo".to_owned()];
        for bad in [
            "/home/x/plant.faris",
            "~/studies",
            "C:\\Users\\a\\plant.faris",
            "\\\\server\\share\\x",
            "name@example.com",
            "Authorization: Bearer abc",
            "api_key",
            "0123456789abcdef0123456789abcdef0123456789abcdef",
            "Tokamak-Bravo-7",
        ] {
            assert!(!scan(bad, &secrets).is_empty(), "{bad} was not found");
        }
        for fine in [
            "crates/faris-app/src/lib.rs:12",
            "faris_app::FarisApp::ui",
            "1.2.3",
            "dependency eframe-0.36.2/src/x.rs:9",
            "ui.scale",
            "x86_64",
        ] {
            assert!(scan(fine, &secrets).is_empty(), "{fine} was flagged");
        }
        let (clean, n) = redact(
            "{\"path\": \"/home/x/y\", \"who\": \"a@b.org\", \"ok\": 3}",
            &[],
        );
        assert_eq!(n, 2);
        assert!(scan(&clean, &[]).is_empty(), "{clean}");
        assert!(clean.contains("[redacted]") && clean.contains("\"ok\": 3"));
    }

    // Verifies: REL-040
    #[test]
    fn nothing_in_the_crash_code_can_send_anything() {
        // Crash records have no upload path: the module opens no sockets and calls no
        // network library, so with reports off or pending there are 0 outbound requests.
        let source = include_str!("crash.rs");
        let (code, _) = source.split_once("#[cfg(test)]").unwrap();
        for word in [
            "TcpStream",
            "UdpSocket",
            "reqwest",
            "ureq",
            "hyper",
            "curl",
            "http://",
            "https://",
            "std::net",
        ] {
            assert!(!code.contains(word), "crash.rs mentions {word}");
        }
    }
}
