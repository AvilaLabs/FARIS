# FNS-TOF OpenMC adapter

This case package consumes the JAEA FNS time-of-flight benchmark as published
in the IAEA-NDS `open-benchmarks` repository at commit
`24b9efe3f2084a51eb1e83d9a08823ada5b9fe02`. The repository identifies the
benchmark inputs and experimental tables as CC BY 4.0. The manifest builder
copies only the compact measurement rows into attributed FARIS manifests; it
does not copy source decks or CSV files. Each upstream input/table/license hash
is retained and checked against a clean external checkout.

Primary benchmark description: [JADE experimental benchmarks, FNS time of
flight](https://jade-a-nuclear-data-libraries-vv-tool.readthedocs.io/en/latest/benchmarks/experimental.html#fns-time-of-flight-experiment).
The exact JADE MCNP postprocessor is pinned at
[`FNS-TOF.yaml`](https://github.com/JADE-V-V/JADE/blob/5ed62bdabbfb1d77a02d98db486916f5b47ec86d/src/jade/resources/default_cfg/benchmarks_pp/raw/mcnp/FNS-TOF.yaml).
The primary MCNP manual documents vertical multi-card data syntax in section
4.4.5.2, page 250 of the 2022 edition
([LANL manual](https://mcnp.lanl.gov/pdf_files/TechReport_2022_LANL_LA-UR-22-30006Rev.1_KuleszaAdamsEtAl.pdf)).

The adapter joins CSVs to MCNP point tallies only by the exact angle text in
the `FC` label and checks the tally number and `FM` factor. It preserves every
available fine and coarse energy response. A source angle/table absent from a
source configuration remains absent; the builder never fills one by position
or by borrowing another configuration. Source `SI1/SP1` weights are converted
from MCNP histogram bin masses to normalized OpenMC PDF density per eV.
The pinned MCNP manual says `SI` defaults to histogram option `H`, `SP` defaults
to bin-probability option `D`, the first `SP` entry must be zero, and these bin
probabilities need not sum to one. The adapter retains the literal deck values,
then normalizes them only for OpenMC source sampling.

The published JADE narrative says no `FM` card was used, while every pinned
source deck contains per-detector `FM` cards. The pinned JADE raw postprocessor
applies lethargy conversion and no second geometric factor. The manifest
documents this conflict and records a numeric check that each deck `FM`
matches the JADE $L^2/A_s$ relation within 0.4%. The adapter therefore applies
the exact input deck `FM` once and divides the integrated energy-bin flux and
standard error by `ln(E_hi/E_lo)`. For the first bin whose lower edge is zero,
it follows JADE's pinned transform: prepend `1e-10 MeV` as numerical zero
before taking the logarithmic ratio. The transform source hash is recorded in
each manifest. It does not apply another $L^2/A_s$ factor.

For each published coarse energy range, the OpenMC adapter requests a separate
coarse energy-filter tally and takes that tally's own statepoint standard
deviation. It does not combine fine-bin marginal standard deviations, which
would require unavailable cross-bin covariance information.

## Finite detector estimator

OpenMC does not provide the MCNP `F5` point-detector estimator. The runnable
adapter converts each pinned MCNP model with the public
[`openmc_mcnp_adapter`](https://github.com/openmc-dev/openmc_mcnp_adapter),
replaces its source/tallies with the pinned source distribution and a spherical
vacuum-cell flux tally at each exact point, then estimates point response by
successive sphere-radius halving. The converter itself states it has not had
methodical verification; its exact installed code identity is sealed in the
run record. The upstream converter is MIT-licensed and remains an external
dependency; no converter source is vendored. An
independent MCNP/OpenMC geometry/material equivalence review has not been
completed. Surface-boundary and sampled detector-containment checks are
necessary guards, not converter verification. Input translation and
finite-volume qualification do not establish physical validation; the case
manifest marks converter verification `NOT_EVALUATED` and the shared scoring
gate keeps verdicts `INCONCLUSIVE` while measurement covariance is unknown.
The required converter source is pinned to commit
`80fda5abeed22502e2f6e19e57a9b2a8762844a8` (MIT, distribution version `0.1.0`,
Python source tree SHA256
`a48ac7dede3729d6695f0848d15b2ddb43a9c0f1487be617cd70d0961ce633c1`). The run
plan reports this pin; run records verify and retain the installed source hash.

Before any production run, the predeclared qualification requires two
successive halvings from 2.5 to 1.25 to 0.625 cm. Every response bin must change
by no more than 1%, both endpoint relative Monte Carlo standard errors must be
no more than 2%, and zero/rare bins must resolve. Missing bins or failed
transitions make the qualification inconclusive; the runner will not seal a
validation run record. Sampled sphere-containment checks reject a converted
geometry boundary clipping the finite detector volume. They are a geometry
sanity check, not proof of complete MCNP/OpenMC geometric equivalence.

## Build manifests

The 14 generated, sub-megabyte manifests are checked in one level above this
helper package, in their standard discovery folders `validation/cases/fns-tof-*/manifest.json`.
Their reference rows are attributed CC BY 4.0; original source decks and CSVs
remain external and hash-bound. To regenerate them, use a clean checkout of the
exact external source commit:

```sh
python3 validation/cases/fns-tof-jade-v1/build_manifest.py \
  --upstream /path/to/open-benchmarks --out validation/cases
```

The source repository is available at
`https://github.com/IAEA-NDS/open-benchmarks`; checkout command:

```sh
git clone https://github.com/IAEA-NDS/open-benchmarks /path/to/open-benchmarks
git -C /path/to/open-benchmarks checkout 24b9efe3f2084a51eb1e83d9a08823ada5b9fe02
```

## Plan and execute

`plan` verifies the manifest-bound external input hashes and prints bounded run
settings without importing OpenMC or starting a solver:

```sh
python3 validation/cases/fns-tof-jade-v1/run_case.py plan \
  --upstream /path/to/open-benchmarks \
  --manifest validation/cases/fns-tof-fe-20/manifest.json \
  --cross-sections /path/to/cross_sections.xml \
  --particles 100000 --batches 100 --threads 1
```

After reviewing the plan and providing OpenMC 0.15.3, a compatible audited
cross-section library, and a pinned `openmc_mcnp_adapter` installation, run:

```sh
python3 validation/cases/fns-tof-jade-v1/run_case.py run \
  --upstream /path/to/open-benchmarks \
  --manifest validation/cases/fns-tof-fe-20/manifest.json \
  --cross-sections /path/to/cross_sections.xml \
  --work-dir /path/to/new-run-directory \
  --particles 100000 --batches 100 --threads 1 --timeout 3600 \
  --i-have-the-cpu
```

The per-radius limit is 50 million histories, 500 batches, eight threads, and
three hours; the default is 100,000 histories, 100 batches, one thread, and one
hour per radius. Existing output directories and record paths are never
overwritten. A failure preserves logs/intermediate radius data but cannot emit
a sealed scored run. The actual CSV confidence meaning and experimental
cross-bin covariance are not published in the source table; FARIS scoring must
continue to report `INCONCLUSIVE` unless that uncertainty model is established.
