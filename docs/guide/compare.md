# Compare and the allocation sweep

Press 4 to open the Compare step. The bottom panel shows **Compare the four recorded arrangements**. Drag the panel's top edge to resize it.

## The four arrangements

The panel has one column for each allocation, Reference (0.45 m blanket, 0.45 m shield) and Breeder-heavy (0.55 m, 0.35 m), and one row for each port setting, **With port** and **No port**. Each cell shows breeding, magnet flux, magnet swaps over the horizon, the first swap, lifetime net electricity and final usable tritium. Transport rows carry Monte Carlo standard errors. History rows are conditional on the operating assumptions you selected.

## What changes

**What changes** lists four contrasts. Each row is the second arrangement minus the first:

- Breeder-heavy − Reference, with port.
- Breeder-heavy − Reference, no port.
- Port − No port, reference.
- Port − No port, breeder-heavy.

Columns give the change in breeding, magnet flux, swaps, first swap and net electricity. A one-line takeaway is written from the current numbers. Read this table first. Bar charts of lifetime net electricity and magnet swaps follow.

## The 2σ flags

Each transport difference carries a flag. FARIS compares the difference with 2·√(SE₁² + SE₂²), where SE₁ and SE₂ are the two standard errors.

- "beyond 2σ sampling noise" means the difference exceeds that.
- "within 2σ sampling noise" means it does not.

The runs use different seeds and their covariance is not modelled. So the flag is a screening aid, not a significance test. "Beyond 2σ sampling noise" says nothing about nuclear data, geometry or model form.

## Uncertainty in the history comparison

When the ensembles are ready, this section shows each contrast as a paired difference, sample by sample, with its range and the share of pairs where one arrangement is below, equal to or above the other. A contrast that cannot be compared says why and what to do. See [Uncertainty ensembles](uncertainty.md).

## The allocation sweep

Below the contrasts is the **Allocation sweep**, a set of seven recorded transport runs. They keep the same radial envelope and move thickness from shield to blanket. Each is an independent run with its own seed.

Move the **Allocation** slider through the splits. It shows the blanket and shield thickness of the selected point. Three charts show breeding, magnet flux and the histories. Their bars are sampling error only.

**What the sweep shows** lists the findings the recorded runs support. Statements about transport carry the badge *calculated*. Statements about replacements and electricity carry *conditional on* the selected preset, because they come from the operating history under authored assumptions. Transport uncertainty is not propagated into them.

Next: [Evidence and Avila Core receipts](evidence.md).
