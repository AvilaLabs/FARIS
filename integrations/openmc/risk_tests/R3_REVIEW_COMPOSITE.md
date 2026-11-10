# R3 full-chain CE review composite

`r3_review_composite.py` creates a new, review-only OpenMC cross-section index
from the downloaded TENDL-2025 candidate, adds the exact local FENDL-3.2 H1/H2
files, and adds the Ne24/S42 NJOY DMIN pilot conversions. It does not edit or
replace the downloaded TENDL or project FENDL/ENDF indexes or HDF5 inputs.
Every output directory and report path must be new.

The preparation command checks the candidate index identity, actual FENDL H-file
hashes, pilot source/output/NJOY hashes and pilot structural audit receipts. It
does not hash the multi-gigabyte TENDL archive or the thousands of candidate
HDF5 files:

```bash
OPENMC_PY=/home/connoravila/micromamba/envs/openmc0153dd/bin/python
PYTHONPATH=integrations/openmc/risk_tests "$OPENMC_PY" \
  integrations/openmc/risk_tests/r3_review_composite.py prepare \
  --out-dir runs/continuation/r3-ce-review-20261109
```

The full audit is an explicit, serialized I/O task. It hashes each
chain-required HDF5 file, verifies the reachable depletion-chain channels,
checks their 294 K datasets, and confirms energy grids reach the R3 20 MeV
response upper boundary. It also checks chain, materials, index, and data file
identities remain stable during the audit. Use the exact serialized materials
XML bound to the planned R3 RM-M model:

```bash
PYTHONPATH=integrations/openmc/risk_tests "$OPENMC_PY" \
  integrations/openmc/risk_tests/r3_review_composite.py audit \
  --review-index runs/continuation/r3-ce-review-20261109/cross_sections.review.xml \
  --chain /home/connoravila/nuclear-data/p32-work/chain/depletion/chain.xml \
  --materials runs/continuation/s0g-rm-m/materials.xml \
  --out runs/continuation/r3-ce-review-20261109/coverage-audit.json
```

The result can report `STRUCTURAL_CE_PRESENCE_PASS_UNQUALIFIED`, but
`transport_eligible` remains false. That status confirms only indexed nuclides,
chain MT groups, 294 K arrays, and the selected energy boundary are present. It
does not qualify evaluation provenance, processing equivalence, URR treatment,
covariance, or depletion-model accuracy. The candidate TENDL evaluation basis
is unmatched to the R3 D1S FENDL-3.2/ENDF-B-VII.1 basis, so this review cannot
support a matched-data VAL043 claim. The pilot audit records are structural and
descriptive; they are not physical validation.

The two DMIN pilot receipts bind the original TENDL source bytes, NJOY
executable, OpenMC 0.15.3 output, and the corresponding 13/13 selected NJOY
CTest log. The review plan also records the pilot structural/descriptive audit
hashes and the exact 294 K/200 MeV conversion settings. A passing processor test
and sample processing audit establish reproducibility and structure, not
evaluation accuracy.
