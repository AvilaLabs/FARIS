//! Rolling backups of earlier saved versions, and the rule that an upgraded
//! study is a new file (REL-016, REL-025).
//!
//! Before a save replaces `plant.faris`, the file that is there is copied into
//! `plant.faris.backups/` beside it, byte for byte, so a restored backup has the
//! hash the file had when it was saved. The newest few are kept (`backups.keep`,
//! 3 unless changed). Nothing here ever edits a study file in place: a backup is
//! a copy, a restore copies a backup over the study through a temporary file
//! and a rename, and a study written by an older format is saved under a new name.

use faris_engine::atomic_file::{self, Existing};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

/// Backups kept when the setting is not changed.
pub const DEFAULT_KEEP: usize = 3;
const EXTENSION: &str = "faris";

/// The folder holding the backups of `study`: `<name>.backups` beside it.
pub fn dir_for(study: &Path) -> PathBuf {
    let mut name = study
        .file_name()
        .map_or_else(|| "study".into(), |n| n.to_os_string());
    name.push(".backups");
    atomic_file::parent_directory(study).join(name)
}

/// One kept earlier version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backup {
    pub path: PathBuf,
    /// When it was taken, in unix seconds.
    pub unix: u64,
    pub bytes: u64,
}

/// `000001760000-0.faris`: the time first, so names sort by age.
fn parse_name(name: &str) -> Option<u64> {
    let stem = name.strip_suffix(&format!(".{EXTENSION}"))?;
    let (time, counter) = stem.split_once('-')?;
    counter.parse::<u32>().ok()?;
    time.parse().ok()
}

/// The kept versions of `study`, newest first.
pub fn list(study: &Path) -> Vec<Backup> {
    let Ok(entries) = fs::read_dir(dir_for(study)) else {
        return Vec::new();
    };
    let mut found: Vec<(Backup, String)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let unix = parse_name(&name)?;
            let bytes = entry.metadata().ok()?.len();
            Some((
                Backup {
                    path: entry.path(),
                    unix,
                    bytes,
                },
                name,
            ))
        })
        .collect();
    found.sort_by(|a, b| b.0.unix.cmp(&a.0.unix).then_with(|| b.1.cmp(&a.1)));
    found.into_iter().map(|(backup, _)| backup).collect()
}

fn next_name(directory: &Path, unix: u64) -> String {
    (0u32..)
        .map(|n| format!("{unix:012}-{n}.{EXTENSION}"))
        .find(|name| !directory.join(name).exists())
        .expect("an unused name exists")
}

/// Copy the file at `study`, if there is one, into its backup folder, then keep
/// only the newest `keep`. Returns the backup made.
pub fn rotate(study: &Path, keep: usize, now_unix: u64) -> io::Result<Option<PathBuf>> {
    if !study.is_file() {
        return Ok(None);
    }
    let directory = dir_for(study);
    fs::create_dir_all(&directory)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".faris-backup-")
        .suffix(".tmp")
        .tempfile_in(&directory)?;
    io::copy(&mut fs::File::open(study)?, &mut temporary)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    let made = directory.join(next_name(&directory, now_unix));
    atomic_file::commit(temporary, &made, Existing::Refuse)?;
    prune(study, keep.max(1));
    Ok(Some(made))
}

fn prune(study: &Path, keep: usize) {
    for old in list(study).into_iter().skip(keep) {
        let _ = fs::remove_file(old.path);
    }
}

/// Replace `study` with the contents of `backup`. The version being replaced is
/// itself backed up first, so a restore can be taken back.
pub fn restore(study: &Path, backup: &Path, keep: usize, now_unix: u64) -> io::Result<()> {
    let mut temporary = tempfile::Builder::new()
        .prefix(".faris-restore-")
        .suffix(".tmp")
        .tempfile_in(atomic_file::parent_directory(study))?;
    io::copy(&mut fs::File::open(backup)?, &mut temporary)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    rotate(study, keep, now_unix)?;
    atomic_file::commit(temporary, study, Existing::Replace)
}

/// True when a file written as `file_format` must be rewritten to be read as
/// `current_format`: the file is of an older version of the format. A newer
/// minor version is not an upgrade: saving it keeps what this version cannot read.
pub fn needs_upgrade(file_format: &str, current_format: &str) -> bool {
    fn version(format: &str) -> Option<(u32, u32)> {
        let text = format.strip_prefix("faris-study/")?;
        let mut parts = text.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next().map_or(Some(0), |m| m.parse().ok())?;
        Some((major, minor))
    }
    match (version(file_format), version(current_format)) {
        (Some(file), Some(current)) => file < current,
        _ => false,
    }
}

/// `plant.faris` becomes `plant (upgraded).faris`, then `plant (upgraded 2).faris`, and so
/// on: the first name that does not exist.
pub fn upgraded_path(original: &Path) -> PathBuf {
    let directory = atomic_file::parent_directory(original);
    let stem = original
        .file_stem()
        .map_or_else(|| "study".into(), |s| s.to_string_lossy().into_owned());
    (1u32..)
        .map(|n| {
            let tag = if n == 1 {
                "upgraded".to_owned()
            } else {
                format!("upgraded {n}")
            };
            directory.join(format!("{stem} ({tag}).{EXTENSION}"))
        })
        .find(|candidate| !candidate.exists())
        .expect("an unused name exists")
}

