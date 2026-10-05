//! Background Monte Carlo ensembles of the operating history.
//!
//! After the nominal histories of the loaded arrangements are calculated, the
//! ensemble of each is calculated on a worker thread: the selected arrangement
//! first, then the others. The job starts 300 ms after the plan last changed,
//! uses at most half of the cores so the interface keeps its frames, and is
//! cancelled by any edit that invalidates the history. Finished ensembles are
//! cached in memory under their full input key (`EnsembleKey`), so an unchanged
//! key never reruns, and a study file can supply ensembles under the same keys.
//!
//! The ranges are transport Monte Carlo sampling uncertainty only.

use eframe::egui;
use faris_engine::{
    history::{HistoryResult, TransportDrivingRates},
    history_ensemble::{
        EnsembleComparison, EnsembleError, EnsembleSettings, EnsembleStatus, HistoryEnsemble,
        compare_ensembles, run_history_ensemble,
    },
    history_uncertainty::{EnsembleCache, EnsembleKey, choose_samples},
    jobs::Cancellation,
};
use faris_model::history::OperatingHistoryAssumptions;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// Quiet period between the plan last changing and the job starting.
type ComparisonResult = Result<Arc<EnsembleComparison>, String>;

pub const DEBOUNCE: Duration = Duration::from_millis(300);
pub use faris_engine::history_uncertainty::SAMPLE_CHOICES;
/// Paired comparisons kept in memory.
const COMPARISON_LIMIT: usize = 64;

/// Runs one ensemble. The real runner is `run_history_ensemble`; tests supply
/// their own to count and to block.
pub type Runner = Arc<
    dyn Fn(
            &TransportDrivingRates,
            &OperatingHistoryAssumptions,
            &EnsembleSettings,
            &Cancellation,
            &(dyn Fn(usize, usize) + Sync),
        ) -> Result<HistoryEnsemble, EnsembleError>
        + Send
        + Sync,
>;

/// What the display may say about one arrangement's ensemble.
#[derive(Clone, Debug)]
pub enum Status {
    /// No history to base an ensemble on yet.
    NotPlanned,
    /// Planned; the job has not started (the quiet period, or the history is
    /// still settling).
    Waiting,
    /// Behind another arrangement's ensemble.
    Queued,
    Running {
        done: usize,
        total: usize,
    },
    Ready(Arc<HistoryEnsemble>),
    /// The calculation itself failed; the text says why.
    Failed(String),
}

/// One arrangement's ensemble to calculate, with the exact inputs.
#[derive(Clone)]
struct Planned {
    id: String,
    key: EnsembleKey,
    rates: TransportDrivingRates,
    assumptions: OperatingHistoryAssumptions,
}

enum Message {
    Finished {
        key: EnsembleKey,
        result: Result<HistoryEnsemble, EnsembleError>,
    },
}

#[derive(Default)]
struct Counter {
    /// Position in the job's list of the ensemble now running.
    index: AtomicUsize,
    done: AtomicUsize,
    total: AtomicUsize,
}

struct Job {
    handle: Option<JoinHandle<()>>,
    cancellation: Cancellation,
    receiver: Receiver<Message>,
    counter: Arc<Counter>,
    /// The arrangements this job calculates, in order.
    order: Vec<(String, EnsembleKey)>,
}

pub struct Uncertainty {
    samples: u32,
    cache: EnsembleCache,
    failures: BTreeMap<EnsembleKey, String>,
    /// Arrangements whose inputs could not be keyed, with the reason.
    unkeyed: BTreeMap<String, String>,
    plan: Vec<Planned>,
    /// What the plan was built from: the history revision and the samples.
    plan_signature: Option<(u64, u32)>,
    plan_changed: Option<Instant>,
    job: Option<Job>,
    /// Cancelled workers still winding down.
    retiring: Vec<JoinHandle<()>>,
    comparisons: RefCell<BTreeMap<(EnsembleKey, EnsembleKey), ComparisonResult>>,
    runner: Runner,
    /// Applied to the rates before the key is formed; development checks only.
    rates_transform: Option<fn(&TransportDrivingRates) -> TransportDrivingRates>,
    /// Calculations started, for tests and the status line.
    runs_started: usize,
}

