//! User settings: how FARIS itself behaves on this machine (tool locations, memory caps,
//! display), never what a scenario means (requirements CFG-002, CFG-006, CFG-017, CFG-070,
//! CFG-080).
//!
//! Every setting is in [`REGISTRY`] with its type, range, default, unit and description. A value
//! comes from the first of these that sets it, last wins: the built-in default, the user settings
//! file, an environment variable, a command-line flag. Every resolved value says which layer
//! supplied it ([`Origin`]). There is no project layer: a setting that changes a result belongs in
//! the project's own inputs, and the registry refuses to hold one in the user file
//! ([`SettingDef::result_changing`]).
//!
//! The file is one JSON object versioned by `schema`. This module only parses and renders bytes;
//! reading and writing the file is the front end's job. The desktop's own sections
//! (`last_folders`, `extra_search_folders`) are kept as they are.

use serde_json::{Map, Value};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
};

/// The version of the file layout this program writes (CFG-070).
pub const SETTINGS_SCHEMA: u32 = 1;

pub const SETTINGS_FILE_NAME: &str = "settings.json";

/// The registry key prefix whose values live in the file's `tools` section.
const TOOLS_PREFIX: &str = "tools.";
/// Top-level sections the desktop writes and this module keeps untouched.
const DESKTOP_SECTIONS: [&str; 2] = ["last_folders", "extra_search_folders"];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ValueType {
    /// A file or folder path; not checked for existence, because a tool may be installed later.
    Path,
    Choice(&'static [&'static str]),
    Int {
        min: i64,
        max: i64,
    },
    Float {
        min: f64,
        max: f64,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct SettingDef {
    pub key: &'static str,
    pub value_type: ValueType,
    /// `None`: not set, and the program says so where it is needed.
    pub default: Option<&'static str>,
    /// Empty for unitless values and paths.
    pub unit: &'static str,
    pub description: &'static str,
    pub env: Option<&'static str>,
    /// True for a setting that changes a computed result. Such a setting belongs in the project
    /// and is refused in the user file (CFG-006). It must carry a `kind_label` (CFG-018).
    pub result_changing: bool,
    /// One of calculated, authored, literature, conditional, not-evaluated.
    pub kind_label: Option<&'static str>,
}

const fn def(
    key: &'static str,
    value_type: ValueType,
    default: Option<&'static str>,
    unit: &'static str,
    env: Option<&'static str>,
    description: &'static str,
) -> SettingDef {
    SettingDef {
        key,
        value_type,
        default,
        unit,
        description,
        env,
        result_changing: false,
        kind_label: None,
    }
}

/// Every setting. Keys are `section.name`; the six `tools.` keys keep the names the desktop
/// already stores.
pub static REGISTRY: &[SettingDef] = &[
    def(
        "tools.cad_python",
        ValueType::Path,
        None,
        "",
        Some("FARIS_CAD_PYTHON"),
        "Python interpreter with cadquery, used to read STEP models when the native reader is not used.",
    ),
    def(
        "tools.openmc_python",
        ValueType::Path,
        None,
        "",
        Some("FARIS_OPENMC_PYTHON"),
        "Python interpreter with OpenMC, used for transport runs.",
    ),
    def(
        "tools.cross_sections",
        ValueType::Path,
        None,
        "",
        Some("FARIS_CROSS_SECTIONS"),
        "The cross_sections.xml file of the nuclear data library OpenMC uses.",
    ),
    def(
        "tools.plasma_source",
        ValueType::Path,
        None,
        "",
        Some("FARIS_PLASMA_SOURCE_PATH"),
        "Folder holding the openmc_plasma_source package, for the plasma source sampler.",
    ),
    def(
        "tools.window_python",
        ValueType::Path,
        None,
        "",
        None,
        "Python interpreter with OpenMC 0.16 or later, used to make weight windows.",
    ),
    def(
        "tools.openmc_package",
        ValueType::Path,
        None,
        "",
        None,
        "Folder of an OpenMC built with the aiming patch.",
    ),
    def(
        "tools.cad_reader",
        ValueType::Choice(&["auto", "native", "python"]),
        Some("auto"),
        "",
        Some("FARIS_CAD_READER"),
        "Which reader opens STEP models: the native worker, the Python helper, or whichever is found.",
    ),
    def(
        "tools.occt_worker",
        ValueType::Path,
        None,
        "",
        Some("FARIS_OCCT_WORKER"),
        "The native CAD worker program; when unset, the one beside the running program.",
    ),
    def(
        "tools.core_executable",
        ValueType::Path,
        None,
        "",
        Some("FARIS_CORE_EXECUTABLE"),
        "The Avila Core program that compiles and runs studies; when unset, the one on PATH.",
    ),
    def(
        "memory.cap_mb",
        ValueType::Int {
            min: 1,
            max: 4_194_304,
        },
        None,
        "MiB",
        Some("FARIS_MEMORY_CAP_MB"),
        "Address-space cap applied to every external job; when unset, no cap is applied and a job is checked against the memory that is free.",
    ),
    def(
        "memory.margin_mb",
        ValueType::Int {
            min: 1,
            max: 4_194_304,
        },
        Some("1024"),
        "MiB",
        Some("FARIS_MEMORY_MARGIN_MB"),
        "Memory kept free for the rest of the machine when the free-memory check is made before a job starts.",
    ),
    def(
        "ui.scale",
        ValueType::Float { min: 0.5, max: 4.0 },
        Some("1"),
        "x",
        None,
        "Interface size as a multiple of the desktop's own scaling. Reserved for the desktop, which does not read it yet.",
    ),
    def(
        "ui.theme",
        ValueType::Choice(&["system", "light", "dark"]),
        Some("system"),
        "",
        None,
        "Colour theme. Reserved for the desktop, which does not read it yet.",
    ),
];

pub fn lookup(key: &str) -> Option<&'static SettingDef> {
    REGISTRY.iter().find(|d| d.key == key)
}