/// Where a save of an opened study goes. A study opened from an older format is never
/// written over: the second value is true and the path is a new one beside it.
pub fn save_destination(
    requested: &Path,
    opened_format: Option<&str>,
    current_format: &str,
) -> (PathBuf, bool) {
    match opened_format {
        Some(format) if needs_upgrade(format, current_format) && requested.exists() => {
            (upgraded_path(requested), true)
        }
        _ => (requested.to_path_buf(), false),
    }
}

/// "3 min ago" style age of a backup for the restore menu.
pub fn age_text(now_unix: u64, then_unix: u64) -> String {
    let s = now_unix.saturating_sub(then_unix);
    match s {
        0..=59 => format!("{s} s ago"),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / faris_model::units::DAY_S as u64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faris_study::sha256_hex;

    fn write(path: &Path, text: &str) {
        fs::write(path, text).unwrap();
    }

    fn hash(path: &Path) -> String {
        sha256_hex(&fs::read(path).unwrap())
    }

    // Verifies: REL-016
    #[test]
    fn saving_keeps_the_newest_versions_and_a_restore_has_the_old_hash() {
        let dir = tempfile::tempdir().unwrap();
        let study = dir.path().join("plant.faris");
        let mut hashes = Vec::new();
        for version in 0..6u64 {
            // The save: back up what is there, then replace it.
            rotate(&study, DEFAULT_KEEP, 1_000 + version).unwrap();
            write(&study, &format!("version {version}"));
            hashes.push(hash(&study));
        }
        let kept = list(&study);
        assert_eq!(kept.len(), 3, "three versions are kept by default");
        // Newest first: the files saved as versions 4, 3 and 2 (version 5 is the study).
        let kept_hashes: Vec<_> = kept.iter().map(|b| hash(&b.path)).collect();
        assert_eq!(
            kept_hashes,
            vec![hashes[4].clone(), hashes[3].clone(), hashes[2].clone()]
        );
        assert!(kept[0].unix > kept[2].unix);
        // Restoring the oldest kept version reproduces its hash.
        restore(&study, &kept[2].path, DEFAULT_KEEP, 2_000).unwrap();
        assert_eq!(hash(&study), hashes[2]);
        // The version that was replaced is itself kept, so the restore can be undone.
        assert!(list(&study).iter().any(|b| hash(&b.path) == hashes[5]));
        assert_eq!(list(&study).len(), 3);
    }

    // Verifies: REL-016
    #[test]
    fn the_number_kept_is_configurable_and_a_first_save_makes_no_backup() {
        let dir = tempfile::tempdir().unwrap();
        let study = dir.path().join("a.faris");
        assert_eq!(
            rotate(&study, 5, 10).unwrap(),
            None,
            "nothing to back up yet"
        );
        assert!(list(&study).is_empty());
        for version in 0..9u64 {
            write(&study, &format!("v{version}"));
            rotate(&study, 5, 100 + version).unwrap();
        }
        assert_eq!(list(&study).len(), 5);
        rotate(&study, 2, 500).unwrap();
        assert_eq!(
            list(&study).len(),
            2,
            "lowering the setting prunes at the next save"
        );
        // Two saves in the same second do not overwrite each other.
        write(&study, "x");
        let a = rotate(&study, 9, 700).unwrap().unwrap();
        let b = rotate(&study, 9, 700).unwrap().unwrap();
        assert_ne!(a, b);
    }

    // Verifies: REL-025
    #[test]
    fn a_study_from_an_older_format_is_saved_as_a_new_file_and_the_original_is_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("old.faris");
        write(&original, "bytes written by an older FARIS");
        let before = hash(&original);
        assert!(needs_upgrade("faris-study/1.0", "faris-study/1.2"));
        assert!(needs_upgrade("faris-study/1", "faris-study/1.1"));
        assert!(!needs_upgrade("faris-study/1.2", "faris-study/1.2"));
        assert!(!needs_upgrade("faris-study/1.3", "faris-study/1.2"));
        assert!(!needs_upgrade("something else", "faris-study/1"));
        let (destination, upgraded) =
            save_destination(&original, Some("faris-study/1.0"), "faris-study/1.2");
        assert!(upgraded);
        assert_ne!(destination, original);
        assert_eq!(destination.file_name().unwrap(), "old (upgraded).faris");
        // The write goes to the new name; the original keeps its bytes.
        atomic_file::write_new(&destination, b"the same study in the new format").unwrap();
        assert_eq!(hash(&original), before);
        // A second upgrade does not overwrite the first.
        let (second, _) = save_destination(&original, Some("faris-study/1.0"), "faris-study/1.2");
        assert_eq!(second.file_name().unwrap(), "old (upgraded 2).faris");
        // A current file is saved in place, as before.
        let (same, upgraded) =
            save_destination(&original, Some("faris-study/1.2"), "faris-study/1.2");
        assert!(!upgraded && same == original);
        // Saving to a path that does not exist yet is already a new file.
        let fresh = dir.path().join("fresh.faris");
        assert_eq!(
            save_destination(&fresh, Some("faris-study/1.0"), "faris-study/1.2"),
            (fresh, false)
        );
    }

    #[test]
    fn ages_read_plainly() {
        assert_eq!(age_text(100, 70), "30 s ago");
        assert_eq!(age_text(1000, 100), "15 min ago");
        assert_eq!(age_text(100_000, 0), "1 days ago");
    }
}
