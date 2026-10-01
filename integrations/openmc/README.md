# OpenMC adapter plan

Status: the reactor adapter is planned. Strict Rust transport requests/imports
and normalization are implemented. A separate bundled worker actually invokes
OpenMC for the synthetic absorber-sphere numerical control; it does not model
the FARIS reactor or provide reactor results.

See [the transport boundary](../../docs/TRANSPORT.md),
[independent controls](../../docs/NUMERICAL_CONTROLS.md), and
[scientific baseline](../../docs/SCIENTIFIC_BASELINE.md).

The first adapter should export the complete scenario geometry and stable
component IDs, assign actual materials, and define a normalized D-T source.
It should return component volumes, spectra and heating, total breeding, spatial
field artifacts, statistical errors, tool/data identities, and geometry diagnostics.

OpenMC's Python API may be used by an external worker. A Rust adapter owns the
FARIS input/output contract, unit conversions, work limits, and job state. Keep
both the display cutaway and material-label placeholders out of solver inputs.
Do not reuse results after geometry, source, material, or library inputs change.
