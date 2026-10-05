# FARIS requirements

Status: target set, 2026-10-05. These files state what FARIS must achieve to be the best tool
available on every surface it has: physics breadth and fidelity, accuracy, performance, usability,
accessibility, configurability, extensibility, interoperability, collaboration, reliability,
security, documentation and quality. Each requirement is one testable sentence with a hard target,
a test that decides it, the evidence behind the number, the phase that must meet it, and where
FARIS stands today.

They sit under the [project roadmap](../ROADMAP.md): the roadmap says what each phase delivers,
these files say how good it has to be. The finished demo keeps its own
[acceptance criteria](../DEMO_ACCEPTANCE.md).

## Principles

1. **Measured, not described.** A target is a number or a pass/fail rule. "Fast", "easy",
   "accurate" and "intuitive" are not targets.
2. **Judged on modest hardware.** Interactive targets are met on the reference laptop, not on a
   workstation.
3. **Worst case, not average.** Latency is judged on P95/P99 and maximum; accuracy on the
   distribution and worst case of C/E, not the mean.
4. **Checkers decide.** A requirement is met when an automated test or recorded benchmark says so
   ([QA-002](21-quality-assurance.md)). Nobody marks a requirement met by hand.
5. **Fail closed.** Missing, stale or out-of-range evidence blocks a verdict. It never passes
   silently.
6. **Every number explains itself.** Each value carries a kind label (calculated, authored,
   literature, conditional, not evaluated), and every Unknown or Estimate says why and what would
   resolve it.
7. **Honest scope.** FARIS produces research screening. It makes no licensing or safety claim, and
   it is a design model, not a digital twin, until it has a live plant data path.
8. **Better than the reference, or a stated reason.** Where a mature tool sets a bar (MCNP and
   OpenMC benchmark practice, licensing-grade fission statistics, NASA GMAT's test regime, Blender
   and Zed responsiveness, WCAG 2.2), FARIS meets or beats it, or the requirement says why not.

## How to read a requirement

| Column | Meaning |
| --- | --- |
| ID | `PREFIX-NNN`. IDs are permanent; a withdrawn requirement keeps its ID and is struck through. |
| Requirement | One "FARIS shall ..." sentence. |
| Target | The number or pass/fail rule. "(provisional: reason; confirm before Fx gate)" marks a number that must be confirmed, by measurement or a primary source, before that phase gate. |
| Verification | The test, benchmark, audit or study that decides pass or fail. |
| Basis | Why this number. **[V]** the number was read in a primary source (linked). **[U]** unverified: snippet, recall or paywalled source; a [U] basis cannot support a hard gate until checked. **FARIS choice**: our own decision, no outside reference. **[internal]**: an Avila Labs lesson. |
| Phase | The roadmap phase that must meet the target: F1 reusable study platform, F2 radiation and evolving material state, F3 component multiphysics, F4 plant operation and life, F5 plasma and source coupling, F6 design and research workflows, F7 broader 3D plant environment. |
| Now | **Met** (with evidence), **Partial** (what exists, what is missing), **No**, or **Unmeasured** (something exists but has not been measured against the target). Recorded 2026-10-05 against main. |

## Reference hardware and models

Targets name the machine and the model they are judged on.

| Name | Definition | Use |
| --- | --- | --- |
| RL, reference laptop | Intel Core i3-N305 (8 cores, 1 thread per core, 3.8 GHz), Intel UHD Graphics (Alder Lake-N, integrated), 30 GB RAM, 3024×2016 display, Ubuntu 26.04 LTS. | All interactive targets; preview-tier compute. |
| RW, reference workstation | Class definition: 16 or more cores, 64 GB or more RAM, discrete GPU with 12 GB or more memory and Vulkan 1.3, NVMe disk. Exact model to be named before the F2 gate. | Large models (RM-L), production transport, GPU transport. |
| RC, reference cluster | Class definition: SLURM, 4 or more nodes with 64 or more cores and 256 GB each, 25 Gb/s or faster interconnect. Exact system to be named before the F6 gate. | MPI transport scaling, large sweeps. |
| RM-S, small model | The current idealised demo: ARC-inspired concentric-torus radial build with one outboard port, four arrangements. | Every phase; regression baseline. |
| RM-M, medium model | A CAD-derived 22.5° (or similar) sector with ports and penetrations, about 10⁴ cells. Fixture to be built in F2. | Preview-tier and production-tier timing, geometry import. |
| RM-L, large model | A full 360° plant with 10⁵ or more cells, ITER E-lite class. Fixture to be built by F7. | Large-model rendering, scaling, memory. |

