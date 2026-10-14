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
