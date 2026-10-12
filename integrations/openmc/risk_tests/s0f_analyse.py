#!/usr/bin/env python3
"""S0f analysis: pooled gains of F0 and F1 against S0b's D2, the bias check, H7 and F1's NUC-050 band.

    s0f_analyse.py --runs DIR --out results-s0f.json

Reads DIR/{F0,F1}-{seed}/record.json and batches.json for the protocol's four seeds. Pooled per configuration: all
batches together give the mean, R and variance per history; T is the sum of the runs' seconds; FOM = 1/(R^2 T); the gain
is FOM / D2's. The lower bound L is the smaller of the seed bound (Student t on the four per-run gains) and the 5th
percentile of a bootstrap over batches (2,000 resamples, numpy seed 20261012). Protocol: CAD_TRANSPORT_S0F.md, "Protocol".
"""
from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import s0f_common as F  # noqa: E402

BOOT_N = 2000
BOOT_SEED = 20261012


def pooled(rows: list[dict], values: list[list[float]]) -> dict:
    allv = [x for v in values for x in v]
    st = F.batch_stats(allv, F.PER_BATCH)
    t = sum(r["seconds_from_start"] for r in rows)
    f = F.fom(st["relative_error"], t)
    gain = f / F.D2_FOM if f else None
    per_run = [r["fom"] / F.D2_FOM for r in rows]
    seed_b = F.seed_lower_bound(per_run)
    import numpy as np
    rng = np.random.default_rng(BOOT_SEED)
    arr = np.array(allv)
    n = len(arr)
    boot = []
    for _ in range(BOOT_N):
        s = arr[rng.integers(0, n, n)]
        m = s.mean()
        r = math.sqrt(s.var(ddof=1) / n) / m
        boot.append(1.0 / (r * r * t) / F.D2_FOM)
    boot_lower = float(np.percentile(boot, 5))
    z = F.z_against_d2(st["mean"], st["std_error"])
    return {"runs": len(rows), "histories": n * F.PER_BATCH, "seconds": t, "a": st, "fom": f, "gain": gain,
            "per_run_gain": per_run, "seed_bound": seed_b, "bootstrap_lower95": boot_lower,
            "lower95": min(seed_b["lower95"], boot_lower), "z_vs_d2": z, "unbiased": abs(z) <= F.Z_UNBIASED,
            "vov_per_run": [r["a"]["vov"] for r in rows], "vov_pooled": st["vov"],
            "tail_resolved": st["vov"] is not None and st["vov"] <= F.VOV_LIMIT,
            "contended_runs": sum(1 for r in rows if float(r["loadavg_end"].split()[0]) > 6.0)}


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--runs", required=True, type=Path)
    p.add_argument("--out", required=True, type=Path)
    p.add_argument("--pair", nargs=2, default=["F0", "F1"], help="reference and candidate configurations")
    p.add_argument("--seed-set", type=int, choices=[1, 2], default=1, help="1: protocol 1's seeds; 2: protocol 2's")
    args = p.parse_args()
    seeds = F.SEEDS if args.seed_set == 1 else F.SEEDS2
    ref_cfg, cand_cfg = args.pair
    res = {"kind": "s0f-results", "d2": {"fom": F.D2_FOM, "fom_sigma": F.D2_FOM_SIGMA, "mean": F.D2_MEAN,
                                         "std_error": F.D2_STD_ERROR}}
    rows, vals = {}, {}
    for cfg in (ref_cfg, cand_cfg):
        rows[cfg], vals[cfg] = [], []
        for seed in seeds:
            d = args.runs / f"{cfg}-{seed}"
            if not (d / "record.json").exists():
                continue
            rows[cfg].append(json.loads((d / "record.json").read_text()))
            vals[cfg].append(json.loads((d / "batches.json").read_text())["a"])
        res[cfg] = pooled(rows[cfg], vals[cfg]) if len(rows[cfg]) == len(seeds) else {"runs": len(rows[cfg]),
                                                                                            "outcome": "NOT_EVALUATED"}
    res["pair"] = [ref_cfg, cand_cfg]
    res["seeds"] = seeds
    complete = all(len(rows[c]) == len(seeds) for c in (ref_cfg, cand_cfg))
    if complete:
        paired = [b["fom"] / a["fom"] for a, b in zip(rows[ref_cfg], rows[cand_cfg])]
        ratio = res[cand_cfg]["fom"] / res[ref_cfg]["fom"]
        h_rule, band = (F.h7_outcome, F.nuc050_outcome) if args.seed_set == 1 else (F.h8_outcome, F.nuc050_outcome2)
        res["h7"] = {"paired_fom_ratios": paired, "pooled_ratio": ratio, "outcome": h_rule(paired, ratio)}
        res["nuc050"] = band(res[cand_cfg]["gain"], res[cand_cfg]["lower95"], res[cand_cfg]["unbiased"])
    else:
        res["h7"] = {"outcome": "NOT_EVALUATED"}
        res["nuc050"] = "NOT_EVALUATED"
    args.out.write_text(json.dumps(res, indent=1))
    for cfg in (ref_cfg, cand_cfg):
        r = res[cfg]
        if "gain" in r:
            print(f"{cfg}: mean {r['a']['mean']:.4e} z {r['z_vs_d2']:+.2f} var/hist {r['a']['var_per_history']:.0f} "
                  f"gain {r['gain']:.1f} (seed L {r['seed_bound']['lower95']:.1f}, boot L {r['bootstrap_lower95']:.1f}) "
                  f"VOV {r['vov_pooled']:.3f}")
    print("H7:" if args.seed_set == 1 else "H8:", res["h7"]["outcome"]); print("NUC-050:", res["nuc050"])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
