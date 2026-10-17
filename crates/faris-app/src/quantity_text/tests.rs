use super::*;
use crate::settings::Settings;
use faris_engine::transport::{NormalizedTally, PhysicalUnit, TallyEstimator};
use faris_model::transport::{ResponseDomain, ScoreDefinition};
use sha2::{Digest, Sha256};

/// Preferences from a list of choices, read the way the desktop reads its settings file, with no
/// environment variable involved.
fn prefs_of(choices: &[(&str, &str)]) -> DisplayPrefs {
    let mut settings = Settings::default();
    for (key, value) in choices {
        settings.set_value(key, value).unwrap();
    }
    derive(&settings, Path::new(""), &|_| None)
}

fn flux_tally() -> NormalizedTally {
    NormalizedTally {
        response_id: "magnets-flux".into(),
        domain: ResponseDomain::WholeModel,
        score: ScoreDefinition::Flux,
        estimator: TallyEstimator::Tracklength,
        mean: 1.5e18,
        standard_error: 1.5e16,
        unit: PhysicalUnit::NeutronsPerSquareMetreSecond,
        integrated_mean: 5.0e8,
        integrated_standard_error: 2.5e6,
        integrated_unit: PhysicalUnit::Watts,
        volume_m3: 1.0,
        volume_standard_error_m3: 0.0,
    }
}

/// The text of four representative quantities, as the panels write them.
fn snapshot(p: &DisplayPrefs) -> Vec<String> {
    let t = flux_tally();
    vec![
        measurement_in_with(p, "particle_flux", "n/m2/s", t.mean, t.standard_error),
        measurement_in_with(
            p,
            "power",
            "MW",
            t.integrated_mean,
            t.integrated_standard_error,
        ),
        quantity_with(p, "time", "d", 30.0 * units::DAY_S, 1),
        quantity_with(p, "service_time", "yr", 7.0 * units::JULIAN_YEAR_S, 1),
        quantity_with(p, "mass", "kg", 1.25, 2),
    ]
}

// Verifies: CFG-040, UX-066, L10N-022
#[test]
fn the_text_of_a_widget_follows_the_unit_set_across_three_sets() {
    let si = prefs_of(&[("units.system", "si")]);
    assert_eq!(
        snapshot(&si),
        [
            "(1.500 ± 0.015)e18 n/m2/s (1 σ standard error)",
            "(5.000 ± 0.025)e8 W (1 σ standard error)",
            "2592000.0 s",
            "220903200.0 s",
            "1.25 kg",
        ]
    );
    let fusion = prefs_of(&[("units.system", "fusion")]);
    assert_eq!(
        snapshot(&fusion),
        [
            "(1.500 ± 0.015)e14 n/cm2/s (1 σ standard error)",
            "500.0 ± 2.5 MW (1 σ standard error)",
            "30.0 d",
            "7.0 yr",
            "1.25 kg",
        ]
    );
    // An override beats the system: SI everywhere except power in GW and time in hours.
    let mixed = prefs_of(&[
        ("units.system", "si"),
        ("units.power", "GW"),
        ("units.time", "h"),
    ]);
    assert_eq!(
        snapshot(&mixed),
        [
            "(1.500 ± 0.015)e18 n/m2/s (1 σ standard error)",
            "0.5000 ± 0.0025 GW (1 σ standard error)",
            "720.0 h",
            "220903200.0 s",
            "1.25 kg",
        ]
    );
    // Every unit is printed beside its number, whichever set is chosen.
    for set in [&si, &fusion, &mixed] {
        for text in snapshot(set) {
            assert!(text.chars().any(char::is_alphabetic), "{text}");
        }
    }
}

// Verifies: CFG-040, UX-066
#[test]
fn with_nothing_chosen_the_panels_keep_their_usual_units() {
    let none = prefs_of(&[]);
    assert_eq!(
        quantity_with(&none, "time", "d", 30.0 * units::DAY_S, 1),
        "30.0 d"
    );
    assert_eq!(unit_symbol_with(&none, "power", "MW"), "MW");
    // Choosing a system or a unit replaces the habit.
    let fusion = prefs_of(&[("units.system", "fusion")]);
    assert_eq!(unit_symbol_with(&fusion, "power", "GW"), "MW");
    let own = prefs_of(&[("units.power", "kW")]);
    assert_eq!(unit_symbol_with(&own, "power", "MW"), "kW");
}

// Verifies: CFG-037
#[test]
fn numbers_follow_the_decimal_mark_and_grouping_setting() {
    let comma = prefs_of(&[("format.decimal", "comma"), ("format.grouping", "point")]);
    assert_eq!(
        quantity_with(&comma, "mass", "kg", 12345.678, 2),
        "12.345,68 kg"
    );
    let text = measurement_with(&comma, 0.1234, 0.0012);
    assert_eq!(text, "0,1234 ± 0,0012 (1 σ standard error)");
    let plain = prefs_of(&[]);
    assert_eq!(
        measurement_with(&plain, 0.1234, 0.0012),
        "0.1234 ± 0.0012 (1 σ standard error)"
    );
}

