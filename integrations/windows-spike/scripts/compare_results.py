#!/usr/bin/env python3
"""Compares two summaries from summarize_statepoint.py bin by bin, in combined sigma.

    compare_results.py A.json B.json [--label-a NAME --label-b NAME] [--out REPORT.md]

z = (mean_a - mean_b) / sqrt(std_a^2 + std_b^2). Bins where both means are zero are skipped.
Prints the worst |z|, the count of bins above 3 sigma and the chi-square per bin. Exit code
is always 0: the verdict is read from the report, not enforced here.
"""
import argparse
import json
import math


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("a")
    ap.add_argument("b")
    ap.add_argument("--label-a", default="A")
    ap.add_argument("--label-b", default="B")
    ap.add_argument("--out")
    args = ap.parse_args()
    a, b = json.load(open(args.a)), json.load(open(args.b))
    rows, zs = [], []
    for key in sorted(a["tallies"]):
        if key not in b["tallies"]:
            rows.append(f"| {key} | missing in {args.label_b} | | | | |")
            continue
        ta, tb = a["tallies"][key], b["tallies"][key]
        for i, (ma, mb, sa, sb) in enumerate(zip(ta["mean"], tb["mean"], ta["std"], tb["std"])):
            for j, (xa, xb, ea, eb) in enumerate(zip(ma, mb, sa, sb)):
                if xa == 0.0 and xb == 0.0:
                    continue
                den = math.hypot(ea, eb)
                z = (xa - xb) / den if den > 0 else (0.0 if xa == xb else math.inf)
                zs.append(z)
                rel = 100.0 * (xa - xb) / xb if xb else math.nan
                rows.append(f"| {key} | {i}.{j} | {xa:.6e} | {xb:.6e} | {rel:+.3f}% | {z:+.2f} |")
    n = len(zs)
    worst = max((abs(z) for z in zs), default=0.0)
    over3 = sum(1 for z in zs if abs(z) > 3)
    chi = sum(z * z for z in zs) / n if n else math.nan
    head = [
        f"Compared {args.label_a} (OpenMC {a['version']}) with {args.label_b} (OpenMC {b['version']}).",
        f"Bins compared: {n}. Worst |z|: {worst:.2f}. Bins above 3 sigma: {over3}. Chi-square per bin: {chi:.2f}.",
        "",
        f"| tally | bin | {args.label_a} | {args.label_b} | difference | z (sigma) |",
        "|---|---|---|---|---|---|",
    ]
    text = "\n".join(head + rows) + "\n"
    print(text)
    if args.out:
        open(args.out, "w").write(text)


if __name__ == "__main__":
    main()
