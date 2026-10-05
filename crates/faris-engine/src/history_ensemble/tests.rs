use super::*;
use crate::history::{
    DrivingCovariance, ELEMENTARY_CHARGE_J_PER_EV, RegionFluxRate, ScalarRate,
    run_operating_history,
};
use crate::transport::{
    NormalizedTally, NormalizedTransportResult, PhysicalUnit, ResponseCovariance, TallyEstimator,
    ToolIdentity,
};
use faris_model::history::{ComponentClass, EnergyAssumptions, PowerPeriod, ServiceLimit};
use faris_model::transport::{
    DtSource, HeatingConvention, HeatingParticleScope, ProducedParticle, ResponseDomain,
    ScoreDefinition,
};

fn assumptions() -> OperatingHistoryAssumptions {
    OperatingHistoryAssumptions {
        schema_version: faris_model::history::OPERATING_HISTORY_VERSION.into(),
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
                component_id: "blanket".into(),
                class: ComponentClass::Replaceable,
                response_id: "flux-blanket".into(),
                metric: "energy_integrated_component_average_neutron_flux".into(),
                unit: "neutrons/m²".into(),
                limit: 1.0e11,
                replacement_duration_s: Some(10.0),
                provenance: "test".into(),
            },
            ServiceLimit {
                component_id: "magnet".into(),
                class: ComponentClass::Permanent,
                response_id: "flux-magnet".into(),
                metric: "energy_integrated_component_average_neutron_flux".into(),
                unit: "neutrons/m²".into(),
                limit: 3.0e11,
                replacement_duration_s: None,
                provenance: "test".into(),
            },
        ],
        energy: EnergyAssumptions {
            alpha_deposition_fraction: Some(1.0),
            thermal_to_electric_efficiency: Some(0.4),
            transport_heat_recovery_fraction: Some(0.9),
            auxiliary_power_mw_while_operating: Some(0.1),
            auxiliary_power_mw_while_off: Some(0.01),
            provenance: "test".into(),
        },
        provenance: "unit test".into(),
    }
}

fn scalar(mean: f64, se: f64, unit: &str, id: &str) -> ScalarRate {
    ScalarRate {
        mean,
        standard_error: Some(se),
        unit: unit.into(),
        response_id: id.into(),
    }
}

/// Rates with a 4x4 covariance (breeder, blanket flux, magnet flux, heat).
/// `relative` scales every standard error relative to its mean; flux entries
/// get a volume term of the same size as their Monte Carlo term.
fn rates_with(relative: f64, correlation: f64) -> TransportDrivingRates {
    let q_ev = 17.6e6;
    let reactions = 1.0e6 / (q_ev * ELEMENTARY_CHARGE_J_PER_EV);
    let means = [1.1, 1.0e10, 1.0e10, 1.0e6];
    let mc_sd: Vec<f64> = means.iter().map(|m| m * relative).collect();
    let n = 4;
    let mut monte_carlo = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..n {
            monte_carlo[i * n + j] = if i == j {
                mc_sd[i] * mc_sd[i]
            } else {
                correlation * mc_sd[i] * mc_sd[j]
            };
        }
    }
    let volume_variance = vec![0.0, mc_sd[1].powi(2), mc_sd[2].powi(2), 0.0];
    let total_se = |i: usize| (monte_carlo[i * n + i] + volume_variance[i]).sqrt();
    let flux_unit = "neutrons/m²/s";
    let mut flux = BTreeMap::new();
    flux.insert(
        "blanket".to_string(),
        scalar(means[1], total_se(1), flux_unit, "flux-blanket"),
    );
    flux.insert(
        "magnet".to_string(),
        scalar(means[2], total_se(2), flux_unit, "flux-magnet"),
    );
    TransportDrivingRates {
        reference_fusion_power_mw: 1.0,
        fusion_reaction_rate_per_s: reactions,
        neutron_source_rate_per_s: reactions,
        total_reaction_energy_ev: q_ev,
        primary_neutron_energy_ev: 14.1e6,
        breeder_h3_per_source_neutron: scalar(
            means[0],
            total_se(0),
            "particles/source_neutron",
            "blanket-tritium",
        ),
        component_average_flux_n_m2_s: flux,
        region_flux_n_m2_s: BTreeMap::new(),
        transport_deposited_heat_w: Some(scalar(means[3], total_se(3), "W", "heating-total")),
        scenario_sha256: "a".repeat(64),
        transport_artifact_sha256: "b".repeat(64),
        solver_digest: format!("sha256:{}", "c".repeat(64)),
        nuclear_data_digest: format!("sha256:{}", "d".repeat(64)),
        covariance: Some(DrivingCovariance {
            method: "test-covariance".into(),
            batches: 100,
            rate_ids: vec![
                "blanket-tritium".into(),
                "flux-blanket".into(),
                "flux-magnet".into(),
                "heating-total".into(),
            ],
            monte_carlo,
            volume_variance,
        }),
    }
}

fn run(
    rates: &TransportDrivingRates,
    settings: &EnsembleSettings,
) -> Result<HistoryEnsemble, EnsembleError> {
    run_history_ensemble(
        rates,
        &assumptions(),
        settings,
        &Cancellation::default(),
        &|_, _| {},
    )
}