// Verifies: UNC-081, UNC-082
#[test]
fn a_value_with_an_uncertainty_has_digits_from_the_uncertainty_and_its_convention() {
    let p = prefs_of(&[]);
    let text = measurement_with(&p, 0.123_456, 0.003_2);
    // The last digit shown is the second digit of the uncertainty, and the convention follows.
    assert_eq!(text, "0.1235 ± 0.0032 (1 σ standard error)");
    // The convention is written in the text itself, not in a hover.
    for (value, error) in [(1.234_567, 0.012_3), (3.2e-7, 1.0e-9), (4.5e20, 3.0e18)] {
        let text = measurement_with(&p, value, error);
        assert!(text.ends_with("(1 σ standard error)"), "{text}");
        assert_eq!(text.matches('±').count(), 1, "{text}");
    }
    // No usable uncertainty: no interval is claimed and no convention is written.
    let none = measurement_with(&p, 0.5, 0.0);
    assert!(
        none.contains("no uncertainty available") && !none.contains('σ'),
        "{none}"
    );
}

// Verifies: L10N-021, CFG-040
#[test]
fn a_typed_quantity_of_the_wrong_dimension_is_refused_with_the_accepted_units() {
    let fusion = prefs_of(&[("units.system", "fusion")]);
    // A bare number is taken in the unit shown; any unit of the same dimension is accepted.
    assert_eq!(
        parse_with(&fusion, "time", "d", "2"),
        Ok(2.0 * units::DAY_S)
    );
    assert_eq!(parse_with(&fusion, "time", "d", "36 h"), Ok(36.0 * 3600.0));
    let plain = prefs_of(&[]);
    assert_eq!(parse_with(&plain, "time", "d", "2"), Ok(2.0 * units::DAY_S));
    assert_eq!(parse_with(&plain, "power", "MW", "1.5"), Ok(1.5e6));
    // A mass for a duration, a dose for a length and an unknown unit are all refused.
    for (group, text) in [("time", "5 kg"), ("length", "3 Gy"), ("time", "4 furlongs")] {
        let message = parse_with(&plain, group, "d", text).unwrap_err();
        assert!(
            message.contains("Next step") || message.contains("accepted"),
            "{message}"
        );
        assert!(message.len() > 30, "{message}");
    }
}

// Verifies: L10N-023
#[test]
fn switching_units_changes_no_stored_byte_and_no_result_hash() {
    let dir = tempfile::tempdir().unwrap();
    let settings_file = dir.path().join("settings.json");
    let result_file = dir.path().join("result.json");
    let tally = flux_tally();
    std::fs::write(&result_file, serde_json::to_vec_pretty(&tally).unwrap()).unwrap();
    let digest = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
    let stored = digest(&std::fs::read(&result_file).unwrap());
    let value = digest(&serde_json::to_vec(&tally).unwrap());
    let mut shown = Vec::new();
    let mut choices = Settings::default();
    for (key, text) in [
        ("units.system", "fusion"),
        ("units.power", "GW"),
        ("units.system", "si"),
        ("format.decimal", "comma"),
    ] {
        choose_in(&mut choices, Some(&settings_file), key, text).unwrap();
        let p = prefs_from_file(&settings_file);
        shown.push(snapshot(&p));
        assert_eq!(
            digest(&std::fs::read(&result_file).unwrap()),
            stored,
            "{key}"
        );
        assert_eq!(digest(&serde_json::to_vec(&tally).unwrap()), value, "{key}");
    }
    // The choices did change what is shown, and only the settings file recorded them.
    assert_ne!(shown[0], shown[1]);
    assert_ne!(shown[1], shown[2]);
    assert!(shown[3][0].contains("1,500"), "{:?}", shown[3]);
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&settings_file).unwrap()).unwrap();
    assert_eq!(written["values"]["units.system"], "si");
    assert_eq!(written["values"]["format.decimal"], "comma");
    // A refused choice changes neither the session nor the file.
    let before = std::fs::read(&settings_file).unwrap();
    assert!(choose_in(&mut choices, Some(&settings_file), "units.power", "kg").is_err());
    assert_eq!(std::fs::read(&settings_file).unwrap(), before);
    forget_in(&mut choices, Some(&settings_file), "units.system");
    assert_eq!(prefs_from_file(&settings_file).system.name(), "si");
}

// Verifies: NUC-049
#[test]
fn the_transport_panel_note_counts_every_voxel_of_the_mesh() {
    let voxel = |bin: u64, error: f64| NormalizedTally {
        domain: ResponseDomain::Mesh {
            mesh_id: "m".into(),
            bin,
        },
        standard_error: error,
        mean: 1.0,
        ..flux_tally()
    };
    let results = [voxel(0, 0.02), voxel(1, 0.2), voxel(2, 0.04)];
    // Four voxels in the mesh and only three scored: the fourth counts as not converged.
    let note = mesh_note(&results, "m", 4).unwrap();
    assert!(
        note.starts_with("Mesh convergence: 2 of 4 voxels (50.0 %)"),
        "{note}"
    );
    assert!(
        note.contains("1 unscored") && note.contains("worst relative error 20.0 %"),
        "{note}"
    );
    assert_eq!(mesh_note(&results, "other", 4), None);
}

// Verifies: NUC-049
#[test]
fn the_convergence_summary_text_names_the_voxels_and_the_worst_error() {
    let c = faris_model::tally_statistics::mesh_convergence(
        &[1.0, 1.0, 1.0, 1.0, 0.0],
        &[0.02, 0.04, 0.08, 0.2, 0.0],
    )
    .unwrap();
    let text = convergence_text(&c);
    assert!(
        text.starts_with("Mesh convergence: 3 of 5 voxels (60.0 %)"),
        "{text}"
    );
    assert!(text.contains("2 ("), "{text}");
    assert!(
        text.contains("1 unscored") && text.ends_with("worst relative error 20.0 %."),
        "{text}"
    );
}
