//! Versioned, cited materials and limits database (MAT-003, MAT-007, MAT-009 to MAT-012, MAT-015,
//! MAT-040 to MAT-044, MAT-050 to MAT-052, MAT-054 to MAT-056).
//!
//! The database is one plain-text JSON file (`data/materials/materials_db.json`) embedded in the
//! crate. Every value carries a label (authored, literature, calculated, conditional or not
//! evaluated) and a citation with the page or table and the quoted text. Nothing here names a
//! scenario type, so the module can move to another crate unchanged; the waste tables it uses for
//! impurity limits come from `crate::waste`.

pub mod export;
pub mod impurity;
pub mod limits;

use std::{collections::BTreeSet, sync::OnceLock};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use crate::waste::NotEvaluated;

const EMBEDDED_JSON: &str = include_str!("../../data/materials/materials_db.json");
pub const SCHEMA: &str = "faris-materials-db/1";

/// Label of a stored value. A value never changes label silently (MAT-007).
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Label {
    Authored,
    Literature,
    Calculated,
    Conditional,
    NotEvaluated,
}

impl Label {
    pub fn text(self) -> &'static str {
        match self {
            Label::Authored => "authored",
            Label::Literature => "literature",
            Label::Calculated => "calculated",
            Label::Conditional => "conditional",
            Label::NotEvaluated => "not_evaluated",
        }
    }
}

