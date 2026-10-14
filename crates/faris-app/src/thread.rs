//! Worker threads on both targets. The desktop uses `std::thread`. The browser
//! page has no threads to start, so there each job runs to completion inside
//! `spawn` and its result is waiting when the caller next polls; the channels
//! and handles the panels already use behave the same.

#[cfg(not(target_arch = "wasm32"))]
pub use std::thread::{Builder, JoinHandle, available_parallelism, sleep, spawn};

#[cfg(target_arch = "wasm32")]
pub use browser::{Builder, JoinHandle, available_parallelism, sleep, spawn};

#[cfg(target_arch = "wasm32")]
mod browser {
    use std::{io, num::NonZeroUsize, time::Duration};

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
    pub struct Builder;

    impl Builder {
        pub fn new() -> Self {
            Self
        }
        pub fn name(self, _name: String) -> Self {
            self
        }
        pub fn spawn<F, T>(self, job: F) -> io::Result<JoinHandle<T>>
        where
            F: FnOnce() -> T + Send + 'static,
            T: Send + 'static,
        {
            // A panic in the job is caught as a thread's would be, so the
            // panel reports it instead of the page stopping.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
            Ok(JoinHandle { result })
        }
    }

    pub fn spawn<F, T>(job: F) -> JoinHandle<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        JoinHandle {
            result: std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)),
        }
    }

    pub fn available_parallelism() -> io::Result<NonZeroUsize> {
        Ok(NonZeroUsize::MIN)
    }

    /// The page cannot block; waits in worker code are skipped.
    pub fn sleep(_duration: Duration) {}
}
