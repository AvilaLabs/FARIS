# Test: does physics-derived maintenance time change the answers?

Recorded 2026-10-06, before any result exists. This protocol is fixed; amendments are appended at the end with a
date and a reason, and never edit the text above them. Its SHA-256 is recorded in the commit that adds it.

## Why this test

Fluence-limited replacement timelines already exist elsewhere. bluemira's `LifeCycle` replaces components at
damage or fluence limits, with stochastic outages, and feeds a tritium fuel-cycle model (see the 2026-10-06
correction in `docs/requirements/research/competitive-landscape-2026-10-01.md`). In bluemira, in PROCESS and in
FARIS 0.1, the time a replacement takes is an authored number. In a real plant it depends on how hot and how
radioactive the in-vessel components are when the plant stops: handling can start only after decay heat has fallen
far enough for the components to be moved without active cooling. EU-DEMO studies, for example, find that about
15 days of cooling are needed for the outboard blanket segments and about 85 days for the inboard segments
([EUROfusion WPPMI report](https://scipub.euro-fusion.org/wp-content/uploads/eurofusion/WPPMICPR18_20248_submitted-4.pdf)).

FARIS could compute that time: 3D transport, then activation per component with ACTINV, then decay heat versus
cooling time, then the replacement duration, then the operating history. This test asks whether doing so changes
any design answer that an authored duration gives. If it does not, the coupling adds no decision value and FARIS
has no physics edge from it. If it does, FARIS answers a question the existing tools answer with a guess.

## What is compared

Two duration models, everything else identical:

- **Fixed (the 0.1 behaviour).** Every replacement takes its authored duration from
  `scenarios/arc-inspired/demountable-magnet-assumptions.json`: 120 days for the magnets, 60 days for the blanket.
- **Computed.** Each replacement takes `work + cooldown`. `cooldown` is the time after the plant stops until the
  decay heat density of the in-vessel components that must be removed falls below a threshold `q*`. `work` is the
  remainder of the authored duration.

**Calibration, so that only physics variation is tested.** For each component class, `q*` is set so that the
first replacement of that class in the reference port arrangement (`port`, `reference`) has exactly its fixed
duration under the computed model. Absolute durations therefore agree at that one event by construction. Every
difference in the results comes from how cooling needs vary between events, arrangements and allocations, which
is the physics the fixed model cannot see.

**Split grid.** `work` is the share `w` of the authored duration, with `w` in {0.25, 0.50, 0.75}. The central case
is `w = 0.50`.

**Governing components.** A magnet replacement must remove everything inside the magnets. Its cooldown is
governed by the first wall, the blanket, the shield and the vessel, and the governing quantity is their combined
decay heat divided by their combined volume. A blanket replacement is governed by the first wall and the blanket.
A second governing quantity, ACTINV's contact gamma dose, is reported beside it as a cross-check, calibrated the
same way. Contact dose is a slab-geometry screening proxy and is not the governing quantity.

**Coupling.** Durations change the timeline, the timeline changes the irradiation histories, and those change the
durations. Iterate history → activation → durations → history until no duration changes by more than 1 day, with
at most 5 iterations. A case that does not converge is reported as not evaluated, with the last two duration sets.

## Inputs

- **Transport.** The four recorded arrangements and the seven-point allocation sweep, rerun with 709-group
  (`fispact-709`) neutron spectra tallied per component at 2 million histories, with the recorded 0.1.0 seeds
  offset by a fixed amount. The histories keep the recorded 0.1.0 rates. Activation uses the new spectra, scaled to
  the recorded component flux, so the history and the activation see the same flux magnitude. The spectrum shape's
  sampling error is reported per component.
- **Activation.** ACTINV 1.4.0, default TENDL-2025 library. The bare transport materials (a lower bound) are the
  primary case. The specification-maximum impurity case from the 0.2 decisions in `docs/DEMO_ROADMAP.md` is a
  secondary run, reported but not used for the verdict. One ACTINV run per component installation, with outages
  as zero-flux steps subdivided on a logarithmic cooling grid (1 hour to 365 days, 40 points), so that every decay
  curve comes from the run itself.
- **Operating assumptions.** The demountable-magnet preset, unchanged except for the duration model.

## Decisions tested

Each is evaluated under both models at every `w`. Lifetime net electricity is over the 30-year horizon.

- **D1. Ranking.** The order of the four recorded arrangements by lifetime net electricity.
- **D2. Allocation optimum.** In the port arrangement, the sweep point (blanket thickness) with the highest lifetime
  net electricity.
- **D3. Downtime contrasts.** For each of the four 0.1 contrasts (breeder − reference with and without the port,
  port − no port at each allocation), the difference in total replacement downtime over life. Computed divided by
  fixed.
- **D4. Replacement policy.** In the port reference arrangement, the blanket is replaced when its fluence reaches a
  fraction `f` of its limit, with `f` in {0.5, 0.6, 0.7, 0.8, 0.9, 1.0}. Under the fixed model the best `f` is 1.0,
  because an earlier replacement costs the same downtime and gains nothing. The question is whether the computed
  model moves the best `f` below 1.0, because a younger blanket cools faster.

## Verdict rules

These are evaluated by a script from the recorded outputs, not by judgement.

- **D1 changes** if the computed ranking differs from the fixed ranking, and every pair that swaps differs by more
  than 2 % of the larger lifetime net electricity under the computed model.
- **D2 changes** if the computed optimum is a different sweep point, and the two points differ by more than 1 % in
  lifetime net electricity under the computed model.
- **D3 changes** if computed / fixed falls outside [0.8, 1.25] for any contrast whose fixed downtime difference is
  at least 30 days.
- **D4 changes** if the computed best `f` is at most 0.9, and it gains more than 1 % lifetime net electricity over
  `f = 1.0`.

The verdict is MATERIAL if any decision changes at `w = 0.50`. It is NOT MATERIAL if no decision changes at any
`w`. Otherwise it is MIXED, and the report names which decisions changed at which `w`.

The thresholds are authored. They are set at what a design review would treat as a real difference, not at
statistical limits. Transport sampling error enters only through the spectra and the recorded rates. A decision
change that sits within the 0.1.0 ensemble ranges of the quantities it compares is flagged as such in the report,
but it still counts.

## What this test does not show

- Decay heat density is a proxy for handling feasibility. Real cooldown depends on component temperatures under
  passive cooling, the remote-handling equipment and the maintenance scheme. The calibration removes the absolute
  scale but not this simplification.
- The geometry is the 0.1 idealised torus with surrogate materials. Activation of the surrogates, especially
  without impurities, is a lower bound.
- A NOT MATERIAL verdict means that, for this study's design questions, physics-derived maintenance time does not
  change the answers. It does not cover other design questions, such as structural material choice, where
  activation differs far more.

## Outputs

`runs/maintenance-coupling/` (gitignored) holds every run. `references/maintenance-coupling-test.json` records the
inputs' hashes, the calibrated `q*` values, every duration set, the decision tables and the verdict.
`docs/notes/MAINTENANCE_COUPLING_RESULT.md` reports it in plain language.

## Amendments

### Amendment 1, 2026-10-06, before any result: how the driver reads this protocol

Written when `scripts/maintenance_coupling_test.py` was built on synthetic inputs, before any real transport or
activation run. Each item resolves a point the text above leaves open. None changes a threshold or a decision rule.

1. Cooldown is interpolated linearly in ln t, and in ln q when both ends are positive. It is the first time the
   governing curve reaches or falls below q*. If the first grid point (1 hour) is already below, the cooldown is
   1 hour.
2. A retained component's decay curve after a shutdown runs only until flux resumes. The replaced component's curve
   is the full cooling grid after its removal. The combined curve covers the time both exist.
3. When an outage is shorter than the cooldown, the crossing cannot be seen. The cooldown is then taken as at
   least the outage length and flagged `window_limited`. Such an event never counts as converged, and the next
   iteration lengthens the outage, so a converged result never rests on a window-limited event. NOT EVALUATED
   applies only when the curve stays above q* for 365 days, or when 5 iterations do not converge.
4. q* is calibrated on the fixed-duration history of `port`/`reference`, with target cooldown `(1 − w)` times the
   fixed duration. After coupling, that event's duration may drift slightly because the timeline moves. There is
   no outer loop to re-calibrate.
5. Convergence compares the durations a history used with the newly computed ones, per event, each clipped to the
   remaining horizon. If the number of replacements changes, events beyond the previous list start from the fixed
   duration.
6. D1: a swapped pair is a pair whose strict order differs between the models. Its gap is divided by the larger
   computed value.
7. D2: the gap is between the fixed-model and computed-model optimum points, under the computed model, divided by
   the larger value.
8. D3: downtime is the sum of replacement intervals, running to the horizon for a replacement that is unfinished.
   A negative ratio counts as a change.
9. D4: the gain is relative to the computed result at `f = 1.0`, and ties pick the larger `f`. Each `f` reruns the
   coupled iteration with q* from the `f = 1.0` calibration.
10. A decision with any NOT EVALUATED input is NOT EVALUATED. The verdict is NOT EVALUATED, never NOT MATERIAL,
    when no decision changes but some decision was not evaluated.
11. The contact-dose cross-check needs a photon response in the ACTINV specs. Without one, it is reported as not
    evaluated, with that reason, and it never feeds a decision.
12. The 365-day end of the cooling grid is 365 × 86,400 s.

### Amendment 2, 2026-10-06, after the first result: full decay curves and more iterations

Written after the first run returned NOT EVALUATED (`docs/notes/MAINTENANCE_COUPLING_RESULT.md`, commit `5daf257`).
That verdict stands and stays in the record. Connor approved this amendment and a second run on 2026-10-06. It changes
how decay curves are obtained and the iteration limit, so that the second run can reach the decisions. `q*`, the `w`
grid, the decisions and their thresholds, the governing sets, the inputs and the convergence tolerance are unchanged.

1. For every replacement event and every governing component in the plant at that shutdown, the decay curve comes
   from its own ACTINV run: that component installation's history from installation to the shutdown, with earlier
   outages as single zero-flux steps, then cooling on the 40-point grid to 365 days. The cooldown is read from these
   curves. No event is window-limited, so Amendment 1 item 3 no longer applies, except that a combined curve still
   above `q*` at 365 days makes the case NOT EVALUATED as before.
2. Amendment 1 item 2 is superseded for the cooldown. Retained and removed components alike have the full cooling grid
   after the shutdown.
3. The iteration limit is 10 (it was 5). The tolerance stays 1 day.
4. `q*` is calibrated by the same rule (Amendment 1 item 4) on curves obtained this way.
5. Before the second run, the driver checks on recorded inputs that a single zero-flux step and the same interval
   subdivided on the grid give the same decay heat to within 1e-6 relative. If not, the second run does not start.
6. The second run is reported beside the first, with its verdict computed by the same rules. The report names this
   amendment as written after the first result.

### Amendment 3, 2026-10-06, after the second result: correction to a citation

Found while checking sources for the validation protocol (`docs/notes/MAINTENANCE_COUPLING_VALIDATION.md`). The "Why
this test" section says EU-DEMO studies find about 15 days of cooling needed for the outboard blanket segments and
about 85 days for the inboard ones, citing the EUROfusion report WPPMI-CPR(18) 20248. That report (P. Frosi et al.,
"DEMO Breeding Blanket temperature evaluation before remote maintenance operation") does not contain those figures. It
evaluates blanket temperatures using decay heat one month after shutdown, against a 100 °C limit at the interface with
the remote-handling equipment. The source of the 15-day and 85-day figures is not known, so they are withdrawn. They
were an illustration only: no calculation, threshold or decision in this test used them, and neither verdict changes.
