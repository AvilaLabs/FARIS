//! Archived study-file fixtures and the published schemas (`docs/schemas/`).
//!
//! The fixtures under `tests/fixtures/` are files written once by the writer of
//! the release named in `FIXTURES.md` and then frozen: every later version must
//! keep opening them, and `scripts/read_study_file.py` (which shares no code
//! with this crate) must agree with the summary committed beside each one.
//! `regenerate_archived_fixtures` rewrites them and is run by hand, only when a
//! release adds a file format version or a layer to the set.

use crate::tests::ensembles::{ensemble_draft, stored};
use crate::tests::{
    draft, evidence_draft, fake_png, raw_zip, write_bundle, write_bundle_with_batch_values,
};
use crate::*;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    io::Read,
    path::{Path, PathBuf},
};

const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
const SCHEMA_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/schemas");

fn schema(name: &str) -> Value {
    let text = std::fs::read_to_string(Path::new(SCHEMA_DIR).join(name)).unwrap();
    serde_json::from_str(&text).unwrap()
}

// ---------------------------------------------------------------------------
// A small JSON Schema checker: the keywords the published schemas use, except
// `pattern` and `format` (no regular-expression engine here handles the
// look-ahead the name patterns use; scripts/test_read_study_file.py checks the
// patterns). Returns one sentence per violation.

fn check(schema: &Value, root: &Value, value: &Value, at: &str, out: &mut Vec<String>) {
    if let Some(target) = schema.get("$ref").and_then(Value::as_str) {
        let mut node = root;
        for part in target.trim_start_matches("#/").split('/') {
            node = &node[part];
        }
        return check(node, root, value, at, out);
    }
    let type_ok = |name: &str| match name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        other => panic!("schema type {other} not supported by the test checker"),
    };
    match schema.get("type") {
        Some(Value::String(name)) if !type_ok(name) => out.push(format!("{at}: not a {name}")),
        Some(Value::Array(names)) if !names.iter().any(|n| type_ok(n.as_str().unwrap())) => {
            out.push(format!("{at}: not one of {names:?}"))
        }
        _ => {}
    }
    if let Some(expected) = schema.get("const")
        && expected != value
    {
        out.push(format!("{at}: not {expected}"));
    }
    if let Some(Value::Array(options)) = schema.get("enum")
        && !options.contains(value)
    {
        out.push(format!("{at}: {value} is not one of {options:?}"));
    }
    if let (Some(min), Some(v)) = (
        schema.get("minimum").and_then(Value::as_f64),
        value.as_f64(),
    ) && v < min
    {
        out.push(format!("{at}: below {min}"));
    }
    if let Some(Value::Array(options)) = schema.get("oneOf") {
        let matching = options
            .iter()
            .filter(|option| {
                let mut inner = Vec::new();
                check(option, root, value, at, &mut inner);
                inner.is_empty()
            })
            .count();
        if matching != 1 {
            out.push(format!("{at}: matches {matching} of the oneOf options"));
        }
    }
    if let Some(inner) = schema.get("not") {
        let mut found = Vec::new();
        check(inner, root, value, at, &mut found);
        if found.is_empty() {
            out.push(format!("{at}: matches a schema it must not match"));
        }
    }
    if let Some(object) = value.as_object() {
        for name in schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if !object.contains_key(name.as_str().unwrap()) {
                out.push(format!("{at}: missing {name}"));
            }
        }
        let properties = schema.get("properties").and_then(Value::as_object);
        for (key, child) in object {
            let path = format!("{at}/{key}");
            if let Some(sub) = properties.and_then(|p| p.get(key)) {
                check(sub, root, child, &path, out);
            } else {
                match schema.get("additionalProperties") {
                    Some(Value::Bool(false)) => out.push(format!("{path}: unknown property")),
                    Some(sub @ Value::Object(_)) => check(sub, root, child, &path, out),
                    _ => {}
                }
            }
        }
    }
    if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
        for (i, child) in array.iter().enumerate() {
            check(items, root, child, &format!("{at}/{i}"), out);
        }
    }
}

fn violations(schema_name: &str, value: &Value) -> Vec<String> {
    let schema = schema(schema_name);
    let mut out = Vec::new();
    check(&schema, &schema, value, "", &mut out);
    out
}

// ---------------------------------------------------------------------------
// The fixture set

