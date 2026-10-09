#!/usr/bin/env python3
"""R3, mesh-based R2S (openmc.deplete.R2SManager) on RM-M, OpenMC 0.15.3.

    r3_r2s.py --model-dir D --out-dir R [--neutron-particles N --neutron-batches B --photon-particles N --photon-budget S]

Steps run one at a time with their own timings: neutron transport (flux and microscopic cross sections per mesh
element and material), activation (1 full-power year at 525 MW, then decay), and photon transport at cooling times of
1 d, 7 d and 30 d. The chain is ~/nuclear-data/p32-work/chain/depletion/chain.xml.

Workarounds OpenMC 0.15.3 needs (recorded in the result): output_dir is a pathlib.Path (the manager divides it with `/`),
micro_kwargs carries an explicit nuclide list, and the photon model's source_rejection_fraction is 1e-4 (the
material-constrained mesh sources reject most samples).

The dose tally is volume-integrated track length times the ICRP-116 AP photon effective dose coefficient. OpenMC
multiplies fixed-source tallies by the total source strength, so with the photon source in photons/s the tally is
pSv.cm per second.
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
import r1_worker  # noqa: E402
import r3_scenario as sc  # noqa: E402
import rm_m_checks as checks  # noqa: E402
import rm_m_openmc as RM  # noqa: E402
import rm_m_spec as spec  # noqa: E402

run_budgeted = r1_worker.run_budgeted
CHAIN = Path.home() / "nuclear-data/p32-work/chain/depletion/chain.xml"


def scoring_mesh(openmc):
    mesh = openmc.RegularMesh(name="r3-scored-region")
    mesh.lower_left = sc.MESH_LOWER_CM
    mesh.upper_right = sc.MESH_UPPER_CM
    mesh.dimension = sc.MESH_DIMENSION
    return mesh


def dose_tally(openmc, mesh, name="photon_dose"):
    energies, coeffs = openmc.data.dose_coefficients("photon", "AP")
    tally = openmc.Tally(name=name)
    tally.filters = [openmc.MeshFilter(mesh), openmc.ParticleFilter(["photon"]),
                     openmc.EnergyFunctionFilter(energies, coeffs, interpolation="log-log")]
    tally.scores = ["flux"]
    return tally


def model_nuclides(openmc, model) -> list[str]:
    return sorted({n for m in model.materials for n in m.get_nuclides()})


def build_models(openmc, model_dir: Path, mesh, n_particles, n_batches, p_particles, p_batches, seeds):
    roles = RM.load_roles(model_dir)
    ids = RM.role_ids(roles)
    neutron, _ = RM.build_model(openmc, model_dir / "rm_m.h5m", roles)
    source, _ = RM.tokamak_mesh_source(openmc, ids["plasma"], rate=1.0)
    s = neutron.settings
    s.run_mode = "fixed source"
    s.batches, s.particles, s.seed = n_batches, n_particles, seeds[0]
    s.source = [source]
    s.photon_transport = False
    s.temperature = {"default": RM.DATA_TEMPERATURE_K, "method": "nearest", "tolerance": 1.0}
    s.output = {"summary": False, "tallies": False}
    photon = openmc.Model(geometry=neutron.geometry, materials=neutron.materials)
    ps = openmc.Settings()
    ps.run_mode = "fixed source"
    ps.batches, ps.particles, ps.seed = p_batches, p_particles, seeds[1]
    ps.photon_transport = True
    ps.source_rejection_fraction = 1.0e-4
    ps.temperature = dict(s.temperature)
    ps.output = {"summary": False, "tallies": False}
    photon.settings = ps
    photon.tallies = openmc.Tallies([dose_tally(openmc, mesh)])
    return neutron, photon, ids


def timed(record, name, fn):
    t0 = time.time()
    entry = {"ok": False, "seconds": None, "peak_rss_mb": None, "error": None}
    record["steps"][name] = entry
    try:
        fn()
        entry["ok"] = True
    except Exception:
        import traceback
        entry["error"] = traceback.format_exc(limit=8)
    finally:
        entry["seconds"] = time.time() - t0
        entry["peak_rss_mb"] = max(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
                                   resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss) / 1024.0
    return entry["ok"]


def inventory_emission(openmc, chain, mgr, time_index):
    """Photon emission of the activated inventory, from the chain's decay data and the nuclide atoms.

    This is a different path from the manager's source (Material.get_decay_photon_energy with clipping): atoms times the
    per-atom photon emission rate, summed over every activation material, and the same sum by nuclide as photon
    power in MeV/s for the nuclide ranking.
    """
    import numpy as np

    results = mgr.results["depletion_results"]
    per_decay = {}
    for nuc in chain.nuclides:
        src = nuc.sources.get("photon") if nuc.sources else None
        if src is None:
            continue
        x, p = np.asarray(src.x, dtype=float), np.asarray(src.p, dtype=float)
        # the chain stores the photon spectrum per atom per second (the decay constant is already in it)
        per_decay[nuc.name] = (float(p.sum()), float((x * p).sum()) / 1.0e6)
    total = 0.0
    power = {}
    for mat in mgr.results["activation_materials"]:
        for name, atoms in results[time_index].get_material(str(mat.id)).get_nuclide_atoms().items():
            if name in per_decay and atoms > 0.0:
                n_ph, mev = per_decay[name]
                total += atoms * n_ph
                power[name] = power.get(name, 0.0) + atoms * mev
    return total, power


def source_strength(sources) -> float:
    total = 0.0
    for ms in sources:
        for src in ms.sources.ravel():
            total += float(src.strength)
    return total


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--model-dir", required=True, type=Path)
    ap.add_argument("--out-dir", required=True, type=Path)
    ap.add_argument("--neutron-particles", type=int, default=20000)
    ap.add_argument("--neutron-batches", type=int, default=50)
    ap.add_argument("--photon-particles", type=int, default=100000)
    ap.add_argument("--photon-max-batches", type=int, default=5000)
    ap.add_argument("--photon-budget", type=float, default=3000.0, help="wall seconds per photon-transport run")
    ap.add_argument("--statepoint-every", type=int, default=5)
    ap.add_argument("--threads", type=int, default=4)
    ap.add_argument("--mat-vol-samples", type=int, default=50000)
    ap.add_argument("--seed", type=int, default=20261201)
    args = ap.parse_args()
    import openmc
    import openmc.deplete

    os.environ["OMP_NUM_THREADS"] = str(args.threads)
    os.environ["PATH"] = str(Path(sys.executable).parent) + os.pathsep + os.environ.get("PATH", "")
    openmc.config["chain_file"] = str(CHAIN)
    openmc.config["cross_sections"] = str(RM.FARIS_XS)  # sub-models built inside the manager do not inherit materials.cross_sections
    out = args.out_dir
    out.mkdir(parents=True, exist_ok=True)
    os.chdir(out)
    mesh = scoring_mesh(openmc)
    neutron, photon, ids = build_models(openmc, args.model_dir, mesh, args.neutron_particles, args.neutron_batches,
                                        args.photon_particles, args.photon_max_batches, (args.seed, args.seed + 1))
    nuclides = model_nuclides(openmc, neutron)
    record = {"kind": "r3-r2s", "openmc_version": openmc.__version__, "threads": args.threads, "steps": {},
              "chain": str(CHAIN), "library": RM.library_identity(),
              "mesh": {"lower_left_cm": sc.MESH_LOWER_CM, "upper_right_cm": sc.MESH_UPPER_CM, "dimension": sc.MESH_DIMENSION},
              "settings": {"neutron_particles_per_batch": args.neutron_particles, "neutron_batches": args.neutron_batches,
                           "photon_particles_per_batch": args.photon_particles, "photon_max_batches": args.photon_max_batches, "photon_budget_s": args.photon_budget,
                           "seeds": [args.seed, args.seed + 1], "activation_groups": sc.ACTIVATION_GROUPS,
                           "mat_vol_samples": args.mat_vol_samples, "timesteps": sc.TIMESTEPS, "source_rate_n_s": sc.SOURCE_RATES[0]},
              "workarounds": ["output_dir passed as pathlib.Path", "micro_kwargs nuclides = model nuclides", "photon source_rejection_fraction = 1e-4",
                            "openmc.config['cross_sections'] set to the audited library (manager sub-models do not inherit it)"],
              "nuclides": nuclides}
    chain = openmc.deplete.Chain.from_xml(str(CHAIN))
    mgr = openmc.deplete.R2SManager(neutron, mesh, photon_model=photon)
    jpath = out / "r2s_record.json"

    def save():
        jpath.write_text(json.dumps(record, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")

    def step1():
        mgr.step1_neutron_transport(out / "neutron_transport", mat_vol_kwargs={"n_samples": args.mat_vol_samples},
                                    micro_kwargs={"nuclides": nuclides, "energies": sc.ACTIVATION_GROUPS, "chain_file": str(CHAIN),
                                                  "run_kwargs": {"output": False}})
        record["activation_regions"] = len(mgr.results["fluxes"])

    if not timed(record, "neutron_transport", step1):
        save()
        return 1
    save()

    def step2():
        mgr.step2_activation(sc.TIMESTEPS, sc.SOURCE_RATES, output_dir=out / "activation", operator_kwargs={"chain_file": str(CHAIN)})

    if not timed(record, "activation", step2):
        save()
        return 1
    save()

    conservation = {}
    contributors = {}

    def step3():
        import numpy as np

        record["photon"] = {}
        for idx in sc.PHOTON_TIME_INDICES:
            label = sc.COOLING_LABELS[idx]
            t0 = time.time()
            sources = mgr.get_decay_photon_source_mesh(idx)
            strength = source_strength(sources)
            total, power = inventory_emission(openmc, chain, mgr, idx)
            conservation[label] = checks_conservation = sc.conservation(strength, total)
            unclipped = 0.0
            for mat in mgr.results["activation_materials"]:
                e = mgr.results["depletion_results"][idx].get_material(str(mat.id)).get_decay_photon_energy(clip_tolerance=0.0)
                unclipped += e.integral() if e is not None else 0.0
            checks_conservation["unclipped_source_check"] = sc.conservation(unclipped, total)
            checks_conservation["note"] = ("source = the manager's own MeshSource strengths (Material.get_decay_photon_energy, default "
                                           "clip_tolerance 1e-6); unclipped_source_check uses clip_tolerance 0")
            contributors[label] = sc.top_contributors(power, 10)
            photon.settings.source = sources
            photon.settings.batches = args.photon_max_batches
            photon.settings.particles = args.photon_particles
            photon.settings.statepoint = {"batches": list(range(args.statepoint_every, args.photon_max_batches + 1, args.statepoint_every))}
            photon.settings.sourcepoint = {"write": False}
            run_dir = out / "photon_transport" / f"time_{idx}"
            run_dir.mkdir(parents=True, exist_ok=True)
            photon.export_to_model_xml(str(run_dir))
            run = run_budgeted(run_dir, args.threads, args.photon_budget)
            files = sorted(run_dir.glob("statepoint.*.h5"), key=lambda p: int(p.stem.split(".")[1]))
            if not files:
                raise RuntimeError(f"photon run at {label} wrote no statepoint inside {args.photon_budget} s (rc {run['returncode']})")
            for old_file in files[:-1]:
                old_file.unlink()
            last = files[-1]
            seconds_run = last.stat().st_mtime - run["t_start"]
            with openmc.StatePoint(str(last)) as sp:
                tally = sp.get_tally(name="photon_dose")
                mean = tally.mean.reshape(sc.MESH_DIMENSION[::-1]).copy()
                std = tally.std_dev.reshape(sc.MESH_DIMENSION[::-1]).copy()
            np.savez(out / f"dose_time_{idx}.npz", mean=mean, std=std)
            record["photon"][label] = {"time_index": idx, "seconds": time.time() - t0, "run": run, "batches": int(last.stem.split(".")[1]),
                                       "histories": int(last.stem.split(".")[1]) * args.photon_particles, "seconds_to_statepoint": seconds_run, "source_strength_photons_per_s": strength,
                                       "unit": "pSv.cm/s per voxel (track length x dose coefficient x photons/s)",
                                       "sum_pSv_cm_per_s": float(mean.sum()), "conservation": checks_conservation}
            save()
        record["conservation"] = conservation
        record["top_photon_power_nuclides_MeV_per_s"] = contributors

    ok = timed(record, "photon_transport", step3)
    record["accepted_conservation"] = all(c["pass"] for c in conservation.values()) if conservation else None
    save()
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
