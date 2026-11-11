# Compensated scalar heating for OpenMC 0.15.3

Apply `faris-openmc-0.15.3-compensated-heating.patch` to clean OpenMC commit
`27e38e894697bb32a1dac7848d2618818b6b8daf`. It carries the existing FARIS
compensated heating prototype and the guard against unsupported configurations.

The five `response-heating-*-whole-model` tallies use compensated event, batch
sum and sum-of-squares accumulation. Their versioned checkpoint records retain
the accumulator state. This prototype requires x86-64 Linux extended precision,
one MPI rank, reduced writable scalar heating tallies and the total-material
nuclide bin. The recorded control uses one thread.

The guard requires continuous-energy transport and an analog or collision
estimator. It rejects a matching-name multigroup tally or tracklength tally
during tally initialization. The C API also rejects a later change to
tracklength and preserves the previous estimator. Ordinary tally names retain
their normal estimator behavior.

The focused regression control is `controls/nuc013_tally_guard.py`. Run it with
the candidate OpenMC Python environment and explicit input paths.

```bash
/path/to/prefix/bin/python controls/nuc013_tally_guard.py \
  --prefix /path/to/prefix \
  --template /path/to/frozen-rm-s/solver \
  --cross-sections /path/to/cross_sections.xml \
  --out /path/to/new-control-output
```

The control uses separate processes for XML initialization, a late C API
estimator change and an ordinary-name comparison. It generates a synthetic
one-group cross-section fixture for the multigroup check. The tracklength
rejections must identify the new guard. OpenMC's existing score validator
rejects multigroup heating before the accumulator's defensive energy-mode
check is reached, and the control records that origin explicitly.
The receipt also records the supported one-port RM-S
closure and continuous-versus-restarted comparison.

See `references/rm-s-heating-guard-control-20261111.json` for the measured scope,
runtime and patch hashes. NUC-013 remains Partial while the wider runtime and
output audit is open.

OpenMC's upstream copyright and MIT terms are reproduced in
`LICENSE-OpenMC-MIT`. Preserve that notice with the modified OpenMC source and
runtime. FARIS retains its repository license for its contributions.
