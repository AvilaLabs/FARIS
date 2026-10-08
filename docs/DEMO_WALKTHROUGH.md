# FARIS functional demo walkthrough

This walkthrough is for the identified local FARIS workspace. It uses a cold-data
numerical surrogate; it does not reproduce ARC or establish a qualified reactor
prediction. The corrected campaign contains four million-history coupled runs with exact
geometry/data identities. Earlier neutron-only and incorrect-clearance runs
remain superseded diagnostics, and must not supply final reactor conclusions.

The verified local offline distribution is at
`dist/FARIS-demo-2026-10-01`. Its exact index and final native checks are recorded
in [portable verification](../references/portable-demo-verification.json) and
[native verification](../references/native-demo-verification.json).
The short native demonstration is:

1. Run the distribution's `verify.sh` (Linux, with the evidence part), then open it
   with `bin/faris-app --package <dir>`, or double-click `bin/faris-app`. No OpenMC installation
   or nuclear data is needed for recorded exploration.
2. Orbit the finite-port scene, select a layer, and inspect its physical inputs.
   Switch allocations and **Show matched control** while retaining the same
   field scale. The cutaway changes only the display.
3. Use **Flux slice** and click a bin; inspect its domain, mean, sampling error
   and source identity. Read the unresolved precision warning before drawing a
   local-map conclusion. **Nuclear heating** shows deposited transport heat,
   not a temperature calculation.
4. Press **Compile study** and inspect the actual Core findings and small
   **Powered by Avila Core** attribution. Saved-case evidence and the current
   draft's compilation have separate identities and states.
5. Scrub the baseline history through an annual planned outage. Inventory,
   power, exposure and cumulative energy follow calculated snapshots. Reference
   flux/heating fields remain explicitly stationary; **Component fluence**
   follows the history.
6. In the left panel, select **Replacement-event demonstration**. Let the shared
   Rust ledger recalculate, then jump to a replacement or permanent-limit event.
   Change recovery or opening fuel under **Operating assumptions** and press
   **Recalculate history**. Earlier curves remain marked until completion.
7. Inspect **Conditional sensitivity** and the packaged paired comparisons.
   The 27 full-history reruns probe recovery, processing delay and service limits.
   A separate packaged study varies annual planned outages over 15, 30 and 60
   days for each transport driver, keeping the remaining baseline inputs fixed.
   These authored ranges are not lifetime
   confidence bounds. The result can remain an unresolved engineering tradeoff.

Current source builds include **Interface size** in the top bar. It scales text,
controls, panels, and plots; Ctrl/Cmd + or − also adjusts size, and Ctrl/Cmd 0
resets it to the desktop-scaled default. `--interface-scale 1.25` starts at 125%.
For normal review, preserve native display detection: avoid forcing X11 or
`WINIT_X11_SCALE_FACTOR=1`, which made the original review session too small
on this workstation's high-DPI desktop. The frozen indexed distribution retains
its original binary and keyboard zoom; the new visible menu is in the updated
source-built app.

Fresh transport and reproducible CLI/export instructions follow below. It takes
substantially longer than recorded exploration, and its sampling quality must
be checked independently of successful execution.

## 1. Check the inputs and tools

Use the scenario, physics case, and nuclear-data audit from the same run branch.
The unported cold reference is a numerical control; the separate coupled-control
and finite-port branches have different scenario byte identities. Never pair a
run record with a similarly named but different scenario file.

```bash
cargo run -- validate scenarios/arc-inspired/cold-coupled-control.scenario.json
cargo run -- reactor --help
cargo run -- history --help
cargo run -- evidence --help
```

For fresh transport, install/use OpenMC 0.15.3 and Python from the documented
local environment, then acquire and audit data following
[PHOTON_LIBRARY_ACQUISITION.md](PHOTON_LIBRARY_ACQUISITION.md). The local audit
records are workstation-specific. Check `references/openmc-library-audit.json`
and `references/photon-library-provenance.json`; the latter documents unresolved
redistribution terms. Never copy nuclear data into the demo bundle.

## 2. Open and inspect the native workspace

Build and launch the app with the same scenario and both physics variants. Load
only run records whose exact scenario hash matches the selected scenario.