RW and RC are deliberately class definitions now. Naming exact machines before the phases that use
them avoids tuning targets to whatever hardware happens to be available.

## Headline targets

One or two defining targets per surface. Each is a full requirement in its file; the rest of the
file is what it takes to get there.

| Surface | Target | ID |
| --- | --- | --- |
| Geometry | Every defect in a seeded corpus of ≥ 20 models and ≥ 11 defect classes is detected before transport, with ≤ 1 % false alarms. Cell volumes match the source within 0.1 % (analytic) and 0.5 % (faceted). | [GEO-023](01-geometry-and-model.md), [GEO-030](01-geometry-and-model.md) |
| Transport | Runs stop on a target error (1 % on TBR), not a fixed history count. Deep shielding gets automatic weight windows worth ≥ 100× in figure of merit. Heating is always the neutron + photon + electron + positron sum. | [NUC-043](02-radiation-transport.md), [NUC-050](02-radiation-transport.md), [NUC-013](02-radiation-transport.md) |
| Activation and materials | Activation results change < 1 % when time steps are halved. A component passes a damage limit only when the 2σ upper bound, data uncertainty included, is below it. No waste class is given without complete nuclide and impurity data. | [ACT-012](03-activation-and-materials.md), [MAT-014](03-activation-and-materials.md), [ACT-039](03-activation-and-materials.md) |
| Plant systems | Required TBR is a surface over burn fraction × fuelling efficiency, availability, processing time and reserve, never one number. Published fuel-cycle (Abdou 2021, within 5 %) and cost (ARIES-ACT, within 10 %) cases are reproduced in CI. Net power comes from ≥ 10 itemised loads. | [FUEL-010](04-plant-systems.md), [FUEL-030](04-plant-systems.md), [ECO-013](04-plant-systems.md), [PWR-001](04-plant-systems.md) |
| Operation and life | Availability from event-driven Monte Carlo with ≥ 10⁴ trials and a 95 % interval half-width ≤ 0.5 points. | [OPS-040](05-operation-and-life.md) |
| Accuracy | Every calculated quantity has a benchmark row with bias and uncertainty, or is shown as "not validated"; a blank row blocks release. Pass/fail against experiment uses a predeclared compatibility rule, and unknown covariance gives INCONCLUSIVE. The 1 s history is within 1 % (TBR) and 2 % (net electricity) of a full transport rerun over 200 random points. | [VAL-051](06-accuracy-and-validation.md), [VAL-027](06-accuracy-and-validation.md), [VAL-061](06-accuracy-and-validation.md) |
| Uncertainty | No number is shown without an uncertainty or a stated reason it has none. Nominal 95 % intervals cover the truth 93–97 % of the time over ≥ 100 seeded repeats. | [UNC-080](07-uncertainty.md), [UNC-002](07-uncertainty.md) |
| Performance | Plant-life what-if recalculation P95 ≤ 100 ms (history) and ≤ 1 s (full coupled plant) on the laptop. Viewport P99 ≤ 16.7 ms. Preview-tier transport result with stated error ≤ 60 s. | [PERF-020](08-performance.md), [PERF-011](08-performance.md), [PERF-025](08-performance.md) |
| Usability | SUS ≥ 80.3 with new domain users by F6. Every core task ≥ 90 % unaided success. Every state-changing action undoable. | [UX-001](09-usability.md), [UX-011](09-usability.md), [UX-020](09-usability.md) |
| Accessibility | 100 % of functions keyboard-operable. Screen-reader access on Linux by F2 and on all platforms by F7. | [A11Y-001](10-accessibility-and-localisation.md), [A11Y-040](10-accessibility-and-localisation.md) |
| Visualisation | Every plotted Monte Carlo value shows its error; high-error cells are hatched, never hidden. | [VIS-030](11-visualization.md), [VIS-032](11-visualization.md) |
| Configurability | Every dimensional input carries a unit and is dimension-checked; ≥ 500 injected wrong-dimension values are all rejected. | [CFG-041](12-configurability.md) |
| Automation | The CLI covers 100 % of non-view operations. The cache never serves a false hit. | [AUTO-002](13-automation-and-extensibility.md), [AUTO-033](13-automation-and-extensibility.md) |
| Interoperability | A reader written only from the published .faris specification passes every fixture. An exported OpenMC model runs unmodified and reproduces the TBR within 3σ. | [INT-001](14-interoperability.md), [INT-020](14-interoperability.md) |
| Design workflows | Optimisers never call a candidate better inside 2σ (0 of 100 false improvements under injected noise). Optimisation on a surrogate is blocked above its error tolerance. | [DSN-023](15-design-workflows.md), [DSN-012](15-design-workflows.md) |
| Evidence | Every result records its determinism class. The checker detects 100 % of 10,000 injected tamperings. | [PRV-020](16-evidence-and-provenance.md), [PRV-005](16-evidence-and-provenance.md) |
| Collaboration | Semantic diff of two studies lists 100 % of changed parameters, with 0 false changes on re-save. | [COL-020](17-collaboration.md) |
| Reliability | 0 damaged files in 10,000 kill-during-save trials. ≤ 30 s of edits lost after any crash. | [REL-010](18-reliability.md), [REL-011](18-reliability.md) |
| Security | Opening any file runs no embedded code and starts no process. Every parser survives 24 h of fuzzing. 0 known critical vulnerabilities at release. | [SEC-001](19-security.md), [SEC-021](19-security.md), [SEC-031](19-security.md) |
| Platforms | Tier-1 Linux, Windows and macOS, each backed by CI evidence. | [PLAT-001](20-platforms-and-distribution.md) |
| Quality | Every requirement here has an automated test, traced in a generated matrix. One release-gate script blocks any release that misses a condition. | [QA-002](21-quality-assurance.md), [QA-070](21-quality-assurance.md) |
| Documentation | Every example in the docs executes in CI. | [DOC-020](22-documentation-and-learning.md) |
| Compliance and privacy | No telemetry by default (0 outbound requests). Every output carries the research-screening statement. | [LEG-021](23-compliance-and-privacy.md), [LEG-040](23-compliance-and-privacy.md) |

