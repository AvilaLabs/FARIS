<!-- kind: explanation -->
# Validation harness

The harness in `validation/` scores FARIS runs against reference data and writes
the report that requirements VAL-027 to VAL-037 ask for. It is standard-library
Python and needs no OpenMC to score, test or report. OpenMC is needed only by a
case's runner, which is a separate script and is never started by the harness.

Nothing here claims that FARIS is validated. A case with no FARIS run is reported
NOT_EVALUATED, with why and next step. Verdicts come from `validation/scoring.py`
and never from a hand edit (VAL-006).

## Layout

| Path | Content |
| --- | --- |
| `validation/scoring.py` | compatibility rule, C/E distribution, bootstrap bias, aggregation refusal, run-record hashing, stale marking |
| `validation/manifest.py` | manifest schema, validator, upstream file hash check |
| `validation/report.py` | per-case and suite report, JSON and Markdown; one command; the release V&V report (VAL-091) |
| `validation/matrix.py`, `quantities.json`, `matrix.json` | the V&V matrix, generated from the quantity registry and manifests; its blank-cell check (VAL-051) |
| `validation/models.py`, `models.json`, `model_matrix.json` | the model matrix: one row per model FARIS reports, with its verification evidence, the validation state of its quantities and what is not covered; its lint and staleness check (VAL-001, VAL-037) |
| `validation/checks.py` | rule-hash, range-hash, not-covered, provenance and code-to-code lints |
| `validation/cases/<case-id>/manifest.json` | one case: source, licence, file hashes, detectors, rule, not-covered list |
| `validation/cases/<case-id>/build_manifest.py` | regenerates the manifest from a read-only upstream checkout |
| `validation/cases/<case-id>/run_case.py` | case runner that builds and runs OpenMC (OKTAVIAN only so far) |

Run the tests with `python3 -m unittest discover -s validation -p 'test_*.py'`. Make the
report with:

```sh
python3 -m validation report --out DIR [--runs RUNS_DIR --identity identity.json]
```

The report also carries the methods, the V&V matrix, the qualified ranges, the known issues and the last run date of each case. Add `--release-record FILE` to write its SHA-256 into the release record. The release gate that checks the matrix and the report is described in [RELEASING.md](RELEASING.md).

`RUNS_DIR` holds `<case-id>.run.json` records. `identity.json` is the current
`library_sha256`, `code_version` and `adapter_sha256`. Without run records every case
is reported NOT_EVALUATED.

## Evidence classes (VAL-001)

Every manifest, every report row, every matrix row and every item of verification evidence in the model matrix
carries exactly one class. A matrix row for a quantity with no benchmark has no evidence and carries the class
`none`, which is allowed nowhere else; `check_matrix` and `check_model_matrix` reject a blank, unknown or second label.

| Class | Meaning |
| --- | --- |
| `verification` | an equation solved correctly against an analytic or manufactured answer |
| `code-to-code` | agreement with another code; shared nuclear data and geometry are listed, and it never counts toward "validated" (VAL-041) |
| `experiment` | agreement with a measurement |
| `plant-reproduction` | agreement with the recorded behaviour of a plant or a published plant design; no case uses it yet |

## The model matrix and the release lint (VAL-001, VAL-006, VAL-037, VAL-053, VAL-091)

`matrix.json` has one row per quantity and benchmark. `model_matrix.json` is the other view: one row per model or
response FARIS reports (transport and its normalisation, design transport, the operating history, the summaries, the
independent controls, uncertainty ensembles, maintenance durations, geometry checks). Each row states

- the outputs it reports and their quantities, read from the Rust output catalogues (`pub const NAME: &[Output]`)
  that classify every number FARIS reports, not typed;
- its verification evidence: tests and controls from `models.json`, each with one evidence class and a check that the
  named test exists in the named file;
- its validation evidence: the status of each of its quantities, taken from `matrix.json`, with the benchmark ids;
- its status (the weakest state among its quantities, a failing benchmark first), the reason and the next step;
- what is not covered, and the hashes of the source files it was generated from.

Both files are generated, never edited:

```sh
python3 -m validation matrix            # matrix.json
python3 -m validation models            # lint, then model_matrix.json
python3 -m validation models --check    # lint and staleness; writes nothing
```

