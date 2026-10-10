# Upstream defect drafts

Defects FARIS found in tools it depends on. **None has been reported upstream.** Each needs two things before Connor
decides whether to report it: verification that it is real (reproduced independently on a clean install, contrary to the
tool's documentation, and not intended), and an assessment of how significant it is (who hits it, whether it fails loudly
or gives a wrong answer silently, and whether a workaround exists).

| Draft | Verified real | Significance | Status |
| --- | --- | --- | --- |
| [OPENMC-001](OPENMC-001-stochastic-slab-mgxs-ignores-model-cross-sections.md): stochastic-slab MGXS ignores `Materials.cross_sections` | Yes | Low–moderate | Verified; not reported |
| [OPENMC-002](OPENMC-002-r2smanager-neutron-step-ignores-model-cross-sections.md): R2S neutron step ignores `Materials.cross_sections` | Yes | Moderate | Verified; not reported |
| [OPENMC-003](OPENMC-003-stochastic-slab-negative-total-low-density.md): negative transport-corrected total aborts random ray | Reproduces; real-world relevance not yet shown | Pending | Two verification runs outstanding |

## Verification record (2026-10-10)

**OPENMC-001.**

- **Reproduced independently** by the coordinator with `OPENMC_CROSS_SECTIONS` and `openmc.config` unset, on stock
  conda-forge builds. On 0.15.3 (`openmc0153dd`) `convert_to_multigroup` runs; on 0.16.0 (`openmc016`) it fails.
- **Contrary to the documentation.** OpenMC documents `Materials.cross_sections` as "the path to an XML cross section
  listing file … If it is not set, the OPENMC_CROSS_SECTIONS environment variable will be used" (`openmc/material.py`,
  develop). `Model.convert_to_multigroup` documents no exception.
- **Not intended:**
  - It is a regression; 0.15.3 kept the materials.
  - OpenMC fixed the same class of defect in depletion before: #1843, fixed by #1846 and #1898.
  - Still present on develop (aa4afc41eb).
- **Significance: low–moderate.**
  - The failure is loud and immediate. The error message is misleading, though: it says no library was "specified in
    materials.xml", while the user specified one.
  - Who hits it: users of random ray or automatic weight windows who name their library on the model rather than through
    the environment.
  - Workaround: set `openmc.config['cross_sections']`.
  - Fix: about two lines (carry the materials' `cross_sections` into the generation model).

**OPENMC-002.**

- **Reproduced independently** the same way. Both 0.15.3 and 0.16.0 exit with status 255 at the R2S neutron step, after
  `mesh.material_volumes` on the same model found the library.
- **Contrary to the documentation**, on the same grounds as OPENMC-001.
- **Not intended:** the photon step in the same file already passes its model to `TemporarySession`; the neutron step does
  not. Still present on develop.
- **Significance: moderate.**
  - The failure is a hard exit from inside the compiled library, with no Python exception and no traceback. It ends a
    notebook kernel or a calling service without saying where.
  - R2S is OpenMC's rigorous shutdown-dose workflow.
  - Workaround: the same global setting.
  - Fix: one line (`TemporarySession(self.neutron_model, ...)`).

**OPENMC-003.** Reproduced by S0b on 0.16.0, not on 0.15.3; same code on develop. Two questions remain before it can be
called significant:

1. **Realistic materials.** The material that went negative is a void stand-in (H-1 at 1e-3 g/cm³). Random ray supports
   explicit voids, and its error message says to use them, so a maintainer could call the stand-in a misuse. Next step: rerun
   with materials a fusion model really has at low density, helium coolant at about 8 MPa (about 0.006 g/cm³) and air
   (0.0012 g/cm³), in place of the stand-in.
2. **Cause.** The draft names PR #3658 (the slab source changed from fully uniform over groups to 1 % uniform) as the likely
   cause, read from the source, not tested. Next step: the same 0.16.0 run with the pre-#3658 source (uniform over groups
   at full strength) patched in. If the negative value disappears, the cause is confirmed.

Each run takes about 25 min. Both are scheduled after the current S0c runs.
