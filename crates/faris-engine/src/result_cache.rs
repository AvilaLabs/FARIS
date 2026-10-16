//! The result cache: one way to build a cache key and one way to keep, check and manage an entry
//! (AUTO-031, AUTO-032, AUTO-034, AUTO-035, AUTO-036, AUTO-038, AUTO-039).
//!
//! A key is built only by [`cache_key`]. It hashes a kind name and a set of named components in
//! their canonical JSON form (`faris_model::canonical`), so inputs that are equal hash equally
//! whatever the key order, white space or number spelling. The FARIS version and the CPU
//! architecture are components of every key. An entry is a folder named `<kind>-<digest>` that
//! holds its files and an `entry.json` listing the key components in readable form, when it was
//! made, how long it took, and the SHA-256 and size of every file.
//!
//! Every read verifies the entry: the components must hash to the recorded digest, every listed
//! file must be present with the recorded size and hash, and no unlisted file may be there. An
//! entry that fails is refused and reported ([`Lookup::Rejected`]), never used. An entry that is
//! whole but was made for a different key than the one asked for is a false hit: it is refused
//! and counted apart from the hits. Counters live in `stats.json` in the cache folder.
//!
//! Folders written before this layer existed have no `entry.json`. They are never read as hits;
//! `faris cache list` shows them as legacy and `faris cache clear` removes them.

use crate::atomic_file::write_atomic;
use faris_model::canonical::canonical_json;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

pub const KEY_SCHEMA: &str = "faris-cache-key/v1";
pub const ENTRY_SCHEMA: &str = "faris-cache-entry/v1";
pub const ENTRY_FILE: &str = "entry.json";
pub const STATS_FILE: &str = "stats.json";
const USED_FILE: &str = "last_used";
const PINNED_FILE: &str = "pinned";
/// Files in an entry folder that are about the entry, not part of its content.
const META_FILES: [&str; 3] = [ENTRY_FILE, USED_FILE, PINNED_FILE];
/// Largest a cache may grow before the least recently used entries are removed.
pub const DEFAULT_MAX_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// Largest `entry.json` read.
const MAX_ENTRY_BYTES: u64 = 4 * 1024 * 1024;

/// The code version every key carries.
pub const FARIS_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("cache {path}: {message}")]
    Io { path: String, message: String },
    #[error("cache: {0}")]
    Invalid(String),
}

fn io_error(path: &Path, error: impl std::fmt::Display) -> CacheError {
    CacheError::Io {
        path: path.display().to_string(),
        message: error.to_string(),
    }
}

/// A built key: the kind, the digest, and the components it was built from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheKey {
    kind: String,
    digest: String,
    components: BTreeMap<String, Value>,
}

impl CacheKey {
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Lowercase hexadecimal SHA-256.
    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn components(&self) -> &BTreeMap<String, Value> {
        &self.components
    }

    /// The folder name of the entry.
    pub fn dir_name(&self) -> String {
        format!("{}-{}", self.kind, self.digest)
    }
}

fn digest_of(kind: &str, components: &BTreeMap<String, Value>) -> String {
    let body = serde_json::json!({
        "schema": KEY_SCHEMA,
        "kind": kind,
        "components": components,
    });
    format!("{:x}", Sha256::digest(canonical_json(&body).as_bytes()))
}

/// The only place a cache key is built (AUTO-031). `kind` names what is cached (lowercase letters,
/// digits and hyphens). `components` are every input that can change the result, by name; a
/// component is a JSON value (a hash string, a number, a whole input document). The FARIS version
/// (`faris_version`) and the CPU architecture (`cpu_arch`) are added to every key and replace a
/// caller's component of the same name.
pub fn cache_key<'a>(
    kind: &str,
    components: impl IntoIterator<Item = (&'a str, Value)>,
) -> CacheKey {
    assert!(
        !kind.is_empty()
            && kind
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
        "a cache kind is lowercase letters, digits and hyphens: {kind:?}"
    );
    let mut map: BTreeMap<String, Value> = components
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect();
    map.insert("faris_version".into(), Value::String(FARIS_VERSION.into()));
    map.insert(
        "cpu_arch".into(),
        Value::String(std::env::consts::ARCH.into()),
    );
    CacheKey {
        kind: kind.to_string(),
        digest: digest_of(kind, &map),
        components: map,
    }
}

/// One file of an entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryFile {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

