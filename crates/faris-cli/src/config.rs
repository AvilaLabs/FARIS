//! `faris config`: the user settings file and the layers over it (CFG-002, CFG-017, CFG-080).
//! The rules live in `faris_model::settings`; this file reads and writes the file and prints.

use clap::Subcommand;
use faris_engine::{
    atomic_file,
    memory::{self, MIB, MemoryPolicy},
};
use faris_model::settings::{
    self, Layers, Origin, REGISTRY, Resolved, SettingDef, UserSettings, parse_flag,
};
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Show every setting with its value, unit and the layer that supplied it.
    List {
        /// One JSON document instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Show one setting's effective value.
    Get {
        key: String,
        /// Also say which layer supplied the value: default, user file, environment or flag.
        #[arg(long)]
        show_origin: bool,
    },
    /// Validate a value and write it to your settings file.
    Set { key: String, value: String },
    /// Remove a value from your settings file, so the default or environment applies again.
    Unset { key: String },
    /// Replace your settings file with an empty one (the desktop's own sections are kept).
    Reset {
        /// Confirm: every value you set in the file is lost.
        #[arg(long)]
        yes: bool,
    },
    /// Print where your settings file is.
    Path,
    /// Compare two settings files: every key whose effective value differs, with old and new value,
    /// unit, layer and effect class. Exits 0 when identical, 6 when they differ.
    Diff {
        a: PathBuf,
        b: PathBuf,
        /// One JSON document (`faris-config-diff/v1`) instead of text.
        #[arg(long)]
        json: bool,
    },
}

/// The settings problems, all at once; exits with status 2.
#[derive(Debug)]
pub struct SettingsRejected(pub Vec<String>);

impl std::fmt::Display for SettingsRejected {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} problem{} in the settings, nothing was started:",
            self.0.len(),
            if self.0.len() == 1 { "" } else { "s" }
        )?;
        for problem in &self.0 {
            write!(formatter, "\n  - {problem}")?;
        }
        Ok(())
    }
}

impl std::error::Error for SettingsRejected {}

struct Loaded {
    path: Option<PathBuf>,
    file: UserSettings,
}

fn load_file() -> Result<Loaded, Box<dyn std::error::Error>> {
    let path = settings::default_path();
    let file = match &path {
        None => UserSettings::default(),
        Some(path) => match std::fs::read(path) {
            Ok(bytes) => UserSettings::parse(&bytes, path).map_err(SettingsRejected)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => UserSettings::default(),
            Err(e) => {
                return Err(SettingsRejected(vec![format!(
                    "The settings file {} could not be read ({e}). Next step: check the file's \
                     permissions.",
                    path.display()
                )])
                .into());
            }
        },
    };
    Ok(Loaded { path, file })
}

fn resolve(loaded: &Loaded, flags: &[String]) -> Result<Vec<Resolved>, Box<dyn std::error::Error>> {
    let mut parsed = Vec::new();
    let mut problems = Vec::new();
    for flag in flags {
        match parse_flag(flag) {
            Ok(pair) => parsed.push(pair),
            Err(m) => problems.push(m),
        }
    }
    let path = loaded.path.clone().unwrap_or_default();
    let env = |name: &str| std::env::var_os(name);
    let layers = Layers {
        file: &loaded.file,
        file_path: &path,
        env: &env,
        flags: &parsed,
    };
    match layers.resolve_all() {
        Ok(all) if problems.is_empty() => Ok(all),
        Ok(_) => Err(SettingsRejected(problems).into()),
        Err(more) => {
            problems.extend(more);
            Err(SettingsRejected(problems).into())
        }
    }
}

/// Checks the whole settings stack before any command starts, so a bad file, variable or flag is
/// reported once with every problem and nothing begins. Applies the memory settings that something
/// other than the built-in default supplied.
pub fn startup(flags: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let all = resolve(&load_file()?, flags)?;
    let pick = |key: &str| all.iter().find(|r| r.key == key);
    let cap = pick("memory.cap_mb");
    let margin = pick("memory.margin_mb");
    if [cap, margin]
        .into_iter()
        .flatten()
        .any(|r| r.origin != Origin::Default)
    {
        let megabytes = |r: Option<&Resolved>| {
            r.and_then(|r| r.value.as_deref())
                .and_then(|v| v.parse::<u64>().ok())
                .map(|v| v.saturating_mul(MIB))
        };
        memory::set_policy(MemoryPolicy {
            cap_bytes: megabytes(cap),
            margin_bytes: megabytes(margin).unwrap_or(memory::DEFAULT_MARGIN_BYTES),
        });
    }
    Ok(())
}

fn definition(key: &str) -> Result<&'static SettingDef, Box<dyn std::error::Error>> {
    settings::lookup(key).ok_or_else(|| {
        SettingsRejected(vec![format!(
            "Unknown setting {key}. Next step: run `faris config list` for the valid keys."
        )])
        .into()
    })
}

fn shown(value: &Option<String>) -> &str {
    value.as_deref().unwrap_or("(not set)")
}

fn config_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    settings::default_path().ok_or_else(|| {
        "cannot find your configuration folder: none of FARIS_CONFIG_DIR, XDG_CONFIG_HOME, HOME (Linux, macOS) or \
         APPDATA (Windows) is set. Next step: set one of them."
            .into()
    })
}

