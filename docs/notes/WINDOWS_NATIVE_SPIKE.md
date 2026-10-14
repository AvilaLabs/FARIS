<!-- kind: explanation -->
# Windows native spike: OpenMC + DAGMC (double-down/Embree) + MOAB + HDF5 on Windows x64

Branch `windows-native-spike`, 2026-10-10 and 2026-10-11. Nothing here is
part of a FARIS release, and nothing was sent to any upstream project.

Question: can the FARIS transport stack be built with MSVC and run natively on Windows x64, driven through
OpenMC's C API, with no Python and no conda?

How to read the claims. **(M)** was measured on a GitHub `windows-latest` runner or on the Ubuntu runner in
the runs linked at the end. **(I)** is inferred from reading source or from the measured results and was not
tested.

## Verdict

**GO, conditional, for the engine.** (M) Every component built with MSVC from source or from official
binaries. OpenMC 0.15.3 with DAGMC and double-down/Embree 4 runs two models on Windows and reproduces the
Linux (conda-forge) tallies in every bin compared (21 bins, worst difference 0.00 sigma at the printed
precision). A Rust program linked against `libopenmc.lib` ran a DAGMC model through the C API and read back
the tally results and the error message buffer. The whole chain, from empty caches, builds in about 5 minutes
of compile time (about 9 minutes for the whole job including smoke tests).

What remains is engineering (packaging, patches carried per OpenMC version, UTF-8 manifest, CAD conversion on
Windows, tests on real models), not research. Details and estimates in the last section. The estimate is
about 3 to 4 weeks of one person for a production-quality Windows bundle, and the largest unknown is the
CAD-to-DAGMC conversion step, which this spike did not touch.

## Environment (M)

| Item | Value |
|---|---|
| Runner image | `windows-latest`, image version 20260925.250.1, Windows SDK 10.0.26100.0 |
| Compiler | MSVC `cl` 19.51.36260 for x64 (Visual Studio 18, toolset 14.51.36231), `/MD` runtime |
| Build tools | CMake 4.4.3, Ninja 1.13.2, git 2.55.0 |
| Rust (probe only) | cargo 1.98.1, `x86_64-pc-windows-msvc` |
| OpenMP | `/openmp:llvm` (LLVM runtime `libomp140.x86_64.dll`) |
| Linux reference | `ubuntu-latest`, micromamba, conda-forge: openmc 0.15.3 `dagmc_nompi_py313h6424856_102`, dagmc 3.2.4 `nompi_doubledown_h90b15eb_109`, double-down 1.1.0, embree 4.4.1, moab 5.6.0, hdf5 1.14.6 |

clang-cl was not needed.

## Components

Times are configure + build + install on the runner, excluding the git clone. The range is over the runs
where the stage was actually built (cold run and the earlier runs).

| Stage | Component | Built | Source / version | Build type | Patches | Time (M) |
|---|---|---|---|---|---|---|
| 1 | HDF5 | yes (official binary, not built) | HDF Group `hdf5-1.14.6-win-vs2022_cl.zip` | n/a | none | download 11 to 16 s |
| 1 | Embree + TBB | yes (official binary) | Embree 4.3.3 `x64.windows.zip`, ships `tbb12.dll` | n/a | none | in the 11 to 16 s above |
| 1 | Eigen3 | not used | n/a | n/a | n/a | n/a |
| 2 | MOAB | yes | 5.6.0, commit 3fa8ae9e8025 | Release, shared | none | 68 to 99 s |
| 3 | double-down | yes | v1.1.0, commit 1aa053273bd4 | RelWithDebInfo, shared | 2 | 5 to 16 s |
| 4 | DAGMC | yes | v3.2.4, commit 65c33fec0a85 | RelWithDebInfo, shared, double-down on | 1 | 9 to 12 s |
| 5 | OpenMC (CSG only build) | yes | v0.15.3, commit 27e38e894697 | RelWithDebInfo | 4 | 154 to 168 s |
| 5 | OpenMC (DAGMC build) | yes | v0.15.3, commit 27e38e894697 | RelWithDebInfo | 4 | 107 to 170 s |
| 6 | Smoke tests | yes | see below | | | 5 to 30 s per run |

