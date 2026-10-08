//! Core evidence as store trees in a study file: packed and referenced round
//! trips, the embedded store as a store, tamper refusals and resolution order.

use crate::tests::draft;
use crate::*;
use faris_engine::evidence_store::{EvidenceStore, StoreVerifyStatus, pack_store};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

fn put(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// A store with the saved studies `port/reference` and `control/reference`.
/// Some content is shared between the trees, as it is in a package.
fn make_store(dir: &Path, name: &str) -> PathBuf {
    let src = dir.join(format!("{name}-src"));
    let mut trees = Vec::new();
    for pair in ["port-reference", "control-reference"] {
        let case = src.join(format!("{pair}-case"));
        put(&case.join("package.json"), b"{\"package\":true}\n");
        put(&case.join("shared.json"), &b"shared text ".repeat(200));
        put(
            &case.join("expected/history.json"),
            format!("{{\"pair\":\"{pair}\"}}\n").as_bytes(),
        );
        put(&case.join("execution-report.json"), b"{\"report\":1}\n");
        let workspace = src.join(format!("{pair}-workspace"));
        put(&workspace.join("step/receipt.json"), pair.as_bytes());
        // Not compressible: stored blobs must not be recompressed.
        let noise: Vec<u8> = (0..4096u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        put(&workspace.join("step/output.bin"), &noise);
        trees.push((format!("{pair}-case"), case));
        trees.push((format!("{pair}-workspace"), workspace));
    }
    let out = dir.join(name);
    pack_store(&out, &trees).unwrap();
    out
}

fn drafts(store: &Path) -> Vec<EvidenceStoreDraft> {
    let opened = EvidenceStore::open(store).unwrap();
    let mut out = Vec::new();
    for (arrangement, allocation) in [("port", "reference"), ("control", "reference")] {
        for (kind, suffix) in [
            (ArchiveKind::Case, "case"),
            (ArchiveKind::Workspace, "workspace"),
        ] {
            let tree = format!("{arrangement}-{allocation}-{suffix}");
            out.push(EvidenceStoreDraft {
                tree: EvidenceStoreTree {
                    arrangement: arrangement.into(),
                    allocation: allocation.into(),
                    kind,
                    files: opened.tree(&tree).unwrap().files.clone(),
                    tree,
                },
                store: Some(store.to_owned()),
            });
        }
    }
    out
}

fn study(dir: &Path, store: &Path, pack: bool, name: &str) -> PathBuf {
    let mut d = draft(dir);
    d.evidence_store = drafts(store);
    d.pack_evidence = pack;
    let target = dir.join(name);
    write_study(&target, &d).unwrap();
    target
}

/// Rewrite a study file entry by entry (all stored): `edit` may change an
/// entry's bytes or drop it by returning `None`; `extra` entries are appended.
fn rewrite(
    src: &Path,
    dst: &Path,
    edit: impl Fn(&str, Vec<u8>) -> Option<Vec<u8>>,
    extra: Vec<(String, Vec<u8>)>,
) {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(src).unwrap()).unwrap();
    let mut writer = zip::ZipWriter::new(std::fs::File::create(dst).unwrap());
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().to_owned();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        if let Some(bytes) = edit(&name, bytes) {
            writer.start_file(name, stored).unwrap();
            writer.write_all(&bytes).unwrap();
        }
    }
    for (name, bytes) in extra {
        writer.start_file(name, stored).unwrap();
        writer.write_all(&bytes).unwrap();
    }
    writer.finish().unwrap();
}

fn edit_manifest(src: &Path, dst: &Path, change: impl Fn(&mut serde_json::Value)) {
    rewrite(
        src,
        dst,
        |name, bytes| {
            if name == "manifest.json" {
                let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                change(&mut value);
                Some(serde_json::to_vec(&value).unwrap())
            } else {
                Some(bytes)
            }
        },
        vec![],
    );
}

fn materialize(
    study: &Path,
    near: Option<&Path>,
    package: Option<&Path>,
) -> Result<Materialized, StudyError> {
    let out = tempfile::tempdir().unwrap();
    let out = out.keep();
    StudyReader::open(study)?.materialize_with(&out, near, package)
}

