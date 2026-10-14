# Changelog

FARIS follows [semantic versioning](https://semver.org/) from 0.1.0. Before
1.0 a minor version may change file formats or the command line; such changes
are listed here under **Changed** with what to do.

Research screening only. FARIS results are not a licensing, safety or design
basis.

## Unreleased

### Added

- Optional `plasma.min_first_wall_clearance_m` in the design file (metres, positive; `null` or absent for no
  minimum; `faris design init` writes `null`). Check 9 fails when the nearest source site is closer to the first
  wall than that, and says so as NOT_EVALUATED if no solid has the role `first_wall`. FARIS sets no minimum itself.
- Validation status beside every calculated number (VAL-054): the transport report, the receipt inspection, the
  history and comparison results and the study export read the V&V matrix and tag each number as validated within
  range, validated but out of range, code-to-code only or not validated, with the why and next step once per
  quantity (`data/validation.csv` in an export). Today every quantity is not validated. The design transport
  report is now `faris-design-transport-report/v0.4`; older reports still read. The desktop panels do not show it yet.
- Replacement events store the criterion, the value reached, the limit and its unit, and the earliest and latest
  time of the trip at two standard errors of the driving flux (first order; the reason when the flux has no
  error). The brief and the sweep show the swap count and first swap with the same band and call a difference
  resolved only when the bands separate. Every history, brief and sweep number carries an interval or the reason
  it has none; the export adds `data/replacements.csv` and `data/uncertainty.csv` and appends uncertainty
  columns to the existing tables. New fields are optional, so older files still read.
- Literature maintenance-duration presets (EU-DEMO blanket 10 months, best case 6 months, internal components 20
  months; Crofts and Harman) with their source link and conditions, each labelled literature. A condition the
  sources FARIS holds do not state is left empty with the reason.
- A remote-handling model: outage length against the number of parallel systems, ports and crews, within 10 % of
  the published five-point table (1 to 8 systems), and a tornado of lifetime net electricity and availability
  against each replacement duration. Both are engine functions; no desktop view yet.
- `faris design check` reports the D-T fusion power the plasma profiles imply next to `plasma.fusion_power_mw`
  (`Source closure`, WITHIN or EXCEEDED the 1e-3 limit) and the smallest distance from the sampled source sites
  to the first wall. A mismatch beyond the limit is a blocking finding of check 9 (SRC-010) with both powers and the
  ratio, which a waiver with a written reason accepts (`SRC-010`, naming the plasma chamber solid); neither value
  is rescaled. A sampling test compares the real source sites with the analytic profile by chi-square.
- `faris design transport`: the fast-neutron flux (above 0.1 MeV) in the TF magnets of a design that passed
  `faris design check`, computed on its CAD geometry and run until the mean over the coils reaches a target
  error. A run stopped by its history or time limit says how many more histories it needs.
  - With `--window-python` (OpenMC 0.16 or later) weight windows for the magnets are made first, and on an
    OpenMC built with the aiming patch the run also aims at a streaming channel it finds. The report names the
    mode that ran and why.
  - The flux is compared with the design's fluence limit over the linked operating history: years to the
    limit, and a status of WITHIN, EXCEEDED or INCONCLUSIVE. The limit's `averaging` decides which flux: the mean
    over the group for `group_average`, the highest piece of any solid for `peak_mesh`. A piece is a ring of the
    limit's dr by dz about the machine axis, cut by the solid; the volume of every piece is measured on the CAD
    geometry. With a peak, the run continues until the highest piece is within twice the target error (10 % by default).
  - The solids and the energy threshold come from the design's fluence limit (`--group` when several groups
    have one), so the response is not fixed to the TF magnets above 0.1 MeV.
  - Neutrons the geometry cannot place are counted in every run. A run that loses more than one in a million
    stops and says so, judges no limit, and reports where the first ten were lost.
  - Weight windows and the channel search are kept between runs on the same model (`--cache-dir`, `--no-cache`),
    and progress is printed while the run samples.
  - A design that gives an emissivity table or a fuel other than D-T is refused; it is never run with the
    tokamak D-T source in its place.
