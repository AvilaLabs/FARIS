# Changelog

FARIS follows [semantic versioning](https://semver.org/) from 0.1.0. Before
1.0 a minor version may change file formats or the command line; such changes
are listed here under **Changed** with what to do.

Research screening only. FARIS results are not a licensing, safety or design
basis.

## 0.1.0 — 2026-10-05

The first public release: an ARC-inspired compact D-T tokamak, studied from
3D Monte Carlo transport through 30 years of operation, with the transport
sampling uncertainty carried into every year.

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
