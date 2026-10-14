//! Every FARIS parser that reads bytes or text from outside the program, as a function of bytes.
//!
//! The cargo-fuzz targets in `fuzz/fuzz_targets` call these, and so does the smoke driver and the
//! test that runs every seed through every entry. An entry must never panic, hang or allocate
//! without a bound whatever the bytes are; it ignores the parser's `Err`, which is the right answer
//! to a bad file.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use faris_engine::{
    case_archive, core_evidence, evidence_store, history, history_ensemble, jobs, maintenance,
    memory, reactor, transport,
};
use faris_model::{LoadedScenario, design as model_design};

/// A fuzz target: its name and the function that takes the bytes.
pub type Entry = (&'static str, fn(&[u8]));

/// Every target, in the order the scripts run them.
pub const ENTRIES: &[Entry] = &[
    ("scenario", scenario),
    ("design_file", design_file),
    ("history_assumptions", history_assumptions),
    ("facets", facets),
    ("step_inspection", step_inspection),
    ("transport_artifact", transport_artifact),
    ("json_records", json_records),
    ("helper_outputs", helper_outputs),
    ("proc_and_config_text", proc_and_config_text),
    ("nuclear_data_audit", nuclear_data_audit),
    ("actinv_result", actinv_result),
    ("png_header", png_header),
    ("file_readers", file_readers),
    ("study_file", study_file),
    ("tar_gz", tar_gz),
    ("evidence_store", evidence_store_index),
    ("saved_case", saved_case),
];

/// A scratch directory that lives as long as the process; each entry replaces what it wrote.
fn scratch() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("faris-fuzz-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch directory");
        dir
    })
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = scratch().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch subdirectory");
    dir
}

fn write_file(name: &str, bytes: &[u8]) -> PathBuf {
    let path = scratch().join(name);
    std::fs::write(&path, bytes).expect("scratch file");
    path
}

/// The scenario the file readers check against: the repository's own.
fn reference_scenario() -> &'static LoadedScenario {
    static SCENARIO: OnceLock<LoadedScenario> = OnceLock::new();
    SCENARIO.get_or_init(|| {
        LoadedScenario::from_bytes(include_bytes!(
            "../../../scenarios/arc-inspired/cold-reference.scenario.json"
        ))
        .expect("the bundled scenario loads")
    })
}

/// A scenario file (`faris-model`).
pub fn scenario(data: &[u8]) {
    if let Ok(loaded) = LoadedScenario::from_bytes(data) {
        let _ = loaded.validate_identity();
    }
}

/// A design file and the checks that run on what it parsed to.
pub fn design_file(data: &[u8]) {
    let outcome = model_design::parse_design(data);
    if let Some(design) = outcome.design {
        let facts = model_design::scenario_facts(data);
        let _ = model_design::check_2_static(&design, "0", Ok(&facts));
        let _ = model_design::check_2_static(&design, "0", Err("not read"));
        let _ = model_design::check_4(&design);
        let _ = model_design::check_5(&design);
        let _ = model_design::void_solids(&design);
        let _ = model_design::check_waivers(&design);
        let _ = model_design::waiver_references(&design);
    }
}

/// Operating-history assumptions, driving rates, and the designs-file and maintenance records.
pub fn history_assumptions(data: &[u8]) {
    let _ = model_design::scenario_facts(data);
    if let Ok(a) = serde_json::from_slice::<faris_model::history::OperatingHistoryAssumptions>(data)
    {
        let _ = a.validate();
    }
    let _ = serde_json::from_slice::<history::TransportDrivingRates>(data);
    if let Ok(a) = serde_json::from_slice::<faris_model::maintenance::MaintenanceAssumptions>(data)
    {
        let _ = a.validate();
    }
    let _ = maintenance::files::parse_designs(data, scratch());
}

/// The facets binary and its manifest. The first two bytes give the manifest's length.
pub fn facets(data: &[u8]) {
    let _ = model_design::parse_facets(data);
    if data.len() >= 2 {
        let n = (usize::from(data[0]) | usize::from(data[1]) << 8).min(data.len() - 2);
        let (manifest, binary) = data[2..].split_at(n);
        let _ = model_design::load_facets(binary, manifest);
    }
}