- The desktop has a **Design check** window that takes a STEP file to a checked design without a terminal: it writes the
  draft design file (`faris design init`) and says which fields are still empty, and it runs the nine import checks
  (`faris design check`) with the stage line, elapsed time and Cancel. It shows each check's status word, every finding's
  item, why and next step, and the measured records with their units. When all nine pass, a button opens the Design flux
  window with the design file and report filled in. The design file itself is still edited outside the app. The Design flux
  window now says each missing input once, beside its field.
- The desktop has a **Design flux** window that runs `faris design transport` without a terminal: it names the tools
  the run needs and which are missing, shows the step, the errors against the target and a labelled estimate of the
  time left while it runs, offers Cancel, and shows the result with every number's error and every limit's why and next step.
- A second local nuclear-data library, built by `scripts/build_nuclear_data_v2.py`: all FENDL-3.2 neutron
  nuclides, ENDF/B-VII.1 photon data for every element in them, and ENDF/B-VIII.1 thermal-scattering tables for
  fusion materials, with a provenance record and an audit of the new `fusion-materials-v1` nuclide set
  (`audit_library.py --nuclide-set`). The first library and the default paths are unchanged. See
  [Nuclear-data library v2](docs/NUCLEAR_DATA_LIBRARY_V2.md).
- `faris design check` now runs import checks 6 to 9. Check 6 converts the STEP model to a DAGMC model as a
  bounded job and records the tolerances, versions, time, peak memory and per-volume triangle counts; it also looks
  for solids that overlap in the CAD model before imprinting. Check 7 matches every converted volume to exactly one
  solid and reports the faceting deviation. Check 8 is FARIS's own integrity check of the faceted model, in
  exact arithmetic: open and non-manifold edges, inconsistent orientation, inverted normals, zero-volume cells,
  slivers, self-intersections, duplicate surfaces, overlaps, gaps and lost particles, each finding with the solids,
  a position, why it stops the import and what to change in the CAD tool. Check 9 samples 100,000 source sites from
  the plasma block and tests that all lie in the plasma chamber. New options: `--work-dir`, `--no-reuse`,
  `--threads`, `--seed`, `--samples`, `--chords`, `--source-sites`, `--deviation-points`, `--imprint-threads`,
  `--plasma-source-path`.
  The report schema is now `faris-design-import-report/v0.2`.
- An optional `waivers` array in the design file lets an author accept a named integrity finding; the finding stays
  in the report and receipts. Open or non-manifold surfaces, inverted normals, zero-volume cells and degenerate
  triangles cannot be waived. See [The design file](docs/guide/design-file.md).
- A corpus of 22 seeded-defect and 24 clean models (`crates/faris-engine/tests/fixtures/corpus`, generated by
  `integrations/cad/faris_corpus.py`) tests the checks.
- Two sliver findings, both waivable (`SLIVER`) and both reporting the mean thickness 2V/A: a solid thinner than the
  faceting tolerance, and a solid whose faceting deviation (GEO-032) is above a tenth of its thickness. A thin layer
  that is faceted accurately is no longer a sliver; the earlier rule (2V/A below ten faceting tolerances) is gone.
- The faceting deviation (GEO-032) measures every sample point on plane, cylinder, cone, sphere and torus faces in
  closed form, and projects the points on other faces with OpenCASCADE on an even stride of at most 2000 per solid
  (`--deviation-points`, default now 2000; it was 0, all points). The report says per solid whether the result is a
  maximum over all points or a sampled maximum.
- The pre-imprint tessellation uses OpenCASCADE's mesher directly, one solid at a time, and reports a solid whose mesh
  is not closed as a tessellation problem. Tessellations and overlap booleans are cached in `--work-dir`
  (keyed by the STEP hash, tolerances and helper version; `--no-reuse` turns this off). `--imprint-threads` now
  defaults to 4.
