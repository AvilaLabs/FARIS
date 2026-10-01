//! Explicit operating-history assumptions. Values are scenario inputs, not
//! hidden defaults or qualified design limits.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const OPERATING_HISTORY_VERSION: &str = "faris-operating-history/v0.1";
/// Maximum nominal integration intervals for a bounded, cancellable history.
pub const MAX_HISTORY_NOMINAL_STEPS: f64 = 2_000_000.0;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OperatingHistoryAssumptions {
    pub schema_version: String,
    pub horizon_s: f64,
    pub maximum_step_s: f64,
    /// Export cadence for routine snapshots; discrete physical events are always retained.
    pub snapshot_interval_s: f64,
    pub initial_available_tritium_kg: f64,
    pub initial_in_process_tritium_kg: f64,
    pub initial_in_process_release_s: f64,
    pub startup_reserve_kg: f64,
    /// Restart hysteresis threshold; operation may restart only at/above this stock.
    pub restart_inventory_kg: f64,
    pub recovery_fraction: f64,
    pub processing_delay_s: f64,
    pub operation: Vec<PowerPeriod>,
    pub planned_outages: Vec<TimeInterval>,
    pub imports: Vec<TritiumImport>,
    pub service_limits: Vec<ServiceLimit>,
    pub energy: EnergyAssumptions,
    pub provenance: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PowerPeriod {
    pub start_s: f64,
    pub end_s: f64,
    pub power_fraction: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TimeInterval {
    pub start_s: f64,
    pub end_s: f64,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TritiumImport {
    pub at_s: f64,
    pub mass_kg: f64,
    pub provenance: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ComponentClass {
    Permanent,
    Replaceable,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ServiceLimit {
    pub component_id: String,
    pub class: ComponentClass,
    pub response_id: String,
    pub metric: String,
    pub unit: String,
    pub limit: f64,
    pub replacement_duration_s: Option<f64>,
    pub provenance: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EnergyAssumptions {
    pub alpha_deposition_fraction: Option<f64>,
    pub thermal_to_electric_efficiency: Option<f64>,
    /// Fraction of the explicitly tallied whole-model transport heating used in the conditional ledger.
    #[serde(alias = "neutron_heat_recovery_fraction")]
    pub transport_heat_recovery_fraction: Option<f64>,
    pub auxiliary_power_mw_while_operating: Option<f64>,
    pub auxiliary_power_mw_while_off: Option<f64>,
    pub provenance: String,
}

impl OperatingHistoryAssumptions {
    pub fn validate(&self) -> Result<(), String> {
        let positive = |value: f64, label: &str| {
            if value.is_finite() && value > 0.0 {
                Ok(())
            } else {
                Err(format!("{label} must be finite and positive"))
            }
        };
        let nonnegative = |value: f64, label: &str| {
            if value.is_finite() && value >= 0.0 {
                Ok(())
            } else {
                Err(format!("{label} must be finite and nonnegative"))
            }
        };
        if self.schema_version != OPERATING_HISTORY_VERSION {
            return Err(format!(
                "schema_version must be {OPERATING_HISTORY_VERSION}"
            ));
        }
        positive(self.horizon_s, "horizon_s")?;
        positive(self.maximum_step_s, "maximum_step_s")?;
        positive(self.snapshot_interval_s, "snapshot_interval_s")?;
        if self.operation.len() > 10_000
            || self.planned_outages.len() > 10_000
            || self.imports.len() > 10_000
            || self.service_limits.len() > 10_000
        {
            return Err("history event and component lists are bounded at 10,000 entries".into());
        }
        if (self.horizon_s / self.maximum_step_s).ceil() > MAX_HISTORY_NOMINAL_STEPS {
            return Err("history exceeds the 2,000,000-step nominal bound".into());
        }
        if (self.horizon_s / self.snapshot_interval_s).ceil() > 50_000.0 {
            return Err("history exceeds the 50,000 routine-snapshot bound".into());
        }
        for (v, name) in [
            (
                self.initial_available_tritium_kg,
                "initial_available_tritium_kg",
            ),
            (
                self.initial_in_process_tritium_kg,
                "initial_in_process_tritium_kg",
            ),
            (
                self.initial_in_process_release_s,
                "initial_in_process_release_s",
            ),
            (self.startup_reserve_kg, "startup_reserve_kg"),
            (self.restart_inventory_kg, "restart_inventory_kg"),
            (self.processing_delay_s, "processing_delay_s"),
        ] {
            nonnegative(v, name)?;
        }
        if !(0.0..=1.0).contains(&self.recovery_fraction) {
            return Err("recovery_fraction must be in [0,1]".into());
        }
        if self.restart_inventory_kg < self.startup_reserve_kg {
            return Err("restart_inventory_kg must be at least startup_reserve_kg".into());
        }
        if self.processing_delay_s > 0.0 && self.maximum_step_s > self.processing_delay_s / 24.0 {
            return Err(
                "maximum_step_s must resolve processing_delay_s with at least 24 steps per delay"
                    .into(),
            );
        }
        if self.initial_in_process_tritium_kg > 0.0 && self.initial_in_process_release_s <= 0.0 {
            return Err(
                "nonzero initial in-process inventory needs a release time after startup".into(),
            );
        }
        if self.initial_in_process_release_s > self.horizon_s {
            return Err("initial in-process release is beyond the history horizon".into());
        }
        if self.startup_reserve_kg > self.initial_available_tritium_kg
            && self.initial_in_process_tritium_kg == 0.0
            && self.imports.is_empty()
        {
            return Err(
                "startup reserve exceeds opening inventory and no import can supply it".into(),
            );
        }
        let mut previous_end = 0.0;
        for period in &self.operation {
            nonnegative(period.start_s, "operation.start_s")?;
            positive(period.end_s, "operation.end_s")?;
            if period.start_s >= period.end_s
                || period.end_s > self.horizon_s
                || period.start_s < previous_end
            {
                return Err(
                    "operation periods must be ordered, non-overlapping, and within the horizon"
                        .into(),
                );
            }
            if !period.power_fraction.is_finite() || !(0.0..=1.0).contains(&period.power_fraction) {
                return Err("power_fraction must be in [0,1]".into());
            }
            previous_end = period.end_s;
        }
        let mut outages = self.planned_outages.clone();
        outages.sort_by(|a, b| {
            a.start_s
                .total_cmp(&b.start_s)
                .then(a.end_s.total_cmp(&b.end_s))
        });
        let mut outage_end = 0.0;
        for outage in &outages {
            nonnegative(outage.start_s, "outage.start_s")?;
            positive(outage.end_s, "outage.end_s")?;
            if outage.start_s >= outage.end_s
                || outage.end_s > self.horizon_s
                || outage.start_s < outage_end
            {
                return Err(
                    "planned outages must be ordered/non-overlapping and within the horizon".into(),
                );
            }
            outage_end = outage.end_s;
        }
        let mut import_times = BTreeSet::new();
        for import in &self.imports {
            nonnegative(import.at_s, "import.at_s")?;
            nonnegative(import.mass_kg, "import.mass_kg")?;
            if import.at_s > self.horizon_s || !import_times.insert(import.at_s.to_bits()) {
                return Err("imports must have unique times within the horizon".into());
            }
            if import.provenance.trim().is_empty() {
                return Err("import provenance is required".into());
            }
        }
        let mut components = BTreeSet::new();
        for limit in &self.service_limits {
            if limit.component_id.trim().is_empty() || !components.insert(&limit.component_id) {
                return Err("service limits need unique nonempty component IDs".into());
            }
            if limit.response_id.trim().is_empty()
                || limit.metric.trim().is_empty()
                || limit.unit.trim().is_empty()
            {
                return Err("service limit response, metric, and unit are required".into());
            }
            positive(limit.limit, "service limit")?;
            if limit.provenance.trim().is_empty() {
                return Err("service-limit provenance is required".into());
            }
            match (limit.class, limit.replacement_duration_s) {
                (ComponentClass::Permanent, None) => {}
                (ComponentClass::Permanent, Some(_)) => {
                    return Err("permanent components cannot have a replacement duration".into());
                }
                (ComponentClass::Replaceable, Some(duration)) => {
                    positive(duration, "replacement_duration_s")?;
                }
                (ComponentClass::Replaceable, None) => {
                    return Err("replaceable components require a replacement duration".into());
                }
            }
        }
        for (fraction, name) in [
            (
                self.energy.alpha_deposition_fraction,
                "alpha_deposition_fraction",
            ),
            (
                self.energy.thermal_to_electric_efficiency,
                "thermal_to_electric_efficiency",
            ),
            (
                self.energy.transport_heat_recovery_fraction,
                "transport_heat_recovery_fraction",
            ),
        ] {
            if let Some(v) = fraction
                && (!v.is_finite() || !(0.0..=1.0).contains(&v))
            {
                return Err(format!("{name} must be in [0,1]"));
            }
        }
        for (power, name) in [
            (
                self.energy.auxiliary_power_mw_while_operating,
                "auxiliary_power_mw_while_operating",
            ),
            (
                self.energy.auxiliary_power_mw_while_off,
                "auxiliary_power_mw_while_off",
            ),
        ] {
            if let Some(v) = power {
                nonnegative(v, name)?;
            }
        }
        if self.provenance.trim().is_empty() || self.energy.provenance.trim().is_empty() {
            return Err("assumption provenance is required".into());
        }
        Ok(())
    }
}
