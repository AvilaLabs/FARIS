//! Clearance index and recycling index (ACT-036, ACT-037).
//!
//! Clearance index: the sum over nuclides of the activity concentration (Bq/g, averaged over the
//! stated mass) divided by that nuclide's clearance level in IAEA GSR Part 3 Table I.2, the
//! summation rule of paragraph I.14. The index is below 1 when the material may be cleared under
//! that table; clearance itself is a regulator's decision, and this is research screening.
//!
//! Recycling index: a dose rate divided by a dose-rate criterion that the caller authors. FARIS
//! carries no criterion value of its own.

use serde::{Deserialize, Serialize};

use super::{
    NotEvaluated, ResultKind, SchemeInfo, WasteInventory, decayed_activity_bq, tables,
    validate_inventory,
};
use crate::{RESEARCH_SCREENING_STATEMENT, math};

/// Default clearance-time search window, years after the inventory reference (ACT-036).
pub const SEARCH_START_YEARS: f64 = 1.0;
pub const SEARCH_END_YEARS: f64 = 100.0;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ClearanceLine {
    pub nuclide: String,
    pub concentration_bq_per_g: f64,
    pub clearance_level_bq_per_g: f64,
    pub ratio: f64,
    /// The level includes progeny listed in the standard's note a.
    pub parent_with_progeny: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ClearanceIndex {
    pub scheme: SchemeInfo,
    pub component_id: String,
    pub time_years: f64,
    /// Sum of the ratios; `None` when the index could not be formed.
    pub index: Option<f64>,
    /// True when the index is below 1 (paragraph I.14: "less than").
    pub below_one: Option<bool>,
    pub lines: Vec<ClearanceLine>,
    pub progeny_not_counted: Vec<String>,
    pub not_evaluated: Option<NotEvaluated>,
    pub kind: ResultKind,
    pub inventory_kind: String,
    pub research_screening: String,
}

/// Index at `time_years` after the same reference as `inventory_time_years`.
pub fn clearance_index(inventory: &WasteInventory, time_years: f64) -> ClearanceIndex {
    let table = tables::iaea();
    let mut out = ClearanceIndex {
        scheme: table.scheme_info(),
        component_id: inventory.component_id.clone(),
        time_years,
        index: None,
        below_one: None,
        lines: Vec::new(),
        progeny_not_counted: Vec::new(),
        not_evaluated: None,
        kind: ResultKind::Calculated,
        inventory_kind: inventory.inventory_kind.clone(),
        research_screening: RESEARCH_SCREENING_STATEMENT.to_string(),
    };
    match index_lines(inventory, time_years) {
        Ok((lines, progeny)) => {
            let index: f64 = lines.iter().map(|l| l.ratio).sum();
            out.index = Some(index);
            out.below_one = Some(index < 1.0);
            out.lines = lines;
            out.progeny_not_counted = progeny;
        }
        Err(e) => out.not_evaluated = Some(e),
    }
    out
}

fn index_lines(
    inventory: &WasteInventory,
    time_years: f64,
) -> Result<(Vec<ClearanceLine>, Vec<String>), NotEvaluated> {
    validate_inventory(inventory)?;
    if !time_years.is_finite() || time_years < inventory.inventory_time_years {
        return Err(NotEvaluated::new(
            "the time is earlier than the inventory time; FARIS decays forward only",
            vec!["time_years".to_string()],
            "Give a time at or after the inventory time.",
        ));
    }
    let mass = inventory.mass_g.ok_or_else(|| {
        NotEvaluated::new(
            "clearance levels are activity per gram and the inventory gives no mass",
            vec!["mass_g".to_string()],
            "Give mass_g, the mass the activity is averaged over.",
        )
    })?;
    let table = tables::iaea();
    let dt = time_years - inventory.inventory_time_years;
    let mut lines = Vec::new();
    let mut progeny = Vec::new();
    let mut no_level: Vec<String> = Vec::new();
    for n in &inventory.nuclides {
        let activity = decayed_activity_bq(n.activity_bq, n.half_life_s, dt);
        if let Some(parent) = &n.progeny_of {
            let covered = table.level(parent).is_some_and(|l| l.parent_with_progeny)
                && inventory.nuclides.iter().any(|o| &o.nuclide == parent);
            if !covered {
                return Err(NotEvaluated::new(
                    format!(
                        "{} is declared a progeny of {parent}, but Table I.2 does not list {parent} as a parent whose progeny are included",
                        n.nuclide
                    ),
                    vec![n.nuclide.clone()],
                    "Remove progeny_of for this nuclide so it needs its own clearance level, or correct the parent.",
                ));
            }
            progeny.push(n.nuclide.clone());
            continue;
        }
        if activity <= 0.0 {
            continue;
        }
        match table.level(&n.nuclide) {
            Some(level) => {
                let c = activity / mass;
                lines.push(ClearanceLine {
                    nuclide: n.nuclide.clone(),
                    concentration_bq_per_g: c,
                    clearance_level_bq_per_g: level.bq_per_g,
                    ratio: c / level.bq_per_g,
                    parent_with_progeny: level.parent_with_progeny,
                });
            }
            None => no_level.push(n.nuclide.clone()),
        }
    }
    if !no_level.is_empty() {
        return Err(NotEvaluated::new(
            "these nuclides have activity and no level in Table I.2, so the index cannot be formed (a missing level is not zero)",
            no_level,
            "Mark a decay product counted with its parent using progeny_of, or use a clearance table that lists the nuclide.",
        ));
    }
    Ok((lines, progeny))
}

/// Where the clearance index falls below 1 in the search window.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ClearanceTime {
    /// Already below 1 at the start of the window.
    BelowOneAtWindowStart { years: f64 },
    /// First time below 1, years after the inventory reference.
    FallsBelowOneAt { years: f64 },
    /// Still at or above 1 at the end of the window.
    NotWithinWindow { index_at_window_end: f64 },
}

