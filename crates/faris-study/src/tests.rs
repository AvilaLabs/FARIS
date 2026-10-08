use crate::*;
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
};

const REQUIRED: [&str; 6] = [
    "run.json",
    "input.json",
    "scenario.json",
    "audit.json",
    "reactor_transport.py",
    "solver/transport-artifact.json",
];

fn bundle(tag: &str) -> faris_engine::core_evidence::RecordedTransportBundle {
    let mut files = BTreeMap::new();
    for name in REQUIRED {
        // scenario.json and the script are shared between bundles; the rest differ.
        let text = if name == "scenario.json" || name.ends_with(".py") {
            format!("{{\"shared\":\"{name}\"}}\n")
        } else {
            format!("{{\"{name}\":\"{tag}\"}}\n")
        };
        files.insert(name.to_owned(), text);
    }
    faris_engine::core_evidence::RecordedTransportBundle {
        schema_version: "faris-recorded-transport-bundle/v0.1".into(),
        files,
        notice: "test notice".into(),
    }
}

fn write_bundle(dir: &Path, name: &str, tag: &str) -> PathBuf {
    let path = dir.join(format!("{name}.transport-bundle.json"));
    let mut json = serde_json::to_vec_pretty(&bundle(tag)).unwrap();
    // Recorded files differ on a trailing newline; both must round-trip.
    if tag.starts_with('s') {
        json.push(b'\n');
    }
    std::fs::write(&path, json).unwrap();
    path
}

fn archive_file(dir: &Path, name: &str, content: &[u8]) -> EvidenceArchive {
    std::fs::write(dir.join(name), content).unwrap();
    EvidenceArchive {
        arrangement: "port".into(),
        allocation: "reference".into(),
        kind: if name.contains("case") {
            ArchiveKind::Case
        } else {
            ArchiveKind::Workspace
        },
        file_name: name.into(),
        sha256: sha256_hex(content),
        bytes: content.len() as u64,
    }
}

fn draft(dir: &Path) -> StudyDraft {
    std::fs::write(dir.join("assumptions.json"), b"{\"a\":1}\n").unwrap();
    StudyDraft {
        port: Some(ArrangementDraft {
            scenario: None,
            physics: vec![],
            bundles: vec![
                write_bundle(dir, "reference", "p1"),
                write_bundle(dir, "breeder-emphasis", "p2"),
            ],
        }),
        control: Some(ArrangementDraft {
            scenario: None,
            physics: vec![],
            bundles: vec![write_bundle(dir, "c-reference", "c1")],
        }),
        sweep: vec![write_bundle(dir, "blanket-030cm", "s1")],
        assumptions: Some(dir.join("assumptions.json")),
        view: ViewState {
            step: "evidence".into(),
            preset: Some("Loaded assumptions".into()),
            year: 12.5,
            field_view: "flux-slice".into(),
            history_tab: "tritium".into(),
            arrangement: "control".into(),
            allocation: "breeder-emphasis".into(),
            sweep_blanket_m: Some(0.55),
            what_if: None,
        },
        zstd_level: 3,
        ..StudyDraft::default()
    }
}

#[test]
fn round_trip_restores_bundles_assumptions_and_view() {
    let dir = tempfile::tempdir().unwrap();
    let draft = draft(dir.path());
    let target = dir.path().join("a.faris");
    let report = write_study(&target, &draft).unwrap();
    assert_eq!(report.file_bytes, std::fs::metadata(&target).unwrap().len());
    let mut reader = StudyReader::open(&target).unwrap();
    assert_eq!(reader.manifest.view, draft.view);
    assert_eq!(reader.verify().unwrap(), report.blob_count);
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let files = reader.materialize(&out, None).unwrap();
    let port = files.port.unwrap();
    assert_eq!(port.bundles.len(), 2);
    for (written, original) in port
        .bundles
        .iter()
        .zip(draft.port.as_ref().unwrap().bundles.iter())
    {
        assert_eq!(
            std::fs::read(written).unwrap(),
            std::fs::read(original).unwrap()
        );
        assert_eq!(written.file_name(), original.file_name());
    }
    assert_eq!(files.control.unwrap().bundles.len(), 1);
    assert_eq!(files.sweep.len(), 1);
    assert_eq!(
        std::fs::read(files.assumptions.unwrap()).unwrap(),
        b"{\"a\":1}\n"
    );
    assert!(files.evidence.mode.is_none());
}

#[test]
fn a_store_descriptor_is_refused_with_a_next_step() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("saved-study-port-reference.json");
    std::fs::write(
        &path,
        br#"{"schema_version":"faris-saved-study-store/v0.1","store":"evidence-store","case_tree":"port-reference-case","workspace_tree":"port-reference-workspace"}"#,
    )
    .unwrap();
    let message = evidence_from_descriptor(&path).unwrap_err().to_string();
    assert!(message.contains("evidence store"), "{message}");
    assert!(message.contains("without --evidence"), "{message}");
}

#[test]
fn identical_files_are_stored_once() {
    let dir = tempfile::tempdir().unwrap();
    let draft = draft(dir.path());
    let target = dir.path().join("a.faris");
    let report = write_study(&target, &draft).unwrap();
    // Four bundles share scenario.json and the script; each has 4 distinct members; plus assumptions.
    assert_eq!(report.blob_count, 4 * 4 + 2 + 1);
    let mut reader = StudyReader::open(&target).unwrap();
    assert_eq!(reader.blob_infos().unwrap().len(), report.blob_count);
}

// Verifies: INT-002
#[test]
fn first_entry_is_the_stored_type_string_at_offset_thirty() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("a.faris");
    write_study(&target, &draft(dir.path())).unwrap();
    let bytes = std::fs::read(&target).unwrap();
    let magic = format!("mimetype{MIMETYPE}");
    assert_eq!(&bytes[30..30 + magic.len()], magic.as_bytes());
}

// Verifies: REL-010
#[test]
fn saving_replaces_atomically_and_leaves_no_temporary_files() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("study");
    std::fs::create_dir(&out).unwrap();
    let target = out.join("a.faris");
    let mut d = draft(dir.path());
    write_study(&target, &d).unwrap();
    d.view.year = 3.0;
    write_study(&target, &d).unwrap();
    assert_eq!(StudyReader::open(&target).unwrap().manifest.view.year, 3.0);
    let names: Vec<_> = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("a.faris")]);
}

