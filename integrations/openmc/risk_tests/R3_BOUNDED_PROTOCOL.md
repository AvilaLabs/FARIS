# R3 bounded transport and activation protocol

The default ``r3_r2s.py`` backend is ``bounded``. It keeps the neutron response
mesh, activation mesh bounds, energy-group boundaries, irradiation schedule,
source slots, cooling indices, and dose criteria fixed. It samples the exact
OpenMC ``MeshMaterialVolumes`` once, tallies flux in
``MeshMaterialFilter.from_volumes`` row order, then stores only nuclide x
reaction x 42 group-mean cross sections. Per-domain one-group MicroXS values
are reconstructed from the sampled flux in the same domain chunks used for
depletion. Every chunk receipt binds the collapse/flux inputs and hashes the
exact MicroXS bytes consumed. Each chunk's dense depletion result is temporary:
the protocol records its hash, extracts a contribution row for every domain
and cooling state, then deletes the result before the next chunk. Compressed
JSONL records use fixed-mtime gzip and canonical JSON; each contribution
carries the exact OpenMC photon energy-distribution XML, source strength,
unclipped cross-check, per-nuclide photon power, and mesh/material source slot.
Zero-emission domains also get rows. The dose exporter rederives the chunk
MicroXS hashes, checks compressed/decompressed archive bounds, validates all
source rows and totals, and rechecks the full reachable-chain coverage guard.
The record does not claim raw depletion inventories are retained; pinned
inputs remain available for deterministic recomputation.

The backend fails before neutron transport when the data-coverage guard is not
``READY_FOR_R2S_TRANSPORT`` or when the declared memory, output, or disk-reserve
gate fails. An absent reaction channel is a refusal; no reaction/nuclide is
pruned or zero-filled. The direct ``--activation-backend direct`` path remains
available for a separate explicitly selected run. It is never chosen as an
automatic fallback.

Run each discretization as a fresh campaign directory. Use the project-pinned
OpenMC 0.15.3 Python environment and an RM-M model directory with the matching
``rm_m.h5m`` and model card:

```bash
OPENMC_PY=/home/connoravila/micromamba/envs/openmc0153dd/bin/python
PYTHONPATH=integrations/openmc/risk_tests "$OPENMC_PY" integrations/openmc/risk_tests/r3_r2s.py \
  --model-dir runs/continuation/s0g-rm-m --out-dir runs/continuation/r3-bounded-100cm \
  --activation-cell-cm 100 --activation-backend bounded

PYTHONPATH=integrations/openmc/risk_tests "$OPENMC_PY" integrations/openmc/risk_tests/r3_r2s.py \
  --model-dir runs/continuation/s0g-rm-m --out-dir runs/continuation/r3-bounded-50cm \
  --activation-cell-cm 50 --activation-backend bounded

PYTHONPATH=integrations/openmc/risk_tests "$OPENMC_PY" integrations/openmc/risk_tests/r3_r2s.py \
  --model-dir runs/continuation/s0g-rm-m --out-dir runs/continuation/r3-bounded-25cm \
  --activation-cell-cm 25 --activation-backend bounded
```

The standard settings preserve the existing particle counts, batches, seeds,
source-rate schedule, 42-group VITAMIN-J structure, activation time steps, and
photon indices. Any settings change creates a distinct scientific run and
must be recorded as such; do not reuse an existing output directory.

Each run samples actual material-bearing domains before neutron transport and
records exact resource estimates. The compact group-mean XS archive is about
`2190 x 7 x 42 x 8 = 5.15 MB` for the currently required nuclide/reaction/group
counts, independent of domain count. Flux storage is `domain_count x 42 x 8`
bytes per mean or standard-deviation array. Temporary depletion memory is
bounded by chunk size and chain states. Source archive size depends on actual
activated spectra and is limited separately by compressed and decompressed
caps. At 25 cm, the older dense MicroXS estimate was about 9.9 GiB for one
tensor; this protocol does not retain it. The sampled run determines whether
the 25 cm source archive fits its declared cap and refuses if it does not.

The standalone offline collapse-to-CAPI check uses the flux file created by
the run and tests Fe56 capture, Fe56 (n,2n), and Ni58 capture against pinned
OpenMC 0.15.3 ``Nuclide.collapse_rate``:

```bash
PYTHONPATH=integrations/openmc/risk_tests "$OPENMC_PY" integrations/openmc/risk_tests/r3_bounded_microxs.py \
  compare-capi --cross-sections data/raw/combined-fendl32-endfbvii1/cross_sections.xml \
  --fluxes runs/continuation/r3-bounded-100cm/neutron_transport/fluxes.npy \
  --output runs/continuation/r3-bounded-100cm/neutron_transport/capi-check.json
```

This is a bounded numerical implementation check, not nuclear-data
qualification or VAL043 evidence. The composite cross-section index must pass
the existing exact full-chain coverage guard before any R2S neutron transport
or depletion run.
