use super::*;
use serde_json::json;
use std::collections::BTreeSet;

const SCENARIO: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scenarios/arc-inspired/cold-reference.scenario.json"
);
const PHYSICS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scenarios/arc-inspired/cold-reference.reference.physics.json"
);

fn scenario() -> Value {
    serde_json::from_slice(&std::fs::read(SCENARIO).unwrap()).unwrap()
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
    fn below(&mut self, n: usize) -> usize {
        self.next() as usize % n
    }
}

/// The same value written as JSON text in another key order, with other white space and other
/// spellings of the same numbers.
fn scramble(value: &Value, rng: &mut Lcg, out: &mut String) {
    let gap = |rng: &mut Lcg, out: &mut String| {
        for _ in 0..rng.below(3) {
            out.push([' ', '\n', '\t'][rng.below(3)]);
        }
    };
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            for i in (1..keys.len()).rev() {
                keys.swap(i, rng.below(i + 1));
            }
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                gap(rng, out);
                out.push_str(&serde_json::to_string(key).unwrap());
                gap(rng, out);
                out.push(':');
                gap(rng, out);
                scramble(&map[*key], rng, out);
            }
            gap(rng, out);
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                gap(rng, out);
                scramble(item, rng, out);
            }
            out.push(']');
        }
        Value::Number(n) => {
            let f = n.as_f64().unwrap();
            let text = match rng.below(4) {
                0 if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}.0", f as i64),
                1 if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}e0", f as i64),
                2 => format!("{f:?}"),
                3 if f.fract() != 0.0 && !format!("{f:?}").contains('e') => format!("{f:?}0"),
                _ => n.to_string(),
            };
            out.push_str(&text);
        }
        other => out.push_str(&other.to_string()),
    }
}

// Verifies: COL-025, CFG-081
#[test]
fn a_thousand_reorderings_and_reformattings_are_not_changes() {
    let original = scenario();
    let mut rng = Lcg(7);
    for round in 0..1000 {
        let mut text = String::new();
        scramble(&original, &mut rng, &mut text);
        let again: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
        let report = diff_models("a", &original, "b", &again);
        assert!(report.identical, "round {round}: {:?}", report.inputs);
        assert_eq!(report.summary.inputs_changed, 0);
    }
}

#[test]
fn crlf_line_endings_and_decimal_comma_text_are_not_changes() {
    let a = json!({"schema_version": "faris-scenario/v0.1", "title": "a\nb", "x": "1,5"});
    let b = json!({"schema_version": "faris-scenario/v0.1", "title": "a\r\nb", "x": "1.50"});
    assert!(diff_models("a", &a, "b", &b).identical);
}

/// An edit applied to the scenario and the path the diff should report for it.
fn random_edit(doc: &mut Value, rng: &mut Lcg) -> String {
    let variants = doc["variants"].as_array().unwrap().len();
    let v = rng.below(variants);
    let vid = doc["variants"][v]["id"].as_str().unwrap().to_string();
    let layers = doc["variants"][v]["layers"].as_array().unwrap().len();
    let l = rng.below(layers);
    let lid = doc["variants"][v]["layers"][l]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let base = format!("variants[id={vid}].layers[id={lid}]");
    match rng.below(9) {
        0 => {
            let layer = &mut doc["variants"][v]["layers"][l];
            let t = layer["thickness_m"].as_f64().unwrap();
            layer["thickness_m"] = json!(t * 1.25 + 0.001);
            format!("{base}.thickness_m")
        }
        1 => {
            doc["variants"][v]["layers"][l]["material_id"] = json!("other-material");
            format!("{base}.material_id")
        }
        2 => {
            doc["variants"][v]["layers"][l]["label"] = json!("Renamed");
            format!("{base}.label")
        }
        3 => {
            doc["variants"][v]["layers"][l]["color"] = json!("#123456");
            format!("{base}.color")
        }
        4 => {
            let r = doc["geometry"]["major_radius_m"].as_f64().unwrap();
            doc["geometry"]["major_radius_m"] = json!(r + 0.1);
            "geometry.major_radius_m".into()
        }
        5 => {
            doc["operating_plan"]["horizon_years"] = json!(41.5);
            "operating_plan.horizon_years".into()
        }
        6 => {
            doc["operating_plan"]["fusion_power_mw"] = json!(612.5);
            "operating_plan.fusion_power_mw".into()
        }
        7 => {
            doc["title"] = json!("A different title");
            "title".into()
        }
        _ => {
            doc["geometry"]["plasma_to_first_wall_gap_m"] = json!(0.0875);
            "geometry.plasma_to_first_wall_gap_m".into()
        }
    }
}

