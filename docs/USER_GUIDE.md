# FARIS user guide

This guide is for release 0.1.0 of the FARIS desktop app and command line. It assumes you know neutronics and plant engineering. It does not assume you know this tool.

## 1. What FARIS answers

FARIS asks one design question of an ARC-inspired compact D-T tokamak: inside a fixed radial build, what changes when you move thickness from the neutron shield to the breeding blanket? It answers with recorded OpenMC transport results and a calculated 30-year operating history. The history follows magnet and blanket exposure, replacement outages, tritium inventory and net electricity. It compares four arrangements: two blanket/shield splits, each with and without an outboard service port.

FARIS is a design model. It is not a digital twin.

> Research screening only. These results are not a licensing, safety or design basis.

This line sits in the bottom bar of every step. It is also in every export. Nothing in FARIS is a licence, qualification, safety or validation claim.

## 2. Getting started

### Open the recorded package

The Linux package holds four recorded transport cases, their Core evidence and an allocation sweep. It needs `python3` and does not need OpenMC or nuclear data. From the package folder, run:

```bash
./verify.sh
./launch.sh
```

`verify.sh` checks the package from a relocated copy and takes a while. `launch.sh` checks the hashes of the bundled programs and files, then opens the app with all four cases loaded. It unpacks the Core evidence into a private temporary folder that stays until the app closes. It needs free temporary space; the package `README.md` gives the amounts. The bundled programs are hash-pinned, not signed.

### Open a .faris file

A `.faris` file is one saved study. Open one in any of these ways:

- File > Open… (Ctrl+O).
- Drop the file on the window.
- Pass it on the command line: `faris-app demo.faris`.

The window title shows the file name, with a dot after it when the view differs from what was saved.

### The first-run tour

The first time the app starts without a study file, it plays a 12-stop tour. Next (or Right arrow, or Enter), Back (Left arrow) and Skip tour (Esc) move through it. Finishing or skipping writes a marker, so it does not repeat. The tour button in the top bar, labelled Tour, replays it. `--tour always` and `--tour never` override the default.

### Interface size

The Interface size menu in the top bar offers 100, 125, 150, 175 and 200 per cent. Ctrl/Cmd + and − adjust it; Ctrl/Cmd 0 resets it. 100 per cent follows your desktop's display scaling. To start larger, pass `--interface-scale 1.25` (allowed range 0.75 to 2).

## 3. The five steps

The top bar has one button per step, and the keys 1 to 5 switch between them. The left panel shows the controls of the current step. The right panel is the Outliner (tick boxes to show or hide components) and the Properties of the selected component. The centre is the 3D viewport. The panel at the bottom is the timeline, or the comparison on step 4.

In the viewport, drag to orbit, Shift-drag to pan, scroll to zoom and click to select. Cutaway is a display option only. It does not change the model or any volume. Frame scene resets the camera.

### 1 Design

Choose the arrangement. Port selects With outboard port or No port (matched control). Allocation lists the blanket/shield splits. A radial-build bar shows the layers. Plant inputs (major radius, radial build, fusion power) carry the badge authored. Sources and assumptions lists the cited literature and the authored assumptions. Look at the radial build first: it shows which layers change between allocations.

### 2 Simulate

The transport card shows, for the selected arrangement: tritium breeding (H3 per source neutron), magnet-region mean flux, total nuclear heating and the number of histories, each with its Monte Carlo standard error. The field-view menu in the viewport controls colours the 3D model: Materials, Mean component flux, Spatial neutron flux (with a Y slice slider), Nuclear heat deposition and Accumulated component fluence. Transport fields use the recorded source strength and stay fixed during outages. Accumulated fluence follows the timeline.

Run and review transport can run new OpenMC transport. It needs your own OpenMC and data (Transport configuration) and is described in [TRANSPORT.md](TRANSPORT.md) and [COLD_REFERENCE.md](COLD_REFERENCE.md). The recorded package does not need it.

### 3 Operate

This step edits the operating assumptions and recalculates the histories of every arrangement and, when present, of the sweep. Start with the preset menu, then move the sliders and watch the timeline.

**Presets.** Hovering a preset shows what it is and its key numbers.

| Preset | What it is |
| --- | --- |
| Demountable magnets | The default. The magnet envelope is replaceable at a literature REBCO screening value of 3×10²² n/m² (fast fluence, E > 0.1 MeV). |
| Loaded assumptions | The assumptions loaded with the study. |
| Authored baseline | The authored baseline scenario. |
| Permanent-trip test | Trips the magnets permanently to exercise the replacement and shutdown logic. It ends with negative net electricity. It is a numerical control, not a plant scenario, and is marked as such. |

