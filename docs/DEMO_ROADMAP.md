# FARIS functional demo roadmap

Status: scoped functional/numerical demo complete on the named Linux workstation,
2026-10-01. This document covers the first demo only.
The implementation now includes coupled neutron/photon transport, component
spectra and direct heating scores, a conservative Rust fuel/event/energy ledger,
actual Core stage execution and receipt verification, saved-study reopening,
and native field/history presentation. The [acceptance matrix](DEMO_ACCEPTANCE.md)
tracks the evidence and scientific limits. The four corrected
primary runs, port-volume checks, sampling/spatial diagnostics and independent
history refinement controls are complete. Paired comparisons, authored
sensitivities, twelve outage-duration probes, portable packaging, actual Core
execution/reopening, and the native walkthrough are verified. Completion is
within the declared cold numerical boundary; it does not assert that unqualified
physical claims or unmet local precision/convergence targets have passed.
The [native verification record](../references/native-demo-verification.json)
binds the final checks to the delivered binaries and package index.

An independent volume check exposed a first-wall clearance error in the earlier
transport geometry. Those runs and their derived histories are preserved as
superseded diagnostics. All four corrected million-history cases pass
cell-ownership checks, and both port cases pass independent volume checks.
An additional directly tallied port window resolves a local sampling effect;
fine/coarse map diagnostics do not establish mesh convergence or physical
qualification.

The [scientific baseline](SCIENTIFIC_BASELINE.md), [cold model boundary](COLD_REFERENCE.md),
[operating-history controls](OPERATING_HISTORY.md), and
[coupled Li-6 control](LITHIUM_CAPTURE_CONTROL.md) distinguish numerical
verification from physical qualification. Material operating state, reactor
reference responses, nuclear-data applicability and engineering limits remain
unqualified. Those gaps restrict claims; completed solver jobs and verified
Core receipts do not turn them into physical passes.

| Milestone | Current implementation status |
| --- | --- |
| M0 | Complete: starting capabilities and demo boundary recorded |
| M1 | Complete within the declared cold numerical boundary: frozen materials/data/source, authored operating inputs and response applicability limits |
| M2 | Complete: typed transport/history/evidence formats, bounded workers, dependencies, actual Core compilation, and native cancellation verified |
| M3 | Corrected coupled primary cases and independent normalization/capture controls complete; physical qualification explicitly NOT_EVALUATED |
| M4 | Complete within the scoped spatial boundary: corrected ownership/volumes, sampling/window and fine/coarse diagnostics, calculated fields, picking and native presentation verified; local-map convergence remains NOT_EVALUATED |
| M5 | Complete within the deterministic numerical boundary: all four drivers verified at 600/500/250 s, independent conservation/event/energy checks, paired comparisons, 27-point reruns and outage-duration probes |
| M6 | Complete: all four genuine Core cases verified and reopened after relocation; fresh native four-stage execution re-inspected after app exit; scoped findings, dependency/edit flow and CLI replay verified |
| M7 | Complete on the named workstation: final indexed distribution, native walkthrough, supported input edit/recalculation, real solver cancellation/recovery, exports and measured startup/frame throughput; physical qualification and second-host execution remain unevaluated |

## The result we are building

**Within one compact D-T tokamak's fixed radial envelope, how does allocating
more space to breeding blanket and less to shielding affect breeding, magnet
exposure, fuel availability, replacements, and lifetime electricity?**

The demo must answer that question with actual calculations and an inspectable
chain of inputs, assumptions, and results. It need not identify a winning design.
An unresolved comparison is useful when its causes and missing evidence are clear.

A successful demonstration lets a person:

1. Open a native Rust/egui workspace containing two reactor arrangements.
2. Inspect credible 3D geometry, materials, and one real local penetration.
3. Select analyses and inspect their required inputs and dependencies.
4. Press **Compile study**, see a small **Powered by Avila Core** treatment, and
   receive real compiler findings or a compiled study.
5. Run a bounded calculation, or load identified results from a completed run.
6. Explore calculated spatial fields and component responses in 3D.
7. Scrub an operating history that actually changes fuel inventory, accumulated
   exposure, component state, outages, and cumulative electricity.
8. Compare the arrangements, inspect uncertainty and assumptions, and export a
   reproducible record of the study.

Live geometry or study edits must produce real changes in the relevant inputs
and subsequent calculations. Recorded results may make the demonstration fast,
but they must remain identifiable, reproducible outputs of the same workflow.

## Scope boundaries

| Included in the required demo | Boundary |
| --- | --- |
| One ARC-inspired compact tokamak | A documented idealized research model; no claim to reproduce the complete ARC design |
| Two blanket/shield allocations | Same outer envelope, materials, source, and shared local feature; allocation is the controlled change |
| Neutronics | Real fixed-source OpenMC calculations, component responses, spectra, and spatial fields |
| Meaningful 3D physics | One explicitly modeled penetration with a matched feature-free control |
| Tritium and operation | A conservative Rust inventory/time model driven by calculated production and explicit recovery/operation assumptions |
| Component service limits | Exposure-based scenario limits with declared sources and applicability; no implicit prediction of actual magnet failure |
| Electricity | A transparent power/energy balance with stated recoverable-heat, efficiency, and auxiliary-load assumptions |
| Avila Core | Generated study contract, genuine compilation, controlled execution/evidence binding for declared stages, and scoped assessments |
| Native presentation | egui controls, wgpu 3D, comparison plots, usable timeline, study diagnostics, and exports |

