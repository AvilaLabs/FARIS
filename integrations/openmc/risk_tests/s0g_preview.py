#!/usr/bin/env python3
"""R2' (PERF-025): the 60 s laptop preview on RM-M from a finished model file (docs/notes/CAD_TRANSPORT_RISK_TESTS.md, "R2'").

    s0g_preview.py --model MODEL.xml --openmc /path/to/bin/openmc --run-dir R --out result.json --seed N [--threads 4]

R2's measurement and rule (r2_preview.py), on a model that already holds its windows and tallies. The FARIS patch
variables are taken from the environment as they are and recorded. The openmc executable is stopped by process group
60 s after its process start, so reading the model, cross sections and windows counts. Batches are 20,000 histories and
every batch writes a statepoint; the last complete one gives TBR and response (a).
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
import s0f_common as F  # noqa: E402

TBR_LIMIT, RESPONSE_A_LIMIT, BUDGET_S, BATCH, MAX_BATCHES = 0.02, 0.20, 60.0, 20000, 400


def rule(tbr_r, a_r) -> dict:
    ok_t = tbr_r is not None and tbr_r <= TBR_LIMIT
    ok_a = a_r is not None and a_r <= RESPONSE_A_LIMIT
    return {"tbr_ok": ok_t, "a_ok": ok_a, "verdict": "PASS" if ok_t and ok_a else "FAIL",
            "rule": "TBR R <= 2 % and response (a) R <= 20 % at 60 s (1 sigma)"}


def write_model(base: Path, run_dir: Path, seed: int) -> Path:
    x = base.read_text()
    x = re.sub(r"<particles>\d+</particles>", f"<particles>{BATCH}</particles>", x, count=1)
    x = re.sub(r"<batches>\d+</batches>", f"<batches>{MAX_BATCHES}</batches>", x, count=1)
    x = re.sub(r"<seed>\d+</seed>", f"<seed>{seed}</seed>", x, count=1)
    every = " ".join(str(b) for b in range(1, MAX_BATCHES + 1))
    x = re.sub(r"<state_point>.*?</state_point>", f"<state_point><batches>{every}</batches></state_point>", x, count=1,
               flags=re.S)
    run_dir.mkdir(parents=True, exist_ok=True)
    out = run_dir / "model.xml"
    out.write_text(x)
    return out


def last_complete_statepoint(run_dir: Path, started: float, budget: float):
    """The newest statepoint written inside the budget that opens cleanly: (path, batches, seconds after start)."""
    import openmc
    found = sorted(run_dir.glob("statepoint.*.h5"), key=lambda p: int(p.name.split(".")[1]), reverse=True)
    for p in found:
        age = p.stat().st_mtime - started
        if age > budget:
            continue
        try:
            with openmc.StatePoint(str(p)) as sp:
                sp.n_realizations
            return p, int(p.name.split(".")[1]), age
        except Exception:
            continue
    return None, 0, None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", required=True, type=Path)
    ap.add_argument("--openmc", required=True, type=Path)
    ap.add_argument("--run-dir", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--seed", required=True, type=int)
    ap.add_argument("--threads", type=int, default=4)
    args = ap.parse_args()
    import openmc
    write_model(args.model, args.run_dir, args.seed)
    env = dict(os.environ, OMP_NUM_THREADS=str(args.threads))
    log = (args.run_dir / "openmc.log").open("wb")
    load0 = Path("/proc/loadavg").read_text().strip()
    started = time.time()
    proc = subprocess.Popen([str(args.openmc), "-s", str(args.threads), "model.xml"], cwd=args.run_dir, env=env,
                            stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    try:
        proc.wait(timeout=BUDGET_S)
        finished = True
    except subprocess.TimeoutExpired:
        finished = False
        os.killpg(proc.pid, signal.SIGKILL)
        proc.wait()
    log.close()
    record = {"kind": "r2-preview-model", "openmc_version": openmc.__version__, "executable": str(args.openmc),
              "threads": args.threads, "budget_s": BUDGET_S, "batch": BATCH, "seed": args.seed, "model": str(args.model),
              "env": {k: os.environ[k] for k in F.PATCH_VARIABLES if k in os.environ}, "finished_before_budget": finished,
              "loadavg_start": load0, "loadavg_end": Path("/proc/loadavg").read_text().strip()}
    sp_path, batches, age = last_complete_statepoint(args.run_dir, started, BUDGET_S)
    tbr_r = a_r = None
    if sp_path is not None:
        with openmc.StatePoint(str(sp_path)) as sp:
            t, a = sp.get_tally(name="tbr"), sp.get_tally(name="tf_fast_a")
            tm, ts, am, asd = float(t.mean.sum()), float(t.std_dev.sum()), float(a.mean.sum()), float(a.std_dev.sum())
        tbr_r, a_r = (ts / tm if tm else None), (asd / am if am else None)
        record.update({"batches": batches, "histories": batches * BATCH, "statepoint_s": age, "tbr": tm, "tbr_r": tbr_r,
                       "response_a": am, "response_a_r": a_r})
    record.update(rule(tbr_r, a_r))
    args.out.write_text(json.dumps(record, indent=1))
    print(json.dumps({k: record.get(k) for k in ("histories", "statepoint_s", "tbr", "tbr_r", "response_a",
                                                  "response_a_r", "verdict")}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
