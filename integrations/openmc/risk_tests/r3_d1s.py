#!/usr/bin/env python3
"""R3, direct one-step (D1S, openmc.deplete.d1s) on RM-M, OpenMC 0.15.3.

    r3_d1s.py --model-dir D --out-dir R [--particles N] [--budget S]

One coupled neutron and photon run with delayed (decay) photons sampled at the reaction site
(settings.use_decay_photons, chain from OPENMC_CHAIN_FILE). The dose tally carries a ParentNuclideFilter; the time
correction factors for 1 full-power year at 525 MW and cooling times of 1 d, 7 d and 30 d are applied afterwards.

The scored mesh is the R3 mesh. The tally also carries a MeshBornFilter with one bin over the same box, so only
photons born inside the box count; that is the region the mesh-based R2S activates, which keeps the two methods on
the same source domain. The neutron source strength is 1, so the tally is per source neutron and the time
correction factors carry the source rate: the result is pSv.cm/s per voxel, as for R2S.

The run is stopped by process group when the wall budget ends; the last statepoint is used.
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
import r3_scenario as sc  # noqa: E402
import rm_m_openmc as RM  # noqa: E402

CHAIN = Path.home() / "nuclear-data/p32-work/chain/depletion/chain.xml"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--model-dir", required=True, type=Path)
    ap.add_argument("--out-dir", required=True, type=Path)
    ap.add_argument("--particles", type=int, default=5000, help="source neutrons per batch")
    ap.add_argument("--max-batches", type=int, default=100000)
    ap.add_argument("--statepoint-every", type=int, default=2)
    ap.add_argument("--budget", type=float, default=7000.0, help="wall seconds for the transport run")
    ap.add_argument("--threads", type=int, default=4)
    ap.add_argument("--seed", type=int, default=20261301)
    args = ap.parse_args()
    import numpy as np
    import openmc
    import openmc.deplete
    from openmc.deplete import d1s

    os.environ["OMP_NUM_THREADS"] = str(args.threads)
    os.environ["OPENMC_CHAIN_FILE"] = str(CHAIN)
    os.environ["PATH"] = str(Path(sys.executable).parent) + os.pathsep + os.environ.get("PATH", "")
    openmc.config["chain_file"] = str(CHAIN)
    openmc.config["cross_sections"] = str(RM.FARIS_XS)
    out = args.out_dir
    out.mkdir(parents=True, exist_ok=True)

    from r3_r2s import dose_tally, scoring_mesh  # after the environment is set

    mesh = scoring_mesh(openmc)
    born = openmc.RegularMesh(name="r3-born-box")
    born.lower_left, born.upper_right, born.dimension = sc.MESH_LOWER_CM, sc.MESH_UPPER_CM, (1, 1, 1)
    roles = RM.load_roles(args.model_dir)
    ids = RM.role_ids(roles)
    model, _ = RM.build_model(openmc, args.model_dir / "rm_m.h5m", roles)
    source, _ = RM.tokamak_mesh_source(openmc, ids["plasma"], rate=1.0)
    s = model.settings
    s.run_mode = "fixed source"
    s.batches, s.particles, s.seed = args.max_batches, args.particles, args.seed
    s.source = [source]
    s.photon_transport = True
    s.use_decay_photons = True
    s.statepoint = {"batches": list(range(args.statepoint_every, args.max_batches + 1, args.statepoint_every))}
    s.sourcepoint = {"write": False}
    s.max_lost_particles = 10000
    s.rel_max_lost_particles = 0.5
    s.temperature = {"default": RM.DATA_TEMPERATURE_K, "method": "nearest", "tolerance": 1.0}
    s.output = {"summary": False, "tallies": False}
    tally = dose_tally(openmc, mesh)
    tally.filters.append(openmc.MeshBornFilter(born))
    model.tallies = openmc.Tallies([tally])
    nuclides = d1s.prepare_tallies(model, chain_file=str(CHAIN))
    run_dir = out / "run"
    run_dir.mkdir(parents=True, exist_ok=True)
    for old in run_dir.glob("statepoint.*.h5"):
        old.unlink()
    model.export_to_model_xml(str(run_dir))
    record = {"kind": "r3-d1s", "openmc_version": openmc.__version__, "threads": args.threads, "chain": str(CHAIN),
              "library": RM.library_identity(), "seed": args.seed, "particles_per_batch": args.particles, "budget_s": args.budget,
              "mesh": {"lower_left_cm": sc.MESH_LOWER_CM, "upper_right_cm": sc.MESH_UPPER_CM, "dimension": sc.MESH_DIMENSION},
              "radionuclides": nuclides, "n_radionuclides": len(nuclides),
              "settings": {"photon_transport": True, "use_decay_photons": True, "born_filter": "MeshBornFilter over the scored box (1 bin)",
                           "timesteps": sc.TIMESTEPS, "source_rates": sc.SOURCE_RATES},
              "workarounds": ["openmc.config['cross_sections'] set to the audited library", "OPENMC_CHAIN_FILE exported for the openmc executable"]}
    run = r1_worker.run_budgeted(run_dir, args.threads, args.budget)
    record["run"] = run
    files = sorted(run_dir.glob("statepoint.*.h5"), key=lambda p: int(p.stem.split(".")[1]))
    if not files:
        record["error"] = f"no statepoint inside the budget (rc {run['returncode']})"
        (out / "d1s_record.json").write_text(json.dumps(record, indent=1, default=str) + "\n", encoding="utf-8")
        return 1
    for old in files[:-1]:
        old.unlink()
    last = files[-1]
    record["batches"] = int(last.stem.split(".")[1])
    record["histories"] = record["batches"] * args.particles
    record["seconds_to_statepoint"] = last.stat().st_mtime - run["t_start"]
    tcf = d1s.time_correction_factors(nuclides, sc.TIMESTEPS, sc.SOURCE_RATES)
    record["photon"] = {}
    per_nuclide_tot = {}
    with openmc.StatePoint(str(last)) as sp:
        t = sp.get_tally(name="photon_dose")
        n_vox = int(np.prod(sc.MESH_DIMENSION))
        mean_all = t.mean.reshape(n_vox, len(nuclides))
        std_all = t.std_dev.reshape(n_vox, len(nuclides))
        record["parent_filter_order"] = [str(f.__class__.__name__) for f in t.filters]
        for idx in sc.PHOTON_TIME_INDICES:
            label = sc.COOLING_LABELS[idx]
            corrected = d1s.apply_time_correction(t, tcf, index=idx, sum_nuclides=True)
            mean = np.asarray(corrected.mean).reshape(sc.MESH_DIMENSION[::-1]).copy()
            std = np.asarray(corrected.std_dev).reshape(sc.MESH_DIMENSION[::-1]).copy()
            factors = np.array([tcf[n][idx] for n in nuclides])
            manual = (mean_all * factors).sum(axis=1)
            if not np.allclose(manual.reshape(mean.shape), mean, rtol=1e-9, atol=0.0):
                record.setdefault("warnings", []).append(f"manual and library time-corrected sums differ at {label}")
            np.savez(out / f"dose_time_{idx}.npz", mean=mean, std=std)
            by_nuc = (mean_all * factors).sum(axis=0)
            per_nuclide_tot[label] = {n: float(v) for n, v in zip(nuclides, by_nuc) if v > 0.0}
            record["photon"][label] = {"time_index": idx, "sum_pSv_cm_per_s": float(mean.sum()),
                                       "unit": "pSv.cm/s per voxel (track length x dose coefficient x time-corrected source rate)"}
    record["dose_by_parent_nuclide_pSv_cm_per_s"] = {k: sc.top_contributors(v, 10) for k, v in per_nuclide_tot.items()}
    record["peak_rss_mb"] = max(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss, resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss) / 1024.0
    (out / "d1s_record.json").write_text(json.dumps(record, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
