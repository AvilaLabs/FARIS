//! Waste mass and volume by class over plant life, replacements included (requirement ACT-038).
//!
//! The input is a list of batches: every part that leaves the plant as waste, each replacement
//! and each component still installed at the end of life. A batch carries its count, the mass
//! and volume of one piece, and the class it was given by a real classification of an
//! inventory. A batch with no classified inventory is counted under `NOT_EVALUATED`; a class is
//! never guessed. Each batch id is counted once: a repeated id is refused.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{NotEvaluated, WasteInventory, classify::classify_nrc};

/// Label of a batch whose class was not calculated.
pub const NOT_EVALUATED_CLASS: &str = "NOT_EVALUATED";
/// The sum test: component and class totals agree with the plant total to this relative amount.
pub const SUM_TOLERANCE: f64 = 1e-9;

/// Why a batch is waste.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BatchOrigin {
    /// The `k`-th replacement of the component (k from 1): the installation taken out.
    Replacement { k: u32 },
    /// The installation still in place when the plant ends.
    EndOfLife,
}

/// One waste batch: `count` identical pieces of one component.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WasteBatch {
    pub batch_id: String,
    pub component_id: String,
    pub origin: BatchOrigin,
    pub count: u32,
    pub mass_kg_each: f64,
    pub volume_m3_each: f64,
    /// `CLASS_A` and so on, or `NOT_EVALUATED`.
    pub class: String,
    /// How the class was obtained, for example `10 CFR 61.55 from inventory solver_result at 10 y`.
    pub class_basis: String,
}

impl WasteBatch {
    pub fn mass_kg(&self) -> f64 {
        f64::from(self.count) * self.mass_kg_each
    }
    pub fn volume_m3(&self) -> f64 {
        f64::from(self.count) * self.volume_m3_each
    }
}

/// An inventory and the time after its reference at which the waste is disposed of.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AssignedInventory {
    pub inventory: WasteInventory,
    pub disposal_time_years: f64,
}

/// Class and its basis for a batch: the 10 CFR 61.55 verdict of the inventory, or
/// `NOT_EVALUATED` with the reason when there is no inventory or the classifier refuses.
pub fn class_of(assigned: Option<&AssignedInventory>) -> (String, String) {
    match assigned {
        None => (
            NOT_EVALUATED_CLASS.to_string(),
            "no inventory was given for this batch".to_string(),
        ),
        Some(a) => {
            let r = classify_nrc(&a.inventory, a.disposal_time_years);
            let basis = match &r.not_evaluated {
                Some(ne) => format!("classifier refused: {}", ne.reason),
                None => format!(
                    "{} from inventory {} ({}) at {} y",
                    r.scheme.name,
                    a.inventory.component_id,
                    a.inventory.inventory_kind,
                    a.disposal_time_years
                ),
            };
            (r.verdict.text().to_string(), basis)
        }
    }
}

/// Mass and volume with the number of pieces.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Totals {
    pub batches: u32,
    pub pieces: u64,
    pub mass_kg: f64,
    pub volume_m3: f64,
}

impl Totals {
    fn add(&mut self, b: &WasteBatch) {
        self.batches += 1;
        self.pieces += u64::from(b.count);
        self.mass_kg += b.mass_kg();
        self.volume_m3 += b.volume_m3();
    }
}

