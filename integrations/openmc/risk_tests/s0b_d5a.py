#!/usr/bin/env python3
"""S0b D5a: reaction-pathway audit of D1S decay-photon emission on 5 cm spheres in vacuum (no CAD). OpenMC 0.15.3 (double-down env).

    s0b_d5a.py neutron   --material tungsten --source mono14|tf --out-dir D [--spectrum S.json]
    s0b_d5a.py d1s       --material tungsten --source mono14|tf --out-dir D [--spectrum S.json]
    s0b_d5a.py photon    --material tungsten --out-dir D [--workers 4]
    s0b_d5a.py analyse   --material tungsten --source mono14|tf --out-dir D

Scenario: 1 FPY (Julian year) at unit source rate, then 1 d cooling. Source: a point isotropic source at the centre, either
14.1 MeV or the TF fast-flux spectrum tabulated from an analog RM-M run. Library and chain as in S0.

  neutron   Neutron-only run (independent of the D1S run) with direct reaction-rate tallies for every chain reaction that
            produces a radionuclide, on each target nuclide, and the partial-MT scores (MT 600-649 for (n,p), 650-699 (n,d),
            700-749 (n,t), 800-849 (n,a)) present in that target's library file. A statepoint is written every batch so the
            batch covariance is available for the ratios.
  d1s       Coupled neutron and decay-photon run (settings.use_decay_photons) with the photon `current` on the sphere surface
            through a SurfaceFilter and a ParentNuclideFilter (d1s.prepare_tallies); times the time-correction factor.
  photon    For every radionuclide that is produced and has a chain photon spectrum: the fraction of emitted photons that
            leave the sphere, for a uniform source in the sphere. Independent of the neutron source.
  analyse   Single-step Bateman activity from the production rates, times the emission per atom and the leaving fraction,
            against D1S; the protocol's rule, and the partial-MT fraction with its sigma.
"""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
import rm_m_openmc as RM  # noqa: E402
import s0b_common as C  # noqa: E402

CHAIN = Path.home() / "nuclear-data/p32-work/chain/depletion/chain.xml"
SPHERE_R_CM = 5.0
MATERIAL_TAGS = {"tungsten": "tungsten", "iron_surrogate": "structure", "tih2": "tih2"}
FAMILIES = {"(n,p)": (600, 649), "(n,d)": (650, 699), "(n,t)": (700, 749), "(n,a)": (800, 849)}
IRRADIATION_S = C.SECONDS_PER_JULIAN_YEAR
COOLING_S = 86400.0
TIMESTEPS = [(1.0, "a"), (1.0, "d")]
SOURCE_RATES = [1.0, 0.0]
COOLING_INDEX = 2
SEEDS = {("neutron", "mono14"): 90100001, ("neutron", "tf"): 90100002, ("d1s", "mono14"): 90100003, ("d1s", "tf"): 90100004,
         ("photon", None): 90100005}
SURFACE_ID = 7
CELL_ID = 7
N_SHELLS = 20


def setup_environment(openmc) -> None:
    os.environ["OPENMC_CHAIN_FILE"] = str(CHAIN)
    os.environ["PATH"] = str(Path(sys.executable).parent) + os.pathsep + os.environ.get("PATH", "")
    openmc.config["chain_file"] = str(CHAIN)
    openmc.config["cross_sections"] = str(RM.FARIS_XS)


def library_partial_mts(nuclide: str) -> list[int]:
    """MT numbers in the nuclide's library file that fall in the protocol's partial families."""
    import h5py

    path = Path(RM.FARIS_XS).parent / "neutron" / f"{nuclide}.h5"
    with h5py.File(path, "r") as f:
        mts = sorted(int(k.split("_")[1]) for k in f[nuclide]["reactions"])
    return [m for m in mts if any(lo <= m <= hi for lo, hi in FAMILIES.values())]


def library_redundant_mts(nuclide: str) -> list[int]:
    import h5py

    path = Path(RM.FARIS_XS).parent / "neutron" / f"{nuclide}.h5"
    with h5py.File(path, "r") as f:
        r = f[nuclide]["reactions"]
        return sorted(int(k.split("_")[1]) for k in r if r[k].attrs.get("redundant", 0))


