#!/usr/bin/env python3
"""S0e driver: E-smoke, E0, E1, E2, E3 if the rules ask for it, inside the 75 min cap; then the rules.

    s0e_run.py --scratch ~/.cache/avila-night/cad-risk-tests/s0e

Generation runs on OpenMC 0.16.0 (s0e_generate.py), production on 0.15.3 with double-down (s0b_run.py), each under the 6 GB
cap on cores 0-3. A production (and the smoke load run) starts only on a quiet machine: 1-minute load below 2.0 and the
FARIS build lock free; the wait is recorded and does not count toward the cap. Before each item the remaining cap is compared
with the item's estimate; an item that does not fit is recorded NOT_EVALUATED with the machine time used so far.
Protocol: docs/notes/CAD_TRANSPORT_S0E.md.
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
import s0d_run as D  # noqa: E402
import s0e_common as S  # noqa: E402

PY016, PY0153, MODEL, G6_MGXS = D.PY016, D.PY0153, D.MODEL, D.G6_MGXS
S0D_DIR = Path.home() / ".cache/avila-night/cad-risk-tests/s0d"
POLL_S = 20.0
MAX_WAIT_S = 3 * 3600.0  # stop waiting for a quiet machine after this; the item is then NOT_EVALUATED


def lock_free(lock: Path = S.BUILD_LOCK) -> bool:
    """True when no cargo build holds the FARIS build lock (flock -n on the lock file)."""
    if not lock.parent.is_dir():
        return True
    return subprocess.run(["flock", "-n", str(lock), "true"], capture_output=True).returncode == 0


def wait_quiet(max_wait_s: float = MAX_WAIT_S, poll_s: float = POLL_S) -> dict:
    """Block until the machine is quiet (S.quiet_now); return {waited_s, quiet, loadavg}."""
    t0 = time.time()
    while True:
        text = C.read_loadavg()
        if S.quiet_now(S.load1(text), lock_free()):
            return {"waited_s": time.time() - t0, "quiet": True, "loadavg": text}
        if time.time() - t0 >= max_wait_s:
            return {"waited_s": time.time() - t0, "quiet": False, "loadavg": text}
        time.sleep(poll_s)


def scope_name(log: Path) -> str:
    """A unique scope name for one job, so a timed-out job can be stopped as a whole."""
    stem = "".join(c if c.isalnum() else "-" for c in f"{log.parent.name}-{log.stem}")
    return f"faris-s0e-{stem}-{os.getpid()}-{time.time_ns()}"


def scoped(cap: list[str], unit: str) -> list[str]:
    """The memory-capped systemd-run prefix with a named scope."""
    i = cap.index("--scope")
    return cap[: i + 1] + [f"--unit={unit}"] + cap[i + 1 :]


def job(argv: list[str], log: Path, timeout_s: float, wait: bool = False, pylib: bool = True) -> dict:
    """One capped job. With wait=True the job starts only on a quiet machine (wait time recorded, not counted).

    pylib=False leaves the OpenMC site directory off PYTHONPATH (the CAD environment of the UX-079 baseline)."""
    quiet = wait_quiet() if wait else None
    if quiet is not None and not quiet["quiet"]:
        return {"returncode": "never_quiet", "wall_s": 0.0, "wait": quiet, "loadavg_start": quiet["loadavg"],
                "loadavg_end": quiet["loadavg"], "contended": False}
    env = dict(os.environ)  # systemd-run --user needs the session bus variables
    env.pop("PYTHONPATH", None)
    if pylib:
        env["PYTHONPATH"] = str(D.PYLIB)
    t0 = time.time()
    load0 = C.read_loadavg()
    unit = scope_name(log)
    with open(log, "w", encoding="utf-8") as out:
        try:
            rc = subprocess.run(scoped(D.CAP, unit) + argv, stdout=out, stderr=subprocess.STDOUT, env=env,
                                timeout=timeout_s).returncode
        except subprocess.TimeoutExpired:
            # killing systemd-run's client leaves the scope (and OpenMC in it) running: stop the whole scope
            subprocess.run(["systemctl", "--user", "stop", f"{unit}.scope"], check=False, capture_output=True)
            rc = "timeout"
    end = C.read_loadavg()
    return {"returncode": rc, "wall_s": time.time() - t0, "wait": quiet, "loadavg_start": load0, "loadavg_end": end,
            "contended": S.contended(end)}


def production_argv(label: str, d: Path, windows: Path, cutoff: bool) -> list[str]:
    argv = [str(PY0153), str(HERE / "s0b_run.py"), "run", "--model-dir", str(MODEL), "--run-dir", str(d / "produce"),
            "--out", str(d / "produce.json"), "--label", f"S0e-{label}", "--seed", str(S.SEEDS["prod"][label]),
            "--budget", str(S.PRODUCTION_S), "--windows", str(windows), "--particles", "10000",
            "--statepoint-every", "5", "--poll", "20", "--max-split", str(S.MAX_SPLIT), "--checkpoints", "--breakdown"]
    if cutoff:
        argv += ["--cutoff-ev", f"{S.CUTOFF_EV:g}"]
    return argv


def generation_argv(seed: int, d: Path, smoke: bool) -> list[str]:
    argv = [str(PY016), str(HERE / "s0e_generate.py"), "--mgxs", str(G6_MGXS), "--model-dir", str(MODEL),
            "--run-dir", str(d / "gen"), "--out", str(d / "gen.json"), "--seed", str(seed)]
    return argv + (["--smoke"] if smoke else [])


class Budget:
    """Machine-time ledger: job wall time only, against the cap."""

    def __init__(self, cap_s: float = S.CAP_S):
        self.cap_s = cap_s
        self.entries: list[dict] = []

    def used(self) -> float:
        return sum(e["wall_s"] for e in self.entries)

    def remaining(self) -> float:
        return self.cap_s - self.used()

    def timeout(self, wanted_s: float) -> float:
        """A job's timeout, never beyond what is left of the cap (at least a minute)."""
        return max(60.0, min(wanted_s, self.remaining()))

    def add(self, item: str, step: str, rec: dict) -> None:
        self.entries.append({"item": item, "step": step, **rec})


