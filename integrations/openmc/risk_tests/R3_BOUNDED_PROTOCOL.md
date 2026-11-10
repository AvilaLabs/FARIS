# R3 bounded transport and activation protocol

The default ``r3_r2s.py`` backend is ``bounded``. It pins the selected energy
basis and keeps the neutron response mesh, activation mesh bounds, irradiation
schedule, source slots, cooling indices, and dose criteria fixed. It samples the exact
OpenMC ``MeshMaterialVolumes`` once, tallies flux in
``MeshMaterialFilter.from_volumes`` row order, then stores only nuclide x
reaction x selected-group-count mean cross sections. Per-domain one-group MicroXS values
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

## Prospective activation energy checks

The historical default, `VITAMIN-J-42`, starts at 1 keV and excludes lower-energy
neutrons from activation. The 2026-11-10 RM-M acceptance diagnostic found that
95.86% of structure Fe56 MT102 captures occurred below 1 keV. The historical
basis therefore cannot establish the full activation reference by itself.

Two opt-in bounded bases have explicit, hash-pinned edges:
`VITAMIN-J-175-plus-0-to-50MeV` (177 groups) and
`SCALE-252-plus-0-to-50MeV` (253 groups). These extend the named OpenMC structures
to the stated endpoints; they are separate prospective configurations. Keep the
same selected basis across a spatial-grid comparison. An energy-basis comparison
needs its own predeclared runs and does not change the existing spatial-grid or
dose-ratio criteria.

An unresolved interval remains below the selected CE tables' 1e-5 eV first
point. OpenMC's CE transport extrapolates the first tabulated segment there,
while the bounded activation collapse clips its integration to the data grid.
The extended bins alone do not establish that this difference is negligible.
See the pinned [OpenMC nuclide implementation](https://github.com/openmc-dev/openmc/blob/v0.15.3/src/nuclide.cpp).

Before depletion and photon transport, a fresh bounded run can measure material
flux and per-nuclide MT102 captures on each side of that edge:

```bash
OPENMC_PY=/home/connoravila/micromamba/envs/openmc0153dd/bin/python
PYTHONPATH="integrations/openmc/risk_tests${PYTHONPATH:+:$PYTHONPATH}" "$OPENMC_PY" integrations/openmc/risk_tests/r3_r2s.py \
  --model-dir runs/continuation/s0g-rm-m-v3-finalized-v1 \
  --out-dir runs/continuation/r3-energy-177-neutron-only \
  --activation-cell-cm 100 --activation-backend bounded \
  --activation-groups VITAMIN-J-175-plus-0-to-50MeV \
  --below-data-edge-diagnostic --stop-after-neutron
```

Use the same declared composite index for both methods and the existing bounded
job wrapper. `--stop-after-neutron` retains the flux, diagnostic batch arrays and
input identities, then exits before MicroXS collapse, depletion, or photon
transport. Its status is `NEUTRON_STAGE_COMPLETE`, with scientific interpretation
`NOT_EVALUATED`. The diagnostic is a capture/flux measurement, not a bound on
the full activation or dose effect. Both new flags default off.

The following historical 42-group examples exclude sub-1-keV activation.
They do not demonstrate energy convergence. Run each discretization as a fresh campaign directory. Use the project-pinned
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
bytes per mean or standard-deviation array for that historical basis; substitute
the selected group count for an extended basis. Temporary depletion memory is
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

## Serial whole-workflow replicate collection

`r3_campaign.py` freezes the replicate count, all stage seeds, the exact three
activation grids, model dependencies, cross-section index, and implementation
hashes before launching any transport. It runs one D1S calculation per
replicate and one independent bounded R2S calculation at each 100, 50, and
25 cm activation grid, then writes the existing pairwise `r3_compare.py`
reports. All commands run serially. The grids are retained as separate
activation-discretization evidence; the response mesh remains the fixed R3
mesh described above. The driver does not add a convergence tolerance, change
the 0.85–1.15 point band, aggregate a campaign verdict, or claim physical data
qualification. `r3_compare.py` continues to report the D1S/R2S covariance as
unknown. Replicate records preserve the independent stage seeds for a later
predeclared uncertainty analysis.

The runner requires at least two replicates and a seed base. Within each
replicate it assigns disjoint seeds to D1S, R2S neutron transport, and R2S
photon transport; seed blocks for different replicates are also disjoint. It
refuses an existing output directory, stops on the first failed command, and
does not resume or overwrite a partial campaign. For the current conditional
method-control composite and fresh RM-M model, a coordinator can predeclare and
start the collection with:

```bash
OPENMC_PY=/home/connoravila/micromamba/envs/openmc0153dd/bin/python
PYTHONPATH=integrations/openmc/risk_tests "$OPENMC_PY" integrations/openmc/risk_tests/r3_campaign.py \
  --model-dir runs/continuation/s0g-rm-m-v3-finalized-v1 \
  --cross-sections runs/continuation/r3-ce-review-20261109-v2/cross_sections.method-control.xml \
  --python "$OPENMC_PY" \
  --out-dir runs/continuation/r3-method-control-replicates-v1 \
  --replicates 2 --seed-base 20261401 --purpose conditional-method-control \
  --activation-groups VITAMIN-J-175-plus-0-to-50MeV
```

`campaign-plan.json` is immutable pre-run configuration; `campaign-state.json`
records its plan hash and each command, log, exit code, and output-record hash. An
`ALL_COMMANDS_EXITED_ZERO` status means only that commands completed and
expected records were written. It is not a convergence, uncertainty, VAL043,
or physical-validation verdict. Source-response mesh convergence and
activation-grid convergence remain distinct questions.

Summarize the collected artifacts, including incomplete campaigns, with:

```bash
python3 integrations/openmc/risk_tests/r3_campaign_summary.py \
  runs/continuation/r3-method-control-replicates-v1 --out campaign-summary.json
```

The summary checks the recorded artifact identities, retains each replicate and
grid, and reports descriptive ratios and adjacent grid differences. It preserves
unknown covariance and source verdicts; it adds no acceptance threshold.

Before every external command the driver persists its exact guarded command,
start time, UTC deadline, resource limits, and a unique named user systemd
scope. On cancellation, timeout, output/log limit, or a lingering output-pipe
descendant, it explicitly requests `systemctl --user stop` for that exact
scope with a bounded wait, then terminates the client process group as fallback.
It uses the existing R6 user systemd scope guard (`MemoryMax=6G`, no swap),
requires 8 GiB free reserve in
addition to the 8 GiB D1S/R2S output cap (comparisons are capped at 32 MiB),
and caps each log at 32 MiB. The
outer task deadlines exceed the existing inner run budgets to allow
finalization: D1S 7,600 s; R2S 182,700 s (24 h neutron + 24 h activation + three
3,000 s photon budgets + 900 s overhead); comparisons 300 s. The plan records
the sum as a worst-case serial campaign bound. A timeout, output/log limit, or
`KeyboardInterrupt` stops the full process group with SIGTERM followed by
SIGKILL if needed; task state remains recorded as incomplete.
