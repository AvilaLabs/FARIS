// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! The JSON examples of FARIS file formats in the handbook parse with the real parsers.
//!
//! `scripts/doc_samples.py` (QA-016) runs this test and counts it. Each handbook file lists how many
//! `json` blocks it holds, so a new example fails here until it is checked.

use faris_engine::maintenance::files::parse_designs;
use faris_engine::shutdown_dose::{
    CovarianceAssessment, ESTIMATE_SCHEMA, REQUEST_SCHEMA, parse_estimate_record, parse_request,
};
use faris_model::design::{NON_WAIVABLE_IDS, Waiver, waiver_id_is_known};
use faris_model::maintenance::MaintenanceAssumptions;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The bodies of the fenced `json` blocks of a handbook file, in order.
fn json_blocks(relative: &str) -> Vec<String> {
    let text = std::fs::read_to_string(root().join(relative)).expect("read the handbook file");
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        match (&mut current, line.trim_end()) {
            (None, "```json") => current = Some(String::new()),
            (Some(body), "```") => {
                blocks.push(std::mem::take(body));
                current = None;
            }
            (Some(body), _) => {
                body.push_str(line);
                body.push('\n');
            }
            _ => {}
        }
    }
    blocks
}

// Verifies: QA-016
#[test]
fn design_file_waivers_example_parses_and_names_waivable_checks() {
    let blocks = json_blocks("docs/guide/design-file.md");
    assert_eq!(blocks.len(), 1, "a new json example needs a check here");
    // The example is the `waivers` member of a design file; wrap it into an object.
    let value: serde_json::Value = serde_json::from_str(&format!("{{{}}}", blocks[0])).unwrap();
    let waivers: Vec<Waiver> = serde_json::from_value(value["waivers"].clone()).unwrap();
    assert!(!waivers.is_empty());
    for waiver in &waivers {
        assert!(waiver_id_is_known(&waiver.check_id), "{}", waiver.check_id);
        assert!(!NON_WAIVABLE_IDS.contains(&waiver.check_id.as_str()));
        assert!(!waiver.reason.is_empty() && !waiver.author.is_empty());
        let date: Vec<&str> = waiver.date.split('-').collect();
        assert_eq!(date.iter().map(|p| p.len()).collect::<Vec<_>>(), [4, 2, 2]);
    }
}

// Verifies: QA-016
#[test]
fn maintenance_examples_parse_and_validate() {
    let blocks = json_blocks("docs/guide/maintenance.md");
    assert_eq!(blocks.len(), 2, "a new json example needs a check here");

    // The designs file: its parser checks that the five named files exist, so make them.
    let dir = tempfile::tempdir().unwrap();
    let designs: serde_json::Value = serde_json::from_str(&blocks[0]).unwrap();
    for entry in designs["designs"].as_object().unwrap().values() {
        for key in [
            "scenario",
            "physics",
            "history_run",
            "spectrum_run",
            "history_assumptions",
        ] {
            let path = dir.path().join(entry[key].as_str().unwrap());
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"{}").unwrap();
        }
    }
    let resolved =
        parse_designs(blocks[0].as_bytes(), dir.path()).expect("the designs example parses");
    assert_eq!(resolved.keys().collect::<Vec<_>>(), ["port__reference"]);

    let assumptions: MaintenanceAssumptions =
        serde_json::from_str(&blocks[1]).expect("the assumptions example parses");
    assumptions
        .validate()
        .expect("the assumptions example validates");
    // The designs example names the design the assumptions calibrate against.
    for class in assumptions.classes.values() {
        if let faris_model::maintenance::Threshold::Calibrate { design, .. } = &class.threshold {
            assert!(
                resolved.contains_key(design),
                "{design} is not in the designs example"
            );
        }
    }
}

// Verifies: QA-016
#[test]
fn activation_zones_example_parses() {
    let blocks = json_blocks("docs/guide/activation.md");
    assert_eq!(blocks.len(), 1, "a new json example needs a check here");
    let zones: Vec<faris_engine::activation::ZoneSpec> = serde_json::from_str(&blocks[0]).unwrap();
    assert!(!zones.is_empty());
    for zone in &zones {
        assert!(zone.mass_g > 0.0 && zone.volume_m3 > 0.0 && !zone.composition.is_empty());
    }
}

// Verifies: QA-016, ACT-038
#[test]
fn waste_lifetime_plan_example_parses() {
    let blocks = json_blocks("docs/guide/waste.md");
    assert_eq!(blocks.len(), 1, "a new json example needs a check here");
    let plan: faris_engine::waste_lifetime::LifetimePlan =
        serde_json::from_str(&blocks[0]).unwrap();
    assert_eq!(plan.components.len(), 2);
    assert!(plan.components.iter().all(|c| c.mass_kg_each > 0.0));
}

// Verifies: QA-016
#[test]
fn shutdown_dose_request_and_estimate_examples_parse() {
    let blocks = json_blocks("docs/guide/shutdown-dose.md");
    assert_eq!(blocks.len(), 2, "a new json example needs a check here");

    let request = parse_request(blocks[0].as_bytes()).expect("the comparison request parses");
    assert_eq!(request.schema, REQUEST_SCHEMA);
    assert_eq!(request.case_id, "synthetic-format-example");
    assert_eq!(request.points.len(), 1);
    assert_eq!(request.points[0].cooling_time_s, 86_400.0);
    assert_eq!(request.points[0].covariance, CovarianceAssessment::Unknown);
    for reference in [&request.points[0].d1s, &request.points[0].r2s] {
        assert_eq!(reference.sha256, "0".repeat(64));
    }

    let estimate =
        parse_estimate_record(blocks[1].as_bytes()).expect("the method estimate record parses");
    assert_eq!(estimate.schema, ESTIMATE_SCHEMA);
    assert_eq!(estimate.method, faris_engine::shutdown_dose::Method::D1s);
    assert_eq!(estimate.cooling_time_s, request.points[0].cooling_time_s);
    assert_eq!(estimate.estimate.value, 1.0);
    assert_eq!(estimate.estimate.standard_error, 0.1);
    assert_eq!(estimate.random_streams, ["synthetic-stream-id".to_string()]);
    for reference in [
        &estimate.geometry,
        &estimate.materials,
        &estimate.source,
        &estimate.irradiation,
        &estimate.response,
        &estimate.location,
        &estimate.run_record,
    ] {
        assert_eq!(reference.sha256, "0".repeat(64));
    }
}
