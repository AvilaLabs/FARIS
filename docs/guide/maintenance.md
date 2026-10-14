<!-- kind: explanation -->
# Computed maintenance durations

> **New in FARIS 0.2.** The download includes a recorded result for the four demo arrangements: open
> **Maintenance** in the top bar and choose **Open the recorded maintenance result**.

Most plant models give every replacement the same fixed outage length. In a real plant the length depends on how
long the parts around the replaced component take to cool down, and that grows as the parts that stay in the machine
collect long-lived activation. FARIS can compute each replacement's outage from the decay heat of those parts and
compare the result with the fixed durations, design by design.

## What FARIS computes

For each design and each replacement over the plant's life:

1. **Cooldown.** FARIS asks ACTINV for the decay heat of every *governing component* (the parts that must cool
   before work can start) as they are at that shutdown, after the irradiation history up to it. The governing set's
   decay heat per volume is followed for up to 365 days. The cooldown ends when it falls below the class threshold
   **q\***.
2. **Outage.** The outage is the cooldown plus the class's work time.
3. **Iteration.** Longer outages shift every later event and change the irradiation history, so FARIS reruns the
   operating history with the new outages and repeats until no outage changes by more than a day (at most 10
   passes).

It then reports, under both the fixed and the computed model, every design's total replacement downtime,
availability and lifetime net electricity, plus the downtime difference between each pair of designs.

The comparison makes no ranking claim. In the validation runs, computed durations changed the downtime differences
between designs by factors of 2 to 14. Whether they change which design produces more electricity depended on how long
outages are assumed to be.

## What you need