#[derive(Clone, Debug, Serialize)]
pub struct ClearanceTimeResult {
    pub scheme: SchemeInfo,
    pub component_id: String,
    pub window_start_years: f64,
    pub window_end_years: f64,
    pub time: Option<ClearanceTime>,
    pub index_at_window_start: Option<f64>,
    pub method: String,
    pub not_evaluated: Option<NotEvaluated>,
    pub kind: ResultKind,
    pub research_screening: String,
}

/// Searches `[start, end]` years for the time the clearance index falls below 1. With decay only
/// the index never rises, so bisection finds the crossing.
pub fn clearance_time(
    inventory: &WasteInventory,
    start_years: f64,
    end_years: f64,
) -> ClearanceTimeResult {
    time_to_below_one(
        tables::iaea().scheme_info(),
        inventory,
        start_years,
        end_years,
        false,
        "bisection on the decay-only clearance index (non-increasing in time); no ingrowth",
        |t| index_lines(inventory, t).map(|(lines, _)| lines.iter().map(|l| l.ratio).sum()),
    )
}

/// The search every index scheme shares: the first time in `[start, end]` at which `sum_at`
/// falls below 1 (`inclusive` false, the IAEA rule "less than") or to at most 1 (`inclusive`
/// true, the rule of the UK schedule, Fetter and VLLW sums, which are exceeded only above 1).
/// `sum_at` must be non-increasing in time, which decay without ingrowth gives.
pub fn time_to_below_one(
    scheme: SchemeInfo,
    inventory: &WasteInventory,
    start_years: f64,
    end_years: f64,
    inclusive: bool,
    method: &str,
    sum_at: impl Fn(f64) -> Result<f64, NotEvaluated>,
) -> ClearanceTimeResult {
    let mut out = ClearanceTimeResult {
        scheme,
        component_id: inventory.component_id.clone(),
        window_start_years: start_years,
        window_end_years: end_years,
        time: None,
        index_at_window_start: None,
        method: method.to_string(),
        not_evaluated: None,
        kind: ResultKind::Calculated,
        research_screening: RESEARCH_SCREENING_STATEMENT.to_string(),
    };
    if !(start_years.is_finite() && end_years.is_finite() && start_years < end_years) {
        out.not_evaluated = Some(NotEvaluated::new(
            "the search window needs a start before its end",
            vec!["window".to_string()],
            "Give a start and an end in years, start before end.",
        ));
        return out;
    }
    let met = |v: f64| {
        if inclusive {
            v <= 1.0 * (1.0 + super::BOUNDARY_TOLERANCE)
        } else {
            v < 1.0
        }
    };
    let first = match sum_at(start_years) {
        Ok(v) => v,
        Err(e) => {
            out.not_evaluated = Some(e);
            return out;
        }
    };
    out.index_at_window_start = Some(first);
    if met(first) {
        out.time = Some(ClearanceTime::BelowOneAtWindowStart { years: start_years });
        return out;
    }
    let last = match sum_at(end_years) {
        Ok(v) => v,
        Err(e) => {
            out.not_evaluated = Some(e);
            return out;
        }
    };
    if !met(last) {
        out.time = Some(ClearanceTime::NotWithinWindow {
            index_at_window_end: last,
        });
        return out;
    }
    let (mut lo, mut hi) = (start_years, end_years);
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        match sum_at(mid) {
            Ok(v) if met(v) => hi = mid,
            _ => lo = mid,
        }
    }
    out.time = Some(ClearanceTime::FallsBelowOneAt { years: hi });
    out
}

