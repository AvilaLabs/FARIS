# Result: does the maintenance coupling result hold up?

Recorded 2026-10-06. Protocol: `docs/notes/MAINTENANCE_COUPLING_VALIDATION.md` (frozen in `b3d70b7`; Amendment 1
records the body SHA-256 `a392e5ab35dc0d97f1af3ab5cef1e582317181057e0aa6cb8446d53839490602`). Record:
`references/maintenance-coupling-validation.json` (SHA-256
`6f658da99b2b0aae14c3b6ae94e1afc99f2127155b828daf92c8d8f3ca82a3a7`), local paths replaced by `<validation-dir>`,
`<actinv>`, `<actinv-data>` and `<nuclear-data>`.

## Summary

| Claim | Label | Holds in | Fails in | Not evaluated in |
| --- | --- | --- | --- | --- |
| C1, no-port pair swaps in D1 | **FRAGILE** | B, V3, V4, V5b, V5c, V5d | V2-low, V2-high, V5a | V1 |
| C2, port pair swaps in D1 | **FRAGILE** | B, V3, V4 | V2-low, V5a, V5b, V5c, V5d | V1, V2-high |
| C3, D3 changes | **INCOMPLETE** | B, V2-low, V2-high, V3, V4, V5a–V5d | none | V1 |
| C4, no-port breeder-minus-reference contrast at least 2× fixed | **INCOMPLETE** | B, V2-low, V2-high, V3, V4, V5a, V5c, V5d | none | V1, V5b |

In plain terms:

- **The port pair's ranking flip does not hold up.** It fails in 5 of the 7 variants where it was evaluated; it was
  the replacement-count step the amended result note described.
- **The no-port pair's ranking flip depends on how long outages are.** It holds in 6 of the 9 evaluated variants,
  including the new transport sample and three of the four service-limit changes, with the authored 60-day blanket
  replacement. It fails with the published EU-DEMO durations (V2, 4 to 7 months), where the fixed-duration model
  already ranks the breeder arrangement first, and with the blanket limit lowered by 20 %. The authored 60 days is
  close to the one public ARC figure ("a couple of months at most", a press interview, not a technical source), and
  V2 applies durations from a large plant to this compact one. So the flip holds for short outages and not for long
  ones; which applies depends on the plant.
- **Physics-derived durations change the downtime differences between designs in every variant that could be
  evaluated.** C3 never failed. The no-port breeder-minus-reference contrast is 2.1 to 14.2 times its fixed value
  wherever it is evaluated: 14.2 in the baseline, 2.1 and 2.4 with published durations, 3.8 to 7.2 with the service
  limits moved.
- C3 and C4 are INCOMPLETE, not ROBUST, because the dose-governed variant could not be evaluated (below).

## Per variant

| Variant | C1 | C2 | C3 | C4 | Notes |
| --- | --- | --- | --- | --- | --- |
| B, baseline | holds (5.6 %) | holds (2.4 %) | holds | holds (14.2×) | reproduced the amended run exactly |
| V1, dose-governed | not evaluated | not evaluated | not evaluated | not evaluated | no contact dose for the blanket |
| V2-low, 30 d + 3.2 months | fails, no swap | fails, no swap | holds | holds (2.1×) | fixed model already ranks breeder first |
| V2-high, 30 d + 5.9 months | fails, no swap | not evaluated | holds | holds (2.4×) | port/breeder did not converge in 10 iterations (last change 29.6 d) |
| V3, impurities | holds (5.6 %) | holds (2.4 %) | holds | holds (14.2×) | almost identical to B |
| V4, new transport sample | holds (5.6 %) | holds (2.4 %) | holds | holds (14.0×) | |
| V5a, blanket limit × 0.8 | fails, no swap | fails (1.8 %) | holds | holds (3.8×) | |
| V5b, blanket limit × 1.2 | holds (2.8 %) | fails (0.7 %) | holds | not evaluated | no-port fixed difference under 30 days |
| V5c, magnet limits × 0.8 | holds (4.5 %) | fails, no swap | holds | holds (4.0×) | |
| V5d, magnet limits × 1.2 | holds (3.0 %) | fails, no swap | holds | holds (7.2×) | |

## Points to know before relying on this

- **V1 is not evaluated, and that is a tool limit, not a physics result.** ACTINV returns no contact-dose proxy for
  the FLiBe blanket. Activated FLiBe emits light-element K X-rays (183 to 849 eV, from F-18, N-16, B-12 and others),
  below the 1 keV low end of the NIST response data. ACTINV refuses the proxy whenever any photon power falls outside
  the response's range, here about 6 parts in 10 million (`dose_response_power_coverage` 0.99999936). Such X-rays
  are absorbed within the material and add essentially nothing to contact dose, but the refusal is by design. Why:
  the blanket's dose curve is missing, so no blanket or magnet cooldown can be read from dose. Next step: an ACTINV
  change (count sub-keV photon power as self-absorbed, with a ledger entry, or extend the response below 1 keV),
  parked in ACTINV's `docs/PARKING.md`; then rerun V1 alone.
- **V3 is a weak test of impurities.** The verified specification limits leave out cobalt (except in iron), niobium
  and uranium, the elements that dominate activation, so adding them changes almost nothing. The 0.2 plan's per-ppm
  treatment of those elements would be the stronger test.
- **D3 with a missing contrast.** The driver counts D3 as changed when an evaluated contrast changes, even if another
  contrast is not evaluated (V2-high, where port/breeder did not converge). Amendment 1 item 10 of the coupling test
  says a decision with any not-evaluated input is not evaluated. Under that stricter reading V2-high's C3 is not
  evaluated; C3's label stays INCOMPLETE either way. The amended run is not affected: every D3 contrast in it was
  evaluated.
- **C4 in V5b.** With the blanket limit raised, the two no-port arrangements' fixed downtimes differ by less than 30
  days, so the ratio is not meaningful and the contrast is not considered.

## What was run

- Each variant ran the amended test's driver on the four arrangements at `w` = 0.50, with a shared content-addressed
  cache seeded from the amended run. Source: `0d021a0`, then `0692d99` for V1.
- V4's four transport runs: 2 million histories each, seeds 83150021, 83150022, 83150013, 83150014 (the amended
  run's seeds plus 1,000,000), 8 to 10 minutes each.
- V1's first attempt was stopped by its 5 GB memory cap. With photon outputs, a first-wall continuation's ACTINV
  result reaches 2.2 GB, and the driver parsed results whole. Commit `0692d99` reads results one step at a time with
  the same decoder; it was tested equal to the whole-file parse, and the points files are unchanged. V1 then resumed
  from its cache. The ACTINV pilot before the run had used the largest specification file rather than the longest
  history, so it missed this.
- Run times: B 3 min (from cache), heat variants 10 to 16 min each, V1 about 35 min in total.

## What this means for the claim

The amended test's MATERIAL verdict stands as recorded, but the part of it that holds up is narrower than "the
ranking changes". What survives every evaluated variant is this: computing maintenance durations from activation
changes the downtime differences between designs by large factors (2 to 14 times for the no-port blanket contrast).
Whether that changes which design wins depends on how long outages are: with short, ARC-like outages the no-port
ranking changes, with long, EU-DEMO-like outages it does not. Settling it needs a realistic geometry with that plant's
own maintenance durations, and the dose-governed check once ACTINV can give the blanket's dose.
