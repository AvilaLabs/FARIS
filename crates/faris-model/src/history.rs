//! Explicit operating-history assumptions. Values are scenario inputs, not
//! hidden defaults or qualified design limits.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const OPERATING_HISTORY_VERSION: &str = "faris-operating-history/v0.1";
/// Maximum nominal integration intervals for a bounded, cancellable history.
pub const MAX_HISTORY_NOMINAL_STEPS: f64 = 4_000_000.0;

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
    /// Optional per-event durations: the k-th replacement of the component takes the k-th
    /// entry; later replacements use `replacement_duration_s`. Absent = every event uses it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement_durations_s: Option<Vec<f64>>,
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
            return Err("history exceeds the 4,000,000-step nominal bound".into());
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
        // A component may carry several limits, each on its own response
        // (for example one per named region); the component is replaced when
        // any of them is reached, so they must agree on class and duration.
        let mut pairs = BTreeSet::new();
        #[allow(clippy::type_complexity)]
        let mut shared: std::collections::BTreeMap<
            &str,
            (ComponentClass, Option<u64>, Option<Vec<u64>>),
        > = std::collections::BTreeMap::new();
        for limit in &self.service_limits {
            if limit.component_id.trim().is_empty()
                || !pairs.insert((&limit.component_id, &limit.response_id))
            {
                return Err(
                    "service limits need nonempty component IDs and unique component/response pairs"
                        .into(),
                );
            }
            let key = (
                limit.class,
                limit.replacement_duration_s.map(f64::to_bits),
                limit
                    .replacement_durations_s
                    .as_ref()
                    .map(|d| d.iter().map(|v| v.to_bits()).collect::<Vec<_>>()),
            );
            if *shared
                .entry(&limit.component_id)
                .or_insert_with(|| key.clone())
                != key
            {
                return Err(
                    "service limits on one component must share class, replacement duration and replacement_durations_s"
                        .into(),
                );
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
            if let Some(list) = &limit.replacement_durations_s {
                if limit.class != ComponentClass::Replaceable {
                    return Err(
                        "only replaceable components can have replacement_durations_s".into(),
                    );
                }
                if list.is_empty() || list.len() > 10_000 {
                    return Err(
                        "replacement_durations_s must hold 1 to 10,000 entries (omit it to use replacement_duration_s)"
                            .into(),
                    );
                }
                for d in list {
                    positive(*d, "replacement_durations_s entry")?;
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    const ASSUMPTIONS: &str =
        include_str!("../../../scenarios/arc-inspired/demountable-magnet-assumptions.json");

    fn limit(response_id: &str) -> ServiceLimit {
        ServiceLimit {
            component_id: "magnets".into(),
            class: ComponentClass::Replaceable,
            response_id: response_id.into(),
            metric: "fast_neutron_flux_region_average".into(),
            unit: "neutrons/m\u{b2}".into(),
            limit: 3e22,
            replacement_duration_s: Some(1e7),
            replacement_durations_s: None,
            provenance: "authored test limit".into(),
        }
    }

    #[test]
    fn several_limits_on_one_component_validate_when_consistent() {
        let mut a: OperatingHistoryAssumptions = serde_json::from_str(ASSUMPTIONS).unwrap();
        a.service_limits = vec![limit("r-a"), limit("r-b"), limit("r-c")];
        a.validate().unwrap();
        // The same response twice on one component is a duplicate.
        a.service_limits.push(limit("r-a"));
        assert!(a.validate().is_err());
        a.service_limits.pop();
        // Limits on one component must agree on class and replacement duration.
        a.service_limits[1].replacement_duration_s = Some(2e7);
        assert!(a.validate().is_err());
        a.service_limits[1].replacement_duration_s = Some(1e7);
        a.service_limits[2].class = ComponentClass::Permanent;
        a.service_limits[2].replacement_duration_s = None;
        assert!(a.validate().is_err());
    }

    #[test]
    fn per_event_durations_validate_and_must_agree_on_a_component() {
        let mut a: OperatingHistoryAssumptions = serde_json::from_str(ASSUMPTIONS).unwrap();
        a.service_limits = vec![limit("r-a"), limit("r-b")];
        for l in &mut a.service_limits {
            l.replacement_durations_s = Some(vec![1e6, 2e6]);
        }
        a.validate().unwrap();
        a.service_limits[1].replacement_durations_s = Some(vec![1e6, 3e6]);
        assert!(a.validate().is_err());
        a.service_limits[1].replacement_durations_s = None;
        assert!(a.validate().is_err());
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            for l in &mut a.service_limits {
                l.replacement_durations_s = Some(vec![1e6, bad]);
            }
            assert!(a.validate().is_err(), "{bad}");
        }
        for l in &mut a.service_limits {
            l.replacement_durations_s = Some(vec![]);
        }
        assert!(a.validate().is_err());
    }

    #[test]
    fn demountable_preset_carries_three_regional_fast_flux_limits() {
        let a: OperatingHistoryAssumptions = serde_json::from_str(ASSUMPTIONS).unwrap();
        a.validate().unwrap();
        let magnets: Vec<_> = a
            .service_limits
            .iter()
            .filter(|l| l.component_id == "magnets")
            .collect();
        assert_eq!(magnets.len(), 3);
        for l in magnets {
            assert_eq!(l.limit, 3e22);
            assert_eq!(l.metric, "fast_neutron_flux_region_average");
            assert!(l.provenance.contains("arXiv:1409.3540"));
        }
        assert!(a.service_limits.iter().all(|l| l.metric
            != "energy_integrated_component_average_neutron_flux"
            || l.component_id != "magnets"));
    }
}