/// The CAD helper's inspection report.
pub fn step_inspection(data: &[u8]) {
    let _ = model_design::StepInspection::from_bytes(data);
}

/// A raw transport artifact and a transport request.
pub fn transport_artifact(data: &[u8]) {
    let _ = transport::TransportArtifact::from_bytes(data);
    if let Ok(request) = serde_json::from_slice::<faris_model::transport::TransportRequest>(data) {
        let _ = request.validate_against(reference_scenario());
    }
}

/// Recorded results read back from disk: one JSON record type chosen by the first byte.
pub fn json_records(data: &[u8]) {
    let Some((&kind, body)) = data.split_first() else {
        return;
    };
    match kind % 13 {
        0 => drop(serde_json::from_slice::<history::HistoryResult>(body)),
        1 => drop(serde_json::from_slice::<history_ensemble::HistoryEnsemble>(
            body,
        )),
        2 => drop(serde_json::from_slice::<maintenance::MaintenanceResult>(
            body,
        )),
        3 => drop(serde_json::from_slice::<reactor::ReactorRun>(body)),
        4 => {
            if let Ok(p) = serde_json::from_slice::<faris_model::physics::PhysicsCase>(body) {
                let _ = p.validate_against(reference_scenario());
            }
        }
        5 => drop(serde_json::from_slice::<transport::NormalizedTransportResult>(body)),
        6 => drop(serde_json::from_slice::<transport::RawTransportSpectra>(
            body,
        )),
        7 => {
            if let Ok(b) = serde_json::from_slice::<core_evidence::RecordedTransportBundle>(body) {
                let _ = b.validate();
            }
        }
        8 => drop(serde_json::from_slice::<core_evidence::CoreEvidenceRun>(
            body,
        )),
        9 => drop(serde_json::from_slice::<jobs::JobResult>(body)),
        10 => drop(serde_json::from_slice::<faris_engine::design::ImportReport>(body)),
        11 => drop(serde_json::from_slice::<
            faris_engine::design::MaterialAuditReport,
        >(body)),
        _ => drop(serde_json::from_slice::<
            faris_occt_client::protocol::Response,
        >(body)),
    }
}

/// What the design import reads back from the helpers and its own cache.
pub fn helper_outputs(data: &[u8]) {
    faris_engine::design::fuzz_helper_outputs(data);
}

/// `/proc` text, the CAD-reader setting and the mesh-preset name.
pub fn proc_and_config_text(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    let _ = memory::parse_meminfo_available(&text);
    #[cfg(target_os = "linux")]
    {
        let _ = memory::sampler::parse_stat_group(&text);
        let _ = memory::sampler::parse_status_kib(&text, "VmHWM:");
        let _ = memory::sampler::parse_status_kib(&text, "VmRSS:");
    }
    let _ = faris_engine::design::CadReader::from_label(text.trim());
    let _ = reactor::MeshPreset::from_cli(&text);
    let _ = faris_model::design::valid_id(&text);
    let _ = faris_model::design::is_reserved(&text);
    let _ = faris_model::design::suggest_id(Some(&text), data.len(), &[]);
}

/// The nuclear-data library audit that binds a case to its cross-section files. The audit's own
/// hash of `cross_sections.xml` is set to the real one so the reader gets past the first check.
pub fn nuclear_data_audit(data: &[u8]) {
    let Ok(mut audit) = serde_json::from_slice::<serde_json::Value>(data) else {
        return;
    };
    let dir = fresh_dir("library");
    let xml = dir.join("cross_sections.xml");
    std::fs::write(&xml, b"<cross_sections/>").expect("scratch file");
    if let (Some(slot), Ok(hash)) = (
        audit.pointer_mut("/cross_sections_xml/sha256"),
        reactor::hash_file(&xml),
    ) {
        *slot = serde_json::Value::String(hash);
    }
    static PHYSICS: OnceLock<faris_model::physics::PhysicsCase> = OnceLock::new();
    let physics = PHYSICS.get_or_init(|| {
        serde_json::from_slice(include_bytes!(
            "../../../scenarios/arc-inspired/cold-reference.reference.physics.json"
        ))
        .expect("the bundled physics case loads")
    });
    let _ = reactor::bind_audited_library(physics, reference_scenario(), &audit, &xml);
}

