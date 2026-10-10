use super::*;
use serde_json::{Value, json};

const PASS: &str = include_str!("../../tests/fixtures/design/pass.faris-design.json");
const INSPECT: &str = include_str!("../../tests/fixtures/design/pass.inspect.json");
const HISTORY: &str = include_str!("../../tests/fixtures/design/pass.operating-history.json");

fn base() -> Value {
    serde_json::from_str(PASS).unwrap()
}

fn design() -> Design {
    parse_design(PASS.as_bytes()).design.unwrap()
}

fn parse(value: &Value) -> ParseOutcome {
    parse_design(&serde_json::to_vec(value).unwrap())
}

fn edit(f: impl FnOnce(&mut Value)) -> ParseOutcome {
    let mut value = base();
    f(&mut value);
    parse(&value)
}

fn inspection() -> StepInspection {
    StepInspection::from_bytes(INSPECT.as_bytes()).unwrap()
}

fn items(findings: &[Finding]) -> Vec<&str> {
    findings.iter().map(|f| f.item.as_str()).collect()
}

fn one(findings: &[Finding]) -> &Finding {
    assert_eq!(findings.len(), 1, "{findings:#?}");
    &findings[0]
}

/// Every finding explains itself: item, why and next step are all stated.
fn explains(findings: &[Finding]) {
    for f in findings {
        assert!(
            !f.item.is_empty() && f.why.len() > 10 && f.next_step.len() > 10,
            "{f:?}"
        );
    }
}

fn history_facts() -> ScenarioFacts {
    scenario_facts(HISTORY.as_bytes())
}

// ---- check 1 --------------------------------------------------------------

#[test]
fn the_pass_fixture_parses_without_findings() {
    let outcome = parse_design(PASS.as_bytes());
    assert!(outcome.findings.is_empty(), "{:#?}", outcome.findings);
    assert!(outcome.design.is_some());
}

#[test]
fn not_json_is_a_finding() {
    let outcome = parse_design(b"{ nope");
    assert!(outcome.design.is_none());
    explains(&outcome.findings);
    assert!(one(&outcome.findings).why.contains("not valid JSON"));
}

#[test]
fn an_unknown_key_stops_the_import_at_any_depth() {
    for path in [
        vec!["surprise"],
        vec!["cad", "extra"],
        vec!["solids", "0", "colour"],
    ] {
        let outcome = edit(|v| {
            let mut node = v;
            for key in &path[..path.len() - 1] {
                node = match key.parse::<usize>() {
                    Ok(i) => &mut node[i],
                    Err(_) => &mut node[*key],
                };
            }
            node[path[path.len() - 1]] = json!(1);
        });
        assert!(outcome.design.is_none(), "{path:?}");
        explains(&outcome.findings);
        assert!(
            one(&outcome.findings).why.contains("unknown field"),
            "{path:?}"
        );
    }
}

#[test]
fn a_misspelt_key_is_not_ignored() {
    let outcome = edit(|v| {
        let cad = v["cad"].as_object_mut().unwrap();
        let unit = cad.remove("length_unit").unwrap();
        cad.insert("lenght_unit".into(), unit);
    });
    assert!(outcome.design.is_none());
    assert!(one(&outcome.findings).why.contains("lenght_unit"));
}

#[test]
fn unfilled_nulls_are_all_listed_by_path() {
    let outcome = edit(|v| {
        v["solids"][1]["material_id"] = Value::Null;
        v["solids"][1]["role"] = Value::Null;
        v["solids"][3]["material_id"] = Value::Null;
        v["plasma"]["major_radius_m"] = Value::Null;
        v["plasma"]["ion_density_m3"]["centre"] = Value::Null;
        v["plasma"]["provenance"] = Value::Null;
        v["operating_scenario"]["sha256"] = Value::Null;
    });
    assert!(
        outcome.design.is_some(),
        "a draft still has the right shape"
    );
    explains(&outcome.findings);
    assert_eq!(
        items(&outcome.findings),
        [
            "operating_scenario.sha256",
            "plasma.ion_density_m3.centre",
            "plasma.major_radius_m",
            "plasma.provenance",
            "solids[1].material_id",
            "solids[1].role",
            "solids[3].material_id",
        ]
    );
}

