// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
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
/// directories cannot be opened for syncing (Windows). Unix errors propagate:
/// a rename whose directory entry was not synced is not reported as durable.
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
    write_with_ops(path, existing, bytes, observe, &mut SystemOps)
}

/// Operations used by the writer state machine. The test backend models volatile and durable
/// directory state while exercising this same sequence as the production filesystem backend.
trait AtomicFileOps {
    type Temporary;

    fn create_temporary(&mut self, directory: &Path) -> io::Result<Self::Temporary>;
    fn write_data(&mut self, temporary: &mut Self::Temporary, bytes: &[u8]) -> io::Result<()>;
    fn sync_file(&mut self, temporary: &mut Self::Temporary) -> io::Result<()>;
    fn rename(
        &mut self,
        temporary: Self::Temporary,
        path: &Path,
        existing: Existing,
    ) -> io::Result<()>;
    fn sync_directory(&mut self, directory: &Path) -> io::Result<()>;
}

struct SystemOps;

impl AtomicFileOps for SystemOps {
    type Temporary = tempfile::NamedTempFile;

    fn create_temporary(&mut self, directory: &Path) -> io::Result<Self::Temporary> {
        tempfile::Builder::new()
            .prefix(".faris-save-")
            .suffix(".tmp")
            .tempfile_in(directory)
    }

    fn write_data(&mut self, temporary: &mut Self::Temporary, bytes: &[u8]) -> io::Result<()> {
        temporary.write_all(bytes)
    }

    fn sync_file(&mut self, temporary: &mut Self::Temporary) -> io::Result<()> {
        temporary.as_file().sync_all()
    }

    fn rename(
        &mut self,
        temporary: Self::Temporary,
        path: &Path,
        existing: Existing,
    ) -> io::Result<()> {
        match existing {
            Existing::Replace => temporary.persist(path).map(|_| ()).map_err(|e| e.error),
            Existing::Refuse => temporary
                .persist_noclobber(path)
                .map(|_| ())
                .map_err(|e| e.error),
        }
    }

    fn sync_directory(&mut self, directory: &Path) -> io::Result<()> {
        sync_directory(directory)
    }
}

fn write_with_ops<O: AtomicFileOps>(
    path: &Path,
    existing: Existing,
    bytes: &[u8],
    observe: &mut dyn FnMut(Step),
    ops: &mut O,
) -> io::Result<()> {
    let directory = parent_directory(path);
    let mut temporary = ops.create_temporary(directory)?;
    observe(Step::CreateTemporary);
    ops.write_data(&mut temporary, bytes)?;
    observe(Step::WriteData);
    ops.sync_file(&mut temporary)?;
    observe(Step::FsyncFile);
    commit_with_ops(temporary, path, existing, observe, ops)
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
    commit_with_ops(temporary, path, existing, observe, &mut SystemOps)
}

