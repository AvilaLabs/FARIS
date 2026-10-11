#!/usr/bin/env python3
"""Compares two faris-tessellation/v1 reports (Python helper, native worker) and their facets.

    compare_tessellate.py PYTHON_DIR NATIVE_DIR

Each directory holds tessellation.json (or the report named by --report) and solids.facets.*.
The Python report has no per-solid table, so per-solid counts come from the facets themselves:
vertices, triangles and surfaces per volume, and closure by the helper's own directed-edge rule.
"""
import argparse
import json
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "integrations" / "cad"))
import faris_cad  # noqa: E402
import faris_facets  # noqa: E402


def per_volume(path: Path):
    data = faris_facets.unpack_facets(path.read_bytes())
    tri = np.array(data["triangles"], dtype=np.int64).reshape(-1, 3)
    vtx = np.array(data["vertices"])
    out = {}
    for first, count, fwd, _rev in data["surfaces"]:
        out.setdefault(fwd - 1, []).append((first, count))
    rows = []
    for vol in sorted(out):
        ids = np.concatenate([tri[f : f + c] for f, c in out[vol]])
        un, rep, _ = faris_cad.open_edge_report(ids)
        used = np.unique(ids)
        p = vtx[ids]
        vol_m3 = float(np.einsum("ij,ij->i", p[:, 0], np.cross(p[:, 1], p[:, 2])).sum() / 6.0)
        rows.append((vol, len(used), len(ids), len(out[vol]), un == 0 and rep == 0, vol_m3))
    return data, rows


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("python", type=Path)
    parser.add_argument("native", type=Path)
    parser.add_argument("--report", default="tessellation.json")
    args = parser.parse_args()
    da, ra = per_volume(args.python / "solids.facets.bin")
    db, rb = per_volume(args.native / "solids.facets.bin")
    same_bytes = (args.python / "solids.facets.bin").read_bytes() == (args.native / "solids.facets.bin").read_bytes()
    mismatches = 0
    worst_vertex = 0.0
    if len(ra) != len(rb):
        print(f"MISMATCH volumes: python {len(ra)} native {len(rb)}")
        return 1
    for x, y in zip(ra, rb):
        if x[:5] != y[:5]:
            mismatches += 1
            print("MISMATCH solid", x[0], "python", x[1:5], "native", y[1:5])
    if len(da["vertices"]) == len(db["vertices"]):
        worst_vertex = float(np.abs(np.array(da["vertices"]) - np.array(db["vertices"])).max())
    closed = sum(1 for r in rb if r[4])
    print(f"solids {len(rb)}; python triangles {sum(r[2] for r in ra)} native {sum(r[2] for r in rb)}; "
          f"closed python {sum(1 for r in ra if r[4])} native {closed}; per-solid mismatches {mismatches}; "
          f"facets byte-identical {same_bytes}; max vertex difference {worst_vertex:.3e} m")
    return 1 if mismatches else 0


if __name__ == "__main__":
    sys.exit(main())