- **ACTINV** and its nuclear data. See [actinv.avilalabs.org](https://actinv.avilalabs.org).
- **Python 3** for FARIS's activation-input builder, which writes the ACTINV inputs. The package has it as
  `tools/build_activation_inputs.py`, and FARIS finds it there.
- **Per design**: a scenario, its physics file, a verified transport run for the history, a 709-group spectrum run
  and operating-history assumptions. These are the same files the [history](operate.md) and
  [transport](transport.md) chapters use. The download does not hold the demo arrangements' 709-group spectrum runs,
  so recomputing needs your own [transport runs](transport.md).

### The designs file

A designs file names each design and its five files. Paths are relative to the file.

```json
{
  "schema_version": "faris-maintenance-designs/v0.1",
  "designs": {
    "port__reference": {
      "scenario": "scenarios/arc-inspired/cold-reference-port.scenario.json",
      "physics": "scenarios/arc-inspired/cold-reference-port.reference.physics.json",
      "history_run": "runs/port-reference/run.json",
      "spectrum_run": "runs/port-reference-709/run.json",
      "history_assumptions": "scenarios/arc-inspired/demountable-magnet-assumptions.json"
    }
  }
}
```

### The maintenance assumptions file

One entry per replacement class: the replaced component, its governing components, the work time and the threshold.

```json
{
  "schema_version": "faris-maintenance-assumptions/v0.1",
  "governing_quantity": "heat",
  "classes": {
    "blanket": {
      "component_id": "blanket",
      "governing": ["first-wall", "blanket"],
      "work_s": 2592000,
      "threshold": {"kind": "calibrate", "design": "port__reference", "target_cooldown_s": 2592000}
    },
    "magnet": {
      "component_id": "magnets",
      "governing": ["first-wall", "blanket", "shield", "vessel"],
      "work_s": 5184000,
      "threshold": {"kind": "q_star", "q_star_w_per_m3": 2179}
    }
  },
  "cooling": {"min_s": 3600, "max_s": 31536000, "points": 40},
  "max_iterations": 10,
  "convergence_s": 86400
}
```

A threshold is set in one of two ways:

- **`calibrate`**: q\* is chosen so that this class's first replacement in the named design, at the fixed durations,
  cools down in exactly `target_cooldown_s`. Durations are then relative to that one outage: FARIS shows how they
  differ between designs and over the plant's life, not how long they are in absolute terms.
- **`q_star`**: a threshold in W/m³ that you supply, for example from a plant's remote-handling limits.

The fixed duration of each class is the `replacement_duration_s` in that design's operating-history assumptions.
`cooling`, `max_iterations` and `convergence_s` may be left out; the values above are the defaults.

## Run it from the desktop

Open **Maintenance** in the top bar, then **Run…**:

1. Choose the designs file and the maintenance assumptions file.
2. Choose the `actinv` executable and the ACTINV data directory (the folder above the catalogue version). FARIS
   remembers them until you close it. You can also pass them at launch with `--actinv` and `--actinv-data`.
3. Optionally choose the result file. By default it is `maintenance-result.json` next to the designs file; an
   existing name gets a number, so nothing is overwritten.
4. Press **Run**.

Anything missing is listed under the form with what to do next. While it runs, the window shows the design,
the pass, ACTINV runs and cached curves, and the elapsed time. **Cancel** stops ACTINV and writes no result. You can
close the window and reopen it to check progress. When the run finishes, the result opens in the window.

ACTINV results are cached in `maintenance-work` next to the designs file, keyed by the exact ACTINV input, so a
rerun with unchanged inputs takes seconds.

## Run it from the command line

```
faris maintenance run --designs designs.json --assumptions maintenance.json \
  --actinv /path/to/actinv --data-dir /path/to/actinv-data --output result.json [--workers 2]
faris maintenance report result.json
```

`run` refuses an existing output file. Its work folder defaults to `result.json.work`; `--cache` points several runs at
one cache. `--impurities` uses the specification-maximum impurity variant instead of the bare materials. `report`
prints markdown tables, or the result itself with `--format json`. See [Command line](cli.md) for the other commands.

## Read the result

Open a result with **Maintenance → Open result…**, or start FARIS with `--maintenance result.json`. In the
downloaded package, **Open the recorded maintenance result** opens the result recorded for the four demo
arrangements after checking its SHA-256. It was computed with ACTINV at the coupling test's settings: each class's
work time and calibration target are half its fixed duration, with q\* calibrated on the `reference-port`
arrangement, and the "Demountable magnets" operating assumptions (in the package as
`maintenance/operating-assumptions.json`).

- **Thresholds**: q\* per class and where it came from.
- **Designs**: downtime, availability and lifetime net electricity, fixed beside computed, and whether the computed
  model could be evaluated.
- **Replacement outages**: per replacement, a grey bar for the fixed duration beside the computed one, split into
  work (blue) and cooldown (orange), against plant time.
- **Replacements**: every replacement with its start, both durations, and *why*: each governing component's share of
  the decay heat when the cooldown ends, and how long it had been in the machine. A cooldown that grows from one
  replacement to the next is usually explained here by a part that is never replaced. In the validation designs
  the first wall gave essentially all of it.
- **Between designs**: the downtime difference under each model and their ratio. The ratio is left out when the fixed
  difference is under 30 days, where it would not mean much.

A result marked *not evaluated* says why and what to try next. Common reasons:

- The decay heat never falls below q\* within 365 days. Raise q\* or reconsider which components govern.
- The outages still change after the last pass. Raise `max_iterations`, or look at the last two passes in
  **All iterations** for durations that alternate.
- A cooldown is cut short by the next restart (*window-limited*), so it is only a lower bound.

## Literature durations and remote-handling limits

Computed durations are one choice. FARIS also carries published ones, labelled **literature** wherever they
appear, with the source link and the conditions they were derived under. They are inputs taken from a paper, not
FARIS results.

| Preset | Whole outage | Conditions |
| --- | --- | --- |
| EU-DEMO blankets and divertor, 4 remote-handling systems | 7.9 months | 5.9 months of handling (Table 1, p. 5; the paper's "about 6 months") + 1 month cooling + 1 month conditioning and pump-down (Sec. 5.2) |
| EU-DEMO blankets and divertor, 8 systems | 5.2 months | 3.2 months of handling, the shortest in Table 1, + the same 2 months |
| EU-DEMO blankets and divertor, 1 system | 24 months | 22 months of handling, one system working all 16 sectors in turn (about 1,000 hours per sector, p. 4), + the same 2 months |

All three come from Crofts and Harman, Fusion Eng. Des. 89 (2014), [arXiv:1412.4008](https://arxiv.org/pdf/1412.4008),
read in full. Each replaces all blanket segments and divertor cassettes of the 2012 EU-DEMO (5 segments and 3 cassettes
in each of 16 sectors) through all 16 upper and 16 divertor ports; replacing the blankets alone takes as long, because
the divertor must come out first (p. 4). The handling months already include the paper's equipment-failure recovery
(about 20 % of the total, p. 5), transporter delays and shift and operator productivity. The paper states no
percentage contingency, so the presets add none and say so.

**Remote-handling constraints.** The outage length follows the number of parallel remote-handling systems, ports and
crews. Handling time is a fixed part plus the shared work divided by the systems that can work at once, which is the
smallest of the systems, the ports and the crews; cooling, conditioning and contingency are added. The model
assumes the systems share the work evenly and never fail or wait for each other. Fitted to the five points in
Table 1 of the paper (22, 11, 5.9, 4.4 and 3.2 months for 1, 2, 4, 6 and 8 systems), it stays within 10 % of each.
The fit is a calibration to those points, not a derivation from the paper's handling sequence.

**Sensitivity to each duration.** For a chosen range of each replacement duration, FARIS reruns the history at the
low and high value of one duration at a time and reports lifetime net electricity and availability for each, largest
effect first (a tornado). Availability here is one minus the replacement downtime over the history. Net electricity
is absent, with the reason, when the history lacks its energy inputs.

## Limits

- **Decay heat governs; contact dose does not.** With dose as the governing quantity and a threshold fixed at the
  first outage, the never-replaced first wall kept the dose above it for more than a year at later replacements in the
  validation designs. FARIS therefore refuses `"governing_quantity": "dose"` for now. Testing a remote-handling dose
  rule needs its own validation.
- **Screening, not a maintenance plan.** Work times, governing sets and thresholds are your assumptions. FARIS does not
  model crews, spare parts, queues or remote-handling equipment.
- **Run time.** The calculation runs ACTINV once per replacement and governing component, and again on each pass. A
  four-design, 30-year case took about ten minutes from an empty cache and about a minute from a full one on a laptop.
- **Checked against the validation test.** On the maintenance coupling test's four designs, FARIS reproduces the
  reference calculation's outages, downtimes and lifetime electricity to 1 part in 10¹⁴. The validation itself is
  described in the repository's `docs/notes/MAINTENANCE_COUPLING_VALIDATION_RESULT.md`.
