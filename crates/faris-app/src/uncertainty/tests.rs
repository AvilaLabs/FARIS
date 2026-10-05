use super::*;
use faris_engine::{
    fixtures::{assumptions, rates_with_covariance, rates_without_covariance},
    history::run_operating_history,
};
use std::sync::Mutex;

fn history(artifact: char, limit_scale: f64) -> HistoryResult {
    let mut a = assumptions();
    a.service_limits[0].limit *= limit_scale;
    run_operating_history(&a, &rates_with_covariance(0.06, 0.4, artifact)).unwrap()
}

fn history_without_covariance(artifact: char) -> HistoryResult {
    run_operating_history(&assumptions(), &rates_without_covariance(0.06, artifact)).unwrap()
}

/// The real engine, counting calls and noting which arrangement ran, by the
/// first letter of its transport artifact hash.
#[derive(Clone, Default)]
struct Log {
    calls: Arc<Mutex<Vec<char>>>,
}

impl Log {
    fn runner(&self) -> Runner {
        let calls = self.calls.clone();
        Arc::new(
            move |rates, assumptions, settings, cancellation, progress| {
                let letter = rates.transport_artifact_sha256.chars().next().unwrap();
                calls.lock().unwrap().push(letter.to_ascii_lowercase());
                run_history_ensemble(rates, assumptions, settings, cancellation, progress)
            },
        )
    }

    fn calls(&self) -> Vec<char> {
        self.calls.lock().unwrap().clone()
    }
}

/// Drive `sync` the way the application does, once per frame, until `done`.
fn drive(
    un: &mut Uncertainty,
    ctx: &egui::Context,
    revision: u64,
    inputs: &[(String, &HistoryResult)],
    done: impl Fn(&Uncertainty) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        un.sync(ctx, revision, inputs);
        if done(un) {
            return;
        }
        assert!(Instant::now() < deadline, "the ensembles did not finish");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn all_ready(ids: &'static [&'static str]) -> impl Fn(&Uncertainty) -> bool {
    move |un| {
        ids.iter()
            .all(|id| matches!(un.status(id), Status::Ready(_)))
    }
}

fn tiny(un: &mut Uncertainty) {
    // Small ensembles keep the tests quick; the setting is the real field.
    un.samples = 6;
}

#[test]
fn the_job_waits_for_the_quiet_period_then_runs_the_selected_arrangement_first() {
    let (a, b, c) = (history('a', 1.0), history('b', 1.0), history('c', 1.0));
    let log = Log::default();
    let mut un = Uncertainty::with_runner(log.runner());
    tiny(&mut un);
    let ctx = egui::Context::default();
    // The caller orders the inputs: the selected arrangement first.
    let inputs: Vec<(String, &HistoryResult)> =
        vec![("B".into(), &b), ("A".into(), &a), ("C".into(), &c)];
    un.sync(&ctx, 1, &inputs);
    assert!(
        matches!(un.status("B"), Status::Waiting),
        "{:?}",
        un.status("B")
    );
    assert!(!un.is_running() && log.calls().is_empty());
    std::thread::sleep(DEBOUNCE + Duration::from_millis(50));
    drive(&mut un, &ctx, 1, &inputs, all_ready(&["A", "B", "C"]));
    assert_eq!(log.calls(), vec!['b', 'a', 'c']);
    assert_eq!(un.runs_started(), 3);
    for id in ["A", "B", "C"] {
        let e = un.ensemble(id).unwrap();
        assert_eq!(e.status, EnsembleStatus::Evaluated);
        assert_eq!(e.samples.len(), 6);
    }
}

#[test]
fn an_unchanged_key_never_reruns() {
    let (a, b) = (history('a', 1.0), history('b', 1.0));
    let log = Log::default();
    let mut un = Uncertainty::with_runner(log.runner());
    tiny(&mut un);
    let ctx = egui::Context::default();
    let inputs: Vec<(String, &HistoryResult)> = vec![("A".into(), &a), ("B".into(), &b)];
    drive(&mut un, &ctx, 1, &inputs, |u| {
        matches!(u.status("A"), Status::Ready(_)) && matches!(u.status("B"), Status::Ready(_))
    });
    assert_eq!(log.calls().len(), 2);

    // The history was recalculated to the same inputs (a new revision): the
    // plan is rebuilt, every key is found in the cache, nothing runs.
    un.sync(&ctx, 2, &inputs);
    assert!(matches!(un.status("A"), Status::Ready(_)));
    std::thread::sleep(DEBOUNCE + Duration::from_millis(50));
    un.sync(&ctx, 2, &inputs);
    assert!(!un.is_running());
    assert_eq!(log.calls().len(), 2);

    // An edit and the edit undone: the earlier ensembles are still cached.
    un.invalidate();
    let edited = history('a', 1.5);
    let edited_inputs: Vec<(String, &HistoryResult)> = vec![("A".into(), &edited)];
    drive(&mut un, &ctx, 3, &edited_inputs, all_ready(&["A"]));
    assert_eq!(log.calls().len(), 3);
    un.invalidate();
    un.sync(&ctx, 4, &inputs);
    assert!(matches!(un.status("A"), Status::Ready(_)));
    std::thread::sleep(DEBOUNCE + Duration::from_millis(50));
    un.sync(&ctx, 4, &inputs);
    assert!(!un.is_running());
    assert_eq!(log.calls().len(), 3);
}

