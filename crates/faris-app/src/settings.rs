//! The desktop's small settings file: where the six external tools are (so nobody retypes them),
//! the folders the design windows last used and the folders "Find tools" also searches.
//!
//! One JSON file in the platform's per-user configuration folder, versioned by `schema`, written
//! atomically with the engine's shared writer. Reading never fails the program: a missing file is
//! an empty one, and a file that cannot be read is copied beside itself as `settings.json.bad`
//! and replaced by an empty one, with a notice for the user.

use faris_engine::atomic_file;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
};

/// The version of the file layout this program writes.
pub const SCHEMA: u32 = 1;

pub const FILE_NAME: &str = "settings.json";

/// The six external tools the design windows ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tool {
    CadPython,
    OpenmcPython,
    CrossSections,
    PlasmaSource,
    WindowPython,
    OpenmcPackage,
}

impl Tool {
    pub const ALL: [Tool; 6] = [
        Tool::CadPython,
        Tool::OpenmcPython,
        Tool::CrossSections,
        Tool::PlasmaSource,
        Tool::WindowPython,
        Tool::OpenmcPackage,
    ];

    /// The key in the settings file.
    pub fn key(self) -> &'static str {
        match self {
            Self::CadPython => "cad_python",
            Self::OpenmcPython => "openmc_python",
            Self::CrossSections => "cross_sections",
            Self::PlasmaSource => "plasma_source",
            Self::WindowPython => "window_python",
            Self::OpenmcPackage => "openmc_package",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::CadPython => "CAD Python",
            Self::OpenmcPython => "OpenMC Python",
            Self::CrossSections => "Cross sections",
            Self::PlasmaSource => "Plasma source folder",
            Self::WindowPython => "Window Python",
            Self::OpenmcPackage => "OpenMC package folder",
        }
    }

    /// The environment variable the engine reads for this tool, if it reads one.
    pub fn environment(self) -> Option<&'static str> {
        match self {
            Self::CadPython => Some("FARIS_CAD_PYTHON"),
            Self::OpenmcPython => Some("FARIS_OPENMC_PYTHON"),
            Self::CrossSections => Some("FARIS_CROSS_SECTIONS"),
            Self::PlasmaSource => Some("FARIS_PLASMA_SOURCE_PATH"),
            Self::WindowPython | Self::OpenmcPackage => None,
        }
    }

    pub fn is_folder(self) -> bool {
        matches!(self, Self::PlasmaSource | Self::OpenmcPackage)
    }
}

/// The folders the two design windows last picked a file in.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastFolders {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub design_check: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub design_flux: Option<String>,
}

/// What the file holds. Every field has a default, so a file from an older version still reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub schema: u32,
    /// Tool paths by `Tool::key`; a tool with no entry is not set.
    #[serde(default)]
    pub tools: BTreeMap<String, String>,
    #[serde(default)]
    pub last_folders: LastFolders,
    /// Folders "Find tools" also looks in: environments, nuclear data, packages.
    #[serde(default)]
    pub extra_search_folders: Vec<String>,
    /// Sections this window does not edit (the command line's `values`, written by `faris config`),
    /// kept as read so saving here never drops them.
    #[serde(flatten)]
    pub other: BTreeMap<String, serde_json::Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            tools: BTreeMap::new(),
            last_folders: LastFolders::default(),
            extra_search_folders: Vec::new(),
            other: BTreeMap::new(),
        }
    }
}

impl Settings {
    /// The value as stored, untrimmed: what a text field shows while it is being typed in.
    pub fn raw(&self, tool: Tool) -> &str {
        self.tools.get(tool.key()).map_or("", String::as_str)
    }

    /// The value to use: trimmed, and `None` when blank.
    pub fn tool(&self, tool: Tool) -> Option<&str> {
        Some(self.raw(tool).trim()).filter(|v| !v.is_empty())
    }

    /// Stores the text as typed; an empty text removes the entry.
    pub fn set_tool(&mut self, tool: Tool, value: &str) {
        if value.is_empty() {
            self.tools.remove(tool.key());
        } else {
            self.tools.insert(tool.key().into(), value.into());
        }
    }

    pub fn has_any_tool(&self) -> bool {
        Tool::ALL.iter().any(|t| self.tool(*t).is_some())
    }
}

/// What reading the file gave.
#[derive(Debug, PartialEq, Eq)]
pub struct Loaded {
    pub settings: Settings,
    /// Said to the user once: the file could not be read, or is from a newer FARIS.
    pub notice: Option<String>,
    /// False when the file is from a newer FARIS: this one must not overwrite it.
    pub writable: bool,
}

fn empty(notice: Option<String>, writable: bool) -> Loaded {
    Loaded {
        settings: Settings::default(),
        notice,
        writable,
    }
}

/// Reads the settings at `path`; never fails.
pub fn load(path: &Path) -> Loaded {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return empty(None, true),
        Err(e) => {
            return empty(
                Some(format!(
                    "The settings file {} could not be read ({e}). FARIS starts with no saved \
                     settings and will not change that file until you change a setting. Next \
                     step: check the file's permissions.",
                    path.display()
                )),
                false,
            );
        }
    };
    match serde_json::from_slice::<Settings>(&bytes) {
        Ok(settings) if settings.schema > SCHEMA => empty(
            Some(format!(
                "The settings file {} was written by a newer FARIS (settings version {}; this \
                 one reads version {SCHEMA}). It is left as it is and your choices this session \
                 are not saved. Next step: use the newer FARIS, or delete that file to start \
                 again.",
                path.display(),
                settings.schema
            )),
            false,
        ),
        Ok(settings) => Loaded {
            settings,
            notice: None,
            writable: true,
        },
        Err(e) => {
            let bad = bad_copy_path(path);
            let kept = match std::fs::copy(path, &bad) {
                Ok(_) => format!("A copy is kept as {}.", bad.display()),
                Err(copy_error) => format!("A copy could not be kept ({copy_error})."),
            };
            empty(
                Some(format!(
                    "The settings file {} could not be read ({e}), so FARIS started with no \
                     saved settings. {kept} Next step: set the tools again (Find tools does it \
                     in one click).",
                    path.display()
                )),
                true,
            )
        }
    }
}

