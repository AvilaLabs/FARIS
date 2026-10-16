//! Impurity watch-list, the impurity gate on a waste verdict, and the largest impurity a target
//! waste class allows (MAT-040 to MAT-044).
//!
//! The inverse calculation uses that the sums of fractions in 10 CFR 61.55 are linear in the
//! concentration of each nuclide, and that activation at a fixed flux is linear in the initial
//! composition (a per-element yield, in becquerel per weight ppm, comes from the activation
//! calculation). It does not model self-shielding or any change of the spectrum with composition.

use serde::{Deserialize, Serialize};

use super::{Cite, ClassThreshold, DbBinding, ElementLimit, MaterialsDb, NotEvaluated, SpecKind};
use crate::{
    RESEARCH_SCREENING_STATEMENT,
    waste::{
        NuclideActivity, ResultKind, WasteInventory,
        classify::{ColumnSums, Verdict, WasteResult, classify_nrc, sums_unchecked},
        tables,
    },
};

/// Class limits read from a 10 CFR 61.55 row, in the row's unit.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ThresholdInfo {
    pub scheme: String,
    pub table: String,
    pub row_label: String,
    pub unit: String,
    /// Largest single-nuclide concentration that keeps the waste in Class A, B and C. `None` where
    /// the row has no such limit.
    pub class_a_max: Option<f64>,
    pub class_b_max: Option<f64>,
    pub class_c_max: Option<f64>,
}

