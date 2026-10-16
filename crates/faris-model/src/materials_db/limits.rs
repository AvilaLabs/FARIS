//! Comparing a result with a stored limit (MAT-010 to MAT-012, MAT-015, MAT-050 to MAT-052).
//!
//! A comparison happens only when the case says what the limit needs: for a dpa limit the
//! convention must match; where the limit states an irradiation temperature window, a spectrum or
//! a material form, the case must state its own. A condition that does not hold leaves the
//! comparison in place but labelled conditional, with the unmet conditions listed. Every result
//! carries the database binding (version and content hash).

use serde::{Deserialize, Serialize};

use super::{DbBinding, Label, Limit, LimitStatus, MaterialsDb, NotEvaluated, Quantity};
use crate::RESEARCH_SCREENING_STATEMENT;

/// What is known about the case a value comes from.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CaseConditions {
    /// Convention the case's dpa values use (`nrt`, `arc_dpa`, ...).
    #[serde(default)]
    pub dpa_convention: Option<String>,
    #[serde(default)]
    pub irradiation_temperature_c: Option<f64>,
    #[serde(default)]
    pub spectrum: Option<String>,
    #[serde(default)]
    pub form: Option<String>,
}

/// A user's replacement of a stored limit (MAT-052). Never labelled literature.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LimitOverride {
    pub limit_id: String,
    pub value: f64,
    /// The dpa convention the override is written in, where the stored limit does not state one.
    #[serde(default)]
    pub convention: Option<String>,
    pub author: String,
    /// When it was written, as text the caller supplies (FARIS reads no clock).
    pub time: String,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub enum ComparisonLabel {
    #[serde(rename = "literature")]
    Literature,
    #[serde(rename = "conditional")]
    Conditional,
    #[serde(rename = "authored_override")]
    AuthoredOverride,
    #[serde(rename = "not_evaluated")]
    NotEvaluated,
}

#[derive(Clone, Debug, Serialize)]
pub struct Comparison {
    pub limit_id: String,
    pub material: String,
    pub quantity: Quantity,
    pub case_value: f64,
    pub limit_value: Option<f64>,
    pub unit: String,
    /// `WITHIN`, `EXCEEDED` or `NOT_EVALUATED`.
    pub verdict: String,
    pub label: ComparisonLabel,
    /// Value over limit.
    pub ratio: Option<f64>,
    pub conditions_unmet: Vec<String>,
    pub limit_conditions: Vec<String>,
    pub limit_status: LimitStatus,
    pub applied_override: Option<LimitOverride>,
    pub stored_value: Option<f64>,
    pub not_evaluated: Option<NotEvaluated>,
    pub database: DbBinding,
    pub research_screening: String,
}

fn not_evaluated(
    db: &MaterialsDb,
    limit: &Limit,
    value: f64,
    ne: NotEvaluated,
    applied_override: Option<LimitOverride>,
) -> Comparison {
    Comparison {
        limit_id: limit.id.clone(),
        material: limit.material.clone(),
        quantity: limit.quantity,
        case_value: value,
        limit_value: None,
        unit: limit.unit.clone(),
        verdict: "NOT_EVALUATED".to_string(),
        label: ComparisonLabel::NotEvaluated,
        ratio: None,
        conditions_unmet: Vec::new(),
        limit_conditions: limit.conditions.clone(),
        limit_status: limit.status,
        applied_override,
        stored_value: limit.value,
        not_evaluated: Some(ne),
        database: db.binding.clone(),
        research_screening: RESEARCH_SCREENING_STATEMENT.to_string(),
    }
}