#[test]
fn a_different_sample_count_runs_again_and_the_first_stays_cached() {
    let a = history('a', 1.0);
    let log = Log::default();
    let mut un = Uncertainty::with_runner(log.runner());
    let ctx = egui::Context::default();
    let inputs: Vec<(String, &HistoryResult)> = vec![("A".into(), &a)];
    un.samples = 4;
    drive(&mut un, &ctx, 1, &inputs, all_ready(&["A"]));
    un.samples = 5;
    // The sample count is part of the plan signature.
    un.sync(&ctx, 1, &inputs);
    assert!(!matches!(un.status("A"), Status::Ready(_)));
    drive(&mut un, &ctx, 1, &inputs, all_ready(&["A"]));
    assert_eq!(log.calls().len(), 2);
    assert_eq!(un.ensemble("A").unwrap().samples.len(), 5);
    un.samples = 4;
    un.sync(&ctx, 1, &inputs);
    assert_eq!(un.ensemble("A").unwrap().samples.len(), 4);
    assert_eq!(log.calls().len(), 2);
}

#[test]
fn an_edit_cancels_the_running_ensemble_and_nothing_partial_appears() {
    let a = history('a', 1.0);
    let started = Arc::new(AtomicUsize::new(0));
    let saw_cancel = Arc::new(AtomicUsize::new(0));
    let (s, c) = (started.clone(), saw_cancel.clone());
    let runner: Runner = Arc::new(move |_, _, _, cancellation, progress| {
        s.fetch_add(1, Ordering::SeqCst);
        // A long ensemble: report a little progress, then wait to be cancelled.
        progress(3, 200);
        while !cancellation.is_cancelled() {
            std::thread::sleep(Duration::from_millis(5));
        }
        c.fetch_add(1, Ordering::SeqCst);
        Err(EnsembleError::Cancelled)
    });
    let mut un = Uncertainty::with_runner(runner);
    let ctx = egui::Context::default();
    let inputs: Vec<(String, &HistoryResult)> = vec![("A".into(), &a)];
    let deadline = Instant::now() + Duration::from_secs(20);
    while !matches!(un.status("A"), Status::Running { .. }) || started.load(Ordering::SeqCst) == 0 {
        un.sync(&ctx, 1, &inputs);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    // Progress is visible while it runs.
    loop {
        if let Status::Running { done, total } = un.status("A")
            && done == 3
        {
            assert_eq!(total, 200);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    // The history is invalidated by an edit.
    un.invalidate();
    assert!(!un.is_running());
    assert!(matches!(un.status("A"), Status::NotPlanned));
    let deadline = Instant::now() + Duration::from_secs(20);
    while saw_cancel.load(Ordering::SeqCst) == 0 {
        un.collect();
        assert!(Instant::now() < deadline, "the worker never saw the cancel");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(un.cache.is_empty() && un.persistable().is_empty());
}

#[test]
fn a_record_without_covariance_is_not_evaluated_at_once_without_a_job() {
    let a = history_without_covariance('a');
    let log = Log::default();
    let mut un = Uncertainty::with_runner(log.runner());
    let ctx = egui::Context::default();
    let inputs: Vec<(String, &HistoryResult)> = vec![("A".into(), &a)];
    un.sync(&ctx, 1, &inputs);
    let Status::Ready(e) = un.status("A") else {
        panic!("{:?}", un.status("A"));
    };
    let EnsembleStatus::NotEvaluated { why, next_step } = &e.status else {
        panic!("expected not evaluated");
    };
    assert!(why.contains("no covariance") && next_step.starts_with("Rerun transport"));
    assert!(!un.is_running());
    // It is cached under its key: no further runs.
    std::thread::sleep(DEBOUNCE + Duration::from_millis(50));
    un.sync(&ctx, 1, &inputs);
    un.invalidate();
    un.sync(&ctx, 2, &inputs);
    assert_eq!(log.calls().len(), 1);
}

#[test]
fn stored_ensembles_are_reused_only_under_an_exact_key() {
    let a = history('a', 1.0);
    let edited = history('a', 1.2);
    let key = |h: &HistoryResult, samples| {
        EnsembleKey::new(&h.driving_rates, &h.assumptions, samples).unwrap()
    };
    let stored = |h: &HistoryResult, samples: u32| {
        let k = key(h, samples);
        let e = run_history_ensemble(
            &h.driving_rates,
            &h.assumptions,
            &EnsembleSettings {
                samples,
                seed: Some(k.seed),
                threads: Some(1),
            },
            &Cancellation::default(),
            &|_, _| {},
        )
        .unwrap();
        (k, e)
    };
    let ctx = egui::Context::default();
    let inputs: Vec<(String, &HistoryResult)> = vec![("A".into(), &a)];

    // Matching key: reused, nothing runs.
    let log = Log::default();
    let mut un = Uncertainty::with_runner(log.runner());
    un.seed(vec![stored(&a, 200)]);
    // The setting follows the file.
    assert_eq!(un.samples(), 200);
    un.sync(&ctx, 1, &inputs);
    assert!(matches!(un.status("A"), Status::Ready(_)));
    std::thread::sleep(DEBOUNCE + Duration::from_millis(50));
    un.sync(&ctx, 1, &inputs);
    assert!(!un.is_running() && log.calls().is_empty());
    assert_eq!(un.persistable().len(), 1);

    // A stored ensemble of a sample count the app does not offer: the setting
    // stays and the key misses, so it is recomputed.
    let log = Log::default();
    let mut un = Uncertainty::with_runner(log.runner());
    un.seed(vec![stored(&a, 6)]);
    un.sync(&ctx, 1, &inputs);
    assert_eq!(un.samples(), 200);
    assert!(!matches!(un.status("A"), Status::Ready(_)));
    drop(un);

    // Ensemble stored for different assumptions: a miss, recomputed.
    let log = Log::default();
    let mut un = Uncertainty::with_runner(log.runner());
    tiny(&mut un);
    un.seed(vec![stored(&edited, 6)]);
    drive(&mut un, &ctx, 1, &inputs, all_ready(&["A"]));
    assert_eq!(log.calls().len(), 1);

    // An entry whose key does not describe its ensemble is dropped.
    let mut un = Uncertainty::with_runner(Log::default().runner());
    let (mut k, e) = stored(&a, 6);
    k.samples = 7;
    un.seed(vec![(k, e)]);
    assert!(un.cache.is_empty());
}

#[test]
fn the_pairwise_comparison_needs_two_evaluated_ensembles_and_is_remembered() {
    let (a, b) = (history('a', 1.0), history('b', 1.0));
    let none = history_without_covariance('c');
    let mut un = Uncertainty::with_runner(Log::default().runner());
    tiny(&mut un);
    let ctx = egui::Context::default();
    let inputs: Vec<(String, &HistoryResult)> =
        vec![("A".into(), &a), ("B".into(), &b), ("N".into(), &none)];
    drive(&mut un, &ctx, 1, &inputs, all_ready(&["A", "B", "N"]));
    let comparison = un.comparison("A", "B").unwrap().unwrap();
    assert_eq!(comparison.pairs, 6);
    // Asked again: the same stored result.
    assert!(Arc::ptr_eq(
        &comparison,
        &un.comparison("A", "B").unwrap().unwrap()
    ));
    // A not-evaluated side gives no comparison; the caller shows its reason.
    assert!(un.comparison("A", "N").is_none());
    assert!(un.comparison("A", "missing").is_none());
}

#[test]
fn a_fixture_session_stores_nothing_and_uses_the_replaced_rates() {
    // The recorded rates have no covariance; the fixture gives them a
    // synthetic one so the evaluated views can be seen.
    fn with_synthetic(rates: &TransportDrivingRates) -> TransportDrivingRates {
        let mut r = rates.clone();
        r.covariance = Some(faris_engine::fixtures::synthetic_covariance(rates, 0.4));
        r
    }
    let a = history_without_covariance('a');
    let mut un =
        Uncertainty::with_runner(Log::default().runner()).with_rates_transform(with_synthetic);
    tiny(&mut un);
    let ctx = egui::Context::default();
    let inputs: Vec<(String, &HistoryResult)> = vec![("A".into(), &a)];
    drive(&mut un, &ctx, 1, &inputs, all_ready(&["A"]));
    assert_eq!(un.ensemble("A").unwrap().status, EnsembleStatus::Evaluated);
    assert!(un.is_fixture() && un.persistable().is_empty());
}