/// `entry.json`: what an entry says about itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntryRecord {
    pub schema: String,
    pub kind: String,
    pub key: String,
    /// Every component of the key, as it was hashed.
    pub components: BTreeMap<String, Value>,
    pub faris_version: String,
    pub created_unix: u64,
    pub created_utc: String,
    /// Seconds the original calculation took; 0 when not measured.
    pub compute_seconds: f64,
    pub files: Vec<EntryFile>,
    pub total_bytes: u64,
}

/// Where a cached result came from, to be shown with the result (AUTO-039).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CacheProvenance {
    pub kind: String,
    pub key: String,
    pub created_utc: String,
    pub faris_version: String,
    pub compute_seconds: f64,
}

impl CacheProvenance {
    /// The sentence shown beside a result that was served from the cache.
    pub fn text(&self) -> String {
        let date = self.created_utc.get(..10).unwrap_or(&self.created_utc);
        let took = if self.compute_seconds > 0.0 {
            format!(", {:.0} s to calculate", self.compute_seconds)
        } else {
            String::new()
        };
        format!(
            "reused from {date} (made by FARIS {}{took}; {} key {})",
            self.faris_version,
            self.kind,
            self.key.get(..12).unwrap_or(&self.key)
        )
    }
}

/// A verified entry.
#[derive(Clone, Debug)]
pub struct Hit {
    pub dir: PathBuf,
    pub provenance: CacheProvenance,
}

impl Hit {
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

/// The result of asking for a key.
#[derive(Debug)]
pub enum Lookup {
    Hit(Hit),
    Miss,
    /// An entry was there and was refused (damaged, or made for another key). The text says why;
    /// the caller reports it and recalculates.
    Rejected(String),
}

impl Lookup {
    pub fn hit(self) -> Option<Hit> {
        match self {
            Self::Hit(hit) => Some(hit),
            _ => None,
        }
    }
}

/// Counters, kept in `stats.json` (AUTO-038). A false hit is an entry that was whole but made for
/// a different key than the one asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheStats {
    #[serde(default)]
    pub hits: u64,
    #[serde(default)]
    pub misses: u64,
    #[serde(default)]
    pub corrupt: u64,
    #[serde(default)]
    pub false_hits: u64,
    #[serde(default)]
    pub stored: u64,
    #[serde(default)]
    pub evicted: u64,
}

impl CacheStats {
    pub fn lookups(&self) -> u64 {
        self.hits + self.misses + self.corrupt + self.false_hits
    }

    /// Hits over all lookups; `None` before the first lookup.
    pub fn hit_rate(&self) -> Option<f64> {
        (self.lookups() > 0).then(|| self.hits as f64 / self.lookups() as f64)
    }

    /// False hits over everything that looked like a hit (hits plus false hits); `None` when
    /// nothing did. This is 0 unless an entry disagrees with its key.
    pub fn false_hit_rate(&self) -> Option<f64> {
        let found = self.hits + self.false_hits;
        (found > 0).then(|| self.false_hits as f64 / found as f64)
    }

    fn add(&mut self, other: &CacheStats) {
        self.hits += other.hits;
        self.misses += other.misses;
        self.corrupt += other.corrupt;
        self.false_hits += other.false_hits;
        self.stored += other.stored;
        self.evicted += other.evicted;
    }
}

/// An entry as `faris cache list` shows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EntryInfo {
    pub dir_name: String,
    pub kind: String,
    pub key: String,
    pub created_utc: String,
    pub last_used_unix: u64,
    pub bytes: u64,
    pub pinned: bool,
    pub faris_version: String,
}

/// What `verify` found for one folder.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct VerifyLine {
    pub dir_name: String,
    /// `ok`, `damaged` or `legacy`.
    pub status: &'static str,
    pub detail: String,
}

/// A cache folder.
pub struct ResultCache {
    root: PathBuf,
    max_bytes: u64,
    session: Mutex<CacheStats>,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `2026-10-15T09:30:00Z` for a Unix time.
pub fn utc_text(unix: u64) -> String {
    let (year, month, day) = faris_model::display::civil_date(unix as i64);
    let rest = unix % faris_model::units::DAY_S as u64;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

fn sha256_path(path: &Path) -> std::io::Result<(String, u64)> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        total += n as u64;
    }
    Ok((format!("{:x}", hasher.finalize()), total))
}

