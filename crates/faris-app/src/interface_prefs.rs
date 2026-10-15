//! The interface size, kept between sessions (UX-060) as the shared `ui.scale` setting in the
//! user settings file, so `faris config get ui.scale` and the desktop agree.
//!
//! Earlier versions kept the size in a one-line `interface.txt` beside the tour marker. That file
//! is read once: its value moves into the settings file and the old file is removed. Reading never
//! fails the program: a missing or unreadable file means the default size, and a settings file
//! from a newer FARIS is read but not overwritten.

use crate::settings::{self, Settings, UI_SCALE};
use faris_model::settings::Origin;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

/// The name of the retired size file.
pub const OLD_FILE_NAME: &str = "interface.txt";
pub const DEFAULT_SCALE: f32 = 1.0;
/// The range the `--interface-scale` option and the size menu accept.
pub const SCALE_RANGE: std::ops::RangeInclusive<f32> = 0.75..=2.0;

/// The user settings file.
pub fn path() -> Option<PathBuf> {
    settings::default_path()
}

/// The retired size file beside the tour marker.
pub fn old_path() -> Option<PathBuf> {
    crate::tour::marker_path().map(|marker| marker.with_file_name(OLD_FILE_NAME))
}

/// The scale in the old file's text, if it holds a valid one.
pub fn parse_old(text: &str) -> Option<f32> {
    text.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        let scale: f32 = (key.trim() == "interface_scale")
            .then(|| value.trim().parse().ok())
            .flatten()?;
        (scale.is_finite() && SCALE_RANGE.contains(&scale)).then_some(scale)
    })
}

