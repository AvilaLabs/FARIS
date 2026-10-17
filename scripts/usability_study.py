#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Analysis and raw-data store of the usability benchmark (requirements UX-002, UX-074, UX-075).

    python3 scripts/usability_study.py register --store DIR FILE...   copy raw files in, hashed, append-only
    python3 scripts/usability_study.py verify   --store DIR           re-hash every stored file
    python3 scripts/usability_study.py report   --store DIR [--out FILE] [--phase F1|F6]
    python3 scripts/usability_study.py check    --store DIR --report FILE   regenerate the report and diff
    python3 scripts/usability_study.py lint     FILE...                  SUS statements in report texts (UX-002)

The analysis is the one written in docs/notes/USABILITY_ANALYSIS_PLAN.md, fixed before session 1:

  success   adjusted-Wald 95 % interval per task (add z^2/2 successes and z^2 trials)
  time      geometric mean, median and P90 with a percentile-bootstrap 95 % interval (10,000 resamples, fixed seed)
  SUS       mean, sample SD and t-interval; every SUS line prints n, mean, SD and the 95 % CI, and n < 12 is
            labelled "formative, not a benchmark" (UX-002)
  drops     no result is dropped except by a rule in exclusions.json; every drop is listed in the report

Raw files (CSV, UTF-8): sus.csv (participant, q1..q10 with 1..5), tasks.csv (participant, task, success 0/1,
seconds, assisted 0/1), exclusions.json ({"rules": [{"id", "column", "equals" or "above", "reason"}]}).
The store keeps each file once under its SHA-256 and records it in ledger.jsonl, to which lines are only
appended; a name registered twice with other bytes is refused. The report's numbers and its verdict come from
the raw files only; `check` fails when the stored report differs from a regeneration.
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import io
import json
import math
import random
import re
import statistics
import sys
from pathlib import Path

Z95 = 1.959963984540054
BOOTSTRAP_RESAMPLES = 10_000
BOOTSTRAP_SEED = 20261017
FORMATIVE_BELOW = 12
FORMATIVE_LABEL = "formative, not a benchmark"
# UX-001: (minimum mean, minimum lower bound of the 95 % CI) per phase, with n >= 20.
SUS_BARS = {"F1": (74.0, 68.0), "F6": (80.3, 74.0)}
SUS_MIN_N = 20


# --- statistics -------------------------------------------------------------------------------

def _betacf(a: float, b: float, x: float) -> float:
    tiny = 1e-300
    qab, qap, qam = a + b, a + 1.0, a - 1.0
    c, d = 1.0, 1.0 - qab * x / qap
    d = 1.0 / (d if abs(d) > tiny else tiny)
    h = d
    for m in range(1, 300):
        m2 = 2 * m
        aa = m * (b - m) * x / ((qam + m2) * (a + m2))
        d = 1.0 + aa * d
        d = 1.0 / (d if abs(d) > tiny else tiny)
        c = 1.0 + aa / c
        c = c if abs(c) > tiny else tiny
        h *= d * c
        aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2))
        d = 1.0 + aa * d
        d = 1.0 / (d if abs(d) > tiny else tiny)
        c = 1.0 + aa / c
        c = c if abs(c) > tiny else tiny
        delta = d * c
        h *= delta
        if abs(delta - 1.0) < 1e-15:
            break
    return h


def _betainc(a: float, b: float, x: float) -> float:
    if x <= 0.0:
        return 0.0
    if x >= 1.0:
        return 1.0
    front = math.exp(math.lgamma(a + b) - math.lgamma(a) - math.lgamma(b) + a * math.log(x) + b * math.log1p(-x))
    if x < (a + 1.0) / (a + b + 2.0):
        return front * _betacf(a, b, x) / a
    return 1.0 - front * _betacf(b, a, 1.0 - x) / b


def t_cdf(t: float, df: int) -> float:
    tail = 0.5 * _betainc(df / 2.0, 0.5, df / (df + t * t))
    return 1.0 - tail if t > 0 else tail


