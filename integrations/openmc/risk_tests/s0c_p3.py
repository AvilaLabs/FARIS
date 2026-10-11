#!/usr/bin/env python3
"""S0c Q3: does a voxel-uniform R2S source read high (VAL-043, hypothesis H2). OpenMC 0.15.3, double-down env.

    s0c_p3.py d1s --out-dir D --batches B --particles N
    s0c_p3.py r2s --mode mesh --voxel 10 --out-dir D --neutron-batches B --neutron-particles N --photon-batches B --photon-particles N
    s0c_p3.py r2s --mode cell --out-dir D ...

P3: a 40 cm slab of pure iron (RM-M's iron surrogate), 100 cm x 100 cm with reflective sides, a 14.1 MeV planar source on
the x = 0 face (mono-directional along +x, sampled uniformly over the face), and a 10 cm void detector layer on the far face
(vacuum beyond). Irradiation 1 Julian year at unit source rate, cooling 1 day, the p32 chain. Result: photon current through
the x = 40 cm plane into the detector layer, in photons per second (unit source rate).

  d1s   OpenMC D1S: decay photons sampled at the reaction site, ParentNuclideFilter, time-correction factors at 1 d. A
        statepoint every batch gives the batch values of the corrected sum, so R includes the correlation between nuclides.
  r2s   R2SManager (direct reaction rates): mesh voxels of --voxel cm across the thickness (full width in y and z), or
        cells of 2 cm. Alongside the total current, a second tally splits it by birth voxel (MeshBornFilter or
        CellBornFilter); with the voxel flux errors of the neutron step it gives a first-order estimate of the neutron-step
        relative error (independent voxels): R_n^2 = sum (l_v r_v)^2 / (sum l_v)^2, r_v the error of the flux above 1 MeV.

The library path workaround of S0b OPENMC-002 applies: openmc.config['cross_sections'] is set.
"""
from __future__ import annotations

import argparse
import json
import math
import os
from pathlib import Path
import resource
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
import rm_m_openmc as RM  # noqa: E402
import s0b_common as C  # noqa: E402
import s0c_common as S  # noqa: E402

CHAIN = Path.home() / "nuclear-data/p32-work/chain/depletion/chain.xml"
HALF = 50.0
GROUPS = "VITAMIN-J-42"
FAST_NEUTRON_EV = 1.0e6
SURFACE_ID, SLAB_CELL_ID = 40, 1


def setup(openmc) -> None:
    os.environ["OPENMC_CHAIN_FILE"] = str(CHAIN)
    os.environ["PATH"] = str(Path(sys.executable).parent) + os.pathsep + os.environ.get("PATH", "")
    openmc.config["chain_file"] = str(CHAIN)
    openmc.config["cross_sections"] = str(RM.FARIS_XS)


def build(openmc, n_cells: int = 1):
    """Geometry and materials of P3. n_cells > 1 divides the slab into equal cells, each with its own copy of the iron."""
    fe = RM.build_materials(openmc, ["structure"])["structure"]
    t = S.P3_THICKNESS_CM
    bc = {"boundary_type": "reflective"}
    y0, y1 = openmc.YPlane(y0=-HALF, **bc), openmc.YPlane(y0=HALF, **bc)
    z0, z1 = openmc.ZPlane(z0=-HALF, **bc), openmc.ZPlane(z0=HALF, **bc)
    xs = [openmc.XPlane(x0=t * i / n_cells, boundary_type="vacuum" if i == 0 else "transmission", surface_id=SURFACE_ID + 1 + i) for i in range(n_cells + 1)]
    xs[-1].surface_id = SURFACE_ID    # the detector-layer face
    xend = openmc.XPlane(x0=t + S.P3_DETECTOR_CM, boundary_type="vacuum", surface_id=SURFACE_ID + 100)
    cells, mats = [], []
    for i in range(n_cells):
        mat = fe
        if n_cells > 1:
            mat = fe.clone()
            mat.name = f"iron_{i}"
        if n_cells > 1 or not mats:
            mats.append(mat)
        cells.append(openmc.Cell(cell_id=SLAB_CELL_ID + i, fill=mat, region=+xs[i] & -xs[i + 1] & +y0 & -y1 & +z0 & -z1))
        cells[-1].volume = (t / n_cells) * (2 * HALF) ** 2
    det = openmc.Cell(cell_id=900, region=+xs[-1] & -xend & +y0 & -y1 & +z0 & -z1)
    model = openmc.Model(geometry=openmc.Geometry(cells + [det]), materials=openmc.Materials(mats))
    model.materials.cross_sections = str(RM.FARIS_XS)
    return model, xs[-1], cells