fn settings(samples: u32, threads: usize) -> EnsembleSettings {
    EnsembleSettings {
        samples,
        seed: Some(12345),
        threads: Some(threads),
    }
}

#[test]
fn rates_fixture_is_valid() {
    rates_with(0.05, 0.4).validate().unwrap();
}

#[test]
fn zero_covariance_reproduces_deterministic_history_exactly() {
    let mut rates = rates_with(0.0, 0.0);
    for r in rates.component_average_flux_n_m2_s.values_mut() {
        r.standard_error = Some(0.0);
    }
    rates.breeder_h3_per_source_neutron.standard_error = Some(0.0);
    rates
        .transport_deposited_heat_w
        .as_mut()
        .unwrap()
        .standard_error = Some(0.0);
    rates.validate().unwrap();
    let ensemble = run(&rates, &settings(6, 2)).unwrap();
    assert_eq!(ensemble.status, EnsembleStatus::Evaluated);
    let deterministic = run_operating_history(&assumptions(), &rates).unwrap();
    let last = deterministic.snapshots.last().unwrap();
    let nominal = ensemble.nominal.as_ref().unwrap();
    assert_eq!(
        nominal.final_available_tritium_kg,
        last.available_tritium_kg
    );
    for sample in &ensemble.samples {
        let mut a = sample.clone();
        a.index = 0;
        assert_eq!(&a, nominal);
        assert_eq!(a.replacements, last.component_replacements);
        assert_eq!(a.full_power_time_s, last.cumulative_full_power_seconds);
        assert_eq!(
            a.cumulative_net_electricity_mwh,
            last.cumulative_net_electricity_mwh
        );
    }
    // The fixture exercises both a replacement and the permanent stop.
    assert!(nominal.replacements["blanket"] >= 1);
    assert_eq!(nominal.outcome, HistoryOutcome::PermanentComponentLimit);
}

#[test]
fn identical_for_any_thread_count() {
    let rates = rates_with(0.2, 0.5);
    let one = serde_json::to_string(&run(&rates, &settings(12, 1)).unwrap()).unwrap();
    for threads in [2, 4] {
        let other = serde_json::to_string(&run(&rates, &settings(12, threads)).unwrap()).unwrap();
        assert_eq!(one, other, "threads = {threads}");
    }
    // And the draws really vary between samples.
    let ensemble = run(&rates, &settings(12, 3)).unwrap();
    let times: BTreeSet<u64> = ensemble
        .samples
        .iter()
        .map(|s| s.terminal_time_s.to_bits())
        .collect();
    assert!(times.len() > 1);
}

#[test]
fn drawn_moments_match_mean_and_covariance() {
    let rates = rates_with(0.05, 0.6);
    let covariance = rates.covariance.clone().unwrap();
    let sampler = Sampler::new(&rates).unwrap();
    let n = sampler.means.len();
    let count = 20_000_usize;
    let mut sum = vec![0.0; n];
    let mut draws = Vec::with_capacity(count);
    for i in 0..count {
        let d = sampler.draw(777, i as u64);
        assert!(d.accepted && d.rejections == 0);
        for (s, v) in sum.iter_mut().zip(&d.values) {
            *s += v;
        }
        draws.push(d.values);
    }
    let mean: Vec<f64> = sum.iter().map(|s| s / count as f64).collect();
    let total = |i: usize, j: usize| {
        covariance.monte_carlo[i * n + j]
            + if i == j {
                covariance.volume_variance[i]
            } else {
                0.0
            }
    };
    for i in 0..n {
        let se = (total(i, i) / count as f64).sqrt();
        assert!(
            (mean[i] - sampler.means[i]).abs() < 4.0 * se,
            "mean {i}: {} vs {}",
            mean[i],
            sampler.means[i]
        );
        for j in 0..n {
            let sample_cov = draws
                .iter()
                .map(|x| (x[i] - mean[i]) * (x[j] - mean[j]))
                .sum::<f64>()
                / (count as f64 - 1.0);
            let se = ((total(i, i) * total(j, j) + total(i, j).powi(2)) / count as f64).sqrt();
            assert!(
                (sample_cov - total(i, j)).abs() < 4.0 * se,
                "cov {i},{j}: {sample_cov} vs {}",
                total(i, j)
            );
        }
    }
}

#[test]
fn semi_definite_covariance_is_factored_and_sampled() {
    let a = [4.0, 6.0, 6.0, 9.0];
    let l = factor_covariance(&a, 2).unwrap();
    for i in 0..2 {
        for j in 0..2 {
            let v: f64 = (0..2).map(|k| l[i * 2 + k] * l[j * 2 + k]).sum();
            assert!((v - a[i * 2 + j]).abs() < 1e-9);
        }
    }
    let mut rates = rates_with(0.05, 1.0);
    // Perfect correlation between the two fluxes is semi-definite.
    rates.validate().unwrap();
    let sampler = Sampler::new(&rates).unwrap();
    for i in 0..50 {
        let d = sampler.draw(1, i);
        assert!(d.accepted);
    }
    // Zero-variance rate with a nonzero covariance, and non-PSD, are refused.
    assert!(factor_covariance(&[0.0, 1.0, 1.0, 1.0], 2).is_err());
    assert!(factor_covariance(&[1.0, 2.0, 2.0, 1.0], 2).is_err());
    assert!(factor_covariance(&[1.0, 0.5, 0.4, 1.0], 2).is_err());
    rates.covariance.as_mut().unwrap().monte_carlo[1] *= 2.0;
    assert!(rates.validate().is_err());
}

