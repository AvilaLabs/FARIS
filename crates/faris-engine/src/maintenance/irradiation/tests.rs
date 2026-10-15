use super::*;
use crate::fixtures;
use crate::history::{HistoryEvent, HistoryOutcome, run_operating_history_cancellable};
use crate::jobs::Cancellation;

fn base_history() -> HistoryResult {
    run_operating_history_cancellable(
        &fixtures::assumptions(),
        &fixtures::rates_without_covariance(0.0, 'a'),
        &Cancellation::default(),
    )
    .unwrap()
}

/// A history with exactly these source-on intervals `(start, end, power_fraction)`, a snapshot
/// at each start, and the given horizon; no replacements.
fn synthetic(pulses: &[(f64, f64, f64)], horizon_s: f64) -> HistoryResult {
    let mut h = base_history();
    let template = h.snapshots[0].clone();
    h.outcome = HistoryOutcome::HorizonCompleted;
    h.assumptions.horizon_s = horizon_s;
    h.assumptions.operation = vec![];
    h.events.clear();
    h.snapshots.clear();
    let event = |time_s: f64, order: u32, kind: EventKind| HistoryEvent {
        time_s,
        order,
        kind,
        component_id: None,
        response_id: None,
        mass_kg: None,
        note: "synthetic".into(),
        trip: None,
    };
    for (i, (a, b, p)) in pulses.iter().enumerate() {
        h.events
            .push(event(*a, 2 * i as u32, EventKind::OperationStarted));
        h.events
            .push(event(*b, 2 * i as u32 + 1, EventKind::OperationStopped));
        let mut s = template.clone();
        (s.time_s, s.operating, s.power_fraction) = (*a, true, *p);
        h.snapshots.push(s);
    }
    h
}

fn step(duration_s: f64, flux_scale: f64) -> IrradiationStep {
    IrradiationStep {
        duration_s,
        flux_scale,
    }
}

fn close(a: f64, b: f64, tol: f64, what: &str) {
    assert!(
        (a - b).abs() <= tol * a.abs().max(b.abs()).max(f64::MIN_POSITIVE),
        "{what}: {a:e} vs {b:e}"
    );
}

// Verifies: ACT-009
#[test]
fn schedule_from_a_known_history_matches_the_hand_computed_one() {
    // On 0-10 s (power 1.0, then 0.8 from a snapshot at 5 s), off 5 s, on 15-25 s at 0.5, off to 100 s.
    let mut h = synthetic(&[(0.0, 10.0, 1.0), (15.0, 25.0, 0.5)], 100.0);
    let mut mid = h.snapshots[0].clone();
    (mid.time_s, mid.power_fraction) = (5.0, 0.8);
    h.snapshots.insert(1, mid);
    let got = full_steps(&h, (0.0, 100.0), &[]).unwrap();
    let want = [
        step(5.0, 1.0),
        step(5.0, 0.8),
        step(5.0, 0.0),
        step(10.0, 0.5),
        step(75.0, 0.0),
    ];
    assert_eq!(got, want);
    close(
        fpe(&got),
        5.0 + 4.0 + 5.0,
        1e-15,
        "full-power-equivalent seconds",
    );
    // A window inside the history keeps the power in force at its start (0.8 from 5 s on).
    let got = full_steps(&h, (7.0, 20.0), &[]).unwrap();
    assert_eq!(got, [step(3.0, 0.8), step(5.0, 0.0), step(5.0, 0.5)]);
}

