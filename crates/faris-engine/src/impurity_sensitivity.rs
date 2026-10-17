//! Impurity sensitivity: which trace elements of a material matter most for activation, decay
//! heat and waste class (requirements ACT-025 and MAT-040).
//!
//! At a fixed neutron flux the inventory is linear in the starting composition to the extent the
//! starting nuclides are not burned up, which holds for trace elements (a part per million is
//! consumed by a part per million of itself, and cross-feeding between impurities is a second-order
//! term). That is why a per-element response can come from its own run: one run of the matrix with
//! only that element added gives the response per weight ppm, and the sum over elements rebuilds
//! the run with all of them. This module does not assume it. A second, independent estimate is a
//! central finite difference around the full specification (all other impurities present), and
//! the two are compared per element; the result carries the comparison and whether it passed.
//!
//! All runs go to ACTINV in one request as separate zones, through `activation::run_activation`
//! (same bounded job runner, same cache). Half-lives for the waste classification come from the
//! same decay data ACTINV used, through `actinv inspect decay`. Nothing here is a transport or
//! regulatory result: the numbers are screening values for the composition and flux given.

use crate::activation::{
    ActivationConfig, ActivationRequest, CompositionBasis, CoolingPoint, GroupFlux, Kind,
    PhotonOptions, SolverMode, ZoneInput, ZoneResult, run_activation,
};
use crate::jobs::{Cancellation, ExecutionStatus, JobSpec, run_job};
use crate::maintenance::actinv::{resolve_program, tail_of};
use crate::maintenance::irradiation::IrradiationStep;
use faris_model::waste::classify::{ColumnSums, classify_nrc, required_nuclides};
use faris_model::waste::{NuclideActivity, WasteForm, WasteInventory};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const RESULT_SCHEMA: &str = "faris-impurity-sensitivity/v0.1";
/// The finite difference and the single-element response must agree within this relative amount.
pub const AGREEMENT_TOLERANCE: f64 = 0.20;
/// A response smaller than this fraction of the full-specification total is below what the check
/// resolves; it is ranked but left out of the agreement test and said so.
pub const RESOLUTION_FLOOR: f64 = 1.0e-10;

const ACTINIDES: [&str; 12] = [
    "Ac", "Th", "Pa", "U", "Np", "Pu", "Am", "Cm", "Bk", "Cf", "Es", "Fm",
];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Impurity {
    pub element: String,
    pub wt_ppm: f64,
    /// `nominal`, `specified`, `measured` or `assumed` (MAT-040).
    pub kind: String,
    pub source: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SensitivityRequest {
    /// Alloy elements in weight percent, without the impurities; scaled to leave room for them.
    pub matrix_wt_percent: BTreeMap<String, f64>,
    pub impurities: Vec<Impurity>,
    pub mass_g: f64,
    pub volume_m3: f64,
    pub spectrum: GroupFlux,
    pub steps: Vec<IrradiationStep>,
    /// Cooling times since shutdown, seconds. Every response is reported at each of them.
    pub cooling_s: Vec<f64>,
    /// Relative step of the finite difference: concentration x (1 -/+ h). Default 0.5.
    #[serde(default = "default_step")]
    pub relative_step: f64,
}

fn default_step() -> f64 {
    0.5
}

/// The sums of fractions of 10 CFR 61.55 at a cooling time, and the one number the ranking uses.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct WasteSums {
    pub table1: f64,
    pub table2_col1: f64,
    pub table2_col2: f64,
    pub table2_col3: f64,
}

impl WasteSums {
    /// Table 1 plus Table 2 column 3: both must stay at or under 1 for Class C, and both grow with
    /// the inventory, so their sum is a single linear measure of the burden on the class limits.
    pub fn class_c_burden(&self) -> f64 {
        self.table1 + self.table2_col3
    }
}

impl From<ColumnSums> for WasteSums {
    fn from(s: ColumnSums) -> Self {
        Self {
            table1: s.table1,
            table2_col1: s.table2_col1,
            table2_col2: s.table2_col2,
            table2_col3: s.table2_col3,
        }
    }
}

/// What one run gives at one cooling time.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Response {
    pub activity_bq: f64,
    pub decay_heat_w_per_m3: f64,
    /// None when the classifier refused (see `waste_not_evaluated` on the point).
    pub waste: Option<WasteSums>,
}

impl Response {
    fn waste_burden(&self) -> Option<f64> {
        self.waste.map(|w| w.class_c_burden())
    }
}

/// Per weight ppm of one element, or an effect at its specified content.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Slope {
    pub activity_bq: f64,
    pub decay_heat_w_per_m3: f64,
    pub waste_class_c_burden: Option<f64>,
    pub waste: Option<WasteSums>,
}