The lint fails when a catalogue reports calculated numbers but no model claims it, when a model's quantity is not in
the registry, when a quantity of the Rust `Quantity` enum has no registry row, when a named test or control does not
exist, or when an evidence item has no single class. A developer who adds a calculated output in Rust therefore adds
its quantity and its model row in the same change.

A row is STALE when anything it was generated from has changed: a source file of the model, a case manifest, the
quantity registry, the models file, or the rules. `scripts/release_gate.py` regenerates both files from the tree and
refuses the release while they differ from the committed ones, naming the row and the file. Regeneration is
deliberate: change the code, rerun the affected benchmarks, then regenerate and commit. The gate also re-derives the
report's matrix from its case records (VAL-006): a hand-edited cell, a verdict count that is not the tally of its
detector rows, or a row that claims validation without a case record that has a PASS or FAIL verdict fails the gate.

What this does not do yet: the source hash is a hash of the model's files, not of the compiled kernel; a run record
carries library, code version and adapter hashes (see below) but is not read from an Avila Core store receipt; no
number on screen links to its matrix row (VAL-053) beyond the validation state beside it (VAL-054).

## The rule (VAL-027, VAL-028)

A response is compatible when

    |C - E| <= k * sqrt(u_C^2 + u_E^2 - 2 Cov(C, E))

`k` and the covariance treatment are stored in the manifest, with their basis, and a
`declared` date: the rule is written before any result is read (VAL-005). Treatments:
`independent` (Cov = 0), `correlation` (`rho`), `shared_normalisation`
(`shared_relative_u`, Cov = s^2 C E), `explicit` (a value) and `unknown`.

- `unknown` covariance, a missing uncertainty, or a covariance larger than the
  uncertainties allow gives INCONCLUSIVE, never PASS.
- A Monte Carlo error above 0.5 times the reference uncertainty gives INCONCLUSIVE and
  the report says what precision is needed.
- A reference value of zero has no C/E; the row is NOT_EVALUATED.
- A manifest whose `normalisation.status` is not `confirmed` and which sets
  `blocks_scoring` keeps the C/E numbers but cannot produce PASS or FAIL.
- A detector with a `blocked` entry is NOT_EVALUATED with its stated why and next step.

## Statistics (VAL-030, VAL-031, VAL-035)

C/E is summarised per response class, from all detectors: count, mean, median,
standard deviation, 5th and 95th percentile, minimum and maximum, with the worst case
(largest |C/E - 1|) named first. Bias is the mean of log(C/E) with a 95 % percentile
bootstrap interval over detectors, at least 5,000 resamples, seeded (default
20261009, recorded in the report). Fewer than five detectors in a class is labelled
"too few to estimate".

`aggregate_ce` raises `AggregationError` if its rows differ in response class, library
hash or code version. The report has no headline number across them.

## Run records, literature and identity (VAL-036, VAL-037)

Only a sealed FARIS run record is scored: `kind: "faris-run"` with a `record_sha256`
over its content. An edited, unsealed or wrong-case record is refused. A record of
kind `literature` is refused outright; literature values live in a manifest's
`literature_context` with kind `literature`, and the report prints them as labelled
context only.

A record carries the identity it was produced with (library hash, code version,
adapter hash). If it differs from the current identity every row is STALE, naming the
field that changed. Each scored row repeats the three values.

## Validation status beside a result (VAL-054)

Every calculated number FARIS shows or exports carries the validation state of the quantity it is an
instance of: one of the twelve quantities of the V&V matrix (TBR, heating, flux, fluence, dpa, gas
production, activation, decay heat, shutdown dose rate, tritium inventory, availability, net electric
power). The Rust code reads `validation/matrix.json` at build time, so a result and the release gate agree.

| State | What it means for a reader |
| --- | --- |
| validated within range | A benchmark with an experimental reference was scored and agrees within its rule. It holds only inside the benchmark's qualified range, which is in the matrix row. |
| validated but out of range | At least one scored benchmark detector FAILs its rule. Treat the number as unreliable until the cause is found. |
| code-to-code only | The only agreement is with another code (shared nuclear data and geometry are listed). That is verification, not validation (VAL-041). |
| not validated | No FARIS run has been scored against a benchmark that measures this quantity. The number is a calculation with a stated sampling error and nothing more. Today every quantity is in this state. |