Activation/decay heat through ACTINV is a possible extension after component
spectra and operating histories exist. It is not required to complete this demo.
An activation selector must show unavailable until its adapter is implemented.
Converra may supply documented design inputs if useful; integrating it is also
not a completion requirement.

Self-consistent plasma evolution, thermal hydraulics, stress/fracture, detailed
superconductor degradation, full plant CAD, detailed fuel processing, economics,
remote execution, and other reactor concepts are outside this roadmap. Record
any simplified input they supply as an assumption or external input. The demo
must remain useful without those models.

Rust owns the scenario, adapter boundaries, normalization, time model, and shared
operations. Python can prepare or execute an external scientific solver. The
desktop uses egui and wgpu. The simulation remains independently usable through
the CLI; the Core experience is a required part of the finished desktop demo.

## Starting point and unresolved choices

| Area | M0 starting state (historical) |
| --- | --- |
| Rust model/engine/CLI | Scenario validation, source identity, geometry metadata, deterministic export |
| Desktop | Working cutaway, orbit/zoom, picking, visibility, arrangement selection, inspector |
| Geometry | Concentric circular tori; magnet envelope; no penetration or detailed component structure |
| Transport and physical fields | Not implemented; material compositions and nuclear data unassigned |
| Operating history | Slider only; no fuel, exposure, replacement, or electricity model |
| Avila Core | No generated contract, compiler invocation, case package, or receipts |
| Presentation | Initial panels and basic shaded meshes; substantial refinement required |

The authored starting allocations are 0.45 m blanket / 0.45 m shield and
0.55 m blanket / 0.35 m shield within a 1.20 m total radial build. The scenario
uses a 3.3 m major radius, 525 MW nominal fusion power, and a 30-year horizon.
Their provenance and limitations are in [DEMO.md](DEMO.md) and the scenario.

Before quantitative implementation, settle and record:

- An accessible named transport reference, its exact geometry/source/materials,
  and which responses it can validate. A simpler reference can validate a
  specific calculation without validating the entire tokamak design.
- Blanket, shield, first-wall, vessel, and magnet-region compositions, densities,
  enrichment, temperatures, homogenization, and any coolant/void fractions.
- One evaluated nuclear-data distribution, required temperatures/reaction data,
  installation footprint, and acquisition/redistribution conditions.
- D-T source energy/angular/spatial distributions and source-rate normalization.
- One penetration's location, shape, fill, affected components, and tally regions.
- Steady or pulsed operation, initial tritium inventories, recovery delay/losses,
  startup/reserve rules, service-limit scenarios, and replacement durations.
- Recoverable-heat accounting, thermal efficiency, operating/outage auxiliary
  loads, and the meaning of reported net electricity.
- The pinned Core implementation/profile, supported binding interface, and
  uncertainty/qualification policy for this research study.
- A named demonstration workstation and acceptable run/resource budgets.

These choices are implementation prerequisites, not requests for a predetermined
scientific outcome. Do not silently tune them to obtain a favorable comparison.

## Delivery sequence

Milestones describe usable increments. Finish the listed exit evidence before
calling a milestone complete. UI refinement can proceed alongside scientific
work once the shared result formats are settled.

| Milestone | Dependencies | Deliverable | Exit evidence |
| --- | --- | --- | --- |
| M0 — Establish the starting state | Existing scaffold | Honest current capability map and this bounded demo plan | Existing scene/CLI checks recorded; missing calculations remain absent |
| M1 — Freeze the scientific baseline | M0 | Reference case, materials/data/source choices, operating assumptions, response definitions, validation criteria | Inputs are obtainable and identified; each intended claim has a stated validation route and limitation |
| M2 — Build study and job foundations | M1 | Versioned result/field/history formats, dependency planner, bounded workers, generated Core contract, genuine Compile study flow | Valid/invalid studies exercise the actual compiler; readiness is separate; jobs cancel without freezing the UI |
| M3 — Calculate and check the baseline | M1, M2 | First working OpenMC adapter, component rates/spectra, normalization, one reproduced reference | Reference responses meet predeclared criteria or the claim is explicitly restricted; CLI and app consume the same records |
| M4 — Deliver spatial comparison | M3 | Both arrangements, shared penetration and control, refined 3D geometry, real field overlays | Cell/mesh mapping and volumes verified; penetration effect examined with uncertainty; spatial results come from transport |
| M5 — Couple operating history | M3; M4 for final field inputs | Rust fuel/exposure/event/power model and comparisons | Mass balance, time/event checks, replacements, outage decay, and energy accounting pass independent controls |
| M6 — Connect Core and the complete experience | M2–M5 | Compile → readiness → run/reuse → results → evidence for both arrangements | Actual stage receipts and evidence bind to the study; diagnostics and verdicts appear on the relevant UI objects; CLI reproduces the study |
| M7 — Finish and deliver the demo | M4–M6 | Polished native app, verified recorded results, fresh-run path, export bundle, short walkthrough | End-to-end acceptance below passes on the named workstation; scientific limits remain visible |

Critical path: **reference/data → valid solver inputs → checked transport →
spatial results and history → bound evidence and final presentation**. A more
elaborate renderer or a generated contract cannot substitute for that path.

Do not promise wall-clock dates before measuring the reference calculation and
data preparation. After M1/M3, size remaining work using observed runtime,
integration effort, and unresolved scientific inputs.

## Detailed requirements

