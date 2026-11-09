// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! Every entry runs on a few plain inputs and on any seed corpus present, so a change that makes a
//! parser panic is caught by `cargo test` without the fuzzer (QA-033).

use faris_fuzz_entries::ENTRIES;

// Verifies: QA-033
#[test]
fn every_entry_survives_plain_inputs_and_the_seed_corpus() {
    let plain: &[&[u8]] = &[
        b"",
        b"{}",
        b"[]",
        b"null",
        b"\xff\xff\xff\xff\xff\xff\xff\xff",
        b"{\"schema_version\": 1, \"schema\": \"x\", \"steps\": [[1]], \"files\": {}}",
        b"\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
    ];
    for (name, entry) in ENTRIES {
        for input in plain {
            entry(input);
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("corpus")
            .join(name);
        if let Ok(read) = std::fs::read_dir(dir) {
            for file in read.flatten() {
                entry(&std::fs::read(file.path()).unwrap());
            }
        }
    }
}

#[test]
fn the_entry_names_are_unique_and_every_one_has_a_fuzz_target_file() {
    let targets = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fuzz_targets");
    let mut seen = std::collections::BTreeSet::new();
    for (name, _) in ENTRIES {
        assert!(seen.insert(*name), "{name} is listed twice");
        assert!(
            targets.join(format!("{name}.rs")).is_file(),
            "{name} has no fuzz_targets/{name}.rs"
        );
    }
}

#[test]
fn spatial_dose_parser_fixtures_cover_valid_curve_and_result_and_reject_malformed_versions() {
    let curves = include_bytes!("fixtures/spatial-dose-curves-v02.json");
    let bundle = faris_engine::maintenance::spatial_dose::CurveBundle::parse(curves)
        .expect("synthetic structural curve fixture parses");
    assert_eq!(bundle.curves.len(), 1);
    assert_eq!(bundle.curves[0].points.len(), 1);
    assert!(bundle.curves[0].points[0].d1s.is_none());
    assert_eq!(
        bundle.curves[0].points[0].r2s.method,
        faris_model::maintenance::DoseMethod::R2s
    );

    let result = include_bytes!("fixtures/dose-maintenance-result-v02.json");
    let parsed = faris_engine::maintenance::parse_dose_result(result)
        .expect("synthetic dose-result fixture parses");
    assert_eq!(parsed.designs.len(), 1);
    assert_eq!(
        parsed.assumptions.governing_quantity,
        faris_model::maintenance::GoverningQuantity::Dose
    );

    assert!(
        faris_engine::maintenance::spatial_dose::CurveBundle::parse(include_bytes!(
            "fixtures/spatial-dose-curves-invalid-schema.json"
        ))
        .is_err()
    );
    assert!(
        faris_engine::maintenance::parse_dose_result(include_bytes!(
            "fixtures/dose-maintenance-result-invalid-schema.json"
        ))
        .is_err()
    );

    let mut wrong_governing_quantity: serde_json::Value = serde_json::from_slice(result).unwrap();
    wrong_governing_quantity["assumptions"]["governing_quantity"] = "heat".into();
    assert!(
        faris_engine::maintenance::parse_dose_result(
            &serde_json::to_vec(&wrong_governing_quantity).unwrap()
        )
        .is_err()
    );

    for receipt in [
        include_bytes!("fixtures/dose-cad-preparation-receipt.json").as_slice(),
        include_bytes!("fixtures/r2s-event-receipt.json").as_slice(),
        include_bytes!("fixtures/r2s-event-receipt-invalid-status.json").as_slice(),
    ] {
        faris_fuzz_entries::maintenance_spatial_dose(receipt);
    }
}
