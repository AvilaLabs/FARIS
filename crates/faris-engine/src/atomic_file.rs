//! One way to write a file a user will keep: a temporary file in the same
//! directory, written and fsynced, renamed over the destination, then the
//! directory fsynced so the rename itself survives a power cut. A reader sees
//! the previous complete file or the new complete file, never a mixture.
//!
//! A process killed mid-save can leave its `.faris-save-*.tmp` file behind;
//! the destination is untouched in that case.

use std::{
    fs::File,
    io::{self, Write},
    path::Path,
};

pub mod campaign;

/// One durable-state step of a save, in the order it happens. Tests record
/// these to check the ordering; production passes a no-op observer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    CreateTemporary,
    WriteData,
    FsyncFile,
    Rename,
    FsyncDirectory,
}

/// What to do when the destination already exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Existing {
    /// Replace it atomically (saving a study again).
    Replace,
    /// Fail and leave it alone (a report is never overwritten).
    Refuse,
}

/// The directory a file will be created in: its parent, or `.` for a bare name.
pub fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

/// Flush the directory entry so a completed rename is durable. A no-op where
/// directories cannot be opened for syncing (Windows); the result is ignored
/// by callers that already committed the rename.
pub fn sync_directory(directory: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(directory)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = directory;
        Ok(())
    }
}

/// Write `bytes` to `path` atomically, replacing any existing file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_with(path, Existing::Replace, bytes, &mut |_| {})
}

/// Write `bytes` to a new file at `path` atomically; fails with
/// `AlreadyExists` and changes nothing if `path` exists.
pub fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_with(path, Existing::Refuse, bytes, &mut |_| {})
}

/// [`write_atomic`] / [`write_new`] with an observer that sees each step.
pub fn write_with(
    path: &Path,
    existing: Existing,
    bytes: &[u8],
    observe: &mut dyn FnMut(Step),
) -> io::Result<()> {
    let directory = parent_directory(path);
    let mut temporary = tempfile::Builder::new()
        .prefix(".faris-save-")
        .suffix(".tmp")
        .tempfile_in(directory)?;
    observe(Step::CreateTemporary);
    temporary.write_all(bytes)?;
    observe(Step::WriteData);
    temporary.as_file().sync_all()?;
    observe(Step::FsyncFile);
    commit_with(temporary, path, existing, observe)
}

/// Rename an already written and fsynced temporary file onto `path`, then
/// fsync the directory. Used by writers that stream into the temporary file.
pub fn commit(
    temporary: tempfile::NamedTempFile,
    path: &Path,
    existing: Existing,
) -> io::Result<()> {
    commit_with(temporary, path, existing, &mut |_| {})
}

fn commit_with(
    temporary: tempfile::NamedTempFile,
    path: &Path,
    existing: Existing,
    observe: &mut dyn FnMut(Step),
) -> io::Result<()> {
    match existing {
        Existing::Replace => temporary.persist(path).map(|_| ()).map_err(|e| e.error)?,
        Existing::Refuse => temporary
            .persist_noclobber(path)
            .map(|_| ())
            .map_err(|e| e.error)?,
    }
    observe(Step::Rename);
    // The rename is done; a failure to sync the directory only weakens
    // durability, so it is not reported as a failed save.
    let _ = sync_directory(parent_directory(path));
    observe(Step::FsyncDirectory);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: REL-010
    #[test]
    fn replace_and_refuse_leave_no_temporary_files() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("report.json");
        write_new(&target, b"one").unwrap();
        let error = write_new(&target, b"two").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&target).unwrap(), b"one");
        write_atomic(&target, b"three").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"three");
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("report.json")]);
    }

    // Verifies: REL-010
    #[test]
    fn a_failed_save_leaves_the_previous_file() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nowhere").join("a.json");
        assert!(write_atomic(&missing, b"x").is_err());
        assert!(!missing.exists());
    }

    /// A model of a filesystem that may lose anything not yet fsynced: after
    /// any prefix of the recorded steps, every unsynced step may or may not
    /// have reached the disk, in any combination. The destination must then be
    /// the previous file or the complete new file.
    fn crash_states_are_safe(steps: &[Step]) -> Result<usize, String> {
        let mut checked = 0;
        for prefix in 0..=steps.len() {
            let done = &steps[..prefix];
            // Steps at or before the last barrier that covers them are durable.
            let has = |s: Step| done.contains(&s);
            let data_durable = has(Step::FsyncFile);
            // Unsynced steps: data (if no fsync yet) and the rename (if no
            // directory fsync yet) may each be lost independently.
            for rename_persisted in [false, true] {
                if rename_persisted && !has(Step::Rename) {
                    continue;
                }
                if !rename_persisted && has(Step::FsyncDirectory) {
                    continue;
                }
                for data_persisted in [false, true] {
                    if data_persisted && !has(Step::WriteData) {
                        continue;
                    }
                    if !data_persisted && data_durable {
                        continue;
                    }
                    checked += 1;
                    if rename_persisted && !data_persisted && !data_durable {
                        return Err(format!(
                            "crash after {done:?}: the new name can survive without its data"
                        ));
                    }
                }
            }
        }
        Ok(checked)
    }

    // Verifies: REL-010
    #[test]
    fn modelled_power_loss_states_never_expose_partial_data() {
        let mut steps = Vec::new();
        let dir = tempfile::tempdir().unwrap();
        write_with(
            &dir.path().join("a.json"),
            Existing::Replace,
            b"{}",
            &mut |s| steps.push(s),
        )
        .unwrap();
        assert_eq!(
            steps,
            vec![
                Step::CreateTemporary,
                Step::WriteData,
                Step::FsyncFile,
                Step::Rename,
                Step::FsyncDirectory
            ]
        );
        let states = crash_states_are_safe(&steps).unwrap();
        assert!(states >= 6, "model explored only {states} states");
    }

    // Verifies: REL-010
    #[test]
    fn the_power_loss_model_catches_a_rename_before_fsync() {
        let wrong = [
            Step::CreateTemporary,
            Step::WriteData,
            Step::Rename,
            Step::FsyncFile,
            Step::FsyncDirectory,
        ];
        assert!(crash_states_are_safe(&wrong).is_err());
    }
}
