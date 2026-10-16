//! Source scan for process starts and dynamic loading (SEC-001). It fails when
//! a call that starts a process or loads a library appears anywhere in the
//! workspace that is not in the reviewed list below, or when a reviewed file
//! gains another such call. Each entry says where the program and the
//! arguments come from; a new entry needs the same review.
//!
//! This is a scan of source text, not a proof: it finds the calls this
//! codebase would use (`Command::new`, the job runner, the Python launcher) and
//! the names of libraries and openers it must not use. Test code is skipped
//! (files under `tests/`, `tests.rs`, `*_tests.rs`, and inline `mod tests`
//! blocks), because test code is not shipped.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// Text that starts a process, hands a program to the job runner, or loads code.
const TOKENS: [&str; 17] = [
    "Command::new",
    "CommandExt",
    "libloading",
    "dlopen",
    "dlsym",
    "xdg-open",
    "execv",
    "posix_spawn",
    "webbrowser",
    "open::that",
    "opener::",
    "fork(",
    "run_job(",
    "run_job_with_lines(",
    "launch_python(",
    "launch_python_with_lines(",
    "Worker::new(",
];

/// (file under `crates/`, token, count, origin, note). Origin says where the program and the
/// arguments come from: F = fixed in code, S = user settings (tool paths and command-line
/// options), - = not a start (a definition or a forwarding call). C, the content of an opened
/// file, is never allowed: `no_reviewed_entry_takes_anything_from_file_content` fails on it.
#[rustfmt::skip]
const REVIEWED: &[(&str, &str, usize, &str, &str)] = &[
    // The process spawn of the job runner, one per platform: Linux runs `prlimit` at a fixed path,
    // macOS runs `/bin/sh` (which sets `ulimit -f` and execs the job program), Windows starts the
    // job program itself and assigns it to a job object.
    ("faris-engine/src/jobs.rs", "Command::new", 3, "FS", "program is an absolute tool path (S); arguments F or S"),
    ("faris-engine/src/jobs.rs", "CommandExt", 1, "-", "process_group for the Unix spawns above"),
    ("faris-engine/src/jobs.rs", "run_job(", 1, "-", "definition"),
    ("faris-engine/src/jobs.rs", "run_job_with_lines(", 3, "-", "definition, forwarding call, stub for targets without processes"),
    // Callers of the job runner.
    ("faris-engine/src/core_evidence.rs", "run_job(", 1, "FS", "Core executable S; arguments F plus package paths S"),
    ("faris-engine/src/study.rs", "run_job(", 1, "FS", "Core executable S; arguments F"),
    ("faris-engine/src/reactor.rs", "run_job(", 1, "FS", "Python S; worker script embedded F; paths S"),
    ("faris-engine/src/reactor/windows.rs", "run_job(", 1, "FS", "window Python S (--window-python or the Tools setting); helper script embedded F; paths S"),
    ("faris-engine/src/maintenance/actinv.rs", "run_job(", 1, "FS", "Python and ACTINV S; component ids from file text go in as one --component=ID token"),
    ("faris-engine/src/activation.rs", "run_job(", 1, "FS", "ACTINV S (--actinv); arguments F (run) plus problem and result paths in a scratch folder named by a hash; zone ids from file text never reach an argument"),
    ("faris-engine/src/design.rs", "run_job_with_lines(", 1, "-", "the Python launcher"),
    ("faris-engine/src/design.rs", "launch_python(", 2, "-", "definition and forwarding call"),
    ("faris-engine/src/design.rs", "launch_python_with_lines(", 2, "-", "definition and forwarding call"),
    ("faris-engine/src/design/external.rs", "launch_python(", 5, "FS", "interpreter S; helper scripts embedded F; STEP path made absolute"),
    ("faris-engine/src/design/transport.rs", "launch_python_with_lines(", 1, "FS", "interpreter S; script embedded F; model path validated by converted_model"),
    ("faris-cli/src/adapters.rs", "run_job(", 1, "FS", "faris adapters list: program is a fixed tool name (openmc, actinv, avila-core) found on PATH; the only argument is `--version`; five-second bound"),
    ("faris-cli/src/control.rs", "run_job(", 2, "FS", "Python S; script embedded F (Linux and other targets)"),
    // Other spawns.
    ("faris-occt-client/src/client.rs", "Command::new", 1, "FS", "worker program S (FARIS_OCCT_WORKER or beside the executable); two scratch paths F"),
    ("faris-app/src/tool_finder.rs", "run_job(", 1, "S", "Find tools: the program is a Python interpreter found on PATH, in conda/mamba environment folders, ~/.venvs or a search folder the user added; arguments are fixed (`-I -c` and the embedded probe); bounded time, memory and output"),
    ("faris-app/src/export_panel.rs", "Command::new", 1, "FS", "xdg-open F; one argument, the export folder FARIS just made under a user-chosen parent, name sanitised"),
    ("faris-app/src/export_panel.rs", "xdg-open", 1, "F", "same call"),
    // Build script, runs at compile time on the builder's machine.
    ("faris-occt/build.rs", "Command::new", 6, "F", "curl, tar, cmake, ninja; URLs and versions pinned in the script"),
];