// ----- hand-built containers for tamper tests -----

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = flate2::Crc::new();
    crc.update(bytes);
    crc.sum()
}

/// A zip of stored entries, with whatever names and contents are given.
fn raw_zip(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let mut local = Vec::new();
        local.extend_from_slice(&0x04034b50u32.to_le_bytes());
        local.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
        local.extend_from_slice(&crc.to_le_bytes());
        local.extend_from_slice(&(data.len() as u32).to_le_bytes());
        local.extend_from_slice(&(data.len() as u32).to_le_bytes());
        local.extend_from_slice(&(name.len() as u16).to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes());
        local.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&local);
        out.extend_from_slice(data);
        let mut c = Vec::new();
        c.extend_from_slice(&0x02014b50u32.to_le_bytes());
        c.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
        c.extend_from_slice(&crc.to_le_bytes());
        c.extend_from_slice(&(data.len() as u32).to_le_bytes());
        c.extend_from_slice(&(data.len() as u32).to_le_bytes());
        c.extend_from_slice(&(name.len() as u16).to_le_bytes());
        c.extend_from_slice(&[0; 12]);
        c.extend_from_slice(&offset.to_le_bytes());
        c.extend_from_slice(name.as_bytes());
        central.extend_from_slice(&c);
    }
    let start = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x06054b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// A small valid container: one assumptions blob.
struct Tiny {
    content: Vec<u8>,
    sha: String,
}

impl Tiny {
    fn new() -> Self {
        let content = b"{\"x\":1}\n".to_vec();
        Self {
            sha: sha256_hex(&content),
            content,
        }
    }

    fn manifest(&self, format: &str, bytes: u64, encoding: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "format": format,
            "assumptions": self.sha,
            "future_field": {"ignored": true},
            "blobs": [{"sha256": self.sha, "bytes": bytes, "media_type": "application/json", "encoding": encoding}],
        }))
        .unwrap()
    }

    fn entries(&self, manifest: Vec<u8>) -> Vec<(String, Vec<u8>)> {
        vec![
            ("mimetype".into(), MIMETYPE.as_bytes().to_vec()),
            ("manifest.json".into(), manifest),
            (format!("blobs/{}", self.sha), self.content.clone()),
        ]
    }

    fn good(&self) -> Vec<(String, Vec<u8>)> {
        self.entries(self.manifest("faris-study/1", self.content.len() as u64, "verbatim"))
    }
}

fn open_raw(entries: &[(String, Vec<u8>)]) -> Result<StudyReader, StudyError> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.faris");
    std::fs::write(&path, raw_zip(entries)).unwrap();
    StudyReader::open(&path)
}

// Verifies: INT-001
#[test]
fn the_hand_built_container_reads_and_ignores_unknown_manifest_fields() {
    let tiny = Tiny::new();
    let mut reader = open_raw(&tiny.good()).unwrap();
    assert_eq!(reader.verify().unwrap(), 1);
    assert_eq!(reader.read_blob(&tiny.sha).unwrap(), tiny.content);
}

// Verifies: PRV-053
#[test]
fn a_minor_version_is_accepted() {
    let tiny = Tiny::new();
    let e = tiny.entries(tiny.manifest("faris-study/1.4", tiny.content.len() as u64, "verbatim"));
    open_raw(&e).unwrap();
}

// Verifies: CFG-074, SEC-004, REL-021, INT-004, INT-083
#[test]
fn an_unknown_major_version_is_refused_and_named() {
    let tiny = Tiny::new();
    for format in [
        "faris-study/2",
        "faris-study/2.1",
        "other/1",
        "faris-study/",
    ] {
        let e = tiny.entries(tiny.manifest(format, tiny.content.len() as u64, "verbatim"));
        match open_raw(&e) {
            Err(StudyError::UnsupportedVersion(v)) => assert_eq!(v, format),
            other => panic!("{format}: {:?}", other.err()),
        }
    }
}

// Verifies: SEC-004, REL-021
#[test]
fn an_unknown_encoding_is_refused() {
    let tiny = Tiny::new();
    let e = tiny.entries(tiny.manifest("faris-study/1", tiny.content.len() as u64, "array-f64-le"));
    match open_raw(&e) {
        Err(StudyError::UnsupportedEncoding { encoding, .. }) => {
            assert_eq!(encoding, "array-f64-le")
        }
        other => panic!("{:?}", other.err()),
    }
}

// Verifies: REL-020, PRV-004, PRV-005, SEC-010, COL-003, INT-004
#[test]
fn an_altered_blob_is_named_and_refused() {
    let tiny = Tiny::new();
    let mut e = tiny.good();
    e[2].1 = b"{\"x\":2}\n".to_vec();
    let mut reader = open_raw(&e).unwrap();
    match reader.read_blob(&tiny.sha) {
        Err(StudyError::HashMismatch { blob, .. }) => assert_eq!(blob, tiny.sha),
        other => panic!("{:?}", other.err()),
    }
    assert!(reader.verify().is_err());
}

#[test]
fn a_wrong_recorded_size_is_refused() {
    let tiny = Tiny::new();
    let e = tiny.entries(tiny.manifest("faris-study/1", 3, "verbatim"));
    let mut reader = open_raw(&e).unwrap();
    assert!(matches!(
        reader.read_blob(&tiny.sha),
        Err(StudyError::SizeMismatch { .. })
    ));
}

// Verifies: SEC-003, REL-021
#[test]
fn an_entry_larger_than_its_recorded_size_is_refused() {
    let tiny = Tiny::new();
    let mut e = tiny.good();
    e[2].1.extend_from_slice(&[b' '; 4096]);
    let mut reader = open_raw(&e).unwrap();
    assert!(matches!(
        reader.read_blob(&tiny.sha),
        Err(StudyError::SizeMismatch { .. })
    ));
}

// Verifies: SEC-002, SEC-004, REL-021
#[test]
fn unexpected_entry_names_are_refused() {
    let tiny = Tiny::new();
    for name in [
        "../evil".to_owned(),
        "/abs".to_owned(),
        "blobs/../manifest.json".to_owned(),
        "blobs/zz".to_owned(),
        format!("blobs/{}/x", tiny.sha),
        "notes.txt".to_owned(),
        "blobs/".to_owned(),
    ] {
        let mut e = tiny.good();
        e.push((name.clone(), b"x".to_vec()));
        assert!(open_raw(&e).is_err(), "{name} accepted");
    }
}