/// A dose-rate criterion written by the user. FARIS supplies no value.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RecyclingCriterion {
    pub name: String,
    pub dose_rate_limit_sv_per_h: f64,
    /// Where the number comes from (a document and page, or the author and reason).
    pub source: String,
}

/// Dose rate of one nuclide at the inventory time, with its uncertainty, at the criterion's
/// reference geometry (the caller's: contact, 1 m, handling).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DoseRateContribution {
    pub nuclide: String,
    pub half_life_s: f64,
    pub dose_rate_sv_per_h: f64,
    pub sigma_sv_per_h: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecyclingResult {
    /// Always present: a number is never shown without its criterion.
    pub criterion: RecyclingCriterion,
    pub criterion_kind: ResultKind,
    pub time_years: f64,
    pub dose_rate_sv_per_h: Option<f64>,
    pub index: Option<f64>,
    pub index_sigma: Option<f64>,
    /// `WITHIN` when the index is at most 1, `EXCEEDED` otherwise, `NOT_EVALUATED` without an index.
    pub verdict: String,
    pub sigma_method: String,
    pub not_evaluated: Option<NotEvaluated>,
    pub research_screening: String,
}

/// Recycling index at `time_years` after the reference time the contributions are given at.
/// Contributions decay by their own half-lives; their sigmas add linearly (fully correlated, the
/// cautious choice) and scale with the same decay factor.
pub fn recycling_index(
    criterion: &RecyclingCriterion,
    contributions: &[DoseRateContribution],
    contribution_time_years: f64,
    time_years: f64,
) -> RecyclingResult {
    let mut out = RecyclingResult {
        criterion: criterion.clone(),
        criterion_kind: ResultKind::Authored,
        time_years,
        dose_rate_sv_per_h: None,
        index: None,
        index_sigma: None,
        verdict: "NOT_EVALUATED".to_string(),
        sigma_method: "sigmas of the contributions add linearly (fully correlated, cautious)"
            .to_string(),
        not_evaluated: None,
        research_screening: RESEARCH_SCREENING_STATEMENT.to_string(),
    };
    let ne = |reason: &str, missing: &str, next: &str| {
        Some(NotEvaluated::new(reason, vec![missing.to_string()], next))
    };
    if criterion.name.trim().is_empty() || criterion.source.trim().is_empty() {
        out.not_evaluated = ne(
            "the criterion has no name or no source; no index is shown without its criterion",
            "criterion",
            "Give the criterion a name and the source of its value.",
        );
        return out;
    }
    if !criterion.dose_rate_limit_sv_per_h.is_finite() || criterion.dose_rate_limit_sv_per_h <= 0.0
    {
        out.not_evaluated = ne(
            "the criterion dose rate must be a positive number",
            "dose_rate_limit_sv_per_h",
            "Give the dose-rate limit in Sv per hour.",
        );
        return out;
    }
    if contributions.is_empty() || time_years < contribution_time_years || !time_years.is_finite() {
        out.not_evaluated = ne(
            "there are no dose-rate contributions, or the time is before the time they are given at",
            "dose_rate_contributions",
            "Give per-nuclide dose rates at the reference time and a time at or after it.",
        );
        return out;
    }
    let dt = time_years - contribution_time_years;
    let mut rate = 0.0;
    let mut sigma = 0.0;
    for c in contributions {
        if !(c.half_life_s.is_finite() && c.half_life_s > 0.0)
            || !(c.dose_rate_sv_per_h.is_finite() && c.dose_rate_sv_per_h >= 0.0)
            || !(c.sigma_sv_per_h.is_finite() && c.sigma_sv_per_h >= 0.0)
        {
            out.not_evaluated = ne(
                &format!(
                    "the dose-rate entry for {} has a bad half-life, dose rate or sigma",
                    c.nuclide
                ),
                &c.nuclide,
                "Give a positive half-life and non-negative dose rate and sigma.",
            );
            return out;
        }
        let factor = math::powf(2.0, -dt * super::YEAR_S / c.half_life_s);
        rate += c.dose_rate_sv_per_h * factor;
        sigma += c.sigma_sv_per_h * factor;
    }
    let limit = criterion.dose_rate_limit_sv_per_h;
    out.dose_rate_sv_per_h = Some(rate);
    out.index = Some(rate / limit);
    out.index_sigma = Some(sigma / limit);
    out.verdict = if rate / limit <= 1.0 {
        "WITHIN"
    } else {
        "EXCEEDED"
    }
    .to_string();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::waste::{NuclideActivity, WasteForm, YEAR_S};

    fn inventory(entries: &[(&str, f64, f64)], mass_g: f64) -> WasteInventory {
        WasteInventory {
            component_id: "c".to_string(),
            waste_form: WasteForm::ActivatedMetal,
            volume_m3: Some(1.0),
            mass_g: Some(mass_g),
            inventory_time_years: 0.0,
            transuranics_evaluated: true,
            inventory_kind: "authored_test".to_string(),
            nuclides: entries
                .iter()
                .map(|(n, bq, hl_y)| NuclideActivity {
                    nuclide: (*n).to_string(),
                    activity_bq: *bq,
                    half_life_s: hl_y * YEAR_S,
                    alpha_emitter: None,
                    progeny_of: None,
                })
                .collect(),
        }
    }

    // Verifies: ACT-036
    #[test]
    fn the_index_is_the_sum_of_concentration_over_level() {
        // 1000 g with 50 Bq of Co-60 (0.1 Bq/g level) and 20000 Bq of Ni-63 (100 Bq/g level).
        let i = inventory(
            &[("Co-60", 50.0, 5.2713), ("Ni-63", 20_000.0, 100.1)],
            1000.0,
        );
        let r = clearance_index(&i, 0.0);
        let expected = (50.0 / 1000.0) / 0.1 + (20_000.0 / 1000.0) / 100.0;
        assert!((r.index.unwrap() - expected).abs() < 1e-12);
        assert_eq!(r.lines.len(), 2);
        assert_eq!(r.below_one, Some(true));
        assert_eq!(r.scheme.id, "iaea-gsr3-table-i2");
        assert!(!r.scheme.version.is_empty() && !r.scheme.date.is_empty());
    }

    // Verifies: ACT-036
    #[test]
    fn a_single_nuclide_falls_below_one_after_log2_of_its_excess_half_lives() {
        // Co-60 at 8 times its level: index 8 falls below 1 after just over 3 half-lives.
        let hl = 5.2713;
        let i = inventory(&[("Co-60", 0.8 * 1000.0, hl)], 1000.0);
        let r = clearance_time(&i, SEARCH_START_YEARS, SEARCH_END_YEARS);
        let expected = 3.0 * hl;
        match r.time.unwrap() {
            ClearanceTime::FallsBelowOneAt { years } => {
                assert!((years - expected).abs() < 1e-9, "{years} vs {expected}");
            }
            other => panic!("{other:?}"),
        }
        assert!(r.index_at_window_start.unwrap() > 1.0);
    }

    // Verifies: ACT-036
    #[test]
    fn a_window_that_ends_too_early_or_starts_already_clear_is_reported() {
        let hot = inventory(&[("Co-60", 1.0e9, 5.2713)], 1000.0);
        let r = clearance_time(&hot, 1.0, 100.0);
        assert!(matches!(
            r.time,
            Some(ClearanceTime::NotWithinWindow { .. })
        ));
        let cool = inventory(&[("Co-60", 1.0, 5.2713)], 1000.0);
        let r = clearance_time(&cool, 1.0, 100.0);
        assert!(matches!(
            r.time,
            Some(ClearanceTime::BelowOneAtWindowStart { .. })
        ));
    }

    // Verifies: ACT-036
    #[test]
    fn a_nuclide_without_a_level_refuses_the_index() {
        let i = inventory(&[("Ba-137m", 5.0, 4.85e-6)], 1000.0);
        let r = clearance_index(&i, 0.0);
        assert!(r.index.is_none());
        assert_eq!(r.not_evaluated.unwrap().missing, vec!["Ba-137m"]);
        // Declared as progeny of Cs-137 (which Table I.2 lists with progeny) it is covered.
        let mut i = inventory(
            &[("Cs-137", 50.0, 30.08), ("Ba-137m", 47.0, 4.85e-6)],
            1000.0,
        );
        i.nuclides[1].progeny_of = Some("Cs-137".to_string());
        let r = clearance_index(&i, 0.0);
        assert!((r.index.unwrap() - (50.0 / 1000.0) / 0.1).abs() < 1e-12);
        assert_eq!(r.progeny_not_counted, vec!["Ba-137m"]);
        // A parent that Table I.2 does not list with progeny does not cover it.
        let mut i = inventory(&[("Co-60", 50.0, 5.27), ("Ba-137m", 47.0, 4.85e-6)], 1000.0);
        i.nuclides[1].progeny_of = Some("Co-60".to_string());
        assert!(clearance_index(&i, 0.0).not_evaluated.is_some());
    }

    // Verifies: ACT-036, ACT-040
    #[test]
    fn the_index_is_taken_at_the_stated_time() {
        let i = inventory(&[("Co-60", 400.0, 5.2713)], 1000.0);
        let now = clearance_index(&i, 0.0).index.unwrap();
        let later = clearance_index(&i, 5.2713).index.unwrap();
        assert!((later - now / 2.0).abs() < 1e-12);
        assert!(clearance_index(&i, -1.0).not_evaluated.is_some());
    }

    fn criterion() -> RecyclingCriterion {
        RecyclingCriterion {
            name: "test handling criterion".to_string(),
            dose_rate_limit_sv_per_h: 1.0e-4,
            source: "authored for this test".to_string(),
        }
    }

    // Verifies: ACT-037
    #[test]
    fn the_recycling_index_is_dose_rate_over_the_authored_criterion_with_sigma() {
        let contributions = [
            DoseRateContribution {
                nuclide: "Co-60".to_string(),
                half_life_s: 5.2713 * YEAR_S,
                dose_rate_sv_per_h: 2.0e-4,
                sigma_sv_per_h: 2.0e-5,
            },
            DoseRateContribution {
                nuclide: "Nb-94".to_string(),
                half_life_s: 20_300.0 * YEAR_S,
                dose_rate_sv_per_h: 1.0e-5,
                sigma_sv_per_h: 1.0e-6,
            },
        ];
        let r = recycling_index(&criterion(), &contributions, 0.0, 0.0);
        assert!((r.index.unwrap() - 2.1).abs() < 1e-12);
        assert!((r.index_sigma.unwrap() - 0.21).abs() < 1e-12);
        assert_eq!(r.verdict, "EXCEEDED");
        assert_eq!(r.criterion_kind, ResultKind::Authored);
        assert_eq!(r.criterion.source, "authored for this test");
        let later = recycling_index(&criterion(), &contributions, 0.0, 5.2713);
        assert!(later.index.unwrap() < 1.1);
        let json = serde_json::to_value(&later).unwrap();
        assert_eq!(json["criterion_kind"], "authored");
    }

    // Verifies: ACT-037
    #[test]
    fn no_index_is_given_without_a_criterion_value_and_source() {
        let mut c = criterion();
        c.source = " ".to_string();
        let contributions = [DoseRateContribution {
            nuclide: "Co-60".to_string(),
            half_life_s: 5.0 * YEAR_S,
            dose_rate_sv_per_h: 1.0,
            sigma_sv_per_h: 0.1,
        }];
        let r = recycling_index(&c, &contributions, 0.0, 0.0);
        assert!(r.index.is_none());
        assert_eq!(r.verdict, "NOT_EVALUATED");
        let mut c = criterion();
        c.dose_rate_limit_sv_per_h = 0.0;
        assert!(
            recycling_index(&c, &contributions, 0.0, 0.0)
                .index
                .is_none()
        );
    }
}
