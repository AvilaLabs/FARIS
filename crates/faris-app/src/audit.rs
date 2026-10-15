// Source-level audits of the desktop crate: rules that hold across every panel.

const SOURCES: &[(&str, &str)] = &[
    ("compare_panel.rs", include_str!("compare_panel.rs")),
    ("history_panel.rs", include_str!("history_panel.rs")),
    ("lib.rs", include_str!("lib.rs")),
    ("maintenance_panel.rs", include_str!("maintenance_panel.rs")),
    ("sweep_panel.rs", include_str!("sweep_panel.rs")),
    ("transport_panel.rs", include_str!("transport_panel.rs")),
    (
        "design_check_panel.rs",
        include_str!("design_check_panel.rs"),
    ),
    ("study_panel.rs", include_str!("study_panel.rs")),
    ("tour.rs", include_str!("tour.rs")),
];

/// The number in a font-size literal that starts at `rest`, if it is one.
fn literal_size(rest: &str) -> Option<f32> {
    let end = rest.find(')')?;
    rest[..end].trim().parse().ok()
}

// Verifies: A11Y-021
#[test]
fn no_text_style_is_set_below_eleven_pixels() {
    let mut checked = 0;
    for (name, source) in SOURCES {
        for marker in ["FontId::proportional(", "FontId::monospace(", ".size("] {
            for (at, _) in source.match_indices(marker) {
                if let Some(size) = literal_size(&source[at + marker.len()..]) {
                    checked += 1;
                    assert!(size >= 11.0, "{name}: {marker}{size}) is below 11 px");
                }
            }
        }
    }
    assert!(checked >= 8, "only {checked} font literals found");
}

// Verifies: A11Y-021
#[test]
fn every_text_style_is_at_least_eleven_pixels_and_body_text_thirteen() {
    let mut style = eframe::egui::Style::default();
    crate::apply_text_sizes(&mut style);
    for (text_style, font) in &style.text_styles {
        assert!(
            font.size >= crate::MIN_TEXT_PX,
            "{text_style:?} is {} px",
            font.size
        );
    }
    for body in [
        eframe::egui::TextStyle::Body,
        eframe::egui::TextStyle::Button,
    ] {
        assert!(style.text_styles[&body].size >= crate::BODY_TEXT_PX);
    }
}

// Verifies: UX-037
#[test]
fn viewport_hint_gives_a_key_for_every_drag() {
    let hint = crate::VIEWPORT_HELP;
    for needle in [
        "Drag to orbit",
        "Shift-drag to pan",
        "arrows orbit",
        "Shift and arrows pan",
        "+ and −",
        "0 resets",
    ] {
        assert!(hint.contains(needle), "hint lacks {needle}");
    }
}
