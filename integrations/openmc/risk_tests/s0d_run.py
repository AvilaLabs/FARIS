#!/usr/bin/env python3
"""S0d driver: T1, then T3 if the rule asks for it, then T2, inside the 60 min box; then the rules.

    s0d_run.py --scratch ~/.cache/avila-night/cad-risk-tests/s0d

Generation runs on OpenMC 0.16.0 (s0d_generate.py), production on 0.15.3 with double-down (s0b_run.py), each under the
6 GB cap on cores 0-3. Before each item the remaining box is compared with the item's estimate; an item that does not fit
is recorded NOT_EVALUATED with the estimate. Protocol: docs/notes/CAD_TRANSPORT_S0D.md.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import time

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import s0b_common as C  # noqa: E402
import s0d_common as S  # noqa: E402

HOME = Path.home()
PY016 = HOME / "micromamba/envs/openmc016/bin/python"
PY0153 = HOME / "micromamba/envs/openmc0153dd/bin/python"
MODEL = HOME / ".cache/avila-night/cad-risk-tests/models/rm-m"
S0B = HOME / ".cache/avila-night/cad-risk-tests/s0b"
G6_MGXS = S0B / "d3/G6/generate/mgxs.h5"
D2 = S0B / "d2/analog.json"
PYLIB = HOME / ".cache/avila-night/cad-risk-tests/pylib"
CAP = ["systemd-run", "--user", "--scope", "--quiet", "-p", "MemoryMax=6G", "-p", "MemorySwapMax=0", "--",
       "taskset", "-c", "0-3"]
MODES = {"T1": "targeted", "T2": "cadis", "T3": "global"}
ESTIMATE_S = {"T1": 1380.0, "T3": 1380.0, "T2": 1020.0}  # protocol: generation + 600 s production


def job(argv: list[str], log: Path, timeout_s: float) -> dict:
    env = dict(os.environ, PYTHONPATH=str(PYLIB))  # systemd-run --user needs the session bus variables
    t0 = time.time()
    load0 = C.read_loadavg()
    with open(log, "w", encoding="utf-8") as out:
        try:
            rc = subprocess.run(CAP + argv, stdout=out, stderr=subprocess.STDOUT, env=env, timeout=timeout_s).returncode
        except subprocess.TimeoutExpired:
            rc = "timeout"
    return {"returncode": rc, "wall_s": time.time() - t0, "loadavg_start": load0, "loadavg_end": C.read_loadavg()}


def item(label: str, scratch: Path, ledger: dict) -> dict:
    d = scratch / label.lower()
    d.mkdir(parents=True, exist_ok=True)
    gen = job([str(PY016), str(HERE / "s0d_generate.py"), "--mode", MODES[label], "--mgxs", str(G6_MGXS),
               "--model-dir", str(MODEL), "--run-dir", str(d / "gen"), "--out", str(d / "gen.json"),
               "--seed", str(S.SEEDS["gen"][label])], d / "gen.log", 2400)
    ledger["entries"].append({"item": label, "step": "generate", **gen})
    out = {"label": label, "mode": MODES[label], "generate": gen}
    windows = d / "gen/weight_windows.h5"
    if gen["returncode"] != 0 or not windows.is_file():
        out["note"] = "NOT_EVALUATED: generation failed; see gen.log and gen.json"
        return out
    prod = job([str(PY0153), str(HERE / "s0b_run.py"), "run", "--model-dir", str(MODEL), "--run-dir", str(d / "produce"),
                "--out", str(d / "produce.json"), "--label", f"S0d-{label}", "--seed", str(S.SEEDS["prod"][label]),
                "--budget", str(S.PRODUCTION_S), "--windows", str(windows), "--particles", "10000",
                "--statepoint-every", "5", "--poll", "20", "--max-split", str(S.MAX_SPLIT), "--checkpoints"],
               d / "produce.log", S.PRODUCTION_S + 600)
    ledger["entries"].append({"item": label, "step": "produce", **prod})
    out["produce"] = prod
    out.update(score(d / "produce.json"))
    return out


def score(path: Path) -> dict:
    if not path.is_file():
        return {"note": "NOT_EVALUATED: no production record", "gain": None, "sigma": None, "unbiased": None}
    run = json.loads(path.read_text(encoding="utf-8"))
    ref = json.loads(D2.read_text(encoding="utf-8"))
    f, fr = run.get("fom_a") or {}, ref["fom_a"]
    if not f or f.get("fom") is None:
        return {"note": "NOT_EVALUATED: no FOM", "gain": None, "sigma": None, "unbiased": None}
    g = C.gain_with_sigma(f, fr)
    a, ar = run["response"]["a"], ref["response"]["a"]
    z = C.z_score(a["mean"], a["std_error"], ar["mean"], ar["std_error"])
    return {"fom": f, "gain": g["gain"], "sigma": g["sigma"], "z": z, "unbiased": C.unbiased(z),
            "R_a": a["relative_error"], "histories": run["response"].get("histories"),
            "histories_per_second": run["response"].get("histories_per_second"), "lost": run.get("lost")}


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--scratch", required=True, type=Path)
    args = ap.parse_args(argv)
    scratch = args.scratch
    scratch.mkdir(parents=True, exist_ok=True)
    ledger = {"start_epoch": time.time(), "box_s": S.TIME_BOX_S, "entries": []}
    rows: dict[str, dict] = {}

    def used() -> float:
        return sum(e["wall_s"] for e in ledger["entries"])

    def fits(label: str) -> bool:
        return used() + ESTIMATE_S[label] <= S.TIME_BOX_S

    def save():
        result = {"rows": rows, "ledger": ledger, "job_wall_s": used(),
                  "rule": S.h4_rule({k: v for k, v in rows.items() if v.get("gain") is not None or k in ("T1", "T2")})}
        (scratch / "results-s0d.json").write_text(json.dumps(result, indent=1, sort_keys=True, default=str) + "\n",
                                                  encoding="utf-8")
        return result

    for label in ("T1", "T3", "T2"):
        if label == "T3" and not S.t3_required(rows.get("T1")):
            rows["T3"] = {"label": "T3", "note": "not run: T1's gain is within 3 sigma of G6's (rule)", "gain": None}
            save()
            continue
        if not fits(label):
            rows[label] = {"label": label, "gain": None, "unbiased": None,
                           "note": f"NOT_EVALUATED: does not fit the box ({used():.0f} s used, estimate {ESTIMATE_S[label]:.0f} s)"}
            save()
            continue
        rows[label] = item(label, scratch, ledger)
        save()
    result = save()
    print(json.dumps(result["rule"], indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
