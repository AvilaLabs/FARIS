//! The semantic diff of two studies or two models (GEO-067, GEO-068, COL-020, COL-022, COL-023,
//! COL-025).
//!
//! A diff is not a text diff. Two documents are read as values, so key order, white space, number
//! spelling and line endings are not changes (`faris_model::canonical`). Every difference is a
//! parameter with a path, an old and a new value and, when the key name says so, a unit. The
//! report keeps the causes apart from the effects: inputs changed, evidence changed, results
//! changed, and presentation-only changes (labels, colours, the view). Every input change carries
//! a cost class (instant, seconds, transport run) and the kinds of result it invalidates; every
//! result is then listed as invalidated or reusable, and every changed result names the input
//! changes that can explain it or says that none was found.
//!
//! The rules that decide the cost class are in this file (`classify`), one place, so the desktop
//! and the command line agree. A path the rules do not know is treated as needing a new transport
//! run: a wrong "reusable" would serve a stale result.

mod components;
mod render;
#[cfg(test)]
mod tests;

pub use components::{
    ComponentChange, GeometryRow, GeometrySummary, cell_rows, geometry_summary,
    geometry_summary_csv,
};
pub use render::render_text;

use faris_model::canonical::{canonical_json, leaves_equal, values_equal};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// The version of the JSON this module writes. A change to a field name or meaning bumps it.
pub const DIFF_SCHEMA: &str = "faris-diff/v1";

/// Most changes listed for one document; the rest are counted (`truncated`).
const MAX_CHANGES_PER_DOCUMENT: usize = 2000;
/// Largest id-keyed array whose order is compared element by element (the move search is
/// quadratic).
const MAX_MOVE_SEARCH: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CostClass {
    /// Nothing is recalculated: a label, a colour, the view.
    Instant,
    /// The operating history and what depends on it is recalculated; the transport is reused.
    Seconds,
    /// The transport has to be run again.
    TransportRun,
}

impl CostClass {
    pub fn words(self) -> &'static str {
        match self {
            Self::Instant => "instant",
            Self::Seconds => "seconds",
            Self::TransportRun => "transport run",
        }
    }

    fn affects(self) -> &'static [&'static str] {
        match self {
            Self::Instant => &[],
            Self::Seconds => &["history", "ensembles"],
            Self::TransportRun => &["transport", "history", "ensembles", "evidence"],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Removed,
    Changed,
    /// Same element, another place in an ordered list.
    Moved,
}

/// What kind of document an input is. It decides which paths are labels and which are physics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocKind {
    Scenario,
    Physics,
    /// The OpenMC input a transport bundle records (`faris-openmc-input`).
    TransportInput,
    /// Operating-history assumptions.
    Assumptions,
    Design,
    Other,
}

impl DocKind {
    /// The kind a document names itself as, by its `schema_version` or `schema` field.
    pub fn detect(value: &Value) -> Self {
        let tag = value
            .get("schema_version")
            .or_else(|| value.get("schema"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if tag.starts_with("faris-scenario/") {
            Self::Scenario
        } else if tag.starts_with("faris-openmc-input/") {
            Self::TransportInput
        } else if tag.starts_with("faris-operating-history/") {
            Self::Assumptions
        } else if tag.starts_with("faris-design/") {
            Self::Design
        } else if value.get("component_assignments").is_some() && value.get("materials").is_some() {
            Self::Physics
        } else {
            Self::Other
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Scenario => "scenario",
            Self::Physics => "physics case",
            Self::TransportInput => "transport input",
            Self::Assumptions => "operating assumptions",
            Self::Design => "design file",
            Self::Other => "document",
        }
    }
}

/// Which section of the report a document belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Inputs,
    Evidence,
    Presentation,
}

/// One difference between the two sides.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Change {
    /// `i1` (input), `e1` (evidence), `p1` (presentation).
    pub id: String,
    pub document: String,
    pub path: String,
    pub kind: ChangeKind,
    pub old: Option<Value>,
    pub new: Option<Value>,
    pub unit: Option<String>,
    pub cost: CostClass,
    /// Kinds of result this change invalidates: transport, history, ensembles, evidence.
    pub invalidates: Vec<&'static str>,
    /// Where it applies: empty for the whole study, `port` for an arrangement, `port/reference`
    /// for one recorded run.
    pub scope: String,
    pub note: Option<String>,
}

