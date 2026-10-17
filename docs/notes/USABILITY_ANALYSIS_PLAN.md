<!-- kind: reference -->
# Usability benchmark: analysis plan

Written before the first session (requirements UX-070, UX-074). The protocol, tasks, success criteria,
exclusion rules and this plan are hashed and committed before session 1; later changes are appended with a
reason, never edited. The implementation is `scripts/usability_study.py`, tested on synthetic data in
`scripts/test_usability_study.py`.

## What is computed

| Measure | Statistic | Interval |
| --- | --- | --- |
| Task success (per task) | success rate | adjusted-Wald 95 % interval (add z²/2 successes and z² trials, z = 1.96) |
| Time on task (successful attempts) | geometric mean, median, P90 | percentile bootstrap, 10,000 resamples, fixed seed, 95 % |
| SUS | mean, sample SD | t-interval, n − 1 degrees of freedom, 95 % |

Time on task is treated as log-normal, so the geometric mean is the headline and the median and P90 are
reported beside it. P90 uses linear interpolation between order statistics.

## SUS reporting

Every SUS value is printed as `SUS n=…, mean=…, SD=…, 95% CI [low, high]`. A sample of fewer than 12 carries
the label "formative, not a benchmark". A bare mean is refused by the code, and `usability_study.py lint`
finds one in a report text.

The verdict against UX-001 is computed from the raw scores: at F1 mean ≥ 74 and lower bound ≥ 68, at F6
mean ≥ 80.3 and lower bound ≥ 74, in both cases with n ≥ 20. With n < 20 the verdict is INCONCLUSIVE.

## Drops

No result is dropped except by a rule in `exclusions.json`, which is part of the hashed protocol. Every drop
is listed in the report with the rule and its reason.

## Raw data

Raw responses, timings and scoring files are registered with `usability_study.py register`: each is stored
under its SHA-256, the ledger is append-only, and a name registered again with other bytes is refused. The
report numbers and the verdict are regenerated from the stored raw files; `usability_study.py check` fails
when the stored report differs from a regeneration. Run the script on a dry-run dataset before session 1.