impl SettingDef {
    /// The type and range as one phrase, for messages and the handbook.
    pub fn type_text(&self) -> String {
        match self.value_type {
            ValueType::Path => "path".into(),
            ValueType::Choice(options) => format!("one of {}", options.join(", ")),
            ValueType::Int { min, max } => format!("whole number {min} to {max}"),
            ValueType::Float { min, max } => format!("number {min} to {max}"),
        }
    }

    /// Checks `text` and returns it in its stored form (trimmed; numbers in their shortest form).
    /// The message says what is wrong and what is allowed.
    pub fn normalize(&self, text: &str) -> Result<String, String> {
        let text = text.trim();
        let bad = |why: &str| {
            format!(
                "{}: {why} (allowed: {}). Next step: give a value of that kind, or run \
                 `faris config get {}` to see the current one.",
                self.key,
                self.type_text(),
                self.key
            )
        };
        match self.value_type {
            ValueType::Path => {
                if text.is_empty() {
                    Err(bad(
                        "the path is empty; to clear the setting use `faris config unset`",
                    ))
                } else if text.contains('\0') {
                    Err(bad("the path contains a null character"))
                } else {
                    Ok(text.into())
                }
            }
            ValueType::Choice(options) => options
                .contains(&text)
                .then(|| text.to_string())
                .ok_or_else(|| bad(&format!("'{text}' is not one of the choices"))),
            ValueType::Int { min, max } => match text.parse::<i64>() {
                Ok(n) if (min..=max).contains(&n) => Ok(n.to_string()),
                Ok(n) => Err(bad(&format!("{n} is outside the range"))),
                Err(_) => Err(bad(&format!("'{text}' is not a whole number"))),
            },
            ValueType::Float { min, max } => match text.parse::<f64>() {
                Ok(x) if x.is_finite() && x >= min && x <= max => Ok(format!("{x}")),
                Ok(_) => Err(bad(&format!("'{text}' is outside the range"))),
                Err(_) => Err(bad(&format!("'{text}' is not a number"))),
            },
        }
    }