#[test]
fn perfectly_correlated_draws_move_together() {
    let rates = rates_with(0.05, 1.0);
    let sampler = Sampler::new(&rates).unwrap();
    // Flux entries (1 and 2) are perfectly correlated in their Monte Carlo
    // parts; remove the independent volume term to see it exactly.
    let mut sampler = sampler;
    sampler.volume_sd = vec![0.0; 4];
    for i in 0..20 {
        let d = sampler.draw(9, i).values;
        let a = d[1] - 1.0e10;
        let b = d[2] - 1.0e10;
        assert!((a - b).abs() <= 1e-9 * a.abs().max(1.0), "{a} vs {b}");
    }
}

// Verifies: UNC-011
#[test]
fn missing_covariance_is_not_evaluated_with_exact_text() {
    let mut rates = rates_with(0.05, 0.0);
    rates.covariance = None;
    let ensemble = run(&rates, &settings(10, 1)).unwrap();
    assert_eq!(
        ensemble.status,
        EnsembleStatus::NotEvaluated {
            why: "This transport record has standard errors but no covariance between its results, so correlated sampling is not possible".into(),
            next_step: "Rerun transport with this FARIS version to record batch-resolved results".into(),
        }
    );
    assert!(ensemble.samples.is_empty() && ensemble.nominal.is_none());
}

#[test]
fn large_relative_errors_fail_closed_on_rejections() {
    // 50 % relative error: a few percent of draws are negative.
    let rates = rates_with(0.5, 0.0);
    let ensemble = run(&rates, &settings(200, 1)).unwrap();
    let EnsembleStatus::NotEvaluated { why, next_step } = &ensemble.status else {
        panic!("expected NotEvaluated");
    };
    assert!(
        why.starts_with("Gaussian sampling of the transport means gave non-physical rates in ")
    );
    assert!(why.ends_with(" % of draws; the relative errors are too large for this approximation"));
    assert_eq!(next_step, "run more histories or use variance reduction");
    assert!(ensemble.rejections as f64 > 0.01 * 200.0);
    // A tight covariance has no rejections and is evaluated.
    let ok = run(&rates_with(0.05, 0.0), &settings(20, 1)).unwrap();
    assert_eq!(ok.status, EnsembleStatus::Evaluated);
    assert_eq!(ok.rejections, 0);
}

#[test]
fn wilson_and_order_statistic_intervals_on_known_cases() {
    let (lo, hi) = wilson_interval(5, 10);
    assert!((lo - 0.2366).abs() < 5e-4 && (hi - 0.7634).abs() < 5e-4);
    let (lo, hi) = wilson_interval(0, 20);
    assert_eq!(lo, 0.0);
    assert!((hi - 0.1611).abs() < 5e-4);
    let (lo, hi) = wilson_interval(20, 20);
    assert_eq!(hi, 1.0);
    assert!((lo - 0.8389).abs() < 5e-4);
    // Median of 10: ranks 2 and 9 (coverage 97.9 %).
    assert_eq!(quantile_ci_ranks(10, 0.5), (Some(2), Some(9)));
    // Too few samples for a 5 % quantile interval: no bounds.
    assert_eq!(quantile_ci_ranks(10, 0.05), (None, Some(3)));
    // Large-sample sanity: rank spread ~ 1.96 sqrt(n p (1-p)).
    let (l, u) = quantile_ci_ranks(200, 0.5);
    assert_eq!((l, u), (Some(86), Some(115)));
    let sorted: Vec<f64> = (1..=11).map(f64::from).collect();
    assert_eq!(quantile_sorted(&sorted, 0.5), 6.0);
    assert!((quantile_sorted(&sorted, 0.05) - 1.5).abs() < 1e-12);
}

#[test]
fn cancellation_stops_promptly_and_returns_no_partial_result() {
    let rates = rates_with(0.05, 0.3);
    let pre = Cancellation::default();
    pre.cancel();
    let result = run_history_ensemble(&rates, &assumptions(), &settings(50, 2), &pre, &|_, _| {});
    assert!(matches!(result, Err(EnsembleError::Cancelled)));

    let cancellation = Cancellation::default();
    let cancelled_at: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let callback = |done: usize, _total: usize| {
        if done == 3 {
            *cancelled_at.lock().unwrap() = Some(std::time::Instant::now());
            cancellation.cancel();
        }
    };
    let result = run_history_ensemble(
        &rates,
        &assumptions(),
        &settings(2000, 2),
        &cancellation,
        &callback,
    );
    let elapsed = cancelled_at.lock().unwrap().unwrap().elapsed();
    assert!(matches!(result, Err(EnsembleError::Cancelled)));
    assert!(
        elapsed < std::time::Duration::from_millis(100),
        "took {elapsed:?}"
    );
}