Every row is required unless explicitly marked optional. These are engineering
and product acceptance criteria; they are not automatically Core verdicts.
`D01`–`D13` in [DEMO.md](DEMO.md) remain the high-level requirements; the groups
below expand them into implementable work.

| Requirement group | Primary milestones | High-level coverage |
| --- | --- | --- |
| S — Study scope | M1, M2, M6 | D08, D12 |
| G — Geometry | M1, M3, M4 | D01, D05, D13 |
| A — Materials/source/data | M1, M3 | D03 |
| T — Transport | M3, M4 | D04, D05 |
| L — Operating history | M5 | D06, D07 |
| C — Comparison | M5, M6 | D08 |
| K — Core | M2, M6 | D08, D11, D12 |
| U — Native experience | M2, M4, M6, M7 | D02, D09, D11, D13 |
| J — Execution | M2, M3, M6 | D09, D10 |
| V — Verification | M1–M7 | D01, D03–D11 |
| P — Delivery | M7 | D08, D10, D13 |

### S — Study scope and scientific meaning

| ID | Requirement | Acceptance evidence |
| --- | --- | --- |
| S01 | Preserve the fixed-envelope research question and controlled comparison | Both arrangements share every declared control; changes appear in a typed input diff |
| S02 | Separate observations, literature inputs, authored assumptions, solver outputs, and projections | Each result/input exposes its origin and applicable model boundary |
| S03 | Define each metric, domain, unit, averaging region, and normalization before implementation | Versioned response definitions cover breeding, spectra, heating, exposure, inventories, events, and energy |
| S04 | Separate requested calculations from testable acceptance requirements | A study can request a response without inventing an engineering limit; an assessment requires an explicit criterion and basis |
| S05 | Make the validity domain of stationary transport and exposure-based service limits explicit | Material evolution/feedback exclusions and limit applicability accompany affected results |
| S06 | Preserve excluded and unsupported analyses as visible coverage gaps | Unchecked analyses are reported as outside the study; missing support is not converted into a favorable verdict |

### G — Geometry and component identity

| ID | Requirement | Acceptance evidence |
| --- | --- | --- |
| G01 | One versioned scenario defines solver and display component identities and geometry | Adapter export, viewport, tallies, and history agree on IDs, locations, dimensions, and units |
| G02 | Hold the outer envelope and shared feature fixed across the two allocations | Independent boundary/geometry comparisons confirm only the declared allocation changes |
| G03 | Model one finite penetration and a feature-free control | Full solver regions and display surfaces include the same opening, fill, and affected components |
| G04 | Validate material partitions, boundaries, source containment, gaps, and unintended overlaps | Geometry diagnostics and deliberate negative cases detect malformed geometry and lost-particle problems |
| G05 | Recompute applicable volumes after adding the penetration | Modified-cell volumes are checked independently with reported numerical uncertainty; unmodified full-torus formulas are not reused blindly |
| G06 | Keep display clipping, visibility, and tessellation separate from physical geometry | Camera/cutaway changes leave solver input identity and reported full-component volumes unchanged |
| G07 | Identify resolved structure and geometric surrogates clearly | Magnet envelope and homogenized layers are labeled; visual detail is supported by the actual model |

### A — Materials, source, and scientific data

| ID | Requirement | Acceptance evidence |
| --- | --- | --- |
| A01 | Assign explicit physical materials to every modeled material region | Validated nuclides/fractions, density, temperature, enrichment, and fraction conventions replace unassigned labels |
| A02 | Record material provenance and mixture assumptions | Inputs cite their sources; coolant/void treatment and homogenization are inspectable |
| A03 | Pin evaluated-data identity and confirm required coverage | Library/file identities, nuclides, temperature policy, heating data, and missing reactions are checked before execution |
| A04 | Define and validate the same D-T source for both arrangements | Energy, angular/spatial distributions, support, strength, and sampling settings are identified |
| A05 | Derive absolute source strength consistently from the operating input | Fusion power, per-reaction energy, neutron yield, and unit conversions have independent controls; fusion power is not treated as neutron power |
| A06 | Record the solver and worker environment | Tool/build identity, adapter version, commands, settings, seeds, thread/process counts, and relevant environment are preserved |