#[test]
fn nulls_the_format_allows_are_not_findings() {
    // faceting_tolerance_m, step_name, replacement_group_id, temperature_k and
    // voxel entries are already null in the fixture.
    let value = base();
    assert!(value["cad"]["faceting_tolerance_m"].is_null());
    assert!(value["solids"][0]["step_name"].is_null());
    assert!(value["solids"][0]["replacement_group_id"].is_null());
    assert!(value["materials"][2]["temperature_k"].is_null());
    assert!(value["replacement_groups"][0]["limits"][0]["averaging"]["voxel_m"][2].is_null());
    let outcome = edit(|v| {
        v["plasma"]["field_provenance"]["ion_density_m3"]["reference_id"] = Value::Null;
        v["materials"][2]["provenance"] =
            json!({"label": "authored", "reference_id": null, "note": "x"});
    });
    assert!(outcome.findings.is_empty(), "{:#?}", outcome.findings);
}

#[test]
fn a_null_in_a_required_number_is_listed_once() {
    let outcome = edit(|v| v["materials"][2]["density_kg_m3"] = Value::Null);
    assert_eq!(items(&outcome.findings), ["materials[2].density_kg_m3"]);
}

#[test]
fn a_nullable_key_must_still_be_present() {
    let outcome = edit(|v| {
        v["solids"][0].as_object_mut().unwrap().remove("step_name");
    });
    assert!(outcome.design.is_none());
    assert!(
        one(&outcome.findings)
            .why
            .contains("missing field `step_name`")
    );
}

#[test]
fn the_wrong_schema_version_or_unit_name_is_refused() {
    let outcome = edit(|v| v["schema_version"] = json!("faris-design/v9"));
    assert_eq!(items(&outcome.findings), ["schema_version"]);
    let outcome = edit(|v| v["cad"]["length_unit"] = json!("inch"));
    assert!(outcome.design.is_none());
    assert!(one(&outcome.findings).why.contains("inch"));
    let outcome = edit(|v| v["cad"]["implicit_complement_material_id"] = json!("water"));
    assert_eq!(
        items(&outcome.findings),
        ["cad.implicit_complement_material_id"]
    );
}

#[test]
fn published_provenance_needs_a_reference_id() {
    let outcome = edit(|v| v["materials"][2]["provenance"]["reference_id"] = Value::Null);
    explains(&outcome.findings);
    assert_eq!(
        items(&outcome.findings),
        ["materials[2].provenance.reference_id"]
    );
    let outcome = edit(|v| v["plasma"]["provenance"]["reference_id"] = json!(" "));
    assert_eq!(items(&outcome.findings), ["plasma.provenance.reference_id"]);
}

#[test]
fn limit_shape_rules() {
    let outcome = edit(|v| {
        v["replacement_groups"][0]["limits"][0]
            .as_object_mut()
            .unwrap()
            .remove("energy_threshold_ev");
    });
    explains(&outcome.findings);
    assert!(items(&outcome.findings)[0].ends_with("energy_threshold_ev"));
    let outcome =
        edit(|v| v["replacement_groups"][0]["limits"][0]["metric"] = json!("neutron_fluence"));
    assert!(
        one(&outcome.findings)
            .why
            .contains("only to fast_neutron_fluence")
    );
    let outcome = edit(|v| {
        v["replacement_groups"][0]["limits"][0]["averaging"]["voxel_m"] = json!([0.1, 0.1]);
    });
    assert!(one(&outcome.findings).why.contains("[dr, dz, dphi]"));
    let outcome = edit(|v| v["replacement_groups"][0]["limits"] = json!([]));
    assert_eq!(items(&outcome.findings), ["replacement_groups[0].limits"]);
}

