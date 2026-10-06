# Changelog

FARIS follows [semantic versioning](https://semver.org/) from 0.1.0. Before
1.0 a minor version may change file formats or the command line; such changes
are listed here under **Changed** with what to do.

Research screening only. FARIS results are not a licensing, safety or design
basis.

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
  issues. Linux is built and tested on the reference laptop.
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