    fn json_value(&self, stored: &str) -> Value {
        match self.value_type {
            ValueType::Int { .. } => stored.parse::<i64>().map_or(Value::Null, Value::from),
            ValueType::Float { .. } => stored.parse::<f64>().map_or(Value::Null, Value::from),
            _ => Value::String(stored.into()),
        }
    }
}

/// Which layer supplied a value (CFG-002, CFG-080).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    Default,
    UserFile(PathBuf),
    Environment(String),
    Flag(String),
}

impl Origin {
    /// Short name of the layer: default, user file, environment, flag.
    pub fn layer(&self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::UserFile(_) => "user file",
            Self::Environment(_) => "environment",
            Self::Flag(_) => "flag",
        }
    }

    /// The layer with the file or variable that supplied the value.
    pub fn describe(&self) -> String {
        match self {
            Self::Default => "built-in default".into(),
            Self::UserFile(path) => format!("user file {}", path.display()),
            Self::Environment(name) => format!("environment variable {name}"),
            Self::Flag(flag) => format!("command-line flag {flag}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub key: &'static str,
    /// `None`: not set in any layer.
    pub value: Option<String>,
    pub origin: Origin,
}

/// The user settings file, held as registry keys to stored text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UserSettings {
    entries: BTreeMap<String, String>,
    desktop: Map<String, Value>,
}

impl UserSettings {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Validates and stores. A result-changing setting is refused here (CFG-006).
    pub fn set(&mut self, key: &str, text: &str) -> Result<(), String> {
        let def = lookup(key).ok_or_else(|| unknown_message(&[key.to_string()]))?;
        check_user_layer(def)?;
        let stored = def.normalize(text)?;
        self.entries.insert(key.into(), stored);
        Ok(())
    }

    /// Removes a value so the lower layers apply again; `Ok(false)` when it was not set.
    pub fn unset(&mut self, key: &str) -> Result<bool, String> {
        lookup(key).ok_or_else(|| unknown_message(&[key.to_string()]))?;
        Ok(self.entries.remove(key).is_some())
    }

    /// An empty settings file that keeps the desktop's sections, which this module does not own.
    pub fn cleared(&self) -> Self {
        Self {
            entries: BTreeMap::new(),
            desktop: self.desktop.clone(),
        }
    }

    /// The file bytes this settings object stands for.
    pub fn render(&self) -> Vec<u8> {
        let mut root = Map::new();
        root.insert("schema".into(), SETTINGS_SCHEMA.into());
        let mut tools = Map::new();
        let mut values = Map::new();
        for (key, stored) in &self.entries {
            let Some(def) = lookup(key) else { continue };
            match key.strip_prefix(TOOLS_PREFIX) {
                Some(name) => tools.insert(name.into(), def.json_value(stored)),
                None => values.insert(key.clone(), def.json_value(stored)),
            };
        }
        root.insert("tools".into(), tools.into());
        root.insert("values".into(), values.into());
        for (name, section) in &self.desktop {
            root.insert(name.clone(), section.clone());
        }
        let mut bytes = serde_json::to_vec_pretty(&root).unwrap_or_default();
        bytes.push(b'\n');
        bytes
    }

    /// Reads file bytes. Every problem is returned, not only the first (CFG-017): unknown keys,
    /// values out of range, a newer version, a damaged file. `path` is only named in messages.
    pub fn parse(bytes: &[u8], path: &Path) -> Result<Self, Vec<String>> {
        let at = path.display();
        let root: Value = serde_json::from_slice(bytes).map_err(|e| {
            vec![format!(
                "The settings file {at} is not valid JSON ({e}). Next step: fix or delete the file; \
                 `faris config reset --yes` writes a fresh one."
            )]
        })?;
        let Value::Object(mut root) = root else {
            return Err(vec![format!(
                "The settings file {at} is not a JSON object. Next step: fix or delete the file."
            )]);
        };
        let version = match root.remove("schema") {
            Some(Value::Number(n)) => n.as_u64().and_then(|v| u32::try_from(v).ok()),
            _ => None,
        };
        let Some(version) = version else {
            return Err(vec![format!(
                "The settings file {at} has no usable `schema` version, so FARIS cannot tell \
                 which layout it is. Next step: add `\"schema\": {SETTINGS_SCHEMA}`, or delete the \
                 file."
            )]);
        };
        if version > SETTINGS_SCHEMA {
            return Err(vec![format!(
                "The settings file {at} was written by a newer FARIS (settings version {version}; \
                 this one reads up to {SETTINGS_SCHEMA}). It is left as it is. Next step: use the \
                 newer FARIS, or delete that file to start again."
            )]);
        }
        let root = migrate(root, version);

        let mut problems = Vec::new();
        let mut unknown = Vec::new();
        let mut settings = Self::default();
        let mut items: Vec<(String, Value)> = Vec::new();
        for (name, section) in root {
            match (name.as_str(), section) {
                ("tools", Value::Object(tools)) => items.extend(
                    tools
                        .into_iter()
                        .map(|(tool, json)| (format!("{TOOLS_PREFIX}{tool}"), json)),
                ),
                ("values", Value::Object(values)) => items.extend(values),
                (n, section) if DESKTOP_SECTIONS.contains(&n) => {
                    settings.desktop.insert(name, section);
                }
                (n @ ("tools" | "values"), _) => problems.push(format!(
                    "The `{n}` section of {at} is not a JSON object. Next step: fix or delete it."
                )),
                (other, _) => unknown.push(other.to_string()),
            }
        }
        for (key, json) in items {
            let Some(def) = lookup(&key) else {
                unknown.push(key);
                continue;
            };
            let text = match &json {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                other => {
                    problems.push(format!(
                        "{key}: the file holds {other}, which is not a {} (allowed: {}). Next \
                         step: correct it in {at} or run `faris config unset {key}`.",
                        kind_word(def),
                        def.type_text()
                    ));
                    continue;
                }
            };
            match def
                .normalize(&text)
                .and_then(|v| check_user_layer(def).map(|()| v))
            {
                Ok(stored) => {
                    settings.entries.insert(key, stored);
                }
                Err(message) => problems.push(message),
            }
        }
        if !unknown.is_empty() {
            problems.insert(0, unknown_message(&unknown));
        }
        if problems.is_empty() {
            Ok(settings)
        } else {
            Err(problems)
        }
    }
}

/// The user layer holds no setting that changes a result (CFG-006).
fn check_user_layer(def: &SettingDef) -> Result<(), String> {
    if def.result_changing {
        return Err(format!(
            "{} changes computed results, so it belongs in the project and not in the user \
             settings. Next step: set it in the study's own inputs.",
            def.key
        ));
    }
    Ok(())
}

fn kind_word(def: &SettingDef) -> &'static str {
    match def.value_type {
        ValueType::Path => "path (text)",
        ValueType::Choice(_) => "choice (text)",
        ValueType::Int { .. } => "whole number",
        ValueType::Float { .. } => "number",
    }
}