/// What a result is, which decides which changes can explain it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultKind {
    Transport,
    History,
    Ensemble,
    EvidenceReceipt,
}

impl ResultKind {
    fn tag(self) -> &'static str {
        match self {
            Self::Transport => "transport",
            Self::History => "history",
            Self::Ensemble => "ensembles",
            Self::EvidenceReceipt => "evidence",
        }
    }
}

/// A result of one side or both, by its identity in the study.
#[derive(Clone, Debug)]
pub struct ResultEntry {
    /// `port`, `control`, `sweep`, `ensembles`, `history`, `evidence`.
    pub group: String,
    pub name: String,
    pub kind: ResultKind,
    pub old_sha256: Option<String>,
    pub new_sha256: Option<String>,
    /// The leaf-level differences, when they were asked for and the result is data.
    pub values: Vec<Change>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ResultChange {
    pub id: String,
    pub item: String,
    pub kind: ChangeKind,
    pub old_sha256: Option<String>,
    pub new_sha256: Option<String>,
    /// Input changes (ids) that can explain it; empty when none was found.
    pub caused_by: Vec<String>,
    pub cause_note: Option<String>,
    pub values: Vec<Change>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    Invalidated,
    Reusable,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ImpactItem {
    pub item: String,
    pub standing: Standing,
    /// What it costs to get the item again when it is invalidated.
    pub cost: Option<CostClass>,
    pub because: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Summary {
    pub inputs_changed: usize,
    pub evidence_changed: usize,
    pub results_changed: usize,
    pub presentation_changed: usize,
    /// Highest cost class among the input and evidence changes; `None` when there are none.
    pub highest_cost: Option<CostClass>,
    pub invalidated: usize,
    pub reusable: usize,
    /// Changes beyond the per-document limit, counted but not listed.
    pub not_listed: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DiffReport {
    pub schema: &'static str,
    pub a: String,
    pub b: String,
    /// What was compared: `study file` or the document kind.
    pub compared: String,
    /// True when nothing differs in any section.
    pub identical: bool,
    pub summary: Summary,
    pub inputs: Vec<Change>,
    pub evidence: Vec<Change>,
    pub results: Vec<ResultChange>,
    pub presentation: Vec<Change>,
    pub components: Vec<ComponentChange>,
    pub impact: Vec<ImpactItem>,
}

// ---------------------------------------------------------------------------
// Rules

/// What the rules say about one path.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub section: Section,
    pub cost: CostClass,
    pub note: Option<&'static str>,
}

const LABEL_KEYS: [&str; 10] = [
    "label",
    "title",
    "description",
    "color",
    "notice",
    "references",
    "rationale",
    "provenance",
    "use",
    "url",
];

/// Locations of tools on the machine that made a record, not identities (the hashes beside them
/// are).
const LOCATION_KEYS: [&str; 2] = ["openmc_executable", "cross_sections"];

fn segments(path: &str) -> Vec<&str> {
    path.split('.')
        .map(|p| p.split('[').next().unwrap_or(p))
        .filter(|p| !p.is_empty())
        .collect()
}

/// The cost class and section of a path in a document of this kind. Unknown paths need a
/// transport run: the rule is to fail closed.
pub fn classify(kind: DocKind, path: &str) -> Rule {
    let parts = segments(path);
    let first = parts.first().copied().unwrap_or("");
    let last = parts.last().copied().unwrap_or("");
    let presentation = |note| Rule {
        section: Section::Presentation,
        cost: CostClass::Instant,
        note,
    };
    let input = |cost, note| Rule {
        section: Section::Inputs,
        cost,
        note,
    };
    if parts.iter().any(|p| LABEL_KEYS.contains(p)) {
        return presentation(None);
    }
    if matches!(kind, DocKind::TransportInput) && LOCATION_KEYS.contains(&first) {
        return presentation(Some(
            "where a tool was found on the machine that made the record, not which tool it was",
        ));
    }
    // Free text lists that only describe the study.
    if parts.contains(&"assumptions")
        && matches!(
            kind,
            DocKind::Scenario | DocKind::Physics | DocKind::TransportInput
        )
    {
        return presentation(None);
    }
    if matches!(kind, DocKind::Physics | DocKind::TransportInput)
        && parts.contains(&"scientific_scope")
    {
        return presentation(Some(
            "the stated scope of the case, not a calculation input",
        ));
    }
    let history_only = || {
        input(
            CostClass::Seconds,
            Some("rescales the history; the transport is reused"),
        )
    };
    match kind {
        DocKind::Assumptions => history_only(),
        DocKind::Scenario | DocKind::TransportInput => {
            if last == "horizon_years" {
                return history_only();
            }
            if last == "fusion_power_mw" {
                return input(
                    CostClass::TransportRun,
                    Some("the transport request records the power"),
                );
            }
            input(CostClass::TransportRun, None)
        }
        _ => input(CostClass::TransportRun, None),
    }
}

/// The unit a key name states by its ending, if it does.
pub fn unit_of(path: &str) -> Option<&'static str> {
    const ENDINGS: [(&str, &str); 22] = [
        ("_n_m2_s", "n/m^2/s"),
        ("_kg_m3", "kg/m^3"),
        ("_n_m2", "n/m^2"),
        ("_xyz_m", "m"),
        ("_cm_per_source", "cm per source neutron"),
        ("_years", "y"),
        ("_seconds", "s"),
        ("_percent", "%"),
        ("_m3", "m^3"),
        ("_m2", "m^2"),
        ("_mw", "MW"),
        ("_mwh", "MWh"),
        ("_twh", "TWh"),
        ("_kg", "kg"),
        ("_ev", "eV"),
        ("_rad", "rad"),
        ("_mb", "MiB"),
        ("_cm", "cm"),
        ("_m", "m"),
        ("_s", "s"),
        ("_k", "K"),
        ("_fraction", "fraction"),
    ];
    let name = segments(path).last().copied()?;
    ENDINGS
        .iter()
        .find(|(ending, _)| name.ends_with(ending))
        .map(|(_, unit)| *unit)
}

// ---------------------------------------------------------------------------
// The structural difference of two values

const ID_KEYS: [&str; 7] = [
    "id",
    "component_id",
    "response_id",
    "solid_id",
    "nuclide",
    "name",
    "path",
];

/// The key that identifies the elements of an array of objects, if every element has it once.
fn id_key(items: &[Value]) -> Option<&'static str> {
    if items.is_empty() {
        return None;
    }
    ID_KEYS.iter().copied().find(|key| {
        let mut seen = std::collections::BTreeSet::new();
        items.iter().all(|item| {
            item.get(*key)
                .and_then(Value::as_str)
                .is_some_and(|id| seen.insert(id))
        })
    })
}

fn id_of<'a>(item: &'a Value, key: &str) -> &'a str {
    item.get(key).and_then(Value::as_str).unwrap_or("")
}

