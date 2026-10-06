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

None yet.
