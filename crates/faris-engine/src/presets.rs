//! The operating-assumption presets offered next to the loaded assumptions, and
//! how a recorded preset plus edited what-if values select the assumptions a
//! study was last viewed with. Study files record the stable id; files saved
//! before ids existed hold the old display name, which still resolves. Shared by the desktop and the command line so
//! both recalculate the same histories from a study file.

use faris_model::history::OperatingHistoryAssumptions;

/// Name of the preset made from the study's own assumption file.
pub const LOADED_PRESET: &str = "Loaded assumptions";

/// Stable id of the loaded-assumptions preset.
pub const LOADED_PRESET_ID: &str = "loaded";

/// Shown with the permanent-trip preset: it is a numerical control only.
pub const PERMANENT_TRIP_NOTE: &str = "Numerical control only: it deliberately trips the magnets permanently to exercise the replacement and shutdown logic. This preset ends with negative net electricity and is not a plant scenario.";

#[derive(Clone, Debug)]
pub struct Preset {
    /// Recorded in study files; never changes when the display name does.
    pub id: &'static str,
    /// Short noun phrase shown in the selector.
    pub name: String,
    /// Display names this preset had in earlier versions, still accepted from
    /// saved study files.
    pub legacy_names: &'static [&'static str],
    /// One sentence of detail for the hover text.
    pub detail: &'static str,
    pub assumptions: OperatingHistoryAssumptions,
    /// Extra caution shown ahead of the preset's numbers, if any.
    pub extra_note: Option<&'static str>,
}

impl Preset {
    /// Whether a recorded key (id, current name or an earlier name) names this preset.
    pub fn matches(&self, key: &str) -> bool {
        self.id == key || self.name == key || self.legacy_names.contains(&key)
    }
}

fn bundled(bytes: &[u8]) -> Result<OperatingHistoryAssumptions, String> {
    let a: OperatingHistoryAssumptions =
        serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    a.validate()?;
    Ok(a)
}

/// The four presets, demountable magnets first (the default view).
pub fn operating_presets(loaded: &OperatingHistoryAssumptions) -> Result<Vec<Preset>, String> {
    let preset = |(id, name, legacy_names, detail): (
        &'static str,
        &str,
        &'static [&'static str],
        &'static str,
    ),
                  assumptions,
                  extra_note| Preset {
        id,
        name: name.into(),
        legacy_names,
        detail,
        assumptions,
        extra_note,
    };
    Ok(vec![
        preset(
            (
                "demountable-magnets",
                "Demountable magnets",
                &["Demountable magnets · REBCO fluence limit"],
                "The default. The magnet envelope is replaceable at a literature REBCO fluence limit.",
            ),
            bundled(include_bytes!(
                "../../../scenarios/arc-inspired/demountable-magnet-assumptions.json"
            ))?,
            None,
        ),
        preset(
            (
                LOADED_PRESET_ID,
                LOADED_PRESET,
                &[],
                "The operating assumptions loaded with the study.",
            ),
            loaded.clone(),
            None,
        ),
        preset(
            (
                "authored-baseline",
                "Authored baseline",
                &["Baseline authored scenario"],
                "The authored baseline operating scenario.",
            ),
            bundled(include_bytes!(
                "../../../scenarios/arc-inspired/demo-operating-assumptions.json"
            ))?,
            None,
        ),
        preset(
            (
                "permanent-trip",
                "Permanent-trip test",
                &["Permanent-trip test (numerical control)"],
                "A numerical control, not a plant scenario.",
            ),
            bundled(include_bytes!(
                "../../../scenarios/arc-inspired/demo-event-assumptions.json"
            ))?,
            Some(PERMANENT_TRIP_NOTE),
        ),
    ])
}

/// The service limit of one component, if the assumptions give one.
pub fn service_limit(assumptions: &OperatingHistoryAssumptions, component: &str) -> Option<f64> {
    assumptions
        .service_limits
        .iter()
        .find(|l| l.component_id == component)
        .map(|l| l.limit)
}

/// Which preset and assumptions a recorded view selects: the recorded preset (id or name)
/// (unknown keys leave the default, the first), then the edited what-if
/// values on top when they validate. Returns the preset index and the
/// assumptions to calculate with.
pub fn resolve_view(
    presets: &[Preset],
    preset: Option<&str>,
    what_if: Option<&OperatingHistoryAssumptions>,
) -> Option<(usize, OperatingHistoryAssumptions)> {
    let mut index = 0;
    if let Some(found) = preset.and_then(|key| presets.iter().position(|p| p.matches(key))) {
        index = found;
    }
    let mut assumptions = presets.get(index)?.assumptions.clone();
    if let Some(values) = what_if
        && values.validate().is_ok()
    {
        assumptions = values.clone();
    }
    Some((index, assumptions))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded() -> OperatingHistoryAssumptions {
        bundled(include_bytes!(
            "../../../scenarios/arc-inspired/demo-operating-assumptions.json"
        ))
        .unwrap()
    }

    #[test]
    fn unknown_preset_keeps_the_default_and_edits_apply_on_top() {
        let presets = operating_presets(&loaded()).unwrap();
        let (index, _) = resolve_view(&presets, Some("No such preset"), None).unwrap();
        assert_eq!(index, 0);
        assert_eq!(presets[2].id, "authored-baseline");
        let mut edited = presets[2].assumptions.clone();
        edited.recovery_fraction = 0.5;
        let (index, chosen) =
            resolve_view(&presets, Some("authored-baseline"), Some(&edited)).unwrap();
        assert_eq!(index, 2);
        assert_eq!(chosen.recovery_fraction, 0.5);
    }

    #[test]
    fn files_saved_with_an_old_display_name_still_restore_the_preset() {
        let presets = operating_presets(&loaded()).unwrap();
        for (index, old) in [
            "Demountable magnets · REBCO fluence limit",
            "Loaded assumptions",
            "Baseline authored scenario",
            "Permanent-trip test (numerical control)",
        ]
        .iter()
        .enumerate()
        {
            let (found, _) = resolve_view(&presets, Some(old), None).unwrap();
            assert_eq!(found, index, "{old}");
            let (by_id, _) = resolve_view(&presets, Some(presets[index].id), None).unwrap();
            assert_eq!(by_id, index);
        }
    }

    #[test]
    fn preset_names_are_short_sentence_case_noun_phrases() {
        for preset in operating_presets(&loaded()).unwrap() {
            assert!(preset.name.chars().count() <= 24, "{}", preset.name);
            assert!(preset.name.chars().next().unwrap().is_uppercase());
            assert!(!preset.name.contains(['·', '(']), "{}", preset.name);
            assert!(
                preset
                    .id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '-')
            );
        }
    }
}
