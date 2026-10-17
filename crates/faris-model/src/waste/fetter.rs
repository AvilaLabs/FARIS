//! Fetter, Cheng and Mann specific-activity limits (ACT-033, SAFE-011).
//!
//! `data/waste/fetter_1990_part2.json` holds Table 2 of S. Fetter, E.T. Cheng, F.M. Mann,
//! Long-term radioactive waste from fusion reactors: Part II, Fusion Engineering and Design 13
//! (1990) 239-246: the specific-activity limits (SALs, Ci/m3) for Class C shallow-land burial of
//! activated metal, for every radionuclide with a half-life between 5 years and 10^12 years up to
//! Cm-248. Every row carries the quoted line of the table, and a test compares each row with the
//! stored text of the table (`fetter_1990_part2_table2.txt`).
//!
//! The sum of fractions of those limits is the quantity computed here. The paper tabulates the
//! limits; it does not define a named rating, so FARIS calls the result what it is: a sum of
//! fractions of the Fetter Class C limits.
//!
//! A range in the SAL column runs from the lower value (waste-form indices of the 10 CFR 61
//! environmental impact statement for non-fuel reactor components) to the upper value
//! (high-activity industrial waste). FARIS uses the lower value as the limit and also reports the
//! sum at the upper values, so a verdict that depends on the waste form says so.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::{
    BOUNDARY_TOLERANCE, CURIE_BQ, NotEvaluated, ResultKind, SchemeInfo, WasteForm, WasteInventory,
    decayed_activity_bq, validate_inventory,
};
use crate::RESEARCH_SCREENING_STATEMENT;

const FETTER_JSON: &str = include_str!("../../data/waste/fetter_1990_part2.json");
/// The stored text of Table 2 the rows were read from.
pub const FETTER_SNAPSHOT_TXT: &str = include_str!("../../data/waste/fetter_1990_part2_table2.txt");

/// Half-life under which the paper does not limit a nuclide (it assumes 100 years of isolation).
const NOT_LIMITED_BELOW_S: f64 = 5.0 * super::YEAR_S;

