"""D6 candidate 3: R2SManager's neutron step ignores model.materials.cross_sections.

    python repro3_r2s_library_path.py /path/to/cross_sections.xml /path/to/chain.xml

Run with OPENMC_CROSS_SECTIONS unset and openmc.config['cross_sections'] unset, so the model's own path is the only library.
The other settings the manager needs (a pathlib.Path output directory and an explicit nuclide list) are given.
"""
import sys
import tempfile
from pathlib import Path

import openmc
import openmc.deplete

library, chain = sys.argv[1], sys.argv[2]
openmc.config["chain_file"] = chain       # the chain is not the subject; the library is
fe = openmc.Material(name="iron")
fe.add_nuclide("Fe54", 0.0585)
fe.add_nuclide("Fe56", 0.9175)
fe.add_nuclide("Fe57", 0.0212)
fe.add_nuclide("Fe58", 0.0028)
fe.set_density("g/cm3", 7.87)
sphere = openmc.Sphere(r=10.0, boundary_type="vacuum")
cell = openmc.Cell(fill=fe, region=-sphere)
model = openmc.Model(geometry=openmc.Geometry([cell]), materials=openmc.Materials([fe]))
model.materials.cross_sections = library  # the only place the library is named
model.settings.run_mode = "fixed source"
model.settings.batches, model.settings.particles = 5, 1000
model.settings.source = openmc.IndependentSource(space=openmc.stats.Point(), energy=openmc.stats.Discrete([14.06e6], [1.0]))
mesh = openmc.RegularMesh()
mesh.lower_left, mesh.upper_right, mesh.dimension = (-5, -5, -5), (5, 5, 5), (2, 2, 2)
print("openmc", openmc.__version__, "config cross_sections:", openmc.config.get("cross_sections"))
manager = openmc.deplete.R2SManager(model, mesh)
volumes = mesh.material_volumes(model, n_samples=2000)
print("STEP: mesh.material_volumes ran with the model's library path")
with tempfile.TemporaryDirectory() as tmp:
    try:
        # A missing library is a fatal error inside the OpenMC library (the process exits), so no RESULT line follows it.
        print("STEP: starting the R2S neutron step", flush=True)
        manager.step1_neutron_transport(Path(tmp) / "neutron", mat_vol_kwargs={"n_samples": 2000},
                                        micro_kwargs={"nuclides": fe.get_nuclides(), "energies": "VITAMIN-J-42",
                                                      "chain_file": chain, "run_kwargs": {"output": False}})
        print("RESULT: neutron step ran")
    except Exception as exc:  # noqa: BLE001
        print("RESULT: neutron step failed:", type(exc).__name__, str(exc)[:300])
