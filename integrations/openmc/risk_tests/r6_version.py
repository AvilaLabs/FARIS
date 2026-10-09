#!/usr/bin/env python3
"""R6 (docs/notes/CAD_TRANSPORT_RISK_TESTS.md): choose the OpenMC version for R1-R3.

Runs the R1 generation step and one analog run on RM-M with OpenMC 0.15.3 and
0.16.0, applies the protocol's rule and writes the choice with its reason.
This driver uses only the standard library; the OpenMC work is done by
r6_worker.py in each environment, every job under a 6 GB memory cap with no
swap and at most four threads, one job at a time:

    r6_version.py --model-dir DIR --work-dir DIR --out r6.json

A job that outlives its timeout is stopped by its own process group and the
timeout is recorded; nothing is retried.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
import rm_m_checks as checks  # noqa: E402

HERE = Path(__file__).resolve().parent
HOME = Path.home()
ENVIRONMENTS = {
    "0.15.3": HOME / ".venvs/w003env/bin/python",
    "0.16.0": HOME / "micromamba/envs/openmc016/bin/python",
}
GUARD = ["systemd-run", "--user", "--scope", "--quiet", "-p", "MemoryMax=6G", "-p", "MemorySwapMax=0", "--"]
PYLIB = HOME / ".cache/avila-night/cad-risk-tests/pylib"  # openmc_plasma_source 0.9.0 and NeSST, pure Python, outside the protected envs


def parse_time_file(path: Path) -> dict:
    """Wall seconds and peak RSS (MB) of the largest process, from `/usr/bin/time -v`."""
    out = {"wall_seconds": None, "peak_rss_mb": None, "exit_status": None}
    if not path.is_file():
        return out
    text = path.read_text(encoding="utf-8", errors="replace")
    m = re.search(r"Elapsed \(wall clock\) time \(h:mm:ss or m:ss\): ([\d:.]+)", text)
    if m:
        seconds = 0.0
        for part in m.group(1).split(":"):
            seconds = seconds * 60.0 + float(part)
        out["wall_seconds"] = seconds
    m = re.search(r"Maximum resident set size \(kbytes\): (\d+)", text)
    if m:
        out["peak_rss_mb"] = int(m.group(1)) / 1024.0
    m = re.search(r"Exit status: (\d+)", text)
    if m:
        out["exit_status"] = int(m.group(1))
    return out


def run_guarded(command: list[str], log: Path, timeout_s: float, threads: int = 4) -> dict:
    """Run a command under the memory cap with a timeout; stop it by process group if it overruns."""
    log.parent.mkdir(parents=True, exist_ok=True)
    time_file = log.with_suffix(".time")
    time_file.unlink(missing_ok=True)
    env = dict(os.environ, OMP_NUM_THREADS=str(threads),
               PYTHONPATH=str(PYLIB) + (os.pathsep + os.environ["PYTHONPATH"] if os.environ.get("PYTHONPATH") else ""))
    full = GUARD + ["/usr/bin/time", "-v", "-o", str(time_file)] + [str(c) for c in command]
    t0 = time.time()
    timed_out = False
    with log.open("w", encoding="utf-8") as stream:
        proc = subprocess.Popen(full, stdout=stream, stderr=subprocess.STDOUT, env=env, start_new_session=True)
        try:
            returncode = proc.wait(timeout=timeout_s)
        except subprocess.TimeoutExpired:
            timed_out = True
            os.killpg(proc.pid, signal.SIGTERM)
            try:
                returncode = proc.wait(timeout=30)
            except subprocess.TimeoutExpired:
                os.killpg(proc.pid, signal.SIGKILL)
                returncode = proc.wait()
    result = parse_time_file(time_file)
    result.update({"returncode": returncode, "timed_out": timed_out, "timeout_seconds": timeout_s,
                   "driver_wall_seconds": time.time() - t0, "log": str(log)})
    return result


def load_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8")) if path.is_file() else None


def step_map(generate: dict | None, analog: dict | None) -> dict:
    """Merge the step records of both workers into {step: {ok, workarounds}}."""
    steps = {}
    for record in (generate, analog):
        if record:
            for name, entry in record["steps"].items():
                steps[name] = {"ok": bool(entry["ok"]), "workarounds": list(entry["workarounds"]),
                               "seconds": entry["seconds"], "error": entry["error"]}
    return steps


def flux_of(analog: dict | None):
    try:
        f = analog["analog_run"]["tf_fast_flux"]
        return f["mean"], f["std_error"]
    except (TypeError, KeyError):
        return None


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--model-dir", required=True, type=Path)
    ap.add_argument("--work-dir", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--versions", default="0.15.3,0.16.0")
    ap.add_argument("--threads", type=int, default=4)
    ap.add_argument("--analog-histories", type=int, default=1_000_000)
    ap.add_argument("--analog-batches", type=int, default=100)
    ap.add_argument("--generate-timeout", type=float, default=10800.0)
    ap.add_argument("--analog-timeout", type=float, default=5400.0)
    ap.add_argument("--rays", type=int)
    ap.add_argument("--batches", type=int)
    ap.add_argument("--inactive", type=int)
    ap.add_argument("--groups", type=int)
    ap.add_argument("--skip", default="", help="comma list of 'version:step' to skip (reuse existing results), step is generate or analog")
    args = ap.parse_args()
    skip = set(filter(None, args.skip.split(",")))

    results = {"schema": "faris-r6-version/v1", "protocol": "docs/notes/CAD_TRANSPORT_RISK_TESTS.md#r6-openmc-version",
               "model_dir": str(args.model_dir), "versions": {}}
    for version in args.versions.split(","):
        python = ENVIRONMENTS[version]
        base = args.work_dir / f"openmc-{version}"
        entry = {"python": str(python), "jobs": {}}
        results["versions"][version] = entry
        gen_out, ana_out = base / "generate.json", base / "analog.json"
        if f"{version}:generate" not in skip:
            cmd = [python, HERE / "r6_worker.py", "generate", "--model-dir", args.model_dir, "--run-dir", base / "generate",
                   "--out", gen_out, "--threads", args.threads]
            for flag, value in (("--rays", args.rays), ("--batches", args.batches), ("--inactive", args.inactive), ("--groups", args.groups)):
                if value is not None:
                    cmd += [flag, value]
            entry["jobs"]["generate"] = run_guarded(cmd, base / "generate.log", args.generate_timeout, args.threads)
        if f"{version}:analog" not in skip:
            cmd = [python, HERE / "r6_worker.py", "analog", "--model-dir", args.model_dir, "--run-dir", base / "analog",
                   "--out", ana_out, "--threads", args.threads, "--histories", args.analog_histories, "--batches", args.analog_batches]
            entry["jobs"]["analog"] = run_guarded(cmd, base / "analog.log", args.analog_timeout, args.threads)
        generate, analog = load_json(gen_out), load_json(ana_out)
        entry["steps"] = step_map(generate, analog)
        entry["generate_record"] = generate
        entry["analog_record"] = analog
        entry["tf_fast_flux"] = flux_of(analog)

    a, b = results["versions"].get("0.15.3"), results["versions"].get("0.16.0")
    if a and b and a["tf_fast_flux"] and b["tf_fast_flux"]:
        cmp_out = args.work_dir / "windows_compare.json"
        wa = (a["generate_record"] or {}).get("weight_windows_file")
        wb = (b["generate_record"] or {}).get("weight_windows_file")
        if wa and wb:
            cmp_job = run_guarded([ENVIRONMENTS["0.15.3"], HERE / "r6_worker.py", "compare-windows", wa, wb, "--out", cmp_out],
                                  args.work_dir / "windows_compare.log", 600.0, 1)
            results["window_comparison"] = {"job": cmp_job, "result": load_json(cmp_out)}
        results["rule"] = checks.r6_choice(a["steps"], b["steps"], tuple(a["tf_fast_flux"]), tuple(b["tf_fast_flux"]))
    else:
        results["rule"] = {"choice": "0.15.3", "reasons": ["rule not evaluable: an analog TF fast flux is missing on at least one version; "
                                                         "0.15.3 is kept by default"], "rule1_steps_clean": None, "rule2_flux_agrees": None}
    results["choice"] = results["rule"]["choice"]
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(results, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")
    print(json.dumps({"choice": results["choice"], "reasons": results["rule"]["reasons"]}, indent=1))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