#[derive(Clone, Debug, Deserialize)]
pub struct FetterSource {
    pub title: String,
    pub kind: String,
    pub url: String,
    pub retrieved: String,
    pub original_sha256: String,
    pub snapshot_file: String,
    pub snapshot_sha256: String,
    pub licence: String,
    pub redistribution: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FetterLimit {
    pub nuclide: String,
    pub half_life_text: String,
    pub sal_text: String,
    /// Lower value of the SAL column, Ci/m3; `None` when the whole entry is TMSA.
    pub sal_low_ci_per_m3: Option<f64>,
    /// Upper value of a range; `None` for a single value or an upper value of TMSA.
    pub sal_high_ci_per_m3: Option<f64>,
    pub sal_high_is_tmsa: bool,
    /// The whole entry is TMSA: no concentration of the pure nuclide reaches a limit.
    pub no_limit_tmsa: bool,
    pub quote: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FetterTables {
    pub scheme_id: String,
    pub scheme_name: String,
    pub scheme_version: String,
    pub scheme_date: String,
    pub source: FetterSource,
    pub unit: String,
    pub limits: Vec<FetterLimit>,
}

impl FetterTables {
    pub fn scheme_info(&self) -> SchemeInfo {
        SchemeInfo {
            id: self.scheme_id.clone(),
            name: self.scheme_name.clone(),
            version: self.scheme_version.clone(),
            date: self.scheme_date.clone(),
            citation: format!(
                "{} ({}), {} source, retrieved {}",
                self.source.title, self.source.url, self.source.kind, self.source.retrieved
            ),
            status: "IMPLEMENTED".to_string(),
            status_reason: None,
        }
    }

    pub fn limit(&self, nuclide: &str) -> Option<&FetterLimit> {
        self.limits.iter().find(|l| l.nuclide == nuclide)
    }
}

pub fn fetter() -> &'static FetterTables {
    static TABLES: OnceLock<FetterTables> = OnceLock::new();
    TABLES.get_or_init(|| {
        serde_json::from_str(FETTER_JSON).expect("the embedded Fetter table file is valid")
    })
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub enum FetterVerdict {
    /// The sum at the lower limits is at most 1.
    #[serde(rename = "SUM_AT_OR_BELOW_1")]
    AtOrBelowOne,
    /// Above 1 at the lower limits, at most 1 at the upper values of the ranges: the verdict
    /// depends on the waste-form indices.
    #[serde(rename = "SUM_AT_OR_BELOW_1_AT_UPPER_RANGE_ONLY")]
    AtOrBelowOneAtUpperRangeOnly,
    /// Above 1 even at the upper values.
    #[serde(rename = "SUM_ABOVE_1")]
    AboveOne,
    #[serde(rename = "NOT_EVALUATED")]
    NotEvaluated,
}

impl FetterVerdict {
    pub fn text(self) -> &'static str {
        match self {
            FetterVerdict::AtOrBelowOne => "SUM_AT_OR_BELOW_1",
            FetterVerdict::AtOrBelowOneAtUpperRangeOnly => "SUM_AT_OR_BELOW_1_AT_UPPER_RANGE_ONLY",
            FetterVerdict::AboveOne => "SUM_ABOVE_1",
            FetterVerdict::NotEvaluated => "NOT_EVALUATED",
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct FetterLine {
    pub nuclide: String,
    pub concentration_ci_per_m3: f64,
    pub limit_ci_per_m3: f64,
    pub fraction: f64,
    /// The upper end of the SAL range; `None` when the upper value is TMSA.
    pub upper_limit_ci_per_m3: Option<f64>,
    pub fraction_at_upper: f64,
    pub quote: String,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct FetterNote {
    pub nuclide: String,
    pub activity_bq: f64,
    pub note: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct FetterResult {
    pub scheme: SchemeInfo,
    /// The regime named on the class: the scheme identifier.
    pub regime: String,
    pub verdict: FetterVerdict,
    pub component_id: String,
    pub waste_form_text: String,
    pub disposal_time_years: f64,
    pub inventory_time_years: f64,
    pub decay_treatment: String,
    pub volume_m3: Option<f64>,
    pub lines: Vec<FetterLine>,
    /// Sum of fractions at the lower limits.
    pub sum_at_lower: f64,
    /// Sum of fractions at the upper values of the ranges (TMSA upper values count 0).
    pub sum_at_upper: f64,
    /// Nuclides in the inventory whose Fetter entry is TMSA: no limit can be reached.
    pub no_limit: Vec<FetterNote>,
    /// Nuclides with activity that Table 2 does not list.
    pub unlisted: Vec<FetterNote>,
    /// True when the inventory lists every nuclide of Table 2 that has a finite limit.
    pub inventory_lists_every_limited_nuclide: bool,
    pub scope_note: String,
    pub rule: String,
    pub not_evaluated: Option<NotEvaluated>,
    pub kind: ResultKind,
    pub inventory_kind: String,
    pub research_screening: String,
}

fn exceeds(sum: f64, bound: f64) -> bool {
    sum > bound * (1.0 + BOUNDARY_TOLERANCE)
}

const SCOPE_NOTE: &str = "The limits are Fetter, Cheng and Mann's Class C specific-activity limits for activated metal, on the effective dose equivalent, for long-lived nuclides (half-life over 5 years). They are not a regulation. Nuclides the inventory does not list count as absent, so a sum at or below 1 holds only for the nuclides listed; the field inventory_lists_every_limited_nuclide says whether the inventory covers every limited nuclide of the table.";

/// Sum of fractions of the Fetter limits for one component at a disposal time.
pub fn classify_fetter(inventory: &WasteInventory, disposal_time_years: f64) -> FetterResult {
    let table = fetter();
    let mut out = FetterResult {
        scheme: table.scheme_info(),
        regime: table.scheme_id.clone(),
        verdict: FetterVerdict::NotEvaluated,
        component_id: inventory.component_id.clone(),
        waste_form_text: inventory.waste_form.text().to_string(),
        disposal_time_years,
        inventory_time_years: inventory.inventory_time_years,
        decay_treatment: "each nuclide decayed from the inventory time to the disposal time by its own half-life; no ingrowth from parents".to_string(),
        volume_m3: inventory.volume_m3,
        lines: Vec::new(),
        sum_at_lower: 0.0,
        sum_at_upper: 0.0,
        no_limit: Vec::new(),
        unlisted: Vec::new(),
        inventory_lists_every_limited_nuclide: false,
        scope_note: SCOPE_NOTE.to_string(),
        rule: "NOT_EVALUATED: the reason and the next step are in the result".to_string(),
        not_evaluated: None,
        kind: ResultKind::Calculated,
        inventory_kind: inventory.inventory_kind.clone(),
        research_screening: RESEARCH_SCREENING_STATEMENT.to_string(),
    };
    if let Err(e) = fill(inventory, disposal_time_years, &mut out) {
        out.verdict = FetterVerdict::NotEvaluated;
        out.not_evaluated = Some(e);
    }
    out
}

fn fill(
    inventory: &WasteInventory,
    disposal_time_years: f64,
    out: &mut FetterResult,
) -> Result<(), NotEvaluated> {
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
    if inventory.waste_form != WasteForm::ActivatedMetal {
        return Err(NotEvaluated::new(
            "the Fetter limits are for activated metal (Table 2: \"Specific activity limits for class C disposal of activated metal\") and this waste form is not activated metal",
            vec!["waste_form".to_string()],
            "Classify the component as activated metal if it is, or use a scheme that states limits for this form.",
        ));
    }
    let volume = inventory.volume_m3.ok_or_else(|| {
        NotEvaluated::new(
            "the Fetter limits are in curie per cubic metre and the inventory gives no volume",
            vec!["volume_m3".to_string()],
            "Give volume_m3, the volume the activity is averaged over.",
        )
    })?;
    let table = fetter();
    let dt = disposal_time_years - inventory.inventory_time_years;
    for n in &inventory.nuclides {
        let activity = decayed_activity_bq(n.activity_bq, n.half_life_s, dt);
        if activity <= 0.0 {
            continue;
        }
        let Some(limit) = table.limit(&n.nuclide) else {
            let note = if n.half_life_s < NOT_LIMITED_BELOW_S {
                "half-life under 5 years: the paper does not limit it (wastes are assumed isolated for at least 100 years)"
            } else {
                "not in Table 2 (Table 2 covers half-lives from 5 years to 10^12 years up to Cm-248, except Cm-247, Bk-247, Cf-249, Cf-250 and Cf-251); it adds nothing to the sum"
            };
            out.unlisted.push(FetterNote {
                nuclide: n.nuclide.clone(),
                activity_bq: activity,
                note: note.to_string(),
            });
            continue;
        };
        let Some(low) = limit.sal_low_ci_per_m3 else {
            out.no_limit.push(FetterNote {
                nuclide: n.nuclide.clone(),
                activity_bq: activity,
                note: "Table 2 gives the theoretical maximum specific activity (TMSA) as its limit: even the pure nuclide does not reach it".to_string(),
            });
            continue;
        };
        let c = activity / CURIE_BQ / volume;
        let upper = if limit.sal_high_is_tmsa {
            None
        } else {
            Some(limit.sal_high_ci_per_m3.unwrap_or(low))
        };
        let fraction = c / low;
        let fraction_at_upper = upper.map_or(0.0, |u| c / u);
        out.sum_at_lower += fraction;
        out.sum_at_upper += fraction_at_upper;
        out.lines.push(FetterLine {
            nuclide: n.nuclide.clone(),
            concentration_ci_per_m3: c,
            limit_ci_per_m3: low,
            fraction,
            upper_limit_ci_per_m3: upper,
            fraction_at_upper,
            quote: limit.quote.clone(),
        });
    }
    out.inventory_lists_every_limited_nuclide = table
        .limits
        .iter()
        .filter(|l| l.sal_low_ci_per_m3.is_some())
        .all(|l| inventory.nuclides.iter().any(|n| n.nuclide == l.nuclide));
    let (lo, hi) = (out.sum_at_lower, out.sum_at_upper);
    out.verdict = if !exceeds(lo, 1.0) {
        out.rule = format!("sum of fractions {lo:.6} at the lower limits does not exceed 1");
        FetterVerdict::AtOrBelowOne
    } else if !exceeds(hi, 1.0) {
        out.rule = format!(
            "sum of fractions {lo:.6} at the lower limits exceeds 1 and {hi:.6} at the upper values of the ranges does not: the result depends on the waste-form indices"
        );
        FetterVerdict::AtOrBelowOneAtUpperRangeOnly
    } else {
        out.rule = format!(
            "sum of fractions {lo:.6} at the lower limits and {hi:.6} at the upper values both exceed 1"
        );
        FetterVerdict::AboveOne
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::waste::NuclideActivity;
    use sha2::{Digest, Sha256};

    fn nuclide(name: &str, curie: f64, half_life_years: f64) -> NuclideActivity {
        NuclideActivity {
            nuclide: name.to_string(),
            activity_bq: curie * CURIE_BQ,
            half_life_s: half_life_years * crate::waste::YEAR_S,
            alpha_emitter: None,
            progeny_of: None,
        }
    }

    fn inventory(nuclides: Vec<NuclideActivity>) -> WasteInventory {
        WasteInventory {
            component_id: "fetter-test".to_string(),
            waste_form: WasteForm::ActivatedMetal,
            volume_m3: Some(1.0),
            mass_g: Some(1.0e6),
            inventory_time_years: 0.0,
            transuranics_evaluated: true,
            inventory_kind: "authored_test".to_string(),
            nuclides,
        }
    }

    fn normalised(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    // Verifies: ACT-033, SAFE-011
    #[test]
    fn every_row_is_a_quoted_line_of_the_stored_table_and_its_values_follow_from_the_quote() {
        let t = fetter();
        assert_eq!(t.limits.len(), 101);
        // The stored table text, line by line, with runs of blanks collapsed.
        let lines: Vec<String> = FETTER_SNAPSHOT_TXT.lines().map(normalised).collect();
        for row in &t.limits {
            assert!(
                lines.contains(&row.quote),
                "{} is not a line of the stored table: {}",
                row.nuclide,
                row.quote
            );
            // The quote reads: nuclide, half-life, SAL, other values.
            assert!(
                row.quote.starts_with(&format!(
                    "{} {} {}",
                    row.nuclide, row.half_life_text, row.sal_text
                )),
                "{}",
                row.quote
            );
            let parsed = |s: &str| -> Option<f64> {
                (s != "TMSA").then(|| s.parse::<f64>().expect("SAL number"))
            };
            let (low_text, high_text) = match row.sal_text.split_once(" - ") {
                Some((a, b)) => (a, Some(b)),
                None => (row.sal_text.as_str(), None),
            };
            assert_eq!(
                parsed(low_text),
                row.sal_low_ci_per_m3,
                "{} low",
                row.nuclide
            );
            assert_eq!(
                row.no_limit_tmsa,
                low_text == "TMSA",
                "{} tmsa",
                row.nuclide
            );
            assert_eq!(
                high_text.and_then(parsed),
                row.sal_high_ci_per_m3,
                "{} high",
                row.nuclide
            );
            assert_eq!(
                row.sal_high_is_tmsa,
                high_text == Some("TMSA"),
                "{} high tmsa",
                row.nuclide
            );
        }
        // No two rows for one nuclide.
        let mut names: Vec<&str> = t.limits.iter().map(|l| l.nuclide.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), t.limits.len());
    }

    // Verifies: ACT-033
    #[test]
    fn the_stored_text_matches_its_recorded_hash_and_the_source_is_recorded() {
        let t = fetter();
        let hash = format!("{:x}", Sha256::digest(FETTER_SNAPSHOT_TXT.as_bytes()));
        assert_eq!(hash, t.source.snapshot_sha256);
        assert_eq!(t.source.original_sha256.len(), 64);
        assert_eq!(t.source.retrieved.len(), 10);
        assert_eq!(t.source.kind, "primary");
        assert!(FETTER_SNAPSHOT_TXT.contains("SHA-256 of that PDF: "));
        assert!(FETTER_SNAPSHOT_TXT.contains(&t.source.original_sha256));
        assert!(FETTER_SNAPSHOT_TXT.contains("Fusion Engineering and Design 13 (1990)"));
    }

    // Verifies: SAFE-011
    #[test]
    fn the_nb94_limit_is_0_2_curie_per_cubic_metre_as_in_10_cfr_61_table_1() {
        let nb = fetter().limit("Nb-94").unwrap();
        assert_eq!(nb.sal_low_ci_per_m3, Some(0.2));
        assert_eq!(nb.sal_high_ci_per_m3, None);
        let nrc = crate::waste::tables::nrc();
        let nrc_nb = nrc
            .table1
            .iter()
            .find(|r| r.label == "Nb-94 in activated metal")
            .unwrap();
        assert_eq!(nrc_nb.unit, "Ci/m3");
        assert_eq!(nb.sal_low_ci_per_m3, Some(nrc_nb.limit));
        // The paper's own comparison column reads 0.2 (10 CFR 61) for Nb-94.
        assert!(nb.quote.ends_with("0.2 (10 CFR 61)"));
    }

    // Verifies: ACT-033
    #[test]
    fn hand_computed_sum_of_fractions_with_lower_and_upper_limits() {
        // 1 m3. Nb-94 0.1 Ci: 0.1/0.2 = 0.5 at either end. Tc-99 0.03 Ci: 0.03/0.06 = 0.5 at the
        // lower limit and 0.03/0.6 = 0.05 at the upper. Ag-108m 0.3 Ci: 0.3/3 = 0.1 at either end.
        let inv = inventory(vec![
            nuclide("Nb-94", 0.1, 20_000.0),
            nuclide("Tc-99", 0.03, 213_000.0),
            nuclide("Ag-108m", 0.3, 127.0),
        ]);
        let r = classify_fetter(&inv, 0.0);
        assert_eq!(r.regime, "fetter-specific-activity");
        assert!((r.sum_at_lower - 1.1).abs() < 1e-9, "{}", r.sum_at_lower);
        assert!((r.sum_at_upper - 0.65).abs() < 1e-9, "{}", r.sum_at_upper);
        assert_eq!(r.verdict, FetterVerdict::AtOrBelowOneAtUpperRangeOnly);
        assert_eq!(r.lines.len(), 3);
        // Without the silver the lower sum is exactly 1, which is not above 1.
        let inv = inventory(vec![
            nuclide("Nb-94", 0.1, 20_000.0),
            nuclide("Tc-99", 0.03, 213_000.0),
        ]);
        let r = classify_fetter(&inv, 0.0);
        assert!((r.sum_at_lower - 1.0).abs() < 1e-9);
        assert_eq!(r.verdict, FetterVerdict::AtOrBelowOne);
        // Nb-94 at 1 Ci/m3 is 5 times its limit at both ends.
        let r = classify_fetter(&inventory(vec![nuclide("Nb-94", 1.0, 20_000.0)]), 0.0);
        assert!((r.sum_at_lower - 5.0).abs() < 1e-9);
        assert_eq!(r.verdict, FetterVerdict::AboveOne);
        assert!(!r.inventory_lists_every_limited_nuclide);
    }

    // Verifies: ACT-033
    #[test]
    fn a_tmsa_limit_never_adds_and_an_unlisted_nuclide_is_reported() {
        let inv = inventory(vec![
            nuclide("Mn-53", 5.0, 3.7e6),
            nuclide("Co-60", 1.0, 5.27),
            nuclide("Fe-55", 1.0, 2.7),
        ]);
        let r = classify_fetter(&inv, 0.0);
        // Mn-53 is TMSA; Co-60 3.E+08 gives 1/3e8; Fe-55 (2.7 y) is not limited.
        assert_eq!(r.no_limit.len(), 1);
        assert_eq!(r.no_limit[0].nuclide, "Mn-53");
        assert!((r.sum_at_lower - 1.0 / 3.0e8).abs() < 1e-18);
        assert_eq!(r.unlisted.len(), 1);
        assert!(r.unlisted[0].note.contains("under 5 years"));
        assert_eq!(r.verdict, FetterVerdict::AtOrBelowOne);
    }

    // Verifies: ACT-033
    #[test]
    fn decay_to_the_disposal_time_halves_a_nuclide_per_half_life() {
        // Ag-108m 6 Ci with a 127 y half-life, disposed of 127 y later: 3 Ci/m3 against 3: 1.
        let mut inv = inventory(vec![nuclide("Ag-108m", 6.0, 127.0)]);
        inv.inventory_time_years = 0.0;
        let r = classify_fetter(&inv, 127.0);
        assert!((r.sum_at_lower - 1.0).abs() < 1e-9, "{}", r.sum_at_lower);
        assert_eq!(r.verdict, FetterVerdict::AtOrBelowOne);
    }

    // Verifies: ACT-033
    #[test]
    fn a_form_a_volume_or_a_time_that_cannot_be_used_is_refused() {
        let mut inv = inventory(vec![nuclide("Nb-94", 0.1, 20_000.0)]);
        inv.waste_form = WasteForm::Other;
        let r = classify_fetter(&inv, 0.0);
        assert_eq!(r.verdict, FetterVerdict::NotEvaluated);
        assert!(
            r.not_evaluated
                .unwrap()
                .missing
                .contains(&"waste_form".to_string())
        );
        let mut inv = inventory(vec![nuclide("Nb-94", 0.1, 20_000.0)]);
        inv.volume_m3 = None;
        let r = classify_fetter(&inv, 0.0);
        assert_eq!(
            r.not_evaluated.unwrap().missing,
            vec!["volume_m3".to_string()]
        );
        let inv = inventory(vec![nuclide("Nb-94", 0.1, 20_000.0)]);
        let r = classify_fetter(&inv, -1.0);
        assert_eq!(r.verdict, FetterVerdict::NotEvaluated);
    }
}
