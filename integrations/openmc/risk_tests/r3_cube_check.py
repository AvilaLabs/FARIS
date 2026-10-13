#!/usr/bin/env python3
"""R3 cube check (docs/notes/CAD_TRANSPORT_RISK_TESTS.md): Mn-54 shutdown dose from a small iron cube, three ways.

    r3_cube_check.py OUT_DIR [--histories 30000000]

A 2 cm cube of iron at 1 g/cm3 in vacuum, a 14.1 MeV beam on one face. Dose is track length times the ICRP-116 AP
coefficient in a 6 cm box (3 x 3 x 3 voxels of 2 cm, the cube in the middle), 30 d after one full-power year at
R3's source rate. The cube is thin for neutrons and photons, so voxel averaging cannot matter.
  D1S     openmc.deplete.d1s, photons born in the box, parent Mn54.
  R2S     openmc.deplete.R2SManager on the same mesh.
  manual  Fe54 (n,p) rate from a direct tally, the Bateman factor, the chain's Mn54 photons per decay, and the dose
          per photon from a photon-only run with the chain's spectrum born uniformly in the cube.
"""
from __future__ import annotations

import argparse
import json
import math
import os
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import r3_scenario as sc  # noqa: E402
import rm_m_openmc as RM  # noqa: E402

CHAIN = Path.home() / "nuclear-data/p32-work/chain/depletion/chain.xml"
HALF, VOX, IDX = 1.0, 2.0, 4          # cube half-edge (cm), voxel (cm), time index of 30 d


def base_model(openmc):
    openmc.reset_auto_ids()
    iron = openmc.Material(name="iron")
    iron.set_density("g/cm3", 1.0)
    for n, a in (("Fe54", 0.05845), ("Fe56", 0.91754), ("Fe57", 0.02119), ("Fe58", 0.00282)):
        iron.add_nuclide(n, a)
    iron.depletable = True
    mats = openmc.Materials([iron])
    mats.cross_sections = str(RM.FARIS_XS)
    cube = openmc.model.RectangularParallelepiped(-HALF, HALF, -HALF, HALF, -HALF, HALF)
    world = openmc.model.RectangularParallelepiped(-20, 20, -20, 20, -20, 20, boundary_type="vacuum")
    geom = openmc.Geometry([openmc.Cell(region=-cube, fill=iron), openmc.Cell(region=-world & +cube)])
    return openmc.Model(geometry=geom, materials=mats), iron


def settings(openmc, particles, batches, seed, photon=False):
    s = openmc.Settings()
    s.run_mode, s.particles, s.batches, s.seed = "fixed source", particles, batches, seed
    s.photon_transport = photon
    s.output = {"summary": False, "tallies": False}
    return s


def beam(openmc):
    return openmc.IndependentSource(
        space=openmc.stats.CartesianIndependent(openmc.stats.Discrete([-10.0], [1.0]), openmc.stats.Uniform(-HALF, HALF),
                                                openmc.stats.Uniform(-HALF, HALF)),
        angle=openmc.stats.Monodirectional((1.0, 0.0, 0.0)), energy=openmc.stats.Discrete([1.41e7], [1.0]))


def box_mesh(openmc, n):
    m = openmc.RegularMesh()
    m.lower_left, m.upper_right, m.dimension = (-1.5 * VOX,) * 3, (1.5 * VOX,) * 3, (n, n, n)
    return m


def dose_tally(openmc, mesh):
    e, c = openmc.data.dose_coefficients("photon", "AP")
    t = openmc.Tally(name="photon_dose")
    t.filters = [openmc.MeshFilter(mesh), openmc.ParticleFilter(["photon"]),
                 openmc.EnergyFunctionFilter(e, c, interpolation="log-log")]
    t.scores = ["flux"]
    return t