/// `settings.json` -> `settings.json.bad`.
pub fn bad_copy_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".bad");
    PathBuf::from(name)
}

/// Writes the settings atomically, creating the folder.
pub fn save(path: &Path, settings: &Settings) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create the folder {}: {e}", dir.display()))?;
    }
    atomic_file::write_atomic(path, &bytes)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
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
/// `FARIS_CONFIG_DIR`, when set to an absolute folder, replaces all of these on every platform.
/// `None` when the variables that name it are unset.
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
    config_dir_with(Platform::current(), &|n| std::env::var_os(n)).map(|d| d.join(FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn saving_keeps_the_sections_the_command_line_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(
            &path,
            r#"{"schema":1,"tools":{},"values":{"memory.cap_mb":4096,"ui.theme":"dark"}}"#,
        )
        .unwrap();
        let mut loaded = load(&path).settings;
        loaded.set_tool(Tool::CadPython, "/opt/cad/bin/python");
        save(&path, &loaded).unwrap();
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["values"]["memory.cap_mb"], 4096);
        assert_eq!(written["values"]["ui.theme"], "dark");
        assert_eq!(written["tools"]["cad_python"], "/opt/cad/bin/python");
    }

    #[test]
    fn settings_round_trip_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("faris").join(FILE_NAME);
        let mut settings = Settings::default();
        settings.set_tool(Tool::CadPython, "/opt/cad/bin/python");
        settings.set_tool(Tool::PlasmaSource, "/data/plasma");
        settings.last_folders.design_check = Some("/work/check".into());
        settings.extra_search_folders.push("/work/envs".into());
        save(&path, &settings).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.settings, settings);
        assert_eq!(loaded.notice, None);
        assert!(loaded.writable);
        assert_eq!(
            loaded.settings.tool(Tool::CadPython),
            Some("/opt/cad/bin/python")
        );
        assert_eq!(loaded.settings.tool(Tool::OpenmcPython), None);
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("\"schema\": 1")
        );
    }

    #[test]
    fn a_missing_file_is_an_empty_settings_without_a_notice() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load(&dir.path().join("nothing.json"));
        assert_eq!(loaded.settings, Settings::default());
        assert_eq!(loaded.notice, None);
        assert!(loaded.writable);
    }

    #[test]
    fn a_corrupt_file_is_kept_beside_itself_and_the_user_is_told() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, b"{ not json").unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.settings, Settings::default());
        let notice = loaded.notice.expect("the user is told");
        assert!(notice.contains(".bad"), "{notice}");
        assert!(notice.contains("Next step"), "{notice}");
        assert_eq!(std::fs::read(bad_copy_path(&path)).unwrap(), b"{ not json");
        // A second load of the same bad file still starts empty.
        assert_eq!(load(&path).settings, Settings::default());
    }

    #[test]
    fn a_file_from_a_newer_faris_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, br#"{"schema": 99, "tools": {"cad_python": "/x"}}"#).unwrap();
        let loaded = load(&path);
        assert!(!loaded.writable);
        assert!(loaded.notice.unwrap().contains("newer FARIS"));
        assert_eq!(loaded.settings, Settings::default());
    }

    #[test]
    fn an_older_file_with_missing_fields_still_reads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, br#"{"schema": 1}"#).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.settings, Settings::default());
        assert_eq!(loaded.notice, None);
    }

    #[test]
    fn an_empty_value_removes_the_entry() {
        let mut settings = Settings::default();
        settings.set_tool(Tool::CadPython, "  /a/python  ");
        assert_eq!(settings.tool(Tool::CadPython), Some("/a/python"));
        assert_eq!(settings.raw(Tool::CadPython), "  /a/python  ");
        assert!(settings.has_any_tool());
        settings.set_tool(Tool::CadPython, "   ");
        assert_eq!(settings.tool(Tool::CadPython), None, "blank is not set");
        assert!(!settings.has_any_tool());
        settings.set_tool(Tool::CadPython, "");
        assert!(settings.tools.is_empty());
    }

    #[test]
    fn the_folder_follows_each_platforms_convention() {
        let linux = config_dir_with(
            Platform::Linux,
            &lookup(&[("HOME", "/home/u"), ("XDG_CONFIG_HOME", "/cfg")]),
        );
        assert_eq!(linux, Some(PathBuf::from("/cfg/faris")));
        let linux_default = config_dir_with(Platform::Linux, &lookup(&[("HOME", "/home/u")]));
        assert_eq!(linux_default, Some(PathBuf::from("/home/u/.config/faris")));
        let relative = config_dir_with(
            Platform::Linux,
            &lookup(&[("HOME", "/home/u"), ("XDG_CONFIG_HOME", "relative")]),
        );
        assert_eq!(relative, Some(PathBuf::from("/home/u/.config/faris")));
        let mac = config_dir_with(Platform::MacOs, &lookup(&[("HOME", "/Users/u")]));
        assert_eq!(
            mac,
            Some(PathBuf::from("/Users/u/Library/Application Support/faris"))
        );
        let windows = config_dir_with(Platform::Windows, &lookup(&[("APPDATA", "C:\\A")]));
        assert_eq!(windows, Some(PathBuf::from("C:\\A").join("faris")));
        assert_eq!(config_dir_with(Platform::Linux, &lookup(&[])), None);
    }
}