```bash
cargo run -p faris-app -- \
  --scenario scenarios/arc-inspired/cold-coupled-control.scenario.json \
  --physics scenarios/arc-inspired/cold-coupled-control.reference.physics.json \
  --physics scenarios/arc-inspired/cold-coupled-control.breeder-emphasis.physics.json \
  --field-view flux-slice
```

Inspect the radial allocation, source and material assumptions, and declared
surrogates. Orbit, zoom, select a component, hide/show it, and use the display
cutaway. The cutaway is a viewport operation; it must not alter the solver case.
Switch the scalar field and read its quantity, units, field scale, sampling error,
and source record. Mesh bins average the full bin volume, including void; a bin
is not a pointwise peak. Gray/empty bins must not be interpreted as proof of
zero flux.

For a final paired spatial comparison, use the corrected ported and matched
feature-free control scenarios/runs supplied with the release. Confirm that their
physics cases bind to their own exact scenario SHA-256 and that the declared
port dimensions and affected component IDs are visible. Both corrected port cases have matching OpenMC ownership/volume and independent
adaptive-quadrature reports. The directly tallied mixed-volume port window
resolves a sampling difference, while fine/coarse map diagnostics do not
establish local mesh convergence. Read [the refinement receipt](../references/transport-refinement-results.json)
before interpreting local bins or the magnet-region estimates.

## 3. Compile and inspect Core evidence

The desktop's **Compile study** invokes the configured actual Core compiler.
Confirm the attribution appears only for that attempt, inspect the compiled
identity and findings, and distinguish compilation from run readiness and
scientific assessment. Select breeding, shielding, heating, history, and energy
only when the relevant inputs are present; missing responses must remain visible
as gaps.

For CLI evidence from a revalidated run, prepare a generated case and execute it
with the pinned local Core binary:

```bash
cargo run -- evidence prepare \
  --run runs/<eligible-variant>/run.json \
  --study runs/<generated-study>/study.json \
  --assumptions scenarios/arc-inspired/demo-operating-assumptions.json \
  --core /path/to/avila-core --output /tmp/faris-core-case-<variant>

cargo run -- evidence run \
  --case /tmp/faris-core-case-<variant> \
  --core /path/to/avila-core \
  --workspace /tmp/faris-core-work-<variant> \
  --output /tmp/faris-core-report-<variant>.json
```

Use fresh output paths. The evidence record binds declarations and stage inputs;
Core is not the OpenMC solver and a complete stage report is not a qualified
physical conclusion. The four Core states shown by software fixtures describe
Core semantics, not verdicts on this reactor scenario.

## 4. Run or replay transport

The replay path checks the saved input, scenario, data audit, adapter, raw tally,
volume, and normalization identities before exposing results:

```bash
cargo run -- reactor inspect \
  --scenario scenarios/arc-inspired/cold-coupled-control.scenario.json \
  --run runs/<eligible-variant>/run.json
```

Before generating the history/energy Core case, generate a study with all four
analyses selected (`breeding,shielding,fuel-history,electricity`) using
`faris study generate`. Use its `study.json` in `evidence prepare`; the default
case selection requests only breeding and shielding.

```bash
cargo run -- study generate \
  --scenario scenarios/arc-inspired/cold-coupled-control.scenario.json \
  --variant reference \
  --analysis breeding,shielding,fuel-history,electricity \
  --output /tmp/faris-study-reference
```

For a fresh run, provide an exclusive output directory and the explicit tool/data
paths. The current default is 100 batches × 10,000 particles and one thread;
the CLI accepts at most 10 million total histories, 32 threads, and 3,600
seconds per job. Linux execution inherits 4 GiB address-space and 256 MiB
per-file limits; the runner monitors at most 512 MiB and 2,048 regular artifact
files across its working/output roots. These are process/output ceilings, not a
complete hostile-code sandbox or a measured production resource budget. The
CLI reports execution separately from scientific
qualification. Ctrl-C cancels the owned job; a partial directory is not a
completed result.

```bash
cargo run -- reactor run \
  --scenario scenarios/arc-inspired/cold-coupled-control.scenario.json \
  --physics scenarios/arc-inspired/cold-coupled-control.reference.physics.json \
  --audit references/openmc-library-audit.json \
  --cross-sections /path/to/combined-fendl32-endfbvii1/cross_sections.xml \
  --python /path/to/openmc-env/bin/python \
  --openmc /path/to/openmc-env/bin/openmc \
  --batches 100 --particles 10000 --seed 123456789 --threads 1 \
  --timeout-seconds 3600 --output runs/<new-exclusive-run>
```

