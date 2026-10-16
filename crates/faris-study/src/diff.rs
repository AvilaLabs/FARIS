//! The semantic diff of two `.faris` study files (COL-020, COL-022, COL-023, COL-024).
//!
//! The comparison is made from the manifests first: two blobs with the same SHA-256 are the same
//! bytes and are never read. Only the blobs that differ are decompressed, checked against their
//! hash by the reader, parsed and compared as values (`faris_engine::semantic_diff`), so a study
//! that changed in one place costs the size of that place, not of the file.
//!
//! What each part of a study is, for the report:
//!
//! - inputs: the scenario, the physics cases and the operating assumptions; in a recorded
//!   transport bundle, its `scenario.json`, its `input.json` and its adapter script;
//! - evidence: the Core evidence layer and a bundle's `audit.json`;
//! - results: the rest of a recorded transport bundle (`run.json`, `solver/`), and the stored
//!   history ensembles;
//! - presentation: the view the author left open, who saved the file, a bundle's notice.

use crate::{BundleRecord, EnsembleRecord, StudyError, StudyReader, sha256_hex};
use faris_engine::semantic_diff::{
    CostClass, DiffBuilder, DiffReport, DocKind, ResultEntry, ResultKind, result_changes,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Most leaf differences listed for one result file when result values are asked for.
const MAX_RESULT_VALUES: usize = 200;

#[derive(Clone, Copy, Debug, Default)]
pub struct StudyDiffOptions {
    /// Also list the leaf differences inside changed result files. Off by default: result files
    /// are megabytes of numbers, and the report already says which result changed and why.
    pub include_result_values: bool,
}

/// Compare two study files.
pub fn diff_study_files(
    a: &Path,
    b: &Path,
    options: StudyDiffOptions,
) -> Result<DiffReport, StudyError> {
    let mut ra = StudyReader::open(a)?;
    let mut rb = StudyReader::open(b)?;
    diff_readers(
        &mut ra,
        &mut rb,
        &a.display().to_string(),
        &b.display().to_string(),
        options,
    )
}

struct Pair<'a> {
    a: &'a mut StudyReader,
    b: &'a mut StudyReader,
}

impl Pair<'_> {
    fn read_json(reader: &mut StudyReader, hash: Option<&str>) -> Option<Value> {
        let bytes = reader.read_blob(hash?).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Both sides of a blob that differs, parsed as JSON; `None` for a side without it or with
    /// content that is not JSON.
    fn json_pair(&mut self, a: Option<&str>, b: Option<&str>) -> (Option<Value>, Option<Value>) {
        (Self::read_json(self.a, a), Self::read_json(self.b, b))
    }
}

/// An input document known by blob hash on each side.
fn input_blob(
    builder: &mut DiffBuilder,
    pair: &mut Pair,
    name: &str,
    kind: Option<DocKind>,
    scope: &str,
    a: Option<&str>,
    b: Option<&str>,
) {
    if a == b {
        return;
    }
    let (va, vb) = pair.json_pair(a, b);
    let readable = |hash: Option<&str>, value: &Option<Value>| hash.is_none() || value.is_some();
    if readable(a, &va) && readable(b, &vb) {
        let kind = kind
            .or_else(|| va.as_ref().or(vb.as_ref()).map(DocKind::detect))
            .unwrap_or(DocKind::Other);
        builder.input_document(name, kind, scope, va.as_ref(), vb.as_ref());
    } else {
        builder.hashed_input(
            name,
            scope,
            a,
            b,
            CostClass::TransportRun,
            "not JSON, compared by hash only",
        );
    }
}