// Verifies: PRV-040
#[test]
fn packed_store_evidence_round_trips_as_a_verifiable_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = make_store(dir.path(), "evidence-store");
    let target = study(dir.path(), &store, true, "packed.faris");
    let mut reader = StudyReader::open(&target).unwrap();
    let layer = reader.manifest.layers.evidence_store.clone().unwrap();
    assert_eq!(layer.mode, EvidenceMode::Packed);
    assert_eq!(layer.trees.len(), 4);
    assert!(reader.manifest.layers.evidence.is_none());
    reader.verify().unwrap();

    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let files = reader.materialize(&out, None).unwrap();
    let state = files.evidence_store.unwrap();
    assert_eq!(state.mode, EvidenceMode::Packed);
    assert_eq!(state.found.len(), 2);
    assert!(state.missing.is_empty());
    assert!(
        state
            .found
            .iter()
            .all(|s| s.source == StoreSource::Embedded)
    );
    let extracted = &state.found[0].store;
    assert_eq!(extracted, &out.join("evidence-store"));
    let report = faris_engine::evidence_store::verify_store(extracted);
    assert_eq!(report.status, StoreVerifyStatus::Verified, "{report:?}");
    // The trees in the extracted store are exactly the recorded ones, and the
    // blobs are the source store's bytes, not recompressed.
    let source = EvidenceStore::open(&store).unwrap();
    let embedded = EvidenceStore::open(extracted).unwrap();
    assert_eq!(embedded.index(), source.index());
    for tree in &source.index().trees {
        for file in &tree.files {
            let blob = faris_engine::evidence_store::blob_path_in_store(&file.sha256);
            assert_eq!(
                std::fs::read(extracted.join(&blob)).unwrap(),
                std::fs::read(store.join(&blob)).unwrap()
            );
        }
    }
    // Entries are stored in the zip, not recompressed.
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&target).unwrap()).unwrap();
    let mut store_entries = 0;
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).unwrap();
        if entry.name().starts_with("evidence-store/") {
            store_entries += 1;
            assert_eq!(entry.compression(), zip::CompressionMethod::Stored);
        }
    }
    let distinct = source
        .index()
        .trees
        .iter()
        .flat_map(|t| &t.files)
        .map(|f| &f.sha256)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    assert_eq!(store_entries, distinct + 1);
    // The saved case inspects the same from the embedded store as from the source.
    let read = |s: &EvidenceStore| {
        s.read_file("port-reference-case", "execution-report.json", 1 << 20)
            .unwrap()
    };
    assert_eq!(read(&embedded), read(&source));
}

#[test]
fn a_packed_file_holds_only_the_trees_it_records() {
    let dir = tempfile::tempdir().unwrap();
    let store = make_store(dir.path(), "evidence-store");
    let mut d = draft(dir.path());
    d.evidence_store = drafts(&store)
        .into_iter()
        .filter(|t| t.tree.arrangement == "port")
        .collect();
    d.pack_evidence = true;
    let target = dir.path().join("one.faris");
    write_study(&target, &d).unwrap();
    let mut reader = StudyReader::open(&target).unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    reader.materialize(&out, None).unwrap();
    let embedded = EvidenceStore::open(&out.join("evidence-store")).unwrap();
    let names: Vec<_> = embedded
        .index()
        .trees
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(names, ["port-reference-case", "port-reference-workspace"]);
}

// Verifies: PRV-040, PRV-042
#[test]
fn referenced_store_evidence_records_listings_without_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let store = make_store(dir.path(), "evidence-store");
    let target = study(dir.path(), &store, false, "ref.faris");
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&target).unwrap()).unwrap();
    assert!((0..archive.len()).all(|i| {
        !archive
            .by_index_raw(i)
            .unwrap()
            .name()
            .starts_with("evidence-store")
    }));
    let reader = StudyReader::open(&target).unwrap();
    let layer = reader.manifest.layers.evidence_store.clone().unwrap();
    assert_eq!(layer.mode, EvidenceMode::Referenced);
    let source = EvidenceStore::open(&store).unwrap();
    for tree in &layer.trees {
        assert_eq!(tree.files, source.tree(&tree.tree).unwrap().files);
    }
    // Not beside the file and not in a package: not included, with why.
    let state = materialize(&target, Some(&dir.path().join("elsewhere")), None)
        .unwrap()
        .evidence_store
        .unwrap();
    assert!(state.found.is_empty());
    assert_eq!(state.missing.len(), 2);
    assert!(
        state.missing[0].reason.contains("beside the file"),
        "{:?}",
        state.missing[0]
    );
    assert!(
        state.missing[0]
            .reason
            .contains("not started from a package")
    );
}