#[test]
fn progress_reports_every_sample() {
    let rates = rates_with(0.05, 0.3);
    let count = AtomicUsize::new(0);
    let max_seen = AtomicUsize::new(0);
    run_history_ensemble(
        &rates,
        &assumptions(),
        &settings(8, 3),
        &Cancellation::default(),
        &|done, total| {
            assert_eq!(total, 8);
            count.fetch_add(1, Ordering::SeqCst);
            max_seen.fetch_max(done, Ordering::SeqCst);
        },
    )
    .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 8);
    assert_eq!(max_seen.load(Ordering::SeqCst), 8);
}

#[test]
fn seed_derivation_is_stable() {
    let a = assumptions();
    let seed = derive_seed(&"b".repeat(64), &a).unwrap();
    assert_eq!(seed, GOLDEN_SEED, "seed derivation changed: {seed:#018x}");
    assert_eq!(seed, derive_seed(&"b".repeat(64), &a).unwrap());
    assert_ne!(seed, derive_seed(&"c".repeat(64), &a).unwrap());
    let mut other = a.clone();
    other.horizon_s += 1.0;
    assert_ne!(seed, derive_seed(&"b".repeat(64), &other).unwrap());
    // Default settings use it.
    let ensemble = run(
        &rates_with(0.05, 0.0),
        &EnsembleSettings {
            samples: 2,
            seed: None,
            threads: Some(1),
        },
    )
    .unwrap();
    assert_eq!(ensemble.seed, seed);
}

const GOLDEN_SEED: u64 = 0x9328_5df4_3189_bc2d;

#[test]
fn generator_streams_are_distinct_and_reproducible() {
    let mut a = Rng::stream(1, 0);
    let mut b = Rng::stream(1, 0);
    let mut c = Rng::stream(1, 1);
    let mut d = Rng::stream(2, 0);
    let (x, y, z, w) = (a.next_u64(), b.next_u64(), c.next_u64(), d.next_u64());
    assert_eq!(x, y);
    assert!(x != z && x != w && z != w);
    // Normals: mean ~ 0, variance ~ 1.
    let mut rng = Rng::stream(5, 5);
    let samples: Vec<f64> = (0..100_000).map(|_| rng.next_normal()).collect();
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    let var = samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / samples.len() as f64;
    assert!(mean.abs() < 0.015 && (var - 1.0).abs() < 0.02);
}

#[test]
fn summary_has_intervals_bands_and_fractions() {
    let rates = rates_with(0.1, 0.3);
    let ensemble = run(&rates, &settings(100, 2)).unwrap();
    let summary = ensemble.summary.as_ref().unwrap();
    let full_power = summary
        .continuous
        .iter()
        .find(|c| c.name == "full_power_time_s")
        .unwrap();
    assert_eq!(full_power.n, 100);
    assert!(full_power.p5.value <= full_power.p50.value);
    assert!(full_power.p50.value <= full_power.p95.value);
    assert!(full_power.p5.ci95_low.is_some() && full_power.p95.ci95_high.is_some());
    let status = summary
        .discrete
        .iter()
        .find(|d| d.name == "terminal_status")
        .unwrap();
    assert_eq!(
        status.categories.values().map(|p| p.count).sum::<u32>(),
        100
    );
    let replacements = summary
        .discrete
        .iter()
        .find(|d| d.name == "replacements:blanket")
        .unwrap();
    for p in replacements.categories.values() {
        assert!(p.wilson95_low <= p.fraction && p.fraction <= p.wilson95_high);
    }
    assert_eq!(summary.time_grid_s.len(), SERIES_GRID_POINTS);
    let band = summary
        .series_bands
        .iter()
        .find(|b| b.name == "fluence_n_m2:magnet")
        .unwrap();
    assert_eq!(band.p50.len(), SERIES_GRID_POINTS);
    assert_eq!(band.n[0], 100);
    assert!(band.p5[10].unwrap() <= band.p95[10].unwrap());
}

#[test]
fn paired_comparison_counts_pairs_and_refuses_dependent_runs() {
    let rates_a = rates_with(0.1, 0.3);
    let mut rates_b = rates_a.clone();
    rates_b.transport_artifact_sha256 = "e".repeat(64);
    rates_b
        .component_average_flux_n_m2_s
        .get_mut("magnet")
        .unwrap()
        .mean *= 1.2;
    let a = run(
        &rates_a,
        &EnsembleSettings {
            seed: Some(1),
            ..settings(40, 2)
        },
    )
    .unwrap();
    let b = run(
        &rates_b,
        &EnsembleSettings {
            seed: Some(2),
            ..settings(40, 2)
        },
    )
    .unwrap();
    let comparison = compare_ensembles(&a, &b).unwrap();
    assert_eq!(comparison.pairs, 40);
    let out = comparison
        .outputs
        .iter()
        .find(|o| o.name == "terminal_time_s")
        .unwrap();
    assert_eq!(
        out.a_less_than_b.count + out.a_equal_to_b.count + out.a_greater_than_b.count,
        40
    );
    // Higher magnet flux stops operation sooner in B.
    assert!(out.a_greater_than_b.count > out.a_less_than_b.count);
    assert!(compare_ensembles(&a, &a).is_err());
    let mut same_artifact = rates_a.clone();
    same_artifact
        .component_average_flux_n_m2_s
        .get_mut("magnet")
        .unwrap()
        .mean *= 1.2;
    let c = run(
        &same_artifact,
        &EnsembleSettings {
            seed: Some(3),
            ..settings(40, 2)
        },
    )
    .unwrap();
    assert!(compare_ensembles(&a, &c).is_err());
}

