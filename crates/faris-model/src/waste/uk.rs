//! UK waste thresholds and UK out-of-scope concentrations (ACT-033, SAFE-011, ACT-036).
//!
//! Two schemes, each with its values read from a retrieved primary source:
//!
//! - `uk-llw-ilw`: the UK definitions of low level waste (LLW, not exceeding 4 GBq/te alpha or
//!   12 GBq/te beta/gamma) and of high-volume very low level waste (VLLW, 4 MBq/te of total
//!   activity, 40 MBq/te for tritium), from the 2016 UK strategy and the 2007 policy
//!   (`data/waste/uk_llw_ilw.json`, quotes checked against `uk_llw_policy_extracts.txt`).
//!   Intermediate level waste is defined as waste above the upper LLW boundaries that needs no
//!   heat allowance; high level waste by a heat criterion that has no number. So the result above
//!   LLW is `ABOVE_LLW_ILW_OR_HLW`, and FARIS does not separate ILW from HLW. The low-volume VLLW
//!   limits are per 0.1 m3 and per item and are not evaluated.
//! - `uk-epr2016-sch23-table2`: the concentrations of Schedule 23 Part 3 Table 2 of the
//!   Environmental Permitting (England and Wales) Regulations 2016 above which a solid is
//!   radioactive waste (`uk_epr2016_sch23_table2.json`, checked row for row against
//!   `uk_epr2016_sch23_part3.xml`). At or below the summation rule's 1 the material is out of
//!   scope of the radioactive-waste provisions, as far as these values go.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::{
    BOUNDARY_TOLERANCE, NotEvaluated, ResultKind, SchemeInfo, WasteInventory, decayed_activity_bq,
    validate_inventory,
};
use crate::RESEARCH_SCREENING_STATEMENT;

const UK_LLW_JSON: &str = include_str!("../../data/waste/uk_llw_ilw.json");
/// The verbatim policy extracts the LLW and VLLW values were read from.
pub const UK_POLICY_SNAPSHOT_TXT: &str =
    include_str!("../../data/waste/uk_llw_policy_extracts.txt");
const UK_CLEARANCE_JSON: &str = include_str!("../../data/waste/uk_epr2016_sch23_table2.json");
/// The stored legislation.gov.uk text of Schedule 23 Part 3.
pub const UK_SCHEDULE_SNAPSHOT_XML: &str =
    include_str!("../../data/waste/uk_epr2016_sch23_part3.xml");

/// Years in the 1 GBq per tonne unit conversion: 1 GBq/te = 1000 Bq/g.
const BQ_PER_G_PER_GBQ_PER_TE: f64 = 1000.0;
/// 1 MBq/te = 1 Bq/g.
const BQ_PER_G_PER_MBQ_PER_TE: f64 = 1.0;

// ------------------------------------------------------------------ LLW / VLLW