#[test]
fn a_store_beside_the_file_wins_over_the_package_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = make_store(dir.path(), "evidence-store");
    let target = study(dir.path(), &store, false, "ref.faris");
    // A second copy of the same store as the "package" store.
    let package = make_store(dir.path(), "package-store");
    let both = materialize(&target, Some(dir.path()), Some(&package))
        .unwrap()
        .evidence_store
        .unwrap();
    assert_eq!(both.found.len(), 2);
    assert!(
        both.found
            .iter()
            .all(|s| s.source == StoreSource::BesideFile)
    );
    assert!(both.found.iter().all(|s| s.store == store));
    // Without a store beside the file the package store is used.
    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let only = materialize(&target, Some(&elsewhere), Some(&package))
        .unwrap()
        .evidence_store
        .unwrap();
    assert!(only.found.iter().all(|s| s.source == StoreSource::Package));
    assert!(only.found.iter().all(|s| s.store == package));
    assert_eq!(
        only.found[0].store_json_sha256,
        sha256_file(&package.join("store.json")).unwrap().0
    );
}

#[test]
fn a_store_that_lists_different_files_is_not_used() {
    let dir = tempfile::tempdir().unwrap();
    let store = make_store(dir.path(), "evidence-store");
    let target = study(dir.path(), &store, false, "ref.faris");
    // A store whose port case tree has one more file than the study recorded.
    let other_src = dir.path().join("other-src");
    let mut trees = Vec::new();
    for pair in ["port-reference", "control-reference"] {
        for kind in ["case", "workspace"] {
            let tree = other_src.join(format!("{pair}-{kind}"));
            put(&tree.join("only.json"), pair.as_bytes());
            trees.push((format!("{pair}-{kind}"), tree));
        }
    }
    let other = dir.path().join("other-store");
    pack_store(&other, &trees).unwrap();
    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let state = materialize(&target, Some(&elsewhere), Some(&other))
        .unwrap()
        .evidence_store
        .unwrap();
    assert!(state.found.is_empty());
    let reason = &state.missing[0].reason;
    assert!(reason.contains("lists different files"), "{reason}");
    assert!(reason.contains("port-reference-case") || reason.contains("control-reference-case"));
    // A store without the tree at all says so.
    let empty_src = dir.path().join("empty-src");
    put(&empty_src.join("x.json"), b"{}");
    let empty = dir.path().join("empty-store");
    pack_store(&empty, &[("unrelated".to_owned(), empty_src)]).unwrap();
    let state = materialize(&target, Some(&elsewhere), Some(&empty))
        .unwrap()
        .evidence_store
        .unwrap();
    assert!(
        state.missing[0].reason.contains("has no tree"),
        "{:?}",
        state.missing[0]
    );
}

#[test]
fn a_mismatching_store_beside_the_file_falls_through_to_the_package() {
    let dir = tempfile::tempdir().unwrap();
    let store = make_store(dir.path(), "evidence-store");
    let target = study(dir.path(), &store, false, "ref.faris");
    let package = make_store(dir.path(), "package-store");
    // A damaged beside-the-file store: its index lists other files.
    let beside = dir.path().join("beside");
    std::fs::create_dir(&beside).unwrap();
    let broken_src = dir.path().join("broken-src");
    put(&broken_src.join("x.json"), b"{}");
    pack_store(
        &beside.join("evidence-store"),
        &[("port-reference-case".into(), broken_src)],
    )
    .unwrap();
    let state = materialize(&target, Some(&beside), Some(&package))
        .unwrap()
        .evidence_store
        .unwrap();
    assert!(state.found.iter().all(|s| s.source == StoreSource::Package));
    assert_eq!(state.found.len(), 2);
}