def not_evaluated(label: str, budget: Budget, why: str) -> dict:
    return {"label": label, "gain": None, "sigma": None, "unbiased": None,
            "note": f"NOT_EVALUATED: {why} ({budget.used():.0f} s of {budget.cap_s:.0f} s used)"}


def fit_or_skip(label: str, key: str, budget: Budget) -> dict | None:
    if S.fits(budget.used(), S.ESTIMATE_S[key], budget.cap_s):
        return None
    return not_evaluated(label, budget, f"does not fit the cap (estimate {S.ESTIMATE_S[key]:.0f} s)")


def smoke(scratch: Path, budget: Budget) -> dict:
    """Run 0: two-batch cylindrical generation, mesh check of the window file, one-batch 0.15.3 load. Passes if all hold."""
    d = scratch / "smoke"
    d.mkdir(parents=True, exist_ok=True)
    out: dict = {"label": "E-smoke", "passed": False}
    gen = job(generation_argv(S.SEEDS["gen"]["smoke"], d, True), d / "gen.log", budget.timeout(900))
    budget.add("E-smoke", "generate", gen)
    out["generate"] = gen
    windows = d / "gen/weight_windows.h5"
    if gen["returncode"] != 0 or not windows.is_file():
        out["note"] = "generation failed or wrote no window file; see gen.log and gen.json"
        return out
    check = job([str(PY0153), str(HERE / "s0e_check_windows.py"), str(windows), "--out", str(d / "check.json")],
                d / "check.log", budget.timeout(300))
    budget.add("E-smoke", "check_windows", check)
    out["check"] = check
    out["check_result"] = json.loads((d / "check.json").read_text()) if (d / "check.json").is_file() else None
    load = job([str(PY0153), str(HERE / "s0b_run.py"), "run", "--model-dir", str(MODEL), "--run-dir", str(d / "load"),
                "--out", str(d / "load.json"), "--label", "S0e-smoke-load", "--seed", str(S.SEEDS["gen"]["smoke"]),
                "--histories", "10000", "--particles", "10000", "--budget", "600", "--windows", str(windows),
                "--max-split", str(S.MAX_SPLIT), "--checkpoints", "--breakdown"],
               d / "load.log", budget.timeout(900), wait=True)
    budget.add("E-smoke", "load", load)
    out["load"] = load
    rec = json.loads((d / "load.json").read_text()) if (d / "load.json").is_file() else {}
    out["load_record"] = {"lost": rec.get("lost"), "response": rec.get("response"), "error": rec.get("error")}
    out["passed"] = smoke_passed(gen, out["check_result"], load, rec)
    return out


def smoke_passed(gen: dict, check: dict | None, load: dict, rec: dict) -> bool:
    """Both jobs complete, the window mesh is the cylindrical mesh with the protocol's edges, 0 particles lost."""
    lost = rec.get("lost") or {}
    return bool(gen.get("returncode") == 0 and load.get("returncode") == 0 and check and check.get("passed")
                and "response" in rec and lost.get("restart_files") == 0)


