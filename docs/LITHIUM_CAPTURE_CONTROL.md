# Coupled thermal Li-6 capture control

This is a separate OpenMC numerical control for checking the coupled neutron,
photon, charged-particle heating tallies and tritium-production score. It is
not a FARIS model, blanket result, materials qualification, evaluation
validation, or engineering prediction. The medium is an authored mathematical
idealization: pure Li-6 at `0.01 atom/barn-cm`, with no asserted physical
temperature, in a 10 cm radius vacuum-bounded sphere. The source is one
isotropic 0.0253 eV neutron at the center. All listed responses are per source
neutron; the control does not apply a reactor source rate or volume
normalization.

OpenMC 0.15.3 documents flux in particle-cm/source-particle, reaction rates in
reactions/source-particle, `H3-production` as tritium-particle production, and
`heating` in eV/source-particle. Its neutron heating score corresponds to
NJOY/HEATR MT=301; photon heating is direct photon energy deposition. The
control makes separate neutron, photon, electron, and positron `heating`
tallies, plus the directly scored total over all particle scopes. Under the
selected `led` electron treatment an electron-specific response can be zero
because electron energy is deposited locally during photon transport; it is
still explicitly included in the component identity. The run record and
independent checker verify all nine score definitions, filters, nuclides,
estimators, means, and standard errors against the raw statepoint. The
all-particle tally's own mean and standard error remain the reported total;
component uncertainties are used only for the closure screen, not combined to
invent a total uncertainty. [Pinned OpenMC tally documentation](https://github.com/openmc-dev/openmc/blob/v0.15.3/docs/source/usersguide/tallies.rst),
[pinned OpenMC energy-deposition implementation](https://github.com/openmc-dev/openmc/blob/v0.15.3/src/tallies/tally_scoring.cpp)

The independent energy screen derives two reaction Q values from neutral atomic
masses in AME2020: Li-6(n,t)He-4 and Li-6(n,gamma)Li-7. The independent checker
uses decimal arithmetic and cross-checks both against the local evaluated
library within a predeclared 1 keV per-reaction allowance. [AME2020 mass table](https://www-nds.iaea.org/amdc/ame2020/mass_1.mas20.txt)

Before the final run, acceptance was fixed as follows:

- `H3-production` must agree with `(n,t)` events within three times the sum of
  their reported standard errors plus `1e-10` particles/source.
- Neutron absorption must agree with `(n,t)` plus `(n,gamma)` within three
  times the sum of those three standard errors plus `1e-10` reactions/source.
- Total heating must lie between the tritium-weighted Li-6(n,t) Q contribution
  (minus the source energy) and that contribution plus the full Li-6(n,gamma)
  Q (plus source energy). The interval is expanded by three times the
  propagated tally standard errors, the 1 keV Q cross-check allowance per
  reaction, and a 0.1% relative model floor.
- All-particle total heating must agree with the sum of the neutron-, photon-,
  electron-, and positron-filtered heating tallies within three times the sum
  of the directly reported standard errors plus `1e-6 eV/source`.

These three-standard-error tests are practical screening rules, not rigorous
confidence statements. Gamma local deposition is deliberately bounded from
zero to the full capture Q because the control does not separately tally
escaping photon energy. A PASS only means the exact recorded control satisfies
these response identities and energy bounds.

The earlier neutron/photon-only component screen is retained as
[`lithium-capture-control-limited-screen-superseded.json`](../references/lithium-capture-control-limited-screen-superseded.json)
for audit history; it is superseded because it omitted electron and positron
scopes from the heating closure.

## Reproduce and inspect

Use the same OpenMC 0.15.3 executable and Python environment identified in the
run record. The local combined library is ignored by Git and must be acquired
and audited separately; its FENDL acquisition provenance and redistribution
terms are unresolved. See [photon data acquisition](PHOTON_LIBRARY_ACQUISITION.md).

```sh
OPENMC_PY=/path/to/openmc-0.15.3/bin/python
OPENMC_EXE=/path/to/openmc-0.15.3/bin/openmc
"$OPENMC_PY" integrations/openmc/lithium_capture_control.py \
  --out runs/lithium-capture-control-reproduction-001 \
  --openmc "$OPENMC_EXE" --seed 20261001 \
  --batches 100 --particles 100000 --threads 8 --timeout 600
"$OPENMC_PY" controls/check_lithium_capture.py \
  runs/lithium-capture-control-reproduction-001/control-result.json
```

The generator bounds each child process to 4 GiB address space and 256 MiB
per-file output, checks a 512 MiB/256-file output-tree limit, and enforces a
wall timeout. These are resource controls for the identified local solver, not
a hostile-code sandbox. The checker reopens the statepoint using OpenMC's
Python API and rehashes the executable, library inputs, statepoint, and output
tree.

The 10-million-history local result is summarized in
[`lithium-capture-control-verification.json`](../references/lithium-capture-control-verification.json).
The statepoint itself and nuclear-data files remain local and ignored; the
reference summary carries hashes and responses, not a redistributed simulation
artifact.