// Verifies: PRV-042
#[test]
fn tampered_packed_stores_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = make_store(dir.path(), "evidence-store");
    let good = study(dir.path(), &store, true, "good.faris");
    let some_blob = {
        let reader = StudyReader::open(&good).unwrap();
        distinct_digest(&reader)
    };
    let blob_name = format!("evidence-store/blobs/{}/{}.xz", &some_blob[..2], some_blob);

    // A flipped byte inside an embedded blob.
    let flipped = dir.path().join("flipped.faris");
    rewrite(
        &good,
        &flipped,
        |name, mut bytes| {
            if name == blob_name {
                let middle = bytes.len() / 2;
                bytes[middle] ^= 0x01;
            }
            Some(bytes)
        },
        vec![],
    );
    let error = StudyReader::open(&flipped).unwrap().verify().unwrap_err();
    assert!(error.is_verification_failure(), "{error}");
    assert!(materialize(&flipped, None, None).is_err());

    // An entry larger than its listing allows is refused on open.
    let oversized = dir.path().join("oversized.faris");
    rewrite(
        &good,
        &oversized,
        |name, mut bytes| {
            if name == blob_name {
                bytes.extend(std::iter::repeat_n(0u8, 8192));
            }
            Some(bytes)
        },
        vec![],
    );
    let error = StudyReader::open(&oversized).err().unwrap();
    assert!(
        error.to_string().contains("larger than its listing allows"),
        "{error}"
    );

    // A blob entry missing from the file.
    let missing = dir.path().join("missing.faris");
    rewrite(
        &good,
        &missing,
        |n, b| (n != blob_name).then_some(b),
        vec![],
    );
    assert!(StudyReader::open(&missing).is_err());

    // An extra blob entry the trees do not list.
    let extra_digest = sha256_hex(b"extra");
    let extra = dir.path().join("extra.faris");
    rewrite(
        &good,
        &extra,
        |_, bytes| Some(bytes),
        vec![(
            format!(
                "evidence-store/blobs/{}/{extra_digest}.xz",
                &extra_digest[..2]
            ),
            vec![0; 16],
        )],
    );
    assert!(StudyReader::open(&extra).is_err());

    // Store entries with no packed layer: refused.
    let orphan = dir.path().join("orphan.faris");
    edit_manifest(&good, &orphan, |m| {
        m["layers"]
            .as_object_mut()
            .unwrap()
            .remove("evidence_store");
    });
    let error = StudyReader::open(&orphan).err().unwrap();
    assert!(
        error
            .to_string()
            .contains("does not record packed store evidence"),
        "{error}"
    );

    // store.json replaced by an index of different trees.
    let swapped = dir.path().join("swapped.faris");
    rewrite(
        &good,
        &swapped,
        |name, bytes| {
            if name == "evidence-store/store.json" {
                let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                value["trees"].as_array_mut().unwrap().pop();
                Some(serde_json::to_vec(&value).unwrap())
            } else {
                Some(bytes)
            }
        },
        vec![],
    );
    let error = materialize(&swapped, None, None).err().unwrap();
    assert!(
        error.to_string().contains("different trees or files"),
        "{error}"
    );
}

fn distinct_digest(reader: &StudyReader) -> String {
    reader
        .manifest
        .layers
        .evidence_store
        .as_ref()
        .unwrap()
        .trees[0]
        .files[0]
        .sha256
        .clone()
}

