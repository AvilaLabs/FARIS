# Native CAD layer, first slice (OpenCASCADE behind a C++ shim)

Status: built and measured 2026-10-11 on branch `occt-layer`. Design:
[EASE_OF_USE_PLAN.md](EASE_OF_USE_PLAN.md) section 3. Crate: `crates/faris-occt` (its README says how to build
and use it).

## What exists

* OpenCASCADE 7.9.3 (tag `V7_9_3`, the release behind OCP 7.9.3.1) built from the official source tarball,
  `https://github.com/Open-Cascade-SAS/OCCT/archive/refs/tags/V7_9_3.tar.gz`, sha256
  `5ecf094ec6b12d5413dfb851d8c3590c354058aee556e32e408bdfbf8c357d57`, checked by `build.rs` before unpacking.
  Static libraries, 25 toolkits, no optional third-party library, no module switched on beyond the six toolkits
  asked for.
* A C++ shim (`cpp/shim.cc`) behind a `cxx` bridge: STEP read through `STEPCAFControl_Reader`, per-solid
  meshing with `BRepMesh_IncrementalMesh`, batch face classification with `BRepTopAdaptor_FClass2d`.
* `faris-occt-worker`: one request per process, wall-clock limit enforced by the caller, `RLIMIT_AS` and
  `RLIMIT_CPU` set by the worker on itself. Timeout, crash and reported failure are different errors.
* Not wired into `faris design check`. There is no `faris` subcommand; `faris-occt-worker inspect STEP` prints the
  same report as `step-inspect` for use by hand. The workspace commands in `Makefile` and `ci.yml` exclude the
  crate, because its first build compiles about 4,500 OpenCASCADE files; `make occt-check` runs its gate.

## Measurements

Machine: the shared 30 GB laptop, other jobs running, so times are indicative; Python and native runs were made
back to back. Python is the helper in `integrations/cad` with cadquery 2.8 / OCP 7.9.3.1.

### OpenCASCADE build

| | |
| --- | --- |
| Wall time (3 jobs, load average 9 to 11 on 8 cores) | 92 minutes, including the Rust dependencies |
| Peak memory of the whole build (cgroup) | 2.1 GiB (2.25 GB) |
| Compile units | 4,455 |
| Installed size | 252 MB |
| `faris-occt-worker`, release build | 41 MB |

### A. STEP reading (step-inspect)

Same solid count, order, names, validity and unit fields on all five files. Volumes agree to 2.1e-16 relative;
centroids and bounding boxes are bit-identical.

| File | Solids | Worst volume difference | Centroid, box | Python read + measure | Native read + measure |
| --- | --- | --- | --- | --- | --- |
| seeded-missing-material-boxes | 3 | 1.8e-16 | identical | 0.08 + 0.03 s | 0.04 + 0.02 s |
| seeded-missing-material-port | 3 | 1.2e-16 | identical | 0.05 + 0.02 s | 0.05 + 0.02 s |
| seeded-wrong-units-boxes | 3 | 1.8e-16 | identical | 0.05 + 0.02 s | 0.04 + 0.02 s |
| seeded-wrong-units-tori | 3 | 1.6e-16 | identical | 0.02 + 0.20 s | 0.02 + 0.20 s |
| RM-M | 30 | 2.1e-16 | identical | 30.9 + 27.9 s (64 s wall, 852 MB) | 19.9 + 21.8 s (43 s wall, 395 MB) |

The native report also holds, per solid, the assembly path (`rm_m/plasma`), the face count and the colour (none of
these files carries a colour). The corpus tests compare against the reports recorded in the repository; the
`parity/compare_inspect.py` script compares against a live Python run.

### B. Tessellation (chord 0.1 cm, angular 0.1 rad, shape unit 1 mm)

The native `faris-facets/v1` files are byte-identical to the Python helper's on all five models (same vertices
in the same order, same triangles, same surfaces). All solids close: 3 of 3 on each corpus file, 30 of 30 on
RM-M, 1,915,942 triangles on RM-M.