Material conventions must follow the selected tool's documented interface;
[OpenMC material definitions](https://docs.openmc.org/en/stable/usersguide/materials.html)
cover fraction types, density, temperatures, mixtures, and cross-section selection.

### T — Actual transport results

| ID | Requirement | Acceptance evidence |
| --- | --- | --- |
| T01 | Execute a real fixed-source transport problem from complete FARIS inputs | Solver output/statepoint, logs, exit state, input identities, and diagnostic summary exist |
| T02 | Calculate total and component tritium production with a defined breeding ratio | Tally coverage and numerator/denominator are explicit; production is linked to the fuel model |
| T03 | Calculate component energy spectra and the chosen magnet exposure response | Energy bins, threshold conventions, averaging region, and the distinction between regional average and local response are retained |
| T04 | Calculate nuclear heating under an explicit photon treatment | Coupled photon deposition or local-deposition approximation is identified; the result is not presented as temperature |
| T05 | Produce genuinely spatial flux/heating/production fields for the chosen demo layers | Recorded 3D coordinates, cells/mesh, material occupancy, units, estimates, uncertainties, and tally identity map into the viewport |
| T06 | Normalize each tally exactly once and use the applicable volume | Rust controls cover per-source quantities, absolute rates, and volume-normalized responses; exported raw values remain available |
| T07 | Use a bounded sampling plan and report achieved statistical precision | Histories/batches/seed and acceptance targets are defined before the production run; weakly sampled bins are visibly identified |
| T08 | Retain solver warnings and failure states | Invalid geometry, incomplete statepoints, missing data, and abnormal exits never become completed physical results |

OpenMC documents `H3-production`, flux, reaction, heating, and damage-energy
scores with different per-source units. Absolute rates and flux densities require
the appropriate strength and volume conversions. See
[OpenMC tallies and normalization](https://docs.openmc.org/en/stable/usersguide/tallies.html).
The exact score/estimator/data support must be checked against the pinned release.
Damage-energy is not automatically a calibrated magnet lifetime or DPA response.

### L — Fuel, exposure, maintenance, and electricity

| ID | Requirement | Acceptance evidence |
| --- | --- | --- |
| L01 | Run a real Rust state/time model with explicit initial conditions | Saved snapshots identify calendar time, operating state, integrated power/exposure, and all tracked inventories |
| L02 | Distinguish calendar time from full-power time and handle events at their actual times | Pulse/outage/replacement boundaries are respected; results converge under time-step refinement |
| L03 | Conserve tritium across available and in-process inventories, production, burn, losses, transfers, imports/exports, and decay | A per-step and cumulative residual closes within a predeclared tolerance; internal transfers cancel in the site total |
| L04 | Represent recovery/processing delay and startup/reserve rules explicitly | Delayed production cannot be spent early; the model stops or reduces operation on an actual fuel deficit and records the event |
| L05 | Apply decay during all calendar intervals and preserve declared processing/retention behavior during outages | Analytic decay-only and delay controls pass; outage policies are inspectable |
| L06 | Accumulate the selected exposure response according to actual operation | Instantaneous rates and accumulated exposure remain distinct; exposure does not grow during source-off periods |
| L07 | Evaluate service limits with traceable response/limit compatibility | Metric, energy range, units, component, assumptions, and uncertainty match; permanent-component limits terminate or restrict operation according to declared policy |
| L08 | Replace only declared replaceable components and account for downtime | Replacement resets only their appropriate local state; permanent components and site inventories retain their history; simultaneous events have deterministic ordering |
| L09 | Compute recoverable thermal power, gross electricity, auxiliaries, and cumulative net electricity without double counting | Contribution ledger identifies neutron/charged-particle/reaction-energy treatment; signed net power is integrated consistently through operation and outages |
| L10 | Return an actual operating outcome for both alternatives | The comparison exposes fuel-limited operation, replacement outages, end-of-life cause, completed horizon, or unresolved prerequisites |

A breeding ratio above one is not itself a fuel-sufficiency result. Sufficiency
depends on the modeled losses, delays, reserves, decay, and operating schedule.
Service-limit projections are conditional scenario results unless their response
model and limits have supporting qualification for the stated use.

### C — Comparison and uncertainty

| ID | Requirement | Acceptance evidence |
| --- | --- | --- |
| C01 | Compare both arrangements using common definitions and operating assumptions | Input diff, common criteria, and comparable outputs accompany every comparison |
| C02 | Distinguish Monte Carlo sampling uncertainty, assumed parameter ranges, model discrepancy, and qualified bounds | Display and records name the uncertainty source and interpretation; sampling error is not advertised as total uncertainty |
| C03 | Assess differences with appropriate dependence/covariance treatment | Seeds/replicates and any correlation assumptions are recorded; a directional claim is made only when supported |
| C04 | Show breeding, selected magnet response, fuel inventory, operating availability, events, and cumulative net electricity together | Tables/plots share input/run identity and timeline; unavailable values remain absent |
| C05 | Include a small sensitivity study of the consequential lifetime assumptions | A bounded sweep covers recovery/loss and service-limit/outage assumptions, with ranges justified and histories recalculated |
| C06 | Report the supported conclusion and what limits it | Ranking, tradeoff, or unresolved comparison includes applicable reference results and dominant assumptions |

### K — Avila Core study compilation and evidence

| ID | Requirement | Acceptance evidence |
| --- | --- | --- |
| K01 | Generate a study from selections, scenario, assumptions, metrics, and criteria using one reusable demo template | Ordinary use requires no hand-authored Core JSON; generated declarations can be inspected and exported |
| K02 | Expand domain dependencies explicitly | Selecting lifetime/fuel/exposure adds required upstream responses; missing support, external inputs, and assumptions are shown without silent substitution |
| K03 | Generate a complete compatible contract, registry snapshot, and required bindings | Quantity kinds/units, slots, parameters, reproducibility, purposes, claim models, and requirement bases resolve against a pinned Core profile |
| K04 | Preserve numeric meaning across the FARIS/Core boundary | Exact decimal/unit representations and conversion policy are tested; UI rounding cannot change a criterion or verdict |
| K05 | Invoke the real Core compiler and consume its structured outcome | A valid study compiles; missing/incompatible declarations produce actual findings with stable codes, locations, and relevant UI links |
| K06 | Separate compilation from artifact readiness and scientific evaluation | A compiled study can still lack runnable data/executables; FARIS preflight exposes that state; compilation is never shown as an engineering pass |
| K07 | Bind scenario, selections, template, registry, compiler/profile, data, methods, and criteria to the compiled study/run | Changes create a new study identity; old results remain inspectable and visibly belong to their original inputs |
| K08 | Use a few coarse executable stages and reusable packaging | Geometry/material preparation, transport/normalization, and operating-history/comparison stages have documented boundaries; no contract per component or time step |
| K09 | Use real execution/evidence records for Core-backed results | Declared executable/input/output identities and receipts verify; admissible claims and requirement verdicts derive from those bound records |
| K10 | Preserve all four assessment states and their scope | `PASS`, `FAIL`, `INCONCLUSIVE`, and `NOT_EVALUATED` remain distinct; unsupported/omitted analyses cannot appear as established requirements |
| K11 | Keep research assumptions and qualification gaps visible | A conditional service-limit scenario or unqualified estimate cannot be presented as a qualified physical prediction; policy changes are explicit |
| K12 | Keep normal FARIS execution independently usable and measure integration effort | Shared Rust operations and CLI work without the UI; adding the same study requires generated/reusable metadata, with setup and upkeep recorded |

FARIS owns domain dependency expansion and contract materialization. Core checks
the declarations it is given. It cannot discover omitted physical effects or
verify unnamed data merely because a study compiles. The current local compiler
exposes contract/registry compilation and compilation with bound template
material; verify the selected implementation before choosing the integration API.
See [the integration plan](../integrations/avila-core/README.md).

The study panel should distinguish **requested outputs**, **assessment
criteria**, and **required dependencies**. It must explain why a dependency is
included. Removing an analysis changes study coverage; it does not establish that
the physical effect is negligible. A required dependency can be supplied by an
identified external result or declared approximation only when the study
explicitly accepts that boundary.

The first contract generator supports the one demo study family. It does not
need arbitrary contract editing, automatic solver selection, a generic workflow
canvas, or an agent to invent requirements.

For a response-only request, any Core requirement must explicitly concern a
declared calculation/data check, or the UI must request a missing assessment
criterion where the selected Core profile needs one. Never invent an engineering
threshold simply to obtain a compiled contract.

### U — Native workspace, 3D quality, and interaction

| ID | Requirement | Acceptance evidence |
| --- | --- | --- |
| U01 | Deliver a native egui/wgpu application with a coherent Blender-inspired workspace | Scenario/study controls, central viewport, outliner, inspector, timeline, and results remain usable at supported sizes/scales |
| U02 | Replace crude presentation with model-supported geometry and clear shading | Sufficient tessellation, stable normals/caps, distinct component boundaries, camera framing, lighting, and a visibly modeled penetration; no material visual artifacts |
| U03 | Keep exploration dependable | Orbit, zoom, frame, component selection/hiding, cutaway, and slice controls behave consistently; hidden components do not steal picks |
| U04 | Make fields interpretable | Colorbar, quantity, units, scale/log convention, common comparison range, time meaning, uncertainty/coverage, and data identity are visible; absent bins differ from actual zero |
| U05 | Keep field/geometry correspondence accurate | Selection resolves the right material/component; slice/hover samples the actual recorded region/bin; mixed-volume bins expose occupancy treatment |
| U06 | Make the timeline drive calculated state | Scrubbing selects/interpolates identified snapshots; current power, inventories, cumulative exposure/energy, replacements, and outage states update; interpolation never crosses discrete events silently |
| U07 | Provide an actionable Compile study control near the upper right | State shows draft/changed, compiling, compiled, or rejected with relevant findings; independent run readiness and Run/Cancel controls remain clear |
| U08 | Add the exact text **Powered by Avila Core** to the actual compile experience | A small Core mark and text sit beside the compile progress/result; a subtle pulse/spinner while compiling settles into a quiet attribution after completion |
| U09 | Keep the attribution truthful and considerate | Show it for genuine Core attempts, including rejection; tooltip exposes Core version/profile; no fake wait, obstructive popup, approval badge, or forced animation; reduced-motion behavior is available |
| U10 | Present useful component and comparison evidence | Inspector links dimensions, materials, response definitions, results, assumptions, and run records; selection connects to the plots/diagnostics |
| U11 | Support the actual demonstration path | Readable contrast/type, keyboard-accessible primary controls, progress, error recovery, saved study reopening, and a clear recorded-results versus fresh-run choice |

The badge is a small brand detail, not the central result. Use the existing
Avila Core mark at an appropriate small size and retain a text-only fallback.
Display a static result/attribution immediately when compilation is fast.
This requirement does not add a badge to the current non-Core scaffold.

Reference-rate fields can stay constant in a stationary transport model, while
accumulated exposure and inventories evolve. The UI must state which is shown.
An instantaneous source-dependent field must follow source-off operation;
decay-heat fields remain unavailable until an activation model supplies them.

### J — Execution, responsiveness, and reuse

| ID | Requirement | Acceptance evidence |
| --- | --- | --- |
| J01 | Keep substantial preparation, I/O, compilation, and solver work off the egui thread | Camera and controls remain responsive while a job runs; progress is delivered through a bounded job interface |
| J02 | Bound histories, memory/disk artifacts, threads/processes, and runtime | The named workstation has measured budgets and preflight estimates; invalid or excessive requests are refused with a useful reason |
| J03 | Support cancellation and child cleanup | Cancel stops owned work/children, preserves diagnostics, and never labels partial output as a completed result |
| J04 | Verify reuse against the appropriate inputs and execution records | Unchanged results reuse only after identity checks; geometry/material/source/data/settings changes invalidate affected transport; history-only changes preserve eligible upstream transport |
| J05 | Separate recorded-result loading from fresh solver execution | A packaged result opens quickly with provenance; Run fresh is explicitly available when compatible tools/data are installed |
| J06 | Preserve useful failures and recover cleanly | Logs, diagnostics, job state, and input identity survive an error; reopening does not turn interrupted work into success or overwrite a completed run |
| J07 | Make desktop and CLI clients of the same operations | Equivalent studies yield equivalent definitions, normalized results, histories, and comparisons; no duplicated UI physics |
| J08 | Validate and publish completed artifacts safely | Version/schema, dimensions, units, IDs, finite values, identities, and size limits are checked; completed records are published atomically and partial writes remain incomplete |

Changing threads/builds may prevent byte-identical Monte Carlo results.
Distinguish input/execution identity, deterministic Rust postprocessing, and
statistical reproducibility. State the reproducibility claim actually supported.

### Shared artifacts to establish in M2

Choose the smallest useful formats and version them; this table specifies their
contents, not new schemas that already exist. The UI must read the same artifacts
as the CLI, and large field arrays must support bounded loading.

| Artifact | Required contents | Responsible boundary |
| --- | --- | --- |
| Study definition | Scenario reference, selections, assumptions, metrics/criteria, dependency plan, template/version, input identity | Rust model/engine |
| Physical geometry/material map | Full regions, stable IDs, material/source definitions, units, volumes and applicable uncertainty, local feature | Rust geometry and solver adapter |
| Solver invocation and raw results | Exact exported inputs, tool/data/settings identity, seeds, logs, statepoint/raw tallies, execution status | External worker through the Rust job/adapter boundary |
| Normalized responses and fields | Raw tally references, response definition, units, normalization, component/cell/mesh coordinates, occupancy, values, sampling uncertainty | Rust normalization/result boundary |
| Operating history | Model/version, rates/assumptions, inventories, exposure, events, power/energy ledger, residuals, snapshots, end-state reason | Rust time/event engine |
| Comparison and sensitivity | Both study/run identities, controlled input diff, metrics, curves, uncertainty treatment, sensitivity inputs and supported conclusions | Rust comparison operations |
| Core/evidence bundle | Generated declarations, compiled identity/findings, applicable bindings/receipts/claims/verdicts, referenced artifact identities and export index | Core adapter and export boundary |

### Initial workstation performance targets

These are proposed demo targets to confirm in M1 and measure in M7, not claims
about current performance or scientific accuracy:

- Open the packaged study and first useful scene within five seconds.
- Sustain at least 30 frames per second during orbit/scrub at 1440 × 900 on the
  named workstation with the delivered field dataset.
- Acknowledge Run/Cancel and other primary actions within 250 ms; expensive work
  continues in workers. Stop owned solver children within ten seconds of Cancel,
  with the termination state reported if graceful shutdown is unavailable.
- Set explicit memory/disk limits and a fresh-transport runtime budget after the
  M3 reference measurement. Show measured/estimated runtime and sampling quality.

Fresh production transport need not finish during the short presentation.
Demonstrate a genuine history recalculation interactively, allow the full fresh
transport path, and use verified recorded transport where its measured cost
exceeds the presentation window.

### V — Scientific and implementation verification

| ID | Requirement | Acceptance evidence |
| --- | --- | --- |
| V01 | Define response-specific validation criteria before using production comparisons | Named reference, expected response, applicable uncertainty, and tolerance are recorded; failed comparisons restrict the affected claim |
| V02 | Check geometry, source, tally coverage, normalization, and units independently | Simple controls expose swapped units, wrong volumes, double normalization, and missed production regions |
| V03 | Check sampling and spatial refinement | Additional histories/replicates and a refined mesh assess the stability of reported totals and local effects; a voxel average is not labeled a pointwise peak |
| V04 | Check the history model against independent limiting cases | Decay-only, delayed-recovery, constant-power energy, fuel starvation, replacement, permanent-component limit, and simultaneous-event controls pass |
| V05 | Exercise real Core failures and scoped evaluations | Missing bindings, wrong kinds/units, inadequate claim models, stale identities, overlapping uncertainty, and missing evidence produce their appropriate actual outcomes |
| V06 | Run one end-to-end workflow for each arrangement | Scenario → compilation/preflight → transport → normalization → history → comparison → export is reproduced and inspected |
| V07 | Verify UI behavior and workstation performance | Native launch, fields/picking, variant edits, timeline events, cancellation, missing dependencies, scaling, and recorded-results startup are checked |

Use the neighboring Fusion Energy Ledger as a candidate source of model ideas,
not a validated absolute breeding predictor. The local
`fusion-energy-ledger/VALIDATION_REPORT.md` records an 8.5% full-reactor WCLL
TBR overprediction and failed mechanism reproduction. That neighboring project
and report are not included in this repository.
Record what is reused and independently recheck its implementation and boundary.

### P — Demo delivery and reproducibility

| ID | Requirement | Acceptance evidence |
| --- | --- | --- |
| P01 | Ship a working native binary for the named demo workstation | Clean launch and compatible drivers/tool dependencies are documented; the first release need not ship all operating systems |
| P02 | Include verified precomputed results for both arrangements and the relevant controls | Identity-checked artifacts contain the real fields, histories, comparison, diagnostics, and provenance needed to explore offline |
| P03 | Provide a separate documented fresh-run path | Required solver/data versions and acquisition steps reproduce the study without hidden author-only files; licenses govern bundled data |
| P04 | Export a portable study/evidence bundle | Scenario, selections, assumptions, generated contract/registry, applicable bound records, result formats, identities, and verification instructions are included; required external data is identified |
| P05 | Provide a short walkthrough and a factual capability statement | Demonstration explains the question, calculation, local effect, operating consequence, uncertainty, and reproducibility; current limitations accompany the result |
| P06 | Leave repository and artifacts maintainable | Source/build lockfiles, focused checks, AGPL-3.0-only project licensing, documented third-party/data terms, and ignored heavyweight run/data directories are ready; no implicit GitHub publication |

## Completion criteria for the finished demo

The finished demo passes this complete sequence on the named workstation:

1. **Open:** load the identified study and both computed arrangements without
   requiring a solver run. Confirm data identities and inspect the shared
   geometry, materials, and penetration.
2. **Compile:** select the required study analyses and compile with real Core.
   Confirm the small attribution, actual compiled identity, dependency summary,
   and separate run readiness. Deliberately remove a prerequisite and observe
   an actionable preflight/compiler finding at the correct boundary.
3. **Explore:** select a component, cut/slice the scene, switch a real field, and
   inspect its units, region definition, uncertainty, and source record.
4. **Compare:** switch allocations with a consistent field scale; examine the
   feature-free control and the local penetration result. The displayed effect
   must be supported by the achieved numerical precision.
5. **Follow history:** scrub across operation, fuel delay, an outage, and a
   replacement or declared end-of-life event. Show inventory balance,
   permanent-component history, and cumulative net electricity.
6. **Inspect the conclusion:** show the supported tradeoff/ranking or unresolved
   comparison, the relevant conditional assessments, and at least one sensitivity
   that changes or constrains the interpretation.
7. **Recalculate:** change a supported input and run the affected stages within
   the declared budget. A fuel/recovery assumption can rerun history using valid
   transport; a geometry/source/material change requires new transport.
8. **Cancel/recover:** cancel an owned fresh job, inspect its preserved state,
   and return to completed results without stale or partial output appearing valid.
9. **Reproduce/export:** reproduce through the CLI, export the study, and verify
   its bound records using the stated tools. A second installation can load the
   bundle and follow documented external-data instructions for a fresh run.

The demo is complete when those actions work with real data and the requirements
above have supporting evidence. A good-looking reactor, a compiled contract,
handwritten curves, or an untraceable solver output alone is insufficient.

## Original implementation sequence (completed within the declared demo boundary)

1. Resolve M1's reference/material/data/source choices and define acceptance
   targets for the specific responses we will show.
2. Freeze the smallest shared artifacts needed by transport, fields, and history.
3. Implement worker/job ownership and a headless transport reference slice.
4. Generate and compile the one Core study template against the pinned registry,
   exercising real rejection paths before adding the compile attribution.
5. Build outward from the checked reference into the two arrangements and one
   penetration; refine the 3D presentation alongside those real artifacts.

Reuse existing solvers and data where they satisfy this study. Create new FARIS
tools only where this chain exposes a concrete missing interface or model.

## Next steps (recorded 2026-10-05, after 0.1.0)

### Where 0.1.0 stands

Delivered since the 2026-10-01 plan: transport sampling uncertainty carried
through the operating history (ensembles with medians, 90 % ranges, event
probabilities and the 1 % non-physical-draw rule), fast flux above 0.1 MeV in
three named magnet regions with per-region service limits, `.faris` study
files, and PDF/CSV/chart export from the desktop and the command line. The
recorded transport uses 10 million histories per arrangement and sweep point
and 30 million for the port-free controls, whose port-sector magnet flux is
otherwise too poorly sampled for Gaussian ensembles.

An expert reviewer's first two objections remain: the magnet check uses
region averages, not the local peak, and maintenance timing ignores
activation and decay heat. Release 0.2 addresses both, plus the sign-in token
storage.

### One transport campaign for both physics items

Both items need new transport runs, so they share one campaign:

- **709-group component spectra.** Component spectra are recorded in 10 coarse
  groups (edges 1 keV … 20 MeV, then to 1 GeV). ACTINV needs its 709-group
  structure; rebinning 10 groups would assume the spectrum shape inside each
  group, and threshold and resonance reaction rates depend on exactly that
  shape. The runs tally per-component neutron spectra on the 709-group
  structure directly, so no within-group shape is assumed (ACT-002).
- **A peak-magnet mesh with weight windows** (next section).

### Magnet peak with FW-CADIS weight windows

OpenMC 0.15.3 provides `WeightWindowGenerator(method="fw_cadis")`, driven by a
random-ray multigroup pass (`Model.convert_to_multigroup`,
`convert_to_random_ray`, `settings.random_ray` with `adjoint`). Plan:

1. A fine mesh over the magnet behind the port and a matching inboard slab,
   scoring fast flux above 0.1 MeV (a new mesh preset; current presets do not
   target the magnet).
2. A separate generation pass builds the multigroup model, runs random ray
   with that mesh as the FW-CADIS objective, and writes `weight_windows.h5`.
3. The weight-window file becomes a hashed run input: its SHA-256, generator
   method and parameters, and the generating run's identity enter the input
   record, so recorded runs stay verifiable.
4. The continuous-energy run applies it (`weight_windows_on`), and reports
   per-bin relative error, variance of the variance and figure of merit. The
   peak bin and the region average are both shown, each with 2σ.
5. Cross-check against MAGIC on the same mesh, and against the existing
   analog region averages.

Constraints found in the solver: random ray needs isotropic, isothermal
multigroup data, void as a null-filled cell (FARIS already does this), and a
discrete-energy source, so the 14.1 MeV line is represented as one group.
Weighting changes batch statistics, so the batch-means response covariance is
rechecked for weighted runs before ensembles use it. The generated windows
depend on the multigroup library and are recorded, not regenerated.

### Activation and decay heat with ACTINV

Run the pinned `actinv` command line as a subprocess (JSON in, JSON out),
matching how FARIS runs OpenMC:

1. Spectrum per component from the 709-group tally, converted to
   n cm⁻² s⁻¹ with the run's source rate.
2. Material per component from the physics file's nuclide mixture, mass from
   volume and density, plus an authored impurity list per material with its
   source. Without one, results are labelled a lower bound: the surrogate
   materials carry none of the impurities (Co, Nb, Mo, Ni, Mn, Ag) that
   dominate fusion activation.
3. Irradiation schedule per component from the history events (operating
   intervals scaled by power fraction, outages as zero-flux steps), cut at
   that component's replacements. A removed component gets its own run, which
   gives its inventory at removal. Every lumping rule is recorded and its
   error checked against an unlumped run (ACT-009, ACT-010).
4. Results at each outage and replacement and over a cooling grid from 1 s to
   1e9 s (ACT-019): decay heat, activity and dominant nuclides, on the
   timeline and the 3D model.
5. Receipt: transport run, history and spectrum hashes, the ACTINV problem
   file, its result certificate (library, decay and covariance hashes) and
   binary version (ACT-001).

Shutdown dose needs a decay-photon transport and stays `NOT_EVALUATED`.
ACTINV gives cross-section uncertainty only; the spectrum's sampling error
would enter through the transport ensemble, not through ACTINV.

### Sign-in token storage

SEC-041: store the account token in the operating-system credential store
where one exists, with an owner-only file fallback. This lives in the account
crate, not in FARIS.

### Decisions for 0.2 (made 2026-10-05)

- **Impurities.** Activation runs each material twice: bare (the transport
  nuclides only, labelled a lower bound) and with an authored impurity list at
  the published specification maximum of the nearest commercial grade (for
  example ASTM B170 oxygen-free copper for the magnet surrogate), every element
  cited to its specification. Both are shown. Reason: the bare surrogates
  omit the elements that dominate activation, and a specification maximum is
  a citable, conservative screening value rather than an invented one. The
  transport materials are unchanged, so transport and activation stay
  consistent apart from trace elements too dilute to affect the neutron field.
- **Which outages.** All of them. One ACTINV run per component installation,
  from installation to removal plus the cooling grid, with outages as
  zero-flux steps. ACTINV reports after every step, so every outage and the
  removal come from the same run at no extra cost. Reason: the cost is per
  installation, not per outage, so selecting a subset saves nothing.
- **Library.** ACTINV's default full-coverage TENDL-2025 neutron bundle, its
  hash recorded in each receipt. The patched remediation bundle (44 leaked
  ordinates zeroed; not an official TENDL release) is run once in the test run
  on one component, and the difference is reported. Reason: follow the solver's
  default and measure the alternative instead of guessing.
- **Spectrum uncertainty.** Reported separately in 0.2, not propagated into
  decay heat. Reason: component-average flux errors at 10 million histories
  are 0.03 % (blanket) to about 1 %, far below the cross-section uncertainty
  ACTINV propagates; the magnet (5–25 %) is the exception and is flagged.
  Propagation through the transport ensemble needs one ACTINV run per sample
  and component, and ACTINV's own linear-response test found flux linearity
  failing in soft spectra, so it is not approximated linearly.
- **Integration.** The pinned `actinv` command line as a subprocess with
  explicit data paths, not a crate dependency. Reason: the same boundary
  FARIS uses for OpenMC, no build coupling, and the CLI emits a hashed
  certificate.
- **Spectrum groups.** Tally 709 groups in transport rather than rebin 10.
  Reason above; this also means neither ACTINV input gap recorded on
  2026-10-05 (ACTINV `docs/PARKING.md`) blocks FARIS.
- **Magnet peak method.** FW-CADIS weight windows, with MAGIC as a cross-check
  on the same mesh. Reason: MAGIC is seeded by a forward flux that is itself
  nearly empty behind a metre of shield.
- **Shutdown dose.** `NOT_EVALUATED` in 0.2; decay heat, activity and dominant
  nuclides only.
- **Order.** Two short test runs first (a 1M-history run with 709-group
  spectra pushed through ACTINV for one component; FW-CADIS weight windows on
  the port-free control measured against the analog run), then one combined
  campaign. Either test can fail without costing a campaign.

### Smaller open items

- File dialogs (Open, Save as, Export) and their keyboard shortcuts have not
  been exercised in a live session.
- The packaged demo in `dist/` still has the earlier binaries, and its Core
  receipts cover only the loaded baseline assumptions, not the default
  demountable-magnet preset.
- The native interface-check plans in `references/native-demo-checks` click
  fixed positions that moved with the step layout.

## References for implementation

- [Demo question and starting scenario](DEMO.md).
- [FARIS Rust and adapter boundaries](ARCHITECTURE.md).
- [OpenMC geometry](https://docs.openmc.org/en/stable/usersguide/geometry.html),
  [execution settings](https://docs.openmc.org/en/stable/usersguide/settings.html),
  and [geometry troubleshooting](https://docs.openmc.org/en/stable/usersguide/troubleshoot.html).
- [OpenMC photon physics](https://docs.openmc.org/en/stable/methods/photon_physics.html)
  for the selected heating/deposition treatment.
- [Avila Core](https://github.com/AvilaLabs/Avila-Core); choose its pinned
  implemented compiler/runner semantics rather than assuming every product
  roadmap feature is available.