- The files FARIS saves for you (study files, design drafts, check, transport and evidence reports, exports) are
  written to a temporary file in the same folder, flushed to disk, renamed into place and the folder flushed, so a
  crash or power cut leaves the previous file or the complete new one. A killed save can leave a `.faris-save-*.tmp`
  file beside it; delete it. Tests kill the saving process at random points (300 trials in the normal test run,
  10,000 with `--ignored`).
- Memory is budgeted. Every external job runs under a memory cap (default 4 GiB; `FARIS_MEMORY_CAP_MB` lowers it
  for all jobs). Before a job starts FARIS compares the cap with the memory free now, less a reserve
  (`FARIS_MEMORY_MARGIN_MB`, default 1024), and refuses the job with the numbers and what to do if it would not fit.
  Where the system gives free memory (Linux), a finished job's result records its peak memory and the check made.

### Changed

- The reference design RM-M (`scenarios/rm-m/rm-m.faris-design.json`) now closes: its ion densities (centre,
  pedestal, separatrix) were multiplied by 1.65989, so its profiles imply the published 525 MW instead of 190.5 MW.
  The source shape is unchanged; the density is a normalisation, not a published plasma density. The file's sha256
  changed, so a design check or run recorded against the old file names a different file. The test-set builder
  does the same per model, and the fixture designs of the tests were scaled the same way.
- `faris design check` judges every solid's faceted volume against its CAD volume and lists each one as WITHIN or
  EXCEEDED in the report, with the faceting tolerance it was converted at and whether that was the default or the
  design file's `cad.faceting_tolerance_m`. At the default tolerance a solid more than 0.1 % off now stops the check
  (it was 0.5 %); at a tolerance you set the limit stays 0.5 %. The message says which solid and to set a smaller
  tolerance. The seeded-defect corpus now has overlaps of 1e-4 and 1e-5 of the smaller solid, with the overlap volume
  OpenCASCADE measured for each.
- `faris design transport` refuses a check report whose conversion directory is not an absolute path without `..`
  or whose `model.h5m` is a link; the report is rewritten by `faris design check --work-dir DIR --report FILE`.
- The component ids sent to the activation input builder are passed as `--component=ID`, one token each.
- The OKTAVIAN aluminium validation case now has a confirmed normalisation: the
  experimental values are the leakage current through the whole outer surface
  (MCNP surface 6) per source neutron, per unit lethargy (neutrons) or per MeV
  (photons), with no area factor. The earlier note that the upstream tally was a
  flux (F2) was wrong; the manifest and `docs/VALIDATION.md` say so. The neutron
  bins other than the first are no longer blocked by the normalisation (photon
  bins and the first bin stay blocked). The OpenMC runner tallies the current on
  surface 6 itself instead of a 0.2 cm void shell, scores the net current and
  records a check against the outward partial current; no transport run has been made with it yet.
- FARIS now pins Avila Core at `faris-core-2026-10-09` (commit `d6e984a`,
  reported as `avila-core 0.1.0`); the desktop workflow builds that tag by
  default. `verify.sh` replays each Core export with `avila-core export
  --report-only` straight from the evidence store, so it no longer unpacks any
  case tree or needs temporary space for the Core exports: it needs room for a
  relocated copy of the package and one recomputed history. The copies are
  made of hard links in a hidden folder beside the package (measured: 0.1 MB
  of new data there and 18 MB of temporary space; it asks for about 100 MB
  free as a margin, and says how many files it linked or copied); if links
  are impossible it copies the files instead and needs about 260 MB more. FARIS passes `--directory` to `avila-core run`, because Core
  now keeps run results in an evidence store by default and FARIS packs the
  workspace folder itself. Nothing changes in the package or `.faris` formats.