fn default_runner() -> Runner {
    Arc::new(|rates, assumptions, settings, cancellation, progress| {
        run_history_ensemble(rates, assumptions, settings, cancellation, progress)
    })
}

/// At most half the cores, between one and four: the interface thread and the
/// renderer keep the rest.
fn worker_threads() -> usize {
    std::thread::available_parallelism()
        .map_or(2, |n| n.get() / 2)
        .clamp(1, 4)
}

impl Default for Uncertainty {
    fn default() -> Self {
        Self::with_runner(default_runner())
    }
}

impl Uncertainty {
    pub fn with_runner(runner: Runner) -> Self {
        Self {
            samples: SAMPLE_CHOICES[0],
            cache: EnsembleCache::default(),
            failures: BTreeMap::new(),
            unkeyed: BTreeMap::new(),
            plan: Vec::new(),
            plan_signature: None,
            plan_changed: None,
            job: None,
            retiring: Vec::new(),
            comparisons: RefCell::new(BTreeMap::new()),
            runner,
            rates_transform: None,
            runs_started: 0,
        }
    }

    pub fn samples(&self) -> u32 {
        self.samples
    }

    pub fn set_samples(&mut self, samples: u32) {
        if self.samples != samples {
            self.samples = samples;
        }
    }

    /// Development checks only: replace the rates before keys are formed.
    #[cfg(any(test, feature = "uncertainty-fixture"))]
    pub fn with_rates_transform(
        mut self,
        transform: fn(&TransportDrivingRates) -> TransportDrivingRates,
    ) -> Self {
        self.rates_transform = Some(transform);
        self
    }

    /// True when ensembles are calculated on rates that were not recorded, so
    /// nothing from this session may be stored in a study file.
    pub fn is_fixture(&self) -> bool {
        self.rates_transform.is_some()
    }

    pub fn runs_started(&self) -> usize {
        self.runs_started
    }

    #[cfg(test)]
    pub fn is_running(&self) -> bool {
        self.job.is_some()
    }

    /// Cancel the running job and forget the plan: the history it belongs to
    /// has been invalidated by an edit. Finished ensembles stay cached.
    pub fn invalidate(&mut self) {
        self.cancel_job();
        self.plan.clear();
        self.unkeyed.clear();
        self.plan_signature = None;
        self.plan_changed = None;
    }

    fn cancel_job(&mut self) {
        if let Some(mut job) = self.job.take() {
            job.cancellation.cancel();
            if let Some(handle) = job.handle.take() {
                self.retiring.push(handle);
            }
        }
    }

    /// Put finished ensembles from a study file into the cache. Entries whose
    /// key does not describe their ensemble are dropped. The sample setting
    /// follows the file when it holds ensembles of one of the offered counts.
    pub fn seed(&mut self, stored: Vec<(EnsembleKey, HistoryEnsemble)>) {
        let mut counts = Vec::new();
        for (key, ensemble) in stored {
            if !key.describes(&ensemble) {
                continue;
            }
            counts.push(key.samples);
            self.cache.insert(key, Arc::new(ensemble));
        }
        if let Some(samples) = choose_samples(counts) {
            self.samples = samples;
        }
    }

    /// The finished ensembles belonging to the current plan, for saving.
    /// Empty in a fixture session.
    pub fn persistable(&self) -> Vec<(String, EnsembleKey, Arc<HistoryEnsemble>)> {
        if self.is_fixture() {
            return Vec::new();
        }
        self.plan
            .iter()
            .filter_map(|p| Some((p.id.clone(), p.key.clone(), self.cache.get(&p.key)?)))
            .collect()
    }

    fn key_of(&self, id: &str) -> Option<&EnsembleKey> {
        self.plan.iter().find(|p| p.id == id).map(|p| &p.key)
    }

    pub fn status(&self, id: &str) -> Status {
        if let Some(why) = self.unkeyed.get(id) {
            return Status::Failed(why.clone());
        }
        let Some(planned) = self.plan.iter().find(|p| p.id == id) else {
            return Status::NotPlanned;
        };
        if let Some(ensemble) = self.cache.get(&planned.key) {
            return Status::Ready(ensemble);
        }
        if let Some(why) = self.failures.get(&planned.key) {
            return Status::Failed(why.clone());
        }
        let Some(job) = &self.job else {
            return Status::Waiting;
        };
        match job.order.iter().position(|(i, _)| i == id) {
            Some(position) => {
                let current = job.counter.index.load(Ordering::Acquire);
                match position.cmp(&current) {
                    std::cmp::Ordering::Equal => Status::Running {
                        done: job.counter.done.load(Ordering::Acquire),
                        total: job.counter.total.load(Ordering::Acquire),
                    },
                    std::cmp::Ordering::Greater => Status::Queued,
                    std::cmp::Ordering::Less => Status::Waiting,
                }
            }
            None => Status::Waiting,
        }
    }

