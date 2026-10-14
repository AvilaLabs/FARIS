//! Clocks that work on both targets: `std::time::Instant` and `SystemTime`
//! panic in the browser, so the browser build uses `web-time`.

#[cfg(not(target_arch = "wasm32"))]
pub use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
#[cfg(target_arch = "wasm32")]
pub use web_time::{Duration, Instant, SystemTime, UNIX_EPOCH};