#[test]
fn settings_are_bounded() {
    let rates = rates_with(0.05, 0.0);
    assert!(run(&rates, &settings(0, 1)).is_err());
    assert!(run(&rates, &settings(MAX_SAMPLES + 1, 1)).is_err());
    assert!(run(&rates, &settings(5, 0)).is_err());
}

// Verifies: UNC-011
#[test]
fn driving_covariance_selects_scales_and_separates_volume_variance() {
    let source_rate = 525.0e6 / (17.6e6 * ELEMENTARY_CHARGE_J_PER_EV);
    let volume: f64 = 20.0;
    let volume_se: f64 = 1.0;
    let flux_mean: f64 = 1.0e13;
    let flux_integrated = flux_mean * volume;
    let flux_mc_se: f64 = 5.0e11;
    let flux_integrated_se = flux_mc_se * volume;
    let flux_total_se = (flux_mc_se.powi(2) + (flux_mean * volume_se / volume).powi(2)).sqrt();
    let breeder_integrated = 1.1 * source_rate;
    let breeder_se = 0.01 * source_rate;
    let heat_se = 1.0e6;
    let tally = |id: &str,
                 domain: ResponseDomain,
                 score: ScoreDefinition,
                 unit: PhysicalUnit,
                 integrated_unit: PhysicalUnit,
                 integrated_mean: f64,
                 integrated_se: f64,
                 mean: f64,
                 se: f64,
                 vse: f64| NormalizedTally {
        response_id: id.into(),
        domain,
        score,
        estimator: TallyEstimator::Tracklength,
        mean,
        standard_error: se,
        unit,
        integrated_mean,
        integrated_standard_error: integrated_se,
        integrated_unit,
        volume_m3: volume,
        volume_standard_error_m3: vse,
    };
    let component = |id: &str| ResponseDomain::Component {
        component_id: id.into(),
    };
    let reactions = source_rate;
    let results = vec![
        tally(
            "blanket-tritium",
            component("blanket"),
            ScoreDefinition::ParticleProduction {
                particle: ProducedParticle::Tritium,
                score: "H3-production".into(),
            },
            PhysicalUnit::ParticlesPerCubicMetreSecond,
            PhysicalUnit::ParticlesPerSecond,
            breeder_integrated,
            breeder_se,
            breeder_integrated / volume,
            breeder_se / volume,
            0.0,
        ),
        tally(
            "blanket-flux",
            component("blanket"),
            ScoreDefinition::Flux,
            PhysicalUnit::NeutronsPerSquareMetreSecond,
            PhysicalUnit::NeutronMetresPerSecond,
            flux_integrated,
            flux_integrated_se,
            flux_mean,
            flux_total_se,
            volume_se,
        ),
        tally(
            "heating-total-whole-model",
            ResponseDomain::WholeModel,
            ScoreDefinition::Heating {
                convention: HeatingConvention::Heating,
                particle_scope: HeatingParticleScope::Total,
            },
            PhysicalUnit::WattsPerCubicMetre,
            PhysicalUnit::Watts,
            5.0e8,
            heat_se,
            5.0e8 / volume,
            heat_se / volume,
            0.0,
        ),
    ];
    // Matrix order differs from the driving order on purpose.
    let sds = [heat_se, flux_integrated_se, breeder_se];
    let mut integrated = vec![0.0; 9];
    for i in 0..3 {
        for j in 0..3 {
            integrated[i * 3 + j] = if i == j { 1.0 } else { 0.3 } * sds[i] * sds[j];
        }
    }
    let normalized = NormalizedTransportResult {
        schema_version: "faris-normalized-transport/v0.1".into(),
        scenario_id: "s".into(),
        scenario_sha256: "a".repeat(64),
        variant_id: "v".into(),
        source: DtSource {
            energy_per_reaction_ev: 17.6e6,
            neutron_energy_ev: 14.1e6,
            neutrons_per_reaction: 1.0,
            distribution_id: "d".into(),
        },
        solver: ToolIdentity {
            name: "OpenMC".into(),
            version: "0.15.3".into(),
            digest: format!("sha256:{}", "1".repeat(64)),
        },
        nuclear_data: ToolIdentity {
            name: "data".into(),
            version: "1".into(),
            digest: format!("sha256:{}", "2".repeat(64)),
        },
        histories: 1000,
        source_reaction_rate_per_s: reactions,
        source_neutron_rate_per_s: source_rate,
        results,
        response_covariance: Some(ResponseCovariance {
            method: "batch-means-sample-covariance/v1".into(),
            batches: 100,
            response_ids: vec![
                "heating-total-whole-model".into(),
                "blanket-flux".into(),
                "blanket-tritium".into(),
            ],
            integrated,
        }),
    };
    let rates =
        TransportDrivingRates::from_normalized(&normalized, 525.0, &"b".repeat(64)).unwrap();
    let cov = rates.covariance.as_ref().expect("covariance carried");
    assert_eq!(
        cov.rate_ids,
        [
            "blanket-tritium",
            "blanket-flux",
            "heating-total-whole-model"
        ]
    );
    let n = 3;
    // Volume term is separate and independent; totals equal the reported SEs.
    assert_eq!(cov.volume_variance[0], 0.0);
    assert_eq!(cov.volume_variance[2], 0.0);
    let flux = &rates.component_average_flux_n_m2_s["blanket"];
    let flux_total = cov.monte_carlo[n + 1] + cov.volume_variance[1];
    assert!((flux_total / flux.standard_error.unwrap().powi(2) - 1.0).abs() < 1e-9);
    assert!((cov.monte_carlo[n + 1] / flux_mc_se.powi(2) - 1.0).abs() < 1e-9);
    assert!((cov.volume_variance[1] / flux_mc_se.powi(2) - 1.0).abs() < 1e-9);
    let breeder = &rates.breeder_h3_per_source_neutron;
    assert!((cov.monte_carlo[0] / breeder.standard_error.unwrap().powi(2) - 1.0).abs() < 1e-9);
    // Off-diagonal in driving units: correlation 0.3 survives the scaling.
    let corr = cov.monte_carlo[1] / (cov.monte_carlo[0] * cov.monte_carlo[n + 1]).sqrt();
    assert!((corr - 0.3).abs() < 1e-9);
    // A missing entry or a wrong diagonal fails closed; no covariance gives None.
    let mut missing = normalized.clone();
    let rc = missing.response_covariance.as_mut().unwrap();
    rc.response_ids[1] = "other".into();
    assert!(TransportDrivingRates::from_normalized(&missing, 525.0, &"b".repeat(64)).is_err());
    let mut wrong = normalized.clone();
    wrong.response_covariance.as_mut().unwrap().integrated[4] *= 1.5;
    assert!(TransportDrivingRates::from_normalized(&wrong, 525.0, &"b".repeat(64)).is_err());
    let mut none = normalized;
    none.response_covariance = None;
    assert!(
        TransportDrivingRates::from_normalized(&none, 525.0, &"b".repeat(64))
            .unwrap()
            .covariance
            .is_none()
    );
    // JSON round trip keeps the covariance; old records omit the field.
    let json = serde_json::to_value(&rates).unwrap();
    let back: TransportDrivingRates = serde_json::from_value(json).unwrap();
    assert_eq!(back, rates);
    let mut stripped = serde_json::to_value(&rates).unwrap();
    stripped.as_object_mut().unwrap().remove("covariance");
    let old: TransportDrivingRates = serde_json::from_value(stripped).unwrap();
    assert!(old.covariance.is_none());
}