/// Compares `value` with the limit `limit_id` under the case's conditions.
pub fn compare(
    db: &MaterialsDb,
    limit_id: &str,
    value: f64,
    case: &CaseConditions,
    override_: Option<&LimitOverride>,
) -> Result<Comparison, NotEvaluated> {
    let limit = db.limit(limit_id).ok_or_else(|| {
        NotEvaluated::new(
            format!("limit {limit_id} is not in the database"),
            vec![limit_id.to_string()],
            "List the limits with `faris materials limits` and use one of those ids.",
        )
    })?;
    if let Some(o) = override_ {
        if o.limit_id != limit.id {
            return Err(NotEvaluated::new(
                format!(
                    "the override is for {} and the limit is {}",
                    o.limit_id, limit.id
                ),
                vec![o.limit_id.clone()],
                "Give the override for the same limit id.",
            ));
        }
        for (what, text) in [
            ("author", &o.author),
            ("time", &o.time),
            ("reason", &o.reason),
        ] {
            if text.trim().is_empty() {
                return Err(NotEvaluated::new(
                    format!("an override needs its {what}"),
                    vec![format!("override.{what}")],
                    "Give the author, the time and the reason of the override.",
                ));
            }
        }
        if !(o.value.is_finite() && o.value >= 0.0) {
            return Err(NotEvaluated::new(
                "an override value must be a number of at least 0",
                vec!["override.value".to_string()],
                "Give the replacement limit.",
            ));
        }
    }
    if !value.is_finite() {
        return Ok(not_evaluated(
            db,
            limit,
            value,
            NotEvaluated::new(
                "the case value is not a finite number",
                vec!["value".to_string()],
                "Give the case value.",
            ),
            override_.cloned(),
        ));
    }
    // A stored limit without a value gives its own reason, unless an override replaces it.
    let limit_value = match (override_, limit.value) {
        (Some(o), _) => o.value,
        (None, Some(v)) => v,
        (None, None) => {
            let r = limit.not_evaluated.clone().map_or_else(
                || {
                    NotEvaluated::new(
                        "the limit has no value",
                        vec![limit.id.clone()],
                        "Add a cited value.",
                    )
                },
                |r| NotEvaluated::new(r.reason, vec![limit.id.clone()], r.next_step),
            );
            return Ok(not_evaluated(db, limit, value, r, None));
        }
    };
    let mut unmet: Vec<String> = Vec::new();
    // MAT-011: a dpa value is compared only with a limit in the same convention.
    if limit.quantity == Quantity::Dpa {
        let limit_convention = override_
            .and_then(|o| o.convention.as_deref())
            .or_else(|| limit.convention.value().map(String::as_str));
        match (limit_convention, case.dpa_convention.as_deref()) {
            (Some(l), Some(c)) if l.eq_ignore_ascii_case(c) => {}
            (Some(l), Some(c)) => {
                return Ok(not_evaluated(
                    db,
                    limit,
                    value,
                    NotEvaluated::new(
                        format!(
                            "the limit is in the {l} dpa convention and the value is in {c}; the conventions differ by up to a factor of about three in metals, so they are not compared"
                        ),
                        vec!["dpa_convention".to_string()],
                        "Convert both to one convention outside FARIS and state it, or recalculate the damage in the limit's convention.",
                    ),
                    override_.cloned(),
                ));
            }
            (None, _) => {
                return Ok(not_evaluated(
                    db,
                    limit,
                    value,
                    NotEvaluated::new(
                        format!(
                            "the limit does not state its dpa convention ({}), so a dpa value cannot be compared with it",
                            limit.convention.reason().unwrap_or("no reason given")
                        ),
                        vec!["limit.convention".to_string()],
                        "Find the convention in the source and enter it as an authored override that names it, or choose a limit that states it.",
                    ),
                    override_.cloned(),
                ));
            }
            (Some(_), None) => {
                return Ok(not_evaluated(
                    db,
                    limit,
                    value,
                    NotEvaluated::new(
                        "the case does not state the dpa convention of its value",
                        vec!["case.dpa_convention".to_string()],
                        "Set dpa_convention in the case conditions (for example nrt or arc_dpa).",
                    ),
                    override_.cloned(),
                ));
            }
        }
    }
    // MAT-015: stated conditions of the limit against the case.
    if let Some((lo, hi)) = limit.temperature_window_c.value() {
        match case.irradiation_temperature_c {
            None => {
                return Ok(not_evaluated(
                    db,
                    limit,
                    value,
                    NotEvaluated::new(
                        format!(
                            "the limit holds for irradiation temperatures {lo} to {hi} degC and the case gives none"
                        ),
                        vec!["case.irradiation_temperature_c".to_string()],
                        "Give the irradiation temperature of the component.",
                    ),
                    override_.cloned(),
                ));
            }
            Some(t) if t < *lo || t > *hi => unmet.push(format!(
                "irradiation temperature {t} degC is outside the limit's window {lo} to {hi} degC"
            )),
            Some(_) => {}
        }
    }
    for (what, stated, given) in [
        ("spectrum", limit.spectrum.value(), case.spectrum.as_ref()),
        ("material form", limit.form.value(), case.form.as_ref()),
    ] {
        if let Some(wanted) = stated {
            match given {
                None => {
                    return Ok(not_evaluated(
                        db,
                        limit,
                        value,
                        NotEvaluated::new(
                            format!(
                                "the limit is stated for: {wanted} ({what}) and the case gives none"
                            ),
                            vec![format!("case.{}", what.replace(' ', "_"))],
                            format!("Give the case's {what}."),
                        ),
                        override_.cloned(),
                    ));
                }
                Some(g) if !g.eq_ignore_ascii_case(wanted) => {
                    unmet.push(format!(
                        "the case's {what} is '{g}' and the limit is stated for '{wanted}'"
                    ));
                }
                Some(_) => {}
            }
        }
    }
    let label = if override_.is_some() {
        ComparisonLabel::AuthoredOverride
    } else if !unmet.is_empty() || limit.kind == Label::Conditional {
        ComparisonLabel::Conditional
    } else {
        ComparisonLabel::Literature
    };
    Ok(Comparison {
        limit_id: limit.id.clone(),
        material: limit.material.clone(),
        quantity: limit.quantity,
        case_value: value,
        limit_value: Some(limit_value),
        unit: limit.unit.clone(),
        verdict: if value <= limit_value {
            "WITHIN"
        } else {
            "EXCEEDED"
        }
        .to_string(),
        label,
        ratio: (limit_value > 0.0).then(|| value / limit_value),
        conditions_unmet: unmet,
        limit_conditions: limit.conditions.clone(),
        limit_status: limit.status,
        applied_override: override_.cloned(),
        stored_value: limit.value,
        not_evaluated: None,
        database: db.binding.clone(),
        research_screening: RESEARCH_SCREENING_STATEMENT.to_string(),
    })
}