// Verifies: ACT-009
#[test]
fn requested_times_stay_step_boundaries_in_pulses_and_gaps() {
    let h = synthetic(&[(0.0, 10.0, 1.0), (15.0, 25.0, 0.5)], 100.0);
    let got = full_steps(&h, (0.0, 100.0), &[4.0, 30.0]).unwrap();
    assert_eq!(
        got,
        [
            step(4.0, 1.0),
            step(6.0, 1.0),
            step(5.0, 0.0),
            step(10.0, 0.5),
            step(5.0, 0.0),
            step(70.0, 0.0),
        ]
    );
    // A lumping run does not cross them.
    let rule = CondensationRule {
        tolerance: 1.0,
        shortest_half_life_s: 100.0,
    };
    let (lumped, _) = condense(&got, &rule, &[4.0, 30.0]).unwrap();
    let mut t = 0.0;
    let ends: Vec<f64> = lumped
        .iter()
        .map(|s| {
            t += s.duration_s;
            t
        })
        .collect();
    assert!(ends.contains(&4.0) && ends.contains(&30.0), "{ends:?}");
}

// Verifies: ACT-009
// Verifies: ACT-010
#[test]
fn condensation_matches_a_hand_computed_lumping() {
    // Limit = 100 s * ln(2) / ln(2) = 100 s: three 10 s pulses 5 s apart (40 s) become one step
    // of the same full-power-equivalent time, the 200 s outage stays, the last pulse is alone.
    let rule = CondensationRule {
        tolerance: 1.0,
        shortest_half_life_s: 100.0,
    };
    close(rule.max_block_s(), 100.0, 1e-15, "block limit");
    let steps = [
        step(10.0, 1.0),
        step(5.0, 0.0),
        step(10.0, 1.0),
        step(5.0, 0.0),
        step(10.0, 1.0),
        step(200.0, 0.0),
        step(10.0, 1.0),
    ];
    let (got, report) = condense(&steps, &rule, &[]).unwrap();
    assert_eq!(got, [step(40.0, 0.75), step(200.0, 0.0), step(10.0, 1.0)]);
    assert_eq!(
        (
            report.unlumped_step_count,
            report.condensed_step_count,
            report.lumped_block_count,
            report.outages_kept
        ),
        (7, 3, 1, 1)
    );
    close(report.longest_block_s, 40.0, 1e-15, "longest block");
    close(
        report.full_power_equivalent_s_unlumped,
        40.0,
        1e-15,
        "fpe before",
    );
    close(
        report.full_power_equivalent_s_condensed,
        40.0,
        1e-15,
        "fpe after",
    );
    close(
        report.relative_error_bound,
        math::powf(2.0, 0.4) - 1.0,
        1e-12,
        "bound",
    );
    // Tolerance zero lumps nothing.
    let (same, _) = condense(&steps, &CondensationRule::LOSSLESS, &[]).unwrap();
    assert_eq!(same, steps);
    // Bad rules say why.
    let bad = CondensationRule {
        tolerance: -0.1,
        shortest_half_life_s: 1.0,
    };
    assert!(
        condense(&steps, &bad, &[])
            .unwrap_err()
            .contains("tolerance")
    );
}

// Verifies: ACT-009
#[test]
fn changing_the_history_changes_the_activation_input_hash() {
    let rule = CondensationRule::LOSSLESS;
    let pulses = [(0.0, 10.0, 1.0), (15.0, 25.0, 0.5)];
    let a = irradiation_schedule(&synthetic(&pulses, 100.0), None, &rule, &[]).unwrap();
    let again = irradiation_schedule(&synthetic(&pulses, 100.0), None, &rule, &[]).unwrap();
    assert_eq!(a, again);
    assert!(a.verify());
    assert_eq!(a.history_sha256.len(), 64);
    for (what, other) in [
        ("longer outage", [(0.0, 10.0, 1.0), (16.0, 25.0, 0.5)]),
        ("lower power", [(0.0, 10.0, 1.0), (15.0, 25.0, 0.4)]),
    ] {
        let b = irradiation_schedule(&synthetic(&other, 100.0), None, &rule, &[]).unwrap();
        assert_ne!(a.history_sha256, b.history_sha256, "{what}: history hash");
        assert_ne!(a.input_sha256, b.input_sha256, "{what}: input hash");
    }
    // A change that leaves the steps alone still changes the hash through the history.
    let mut h = synthetic(&pulses, 100.0);
    h.notice.push_str(" edited");
    let c = irradiation_schedule(&h, None, &rule, &[]).unwrap();
    assert_eq!(a.steps, c.steps);
    assert_ne!(a.input_sha256, c.input_sha256);
    // The rule is part of the input too, and a tampered receipt does not verify.
    let loose = CondensationRule {
        tolerance: 0.5,
        shortest_half_life_s: 1.0,
    };
    let d = irradiation_schedule(&synthetic(&pulses, 100.0), None, &loose, &[]).unwrap();
    assert_ne!(a.input_sha256, d.input_sha256);
    let mut tampered = a.clone();
    tampered.steps[0].flux_scale = 2.0;
    assert!(!tampered.verify());
    // The receipt round-trips.
    let back: IrradiationSchedule =
        serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
    assert_eq!(back, a);
}