def material_model(openmc, material: str):
    tag = MATERIAL_TAGS[material]
    mat = RM.build_materials(openmc, [tag])[tag]
    sphere = openmc.Sphere(surface_id=SURFACE_ID, r=SPHERE_R_CM, boundary_type="vacuum")
    cell = openmc.Cell(cell_id=CELL_ID, fill=mat, region=-sphere)
    model = openmc.Model(geometry=openmc.Geometry([cell]), materials=openmc.Materials([mat]))
    model.materials.cross_sections = str(RM.FARIS_XS)
    return model, mat, sphere, cell


def neutron_source(openmc, kind: str, spectrum_path: Path | None):
    if kind == "mono14":
        energy = openmc.stats.Discrete([14.1e6], [1.0])
    else:
        spec = json.loads(Path(spectrum_path).read_text(encoding="utf-8"))
        edges = spec["edges_ev"]
        p = list(spec["mean"]) + [0.0]
        energy = openmc.stats.Tabular(edges, p, interpolation="histogram")
    return openmc.IndependentSource(space=openmc.stats.Point((0.0, 0.0, 0.0)), angle=openmc.stats.Isotropic(), energy=energy,
                                    particle="neutron", strength=1.0)


def producers(chain, nuclides: list[str]) -> list[dict]:
    """Every chain reaction on a model nuclide whose product has a half-life (the set D1S considers)."""
    out = []
    for nuc in chain.nuclides:
        if nuc.name not in nuclides:
            continue
        for rx in nuc.reactions:
            if rx.target is None or rx.type == "fission":
                continue
            if chain[rx.target].half_life is None:
                continue
            out.append({"target": nuc.name, "reaction": rx.type, "product": rx.target, "branching": float(rx.branching_ratio)})
    return out


def scores_for(target: str, reactions: list[str]) -> tuple[list[str], dict]:
    """Tally scores for one target: its producing reactions, then the library's partial MTs in each reaction's family."""
    scores = list(reactions)
    partial = {}
    present = library_partial_mts(target)
    for rx in reactions:
        fam = FAMILIES.get(rx)
        mts = [m for m in present if fam and fam[0] <= m <= fam[1]]
        partial[rx] = mts
        scores += [str(m) for m in mts]
    return scores, partial


def run_cli(run_dir: Path, threads: int) -> dict:
    return RM.run_openmc_cli(run_dir, threads)


# ---------------------------------------------------------------------------
# neutron stage

