#!/usr/bin/env python3
"""Writes the two smoke-test models used by the Windows native spike.

Run once with an OpenMC 0.15.3 Python environment; the generated XML is committed so the
Windows runner needs no Python OpenMC. The CSG model and the DAGMC model describe the same
three nested hydrogen cubes (half widths 5, 10 and 15 cm, fixture
integrations/cad/fixtures/three-boxes.h5m) with an isotropic 14.1 MeV point source at the
origin and the same tallies, so the two runs also cross-check each other.

    make_models.py OUTDIR  (writes OUTDIR/csg and OUTDIR/dagmc)
"""
import sys
from pathlib import Path

import openmc

FIXTURE = Path(__file__).resolve().parents[2] / "cad" / "fixtures" / "three-boxes.h5m"
SEED = 20261010
PARTICLES = 20000
BATCHES = 20
ENERGY_BINS = [0.0, 1.0e5, 1.0e6, 1.0e7, 1.5e7]
# Volume ids in the fixture (GLOBAL_ID of the three Volume sets) and their half widths.
CELLS = [(1, 5.0, "core"), (2, 10.0, "shell1"), (3, 15.0, "shell2")]
DENSITY = {"core": 0.10, "shell1": 0.30, "shell2": 0.20}  # g/cm3 of H1, not physical


def materials():
    mats = []
    for _, _, name in CELLS:
        m = openmc.Material(name=name)
        m.add_nuclide("H1", 1.0)
        m.set_density("g/cm3", DENSITY[name])
        mats.append(m)
    return mats


def settings():
    s = openmc.Settings()
    s.run_mode = "fixed source"
    s.particles = PARTICLES
    s.batches = BATCHES
    s.seed = SEED
    s.photon_transport = False
    src = openmc.IndependentSource()
    src.space = openmc.stats.Point((0.0, 0.0, 0.0))
    src.energy = openmc.stats.Discrete([14.1e6], [1.0])
    src.particle = "neutron"
    s.source = [src]
    return s


def tallies(cell_ids):
    cf = openmc.CellFilter(cell_ids)
    ef = openmc.EnergyFilter(ENERGY_BINS)
    t1 = openmc.Tally(1, name="flux_by_cell")
    t1.filters = [cf]
    t1.scores = ["flux", "absorption"]
    t2 = openmc.Tally(2, name="flux_by_cell_energy")
    t2.filters = [cf, ef]
    t2.scores = ["flux"]
    t3 = openmc.Tally(3, name="elastic_by_cell")
    t3.filters = [cf]
    t3.scores = ["elastic", "(n,2n)"]
    return openmc.Tallies([t1, t2, t3])


def csg(out):
    mats = materials()
    cells = []
    inner = None
    for (cid, hw, name), mat in zip(CELLS, mats):
        last = cid == CELLS[-1][0]
        planes = [
            openmc.XPlane(-hw), openmc.XPlane(hw), openmc.YPlane(-hw),
            openmc.YPlane(hw), openmc.ZPlane(-hw), openmc.ZPlane(hw),
        ]
        if last:
            for p in planes:
                p.boundary_type = "vacuum"
        region = +planes[0] & -planes[1] & +planes[2] & -planes[3] & +planes[4] & -planes[5]
        if inner is not None:
            region &= ~inner
        cells.append(openmc.Cell(cid, name=name, fill=mat, region=region))
        inner = +planes[0] & -planes[1] & +planes[2] & -planes[3] & +planes[4] & -planes[5]
    geom = openmc.Geometry(cells)
    out.mkdir(parents=True, exist_ok=True)
    geom.export_to_xml(out / "geometry.xml")
    openmc.Materials(mats).export_to_xml(out / "materials.xml")
    settings().export_to_xml(out / "settings.xml")
    tallies([c[0] for c in CELLS]).export_to_xml(out / "tallies.xml")


def dagmc(out):
    mats = materials()
    # Material tags in the h5m are mat:core, mat:shell1, mat:shell2 and mat:void_comp.
    # The fixture has no graveyard, so the DAGMC universe sits in a vacuum-bounded box.
    univ = openmc.DAGMCUniverse(filename=str(FIXTURE), auto_geom_ids=False)
    geom = openmc.Geometry(univ.bounded_universe(padding_distance=2.0))
    out.mkdir(parents=True, exist_ok=True)
    geom.export_to_xml(out / "geometry.xml")
    # The exporter writes the absolute path of the h5m; the model runs from its own directory.
    text = (out / "geometry.xml").read_text()
    (out / "geometry.xml").write_text(text.replace(str(univ.filename), "three-boxes.h5m"))
    openmc.Materials(mats).export_to_xml(out / "materials.xml")
    settings().export_to_xml(out / "settings.xml")
    tallies([c[0] for c in CELLS]).export_to_xml(out / "tallies.xml")


if __name__ == "__main__":
    root = Path(sys.argv[1])
    csg(root / "csg")
    dagmc(root / "dagmc")
