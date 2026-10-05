use super::*;
use crate::fixtures::{assumptions, ensemble, rates_with_covariance, rates_without_covariance};
use crate::history::{HistoryOutcome, run_operating_history};
use crate::history_ensemble::{
    ENSEMBLE_METHOD_ID, EnsembleSettings, compare_ensembles, nominal_sample, run_history_ensemble,
};
use crate::jobs::Cancellation;
use std::collections::BTreeMap;

fn key(samples: u32) -> EnsembleKey {
    EnsembleKey::new(
        &rates_with_covariance(0.06, 0.4, 'a'),
        &assumptions(),
        samples,
    )
    .unwrap()
}

// Verifies: AUTO-030
#[test]
fn the_same_inputs_give_the_same_key_and_hit_the_cache() {
    let a = key(10);
    assert_eq!(a, key(10));
    let mut cache = EnsembleCache::default();
    assert!(cache.get(&a).is_none());
    cache.insert(a.clone(), Arc::new(ensemble(4, a.seed, 'a')));
    assert!(cache.contains(&key(10)));
    assert!(cache.get(&key(10)).is_some());
    assert_eq!(cache.len(), 1);
}

// Verifies: AUTO-033
#[test]
fn changing_any_component_of_the_key_misses() {
    let base = key(10);
    let mut cache = EnsembleCache::default();
    cache.insert(base.clone(), Arc::new(ensemble(4, base.seed, 'a')));

    // Samples.
    assert!(!cache.contains(&key(11)));
    // A rate.
    let mut rates = rates_with_covariance(0.06, 0.4, 'a');
    rates
        .component_average_flux_n_m2_s
        .get_mut("magnets")
        .unwrap()
        .mean *= 1.001;
    assert!(!cache.contains(&EnsembleKey::new(&rates, &assumptions(), 10).unwrap()));
    // One covariance entry: the key covers the covariance, not just the means.
    let mut rates = rates_with_covariance(0.06, 0.4, 'a');
    rates.covariance.as_mut().unwrap().monte_carlo[1] *= 0.999;
    assert!(!cache.contains(&EnsembleKey::new(&rates, &assumptions(), 10).unwrap()));
    let mut rates = rates_with_covariance(0.06, 0.4, 'a');
    rates.covariance.as_mut().unwrap().volume_variance[2] = 1.0;
    assert!(!cache.contains(&EnsembleKey::new(&rates, &assumptions(), 10).unwrap()));
    // An assumption.
    let mut edited = assumptions();
    edited.service_limits[0].limit *= 1.01;
    let k = EnsembleKey::new(&rates_with_covariance(0.06, 0.4, 'a'), &edited, 10).unwrap();
    assert!(!cache.contains(&k));
    // The transport artifact changes the derived seed as well as the rates.
    let k = EnsembleKey::new(&rates_with_covariance(0.06, 0.4, 'b'), &assumptions(), 10).unwrap();
    assert_ne!(k.seed, base.seed);
    assert!(!cache.contains(&k));
    // Method and ledger identity, and the seed on their own.
    for edit in [
        (|k: &mut EnsembleKey| k.method.push('x')) as fn(&mut EnsembleKey),
        |k| k.history_model.push('x'),
        |k| k.seed ^= 1,
        |k| k.rates_sha256 = "0".repeat(64),
        |k| k.assumptions_sha256 = "0".repeat(64),
    ] {
        let mut k = base.clone();
        edit(&mut k);
        assert!(!cache.contains(&k));
    }
}

#[test]
fn the_key_round_trips_with_its_seed_as_text() {
    let k = EnsembleKey {
        seed: u64::MAX,
        ..key(10)
    };
    let json = serde_json::to_string(&k).unwrap();
    assert!(json.contains(&format!("\"seed\":\"{}\"", u64::MAX)));
    assert_eq!(serde_json::from_str::<EnsembleKey>(&json).unwrap(), k);
    let bad = json.replace(&u64::MAX.to_string(), "-3");
    assert!(serde_json::from_str::<EnsembleKey>(&bad).is_err());
}