#[test]
fn material_shape_rules() {
    let outcome = edit(|v| v["materials"][0]["density_kg_m3"] = json!(1000.0));
    assert!(one(&outcome.findings).why.contains("catalog supplies"));
    let outcome = edit(|v| {
        v["materials"][2].as_object_mut().unwrap().remove("label");
    });
    assert_eq!(items(&outcome.findings), ["materials[2].label"]);
    let outcome = edit(|v| v["materials"][2]["components"][0]["nuclide"] = json!("Cu63"));
    assert!(
        one(&outcome.findings)
            .why
            .contains("exactly one of element and nuclide")
    );
    let outcome = edit(|v| {
        v["materials"][2]["components"][1] =
            json!({"nuclide": "Fe56", "fraction": 0.3, "isotopes": {"Fe56": 1.0}});
    });
    assert_eq!(
        items(&outcome.findings),
        ["materials[2].components[1].isotopes"]
    );
}

// ---- check 2 --------------------------------------------------------------

const ZERO: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn linked_design() -> (Design, ScenarioFacts) {
    let facts = history_facts();
    let mut d = design();
    d.operating_scenario.sha256 = Some(facts.sha256.clone());
    (d, facts)
}

#[test]
fn check_2_passes_on_the_fixture() {
    let (d, facts) = linked_design();
    assert!(check_2_static(&d, ZERO, Ok(&facts)).is_empty());
    let (findings, notices) = check_2_unit(&d, ZERO, &inspection());
    assert!(findings.is_empty() && notices.is_empty());
}

#[test]
fn check_2_step_hash_mismatch() {
    let (d, facts) = linked_design();
    let findings = check_2_static(&d, &"f".repeat(64), Ok(&facts));
    explains(&findings);
    assert_eq!(items(&findings), ["cad.step_sha256"]);
}

#[test]
fn check_2_sector_and_axis_stop_with_the_specs_reason() {
    let (mut d, facts) = linked_design();
    d.cad.extent.kind = "sector".into();
    d.cad.machine_axis = "x".into();
    let findings = check_2_static(&d, ZERO, Ok(&facts));
    explains(&findings);
    assert_eq!(items(&findings), ["cad.extent", "cad.machine_axis"]);
    assert!(findings[0].why.contains("no periodic boundary"));
    assert!(findings[0].next_step.contains("patterning the sector"));
    assert!(findings[1].next_step.contains("+z through the origin"));
    d.cad.extent.kind = "wedge".into();
    assert!(
        check_2_static(&d, ZERO, Ok(&facts))[0]
            .why
            .contains("not known")
    );
}

#[test]
fn check_2_operating_scenario_rules() {
    let (d, facts) = linked_design();
    // hash mismatch
    let mut wrong = facts.clone();
    wrong.sha256 = "a".repeat(64);
    let findings = check_2_static(&d, ZERO, Ok(&wrong));
    explains(&findings);
    assert_eq!(items(&findings), ["operating_scenario.sha256"]);
    // service limits
    let mut limited = facts.clone();
    limited.service_limits = 2;
    let findings = check_2_static(&d, ZERO, Ok(&limited));
    explains(&findings);
    assert!(one(&findings).why.contains("2 service_limits"));
    // unreadable file
    let findings = check_2_static(&d, ZERO, Err("No such file"));
    explains(&findings);
    assert!(one(&findings).why.contains("could not be read"));
    // not a history document
    let bad = scenario_facts(b"{\"schema_version\": \"x\"}");
    assert!(bad.problem.is_some());
    let findings = check_2_static(
        &d,
        ZERO,
        Ok(&ScenarioFacts {
            sha256: facts.sha256.clone(),
            ..bad
        }),
    );
    assert!(
        one(&findings)
            .why
            .contains("not a usable operating-history file")
    );
}

#[test]
fn a_real_history_file_with_service_limits_is_counted() {
    let facts = scenario_facts(include_bytes!(
        "../../../../scenarios/arc-inspired/demo-operating-assumptions.json"
    ));
    assert!(facts.problem.is_none(), "{:?}", facts.problem);
    assert!(facts.service_limits > 0);
    assert_eq!(history_facts().service_limits, 0);
}

