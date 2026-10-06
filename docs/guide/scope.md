# Scope and limits

FARIS 0.1.0 is research-screening software and a deliberately narrow demo. Its results are not a licensing, safety or design basis. Nothing in FARIS is a licence, safety, certification or validation claim.

FARIS is a design model of one study. It is not a model of a real machine.

## What the demo is not

- **Not the published ARC design.** The geometry is idealised: concentric tori with one outboard port. The ARC paper is design precedent, not a specification.
- **Not a plugged port.** The outboard port is an open 0.30 m × 0.30 m duct with no shield plug. It is a bounding streaming case. In the port arrangements it drives the magnet replacements. With the demountable-magnet preset, the port-sector magnet limit is reached 0.65 years into operation and the magnets are replaced about 30 times in 30 years. Without the port, they are replaced at most once. A shield plug would reduce the streaming, and FARIS has not modelled one.
- **Not a peak magnet fluence.** The magnet check uses regional averages: inboard, outboard and port sector. A local peak needs variance reduction aimed at the magnet, which is planned for 0.2. A hot spot can reach a limit sooner than its region's average does.
- **No activation, decay heat or shutdown dose.** Maintenance timing ignores them. They are planned for 0.2, and shutdown dose later. There are no physical degradation models.
- **Linux only.** The bundled programs are hash-pinned, not signed.

## Transport inputs

Transport is a cold-data surrogate. The materials are authored recipes, not a design's real materials. The first wall is tungsten, the blanket solid Li₂BeF₄ and the shield an ideal titanium hydride. Iron stands in for the structure and copper for the magnet. The data are at one nominal temperature, with no thermal-scattering data for the crystalline blanket and shield materials. Low-energy spectra and breeding are therefore not qualified.

The source is authored. The whole-model tritium response includes production anywhere in the model, so it is a gross rate and not a breeder-only breeding ratio. Heating is an OpenMC deposition score, not a thermal balance, and the history's energy ledger is not a coolant or thermal-cycle calculation.

Scientific qualification is not evaluated, including for completed transport. A successful run is not a scientific verdict. The recorded campaign does not reproduce an experimental neutron-transport benchmark, and it has no design-level validation. A code-to-code comparison against an ITER 1D reference is unavailable without traceable reference responses.

## Uncertainty

Only transport Monte Carlo sampling uncertainty is propagated. Nuclear-data, geometry, material and model uncertainty are not, so the true uncertainty is larger than the ranges shown. The tritium half-life and every authored assumption are not sampled either.

The recorded runs use 30 million histories for the port-free controls and 10 million for the port and sweep cases. Standard errors are below 0.04 % for tritium production and heating, and 5 % to 24 % for the regional magnet fast flux (up to 30 % for the port-sector region of the port-free controls).

## Authored assumptions

Service limits, outage durations, recovery fractions, processing delays and plant efficiencies are authored or literature screening values. They are not material allowables or measured plant data. Fuel, maintenance, replacement events and electricity are conditional on them. The planned outages are illustrative and are not an availability estimate.

The magnet's 3 × 10²² n/m² fast-fluence limit is a literature REBCO screening value (Sorbom et al. 2015). It is applied to the average of each region.

## Local fields

Sparse local mesh estimates keep their unresolved sampling uncertainty. The port-window comparison is a declared mixed-volume spatial average, not a magnet peak.

## Where the evidence stands

The repository tracks the evidence and the open work: the [scientific baseline](https://github.com/AvilaLabs/FARIS/blob/main/docs/SCIENTIFIC_BASELINE.md), the [requirements](https://github.com/AvilaLabs/FARIS/blob/main/docs/requirements/README.md) and the [demo roadmap](https://github.com/AvilaLabs/FARIS/blob/main/docs/DEMO_ROADMAP.md). Keep the exact version with a study. Later changes do not rewrite what a recorded study holds.

Next: [Troubleshooting](troubleshooting.md).