- The package's Core evidence is one content-addressed evidence store
  (`evidence-store/`, Avila Core format `avila.core/evidence-store/v0.1`)
  instead of eight `.tar.gz` archives. Each distinct file is kept once,
  xz-compressed, under its SHA-256. The package index is
  `faris-recorded-demo-package/v0.7` and the saved-study descriptors are
  `faris-saved-study-store/v0.1`. The app reads and inspects the saved Core
  evidence in place with length and SHA-256 checked on every read: no
  temporary expansion, no 0.9 GB of temporary space and no wait on opening.
  `verify.sh` unpacks one case tree at a time and needs far less temporary
  space. New: `faris evidence inspect --store DIR --case-tree NAME
  --workspace-tree NAME` and `faris evidence verify-store DIR`.
  Older `.faris` files and packages still open.

- `.faris` files can carry Core evidence as trees of the evidence store
  (study file format 1.1, optional `layers.evidence_store`). A packed file
  embeds a valid evidence store holding exactly the recorded trees, copied
  without recompression; opening extracts it into the run workspace, verifies
  it and inspects the saved cases in place. A referenced file records each
  tree's full file list and finds the trees in an `evidence-store` folder
  beside the file, then in the store of the package the app was started from,
  accepting a tree only when the store lists exactly the recorded files;
  otherwise the Evidence step names the tree, where it looked and what to do.
  `faris study-file create --evidence` accepts a `faris-saved-study-store/v0.1`
  descriptor (with `--pack-evidence` to embed), `inspect` and `unpack` report
  the store layer, and a session started from a package can now save its Core
  evidence. A file opened with its evidence packed keeps it on the next save.
  Older FARIS versions open a referenced file with the evidence marked as not
  included and refuse a packed one; files with archives are unchanged.

- Core stage outputs are now `faris-core-stage-output/v0.2`. Each stage writes
  only what it newly computes: the transport output no longer carries the whole
  recorded transport bundle, and the normalize output no longer carries the
  bundle or the normalized result again. The normalize stage takes the
  transport output and the scenario (no physics input) and writes the claims
  plus the driving rates; the history stage reads those rates. Core already
  binds each stage input by SHA-256 to the upstream step's receipt-verified
  output, so nothing is re-verified from scratch. Claim values (`tbr`,
  `magnet_flux`) and the history result are identical to those of v0.1 for the
  same recorded run. Cases and `.faris` files saved with v0.1 outputs still
  open and inspect; newly prepared cases are much smaller. The categorical
  normalize claim now takes the value `normalized_transport_extracted`.
  Re-run `faris evidence prepare` to move an old case to v0.2.
  The FARIS stage capabilities are now major version 2 (adapters `@2`) and
  the generated registry is revision 2, because their inputs and outputs
  changed. A study generated by an earlier version no longer matches; generate
  it again with `faris study generate` (the desktop always regenerates it).
  For the recorded port arrangement, the case shrinks from 74 MB to 36 MB and
  its Core workspace from 132 MB to 55 MB.

- The sixteen recorded 30-year histories (four event-control histories and the
  twelve outage-duration probes, about 18 MB each) are no longer in the
  evidence pack. Each record keeps the assumptions, the exact transport rates
  and the history's SHA-256 and size, and `verify.sh` recomputes every history
  with the package's own `faris history run` and requires the same digest. The
  evidence pack shrinks from 35.8 MB to 9.6 MB. The package index is
  `faris-recorded-demo-package/v0.7`; the event-history and outage-duration
  provenance and the outage-duration summary are `v0.2`. They record
  `history_bytes` and no longer name a `history_path`. A v0.6 package no longer
  opens in this version; build it again with `package_recorded_demo.py`.
- FARIS's model and engine compute with platform-independent math: every
  `exp`, `ln`, `powf`, trigonometric and similar function is the pure-Rust
  `libm` one (`faris_model::math`), and `powi` is repeated multiplication in a
  fixed order, so a history is bit-identical on Linux, Windows and macOS (Intel
  and Apple silicon). `desktop.yml` checks this on all four with a SHA-256 test
  of a 30-year history and an ensemble, and Clippy rejects the std methods.
  Results recorded earlier are unchanged: all sixteen recorded histories
  recompute byte for byte.

