#!/usr/bin/env python3
"""Compares the native batch face classifier with the Python per-point loop on the same points.

    classify_parity.py STEP --worker PATH --total-points N [--solids 0,1,..] [--seed S] [--scratch DIR]

For every face of the chosen solids (default: all), N / (number of faces) random (u, v)
points are drawn uniformly from the face's UV box widened by 5 % on each side. Python
classifies them one point at a time with BRepTopAdaptor_FClass2d, as faris_deviation.py's
exact_pairs does; the worker classifies each face's points in one native call. The script
prints both wall times and the number of points whose states differ.

Run with the CAD environment (cadquery / OCP) under the memory cap.
"""
import argparse
import json
import subprocess
import sys
import time
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "integrations" / "cad"))
import faris_cad  # noqa: E402

TOLERANCE = 1.0e-7  # faris_deviation.CLASSIFY_TOLERANCE


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("step", type=Path)
    parser.add_argument("--worker", type=Path, required=True)
    parser.add_argument("--total-points", type=int, required=True)
    parser.add_argument("--solids", default=None, help="comma-separated solid indices (default all)")
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--scratch", type=Path, required=True)
    parser.add_argument("--skip-python", action="store_true", help="time the native side only")
    args = parser.parse_args()
    args.scratch.mkdir(parents=True, exist_ok=True)

    from OCP.BRepTools import BRepTools
    from OCP.BRepTopAdaptor import BRepTopAdaptor_FClass2d
    from OCP.TopAbs import TopAbs_FACE, TopAbs_IN, TopAbs_ON
    from OCP.TopExp import TopExp_Explorer
    from OCP.TopoDS import TopoDS
    from OCP.gp import gp_Pnt2d

    t = time.monotonic()
    _reader, solids, _named = faris_cad.read_step(args.step)
    print(f"python read_step: {time.monotonic() - t:.1f} s, {len(solids)} solids", flush=True)
    chosen = list(range(len(solids))) if args.solids is None else [int(x) for x in args.solids.split(",")]
    faces = []  # (solid, face id, face)
    for s in chosen:
        explorer = TopExp_Explorer(solids[s], TopAbs_FACE)
        number = 0
        while explorer.More():
            number += 1
            faces.append((s, number, TopoDS.Face_s(explorer.Current())))
            explorer.Next()
    per_face = max(1, args.total_points // len(faces))
    rng = np.random.default_rng(args.seed)
    batches, points = [], []
    for s, number, face in faces:
        u0, u1, v0, v1 = BRepTools.UVBounds_s(face)
        du, dv = 0.05 * (u1 - u0), 0.05 * (v1 - v0)
        u = rng.uniform(u0 - du, u1 + du, per_face)
        v = rng.uniform(v0 - dv, v1 + dv, per_face)
        pts = args.scratch / f"s{s}f{number}.points"
        out = args.scratch / f"s{s}f{number}.states"
        np.column_stack([u, v]).astype("<f8").tofile(pts)
        batches.append({"solid": s, "face": number, "points": str(pts), "states": str(out)})
        points.append((u, v))
    total = per_face * len(faces)
    print(f"{len(faces)} faces, {per_face} points each, {total} points", flush=True)

    request = {
        "schema": "faris-occt-request/v1",
        "limits": {"address_space_mb": 8192, "cpu_seconds": 3600},
        "op": "classify",
        "step": str(args.step),
        "tolerance": TOLERANCE,
        "batches": batches,
    }
    request_path, response_path = args.scratch / "request.json", args.scratch / "response.json"
    request_path.write_text(json.dumps(request))
    t = time.monotonic()
    subprocess.run([str(args.worker), str(request_path), str(response_path)], check=True)
    wall = time.monotonic() - t
    result = json.loads(response_path.read_text())["result"]
    print(f"native: worker wall {wall:.2f} s (STEP read {result['read_seconds']:.2f} s, "
          f"classification {result['classify_seconds']:.3f} s for {result['total_points']} points; "
          f"peak RSS {result['peak_rss_mb']:.0f} MiB)", flush=True)
    if args.skip_python:
        return 0

    accepted = (TopAbs_IN.value, TopAbs_ON.value)
    code = {TopAbs_IN.value: 0, TopAbs_ON.value: 1}
    differ = 0
    python_seconds = 0.0
    counts = np.zeros(4, dtype=np.int64)
    probe = gp_Pnt2d(0.0, 0.0)
    for (s, number, face), (u, v), batch in zip(faces, points, batches):
        native = np.fromfile(batch["states"], dtype=np.uint8)
        t = time.monotonic()
        classifier = BRepTopAdaptor_FClass2d(face, TOLERANCE)
        states = np.empty(len(u), dtype=np.uint8)
        for i, (x, y) in enumerate(zip(u.tolist(), v.tolist())):
            probe.SetCoord(x, y)
            value = classifier.Perform(probe).value
            # TopAbs_State: IN 0, OUT 1, ON 2, UNKNOWN 3  ->  native codes 0, 2, 1, 3
            states[i] = {0: 0, 1: 2, 2: 1, 3: 3}[value]
        python_seconds += time.monotonic() - t
        differ += int((states != native).sum())
        counts += np.bincount(native, minlength=4)
    print(f"python per-point loop: {python_seconds:.1f} s for {total} points")
    print(f"states in/on/out/unknown: {counts.tolist()}")
    print(f"points whose state differs between python and native: {differ}")
    return 0 if differ == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