/// One raw difference before it is given a rule and an id.
#[derive(Clone, Debug)]
pub struct RawChange {
    pub path: String,
    pub kind: ChangeKind,
    pub old: Option<Value>,
    pub new: Option<Value>,
}

struct Walker<'a> {
    out: &'a mut Vec<RawChange>,
    limit: usize,
    not_listed: usize,
}

impl Walker<'_> {
    fn push(&mut self, change: RawChange) {
        if self.out.len() < self.limit {
            self.out.push(change);
        } else {
            self.not_listed += 1;
        }
    }

    fn walk(&mut self, path: &str, a: &Value, b: &Value) {
        match (a, b) {
            (Value::Object(x), Value::Object(y)) => {
                let mut keys: Vec<&String> = x.keys().chain(y.keys()).collect();
                keys.sort();
                keys.dedup();
                for key in keys {
                    let child = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    match (x.get(key), y.get(key)) {
                        (Some(p), Some(q)) => self.walk(&child, p, q),
                        (Some(p), None) => self.push(RawChange {
                            path: child,
                            kind: ChangeKind::Removed,
                            old: Some(p.clone()),
                            new: None,
                        }),
                        (None, Some(q)) => self.push(RawChange {
                            path: child,
                            kind: ChangeKind::Added,
                            old: None,
                            new: Some(q.clone()),
                        }),
                        (None, None) => {}
                    }
                }
            }
            (Value::Array(x), Value::Array(y)) => self.walk_arrays(path, x, y),
            _ => {
                if !leaves_equal(a, b) {
                    self.push(RawChange {
                        path: path.to_string(),
                        kind: ChangeKind::Changed,
                        old: Some(a.clone()),
                        new: Some(b.clone()),
                    });
                }
            }
        }
    }

    fn walk_arrays(&mut self, path: &str, x: &[Value], y: &[Value]) {
        let keyed = match (id_key(x), id_key(y)) {
            (Some(p), Some(q)) if p == q => Some(p),
            _ => None,
        };
        let Some(key) = keyed else {
            for i in 0..x.len().max(y.len()) {
                let child = format!("{path}[{i}]");
                match (x.get(i), y.get(i)) {
                    (Some(p), Some(q)) => self.walk(&child, p, q),
                    (Some(p), None) => self.push(RawChange {
                        path: child,
                        kind: ChangeKind::Removed,
                        old: Some(p.clone()),
                        new: None,
                    }),
                    (None, Some(q)) => self.push(RawChange {
                        path: child,
                        kind: ChangeKind::Added,
                        old: None,
                        new: Some(q.clone()),
                    }),
                    (None, None) => {}
                }
            }
            return;
        };
        let pos_a: BTreeMap<&str, usize> = x
            .iter()
            .enumerate()
            .map(|(i, v)| (id_of(v, key), i))
            .collect();
        let pos_b: BTreeMap<&str, usize> = y
            .iter()
            .enumerate()
            .map(|(i, v)| (id_of(v, key), i))
            .collect();
        // Elements in the order of the first side, then those only in the second.
        for item in x {
            let id = id_of(item, key);
            let child = format!("{path}[{key}={id}]");
            match pos_b.get(id) {
                Some(&j) => self.walk(&child, item, &y[j]),
                None => self.push(RawChange {
                    path: child,
                    kind: ChangeKind::Removed,
                    old: Some(item.clone()),
                    new: None,
                }),
            }
        }
        for item in y {
            let id = id_of(item, key);
            if !pos_a.contains_key(id) {
                self.push(RawChange {
                    path: format!("{path}[{key}={id}]"),
                    kind: ChangeKind::Added,
                    old: None,
                    new: Some(item.clone()),
                });
            }
        }
        // Moves: among the elements both sides have, those outside a longest common run of the
        // two orders changed place.
        let common_a: Vec<&str> = x
            .iter()
            .map(|v| id_of(v, key))
            .filter(|id| pos_b.contains_key(id))
            .collect();
        let common_b: Vec<&str> = y
            .iter()
            .map(|v| id_of(v, key))
            .filter(|id| pos_a.contains_key(id))
            .collect();
        if common_a == common_b || common_a.len() > MAX_MOVE_SEARCH {
            return;
        }
        let stayed = longest_common_subsequence(&common_a, &common_b);
        for id in common_a.iter().filter(|id| !stayed.contains(*id)) {
            self.push(RawChange {
                path: format!("{path}[{key}={id}]"),
                kind: ChangeKind::Moved,
                old: Some(Value::from(pos_a[id])),
                new: Some(Value::from(pos_b[id])),
            });
        }
    }
}