// Verifies: SEC-004, REL-021
#[test]
fn a_duplicate_entry_is_refused() {
    let tiny = Tiny::new();
    let mut e = tiny.good();
    e.push(e[2].clone());
    assert!(open_raw(&e).is_err());
    let mut e = tiny.good();
    e.push(e[1].clone());
    assert!(open_raw(&e).is_err());
}

#[test]
fn mimetype_must_come_first_and_be_right() {
    let tiny = Tiny::new();
    let mut e = tiny.good();
    e.swap(0, 1);
    assert!(open_raw(&e).is_err());
    let mut e = tiny.good();
    e[0].1 = b"application/zip".to_vec();
    assert!(open_raw(&e).is_err());
}

// Verifies: SEC-004, REL-021
#[test]
fn a_blob_without_an_entry_or_an_entry_without_a_blob_is_refused() {
    let tiny = Tiny::new();
    let mut e = tiny.good();
    e.pop();
    assert!(open_raw(&e).is_err());
    let mut e = tiny.good();
    e.push((format!("blobs/{}", sha256_hex(b"other")), b"other".to_vec()));
    assert!(open_raw(&e).is_err());
}

// Verifies: SEC-003
#[test]
fn declared_totals_beyond_the_bound_are_refused() {
    let tiny = Tiny::new();
    let e = tiny.entries(tiny.manifest("faris-study/1", 5 * 1024 * 1024 * 1024, "verbatim"));
    assert!(open_raw(&e).is_err());
}

// Verifies: SEC-002
#[test]
fn unsafe_bundle_names_in_the_manifest_are_refused() {
    let tiny = Tiny::new();
    let manifest = serde_json::to_vec(&serde_json::json!({
        "format": "faris-study/1",
        "sweep": [{"name": "../escape", "schema_version": "x", "notice": "", "files": {"run.json": tiny.sha}}],
        "blobs": [{"sha256": tiny.sha, "bytes": tiny.content.len(), "media_type": "application/json", "encoding": "verbatim"}],
    }))
    .unwrap();
    assert!(open_raw(&tiny.entries(manifest)).is_err());
}

// Verifies: INT-004
#[test]
fn not_a_zip_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.faris");
    std::fs::write(&path, b"not a zip at all").unwrap();
    assert!(StudyReader::open(&path).is_err());
    assert!(StudyReader::open(&dir.path().join("missing.faris")).is_err());
}

#[test]
fn verification_failures_are_distinguished_from_input_errors() {
    let tiny = Tiny::new();
    let e = tiny.entries(tiny.manifest("faris-study/2", 1, "verbatim"));
    assert!(open_raw(&e).err().unwrap().is_verification_failure());
    let dir = tempfile::tempdir().unwrap();
    let missing = StudyReader::open(&dir.path().join("none.faris"))
        .err()
        .unwrap();
    assert!(!missing.is_verification_failure());
}

// ----- evidence layer -----

fn evidence_draft(dir: &Path, pack: bool) -> (StudyDraft, Vec<EvidenceArchive>) {
    let mut d = draft(dir);
    let case = archive_file(dir, "reference-case.tar.gz", b"case archive bytes");
    let workspace = archive_file(
        dir,
        "reference-workspace.tar.gz",
        b"workspace archive bytes",
    );
    d.evidence = [&case, &workspace]
        .into_iter()
        .map(|a| EvidenceDraft {
            archive: a.clone(),
            path: Some(dir.join(&a.file_name)),
        })
        .collect();
    d.pack_evidence = pack;
    (d, vec![case, workspace])
}

// Verifies: PRV-040
#[test]
fn packed_evidence_is_stored_and_restored() {
    let dir = tempfile::tempdir().unwrap();
    let (d, archives) = evidence_draft(dir.path(), true);
    let target = dir.path().join("p.faris");
    write_study(&target, &d).unwrap();
    let mut reader = StudyReader::open(&target).unwrap();
    let infos = reader.blob_infos().unwrap();
    for archive in &archives {
        let info = infos.iter().find(|i| i.sha256 == archive.sha256).unwrap();
        assert!(
            !info.compressed,
            "evidence archives are stored, not recompressed"
        );
        assert_eq!(info.media_type, "application/gzip");
    }
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let files = reader.materialize(&out, None).unwrap();
    assert_eq!(files.evidence.mode, Some(EvidenceMode::Packed));
    assert!(files.evidence.missing.is_empty());
    let (case, workspace) = files.evidence.pair("port", "reference").unwrap();
    assert_eq!(std::fs::read(&case.path).unwrap(), b"case archive bytes");
    assert_eq!(
        std::fs::read(&workspace.path).unwrap(),
        b"workspace archive bytes"
    );
}

// Verifies: PRV-040, PRV-042
#[test]
fn referenced_evidence_records_hashes_without_the_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let (d, archives) = evidence_draft(dir.path(), false);
    let target = dir.path().join("r.faris");
    let report = write_study(&target, &d).unwrap();
    assert_eq!(report.evidence, Some(EvidenceMode::Referenced));
    let mut reader = StudyReader::open(&target).unwrap();
    let layer = reader.manifest.layers.evidence.clone().unwrap();
    assert_eq!(layer.mode, EvidenceMode::Referenced);
    assert_eq!(layer.archives, archives);
    assert!(
        reader
            .blob_infos()
            .unwrap()
            .iter()
            .all(|i| i.media_type != "application/gzip")
    );

    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    // Not beside the file: missing, and the study still materializes.
    let state = reader
        .materialize(&out, Some(&dir.path().join("elsewhere")))
        .unwrap()
        .evidence;
    assert_eq!(state.missing.len(), 2);
    assert!(state.available.is_empty());
    assert!(state.pair("port", "reference").is_none());
    // Beside the file with matching hashes: found.
    let out2 = dir.path().join("out2");
    std::fs::create_dir(&out2).unwrap();
    let state = reader
        .materialize(&out2, Some(dir.path()))
        .unwrap()
        .evidence;
    assert!(state.missing.is_empty());
    assert!(state.pair("port", "reference").is_some());
    // Beside the file with a different content: refused, with the reason.
    std::fs::write(dir.path().join("reference-case.tar.gz"), b"changed").unwrap();
    let out3 = dir.path().join("out3");
    std::fs::create_dir(&out3).unwrap();
    let state = reader
        .materialize(&out3, Some(dir.path()))
        .unwrap()
        .evidence;
    assert_eq!(state.missing.len(), 1);
    assert_eq!(state.missing[0].reason, MissReason::HashMismatch);
    assert!(state.pair("port", "reference").is_none());
}

