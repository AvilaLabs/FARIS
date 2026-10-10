use super::*;
use serde_json::{Value, json};
use std::cell::Cell;

const PASS: &str =
    include_str!("../../../faris-model/tests/fixtures/design/pass.faris-design.json");
const INSPECT: &str = include_str!("../../../faris-model/tests/fixtures/design/pass.inspect.json");
const HISTORY: &str =
    include_str!("../../../faris-model/tests/fixtures/design/pass.operating-history.json");
const STEP_BYTES: &[u8] = b"ISO-10303-21; stand-in for a STEP file;";

#[derive(Clone, Copy)]
enum Audit {
    Pass,
    NotEvaluated,
    MissingNuclide,
    BadElement,
}

struct Fake {
    inspection: StepInspection,
    audit: Audit,
    inspect_calls: Cell<u32>,
    audit_calls: Cell<u32>,
    last_assumed: Cell<Option<LengthUnit>>,
}

impl Fake {
    fn new() -> Self {
        Self::with(serde_json::from_str(INSPECT).unwrap())
    }
    fn with(mut inspection: StepInspection) -> Self {
        inspection.step.sha256 = sha256_hex(STEP_BYTES);
        Self {
            inspection,
            audit: Audit::Pass,
            inspect_calls: Cell::new(0),
            audit_calls: Cell::new(0),
            last_assumed: Cell::new(None),
        }
    }
}

impl Backends for Fake {
    fn inspect_step(
        &self,
        _step: &Path,
        assume: Option<LengthUnit>,
    ) -> Result<InspectOutcome, DesignError> {
        self.inspect_calls.set(self.inspect_calls.get() + 1);
        self.last_assumed.set(assume);
        Ok(InspectOutcome {
            inspection: self.inspection.clone(),
            helper_sha256: "h".repeat(64),
            elapsed_seconds: 1.5,
        })
    }
    fn audit_materials(&self, materials: &[AuditMaterial]) -> Result<AuditOutcome, DesignError> {
        self.audit_calls.set(self.audit_calls.get() + 1);
        if matches!(self.audit, Audit::NotEvaluated) {
            return Ok(AuditOutcome::NotEvaluated {
                why: "no OpenMC interpreter is configured".into(),
                next_step: "Pass --openmc-python and --cross-sections.".into(),
            });
        }
        let mut report = MaterialAuditReport {
            schema: "faris-material-audit/v1".into(),
            openmc: "0.15.3".into(),
            cross_sections_sha256: Some("c".repeat(64)),
            materials: Vec::new(),
            nuclides: BTreeMap::new(),
            photon_elements: BTreeMap::new(),
        };
        for material in materials {
            let (id, nuclides): (&str, BTreeMap<String, f64>) = match material {
                AuditMaterial::Nuclides { id, atom_fractions } => (id, atom_fractions.clone()),
                AuditMaterial::Recipe { id, components, .. } => (
                    id,
                    components
                        .iter()
                        .map(|c| (c.element.clone().or(c.nuclide.clone()).unwrap(), c.fraction))
                        .collect(),
                ),
            };
            let bad_element = matches!(self.audit, Audit::BadElement) && id == "magnet-pack";
            report.materials.push(AuditedMaterial {
                id: id.to_string(),
                nuclides: (!bad_element).then(|| nuclides.clone()),
                error: bad_element.then(|| "ValueError: Element name \"Xx\" not recognised".into()),
            });
            if !bad_element {
                for name in nuclides.keys() {
                    let missing = matches!(self.audit, Audit::MissingNuclide) && name == "W184";
                    report.nuclides.insert(
                        name.clone(),
                        AuditedNuclide {
                            present: !missing,
                            readable: !missing,
                            temperatures_k: vec![294.0],
                            error: missing.then(|| "no entry in cross_sections.xml".into()),
                        },
                    );
                }
            }
        }
        Ok(AuditOutcome::Evaluated {
            report,
            elapsed_seconds: 2.0,
        })
    }
}

/// A directory holding the STEP stand-in, the operating history and the fixture
/// design with its two hashes filled in.
struct Case {
    dir: tempfile::TempDir,
    design: PathBuf,
}