fn raw_manifest(format: &str, extra: Value) -> Vec<(String, Vec<u8>)> {
    let content = b"{\"x\":1}\n".to_vec();
    let sha = sha256_hex(&content);
    let mut manifest = json!({
        "format": format,
        "assumptions": sha,
        "blobs": [{"sha256": sha, "bytes": content.len(), "media_type": "application/json", "encoding": "verbatim"}],
    });
    manifest
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    vec![
        ("mimetype".into(), MIMETYPE.as_bytes().to_vec()),
        (
            "manifest.json".into(),
            serde_json::to_vec(&manifest).unwrap(),
        ),
        (format!("blobs/{sha}"), content),
    ]
}

/// Writes the whole set into `out`; returns the file names, in order.
fn build_set(out: &Path) -> Vec<String> {
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    let mut names = Vec::new();
    let mut put = |name: &str, build: &mut dyn FnMut(&Path)| {
        build(&out.join(format!("{name}.faris")));
        names.push(name.to_owned());
    };
    let write = |draft: &StudyDraft, path: &Path| {
        write_study(path, draft).unwrap();
    };
    let single = |bundles: Vec<PathBuf>| StudyDraft {
        port: Some(ArrangementDraft {
            scenario: None,
            physics: vec![],
            bundles,
        }),
        zstd_level: 3,
        ..StudyDraft::default()
    };

    // Hand-built containers: stored entries, a minimal manifest, then the
    // forward-compatibility shapes a newer minor version may write.
    put("01-hand-built-minimal", &mut |p| {
        std::fs::write(p, raw_zip(&raw_manifest("faris-study/1", json!({})))).unwrap()
    });
    put("02-newer-minor-unknown-fields", &mut |p| {
        let extra = json!({"future_field": {"ignored": true}, "created_by": "faris 1.9.0", "view": {"step": "design", "future_view_field": 3}});
        std::fs::write(p, raw_zip(&raw_manifest("faris-study/1.4", extra))).unwrap()
    });
    put("03-minor-1-1-with-store-layer-referenced", &mut |p| {
        let extra = json!({"layers": {"evidence": null, "evidence_store": {"mode": "referenced", "trees": []}}});
        std::fs::write(p, raw_zip(&raw_manifest("faris-study/1.1", extra))).unwrap()
    });
    put("04-empty-study", &mut |p| write(&single(vec![]), p));
    put("05-assumptions-only", &mut |p| {
        std::fs::write(w.join("a.json"), b"{\"a\":1}\n").unwrap();
        write(
            &StudyDraft {
                assumptions: Some(w.join("a.json")),
                zstd_level: 3,
                ..StudyDraft::default()
            },
            p,
        )
    });
    put("06-view-only", &mut |p| {
        let mut d = single(vec![]);
        d.view = ViewState {
            step: "history".into(),
            year: 7.25,
            sweep_blanket_m: Some(0.4),
            ..ViewState::default()
        };
        write(&d, p)
    });
    put("07-port-one-bundle", &mut |p| {
        write(&single(vec![write_bundle(w, "one", "p1")]), p)
    });
    put("08-port-two-bundles-sharing-blobs", &mut |p| {
        write(
            &single(vec![write_bundle(w, "a", "p1"), write_bundle(w, "b", "p2")]),
            p,
        )
    });
    put("09-control-only", &mut |p| {
        let mut d = single(vec![]);
        d.port = None;
        d.control = Some(ArrangementDraft {
            bundles: vec![write_bundle(w, "c", "c1")],
            ..ArrangementDraft::default()
        });
        write(&d, p)
    });
    put("10-both-arrangements-and-sweep-and-assumptions", &mut |p| {
        write(&draft(w), p)
    });
    put("11-sweep-only-with-trailing-newline", &mut |p| {
        let mut d = single(vec![]);
        d.port = None;
        d.sweep = vec![write_bundle(w, "s", "s1")];
        write(&d, p)
    });
    put("12-bundle-with-batch-values", &mut |p| {
        write(
            &single(vec![write_bundle_with_batch_values(w, "bv", "p1")]),
            p,
        )
    });
    put("13-scenario-and-physics-blobs", &mut |p| {
        std::fs::write(w.join("scenario.json"), b"{\"scenario\":1}\n").unwrap();
        std::fs::write(w.join("physics.json"), b"{\"physics\":1}\n").unwrap();
        let mut d = single(vec![]);
        d.port = Some(ArrangementDraft {
            scenario: Some(w.join("scenario.json")),
            physics: vec![w.join("physics.json")],
            bundles: vec![],
        });
        write(&d, p)
    });
    put("14-zstd-level-1", &mut |p| {
        let mut d = draft(w);
        d.zstd_level = 1;
        write(&d, p)
    });
    put("15-zstd-level-19", &mut |p| {
        let mut d = draft(w);
        d.zstd_level = 19;
        write(&d, p)
    });
    put("16-preview-thumbnail", &mut |p| {
        let mut d = draft(w);
        d.preview_png = Some(fake_png(32, 24, 300));
        write(&d, p)
    });
    put("17-what-if-assumptions-in-view", &mut |p| {
        let mut d = draft(w);
        d.view.what_if = Some(faris_engine::fixtures::assumptions());
        write(&d, p)
    });
    put("18-evidence-archives-referenced", &mut |p| {
        write(&evidence_draft(w, false).0, p)
    });
    put("19-evidence-archives-packed", &mut |p| {
        write(&evidence_draft(w, true).0, p)
    });
    let store = crate::store_tests::make_store(w, "evidence-store");
    put("20-evidence-store-referenced", &mut |p| {
        let mut d = draft(w);
        d.evidence_store = crate::store_tests::drafts(&store);
        write(&d, p)
    });
    put("21-evidence-store-packed", &mut |p| {
        let mut d = draft(w);
        d.evidence_store = crate::store_tests::drafts(&store);
        d.pack_evidence = true;
        write(&d, p)
    });
    put("22-history-ensembles", &mut |p| {
        let (ka, ea) = stored(6, 'a');
        let (kb, eb) = stored(6, 'b');
        let mut d = draft(w);
        d.ensembles = vec![
            ensemble_draft("reference", ka, ea),
            ensemble_draft("breeder-emphasis", kb, eb),
        ];
        write(&d, p)
    });
    put("23-ensemble-not-evaluated", &mut |p| {
        use faris_engine::history_ensemble::EnsembleStatus;
        let (k, mut e) = stored(4, 'a');
        e.status = EnsembleStatus::NotEvaluated {
            why: "no covariance".into(),
            next_step: "rerun transport".into(),
        };
        e.samples.clear();
        e.nominal = None;
        e.summary = None;
        let mut d = draft(w);
        d.ensembles = vec![ensemble_draft("reference", k, e)];
        write(&d, p)
    });
    put("24-everything-but-store-evidence", &mut |p| {
        let (k, e) = stored(5, 'a');
        let mut d = evidence_draft(w, true).0;
        d.preview_png = Some(fake_png(16, 16, 100));
        d.ensembles = vec![ensemble_draft("reference", k, e)];
        d.view.what_if = Some(faris_engine::fixtures::assumptions());
        write(&d, p)
    });
    names
}