/// An ACTINV result file, read as a stream.
pub fn actinv_result(data: &[u8]) {
    let _ = maintenance::actinv::scan_result(Cursor::new(data));
}

/// The size of a PNG preview, read from its header.
pub fn png_header(data: &[u8]) {
    let _ = faris_study::png_dimensions(data);
}

/// Readers that take a path: the first byte picks one and the rest is the file.
pub fn file_readers(data: &[u8]) {
    let Some((&kind, body)) = data.split_first() else {
        return;
    };
    let path = write_file("input.bin", body);
    let scenario = reference_scenario();
    match kind % 8 {
        0 => drop(maintenance::actinv::read_points(&path)),
        1 => drop(faris_occt_client::protocol::read_points(&path)),
        2 => drop(reactor::load_physics_case(&path, scenario)),
        3 => drop(reactor::load_reactor_run(&path, scenario)),
        4 => drop(core_evidence::load_recorded_bundle(&path, scenario)),
        5 => drop(core_evidence::scenario_from_bundle_file(&path)),
        6 => drop(faris_study::descriptor_evidence(&path)),
        _ => drop(faris_study::evidence_from_descriptor(&path)),
    }
}

/// A `.faris` study file: open it, verify it, and unpack it.
pub fn study_file(data: &[u8]) {
    let path = write_file("input.faris", data);
    let Ok(mut reader) = faris_study::StudyReader::open(&path) else {
        return;
    };
    let _ = reader.preview_status();
    let _ = reader.preview();
    let _ = reader.blob_infos();
    let _ = reader.ensembles();
    let _ = reader.verify();
    let out = fresh_dir("study-out");
    let _ = reader.materialize(&out, None);
}

/// An evidence archive (`.tar.gz`) and a recorded tree's manifest.
pub fn tar_gz(data: &[u8]) {
    let archive = write_file("input.tar.gz", data);
    let out = scratch().join("tar-out");
    let _ = std::fs::remove_dir_all(&out);
    let _ = faris_study::extract_tar_gz(&archive, &out);
    // The same bytes as a manifest beside a (here empty) archive.
    let manifest = write_file("tree.json", data);
    let out = scratch().join("tree-out");
    let _ = std::fs::remove_dir_all(&out);
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let _ = faris_study::extract_recorded_tree(&archive, &manifest, &out, &cancel);
}

/// The index of an evidence store (`store.json`).
pub fn evidence_store_index(data: &[u8]) {
    let dir = fresh_dir("store");
    std::fs::write(dir.join("store.json"), data).expect("scratch file");
    if let Ok(store) = evidence_store::EvidenceStore::open(&dir) {
        let _ = store.verify();
        let _ = store.read_file("case", "package.json", 1 << 20);
    }
    let _ = evidence_store::verify_store(&dir);
}

/// Several files at once, written as `name\n content` blocks separated by a line `=====`:
/// the first block is the execution report, the rest go to the case tree. The workspace is empty.
fn blocks(data: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut rest = data;
    loop {
        let (block, next) = match find(rest, b"\n=====\n") {
            Some(at) => (&rest[..at], Some(&rest[at + 7..])),
            None => (rest, None),
        };
        let (name, body) = match block.iter().position(|b| *b == b'\n') {
            Some(at) => (&block[..at], &block[at + 1..]),
            None => (block, &block[..0]),
        };
        let name = String::from_utf8_lossy(name).to_string();
        let safe = !name.is_empty()
            && name.len() < 64
            && !name.starts_with('/')
            && !name
                .split('/')
                .any(|p| p.is_empty() || p == "." || p == "..");
        if safe && out.len() < 24 {
            out.push((name, body.to_vec()));
        }
        match next {
            Some(next) => rest = next,
            None => break,
        }
    }
    out
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// A saved Core case: its files, the execution report, and an empty workspace.
pub fn saved_case(data: &[u8]) {
    let case = fresh_dir("case");
    let workspace = fresh_dir("workspace");
    let files = blocks(data);
    for (name, body) in &files {
        let path = case.join(name);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, body);
    }
    let report = case.join("execution-report.json");
    let _ = case_archive::inspect_saved_case(&case, &report, &workspace);
}
