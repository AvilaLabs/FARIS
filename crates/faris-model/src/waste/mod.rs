//! Waste classification and clearance of activated components (requirements ACT-030 to ACT-040).
//!
//! Self-contained on purpose: nothing here names a FARIS scenario, so the tables and the
//! classifier can move to another crate unchanged. The input is a nuclide inventory per
//! component (`WasteInventory`); whatever produces the inventory (an ACTINV run, a file)
//! fills it. Every result carries the scheme name, version and date, and the statement that the
//! result is research screening and not a regulatory determination.

pub mod classify;
pub mod clearance;
pub mod lifetime;
pub mod tables;

use serde::{Deserialize, Serialize};

use crate::math;

/// Seconds in a Julian year (365.25 days), the year used for every decay time here.
pub const YEAR_S: f64 = crate::units::JULIAN_YEAR_S;
/// Becquerel per curie (exact, by definition).
pub use crate::units::CURIE_BQ;
/// Fractions and sums that differ from a bound by no more than this relative amount are on it,
/// so a sum of exactly 1 stays inside its class and floating-point noise cannot move it.
pub const BOUNDARY_TOLERANCE: f64 = 1e-9;

/// Waste form. The activated-metal columns of 10 CFR 61.55 apply only to `ActivatedMetal`.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WasteForm {
    ActivatedMetal,
    Other,
}

impl WasteForm {
    pub fn text(self) -> &'static str {
        match self {
            WasteForm::ActivatedMetal => "activated metal",
            WasteForm::Other => "other waste form",
        }
    }
}

/// One nuclide of an inventory, as activity in becquerel at the inventory time.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NuclideActivity {
    /// `Co-60`, `Ag-110m`: element symbol, hyphen, mass number, optional `m`.
    pub nuclide: String,
    pub activity_bq: f64,
    pub half_life_s: f64,
    /// Whether the nuclide decays by alpha emission. Needed only for a transuranic nuclide
    /// with a half-life over 5 years; the regulation's group row is for alpha emitters.
    #[serde(default)]
    pub alpha_emitter: Option<bool>,
    /// Set when this entry is a decay product whose dose is counted with its parent (for
    /// example `Ba-137m` with `Cs-137`). A progeny entry is not counted in any sum.
    #[serde(default)]
    pub progeny_of: Option<String>,
}

/// The activated inventory of one component and how its concentration is averaged.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WasteInventory {
    pub component_id: String,
    pub waste_form: WasteForm,
    /// Volume the activity is averaged over (10 CFR 61.55(a)(8)), for the curie per cubic
    /// metre limits. The caller states which volume this is, for example the displaced volume.
    #[serde(default)]
    pub volume_m3: Option<f64>,
    /// Mass the activity is averaged over, for the nanocurie per gram limits and clearance.
    #[serde(default)]
    pub mass_g: Option<f64>,
    /// Years after the reference time (for example shutdown) at which the activities are given.
    #[serde(default)]
    pub inventory_time_years: f64,
    /// True when the activation calculation covered the alpha-emitting transuranic nuclides,
    /// so that none listed means none produced.
    #[serde(default)]
    pub transuranics_evaluated: bool,
    /// What produced the inventory, for the kind label on the result (for example `solver_result`).
    pub inventory_kind: String,
    pub nuclides: Vec<NuclideActivity>,
}

/// A refusal to give a value, with the reason and what to do next.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct NotEvaluated {
    pub reason: String,
    pub missing: Vec<String>,
    pub next_step: String,
}

impl NotEvaluated {
    pub fn new(
        reason: impl Into<String>,
        missing: Vec<String>,
        next_step: impl Into<String>,
    ) -> Self {
        Self {
            reason: reason.into(),
            missing,
            next_step: next_step.into(),
        }
    }
}

/// Name, version and date of a classification scheme, written on every result.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SchemeInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub date: String,
    pub citation: String,
    /// `IMPLEMENTED`, or `NOT_EVALUATED` when FARIS does not yet carry the scheme.
    pub status: String,
    pub status_reason: Option<String>,
}

/// Every scheme FARIS lists. Two are implemented; the others say why they are not.
pub fn schemes() -> Vec<SchemeInfo> {
    let nrc = tables::nrc();
    let iaea = tables::iaea();
    let planned = |id: &str, name: &str, citation: &str| {
        SchemeInfo {
        id: id.to_string(),
        name: name.to_string(),
        version: "not loaded".to_string(),
        date: "not loaded".to_string(),
        citation: citation.to_string(),
        status: "NOT_EVALUATED".to_string(),
        status_reason: Some(
            "FARIS has not transcribed and checked this scheme's values from its primary source, \
             so it gives no result under it; use the NRC or IAEA scheme, or supply the limits as authored values"
                .to_string(),
        ),
    }
    };
    vec![
        nrc.scheme_info(),
        iaea.scheme_info(),
        planned(
            "uk-llw-ilw",
            "UK low-level and intermediate-level waste thresholds",
            "UK Low Level Waste Policy, 4 and 12 GBq per tonne boundaries",
        ),
        planned(
            "fetter-specific-activity",
            "Fetter, Cheng and Mann specific-activity limits",
            "S. Fetter, E.T. Cheng, F.M. Mann, Long-term radioactive waste from fusion reactors: Part II, Fusion Eng. Des. 13 (1990)",
        ),
    ]
}

