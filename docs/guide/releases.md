# Releases

FARIS follows [semantic versioning](https://semver.org/) from 0.1.0. Before 1.0, a minor version may change file formats or the command line. The changelog lists such changes under **Changed**, with what to do.

The full history is in the [changelog](https://github.com/AvilaLabs/FARIS/blob/main/CHANGELOG.md). Releases are on the [releases page](https://github.com/AvilaLabs/FARIS/releases).

## 0.2.0, 2026-10-08

Replacement outages computed from activation. See [Computed maintenance durations](maintenance.md).

- **Maintenance.** FARIS computes how long each replacement takes from the decay heat of the parts around the
  replaced component, reruns the 30-year history with those outages, and compares every design's downtime,
  availability and lifetime electricity with the fixed durations. In the validation runs, the downtime difference
  between designs came out 2 to 14 times its fixed-duration value.
- **Desktop.** A **Maintenance** window shows a result: both models side by side, a chart of every outage, and for
  each replacement which parts held the cooldown up and how long they had been in the machine. **Run…** computes a
  result with progress and cancel.
- **Command line.** `faris maintenance run` and `faris maintenance report`.
- **Package.** A recorded result for the four demo arrangements, and the activation-input builder for your own runs.
  Computing a result needs ACTINV, its data, Python and your own transport runs.

The demo study and its recorded transport and history results are the same as in 0.1.1.

## 0.1.1, 2026-10-06

The same demo study and results as 0.1.0. What changes is how it is distributed.

- **Download.** A smaller app download for each platform, with the Core receipts in an optional evidence pack. The Linux app download is about 35 MB and the evidence pack about 78 MB. 0.1.0 was one 112 MB archive.
- **Launcher.** `bin/faris-app` opens the study and checks the SHA-256 of every file it needs. Python is not needed, and `launch.sh` is gone.
- **Platforms.** Windows and macOS (Apple silicon and Intel) downloads. The programs are not signed. Windows and macOS builds are new in 0.1.1.
- **Fixes.** The tour states the real history counts. The export button hover no longer gives a page count. Saved Core evidence waits up to 10 minutes for unpacking and says why if it gives up. On Windows the app opens without a console window, and the `faris history` commands run.

A 0.1.0 package keeps its own `launch.sh` and verifier. To move to 0.1.1, download the new files. See [Download and verify](install.md).

## 0.1.0, 2026-10-05

The first public release, a deliberately narrow demo that is complete for one study: an ARC-inspired compact D-T tokamak, studied from 3D Monte Carlo transport through 30 years of operation, with the transport sampling uncertainty carried into every year.

- **Study.** A five-step workspace (Design, Simulate, Operate, Compare, Evidence) with a replayable first-run tour. Four recorded arrangements: two blanket/shield allocations, each with a finite outboard service port and a matched port-free control. A seven-point allocation sweep. A 30-year operating history that recalculates in about a second from sliders.
- **Uncertainty.** Covariance between all scalar transport responses from per-batch results. History ensembles of up to 2,000 histories, with a median, a 90 % range and event probabilities for every output. Ensembles fail closed when more than 1 % of draws are non-physical. A paired comparison of two arrangements' ensembles.
- **Magnets.** Fast-neutron flux on the inboard half, the outboard half and the sector behind the port, each with its own service limit. The first limit reached triggers the replacement and is named in the event.
- **Files and export.** `.faris` study files checked by hash on every open. Export to a PDF brief, CSV tables and SVG and PNG charts, from the desktop and from `faris study-file export`.
- **Evidence.** Studies compile and run through Avila Core, which records hash-bound receipts.

The known limits of this release are in [Scope and limits](scope.md).

Next: [Where FARIS is going](roadmap.md).
