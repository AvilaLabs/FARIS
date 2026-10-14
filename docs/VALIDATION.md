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

Every manifest and every report row carries exactly one class.

| Class | Meaning |
| --- | --- |
| `verification` | an equation solved correctly against an analytic or manufactured answer |
| `code-to-code` | agreement with another code; shared nuclear data and geometry are listed, and it never counts toward "validated" (VAL-041) |
| `experiment` | agreement with a measurement |

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

### ITER_1D (`iter-1d`, code-to-code)

Uses the identities already registered in `references/iter-1d-reference.json`
(VAL-044). It has two detectors, one per declared response class, and no reference
values: until a second code is run the case is NOT_EVALUATED with that reason.

## Licences

The harness is AGPL-3.0-only like the rest of FARIS. Reference tables inside a manifest
keep their own licence, stated in the manifest's `license` block and reproduced in the
report. Nuclear data used by a runner stays outside the repository.