**What if… sliders.** Each edit cancels any running calculation and recalculates after a short pause. Each row shows an edited badge and a "revert to preset" button once you move it away from the preset.

- Magnet service limit (10²¹ to 10²³ n/m², logarithmic), with the literature value marked and a reset button.
- Magnet replacement duration (14 to 365 days).
- Blanket service limit (10²⁵ to 10²⁷ n/m²).
- Tritium recovery fraction (0 to 1).
- Processing delay (0 to 7 days).
- Thermal-to-electric efficiency (0 to 1).
- Opening usable fuel / kg.

Revert all to preset undoes every edit. Recalculate history is a manual fallback. Rows only appear when the preset declares them.

**Magnet limits per region.** The magnet limit applies to fast fluence averaged over each named region: inboard, outboard and port sector. Fluence is each region's flux times operating time. The first region to reach the limit triggers the swap. The local peak inside a region is not resolved, so a hot spot can reach the limit sooner. One slider moves all region limits together.

**The timeline.** The bottom panel plots Magnet fluence, Electricity, Tritium or Full-power years for each arrangement. Click an arrangement chip to show or hide it; the one drawn in 3D is drawn thicker. Click or drag on the plot, or use the calendar years slider, to scrub. Scrubbing selects the computed snapshot at or before the requested time, and events are never interpolated across. In the Accumulated component fluence view, the 3D colours follow the scrubbed year. Jump to a calculated event… lists magnet replacements and every event, such as tritium imports, planned outages, service limits reached and replacements starting and completing.

The headline row shows:

- Year.
- State: Operating, Magnet replacement, Component replacement, Planned outage, Fuel-limited or Stopped.
- Usable tritium (with the amount still in processing on hover).
- Net electricity so far.
- Magnet swaps so far.
- Next magnet swap.

Replacement outages and planned outages are authored in duration and timing. The planned outages are illustrative, not an availability estimate. Select a component in the Outliner to see its accumulated fluence, replacements completed, trigger and operating events.

**Tritium inventory and net electricity.** Usable tritium follows breeder production, D-T burn, imports, processing delay, process loss and decay. It is conditional on the authored recovery fraction and delay. Net electricity is gross output minus auxiliary load under authored energy assumptions. It is unavailable when no total nuclear-heat response is bound to the transport run.

Conditional sensitivity (a collapsed section) runs 27 full-history reruns: recovery 0.90, 0.95 and 0.99, and delay and service limits at ×0.5, ×1 and ×2. These are authored probes, not uncertainty bounds. The Uncertainty section is covered in section 5.

### 4 Compare

The bottom panel shows the four arrangements side by side: breeding, magnet flux, magnet swaps over the horizon, first swap, lifetime net electricity and final usable tritium. "What changes" lists four contrasts, each the second arrangement minus the first, with a one-line takeaway written from the current numbers. Bar charts follow. Below them is the allocation sweep: a slider through the recorded blanket/shield splits, with three charts and the findings the recorded runs support. Drag the panel's top edge to resize it. Read the "What changes" table first, then look at the flags (section 4).

### 5 Evidence

This step shows what Avila Core checked. Compilation, run readiness and the scientific verdict are separate lines. The verdict reads NOT_EVALUATED. A saved Core workflow reads "executed and verified" when its receipts were re-checked on opening; that states the workflow ran, not that the design works. Use the Compile study button (top bar) and Run bound study stages only with an Avila Core executable set under Compiler settings (or `--core`). The recorded package supplies one.

If the saved receipts cover only the loaded assumptions, a badge says so, and Use the covered assumptions switches to them.

## 4. Reading the numbers

Every value carries a badge. Hover it for why it applies and what would settle it.

| Badge | Meaning |
| --- | --- |
| calculated | Calculated by the FARIS engine or a recorded solver run. |
| checked | A numerical control or check passed within its declared scope. |
| authored | An assumption written for this scenario. Tunable, not measured. |
| literature | Taken from cited literature. Citing a source does not qualify the value. |
| conditional | Valid only under stated conditions, such as the cold-data surrogate. |
| partial | A precision or coverage goal is not met. Use with care. |
| not evaluated | No claim either way. |
| failed | A check failed, or an error. |

Every not evaluated value, and every missing range, has an explanation: why it is not evaluated and a next step. Hover shows it, and it is also written as text on the screen and in exports. A transport result is also labelled "cold-data surrogate · NOT_EVALUATED": scientific qualification is not evaluated, even for completed transport.