impl Case {
    fn new() -> Self {
        Self::with(|_| {})
    }
    fn with(edit: impl FnOnce(&mut Value)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("fixture.step"), STEP_BYTES).unwrap();
        std::fs::write(dir.path().join("fixture.operating-history.json"), HISTORY).unwrap();
        let mut value: Value = serde_json::from_str(PASS).unwrap();
        value["cad"]["step_sha256"] = json!(sha256_hex(STEP_BYTES));
        value["operating_scenario"]["sha256"] = json!(sha256_hex(HISTORY.as_bytes()));
        edit(&mut value);
        let design = dir.path().join("fixture.faris-design.json");
        std::fs::write(&design, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        Self { dir, design }
    }
    fn check(&self, backends: &Fake) -> ImportReport {
        check_design(&self.design, backends).unwrap()
    }
}

fn statuses(report: &ImportReport) -> Vec<(u8, CheckStatus)> {
    report.checks.iter().map(|c| (c.check, c.status)).collect()
}

use CheckStatus::{Fail, NotEvaluated, Pass};

#[test]
// Verifies: GEO-019, GEO-039
fn a_good_design_passes_checks_1_to_5_and_lists_the_rest_as_not_evaluated() {
    let case = Case::new();
    let fake = Fake::new();
    let report = case.check(&fake);
    assert_eq!(
        statuses(&report),
        [
            (1, Pass),
            (2, Pass),
            (3, Pass),
            (4, Pass),
            (5, Pass),
            (6, NotEvaluated),
            (7, NotEvaluated),
            (8, NotEvaluated),
            (9, NotEvaluated),
        ]
    );
    for check in &report.checks[5..] {
        assert_eq!(check.why.as_deref(), Some("implemented in stage S1b"));
        assert!(check.next_step.is_some());
    }
    assert!(report.evaluated_checks_pass);
    assert!(!report.import_complete, "conversion does not exist yet");
    assert_eq!(report.schema_version, IMPORT_REPORT_VERSION);
    assert_eq!(report.design.id.as_deref(), Some("fixture"));
    assert_eq!(report.void_solids.len(), 1);
    assert_eq!(report.checks[4].sub_checks.len(), 2);
    assert_eq!(fake.last_assumed.get(), Some(LengthUnit::Mm));
    let tool = report.cad_tool.as_ref().unwrap();
    assert_eq!(tool.occt.as_deref(), Some("7.9"));
    assert_eq!(tool.tolerances.volume_relative, 1e-6);
    assert_eq!(report.step.as_ref().unwrap().solids, Some(4));
    // the audit saw all three materials, catalog ones as ready vectors
    assert_eq!(report.material_audit.as_ref().unwrap().materials.len(), 3);
    // the report is a JSON document that reads back
    let text = serde_json::to_string(&report).unwrap();
    let back: ImportReport = serde_json::from_str(&text).unwrap();
    assert_eq!(back, report);
    let readable = render_text(&report);
    assert!(readable.contains("Check 1  PASS"));
    assert!(readable.contains("Check 6  NOT_EVALUATED"));
    assert!(readable.contains("implemented in stage S1b"));
    assert!(readable.contains("Void solids"));
}

#[test]
fn check_1_failure_lists_every_null_and_stops_before_the_helper_runs() {
    let case = Case::with(|v| {
        v["solids"][1]["material_id"] = Value::Null;
        v["solids"][2]["role"] = Value::Null;
        v["plasma"]["fusion_power_mw"] = Value::Null;
    });
    let fake = Fake::new();
    let report = case.check(&fake);
    assert_eq!(report.checks[0].status, Fail);
    let items: Vec<&str> = report.checks[0]
        .findings
        .iter()
        .map(|f| f.item.as_str())
        .collect();
    assert_eq!(
        items,
        [
            "plasma.fusion_power_mw",
            "solids[1].material_id",
            "solids[2].role"
        ]
    );
    assert!(report.checks[1..5].iter().all(|c| c.status == NotEvaluated));
    assert!(
        report.checks[1]
            .why
            .as_ref()
            .unwrap()
            .contains("check 1 failed")
    );
    assert_eq!(fake.inspect_calls.get(), 0, "the CAD helper is not run");
    assert!(!report.evaluated_checks_pass);
    assert!(report.summary.starts_with("FAIL: check 1"));
}

#[test]
fn an_unknown_key_stops_at_check_1() {
    let case = Case::with(|v| v["bogus"] = json!(1));
    let report = case.check(&Fake::new());
    assert_eq!(report.checks[0].status, Fail);
    assert!(
        report.checks[0].findings[0]
            .why
            .contains("unknown field `bogus`")
    );
    assert_eq!(report.design.id, None);
}