fn unknown_message(keys: &[String]) -> String {
    format!(
        "Unknown setting{} {}. FARIS refuses settings it does not know, so a typo cannot pass \
         silently. Next step: run `faris config list` for the valid keys, and remove or correct \
         the unknown ones.",
        if keys.len() == 1 { "" } else { "s" },
        keys.join(", ")
    )
}

/// Brings a file written at an older `from` version to the current layout, one step at a time.
/// Version 0 was the first layout, which held only the `tools` section; it needs no change
/// beyond the version bump.
fn migrate(mut root: Map<String, Value>, from: u32) -> Map<String, Value> {
    let mut version = from;
    while version < SETTINGS_SCHEMA {
        // Each arm is one explicit step from `version` to `version + 1`.
        if version == 0 {
            root = migrate_0_to_1(root);
        }
        version += 1;
    }
    root
}

fn migrate_0_to_1(root: Map<String, Value>) -> Map<String, Value> {
    root
}

/// Layers above the user file, as the front end collected them.
pub struct Layers<'a> {
    pub file: &'a UserSettings,
    pub file_path: &'a Path,
    pub env: &'a dyn Fn(&str) -> Option<OsString>,
    /// `(key, value)` pairs from the command line, in the order given; the last one wins.
    pub flags: &'a [(String, String)],
}

