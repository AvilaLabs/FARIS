#!/usr/bin/env python3
"""S0c Q2: what the 60 s preview spends its time on (PERF-025). RM-M, G6 windows, 8 threads, nothing else running.

    s0c_q2.py prepare --model-dir D --windows W.h5 --run-dir R
    s0c_q2.py run     --run-dir R --out record.json [--previews 2] [--budget 60] [--threads 8]

`prepare` writes the model exactly as r2_preview.py does (G6 windows with `max_split` 1000 and surface and collision
checkpoints, 1000-history batches, no statepoints) with a new seed. `run` is one Python process with openmc.lib:

  cold      time `init`, `simulation_init` and the first batch separately; then `simulation_finalize`.
  warm x N  `reset`, `simulation_init`, then `next_batch` until `--budget` seconds have passed since `init` returned (the preview
            began there; `simulation_init` counts, `init` does not), then TBR and (a) with their R from the tallies through
            openmc.lib. Every preview uses the same seed.

The cold start and the warm previews share the process, so the warm previews run in a loaded session. Pure rule:
s0c_common.q2_rule (TBR R <= 2 % and (a) R <= 20 % for every warm preview).
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import resource
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
import r1_worker  # noqa: E402
import rm_m_checks as checks  # noqa: E402
import rm_m_openmc as RM  # noqa: E402
import s0b_common as C  # noqa: E402
import s0c_common as S  # noqa: E402

BATCH = 1000
MAX_SPLIT = 1000


def prepare(args) -> int:
    import openmc

    run_dir = args.run_dir
    run_dir.mkdir(parents=True, exist_ok=True)
    roles = RM.load_roles(args.model_dir)
    model, _ = RM.build_model(openmc, args.model_dir / "rm_m.h5m", roles)
    source, _ = RM.tokamak_mesh_source(openmc, RM.role_ids(roles)["plasma"], rate=1.0)
    s = model.settings
    s.run_mode = "fixed source"
    s.batches, s.particles, s.seed = 100000, BATCH, S.SEEDS["q2"]
    s.source = [source]
    s.photon_transport = False
    s.statepoint = {"batches": []}
    s.sourcepoint = {"write": False}
    s.max_lost_particles = 10000
    s.rel_max_lost_particles = 0.5
    s.temperature = {"default": RM.DATA_TEMPERATURE_K, "method": "nearest", "tolerance": 1.0}
    s.output = {"summary": False, "tallies": False}
    wws = openmc.hdf5_to_wws(str(args.windows))
    for ww in wws:
        ww.mesh.id = 9000
        ww.max_split = MAX_SPLIT
    s.weight_windows = wws
    s.weight_windows_on = True
    s.weight_window_checkpoints = {"surface": True, "collision": True}
    li_mats = [m for m in model.materials if {"Li6", "Li7"} & set(m.get_nuclides())]
    tbr = openmc.Tally(name="tbr")
    tbr.filters = [openmc.MaterialFilter(li_mats)]
    tbr.scores = ["H3-production"]
    model.tallies = openmc.Tallies([tbr, r1_worker.response_a_material_tally(openmc, model)])
    model.export_to_model_xml(str(run_dir))
    (run_dir / "prepare.json").write_text(json.dumps({"seed": S.SEEDS["q2"], "windows": str(args.windows), "windows_sha256": RM.sha256(args.windows),
                                                      "max_split": MAX_SPLIT, "checkpoints": "surface and collision", "particles_per_batch": BATCH,
                                                      "lithium_materials": [m.name for m in li_mats], "library": RM.library_identity()},
                                                     indent=1, default=str) + "\n", encoding="utf-8")
    return 0


def tally_ids(model_xml: Path) -> dict:
    """Tally ids by name from the model XML (openmc.lib tallies carry no name in 0.15.3)."""
    import re

    text = model_xml.read_text(encoding="utf-8")
    return {name: int(i) for i, name in re.findall(r'<tally id="(\d+)" name="(\w+)"', text)}


def read_tallies(lib, ids: dict) -> dict:
    """TBR (summed over lithium materials) and (a) from the live tallies: mean, std_error, relative_error, realizations."""
    out = {}
    for key, name in (("tbr", "tbr"), ("a", "tf_fast_a")):
        if name not in ids:
            continue
        t = lib.tallies[ids[name]]
        mean, sd = t.mean.ravel(), t.std_dev.ravel()
        m = float(mean.sum())
        e = float((sd ** 2).sum() ** 0.5)
        out[key] = {"mean": m, "std_error": e, "relative_error": e / m if m > 0 else None, "realizations": int(t.num_realizations)}
    return out


def run(args) -> int:
    import openmc.lib as lib

    os.chdir(args.run_dir)
    os.environ["OMP_NUM_THREADS"] = str(args.threads)
    rec = {"kind": "s0c-q2", "threads": args.threads, "affinity_cpus": sorted(os.sched_getaffinity(0)), "seed": S.SEEDS["q2"],
           "particles_per_batch": BATCH, "budget_s": args.budget, "loadavg_start": C.read_loadavg(), "warm": []}
    ids = tally_ids(args.run_dir / "model.xml")
    rec["tally_ids"] = ids
    p0 = time.perf_counter()
    lib.init(["--threads", str(args.threads)])
    t_init = time.perf_counter() - p0
    t0 = time.perf_counter()
    lib.simulation_init()
    t_siminit = time.perf_counter() - t0
    t0 = time.perf_counter()
    lib.next_batch()
    t_first = time.perf_counter() - t0
    rec["cold"] = {"init_s": t_init, "simulation_init_s": t_siminit, "first_batch_s": t_first,
                   "process_to_first_batch_s": time.perf_counter() - p0}
    rec["cold"].update({"tallies_after_first_batch": read_tallies(lib, ids)})
    lib.simulation_finalize()
    for k in range(args.previews):
        lib.reset()
        began = time.perf_counter()
        lib.simulation_init()
        t_si = time.perf_counter() - began
        batches, first = 0, None
        while True:
            t1 = time.perf_counter()
            lib.next_batch()
            batches += 1
            if first is None:
                first = time.perf_counter() - t1
            if time.perf_counter() - began >= args.budget:
                break
        elapsed = time.perf_counter() - began
        tallies = read_tallies(lib, ids)
        lib.simulation_finalize()
        rec["warm"].append({"preview": k + 1, "simulation_init_s": t_si, "first_batch_s": first, "elapsed_s": elapsed, "batches": batches,
                            "histories": batches * BATCH, "tbr": tallies.get("tbr"), "a": tallies.get("a"),
                            "tbr_r": (tallies.get("tbr") or {}).get("relative_error"), "a_r": (tallies.get("a") or {}).get("relative_error")})
    rec["loadavg_end"] = C.read_loadavg()
    rec["rule"] = S.q2_rule([{"tbr_r": w["tbr_r"], "a_r": w["a_r"]} for w in rec["warm"]])
    rec["peak_rss_mb"] = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024.0
    lib.finalize()   # prints the timing summary into the log (cross-section reading, initialization)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(rec, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")
    print(json.dumps({k: rec[k] for k in ("cold", "warm", "rule")}, indent=1, default=str))
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("prepare")
    p.add_argument("--model-dir", required=True, type=Path)
    p.add_argument("--windows", required=True, type=Path)
    p.add_argument("--run-dir", required=True, type=Path)
    p = sub.add_parser("run")
    p.add_argument("--run-dir", required=True, type=Path)
    p.add_argument("--out", required=True, type=Path)
    p.add_argument("--threads", type=int, default=8)
    p.add_argument("--previews", type=int, default=2)
    p.add_argument("--budget", type=float, default=60.0)
    args = ap.parse_args()
    return {"prepare": prepare, "run": run}[args.cmd](args)


if __name__ == "__main__":
    raise SystemExit(main())