/// Years to seconds.
pub fn years_to_seconds(years: f64) -> f64 {
    years * YEAR_S
}

/// Activity left after `years` of decay of a nuclide with the given half-life. No ingrowth.
pub fn decayed_activity_bq(activity_bq: f64, half_life_s: f64, years: f64) -> f64 {
    activity_bq * math::powf(2.0, -years_to_seconds(years) / half_life_s)
}

/// True when `name` looks like `Co-60` or `Ag-110m`.
pub fn is_nuclide_name(name: &str) -> bool {
    let Some((symbol, rest)) = name.split_once('-') else {
        return false;
    };
    let mut symbol_chars = symbol.chars();
    let symbol_ok = match (
        symbol_chars.next(),
        symbol_chars.next(),
        symbol_chars.next(),
    ) {
        (Some(a), None, None) => a.is_ascii_uppercase(),
        (Some(a), Some(b), None) => a.is_ascii_uppercase() && b.is_ascii_lowercase(),
        _ => false,
    };
    let digits = rest.strip_suffix('m').unwrap_or(rest);
    symbol_ok
        && !digits.is_empty()
        && digits.len() <= 3
        && digits.chars().all(|c| c.is_ascii_digit())
}

/// Element symbol of a nuclide name (`Co-60` gives `Co`).
pub fn element_symbol(name: &str) -> &str {
    name.split_once('-').map_or(name, |(symbol, _)| symbol)
}

/// True for the elements above uranium (atomic number over 92).
pub fn is_transuranic_element(symbol: &str) -> bool {
    const TRANSURANIC: [&str; 26] = [
        "Np", "Pu", "Am", "Cm", "Bk", "Cf", "Es", "Fm", "Md", "No", "Lr", "Rf", "Db", "Sg", "Bh",
        "Hs", "Mt", "Ds", "Rg", "Cn", "Nh", "Fl", "Mc", "Lv", "Ts", "Og",
    ];
    TRANSURANIC.contains(&symbol)
}

/// Checks that an inventory is usable at all (names, numbers, duplicates), before any rule.
pub fn validate_inventory(inventory: &WasteInventory) -> Result<(), NotEvaluated> {
    let next = "Correct the inventory file and run again.";
    let mut seen = std::collections::BTreeSet::new();
    for n in &inventory.nuclides {
        if !is_nuclide_name(&n.nuclide) {
            return Err(NotEvaluated::new(
                format!(
                    "`{}` is not a nuclide name like Co-60 or Ag-110m",
                    n.nuclide
                ),
                vec![n.nuclide.clone()],
                next,
            ));
        }
        if !seen.insert(n.nuclide.clone()) {
            return Err(NotEvaluated::new(
                format!("{} is listed twice", n.nuclide),
                vec![n.nuclide.clone()],
                "Sum the entries into one before classifying.",
            ));
        }
        if !n.activity_bq.is_finite() || n.activity_bq < 0.0 {
            return Err(NotEvaluated::new(
                format!(
                    "the activity of {} is not a finite number of at least 0",
                    n.nuclide
                ),
                vec![n.nuclide.clone()],
                next,
            ));
        }
        if !n.half_life_s.is_finite() || n.half_life_s <= 0.0 {
            return Err(NotEvaluated::new(
                format!(
                    "the half-life of {} is missing or not positive; it is needed to decay the inventory and to tell short-lived nuclides apart",
                    n.nuclide
                ),
                vec![n.nuclide.clone()],
                "Give the half-life in seconds from the decay data the activation calculation used.",
            ));
        }
    }
    for (what, value) in [
        ("volume_m3", inventory.volume_m3),
        ("mass_g", inventory.mass_g),
    ] {
        if let Some(v) = value
            && (!v.is_finite() || v <= 0.0)
        {
            return Err(NotEvaluated::new(
                format!("{what} must be a positive number"),
                vec![what.to_string()],
                next,
            ));
        }
    }
    if !inventory.inventory_time_years.is_finite() || inventory.inventory_time_years < 0.0 {
        return Err(NotEvaluated::new(
            "inventory_time_years must be 0 or more",
            vec!["inventory_time_years".to_string()],
            next,
        ));
    }
    Ok(())
}

/// Label of what a result is. Kept as an enum so no module holds a bare label constant.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResultKind {
    Calculated,
    Authored,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nuclide_names_are_checked() {
        for good in ["Co-60", "Ag-110m", "H-3", "Cm-242", "Ba-137m"] {
            assert!(is_nuclide_name(good), "{good}");
        }
        for bad in ["Co60", "co-60", "Co-", "Cobalt-60", "Co-6000", "Co-60x", ""] {
            assert!(!is_nuclide_name(bad), "{bad}");
        }
    }

    #[test]
    fn decay_halves_per_half_life() {
        let half_life_s = 5.0 * YEAR_S;
        let a = decayed_activity_bq(8.0, half_life_s, 10.0);
        assert!((a - 2.0).abs() < 1e-12);
    }

    #[test]
    fn schemes_name_version_and_date_and_say_why_a_scheme_is_missing() {
        let list = schemes();
        assert_eq!(list.len(), 4);
        for s in &list {
            assert!(!s.name.is_empty() && !s.version.is_empty() && !s.date.is_empty());
            if s.status == "NOT_EVALUATED" {
                assert!(s.status_reason.is_some());
            }
        }
        assert_eq!(list.iter().filter(|s| s.status == "IMPLEMENTED").count(), 2);
    }
}
