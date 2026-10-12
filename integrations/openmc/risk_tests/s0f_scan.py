#!/usr/bin/env python3
"""S0f production run: one configuration and seed of RM-M, batch by batch through openmc.lib, with a JSON record.

    s0f_scan.py --base-model BASE.xml --run-dir D --config F0|F1 --seed S [--threads 4]

The base model is p1-fr's model.xml (windows inline). The run writes D/model.xml (seed, 1,000 histories per batch, 3,000
batches, no statepoints except the last, the side-face box tally), then D/batches.json and D/record.json. Which OpenMC
library runs is decided by the environment this script starts in (F0: the stock package; F1: the patched shadow package
with OPENMC_WW_SKIP_VOID=1 and OPENMC_APERTURE_BIAS); the record carries the library path, its sha256 and the variables.
Protocol: docs/notes/CAD_TRANSPORT_S0F.md, "Protocol".
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import s0f_common as F  # noqa: E402

STRIP_TALLY = """
    <mesh id="9101"><dimension>1 2 1</dimension><lower_left>575.0 -50.0 -30.0</lower_left><upper_right>655.0 50.0 30.0</upper_right></mesh>
    <filter id="101" type="mesh"><bins>9101</bins></filter>
    <filter id="102" type="material"><bins>4</bins></filter>
    <filter id="103" type="energy"><bins>100000.0 20000000.0</bins></filter>
    <tally id="101" name="strip"><filters>101 102 103</filters><scores>flux</scores></tally>
  </tallies>"""


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def write_model(base: Path, run_dir: Path, seed: int, batches: int) -> Path:
    x = base.read_text()
    x = re.sub(r"<particles>\d+</particles>", f"<particles>{F.PER_BATCH}</particles>", x, count=1)
    x = re.sub(r"<batches>\d+</batches>", f"<batches>{batches}</batches>", x, count=1)
    x = re.sub(r"<seed>\d+</seed>", f"<seed>{seed}</seed>", x, count=1)
    x = re.sub(r"<state_point>.*?</state_point>", f"<state_point><batches>{batches}</batches></state_point>", x,
               count=1, flags=re.S)
    i = x.rindex("</tallies>")
    x = x[:i] + STRIP_TALLY.lstrip("\n") + x[i + len("</tallies>"):]
    run_dir.mkdir(parents=True, exist_ok=True)
    out = run_dir / "model.xml"
    out.write_text(x)
    return out


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--base-model", required=True, type=Path)
    p.add_argument("--run-dir", required=True, type=Path)
    p.add_argument("--config", required=True, choices=sorted(F.CONFIG_ENV))
    p.add_argument("--seed", required=True, type=int)
    p.add_argument("--threads", type=int, default=4)
    p.add_argument("--batches", type=int, default=F.BATCHES, help="smoke tests only; production uses the protocol's 3,000")
    args = p.parse_args()
    env = {k: os.environ.get(k) for k in F.PATCH_VARIABLES}
    if not F.env_matches(args.config, env):
        raise SystemExit(f"{args.config} needs exactly these FARIS variables: {F.CONFIG_ENV[args.config]}")
    model = write_model(args.base_model, args.run_dir, args.seed, args.batches)
    load_start = Path("/proc/loadavg").read_text().strip()
    t0 = time.monotonic()
    os.chdir(args.run_dir)
    import numpy as np
    import openmc.lib as L
    lib_path = Path(L._dll._name).resolve()
    if args.config == "F0" and "openmc-presplit" in str(lib_path):
        raise SystemExit("F0 must run on the stock library")
    if args.config != "F0" and "openmc-presplit" not in str(lib_path):
        raise SystemExit(f"{args.config} must run on the patched library")
    L.init(["-s", str(args.threads)])
    L.simulation_init()
    a_tally = L.tallies[2]
    s_tally = L.tallies[101]
    a, strip = [], []
    prev_a, prev_s = 0.0, np.zeros(2)
    while True:
        status = L.next_batch()
        ra = float(a_tally.results[..., 1].sum())
        rs = s_tally.results[..., 1].reshape(-1)[:2].copy()
        a.append(ra - prev_a)
        strip.append((rs - prev_s).tolist())
        prev_a, prev_s = ra, rs
        if status != 0:
            break
    L.simulation_finalize()
    L.finalize()
    seconds = time.monotonic() - t0
    load_end = Path("/proc/loadavg").read_text().strip()
    Path("batches.json").write_text(json.dumps({"a": a, "strip": strip}))
    st = F.batch_stats(a, F.PER_BATCH)
    record = {"kind": "s0f-scan", "config": args.config, "seed": args.seed, "batches": len(a), "per_batch": F.PER_BATCH,
              "histories": len(a) * F.PER_BATCH, "seconds_from_start": seconds, "histories_per_second": len(a) * F.PER_BATCH / seconds,
              "a": st, "fom": F.fom(st["relative_error"], seconds), "library": str(lib_path), "library_sha256": sha256(lib_path),
              "env": env, "base_model": str(args.base_model), "base_model_sha256": sha256(args.base_model),
              "model_sha256": sha256(model), "batches_sha256": sha256(Path("batches.json")), "threads": args.threads,
              "affinity_cpus": sorted(os.sched_getaffinity(0)), "loadavg_start": load_start, "loadavg_end": load_end,
              "python": sys.executable}
    Path("record.json").write_text(json.dumps(record, indent=1))
    print(json.dumps({k: record[k] for k in ("config", "seed", "histories", "seconds_from_start", "fom")}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