/// The sum test: the plant total against the sum over components and over classes.
#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
pub struct SumCheck {
    pub plant_mass_kg: f64,
    pub by_component_mass_kg: f64,
    pub by_class_mass_kg: f64,
    pub plant_volume_m3: f64,
    pub by_component_volume_m3: f64,
    pub by_class_volume_m3: f64,
    pub max_relative_difference: f64,
    pub passed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct LifetimeWaste {
    pub batches: Vec<WasteBatch>,
    pub plant: Totals,
    pub by_class: BTreeMap<String, Totals>,
    pub by_component: BTreeMap<String, Totals>,
    /// Component, then class.
    pub by_component_class: BTreeMap<String, BTreeMap<String, Totals>>,
    pub sum_check: SumCheck,
    /// Batches whose class is `NOT_EVALUATED`, so a reader sees what is unclassified.
    pub not_evaluated_batches: Vec<String>,
    pub research_screening: String,
}

fn rel(a: f64, b: f64) -> f64 {
    let scale = a.abs().max(b.abs());
    if scale == 0.0 {
        0.0
    } else {
        (a - b).abs() / scale
    }
}

fn sum_of(m: &BTreeMap<String, Totals>, f: impl Fn(&Totals) -> f64) -> f64 {
    m.values().map(f).sum()
}

/// Total waste by class and component. The plant total is summed straight from the batches; the
/// component and class totals are built separately, so the sum test compares independent sums.
pub fn lifetime_waste(batches: Vec<WasteBatch>) -> Result<LifetimeWaste, NotEvaluated> {
    let next = "Correct the batch list and run again.";
    let mut seen = BTreeSet::new();
    for b in &batches {
        if !seen.insert(b.batch_id.clone()) {
            return Err(NotEvaluated::new(
                format!(
                    "batch `{}` appears twice; each batch is counted once",
                    b.batch_id
                ),
                vec![b.batch_id.clone()],
                next,
            ));
        }
        let ok = |v: f64| v.is_finite() && v >= 0.0;
        if b.count == 0 || !ok(b.mass_kg_each) || !ok(b.volume_m3_each) {
            return Err(NotEvaluated::new(
                format!(
                    "batch `{}` needs a count of at least 1 and a mass and volume of 0 or more",
                    b.batch_id
                ),
                vec![b.batch_id.clone()],
                next,
            ));
        }
        if b.class.is_empty() {
            return Err(NotEvaluated::new(
                format!(
                    "batch `{}` has no class label; use NOT_EVALUATED",
                    b.batch_id
                ),
                vec![b.batch_id.clone()],
                next,
            ));
        }
    }
    let mut plant = Totals::default();
    let mut by_class: BTreeMap<String, Totals> = BTreeMap::new();
    let mut by_component: BTreeMap<String, Totals> = BTreeMap::new();
    let mut by_cc: BTreeMap<String, BTreeMap<String, Totals>> = BTreeMap::new();
    for b in &batches {
        plant.add(b);
        by_class.entry(b.class.clone()).or_default().add(b);
        by_component
            .entry(b.component_id.clone())
            .or_default()
            .add(b);
        by_cc
            .entry(b.component_id.clone())
            .or_default()
            .entry(b.class.clone())
            .or_default()
            .add(b);
    }
    let cm = sum_of(&by_component, |t| t.mass_kg);
    let km = sum_of(&by_class, |t| t.mass_kg);
    let cv = sum_of(&by_component, |t| t.volume_m3);
    let kv = sum_of(&by_class, |t| t.volume_m3);
    let worst = [
        rel(plant.mass_kg, cm),
        rel(plant.mass_kg, km),
        rel(plant.volume_m3, cv),
        rel(plant.volume_m3, kv),
    ]
    .into_iter()
    .fold(0.0, f64::max);
    let not_evaluated_batches = batches
        .iter()
        .filter(|b| b.class == NOT_EVALUATED_CLASS)
        .map(|b| b.batch_id.clone())
        .collect();
    Ok(LifetimeWaste {
        sum_check: SumCheck {
            plant_mass_kg: plant.mass_kg,
            by_component_mass_kg: cm,
            by_class_mass_kg: km,
            plant_volume_m3: plant.volume_m3,
            by_component_volume_m3: cv,
            by_class_volume_m3: kv,
            max_relative_difference: worst,
            passed: worst <= SUM_TOLERANCE,
        },
        batches,
        plant,
        by_class,
        by_component,
        by_component_class: by_cc,
        not_evaluated_batches,
        research_screening: crate::RESEARCH_SCREENING_STATEMENT.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(id: &str, comp: &str, k: u32, count: u32, class: &str) -> WasteBatch {
        WasteBatch {
            batch_id: id.into(),
            component_id: comp.into(),
            origin: BatchOrigin::Replacement { k },
            count,
            mass_kg_each: 1234.5678,
            volume_m3_each: 0.31,
            class: class.into(),
            class_basis: "test".into(),
        }
    }

    // Verifies: ACT-038
    #[test]
    fn totals_sum_and_each_batch_counts_once() {
        let b = vec![
            batch("a#1", "a", 1, 3, "CLASS_C"),
            batch("a#2", "a", 2, 3, "CLASS_C"),
            batch("b#1", "b", 1, 7, NOT_EVALUATED_CLASS),
        ];
        let r = lifetime_waste(b).unwrap();
        assert_eq!(r.plant.pieces, 13);
        assert_eq!(r.plant.batches, 3);
        assert_eq!(r.by_class["CLASS_C"].pieces, 6);
        assert_eq!(r.by_class[NOT_EVALUATED_CLASS].batches, 1);
        assert!(r.sum_check.passed);
        assert_eq!(r.not_evaluated_batches, vec!["b#1".to_string()]);
        let pieces: u64 = r.by_component.values().map(|t| t.pieces).sum();
        assert_eq!(pieces, 13);
    }

    // Verifies: ACT-038
    #[test]
    fn a_repeated_batch_is_refused_not_double_counted() {
        let b = vec![
            batch("a#1", "a", 1, 1, "CLASS_A"),
            batch("a#1", "a", 1, 1, "CLASS_A"),
        ];
        assert!(lifetime_waste(b).unwrap_err().reason.contains("twice"));
    }

    #[test]
    fn missing_inventory_is_not_evaluated_not_a_class() {
        let (c, why) = class_of(None);
        assert_eq!(c, NOT_EVALUATED_CLASS);
        assert!(why.contains("no inventory"));
    }
}