def t_quantile(p: float, df: int) -> float:
    """Student t quantile by bisection on the exact CDF."""
    lo, hi = 0.0, 1e4
    for _ in range(200):
        mid = (lo + hi) / 2.0
        if t_cdf(mid, df) < p:
            lo = mid
        else:
            hi = mid
    return (lo + hi) / 2.0


def mean_sd_ci(values: list[float]) -> dict:
    """n, mean, sample SD and the t-based 95 % interval."""
    n = len(values)
    if n < 2:
        raise ValueError(f"need at least 2 values for an SD and an interval, got {n}")
    mean = statistics.fmean(values)
    sd = statistics.stdev(values)
    half = t_quantile(0.975, n - 1) * sd / math.sqrt(n)
    return {"n": n, "mean": mean, "sd": sd, "ci95_low": mean - half, "ci95_high": mean + half}


def adjusted_wald(successes: int, n: int, z: float = Z95) -> dict:
    """Agresti-Coull adjusted-Wald interval, clipped to [0, 1]."""
    if n <= 0 or not 0 <= successes <= n:
        raise ValueError(f"need 0 <= successes <= n and n > 0, got {successes} of {n}")
    n_adj = n + z * z
    p_adj = (successes + z * z / 2.0) / n_adj
    half = z * math.sqrt(p_adj * (1.0 - p_adj) / n_adj)
    return {"n": n, "successes": successes, "rate": successes / n, "ci95_low": max(0.0, p_adj - half),
            "ci95_high": min(1.0, p_adj + half)}


def percentile(values: list[float], q: float) -> float:
    """Linear-interpolation percentile (q in 0..100)."""
    v = sorted(values)
    pos = (len(v) - 1) * q / 100.0
    lo = math.floor(pos)
    hi = min(lo + 1, len(v) - 1)
    return v[lo] + (v[hi] - v[lo]) * (pos - lo)


def geometric_mean(values: list[float]) -> float:
    return math.exp(statistics.fmean(math.log(v) for v in values))


def time_summary(seconds: list[float], resamples: int = BOOTSTRAP_RESAMPLES, seed: int = BOOTSTRAP_SEED) -> dict:
    """Geometric mean, median and P90 of times, each with a percentile-bootstrap 95 % interval."""
    if len(seconds) < 2 or any(s <= 0 for s in seconds):
        raise ValueError("need at least 2 positive times")
    stats = {"geometric_mean": geometric_mean, "median": statistics.median, "p90": lambda v: percentile(v, 90.0)}
    rng = random.Random(seed)
    draws = {k: [] for k in stats}
    for _ in range(resamples):
        sample = [seconds[rng.randrange(len(seconds))] for _ in seconds]
        for k, f in stats.items():
            draws[k].append(f(sample))
    out = {"n": len(seconds), "resamples": resamples, "seed": seed}
    for k, f in stats.items():
        out[k] = {"value": f(seconds), "ci95_low": percentile(draws[k], 2.5), "ci95_high": percentile(draws[k], 97.5)}
    return out


# --- SUS --------------------------------------------------------------------------------------

def sus_score(items: list[int]) -> float:
    """The 0..100 SUS score of ten 1..5 item responses (odd items score r-1, even items 5-r)."""
    if len(items) != 10 or any(not isinstance(r, int) or not 1 <= r <= 5 for r in items):
        raise ValueError("a SUS response is ten integers from 1 to 5")
    return 2.5 * sum((r - 1) if i % 2 == 0 else (5 - r) for i, r in enumerate(items))


def sus_report(scores: list[float]) -> str:
    """The one-line SUS statement: always n, mean, SD and the 95 % CI; n < 12 carries the formative label."""
    s = mean_sd_ci(scores)
    line = f"SUS n={s['n']}, mean={s['mean']:.1f}, SD={s['sd']:.1f}, 95% CI [{s['ci95_low']:.1f}, {s['ci95_high']:.1f}]"
    return line + (f" ({FORMATIVE_LABEL})" if s["n"] < FORMATIVE_BELOW else "")