Repeat with breeder-emphasis and an independent seed. For comparisons, also run
the feature-free control and finite port as separate, explicitly identified
branches. A seed change is a distinct Monte Carlo execution. Report sampling
standard errors separately from physical/data/model uncertainty; the latter is
not currently quantified.

## 5. Recalculate operating history and compare

Only use a history record generated from a revalidated eligible transport run.
The example assumptions are authored controls: recovery, inventory, delay,
operating schedule, service thresholds, thermal recovery, efficiency, and loads
are not measured plant specifications. Missing heating must keep recovered heat
and net electricity unavailable.

```bash
cargo run -- history from-run \
  --scenario scenarios/arc-inspired/cold-coupled-control.scenario.json \
  --run runs/<eligible-variant>/run.json \
  --assumptions scenarios/arc-inspired/demo-operating-assumptions.json \
  --output /tmp/faris-history-<variant>.json
```

Scrub operation and planned outages in the baseline. Select the separate
**Replacement-event demonstration** preset to inspect blanket replacement and
the permanent magnet-limit event. Its thresholds are explicitly authored to
exercise those paths. Fuel starvation/restart is independently checked with
limiting-case controls; it is not guaranteed to occur in every baseline history. The
ledger should update inventory, component-average energy-integrated flux
exposure, and cumulative energy under the stated assumptions. These conditional
service triggers are not material allowables. Run the authored sensitivity grid
and inspect its changed inputs; it is a deterministic design-space probe, not a
probability distribution. Compare both arrangements with the same assumptions
and inspect whether precision actually supports a difference. A point ranking
without supported uncertainty is not a conclusion.

## 6. Verify and package

Run focused Rust tests and independent controls for the current source revision.
The scripts in `controls/` are separately implemented software/numerical checks;
read their scope statements before treating a passing control as evidence. The
acceptance matrix in [DEMO_ACCEPTANCE.md](DEMO_ACCEPTANCE.md) identifies what
remains open.

Before packaging, generate the history refinement report that anchors the
outage-duration study. It is produced from the same four run records and the same
`faris` binary that go into the package; the packager requires its multiplier-1.0
histories to match the report's 600 s histories byte for byte. The history outputs
do not embed file paths, but they depend on the run records, the scenario and
assumption values, and the binary:

```bash
python3 scripts/make_history_refinement_report.py \
  --faris target/release/faris \
  --control-scenario scenarios/arc-inspired/cold-coupled-control.scenario.json \
  --port-scenario scenarios/arc-inspired/cold-reference-port.scenario.json \
  --control-reference-run runs/<control-reference>/run.json \
  --control-breeder-run runs/<control-breeder>/run.json \
  --port-reference-run runs/<port-reference>/run.json \
  --port-breeder-run runs/<port-breeder>/run.json \
  --assumptions scenarios/arc-inspired/demo-operating-assumptions.json \
  --event-assumptions scenarios/arc-inspired/demo-event-assumptions.json \
  --work-dir <new scratch directory> \
  --output references/operating-history-primary-refinement-v4.json
```

It refuses an existing work directory or output, and writes nothing if any
independent check or refinement gate fails. The v3 report remains in
`references/` as a superseded historical record.

The four corrected runs and both independent port-volume reports now exist.
Use their exact identities to create the portable four-run hash-indexed package. For each port
run, the packager also requires the worker's OpenMC geometry-ownership audit to
pass all plasma, clearance, and component probes, and checks that every sampled
port intersection is confirmed void in the final geometry:

