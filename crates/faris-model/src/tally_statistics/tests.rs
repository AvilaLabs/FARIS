use super::*;

/// Small deterministic generator (xorshift64*); tests need reproducible synthetic tallies, not quality.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        for _ in 0..32 {
            rng.uniform();
        }
        rng
    }
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let x = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
        ((x >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    fn normal(&mut self) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * math::ln(u)).sqrt() * math::cos(2.0 * std::f64::consts::PI * v)
    }
    fn poisson(&mut self, lambda: f64) -> u64 {
        let limit = math::exp(-lambda);
        let (mut k, mut p) = (0u64, 1.0);
        loop {
            p *= self.uniform();
            if p <= limit {
                return k;
            }
            k += 1;
        }
    }
}

fn normal_batches(rng: &mut Rng, n: usize, mean: f64, sd: f64) -> Vec<f64> {
    (0..n).map(|_| mean + sd * rng.normal()).collect()
}

fn state(health: &TallyHealth, id: u8) -> CheckState {
    health.checks.iter().find(|c| c.id == id).unwrap().state
}

#[test]
fn special_functions_match_published_values() {
    // Student t, two-sided 95 %.
    assert!((student_t_quantile(0.975, 1.0) - 12.7062).abs() < 1e-3);
    assert!((student_t_quantile(0.975, 10.0) - 2.2281).abs() < 1e-3);
    assert!((student_t_quantile(0.975, 29.0) - 2.0452).abs() < 1e-3);
    assert!((normal_quantile(0.975) - 1.959964).abs() < 1e-5);
    // Garwood: 10 events, 95 % two-sided; 0 events, one- and two-sided.
    let (lo, hi) = garwood_poisson(10, 0.95);
    assert!(
        (lo - 4.7954).abs() < 1e-3 && (hi - 18.3904).abs() < 1e-3,
        "{lo} {hi}"
    );
    assert!((poisson_upper_bound(0, 0.95) - 2.9957).abs() < 1e-3);
    assert!((garwood_poisson(0, 0.95).1 - 3.6889).abs() < 1e-3);
    // Wilson: 0 of 10 has the upper end z^2 / (n + z^2).
    let (w_lo, w_hi) = wilson_interval(0, 10, 0.95).unwrap();
    let z2 = 1.959964 * 1.959964;
    assert!(w_lo.abs() < 1e-12);
    assert!((w_hi - z2 / (10.0 + z2)).abs() < 1e-4);
    assert!(wilson_interval(11, 10, 0.95).is_none());
}

// Verifies: NUC-041
#[test]
fn variance_of_variance_has_the_known_value_and_flags_a_heavy_tail() {
    // Hand calculation: x = 0,0,0,0,1; mean 0.2; sum d^2 = 0.8; sum d^4 = 0.416; 0.416 / 0.64 - 0.2 = 0.45.
    let v = variance_of_variance(&[0.0, 0.0, 0.0, 0.0, 1.0]).unwrap();
    assert!((v - 0.45).abs() < 1e-12, "{v}");
    assert!(variance_of_variance(&[1.0, 1.0, 1.0]).is_none());
    assert!(variance_of_variance(&[1.0, 2.0]).is_none());
    // Normal batches: about 2/n, below the limit. Heavy-tailed (lognormal sigma 3): above it.
    let mut rng = Rng::new(7);
    let normal = normal_batches(&mut rng, 400, 1.0, 0.2);
    let v_normal = variance_of_variance(&normal).unwrap();
    assert!(v_normal < 0.03, "{v_normal}");
    let heavy: Vec<f64> = (0..400).map(|_| math::exp(3.0 * rng.normal())).collect();
    let v_heavy = variance_of_variance(&heavy).unwrap();
    assert!(v_heavy > VOV_LIMIT, "{v_heavy}");
    let health = assess_tally(&heavy, None, TallyKind::Volume);
    assert_eq!(state(&health, 5), CheckState::Fail);
    assert_eq!(health.variance_of_variance, Some(v_heavy));
    assert_eq!(health.status, TallyStatus::Failing);
    // Every tally report carries the value.
    assert!(
        assess_tally(&normal, None, TallyKind::Volume)
            .variance_of_variance
            .is_some()
    );
}