def lint_sus_text(text: str) -> list[str]:
    """UX-002 report lint: every line that gives a SUS value must carry n, mean, SD and the 95 % CI, and n < 12 the label."""
    problems = []
    for number, line in enumerate(text.splitlines(), 1):
        if not re.search(r"\bSUS\b.*\d", line):
            continue
        missing = [name for name, pat in (("n", r"\bn\s*=\s*\d+"), ("mean", r"\bmean\s*=\s*\d"), ("SD", r"\bSD\s*=\s*\d"),
                                         ("95% CI", r"95\s?% CI \[")) if not re.search(pat, line)]
        if missing:
            problems.append(f"line {number}: SUS value without {', '.join(missing)}")
            continue
        n = int(re.search(r"\bn\s*=\s*(\d+)", line).group(1))
        if n < FORMATIVE_BELOW and FORMATIVE_LABEL not in line:
            problems.append(f"line {number}: SUS with n={n} lacks the label {FORMATIVE_LABEL!r}")
    return problems


def sus_verdict(scores: list[float], phase: str) -> dict:
    """UX-001 computed, not typed: the bars of the phase on the mean and the lower bound, with n >= 20."""
    mean_bar, low_bar = SUS_BARS[phase]
    s = mean_sd_ci(scores)
    checks = {"n": s["n"] >= SUS_MIN_N, "mean": s["mean"] >= mean_bar, "ci95_low": s["ci95_low"] >= low_bar}
    if not checks["n"]:
        verdict = "INCONCLUSIVE"
    else:
        verdict = "PASS" if all(checks.values()) else "FAIL"
    return {"phase": phase, "bars": {"mean": mean_bar, "ci95_low": low_bar, "n": SUS_MIN_N}, "checks": checks, "verdict": verdict}


# --- raw files and the store ------------------------------------------------------------------

def read_csv(text: str) -> list[dict]:
    return list(csv.DictReader(io.StringIO(text)))


def apply_exclusions(rows: list[dict], rules: list[dict], source: str) -> tuple[list[dict], list[dict]]:
    """(kept rows, drops): a row is dropped only by a rule of the protocol, and every drop is returned."""
    kept, drops = [], []
    for row in rows:
        why = None
        for rule in rules:
            col = rule["column"]
            if col not in row:
                continue
            if ("equals" in rule and row[col] == str(rule["equals"])) or ("above" in rule and float(row[col]) > float(rule["above"])):
                why = rule
                break
        if why:
            drops.append({"source": source, "row": row, "rule": why["id"], "reason": why["reason"]})
        else:
            kept.append(row)
    return kept, drops


def build_report(sus_text: str, tasks_text: str, exclusions_text: str, phase: str = "F1") -> dict:
    rules = json.loads(exclusions_text).get("rules", [])
    sus_rows, sus_drops = apply_exclusions(read_csv(sus_text), rules, "sus.csv")
    task_rows, task_drops = apply_exclusions(read_csv(tasks_text), rules, "tasks.csv")
    scores = [sus_score([int(r[f"q{i}"]) for i in range(1, 11)]) for r in sus_rows]
    report = {"phase": phase, "sus": {**mean_sd_ci(scores), "line": sus_report(scores),
                                      "formative": len(scores) < FORMATIVE_BELOW, "verdict": sus_verdict(scores, phase)},
              "tasks": {}, "drops": sus_drops + task_drops}
    for task in sorted({r["task"] for r in task_rows}):
        rows = [r for r in task_rows if r["task"] == task]
        report["tasks"][task] = {
            "success": adjusted_wald(sum(int(r["success"]) for r in rows), len(rows)),
            "time": time_summary([float(r["seconds"]) for r in rows if r["success"] == "1"]),
        }
    return report