Notes.

- **HDF5 version and channel.** HDF5 1.14.6, official HDF Group Windows zip (built with `cl`, VS 2022). (M)
  DAGMC issue 972 (HDF5 1.14.3+ ships no `libhdf5.lib`) did not occur: the 1.14.6 package ships both
  `libhdf5.lib` (static) and `hdf5.lib` (import library for `hdf5.dll`), and a CMake package at
  `<root>/cmake/hdf5-config.cmake`. CMake 4's `FindHDF5` took the config route and returned
  `hdf5::hdf5-shared`, so MOAB, DAGMC and OpenMC all link the same `hdf5.dll`. Only a RelWithDebInfo import
  target exists in that package, so the builds pass `CMAKE_MAP_IMPORTED_CONFIG_RELEASE=RELWITHDEBINFO;RELEASE`.
  Every compile gets `H5_BUILT_AS_DYNAMIC_LIB`. 1.14.6 was chosen because it is the version conda-forge uses
  for the Linux reference. 1.12 and 1.10 were not needed.
- **Why shared HDF5.** OpenMC PR 2919 links HDF5 statically for the Python loader. FARIS has no Python loader,
  and static HDF5 in both `MOAB.dll` and `libopenmc.dll` would give two copies of the library state. (I)
- **Eigen3.** MOAB uses it only with `ENABLE_EIGEN3=ON` and DAGMC only for `dagtally`; both are off. (M, the
  configure logs show no Eigen lookup)
- **MOAB** configured with HDF5 on, MPI off, Fortran off, BLAS/LAPACK off, tests off, NetCDF off, shared.
  It built unchanged. (M)
- **DAGMC** configured with `DOUBLE_DOWN=ON`, no Geant4/FLUKA/PyNE/UWUW/tally/tools/tests, shared only.
  `make_watertight` was not built.
- **OpenMC** configured with OpenMP on, MPI/MCPL/NCrystal/libMesh off, `OPENMC_USE_DAGMC=ON` for the DAGMC build.
- Cold-cache job: `windows-cad` took 8 min 34 s from start to finish, `windows-csg` 4 min 4 s. (M)

## Patches (all under `integrations/windows-spike/patches/`)

Each patch file starts with a prose header explaining why. Summary:

| Patch | Component | Why | Origin |
|---|---|---|---|
| `openmc/0001-cmake-msvc-shared-dll.patch` | OpenMC | 0.15.3 builds `libopenmc` STATIC under MSVC; it must be a DLL. Export-all-symbols, `OPENMC_DLL_EXPORTS`, `/openmp:llvm`, `H5_BUILT_AS_DYNAMIC_LIB` | PR 2919, minus its static-HDF5 block |
| `openmc/0002-dll-export-globals.patch` | OpenMC | `OPENMC_API` dllexport/dllimport for the globals that cross the DLL boundary (`openmc_err_msg`, `settings::run_mode`, `solver_type`, `mpi::master`, `filter_map`, the `*_ENABLED` flags) | PR 2919 |
| `openmc/0003-msvc-language-fixes.patch` | OpenMC | `omp atomic` without `seq_cst`, OpenMP reductions into locals, `<tuple>`, `and` -> `&&`, `M_PI` -> `PI`, iterator invalidated by `erase`, uninitialised variable | PR 2919, minus the `struct BoundaryInfo` change (see below) |
| `openmc/0004-windows-paths.patch` | OpenMC | `cross_sections.xml` library paths with a drive letter or backslashes | PR 2919 |
| `double-down/0001-msvc-dll-embree4.patch` | double-down | GCC-only flags, `__forceinline`/`__aligned` redefinitions, DLL export, install destinations, link `MOAB.lib` by full path | written for the spike |
| `double-down/0002-ray-struct-layout.patch` | double-down | Layout bug that crashed transport (next section) | written for the spike |
| `dagmc/0001-msvc-shared-doubledown.patch` | DAGMC | `FindMOAB.cmake` forced static libraries on MSVC and set POSIX-only PATH and library-suffix overrides; the shared install had no `RUNTIME` destination | written for the spike |

