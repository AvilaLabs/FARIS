#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Execute native FARIS transport, history-rate binding, and ACTINV-coupled maintenance."""
from __future__ import annotations
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import select
import signal
import subprocess
import sys
import time
from typing import Any

HERE = Path(__file__).resolve().parent
CASE_DIR = HERE / "cases" / "controlled_fixed" / "faris"
CASE = json.loads((CASE_DIR / "case.json").read_text(encoding="utf-8"))
REPO_ROOT = HERE.parents[1]


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for b in iter(lambda: f.read(1024 * 1024), b""):
            h.update(b)
    return h.hexdigest()


def verify_case_files() -> dict[str, Path]:
    out = {}
    for name, item in CASE["files"].items():
        path = (CASE_DIR / item["path"]).resolve()
        if not path.is_relative_to(CASE_DIR.resolve()) or not path.is_file():
            raise ValueError(f"FARIS {name} input is missing or outside its case folder")
        if sha256(path) != item["sha256"]:
            raise ValueError(f"FARIS {name} input SHA-256 mismatch")
        out[name] = path
    return out


def bounded_cli(argv: list[str], cwd: Path, timeout: int, output_limit: int) -> dict[str, Any]:
    """Bound wall time and captured logs without limiting FARIS-created artifacts."""
    p = subprocess.Popen(argv, cwd=cwd, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                         stderr=subprocess.PIPE, shell=False, start_new_session=(os.name == "posix"))
    assert p.stdout and p.stderr
    outfd, errfd = p.stdout.fileno(), p.stderr.fileno()
    buffers = {outfd: bytearray(), errfd: bytearray()}
    streams = {outfd: p.stdout, errfd: p.stderr}
    for fd in streams:
        os.set_blocking(fd, False)
    deadline = time.monotonic() + timeout

    def stop():
        if os.name == "posix":
            try:
                os.killpg(p.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        elif p.poll() is None:
            p.terminate()
        if p.poll() is None:
            try:
                p.wait(timeout=2)
            except subprocess.TimeoutExpired:
                if os.name == "posix":
                    try:
                        os.killpg(p.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                else:
                    p.kill()
                p.wait()

    try:
        while streams:
            remain = deadline - time.monotonic()
            if remain <= 0:
                stop()
                raise subprocess.TimeoutExpired(argv, timeout)
            readable, _, _ = select.select(list(streams), [], [], min(remain, 0.2))
            for fd in readable:
                try:
                    block = os.read(fd, 8192)
                except BlockingIOError:
                    continue
                if not block:
                    streams[fd].close()
                    del streams[fd]
                    continue
                buffers[fd].extend(block)
                if sum(map(len, buffers.values())) > output_limit:
                    stop()
                    raise ValueError("FARIS CLI stdout/stderr exceeded the configured log bound")
        code = p.wait(timeout=max(0.1, deadline - time.monotonic()))
    except BaseException:
        stop()
        for stream in streams.values():
            stream.close()
        raise
    p.stdout.close()
    p.stderr.close()
    return {"argv": argv, "return_code": code,
            "stdout": bytes(buffers[outfd])[-4000:].decode("utf-8", "replace"),
            "stderr": bytes(buffers[errfd])[-4000:].decode("utf-8", "replace")}


def run_stage(name: str, argv: list[str], cwd: Path, timeout: int, log_limit: int,
              record_file: Path) -> dict[str, Any]:
    start = time.monotonic()
    result = bounded_cli(argv, cwd, timeout, log_limit)
    result.update({"stage": name, "wall_seconds": round(time.monotonic() - start, 6)})
    record_file.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    if result["return_code"] != 0:
        raise RuntimeError(f"{name} failed ({result['return_code']}): {result['stderr'][-1200:]}")
    return result


def make_history(rates_path: Path, template_path: Path, output_path: Path) -> dict[str, Any]:
    rates = json.loads(rates_path.read_text(encoding="utf-8"))
    item = rates.get("component_average_flux_n_m2_s", {}).get("blanket")
    rate = item.get("mean") if isinstance(item, dict) else None
    if isinstance(rate, bool) or not isinstance(rate, (int, float)) or not math.isfinite(rate) or rate <= 0:
        raise ValueError("native TransportDrivingRates has no finite positive blanket flux")
    lifetime = CASE["matched_case"]["blanket_lifetime_full_power_seconds"]
    limit = float(rate) * lifetime
    history = json.loads(template_path.read_text(encoding="utf-8"))
    provenance = (
        "Authored verification threshold from native FARIS blanket flux rate "
        f"{float(rate):.17g} neutrons/m2/s (rates SHA-256 {sha256(rates_path)}) times "
        f"{lifetime:.17g} s. It controls this test interval; it is not a material limit, "
        "measurement, or validated service-life model."
    )
    history["service_limits"] = [{
        "component_id": "blanket", "class": "replaceable", "response_id": "blanket-flux",
        "metric": "energy_integrated_component_average_neutron_flux", "unit": "neutrons/m²",
        "limit": limit,
        "replacement_duration_s": CASE["matched_case"]["fixed_blanket_replacement_seconds"],
        "provenance": provenance,
    }]
    output_path.write_text(json.dumps(history, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return {"rate_n_m2_s": float(rate), "integrated_flux_limit_n_m2": limit,
            "operating_interval_s": lifetime, "rates_sha256": sha256(rates_path),
            "history_sha256": sha256(output_path), "provenance": provenance}


def native_check(result_path: Path) -> dict[str, Any]:
    result = json.loads(result_path.read_text(encoding="utf-8"))
    if result.get("schema_version") != "faris-maintenance-result/v0.1":
        raise ValueError("expected native faris-maintenance-result/v0.1")
    design = result.get("designs", {}).get(CASE["case_id"])
    if not isinstance(design, dict):
        raise ValueError("native maintenance result omitted the control design")
    fixed, computed = design["fixed"], design["computed"]
    summary = computed.get("summary")
    horizon = CASE["matched_case"]["horizon_s"]
    fixed_count = CASE["matched_case"]["expected_fixed_blanket_replacement_count"]
    fixed_duration = CASE["matched_case"]["fixed_blanket_replacement_seconds"]
    fixed_events = [e for e in fixed.get("replacements", []) if e.get("class") == "blanket"]
    computed_events = [e for e in (summary or {}).get("replacements", []) if e.get("class") == "blanket"]
    thresholds = design.get("thresholds", {})
    checks = {
        "fixed_history_horizon_matches_peer_case": math.isclose(float(fixed.get("history_end_s", -1)), horizon, rel_tol=0, abs_tol=1e-6),
        "fixed_history_completed": fixed.get("outcome") == "horizon_completed",
        "fixed_blanket_event_count_matches_one_boundary_replacement": len(fixed_events) == fixed_count,
        "fixed_event_duration_is_60_days": all(e.get("end_s") is not None and math.isclose(e["end_s"] - e["start_s"], fixed_duration, rel_tol=0, abs_tol=1e-6) for e in fixed_events),
        "computed_history_evaluated": computed.get("status") == "EVALUATED" and summary is not None,
        "computed_history_horizon_matches_peer_case": summary is not None and math.isclose(float(summary.get("history_end_s", -1)), horizon, rel_tol=0, abs_tol=1e-6),
        "q_star_calibration_evaluated": any(v.get("source") == "calibrated" and v.get("status") == "EVALUATED" for v in thresholds.values()),
    }
    return {
        "status": "native FARIS fixed/computed case completed" if all(checks.values()) else "incomplete; inspect native outputs and checks",
        "checks": checks, "fixed_summary": fixed, "computed_summary": summary,
        "computed_status": computed.get("status"), "computed_not_evaluated": computed.get("not_evaluated"),
        "fixed_blanket_events": fixed_events, "computed_blanket_events": computed_events,
        "thresholds": thresholds, "decay_source": result.get("decay_source"),
        "inputs": result.get("inputs"),
        "faris_native_energy_kept_separate": {
            "fixed_lifetime_net_electricity_mwh": fixed.get("lifetime_net_electricity_mwh"),
            "computed_lifetime_net_electricity_mwh": (summary or {}).get("lifetime_net_electricity_mwh"),
            "note": "FARIS-native energy under authored FARIS assumptions; kept separate from shared peer MW(e) normalization.",
        },
    }


def run(faris: str, openmc_python: str, openmc: str, cross_sections: Path, audit: Path,
        actinv: str, data_dir: Path, out: Path, batches: int, particles: int, threads: int) -> dict[str, Any]:
    files = verify_case_files()
    if not cross_sections.is_file() or not audit.is_file() or not data_dir.is_dir():
        raise ValueError("audit, cross_sections.xml, and ACTINV data directory must exist")
    if out.exists() and any(out.iterdir()):
        raise ValueError(f"output directory is not empty: {out}")
    out.mkdir(parents=True, exist_ok=True)
    limits = CASE["limits"]
    records = {}
    transport = out / "transport"
    cmd = [faris, "reactor", "run", "--scenario", str(files["scenario"]),
        "--physics", str(files["physics"]), "--audit", str(audit.resolve()),
        "--cross-sections", str(cross_sections.resolve()), "--python", openmc_python,
        "--openmc", openmc, "--output", str(transport), "--batches", str(batches),
        "--particles", str(particles), "--seed", "20261109", "--threads", str(threads),
        "--timeout-seconds", str(limits["reactor_timeout_s"]), "--mesh-preset", "coarse",
        "--activation-spectra", "fispact-709"]
    records["transport"] = run_stage("transport", cmd, out, limits["reactor_timeout_s"] + 120,
        limits["stdout_stderr_limit_bytes"], out / "transport-cli.json")
    run_json = transport / "run.json"
    spectra = transport / "solver" / "transport-spectra.json"
    if not run_json.is_file() or not spectra.is_file():
        raise RuntimeError("reactor run omitted run.json or 709-group spectra")
    rates, rate_history = out / "transport-rates.json", out / "rates-only-history.json"
    records["rate_binding"] = run_stage("rate_binding", [faris, "history", "from-run",
        "--scenario", str(files["scenario"]), "--run", str(run_json),
        "--assumptions", str(files["history_template"]), "--output", str(rate_history),
        "--rates-output", str(rates)], out, limits["history_timeout_s"],
        limits["stdout_stderr_limit_bytes"], out / "rate-binding-cli.json")
    history = out / "operating-history.json"
    rate_map = make_history(rates, files["history_template"], history)
    designs = out / "maintenance-designs.json"
    designs.write_text(json.dumps({"schema_version": "faris-maintenance-designs/v0.1", "designs": {
        CASE["case_id"]: {"scenario": str(files["scenario"]), "physics": str(files["physics"]),
            "history_run": str(run_json), "spectrum_run": str(run_json), "history_assumptions": str(history)}
    }}, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    result_path, work_dir = out / "faris-maintenance-result.json", out / "maintenance-work"
    builder = REPO_ROOT / "scripts" / "build_activation_inputs.py"
    cmd = [faris, "maintenance", "run", "--designs", str(designs),
        "--assumptions", str(files["maintenance_assumptions"]), "--actinv", actinv,
        "--data-dir", str(data_dir.resolve()), "--output", str(result_path),
        "--work-dir", str(work_dir), "--workers", "1", "--python", sys.executable,
        "--builder", str(builder)]
    records["maintenance"] = run_stage("maintenance", cmd, out, limits["maintenance_timeout_s"],
        limits["stdout_stderr_limit_bytes"], out / "maintenance-cli.json")
    report = {
        "schema": "faris-native-maintenance-control-result/0.1.0",
        "case_id": CASE["case_id"], "scope": CASE["scope"],
        "case_manifest_sha256": sha256(CASE_DIR / "case.json"),
        "case_files_sha256": {k: sha256(v) for k, v in files.items()},
        "audit_sha256": sha256(audit), "cross_sections_sha256": sha256(cross_sections),
        "reactor_run_sha256": sha256(run_json), "transport_spectra_sha256": sha256(spectra),
        "rate_to_integrated_flux_mapping": rate_map,
        "native_result_path": str(result_path), "native_result_sha256": sha256(result_path),
        "native_faris": native_check(result_path), "cli_runs": records,
        "interpretation": "Native FARIS fixed and ACTINV-coupled histories share one authored scenario, physics, transport run, horizon, and integrated-flux trigger. Maintenance durations use the actual FARIS coupling path. This is code verification under authored assumptions, not physical validation. FARIS-native energy is separate from peer normalization.",
    }
    (out / "native-faris-comparison.json").write_text(
        json.dumps(report, indent=2, sort_keys=True, allow_nan=False) + "\n", encoding="utf-8")
    return report


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for name, help_text in [
        ("faris", "compiled FARIS executable"), ("openmc-python", "Python with OpenMC"),
        ("openmc", "OpenMC executable"), ("cross-sections", "cross_sections.xml"),
        ("audit", "read-only material/data audit"), ("actinv", "ACTINV executable"),
        ("data-dir", "ACTINV data root"),
    ]:
        parser.add_argument(f"--{name}", required=True, help=help_text,
                            type=Path if name in {"cross-sections", "audit", "data-dir"} else str)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--batches", type=int, default=100)
    parser.add_argument("--particles", type=int, default=10_000)
    parser.add_argument("--threads", type=int, default=1)
    args = parser.parse_args(argv)
    if not 30 <= args.batches <= 1000 or not 1 <= args.particles <= 1_000_000 or not 1 <= args.threads <= 32:
        parser.error("sampling settings are outside the FARIS reactor CLI bounds")
    try:
        report = run(args.faris, args.openmc_python, args.openmc,
            args.cross_sections.resolve(), args.audit.resolve(), args.actinv,
            args.data_dir.resolve(), args.out.resolve(), args.batches, args.particles, args.threads)
    except (OSError, ValueError, RuntimeError, json.JSONDecodeError, subprocess.TimeoutExpired) as error:
        print(f"FAIL {error}", file=sys.stderr)
        return 1
    print(json.dumps(report["native_faris"], indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