    pub fn ensemble(&self, id: &str) -> Option<Arc<HistoryEnsemble>> {
        match self.status(id) {
            Status::Ready(ensemble) => Some(ensemble),
            _ => None,
        }
    }

    /// The paired comparison of two arrangements' ensembles (second minus
    /// first). None unless both are evaluated; the error text says why a pair
    /// cannot be compared.
    pub fn comparison(&self, a: &str, b: &str) -> Option<Result<Arc<EnsembleComparison>, String>> {
        let (ea, eb) = (self.ensemble(a)?, self.ensemble(b)?);
        if ea.status != EnsembleStatus::Evaluated || eb.status != EnsembleStatus::Evaluated {
            return None;
        }
        let pair = (self.key_of(a)?.clone(), self.key_of(b)?.clone());
        let mut memo = self.comparisons.borrow_mut();
        if let Some(found) = memo.get(&pair) {
            return Some(found.clone());
        }
        if memo.len() >= COMPARISON_LIMIT {
            memo.clear();
        }
        let result = compare_ensembles(&ea, &eb).map(Arc::new);
        memo.insert(pair, result.clone());
        Some(result)
    }

    /// Plan the ensembles of `inputs` (selected arrangement first) when the
    /// history revision or the sample count changed since the last plan.
    fn plan_for(&mut self, revision: u64, inputs: &[(String, &HistoryResult)]) {
        if self.plan_signature == Some((revision, self.samples)) {
            return;
        }
        self.cancel_job();
        self.plan.clear();
        self.unkeyed.clear();
        self.plan_signature = Some((revision, self.samples));
        self.plan_changed = Some(Instant::now());
        for (id, history) in inputs {
            let rates = match self.rates_transform {
                Some(transform) => transform(&history.driving_rates),
                None => history.driving_rates.clone(),
            };
            let assumptions = history.assumptions.clone();
            match EnsembleKey::new(&rates, &assumptions, self.samples) {
                Ok(key) => self.plan.push(Planned {
                    id: id.clone(),
                    key,
                    rates,
                    assumptions,
                }),
                // Inputs that cannot even be keyed are shown as a failure.
                Err(why) => {
                    self.unkeyed.insert(id.clone(), why);
                }
            }
        }
        // A record without covariance cannot be sampled: its ensemble is the
        // instant "not evaluated" result, so it is made here, not in a job.
        let instant: Vec<Planned> = self
            .plan
            .iter()
            .filter(|p| p.rates.covariance.is_none() && !self.cache.contains(&p.key))
            .filter(|p| !self.failures.contains_key(&p.key))
            .cloned()
            .collect();
        for p in instant {
            self.run_inline(&p);
        }
    }

    fn settings_for(&self, planned: &Planned) -> EnsembleSettings {
        EnsembleSettings {
            samples: self.samples,
            seed: Some(planned.key.seed),
            threads: Some(worker_threads()),
        }
    }

    fn run_inline(&mut self, planned: &Planned) {
        self.runs_started += 1;
        let result = (self.runner)(
            &planned.rates,
            &planned.assumptions,
            &self.settings_for(planned),
            &Cancellation::default(),
            &|_, _| {},
        );
        self.accept(planned.key.clone(), result);
    }

    fn accept(&mut self, key: EnsembleKey, result: Result<HistoryEnsemble, EnsembleError>) {
        match result {
            Ok(ensemble) if key.describes(&ensemble) => {
                self.cache.insert(key, Arc::new(ensemble));
            }
            Ok(_) => {
                self.failures.insert(
                    key,
                    "The calculated ensemble does not match its inputs.".into(),
                );
            }
            // A cancelled ensemble is never partial and never shown.
            Err(EnsembleError::Cancelled) => {}
            Err(EnsembleError::Invalid(why)) => {
                self.failures.insert(key, why);
            }
        }
    }