/// The class thresholds of one 10 CFR 61.55 row, `None` if the row does not exist.
pub fn nrc_threshold(table: &str, row_label: &str) -> Option<ThresholdInfo> {
    let t = tables::nrc();
    match table {
        "table1" => t
            .table1
            .iter()
            .find(|r| r.label == row_label)
            .map(|r| ThresholdInfo {
                scheme: "nrc-10cfr61.55".to_string(),
                table: "table1".to_string(),
                row_label: r.label.clone(),
                unit: r.unit.clone(),
                // (a)(3): up to 0.1 times the value is Class A, up to the value is Class C.
                class_a_max: Some(0.1 * r.limit),
                class_b_max: None,
                class_c_max: Some(r.limit),
            }),
        "table2" => t
            .table2
            .iter()
            .find(|r| r.label == row_label)
            .map(|r| ThresholdInfo {
                scheme: "nrc-10cfr61.55".to_string(),
                table: "table2".to_string(),
                row_label: r.label.clone(),
                unit: r.unit.clone(),
                class_a_max: r.col1,
                class_b_max: r.col2,
                class_c_max: r.col3,
            }),
        _ => None,
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "scheme")]
pub enum ResolvedThreshold {
    #[serde(rename = "nrc-10cfr61.55")]
    Nrc(ThresholdInfo),
    #[serde(rename = "fetter-1990-sal")]
    Fetter {
        nuclide: String,
        value: f64,
        unit: String,
        note: String,
        cite: Cite,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct WatchItem {
    pub element: String,
    pub nuclides: Vec<String>,
    pub reason: String,
    pub cite: Cite,
    pub class_threshold: ResolvedThreshold,
    pub literature_element_limit: Option<ElementLimit>,
}

/// The watch-list with every 10 CFR 61.55 threshold read from the encoded tables (MAT-041).
pub fn watchlist(db: &MaterialsDb) -> Vec<WatchItem> {
    db.data
        .watchlist
        .iter()
        .filter_map(|w| {
            let class_threshold = match &w.class_threshold {
                ClassThreshold::Nrc { table, row_label } => {
                    ResolvedThreshold::Nrc(nrc_threshold(table, row_label)?)
                }
                ClassThreshold::Fetter {
                    nuclide,
                    value,
                    unit,
                    note,
                    cite,
                } => ResolvedThreshold::Fetter {
                    nuclide: nuclide.clone(),
                    value: *value,
                    unit: unit.clone(),
                    note: note.clone(),
                    cite: cite.clone(),
                },
            };
            Some(WatchItem {
                element: w.element.clone(),
                nuclides: w.nuclides.clone(),
                reason: w.reason.clone(),
                cite: w.cite.clone(),
                class_threshold,
                literature_element_limit: w.literature_element_limit.clone(),
            })
        })
        .collect()
}

/// An impurity limit the user adds for an element the stored specification does not cover.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AssumedImpurity {
    pub element: String,
    pub max_wt_ppm: f64,
    pub author: String,
    pub reason: String,
}

/// Which stored specification a waste verdict rests on, plus the user's own additions.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpecSelection {
    pub spec_id: String,
    #[serde(default)]
    pub assumed: Vec<AssumedImpurity>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ImpurityLine {
    pub element: String,
    pub max_wt_ppm: f64,
    /// `specified`, `measured`, `nominal` or `assumed`.
    pub kind: String,
    pub source: String,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ImpurityBasis {
    pub spec_id: String,
    pub spec_kind: SpecKind,
    pub lines: Vec<ImpurityLine>,
}

/// The watch-list elements a waste verdict needs an impurity value for.
pub fn required_elements(db: &MaterialsDb) -> Vec<String> {
    db.data
        .watchlist
        .iter()
        .map(|w| w.element.clone())
        .collect()
}

/// Checks that a waste verdict has an impurity specification covering the watch-list (MAT-043).
pub fn impurity_basis(
    db: &MaterialsDb,
    material_id: &str,
    selection: Option<&SpecSelection>,
) -> Result<ImpurityBasis, NotEvaluated> {
    let needed = required_elements(db);
    let Some(selection) = selection else {
        return Err(NotEvaluated::new(
            "no impurity specification was given, so no waste class is stated; trace elements such as Nb, Mo, Ag and Co decide the class of activated steel",
            needed,
            "Choose a stored specification (faris materials show) and add an assumed limit, with author and reason, for each element it does not cover.",
        ));
    };
    let material = db.material(material_id).ok_or_else(|| {
        NotEvaluated::new(
            format!("material {material_id} is not in the database"),
            vec![material_id.to_string()],
            "List the materials with `faris materials list`.",
        )
    })?;
    let spec = material
        .impurity_specs
        .iter()
        .find(|s| s.id == selection.spec_id)
        .ok_or_else(|| {
            NotEvaluated::new(
                format!(
                    "{material_id} has no impurity specification {}",
                    selection.spec_id
                ),
                vec![selection.spec_id.clone()],
                "Use one of the specification ids shown for the material.",
            )
        })?;
    let mut lines: Vec<ImpurityLine> = spec
        .limits
        .iter()
        .map(|l| ImpurityLine {
            element: l.element.clone(),
            max_wt_ppm: l.max_wt_ppm,
            kind: spec.kind.text().to_string(),
            source: format!("{}, {}", l.cite.source, l.cite.locator),
        })
        .collect();
    for a in &selection.assumed {
        if a.author.trim().is_empty()
            || a.reason.trim().is_empty()
            || !(a.max_wt_ppm.is_finite() && a.max_wt_ppm >= 0.0)
        {
            return Err(NotEvaluated::new(
                format!(
                    "the assumed limit for {} needs an author, a reason and a value of at least 0",
                    a.element
                ),
                vec![a.element.clone()],
                "Fill the author, the reason and the limit in wt ppm.",
            ));
        }
        if lines.iter().any(|l| l.element == a.element) {
            return Err(NotEvaluated::new(
                format!(
                    "{} is already in the stored specification; an assumed limit would silently replace it",
                    a.element
                ),
                vec![a.element.clone()],
                "Remove the assumed entry, or choose another specification.",
            ));
        }
        lines.push(ImpurityLine {
            element: a.element.clone(),
            max_wt_ppm: a.max_wt_ppm,
            kind: SpecKind::Assumed.text().to_string(),
            source: format!("authored by {}: {}", a.author, a.reason),
        });
    }
    let missing: Vec<String> = needed
        .into_iter()
        .filter(|e| !lines.iter().any(|l| &l.element == e))
        .collect();
    if !missing.is_empty() {
        return Err(NotEvaluated::new(
            format!(
                "specification {} gives no limit for these watch-list elements, so their activation cannot be bounded",
                spec.id
            ),
            missing,
            "Add an assumed limit for each (author, reason, wt ppm), for example a measured assay, then run again.",
        ));
    }
    Ok(ImpurityBasis {
        spec_id: spec.id.clone(),
        spec_kind: spec.kind,
        lines,
    })
}

#[derive(Clone, Debug, Serialize)]
pub struct MaterialWasteVerdict {
    pub material: String,
    pub impurity_basis: Option<ImpurityBasis>,
    pub classification: Option<WasteResult>,
    /// `CLASS_A` and so on, or `NOT_EVALUATED`.
    pub verdict: String,
    pub not_evaluated: Option<NotEvaluated>,
    pub database: DbBinding,
    pub research_screening: String,
}

/// A waste class for a component of a stored material, only with a stated impurity specification
/// that the inventory was computed with (MAT-043, MAT-051).
pub fn classify_material(
    db: &MaterialsDb,
    material_id: &str,
    selection: Option<&SpecSelection>,
    inventory_spec_id: &str,
    inventory: &WasteInventory,
    disposal_time_years: f64,
) -> MaterialWasteVerdict {
    let mut out = MaterialWasteVerdict {
        material: material_id.to_string(),
        impurity_basis: None,
        classification: None,
        verdict: "NOT_EVALUATED".to_string(),
        not_evaluated: None,
        database: db.binding.clone(),
        research_screening: RESEARCH_SCREENING_STATEMENT.to_string(),
    };
    match impurity_basis(db, material_id, selection) {
        Err(e) => out.not_evaluated = Some(e),
        Ok(basis) => {
            if inventory_spec_id != basis.spec_id {
                out.not_evaluated = Some(NotEvaluated::new(
                    format!(
                        "the inventory was computed with specification '{inventory_spec_id}' and the verdict asks for '{}'; the two must be the same",
                        basis.spec_id
                    ),
                    vec!["inventory_spec_id".to_string()],
                    "Recompute the activation with the chosen specification, or choose the one the inventory used.",
                ));
            } else {
                let r = classify_nrc(inventory, disposal_time_years);
                out.verdict = r.verdict.text().to_string();
                if let Some(ne) = &r.not_evaluated {
                    out.not_evaluated = Some(ne.clone());
                }
                out.classification = Some(r);
            }
            out.impurity_basis = Some(basis);
        }
    }
    out
}

/// Activity per weight ppm of one element, as nuclide activities (Bq per wt ppm) on the same basis
/// (component, volume, mass, decay reference) as the inventory it is used with.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ElementYield {
    pub element: String,
    pub per_ppm: Vec<NuclideActivity>,
}

fn yield_inventory(base: &WasteInventory, y: &ElementYield, scale: f64) -> WasteInventory {
    let mut inv = base.clone();
    inv.nuclides = y
        .per_ppm
        .iter()
        .map(|n| NuclideActivity {
            activity_bq: n.activity_bq * scale,
            ..n.clone()
        })
        .collect();
    inv
}

/// The inventory with `element` at `new_ppm` instead of `current_ppm`. Linear in the element.
pub fn inventory_at_ppm(
    base: &WasteInventory,
    y: &ElementYield,
    current_ppm: f64,
    new_ppm: f64,
) -> Result<WasteInventory, NotEvaluated> {
    let mut inv = base.clone();
    for n in &y.per_ppm {
        let delta = n.activity_bq * (new_ppm - current_ppm);
        match inv.nuclides.iter_mut().find(|b| b.nuclide == n.nuclide) {
            Some(b) => {
                b.activity_bq += delta;
                if b.activity_bq < -1e-9 * n.activity_bq.abs().max(1.0) {
                    return Err(NotEvaluated::new(
                        format!(
                            "removing {current_ppm} ppm of {} takes {} below zero; the inventory and the yield are inconsistent",
                            y.element, n.nuclide
                        ),
                        vec![n.nuclide.clone()],
                        "Check that current_ppm is the element's content the inventory was computed with.",
                    ));
                }
                b.activity_bq = b.activity_bq.max(0.0);
            }
            None => {
                if delta < 0.0 {
                    return Err(NotEvaluated::new(
                        format!(
                            "{} is not in the inventory but the element's current content should have produced it",
                            n.nuclide
                        ),
                        vec![n.nuclide.clone()],
                        "Add the nuclide to the inventory, or set current_ppm to 0.",
                    ));
                }
                inv.nuclides.push(NuclideActivity {
                    activity_bq: delta,
                    ..n.clone()
                });
            }
        }
    }
    Ok(inv)
}

#[derive(Clone, Debug, Serialize)]
pub struct ConstraintLine {
    pub constraint: String,
    /// Sum of fractions of this column without the element.
    pub base_sum: f64,
    /// Added to the sum per wt ppm of the element.
    pub slope_per_ppm: f64,
    pub bound: f64,
    /// Largest ppm this constraint allows; `None` when the element does not enter the column.
    pub max_wt_ppm: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MaxImpurity {
    pub element: String,
    pub target: Verdict,
    pub target_text: String,
    pub disposal_time_years: f64,
    /// Largest content that still gives at most the target class. `None` when no constraint binds
    /// (the element does not enter any limiting column) or the result is NOT_EVALUATED.
    pub max_wt_ppm: Option<f64>,
    /// True when the class is above the target even with none of the element.
    pub exceeded_without_element: bool,
    pub binding_constraint: Option<String>,
    pub constraints: Vec<ConstraintLine>,
    pub not_evaluated: Option<NotEvaluated>,
    pub database: DbBinding,
    pub kind: ResultKind,
    pub research_screening: String,
}

/// Largest content of `y.element` that keeps `inventory` at or below `target` (Class A, B or C) at
/// the disposal time; `current_ppm` is the content the inventory was computed with (MAT-042).
pub fn max_impurity_for_class(
    db: &MaterialsDb,
    inventory: &WasteInventory,
    y: &ElementYield,
    current_ppm: f64,
    target: Verdict,
    disposal_time_years: f64,
) -> MaxImpurity {
    let mut out = MaxImpurity {
        element: y.element.clone(),
        target,
        target_text: target.text().to_string(),
        disposal_time_years,
        max_wt_ppm: None,
        exceeded_without_element: false,
        binding_constraint: None,
        constraints: Vec::new(),
        not_evaluated: None,
        database: db.binding.clone(),
        kind: ResultKind::Calculated,
        research_screening: RESEARCH_SCREENING_STATEMENT.to_string(),
    };
    if !matches!(target, Verdict::ClassA | Verdict::ClassB | Verdict::ClassC) {
        out.not_evaluated = Some(NotEvaluated::new(
            "the target must be Class A, B or C",
            vec!["target".to_string()],
            "Choose CLASS_A, CLASS_B or CLASS_C.",
        ));
        return out;
    }
    if !(current_ppm.is_finite() && current_ppm >= 0.0) {
        out.not_evaluated = Some(NotEvaluated::new(
            "current_ppm must be a number of at least 0",
            vec!["current_ppm".to_string()],
            "Give the element's content, in wt ppm, that the inventory was computed with.",
        ));
        return out;
    }
    // The inventory must itself be classifiable (complete), else there is no base to subtract from.
    let forward = classify_nrc(inventory, disposal_time_years);
    if let Some(ne) = forward.not_evaluated {
        out.not_evaluated = Some(ne);
        return out;
    }
    let slope: ColumnSums =
        match sums_unchecked(&yield_inventory(inventory, y, 1.0), disposal_time_years) {
            Ok(s) => s,
            Err(e) => {
                out.not_evaluated = Some(e);
                return out;
            }
        };
    let base = ColumnSums {
        table1: forward.sums.table1 - current_ppm * slope.table1,
        table2_col1: forward.sums.table2_col1 - current_ppm * slope.table2_col1,
        table2_col2: forward.sums.table2_col2 - current_ppm * slope.table2_col2,
        table2_col3: forward.sums.table2_col3 - current_ppm * slope.table2_col3,
    };
    let rows: [(&str, f64, f64, f64); 2] = match target {
        Verdict::ClassA => [
            ("Table 1 sum at most 0.1", base.table1, slope.table1, 0.1),
            (
                "Table 2 column 1 sum at most 1",
                base.table2_col1,
                slope.table2_col1,
                1.0,
            ),
        ],
        Verdict::ClassB => [
            ("Table 1 sum at most 0.1", base.table1, slope.table1, 0.1),
            (
                "Table 2 column 2 sum at most 1",
                base.table2_col2,
                slope.table2_col2,
                1.0,
            ),
        ],
        _ => [
            ("Table 1 sum at most 1", base.table1, slope.table1, 1.0),
            (
                "Table 2 column 3 sum at most 1",
                base.table2_col3,
                slope.table2_col3,
                1.0,
            ),
        ],
    };
    let mut best: Option<(f64, &str)> = None;
    for (name, base_sum, slope_sum, bound) in rows {
        let max = (slope_sum > 0.0).then(|| ((bound - base_sum) / slope_sum).max(0.0));
        if base_sum > bound * (1.0 + crate::waste::BOUNDARY_TOLERANCE) {
            out.exceeded_without_element = true;
        }
        if let Some(m) = max
            && best.is_none_or(|b| m < b.0)
        {
            best = Some((m, name));
        }
        out.constraints.push(ConstraintLine {
            constraint: name.to_string(),
            base_sum,
            slope_per_ppm: slope_sum,
            bound,
            max_wt_ppm: max,
        });
    }
    out.max_wt_ppm = best.map(|b| {
        if out.exceeded_without_element {
            0.0
        } else {
            b.0
        }
    });
    out.binding_constraint = best.map(|b| b.1.to_string());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::waste::{CURIE_BQ, WasteForm, YEAR_S, classify::required_nuclides};

    fn db() -> &'static MaterialsDb {
        MaterialsDb::embedded()
    }