#[test]
fn check_2_unit_mismatch_and_unaccepted_units() {
    let mut d = design();
    d.cad.length_unit = LengthUnit::M;
    let (findings, _) = check_2_unit(&d, ZERO, &inspection());
    explains(&findings);
    assert_eq!(items(&findings), ["cad.length_unit"]);
    assert!(findings[0].why.contains("declares mm"));
    assert!(findings[0].next_step.contains("\"mm\""));
    let mut inch = inspection();
    inch.unit.declared = Some("inch".into());
    let (findings, _) = check_2_unit(&design(), ZERO, &inch);
    assert!(one(&findings).why.contains("does not accept"));
}

#[test]
fn check_2_several_units_or_disagreeing_readers_stop() {
    let mut multiple = inspection();
    multiple.unit.declared = None;
    multiple.unit.problems = vec![
        "the file declares several different length units: text ['mm', 'm'], OpenCASCADE ['mm', 'm']".into(),
        "the two unit readers disagree: text parse ['mm'], OpenCASCADE ['inch']".into(),
    ];
    let (findings, _) = check_2_unit(&design(), ZERO, &multiple);
    explains(&findings);
    assert_eq!(findings.len(), 2);
    assert!(findings[0].why.contains("several different length units"));
    assert!(findings[1].why.contains("disagree"));
}

#[test]
fn check_2_no_declared_unit_uses_the_design_unit_and_says_so() {
    let mut none = inspection();
    none.unit.declared = None;
    none.unit.used_is_assumed = true;
    let (findings, notices) = check_2_unit(&design(), ZERO, &none);
    assert!(findings.is_empty());
    assert!(notices[0].message.contains("declares no length unit"));
    assert!(notices[0].message.contains("read in mm"));
}

#[test]
fn check_2_unreadable_step_and_changed_file() {
    let mut bad = inspection();
    bad.problems =
        vec!["STEP read failed: RuntimeError: OpenCASCADE could not read the STEP file".into()];
    let (findings, _) = check_2_unit(&design(), ZERO, &bad);
    explains(&findings);
    assert!(one(&findings).why.contains("could not read"));
    let (findings, _) = check_2_unit(&design(), &"b".repeat(64), &inspection());
    assert!(one(&findings).why.contains("changed while"));
}

#[test]
fn inspection_schema_is_checked() {
    let mut value: Value = serde_json::from_str(INSPECT).unwrap();
    value["schema"] = json!("faris-step-inspect/v2");
    let error = StepInspection::from_bytes(&serde_json::to_vec(&value).unwrap()).unwrap_err();
    assert!(error.contains("faris-step-inspect/v1"));
    assert!(StepInspection::from_bytes(b"[]").is_err());
}

// ---- check 3 --------------------------------------------------------------

#[test]
fn check_3_passes_on_matching_fingerprints() {
    let (findings, notices) = check_3(&design(), &inspection());
    assert!(findings.is_empty() && notices.is_empty(), "{findings:#?}");
}

#[test]
fn check_3_solid_count_mismatch_names_the_missing_solid() {
    let mut i = inspection();
    i.solids.push(InspectSolid {
        step_index: 4,
        step_name: Some("divertor".into()),
        cad_volume_m3: Some(1.0),
        centroid_m: Some([0.0, 0.0, -1.0]),
        bbox_min_m: None,
        bbox_max_m: None,
        occt_valid: true,
        gprop_relative_error_estimate: 0.0,
    });
    let (findings, _) = check_3(&design(), &i);
    explains(&findings);
    assert_eq!(items(&findings), ["solids", "STEP solid 4"]);
    assert!(
        findings[0]
            .why
            .contains("5 solids and the design file lists 4")
    );
    assert!(findings[1].why.contains("divertor"));
    // the other direction: an entry with no solid
    let mut d = design();
    d.solids.push(Solid {
        id: "extra".into(),
        step_index: 9,
        ..d.solids[0].clone()
    });
    let (findings, _) = check_3(&d, &inspection());
    assert!(
        findings
            .iter()
            .any(|f| f.item.contains("step_index") && f.why.contains("does not exist"))
    );
}

