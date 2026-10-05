# FARIS project roadmap

This roadmap covers the long-term FARIS project. The focused first demo has its
own [DEMO_ROADMAP.md](DEMO_ROADMAP.md), containing its detailed requirements,
implementation milestones, and completion criteria.

Status: high-level direction, 2026-09-30. Later phases are proposed development
areas, not implemented capabilities or scheduled commitments. The current
application is a Rust/native geometry scaffold.

## Long-term objective

Build a native 3D research environment for studying a fusion plant as a coupled
system: geometry, plasma/source conditions, radiation, materials, magnets, fuel,
operation, maintenance, and plant performance across its life.

FARIS should connect specialized open-source tools and Avila Labs tools through
shared scenarios and explicit interfaces. Studies should reveal consequential
missing models or data and help identify new tools worth creating. Useful narrow
studies should remain available while the broader environment grows.

Rust owns the scenario and shared simulation operations. egui provides the
interface and wgpu the native 3D workspace. External tools may use other
languages. Avila Core supplies generated study compilation and evidence records;
the simulation also remains usable independently through the CLI.

## Development phases

The first phase is the immediate focus. Later phases can overlap where their
dependencies are satisfied; their detailed requirements will be written around
concrete research questions and available validation evidence.

The measurable targets each phase must meet, across physics, accuracy, performance,
usability, accessibility, configurability, interoperability, reliability, security and
quality, are in [requirements/](requirements/README.md). A phase gate is not passed while any
of its requirements is provisional, unmet or unmeasured.

| Phase | Result | Main dependencies |
| --- | --- | --- |
| F0 — Functional narrow demo | A real blanket/shield tradeoff study with transport fields, fuel/exposure/energy history, native 3D exploration, and the Core study experience | Complete the separate demo roadmap |
| F1 — Reusable study platform | Extend the demo's scenario, geometry, artifact, adapter, job, and comparison interfaces to support additional bounded studies | Demonstrated interfaces and limitations from F0; a second concrete study |
| F2 — Radiation and evolving material state | Couple transport to activation/transmutation, decay heat, material inventories, and irradiation history; update environments where state changes matter | Validated spectra, material data, history model, and suitable inventory adapters |
| F3 — Component multiphysics | Add relevant thermal, mechanical, magnet, and materials-response models, replacing selected service-limit assumptions with supported calculations | Defined coupling interfaces, applicability evidence, and response-specific reference cases |
| F4 — Plant operation and life | Expand fuel processing, maintenance, reliability, operating schedules, power balance, and replacement logistics into a broader plant model | Component models and validated conservation/event semantics |
| F5 — Plasma and source coupling | Integrate plasma/scenario tools and more detailed spatial/time-dependent source models when the research question requires them | Suitable external tools, source normalization, and verified coupling methods |
| F6 — Design and research workflows | Support parameter studies, sensitivities, candidate comparison, optimization, and repeatable investigations across the integrated models | Stable study definitions, uncertainty treatment, bounded execution, and result reuse |
| F7 — Broader 3D plant environment | Expand geometry fidelity, plant systems, and supported configurations into the intended full research workspace | Demonstrated demand, available models/data, and validated studies for each added domain |

ACTINV, Converra, OpenMC, and the wider ecosystem can contribute as their inputs,
outputs, and applicability fit a phase. The [tooling shortlist](TOOLING.md) is a
starting point; integrating every listed framework is not a project milestone.

## Work that continues across phases

- Keep one scenario authoritative across geometry, solvers, history, and display.
- Grow the native workspace into a useful scientific interface with credible
  geometry, interpretable calculated fields, timelines, and comparisons.
- Preserve input/tool/data identity, units, assumptions, uncertainty, and scoped
  assessments as integrations become more complex.
- Reuse generated Core study templates and coarse stages; measure whether the
  integration reduces repeated work and improves research reproducibility.
- Maintain bounded execution, cancellation, appropriate result reuse, and
  desktop/CLI parity as calculation costs grow.
- Validate each added response within its stated domain; expose gaps and
  unresolved comparisons instead of implying complete physical coverage.
- Keep FARIS under AGPL-3.0-only and record applicable third-party/data terms.

## How this roadmap grows

Finish and evaluate the first demo before freezing detailed requirements for
later phases. Use its scientific findings, integration costs, and user experience
to choose the next bounded study. Expand the platform where that study exposes
a consequential need, integrating existing tools before creating a new model.

This project roadmap owns the broad direction. Each focused demo or subsequent
delivery should have a separate plan with its own requirements and acceptance
criteria. The first is [the functional demo roadmap](DEMO_ROADMAP.md).
