# Optional MCNP neutron importance-zero support for OpenMC 0.15.3

This optional patch implements the narrow MCNP `imp:n=0` behavior required by
FNS neutron benchmarks whose zero-importance CSG cells contain other geometry
regions as well. OpenMC 0.15.3 and 0.16.0 do not provide a per-cell neutron
importance or kill-on-entry control. A vacuum boundary on one enclosing sphere
is not equivalent when the MCNP zero-importance cell is a union that also
covers interior regions.

The patch is specific to the FARIS OpenMC 0.15.3 AIM-patched source. From the
OpenMC source root, apply the AIM patch first, then apply this patch with its
FARIS checkout path:

```bash
git apply /path/to/project-faris/integrations/openmc/patches/faris-openmc-0.15.3-aim.patch
git apply --unidiff-zero /path/to/project-faris/integrations/openmc/patches/faris-openmc-0.15.3-mcnp-imp0.patch
```

The patch was checked for clean application against the local 0.15.3 AIM-patched
source snapshot used by FARIS;
this check is not a compile or runtime validation. Current hashes:

- AIM patch: `8a72af3a15891f5b1972b26427ad4fd414cd6e491fc42db8a0e9262ecbc89dcc`
- MCNP importance-zero patch: `c0f6bffede6d25ccfee8251404a22e50c01228c5346683ea3884cf0b10f8da37`

Do not use this patch for ordinary reactor or maintenance runs. Those models
must not contain the opt-in attribute and remain supported by stock OpenMC.
For the two FNS N/O benchmark cases, the runner must require the exported C API
symbol `faris_openmc_mcnp_imp0_patch_version` to equal `1` before launch. A
stock executable ignores unknown XML attributes, so a missing capability must
stop the run before transport. The runner must also record the final geometry
XML hash after attribute injection and retain the separate kill count in the
run receipt.

## Input contract

After exporting the final OpenMC `geometry.xml`, the benchmark runner checks the
source-deck hash and the reviewed MCNP-to-OpenMC sidecar mapping. For each
verified neutron cell with MCNP importance zero, it sets exactly:

```xml
<cell id="1" name="..." material="void" region="..."
      faris_neutron_importance="0" />
```

The illustrative ID must be replaced by the sidecar's verified OpenMC cell ID.
The attribute is injected after standard OpenMC serialization; stock OpenMC
Python classes are unchanged. It may only have the exact value `0`. Other
values fail during geometry loading. The benchmark's ordinary OpenMC cell name
remains provenance text and does not control physics.

The marker means: terminate a neutron when a surface crossing moves it into
that CSG cell. A neutron born there is not killed by this feature; the control
must start source neutrons in a separate live cell. The particle carries its
incoming weight to the boundary tally, then its weight is set to zero before it
can score track length in the zero-importance cell. The surface weight-window
operation is skipped on this entry, so roulette cannot replace the explicit
MCNP kill. This does not model absorption and does not increment OpenMC's lost
particle counter. The patch exposes the independent cumulative counter
`faris_openmc_mcnp_imp0_kill_events`; it is an intentional variance-reduction
termination count, not a physical reaction count. The patch does not change
particle normalization or any non-marked cell's behavior. Its counter is
thread-safe for OpenMP and the control below uses one MPI process, matching the
standard FARIS worker's single-process launch.

## Required small runtime control

Run this control using the exact selected patched OpenMC CLI and its linked
`libopenmc.so`. Use one fixed-source batch of 1,000 histories, seed 271828,
unit source strength, no weight windows, no variance reduction, and no MPI
launcher. It can be built through OpenMC 0.15.3's Python API and then injected
with the XML attribute above.

Geometry is three concentric void cells in the root universe:

- cell 1: sphere `r < 1 cm`, live source cell;
- cell 2: shell `1 < r < 1.5 cm`, live cell;
- cell 3: shell `1.5 < r < 2 cm`, marked with `faris_neutron_importance="0"`;
- the radius-2 sphere is a vacuum boundary.

Place an isotropic 14.1 MeV neutron point source at the origin. Add track-length
tallies for cells 1, 2, and 3. Check the capability symbol before `openmc_init`,
then read the normal lost counter and the FARIS kill counter before simulation
finalization.

Expected for neutrons: 1,000 intentional kill events, zero lost particles,
about 1 cm mean track length in cell 1, 0.5 cm in live cell 2, and zero in cell
3. Tolerances should be tight enough to catch a premature kill at the first
live-cell crossing; the exact track-length means are deterministic in this
void geometry. Re-run with a photon source and photon transport enabled: the
neutron-only marker must produce zero FARIS kills; photons traverse the marked
cell and leak at the vacuum boundary without a geometry-loss count. As a
negative control, remove the attribute and rerun neutrons: the FARIS kill
counter must be zero and neutrons traverse cell 3 to the vacuum boundary.

These controls establish only the patch's entry-kill semantics, counter
separation, and lack of interference with live-cell and photon transport. They
do not validate the FNS benchmark or reactor physics. The full N/O benchmark
run still needs its recorded source, geometry, code, tally, and uncertainty
evidence.
