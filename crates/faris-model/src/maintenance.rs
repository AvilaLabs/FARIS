//! Assumptions for computed maintenance durations: which components are replaced, which
//! components' decay heat governs the cooldown before the replacement, the threshold that
//! ends the cooldown, and the iteration limits. Values are authored inputs.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAINTENANCE_ASSUMPTIONS_VERSION: &str = "faris-maintenance-assumptions/v0.1";

const DAY_S: f64 = 86_400.0;

/// The quantity whose fall below a threshold ends the cooldown.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GoverningQuantity {
    Heat,
    /// Recognised so a file asking for it is refused with the reason, not as a typo.
    Dose,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Threshold {
    /// Cooldown ends when the governing set's decay heat per volume falls to this (W/m³).
    QStar { q_star_w_per_m3: f64 },
    /// q* is the value that makes the class's first replacement in the named design, at the
    /// fixed durations, cool for exactly the target.
    Calibrate {
        design: String,
        target_cooldown_s: f64,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceClass {
    /// The replaced component.
    pub component_id: String,
    /// Components whose decay heat must have fallen before the replacement can start work.
    pub governing: Vec<String>,
    /// Hands-on time after the cooldown.
    pub work_s: f64,
    pub threshold: Threshold,
}

/// Log-spaced cooling times, endpoints exact.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct CoolingGrid {
    pub min_s: f64,
    pub max_s: f64,
    pub points: usize,
}

impl Default for CoolingGrid {
    fn default() -> Self {
        Self {
            min_s: 3600.0,
            max_s: 365.0 * DAY_S,
            points: 40,
        }
    }
}

fn default_max_iterations() -> u32 {
    10
}

fn default_convergence_s() -> f64 {
    DAY_S
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceAssumptions {
    pub schema_version: String,
    pub governing_quantity: GoverningQuantity,
    pub classes: BTreeMap<String, MaintenanceClass>,
    #[serde(default)]
    pub cooling: CoolingGrid,
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    #[serde(default = "default_convergence_s")]
    pub convergence_s: f64,
}

impl MaintenanceAssumptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != MAINTENANCE_ASSUMPTIONS_VERSION {
            return Err(format!(
                "schema_version must be {MAINTENANCE_ASSUMPTIONS_VERSION}"
            ));
        }
        if self.governing_quantity == GoverningQuantity::Dose {
            return Err("dose-governed durations are not supported in 0.2: with a fixed threshold the never-replaced first wall keeps contact dose above it past a year at later replacements (docs/notes/MAINTENANCE_COUPLING_VALIDATION_RESULT.md); use heat".into());
        }
        if self.classes.is_empty() {
            return Err("at least one maintenance class is required".into());
        }
        let mut replaced = BTreeSet::new();
        for (name, class) in &self.classes {
            if name.trim().is_empty() || class.component_id.trim().is_empty() {
                return Err("class names and component IDs must be nonempty".into());
            }
            if !replaced.insert(class.component_id.as_str()) {
                return Err(format!(
                    "two classes replace component {}; a component has one class",
                    class.component_id
                ));
            }
            if class.governing.is_empty() {
                return Err(format!(
                    "class {name} needs at least one governing component"
                ));
            }
            let mut seen = BTreeSet::new();
            for id in &class.governing {
                if id.trim().is_empty() || !seen.insert(id.as_str()) {
                    return Err(format!(
                        "class {name} governing components must be nonempty and unique"
                    ));
                }
            }
            if !class.work_s.is_finite() || class.work_s < 0.0 {
                return Err(format!(
                    "class {name} work_s must be finite and nonnegative"
                ));
            }
            match &class.threshold {
                Threshold::QStar { q_star_w_per_m3 } => {
                    if !q_star_w_per_m3.is_finite() || *q_star_w_per_m3 <= 0.0 {
                        return Err(format!(
                            "class {name} q_star_w_per_m3 must be finite and positive"
                        ));
                    }
                }
                Threshold::Calibrate {
                    design,
                    target_cooldown_s,
                } => {
                    if design.trim().is_empty() {
                        return Err(format!("class {name} calibration needs a design name"));
                    }
                    if !target_cooldown_s.is_finite() || *target_cooldown_s <= 0.0 {
                        return Err(format!(
                            "class {name} target_cooldown_s must be finite and positive"
                        ));
                    }
                }
            }
        }
        let g = &self.cooling;
        if !(g.min_s.is_finite() && g.max_s.is_finite() && 0.0 < g.min_s && g.min_s < g.max_s) {
            return Err("cooling grid needs finite 0 < min_s < max_s".into());
        }
        if !(2..=1000).contains(&g.points) {
            return Err("cooling grid points must be 2 to 1000".into());
        }
        if !(1..=100).contains(&self.max_iterations) {
            return Err("max_iterations must be 1 to 100".into());
        }
        if !self.convergence_s.is_finite() || self.convergence_s <= 0.0 {
            return Err("convergence_s must be finite and positive".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> MaintenanceAssumptions {
        serde_json::from_str(
            r#"{
              "schema_version": "faris-maintenance-assumptions/v0.1",
              "governing_quantity": "heat",
              "classes": {
                "magnet": {"component_id": "magnets", "governing": ["first-wall", "blanket"],
                           "work_s": 1000.0,
                           "threshold": {"kind": "calibrate", "design": "ref", "target_cooldown_s": 5000.0}},
                "blanket": {"component_id": "blanket", "governing": ["blanket"], "work_s": 0.0,
                            "threshold": {"kind": "q_star", "q_star_w_per_m3": 12.5}}
              }
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn parses_with_defaults_and_validates() {
        let a = base();
        a.validate().unwrap();
        assert_eq!(a.cooling, CoolingGrid::default());
        assert_eq!(a.max_iterations, 10);
        assert_eq!(a.convergence_s, 86_400.0);
        let back: MaintenanceAssumptions =
            serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(back, a);
    }

    #[test]
    fn dose_deserializes_but_is_refused_with_the_reason() {
        let mut a = base();
        a.governing_quantity = GoverningQuantity::Dose;
        let err = a.validate().unwrap_err();
        assert!(err.contains("dose-governed durations are not supported in 0.2"));
        assert!(err.contains("use heat"));
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let mut v = serde_json::to_value(base()).unwrap();
        v["extra"] = 1.into();
        assert!(serde_json::from_value::<MaintenanceAssumptions>(v).is_err());
        let mut v = serde_json::to_value(base()).unwrap();
        v["classes"]["magnet"]["threshold"]["q_star_w_per_m3"] = 1.0.into();
        assert!(serde_json::from_value::<MaintenanceAssumptions>(v).is_err());
    }

    #[test]
    fn bad_inputs_are_refused() {
        type Edit = Box<dyn Fn(&mut MaintenanceAssumptions)>;
        let cases: Vec<(&str, Edit)> = vec![
            ("schema", Box::new(|a| a.schema_version = "x".into())),
            ("empty", Box::new(|a| a.classes.clear())),
            (
                "duplicate component",
                Box::new(|a| a.classes.get_mut("blanket").unwrap().component_id = "magnets".into()),
            ),
            (
                "no governing",
                Box::new(|a| a.classes.get_mut("magnet").unwrap().governing.clear()),
            ),
            (
                "duplicate governing",
                Box::new(|a| {
                    a.classes.get_mut("magnet").unwrap().governing = vec!["a".into(), "a".into()]
                }),
            ),
            (
                "negative work",
                Box::new(|a| a.classes.get_mut("magnet").unwrap().work_s = -1.0),
            ),
            (
                "nan work",
                Box::new(|a| a.classes.get_mut("magnet").unwrap().work_s = f64::NAN),
            ),
            (
                "zero q*",
                Box::new(|a| {
                    a.classes.get_mut("blanket").unwrap().threshold = Threshold::QStar {
                        q_star_w_per_m3: 0.0,
                    }
                }),
            ),
            (
                "zero target",
                Box::new(|a| {
                    a.classes.get_mut("magnet").unwrap().threshold = Threshold::Calibrate {
                        design: "ref".into(),
                        target_cooldown_s: 0.0,
                    }
                }),
            ),
            (
                "grid order",
                Box::new(|a| a.cooling.min_s = a.cooling.max_s),
            ),
            ("grid zero", Box::new(|a| a.cooling.min_s = 0.0)),
            ("grid points 1", Box::new(|a| a.cooling.points = 1)),
            ("grid points big", Box::new(|a| a.cooling.points = 1001)),
            ("iterations 0", Box::new(|a| a.max_iterations = 0)),
            ("iterations 101", Box::new(|a| a.max_iterations = 101)),
            ("convergence", Box::new(|a| a.convergence_s = 0.0)),
        ];
        for (label, edit) in cases {
            let mut a = base();
            edit(&mut a);
            assert!(a.validate().is_err(), "{label} should be refused");
        }
    }
}