/// TEST-ONLY MEASUREMENT: builds a covariance from recorded standard errors
/// assuming all rates are INDEPENDENT. This is a timing fixture, not a physical
/// covariance, and is not reachable from the CLI. Run with
/// `FARIS_ENSEMBLE_TIMING_RATES=<rates or history json> cargo test --release
/// timing_with_synthetic_diagonal_covariance -- --ignored --nocapture`.
#[test]
#[ignore]
fn timing_with_synthetic_diagonal_covariance() {
    let path =
        std::env::var("FARIS_ENSEMBLE_TIMING_RATES").expect("set FARIS_ENSEMBLE_TIMING_RATES");
    let assumptions_path = std::env::var("FARIS_ENSEMBLE_TIMING_ASSUMPTIONS")
        .expect("set FARIS_ENSEMBLE_TIMING_ASSUMPTIONS");
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let rates_value = value.get("driving_rates").unwrap_or(&value).clone();
    let mut rates: TransportDrivingRates = serde_json::from_value(rates_value).unwrap();
    let assumptions: OperatingHistoryAssumptions =
        serde_json::from_slice(&std::fs::read(assumptions_path).unwrap()).unwrap();
    // FARIS_ENSEMBLE_TIMING_CAP=0.2 caps every relative standard error at 20 %
    // (test-only, so a record with very noisy fluxes still reaches the history
    // runs and can be timed); unset leaves the recorded errors untouched.
    if let Ok(cap) = std::env::var("FARIS_ENSEMBLE_TIMING_CAP") {
        let cap: f64 = cap.parse().unwrap();
        let cap_rate = |r: &mut ScalarRate| {
            r.standard_error = r.standard_error.map(|se| se.min(cap * r.mean.abs()));
        };
        cap_rate(&mut rates.breeder_h3_per_source_neutron);
        rates
            .component_average_flux_n_m2_s
            .values_mut()
            .for_each(cap_rate);
    }
    let entries = rates.covariance_entries();
    let n = entries.len();
    let mut monte_carlo = vec![0.0; n * n];
    for (i, (_, r)) in entries.iter().enumerate() {
        monte_carlo[i * n + i] = r.standard_error.unwrap().powi(2);
    }
    let ids = entries.iter().map(|e| e.0.to_string()).collect();
    rates.covariance = Some(DrivingCovariance {
        method: "TEST-ONLY-synthetic-independent-diagonal".into(),
        batches: 100,
        rate_ids: ids,
        monte_carlo,
        volume_variance: vec![0.0; n],
    });
    rates.validate().unwrap();
    for threads in [1, 7] {
        let start = std::time::Instant::now();
        let ensemble = run_history_ensemble(
            &rates,
            &assumptions,
            &EnsembleSettings {
                samples: 200,
                seed: Some(1),
                threads: Some(threads),
            },
            &Cancellation::default(),
            &|_, _| {},
        )
        .unwrap();
        println!(
            "threads={threads}: {:.2} s, status {:?}, rejections {}",
            start.elapsed().as_secs_f64(),
            ensemble.status,
            ensemble.rejections
        );
    }
}