def total(sp, name):
    t = sp.get_tally(name=name)
    return float(t.mean.sum()), float((t.std_dev ** 2).sum() ** 0.5)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("out", type=Path)
    ap.add_argument("--histories", type=int, default=30_000_000)
    args = ap.parse_args()
    import numpy as np
    import openmc
    import openmc.deplete
    from openmc.deplete import d1s

    os.environ["OPENMC_CHAIN_FILE"] = str(CHAIN)
    os.environ["PATH"] = str(Path(sys.executable).parent) + os.pathsep + os.environ.get("PATH", "")
    openmc.config["chain_file"] = str(CHAIN)
    openmc.config["cross_sections"] = str(RM.FARIS_XS)
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    per_batch, rate = args.histories // 100, sc.SOURCE_RATES[0]
    chain = openmc.deplete.Chain.from_xml(str(CHAIN))
    mn = chain["Mn54"]
    lam = math.log(2) / mn.half_life
    spec = mn.sources["photon"]
    per_decay = float(np.sum(spec.p)) / lam
    res = {"histories": args.histories, "mn54_photons_per_decay": per_decay,
           "mn54_energy_per_decay_keV": float(np.sum(np.asarray(spec.x) * np.asarray(spec.p))) / lam / 1e3}
    T, cool = 365.25 * 86400.0, 30 * 86400.0
    bateman = (1 - math.exp(-lam * T)) * math.exp(-lam * cool)

    # manual: production, then dose per photon
    m, iron = base_model(openmc)
    m.settings = settings(openmc, per_batch, 100, 1)
    m.settings.source = beam(openmc)
    t = openmc.Tally(name="prod")
    t.nuclides, t.scores = ["Fe54"], ["(n,p)"]
    m.tallies = openmc.Tallies([t])
    with openmc.StatePoint(m.run(cwd=out / "prod", output=False)) as sp:
        p, pe = total(sp, "prod")
    m, iron = base_model(openmc)
    m.settings = settings(openmc, 100000, 100, 2, photon=True)
    src = openmc.IndependentSource(space=openmc.stats.Box((-HALF,) * 3, (HALF,) * 3), angle=openmc.stats.Isotropic(),
                                   energy=openmc.stats.Discrete(spec.x, np.asarray(spec.p) / np.sum(spec.p)), particle="photon")
    m.settings.source = src
    m.tallies = openmc.Tallies([dose_tally(openmc, box_mesh(openmc, 3))])
    with openmc.StatePoint(m.run(cwd=out / "per_photon", output=False)) as sp:
        g, ge = total(sp, "photon_dose")
    manual = p * rate * bateman * per_decay * g
    res["manual"] = {"fe54_np_per_source": p, "rel": pe / p, "dose_per_photon": g, "dose_rel": ge / g, "bateman": bateman,
                     "dose": manual, "rel_total": math.hypot(pe / p, ge / g)}

    # D1S
    m, iron = base_model(openmc)
    m.settings = settings(openmc, per_batch, 100, 3, photon=True)
    m.settings.source = beam(openmc)
    m.settings.use_decay_photons = True
    dt = dose_tally(openmc, box_mesh(openmc, 3))
    dt.filters.append(openmc.MeshBornFilter(box_mesh(openmc, 1)))
    m.tallies = openmc.Tallies([dt])
    nuclides = d1s.prepare_tallies(m, chain_file=str(CHAIN))
    tcf = d1s.time_correction_factors(nuclides, sc.TIMESTEPS, sc.SOURCE_RATES)
    with openmc.StatePoint(m.run(cwd=out / "d1s", output=False)) as sp:
        tt = sp.get_tally(name="photon_dose")
        mean = tt.mean.reshape(27, len(nuclides))
        std = tt.std_dev.reshape(27, len(nuclides))
        k = nuclides.index("Mn54")
        f = tcf["Mn54"][IDX]
        allf = np.array([tcf[n][IDX] for n in nuclides])
        res["d1s"] = {"dose_mn54": float(mean[:, k].sum() * f), "rel": float((std[:, k] ** 2).sum() ** 0.5 / mean[:, k].sum()),
                      "dose_all": float((mean * allf).sum()), "tcf_mn54": float(f), "expected_tcf": rate * bateman}

    # R2S
    m, iron = base_model(openmc)
    m.settings = settings(openmc, per_batch, 100, 4)
    m.settings.source = beam(openmc)
    mesh = box_mesh(openmc, 3)
    photon = openmc.Model(geometry=m.geometry, materials=m.materials)
    photon.settings = settings(openmc, 100000, 100, 5, photon=True)
    photon.tallies = openmc.Tallies([dose_tally(openmc, mesh)])
    mgr = openmc.deplete.R2SManager(m, mesh, photon_model=photon)
    os.chdir(out)
    mgr.step1_neutron_transport(out / "r2s_neutron", mat_vol_kwargs={"n_samples": 1_000_000},
                                micro_kwargs={"nuclides": ["Fe54", "Fe56", "Fe57", "Fe58"], "energies": sc.ACTIVATION_GROUPS,
                                              "chain_file": str(CHAIN), "run_kwargs": {"output": False}})
    mgr.step2_activation(sc.TIMESTEPS, sc.SOURCE_RATES, output_dir=out / "r2s_activation", operator_kwargs={"chain_file": str(CHAIN)})
    sources = mgr.get_decay_photon_source_mesh(IDX)
    strength = sum(float(s.strength) for ms in sources for s in ms.sources.ravel())
    atoms = sum(mgr.results["depletion_results"][IDX].get_material(str(mat.id)).get_nuclide_atoms().get("Mn54", 0.0)
                for mat in mgr.results["activation_materials"])
    photon.settings.source = sources
    with openmc.StatePoint(photon.run(cwd=out / "r2s_photon", output=False)) as sp:
        r, re_ = total(sp, "photon_dose")
    res["r2s"] = {"dose_all": r, "rel": re_ / r, "source_photons_per_s": strength, "mn54_atoms": atoms,
                  "mn54_activity": atoms * lam, "manual_activity": p * rate * bateman}
    res["ratios"] = {"d1s_mn54_over_manual": res["d1s"]["dose_mn54"] / manual, "r2s_over_manual": r / manual,
                     "d1s_all_over_r2s": res["d1s"]["dose_all"] / r}
    (out / "cube.json").write_text(json.dumps(res, indent=1))
    print(json.dumps(res, indent=1))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