#[test]
fn the_key_describes_its_own_ensemble_only() {
    let k = key(5);
    let e = run_history_ensemble(
        &rates_with_covariance(0.06, 0.4, 'a'),
        &assumptions(),
        &EnsembleSettings {
            samples: 5,
            seed: None,
            threads: Some(1),
        },
        &Cancellation::default(),
        &|_, _| {},
    )
    .unwrap();
    assert_eq!(e.method, ENSEMBLE_METHOD_ID);
    assert!(k.describes(&e));
    assert!(!key(6).describes(&e));
}

#[test]
fn the_cache_drops_its_oldest_entry_when_full() {
    let mut cache = EnsembleCache::default();
    let e = Arc::new(ensemble(2, 1, 'a'));
    for samples in 1..=30 {
        cache.insert(key(samples), e.clone());
    }
    assert_eq!(cache.len(), 24);
    assert!(!cache.contains(&key(1)));
    assert!(cache.contains(&key(30)));
}

#[test]
fn percent_text_never_rounds_rare_or_near_certain_to_nothing_or_one() {
    assert_eq!(percent_text(0.0), "0 %");
    assert_eq!(percent_text(0.004), "<1 %");
    assert_eq!(percent_text(0.62), "62 %");
    assert_eq!(percent_text(0.316), "32 %");
    assert_eq!(percent_text(0.996), ">99 %");
    assert_eq!(percent_text(1.0), "100 %");
}

#[test]
fn ranges_use_units_and_switch_to_to_when_negative() {
    assert_eq!(
        range_text(
            YEARS,
            16.8 * JULIAN_YEAR_SECONDS,
            18.01 * JULIAN_YEAR_SECONDS
        ),
        "16.80–18.01 y"
    );
    assert_eq!(
        range_text(TERAWATT_HOURS, -2.0e5, 8.0e5),
        "-0.200 to 0.800 TWh"
    );
    assert_eq!(KILOGRAMS.with_symbol(0.5), "0.500 kg");
}

fn proportion(count: u32, n: u32) -> Proportion {
    let (lo, hi) = crate::history_ensemble::wilson_interval(count, n);
    Proportion {
        count,
        fraction: f64::from(count) / f64::from(n),
        wilson95_low: lo,
        wilson95_high: hi,
    }
}

fn discrete(name: &str, counts: &[(&str, u32)]) -> DiscreteSummary {
    let n: u32 = counts.iter().map(|c| c.1).sum();
    DiscreteSummary {
        name: name.into(),
        n,
        categories: counts
            .iter()
            .map(|(k, c)| (k.to_string(), proportion(*c, n)))
            .collect::<BTreeMap<_, _>>(),
    }
}

#[test]
fn the_distribution_sentence_lists_the_most_frequent_first_and_skips_zeros() {
    let d = discrete(
        "replacements:magnets",
        &[("3", 7), ("4", 62), ("5", 31), ("6", 0)],
    );
    assert_eq!(
        count_distribution_sentence("Magnet swaps", &d),
        "Magnet swaps: 4 in 62 % of samples, 5 in 31 %, 3 in 7 %"
    );
    let detail = count_distribution_detail(&d);
    assert!(
        detail.contains("4: 62 % of samples (62/100; 95 % interval 52–71 %)"),
        "{detail}"
    );
    assert!(!detail.contains("6:"));
    let one = discrete("replacements:magnets", &[("4", 200)]);
    assert_eq!(
        count_distribution_sentence("Magnet swaps", &one),
        "Magnet swaps: 4 in 100 % of samples"
    );
    let status = discrete(
        "terminal_status",
        &[
            ("horizon_completed", 98),
            ("fuel_limited_at_horizon", 0),
            ("permanent_component_limit", 2),
        ],
    );
    assert_eq!(
        status_distribution_sentence("Outcome", &status),
        "Outcome: runs to the end of the horizon in 98 % of samples, stops at a permanent component limit in 2 %"
    );
}

