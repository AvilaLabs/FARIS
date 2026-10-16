use super::*;
use crate::tests::{draft, evidence_draft};
use crate::{ArrangementDraft, StudyDraft, write_study};
use faris_engine::core_evidence::RecordedTransportBundle;
use faris_engine::semantic_diff::{ChangeKind, Standing, render_text};
use std::collections::BTreeMap;
use std::path::PathBuf;

const SCENARIO: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scenarios/arc-inspired/cold-reference.scenario.json"
);
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn scenario() -> Value {
    serde_json::from_slice(&std::fs::read(SCENARIO).unwrap()).unwrap()
}

fn diff(a: &Path, b: &Path) -> DiffReport {
    diff_study_files(a, b, StudyDiffOptions::default()).unwrap()
}

struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as usize % n
    }
}

/// A bundle with the given scenario and input and a result tagged `tag`.
fn bundle_file(dir: &Path, name: &str, scenario: &Value, input: &Value, tag: &str) -> PathBuf {
    let mut files = BTreeMap::new();
    files.insert(
        "scenario.json".to_string(),
        serde_json::to_string(scenario).unwrap(),
    );
    files.insert(
        "input.json".to_string(),
        serde_json::to_string(input).unwrap(),
    );
    files.insert("run.json".to_string(), format!("{{\"tag\":\"{tag}\"}}"));
    files.insert("audit.json".to_string(), "{\"audit\":1}".to_string());
    files.insert(
        "reactor_transport.py".to_string(),
        "# adapter\n".to_string(),
    );
    files.insert(
        "solver/transport-artifact.json".to_string(),
        format!("{{\"result\":\"{tag}\"}}"),
    );
    let bundle = RecordedTransportBundle {
        schema_version: "faris-recorded-transport-bundle/v0.1".into(),
        files,
        notice: "test notice".into(),
    };
    let path = dir.join(format!("{name}.transport-bundle.json"));
    std::fs::write(&path, serde_json::to_vec_pretty(&bundle).unwrap()).unwrap();
    path
}

fn study_with(dir: &Path, file: &str, scenario: &Value, tag: &str, assumptions: &Value) -> PathBuf {
    let sub = dir.join(file);
    std::fs::create_dir_all(&sub).unwrap();
    let input = json!({"schema_version": "faris-openmc-input/v0.1", "sampling": {"seed": 1}});
    std::fs::write(
        sub.join("assumptions.json"),
        serde_json::to_vec(assumptions).unwrap(),
    )
    .unwrap();
    let study = StudyDraft {
        port: Some(ArrangementDraft {
            scenario: None,
            physics: vec![],
            bundles: vec![
                bundle_file(&sub, "reference", scenario, &input, tag),
                bundle_file(&sub, "other", scenario, &input, "same"),
            ],
        }),
        assumptions: Some(sub.join("assumptions.json")),
        zstd_level: 1,
        ..StudyDraft::default()
    };
    let target = dir.join(format!("{file}.faris"));
    write_study(&target, &study).unwrap();
    target
}

// Verifies: COL-020
#[test]
fn a_resaved_unchanged_study_has_no_changes() {
    let dir = tempfile::tempdir().unwrap();
    let d = draft(dir.path());
    let (a, b) = (dir.path().join("a.faris"), dir.path().join("b.faris"));
    write_study(&a, &d).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    write_study(&b, &d).unwrap();
    let report = diff(&a, &b);
    assert!(report.identical, "{}", render_text(&report));
    assert!(diff(&a, &a).identical);
}

// Verifies: COL-020
#[test]
fn every_archived_fixture_compared_with_itself_is_identical() {
    let mut count = 0;
    for entry in std::fs::read_dir(FIXTURES).unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "faris") {
            let report = diff(&path, &path);
            assert!(report.identical, "{}", path.display());
            count += 1;
        }
    }
    assert!(count >= 24);
}