fn diff_readers(
    ra: &mut StudyReader,
    rb: &mut StudyReader,
    a_label: &str,
    b_label: &str,
    options: StudyDiffOptions,
) -> Result<DiffReport, StudyError> {
    let mut builder = DiffBuilder::new(a_label, b_label, "study file");
    let (ma, mb) = (ra.manifest.clone(), rb.manifest.clone());
    let mut pair = Pair { a: ra, b: rb };

    // Presentation: the view, who saved the file, the format revision.
    let head = |m: &crate::Manifest| {
        json!({
            "created_by": m.created_by,
            "format": m.format,
            "evidence_mode": m.layers.evidence.as_ref().map(|e| format!("{:?}", e.mode))
                .or_else(|| m.layers.evidence_store.as_ref().map(|e| format!("{:?}", e.mode))),
        })
    };
    builder.presentation_document("study", Some(&head(&ma)), Some(&head(&mb)));
    builder.presentation_document(
        "view",
        Some(&serde_json::to_value(&ma.view).unwrap_or(Value::Null)),
        Some(&serde_json::to_value(&mb.view).unwrap_or(Value::Null)),
    );

    // Inputs.
    input_blob(
        &mut builder,
        &mut pair,
        "assumptions",
        Some(DocKind::Assumptions),
        "",
        ma.assumptions.as_deref(),
        mb.assumptions.as_deref(),
    );
    for (group, arr_a, arr_b) in [
        ("port", &ma.arrangements.port, &mb.arrangements.port),
        (
            "control",
            &ma.arrangements.control,
            &mb.arrangements.control,
        ),
    ] {
        let none = Default::default();
        let (x, y) = (
            arr_a.as_ref().unwrap_or(&none),
            arr_b.as_ref().unwrap_or(&none),
        );
        input_blob(
            &mut builder,
            &mut pair,
            &format!("{group}/scenario"),
            Some(DocKind::Scenario),
            group,
            x.scenario.as_deref(),
            y.scenario.as_deref(),
        );
        physics_blobs(&mut builder, &mut pair, group, &x.physics, &y.physics);
        bundles(
            &mut builder,
            &mut pair,
            group,
            &x.bundles,
            &y.bundles,
            options,
        );
    }
    bundles(
        &mut builder,
        &mut pair,
        "sweep",
        &ma.sweep,
        &mb.sweep,
        options,
    );

    // Evidence.
    evidence_layers(&mut builder, &mut pair, &ma, &mb);

    // Results that are stored but not bundled: the history ensembles.
    ensembles(&mut builder, &ma.ensembles, &mb.ensembles);

    // Calculated when the study opens, so there is nothing stored to compare; whether the inputs
    // above invalidate it is still reported.
    builder.result(ResultEntry {
        group: "history".into(),
        name: "operating-history".into(),
        kind: ResultKind::History,
        old_sha256: Some("recalculated on open".into()),
        new_sha256: Some("recalculated on open".into()),
        values: vec![],
    });
    Ok(builder.finish())
}

/// Physics cases are matched by the variant they are for.
fn physics_blobs(
    builder: &mut DiffBuilder,
    pair: &mut Pair,
    group: &str,
    a: &[String],
    b: &[String],
) {
    let label = |reader: &mut StudyReader, hash: &str, index: usize| -> String {
        reader
            .read_blob(hash)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|v| {
                v.get("variant_id")
                    .and_then(Value::as_str)
                    .map(String::from)
            })
            .unwrap_or_else(|| format!("#{}", index + 1))
    };
    let mut named_a: BTreeMap<String, &str> = BTreeMap::new();
    for (i, hash) in a.iter().enumerate() {
        named_a.insert(label(pair.a, hash, i), hash);
    }
    let mut named_b: BTreeMap<String, &str> = BTreeMap::new();
    for (i, hash) in b.iter().enumerate() {
        named_b.insert(label(pair.b, hash, i), hash);
    }
    let names: BTreeSet<&String> = named_a.keys().chain(named_b.keys()).collect();
    for name in names {
        input_blob(
            builder,
            pair,
            &format!("{group}/physics/{name}"),
            Some(DocKind::Physics),
            group,
            named_a.get(name).copied(),
            named_b.get(name).copied(),
        );
    }
}

fn bundle_map(list: &[BundleRecord]) -> BTreeMap<&str, &BundleRecord> {
    list.iter().map(|b| (b.name.as_str(), b)).collect()
}