#[test]
// Verifies: GEO-019
fn hash_mismatch_fails_check_2_without_running_the_helper() {
    let case = Case::with(|v| v["cad"]["step_sha256"] = json!("1".repeat(64)));
    let fake = Fake::new();
    let report = case.check(&fake);
    assert_eq!(report.checks[1].status, Fail);
    assert_eq!(report.checks[1].findings[0].item, "cad.step_sha256");
    assert_eq!(fake.inspect_calls.get(), 0);
    assert!(report.checks[2..5].iter().all(|c| c.status == NotEvaluated));
}

#[test]
fn a_missing_step_file_is_a_check_2_finding() {
    let case = Case::new();
    std::fs::remove_file(case.dir.path().join("fixture.step")).unwrap();
    let report = case.check(&Fake::new());
    assert_eq!(report.checks[1].status, Fail);
    assert!(report.checks[1].findings[0].why.contains("cannot be read"));
}

#[test]
// Verifies: GEO-011
fn unit_mismatch_sector_axis_and_multiple_units_fail_check_2() {
    let case = Case::with(|v| {
        v["cad"]["length_unit"] = json!("m");
        v["cad"]["extent"]["kind"] = json!("sector");
        v["cad"]["machine_axis"] = json!("y");
    });
    let report = case.check(&Fake::new());
    let items: Vec<&str> = report.checks[1]
        .findings
        .iter()
        .map(|f| f.item.as_str())
        .collect();
    assert_eq!(items, ["cad.extent", "cad.machine_axis", "cad.length_unit"]);
    let mut multiple = serde_json::from_str::<StepInspection>(INSPECT).unwrap();
    multiple.unit.problems = vec!["the file declares several different length units".into()];
    let report = Case::new().check(&Fake::with(multiple));
    assert_eq!(report.checks[1].status, Fail);
    assert!(
        report.checks[1].findings[0]
            .why
            .contains("several different length units")
    );
}

#[test]
// Verifies: GEO-011
fn a_file_with_no_declared_unit_is_read_in_the_design_unit_and_the_report_says_so() {
    let mut none = serde_json::from_str::<StepInspection>(INSPECT).unwrap();
    none.unit.declared = None;
    none.unit.used_is_assumed = true;
    let report = Case::new().check(&Fake::with(none));
    assert_eq!(report.checks[1].status, Pass);
    assert!(
        report
            .notices
            .iter()
            .any(|n| n.message.contains("declares no length unit"))
    );
    assert_eq!(
        report.step.as_ref().unwrap().unit_declared_by_file,
        Some(false)
    );
    assert!(render_text(&report).contains("the file declares none"));
}

#[test]
fn operating_scenario_hash_and_service_limits_fail_check_2() {
    let case = Case::with(|v| v["operating_scenario"]["sha256"] = json!("2".repeat(64)));
    let report = case.check(&Fake::new());
    assert_eq!(
        report.checks[1].findings[0].item,
        "operating_scenario.sha256"
    );
    // a real history with service limits
    let with_limits =
        include_str!("../../../../scenarios/arc-inspired/demo-operating-assumptions.json");
    let case = Case::with(|v| {
        v["operating_scenario"]["sha256"] = json!(sha256_hex(with_limits.as_bytes()));
    });
    std::fs::write(
        case.dir.path().join("fixture.operating-history.json"),
        with_limits,
    )
    .unwrap();
    let report = case.check(&Fake::new());
    assert_eq!(report.checks[1].status, Fail);
    assert!(report.checks[1].findings[0].why.contains("service_limits"));
    // a missing history file
    let case = Case::new();
    std::fs::remove_file(case.dir.path().join("fixture.operating-history.json")).unwrap();
    let report = case.check(&Fake::new());
    assert!(
        report.checks[1].findings[0]
            .why
            .contains("could not be read")
    );
}

#[test]
// Verifies: GEO-010
fn fingerprint_failure_in_volume_and_in_centroid_fails_check_3() {
    let case = Case::with(|v| {
        v["solids"][1]["fingerprint"]["cad_volume_m3"] = json!(2.6);
        v["solids"][2]["fingerprint"]["centroid_m"] = json!([0.0, 1.5, 0.0]);
    });
    let report = case.check(&Fake::new());
    assert_eq!(report.checks[2].status, Fail);
    let items: Vec<&str> = report.checks[2]
        .findings
        .iter()
        .map(|f| f.item.as_str())
        .collect();
    assert_eq!(
        items,
        [
            "solids[1] (first-wall).fingerprint.cad_volume_m3",
            "solids[2] (blanket).fingerprint.centroid_m"
        ]
    );
    assert!(report.checks[3..5].iter().all(|c| c.status == NotEvaluated));
}

