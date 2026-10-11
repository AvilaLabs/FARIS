//! OpenCASCADE behind a thin C++ shim, for FARIS's CAD layer.
//!
//! The crate reads STEP files (units, solids in reader order, names, assembly paths, colours,
//! volumes, centroids, boxes, validity), tessellates solids into `faris-facets/v1`, and
//! classifies batches of (u, v) points against faces. OpenCASCADE is built from a pinned
//! source release as static libraries by `build.rs`. See `README.md` for the licence note.
//!
//! FARIS calls OpenCASCADE only through [`client::Worker`], which runs the
//! `faris-occt-worker` binary as a separate process with time and memory limits. The
//! in-process API in [`native`] exists for that binary and for tests.

mod ffi;

pub use faris_occt_client::client;
pub mod inspect;
pub mod native;
pub mod tessellate;
pub mod units;
pub mod worker;

/// The OpenCASCADE release this crate is pinned to.
pub const OCCT_TAG: &str = env!("FARIS_OCCT_TAG");
/// The sha256 of that release's source tarball.
pub const OCCT_SOURCE_SHA256: &str = env!("FARIS_OCCT_SHA256");
