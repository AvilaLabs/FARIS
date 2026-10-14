# Run your own transport

You do not need this to explore the recorded study. Use it to run new fixed-source transport yourself, for example to record a case with your own seed or history count. It uses the same engine from the command line and from the desktop.

## What you need

- OpenMC 0.15.3 in an environment you provide. The run refuses any other version.
- The nuclear data the study was audited against. The recorded runs use FENDL-3.2 neutron data and ENDF/B-VII.1 photon data. The photon data come from a local overlay that FARIS does not ship. How to obtain and build it is described in [Nuclear data](nuclear-data.md).
- A library audit of your own installation. The audit file in the repository records one workstation's cache. Run `integrations/openmc/audit_library.py` on yours and supply its output. FARIS does not silently accept differences in library content.

The data stay outside the repository. They are not part of the package.

A second local library, v2, covers the full FENDL-3.2 neutron set, photon data for every element in it and ENDF/B-VIII.1 thermal-scattering tables for fusion materials. It is described in [Nuclear data](nuclear-data.md) and is not the default. FARIS records which library a run used, by the digests of its `cross_sections.xml` and its audit.

## Run a case

```bash
faris reactor run \
  --scenario scenarios/arc-inspired/cold-coupled-control.scenario.json \
  --physics scenarios/arc-inspired/cold-coupled-control.reference.physics.json \
  --mesh-preset outboard-local \
  --audit references/openmc-library-audit.json \
  --cross-sections /path/to/cross_sections.xml \
  --python /path/to/openmc-env/bin/python --openmc /path/to/openmc-env/bin/openmc \
  --particles 10000 --batches 100 --seed 123456789 --threads 1 \
  --output runs/cold-coupled-control-reference-001
```

The scenario and physics files shown are the port-free arrangement with the reference allocation. The port arrangements use `cold-reference-port.scenario.json` and its matching physics files.

To stop on an error instead of a fixed count, add `--target-error 0.05`. The run then stops as soon as the relative standard error of the magnet fast flux is at or below 5 %, but not before 30 batches, and `--batches` becomes the budget: the most batches it may use. The result says how many histories were sampled, the error reached and why it stopped. If the budget runs out first, the result says the target was not met; run again with a larger budget (up to the limits below) or more threads. The magnet fast flux is the hardest response in the model to sample, so a 5 % target on it needs millions of histories.

Add `--window-python /path/to/openmc-0.16-env/bin/python` and the target is reached in minutes instead of hours. That Python (OpenMC 0.16 or later, a separate environment from the 0.15.3 one that runs the transport) first makes weight windows aimed at the magnet fast flux: it condenses the library to 17 energy groups on the model's materials, solves the adjoint problem of the magnet fast flux by random ray, and writes windows on a 25 cm mesh. The run then splits and roulettes neutrons on them. The windows change how fast the error falls, not the answer; the same generator is used by `faris design transport`. On the demo scenario (3 threads) the windows take about two minutes, and the magnet fast flux then reached 5 % after 740,000 to 860,000 histories: 5 minutes from the command to the finished record the first time, 2.5 to 3 minutes when the windows are reused. An analog run needs tens of millions of histories for the same error (two 1.3 million history analog runs were at 29 % and 35 %). The windows are made once for a given model, materials, data and response and kept in `faris-transport-cache` beside `--output` (`--window-cache DIR` to put them elsewhere, `--no-window-cache` to make them for this run only), so a second run on the same model starts sampling at once.

With windows use small batches, for example `--batches 1000 --particles 20000`: a run never stops before its 30th batch, and 30 batches of 250,000 would already be 7.5 million histories. Without `--window-python`, or if making the windows fails, the run is analog as before and its record says so and why. A preview (no `--target-error`) is always analog.