def render(report: dict) -> str:
    return json.dumps(report, indent=2, sort_keys=True) + "\n"


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def ledger_entries(store: Path) -> list[dict]:
    path = store / "ledger.jsonl"
    if not path.is_file():
        return []
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]


def register(store: Path, files: list[Path]) -> list[dict]:
    """Copy files into the store under their SHA-256 and append them to the ledger; never overwrite."""
    (store / "objects").mkdir(parents=True, exist_ok=True)
    entries = ledger_entries(store)
    names = {e["name"]: e["sha256"] for e in entries}
    added = []
    for file in files:
        data = file.read_bytes()
        digest = sha256(data)
        if names.get(file.name, digest) != digest:
            raise ValueError(f"{file.name} is already registered with other bytes (append-only: register it under a new name)")
        if file.name in names:
            continue
        (store / "objects" / digest).write_bytes(data)
        entry = {"name": file.name, "sha256": digest, "bytes": len(data)}
        with (store / "ledger.jsonl").open("a", encoding="utf-8") as f:
            f.write(json.dumps(entry, sort_keys=True) + "\n")
        names[file.name] = digest
        added.append(entry)
    return added


def verify(store: Path) -> list[str]:
    problems = []
    for e in ledger_entries(store):
        obj = store / "objects" / e["sha256"]
        if not obj.is_file():
            problems.append(f"{e['name']}: stored object {e['sha256'][:12]} is missing")
        elif sha256(obj.read_bytes()) != e["sha256"]:
            problems.append(f"{e['name']}: stored bytes no longer match {e['sha256'][:12]}")
    return problems


def stored_text(store: Path, name: str) -> str:
    found = [e for e in ledger_entries(store) if e["name"] == name]
    if not found:
        raise ValueError(f"{name} is not registered in the store")
    return (store / "objects" / found[-1]["sha256"]).read_text(encoding="utf-8")


def report_from_store(store: Path, phase: str = "F1") -> dict:
    problems = verify(store)
    if problems:
        raise ValueError("the store fails its hash check: " + "; ".join(problems))
    report = build_report(*(stored_text(store, n) for n in ("sus.csv", "tasks.csv", "exclusions.json")), phase=phase)
    report["inputs"] = {e["name"]: e["sha256"] for e in ledger_entries(store)}
    return report


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="usability benchmark raw-data store and analysis")
    sub = ap.add_subparsers(dest="cmd", required=True)
    lint = sub.add_parser("lint")
    lint.add_argument("files", type=Path, nargs="+")
    for name in ("register", "verify", "report", "check"):
        p = sub.add_parser(name)
        p.add_argument("--store", type=Path, required=True)
        if name == "register":
            p.add_argument("files", type=Path, nargs="+")
        if name in ("report", "check"):
            p.add_argument("--phase", choices=sorted(SUS_BARS), default="F1")
        if name == "report":
            p.add_argument("--out", type=Path)
        if name == "check":
            p.add_argument("--report", type=Path, required=True)
    args = ap.parse_args(argv)
    try:
        if args.cmd == "lint":
            problems = [f"{f}: {p}" for f in args.files for p in lint_sus_text(f.read_text(encoding="utf-8"))]
            print("\n".join(problems) or "SUS statements are complete")
            return 1 if problems else 0
        if args.cmd == "register":
            for e in register(args.store, args.files):
                print(f"registered {e['name']} {e['sha256']}")
        elif args.cmd == "verify":
            problems = verify(args.store)
            print("\n".join(problems) or "store verified")
            return 1 if problems else 0
        elif args.cmd == "report":
            text = render(report_from_store(args.store, args.phase))
            if args.out:
                args.out.write_text(text, encoding="utf-8")
            print(text, end="")
        else:
            fresh = render(report_from_store(args.store, args.phase))
            if args.report.read_text(encoding="utf-8") != fresh:
                print("the stored report differs from a regeneration from the raw files", file=sys.stderr)
                return 1
            print("report matches its raw files")
    except (ValueError, OSError, KeyError) as e:
        print(f"usability study: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
