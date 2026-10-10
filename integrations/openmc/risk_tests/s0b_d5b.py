#!/usr/bin/env python3
"""S0b D5b: where D1S's time goes. Four RM-M configurations, 300 s each, 4 threads. OpenMC 0.15.3 (the D1 build).

    s0b_d5b.py --mode 1..4 --model-dir D --run-dir R --out record.json [--budget 300] [--seed S]

  1  neutron-only analog (photon transport off)
  2  photon transport on, prompt photons only (no decay photons), no tallies
  3  D1S decay photons on (use_decay_photons), no tallies
  4  S0's full D1S tally: photon dose on the R3 mesh x ParentNuclideFilter x MeshBornFilter

The run is stopped at the budget. A statepoint is written every batch; the rate is the particles per second between the first and
the last statepoint (start-up excluded) and, for reference, histories over the time of the last statepoint (start-up included).
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
import r3_scenario as sc  # noqa: E402
import rm_m_openmc as RM  # noqa: E402
import s0b_common as C  # noqa: E402

CHAIN = Path.home() / "nuclear-data/p32-work/chain/depletion/chain.xml"
PARTICLES = {1: 1000, 2: 1000, 3: 200, 4: 200}
LABELS = {1: "neutron-only analog", 2: "photon transport, no decay photons", 3: "D1S decay photons, no tallies",
          4: "S0's full D1S tally (mesh x parent nuclide x mesh-born)"}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--mode", type=int, required=True, choices=[1, 2, 3, 4])
    ap.add_argument("--model-dir", required=True, type=Path)
    ap.add_argument("--run-dir", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--budget", type=float, default=300.0)
    ap.add_argument("--threads", type=int, default=4)
    ap.add_argument("--seed", type=int, default=90070001)
    args = ap.parse_args()
    import numpy as np  # noqa: F401
    import openmc
    import openmc.deplete  # noqa: F401
    from openmc.deplete import d1s

    os.environ["OMP_NUM_THREADS"] = str(args.threads)
    os.environ["OPENMC_CHAIN_FILE"] = str(CHAIN)
    os.environ["PATH"] = str(Path(sys.executable).parent) + os.pathsep + os.environ.get("PATH", "")
    openmc.config["chain_file"] = str(CHAIN)
    openmc.config["cross_sections"] = str(RM.FARIS_XS)
    run_dir = args.run_dir
    run_dir.mkdir(parents=True, exist_ok=True)
    for old in run_dir.glob("statepoint.*.h5"):
        old.unlink()
    roles = RM.load_roles(args.model_dir)
    ids = RM.role_ids(roles)
    model, _ = RM.build_model(openmc, args.model_dir / "rm_m.h5m", roles)
    source, _ = RM.tokamak_mesh_source(openmc, ids["plasma"], rate=1.0)
    particles = PARTICLES[args.mode]
    max_batches = 100000
    s = model.settings
    s.run_mode = "fixed source"
    s.batches, s.particles, s.seed = max_batches, particles, args.seed
    s.source = [source]
    s.photon_transport = args.mode >= 2
    s.use_decay_photons = args.mode >= 3
    s.statepoint = {"batches": list(range(1, max_batches + 1))}
    s.sourcepoint = {"write": False}
    s.max_lost_particles = 10000
    s.rel_max_lost_particles = 0.5
    s.temperature = {"default": RM.DATA_TEMPERATURE_K, "method": "nearest", "tolerance": 1.0}
    s.output = {"summary": False, "tallies": False}
    nuclides = None
    if args.mode == 4:
        from r3_r2s import dose_tally, scoring_mesh

        mesh = scoring_mesh(openmc)
        born = openmc.RegularMesh(name="r3-born-box")
        born.lower_left, born.upper_right, born.dimension = sc.MESH_LOWER_CM, sc.MESH_UPPER_CM, (1, 1, 1)
        tally = dose_tally(openmc, mesh)
        tally.filters.append(openmc.MeshBornFilter(born))
        model.tallies = openmc.Tallies([tally])
        nuclides = d1s.prepare_tallies(model, chain_file=str(CHAIN))
    model.export_to_model_xml(str(run_dir))
    record = {"kind": "s0b-d5b", "mode": args.mode, "label": LABELS[args.mode], "openmc_version": openmc.__version__,
              "python": sys.executable, "threads": args.threads, "affinity_cpus": sorted(os.sched_getaffinity(0)), "seed": args.seed,
              "particles_per_batch": particles, "budget_s": args.budget, "photon_transport": bool(s.photon_transport),
              "use_decay_photons": bool(s.use_decay_photons), "radionuclides_in_filter": len(nuclides) if nuclides else None,
              "loadavg_start": C.read_loadavg()}
    run = r1_worker.run_budgeted(run_dir, args.threads, args.budget)
    record["loadavg_end"] = C.read_loadavg()
    record["run"] = run
    record["peak_rss_mb_children"] = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss / 1024.0
    files = sorted(run_dir.glob("statepoint.*.h5"), key=lambda p: int(p.stem.split(".")[1]))
    if len(files) < 2:
        record["error"] = f"only {len(files)} statepoints inside the budget"
    else:
        # a statepoint cut off mid-write is skipped
        usable = []
        for p in files:
            if p.stat().st_size > 0:
                usable.append(p)
        first, last = usable[0], usable[-1]
        b1, b2 = int(first.stem.split(".")[1]), int(last.stem.split(".")[1])
        t1, t2 = first.stat().st_mtime - run["t_start"], last.stat().st_mtime - run["t_start"]
        record["first_statepoint"] = {"batch": b1, "seconds": t1}
        record["last_statepoint"] = {"batch": b2, "seconds": t2}
        record["histories_last"] = b2 * particles
        record["rate_between_statepoints_particles_per_s"] = (b2 - b1) * particles / (t2 - t1) if t2 > t1 else None
        record["rate_including_startup_particles_per_s"] = b2 * particles / t2
        record["startup_to_first_batch_s"] = t1
        for p in files[:-1]:
            p.unlink()
    record["lost"] = RM.count_lost_particles(run_dir)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(record, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")
    print(json.dumps({k: record.get(k) for k in ("label", "histories_last", "rate_between_statepoints_particles_per_s",
                                                 "rate_including_startup_particles_per_s", "startup_to_first_batch_s")}, indent=1))
    return 0 if "error" not in record else 1


if __name__ == "__main__":
    raise SystemExit(main())
