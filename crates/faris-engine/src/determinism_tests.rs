//! Cross-platform bit-identity of the history math. The expected digests are
//! the SHA-256 of the compact JSON of each result; they must be equal on every
//! desktop platform (Linux, Windows, macOS on both CPU families) because
//! `verify.sh` recomputes recorded histories and compares digests. A failure
//! means a platform math library or a compiler transformation leaked into the
//! numbers: route the function through `faris_model::math` rather than editing
//! the constants. Run in release mode, where the 30-year history is quick:
//! `cargo test -p faris-engine --release determinism_`.

use crate::fixtures;
use crate::history::{TransportDrivingRates, run_operating_history};
use faris_model::history::OperatingHistoryAssumptions;
use sha2::{Digest, Sha256};

const DEMO_ASSUMPTIONS: &str =
    include_str!("../../../scenarios/arc-inspired/demo-operating-assumptions.json");
const RATES: &str = include_str!("../tests/fixtures/determinism_rates.json");

/// The 30-year demo history on the checked-in control reference rates.
const HISTORY_SHA256: &str = "b12eb936d6b6219dd7346fa3c1660736de1465732c3cff7699d94361ecc0b8af";
/// Eight sampled histories of the small synthetic fixture, seed 20261008.
const ENSEMBLE_SHA256: &str = "5896ab4ad906a6211d26d39ccf57f7b254433710c44770c5d7c4703fb45f8fd8";

fn digest<T: serde::Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).expect("results serialize");
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn determinism_operating_history_digest() {
    let assumptions: OperatingHistoryAssumptions = serde_json::from_str(DEMO_ASSUMPTIONS).unwrap();
    let rates: TransportDrivingRates = serde_json::from_str(RATES).unwrap();
    let history = run_operating_history(&assumptions, &rates).expect("the demo history runs");
    assert_eq!(digest(&history), HISTORY_SHA256);
}

#[test]
fn determinism_ensemble_digest() {
    let ensemble = fixtures::ensemble(8, 20261008, 'a');
    assert_eq!(digest(&ensemble), ENSEMBLE_SHA256);
}