/// The size saved in `file`, if one was set; `None` means the default. An old `interface.txt` at
/// `old` is moved into `file` first, once.
pub fn load(
    file: &Path,
    old: Option<&Path>,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Option<f32> {
    let loaded = settings::load(file);
    let mut settings = loaded.settings;
    if let Some(old) = old
        && let Some(moved) = migrate_old(file, old, &mut settings, loaded.writable)
    {
        eprintln!(
            "FARIS moved the interface size {moved} from {} into {}",
            old.display(),
            file.display()
        );
    }
    let resolved = settings.resolve(file, env);
    let entry = settings::find(&resolved, UI_SCALE)?;
    let scale: f32 = entry.value.as_deref()?.parse().ok()?;
    (!matches!(entry.origin, Origin::Default) && scale.is_finite() && SCALE_RANGE.contains(&scale))
        .then_some(scale)
}

/// Badge text and hover text for where the interface size comes from now (CFG-002).
pub fn origin_badge(file: &Path) -> Option<(&'static str, String)> {
    let settings = settings::load(file).settings;
    let resolved = settings.resolve(file, &|name| std::env::var_os(name));
    let entry = settings::find(&resolved, UI_SCALE)?;
    Some((
        settings::origin_badge(&entry.origin),
        settings::origin_hover(&entry.origin),
    ))
}

/// Moves a size from the old file into the settings (and the settings file), then removes the old
/// file. The settings file's own value wins when it has one. Returns the size moved.
fn migrate_old(file: &Path, old: &Path, settings: &mut Settings, writable: bool) -> Option<f32> {
    let text = std::fs::read_to_string(old).ok()?;
    if settings.value(UI_SCALE).is_some() {
        let _ = std::fs::remove_file(old);
        return None;
    }
    if !writable {
        // The settings file cannot be changed; keep the old file for when it can.
        return None;
    }
    let Some(scale) = parse_old(&text) else {
        // Unusable text means the default; it will not become usable later.
        let _ = std::fs::remove_file(old);
        return None;
    };
    settings.set_value(UI_SCALE, &scale.to_string()).ok()?;
    settings::save(file, settings).ok()?;
    let _ = std::fs::remove_file(old);
    Some(scale)
}

/// Write the scale; a failure only logs, the size still applies for this session. Other sections
/// of the file are kept as they are.
pub fn save(file: &Path, scale: f32) {
    change(file, |settings| {
        settings.set_value(UI_SCALE, &scale.to_string())
    });
}

/// Back to the default size, forgetting the saved one.
pub fn reset(file: &Path) {
    change(file, |settings| {
        settings.unset_value(UI_SCALE);
        Ok(())
    });
}

fn change(file: &Path, edit: impl FnOnce(&mut Settings) -> Result<(), String>) {
    let loaded = settings::load(file);
    if !loaded.writable {
        eprintln!(
            "FARIS did not remember the interface size: {}",
            loaded.notice.unwrap_or_default()
        );
        return;
    }
    let mut settings = loaded.settings;
    let result = edit(&mut settings).and_then(|()| settings::save(file, &settings));
    if let Err(error) = result {
        eprintln!(
            "FARIS could not remember the interface size in {}: {error}",
            file.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<OsString> {
        None
    }

    // Verifies: UX-060
    #[test]
    fn the_size_survives_a_restart_and_reset_forgets_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("faris").join(settings::FILE_NAME);
        assert_eq!(load(&file, None, &no_env), None);
        save(&file, 1.5);
        assert_eq!(load(&file, None, &no_env), Some(1.5));
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(written["values"]["ui.scale"], 1.5);
        reset(&file);
        assert_eq!(load(&file, None, &no_env), None);
        reset(&file);
    }

    // Verifies: UX-060
    #[test]
    fn saving_the_size_keeps_every_other_section() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(settings::FILE_NAME);
        std::fs::write(
            &file,
            r#"{"schema":1,"tools":{"cad_python":"/p"},"last_folders":{"design_flux":"/w"},"extra_search_folders":["/e"],"values":{"memory.cap_mb":4096}}"#,
        )
        .unwrap();
        save(&file, 1.25);
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(written["values"]["ui.scale"], 1.25);
        assert_eq!(written["values"]["memory.cap_mb"], 4096);
        assert_eq!(written["tools"]["cad_python"], "/p");
        assert_eq!(written["last_folders"]["design_flux"], "/w");
        assert_eq!(written["extra_search_folders"][0], "/e");
    }

    // Verifies: UX-060
    #[test]
    fn an_old_interface_file_moves_into_the_settings_once() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(settings::FILE_NAME);
        let old = dir.path().join(OLD_FILE_NAME);
        std::fs::write(&old, "# note\ninterface_scale = 1.75\n").unwrap();
        assert_eq!(load(&file, Some(&old), &no_env), Some(1.75));
        assert!(!old.exists(), "the old file is gone after the move");
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(written["values"]["ui.scale"], 1.75);
        // A later old file does not override the settings file's own value.
        std::fs::write(&old, "interface_scale = 1.0\n").unwrap();
        assert_eq!(load(&file, Some(&old), &no_env), Some(1.75));
        assert!(!old.exists());
    }

    #[test]
    fn a_settings_file_from_a_newer_faris_is_not_overwritten_by_the_move() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(settings::FILE_NAME);
        let old = dir.path().join(OLD_FILE_NAME);
        let newer = br#"{"schema": 99, "tools": {}}"#;
        std::fs::write(&file, newer).unwrap();
        std::fs::write(&old, "interface_scale = 1.5\n").unwrap();
        assert_eq!(load(&file, Some(&old), &no_env), None);
        assert_eq!(std::fs::read(&file).unwrap(), newer);
        assert!(old.exists(), "kept until the settings file can take it");
        save(&file, 1.5);
        assert_eq!(std::fs::read(&file).unwrap(), newer);
    }

    #[test]
    fn unusable_old_text_means_the_default() {
        for text in [
            "",
            "interface_scale = x",
            "interface_scale = 9",
            "interface_scale = NaN",
            "other = 1.25",
        ] {
            assert_eq!(parse_old(text), None, "{text}");
        }
        assert_eq!(parse_old("# note\ninterface_scale=0.75"), Some(0.75));
    }
}