/// Magnet with three regional fast-flux limits (3e11 n/m2 each, 10 s swap), no
/// energy-integrated limit, and uncorrelated region fluxes with the given
/// relative error. Matrix order: breeder, blanket flux, magnet flux, the three
/// regions in response-ID order, heating.
fn regional_case(
    region_means: [f64; 3],
    relative: f64,
) -> (OperatingHistoryAssumptions, TransportDrivingRates) {
    let mut a = assumptions();
    a.service_limits = ["r-inboard", "r-outboard", "r-port"]
        .iter()
        .map(|id| ServiceLimit {
            component_id: "magnet".into(),
            class: ComponentClass::Replaceable,
            response_id: (*id).into(),
            metric: crate::history::FAST_FLUX_REGION_METRIC.into(),
            unit: "neutrons/m²".into(),
            limit: 3.0e11,
            replacement_duration_s: Some(10.0),
            provenance: "test".into(),
        })
        .collect();
    let mut rates = rates_with(0.0, 0.0);
    let flux_unit = "neutrons/m²/s";
    for (id, mean) in ["r-inboard", "r-outboard", "r-port"]
        .iter()
        .zip(region_means)
    {
        rates.region_flux_n_m2_s.insert(
            (*id).into(),
            RegionFluxRate {
                component_id: "magnet".into(),
                region: None,
                energy_min_ev: 1.0e5,
                rate: scalar(mean, mean * relative, flux_unit, id),
            },
        );
    }
    let entries = rates.covariance_entries();
    let n = entries.len();
    let mut monte_carlo = vec![0.0; n * n];
    for (i, (_, rate)) in entries.iter().enumerate() {
        monte_carlo[i * n + i] = rate.standard_error.unwrap().powi(2);
    }
    let rate_ids: Vec<String> = entries.iter().map(|e| e.0.to_owned()).collect();
    rates.covariance = Some(DrivingCovariance {
        method: "test-covariance".into(),
        batches: 100,
        rate_ids,
        monte_carlo,
        volume_variance: vec![0.0; n],
    });
    rates.validate().unwrap();
    (a, rates)
}

#[test]
fn ensemble_reports_which_region_tripped_the_component_first() {
    // The port-sector flux is 3x the others: it reaches the limit first in
    // essentially every sample at 10% sampling error.
    let (a, rates) = regional_case([1.0e10, 1.0e10, 3.0e10], 0.1);
    let ensemble = run_history_ensemble(
        &rates,
        &a,
        &settings(60, 2),
        &Cancellation::default(),
        &|_, _| {},
    )
    .unwrap();
    assert_eq!(ensemble.status, EnsembleStatus::Evaluated);
    for sample in std::iter::once(ensemble.nominal.as_ref().unwrap()).chain(&ensemble.samples) {
        assert_eq!(sample.first_trigger_response["magnet"], "r-port");
        assert_eq!(
            sample.rates.region_flux_n_m2_s.len(),
            3,
            "sampled region fluxes are reported per sample"
        );
    }
    let nominal = ensemble.nominal.as_ref().unwrap();
    assert_eq!(nominal.rates.region_flux_n_m2_s["r-port"], 3.0e10);
    let summary = ensemble.summary.as_ref().unwrap();
    let first = summary
        .discrete
        .iter()
        .find(|d| d.name == "first_trigger:magnet")
        .unwrap();
    assert_eq!(first.n, 60);
    assert_eq!(first.categories["r-port"].count, 60);
    assert_eq!(first.categories["none"].count, 0);
    assert!(!first.categories.contains_key("r-inboard"));
    // The legacy component with no region limit is not listed.
    assert!(
        summary
            .discrete
            .iter()
            .all(|d| d.name != "first_trigger:blanket")
    );
}

#[test]
fn near_tied_regions_split_the_first_trigger_fractions() {
    // Equal means with independent 20% errors: either of two regions can be
    // first, so both are named, and the fractions are over all samples.
    let (a, rates) = regional_case([3.0e10, 3.0e10, 5.0e9], 0.2);
    let ensemble = run_history_ensemble(
        &rates,
        &a,
        &settings(120, 2),
        &Cancellation::default(),
        &|_, _| {},
    )
    .unwrap();
    let summary = ensemble.summary.as_ref().unwrap();
    let first = summary
        .discrete
        .iter()
        .find(|d| d.name == "first_trigger:magnet")
        .unwrap();
    let c = |k: &str| first.categories.get(k).map_or(0, |p| p.count);
    assert!(c("r-inboard") > 20 && c("r-outboard") > 20, "{first:?}");
    assert_eq!(
        c("r-inboard") + c("r-outboard") + c("r-port") + c("none"),
        120
    );
    assert_eq!(c("r-port"), 0);
}

