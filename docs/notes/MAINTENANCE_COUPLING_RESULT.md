# Result: does physics-derived maintenance time change the answers?

Recorded 2026-10-06. Protocol: `docs/notes/MAINTENANCE_COUPLING_TEST.md` (body SHA-256
`f287acd51542ffa4bd5d78eaad07912e35dacc6b4f63d14ce12616eb5a3440f2`, with Amendment 1).

## Verdict: NOT EVALUATED

The script's verdict is **NOT EVALUATED**. No decision could be evaluated at any `w`, because most coupled cases did
not converge within the protocol's 5 iterations:

| `w` | Cases converged (of 17) | D1 | D2 | D3 | D4 |
| --- | --- | --- | --- | --- | --- |
| 0.25 | 0 | not evaluated | not evaluated | not evaluated | not evaluated |
| 0.50 | 1 (port/breeder) | not evaluated | not evaluated | not evaluated | not evaluated |
| 0.75 | 1 | not evaluated | not evaluated | not evaluated | not evaluated |

Why: each decision compares cases, and a decision with any unconverged case is not evaluated (Amendment 1, item 10).
Next step: the method change described under "What would let it converge" below. This verdict stands as recorded
whatever a later run shows.

## What was run

- **Transport.** The recorded 0.1.0 runs set the histories and the component flux. Eleven new runs (2 million
  histories each, seeds offset from 0.1.0) gave the 709-group spectrum shapes, scaled to the recorded flux. Each new
  run has the same scenario and physics hash as the run it pairs with. Spectrum sampling error (flux-weighted mean
  relative error per component) is below 0.5 % for the first wall and blanket. For the magnets it is 47 %, but the
  magnets are in neither governing set, so it does not enter any duration.
- **Activation.** ACTINV 1.4.0 with TENDL-2025 (709 groups), bare transport materials only, heat-only outputs.
  Calibrated `q*` (W/m³) at `w` = 0.25 / 0.50 / 0.75: blanket 3,705 / 4,196 / 4,828; magnet 1,699 / 2,179 / 2,813.
- **Contact dose.** The cross-check is not evaluated: the ACTINV specs have no photon response (Amendment 1, item 11).
- **Run.** About 7,500 ACTINV runs, 2 h 36 min. The driver was stopped once at 04:14 before it ran out of memory,
  fixed to keep fewer histories in memory (commit `fcfedf3`, which changes memory use and restarting only), and resumed
  from its saved outputs with the same configuration. The result records `"run": {"resumes": 1}`.
- **Record.** `references/maintenance-coupling-test.json` is the result with local paths replaced by `<run-dir>`,
  `<actinv>` and `<actinv-data>` (SHA-256 `a07c42c1e5a37e1cf2002ace7322b6aedf7e708ad46c5cca0b0ba2e48b495d00`).
  The unedited file has SHA-256 `2295b5cfc45ca060094f79484fd63ad557c1c135a275169845f11dff370678ee`.

## Why the cases did not converge

The method could not see far enough along the decay curves.

1. **The decay curve stops when the plant restarts.** A component that stays in the plant is irradiated again when the
   outage ends, so its decay curve is known only for the length of the outage. When the needed cooldown is longer,
   the event is window-limited (Amendment 1, item 3): its cooldown is taken as the outage length, and the next
   iteration lengthens the outage by only the work time, 30 days at `w` = 0.50. An event that needs several months
   more than its outage needs several iterations just to see its crossing. At the fixed durations, 25 of the 39
   events in port/reference were window-limited.
2. **The number of replacements changes.** Longer outages leave less operating time, so fewer replacements fit in 30
   years. Each change shifts the timeline and restarts the comparison for the later events.

The iterations do settle once the curves are visible. Port/breeder converged at iteration 5 with a largest change of
0.4 days. Port/reference ended with one window-limited blanket event still growing by 30 days per iteration.

## What the unconverged results show

This is an observation from the last iteration, not one of the protocol's decisions, and it is not a verdict.

The needed cooldown grows with plant age. The first wall, shield and vessel are never replaced, so long-lived activation
builds up in them. In port/reference at `w` = 0.50, the cooldown before a magnet replacement is 60 days at year 0.7 (the
calibration event) and rises to about 150 to 180 days by years 20 to 30. Blanket replacements rise from 35 days to more
than 150 days. Many computed durations are lower bounds, because the iterations stopped while those outages were still
growing. A fixed-duration model, as in FARIS 0.1, PROCESS or bluemira, gives every replacement the same duration. It
cannot show this trend.

Whether the trend changes a design decision is exactly what the test asks, and it is not answered yet.

## What would let it converge

A method change, written as an amendment after this result and marked as such:

- **Full decay curves.** For every shutdown, run the history up to that shutdown and then cool on the 40-point grid
  for 365 days. The cooldown is then read from the curve itself, and no event is window-limited.
- **More iterations.** Allow up to 10, because event counts settle within a few iterations once durations are seen
  directly.

`q*`, the `w` grid, the decisions and their thresholds stay as they are. The amended run would be reported beside this
one, and this NOT EVALUATED verdict stays in the record.

