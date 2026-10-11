//! `faris-occt-worker REQUEST.json RESPONSE.json` runs one request (see `faris_occt::worker`)
//! and writes the response file. It sets its own memory and CPU limits first.
//!
//! `faris-occt-worker inspect STEP [--assume-unit mm]` prints the `step-inspect` report to
//! standard output, for use by hand.

use faris_occt::worker::{Limits, Operation, Outcome, RESPONSE_SCHEMA, Request, Response, execute};
use nix::sys::resource::{Resource, setrlimit};
use std::{fs, path::PathBuf, process::ExitCode};

fn apply_limits(limits: &Limits) -> Result<(), String> {
    if let Some(mb) = limits.address_space_mb {
        let bytes = mb.saturating_mul(1 << 20);
        setrlimit(Resource::RLIMIT_AS, bytes, bytes)
            .map_err(|e| format!("could not set the memory limit: {e}"))?;
    }
    if let Some(seconds) = limits.cpu_seconds {
        setrlimit(Resource::RLIMIT_CPU, seconds, seconds)
            .map_err(|e| format!("could not set the CPU limit: {e}"))?;
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("inspect") {
        return inspect_by_hand(&args[1..]);
    }
    let [request_path, response_path] = args.as_slice() else {
        eprintln!("usage: faris-occt-worker REQUEST.json RESPONSE.json");
        return ExitCode::from(2);
    };
    let outcome = fs::read(request_path)
        .map_err(|e| format!("{request_path}: {e}"))
        .and_then(|bytes| {
            serde_json::from_slice::<Request>(&bytes).map_err(|e| format!("bad request: {e}"))
        })
        .and_then(|request| {
            if request.schema != faris_occt::worker::REQUEST_SCHEMA {
                return Err(format!(
                    "request schema {:?} is not supported",
                    request.schema
                ));
            }
            apply_limits(&request.limits)?;
            execute(&request.operation)
        });
    let response = Response {
        schema: RESPONSE_SCHEMA.into(),
        outcome: match outcome {
            Ok(result) => Outcome::Ok { result },
            Err(message) => Outcome::Error { message },
        },
    };
    let failed = matches!(response.outcome, Outcome::Error { .. });
    let written = serde_json::to_vec(&response)
        .map_err(|e| e.to_string())
        .and_then(|bytes| {
            fs::write(response_path, bytes).map_err(|e| format!("{response_path}: {e}"))
        });
    match (written, failed) {
        (Err(error), _) => {
            eprintln!("faris-occt-worker: {error}");
            ExitCode::from(3)
        }
        (Ok(()), true) => ExitCode::from(1),
        (Ok(()), false) => ExitCode::SUCCESS,
    }
}

fn inspect_by_hand(args: &[String]) -> ExitCode {
    let mut step: Option<PathBuf> = None;
    let mut assume_unit = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--assume-unit" => assume_unit = iter.next().cloned(),
            other => step = Some(PathBuf::from(other)),
        }
    }
    let Some(step) = step else {
        eprintln!("usage: faris-occt-worker inspect STEP [--assume-unit m|cm|mm|inch]");
        return ExitCode::from(2);
    };
    match execute(&Operation::Inspect { step, assume_unit }) {
        Ok(value) => {
            println!("{}", serde_json::to_string_pretty(&value).expect("json"));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("faris-occt-worker: {error}");
            ExitCode::from(1)
        }
    }
}
