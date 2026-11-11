# Compensated scalar heating for OpenMC 0.15.3

Apply `faris-openmc-0.15.3-compensated-heating.patch` to clean OpenMC commit
`27e38e894697bb32a1dac7848d2618818b6b8daf`. It carries the existing FARIS
compensated heating prototype and the guard against unsupported configurations.

The five `response-heating-*-whole-model` tallies use compensated event, batch
sum and sum-of-squares accumulation. Their versioned checkpoint records retain
the accumulator state. This prototype requires x86-64 Linux extended precision,
one MPI rank, reduced writable scalar heating tallies and the total-material
nuclide bin. The recorded control uses one thread.

The guard requires continuous-energy transport and an analog or collision
estimator. It rejects a matching-name multigroup tally or tracklength tally
during tally initialization. The C API also rejects a later change to
tracklength and preserves the previous estimator. Ordinary tally names retain
their normal estimator behavior.

The focused regression control is `controls/nuc013_tally_guard.py`. Run it with
the candidate OpenMC Python environment and explicit input paths.

```bash
/path/to/prefix/bin/python controls/nuc013_tally_guard.py \
  --prefix /path/to/prefix \
  --template /path/to/frozen-rm-s/solver \
  --cross-sections /path/to/cross_sections.xml \
  --out /path/to/new-control-output
```

The control uses separate processes for XML initialization, a late C API
estimator change and an ordinary-name comparison. It generates a synthetic
one-group cross-section fixture for the multigroup check. The tracklength
rejections must identify the new guard. OpenMC's existing score validator
rejects multigroup heating before the accumulator's defensive energy-mode
check is reached, and the control records that origin explicitly.
The receipt also records the supported one-port RM-S
closure and continuous-versus-restarted comparison.

See `references/rm-s-heating-guard-control-20261111.json` for the measured scope,
runtime and patch hashes. NUC-013 remains Partial while its output audit is open.

OpenMC's upstream copyright and MIT terms are reproduced in
`LICENSE-OpenMC-MIT`. Preserve that notice with the modified OpenMC source and
runtime. FARIS retains its repository license for its contributions.

## Build a local runtime package

The builder reads the pinned Git commit and vendor commits into a fresh output
directory. It builds single-rank CSG OpenMC with OpenMP enabled, MPI disabled and
DAGMC disabled. Use Python 3.12 or later, CMake 3.19 or later, Ninja and an x86-64
Linux C++ compiler with 64-bit extended-precision `long double`.

```bash
python3 integrations/openmc/risk_tests/build_heating_runtime.py \
  --source-repo /path/to/openmc-checkout \
  --vendor-root /path/to/checkout-containing-vendor \
  --dependency-prefix /path/to/dependency-environment \
  --cmake /path/to/cmake --ninja /path/to/ninja --cxx /usr/bin/c++ \
  --jobs 2 --output-dir runs/openmc-heating-runtime
```

The dependency prefix supplies Python, the OpenMC Python dependencies and native
libraries such as HDF5. The package records that prefix, interpreter and library
hashes, Python distribution versions and metadata hashes, source and patch
hashes, build commands, tool versions and license notices. Its Python environment
loads dependencies explicitly from the recorded prefix. Evaluated transport
libraries are supplied separately through the study's cross-sections path.

The output archive contains `bin/openmc`, `bin/python` and `lib/libopenmc.so`
under one prefix. Use those executable paths directly after extraction. The CLI
has the relative RPATH required by the existing FARIS adapter, and the Python
API uses the same shared library.

```bash
python3 integrations/openmc/risk_tests/verify_heating_runtime.py \
  runs/openmc-heating-runtime/faris-openmc-0.15.3-compensated-heating-linux.tar.gz \
  runs/openmc-heating-runtime-extracted \
  --reactor-transport integrations/openmc/reactor_transport.py \
  --output runs/openmc-heating-runtime-verification.json
```

The verifier checks the archive inventory, notices and dependency identities,
then runs the relocated CLI version command and Python C API probe with loader
overrides cleared. The [local package control](../../../references/heating-runtime-package-control-20261111.json)
records the clean builds, relocation and numerical controls. Archive integrity
regressions run with `python3 integrations/openmc/test_heating_runtime_package.py`.