    // Verifies: MAT-041
    #[test]
    fn the_watch_list_has_the_required_elements_each_with_a_reason_and_a_threshold() {
        let list = watchlist(db());
        let elements: Vec<_> = list.iter().map(|w| w.element.as_str()).collect();
        for e in ["Nb", "Mo", "Ag", "Co", "Gd", "Tb", "Ho"] {
            assert!(elements.contains(&e), "{e}");
        }
        for w in &list {
            assert!(
                !w.reason.is_empty() && !w.cite.quote.is_empty() && !w.nuclides.is_empty(),
                "{}",
                w.element
            );
        }
        let nb = list.iter().find(|w| w.element == "Nb").unwrap();
        match &nb.class_threshold {
            ResolvedThreshold::Nrc(t) => {
                assert_eq!(t.row_label, "Nb-94 in activated metal");
                assert_eq!(t.class_c_max, Some(0.2));
                assert!((t.class_a_max.unwrap() - 0.02).abs() < 1e-15);
                assert_eq!(t.unit, "Ci/m3");
            }
            other => panic!("{other:?}"),
        }
        let co = list.iter().find(|w| w.element == "Co").unwrap();
        match &co.class_threshold {
            ResolvedThreshold::Nrc(t) => {
                assert_eq!(
                    (t.class_a_max, t.class_b_max, t.class_c_max),
                    (Some(700.0), None, None)
                );
            }
            other => panic!("{other:?}"),
        }
        // An element whose nuclide 10 CFR 61.55 does not list says so and cites the literature.
        let ag = list.iter().find(|w| w.element == "Ag").unwrap();
        match &ag.class_threshold {
            ResolvedThreshold::Fetter {
                nuclide,
                value,
                note,
                ..
            } => {
                assert_eq!((nuclide.as_str(), *value), ("Ag-108m", 3.0));
                assert!(note.contains("no row for Ag-108m"));
            }
            other => panic!("{other:?}"),
        }
    }