### What is measured today

FARIS already has three things no surveyed tool combines: Monte Carlo transport driving a
plant-life history that recalculates in about a second, uncertainty flags carried into comparisons,
and hash-bound evidence for every number ([competitive landscape](research/competitive-landscape-2026-10-01.md)).
Its main gaps against these targets:

- no validation suite against experiment yet;
- uncertainty is not yet propagated into the history;
- no undo, autosave or screen-reader exposure;
- Linux only;
- no cost model;
- a two-compartment tritium model;
- no CAD import;
- fixed 1 M-history transport at about 20 minutes per case.

## Files

| File | Prefixes | Requirements | Met | Partial | Unmeasured | No |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| [Geometry and model](01-geometry-and-model.md) | GEO | 69 | 0 | 26 | 1 | 42 |
| [Radiation transport and neutron source](02-radiation-transport.md) | NUC, SRC | 104 | 1 | 43 | 2 | 58 |
| [Activation, waste and materials](03-activation-and-materials.md) | ACT, MAT | 97 | 1 | 9 | 0 | 87 |
| [Plant systems](04-plant-systems.md) | MAG, THM, FUEL, PWR, ECO, SAFE | 99 | 4 | 16 | 0 | 79 |
| [Operation and life](05-operation-and-life.md) | OPS | 45 | 2 | 8 | 0 | 35 |
| [Accuracy and validation](06-accuracy-and-validation.md) | VAL | 72 | 1 | 18 | 0 | 53 |
| [Uncertainty](07-uncertainty.md) | UNC | 61 | 2 | 12 | 0 | 47 |
| [Performance](08-performance.md) | PERF | 37 | 1 | 12 | 6 | 18 |
| [Usability](09-usability.md) | UX | 71 | 0 | 21 | 3 | 47 |
| [Accessibility and localisation](10-accessibility-and-localisation.md) | A11Y, L10N | 57 | 0 | 12 | 13 | 32 |
| [Visualization](11-visualization.md) | VIS | 49 | 1 | 31 | 1 | 16 |
| [Configurability](12-configurability.md) | CFG | 63 | 1 | 17 | 1 | 44 |
| [Automation and extensibility](13-automation-and-extensibility.md) | AUTO | 74 | 0 | 16 | 0 | 58 |
| [Interoperability](14-interoperability.md) | INT | 59 | 1 | 19 | 3 | 36 |
| [Design workflows](15-design-workflows.md) | DSN | 75 | 2 | 21 | 0 | 52 |
| [Evidence and provenance](16-evidence-and-provenance.md) | PRV | 42 | 5 | 19 | 2 | 16 |
| [Collaboration](17-collaboration.md) | COL | 40 | 2 | 5 | 0 | 33 |
| [Reliability](18-reliability.md) | REL | 38 | 2 | 11 | 1 | 24 |
| [Security](19-security.md) | SEC | 55 | 8 | 20 | 1 | 26 |
| [Platforms and distribution](20-platforms-and-distribution.md) | PLAT | 43 | 4 | 18 | 0 | 21 |
| [Quality assurance](21-quality-assurance.md) | QA | 77 | 3 | 21 | 1 | 52 |
| [Documentation and learning](22-documentation-and-learning.md) | DOC | 45 | 0 | 15 | 1 | 29 |
| [Compliance and privacy](23-compliance-and-privacy.md) | LEG | 36 | 5 | 12 | 2 | 17 |
| **Total** | | **1408** | **46** | **402** | **38** | **922** |