```bash
python3 scripts/package_recorded_demo.py \
  --faris target/release/faris \
  --faris-app target/release/faris-app \
  --core /tmp/faris-demo-avila-core \
  --core-source-repo ../project-north-star-blanket \
  --core-source-revision 2f8f838c081ae375f2ff3d542e986d0f4c104f96 \
  --control-scenario scenarios/arc-inspired/cold-coupled-control.scenario.json \
  --control-reference-run runs/<control-reference>/run.json \
  --control-breeder-run runs/<control-breeder>/run.json \
  --port-scenario scenarios/arc-inspired/cold-reference-port.scenario.json \
  --port-reference-run runs/<port-reference>/run.json \
  --port-breeder-run runs/<port-breeder>/run.json \
  --port-reference-volume-report runs/<port-reference>/volume-check.json \
  --port-breeder-volume-report runs/<port-breeder>/volume-check.json \
  --assumptions scenarios/arc-inspired/demo-operating-assumptions.json \
  --event-assumptions scenarios/arc-inspired/demo-event-assumptions.json \
  --sensitivity-grid scenarios/arc-inspired/demo-operating-sensitivity.json \
  --support-report history-refinement=references/operating-history-primary-refinement-v4.json \
  --sweep-bundle runs/allocation-sweep/bundles/blanket-030cm.transport-bundle.json \
  --sweep-bundle runs/allocation-sweep/bundles/blanket-035cm.transport-bundle.json \
  --sweep-bundle runs/allocation-sweep/bundles/blanket-040cm.transport-bundle.json \
  --sweep-bundle runs/allocation-sweep/bundles/blanket-045cm.transport-bundle.json \
  --sweep-bundle runs/allocation-sweep/bundles/blanket-050cm.transport-bundle.json \
  --sweep-bundle runs/allocation-sweep/bundles/blanket-055cm.transport-bundle.json \
  --sweep-bundle runs/allocation-sweep/bundles/blanket-060cm.transport-bundle.json \
  --version 0.1.1 \
  --output dist/FARIS-demo-2026-10-01
```

Each `--sweep-bundle` is a portable recorded bundle made by `faris transport pack`
for one variant of `scenarios/arc-inspired/allocation-sweep/scenario.json`. The
packager validates every one with the recorded-bundle contract (embedded scenario
hash, `blanket-NNNcm` variant id declared by that scenario, distinct transport
seeds, no duplicate variant), copies them under `sweep/bundles/`, indexes them,
and adds a sweep table to the package README. The app opens the verified
bundles at launch; `verify.sh` revalidates them and its
tamper control targets a sweep file when one exists. Omitting the flag is allowed
and yields a package with no sweep (the packager prints a note). The sweep runs
carry transport identity only; they have no Core evidence cases.

The packager checks all four scenario/run identities and normalized heating/H3
responses, requires port geometry reports to bind to each raw artifact, archives
the exact worker ownership and void-confirmation audits with their input/run/
artifact hashes, generates
the complete study selection, prepares each Core case, runs the real Core
workflow, invokes Core's verified `export` for each case, and records its receipts/reports and the four portable
`RecordedTransportBundle` JSON files. It also reopens each prepared case with
`faris evidence inspect` and stores the identity-revalidation report beside the
package. It writes a SHA-256 index and refuses an
existing output directory. It excludes statepoints, HDF5, ENDF, ZIPs, and
nuclear-data files; recipients need compatible external data for fresh runs.
Recorded results remain inspectable offline, but identity checks and Core
workflow completion are not scientific qualification.
The package copies a bounded allowlist of scientific background, acquisition
instructions, independent controls, and support metadata. Add final campaign
verification summaries with repeatable `--support-report LABEL=JSON_PATH` only
after those reports bind to the fresh run identities; absolute workstation paths
are redacted in packaged support JSON. `DEMO_ACCEPTANCE.md` is not copied as a
snapshot because the final package and its verifier define release acceptance.

