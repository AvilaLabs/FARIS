# Dose-governed replacement duration

FARIS can couple replacement durations to **ICRP-116 AP photon effective dose rate** at a named remote-handling task location. The metric is `icrp116_ap_effective_dose_rate`, reported in `Sv/h`. Ambient dose equivalent, contact dose, ACTINV's Gamma-air proxy, and other quantities are not interchangeable with this criterion.

The user authors a dose threshold and a task-location identity in `faris-maintenance-assumptions/v0.2`:

```json
{
  "schema_version": "faris-maintenance-assumptions/v0.2",
  "governing_quantity": "dose",
  "classes": {
    "magnet": {
      "component_id": "magnet",
      "governing": [],
      "work_s": 86400.0,
      "threshold": {
        "kind": "dose_rate_limit",
        "value_sv_h": 0.0001,
        "quantity": "icrp116_ap_effective_dose_rate",
        "method": "d1s",
        "source": "<remote handling limit and citation>",
        "location_id": "<authored worker task location>",
        "location_sha256": "<SHA-256 of the exact task-location definition>"
      }
    }
  },
  "cooling": { "min_s": 1.0, "max_s": 31536000.0, "points": 80 },
  "convergence_s": 3600.0,
  "max_iterations": 12
}
```

The `source` and location must be selected for the task being assessed. The dose curve is the total spatial field at that point; FARIS does not add component dose estimates or volume-average them. The dose threshold is an authored limit, not a measured duration.

## Curve bundle

Supply a `faris-maintenance-spatial-dose-curves/v0.2` JSON file (v0.1 is still readable) to the native **Dose-governed maintenance** tool or CLI. The bundle contains a `curves` array with one record per design, exact operating-history hash, replacement class/component/ordinal/shutdown, and task-location identity. Each record has ordered `points`; every point holds an R2S estimate and an optional D1S estimate. Each estimate records the method, metric, `Sv/h`, explicit source normalisation, cooling time, value and required pointwise standard error, physical input hashes and references to the actual hashed geometry, materials, source, irradiation, response, and task-location artefacts, plus a hashed run-record artefact and random-stream IDs. The provider verifies every artefact byte hash, requires D1S/R2S physical bases to match when both are supplied, and checks event and location hashes against the current maintenance event.

The native tool can prepare dose physics from a passing checked CAD Design and its ImportReport, then run a bounded, staged spatial dose calculation for each exact event history. Preparation builds a static full-geometry model and clones the checked numeric material once per solid, with a solid/material-to-history-component map. The neutron stage exports 709-group track length for each activation-mesh/material intersection. The Rust ACTINV engine applies the exact irradiation history for every currently installed component, then supplies all decay emitters to one full-geometry OpenMC photon run per cooling time. The response is the whole-geometry photon field at the authored ICRP-116 AP location. Missing activation channels, out-of-range photon rates, incomplete mesh/material coverage, or failed source/volume conservation refuse the curve; missing data are never assigned zero. The standard error covers photon batches conditional on the activation source and does not represent the full physical-model uncertainty.

The CAD preparation config uses `faris-maintenance-dose-cad-config/v0.2` and requires explicit ACTINV executable, data directory, cache directory, timeout, and photon group count alongside the checked cross-sections, authored solid/history mapping, activation mesh, and response locations. The TENDL-709 library index, NPZ, decay files, executable, OpenMC runtime, geometry, and outputs are hash-bound. The legacy OpenMC depletion route remains readable for previously prepared v0.1 manifests when its chain/data coverage is adequate. Stock D1S is only meaningful for compatible common-history events; a heterogeneous-history ACTINV R2S curve carries explicit D1S `NOT_EVALUATED` and does not satisfy the D1S/R2S validation gate.

Legacy design identities bind the five SHA-256 values for `scenario`, `physics`, `history_run`, `spectrum_run`, and `history_assumptions`. Native CAD dose designs bind `design_file`, `history_run`, `history_assumptions`, and `dose_physics`; the CAD history-rate artefact binds the checked CAD design ID/hash and the fusion power from that design. The design-linked operating-history assumptions can be explicitly overridden as planning inputs without changing the neutron response identity. Each curve also names `history_sha256`, computed as SHA-256 over `serde_json::to_vec(&HistoryResult)` for the precise history used to build that event's irradiation, the installation start time, the authored irradiation condensation rule, and the irradiation schedule hash. FARIS rebuilds and checks that exact schedule from the current event history. Each point's `physical_input_sha256.event` is computed with the engine's `event_identity_sha256` helper from the design, history, replacement, installation, irradiation schedule and location identity. The replacement key includes the component, replacement number `k`, and exact shutdown time. A fixed-irradiation curve cannot stand in for a later whole-life event.