A quantity with several benchmark rows shows the failing state first (any row out of range), then the
strongest evidence. A quantity the matrix does not name shows "not validated: this quantity is not in the
validation matrix" with a next step, never a blank.

How it shows: a short tag after the number, for example `[not validated: flux]`, and the why and next step once
per quantity in a block at the end of the text (`faris design transport`, `faris reactor inspect`). The CSV
exports have a `<quantity>_validation` column, or a `validation` column on the long-format tables, and
`data/validation.csv` holds the scope, why and next step of each quantity. The PDF marks a row with a code
(`NV`, `VR`, `VO`, `CC`) and prints the legend. A value that is an authored input, a conversion of one or a
count is not a calculated number and carries no state; the report types list which fields those are.

A new numeric field in `DesignTransportReport`, the transport receipt, the history and comparison results, the
brief summaries and the exported tables fails a test until it is classed as a calculated number (with its
quantity) or as not calculated (with the reason).

## Adding a case

1. Take the reference data from a source whose licence permits redistribution of the
   tables you copy (record SPDX id and attribution text). Keep upstream input files
   outside the repository; the manifest holds their sha256 and a pinned commit or DOI.
2. Write `build_manifest.py` so the manifest can be regenerated from a clean checkout
   and so the hashes are checked, not typed.
3. Declare the detectors with their response class, reference value and uncertainty;
   the rule (`k`, covariance, basis, date); the normalisation fields of VAL-029; the
   `not_covered` list (VAL-075); and the qualified-range fields (VAL-070). Anything
   unresolved is stated as blocked or unconfirmed, with why and next step, not guessed.
4. Add a runner that produces a sealed run record, and a `--i-have-the-cpu` guard if it
   starts transport.
5. Run the manifest validator tests; `python3 scripts/requirement_trace.py --check`.

## Cases

### OKTAVIAN aluminium sphere (`oktavian-al`, experiment)

Source: IAEA-NDS/open-benchmarks, commit `1d9855b00e05c5d5e25a11026ac56940dc75b516`, all
contents CC-BY-4.0 (attribution in the manifest). Leakage neutron spectrum (134 bins,
0.1 to 20.7 MeV) and photon spectrum (57 bins) with the uncertainty column as provided.
The experimental tables are copied into the manifest unchanged; the OpenMC, MCNP and
Serpent inputs are hashed and not copied.

What the first pass found, all recorded in the manifest:

- The published OpenMC `materials.xml` has pure Cr50 for the steel shells and pure Al27
  for the aluminium; the MCNP input has 13-nuclide stainless steel and aluminium with
  impurities. The OpenMC file also has no settings, source or tallies. The runner uses
  the OpenMC geometry with the MCNP materials and the MCNP source histogram.
- The CSV unit is the leakage current through the whole outer surface of the sphere (MCNP
  surface 6, r = 19.95 cm) per source neutron: neutrons per unit lethargy, photons per MeV,
  no area factor. Status `confirmed`; the evidence is in `normalisation.basis`: the original
  report (Ichihara et al., OSTI ETDE 407537) normalises the leakage current per source
  neutron; the upstream MCNP tallies `f21:n 6` and `f41:p 6` are type-1 tallies (current
  through a surface, not per cm2) and the JADE post-processing only divides by the lethargy
  or energy width; the CSV spectra integrate to 0.942 neutrons per source neutron above
  0.097 MeV and to 0.37 to 0.40 photons per source neutron above 0.5 MeV (depending on the
  unresolved photon edge convention), physical for a 40 cm sphere where per-cm2 values
  would be about 5000 times smaller. The per-cm2 values in `eepeterson/openmc_fusion_benchmarks`
  are that packaging's division by 4 pi 19.5^2 cm2 and are not used.
  Correction: an earlier version of this page and of the manifest called the unit
  `inferred` and said the upstream tally "is a flux (MCNP F2) not a current". That was
  wrong: F21 and F41 are type-1 tallies. The git history keeps the old text.
- The `Error` column does not say whether it is statistical only; it is below 0.3 % near
  the 15 MeV peak.
