"""D6 candidate 2: the MGXS library from convert_to_multigroup(method='stochastic_slab') can hold a negative transport-corrected
total cross section for a low-density material in one group; the random-ray solver then aborts on it.

    python repro2_negative_total.py /path/to/cross_sections.xml

Eight materials as in a fusion blanket model, one of them a void stand-in (H1 at 1e-3 g/cm3). Eight energy groups, P0 correction,
20000 particles per batch (the 200 batches the stochastic-slab method uses). The default seed is used. The script sets
openmc.config['cross_sections'] so that the separate library-path defect (repro1) does not interfere. Whether a group goes
negative depends on the statistics of the slab run, so the result is the file the run writes, which is printed in full.
"""
import sys
import tempfile

import h5py
import numpy as np
import openmc
import openmc.mgxs

library = sys.argv[1]
openmc.config["cross_sections"] = library
edges = [1.0e-5, 1.0, 1.0e3, 1.0e5, 5.0e5, 2.0e6, 6.0e6, 1.35e7, 2.0e7]


def material(name, nuclides, density):
    m = openmc.Material(name=name)
    for nuclide, fraction in nuclides.items():
        m.add_nuclide(nuclide, fraction, percent_type="ao")
    m.set_density("g/cm3", density)
    return m


iron = {"Fe54": 0.05845, "Fe56": 0.91754, "Fe57": 0.02119, "Fe58": 0.00282}
tungsten = {"W180": 0.0012, "W182": 0.265, "W183": 0.1431, "W184": 0.3064, "W186": 0.2843}
titanium_hydride = {"H1": 0.66656, "H2": 0.0001, "Ti46": 0.0275, "Ti47": 0.0248, "Ti48": 0.24573, "Ti49": 0.01803, "Ti50": 0.01727}
flibe = {"Be9": 0.142857, "F19": 0.571429, "Li6": 0.257143, "Li7": 0.028571}
materials = [
    material("beryllium", {"Be9": 1.0}, 1.85),
    material("filler", {"H1": 1.0}, 1.0e-3),
    material("flibe", flibe, 2.137),
    material("structure", iron, 7.874),
    material("tf_coil", {"Cu63": 0.5, "Cu65": 0.2, **{k: 0.3 * v for k, v in iron.items()}}, 8.5),
    material("thermal_insulation", iron, 1.0),
    material("tih2", titanium_hydride, 3.75),
    material("tungsten", tungsten, 19.3),
]
# nested boxes, one material per shell, the void stand-in in the middle
bounds = [10.0 * (i + 1) for i in range(len(materials))]
surfaces = [openmc.model.RectangularParallelepiped(-b, b, -b, b, -b, b) for b in bounds]
surfaces[-1].boundary_type = "vacuum"
order = [1, 0, 2, 3, 4, 5, 6, 7]  # filler innermost
cells, previous = [], None
for position, index in enumerate(order):
    region = -surfaces[position] if previous is None else (-surfaces[position] & +previous)
    cells.append(openmc.Cell(fill=materials[index], region=region))
    previous = surfaces[position]
model = openmc.Model(geometry=openmc.Geometry(cells), materials=openmc.Materials(materials))
model.settings.run_mode = "fixed source"
model.settings.particles, model.settings.batches = 1000, 10
model.settings.source = openmc.IndependentSource(space=openmc.stats.Point(), energy=openmc.stats.Discrete([14.06e6], [1.0]))
print("openmc", openmc.__version__)
with tempfile.TemporaryDirectory() as tmp:
    path = tmp + "/mgxs.h5"
    model.convert_to_multigroup(method="stochastic_slab", groups=openmc.mgxs.EnergyGroups(edges), nparticles=20000,
                                overwrite_mgxs_library=False, mgxs_path=path, correction="P0")
    negative = []
    with h5py.File(path, "r") as f:
        for name in f:
            if not isinstance(f[name], h5py.Group) or "294K" not in f[name]:
                continue
            total = np.asarray(f[name]["294K"]["total"])
            print(f"{name}: total by group = {np.array2string(total, precision=5)}")
            if (total <= 0.0).any():
                negative.append((name, np.where(total <= 0.0)[0].tolist()))
    print("negative or zero totals (material, groups):", negative)
    model.settings.energy_mode = "multi-group"
    model.settings.batches, model.settings.inactive, model.settings.particles = 20, 10, 500
    model.settings.random_ray = {"distance_inactive": 20.0, "distance_active": 100.0,
                                 "ray_source": openmc.IndependentSource(space=openmc.stats.Box((-80, -80, -80), (80, 80, 80)))}
    try:
        model.run(cwd=tmp, output=False)
        print("RESULT: random-ray solve completed")
    except Exception as exc:  # noqa: BLE001
        print("RESULT: random-ray solve failed:", str(exc)[-400:])
