# Run your own transport

You do not need this to explore the recorded study. Use it to run new fixed-source transport yourself, for example to record a case with your own seed or history count. It uses the same engine from the command line and from the desktop.

## What you need

- OpenMC 0.15.3 in an environment you provide. The run refuses any other version.
- The nuclear data the study was audited against. The recorded runs use FENDL-3.2 neutron data and ENDF/B-VII.1 photon data. The photon data come from a local overlay that FARIS does not ship. Its acquisition is described in the repository's [photon library document](https://github.com/AvilaLabs/FARIS/blob/main/docs/PHOTON_LIBRARY_ACQUISITION.md).
- A library audit of your own installation. The audit file in the repository records one workstation's cache. Run `integrations/openmc/audit_library.py` on yours and supply its output. FARIS does not silently accept differences in library content.

The data stay outside the repository. They are not part of the package.

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

Repeat `--run` for the other arrangement. Add `--python`, `--openmc`, `--audit` and `--cross-sections` to enable **Run and review transport** in the Simulate step, or set them under **Transport configuration**. Closing the app cancels its transport workers. Camera interaction stays available while a job runs.

A study started from `--run` records cannot be saved as a `.faris` file, because a study file holds recorded-transport bundles. `faris transport pack` packages a completed run into a portable bundle:

```bash
faris transport pack --run runs/cold-coupled-control-reference-001/run.json --output reference.transport-bundle.json
```

## Where to read more

The repository documents the details: the [cold-reference workflow](https://github.com/AvilaLabs/FARIS/blob/main/docs/COLD_REFERENCE.md) covers the materials, the source, the recorded tallies and the verification steps, and the [transport boundary](https://github.com/AvilaLabs/FARIS/blob/main/docs/TRANSPORT.md) covers the contracts, normalization and trust limits.

Next: [Reading the numbers](results.md).