The package retains each recorded bundle's raw and normalized energy spectra,
worker receipt, and all normalized mesh-bin fields, with hashes checked against
the run record. It also produces two paired history comparisons, four event-
control histories, and four 27-point sensitivity records from the selected
release CLI and exact fresh run records. Their provenance binds the scenario,
run record, raw tally artifact, assumptions, and grid. The index reports total
file count and bytes and the verifier enforces the 1 GiB / 2,048-file delivered
package caps. Case/workspace trees are held in one evidence store
(`evidence-store/`, Avila Core format `avila.core/evidence-store/v0.1`): a
`store.json` that lists each tree's files with exact lengths and hashes, and
one xz-compressed blob per distinct file content under `blobs/`. The package
index records the store's `store.json` hash, blob count and per-tree totals,
and indexes `store.json` and every blob, so the delivered-file caps apply to
them. Each tree keeps a 1.5 GiB / 8,192-file aggregate budget and a 64 MiB
per-file ceiling. These are delivery disk budgets, separate from each native
case's 512 MiB bound and the fresh solver job limits above. The app verifies
the index and reads the checked files in place from the store, with length and
SHA-256 checked on every read; nothing is expanded and no temporary directory
is used. The validated transport scene opens while saved Core evidence is read
in the background, and evidence becomes available only after actual receipt
inspection. Path depth and implicit-directory counts are bounded. `verify.sh`
unpacks one case tree at a time because `avila-core export` needs a real
folder; its free-space check covers the largest tree, its export copy,
directory blocks and 64 MiB. Opening a legacy `.faris` file with referenced
archives still extracts them to a private temporary directory.
These deterministic history
probes are authored scenario studies, not
probability distributions or lifetime uncertainty bounds.

The separate outage-duration study contains twelve additional full Rust
histories: four fixed transport drivers at annual outage durations of 15, 30
and 60 days. Its one-factor changes preserve outage start dates and every other
baseline input. Assumptions, driving rates, full histories and provenance are
retained for independent replay; the 30-day level reproduces the baseline.

The local Linux distribution includes hash-pinned, read-only copies of the
release FARIS CLI, native app, and Core executable. Launch the offline four-case
workspace with `dist/FARIS-demo-2026-10-01/bin/faris-app` (or double-click it); run
`dist/FARIS-demo-2026-10-01/verify.sh` to rehash the package, reopen all four Core
cases, and exercise a tamper-negative copy. These binaries target the recorded
OS and architecture and may require compatible system libraries. Their hashes
check byte identity only; they are unsigned and do not establish authenticity.
The package copies FARIS's AGPL license and Core's license/third-party notices
from the declared source revisions, and records the repository commits plus
the build commands in `SOURCE_PROVENANCE.md`.

Launch and verification preserve the indexed distribution. Generated study and
fresh-run records default to `$XDG_STATE_HOME/faris/recorded-demo-runs/<package-name>`
(or `~/.local/state/faris/recorded-demo-runs/<package-name>`) on Linux,
`~/Library/Application Support/FARIS/recorded-demo-runs/<package-name>` on macOS
and `%LOCALAPPDATA%\FARIS\recorded-demo-runs\<package-name>` on Windows. Pass
`--runs-directory /path/outside/the/distribution` to choose another output root.
The app refuses a run directory inside the distribution.

Opening the package needs no temporary space for the Core evidence. The
evidence is read in place from the store.

```bash
dist/FARIS-demo-2026-10-01/bin/faris-app --package dist/FARIS-demo-2026-10-01 \
  --runs-directory "$PWD/runs/demo-native"
```

For the native **Run transport** action, supply the documented Python/OpenMC
executables, nuclear-data audit and cross-sections XML. When loading recorded
bundles, reuse their bound physics inputs; omit `--physics` and
`--control-physics` overrides unless they match those identities exactly.
The identity guard rejects mismatched inputs before calculation. Missing tools,
genuine OpenMC cancellation, stale-history recovery, and numeric editing were
checked on the delivered binary. Those bounded checks use egui's native input
hook; OS input injection and a fresh run on a second installation were not
verified.

Open the port arrangement and its feature-free control from that package with:

```bash
dist/FARIS-demo-2026-10-01/bin/faris-app --package dist/FARIS-demo-2026-10-01
```

Each saved-study descriptor identifies one exact prepared Core case, its
execution report, and its completed receipt workspace. Reopening rehashes the
case package and stage files and shows the original scenario/variant and Core
states. It does not attach saved results to a newly selected scenario; unsigned
digests establish identity consistency only, not record authenticity or physical
qualification.

After copying the package to another directory or machine, rehash its index and
reopen all four saved Core cases against the same pinned FARIS and Core
executables. This command creates an unchanged relocated copy, runs each
`faris evidence inspect`, then makes a separate temporary tampered copy and
checks that its modified indexed file is rejected. The original package and
the relocated copy remain untouched by the negative control:

```bash
python3 scripts/verify_recorded_demo.py \
  --package dist/FARIS-demo-2026-10-01 \
  --relocated-copy /tmp/faris-recorded-study-relocated \
  --faris target/release/faris \
  --core /path/to/avila-core
```

The verifier reports `EXPECTED_REJECTION` for the deliberately changed copy;
it does not treat matching hashes as authenticity or scientific qualification.

## README media

The README's animated GIF and screenshots come from the app's own frames, not a
screen recorder. `--record-frames` (hidden, development only) saves the window as
`frame-00000.png`, ... into an existing empty folder at `--record-fps` (default 15,
measured on frame time) and stops when the interface-check plan ends, or after
`--record-seconds` (default 20) with no plan. At most 600 frames are saved. PNGs
are encoded on a worker thread; frames dropped because its queue was full are
counted on stderr at exit, and `frames.json` records each frame's capture time.
A release build records closest to the requested rate; a debug build works but
runs well under 15 frames per second, and the assembler then resamples by time.

The plan `references/readme-capture/tour.plan.json` presses steps 2, 3 and 4,
orbits the camera, and moves the year cursor 0 to 30, without any pixel positions
(plan actions `tap`, `orbit`, `set_year`; `time_s` sets pauses). Start the app on a
recorded study or a package (`--package`) so Simulate and Operate show
real fields, for example:

```bash
mkdir -p /tmp/faris-frames
target/release/faris-app STUDY.faris \
  --window-width 1440 --window-height 900 \
  --interface-check references/readme-capture/tour.plan.json \
  --interface-check-output /tmp/faris-tour-report.json \
  --record-frames /tmp/faris-frames
python3 scripts/make_readme_gif.py /tmp/faris-frames docs/images/faris-tour.gif \
  --width 960 --fps 12 --max-bytes 8000000
```

Without a study argument the app opens the geometry demo on Design. The
assembler builds one global palette (no colour flicker), loops forever, and steps
the width down 960, 800, 720, 640 if the file exceeds `--max-bytes`. Single
screenshots use `--capture docs/images/<name>.png` as before (add `--step`,
`--field-view`, `--initial-year`). Run `python3 -m unittest discover -s scripts
-p 'test_*.py'` for the assembler's tests.

## Release download

A release is the verified package in two archives: the app part, one for each
platform, and the evidence part, the same for every platform. Set the workspace
version in `Cargo.toml` and date the version's section in `CHANGELOG.md` (the
script refuses an "unreleased" heading), rebuild the package with binaries of
that version (`package_recorded_demo.py --version 0.1.1`), run its `verify.sh`,
then:

```bash
python3 scripts/make_release.py --package dist/<package> --version 0.1.1 \
  --output-dir dist/release-0.1.1
```

The script refuses a version mismatch (Cargo.toml, changelog, the bundled
`faris` and `faris-app`), an undated changelog section, symbolic links and an
existing output directory, and reruns the package's binary manifest check. It
writes `FARIS-<version>-linux-x86_64.tar.gz` and
`FARIS-<version>-evidence.tar.gz` with sorted entries, zeroed owners and the
release commit time as every timestamp, so the same package gives the same
bytes; `SHA256SUMS` for both; and `RELEASE_NOTES.md` from the changelog section.
It does not tag or upload.

The Windows and macOS programs are built by CI (`.github/workflows/desktop.yml`),
which uploads each platform's `bin/` and `build.json`. Turn the verified Linux
package into that platform's package with `scripts/retarget_package.py`:

```bash
python3 scripts/retarget_package.py --package dist/<linux-package> \
  --build <desktop-build-folder> --output dist/<platform-package>
```

It refuses unless the build's FARIS and Core commits and `faris --version` equal
what the Linux package recorded and every program has its recorded hash and size.
It executes nothing and refuses an existing output directory. Then write that
platform's app archive (`.zip` for Windows, `.tar.gz` for macOS):

```bash
python3 scripts/make_release.py --package dist/<platform-package> --version 0.1.1 \
  --output-dir dist/release-0.1.1 --app-only
```

With `--app-only`, `--output-dir` must already hold the `SHA256SUMS` of the first
run. The script writes only the app archive, appends its line to `SHA256SUMS` and
rewrites `RELEASE_NOTES.md` for every platform archive present. The evidence
archive is the same whichever package it is built from.