impl Layers<'_> {
    /// Every setting, resolved. All problems are returned together: a bad environment value, an
    /// unknown flag key, a value out of range (CFG-017).
    pub fn resolve_all(&self) -> Result<Vec<Resolved>, Vec<String>> {
        let mut problems = Vec::new();
        let mut flags: BTreeMap<&str, &str> = BTreeMap::new();
        let mut unknown = Vec::new();
        for (key, value) in self.flags {
            if lookup(key).is_none() {
                unknown.push(key.clone());
            } else {
                flags.insert(key, value);
            }
        }
        if !unknown.is_empty() {
            problems.push(unknown_message(&unknown));
        }
        let mut out = Vec::with_capacity(REGISTRY.len());
        for def in REGISTRY {
            let mut resolved = Resolved {
                key: def.key,
                value: def.default.map(String::from),
                origin: Origin::Default,
            };
            if let Some(v) = self.file.get(def.key) {
                resolved.value = Some(v.into());
                resolved.origin = Origin::UserFile(self.file_path.to_path_buf());
            }
            if let Some(name) = def.env
                && let Some(raw) = (self.env)(name).filter(|v| !v.is_empty())
            {
                match def.normalize(&raw.to_string_lossy()) {
                    Ok(v) => {
                        resolved.value = Some(v);
                        resolved.origin = Origin::Environment(name.into());
                    }
                    Err(m) => problems.push(format!(
                        "Environment variable {name} for setting {m} Next step: correct or unset \
                         the variable."
                    )),
                }
            }
            if let Some(raw) = flags.get(def.key) {
                match def.normalize(raw) {
                    Ok(v) => {
                        resolved.value = Some(v);
                        resolved.origin = Origin::Flag(format!("--setting {}", def.key));
                    }
                    Err(m) => problems.push(format!("Flag --setting {m}")),
                }
            }
            out.push(resolved);
        }
        if problems.is_empty() {
            Ok(out)
        } else {
            Err(problems)
        }
    }
}

/// Splits `KEY=VALUE` as the `--setting` flag gives it.
pub fn parse_flag(text: &str) -> Result<(String, String), String> {
    text.split_once('=')
        .map(|(k, v)| (k.trim().to_string(), v.to_string()))
        .ok_or_else(|| {
            format!(
                "--setting {text}: expected KEY=VALUE, for example --setting memory.cap_mb=2048. \
                 Next step: add the '=' and a value."
            )
        })
}

/// The platform a settings folder is looked up for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Linux,
    MacOs,
    Windows,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }
}