### Added

- The design file `faris-design/v0.1` and import checks 1 to 5 for a STEP model.
  `faris design init MODEL.step` (and `faris-app --design-init MODEL.step`) writes
  a draft that lists every solid with its fingerprint and leaves every material,
  role and plasma value null. `faris design check DESIGN.json` runs the checks
  (schema and nulls, STEP hash, unit and extent, solid count and fingerprints,
  ids and references, roles, materials and the nuclear-data audit) and writes a
  JSON report. A separate CAD process reads the STEP file; it needs a Python with
  cadquery, which FARIS does not install. Conversion and the later checks 6 to 9
  are reported as not evaluated. See [The design file](docs/guide/design-file.md).

### Fixed

- Large machines no longer fail writing their weight windows: `faris design transport` died with "File too large"
  on a machine 1.8 times the reference size, because the windows' bounds (475,695 voxels, 17 groups) were written
  into the model file as text and passed the job runner's 256 MiB file limit. The run now hands OpenMC the windows as
  an HDF5 file (the model file stays under a megabyte), and the aiming step caps the bounds in that file. Results
  are unchanged to the last digit for a given seed. Kept window caches from before are not reused and are made again.

## 0.2.0 — 2026-10-08

Replacement outages computed from activation. FARIS can now work out how long
each replacement takes from the decay heat of the parts around the replaced
component, rerun the 30-year history with those outages, and compare every
design's downtime, availability and lifetime electricity with the fixed-duration
model. The demo study and its recorded results are otherwise the same as 0.1.1.

### Added

- `faris maintenance run` computes replacement outages for every design in a
  designs file (`faris-maintenance-designs/v0.1`) from a maintenance
  assumptions file (`faris-maintenance-assumptions/v0.1`): per replacement
  class, the governing components, the work time and a threshold q*, either
  calibrated on one design's first replacement or given in W/m³. Each outage is
  the cooldown until the governing components' decay heat per volume falls
  below q*, plus the work time; the history is rerun until no outage changes by
  more than a day. It needs ACTINV, its data and Python, and writes
  `faris-maintenance-result/v0.1`. `faris maintenance report` prints a result as
  tables.
- A **Maintenance** window in the desktop (top bar). It opens a result and
  shows each design's downtime, availability and lifetime net electricity under
  both models, a chart of every replacement's fixed and computed outage, and
  for each replacement which components held the cooldown up and how long they
  had been in the machine. **Run…** computes a result from the desktop with
  progress and cancel. `--maintenance FILE` opens a result at launch.
- The package carries a recorded maintenance result for the four arrangements,
  opened from the Maintenance window, and the activation-input builder
  (`tools/build_activation_inputs.py`) for computing results from your own
  transport runs.

### Limits

- Decay heat governs the cooldown. Contact dose is refused as the governing
  quantity: with a threshold fixed at the first outage, the never-replaced first
  wall kept dose above it for more than a year at later replacements.
- With a calibrated threshold, outages are relative to the calibration outage:
  FARIS shows how they differ between designs and grow over the plant's life,
  not how long they are in absolute terms.
- Running the calculation needs your own transport runs with a 709-group
  spectrum; the packaged arrangements' spectrum runs are not in the download.

## 0.1.1 — 2026-10-06

The same demo study and results as 0.1.0. What changes is how it is
distributed: a smaller download split in two, a launcher with no Python, and
Windows and macOS downloads.

### Changed