Counts are generated from the tables on 2026-10-05. 46 of 1408 requirements are met today; that is the point of the set. It describes where FARIS is going, not where it is.

## Change control

- A target may be tightened at any time. Loosening one needs a recorded reason: a measurement
  showing it is unachievable on the reference hardware, or a corrected source. The old value stays
  in the row's history (git) and the reason goes in the commit message.
- New requirements take the next free ID in their section. IDs are never reused.
- Provisional targets are confirmed or replaced before the phase gate they name. A phase gate is
  not passed while any of its requirements is provisional, No or Unmeasured.
- "Now" is refreshed at every phase gate and whenever a requirement's test lands.
- Each requirement gains a link to its test when the test exists ([QA-002](21-quality-assurance.md)).

## Sources

The numbers come from research done on 2026-10-05 across six areas: transport and nuclear data;
plant engineering; usability, accessibility and visualisation; configurability and
interoperability; quality, reliability and security; and analogue industries (fission core-design
suites, digital-twin practice, multiphysics platforms, spacecraft analysis tools). Primary sources
are linked in each row. Where a page was paywalled or blocked, the row says [U] and the target is
provisional. The research notes themselves are in [research/](research/): R1 transport and nuclear data, R2 plant
engineering, R3 usability and visualisation, R4 configurability and interoperability, R5 quality,
reliability and security, R6 analogue industries, plus the 2026-10-01 competitive landscape. A Basis
cell such as "R5 PD-01" or "R4 trap" points to a row or trap in those notes; it carries no outside
source of its own and counts as a FARIS choice. Before quoting any number outside Avila Labs, open the linked primary source.
