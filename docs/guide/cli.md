# Command line

The package holds two programs: `faris-app`, the desktop, and `faris`, the command line. The `faris` command needs no graphics, Python, OpenMC or Avila Core, except for the commands marked below. Run `faris --version` to check the version and `faris COMMAND --help` for every flag of a command.

Errors exit with status 2. A study file that fails verification exits 1.

## Commands

| Command | What it does |
| --- | --- |
| `faris validate SCENARIO` | Validates a scenario without running physics. |
| `faris export --scenario S --output O` | Exports geometry metadata and its evaluation status. It refuses an existing destination. It is not the study export below. |
| `faris doctor` | Reports which optional tools are on `PATH`. It runs nothing. |
| `faris study-file create` | Writes a `.faris` file from recorded bundles and assumptions. |
| `faris study-file inspect FILE` | Summarises a file without extracting it. |
| `faris study-file verify FILE` | Rehashes every blob and rebuilds every bundle. Exits 1 on any failure. |
| `faris study-file unpack FILE DIR` | Writes the contents back out as ordinary files. Refuses an existing directory. |
| `faris study-file export FILE --output DIR` | Writes the [export folder](export.md). |
| `faris history run` | One deterministic history from `--assumptions`, `--rates` and `--output`. |
| `faris history validate` | Validates `--assumptions` and `--rates` without calculating. |
| `faris history from-run` | Binds rates from a successful transport run and calculates the history. |
| `faris history ensemble` | An ensemble of histories on sampled transport rates. |
| `faris history compare-runs` | Compares two runs under identical assumptions. |
| `faris history sensitivity` | Full-rerun sensitivity over a `--grid` file. |
| `faris maintenance run` | Computes replacement outages from decay heat for every design and compares them with the fixed ones. Needs ACTINV and Python. See [Computed maintenance durations](maintenance.md). |
| `faris maintenance report RESULT` | Prints a maintenance result as tables. |
| `faris transport pack` | Packages a completed run into a portable bundle. |
| `faris transport validate-request` | Validates a transport request against the exact scenario bytes. |
| `faris transport normalize` | Converts solver-reported per-source scores to physical rates and densities. |
| `faris reactor run` | Runs fixed-source transport through OpenMC. Needs your own OpenMC and data. See [Run your own transport](transport.md). |
| `faris reactor inspect` | Revalidates a saved run's input, artifact, volumes and normalization. |
| `faris design init STEP` and `faris design check DESIGN` | Available from the next release. Write a draft design file for a STEP model, and run import checks 1 to 5 on it. Needs a Python with cadquery. See [The design file](design-file.md). |
| `faris control absorber` | A synthetic one-group absorber, a mathematical control and not reactor physics. Needs OpenMC. |
| `faris study generate` and `faris study compile` | Generate a Core study, and compile it with an Avila Core executable you choose. No solver runs. |
| `faris evidence prepare`, `run`, `inspect`, `stage`, `verify-store` | Package and execute identified evidence with Avila Core. See [Evidence](evidence.md). |

## Work with study files

```bash
faris study-file create --bundle port/bundles/reference.transport-bundle.json \
  --control-bundle control/bundles/reference.transport-bundle.json \
  --sweep-bundle sweep/blanket-045cm.transport-bundle.json \
  --assumptions operating-assumptions.json \
  --evidence saved-study-port-reference.json [--pack-evidence] -o demo.faris
faris study-file inspect demo.faris
faris study-file verify demo.faris
faris study-file unpack demo.faris out/
faris study-file export demo.faris --output exports/
```

`--bundle`, `--physics` and `--sweep-bundle` are repeatable. `--evidence` takes a saved-study descriptor from a package (`faris-saved-study-store/v0.1`, naming two trees of the package's evidence store) or from an older package (archives); use one kind per file. `--pack-evidence` stores the Core evidence inside the file; without it the file records the trees' file lists and hashes and the evidence stays in the package. `--view` records a view from a JSON file, `--zstd-level` sets the compression level (default 15), and `--preview` stores a PNG thumbnail. `create` never overwrites an existing file. See [Study files](study-files.md).

## Calculate a history

Operating histories and their uncertainty run headless from a recorded transport run:

```bash
faris history from-run --scenario <scenario.json> --run <run.json> \
  --assumptions scenarios/arc-inspired/demountable-magnet-assumptions.json \
  --output history.json --rates-output rates.json
faris history ensemble --assumptions scenarios/arc-inspired/demountable-magnet-assumptions.json \
  --rates rates.json --samples 200 --output ensemble.json
```

`--samples` is 1 to 2000 and defaults to 200. `--seed` and `--threads` are optional. An ensemble that is not evaluated is written and exits 0, so read its `status`. [Uncertainty ensembles](uncertainty.md) explains the result, and [Method](method.md) the calculation.

## Desktop options

`faris-app` takes a `.faris` file as its argument. Options you may want:

| Option | Effect |
| --- | --- |
| `--step STEP` | Opens on `design`, `simulate`, `operate`, `compare` or `evidence`. |
| `--tour auto\|always\|never` | Controls the guided tour. |
| `--interface-scale N` | Starts with the interface magnified by N (0.75 to 2). |
| `--initial-year N` | Starts the timeline at year N. |
| `--field-view VIEW` | Starts with a field view for results loaded with `--run`. |
| `--core PATH` | The Avila Core executable used by **Compile study**. |
| `--package DIR` | Opens the recorded study package in DIR. Without a study file or data options, the app looks beside its own `bin` folder. |
| `--runs-directory DIR` | Where generated runs go. It must be outside the package. The default is a per-user folder; see [Download and verify](install.md). |
| `--window-width N`, `--window-height N` | The initial window size. |

Run `faris-app --help` for the rest.

Next: [Run your own transport](transport.md).
