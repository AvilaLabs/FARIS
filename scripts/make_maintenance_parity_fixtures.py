#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Writes crates/faris-engine/tests/fixtures/maintenance_parity.json: inputs and outputs of the cooling-curve
functions of scripts/maintenance_coupling_test.py, which the Rust port in faris_engine::maintenance must reproduce.

Run from the repository root: python3 scripts/make_maintenance_parity_fixtures.py
"""
from __future__ import annotations

import importlib.util
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "crates" / "faris-engine" / "tests" / "fixtures" / "maintenance_parity.json"


def load_reference():
    spec = importlib.util.spec_from_file_location("maintenance_coupling_test", HERE / "maintenance_coupling_test.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def log_curve(t0, t1, n, q0, decay, shape=1.0):
    """n log-spaced points from t0 to t1 with value q0 * exp(-decay * (ln(t/t0)) ** shape)."""
    import math
    out = []
    for i in range(n):
        t = t0 * (t1 / t0) ** (i / (n - 1))
        out.append([t, q0 * math.exp(-decay * math.log(t / t0) ** shape)])
    return out


def main() -> None:
    ref = load_reference()
    day = ref.DAY_S
    fixtures: dict = {"cooling_grid_s": ref.cooling_grid_s()}

    pts = [[10.0, 4.0], [100.0, 1.0], [1000.0, 0.25], [10000.0, 0.0]]
    pos = [[10.0, 4.0], [100.0, 1.0], [1000.0, 0.25]]
    neg = [[1.0, -2.0], [10.0, -1.0], [100.0, 3.0]]
    zero = [[1.0, 0.0], [10.0, 0.0], [100.0, 5.0]]
    two = [[5.0, 2.0], [50.0, 20.0]]
    one = [[7.0, 3.0]]
    cases = []
    for points, ts in [
        (pos, [1.0, 10.0, 11.0, 31.6, 100.0, 500.0, 1000.0, 5000.0]),
        (pts, [10.0, 300.0, 5000.0, 9999.0, 10000.0, 20000.0]),
        (neg, [0.5, 1.0, 3.0, 10.0, 30.0, 100.0, 1e3]),
        (zero, [0.5, 2.0, 10.0, 50.0, 100.0]),
        (two, [1.0, 5.0, 16.0, 50.0, 80.0]),
        (one, [1.0, 7.0, 9.0]),
    ]:
        for t in ts:
            cases.append({"points": points, "t": t, "expected": ref.interpolate([tuple(p) for p in points], t)})
    fixtures["interpolate"] = cases

    grid = ref.cooling_grid_s()
    a = [[t, 1e4 * (t / 3600.0) ** -0.8] for t in grid]
    b = [[t, 3e3 * (t / 3600.0) ** -0.5 + 5.0] for t in grid[5:30]]
    c = [[t, 50.0 * 0.5 ** (i / 4)] for i, t in enumerate(grid[2:])]
    d = [[t, 7.0 + 0.0 * i] for i, t in enumerate(grid[:20:3])]
    short = [[t, 2.0] for t in (1000.0, 4000.0, 9000.0)]
    combos = [
        ({"a": a, "b": b}, {"a": 2.0, "b": 3.0}, 365 * day),
        ({"a": a, "b": b, "c": c}, {"a": 1.0, "b": 0.5, "c": 4.0}, 365 * day),
        ({"a": a, "c": c, "d": d}, {"a": 1.5, "c": 2.5, "d": 1.0}, 365 * day),
        ({"a": a, "b": b}, {"a": 2.0, "b": 3.0}, 30 * day),
        ({"a": a, "s": short}, {"a": 1.0, "s": 1.0}, 365 * day),
        ({"a": [[1.0, 1.0], [2.0, 1.0]], "z": [[10.0, 1.0], [20.0, 1.0]]}, {"a": 1.0, "z": 1.0}, 365 * day),
    ]
    out = []
    for series, weights, max_s in combos:
        entry = {"series": list(series.values()), "volumes": [weights[k] for k in series], "max_s": max_s}
        try:
            entry["expected"] = ref.combined_curve({k: [tuple(p) for p in v] for k, v in series.items()}, weights, max_s)
        except ValueError:
            entry["expected"] = None
        out.append(entry)
    fixtures["combined_curve"] = out

    falling = log_curve(3600.0, 365 * day, 40, 1000.0, 0.45)
    zeros = [[1000.0, 5.0], [2000.0, 3.0], [4000.0, 0.0], [8000.0, 0.0]]
    flat_end = [[t, q] for t, q in log_curve(3600.0, 100 * day, 25, 80.0, 0.3)]
    never = log_curve(3600.0, 365 * day, 40, 1000.0, 0.05)
    wlim = [[3600.0, 50.0], [7200.0, 40.0], [20000.0, 30.0]]
    mono_bump = [[3600.0, 10.0], [7200.0, 20.0], [14400.0, 5.0], [28800.0, 1.0]]
    cd_cases = []
    for curve, qs in [
        (falling, [2000.0, 1000.0, 500.0, 100.0, 10.0, 1.0, 0.0]),
        (zeros, [4.0, 2.0, 0.0, 1.0]),
        (flat_end, [100.0, 50.0, 1.0]),
        (never, [10.0, 400.0]),
        (wlim, [100.0, 35.0, 10.0]),
        (mono_bump, [15.0, 5.0, 8.0]),
        ([], [1.0]),
    ]:
        for q in qs:
            cd_cases.append({"curve": curve, "q_star": q, "max_s": 365 * day,
                             "expected": ref.cooldown([tuple(p) for p in curve], q, 365 * day)})
    fixtures["cooldown"] = cd_cases

    cal_cases = []
    for curve, targets in [
        (falling, [3600.0, 10 * day, 100 * day, 365 * day, 1000.0, 400 * day]),
        (wlim, [7200.0, 10000.0, 30000.0]),
        (mono_bump, [5000.0, 10000.0, 20000.0, 28800.0]),
        (zeros, [1500.0, 3000.0, 6000.0]),
        ([], [100.0]),
    ]:
        for target in targets:
            cal_cases.append({"curve": curve, "target_cooldown_s": target,
                              "expected": ref.calibrate([tuple(p) for p in curve], target)})
    fixtures["calibrate"] = cal_cases

    OUT.write_text(json.dumps(fixtures, indent=1) + "\n", encoding="utf-8")
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