## Native checked CAD whole-life route

The Design flux window can optionally create history rates from an actual checked CAD design. Enable **Whole-life history rates** and explicitly map every active solid to a history component; the `blanket` component is also named for the H3 response. Source identity and fusion power come from the checked CAD design. FARIS keeps the mapping and design bound to that transport run and discards the continuation if its inputs change.

After a successful transport run, continue through coupled heating. The whole-model energy response must independently meet its target and closure check before FARIS combines it with the CAD history rates. The winding-pack peak remains a separate response. In the Dose-governed maintenance window, the design-linked `OperatingHistoryAssumptions` are used by default; an explicit file can override planning assumptions. Prepare or choose CAD dose physics from the same checked design, solid mapping, and cross-section index. FARIS verifies those identities before it writes a new `faris-maintenance-cad-dose-designs/v0.2` input. That input contains `design_file`, the hash-bound history-rate artefact, optional `history_assumptions`, and the dose-physics manifest; it does not need a separate CSG scenario, physics file, or spectrum run.

The resulting whole-life run uses the engine's maintenance calculation. It presents the authored fixed-duration baseline beside the dose-governed durations, replacement downtime, availability, and net electric output, with validation scope and next steps. The comparison identifies the checked CAD design and source hashes. The ordinary in-app arrangement history remains a separate study; it is not relabeled as CAD-derived history. CAD dose preparation and spatial dose curves still require the authored task locations, dose assumptions, and event-specific validated inputs described above.

The run-record JSON is required to repeat the method, cooling time, estimate and standard error, response quantity/unit/normalisation, six input artefact hashes, random streams, and exact maintenance event identity. Its event identity includes the design input hashes, history, component/replacement number, shutdown and install times, irradiation schedule and task location. Merely changing the outer bundle hash or event label cannot relabel an unrelated historical run.

FARIS selects the authored method (D1S by default, R2S if selected) and records D1S/R2S central-value ratios against the 0.85–1.15 declared comparison band. Comparison covariance defaults to `unknown`; in-band values then remain `INCONCLUSIVE_COVARIANCE`. To calculate a pointwise ratio standard error, supply `covariance: {"kind":"independent_assumed","rationale":"...","evidence":{"path":"...","sha256":"..."}}` and a hash-verified evidence file. This is an authored independence assumption, not an inference from random-stream IDs. An out-of-band central value makes the event NOT_EVALUATED. Pointwise standard errors do not establish cooling-time uncertainty, so that uncertainty remains NOT_EVALUATED.

The manifest and every run-record artefact are limited to 16 MiB. Physical input artefacts are streamed and hash-checked up to 256 MiB each, with a 512 MiB aggregate hash budget per validation operation. Shared artefacts are charged once while their file identity and metadata remain stable; a changed file is rehashed and must still match its declared hash. A bundle is limited to 100,000 events and 512 cooling points per event. If a history iteration changes and there is no exact matching event curve, that iteration is NOT_EVALUATED. The coupling loop does not reuse a stale curve.

Example CLI invocation for a previously prepared and hash-verified event-curve bundle:

```sh
faris maintenance dose-run \
  --designs designs.json \
  --assumptions dose-maintenance.json \
  --spatial-dose-curves event-curves.json \
  --output dose-maintenance-result.json
```

When the native dose workflow launches the OpenMC event worker, it uses the selected Python
interpreter and the shared bounded-job runner. The worker receives an allowlisted environment:
`PATH` starts with that interpreter's directory, `PYTHONPATH` is taken only from the documented
`FARIS_PLASMA_SOURCE_PATH` setting (the directory containing `openmc_plasma_source`), and
`PYTHONDONTWRITEBYTECODE=1`; unrelated parent-process variables are cleared. Install the Python
dependencies in the selected interpreter. Cancellation and the run budget stop the whole worker
process tree, while memory, captured logs, and generated artefacts are bounded. Each event's
work directory retains `worker.log` and `worker-job.json` with captured output and runner limits.

FARIS does not model evolving geometry. Results retain the explicit neutron-response, activation, photon-transport, geometry, composition and nuclear-data limitations in their receipts; they are not validated facility dose or an operational release.
