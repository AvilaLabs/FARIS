# Does stopping at the target error bias the answer?

`faris design transport` stops at the first batch where the relative standard error of the response is at or
below the target (after 30 batches). A run that stops the moment its error estimate dips under a target can stop
on a low estimate of its own variance, and a streaming response has rare large scores that make this more likely.
This note tests whether the stopped answers are biased on RM-M.

## Protocol (frozen 2026-10-13, before any run of it)

**Model and configuration.** RM-M's checked design (`scenarios/rm-m/rm-m.faris-design.json`, sha256 b60c069e…) and
its check report with conversion 19f9b240…, FARIS at commit 46360f0, FENDL-3.2 with the ENDF/B-VII.1 photon
overlay, OpenMC 0.15.3 with the aiming patch for the run and 0.16.0 for the windows, 4 threads on cores 0–3,
20,000 histories per batch. Windows and the channel search come from one cache directory, filled by the first run,
so every run uses the same windows and the same aimed model.

**Runs.**

- **Stopped runs:** 20 runs with `--target-error 0.05` and seeds 501 to 520, each as a user would run it.
- **Long run:** one run with seed 601, `--target-error 0.001` and `--max-histories 12000000`, so that it ends at
  its history limit and not at a target. Its expected error is about 1.5 %.

**Quantities.** The response is the mean flux above 0.1 MeV over the 18 TF coils. With `m_i` the 20 stopped
results, `m_stop` is their plain mean and `s_stop` the standard error of that mean from their scatter. `m_long`
and `s_long` are the long run's value and standard error. The bias estimate is `b = m_stop / m_long - 1`, with
standard error `s_b` from `s_stop` and `s_long` combined in quadrature (relative).

**Reading rule.**

- **PASS:** `|b| <= 2 s_b` and `s_b <= 0.025`. No bias is detected at the 2.5 % level or better.
- **FAIL:** `|b| > 2 s_b`. The stop rule is changed (for example, the target must hold over several consecutive
  batches) and the test repeated under an amendment.
- **INCONCLUSIVE:** otherwise (`s_b > 0.025` with `|b| <= 2 s_b`).

Also reported, without a verdict: the number of the 20 runs whose own one-standard-error interval contains
`m_long` (about 14 expected if the reported errors are honest), the histories each run took, and the scatter of
the 20 results against the 5 % they each report.

**Limits.** One model, one target error, one batch size. The test cannot show the absence of a bias smaller than
its `s_b`. A machine that is busy changes run times, not results.

## Results

Appended below when the runs are done.

### 2026-10-13: verdict PASS

All 21 runs completed with no rerun, all in mode `aimed`, with binary sha256 a0ad679b…. Seed 501 made the weight
windows and the channel search; seeds 502–520 and 601 used the kept ones. Raw files are in
`~/.cache/avila-night/stop-rule/` (`run-SEED.json`, `results.json`); the figures below were recomputed from the
run files.

| Quantity | Value |
|---|---|
| `m_stop` (mean of the 20 stopped runs) | 9.5567e12 n/m²/s |
| `s_stop` | 0.94 % |
| `m_long` (seed 601, 12,000,000 histories, stopped at its history limit) | 9.7718e12 n/m²/s |
| `s_long` | 1.26 % |
| `b` | −2.20 % |
| `s_b` | 1.57 % |

`|b|` = 2.20 % is within `2 s_b` = 3.14 %, and `s_b` is under 2.5 %: **PASS**. No bias from stopping at the
target is detected at this resolution. The estimate is negative (the stopped runs sit 1.4 `s_b` low), which is
the direction a stop on a low variance estimate would give; this test cannot tell a bias of that size from none.

Also recorded:

- Coverage: 13 of 20 stopped runs lie within their own reported error of `m_long` (about 14 expected).
- Scatter of the 20 results: 4.20 % (sample standard deviation); their mean reported error is 4.44 %.
- Histories: 15 of 20 stopped at the first allowed batch (600,000 histories), median 600,000, largest 1,140,000.
  At 5 % on this model the minimum of 30 batches, not the target, usually decides when the run stops.
- Sampling took 26–50 s per stopped run and 558 s for the long run; with preparation, seed 501 took 11 minutes
  and the others 62–88 s each.

Deviations: the runs were started by a wrapper script that set the seed, target and history limit and added an
exit-code line to each output file; nothing else differs from the frozen command.

Scope: this result is for the stop rule on the mean, which is what the tested binary applies. A run that also
judges a peak inside a solid stops later, when the highest piece meets the target as well; that rule is not
covered by this test.
