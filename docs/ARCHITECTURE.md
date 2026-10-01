# Architecture

FARIS owns scenario semantics and the coupled research question. Existing
scientific tools supply specialized calculations through explicit adapters.
The native app and CLI use the same Rust operations.

```text
scenario JSON
    ↓
faris-model: validation, units, component identity, source identity
    ↓
faris-engine: geometry → environment → state/history → comparisons
    ├── external adapters: transport, activation, plasma, materials, fuel cycle
    ├── faris-cli: batch/reproduction/export
    └── faris-app: egui workspace + wgpu scene

FARIS study generator → Avila Core compilation → readiness / execution / evidence
```

The current engine implements geometry and explicit empty evaluation records.
The environment, state/history, comparisons, and external adapters are future
work. The app draws the engine's tessellated geometry in an egui paint callback
with a depth buffer; it does not reproduce domain geometry in a second language.

## Current contracts

- `faris-scenario/v0.1` is parsed by the Rust types in `faris-model`. Unknown and
  duplicate fields are rejected. The two alternatives must retain component
  identities and order and share their declared total radial build.
- Geometry lengths use metres, power uses MW, and the horizon uses calendar years.
  The coordinate system is right-handed, with Y vertical.
- `circular-concentric-tori/v0.1` requires a non-self-intersecting torus. The
  preview model supports positive principal lengths from 1e-6 to 10000 m.
- `faris-demo/v0.1` exports full-torus radii and volumes, assumptions, references,
  a hash of exact scenario bytes, and evaluation states. It contains no physical
  fields until a future result contract is implemented.
- Display tessellation and the display cutaway are presentation approximations.
  Volumes are analytic full-torus values. Future solver export must use the full
  scenario geometry, including explicitly modeled penetrations.

## Future adapter boundary

Every adapter will declare accepted inputs, supported domain, units, model/tool
version, data identity, and returned fields. It must return an explicit unresolved
or unevaluated state when input data or physics coverage is missing.

Transport outputs need source-particle normalization, component volumes and
spectra, mesh coordinates, tritium-production rates, heating, statistical error,
and applicable nuclear-data identity. Conversion from per-source-particle tallies
to absolute rates is owned by a tested Rust boundary. Metres versus centimetres,
MW versus watts, and density conventions cannot remain implicit.

Lifetime outputs need separate calendar time and integrated exposure, site and
component inventories, environmental conditions, event history, and service-limit
assumptions. Tritium decays during outages; exposure follows the actual operating
schedule. Replacements reset only their own material state. Fuel processing may
need shorter steps than the reporting timeline.

The first adapters can call external programs or small Python utilities where
an existing solver's Python interface is the practical entry point. Those
utilities prepare/execute the solver and return documented artifacts; they do
not move the authoritative FARIS model or time loop out of Rust.

## Execution and evidence

Before adding execution, introduce a worker/job boundary with finite work limits,
cancellation, and child cleanup. The app loads completed artifacts and sends
requests; it does not block its event thread on solvers.

Use `PASS`, `FAIL`, `INCONCLUSIVE`, and `NOT_EVALUATED` for explicitly scoped
assessments. Geometry validity and solver completion cannot imply engineering
acceptance. Keep sampling error, assumed ranges, data uncertainty, model discrepancy,
and rigorous enclosures distinguishable rather than collapsing them into one band.

The finished demo includes a Core-powered study experience. FARIS expands the
selected domain analyses into their dependencies and generates a reusable study
contract against a pinned registry/profile. Compilation, current artifact/run
readiness, and scientific assessments remain separate. Core validates the
declarations it receives; FARIS must explicitly encode the scientific dependencies.

Use a few coarse execution stages and reusable/generated packaging. Keep the
simulation and its ordinary CLI usable without Core. A small Powered by Avila Core
attribution accompanies actual compiler progress/results. See
[the Core integration plan](../integrations/avila-core/README.md) and
[the demo roadmap](ROADMAP.md). This scaffold contains no Core package or receipts.