MOAB needed no patch. Beyond these patches the build uses `/bigobj`, `/FIiso646.h`, `-DNOMINMAX`,
`-D_USE_MATH_DEFINES`, `-D_CRT_SECURE_NO_WARNINGS` via `CXXFLAGS`, plus `CMAKE_POLICY_VERSION_MINIMUM=3.5`
because CMake 4 rejects double-down's `cmake_minimum_required(VERSION 3.4)`.

PR 2919 is written against OpenMC `develop`; the patches here are adapted from it to v0.15.3 (hunks that did
not apply were redone by hand). Its tests, CI and Python changes were not used.

## Failures met, and the fix 

| Stage | Failure (M) | Fix |
|---|---|---|
| 3 double-down | CMake 4 rejects `cmake_minimum_required` below 3.5 | `-DCMAKE_POLICY_VERSION_MINIMUM=3.5` |
| 5 OpenMC | `LNK2019 cross_lattice(..., BoundaryInfo const&)` unresolved: PR 2919 forward-declares `struct BoundaryInfo`, but in 0.15.3 it is a `class`, and MSVC mangles the two differently | dropped that hunk |
| 5 OpenMC (DAGMC build) | `LNK1181 cannot open input file 'MOAB.lib'`: double-down's exported link interface named the target `MOAB` | link `MOAB.lib` by path in double-down |
| 6 smoke | `0xC0000135` (DLL not found): my bundling script dropped Embree/TBB (a PowerShell operator-precedence mistake in the directory list) | bundle by walking `dumpbin /dependents` |
| 6 smoke | **access violation in transport, DAGMC model, double-down** | patch `double-down/0002`, below |

No component needed more than three fix attempts, so the six-attempt stop rule was not reached.

### The double-down crash (M, diagnosed under `cdb` on the runner)

Symptom: the DAGMC model started batch 1 and died with `0xC0000005`, with one thread as well as four. Stack:
`MOAB!GeomQueryTool::RayHistory::in_history` called from `dd!backface_cull` called from
`dd!MBDblTriIntersectFunc`, called by `embree4!rtcIntersect1`. The faulting pointer was the particle's
history pointer with its upper 32 bits zero.

Cause (read from the source, consistent with the register dump): in `MBDblTriIntersectFunc` the Embree
`args->rayhit` is read as `MBRayHit` (`MOABRay.cpp`), then `DblTriIntersectFunc` (`primitives.cpp`) reads the
same memory as `RTCDRayHit` and writes the hit at `sizeof(RTCDRay)`. `MBRay` derives from `RTCDRay` and adds
`orientation` and `rh`, so the hit lies at a different offset in the two types. GCC and Clang place the two
added fields in the tail padding of the non-POD base `RTCDRay`, so the offsets agree by accident. MSVC does not
reuse tail padding, the hit is written 16 bytes too early and overwrites `rh`. The same applies to
`MBRayAccumulate`.

Fix: the four extra members (`orientation`, `rh`, `sum`, `num_hit`) move into `RTCDRay`. This also removes the
latent difference on other compilers. (I) It would be worth sending upstream; that is Connor's call and was not
done.

Control: the same OpenMC and bundle with a DAGMC built without double-down (stock MOAB ray tracer, only
`dagmc.dll` swapped) ran correctly before the fix, which localised the fault to double-down.

## Smoke tests (M)

Models (`integrations/windows-spike/models/`, generated by `scripts/make_models.py`): three nested cubes with
half widths 5, 10 and 15 cm filled with H1 only (0.10, 0.30, 0.20 g/cm3, not physical), a 14.1 MeV isotropic
neutron point source at the origin, vacuum outside, neutron transport only, fixed-source, 20 batches of
20000 particles, seed 20261010, 4 OpenMP threads. Tallies: cell flux and absorption in the three cells,
cell flux in four energy groups, elastic and (n,2n) rates. 21 non-zero bins in total. Cross sections:
ENDF/B-VII.1 `H1.h5` (0.3 MB, public-domain evaluation, copied from the local OpenMC HDF5 library) with a
one-line `cross_sections.xml`. The CSG model is the same geometry written with planes. The DAGMC model is the
FARIS fixture `integrations/cad/fixtures/three-boxes.h5m` (three nested boxes), placed in a vacuum-bounded box with
`DAGMCUniverse.bounded_universe`.

