use super::*;
use crate::fixtures;
use crate::history::{HistoryResult, run_operating_history_cancellable};
use faris_model::maintenance::{GoverningQuantity, MAINTENANCE_ASSUMPTIONS_VERSION};
use serde_json::Value;

// ---------------------------------------------------------------- parity ----

const PARITY: &str = include_str!("../../tests/fixtures/maintenance_parity.json");

fn parity() -> Value {
    serde_json::from_str(PARITY).unwrap()
}

fn close(a: f64, b: f64, what: &str) {
    let scale = a.abs().max(b.abs());
    assert!(
        (a - b).abs() <= 1e-12 * scale || a == b,
        "{what}: rust {a:e} vs python {b:e}"
    );
}

fn pairs(v: &Value) -> Vec<(f64, f64)> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| (p[0].as_f64().unwrap(), p[1].as_f64().unwrap()))
        .collect()
}

#[test]
fn cooling_grid_matches_the_reference() {
    let want: Vec<f64> = parity()["cooling_grid_s"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    let got = cooling_grid_s(&CoolingGrid::default());
    assert_eq!(got.len(), want.len());
    for (i, (g, w)) in got.iter().zip(&want).enumerate() {
        close(*g, *w, &format!("grid[{i}]"));
    }
    assert_eq!((got[0], got[39]), (3600.0, 365.0 * 86400.0));
}

#[test]
fn interpolate_matches_the_reference() {
    let fixtures = parity();
    let cases = fixtures["interpolate"].as_array().unwrap();
    assert!(cases.len() >= 30);
    for (i, c) in cases.iter().enumerate() {
        let got = interpolate(&pairs(&c["points"]), c["t"].as_f64().unwrap());
        close(
            got,
            c["expected"].as_f64().unwrap(),
            &format!("interpolate case {i}"),
        );
    }
}

#[test]
fn combined_curve_matches_the_reference() {
    let fixtures = parity();
    for (i, c) in fixtures["combined_curve"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let series: Vec<Vec<(f64, f64)>> =
            c["series"].as_array().unwrap().iter().map(pairs).collect();
        let refs: Vec<&[(f64, f64)]> = series.iter().map(Vec::as_slice).collect();
        let volumes: Vec<f64> = c["volumes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let got = combined_curve(&refs, &volumes, c["max_s"].as_f64().unwrap());
        if c["expected"].is_null() {
            assert!(got.is_err(), "combined case {i} should fail");
            continue;
        }
        let want = pairs(&c["expected"]);
        let got = got.unwrap();
        assert_eq!(got.len(), want.len(), "combined case {i} length");
        for (j, (g, w)) in got.iter().zip(&want).enumerate() {
            close(g.0, w.0, &format!("combined {i} t[{j}]"));
            close(g.1, w.1, &format!("combined {i} q[{j}]"));
        }
    }
}

#[test]
fn cooldown_matches_the_reference() {
    let fixtures = parity();
    let cases = fixtures["cooldown"].as_array().unwrap();
    let (mut evaluated, mut limited, mut gaps) = (0, 0, 0);
    for (i, c) in cases.iter().enumerate() {
        let curve = pairs(&c["curve"]);
        let got = cooldown(
            &curve,
            c["q_star"].as_f64().unwrap(),
            c["max_s"].as_f64().unwrap(),
        );
        let want = &c["expected"];
        if want["status"] == "NOT_EVALUATED" {
            let ne = got.unwrap_err();
            assert!(!ne.reason.is_empty() && !ne.next_step.is_empty());
            gaps += 1;
            continue;
        }
        let got = got.unwrap_or_else(|e| panic!("cooldown case {i}: {e:?}"));
        close(
            got.cooldown_s,
            want["cooldown_s"].as_f64().unwrap(),
            &format!("cooldown {i}"),
        );
        assert_eq!(
            got.window_limited,
            want["window_limited"].as_bool().unwrap(),
            "case {i}"
        );
        evaluated += 1;
        limited += usize::from(got.window_limited);
    }
    assert!(evaluated > 0 && limited > 0 && gaps > 0);
}

#[test]
fn calibrate_matches_the_reference() {
    let fixtures = parity();
    let cases = fixtures["calibrate"].as_array().unwrap();
    let (mut evaluated, mut gaps) = (0, 0);
    for (i, c) in cases.iter().enumerate() {
        let curve = pairs(&c["curve"]);
        let got = calibrate(
            &curve,
            c["target_cooldown_s"].as_f64().unwrap(),
            365.0 * 86400.0,
        );
        let want = &c["expected"];
        if want["status"] == "NOT_EVALUATED" {
            let ne = got.unwrap_err();
            assert!(!ne.reason.is_empty() && !ne.next_step.is_empty());
            gaps += 1;
        } else {
            close(
                got.unwrap(),
                want["q_star"].as_f64().unwrap(),
                &format!("calibrate {i}"),
            );
            evaluated += 1;
        }
    }
    assert!(evaluated > 3 && gaps > 3);
}

#[test]
fn generalized_grid_has_exact_endpoints_and_log_spacing() {
    let grid = CoolingGrid {
        min_s: 1.0,
        max_s: 1000.0,
        points: 4,
    };
    let g = cooling_grid_s(&grid);
    assert_eq!((g[0], g[3]), (1.0, 1000.0));
    close(g[1], 10.0, "g1");
    close(g[2], 100.0, "g2");
}

// ------------------------------------------------------------------ loop ----

const MAGNET_TARGET_S: f64 = 10.0;

fn class_assumptions(threshold: Threshold) -> MaintenanceAssumptions {
    let mut classes = BTreeMap::new();
    classes.insert(
        "magnet".to_string(),
        MaintenanceClass {
            component_id: "magnets".into(),
            governing: vec!["blanket".into(), "magnets".into()],
            work_s: 2.0,
            threshold,
        },
    );
    MaintenanceAssumptions {
        schema_version: MAINTENANCE_ASSUMPTIONS_VERSION.into(),
        governing_quantity: GoverningQuantity::Heat,
        classes,
        cooling: CoolingGrid {
            min_s: 1.0,
            max_s: 1000.0,
            points: 40,
        },
        max_iterations: 10,
        convergence_s: 1e-6,
    }
}

fn calibrated() -> MaintenanceAssumptions {
    class_assumptions(Threshold::Calibrate {
        design: "ref".into(),
        target_cooldown_s: MAGNET_TARGET_S,
    })
}

fn designs() -> Vec<DesignInput> {
    let alt = {
        let mut a = fixtures::assumptions();
        for l in &mut a.service_limits {
            if l.component_id == "magnets" {
                l.limit = 2.0e11;
            }
        }
        a
    };
    vec![
        DesignInput {
            name: "ref".into(),
            history_assumptions: fixtures::assumptions(),
        },
        DesignInput {
            name: "alt".into(),
            history_assumptions: alt,
        },
    ]
}

/// Counts runs and delegates to the real operating-history model.
#[derive(Default)]
struct Runner {
    runs: Vec<(String, Option<Vec<f64>>)>,
}

impl HistoryRunner for Runner {
    fn run(
        &mut self,
        design: &str,
        assumptions: &OperatingHistoryAssumptions,
        cancel: &Cancellation,
    ) -> Result<HistoryResult, String> {
        let magnets = assumptions
            .service_limits
            .iter()
            .find(|l| l.component_id == "magnets")
            .and_then(|l| l.replacement_durations_s.clone());
        self.runs.push((design.to_string(), magnets));
        run_operating_history_cancellable(
            assumptions,
            &fixtures::rates_without_covariance(0.0, 'a'),
            cancel,
        )
    }
}

/// Sum of two exponentials whose amplitude grows with the installation's time in the machine,
/// so cooldowns lengthen with plant age.
struct Analytic {
    scale: BTreeMap<String, f64>,
    /// End of the curve in seconds since shutdown, when shorter than the grid.
    cut_s: Option<f64>,
    calls: Vec<(String, usize)>,
    cancel_on_call: Option<u32>,
}

impl Analytic {
    fn new() -> Self {
        Self {
            scale: BTreeMap::new(),
            cut_s: None,
            calls: Vec::new(),
            cancel_on_call: None,
        }
    }
}

const VOLUME_M3: f64 = 2.0;

fn installed_at(history: &HistoryResult, component: &str, shutdown_s: f64) -> f64 {
    history
        .events
        .iter()
        .filter(|e| {
            e.component_id.as_deref() == Some(component)
                && e.kind == crate::history::EventKind::ReplacementCompleted
                && e.time_s <= shutdown_s
        })
        .map(|e| e.time_s)
        .fold(0.0, f64::max)
}

fn analytic_heat_w(amplitude_w: f64, age_s: f64, t: f64) -> f64 {
    amplitude_w * (1.0 + age_s / 10.0) * (0.7 * math::exp(-t / 8.0) + 0.3 * math::exp(-t / 300.0))
}

impl DecaySource for Analytic {
    fn curves(
        &mut self,
        design: &str,
        history: &HistoryResult,
        requests: &[CurveRequest],
        grid_s: &[f64],
        cancel: &Cancellation,
    ) -> Result<Vec<Result<InstallationCurve, NotEvaluated>>, String> {
        self.calls.push((design.to_string(), requests.len()));
        if self.cancel_on_call == Some(self.calls.len() as u32) {
            cancel.cancel();
        }
        let scale = self.scale.get(design).copied().unwrap_or(1.0);
        Ok(requests
            .iter()
            .map(|r| {
                let install_s = installed_at(history, &r.component, r.shutdown_s);
                let age = r.shutdown_s - install_s;
                let amp = if r.component == "magnets" { 40.0 } else { 25.0 } * scale;
                let points = grid_s
                    .iter()
                    .copied()
                    .filter(|&t| self.cut_s.is_none_or(|c| t <= c))
                    .map(|t| (t, analytic_heat_w(amp, age, t)))
                    .collect();
                Ok(InstallationCurve {
                    points,
                    volume_m3: VOLUME_M3,
                    install_s,
                })
            })
            .collect())
    }
}

fn run(
    a: &MaintenanceAssumptions,
    runner: &mut Runner,
    source: &mut Analytic,
    cancel: &Cancellation,
) -> Result<MaintenanceResult, String> {
    run_maintenance(a, &designs(), runner, source, cancel, &mut |_, _, _| {})
}

#[test]
fn calibrated_loop_converges_and_lengthens_cooldowns_with_age() {
    let (mut runner, mut source) = (Runner::default(), Analytic::new());
    let result = run(
        &calibrated(),
        &mut runner,
        &mut source,
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(result.schema_version, MAINTENANCE_RESULT_VERSION);
    let r = &result.designs["ref"];
    assert_eq!(r.computed.status, Status::Evaluated);
    let n = r.computed.converged_at_iteration.unwrap();
    assert!(
        n >= 2,
        "durations change from the fixed ones, so one iteration is not enough"
    );
    // Iteration 1 reuses the fixed history: one run for it, one more per later iteration.
    let ref_runs = runner.runs.iter().filter(|(d, _)| d == "ref").count();
    assert_eq!(ref_runs as u32, n);
    assert_eq!(runner.runs[0].1, None);
    // One batch call per design per iteration, plus the calibration batch.
    assert_eq!(
        source.calls.iter().filter(|(d, _)| d == "ref").count() as u32,
        n + 1
    );

    // Calibration: the first replacement at the fixed durations cools for exactly the target.
    let first = &r.computed.iterations[0].events[0];
    assert_eq!((first.k, first.duration_used_s), (1, 5.0));
    close(
        first.cooldown_s.unwrap(),
        MAGNET_TARGET_S,
        "calibrated first cooldown",
    );
    close(
        first.duration_computed_s.unwrap(),
        MAGNET_TARGET_S + 2.0,
        "first duration",
    );
    let th = &r.thresholds["magnet"];
    assert_eq!(th.source, "calibrated");
    assert_eq!(th.status, Status::Evaluated);
    assert_eq!(th.calibration_event_start_s, Some(first.start_s));

    // Later cooldowns are longer: the installations are older.
    let last = r.computed.iterations.last().unwrap();
    let cooldowns: Vec<f64> = last.events.iter().map(|e| e.cooldown_s.unwrap()).collect();
    assert!(cooldowns.len() >= 2);
    assert!(cooldowns.windows(2).all(|w| w[1] > w[0]), "{cooldowns:?}");

    // Converged: the durations used are the durations computed, within the step.
    assert!(last.max_change_s <= 1e-6);
    for e in &last.events {
        assert!((e.duration_used_s - e.duration_computed_s.unwrap()).abs() <= 1e-6);
    }
    // The computed history ran with those durations on every limit of the component.
    let (_, used) = runner.runs.iter().rev().find(|(d, _)| d == "ref").unwrap();
    assert_eq!(used.as_ref().unwrap().len(), last.events.len());

    // Summaries.
    let (fixed, computed) = (&r.fixed, r.computed.summary.as_ref().unwrap());
    assert!(computed.total_replacement_downtime_s > fixed.total_replacement_downtime_s);
    assert!(computed.availability < fixed.availability);
    assert!(fixed.availability < 1.0 && fixed.history_end_s == 100.0);
    assert!(fixed.lifetime_net_electricity_mwh.is_some());
    close(
        fixed.availability,
        1.0 - fixed.total_replacement_downtime_s / 100.0,
        "availability",
    );
    assert!(
        computed.lifetime_net_electricity_mwh.unwrap()
            < fixed.lifetime_net_electricity_mwh.unwrap()
    );
    // The other design evaluated too, with its own events.
    assert_eq!(result.designs["alt"].computed.status, Status::Evaluated);
    assert!(result.designs["alt"].fixed.replacements.len() > r.fixed.replacements.len());
}

#[test]
fn shares_sum_to_one_and_in_service_time_is_shutdown_minus_install() {
    let (mut runner, mut source) = (Runner::default(), Analytic::new());
    let result = run(
        &calibrated(),
        &mut runner,
        &mut source,
        &Cancellation::default(),
    )
    .unwrap();
    let mut seen = 0;
    for d in result.designs.values() {
        for it in &d.computed.iterations {
            for e in &it.events {
                let why = e.why.as_ref().unwrap();
                let sum: f64 = why.governing.iter().map(|g| g.share).sum();
                assert!((sum - 1.0).abs() < 1e-12, "{sum}");
                assert!(why.curve_start_w_per_m3 > 0.0 && why.curve_start_s >= 1.0);
                let blanket = &why.governing[0];
                let magnets = &why.governing[1];
                assert_eq!(blanket.component, "blanket");
                close(blanket.in_service_s, e.start_s, "permanent blanket age");
                if e.k == 1 {
                    close(magnets.in_service_s, e.start_s, "first magnet age");
                } else {
                    assert!(magnets.in_service_s < e.start_s);
                }
                // The magnets' amplitude is larger than the blanket's at equal age.
                assert!(magnets.share > 0.0 && blanket.share > 0.0);
                seen += 1;
            }
        }
    }
    assert!(seen > 6);
}

#[test]
fn explicit_q_star_skips_calibration() {
    let a = class_assumptions(Threshold::QStar {
        q_star_w_per_m3: 4.0,
    });
    let (mut runner, mut source) = (Runner::default(), Analytic::new());
    let result = run(&a, &mut runner, &mut source, &Cancellation::default()).unwrap();
    let r = &result.designs["ref"];
    assert_eq!(r.thresholds["magnet"].source, "explicit");
    assert_eq!(r.thresholds["magnet"].q_star_w_per_m3, Some(4.0));
    assert_eq!(r.thresholds["magnet"].calibration_design, None);
    assert_eq!(r.computed.status, Status::Evaluated);
    // No calibration batch: one source call per design per iteration.
    let ref_calls = source.calls.iter().filter(|(d, _)| d == "ref").count() as u32;
    assert_eq!(ref_calls, r.computed.converged_at_iteration.unwrap());
    // The cooldown is where the analytic curve reaches q* (the grid is coarse: 10%).
    for e in &r.computed.iterations.last().unwrap().events {
        let t = e.cooldown_s.unwrap();
        let age_m = e.why.as_ref().unwrap().governing[1].in_service_s;
        let age_b = e.why.as_ref().unwrap().governing[0].in_service_s;
        let q =
            (analytic_heat_w(25.0, age_b, t) + analytic_heat_w(40.0, age_m, t)) / (2.0 * VOLUME_M3);
        assert!((q / 4.0 - 1.0).abs() < 0.1, "q at cooldown {q}");
        close(
            e.duration_computed_s.unwrap(),
            t + 2.0,
            "work plus cooldown",
        );
    }
}

#[test]
fn a_design_that_never_cools_is_not_evaluated_while_the_other_evaluates() {
    let (mut runner, mut source) = (Runner::default(), Analytic::new());
    source.scale.insert("alt".into(), 1e6);
    let result = run(
        &calibrated(),
        &mut runner,
        &mut source,
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(result.designs["ref"].computed.status, Status::Evaluated);
    let alt = &result.designs["alt"];
    assert_eq!(alt.computed.status, Status::NotEvaluated);
    let ne = alt.computed.not_evaluated.as_ref().unwrap();
    assert!(ne.reason.contains("never falls below q*"), "{}", ne.reason);
    assert!(
        ne.reason.starts_with("iteration 1: magnets replacement 1"),
        "{}",
        ne.reason
    );
    assert!(!ne.next_step.is_empty());
    assert!(alt.computed.summary.is_none());
    // The failing event is on record, and the fixed model still reports.
    let rec = &alt.computed.iterations[0].events[0];
    assert!(rec.not_evaluated.is_some() && rec.cooldown_s.is_none());
    assert!(alt.fixed.total_replacement_downtime_s > 0.0);
    // The contrast keeps the fixed difference and explains the absent computed one.
    let c = &result.contrasts[0];
    assert_eq!((c.a.as_str(), c.b.as_str()), ("alt", "ref"));
    assert!(c.computed_difference_s.is_none() && c.ratio_computed_over_fixed.is_none());
    assert!(c.not_evaluated.is_some());
}

#[test]
fn failed_calibration_makes_every_design_not_evaluated_with_the_reason() {
    let a = class_assumptions(Threshold::Calibrate {
        design: "ref".into(),
        target_cooldown_s: 5000.0,
    });
    let (mut runner, mut source) = (Runner::default(), Analytic::new());
    let result = run(&a, &mut runner, &mut source, &Cancellation::default()).unwrap();
    for d in result.designs.values() {
        assert_eq!(d.thresholds["magnet"].status, Status::NotEvaluated);
        assert_eq!(d.computed.status, Status::NotEvaluated);
        let ne = d.computed.not_evaluated.as_ref().unwrap();
        assert!(
            ne.reason.starts_with("calibration unavailable for magnet"),
            "{}",
            ne.reason
        );
        assert!(ne.reason.contains("outside the curve"));
        assert!(!ne.next_step.is_empty());
        assert!(d.fixed.total_replacement_downtime_s > 0.0);
    }
}

#[test]
fn no_convergence_within_the_iteration_limit_is_not_evaluated() {
    let mut a = calibrated();
    a.max_iterations = 1;
    let (mut runner, mut source) = (Runner::default(), Analytic::new());
    let result = run(&a, &mut runner, &mut source, &Cancellation::default()).unwrap();
    let d = &result.designs["ref"];
    assert_eq!(d.computed.status, Status::NotEvaluated);
    let ne = d.computed.not_evaluated.as_ref().unwrap();
    assert!(
        ne.reason
            .starts_with("no convergence in 1 iterations (last change"),
        "{}",
        ne.reason
    );
    assert!(!ne.next_step.is_empty());
    assert_eq!(d.computed.iterations.len(), 1);
    assert!(d.computed.iterations[0].max_change_s > 1.0);
}

#[test]
fn a_cooldown_cut_short_by_the_window_blocks_convergence() {
    let a = class_assumptions(Threshold::QStar {
        q_star_w_per_m3: 1e-9,
    });
    let (mut runner, mut source) = (Runner::default(), Analytic::new());
    source.cut_s = Some(20.0);
    let result = run(&a, &mut runner, &mut source, &Cancellation::default()).unwrap();
    let d = &result.designs["ref"];
    assert_eq!(d.computed.status, Status::NotEvaluated);
    assert!(
        d.computed
            .not_evaluated
            .as_ref()
            .unwrap()
            .reason
            .contains("window")
    );
    let it = &d.computed.iterations[0];
    assert!(it.window_limited);
    assert!(
        it.events
            .iter()
            .all(|e| e.window_limited && e.cooldown_s.unwrap() <= 20.0)
    );
}

#[test]
fn cancellation_returns_promptly() {
    // Before anything runs.
    let (mut runner, mut source) = (Runner::default(), Analytic::new());
    let cancel = Cancellation::default();
    cancel.cancel();
    let err = run(&calibrated(), &mut runner, &mut source, &cancel).unwrap_err();
    assert_eq!(err, "cancelled");
    assert!(runner.runs.is_empty() && source.calls.is_empty());

    // Raised by the source during the calibration batch: no history is run after it.
    let (mut runner, mut source) = (Runner::default(), Analytic::new());
    source.cancel_on_call = Some(1);
    let err = run(
        &calibrated(),
        &mut runner,
        &mut source,
        &Cancellation::default(),
    )
    .unwrap_err();
    assert_eq!(err, "cancelled");
    assert_eq!(runner.runs.len(), 2, "only the two fixed histories");
    assert_eq!(source.calls.len(), 1);
}

#[test]
fn bad_input_is_an_error_not_a_verdict() {
    let (mut runner, mut source) = (Runner::default(), Analytic::new());
    let cancel = Cancellation::default();
    let go = |a: &MaintenanceAssumptions, d: &[DesignInput], r: &mut Runner, s: &mut Analytic| {
        run_maintenance(a, d, r, s, &cancel, &mut |_, _, _| {})
    };
    assert!(go(&calibrated(), &[], &mut runner, &mut source).is_err());
    let mut dup = designs();
    dup[1].name = "ref".into();
    assert!(go(&calibrated(), &dup, &mut runner, &mut source).is_err());
    let missing = class_assumptions(Threshold::Calibrate {
        design: "nowhere".into(),
        target_cooldown_s: 10.0,
    });
    assert!(
        go(&missing, &designs(), &mut runner, &mut source)
            .unwrap_err()
            .contains("nowhere")
    );
    let mut dose = calibrated();
    dose.governing_quantity = GoverningQuantity::Dose;
    assert!(
        go(&dose, &designs(), &mut runner, &mut source)
            .unwrap_err()
            .contains("use heat")
    );
    // A class whose component has no replaceable limit in a design.
    let mut other = calibrated();
    other.classes.get_mut("magnet").unwrap().component_id = "blanket".into();
    assert!(
        go(&other, &designs(), &mut runner, &mut source)
            .unwrap_err()
            .contains("replacement_duration_s")
    );
    assert!(runner.runs.is_empty());
}

#[test]
fn progress_reports_design_iteration_and_message() {
    let (mut runner, mut source) = (Runner::default(), Analytic::new());
    let mut seen: Vec<(String, u32, String)> = Vec::new();
    run_maintenance(
        &calibrated(),
        &designs(),
        &mut runner,
        &mut source,
        &Cancellation::default(),
        &mut |d, i, m| seen.push((d.into(), i, m.into())),
    )
    .unwrap();
    assert!(
        seen.iter()
            .any(|(d, i, m)| d == "ref" && *i == 0 && m.contains("fixed"))
    );
    assert!(
        seen.iter()
            .any(|(d, i, m)| d == "alt" && *i == 1 && m.contains("max change"))
    );
}

#[test]
fn contrast_ratio_is_null_under_thirty_days_and_reported_above() {
    fn design(fixed_s: f64, computed_s: Option<f64>) -> DesignResult {
        let summary = |downtime| HistorySummary {
            outcome: HistoryOutcome::HorizonCompleted,
            history_end_s: 1e9,
            lifetime_net_electricity_mwh: Some(1.0),
            total_replacement_downtime_s: downtime,
            availability: 0.9,
            replacements: vec![],
        };
        DesignResult {
            fixed: summary(fixed_s),
            computed: ComputedResult {
                status: if computed_s.is_some() {
                    Status::Evaluated
                } else {
                    Status::NotEvaluated
                },
                not_evaluated: None,
                summary: computed_s.map(summary),
                converged_at_iteration: None,
                iterations: vec![],
            },
            thresholds: BTreeMap::new(),
        }
    }
    let day = 86_400.0;
    let mut designs = BTreeMap::new();
    designs.insert("b".to_string(), design(10.0 * day, Some(50.0 * day)));
    designs.insert("a".to_string(), design(60.0 * day, Some(200.0 * day)));
    designs.insert("c".to_string(), design(65.0 * day, Some(80.0 * day)));
    designs.insert("d".to_string(), design(1.0 * day, None));
    let c = contrasts(&designs);
    let get = |a: &str, b: &str| c.iter().find(|x| x.a == a && x.b == b).unwrap();
    assert_eq!(c.len(), 6);
    // a-b: fixed 50 d, computed 150 d, ratio 3.
    let ab = get("a", "b");
    close(ab.fixed_difference_s, 50.0 * day, "fixed");
    close(ab.computed_difference_s.unwrap(), 150.0 * day, "computed");
    close(ab.ratio_computed_over_fixed.unwrap(), 3.0, "ratio");
    assert!(ab.not_evaluated.is_none());
    // a-c: fixed difference 5 d is under 30 d: no ratio, with a reason.
    let ac = get("a", "c");
    assert!(ac.computed_difference_s.is_some());
    assert!(ac.ratio_computed_over_fixed.is_none());
    let ne = ac.not_evaluated.as_ref().unwrap();
    assert!(ne.reason.contains("30 days") && !ne.next_step.is_empty());
    // A design without a computed model gives no computed difference.
    assert!(get("a", "d").computed_difference_s.is_none());
    assert!(get("a", "d").not_evaluated.is_some());
}

#[test]
fn result_round_trips_through_json() {
    let (mut runner, mut source) = (Runner::default(), Analytic::new());
    let result = run(
        &calibrated(),
        &mut runner,
        &mut source,
        &Cancellation::default(),
    )
    .unwrap();
    let json = serde_json::to_string(&result).unwrap();
    let back: MaintenanceResult = serde_json::from_str(&json).unwrap();
    assert_eq!(back, result);
    assert!(back.inputs.is_empty());
}

#[test]
fn replacement_events_are_counted_per_component_in_time_order() {
    let history = run_operating_history_cancellable(
        &fixtures::assumptions(),
        &fixtures::rates_without_covariance(0.0, 'a'),
        &Cancellation::default(),
    )
    .unwrap();
    let a = calibrated();
    let events = replacement_events(&history, &a.classes);
    assert!(events.len() >= 2);
    for (i, e) in events.iter().enumerate() {
        assert_eq!((e.k as usize, e.class.as_str()), (i + 1, "magnet"));
        close(e.end_s.unwrap_or(100.0) - e.start_s, 5.0, "fixed duration");
    }
    let total: f64 = events
        .iter()
        .map(|e| e.end_s.unwrap_or(100.0) - e.start_s)
        .sum();
    close(total_downtime_s(&history, &a.classes), total, "downtime");
    assert_eq!(history_end_s(&history), 100.0);
}