| Model | Python, one worker | Native, one thread |
| --- | --- | --- |
| RM-M STEP read | 65.9 s | 41.0 s |
| RM-M meshing | 148.0 s | 77.3 s |
| RM-M total wall | 237.8 s | 119.9 s |
| RM-M peak memory | 1.0 GB | 0.63 GB |

The 33 s mesh and 54 s read recorded for the Python helper were made with a pool of three worker processes; the
rows above are single-process, so they compare like with like. Native meshing is not yet spread over solids.

### C. Batch face classification

For every face of the chosen solids, random (u, v) points in the face's UV box widened by 5 %; Python classifies one
point at a time with `BRepTopAdaptor_FClass2d` (the loop in `exact_pairs`), the worker classifies each face's points
in one call.

| Model | Faces | Points | Python per-point | Native batch | Different states |
| --- | --- | --- | --- | --- | --- |
| seeded-wrong-units-tori | 5 | 300,000 | 3.0 s | 0.25 s | 0 |
| seeded-missing-material-port | 20 | 300,000 | 3.0 s | 0.31 s | 0 |
| seeded-wrong-units-boxes | 30 | 300,000 | 2.9 s | 0.14 s | 0 |
| RM-M, all faces | 13,824 | 4,783,104 | 64.1 s | 4.1 s | 0 |

That is 0.9 s per million points native against 13.4 s per million in Python, 15 times faster. The 138 s recorded
for the 4.79 million points of the RM-M deviation check includes the foot-point computation in numpy as well as
the classifier; this run times the classifier alone. The Rust test
`batch_classifier_matches_per_point_calls_on_every_face` also compares the batch call with per-point calls (a new
classifier for each point) on every face of three corpus models, and
`reference_classifier_agrees_with_the_batch_classifier_off_the_boundary` compares it with a different OpenCASCADE
algorithm (`BRepClass_FaceClassifier`).

## Deviations from the design

* OpenCASCADE's XCAF document model depends on `TKService`, `TKV3d`, `TKHLR` and `TKVCAF` (the Visualization and
  application-framework toolkits), so they are built. No OpenGL, X11 or FreeType is involved. They add to the
  4,455 compile units.
* `unsafe_code` is denied instead of forbidden in this crate, with one allowance on the bridge module, because the
  `cxx` bridge expands to unsafe blocks. The rest of the workspace still forbids it.
* The crate is excluded from workspace-wide `make check` and CI (see above).
* The worker takes its address-space and CPU limits from the request and sets them on itself; the wall-clock limit is
  enforced by the caller. This keeps the caller free of `unsafe` and of `pre_exec`.
* Facets are written by `faris-model`'s own writer, so the format has one definition on the Rust side.

## Remaining work

1. **Wire into `faris design check`.** Add the worker as the CAD reader beside the Python helper, run both on the
   corpus and RM-M in CI-equivalent tests until the Python path can be dropped. The step-inspect, facets and
   tessellation outputs already have the helper's schemas. Needs a way to find the worker next to `faris`, and the
   worker built into the release package.
2. **Local imprint of touching pairs only.** A shim function that takes two solids (indices) and runs
   `BOPAlgo_Builder`/`BRepAlgoAPI_Fuse`-style general fuse on that pair, returning the shared faces and the two
   solids' split faces, with the same result checks as the exact integrity check. `TKBool` is already linked.
   Candidate pairs come from the existing bounding-box and facet tests.
3. **Native meshing for conversion.** Mesh the imprinted shapes (face ids carried through the imprint) and weld by
   topology rather than by position; then parallelise over solids in a pool of worker processes (the Python helper
   used three, native single-process meshing of RM-M is already 2 times faster).
4. **DAGMC `.h5m` writer.** Plain HDF5 in MOAB's layout, written from the facets and the solid-to-material map,
   with a gate that loads every output through MOAB, DAGMC and OpenMC. Needs an HDF5 binding (or the C library
   built the same way) and a decision on the MOAB writer fallback.
5. **Also open:** IGES and BREP readers (`TKDEIGES` and `BRepTools::Read` are not built), colours and layers on
   real assemblies (none of the test files carries a colour), a worker pool, and macOS and Windows builds of the
   OpenCASCADE step (the CMake options are portable; the build has only run on Linux).
