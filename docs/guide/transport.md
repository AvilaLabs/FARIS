<!-- kind: how-to -->
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

Add `--window-python /path/to/openmc-0.16-env/bin/python` and the target is reached in minutes instead of hours. That Python (OpenMC 0.16 or later, a separate environment from the 0.15.3 one that runs the transport) first makes weight windows aimed at the magnet fast flux: it condenses the library to 17 energy groups on the model's materials, solves the adjoint problem of the magnet fast flux by random ray, and writes windows on a 25 cm mesh. The run then splits and roulettes neutrons on them. The windows change how fast the error falls, not the answer; the same generator is used by `faris design transport`. The windows are made once for a given model, materials, data and response and kept in `faris-transport-cache` beside `--output` (`--window-cache DIR` to put them elsewhere, `--no-window-cache` to make them for this run only), so a second run on the same model starts sampling at once.

A windowed run is two passes, because the windows serve one response and are not shown to leave the others unbiased at this precision (on the demo the total tritium was far noisier with them, 44 to 64 % against 0.09 % without). The **analog pass** is an ordinary run that samples until every whole-model response (total tritium and the heating family) is at the target; it gives every response except the magnet fast flux. The **windowed pass** is a complete run of its own in `windowed-pass/` that samples the magnet fast flux with the windows until it is known to the target; it gives that one response. The record, `faris reactor inspect` (a `pass` on every response and a `windowed_pass` section), the CLI summary and the result view say which pass each response came from. The responses deep behind the blanket (the vessel, the coil layer apart from the fast flux) are not watched by the analog pass, which cannot reach the target on them; they are reported with the error the pass reached, which can be large.

On the demo scenario (3 threads) a windowed run took 6.4 minutes from the command to the finished record the first time (the windows 114 s, the analog pass 300,000 histories, the windowed pass 940,000 histories to 4.9 % on the magnet fast flux) and 3.4 minutes when the windows were reused (300,000 and 600,000 histories). An analog run needs tens of millions of histories for the same error on the magnet fast flux (two 1.3 million history analog runs were at 29 % and 35 %).

With windows use small batches, for example `--batches 1000 --particles 20000`: a run never stops before its 30th batch, and 30 batches of 250,000 would already be 7.5 million histories. Without `--window-python`, or if making the windows fails, the run is a single analog pass as before and its record says so and why. A preview (no `--target-error`) is always analog.

Both passes also apply a check on how steady the batches are: a target counts as reached only when the variance of the variance of the batch values is at most 0.1 (the limit MCNP uses and `faris design transport` applies). If the error is at the target but the batches disagree, the run goes on from its last statepoint with more batches; if the budget runs out first the result says the target was reached but is not trusted.

The windows are in the windowed pass's directory as `windows.h5`, with the helper's record `windows-record.json`; `faris reactor inspect` checks them and both passes against the hashes in the input and reports whether the run used them.

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

Repeat `--run` for the other arrangement. Add `--python`, `--openmc`, `--audit` and `--cross-sections` to enable **Run and review transport** in the Simulate step, or set them under **Transport configuration**. Under **Run quality**, **Preview** runs a fixed 90,000 histories, about a minute on two threads; it states the errors it reached, and the magnet fast flux is only roughly known at that size. **Production** stops once the magnet fast flux is known to 5 %, or when its budget is used, and says which. With a **Window Python** set (OpenMC 0.16 or later; the same setting as in the Design flux window, under **Transport configuration** here) Production runs two passes: an ordinary pass for the whole-model results (tritium, heating), then a pass with weight windows aimed at the magnet flux, which finishes in minutes; the budget of the windowed pass is 20 million histories in batches of 20,000, and the windows are kept in `faris-transport-cache` in the runs folder for the next run. The result says which pass each number came from. Without a Window Python it is a single analog pass, the budget is 50 million histories, and it can take hours; the result says that windows were not used and why. The app uses up to four threads, one fewer than the machine has. Closing the app cancels its transport workers. Camera interaction stays available while a job runs.

A study started from `--run` records cannot be saved as a `.faris` file, because a study file holds recorded-transport bundles. `faris transport pack` packages a completed run into a portable bundle:

```bash
faris transport pack --run runs/cold-coupled-control-reference-001/run.json --output reference.transport-bundle.json
```

## Where to read more

The repository documents the details: the [cold-reference workflow](https://github.com/AvilaLabs/FARIS/blob/main/docs/COLD_REFERENCE.md) covers the materials, the source, the recorded tallies and the verification steps, and the [transport boundary](https://github.com/AvilaLabs/FARIS/blob/main/docs/TRANSPORT.md) covers the contracts, normalization and trust limits.

Next: [Reading the numbers](results.md).
