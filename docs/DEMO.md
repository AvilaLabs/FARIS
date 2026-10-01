# First demo: blanket, shielding, and plant life

The question is: **within one compact D-T tokamak's fixed radial envelope, how
do two blanket/shield allocations change breeding, magnet exposure, fuel
availability, replacement schedules, and lifetime net electricity?**

The demo must make that causal chain inspectable in a native 3D workspace.
Its conclusion is whatever the calculations support, including an unresolved
comparison. A predetermined winner is not a requirement.

The complete demo-only implementation plan, detailed requirements, dependencies,
and acceptance walkthrough are in [ROADMAP.md](ROADMAP.md). The current
geometry scaffold does not meet that functional-demo definition.

## Starting scenario

`scenarios/arc-inspired/scenario.json` is the authoritative input. The reference
allocation has a 0.45 m blanket and 0.45 m shield; the alternative moves 0.10 m
from shielding to the blanket. All other layer dimensions and identities are
held fixed, giving the same 1.20 m radial build. The 3.3 m major radius and
525 MW nominal fusion power are inspired by the public
[ARC heat-exhaust study](https://arxiv.org/abs/1809.10555).

Other dimensions and the 30-year horizon are authored assumptions. This is not
an ARC reproduction. The present geometry uses circular concentric tori; it
omits ports, a divertor, detailed blankets, discrete coils, and support structures.
The magnet envelope is only a volume reservation. All physical materials must
be assigned before transport can run.

## User journey

1. Explore the reactor in 3D; cut it open and select a component.
2. Inspect the component's geometry, material, and scientific results.
3. Switch arrangements and see geometry and calculated field changes together.
4. Move through an operating history, including outages and replacements.
5. Compare fuel inventory and lifetime electricity, and inspect limiting assumptions.

The study controls let the user select analyses, inspect their dependencies and
assessment criteria, and press **Compile study**. FARIS generates the contract
and invokes Avila Core. A small **Powered by Avila Core** mark accompanies actual
compilation progress and its result. Run readiness and scientific verdicts are
shown separately from compilation.

The scaffold supports geometry exploration and a timeline selector. The final
demo additionally needs real transport results and a working lifetime model.

## Requirements for completing this demo

| ID | Requirement | Completion evidence |
| --- | --- | --- |
| D01 | One input defines displayed and calculated component identities and geometry | Geometry adapter and viewport agree on IDs, boundaries, volumes, and units |
| D02 | Native egui interface with a functioning 3D viewport | Launch, selection, visibility, cutaway, camera, and variant checks |
| D03 | Both arrangements use traceable material definitions and the same source model | Recorded compositions, densities, temperatures, source normalization, and nuclear-data identity |
| D04 | Real spatial transport results are attached to the corresponding geometry | Mesh/cell fields, component spectra, breeding, heating, uncertainties, and a reference/refinement check |
| D05 | A meaningful local feature is represented before presenting detailed 3D physics claims | Explicit port, penetration, or other chosen asymmetry with controlled comparison; fidelity limitations retained |
| D06 | Operating history conserves fuel and accumulates exposure consistently | Mass-balance controls, calendar/full-power-time checks, replacement and outage checks |
| D07 | Replacements reset only the replaced component | Permanent components and site inventories retain their appropriate history |
| D08 | Every displayed result is identifiable and scoped | Source/model/assumptions attached; unsupported quantities remain absent or unresolved |
| D09 | Interaction stays responsive during expensive work | Cached results load immediately; fresh calculations use bounded, cancellable workers |
| D10 | Both alternatives can be reproduced through the CLI | Same scenario and engine produce the same identifiable results as the desktop |
| D11 | A generated Core study supports genuine compile, execution, and evidence inspection | Actual compiler findings, compiled identities, stage receipts, and scoped assessments; ordinary use needs no manually authored contracts |
| D12 | Study selections reveal dependencies and coverage | Required inputs and criteria are explicit; excluded analyses and unsupported claims remain visible |
| D13 | The demo has a polished, functional native presentation | Credible model-supported geometry, real field overlays, responsive timeline/comparisons, useful diagnostics, and the small Core compile attribution |

## Inputs still to settle

Choose a reference geometry/source dataset that is actually accessible, assign
the blanket and shield material compositions, and select the nuclear-data
library. Define the pulsed or steady operating schedule, component service-limit
assumptions, replacement durations, and initial tritium inventory. Identify which
reported quantities can be checked against the chosen reference.

The existing Fusion Energy Ledger can inform the early power/fuel model. Its
absolute WCLL breeding prediction failed its reference comparison, so its TBR
results cannot be adopted as qualified predictions for this demo. Use its
documented limitations when considering reuse.

Temperature, stress, plasma evolution, fracture mechanics, detailed fuel
processing, and plant costing can initially be declared assumptions or scoped
external inputs. They become calculated fields only as relevant models are
integrated. The full multiscale virtual neutron source is a later research module.
