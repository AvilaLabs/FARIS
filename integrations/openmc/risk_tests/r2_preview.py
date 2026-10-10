#!/usr/bin/env python3
"""R2 (PERF-025): the 60 s laptop preview on RM-M, OpenMC 0.15.3. Run alone.

    r2_preview.py --model-dir D --windows W.h5 --run-dir R --out result.json

The model (with the R1 windows, source strength 1) is written first and is not timed. The openmc executable is then
started with 8 threads and stopped by process group 60 s after its process start, so cross-section and window loading
count. Batches are 1000 histories and every batch writes a statepoint; the last one gives TBR and response (a).

TBR is the H3-production rate summed over every lithium-bearing material, per source neutron. Response (a) is the TF
fast flux (E > 0.1 MeV) over the TF coil material, in cm per source neutron.
Rule: PASS if the TBR relative error is at most 2 % and response (a) at most 20 % (1 sigma).
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import resource
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import r1_worker  # noqa: E402
import rm_m_checks as checks  # noqa: E402
import rm_m_openmc as RM  # noqa: E402

TBR_LIMIT = 0.02
RESPONSE_A_LIMIT = 0.20
BUDGET_S = 60.0
BATCH = 1000


def rule(tbr_r, a_r) -> dict:
    ok_t = tbr_r is not None and tbr_r <= TBR_LIMIT
    ok_a = a_r is not None and a_r <= RESPONSE_A_LIMIT
    return {"tbr_ok": ok_t, "a_ok": ok_a, "verdict": "PASS" if ok_t and ok_a else "FAIL",
            "rule": "TBR R <= 2 % and response (a) R <= 20 % at 60 s (1 sigma)"}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--model-dir", required=True, type=Path)
    ap.add_argument("--windows", required=True, type=Path)
    ap.add_argument("--run-dir", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--threads", type=int, default=8)
    ap.add_argument("--budget", type=float, default=BUDGET_S)
    ap.add_argument("--seed", type=int, default=20261401)
    args = ap.parse_args()
    import openmc

    run_dir = args.run_dir
    run_dir.mkdir(parents=True, exist_ok=True)
    for old in run_dir.glob("statepoint.*.h5"):
        old.unlink()
    roles = RM.load_roles(args.model_dir)
    model, _ = RM.build_model(openmc, args.model_dir / "rm_m.h5m", roles)
    source, _ = RM.tokamak_mesh_source(openmc, RM.role_ids(roles)["plasma"], rate=1.0)
    s = model.settings
    max_batches = 100000
    s.run_mode = "fixed source"
    s.batches, s.particles, s.seed = max_batches, BATCH, args.seed
    s.source = [source]
    s.photon_transport = False
    s.statepoint = {"batches": list(range(1, max_batches + 1))}
    s.sourcepoint = {"write": False}
    s.max_lost_particles = 10000
    s.rel_max_lost_particles = 0.5
    s.temperature = {"default": RM.DATA_TEMPERATURE_K, "method": "nearest", "tolerance": 1.0}
    s.output = {"summary": False, "tallies": False}
    wws = openmc.hdf5_to_wws(str(args.windows))
    for ww in wws:
        ww.mesh.id = 9000  # the file's mesh id can collide with the source mesh
    s.weight_windows = wws
    s.weight_windows_on = True
    li_mats = [m for m in model.materials if {"Li6", "Li7"} & set(m.get_nuclides())]
    tbr = openmc.Tally(name="tbr")
    tbr.filters = [openmc.MaterialFilter(li_mats)]
    tbr.scores = ["H3-production"]
    model.tallies = openmc.Tallies([tbr, r1_worker.response_a_material_tally(openmc, model)])
    model.export_to_model_xml(str(run_dir))

    os.environ["OMP_NUM_THREADS"] = str(args.threads)
    run = r1_worker.run_budgeted(run_dir, args.threads, args.budget)
    files = sorted(run_dir.glob("statepoint.*.h5"), key=lambda p: int(p.stem.split(".")[1]))
    record = {"kind": "r2-preview", "openmc_version": openmc.__version__, "threads": args.threads, "budget_s": args.budget,
              "seed": args.seed, "particles_per_batch": BATCH, "source_strength": 1.0, "windows": str(args.windows),
              "windows_sha256": RM.sha256(args.windows), "lithium_materials": [m.name for m in li_mats], "run": run}
    if not files:
        record["error"] = "no statepoint written inside the budget"
    else:
        first, last = files[0], files[-1]
        record["batches"] = int(last.stem.split(".")[1])
        record["histories"] = record["batches"] * BATCH
        record["seconds_to_first_batch"] = first.stat().st_mtime - run["t_start"]
        record["seconds_to_last_statepoint"] = last.stat().st_mtime - run["t_start"]
        with openmc.StatePoint(str(last)) as sp:
            t = sp.get_tally(name="tbr")
            a = sp.get_tally(name="tf_fast_a")
            tm, ts = float(t.mean.sum()), float((t.std_dev ** 2).sum() ** 0.5)
            am, asd = float(a.mean.ravel()[0]), float(a.std_dev.ravel()[0])
        record["tbr"] = {"mean": tm, "std_error": ts, "relative_error": ts / tm if tm > 0 else None, "unit": "tritons per source neutron"}
        record["a"] = {"mean": am, "std_error": asd, "relative_error": asd / am if am > 0 else None, "unit": checks.UNIT_PER_SOURCE}
        record["rule"] = rule(record["tbr"]["relative_error"], record["a"]["relative_error"])
        for p in files[:-1]:
            p.unlink()
    record["peak_rss_mb"] = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss / 1024.0
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(record, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")
    print(json.dumps({k: record.get(k) for k in ("batches", "histories", "seconds_to_first_batch", "seconds_to_last_statepoint", "tbr", "a", "rule")}, indent=1, default=str))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
