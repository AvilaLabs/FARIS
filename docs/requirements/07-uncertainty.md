# Uncertainty

How FARIS quantifies, propagates and shows what it does not know. Every displayed number carries an uncertainty or a stated reason it has none: transport tallies, derived quantities, the 30-year history and every plant output. Monte Carlo error, nuclear-data error and model-form error are kept apart, and a verdict that depends on a margin uses an upper bound, never the central value. Accuracy against experiment is in [06-accuracy-and-validation](06-accuracy-and-validation.md); performance of the sampling machinery is in [08-performance](08-performance.md). Reference hardware and models are in the [index](README.md#reference-hardware-and-models).

The research uncertainty figures here are context for what FARIS should find, not targets to copy. Nuclear-data uncertainty on a fission PWR (about 440 pcm at beginning of cycle to 510 pcm at end of cycle, one study, one code) says nothing about fusion tritium breeding.

## Monte Carlo statistical error

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| UNC-001 | FARIS shall report a standard error on every transport tally (TBR, heating, flux, reaction rates, spectra, mesh voxels, fluence, dose). | 100 % of tally outputs carry a standard error and the number of independent batches behind it. | Tally schema test; UI audit of every panel. | OpenMC reports standard errors [V: [docs](https://docs.openmc.org)] | F1 | Met: TBR, mesh flux, heating and spectra carry standard errors in the demo (`faris-engine` response records) |
| UNC-002 | FARIS shall verify that the reported 95 % interval covers the truth at the nominal rate over repeated seeded runs. | Empirical coverage 93 % to 97 % for a nominal 95 % interval over ≥ 100 seeded repeats of each analytic control; reported with its own binomial interval. | Replicate-seed test on the analytic suite (VAL-011). | FARIS choice; R6 candidate | F2 | Partial: one 1 M-history pure-absorber control passes the 3 σ rule; no repeated-seed coverage test |
| UNC-003 | FARIS shall use intervals that stay valid at low counts, not a Gaussian assumption. | Coverage ≥ 93 % in synthetic low-count tests (1 to 20 scoring events per bin); bins with fewer than 10 scoring events show a one-sided bound or NOT_EVALUATED. | Synthetic Poisson and heavy-tail tally tests. | R1 candidate N-037; FARIS choice | F2 | No |
| UNC-004 | FARIS shall treat a zero-scoring bin as not evaluated, never as zero. | 0 voxels with score zero shown as zero dose, heating or flux; each is labelled with why and the histories needed for a bound. | Fixture with a deliberately unreached voxel. | R1 trap 4; `docs/OPERATING_HISTORY.md` states a zero tally gives no bound [internal] | F2 | Partial: zero tally accepted but flagged as giving no bound in `docs/OPERATING_HISTORY.md`; no voxel mask |
| UNC-005 | FARIS shall run statistical health checks on every tally and refuse a verdict from a failing tally. | Checks for relative error, variance of the variance, figure-of-merit stability and tally-distribution slope on 100 % of tallies; failing tally is NOT_EVALUATED. Guidance values: relative error < 0.10 (< 0.05 for point detectors), variance of the variance < 0.1, tally slope > 3. | Synthetic bad-tally fixtures (one per check). | MCNP statistical checks [V: [MCNP](https://mcnp.lanl.gov)] | F2 | No |
| UNC-006 | FARIS shall report the figure of merit (1/(σ²·T)) for every tally and its stability. | FOM shown with trend; change < 20 % over the last half of the run, else flagged. | Test with drifting FOM fixture. | MCNP practice [V]; R6 differentiator | F2 | No |
| UNC-007 | FARIS shall run until a target error is reached or a budget is exhausted, and report which one stopped the run. | Trigger on relative error with the stop reason stored in the receipt; defaults 1 % for TBR and 10 % for deep mesh. | Trigger test on a controlled problem. | OpenMC batch triggers [V: [docs](https://docs.openmc.org)] | F2 | No: fixed 1 M histories |
| UNC-008 | FARIS shall estimate error from independent batches and independent seeds as a check on the reported error. | Seed-to-seed spread agrees with the reported standard error within the sampling error of the spread (z < 3) on 100 % of benchmark cases. | Repeated-seed run on the benchmark suite. | `docs/SCIENTIFIC_BASELINE.md` calls for independent seeds as a stability check [internal] | F2 | No |
| UNC-009 | FARIS shall state when a tally is dominated by rare histories (deep shield, magnet region) and give the achieved relative error beside the value. | Magnet-region quantities show relative error; any above 10 % are labelled low-statistics. | UI audit. | Magnet-region average flux was ≈ 49.6 % relative standard error in the 1 M-history reference run [internal: `docs/OPERATING_HISTORY.md`] | F1 | Partial: standard error stored; no low-statistics label |

## Propagation to derived quantities

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| UNC-010 | FARIS shall propagate Monte Carlo error through every derived quantity (fluence, replacement date, tritium inventory, net electricity, dose, margin). | 100 % of derived numbers carry a propagated interval, or a stated reason none exists. | Registry lint: derived quantity without an interval or a reason fails. | Practice in SCALE Sampler [U] | F2 | No: transport standard errors are attached to inputs but not propagated into fluence, trip timing or history outputs (`docs/OPERATING_HISTORY.md`) |
| UNC-011 | FARIS shall keep the covariance between tallies from the same run and use it in sums, ratios and differences. | Covariance (or a bounded estimate) stored per run; differences between runs that share a seed or source use it; absent covariance gives INCONCLUSIVE and never an assumed zero. | Tests with perfectly correlated and independent fixtures. | `docs/OPERATING_HISTORY.md`: shared seeds do not supply a covariance [internal] | F2 | Partial: comparisons treat correlation as unknown and claim no significance |
| UNC-012 | FARIS shall propagate by the cheapest valid method and say which: analytic first-order, sampling, or batch resampling. | Method named per quantity; first-order and sampling agree within 20 % of the interval width on the reference history (provisional: depends on non-linearity; confirm before F4 gate). | Comparison test over the slider box. | FARIS choice | F2 | No |
| UNC-013 | FARIS shall report the sum of tally parts with the correct combined error. | Total of zones equals the global tally within 3 σ; zone error ≤ 2 % for zones above 5 % of the total. | Sum-check test. | R1 candidate N-007 | F2 | No |
| UNC-014 | FARIS shall verify propagation with a synthetic plant whose answer is analytic. | Propagated σ within 5 % of the analytic σ on ≥ 5 chains (sum, product, ratio, threshold crossing, integral). | Analytic propagation tests. | FARIS choice | F2 | No |
| UNC-015 | FARIS shall report probability of crossing a limit (fluence, dose, temperature) from the propagated distribution, not only the central crossing time. | Crossing date shown as P5/P50/P95; replacement schedules use the earlier bound when the risk is a limit exceedance. | Test with a known distribution. | FARIS choice | F4 | No |
| UNC-016 | FARIS shall carry units and kind labels through uncertainty (same unit as the value; kind of the interval matches the least certain input). | 0 intervals without unit; interval kind is the weakest input kind in 100 % of tests. | Unit and kind property tests. | House rule: every number carries a kind label | F1 | Partial: kind labels exist on numbers; not on intervals |

## Nuclear data uncertainty

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| UNC-020 | FARIS shall report a nuclear-data uncertainty on TBR, kept separate from the Monte Carlo error. | Reported with method (sampling or sensitivity), library and covariance source. For the EU DEMO helium-cooled pebble-bed reference the published TBR data uncertainty is 3.2 % (JEFF-3.2), 5.6 % (JEFF-3.3T1) and 8.6 % (TENDL-2014); FARIS' value on that reference falls within 3 % to 9 %. | Comparison with the published reference case. | [EPJ Web Conf. ND2016 09025](https://www.epj-conferences.org/articles/epjconf/abs/2017/15/epjconf-nd2016_09025/epjconf-nd2016_09025.html) [V: abstract level] | F2 | No |
| UNC-021 | FARIS shall report nuclear-data uncertainty on heating, fluence and dose on the magnets. | Reported for the three magnet responses; if covariances are absent the cell says NOT_EVALUATED with the missing nuclide list. | Registry test. | R1 candidate N-053 | F2 | No |
| UNC-022 | FARIS shall flag every nuclide lacking covariance or photon-production data. | 100 % of flagged nuclides listed per run with their share of the response (sensitivity-weighted). | Test with a library lacking a known covariance. | R1 candidate N-054 | F2 | No |
| UNC-023 | FARIS shall show the library-to-library spread (FENDL-3.2b, ENDF/B-VIII.1, JEFF-4.0) as an additional, separate data-uncertainty indicator. | ≥ 2 libraries on every headline response; spread shown beside covariance-based uncertainty, never replacing it. | Cross-library run. | R1 candidate N-006; JEFF-4.0 release details [U] | F2 | No |
| UNC-024 | FARIS shall propagate activation data uncertainty (cross-sections, decay data, pathways) onto activity and decay heat for the top contributing nuclides. | 1-σ on the top 10 nuclides by decay-heat contribution; compared with published cases. | Cross-code check with ACTINV and FISPACT-II. | FISPACT-II uncertainty capability [U] | F2 | No |
| UNC-025 | FARIS shall record the covariance source and processing route in the receipt. | Library, covariance file hash, sampling method, number of samples in 100 % of receipts. | Receipt schema test. | Avila Core receipts [internal] | F2 | No |
| UNC-026 | FARIS shall check sampling convergence for nuclear-data propagation. | Reported uncertainty changes < 10 % when the sample count doubles; otherwise labelled "not converged". | Convergence test. | FARIS choice | F2 | No |
| UNC-027 | FARIS shall state that data uncertainty is an estimate from a library's covariances, which may be incomplete, and never present it as a total error. | Fixed wording in 100 % of data-uncertainty displays; "excludes: <list>" always shown. | UI text lint. | Honest-unknown rule [internal] | F2 | No |

## Sensitivities

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| UNC-030 | FARIS shall compute adjoint or perturbation sensitivities of TBR and magnet heating to material composition, density and geometry. | Sensitivities for the 10 most influential parameters per response. | Test suite on a controlled problem. | TSUNAMI-style sensitivity [U]; R1 candidate N-063 | F2 | No |
| UNC-031 | FARIS shall verify sensitivities against central finite differences. | Agreement within 2 combined σ on ≥ 10 test parameters; step size shown and checked (halving the step changes the finite difference by < 10 %). | CI test. | R6 candidate | F2 | No |
| UNC-032 | FARIS shall verify adjoint reciprocity on a simple problem. | Forward and adjoint responses agree within 3 σ on 3 analytic problems. | Reciprocity test. | R1 candidate N-076 | F2 | No |
| UNC-033 | FARIS shall show a ranked sensitivity list (tornado) beside each headline result with intervals. | 100 % of headline results link to a ranked list; every bar has an interval and a unit. | UI audit. | FARIS choice | F2 | No |
| UNC-034 | FARIS shall report which plant-chain inputs drive each plant output (replacement interval, net electricity, tritium inventory). | Top 5 inputs per output with rank stable over bootstrap (Spearman ≥ 0.8 between halves). | Stability test. | FARIS choice | F4 | No |
| UNC-035 | FARIS shall check the top-5 ranking against a 20 % input perturbation and report whether it changes. | Rank change flagged; test repeated on every release. | Perturbation test. | R6 candidate (asset criticality) | F4 | No |

## Global sensitivity

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| UNC-040 | FARIS shall compute Sobol indices (first order and total) over authored assumptions and geometry. | Indices with a bootstrap 95 % interval from ≥ 1,000 resamples; method and sample count in the receipt. | Analytic test functions (Ishigami and a product function). | optiSLang-style sensitivity [U] | F6 | No |
| UNC-041 | Sobol estimates shall pass an additivity and convergence check. | On an additive test the first-order indices sum to 1 ± 0.05; indices change < 0.05 when the sample doubles. | Analytic test. | R6 candidate | F6 | No |
| UNC-042 | FARIS shall reuse stored transport so global sensitivity does not rerun Monte Carlo. | 10,000 history evaluations over the fast tier in ≤ 100 s on RL, at the PERF-020 rate; any input needing new transport is excluded or costs a stated number of runs. | Benchmark. | PERF-020, PERF-021 | F6 | No |
| UNC-043 | FARIS shall refuse to give a global sensitivity ranking for inputs outside the qualified range. | 100 % of such inputs are listed as NOT_EVALUATED with the range (VAL-070). | Out-of-range fuzz. | Fail-closed house rule | F6 | No |

## Epistemic and aleatory separation, margins and verdicts

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| UNC-050 | FARIS shall separate aleatory uncertainty (random sampling, random failures) from epistemic uncertainty (data, model form, authored assumptions) in every total. | Totals show at least three parts: Monte Carlo, data, and model-form or authored; each can be hidden but never merged unseen. | UI and export test. | R1 candidate N-064; Oberkampf and Roy [U] | F2 | No |
| UNC-051 | FARIS shall carry authored assumptions as intervals or sets, not point values, when they drive a verdict. | 100 % of verdict-driving authored inputs have a stated range and source; a point value shows "no range given" and cannot give PASS. | Verdict checker test. | Fail-closed house rule | F2 | Partial: authored assumptions are labelled; no ranges |
| UNC-052 | FARIS shall give margin verdicts on the 2-sigma upper bound including data uncertainty. | PASS only if the 2 σ upper bound (Monte Carlo plus data) is below the limit; otherwise INCONCLUSIVE or FAIL; central value alone gives no verdict. | Edge-value unit tests (value at limit, bound at limit). | R1 candidate N-015 | F2 | Partial: compare view applies a 2-sigma resolution test (`difference_resolved_2sigma`); no upper-bound verdicts |
| UNC-053 | FARIS shall mark verdicts "conditional" when they hold only under authored assumptions. | 100 % of conditional verdicts list each assumption, its source and the change that would flip the verdict. | Verdict fixture test. | House rule: kind labels | F2 | Partial: HorizonCompleted is distinguished from PASS; no flip analysis |
| UNC-054 | FARIS shall mark a verdict NOT_EVALUATED when the evidence is unconverged, stale or out of range, and explain why and what to do next. | 100 % of NOT_EVALUATED have a reason code and a next step as text and on hover or tap. | UI test. | House rule | F1 | Partial: not-evaluated states exist; explanations incomplete |
| UNC-055 | FARIS shall show the margin to each limit with its interval and the probability that the limit is met. | Margin and exceedance probability for 100 % of limit checks; limits carry source and conditions (for example REBCO fluence limits depend on temperature). | Limit-table lint. | R1 candidate N-014; REBCO limits depend on temperature [V: [Fischer 2018](https://doi.org/10.1088/1361-6668/aaadf2)] | F3 | No |
| UNC-056 | FARIS shall add a model-form allowance for idealised geometry and homogenised blankets, as an authored interval, until a reference delta exists. | Allowance shown as its own bar with source "authored"; replaced by a calculated delta when VAL-076 evidence exists. | UI test. | R1 trap 6: 1D can overshoot TBR 5–10 % [U] | F2 | No |

## Plant chain, P5/P50/P95

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| UNC-060 | FARIS shall propagate uncertainty through the whole plant chain (transport, activation, tritium, availability, power balance, cost) and show P5, P50 and P95 for each output. | 100 % of plant outputs show the three percentiles or a stated reason not. | Registry lint plus UI audit. | Sampling practice [U]; FARIS choice | F4 | No |
| UNC-061 | FARIS shall draw correlated samples when inputs share a source (for example one transport run feeding many components). | Correlation structure stored; independence never assumed without a note; test against a perfectly correlated fixture. | Fixture test. | `docs/OPERATING_HISTORY.md` [internal] | F4 | No |
| UNC-062 | FARIS shall report availability, mean time between failures and capacity factor with confidence intervals from stochastic maintenance histories. | 95 % interval from ≥ 1,000 stochastic histories in < 10 s on RL; analytic exponential-failure cases within 2 %. | Analytic reference cases. | R6 candidate (RAMI) | F4 | No |
| UNC-063 | FARIS shall report the width of its percentiles with a sampling error of its own. | Percentile standard error from bootstrap shown; P5 and P95 need ≥ 1,000 samples or are labelled coarse. | Test with synthetic distributions. | FARIS choice | F4 | No |
| UNC-064 | FARIS shall keep the 30-year history's displayed intervals current as sliders move. | Interval updates with the history at the PERF-020 rate (≤ 100 ms P95 for history, ≤ 1 s for coupled plant). | Latency benchmark. | PERF-020 | F4 | No |
| UNC-065 | FARIS shall report stored-run uncertainty and plant-chain uncertainty as separate layers. | Two views: "from transport only" and "plus authored assumptions"; both exportable. | UI test. | FARIS choice | F4 | No |

## Surrogates

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| UNC-070 | FARIS shall report a held-out error bound for every surrogate or emulator, with its sample size. | Error shown on 100 % of surrogate outputs; hold-out set ≥ 20 % of samples and never used in fitting. | Hold-out audit script. | R6 candidate (DesignXplorer-style) | F6 | No |
| UNC-071 | FARIS shall refuse to optimise on a surrogate whose error exceeds the user's tolerance. | Default tolerance 2 % (provisional: tool defaults vary; confirm before F6 gate); refusal message names the error and how to reduce it. | Test with a deliberately poor surrogate. | R6 trap: optimising a surrogate artefact | F6 | No |
| UNC-072 | FARIS shall verify the optimum found on a surrogate by a full recalculation before reporting it. | 100 % of reported optima rechecked at the full tier; difference shown. | Optimiser test. | R6 trap | F6 | No |
| UNC-073 | FARIS shall flag surrogate outputs outside the training range. | 100 % flagged NOT_EVALUATED with why and next step. | Range fuzz. | Fail-closed house rule | F6 | No |
| UNC-074 | FARIS shall add the surrogate error to the displayed uncertainty in quadrature, with its own part visible. | Surrogate part listed as an item in the uncertainty breakdown. | UI test. | FARIS choice | F6 | No |

## Display rules

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| UNC-080 | FARIS shall show no number without an uncertainty or a reason it has none. | 100 % of numbers on screen, in the PDF brief, CSV and charts carry an interval, or "no uncertainty, because ... (next step: ...)". | UI audit script over every panel and export. | Unknown must explain itself [internal]; R1 candidate N-083 | F1 | Partial: standard errors shown on transport results and compare flags; plant outputs and history have none |
| UNC-081 | FARIS shall set significant figures from the uncertainty. | The value is rounded so the last shown digit is the first or second digit of the 1-σ uncertainty; uncertainty shown to 1–2 significant digits; no digit beyond it. | Formatter unit tests on 1,000 random value and uncertainty pairs plus golden screenshots. | JCGM GUM reporting practice [U] | F1 | No |
| UNC-082 | FARIS shall state the interval convention (1 σ, 2 σ, 95 % interval, percentile) beside every interval. | Convention visible without hover in 100 % of displays; never mixed in one table without a column label. | UI audit. | FARIS choice | F1 | No |
| UNC-083 | FARIS shall draw error bars, intervals or bands on every chart, and mesh error maps beside every field. | 100 % of numeric charts carry intervals; field views have a relative-error layer; voxels with error > 10 % are hatched. | Golden screenshot tests. | R1 candidate N-021 | F2 | Partial: compare view flags at 2 σ; no field error layer |
| UNC-084 | FARIS shall not rank or colour two results as different unless the difference is resolved by its uncertainty. | Compare view flags a difference only when the combined interval excludes zero; unresolved cases say "within 2σ sampling noise". | Unit tests (extends `difference_resolved_2sigma`). | Compare view 2-σ flag [internal] | F1 | Met: two-sigma resolution function with tests in `comparison.rs` (limits: no covariance) |
| UNC-085 | FARIS shall show the uncertainty breakdown on hover or tap and as text. | Breakdown lists Monte Carlo, data, model-form, authored and surrogate parts with their share of variance. | UI test. | House rule | F2 | No |
| UNC-086 | FARIS shall never display an interval narrower than the evidence supports: when a part is not evaluated, the display says "excludes: <part>". | 100 % of partial uncertainties list excluded parts; fixture with a missing data part must show the notice. | UI test. | R5 VV-12 candidate | F1 | No |
| UNC-087 | FARIS shall give each plot and table an accessible text form of its uncertainty. | Text equivalent for 100 % of intervals (screen-reader readable). | Accessibility audit (A11Y). | WCAG 2.2 principle [U] | F2 | No |
| UNC-088 | FARIS shall carry uncertainty intact through CSV, PDF and API exports. | Every exported number has a paired uncertainty column or field; round-trip changes it by ≤ 1 ulp. | Export golden and round-trip test. | FARIS choice | F1 | Partial: CSV exports standard errors where stored |

## Traps

- **Relative error is precision, not accuracy.** A 0.2 % standard error on a leaky model is a confident wrong number (UNC-005, UNC-056).
- **Mean-only intervals.** A symmetric 2 σ bar on a skewed quantity (fluence limit crossing, availability) misleads; use percentiles or non-parametric bounds (UNC-015, UNC-060).
- **Independence assumed by default.** Differences between runs that share a seed, and sums of tallies from one run, are correlated. Treating them as independent overstates significance (UNC-011).
- **Coverage of the wrong interval.** A coverage test on a well-behaved control (high counts) does not show coverage in deep shield voxels; test low-count bins as well (UNC-002, UNC-003).
- **Data uncertainty from covariances is a floor.** Missing covariances count as zero unless flagged. A small data uncertainty may only mean a thin library (UNC-022, UNC-027).
- **Library spread is not uncertainty.** Two libraries can agree and both be wrong; spread is an indicator only (UNC-023).
- **False precision from tidy numbers.** Showing 5 digits because the value has 5 digits. Digits follow the uncertainty (UNC-081).
- **A surrogate with a good average error.** The optimiser lives at the worst point; use worst-case hold-out error and verify the optimum (UNC-070, UNC-072).
- **Fission uncertainty magnitudes as expectations.** The PWR figures of 440–510 pcm for data are one study; they say nothing about what fusion tritium breeding or shutdown dose uncertainty should be.
