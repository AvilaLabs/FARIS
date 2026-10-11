//! The client side of the native CAD worker: the request and response types
//! ([`protocol`]) and the process client with its wall-clock bound ([`client`]).
//!
//! This crate holds no OpenCASCADE and builds in seconds. `faris-occt` builds the worker that
//! answers these requests; `faris-engine` depends only on this crate, so a normal FARIS build
//! never compiles OpenCASCADE.

pub mod client;
pub mod protocol;