    fn selection(spec: &str) -> SpecSelection {
        SpecSelection {
            spec_id: spec.to_string(),
            assumed: vec![],
        }
    }

    fn assumed(element: &str) -> AssumedImpurity {
        AssumedImpurity {
            element: element.to_string(),
            max_wt_ppm: 1.0,
            author: "C. Tester".to_string(),
            reason: "assay of the heat, test value".to_string(),
        }
    }

    // Verifies: MAT-043
    #[test]
    fn a_waste_verdict_without_an_impurity_specification_is_refused() {
        let inv = inventory(&[]);
        let v = classify_material(db(), "eurofer97", None, "eurofer97-specified", &inv, 0.0);
        assert_eq!(v.verdict, "NOT_EVALUATED");
        let ne = v.not_evaluated.unwrap();
        assert_eq!(ne.missing, ["Nb", "Mo", "Ag", "Co", "Gd", "Tb", "Ho"]);
        assert!(ne.next_step.contains("specification"));
        assert!(v.classification.is_none());
        // The stored specification lacks Ag, Gd, Tb and Ho: refused, naming them.
        let v = classify_material(
            db(),
            "eurofer97",
            Some(&selection("eurofer97-specified")),
            "eurofer97-specified",
            &inv,
            0.0,
        );
        assert_eq!(v.not_evaluated.unwrap().missing, ["Ag", "Gd", "Tb", "Ho"]);
        // Adding authored limits for them lets the classification run.
        let sel = SpecSelection {
            spec_id: "eurofer97-specified".to_string(),
            assumed: ["Ag", "Gd", "Tb", "Ho"]
                .iter()
                .map(|e| assumed(e))
                .collect(),
        };
        let v = classify_material(
            db(),
            "eurofer97",
            Some(&sel),
            "eurofer97-specified",
            &inv,
            0.0,
        );
        assert_eq!(v.verdict, "CLASS_A");
        let basis = v.impurity_basis.unwrap();
        assert_eq!(
            basis.lines.iter().filter(|l| l.kind == "assumed").count(),
            4
        );
        assert_eq!(
            basis.lines.iter().filter(|l| l.kind == "specified").count(),
            8
        );
        assert_eq!(v.database, db().binding);
    }