def stage_neutron(args) -> int:
    import openmc
    import openmc.deplete

    setup_environment(openmc)
    chain = openmc.deplete.Chain.from_xml(str(CHAIN))
    model, mat, sphere, cell = material_model(openmc, args.material)
    nuclides = sorted(mat.get_nuclides())
    prods = producers(chain, nuclides)
    by_target: dict[str, list[str]] = {}
    for p in prods:
        by_target.setdefault(p["target"], [])
        if p["reaction"] not in by_target[p["target"]]:
            by_target[p["target"]].append(p["reaction"])
    out = args.out_dir
    run_dir = out / "neutron_run"
    run_dir.mkdir(parents=True, exist_ok=True)
    for old in run_dir.glob("statepoint.*.h5"):
        old.unlink()
    s = model.settings
    s.run_mode = "fixed source"
    s.batches, s.particles, s.seed = args.batches, args.particles, SEEDS[("neutron", args.source)]
    s.source = [neutron_source(openmc, args.source, args.spectrum)]
    s.photon_transport = False
    s.statepoint = {"batches": list(range(1, args.batches + 1))}
    s.sourcepoint = {"write": False}
    s.temperature = {"default": RM.DATA_TEMPERATURE_K, "method": "nearest", "tolerance": 1.0}
    s.output = {"summary": False, "tallies": False}
    shells = [SPHERE_R_CM * i / N_SHELLS for i in range(N_SHELLS + 1)]
    smesh = openmc.SphericalMesh(r_grid=shells, phi_grid=[0.0, 2.0 * math.pi], theta_grid=[0.0, math.pi], mesh_id=77)
    tallies, layout = [], {}
    for target, reactions in sorted(by_target.items()):
        scores, partial = scores_for(target, reactions)
        t = openmc.Tally(name=f"rates:{target}")
        t.filters = [openmc.CellFilter([cell])]
        t.nuclides = [target]
        t.scores = scores
        tallies.append(t)
        r = openmc.Tally(name=f"radial:{target}")
        r.filters = [openmc.MeshFilter(smesh)]
        r.nuclides = [target]
        r.scores = list(reactions)
        tallies.append(r)
        layout[target] = {"scores": scores, "reactions": reactions, "partial_mts": partial,
                          "redundant_mts_in_file": [m for m in library_redundant_mts(target) if 100 <= m < 200]}
    model.tallies = openmc.Tallies(tallies)
    model.export_to_model_xml(str(run_dir))
    rec = {"kind": "d5a-neutron", "material": args.material, "source": args.source, "seed": s.seed, "threads": args.threads,
           "batches": args.batches, "particles_per_batch": args.particles, "histories": args.batches * args.particles,
           "openmc_version": openmc.__version__, "affinity_cpus": sorted(os.sched_getaffinity(0)), "loadavg_start": C.read_loadavg(),
           "spectrum": str(args.spectrum) if args.spectrum else None, "producers": prods, "layout": layout, "shell_edges_cm": shells,
           "library": RM.library_identity(), "nuclides": nuclides}
    rec["run"] = run_cli(run_dir, args.threads)
    rec["loadavg_end"] = C.read_loadavg()
    if rec["run"]["returncode"] != 0:
        rec["error"] = "openmc failed"
        (out / "rates.json").write_text(json.dumps(rec, indent=1, default=str) + "\n", encoding="utf-8")
        return 1
    # per-batch values from cumulative sums
    cum = {name: [] for name in [f"rates:{t}" for t in layout] + [f"radial:{t}" for t in layout]}
    for b in range(1, args.batches + 1):
        path = run_dir / f"statepoint.{b:0{len(str(args.batches))}d}.h5"
        with openmc.StatePoint(str(path)) as sp:
            for name in cum:
                cum[name].append(sp.get_tally(name=name).sum.reshape(-1).copy())
        if b < args.batches:
            path.unlink()
    import numpy as np

    per_batch = {}
    for name, rows in cum.items():
        arr = np.array(rows)
        per = np.diff(np.vstack([np.zeros_like(arr[:1]), arr]), axis=0)
        per_batch[name] = per
    rec["rates"] = {}
    rec["radial"] = {}
    for target, lay in layout.items():
        per = per_batch[f"rates:{target}"]
        rec["rates"][target] = {sc: per[:, i].tolist() for i, sc in enumerate(lay["scores"])}
        rad = per_batch[f"radial:{target}"]
        nrx = len(lay["reactions"])
        rad = rad.reshape(args.batches, N_SHELLS, nrx)
        rec["radial"][target] = {rx: {"mean": rad[:, :, i].mean(axis=0).tolist()} for i, rx in enumerate(lay["reactions"])}
    rec["statepoint_sha256"] = RM.sha256(run_dir / f"statepoint.{args.batches:0{len(str(args.batches))}d}.h5")
    (out / "rates.json").write_text(json.dumps(rec, indent=1, default=str) + "\n", encoding="utf-8")
    return 0


# ---------------------------------------------------------------------------
# D1S stage

