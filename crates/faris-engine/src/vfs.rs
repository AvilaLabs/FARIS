//! Reading recorded files by path on both targets.
//!
//! On the desktop these are the operating system's files. The browser build
//! has no file system: the page puts the bytes it fetched or was given into an
//! in-memory table under the paths the loaders will ask for, and the same
//! loaders read them from there.

use std::{io, path::Path};

/// A regular file's content, refusing anything larger than `cap` bytes (the
/// caller sees `cap + 1` bytes at most and decides how to report it).
#[cfg(not(target_arch = "wasm32"))]
pub fn read_capped(path: &Path, cap: u64) -> io::Result<Vec<u8>> {
    use std::io::Read;
    if !std::fs::metadata(path)?.is_file() {
        return Err(io::Error::other("input must be a regular file"));
    }
    let file = std::fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("input must be a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(cap.saturating_add(1)).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// The whole file.
#[cfg(not(target_arch = "wasm32"))]
pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    std::fs::read(path)
}

/// The size of a regular file, or `None` when the path is not one.
#[cfg(not(target_arch = "wasm32"))]
pub fn file_len(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.len())
}

/// Create a new file; fails if the path exists. Missing parent folders are
/// created.
#[cfg(not(target_arch = "wasm32"))]
pub fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::atomic_file::write_new(path, bytes)
}

/// Write a file, creating its folders and replacing what was there.
#[cfg(not(target_arch = "wasm32"))]
pub fn write_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)
}

#[cfg(target_arch = "wasm32")]
mod table {
    use super::*;
    use std::path::PathBuf;
    use std::{collections::BTreeMap, sync::Arc, sync::Mutex};

    static FILES: Mutex<BTreeMap<PathBuf, Arc<Vec<u8>>>> = Mutex::new(BTreeMap::new());

    fn files() -> std::sync::MutexGuard<'static, BTreeMap<PathBuf, Arc<Vec<u8>>>> {
        FILES.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn missing(path: &Path) -> io::Error {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} is not loaded in this browser tab", path.display()),
        )
    }

    pub fn get(path: &Path) -> io::Result<Arc<Vec<u8>>> {
        files().get(path).cloned().ok_or_else(|| missing(path))
    }

    pub fn put(path: &Path, bytes: Vec<u8>) {
        files().insert(path.to_path_buf(), Arc::new(bytes));
    }

    pub fn put_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
        let mut files = files();
        if files.contains_key(path) {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        files.insert(path.to_path_buf(), Arc::new(bytes.to_vec()));
        Ok(())
    }

    pub fn remove_under(prefix: &Path) {
        files().retain(|path, _| !path.starts_with(prefix));
    }

    pub fn len(path: &Path) -> Option<u64> {
        files().get(path).map(|b| b.len() as u64)
    }
}

#[cfg(target_arch = "wasm32")]
pub fn read_capped(path: &Path, cap: u64) -> io::Result<Vec<u8>> {
    let bytes = table::get(path)?;
    let end = bytes.len().min(cap.saturating_add(1) as usize);
    Ok(bytes[..end].to_vec())
}

#[cfg(target_arch = "wasm32")]
pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    Ok(table::get(path)?.as_ref().clone())
}

#[cfg(target_arch = "wasm32")]
pub fn file_len(path: &Path) -> Option<u64> {
    table::len(path)
}

#[cfg(target_arch = "wasm32")]
pub fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    table::put_new(path, bytes)
}

#[cfg(target_arch = "wasm32")]
pub fn write_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    table::put(path, bytes.to_vec());
    Ok(())
}

/// Browser only: make `bytes` readable at `path`.
#[cfg(target_arch = "wasm32")]
pub fn install(path: &Path, bytes: Vec<u8>) {
    table::put(path, bytes);
}

/// Browser only: forget `path`, once everything that needed it has loaded.
#[cfg(target_arch = "wasm32")]
pub fn remove(path: &Path) {
    table::remove_under(path);
}

/// A private scratch folder that is removed when dropped: a temporary
/// directory on the desktop, a folder in the in-memory table in the browser.
pub struct ScratchDir {
    #[cfg(not(target_arch = "wasm32"))]
    dir: tempfile::TempDir,
    #[cfg(target_arch = "wasm32")]
    path: std::path::PathBuf,
}

impl ScratchDir {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            dir: tempfile::tempdir()?,
        })
    }

    /// A scratch folder inside `parent`, whose name starts with `prefix`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new_in(parent: &Path, prefix: &str) -> io::Result<Self> {
        std::fs::create_dir_all(parent)?;
        Ok(Self {
            dir: tempfile::Builder::new().prefix(prefix).tempdir_in(parent)?,
        })
    }

    #[cfg(target_arch = "wasm32")]
    pub fn new_in(_parent: &Path, _prefix: &str) -> io::Result<Self> {
        Self::new()
    }

    #[cfg(target_arch = "wasm32")]
    pub fn new() -> io::Result<Self> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        Ok(Self {
            path: std::path::PathBuf::from(format!("/faris-scratch/{id}")),
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    #[cfg(target_arch = "wasm32")]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for ScratchDir {
    fn drop(&mut self) {
        table::remove_under(&self.path);
    }
}
