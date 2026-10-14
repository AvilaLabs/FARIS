//! Worker threads on both targets. The desktop uses `std::thread`. The browser
//! page has no threads to start, so there each job runs to completion inside
//! `spawn` and its result is waiting when the caller next polls; the channels
//! and handles the panels already use behave the same.

#[cfg(not(target_arch = "wasm32"))]
pub use std::thread::{Builder, JoinHandle, available_parallelism, sleep, spawn};

#[cfg(target_arch = "wasm32")]
pub use browser::{Builder, JoinHandle, available_parallelism, sleep, slow_jobs, spawn};

#[cfg(target_arch = "wasm32")]
mod browser {
    use std::{io, num::NonZeroUsize, sync::Mutex, time::Duration};

    /// Jobs that held the page for 100 ms or more: (name, milliseconds).
    static SLOW_JOBS: Mutex<Vec<(String, u32)>> = Mutex::new(Vec::new());

    pub fn slow_jobs() -> Vec<(String, u32)> {
        SLOW_JOBS.lock().map(|v| v.clone()).unwrap_or_default()
    }

    fn timed<T>(name: &str, job: impl FnOnce() -> T) -> std::thread::Result<T> {
        let started = web_time::Instant::now();
        // A panic in the job is caught as a thread's would be, so the
        // panel reports it instead of the page stopping.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
        let ms = started.elapsed().as_millis() as u32;
        if ms >= 100
            && let Ok(mut slow) = SLOW_JOBS.lock()
        {
            slow.push((name.to_owned(), ms));
        }
        result
    }

    pub struct JoinHandle<T> {
        result: std::thread::Result<T>,
    }

    impl<T> JoinHandle<T> {
        pub fn is_finished(&self) -> bool {
            true
        }
        pub fn join(self) -> std::thread::Result<T> {
            self.result
        }
    }

    #[derive(Default)]
    pub struct Builder {
        name: String,
    }

    impl Builder {
        pub fn new() -> Self {
            Self::default()
        }
        pub fn name(mut self, name: String) -> Self {
            self.name = name;
            self
        }
        pub fn spawn<F, T>(self, job: F) -> io::Result<JoinHandle<T>>
        where
            F: FnOnce() -> T + Send + 'static,
            T: Send + 'static,
        {
            Ok(JoinHandle {
                result: timed(&self.name, job),
            })
        }
    }

    pub fn spawn<F, T>(job: F) -> JoinHandle<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        JoinHandle {
            result: timed("unnamed", job),
        }
    }

    pub fn available_parallelism() -> io::Result<NonZeroUsize> {
        Ok(NonZeroUsize::MIN)
    }

    /// The page cannot block; waits in worker code are skipped.
    pub fn sleep(_duration: Duration) {}
}
