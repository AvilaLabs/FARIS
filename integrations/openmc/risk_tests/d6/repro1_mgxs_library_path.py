"""D6 candidate 1: Model.convert_to_multigroup(method='stochastic_slab') ignores model.materials.cross_sections.

Run with OPENMC_CROSS_SECTIONS unset and openmc.config['cross_sections'] unset, so the model's own path is the only library.
    python repro1_mgxs_library_path.py /path/to/cross_sections.xml
"""
import sys
import tempfile

import openmc
import openmc.mgxs

library = sys.argv[1]
fe = openmc.Material(name="iron")
fe.add_nuclide("Fe56", 1.0)
fe.set_density("g/cm3", 7.87)
water = openmc.Material(name="hydrogen")
water.add_nuclide("H1", 1.0)
water.set_density("g/cm3", 0.1)
box = openmc.model.RectangularParallelepiped(-10, 10, -10, 10, -10, 10, boundary_type="vacuum")
split = openmc.XPlane(0.0)
left = openmc.Cell(fill=fe, region=-box & -split)
right = openmc.Cell(fill=water, region=-box & +split)
model = openmc.Model(geometry=openmc.Geometry([left, right]), materials=openmc.Materials([fe, water]))
model.materials.cross_sections = library          # the only place the library is named
model.settings.particles = 1000
model.settings.batches = 5
model.settings.run_mode = "fixed source"
model.settings.source = openmc.IndependentSource(space=openmc.stats.Point(), energy=openmc.stats.Discrete([14.06e6], [1.0]))
print("openmc", openmc.__version__, "config cross_sections:", openmc.config.get("cross_sections"))
with tempfile.TemporaryDirectory() as tmp:
    try:
        model.convert_to_multigroup(method="stochastic_slab", groups=openmc.mgxs.EnergyGroups([1e-5, 1.0, 1e3, 1e5, 2e7]),
                                    nparticles=2000, overwrite_mgxs_library=False, mgxs_path=tmp + "/mgxs.h5", correction="P0")
        print("RESULT: convert_to_multigroup ran")
    except Exception as exc:  # noqa: BLE001
        print("RESULT: failed:", type(exc).__name__, str(exc)[:300])