// Verifies: PRV-042
#[test]
fn packing_needs_the_archives_and_checks_them() {
    let dir = tempfile::tempdir().unwrap();
    let (mut d, _) = evidence_draft(dir.path(), true);
    d.evidence[0].path = None;
    let target = dir.path().join("x.faris");
    assert!(write_study(&target, &d).is_err());
    let (mut d, _) = evidence_draft(dir.path(), true);
    std::fs::write(dir.path().join("reference-case.tar.gz"), b"different").unwrap();
    d.evidence[0].path = Some(dir.path().join("reference-case.tar.gz"));
    let error = write_study(&target, &d).unwrap_err();
    assert!(error.to_string().contains("does not match"), "{error}");
    assert!(!target.exists());
}

// ----- tar extraction -----

fn ustar(entries: &[(&str, &[u8], u8)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, data, kind) in entries {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        header[100..107].copy_from_slice(b"0000444");
        header[124..135].copy_from_slice(format!("{:011o}", data.len()).as_bytes());
        header[156] = *kind;
        header[257..262].copy_from_slice(b"ustar");
        header[148..156].copy_from_slice(b"        ");
        let sum: u32 = header.iter().map(|b| u32::from(*b)).sum();
        header[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(data);
        out.extend(std::iter::repeat_n(0u8, (512 - data.len() % 512) % 512));
    }
    out.extend(std::iter::repeat_n(0u8, 1024));
    out
}

fn gz(bytes: &[u8], path: &Path) {
    let mut encoder = flate2::write::GzEncoder::new(
        std::fs::File::create(path).unwrap(),
        flate2::Compression::fast(),
    );
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap();
}

#[test]
fn tar_gz_extracts_regular_files() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("a.tar.gz");
    gz(
        &ustar(&[("a.json", b"one", b'0'), ("sub/b.json", &[7u8; 700], b'0')]),
        &archive,
    );
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let report = extract_tar_gz(&archive, &out).unwrap();
    assert_eq!((report.files, report.bytes), (2, 703));
    assert_eq!(std::fs::read(out.join("a.json")).unwrap(), b"one");
    assert_eq!(std::fs::read(out.join("sub/b.json")).unwrap().len(), 700);
}

// Verifies: SEC-002, PRV-044, REL-021
#[test]
fn tar_gz_refuses_traversal_links_duplicates_and_damage() {
    let dir = tempfile::tempdir().unwrap();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("traversal", ustar(&[("../evil", b"x", b'0')])),
        ("absolute", ustar(&[("/evil", b"x", b'0')])),
        ("symlink", ustar(&[("link", b"", b'2')])),
        ("directory", ustar(&[("dir", b"", b'5')])),
        ("duplicate", ustar(&[("a", b"x", b'0'), ("a", b"y", b'0')])),
    ];
    for (label, tar) in cases {
        let archive = dir.path().join(format!("{label}.tar.gz"));
        gz(&tar, &archive);
        let out = dir.path().join(format!("out-{label}"));
        std::fs::create_dir(&out).unwrap();
        assert!(extract_tar_gz(&archive, &out).is_err(), "{label} accepted");
        assert!(!dir.path().join("evil").exists());
    }
    let mut tar = ustar(&[("a", b"x", b'0')]);
    tar[10] ^= 1;
    let archive = dir.path().join("bad.tar.gz");
    gz(&tar, &archive);
    let out = dir.path().join("out-bad");
    std::fs::create_dir(&out).unwrap();
    assert!(extract_tar_gz(&archive, &out).is_err());
    std::fs::write(dir.path().join("plain.tar.gz"), b"not gzip").unwrap();
    assert!(extract_tar_gz(&dir.path().join("plain.tar.gz"), &out).is_err());
}

// Verifies: SEC-002
#[test]
fn evidence_paths_must_be_safe_relative_paths() {
    let dir = tempfile::tempdir().unwrap();
    for bad in [
        "../x.tar.gz",
        "/abs.tar.gz",
        "a//b.tar.gz",
        "a/b/c/d/e.tar.gz",
        "a b.tar.gz",
    ] {
        let (mut d, _) = evidence_draft(dir.path(), false);
        d.evidence[0].archive.file_name = bad.into();
        assert!(
            write_study(&dir.path().join("x.faris"), &d).is_err(),
            "{bad}"
        );
    }
    // Package-style relative paths are fine and survive a round trip.
    let (mut d, _) = evidence_draft(dir.path(), true);
    std::fs::create_dir_all(dir.path().join("port/archives")).unwrap();
    std::fs::copy(
        dir.path().join("reference-case.tar.gz"),
        dir.path().join("port/archives/reference-case.tar.gz"),
    )
    .unwrap();
    d.evidence[0].archive.file_name = "port/archives/reference-case.tar.gz".into();
    d.evidence[0].path = Some(dir.path().join("port/archives/reference-case.tar.gz"));
    let target = dir.path().join("rel.faris");
    write_study(&target, &d).unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let state = StudyReader::open(&target)
        .unwrap()
        .materialize(&out, None)
        .unwrap()
        .evidence;
    assert!(
        out.join("evidence/port/archives/reference-case.tar.gz")
            .is_file()
    );
    assert_eq!(state.available.len(), 2);
}

