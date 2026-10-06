# Uncertainty ensembles

An ensemble carries the Monte Carlo sampling uncertainty of the transport through the 30-year history. You see it on the Operate step, under **Uncertainty in the history**.

## How it runs

The app starts an ensemble for each arrangement in the background once the history settles, the selected arrangement first. You do not start it by hand.

- It can be cancelled. Any edit that changes the history cancels it.
- Nothing partial is shown.
- Finished ensembles are reused when the inputs are identical.
- The ensembles are saved in a [study file](study-files.md), so a reopened study does not recompute them.

Under **Samples**, choose 200 or 1000. 1000 gives narrower sampling error and takes about five times longer. For 200 samples of four arrangements, expect minutes. The same seed gives the same result at any thread count.

## What it does

FARIS draws the driving rates from a multivariate normal distribution. The rates are the breeder H3 per source neutron, the component fluxes and the heating. The distribution uses the recorded transport means and the covariance between them, so correlated quantities move together. FARIS runs the deterministic history once for each draw and summarises the spread. [Method](method.md) has the details.

## What you see

- Each continuous output shows its nominal value beside the median and the 5 to 95 per cent range (P5 to P95). The range is a 90 % range. Each quantile has a distribution-free 95 per cent confidence interval, where enough samples exist.
- Discrete outputs, such as swap counts, show the share of samples with each value, with Wilson 95 per cent intervals.
- For a magnet with region limits, the first-trigger line says which region reached its limit first, for example "Magnet swap triggered first by: port sector in 97 % of samples, inboard in 3 %".
- The timeline gets a shaded P5 to P95 band, labelled "bands: P5–P95, transport sampling only".
- In [Compare](compare.md), paired differences use the same sample index in both arrangements. The pairing is valid only for independent transport runs.

A share of samples with a given swap count describes sampling noise. It is not a lifetime estimate.

## The 1 per cent rule

A draw with a physically impossible rate, such as a negative rate or a non-positive heating power, is rejected and redrawn. If rejections exceed 1 per cent of accepted samples, the ensemble is not evaluated. The result states the rejection percentage and the next step: run more histories or use variance reduction. No summary is produced.

A transport record without covariance is also not evaluated. Independence is never assumed. The result says to rerun transport with this FARIS version to record the batch-resolved results.

## What it does not include

The ranges cover transport Monte Carlo sampling uncertainty only. They do not include nuclear-data, geometry, material or model-form uncertainty, the tritium half-life, or any authored assumption. The true uncertainty is larger. The section carries the line "Transport Monte Carlo sampling uncertainty only, not nuclear data, model or assumption uncertainty."

## From the command line

```bash
faris history ensemble --assumptions A.json --rates R.json --output E.json \
  --samples 200 --seed 1 --threads 4
```

`--samples` is 1 to 2000 and defaults to 200. `--seed` and `--threads` are optional. An ensemble that is not evaluated is written and the command exits 0, so read the `status` field of the output. See [Command line](cli.md).

Next: [Compare and the allocation sweep](compare.md).