def stage_d1s(args) -> int:
    import numpy as np
    import openmc
    import openmc.deplete
    from openmc.deplete import d1s

    setup_environment(openmc)
    model, mat, sphere, cell = material_model(openmc, args.material)
    out = args.out_dir
    run_dir = out / "d1s_run"
    run_dir.mkdir(parents=True, exist_ok=True)
    for old in run_dir.glob("statepoint.*.h5"):
        old.unlink()
    s = model.settings
    s.run_mode = "fixed source"
    s.batches, s.particles, s.seed = args.batches, args.particles, SEEDS[("d1s", args.source)]
    s.source = [neutron_source(openmc, args.source, args.spectrum)]
    s.photon_transport = True
    s.use_decay_photons = True
    s.statepoint = {"batches": [args.batches]}
    s.sourcepoint = {"write": False}
    s.temperature = {"default": RM.DATA_TEMPERATURE_K, "method": "nearest", "tolerance": 1.0}
    s.output = {"summary": False, "tallies": False}
    leak = openmc.Tally(name="photon_leak")
    leak.filters = [openmc.SurfaceFilter([sphere]), openmc.ParticleFilter(["photon"])]
    leak.scores = ["current"]
    model.tallies = openmc.Tallies([leak])
    nuclides = d1s.prepare_tallies(model, chain_file=str(CHAIN))
    model.export_to_model_xml(str(run_dir))
    rec = {"kind": "d5a-d1s", "material": args.material, "source": args.source, "seed": s.seed, "threads": args.threads,
           "batches": args.batches, "particles_per_batch": args.particles, "histories": args.batches * args.particles,
           "openmc_version": openmc.__version__, "affinity_cpus": sorted(os.sched_getaffinity(0)), "loadavg_start": C.read_loadavg(),
           "spectrum": str(args.spectrum) if args.spectrum else None, "radionuclides": nuclides, "chain": str(CHAIN),
           "timesteps": TIMESTEPS, "source_rates": SOURCE_RATES, "cooling_index": COOLING_INDEX, "library": RM.library_identity()}
    rec["run"] = run_cli(run_dir, args.threads)
    rec["loadavg_end"] = C.read_loadavg()
    if rec["run"]["returncode"] != 0:
        rec["error"] = "openmc failed"
        (out / "d1s.json").write_text(json.dumps(rec, indent=1, default=str) + "\n", encoding="utf-8")
        return 1
    tcf = d1s.time_correction_factors(nuclides, TIMESTEPS, SOURCE_RATES)
    path = run_dir / f"statepoint.{args.batches}.h5"
    rec["statepoint_sha256"] = RM.sha256(path)
    with openmc.StatePoint(str(path)) as sp:
        t = sp.get_tally(name="photon_leak")
        order = [type(f).__name__ for f in t.filters]
        parent = next(f for f in t.filters if isinstance(f, openmc.ParentNuclideFilter))
        bins = [str(b) for b in parent.bins]
        mean = t.mean.reshape(-1)
        sd = t.std_dev.reshape(-1)
        lead = int(np.prod([f.num_bins for f in t.filters[:t.filters.index(parent)]]))
        trail = int(np.prod([f.num_bins for f in t.filters[t.filters.index(parent) + 1:]]))
        if lead != 1 or trail != 1:
            raise RuntimeError(f"unexpected tally layout {order}")
    from openmc.data import half_life

    rec["filter_order"] = order
    rec["nuclides"] = {}
    for i, name in enumerate(bins):
        h = float(tcf[name][COOLING_INDEX])
        rec["nuclides"][name] = {"tally_mean": float(mean[i]), "tally_std_error": float(sd[i]), "time_correction": h,
                                 "leak": float(mean[i]) * h, "leak_std_error": float(sd[i]) * h,
                                 "half_life_openmc_data_s": half_life(name)}
    rec["total_leak"] = sum(v["leak"] for v in rec["nuclides"].values())
    (out / "d1s.json").write_text(json.dumps(rec, indent=1, default=str) + "\n", encoding="utf-8")
    return 0


# ---------------------------------------------------------------------------
# photon stage

def photon_source(openmc, chain, name: str, radial=None):
    dist = chain[name].sources.get("photon")
    if dist is None:
        return None, 0.0
    total = float(dist.integral())
    if total <= 0.0:
        return None, 0.0
    if radial is None:
        r = openmc.stats.PowerLaw(0.0, SPHERE_R_CM, 2.0)
    else:
        edges, weights = radial  # shell edges (cm) and the production in each shell; uniform in volume within a shell
        keep = [i for i, w in enumerate(weights) if w > 0.0]
        total_w = sum(weights[i] for i in keep)
        r = openmc.stats.Mixture([weights[i] / total_w for i in keep],
                                 [openmc.stats.PowerLaw(edges[i], edges[i + 1], 2.0) for i in keep])
    space = openmc.stats.SphericalIndependent(r, openmc.stats.Uniform(-1.0, 1.0), openmc.stats.Uniform(0.0, 2.0 * math.pi))
    src = openmc.IndependentSource(space=space, angle=openmc.stats.Isotropic(), energy=dist, particle="photon", strength=1.0)
    return src, total


def one_photon_run(task) -> dict:
    """Run one photon leakage job in its own directory with one OpenMP thread (used from a thread pool)."""
    run_dir, threads = task
    t0 = time.time()
    info = RM.run_openmc_cli(Path(run_dir), threads)
    return {"returncode": info["returncode"], "wall_s": time.time() - t0}


