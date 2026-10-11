#!/usr/bin/env python3
"""S0e window generation (E2 and E-smoke) on OpenMC 0.16.0: S0d T2's CADIS windows on the cylindrical mesh.

    s0e_generate.py --mgxs G6/mgxs.h5 --model-dir D --run-dir R --out gen.json --seed N [--smoke]

Protocol: docs/notes/CAD_TRANSPORT_S0E.md. This is s0d_generate.py in cadis mode with --mesh cylindrical: the H6 mesh
(s0e_common.cylindrical_edges) serves both as the random-ray source-region mesh and as the WeightWindowGenerator mesh, the
adjoint source is T2's, and G6's mgxs.h5 is reused. The generation record carries the mesh type, bin counts and the
sha256 of the edge lists. --smoke runs two batches, one inactive.
"""
from __future__ import annotations

import argparse
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import s0d_generate as G  # noqa: E402


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--mgxs", required=True, type=Path, help="S0b G6's mgxs.h5")
    ap.add_argument("--model-dir", required=True, type=Path)
    ap.add_argument("--run-dir", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--seed", required=True, type=int)
    ap.add_argument("--threads", type=int, default=4)
    ap.add_argument("--smoke", action="store_true", help="E-smoke: two batches, one inactive")
    args = ap.parse_args(argv)
    args.mode, args.mesh = "cadis", "cylindrical"
    return G.generate(args)


if __name__ == "__main__":
    sys.exit(main())
