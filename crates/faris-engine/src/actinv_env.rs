// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! Environment variables deliberately passed to ACTINV jobs.

use std::ffi::{OsStr, OsString};
use std::path::Path;

const PASSTHROUGH: &[&str] = &[
    "PATH",
    "HOME",
    "LANG",
    "LC_ALL",
    "TMPDIR",
    "ACTINV_CACHE_DIR",
    "XDG_CACHE_HOME",
    "LOCALAPPDATA",
];

fn allowed_name(name: &OsStr, case_insensitive: bool) -> Option<&'static str> {
    PASSTHROUGH.iter().copied().find(|allowed| {
        if case_insensitive {
            // The allowlist contains ASCII only. Compare without a lossy conversion of the
            // inherited name, preserving Windows's case-insensitive environment lookup.
            name.as_encoded_bytes()
                .eq_ignore_ascii_case(allowed.as_bytes())
        } else {
            name == OsStr::new(allowed)
        }
    })
}

/// Keep only runtime basics and the platform cache-root overrides ACTINV documents. The data
/// directory comes from the adapter's checked configuration, never from the ambient environment.
pub(crate) fn job_environment(
    inherited: impl IntoIterator<Item = (OsString, OsString)>,
    data_dir: Option<&Path>,
) -> Vec<(OsString, OsString)> {
    let mut environment: Vec<_> = inherited
        .into_iter()
        .filter_map(|(name, value)| {
            allowed_name(&name, cfg!(windows)).map(|canonical| (canonical.into(), value))
        })
        .collect();
    if let Some(path) = data_dir {
        environment.push(("ACTINV_DATA_DIR".into(), path.as_os_str().to_owned()));
    }
    environment
}

/// Resolve ACTINV's documented cache precedence, reject an unsafe cache-root leaf, and pass the
/// canonical directory as an explicit `ACTINV_CACHE_DIR` so the runner monitors the same root.
pub(crate) fn for_actinv(
    inherited: impl IntoIterator<Item = (OsString, OsString)>,
    data_dir: Option<&Path>,
) -> Result<(Vec<(OsString, OsString)>, std::path::PathBuf), String> {
    for_actinv_with_cache_dir(inherited, data_dir, None)
}