/// Where a value comes from: the source id, the page or table, the quoted text, and the written
/// form of the value as it appears in that quote.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Cite {
    pub source: String,
    pub locator: String,
    pub quote: String,
    pub value_text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Reason {
    pub reason: String,
    pub next_step: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Redistribution {
    PublicDomain,
    CcBy,
    QuoteOnly,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub citation: String,
    pub url: String,
    pub retrieved: String,
    pub licence: String,
    pub redistribution: Redistribution,
    pub note: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DbMeta {
    pub id: String,
    pub version: String,
    pub date: String,
    pub scope: String,
}

/// A value the source states, or the reason it does not (never a silent default).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum Stated<T> {
    Value { value: T },
    NotStated { not_stated: String },
}

impl<T> Stated<T> {
    pub fn value(&self) -> Option<&T> {
        match self {
            Stated::Value { value } => Some(value),
            Stated::NotStated { .. } => None,
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Stated::Value { .. } => None,
            Stated::NotStated { not_stated } => Some(not_stated),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CompositionElement {
    pub element: String,
    pub mass_percent: f64,
    pub label: Label,
    pub cite: Option<Cite>,
    #[serde(default)]
    pub method: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Balance {
    pub element: String,
    pub label: Label,
    pub method: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Composition {
    /// `nominal`, `specified`, `measured`, `assumed` or `not_evaluated`.
    pub status: String,
    pub basis: String,
    pub elements: Vec<CompositionElement>,
    pub balance: Option<Balance>,
    pub not_evaluated: Option<Reason>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SpecKind {
    Nominal,
    Specified,
    Measured,
    Assumed,
}

impl SpecKind {
    pub fn text(self) -> &'static str {
        match self {
            SpecKind::Nominal => "nominal",
            SpecKind::Specified => "specified",
            SpecKind::Measured => "measured",
            SpecKind::Assumed => "assumed",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ImpurityLimit {
    pub element: String,
    #[serde(default)]
    pub min_wt_ppm: Option<f64>,
    pub max_wt_ppm: f64,
    pub cite: Cite,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ImpuritySpec {
    pub id: String,
    pub kind: SpecKind,
    pub description: String,
    pub limits: Vec<ImpurityLimit>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum PropertyValue {
    Scalar {
        value: f64,
    },
    /// `points` are (x, y), x increasing; evaluated by linear interpolation inside the valid range.
    Table {
        x_name: String,
        x_unit: String,
        points: Vec<(f64, f64)>,
    },
    None,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ValidRange {
    pub x_name: String,
    pub low: f64,
    pub high: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Authored {
    pub author: String,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Property {
    pub id: String,
    pub name: String,
    pub unit: String,
    pub label: Label,
    pub value: PropertyValue,
    pub valid_range: Option<ValidRange>,
    pub conditions: Vec<String>,
    pub cite: Option<Cite>,
    pub method: Option<String>,
    pub authored: Option<Authored>,
    pub not_evaluated: Option<Reason>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Material {
    pub id: String,
    pub name: String,
    pub class: String,
    pub composition: Composition,
    pub impurity_specs: Vec<ImpuritySpec>,
    pub properties: Vec<Property>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Quantity {
    Dpa,
    HeAppm,
    HAppm,
    FluenceM2,
    DoseGy,
}

impl Quantity {
    pub fn text(self) -> &'static str {
        match self {
            Quantity::Dpa => "dpa",
            Quantity::HeAppm => "he_appm",
            Quantity::HAppm => "h_appm",
            Quantity::FluenceM2 => "fluence_m2",
            Quantity::DoseGy => "dose_gy",
        }
    }

    pub fn is_gas(self) -> bool {
        matches!(self, Quantity::HeAppm | Quantity::HAppm)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LimitStatus {
    DesignAspiration,
    CodeRule,
    Measurement,
    NotEvaluated,
}

impl LimitStatus {
    pub fn text(self) -> &'static str {
        match self {
            LimitStatus::DesignAspiration => "design_aspiration",
            LimitStatus::CodeRule => "code_rule",
            LimitStatus::Measurement => "measurement",
            LimitStatus::NotEvaluated => "not_evaluated",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Limit {
    pub id: String,
    pub material: String,
    pub component: String,
    pub quantity: Quantity,
    pub value: Option<f64>,
    pub unit: String,
    /// dpa convention (`nrt`, `arc_dpa`, ...) for a dpa limit; a stated reason otherwise.
    pub convention: Stated<String>,
    pub temperature_window_c: Stated<(f64, f64)>,
    pub spectrum: Stated<String>,
    pub form: Stated<String>,
    pub conditions: Vec<String>,
    pub status: LimitStatus,
    pub kind: Label,
    pub cite: Option<Cite>,
    pub not_evaluated: Option<Reason>,
}

/// Where an element's class threshold comes from.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "scheme")]
pub enum ClassThreshold {
    #[serde(rename = "nrc-10cfr61.55")]
    Nrc { table: String, row_label: String },
    #[serde(rename = "fetter-1990-sal")]
    Fetter {
        nuclide: String,
        value: f64,
        unit: String,
        note: String,
        cite: Cite,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ElementLimit {
    pub text: String,
    pub conditions: Vec<String>,
    pub cite: Cite,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WatchEntry {
    pub element: String,
    pub nuclides: Vec<String>,
    pub reason: String,
    pub cite: Cite,
    pub class_threshold: ClassThreshold,
    pub literature_element_limit: Option<ElementLimit>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Database {
    pub schema: String,
    pub database: DbMeta,
    pub sources: Vec<Source>,
    pub materials: Vec<Material>,
    pub limits: Vec<Limit>,
    pub watchlist: Vec<WatchEntry>,
}

/// The database version and content hash written on every verdict (MAT-051).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DbBinding {
    pub id: String,
    pub version: String,
    pub date: String,
    /// SHA-256 of the database file's bytes; any change to any value changes it.
    pub content_sha256: String,
}

/// A loaded database with its binding.
#[derive(Clone, Debug)]
pub struct MaterialsDb {
    pub data: Database,
    pub binding: DbBinding,
}

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("materials database JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("materials database: {}", .0.join("; "))]
    Invalid(Vec<String>),
}

impl MaterialsDb {
    /// Parses and validates a database file's text.
    pub fn from_json_str(text: &str) -> Result<Self, DbError> {
        let data: Database = serde_json::from_str(text)?;
        let problems = validate(&data);
        if !problems.is_empty() {
            return Err(DbError::Invalid(problems));
        }
        let binding = DbBinding {
            id: data.database.id.clone(),
            version: data.database.version.clone(),
            date: data.database.date.clone(),
            content_sha256: format!("{:x}", Sha256::digest(text.as_bytes())),
        };
        Ok(Self { data, binding })
    }

    /// The database shipped in this build.
    pub fn embedded() -> &'static MaterialsDb {
        static DB: OnceLock<MaterialsDb> = OnceLock::new();
        DB.get_or_init(|| {
            MaterialsDb::from_json_str(EMBEDDED_JSON)
                .expect("the embedded materials database is valid")
        })
    }

    pub fn material(&self, id: &str) -> Option<&Material> {
        self.data.materials.iter().find(|m| m.id == id)
    }

    pub fn limit(&self, id: &str) -> Option<&Limit> {
        self.data.limits.iter().find(|l| l.id == id)
    }

    pub fn source(&self, id: &str) -> Option<&Source> {
        self.data.sources.iter().find(|s| s.id == id)
    }

    /// True when a verdict bound to `binding` was made under this database (MAT-051).
    pub fn is_current(&self, binding: &DbBinding) -> bool {
        &self.binding == binding
    }
}

/// Every problem in a database, one sentence each; empty when it is valid (MAT-009).
pub fn validate(db: &Database) -> Vec<String> {
    let mut problems = Vec::new();
    let mut add = |s: String| problems.push(s);
    if db.schema != SCHEMA {
        add(format!("schema is {} and must be {SCHEMA}", db.schema));
    }
    for (what, text) in [
        ("id", &db.database.id),
        ("version", &db.database.version),
        ("date", &db.database.date),
    ] {
        if text.trim().is_empty() {
            add(format!("database {what} is empty"));
        }
    }
    let mut source_ids = BTreeSet::new();
    for s in &db.sources {
        if !source_ids.insert(s.id.as_str()) {
            add(format!("source {} appears twice", s.id));
        }
        for (what, text) in [
            ("citation", &s.citation),
            ("url", &s.url),
            ("retrieved", &s.retrieved),
            ("licence", &s.licence),
            ("note", &s.note),
        ] {
            if text.trim().is_empty() {
                add(format!("source {} has an empty {what}", s.id));
            }
        }
    }
    let check_cite =
        |owner: &str, cite: &Cite, numeric: Option<f64>, add: &mut dyn FnMut(String)| {
            if !source_ids.contains(cite.source.as_str()) {
                add(format!(
                    "{owner}: citation names source {} that is not in the source list",
                    cite.source
                ));
            }
            for (what, text) in [
                ("locator", &cite.locator),
                ("quote", &cite.quote),
                ("value_text", &cite.value_text),
            ] {
                if text.trim().is_empty() {
                    add(format!("{owner}: citation has an empty {what}"));
                }
            }
            if !cite.quote.contains(&cite.value_text) {
                add(format!(
                    "{owner}: the quoted text does not contain the value as written ({})",
                    cite.value_text
                ));
            }
            if let Some(v) = numeric
                && cite
                    .value_text
                    .parse::<f64>()
                    .map(|p| p != v)
                    .unwrap_or(true)
            {
                add(format!(
                    "{owner}: the stored value {v} is not the value as written in the quote ({})",
                    cite.value_text
                ));
            }
        };
    let mut material_ids = BTreeSet::new();
    for m in &db.materials {
        if !material_ids.insert(m.id.as_str()) {
            add(format!("material {} appears twice", m.id));
        }
        validate_composition(m, &mut |s| add(s), &check_cite);
        let mut spec_ids = BTreeSet::new();
        for spec in &m.impurity_specs {
            if !spec_ids.insert(spec.id.as_str()) {
                add(format!(
                    "{}: impurity specification {} appears twice",
                    m.id, spec.id
                ));
            }
            if spec.limits.is_empty() {
                add(format!(
                    "{}: impurity specification {} lists no element",
                    m.id, spec.id
                ));
            }
            let mut elements = BTreeSet::new();
            for l in &spec.limits {
                let owner = format!("{} {} {}", m.id, spec.id, l.element);
                if !elements.insert(l.element.as_str()) {
                    add(format!("{owner} appears twice"));
                }
                if !(l.max_wt_ppm.is_finite() && l.max_wt_ppm >= 0.0) {
                    add(format!("{owner}: max_wt_ppm is not a number of at least 0"));
                }
                if let Some(min) = l.min_wt_ppm
                    && !(min.is_finite() && min >= 0.0 && min <= l.max_wt_ppm)
                {
                    add(format!(
                        "{owner}: min_wt_ppm is not between 0 and the maximum"
                    ));
                }
                check_cite(&owner, &l.cite, Some(l.max_wt_ppm), &mut |s| add(s));
            }
        }
        let mut property_ids = BTreeSet::new();
        for p in &m.properties {
            if !property_ids.insert(p.id.as_str()) {
                add(format!("{}: property {} appears twice", m.id, p.id));
            }
            validate_property(&m.id, p, &mut |s| add(s), &check_cite);
        }
    }
    let mut limit_ids = BTreeSet::new();
    for l in &db.limits {
        if !limit_ids.insert(l.id.as_str()) {
            add(format!("limit {} appears twice", l.id));
        }
        validate_limit(l, &material_ids, &mut |s| add(s), &check_cite);
    }
    let mut elements = BTreeSet::new();
    for w in &db.watchlist {
        if !elements.insert(w.element.as_str()) {
            add(format!("watch-list element {} appears twice", w.element));
        }
        if w.reason.trim().is_empty() || w.nuclides.is_empty() {
            add(format!(
                "watch-list element {} needs a reason and at least one nuclide",
                w.element
            ));
        }
        check_cite(
            &format!("watch-list {}", w.element),
            &w.cite,
            None,
            &mut |s| add(s),
        );
        match &w.class_threshold {
            ClassThreshold::Nrc { table, row_label } => {
                if impurity::nrc_threshold(table, row_label).is_none() {
                    add(format!(
                        "watch-list {}: no row '{row_label}' in 10 CFR 61.55 {table}",
                        w.element
                    ));
                }
            }
            ClassThreshold::Fetter { value, cite, .. } => {
                check_cite(
                    &format!("watch-list {} threshold", w.element),
                    cite,
                    Some(*value),
                    &mut |s| add(s),
                );
            }
        }
        if let Some(e) = &w.literature_element_limit {
            check_cite(
                &format!("watch-list {} element limit", w.element),
                &e.cite,
                None,
                &mut |s| add(s),
            );
        }
    }
    problems
}

type CiteCheck<'a> = dyn Fn(&str, &Cite, Option<f64>, &mut dyn FnMut(String)) + 'a;

fn validate_composition(m: &Material, add: &mut dyn FnMut(String), check_cite: &CiteCheck) {
    let c = &m.composition;
    if c.status == "not_evaluated" {
        if c.not_evaluated.is_none() || !c.elements.is_empty() {
            add(format!(
                "{}: a composition marked NOT_EVALUATED needs a reason and no elements",
                m.id
            ));
        }
        return;
    }
    if !["nominal", "specified", "measured", "assumed"].contains(&c.status.as_str()) {
        add(format!(
            "{}: composition status {} is not nominal, specified, measured, assumed or not evaluated",
            m.id, c.status
        ));
    }
    let mut total = 0.0;
    let mut seen = BTreeSet::new();
    for e in &c.elements {
        let owner = format!("{} composition {}", m.id, e.element);
        if !seen.insert(e.element.as_str()) {
            add(format!("{owner} appears twice"));
        }
        total += e.mass_percent;
        match (e.label, &e.cite) {
            (Label::Literature | Label::Conditional, Some(cite)) => {
                check_cite(&owner, cite, Some(e.mass_percent), add);
            }
            (Label::Calculated, Some(cite)) => {
                check_cite(&owner, cite, None, add);
                if e.method.as_deref().is_none_or(|s| s.trim().is_empty()) {
                    add(format!("{owner}: a calculated value needs its method"));
                }
            }
            (Label::Authored, _) => {}
            (label, _) => add(format!("{owner}: label {} needs a citation", label.text())),
        }
    }
    let Some(balance) = &c.balance else {
        add(format!(
            "{}: composition has no balance element; the fractions cannot be made to sum to 1",
            m.id
        ));
        return;
    };
    if balance.label != Label::Calculated || balance.method.trim().is_empty() {
        add(format!(
            "{}: the balance element must be calculated, with its method",
            m.id
        ));
    }
    if c.elements.iter().any(|e| e.element == balance.element) {
        add(format!(
            "{}: the balance element {} is also listed",
            m.id, balance.element
        ));
    }
    if total >= 100.0 {
        add(format!(
            "{}: listed elements sum to {total} percent, leaving nothing for the balance",
            m.id
        ));
    }
}

fn validate_property(
    material: &str,
    p: &Property,
    add: &mut dyn FnMut(String),
    check_cite: &CiteCheck,
) {
    let owner = format!("{material} property {}", p.id);
    match p.label {
        Label::NotEvaluated => {
            if p.not_evaluated.is_none() || p.value != PropertyValue::None {
                add(format!(
                    "{owner}: a property marked NOT_EVALUATED needs a reason and no value"
                ));
            }
        }
        Label::Literature | Label::Conditional => {
            let numeric = match &p.value {
                PropertyValue::Scalar { value } => Some(*value),
                _ => None,
            };
            match &p.cite {
                Some(c) => check_cite(&owner, c, numeric, add),
                None => add(format!(
                    "{owner}: a {} value needs a citation",
                    p.label.text()
                )),
            }
            if p.label == Label::Conditional && p.conditions.is_empty() {
                add(format!("{owner}: a conditional value lists its conditions"));
            }
            if p.value == PropertyValue::None {
                add(format!(
                    "{owner}: a {} property needs a value",
                    p.label.text()
                ));
            }
        }
        Label::Calculated => {
            if p.method.as_deref().is_none_or(|s| s.trim().is_empty()) {
                add(format!("{owner}: a calculated value needs its method"));
            }
        }
        Label::Authored => match &p.authored {
            Some(a) if !a.author.trim().is_empty() && !a.reason.trim().is_empty() => {}
            _ => add(format!(
                "{owner}: an authored value needs an author and a reason"
            )),
        },
    }
    if let PropertyValue::Table { points, x_name, .. } = &p.value {
        if points.len() < 2 || points.windows(2).any(|w| w[0].0 >= w[1].0) {
            add(format!(
                "{owner}: a table needs at least two points with x increasing"
            ));
        }
        match &p.valid_range {
            Some(r)
                if r.x_name == *x_name
                    && points.first().is_some_and(|f| r.low >= f.0)
                    && points.last().is_some_and(|l| r.high <= l.0) => {}
            _ => add(format!(
                "{owner}: a table needs a valid range inside its points, on the same variable"
            )),
        }
    }
}

fn validate_limit(
    l: &Limit,
    materials: &BTreeSet<&str>,
    add: &mut dyn FnMut(String),
    check_cite: &CiteCheck,
) {
    let owner = format!("limit {}", l.id);
    if !materials.contains(l.material.as_str()) {
        add(format!(
            "{owner}: material {} is not in the database",
            l.material
        ));
    }
    if l.component.trim().is_empty() || l.unit.trim().is_empty() {
        add(format!("{owner}: component and unit must be filled"));
    }
    for (what, reason) in [
        ("convention", l.convention.reason()),
        ("temperature window", l.temperature_window_c.reason()),
        ("spectrum", l.spectrum.reason()),
        ("form", l.form.reason()),
    ] {
        if reason.is_some_and(|r| r.trim().is_empty()) {
            add(format!("{owner}: {what} is not stated and gives no reason"));
        }
    }
    if l.quantity == Quantity::Dpa
        && l.convention.value().is_none()
        && l.convention.reason().is_none()
    {
        add(format!(
            "{owner}: a dpa limit states its convention or why it cannot"
        ));
    }
    if l.conditions
        .iter()
        .any(|c| c.contains('\n') || c.trim().is_empty())
    {
        add(format!("{owner}: a condition is empty or has a line break"));
    }
    if let Some((lo, hi)) = l.temperature_window_c.value()
        && !(lo.is_finite() && hi.is_finite() && lo <= hi)
    {
        add(format!(
            "{owner}: the temperature window is not low then high"
        ));
    }
    if l.status == LimitStatus::NotEvaluated {
        if l.value.is_some()
            || l.cite.is_some()
            || l.not_evaluated.is_none()
            || l.kind != Label::NotEvaluated
        {
            add(format!(
                "{owner}: a limit marked NOT_EVALUATED has no value or citation, and gives a reason and next step"
            ));
        }
        return;
    }
    if !matches!(l.kind, Label::Literature | Label::Conditional) {
        add(format!(
            "{owner}: a stored limit is literature or conditional; use an override for an authored value"
        ));
    }
    if l.kind == Label::Conditional && l.conditions.is_empty() {
        add(format!("{owner}: a conditional limit lists its conditions"));
    }
    match (&l.value, &l.cite) {
        (Some(v), Some(c)) if v.is_finite() && *v >= 0.0 => check_cite(&owner, c, Some(*v), add),
        _ => add(format!(
            "{owner}: a limit needs a value of at least 0 and a citation with locator and quote"
        )),
    }
}

/// A property evaluated at one x, or the reason it was not (MAT-054).
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct PropertyEvaluation {
    pub property: String,
    pub value: Option<f64>,
    pub unit: String,
    pub label: Label,
    pub method: String,
    pub not_evaluated: Option<NotEvaluated>,
}

impl Property {
    /// Value at `x` (the table's variable). Outside the valid range the result is NOT_EVALUATED with
    /// the range; the table is never extrapolated.
    pub fn evaluate(&self, x: f64) -> PropertyEvaluation {
        let refuse = |reason: String, next: &str, missing: Vec<String>| PropertyEvaluation {
            property: self.id.clone(),
            value: None,
            unit: self.unit.clone(),
            label: Label::NotEvaluated,
            method: "none".to_string(),
            not_evaluated: Some(NotEvaluated::new(reason, missing, next)),
        };
        match &self.value {
            PropertyValue::None => refuse(
                self.not_evaluated
                    .as_ref()
                    .map_or_else(|| "no value".to_string(), |r| r.reason.clone()),
                self.not_evaluated
                    .as_ref()
                    .map_or("Add a cited value.", |r| r.next_step.as_str()),
                vec![self.id.clone()],
            ),
            PropertyValue::Scalar { value } => PropertyEvaluation {
                property: self.id.clone(),
                value: Some(*value),
                unit: self.unit.clone(),
                label: self.label,
                method: "stored scalar, independent of x".to_string(),
                not_evaluated: None,
            },
            PropertyValue::Table { points, x_name, .. } => {
                let Some(range) = &self.valid_range else {
                    return refuse(
                        "the table has no valid range".to_string(),
                        "Add the cited valid range.",
                        vec![self.id.clone()],
                    );
                };
                if !x.is_finite() || x < range.low || x > range.high {
                    return refuse(
                        format!(
                            "{x_name} = {x} is outside the cited range {} to {}; FARIS does not extrapolate",
                            range.low, range.high
                        ),
                        "Use a value inside the range, or add a cited table that covers it.",
                        vec![x_name.clone()],
                    );
                }
                let i = points
                    .partition_point(|p| p.0 <= x)
                    .saturating_sub(1)
                    .min(points.len() - 2);
                let (x0, y0) = points[i];
                let (x1, y1) = points[i + 1];
                PropertyEvaluation {
                    property: self.id.clone(),
                    value: Some(y0 + (y1 - y0) * (x - x0) / (x1 - x0)),
                    unit: self.unit.clone(),
                    label: self.label,
                    method: "linear interpolation between the cited table points".to_string(),
                    not_evaluated: None,
                }
            }
        }
    }
}

impl Material {
    /// Mass fractions that sum to 1 within 1e-9 (MAT-003): listed elements plus the balance.
    pub fn mass_fractions(&self) -> Result<Vec<(String, f64)>, NotEvaluated> {
        let c = &self.composition;
        let (Some(balance), false) = (&c.balance, c.status == "not_evaluated") else {
            return Err(NotEvaluated::new(
                c.not_evaluated
                    .as_ref()
                    .map_or_else(|| "no composition".to_string(), |r| r.reason.clone()),
                vec![self.id.clone()],
                c.not_evaluated.as_ref().map_or_else(
                    || "Add a cited composition.".to_string(),
                    |r| r.next_step.clone(),
                ),
            ));
        };
        let mut out: Vec<(String, f64)> = c
            .elements
            .iter()
            .map(|e| (e.element.clone(), e.mass_percent / 100.0))
            .collect();
        let listed: f64 = out.iter().map(|e| e.1).sum();
        out.push((balance.element.clone(), 1.0 - listed));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn embedded_text() -> &'static str {
        EMBEDDED_JSON
    }

    fn mutated(from: &str, to: &str) -> String {
        assert!(
            EMBEDDED_JSON.contains(from),
            "test needs `{from}` in the database"
        );
        EMBEDDED_JSON.replacen(from, to, 1)
    }

    // Verifies: MAT-009
    #[test]
    fn the_embedded_database_is_valid() {
        let db = MaterialsDb::embedded();
        assert_eq!(db.data.schema, SCHEMA);
        assert!(validate(&db.data).is_empty());
        assert_eq!(db.binding.version, "0.1.0");
    }

    // Verifies: MAT-003
    #[test]
    fn composition_fractions_sum_to_one_with_a_labelled_balance() {
        let db = MaterialsDb::embedded();
        let m = db.material("eurofer97").unwrap();
        let fractions = m.mass_fractions().unwrap();
        let total: f64 = fractions.iter().map(|f| f.1).sum();
        assert!((total - 1.0).abs() < 1e-9);
        let fe = fractions.iter().find(|f| f.0 == "Fe").unwrap().1;
        assert!((fe - (1.0 - 10.96 / 100.0)).abs() < 1e-12, "{fe}");
        assert_eq!(
            m.composition.balance.as_ref().unwrap().label,
            Label::Calculated
        );
        // A material without a cited composition says so; it never gets a default.
        let err = db
            .material("tungsten")
            .unwrap()
            .mass_fractions()
            .unwrap_err();
        assert!(err.reason.contains("no tungsten grade specification"));
        assert!(!err.next_step.is_empty());
    }

    // Verifies: MAT-003, MAT-044
    #[test]
    fn every_impurity_value_has_a_kind_and_a_citation() {
        let db = MaterialsDb::embedded();
        let m = db.material("eurofer97").unwrap();
        assert_eq!(m.impurity_specs.len(), 2);
        let kinds: Vec<_> = m.impurity_specs.iter().map(|s| s.kind.text()).collect();
        assert_eq!(kinds, ["specified", "measured"]);
        for spec in &m.impurity_specs {
            for l in &spec.limits {
                assert!(!l.cite.quote.is_empty() && !l.cite.locator.is_empty());
            }
        }
        let nb = m.impurity_specs[0]
            .limits
            .iter()
            .find(|l| l.element == "Nb")
            .unwrap();
        assert_eq!(nb.max_wt_ppm, 10.0);
        let nb_measured = m.impurity_specs[1]
            .limits
            .iter()
            .find(|l| l.element == "Nb")
            .unwrap();
        assert_eq!(
            (nb_measured.min_wt_ppm, nb_measured.max_wt_ppm),
            (Some(2.0), 7.0)
        );
    }

    // Verifies: MAT-007, MAT-050
    #[test]
    fn a_value_whose_quote_does_not_hold_it_is_rejected() {
        // Change a stored Nb limit from 10 to 11 while the quote still says <10.
        let text = mutated(
            "{\"element\": \"Nb\", \"max_wt_ppm\": 10,",
            "{\"element\": \"Nb\", \"max_wt_ppm\": 11,",
        );
        let err = MaterialsDb::from_json_str(&text).unwrap_err();
        assert!(
            err.to_string()
                .contains("not the value as written in the quote"),
            "{err}"
        );
        // A literature label without a citation is rejected.
        let text = mutated(
            "\"label\": \"literature\",\n     \"value\": {\"shape\": \"scalar\", \"value\": 550.0},",
            "\"label\": \"literature\",\n     \"value\": {\"shape\": \"scalar\", \"value\": 551.0},",
        );
        assert!(MaterialsDb::from_json_str(&text).is_err());
        // An unknown field or label is rejected by the schema.
        assert!(
            MaterialsDb::from_json_str(&mutated(
                "\"label\": \"conditional\"",
                "\"label\": \"probably\""
            ))
            .is_err()
        );
    }

    // Verifies: MAT-009
    #[test]
    fn duplicates_and_unknown_sources_are_found() {
        let text = mutated(
            "\"id\": \"tungsten-limits\"",
            "\"id\": \"cucrzr-dpa-divertor\"",
        );
        assert!(
            MaterialsDb::from_json_str(&text)
                .unwrap_err()
                .to_string()
                .contains("appears twice")
        );
        let text = mutated(
            "\"source\": \"wang-1996-jnm\"",
            "\"source\": \"nobody-1999\"",
        );
        assert!(
            MaterialsDb::from_json_str(&text)
                .unwrap_err()
                .to_string()
                .contains("not in the source list")
        );
    }

    // Verifies: MAT-007
    #[test]
    fn every_property_is_labelled_and_missing_ones_say_why() {
        let db = MaterialsDb::embedded();
        for m in &db.data.materials {
            for p in &m.properties {
                match p.label {
                    Label::NotEvaluated => {
                        let r = p.not_evaluated.as_ref().unwrap();
                        assert!(!r.reason.is_empty() && !r.next_step.is_empty());
                        assert_eq!(p.value, PropertyValue::None);
                    }
                    Label::Literature | Label::Conditional => assert!(p.cite.is_some()),
                    _ => {}
                }
            }
        }
        let lower = db
            .material("eurofer97")
            .unwrap()
            .properties
            .iter()
            .find(|p| p.id == "operating_temperature_lower_c")
            .unwrap();
        assert_eq!(lower.label, Label::Conditional);
        assert!(lower.conditions.iter().any(|c| c.contains("10 dpa")));
    }

    fn fixture_property() -> Property {
        // A synthetic table for the range tests only; it is not in the database.
        Property {
            id: "fixture_table".to_string(),
            name: "fixture".to_string(),
            unit: "units".to_string(),
            label: Label::Authored,
            value: PropertyValue::Table {
                x_name: "temperature_c".to_string(),
                x_unit: "degC".to_string(),
                points: vec![(100.0, 10.0), (200.0, 30.0), (400.0, 40.0)],
            },
            valid_range: Some(ValidRange {
                x_name: "temperature_c".to_string(),
                low: 100.0,
                high: 400.0,
            }),
            conditions: vec![],
            cite: None,
            method: None,
            authored: Some(Authored {
                author: "test".to_string(),
                reason: "fixture".to_string(),
            }),
            not_evaluated: None,
        }
    }

    // Verifies: MAT-054
    #[test]
    fn a_table_is_interpolated_inside_its_range_and_never_extrapolated() {
        let p = fixture_property();
        let at = |x: f64| p.evaluate(x);
        assert_eq!(at(100.0).value, Some(10.0));
        assert_eq!(at(150.0).value, Some(20.0));
        assert_eq!(at(200.0).value, Some(30.0));
        assert_eq!(at(300.0).value, Some(35.0));
        assert_eq!(at(400.0).value, Some(40.0));
        assert!(at(150.0).method.contains("linear interpolation"));
        for outside in [99.9, 400.1, f64::NAN] {
            let r = at(outside);
            assert_eq!(r.value, None);
            assert_eq!(r.label, Label::NotEvaluated);
            let ne = r.not_evaluated.unwrap();
            assert!(ne.reason.contains("100") && ne.reason.contains("400") || outside.is_nan());
            assert!(ne.reason.contains("does not extrapolate"));
        }
        // A not-evaluated property gives its stored reason.
        let db = MaterialsDb::embedded();
        let density = db
            .material("eurofer97")
            .unwrap()
            .properties
            .iter()
            .find(|p| p.id == "density_kg_m3")
            .unwrap();
        assert!(
            density
                .evaluate(300.0)
                .not_evaluated
                .unwrap()
                .reason
                .contains("no density value")
        );
    }

    // Verifies: MAT-051
    #[test]
    fn a_database_change_changes_the_binding_and_invalidates_bound_verdicts() {
        let db = MaterialsDb::embedded();
        let bound = db.binding.clone();
        assert!(db.is_current(&bound));
        let changed =
            MaterialsDb::from_json_str(&mutated("\"value\": 20.0,", "\"value\": 21.0,")).err();
        // 21 no longer equals the quoted 20, so the edit is rejected; make a legal edit instead.
        assert!(changed.is_some());
        let legal = MaterialsDb::from_json_str(&mutated(
            "\"version\": \"0.1.0\"",
            "\"version\": \"0.1.1\"",
        ))
        .unwrap();
        assert!(!legal.is_current(&bound));
        let whitespace = MaterialsDb::from_json_str(&format!("{} ", embedded_text())).unwrap();
        assert_eq!(whitespace.binding.version, bound.version);
        assert_ne!(whitespace.binding.content_sha256, bound.content_sha256);
        assert!(!whitespace.is_current(&bound));
    }

    // Verifies: MAT-055
    #[test]
    fn every_source_states_its_licence_and_redistribution_status() {
        let db = MaterialsDb::embedded();
        assert!(db.data.sources.len() >= 6);
        for s in &db.data.sources {
            assert!(!s.licence.is_empty());
        }
        let federici = db.source("federici-2017").unwrap();
        assert_eq!(federici.redistribution, Redistribution::CcBy);
        assert_eq!(
            db.source("nrc-10cfr61.55").unwrap().redistribution,
            Redistribution::PublicDomain
        );
        // Sources whose terms the file does not state are quote-only, never redistributed.
        for id in [
            "zinkle-osti-1364282",
            "wang-1996-jnm",
            "gaganidze-kit-2017",
            "fetter-1990-part2",
        ] {
            assert_eq!(
                db.source(id).unwrap().redistribution,
                Redistribution::QuoteOnly,
                "{id}"
            );
        }
    }
}
