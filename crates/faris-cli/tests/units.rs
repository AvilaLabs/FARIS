//! Units, number formats and Unicode names, end to end through the command line.

use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn faris(config: &Path, arguments: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_faris"))
        .args(arguments)
        .env("FARIS_CONFIG_DIR", config)
        .env_remove("FARIS_UNITS")
        .output()
        .expect("run faris")
}

fn run(config: &Path, arguments: &[&str]) -> Output {
    let owned: Vec<&std::ffi::OsStr> = arguments.iter().map(std::ffi::OsStr::new).collect();
    faris(config, &owned)
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn bundle_file(dir: &Path, name: &str, tag: &str) -> PathBuf {
    let mut files = std::collections::BTreeMap::new();
    for member in [
        "run.json",
        "input.json",
        "scenario.json",
        "audit.json",
        "reactor_transport.py",
        "solver/transport-artifact.json",
    ] {
        files.insert(member.to_owned(), format!("{{\"{member}\":\"{tag}\"}}\n"));
    }
    let bundle = faris_engine::core_evidence::RecordedTransportBundle {
        schema_version: "faris-recorded-transport-bundle/v0.1".into(),
        files,
        notice: "test".into(),
    };
    let path = dir.join(format!("{name}.transport-bundle.json"));
    std::fs::write(&path, serde_json::to_vec_pretty(&bundle).unwrap()).unwrap();
    path
}

// Verifies: L10N-034
#[test]
fn unicode_names_and_paths_survive_a_study_file_round_trip_unchanged() {
    let config = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    // A folder and file names with accents, CJK, a symbol, a space; one bundle name in NFC and
    // one in NFD (the same visible text, different code points) so a silent normalisation shows.
    let dir = root.path().join("\u{e9}tude \u{2622} \u{7814}\u{7a76}");
    std::fs::create_dir(&dir).unwrap();
    let nfc = "r\u{e9}f\u{e9}rence";
    let nfd = "re\u{301}fe\u{301}rence-nfd";
    let cjk = "\u{30d6}\u{30e9}\u{30f3}\u{30b1}\u{30c3}\u{30c8}-\u{5c64}";
    let bundles = [
        bundle_file(&dir, nfc, "a"),
        bundle_file(&dir, nfd, "b"),
        bundle_file(&dir, cjk, "c"),
    ];
    let study = dir.join("\u{e9}tude-\u{65e5}\u{672c}\u{8a9e}.faris");
    let mut args: Vec<&std::ffi::OsStr> = vec!["study-file".as_ref(), "create".as_ref()];
    for bundle in &bundles[..2] {
        args.push("--bundle".as_ref());
        args.push(bundle.as_os_str());
    }
    args.push("--sweep-bundle".as_ref());
    args.push(bundles[2].as_os_str());
    args.push("-o".as_ref());
    args.push(study.as_os_str());
    let created = faris(config.path(), &args);
    assert_eq!(created.status.code(), Some(0), "{}", text(&created));
    // The command-line output carries the path exactly.
    let reported: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    assert_eq!(reported["file"], study.to_str().unwrap());

    let inspected = faris(
        config.path(),
        &["study-file".as_ref(), "inspect".as_ref(), study.as_os_str()],
    );
    assert_eq!(inspected.status.code(), Some(0), "{}", text(&inspected));
    let summary: serde_json::Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(summary["file"], study.to_str().unwrap());
    let port: Vec<&str> = summary["arrangements"]["port"]["bundles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect();
    assert!(port.contains(&nfc), "{port:?}");
    assert!(port.contains(&nfd), "{port:?}");
    assert_ne!(nfc.as_bytes(), nfd.as_bytes());
    assert_eq!(summary["sweep"][0], cjk);

    let verified = faris(
        config.path(),
        &["study-file".as_ref(), "verify".as_ref(), study.as_os_str()],
    );
    assert_eq!(verified.status.code(), Some(0), "{}", text(&verified));

    let out = dir.join("\u{fc}nz\u{f6}\u{df}-entpackt");
    let unpacked = faris(
        config.path(),
        &[
            "study-file".as_ref(),
            "unpack".as_ref(),
            study.as_os_str(),
            out.as_os_str(),
        ],
    );
    assert_eq!(unpacked.status.code(), Some(0), "{}", text(&unpacked));
    let names: Vec<String> = std::fs::read_dir(out.join("port/bundles"))
        .unwrap()
        .chain(std::fs::read_dir(out.join("sweep")).unwrap())
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    for name in [nfc, nfd, cjk] {
        let want = format!("{name}.transport-bundle.json");
        assert!(names.contains(&want), "{want} not in {names:?}");
    }
    // The unpacked bundle holds the bytes that went in.
    for (name, original) in [(nfc, &bundles[0]), (nfd, &bundles[1])] {
        let written =
            std::fs::read(out.join(format!("port/bundles/{name}.transport-bundle.json"))).unwrap();
        assert_eq!(written, std::fs::read(original).unwrap());
    }
}

// Verifies: UX-066, CFG-040, CFG-037
#[test]
fn the_unit_and_format_settings_are_listed_and_stored_by_symbol() {
    let config = tempfile::tempdir().unwrap();
    let listed = run(config.path(), &["config", "list"]);
    assert_eq!(listed.status.code(), Some(0), "{}", text(&listed));
    let out = text(&listed);
    for line in [
        "units.system",
        "units.energy_particle",
        "units.heat_flux",
        "format.decimal",
        "format.date",
    ] {
        assert!(out.contains(line), "{line} in {out}");
    }
    // The default is SI and plain numbers.
    let system = run(config.path(), &["config", "get", "units.system"]);
    assert_eq!(text(&system).trim(), "si");
    // An alias is accepted and stored under its symbol.
    let set = run(
        config.path(),
        &["config", "set", "units.heat_flux", "MW/m\u{b2}"],
    );
    assert_eq!(set.status.code(), Some(0), "{}", text(&set));
    let got = run(config.path(), &["config", "get", "units.heat_flux"]);
    assert_eq!(text(&got).trim(), "MW/m2");
    // The settings file holds the unit, not a number.
    let file = std::fs::read_to_string(config.path().join("settings.json")).unwrap();
    assert!(file.contains("\"units.heat_flux\": \"MW/m2\""), "{file}");
    // A unit of the wrong quantity is refused with the cause and the accepted ones.
    let wrong = run(config.path(), &["config", "set", "units.heat_flux", "keV"]);
    assert_eq!(wrong.status.code(), Some(2));
    let message = text(&wrong);
    assert!(
        message.contains("keV") && message.contains("W/cm2"),
        "{message}"
    );
}