fn bundles(
    builder: &mut DiffBuilder,
    pair: &mut Pair,
    group: &str,
    a: &[BundleRecord],
    b: &[BundleRecord],
    options: StudyDiffOptions,
) {
    let (map_a, map_b) = (bundle_map(a), bundle_map(b));
    let names: BTreeSet<&str> = map_a.keys().chain(map_b.keys()).copied().collect();
    for name in names {
        let (x, y) = (map_a.get(name).copied(), map_b.get(name).copied());
        if x == y {
            // Same record: same members by hash, same notice.
            push_result(builder, group, name, x, y, vec![]);
            continue;
        }
        diff_bundle(builder, pair, group, name, x, y, options);
    }
}

fn member_hash(record: Option<&BundleRecord>, member: &str) -> Option<String> {
    record.and_then(|r| r.files.get(member)).cloned()
}

/// The identity of a bundle's result files: one hash over their names and hashes.
fn result_identity(record: &BundleRecord) -> String {
    let mut text = String::new();
    for (member, hash) in &record.files {
        if is_result_member(member) {
            text.push_str(&format!("{member}:{hash}\n"));
        }
    }
    sha256_hex(text.as_bytes())
}

fn is_result_member(member: &str) -> bool {
    !matches!(member, "scenario.json" | "input.json" | "audit.json") && !member.ends_with(".py")
}

fn push_result(
    builder: &mut DiffBuilder,
    group: &str,
    name: &str,
    a: Option<&BundleRecord>,
    b: Option<&BundleRecord>,
    values: Vec<faris_engine::semantic_diff::Change>,
) {
    builder.result(ResultEntry {
        group: group.into(),
        name: name.into(),
        kind: ResultKind::Transport,
        old_sha256: a.map(result_identity),
        new_sha256: b.map(result_identity),
        values,
    });
}

fn diff_bundle(
    builder: &mut DiffBuilder,
    pair: &mut Pair,
    group: &str,
    name: &str,
    a: Option<&BundleRecord>,
    b: Option<&BundleRecord>,
    options: StudyDiffOptions,
) {
    let scope = format!("{group}/{name}");
    let members: BTreeSet<&str> = [a, b]
        .into_iter()
        .flatten()
        .flat_map(|r| r.files.keys().map(String::as_str))
        .collect();
    let mut values = Vec::new();
    for member in members {
        let (ha, hb) = (member_hash(a, member), member_hash(b, member));
        if ha == hb {
            continue;
        }
        let (ha, hb) = (ha.as_deref(), hb.as_deref());
        let document = format!("{scope}/{member}");
        match member {
            "scenario.json" => input_blob(
                builder,
                pair,
                &document,
                Some(DocKind::Scenario),
                &scope,
                ha,
                hb,
            ),
            "input.json" => input_blob(
                builder,
                pair,
                &document,
                Some(DocKind::TransportInput),
                &scope,
                ha,
                hb,
            ),
            "audit.json" => {
                let (va, vb) = pair.json_pair(ha, hb);
                if (ha.is_none() || va.is_some()) && (hb.is_none() || vb.is_some()) {
                    builder.evidence_document(&document, &scope, va.as_ref(), vb.as_ref());
                } else {
                    builder.hashed_evidence(&document, &scope, ha, hb);
                }
            }
            m if m.ends_with(".py") => builder.hashed_input(
                &document,
                &scope,
                ha,
                hb,
                CostClass::TransportRun,
                "the adapter script that ran the transport (hash only)",
            ),
            _ if options.include_result_values => {
                if let (Some(x), Some(y)) = (ha, hb) {
                    let (va, vb) = pair.json_pair(Some(x), Some(y));
                    if let (Some(va), Some(vb)) = (va, vb) {
                        values.extend(result_changes(&document, &va, &vb, MAX_RESULT_VALUES).0);
                    }
                }
            }
            _ => {}
        }
    }
    if let (Some(x), Some(y)) = (a, b) {
        builder.presentation_document(
            &format!("{scope}/record"),
            Some(&json!({"notice": x.notice, "schema_version": x.schema_version})),
            Some(&json!({"notice": y.notice, "schema_version": y.schema_version})),
        );
    }
    push_result(builder, group, name, a, b, values);
}

