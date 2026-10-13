#!/usr/bin/env python3
"""S0f second geometry (fact 15): a shielded room with one straight duct, to test whether the automatic targets of
s0f_find_targets.py carry beyond RM-M. Nothing here was tuned on RM-M's layout.

    s0f_second_model.py OUT_DIR [--magic-batches 8] [--magic-particles 200000]

It differs from RM-M on purpose: constructive solid geometry, not CAD; a uniform box source, not a plasma; a 16 cm x
24 cm duct along +y through 200 cm of TiH2, off the source's centre line; and a response slab that faces the duct
instead of lying beside it. The response is the flux above 0.1 MeV in material 4, as in RM-M. Writes OUT_DIR/
analog.xml (no windows) and OUT_DIR/windows.xml (windows from OpenMC's own MAGIC generator on mesh 9000).
"""
from __future__ import annotations

import argparse
from pathlib import Path
import subprocess
import sys

CROSS_SECTIONS = Path(__file__).resolve().parents[3] / "data/raw/combined-fendl32-endfbvii1/cross_sections.xml"
ROOM, WALL_Y, OUTER = 150.0, (150.0, 350.0), (200.0, -200.0, 460.0)
DUCT_X, DUCT_Z = (12.0, 28.0), (-32.0, -8.0)
SLAB = ((-40.0, 80.0), (400.0, 410.0), (-80.0, 40.0))
SOURCE_HALF = 100.0
MESH = {"lower_left": (-200.0, -200.0, -200.0), "width": (20.0, 20.0, 20.0), "dimension": (20, 33, 20)}
ENERGY_BOUNDS = [0.0, 1.0e5, 1.0e6, 3.0e6, 6.0e6, 1.0e7, 1.3e7, 2.0e7]


def build(windows=None, generator: bool = False, particles: int = 10000, batches: int = 300):
    import openmc
    openmc.reset_auto_ids()
    tih2 = openmc.Material(6, name="tih2")
    tih2.set_density("g/cm3", 3.75)
    for n, a in (("H1", 0.66656284), ("H2", 0.00010382666666666666), ("Ti46", 0.0275), ("Ti47", 0.0248),
                 ("Ti48", 0.24573333333333333), ("Ti49", 0.018033333333333335), ("Ti50", 0.017266666666666666)):
        tih2.add_nuclide(n, a)
    coil = openmc.Material(4, name="response")
    coil.set_density("g/cm3", 8.422769375)
    for n, a in (("Cu63", 0.34942704694733445), ("Cu65", 0.15589044683044492), ("Fe54", 0.02891419248868879),
                 ("Fe56", 0.45389098675913614), ("Fe57", 0.01048232230684885), ("Fe58", 0.0013950046675466616)):
        coil.add_nuclide(n, a)
    mats = openmc.Materials([coil, tih2])
    mats.cross_sections = str(CROSS_SECTIONS)
    box = lambda x, y, z, **kw: openmc.model.RectangularParallelepiped(x[0], x[1], y[0], y[1], z[0], z[1], **kw)
    h, y0, y1 = OUTER
    outer = box((-h, h), (y0, y1), (-h, h), boundary_type="vacuum")
    back = openmc.YPlane(WALL_Y[1])                   # the wall's far face
    room = box((-ROOM, ROOM), (-ROOM, ROOM), (-ROOM, ROOM))
    duct = box(DUCT_X, WALL_Y, DUCT_Z)
    slab = box(*SLAB)
    cells = [openmc.Cell(name="room", region=-room), openmc.Cell(name="duct", region=-duct),
             openmc.Cell(name="shield", region=-outer & -back & +room & +duct, fill=tih2),
             openmc.Cell(name="response", region=-slab, fill=coil),
             openmc.Cell(name="beyond", region=-outer & +back & +slab)]
    st = openmc.Settings()
    st.run_mode, st.particles, st.batches, st.seed = "fixed source", particles, batches, 78000001
    st.source = openmc.IndependentSource(
        space=openmc.stats.Box((-SOURCE_HALF,) * 3, (SOURCE_HALF,) * 3), angle=openmc.stats.Isotropic(),
        energy=openmc.stats.Discrete([1.41e7], [1.0]))
    st.cutoff = {"energy_neutron": 1.0e5}
    st.statepoint = {"batches": [batches]}
    st.sourcepoint = {"write": False}
    st.output = {"tallies": False}
    mesh = openmc.RegularMesh(mesh_id=9000, name="ww-mesh")
    mesh.dimension, mesh.lower_left, mesh.width = MESH["dimension"], MESH["lower_left"], MESH["width"]
    if generator:
        st.weight_window_generators = [openmc.WeightWindowGenerator(
            mesh, energy_bounds=ENERGY_BOUNDS, particle_type="neutron", max_realizations=batches, update_interval=1)]
        st.weight_windows_on = True
    if windows is not None:
        ww = openmc.hdf5_to_wws(windows)[0]
        ww.id, ww.max_split = 1, 1000
        st.weight_windows, st.weight_windows_on = [ww], True
        st.weight_window_checkpoints = {"collision": True, "surface": True}
    tally = openmc.Tally(2, name="response_fast")
    tally.filters = [openmc.MaterialFilter([coil]), openmc.EnergyFilter([1.0e5, 2.0e7])]
    tally.scores = ["flux"]
    return openmc.Model(openmc.Geometry(cells), mats, st, openmc.Tallies([tally]))