/// Round-trips the real review-demo inputs: port and control bundles, the
/// allocation sweep, assumptions and the four saved Core studies. Run with
/// `--ignored --nocapture` with FARIS_DEMO_DIR and FARIS_SWEEP_DIR set.
#[test]
#[ignore = "needs the packaged demo inputs and the sweep bundles on disk"]
fn real_demo_inputs_round_trip_with_sizes() {
    let (Some(demo), Some(sweep_dir)) = (
        std::env::var_os("FARIS_DEMO_DIR").map(PathBuf::from),
        std::env::var_os("FARIS_SWEEP_DIR").map(PathBuf::from),
    ) else {
        eprintln!(
            "skipped: set FARIS_DEMO_DIR (packaged demo) and FARIS_SWEEP_DIR (sweep bundles)"
        );
        return;
    };
    let bundles = |role: &str| -> Vec<PathBuf> {
        ["reference", "breeder-emphasis"]
            .iter()
            .map(|n| {
                demo.join(role)
                    .join(format!("bundles/{n}.transport-bundle.json"))
            })
            .collect()
    };
    let mut sweep: Vec<PathBuf> = std::fs::read_dir(&sweep_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    sweep.sort();
    assert_eq!(sweep.len(), 7);
    let mut evidence = Vec::new();
    for arrangement in ["port", "control"] {
        for allocation in ["reference", "breeder-emphasis"] {
            evidence.extend(
                evidence_from_descriptor(
                    &demo.join(format!("saved-study-{arrangement}-{allocation}.json")),
                )
                .unwrap(),
            );
        }
    }
    assert_eq!(evidence.len(), 8);
    let original_json: u64 = bundles("port")
        .iter()
        .chain(&bundles("control"))
        .chain(&sweep)
        .map(|p| std::fs::metadata(p).unwrap().len())
        .sum();
    let level: i64 = std::env::var("FARIS_ZSTD_LEVEL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_ZSTD_LEVEL);
    let dir = tempfile::tempdir().unwrap();
    let mut draft = StudyDraft {
        port: Some(ArrangementDraft {
            scenario: Some(demo.join("port/scenario.json")),
            physics: vec![],
            bundles: bundles("port"),
        }),
        control: Some(ArrangementDraft {
            scenario: Some(demo.join("control/scenario.json")),
            physics: vec![],
            bundles: bundles("control"),
        }),
        sweep,
        assumptions: Some(demo.join("operating-assumptions.json")),
        evidence,
        pack_evidence: false,
        zstd_level: level,
        ..StudyDraft::default()
    };
    let referenced = dir.path().join("referenced.faris");
    let started = std::time::Instant::now();
    let report = write_study(&referenced, &draft).unwrap();
    let save_seconds = started.elapsed().as_secs_f64();
    draft.pack_evidence = true;
    let packed = dir.path().join("packed.faris");
    let started = std::time::Instant::now();
    let packed_report = write_study(&packed, &draft).unwrap();
    let packed_seconds = started.elapsed().as_secs_f64();

    let started = std::time::Instant::now();
    let mut reader = StudyReader::open(&referenced).unwrap();
    reader.verify().unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let files = reader.materialize(&out, Some(&demo)).unwrap();
    let open_seconds = started.elapsed().as_secs_f64();
    // Reconstructed bundles are byte-identical to the recorded JSON files.
    let mut pairs: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (role, files) in [
        ("port", files.port.as_ref().unwrap()),
        ("control", files.control.as_ref().unwrap()),
    ] {
        pairs.extend(files.bundles.iter().cloned().zip(bundles(role)));
    }
    for (written, original) in &pairs {
        assert_eq!(
            std::fs::read(written).unwrap(),
            std::fs::read(original).unwrap(),
            "{}",
            original.display()
        );
    }
    assert_eq!(files.sweep.len(), 7);
    for written in &files.sweep {
        let original = sweep_dir.join(written.file_name().unwrap());
        assert_eq!(
            std::fs::read(written).unwrap(),
            std::fs::read(original).unwrap()
        );
    }
    // The archives sit under the package root by their relative paths.
    assert_eq!(files.evidence.available.len(), 8);
    assert!(files.evidence.missing.is_empty());
    // Packed: extract one case archive and find the report the app needs.
    let mut packed_reader = StudyReader::open(&packed).unwrap();
    let out2 = dir.path().join("out2");
    std::fs::create_dir(&out2).unwrap();
    let packed_files = packed_reader.materialize(&out2, None).unwrap();
    assert_eq!(packed_files.evidence.available.len(), 8);
    let (case, _) = packed_files.evidence.pair("port", "reference").unwrap();
    let extracted = dir.path().join("case");
    std::fs::create_dir(&extracted).unwrap();
    let extraction = extract_tar_gz(&case.path, &extracted).unwrap();
    assert!(extracted.join("execution-report.json").is_file());
    assert!(extracted.join("package.json").is_file());

    println!(
        "zstd level {level}: original bundle JSON {:.1} MB; referenced {:.2} MB ({} blobs, {:.1} MB unique) saved in {save_seconds:.1} s; packed {:.2} MB saved in {packed_seconds:.1} s; open (verify + materialize) {open_seconds:.1} s; case archive extracts to {} files / {:.0} MB",
        original_json as f64 / 1e6,
        report.file_bytes as f64 / 1e6,
        report.blob_count,
        report.original_bytes as f64 / 1e6,
        packed_report.file_bytes as f64 / 1e6,
        extraction.files,
        extraction.bytes as f64 / 1e6,
    );
    assert!(
        report.file_bytes < 8_000_000,
        "referenced study should be a few MB"
    );
    assert!(packed_report.file_bytes < report.file_bytes + 60_000_000);
}

// ---------------------------------------------------------------------------
// Derived history ensembles

mod ensembles {
    use super::*;
    use faris_engine::fixtures::{assumptions, ensemble, rates_with_covariance};
    use faris_engine::history_ensemble::{EnsembleStatus, HistoryEnsemble};
    use faris_engine::history_uncertainty::EnsembleKey;
    use std::sync::Arc;

    fn key(samples: u32, artifact: char) -> EnsembleKey {
        EnsembleKey::new(
            &rates_with_covariance(0.06, 0.4, artifact),
            &assumptions(),
            samples,
        )
        .unwrap()
    }

    /// An ensemble of `samples` samples with the key that describes it.
    fn stored(samples: u32, artifact: char) -> (EnsembleKey, HistoryEnsemble) {
        let k = key(samples, artifact);
        (k.clone(), ensemble(samples, k.seed, artifact))
    }

    fn ensemble_draft(variant: &str, key: EnsembleKey, value: HistoryEnsemble) -> EnsembleDraft {
        EnsembleDraft {
            scenario_sha256: "a".repeat(64),
            variant: variant.into(),
            key,
            ensemble: Arc::new(value),
        }
    }

    // Verifies: AUTO-030, AUTO-033
    #[test]
    fn a_study_stores_ensembles_and_reuses_one_only_on_an_exact_key() {
        let dir = tempfile::tempdir().unwrap();
        let mut study = draft(dir.path());
        let (ka, ea) = stored(6, 'a');
        let (kb, eb) = stored(6, 'b');
        study.ensembles = vec![
            ensemble_draft("reference", ka.clone(), ea.clone()),
            ensemble_draft("breeder-emphasis", kb.clone(), eb.clone()),
            // The same key twice is stored once.
            ensemble_draft("reference", ka.clone(), ea.clone()),
        ];
        let target = dir.path().join("e.faris");
        write_study(&target, &study).unwrap();
        let mut reader = StudyReader::open(&target).unwrap();
        assert_eq!(reader.manifest.ensembles.len(), 2);
        reader.verify().unwrap();
        let all = reader.ensembles().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].ensemble, ea);
        assert_eq!(all[0].variant, "reference");

        assert_eq!(reader.ensemble_for(&ka).unwrap(), Some(ea));
        assert_eq!(reader.ensemble_for(&kb).unwrap(), Some(eb));
        // Any difference in the key finds nothing: the caller recomputes.
        assert_eq!(reader.ensemble_for(&key(7, 'a')).unwrap(), None);
        let mut other_rates = ka.clone();
        other_rates.rates_sha256 = "0".repeat(64);
        assert_eq!(reader.ensemble_for(&other_rates).unwrap(), None);
        let mut other_assumptions = ka.clone();
        other_assumptions.assumptions_sha256 = "0".repeat(64);
        assert_eq!(reader.ensemble_for(&other_assumptions).unwrap(), None);
        let mut other_seed = ka.clone();
        other_seed.seed ^= 1;
        assert_eq!(reader.ensemble_for(&other_seed).unwrap(), None);
        let mut other_method = ka;
        other_method.method.push('x');
        assert_eq!(reader.ensemble_for(&other_method).unwrap(), None);
    }

    #[test]
    fn a_not_evaluated_ensemble_round_trips_with_its_reasons() {
        let dir = tempfile::tempdir().unwrap();
        let (k, mut value) = stored(4, 'a');
        value.status = EnsembleStatus::NotEvaluated {
            why: "no covariance".into(),
            next_step: "rerun transport".into(),
        };
        value.samples.clear();
        value.nominal = None;
        value.summary = None;
        let mut study = draft(dir.path());
        study.ensembles = vec![ensemble_draft("reference", k.clone(), value.clone())];
        let target = dir.path().join("n.faris");
        write_study(&target, &study).unwrap();
        let mut reader = StudyReader::open(&target).unwrap();
        assert_eq!(reader.ensemble_for(&k).unwrap(), Some(value));
    }

    #[test]
    fn files_without_ensembles_still_open() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("old.faris");
        write_study(&target, &draft(dir.path())).unwrap();
        let mut reader = StudyReader::open(&target).unwrap();
        assert!(reader.manifest.ensembles.is_empty());
        assert!(reader.ensembles().unwrap().is_empty());
        assert_eq!(reader.ensemble_for(&key(6, 'a')).unwrap(), None);
        let tiny = Tiny::new();
        let mut reader = open_raw(&tiny.good()).unwrap();
        assert!(reader.ensembles().unwrap().is_empty());
    }

    #[test]
    fn the_writer_refuses_an_ensemble_that_does_not_match_its_key() {
        let dir = tempfile::tempdir().unwrap();
        let (k, e) = stored(6, 'a');
        let mut wrong = k;
        wrong.samples = 7;
        let mut study = draft(dir.path());
        study.ensembles = vec![ensemble_draft("reference", wrong, e)];
        let result = write_study(&dir.path().join("w.faris"), &study);
        assert!(matches!(result, Err(StudyError::Input(_))), "{result:?}");
        assert!(!dir.path().join("w.faris").exists());
    }

    /// A container holding one ensemble blob and a manifest naming it with
    /// `key`, built by hand so each part can be damaged.
    fn container(
        key: &serde_json::Value,
        blob: &[u8],
        repeat_record: bool,
    ) -> Vec<(String, Vec<u8>)> {
        let sha = sha256_hex(blob);
        let record = serde_json::json!({
            "blob": sha, "scenario_sha256": "a".repeat(64), "variant": "reference", "key": key,
        });
        let records = if repeat_record {
            vec![record.clone(), record]
        } else {
            vec![record]
        };
        let manifest = serde_json::to_vec(&serde_json::json!({
            "format": "faris-study/1",
            "ensembles": records,
            "blobs": [{"sha256": sha, "bytes": blob.len(), "media_type": ENSEMBLE_MEDIA_TYPE, "encoding": "verbatim"}],
        }))
        .unwrap();
        vec![
            ("mimetype".into(), MIMETYPE.as_bytes().to_vec()),
            ("manifest.json".into(), manifest),
            (format!("blobs/{sha}"), blob.to_vec()),
        ]
    }

    // Verifies: SEC-004, REL-021
    #[test]
    fn a_damaged_ensemble_record_refuses_the_file() {
        let (k, e) = stored(6, 'a');
        let key_json = serde_json::to_value(&k).unwrap();
        let blob = serde_json::to_vec(&e).unwrap();

        // The good hand-built container reads.
        let mut reader = open_raw(&container(&key_json, &blob, false)).unwrap();
        assert_eq!(reader.ensembles().unwrap().len(), 1);

        // The same key listed twice.
        assert!(open_raw(&container(&key_json, &blob, true)).is_err());

        // A key that disagrees with the ensemble's own sample count.
        let mut lying = key_json.clone();
        lying["samples"] = 99.into();
        let mut reader = open_raw(&container(&lying, &blob, false)).unwrap();
        assert!(matches!(reader.ensembles(), Err(StudyError::Corrupt(_))));
        assert!(reader.verify().is_err());

        // A blob that is not an ensemble, or that carries a field this reader
        // does not know, is refused rather than skipped.
        let mut reader =
            open_raw(&container(&key_json, b"{\"not\":\"an ensemble\"}", false)).unwrap();
        assert!(matches!(reader.ensembles(), Err(StudyError::Corrupt(_))));
        let mut extra: serde_json::Value = serde_json::from_slice(&blob).unwrap();
        extra["surprise"] = true.into();
        let extra = serde_json::to_vec(&extra).unwrap();
        let mut reader = open_raw(&container(&key_json, &extra, false)).unwrap();
        assert!(matches!(reader.ensembles(), Err(StudyError::Corrupt(_))));

        // A key with an unknown part or a seed that is not text.
        let mut unknown = key_json.clone();
        unknown["extra"] = 1.into();
        assert!(open_raw(&container(&unknown, &blob, false)).is_err());
        let mut numeric = key_json.clone();
        numeric["seed"] = 5.into();
        assert!(open_raw(&container(&numeric, &blob, false)).is_err());

        // A record naming a blob the file does not hold.
        let mut entries = container(&key_json, &blob, false);
        entries.pop();
        assert!(open_raw(&entries).is_err());

        // An altered blob fails its hash and is named.
        let mut entries = container(&key_json, &blob, false);
        let sha = sha256_hex(&blob);
        let last = entries.len() - 1;
        entries[last].1[0] = b' ';
        let mut reader = open_raw(&entries).unwrap();
        match reader.ensembles() {
            Err(StudyError::HashMismatch { blob, .. }) => assert_eq!(blob, sha),
            other => panic!("{other:?}"),
        }
    }
}