#[test]
fn rows_pair_each_nominal_value_with_its_range_or_distribution() {
    let rates = rates_with_covariance(0.06, 0.4, 'a');
    let e = run_history_ensemble(
        &rates,
        &assumptions(),
        &EnsembleSettings {
            samples: 12,
            seed: Some(3),
            threads: Some(1),
        },
        &Cancellation::default(),
        &|_, _| {},
    )
    .unwrap();
    let nominal = e.nominal.clone().unwrap();
    let rows = uncertainty_rows(&nominal, Some(&e));
    let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"full_power_time_s"));
    assert!(names.contains(&"final_available_tritium_kg"));
    assert!(names.contains(&"cumulative_net_electricity_mwh"));
    assert!(names.contains(&"first_replacement_time_s:magnets"));
    assert!(names.contains(&"replacements:magnets"));
    assert_eq!(names.last(), Some(&"terminal_status"));
    for row in &rows {
        let result = row
            .result
            .as_ref()
            .unwrap_or_else(|| panic!("{}", row.name));
        assert!(!row.nominal.is_empty() && !result.text.is_empty() && !result.detail.is_empty());
    }
    let full_power = &rows[0];
    assert!(full_power.nominal.ends_with(" y"));
    let text = &full_power.result.as_ref().unwrap().text;
    assert!(
        text.starts_with("median ") && text.contains(", P5–P95 ") && text.ends_with(" y"),
        "{text}"
    );
    let swaps = rows
        .iter()
        .find(|r| r.name == "replacements:magnets")
        .unwrap();
    assert!(
        swaps
            .result
            .as_ref()
            .unwrap()
            .text
            .starts_with("Magnet swaps: ")
    );
    // The nominal value in a row is the history's own, from the same function
    // the app uses when no ensemble exists.
    let history = run_operating_history(&assumptions(), &rates).unwrap();
    assert_eq!(nominal_sample(&history).unwrap(), nominal);
}

#[test]
fn rows_without_an_evaluated_ensemble_keep_the_nominal_value_and_no_result() {
    let rates = rates_without_covariance(0.06, 'a');
    let history = run_operating_history(&assumptions(), &rates).unwrap();
    let nominal = nominal_sample(&history).unwrap();
    let not_evaluated = run_history_ensemble(
        &rates,
        &assumptions(),
        &EnsembleSettings::default(),
        &Cancellation::default(),
        &|_, _| {},
    )
    .unwrap();
    let (why, next) = not_evaluated_text(&not_evaluated).unwrap();
    assert!(why.contains("no covariance") && next.starts_with("Rerun transport"));
    for ensemble in [Some(&not_evaluated), None] {
        let rows = uncertainty_rows(&nominal, ensemble);
        assert!(rows.len() >= 6);
        assert!(
            rows.iter()
                .all(|r| r.result.is_none() && !r.nominal.is_empty())
        );
    }
    assert_eq!(
        uncertainty_rows(&nominal, None)
            .last()
            .map(|r| r.nominal.as_str()),
        Some(match nominal.outcome {
            HistoryOutcome::HorizonCompleted => "runs to the end of the horizon",
            HistoryOutcome::FuelLimitedAtHorizon => "ends the horizon short of fuel",
            HistoryOutcome::PermanentComponentLimit => "stops at a permanent component limit",
        })
    );
}

#[test]
fn a_first_replacement_that_not_every_sample_reaches_says_so() {
    let e = ensemble(10, 5, 'a');
    let mut e2 = e.clone();
    let summary = e2.summary.as_mut().unwrap();
    let s = summary
        .continuous
        .iter_mut()
        .find(|c| c.name == "first_replacement_time_s:magnets")
        .unwrap();
    s.n = 7;
    let rows = uncertainty_rows(e.nominal.as_ref().unwrap(), Some(&e2));
    let row = rows
        .iter()
        .find(|r| r.name == "first_replacement_time_s:magnets")
        .unwrap();
    let result = row.result.as_ref().unwrap();
    assert!(
        result.text.ends_with("(7 of 10 samples)"),
        "{}",
        result.text
    );
    assert!(result.detail.contains("7 of 10 samples have this value"));
}