/// The ids in a longest common subsequence of two lists of distinct ids.
fn longest_common_subsequence<'a>(
    a: &[&'a str],
    b: &[&'a str],
) -> std::collections::BTreeSet<&'a str> {
    let mut table = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            table[i][j] = if a[i] == b[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut kept = std::collections::BTreeSet::new();
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            kept.insert(a[i]);
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    kept
}

/// Every difference between two values, as paths with old and new values. Returns the number of
/// differences beyond the limit that were counted but not listed.
pub fn diff_values(a: &Value, b: &Value, limit: usize) -> (Vec<RawChange>, usize) {
    let mut out = Vec::new();
    let mut walker = Walker {
        out: &mut out,
        limit,
        not_listed: 0,
    };
    if !values_equal(a, b) {
        walker.walk("", a, b);
    }
    let not_listed = walker.not_listed;
    (out, not_listed)
}

// ---------------------------------------------------------------------------
// Building a report

/// Collects the parts of a comparison and puts them into a [`DiffReport`].
pub struct DiffBuilder {
    a: String,
    b: String,
    compared: String,
    inputs: Vec<Change>,
    evidence: Vec<Change>,
    presentation: Vec<Change>,
    results: Vec<ResultEntry>,
    components: Vec<ComponentChange>,
    not_listed: usize,
}

impl DiffBuilder {
    pub fn new(a: impl Into<String>, b: impl Into<String>, compared: impl Into<String>) -> Self {
        Self {
            a: a.into(),
            b: b.into(),
            compared: compared.into(),
            inputs: Vec::new(),
            evidence: Vec::new(),
            presentation: Vec::new(),
            results: Vec::new(),
            components: Vec::new(),
            not_listed: 0,
        }
    }

    fn place(&mut self, section: Section, mut change: Change) {
        let (list, prefix) = match section {
            Section::Inputs => (&mut self.inputs, "i"),
            Section::Evidence => (&mut self.evidence, "e"),
            Section::Presentation => (&mut self.presentation, "p"),
        };
        change.id = format!("{prefix}{}", list.len() + 1);
        list.push(change);
    }

    /// The structural difference of one input document. `scope` says where it applies (see
    /// [`Change::scope`]). A document on one side only is an addition or a removal of the whole.
    pub fn input_document(
        &mut self,
        name: &str,
        kind: DocKind,
        scope: &str,
        a: Option<&Value>,
        b: Option<&Value>,
    ) {
        let (raw, skipped) = match (a, b) {
            (Some(a), Some(b)) => {
                let (raw, skipped) = diff_values(a, b, MAX_CHANGES_PER_DOCUMENT);
                if kind == DocKind::Scenario || kind == DocKind::TransportInput {
                    self.components
                        .extend(components::component_changes(name, a, b));
                }
                (raw, skipped)
            }
            (Some(a), None) => (
                vec![RawChange {
                    path: String::new(),
                    kind: ChangeKind::Removed,
                    old: Some(a.clone()),
                    new: None,
                }],
                0,
            ),
            (None, Some(b)) => (
                vec![RawChange {
                    path: String::new(),
                    kind: ChangeKind::Added,
                    old: None,
                    new: Some(b.clone()),
                }],
                0,
            ),
            (None, None) => (Vec::new(), 0),
        };
        self.not_listed += skipped;
        for change in raw {
            let rule = classify(kind, &change.path);
            let unit = unit_of(&change.path).map(String::from);
            let built = Change {
                id: String::new(),
                document: name.to_string(),
                path: change.path,
                kind: change.kind,
                old: change.old,
                new: change.new,
                unit,
                cost: rule.cost,
                invalidates: rule.cost.affects().to_vec(),
                scope: scope.to_string(),
                note: rule.note.map(String::from),
            };
            self.place(rule.section, built);
        }
    }

    /// An input known only by its hash (a script, a file whose content is not read).
    pub fn hashed_input(
        &mut self,
        name: &str,
        scope: &str,
        a: Option<&str>,
        b: Option<&str>,
        cost: CostClass,
        note: &str,
    ) {
        if a == b {
            return;
        }
        self.place(Section::Inputs, hash_change(name, scope, a, b, cost, note));
    }

    /// A piece of evidence (a Core archive, a library audit), compared by value or by hash.
    pub fn evidence_document(
        &mut self,
        name: &str,
        scope: &str,
        a: Option<&Value>,
        b: Option<&Value>,
    ) {
        let (raw, skipped) = match (a, b) {
            (Some(a), Some(b)) => diff_values(a, b, MAX_CHANGES_PER_DOCUMENT),
            (Some(a), None) => (
                vec![RawChange {
                    path: String::new(),
                    kind: ChangeKind::Removed,
                    old: Some(a.clone()),
                    new: None,
                }],
                0,
            ),
            (None, Some(b)) => (
                vec![RawChange {
                    path: String::new(),
                    kind: ChangeKind::Added,
                    old: None,
                    new: Some(b.clone()),
                }],
                0,
            ),
            (None, None) => (Vec::new(), 0),
        };
        self.not_listed += skipped;
        for change in raw {
            let built = Change {
                id: String::new(),
                document: name.to_string(),
                unit: unit_of(&change.path).map(String::from),
                path: change.path,
                kind: change.kind,
                old: change.old,
                new: change.new,
                cost: CostClass::Seconds,
                invalidates: vec!["evidence"],
                scope: scope.to_string(),
                note: Some("the Core evidence is checked again".into()),
            };
            self.place(Section::Evidence, built);
        }
    }

    /// Evidence known only by its hash.
    pub fn hashed_evidence(&mut self, name: &str, scope: &str, a: Option<&str>, b: Option<&str>) {
        if a == b {
            return;
        }
        let mut change = hash_change(
            name,
            scope,
            a,
            b,
            CostClass::Seconds,
            "the Core evidence is checked again",
        );
        change.invalidates = vec!["evidence"];
        self.place(Section::Evidence, change);
    }

    /// A presentation-only document (the view, who saved the file).
    pub fn presentation_document(&mut self, name: &str, a: Option<&Value>, b: Option<&Value>) {
        if let (Some(a), Some(b)) = (a, b) {
            let (raw, skipped) = diff_values(a, b, MAX_CHANGES_PER_DOCUMENT);
            self.not_listed += skipped;
            for change in raw {
                let built = Change {
                    id: String::new(),
                    document: name.to_string(),
                    unit: unit_of(&change.path).map(String::from),
                    path: change.path,
                    kind: change.kind,
                    old: change.old,
                    new: change.new,
                    cost: CostClass::Instant,
                    invalidates: vec![],
                    scope: String::new(),
                    note: None,
                };
                self.place(Section::Presentation, built);
            }
        }
    }

    /// A result, with the hashes of its content on each side.
    pub fn result(&mut self, entry: ResultEntry) {
        self.results.push(entry);
    }

    pub fn finish(mut self) -> DiffReport {
        let inputs = std::mem::take(&mut self.inputs);
        let evidence = std::mem::take(&mut self.evidence);
        let causes: Vec<&Change> = inputs.iter().chain(evidence.iter()).collect();
        let explains = |entry: &ResultEntry, change: &Change| {
            change.invalidates.contains(&entry.kind.tag()) && scope_reaches(entry, &change.scope)
        };
        let mut results = Vec::new();
        let mut impact = Vec::new();
        for entry in &self.results {
            let item = format!("{}/{}", entry.group, entry.name);
            let because: Vec<String> = causes
                .iter()
                .filter(|c| explains(entry, c))
                .map(|c| c.id.clone())
                .collect();
            let kind = match (&entry.old_sha256, &entry.new_sha256) {
                (Some(a), Some(b)) if a == b => None,
                (Some(_), Some(_)) => Some(ChangeKind::Changed),
                (Some(_), None) => Some(ChangeKind::Removed),
                (None, Some(_)) => Some(ChangeKind::Added),
                (None, None) => None,
            };
            if let Some(kind) = kind {
                results.push(ResultChange {
                    id: format!("r{}", results.len() + 1),
                    item: item.clone(),
                    kind,
                    old_sha256: entry.old_sha256.clone(),
                    new_sha256: entry.new_sha256.clone(),
                    cause_note: because
                        .is_empty()
                        .then(|| "no input change found".to_string()),
                    caused_by: because.clone(),
                    values: entry.values.clone(),
                });
            }
            let cost = causes
                .iter()
                .filter(|c| explains(entry, c))
                .map(|c| c.cost)
                .max();
            // An invalidated result needs the cost of getting it again: the dearest cause decides.
            impact.push(ImpactItem {
                item,
                standing: if because.is_empty() {
                    Standing::Reusable
                } else {
                    Standing::Invalidated
                },
                cost,
                because,
            });
        }
        let highest_cost = causes.iter().map(|c| c.cost).max();
        let summary = Summary {
            inputs_changed: inputs.len(),
            evidence_changed: evidence.len(),
            results_changed: results.len(),
            presentation_changed: self.presentation.len(),
            highest_cost,
            invalidated: impact
                .iter()
                .filter(|i| i.standing == Standing::Invalidated)
                .count(),
            reusable: impact
                .iter()
                .filter(|i| i.standing == Standing::Reusable)
                .count(),
            not_listed: self.not_listed,
        };
        let identical = summary.inputs_changed == 0
            && summary.evidence_changed == 0
            && summary.results_changed == 0
            && summary.presentation_changed == 0
            && self.components.is_empty()
            && summary.not_listed == 0;
        DiffReport {
            schema: DIFF_SCHEMA,
            a: self.a,
            b: self.b,
            compared: self.compared,
            identical,
            summary,
            inputs,
            evidence,
            results,
            presentation: self.presentation,
            components: self.components,
            impact,
        }
    }
}

fn hash_change(
    name: &str,
    scope: &str,
    a: Option<&str>,
    b: Option<&str>,
    cost: CostClass,
    note: &str,
) -> Change {
    let kind = match (a, b) {
        (Some(_), Some(_)) => ChangeKind::Changed,
        (Some(_), None) => ChangeKind::Removed,
        _ => ChangeKind::Added,
    };
    Change {
        id: String::new(),
        document: name.to_string(),
        path: "sha256".into(),
        kind,
        old: a.map(|s| Value::String(s.into())),
        new: b.map(|s| Value::String(s.into())),
        unit: None,
        cost,
        invalidates: cost.affects().to_vec(),
        scope: scope.to_string(),
        note: Some(note.to_string()),
    }
}

/// Whether a change with this scope can reach the result.
fn scope_reaches(entry: &ResultEntry, scope: &str) -> bool {
    if scope.is_empty() {
        return true;
    }
    let (group, name) = scope.split_once('/').unwrap_or((scope, ""));
    match entry.kind {
        // The history is recalculated from every allocation's rates; an ensemble belongs to the
        // variant it names; evidence receipts follow the arrangement.
        ResultKind::History => true,
        ResultKind::Ensemble => name.is_empty() || name == entry.name,
        ResultKind::Transport | ResultKind::EvidenceReceipt => {
            group == entry.group && (name.is_empty() || name == entry.name)
        }
    }
}

/// The leaf-level differences of a result document (a transport artifact, a history), listed
/// with old and new values. They are the effects: they carry no cost class and invalidate
/// nothing. At most `limit` are returned; the second value counts the rest.
pub fn result_changes(document: &str, a: &Value, b: &Value, limit: usize) -> (Vec<Change>, usize) {
    let (raw, skipped) = diff_values(a, b, limit);
    let changes = raw
        .into_iter()
        .enumerate()
        .map(|(i, change)| Change {
            id: format!("v{}", i + 1),
            document: document.to_string(),
            unit: unit_of(&change.path).map(String::from),
            path: change.path,
            kind: change.kind,
            old: change.old,
            new: change.new,
            cost: CostClass::Instant,
            invalidates: vec![],
            scope: String::new(),
            note: None,
        })
        .collect();
    (changes, skipped)
}

/// The canonical text of a value, for hashing a result's content on both sides the same way.
pub fn content_text(value: &Value) -> String {
    canonical_json(value)
}

/// Diff two standalone model documents (scenario, physics case, operating assumptions, design
/// file): the same report as a study diff, with the one document as the only input.
pub fn diff_models(a_label: &str, a: &Value, b_label: &str, b: &Value) -> DiffReport {
    let kind = DocKind::detect(a);
    let other = DocKind::detect(b);
    let mut builder = DiffBuilder::new(
        a_label,
        b_label,
        if kind == other {
            kind.name().to_string()
        } else {
            format!("{} against {}", kind.name(), other.name())
        },
    );
    builder.input_document(kind.name(), kind, "", Some(a), Some(b));
    builder.finish()
}