// Verifies: COL-020, GEO-067
#[test]
fn exactly_the_seeded_edits_are_listed_over_250_trials() {
    let base = scenario();
    let mut rng = Lcg(2026);
    let mut edits = 0;
    for trial in 0..250 {
        let mut edited = base.clone();
        let mut expected = BTreeSet::new();
        for _ in 0..1 + rng.below(4) {
            expected.insert(random_edit(&mut edited, &mut rng));
        }
        edits += expected.len();
        let report = diff_models("base", &base, "edited", &edited);
        let listed: BTreeSet<String> = report
            .inputs
            .iter()
            .chain(&report.presentation)
            .map(|c| c.path.clone())
            .collect();
        assert_eq!(listed, expected, "trial {trial}");
        for change in report.inputs.iter().chain(&report.presentation) {
            assert_eq!(change.kind, ChangeKind::Changed);
            assert_ne!(change.old, change.new);
        }
    }
    assert!(edits >= 250);
}

const DESIGN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/corpus/seeded-missing-material-boxes.design.json"
);

fn design() -> Value {
    serde_json::from_slice(&std::fs::read(DESIGN).unwrap()).unwrap()
}

/// One random edit of a design file: a solid's material or role, its placement (a centroid
/// coordinate) or volume, or a plasma value. Every edit moves its value away from the original and
/// never back, so the edited paths are exactly the ones the diff must list.
fn random_design_edit(doc: &mut Value, rng: &mut Lcg) -> String {
    let solids = doc["solids"].as_array().unwrap().len();
    let s = rng.below(solids);
    let id = doc["solids"][s]["id"].as_str().unwrap().to_string();
    let base = format!("solids[id={id}]");
    match rng.below(5) {
        0 => {
            doc["solids"][s]["material_id"] = json!("edited-material");
            format!("{base}.material_id")
        }
        1 => {
            let k = rng.below(3);
            let v = doc["solids"][s]["fingerprint"]["centroid_m"][k]
                .as_f64()
                .unwrap();
            doc["solids"][s]["fingerprint"]["centroid_m"][k] = json!(v + 0.05);
            format!("{base}.fingerprint.centroid_m[{k}]")
        }
        2 => {
            let v = doc["solids"][s]["fingerprint"]["cad_volume_m3"]
                .as_f64()
                .unwrap();
            doc["solids"][s]["fingerprint"]["cad_volume_m3"] = json!(v * 1.5 + 0.001);
            format!("{base}.fingerprint.cad_volume_m3")
        }
        3 => {
            doc["solids"][s]["role"] = json!("edited-role");
            format!("{base}.role")
        }
        _ => {
            let v = doc["plasma"]["major_radius_m"].as_f64().unwrap();
            doc["plasma"]["major_radius_m"] = json!(v + 0.1);
            "plasma.major_radius_m".into()
        }
    }
}

