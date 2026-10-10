# OPENMC-001: `convert_to_multigroup(method="stochastic_slab")` ignores `model.materials.cross_sections` (0.16.0)

Status: drafted, not posted. Found by the S0 risk tests (R6) and reproduced in S0b D6. Draft for an issue against
`openmc-dev/openmc`.

## Versions and builds

| | Result |
| --- | --- |
| OpenMC 0.16.0 (conda-forge, `~/micromamba/envs/openmc016`) | fails |
| OpenMC 0.15.3 commit 27e38e89 (conda-forge `dagmc_nompi_py313h6424856_102`, `~/micromamba/envs/openmc0153dd`) | works |

Linux x86-64, Python 3.13. Nuclear data: FENDL-3.2 neutron HDF5 files (294 K) with an ENDF/B-VII.1 photon overlay; the data is not
the point, any library named only through `model.materials.cross_sections` shows it.

## Reproducer

Run with `OPENMC_CROSS_SECTIONS` unset and `openmc.config["cross_sections"]` unset, and the `openmc` executable on `PATH`:
`python repro1_mgxs_library_path.py /path/to/cross_sections.xml`.

```python
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
```

## Expected

The generation run uses the library named on the model, as 0.15.3 does and as every other run of that model does.

## Observed

0.16.0:

```
openmc 0.16.0 config cross_sections: None
RESULT: failed: RuntimeError No cross_sections.xml file was specified in materials.xml or in the OPENMC_CROSS_SECTIONS environment variable. OpenMC needs such a file to identify where to find data libraries. ...
```

0.15.3 prints `RESULT: convert_to_multigroup ran`.

Workaround: `openmc.config["cross_sections"] = library` (or `OPENMC_CROSS_SECTIONS`) before the call.

## Cause (read from source)

`openmc/model/model.py` in v0.16.0, `Model._isothermal_stochastic_slab_mgxs`, lines 2372-2373:

```python
model = openmc.Model()
model.geometry = stoch_geom
```

The generation model is built from the slab geometry alone, so its `materials` collection is new and carries no `cross_sections`. In
v0.15.3 `Model._generate_stochastic_slab_mgxs` writes `model.materials = self.materials` (line 1920), which keeps the attribute.

## Status on develop

Still present on develop at aa4afc41eb (2026-10-10): `_isothermal_stochastic_slab_mgxs` still builds `model = openmc.Model(); model.geometry = stoch_geom`, and its body is identical to v0.16.0. No open pull request fixes it.

## Issue-search record (2026-10-10, `gh search issues|prs --repo openmc-dev/openmc`)

No open or closed issue or pull request describes this. Closest, not the same:
#1843 (closed, 2021) "Operator should respect Materials.cross_sections" (the depletion operator, a different code path);
#2542 (open) "`Materials.cross_sections` handles elements differently than OPENMC_CROSS_SECTIONS".

Raw output of the queries:

```
## issues: 'model.materials.cross_sections'  (3 hits)
  #2374 [open] Documentation on heating doesn't explain keff normalization
  #4185 [open] Model.init_lib raises AttributeError when directory is a pathlib.Path
  #3723 [open] Explicit Void Regions in Kinetic Random Ray
## prs: 'model.materials.cross_sections'  (1 hits)
  #1946 [merged] Depletion-related bug fixes
## issues: 'OPENMC_CROSS_SECTIONS multigroup'  (1 hits)
  #3367 [closed] LLVM 20 Fails at Loading MGXS Data Files
```