#[derive(Clone, Debug, Deserialize)]
pub struct PolicySource {
    pub tag: String,
    pub title: String,
    pub kind: String,
    pub url: String,
    pub retrieved: String,
    pub original_sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LlwBoundary {
    pub alpha_gbq_per_te: f64,
    pub beta_gamma_gbq_per_te: f64,
    pub quote: String,
    pub source_tag: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct VllwHighVolume {
    pub total_mbq_per_te: f64,
    pub tritium_mbq_per_te: f64,
    pub quote: String,
    pub tritium_quote: String,
    pub source_tag: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct VllwLowVolume {
    pub per_0_1_m3_kbq: f64,
    pub per_item_kbq: f64,
    pub per_0_1_m3_c14_h3_kbq: f64,
    pub per_item_c14_h3_kbq: f64,
    pub quote: String,
    pub source_tag: String,
    pub evaluated_by_faris: bool,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct QualitativeDefinition {
    pub quote: String,
    pub source_tag: String,
    #[serde(default)]
    pub quantitative: Option<bool>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UkWasteTables {
    pub scheme_id: String,
    pub scheme_name: String,
    pub scheme_version: String,
    pub scheme_date: String,
    pub sources: Vec<PolicySource>,
    pub snapshot_file: String,
    pub snapshot_sha256: String,
    pub licence: String,
    pub llw: LlwBoundary,
    pub vllw_high_volume: VllwHighVolume,
    pub vllw_low_volume: VllwLowVolume,
    pub ilw: QualitativeDefinition,
    pub hlw: QualitativeDefinition,
}

impl UkWasteTables {
    pub fn scheme_info(&self) -> SchemeInfo {
        let sources = self
            .sources
            .iter()
            .map(|s| format!("{} ({}), retrieved {}", s.title, s.url, s.retrieved))
            .collect::<Vec<_>>()
            .join("; ");
        SchemeInfo {
            id: self.scheme_id.clone(),
            name: self.scheme_name.clone(),
            version: self.scheme_version.clone(),
            date: self.scheme_date.clone(),
            citation: sources,
            status: "IMPLEMENTED".to_string(),
            status_reason: None,
        }
    }

    /// Retrieval date of the newest source.
    pub fn retrieved(&self) -> &str {
        self.sources
            .iter()
            .map(|s| s.retrieved.as_str())
            .max()
            .unwrap_or("")
    }
}

pub fn uk_waste() -> &'static UkWasteTables {
    static TABLES: OnceLock<UkWasteTables> = OnceLock::new();
    TABLES.get_or_init(|| {
        serde_json::from_str(UK_LLW_JSON).expect("the embedded UK LLW file is valid")
    })
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub enum UkCategory {
    /// High-volume very low level waste (a sub-category of LLW).
    #[serde(rename = "VLLW_HIGH_VOLUME")]
    VllwHighVolume,
    #[serde(rename = "LLW")]
    Llw,
    /// Above an LLW boundary: intermediate or high level waste. The heat criterion that
    /// separates them has no number and is not evaluated.
    #[serde(rename = "ABOVE_LLW_ILW_OR_HLW")]
    AboveLlw,
    #[serde(rename = "NOT_EVALUATED")]
    NotEvaluated,
}

impl UkCategory {
    pub fn text(self) -> &'static str {
        match self {
            UkCategory::VllwHighVolume => "VLLW_HIGH_VOLUME",
            UkCategory::Llw => "LLW",
            UkCategory::AboveLlw => "ABOVE_LLW_ILW_OR_HLW",
            UkCategory::NotEvaluated => "NOT_EVALUATED",
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct UkNuclideLine {
    pub nuclide: String,
    /// `alpha` or `beta_gamma`, from the inventory's `alpha_emitter` flag.
    pub emission: String,
    pub activity_bq: f64,
    pub concentration_bq_per_g: f64,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct UkCriterion {
    pub name: String,
    pub value: f64,
    pub limit: f64,
    pub unit: String,
    pub fraction: f64,
    pub met: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct UkNotEvaluatedCriterion {
    pub name: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct UkWasteResult {
    pub scheme: SchemeInfo,
    /// The regime named on the class: the scheme identifier.
    pub regime: String,
    pub verdict: UkCategory,
    pub component_id: String,
    pub disposal_time_years: f64,
    pub inventory_time_years: f64,
    pub decay_treatment: String,
    pub mass_g: Option<f64>,
    pub lines: Vec<UkNuclideLine>,
    pub alpha_gbq_per_te: f64,
    pub beta_gamma_gbq_per_te: f64,
    pub criteria: Vec<UkCriterion>,
    pub criteria_not_evaluated: Vec<UkNotEvaluatedCriterion>,
    pub rule: String,
    pub interpretation_note: String,
    pub not_evaluated: Option<NotEvaluated>,
    pub kind: ResultKind,
    pub inventory_kind: String,
    pub research_screening: String,
}

fn exceeds(value: f64, bound: f64) -> bool {
    value > bound * (1.0 + BOUNDARY_TOLERANCE)
}

const INTERPRETATION: &str = "The LLW definition is read as two limits that both hold: alpha at most 4 GBq/te and beta/gamma at most 12 GBq/te, each over the activity of its own emission type. Every listed nuclide counts at its full decayed activity, decay products listed as progeny included, because the definitions are of the radioactive content. The high-volume VLLW test reads the 4 MBq/te total and the 40 MBq/te tritium limits as a sum of fractions (non-tritium over 4, tritium over 40) not above 1; the text states each limit alone and gives no rule for a mixture.";

/// Classifies one component under the UK LLW definition at a disposal time.
pub fn classify_uk_waste(inventory: &WasteInventory, disposal_time_years: f64) -> UkWasteResult {
    let t = uk_waste();
    let mut out = UkWasteResult {
        scheme: t.scheme_info(),
        regime: t.scheme_id.clone(),
        verdict: UkCategory::NotEvaluated,
        component_id: inventory.component_id.clone(),
        disposal_time_years,
        inventory_time_years: inventory.inventory_time_years,
        decay_treatment: "each nuclide decayed from the inventory time to the disposal time by its own half-life; no ingrowth from parents (supply the inventory at the disposal time where ingrowth matters)".to_string(),
        mass_g: inventory.mass_g,
        lines: Vec::new(),
        alpha_gbq_per_te: 0.0,
        beta_gamma_gbq_per_te: 0.0,
        criteria: Vec::new(),
        criteria_not_evaluated: vec![
            UkNotEvaluatedCriterion {
                name: "low-volume VLLW (per 0.1 m3 and per item)".to_string(),
                reason: t.vllw_low_volume.reason.clone(),
            },
            UkNotEvaluatedCriterion {
                name: "ILW against HLW (heat)".to_string(),
                reason: t.hlw.reason.clone().unwrap_or_default(),
            },
        ],
        rule: "NOT_EVALUATED: the reason and the next step are in the result".to_string(),
        interpretation_note: INTERPRETATION.to_string(),
        not_evaluated: None,
        kind: ResultKind::Calculated,
        inventory_kind: inventory.inventory_kind.clone(),
        research_screening: RESEARCH_SCREENING_STATEMENT.to_string(),
    };
    if let Err(e) = fill_uk_waste(inventory, disposal_time_years, &mut out) {
        out.verdict = UkCategory::NotEvaluated;
        out.not_evaluated = Some(e);
    }
    out
}

fn fill_uk_waste(
    inventory: &WasteInventory,
    disposal_time_years: f64,
    out: &mut UkWasteResult,
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
    let mass = inventory.mass_g.ok_or_else(|| {
        NotEvaluated::new(
            "the UK limits are activity per tonne and the inventory gives no mass",
            vec!["mass_g".to_string()],
            "Give mass_g, the mass the activity is averaged over.",
        )
    })?;
    let dt = disposal_time_years - inventory.inventory_time_years;
    let no_flag: Vec<String> = inventory
        .nuclides
        .iter()
        .filter(|n| {
            n.alpha_emitter.is_none() && decayed_activity_bq(n.activity_bq, n.half_life_s, dt) > 0.0
        })
        .map(|n| n.nuclide.clone())
        .collect();
    if !no_flag.is_empty() {
        return Err(NotEvaluated::new(
            "the UK limits differ for alpha and for beta/gamma activity, and these nuclides with activity have no alpha_emitter flag (a missing flag is not taken as beta/gamma)",
            no_flag,
            "Set alpha_emitter to true or false for each from the decay data.",
        ));
    }
    let t = uk_waste();
    let (mut alpha, mut beta_gamma, mut tritium, mut other) = (0.0, 0.0, 0.0, 0.0);
    for n in &inventory.nuclides {
        let activity = decayed_activity_bq(n.activity_bq, n.half_life_s, dt);
        if activity <= 0.0 {
            continue;
        }
        let bq_per_g = activity / mass;
        let is_alpha = n.alpha_emitter == Some(true);
        if is_alpha {
            alpha += bq_per_g;
        } else {
            beta_gamma += bq_per_g;
        }
        if n.nuclide == "H-3" {
            tritium += bq_per_g;
        } else {
            other += bq_per_g;
        }
        out.lines.push(UkNuclideLine {
            nuclide: n.nuclide.clone(),
            emission: if is_alpha { "alpha" } else { "beta_gamma" }.to_string(),
            activity_bq: activity,
            concentration_bq_per_g: bq_per_g,
        });
    }
    out.alpha_gbq_per_te = alpha / BQ_PER_G_PER_GBQ_PER_TE;
    out.beta_gamma_gbq_per_te = beta_gamma / BQ_PER_G_PER_GBQ_PER_TE;
    let criterion = |name: &str, value: f64, limit: f64, unit: &str| UkCriterion {
        name: name.to_string(),
        value,
        limit,
        unit: unit.to_string(),
        fraction: value / limit,
        met: !exceeds(value, limit),
    };
    let alpha_c = criterion(
        "LLW alpha",
        out.alpha_gbq_per_te,
        t.llw.alpha_gbq_per_te,
        "GBq/te",
    );
    let bg_c = criterion(
        "LLW beta/gamma",
        out.beta_gamma_gbq_per_te,
        t.llw.beta_gamma_gbq_per_te,
        "GBq/te",
    );
    let h3_fraction = tritium / (t.vllw_high_volume.tritium_mbq_per_te * BQ_PER_G_PER_MBQ_PER_TE);
    let other_fraction = other / (t.vllw_high_volume.total_mbq_per_te * BQ_PER_G_PER_MBQ_PER_TE);
    let vllw_sum = h3_fraction + other_fraction;
    let vllw_c = UkCriterion {
        name: "high-volume VLLW (non-tritium over 4 MBq/te plus tritium over 40 MBq/te)"
            .to_string(),
        value: vllw_sum,
        limit: 1.0,
        unit: "sum of fractions".to_string(),
        fraction: vllw_sum,
        met: !exceeds(vllw_sum, 1.0),
    };
    let llw = alpha_c.met && bg_c.met;
    let vllw = vllw_c.met;
    out.rule = if llw && vllw {
        format!(
            "alpha {:.6} GBq/te (limit {}) and beta/gamma {:.6} GBq/te (limit {}) do not exceed the LLW limits, and the high-volume VLLW sum {:.6} does not exceed 1",
            alpha_c.value,
            t.llw.alpha_gbq_per_te,
            bg_c.value,
            t.llw.beta_gamma_gbq_per_te,
            vllw_sum
        )
    } else if llw {
        format!(
            "alpha {:.6} GBq/te (limit {}) and beta/gamma {:.6} GBq/te (limit {}) do not exceed the LLW limits; the high-volume VLLW sum {:.6} exceeds 1",
            alpha_c.value,
            t.llw.alpha_gbq_per_te,
            bg_c.value,
            t.llw.beta_gamma_gbq_per_te,
            vllw_sum
        )
    } else {
        format!(
            "alpha {:.6} GBq/te (limit {}) or beta/gamma {:.6} GBq/te (limit {}) exceeds an LLW limit",
            alpha_c.value, t.llw.alpha_gbq_per_te, bg_c.value, t.llw.beta_gamma_gbq_per_te
        )
    };
    out.verdict = match (llw, vllw) {
        (true, true) => UkCategory::VllwHighVolume,
        (true, false) => UkCategory::Llw,
        _ => UkCategory::AboveLlw,
    };
    out.criteria = vec![alpha_c, bg_c, vllw_c];
    Ok(())
}

// ------------------------------------------------------------------ out of scope (Schedule 23)

#[derive(Clone, Debug, Deserialize)]
pub struct ScheduleSource {
    pub title: String,
    pub kind: String,
    pub url: String,
    pub data_url: String,
    pub retrieved: String,
    pub original_sha256: String,
    pub snapshot_file: String,
    pub snapshot_sha256: String,
    pub licence: String,
    pub redistribution: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UkLevel {
    /// First cell of the Table 2 row: `Co-60`, `Sr-90+`, `U-238sec`.
    pub label: String,
    pub nuclide: String,
    /// `+` (parent in secular equilibrium with the Table 3 daughters), `sec` or empty.
    pub suffix: String,
    pub bq_per_g: f64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DaughterRow {
    pub parent: String,
    pub daughters: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UkClearanceTables {
    pub scheme_id: String,
    pub scheme_name: String,
    pub scheme_version: String,
    pub scheme_date: String,
    pub source: ScheduleSource,
    pub rule_quote: String,
    pub levels: Vec<UkLevel>,
    pub table3: Vec<DaughterRow>,
}

impl UkClearanceTables {
    pub fn scheme_info(&self) -> SchemeInfo {
        SchemeInfo {
            id: self.scheme_id.clone(),
            name: self.scheme_name.clone(),
            version: self.scheme_version.clone(),
            date: self.scheme_date.clone(),
            citation: format!(
                "{} ({}), retrieved {}",
                self.source.title, self.source.url, self.source.retrieved
            ),
            status: "IMPLEMENTED".to_string(),
            status_reason: None,
        }
    }

    fn row(&self, nuclide: &str, suffix: &str) -> Option<&UkLevel> {
        self.levels
            .iter()
            .find(|l| l.nuclide == nuclide && l.suffix == suffix)
    }

    /// The daughters Table 3 names for a `+` parent.
    pub fn daughters_of(&self, nuclide: &str) -> Option<&[String]> {
        let label = format!("{nuclide}+");
        self.table3
            .iter()
            .find(|r| r.parent == label)
            .map(|r| r.daughters.as_slice())
    }

    /// The row that applies to a nuclide. Where a nuclide has both a plain row and a `+` row, the
    /// `+` row (which already includes the daughters) applies when the inventory lists a
    /// daughter as that nuclide's progeny, and the plain row otherwise. `sec` rows name whole
    /// chains and never match one nuclide.
    pub fn level_for(&self, nuclide: &str, progeny_listed: bool) -> Option<&UkLevel> {
        match (self.row(nuclide, ""), self.row(nuclide, "+")) {
            (Some(plain), Some(plus)) => Some(if progeny_listed { plus } else { plain }),
            (Some(row), None) | (None, Some(row)) => Some(row),
            (None, None) => None,
        }
    }
}

pub fn uk_clearance() -> &'static UkClearanceTables {
    static TABLES: OnceLock<UkClearanceTables> = OnceLock::new();
    TABLES.get_or_init(|| {
        serde_json::from_str(UK_CLEARANCE_JSON)
            .expect("the embedded UK Schedule 23 table file is valid")
    })
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct UkClearanceLine {
    pub nuclide: String,
    pub table_row: String,
    pub concentration_bq_per_g: f64,
    pub level_bq_per_g: f64,
    pub ratio: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct UkClearanceResult {
    pub scheme: SchemeInfo,
    /// The regime named on the class: the scheme identifier.
    pub regime: String,
    pub component_id: String,
    pub time_years: f64,
    /// Sum of the ratios; `None` when it could not be formed.
    pub index: Option<f64>,
    /// True when the sum does not exceed 1: the concentration does not exceed the Table 2 values.
    pub at_or_below_one: Option<bool>,
    /// `OUT_OF_SCOPE_BY_TABLE_2`, `ABOVE_TABLE_2` or `NOT_EVALUATED`.
    pub class: String,
    pub lines: Vec<UkClearanceLine>,
    pub progeny_not_counted: Vec<String>,
    pub rule: String,
    pub scope_note: String,
    pub not_evaluated: Option<NotEvaluated>,
    pub kind: ResultKind,
    pub inventory_kind: String,
    pub research_screening: String,
}

const UK_SCOPE_NOTE: &str = "Schedule 23 applies in England and Wales. Table 2 values apply to solids and relevant liquids that contain radionuclides not of natural terrestrial or cosmic origin (paragraph 6); the summation rule calls the values exceeded when the sum is greater than 1, so a sum of exactly 1 is not above. FARIS does not apply the paragraph 6A rule on dilution or the paragraph 7 rule on half-lives of at most 100 seconds. A nuclide with no Table 2 row falls under the schedule's dose criterion, which FARIS does not compute.";

/// The UK (England and Wales) Table 2 sum of ratios for one component at a time.
pub fn uk_clearance_index(inventory: &WasteInventory, time_years: f64) -> UkClearanceResult {
    let t = uk_clearance();
    let mut out = UkClearanceResult {
        scheme: t.scheme_info(),
        regime: t.scheme_id.clone(),
        component_id: inventory.component_id.clone(),
        time_years,
        index: None,
        at_or_below_one: None,
        class: "NOT_EVALUATED".to_string(),
        lines: Vec::new(),
        progeny_not_counted: Vec::new(),
        rule: "NOT_EVALUATED: the reason and the next step are in the result".to_string(),
        scope_note: UK_SCOPE_NOTE.to_string(),
        not_evaluated: None,
        kind: ResultKind::Calculated,
        inventory_kind: inventory.inventory_kind.clone(),
        research_screening: RESEARCH_SCREENING_STATEMENT.to_string(),
    };
    match uk_index_lines(inventory, time_years) {
        Ok((lines, progeny)) => {
            let index: f64 = lines.iter().map(|l| l.ratio).sum();
            let within = !exceeds(index, 1.0);
            out.index = Some(index);
            out.at_or_below_one = Some(within);
            out.class = if within {
                "OUT_OF_SCOPE_BY_TABLE_2"
            } else {
                "ABOVE_TABLE_2"
            }
            .to_string();
            out.rule = format!(
                "sum of concentration over Table 2 value is {index:.6}; the values are exceeded only when the sum is greater than 1"
            );
            out.lines = lines;
            out.progeny_not_counted = progeny;
        }
        Err(e) => out.not_evaluated = Some(e),
    }
    out
}

fn uk_index_lines(
    inventory: &WasteInventory,
    time_years: f64,
) -> Result<(Vec<UkClearanceLine>, Vec<String>), NotEvaluated> {
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
            "Table 2 values are activity per gram and the inventory gives no mass",
            vec!["mass_g".to_string()],
            "Give mass_g, the mass the activity is averaged over.",
        )
    })?;
    let t = uk_clearance();
    let dt = time_years - inventory.inventory_time_years;
    let mut lines = Vec::new();
    let mut progeny = Vec::new();
    let mut no_level: Vec<String> = Vec::new();
    for n in &inventory.nuclides {
        let activity = decayed_activity_bq(n.activity_bq, n.half_life_s, dt);
        if let Some(parent) = &n.progeny_of {
            let named = t
                .daughters_of(parent)
                .is_some_and(|d| d.iter().any(|x| x == &n.nuclide))
                && inventory.nuclides.iter().any(|o| &o.nuclide == parent);
            if !named {
                return Err(NotEvaluated::new(
                    format!(
                        "{} is declared a progeny of {parent}, but Table 3 does not name it as a daughter of a `{parent}+` parent",
                        n.nuclide
                    ),
                    vec![n.nuclide.clone()],
                    "Remove progeny_of for this nuclide so it needs its own Table 2 value, or correct the parent.",
                ));
            }
            progeny.push(n.nuclide.clone());
            continue;
        }
        if activity <= 0.0 {
            continue;
        }
        let progeny_listed = inventory
            .nuclides
            .iter()
            .any(|o| o.progeny_of.as_deref() == Some(n.nuclide.as_str()));
        match t.level_for(&n.nuclide, progeny_listed) {
            Some(level) => {
                let c = activity / mass;
                lines.push(UkClearanceLine {
                    nuclide: n.nuclide.clone(),
                    table_row: level.label.clone(),
                    concentration_bq_per_g: c,
                    level_bq_per_g: level.bq_per_g,
                    ratio: c / level.bq_per_g,
                });
            }
            None => no_level.push(n.nuclide.clone()),
        }
    }
    if !no_level.is_empty() {
        return Err(NotEvaluated::new(
            "these nuclides have activity and no row in Table 2; the schedule then sets the value by a 10 microsievert per year dose calculation that FARIS does not do, so the sum cannot be formed (a missing value is not zero)",
            no_level,
            "Supply a dose-based value as an authored input, or use a clearance table that lists the nuclide.",
        ));
    }
    Ok((lines, progeny))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::waste::{CURIE_BQ, NuclideActivity, WasteForm, YEAR_S};
    use sha2::{Digest, Sha256};

    fn normalised(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn nuclide(name: &str, bq: f64, alpha: Option<bool>) -> NuclideActivity {
        NuclideActivity {
            nuclide: name.to_string(),
            activity_bq: bq,
            half_life_s: 1.0e6 * YEAR_S,
            alpha_emitter: alpha,
            progeny_of: None,
        }
    }

    fn inventory(mass_g: f64, nuclides: Vec<NuclideActivity>) -> WasteInventory {
        WasteInventory {
            component_id: "uk-test".to_string(),
            waste_form: WasteForm::Other,
            volume_m3: Some(1.0),
            mass_g: Some(mass_g),
            inventory_time_years: 0.0,
            transuranics_evaluated: true,
            inventory_kind: "authored_test".to_string(),
            nuclides,
        }
    }

    // Verifies: ACT-033, SAFE-011
    #[test]
    fn the_policy_values_and_quotes_are_in_the_stored_extracts() {
        let t = uk_waste();
        let text = normalised(UK_POLICY_SNAPSHOT_TXT);
        for quote in [
            &t.llw.quote,
            &t.vllw_high_volume.quote,
            &t.vllw_high_volume.tritium_quote,
            &t.vllw_low_volume.quote,
            &t.ilw.quote,
            &t.hlw.quote,
        ] {
            assert!(text.contains(&normalised(quote)), "missing: {quote}");
        }
        // The numbers are the ones in the quotes.
        assert_eq!(t.llw.alpha_gbq_per_te, 4.0);
        assert_eq!(t.llw.beta_gamma_gbq_per_te, 12.0);
        assert!(t.llw.quote.contains(
            "four gigabecquerels per tonne (GBq/te) of alpha or 12 GBq/te of beta/gamma"
        ));
        assert_eq!(t.vllw_high_volume.total_mbq_per_te, 4.0);
        assert!(
            t.vllw_high_volume
                .quote
                .contains("four megabecquerels per tonne (MBq/te)")
        );
        assert_eq!(t.vllw_high_volume.tritium_mbq_per_te, 40.0);
        assert!(t.vllw_high_volume.tritium_quote.contains("40MBq/te"));
        assert_eq!(t.vllw_low_volume.per_0_1_m3_kbq, 400.0);
        assert_eq!(t.vllw_low_volume.per_item_kbq, 40.0);
        assert!(!t.vllw_low_volume.evaluated_by_faris);
        assert_eq!(t.hlw.quantitative, Some(false));
        // The same LLW definition is in both documents.
        let two = text
            .matches("four gigabecquerels per tonne (GBq/te) of alpha or 12 GBq/te of beta/gamma")
            .count();
        assert_eq!(two, 2);
    }

    // Verifies: ACT-033
    #[test]
    fn the_policy_extracts_match_their_hash_and_name_both_sources_with_hashes() {
        let t = uk_waste();
        let hash = format!("{:x}", Sha256::digest(UK_POLICY_SNAPSHOT_TXT.as_bytes()));
        assert_eq!(hash, t.snapshot_sha256);
        assert_eq!(t.sources.len(), 2);
        for s in &t.sources {
            assert_eq!(s.original_sha256.len(), 64);
            assert_eq!(s.retrieved.len(), 10);
            assert_eq!(s.kind, "primary");
            assert!(
                s.url
                    .starts_with("https://assets.publishing.service.gov.uk/")
            );
            assert!(
                UK_POLICY_SNAPSHOT_TXT.contains(&s.original_sha256),
                "{}",
                s.tag
            );
            assert!(UK_POLICY_SNAPSHOT_TXT.contains(&s.url), "{}", s.tag);
        }
    }

    // Verifies: ACT-033
    #[test]
    fn hand_computed_llw_boundaries() {
        let mass = 1.0e6; // 1 tonne, so 1 Bq/g = 1 MBq/te
        // 4 GBq of alpha and 12 GBq of beta/gamma in one tonne: exactly on both LLW limits.
        // The VLLW test: 12 GBq/te is 3000 times 4 MBq/te, so above VLLW, still LLW.
        let on = inventory(
            mass,
            vec![
                nuclide("Am-241", 4.0e9, Some(true)),
                nuclide("Co-60", 12.0e9, Some(false)),
            ],
        );
        let r = classify_uk_waste(&on, 0.0);
        assert_eq!(r.regime, "uk-llw-ilw");
        assert!((r.alpha_gbq_per_te - 4.0).abs() < 1e-9);
        assert!((r.beta_gamma_gbq_per_te - 12.0).abs() < 1e-9);
        assert_eq!(r.verdict, UkCategory::Llw);
        // 1 percent more beta/gamma is above LLW.
        let over = inventory(
            mass,
            vec![
                nuclide("Am-241", 4.0e9, Some(true)),
                nuclide("Co-60", 12.12e9, Some(false)),
            ],
        );
        assert_eq!(classify_uk_waste(&over, 0.0).verdict, UkCategory::AboveLlw);
        // Alpha alone above 4 GBq/te is above LLW although the beta/gamma is far below.
        let alpha = inventory(
            mass,
            vec![
                nuclide("Pu-239", 5.0e9, Some(true)),
                nuclide("Co-60", 1.0e9, Some(false)),
            ],
        );
        let r = classify_uk_waste(&alpha, 0.0);
        assert_eq!(r.verdict, UkCategory::AboveLlw);
        assert!(!r.criteria[0].met && r.criteria[1].met);
    }

    // Verifies: ACT-033
    #[test]
    fn hand_computed_high_volume_vllw_with_and_without_tritium() {
        let mass = 1.0e6;
        // 3 MBq of Co-60 in a tonne: 3/4 = 0.75; 20 MBq of H-3: 20/40 = 0.5. Sum 1.25: LLW.
        let mix = inventory(
            mass,
            vec![
                nuclide("Co-60", 3.0e6, Some(false)),
                nuclide("H-3", 20.0e6, Some(false)),
            ],
        );
        let r = classify_uk_waste(&mix, 0.0);
        assert!((r.criteria[2].value - 1.25).abs() < 1e-9);
        assert_eq!(r.verdict, UkCategory::Llw);
        // 20 MBq of H-3 alone: 0.5, VLLW. 4 MBq of Co-60 alone: exactly 1, still VLLW.
        let h3 = inventory(mass, vec![nuclide("H-3", 20.0e6, Some(false))]);
        assert_eq!(
            classify_uk_waste(&h3, 0.0).verdict,
            UkCategory::VllwHighVolume
        );
        let co = inventory(mass, vec![nuclide("Co-60", 4.0e6, Some(false))]);
        assert_eq!(
            classify_uk_waste(&co, 0.0).verdict,
            UkCategory::VllwHighVolume
        );
        // 40 MBq of H-3 and nothing else is on the tritium limit.
        let h3_on = inventory(mass, vec![nuclide("H-3", 40.0e6, Some(false))]);
        assert_eq!(
            classify_uk_waste(&h3_on, 0.0).verdict,
            UkCategory::VllwHighVolume
        );
        // 5 tonnes dilute the same activity: 3 MBq + 20 MBq over 5 te is 0.6/4 + 4/40 = 0.25.
        let five = inventory(
            5.0 * mass,
            vec![
                nuclide("Co-60", 3.0e6, Some(false)),
                nuclide("H-3", 20.0e6, Some(false)),
            ],
        );
        let r = classify_uk_waste(&five, 0.0);
        assert!(
            (r.criteria[2].value - 0.25).abs() < 1e-9,
            "{}",
            r.criteria[2].value
        );
        assert_eq!(r.verdict, UkCategory::VllwHighVolume);
        assert_eq!(r.criteria_not_evaluated.len(), 2);
    }

    // Verifies: ACT-033
    #[test]
    fn uk_waste_refuses_a_missing_flag_or_mass_and_decays_to_the_disposal_time() {
        let mut inv = inventory(1.0e6, vec![nuclide("Co-60", 1.0e9, None)]);
        let r = classify_uk_waste(&inv, 0.0);
        assert_eq!(r.verdict, UkCategory::NotEvaluated);
        assert_eq!(r.not_evaluated.unwrap().missing, vec!["Co-60".to_string()]);
        inv.nuclides[0].alpha_emitter = Some(false);
        inv.mass_g = None;
        let r = classify_uk_waste(&inv, 0.0);
        assert_eq!(r.not_evaluated.unwrap().missing, vec!["mass_g".to_string()]);
        // 24 GBq of a nuclide with a 10 year half-life is 12 GBq after 10 years: on the limit.
        let mut decay = inventory(1.0e6, vec![nuclide("Cs-137", 24.0e9, Some(false))]);
        decay.nuclides[0].half_life_s = 10.0 * YEAR_S;
        assert_eq!(classify_uk_waste(&decay, 0.0).verdict, UkCategory::AboveLlw);
        assert_eq!(classify_uk_waste(&decay, 10.0).verdict, UkCategory::Llw);
    }

    // The table in the stored schedule, as rows of cell texts with superscripts as `^n`.
    fn snapshot_table2_rows() -> Vec<Vec<String>> {
        let xml = UK_SCHEDULE_SNAPSHOT_XML;
        let start = xml
            .find("Concentration in becquerels per gram (Bq/g)")
            .unwrap();
        let body = &xml[start..];
        let body = &body[..body.find("</xhtml:table>").unwrap()];
        body.split("<xhtml:tr>")
            .skip(1)
            .map(|row| {
                row.split("<xhtml:td>")
                    .skip(1)
                    .map(|cell| cell_text(&cell[..cell.find("</xhtml:td>").unwrap()]))
                    .collect()
            })
            .collect()
    }

    fn cell_text(markup: &str) -> String {
        let marked = markup.replace("<Superior>", "^").replace("</Superior>", "");
        let mut out = String::new();
        let mut in_tag = false;
        for c in marked.chars() {
            match c {
                '<' => in_tag = true,
                '>' => in_tag = false,
                c if !in_tag => out.push(c),
                _ => {}
            }
        }
        out.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .replace("^ ", "^")
    }

    fn cell_value(cell: &str) -> f64 {
        match cell.split_once('^') {
            Some((base, exp)) => {
                assert_eq!(base, "10", "{cell}");
                crate::math::powi(10.0, exp.parse().unwrap())
            }
            None => cell.parse().unwrap(),
        }
    }

    // Verifies: ACT-036, ACT-033
    #[test]
    fn table2_equals_the_stored_schedule_row_for_row() {
        let t = uk_clearance();
        let rows: Vec<Vec<String>> = snapshot_table2_rows()
            .into_iter()
            .filter(|r| r.len() == 2 && r[0].chars().next().is_some_and(|c| c.is_ascii_uppercase()))
            .filter(|r| {
                r[0].split('-')
                    .nth(1)
                    .is_some_and(|d| d.starts_with(|c: char| c.is_ascii_digit()))
            })
            .collect();
        assert_eq!(rows.len(), t.levels.len());
        assert_eq!(rows.len(), 284);
        for (cells, level) in rows.iter().zip(&t.levels) {
            assert_eq!(cells[0], level.label);
            assert_eq!(cell_value(&cells[1]), level.bq_per_g, "{}", level.label);
            let suffix = if level.label.ends_with('+') {
                "+"
            } else if level.label.ends_with("sec") {
                "sec"
            } else {
                ""
            };
            assert_eq!(level.suffix, suffix);
            assert_eq!(format!("{}{}", level.nuclide, suffix), level.label);
        }
    }

    // Verifies: ACT-036
    #[test]
    fn table3_daughters_equal_the_stored_schedule_and_the_hash_and_date_are_recorded() {
        let t = uk_clearance();
        let hash = format!("{:x}", Sha256::digest(UK_SCHEDULE_SNAPSHOT_XML.as_bytes()));
        assert_eq!(hash, t.source.snapshot_sha256);
        assert_eq!(t.source.retrieved.len(), 10);
        assert_eq!(t.source.original_sha256.len(), 64);
        assert!(UK_SCHEDULE_SNAPSHOT_XML.contains(&t.source.original_sha256));
        assert!(t.source.url.starts_with("https://www.legislation.gov.uk/"));
        let xml = UK_SCHEDULE_SNAPSHOT_XML;
        let start = xml.find("Parent radionuclide").unwrap();
        let body = &xml[start..];
        let body = &body[..body.find("</xhtml:table>").unwrap()];
        let rows: Vec<Vec<String>> = body
            .split("<xhtml:tr>")
            .skip(1)
            .map(|row| {
                row.split("<xhtml:td>")
                    .skip(1)
                    .map(|cell| cell_text(&cell[..cell.find("</xhtml:td>").unwrap()]))
                    .collect()
            })
            .collect();
        assert_eq!(rows.len(), t.table3.len());
        for (cells, row) in rows.iter().zip(&t.table3) {
            assert_eq!(cells[0], row.parent);
            assert_eq!(cells[1], row.daughters.join(", "));
        }
        // Every plus row in Table 2 has a daughter row in Table 3.
        for l in t.levels.iter().filter(|l| l.suffix == "+") {
            assert!(t.daughters_of(&l.nuclide).is_some(), "{}", l.label);
        }
        // The rule the values serve is quoted from the stored text.
        assert!(normalised(UK_SCHEDULE_SNAPSHOT_XML).contains(&normalised(&t.rule_quote)));
    }

    // Verifies: ACT-036
    #[test]
    fn the_uk_values_equal_the_iaea_values_for_a_set_read_from_the_schedule() {
        let t = uk_clearance();
        let level = |n: &str| t.level_for(n, false).unwrap().bq_per_g;
        assert_eq!(level("H-3"), 100.0);
        assert_eq!(level("C-14"), 10.0);
        assert_eq!(level("Co-60"), 0.1);
        assert_eq!(level("Nb-94"), 0.1);
        assert_eq!(level("Ag-110m"), 0.1);
        assert_eq!(level("Cs-137"), 1.0);
        assert_eq!(level("Fe-55"), 1000.0);
        assert!(t.level_for("U-238sec", false).is_none());
    }

    // Verifies: ACT-036
    #[test]
    fn hand_computed_uk_out_of_scope_sum_of_ratios() {
        // 1000 g. Co-60 20 Bq (0.02 Bq/g over 0.1 = 0.2), H-3 20000 Bq (20 Bq/g over 100 = 0.2),
        // C-14 5000 Bq (5 Bq/g over 10 = 0.5): sum 0.9, at or below 1.
        let inv = inventory(
            1000.0,
            vec![
                nuclide("Co-60", 20.0 * 1.0, None),
                nuclide("H-3", 20000.0, None),
                nuclide("C-14", 5000.0, None),
            ],
        );
        let r = uk_clearance_index(&inv, 0.0);
        assert_eq!(r.regime, "uk-epr2016-sch23-table2");
        assert!((r.index.unwrap() - 0.9).abs() < 1e-9, "{:?}", r.index);
        assert_eq!(r.class, "OUT_OF_SCOPE_BY_TABLE_2");
        // Adding 10 Bq of Co-60 (0.1 more) puts the sum exactly on 1: not above.
        let inv = inventory(
            1000.0,
            vec![
                nuclide("Co-60", 30.0, None),
                nuclide("H-3", 20000.0, None),
                nuclide("C-14", 5000.0, None),
            ],
        );
        let r = uk_clearance_index(&inv, 0.0);
        assert!((r.index.unwrap() - 1.0).abs() < 1e-9);
        assert_eq!(r.at_or_below_one, Some(true));
        // 1 Ci of Co-60 in 1000 g is far above.
        let inv = inventory(1000.0, vec![nuclide("Co-60", CURIE_BQ, None)]);
        assert_eq!(uk_clearance_index(&inv, 0.0).class, "ABOVE_TABLE_2");
    }

    // Verifies: ACT-036
    #[test]
    fn uk_progeny_use_the_plus_row_and_a_missing_row_is_refused() {
        // Sr-90+ is 1 Bq/g with Y-90 included; Y-90 listed as progeny is not counted again.
        let mut y90 = nuclide("Y-90", 500.0, None);
        y90.progeny_of = Some("Sr-90".to_string());
        let inv = inventory(1000.0, vec![nuclide("Sr-90", 500.0, None), y90]);
        let r = uk_clearance_index(&inv, 0.0);
        assert!((r.index.unwrap() - 0.5).abs() < 1e-9);
        assert_eq!(r.progeny_not_counted, vec!["Y-90".to_string()]);
        assert_eq!(r.lines[0].table_row, "Sr-90+");
        // Cf-253 has a plain row (100) and a plus row (1): the plus row only with progeny listed.
        let t = uk_clearance();
        assert_eq!(t.level_for("Cf-253", false).unwrap().bq_per_g, 100.0);
        assert_eq!(t.level_for("Cf-253", true).unwrap().bq_per_g, 1.0);
        // A daughter that Table 3 does not name is refused.
        let mut bad = nuclide("Cs-137", 1.0, None);
        bad.progeny_of = Some("Sr-90".to_string());
        let inv = inventory(1000.0, vec![nuclide("Sr-90", 500.0, None), bad]);
        assert!(uk_clearance_index(&inv, 0.0).not_evaluated.is_some());
        // A nuclide with no Table 2 row is refused, not taken as zero.
        let inv = inventory(1000.0, vec![nuclide("Xx-999", 1.0, None)]);
        let r = uk_clearance_index(&inv, 0.0);
        assert_eq!(r.class, "NOT_EVALUATED");
        // No mass is refused.
        let mut inv = inventory(1000.0, vec![nuclide("Co-60", 1.0, None)]);
        inv.mass_g = None;
        assert_eq!(
            uk_clearance_index(&inv, 0.0).not_evaluated.unwrap().missing,
            vec!["mass_g".to_string()]
        );
    }
}