    // Verifies: MAT-040, MAT-043
    #[test]
    fn two_specifications_give_two_verdicts_and_the_inventory_must_match_the_one_chosen() {
        let sel = |spec: &str| SpecSelection {
            spec_id: spec.to_string(),
            assumed: ["Ag", "Gd", "Tb", "Ho"]
                .iter()
                .map(|e| assumed(e))
                .collect(),
        };
        let inv = inventory(&[]);
        let a = classify_material(
            db(),
            "eurofer97",
            Some(&sel("eurofer97-specified")),
            "eurofer97-specified",
            &inv,
            0.0,
        );
        let b = classify_material(
            db(),
            "eurofer97",
            Some(&sel("eurofer97-achieved-lindau2005")),
            "eurofer97-achieved-lindau2005",
            &inv,
            0.0,
        );
        assert_eq!(
            a.impurity_basis.as_ref().unwrap().spec_kind,
            SpecKind::Specified
        );
        assert_eq!(
            b.impurity_basis.as_ref().unwrap().spec_kind,
            SpecKind::Measured
        );
        assert_ne!(
            serde_json::to_string(&a.impurity_basis).unwrap(),
            serde_json::to_string(&b.impurity_basis).unwrap()
        );
        let wrong = classify_material(
            db(),
            "eurofer97",
            Some(&sel("eurofer97-specified")),
            "eurofer97-achieved-lindau2005",
            &inv,
            0.0,
        );
        assert_eq!(wrong.verdict, "NOT_EVALUATED");
        // An assumed entry that would replace a stored one is refused.
        let mut dup = sel("eurofer97-specified");
        dup.assumed.push(assumed("Nb"));
        assert!(impurity_basis(db(), "eurofer97", Some(&dup)).is_err());
    }