/// Every regular file under `dir`, as `/`-joined relative paths, sorted. Links are not followed
/// and not accepted: an entry holds plain files.
fn walk(dir: &Path) -> Result<Vec<String>, String> {
    fn go(base: &Path, at: &Path, out: &mut Vec<String>) -> Result<(), String> {
        let read = std::fs::read_dir(at).map_err(|e| format!("{}: {e}", at.display()))?;
        for item in read {
            let item = item.map_err(|e| format!("{}: {e}", at.display()))?;
            let kind = item
                .file_type()
                .map_err(|e| format!("{}: {e}", item.path().display()))?;
            let path = item.path();
            if kind.is_dir() {
                go(base, &path, out)?;
            } else if kind.is_file() {
                let relative = path.strip_prefix(base).map_err(|e| e.to_string())?;
                let text = relative
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push(text);
            } else {
                return Err(format!(
                    "{} is a link or special file; an entry holds plain files only",
                    path.display()
                ));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    go(dir, dir, &mut out)?;
    out.retain(|p| !META_FILES.contains(&p.as_str()));
    out.sort();
    Ok(out)
}

/// Reads and checks an entry folder on its own terms. `Err` is the reason it cannot be trusted.
fn verify_dir(dir: &Path) -> Result<EntryRecord, String> {
    let record_path = dir.join(ENTRY_FILE);
    let meta = std::fs::metadata(&record_path).map_err(|e| format!("{ENTRY_FILE}: {e}"))?;
    if meta.len() > MAX_ENTRY_BYTES {
        return Err(format!(
            "{ENTRY_FILE} is larger than {MAX_ENTRY_BYTES} bytes"
        ));
    }
    let bytes = std::fs::read(&record_path).map_err(|e| format!("{ENTRY_FILE}: {e}"))?;
    let record: EntryRecord =
        serde_json::from_slice(&bytes).map_err(|e| format!("{ENTRY_FILE} is not valid: {e}"))?;
    if record.schema != ENTRY_SCHEMA {
        return Err(format!(
            "{ENTRY_FILE} has the unknown schema {}",
            record.schema
        ));
    }
    if digest_of(&record.kind, &record.components) != record.key {
        return Err(
            "the recorded key components do not hash to the recorded key (the record was edited)"
                .into(),
        );
    }
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned());
    if name.as_deref() != Some(&format!("{}-{}", record.kind, record.key)) {
        return Err("the folder name is not the entry's own kind and key".into());
    }
    let mut total = 0;
    for file in &record.files {
        let path = dir.join(&file.path);
        if file.path.split('/').any(|p| p.is_empty() || p == "..") || file.path.starts_with('/') {
            return Err(format!("{} is not a plain relative path", file.path));
        }
        let (sha, size) = sha256_path(&path).map_err(|e| format!("{}: {e}", file.path))?;
        if size != file.bytes {
            return Err(format!(
                "{} is {size} bytes, the record says {}",
                file.path, file.bytes
            ));
        }
        if !sha.eq_ignore_ascii_case(&file.sha256) {
            return Err(format!("{} does not match its recorded SHA-256", file.path));
        }
        total += size;
    }
    if total != record.total_bytes {
        return Err("the file sizes do not add up to the recorded total".into());
    }
    let listed: std::collections::BTreeSet<&str> =
        record.files.iter().map(|f| f.path.as_str()).collect();
    for found in walk(dir)? {
        if !listed.contains(found.as_str()) {
            return Err(format!("{found} is in the folder but not in the record"));
        }
    }
    Ok(record)
}

fn read_used(dir: &Path) -> Option<u64> {
    std::fs::read_to_string(dir.join(USED_FILE))
        .ok()?
        .trim()
        .parse()
        .ok()
}

impl ResultCache {
    /// The cache in `root`; the folder is created when the first entry is stored.
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            max_bytes: DEFAULT_MAX_BYTES,
            session: Mutex::new(CacheStats::default()),
        }
    }

    /// Entries are removed, least recently used first, when the cache grows past this.
    pub fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn entry_dir(&self, key: &CacheKey) -> PathBuf {
        self.root.join(key.dir_name())
    }

    /// The counters of this process since the cache was opened.
    pub fn session_stats(&self) -> CacheStats {
        self.session.lock().map_or_else(|p| *p.into_inner(), |s| *s)
    }

    /// The counters kept in the cache folder over all runs.
    pub fn stats(&self) -> CacheStats {
        std::fs::read(self.root.join(STATS_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn count(&self, change: CacheStats) {
        if let Ok(mut session) = self.session.lock() {
            session.add(&change);
        }
        // Best effort: a counter that cannot be written never fails a lookup.
        let mut total = self.stats();
        total.add(&change);
        if std::fs::create_dir_all(&self.root).is_ok()
            && let Ok(bytes) = serde_json::to_vec_pretty(&total)
        {
            let _ = write_atomic(&self.root.join(STATS_FILE), &bytes);
        }
    }

    /// Ask for a key. Verifies the whole entry on every call.
    pub fn lookup(&self, key: &CacheKey) -> Lookup {
        let dir = self.entry_dir(key);
        if !dir.is_dir() {
            self.count(CacheStats {
                misses: 1,
                ..CacheStats::default()
            });
            return Lookup::Miss;
        }
        match verify_dir(&dir) {
            Err(reason) => {
                self.count(CacheStats {
                    corrupt: 1,
                    ..CacheStats::default()
                });
                Lookup::Rejected(format!(
                    "the cached {} entry {} was refused and will be recalculated: {reason}",
                    key.kind(),
                    key.dir_name()
                ))
            }
            Ok(record) => {
                if record.key != key.digest
                    || record.kind != key.kind
                    || canonical_json(&serde_json::to_value(&record.components).unwrap_or_default())
                        != canonical_json(
                            &serde_json::to_value(&key.components).unwrap_or_default(),
                        )
                {
                    self.count(CacheStats {
                        false_hits: 1,
                        ..CacheStats::default()
                    });
                    return Lookup::Rejected(format!(
                        "the cached entry {} was made for a different key ({}) than the one asked for ({}); it was refused and counted as a false hit",
                        key.dir_name(),
                        record.key,
                        key.digest
                    ));
                }
                let _ = std::fs::write(dir.join(USED_FILE), now_unix().to_string());
                self.count(CacheStats {
                    hits: 1,
                    ..CacheStats::default()
                });
                Lookup::Hit(Hit {
                    dir,
                    provenance: CacheProvenance {
                        kind: record.kind,
                        key: record.key,
                        created_utc: record.created_utc,
                        faris_version: record.faris_version,
                        compute_seconds: record.compute_seconds,
                    },
                })
            }
        }
    }

    /// The bytes of one file of an entry, if the entry is whole.
    pub fn get_bytes(&self, key: &CacheKey, name: &str) -> (Lookup, Option<Vec<u8>>) {
        let lookup = self.lookup(key);
        let bytes = match &lookup {
            Lookup::Hit(hit) => std::fs::read(hit.path(name)).ok(),
            _ => None,
        };
        (lookup, bytes)
    }

    /// Store one file as a whole entry, replacing any entry for the key. Written beside the cache
    /// and renamed into place, so an interrupted store leaves nothing a lookup would read.
    pub fn put_bytes(
        &self,
        key: &CacheKey,
        name: &str,
        bytes: &[u8],
        compute_seconds: f64,
    ) -> Result<(), CacheError> {
        if name.is_empty() || name.contains('/') || name.contains('\\') || name.starts_with('.') {
            return Err(CacheError::Invalid(format!(
                "{name:?} is not a plain file name"
            )));
        }
        std::fs::create_dir_all(&self.root).map_err(|e| io_error(&self.root, e))?;
        let temp = self
            .root
            .join(format!(".tmp-{}-{}", key.digest(), std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir(&temp).map_err(|e| io_error(&temp, e))?;
        let written = std::fs::write(temp.join(name), bytes)
            .map_err(|e| io_error(&temp, e))
            .and_then(|()| write_record(&temp, key, compute_seconds));
        if let Err(error) = written {
            let _ = std::fs::remove_dir_all(&temp);
            return Err(error);
        }
        let dir = self.entry_dir(key);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::rename(&temp, &dir).map_err(|e| io_error(&dir, e))?;
        self.count(CacheStats {
            stored: 1,
            ..CacheStats::default()
        });
        self.enforce_limit(Some(key));
        Ok(())
    }

    /// Record the files now in `entry_dir(key)` as the entry (for entries built in place, such as
    /// weight windows a helper writes). Call again after adding files. Keeps the original date
    /// of an entry already recorded under the same key.
    pub fn seal(&self, key: &CacheKey, compute_seconds: f64) -> Result<(), CacheError> {
        let dir = self.entry_dir(key);
        write_record(&dir, key, compute_seconds)?;
        self.count(CacheStats {
            stored: 1,
            ..CacheStats::default()
        });
        self.enforce_limit(Some(key));
        Ok(())
    }

    /// Remove the entry of a key (a refused entry, before it is rebuilt).
    pub fn discard(&self, key: &CacheKey) {
        let _ = std::fs::remove_dir_all(self.entry_dir(key));
    }

    fn entry_folders(&self) -> Vec<PathBuf> {
        let Ok(read) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut folders: Vec<PathBuf> = read
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .collect();
        folders.sort();
        folders
    }

    /// Folders made before this layer, which have no record: reactor windows (`reactor-windows-`
    /// and a 64-digit name) and design-transport entries (a 64-digit name).
    fn is_legacy(path: &Path) -> bool {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let Some(name) = name else { return false };
        !path.join(ENTRY_FILE).exists()
            && !name.starts_with('.')
            && (name.starts_with("reactor-windows-")
                || (name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit())))
    }

    /// Every entry with a readable record, newest use first is not promised; sorted by name.
    pub fn list(&self) -> Vec<EntryInfo> {
        let mut out = Vec::new();
        for dir in self.entry_folders() {
            let Ok(bytes) = std::fs::read(dir.join(ENTRY_FILE)) else {
                continue;
            };
            let Ok(record) = serde_json::from_slice::<EntryRecord>(&bytes) else {
                continue;
            };
            out.push(EntryInfo {
                dir_name: dir
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                kind: record.kind,
                key: record.key,
                created_utc: record.created_utc,
                last_used_unix: read_used(&dir).unwrap_or(record.created_unix),
                bytes: record.total_bytes,
                pinned: dir.join(PINNED_FILE).exists(),
                faris_version: record.faris_version,
            });
        }
        out
    }

    /// Rehash every entry (AUTO-035). Folders with a record are `ok` or `damaged`; folders from
    /// before the record existed are `legacy` and are never served.
    pub fn verify_all(&self) -> Vec<VerifyLine> {
        let mut out = Vec::new();
        for dir in self.entry_folders() {
            let dir_name = dir
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            if dir_name.starts_with(".tmp-") {
                out.push(VerifyLine {
                    dir_name,
                    status: "damaged",
                    detail: "an interrupted store left this folder; `faris cache clear` removes it"
                        .into(),
                });
            } else if dir.join(ENTRY_FILE).exists() {
                match verify_dir(&dir) {
                    Ok(_) => out.push(VerifyLine {
                        dir_name,
                        status: "ok",
                        detail: String::new(),
                    }),
                    Err(reason) => out.push(VerifyLine {
                        dir_name,
                        status: "damaged",
                        detail: reason,
                    }),
                }
            } else if Self::is_legacy(&dir) {
                out.push(VerifyLine {
                    dir_name,
                    status: "legacy",
                    detail: "made before cache entries recorded their key; it is not read and can be cleared".into(),
                });
            }
        }
        out
    }

    /// The record of the single entry whose key or folder name starts with `prefix`.
    pub fn explain(&self, prefix: &str) -> Result<EntryRecord, CacheError> {
        let matches: Vec<PathBuf> = self
            .entry_folders()
            .into_iter()
            .filter(|dir| {
                let name = dir.file_name().map(|n| n.to_string_lossy().into_owned());
                name.is_some_and(|n| {
                    n.starts_with(prefix)
                        || n.rsplit('-').next().is_some_and(|k| k.starts_with(prefix))
                })
            })
            .collect();
        match matches.as_slice() {
            [] => Err(CacheError::Invalid(format!(
                "no entry starts with {prefix}. Next step: run `faris cache list` for the keys"
            ))),
            [one] => {
                let bytes = std::fs::read(one.join(ENTRY_FILE)).map_err(|e| {
                    CacheError::Invalid(format!(
                        "{} has no readable record ({e}); it is a legacy folder or damaged",
                        one.display()
                    ))
                })?;
                serde_json::from_slice(&bytes).map_err(|e| {
                    CacheError::Invalid(format!("{} has an unreadable record: {e}", one.display()))
                })
            }
            _ => Err(CacheError::Invalid(format!(
                "{} entries start with {prefix}. Next step: give more characters of the key",
                matches.len()
            ))),
        }
    }

    /// Pin or unpin the entry whose folder name or key starts with `prefix`. A pinned entry is
    /// never removed by `gc` or the size limit.
    pub fn set_pinned(&self, prefix: &str, pinned: bool) -> Result<String, CacheError> {
        let record = self.explain(prefix)?;
        let dir = self.root.join(format!("{}-{}", record.kind, record.key));
        let marker = dir.join(PINNED_FILE);
        if pinned {
            std::fs::write(&marker, b"pinned\n").map_err(|e| io_error(&marker, e))?;
        } else if marker.exists() {
            std::fs::remove_file(&marker).map_err(|e| io_error(&marker, e))?;
        }
        Ok(format!("{}-{}", record.kind, record.key))
    }

    /// Remove least recently used, unpinned entries until the total is at most `max_bytes`. The
    /// entry named by `keep` is never removed. Returns the folder names removed and the bytes
    /// left (which can stay above the limit when what remains is pinned).
    pub fn gc(&self, max_bytes: u64, keep: Option<&CacheKey>) -> (Vec<String>, u64) {
        let mut entries = self.list();
        let mut total: u64 = entries.iter().map(|e| e.bytes).sum();
        entries.sort_by_key(|e| (e.last_used_unix, e.dir_name.clone()));
        let keep_name = keep.map(CacheKey::dir_name);
        let mut removed = Vec::new();
        for entry in entries {
            if total <= max_bytes {
                break;
            }
            if entry.pinned || keep_name.as_deref() == Some(entry.dir_name.as_str()) {
                continue;
            }
            if std::fs::remove_dir_all(self.root.join(&entry.dir_name)).is_ok() {
                total = total.saturating_sub(entry.bytes);
                removed.push(entry.dir_name);
            }
        }
        if !removed.is_empty() {
            self.count(CacheStats {
                evicted: removed.len() as u64,
                ..CacheStats::default()
            });
        }
        (removed, total)
    }

    fn enforce_limit(&self, keep: Option<&CacheKey>) {
        let _ = self.gc(self.max_bytes, keep);
    }

    /// Remove every entry, legacy folder and interrupted store. Pinned entries stay unless
    /// `include_pinned`. The counters file stays. Returns the folder names removed.
    pub fn clear(&self, include_pinned: bool) -> Vec<String> {
        let mut removed = Vec::new();
        for dir in self.entry_folders() {
            let name = dir
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let ours =
                dir.join(ENTRY_FILE).exists() || Self::is_legacy(&dir) || name.starts_with(".tmp-");
            if !ours || (!include_pinned && dir.join(PINNED_FILE).exists()) {
                continue;
            }
            if std::fs::remove_dir_all(&dir).is_ok() {
                removed.push(name);
            }
        }
        removed
    }
}

/// Writes `entry.json` for the files now in `dir`.
fn write_record(dir: &Path, key: &CacheKey, compute_seconds: f64) -> Result<(), CacheError> {
    let previous = verify_dir_record_only(dir).filter(|r| r.key == key.digest);
    let mut files = Vec::new();
    let mut total = 0;
    for path in walk(dir).map_err(CacheError::Invalid)? {
        let (sha256, bytes) = sha256_path(&dir.join(&path)).map_err(|e| io_error(dir, e))?;
        total += bytes;
        files.push(EntryFile {
            path,
            sha256,
            bytes,
        });
    }
    let created = previous.as_ref().map_or_else(now_unix, |r| r.created_unix);
    let seconds = if compute_seconds > 0.0 {
        compute_seconds
    } else {
        previous.map_or(0.0, |r| r.compute_seconds)
    };
    let record = EntryRecord {
        schema: ENTRY_SCHEMA.into(),
        kind: key.kind.clone(),
        key: key.digest.clone(),
        components: key.components.clone(),
        faris_version: FARIS_VERSION.into(),
        created_unix: created,
        created_utc: utc_text(created),
        compute_seconds: seconds,
        files,
        total_bytes: total,
    };
    let bytes =
        serde_json::to_vec_pretty(&record).map_err(|e| CacheError::Invalid(e.to_string()))?;
    write_atomic(&dir.join(ENTRY_FILE), &bytes).map_err(|e| io_error(dir, e))
}

/// The record of a folder without checking its files (used to keep the date of a re-sealed entry).
fn verify_dir_record_only(dir: &Path) -> Option<EntryRecord> {
    serde_json::from_slice(&std::fs::read(dir.join(ENTRY_FILE)).ok()?).ok()
}

#[cfg(test)]
mod tests;