#[test]
// Verifies: GEO-010
fn solid_count_mismatch_fails_check_3() {
    let case = Case::with(|v| {
        v["solids"].as_array_mut().unwrap().pop();
    });
    let report = case.check(&Fake::new());
    assert_eq!(report.checks[2].status, Fail);
    assert!(
        report.checks[2].findings[0]
            .why
            .contains("4 solids and the design file lists 3")
    );
}

#[test]
fn id_and_reference_problems_fail_check_4_and_stop_check_5() {
    let case = Case::with(|v| {
        v["solids"][1]["id"] = json!("Vacuum");
        v["solids"][3]["replacement_group_id"] = json!("nope");
    });
    let fake = Fake::new();
    let report = case.check(&fake);
    assert_eq!(report.checks[3].status, Fail);
    assert_eq!(report.checks[4].status, NotEvaluated);
    assert_eq!(fake.audit_calls.get(), 0);
}

#[test]
// Verifies: GEO-039
fn role_rules_fail_check_5_and_skip_the_audit() {
    let case = Case::with(|v| {
        v["solids"][2]["material_id"] = json!("void");
    });
    let fake = Fake::new();
    let report = case.check(&fake);
    assert_eq!(report.checks[4].status, Fail);
    assert_eq!(
        fake.audit_calls.get(),
        0,
        "no point auditing a design that fails the rules"
    );
    assert_eq!(report.checks[4].sub_checks[1].status, NotEvaluated);
    assert_eq!(
        report.void_solids.len(),
        2,
        "void solids are listed even on failure"
    );
}

#[test]
fn an_audit_that_cannot_run_is_not_evaluated_never_pass() {
    let mut fake = Fake::new();
    fake.audit = Audit::NotEvaluated;
    let report = Case::new().check(&fake);
    assert_eq!(report.checks[4].status, NotEvaluated);
    assert_eq!(report.checks[4].sub_checks[0].status, Pass);
    assert_eq!(report.checks[4].sub_checks[1].status, NotEvaluated);
    assert!(
        report.checks[4]
            .why
            .as_ref()
            .unwrap()
            .contains("no OpenMC interpreter")
    );
    assert!(report.evaluated_checks_pass, "nothing failed");
    assert!(report.summary.contains("could not be fully evaluated"));
    assert_eq!(report.material_audit.unwrap().status, NotEvaluated);
}

#[test]
fn a_nuclide_missing_from_the_library_fails_check_5() {
    let mut fake = Fake::new();
    fake.audit = Audit::MissingNuclide;
    let report = Case::new().check(&fake);
    assert_eq!(report.checks[4].status, Fail);
    let finding = &report.checks[4].findings[0];
    assert!(finding.item.contains("tungsten") && finding.item.contains("W184"));
    assert!(finding.why.contains("missing in the cross-section library"));
    assert!(finding.next_step.contains("audited library"));
    assert_eq!(report.checks[4].sub_checks[1].status, Fail);
}

#[test]
fn an_unknown_element_fails_check_5() {
    let mut fake = Fake::new();
    fake.audit = Audit::BadElement;
    let report = Case::new().check(&fake);
    assert_eq!(report.checks[4].status, Fail);
    assert!(
        report.checks[4].findings[0]
            .why
            .contains("could not expand")
    );
}

// ---- design init ------------------------------------------------------------

fn draft_inspection() -> StepInspection {
    let mut i: StepInspection = serde_json::from_str(INSPECT).unwrap();
    i.solids[0].step_name = Some("Plasma".into());
    i.solids[1].step_name = Some("vacuum".into());
    i.solids[2].step_name = Some("Plasma!".into());
    i.solids[3].step_name = None;
    i
}