#[test]
fn check_3_volume_fingerprint_tolerance_edges() {
    let mut d = design();
    d.solids[1].fingerprint.cad_volume_m3 *= 1.0 + 5e-7;
    assert!(check_3(&d, &inspection()).0.is_empty(), "inside 1e-6");
    d.solids[1].fingerprint.cad_volume_m3 = 2.5 * (1.0 + 2e-6);
    let (findings, _) = check_3(&d, &inspection());
    explains(&findings);
    assert_eq!(
        items(&findings),
        ["solids[1] (first-wall).fingerprint.cad_volume_m3"]
    );
    assert!(
        findings[0]
            .why
            .contains("edited after the design file was written")
    );
}

#[test]
fn check_3_centroid_tolerance_is_relative_to_the_model_diagonal() {
    let mut d = design();
    // 1e-6 * 20 m = 2e-5 m
    d.solids[2].fingerprint.centroid_m[1] += 1.5e-5;
    assert!(
        check_3(&d, &inspection()).0.is_empty(),
        "inside the tolerance"
    );
    d.solids[2].fingerprint.centroid_m[1] += 1.0e-4;
    let (findings, _) = check_3(&d, &inspection());
    explains(&findings);
    assert_eq!(
        items(&findings),
        ["solids[2] (blanket).fingerprint.centroid_m"]
    );
    // a wrong volume and a wrong centroid are both reported
    d.solids[2].fingerprint.cad_volume_m3 = 99.0;
    assert_eq!(check_3(&d, &inspection()).0.len(), 2);
}

#[test]
fn check_3_duplicate_and_reordered_step_indexes() {
    let mut d = design();
    d.solids[1].step_index = 2;
    let (findings, _) = check_3(&d, &inspection());
    assert!(
        findings
            .iter()
            .any(|f| f.item.ends_with(".step_index") && f.why.contains("also used"))
    );
    // swapped entries: fingerprints no longer match their indexes
    let mut d = design();
    d.solids[1].step_index = 2;
    d.solids[2].step_index = 1;
    let (findings, _) = check_3(&d, &inspection());
    assert_eq!(
        findings.len(),
        4,
        "volume and centroid of both swapped entries: {findings:#?}"
    );
}

#[test]
fn check_3_indistinguishable_solids_are_named() {
    let mut i = inspection();
    i.solids[3].cad_volume_m3 = i.solids[2].cad_volume_m3;
    i.solids[3].centroid_m = i.solids[2].centroid_m;
    let mut d = design();
    d.solids[3].fingerprint = d.solids[2].fingerprint.clone();
    let (findings, _) = check_3(&d, &i);
    explains(&findings);
    let finding = one(&findings);
    assert!(finding.item.contains("blanket") && finding.item.contains("coil-01"));
    assert!(finding.why.contains("overlap"));
}

#[test]
fn check_3_name_mismatch_is_a_notice_and_invalid_solids_are_information() {
    let mut i = inspection();
    i.solids[1].step_name = Some("renamed".into());
    i.solids[2].occt_valid = false;
    let (findings, notices) = check_3(&design(), &i);
    assert!(findings.is_empty());
    assert_eq!(notices.len(), 2);
    assert!(notices[0].message.contains("never used to bind"));
    assert!(notices[1].message.contains("BRepCheck"));
}

// ---- check 4 --------------------------------------------------------------

#[test]
fn check_4_passes_on_the_fixture() {
    assert!(check_4(&design()).is_empty());
}

