# Operation and life

How FARIS models a plant over its life: operating history, pulsed and steady duty, fatigue, storage, dispatch, startup, schedules, maintenance, replacement, reliability and availability, asset criticality, and the use of operating records. Reference hardware and models (RL, RW, RM-S, RM-M, RM-L) are defined in the [index](README.md#reference-hardware-and-models). Recalculation speed for coupled histories is PERF-020 and is not restated here. Benchmark accuracy is decided in the validation file and uncertainty in the uncertainty file; this file describes the capability. Shutdown dose rates come from the radiation-transport file; this file only requires the link to maintenance work.

Today FARIS has a deterministic 30-year ledger: authored power periods, planned outages, fluence-limited replacement with named outage durations, permanent limits that stop operation, delayed tritium processing, and a 27-case sensitivity grid. It has no stochastic failure model, no pulsed cycle model, no dispatch and no data ingest. Rows say exactly what exists. FARIS is a design model, not a digital twin, until a live plant data path exists.

## Plant-life history and duty cycle

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| OPS-001 | FARIS shall simulate a multi-year operating history with component replacement, outages and permanent limits from one authored schedule. | 30-year history for every arrangement with event list; replacement resets only the named component; site inventories and permanent limits persist. | Regression on the existing controls and event-assumption scenarios. | Operating-history ledger [internal] | F1 | Met: continuous ledger with replacement, outages, permanent stop (OPERATING_HISTORY.md) |
| OPS-002 | FARIS shall refine its time stepping until displayed outputs stop changing and show the gate result. | Endpoint inventories, fluence, event timing and electricity change by less than the stated gate when the step is halved, on every release control. | Step-halving refinement gate in CI. | Extended displayed-output refinement gate at 600, 500 and 250 s [internal] | F1 | Met: refinement gates pass on four corrected 1M-history records |
| OPS-003 | FARIS shall model pulsed operation with pulse length, dwell, ramp time and duty factor and report average net power. | Average net power equals the hand calculation within 1e-6 relative; duty factor derived, not typed. | Hand-calculation fixture. | EU DEMO baseline pulsed, pulse about 2 h, dwell about 10–20 min [U] | F4 | No: power periods are piecewise constant fractions |
| OPS-004 | FARIS shall model steady-state operation with the recirculating power its current drive requires. | Current-drive wall-plug power appears as a named load (see PWR-001 in the plant-systems file); steady and pulsed options compared on one chart. | Closure test. | Steady-state option raises recirculating power through current drive [U] | F4 | No |
| OPS-005 | FARIS shall separate operating time into operating, planned outage, unplanned outage, fluence-driven outage, dwell and starved (no fuel) states, and report each in days and percent. | Six states sum to the horizon within 1e-9 relative. | Closure test. | FARIS choice | F4 | Partial: operating, outage, replacement and fuel-starved states are distinguished; no unplanned state |

## Fatigue, storage and dispatch

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| OPS-010 | FARIS shall count load cycles on the central solenoid and on structure and report the cycle-life margin. | Cycle count by rainflow or pulse count, method stated; margin against an authored limit; count matches a hand count on a 10-pulse fixture. | Hand-count fixture. | Central-solenoid fatigue of order 1e4 cycles [U] | F4 | No |
| OPS-011 | FARIS shall report thermal-cycling counts per component and flag components past an authored cycle limit. | Count per component per year; a limit exceedance shows FAIL with scope; no limit gives NOT_EVALUATED with next step. | Fixture at, below and above the limit. | ITER divertor qualified for 5,000 cycles at 10 MW/m² [V](https://www.iter.org/machine/divertor) | F4 | No |
| OPS-012 | FARIS shall size thermal storage to bridge the dwell and report energy and cost of the storage. | Storage in MWh equals dwell × bridged power within 1e-9 relative; losses authored and shown. | Energy-balance fixture. | Molten salt and steam accumulators smooth output [U] | F4 | No |
| OPS-013 | FARIS shall dispatch the plant against a user-supplied hourly demand or price series. | 8,760 h year simulated in under 5 s on RL (provisional: no reference tool; confirm before F4 gate); missing hours or non-monotonic time stamps are refused. | Timing test; malformed-series test. | FARIS choice | F4 | No |
| OPS-014 | FARIS shall report load-following losses: ramp energy, minimum stable power and curtailed energy. | Three quantities in MWh over the series; energy balance closes within 1e-6 relative. | Closure test. | FARIS choice | F4 | No |
| OPS-015 | FARIS shall carry grid connection and black-start energy as authored inputs and show them in the balance. | Both inputs required for plants that import power; missing gives NOT_EVALUATED. | Registry test. | FARIS choice | F4 | No |

## Startup, shutdown and schedules

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| OPS-020 | FARIS shall model startup and shutdown energy and the tritium inventory change across a ramp. | Startup energy and tritium ramp reported per start; conserved mass closes within 1e-10 relative. | Mass and energy closure test. | FARIS choice | F4 | Partial: restart hysteresis after starvation exists; no startup energy |
| OPS-021 | FARIS shall let the user edit the operating schedule (phases, power levels, outages) with undo and redo, and record each edit in the receipt. | 100 % of edits undoable to the study's opening state; each edit hashed in the study file; edits with out-of-range values are refused with a reason. | UI automation test; file audit. | FARIS choice | F4 | Partial: schedules are authored in JSON assumptions; no in-app editing |
| OPS-022 | FARIS shall validate a schedule before use. | Overlapping periods, negative durations, power fraction outside 0–1 and outages outside the horizon rejected with the offending line. | Invalid-schedule corpus. | FARIS choice | F1 | Partial: model validation exists; corpus not measured |
| OPS-023 | FARIS shall compare schedules side by side with the existing comparison view and its 2-sigma flags. | Two schedules shown with differences in net electricity, outage days and tritium end inventory; differences inside 2 sigma flagged as unresolved. | Compare-view fixture. | Existing compare view [internal] | F4 | Partial: compare view exists for arrangements, not for schedules |

## Maintenance and replacement

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| OPS-030 | FARIS shall replace components when an authored fluence limit is reached, using the named outage duration, and report the replacement year with its uncertainty. | Replacement year with 2-sigma band propagated from the transport tally error; band computed, not estimated by hand. | Fixture with known flux and error. | Existing replacement logic [internal] | F4 | Partial: deterministic replacement exists; no uncertainty band |
| OPS-031 | FARIS shall take replacement durations as cited inputs and show sensitivity to them. | Tornado of net electricity and availability against each duration; durations from literature carry their link. | Sensitivity fixture. | Crofts et al. DEMO: full blanket replacement about 10 months (421 modules, 4 ports, 4 parallel systems, 20 % contingency), about 6 months best case; internals about 20 months; about 1,000 h per sector [V, via search]: [arXiv 1412.4008](https://arxiv.org/pdf/1412.4008) | F4 | Partial: replacement durations authored per component; no sensitivity view |
| OPS-032 | FARIS shall offer literature maintenance-duration presets with their conditions. | Presets: DEMO blanket 10 months, best case 6 months, internal components 20 months, each labelled literature with port count and parallel remote-handling systems stated. | Preset audit. | Crofts et al. [V, via search] | F4 | No |
| OPS-033 | FARIS shall model remote-handling constraints: number of parallel systems, ports and crews, and show how they change outage length. | Outage length changes with the number of parallel systems on a fixture; the model reproduces the stated Crofts duration for the stated inputs within 10 % (provisional: only summary figures read; confirm before F4 gate). | Reproduction fixture. | Crofts et al. [V, via search]; remote handling availability is a first-order driver | F4 | No |
| OPS-034 | FARIS shall batch component replacements into shared outages and report the saving. | Batched and separate schedules compared; outage days and electricity shown for each. | Fixture. | FARIS choice | F4 | No |
| OPS-035 | FARIS shall couple fluence to failure hazard so that hazard rises near the limit, as a selectable model. | Hazard function reproduces its analytic form; selecting "off" gives the current deterministic behaviour exactly. | Analytic fixture; regression. | FARIS choice | F4 | No |

## Reliability, availability and maintainability engine

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| OPS-040 | FARIS shall simulate plant availability by event-driven Monte Carlo with Weibull failure laws, repair crews, spares and remote-handling constraints. | At least 10,000 trials with a 95 % confidence-interval half-width of 0.5 points or better on availability for the reference case; seeds recorded; same seed gives the same result. | Analytic M/M/1 and exponential cases; determinism test. | AvailSim4 is a discrete-event Monte Carlo engine [V](https://indico.cern.ch/event/1507105/contributions/6342553/attachments/3008536/5303766/CB-SM_AvailSim4-Feb2025.pdf); 10⁴+ trials practice [U] | F4 | No |
| OPS-041 | FARIS shall verify the engine against closed-form results. | Simulated availability within the confidence interval of the exact value for single-unit exponential failure and repair, and for a two-unit parallel case, at 100 % of test cases. | Analytic test suite. | FARIS choice | F4 | No |
| OPS-042 | FARIS shall support phase-dependent failure and repair rates (operating, dwell, shutdown). | Rate changes with phase on a fixture; compared with analytic phase-weighted rate within 1 %. | Analytic fixture. | AvailSim4 phase-dependent rates [V] | F4 | No |
| OPS-043 | FARIS shall model repair crews and spare parts as limited resources with queues. | Waiting time appears in downtime; one crew and zero spares reproduces the hand-calculated queue on a fixture. | Queue fixture. | FARIS choice | F4 | No |
| OPS-044 | FARIS shall report availability, MTBF, MTTR and capacity factor with 95 % confidence intervals. | From at least 1,000 stochastic histories in under 10 s on RL (provisional: reference tool speed unknown; confirm before F4 gate); definitions of each metric stored in the export. | Exponential reference cases; timing test. | Maximo-style asset metrics and PROCESS availability model [U] | F4 | No |
| OPS-045 | FARIS shall run at least 10⁴ trials on RW and use variance reduction or quasi-Monte Carlo when asked, with results stated to agree with plain Monte Carlo. | Quasi-Monte Carlo and importance splitting agree with plain Monte Carlo within 2 sigma on three cases. | Paired runs. | AvailSim4 quasi-Monte Carlo and importance splitting [V] | F6 | No |

## Block diagrams and availability separation

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| OPS-050 | FARIS shall support reliability block diagrams with series, parallel, k-out-of-n and standby structures. | Exact analytic availability for small cases lies within the Monte Carlo confidence interval; analytic solver used where it exists. | Analytic test on at least 8 structures. | ITER and DEMO RAMI use reliability block diagrams [U] | F4 | No |
| OPS-051 | FARIS shall separate planned, unplanned and fluence-driven outages in every availability result. | Three categories shown as days and percent; sum equals total downtime within 1e-9 relative. | Closure test. | FARIS choice | F4 | Partial: planned outages and fluence-driven replacement are separate in the history; no unplanned category |
| OPS-052 | FARIS shall compare availability with labelled thresholds: PPCS 75 %, ARIES-AT 0.85, and the EU DEMO early-operation range of about 30 % or more. | Pass or fail flag per threshold with the source link and kind label; no threshold is applied unlabelled. | Threshold-table audit. | PPCS 75 % [V, via search of Crofts and Federici]: [Federici](https://nucleus.iaea.org/sites/fusionportal/Technical%20Meeting%20Proceedings/3rd%20DEMO/website/talks/G_Federici.pdf); ARIES-AT 0.85 [V, via search](https://www.sciencedirect.com/science/article/abs/pii/S0920379605007210); DEMO about 30 % [V, via search of Federici et al.](https://iopscience.iop.org/article/10.1088/1741-4326/57/9/092002) | F4 | No |
| OPS-053 | FARIS shall keep aleatory and epistemic uncertainty separate in availability results. | Confidence interval from trials labelled sampling-only; a second band from varied input distributions (failure-rate and duration uncertainty) shown separately; the two are never merged into one number. | Fixture with known input spread. | CI width reflects sampling, not input uncertainty [internal: FARIS research trap 8] | F4 | No |
| OPS-054 | FARIS shall label every failure rate as authored, literature or analogue-derived, and show invented rates as such. | 100 % of failure laws carry a label and source; availability with any unsourced law carries a banner. | Label audit. | MTBF-based Monte Carlo with invented MTBFs gives false precision [internal: FARIS research trap 8] | F4 | No |

## Optimisation and criticality

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| OPS-060 | FARIS shall provide a maintenance schedule optimiser with constraints and recorded seeds. | Finds a schedule within 1 % of exhaustive search on 5-component cases; same seed gives the same result; infeasible constraints reported, not relaxed. | Exhaustive-search comparison. | Reload-pattern search analogue in CMS5 (algorithm not verified) [U] | F6 | No |
| OPS-061 | FARIS shall keep the optimiser's inner history evaluation fast enough for search. | 30-year history inside the optimiser at P95 ≤ 5 s per candidate batch of the reference size on RL (see PERF-020 for the base figure; provisional: batch size not set; confirm before F6 gate). | Timing test. | Fission-suite lifecycle tool analogue [U] | F6 | No |
| OPS-062 | FARIS shall compute per-component criticality as consequence × likelihood from modelled fluence, replacement cost and downtime. | Every component listed with safety, operational and financial consequence dimensions; rank matches a hand analysis (Spearman ≥ 0.9) on the worked example. | Worked example against hand calculation. | Criticality = consequence × likelihood, ISO 55001 style: [guide](https://www.glocertinternational.com/resources/guides/iso-55001-asset-criticality-and-risk-management/) [U]; Maximo Health [U] | F4 | No |
| OPS-063 | FARIS shall report how stable the top-ranked criticality list is. | Top-5 ranking re-evaluated with every input varied by 20 %; changes listed. | Perturbation test. | FARIS choice | F4 | No |
| OPS-064 | FARIS shall keep component consequence weights as authored inputs with provenance. | Weights stored per dimension; no default weights present a ranking as calculated. | Registry test. | FARIS choice | F4 | No |

## Operating data and re-baselining

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| OPS-070 | FARIS shall ingest operating records (CSV and IMAS-style data) and bind them to a study by hash. | Import refuses unit, time-base or schema mismatches with the offending row; the data hash is part of the study identity. | Import corpus; tamper test. | EPRI digital twin practice compares actual against virtual [U] | F4 | No |
| OPS-071 | FARIS shall re-baseline predictions from ingested data and show the change. | On synthetic data from a known model, the injected parameter is recovered within 5 %; prediction error before and after re-baselining shown. | Synthetic-data recovery test. | EPRI digital twin report [U]: [report](https://restservice.epri.com/publicdownload/000000003002020014/0/Product) | F4 | No |
| OPS-072 | FARIS shall describe itself as a design model, and shall not use "digital twin" for any feature without a live plant data path. | Text lint finds zero uses of the phrase "digital twin" in the interface and docs except in a definition or this rule; passes in CI. | Grep lint in CI. | ISO 23247 implies live synchronisation [V](https://www.iso.org/standard/78743.html); fusion digital-twin claims lack public numbers [U] | F4 | No |
| OPS-073 | FARIS shall publish a conformance statement mapping its features to ISO 23247 functional entities and say which are not implemented. | One published table: simulation, analytics, reporting, synchronisation each marked implemented or not. | Document check. | ISO 23247-2 [V] | F4 | No |
| OPS-074 | FARIS shall refuse to blend measured and modelled values in one number. | Measured and calculated values are always shown in separate labelled fields; a re-baselined parameter is labelled conditional. | Label audit. | House rule; no fusion plant yet provides live data [internal] | F4 | No |

## Maintenance worker dose

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| OPS-080 | FARIS shall link each maintenance task to the shutdown dose rate for its location and cooling time, using the shutdown-dose capability in the radiation-transport file. | Dose rate in µSv/h at the task place and time; task dose = rate × duration × crew; missing shutdown-dose result gives NOT_EVALUATED with next step. | Fixture with known rate. | ITER limit of about 10 µSv/h at 10⁶ s [U] | F4 | No |
| OPS-081 | FARIS shall report accumulated worker dose against an authored annual limit and flag plans that exceed it. | Dose per worker per year against limit; exceedance shows FAIL with scope; limit labelled authored. | Fixture at, below and above the limit. | FARIS choice | F4 | No |
| OPS-082 | FARIS shall allow the cooling time before maintenance to be a schedule variable and show its effect on dose and downtime together. | Dose and outage length plotted against cooling time on one chart with kind labels. | Sweep fixture. | FARIS choice | F4 | No |

## Traps

- A 30-year recalculation that runs in one second proves the ledger is fast, not that its failure or repair assumptions are right.
- Monte Carlo confidence intervals on availability measure sampling noise. Invented failure rates give a tight interval around a meaningless number.
- Availability compared with 75 % or 0.85 is a comparison with a policy target, not a validation. Show the threshold's source.
- A deterministic replacement year hides the uncertainty from transport error and from the authored fluence limit.
- "Digital twin" wording without live plant data overclaims. FARIS is a design model.
- Re-baselining on a handful of operating records can overfit. Show the data count and the parameter band, not only the new best fit.
- A criticality rank depends on the consequence weights as much as on the physics. Show the weights and how stable the rank is.
