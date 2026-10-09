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
| `validation/report.py` | per-case and suite report, JSON and Markdown; one command |
| `validation/cases/<case-id>/manifest.json` | one case: source, licence, file hashes, detectors, rule, not-covered list |
| `validation/cases/<case-id>/build_manifest.py` | regenerates the manifest from a read-only upstream checkout |
| `validation/cases/<case-id>/run_case.py` | case runner that builds and runs OpenMC (OKTAVIAN only so far) |

Run the tests with `python3 -m unittest discover -s validation -p 'test_*.py'`. Make the
report with:

```sh
python3 -m validation report --out DIR [--runs RUNS_DIR --identity identity.json]
```

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
- The CSV unit is undocumented. Compared with the MIT-licensed packaging in
  `eepeterson/openmc_fusion_benchmarks`, the neutron values equal its per-cm2 surface values
  times 4 pi 19.5^2 cm2 divided by the lethargy width, and the photon values the same
  divided by the bin width in MeV (constant to 0.05 % and 1e-11). The area uses 19.5 cm
  although the tally surface is at 19.95 cm. The status is `inferred` and blocks PASS and
  FAIL until it is confirmed.
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
runs transport and needs `--i-have-the-cpu`. It stands a 0.2 cm void shell outside the
tally surface in for the MCNP surface flux, and converts with the shell dilution factor
(0.990) and the manifest units. The shell estimator differs from an F2 surface flux; that
difference is part of what the run will measure and is not corrected.

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