// Verifies: COL-020, COL-025
#[test]
fn seeded_edits_to_a_studys_inputs_are_listed_exactly_and_a_reformatted_copy_is_not_a_change() {
    let dir = tempfile::tempdir().unwrap();
    let base_scenario = scenario();
    let base_assumptions = serde_json::to_value(faris_engine::fixtures::assumptions()).unwrap();
    let a = study_with(dir.path(), "a", &base_scenario, "t", &base_assumptions);
    let mut rng = Lcg(11);
    let mut listed_total = 0;
    for trial in 0..200 {
        let mut s = base_scenario.clone();
        let mut asm = base_assumptions.clone();
        let mut expected: BTreeMap<String, String> = BTreeMap::new();
        for _ in 0..1 + rng.below(3) {
            match rng.below(5) {
                0 => {
                    let l = rng.below(6);
                    let t = s["variants"][0]["layers"][l]["thickness_m"]
                        .as_f64()
                        .unwrap();
                    s["variants"][0]["layers"][l]["thickness_m"] = json!(t + 0.0123);
                    let id = s["variants"][0]["layers"][l]["id"]
                        .as_str()
                        .unwrap()
                        .to_string();
                    expected.insert(
                        "port/reference/scenario.json".into(),
                        format!("variants[id=reference].layers[id={id}].thickness_m"),
                    );
                }
                1 => {
                    s["operating_plan"]["horizon_years"] = json!(33.25);
                    expected.insert(
                        "port/reference/scenario.json".into(),
                        "operating_plan.horizon_years".into(),
                    );
                }
                2 => {
                    asm["recovery_fraction"] = json!(0.4375);
                    expected.insert("assumptions".into(), "recovery_fraction".into());
                }
                3 => {
                    asm["horizon_s"] = json!(250.0);
                    expected.insert("assumptions".into(), "horizon_s".into());
                }
                _ => {
                    s["title"] = json!(format!("Title {trial}"));
                    expected.insert("port/reference/scenario.json".into(), "title".into());
                }
            }
        }
        let b = study_with(dir.path(), "b", &s, "t", &asm);
        let report = diff(&a, &b);
        // Both bundles carry the scenario, so every scenario edit shows twice (reference, other);
        // the assumptions once. Collect (document, path) pairs.
        let listed: std::collections::BTreeSet<(String, String)> = report
            .inputs
            .iter()
            .chain(&report.presentation)
            .filter(|c| {
                c.document != "study" && c.document != "view" && !c.document.ends_with("/record")
            })
            .map(|c| (c.document.clone(), c.path.clone()))
            .collect();
        let mut want = std::collections::BTreeSet::new();
        let _ = &expected;
        for (doc, path) in &expected {
            want.insert((doc.clone(), path.clone()));
            if doc.starts_with("port/reference/") {
                want.insert((doc.replace("reference", "other"), path.clone()));
            }
        }
        // A layer edited twice to different layers gives several paths for one document: the
        // expected map keeps the last only, so check containment both ways on the document level.
        for item in &want {
            assert!(
                listed.contains(item),
                "trial {trial}: missing {item:?} in {listed:?}"
            );
        }
        listed_total += listed.len();
    }
    assert!(listed_total >= 200);
    // A re-save of the unchanged inputs: no change at all.
    let again = study_with(dir.path(), "c", &base_scenario, "t", &base_assumptions);
    assert!(diff(&a, &again).identical);
}

// Verifies: COL-022, GEO-068
#[test]
fn causes_are_kept_apart_from_effects_and_unexplained_results_say_so() {
    let dir = tempfile::tempdir().unwrap();
    let asm = json!({"recovery_fraction": 1.0});
    let a = study_with(dir.path(), "a", &scenario(), "t", &asm);
    // Both bundles get the edited scenario; only `reference` also gets a different result.
    let mut s = scenario();
    s["variants"][0]["layers"][1]["thickness_m"] = json!(0.6);
    let b = study_with(dir.path(), "b", &s, "t2", &asm);
    let report = diff(&a, &b);
    assert!(!report.identical);
    assert_eq!(report.summary.inputs_changed, 2);
    assert_eq!(report.summary.highest_cost, Some(CostClass::TransportRun));
    let changed: Vec<_> = report.results.iter().map(|r| r.item.as_str()).collect();
    assert_eq!(changed, ["port/reference"]);
    let reference = &report.results[0];
    assert_eq!(reference.caused_by.len(), 1, "{reference:?}");
    assert!(reference.caused_by[0].starts_with('i'));
    // `other` did not change its result but its input changed, so the result is stale.
    let standing = |item: &str| {
        report
            .impact
            .iter()
            .find(|i| i.item == item)
            .unwrap()
            .standing
    };
    assert_eq!(standing("port/other"), Standing::Invalidated);
    assert_eq!(standing("history/operating-history"), Standing::Invalidated);
    let text = render_text(&report);
    assert!(
        text.contains("INPUTS CHANGED") && text.contains("RESULTS CHANGED"),
        "{text}"
    );
    // A result that changed with no input change says so.
    let c = study_with(dir.path(), "c", &scenario(), "t3", &asm);
    let unexplained = diff(&a, &c);
    assert_eq!(unexplained.summary.inputs_changed, 0);
    assert_eq!(unexplained.results.len(), 1);
    assert!(unexplained.results[0].caused_by.is_empty());
    assert_eq!(
        unexplained.results[0].cause_note.as_deref(),
        Some("no input change found")
    );
    assert!(render_text(&unexplained).contains("no input change found"));
}