In Compare, each transport difference carries a flag. The difference is compared with 2·√(SE₁²+SE₂²). "beyond 2σ sampling noise" means it exceeds that. "within 2σ sampling noise" means it does not. The runs use different seeds and their covariance is not modelled, so this is a screening flag, not a significance test. "Beyond 2σ sampling noise" says nothing about nuclear data, geometry or model form.

## 5. Uncertainty

On the Operate step, the Uncertainty in the history section runs an ensemble. The app starts it for each arrangement in the background once the history settles, the selected arrangement first. It can be cancelled, and any edit that changes the history cancels it. Nothing partial is shown. Finished ensembles are reused when the inputs are identical.

**What it does.** FARIS draws the driving rates (breeder H3 per source neutron, component fluxes and heating) from a multivariate normal. It uses the recorded transport means and covariance. It runs the deterministic history once per draw. The Samples choice is 200 or 1000; 1000 gives narrower sampling error and takes about five times longer. The same seed gives the same result at any thread count. For 200 samples of four arrangements, expect minutes ([STUDY_FILE.md](STUDY_FILE.md)).

**What you see.**

- Each continuous output shows its nominal value beside the median and the 5 to 95 per cent range (P5 to P95). Each has a distribution-free 95 per cent confidence interval on those quantiles, where enough samples exist.
- Discrete outputs, such as swap counts and the region that triggered first, show the share of samples with Wilson 95 per cent intervals.
- The timeline gets a shaded P5 to P95 band, labelled "bands: P5–P95, transport sampling only".
- In Compare, paired differences use the same sample index in both arrangements, so the pairing is valid only for independent transport runs.

**The 1 per cent rule.** A draw with a physically impossible rate (negative, or non-positive heating) is rejected and redrawn. If rejections exceed 1 per cent of accepted samples, the ensemble is NOT_EVALUATED, with the rejection percentage and the next step "run more histories or use variance reduction". A transport record without covariance is also NOT_EVALUATED: independence is never assumed.

**What it does not include.** Nuclear data, geometry, materials, model form, the tritium half-life and every authored assumption. The line "Transport Monte Carlo sampling uncertainty only, not nuclear data, model or assumption uncertainty." is shown with the section. A share of samples with a given swap count describes sampling noise, not a lifetime estimate.

**From the command line.**

```bash
faris history ensemble --assumptions A.json --rates R.json --output E.json \
  --samples 200 --seed 1 --threads 4
```

`--samples` is 1 to 2000. `--seed` and `--threads` are optional. A NOT_EVALUATED ensemble is written and exits 0, so read its `status`. Details are in [OPERATING_HISTORY.md](OPERATING_HISTORY.md).

## 6. Study files and export

**Save and open.** File > Save (Ctrl+S) and Save as… (Ctrl+Shift+S) write a `.faris` file. It holds both arrangements, the recorded transport, the allocation sweep, the operating assumptions, any finished ensembles and the view you left open. Calculated histories are not stored and recalculate on opening. Recorded files are checked by hash on every open. Save is greyed out, with a reason on hover, when nothing recorded can be saved. See [STUDY_FILE.md](STUDY_FILE.md).

**Referenced or packed evidence.** By default the Core evidence archives (the menu item shows their size) are recorded by name and hash and not stored. The study opens fully without them, and the Evidence step says "Core receipts not included", with the reason and next step. To use them, put the archives next to the file at the recorded relative paths and reopen it. Or tick "Include Core evidence in saved files (about +50 MB)" in the File menu and save again.

**Export.** The Export… button in the top bar asks for a folder and writes `<study name>-export/` into it. It never writes into an existing folder. It is unavailable while the histories, uncertainty ranges, sweep or saved evidence are still calculating or loading. Hover it for the reason.

| File | Contents |
| --- | --- |
| `summary.pdf` | Two pages: comparison, flags, timeline, sweep, assumptions, caveats and unknowns. A third page, on uncertainty, appears when an ensemble exists. |
| `data/*.csv` | histories, comparison, differences, sweep, assumptions, caveats, and the ensemble samples, summary and paired comparison. |
| `charts/` | The charts as SVG and PNG, and the 3D view when it was captured. |
| `export-manifest.json` | Every file with its SHA-256, the version, the statement and the study-file hash. |

Every CSV starts with a `# ` comment line holding the statement. Skip it with `pandas.read_csv(..., comment="#")`. From the command line:

```bash
faris study-file export demo.faris --output /path/to/parent
```

