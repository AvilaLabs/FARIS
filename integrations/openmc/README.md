# OpenMC adapter plan

Status: planned. No OpenMC invocation or transport result is present.

The first adapter should export the complete scenario geometry and stable
component IDs, assign actual materials, and define a normalized D-T source.
It should return component volumes, spectra and heating, total breeding, spatial
field artifacts, statistical errors, tool/data identities, and geometry diagnostics.

OpenMC's Python API may be used by an external worker. A Rust adapter owns the
FARIS input/output contract, unit conversions, work limits, and job state. Keep
both the display cutaway and material-label placeholders out of solver inputs.
Do not reuse results after geometry, source, material, or library inputs change.