#[test]
fn the_draft_holds_every_solid_with_null_material_and_role_and_safe_unique_ids() {
    let draft = build_draft("m.step", "My Model", &draft_inspection(), LengthUnit::Mm).unwrap();
    let ids: Vec<&str> = draft.solids.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["plasma", "solid-001", "plasma-2", "solid-003"]);
    assert_eq!(draft.id, "my-model");
    assert!(
        draft
            .solids
            .iter()
            .all(|s| s.material_id.is_none() && s.role.is_none())
    );
    assert!(
        draft
            .solids
            .iter()
            .all(|s| s.replacement_group_id.is_none() && s.tally)
    );
    assert_eq!(draft.solids[0].fingerprint.cad_volume_m3, 10.0);
    assert_eq!(draft.solids[0].step_name.as_deref(), Some("Plasma"));
    assert!(draft.materials.is_empty() && draft.replacement_groups.is_empty());
    assert_eq!(draft.cad.machine_axis, "z");
    assert_eq!(draft.cad.extent.kind, "full");
    assert_eq!(draft.cad.step_sha256, draft_inspection().step.sha256);
    assert!(draft.plasma.major_radius_m.is_none());
}

#[test]
fn init_writes_the_draft_and_never_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let step = dir.path().join("machine one.step");
    std::fs::write(&step, STEP_BYTES).unwrap();
    let fake = Fake::new();
    let summary = design_init(&step, None, None, &fake).unwrap();
    assert_eq!(
        summary.path,
        dir.path()
            .canonicalize()
            .unwrap()
            .join("machine one.faris-design.json")
    );
    assert_eq!(summary.solids, 4);
    assert!(summary.unit_declared_by_file);
    // 4 material_id + 4 role + the plasma block + its provenance + 2 scenario fields
    assert!(summary.nulls_to_fill > 20, "{}", summary.nulls_to_fill);
    assert!(summary.line().contains("DRAFT"));
    let written: Value = serde_json::from_slice(&std::fs::read(&summary.path).unwrap()).unwrap();
    assert_eq!(written["cad"]["step_file"], "machine one.step");
    assert_eq!(written["solids"][0]["material_id"], Value::Null);
    assert_eq!(written["materials"], json!([]));
    assert_eq!(written["plasma"]["ion_density_m3"]["centre"], Value::Null);
    assert_eq!(written["operating_scenario"]["path"], Value::Null);
    // a second run refuses
    let again = design_init(&step, None, None, &fake).unwrap_err();
    assert!(again.to_string().contains("never overwrites"));
}

