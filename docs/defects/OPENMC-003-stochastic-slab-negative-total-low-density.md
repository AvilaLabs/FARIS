# OPENMC-003: stochastic-slab MGXS can hold a negative total cross section for a low-density material; random ray then aborts (0.16.0)

Status: drafted, not posted. Found by the S0 risk tests (R6) and reproduced in S0b D6. Draft for an issue against
`openmc-dev/openmc`.

## Versions and builds

| | Result |
| --- | --- |
| OpenMC 0.16.0 (conda-forge, `~/micromamba/envs/openmc016`) | a group of the void stand-in has a negative total; the random-ray solve aborts |
| OpenMC 0.15.3 commit 27e38e89 (conda-forge `dagmc_nompi_py313h6424856_102`, `~/micromamba/envs/openmc0153dd`) | all totals positive; the random-ray solve completes |

Linux x86-64, Python 3.13, 3 threads. Nuclear data: FENDL-3.2 neutron HDF5 files (294 K) with an ENDF/B-VII.1 photon overlay.
The same negative value (-1.54e-4 in the second group of the filler, 6 to 13.5 MeV) appeared in the S0 R6 run on the full 30-solid DAGMC model, so it is
not specific to this small model.

## Reproducer

`python repro2_negative_total.py /path/to/cross_sections.xml` (about 25 minutes on 3 threads for 0.16.0; the 200 batches of
20000 particles are the stochastic-slab method's own settings). The script sets `openmc.config["cross_sections"]` first, so the
separate library-path defect (OPENMC-001) does not interfere. The default seed is used, and the result is reproducible: the S0
run on a different model produced the same -0.00015 in the same group.

```python
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
```

## Expected

A positive total cross section in every group, as 0.15.3 produces, so the random-ray solver can run on the converted model.

## Observed

0.16.0 (the material block prints, then the solver aborts):

```
openmc 0.16.0
beryllium_1: total by group = [0.11566 0.03182 0.15025 0.29134 0.48534 0.68078 0.71272 0.73614]
filler_2: total by group = [ 0.00039 -0.00015  0.00069  0.00166  0.00245  0.00314  0.0028   0.00618]
flibe_3: total by group = [0.09858 0.05694 0.1371  0.23476 0.4686  0.37978 0.68375 8.36737]
structure_4: total by group = [0.13532 0.13808 0.17525 0.17298 0.23651 0.50112 0.89978 1.06225]
tf_coil_5: total by group = [0.13809 0.1391  0.18186 0.22228 0.36511 0.57549 0.68735 0.86567]
thermal_insulation_6: total by group = [0.01719 0.01737 0.02208 0.02282 0.0332  0.06952 0.11415 0.13599]
tih2_7: total by group = [0.12736 0.06893 0.19396 0.33624 0.46862 1.51148 0.72376 1.41169]
tungsten_8: total by group = [0.16963 0.1718  0.22935 0.29413 0.40903 0.72014 0.94216 0.82031]
negative or zero totals (material, groups): [('filler_2', [1])]
RESULT: random-ray solve failed: No zero or negative total macroscopic cross sections allowed in random ray mode. If the intention is to make a void material, use a cell fill of 'None' instead.
```

0.15.3:

```
openmc 0.15.3
beryllium: total by group = [0.08148 0.10192 0.17139 0.29619 0.48757 0.67994 0.71056 0.73017]
filler: total by group = [0.00029 0.00032 0.00071 0.00158 0.00262 0.00331 0.00316 0.006  ]
flibe: total by group = [0.07574 0.09854 0.1393  0.23751 0.47411 0.38338 0.64049 7.22247]
structure: total by group = [0.1308  0.13982 0.18032 0.17675 0.23705 0.50336 0.89522 1.04992]
tf_coil: total by group = [0.13705 0.14715 0.18627 0.2217  0.36586 0.57512 0.67842 0.84846]
thermal_insulation: total by group = [0.01661 0.0178  0.02289 0.02353 0.03371 0.06953 0.11341 0.13419]
tih2: total by group = [0.11081 0.1207  0.20534 0.33251 0.48829 1.53694 0.74302 1.42929]
tungsten: total by group = [0.16176 0.18612 0.22879 0.29436 0.41183 0.72393 0.89776 0.75367]
negative or zero totals (material, groups): []
RESULT: random-ray solve completed
```

Further observation, not part of the failure: OpenMC MGXS arrays run from the highest energy group to the lowest, so the
second entry is the 6 to 13.5 MeV group and the last is the thermal group. In 0.16.0 that 6 to 13.5 MeV group's totals of the
hydrogen-bearing and beryllium materials are lower than in 0.15.3 for the same inputs, while iron and tungsten agree to a few
percent (beryllium 0.032 against 0.102, FLiBe 0.057 against 0.099, TiH2 0.069 against 0.121 cm^-1). The negative value is in the
lowest-density material of the same group.

## Notes on cause (read from source, not proven)

With `correction="P0"` the stochastic-slab path asks for the `nu-transport` MGXS (`openmc/model/model.py` v0.16.0 in
`_auto_generate_mgxs_lib`, the `mgxs_types` list for `correction == 'P0'`) and stores it as the group total
(`openmc/mgxs/library.py` v0.16.0 lines 1059-1062: `xsdata.set_total_mgxs(mymgxs, ...)`). That quantity is the total minus the
P1 scattering moment, and each is a tally estimate. For a material at 1e-3 g/cm3 the tallied P1 term can exceed the tallied total
in a group, so the difference is negative. No clipping was found in the code path read. In `_generate_stochastic_slab_mgxs` the
random-ray solver's own check (`No zero or negative total macroscopic cross sections allowed in random ray mode`) then stops the
run. This has not been confirmed by changing the code.

What changed between 0.15.3 and 0.16.0 (likely, not proven): PR #3658 ("Improved automatic MGXS generation for random ray",
merged 2025-12-03, first released in 0.16.0) replaced the slab source, which was a discrete source spread uniformly over all
groups, with the model's own source at strength 0.99 plus that uniform discrete source at strength 0.01
(`Model._create_mgxs_sources`). Its docstring says the uniform part is there "to ensure that total cross sections are
non-zero". With 1 % of the particles born uniformly over the groups, a group away from the source peak gets few tallies, and for
hydrogen (lab-frame mean scattering cosine 2/3, so the transport-corrected total is about a third of the total) at low density the
tallied P1 term can exceed the tallied total. The function is unchanged on develop (aa4afc41eb, 2026-10-10).

