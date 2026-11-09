// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! Assumptions for computed maintenance durations: replacement groups, cooldown criteria,
//! work time and iteration limits. Values and dose-rate limits are authored inputs.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAINTENANCE_ASSUMPTIONS_VERSION: &str = "faris-maintenance-assumptions/v0.1";
pub const MAINTENANCE_DOSE_ASSUMPTIONS_VERSION: &str = "faris-maintenance-assumptions/v0.2";
pub const PHOTON_EFFECTIVE_DOSE_RATE_QUANTITY: &str = "icrp116_ap_effective_dose_rate";

use crate::units::DAY_S;

/// The quantity whose fall below a threshold ends the cooldown.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GoverningQuantity {
    Heat,
    /// Recognised so a file asking for it is refused with the reason, not as a typo.
    Dose,
}

/// Transport method selected to provide the spatial photon dose-rate curve.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DoseMethod {
    /// Fast method; its results must retain the matched R2S comparison status.
    #[default]
    D1s,
    /// Rigorous two-step reference method.
    R2s,
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
    /// Remote-handling dose-rate limit at a named, hash-identified task location.
    /// This is a photon effective dose-rate criterion in Sv/h, not a contact-dose proxy.
    DoseRateLimit {
        value_sv_h: f64,
        quantity: String,
        #[serde(default)]
        method: DoseMethod,
        source: String,
        location_id: String,
        location_sha256: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceClass {
    /// The replaced component.
    pub component_id: String,
    /// Components whose decay heat governs cooldown for heat mode. Empty in dose mode: the
    /// spatial dose curve is the total field at `DoseRateLimit.location_id`.
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
            min_s: crate::units::HOUR_S,
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
        if ![
            MAINTENANCE_ASSUMPTIONS_VERSION,
            MAINTENANCE_DOSE_ASSUMPTIONS_VERSION,
        ]
        .contains(&self.schema_version.as_str())
        {
            return Err(format!(
                "schema_version must be {MAINTENANCE_ASSUMPTIONS_VERSION} or {MAINTENANCE_DOSE_ASSUMPTIONS_VERSION}"
            ));
        }
        if self.governing_quantity == GoverningQuantity::Dose
            && self.schema_version != MAINTENANCE_DOSE_ASSUMPTIONS_VERSION
        {
            return Err(format!(
                "dose-governed durations require {MAINTENANCE_DOSE_ASSUMPTIONS_VERSION} and hash-bound spatial dose curves at the named task location"
            ));
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
            if class.governing.is_empty() && self.governing_quantity == GoverningQuantity::Heat {
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
            match (&self.governing_quantity, &class.threshold) {
                (GoverningQuantity::Heat, Threshold::QStar { q_star_w_per_m3 }) => {
                    if !q_star_w_per_m3.is_finite() || *q_star_w_per_m3 <= 0.0 {
                        return Err(format!(
                            "class {name} q_star_w_per_m3 must be finite and positive"
                        ));
                    }
                }
                (
                    GoverningQuantity::Heat,
                    Threshold::Calibrate {
                        design,
                        target_cooldown_s,
                    },
                ) => {
                    if design.trim().is_empty() {
                        return Err(format!("class {name} calibration needs a design name"));
                    }
                    if !target_cooldown_s.is_finite() || *target_cooldown_s <= 0.0 {
                        return Err(format!(
                            "class {name} target_cooldown_s must be finite and positive"
                        ));
                    }
                }
                (
                    GoverningQuantity::Dose,
                    Threshold::DoseRateLimit {
                        value_sv_h,
                        quantity,
                        method: _,
                        source,
                        location_id,
                        location_sha256,
                    },
                ) => {
                    if self.schema_version != MAINTENANCE_DOSE_ASSUMPTIONS_VERSION {
                        return Err(format!(
                            "class {name} dose limit requires {MAINTENANCE_DOSE_ASSUMPTIONS_VERSION}"
                        ));
                    }
                    if !value_sv_h.is_finite() || *value_sv_h <= 0.0 {
                        return Err(format!(
                            "class {name} dose limit must be finite and positive Sv/h"
                        ));
                    }
                    if quantity != PHOTON_EFFECTIVE_DOSE_RATE_QUANTITY {
                        return Err(format!(
                            "class {name} dose quantity must be {PHOTON_EFFECTIVE_DOSE_RATE_QUANTITY} (ICRP-116 AP photon effective dose rate)"
                        ));
                    }
                    if source.trim().is_empty() || source.len() > 2048 {
                        return Err(format!("class {name} dose limit source is required"));
                    }
                    if location_id.trim().is_empty() || location_id.len() > 256 {
                        return Err(format!("class {name} dose task location ID is required"));
                    }
                    if location_sha256.len() != 64
                        || !location_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
                    {
                        return Err(format!(
                            "class {name} dose location needs a 64-character SHA-256"
                        ));
                    }
                }
                (GoverningQuantity::Heat, Threshold::DoseRateLimit { .. }) => {
                    return Err(format!(
                        "class {name} dose-rate limit is only valid when governing_quantity is dose"
                    ));
                }
                (GoverningQuantity::Dose, _) => {
                    return Err(format!(
                        "class {name} requires a dose_rate_limit threshold in Sv/h"
                    ));
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
    fn dose_in_v01_is_refused_until_spatial_curve_schema_is_selected() {
        let mut a = base();
        a.governing_quantity = GoverningQuantity::Dose;
        let err = a.validate().unwrap_err();
        assert!(err.contains(MAINTENANCE_DOSE_ASSUMPTIONS_VERSION));
        assert!(err.contains("spatial dose curves"));
        assert!(!err.contains("contact-dose proxies"));
    }

    #[test]
    fn dose_v02_requires_an_explicit_sv_rate_source_and_hash_identified_location() {
        let mut a = base();
        a.schema_version = MAINTENANCE_DOSE_ASSUMPTIONS_VERSION.into();
        a.governing_quantity = GoverningQuantity::Dose;
        for (name, class) in &mut a.classes {
            class.governing.clear();
            class.threshold = Threshold::DoseRateLimit {
                value_sv_h: 1.0e-4,
                quantity: PHOTON_EFFECTIVE_DOSE_RATE_QUANTITY.into(),
                method: DoseMethod::D1s,
                source: format!("remote handling limit for {name}"),
                location_id: format!("task-{name}"),
                location_sha256: "a".repeat(64),
            };
        }
        a.validate().unwrap();
        let mut invalid = a.clone();
        let Threshold::DoseRateLimit {
            location_sha256, ..
        } = &mut invalid.classes.get_mut("magnet").unwrap().threshold
        else {
            unreachable!()
        };
        location_sha256.pop();
        assert!(
            invalid
                .validate()
                .unwrap_err()
                .contains("64-character SHA-256")
        );

        let mut invalid = a;
        let Threshold::DoseRateLimit { quantity, .. } =
            &mut invalid.classes.get_mut("magnet").unwrap().threshold
        else {
            unreachable!()
        };
        *quantity = "contact_gamma_air_dose_proxy_Gy_h".into();
        assert!(invalid.validate().unwrap_err().contains("ICRP-116 AP"));
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