// Verifies: GEO-067
#[test]
fn exactly_the_seeded_edits_to_a_design_file_and_its_placements_are_listed_over_100_trials() {
    let base = design();
    assert_eq!(DocKind::detect(&base), DocKind::Design);
    let mut rng = Lcg(4242);
    let (mut edits, mut placements) = (0, 0);
    for trial in 0..100 {
        let mut edited = base.clone();
        let mut expected = BTreeSet::new();
        for _ in 0..1 + rng.below(4) {
            expected.insert(random_design_edit(&mut edited, &mut rng));
        }
        let report = diff_models("base", &base, "edited", &edited);
        let listed: BTreeSet<String> = report
            .inputs
            .iter()
            .chain(&report.presentation)
            .map(|c| c.path.clone())
            .collect();
        assert_eq!(listed, expected, "trial {trial}");
        edits += expected.len();
        placements += expected.iter().filter(|p| p.contains("centroid_m")).count();
        for change in report.inputs.iter().chain(&report.presentation) {
            assert_eq!(change.kind, ChangeKind::Changed);
            assert_ne!(change.old, change.new);
        }
    }
    assert!(
        edits >= 100 && placements > 0,
        "{edits} edits, {placements} placements"
    );
}

fn unit_of_path(report: &DiffReport, path: &str) -> Option<String> {
    report
        .inputs
        .iter()
        .find(|c| c.path == path)
        .and_then(|c| c.unit.clone())
}

// Verifies: COL-020
#[test]
fn a_change_carries_its_old_value_new_value_and_unit() {
    let a = scenario();
    let mut b = a.clone();
    b["variants"][0]["layers"][1]["thickness_m"] = json!(0.5);
    let report = diff_models("a", &a, "b", &b);
    let change = &report.inputs[0];
    assert_eq!(
        change.path,
        "variants[id=reference].layers[id=blanket].thickness_m"
    );
    assert_eq!(change.old, Some(json!(0.45)));
    assert_eq!(change.new, Some(json!(0.5)));
    assert_eq!(change.unit.as_deref(), Some("m"));
    assert_eq!(unit_of_path(&report, &change.path).as_deref(), Some("m"));
    let text = render_text(&report);
    assert!(text.contains("0.45 -> 0.5 m"), "{text}");
    assert!(text.contains("transport run"), "{text}");
}