- The download is split in two. The app download, one per platform
  (`FARIS-0.1.1-linux-x86_64.tar.gz`, `FARIS-0.1.1-windows-x86_64.zip`,
  `FARIS-0.1.1-macos-aarch64.tar.gz` for Apple silicon and
  `FARIS-0.1.1-macos-x86_64.tar.gz` for Intel Macs), holds everything the app
  needs to open the whole study. The Linux app download is about 35 MB. The
  optional `FARIS-0.1.1-evidence.tar.gz`, about 78 MB and the same for every
  platform, adds the Avila Core receipts and is what `verify.sh` checks. 0.1.0
  was one 112 MB archive. `SHA256SUMS` covers every file. Each archive unpacks
  to a folder `FARIS-0.1.1/`; unpack the evidence pack into the same place so
  its files merge.
- Python is no longer needed to open the study, and `launch.sh` is gone. Double-click
  `bin/faris-app` (`bin\faris-app.exe` on Windows), or run it from a terminal.
  It finds the study beside its `bin` folder, checks the SHA-256 of every file
  it needs and opens the four recorded arrangements and the sweep.
  `faris-app --package DIR` opens a package elsewhere. If a check fails, the app
  opens with nothing loaded and says why and what to do (download again and
  check `SHA256SUMS`).
- Windows and macOS downloads. The programs are not signed. Windows SmartScreen
  may warn: choose More info, then Run anyway. On macOS the programs are neither
  signed nor notarized: after unpacking, run
  `xattr -dr com.apple.quarantine FARIS-0.1.1` once in Terminal, or open
  `bin/faris-app` and allow it under System Settings, Privacy & Security, Open
  Anyway. Windows and macOS builds are new in 0.1.1; report problems on GitHub
  issues. Every platform's programs are built by CI from the release commit;
  the study evidence was recorded on Linux on the reference laptop. The Linux
  programs need glibc 2.35 or newer: Ubuntu 22.04 or later, Debian 12 or later.
- With the evidence pack, the app unpacks the Core evidence (823 MB) into a
  private temporary folder while it runs and deletes it on exit, so it needs
  about 0.9 GB of free temporary space. Without it, everything works except
  the saved receipts: the Evidence step says "Core receipts not included", why,
  and the next step.
- `verify.sh` needs Linux, `python3` and the evidence pack, and about 2 GB of
  temporary space; it checks the exact amount first. The Windows and macOS
  downloads are checked by the app itself at launch.
- Generated runs go to a per-user folder outside the package:
  `$XDG_STATE_HOME/faris/recorded-demo-runs/<folder>` on Linux (`~/.local/state`
  if unset), `~/Library/Application Support/FARIS/recorded-demo-runs/<folder>`
  on macOS and `%LOCALAPPDATA%\FARIS\recorded-demo-runs\<folder>` on Windows.
  `--runs-directory` overrides it and must be outside the package. The tour
  marker `tour-completed` is in `$XDG_CONFIG_HOME/faris` or `~/.config/faris` on
  Linux, `~/Library/Application Support/FARIS` on macOS and `%APPDATA%\FARIS`
  on Windows.

**What to do.** Nothing, if you stay on 0.1.0: a 0.1.0 package keeps its own
`launch.sh` and verifier and still works as before. To use 0.1.1, download the
app archive for your platform and, if you want the Core receipts, the evidence
pack. A `.faris` file made with 0.1.0 opens in 0.1.1 unchanged.

### Added

These are for development and for the maintenance study that follows 0.1.1.
They change nothing in the recorded study.

- `faris reactor run --activation-spectra fispact-709` also tallies each
  component's neutron spectrum in the 709 groups ACTINV uses. Without the
  option, requests, tallies and recorded results are unchanged, and recorded
  0.1 runs still inspect and load.
- Operating assumptions accept `replacement_durations_s` on a service limit:
  the k-th replacement of that component takes the k-th duration, and later
  ones fall back to `replacement_duration_s`.
- `faris-app --check-package REPORT.json` opens a package without a window,
  reopens its saved studies, loads its inputs and writes a JSON report. The
  release checks use it on each platform.

### Fixed

- The 0.1.0 Linux programs needed glibc 2.43, so they ran only on the newest
  distributions. 0.1.1's are built on Ubuntu 22.04 and need 2.35 or newer.
