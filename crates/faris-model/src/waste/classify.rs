//! 10 CFR 61.55 classification of one activated component (ACT-030 to ACT-035, ACT-039, ACT-040).
//!
//! What is implemented, by paragraph:
//! - (a)(3) and (a)(4): Table 1 against 0.1 and 1 times its value; Table 2 columns 1, 2 and 3.
//! - (a)(5): both tables together. Table 1 at or under 0.1 times leaves the class to Table 2;
//!   over 0.1 and not over 1 makes Class C when Table 2 column 3 holds; over 1 is above Class C.
//! - (a)(6): a waste with no nuclide in either table is Class A, but only when the inventory
//!   is complete (see below).
//! - (a)(7): the sum of fractions, within one column of one table, each nuclide shown.
//! - (a)(8): the concentration is the activity over one stated volume (curie per cubic metre) or
//!   one stated mass (nanocurie per gram). FARIS does no hot-spot or package-level averaging test.
//!
//! Not implemented: the NRC concentration-averaging guidance for mixed packages, intruder
//! assessments, site waste-acceptance criteria and state rules.
//!
//! Fail closed: a class is refused when a nuclide a table needs is absent from the inventory,
//! when a transuranic nuclide lacks its alpha flag, when averaging volume or mass is missing,
//! and when an activated-metal-only nuclide (Nb-94, Ni-59) appears in another waste form.

use serde::Serialize;

use super::{
    BOUNDARY_TOLERANCE, CURIE_BQ, NotEvaluated, ResultKind, SchemeInfo, WasteForm, WasteInventory,
    decayed_activity_bq, element_symbol, is_transuranic_element, tables, validate_inventory,
};
use crate::RESEARCH_SCREENING_STATEMENT;

/// Half-life under which a nuclide counts in the "total of all nuclides with less than 5 year
/// half-life" row of Table 2, in seconds.
const SHORT_LIVED_S: f64 = 5.0 * super::YEAR_S;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verdict {
    #[serde(rename = "CLASS_A")]
    ClassA,
    #[serde(rename = "CLASS_B")]
    ClassB,
    #[serde(rename = "CLASS_C")]
    ClassC,
    #[serde(rename = "GREATER_THAN_CLASS_C")]
    GreaterThanClassC,
    #[serde(rename = "NOT_EVALUATED")]
    NotEvaluated,
}

impl Verdict {
    pub fn text(self) -> &'static str {
        match self {
            Verdict::ClassA => "CLASS_A",
            Verdict::ClassB => "CLASS_B",
            Verdict::ClassC => "CLASS_C",
            Verdict::GreaterThanClassC => "GREATER_THAN_CLASS_C",
            Verdict::NotEvaluated => "NOT_EVALUATED",
        }
    }
}

/// One nuclide's share of one table column (ACT-035).
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct FractionLine {
    /// `table1`, `table2_col1`, `table2_col2` or `table2_col3`.
    pub column: String,
    pub row_label: String,
    pub nuclide: String,
    pub concentration: f64,
    pub concentration_unit: String,
    pub limit: f64,
    pub fraction: f64,
}

/// A nuclide with activity that neither table lists. Under (a)(6) it does not change the class.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct UnlistedLine {
    pub nuclide: String,
    pub activity_bq: f64,
    pub note: String,
}

