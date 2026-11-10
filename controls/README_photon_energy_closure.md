<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<!-- SPDX-FileCopyrightText: 2026 Avila Labs -->

# OpenMC photon energy closure control

This is a small execution control for one specific coupling path: OpenMC 0.15.3 transports isotropic 1 MeV photons through an Fe56 sphere, deposits charged-particle energy with LED, and allows Fe atomic relaxation. It checks the whole unfiltered heating score plus the escaping photon energy current against the known 1 MeV source energy. It also partitions heating over two adjacent material IDs of identical Fe56 composition and checks whole versus components batch by batch.

The shell's outer sphere is a vacuum boundary. It has no inbound population, so the `current` tally at that surface is outward current. `EnergyFunctionFilter` weights each crossing by its incident energy in eV. Its interpolation is identity on [0, 2 MeV]; no photon in this source history can exceed 1 MeV. OpenMC 0.15.3 has atomic relaxation enabled by default when photon transport is on and provides no Python toggle to disable it. The control reads the Fe photon HDF5 through `openmc.data.IncidentPhoton` and requires the audited relaxation transitions before it writes inputs. Electrons and positrons use local energy deposition (LED).

## Frozen small run

The default is 40 batches × 5,000 source particles (200,000 photons). Every batch has a statepoint to recover batch means from successive cumulative tally sums. The C API runner already in `integrations/openmc/reactor_transport.py` is used with the exact OpenMC executable-linked library, verified through the ELF RPATH and recorded by SHA-256. The run rejects any nonzero C API lost-particle count and any missing batch statepoint. Hard timeout is 300 seconds. It creates a fresh output directory and refuses overwrites.

Example after coordinator releases the serial solver queue:

```sh
ROOT="$(git rev-parse --show-toplevel)"
PY="${OPENMC0153_PYTHON:?set the selected OpenMC 0.15.3 Python interpreter}"
EXE="${OPENMC_EXECUTABLE:?set the OpenMC executable paired with that interpreter}"
LIB="${OPENMC_CAPI_LIBRARY:?set the exact libopenmc.so linked to that executable}"
INDEX="${FARIS_CROSS_SECTIONS:-$ROOT/data/raw/combined-fendl32-endfbvii1/cross_sections.xml}"
OUT="/tmp/faris-photon-energy-closure-$(date +%s)"
PYTHONDONTWRITEBYTECODE=1 OMP_NUM_THREADS=1 "$PY" "$ROOT/controls/photon_energy_closure.py" \
  --cross-sections "$INDEX" --output-dir "$OUT" --openmc-executable "$EXE" --capi-library "$LIB"
```

The execution writes `control-input.json`, `control-result.json`, the loss receipt, C API logs, and per-batch statepoints. It records the OpenMC Python version and executable hash, exact cross-section XML and Fe data hashes, selected C API library path/hash, geometry, source and settings. The default combined index contains FENDL-3.2 neutron data and the audited ENDF/B-VII.1 photon overlay. The audited combined-library index and photon provenance files must match the supplied data tree.

## Predeclared pass rules

- Energy closure: `abs(mean(H + L - 1,000,000 eV)) <= 5*SE_same_batch + 5,000 eV`, `SE_same_batch <= 20,000 eV`, and absolute mean residual at most 25,000 eV. `H` is unfiltered total heating and `L` is outgoing energy current. The standard error comes from per-batch paired differences, so its variance includes `2 Cov(H,L)`.
- Component closure: each batch's `H - H_inner - H_shell` absolute residual is at most `1e-5 eV/source`.
- Covariance audit: the sample variance of the batchwise `H+L` values must equal `Var(H)+Var(L)+2Cov(H,L)` to `1e-10` relative / absolute tolerance.
- Lost particles: the exact C API `n_lost_particles` delta must be zero, and all 40 requested batches must be present.

The material partition is a tally bookkeeping check. The energy balance is only a photon-source/Fe/LED/atomic-relaxation control using the specified audited data. It does not establish neutron heating, neutron-induced gamma production, reactor material response, blanket performance, or general coupled-heating qualification.

## Observed control run

The coordinator's fresh 40 × 5,000 OpenMC 0.15.3 C API run passed these same criteria: energy-closure mean `6.43049e-8 eV/source`, same-batch standard error `1.61108e-9 eV/source`, maximum component residual `4.84288e-8 eV/source`, and zero lost particles. The first solver run completed, but its post-run analysis stopped because the reader expected `statepoint.1.h5` while OpenMC wrote the padded `statepoint.01.h5`. That was an analysis filename mismatch, not a physics failure. The coordinator reran the analysis path with padded-name handling in a fresh v2 run; the same frozen thresholds passed. Raw solver outputs remain outside the repository.

## Cheap checks (no transport)

```sh
PYTHONDONTWRITEBYTECODE=1 FARIS_CROSS_SECTIONS="$INDEX" "$PY" -m unittest "$ROOT/controls/test_photon_energy_closure.py"
PYTHONDONTWRITEBYTECODE=1 "$PY" "$ROOT/controls/photon_energy_closure.py" \
  --cross-sections "$INDEX" --output-dir /tmp/faris-photon-energy-prepared --prepare-only
```

The second command exercises OpenMC 0.15.3 Python model construction and XML export only; it does not initialize the solver or build geometry through a CAD pipeline.