#[test]
fn check_4_bad_and_reserved_ids() {
    for bad in ["Plasma", "-x", "has space", "", &"a".repeat(29), "wall_1"] {
        let mut d = design();
        d.solids[1].id = bad.to_string();
        let findings = check_4(&d);
        explains(&findings);
        assert_eq!(findings.len(), 1, "{bad:?}");
        assert!(findings[0].why.contains("^[a-z0-9]"), "{bad:?}");
    }
    assert!(valid_id(&"a".repeat(28)) && valid_id("0-9") && valid_id("a"));
    for reserved in ["vacuum", "Vacuum", "GRAVEYARD", "graveyard", "void", "Void"] {
        let mut d = design();
        d.solids[1].id = reserved.to_string();
        d.materials[1].id = reserved.to_string();
        let findings = check_4(&d);
        explains(&findings);
        assert!(
            findings
                .iter()
                .filter(|f| f.item.ends_with(".id"))
                .all(|f| f.why.contains("reserved")),
            "{reserved}"
        );
        assert!(findings.iter().any(|f| f.item.starts_with("solids")));
        assert!(findings.iter().any(|f| f.item.starts_with("materials")));
    }
}

#[test]
fn check_4_duplicate_ids() {
    let mut d = design();
    d.solids[2].id = "plasma".into();
    d.materials[1].id = "tungsten".into();
    d.replacement_groups.push(d.replacement_groups[0].clone());
    d.references.push(d.references[0].clone());
    let findings = check_4(&d);
    explains(&findings);
    let found = items(&findings);
    for item in [
        "solids[2].id",
        "materials[1].id",
        "replacement_groups[1].id",
        "references[1].id",
    ] {
        assert!(found.contains(&item), "{item} in {found:?}");
    }
}

#[test]
fn check_4_unresolved_references() {
    let mut d = design();
    d.solids[1].material_id = Some("unobtainium".into());
    d.solids[3].replacement_group_id = Some("no-such-group".into());
    d.plasma.chamber_solid_id = Some("no-such-solid".into());
    d.materials[0].catalog_id = Some("not-in-catalog".into());
    d.materials[2].provenance.as_mut().unwrap().reference_id = Some("nobody-2099".into());
    d.replacement_groups[0].limits[0].provenance.reference_id = Some("missing-ref".into());
    let findings = check_4(&d);
    explains(&findings);
    let found = items(&findings);
    for item in [
        "solids[1] (first-wall).material_id",
        "solids[3] (coil-01).replacement_group_id",
        "plasma.chamber_solid_id",
        "materials[0].catalog_id",
        "materials[2].provenance.reference_id",
        "replacement_groups[0].limits[0].provenance.reference_id",
    ] {
        assert!(found.contains(&item), "{item} in {found:?}");
    }
    // the unresolved group also leaves tf-coils with no solid
    assert!(
        found
            .iter()
            .any(|i| i.starts_with("replacement_groups[0]") && !i.contains("limits"))
    );
    assert!(
        findings.iter().any(|f| f.why.contains("tungsten-natural")),
        "lists the catalog"
    );
}

#[test]
fn check_4_a_group_needs_a_solid_and_void_resolves() {
    let mut d = design();
    d.solids[3].replacement_group_id = None;
    let findings = check_4(&d);
    assert_eq!(items(&findings), ["replacement_groups[0] (tf-coils)"]);
    let mut d = design();
    d.solids[1].material_id = Some("void".into());
    assert!(check_4(&d).is_empty(), "void is a built-in material id");
}

// ---- check 5 --------------------------------------------------------------

#[test]
fn check_5_passes_on_the_fixture_and_lists_the_void_solid() {
    let d = design();
    let (findings, notices) = check_5(&d);
    assert!(findings.is_empty(), "{findings:#?}");
    assert!(
        notices
            .iter()
            .any(|n| n.message.contains("dose-rate rule of stage S3"))
    );
    assert_eq!(
        void_solids(&d),
        [VoidSolid {
            id: "plasma".into(),
            step_index: 0,
            role: Role::PlasmaChamber
        }]
    );
}