def produce(label: str, d: Path, windows: Path, cutoff: bool, budget: Budget, item: str | None = None) -> dict:
    prod = job(production_argv(label, d, windows, cutoff), d / "produce.log", budget.timeout(S.PRODUCTION_S + 600), wait=True)
    budget.add(item or label, "produce", prod)
    out = {"label": label, "produce": prod, "windows": str(windows), "cutoff_ev": S.CUTOFF_EV if cutoff else None,
           "contended": prod["contended"]}
    if prod["returncode"] == "never_quiet":
        out.update(not_evaluated(label, budget, "the machine did not become quiet"))
        return out
    out.update(D.score(d / "produce.json"))
    return out


def e2_item(scratch: Path, budget: Budget) -> dict:
    d = scratch / "e2"
    d.mkdir(parents=True, exist_ok=True)
    gen = job(generation_argv(S.SEEDS["gen"]["E2"], d, False), d / "gen.log", budget.timeout(2400))
    budget.add("E2", "generate", gen)
    windows = d / "gen/weight_windows.h5"
    if gen["returncode"] != 0 or not windows.is_file():
        return {"label": "E2", "generate": gen, **not_evaluated("E2", budget, "generation failed; see gen.log and gen.json")}
    row = produce("E2", d, windows, False, budget)
    row["generate"] = gen
    return row


def t2_reference() -> tuple[float | None, float | None]:
    """S0d T2's recorded gain and sigma, for the E0 repeatability check."""
    path = S0D_DIR / "results-s0d.json"
    try:
        t2 = json.loads(path.read_text(encoding="utf-8"))["rows"]["T2"]
        return t2.get("gain"), t2.get("sigma")
    except (OSError, KeyError, ValueError):
        return None, None


def summarise(rows: dict[str, dict], budget: Budget, extra: dict) -> dict:
    scored = {k: v for k, v in rows.items() if k in ("E0", "E1", "E2", "E3")}
    h5, h6 = S.h5_rule(scored.get("E0"), scored.get("E1")), S.h6_rule(scored.get("E0"), scored.get("E2"))
    t2g, t2s = t2_reference()
    return {"rows": rows, "ledger": {"cap_s": budget.cap_s, "entries": budget.entries}, "job_wall_s": budget.used(),
            "rule": {"H5": h5, "H6": h6, "E3_required": S.e3_required(h5, h6), **S.g_star(scored)},
            "e0_vs_t2": {"t2_gain": t2g, "t2_sigma": t2s, "z": S.repeatability_z(scored.get("E0"), t2g, t2s)}, **extra}


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--scratch", required=True, type=Path)
    args = ap.parse_args(argv)
    scratch = args.scratch
    scratch.mkdir(parents=True, exist_ok=True)
    budget = Budget()
    rows: dict[str, dict] = {}
    extra = {"start_epoch": time.time(), "protocol": "docs/notes/CAD_TRANSPORT_S0E.md"}

    def save() -> dict:
        result = summarise(rows, budget, extra)
        (scratch / "results-s0e.json").write_text(json.dumps(result, indent=1, sort_keys=True, default=str) + "\n",
                                                  encoding="utf-8")
        return result

    skipped = fit_or_skip("E-smoke", "smoke", budget)
    rows["E-smoke"] = skipped or smoke(scratch, budget)
    save()
    t2_windows = S0D_DIR / "t2/gen/weight_windows.h5"
    for label, cutoff in (("E0", False), ("E1", True)):
        skipped = fit_or_skip(label, label, budget)
        if skipped is None and not t2_windows.is_file():
            skipped = not_evaluated(label, budget, f"S0d T2's window file is missing: {t2_windows}")
        if skipped is None:
            d = scratch / label.lower()
            d.mkdir(parents=True, exist_ok=True)
            skipped = produce(label, d, t2_windows, cutoff, budget)
        rows[label] = skipped
        save()
    if rows["E-smoke"].get("passed"):
        rows["E2"] = fit_or_skip("E2", "E2", budget) or e2_item(scratch, budget)
    else:
        rows["E2"] = not_evaluated("E2", budget, "E-smoke did not pass")
    save()
    h5, h6 = S.h5_rule(rows.get("E0"), rows.get("E1")), S.h6_rule(rows.get("E0"), rows.get("E2"))
    if not S.e3_required(h5, h6):
        rows["E3"] = {"label": "E3", "gain": None, "sigma": None, "unbiased": None,
                      "note": "not run: H5 and H6 are not both confirmed (rule)"}
    else:
        skipped = fit_or_skip("E3", "E3", budget)
        if skipped is None:
            d = scratch / "e3"
            d.mkdir(parents=True, exist_ok=True)
            skipped = produce("E3", d, scratch / "e2/gen/weight_windows.h5", True, budget)
        rows["E3"] = skipped
    result = save()
    print(json.dumps(result["rule"], indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