// Verifies: NUC-042
#[test]
fn figure_of_merit_has_the_known_value_and_is_stable_for_a_stable_run() {
    assert!((figure_of_merit(0.1, 100.0).unwrap() - 1.0).abs() < 1e-12);
    assert!(figure_of_merit(0.0, 1.0).is_none());
    assert!(figure_of_merit(0.1, 0.0).is_none());
    let mut rng = Rng::new(11);
    let stable = normal_batches(&mut rng, 400, 1.0, 0.3);
    let health = assess_tally(&stable, None, TallyKind::Volume);
    let fom = health.fom.as_ref().expect("figure of merit computed");
    assert_eq!(fom.basis, CostBasis::Batches);
    assert!(fom.trend.len() >= 5);
    assert!(
        fom.max_change_last_half < 0.6,
        "{}",
        fom.max_change_last_half
    );
    assert_eq!(state(&health, 8), CheckState::Pass);
    assert_eq!(state(&health, 9), CheckState::Pass);
}

// Verifies: UNC-006
#[test]
fn a_drifting_figure_of_merit_is_flagged_with_recorded_times() {
    let mut rng = Rng::new(12);
    let values = normal_batches(&mut rng, 100, 1.0, 0.3);
    // Each batch takes longer than the one before: the cost grows as k^2, the FOM falls as 1/k^2.
    let seconds: Vec<f64> = (1..=100).map(|k| (k * k) as f64).collect();
    let health = assess_tally(&values, Some(&seconds), TallyKind::Volume);
    let fom = health.fom.as_ref().unwrap();
    assert_eq!(fom.basis, CostBasis::Seconds);
    assert!(fom.max_change_last_half > 1.0);
    assert_eq!(state(&health, 8), CheckState::Fail);
    assert_eq!(state(&health, 9), CheckState::Fail);
    assert_eq!(health.status, TallyStatus::Failing);
    // The same values at a steady cost per batch are not flagged.
    let steady: Vec<f64> = (1..=100).map(|k| 2.5 * k as f64).collect();
    let health = assess_tally(&values, Some(&steady), TallyKind::Volume);
    assert_eq!(state(&health, 8), CheckState::Pass);
    // Bad time series are refused with a reason, not used.
    let health = assess_tally(&values, Some(&seconds[..50]), TallyKind::Volume);
    assert_eq!(health.status, TallyStatus::NotEvaluated);
    assert!(health.summary.contains("batch times"));
}

#[test]
fn a_good_tally_passes_with_one_gap_and_gets_a_verdict() {
    let mut rng = Rng::new(21);
    let values = normal_batches(&mut rng, 400, 1.0, 0.5);
    let health = assess_tally(&values, None, TallyKind::Volume);
    assert_eq!(
        health.status,
        TallyStatus::PassingWithGaps,
        "{:?}",
        health.checks
    );
    assert!(health.status.allows_verdict());
    assert_eq!(state(&health, 10), CheckState::NotEvaluated);
    let gap = health.checks.iter().find(|c| c.id == 10).unwrap();
    assert!(gap.detail.contains("per-history"));
    assert_eq!(health.checks.len(), 10);
}

