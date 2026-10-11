# Upstream defect drafts

Defects FARIS found in tools it depends on. **None has been reported upstream.** Each needs two things before Connor
decides whether to report it: verification that it is real (reproduced independently on a clean install, contrary to the
tool's documentation, and not intended), and an assessment of how significant it is (who hits it, whether it fails loudly
or gives a wrong answer silently, and whether a workaround exists).

| Draft | Verified real | Significance | Status |
| --- | --- | --- | --- |
| [OPENMC-001](OPENMC-001-stochastic-slab-mgxs-ignores-model-cross-sections.md): stochastic-slab MGXS ignores `Materials.cross_sections` | Yes | Low–moderate | Verified; not reported |
| [OPENMC-002](OPENMC-002-r2smanager-neutron-step-ignores-model-cross-sections.md): R2S neutron step ignores `Materials.cross_sections` | Yes | Moderate | Verified; not reported |
| [OPENMC-003](OPENMC-003-stochastic-slab-negative-total-low-density.md): negative transport-corrected total aborts random ray | Yes | Moderate–high | Verified; not reported |
| [DOUBLEDOWN-001](DOUBLEDOWN-001-ray-hit-layout-depends-on-tail-padding.md): ray-hit layouts agree only through tail-padding reuse; MSVC crash | Yes | Low now, latent | Verified; not reported |
| [OPENMC-004](OPENMC-004-first-check-in-invalid-window-splits-without-limit.md): a first window check in an invalid bin scales windows as if born at 1.0; the history splits without limit | Yes | Moderate–high | Verified; not reported |

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

**OPENMC-003.**

- **Reproduced** by S0b on 0.16.0, not on 0.15.3; same code on develop (aa4afc41eb).
- **Real materials (verified 2026-10-10).** Helium coolant at about 8 MPa (5.7e-3 g/cm3), a standard fusion blanket
  coolant, goes negative in the 6–13.5 MeV group and stops the random-ray solve. So the stand-in was not a misuse: real
  helium-cooled models hit it.
- **Cause (verified 2026-10-10).** With the slab source restored to its form before PR #3658 (uniform over groups at full
  strength), the same model has no non-positive total and the solve completes. The 1 % uniform share introduced by #3658
  is the cause on this model.
- **Significance: moderate–high.**
  - Who hits it: anyone generating random-ray weight windows (FW-CADIS) on a model with a low-density real material,
    helium coolant above all, on 0.16.0.
  - It fails loudly, but the message tells the user to make the material a void, which is wrong for a coolant.
  - It is intermittent: whether a group goes negative depends on how many slab particles reach it, so a model can pass on
    one seed and fail on another.
  - Workaround: stay on 0.15.3, or replace `Model._create_mgxs_sources` with the uniform source, as run B does.
  - Fix: maintainers' choice among the remedies in the draft (clip or flag a non-positive transport-corrected total, a
    larger uniform share, or the uncorrected total where the correction is unresolved).
- Details and output hashes: the draft's Verification section.

## Verification record (2026-10-11)

**DOUBLEDOWN-001.**

- **Reproduced** on the Windows runner (crash in batch 1, diagnosed under `cdb`; a DAGMC build without double-down ran
  the same model correctly, which places the fault in double-down). The patch removes the crash and gives Linux's
  result in all 21 bins.
- **The cause is confirmed on Linux** by a layout probe compiled with GCC 15.2 against the conda-forge headers: the four
  members `MBRay` and `MBRayAccumulate` add sit in `RTCDRay`'s tail padding (bytes 136–159 of 160), so `hit` is at 160 in
  all three hit structs only by that accident.
- **Still present** on `develop` at 437ad20 (2026-08-29): the same declarations.
- **Not contrary to documentation.** double-down does not claim Windows support; its CI runs on Ubuntu only, and none of
  its 57 issues mentions Windows or MSVC. The defect is undefined behaviour (one object read through two unrelated struct
  types) that happens to work under the Itanium ABI.
- **Significance: low now, latent.**
  - Who hits it today: anyone building double-down with MSVC, which in practice means a native Windows OpenMC with
    DAGMC. FARIS's Windows plan is such a build.
  - The failure on MSVC is loud (an access violation in the first batch).
  - Latent risk on Linux and macOS: a member added to `MBRay` or `MBRayAccumulate` beyond the 24 bytes of padding, or a
    change that shrinks it, moves `hit` there as well. The write would then land on `rh` or on the ray, which can crash
    or, if the overwritten values stay plausible, give wrong ray-fire results without an error.
  - Workaround: FARIS's patch (four members moved into `RTCDRay`).
  - Fix: that patch, plus a `static_assert` on the `hit` offsets.

**OPENMC-004.**

- **Reproduced independently** by the coordinator with a 100 cm iron cube and a 14.1 MeV point source on stock
  conda-forge 0.15.3 and 0.16.0. With every window bin valid, 200 histories take 1.2 s. With only the source bin set to
  −1, they do not finish in 120 s. On RM-M, filling E2′'s 16 invalid bins turned a stall into 30,000 histories in 8.8 s.
- **Contrary to the documentation and not intended.**
  - The code's own comment says windows are normalized "based on … the value of the weight window the particle was born
    in". A particle born in a rejected bin gets 1.0 instead, which is not a window of the file at all.
  - The generator that writes the −1 bins is OpenMC's own (`WeightWindowGenerator`, `update_weights`), so stock
    generator output can stall stock transport.
  - Still present on develop (aa4afc41e, `src/weight_windows.cpp` lines 966–967 and 982–988).
- **Significance: moderate–high.**
  - Who hits it: anyone applying FW-CADIS or MAGIC windows whose rejected bins lie where particles first meet a window.
    Most likely with fine meshes, few random-ray rays, or a source region with low flux statistics.
  - The failure is silent: no error or warning, and a batch that never ends. A run with a wall-clock limit records
    nothing, and the stall reads as a slow run or a hung job.
  - Workaround: fill rejected bins from their neighbours before transport (FARIS: `s0f_common.fill_file`), or lower
    `max_history_splits`.
  - Fix: set `wgt_ww_born` at the first valid window (a few lines), or have the generator never write −1 inside its
    energy range.
