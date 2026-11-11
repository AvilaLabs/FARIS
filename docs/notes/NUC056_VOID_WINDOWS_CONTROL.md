<!-- kind: explanation -->
# Bounded void-window control (NUC-056 support)

`integrations/openmc/risk_tests/void_windows_control.py` runs the existing
iron/hydrogen slab fixture through three separate fixed-source passes:

1. OpenMC MAGIC window generation (30 batches × 2,000 particles, one thread,
   the reserved window-generation seed);
2. analogue detector-void response (100 × 10,000, seed 101);
3. windowed detector-void response (40 × 5,000, seed 202).

The control uses the checked FENDL-3.2 H1 and Fe56 files named by
`FARIS_CI_NEUTRON_DATA`. It predeclares the existing NUC-051 criteria: each
relative standard error is below 5%, the means differ by less than 3 combined
standard errors, and every completed pass records zero lost-particle events
from the exact selected CLI-linked OpenMC C API library. Each run has a fresh
directory and no sourcepoint/restart input. The generation output is used only
as a weight-window map. The receipt records method, seeds, solver/runtime/data
hashes, exact statepoint counts, per-void mesh bins, window bounds, pass
receipts, response estimates, and a canonical digest.

Run it serially with the selected OpenMC 0.15.3 Python and the CLI beside that
Python. The coordinator should supply the local H1/Fe56 data folder and a new
ignored output directory:

```bash
FARIS_CI_NEUTRON_DATA=/path/to/verified-H1-Fe56 \
  /path/to/openmc0153/bin/python integrations/openmc/risk_tests/void_windows_control.py \
  --openmc-executable /path/to/openmc0153/bin/openmc \
  --output-dir runs/continuation/void-windows-control-<unique-id>
```

Each of the three solver passes has a 600-second timeout. The output directory
must not exist. A successful run writes `control.json`; its statepoints and logs
are retained with it. The control is Linux-specific because it verifies the
selected executable's `$ORIGIN/../lib` linkage.

## Scope

This control audits the slab fixture's one explicit CSG detector void cell
(cell 12), and verifies positive finite windows in every active energy group
for mesh bins 60–64. An entirely inactive energy group is reported separately;
it is not treated as a partially covered active map. This does not inventory a
CAD/DAGMC model, count its implicit complement as a void, or qualify every CAD
void. A `PASS` means only that this one bounded fixture met its declared
criteria. It does not set NUC-056 Met.

Distinct seed labels and separate fresh run directories establish that this
control supplied no generator sourcepoint/restart history to either production
pass. They do not prove the underlying OpenMC pseudorandom sequences are
mathematically non-overlapping; the receipt records `stream_nonoverlap` as
`NOT_PROVEN`, so NUC-053 remains partial.

To close the full CAD scope, the later design-run audit must bind the checked
GEO-039 void-solid inventory and GEO-022 sampled-cavity records to the exact
converted H5M hash, enumerate every loaded DAGMC cell (including OpenMC's
implicit-complement cell when present), and map those cell IDs back to the
source void/cavity inventory. For every listed cell it must record the applied
mesh bins and active energy-group bounds intersecting that cell, the configured
split cap, and the model/run hashes. The actual production receipt should carry
a per-void row with source inventory IDs, DAGMC cell ID/material or complement
identity, mesh-bin coverage, window-bound audit, and the cell's analogue and
windowed response mean/error where that response is defined. Include the actual
selected linked-library loss count and reject unknown counts. The imported
CAD void list alone is insufficient: include the runtime DAGMC cell count and
implicit complement so no transport void is silently omitted.