    /// Synthetic inventory for the tests: required nuclides at zero plus the entries (Ci per m3
    /// over 1 m3, half-life in years). Not real activation results.
    fn inventory(entries: &[(&str, f64, f64)]) -> WasteInventory {
        let form = WasteForm::ActivatedMetal;
        let mut nuclides: Vec<NuclideActivity> = required_nuclides(form)
            .into_iter()
            .map(|n| NuclideActivity {
                nuclide: n,
                activity_bq: 0.0,
                half_life_s: 100.0 * YEAR_S,
                alpha_emitter: None,
                progeny_of: None,
            })
            .collect();
        for (name, ci, hl) in entries {
            if let Some(n) = nuclides.iter_mut().find(|n| n.nuclide == *name) {
                n.activity_bq = ci * CURIE_BQ;
                n.half_life_s = hl * YEAR_S;
            } else {
                nuclides.push(NuclideActivity {
                    nuclide: (*name).to_string(),
                    activity_bq: ci * CURIE_BQ,
                    half_life_s: hl * YEAR_S,
                    alpha_emitter: None,
                    progeny_of: None,
                });
            }
        }
        WasteInventory {
            component_id: "synthetic".to_string(),
            waste_form: form,
            volume_m3: Some(1.0),
            mass_g: Some(1.0e6),
            inventory_time_years: 0.0,
            transuranics_evaluated: true,
            inventory_kind: "authored_test".to_string(),
            nuclides,
        }
    }

