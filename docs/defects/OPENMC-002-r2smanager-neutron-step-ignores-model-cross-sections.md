# OPENMC-002: `R2SManager.step1_neutron_transport` ignores `model.materials.cross_sections` (0.15.3 and 0.16.0)

Status: drafted, not posted. Found by the S0 risk tests (R3) and reproduced in S0b D6. Draft for an issue against
`openmc-dev/openmc`.

## Versions and builds

| | Result |
| --- | --- |
| OpenMC 0.16.0 (conda-forge, `~/micromamba/envs/openmc016`) | fails |
| OpenMC 0.15.3 commit 27e38e89 (conda-forge `dagmc_nompi_py313h6424856_102`, `~/micromamba/envs/openmc0153dd`) | fails |

Linux x86-64, Python 3.13. Chain: a depletion chain with the usual (n,2n), (n,3n), (n,4n), (n,gamma), (n,p), (n,a) reactions
(the chain is not the subject).

## Reproducer

Run with `OPENMC_CROSS_SECTIONS` unset and `openmc.config["cross_sections"]` unset:
`python repro3_r2s_library_path.py /path/to/cross_sections.xml /path/to/chain.xml`.

```python
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
```

## Expected

The neutron step uses the library named on the neutron model, like `mesh.material_volumes(model)` just before it and like
`Model.run`. The reproducer shows that both of those find it.

## Observed

Both versions:

```
STEP: mesh.material_volumes ran with the model's library path
STEP: starting the R2S neutron step
 ERROR: No cross_sections.xml file was specified in materials.xml or in the
        OPENMC_CROSS_SECTIONS environment variable. ...
```

The error is raised inside the OpenMC library (the process exits with status 255), so no Python exception reaches the script.

Workaround: `openmc.config["cross_sections"] = library` before creating the manager.

## Cause (read from source)

`openmc/deplete/r2s.py`, `R2SManager.step1_neutron_transport`: v0.15.3 line 310 `with TemporarySession():`, v0.16.0 line 371
`with TemporarySession(output=False):`. With no model, `openmc.lib.TemporarySession.__init__` builds a minimal model
(`openmc/lib/core.py` v0.15.3 lines 654-660, v0.16.0 lines 692-698: a vacuum sphere, one empty cell, no materials, no
`cross_sections`) and initialises the library on it, so the library looks for `cross_sections.xml` through the environment or
`openmc.config` only. The neutron model reaches `get_microxs_and_flux` only after that initialisation. Checked directly with
`openmc.lib.TemporarySession(model)` (initialises) against `openmc.lib.TemporarySession()` (exits with the error above) on both
versions, with the library named only on `model.materials.cross_sections`.

(An alternative fix is `TemporarySession(self.neutron_model)`.)

## Status on develop

Still present on develop at aa4afc41eb (2026-10-10): `openmc/deplete/r2s.py` line 371 `with TemporarySession(output=False):`. The photon step in the same file (line 684) already passes its model, `TemporarySession(self.photon_model, cwd=photon_dir)`, so passing `self.neutron_model` in the neutron step would match it.

## Issue-search record (2026-10-10, `gh search issues|prs --repo openmc-dev/openmc`)

No open or closed issue or pull request describes this. Related history, not the same: #1843 (closed, 2021) "Operator should
respect Materials.cross_sections" (depletion operator); #3475 and #3505 (merged) introduced and adjusted `TemporarySession`;
#3508 (merged) added the R2S manager.

Raw output of the queries:

```
## issues: 'R2SManager'  (1 hits)
  #3518 [closed] MeshMaterialFilter bug with material ids
## prs: 'R2SManager'  (7 hits)
  #4046 [merged] Allow user tallies during R2S neutron transport step
  #3994 [merged] Make sure output is treated consistently in R2SManager
  #4013 [merged] Enable parent-nuclide tally breakdowns in R2S calculations
  #3860 [merged] Support multiple meshes in R2S calculations
  #3731 [merged] Optionally compute bounding boxes in Mesh.material_volumes
  #3632 [merged] Support MPI parallelism in R2SManager
  #3508 [merged] Automate workflow for mesh- or cell-based R2S calculations
## prs: 'TemporarySession'  (9 hits)
  #3980 [open] Deplete: Flux mode (mg-flux); faster collapse via SparseXSTable
  #3994 [merged] Make sure output is treated consistently in R2SManager
  #3942 [closed] wwinp files: Fix MemoryError in WeightWindowsList.export_to_hdf5 and speed up from_wwinp
  #3646 [merged] Add two MPI barriers in R2S workflow
  #3632 [merged] Support MPI parallelism in R2SManager
  #3569 [merged] Criticality search method on the Model class
  #3505 [merged] Allow already-initialized openmc.lib in TemporarySession
  #3475 [merged] Introduce `openmc.lib.TemporarySession` context manager
  #3466 [merged] Support flux collapse method in `get_microxs_and_flux`
```
