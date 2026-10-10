//! Worker threads with progress and cancel, in the engine's conventions
//! (PERF-022, PERF-024): `std::thread::scope` workers pull chunks of work,
//! check the shared [`Cancellation`] between chunks, and report
//! `(stage, done, total)` after each chunk from whichever thread finished it.
//! Results are merged in chunk order, so the answer never depends on the thread
//! count or on which thread was fast.

use crate::jobs::Cancellation;
use std::{
    ops::Range,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

/// The run was cancelled between chunks; nothing partial is returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

pub type Progress<'a> = &'a (dyn Fn(&str, usize, usize) + Sync);

#[derive(Clone, Copy)]
pub struct Ctl<'a> {
    pub cancel: &'a Cancellation,
    pub progress: Progress<'a>,
    pub threads: usize,
}

pub fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 16)
}

impl<'a> Ctl<'a> {
    pub fn new(cancel: &'a Cancellation, progress: Progress<'a>, threads: usize) -> Self {
        Self {
            cancel,
            progress,
            threads: threads.max(1),
        }
    }

    pub fn check(&self) -> Result<(), Cancelled> {
        if self.cancel.is_cancelled() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }

    pub fn report(&self, stage: &str, done: usize, total: usize) {
        (self.progress)(stage, done, total);
    }

    /// Run `work` over `items` split into chunks of `chunk`, on the configured threads.
    /// Returns one result per chunk, in chunk order.
    pub fn run<T: Send>(
        &self,
        stage: &str,
        items: usize,
        chunk: usize,
        work: impl Fn(Range<usize>) -> T + Sync,
    ) -> Result<Vec<T>, Cancelled> {
        self.check()?;
        let chunk = chunk.max(1);
        let chunks = items.div_ceil(chunk);
        let next = AtomicUsize::new(0);
        let done = AtomicUsize::new(0);
        let results: Mutex<Vec<(usize, T)>> = Mutex::new(Vec::with_capacity(chunks));
        let worker = || {
            loop {
                if self.cancel.is_cancelled() {
                    break;
                }
                let index = next.fetch_add(1, Ordering::AcqRel);
                if index >= chunks {
                    break;
                }
                let range = index * chunk..((index + 1) * chunk).min(items);
                let len = range.len();
                let value = work(range);
                results.lock().expect("results lock").push((index, value));
                let finished = done.fetch_add(len, Ordering::AcqRel) + len;
                self.report(stage, finished, items);
            }
        };
        let threads = self.threads.min(chunks.max(1));
        if threads <= 1 {
            worker();
        } else {
            std::thread::scope(|scope| {
                for _ in 0..threads {
                    scope.spawn(worker);
                }
            });
        }
        self.check()?;
        let mut collected = results.into_inner().expect("results lock");
        collected.sort_by_key(|(index, _)| *index);
        Ok(collected.into_iter().map(|(_, value)| value).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn results_come_back_in_chunk_order_for_any_thread_count() {
        let cancel = Cancellation::default();
        for threads in [1, 2, 7] {
            let seen = AtomicUsize::new(0);
            let progress = |_: &str, done: usize, total: usize| {
                assert!(done <= total);
                seen.fetch_add(1, Ordering::Relaxed);
            };
            let ctl = Ctl::new(&cancel, &progress, threads);
            let out = ctl
                .run("sum", 1000, 64, |r| r.map(|i| i as u64).sum::<u64>())
                .unwrap();
            assert_eq!(out.len(), 16);
            assert_eq!(out.iter().sum::<u64>(), 499_500);
            assert_eq!(out[0], (0..64u64).sum::<u64>());
            assert_eq!(seen.load(Ordering::Relaxed), 16);
        }
    }

    // Verifies: PERF-024
    #[test]
    fn cancelling_stops_the_workers_and_returns_nothing_partial() {
        let cancel = Cancellation::default();
        let started = AtomicBool::new(false);
        let progress = |_: &str, _: usize, _: usize| {};
        let ctl = Ctl::new(&cancel, &progress, 4);
        let result = ctl.run("slow", 10_000, 1, |_| {
            started.store(true, Ordering::Release);
            cancel.cancel();
            std::thread::sleep(std::time::Duration::from_millis(1));
        });
        assert_eq!(result.unwrap_err(), Cancelled);
        assert!(started.load(Ordering::Acquire));
        // a pre-cancelled run does no work
        let ran = AtomicBool::new(false);
        let again = ctl.run("never", 10, 1, |_| ran.store(true, Ordering::Release));
        assert!(again.is_err());
        assert!(!ran.load(Ordering::Acquire));
    }

    #[test]
    fn zero_items_is_an_empty_result() {
        let cancel = Cancellation::default();
        let progress = |_: &str, _: usize, _: usize| {};
        let ctl = Ctl::new(&cancel, &progress, 3);
        assert!(ctl.run("none", 0, 8, |_| 1).unwrap().is_empty());
    }
}
