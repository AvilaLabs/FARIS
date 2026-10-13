#!/usr/bin/env python3
"""R3' (docs/notes/CAD_TRANSPORT_RISK_TESTS.md): rerun R3's D1S model file on another OpenMC 0.15.3 build.

    r3p_d1s_rerun.py --r3-d1s DIR --out-dir R --seed N [--budget S]

Takes DIR/run/model.xml and DIR/d1s_record.json from R3 as they are, changes only the seed, runs the openmc
executable that sits beside this interpreter for the wall budget, and writes the same outputs as r3_d1s.py
(dose_time_*.npz and d1s_record.json) with the same time correction. The model is not rebuilt, so the plasma-source
package is not needed.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import r1_worker  # noqa: E402
import r3_scenario as sc  # noqa: E402

CHAIN = Path.home() / "nuclear-data/p32-work/chain/depletion/chain.xml"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--r3-d1s", required=True, type=Path)
    ap.add_argument("--out-dir", required=True, type=Path)
    ap.add_argument("--seed", required=True, type=int)
    ap.add_argument("--budget", type=float, default=1500.0)
    ap.add_argument("--threads", type=int, default=4)
    args = ap.parse_args()
    import numpy as np
    import openmc
    from openmc.deplete import d1s

    os.environ["OPENMC_CHAIN_FILE"] = str(CHAIN)
    openmc.config["chain_file"] = str(CHAIN)
    old = json.loads((args.r3_d1s / "d1s_record.json").read_text())
    nuclides, particles = old["radionuclides"], old["particles_per_batch"]
    src = (args.r3_d1s / "run" / "model.xml").read_text()
    x, n = re.subn(r"<seed>\d+</seed>", f"<seed>{args.seed}</seed>", src, count=1)
    if n != 1:
        raise SystemExit("the model file has no seed to change")
    run_dir = args.out_dir / "run"
    run_dir.mkdir(parents=True, exist_ok=True)
    if any(run_dir.glob("statepoint.*.h5")):
        raise SystemExit(f"{run_dir} already holds statepoints")
    (run_dir / "model.xml").write_text(x)
    record = {"kind": "r3p-d1s", "openmc_version": openmc.__version__, "threads": args.threads, "chain": str(CHAIN),
              "seed": args.seed, "particles_per_batch": particles, "budget_s": args.budget,
              "source_model_sha256": hashlib.sha256(src.encode()).hexdigest(), "radionuclides": nuclides,
              "n_radionuclides": len(nuclides), "settings": old["settings"], "mesh": old["mesh"]}
    run = r1_worker.run_budgeted(run_dir, args.threads, args.budget)
    record["run"] = run
    files = sorted(run_dir.glob("statepoint.*.h5"), key=lambda p: int(p.stem.split(".")[1]))
    good = None
    for f in reversed(files):                         # the newest may be half-written when the budget ends
        try:
            with openmc.StatePoint(str(f)) as sp:
                sp.n_realizations
            good = f
            break
        except Exception:
            continue
    if good is None:
        record["error"] = f"no readable statepoint inside the budget (rc {run['returncode']})"
        (args.out_dir / "d1s_record.json").write_text(json.dumps(record, indent=1, default=str) + "\n")
        return 1
    for f in files:
        if f != good:
            f.unlink()
    record["batches"] = int(good.stem.split(".")[1])
    record["histories"] = record["batches"] * particles
    record["seconds_to_statepoint"] = good.stat().st_mtime - run["t_start"]
    tcf = d1s.time_correction_factors(nuclides, sc.TIMESTEPS, sc.SOURCE_RATES)
    record["photon"], per_nuclide_tot = {}, {}
    with openmc.StatePoint(str(good)) as sp:
        t = sp.get_tally(name="photon_dose")
        n_vox = int(np.prod(sc.MESH_DIMENSION))
        mean_all = t.mean.reshape(n_vox, len(nuclides))
        for idx in sc.PHOTON_TIME_INDICES:
            label = sc.COOLING_LABELS[idx]
            corrected = d1s.apply_time_correction(t, tcf, index=idx, sum_nuclides=True)
            mean = np.asarray(corrected.mean).reshape(sc.MESH_DIMENSION[::-1]).copy()
            std = np.asarray(corrected.std_dev).reshape(sc.MESH_DIMENSION[::-1]).copy()
            factors = np.array([tcf[nuc][idx] for nuc in nuclides])
            np.savez(args.out_dir / f"dose_time_{idx}.npz", mean=mean, std=std)
            by_nuc = (mean_all * factors).sum(axis=0)
            per_nuclide_tot[label] = {nuc: float(v) for nuc, v in zip(nuclides, by_nuc) if v > 0.0}
            record["photon"][label] = {"time_index": idx, "sum_pSv_cm_per_s": float(mean.sum())}
    record["dose_by_parent_nuclide_pSv_cm_per_s"] = {k: sc.top_contributors(v, 10) for k, v in per_nuclide_tot.items()}
    (args.out_dir / "d1s_record.json").write_text(json.dumps(record, indent=1, sort_keys=True, default=str) + "\n")
    print(json.dumps({"histories": record["histories"], "photon": record["photon"]}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
