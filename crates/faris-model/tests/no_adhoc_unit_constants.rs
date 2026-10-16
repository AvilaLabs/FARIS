//! A static lint over the calculation crates: no ad-hoc unit conversion constants outside the
//! unit registry (CFG-044). A conversion factor between units lives in `faris_model::units` and
//! is imported; a second copy in another crate could drift from the first.

use std::path::{Path, PathBuf};

/// Spellings of the factors the registry owns: seconds per day, hour and Julian year, the
/// electronvolt in joules, the curie in becquerel, the barn in square metres, MWd and GWd in joules.
const FORBIDDEN: [&str; 9] = [
    "86_400",
    "86400",
    "31_557_600",
    "365.25",
    "3600.0",
    "1.602176634",
    "3.7e10",
    "1e-28",
    "8.64e",
];

/// Lines that carry a number which is not a unit conversion, each with the reason.
const ALLOWED: [(&str, &str); 2] = [
    (
        "faris-engine/src/jobs.rs",
        "a job time limit of one day in seconds, not a conversion",
    ),
    (
        "faris-cli/src/control.rs",
        "the same limit as a command-line range",
    ),
];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The offending lines of one source file, up to its first `#[cfg(test)]`.
fn offenders(text: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for (number, line) in text.lines().enumerate() {
        if line.trim() == "#[cfg(test)]" {
            break;
        }
        let code = line.trim_start();
        if code.starts_with("//") {
            continue;
        }
        if FORBIDDEN.iter().any(|f| line.contains(f)) {
            found.push((number + 1, line.trim().to_string()));
        }
    }
    found
}

// Verifies: CFG-044
#[test]
fn no_unit_conversion_constants_outside_the_registry() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for name in [
        "faris-model",
        "faris-engine",
        "faris-cli",
        "faris-report",
        "faris-study",
    ] {
        sources(&crates.join(name).join("src"), &mut files);
    }
    assert!(files.len() > 40, "{} files scanned", files.len());
    let mut bad = Vec::new();
    for file in files {
        let relative = file
            .strip_prefix(&crates)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let name = file.file_name().unwrap().to_string_lossy();
        // The registry itself, and test files, may spell the values out to check them.
        if relative.ends_with("faris-model/src/units.rs")
            || name == "tests.rs"
            || relative.contains("/tests/")
            || ALLOWED.iter().any(|(path, _)| relative.ends_with(path))
        {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        for (line, code) in offenders(&text) {
            bad.push(format!("{relative}:{line}: {code}"));
        }
    }
    assert!(
        bad.is_empty(),
        "ad-hoc unit constants; import them from faris_model::units instead:\n{}",
        bad.join("\n")
    );
}

// Verifies: CFG-044
#[test]
fn the_lint_catches_a_planted_constant() {
    let planted = "fn f(s: f64) -> f64 { s / 86_400.0 }\n#[cfg(test)]\nconst X: f64 = 86_400.0;\n";
    assert_eq!(offenders(planted).len(), 1);
    assert!(offenders("// a day is 86400 s\nlet a = 1;\n").is_empty());
}
