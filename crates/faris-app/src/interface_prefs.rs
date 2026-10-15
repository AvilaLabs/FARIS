//! The interface size, kept between sessions (UX-060).
//!
//! A minimal per-user text file, `interface.txt`, beside the tour marker:
//! one line, `interface_scale = 1.25`. It stands alone on purpose; the wider
//! settings layer is built separately in faris-model and will replace this.
//! Reading never fails the program: a missing or unreadable file means the
//! default size.

use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "interface.txt";
pub const DEFAULT_SCALE: f32 = 1.0;
/// The range the `--interface-scale` option accepts.
pub const SCALE_RANGE: std::ops::RangeInclusive<f32> = 0.75..=2.0;

/// The file beside the tour marker in the per-user configuration folder.
pub fn path() -> Option<PathBuf> {
    crate::tour::marker_path().map(|marker| marker.with_file_name(FILE_NAME))
}

/// The scale in the file's text, if it holds a valid one.
pub fn parse(text: &str) -> Option<f32> {
    text.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        let scale: f32 = (key.trim() == "interface_scale")
            .then(|| value.trim().parse().ok())
            .flatten()?;
        (scale.is_finite() && SCALE_RANGE.contains(&scale)).then_some(scale)
    })
}

pub fn render(scale: f32) -> String {
    format!(
        "# FARIS interface size, 0.75 to 2. Delete this file to return to the default.\ninterface_scale = {scale}\n"
    )
}

pub fn load(path: &Path) -> Option<f32> {
    parse(&std::fs::read_to_string(path).ok()?)
}

/// Write the scale; a failure only logs, the size still applies for this session.
pub fn save(path: &Path, scale: f32) {
    let result = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(path, render(scale)));
    if let Err(error) = result {
        eprintln!(
            "FARIS could not remember the interface size in {}: {error}",
            path.display()
        );
    }
}

/// Back to the default size, forgetting the saved one.
pub fn reset(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => eprintln!("FARIS could not remove {}: {error}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: UX-060
    #[test]
    fn the_size_survives_a_restart_and_reset_forgets_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("faris").join(FILE_NAME);
        assert_eq!(load(&file), None);
        save(&file, 1.5);
        assert_eq!(load(&file), Some(1.5));
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("interface_scale = 1.5"));
        reset(&file);
        assert_eq!(load(&file), None);
        reset(&file);
    }

    #[test]
    fn unusable_text_means_the_default() {
        for text in [
            "",
            "interface_scale = x",
            "interface_scale = 9",
            "interface_scale = NaN",
            "other = 1.25",
        ] {
            assert_eq!(parse(text), None, "{text}");
        }
        assert_eq!(parse("# note\ninterface_scale=0.75"), Some(0.75));
    }
}