/// One criterion of a time-to-limit check: accumulated value and a constant rate per year.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Criterion {
    pub limit_id: String,
    pub accumulated: f64,
    pub rate_per_year: f64,
    #[serde(default, rename = "override")]
    pub override_: Option<LimitOverride>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CriterionResult {
    pub limit_id: String,
    pub quantity: Quantity,
    /// Years until the limit is reached at the stated rate; `None` when not evaluated or when the
    /// rate is not positive.
    pub years_to_limit: Option<f64>,
    pub comparison: Comparison,
}

#[derive(Clone, Debug, Serialize)]
pub struct TimeToLimit {
    pub material: String,
    pub criteria: Vec<CriterionResult>,
    /// The limit reached first.
    pub governing: Option<String>,
    pub governing_years: Option<f64>,
    pub not_evaluated: Option<NotEvaluated>,
    pub database: DbBinding,
    pub research_screening: String,
}

/// Time to every limit of `material` named in `criteria`, and which is reached first (MAT-012).
/// A dpa criterion is refused when the material has a helium or hydrogen limit that the
/// criteria do not also check.
pub fn time_to_limit(
    db: &MaterialsDb,
    material: &str,
    criteria: &[Criterion],
    case: &CaseConditions,
) -> TimeToLimit {
    let base = |criteria: Vec<CriterionResult>, governing, governing_years, ne| TimeToLimit {
        material: material.to_string(),
        criteria,
        governing,
        governing_years,
        not_evaluated: ne,
        database: db.binding.clone(),
        research_screening: RESEARCH_SCREENING_STATEMENT.to_string(),
    };
    if db.material(material).is_none() {
        return base(
            Vec::new(),
            None,
            None,
            Some(NotEvaluated::new(
                format!("material {material} is not in the database"),
                vec![material.to_string()],
                "List the materials with `faris materials list`.",
            )),
        );
    }
    let mut quantities = Vec::new();
    for c in criteria {
        match db.limit(&c.limit_id) {
            Some(l) if l.material == material => quantities.push(l.quantity),
            _ => {
                return base(
                    Vec::new(),
                    None,
                    None,
                    Some(NotEvaluated::new(
                        format!("limit {} is not a limit of {material}", c.limit_id),
                        vec![c.limit_id.clone()],
                        "Use limit ids that belong to the material.",
                    )),
                );
            }
        }
    }
    if quantities.contains(&Quantity::Dpa) {
        let uncovered: Vec<String> = db
            .data
            .limits
            .iter()
            .filter(|l| {
                l.material == material
                    && l.quantity.is_gas()
                    && l.status != LimitStatus::NotEvaluated
            })
            .filter(|l| !quantities.contains(&l.quantity))
            .map(|l| l.id.clone())
            .collect();
        if !uncovered.is_empty() {
            return base(
                Vec::new(),
                None,
                None,
                Some(NotEvaluated::new(
                    format!(
                        "{material} has a helium or hydrogen limit, so a dpa-only verdict is refused; the gas limit(s) must be checked with it"
                    ),
                    uncovered,
                    "Add a criterion for each listed gas limit with its accumulated value and rate (appm, appm per year).",
                )),
            );
        }
    }
    let mut results = Vec::new();
    for c in criteria {
        let comparison = match compare(db, &c.limit_id, c.accumulated, case, c.override_.as_ref()) {
            Ok(cmp) => cmp,
            Err(e) => return base(results, None, None, Some(e)),
        };
        let years = match (comparison.limit_value, comparison.not_evaluated.is_none()) {
            (Some(limit), true) if c.rate_per_year > 0.0 && c.rate_per_year.is_finite() => {
                Some(((limit - c.accumulated) / c.rate_per_year).max(0.0))
            }
            _ => None,
        };
        results.push(CriterionResult {
            limit_id: c.limit_id.clone(),
            quantity: comparison.quantity,
            years_to_limit: years,
            comparison,
        });
    }
    if results.iter().any(|r| r.comparison.not_evaluated.is_some()) {
        let missing: Vec<String> = results
            .iter()
            .filter(|r| r.comparison.not_evaluated.is_some())
            .map(|r| r.limit_id.clone())
            .collect();
        return base(
            results,
            None,
            None,
            Some(NotEvaluated::new(
                "at least one criterion could not be evaluated, so the governing criterion cannot be named",
                missing,
                "Read the reason on each criterion marked NOT_EVALUATED and supply what it asks for.",
            )),
        );
    }
    let governing = results
        .iter()
        .filter_map(|r| r.years_to_limit.map(|y| (r.limit_id.clone(), y)))
        .min_by(|a, b| a.1.total_cmp(&b.1));
    base(
        results,
        governing.as_ref().map(|g| g.0.clone()),
        governing.map(|g| g.1),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> &'static MaterialsDb {
        MaterialsDb::embedded()
    }

    fn case_nrt() -> CaseConditions {
        CaseConditions {
            dpa_convention: Some("nrt".to_string()),
            irradiation_temperature_c: Some(400.0),
            spectrum: Some("DEMO fusion neutron spectrum, blanket front wall".to_string()),
            form: Some("plate".to_string()),
        }
    }

    fn convention_override(limit_id: &str, value: f64, convention: &str) -> LimitOverride {
        LimitOverride {
            limit_id: limit_id.to_string(),
            value,
            convention: Some(convention.to_string()),
            author: "C. Tester".to_string(),
            time: "2026-10-15T12:00:00Z".to_string(),
            reason: "Federici's dpa taken as NRT for this study".to_string(),
        }
    }

    // Verifies: MAT-010
    #[test]
    fn every_limit_row_is_complete() {
        for l in &db().data.limits {
            assert!(!l.component.is_empty() && !l.unit.is_empty(), "{}", l.id);
            for stated in [
                l.convention
                    .reason()
                    .map(str::to_string)
                    .or(l.convention.value().cloned()),
                l.spectrum
                    .reason()
                    .map(str::to_string)
                    .or(l.spectrum.value().cloned()),
                l.form
                    .reason()
                    .map(str::to_string)
                    .or(l.form.value().cloned()),
            ] {
                assert!(stated.is_some_and(|s| !s.is_empty()), "{}", l.id);
            }
            assert!(
                l.temperature_window_c.value().is_some()
                    || l.temperature_window_c.reason().is_some()
            );
            if l.status != LimitStatus::NotEvaluated {
                let c = l.cite.as_ref().unwrap();
                assert!(!c.locator.is_empty() && !c.quote.is_empty(), "{}", l.id);
            } else {
                assert!(l.not_evaluated.is_some(), "{}", l.id);
            }
        }
    }

    // Verifies: MAT-011
    #[test]
    fn a_dpa_value_is_not_compared_when_the_convention_is_unstated_or_differs() {
        // The stored EUROFER97 limits do not state a convention: refused even with a matching case.
        let r = compare(
            db(),
            "eurofer97-dpa-starter-blanket",
            15.0,
            &case_nrt(),
            None,
        )
        .unwrap();
        assert_eq!(r.verdict, "NOT_EVALUATED");
        assert!(
            r.not_evaluated
                .unwrap()
                .reason
                .contains("does not state its dpa convention")
        );
        // An authored override that names NRT makes a comparison possible, and only for NRT values.
        let o = convention_override("eurofer97-dpa-starter-blanket", 20.0, "nrt");
        let ok = compare(
            db(),
            "eurofer97-dpa-starter-blanket",
            15.0,
            &case_nrt(),
            Some(&o),
        )
        .unwrap();
        assert_eq!(ok.verdict, "WITHIN");
        let mut arc = case_nrt();
        arc.dpa_convention = Some("arc_dpa".to_string());
        let mismatch =
            compare(db(), "eurofer97-dpa-starter-blanket", 15.0, &arc, Some(&o)).unwrap();
        assert_eq!(mismatch.verdict, "NOT_EVALUATED");
        let reason = mismatch.not_evaluated.unwrap().reason;
        assert!(reason.contains("nrt") && reason.contains("arc_dpa"));
        // No convention on the case at all.
        let mut none = case_nrt();
        none.dpa_convention = None;
        let r = compare(db(), "eurofer97-dpa-starter-blanket", 15.0, &none, Some(&o)).unwrap();
        assert_eq!(r.verdict, "NOT_EVALUATED");
    }

    // Verifies: MAT-052
    #[test]
    fn an_override_is_labelled_authored_and_carries_author_time_and_reason() {
        let o = convention_override("eurofer97-dpa-starter-blanket", 30.0, "nrt");
        let r = compare(
            db(),
            "eurofer97-dpa-starter-blanket",
            25.0,
            &case_nrt(),
            Some(&o),
        )
        .unwrap();
        assert_eq!(r.label, ComparisonLabel::AuthoredOverride);
        assert_eq!(r.limit_value, Some(30.0));
        assert_eq!(r.stored_value, Some(20.0));
        assert_eq!(r.verdict, "WITHIN");
        let applied = r.applied_override.as_ref().unwrap();
        assert_eq!(
            (applied.author.as_str(), applied.time.as_str()),
            ("C. Tester", "2026-10-15T12:00:00Z")
        );
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["label"], "authored_override");
        assert_ne!(json["label"], "literature");
        // Missing author, wrong limit, or negative value are refused.
        let mut bad = o.clone();
        bad.author = " ".to_string();
        assert!(
            compare(
                db(),
                "eurofer97-dpa-starter-blanket",
                25.0,
                &case_nrt(),
                Some(&bad)
            )
            .is_err()
        );
        let mut other = o.clone();
        other.limit_id = "eurofer97-dpa-second-blanket".to_string();
        assert!(
            compare(
                db(),
                "eurofer97-dpa-starter-blanket",
                25.0,
                &case_nrt(),
                Some(&other)
            )
            .is_err()
        );
    }

    // Verifies: MAT-015
    #[test]
    fn conditions_outside_the_stated_window_make_the_comparison_conditional() {
        // The stored He limit of 316SS applies to an annealed GTA weld; a plate case does not match.
        let case = CaseConditions {
            form: Some("plate".to_string()),
            ..CaseConditions::default()
        };
        let r = compare(db(), "ss316-he-weld-cracking", 0.5, &case, None).unwrap();
        assert_eq!(r.verdict, "WITHIN");
        assert_eq!(r.label, ComparisonLabel::Conditional);
        assert!(r.conditions_unmet[0].contains("plate"));
        let right = CaseConditions {
            form: Some("annealed 316SS, gas tungsten arc (GTA) weld".to_string()),
            ..CaseConditions::default()
        };
        let r = compare(db(), "ss316-he-weld-cracking", 2.0, &right, None).unwrap();
        assert_eq!(r.verdict, "EXCEEDED");
        assert_eq!(r.label, ComparisonLabel::Literature);
        assert!(r.conditions_unmet.is_empty());
        // A case that does not state its form is not compared at all.
        let r = compare(
            db(),
            "ss316-he-weld-cracking",
            2.0,
            &CaseConditions::default(),
            None,
        )
        .unwrap();
        assert_eq!(r.verdict, "NOT_EVALUATED");
        assert!(r.not_evaluated.unwrap().next_step.contains("form"));
    }

    // Verifies: MAT-015
    #[test]
    fn a_temperature_window_is_tested_when_a_limit_states_one() {
        // No stored limit states a window yet, so build a database with one by editing the JSON.
        let text = include_str!("../../data/materials/materials_db.json").replacen(
            "\"temperature_window_c\": {\"not_stated\": \"no irradiation temperature is given with the lifetime\"},",
            "\"temperature_window_c\": {\"value\": [300.0, 500.0]},",
            1,
        );
        let db = MaterialsDb::from_json_str(&text).unwrap();
        let o = convention_override("eurofer97-dpa-starter-blanket", 20.0, "nrt");
        let mut case = case_nrt();
        let inside = compare(&db, "eurofer97-dpa-starter-blanket", 10.0, &case, Some(&o)).unwrap();
        assert!(inside.conditions_unmet.is_empty());
        case.irradiation_temperature_c = Some(250.0);
        let outside = compare(&db, "eurofer97-dpa-starter-blanket", 10.0, &case, Some(&o)).unwrap();
        assert!(outside.conditions_unmet[0].contains("outside"));
        case.irradiation_temperature_c = None;
        let unknown = compare(&db, "eurofer97-dpa-starter-blanket", 10.0, &case, Some(&o)).unwrap();
        assert_eq!(unknown.verdict, "NOT_EVALUATED");
    }

    // Verifies: MAT-012
    #[test]
    fn the_first_limit_reached_is_named_and_a_dpa_only_verdict_is_refused_with_a_gas_limit() {
        let o20 = convention_override("eurofer97-dpa-starter-blanket", 20.0, "nrt");
        let dpa = Criterion {
            limit_id: "eurofer97-dpa-starter-blanket".to_string(),
            accumulated: 2.0,
            rate_per_year: 3.0,
            override_: Some(o20.clone()),
        };
        // EUROFER97 has a stored He limit, so dpa alone is refused.
        let refused = time_to_limit(db(), "eurofer97", std::slice::from_ref(&dpa), &case_nrt());
        assert!(refused.governing.is_none());
        let ne = refused.not_evaluated.unwrap();
        assert_eq!(ne.missing, vec!["eurofer97-he-starter-blanket"]);
        assert!(ne.reason.contains("dpa-only verdict is refused"));
        // With the He criterion: 2 dpa at 3 per year reaches 20 dpa in 6 years; He 100 appm at 20 appm
        // per year reaches 200 appm in 5 years, so helium governs.
        let he = Criterion {
            limit_id: "eurofer97-he-starter-blanket".to_string(),
            accumulated: 100.0,
            rate_per_year: 20.0,
            override_: None,
        };
        let both = time_to_limit(db(), "eurofer97", &[dpa.clone(), he.clone()], &case_nrt());
        assert!(both.not_evaluated.is_none(), "{:?}", both.not_evaluated);
        assert_eq!(
            both.governing.as_deref(),
            Some("eurofer97-he-starter-blanket")
        );
        assert!((both.governing_years.unwrap() - 5.0).abs() < 1e-12);
        let dpa_years = both
            .criteria
            .iter()
            .find(|c| c.quantity == Quantity::Dpa)
            .unwrap()
            .years_to_limit
            .unwrap();
        assert!((dpa_years - 6.0).abs() < 1e-12);
        // Faster dpa makes dpa govern.
        let fast = Criterion {
            rate_per_year: 9.0,
            ..dpa
        };
        let both = time_to_limit(db(), "eurofer97", &[fast, he], &case_nrt());
        assert_eq!(
            both.governing.as_deref(),
            Some("eurofer97-dpa-starter-blanket")
        );
        assert!((both.governing_years.unwrap() - 2.0).abs() < 1e-12);
        assert_eq!(both.database.version, "0.1.0");
    }

    // Verifies: MAT-050, MAT-051
    #[test]
    fn every_comparison_carries_the_database_binding_and_a_stored_limit_has_its_quote() {
        let right = CaseConditions {
            form: Some("annealed 316SS, gas tungsten arc (GTA) weld".to_string()),
            ..CaseConditions::default()
        };
        let r = compare(db(), "ss316-he-weld-cracking", 0.7, &right, None).unwrap();
        assert_eq!(r.database, db().binding);
        assert!(db().is_current(&r.database));
        let l = db().limit("ss316-he-weld-cracking").unwrap();
        let cite = l.cite.as_ref().unwrap();
        assert!(cite.quote.contains("approximately 1.0 appm"));
        assert_eq!(cite.value_text, "1.0");
        assert!(r.research_screening.contains("Research screening only"));
    }

    // Verifies: MAT-016
    #[test]
    fn not_evaluated_limit_rows_give_their_reason() {
        for id in [
            "tungsten-limits",
            "inconel718-limits",
            "sic-sic-limits",
            "rebco-fluence-limits",
            "magnet-insulator-dose-limits",
            "eurofer97-h-appm",
        ] {
            let r = compare(db(), id, 1.0, &CaseConditions::default(), None).unwrap();
            assert_eq!(r.verdict, "NOT_EVALUATED", "{id}");
            let ne = r.not_evaluated.unwrap();
            assert!(!ne.reason.is_empty() && !ne.next_step.is_empty());
        }
        for material in [
            "eurofer97",
            "tungsten",
            "cucrzr",
            "inconel718",
            "sic-sic",
            "rebco",
            "magnet-insulator",
        ] {
            assert!(
                db().data.limits.iter().any(|l| l.material == material),
                "{material}"
            );
        }
        assert!(compare(db(), "no-such-limit", 1.0, &CaseConditions::default(), None).is_err());
    }
}
