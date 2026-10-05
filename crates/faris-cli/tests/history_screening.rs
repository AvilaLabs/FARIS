//! `faris history` prints the research-screening statement after writing a
//! result, and leaves the result file's bytes (receipt-bound) alone.

use std::process::Command;

fn faris(arguments: &[&std::ffi::OsStr]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_faris"))
        .args(arguments)
        .output()
        .expect("run faris")
}

// Verifies: LEG-040
#[test]
fn history_run_and_ensemble_print_the_statement_without_changing_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let assumptions = dir.path().join("assumptions.json");
    let rates = dir.path().join("rates.json");
    let write = |path: &std::path::Path, value: &dyn erased::Json| {
        std::fs::write(path, value.to_vec()).unwrap();
    };
    write(&assumptions, &faris_engine::fixtures::assumptions());
    write(
        &rates,
        &faris_engine::fixtures::rates_without_covariance(0.05, 'a'),
    );

    let run_out = dir.path().join("history.json");
    let run = faris(&[
        "history".as_ref(),
        "run".as_ref(),
        "--assumptions".as_ref(),
        assumptions.as_os_str(),
        "--rates".as_ref(),
        rates.as_os_str(),
        "--output".as_ref(),
        run_out.as_os_str(),
    ]);
    assert!(run.status.success(), "{run:?}");
    let statement = faris_model::RESEARCH_SCREENING_STATEMENT;
    assert!(String::from_utf8_lossy(&run.stderr).contains(statement));
    // The file is the HistoryResult exactly, with its own notice.
    let written: faris_engine::history::HistoryResult =
        serde_json::from_slice(&std::fs::read(&run_out).unwrap()).unwrap();
    assert_eq!(
        written,
        faris_engine::history::run_operating_history(
            &faris_engine::fixtures::assumptions(),
            &faris_engine::fixtures::rates_without_covariance(0.05, 'a'),
        )
        .unwrap()
    );

    let ens_out = dir.path().join("ensemble.json");
    let ens = faris(&[
        "history".as_ref(),
        "ensemble".as_ref(),
        "--assumptions".as_ref(),
        assumptions.as_os_str(),
        "--rates".as_ref(),
        rates.as_os_str(),
        "--samples".as_ref(),
        "4".as_ref(),
        "--output".as_ref(),
        ens_out.as_os_str(),
    ]);
    assert!(ens.status.success(), "{ens:?}");
    assert!(String::from_utf8_lossy(&ens.stderr).contains(statement));
}

mod erased {
    pub trait Json {
        fn to_vec(&self) -> Vec<u8>;
    }
    impl<T: serde::Serialize> Json for T {
        fn to_vec(&self) -> Vec<u8> {
            serde_json::to_vec_pretty(self).unwrap()
        }
    }
}