The runs use only the bundle folder plus the Windows system directories on `PATH`, so a missing DLL fails
the run.

| Run | Windows result | Linux (conda-forge, double-down) |
|---|---|---|
| (a) CSG model, OpenMC built without DAGMC | leakage fraction 0.65322 +/- 0.00060 | 0.65322 +/- 0.00060 |
| (a) CSG model, OpenMC built with DAGMC | same | same |
| (b) DAGMC model, double-down/Embree (message "Using the DOUBLE-DOWN interface to Embree") | 0.65328 +/- 0.00086 | 0.65328 +/- 0.00086 |
| (b2) DAGMC model, stock MOAB ray tracer, `dagmc.dll` swapped | 0.65328 +/- 0.00086 | 0.65328 +/- 0.00086 |

Bin-by-bin comparison with `scripts/compare_results.py`, z = (a - b) / sqrt(sa^2 + sb^2):

| Comparison | Bins | Worst abs z | Bins over 3 sigma | Chi-square per bin |
|---|---|---|---|---|
| Windows CSG vs Linux CSG (both Windows builds) | 21 | 0.00 | 0 | 0.00 |
| Windows DAGMC (double-down) vs Linux DAGMC | 21 | 0.00 | 0 | 0.00 |
| Windows DAGMC (stock tracer) vs Linux DAGMC | 21 | 0.00 | 0 | 0.00 |
| Windows DAGMC vs Windows CSG | 21 | 1.55 | 0 | 0.44 |
| Linux DAGMC vs Linux CSG | 21 | 1.55 | 0 | 0.44 |

The Windows and Linux numbers agree to the printed precision (7 digits). The identical sigma between DAGMC
and CSG on both platforms is the statistical difference of the two geometry descriptions and shows that
the Windows run follows the same random walk as Linux. (I) That bit-level agreement across different compilers,
libm and Embree minor versions (4.3.3 here, 4.4.1 on conda) is more than the model has to give; it holds for
this small H1-only model and should not be expected for every model.

Throughput, one run each, 4 threads, different machines (so not a benchmark): Windows DAGMC with double-down
62.5k particles/s, Linux DAGMC 45.7k. The stock MOAB ray tracer was
about three times slower than double-down on Windows (21.6k particles/s) on this model.

### C API from Rust (M)

