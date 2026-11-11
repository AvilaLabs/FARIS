// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! One way to write a file a user will keep: a temporary file in the same
//! directory, written and fsynced, renamed over the destination, then the
//! directory synced where that operation is supported. On Windows the directory
//! sync remains a no-op and is explicitly not treated as durability evidence.
//!
//! A process killed mid-save can leave its `.faris-save-*.tmp` file behind;
//! the destination is untouched in that case.

use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
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
    CreateRecoveryCopy,
    WriteRecoveryCopy,
    FsyncRecoveryCopy,
    PersistRecoveryCopy,
    FsyncRecoveryDirectory,
    Rename,
    FsyncDirectory,
    CreateRollbackCopy,
    WriteRollbackCopy,
    FsyncRollbackCopy,
    RollbackRename,
    FsyncRollbackDirectory,
    RemoveUncommittedTarget,
    DirectorySyncUnsupported,
    RecoveryCleanupPending,
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

struct DestinationLock {
    #[cfg(unix)]
    _file: File,
}

/// Lock the parent directory inode on Unix. Writers through symlinked parent aliases
/// therefore share the same lock, and no persistent lock file is created.
#[cfg(unix)]
fn acquire_destination_lock(path: &Path) -> io::Result<DestinationLock> {
    let file = File::open(parent_directory(path))?;
    file.lock()?;
    Ok(DestinationLock { _file: file })
}

#[cfg(not(unix))]
fn acquire_destination_lock(_path: &Path) -> io::Result<DestinationLock> {
    Ok(DestinationLock {})
}

/// Short prefix for recovery entries owned by saves of this destination.
/// Hashing the basename avoids exceeding filesystem component limits.
fn recovery_prefix(path: &Path) -> io::Result<String> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "atomic save path has no file name",
        )
    })?;
    let mut digest = Sha256::new();
    digest.update(name.as_encoded_bytes());
    Ok(format!(".faris-previous-{:x}-", digest.finalize()))
}

static RECOVERY_ID: AtomicU64 = AtomicU64::new(0);

/// Generate a per-save recovery name. Installation always uses no-clobber rename;
/// an existing entry is never interpreted as ours or removed.
fn recovery_path(path: &Path) -> io::Result<std::path::PathBuf> {
    let prefix = recovery_prefix(path)?;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let id = RECOVERY_ID.fetch_add(1, Ordering::Relaxed);
    Ok(parent_directory(path).join(format!("{prefix}{}-{timestamp}-{id}", std::process::id())))
}

/// List previous-version copies created by saves of this destination.
/// This is an inventory only; callers should verify a candidate before restoring it.
pub fn recovery_files(path: &Path) -> io::Result<Vec<std::path::PathBuf>> {
    let prefix = recovery_prefix(path)?;
    let mut found = Vec::new();
    for entry in std::fs::read_dir(parent_directory(path))? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            found.push(entry.path());
        }
    }
    found.sort();
    Ok(found)
}

/// Flush the directory entry so a completed rename is durable. On Windows this remains a
/// no-op, is explicitly recorded as unsupported, and is not treated as durability evidence.
/// Existing-target replacement keeps its legacy same-directory atomic-rename behavior there,
/// without the Unix backup/rollback durability claim. Unix errors propagate.
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
    type Lock;

    fn lock_destination(&mut self, path: &Path) -> io::Result<Self::Lock>;
    fn reject_symlink_destination(&mut self, path: &Path, existing: Existing) -> io::Result<()>;
    fn validate_destination(&mut self, path: &Path, existing: Existing) -> io::Result<()>;
    fn directory_sync_is_durable(&self) -> bool;
    fn create_temporary(&mut self, directory: &Path) -> io::Result<Self::Temporary>;
    fn write_data(&mut self, temporary: &mut Self::Temporary, bytes: &[u8]) -> io::Result<()>;
    fn copy_file(&mut self, source: &Path, temporary: &mut Self::Temporary) -> io::Result<()>;
    fn same_contents(&mut self, left: &Path, right: &Path) -> io::Result<bool>;
    fn sync_file(&mut self, temporary: &mut Self::Temporary) -> io::Result<()>;
    fn rename(
        &mut self,
        temporary: Self::Temporary,
        path: &Path,
        existing: Existing,
    ) -> io::Result<()>;
    fn exists(&mut self, path: &Path) -> io::Result<bool>;
    fn remove_file(&mut self, path: &Path) -> io::Result<()>;
    fn sync_directory(&mut self, directory: &Path) -> io::Result<()>;
}

struct SystemOps;

impl AtomicFileOps for SystemOps {
    type Temporary = tempfile::NamedTempFile;
    type Lock = DestinationLock;

    fn lock_destination(&mut self, path: &Path) -> io::Result<Self::Lock> {
        acquire_destination_lock(path)
    }

