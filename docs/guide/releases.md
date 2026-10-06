# Releases

FARIS follows [semantic versioning](https://semver.org/) from 0.1.0. Before 1.0, a minor version may change file formats or the command line. The changelog lists such changes under **Changed**, with what to do.

The full history is in the [changelog](https://github.com/AvilaLabs/FARIS/blob/main/CHANGELOG.md). Releases are on the [releases page](https://github.com/AvilaLabs/FARIS/releases).

## 0.1.0, 2026-10-05

The first public release, a deliberately narrow demo that is complete for one study: an ARC-inspired compact D-T tokamak, studied from 3D Monte Carlo transport through 30 years of operation, with the transport sampling uncertainty carried into every year.

- **Study.** A five-step workspace (Design, Simulate, Operate, Compare, Evidence) with a replayable first-run tour. Four recorded arrangements: two blanket/shield allocations, each with a finite outboard service port and a matched port-free control. A seven-point allocation sweep. A 30-year operating history that recalculates in about a second from sliders.
- **Uncertainty.** Covariance between all scalar transport responses from per-batch results. History ensembles of up to 2,000 histories, with a median, a 90 % range and event probabilities for every output. Ensembles fail closed when more than 1 % of draws are non-physical. A paired comparison of two arrangements' ensembles.
- **Magnets.** Fast-neutron flux on the inboard half, the outboard half and the sector behind the port, each with its own service limit. The first limit reached triggers the replacement and is named in the event.
- **Files and export.** `.faris` study files checked by hash on every open. Export to a PDF brief, CSV tables and SVG and PNG charts, from the desktop and from `faris study-file export`.
- **Evidence.** Studies compile and run through Avila Core, which records hash-bound receipts.

The known limits of this release are in [Scope and limits](scope.md).

Next: [Where FARIS is going](roadmap.md).
