#!/usr/bin/env python3
"""E-smoke check: a window file's mesh is the H6 cylindrical mesh. OpenMC 0.15.3 (or 0.16.0).

    s0e_check_windows.py WINDOWS.h5 --out check.json

Exit 0 when the first window set's mesh is a CylindricalMesh whose R, Z and phi edges equal s0e_common.cylindrical_edges()
(relative 1e-9). Protocol: docs/notes/CAD_TRANSPORT_S0E.md, run 0.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import s0e_common as E  # noqa: E402


def compare(found: dict, expected: dict, rtol: float = 1e-9) -> dict:
    """Edge lists equal within rtol; returns {axis: bool} and the largest relative difference."""
    ok, worst = {}, 0.0
    for axis in ("r_cm", "z_cm", "phi_rad"):
        a, b = list(found[axis]), list(expected[axis])
        same = len(a) == len(b) and all(abs(x - y) <= rtol * max(1.0, abs(y)) for x, y in zip(a, b))
        ok[axis] = same
        if len(a) == len(b):
            worst = max([worst] + [abs(x - y) / max(1.0, abs(y)) for x, y in zip(a, b)])
    return {"equal": ok, "all_equal": all(ok.values()), "worst_relative_difference": worst}


def check(path: Path) -> dict:
    import openmc

    out = {"file": str(path), "openmc_version": openmc.__version__}
    wws = openmc.hdf5_to_wws(str(path))
    mesh = wws[0].mesh
    out["mesh_class"] = type(mesh).__name__
    out["cylindrical"] = isinstance(mesh, openmc.CylindricalMesh)
    if out["cylindrical"]:
        found = {"r_cm": [float(x) for x in mesh.r_grid], "z_cm": [float(x) for x in mesh.z_grid],
                 "phi_rad": [float(x) for x in mesh.phi_grid]}
        out["edges_sha256_found"] = E.edges_sha256(found)
        out["edges_sha256_expected"] = E.edges_sha256(E.cylindrical_edges())
        out.update(compare(found, E.cylindrical_edges()))
        out["origin"] = [float(x) for x in mesh.origin]
    out["passed"] = bool(out["cylindrical"] and out.get("all_equal"))
    return out


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("windows", type=Path)
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args(argv)
    result = check(args.windows)
    args.out.write_text(json.dumps(result, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(result, indent=1, sort_keys=True))
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