    fn validate_destination(&mut self, path: &Path, existing: Existing) -> io::Result<()> {
        match std::fs::symlink_metadata(path) {
            Ok(_) if existing == Existing::Refuse => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("destination already exists: {}", path.display()),
            )),
            Ok(metadata)
                if metadata.file_type().is_symlink()
                    && existing == Existing::Replace
                    && self.directory_sync_is_durable() =>
            {
                Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "atomic replacement refuses symlink destinations: {}",
                        path.display()
                    ),
                ))
            }
            Ok(metadata) if metadata.file_type().is_symlink() && existing == Existing::Replace => {
                Ok(())
            }
            Ok(metadata) if metadata.file_type().is_file() => Ok(()),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "atomic replacement only supports regular files: {}",
                    path.display()
                ),
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn reject_symlink_destination(&mut self, path: &Path, existing: Existing) -> io::Result<()> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata)
                if metadata.file_type().is_symlink()
                    && existing == Existing::Replace
                    && self.directory_sync_is_durable() =>
            {
                Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "atomic replacement refuses symlink destinations: {}",
                        path.display()
                    ),
                ))
            }
            Ok(metadata) if metadata.file_type().is_symlink() && existing == Existing::Refuse => {
                Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("destination already exists: {}", path.display()),
                ))
            }
            Ok(_) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn directory_sync_is_durable(&self) -> bool {
        cfg!(unix)
    }

    fn create_temporary(&mut self, directory: &Path) -> io::Result<Self::Temporary> {
        tempfile::Builder::new()
            .prefix(".faris-save-")
            .suffix(".tmp")
            .tempfile_in(directory)
    }

    fn write_data(&mut self, temporary: &mut Self::Temporary, bytes: &[u8]) -> io::Result<()> {
        temporary.write_all(bytes)
    }

    fn copy_file(&mut self, source: &Path, temporary: &mut Self::Temporary) -> io::Result<()> {
        let mut source = File::open(source)?;
        let permissions = source.metadata()?.permissions();
        temporary.as_file().set_permissions(permissions)?;
        io::copy(&mut source, temporary.as_file_mut())?;
        Ok(())
    }

    fn same_contents(&mut self, left: &Path, right: &Path) -> io::Result<bool> {
        use std::io::Read;
        let mut left = File::open(left)?;
        let mut right = File::open(right)?;
        let mut left_buf = [0; 64 * 1024];
        let mut right_buf = [0; 64 * 1024];
        loop {
            let left_count = left.read(&mut left_buf)?;
            let right_count = right.read(&mut right_buf)?;
            if left_count != right_count || left_buf[..left_count] != right_buf[..right_count] {
                return Ok(false);
            }
            if left_count == 0 {
                return Ok(true);
            }
        }
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

    fn exists(&mut self, path: &Path) -> io::Result<bool> {
        match std::fs::symlink_metadata(path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn remove_file(&mut self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
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
    // The first validation is read-only: reject a symlink destination without creating a
    // lock/temp artifact. Revalidate after locking to close the writer/writer race.
    ops.reject_symlink_destination(path, existing)?;
    let _destination_lock = ops.lock_destination(path)?;
    ops.validate_destination(path, existing)?;
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
    let mut ops = SystemOps;
    ops.reject_symlink_destination(path, existing)?;
    let _destination_lock = ops.lock_destination(path)?;
    ops.validate_destination(path, existing)?;
    commit_with_ops(temporary, path, existing, observe, &mut SystemOps)
}

fn commit_with_ops<O: AtomicFileOps>(
    temporary: O::Temporary,
    path: &Path,
    existing: Existing,
    observe: &mut dyn FnMut(Step),
    ops: &mut O,
) -> io::Result<()> {
    let directory = parent_directory(path);
    let has_previous = existing == Existing::Replace && ops.exists(path)?;

    // Keep the established save behavior where the platform has no durable directory
    // namespace barrier (currently Windows): file data is still synced and the same-dir
    // temp is atomically renamed, but the result does not claim namespace durability.
    // REL-010 remains Partial on this path until a native barrier and controls exist.
    if has_previous && !ops.directory_sync_is_durable() {
        ops.rename(temporary, path, existing)?;
        observe(Step::Rename);
        ops.sync_directory(directory)?;
        observe(Step::DirectorySyncUnsupported);
        return Ok(());
    }

    let recovery = recovery_path(path)?;

    // Preserve and make the previous bytes durable before the replacement rename.
    // Every save gets a fresh short name; older recovery entries are never touched.
    if has_previous {
        let mut backup = ops.create_temporary(directory)?;
        observe(Step::CreateRecoveryCopy);
        ops.copy_file(path, &mut backup)?;
        observe(Step::WriteRecoveryCopy);
        ops.sync_file(&mut backup)?;
        observe(Step::FsyncRecoveryCopy);
        ops.rename(backup, &recovery, Existing::Refuse)?;
        observe(Step::PersistRecoveryCopy);
        if let Err(error) = ops.sync_directory(directory) {
            let cleanup = ops
                .remove_file(&recovery)
                .and_then(|()| ops.sync_directory(directory));
            return Err(match cleanup {
                Ok(()) => error,
                Err(cleanup_error) => io::Error::new(
                    error.kind(),
                    format!(
                        "previous destination is unchanged; recovery-copy directory sync failed ({error}); cleanup is unconfirmed ({cleanup_error}); inspect {}",
                        recovery.display()
                    ),
                ),
            });
        }
        observe(Step::FsyncRecoveryDirectory);

        // If another writer completed while this copy was being prepared, do not use a
        // stale backup as if it represented the just-read destination.
        match ops.same_contents(path, &recovery) {
            Ok(true) => {}
            Ok(false) => {
                let cleanup = ops
                    .remove_file(&recovery)
                    .and_then(|()| ops.sync_directory(directory));
                return Err(match cleanup {
                    Ok(()) => io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "destination changed while its recovery copy was being prepared",
                    ),
                    Err(cleanup_error) => io::Error::new(
                        io::ErrorKind::WouldBlock,
                        format!(
                            "destination changed while its recovery copy was prepared; cleanup is unconfirmed ({cleanup_error}); inspect {}",
                            recovery.display()
                        ),
                    ),
                });
            }
            Err(error) => {
                let cleanup = ops
                    .remove_file(&recovery)
                    .and_then(|()| ops.sync_directory(directory));
                return Err(match cleanup {
                    Ok(()) => error,
                    Err(cleanup_error) => io::Error::new(
                        error.kind(),
                        format!(
                            "could not verify the previous-version copy ({error}); cleanup is unconfirmed ({cleanup_error}); inspect {}",
                            recovery.display()
                        ),
                    ),
                });
            }
        }
    }

    if let Err(error) = ops.rename(temporary, path, existing) {
        if !has_previous {
            return Err(error);
        }
        return match ops.same_contents(path, &recovery) {
            Ok(true) => {
                let cleanup = ops
                    .remove_file(&recovery)
                    .and_then(|()| ops.sync_directory(directory));
                match cleanup {
                    Ok(()) => Err(error),
                    Err(cleanup_error) => Err(io::Error::new(
                        error.kind(),
                        format!(
                            "replacement rename failed ({error}); previous target is unchanged but backup cleanup is unconfirmed ({cleanup_error}); recovery copy: {}",
                            recovery.display()
                        ),
                    )),
                }
            }
            Ok(false) => Err(io::Error::new(
                error.kind(),
                format!(
                    "replacement rename failed ({error}); destination no longer matches retained recovery copy {}",
                    recovery.display()
                ),
            )),
            Err(verify_error) => Err(io::Error::new(
                error.kind(),
                format!(
                    "replacement rename failed ({error}); cannot verify destination ({verify_error}); recovery copy retained at {}",
                    recovery.display()
                ),
            )),
        };
    }
    observe(Step::Rename);
    if let Err(error) = ops.sync_directory(directory) {
        if !has_previous {
            if existing == Existing::Refuse {
                // The no-clobber create is complete, but this platform/filesystem did not
                // confirm the namespace barrier. Leave it in place rather than risking removal
                // of a path that another actor may have installed after our rename.
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "directory sync failed; no-clobber create may have committed a complete file at {}: {error}",
                        path.display()
                    ),
                ));
            }
            // The lock proves this invocation created the new name. Restore absence when
            // possible; never remove a raced-in target because all FARIS writers share the lock.
            let rollback = ops.remove_file(path).and_then(|()| {
                observe(Step::RemoveUncommittedTarget);
                ops.sync_directory(directory)
            });
            return Err(match rollback {
                Ok(()) => io::Error::new(
                    error.kind(),
                    format!(
                        "directory sync failed; newly created destination was removed: {error}"
                    ),
                ),
                Err(rollback_error) => io::Error::new(
                    error.kind(),
                    format!(
                        "directory sync failed ({error}); absent-state rollback is unconfirmed ({rollback_error}); a complete new file may remain at {}",
                        path.display()
                    ),
                ),
            });
        }

        let rollback = (|| {
            let mut restored = ops.create_temporary(directory)?;
            observe(Step::CreateRollbackCopy);
            ops.copy_file(&recovery, &mut restored)?;
            observe(Step::WriteRollbackCopy);
            ops.sync_file(&mut restored)?;
            observe(Step::FsyncRollbackCopy);
            ops.rename(restored, path, Existing::Replace)?;
            observe(Step::RollbackRename);
            ops.sync_directory(directory)?;
            observe(Step::FsyncRollbackDirectory);
            Ok::<(), io::Error>(())
        })();

        return match rollback {
            Ok(()) => {
                // The old destination is now durable. Sidecar removal is cleanup only;
                // if it is not durable, the recoverable extra copy may reappear on boot.
                let cleanup = ops
                    .remove_file(&recovery)
                    .and_then(|()| ops.sync_directory(directory));
                let cleanup_note = match cleanup {
                    Ok(()) => String::new(),
                    Err(cleanup_error) => {
                        observe(Step::RecoveryCleanupPending);
                        eprintln!(
                            "FARIS atomic save rolled back to the previous destination; recovery-sidecar cleanup is unconfirmed ({cleanup_error}): {}",
                            recovery.display()
                        );
                        format!(
                            "; recovery-sidecar cleanup is unconfirmed at {}",
                            recovery.display()
                        )
                    }
                };
                Err(io::Error::new(
                    error.kind(),
                    format!(
                        "replacement directory sync failed; previous destination restored: {error}{cleanup_note}"
                    ),
                ))
            }
            Err(rollback_error) => Err(io::Error::new(
                error.kind(),
                format!(
                    "replacement directory sync failed ({error}); rollback failed ({rollback_error}); previous version retained at {}",
                    recovery.display()
                ),
            )),
        };
    }
    if ops.directory_sync_is_durable() {
        observe(Step::FsyncDirectory);
    } else {
        observe(Step::DirectorySyncUnsupported);
    }
    if has_previous {
        // Once the new destination is durable, remove only this invocation's backup.
        // Older recovery entries are never touched because every operation has a unique name.
        let cleanup = ops
            .remove_file(&recovery)
            .and_then(|()| ops.sync_directory(directory));
        if let Err(cleanup_error) = cleanup {
            observe(Step::RecoveryCleanupPending);
            eprintln!(
                "FARIS atomic save committed durably; recovery-sidecar cleanup is unconfirmed ({cleanup_error}): {}",
                recovery.display()
            );
        }
    }
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
        assert_eq!(names, [std::ffi::OsString::from("report.json")]);
    }

    // Verifies: REL-010
    #[test]
    fn a_failed_save_leaves_the_previous_file() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nowhere").join("a.json");
        assert!(write_atomic(&missing, b"x").is_err());
        assert!(!missing.exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_destinations_are_rejected_before_creating_lock_or_temp_files() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let referent = dir.path().join("referent.json");
        let link = dir.path().join("study.faris");
        std::fs::write(&referent, b"referent unchanged").unwrap();
        symlink(&referent, &link).unwrap();
        let initial_entries = std::fs::read_dir(dir.path()).unwrap().count();

        let error = write_atomic(&link, b"replacement").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&referent).unwrap(), b"referent unchanged");
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            initial_entries
        );

        let error = write_new(&link, b"new").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            initial_entries
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_parent_aliases_share_the_same_lock_and_target() {
        fn assert_waiter_blocks(held_path: &Path, waiting_path: &Path) {
            let held = acquire_destination_lock(held_path).unwrap();
            let waiting_path = waiting_path.to_path_buf();
            let (started_tx, started_rx) = std::sync::mpsc::channel();
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let waiter = std::thread::spawn(move || {
                started_tx.send(()).unwrap();
                acquire_destination_lock(&waiting_path).unwrap();
                done_tx.send(()).unwrap();
            });
            started_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap();
            assert!(
                done_rx
                    .recv_timeout(std::time::Duration::from_millis(40))
                    .is_err()
            );
            drop(held);
            done_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap();
            waiter.join().unwrap();
        }
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let real_target = dir.path().join("study.faris");
        let alias_parent = dir.path().join("alias");
        symlink(dir.path(), &alias_parent).unwrap();
        let alias_target = alias_parent.join("study.faris");
        assert_waiter_blocks(&real_target, &alias_target);
        write_atomic(&alias_target, b"created through parent alias").unwrap();
        write_atomic(&real_target, b"replaced through canonical parent").unwrap();
        assert_eq!(
            std::fs::read(real_target).unwrap(),
            b"replaced through canonical parent"
        );
    }

    #[cfg(unix)]
    #[test]
    fn repeated_saves_preserve_older_recovery_files_and_leave_legacy_suffix_alone() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("study.faris");
        std::fs::write(&target, b"original").unwrap();
        let unrelated = dir.path().join("study.faris.faris-previous");
        std::fs::write(&unrelated, b"user file").unwrap();

        // Interrupt after the new destination is durable but before this call cleans up
        // its uniquely named recovery copy, modelling a process kill in that window.
        let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = write_with(&target, Existing::Replace, b"second", &mut |step| {
                if step == Step::FsyncDirectory {
                    panic!("simulate interruption before invocation-local cleanup");
                }
            });
        }));
        assert!(interrupted.is_err());
        let first_recoveries = recovery_files(&target).unwrap();
        assert_eq!(first_recoveries.len(), 1);
        assert_eq!(std::fs::read(&first_recoveries[0]).unwrap(), b"original");
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"user file");

        write_atomic(&target, b"third").unwrap();
        let recoveries = recovery_files(&target).unwrap();
        assert_eq!(recoveries.len(), 1);
        assert_eq!(std::fs::read(&recoveries[0]).unwrap(), b"original");
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"user file");
    }

    #[cfg(unix)]
    #[test]
    fn recovery_names_fit_long_destination_basenames() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(format!("{}.dat", "x".repeat(240)));
        std::fs::write(&target, b"old").unwrap();
        let mut checked_recovery = false;
        write_with(&target, Existing::Replace, b"new", &mut |step| {
            if step == Step::FsyncDirectory {
                let copies = recovery_files(&target).unwrap();
                assert_eq!(copies.len(), 1);
                assert_eq!(std::fs::read(&copies[0]).unwrap(), b"old");
                assert!(copies[0].file_name().unwrap().len() < 200);
                checked_recovery = true;
            }
        })
        .unwrap();
        assert!(checked_recovery);
        assert!(recovery_files(&target).unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn destination_lock_blocks_a_second_writer_for_absent_then_existing_target() {
        fn assert_waiter_blocks(held_path: &Path, waiting_path: &Path) {
            let held = acquire_destination_lock(held_path).unwrap();
            let (started_tx, started_rx) = std::sync::mpsc::channel();
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let waiting_path = waiting_path.to_path_buf();
            let waiter = std::thread::spawn(move || {
                started_tx.send(()).unwrap();
                let _lock = acquire_destination_lock(&waiting_path).unwrap();
                done_tx.send(()).unwrap();
            });
            started_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap();
            assert!(
                done_rx
                    .recv_timeout(std::time::Duration::from_millis(40))
                    .is_err()
            );
            drop(held);
            done_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap();
            waiter.join().unwrap();
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("study.faris");
        assert_waiter_blocks(&path, &path); // Entry absent: both writers still share a lock.
        std::fs::write(&path, b"old").unwrap();
        assert_waiter_blocks(&path, &path); // Existing entry shares the parent-directory lock.
    }

    #[cfg(unix)]
    #[test]
    fn writer_waiting_during_postrename_rollback_cannot_be_overwritten_by_rollback() {
        fn exercise(initial: Option<&[u8]>) {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("study.faris");
            if let Some(old) = initial {
                std::fs::write(&path, old).unwrap();
            }
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let writer_path = path.clone();
            let old = initial.map(ToOwned::to_owned);
            let writer_a = std::thread::spawn(move || {
                let _lock = acquire_destination_lock(&writer_path).unwrap();
                // Model A's target rename followed by injected directory-sync failure.
                std::fs::write(&writer_path, b"writer A complete").unwrap();
                ready_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                // A's recovery rename/removal occurs while its destination lock is held.
                match old {
                    Some(old) => std::fs::write(&writer_path, old).unwrap(),
                    None => std::fs::remove_file(&writer_path).unwrap(),
                }
            });
            ready_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap();

            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let waiting_path = path.clone();
            let writer_b = std::thread::spawn(move || {
                write_atomic(&waiting_path, b"writer B complete").unwrap();
                done_tx.send(()).unwrap();
            });
            assert!(
                done_rx
                    .recv_timeout(std::time::Duration::from_millis(40))
                    .is_err()
            );
            release_tx.send(()).unwrap();
            writer_a.join().unwrap();
            done_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap();
            writer_b.join().unwrap();
            assert_eq!(std::fs::read(path).unwrap(), b"writer B complete");
        }

        exercise(None);
        exercise(Some(b"original complete"));
    }

    #[cfg(unix)]
    #[test]
    fn lock_acquisition_error_precedes_any_destination_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("missing").join("study.faris");
        let error = write_atomic(&target, b"new complete").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(!target.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[cfg(not(unix))]
    #[test]
    fn existing_target_replace_preserves_legacy_behavior_without_durability_claim() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("study.faris");
        std::fs::write(&target, b"old complete").unwrap();
        let mut steps = Vec::new();
        write_with(&target, Existing::Replace, b"new complete", &mut |step| {
            steps.push(step)
        })
        .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new complete");
        assert!(recovery_files(&target).unwrap().is_empty());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        assert_eq!(steps.last(), Some(&Step::DirectorySyncUnsupported));
        assert!(!steps.contains(&Step::CreateRecoveryCopy));
        let stream_target = dir.path().join("streamed.faris");
        std::fs::write(&stream_target, b"stream old").unwrap();
        let mut temporary = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        temporary.write_all(b"stream new").unwrap();
        temporary.as_file().sync_all().unwrap();
        commit(temporary, &stream_target, Existing::Replace).unwrap();
        assert_eq!(std::fs::read(&stream_target).unwrap(), b"stream new");
        assert!(recovery_files(&stream_target).unwrap().is_empty());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 4);
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
        let directory_barrier = if cfg!(unix) {
            Step::FsyncDirectory
        } else {
            Step::DirectorySyncUnsupported
        };
        assert_eq!(
            steps,
            vec![
                Step::CreateTemporary,
                Step::WriteData,
                Step::FsyncFile,
                Step::Rename,
                directory_barrier
            ]
        );
        #[cfg(unix)]
        assert!(crash_states_are_safe(&steps).unwrap() >= 6);
    }

    // Verifies: REL-010
    #[cfg(unix)]
    #[test]
    fn replacement_durably_stages_previous_file_before_rename() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a.json");
        std::fs::write(&target, b"previous").unwrap();
        let mut steps = Vec::new();
        write_with(&target, Existing::Replace, b"next", &mut |step| {
            steps.push(step);
            if step == Step::FsyncRecoveryDirectory {
                let copies = recovery_files(&target).unwrap();
                assert_eq!(copies.len(), 1);
                assert_eq!(std::fs::read(&copies[0]).unwrap(), b"previous");
                assert_eq!(std::fs::read(&target).unwrap(), b"previous");
            }
        })
        .unwrap();
        assert_eq!(
            steps,
            vec![
                Step::CreateTemporary,
                Step::WriteData,
                Step::FsyncFile,
                Step::CreateRecoveryCopy,
                Step::WriteRecoveryCopy,
                Step::FsyncRecoveryCopy,
                Step::PersistRecoveryCopy,
                Step::FsyncRecoveryDirectory,
                Step::Rename,
                Step::FsyncDirectory,
            ]
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"next");
        assert!(recovery_files(&target).unwrap().is_empty());
    }

    /// Volatile/durable named files for exercising backup, replacement, and rollback.
    #[derive(Clone)]
    struct ModelSnapshot {
        temporary: Option<Vec<u8>>,
        durable_payloads: Vec<Vec<u8>>,
        target: Option<Vec<u8>>,
        durable_target: Option<Vec<u8>>,
        target_dir_dirty: bool,
        backup: Option<Vec<u8>>,
        durable_backup: Option<Vec<u8>>,
        backup_dir_dirty: bool,
        archives: Vec<Vec<u8>>,
        durable_archives: Vec<Vec<u8>>,
    }

    struct ModelOps {
        snapshot: ModelSnapshot,
        history: Vec<ModelSnapshot>,
        temp_create_calls: usize,
        fail_temp_create_calls: Vec<usize>,
        file_sync_calls: usize,
        fail_file_sync_calls: Vec<usize>,
        directory_sync_calls: usize,
        fail_directory_sync_calls: Vec<usize>,
        rename_calls: usize,
        fail_backup_persist: bool,

        fail_target_rename: bool,
        fail_rollback_rename: bool,
        fail_backup_copy: bool,
        fail_rollback_copy: bool,
        fail_same_contents: bool,
        mismatch_same_contents: bool,
        fail_remove: bool,
        fail_write_data: bool,
        lose_file_sync: bool,
    }

    impl ModelOps {
        fn new(previous: &[u8]) -> Self {
            Self {
                snapshot: ModelSnapshot {
                    temporary: None,
                    durable_payloads: Vec::new(),
                    target: Some(previous.to_vec()),
                    durable_target: Some(previous.to_vec()),
                    target_dir_dirty: false,
                    backup: None,
                    durable_backup: None,
                    backup_dir_dirty: false,
                    archives: Vec::new(),
                    durable_archives: Vec::new(),
                },
                history: Vec::new(),
                temp_create_calls: 0,
                fail_temp_create_calls: Vec::new(),
                file_sync_calls: 0,
                fail_file_sync_calls: Vec::new(),
                directory_sync_calls: 0,
                fail_directory_sync_calls: Vec::new(),
                rename_calls: 0,
                fail_backup_persist: false,

                fail_target_rename: false,
                fail_rollback_rename: false,
                fail_backup_copy: false,
                fail_rollback_copy: false,
                fail_same_contents: false,
                mismatch_same_contents: false,
                fail_remove: false,
                fail_write_data: false,
                lose_file_sync: false,
            }
        }

        fn record(&mut self) {
            self.history.push(self.snapshot.clone());
        }

        fn is_backup_path(path: &Path) -> bool {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".faris-previous-"))
        }

        /// A crash may retain or drop each unsynced rename. Durable old bytes remain
        /// recoverable at the target or backup; unsynced file contents may be partial.
        fn crash_reopens(&self, state: &ModelSnapshot, previous: &[u8]) -> Vec<Vec<u8>> {
            if state.target_dir_dirty {
                let visible = state.target.clone().unwrap_or_else(|| b"<absent>".to_vec());
                let old = state
                    .durable_target
                    .clone()
                    .unwrap_or_else(|| previous.to_vec());
                let visible = if state
                    .target
                    .as_ref()
                    .is_some_and(|bytes| !state.durable_payloads.contains(bytes))
                {
                    b"<partial-or-missing-data>".to_vec()
                } else {
                    visible
                };
                vec![old, visible]
            } else {
                vec![state.target.clone().unwrap_or_else(|| b"<absent>".to_vec())]
            }
        }

        fn crash_pairs(
            &self,
            state: &ModelSnapshot,
            previous: &[u8],
        ) -> Vec<(Vec<u8>, Option<Vec<u8>>)> {
            let targets = self.crash_reopens(state, previous);
            let mut preserved = vec![state.durable_backup.clone()];
            preserved.extend(state.durable_archives.iter().cloned().map(Some));
            if state.backup_dir_dirty {
                preserved.push(state.backup.clone());
            }
            let backups = preserved
                .into_iter()
                .map(|backup| {
                    backup.map(|bytes| {
                        if state.durable_payloads.contains(&bytes) {
                            bytes
                        } else {
                            b"<partial-backup-data>".to_vec()
                        }
                    })
                })
                .collect::<Vec<_>>();
            targets
                .into_iter()
                .flat_map(|target| {
                    backups
                        .iter()
                        .cloned()
                        .map(move |backup| (target.clone(), backup))
                })
                .collect()
        }
    }

    impl AtomicFileOps for ModelOps {
        type Temporary = Vec<u8>;
        type Lock = ();

        fn lock_destination(&mut self, _path: &Path) -> io::Result<Self::Lock> {
            Ok(())
        }

        fn reject_symlink_destination(
            &mut self,
            _path: &Path,
            _existing: Existing,
        ) -> io::Result<()> {
            Ok(())
        }

        fn validate_destination(&mut self, _path: &Path, existing: Existing) -> io::Result<()> {
            if existing == Existing::Refuse && self.snapshot.target.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "target exists",
                ));
            }
            Ok(())
        }

        fn directory_sync_is_durable(&self) -> bool {
            true
        }

        fn create_temporary(&mut self, _directory: &Path) -> io::Result<Self::Temporary> {
            self.temp_create_calls += 1;
            if self
                .fail_temp_create_calls
                .contains(&self.temp_create_calls)
            {
                return Err(io::Error::other("injected temporary create failure"));
            }
            self.snapshot.temporary = Some(Vec::new());
            self.record();
            Ok(Vec::new())
        }

        fn write_data(&mut self, temporary: &mut Self::Temporary, bytes: &[u8]) -> io::Result<()> {
            if self.fail_write_data {
                return Err(io::Error::other("injected write failure"));
            }
            temporary.clear();
            temporary.extend_from_slice(bytes);
            self.snapshot.temporary = Some(temporary.clone());
            self.record();
            Ok(())
        }

        fn copy_file(&mut self, source: &Path, temporary: &mut Self::Temporary) -> io::Result<()> {
            if Self::is_backup_path(source) && self.fail_rollback_copy {
                return Err(io::Error::other("injected rollback copy failure"));
            }
            if !Self::is_backup_path(source) && self.fail_backup_copy {
                return Err(io::Error::other("injected backup copy failure"));
            }
            let contents = if Self::is_backup_path(source) {
                self.snapshot.backup.as_ref()
            } else {
                self.snapshot.target.as_ref()
            }
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "model source missing"))?;
            temporary.clear();
            temporary.extend_from_slice(contents);
            self.snapshot.temporary = Some(temporary.clone());
            self.record();
            Ok(())
        }

        fn same_contents(&mut self, left: &Path, right: &Path) -> io::Result<bool> {
            if self.fail_same_contents {
                return Err(io::Error::other(
                    "injected recovery-copy verification failure",
                ));
            }
            if self.mismatch_same_contents {
                return Ok(false);
            }
            let contents = |path: &Path| {
                if Self::is_backup_path(path) {
                    self.snapshot.backup.as_ref()
                } else {
                    self.snapshot.target.as_ref()
                }
            };
            let left = contents(left);
            let right = contents(right);
            Ok(left == right)
        }

        fn sync_file(&mut self, temporary: &mut Self::Temporary) -> io::Result<()> {
            self.file_sync_calls += 1;
            if self.fail_file_sync_calls.contains(&self.file_sync_calls) {
                return Err(io::Error::other("injected file sync failure"));
            }
            if !self.lose_file_sync && !self.snapshot.durable_payloads.contains(temporary) {
                self.snapshot.durable_payloads.push(temporary.clone());
            }
            self.record();
            Ok(())
        }

        fn rename(
            &mut self,
            temporary: Self::Temporary,
            path: &Path,
            existing: Existing,
        ) -> io::Result<()> {
            self.rename_calls += 1;
            if Self::is_backup_path(path) {
                if self.fail_backup_persist {
                    return Err(io::Error::other("injected backup persist failure"));
                }
                if existing == Existing::Refuse && self.snapshot.backup.is_some() {
                    self.snapshot
                        .archives
                        .push(self.snapshot.backup.take().unwrap());
                    if let Some(durable) = self.snapshot.durable_backup.take() {
                        self.snapshot.durable_archives.push(durable);
                    }
                }
                self.snapshot.backup = Some(temporary);
                self.snapshot.backup_dir_dirty = true;
            } else {
                if (self.rename_calls == 2 && self.fail_target_rename)
                    || (self.rename_calls == 3 && self.fail_rollback_rename)
                {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "injected Windows-style sharing violation",
                    ));
                }
                if existing == Existing::Refuse && self.snapshot.target.is_some() {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "target exists",
                    ));
                }
                self.snapshot.target = Some(temporary);
                self.snapshot.target_dir_dirty = true;
            }
            self.snapshot.temporary = None;
            self.record();
            Ok(())
        }

        fn exists(&mut self, path: &Path) -> io::Result<bool> {
            Ok(if Self::is_backup_path(path) {
                self.snapshot.backup.is_some()
            } else {
                self.snapshot.target.is_some()
            })
        }

        fn remove_file(&mut self, path: &Path) -> io::Result<()> {
            if self.fail_remove {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected cleanup failure",
                ));
            }
            if Self::is_backup_path(path) {
                self.snapshot.backup = None;
                self.snapshot.backup_dir_dirty = true;
            } else {
                self.snapshot.target = None;
                self.snapshot.target_dir_dirty = true;
            }
            self.record();
            Ok(())
        }

        fn sync_directory(&mut self, _directory: &Path) -> io::Result<()> {
            self.directory_sync_calls += 1;
            if self
                .fail_directory_sync_calls
                .contains(&self.directory_sync_calls)
            {
                return Err(io::Error::other("injected directory sync failure"));
            }
            if self.snapshot.backup_dir_dirty {
                self.snapshot.durable_backup = self.snapshot.backup.clone().map(|bytes| {
                    if self.snapshot.durable_payloads.contains(&bytes) {
                        bytes
                    } else {
                        b"<partial-backup-data>".to_vec()
                    }
                });
                self.snapshot.backup_dir_dirty = false;
            }
            if self.snapshot.target_dir_dirty {
                let target = self.snapshot.target.clone();
                self.snapshot.durable_target = target.map(|bytes| {
                    if self.snapshot.durable_payloads.contains(&bytes) {
                        bytes
                    } else {
                        b"<partial-or-missing-data>".to_vec()
                    }
                });
                self.snapshot.target_dir_dirty = false;
            }
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

        assert!(model.history.len() > 5);
        for (index, state) in model.history.iter().enumerate() {
            for reopened in model.crash_reopens(state, previous) {
                assert!(
                    reopened == previous || reopened == next,
                    "crash/reopen exposed partial data: {reopened:?}"
                );
            }
            let committed =
                state.durable_target.as_deref() == Some(&next[..]) && !state.target_dir_dirty;
            if !committed {
                for (target, backup) in model.crash_pairs(state, previous) {
                    assert!(
                        target.as_slice() == previous || backup.as_deref() == Some(&previous[..]),
                        "history prefix {index} loses the old version: target={target:?}, backup={backup:?}"
                    );
                }
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

        let after_rename = model
            .history
            .iter()
            .find(|state| state.target_dir_dirty && state.target.as_deref() == Some(&next[..]))
            .expect("replacement rename is represented in the model history");
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
        model.fail_directory_sync_calls = vec![2];
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
                Step::CreateRecoveryCopy,
                Step::WriteRecoveryCopy,
                Step::FsyncRecoveryCopy,
                Step::PersistRecoveryCopy,
                Step::FsyncRecoveryDirectory,
                Step::Rename,
                Step::CreateRollbackCopy,
                Step::WriteRollbackCopy,
                Step::FsyncRollbackCopy,
                Step::RollbackRename,
                Step::FsyncRollbackDirectory,
            ]
        );
        assert_eq!(model.snapshot.durable_target, Some(b"previous".to_vec()));
        assert_eq!(
            model.crash_reopens(&model.snapshot, b"previous"),
            vec![b"previous".to_vec()]
        );
    }

    // Verifies: REL-010
    #[test]
    fn absent_replace_target_is_removed_on_postrename_sync_error() {
        let mut model = ModelOps::new(b"seed");
        model.snapshot.target = None;
        model.snapshot.durable_target = None;
        model.fail_directory_sync_calls = vec![1];
        let error = write_with_ops(
            Path::new("a.json"),
            Existing::Replace,
            b"new",
            &mut |_| {},
            &mut model,
        )
        .unwrap_err();
        assert!(error.to_string().contains("removed"));
        assert_eq!(model.snapshot.target, None);
        assert_eq!(model.snapshot.durable_target, None);
    }

    // Verifies: REL-010
    #[test]
    fn no_clobber_create_error_does_not_unlink_a_complete_destination() {
        let mut model = ModelOps::new(b"seed");
        model.snapshot.target = None;
        model.snapshot.durable_target = None;
        model.fail_directory_sync_calls = vec![1];
        let error = write_with_ops(
            Path::new("a.json"),
            Existing::Refuse,
            b"new",
            &mut |_| {},
            &mut model,
        )
        .unwrap_err();
        assert!(error.to_string().contains("complete file"));
        assert_eq!(model.snapshot.target.as_deref(), Some(&b"new"[..]));
        assert_eq!(model.snapshot.durable_target, None);
    }

    // Verifies: REL-010
    #[test]
    fn backup_stage_errors_leave_old_destination_and_resolve_sidecar() {
        for point in [
            "temp-create",
            "write",
            "backup-copy",
            "backup-file-sync",
            "backup-persist",
            "backup-dir-sync",
            "backup-verify-error",
            "backup-verify-mismatch",
        ] {
            let mut model = ModelOps::new(b"previous");
            match point {
                "temp-create" => model.fail_temp_create_calls = vec![2],
                "write" => model.fail_write_data = true,
                "backup-copy" => model.fail_backup_copy = true,
                "backup-file-sync" => model.fail_file_sync_calls = vec![2],
                "backup-persist" => model.fail_backup_persist = true,
                "backup-dir-sync" => model.fail_directory_sync_calls = vec![1],
                "backup-verify-error" => model.fail_same_contents = true,
                "backup-verify-mismatch" => model.mismatch_same_contents = true,
                _ => unreachable!(),
            }
            let error = write_with_ops(
                Path::new("a.json"),
                Existing::Replace,
                b"next",
                &mut |_| {},
                &mut model,
            )
            .unwrap_err();
            assert!(!error.to_string().is_empty(), "{point}");
            assert_eq!(
                model.snapshot.target.as_deref(),
                Some(&b"previous"[..]),
                "{point}"
            );
            assert_eq!(
                model.snapshot.durable_target.as_deref(),
                Some(&b"previous"[..]),
                "{point}"
            );
            assert!(
                !model
                    .history
                    .iter()
                    .any(|state| state.target.as_deref() == Some(&b"next"[..])),
                "{point}"
            );
        }
    }

    // Verifies: REL-010
    #[test]
    fn every_rollback_failure_retains_old_durable_sidecar_and_names_it() {
        for point in [
            "rollback-create",
            "rollback-copy",
            "rollback-file-sync",
            "rollback-rename",
            "rollback-dir-sync",
        ] {
            let mut model = ModelOps::new(b"previous");
            model.fail_directory_sync_calls = if point == "rollback-dir-sync" {
                vec![2, 3]
            } else {
                vec![2]
            };
            match point {
                "rollback-create" => model.fail_temp_create_calls = vec![3],
                "rollback-copy" => model.fail_rollback_copy = true,
                "rollback-file-sync" => model.fail_file_sync_calls = vec![3],
                "rollback-rename" => model.fail_rollback_rename = true,
                "rollback-dir-sync" => {}
                _ => unreachable!(),
            }
            let error = write_with_ops(
                Path::new("a.json"),
                Existing::Replace,
                b"next",
                &mut |_| {},
                &mut model,
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("faris-previous-"),
                "{point}: {error}"
            );
            assert_eq!(
                model.snapshot.durable_backup.as_deref(),
                Some(&b"previous"[..]),
                "{point}"
            );
            for (target, backup) in model.crash_pairs(&model.snapshot, b"previous") {
                assert!(
                    target.as_slice() == b"previous" || target.as_slice() == b"next",
                    "{point}"
                );
                assert_eq!(backup.as_deref(), Some(&b"previous"[..]), "{point}");
            }
        }
    }

    // Verifies: REL-010
    #[test]
    fn primary_rename_error_cleans_the_staged_backup_before_returning() {
        let mut model = ModelOps::new(b"previous");
        model.fail_target_rename = true; // Includes Windows-style sharing violations.
        let error = write_with_ops(
            Path::new("a.json"),
            Existing::Replace,
            b"next",
            &mut |_| {},
            &mut model,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(model.snapshot.target.as_deref(), Some(&b"previous"[..]));
        assert_eq!(
            model.snapshot.durable_target.as_deref(),
            Some(&b"previous"[..])
        );
        assert!(model.snapshot.backup.is_none());
        assert!(model.snapshot.durable_backup.is_none());
    }

    // Verifies: REL-010
    #[test]
    fn primary_rename_error_reports_and_retains_backup_if_cleanup_fails() {
        let mut model = ModelOps::new(b"previous");
        model.fail_target_rename = true;
        model.fail_remove = true;
        let error = write_with_ops(
            Path::new("a.json"),
            Existing::Replace,
            b"next",
            &mut |_| {},
            &mut model,
        )
        .unwrap_err();
        assert!(error.to_string().contains("faris-previous-"));
        assert_eq!(model.snapshot.target.as_deref(), Some(&b"previous"[..]));
        assert_eq!(
            model.snapshot.durable_backup.as_deref(),
            Some(&b"previous"[..])
        );

        // A later update gets a distinct recovery name and proceeds.
        model.fail_remove = false;
        model.fail_target_rename = false;
        write_with_ops(
            Path::new("a.json"),
            Existing::Replace,
            b"retry",
            &mut |_| {},
            &mut model,
        )
        .unwrap();
        assert_eq!(model.snapshot.target.as_deref(), Some(&b"retry"[..]));
    }

    #[test]
    fn primary_rename_error_reports_backup_if_cleanup_directory_sync_fails() {
        let mut model = ModelOps::new(b"previous");
        model.fail_target_rename = true;
        model.fail_directory_sync_calls = vec![2]; // Backup staging succeeds; cleanup barrier fails.
        let error = write_with_ops(
            Path::new("a.json"),
            Existing::Replace,
            b"next",
            &mut |_| {},
            &mut model,
        )
        .unwrap_err();
        assert!(error.to_string().contains("faris-previous-"));
        assert_eq!(model.snapshot.target.as_deref(), Some(&b"previous"[..]));
        assert_eq!(
            model.snapshot.durable_backup.as_deref(),
            Some(&b"previous"[..])
        );
    }

    // Verifies: REL-010
    #[test]
    fn failed_rollback_retains_durable_previous_copy_and_reports_its_path() {
        let mut model = ModelOps::new(b"previous");
        model.fail_directory_sync_calls = vec![2, 3];
        let error = write_with_ops(
            Path::new("a.json"),
            Existing::Replace,
            b"next",
            &mut |_| {},
            &mut model,
        )
        .unwrap_err();
        assert!(error.to_string().contains("faris-previous-"));
        assert_eq!(model.snapshot.durable_backup, Some(b"previous".to_vec()));
        for (target, backup) in model.crash_pairs(&model.snapshot, b"previous") {
            assert!(target.as_slice() == b"previous" || target.as_slice() == b"next");
            assert_eq!(backup.as_deref(), Some(&b"previous"[..]));
        }
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
