//! The operating-assumption presets offered next to the loaded assumptions, and
//! how a recorded preset name plus edited what-if values select the assumptions
//! a study was last viewed with. Shared by the desktop and the command line so
//! both recalculate the same histories from a study file.

use faris_model::history::OperatingHistoryAssumptions;

/// Name of the preset made from the study's own assumption file.
pub const LOADED_PRESET: &str = "Loaded assumptions";

/// Shown with the permanent-trip preset: it is a numerical control only.
pub const PERMANENT_TRIP_NOTE: &str = "Numerical control only: it deliberately trips the magnets permanently to exercise the replacement and shutdown logic. This preset ends with negative net electricity and is not a plant scenario.";

#[derive(Clone, Debug)]
pub struct Preset {
    pub name: String,
    pub assumptions: OperatingHistoryAssumptions,
    /// Extra caution shown ahead of the preset's numbers, if any.
    pub extra_note: Option<&'static str>,
}

fn bundled(bytes: &[u8]) -> Result<OperatingHistoryAssumptions, String> {
    let a: OperatingHistoryAssumptions =
        serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    a.validate()?;
    Ok(a)
}

/// The four presets, demountable magnets first (the default view).
pub fn operating_presets(loaded: &OperatingHistoryAssumptions) -> Result<Vec<Preset>, String> {
    let preset = |name: &str, assumptions, extra_note| Preset {
        name: name.into(),
        assumptions,
        extra_note,
    };
    Ok(vec![
        preset(
            "Demountable magnets · REBCO fluence limit",
            bundled(include_bytes!(
                "../../../scenarios/arc-inspired/demountable-magnet-assumptions.json"
            ))?,
            None,
        ),
        preset(LOADED_PRESET, loaded.clone(), None),
        preset(
            "Baseline authored scenario",
            bundled(include_bytes!(
                "../../../scenarios/arc-inspired/demo-operating-assumptions.json"
            ))?,
            None,
        ),
        preset(
            "Permanent-trip test (numerical control)",
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

/// Which preset and assumptions a recorded view selects: the named preset
/// (unknown names leave the default, the first), then the edited what-if
/// values on top when they validate. Returns the preset index and the
/// assumptions to calculate with.
pub fn resolve_view(
    presets: &[Preset],
    preset: Option<&str>,
    what_if: Option<&OperatingHistoryAssumptions>,
) -> Option<(usize, OperatingHistoryAssumptions)> {
    let mut index = 0;
    if let Some(found) = preset.and_then(|name| presets.iter().position(|p| p.name == name)) {
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
        let mut edited = presets[2].assumptions.clone();
        edited.recovery_fraction = 0.5;
        let (index, chosen) =
            resolve_view(&presets, Some("Baseline authored scenario"), Some(&edited)).unwrap();
        assert_eq!(index, 2);
        assert_eq!(chosen.recovery_fraction, 0.5);
    }
}
