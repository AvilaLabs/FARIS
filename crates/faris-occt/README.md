# faris-occt

OpenCASCADE (OCCT) driven from Rust through a thin C++ shim, so FARIS can read STEP files and
tessellate solids without Python. This is the first slice of the native CAD layer described in
section 3 of `docs/notes/EASE_OF_USE_PLAN.md`.

## What it does

| Part | Function | Where |
| --- | --- | --- |
| A | Read a STEP file with `STEPCAFControl_Reader`: the declared length unit (read by OpenCASCADE and, independently, by a text parse of the file), and for every solid in reader order its name, assembly path, colour, volume, centroid, bounding box, validity (`BRepCheck_Analyzer`) and face count. Output is the `faris-step-inspect/v1` report that `faris_cad.py step-inspect` writes, with the extra solid fields `assembly_path`, `colour_srgb` and `faces`. | `inspect.rs`, `units.rs`, `cpp/shim.cc` |
| B | Mesh each solid alone with `BRepMesh_IncrementalMesh`, weld nodes by position, check each solid's mesh for closure, and write `faris-facets/v1` with the unimprinted flag. Same rules as `tessellate` in `faris_cad.py`. | `tessellate.rs` |
| C | Classify many (u, v) points against one face in a single native call (`BRepTopAdaptor_FClass2d`), replacing the per-point loop in `faris_deviation.py` (`exact_pairs`). | `native.rs`, `worker.rs` |

Face ids are 1-based, in `TopExp_Explorer` order within the solid (the numbering `tessellate`
uses for its surface ids). Solid indices are 0-based, in the order OpenCASCADE's STEP reader
returns them. State codes of the classifier: 0 in, 1 on, 2 out, 3 unknown.

## Process model

FARIS never calls OpenCASCADE in its own process. `client::Worker` starts
`faris-occt-worker REQUEST.json RESPONSE.json`, enforces a wall-clock limit by killing the
process, and reports a timeout, a crash (signal or exit without a response), a failure the worker
reported, or a malformed response. The worker sets `RLIMIT_AS` and `RLIMIT_CPU` on itself from
the request before it touches OpenCASCADE. Large data (facets, points, states) goes through
files named in the request. The request and response formats are in `src/worker.rs`.

`faris-occt-worker inspect STEP [--assume-unit mm]` prints the inspect report for use by hand.

The crate is a workspace member but not a default member, so building `faris-cli` does not build
OpenCASCADE. It is not wired into `faris design check`.

## Building

`build.rs` builds OpenCASCADE from the pinned source release with CMake as static libraries,
installs it, then compiles `cpp/shim.cc` with the `cxx` crate.

* Release: `V7_9_3` (OCCT 7.9.3, the version of the OCP 7.9.3.1 wheel behind `faris_cad.py`)
* Source: `https://github.com/Open-Cascade-SAS/OCCT/archive/refs/tags/V7_9_3.tar.gz`
* sha256: `5ecf094ec6b12d5413dfb851d8c3590c354058aee556e32e408bdfbf8c357d57`

The tarball's hash is checked before it is unpacked; a different file stops the build. Only the
toolkits `TKDESTEP`, `TKXCAF`, `TKBool`, `TKMesh`, `TKPrim` and `TKTopAlgo` and their
dependencies are built. Every module (`BUILD_MODULE_*`) and every optional third-party library
(FreeType, Tk, X11, TBB, VTK, ...) is off. The XCAF document model depends on `TKService`,
`TKV3d`, `TKHLR` and `TKVCAF`, so those are built too; none of them needs OpenGL or a display.

Environment variables (all optional):

| Variable | Meaning |
| --- | --- |
| `FARIS_OCCT_PREFIX` | Install and cache directory for the static OpenCASCADE. Default: inside the cargo output directory, so it is rebuilt after `cargo clean`. A stamp file in it records the tarball hash and the CMake options; if it matches, OpenCASCADE is not built again. |
| `FARIS_OCCT_TARBALL` | A local copy of the release tarball. Default: downloaded with `curl` into `<prefix>.src`. |
| `FARIS_CMAKE` | The cmake program. Default: `cmake` on the path. |

The OpenCASCADE compile runs at most three parallel jobs and writes only under the prefix. It
needs `cmake` (4.x works; the build passes `CMAKE_POLICY_VERSION_MINIMUM`), a C++17 compiler and,
for speed, `ninja`.

Because the first build compiles several thousand OpenCASCADE files, set `FARIS_OCCT_PREFIX` to a
directory outside the cargo target directory and reuse it.

## Parity scripts

`parity/` holds Python scripts that compare this crate with the Python helper on the same input
(run with the CAD environment: cadquery 2.8 / OCP 7.9.3.1):

* `compare_inspect.py PYTHON.json NATIVE.json` compares two step-inspect reports.
* `compare_tessellate.py PYTHON_DIR NATIVE_DIR` compares two `tessellate` outputs per solid.
* `classify_parity.py STEP --worker PATH --total-points N --scratch DIR` classifies random (u, v)
  points once per point in Python and once per face in the worker, and reports both times and the
  number of differing states.

## Licence

OpenCASCADE Technology is licensed under the GNU LGPL 2.1 with the OCCT exception
(`LICENSE_LGPL_21.txt` and `OCCT_LGPL_EXCEPTION.txt` in the source release). It is statically
linked into the `faris-occt-worker` program, which is part of FARIS, an AGPL-3.0 program. The
full source of FARIS and every build script, including this crate's `build.rs`, is public, and
the OpenCASCADE source is the unmodified release named above, so anyone can rebuild the program
with a different OpenCASCADE.