- The tour states the real history counts: 10 million per port case and 30
  million per port-free control.
- The export button's hover no longer gives a page count.
- Saved Core evidence now waits up to 10 minutes for unpacking, and says why if
  it gives up.
- On Windows the app opens without a console window.
- On Windows the `faris history` commands no longer refuse to run.

## 0.1.0 — 2026-10-05

The first public release, a deliberately narrow demo that is complete for one
study: an ARC-inspired compact D-T tokamak, studied from 3D Monte Carlo
transport through 30 years of operation, with the transport sampling
uncertainty carried into every year. What comes next is in the README, "Where
FARIS is going".

### Study

- Five-step workspace: Design, Simulate, Operate, Compare, Evidence (keys 1 to 5),
  with a first-run tour that can be replayed.
- Four recorded arrangements: two blanket/shield allocations, each with a finite
  outboard service port and a matched port-free control. Coupled neutron/photon
  OpenMC fixed-source transport with FENDL-3.2 neutron and ENDF/B-VII.1 photon
  data.
- A seven-point blanket/shield allocation sweep.
- A 30-year operating history that recalculates in about a second from sliders:
  tritium inventory and losses, fuel-limited operation and restart, planned
  outages, exposure-triggered component replacements, and signed net
  electricity.

### Uncertainty

- Transport records the covariance between all scalar responses from per-batch
  results, so correlated quantities (tritium production and heating, say) move
  together when sampled.
- History ensembles: up to 2,000 histories on transport rates sampled from that
  covariance. Every output gets a median, a 90 % range with order-statistic
  confidence limits, and event probabilities. Ensembles fail closed: if more than
  1 % of draws are non-physical, the result is not evaluated and says why and
  what to do next.
- Paired comparison of two arrangements' ensembles from independent transport
  runs.

### Magnets

- Fast-neutron (above 0.1 MeV) flux on the magnet's inboard half, outboard
  half and the 20° sector behind the port, each with its own service limit.
  The first limit reached triggers the replacement and is named in the event.
- The default preset treats the magnet as demountable at a literature REBCO
  screening fluence of 3 × 10²² n/m² (Sorbom et al. 2015).

### Files and export

- `.faris` study files: one file holds both arrangements, the recorded transport,
  the sweep, the assumptions, stored ensembles and the view. Recorded bytes are
  checked by hash on every open; a damaged file is refused and the damaged part
  named.
- Export to a 3-page PDF brief, CSV tables, and SVG/PNG charts, stamped with the
  study file's SHA-256. The same export runs from the desktop and from
  `faris study-file export`.
- Every PDF page, CSV, chart and the export manifest carry the research-screening
  statement.

### Evidence

- Studies compile and run through Avila Core, which records hash-bound receipts;
  saved cases reopen only if every recorded byte still matches.

### Known limits

- Linux only. The binaries are not signed.
- Idealised geometry: concentric tori and one outboard port. Not the published
  ARC design.
- The outboard port is an open 0.30 m × 0.30 m duct with no shield plug, so it
  is a bounding streaming case. With the demountable-magnet preset the
  port-sector magnet limit is reached 0.65 years into operation and the magnets
  are replaced about 30 times in 30 years; without the port, at most once. A
  shield plug would reduce the streaming; FARIS has not modelled one.
- Recorded transport: port-free controls at 30 million histories, port and
  sweep cases at 10 million. Monte Carlo standard errors are below 0.04 % for
  tritium production and heating, and 5 % to 24 % for regional magnet fast
  flux (up to 30 % for the port-sector region of the port-free controls).
- Only transport sampling uncertainty is propagated. Nuclear-data, geometry,
  material and model uncertainty are not.
- Magnet fluence is a regional average, not a local peak. A true peak needs
  variance reduction aimed at the magnet (planned for 0.2).
- No activation, decay heat or shutdown dose yet (planned for 0.2).
- Service limits, outage durations and plant efficiencies are authored
  assumptions, not material allowables or measured values.