/// Zip entry names of a study file, sorted.
fn entry_names(path: &Path) -> Vec<String> {
    let archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut names: Vec<String> = archive.file_names().map(str::to_owned).collect();
    names.sort();
    names
}

/// What the independent reader must reproduce (`faris-study-summary-1.schema.json`).
fn summary(path: &Path) -> Value {
    let mut reader = StudyReader::open(path).unwrap();
    reader.verify().unwrap();
    let m = &reader.manifest;
    let mut records = m.sweep.len() + m.ensembles.len();
    for role in [&m.arrangements.port, &m.arrangements.control]
        .into_iter()
        .flatten()
    {
        records += role.bundles.len();
    }
    let layers = &m.layers;
    let mode = |mode: EvidenceMode| match mode {
        EvidenceMode::Packed => "packed",
        EvidenceMode::Referenced => "referenced",
    };
    let evidence_mode = match (&layers.evidence, &layers.evidence_store) {
        (Some(l), _) => {
            records += l.archives.len();
            format!("archives-{}", mode(l.mode))
        }
        (None, Some(l)) => {
            records += l.trees.len();
            format!("store-{}", mode(l.mode))
        }
        (None, None) => "none".into(),
    };
    let mut blobs: Vec<_> = m.blobs.iter().map(|b| b.sha256.clone()).collect();
    blobs.sort();
    let entries = entry_names(path);
    json!({
        "format": m.format,
        "created_by": m.created_by,
        "blob_count": blobs.len(),
        "blob_sha256": blobs,
        "has_preview": entries.iter().any(|n| n == "preview.png"),
        "entries": entries,
        "record_count": records,
        "evidence_mode": evidence_mode,
    })
}

fn fixtures() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(FIXTURE_DIR)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "faris"))
        .collect();
    found.sort();
    found
}