The windows serve the magnet fast flux and nothing else. Other responses are sampled less well with them than without: on the demo the total tritium had a relative error of 44 to 64 % in the windowed runs, against 0.09 % in an analog run of 1.3 million histories, and its batch values were too uneven to trust the stated error (variance of the variance 0.3 to 0.9, against the 0.1 limit). The result says so beside the numbers. Take the breeding ratio and heating from a Preview or an analog run, and the magnet flux from the windowed one.

The windows are in the run directory as `windows.h5`, with the helper's record `windows-record.json`; `faris reactor inspect` checks them against the hashes in the input and reports whether the run used them.

The limits are:

- 30 to 1000 batches, and at most 50 million histories in all.
- At most 32 threads.
- A timeout of 3,600 seconds by default (`--timeout-seconds`), at most 14,400.
- `--output` must be a new directory. The command refuses an existing one.

`--mesh-preset` is `coarse` (the default), `outboard-local-coarse`, `outboard-local` or `outboard-port-window`. The recorded runs use the outboard-local mesh. Ctrl-C cancels the run and its solver processes. These limits bound resources. They are not a sandbox.

A run records the per-batch results behind the covariance that the [uncertainty ensembles](uncertainty.md) need. Small regions need many histories. At 10 million histories, the port-sector fast flux of a port-free control has a 36 to 54 % relative error. That is too poorly sampled for ensembles, which is why the recorded controls use 30 million. A 30 million history run took about 68 minutes at 7 threads on the reference laptop.

## Check a run

```bash
faris reactor inspect \
  --scenario scenarios/arc-inspired/cold-coupled-control.scenario.json \
  --run runs/cold-coupled-control-reference-001/run.json
```

This rechecks the run's recorded inputs, raw artifact, sampling, audit, adapter identity, volumes and normalization. Neither a completed process nor an accepted normalization is a scientific verdict. Scientific qualification stays not evaluated.

## Replay or run in the desktop

```bash
faris-app \
  --scenario scenarios/arc-inspired/cold-coupled-control.scenario.json \
  --physics scenarios/arc-inspired/cold-coupled-control.reference.physics.json \
  --physics scenarios/arc-inspired/cold-coupled-control.breeder-emphasis.physics.json \
  --run runs/cold-coupled-control-reference-001/run.json --field-view flux-slice
```

Repeat `--run` for the other arrangement. Add `--python`, `--openmc`, `--audit` and `--cross-sections` to enable **Run and review transport** in the Simulate step, or set them under **Transport configuration**. Under **Run quality**, **Preview** runs a fixed 90,000 histories, about a minute on two threads; it states the errors it reached, and the magnet fast flux is only roughly known at that size. **Production** stops once the magnet fast flux is known to 5 %, or when its budget is used, and says which. With a **Window Python** set (OpenMC 0.16 or later; the same setting as in the Design flux window, under **Transport configuration** here) Production first makes weight windows aimed at the magnet flux, so it finishes in minutes, and the budget is 20 million histories in batches of 20,000; the windows are kept in `faris-transport-cache` in the runs folder for the next run, and the result states how well the total tritium is known, because the windows leave it less precise. Without a Window Python it is analog, the budget is 50 million histories, and it can take hours; the result says that windows were not used and why. The app uses up to four threads, one fewer than the machine has. Closing the app cancels its transport workers. Camera interaction stays available while a job runs.

A study started from `--run` records cannot be saved as a `.faris` file, because a study file holds recorded-transport bundles. `faris transport pack` packages a completed run into a portable bundle:

```bash
faris transport pack --run runs/cold-coupled-control-reference-001/run.json --output reference.transport-bundle.json
```

## Where to read more

The repository documents the details: the [cold-reference workflow](https://github.com/AvilaLabs/FARIS/blob/main/docs/COLD_REFERENCE.md) covers the materials, the source, the recorded tallies and the verification steps, and the [transport boundary](https://github.com/AvilaLabs/FARIS/blob/main/docs/TRANSPORT.md) covers the contracts, normalization and trust limits.

Next: [Reading the numbers](results.md).
