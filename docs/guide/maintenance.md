# Computed maintenance durations

> **Available from FARIS 0.2.** The 0.1.1 download does not include it. Until 0.2 is released, build FARIS from the
> `main` branch to use it.

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
- **Python 3** for FARIS's activation-input builder, which writes the ACTINV inputs.
- **Per design**: a scenario, its physics file, a verified transport run for the history, a 709-group spectrum run
  and operating-history assumptions. These are the same files the [history](operate.md) and
  [transport](transport.md) chapters use.

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

Open a result with **Maintenance → Open result…**, or start FARIS with `--maintenance result.json`.

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