/// Run by hand: `cargo test -p faris-study regenerate_archived_fixtures -- --ignored`.
#[test]
#[ignore = "rewrites the frozen fixtures; run only when a release adds a format version or layer"]
fn regenerate_archived_fixtures() {
    let dir = Path::new(FIXTURE_DIR);
    std::fs::create_dir_all(dir).unwrap();
    for old in std::fs::read_dir(dir).unwrap() {
        let path = old.unwrap().path();
        if path
            .extension()
            .is_some_and(|e| e == "faris" || e == "json")
            && path.file_name().is_some_and(|n| n != "FIXTURES.md")
        {
            std::fs::remove_file(path).unwrap();
        }
    }
    for name in build_set(dir) {
        let file = dir.join(format!("{name}.faris"));
        let mut text = serde_json::to_string_pretty(&summary(&file)).unwrap();
        text.push('\n');
        std::fs::write(dir.join(format!("{name}.summary.json")), text).unwrap();
    }
}

// Verifies: PRV-053, DOC-042, INT-001
#[test]
fn every_archived_fixture_opens_verifies_and_matches_its_summary() {
    let files = fixtures();
    assert!(
        files.len() >= 20,
        "{} fixtures; the set must hold at least 20",
        files.len()
    );
    let mut formats = BTreeSet::new();
    for file in &files {
        let committed = std::fs::read_to_string(file.with_extension("summary.json"))
            .unwrap_or_else(|e| panic!("{}: no committed summary: {e}", file.display()));
        let committed: Value = serde_json::from_str(&committed).unwrap();
        let now = summary(file);
        assert_eq!(
            now,
            committed,
            "{} disagrees with its summary",
            file.display()
        );
        formats.insert(now["format"].as_str().unwrap().to_owned());
        // The file's size stays well inside the repository rule.
        assert!(std::fs::metadata(file).unwrap().len() < 1_000_000);
    }
    // One major version so far; a new major is added here with its fixtures.
    assert!(formats.iter().all(|f| f.starts_with("faris-study/1")));
    assert!(formats.contains("faris-study/1"));
    assert!(formats.contains("faris-study/1.4"));
}

#[test]
fn the_generator_and_the_committed_set_name_the_same_files() {
    let scratch = tempfile::tempdir().unwrap();
    let generated: BTreeSet<_> = build_set(scratch.path()).into_iter().collect();
    let committed: BTreeSet<_> = fixtures()
        .iter()
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(generated, committed);
}

// ---------------------------------------------------------------------------
// Schemas

fn manifest_value(path: &Path) -> Value {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut text = Vec::new();
    archive
        .by_name("manifest.json")
        .unwrap()
        .read_to_end(&mut text)
        .unwrap();
    serde_json::from_slice(&text).unwrap()
}

// Verifies: INT-001, INT-003, INT-041
#[test]
fn documents_written_by_the_writer_validate_against_the_published_schemas() {
    let mut ensembles = 0;
    let mut stores = 0;
    for file in fixtures() {
        let manifest = manifest_value(&file);
        let found = violations("faris-study-manifest-1.schema.json", &manifest);
        assert!(found.is_empty(), "{}: {found:?}", file.display());
        let found = violations("faris-study-summary-1.schema.json", &summary(&file));
        assert!(found.is_empty(), "{}: {found:?}", file.display());
        let mut reader = StudyReader::open(&file).unwrap();
        for record in reader.manifest.ensembles.clone() {
            let blob: Value =
                serde_json::from_slice(&reader.read_blob(&record.blob).unwrap()).unwrap();
            let found = violations("faris-history-ensemble-1.schema.json", &blob);
            assert!(found.is_empty(), "{}: {found:?}", file.display());
            ensembles += 1;
        }
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&file).unwrap()).unwrap();
        if let Ok(mut index) = archive.by_name("evidence-store/store.json") {
            let mut text = Vec::new();
            index.read_to_end(&mut text).unwrap();
            let found = violations(
                "faris-evidence-store-index-1.schema.json",
                &serde_json::from_slice(&text).unwrap(),
            );
            assert!(found.is_empty(), "{}: {found:?}", file.display());
            stores += 1;
        }
    }
    assert!(
        ensembles >= 4 && stores >= 1,
        "{ensembles} ensembles, {stores} stores"
    );
}

#[test]
fn the_schemas_reject_documents_the_reader_would_refuse() {
    let good = manifest_value(
        &Path::new(FIXTURE_DIR).join("10-both-arrangements-and-sweep-and-assumptions.faris"),
    );
    let name = "faris-study-manifest-1.schema.json";
    assert!(violations(name, &good).is_empty());
    let broken = |change: &dyn Fn(&mut Value)| {
        let mut value = good.clone();
        change(&mut value);
        assert!(
            !violations(name, &value).is_empty(),
            "accepted: {value:.200}"
        );
    };
    broken(&|v| v["blobs"][0]["encoding"] = json!("array-f64-le"));
    broken(&|v| v["blobs"][0]["bytes"] = json!(-1));
    broken(&|v| v["blobs"] = json!("none"));
    broken(&|v| {
        v.as_object_mut().unwrap().remove("format");
    });
    broken(&|v| {
        v["layers"]["evidence"] = json!({"mode": "packed", "archives": []});
        v["layers"]["evidence_store"] = json!({"mode": "packed", "trees": []});
    });
    broken(&|v| v["view"]["year"] = json!("soon"));
}