## Amended run (Amendment 2): MATERIAL

Recorded 2026-10-06. Amendment 2 was written after the result above and is marked as such in the protocol. It changes
how the decay curves are read and allows 10 iterations; `q*`, the `w` grid, the decisions and their thresholds are
unchanged. The NOT EVALUATED verdict above stays in the record.

The script's verdict is **MATERIAL**: D1 and D3 change at the central `w` = 0.50, and also at 0.25 and 0.75.

| `w` | Cases converged (of 17) | D1 | D2 | D3 | D4 |
| --- | --- | --- | --- | --- | --- |
| 0.25 | 17 | changed | not changed | changed | not changed |
| 0.50 | 17 | changed | not changed | changed | not changed |
| 0.75 | 16 | changed | not evaluated | changed | not changed |

At `w` = 0.75 the 35 cm shield case did not converge in 10 iterations (last change 4.8 days), so D2 is not evaluated
there. Why: D2 compares every shield thickness, and one is missing. Next step: none needed for the verdict, which is
settled at `w` = 0.50; D2 is unchanged (45 cm) wherever it was evaluated.

### What changed

- **D1, ranking of the arrangements by lifetime net electricity.** Fixed durations rank no-port/reference,
  no-port/breeder, port/reference, port/breeder. Computed durations swap both pairs. The no-port swap has a gap of 5.4
  to 6.1 % (threshold 2 %). The port swap has a gap of 2.37 % at every `w`. It comes from the replacement count: with
  computed durations both port arrangements fit 32 replacements (fixed: 39 and 37). Replacements are triggered by
  fluence, which grows with full-power time, and in both port arrangements the 30-year horizon ends during the last
  outage. Full-power time is then fixed by the count: 14.311 years (reference) and 14.677 years (breeder) at every
  `w`. That is why the port lifetimes, and this gap, are the same at all three `w`.
- **D3, size of the design differences in replacement downtime.** Three of four contrasts leave the 0.8 to 1.25 band:

  | Contrast | Ratio computed / fixed at `w` = 0.25 / 0.50 / 0.75 |
  | --- | --- |
  | breeder minus reference, no port | 14.5 / 14.2 / 14.2 |
  | breeder minus reference, port | 0.48 / 0.09 / 0.07 |
  | port minus no port, reference | 0.79 / 0.78 / 0.79 |
  | port minus no port, breeder | 0.97 / 0.97 / 0.98 (inside the band) |

  The 14x contrast at `w` = 0.50: with fixed durations every no-port blanket outage is 60 days, and the two
  arrangements differ by 40 days in total. With computed durations the reference blanket outages grow from 154 to 295
  days over the life (11 replacements, 2,645 days) and the breeder ones from 127 to 289 days (10 replacements, 2,077
  days), a difference of 568 days. Both the shorter outages and the one fewer replacement contribute.
- **D2, best shield thickness:** 45 cm with either model. **D4, best operating fraction:** the computed optimum moves
  (0.9 or 1.0 against 0.8), but its gain over `f` = 1.0 is below the 1 % threshold, so the decision does not change.

### Why the no-port outages are long

The blanket `q*` is calibrated on the first blanket replacement in port/reference, so that event takes exactly its
fixed 60 days. The no-port arrangements reach that heat level later: the first no-port reference blanket replacement
(year 2.0) needs 124 days of cooling, against 35 days in port/reference (year 3.2) and 4 days in port/breeder. The
no-port blankets are replaced more often (every 2.0 to 2.5 years against 3.2 to 4.1), which points to a higher flux
on them, and so more activity at shutdown. This is read from the computed results; no separate check of the flux was
made.

### What was run

- **Method.** Amendment 2: for every shutdown and governing component, the history up to that shutdown, then 40
  cooling points to 365 days; results cached by the SHA-256 of the input. Equivalence check (single zero-flux step
  against the subdivided outage): largest relative difference 9.2e-9, tolerance 1e-6, passed.
- **Run.** 26,054 ACTINV runs, 9,900 served from the cache, 3 h 33 min. The driver was started single-threaded, stopped
  after 20 minutes, changed to run 4 ACTINV processes at once and to run `w` = 0.50 first (commit `0555603`, which
  changes speed and order only; a test checks that 1 and 4 workers give the same result), and resumed with the same
  configuration. The result records `"run": {"resumes": 1}`.
- **Record.** `references/maintenance-coupling-test-a2.json`, local paths replaced by `<run-dir>`, `<actinv>` and
  `<actinv-data>` (SHA-256 `deee2e67fdce9db02a7e1c4e98617dd78731ce8b168dc6814719ea4b412e2328`; unedited file
  `0502f6582d912ab0fdbae0fe8b6ceb8ce162d26c45a2c5fd220a87ca3a698440`).

### What this answers, and what it does not

Within this test, physics-derived maintenance durations change which arrangement delivers the most electricity and
change the size of the downtime differences between designs by large factors. They do not change the best shield
thickness or the best operating fraction. The calibration ties every duration to one authored number per class, so
the result is about how durations differ between designs and over the plant's life, not about their absolute size.
