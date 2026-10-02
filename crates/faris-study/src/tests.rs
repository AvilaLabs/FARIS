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

#[test]
fn first_entry_is_the_stored_type_string_at_offset_thirty() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("a.faris");
    write_study(&target, &draft(dir.path())).unwrap();
    let bytes = std::fs::read(&target).unwrap();
    let magic = format!("mimetype{MIMETYPE}");
    assert_eq!(&bytes[30..30 + magic.len()], magic.as_bytes());
}

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

#[test]
fn the_hand_built_container_reads_and_ignores_unknown_manifest_fields() {
    let tiny = Tiny::new();
    let mut reader = open_raw(&tiny.good()).unwrap();
    assert_eq!(reader.verify().unwrap(), 1);
    assert_eq!(reader.read_blob(&tiny.sha).unwrap(), tiny.content);
}

#[test]
fn a_minor_version_is_accepted() {
    let tiny = Tiny::new();
    let e = tiny.entries(tiny.manifest("faris-study/1.4", tiny.content.len() as u64, "verbatim"));
    open_raw(&e).unwrap();
}

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

#[test]
fn declared_totals_beyond_the_bound_are_refused() {
    let tiny = Tiny::new();
    let e = tiny.entries(tiny.manifest("faris-study/1", 5 * 1024 * 1024 * 1024, "verbatim"));
    assert!(open_raw(&e).is_err());
}

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
