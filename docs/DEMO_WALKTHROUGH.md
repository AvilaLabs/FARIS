# FARIS functional demo walkthrough

This walkthrough is for the identified local FARIS workspace. It uses a cold-data
numerical surrogate; it does not reproduce ARC or establish a qualified reactor
prediction. The corrected campaign contains four million-history coupled runs with exact
geometry/data identities. Earlier neutron-only and incorrect-clearance runs
remain superseded diagnostics, and must not supply final reactor conclusions.

The offline distribution is being finalized at
`dist/FARIS-demo-2026-10-01`. Once its verification receipt is recorded, the
short native demonstration is:

1. Run the distribution's `verify.sh`, then `launch.sh`. No OpenMC installation
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
   The 27 full-history reruns probe authored assumptions; they are not lifetime
   confidence bounds. The result can remain an unresolved engineering tradeoff.

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
  --output dist/FARIS-demo-2026-10-01
```

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
file count and bytes and the verifier enforces the 512 MiB / 2,048-file package
caps. These deterministic history probes are authored scenario studies, not
probability distributions or lifetime uncertainty bounds.

The local Linux distribution includes hash-pinned, read-only copies of the
release FARIS CLI, native app, and Core executable. Launch the offline four-case
workspace with `dist/FARIS-demo-2026-10-01/launch.sh`; run
`dist/FARIS-demo-2026-10-01/verify.sh` to rehash the package, reopen all four Core
cases, and exercise a tamper-negative copy. These binaries target the recorded
OS and architecture and may require compatible system libraries. Their hashes
check byte identity only; they are unsigned and do not establish authenticity.
The package copies FARIS's AGPL license and Core's license/third-party notices
from the declared source revisions, and records the repository commits plus
the build commands in `SOURCE_PROVENANCE.md`.

Open the port arrangement and its feature-free control from that package with:

```bash
dist/FARIS-demo-2026-10-01/launch.sh
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