impl Slope {
    fn between(a: &Response, b: &Response, d_ppm: f64) -> Self {
        let waste = match (a.waste, b.waste) {
            (Some(a), Some(b)) => Some(WasteSums {
                table1: (a.table1 - b.table1) / d_ppm,
                table2_col1: (a.table2_col1 - b.table2_col1) / d_ppm,
                table2_col2: (a.table2_col2 - b.table2_col2) / d_ppm,
                table2_col3: (a.table2_col3 - b.table2_col3) / d_ppm,
            }),
            _ => None,
        };
        Self {
            activity_bq: (a.activity_bq - b.activity_bq) / d_ppm,
            decay_heat_w_per_m3: (a.decay_heat_w_per_m3 - b.decay_heat_w_per_m3) / d_ppm,
            waste_class_c_burden: waste.map(|w| w.class_c_burden()),
            waste,
        }
    }

    fn scaled(&self, k: f64) -> Self {
        Self {
            activity_bq: self.activity_bq * k,
            decay_heat_w_per_m3: self.decay_heat_w_per_m3 * k,
            waste_class_c_burden: self.waste_class_c_burden.map(|v| v * k),
            waste: self.waste.map(|w| WasteSums {
                table1: w.table1 * k,
                table2_col1: w.table2_col1 * k,
                table2_col2: w.table2_col2 * k,
                table2_col3: w.table2_col3 * k,
            }),
        }
    }