/// The schema lists exactly the properties the writer's types emit: a field
/// added to a record without the schema changing fails here.
#[test]
fn the_manifest_schema_names_every_field_the_types_write() {
    let schema = schema("faris-study-manifest-1.schema.json");
    let props = |node: &Value| -> BTreeSet<String> {
        node["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    };
    let keys = |value: &Value| -> BTreeSet<String> {
        value.as_object().unwrap().keys().cloned().collect()
    };
    let defs = &schema["$defs"];
    let sha = "a".repeat(64);
    let archive = EvidenceArchive {
        arrangement: "port".into(),
        allocation: "reference".into(),
        kind: ArchiveKind::Case,
        file_name: "x".into(),
        sha256: sha.clone(),
        bytes: 1,
    };
    assert_eq!(
        props(&defs["evidenceLayer"]["properties"]["archives"]["items"]),
        keys(&serde_json::to_value(&archive).unwrap())
    );
    let blob = BlobRecord {
        sha256: sha.clone(),
        bytes: 1,
        media_type: "x".into(),
        encoding: "verbatim".into(),
    };
    assert_eq!(
        props(&defs["blob"]),
        keys(&serde_json::to_value(&blob).unwrap())
    );
    let bundle = BundleRecord {
        name: "x".into(),
        schema_version: "x".into(),
        notice: "x".into(),
        files: Default::default(),
        trailing_newline: false,
    };
    assert_eq!(
        props(&defs["bundle"]),
        keys(&serde_json::to_value(&bundle).unwrap())
    );
    assert_eq!(
        props(&defs["view"]),
        keys(&serde_json::to_value(ViewState::default()).unwrap())
    );
    let arrangement = serde_json::to_value(ArrangementRecord::default()).unwrap();
    assert_eq!(props(&defs["arrangement"]), keys(&arrangement));
    let all = serde_json::to_value(Manifest {
        format: FORMAT.into(),
        created_by: "x".into(),
        arrangements: Arrangements::default(),
        sweep: vec![],
        assumptions: None,
        view: ViewState::default(),
        layers: Layers {
            evidence: None,
            evidence_store: Some(EvidenceStoreLayer {
                mode: EvidenceMode::Packed,
                trees: vec![],
            }),
        },
        ensembles: vec![EnsembleRecord {
            blob: sha.clone(),
            scenario_sha256: sha.clone(),
            variant: "x".into(),
            key: crate::tests::ensembles::key(2, 'a'),
        }],
        blobs: vec![],
        extra: Default::default(),
    })
    .unwrap();
    assert_eq!(props(&schema), keys(&all));
    assert_eq!(props(&schema["properties"]["layers"]), keys(&all["layers"]));
}

// ---------------------------------------------------------------------------
// Unknown fields from a newer minor version

// Verifies: PRV-055
#[test]
fn unknown_top_level_manifest_fields_are_listed_and_survive_a_re_save() {
    let newer = Path::new(FIXTURE_DIR).join("02-newer-minor-unknown-fields.faris");
    let reader = StudyReader::open(&newer).unwrap();
    let unknown = reader.unknown_manifest_fields().clone();
    assert_eq!(
        unknown.keys().collect::<Vec<_>>(),
        ["future_field"],
        "the reader names every top-level property it does not know"
    );
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("saved.faris");
    write_study_preserving(&target, &draft(dir.path()), &unknown).unwrap();
    let saved = StudyReader::open(&target).unwrap();
    assert_eq!(saved.unknown_manifest_fields(), &unknown);
    assert_eq!(
        saved.manifest.format, FORMAT,
        "the new file is written in this version's format"
    );
    // A plain save writes none, and an ordinary file reports none.
    write_study(&target, &draft(dir.path())).unwrap();
    assert!(
        StudyReader::open(&target)
            .unwrap()
            .unknown_manifest_fields()
            .is_empty()
    );
    // A name this version defines is not an unknown field.
    let clash = std::collections::BTreeMap::from([("blobs".to_owned(), json!([]))]);
    let result = write_study_preserving(&target, &draft(dir.path()), &clash);
    assert!(matches!(result, Err(StudyError::Input(_))), "{result:?}");
}