#[test]
fn from_normalized_binds_whole_and_regional_fast_flux_into_the_covariance() {
    let source_rate = 525.0e6 / (17.6e6 * ELEMENTARY_CHARGE_J_PER_EV);
    let volume: f64 = 4.0;
    let tally = |id: &str, domain, score, unit, iunit, imean: f64, ise: f64| NormalizedTally {
        response_id: id.into(),
        domain,
        score,
        estimator: TallyEstimator::Tracklength,
        mean: imean / volume,
        standard_error: ise / volume,
        unit,
        integrated_mean: imean,
        integrated_standard_error: ise,
        integrated_unit: iunit,
        volume_m3: volume,
        volume_standard_error_m3: 0.0,
    };
    let fast = ScoreDefinition::FluxAbove {
        energy_min_ev: 1.0e5,
    };
    let flux_units = (
        PhysicalUnit::NeutronsPerSquareMetreSecond,
        PhysicalUnit::NeutronMetresPerSecond,
    );
    let region = ResponseDomain::ComponentRegion {
        component_id: "magnets".into(),
        region: faris_model::transport::ToroidalRegion::PortSector {
            half_width_rad: 0.1745,
        },
    };
    let results = vec![
        tally(
            "blanket-tritium",
            ResponseDomain::Component {
                component_id: "blanket".into(),
            },
            ScoreDefinition::ParticleProduction {
                particle: ProducedParticle::Tritium,
                score: "H3-production".into(),
            },
            PhysicalUnit::ParticlesPerCubicMetreSecond,
            PhysicalUnit::ParticlesPerSecond,
            1.1 * source_rate,
            0.01 * source_rate,
        ),
        tally(
            "magnets-fast-flux",
            ResponseDomain::Component {
                component_id: "magnets".into(),
            },
            fast.clone(),
            flux_units.0,
            flux_units.1,
            8.0e13,
            4.0e12,
        ),
        tally(
            "magnets-port-sector-fast-flux",
            region.clone(),
            fast,
            flux_units.0,
            flux_units.1,
            2.0e14,
            1.0e13,
        ),
    ];
    let sds = [0.01 * source_rate, 4.0e12, 1.0e13];
    let mut integrated = vec![0.0; 9];
    for i in 0..3 {
        for j in 0..3 {
            integrated[i * 3 + j] = if i == j { 1.0 } else { 0.5 } * sds[i] * sds[j];
        }
    }
    let normalized = NormalizedTransportResult {
        schema_version: "faris-normalized-transport/v0.1".into(),
        scenario_id: "s".into(),
        scenario_sha256: "a".repeat(64),
        variant_id: "v".into(),
        source: DtSource {
            energy_per_reaction_ev: 17.6e6,
            neutron_energy_ev: 14.1e6,
            neutrons_per_reaction: 1.0,
            distribution_id: "d".into(),
        },
        solver: ToolIdentity {
            name: "OpenMC".into(),
            version: "0.15.3".into(),
            digest: format!("sha256:{}", "1".repeat(64)),
        },
        nuclear_data: ToolIdentity {
            name: "data".into(),
            version: "1".into(),
            digest: format!("sha256:{}", "2".repeat(64)),
        },
        histories: 1000,
        source_reaction_rate_per_s: source_rate,
        source_neutron_rate_per_s: source_rate,
        results,
        response_covariance: Some(ResponseCovariance {
            method: "batch-means-sample-covariance/v1".into(),
            batches: 100,
            response_ids: vec![
                "blanket-tritium".into(),
                "magnets-fast-flux".into(),
                "magnets-port-sector-fast-flux".into(),
            ],
            integrated,
        }),
    };
    let rates =
        TransportDrivingRates::from_normalized(&normalized, 525.0, &"b".repeat(64)).unwrap();
    // Fast-flux responses are region rates, never component-average fluxes.
    assert!(rates.component_average_flux_n_m2_s.is_empty());
    assert_eq!(rates.region_flux_n_m2_s.len(), 2);
    let port = &rates.region_flux_n_m2_s["magnets-port-sector-fast-flux"];
    assert_eq!(port.component_id, "magnets");
    assert_eq!(port.energy_min_ev, 1.0e5);
    assert!(port.region.is_some());
    assert!((port.rate.mean - 5.0e13).abs() < 1.0);
    assert!(
        rates.region_flux_n_m2_s["magnets-fast-flux"]
            .region
            .is_none()
    );
    let cov = rates.covariance.as_ref().unwrap();
    assert_eq!(
        cov.rate_ids,
        [
            "blanket-tritium",
            "magnets-fast-flux",
            "magnets-port-sector-fast-flux"
        ]
    );
    let corr = cov.monte_carlo[5] / (cov.monte_carlo[4] * cov.monte_carlo[8]).sqrt();
    assert!((corr - 0.5).abs() < 1e-9);
    // JSON round trip, and an old record without the field still parses.
    let back: TransportDrivingRates =
        serde_json::from_value(serde_json::to_value(&rates).unwrap()).unwrap();
    assert_eq!(back, rates);
    let mut old = serde_json::to_value(&rates).unwrap();
    old.as_object_mut().unwrap().remove("region_flux_n_m2_s");
    old.as_object_mut().unwrap().remove("covariance");
    let parsed: TransportDrivingRates = serde_json::from_value(old).unwrap();
    assert!(parsed.region_flux_n_m2_s.is_empty());
}