def flat_windows(src: Path, dst: Path, reach: float, flat: float = 0.2, ratio: float = 5.0) -> None:
    """Windows that leave the bulk alone (lower bound `flat` everywhere above the cut-off) and expect weight
    3 x `reach` in the mesh layer holding the response slab, `reach` being the chance that a source neutron arrives
    there uncollided. s0f_los_windows.py then caps the bounds along the line of sight from that value. These are
    not adjoint windows: the second model has no adjoint pipeline, and this is the least that aiming needs."""
    import re
    import numpy as np
    x = src.read_text()
    i = x.index('<weight_windows id="1">')
    k = x.index("</weight_windows>", i)
    ww = x[i:k]
    n_e, (nx, ny, nz) = len(ENERGY_BOUNDS) - 1, MESH["dimension"]
    lower = np.full((n_e, nz, ny, nx), flat)
    lower[0] = -1.0                                   # below the cut-off
    idx = lambda lo, hi, d: slice(int((lo - MESH["lower_left"][d]) // MESH["width"][d]),
                                  int(-((hi - MESH["lower_left"][d]) // -MESH["width"][d])))
    lower[1:, idx(*SLAB[2], 2), idx(*SLAB[1], 1), idx(*SLAB[0], 0)] = 3.0 * reach
    upper = np.where(lower > 0, lower * ratio, -1.0)
    fmt = lambda a: " ".join(repr(float(v)) for v in a.ravel())
    ww = re.sub(r"<lower_ww_bounds>.*?</lower_ww_bounds>", f"<lower_ww_bounds>{fmt(lower)}</lower_ww_bounds>", ww, flags=re.S)
    ww = re.sub(r"<upper_ww_bounds>.*?</upper_ww_bounds>", f"<upper_ww_bounds>{fmt(upper)}</upper_ww_bounds>", ww, flags=re.S)
    dst.write_text(x[:i] + ww + x[k:])


def source_reach(reverse_statepoint: Path, face_area: float, cell: float = 10.0) -> float:
    """Mean over the source box of the uncollided reach probability onto the face (s0f_find_targets.reverse_map)."""
    import numpy as np
    import openmc
    dims = tuple(int(round(MESH["dimension"][d] * MESH["width"][d] / cell)) for d in range(3))
    with openmc.StatePoint(reverse_statepoint) as sp:
        f = sp.get_tally(name="uncollided").get_values(value="mean").reshape(dims, order="F")
    p = f / cell ** 3 * face_area / 4.0
    c = [MESH["lower_left"][d] + cell * (np.arange(dims[d]) + 0.5) for d in range(3)]
    m = [np.abs(a) < SOURCE_HALF for a in c]
    return float(p[np.ix_(*m)].mean())


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("out", type=Path)
    ap.add_argument("--magic-batches", type=int, default=8)
    ap.add_argument("--magic-particles", type=int, default=200000)
    args = ap.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    build().export_to_model_xml(args.out / "analog.xml")
    gen = args.out / "magic"
    gen.mkdir(exist_ok=True)
    build(generator=True, particles=args.magic_particles, batches=args.magic_batches).export_to_model_xml(gen / "model.xml")
    exe = str(Path(sys.executable).parent / "openmc")
    subprocess.run([exe, "-s", "4", "model.xml"], cwd=gen, check=True, capture_output=True)
    build(windows=gen / "weight_windows.h5").export_to_model_xml(args.out / "windows.xml")
    print(args.out / "windows.xml")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