    fn metric(&self, m: Metric) -> Option<f64> {
        match m {
            Metric::Activity => Some(self.activity_bq),
            Metric::DecayHeat => Some(self.decay_heat_w_per_m3),
            Metric::Waste => self.waste_class_c_burden,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    Activity,
    DecayHeat,
    Waste,
}

impl Metric {
    const ALL: [Metric; 3] = [Metric::Activity, Metric::DecayHeat, Metric::Waste];
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ElementResult {
    pub element: String,
    pub wt_ppm: f64,
    pub kind: String,
    pub source: String,
    /// Response per weight ppm from the run with only this element added to the matrix.
    pub per_ppm: Slope,
    /// Central finite difference per weight ppm around the full specification.
    pub finite_difference_per_ppm: Slope,
    /// `per_ppm` times the specified content: the element's share of the full-specification response.
    pub effect_at_specified: Slope,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RankLine {
    pub rank: usize,
    pub element: String,
    /// The effect at the specified content this rank is by (absolute value).
    pub effect: f64,
    /// False when the effect is below the resolution of the agreement check.
    pub resolved: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Agreement {
    pub metric: Metric,
    pub elements_compared: usize,
    pub elements_below_resolution: Vec<String>,
    /// Largest |per_ppm - finite difference| over the larger of the two, over compared elements.
    pub max_relative_difference: f64,
    pub worst_element: Option<String>,
    /// Pairs the two estimates order differently, with whether the primary effects are within the tolerance.
    pub discordant_pairs: Vec<DiscordantPair>,
    pub passed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DiscordantPair {
    pub a: String,
    pub b: String,
    pub effects_within_tolerance: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PointResult {
    pub cooling_s: f64,
    /// All impurities at their specified content.
    pub full_specification: Response,
    /// The matrix alone.
    pub matrix_only: Response,
    pub elements: Vec<ElementResult>,
    pub activity_ranking: Vec<RankLine>,
    pub decay_heat_ranking: Vec<RankLine>,
    pub waste_ranking: Vec<RankLine>,
    /// Mean of the three ranks, lowest first.
    pub overall_ranking: Vec<(usize, String, f64)>,
    pub agreement: Vec<Agreement>,
    pub waste_not_evaluated: Option<String>,
    /// Nuclide states the classifier's names cannot hold, and how they were listed to it.
    pub waste_notes: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SensitivityResult {
    pub schema: String,
    pub research_screening: String,
    pub solver: String,
    pub runs: usize,
    pub relative_step: f64,
    pub impurities: Vec<Impurity>,
    /// `calculated`, or `conditional` when a data gap makes some zone a lower bound.
    pub kind: Kind,
    pub lower_bound_zones: Vec<String>,
    pub linearity_basis: String,
    pub points: Vec<PointResult>,
}

// -------------------------------------------------------------------- input --

fn validate(r: &SensitivityRequest) -> Result<(), String> {
    if r.impurities.is_empty() {
        return Err("no impurities were given".into());
    }
    if !(r.relative_step > 0.0 && r.relative_step < 1.0) {
        return Err("relative_step must be between 0 and 1".into());
    }
    let mut seen = BTreeSet::new();
    for i in &r.impurities {
        if i.element.is_empty() || i.element.chars().any(|c| c.is_ascii_digit()) {
            return Err(format!(
                "impurity `{}` must be an element symbol such as Nb",
                i.element
            ));
        }
        if !seen.insert(i.element.clone()) {
            return Err(format!("impurity {} is listed twice", i.element));
        }
        if r.matrix_wt_percent.contains_key(&i.element) {
            return Err(format!(
                "{} is in the matrix and also an impurity; give it once",
                i.element
            ));
        }
        if !(i.wt_ppm.is_finite() && i.wt_ppm > 0.0) {
            return Err(format!("impurity {} needs a positive wt_ppm", i.element));
        }
        if i.source.trim().is_empty() || i.kind.trim().is_empty() {
            return Err(format!(
                "impurity {} needs a kind and a source (MAT-040)",
                i.element
            ));
        }
    }
    let total_ppm: f64 = r.impurities.iter().map(|i| i.wt_ppm).sum();
    if total_ppm >= 1.0e4 {
        return Err(
            "the impurities add up to 1 wt% or more; this is a trace-element screen".into(),
        );
    }
    if r.matrix_wt_percent.is_empty()
        || r.matrix_wt_percent
            .values()
            .any(|v| !v.is_finite() || *v < 0.0)
    {
        return Err(
            "the matrix needs at least one element with a nonnegative weight percent".into(),
        );
    }
    Ok(())
}

/// Matrix scaled to `100 - sum(impurities)` weight percent, plus each impurity.
fn composition(r: &SensitivityRequest, ppm: &BTreeMap<&str, f64>) -> BTreeMap<String, f64> {
    let impurity_pct: f64 = ppm.values().sum::<f64>() / 1.0e4;
    let matrix_sum: f64 = r.matrix_wt_percent.values().sum();
    let scale = (100.0 - impurity_pct) / matrix_sum;
    let mut c: BTreeMap<String, f64> = r
        .matrix_wt_percent
        .iter()
        .map(|(k, v)| (k.clone(), v * scale))
        .collect();
    for (k, v) in ppm {
        if *v > 0.0 {
            c.insert((*k).to_string(), v / 1.0e4);
        }
    }
    c
}

// ---------------------------------------------------------------- half-lives --

#[derive(Clone, Copy, Debug)]
struct DecayInfo {
    half_life_s: f64,
    alpha: bool,
}

/// FARIS name of an ACTINV nuclide: `Co60` to `Co-60`, `Co58m1` to `Co-58m`. A higher isomer
/// (`Hf179m2`) has no name of its own in the classifier; it takes the `m` name and the flag says
/// it was folded.
fn faris_name(actinv: &str) -> Option<(String, bool)> {
    let split = actinv.find(|c: char| c.is_ascii_digit())?;
    let (symbol, rest) = actinv.split_at(split);
    let (mass, isomer, folded) = match rest.split_once('m') {
        Some((mass, "1")) => (mass, "m", false),
        Some((mass, _)) => (mass, "m", true),
        None => (rest, "", false),
    };
    Some((format!("{symbol}-{mass}{isomer}"), folded))
}

fn actinv_name(faris: &str) -> String {
    let (symbol, rest) = faris.split_once('-').unwrap_or((faris, ""));
    match rest.strip_suffix('m') {
        Some(mass) => format!("{symbol}{mass}m1"),
        None => format!("{symbol}{rest}"),
    }
}

/// Half-lives and alpha branching from one decay file, and the names the file does not hold.
/// `actinv inspect decay` stops at the first nuclide it lacks, so a lacking one is taken out and
/// the file asked again.
fn decay_from_file(
    config: &ActivationConfig,
    file: &std::path::Path,
    names: &BTreeSet<String>,
    cancel: &Cancellation,
) -> Result<(BTreeMap<String, DecayInfo>, BTreeSet<String>), String> {
    let actinv = resolve_program(&config.actinv)?;
    let out = config.work_dir.join("decay-info.json");
    std::fs::create_dir_all(&config.work_dir)
        .map_err(|e| format!("{}: {e}", config.work_dir.display()))?;
    let mut asking = names.clone();
    let mut missing = BTreeSet::new();
    loop {
        if asking.is_empty() {
            return Ok((BTreeMap::new(), missing));
        }
        let mut arguments: Vec<std::ffi::OsString> =
            vec!["inspect".into(), "decay".into(), file.into()];
        arguments.extend(asking.iter().map(|n| n.into()));
        arguments.push("--out".into());
        arguments.push(out.clone().into());
        let spec = JobSpec {
            program: actinv.clone(),
            arguments,
            working_directory: config.work_dir.clone(),
            environment: ["PATH", "HOME", "LANG", "LC_ALL", "TMPDIR"]
                .into_iter()
                .filter_map(|k| std::env::var_os(k).map(|v| (k.into(), v)))
                .collect(),
            timeout: config.timeout,
            capture_limit_bytes: 1 << 20,
            artifact_roots: Vec::new(),
            resource_limits: config.resource_limits,
        };
        let outcome = run_job(&spec, cancel).map_err(|e| format!("actinv inspect decay: {e}"))?;
        if outcome.execution_status != ExecutionStatus::Succeeded {
            let text = format!("{}\n{}", outcome.stdout, outcome.stderr);
            let lacking = asking
                .iter()
                .find(|n| text.contains(&format!("{n} (ZA=")))
                .cloned();
            match lacking {
                Some(n) => {
                    asking.remove(&n);
                    missing.insert(n);
                    continue;
                }
                None => return Err(format!("actinv inspect decay failed: {}", tail_of(&text))),
            }
        }
        let bytes = std::fs::read(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        let _ = std::fs::remove_file(&out);
        let v: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        let mut map = BTreeMap::new();
        for n in v["nuclides"].as_array().into_iter().flatten() {
            let (Some(name), Some(hl)) = (n["nuclide"].as_str(), n["half_life_s"].as_f64()) else {
                continue;
            };
            if !(hl.is_finite() && hl > 0.0) {
                continue;
            }
            let alpha = n["modes"].as_array().into_iter().flatten().any(|m| {
                m["type"].as_str() == Some("alpha") && m["branching_ratio"].as_f64() > Some(0.5)
            });
            map.insert(
                name.to_string(),
                DecayInfo {
                    half_life_s: hl,
                    alpha,
                },
            );
        }
        return Ok((map, missing));
    }
}

/// Half-lives from the decay data ACTINV used: the primary file, then the fallback for what the
/// primary lacks. A nuclide in neither is absent from the result.
fn decay_info(
    config: &ActivationConfig,
    names: &BTreeSet<String>,
    cancel: &Cancellation,
) -> Result<BTreeMap<String, DecayInfo>, String> {
    let (mut map, missing) = decay_from_file(config, &config.data.decay_primary, names, cancel)?;
    if let Some((fallback, _)) = &config.data.decay_fallback {
        let (more, _) = decay_from_file(config, fallback, &missing, cancel)?;
        map.extend(more);
    }
    Ok(map)
}

// ----------------------------------------------------------------- responses --

fn has_actinide(zone: &ZoneInput) -> bool {
    zone.composition.keys().any(|k| {
        let symbol: String = k.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
        ACTINIDES.contains(&symbol.as_str())
    })
}

/// The 10 CFR 61.55 sums of one cooling point as activated metal. Err is the reason the
/// classifier would not state them.
fn waste_sums(
    zone: &ZoneInput,
    point: &CoolingPoint,
    decay: &BTreeMap<String, DecayInfo>,
) -> Result<(WasteSums, Vec<String>), String> {
    let mut nuclides: BTreeMap<String, NuclideActivity> = BTreeMap::new();
    for name in required_nuclides(WasteForm::ActivatedMetal) {
        let info = decay.get(&actinv_name(&name)).ok_or_else(|| {
            format!("{name}: no half-life in the decay data, so it cannot be listed")
        })?;
        nuclides.insert(
            name.clone(),
            NuclideActivity {
                nuclide: name,
                activity_bq: 0.0,
                half_life_s: info.half_life_s,
                alpha_emitter: Some(info.alpha),
                progeny_of: None,
            },
        );
    }
    let short = 5.0 * faris_model::waste::YEAR_S;
    let listed = required_nuclides(WasteForm::ActivatedMetal);
    let mut notes = Vec::new();
    for n in point.nuclides.iter().filter(|n| n.activity_bq > 0.0) {
        let (name, folded) =
            faris_name(&n.nuclide).ok_or_else(|| format!("{}: not a nuclide name", n.nuclide))?;
        let info = decay
            .get(&n.nuclide)
            .ok_or_else(|| format!("{}: no half-life in the decay data", n.nuclide))?;
        // The classifier names hold one state per mass number and a single `m`. A higher isomer
        // takes the `m` name, or the plain mass name when another state is there across the
        // 5-year half-life that the short-lived total of Table 2 turns on. Only the half-life
        // reaches a rule for such a nuclide, as no table lists it; a listed name is never shared.
        let plain = name.trim_end_matches('m').to_string();
        let mut candidates = vec![name.clone()];
        if name != plain {
            candidates.push(plain);
        }
        let mut placed = false;
        for (i, candidate) in candidates.iter().enumerate() {
            if (folded || i > 0) && listed.contains(candidate) {
                continue;
            }
            match nuclides.get_mut(candidate) {
                Some(e) if (e.half_life_s < short) == (info.half_life_s < short) => {
                    e.activity_bq += n.activity_bq;
                    e.half_life_s = e.half_life_s.max(info.half_life_s);
                }
                Some(_) => continue,
                None => {
                    nuclides.insert(
                        candidate.clone(),
                        NuclideActivity {
                            nuclide: candidate.clone(),
                            activity_bq: n.activity_bq,
                            half_life_s: info.half_life_s,
                            alpha_emitter: Some(info.alpha),
                            progeny_of: None,
                        },
                    );
                }
            }
            if *candidate != faris_name(&n.nuclide).map(|f| f.0).unwrap_or_default() || folded {
                notes.push(format!(
                    "{} is listed to the classifier as {candidate}: only its half-life reaches a rule",
                    n.nuclide
                ));
            }
            placed = true;
            break;
        }
        if !placed {
            return Err(format!(
                "{}: no classifier name is free for it on its side of the 5-year half-life",
                n.nuclide
            ));
        }
    }
    let years = point.cooling_s / faris_model::units::JULIAN_YEAR_S;
    let inventory = WasteInventory {
        component_id: zone.id.clone(),
        waste_form: WasteForm::ActivatedMetal,
        volume_m3: Some(zone.volume_m3),
        mass_g: Some(zone.mass_g),
        inventory_time_years: years,
        // No actinide target in the composition, so no transuranic nuclide can be produced.
        transuranics_evaluated: !has_actinide(zone),
        inventory_kind: "solver_result".into(),
        nuclides: nuclides.into_values().collect(),
    };
    let r = classify_nrc(&inventory, years);
    match r.not_evaluated {
        Some(ne) => Err(ne.reason),
        None => Ok((r.sums.into(), notes)),
    }
}

fn response(
    zone: &ZoneInput,
    result: &ZoneResult,
    index: usize,
    decay: &BTreeMap<String, DecayInfo>,
) -> (Response, Option<String>, Vec<String>) {
    let point = &result.points[index];
    let (waste, why, notes) = match waste_sums(zone, point, decay) {
        Ok((w, notes)) => (Some(w), None, notes),
        Err(e) => (None, Some(e), Vec::new()),
    };
    (
        Response {
            activity_bq: point.activity_bq,
            decay_heat_w_per_m3: point.decay_heat.total_w_per_m3,
            waste,
        },
        why,
        notes,
    )
}

fn rank(
    elements: &[ElementResult],
    metric: Metric,
    nominal_total: f64,
) -> (Vec<RankLine>, BTreeMap<String, usize>) {
    let mut lines: Vec<(String, f64)> = elements
        .iter()
        .map(|e| {
            (
                e.element.clone(),
                e.effect_at_specified.metric(metric).unwrap_or(0.0).abs(),
            )
        })
        .collect();
    lines.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let out: Vec<RankLine> = lines
        .iter()
        .enumerate()
        .map(|(i, (element, effect))| RankLine {
            rank: i + 1,
            element: element.clone(),
            effect: *effect,
            resolved: *effect > RESOLUTION_FLOOR * nominal_total.abs(),
        })
        .collect();
    let by = out.iter().map(|l| (l.element.clone(), l.rank)).collect();
    (out, by)
}

fn agreement(elements: &[ElementResult], metric: Metric, nominal_total: f64) -> Agreement {
    // (element, effect by the primary estimate, effect by the finite difference, both slopes)
    let mut compared = Vec::new();
    let mut below = Vec::new();
    for e in elements {
        let (Some(p), Some(f)) = (
            e.per_ppm.metric(metric),
            e.finite_difference_per_ppm.metric(metric),
        ) else {
            continue;
        };
        let effect = (p * e.wt_ppm).abs();
        if effect > RESOLUTION_FLOOR * nominal_total.abs() {
            compared.push((e.element.as_str(), effect, (f * e.wt_ppm).abs(), p, f));
        } else {
            below.push(e.element.clone());
        }
    }
    let mut worst = (0.0_f64, None);
    for (name, _, _, p, f) in &compared {
        let scale = p.abs().max(f.abs());
        let d = if scale > 0.0 {
            (p - f).abs() / scale
        } else {
            0.0
        };
        if d > worst.0 || worst.1.is_none() {
            worst = (d, Some((*name).to_string()));
        }
    }
    let mut pairs = Vec::new();
    for (i, a) in compared.iter().enumerate() {
        for b in &compared[i + 1..] {
            if (a.1 - b.1) * (a.2 - b.2) < 0.0 {
                pairs.push(DiscordantPair {
                    a: a.0.to_string(),
                    b: b.0.to_string(),
                    effects_within_tolerance: (a.1 - b.1).abs()
                        <= AGREEMENT_TOLERANCE * a.1.max(b.1),
                });
            }
        }
    }
    let passed = worst.0 <= AGREEMENT_TOLERANCE && pairs.iter().all(|p| p.effects_within_tolerance);
    Agreement {
        metric,
        elements_compared: compared.len(),
        elements_below_resolution: below,
        max_relative_difference: worst.0,
        worst_element: worst.1,
        discordant_pairs: pairs,
        passed,
    }
}

// ----------------------------------------------------------------------- run --

/// Runs the matrix, the full specification, one run per element alone and two per element for the
/// finite difference, in one ACTINV request, and ranks the elements.
pub fn impurity_sensitivity(
    request: &SensitivityRequest,
    config: &ActivationConfig,
    cancel: &Cancellation,
) -> Result<SensitivityResult, String> {
    validate(request)?;
    let h = request.relative_step;
    let nominal: BTreeMap<&str, f64> = request
        .impurities
        .iter()
        .map(|i| (i.element.as_str(), i.wt_ppm))
        .collect();
    let mut runs: Vec<(String, BTreeMap<&str, f64>)> = vec![
        ("matrix".into(), BTreeMap::new()),
        ("full".into(), nominal.clone()),
    ];
    for i in &request.impurities {
        let e = i.element.as_str();
        runs.push((format!("only:{e}"), BTreeMap::from([(e, i.wt_ppm)])));
        for (tag, factor) in [("lo", 1.0 - h), ("hi", 1.0 + h)] {
            let mut m = nominal.clone();
            m.insert(e, i.wt_ppm * factor);
            runs.push((format!("{tag}:{e}"), m));
        }
    }
    let zones: Vec<ZoneInput> = runs
        .iter()
        .map(|(id, ppm)| ZoneInput {
            id: id.clone(),
            mass_g: request.mass_g,
            volume_m3: request.volume_m3,
            basis: CompositionBasis::WtPercent,
            composition: composition(request, ppm),
            spectrum: request.spectrum.clone(),
        })
        .collect();
    let act = ActivationRequest {
        zones: zones.clone(),
        steps: request.steps.clone(),
        schedule_input_sha256: None,
        cooling_s: request.cooling_s.clone(),
        photon: PhotonOptions::default(),
        mode: SolverMode::Auto,
    };
    let (result, _) = run_activation(&act, config, cancel)?;
    let by_id: BTreeMap<&str, (&ZoneInput, &ZoneResult)> = zones
        .iter()
        .zip(&result.zones)
        .map(|(z, r)| (z.id.as_str(), (z, r)))
        .collect();

    let mut names: BTreeSet<String> = required_nuclides(WasteForm::ActivatedMetal)
        .iter()
        .map(|n| actinv_name(n))
        .collect();
    for z in &result.zones {
        for p in &z.points {
            names.extend(
                p.nuclides
                    .iter()
                    .filter(|n| n.activity_bq > 0.0)
                    .map(|n| n.nuclide.clone()),
            );
        }
    }
    let decay = decay_info(config, &names, cancel)?;

    let lower_bound_zones: Vec<String> = result
        .zones
        .iter()
        .filter(|z| z.completeness != crate::activation::Completeness::Complete)
        .map(|z| z.id.clone())
        .collect();
    let mut points = Vec::new();
    for (index, &cooling_s) in request.cooling_s.iter().enumerate() {
        let notes = std::cell::RefCell::new(BTreeSet::new());
        let get = |id: &str| {
            let (z, r) = by_id[id];
            let (response, why, n) = response(z, r, index, &decay);
            notes.borrow_mut().extend(n);
            (response, why)
        };
        let (matrix_only, ne_matrix) = get("matrix");
        let (full, ne_full) = get("full");
        let mut waste_ne = ne_matrix.or(ne_full);
        let mut elements = Vec::new();
        for i in &request.impurities {
            let e = i.element.as_str();
            let (only, ne1) = get(&format!("only:{e}"));
            let (lo, ne2) = get(&format!("lo:{e}"));
            let (hi, ne3) = get(&format!("hi:{e}"));
            waste_ne = waste_ne.or(ne1).or(ne2).or(ne3);
            let per_ppm = Slope::between(&only, &matrix_only, i.wt_ppm);
            elements.push(ElementResult {
                element: i.element.clone(),
                wt_ppm: i.wt_ppm,
                kind: i.kind.clone(),
                source: i.source.clone(),
                effect_at_specified: per_ppm.scaled(i.wt_ppm),
                finite_difference_per_ppm: Slope::between(&hi, &lo, 2.0 * h * i.wt_ppm),
                per_ppm,
            });
        }
        let total = |m: Metric| match m {
            Metric::Activity => full.activity_bq,
            Metric::DecayHeat => full.decay_heat_w_per_m3,
            Metric::Waste => full.waste_burden().unwrap_or(0.0),
        };
        let (activity_ranking, a) = rank(&elements, Metric::Activity, total(Metric::Activity));
        let (decay_heat_ranking, d) = rank(&elements, Metric::DecayHeat, total(Metric::DecayHeat));
        let (waste_ranking, w) = rank(&elements, Metric::Waste, total(Metric::Waste));
        let mut overall: Vec<(String, f64)> = elements
            .iter()
            .map(|e| {
                let r = |m: &BTreeMap<String, usize>| m[&e.element] as f64;
                (e.element.clone(), (r(&a) + r(&d) + r(&w)) / 3.0)
            })
            .collect();
        overall.sort_by(|x, y| x.1.total_cmp(&y.1).then_with(|| x.0.cmp(&y.0)));
        let overall_ranking = overall
            .into_iter()
            .enumerate()
            .map(|(i, (e, m))| (i + 1, e, m))
            .collect();
        let agreement = Metric::ALL
            .iter()
            .map(|m| agreement(&elements, *m, total(*m)))
            .collect();
        points.push(PointResult {
            cooling_s,
            full_specification: full,
            matrix_only,
            elements,
            activity_ranking,
            decay_heat_ranking,
            waste_ranking,
            overall_ranking,
            agreement,
            waste_not_evaluated: waste_ne,
            waste_notes: notes.into_inner().into_iter().collect(),
        });
    }
    Ok(SensitivityResult {
        schema: RESULT_SCHEMA.into(),
        research_screening: faris_model::RESEARCH_SCREENING_STATEMENT.into(),
        solver: result.data.solver.clone(),
        runs: runs.len(),
        relative_step: h,
        impurities: request.impurities.clone(),
        kind: if lower_bound_zones.is_empty() {
            Kind::Calculated
        } else {
            Kind::Conditional
        },
        lower_bound_zones,
        linearity_basis: "Responses per weight ppm are linear in the starting composition at a \
            fixed flux for trace elements; the per-element response and a central finite \
            difference around the full specification are both reported and compared."
            .into(),
        points,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activation::{ActivationData, GROUPS};
    use faris_model::math;
    use std::path::PathBuf;
    use std::time::Duration;

    const EDGES: &str = include_str!("../tests/fixtures/activation/fispact709_edges.json");

    #[test]
    fn actinv_and_faris_nuclide_names_convert_both_ways() {
        assert_eq!(faris_name("Co60"), Some(("Co-60".into(), false)));
        assert_eq!(faris_name("Ag110m1"), Some(("Ag-110m".into(), false)));
        assert_eq!(faris_name("Hf179m2"), Some(("Hf-179m".into(), true)));
        assert_eq!(actinv_name("Ag-110m"), "Ag110m1");
        assert_eq!(actinv_name("Nb-94"), "Nb94");
    }

    fn slope(activity: f64) -> Slope {
        Slope {
            activity_bq: activity,
            decay_heat_w_per_m3: activity,
            waste_class_c_burden: None,
            waste: None,
        }
    }

    fn element(name: &str, ppm: f64, primary: f64, fd: f64) -> ElementResult {
        ElementResult {
            element: name.into(),
            wt_ppm: ppm,
            kind: "assumed".into(),
            source: "test".into(),
            per_ppm: slope(primary),
            finite_difference_per_ppm: slope(fd),
            effect_at_specified: slope(primary * ppm),
        }
    }

    // Verifies: ACT-025
    #[test]
    fn the_agreement_check_fails_past_twenty_percent_and_flags_swapped_ranks() {
        let good = vec![element("A", 10.0, 3.0, 3.3), element("B", 10.0, 1.0, 0.9)];
        assert!(agreement(&good, Metric::Activity, 100.0).passed);
        // 25 % apart: fails on the value.
        let off = vec![element("A", 10.0, 3.0, 4.0)];
        let a = agreement(&off, Metric::Activity, 100.0);
        assert!(!a.passed && a.max_relative_difference > AGREEMENT_TOLERANCE);
        // A and B swap by the finite difference while their effects differ by 50 %: not a near tie.
        let swapped = vec![element("A", 10.0, 3.0, 2.0), element("B", 10.0, 2.0, 2.4)];
        let a = agreement(&swapped, Metric::Activity, 100.0);
        assert_eq!(a.discordant_pairs.len(), 1);
        assert!(!a.discordant_pairs[0].effects_within_tolerance && !a.passed);
        // The same swap between two within 20 % is a tie, not a failure of the ranking.
        let tie = vec![element("A", 10.0, 3.0, 2.8), element("B", 10.0, 2.8, 3.0)];
        let a = agreement(&tie, Metric::Activity, 100.0);
        assert_eq!(a.discordant_pairs.len(), 1);
        assert!(a.passed);
        // A response under the resolution floor is ranked, not compared.
        let tiny = vec![element("A", 1.0, 1.0e-20, 5.0e-20)];
        let a = agreement(&tiny, Metric::Activity, 1.0e6);
        assert_eq!(a.elements_below_resolution, vec!["A".to_string()]);
    }

    // Verifies: ACT-025
    #[test]
    fn the_ranking_is_by_absolute_effect_at_the_specified_content() {
        let elements = vec![
            element("Low", 100.0, 0.1, 0.1),
            element("High", 1.0, 20.0, 20.0),
            element("Mid", 10.0, -3.0, -3.0),
        ];
        let (lines, by) = rank(&elements, Metric::Activity, 100.0);
        let order: Vec<_> = lines.iter().map(|l| l.element.as_str()).collect();
        assert_eq!(order, vec!["Mid", "High", "Low"]);
        assert_eq!(by["Mid"], 1);
    }

    #[test]
    fn a_bad_specification_is_refused_before_any_run() {
        let mut r = request();
        assert!(validate(&r).is_ok());
        r.impurities.push(imp("Nb", 5.0));
        assert!(validate(&r).unwrap_err().contains("twice"));
        let mut r = request();
        r.impurities.push(imp("Fe", 5.0));
        assert!(validate(&r).unwrap_err().contains("matrix"));
        let mut r = request();
        r.impurities[0].source = " ".into();
        assert!(validate(&r).unwrap_err().contains("source"));
        let mut r = request();
        r.relative_step = 1.0;
        assert!(validate(&r).is_err());
    }

    /// An authored three-part spectrum on the 709 groups (thermal tail, 1/E, 14 MeV peak) of
    /// about 1e14 n/cm2/s. It stands for no transport result; it only gives the activation
    /// something realistic in shape to act on.
    fn authored_spectrum() -> GroupFlux {
        let edges: Vec<f64> = serde_json::from_str(EDGES).unwrap();
        let kt = 0.0253;
        let flux = edges
            .windows(2)
            .map(|w| {
                let (lo, hi) = (w[0].max(1.0e-5), w[1]);
                let u = math::ln(hi / lo);
                let e = (lo * hi).sqrt();
                let thermal = 3.0e12 * (e / kt) * math::exp(-e / kt);
                let epithermal = if (0.5..1.0e5).contains(&e) {
                    1.0e13
                } else {
                    0.0
                };
                let fast =
                    5.0e13 * math::exp(-math::powi(e - 14.1e6, 2) / (2.0 * math::powi(0.6e6, 2)));
                let slowing = if (1.0e5..1.0e7).contains(&e) {
                    8.0e12
                } else {
                    0.0
                };
                u * (thermal + epithermal + slowing + fast)
            })
            .collect::<Vec<_>>();
        assert_eq!(flux.len(), GROUPS);
        GroupFlux {
            energy_edges_ev: edges,
            flux_per_cm2_s: flux,
        }
    }

    fn imp(element: &str, wt_ppm: f64) -> Impurity {
        Impurity {
            element: element.into(),
            wt_ppm,
            kind: "assumed".into(),
            source: "authored test specification, not a material certificate".into(),
        }
    }

    fn request() -> SensitivityRequest {
        let matrix = [
            ("Fe", 89.0),
            ("Cr", 9.0),
            ("W", 1.1),
            ("Mn", 0.45),
            ("V", 0.2),
            ("Ta", 0.12),
        ];
        SensitivityRequest {
            matrix_wt_percent: matrix.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            impurities: vec![
                imp("Nb", 50.0),
                imp("Mo", 50.0),
                imp("Ag", 10.0),
                imp("Co", 50.0),
                imp("Ni", 400.0),
                imp("Cu", 100.0),
                imp("Al", 100.0),
                imp("Eu", 1.0),
                imp("Tb", 1.0),
                imp("Ho", 1.0),
                imp("Gd", 1.0),
                imp("Ti", 100.0),
            ],
            mass_g: 1.0e6,
            volume_m3: 0.13,
            spectrum: authored_spectrum(),
            steps: vec![IrradiationStep {
                duration_s: faris_model::units::JULIAN_YEAR_S,
                flux_scale: 1.0,
            }],
            cooling_s: vec![
                86_400.0,
                faris_model::units::JULIAN_YEAR_S,
                10.0 * faris_model::units::JULIAN_YEAR_S,
                30.0 * faris_model::units::JULIAN_YEAR_S,
            ],
            relative_step: 0.5,
        }
    }

    /// The installed ACTINV and its data, or None with the reason printed (CI has neither).
    fn live(work: &std::path::Path) -> Option<ActivationConfig> {
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        let actinv = std::env::var_os("FARIS_ACTINV")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".cargo/bin/actinv"));
        let data_dir = std::env::var_os("FARIS_ACTINV_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("Documents/actinv/actinv-data"));
        let data = ActivationData::catalog(&data_dir);
        if !actinv.is_file() || !data.library.is_file() || !data.decay_primary.is_file() {
            eprintln!(
                "skipped: ACTINV is not installed here; set FARIS_ACTINV and FARIS_ACTINV_DATA"
            );
            return None;
        }
        let mut config = ActivationConfig::new(actinv, data, work.join("work"));
        config.timeout = Duration::from_secs(240);
        // A cache folder keeps a repeated run of this test from solving the same zones again.
        config.cache_dir = std::env::var_os("FARIS_TEST_ACTIVATION_CACHE").map(PathBuf::from);
        Some(config)
    }

    // Verifies: ACT-025, MAT-040
    #[test]
    fn live_twelve_impurities_are_ranked_and_agree_with_the_finite_difference() {
        let dir = tempfile::tempdir().unwrap();
        let Some(config) = live(dir.path()) else {
            return;
        };
        let r = impurity_sensitivity(&request(), &config, &Cancellation::default()).unwrap();
        assert_eq!(r.impurities.len(), 12);
        assert_eq!(r.runs, 2 + 3 * 12);
        let mut worst: f64 = 0.0;
        let mut compared = 0;
        for p in &r.points {
            eprintln!(
                "t={} s waste_ne={:?} activity top: {:?}",
                p.cooling_s,
                p.waste_not_evaluated,
                p.activity_ranking.iter().take(4).collect::<Vec<_>>()
            );
            for a in &p.agreement {
                eprintln!(
                    "  {:?}: compared {} max rel diff {:.3e} worst {:?} discordant {}",
                    a.metric,
                    a.elements_compared,
                    a.max_relative_difference,
                    a.worst_element,
                    a.discordant_pairs.len()
                );
                assert!(a.passed, "{:?} at {} s: {:?}", a.metric, p.cooling_s, a);
                worst = worst.max(a.max_relative_difference);
                compared = compared.max(a.elements_compared);
            }
            assert_eq!(p.activity_ranking.len(), 12);
            assert!(
                p.activity_ranking
                    .windows(2)
                    .all(|w| w[0].effect >= w[1].effect)
            );
            assert!(
                p.decay_heat_ranking
                    .windows(2)
                    .all(|w| w[0].effect >= w[1].effect)
            );
        }
        assert!(compared >= 10, "only {compared} elements were resolved");
        assert!(worst <= AGREEMENT_TOLERANCE);
        assert!(r.points.iter().any(|p| p.waste_not_evaluated.is_none()));
        // the effects of the elements add up to the full-specification excess (linearity)
        for p in &r.points {
            let sum: f64 = p
                .elements
                .iter()
                .map(|e| e.effect_at_specified.activity_bq)
                .sum();
            let excess = p.full_specification.activity_bq - p.matrix_only.activity_bq;
            eprintln!("t={} additivity {:.4}", p.cooling_s, sum / excess);
        }
    }
}