// Verifies: PRV-042
#[test]
fn a_changed_listing_extra_or_missing_trees_and_both_layers_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = make_store(dir.path(), "evidence-store");
    let packed = study(dir.path(), &store, true, "packed.faris");
    let referenced = study(dir.path(), &store, false, "ref.faris");

    // A listing that disagrees with the embedded store (a size) is refused.
    let resized = dir.path().join("resized.faris");
    edit_manifest(&packed, &resized, |m| {
        let file = &mut m["layers"]["evidence_store"]["trees"][0]["files"][0];
        let bytes = file["bytes"].as_u64().unwrap();
        file["bytes"] = (bytes + 1).into();
    });
    assert!(materialize(&resized, None, None).is_err());

    // An extra tree in the manifest that the embedded store lacks.
    let extra = dir.path().join("extra-tree.faris");
    edit_manifest(&packed, &extra, |m| {
        let trees = m["layers"]["evidence_store"]["trees"]
            .as_array_mut()
            .unwrap();
        let mut a = trees[0].clone();
        let mut b = trees[1].clone();
        a["allocation"] = "added".into();
        a["tree"] = "port-added-case".into();
        b["allocation"] = "added".into();
        b["tree"] = "port-added-workspace".into();
        trees.push(a);
        trees.push(b);
    });
    let error = materialize(&extra, None, None).err().unwrap();
    assert!(
        error.to_string().contains("different trees or files"),
        "{error}"
    );

    // A missing tree (the partner of a saved study) is a structural refusal.
    let lone = dir.path().join("lone.faris");
    edit_manifest(&referenced, &lone, |m| {
        m["layers"]["evidence_store"]["trees"]
            .as_array_mut()
            .unwrap()
            .pop();
    });
    let error = StudyReader::open(&lone).err().unwrap();
    assert!(error.to_string().contains("no partner"), "{error}");

    // Both layers present.
    let both = dir.path().join("both.faris");
    edit_manifest(&referenced, &both, |m| {
        m["layers"]["evidence"] = serde_json::json!({"mode": "referenced", "archives": []});
    });
    let error = StudyReader::open(&both).err().unwrap();
    assert!(
        error
            .to_string()
            .contains("both as archives and as store trees"),
        "{error}"
    );

    // Unsorted files and a repeated tree name.
    let unsorted = dir.path().join("unsorted.faris");
    edit_manifest(&referenced, &unsorted, |m| {
        m["layers"]["evidence_store"]["trees"][0]["files"]
            .as_array_mut()
            .unwrap()
            .reverse();
    });
    assert!(StudyReader::open(&unsorted).is_err());
    let repeated = dir.path().join("repeated.faris");
    edit_manifest(&referenced, &repeated, |m| {
        let name = m["layers"]["evidence_store"]["trees"][0]["tree"].clone();
        m["layers"]["evidence_store"]["trees"][1]["tree"] = name;
    });
    assert!(StudyReader::open(&repeated).is_err());
}

#[test]
fn a_writer_refuses_both_kinds_and_unavailable_or_changed_sources() {
    let dir = tempfile::tempdir().unwrap();
    let store = make_store(dir.path(), "evidence-store");
    let target = dir.path().join("x.faris");
    // Both archives and trees.
    let mut d = draft(dir.path());
    d.evidence_store = drafts(&store);
    d.evidence = vec![EvidenceDraft {
        archive: EvidenceArchive {
            arrangement: "port".into(),
            allocation: "reference".into(),
            kind: ArchiveKind::Case,
            file_name: "a.tar.gz".into(),
            sha256: "0".repeat(64),
            bytes: 1,
        },
        path: None,
    }];
    assert!(write_study(&target, &d).is_err());
    // Packing without a source store says why.
    let mut d = draft(dir.path());
    d.evidence_store = drafts(&store);
    d.evidence_store[0].store = None;
    d.pack_evidence = true;
    let error = write_study(&target, &d).unwrap_err();
    assert!(error.to_string().contains("saved by reference"), "{error}");
    // A listing that differs from the source store.
    let mut d = draft(dir.path());
    d.evidence_store = drafts(&store);
    d.evidence_store[0].tree.files.pop();
    d.pack_evidence = true;
    let error = write_study(&target, &d).unwrap_err();
    assert!(
        error.to_string().contains("lists different files"),
        "{error}"
    );
    assert!(!target.exists());
    // A corrupted blob in the source store is caught before copying.
    let mut d = draft(dir.path());
    d.evidence_store = drafts(&store);
    d.pack_evidence = true;
    let digest = d.evidence_store[0].tree.files[0].sha256.clone();
    let blob = store.join(faris_engine::evidence_store::blob_path_in_store(&digest));
    let mut bytes = std::fs::read(&blob).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0x01;
    std::fs::write(&blob, bytes).unwrap();
    let error = write_study(&target, &d).unwrap_err();
    assert!(error.to_string().contains("did not verify"), "{error}");
    assert!(!target.exists());
}

