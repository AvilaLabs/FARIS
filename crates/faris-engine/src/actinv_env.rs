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

#[cfg(test)]
mod tests {
    use super::{allowed_name, job_environment};
    use crate::jobs::{JobSpec, ResourceLimits};
    use std::collections::BTreeMap;
    use std::ffi::{OsStr, OsString};
    use std::path::PathBuf;
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
}
