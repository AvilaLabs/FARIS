//! Exit codes by failure class, SIGINT on a running job, and the tool listing, end to end.

use std::process::{Command, Output};

fn faris(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_faris"))
        .args(arguments)
        .output()
        .expect("run faris")
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("exited normally")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn each_failure_class_exits_with_its_own_documented_code() {
    // Verifies: AUTO-004
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.faris");
    let broken = dir.path().join("broken.faris");
    std::fs::write(&broken, b"not a zip").unwrap();
    let missing = missing.to_str().unwrap();
    let broken = broken.to_str().unwrap();

    // ok
    assert_eq!(code(&faris(&["doctor"])), 0);
    assert_eq!(code(&faris(&["--help"])), 0);
    assert_eq!(code(&faris(&["--version"])), 0);
    // usage: the command line does not parse
    assert_eq!(code(&faris(&["no-such-command"])), 64);
    assert_eq!(code(&faris(&["export"])), 64);
    // validation: well-formed command, input missing
    assert_eq!(code(&faris(&["study-file", "verify", missing])), 2);
    // rejected evidence: a file that is not a study
    assert_eq!(code(&faris(&["study-file", "verify", broken])), 1);
    assert_eq!(
        code(&faris(&["export", "--study", broken, "--output", "."])),
        1
    );
}

#[test]
fn the_help_text_lists_every_exit_code() {
    // Verifies: AUTO-004
    let help = text(&faris(&["--help"]).stdout);
    for line in [
        "0  ok",
        "1  rejected evidence",
        "2  validation",
        "3  solver failure",
        "4  cancelled",
        "5  resource refused",
        "64  usage",
    ] {
        assert!(help.contains(line), "{line} missing from:\n{help}");
    }
}

#[cfg(unix)]
mod jobs {
    use super::*;
    use std::{
        io::Read,
        os::unix::fs::PermissionsExt,
        path::Path,
        process::Stdio,
        time::{Duration, Instant},
    };

    /// A stand-in "python" and "openmc": a shell script that starts a long sleep, records
    /// its pid, and waits.
    fn sleeper(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
        let pid_file = dir.join("sleeper.pid");
        let script = dir.join("sleeper.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nsleep 120 &\necho $! > '{}'\nwait\n",
                pid_file.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        (script, pid_file)
    }

    fn control_args(script: &Path, output: &Path) -> Vec<String> {
        let script = script.to_str().unwrap().to_owned();
        vec![
            "control".into(),
            "absorber".into(),
            "--python".into(),
            script.clone(),
            "--openmc".into(),
            script,
            "--output".into(),
            output.to_str().unwrap().into(),
        ]
    }

    // Verifies: AUTO-005
    #[test]
    fn sigint_stops_a_running_job_within_two_seconds_and_leaves_no_orphan() {
        let dir = tempfile::tempdir().unwrap();
        let (script, pid_file) = sleeper(dir.path());
        let output = dir.path().join("evidence");
        let mut child = Command::new(env!("CARGO_BIN_EXE_faris"))
            .args(control_args(&script, &output))
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let waited = Instant::now();
        let sleeper_pid = loop {
            if let Some(pid) = std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|text| text.trim().parse::<u32>().ok())
            {
                break pid;
            }
            assert!(
                waited.elapsed() < Duration::from_secs(20),
                "job never started"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        let interrupted = Instant::now();
        let sent = Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap();
        assert!(sent.success());
        let status = child.wait().unwrap();
        let stopped_in = interrupted.elapsed();
        assert!(
            stopped_in <= Duration::from_secs(2),
            "took {stopped_in:?} to stop"
        );
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        assert_eq!(status.code(), Some(4), "{stderr}");
        // The job's own child is gone too (the runner kills the process group).
        #[cfg(target_os = "linux")]
        {
            let proc = std::path::PathBuf::from(format!("/proc/{sleeper_pid}"));
            let deadline = Instant::now() + Duration::from_secs(2);
            while proc.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(!proc.exists(), "pid {sleeper_pid} outlived the job");
        }
        #[cfg(not(target_os = "linux"))]
        let _ = sleeper_pid;
        assert!(output.join("execution.json").is_file());
    }

    // Verifies: AUTO-004, PERF-041
    #[test]
    fn a_job_the_memory_check_refuses_exits_five_and_names_the_setting() {
        let dir = tempfile::tempdir().unwrap();
        let (script, _) = sleeper(dir.path());
        let output = dir.path().join("evidence");
        let mut arguments = vec![
            "--setting".to_owned(),
            // More memory kept free than any machine has.
            "memory.margin_mb=4194304".to_owned(),
        ];
        arguments.extend(control_args(&script, &output));
        let refs: Vec<&str> = arguments.iter().map(String::as_str).collect();
        let result = faris(&refs);
        assert_eq!(code(&result), 5, "{}", text(&result.stderr));
        assert!(
            text(&result.stderr).contains("memory"),
            "{}",
            text(&result.stderr)
        );
    }

    // Verifies: AUTO-004
    #[test]
    fn a_tool_that_exits_non_zero_is_a_solver_failure() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fails.sh");
        std::fs::write(&script, "#!/bin/sh\nexit 7\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = dir.path().join("evidence");
        let arguments = control_args(&script, &output);
        let refs: Vec<&str> = arguments.iter().map(String::as_str).collect();
        let result = faris(&refs);
        assert_eq!(code(&result), 3, "{}", text(&result.stderr));
    }

    // Verifies: AUTO-066
    #[test]
    fn adapters_list_reports_path_hash_version_and_health() {
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("openmc");
        std::fs::write(&tool, "#!/bin/sh\necho 'fake-openmc 9.9.9'\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let broken = dir.path().join("actinv");
        std::fs::write(&broken, "#!/bin/sh\nexit 3\n").unwrap();
        std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o755)).unwrap();

        let run = |extra: &[&str]| {
            let mut arguments = vec!["adapters", "list"];
            arguments.extend_from_slice(extra);
            let output = Command::new(env!("CARGO_BIN_EXE_faris"))
                .args(arguments)
                .env("PATH", dir.path())
                .output()
                .unwrap();
            assert_eq!(code(&output), 0, "{}", text(&output.stderr));
            serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
        };
        let started = Instant::now();
        let listing = run(&[]);
        assert!(started.elapsed() < Duration::from_secs(15));
        let tools = listing["tools"].as_array().unwrap();
        let find = |name: &str| tools.iter().find(|t| t["name"] == name).unwrap();
        let openmc = find("openmc");
        assert_eq!(openmc["version"], "fake-openmc 9.9.9");
        assert_eq!(openmc["health"], "OK");
        let expected = {
            use sha2::{Digest, Sha256};
            format!("{:x}", Sha256::digest(std::fs::read(&tool).unwrap()))
        };
        assert_eq!(openmc["sha256"], expected.as_str());
        assert_eq!(find("actinv")["health"], "FAILED");
        assert!(
            find("actinv")["health_detail"]
                .as_str()
                .unwrap()
                .contains("by hand")
        );
        assert_eq!(find("avila-core")["health"], "NOT_FOUND");
        assert!(find("avila-core")["next_step"].as_str().is_some());
        assert_eq!(listing["bundled_adapters"].as_array().unwrap().len(), 2);

        let unprobed = run(&["--no-probe"]);
        let openmc = unprobed["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "openmc")
            .unwrap();
        assert_eq!(openmc["health"], "NOT_EVALUATED");
        assert!(openmc["version"].is_null());
        assert_eq!(openmc["sha256"], expected.as_str());
    }
}