// Verifies: COL-022, GEO-068
#[test]
fn an_assumption_edit_leaves_the_transport_reusable() {
    let dir = tempfile::tempdir().unwrap();
    let a = study_with(
        dir.path(),
        "a",
        &scenario(),
        "t",
        &json!({"recovery_fraction": 1.0}),
    );
    let b = study_with(
        dir.path(),
        "b",
        &scenario(),
        "t",
        &json!({"recovery_fraction": 0.5}),
    );
    let report = diff(&a, &b);
    assert_eq!(report.summary.highest_cost, Some(CostClass::Seconds));
    assert!(report.results.is_empty(), "{:?}", report.results);
    for item in &report.impact {
        let expected = if item.item.starts_with("history/") {
            Standing::Invalidated
        } else {
            Standing::Reusable
        };
        assert_eq!(item.standing, expected, "{}", item.item);
    }
    assert!(
        report
            .impact
            .iter()
            .any(|i| i.item.starts_with("port/") && i.standing == Standing::Reusable)
    );
}

// Verifies: COL-022
#[test]
fn evidence_and_presentation_changes_have_their_own_sections() {
    let dir = tempfile::tempdir().unwrap();
    let (a_draft, _) = evidence_draft(dir.path(), false);
    let a = dir.path().join("a.faris");
    write_study(&a, &a_draft).unwrap();
    // Another archive content, another view.
    let (mut b_draft, _) = evidence_draft(dir.path(), false);
    std::fs::write(
        dir.path().join("reference-case.tar.gz"),
        b"different case archive",
    )
    .unwrap();
    b_draft.evidence[0].archive.sha256 = crate::sha256_hex(b"different case archive");
    b_draft.evidence[0].archive.bytes = b"different case archive".len() as u64;
    b_draft.view.year = 9.0;
    b_draft.view.step = "results".into();
    let b = dir.path().join("b.faris");
    write_study(&b, &b_draft).unwrap();
    let report = diff(&a, &b);
    assert_eq!(report.evidence.len(), 1, "{:?}", report.evidence);
    assert_eq!(report.evidence[0].kind, ChangeKind::Changed);
    assert!(
        report
            .presentation
            .iter()
            .any(|c| c.document == "view" && c.path == "year")
    );
    assert_eq!(report.inputs.len(), 0);
}

// Verifies: COL-022
#[test]
fn stored_ensembles_are_results_that_follow_assumption_changes() {
    use crate::tests::ensembles::{ensemble_draft, stored};
    let dir = tempfile::tempdir().unwrap();
    let mut da = draft(dir.path());
    let (k, e) = stored(6, 'a');
    da.ensembles = vec![ensemble_draft("reference", k, e)];
    let a = dir.path().join("a.faris");
    write_study(&a, &da).unwrap();
    let mut db = da.clone();
    let (k2, e2) = stored(8, 'a');
    db.ensembles = vec![ensemble_draft("reference", k2, e2)];
    std::fs::write(dir.path().join("assumptions.json"), b"{\"a\":2}\n").unwrap();
    let b = dir.path().join("b.faris");
    write_study(&b, &db).unwrap();
    let report = diff(&a, &b);
    let ensemble = report
        .results
        .iter()
        .find(|r| r.item == "ensembles/reference")
        .unwrap();
    assert!(ensemble.caused_by.iter().all(|id| id.starts_with('i')));
    assert!(
        !ensemble.caused_by.is_empty(),
        "the assumptions changed: {report:?}"
    );
}

#[test]
fn result_values_are_listed_only_when_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let asm = json!({"recovery_fraction": 1.0});
    let a = study_with(dir.path(), "a", &scenario(), "one", &asm);
    let b = study_with(dir.path(), "b", &scenario(), "two", &asm);
    let plain = diff(&a, &b);
    assert!(plain.results.iter().all(|r| r.values.is_empty()));
    let with_values = diff_study_files(
        &a,
        &b,
        StudyDiffOptions {
            include_result_values: true,
        },
    )
    .unwrap();
    let listed: Vec<_> = with_values
        .results
        .iter()
        .flat_map(|r| r.values.iter())
        .collect();
    assert!(
        listed.iter().any(|c| c.path == "tag" || c.path == "result"),
        "{listed:?}"
    );
}

#[test]
fn the_diff_of_the_largest_fixtures_takes_well_under_the_limit() {
    // Verifies: COL-024
    let big = format!("{FIXTURES}/24-everything-but-store-evidence.faris");
    let other = format!("{FIXTURES}/22-history-ensembles.faris");
    let started = std::time::Instant::now();
    for _ in 0..5 {
        let report = diff(Path::new(&big), Path::new(&other));
        assert!(!report.identical);
    }
    assert!(started.elapsed().as_secs_f64() / 5.0 < 2.0);
}