#[test]
fn band_coverage_is_noted_only_where_samples_are_missing() {
    assert_eq!(band_coverage_note(10, 10), None);
    assert!(
        band_coverage_note(7, 10)
            .unwrap()
            .contains("7 of 10 samples")
    );
    assert_eq!(progress_text(120, 200), "Uncertainty: 120 of 200 samples");
}

#[test]
fn paired_lines_name_the_arrangements_and_report_swap_shares() {
    let a = ensemble(20, 11, 'a');
    let b = ensemble(20, 12, 'b');
    let comparison = compare_ensembles(&a, &b).unwrap();
    let lines = comparison_lines("Reference", "Breeder-heavy", &comparison);
    let swaps = lines
        .iter()
        .find(|l| l.name == "replacements:magnets")
        .unwrap();
    let sentence = swaps.sentence.as_ref().unwrap();
    assert!(
        sentence.starts_with("Reference has fewer magnet swaps than Breeder-heavy in "),
        "{sentence}"
    );
    assert!(
        sentence.contains("of paired samples (95 % interval ")
            && sentence.contains("the same number in ")
    );
    assert!(swaps.detail.contains("20 paired samples."));
    let energy = lines
        .iter()
        .find(|l| l.name == "cumulative_net_electricity_mwh")
        .unwrap();
    assert!(energy.difference.starts_with("median ") && energy.difference.contains("TWh"));
    assert!(energy.sentence.is_none());
}

#[test]
fn trigger_regions_read_as_words_and_the_sentence_names_the_first_region() {
    assert_eq!(
        trigger_region_text("magnets", "magnets-port-sector-fast-flux"),
        "port sector"
    );
    assert_eq!(
        trigger_region_text("magnets", "magnets-inboard-fast-flux"),
        "inboard"
    );
    assert_eq!(trigger_region_text("magnets", "none"), "no swap");
    // An id that does not follow the naming stays as written.
    assert_eq!(trigger_region_text("magnets", "r-port"), "r port");
    let d = discrete(
        "first_trigger:magnets",
        &[
            ("magnets-port-sector-fast-flux", 97),
            ("magnets-inboard-fast-flux", 3),
            ("none", 0),
        ],
    );
    assert_eq!(
        trigger_distribution_sentence("magnets", "Magnet swap triggered first by", &d),
        "Magnet swap triggered first by: port sector in 97 % of samples, inboard in 3 %"
    );
    let detail = trigger_distribution_detail("magnets", &d);
    assert!(
        detail.contains("port sector: 97 % of samples (97/100;"),
        "{detail}"
    );
    assert!(!detail.contains("no swap"));
}

#[test]
fn a_nominal_trigger_gets_a_row_even_without_an_ensemble() {
    let rates = rates_without_covariance(0.06, 'a');
    let history = run_operating_history(&assumptions(), &rates).unwrap();
    let mut nominal = nominal_sample(&history).unwrap();
    nominal
        .first_trigger_response
        .insert("magnets".into(), "magnets-port-sector-fast-flux".into());
    let rows = trigger_rows(&nominal, None, &["magnets", "magnets"]);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.name, "first_trigger:magnets");
    assert_eq!(row.label, "Magnet swap triggered first by");
    assert_eq!(row.nominal, "port sector");
    assert!(row.result.is_none());
    // A component with a single limit has no region to name.
    assert!(trigger_rows(&nominal, None, &[]).is_empty());
    // On a history with a single limit the outcome stays the last row.
    let rows = history_rows(&history, None);
    assert!(rows.iter().all(|r| !r.name.starts_with("first_trigger:")));
    assert_eq!(rows.last().unwrap().name, "terminal_status");
}