/// The per-user configuration folder for FARIS: `$XDG_CONFIG_HOME/faris` or `~/.config/faris`
/// (Linux), `~/Library/Application Support/faris` (macOS), `%APPDATA%\faris` (Windows).
/// `FARIS_CONFIG_DIR`, when set to an absolute folder, replaces all of these on every platform. The same
/// folder the desktop uses. `None` when the variables that name it are unset.
pub fn config_dir_with(
    platform: Platform,
    var: &dyn Fn(&str) -> Option<OsString>,
) -> Option<PathBuf> {
    let nonempty = |name: &str| var(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(folder) = nonempty("FARIS_CONFIG_DIR").filter(|p| p.is_absolute()) {
        return Some(folder);
    }
    let base = match platform {
        Platform::Linux => nonempty("XDG_CONFIG_HOME")
            .filter(|p| p.is_absolute())
            .or_else(|| nonempty("HOME").map(|h| h.join(".config"))),
        Platform::MacOs => nonempty("HOME").map(|h| h.join("Library").join("Application Support")),
        Platform::Windows => nonempty("APPDATA")
            .or_else(|| nonempty("USERPROFILE").map(|h| h.join("AppData").join("Roaming"))),
    }?;
    Some(base.join("faris"))
}

/// The settings file of this user on this machine.
pub fn default_path() -> Option<PathBuf> {
    config_dir_with(Platform::current(), &|n| std::env::var_os(n))
        .map(|d| d.join(SETTINGS_FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<OsString> {
        None
    }

    fn resolve(
        file: &UserSettings,
        env: &[(&str, &str)],
        flags: &[(&str, &str)],
    ) -> Result<Vec<Resolved>, Vec<String>> {
        let env = |name: &str| {
            env.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| OsString::from(v))
        };
        let flags: Vec<(String, String)> = flags
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Layers {
            file,
            file_path: Path::new("/u/settings.json"),
            env: &env,
            flags: &flags,
        }
        .resolve_all()
    }

    fn get<'a>(all: &'a [Resolved], key: &str) -> &'a Resolved {
        all.iter().find(|r| r.key == key).unwrap()
    }

    /// The handbook's table row for a setting: the page and the registry must agree.
    fn handbook_row(d: &SettingDef) -> String {
        format!(
            "| `{}` | {} | {} | {} | {} | {} |",
            d.key,
            d.type_text(),
            d.default.map_or("not set".into(), |v| format!("`{v}`")),
            if d.unit.is_empty() { "-" } else { d.unit },
            d.env.map_or("-".into(), |e| format!("`{e}`")),
            d.description
        )
    }

    #[test]
    fn registry_is_well_formed() {
        let mut seen = std::collections::BTreeSet::new();
        for d in REGISTRY {
            assert!(seen.insert(d.key), "duplicate key {}", d.key);
            assert!(d.key.contains('.'), "{}", d.key);
            assert!(!d.description.is_empty(), "{}", d.key);
            assert!(!d.description.contains('|'), "{}", d.key);
            if let Some(default) = d.default {
                assert_eq!(d.normalize(default).as_deref(), Ok(default), "{}", d.key);
            }
            if d.result_changing {
                assert!(!d.unit.is_empty() || d.kind_label.is_some(), "{}", d.key);
                assert!(d.kind_label.is_some(), "{}", d.key);
            }
        }
    }

    #[test]
    fn handbook_page_lists_exactly_the_registry() {
        // Verifies: CFG-002
        let page = include_str!("../../../docs/guide/settings.md");
        assert!(page.starts_with("<!-- kind: reference -->"));
        let (_, table) = page
            .split_once("## All settings")
            .expect("settings section");
        let rows: Vec<&str> = table.lines().filter(|l| l.starts_with("| `")).collect();
        assert_eq!(
            rows.len(),
            REGISTRY.len(),
            "rows on the page vs the registry"
        );
        for d in REGISTRY {
            let want = handbook_row(d);
            assert!(
                rows.contains(&want.as_str()),
                "missing or stale row: {want}"
            );
        }
    }

    #[test]
    fn layers_win_in_order_and_each_value_names_its_origin() {
        // Verifies: CFG-002
        // Verifies: CFG-080
        let mut file = UserSettings::default();
        file.set("memory.margin_mb", "2048").unwrap();
        file.set("memory.cap_mb", "1000").unwrap();
        file.set("ui.theme", "dark").unwrap();

        let all = resolve(
            &file,
            &[("FARIS_MEMORY_CAP_MB", "3000")],
            &[("memory.margin_mb", "512")],
        )
        .unwrap();
        let cap = get(&all, "memory.cap_mb");
        assert_eq!(cap.value.as_deref(), Some("3000"));
        assert_eq!(
            cap.origin,
            Origin::Environment("FARIS_MEMORY_CAP_MB".into())
        );
        let margin = get(&all, "memory.margin_mb");
        assert_eq!(margin.value.as_deref(), Some("512"));
        assert!(matches!(margin.origin, Origin::Flag(_)));
        let theme = get(&all, "ui.theme");
        assert_eq!(theme.value.as_deref(), Some("dark"));
        assert_eq!(theme.origin, Origin::UserFile("/u/settings.json".into()));
        let scale = get(&all, "ui.scale");
        assert_eq!(scale.value.as_deref(), Some("1"));
        assert_eq!(scale.origin, Origin::Default);
        let tool = get(&all, "tools.cad_python");
        assert_eq!(tool.value, None);
        assert_eq!(tool.origin, Origin::Default);
    }

    #[test]
    fn every_setting_resolves_with_a_nonempty_origin_description() {
        // Verifies: CFG-002
        // Verifies: CFG-080
        let all = resolve(&UserSettings::default(), &[], &[]).unwrap();
        assert_eq!(all.len(), REGISTRY.len());
        for r in &all {
            assert!(!r.origin.describe().is_empty(), "{}", r.key);
            assert!(!r.origin.layer().is_empty(), "{}", r.key);
        }
        // Each layer, for a setting that has an environment variable.
        let mut file = UserSettings::default();
        file.set("tools.cad_python", "/f/py").unwrap();
        for (env, flags, layer) in [
            (vec![], vec![], "user file"),
            (vec![("FARIS_CAD_PYTHON", "/e/py")], vec![], "environment"),
            (
                vec![("FARIS_CAD_PYTHON", "/e/py")],
                vec![("tools.cad_python", "/x/py")],
                "flag",
            ),
        ] {
            let all = resolve(&file, &env, &flags).unwrap();
            assert_eq!(get(&all, "tools.cad_python").origin.layer(), layer);
        }
    }

    #[test]
    fn validation_reports_every_problem_at_once() {
        // Verifies: CFG-017
        let bytes = br#"{
            "schema": 1,
            "tools": {"cad_reader": "fast", "nonsense": "x"},
            "values": {"memory.cap_mb": 0, "ui.scale": 9.5, "ui.theme": 3, "memory.margn_mb": 1}
        }"#;
        let problems = UserSettings::parse(bytes, Path::new("/u/settings.json")).unwrap_err();
        let all = problems.join("\n");
        for needle in [
            "nonsense",
            "memory.margn_mb",
            "tools.cad_reader",
            "memory.cap_mb",
            "ui.scale",
            "ui.theme",
        ] {
            assert!(all.contains(needle), "{needle} not reported in:\n{all}");
        }
        assert!(all.contains("Next step"), "{all}");
        // Five bad settings and two unknown keys in one message list: unknown keys share one entry.
        assert_eq!(problems.len(), 5, "{all}");
    }

    #[test]
    fn bad_environment_and_flag_values_are_reported_together() {
        // Verifies: CFG-017
        let problems = resolve(
            &UserSettings::default(),
            &[("FARIS_MEMORY_CAP_MB", "lots"), ("FARIS_CAD_READER", "gpu")],
            &[("ui.scale", "huge"), ("no.such", "1")],
        )
        .unwrap_err();
        assert_eq!(problems.len(), 4, "{problems:?}");
    }

    #[test]
    fn unknown_keys_and_newer_versions_are_refused_with_a_reason() {
        // Verifies: CFG-070
        let path = Path::new("/u/settings.json");
        let unknown = UserSettings::parse(br#"{"schema":1,"extra":true}"#, path).unwrap_err();
        assert!(unknown[0].contains("Unknown setting extra"), "{unknown:?}");
        assert!(unknown[0].contains("Next step"));
        let newer = UserSettings::parse(br#"{"schema":2}"#, path).unwrap_err();
        assert!(newer[0].contains("newer FARIS"), "{newer:?}");
        assert!(newer[0].contains("Next step"));
        let missing = UserSettings::parse(br#"{"tools":{}}"#, path).unwrap_err();
        assert!(missing[0].contains("schema"), "{missing:?}");
    }

    #[test]
    fn an_older_version_migrates_and_is_written_at_the_current_one() {
        // Verifies: CFG-070
        let path = Path::new("/u/settings.json");
        let old = UserSettings::parse(br#"{"schema":0,"tools":{"cad_python":"/p"}}"#, path)
            .expect("version 0 migrates");
        assert_eq!(old.get("tools.cad_python"), Some("/p"));
        let written: Value = serde_json::from_slice(&old.render()).unwrap();
        assert_eq!(written["schema"], SETTINGS_SCHEMA);
    }

    #[test]
    fn every_registry_file_type_in_the_settings_file_carries_a_version() {
        // Verifies: CFG-070
        let written: Value = serde_json::from_slice(&UserSettings::default().render()).unwrap();
        assert_eq!(written["schema"], SETTINGS_SCHEMA);
    }

    #[test]
    fn file_round_trips_and_keeps_the_desktops_sections() {
        let path = Path::new("/u/settings.json");
        let bytes = br#"{
            "schema": 1,
            "tools": {"openmc_python": "/o/py"},
            "values": {"ui.scale": 1.5, "memory.cap_mb": 4096},
            "last_folders": {"design_check": "/d"},
            "extra_search_folders": ["/x"]
        }"#;
        let parsed = UserSettings::parse(bytes, path).unwrap();
        assert_eq!(parsed.get("ui.scale"), Some("1.5"));
        assert_eq!(parsed.get("memory.cap_mb"), Some("4096"));
        let again = UserSettings::parse(&parsed.render(), path).unwrap();
        assert_eq!(again, parsed);
        let written: Value = serde_json::from_slice(&parsed.render()).unwrap();
        assert_eq!(written["last_folders"]["design_check"], "/d");
        assert_eq!(written["extra_search_folders"][0], "/x");
        assert_eq!(written["values"]["memory.cap_mb"], 4096);
    }

    #[test]
    fn set_validates_and_unset_restores_the_lower_layer() {
        let mut file = UserSettings::default();
        assert!(file.set("memory.cap_mb", "0").is_err());
        assert!(file.set("nope", "1").is_err());
        file.set("ui.scale", " 1.50 ").unwrap();
        assert_eq!(file.get("ui.scale"), Some("1.5"));
        assert_eq!(file.unset("ui.scale"), Ok(true));
        assert_eq!(file.unset("ui.scale"), Ok(false));
        assert!(file.unset("nope").is_err());
        assert!(file.is_empty());
    }

    #[test]
    fn result_changing_settings_are_refused_in_the_user_layer() {
        // Verifies: CFG-006
        // The registry holds none today; prove the gate with a synthetic definition.
        assert!(REGISTRY.iter().all(|d| !d.result_changing));
        let synthetic = SettingDef {
            result_changing: true,
            kind_label: Some("authored"),
            ..def(
                "physics.test",
                ValueType::Float { min: 0.0, max: 1.0 },
                None,
                "1",
                None,
                "test",
            )
        };
        let message = check_user_layer(&synthetic).unwrap_err();
        assert!(message.contains("belongs in the project"), "{message}");
        assert!(check_user_layer(lookup("ui.scale").unwrap()).is_ok());
    }

    #[test]
    fn config_folder_follows_the_platform() {
        // Absolute on the host, so the test means the same on Linux, macOS and Windows.
        let root = std::env::temp_dir();
        let (x, h) = (root.join("x"), root.join("h"));
        let var = |name: &str| match name {
            "XDG_CONFIG_HOME" => Some(x.clone().into_os_string()),
            "HOME" => Some(h.clone().into_os_string()),
            _ => None,
        };
        assert_eq!(
            config_dir_with(Platform::Linux, &var),
            Some(x.join("faris"))
        );
        assert_eq!(
            config_dir_with(Platform::Linux, &no_env),
            None,
            "no variables, no folder"
        );
        assert_eq!(
            config_dir_with(Platform::MacOs, &var),
            Some(h.join("Library").join("Application Support").join("faris"))
        );
        let chosen = root.join("chosen");
        let with_override = |name: &str| match name {
            "FARIS_CONFIG_DIR" => Some(chosen.clone().into_os_string()),
            other => var(other),
        };
        for platform in [Platform::Linux, Platform::MacOs, Platform::Windows] {
            assert_eq!(
                config_dir_with(platform, &with_override),
                Some(chosen.clone())
            );
        }
    }
}