// Verifies: UNC-005
#[allow(clippy::vec_init_then_push)] // fixtures are built one commented case at a time
#[test]
fn each_statistical_check_fails_on_its_own_bad_tally_and_a_failing_tally_is_refused() {
    let mut rng = Rng::new(31);
    let mut fixtures: Vec<(u8, Vec<f64>, Option<Vec<f64>>)> = Vec::new();
    // 1: the mean climbs steadily.
    fixtures.push((
        1,
        (0..100)
            .map(|k| 1.0 + 0.02 * k as f64 + 0.05 * rng.normal())
            .collect(),
        None,
    ));
    // 2: noise swamps the mean.
    fixtures.push((2, normal_batches(&mut rng, 100, 1.0, 6.0), None));
    // 3: the spread keeps growing, so the relative error rises over the last half.
    fixtures.push((
        3,
        (0..200)
            .map(|k| 1.0 + 0.002 * (k * k) as f64 * 0.01 * rng.normal())
            .collect(),
        None,
    ));
    // 4: the spread grows as sqrt(k), so the relative error stays flat instead of falling.
    fixtures.push((
        4,
        (0..300)
            .map(|k| 1.0 + 0.05 * ((k + 1) as f64).sqrt() * rng.normal())
            .collect(),
        None,
    ));
    // 5: one batch holds much of the sum.
    let mut spike = normal_batches(&mut rng, 100, 1.0, 0.2);
    spike[10] = 40.0;
    fixtures.push((5, spike, None));
    // 6: a heavy batch arrives late in the run.
    let mut late = normal_batches(&mut rng, 100, 1.0, 0.2);
    late[80] = 40.0;
    fixtures.push((6, late, None));
    // 7: a heavy batch early, and nothing after it averages it out (1000 batches).
    let mut early = normal_batches(&mut rng, 1000, 1.0, 0.2);
    early[100] = 400.0;
    fixtures.push((7, early, None));
    // 8 and 9: the cost per batch grows, so the figure of merit falls.
    let steady = normal_batches(&mut rng, 100, 1.0, 0.3);
    let grow: Vec<f64> = (1..=100).map(|k| (k * k) as f64).collect();
    fixtures.push((8, steady.clone(), Some(grow.clone())));
    fixtures.push((9, steady, Some(grow)));
    for (id, values, seconds) in &fixtures {
        let health = assess_tally(values, seconds.as_deref(), TallyKind::Volume);
        assert_eq!(
            state(&health, *id),
            CheckState::Fail,
            "check {id} should fail: {:#?}",
            health.checks
        );
        assert_eq!(health.status, TallyStatus::Failing, "check {id}");
        assert!(!health.status.allows_verdict());
        assert!(health.summary.contains("no verdict may rest on this tally"));
    }
    // 10 cannot be evaluated from batch values and says what it needs.
    let health = assess_tally(
        &normal_batches(&mut rng, 100, 1.0, 0.1),
        None,
        TallyKind::Volume,
    );
    assert_eq!(state(&health, 10), CheckState::NotEvaluated);
    // A point detector has the tighter relative error guideline.
    let values = normal_batches(&mut rng, 100, 1.0, 0.7);
    let volume = assess_tally(&values, None, TallyKind::Volume);
    let point = assess_tally(&values, None, TallyKind::PointDetector);
    assert_eq!(state(&volume, 2), CheckState::Pass);
    assert_eq!(state(&point, 2), CheckState::Fail);
    // Too few batches and non-finite values are NOT_EVALUATED with a reason.
    let few = assess_tally(&[1.0, 2.0, 3.0], None, TallyKind::Volume);
    assert_eq!(few.status, TallyStatus::NotEvaluated);
    assert!(
        few.checks
            .iter()
            .all(|c| c.state == CheckState::NotEvaluated)
    );
    assert!(few.summary.contains("run with more batches"));
    let mut nan = normal_batches(&mut rng, 50, 1.0, 0.1);
    nan[3] = f64::NAN;
    assert_eq!(
        assess_tally(&nan, None, TallyKind::Volume).status,
        TallyStatus::NotEvaluated
    );
    // A tally that never scored fails check 2 and says so.
    let zero = assess_tally(&[0.0; 50], None, TallyKind::Volume);
    assert_eq!(state(&zero, 2), CheckState::Fail);
    assert_eq!(zero.status, TallyStatus::Failing);
}

#[test]
fn good_normal_tallies_are_refused_rarely() {
    // Measured false-refusal rate on synthetic normal batches. The variance of the variance below 0.1
    // is the demanding check at few batches (its expectation is 2/n).
    let trials = 400;
    for (n, allowed) in [(30usize, 0.25), (100, 0.08), (300, 0.04)] {
        let refused = (0..trials)
            .filter(|seed| {
                let mut rng = Rng::new(*seed as u64 + 1000);
                let values = normal_batches(&mut rng, n, 1.0, 0.3);
                assess_tally(&values, None, TallyKind::Volume).status == TallyStatus::Failing
            })
            .count();
        let rate = refused as f64 / trials as f64;
        eprintln!("good normal tallies, {n} batches: refused {refused} of {trials}");
        assert!(rate <= allowed, "{n} batches refused at {rate}");
    }
}

// Verifies: NUC-047
#[test]
fn effective_scoring_histories_are_reported_and_low_samples_flagged() {
    assert!((effective_scoring_histories(1.0, 0.05).unwrap() - 400.0).abs() < 1e-9);
    assert!((effective_scoring_histories(-2.0, 0.1).unwrap() - 400.0).abs() < 1e-9);
    assert!(effective_scoring_histories(0.0, 0.1).is_none());
    assert!(effective_scoring_histories(1.0, 0.0).is_none());
    // A deep-penetration tally: a rare scoring event with weight 1 in 100 batches of 10 000 histories.
    let mut rng = Rng::new(41);
    let deep: Vec<f64> = (0..100)
        .map(|_| rng.poisson(1.5) as f64 / 10_000.0)
        .collect();
    let health = assess_tally(&deep, None, TallyKind::Volume);
    let n_eff = health.effective_histories.unwrap();
    assert!(n_eff < LOW_SAMPLE_HISTORIES, "{n_eff}");
    assert!(health.low_sample);
    // A well scored tally is not flagged.
    let shallow = normal_batches(&mut rng, 100, 1.0, 0.1);
    let health = assess_tally(&shallow, None, TallyKind::Volume);
    assert!(health.effective_histories.unwrap() > LOW_SAMPLE_HISTORIES);
    assert!(!health.low_sample);
    // A tally with no spread or no score has no effective count and is flagged.
    assert!(assess_tally(&[0.0; 50], None, TallyKind::Volume).low_sample);
}