/// Sums of fractions per column.
#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq)]
pub struct ColumnSums {
    pub table1: f64,
    pub table2_col1: f64,
    pub table2_col2: f64,
    pub table2_col3: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Averaging {
    pub volume_m3: Option<f64>,
    pub mass_g: Option<f64>,
    pub note: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct WasteResult {
    pub scheme: SchemeInfo,
    pub verdict: Verdict,
    pub component_id: String,
    pub waste_form: WasteForm,
    pub waste_form_text: String,
    pub disposal_time_years: f64,
    pub inventory_time_years: f64,
    /// How the inventory was brought to the disposal time.
    pub decay_treatment: String,
    pub averaging: Averaging,
    pub fractions: Vec<FractionLine>,
    pub sums: ColumnSums,
    /// Nuclides carried with their parent and left out of every sum.
    pub progeny_not_counted: Vec<String>,
    pub unlisted: Vec<UnlistedLine>,
    /// The rule path that gave the class, in words.
    pub rule: String,
    pub not_evaluated: Option<NotEvaluated>,
    pub kind: ResultKind,
    pub inventory_kind: String,
    pub research_screening: String,
}

fn exceeds(sum: f64, bound: f64) -> bool {
    sum > bound * (1.0 + BOUNDARY_TOLERANCE)
}

/// The class the sums give under (a)(3) to (a)(5), and the rule text.
pub fn class_from_sums(sums: &ColumnSums) -> (Verdict, String) {
    let t2 = |s: &ColumnSums| {
        if !exceeds(s.table2_col1, 1.0) {
            (
                Verdict::ClassA,
                format!(
                    "Table 2 column 1 sum {:.6} does not exceed 1 ((a)(4)(i))",
                    s.table2_col1
                ),
            )
        } else if !exceeds(s.table2_col2, 1.0) {
            (
                Verdict::ClassB,
                format!(
                    "Table 2 column 1 sum {:.6} exceeds 1 and column 2 sum {:.6} does not ((a)(4)(ii))",
                    s.table2_col1, s.table2_col2
                ),
            )
        } else if !exceeds(s.table2_col3, 1.0) {
            (
                Verdict::ClassC,
                format!(
                    "Table 2 column 2 sum {:.6} exceeds 1 and column 3 sum {:.6} does not ((a)(4)(iii))",
                    s.table2_col2, s.table2_col3
                ),
            )
        } else {
            (
                Verdict::GreaterThanClassC,
                format!(
                    "Table 2 column 3 sum {:.6} exceeds 1 ((a)(4)(iv))",
                    s.table2_col3
                ),
            )
        }
    };
    if exceeds(sums.table1, 1.0) {
        (
            Verdict::GreaterThanClassC,
            format!("Table 1 sum {:.6} exceeds 1 ((a)(3)(iii))", sums.table1),
        )
    } else if exceeds(sums.table1, 0.1) {
        if exceeds(sums.table2_col3, 1.0) {
            (
                Verdict::GreaterThanClassC,
                format!(
                    "Table 1 sum {:.6} exceeds 0.1 but Table 2 column 3 sum {:.6} exceeds 1 ((a)(5)(ii))",
                    sums.table1, sums.table2_col3
                ),
            )
        } else {
            (
                Verdict::ClassC,
                format!(
                    "Table 1 sum {:.6} exceeds 0.1 and not 1, and Table 2 column 3 sum {:.6} does not exceed 1 ((a)(3)(ii), (a)(5)(ii))",
                    sums.table1, sums.table2_col3
                ),
            )
        }
    } else {
        let (verdict, text) = t2(sums);
        (
            verdict,
            format!(
                "Table 1 sum {:.6} does not exceed 0.1, so {text} ((a)(5)(i))",
                sums.table1
            ),
        )
    }
}

struct Computed {
    fractions: Vec<FractionLine>,
    sums: ColumnSums,
    progeny: Vec<String>,
    unlisted: Vec<UnlistedLine>,
}

/// Concentration of `activity_bq` in the unit of a table row, over the stated volume or mass.
fn concentration(
    activity_bq: f64,
    unit: &str,
    inventory: &WasteInventory,
    nuclide: &str,
) -> Result<f64, NotEvaluated> {
    if unit == "nCi/g" {
        let mass = inventory.mass_g.ok_or_else(|| {
            NotEvaluated::new(
                format!(
                    "{nuclide} has a limit in nanocurie per gram and the inventory gives no mass"
                ),
                vec!["mass_g".to_string()],
                "Give mass_g, the mass the activity is averaged over.",
            )
        })?;
        Ok(activity_bq / CURIE_BQ * 1e9 / mass)
    } else {
        let volume = inventory.volume_m3.ok_or_else(|| {
            NotEvaluated::new(
                format!("{nuclide} has a limit in curie per cubic metre and the inventory gives no volume"),
                vec!["volume_m3".to_string()],
                "Give volume_m3, the volume the activity is averaged over (for example the displaced volume).",
            )
        })?;
        Ok(activity_bq / CURIE_BQ / volume)
    }
}

/// The nuclides every classification needs in the inventory, as entries (zero activity counts).
pub fn required_nuclides(form: WasteForm) -> Vec<String> {
    let t = tables::nrc();
    let mut names: Vec<String> = Vec::new();
    for row in t.table1_for(form) {
        if let Some(n) = &row.nuclide {
            names.push(n.clone());
        }
    }
    for row in t.table2_for(form) {
        if let Some(n) = &row.nuclide {
            names.push(n.clone());
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Sums of fractions for an inventory at a disposal time. `check_required` is true for a
/// classification and false for the per-ppm yield of one impurity element, which is a delta
/// and not a full inventory.
fn compute(
    inventory: &WasteInventory,
    disposal_time_years: f64,
    check_required: bool,
) -> Result<Computed, NotEvaluated> {
    validate_inventory(inventory)?;
    if !disposal_time_years.is_finite() || disposal_time_years < inventory.inventory_time_years {
        return Err(NotEvaluated::new(
            format!(
                "the disposal time ({disposal_time_years} y) is earlier than the inventory time ({} y); FARIS decays forward only",
                inventory.inventory_time_years
            ),
            vec!["disposal_time_years".to_string()],
            "Give a disposal time at or after the inventory time, or an inventory taken earlier.",
        ));
    }
    let nrc = tables::nrc();
    let form = inventory.waste_form;
    let dt = disposal_time_years - inventory.inventory_time_years;

    let mut missing: Vec<String> = Vec::new();
    if check_required {
        for name in required_nuclides(form) {
            if !inventory.nuclides.iter().any(|n| n.nuclide == name) {
                missing.push(name);
            }
        }
        if !inventory.transuranics_evaluated
            && !inventory
                .nuclides
                .iter()
                .any(|n| n.alpha_emitter == Some(true))
        {
            missing.push("alpha-emitting transuranic nuclides (set transuranics_evaluated once the calculation covers them)".to_string());
        }
    }
    let mut alpha_flag_missing: Vec<String> = Vec::new();
    let mut metal_only_present: Vec<String> = Vec::new();

    let table1 = nrc.table1_for(form);
    let table2 = nrc.table2_for(form);
    let metal_only = nrc.activated_metal_only_nuclides();

    let mut fractions: Vec<FractionLine> = Vec::new();
    let mut sums = ColumnSums::default();
    let mut progeny: Vec<String> = Vec::new();
    let mut unlisted: Vec<UnlistedLine> = Vec::new();
    let mut error: Option<NotEvaluated> = None;

    for n in &inventory.nuclides {
        let activity = decayed_activity_bq(n.activity_bq, n.half_life_s, dt);
        if n.progeny_of.is_some() {
            progeny.push(n.nuclide.clone());
            continue;
        }
        if activity <= 0.0 {
            continue;
        }
        let mut listed = false;
        let mut push = |column: &str, label: &str, unit: &str, limit: f64, sums_slot: &mut f64| {
            match concentration(activity, unit, inventory, &n.nuclide) {
                Ok(c) => {
                    let fraction = c / limit;
                    *sums_slot += fraction;
                    fractions.push(FractionLine {
                        column: column.to_string(),
                        row_label: label.to_string(),
                        nuclide: n.nuclide.clone(),
                        concentration: c,
                        concentration_unit: unit.to_string(),
                        limit,
                        fraction,
                    });
                }
                Err(e) => error = error.take().or(Some(e)),
            }
        };
        // Table 1, by nuclide.
        for row in table1
            .iter()
            .filter(|r| r.nuclide.as_deref() == Some(&n.nuclide))
        {
            listed = true;
            push("table1", &row.label, &row.unit, row.limit, &mut sums.table1);
        }
        // Table 1, the alpha-emitting transuranic group (Pu-241 and Cm-242 have their own rows).
        let transuranic_long_lived = is_transuranic_element(element_symbol(&n.nuclide))
            && n.half_life_s > SHORT_LIVED_S
            && n.nuclide != "Pu-241";
        if transuranic_long_lived {
            match n.alpha_emitter {
                Some(true) => {
                    if let Some(row) = table1
                        .iter()
                        .find(|r| r.group.as_deref() == Some("alpha_transuranic_half_life_over_5y"))
                    {
                        listed = true;
                        push("table1", &row.label, &row.unit, row.limit, &mut sums.table1);
                    }
                }
                Some(false) => {}
                None => alpha_flag_missing.push(n.nuclide.clone()),
            }
        }
        // Activated-metal-only nuclides in another form have no limit anywhere.
        if form == WasteForm::Other && metal_only.contains(&n.nuclide.as_str()) {
            metal_only_present.push(n.nuclide.clone());
        }
        // Table 2, by nuclide.
        for row in table2
            .iter()
            .filter(|r| r.nuclide.as_deref() == Some(&n.nuclide))
        {
            listed = true;
            for column in 1..=3u8 {
                if let Some(limit) = row.column(column) {
                    let (name, slot) = match column {
                        1 => ("table2_col1", &mut sums.table2_col1),
                        2 => ("table2_col2", &mut sums.table2_col2),
                        _ => ("table2_col3", &mut sums.table2_col3),
                    };
                    push(name, &row.label, &row.unit, limit, slot);
                }
            }
        }
        // Table 2, every nuclide with a half-life under 5 years (column 1 only).
        if n.half_life_s < SHORT_LIVED_S
            && let Some(row) = table2
                .iter()
                .find(|r| r.group.as_deref() == Some("total_half_life_under_5y"))
            && let Some(limit) = row.col1
        {
            listed = true;
            push(
                "table2_col1",
                &row.label,
                &row.unit,
                limit,
                &mut sums.table2_col1,
            );
        }
        if !listed {
            unlisted.push(UnlistedLine {
                nuclide: n.nuclide.clone(),
                activity_bq: activity,
                note: "not in Table 1 or Table 2; under 61.55(a)(6) it does not change the class"
                    .to_string(),
            });
        }
    }
    if let Some(e) = error {
        return Err(e);
    }
    if !metal_only_present.is_empty() {
        return Err(NotEvaluated::new(
            format!(
                "{} appear(s) in a waste form other than activated metal; Table 1 gives limits for them in activated metal only, so 61.55 states no limit for this form",
                metal_only_present.join(", ")
            ),
            metal_only_present,
            "Classify the component as activated metal if it is, or ask the regulator which limit applies to this form.",
        ));
    }
    if !alpha_flag_missing.is_empty() {
        return Err(NotEvaluated::new(
            "a transuranic nuclide with a half-life over 5 years has no alpha_emitter flag; the Table 1 row is for alpha emitters",
            alpha_flag_missing,
            "Set alpha_emitter to true or false for each from the decay data.",
        ));
    }
    if !missing.is_empty() {
        return Err(NotEvaluated::new(
            "the inventory does not list every nuclide the tables need, so no class can be given (a missing nuclide is not taken as absent)",
            missing,
            "Add each missing nuclide with its activity; give 0 where the activation calculation covered it and none was produced. A class is never defaulted to Class A.",
        ));
    }
    Ok(Computed {
        fractions,
        sums,
        progeny,
        unlisted,
    })
}

/// Sums of fractions only, for the impurity budget. No completeness check on the inventory.
pub(crate) fn sums_unchecked(
    inventory: &WasteInventory,
    disposal_time_years: f64,
) -> Result<ColumnSums, NotEvaluated> {
    compute(inventory, disposal_time_years, false).map(|c| c.sums)
}

/// Classifies one component under 10 CFR 61.55 at a stated disposal time.
pub fn classify_nrc(inventory: &WasteInventory, disposal_time_years: f64) -> WasteResult {
    let scheme = tables::nrc().scheme_info();
    let averaging = Averaging {
        volume_m3: inventory.volume_m3,
        mass_g: inventory.mass_g,
        note: "61.55(a)(8): concentration is the activity over this one volume (Ci/m3 limits) or mass (nCi/g limits); no hot-spot or package-level averaging test is applied".to_string(),
    };
    let base = |verdict, fractions, sums, progeny, unlisted, rule, not_evaluated| {
        WasteResult {
        scheme: scheme.clone(),
        verdict,
        component_id: inventory.component_id.clone(),
        waste_form: inventory.waste_form,
        waste_form_text: inventory.waste_form.text().to_string(),
        disposal_time_years,
        inventory_time_years: inventory.inventory_time_years,
        decay_treatment: "each nuclide decayed from the inventory time to the disposal time by its own half-life; no ingrowth from parents (supply the inventory at the disposal time where ingrowth matters)".to_string(),
        averaging: averaging.clone(),
        fractions,
        sums,
        progeny_not_counted: progeny,
        unlisted,
        rule,
        not_evaluated,
        kind: ResultKind::Calculated,
        inventory_kind: inventory.inventory_kind.clone(),
        research_screening: RESEARCH_SCREENING_STATEMENT.to_string(),
    }
    };
    match compute(inventory, disposal_time_years, true) {
        Ok(c) => {
            let (verdict, rule) = class_from_sums(&c.sums);
            let rule = if c.fractions.is_empty() {
                "no nuclide with activity is in Table 1 or Table 2: Class A under 61.55(a)(6), because the inventory lists every nuclide the tables need".to_string()
            } else {
                rule
            };
            base(
                verdict,
                c.fractions,
                c.sums,
                c.progeny,
                c.unlisted,
                rule,
                None,
            )
        }
        Err(e) => base(
            Verdict::NotEvaluated,
            Vec::new(),
            ColumnSums::default(),
            Vec::new(),
            Vec::new(),
            "NOT_EVALUATED: the reason and the next step are in the result".to_string(),
            Some(e),
        ),
    }
}

/// Years from the inventory time at which the class first drops to at most `target`, searched on a
/// grid of `step_years` up to `max_years`; `None` if it never does in that span. Decay only.
pub fn first_time_at_most(
    inventory: &WasteInventory,
    target: Verdict,
    step_years: f64,
    max_years: f64,
) -> Option<f64> {
    let mut t = inventory.inventory_time_years;
    let end = inventory.inventory_time_years + max_years;
    while t <= end + 1e-12 {
        let r = classify_nrc(inventory, t);
        if r.verdict == Verdict::NotEvaluated {
            return None;
        }
        if r.verdict <= target {
            return Some(t);
        }
        t += step_years;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::waste::{NuclideActivity, YEAR_S};

    /// An inventory with every required nuclide at zero, then the given concentrations in Ci/m3
    /// over 1 m3 (1 g of mass per 1e6 m3 is not needed: mass is 1e6 g for nCi/g cases).
    fn inv(form: WasteForm, entries: &[(&str, f64, f64)]) -> WasteInventory {
        let mut nuclides: Vec<NuclideActivity> = required_nuclides(form)
            .into_iter()
            .map(|n| NuclideActivity {
                half_life_s: half_life(&n) * YEAR_S,
                nuclide: n,
                activity_bq: 0.0,
                alpha_emitter: None,
                progeny_of: None,
            })
            .collect();
        for (name, ci_per_m3, half_life_y) in entries {
            let activity = ci_per_m3 * CURIE_BQ;
            if let Some(n) = nuclides.iter_mut().find(|n| n.nuclide == *name) {
                n.activity_bq = activity;
            } else {
                nuclides.push(NuclideActivity {
                    nuclide: (*name).to_string(),
                    activity_bq: activity,
                    half_life_s: half_life_y * YEAR_S,
                    alpha_emitter: None,
                    progeny_of: None,
                });
            }
        }
        WasteInventory {
            component_id: "test".to_string(),
            waste_form: form,
            volume_m3: Some(1.0),
            mass_g: Some(1.0e6),
            inventory_time_years: 0.0,
            transuranics_evaluated: true,
            inventory_kind: "authored_test".to_string(),
            nuclides,
        }
    }

    /// Half-lives in years for the table nuclides (test data, from the nuclide charts).
    fn half_life(name: &str) -> f64 {
        match name {
            "H-3" => 12.32,
            "C-14" => 5700.0,
            "Co-60" => 5.2713,
            "Ni-59" => 76_000.0,
            "Ni-63" => 100.1,
            "Nb-94" => 20_300.0,
            "Tc-99" => 211_100.0,
            "I-129" => 1.57e7,
            "Sr-90" => 28.8,
            "Cs-137" => 30.08,
            "Pu-241" => 14.33,
            "Cm-242" => 0.4457,
            _ => 1.0,
        }
    }

    fn class(form: WasteForm, entries: &[(&str, f64, f64)]) -> Verdict {
        classify_nrc(&inv(form, entries), 0.0).verdict
    }

    // Verifies: ACT-030, ACT-035
    #[test]
    fn the_regulations_own_worked_example_gives_class_b_with_sum_0_83() {
        let r = classify_nrc(
            &inv(
                WasteForm::Other,
                &[("Sr-90", 50.0, 28.8), ("Cs-137", 22.0, 30.08)],
            ),
            0.0,
        );
        assert_eq!(r.verdict, Verdict::ClassB);
        assert!((r.sums.table2_col2 - (50.0 / 150.0 + 22.0 / 44.0)).abs() < 1e-12);
        assert!((r.sums.table2_col2 - 0.8333333333).abs() < 1e-9);
        let col2: Vec<_> = r
            .fractions
            .iter()
            .filter(|f| f.column == "table2_col2")
            .collect();
        assert_eq!(col2.len(), 2);
        let total: f64 = col2.iter().map(|f| f.fraction).sum();
        assert!((total - r.sums.table2_col2).abs() < 1e-15);
        assert!(r.research_screening.contains("Research screening only"));
        assert_eq!(r.scheme.id, "nrc-10cfr61.55");
    }

    // Verifies: ACT-030
    #[test]
    fn twenty_five_hand_computed_cases_give_the_right_class() {
        use Verdict::*;
        let o = WasteForm::Other;
        let m = WasteForm::ActivatedMetal;
        // (form, entries, expected). Concentrations are Ci/m3 over 1 m3; half-lives in years.
        type Case<'a> = (WasteForm, Vec<(&'a str, f64, f64)>, Verdict);
        let cases: Vec<Case> = vec![
            // empty inventory: every required nuclide present at 0, so Class A under (a)(6)
            (o, vec![], ClassA),
            // Table 2 column 1 single nuclides
            (o, vec![("Cs-137", 1.0, 30.08)], ClassA),
            (o, vec![("Cs-137", 1.0001, 30.08)], ClassB),
            (o, vec![("Cs-137", 44.0, 30.08)], ClassB),
            (o, vec![("Cs-137", 44.5, 30.08)], ClassC),
            (o, vec![("Cs-137", 4600.0, 30.08)], ClassC),
            (o, vec![("Cs-137", 4601.0, 30.08)], GreaterThanClassC),
            (o, vec![("Sr-90", 0.04, 28.8)], ClassA),
            (o, vec![("Sr-90", 0.05, 28.8)], ClassB),
            (o, vec![("Sr-90", 7001.0, 28.8)], GreaterThanClassC),
            // H-3 and Co-60 have a column 1 limit only: above it the waste is Class B
            (o, vec![("H-3", 40.0, 12.32)], ClassA),
            (o, vec![("H-3", 41.0, 12.32)], ClassB),
            (o, vec![("H-3", 1.0e6, 12.32)], ClassB),
            (o, vec![("Co-60", 701.0, 5.2713)], ClassB),
            // H-3 above its limit plus enough Ni-63 for column 3 makes Class C
            (
                o,
                vec![("H-3", 100.0, 12.32), ("Ni-63", 701.0, 100.1)],
                GreaterThanClassC,
            ),
            (
                o,
                vec![("H-3", 100.0, 12.32), ("Ni-63", 400.0, 100.1)],
                ClassC,
            ),
            // Ni-63 plain versus activated metal: the same 10 Ci/m3
            (o, vec![("Ni-63", 10.0, 100.1)], ClassB),
            (m, vec![("Ni-63", 10.0, 100.1)], ClassA),
            // aggregate of nuclides with a half-life under 5 years
            (o, vec![("Mn-54", 700.0, 0.855)], ClassA),
            (o, vec![("Mn-54", 701.0, 0.855)], ClassB),
            // Table 1 with Table 2 empty: 0.1 and 1 times the limit
            (o, vec![("Tc-99", 0.3, 211_100.0)], ClassA),
            (o, vec![("Tc-99", 0.31, 211_100.0)], ClassC),
            (o, vec![("Tc-99", 3.01, 211_100.0)], GreaterThanClassC),
            // C-14: 8 plain, 80 in activated metal
            (o, vec![("C-14", 5.0, 5700.0)], ClassC),
            (o, vec![("C-14", 9.0, 5700.0)], GreaterThanClassC),
            (m, vec![("C-14", 9.0, 5700.0)], ClassC),
            (m, vec![("C-14", 7.0, 5700.0)], ClassA),
            // Nb-94 in activated metal, with a column 1 exceedance from Cs-137: Class C by (a)(5)(ii)
            (
                m,
                vec![("Nb-94", 0.1, 20_300.0), ("Cs-137", 5.0, 30.08)],
                ClassC,
            ),
            (m, vec![("Nb-94", 0.2001, 20_300.0)], GreaterThanClassC),
            // Table 1 above 0.1 and Table 2 column 3 exceeded: above Class C
            (
                m,
                vec![("Nb-94", 0.1, 20_300.0), ("Cs-137", 5000.0, 30.08)],
                GreaterThanClassC,
            ),
            // Ni-59 in activated metal
            (m, vec![("Ni-59", 22.0, 76_000.0)], ClassA),
            (m, vec![("Ni-59", 23.0, 76_000.0)], ClassC),
        ];
        assert!(cases.len() >= 20);
        for (i, (form, entries, expected)) in cases.iter().enumerate() {
            assert_eq!(class(*form, entries), *expected, "case {i}: {entries:?}");
        }
    }

    // Verifies: ACT-035
    #[test]
    fn the_class_changes_at_a_sum_of_exactly_one() {
        // Two nuclides each at half their column 1 limit: sum exactly 1 stays Class A.
        let at = classify_nrc(
            &inv(
                WasteForm::Other,
                &[("Sr-90", 0.02, 28.8), ("Cs-137", 0.5, 30.08)],
            ),
            0.0,
        );
        assert_eq!(at.verdict, Verdict::ClassA);
        assert!((at.sums.table2_col1 - 1.0).abs() < 1e-12);
        let over = classify_nrc(
            &inv(
                WasteForm::Other,
                &[("Sr-90", 0.02, 28.8), ("Cs-137", 0.5 * (1.0 + 1e-8), 30.08)],
            ),
            0.0,
        );
        assert_eq!(over.verdict, Verdict::ClassB);
        let under = classify_nrc(
            &inv(
                WasteForm::Other,
                &[("Sr-90", 0.02, 28.8), ("Cs-137", 0.5 * (1.0 - 1e-6), 30.08)],
            ),
            0.0,
        );
        assert_eq!(under.verdict, Verdict::ClassA);
    }

    // Verifies: ACT-034
    #[test]
    fn the_same_inventory_in_both_forms_uses_different_limits_and_shows_the_form() {
        let entries = [("Ni-63", 10.0, 100.1), ("C-14", 5.0, 5700.0)];
        let metal = classify_nrc(&inv(WasteForm::ActivatedMetal, &entries), 0.0);
        let other = classify_nrc(&inv(WasteForm::Other, &entries), 0.0);
        assert_eq!(metal.verdict, Verdict::ClassA);
        assert_eq!(other.verdict, Verdict::ClassC);
        assert_eq!(metal.waste_form_text, "activated metal");
        assert_eq!(other.waste_form_text, "other waste form");
        assert!(
            metal
                .fractions
                .iter()
                .any(|f| f.row_label == "Ni-63 in activated metal")
        );
        assert!(other.fractions.iter().any(|f| f.row_label == "Ni-63"));
        assert!(
            !other
                .fractions
                .iter()
                .any(|f| f.row_label.contains("activated metal"))
        );
    }

    // Verifies: ACT-034, ACT-039
    #[test]
    fn nb94_outside_activated_metal_is_refused_not_passed_as_class_a() {
        let r = classify_nrc(&inv(WasteForm::Other, &[("Nb-94", 0.1, 20_300.0)]), 0.0);
        assert_eq!(r.verdict, Verdict::NotEvaluated);
        let ne = r.not_evaluated.unwrap();
        assert_eq!(ne.missing, vec!["Nb-94".to_string()]);
        assert!(ne.reason.contains("activated metal"));
        assert!(!ne.next_step.is_empty());
    }

    // Verifies: ACT-039
    #[test]
    fn a_truncated_inventory_is_refused_with_the_missing_nuclides() {
        let mut i = inv(WasteForm::ActivatedMetal, &[("Nb-94", 0.01, 20_300.0)]);
        i.nuclides
            .retain(|n| n.nuclide != "C-14" && n.nuclide != "Tc-99" && n.nuclide != "Ni-59");
        let r = classify_nrc(&i, 0.0);
        assert_eq!(r.verdict, Verdict::NotEvaluated);
        let ne = r.not_evaluated.unwrap();
        assert_eq!(ne.missing, vec!["C-14", "Ni-59", "Tc-99"]);
        assert!(ne.next_step.contains("never defaulted to Class A"));
        assert!(r.fractions.is_empty());
        // The nuclides it did have are not enough for Class A either.
        assert_ne!(r.verdict, Verdict::ClassA);
    }

    // Verifies: ACT-039
    #[test]
    fn missing_volume_mass_alpha_flag_and_transuranic_coverage_are_refused() {
        let mut i = inv(WasteForm::Other, &[("Cs-137", 1.0, 30.08)]);
        i.volume_m3 = None;
        let r = classify_nrc(&i, 0.0);
        assert_eq!(r.not_evaluated.unwrap().missing, vec!["volume_m3"]);

        let mut i = inv(WasteForm::Other, &[("Pu-241", 1.0e-9, 14.33)]);
        i.mass_g = None;
        assert_eq!(
            classify_nrc(&i, 0.0).not_evaluated.unwrap().missing,
            vec!["mass_g"]
        );

        let i = inv(WasteForm::Other, &[("Am-241", 1.0e-9, 432.6)]);
        let r = classify_nrc(&i, 0.0);
        assert_eq!(r.not_evaluated.unwrap().missing, vec!["Am-241"]);

        let mut i = inv(WasteForm::Other, &[]);
        i.transuranics_evaluated = false;
        let r = classify_nrc(&i, 0.0);
        assert_eq!(r.verdict, Verdict::NotEvaluated);
        assert!(r.not_evaluated.unwrap().missing[0].contains("transuranic"));
    }

    // Verifies: ACT-030
    #[test]
    fn alpha_transuranics_use_nanocurie_per_gram_over_the_stated_mass() {
        let mut i = inv(WasteForm::Other, &[("Am-241", 0.0, 432.6)]);
        // 100 nCi/g over 1e6 g is 0.1 Ci: exactly the Table 1 limit.
        let am = i
            .nuclides
            .iter_mut()
            .find(|n| n.nuclide == "Am-241")
            .unwrap();
        am.activity_bq = 0.1 * CURIE_BQ;
        am.alpha_emitter = Some(true);
        let r = classify_nrc(&i, 0.0);
        assert!((r.sums.table1 - 1.0).abs() < 1e-12, "{}", r.sums.table1);
        assert_eq!(r.verdict, Verdict::ClassC);
        assert_eq!(r.fractions[0].concentration_unit, "nCi/g");
        // An alpha flag of false leaves it unlisted: no limit applies.
        let am = i
            .nuclides
            .iter_mut()
            .find(|n| n.nuclide == "Am-241")
            .unwrap();
        am.alpha_emitter = Some(false);
        let r = classify_nrc(&i, 0.0);
        assert_eq!(r.verdict, Verdict::ClassA);
        assert_eq!(r.unlisted[0].nuclide, "Am-241");
    }

    // Verifies: ACT-040
    #[test]
    fn the_class_is_taken_at_the_stated_disposal_time_and_the_receipt_changes_with_it() {
        // Mn-54 (0.855 y) in the aggregate: 1000 Ci/m3 now is Class B; after 1 y it is below 700.
        let i = inv(WasteForm::Other, &[("Mn-54", 1000.0, 0.855)]);
        let now = classify_nrc(&i, 0.0);
        let later = classify_nrc(&i, 1.0);
        assert_eq!(now.verdict, Verdict::ClassB);
        assert_eq!(later.verdict, Verdict::ClassA);
        let expected = 1000.0 * crate::math::powf(2.0, -1.0 / 0.855) / 700.0;
        assert!((later.sums.table2_col1 - expected).abs() < 1e-9);
        assert_ne!(
            serde_json::to_string(&now).unwrap(),
            serde_json::to_string(&later).unwrap()
        );
        assert_eq!(later.disposal_time_years, 1.0);
        // An inventory given at 5 y cannot be disposed of at 2 y.
        let mut i = i;
        i.inventory_time_years = 5.0;
        let r = classify_nrc(&i, 2.0);
        assert_eq!(r.verdict, Verdict::NotEvaluated);
    }

    // Verifies: ACT-040
    #[test]
    fn the_first_time_at_most_a_class_is_found_on_a_grid() {
        let i = inv(WasteForm::Other, &[("Mn-54", 1000.0, 0.855)]);
        let t = first_time_at_most(&i, Verdict::ClassA, 0.5, 10.0).unwrap();
        assert_eq!(t, 0.5);
        let never = inv(WasteForm::Other, &[("Cs-137", 5000.0, 30.08)]);
        assert_eq!(first_time_at_most(&never, Verdict::ClassA, 1.0, 5.0), None);
    }

    // Verifies: ACT-035
    #[test]
    fn progeny_are_listed_but_not_counted() {
        let mut i = inv(WasteForm::Other, &[("Cs-137", 0.5, 30.08)]);
        i.nuclides.push(NuclideActivity {
            nuclide: "Ba-137m".to_string(),
            activity_bq: 0.5 * CURIE_BQ * 0.944,
            half_life_s: 153.12,
            alpha_emitter: None,
            progeny_of: Some("Cs-137".to_string()),
        });
        let r = classify_nrc(&i, 0.0);
        assert_eq!(r.progeny_not_counted, vec!["Ba-137m"]);
        assert!((r.sums.table2_col1 - 0.5).abs() < 1e-12);
        // Without the declaration the 2.55-minute nuclide enters the under-5-year total.
        i.nuclides.last_mut().unwrap().progeny_of = None;
        let r = classify_nrc(&i, 0.0);
        assert!(r.sums.table2_col1 > 0.5);
    }

    // Verifies: ACT-033
    #[test]
    fn every_result_carries_scheme_name_version_date_and_labels() {
        let r = classify_nrc(&inv(WasteForm::Other, &[]), 3.0);
        let json = serde_json::to_value(&r).unwrap();
        for key in ["name", "version", "date", "citation"] {
            assert!(
                json["scheme"][key].as_str().is_some_and(|s| !s.is_empty()),
                "{key}"
            );
        }
        assert_eq!(json["kind"], "calculated");
        assert_eq!(json["verdict"], "CLASS_A");
        assert!(
            json["research_screening"]
                .as_str()
                .unwrap()
                .contains("not a licensing")
        );
    }
}