/// Resolve ACTINV's cache the same way as [`for_actinv`], with an explicit optional cache root
/// for isolated callers such as fake-tool tests. This avoids mutating the process environment.
pub(crate) fn for_actinv_with_cache_dir(
    inherited: impl IntoIterator<Item = (OsString, OsString)>,
    data_dir: Option<&Path>,
    cache_dir: Option<&Path>,
) -> Result<(Vec<(OsString, OsString)>, std::path::PathBuf), String> {
    let mut environment = job_environment(inherited, data_dir);
    if let Some(path) = cache_dir {
        if !path.is_absolute() {
            return Err("explicit ACTINV cache directory must be absolute".into());
        }
        environment.retain(|(name, _)| name != "ACTINV_CACHE_DIR");
        environment.push(("ACTINV_CACHE_DIR".into(), path.as_os_str().to_owned()));
    }
    let value = |name: &str| {
        environment
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    let root = if let Some(path) = value("ACTINV_CACHE_DIR") {
        if path.is_empty() {
            return Err("ACTINV_CACHE_DIR is empty".into());
        }
        let path = std::path::PathBuf::from(path.as_os_str());
        if !path.is_absolute() {
            return Err("ACTINV_CACHE_DIR must be an absolute path".into());
        }
        path
    } else {
        #[cfg(windows)]
        let platform_root = value("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|path| path.join("ACTINV"));
        #[cfg(not(windows))]
        let platform_root: Option<std::path::PathBuf> = None;
        if let Some(root) = platform_root {
            root
        } else if let Some(path) = value("XDG_CACHE_HOME") {
            let path = std::path::PathBuf::from(path.as_os_str());
            if !path.is_absolute() {
                return Err("XDG_CACHE_HOME must be absolute for ACTINV cache placement".into());
            }
            path.join("actinv")
        } else if let Some(path) = value("HOME") {
            let path = std::path::PathBuf::from(path.as_os_str());
            if !path.is_absolute() {
                return Err("HOME must be absolute for ACTINV cache placement".into());
            }
            path.join(".cache").join("actinv")
        } else {
            return Err("cannot determine ACTINV cache directory; set ACTINV_CACHE_DIR".into());
        }
    };
    match std::fs::symlink_metadata(&root) {
        Ok(metadata) if is_link_or_reparse(&metadata) || !metadata.is_dir() => {
            return Err(format!(
                "ACTINV cache root must be a real directory: {}",
                root.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "cannot inspect ACTINV cache {}: {error}",
                root.display()
            ));
        }
    }
    std::fs::create_dir_all(&root)
        .map_err(|error| format!("cannot create ACTINV cache {}: {error}", root.display()))?;
    let metadata = std::fs::symlink_metadata(&root)
        .map_err(|error| format!("cannot inspect ACTINV cache {}: {error}", root.display()))?;
    if is_link_or_reparse(&metadata) || !metadata.is_dir() {
        return Err(format!(
            "ACTINV cache root must be a real directory: {}",
            root.display()
        ));
    }
    let root = root
        .canonicalize()
        .map_err(|error| format!("cannot resolve ACTINV cache directory: {error}"))?;
    environment.retain(|(name, _)| name != "ACTINV_CACHE_DIR");
    environment.push(("ACTINV_CACHE_DIR".into(), root.as_os_str().to_owned()));
    Ok((environment, root))
}

#[cfg(windows)]
fn is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(test)]
mod tests {
    use super::{allowed_name, for_actinv, for_actinv_with_cache_dir, job_environment};
    use crate::jobs::{JobSpec, ResourceLimits};
    use std::collections::BTreeMap;
    use std::ffi::{OsStr, OsString};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    #[test]
    fn environment_names_keep_windows_lookup_and_unix_exact_matching() {
        for (name, expected) in [("Path", "PATH"), ("xDg_CaChE_HoMe", "XDG_CACHE_HOME")] {
            assert_eq!(allowed_name(OsStr::new(name), true), Some(expected));
            assert_eq!(allowed_name(OsStr::new(name), false), None);
        }
        assert_eq!(allowed_name(OsStr::new("PATH"), false), Some("PATH"));
        assert_eq!(allowed_name(OsStr::new("AWS_ACCESS_KEY_ID"), true), None);
        let got: BTreeMap<_, _> = job_environment(
            [(OsString::from("Path"), OsString::from("/tool/bin"))],
            None,
        )
        .into_iter()
        .collect();
        assert_eq!(got.contains_key(OsStr::new("PATH")), cfg!(windows));
    }

    #[test]
    fn actinv_job_keeps_documented_cache_roots_but_no_unrelated_environment() {
        let inherited = [
            ("PATH", "/usr/bin"),
            ("HOME", "/home/user"),
            ("LANG", "en_US.UTF-8"),
            ("LC_ALL", "C"),
            ("TMPDIR", "/tmp"),
            ("ACTINV_CACHE_DIR", "/custom/actinv"),
            ("XDG_CACHE_HOME", "/xdg/cache"),
            ("LOCALAPPDATA", "C:\\Users\\user\\AppData\\Local"),
            ("ACTINV_DATA_DIR", "/untrusted/data"),
            ("ACTINV_LICENSE_KEY", "secret"),
            ("AWS_ACCESS_KEY_ID", "secret"),
        ]
        .into_iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)));
        let data_dir = PathBuf::from("/checked/data");
        let environment = job_environment(inherited, Some(&data_dir));
        let spec = JobSpec {
            program: "actinv".into(),
            arguments: Vec::new(),
            working_directory: "/tmp".into(),
            environment,
            timeout: Duration::from_secs(1),
            capture_limit_bytes: 1024,
            artifact_roots: Vec::new(),
            resource_limits: ResourceLimits::default(),
            memory_workload: None,
        };
        let got: BTreeMap<_, _> = spec
            .environment
            .into_iter()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                )
            })
            .collect();
        assert_eq!(got.len(), 9);
        assert_eq!(got["PATH"], "/usr/bin");
        assert_eq!(got["HOME"], "/home/user");
        assert_eq!(got["ACTINV_CACHE_DIR"], "/custom/actinv");
        assert_eq!(got["XDG_CACHE_HOME"], "/xdg/cache");
        assert_eq!(got["LOCALAPPDATA"], "C:\\Users\\user\\AppData\\Local");
        assert_eq!(got["ACTINV_DATA_DIR"], "/checked/data");
        assert!(!got.contains_key("ACTINV_LICENSE_KEY"));
        assert!(!got.contains_key("AWS_ACCESS_KEY_ID"));
    }

    #[test]
    fn cache_override_is_canonicalized_and_invalid_overrides_fail_clearly() {
        let temp = tempfile::tempdir().unwrap();
        let requested = temp.path().join("explicit-cache");
        let (environment, cache_root) = for_actinv(
            [(
                OsString::from("ACTINV_CACHE_DIR"),
                requested.clone().into_os_string(),
            )],
            None,
        )
        .unwrap();
        assert_eq!(cache_root, requested.canonicalize().unwrap());
        assert!(
            environment.iter().any(|(key, value)| {
                key == "ACTINV_CACHE_DIR" && value == cache_root.as_os_str()
            })
        );

        let error = for_actinv(
            [(OsString::from("ACTINV_CACHE_DIR"), "relative-cache".into())],
            None,
        )
        .unwrap_err();
        assert!(error.contains("must be an absolute path"));
    }

    #[test]
    fn explicit_cache_root_is_injected_without_changing_inherited_environment() {
        let temp = tempfile::tempdir().unwrap();
        let prepared = temp.path().join("prepared-cache");
        let inherited = vec![
            (OsString::from("HOME"), OsString::from("/home/user")),
            (
                OsString::from("XDG_CACHE_HOME"),
                OsString::from("/user/cache"),
            ),
            (
                OsString::from("ACTINV_CACHE_DIR"),
                OsString::from("/user/actinv"),
            ),
        ];
        let (environment, root) =
            for_actinv_with_cache_dir(inherited.clone(), None, Some(&prepared)).unwrap();
        assert_eq!(root, prepared.canonicalize().unwrap());
        let values: BTreeMap<_, _> = environment.into_iter().collect();
        assert_eq!(
            values[OsStr::new("ACTINV_CACHE_DIR")].as_os_str(),
            root.as_os_str()
        );
        assert_eq!(
            values[OsStr::new("HOME")].as_os_str(),
            OsStr::new("/home/user")
        );
        assert_eq!(
            values[OsStr::new("XDG_CACHE_HOME")].as_os_str(),
            OsStr::new("/user/cache")
        );
        assert_eq!(inherited[2].1, OsStr::new("/user/actinv"));
        assert!(for_actinv_with_cache_dir(inherited, None, Some(Path::new("relative"))).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn explicit_cache_root_symlink_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        let link = temp.path().join("link");
        std::fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let error = for_actinv(
            [(OsString::from("ACTINV_CACHE_DIR"), link.into_os_string())],
            None,
        )
        .unwrap_err();
        assert!(error.contains("real directory"));
    }
}
