//! Where large temporary data goes (requirement PLAT-029). The system temporary folder may be memory-backed, so
//! unpacked studies and conversion work folders are made under a FARIS folder instead: the folder named by
//! `FARIS_SCRATCH_DIR`, else `scratch` in the per-user state folder that recorded-demo runs also use.

use std::path::PathBuf;

/// The environment variable that names the scratch folder.
pub const SCRATCH_ENV: &str = "FARIS_SCRATCH_DIR";

/// The folder scratch directories are made in, from an environment lookup so tests need not change the process
/// environment. `None` when neither the variable nor a home folder gives an absolute path.
pub fn scratch_base_with(os: &str, env: &dyn Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    let absolute = |name: &str| env(name).filter(|p| p.is_absolute());
    if let Some(dir) = absolute(SCRATCH_ENV) {
        return Some(dir);
    }
    let state = match os {
        "windows" => absolute("LOCALAPPDATA")
            .map(|p| p.join("FARIS"))
            .or_else(|| absolute("USERPROFILE").map(|p| p.join("AppData/Local/FARIS"))),
        "macos" => absolute("HOME").map(|p| p.join("Library/Application Support/FARIS")),
        _ => absolute("XDG_STATE_HOME")
            .map(|p| p.join("faris"))
            .or_else(|| absolute("HOME").map(|p| p.join(".local/state/faris"))),
    };
    state.map(|p| p.join("scratch"))
}

/// The scratch folder for this process.
pub fn scratch_base() -> Option<PathBuf> {
    scratch_base_with(std::env::consts::OS, &|name| {
        std::env::var_os(name).map(PathBuf::from)
    })
}

/// A new temporary directory under the scratch folder, removed when dropped. Only when no scratch folder can be
/// named at all (no home folder, as in a browser) does it use the system temporary folder.
pub fn tempdir(prefix: &str) -> std::io::Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(prefix);
    match scratch_base() {
        Some(base) => {
            std::fs::create_dir_all(&base)?;
            builder.tempdir_in(base)
        }
        None => builder.tempdir(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<PathBuf> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| PathBuf::from(v))
        }
    }

    // Verifies: PLAT-029
    #[test]
    fn the_scratch_folder_is_configured_or_in_the_user_state_folder() {
        let configured = env(&[(SCRATCH_ENV, "/data/faris-scratch"), ("HOME", "/home/a")]);
        assert_eq!(
            scratch_base_with("linux", &configured),
            Some(PathBuf::from("/data/faris-scratch"))
        );
        let home = env(&[("HOME", "/home/a")]);
        assert_eq!(
            scratch_base_with("linux", &home),
            Some(PathBuf::from("/home/a/.local/state/faris/scratch"))
        );
        let xdg = env(&[("XDG_STATE_HOME", "/state"), ("HOME", "/home/a")]);
        assert_eq!(
            scratch_base_with("linux", &xdg),
            Some(PathBuf::from("/state/faris/scratch"))
        );
        assert_eq!(
            scratch_base_with("macos", &home),
            Some(PathBuf::from(
                "/home/a/Library/Application Support/FARIS/scratch"
            ))
        );
        let windows = env(&[("LOCALAPPDATA", "/c/Local")]);
        assert_eq!(
            scratch_base_with("windows", &windows),
            Some(PathBuf::from("/c/Local/FARIS/scratch"))
        );
        // A relative folder is not trusted.
        let relative = env(&[(SCRATCH_ENV, "scratch"), ("HOME", "relative")]);
        assert_eq!(scratch_base_with("linux", &relative), None);
    }
}
