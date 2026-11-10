<!-- kind: explanation -->
# Where FARIS is going

0.2 added [computed maintenance durations](maintenance.md): replacement outages worked out from the decay heat of
the parts around each replaced component, through [ACTINV](https://github.com/AvilaLabs/ACTINV). Next, on the same study:

- **ARC-fitted geometry.** Separate inboard, outboard and vertical thickness for every layer, using the published ARC radial build. It adds 18 discrete TF coils, a double-null divertor, several ports, and cross-section and radial-build views. FARIS can then be compared with the published ARC magnet lifetime (at least 9 full-power years to 3 × 10²² n/m²; Sorbom et al. 2015), and the computed outages with ARC's own maintenance durations.
- **Peak magnet fluence.** Variance reduction aimed at the magnet (FW-CADIS weight windows), so the local peak is reported alongside the regional averages.
- **Faster maintenance runs.** Most of the run time is ACTINV loading its data for each short cooling run; batching them in one ACTINV process could remove most of that.
- **Dose and remote handling.** Dose-governed maintenance now uses ICRP-116 AP photon effective dose rate at an authored task location, in Sv/h, with an authored remote-handling limit and exact event/history coupling. The bounded CAD path uses 709-group neutron response, ACTINV per installed component history, and full-geometry photon transport. Missing event curves remain `NOT_EVALUATED`; incomplete activation or source coverage is refused. Heterogeneous installation histories use R2S, with stock D1S explicitly `NOT_EVALUATED`; that case does not pass the D1S/R2S comparison gate. Source-resolution convergence, method comparison, and experimental validation remain open. See [Dose-governed maintenance](maintenance-spatial-dose.md).

Your own design from CAD is on main and in the next release: `faris design init`, `check` and `transport` take a STEP file with a design file naming each solid's material and role, and the desktop app has Design check and Design flux windows ([design file guide](design-file.md)). Sweep integration and validation of the CAD shutdown-dose path remain release work. The plan and its decisions are in the [next-release plan](https://github.com/AvilaLabs/FARIS/blob/main/docs/NEXT_RELEASE_PLAN.md).

Measurable goals for every part of FARIS are in the [requirements](https://github.com/AvilaLabs/FARIS/blob/main/docs/requirements/README.md).

Next: [Avila Labs account (optional)](account.md).