This recalculates the histories. It has no 3D view, and it reuses only the ensembles stored in the file. See [STUDY_EXPORT.md](STUDY_EXPORT.md).

## 7. Command line

The `faris` command needs no graphics, Python, OpenMC or Core.

| Command | What it does |
| --- | --- |
| `faris validate SCENARIO` | Validates a scenario and prints its identity. |
| `faris study-file create ... -o FILE` | Writes a `.faris` file from bundles and assumptions. `--pack-evidence` stores Core evidence inside. |
| `faris study-file inspect FILE` | Summarises a file without extracting it. |
| `faris study-file verify FILE` | Rehashes everything. Exits 1 on any failure. |
| `faris study-file unpack FILE DIR` | Writes the contents out. Refuses an existing directory. |
| `faris study-file export FILE --output DIR` | Writes the export folder. |
| `faris history run` | `--assumptions`, `--rates`, `--output`: one deterministic history. |
| `faris history from-run` | `--scenario`, `--run`, `--assumptions`, `--output`: bind rates from a successful run and calculate. |
| `faris history ensemble` | Section 5. |
| `faris history compare-runs` | Compare two runs under identical assumptions. |
| `faris history sensitivity` | Full-rerun sensitivity over a `--grid` file. |
| `faris transport pack --run RUN --output BUNDLE` | Packages a completed run into a portable bundle. |
| `faris doctor` | Reports whether `openmc`, `actinv` and `avila-core` are on `PATH`. It runs nothing. |

Run `faris COMMAND --help` for every flag. Errors exit with status 2; a file that fails verification exits 1. To run new transport, follow [TRANSPORT.md](TRANSPORT.md) and [COLD_REFERENCE.md](COLD_REFERENCE.md). The history commands are in [OPERATING_HISTORY.md](OPERATING_HISTORY.md).

## 8. Limits of this release

- **Geometry.** One ARC-inspired compact D-T tokamak, built from idealised
  concentric tori and one outboard port. It does not reproduce the published
  ARC engineering design.
- **Physics inputs.** Transport is a cold-data surrogate with an authored source
  and surrogate material recipes. Scientific qualification is NOT_EVALUATED,
  including for completed transport.
- **Uncertainty.** Only transport sampling uncertainty is propagated.
  Nuclear-data, geometry, material and model uncertainty are not, so the true
  uncertainty is larger than the ranges shown.
- **Magnet fluence.** Limits use regional averages. The local peak is not
  resolved; it needs variance reduction aimed at the magnet (planned for 0.2).
- **Activation.** No activation, decay heat or shutdown dose yet (planned for
  0.2). No physical degradation models.
- **Assumptions.** Service limits, outage durations, recovery fractions and
  plant efficiencies are authored or literature screening values, not material
  allowables or measured plant data. Fuel, maintenance, service events and
  electricity are conditional on them.
- **Local fields.** Sparse local mesh estimates keep unresolved sampling
  uncertainty. The port-window comparison is a declared mixed-volume spatial
  average, not a magnet peak.
- **Benchmarks.** Code-to-code comparison against ITER_1D is unavailable
  without traceable reference responses.
- **Platforms.** Linux only. The bundled programs are hash-pinned, not signed.

## 9. Troubleshooting

**"recorded case, mesh or response definitions differ from this FARIS build: …"** The study was recorded by an earlier build. The full message continues: "The run was probably recorded by an earlier version; rerun the transport case with this build to use it."

**"Cannot open NAME: …"** The file could not be opened. The reason follows. Reading is fail-closed. A file is refused if a blob does not match its recorded SHA-256 ("blob … does not match its recorded SHA-256"), if its size is wrong, if it uses an unsupported encoding, or if its version is not major version 1 ("unsupported study-file version …"). Run `faris study-file verify FILE` to see which part failed.

**"Core receipts not included".** The Core archives are referenced, not packed. See section 6. A listed archive that is "not found" is missing from beside the file. One whose SHA-256 differs from the record is not used.

**"No uncertainty range: …".** The ensemble is NOT_EVALUATED. The line gives the reason and the next step. Typical reasons are a transport record without covariance, or more than 1 per cent rejected draws.

**Export is unavailable.** Hover the button. It says when histories are still calculating, or when the study has no operating assumptions.

**The 3D view is missing from an export.** The export notes "The window capture did not arrive; this graphics backend may not support screenshots." The rest of the export is written.

**Graphics.** The desktop needs a graphical session and compatible graphics drivers. Leave display-backend and DPI overrides unset in normal use, so a high-DPI desktop reports its native scale. The recorded frame rates were measured on both X11 and Wayland.
