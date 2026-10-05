# Design workflows

How FARIS turns one study into many: parameter studies, designs of experiments, surrogate models, optimisation, constraints, sensitivity ranking, comparison of candidates, trade-study records and the traceability from a user's design requirements to checker verdicts with margins. The workflows must stay honest when the underlying numbers are noisy Monte Carlo results: every claimed improvement is tested against sampling error, and every surrogate is checked on points it has not seen. Timings are on the reference laptop (RL) and workstation (RW); see [the index](README.md#reference-hardware-and-models). Parallel-run throughput is in PERF-034 and caching in AUTO-030 to AUTO-039; uncertainty propagation and global sensitivity of the physics itself are in 07-uncertainty (UNC) and are only linked here. Today FARIS has a 7-point blanket and shield allocation sweep with a compare view that flags differences against 2σ, and a 27-point authored history sensitivity grid (bounded at 64 reruns) run from the CLI.

## Parameter studies and sampling

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| DSN-001 | FARIS shall run a study over any registered numeric parameter or set of parameters. | ≥ 10 parameters selectable per study from the parameter registry; each carries unit, range (CFG-004) and cost class (PERF-021); selecting a parameter outside its range is refused. | Study-definition test over the registry. | Dakota, Workbench parameter manager [U] | F6 | Partial: one parameter (blanket allocation) swept in the app; history grid via CLI file |
| DSN-002 | FARIS shall offer full-factorial, Latin hypercube, Sobol or Halton quasi-random and user-supplied designs. | ≥ 4 methods; discrepancy (star or centred) reported for each design; values match scipy.stats.qmc reference tables to 1e-12 for fixed seeds. | Unit tests against scipy reference values. | Dakota, scipy.stats.qmc [U] | F6 | No |
| DSN-003 | FARIS shall estimate the cost of a design before it is run. | Case count, cached case count, predicted time (PERF-026) and memory shown before start; Saltelli designs show N(2D+2) cases and Morris r(D+1), for example D = 10, N = 1,024 shows 22,528. | Cost-preview test against the formulas. | SALib Saltelli sampler N(2D+2) [V]: [SALib basics](https://salib.readthedocs.io/en/latest/user_guide/basics.html); Morris rows (G/D+1)N/T [V]: [SALib Morris](https://salib.readthedocs.io/en/latest/api/SALib.sample.morris.html) | F6 | No |
| DSN-004 | FARIS shall refuse studies that cannot finish within stated resources. | A design projected to exceed the time budget, memory (PERF-041) or the 64-rerun history limit without an explicit raise is refused with the numbers and an alternative. | Admission test. | Fail-closed house rule; history grid bound of 64 reruns (docs/OPERATING_HISTORY.md) | F6 | Partial: history sensitivity bounds the grid at 64 reruns |
| DSN-005 | Every design shall be reproducible from a seed. | Same seed and settings give the same design points on 100 % of 20 runs, on two architectures; the seed is recorded in the study. | Reproduction test. | FARIS choice | F6 | No |
| DSN-006 | Parameters shall be able to include discrete choices such as arrangement or material. | Categorical and integer parameters supported by factorial and random designs; labelled in tables and charts. | Mixed-parameter test. | Dakota, Optuna [U] | F6 | Partial: four arrangements exist as separate cases; not a swept parameter |
| DSN-007 | A study shall be extendable by adding points without redoing existing ones. | Adding N points runs exactly N new cases; 100 % of prior points reused through the cache (AUTO-030). | Extension test. | Optuna storage, Dakota restart [U] | F6 | No |
| DSN-008 | A study shall be pausable, resumable and killable at any point. | Pause and resume loses ≤ 1 case; a kill resumes per AUTO-050 and AUTO-051. | Kill-and-resume test. | R6 sweep row | F6 | No |
| DSN-009 | A study shall record which cases failed, why and what to do. | 100 % of failures listed with cause and remedy; failed cases are excluded from statistics with a visible count, never dropped silently. | Fault-injection test (AUTO-044). | Fail-closed house rule | F6 | No |

## Surrogate models

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| DSN-010 | FARIS shall build response-surface surrogates from study results. | ≥ 2 families (polynomial or polynomial chaos, Gaussian process) with a recorded training set, seed and settings. | Build test on analytic functions. | Dakota, SMT, DesignXplorer [U] | F6 | No |
| DSN-011 | FARIS shall report surrogate error on held-out data. | Held-out RMSE and Q² on points never used in training, k-fold or leave-one-out; training error is never shown as accuracy; ≥ 20 % of points held out or a cross-validation of ≥ 5 folds. | Test with a deliberately overfit surrogate: the report flags it. | Q² ≥ 0.9 is a heuristic [U]; Trap: training-set accuracy is meaningless [R4] | F6 | No |
| DSN-012 | FARIS shall refuse surrogate use above a stated error tolerance. | Default tolerance 2 % of the output range (provisional: from analogue practice, not a published standard; confirm before F6 gate); above it the surrogate is shown but optimisation on it is blocked. | Threshold test. | HEEDS and DesignXplorer practice [U]; FARIS choice | F6 | No |
| DSN-013 | FARIS shall flag extrapolation. | Any request outside the convex hull or training range of the surrogate is marked "extrapolation" and refused for verdicts; 100 % of 200 outside points flagged. | Boundary test. | FARIS choice | F6 | No |
| DSN-014 | A surrogate optimum shall be verified on the true model before it is labelled calculated. | 100 % of surrogate-found optima carry "surrogate estimate" until a full run confirms within the stated tolerance; the confirmation run is linked. | Workflow test. | Trap: optimisers exploit surrogate errors [R4] | F6 | No |
| DSN-015 | Every evaluation shall be tagged as true model or surrogate. | 100 % of points in tables, charts and exports carry the tag. | Export audit. | R4 DSN-9 | F6 | No |
| DSN-016 | Surrogates shall carry predictive uncertainty where the family allows. | Gaussian process variance shown as bands; coverage of the 95 % band on held-out points between 90 % and 99 % on 3 test functions. | Coverage test. | Gaussian process practice [U] | F6 | No |
| DSN-017 | Surrogates shall use the Monte Carlo standard errors of their training points. | Training weights use reported σ; a surrogate trained on noisy points never shows a fit smoother than the noise supports (residual test). | Noise-injection test. | Noisy GP practice [U] | F6 | No |
| DSN-018 | Adaptive refinement shall add points where error is largest until the tolerance is met or the budget ends. | On a 3-parameter analytic test, held-out error falls below tolerance in ≤ 40 points; stops cleanly at the budget with the error stated. | Benchmark test. | Adaptive sampling practice [U] | F6 | No |
| DSN-019 | A surrogate shall be saved with its provenance. | Training points, seed, settings, held-out error and FARIS version stored in the study; reloaded surrogate reproduces predictions to 1e-12. | Save and reload test. | Provenance rule | F6 | No |

## Optimisation

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| DSN-020 | FARIS shall include a gradient-free single-objective optimiser. | Converges on Rosenbrock and Branin to within 1e-3 of the known optimum within the stated budget on 20 seeds, 20 of 20. | Analytic benchmark tests. | Branin and Rosenbrock are standard test functions; targets FARIS choice | F6 | No |
| DSN-021 | FARIS shall include a multi-objective optimiser of the NSGA-II class. | On ZDT1, ZDT2 and ZDT3 with 10,000 evaluations, hypervolume within 1 % of the reference front's hypervolume (provisional: tolerance and reference point to be set from a baseline run; confirm before F6 gate). | ZDT benchmark with fixed reference point. | NSGA-II (Deb) and ZDT suite [U] | F6 | No |
| DSN-022 | FARIS shall include a constrained local optimiser. | Solves a standard constrained test set (≥ 5 problems) to within 1e-6 of the known optimum with constraints satisfied. | Test set. | VMCON in PROCESS is the fusion precedent [V that PROCESS uses VMCON]: [PROCESS](https://ukaea.github.io/PROCESS/) | F6 | No |
| DSN-023 | Optimisers shall use the standard errors of noisy objectives. | An optimiser refuses to call a candidate better when the difference is under 2σ of the combined errors; on injected-noise tests, 0 of 100 false improvements are reported. | Noise-injection test. | Existing 2σ flag in the compare view; noisy GP practice [U]; Trap: winner's curse [R4] | F6 | Partial: the compare view flags differences against 2σ; optimisers do not exist |
| DSN-024 | Hypervolume results shall use a recorded, fixed reference point. | The reference point is stored with every hypervolume; two runs with different reference points are not compared by the UI. | Compare test. | Trap: hypervolume comparisons need a fixed reference [R4] | F6 | No |
| DSN-025 | Every optimiser run shall be reproducible. | Same seed, settings and code give identical populations on 20 of 20 runs for deterministic objectives. | Reproduction test. | FARIS choice | F6 | No |
| DSN-026 | Optimisers shall be stoppable on budget, time, convergence or a user's cancel, with the best-so-far result saved. | 100 % of stop modes tested; cancel keeps all evaluated points; stop reason recorded. | Stop test. | PERF-024 | F6 | No |
| DSN-027 | FARIS shall expose objective and constraint evaluation to external optimisers. | Tested examples for ≥ 2 external frameworks (pymoo, Optuna first); evaluation through the API with cache reuse (AUTO-030). | CI examples. | pymoo, Optuna, Dakota, OpenMDAO [U] | F6 | No |
| DSN-028 | FARIS shall support staged fidelity: cheap screening, then full confirmation. | A documented workflow promotes the top k candidates from preview runs (PERF-025) to production runs; promoted candidates carry both results and the screening error. | Workflow example in CI. | R4 multi-fidelity row | F6 | No |
| DSN-029 | Optimiser settings shall be exposed with safe defaults and the effect of each stated. | 100 % of optimiser settings in the registry with defaults, ranges and effect lines (CFG-004, CFG-051). | Registry lint. | House rule: no hidden configuration | F6 | No |

## Constraints and feasibility

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| DSN-030 | Constraints shall be first-class study objects. | A constraint names a quantity, bound, unit, kind label and source; examples: tritium breeding ratio ≥ a threshold, component fluence limit, net electricity ≥ 0; stored in the study. | Study round-trip test. | PROCESS constraint equations [U] | F6 | Partial: demo checks and thresholds exist per scenario; no user-defined constraints |
| DSN-031 | Every constraint shall show its margin and the margin's uncertainty. | 100 % of constraint results show margin with unit, standard error where available, and kind label. | UI and export audit. | House rule | F6 | No |
| DSN-032 | FARIS shall judge feasibility on the safe side of uncertainty. | Option "feasible only if the 2σ bound passes"; default on for decisions and recorded in the result; a candidate within 2σ of a limit is "inconclusive", never "pass". | Boundary test with injected noise. | Fail-closed house rule; PASS, FAIL, INCONCLUSIVE, NOT_EVALUATED (AGENTS.md) | F6 | No |
| DSN-033 | FARIS shall show the active set: which constraints bind at the selected candidate. | Binding constraints listed with margin ≤ stated tolerance; ordered by margin; 100 % correct on 10 analytic problems. | Active-set test. | PROCESS reports active constraints [U] | F6 | No |
| DSN-034 | Constraint handling in optimisers shall be documented and testable. | Feasibility-first (Deb rules) or penalty method named per optimiser; a test with an infeasible start reaches feasibility on 20 of 20 runs. | Test. | Deb constraint handling [U] | F6 | No |
| DSN-035 | A constraint missing evidence shall block the verdict. | Missing, stale or out-of-range evidence gives NOT_EVALUATED with the reason and next step; 0 passes without evidence in a 100-case test. | Fail-closed test. | Fail-closed house rule | F1 | Partial: demo shows not-evaluated states for absent physics; not general |
| DSN-036 | Infeasible regions shall be shown. | Design-space plots shade infeasible regions per constraint and mark the surrogate-versus-true distinction. | UI snapshot. | FARIS choice | F6 | No |

## Sensitivity ranking

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| DSN-040 | FARIS shall rank parameters by their effect on any output. | One-at-a-time, Morris screening and Sobol first and total order indices offered; ranking shown with confidence intervals. | Analytic test: Ishigami. | SALib Saltelli and Morris [V]: [SALib basics](https://salib.readthedocs.io/en/latest/user_guide/basics.html) | F6 | Partial: a 27-point grid sensitivity of the 30-year history exists (CLI `history sensitivity`); no ranking, no indices |
| DSN-041 | Sobol indices shall be accurate on a known function. | On Ishigami (a = 7, b = 0.1) first and total indices within 0.02 of the analytic values at N = 2¹⁴ (provisional: recalled analytic values; confirm before F6 gate). | Analytic benchmark. | Ishigami analytic values [U]; Saltelli et al. 2010 [U] | F6 | No |
| DSN-042 | Sensitivity indices shall come with bootstrap intervals and shall refuse over-reading. | 95 % intervals always shown; a warning when interval width exceeds 0.1; ranking suppressed when intervals overlap fully. | Test with small N. | Trap: Sobol with small N or correlated inputs is unstable [R4] | F6 | No |
| DSN-043 | FARIS shall detect when inputs are correlated and refuse variance-based indices. | Correlation above a stated threshold (provisional: 0.3; confirm before F6 gate) blocks Sobol indices with an explanation and an alternative. | Correlated-input test. | R4 trap | F6 | No |
| DSN-044 | Sensitivity shall cover authored assumptions as well as geometry. | Authored assumptions (recovery, delay, limits, availability) selectable alongside geometry; each result labelled conditional on authored values, not a probability. | Study test. | OPERATING_HISTORY.md: sensitivity is conditional; R6 sweeps row | F4 | Partial: three authored assumptions varied in the history grid, labelled conditional |
| DSN-045 | FARIS shall state when a sensitivity is zero because a threshold was never reached. | Parameters with no effect carry the reason, for example "service threshold never reached because fuel ran out first"; ≥ 90 % of zero-effect results explained in a review of 20 cases. | Explanation audit. | OPERATING_HISTORY.md service-threshold finding | F4 | Partial: documented in the research note, not in the app |
| DSN-046 | Link to uncertainty propagation. | Sensitivity ranking and the uncertainty budget in 07-uncertainty (UNC) use the same sampling and the same cache. | Shared-run test. | FARIS choice | F6 | No |
| DSN-047 | Convergence of every Monte Carlo sweep output shall be shown. | Running mean and standard error plotted as cases are added, for 100 % of Monte Carlo outputs; the plot states whether N is enough by the stopping rule. | UI test. | R4 DSN-13 | F6 | No |

## Candidate comparison and trade studies

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| DSN-050 | FARIS shall compare any two or more candidates side by side. | Table and chart for ≥ 2 and ≤ 12 candidates; every difference shows size, combined σ and whether it clears 2σ; "not resolved" never reads as "equal". | Compare test with injected noise. | Existing compare view with 2σ flags | F1 | Met: compare view flags differences against 2·√(SE₁²+SE₂²) (crates/faris-engine/src/brief.rs, comparison.rs) for the 4 arrangements and the sweep |
| DSN-051 | FARIS shall support paired comparison with common seeds. | Option to share seeds; shared-seed differences use the paired variance estimate; the note says whether covariance is modelled. | Paired-variance test. | brief.rs states covariance is not modelled for distinct seeds | F6 | Partial: compare uses distinct seeds, covariance not modelled (stated in the brief) |
| DSN-052 | A sweep shall note multiple comparisons. | With k > 2 candidates the report states the number of pairs and the false-flag rate expected at 2σ; flag threshold adjustable. | Report test. | FARIS choice | F6 | No |
| DSN-053 | FARIS shall keep trade-study records. | A record has question, candidates, criteria, weights where used, results with kind labels, decision, author, date and evidence hash; stored in the study; 100 % of fields required. | Schema test. | Systems-engineering trade-study practice [U] | F6 | No |
| DSN-054 | A trade-study decision shall never be a hand-edited verdict. | The record's verdict fields are checker outputs; the human adds a decision and a reason as separate fields, and any override is logged. | Edit-block test. | Verdicts are derived by checkers (house rule) | F6 | No |
| DSN-055 | Weighted ranking shall show weight sensitivity. | A ranking with weights shows how many weight changes of ±20 % flip the winner; shown as a robustness count. | Test on 3 fixtures. | Multi-criteria decision practice [U] | F6 | No |
| DSN-056 | FARIS shall present the Pareto front and its dominated and feasible status. | Front exported as CSV and chart with feasible, infeasible and dominated marked; hypervolume with its reference point (DSN-024). | Export test. | pymoo [U] | F6 | No |
| DSN-057 | A candidate shall be able to be pinned and carried between studies. | Pinned candidates keep inputs, results and hashes; reopening shows them unchanged or marks them stale with the cause. | Pin test. | FARIS choice | F6 | No |

## Execution of sweeps

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| DSN-060 | Study cases shall run in parallel under the queue. | Efficiency and overhead per PERF-034; default concurrent transport jobs per PERF-043 and CFG-058. | PERF-034 tests. | PERF-034 | F6 | No |
| DSN-061 | Cases already cached shall not rerun. | Study over a cached design finishes in ≤ 1 s per 100 cases with 0 transport runs; 0 false hits per AUTO-033. | Re-run test. | PERF-027 | F1 | Partial: recorded sweep bundles are reused; no cache |
| DSN-062 | A study shall show progress and a time estimate. | Per-case state and total estimate with absolute error ≤ 30 % after 20 % of cases (PERF-022). | Timing test. | PERF-022 | F6 | No |
| DSN-063 | A study shall be runnable from CLI, Python and service as from the app. | Same study definition file runs in all three with identical result hashes for deterministic parts (AUTO-002). | Parity test. | AUTO-002 | F6 | Partial: the history sensitivity grid runs from the CLI; the transport sweep does not |
| DSN-064 | The sweep shall tell the user which edits would need new transport. | Cost classes (PERF-021) shown per swept parameter; a study mixing "instant" and "transport run" parameters shows both counts. | UI test. | PERF-021 | F1 | No |

## Requirements and traceability

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| DSN-070 | A user shall be able to state a design requirement as a checkable constraint. | A requirement has id, text, quantity, comparison, limit, unit, scope and source; stored in the study and exchanged through INT-060 and INT-062. | Round-trip test. | SysML v2 requirements as constraints [U] | F6 | No |
| DSN-071 | Each requirement shall be judged by a checker, giving PASS, FAIL, INCONCLUSIVE or NOT_EVALUATED with a margin. | Margin, uncertainty, evidence hashes and checker version recorded; 100 % of 50 test requirements give the expected verdict, including all four states. | Checker test suite with injected evidence. | PASS, FAIL, INCONCLUSIVE, NOT_EVALUATED preserved with scope (AGENTS.md) | F6 | Partial: Core compiles and checks study-level properties and records receipts; no user-authored requirements |
| DSN-072 | Verdicts shall never be edited by hand. | 0 editable verdict fields in UI, file and API; a hand edit of a saved file is detected on load (hash mismatch). | Tamper test. | House rule | F6 | Partial: receipts bind verdicts to hashes; no user-facing editing exists |
| DSN-073 | Traceability shall run in both directions. | `faris trace <requirement>` lists the results and evidence behind its verdict; `faris trace <result>` lists the requirements it affects; complete for 100 % of requirements in the demo requirement set. | Trace test. | AiiDA-style ancestry [U]; SIMULIA traceable requirement [U] | F6 | No |
| DSN-074 | A requirement shall go stale when its evidence changes. | Edit of any upstream input, setting or data library flips the verdict to NOT_EVALUATED until re-run in 100 % of 50 mutation cases. | Mutation test (AUTO-033 key components). | Fail-closed house rule | F6 | No |
| DSN-075 | Requirements shall be grouped, versioned and diffed. | Requirement set has a version and a diff view (CFG-081 style); changes recorded with date and author. | Diff test. | FARIS choice | F6 | No |
| DSN-076 | A requirement-set summary shall be exportable as a report. | PDF and CSV list each requirement, verdict, margin, uncertainty, evidence hash; stamped with the study hash; statement that outputs are research screening and not a licensing or safety claim. | Export test. | House rule | F6 | No |
| DSN-077 | A requirement shall state its scope. | Verdict names what it covers (arrangement, case, assumptions) and what it does not; a verdict never extends beyond its evidence. | Scope text test. | AGENTS: preserve verdicts with scope | F6 | Partial: demo verdicts carry scope text |

## Visualisation and recommendations

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| DSN-080 | FARIS shall visualise the design space. | Parallel-coordinates, scatter-matrix, response curves for ≥ 2 parameters, Pareto plot and constraint shading; 100 % linked to the case table and 3D view; selection response ≤ 100 ms (PERF-016). | UI test and timing. | ParaView and DesignXplorer practice [U] | F6 | Partial: 7-point sweep chart and table in the app; none of the others |
| DSN-081 | Plots shall show uncertainty and kind labels for every point. | Error bars or bands on 100 % of Monte Carlo points; point shape or colour encodes true model versus surrogate and conditional versus calculated. | Snapshot test. | House rules; see 11-visualization | F6 | Partial: sweep shows error bars and kind labels |
| DSN-082 | FARIS shall explain a recommendation in plain language. | Each recommendation says what was compared, what drove the difference, its size against 2σ, which assumptions it depends on and what would change it; reading level stated in the test. | Explanation-content test with a required-items checklist. | Existing plain-language sweep findings (sweep.rs) | F6 | Partial: transport and history findings are generated in plain language for the sweep |
| DSN-083 | A recommendation shall be withheld when the data cannot support it. | When differences do not clear 2σ or evidence is missing, the message is "not resolved" or "not evaluated" with why and the next step, never a winner. | Fixture with noise-level differences. | Fail-closed house rule; Unknown must explain itself | F1 | Met: sweep findings use a resolution test between sampled values and state when points are not resolved (`difference_resolved`, crates/faris-engine/src/sweep.rs) |
| DSN-084 | A recommendation shall list the assumptions it is conditional on. | 100 % of recommendations list the authored assumptions they depend on, with their kind labels; changing one marks the recommendation stale. | Staleness test. | OPERATING_HISTORY.md conditional results | F4 | Partial: conditional wording present; no staleness link |
| DSN-085 | FARIS shall not call any output a design approval. | 0 occurrences of licensing, safety or approval language in generated recommendations (lint); research-screening statement present. | Text lint. | House rule | F1 | Partial: statement present in the brief; no lint over sweep text |

## Study templates

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| DSN-090 | FARIS shall ship study templates for common questions. | ≥ 5 templates at F6 (blanket allocation sweep, breeding margin scan, replacement interval study, shield thickness trade, assumption sensitivity); each validates and runs in CI. | Template CI. | R4 CFG-11 | F6 | Partial: the 7-point allocation sweep is one fixed template; no template system |
| DSN-091 | A template shall state its question, parameters, cost and limits. | Cost class, expected time on RL, default sample size, validity range and known limits on the template page. | Template lint. | PERF-026 | F6 | No |
| DSN-092 | Users shall be able to save a study as a template. | Template saved from any study in ≤ 5 steps; parameters named; shareable as one file; loads on a clean install (AUTO-025). | Round-trip test. | FARIS choice | F6 | No |
| DSN-093 | A template shall pin the versions it was validated with. | Template names FARIS, adapter and data library versions; a mismatch shows a warning and requires a re-run before any verdict. | Version-gate test. | Fail-closed house rule | F6 | No |

## Traps

- Surrogate accuracy measured on training points, or on points from the same sampling plan, says nothing. Only held-out error counts, and an optimum found on a surrogate must be confirmed on the true model (DSN-011, DSN-014).
- Optimising noisy Monte Carlo results without the standard errors finds noise. A reported improvement under 2σ is not an improvement.
- Hypervolume is only comparable with the same reference point; a "better front" with a different reference is a different measurement.
- Sobol indices with small samples or correlated inputs are unstable. Intervals and a refusal rule come before any ranking.
- A sweep that shows the winner and hides which differences were not resolved is a recommendation the data cannot support. Not resolved is a result.
- Cases per hour depends on cache hits. Report cold and warm figures separately (see PERF-034).
- "Feasible" at the mean value of a noisy constraint is not feasible. Use the safe side of the uncertainty or call it inconclusive.
- Sensitivity of an authored assumption is conditional on the authored values around it. It is not a probability or a likely range.
- A requirement verdict that nobody can trace to evidence is decoration. Traceability is measured by replay: change an input and the verdict must go stale.