#[test]
fn check_5_plasma_chamber_rules() {
    let mut d = design();
    d.solids[0].role = Some(Role::Structure);
    let findings = check_5(&d).0;
    explains(&findings);
    assert!(
        findings
            .iter()
            .any(|f| f.why.contains("no solid has the role plasma_chamber"))
    );
    let mut d = design();
    d.solids[1].role = Some(Role::PlasmaChamber);
    d.solids[1].material_id = Some("void".into());
    assert!(
        check_5(&d).0[0]
            .why
            .contains("2 solids have the role plasma_chamber")
    );
    let mut d = design();
    d.solids[0].material_id = Some("tungsten".into());
    assert_eq!(items(&check_5(&d).0), ["solids[0] (plasma).material_id"]);
    let mut d = design();
    d.plasma.chamber_solid_id = Some("blanket".into());
    let findings = check_5(&d).0;
    assert_eq!(items(&findings), ["plasma.chamber_solid_id"]);
    explains(&findings);
}

#[test]
fn check_5_void_is_allowed_only_in_three_roles() {
    for role in [Role::PlasmaChamber, Role::Vacuum, Role::PortPlug] {
        assert!(role.allows_void());
    }
    for role in [
        Role::FirstWall,
        Role::Divertor,
        Role::Blanket,
        Role::Multiplier,
        Role::Shield,
        Role::VacuumVessel,
        Role::TfMagnet,
        Role::PfMagnet,
        Role::CsMagnet,
        Role::Structure,
    ] {
        assert!(!role.allows_void(), "{role:?}");
        let mut d = design();
        d.solids[2].role = Some(role);
        d.solids[2].material_id = Some("void".into());
        let findings = check_5(&d).0;
        explains(&findings);
        assert!(
            one(&findings).why.contains("cannot pass unseen"),
            "{role:?}"
        );
    }
    let mut d = design();
    d.solids[2].role = Some(Role::PortPlug);
    d.solids[2].material_id = Some("void".into());
    assert!(check_5(&d).0.is_empty());
    assert_eq!(void_solids(&d).len(), 2, "every void solid is listed");
    d.solids[2].role = Some(Role::Vacuum);
    d.solids[2].material_id = Some("tungsten".into());
    assert!(check_5(&d).0[0].why.contains("must be \"void\""));
}

fn nuclide_name_is_accepted(name: &str) -> bool {
    let mut d = design();
    d.materials[2].components.as_mut().unwrap()[0].element = None;
    d.materials[2].components.as_mut().unwrap()[0].nuclide = Some(name.into());
    check_5(&d)
        .0
        .iter()
        .all(|f| !f.why.contains("not a nuclide name"))
}

#[test]
fn check_5_material_rules() {
    let mut d = design();
    d.materials[2].density_kg_m3 = Some(0.0);
    assert_eq!(
        items(&check_5(&d).0),
        ["materials[2] (magnet-pack).density_kg_m3"]
    );
    d.materials[2].density_kg_m3 = Some(-5.0);
    assert_eq!(check_5(&d).0.len(), 1);
    d.materials[2].density_kg_m3 = Some(8500.0);
    d.materials[2].temperature_k = Some(-1.0);
    assert!(items(&check_5(&d).0)[0].ends_with("temperature_k"));
    d.materials[2].temperature_k = None;
    // fractions sum to 1 within 1e-9
    d.materials[2].components.as_mut().unwrap()[0].fraction = 0.6 + 5e-10;
    assert!(check_5(&d).0.is_empty(), "inside 1e-9");
    d.materials[2].components.as_mut().unwrap()[0].fraction = 0.6 + 1e-8;
    let findings = check_5(&d).0;
    explains(&findings);
    assert!(one(&findings).why.contains("sum to"));
    d.materials[2].components.as_mut().unwrap()[0].fraction = 0.6;
    // isotope fractions
    let li = &mut d.materials[2].components.as_mut().unwrap()[2];
    li.isotopes = Some([("Li6".to_string(), 0.9), ("Li7".to_string(), 0.2)].into());
    assert!(items(&check_5(&d).0)[0].ends_with("isotopes"));
    let li = &mut d.materials[2].components.as_mut().unwrap()[2];
    li.isotopes = Some([("Be9".to_string(), 1.0)].into());
    assert!(check_5(&d).0[0].why.contains("not a nuclide of Li"));
    // names
    d.materials[2].components.as_mut().unwrap()[2].isotopes = None;
    d.materials[2].components.as_mut().unwrap()[0].element = Some("copper".into());
    assert!(check_5(&d).0[0].why.contains("not an element symbol"));
    for ok in ["Fe56", "Li6", "Ag110_m1", "U235"] {
        assert!(nuclide_name_is_accepted(ok), "{ok}");
    }
    for bad in ["Fe", "56", "Fe56x", "fe56", "Fe1234", "Fe56_m"] {
        assert!(!nuclide_name_is_accepted(bad), "{bad}");
    }
}