#[test]
fn init_refuses_files_it_cannot_describe() {
    let dir = tempfile::tempdir().unwrap();
    let step = dir.path().join("a.step");
    std::fs::write(&step, STEP_BYTES).unwrap();
    // undeclared unit and no --length-unit
    let mut none = serde_json::from_str::<StepInspection>(INSPECT).unwrap();
    none.unit.declared = None;
    let error = design_init(&step, None, None, &Fake::with(none.clone())).unwrap_err();
    assert!(error.to_string().contains("--length-unit"));
    // with the option it works and says the unit was not declared
    let summary = design_init(
        &step,
        Some(&dir.path().join("b.json")),
        Some(LengthUnit::Cm),
        &Fake::with(none),
    )
    .unwrap();
    assert!(!summary.unit_declared_by_file);
    assert!(summary.line().contains("NOT declared"));
    // option disagrees with the file
    let error = design_init(
        &step,
        Some(&dir.path().join("c.json")),
        Some(LengthUnit::M),
        &Fake::new(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("declares mm"));
    // several units
    let mut multiple = serde_json::from_str::<StepInspection>(INSPECT).unwrap();
    multiple.unit.problems = vec!["the file declares several different length units".into()];
    let error = design_init(
        &step,
        Some(&dir.path().join("d.json")),
        None,
        &Fake::with(multiple),
    )
    .unwrap_err();
    assert!(error.to_string().contains("several different length units"));
    assert!(!dir.path().join("d.json").exists());
    // inch
    let mut inch = serde_json::from_str::<StepInspection>(INSPECT).unwrap();
    inch.unit.declared = Some("inch".into());
    let error = design_init(
        &step,
        Some(&dir.path().join("e.json")),
        None,
        &Fake::with(inch),
    )
    .unwrap_err();
    assert!(error.to_string().contains("does not accept"));
}

#[test]
// Verifies: GEO-027
fn a_draft_with_nulls_stops_at_check_1_and_a_filled_draft_passes() {
    let dir = tempfile::tempdir().unwrap();
    let step = dir.path().join("fixture.step");
    std::fs::write(&step, STEP_BYTES).unwrap();
    let fake = Fake::new();
    let summary = design_init(&step, None, None, &fake).unwrap();
    let calls_after_init = fake.inspect_calls.get();

    // 1. the draft as written: check 1 lists every null and nothing else runs
    let report = check_design(&summary.path, &fake).unwrap();
    assert_eq!(report.checks[0].status, Fail);
    assert_eq!(report.checks[0].findings.len(), summary.nulls_to_fill);
    assert_eq!(
        fake.inspect_calls.get(),
        calls_after_init,
        "check 1 stops before the helper"
    );

    // 2. fill it in the way a user would
    let mut value: Value = serde_json::from_slice(&std::fs::read(&summary.path).unwrap()).unwrap();
    let fixture: Value = serde_json::from_str(PASS).unwrap();
    value["materials"] = fixture["materials"].clone();
    value["references"] = fixture["references"].clone();
    value["replacement_groups"] = fixture["replacement_groups"].clone();
    let ids: Vec<String> = value["solids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect();
    let (materials, roles) = (
        ["void", "tungsten", "flibe", "magnet-pack"],
        ["plasma_chamber", "first_wall", "blanket", "tf_magnet"],
    );
    for (i, solid) in value["solids"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        solid["material_id"] = json!(materials[i]);
        solid["role"] = json!(roles[i]);
    }
    value["solids"][3]["replacement_group_id"] = json!("tf-coils");
    let mut plasma = fixture["plasma"].clone();
    plasma["chamber_solid_id"] = json!(ids[0]);
    value["plasma"] = plasma;
    std::fs::write(dir.path().join("fixture.operating-history.json"), HISTORY).unwrap();
    value["operating_scenario"] = json!({
        "path": "fixture.operating-history.json",
        "sha256": sha256_hex(HISTORY.as_bytes()),
    });
    std::fs::write(&summary.path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

    // 3. the fingerprints init wrote are the ones the helper reports, so check passes
    let report = check_design(&summary.path, &fake).unwrap();
    assert_eq!(report.checks[0].findings, []);
    assert_eq!(
        statuses(&report)[..5],
        [(1, Pass), (2, Pass), (3, Pass), (4, Pass), (5, Pass)],
        "{}",
        render_text(&report)
    );
    assert!(report.evaluated_checks_pass);
}

// ---- the external jobs ------------------------------------------------------

#[cfg(unix)]
fn fake_python(dir: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("fake-python");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

const FIND_OUT: &str =
    "out=; prev=; for a in \"$@\"; do [ \"$prev\" = --out ] && out=$a; prev=$a; done;";

#[cfg(unix)]
#[test]
fn the_cad_helper_runs_as_a_bounded_job_and_its_report_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let step = dir.path().join("fixture.step");
    std::fs::write(&step, STEP_BYTES).unwrap();
    let report_path = dir.path().join("canned.json");
    std::fs::write(&report_path, INSPECT).unwrap();
    let python = fake_python(
        dir.path(),
        &format!(
            "{FIND_OUT} case \"$*\" in *--assume-unit\\ mm*) ;; *) echo missing assume-unit >&2; exit 4;; esac; cp {} \"$out\"",
            report_path.display()
        ),
    );
    let tools = ToolPaths {
        cad_python: Some(python),
        ..ToolPaths::default()
    };
    let cancel = Cancellation::default();
    let backends = ExternalBackends {
        tools: &tools,
        cancellation: &cancel,
    };
    let ran = backends.inspect_step(&step, Some(LengthUnit::Mm)).unwrap();
    assert_eq!(ran.inspection.solids.len(), 4);
    assert_eq!(ran.helper_sha256, sha256_hex(CAD_HELPER.as_bytes()));
    assert!(ran.elapsed_seconds >= 0.0);
}

#[cfg(unix)]
#[test]
fn a_failing_helper_is_a_tool_error_with_its_message() {
    let dir = tempfile::tempdir().unwrap();
    let step = dir.path().join("fixture.step");
    std::fs::write(&step, STEP_BYTES).unwrap();
    let python = fake_python(
        dir.path(),
        "echo 'ImportError: no module named cadquery' >&2; exit 1",
    );
    let tools = ToolPaths {
        cad_python: Some(python),
        ..ToolPaths::default()
    };
    let cancel = Cancellation::default();
    let backends = ExternalBackends {
        tools: &tools,
        cancellation: &cancel,
    };
    let error = backends.inspect_step(&step, None).err().unwrap();
    assert!(matches!(error, DesignError::Tool(_)));
    assert!(
        error.to_string().contains("no module named cadquery"),
        "{error}"
    );
    // a helper that writes garbage
    let python = fake_python(dir.path(), &format!("{FIND_OUT} echo '{{}}' > \"$out\""));
    let tools = ToolPaths {
        cad_python: Some(python),
        ..ToolPaths::default()
    };
    let backends = ExternalBackends {
        tools: &tools,
        cancellation: &cancel,
    };
    assert!(
        backends
            .inspect_step(&step, None)
            .err()
            .unwrap()
            .to_string()
            .contains("could not be read")
    );
}

#[test]
fn a_missing_cad_interpreter_says_how_to_configure_one() {
    let dir = tempfile::tempdir().unwrap();
    let step = dir.path().join("fixture.step");
    std::fs::write(&step, STEP_BYTES).unwrap();
    let tools = ToolPaths::default();
    let cancel = Cancellation::default();
    let backends = ExternalBackends {
        tools: &tools,
        cancellation: &cancel,
    };
    let error = backends
        .inspect_step(&step, None)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("--cad-python") && error.contains("FARIS_CAD_PYTHON"));
    let tools = ToolPaths {
        cad_python: Some(dir.path().join("nope")),
        ..ToolPaths::default()
    };
    let backends = ExternalBackends {
        tools: &tools,
        cancellation: &cancel,
    };
    assert!(
        backends
            .inspect_step(&step, None)
            .err()
            .unwrap()
            .to_string()
            .contains("cannot be used")
    );
}

#[test]
fn without_openmc_the_audit_is_not_evaluated_with_the_reason() {
    let cancel = Cancellation::default();
    for tools in [
        ToolPaths::default(),
        ToolPaths {
            openmc_python: Some("/usr/bin/python3".into()),
            ..ToolPaths::default()
        },
        ToolPaths {
            cross_sections: Some("/x/cross_sections.xml".into()),
            ..ToolPaths::default()
        },
    ] {
        let backends = ExternalBackends {
            tools: &tools,
            cancellation: &cancel,
        };
        match backends.audit_materials(&[]).unwrap() {
            AuditOutcome::NotEvaluated { why, next_step } => {
                assert!(
                    why.contains("no OpenMC interpreter") || why.contains("no cross_sections.xml")
                );
                assert!(next_step.contains("--openmc-python"));
            }
            AuditOutcome::Evaluated { .. } => panic!("must not evaluate"),
        }
    }
}

#[cfg(unix)]
#[test]
fn an_audit_job_that_fails_is_not_evaluated_not_pass() {
    let dir = tempfile::tempdir().unwrap();
    let xs = dir.path().join("cross_sections.xml");
    std::fs::write(&xs, "<cross_sections/>").unwrap();
    let python = fake_python(
        dir.path(),
        "echo 'material_audit: needs the OpenMC environment' >&2; exit 3",
    );
    let tools = ToolPaths {
        openmc_python: Some(python),
        cross_sections: Some(xs),
        ..ToolPaths::default()
    };
    let cancel = Cancellation::default();
    let backends = ExternalBackends {
        tools: &tools,
        cancellation: &cancel,
    };
    match backends.audit_materials(&[]).unwrap() {
        AuditOutcome::NotEvaluated { why, .. } => {
            assert!(why.contains("needs the OpenMC environment"), "{why}")
        }
        AuditOutcome::Evaluated { .. } => panic!("must not evaluate"),
    }
}

#[test]
fn tool_paths_prefer_flags_over_the_environment() {
    let tools = ToolPaths::from_flags_and_environment(
        Some("/a".into()),
        Some("/b".into()),
        Some("/c".into()),
    );
    assert_eq!(tools.cad_python.as_deref(), Some(Path::new("/a")));
    assert_eq!(tools.openmc_python.as_deref(), Some(Path::new("/b")));
    assert_eq!(tools.cross_sections.as_deref(), Some(Path::new("/c")));
}

#[test]
fn the_embedded_helpers_are_the_files_in_the_repository() {
    assert!(CAD_HELPER.contains("faris-step-inspect/v1"));
    assert!(MATERIAL_AUDIT.contains("faris-material-audit/v1"));
    assert!(AUDIT_LIBRARY.contains("def build_audit"));
}