// Verifies: GEO-068
#[test]
fn fifty_known_edits_get_the_right_cost_class() {
    let base = scenario();
    let layer_ids: Vec<String> = base["variants"][0]["layers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["id"].as_str().unwrap().to_string())
        .collect();
    let mut cases: Vec<(Value, CostClass)> = Vec::new();
    for (i, id) in layer_ids.iter().enumerate() {
        let mut thickness = base.clone();
        thickness["variants"][0]["layers"][i]["thickness_m"] = json!(0.9);
        cases.push((thickness, CostClass::TransportRun));
        let mut material = base.clone();
        material["variants"][0]["layers"][i]["material_id"] = json!("swapped");
        cases.push((material, CostClass::TransportRun));
        let mut label = base.clone();
        label["variants"][0]["layers"][i]["label"] = json!(format!("{id} renamed"));
        cases.push((label, CostClass::Instant));
        let mut color = base.clone();
        color["variants"][0]["layers"][i]["color"] = json!("#000000");
        cases.push((color, CostClass::Instant));
    }
    for key in [
        "major_radius_m",
        "plasma_minor_radius_m",
        "plasma_to_first_wall_gap_m",
    ] {
        let mut geometry = base.clone();
        let v = geometry["geometry"][key].as_f64().unwrap();
        geometry["geometry"][key] = json!(v + 0.05);
        cases.push((geometry, CostClass::TransportRun));
    }
    let mut horizon = base.clone();
    horizon["operating_plan"]["horizon_years"] = json!(12.0);
    cases.push((horizon, CostClass::Seconds));
    let mut power = base.clone();
    power["operating_plan"]["fusion_power_mw"] = json!(400.0);
    cases.push((power, CostClass::TransportRun));
    let mut title = base.clone();
    title["title"] = json!("new");
    cases.push((title, CostClass::Instant));
    let mut note = base.clone();
    note["assumptions"][0] = json!("A reworded assumption.");
    cases.push((note, CostClass::Instant));
    let mut reference = base.clone();
    reference["references"][0]["title"] = json!("Another title");
    cases.push((reference, CostClass::Instant));
    // The other allocation's layers: thickness, material and label.
    for i in 0..layer_ids.len() {
        let mut thickness = base.clone();
        thickness["variants"][1]["layers"][i]["thickness_m"] = json!(0.31);
        cases.push((thickness, CostClass::TransportRun));
        let mut material = base.clone();
        material["variants"][1]["layers"][i]["material_id"] = json!("swapped");
        cases.push((material, CostClass::TransportRun));
        let mut label = base.clone();
        label["variants"][1]["layers"][i]["label"] = json!("renamed");
        cases.push((label, CostClass::Instant));
    }
    let mut wrong = 0;
    for (edited, expected) in &cases {
        let report = diff_models("a", &base, "b", edited);
        let cost = report
            .inputs
            .iter()
            .chain(&report.presentation)
            .map(|c| c.cost)
            .max();
        if cost != Some(*expected) {
            wrong += 1;
        }
    }
    assert!(cases.len() >= 50, "{} cases", cases.len());
    assert_eq!(wrong, 0);
}

// Verifies: GEO-068
#[test]
fn assumption_edits_reuse_the_transport_and_recalculate_the_history() {
    let a = serde_json::to_value(crate::fixtures::assumptions()).unwrap();
    let mut b = a.clone();
    b["recovery_fraction"] = json!(0.5);
    let report = diff_models("a", &a, "b", &b);
    let change = &report.inputs[0];
    assert_eq!(change.cost, CostClass::Seconds);
    assert_eq!(change.invalidates, vec!["history", "ensembles"]);
    b["provenance"] = json!("reworded");
    let report = diff_models("a", &a, "b", &b);
    assert_eq!(report.presentation.len(), 1);
}

// Verifies: GEO-068, COL-022
#[test]
fn the_impact_names_what_is_invalidated_and_what_stays_reusable() {
    let mut builder = DiffBuilder::new("a", "b", "test");
    let a = scenario();
    let mut b = a.clone();
    b["variants"][0]["layers"][1]["thickness_m"] = json!(0.5);
    builder.input_document(
        "scenario.json",
        DocKind::Scenario,
        "port/reference",
        Some(&a),
        Some(&b),
    );
    let assumptions =
        json!({"schema_version": "faris-operating-history/v0.1", "recovery_fraction": 1.0});
    let mut other = assumptions.clone();
    other["recovery_fraction"] = json!(0.5);
    builder.input_document(
        "assumptions",
        DocKind::Assumptions,
        "",
        Some(&assumptions),
        Some(&other),
    );
    let entry = |group: &str, name: &str, kind, old: &str, new: &str| ResultEntry {
        group: group.into(),
        name: name.into(),
        kind,
        old_sha256: Some(old.into()),
        new_sha256: Some(new.into()),
        values: vec![],
    };
    builder.result(entry(
        "port",
        "reference",
        ResultKind::Transport,
        "aa",
        "bb",
    ));
    builder.result(entry(
        "port",
        "breeder-emphasis",
        ResultKind::Transport,
        "cc",
        "cc",
    ));
    builder.result(entry(
        "control",
        "reference",
        ResultKind::Transport,
        "dd",
        "ee",
    ));
    builder.result(entry(
        "history",
        "operating-history",
        ResultKind::History,
        "ff",
        "ff",
    ));
    let report = builder.finish();
    let standing = |item: &str| {
        report
            .impact
            .iter()
            .find(|i| i.item == item)
            .unwrap_or_else(|| panic!("{item}"))
            .standing
    };
    assert_eq!(standing("port/reference"), Standing::Invalidated);
    assert_eq!(standing("port/breeder-emphasis"), Standing::Reusable);
    assert_eq!(standing("control/reference"), Standing::Reusable);
    assert_eq!(standing("history/operating-history"), Standing::Invalidated);
    // The changed result links to the change that explains it; the one nothing explains says so.
    let linked = report
        .results
        .iter()
        .find(|r| r.item == "port/reference")
        .unwrap();
    assert_eq!(linked.caused_by, vec!["i1".to_string()]);
    let unexplained = report
        .results
        .iter()
        .find(|r| r.item == "control/reference")
        .unwrap();
    assert!(unexplained.caused_by.is_empty());
    assert_eq!(
        unexplained.cause_note.as_deref(),
        Some("no input change found")
    );
    assert_eq!(report.summary.highest_cost, Some(CostClass::TransportRun));
}

// Verifies: GEO-067
#[test]
fn added_removed_moved_and_material_changed_components_and_volume_changes_are_listed() {
    let a = scenario();
    // Moved: the shield and the vessel swap places (thicknesses kept, so volumes shift).
    let mut moved = a.clone();
    let layers = moved["variants"][0]["layers"].as_array_mut().unwrap();
    layers.swap(2, 3);
    let report = diff_models("a", &a, "b", &moved);
    let of = |report: &DiffReport, id: &str| {
        report
            .components
            .iter()
            .find(|c| c.variant == "reference" && c.component == id)
            .cloned()
    };
    let shield = of(&report, "shield").unwrap();
    let vessel = of(&report, "vessel").unwrap();
    assert!(
        shield.changes.contains(&"moved") || vessel.changes.contains(&"moved"),
        "{shield:?} {vessel:?}"
    );
    // Removed and added.
    let mut removed = a.clone();
    removed["variants"][0]["layers"]
        .as_array_mut()
        .unwrap()
        .remove(4);
    let report = diff_models("a", &a, "b", &removed);
    let gap = of(&report, "magnet-gap").unwrap();
    assert_eq!(gap.changes, vec!["removed"]);
    // The magnets slid inward: same order, same thickness, smaller radii: a volume change only.
    let magnets = of(&report, "magnets").unwrap();
    assert_eq!(magnets.changes, vec!["volume"]);
    assert!(magnets.volume_delta_m3.unwrap() < 0.0);
    let mut added = a.clone();
    added["variants"][0]["layers"].as_array_mut().unwrap().insert(
        3,
        json!({"id": "liner", "label": "Liner", "material_id": "pure-iron-surrogate", "thickness_m": 0.01, "color": "#888888"}),
    );
    let report = diff_models("a", &a, "b", &added);
    assert_eq!(of(&report, "liner").unwrap().changes, vec!["added"]);
    assert!(of(&report, "magnets").unwrap().changes.contains(&"volume"));
    assert!(!of(&report, "first-wall").is_some());
    // Material change.
    let mut material = a.clone();
    material["variants"][0]["layers"][1]["material_id"] = json!("lead-lithium");
    let report = diff_models("a", &a, "b", &material);
    let blanket = of(&report, "blanket").unwrap();
    assert_eq!(blanket.changes, vec!["material"]);
    assert_eq!(blanket.material_new.as_deref(), Some("lead-lithium"));
    // Thickness: that cell and every cell outside it change volume.
    let mut thicker = a.clone();
    thicker["variants"][0]["layers"][1]["thickness_m"] = json!(0.5);
    let report = diff_models("a", &a, "b", &thicker);
    let ids: BTreeSet<_> = report
        .components
        .iter()
        .map(|c| c.component.clone())
        .collect();
    assert_eq!(
        ids,
        ["blanket", "shield", "vessel", "magnet-gap", "magnets"]
            .into_iter()
            .map(String::from)
            .collect()
    );
    assert!(
        of(&report, "blanket")
            .unwrap()
            .changes
            .contains(&"thickness")
    );
    // Nothing changed, nothing listed.
    assert!(diff_models("a", &a, "b", &a).components.is_empty());
}

// Verifies: GEO-069
#[test]
fn the_geometry_table_total_mass_is_the_sum_of_the_cells() {
    let scenario = scenario();
    let loaded =
        faris_model::LoadedScenario::from_bytes(&std::fs::read(SCENARIO).unwrap()).unwrap();
    let physics =
        crate::reactor::load_physics_case(std::path::Path::new(PHYSICS), &loaded).unwrap();
    let summary = geometry_summary(&scenario, "reference", Some(&physics)).unwrap();
    assert_eq!(summary.rows.len(), 6);
    // An independent route to the total: the closed form per layer from the layer list.
    let g = &scenario["geometry"];
    let r0 = g["major_radius_m"].as_f64().unwrap();
    let mut inner = g["plasma_minor_radius_m"].as_f64().unwrap()
        + g["plasma_to_first_wall_gap_m"].as_f64().unwrap();
    let mut expected = 0.0;
    for layer in scenario["variants"][0]["layers"].as_array().unwrap() {
        let outer = inner + layer["thickness_m"].as_f64().unwrap();
        let material = layer["material_id"].as_str().unwrap();
        let density = physics
            .materials
            .iter()
            .find(|m| m.id == material)
            .and_then(|m| match &m.recipe {
                faris_model::physics::MaterialRecipe::NuclideMixture { density_kg_m3, .. } => {
                    Some(*density_kg_m3)
                }
                _ => None,
            })
            .unwrap_or(0.0);
        expected += density
            * 2.0
            * faris_model::math::powi(std::f64::consts::PI, 2)
            * r0
            * (outer * outer - inner * inner);
        inner = outer;
    }
    let total = summary.total_mass_kg.unwrap();
    assert!(
        (total - expected).abs() / expected < 1e-9,
        "{total} vs {expected}"
    );
    let by_rows: f64 = summary.rows.iter().map(|r| r.mass_kg.unwrap()).sum();
    assert!((total - by_rows).abs() / total < 1e-9);
    let void = summary.rows.iter().find(|r| r.id == "magnet-gap").unwrap();
    assert_eq!(void.mass_kg, Some(0.0));
    assert_eq!(void.density_kg_m3, None);
    let csv = geometry_summary_csv(&summary);
    let mut lines = csv.lines();
    assert!(
        lines
            .next()
            .unwrap()
            .starts_with("cell_name,cell_id,variant,material_id,volume_m3,")
    );
    assert_eq!(csv.lines().count(), 1 + 6 + 1);
    assert!(csv.lines().last().unwrap().starts_with("TOTAL,"));
    assert!(csv.contains("analytic torus shell,calculated"));
}

// Verifies: GEO-069
#[test]
fn without_a_physics_case_the_mass_is_not_evaluated_not_invented() {
    let summary = geometry_summary(&scenario(), "reference", None).unwrap();
    assert!(summary.rows.iter().all(|r| r.mass_kg.is_none()));
    assert!(summary.total_mass_kg.is_none());
    assert!(summary.notes[0].contains("NOT_EVALUATED"));
    let csv = geometry_summary_csv(&summary);
    assert!(csv.contains("mass not evaluated"));
    assert!(
        geometry_summary(&scenario(), "no-such-variant", None)
            .unwrap_err()
            .contains("Next step")
    );
}

#[test]
fn a_document_on_one_side_only_is_an_addition_or_a_removal() {
    let mut builder = DiffBuilder::new("a", "b", "test");
    builder.input_document(
        "extra.json",
        DocKind::Other,
        "",
        None,
        Some(&json!({"x": 1})),
    );
    builder.input_document(
        "gone.json",
        DocKind::Other,
        "",
        Some(&json!({"x": 1})),
        None,
    );
    let report = builder.finish();
    assert_eq!(report.inputs[0].kind, ChangeKind::Added);
    assert_eq!(report.inputs[1].kind, ChangeKind::Removed);
}

#[test]
fn ordered_lists_report_moves_and_the_json_is_versioned() {
    let a = json!({"items": [{"id": "a"}, {"id": "b"}, {"id": "c"}, {"id": "d"}]});
    let b = json!({"items": [{"id": "d"}, {"id": "a"}, {"id": "b"}, {"id": "c"}]});
    let (raw, _) = diff_values(&a, &b, 100);
    assert_eq!(raw.len(), 1);
    assert_eq!(raw[0].kind, ChangeKind::Moved);
    assert_eq!(raw[0].path, "items[id=d]");
    let report = diff_models("a", &a, "b", &b);
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["schema"], "faris-diff/v1");
    for key in [
        "a",
        "b",
        "compared",
        "identical",
        "summary",
        "inputs",
        "evidence",
        "results",
        "presentation",
        "components",
        "impact",
    ] {
        assert!(json.get(key).is_some(), "{key}");
    }
}
