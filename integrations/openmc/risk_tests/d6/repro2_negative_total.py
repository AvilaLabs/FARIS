"""D6 candidate 2: the MGXS library from convert_to_multigroup(method='stochastic_slab') can hold a negative total cross section
for a low-density material in one group, and the random-ray solver then aborts.

    python repro2_negative_total.py /path/to/cross_sections.xml [nparticles]

A void stand-in (H1 at 1e-3 g/cm3) beside iron, eight energy groups, P0 transport correction. The script sets
openmc.config['cross_sections'] so that candidate 1 (the model's own path being ignored on 0.16.0) does not interfere.
"""
import sys
import tempfile

import h5py
import numpy as np
import openmc
import openmc.mgxs

library = sys.argv[1]
nparticles = int(sys.argv[2]) if len(sys.argv) > 2 else 20000
openmc.config["cross_sections"] = library
edges = [1.0e-5, 1.0, 1.0e3, 1.0e5, 5.0e5, 2.0e6, 6.0e6, 1.35e7, 2.0e7]

fe = openmc.Material(name="iron")
fe.add_nuclide("Fe56", 1.0)
fe.set_density("g/cm3", 7.87)
filler = openmc.Material(name="filler")
filler.add_nuclide("H1", 1.0)
filler.set_density("g/cm3", 1.0e-3)
box = openmc.model.RectangularParallelepiped(-50, 50, -50, 50, -50, 50, boundary_type="vacuum")
shell = openmc.model.RectangularParallelepiped(-20, 20, -20, 20, -20, 20)
inner = openmc.Cell(fill=filler, region=-shell)
outer = openmc.Cell(fill=fe, region=-box & +shell)
model = openmc.Model(geometry=openmc.Geometry([inner, outer]), materials=openmc.Materials([fe, filler]))
model.settings.particles = 1000
model.settings.batches = 10
model.settings.run_mode = "fixed source"
model.settings.source = openmc.IndependentSource(space=openmc.stats.Point(), energy=openmc.stats.Discrete([14.06e6], [1.0]),
                                                 constraints={"domains": [inner]})
print("openmc", openmc.__version__)
with tempfile.TemporaryDirectory() as tmp:
    path = tmp + "/mgxs.h5"
    model.convert_to_multigroup(method="stochastic_slab", groups=openmc.mgxs.EnergyGroups(edges), nparticles=nparticles,
                                overwrite_mgxs_library=False, mgxs_path=path, correction="P0")
    with h5py.File(path, "r") as f:
        for name in f:
            if name in ("energy_groups", "group structure") or not isinstance(f[name], h5py.Group) or "294K" not in f[name]:
                continue
            total = np.asarray(f[name]["294K"]["total"])
            print(f"{name}: total by group = {np.array2string(total, precision=4)}; min = {total.min():.4g}")
            print(f"{name}: negative groups = {np.where(total <= 0.0)[0].tolist()}")
    # random-ray solve on the converted model
    model.settings.energy_mode = "multi-group"
    model.settings.batches, model.settings.inactive, model.settings.particles = 20, 10, 500
    model.settings.random_ray = {"distance_inactive": 20.0, "distance_active": 100.0,
                                 "ray_source": openmc.IndependentSource(space=openmc.stats.Box((-50, -50, -50), (50, 50, 50)))}
    try:
        model.run(cwd=tmp, output=False)
        print("RESULT: random-ray solve completed")
    except Exception as exc:  # noqa: BLE001
        print("RESULT: random-ray solve failed:", str(exc)[-400:])
