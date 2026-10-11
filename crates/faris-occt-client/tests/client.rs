//! The process client against stand-in workers (shell scripts that answer the JSON protocol).
#![cfg(unix)]

use faris_occt_client::client::{Bounds, Worker, WorkerError};
use faris_occt_client::protocol::Operation;
use std::{os::unix::fs::PermissionsExt, path::Path, time::Instant};

const OK_PING: &str = r#"printf '{"schema":"faris-occt-response/v1","status":"ok","result":{"occt":"7.9.3","occt_tag":"V7_9_3","helper":"faris-occt-1"}}' > "$2""#;

fn script(dir: &Path, body: &str) -> Worker {
    let path = dir.join("stub-worker");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    Worker::new(path)
}

fn short(seconds: f64) -> Bounds {
    Bounds {
        wall_seconds: seconds,
        ..Bounds::default()
    }
}

#[test]
fn a_ping_answer_names_the_worker() {
    let dir = tempfile::tempdir().unwrap();
    let pong = script(dir.path(), OK_PING).ping().unwrap();
    assert_eq!(pong.occt, "7.9.3");
    assert_eq!(pong.occt_tag.as_deref(), Some("V7_9_3"));
    assert_eq!(pong.helper.as_deref(), Some("faris-occt-1"));
}

#[test]
fn an_older_ping_answer_without_a_tag_is_still_read() {
    let dir = tempfile::tempdir().unwrap();
    let worker = script(
        dir.path(),
        r#"printf '{"schema":"faris-occt-response/v1","status":"ok","result":{"occt":"7.9.3"}}' > "$2""#,
    );
    let pong = worker.ping().unwrap();
    assert!(pong.occt_tag.is_none() && pong.helper.is_none());
}

#[test]
fn a_ping_with_no_version_is_a_bad_response() {
    let dir = tempfile::tempdir().unwrap();
    let worker = script(
        dir.path(),
        r#"printf '{"schema":"faris-occt-response/v1","status":"ok","result":{}}' > "$2""#,
    );
    assert!(matches!(
        worker.ping().unwrap_err(),
        WorkerError::BadResponse(_)
    ));
}

#[test]
fn a_worker_that_exits_without_a_response_is_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let error = script(dir.path(), "echo boom >&2; exit 7")
        .ping()
        .unwrap_err();
    assert!(
        matches!(&error, WorkerError::Crashed { stderr_tail, .. } if stderr_tail.contains("boom")),
        "{error}"
    );
}

#[test]
fn a_missing_program_is_a_spawn_error() {
    let error = Worker::new("/nonexistent/faris-occt-worker")
        .ping()
        .unwrap_err();
    assert!(matches!(error, WorkerError::Spawn(_)), "{error}");
}

#[test]
fn a_worker_past_the_wall_clock_is_killed() {
    let dir = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let error = script(dir.path(), "exec sleep 30")
        .run(Operation::Ping, short(0.3))
        .unwrap_err();
    assert!(matches!(error, WorkerError::TimedOut { .. }), "{error}");
    assert!(started.elapsed().as_secs_f64() < 10.0);
}

#[test]
fn a_cancelled_run_kills_the_worker() {
    let dir = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let error = script(dir.path(), "exec sleep 30")
        .run_cancellable(Operation::Ping, short(60.0), &|| {
            started.elapsed().as_secs_f64() > 0.2
        })
        .unwrap_err();
    assert!(matches!(error, WorkerError::Cancelled), "{error}");
    assert!(started.elapsed().as_secs_f64() < 10.0);
}

#[test]
fn an_explicit_worker_path_wins_over_the_search() {
    let worker = Worker::locate_with(Some("/some/where/faris-occt-worker".into())).unwrap();
    assert_eq!(worker.program(), Path::new("/some/where/faris-occt-worker"));
}
