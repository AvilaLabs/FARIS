# FARIS

**Fusion Analysis and Reactor Integration Simulator, by Avila Labs.**

FARIS follows a fusion plant from 3D neutron transport to thirty years of
operation. It shows how a blanket and shield design changes tritium breeding,
magnet exposure, component replacements and net electricity year by year, and
how sure those numbers are.

> **Research screening only.** FARIS results are not a licensing, safety or
> design basis. Every number is labelled as calculated, authored, literature,
> conditional or not evaluated.

> **This release is a demo.** FARIS 0.2.0 is a deliberately narrow
> demonstration, complete for one study: an ARC-inspired tokamak in four
> arrangements, from transport to thirty years of operation with uncertainty.
> New in 0.2: replacement outages computed from activation decay heat. The full
> product widens the geometry and adds peak magnet fluence and shutdown dose;
> see [Where FARIS is going](#where-faris-is-going).

**[Read the FARIS Handbook](https://faris.avilalabs.org/docs/)**: download and verify, a tour of
each step, the command line, how to read the numbers, and the scope and limits.

![FARIS tour: the plant in 3D, transport on the model, the 30-year operating timeline and the four-arrangement comparison](docs/images/faris-tour.gif)

## What it does

The demo studies one ARC-inspired compact D-T tokamak (525 MW fusion)
in four arrangements: two blanket/shield allocations, each with a finite
outboard service port and a matched port-free control.

- **Transport.** Coupled neutron/photon OpenMC runs give tritium production,
  heating, flux spectra, a 3D flux map, and fast-neutron flux on three regions
  of the magnet, each with its Monte Carlo standard error and the covariance
  between all of them.
- **Operation.** A 30-year history recalculates in about a second as you move
  sliders: tritium inventory, fuel-limited stops and restarts, planned outages,
  magnet and blanket replacements when a service limit is reached, and net
  electricity.
- **Uncertainty.** Hundreds of histories on transport rates sampled from the
  recorded covariance give a median and 90 % range for every output, and the
  probability of each event, such as a magnet replacement before year 10.
- **Compare.** Side by side with two-sigma flags, a paired comparison of the
  uncertainty ensembles, and a seven-point blanket/shield allocation sweep.
- **Evidence.** Studies run through Avila Core, which records hash-bound
  receipts. A `.faris` file holds the whole study and is checked byte for byte
  on every open. Export gives a PDF brief, CSV tables and charts, from the
  desktop or the command line.

## Get it

Download the app for your platform from the
[latest release](https://github.com/AvilaLabs/FARIS/releases/latest) and check
it against `SHA256SUMS`:

| Platform | File |
| --- | --- |
| Linux | `FARIS-0.2.0-linux-x86_64.tar.gz` (about 35 MB; glibc 2.35 or newer) |
| Windows | `FARIS-0.2.0-windows-x86_64.zip` |
| macOS, Apple silicon | `FARIS-0.2.0-macos-aarch64.tar.gz` |
| macOS, Intel Mac | `FARIS-0.2.0-macos-x86_64.tar.gz` |

The Linux programs need glibc 2.35 or newer: Ubuntu 22.04 or later, Debian 12
or later. They also need a desktop session (X11 or Wayland) with libxkbcommon and a
Vulkan or OpenGL driver, which desktop installs include. On a minimal install, add
`libxkbcommon-x11-0` (Debian, Ubuntu) or `libxkbcommon-x11` (Fedora); without it
the app stops at launch with a panic in `xkbcommon-dl`.

Each archive unpacks to a folder `FARIS-0.2.0/`. Double-click `bin/faris-app`
(`bin\faris-app.exe` on Windows), or run it from a terminal. It finds the study
beside its `bin` folder, checks the SHA-256 of every file it needs, and opens
the four recorded arrangements and the sweep. `faris-app --package DIR` opens a
package elsewhere. If a check fails, the app opens with nothing loaded and says
why and what to do: download again and check `SHA256SUMS`. Python is not needed.

The optional `FARIS-0.2.0-evidence.tar.gz` (about 36 MB, the same for every
platform) adds the Avila Core receipts to the Evidence step. Unpack it into the
same place, so its files merge into `FARIS-0.2.0/`. It holds one evidence store,
a folder of compressed files in which each distinct file is kept once. With it,
the app reads the saved Core evidence straight from the store and checks each
file's length and SHA-256 as it reads it. Nothing is unpacked, so it needs no
temporary space. Without it, everything works except the saved receipts: the
Evidence step says "Core receipts not included", why, and what to do next.

On Linux, `./verify.sh` rechecks every recorded byte and the Core receipts. It
needs `python3`, the evidence pack and about 1.3 GB of temporary space, and it
checks the exact amount first. The Windows and macOS downloads are checked by the
app itself at launch.

The programs are not signed. Windows SmartScreen may warn: choose More info,
then Run anyway. On macOS the programs are neither signed nor notarized: after
unpacking, run `xattr -dr com.apple.quarantine FARIS-0.2.0` once in Terminal, or
open `bin/faris-app` and allow it under System Settings, Privacy & Security,
Open Anyway. A matching hash shows the bytes are the recorded ones, not who made
them.

It runs offline: no OpenMC, nuclear data or network access is needed to explore
the recorded study. You need a desktop session with working graphics drivers.
Every platform's programs are built by CI from the release commit; the study
evidence was recorded on Linux on the reference laptop. Windows and macOS builds
have been available since 0.1.1; report problems on GitHub issues.

## Using it

The workspace has five steps, keys 1 to 5:

1. **Design**: the plant in 3D. Orbit, cut away, and pick a component to
   inspect it.
2. **Simulate**: transport results on the model: flux, heating, fluence, and
   the local flux map.
3. **Operate**: the 30-year timeline. Change the operating assumptions and
   watch replacements, tritium and electricity follow. Run an uncertainty
   ensemble to see the range.
4. **Compare**: the four arrangements and the allocation sweep.
5. **Evidence**: what was run, with which inputs and receipts, and what is not
   evaluated.

A first-run tour walks through these; replay it with **Tour** in the top bar.
Save the study with Ctrl+S (File menu) and use **Export…** in the top bar for
the PDF brief, CSV tables and charts. The [user guide](docs/USER_GUIDE.md)
covers each step, the uncertainty results and the command line in detail,
and the [handbook](https://faris.avilalabs.org/docs/) covers the whole demo
chapter by chapter. Its source is in `docs/guide`; `mdbook build`, then
`scripts/prepare_web.py` and `npx wrangler@4 deploy --config wrangler.jsonc`
publish it.

## Reading the results

- **Kind labels.** *Calculated* comes from transport or the history ledger.
  *Authored* is an assumption you or the preset set. *Literature* is a cited
  value. *Conditional* depends on authored assumptions. *Not evaluated* means
  FARIS cannot give the number yet; hover or tap it for why and what would
  make it available.
- **Uncertainty.** Ranges carry only the Monte Carlo sampling uncertainty of
  the transport. Nuclear-data, geometry, material and model uncertainty are not
  included, so the true uncertainty is larger.
- **Service limits** (the magnet's 3 × 10²² n/m² fast fluence, for example) are
  screening values from the literature or authored, not material allowables.

## What the demo does not do

- Model a real machine: the geometry is idealised concentric tori with one port,
  not the published ARC design.
- Model a plugged port: the outboard port is an open duct with no shield plug,
  a bounding streaming case. In the port arrangements it drives the magnet
  replacements (about one a year with the default preset). A shield plug would
  reduce the streaming; FARIS has not modelled one.
- Report the local peak magnet fluence: it reports regional averages.
- Shutdown dose, or timing maintenance on contact dose. Decay heat times the
  computed replacement outages, which need ACTINV and your own transport runs;
  the Operate step's history uses fixed outage durations.

## Where FARIS is going

0.2 added computed maintenance durations: replacement outages worked out from
the decay heat of the parts around each replaced component, through
[ACTINV](https://github.com/AvilaLabs/ACTINV). Next, on the same study:

- **ARC-fitted geometry.** Separate inboard, outboard and vertical thickness for
  every layer, using the published ARC radial build. It adds 18 discrete TF coils,
  a double-null divertor, several ports, and cross-section and radial-build views.
  FARIS can then be compared with the published ARC magnet lifetime (at least 9
  full-power years to 3 × 10²² n/m²; Sorbom et al. 2015), and the computed
  outages with ARC's own maintenance durations.
- **Peak magnet fluence.** Variance reduction aimed at the magnet (FW-CADIS weight
  windows), so the local peak is reported alongside the regional averages.
- **Faster maintenance runs.** Most of the run time is ACTINV loading its data
  for each short cooling run; batching them in one ACTINV process could remove
  most of that.
- **Dose and remote handling.** A tested rule for timing maintenance on contact
  dose with remote handling, and material impurities bounded per ppm.

Further out, and not yet scheduled: CAD geometry (through Paramak and DAGMC),
shutdown dose. The plan and its decisions are in
[docs/DEMO_ROADMAP.md](docs/DEMO_ROADMAP.md).

Release notes are in the [changelog](CHANGELOG.md); measurable goals for every
part of FARIS are in [docs/requirements](docs/requirements/README.md).

## Build from source

### Run the desktop

Rust 1.98.1 is pinned. The first build needs network access to crates.io unless
dependencies are already cached. Run from this directory:

```bash
cargo run -p faris-app
```

The scenario is embedded, so the desktop also works after its binary is moved.
Load an authored scenario explicitly:

```bash
cargo run -p faris-app -- --scenario scenarios/arc-inspired/scenario.json
```

Drag in the viewport to orbit, Shift-drag to pan, scroll to zoom, and click a component to inspect
it. Use the outliner to select or hide components. The cutaway is a display
operation; it does not change the full-torus model or reported volumes.

The desktop needs a graphical session and compatible graphics drivers.
Linux source builds may need `pkg-config`, `libxkbcommon-dev`, and
`libwayland-dev` from the system package manager. Every platform's release
programs are built by CI from the release commit; the study evidence was
recorded on Linux on the reference laptop. Windows and macOS builds have been
available since 0.1.1.

Use **Interface size** in the top bar to enlarge text, controls, panels, and
plots together. Ctrl/Cmd + or − adjusts size; Ctrl/Cmd 0 resets it. The 100%
setting follows the desktop's display scaling. To start larger, pass
`--interface-scale 1.25`. Leave display-backend and DPI overrides unset during
normal use so a high-DPI desktop can report its native scale.

Select analyses in the left panel and use **Compile study** in the upper right.
Choose an external Core executable in compiler settings or pass `--core`.
Compilation, run readiness, and scientific assessment are separate. See
[generated Core studies](docs/CORE_STUDIES.md) for the CLI and recorded evidence.

For real transport, use the [cold-reference workflow](docs/COLD_REFERENCE.md).
Provide its scenario, both `--physics` files, the audited data XML, and existing
OpenMC Python/executable paths. The desktop runs the same engine as the CLI.
`--run /path/to/run.json` replays checked local results; `--field-view
flux-slice` starts with calculated spatial fields. Source geometry remains a
full torus. The display cutaway does not change transport.

### Study files

A `.faris` file is one study: both arrangements (with and without the port), the
recorded transport, the allocation sweep, the operating assumptions, and the view
you left open (step, preset, what-if values, year, field view, history tab, selected
arrangement and allocation). Calculated histories are not stored; they recalculate
on opening. The format, its size policy and its reading rules are in
[docs/STUDY_FILE.md](docs/STUDY_FILE.md).

```bash
cargo run -p faris-app -- demo.faris          # or File > Open, Ctrl+O, or drop the file on the window
```

The **File** menu in the top bar has Open, Save, Save as (Ctrl+O, Ctrl+S,
Ctrl+Shift+S). The window title shows the file name and a dot when the view differs
from what was saved. Recorded files are stored byte for byte and checked by hash on
every open, so the Core receipts' "unchanged since checked" guarantee survives a
save. By default the Core evidence archives (the menu item shows their size) are recorded by name
and hash, not included; the study opens fully without them and its Evidence step
says the receipts are not included and how to supply them. Tick **Include Core
evidence in saved files** to store them inside. Archives found beside the file at
the recorded relative paths (for example `port/archives/reference-case.tar.gz`
when the file sits at the package root) are checked against their hashes and used.

```bash
faris study-file create --bundle port/bundles/reference.transport-bundle.json \
  --control-bundle control/bundles/reference.transport-bundle.json \
  --sweep-bundle sweep/blanket-045cm.transport-bundle.json \
  --assumptions operating-assumptions.json \
  --evidence saved-study-port-reference.json [--pack-evidence] -o demo.faris
faris study-file inspect demo.faris     # manifest summary and sizes
faris study-file verify demo.faris      # rehash everything; exit 1 on any failure
faris study-file unpack demo.faris out/ # bundles and files back out; refuses an existing directory
faris study-file export demo.faris --output exports/  # PDF brief, CSV and charts, as the desktop's Export
```

The command-line export uses the uncertainty ensembles stored in the file and
never computes new ones; run them in the desktop or with `faris history ensemble`
first.

To open `.faris` files from your file manager on Linux, run
`scripts/install_desktop_integration.sh /absolute/path/to/faris-app` (user-level; add
`--uninstall` to remove it).

### Use the CLI

The default workspace member is the CLI, so headless operations do not build
the graphics stack. Python, OpenMC, and Avila Core are not required:

```bash
cargo run -- validate scenarios/arc-inspired/scenario.json
cargo run -- export --scenario scenarios/arc-inspired/scenario.json --output runs/geometry-001.json
cargo run -- doctor
```

Export refuses an existing destination to preserve previous run records.
`doctor` checks names on `PATH` without executing tools; detection does not
mean a FARIS adapter is available.

Operating histories and their uncertainty run headless from a recorded
transport run:

```bash
faris history from-run --scenario <scenario.json> --run <run.json> \
  --assumptions scenarios/arc-inspired/demountable-magnet-assumptions.json \
  --output history.json --rates-output rates.json
faris history ensemble --assumptions scenarios/arc-inspired/demountable-magnet-assumptions.json \
  --rates rates.json --samples 200 --output ensemble.json
```

See [OPERATING_HISTORY.md](docs/OPERATING_HISTORY.md) for the ledger and the
ensemble method.

The first executable scientific increment is a mathematical transport control:

```bash
cargo run -- control absorber --python /path/to/openmc-env/bin/python \
  --openmc /path/to/openmc-env/bin/openmc --output runs/absorber-001
```

It uses synthetic one-group data, 1,000,000 histories and one thread by default.
The Rust worker caps histories, runtime and captured logs; Ctrl-C cancels its
  solver process group. External execution currently requires Unix. A numerical
control PASS applies only to the declared control, with its sampling rule and
limitations in [NUMERICAL_CONTROLS.md](docs/NUMERICAL_CONTROLS.md). It establishes
no reactor prediction or nuclear-data qualification. Linux jobs have explicit
address-space, file-size, artifact-count and total-output limits; the runner is
not a general sandbox for hostile executables.

Transport imports are independently usable without OpenMC:

```bash
cargo run -- transport validate-request --scenario scenarios/arc-inspired/scenario.json --request /path/to/request.json
cargo run -- transport normalize --scenario scenarios/arc-inspired/scenario.json \
  --request /path/to/request.json --artifact /path/to/raw-tallies.json --output runs/normalized-001.json
```

An import checks definitions and arithmetic; the artifact's solver identities
and raw numbers are adapter assertions. Import success leaves scientific
qualification `NOT_EVALUATED`. See [TRANSPORT.md](docs/TRANSPORT.md) for contracts,
normalization, trust limits and unavailable reactor functionality.

## Development

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python3 -m unittest discover -s controls -p 'test_*.py'
python3 controls/check_history.py
python3 -m unittest discover -s scripts -p 'test_*.py'
```

Use `--jobs 1` when compiling the graphics stack on a memory-constrained machine.
For a headless core check, run `cargo test -p faris-model -p faris-engine -p faris-cli`.

An opt-in desktop rendering check captures only FARIS's own window and exits:

```bash
cargo run -p faris-app -- --capture runs/desktop-preview.png
```

The screenshot destination's directory must already exist. Screenshots are
development artifacts, not simulation evidence.

## Project map

| Path | Responsibility |
| --- | --- |
| `crates/faris-model/` | Scenario types, units, validation, and identities |
| `crates/faris-engine/` | Shared geometry, transport normalization/jobs, history, comparison and Core evidence |
| `crates/faris-study/` | The `.faris` study file: verified, content-addressed container (no UI) |
| `crates/faris-cli/` | Headless client of the same engine |
| `crates/faris-app/` | Native egui workspace and wgpu renderer |
| `scenarios/arc-inspired/scenario.json` | The single editable demo scenario |
| `docs/USER_GUIDE.md` | User guide for the desktop app and command line (release 0.2.0) |
| `docs/DEMO.md` | Demo question, requirements, and completion criteria |
| `docs/ARCHITECTURE.md` | Rust boundaries, units, adapter and evidence design |
| `docs/ROADMAP.md` | Long-term project direction and development phases |
| `docs/DEMO_ROADMAP.md` | Comprehensive demo-only roadmap: dependencies, detailed requirements, and acceptance walkthrough |
| `docs/TOOLING.md` | Open-source tools and planned integration roles |
| `docs/SCIENTIFIC_BASELINE.md` | Benchmark selection, validation scope, uncertainty rules and unresolved scientific inputs |
| `docs/MATERIAL_BASELINE.md` | Primary-source material candidates and inputs that still need resolution |
| `docs/STUDY_FILE.md` | The `.faris` study file: container, size policy, reading rules |
| `docs/TRANSPORT.md` | Strict raw-tally contracts, dimensions and checked Rust normalization |
| `controls/` | Independent analytic controls and external OpenMC numerical checks |
| `references/` | Identified scientific sources and read-only tool/data audits |
| `integrations/` | Scientific adapters, Core template and integration documentation |
| `scripts/` | Portable distribution packaging, bounded extraction and verification |
| `runs/` | Ignored generated records and captures |
| `data/raw/` | Ignored downloaded scientific inputs |

## License and repository

Copyright © 2026 Avila Labs. FARIS source code and documentation are licensed
under the [GNU Affero General Public License, version 3 only](LICENSE)
(`AGPL-3.0-only`). Third-party tools and data retain their respective licenses.

Cargo packages use `publish = false` while the scaffold evolves.
The project repository is [AvilaLabs/FARIS](https://github.com/AvilaLabs/FARIS).