Possible remedies, for the maintainers to choose: flag or clip a non-positive transport-corrected total, keep a larger uniform
share, or fall back to the uncorrected total in a group whose correction is not resolved.

## Issue-search record (2026-10-10, `gh search issues|prs --repo openmc-dev/openmc`)

No open or closed issue or pull request describes this. Related, not the same: #2881 (closed) "Transport-corrected MGXS
generation with merging broken"; #4107 (open) and #4106 (open) "Add ability to set transport correction ratios externally";
#3315, #3316 (random ray void accuracy, merged).

Raw output of the queries:

```
## prs: 'negative total cross section'  (1 hits)
  #328 [merged] Check for negative values when calculating cross sections from probability tables
## prs: 'negative cross section random ray'  (1 hits)
  #3351 [merged] Random Ray AutoMagic Setup
## issues: 'random ray void'  (3 hits)
  #3723 [open] Explicit Void Regions in Kinetic Random Ray
  #3315 [closed] Random Ray Void Accuracy Issue
  #2989 [closed] Random Ray Explicit Void Treatment
## prs: 'random ray void'  (8 hits)
  #4082 [merged] Void guard kappa-fission when tallying in the random ray solver
  #3733 [merged] Random Ray main function and autosetup refactoring
  #3717 [merged] Local adjoint source for Random Ray
  #3316 [merged] Random Ray Void Accuracy Fix
  #3299 [merged] Random Ray Explicit Void Treatment
  #3430 [merged] Random Ray External Source Plotting Fix
  #2988 [merged] Fixed Source Random Ray
  #2823 [merged] Random Ray Transport
## issues: 'MGXS void material'  (2 hits)
  #3723 [open] Explicit Void Regions in Kinetic Random Ray
  #3929 [closed] Segmentation fault in Mgxs::get_xs when using DAGMCUniverse and multi-group energy mode
## prs: 'MGXS void material'  (1 hits)
  #3766 [merged] Approximate multigroup velocity 
## prs: 'negative sigma_t'  (1 hits)
  #3576 [merged] Random Ray Base Source Region Refactor
## issues: 'nu-transport'  (12 hits)
  #4179 [open] Delayed fission sites rejected by the time cutoff break analog fission tallies
  #4070 [open] MG mode: density_mult applied to total/absorption tally scores but not fission/nu-fission/scatter
  #1106 [open] Reopening of Issue #971: 3D Tally-Score Capability
  #4183 [open] Multigroup FileSource reads the group index stored in OpenMC source files as an energy in eV
  #3039 [closed] Incorrect tally behavior for fixed source, supercritical simulations in neutron-photon coupled mode
  #2881 [closed] Transport-corrected MGXS generation with merging broken
  #1238 [closed] Account for indirect energy release in depletion power normalization
  #657 [closed] Odd bug in openmc.Mgxs/openmc.Library
  #663 [closed] Unable to find merged tally when loading tally from statepoint
  #783 [closed] ValueError in mesh.py
  #989 [closed] Full Core Example 
  #594 [closed] Implement HDF5-Based MGXS Library Format
## prs: 'nu-transport'  (20 hits)
  #4180 [open] Fix analog fission tallies for sites rejected by the time cutoff
  #4053 [open] Add k-infinity estimator for IndependentOperator
  #4073 [open] Add photon multigroup cross section generation
  #4182 [open] Fix double filter weighting of delayed-nu-fission with EnergyoutFilter and DelayedGroupFilter
  #3740 [open] Add `chi` cross sections to MGXS machinery
  #4178 [open] Keep the lifetime clock and delayed group of (n,xn) and split neutrons
  #3702 [open] Random Ray Kinetic Simulation Mode
  #4067 [merged] Fix Mgxs::get_xs CHI_DELAYED indexing the wrong tensor when gout is null
  #1596 [open] Introduce the alpha-k iteration
  #3273 [merged] FW-CADIS Weight Window Generation with Random Ray
  #3191 [merged] Random Ray Adjoint Mode
  #2823 [merged] Random Ray Transport
  #1909 [merged] Removing Legendre filter in diffusion coefficient results
  #1667 [merged] Added DiffusionCoefficient class to mgxs.py
  #1705 [merged] MGXS API Changes for Arbitrary Reaction Channels and for Accurate Chi Computation
  #1451 [merged] Preparation for event-based parallelism (Thread private variable removal)
  #265 [merged] Expand tallying scores to include legendre and spherical harmonic expansions
  #807 [merged] Consistent Multi-Group Scattering Matrices
  #498 [merged] changed p % E to p % last_E for collision and analog tallies
  #502 [merged] Fine-Grained MGXS Library Domain Control 
## issues: 'transport correction'  (1 hits)
  #4107 [open] Add ability to set transport correction ratios externally
## prs: 'transport correction'  (17 hits)
  #4106 [open]  Add ability to set TCRs manually
  #4073 [open] Add photon multigroup cross section generation
  #4105 [closed] Add ability to set TCRs manually
  #3305 [open]  New Feature Development: The Implementation of chord length sampling (CLS) method for stochastic media( #3286 )
  #3351 [merged] Random Ray AutoMagic Setup
  #3235 [merged] Simulation of decay photons through the D1S method
  #1916 [merged] Condensing diffusion instead of transport
  #1909 [merged] Removing Legendre filter in diffusion coefficient results
  #1218 [merged] Speeding up import of transport corrected cross sections
  #1127 [merged] A few small fixes of docs and code comments
  #807 [merged] Consistent Multi-Group Scattering Matrices
  #644 [merged] Scattering Matrix Moments
  #793 [closed] Consistent Multi-Group Scattering Matrices
  #471 [merged] Multi-Group Cross Section Generation Module
  #361 [closed] Utilization of NDPP data for Tallying Estimators with increased efficiency [Much more readable history]
  #643 [merged] Integrated generation of Multi-Group Cross Section libraries with openmc.mgxs.Library
  #376 [merged] Optional nuclide-based isotropic in lab scattering
## prs: 'P0 correction MGXS'  (3 hits)
  #4073 [open] Add photon multigroup cross section generation
  #3351 [merged] Random Ray AutoMagic Setup
  #471 [merged] Multi-Group Cross Section Generation Module
## prs: 'random ray negative'  (1 hits)
  #4110 [merged] Adaptive Volume Estimators
## prs: 'negative total'  (3 hits)
  #4073 [open] Add photon multigroup cross section generation
  #3865 [closed] Potential fix for negative heating from Compton scattering with atomic relaxation
```