def stage_photon(args) -> int:
    import numpy as np
    import openmc
    import openmc.deplete

    setup_environment(openmc)
    chain = openmc.deplete.Chain.from_xml(str(CHAIN))
    model0, mat, sphere, cell = material_model(openmc, args.material)
    prods = producers(chain, sorted(mat.get_nuclides()))
    names = sorted({p["product"] for p in prods})
    out = args.out_dir
    base = out / "photon_runs"
    base.mkdir(parents=True, exist_ok=True)
    radial_jobs = {}
    if args.radial_from:
        rates = json.loads(Path(args.radial_from).read_text(encoding="utf-8"))
        edges = rates["shell_edges_cm"]
        for X in names:
            dens = np.zeros(len(edges) - 1)
            for p in prods:
                if p["product"] != X:
                    continue
                dens += p["branching"] * np.array(rates["radial"][p["target"]][p["reaction"]]["mean"])
            if dens.sum() > 0:
                radial_jobs[X] = (edges, dens.tolist())
    tag = f"radial_{Path(args.radial_from).parent.name}" if args.radial_from else "uniform"
    tasks, meta = [], {}
    for X in names:
        if args.radial_from and X not in radial_jobs:
            continue
        model, mat, sphere, cell = material_model(openmc, args.material)
        src, total = photon_source(openmc, chain, X, radial_jobs.get(X))
        if src is None:
            meta[X] = {"emission_per_atom_s": 0.0, "note": "no photon spectrum in the chain"}
            continue
        s = model.settings
        s.run_mode = "fixed source"
        s.batches, s.particles, s.seed = args.batches, args.particles, SEEDS[("photon", None)]
        s.source = [src]
        s.photon_transport = True
        s.statepoint = {"batches": [args.batches]}
        s.sourcepoint = {"write": False}
        s.output = {"summary": False, "tallies": False}
        leak = openmc.Tally(name="photon_leak")
        leak.filters = [openmc.SurfaceFilter([sphere]), openmc.ParticleFilter(["photon"])]
        leak.scores = ["current"]
        model.tallies = openmc.Tallies([leak])
        rd = base / tag / X
        rd.mkdir(parents=True, exist_ok=True)
        for old in rd.glob("statepoint.*.h5"):
            old.unlink()
        model.export_to_model_xml(str(rd))
        tasks.append((str(rd), 1))
        meta[X] = {"emission_per_atom_s": total, "run_dir": str(rd)}
    rec = {"kind": "d5a-photon", "material": args.material, "variant": tag, "batches": args.batches, "particles_per_batch": args.particles,
           "histories": args.batches * args.particles, "seed": SEEDS[("photon", None)], "workers": args.workers, "threads_per_run": 1,
           "affinity_cpus": sorted(os.sched_getaffinity(0)), "openmc_version": openmc.__version__, "loadavg_start": C.read_loadavg(),
           "radial_from": str(args.radial_from) if args.radial_from else None}
    with ThreadPoolExecutor(max_workers=args.workers) as pool:
        results = list(pool.map(one_photon_run, tasks))
    rec["loadavg_end"] = C.read_loadavg()
    rec["nuclides"] = {}
    for (rd, _), res in zip(tasks, results):
        X = Path(rd).name
        entry = dict(meta[X])
        entry["run"] = res
        if res["returncode"] == 0:
            with openmc.StatePoint(str(Path(rd) / f"statepoint.{args.batches}.h5")) as sp:
                t = sp.get_tally(name="photon_leak")
                entry["leak_fraction"] = float(t.mean.ravel()[0])
                entry["leak_fraction_std_error"] = float(t.std_dev.ravel()[0])
        rec["nuclides"][X] = entry
    for X, m in meta.items():
        rec["nuclides"].setdefault(X, m)
    name = "photon_ref.json" if not args.radial_from else f"photon_ref_{tag}.json"
    (out / name).write_text(json.dumps(rec, indent=1, default=str) + "\n", encoding="utf-8")
    return 0 if all(r["returncode"] == 0 for r in results) else 1


# ---------------------------------------------------------------------------
# analysis