fn commit_with_ops<O: AtomicFileOps>(
    temporary: O::Temporary,
    path: &Path,
    existing: Existing,
    observe: &mut dyn FnMut(Step),
    ops: &mut O,
) -> io::Result<()> {
    ops.rename(temporary, path, existing)?;
    observe(Step::Rename);
    ops.sync_directory(parent_directory(path))?;
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

    /// Check that the recorded writer sequence cannot persist a rename without the data.
    /// The sequence comes from the real production filesystem backend; persistence choices
    /// below are the crash model, not a test of an actual block device.
    fn crash_states_are_safe(steps: &[Step]) -> Result<usize, String> {
        let mut checked = 0;
        for prefix in 0..=steps.len() {
            let done = &steps[..prefix];
            let has = |step: Step| done.contains(&step);
            let data_durable = has(Step::FsyncFile);
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
                            "crash after {done:?}: rename may outlive file data"
                        ));
                    }
                }
            }
        }
        Ok(checked)
    }

    // Verifies: REL-010
    #[test]
    fn production_filesystem_writer_reports_its_operation_order() {
        let mut steps = Vec::new();
        let dir = tempfile::tempdir().unwrap();
        write_with(
            &dir.path().join("a.json"),
            Existing::Replace,
            b"complete payload",
            &mut |step| steps.push(step),
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
        assert!(crash_states_are_safe(&steps).unwrap() >= 6);
    }

    /// Volatile/durable filesystem state for testing crash/reopen outcomes through the
    /// production writer state machine. Data and directory entries become durable only at
    /// their respective sync operations; a crash may retain or drop an unsynced rename.
    #[derive(Clone)]
    struct ModelSnapshot {
        temporary: Option<Vec<u8>>,
        durable_temporary: Option<Vec<u8>>,
        target: Vec<u8>,
        rename_pending: bool,
        directory_synced: bool,
    }

    struct ModelOps {
        snapshot: ModelSnapshot,
        history: Vec<ModelSnapshot>,
        fail_directory_sync: bool,
        lose_file_sync: bool,
    }

    impl ModelOps {
        fn new(previous: &[u8]) -> Self {
            Self {
                snapshot: ModelSnapshot {
                    temporary: None,
                    durable_temporary: None,
                    target: previous.to_vec(),
                    rename_pending: false,
                    directory_synced: false,
                },
                history: Vec::new(),
                fail_directory_sync: false,
                lose_file_sync: false,
            }
        }

        fn record(&mut self) {
            self.history.push(self.snapshot.clone());
        }

        /// Reopening after a crash returns only durable names. Before directory sync, the
        /// rename may have reached disk or may have been lost; both outcomes are represented.
        fn crash_reopens(&self, state: &ModelSnapshot, previous: &[u8]) -> Vec<Vec<u8>> {
            if state.directory_synced {
                vec![state.target.clone()]
            } else if state.rename_pending {
                // An invalid writer order or lost file barrier can leave a surviving
                // rename pointing at absent or incomplete data. Model that as corruption.
                let new = state
                    .durable_temporary
                    .clone()
                    .unwrap_or_else(|| b"<partial-or-missing-data>".to_vec());
                vec![previous.to_vec(), new]
            } else {
                vec![previous.to_vec()]
            }
        }
    }

    impl AtomicFileOps for ModelOps {
        type Temporary = ();

        fn create_temporary(&mut self, _directory: &Path) -> io::Result<Self::Temporary> {
            self.snapshot.temporary = Some(Vec::new());
            self.record();
            Ok(())
        }

        fn write_data(&mut self, _temporary: &mut Self::Temporary, bytes: &[u8]) -> io::Result<()> {
            self.snapshot.temporary = Some(bytes.to_vec());
            self.record();
            Ok(())
        }

        fn sync_file(&mut self, _temporary: &mut Self::Temporary) -> io::Result<()> {
            if !self.lose_file_sync {
                self.snapshot.durable_temporary = self.snapshot.temporary.clone();
            }
            self.record();
            Ok(())
        }

        fn rename(
            &mut self,
            _temporary: Self::Temporary,
            _path: &Path,
            _existing: Existing,
        ) -> io::Result<()> {
            self.snapshot.target = self
                .snapshot
                .temporary
                .take()
                .expect("temporary is created before rename");
            self.snapshot.rename_pending = true;
            self.record();
            Ok(())
        }

        fn sync_directory(&mut self, _directory: &Path) -> io::Result<()> {
            if self.fail_directory_sync {
                return Err(io::Error::other("injected directory sync failure"));
            }
            self.snapshot.directory_synced = true;
            self.record();
            Ok(())
        }
    }

    // Verifies: REL-010
    #[test]
    fn writer_state_machine_crash_reopen_model_keeps_old_or_complete_new_data() {
        let previous = b"previous complete version";
        let next = b"new complete version";
        let dir = tempfile::tempdir().unwrap();
        let mut model = ModelOps::new(previous);
        write_with_ops(
            &dir.path().join("a.json"),
            Existing::Replace,
            next,
            &mut |_| {},
            &mut model,
        )
        .unwrap();

        assert_eq!(model.history.len(), 5);
        for state in &model.history {
            for reopened in model.crash_reopens(state, previous) {
                assert!(
                    reopened == previous || reopened == next,
                    "crash/reopen exposed partial data: {reopened:?}"
                );
            }
        }
        assert_eq!(
            model.crash_reopens(&model.snapshot, previous),
            vec![next.to_vec()]
        );
    }

    // Verifies: REL-010 (model sensitivity to missing file-data durability)
    #[test]
    fn crash_model_flags_rename_when_file_sync_did_not_make_data_durable() {
        let previous = b"previous complete version";
        let next = b"new complete version";
        let mut model = ModelOps::new(previous);
        model.lose_file_sync = true;
        write_with_ops(
            Path::new("a.json"),
            Existing::Replace,
            next,
            &mut |_| {},
            &mut model,
        )
        .unwrap();

        let after_rename = &model.history[3];
        let reopened = model.crash_reopens(after_rename, previous);
        assert!(
            reopened
                .iter()
                .any(|value| value != previous && value != next),
            "the model did not expose the invalid rename's missing/partial data"
        );
    }

    // Verifies: REL-010
    #[test]
    fn directory_sync_failure_is_reported_and_not_observed_as_a_barrier() {
        let mut model = ModelOps::new(b"previous");
        model.fail_directory_sync = true;
        let mut steps = Vec::new();
        let error = write_with_ops(
            Path::new("a.json"),
            Existing::Replace,
            b"next",
            &mut |step| steps.push(step),
            &mut model,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(
            steps,
            vec![
                Step::CreateTemporary,
                Step::WriteData,
                Step::FsyncFile,
                Step::Rename
            ]
        );
        assert!(!model.snapshot.directory_synced);
        assert_eq!(
            model.crash_reopens(&model.snapshot, b"previous"),
            vec![b"previous".to_vec(), b"next".to_vec()]
        );
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