// Verifies: UNC-003
#[test]
fn intervals_cover_poisson_counts_from_1_to_20_events_and_bound_below_10() {
    let mut rng = Rng::new(51);
    let (mut covered, mut one_sided, mut total) = (0usize, 0usize, 0usize);
    for i in 0..10_000 {
        // True mean count in 1..=20; the tally is the count times a weight of 0.5.
        let lambda = 1.0 + 19.0 * (i % 100) as f64 / 99.0;
        let k = rng.poisson(lambda);
        total += 1;
        let weight = 0.5;
        match counts_interval(k, weight, 0.95) {
            TallyInterval::TwoSided { lower, upper, .. } => {
                assert!(k >= 10);
                covered += (lower <= weight * lambda && weight * lambda <= upper) as usize;
            }
            TallyInterval::UpperBound { upper, .. } => {
                assert!(k < 10);
                one_sided += 1;
                covered += (weight * lambda <= upper) as usize;
            }
            TallyInterval::NotEvaluated { .. } => panic!("weight is valid"),
        }
    }
    let coverage = covered as f64 / total as f64;
    eprintln!("Poisson 1 to 20 events: coverage {coverage:.4}, one-sided {one_sided} of {total}");
    assert!(coverage >= 0.93, "{coverage}");
    assert!(one_sided > 1000);
    // A zero count still has an upper bound; a bad weight has nothing.
    assert!(matches!(
        counts_interval(0, 1.0, 0.95),
        TallyInterval::UpperBound { .. }
    ));
    assert!(matches!(
        counts_interval(5, 0.0, 0.95),
        TallyInterval::NotEvaluated { .. }
    ));
}

/// A tally with `batches` batches of unit-weight Poisson counts: (mean per batch, standard error,
/// true mean per batch).
fn poisson_tally(rng: &mut Rng, lambda_total: f64, batches: usize) -> (f64, f64, f64) {
    let per_batch = lambda_total / batches as f64;
    let values: Vec<f64> = (0..batches)
        .map(|_| rng.poisson(per_batch) as f64)
        .collect();
    let (mean, se) = mean_and_standard_error(&values).unwrap();
    (mean, se, per_batch)
}

// Verifies: NUC-044
#[test]
fn tally_intervals_cover_95_percent_over_10000_synthetic_tallies() {
    let mut rng = Rng::new(61);
    let mut covered = 0usize;
    let mut shown = 0usize;
    let mut few_histories = 0usize;
    let mut few_covered = 0usize;
    for i in 0..10_000 {
        // 5 to 2000 expected scoring events over 50 batches: down to a handful of events.
        let lambda_total = 5.0 * math::powf(400.0, (i % 100) as f64 / 99.0);
        let (mean, se, truth) = poisson_tally(&mut rng, lambda_total, 50);
        if se <= 0.0 {
            continue;
        }
        let hit = match tally_interval(mean, se, 50, 0.95) {
            TallyInterval::TwoSided { lower, upper, .. } => lower <= truth && truth <= upper,
            TallyInterval::UpperBound { upper, .. } => truth <= upper,
            TallyInterval::NotEvaluated { .. } => continue,
        };
        shown += 1;
        covered += hit as usize;
        if lambda_total < 1000.0 {
            few_histories += 1;
            few_covered += hit as usize;
        }
    }
    let coverage = covered as f64 / shown as f64;
    let few = few_covered as f64 / few_histories as f64;
    eprintln!(
        "coverage over {shown} tallies: {coverage:.4}; under 1000 scoring histories ({few_histories}): {few:.4}"
    );
    assert!(coverage >= 0.93, "{coverage}");
    assert!(few >= 0.93, "{few}");
}