    /// Synthetic yields in Ci per m3 per wt ppm, converted to Bq.
    fn yield_of(element: &str, entries: &[(&str, f64, f64)]) -> ElementYield {
        ElementYield {
            element: element.to_string(),
            per_ppm: entries
                .iter()
                .map(|(n, ci, hl)| NuclideActivity {
                    nuclide: (*n).to_string(),
                    activity_bq: ci * CURIE_BQ,
                    half_life_s: hl * YEAR_S,
                    alpha_emitter: None,
                    progeny_of: None,
                })
                .collect(),
        }
    }

    // Verifies: MAT-042
    #[test]
    fn the_inverse_agrees_with_the_forward_classification_on_ten_cases() {
        // (base inventory, yield, current ppm, target)
        type Case<'a> = (Vec<(&'a str, f64, f64)>, ElementYield, f64, Verdict);
        let cases: Vec<Case> = vec![
            (
                vec![],
                yield_of("Nb", &[("Nb-94", 0.004, 20_300.0)]),
                0.0,
                Verdict::ClassA,
            ),
            (
                vec![],
                yield_of("Nb", &[("Nb-94", 0.004, 20_300.0)]),
                0.0,
                Verdict::ClassC,
            ),
            (
                vec![("Nb-94", 0.01, 20_300.0)],
                yield_of("Nb", &[("Nb-94", 0.004, 20_300.0)]),
                2.5,
                Verdict::ClassC,
            ),
            (
                vec![("Cs-137", 0.2, 30.08)],
                yield_of("Cs", &[("Cs-137", 0.01, 30.08)]),
                0.0,
                Verdict::ClassA,
            ),
            (
                vec![("Cs-137", 0.2, 30.08)],
                yield_of("Cs", &[("Cs-137", 0.01, 30.08)]),
                0.0,
                Verdict::ClassB,
            ),
            (
                vec![("Cs-137", 0.2, 30.08)],
                yield_of("Cs", &[("Cs-137", 0.01, 30.08)]),
                0.0,
                Verdict::ClassC,
            ),
            (
                vec![("Ni-63", 5.0, 100.1)],
                yield_of("Ni", &[("Ni-63", 0.5, 100.1), ("Ni-59", 0.01, 76_000.0)]),
                4.0,
                Verdict::ClassA,
            ),
            (
                vec![("Ni-63", 5.0, 100.1)],
                yield_of("Ni", &[("Ni-63", 0.5, 100.1), ("Ni-59", 0.01, 76_000.0)]),
                4.0,
                Verdict::ClassB,
            ),
            (
                vec![("Co-60", 100.0, 5.2713)],
                yield_of("Co", &[("Co-60", 20.0, 5.2713)]),
                0.0,
                Verdict::ClassA,
            ),
            (
                vec![("H-3", 10.0, 12.32), ("Mn-54", 100.0, 0.855)],
                yield_of("Mn", &[("Mn-54", 7.0, 0.855)]),
                0.0,
                Verdict::ClassA,
            ),
        ];
        assert!(cases.len() >= 10);
        for (i, (entries, y, current, target)) in cases.iter().enumerate() {
            let mut base = inventory(entries);
            // Put the current content of the element into the base inventory.
            base = inventory_at_ppm(&base, y, 0.0, *current).unwrap();
            let r = max_impurity_for_class(db(), &base, y, *current, *target, 0.0);
            assert!(r.not_evaluated.is_none(), "case {i}: {:?}", r.not_evaluated);
            let max = r
                .max_wt_ppm
                .unwrap_or_else(|| panic!("case {i}: no maximum"));
            // At 99.9 percent of the maximum the forward class is at most the target ...
            let within = inventory_at_ppm(&base, y, *current, max * 0.999).unwrap();
            let verdict = classify_nrc(&within, 0.0).verdict;
            assert!(verdict <= *target, "case {i}: {verdict:?} at 0.999 max");
            // ... and 5 percent above it, the class is above the target.
            let above = inventory_at_ppm(&base, y, *current, max * 1.05).unwrap();
            let verdict = classify_nrc(&above, 0.0).verdict;
            assert!(verdict > *target, "case {i}: {verdict:?} at 1.05 max");
            assert!(r.binding_constraint.is_some());
        }
    }

    // Verifies: MAT-042
    #[test]
    fn the_inverse_names_the_binding_column_and_flags_an_already_exceeded_base() {
        let y = yield_of("Nb", &[("Nb-94", 0.004, 20_300.0)]);
        let base = inventory(&[]);
        let a = max_impurity_for_class(db(), &base, &y, 0.0, Verdict::ClassA, 0.0);
        // Class A: Nb-94 at most 0.02 Ci/m3, so 0.02 / 0.004 = 5 ppm.
        assert!((a.max_wt_ppm.unwrap() - 5.0).abs() < 1e-9);
        assert!(a.binding_constraint.unwrap().contains("Table 1"));
        let c = max_impurity_for_class(db(), &base, &y, 0.0, Verdict::ClassC, 0.0);
        assert!((c.max_wt_ppm.unwrap() - 50.0).abs() < 1e-9);
        // A base already above Class A without the element gives a maximum of zero.
        let hot = inventory(&[("Cs-137", 5.0, 30.08)]);
        let r = max_impurity_for_class(db(), &hot, &y, 0.0, Verdict::ClassA, 0.0);
        assert!(r.exceeded_without_element);
        assert_eq!(r.max_wt_ppm, Some(0.0));
        // An element that enters no column has no maximum, and says so.
        let inert = yield_of("Zz", &[("Mn-53", 1.0, 3.7e6)]);
        let r = max_impurity_for_class(db(), &base, &inert, 0.0, Verdict::ClassA, 0.0);
        assert!(r.not_evaluated.is_none());
        assert_eq!(r.max_wt_ppm, None);
        assert!(!r.exceeded_without_element);
        // An incomplete base inventory is refused, never defaulted.
        let mut truncated = inventory(&[]);
        truncated.nuclides.retain(|n| n.nuclide != "Tc-99");
        let r = max_impurity_for_class(db(), &truncated, &y, 0.0, Verdict::ClassA, 0.0);
        assert_eq!(r.not_evaluated.unwrap().missing, vec!["Tc-99"]);
    }

    // Verifies: MAT-042
    #[test]
    fn the_maximum_moves_with_the_disposal_time() {
        // Mn-54 (0.855 y) enters the under-5-year total (Class A limit 700 Ci/m3): after one
        // half-life twice as much may be present at the start.
        let y = yield_of("Mn", &[("Mn-54", 7.0, 0.855)]);
        let base = inventory(&[]);
        let now = max_impurity_for_class(db(), &base, &y, 0.0, Verdict::ClassA, 0.0)
            .max_wt_ppm
            .unwrap();
        let later = max_impurity_for_class(db(), &base, &y, 0.0, Verdict::ClassA, 0.855)
            .max_wt_ppm
            .unwrap();
        assert!((now - 100.0).abs() < 1e-6);
        assert!((later / now - 2.0).abs() < 1e-9);
    }
}