fn skipped(path: &Path) -> bool {
    let name = path.file_name().unwrap().to_string_lossy();
    path.components().any(|c| c.as_os_str() == "tests")
        || name == "tests.rs"
        || name.ends_with("_tests.rs")
        || name == "testutil.rs"
        || name == "fixtures.rs"
        || path.components().any(|c| c.as_os_str() == "target")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") && !skipped(&path) {
            out.push(path);
        }
    }
}

/// The text of the file without comment lines and without an inline test module
/// (a `#[cfg(test)]` or `#[cfg(all(test, ...))]` attribute directly above `mod`).
fn shipped_lines(text: &str) -> Vec<&str> {
    let lines: Vec<&str> = text.lines().collect();
    let mut keep = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let trimmed = lines[index].trim_start();
        if trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("#[cfg(all(test") {
            let next = lines[index + 1..]
                .iter()
                .map(|l| l.trim_start())
                .find(|l| !l.starts_with("#["))
                .unwrap_or("");
            let inline_module = (next.starts_with("mod ") || next.starts_with("pub(crate) mod "))
                && next.trim_end().ends_with('{');
            if inline_module {
                break;
            }
        }
        if !trimmed.starts_with("//") {
            keep.push(lines[index]);
        }
        index += 1;
    }
    keep
}

fn scan() -> BTreeMap<(String, String), usize> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut files = Vec::new();
    rust_files(crates, &mut files);
    assert!(
        files.len() > 50,
        "the scan found only {} files",
        files.len()
    );
    let mut found = BTreeMap::new();
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        let relative = file
            .strip_prefix(crates)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        for line in shipped_lines(&text) {
            for token in TOKENS {
                let count = line.matches(token).count();
                if count > 0 {
                    *found
                        .entry((relative.clone(), token.to_string()))
                        .or_insert(0) += count;
                }
            }
        }
    }
    found
}

// Verifies: SEC-001
#[test]
fn every_process_start_and_dynamic_load_in_the_workspace_is_on_the_reviewed_list() {
    let found = scan();
    let reviewed: BTreeMap<(String, String), usize> = REVIEWED
        .iter()
        .map(|(file, token, count, _, _)| ((file.to_string(), token.to_string()), *count))
        .collect();
    let mut problems = Vec::new();
    for (key, count) in &found {
        match reviewed.get(key) {
            None => problems.push(format!(
                "{}: {} x {:?} is not on the reviewed list; review where its program and \
                 arguments come from (never from file content), then add it to REVIEWED",
                key.0, count, key.1
            )),
            Some(expected) if expected != count => problems.push(format!(
                "{}: {:?} appears {count} times, {expected} were reviewed; review the new use",
                key.0, key.1
            )),
            Some(_) => {}
        }
    }
    for (key, expected) in &reviewed {
        if !found.contains_key(key) {
            problems.push(format!(
                "{}: reviewed {:?} x {expected} no longer appears; remove it from REVIEWED",
                key.0, key.1
            ));
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

// Verifies: SEC-001
#[test]
fn no_reviewed_entry_takes_anything_from_file_content() {
    for (file, token, _, origin, note) in REVIEWED {
        assert!(
            !origin.is_empty() && origin.chars().all(|c| matches!(c, 'F' | 'S' | '-')),
            "{file} {token}: origin {origin:?} ({note}) must be F, S or -; file content is never allowed"
        );
    }
}

// Verifies: SEC-001
#[test]
fn the_workspace_depends_on_no_dynamic_loader_or_opener_crate() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for entry in fs::read_dir(crates).unwrap().flatten() {
        let manifest = entry.path().join("Cargo.toml");
        let Ok(text) = fs::read_to_string(&manifest) else {
            continue;
        };
        for name in [
            "libloading",
            "dlopen",
            "open ",
            "opener",
            "webbrowser",
            "shell",
        ] {
            for line in text.lines().filter(|l| !l.trim_start().starts_with('#')) {
                let dependency = line.trim_start();
                assert!(
                    !(dependency.starts_with(name) && dependency.contains('=')),
                    "{} depends on {name}: {line}",
                    manifest.display()
                );
            }
        }
    }
}

#[test]
fn the_scanner_sees_a_new_spawn_in_a_synthetic_file() {
    let lines = shipped_lines(
        "fn a() {\n    // Command::new(\"x\");\n    std::process::Command::new(\"y\");\n}\n#[cfg(test)]\nmod tests {\n    Command::new(\"z\");\n}\n",
    );
    let hits: usize = lines
        .iter()
        .map(|l| l.matches("Command::new").count())
        .sum();
    assert_eq!(
        hits, 1,
        "one shipped call, none in comments or the test module"
    );
}