#[test]
fn a_gaussian_interval_would_fail_where_the_tally_interval_holds() {
    // Same 10 000 style of tallies, 8 expected events: mean +- 1.96 se undercovers.
    let mut rng = Rng::new(62);
    let (mut gaussian, mut ours, mut shown) = (0usize, 0usize, 0usize);
    for _ in 0..4000 {
        let (mean, se, truth) = poisson_tally(&mut rng, 8.0, 50);
        if se <= 0.0 {
            continue;
        }
        shown += 1;
        gaussian += ((mean - truth).abs() <= 1.959964 * se) as usize;
        ours += match tally_interval(mean, se, 50, 0.95) {
            TallyInterval::TwoSided { lower, upper, .. } => lower <= truth && truth <= upper,
            TallyInterval::UpperBound { upper, .. } => truth <= upper,
            TallyInterval::NotEvaluated { .. } => false,
        } as usize;
    }
    let (g, o) = (gaussian as f64 / shown as f64, ours as f64 / shown as f64);
    eprintln!("8 expected events: Gaussian {g:.3}, tally interval {o:.3}");
    assert!(g < 0.93);
    assert!(o >= 0.93);
}

#[test]
fn heavy_tailed_tallies_are_caught_by_the_checks_not_by_a_narrow_interval() {
    // Rare scores with lognormal weights (sigma 2): the batch-based interval undercovers, which is why a
    // tally that fails the checks gets no interval. Measure both and require the guard to work.
    let mut rng = Rng::new(63);
    let truth_scale = math::exp(2.0 * 2.0 / 2.0); // mean of lognormal(0, 2)
    let per_batch = 0.3; // expected scoring events per batch
    let (mut passing, mut passing_covered, mut all, mut all_covered) = (0, 0, 0, 0);
    for _ in 0..1000 {
        let values: Vec<f64> = (0..100)
            .map(|_| {
                (0..rng.poisson(per_batch))
                    .map(|_| math::exp(2.0 * rng.normal()))
                    .sum::<f64>()
            })
            .collect();
        let truth = per_batch * truth_scale;
        let health = assess_tally(&values, None, TallyKind::Volume);
        let hit = match tally_interval(health.mean, health.standard_error, 100, 0.95) {
            TallyInterval::TwoSided { lower, upper, .. } => lower <= truth && truth <= upper,
            TallyInterval::UpperBound { upper, .. } => truth <= upper,
            TallyInterval::NotEvaluated { .. } => continue,
        };
        all += 1;
        all_covered += hit as usize;
        if health.status.allows_verdict() {
            passing += 1;
            passing_covered += hit as usize;
        }
    }
    let all_cov = all_covered as f64 / all as f64;
    eprintln!(
        "heavy tail: coverage of all {all} tallies {all_cov:.3}; of the {passing} that passed the checks {:.3}",
        passing_covered as f64 / passing.max(1) as f64
    );
    // The checks refuse most of these tallies: a verdict is rare.
    assert!(passing < all / 2, "{passing} of {all} passed");
}

// Verifies: UNC-008
#[test]
fn reported_error_is_checked_against_the_batches_and_against_seeds() {
    let mut rng = Rng::new(71);
    let (mut agree, mut wrong_caught) = (0, 0);
    for _ in 0..1000 {
        let values = normal_batches(&mut rng, 100, 1.0, 0.3);
        let true_se = 0.3 / 10.0;
        agree += batch_error_agreement(true_se, &values).unwrap().agrees as usize;
        // A reported error that is half of the truth is caught.
        wrong_caught += (!batch_error_agreement(0.5 * true_se, &values)
            .unwrap()
            .agrees) as usize;
    }
    assert!(agree >= 985, "{agree}");
    assert!(wrong_caught >= 990, "{wrong_caught}");
    assert!(batch_error_agreement(0.0, &[1.0, 2.0, 3.0]).is_none());
    // Seeds: K = 40 independent runs of the same problem.
    let sigma = 0.03;
    let runs: Vec<(f64, f64)> = (0..40)
        .map(|_| (1.0 + sigma * rng.normal(), sigma))
        .collect();
    let ok = seed_spread_agreement(&runs).unwrap();
    assert!(ok.agrees, "z = {}", ok.z);
    let understated: Vec<(f64, f64)> = runs.iter().map(|r| (r.0, 0.4 * sigma)).collect();
    let bad = seed_spread_agreement(&understated).unwrap();
    assert!(!bad.agrees && bad.z > 3.0, "z = {}", bad.z);
    assert!(seed_spread_agreement(&runs[..2]).is_none());
}

