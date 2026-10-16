//! `faris cache`: look at, check and trim the result cache (AUTO-032, AUTO-035, AUTO-036,
//! AUTO-038). The rules live in `faris_engine::result_cache`.

use clap::Subcommand;
use faris_engine::result_cache::{DEFAULT_MAX_BYTES, ResultCache, utc_text};
use std::path::PathBuf;

/// One or more cache entries failed verification; exits with status 1.
#[derive(Debug)]
pub struct CacheDamaged(pub String);

impl std::fmt::Display for CacheDamaged {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for CacheDamaged {}

#[derive(Subcommand)]
pub enum CacheCommand {
    /// List the entries: kind, key, size, when made, when last used.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Rehash every entry. A damaged entry is reported and the command exits with status 1.
    Verify {
        #[arg(long)]
        json: bool,
    },
    /// Show every component of one entry's key, with its value.
    Explain {
        /// The key, or the start of it (at least 6 characters).
        key: String,
        #[arg(long)]
        json: bool,
    },
    /// Show hit and false-hit counts, and the two rates, separately.
    Stats {
        #[arg(long)]
        json: bool,
    },
    /// Remove the least recently used entries until the cache is at most this size.
    /// Pinned entries are never removed.
    Gc {
        /// A size such as 500MB or 10GB (powers of 1024).
        #[arg(long, value_parser = parse_size)]
        max_size: u64,
    },
    /// Keep an entry through `gc` and the size limit.
    Pin { key: String },
    /// Let an entry be removed again.
    Unpin { key: String },
    /// Remove every entry (and folders made before entries recorded their key).
    Clear {
        /// Confirm: the entries are deleted and calculated again when next needed.
        #[arg(long)]
        yes: bool,
        /// Remove pinned entries too.
        #[arg(long)]
        include_pinned: bool,
    },
}

/// `cache` takes the folder with it: the default is `faris-transport-cache` in the current folder,
/// the name the transport commands use beside their inputs.
pub fn folder(dir: Option<PathBuf>) -> PathBuf {
    dir.unwrap_or_else(|| PathBuf::from(faris_engine::reactor::WINDOW_CACHE_NAME))
}

pub fn parse_size(text: &str) -> Result<u64, String> {
    let lower = text.trim().to_ascii_lowercase();
    let (digits, factor): (&str, u64) = if let Some(n) = lower.strip_suffix("gb") {
        (n, 1 << 30)
    } else if let Some(n) = lower.strip_suffix("mb") {
        (n, 1 << 20)
    } else if let Some(n) = lower.strip_suffix("kb") {
        (n, 1 << 10)
    } else if let Some(n) = lower.strip_suffix('b') {
        (n, 1)
    } else {
        (lower.as_str(), 1)
    };
    digits
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map(|v| (v * factor as f64) as u64)
        .ok_or_else(|| format!("{text:?} is not a size. Next step: write it like 500MB or 10GB"))
}

fn size_text(bytes: u64) -> String {
    const UNITS: [(&str, f64); 3] = [
        ("GiB", 1_073_741_824.0),
        ("MiB", 1_048_576.0),
        ("KiB", 1024.0),
    ];
    for (unit, size) in UNITS {
        if bytes as f64 >= size {
            return format!("{:.1} {unit}", bytes as f64 / size);
        }
    }
    format!("{bytes} B")
}

fn percent(rate: Option<f64>) -> String {
    rate.map_or_else(
        || "not yet measured".into(),
        |r| format!("{:.1} %", r * 100.0),
    )
}

pub fn run(command: CacheCommand, dir: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let root = folder(dir);
    if !root.is_dir() {
        return Err(format!(
            "there is no cache folder at {}. Next step: give the folder with --dir; the transport commands keep theirs in faris-transport-cache beside the run",
            root.display()
        )
        .into());
    }
    let cache = ResultCache::open(&root);
    match command {
        CacheCommand::List { json } => {
            let entries = cache.list();
            if json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else if entries.is_empty() {
                println!("The cache at {} has no entries.", root.display());
            } else {
                for e in &entries {
                    println!(
                        "{}  {:>10}  made {}  last used {}{}",
                        e.dir_name,
                        size_text(e.bytes),
                        e.created_utc,
                        utc_text(e.last_used_unix),
                        if e.pinned { "  pinned" } else { "" }
                    );
                }
                let total: u64 = entries.iter().map(|e| e.bytes).sum();
                println!(
                    "{} entries, {} (limit {})",
                    entries.len(),
                    size_text(total),
                    size_text(DEFAULT_MAX_BYTES)
                );
            }
        }
        CacheCommand::Verify { json } => {
            let lines = cache.verify_all();
            let damaged = lines.iter().filter(|l| l.status == "damaged").count();
            if json {
                println!("{}", serde_json::to_string_pretty(&lines)?);
            } else {
                for line in &lines {
                    if line.detail.is_empty() {
                        println!("{}  {}", line.status, line.dir_name);
                    } else {
                        println!("{}  {}: {}", line.status, line.dir_name, line.detail);
                    }
                }
                println!("{} checked, {damaged} damaged", lines.len());
            }
            if damaged > 0 {
                return Err(Box::new(CacheDamaged(format!(
                    "{damaged} cache entries are damaged and will not be used. Next step: run `faris cache clear --yes`, or let the next run rebuild them"
                ))));
            }
        }
        CacheCommand::Explain { key, json } => {
            if key.len() < 6 {
                return Err("give at least 6 characters of the key".into());
            }
            let record = cache.explain(&key)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&record)?);
            } else {
                println!("kind: {}", record.kind);
                println!("key: {}", record.key);
                println!(
                    "made: {} by FARIS {}",
                    record.created_utc, record.faris_version
                );
                println!("took: {} s", record.compute_seconds);
                println!("components:");
                for (name, value) in &record.components {
                    let text = serde_json::to_string(value)?;
                    // A large document is shown by its size; its hash is the key's own.
                    if text.len() > 160 {
                        println!(
                            "  {name}: {} characters, sha256 {}",
                            text.len(),
                            faris_study::sha256_hex(
                                faris_model::canonical::canonical_json(value).as_bytes()
                            )
                        );
                    } else {
                        println!("  {name}: {text}");
                    }
                }
                println!("files:");
                for f in &record.files {
                    println!("  {}  {}  {}", f.sha256, size_text(f.bytes), f.path);
                }
            }
        }
        CacheCommand::Stats { json } => {
            let stats = cache.stats();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "hits": stats.hits, "misses": stats.misses, "corrupt": stats.corrupt,
                        "false_hits": stats.false_hits, "stored": stats.stored, "evicted": stats.evicted,
                        "hit_rate": stats.hit_rate(), "false_hit_rate": stats.false_hit_rate(),
                        "bytes": cache.list().iter().map(|e| e.bytes).sum::<u64>(),
                    }))?
                );
            } else {
                println!(
                    "hits {}, misses {}, refused as damaged {}, false hits {}",
                    stats.hits, stats.misses, stats.corrupt, stats.false_hits
                );
                println!("hit rate: {}", percent(stats.hit_rate()));
                println!(
                    "false-hit rate: {} (must be 0)",
                    percent(stats.false_hit_rate())
                );
                println!("stored {}, evicted {}", stats.stored, stats.evicted);
            }
        }
        CacheCommand::Gc { max_size } => {
            let (removed, left) = cache.gc(max_size, None);
            for name in &removed {
                println!("removed {name}");
            }
            println!(
                "{} removed; {} left{}",
                removed.len(),
                size_text(left),
                if left > max_size {
                    " (what is left is pinned, so it stays above the limit)"
                } else {
                    ""
                }
            );
        }
        CacheCommand::Pin { key } => println!("pinned {}", cache.set_pinned(&key, true)?),
        CacheCommand::Unpin { key } => println!("unpinned {}", cache.set_pinned(&key, false)?),
        CacheCommand::Clear {
            yes,
            include_pinned,
        } => {
            if !yes {
                return Err(format!(
                    "this deletes every cache entry in {}; they are calculated again when needed. Next step: run `faris cache clear --yes` to confirm",
                    root.display()
                )
                .into());
            }
            let removed = cache.clear(include_pinned);
            println!("removed {} entries", removed.len());
        }
    }
    Ok(())
}
