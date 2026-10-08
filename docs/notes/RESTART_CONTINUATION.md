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
  late shutdowns; vessel and shield stay within 8e-7. ACTINV's contact-dose proxy takes the material's attenuation from
  the run's starting composition. A full-history run starts from pristine tungsten. A restart run starts from the
  transmuted metal, which after decades is about 10 % rhenium, tantalum and osmium by mass. The restart value uses the
  real composition at shutdown; the difference is in a screening quantity, not an error in the inventory.

## Status

The method stays opt-in and the full method stays the default. Heat-governed runs can use either. Dose-governed reruns
under a frozen protocol keep the full method so they stay comparable with earlier variants; new dose work can use the
restart method, whose attenuation follows the transmuted material.

Speed: baseline B from an empty cache took 8 min 54 s with the restart method and reproduced the full method's verdict
and decisions within about 1e-9. The gain over the full method was only about 1.2×, because a cooling run takes about
0.85 s and most of that is ACTINV loading its data. Batching the cooling runs into one ACTINV call (for example through
`actinv mesh`, which loads the data once) is the next thing to try for speed.