def neutron_source(openmc):
    space = openmc.stats.CartesianIndependent(x=openmc.stats.Discrete([1.0e-4], [1.0]), y=openmc.stats.Uniform(-HALF, HALF), z=openmc.stats.Uniform(-HALF, HALF))
    return openmc.IndependentSource(space=space, angle=openmc.stats.Monodirectional((1.0, 0.0, 0.0)), energy=openmc.stats.Discrete([14.1e6], [1.0]),
                                    particle="neutron", strength=1.0)


def common_settings(s, batches: int, particles: int, seed: int, temperature) -> None:
    s.run_mode = "fixed source"
    s.batches, s.particles, s.seed = batches, particles, seed
    s.sourcepoint = {"write": False}
    s.max_lost_particles = 1000
    s.rel_max_lost_particles = 0.5
    s.temperature = temperature
    s.output = {"summary": False, "tallies": False}


TEMPERATURE = {"default": RM.DATA_TEMPERATURE_K, "method": "nearest", "tolerance": 1.0}


def write(record: dict, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(record, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")


def rss_mb() -> float:
    return max(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss, resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss) / 1024.0


def batch_stats(values: list[float]) -> dict:
    return RM.batch_statistics(values)


# ---------------------------------------------------------------------------

def cmd_d1s(args) -> int:
    import numpy as np
    import openmc
    from openmc.deplete import d1s

    setup(openmc)
    out = args.out_dir
    run_dir = out / "run"
    run_dir.mkdir(parents=True, exist_ok=True)
    for old in run_dir.glob("statepoint.*.h5"):
        old.unlink()
    model, plane, cells = build(openmc, 1)
    s = model.settings
    max_batches = args.batches        # the configured maximum; the run is stopped at --budget and the batches written are used
    common_settings(s, max_batches, args.particles, S.SEEDS["q3_d1s"], TEMPERATURE)
    s.source = [neutron_source(openmc)]
    s.photon_transport = True
    s.use_decay_photons = True
    s.statepoint = {"batches": list(range(1, max_batches + 1))}
    leak = openmc.Tally(name="photon_leak")
    leak.filters = [openmc.SurfaceFilter([plane]), openmc.ParticleFilter(["photon"])]
    leak.scores = ["current"]
    model.tallies = openmc.Tallies([leak])
    nuclides = d1s.prepare_tallies(model, chain_file=str(CHAIN))
    model.export_to_model_xml(str(run_dir))
    rec = {"kind": "s0c-q3-d1s", "seed": s.seed, "threads": args.threads, "affinity_cpus": sorted(os.sched_getaffinity(0)), "loadavg_start": C.read_loadavg(),
           "max_batches": max_batches, "particles_per_batch": args.particles, "radionuclides": nuclides,
           "timesteps": S.P3_TIMESTEPS, "source_rates": S.P3_SOURCE_RATES, "cooling_index": S.P3_COOLING_INDEX, "chain": str(CHAIN),
           "openmc_version": openmc.__version__, "library": RM.library_identity()}
    import r1_worker

    rec["budget_s"] = args.budget
    rec["run"] = r1_worker.run_budgeted(run_dir, args.threads, args.budget)    # stopped by process group at the budget
    rec["loadavg_end"] = C.read_loadavg()
    done = sorted(int(p.stem.split(".")[1]) for p in run_dir.glob("statepoint.*.h5"))
    if not done:
        rec["error"] = f"no statepoint inside the budget (rc {rec['run']['returncode']})"
        write(rec, out / "d1s.json")
        return 1
    analyse_d1s(rec, run_dir, max_batches, out, nuclides)
    return 0


def analyse_d1s(rec: dict, run_dir: Path, max_batches: int, out: Path, nuclides: list[str] | None = None) -> None:
    """D1S result from the statepoints. The mean comes from the last intact statepoint (cumulative sums over all batches); the
    batch-to-batch spread comes from the last K = 100 batches only (reading every statepoint costs more than the run), so
    R = sd(K batches) / sqrt(N) / mean. Older statepoints are deleted unread."""
    import numpy as np
    import openmc
    from openmc.deplete import d1s

    done = sorted(int(p.stem.split(".")[1]) for p in run_dir.glob("statepoint.*.h5"))
    n_used = done[-1] - 1 if rec["run"].get("ran_to_budget") and len(done) > 1 else done[-1]
    width = len(str(max_batches))
    first = max(n_used - 100, done[0])      # the last 100 batches, or those that survive
    k = n_used - first
    cum = {}
    for b in range(first, n_used + 1):
        with openmc.StatePoint(str(run_dir / f"statepoint.{b:0{width}d}.h5")) as sp:
            t = sp.get_tally(name="photon_leak")
            parent = next(f for f in t.filters if isinstance(f, openmc.ParentNuclideFilter))
            order = [str(x) for x in parent.bins]
            cum[b] = t.sum.reshape(-1).copy()
            if b == n_used:
                sd_nuc = t.std_dev.reshape(-1).copy()
    tcf = d1s.time_correction_factors(order, S.P3_TIMESTEPS, S.P3_SOURCE_RATES)
    h = np.array([float(tcf[n][S.P3_COOLING_INDEX]) for n in order])
    per = np.array([cum[b] - cum[b - 1] for b in range(first + 1, n_used + 1)])   # per-batch sums, per source particle
    sub = (per * h).sum(axis=1).tolist()
    mean = float((cum[n_used] * h).sum() / n_used)
    se = S.subset_standard_error(sub, n_used)
    rec.update({"radionuclides": order, "batches_used": n_used, "histories": n_used * rec.get("particles_per_batch", 0)})
    rec["result"] = {"photons_per_s": mean, "std_error": se["std_error"], "relative_error": se["std_error"] / mean if mean > 0 else None, "batches": n_used,
                     "variance_from_last_batches": k, "relative_error_of_sd": se["relative_error_of_sd"],
                     "unit": "photons/s through x = 40 cm at unit source rate, 1 d cooling after 1 FPY"}
    rec["by_nuclide_photons_per_s"] = {n: float(v) for n, v in zip(order, cum[n_used] / n_used * h) if v > 0.0}
    rec["by_nuclide_relative_error_uncorrelated"] = {n: float(sd_nuc[i] / (cum[n_used][i] / n_used)) for i, n in enumerate(order) if cum[n_used][i] > 0.0}
    for p in list(run_dir.glob("statepoint.*.h5")):
        if int(p.stem.split(".")[1]) < first:
            p.unlink()
    rec["peak_rss_mb"] = rss_mb()
    write(rec, out / "d1s.json")
    print(json.dumps({k: rec[k] for k in ("run", "result")}, indent=1, default=str))


def flux_errors(openmc, statepoint: Path) -> list[float | None]:
    """Relative error of the flux above 1 MeV in each domain of the neutron step (groups with lower edge >= 1 MeV)."""
    import numpy as np

    edges = np.asarray(openmc.mgxs.GROUP_STRUCTURES[GROUPS], dtype=float)
    with openmc.StatePoint(str(statepoint)) as sp:
        t = sp.get_tally(name="MicroXS flux")
        mean, sd = t.mean.reshape(-1), t.std_dev.reshape(-1)
    n_groups = len(edges) - 1
    mean, sd = mean.reshape(-1, n_groups), sd.reshape(-1, n_groups)
    keep = edges[:-1] >= FAST_NEUTRON_EV
    out = []
    for m, s in zip(mean[:, keep], sd[:, keep]):
        tot = float(m.sum())
        out.append(float(math.sqrt((s ** 2).sum()) / tot) if tot > 0 else None)
    return out


def neutron_step_r(leak_by_voxel: list[float], r_flux: list[float | None]) -> float | None:
    """First-order relative error of the total from the neutron step: sqrt(sum (l_v r_v)^2) / sum l_v (independent voxels)."""
    total = sum(leak_by_voxel)
    if total <= 0.0 or len(leak_by_voxel) != len(r_flux):
        return None
    num = 0.0
    for l, r in zip(leak_by_voxel, r_flux):
        if l > 0.0:
            if r is None:
                return None
            num += (l * r) ** 2
    return math.sqrt(num) / total


def cmd_d1s_analyse(args) -> int:
    """Re-run only the analysis of a D1S run whose statepoints are in --out-dir/run (used after a run whose analysis was cut off)."""
    rec = {"kind": "s0c-q3-d1s", "recovered_analysis": True, "particles_per_batch": args.particles, "max_batches": args.batches,
           "run": {"ran_to_budget": True, "budget_s": args.budget, "note": "run record lost with the cut-off analysis; stopped at the budget"},
           "seed": S.SEEDS["q3_d1s"], "threads": args.threads, "chain": str(CHAIN)}
    analyse_d1s(rec, args.out_dir / "run", args.batches, args.out_dir)
    return 0


def cmd_r2s(args) -> int:
    import numpy as np
    import openmc
    import openmc.deplete

    setup(openmc)
    out = args.out_dir
    out.mkdir(parents=True, exist_ok=True)
    os.chdir(out)
    if args.mode == "mesh":
        neutron, plane, cells = build(openmc, 1)
        n = S.mesh_dimension(S.P3_THICKNESS_CM, args.voxel)
        domains = openmc.RegularMesh(name=f"p3-{args.voxel}")
        domains.lower_left, domains.upper_right, domains.dimension = [0.0, -HALF, -HALF], [S.P3_THICKNESS_CM, HALF, HALF], (n, 1, 1)
        born_filter = openmc.MeshBornFilter(domains)
    else:
        n = S.mesh_dimension(S.P3_THICKNESS_CM, S.P3_CELL_CM)
        neutron, plane, cells = build(openmc, n)
        domains = cells
        born_filter = openmc.CellBornFilter(cells)
    common_settings(neutron.settings, args.neutron_batches, args.neutron_particles, S.SEEDS["q3_neutron"], TEMPERATURE)
    neutron.settings.source = [neutron_source(openmc)]
    neutron.settings.photon_transport = False
    photon = openmc.Model(geometry=neutron.geometry, materials=neutron.materials)
    ps = openmc.Settings()
    common_settings(ps, args.photon_batches, args.photon_particles, S.SEEDS["q3_photon"], dict(TEMPERATURE))
    ps.photon_transport = True
    photon.settings = ps
    total = openmc.Tally(name="photon_current")
    total.filters = [openmc.SurfaceFilter([plane]), openmc.ParticleFilter(["photon"])]
    total.scores = ["current"]
    split = openmc.Tally(name="photon_current_by_birth")
    split.filters = [openmc.SurfaceFilter([plane]), openmc.ParticleFilter(["photon"]), born_filter]
    split.scores = ["current"]
    photon.tallies = openmc.Tallies([total, split])
    nuclides = sorted({x for m in neutron.materials for x in m.get_nuclides()})
    label = f"mesh{args.voxel:g}" if args.mode == "mesh" else "cell2"
    rec = {"kind": "s0c-q3-r2s", "mode": args.mode, "label": label, "voxel_cm": args.voxel if args.mode == "mesh" else S.P3_CELL_CM, "domains": n,
           "threads": args.threads, "affinity_cpus": sorted(os.sched_getaffinity(0)), "loadavg_start": C.read_loadavg(), "steps": {},
           "seeds": {"neutron": S.SEEDS["q3_neutron"], "photon": S.SEEDS["q3_photon"]}, "neutron_batches": args.neutron_batches,
           "neutron_particles": args.neutron_particles, "photon_batches": args.photon_batches, "photon_particles": args.photon_particles,
           "chain": str(CHAIN), "groups": GROUPS, "nuclides": nuclides, "timesteps": S.P3_TIMESTEPS, "source_rates": S.P3_SOURCE_RATES,
           "workarounds": ["openmc.config['cross_sections'] set to the audited library (manager sub-models do not inherit it)",
                           "micro_kwargs nuclides = model nuclides"], "openmc_version": openmc.__version__, "library": RM.library_identity()}
    mgr = openmc.deplete.R2SManager(neutron, domains, photon_model=photon)
    jpath = out / "r2s.json"

    def timed(name, fn):
        t0 = time.time()
        fn()
        rec["steps"][name] = {"seconds": time.time() - t0, "peak_rss_mb": rss_mb()}
        write(rec, jpath)

    timed("neutron_transport", lambda: mgr.step1_neutron_transport(
        out / "neutron_transport", mat_vol_kwargs={"n_samples": 20000} if args.mode == "mesh" else None,
        micro_kwargs={"nuclides": nuclides, "energies": GROUPS, "chain_file": str(CHAIN), "run_kwargs": {"output": False}}))
    timed("activation", lambda: mgr.step2_activation(S.P3_TIMESTEPS, S.P3_SOURCE_RATES, output_dir=out / "activation", operator_kwargs={"chain_file": str(CHAIN)}))
    boxes = {c.id: openmc.BoundingBox([S.P3_THICKNESS_CM * i / n, -HALF, -HALF], [S.P3_THICKNESS_CM * (i + 1) / n, HALF, HALF]) for i, c in enumerate(cells)} \
        if args.mode == "cell" else None
    idx = S.P3_COOLING_INDEX
    timed("photon_transport", lambda: mgr.step3_photon_transport([idx], boxes, out / "photon_transport", mat_vol_kwargs=None, run_kwargs={"output": False}))
    tallies = mgr.results["photon_tallies"][idx]
    t_total, t_split = tallies[0], tallies[1]
    mean, sd = float(t_total.mean.ravel()[0]), float(t_total.std_dev.ravel()[0])
    split_mean = t_split.mean.ravel().tolist()
    r_flux = flux_errors(openmc, out / "neutron_transport" / "statepoint.h5")
    if args.mode == "mesh":
        # one activation region per voxel (a single material); the born-filter bins follow the mesh order
        pass
    rn = neutron_step_r(split_mean, r_flux)
    rec["result"] = {"photons_per_s": mean, "std_error": sd, "relative_error": sd / mean if mean > 0 else None,
                     "photon_step_relative_error": sd / mean if mean > 0 else None, "neutron_step_relative_error_estimate": rn,
                     "combined_relative_error_estimate": math.hypot(sd / mean, rn) if (mean > 0 and rn is not None) else None,
                     "unit": "photons/s through x = 40 cm at unit source rate, 1 d cooling after 1 FPY"}
    rec["leakage_by_birth_voxel"] = split_mean
    rec["flux_relative_error_above_1MeV_by_voxel"] = r_flux
    rec["photon_batches_run"] = args.photon_batches
    rec["loadavg_end"] = C.read_loadavg()
    rec["total_seconds"] = sum(s["seconds"] for s in rec["steps"].values())
    rec["peak_rss_mb"] = rss_mb()
    write(rec, jpath)
    print(json.dumps({k: rec[k] for k in ("steps", "result", "total_seconds")}, indent=1, default=str))
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("d1s")
    p.add_argument("--out-dir", required=True, type=Path)
    p.add_argument("--batches", type=int, default=5000, help="configured maximum; the run stops at --budget")
    p.add_argument("--particles", type=int, default=500)
    p.add_argument("--budget", type=float, default=240.0)
    p.add_argument("--threads", type=int, default=4)
    p = sub.add_parser("d1s-analyse")
    p.add_argument("--out-dir", required=True, type=Path)
    p.add_argument("--batches", type=int, default=5000)
    p.add_argument("--particles", type=int, default=500)
    p.add_argument("--budget", type=float, default=220.0)
    p.add_argument("--threads", type=int, default=4)
    p = sub.add_parser("r2s")
    p.add_argument("--mode", choices=["mesh", "cell"], required=True)
    p.add_argument("--voxel", type=float, default=10.0)
    p.add_argument("--out-dir", required=True, type=Path)
    p.add_argument("--neutron-batches", type=int, default=20)
    p.add_argument("--neutron-particles", type=int, default=100000)
    p.add_argument("--photon-batches", type=int, default=50)
    p.add_argument("--photon-particles", type=int, default=100000)
    p.add_argument("--threads", type=int, default=4)
    args = ap.parse_args()
    return {"d1s": cmd_d1s, "d1s-analyse": cmd_d1s_analyse, "r2s": cmd_r2s}[args.cmd](args)


if __name__ == "__main__":
    raise SystemExit(main())