/// Inventory at the end of `n` identical pulses (`on_s` on, `off_s` off) of a nuclide produced
/// at unit rate and decaying with `lambda`: a geometric series, derived without the step code.
fn pulse_train_inventory(n: u32, on_s: f64, off_s: f64, lambda: f64) -> f64 {
    let one = -math::exp_m1(-lambda * on_s) / lambda;
    let q_exponent = -lambda * (on_s + off_s);
    one * (-math::exp_m1(f64::from(n) * q_exponent)) / (-math::exp_m1(q_exponent))
}

use faris_model::math;

// Verifies: ACT-009
// Verifies: ACT-010
#[test]
fn condensation_error_stays_inside_the_bound_on_an_analytic_pulse_train() {
    // 20 000 pulses of 1 s every 2 s: 39 999 unlumped steps, past the 10^4 the requirement names.
    let n = 20_000u32;
    let pulses: Vec<(f64, f64, f64)> = (0..n)
        .map(|k| (f64::from(2 * k), f64::from(2 * k + 1), 1.0))
        .collect();
    let h = synthetic(&pulses, f64::from(2 * n));
    let rule = CondensationRule {
        tolerance: 0.01,
        shortest_half_life_s: 3600.0,
    };
    let schedule = irradiation_schedule(&h, None, &rule, &[]).unwrap();
    let full = full_steps(&h, (0.0, f64::from(2 * n)), &[]).unwrap();
    assert_eq!(full.len(), 2 * n as usize);
    assert!(schedule.report.condensed_step_count < full.len() / 20);
    assert!(schedule.report.relative_error_bound <= rule.tolerance * (1.0 + 1e-12));
    close(
        schedule.report.full_power_equivalent_s_condensed,
        f64::from(n),
        1e-12,
        "fpe kept",
    );
    // Final gap: full ends with a 1 s gap after the last pulse; inventory is read at its start.
    let trim = |steps: &[IrradiationStep]| {
        let mut v = steps.to_vec();
        let last = v.pop().unwrap();
        assert_eq!(last.flux_scale, 0.0);
        v
    };
    let (full_t, lumped_t) = (trim(&full), trim(&schedule.steps));
    let mut worst: f64 = 0.0;
    for half_life_s in [3600.0, 2.0e4, 1.0e5, 1.0e7, 1.0e9] {
        let lambda = std::f64::consts::LN_2 / half_life_s;
        let exact = pulse_train_inventory(n, 1.0, 1.0, lambda);
        close(
            direct_inventory(&full_t, lambda),
            exact,
            1e-9,
            "unlumped steps against the closed form",
        );
        let err = (direct_inventory(&lumped_t, lambda) - exact).abs() / exact;
        assert!(
            err <= schedule.report.relative_error_bound,
            "T1/2 {half_life_s}: error {err:e} above bound {:e}",
            schedule.report.relative_error_bound
        );
        worst = worst.max(err);
    }
    assert!(
        worst > 0.0,
        "the lumping changes the answer, so the test can fail"
    );
    // Below the guaranteed half-life the lumping is not safe, and the report says so.
    let lambda = std::f64::consts::LN_2 / 1.0;
    let exact = pulse_train_inventory(n, 1.0, 1.0, lambda);
    let err = (direct_inventory(&lumped_t, lambda) - exact).abs() / exact;
    assert!(err > 0.1, "a 1 s nuclide is not covered: error {err:e}");
    assert!(schedule.report.not_covered.contains("half-life below"));
}

