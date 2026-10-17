use super::*;

// Verifies: L10N-021
#[test]
fn a_field_refuses_a_wrong_dimension_and_a_value_outside_its_range() {
    let p = DisplayPrefs::default();
    let range = 1.0..=2_592_000.0;
    assert_eq!(entry_value(&p, "time", "s", "90", &range), Ok(90.0));
    assert_eq!(entry_value(&p, "time", "s", "2 h", &range), Ok(7200.0));
    // 1000 random-looking entries with units of another quantity are all refused with a message
    // that names what is accepted; none changes the value.
    let wrong = [
        "kg", "MW", "keV", "Gy", "cm", "barn", "Sv", "n/cm2/s", "dpa", "Bq",
    ];
    let mut refused = 0;
    for i in 0..1000 {
        let number = (i * 7919 % 9973) as f64 / 10.0 + 1.0;
        let unit = wrong[i % wrong.len()];
        let message = entry_value(&p, "time", "s", &format!("{number} {unit}"), &range)
            .expect_err("a wrong dimension must be refused");
        assert!(
            message.contains("Next step") || message.contains("accepted"),
            "{message}"
        );
        refused += 1;
    }
    assert_eq!(refused, 1000);
    let outside = entry_value(&p, "time", "s", "40 d", &range).unwrap_err();
    assert!(outside.contains("outside the allowed range") && outside.contains("Next step"));
    assert!(entry_value(&p, "time", "s", "abc", &range).is_err());
}

// Verifies: L10N-021, CFG-040
#[test]
fn a_refused_entry_keeps_the_value_and_says_why_and_an_accepted_one_replaces_it() {
    let p = DisplayPrefs::default();
    let range = 1.0..=2_592_000.0;
    let mut seconds = 3600.0;
    let mut draft = Draft {
        text: "5 kg".into(),
        error: None,
    };
    assert!(!apply_entry(
        &mut draft,
        &p,
        "time",
        "s",
        &mut seconds,
        &range
    ));
    assert_eq!(seconds, 3600.0);
    assert!(
        draft
            .error
            .as_deref()
            .is_some_and(|m| m.contains("Next step"))
    );
    draft.text = "2 h".into();
    assert!(apply_entry(
        &mut draft,
        &p,
        "time",
        "s",
        &mut seconds,
        &range
    ));
    assert_eq!((seconds, draft.error.is_none()), (7200.0, true));
}

// Verifies: CFG-040
#[test]
fn the_field_runs_headless_and_shows_a_stored_value_in_the_unit_chosen() {
    let ctx = egui::Context::default();
    let mut seconds = 3600.0;
    ctx.run_ui(egui::RawInput::default(), |ui| {
        let changed = quantity_field(ui, "limit", "time", "s", &mut seconds, 1.0..=2_592_000.0);
        assert!(!changed);
    })
    .drop_without_applying_deltas();
    assert_eq!(seconds, 3600.0);
    assert_eq!(shown(&DisplayPrefs::default(), "time", "s", 3600.0), "3600");
    assert_eq!(
        shown(&DisplayPrefs::default(), "time", "d", 3600.0),
        "0.04167"
    );
}
