#!/usr/bin/env python3
"""Compares two faris-step-inspect/v1 reports: the Python helper's and the native worker's.

    compare_inspect.py PYTHON.json NATIVE.json [--rel 1e-9]

Prints one summary line per file and a table of the largest differences; exits 1 on a
mismatch in solid count, order, names, unit fields, validity, or when a volume or centroid
differs by more than the relative tolerance. Centroid components are compared against the
solid's own bounding-box diagonal, since a component near zero has no meaningful relative error.
"""
import argparse
import json
import math
import sys
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("python", type=Path)
    parser.add_argument("native", type=Path)
    parser.add_argument("--rel", type=float, default=1e-9)
    args = parser.parse_args()
    a = json.loads(args.python.read_text())
    b = json.loads(args.native.read_text())
    if "result" in b:
        b = b["result"]
    problems = []
    if a["step"]["sha256"] != b["step"]["sha256"]:
        problems.append("different STEP files")
    ua, ub = a["unit"], b["unit"]
    for key in ("declared", "factor_to_m", "used", "used_is_assumed", "text_units", "ocp_units", "problems"):
        if ua.get(key) != ub.get(key):
            problems.append(f"unit.{key}: python {ua.get(key)!r} native {ub.get(key)!r}")
    if a["problems"] != b["problems"]:
        problems.append(f"problems: python {a['problems']} native {b['problems']}")
    sa, sb = a["solids"], b["solids"]
    if len(sa) != len(sb):
        problems.append(f"solid count: python {len(sa)} native {len(sb)}")
    worst_volume = worst_centroid = worst_bbox = 0.0
    for x, y in zip(sa, sb):
        i = x["step_index"]
        if x["step_index"] != y["step_index"]:
            problems.append(f"order differs at {i}")
        if x["step_name"] != y["step_name"]:
            problems.append(f"solid {i}: name python {x['step_name']!r} native {y['step_name']!r}")
        if x["occt_valid"] != y["occt_valid"]:
            problems.append(f"solid {i}: validity python {x['occt_valid']} native {y['occt_valid']}")
        if x["cad_volume_m3"] is None or y["cad_volume_m3"] is None:
            if x["cad_volume_m3"] != y["cad_volume_m3"]:
                problems.append(f"solid {i}: volume presence differs")
            continue
        rv = abs(x["cad_volume_m3"] - y["cad_volume_m3"]) / max(abs(x["cad_volume_m3"]), 1e-300)
        low, high = x["bbox_min_m"], x["bbox_max_m"]
        diagonal = math.dist(low, high)
        rc = max(abs(p - q) for p, q in zip(x["centroid_m"], y["centroid_m"])) / diagonal
        rb = max(abs(p - q) for p, q in zip(low + high, y["bbox_min_m"] + y["bbox_max_m"])) / diagonal
        worst_volume, worst_centroid, worst_bbox = max(worst_volume, rv), max(worst_centroid, rc), max(worst_bbox, rb)
        if rv > args.rel:
            problems.append(f"solid {i}: volume relative difference {rv:.3e}")
        if rc > args.rel:
            problems.append(f"solid {i}: centroid difference {rc:.3e} of the bounding-box diagonal")
        if rb > args.rel:
            problems.append(f"solid {i}: bounding box difference {rb:.3e} of the diagonal")
    print(f"{args.python.name}: python {len(sa)} solids, native {len(sb)}; unit {ua.get('declared')}/{ub.get('declared')}; "
          f"worst volume rel {worst_volume:.2e}, centroid/diag {worst_centroid:.2e}, bbox/diag {worst_bbox:.2e}; "
          f"python read {a['timing_seconds'].get('read_and_transfer', 0):.2f} s measure {a['timing_seconds'].get('measure_solids', 0):.2f} s, "
          f"native read {b['timing_seconds'].get('read_and_transfer', 0):.2f} s measure {b['timing_seconds'].get('measure_solids', 0):.2f} s")
    for p in problems:
        print("  MISMATCH", p)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