const BATCH_VALUES: &str = "solver/transport-batch-values.json";

/// A bundle written the way the OpenMC adapter now writes it: the optional
/// per-batch values file sits beside the other recorded outputs.
fn write_bundle_with_batch_values(dir: &Path, name: &str, tag: &str) -> PathBuf {
    let mut bundle = bundle(tag);
    bundle.files.insert(
        BATCH_VALUES.into(),
        format!("{{\"values\":{{\"flux\":[1.0,2.0,3.0]}},\"tag\":\"{tag}\"}}\n"),
    );
    let path = dir.join(format!("{name}.transport-bundle.json"));
    std::fs::write(&path, serde_json::to_vec_pretty(&bundle).unwrap()).unwrap();
    path
}

fn single_bundle_draft(bundle: PathBuf) -> StudyDraft {
    StudyDraft {
        port: Some(ArrangementDraft {
            scenario: None,
            physics: vec![],
            bundles: vec![bundle],
        }),
        zstd_level: 3,
        ..StudyDraft::default()
    }
}

#[test]
fn batch_values_are_packed_as_a_blob_and_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let original = write_bundle_with_batch_values(dir.path(), "reference", "p1");
    let target = dir.path().join("a.faris");
    let report = write_study(&target, &single_bundle_draft(original.clone())).unwrap();
    // Six required members plus the batch values file.
    assert_eq!(report.blob_count, 7);
    let mut reader = StudyReader::open(&target).unwrap();
    assert_eq!(reader.verify().unwrap(), report.blob_count);
    let record = reader.manifest.arrangements.port.as_ref().unwrap().bundles[0].clone();
    let digest = &record.files[BATCH_VALUES];
    assert_eq!(digest.len(), 64);
    let restored = reader.bundle(&record).unwrap();
    assert!(restored.files[BATCH_VALUES].contains("\"flux\""));
    assert_eq!(sha256_hex(restored.files[BATCH_VALUES].as_bytes()), *digest);
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let files = reader.materialize(&out, None).unwrap();
    assert_eq!(
        std::fs::read(&files.port.unwrap().bundles[0]).unwrap(),
        std::fs::read(&original).unwrap()
    );
}