    fn start_job(&mut self, ctx: &egui::Context) {
        let todo: Vec<Planned> = self
            .plan
            .iter()
            .filter(|p| !self.cache.contains(&p.key) && !self.failures.contains_key(&p.key))
            .cloned()
            .collect();
        if todo.is_empty() {
            return;
        }
        let cancellation = Cancellation::default();
        let counter = Arc::new(Counter::default());
        let (sender, receiver) = mpsc::channel();
        let order: Vec<(String, EnsembleKey)> =
            todo.iter().map(|p| (p.id.clone(), p.key.clone())).collect();
        let order_count = order.len();
        let settings: Vec<EnsembleSettings> = todo.iter().map(|p| self.settings_for(p)).collect();
        let runner = self.runner.clone();
        let (worker_cancel, worker_counter, context) =
            (cancellation.clone(), counter.clone(), ctx.clone());
        let spawned = std::thread::Builder::new()
            .name("faris-history-ensemble".into())
            .spawn(move || {
                for (position, (planned, settings)) in todo.iter().zip(&settings).enumerate() {
                    if worker_cancel.is_cancelled() {
                        break;
                    }
                    worker_counter.done.store(0, Ordering::Release);
                    worker_counter
                        .total
                        .store(settings.samples as usize, Ordering::Release);
                    worker_counter.index.store(position, Ordering::Release);
                    let result = runner(
                        &planned.rates,
                        &planned.assumptions,
                        settings,
                        &worker_cancel,
                        &|done, _| {
                            worker_counter.done.fetch_max(done, Ordering::AcqRel);
                        },
                    );
                    let stop = matches!(result, Err(EnsembleError::Cancelled));
                    let sent = sender.send(Message::Finished {
                        key: planned.key.clone(),
                        result,
                    });
                    context.request_repaint();
                    if stop || sent.is_err() {
                        break;
                    }
                }
            });
        match spawned {
            Ok(handle) => {
                self.runs_started += order_count;
                self.job = Some(Job {
                    handle: Some(handle),
                    cancellation,
                    receiver,
                    counter,
                    order,
                });
            }
            Err(error) => {
                for p in &self.plan {
                    if !self.cache.contains(&p.key) {
                        self.failures
                            .insert(p.key.clone(), format!("Cannot start the worker: {error}"));
                    }
                }
            }
        }
    }

    /// Once per frame, while the history is settled: plan if the history or the
    /// sample count changed, collect finished ensembles, and start the job
    /// once the quiet period has passed. Returns true when something visible
    /// changed.
    pub fn sync(
        &mut self,
        ctx: &egui::Context,
        revision: u64,
        inputs: &[(String, &HistoryResult)],
    ) -> bool {
        self.plan_for(revision, inputs);
        let mut changed = self.collect();
        if self.job.is_none()
            && let Some(since) = self.plan_changed
        {
            let remaining = DEBOUNCE.saturating_sub(since.elapsed());
            if remaining.is_zero() {
                self.plan_changed = None;
                let before = self.runs_started;
                self.start_job(ctx);
                changed |= self.runs_started != before;
            } else {
                ctx.request_repaint_after(remaining);
            }
        }
        if self.job.is_some() {
            // Progress is read from atomics; ask for a calm refresh.
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        changed
    }

    /// Collect finished ensembles and join finished workers.
    pub fn collect(&mut self) -> bool {
        let mut changed = false;
        let mut finished = Vec::new();
        let mut ended = false;
        if let Some(job) = &self.job {
            loop {
                match job.receiver.try_recv() {
                    Ok(Message::Finished { key, result }) => finished.push((key, result)),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        ended = true;
                        break;
                    }
                }
            }
        }
        for (key, result) in finished {
            self.accept(key, result);
            changed = true;
        }
        if ended && let Some(mut job) = self.job.take() {
            if let Some(handle) = job.handle.take() {
                let _ = handle.join();
            }
            changed = true;
        }
        self.retiring.retain(|handle| !handle.is_finished());
        changed
    }
}

impl Drop for Uncertainty {
    fn drop(&mut self) {
        self.cancel_job();
        for handle in self.retiring.drain(..) {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests;