fn evidence_layers(
    builder: &mut DiffBuilder,
    pair: &mut Pair,
    a: &crate::Manifest,
    b: &crate::Manifest,
) {
    let _ = pair;
    let archives = |m: &crate::Manifest| -> BTreeMap<String, (String, String)> {
        m.layers
            .evidence
            .iter()
            .flat_map(|e| e.archives.iter())
            .map(|x| {
                (
                    format!("evidence/{}/{}/{:?}", x.arrangement, x.allocation, x.kind),
                    (x.arrangement.clone(), x.sha256.clone()),
                )
            })
            .collect()
    };
    let (xa, xb) = (archives(a), archives(b));
    let names: BTreeSet<&String> = xa.keys().chain(xb.keys()).collect();
    for name in names {
        let scope = xa
            .get(name)
            .or_else(|| xb.get(name))
            .map_or("", |p| p.0.as_str());
        builder.hashed_evidence(
            name,
            scope,
            xa.get(name).map(|p| p.1.as_str()),
            xb.get(name).map(|p| p.1.as_str()),
        );
    }
    let trees = |m: &crate::Manifest| -> BTreeMap<String, (String, Value)> {
        m.layers
            .evidence_store
            .iter()
            .flat_map(|e| e.trees.iter())
            .map(|t| {
                (
                    format!("evidence/{}/{}/{:?}", t.arrangement, t.allocation, t.kind),
                    (
                        t.arrangement.clone(),
                        json!({"tree": t.tree, "files": t.files}),
                    ),
                )
            })
            .collect()
    };
    let (ta, tb) = (trees(a), trees(b));
    let names: BTreeSet<&String> = ta.keys().chain(tb.keys()).collect();
    for name in names {
        let scope = ta
            .get(name)
            .or_else(|| tb.get(name))
            .map_or("", |p| p.0.as_str());
        builder.evidence_document(
            name,
            scope,
            ta.get(name).map(|p| &p.1),
            tb.get(name).map(|p| &p.1),
        );
    }
    // Whether Core receipts exist for an arrangement is judged with the results.
    let arrangements: BTreeSet<String> = xa
        .values()
        .chain(xb.values())
        .map(|p| p.0.clone())
        .chain(ta.values().chain(tb.values()).map(|p| p.0.clone()))
        .collect();
    for arrangement in arrangements {
        builder.result(ResultEntry {
            group: arrangement,
            name: "core-receipts".into(),
            kind: ResultKind::EvidenceReceipt,
            old_sha256: Some("receipts".into()),
            new_sha256: Some("receipts".into()),
            values: vec![],
        });
    }
}

fn ensembles(builder: &mut DiffBuilder, a: &[EnsembleRecord], b: &[EnsembleRecord]) {
    let named = |list: &[EnsembleRecord]| -> BTreeMap<String, String> {
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        let mut out = BTreeMap::new();
        for record in list {
            let n = seen.entry(record.variant.as_str()).or_default();
            *n += 1;
            let name = if *n == 1 {
                record.variant.clone()
            } else {
                format!("{}#{}", record.variant, *n)
            };
            out.insert(name, record.blob.clone());
        }
        out
    };
    let (xa, xb) = (named(a), named(b));
    let names: BTreeSet<&String> = xa.keys().chain(xb.keys()).collect();
    for name in names {
        builder.result(ResultEntry {
            group: "ensembles".into(),
            name: name.clone(),
            kind: ResultKind::Ensemble,
            old_sha256: xa.get(name).cloned(),
            new_sha256: xb.get(name).cloned(),
            values: vec![],
        });
    }
}

#[cfg(test)]
mod tests;