`integrations/windows-spike/capi-probe` is a small Rust program (no dependencies) that links
`libopenmc.lib` (`#[link(name = "libopenmc", kind = "dylib")]`), calls `openmc_init`, deliberately calls
`openmc_get_tally_index(9999)` and reads the exported global `openmc_err_msg` ("No tally exists with
ID=9999."), runs `openmc_run`, reads tally 1 through `openmc_tally_results`, and finalises. Ran for the
DAGMC model with both ray tracers; its tally means and standard errors equal the statepoint file's in all 6
bins (0.00 sigma). So the dllexport work covers the functions and the global that FARIS needs, and Rust's
MSVC target links and runs against it. Build time of the probe: 13 to 17 s.

### Paths (M)

- A run directory with spaces works.
- A run directory containing a non-ASCII character (`ü`) **fails** with `Failed to open HDF5 file` for
  `H1.h5`: HDF5 on Windows takes UTF-8 file names, while OpenMC hands it the strings from `getenv`, `argv` and
  `std::filesystem::path::string()`, which are in the ANSI code page. Giving `openmc.exe` an
  `activeCodePage=UTF-8` application manifest (`data/utf8.manifest`, merged with `mt.exe`) fixes it: the same
  model then ran from `win dagmc pfad ü utf8`. (I) The FARIS executable needs the same manifest when it
  loads `libopenmc.dll` in process (a non-ASCII Windows user name is enough to trigger it). Not tested with the
  Rust host.

## Bundle (M for sizes, from the DAGMC build)

`openmc.exe` 52 KB, `libopenmc.dll` 20.5 MB, `dagmc.dll` 1.2 MB, `dd.dll` 0.4 MB, `MOAB.dll` 5.9 MB,
`embree4.dll` 36.4 MB, `tbb12.dll` 0.3 MB, `hdf5.dll` 4.3 MB, `hdf5_hl.dll` 0.2 MB, `libomp140.x86_64.dll`
0.7 MB, `MSVCP140.dll` 0.6 MB, `VCRUNTIME140.dll` + `VCRUNTIME140_1.dll` 0.2 MB. About 70 MB unpacked. The
UCRT API sets and `KERNEL32.dll` come from the OS. `.pdb` files (large) were kept apart. `libomp140` and the
VC runtime on the runner came from `System32`; for a release they must come from the Visual Studio
redistributable folders under their redistribution terms. (I)

## Not tested

Photon, electron and thermal-scattering physics; real cross-section libraries; MPI; random-ray and weight
windows (patched code paths in `random_ray/` compile but did not run); mesh tallies; plotting; depletion;
models above a few dozen triangles; overlapping or leaky CAD; memory use and scaling; multi-minute runs;
Windows ARM64; Windows versions other than the runner's; code signing and antivirus behaviour; the CAD-to-DAGMC
conversion (`cad_to_dagmc`, cadquery) on Windows.

## Remaining work for native Windows CAD transport (estimates are mine, in working days)

| Item | Days |
|---|---|
| Move the spike scripts into a release workflow: pin sources by SHA and mirror them (MOAB comes from Bitbucket), cache, reproducible build, artifact | 3 |
| Carry the OpenMC patches per version (0.15.3 now; re-apply for each bump, watch PR 2919), carry the double-down and DAGMC patches, and decide whether to send the double-down layout fix upstream | 2 |
| Packaging: bundle layout, VC redistributable and `libomp140` from the VS redist folder, DLL search path / `SetDllDirectory`, licence texts, installer, signing | 3 to 4 |
| Rust host: link `libopenmc.lib`, UTF-8 `activeCodePage` manifest in the FARIS exe, error-buffer handling (shown to work), `build.rs` for the lib path | 2 to 3 |
| Windows path handling: spaces (works), non-ASCII, UNC, paths over 260 characters, temp dirs, file locking by antivirus, drive letters in `cross_sections.xml` (patched, spot-tested) | 2 |
| Validation on real FARIS models: photon and thermal physics, D1S/R2S flow, random-ray FW-CADIS, weight windows, mesh tallies, 1e5-1e6 facet CAD, memory and speed against Linux, threaded scaling, long runs | 5 to 8 |
| CAD-to-DAGMC on Windows (cadquery/OCP wheels, `cad_to_dagmc`, Gmsh or an alternative): not examined; if the Python helper stays, packaging a Python environment on Windows is a separate bundle | 3 to 6 (unknown) |
| CI for Windows (cache, runner minutes) and a regression test that compares Windows and Linux tallies on the models in this spike | 1 to 2 |
| Total | about 21 to 30 days |

Risks. (I) OpenMC upstream has not merged Windows support, so every OpenMC bump needs a rebase of four patches
. The double-down layout bug
shows that platform risk concentrates in ABI details that GCC and Clang hide; tests on real models should
include DAGMC point-in-volume and ray-fire with orientation and history. The runner image moved to Visual Studio
18 / MSVC 19.51 and CMake 4.4 without breaking the build, but toolchain drift will need pinning.

## Where things are

- Workflow: `.github/workflows/windows-native-spike.yml` (push to `windows-native-spike` or `workflow_dispatch`;
  docs-only pushes are ignored).
- Scripts, patches, models, data, probe: `integrations/windows-spike/`.
- Cold-cache run (all caches rebuilt, all jobs green):
  https://github.com/AvilaLabs/FARIS/actions/runs/38109117013
- Final run, including path tests (warm caches, all jobs green):
  https://github.com/AvilaLabs/FARIS/actions/runs/38109862342
- Earlier failed runs are in the repository's Actions list for branch `windows-native-spike`.
- Artifacts of each run: `windows-cad-logs` (configure, build and install logs, bundle log, `dumpbin`
  dependents, timings, `cdb` stack in the failed runs), `windows-cad-results` and `windows-csg-results`
  (statepoints and tally summaries), `linux-results`, `comparison` (the tables above).
