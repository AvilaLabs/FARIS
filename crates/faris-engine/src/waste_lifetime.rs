//! Waste batches over plant life from a replacement schedule (requirement ACT-038).
//!
//! The maintenance engine says when each component is replaced; the plan says what one
//! installation of it weighs and how much room it takes, and which inventory it is classified
//! from. Every replacement removes one installation, and the installation in place at the end
//! of the history is the end-of-life batch of its component.

use faris_model::waste::NotEvaluated;
use faris_model::waste::lifetime::{
    AssignedInventory, BatchOrigin, WasteBatch, class_of, lifetime_waste,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::maintenance::ReplacementSummary;

/// What one installation of a component is, as waste.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ComponentWaste {
    pub component_id: String,
    /// Identical pieces per installation (for example 18 coils).
    #[serde(default = "one")]
    pub pieces: u32,
    pub mass_kg_each: f64,
    pub volume_m3_each: f64,
    /// Inventory every replaced installation is classified from. Absent: NOT_EVALUATED.
    #[serde(default)]
    pub replacement_inventory: Option<AssignedInventory>,
    /// Inventory of the installation still in place at the end of life. Absent: NOT_EVALUATED.
    #[serde(default)]
    pub end_of_life_inventory: Option<AssignedInventory>,
}

fn one() -> u32 {
    1
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LifetimePlan {
    pub components: Vec<ComponentWaste>,
}

/// Batches for the replacements of a history and the end-of-life installations. A replacement of
/// a component the plan does not describe is refused: its mass is unknown and is not guessed.
pub fn batches_from_replacements(
    plan: &LifetimePlan,
    replacements: &[ReplacementSummary],
) -> Result<Vec<WasteBatch>, NotEvaluated> {
    let known: BTreeSet<&str> = plan
        .components
        .iter()
        .map(|c| c.component_id.as_str())
        .collect();
    let missing: Vec<String> = replacements
        .iter()
        .filter(|r| !known.contains(r.component.as_str()))
        .map(|r| r.component.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if !missing.is_empty() {
        return Err(NotEvaluated::new(
            format!(
                "the history replaces components the plan gives no mass and volume for: {}",
                missing.join(", ")
            ),
            missing,
            "Add the components to the lifetime plan with mass_kg_each and volume_m3_each.",
        ));
    }
    let mut out = Vec::new();
    for c in &plan.components {
        let (class, basis) = class_of(c.replacement_inventory.as_ref());
        for r in replacements
            .iter()
            .filter(|r| r.component == c.component_id)
        {
            out.push(WasteBatch {
                batch_id: format!("{}#{}", c.component_id, r.k),
                component_id: c.component_id.clone(),
                origin: BatchOrigin::Replacement { k: r.k },
                count: c.pieces,
                mass_kg_each: c.mass_kg_each,
                volume_m3_each: c.volume_m3_each,
                class: class.clone(),
                class_basis: basis.clone(),
            });
        }
        let (class, basis) = class_of(c.end_of_life_inventory.as_ref());
        out.push(WasteBatch {
            batch_id: format!("{}#end", c.component_id),
            component_id: c.component_id.clone(),
            origin: BatchOrigin::EndOfLife,
            count: c.pieces,
            mass_kg_each: c.mass_kg_each,
            volume_m3_each: c.volume_m3_each,
            class,
            class_basis: basis,
        });
    }
    Ok(out)
}

/// Lifetime waste of a plan over the replacements of a history.
pub fn lifetime_from_replacements(
    plan: &LifetimePlan,
    replacements: &[ReplacementSummary],
) -> Result<faris_model::waste::lifetime::LifetimeWaste, NotEvaluated> {
    lifetime_waste(batches_from_replacements(plan, replacements)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use crate::history::run_operating_history_cancellable;
    use crate::jobs::Cancellation;
    use crate::maintenance::replacement_events;
    use faris_model::maintenance::MaintenanceClass;
    use faris_model::units::JULIAN_YEAR_S;
    use faris_model::waste::lifetime::NOT_EVALUATED_CLASS;
    use faris_model::waste::{NuclideActivity, WasteForm, WasteInventory};
    use std::collections::BTreeMap;

    /// The synthetic fixture history stretched to 30 Julian years, with the magnet limit set so
    /// the magnets are replaced about every six years and the blanket never reaches its limit.
    fn thirty_year_replacements() -> (Vec<ReplacementSummary>, f64) {
        let f = 30.0 * JULIAN_YEAR_S / 100.0;
        let mut a = fixtures::assumptions();
        a.horizon_s *= f;
        a.maximum_step_s *= f;
        a.snapshot_interval_s *= f;
        for p in &mut a.operation {
            p.end_s *= f;
        }
        for l in &mut a.service_limits {
            if l.component_id == "magnets" {
                // 1e10 n/m2/s for six years: five replacements in thirty
                l.limit = 1.0e10 * 6.0 * JULIAN_YEAR_S;
                l.replacement_duration_s = Some(30.0 * 86_400.0);
            } else {
                l.limit = 1.0e30;
            }
        }
        let h = run_operating_history_cancellable(
            &a,
            &fixtures::rates_without_covariance(0.0, 'a'),
            &Cancellation::default(),
        )
        .unwrap();
        let classes: BTreeMap<String, MaintenanceClass> = BTreeMap::from([(
            "magnet".to_string(),
            MaintenanceClass {
                component_id: "magnets".into(),
                governing: vec!["magnets".into()],
                work_s: 1.0,
                threshold: faris_model::maintenance::Threshold::QStar {
                    q_star_w_per_m3: 1.0,
                },
            },
        )]);
        let r = replacement_events(&h, &classes)
            .into_iter()
            .map(|e| ReplacementSummary {
                class: e.class,
                component: e.component,
                k: e.k,
                start_s: e.start_s,
                end_s: e.end_s,
            })
            .collect();
        (r, h.assumptions.horizon_s)
    }

    fn inventory(co60_bq: f64) -> AssignedInventory {
        let n = |name: &str, bq: f64, half_life_s: f64| NuclideActivity {
            nuclide: name.into(),
            activity_bq: bq,
            half_life_s,
            alpha_emitter: None,
            progeny_of: None,
        };
        AssignedInventory {
            inventory: WasteInventory {
                component_id: "magnets".into(),
                waste_form: WasteForm::Other,
                volume_m3: Some(1.0),
                mass_g: Some(1.0e6),
                inventory_time_years: 0.0,
                transuranics_evaluated: true,
                inventory_kind: "authored_test_inventory".into(),
                nuclides: {
                    let mut v: Vec<NuclideActivity> =
                        faris_model::waste::classify::required_nuclides(WasteForm::Other)
                            .iter()
                            .filter(|name| name.as_str() != "Co-60")
                            .map(|name| n(name, 0.0, 1.0e9))
                            .collect();
                    v.push(n("Co-60", co60_bq, 5.27 * JULIAN_YEAR_S));
                    v
                },
            },
            disposal_time_years: 5.0,
        }
    }

    fn plan(with_inventory: bool) -> LifetimePlan {
        LifetimePlan {
            components: vec![
                ComponentWaste {
                    component_id: "magnets".into(),
                    pieces: 18,
                    mass_kg_each: 5000.5,
                    volume_m3_each: 0.7,
                    replacement_inventory: with_inventory.then(|| inventory(1.0e6)),
                    end_of_life_inventory: None,
                },
                ComponentWaste {
                    component_id: "blanket".into(),
                    pieces: 1,
                    mass_kg_each: 123_456.789,
                    volume_m3_each: 21.3,
                    replacement_inventory: None,
                    end_of_life_inventory: None,
                },
            ],
        }
    }

    // Verifies: ACT-038
    #[test]
    fn thirty_year_history_sums_and_counts_every_batch_once() {
        let (replacements, end_s) = thirty_year_replacements();
        assert!((end_s - 30.0 * JULIAN_YEAR_S).abs() < 1.0);
        assert!(replacements.len() >= 2, "{}", replacements.len());
        let r = lifetime_from_replacements(&plan(true), &replacements).unwrap();
        // one batch per replacement plus one end-of-life batch per component
        assert_eq!(r.plant.batches as usize, replacements.len() + 2);
        let magnet_pieces = 18 * (replacements.len() as u64 + 1);
        assert_eq!(r.by_component["magnets"].pieces, magnet_pieces);
        assert_eq!(r.by_component["blanket"].pieces, 1);
        let mass: f64 = r.by_class.values().map(|t| t.mass_kg).sum();
        let expect = 5000.5 * magnet_pieces as f64 + 123_456.789;
        assert!((mass - expect).abs() / expect < 1e-9);
        assert!(r.sum_check.passed && r.sum_check.max_relative_difference <= 1e-9);
        // replaced magnets carry a real class; the end-of-life batches have no inventory
        assert_ne!(
            r.by_component_class["magnets"]
                .keys()
                .filter(|k| k.as_str() != NOT_EVALUATED_CLASS)
                .count(),
            0
        );
        assert_eq!(r.by_class[NOT_EVALUATED_CLASS].batches, 2);
        let ids: BTreeSet<_> = r.batches.iter().map(|b| b.batch_id.as_str()).collect();
        assert_eq!(ids.len(), r.batches.len());
    }

    // Verifies: ACT-038
    #[test]
    fn without_inventories_every_batch_is_not_evaluated() {
        let (replacements, _) = thirty_year_replacements();
        let r = lifetime_from_replacements(&plan(false), &replacements).unwrap();
        assert_eq!(r.by_class.len(), 1);
        assert_eq!(r.by_class[NOT_EVALUATED_CLASS].batches, r.plant.batches);
    }

    #[test]
    fn a_replaced_component_with_no_mass_is_refused() {
        let (replacements, _) = thirty_year_replacements();
        let mut p = plan(false);
        p.components.retain(|c| c.component_id != "magnets");
        let e = lifetime_from_replacements(&p, &replacements).unwrap_err();
        assert!(e.missing.contains(&"magnets".to_string()));
    }
}