// Verifies: UNC-013
#[test]
fn a_sum_of_parts_uses_the_covariance_of_batches_they_share() {
    let mut rng = Rng::new(81);
    let n = 80;
    // Two zones that share batches and are anti-correlated (a neutron that scores in one does not in the
    // other), plus a global tally that is exactly their sum in every batch.
    let a: Vec<f64> = (0..n).map(|_| 5.0 + rng.normal()).collect();
    let b: Vec<f64> = a
        .iter()
        .map(|x| 3.0 - 0.8 * (x - 5.0) + 0.3 * rng.normal())
        .collect();
    let total: Vec<f64> = a.iter().zip(&b).map(|(x, y)| x + y).collect();
    let cov = batch_mean_covariance(&[&a, &b, &total]).unwrap();
    let means: Vec<f64> = [&a, &b, &total]
        .iter()
        .map(|c| c.iter().sum::<f64>() / n as f64)
        .collect();
    // Sum of the two zones: the error from the covariance equals the error of the summed batch series.
    let (sum, se) = combine_linear(&[1.0, 1.0, 0.0], &means, &cov).unwrap();
    let (_, direct_se) = mean_and_standard_error(&total).unwrap();
    assert!((sum - means[2]).abs() < 1e-12);
    assert!((se - direct_se).abs() < 1e-12, "{se} vs {direct_se}");
    // Taking the zones as independent gives a different (here larger) error: the wrong answer.
    let independent = (cov[0] + cov[4]).sqrt();
    assert!((independent - direct_se).abs() > 0.1 * direct_se);
    // Sum of zones minus the global tally is zero with zero error, because the batches are the same.
    let (diff, diff_se) = combine_linear(&[1.0, 1.0, -1.0], &means, &cov).unwrap();
    assert!(diff.abs() < 1e-9 && diff_se < 1e-9);
    // Shape and validity errors say what is wrong.
    assert!(matches!(
        combine_linear(&[1.0], &[1.0, 2.0], &[1.0; 4]),
        Err(CombineError::Shape(..))
    ));
    assert!(matches!(
        combine_linear(&[1.0, 1.0], &[1.0, 2.0], &[1.0, -2.0, -2.0, 1.0]),
        Err(CombineError::NegativeVariance(_))
    ));
    assert!(matches!(
        combine_linear(&[1.0], &[f64::NAN], &[1.0]),
        Err(CombineError::NotFinite)
    ));
    assert!(batch_mean_covariance(&[&a[..5], &b[..4]]).is_none());
}

// Verifies: NUC-049
#[test]
fn mesh_convergence_fractions_agree_with_a_direct_count() {
    let mut rng = Rng::new(91);
    let means: Vec<f64> = (0..1000)
        .map(|i| {
            if i % 17 == 0 {
                0.0
            } else {
                1.0 + rng.uniform()
            }
        })
        .collect();
    let ses: Vec<f64> = means.iter().map(|m| m * 0.2 * rng.uniform()).collect();
    let c = mesh_convergence(&means, &ses).unwrap();
    let direct_10 = means
        .iter()
        .zip(&ses)
        .filter(|(m, s)| **m > 0.0 && **s > 0.0 && *s / *m <= 0.10)
        .count();
    let direct_5 = means
        .iter()
        .zip(&ses)
        .filter(|(m, s)| **m > 0.0 && **s > 0.0 && *s / *m <= 0.05)
        .count();
    assert_eq!(c.voxels, 1000);
    assert_eq!(c.within_10_percent, direct_10);
    assert_eq!(c.within_5_percent, direct_5);
    assert!((c.fraction_within_10_percent - direct_10 as f64 / 1000.0).abs() < 1e-15);
    assert_eq!(c.scored + c.unscored, 1000);
    assert_eq!(
        c.unscored,
        means.iter().filter(|m| **m == 0.0).count()
            + ses
                .iter()
                .zip(&means)
                .filter(|(s, m)| **m > 0.0 && **s <= 0.0)
                .count()
    );
    assert!(c.worst_relative_error.unwrap() <= 0.2);
    // Unscored voxels pull the fraction down; they never count as converged.
    let all_zero = mesh_convergence(&[0.0; 4], &[0.0; 4]).unwrap();
    assert_eq!(all_zero.fraction_within_10_percent, 0.0);
    assert_eq!(all_zero.unscored, 4);
    assert!(mesh_convergence(&[1.0], &[]).is_none());
    assert!(mesh_convergence(&[], &[]).is_none());
}
