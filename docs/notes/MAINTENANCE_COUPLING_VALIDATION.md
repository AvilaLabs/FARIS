# Validation: does the maintenance coupling result hold up?

Recorded 2026-10-06, before any validation run. Connor approved this protocol on 2026-10-06. It is fixed;
amendments are appended at the end with a date and a reason, and never edit the text above them. Its body SHA-256
(the text before "## Amendments") is recorded in the commit that adds this version. V2's values were set from the
literature review (`docs/notes/MAINTENANCE_LITERATURE_2026-10.md`), never from a validation run.

## Why this test

The amended maintenance coupling test (`docs/notes/MAINTENANCE_COUPLING_RESULT.md`, commit `8732406`) returned
MATERIAL: physics-derived maintenance durations change the ranking of the four arrangements (D1) and the size of the
downtime contrasts (D3), at every `w`. Four limits stand between that result and a claim FARIS can make to others:

1. Cooldown is governed by decay heat density. Remote handling is often limited by gamma dose rate instead.
2. Every duration is calibrated to one authored number per class at one event, so absolute durations are not tested.
3. One set of operating assumptions, bare materials and one transport sample were used.
4. The port-pair swap in D1 (gap 2.37 %) comes from a step in the replacement count (32 against 39 and 37), so it may
   not survive small changes to the assumptions.

This test checks whether the result survives each of these. It does not re-decide the amended test's verdict, which
stands as recorded.

## What is checked

The claims under test, each read from the amended run at `w` = 0.50:

- **C1.** The no-port pair swaps in D1 (no-port/breeder ahead of no-port/reference), gap above 2 %.
- **C2.** The port pair swaps in D1 (port/breeder ahead of port/reference), gap above 2 %.
- **C3.** D3 changes: at least one contrast leaves [0.8, 1.25], with the same rules as the amended test.
- **C4.** The no-port breeder-minus-reference downtime contrast is at least 2 times the fixed one (14.2 in the
  amended run). This is the effect the result note describes; 2 is set so that "much larger than fixed" is tested,
  not the exact factor.

Each claim is evaluated in each variant below with the amended test's driver, rules and thresholds (Amendments 1
and 2 included), changing only what the variant names. Only the four recorded arrangements are run (D2 and D4 are
not re-tested; they did not change), at `w` = 0.50 unless a variant sets its own split.

## Variants

**B. Baseline reproduction.** The amended run's configuration, restricted to the four arrangements at `w` = 0.50.
It must reproduce the amended run's durations, lifetimes and D1/D3 tables exactly (same ACTINV inputs, same cache).
If it does not, no other variant is evaluated and the difference is reported.

**V1. Dose-governed cooldown.** The governing quantity is ACTINV's contact gamma dose proxy
(`contact_gamma_air_dose_proxy_Gy_h`) of the governing components, combined by volume as the heat is now, instead of
decay heat density. `q*` is calibrated by the same rule on the dose curves. Requires ACTINV `outputs` with `dose`
and a photon response covering every element in the materials, built with ACTINV's
`scripts/build_photon_response.py` from the NIST tables named in ACTINV's `docs/DATA.md`. The proxy is a
semi-infinite-slab screening quantity, not a transported dose rate; V1 tests whether the trend over events and
between arrangements differs from the heat trend, not absolute dose.

**V2. Published durations.** Replaces the authored split and anchor with published EU-DEMO ones, which are the
most documented. The `w` grid is not used; each class gets a published cooldown and a published work time.

- **Blanket class, cooldown at the calibration event** (first blanket replacement in port/reference): 30 days. EU-DEMO
  work uses a fixed one-month cooldown before remote handling starts: the Crofts and Harman estimate below, and
  PROCESS's availability model 2, whose scaling comes from a 2014 EUROfusion RAMI report (`process/models/availability.py`, "the +2.0 at the end is for the 1 month cooldown and pump
  down at either end"). `q*` is calibrated so that event's cooldown is 30 days. The other published value found,
  about 24 hours in ARIES-AT (Waganer, "ARIES-AT maintenance system definition and analysis"), is for a
  low-activation advanced design and is not used.