#[test]
fn an_older_bundle_without_batch_values_still_loads() {
    let dir = tempfile::tempdir().unwrap();
    let original = write_bundle(dir.path(), "reference", "p1");
    let target = dir.path().join("old.faris");
    write_study(&target, &single_bundle_draft(original)).unwrap();
    let mut reader = StudyReader::open(&target).unwrap();
    reader.verify().unwrap();
    let record = reader.manifest.arrangements.port.as_ref().unwrap().bundles[0].clone();
    assert!(!record.files.contains_key(BATCH_VALUES));
    let restored = reader.bundle(&record).unwrap();
    assert!(!restored.files.contains_key(BATCH_VALUES));
}

fn extract_limited(
    dir: &Path,
    label: &str,
    tar: &[u8],
    limits: crate::tar::Limits,
) -> (Result<ExtractReport, StudyError>, PathBuf) {
    let archive = dir.join(format!("{label}.tar.gz"));
    gz(tar, &archive);
    let out = dir.join(format!("out-{label}"));
    std::fs::create_dir(&out).unwrap();
    (crate::tar::extract_with_limits(&archive, &out, limits), out)
}

fn small_limits() -> crate::tar::Limits {
    crate::tar::Limits {
        files: 4,
        file_bytes: 1000,
        total_bytes: 2500,
        depth: 3,
    }
}

// Verifies: PRV-044
#[test]
fn tar_limits_default_to_the_documented_production_values() {
    let l = crate::tar::Limits::default();
    assert_eq!(l.files, 4096);
    assert_eq!(l.file_bytes, 64 * 1024 * 1024);
    assert_eq!(l.total_bytes, 1536 * 1024 * 1024);
    assert_eq!(l.depth, 64);
}

// Verifies: PRV-044
#[test]
fn tar_file_count_limit_accepts_at_limit_and_refuses_one_over() {
    let dir = tempfile::tempdir().unwrap();
    let names: Vec<String> = (0..5).map(|i| format!("f{i}")).collect();
    let entries: Vec<(&str, &[u8], u8)> = names
        .iter()
        .map(|n| (n.as_str(), &b"x"[..], b'0'))
        .collect();
    let (ok, _) = extract_limited(dir.path(), "at", &ustar(&entries[..4]), small_limits());
    assert_eq!(ok.unwrap().files, 4);
    let (over, out) = extract_limited(dir.path(), "over", &ustar(&entries), small_limits());
    assert!(over.is_err());
    assert!(
        !out.join("f4").exists(),
        "the file past the limit was written"
    );
}

// Verifies: PRV-044
#[test]
fn tar_per_file_size_limit_accepts_at_limit_and_refuses_one_over() {
    let dir = tempfile::tempdir().unwrap();
    let at = vec![1u8; 1000];
    let over = vec![1u8; 1001];
    let (ok, _) = extract_limited(
        dir.path(),
        "at",
        &ustar(&[("a", &at, b'0')]),
        small_limits(),
    );
    assert_eq!(ok.unwrap().bytes, 1000);
    let (refused, out) = extract_limited(
        dir.path(),
        "over",
        &ustar(&[("a", &over, b'0')]),
        small_limits(),
    );
    assert!(refused.is_err());
    assert!(!out.join("a").exists());
}

