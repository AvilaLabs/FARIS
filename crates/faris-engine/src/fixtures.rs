//! Small synthetic histories and ensembles for tests and development checks.
//!
//! Compiled only for the engine's own tests and when the `fixtures` feature is
//! on; no normal build contains it. Nothing here describes a real plant or a
//! real transport result.

use crate::history::{
    DrivingCovariance, ELEMENTARY_CHARGE_J_PER_EV, ScalarRate, TransportDrivingRates,
};
use crate::history_ensemble::{EnsembleSettings, HistoryEnsemble, run_history_ensemble};
use crate::jobs::Cancellation;
use faris_model::history::{
    ComponentClass, EnergyAssumptions, OPERATING_HISTORY_VERSION, OperatingHistoryAssumptions,
    PowerPeriod, ServiceLimit,
};
use std::collections::BTreeMap;

/// A 100 s history in which the replaceable magnets are swapped about three
/// times and the permanent blanket never reaches its limit.
pub fn assumptions() -> OperatingHistoryAssumptions {
    OperatingHistoryAssumptions {
        schema_version: OPERATING_HISTORY_VERSION.into(),
        horizon_s: 100.0,
        maximum_step_s: 10.0,
        snapshot_interval_s: 10.0,
        initial_available_tritium_kg: 1.0,
        initial_in_process_tritium_kg: 0.0,
        initial_in_process_release_s: 0.0,
        startup_reserve_kg: 0.0,
        restart_inventory_kg: 0.1,
        recovery_fraction: 1.0,
        processing_delay_s: 0.0,
        operation: vec![PowerPeriod {
            start_s: 0.0,
            end_s: 100.0,
            power_fraction: 1.0,
        }],
        planned_outages: vec![],
        imports: vec![],
        service_limits: vec![
            ServiceLimit {
                component_id: "magnets".into(),
                class: ComponentClass::Replaceable,
                response_id: "flux-magnets".into(),
                metric: "energy_integrated_component_average_neutron_flux".into(),
                unit: "neutrons/m²".into(),
                limit: 2.6e11,
                replacement_duration_s: Some(5.0),
                replacement_durations_s: None,
                provenance: "synthetic test fixture".into(),
            },
            ServiceLimit {
                component_id: "blanket".into(),
                class: ComponentClass::Permanent,
                response_id: "flux-blanket".into(),
                metric: "energy_integrated_component_average_neutron_flux".into(),
                unit: "neutrons/m²".into(),
                limit: 1.0e15,
                replacement_duration_s: None,
                replacement_durations_s: None,
                provenance: "synthetic test fixture".into(),
            },
        ],
        energy: EnergyAssumptions {
            alpha_deposition_fraction: Some(1.0),
            thermal_to_electric_efficiency: Some(0.4),
            transport_heat_recovery_fraction: Some(0.9),
            auxiliary_power_mw_while_operating: Some(0.1),
            auxiliary_power_mw_while_off: Some(0.01),
            provenance: "synthetic test fixture".into(),
        },
        provenance: "synthetic test fixture".into(),
    }
}

fn scalar(mean: f64, relative: f64, unit: &str, id: &str) -> ScalarRate {
    ScalarRate {
        mean,
        standard_error: Some(mean * relative),
        unit: unit.into(),
        response_id: id.into(),
    }
}

/// Driving rates with the given relative standard error on every rate and no
/// covariance (the form of every real record today). `artifact` stands in for
/// the transport artifact hash, so two arrangements can be told apart.
pub fn rates_without_covariance(relative: f64, artifact: char) -> TransportDrivingRates {
    let q_ev = 17.6e6;
    let reactions = 1.0e6 / (q_ev * ELEMENTARY_CHARGE_J_PER_EV);
    let flux_unit = "neutrons/m²/s";
    let mut flux = BTreeMap::new();
    flux.insert(
        "blanket".to_string(),
        scalar(1.0e10, relative, flux_unit, "flux-blanket"),
    );
    flux.insert(
        "magnets".to_string(),
        scalar(1.0e10, relative, flux_unit, "flux-magnets"),
    );
    TransportDrivingRates {
        reference_fusion_power_mw: 1.0,
        fusion_reaction_rate_per_s: reactions,
        neutron_source_rate_per_s: reactions,
        total_reaction_energy_ev: q_ev,
        primary_neutron_energy_ev: 14.1e6,
        breeder_h3_per_source_neutron: scalar(
            1.1,
            relative,
            "particles/source_neutron",
            "blanket-tritium",
        ),
        component_average_flux_n_m2_s: flux,
        region_flux_n_m2_s: Default::default(),
        transport_deposited_heat_w: Some(scalar(1.0e6, relative, "W", "heating-total")),
        scenario_sha256: artifact.to_string().repeat(64),
        transport_artifact_sha256: artifact.to_ascii_uppercase().to_string().repeat(64),
        solver_digest: format!("sha256:{}", "c".repeat(64)),
        nuclear_data_digest: format!("sha256:{}", "d".repeat(64)),
        covariance: None,
    }
}

/// A covariance consistent with the recorded standard errors, with the same
/// correlation between every pair of rates. For development checks and tests
/// only: it is not a measured covariance.
pub fn synthetic_covariance(rates: &TransportDrivingRates, correlation: f64) -> DrivingCovariance {
    let entries = rates.covariance_entries();
    let n = entries.len();
    let sd: Vec<f64> = entries
        .iter()
        .map(|e| e.1.standard_error.unwrap_or(0.0))
        .collect();
    let mut monte_carlo = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..n {
            monte_carlo[i * n + j] = if i == j {
                sd[i] * sd[i]
            } else {
                correlation * sd[i] * sd[j]
            };
        }
    }
    DrivingCovariance {
        method: "synthetic-fixture-covariance".into(),
        batches: 100,
        rate_ids: entries.iter().map(|e| e.0.to_string()).collect(),
        monte_carlo,
        volume_variance: vec![0.0; n],
    }
}

/// Rates with a synthetic covariance attached.
pub fn rates_with_covariance(
    relative: f64,
    correlation: f64,
    artifact: char,
) -> TransportDrivingRates {
    let mut rates = rates_without_covariance(relative, artifact);
    rates.covariance = Some(synthetic_covariance(&rates, correlation));
    rates
}

/// Settings for a quick, repeatable ensemble.
pub fn settings(samples: u32, seed: u64) -> EnsembleSettings {
    EnsembleSettings {
        samples,
        seed: Some(seed),
        threads: Some(1),
    }
}

/// A small evaluated ensemble of the fixture history.
pub fn ensemble(samples: u32, seed: u64, artifact: char) -> HistoryEnsemble {
    run_history_ensemble(
        &rates_with_covariance(0.06, 0.4, artifact),
        &assumptions(),
        &settings(samples, seed),
        &Cancellation::default(),
        &|_, _| {},
    )
    .expect("the fixture ensemble runs")
}
