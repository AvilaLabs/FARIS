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