- The photon bin edge convention differs between the two packagings, so the photon
  detectors are blocked. The first neutron bin straddles the 100 keV measurement limit
  and is blocked.
- Neutron data for Al, Si, Cr and Ni are in the FENDL-3.2 HDF5 library but not in
  `references/openmc-library-audit.json`. The runner refuses until they are audited
  (`--audit-extension`).

The runner (`run_case.py`) has three subcommands: `plan` reads files only and prints the
settings, library coverage and cost estimate; `build` exports the OpenMC XML; `run`
runs transport and needs `--i-have-the-cpu`. The runner tallies the current on the published
surface 6 itself, with the energy edges of the MCNP `e21`/`e41` cards and score `current`.
The scored values are the net current (outward minus inward), one `SurfaceFilter` bin per
energy: OpenMC weights each surface crossing by +1 or -1 (`filter_surface.cpp`, v0.15.3, lines
55-59). Surface 6 is convex with only void outside it, so nothing can cross it inward and the
net current equals the outward current. The runner checks that with a second tally, the
outward partial current (`SurfaceFilter` plus `CellFromFilter` on cells 1, 2 and 4, the cells
that border surface 6 from inside, summed over those cells with the standard deviations added
in quadrature). The run record states whether net and partial agree within 3 combined
standard deviations in every bin; if not, it says so. The conversion divides by the lethargy
width (neutrons) or the MeV width (photons) and nothing else. There is no extra shell and no
dilution factor. The photon CSV bin-edge convention stays unresolved (the spectrum integrates
to 0.372 or 0.399 photons per source neutron depending on it), which is why the photon bins
remain blocked; that is the existing photon blocker, not a new one.

Literature context (never scored): the MIT-licensed repository holds OpenMC 0.14.0 and
MCNP results for the same sphere with several libraries, including FENDL-3.2.

### OKTAVIAN aluminium sphere from a STEP file (`oktavian-al-cad`, experiment, geometry path CAD)

The same sphere and the same measurement as `oktavian-al`, but the geometry is the OFB STEP file
(`lfs/benchmarks/oktavian/oktavian_a.step`, five solids, millimetres) converted by FARIS's own CAD helper
(`integrations/cad/faris_cad.py convert`, chord 0.1 cm, angular 0.1 rad) to a DAGMC model, and the materials,
source histogram and tally energy bins come from OFB's specification. Source: OFB `develop` 653d6ab with
openmc_fusion_benchmarks-lfs dace719 (MIT); the measured spectrum is the IAEA table (CC-BY-4.0), which is the
same measurement as OFB's packaged `experiment.h5`: every bin of OFB's table equals the IAEA value times the bin
width divided by one constant, 4 pi (19.5 cm)^2 = 4778.362 cm2 (checked at build time and recorded in the
manifest's `equivalence`). So OFB's stored values are per bin and per cm2, and the unit question left open by
the `oktavian-al` note is settled in the same direction: FARIS's per-source-neutron, per-lethargy, no-area
convention divides by neither. OFB's SS-304 is the real 13-nuclide composition (the same as the MCNP input of the
IAEA package), not pure Cr50.

The rule was written and committed before the first transport run whose result was read (manifest block
`protocol`, declared 2026-10-14):

- Scored: the 133 neutron bins from 101 keV to 20.66 MeV. Photons are recorded and not scored (edge convention
  unresolved, as in `oktavian-al`).
- Per bin: the manifest's rule, |C - E| <= 2 sqrt(u_C^2 + u_E^2), independent; INCONCLUSIVE when the Monte Carlo
  error exceeds 0.5 u_E.
- Case outcome: INCONCLUSIVE if fewer than 67 of the 133 bins are PASS or FAIL; otherwise PASS if at least 90 %
  of those are PASS, FAIL if fewer. The 90 % and the 67 are authored and provisional.
- Against the FARIS CSG run of `oktavian-al` made in the same session with the same library, code, histories,
  seed and cutoff: per bin with a relative Monte Carlo error of 20 % or less in both, agree if the two currents
  differ by at most 2 combined standard deviations; PASS at 90 % agreement over at least 50 bins. This is
  code-to-code verification of the geometry path, not validation (VAL-041).

