# Restart continuation method: equivalence check

Recorded 2026-10-07. Code: `continuation_method` in `scripts/maintenance_coupling_test.py` (default `"full"`),
checked by `scripts/restart_equivalence.py`.

## What it does

The full method runs one ACTINV history per (event, governing component installation): irradiation from installation
to the event, then the cooling grid. The restart method runs one trunk history per installation and starts each
event's cooling run from the trunk's inventory at that shutdown (`atoms_per_g`). The cooling runs use `"mode":
"coupled"`; in auto mode with zero flux ACTINV treats the starting material as a constant trace, which held the
inventory fixed and gave curves about 100 % off in the first check.

## Result

`restart_equivalence.py` rebuilt the curves of five recorded runs with the restart method and compared them point by
point with the full method's cached curves (tolerance 1e-6 relative):

| Set | Curves | Worst heat difference | Worst dose difference | Within 1e-6 |
| --- | --- | --- | --- | --- |
| 1 | 28 | 2.3e-16 | no dose | yes |
| 2 | 22 | 0 | no dose | yes |
| 3 | 114 | 8.5e-7 | no dose | yes |
| 4 | 116 | 8.5e-7 | no dose | yes |
| 5 | 138 | 8.3e-7 | 5.4e-4 | **no** |

- **Heat matches.** Every heat curve agrees within 8.5e-7.
- **Dose does not match to 1e-6.** In set 5, the only set with dose curves, first-wall dose differs by up to 5.4e-4 at
  late shutdowns. Those trunks have about 1,030 states below ACTINV's output floor, and the restart inventory leaves
  them out. ACTINV bounds their heat (`heat_bound_from_below_floor_W_per_g`, at most 2e-9 W/g here) but gives no
  equivalent bound for dose.

## Status

The method stays opt-in and the full method stays the default. It is fit for heat-governed runs. It is not fit for
dose-governed runs until the below-floor states are carried into the restart (or ACTINV bounds their dose). The speed
gain in the trial runs was small, because each cooling run still pays ACTINV's start-up and data load. Batching the
cooling runs into one ACTINV call (for example through `actinv mesh`) is the next thing to try for speed.