- **Blanket class, work:** the EU-DEMO estimate of the time to replace all blankets and divertor cassettes, 3.2
  months with 8 remote-handling systems in parallel and 5.9 months with 4 (O. Crofts, J. Harman, "Maintenance
  duration estimate for a DEMO fusion power plant, based on the EFDA WP12 pre-conceptual studies", Fusion Eng. Des.
  89 (2014), [arXiv:1412.4008](https://arxiv.org/abs/1412.4008), Table 1; read in full on 2026-10-06). A month is
  30.4375 days.
- **Magnet class:** no published duration exists for a comparable event. The only ARC figure for a vessel swap, "a
  couple of months at most", is from a press interview, not a technical source. The magnet class keeps the amended
  test's values at `w` = 0.50, and the report says so.

Two runs: **V2-low** (30 days cooldown, 3.2 months work) and **V2-high** (30 days cooldown, 5.9 months work). These are far longer than the authored 60-day blanket replacement, so the
timeline, replacement counts and lifetimes all move; that is the point of the variant.

**V3. Impurities.** The specification-maximum impurity materials from the 0.2 decisions in `docs/DEMO_ROADMAP.md`,
the amended test's secondary case, run in place of the bare materials.

**V4. Transport sample.** New 709-group spectrum runs (2 million histories each) with seeds offset again by the same
fixed amount, so the spectrum shapes are an independent sample. Histories and component flux are unchanged.

**V5. Service limits.** The fluence limits that trigger replacement in
`scenarios/arc-inspired/demountable-magnet-assumptions.json`, one class at a time: the blanket limit (1e26 n/m²)
× 0.8 and × 1.2, and the three magnet fast-fluence limits (3e22 n/m² each, scaled together) × 0.8 and × 1.2
(V5a–V5d). The blanket limit is an authored trip threshold and the magnet limits are literature-anchored screening
values; ±20 % is a modest change to either, not a statement of their uncertainty. These move the replacement counts, which is what C2 depends on.

## Verdict rules

Evaluated by script from the recorded outputs, not by judgement.

- In each variant, each claim is HOLDS, FAILS or NOT EVALUATED. NOT EVALUATED applies when a case does not converge
  in 10 iterations or a curve stays above `q*` for 365 days; it carries the reason and the case.
- A claim is **ROBUST** if it HOLDS in every variant. It is **FRAGILE** if it FAILS in any variant; the report names
  each variant where it fails. It is **INCOMPLETE** if it never FAILS but is NOT EVALUATED somewhere.
- For C2, each variant also records both port arrangements' replacement counts under the fixed and computed models,
  so the report can say whether a swap or failure follows the count.
- The overall result is the list of claims with their labels. There is no single combined verdict.

## What this test does not show

- V1 uses a screening proxy. A transported shutdown dose rate (R2S) for a few events would be the next check if V1
  and the heat result disagree.
- V2 uses published durations from plants whose design differs from this idealised torus. It tests whether the
  result depends on the authored split and anchor, not whether FARIS predicts a real plant's outage.
- The geometry is still the 0.1 idealised torus with surrogate materials.

## Running it

- Each variant is a separate configuration and output folder under `runs/maintenance-validation/` (gitignored), run
  with the amended test's driver at a recorded commit, 4 ACTINV workers, under the laptop memory cap.
- Before V1, a pilot runs the largest continuation spec with `dose` output under the cap and records time and peak
  memory. If it cannot run within the cap, V1 is NOT EVALUATED with that reason, and the others proceed.
- Expected cost: about 4 of the amended run's 51 coupled cases per variant, roughly 20 to 40 minutes each at the
  amended run's rate; V4 adds 11 transport runs.

## Outputs

`references/maintenance-coupling-validation.json` records each variant's inputs' hashes, `q*`, durations, D1/D3
tables, claim outcomes and the summary, with local paths replaced as in the amended record.
`docs/notes/MAINTENANCE_COUPLING_VALIDATION_RESULT.md` reports it in plain language.

## Amendments

None yet.