Deviations from OFB's specification are listed in the protocol: an isotropic source and probability-weighted
energy bins (OFB's own source function builds neither, see below), a tally on the whole outer boundary instead of
"surface 7", a 1 keV neutron cutoff as in the IAEA MCNP input, and photon data from ENDF/B-VII.1. The library is
FENDL-3.2 neutron data with ENDF/B-VII.1 photon data (`make_library.py`): the photon files of the FENDL-3.2 HDF5
directory crash OpenMC 0.15.3 (segmentation fault in `sample_photon_reaction`), which also stops the existing
`oktavian-al` runner, and `data/raw/combined-fendl32-endfbvii1` has no Al, Si, Cr or Ni neutron data.

Defects found in OFB while building this case are in the manifest (`known_upstream_defects`) and in
[defects/OFB-001-oktavian-source-and-tally-definitions.md](defects/OFB-001-oktavian-source-and-tally-definitions.md).

#### Result of the first run (2026-10-14, appended after the protocol commit 46d23ff)

Sealed records and the analysis are in `validation/cases/oktavian-al-cad/records/` (CAD record sha256 `76c79fd6...`,
CSG comparator record `284d2144...`; `analysis.json` is `analyse.py` applied to both, which uses only the numbers of
the protocol block). Conversion: STEP read, DAGMC conversion and facet export 8.4 s, 604 MB peak, 5 volumes, 18
surfaces, 43,732 triangles; the tally surface was found by rule (surface 6, 125,567 cm2 faceted against 125,664 cm2
for the sphere). Transport: OpenMC 0.15.3, 10,000,000 histories (100 batches), seed 1, 3 threads on cores 0 to 2,
455 s (22,200 histories per second), 0 lost particles; the net current equals the outward partial current in every
bin of both particle types. The CSG comparator took 137 s with the same settings.

Against the experiment, by the declared rule: **FAIL**. Of the 133 scored neutron bins, 29 PASS, 103 FAIL, 1
INCONCLUSIVE (132 are PASS or FAIL, so the coverage condition holds; 22 % PASS against the 90 % required). C/E
over the 133 bins: median 0.906, 5th to 95th percentile 0.47 to 1.77, bias (mean log C/E) -0.077 with 95 % bootstrap
interval [-0.166, +0.017]; the median C/E is 1.001 from 0.1 to 1 MeV, 0.895 from 1 to 10 MeV and 0.709 from 10 to 20.7
MeV. The Monte Carlo error is 0.07 to 2.8 % in the bins below 17 MeV and exceeds half the experimental uncertainty in one
bin only (16.92 MeV, the INCONCLUSIVE one), so the FAILs are not a statistics artefact. The worst bins sit at the 14 to 17 MeV edge of the source histogram
(C/E 2.41 at 13.85 MeV, 4.42 at 16.25 MeV, 43.1 at 16.92 MeV).

Where the FAIL comes from, as measured: the FARIS CSG run (same library, code, histories and seed) scores 28 PASS, 103
FAIL, 2 INCONCLUSIVE with the same bias (-0.078) against the same experiment, and the geometry-path comparison is a
PASS: 132 of the 133 bins agree within 2 combined standard deviations (the one exception is n-1.0706, 2.14 sigma), the
median CAD over CSG is 0.9998, the largest bin deviation is 2.14 sigma. So the CAD-to-DAGMC path reproduces the CSG
result of the same OpenMC, library and source. That is verification of the path (VAL-041), not evidence about the
physics. The disagreement with the measurement is common to both geometries. The OFB results database (OpenMC 0.14.0
and MCNP 6.1.1b CSG runs) shows the same bin pattern near the source edge (C/E as stored 2.35 at 13.85 MeV, 4.75 at
16.25 MeV, 46 at 16.92 MeV for OpenMC with FENDL-3.2), which points at the benchmark's coarse 51-bin source histogram,
whose interval [15.002, 16.579] MeV carries 29.5 % of the neutrons uniformly, against a measured peak that falls
steeply above 15 MeV. This is context from other people's runs and is labelled so; it is not a FARIS result and not
part of any rule. A smoother source representation would be a new case revision with a new declared rule, not an edit
of this one.

Open points, stated as such: the experiment's `Error` column is of unspecified kind, so k = 2 on it may be strict;
the photon spectrum is recorded and not scored; the 90 % threshold was authored before the run and is provisional.

Reproduce the evidence with `make_library.py`, then `run_case.py convert` and `run_case.py run`
(`--particles 10000000 --seed 1 --threads 3`), then `python3 validation/cases/oktavian-al-cad/analyse.py --cad ... --csg ...`.

### Cases awaiting an upstream specification

`fng-streaming-onaxis`, `fng-streaming-offaxis`, `fng-streaming-heating` (OFB item 3, STEP files present) and
`fns-dogleg-duct` (item 2, no geometry at all). Each manifest has `registration.status =
awaiting-upstream-specification` and lists exactly what is missing; every detector is NOT_EVALUATED with why and
next step. They are registered so the gap is visible in the report and the matrix cannot silently skip them.

What is missing for all four: a specification in `src/openmc_fusion_benchmarks/benchmarks/` (OFB `develop` has
`oktavian_al` only), the measured values with uncertainties in a hashable form (the `develop` results database has
`oktavian_al` only), and the licence under which the measured values may be redistributed. For the FNG cases also
the foil or detector positions, the FNG source and the shield compositions (none are in a STEP file); for the
dogleg also the geometry (OFB-LFS has no STEP file for it) and the JAEA FNS source.

The FNS dogleg is the priority of the four: the bent-duct finding in
[notes/CAD_TRANSPORT_TEST_SET.md](notes/CAD_TRANSPORT_TEST_SET.md) (0 of 20,016 void lines from plasma to coil
through the dog-leg duct, so neutrons reach the response without a collision only through thin material) is a
claim about a CAD-derived bent streaming path, and a measured bent duct is the experiment that tests it.

The older `master` branch of OFB (commit c47fc57) has a CSG model and an `experiment.h5` for FNG streaming and
the FNS duct, with literature attributes (`doi:10.1016/S0920-3796(00)00205-2`, ENEA Frascati 1997 to 1998;
`doi:10.1080/00223131.2004.10875640`, JAERI 2004). `develop` removed them, and the repository does not say where
the measured values come from or whether they may be redistributed. They are not registered and no value from
them is used.

### ITER_1D (`iter-1d`, code-to-code)

Uses the identities already registered in `references/iter-1d-reference.json`
(VAL-044). It has two detectors, one per declared response class, and no reference
values: until a second code is run the case is NOT_EVALUATED with that reason.

## Registered sources

### OpenMC-Fusion-Benchmarks (OFB)

Segantin, Pettinari, Shimwell and Peterson, "OpenMC Fusion Benchmarks", Fusion Science and Technology
2026, doi 10.1080/15361055.2026.2664345. An open set of fusion neutronics benchmarks with an OpenMC
specification, STEP geometry, a results database of OpenMC and MCNP runs, and a Python package that builds,
runs and post-processes them. FARIS reads it as a source of inputs and cross-checks. It does not take OFB's
results as its own (VAL-036), and it does not run OFB's package: the case runner builds the model itself from the
pinned files.

| Repository | Pinned commit | Retrieved | Licence |
| --- | --- | --- | --- |
| https://github.com/eepeterson/openmc_fusion_benchmarks, branch `develop` | `653d6aba04d993d2915a4e4e983e2a2119fa0e27` (2026-04-08) | 2026-10-14 | MIT, Copyright (c) 2025 MIT PSFC |
| https://github.com/SteSeg/openmc_fusion_benchmarks-lfs (OFB submodule `lfs`) | `dace719d46c76c3b9f24a98ebf9f85ce1b8092a7` (2025-06-11) | 2026-10-14 | MIT, Copyright (c) 2025 Stefano Segantin |
| the same OFB repository, branch `master` (older layout, read for the inventory below only) | `c47fc573a7e2a9ca11a8958dc67155b1146adf51` (2025-03-21) | 2026-10-14 | MIT, as `develop` |

Licence attribution: the MIT licence requires its copyright notice and permission notice in "all copies or
substantial portions of the Software". FARIS copies no OFB source code. The case manifests hold the sha256 of
the OFB files they use, small numeric tables extracted from the OFB specification (the OKTAVIAN source
histogram, tally edges and material compositions) with the attribution "from OpenMC-Fusion-Benchmarks,
Copyright (c) 2025 MIT PSFC, MIT licence, commit 653d6ab", and nothing else. The licence texts are the files
`LICENSE` and `lfs/LICENSE` in the pinned commits (sha256 `1da773db...` and `0a103dd5...`). The STEP files are
fetched with `git lfs pull` (or from `media.githubusercontent.com` at the pinned submodule commit) into a cache
directory and are not stored in this repository; the LFS object id of each file is its sha256 and the manifests
record it.

What `develop` at the pinned commit contains:

- `src/openmc_fusion_benchmarks/benchmarks/oktavian_al/specifications.yaml` is the only benchmark
  specification. The schema is `benchmarks/benchmark_schema.yaml`.
- `src/openmc_fusion_benchmarks/results_database/oktavian_al/` holds `experiment.h5`, three OpenMC 0.14.0 CSG
  runs (ENDF/B-VIII.0, FENDL-3.2, JEFF-3.3) and four MCNP 6.1.1b CSG runs (ENDF/B-VIII.0, JEFF-3.3, JENDL-4.0,
  TENDL-2017). There are no CAD-geometry results in the database, although the OFB documentation shows CAD runs.
- `lfs/benchmarks/` holds STEP files for `oktavian` (`oktavian_a.step`, `oktavian_a.gltf`), `oktavian_al`,
  `fng_str_onaxis`, `fng_str_offaxis` and `fng_str_heating`.
- No data for FNG, FNS or the ITER port plug, and no results for any other sphere.

Planned upstream (`benchmark_list.txt`, not yet specified in `develop`): FNS clean benchmarks (neutron and photon
spectra, heating and dosimetry in Be, V, Fe, Cu and W assemblies); the FNS dogleg duct streaming benchmark; the
FNG streaming benchmark; the ITER port plug computational benchmark (a shutdown dose rate code comparison); the
FNG shutdown dose rate benchmark (19 decay times after 2 days of irradiation); an ITER E-Lite model.

The older `master` branch (layout before 2025-03-28, commit above) holds CSG models and `experiment.h5` files
for `fng_str`, `fng_w`, `fns_clean_w`, `fns_duct` and eleven OKTAVIAN spheres. `develop` deleted them
(2025-03-28 and 2025-12-01). Their provenance and redistribution status are not stated in the repository, so they
are not registered as sources; see the cases below. Other branches (`fng_dose_r2s` and siblings) hold research
code from 2022 to 2024.

What OFB changes for open items (checked 2026-10-14):

- The per-cm2 question of `oktavian-al` is settled by measurement: OFB's stored experiment is the IAEA table times
  the bin width, divided by 4 pi (19.5 cm)^2 = 4,778.362 cm2, equal in every bin of both spectra to 1e-13 (recorded in
  the `oktavian-al-cad` manifest as `equivalence`). The CSV unit stays "per source neutron, no area factor", as confirmed.
- VAL-023 (FNG shutdown dose rate, blocked on the SINBAD licence) is not unblocked. `develop` has no FNG shutdown dose
  data. The branch `fng_dose_r2s` (commit 80981efc90d29f09fe2168ae0fa91e49af43d8ae, 2024-05-08) holds a digitised
  experimental dose-rate curve (`fng_dose/analysis/experiment.txt`, 1,964 lines of cooling time in years and dose rate in
  Sv/h) and 18 dose-rate values at cooling times from 1 h to 60 d typed into `analysis/plot_results.py`. The branch has
  no licence file at that commit, and nothing states whether the numbers come from SINBAD (NEA terms: no redistribution,
  Art. 1 and 4, see NEXT_RELEASE_PLAN.md), from the published papers or from a digitised figure. FARIS does not use or
  copy them. Whether the Zenodo record 10.5281/zenodo.10660030 (CC-BY-4.0, the research data of the Peterson et al. R2S
  paper) holds the measured values is still unchecked. Asking the authors or the NEA is Connor's call.

## Licences

The harness is AGPL-3.0-only like the rest of FARIS. Reference tables inside a manifest
keep their own licence, stated in the manifest's `license` block and reproduced in the
report. Nuclear data used by a runner stays outside the repository.
