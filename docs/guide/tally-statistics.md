<!-- kind: reference -->
# Tally statistics

A Monte Carlo tally is a mean with an estimated error, and the error is itself an estimate. This page says how FARIS checks that the error can be believed, what interval it shows, how it writes the digits, and what it refuses to say. The code is `faris_model::tally_statistics`, `faris_model::interval_display` and `faris_engine::tally_health`.

## What FARIS keeps from a run

The reactor worker writes `transport-batch-values.json` beside the transport artifact: one value per batch for every scalar response, the differences of consecutive OpenMC cumulative sums. FARIS computes everything below from those batch values. They are batch means, not per-history scores, and that decides what can and cannot be computed.

| Needs | Source | Available |
| --- | --- | --- |
| Mean, standard error, variance of the variance of the batch values | batch values | yes |
| Running mean, relative error and variance of the variance over the last half of the run | batch values | yes |
| Figure of merit per batch | batch values | yes |
| Figure of merit per second | cumulative wall time at the end of each batch (`batch_end_seconds`) | read when present; the worker does not write it yet |
| Count of scoring histories | per-tally event counts | no: estimated as `1/R²` (effective histories) |
| Tally slope (check 10) | the largest per-history scores | no: NOT_EVALUATED |
| Variance of the variance over histories (MCNP's own form) | third and fourth moments of the history scores | no: see "Asking runs for more" |

## The ten statistical checks

The reference is the list of ten checks in the MCNP manual. FARIS runs all ten on every tally with batch values and reports each as PASS, FAIL or NOT_EVALUATED.

| # | Check | Pass condition in FARIS |
| --- | --- | --- |
| 1 | mean has no trend | the running mean has no strictly monotonic drift over the last half larger than its standard error |
| 2 | relative error size | below 0.10 (below 0.05 for a point detector) |
| 3 | relative error decreases | at the end of the run not above its value at half the run, beyond sampling noise |
| 4 | relative error falls as 1/√N | the log-log slope from half the run to the end is within sampling noise of -0.5 |
| 5 | variance of the variance size | below 0.1 |
| 6 | variance of the variance decreases | not above its value at half the run, beyond sampling noise |
| 7 | variance of the variance falls as 1/N | the log-log slope is within sampling noise of -1 |
| 8 | figure of merit constant | within 20 % of the final value over the last half, widened by the sampling noise of the variance estimate |
| 9 | figure of merit has no trend | no strictly monotonic change larger than the same limit |
| 10 | tally slope | above 3 (NOT_EVALUATED here, see above) |

The limits of 0.10, 0.05 and 0.1 are the MCNP guideline values. The 20 % limit is a FARIS choice. Checks 3, 4, 6, 7, 8 and 9 compare two noisy estimates from batch data, so a strict "monotonic decrease" would refuse good tallies. FARIS allows three standard deviations of the sampling noise of the estimate (an error from `k` batches has a relative standard deviation of `1/√(2(k−1))`). This is a FARIS adaptation to batch data, not the MCNP rule. Its effect, measured on synthetic normal batches, is in "False refusals".

A tally with a failing check gets no verdict and no interval. A tally with fewer than 10 batches, or a non-finite batch value, is NOT_EVALUATED. A tally where no check failed but some could not be evaluated is "PASS with gaps"; the gaps are listed.

## Variance of the variance

`VOV = Σ(x−x̄)⁴ / (Σ(x−x̄)²)² − 1/n` over the batch values. It is near `2/n` for normal batches and near 1 when one batch holds most of the sum. MCNP defines the same quantity over the individual history scores; with batch values it is a coarser statistic that sees a heavy batch but not a heavy history that has not occurred. FARIS marks which it computed.

## Figure of merit

`FOM = 1/(R²·T)` for relative error `R` and cost `T`. With recorded batch times `T` is wall seconds. Without them `T` is the batch count and the figure says "per batch, batch times not recorded, equal batches assumed". The two agree only if every batch costs the same time. The trend over the last half is reported with the figure, and the largest change from the final value is checked (check 8).

## Effective number of scoring histories

`N_eff = 1/R²`. A unit-weight tally scored by `q·N` of `N` histories has `R² = (1−q)/(qN)`, so `N_eff` is the count a Poisson tally with the same relative error would have. Weighted scores make it smaller than the number of histories that scored. A tally with fewer than 1000 effective scoring histories is flagged low-sample (a FARIS choice). The true count of scoring histories is not recorded; see below.

## Intervals that hold at low counts

The 1 σ standard error is what the run reports. The interval shown beside it is 95 % and is built so that it does not rely on the Gaussian assumption.

- With `N_eff` of 10 or more, the interval is the wider of two: the Student t interval on the batch means (`n−1` degrees of freedom), and the Garwood Poisson interval scaled to the tally (the exact interval for a count, from the chi-squared quantiles, with coverage of at least the stated level for every mean).
- With `N_eff` under 10, only a one-sided 95 % upper bound is shown (scaled Garwood). There is no lower bound.
- With no scoring event, or a zero or negative mean, there is no interval: NOT_EVALUATED, with the reason and the next step.
- A tally that fails the checks, or whose reported error disagrees with its batch values, gets no interval.

The Wilson score interval is available for proportions, such as the share of histories that scored. It does not replace the Garwood interval for counts.

None of these can see a heavy tail that has not yet been sampled. That is why the checks gate the interval and why the interval always lists what it excludes.

## Error from independent batches and seeds

The reported standard error is compared with the standard error of the batch values. An error estimated from `n` normal batches has a relative sampling error of about `1/√(2(n−1))`; the check passes when the two agree within `z < 3`. Over several seeds, the spread of the means is compared with the root-mean-square reported error in the same way (`s/√(2(K−1))` for `K` runs). The seed check needs independent runs; it is available as a function and not run automatically.

## Sums of tally parts

Parts of one run share batches, so their errors are correlated. FARIS sums parts with the recorded response covariance: the combined variance is `wᵀCw`. If the record has no covariance, or a part is missing from it, the sum is refused and says why. Independence is never assumed. A sum can be checked against a global tally of the same run; the difference is tested against its own error, including the covariance between the sum and the total.

## Mesh and spectrum convergence

For a mesh or spectrum tally FARIS reports the number and fraction of bins with relative error of at most 10 % and at most 5 %, the number with fewer than 1000 effective scoring histories, and the number with no score. Unscored bins count as not converged; they are never counted as zero error.

## Digits and interval wording

The value is rounded so its last shown digit is the last digit of the uncertainty. The uncertainty is shown to two significant digits when its leading three digits are below 355, otherwise to one (the rule of the Particle Data Group, inside the limit of two in the GUM, clause 7.2.6). Scientific form is used outside 10⁻³ to 10⁶. With no usable uncertainty the value is shown to three significant digits and the text says there is none.

Every interval is written with its convention beside it: "1 σ standard error", "2 σ", "95 % confidence interval", "P5 to P95 range". Intervals of different conventions never share a column unless each column is labelled (`single_convention`). Bounds are rounded outward so the displayed interval is never narrower than the computed one. When part of the uncertainty is not evaluated, the text ends with "excludes: <part>". The tally intervals exclude nuclear data, geometry, model form and heavy tails not yet sampled.

## From the command line

```bash
faris transport statistics --batch-values transport-batch-values.json \
  --artifact transport-artifact.json --output statistics.json
```

Each tally prints its value with the 1 σ standard error, its status, the interval with its convention, the relative error, the variance of the variance, the effective scoring histories, the figure of merit, the agreement of reported and batch errors, and every check that did not pass with its reason. `--point-detector` applies the 5 % guideline. The report is JSON when `--output` is given.

## Asking runs for more

Three additions would turn the NOT_EVALUATED parts into results. They change the worker, not the checks.

1. Record the cumulative wall time at the end of each batch as `batch_end_seconds`. The reader already uses it.
2. Request OpenMC higher moments on the scalar tallies (`tally.higher_moments = True`), which add the third and fourth central moments of the batch scores. This gives the batch-level skew and kurtosis from OpenMC itself.
3. For the tally slope and MCNP's history-level variance of the variance, record per-history scores. OpenMC does not keep them. The worker would add a small scoring filter or a tally of the largest scores per batch; this needs a run and is not done here.

## False refusals

A statistical check on noisy data refuses some good tallies. On synthetic normal batches the measured refusal rates are in the test log of `faris-model` (`tally_statistics::tests::good_normal_tallies_are_refused_rarely`): the variance of the variance below 0.1 is the most demanding check at few batches, since its expected value for normal batches is `2/n`. Use 100 or more batches for tallies that must pass. Refusing is the safe error: FARIS prefers to say NOT_EVALUATED on a good tally over giving a verdict from a bad one.

## Sources

- X-5 Monte Carlo Team, *MCNP: A General Monte Carlo N-Particle Transport Code, Version 5*, Volume I, LA-UR-03-1987 (revised 2008), chapter 2, the section on estimation of Monte Carlo precision and the ten statistical checks, including the variance of the variance, the figure of merit and the tally slope (with the work of T. E. Booth and R. A. Forster on the variance of the variance and large-score tails cited there).
- B. C. Kiedrowski and collaborators at Los Alamos on the variance of the variance and figure-of-merit behaviour in MCNP tallies.
- OpenMC documentation, Methods, Tallies: the batch-based standard error and the higher-moments option.
- F. Garwood, "Fiducial limits for the Poisson distribution", *Biometrika* 28 (1936) 437 to 442.
- E. B. Wilson, "Probable inference, the law of succession, and statistical inference", *J. Amer. Statist. Assoc.* 22 (1927) 209 to 212.
- L. D. Brown, T. T. Cai and A. DasGupta, "Interval estimation for a binomial proportion", *Statistical Science* 16 (2001) 101 to 133, on why the Gaussian interval fails at low counts.
- "Student" (W. S. Gosset), *Biometrika* 6 (1908) 1 to 25, for the t interval on batch means.
- JCGM 100:2008 (GUM), clause 7.2.6, on the digits of a reported uncertainty; Particle Data Group, *Review of Particle Physics*, Introduction, on the rounding rule for two digits or one.

The literature check was made from these published references and the project's earlier notes without live access to the papers. The Booth, Forster and Kiedrowski items are cited as the sources the MCNP manual points to, and have not been re-read for this page. Treat their exact titles as to be confirmed before an outside publication.

Next: [Uncertainty ensembles](uncertainty.md).