fn write_file(path: &Path, file: &UserSettings) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create the folder {}: {e}", dir.display()))?;
    }
    atomic_file::write_atomic(path, &file.render())
        .map_err(|e| format!("cannot write {}: {e}", path.display()).into())
}

/// Reads a settings file by itself, and resolves it without the environment or flags.
fn resolve_alone(path: &Path) -> Result<Vec<Resolved>, Box<dyn std::error::Error>> {
    let bytes = std::fs::read(path).map_err(|e| {
        format!(
            "cannot read {}: {e}. Next step: check the path",
            path.display()
        )
    })?;
    let file = UserSettings::parse(&bytes, path).map_err(SettingsRejected)?;
    settings::resolve_file_alone(&file, path).map_err(|p| SettingsRejected(p).into())
}

fn diff_settings(a: &Path, b: &Path, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let changes = settings::diff_resolved(&resolve_alone(a)?, &resolve_alone(b)?);
    let result_changing = changes
        .iter()
        .filter(|c| c.effect == settings::EffectClass::ResultChanging)
        .count();
    if json {
        let rows: Vec<_> = changes
            .iter()
            .map(|c| {
                serde_json::json!({
                    "key": c.key, "old": c.old, "new": c.new, "unit": c.unit,
                    "old_layer": c.old_layer, "new_layer": c.new_layer,
                    "effect": c.effect.word(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schema": "faris-config-diff/v1",
                "a": a.display().to_string(), "b": b.display().to_string(),
                "identical": changes.is_empty(),
                "result_changing": result_changing,
                "changes": rows,
            }))?
        );
    } else if changes.is_empty() {
        println!("No differences between the two configurations.");
    } else {
        println!(
            "{} differences, {result_changing} result-changing",
            changes.len()
        );
        for c in &changes {
            let unit = if c.unit.is_empty() {
                String::new()
            } else {
                format!(" {}", c.unit)
            };
            println!(
                "  {}: {} -> {}{unit}  [{} -> {}]  ({})",
                c.key,
                shown(&c.old),
                shown(&c.new),
                c.old_layer,
                c.new_layer,
                c.effect.word()
            );
        }
    }
    if changes.is_empty() {
        Ok(())
    } else {
        Err(Box::new(crate::diff::DifferencesFound(format!(
            "the two configurations differ in {} settings",
            changes.len()
        ))))
    }
}

pub fn run(command: ConfigCommand, flags: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        ConfigCommand::Diff { a, b, json } => return diff_settings(&a, &b, json),
        ConfigCommand::Path => {
            println!("{}", config_path()?.display());
        }
        ConfigCommand::List { json } => {
            let all = resolve(&load_file()?, flags)?;
            if json {
                let rows: Vec<_> = all
                    .iter()
                    .zip(REGISTRY)
                    .map(|(r, d)| {
                        serde_json::json!({
                            "key": r.key, "value": r.value, "unit": d.unit,
                            "type": d.type_text(), "origin": r.origin.layer(),
                            "origin_detail": r.origin.describe(), "description": d.description,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                let width = REGISTRY.iter().map(|d| d.key.len()).max().unwrap_or(0);
                for (r, d) in all.iter().zip(REGISTRY) {
                    let unit = if d.unit.is_empty() {
                        String::new()
                    } else {
                        format!(" {}", d.unit)
                    };
                    println!(
                        "{:width$}  {}{unit}  [{}]",
                        r.key,
                        shown(&r.value),
                        r.origin.layer()
                    );
                }
            }
        }
        ConfigCommand::Get { key, show_origin } => {
            definition(&key)?;
            let all = resolve(&load_file()?, flags)?;
            let r = all
                .iter()
                .find(|r| r.key == key)
                .ok_or("setting not resolved")?;
            if show_origin {
                println!("{}  [{}]", shown(&r.value), r.origin.describe());
            } else {
                println!("{}", shown(&r.value));
            }
        }
        ConfigCommand::Set { key, value } => {
            let path = config_path()?;
            let mut loaded = load_file()?;
            loaded
                .file
                .set(&key, &value)
                .map_err(|m| SettingsRejected(vec![m]))?;
            write_file(&path, &loaded.file)?;
            println!("Set {key} in {}.", path.display());
            let all = resolve(&loaded, flags)?;
            if let Some(r) = all.iter().find(|r| r.key == key)
                && !matches!(r.origin, Origin::UserFile(_))
            {
                println!(
                    "Note: the effective value is still {} because of the {}, which wins over \
                     your settings file.",
                    shown(&r.value),
                    r.origin.describe()
                );
            }
        }
        ConfigCommand::Unset { key } => {
            let path = config_path()?;
            let mut loaded = load_file()?;
            if loaded
                .file
                .unset(&key)
                .map_err(|m| SettingsRejected(vec![m]))?
            {
                write_file(&path, &loaded.file)?;
                println!("Removed {key} from {}.", path.display());
            } else {
                println!("{key} was not set in {}; nothing changed.", path.display());
            }
        }
        ConfigCommand::Reset { yes } => {
            let path = config_path()?;
            if !yes {
                return Err(format!(
                    "this replaces every value in {} with the defaults. Next step: run `faris \
                     config reset --yes` to confirm.",
                    path.display()
                )
                .into());
            }
            // A damaged file is replaced too: that is the way out of it.
            let kept = load_file().map(|l| l.file.cleared()).unwrap_or_default();
            write_file(&path, &kept)?;
            println!("Reset {} to the defaults.", path.display());
        }
    }
    Ok(())
}