def analyse(args) -> int:
    import openmc
    import openmc.deplete

    setup_environment(openmc)
    chain = openmc.deplete.Chain.from_xml(str(CHAIN))
    src_dir = args.out_dir
    rates = json.loads((src_dir / "rates.json").read_text(encoding="utf-8"))
    d1s_rec = json.loads((src_dir / "d1s.json").read_text(encoding="utf-8"))
    photon = json.loads((args.photon_dir / "photon_ref.json").read_text(encoding="utf-8"))
    radial_path = args.photon_dir / f"photon_ref_radial_{src_dir.name}.json"
    radial = json.loads(radial_path.read_text(encoding="utf-8")) if radial_path.is_file() else None
    prods = rates["producers"]
    per = rates["rates"]
    n_b = rates["batches"]

    def series(target, score):
        return per[target][score]

    products = sorted({p["product"] for p in prods})
    rows = {}
    for X in products:
        lam = math.log(2.0) / chain[X].half_life
        contrib = [p for p in prods if p["product"] == X]
        prod_series = [sum(p["branching"] * series(p["target"], p["reaction"])[b] for p in contrib) for b in range(n_b)]
        mean = math.fsum(prod_series) / n_b
        var = math.fsum((x - mean) ** 2 for x in prod_series) / (n_b - 1) / n_b if n_b > 1 else 0.0
        se = math.sqrt(var)
        bat = C.bateman_single_step(mean, lam, IRRADIATION_S, COOLING_S)
        ph = photon["nuclides"].get(X, {})
        e_atom = ph.get("emission_per_atom_s", 0.0)
        leak_frac = ph.get("leak_fraction")
        leak_frac_se = ph.get("leak_fraction_std_error")
        emission = bat["atoms_cooled"] * e_atom
        rel_p = se / mean if mean else 0.0
        rel_l = (leak_frac_se / leak_frac) if leak_frac else 0.0
        ref = emission * leak_frac if leak_frac is not None else None
        ref_se = ref * math.hypot(rel_p, rel_l) if ref else 0.0
        d = d1s_rec["nuclides"].get(X, {"leak": 0.0, "leak_std_error": 0.0, "tally_mean": 0.0, "tally_std_error": 0.0})
        # partial-MT fraction: production-weighted over the reactions that produce X
        num, den = [], []
        for b in range(n_b):
            nsum, dsum = 0.0, 0.0
            for p in contrib:
                lay = rates["layout"][p["target"]]
                part = lay["partial_mts"].get(p["reaction"], [])
                dsum += p["branching"] * series(p["target"], p["reaction"])[b]
                nsum += p["branching"] * sum(series(p["target"], str(m))[b] for m in part)
            num.append(nsum)
            den.append(dsum)
        frac = C.ratio_of_means_sigma(num, den) if sum(den) > 0 else {"ratio": 0.0, "std_error": 0.0}
        # supplementary reference: source distributed over radius as the production, same emission
        sup = None
        if radial and X in radial["nuclides"] and radial["nuclides"][X].get("leak_fraction") is not None:
            lf, lse = radial["nuclides"][X]["leak_fraction"], radial["nuclides"][X]["leak_fraction_std_error"]
            r_ = emission * lf
            sup = {"ref": r_, "ref_std_error": r_ * math.hypot(rel_p, (lse / lf) if lf else 0.0), "leak_fraction": lf}
        rows[X] = {"half_life_chain_s": chain[X].half_life, "half_life_openmc_data_s": d.get("half_life_openmc_data_s"),
                   "producers": [{k: p[k] for k in ("target", "reaction", "branching")} for p in contrib],
                   "production_per_source_neutron": mean, "production_std_error": se,
                   "bateman": bat, "emission_per_atom_s": e_atom, "emission_photons_per_s": emission,
                   "leak_fraction_uniform": leak_frac, "leak_fraction_std_error": leak_frac_se,
                   "ref_leak": ref, "ref_leak_std_error": ref_se, "d1s_leak": d["leak"], "d1s_leak_std_error": d["leak_std_error"],
                   "d1s_tally_mean": d["tally_mean"], "partial_mt_fraction": frac["ratio"], "partial_mt_fraction_std_error": frac["std_error"],
                   "supplementary_radial_reference": sup}
    d_share = C.shares({k: v["d1s_leak"] for k, v in rows.items()})
    r_share = C.shares({k: (v["ref_leak"] or 0.0) for k, v in rows.items()})
    chosen = C.nuclides_above(d_share, r_share)
    table = []
    for X in chosen:
        v = rows[X]
        rule = C.d5a_rule(v["d1s_leak"], v["d1s_leak_std_error"], v["ref_leak"], v["ref_leak_std_error"]) if v["ref_leak"] else {
            "ratio": None, "sigma": None, "defect": None}
        entry = {"nuclide": X, "d1s_share": d_share[X], "ref_share": r_share[X], **{k: v[k] for k in (
            "d1s_leak", "d1s_leak_std_error", "ref_leak", "ref_leak_std_error", "partial_mt_fraction", "partial_mt_fraction_std_error")},
            "ratio": rule["ratio"], "ratio_sigma": rule["sigma"], "defect": rule["defect"]}
        if rule["defect"]:
            entry["h1"] = C.h1_confirmed(rule["ratio"], rule["sigma"], v["partial_mt_fraction"], v["partial_mt_fraction_std_error"])
        if v["supplementary_radial_reference"] and v["d1s_leak"]:
            s_ = v["supplementary_radial_reference"]
            sr = C.d5a_rule(v["d1s_leak"], v["d1s_leak_std_error"], s_["ref"], s_["ref_std_error"])
            entry["supplementary_radial"] = {"ratio": sr["ratio"], "sigma": sr["sigma"], "defect": sr["defect"]}
        table.append(entry)
    result = {"kind": "d5a-audit", "material": args.material, "source": rates["source"], "scenario": "1 Julian year at unit rate, 1 d cooling",
              "rule": "defect if |D1S/reference - 1| > 3 sigma and > 2 %", "table": table, "all_nuclides": rows,
              "totals": {"d1s": sum(v["d1s_leak"] for v in rows.values()), "reference": sum((v["ref_leak"] or 0.0) for v in rows.values())},
              "records": {"rates": str(src_dir / "rates.json"), "d1s": str(src_dir / "d1s.json"), "photon": str(args.photon_dir / "photon_ref.json")}}
    (src_dir / "audit.json").write_text(json.dumps(result, indent=1, default=str) + "\n", encoding="utf-8")
    print(f"{args.material} {rates['source']}: D1S total {result['totals']['d1s']:.4e}  reference total {result['totals']['reference']:.4e}")
    print(f"{'nuclide':8s} {'D1S':>11s} {'ref':>11s} {'ratio':>8s} {'+-':>7s} {'f_partial':>9s} {'+-':>7s} {'sup.ratio':>9s} defect")
    for e in table:
        sr = e.get("supplementary_radial", {}).get("ratio")
        print(f"{e['nuclide']:8s} {e['d1s_leak']:11.4e} {(e['ref_leak'] or 0):11.4e} {(e['ratio'] or 0):8.4f} {(e['ratio_sigma'] or 0):7.4f} "
              f"{e['partial_mt_fraction']:9.4f} {e['partial_mt_fraction_std_error']:7.4f} {(sr or 0):9.4f} {e['defect']}")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    for name in ("neutron", "d1s", "photon", "analyse"):
        p = sub.add_parser(name)
        p.add_argument("--material", required=True, choices=sorted(MATERIAL_TAGS))
        p.add_argument("--out-dir", required=True, type=Path)
        p.add_argument("--threads", type=int, default=4)
        if name in ("neutron", "d1s", "analyse"):
            p.add_argument("--source", required=True, choices=["mono14", "tf"])
        if name in ("neutron", "d1s"):
            p.add_argument("--spectrum", type=Path)
            p.add_argument("--batches", type=int, default=50)
            p.add_argument("--particles", type=int, default=200000)
        if name == "photon":
            p.add_argument("--batches", type=int, default=10)
            p.add_argument("--particles", type=int, default=40000)
            p.add_argument("--workers", type=int, default=4)
            p.add_argument("--radial-from", type=Path, help="supplementary: source distributed over radius as the production in this rates.json")
        if name == "analyse":
            p.add_argument("--photon-dir", required=True, type=Path)
    args = ap.parse_args()
    if getattr(args, "source", None) == "tf" and args.cmd in ("neutron", "d1s") and not args.spectrum:
        ap.error("--spectrum is required for the tf source")
    return {"neutron": stage_neutron, "d1s": stage_d1s, "photon": stage_photon, "analyse": analyse}[args.cmd](args)


if __name__ == "__main__":
    raise SystemExit(main())