// Verifies: PRV-044
#[test]
fn tar_total_size_limit_accepts_at_limit_and_refuses_one_over() {
    let dir = tempfile::tempdir().unwrap();
    let a = vec![1u8; 1000];
    let b = vec![2u8; 1000];
    let at = vec![3u8; 500];
    let one_over = vec![3u8; 501];
    let (ok, _) = extract_limited(
        dir.path(),
        "at",
        &ustar(&[("a", &a, b'0'), ("b", &b, b'0'), ("c", &at, b'0')]),
        small_limits(),
    );
    assert_eq!(ok.unwrap().bytes, 2500);
    let (refused, out) = extract_limited(
        dir.path(),
        "over",
        &ustar(&[("a", &a, b'0'), ("b", &b, b'0'), ("c", &one_over, b'0')]),
        small_limits(),
    );
    assert!(refused.is_err());
    assert!(!out.join("c").exists());
}

// Verifies: PRV-044
#[test]
fn tar_depth_limit_accepts_at_limit_and_refuses_one_over() {
    let dir = tempfile::tempdir().unwrap();
    let (ok, out) = extract_limited(
        dir.path(),
        "at",
        &ustar(&[("a/b/c", b"x", b'0')]),
        small_limits(),
    );
    ok.unwrap();
    assert!(out.join("a/b/c").is_file());
    let (refused, out) = extract_limited(
        dir.path(),
        "over",
        &ustar(&[("a/b/c/d", b"x", b'0')]),
        small_limits(),
    );
    assert!(refused.is_err());
    assert!(!out.join("a").exists());
}

// Verifies: PRV-044
#[test]
fn tar_gzip_bomb_is_refused_by_the_total_limit_without_expanding_it() {
    let dir = tempfile::tempdir().unwrap();
    // Eight 1 MiB members of zeros: 8 MiB expanded, a few kB compressed.
    let zeros = vec![0u8; 1 << 20];
    let names: Vec<String> = (0..8).map(|i| format!("z{i}")).collect();
    let entries: Vec<(&str, &[u8], u8)> = names
        .iter()
        .map(|n| (n.as_str(), &zeros[..], b'0'))
        .collect();
    let limits = crate::tar::Limits {
        files: 4096,
        file_bytes: 1 << 20,
        total_bytes: 3 << 20,
        depth: 64,
    };
    let (refused, out) = extract_limited(dir.path(), "bomb", &ustar(&entries), limits);
    assert!(refused.is_err());
    assert!(
        std::fs::metadata(dir.path().join("bomb.tar.gz"))
            .unwrap()
            .len()
            < 64 * 1024,
        "the synthetic archive is meant to be small"
    );
    // Exactly three members fit; the fourth is refused from its header, so
    // nothing past the limit is ever written.
    let written: u64 = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum();
    assert_eq!(written, 3 << 20);
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 3);
}

/// Bytes that begin like a PNG of the given size; the reader checks only the
/// header, so the rest is a recognisable filler.
fn fake_png(width: u32, height: u32, filler: usize) -> Vec<u8> {
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
    png.extend_from_slice(b"IHDR");
    png.extend_from_slice(&width.to_be_bytes());
    png.extend_from_slice(&height.to_be_bytes());
    png.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    png.extend((0..filler).map(|i| (i % 251) as u8 ^ 0x5a));
    png
}

#[test]
fn preview_round_trips_and_is_optional() {
    let dir = tempfile::tempdir().unwrap();
    let mut with = draft(dir.path());
    let png = fake_png(512, 341, 4000);
    with.preview_png = Some(png.clone());
    let path = dir.path().join("with.faris");
    let report = write_study(&path, &with).unwrap();
    assert_eq!(report.preview_bytes, Some(png.len() as u64));
    let mut reader = StudyReader::open(&path).unwrap();
    let preview = reader.preview().expect("preview");
    assert_eq!((preview.width, preview.height), (512, 341));
    assert_eq!(preview.png, png);
    assert!(reader.verify().is_ok());

    let without = draft(dir.path());
    let path = dir.path().join("without.faris");
    let report = write_study(&path, &without).unwrap();
    assert_eq!(report.preview_bytes, None);
    let mut reader = StudyReader::open(&path).unwrap();
    assert_eq!(reader.preview_status(), PreviewStatus::Absent);
    assert!(reader.verify().is_ok());
}

#[test]
fn unusable_preview_is_left_out_by_the_writer_without_failing_the_save() {
    let dir = tempfile::tempdir().unwrap();
    let cases = [
        ("not-a-png", b"hello".to_vec()),
        ("too-wide", fake_png(513, 100, 10)),
        ("zero", fake_png(0, 100, 10)),
        ("too-big", fake_png(100, 100, 600 * 1024)),
    ];
    for (label, bytes) in cases {
        let mut d = draft(dir.path());
        d.preview_png = Some(bytes);
        let path = dir.path().join(format!("{label}.faris"));
        let report = write_study(&path, &d).unwrap();
        assert_eq!(report.preview_bytes, None, "{label}");
        assert_eq!(
            StudyReader::open(&path).unwrap().preview_status(),
            PreviewStatus::Absent,
            "{label}"
        );
    }
}

#[test]
fn damaged_or_oversize_preview_is_ignored_and_never_refuses_the_study() {
    let dir = tempfile::tempdir().unwrap();
    let mut d = draft(dir.path());
    d.preview_png = Some(fake_png(256, 170, 3000));
    let path = dir.path().join("p.faris");
    write_study(&path, &d).unwrap();

    // Flip a filler byte inside the stored entry: the container's CRC fails.
    let mut bytes = std::fs::read(&path).unwrap();
    let marker = fake_png(256, 170, 40);
    let at = bytes
        .windows(marker.len())
        .position(|w| w == marker)
        .expect("stored preview bytes");
    bytes[at + marker.len() - 5] ^= 0xff;
    let damaged = dir.path().join("damaged.faris");
    std::fs::write(&damaged, &bytes).unwrap();
    let mut reader = StudyReader::open(&damaged).unwrap();
    assert!(matches!(
        reader.preview_status(),
        PreviewStatus::Ignored { .. }
    ));
    assert!(reader.preview().is_none());
    assert!(reader.verify().is_ok(), "the evidence is unaffected");

    // A header that is no longer a PNG: ignored, with the reason named.
    let mut bytes = std::fs::read(&path).unwrap();
    let at = bytes
        .windows(8)
        .position(|w| w == [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])
        .unwrap();
    bytes[at + 1] = b'X';
    let broken = dir.path().join("broken.faris");
    std::fs::write(&broken, &bytes).unwrap();
    let mut reader = StudyReader::open(&broken).unwrap();
    // The CRC also fails here; either way the study opens and the thumbnail is unused.
    assert!(reader.preview().is_none());
    assert!(reader.verify().is_ok());
}
