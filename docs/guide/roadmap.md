# Where FARIS is going

0.2 added [computed maintenance durations](maintenance.md): replacement outages worked out from the decay heat of
the parts around each replaced component, through [ACTINV](https://github.com/AvilaLabs/ACTINV). Next, on the same study:

- **ARC-fitted geometry.** Separate inboard, outboard and vertical thickness for every layer, using the published ARC radial build. It adds 18 discrete TF coils, a double-null divertor, several ports, and cross-section and radial-build views. FARIS can then be compared with the published ARC magnet lifetime (at least 9 full-power years to 3 × 10²² n/m²; Sorbom et al. 2015), and the computed outages with ARC's own maintenance durations.
- **Peak magnet fluence.** Variance reduction aimed at the magnet (FW-CADIS weight windows), so the local peak is reported alongside the regional averages.
- **Faster maintenance runs.** Most of the run time is ACTINV loading its data for each short cooling run; batching them would cut a run from minutes to seconds.
- **Dose and remote handling.** A tested rule for timing maintenance on contact dose with remote handling, and material impurities bounded per ppm.

Further out, and not yet scheduled: CAD geometry (through Paramak and DAGMC), and shutdown dose. The plan and its decisions are in the [demo roadmap](https://github.com/AvilaLabs/FARIS/blob/main/docs/DEMO_ROADMAP.md).

Measurable goals for every part of FARIS are in the [requirements](https://github.com/AvilaLabs/FARIS/blob/main/docs/requirements/README.md).

Next: [Avila Labs account (optional)](account.md).
