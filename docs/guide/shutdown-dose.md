<!-- kind: how-to -->
# Comparing shutdown-dose methods

FARIS can record a comparison of an already-run spatial D1S estimate with an already-run R2S
estimate. The command does not run OpenMC or derive dose from ACTINV's contact-dose proxy. It
imports the estimates and reports the D1S/R2S ratio at each cooling time.

```sh
faris shutdown-dose compare --request dose-comparison.json --output dose-comparison-result.json
```

`shutdown-dose compare` accepts already-run estimates in the record format below. The
[R3 reference scripts](https://github.com/AvilaLabs/FARIS/blob/main/integrations/openmc/risk_tests/README.md) can produce new D1S and R2S
records for the fixed RM-M research scenario with OpenMC 0.15.3. Their exporter verifies the input
identities and batch arrays, then writes a request for this command. Legacy R3 outputs lack the
required provenance and aggregate uncertainty and are refused.

The prospective reference workflow uses one fixed irradiation schedule and scored region. It does
not yet generate a whole-life dose curve for each replacement event or drive maintenance cooldowns.
Protocol `r3-full-emission-reference/v1` scores all photon descendants entering the response region
without a D1S particle-birth filter. R2S uses a separate activation mesh spanning the verified H5M
geometry bounds. Its 100 cm starting grid requires source-resolution checks at 50 and 25 cm;
the declared diagnostic requires changes no greater than 5% across both halvings at each cooling
time. That mesh check is not experimental dose validation, and its array estimate excludes the
larger activation and solver allocations. These solver runs and their convergence evidence remain
pending. Legacy particle-birth-filter and response-box-activation runs have different emitter
ancestry and are refused as equivalent inputs to the new exporter. Neither the legacy diagnostics
nor successful format and source-serialization checks qualify a maintenance-task dose field.
The activation command can also write grouped decay-photon source files with
`faris activation run --photon-source-dir DIR`; these source files are not transported dose estimates.

The request uses `faris-shutdown-dose-compare-request/v0.1`. Each cooling point names one
`faris-shutdown-dose-estimate/v0.1` record for each method and the expected SHA-256 of its exact
bytes. Paths in the request are relative to the request file. Each estimate record carries the
estimate, standard error, method, cooling time, quantity, unit, and normalisation. It also names
the geometry, materials, source, irradiation schedule, response data, and run record as files with
SHA-256 hashes. FARIS hashes those files and checks that the run record repeats the method, cooling
time, estimate and standard error, response quantity/unit/normalisation/location, sampling IDs,
and all six physical input hashes. D1S and R2S must refer to matching physical inputs and response
definitions. The estimate record is an imported summary; FARIS does not verify the external solver
calculation by hashing its inputs.

Each request point has `cooling_time_s`, `d1s` and `r2s` record references (`path` plus
`sha256`), and an optional covariance assessment. Omit it or use `{"kind":"unknown"}` unless
there is evidence to support independence. `{"kind":"independent_assumed","rationale":"…",
"evidence":{"path":"...","sha256":"..."}}` records an assumption and the file used to
justify it; covariance is not inferred from seed labels.

An estimate record contains `schema`, `method` (`d1s` or `r2s`), `cooling_time_s`, `quantity`,
`unit`, `normalization`, `estimate` (`value` and `standard_error`), and file references for
`geometry`, `materials`, `source`, `irradiation`, `response`, `location`, and `run_record`.
Each reference is `{ "path": "...", "sha256": "64 hex characters" }`, relative to the estimate
record. `location` identifies the scored region definition. The run record is JSON with top-level
`method`, `cooling_time_s`, `estimate` (`value` and `standard_error`), `response` (`quantity`,
`unit`, and `normalization`), and `random_streams`, plus an `artifacts` object holding
`geometry_sha256`, `materials_sha256`, `source_sha256`, `irradiation_sha256`, `response_sha256`,
and `location_sha256`. Mismatches stop the comparison.

The following two blocks show the request and one D1S estimate record. All names, hashes, times,
normalisation text, and dose values are **synthetic format examples**, not measurements or solver
results. The all-zero hashes are placeholders with valid SHA-256 syntax; replace them with hashes
of the actual files. Create the R2S record in the same shape, set `method` to `r2s`, and copy the
matching physical inputs, response definition, cooling time, and scored location. The `run_record`
file must be the actual run record whose fields and hashes the comparison checks.

```json
{
  "schema": "faris-shutdown-dose-compare-request/v0.1",
  "case_id": "synthetic-format-example",
  "points": [
    {
      "cooling_time_s": 86400.0,
      "d1s": {
        "path": "records/d1s-day1.json",
        "sha256": "0000000000000000000000000000000000000000000000000000000000000000"
      },
      "r2s": {
        "path": "records/r2s-day1.json",
        "sha256": "0000000000000000000000000000000000000000000000000000000000000000"
      }
    }
  ]
}
```

```json
{
  "schema": "faris-shutdown-dose-estimate/v0.1",
  "method": "d1s",
  "cooling_time_s": 86400.0,
  "quantity": "synthetic_spatial_dose_rate",
  "unit": "synthetic_unit_per_s",
  "normalization": "synthetic example; replace with the exact scoring and normalization",
  "estimate": {"value": 1.0, "standard_error": 0.1},
  "geometry": {"path": "inputs/geometry.h5m", "sha256": "0000000000000000000000000000000000000000000000000000000000000000"},
  "materials": {"path": "inputs/materials.json", "sha256": "0000000000000000000000000000000000000000000000000000000000000000"},
  "source": {"path": "inputs/source.json", "sha256": "0000000000000000000000000000000000000000000000000000000000000000"},
  "irradiation": {"path": "inputs/irradiation.json", "sha256": "0000000000000000000000000000000000000000000000000000000000000000"},
  "response": {"path": "inputs/response.json", "sha256": "0000000000000000000000000000000000000000000000000000000000000000"},
  "location": {"path": "inputs/location.json", "sha256": "0000000000000000000000000000000000000000000000000000000000000000"},
  "run_record": {"path": "runs/d1s-run.json", "sha256": "0000000000000000000000000000000000000000000000000000000000000000"},
  "random_streams": ["synthetic-stream-id"]
}
```

The report includes each imported estimate, its source-record hash, the ratio, and its uncertainty
only when the request gives an `independent_assumed` covariance assessment with a rationale and a
hash-verified evidence file. This remains a declared independence assumption; different random
seed or stream labels do not prove it. With the default `unknown` covariance, the report omits ratio
uncertainty and marks the point `INCONCLUSIVE`. A zero R2S estimate or non-finite ratio arithmetic
also makes the point inconclusive. Otherwise the central ratio is compared with VAL-043's
0.85–1.15 band. The CLI prints this central-band result separately from the overall status, so an
out-of-band ratio remains visible when covariance is unknown. This code-to-code comparison does not establish experimental validation, dose
compliance, or correctness of either solver result. Hashes verify byte identity, not scientific
correctness.

In the R3 reference workflow, D1S applies each parent nuclide's time factor and sums its spatial
bins within each batch before estimating the standard error. R2S likewise sums spatial bins
within each photon batch. Its error is conditional on the generated photon source: neutron
transport and activation-source uncertainty are not propagated. Neither method includes model
or nuclear-data uncertainty. The exporter records these scopes and keeps method covariance
unknown. A source-serialization check or a successful export does not validate a dose result.

Both estimate records and referenced artefacts are user-supplied evidence. The CLI refuses an
existing output path and never changes the input files.
