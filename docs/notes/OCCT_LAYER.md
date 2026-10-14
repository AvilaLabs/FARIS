<!-- kind: explanation -->
# Native CAD layer, first slice (OpenCASCADE behind a C++ shim)

Status: built and measured 2026-10-11 on branch `occt-layer`; wired into `faris design check` the same day on branch `occt-wire`. Design:
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
* Wired into `faris design check` and `design init` as the CAD reader for the STEP inspect and the per-solid
  tessellation (see "Wired into `faris design check`" below). `faris-occt-worker inspect STEP` still prints the same
  report as `step-inspect` for use by hand. The workspace commands in `Makefile` and `ci.yml` exclude the crate,
  because its first build compiles about 4,500 OpenCASCADE files; `make occt-check` runs its gate.
* `faris-occt-client` (added with the wiring): the request and response types and the process client, with no
  OpenCASCADE and no build script. `faris-occt` depends on it and re-exports it, so the worker and its tests behave
  as before; `faris-engine` depends on it alone, so a normal FARIS build and CI never compile OpenCASCADE.

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

## Wired into `faris design check`

Done 2026-10-11 on branch `occt-wire`.

**Choosing the reader.** `faris design check` (and `design init`) take `--cad-reader auto|native|python` (default
`auto`, or `FARIS_CAD_READER`) and `--occt-worker PATH`. The worker is `--occt-worker`, else `FARIS_OCCT_WORKER`, else
a `faris-occt-worker` file next to the running `faris`. `auto` uses it when it is found and answers a `ping`, and the
Python helper otherwise; when a worker was configured or found but did not answer, the report says why in
`native_unavailable`. `native` with no usable worker stops with the reason and what to do. `python` never looks for a
worker and behaves as before.

**What moved to the worker.** The STEP inspect (check 2: units, solids, names, fingerprints) and the per-solid
tessellation of check 6, both with the schemas the helper writes (`faris-step-inspect/v1`, `faris-facets/v1` and
the tessellation report), unchanged. **What did not move:** the conversion (`cad_to_dagmc`), the overlap booleans, the
faceting deviation (GEO-032) and the source sampler stay on Python. The Python helper classifies faces only inside
the deviation (`exact_pairs`), so the batch face classification has no stage to replace yet; `Operation::Classify`
stays in the worker for when the deviation moves.

**Provenance and reuse.** Each reader stamp in the report holds the reader, the helper version and sha256 (for the
worker: its version and the sha256 of the binary), the worker's sha256 and the OpenCASCADE tag from the `ping`
(`V7_9_3`). It is on `cad_tool` (the inspect) and on `geometry.pre_imprint.tessellation_reader`. A tessellation
recorded in `--work-dir` is keyed by the reader identity: the Python helper's version as before (so older records
stay valid), or `native:<worker sha256>:<OpenCASCADE tag>`. A native result is never reused for a Python request, a
Python result never for a native one, and a different worker binary or a different OpenCASCADE tag is a different
reader.

**Bounds.** The client enforces the stage's wall-clock bound (15 min for the inspect, 1 h for the tessellation: the
Python stages' timeouts) and kills the worker on timeout or when the check is cancelled; the worker sets its own
address-space limit from the request (4 GiB for the inspect, 12 GiB for the tessellation, the Python stages' job
limits) and a CPU limit equal to the wall-clock bound (the worker is one thread). A timeout, a crash or a failure the
worker reports is a failed stage with a next step; a worker that cannot be started or answers unreadably ends the
check.

**Tests.** Without OpenCASCADE (CI): reader selection (`auto`, `native`, `python`, missing worker, worker that fails
its ping) and reuse by reader identity against stand-in workers (shell scripts that speak the JSON protocol), and the
client against stand-ins (timeout, cancel, crash, bad answer). With OpenCASCADE (`make occt-check`):
`crates/faris-occt/tests/reader_parity.rs` runs both readers through the engine on the four corpus files and, with
`-- --ignored` and `FARIS_RM_M_STEP`, on RM-M. It needs the CAD environment in `FARIS_CAD_PYTHON` (cadquery 2.8 /
OCP 7.9.3.1) and asserts the inspect reports equal (numbers to a relative 1e-12) and the facets byte-identical with the
same manifest (except the `helper_version` that names the writer) and the same problems. Both pass: corpus (3 solids
each, up to 158,760 triangles) and RM-M (30 solids, 1,915,942 triangles).

### Measurements: `faris design check` on RM-M, cached conversion

RM-M design file `rm-m.faris-design.json`, release build, conversion reused from an earlier run (hash, tags and
tolerances matching), tessellation and booleans recomputed, `--threads 4`, 6 GiB cgroup cap, the shared laptop. The
two runs were made back to back, Python first. All nine checks PASS in both and every check's findings are identical;
the facets of the tessellation are byte-identical.

| Load average (1, 5, 15 min) | start | end |
| --- | --- | --- |
| `--cad-reader python` | 3.01, 3.23, 3.28 | 8.16, 6.91, 4.71 |
| `--cad-reader native` | 8.16, 6.91, 4.71 | 4.08, 4.36, 4.17 |

(The native run started where the Python run ended, so its start load is the Python run's end load.)

| Stage | Python | Native |
| --- | --- | --- |
| STEP inspect (check 2) | 68.6 s | 41.1 s |
| Per-solid tessellation (check 6) | 74.9 s | 57.8 s |
| Check 6 pre-imprint, whole stage | 133.4 s | 148.6 s |
| Check 7 | 1.4 s | 1.4 s |
| Check 8 (integrity) | 32.3 s | 30.8 s |
| GEO-032 deviation (Python in both) | 159.2 s | 112.2 s |
| Check 9 | 11.0 s | 11.8 s |
| Checks 6 to 9 | 337.5 s | 305.1 s |
| Whole command, wall | 445.7 s | 382.6 s |
| Peak memory | 0.88 GB | 0.86 GB |

The two stages that changed reader are faster in this pair of runs, by 27 s (inspect) and 17 s (tessellation: the
worker reads in 20.1 s and meshes in 37.5 s on one thread; the helper read in 36.1 s and meshed in 24.9 s on three
processes). The whole pre-imprint stage was nevertheless 15 s longer. One cost the native run adds explains it: the
worker reads the STEP file itself and does not write the BREP shape cache the Python tessellation writes
(`work-dir/shapes`), so the first Python stage after it, the booleans, reads the file once more. The booleans' own
reported time (31.4 s and 34.0 s) leaves the read out; the stage's remaining time was about 30 s longer in the native
run, which matches a read of RM-M (30.9 s in the inspect) but was not timed separately. Writing the cache from the
worker belongs with the rest of the move off Python. The other rows are noise: GEO-032 runs the same Python code in
both runs and took 47 s less in the native one, so a single run of a stage moves by 30 % on this machine with other
jobs on it, and the whole-command row (63 s shorter) should not be read as the saving. The stand-alone numbers of
section B (237.8 s against 119.9 s for read plus mesh, one process each) are the cleaner comparison.

## Remaining work

1. **Release packaging of the worker.** Build `faris-occt-worker` into the release package next to `faris` (so `auto`
   finds it without a flag), with its licence notice and the OpenCASCADE source pin, for each platform; the wiring
   itself is done (above). Until then the worker comes from `cargo build -p faris-occt --release` and is passed with
   `--occt-worker` or found next to a `faris` built in the same target directory. Also: have the worker write the BREP
   shape cache, and move the deviation to the batch classifier so the Python path can be dropped.
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