#[test]
fn check_5_limit_rules_and_not_evaluated_notices() {
    let mut d = design();
    d.replacement_groups[0].limits[0].limit = 0.0;
    d.replacement_groups[0].limits[0].averaging = Averaging::PeakMesh {
        voxel_m: vec![Some(-1.0), None, None],
    };
    let findings = check_5(&d).0;
    explains(&findings);
    assert_eq!(findings.len(), 2);
    let mut d = design();
    d.replacement_groups[0].limits[0].metric = Metric::DisplacementsPerAtom;
    d.replacement_groups[0].limits[0].energy_threshold_ev = None;
    let (findings, notices) = check_5(&d);
    assert!(findings.is_empty());
    assert!(notices.iter().any(
        |n| n.message.contains("NOT_EVALUATED") && n.message.contains("displacements_per_atom")
    ));
}

#[test]
fn a_catalog_material_needs_no_recipe_checks() {
    let d = design();
    assert!(d.materials[0].catalog_id.is_some());
    assert!(catalog().len() >= 5);
    let tungsten = catalog_material("tungsten-natural").unwrap();
    let sum: f64 = tungsten.nuclide_atom_fractions.values().sum();
    assert!((sum - 1.0).abs() < 1e-3, "{sum}");
    assert!(catalog_material("vacuum-gap").is_none());
}

// ---- suggested ids --------------------------------------------------------

#[test]
fn suggested_ids_are_safe_unique_and_never_reserved() {
    let none: Vec<String> = vec![];
    assert_eq!(suggest_id(Some("First Wall"), 3, &none), "first-wall");
    assert_eq!(suggest_id(Some("  Tf_Coil__07!! "), 3, &none), "tf-coil-07");
    assert_eq!(suggest_id(None, 7, &none), "solid-007");
    assert_eq!(suggest_id(Some("!!!"), 12, &none), "solid-012");
    assert_eq!(suggest_id(Some(""), 1234, &none), "solid-1234");
    let long = "a".repeat(40);
    assert_eq!(suggest_id(Some(&long), 0, &none).len(), 28);
    assert!(valid_id(&suggest_id(Some("Ünïcode wall"), 0, &none)));
    for reserved in ["Vacuum", "GRAVEYARD", "void"] {
        assert_eq!(suggest_id(Some(reserved), 5, &none), "solid-005");
    }
    // uniqueness within 28 characters
    let taken = vec!["wall".to_string(), "wall-2".to_string()];
    assert_eq!(suggest_id(Some("wall"), 0, &taken), "wall-3");
    let long = "b".repeat(28);
    let taken = vec![long.clone()];
    let id = suggest_id(Some(&long), 0, &taken);
    assert_eq!(id.len(), 28);
    assert!(id.ends_with("-2") && id != long);
    assert!(valid_id(&id));
}

#[test]
fn a_draft_round_trips_through_the_types() {
    let mut value = base();
    value["solids"][1]["material_id"] = Value::Null;
    let outcome = parse(&value);
    let design = outcome.design.unwrap();
    assert!(design.solids[1].material_id.is_none());
    let again: Value = serde_json::to_value(&design).unwrap();
    assert_eq!(again["solids"][1]["material_id"], Value::Null);
    assert_eq!(parse(&again).findings.len(), 1);
}