// Verifies: ACT-009
#[test]
fn a_long_outage_between_campaigns_is_never_averaged_away() {
    // Two campaigns of irregular 3 to 7 s pulses either side of a 30 day outage.
    let day = 86_400.0;
    let mut pulses = Vec::new();
    let mut t = 0.0;
    for k in 0..4_000u32 {
        let on = 3.0 + f64::from(k % 5);
        pulses.push((t, t + on, 1.0));
        t += on + 2.0 + f64::from(k % 3);
    }
    let campaign_end = t;
    t += 30.0 * day;
    for k in 0..4_000u32 {
        let on = 3.0 + f64::from(k % 5);
        pulses.push((t, t + on, 0.5));
        t += on + 2.0 + f64::from(k % 3);
    }
    let horizon = t + 1.0;
    let h = synthetic(&pulses, horizon);
    let rule = CondensationRule {
        tolerance: 0.001,
        shortest_half_life_s: 600.0,
    };
    let s = irradiation_schedule(&h, None, &rule, &[]).unwrap();
    assert!(s.report.outages_kept >= 1);
    assert!(
        s.steps
            .iter()
            .any(|x| x.flux_scale == 0.0 && x.duration_s >= 29.9 * day),
        "the 30 day outage is a step of its own"
    );
    let full = full_steps(&h, (0.0, horizon), &[]).unwrap();
    assert!(full.len() > 10_000);
    assert!(campaign_end < horizon);
    // Every nuclide at or above 600 s stays inside the bound, 30 day outage and all.
    for half_life_s in [600.0, 3600.0, 86_400.0, 30.0 * day, 1.0e8] {
        let lambda = std::f64::consts::LN_2 / half_life_s;
        let exact = direct_inventory(&full, lambda);
        let err = (direct_inventory(&s.steps, lambda) - exact).abs() / exact;
        assert!(
            err <= s.report.relative_error_bound,
            "T1/2 {half_life_s}: {err:e} above {:e}",
            s.report.relative_error_bound
        );
    }
    close(
        s.report.full_power_equivalent_s_condensed,
        s.report.full_power_equivalent_s_unlumped,
        1e-12,
        "fpe kept",
    );
}

// Verifies: ACT-009
#[test]
fn an_installation_schedule_covers_exactly_its_window() {
    let h = base_history();
    let magnets = installations(&h, "magnets").unwrap();
    assert!(magnets.len() >= 2, "fixture replaces the magnets");
    let (from, to) = magnets[1];
    let shutdown = to;
    let s = irradiation_schedule(
        &h,
        Some(("magnets", shutdown)),
        &CondensationRule::LOSSLESS,
        &[shutdown],
    )
    .unwrap();
    assert_eq!((s.window_start_s, s.window_end_s), (from, to));
    close(
        s.steps.iter().map(|x| x.duration_s).sum::<f64>(),
        to - from,
        1e-12,
        "window length",
    );
    assert_eq!(s.component.as_deref(), Some("magnets"));
    assert!(
        irradiation_schedule(
            &h,
            Some(("magnets", -5.0)),
            &CondensationRule::LOSSLESS,
            &[]
        )
        .is_err()
    );
}

#[test]
fn a_history_without_power_says_why() {
    let mut h = synthetic(&[(0.0, 10.0, 1.0)], 20.0);
    h.snapshots.clear();
    let err = full_steps(&h, (0.0, 20.0), &[]).unwrap_err();
    assert!(
        err.contains("no power fraction") && err.contains("snapshots"),
        "{err}"
    );
}