#[test]
fn packed_store_bytes_counts_distinct_stored_blobs() {
    let dir = tempfile::tempdir().unwrap();
    let store = make_store(dir.path(), "evidence-store");
    let all = drafts(&store);
    let total = packed_store_bytes(&all);
    assert!(total > 0);
    // One tree alone is no more than the whole.
    assert!(packed_store_bytes(&all[..1]) <= total);
    let mut unsourced = all.clone();
    unsourced.iter_mut().for_each(|d| d.store = None);
    assert_eq!(packed_store_bytes(&unsourced), 0);
}

#[test]
fn a_store_descriptor_yields_trees_with_their_listings() {
    let dir = tempfile::tempdir().unwrap();
    let store = make_store(dir.path(), "evidence-store");
    let descriptor = dir.path().join("saved-study-port-reference.json");
    std::fs::write(
        &descriptor,
        br#"{"schema_version":"faris-saved-study-store/v0.1","store":"evidence-store","case_tree":"port-reference-case","workspace_tree":"port-reference-workspace","execution_report_member":"execution-report.json"}"#,
    )
    .unwrap();
    let DescriptorEvidence::Store(trees) = descriptor_evidence(&descriptor).unwrap() else {
        panic!("a store descriptor names trees");
    };
    assert_eq!(trees.len(), 2);
    assert_eq!(trees[0].tree.kind, ArchiveKind::Case);
    assert_eq!(trees[0].tree.arrangement, "port");
    assert_eq!(trees[0].tree.allocation, "reference");
    assert_eq!(trees[1].tree.kind, ArchiveKind::Workspace);
    assert_eq!(trees[0].store.as_deref(), Some(store.as_path()));
    let source = EvidenceStore::open(&store).unwrap();
    assert_eq!(
        trees[0].tree.files,
        source.tree("port-reference-case").unwrap().files
    );
    // The archive reader names the way to read it.
    let error = evidence_from_descriptor(&descriptor)
        .unwrap_err()
        .to_string();
    assert!(error.contains("descriptor_evidence"), "{error}");

    let bad = |body: &str| {
        std::fs::write(&descriptor, body).unwrap();
        descriptor_evidence(&descriptor).unwrap_err().to_string()
    };
    assert!(bad(r#"{"schema_version":"faris-saved-study-store/v0.1","store":"../evidence-store","case_tree":"port-reference-case","workspace_tree":"port-reference-workspace"}"#).contains("relative"));
    assert!(bad(r#"{"schema_version":"faris-saved-study-store/v0.1","store":"evidence-store","case_tree":"port-reference-case","workspace_tree":"control-reference-workspace"}"#).contains("different studies"));
    assert!(bad(r#"{"schema_version":"faris-saved-study-store/v0.1","store":"evidence-store","case_tree":"odd","workspace_tree":"port-reference-workspace"}"#).contains("is not named"));
    assert!(bad(r#"{"schema_version":"faris-saved-study-store/v0.1","store":"missing-store","case_tree":"port-reference-case","workspace_tree":"port-reference-workspace"}"#).contains("cannot open the evidence store"));
}

/// The embedded store is a valid store for Avila Core's own verifier. Opt in
/// with FARIS_CORE_BIN=/path/to/avila-core.
#[test]
#[ignore = "needs an avila-core binary (FARIS_CORE_BIN)"]
fn the_embedded_store_verifies_with_core() {
    let Some(core) = std::env::var_os("FARIS_CORE_BIN") else {
        eprintln!("skipped: set FARIS_CORE_BIN");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = make_store(dir.path(), "evidence-store");
    let target = study(dir.path(), &store, true, "packed.faris");
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let mut reader = StudyReader::open(&target).unwrap();
    let root = reader.extract_embedded_store(&out).unwrap();
    let output = std::process::Command::new(core)
        .args(["store", "verify"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}
