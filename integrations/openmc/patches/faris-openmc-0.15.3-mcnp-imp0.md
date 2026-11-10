# Optional MCNP neutron importance-zero support for OpenMC 0.15.3

This optional patch implements MCNP `imp:n=0` termination for the FNS decks
whose verified zero-importance CSG cells remain in the converted OpenMC model.
The current 14-deck map includes retained zero-importance regions in all 14
decks. The converter sidecar must identify each source deck hash, MCNP cell ID,
OpenMC cell ID, particle type, importance, and source region; no region or cell
is inferred from the surrounding sphere. OpenMC 0.15.3 and 0.16.0 do not
provide a per-cell neutron importance or kill-on-entry control. A vacuum
boundary on one enclosing sphere is not equivalent when an importance-zero
cell is a disjoint union that includes interior regions.

The patch is specific to the FARIS OpenMC 0.15.3 AIM-patched source. From the
OpenMC source root, apply the AIM patch first, then apply this patch with its
FARIS checkout path:

```bash
git apply /path/to/project-faris/integrations/openmc/patches/faris-openmc-0.15.3-aim.patch
git apply --unidiff-zero /path/to/project-faris/integrations/openmc/patches/faris-openmc-0.15.3-mcnp-imp0.patch
```

The patch was checked for clean application against the local 0.15.3 AIM-patched
source snapshot used by FARIS. No C++ build or solver run was performed for this
patch handoff. The compiled patch and control script are still pending the
coordinator-owned build and live control. Current hashes:

- AIM patch: `8a72af3a15891f5b1972b26427ad4fd414cd6e491fc42db8a0e9262ecbc89dcc`
- MCNP importance-zero patch: `477bd0b0cb9e1c801c9895da44cbe0a4075426a7ea19eb48232c61e71aea7156`

Ordinary reactor and maintenance runs must not contain the opt-in attribute;
they remain supported by stock OpenMC. For each benchmark whose reviewed
sidecar maps a retained `imp:n=0` neutron cell (the current gate maps all 14
decks), the runner injects the attribute
after OpenMC serializes `geometry.xml`:

```xml
<cell id="VERIFIED_OPENMC_CELL_ID" name="..." material="void" region="..."
      faris_neutron_importance="0" />
```

The attribute is allowed only with the exact value `0`. The C++ parser rejects
other values. The cell name is provenance text and does not control physics.
The adapter must bind the final post-injection geometry XML hash to the
benchmark run record. Before launching transport it must require the exact
selected CLI-linked `libopenmc.so` to export
`faris_openmc_mcnp_imp0_patch_version == 1`. Stock OpenMC ignores unknown XML
attributes, so a missing capability must fail closed before the solver runs.
The run record must keep the selected CLI path/hash and the actual linked
library path/hash separate.

A neutron born in a marked cell or entering one through a surface crossing is
terminated with weight zero. Birth and entry events are counted separately.
For a surface entry, the incoming particle weight is used for the surface
current tally first; then the particle is killed before scoring track length
inside the marked cell. Surface weight-window roulette is skipped for that
entry. A birth kill happens when OpenMC locates the particle's initial cell,
before its first track segment. The patch does not change photon transport,
cell names, source normalization, or unmarked cells. Intentional kills do not
increment OpenMC's physical geometry-loss counter and are not absorptions or
reaction counts.

Each statepoint written by the patched solver child contains these top-level
HDF5 datasets:

- `faris_mcnp_imp0_patch_version` (32-bit integer, currently `1`)
- `faris_mcnp_imp0_kill_events` (64-bit integer, total)
- `faris_mcnp_imp0_birth_kill_events` (64-bit integer)
- `faris_mcnp_imp0_entry_kill_events` (64-bit integer)
- `faris_openmc_n_lost_particles` (32-bit integer, OpenMC's ordinary loss count)

The total is the sum of birth and entry kills. These values are written by the
same CLI process that transported the histories and are restored on a patched
statepoint restart. If a marked model tries to restart from a statepoint without
the FARIS accounting datasets, the patch stops with an error. The release
runner must read and hash-bind these datasets from the actual statepoint; it
must not read a counter from a separately loaded parent-process library and
claim it describes the CLI child.

The kill counters use OpenMP atomics. The live control below is a single MPI
process; this patch handoff makes no multi-rank count claim.

## Required small runtime control

Once the patched binary is built, run the ready-to-use control (do not use a
stock binary):

```bash
python integrations/openmc/risk_tests/imp0_entry_control.py \
  --openmc-executable /path/to/patched/openmc \
  --cross-sections /path/to/openmc-0.15.3/cross_sections.xml \
  --output-dir runs/continuation/mcnp-imp0-control-20261109-v1
```

The script refuses a reused output directory, checks the exact CLI-linked
library capability, and records CLI, linked library, Python, cross-section index, final geometry,
and statepoint identities. It accepts any bounded regular `cross_sections.xml`
that lists neutron and photon catalogues and whose referenced library files
resolve. This is an index/path preflight only; it neither authenticates nor
qualifies the nuclear data. The file bytes and resolved library paths are
rechecked after the controls. The report includes a canonical `receipt_sha256`
covering the JSON record without that field; the validating runner recomputes
it. Each statepoint must report one completed batch, 1,000 particles per batch,
and one tally realization (OpenMC's `n_realizations` counts batches here). It runs four fixed-source cases with one batch of
1,000 histories, seed 271828, unit source strength, no weight windows, and one
MPI process. Each case uses three concentric void cells in the root universe:

- cell 1: sphere `r < 1 cm`, live source cell;
- cell 2: shell `1 < r < 1.5 cm`, live cell;
- cell 3: shell `1.5 < r < 2 cm`, with a vacuum boundary at `r = 2 cm`;
- the opt-in marker is placed on cell 3 for marked cases.

The cell flux tallies use OpenMC's Monte Carlo track-length convention (cm per
source history), so their expected means are the path lengths: 1 cm in cell 1,
0.5 cm in cell 2, and zero or 0.5 cm in cell 3. A surface-current tally on
`r = 1.5 cm` checks that the incoming weight is scored before the cell-3 kill.

Expected checks are:

| Case | Total kills | Birth kills | Entry kills | Lost particles | Cell flux means (1, 2, 3) | Mid-surface current |
| --- | ---: | ---: | ---: | ---: | --- | ---: |
| Marked neutron, source at origin | 1,000 | 0 | 1,000 | 0 | `1`, `0.5`, `0` | 1 |
| Marked neutron, source in cell 3 | 1,000 | 1,000 | 0 | 0 | `0`, `0`, `0` | 0 |
| Marked photon, source at origin | 0 | 0 | 0 | 0 | `1`, `0.5`, `0.5` | 1 |
| Unmarked neutron, source at origin | 0 | 0 | 0 | 0 | `1`, `0.5`, `0.5` | 1 |

These controls check initial-source and cell-entry semantics, separate event
and loss counts, the live-cell crossing, surface-current scoring, photon
behavior, and explicit-marker scope. They do not qualify any FNS benchmark or
reactor physics. Each benchmark run still needs its source, geometry, code,
tally, and uncertainty evidence.

## Coordinator-owned isolated build and control

After reviewing the local disk reserve and selecting the full FENDL neutron
index with the photon overlay used by the FNS material catalogue, the
coordinator can prepare a fresh AIM-patched source snapshot, apply only the
importance-zero patch to that copy, build with one job, check the runtime-linked
shared library, and run the live control with:

```bash
/home/connoravila/micromamba/envs/openmc0153dd/bin/python \
  integrations/openmc/risk_tests/build_mcnp_imp0_runtime.py \
  --cmake /home/connoravila/nuclear-data/p17-toolchain/root/usr/bin/cmake \
  --cmake-library-path /home/connoravila/nuclear-data/p17-toolchain/root/usr/lib/x86_64-linux-gnu \
  --cross-sections /path/to/fns-fendl32-with-photon-overlay/cross_sections.xml \
  --output-dir runs/continuation/toolchains/faris-openmc-0153-imp0-20261109-v4
```

Use the actual full neutron catalogue needed by FNS (including C, N, O and Pb);
the smaller historical 22-neutron/8-photon overlay is insufficient for that
model. The void control only checks index structure and path existence. The
driver refuses to reuse its output directory and requires at least 12 GiB
free before building, preserving an 8 GiB reserve plus 4 GiB build headroom. It
uses the full pinned OpenMC checkout at `27e38e894697bb32a1dac7848d2618818b6b8daf`;
the checked-in AIM patch must exactly match that checkout's nine-file working
diff. It copies that AIM-patched tree without `.git`, `build`, or `install`,
verifies source and vendored-submodule identities before and after copying, and
applies the imp0 patch only to the copy. The earlier `0feb9f3` development
repository is a small patch-authoring subset and is not a buildable OpenMC
source tree. Because the source copy has no `.git` and its upstream
`.git_archival.txt` contains unexpanded `$Format` placeholders, the driver uses
OpenMC's supported `ARCHIVAL_FILE` fallback. It writes that metadata outside
the source tree with the verified base commit and the explicit
`v0.15.3-faris-aim-imp0` build label, then checks the generated `version.h`
reports version 0.15.3, the exact base commit, and no development suffix. The
receipt records the metadata and generator hashes along with both patch hashes.
The CMake executable and its `LD_LIBRARY_PATH` are explicit and bounded to the
CMake subprocesses; runtime validation uses only the isolated OpenMC build and
its dependency environment. The resulting toolchain and `build-receipt.json` are stored under
`runs/continuation/toolchains/`; the live control receipt is written separately
to `runs/continuation/mcnp-imp0-control-20261109-v1/control.json` only when all
checks pass. This driver does not modify installed OpenMC environments.
