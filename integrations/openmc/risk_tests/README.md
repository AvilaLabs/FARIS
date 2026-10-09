# CAD transport risk tests: RM-M model, acceptance and R6

Scripts for stage S0 of the next-release plan. The frozen protocol is
[docs/notes/CAD_TRANSPORT_RISK_TESTS.md](../../../docs/notes/CAD_TRANSPORT_RISK_TESTS.md);
this folder implements "Reference model RM-M", its three acceptance checks and
test R6 (OpenMC version). R1 to R3 are not here.

All scratch output goes under `~/.cache/avila-night/cad-risk-tests/`. Nothing
large is committed. The model card (`rm_m_model_card.json`) is the one small
artefact worth keeping with the scripts.

## Files

| File | Runs under | Purpose |
| --- | --- | --- |
| `rm_m_spec.py` | any Python 3 | Dimensions, published/authored labels, poloidal profiles, model card. Standard library only. |
| `rm_m_checks.py` | any Python 3 | Rules: volume comparison, lost particles, source sites, 3-sigma agreement, the R6 choice. Standard library only. |
| `h5m_volumes.py` | numpy + h5py | Faceted volume, centroid and material tag of every volume in a DAGMC h5m file, read straight from the file. |
| `build_rm_m.py` | CAD environment | Builds RM-M in CadQuery, writes STEP, converts to DAGMC h5m, writes the model card and the solid role map. |
| `rm_m_openmc.py` | OpenMC environment | Library identity check, materials from the FARIS baseline, DAGMC model, tokamak source, TF flux tally, analog run helper. |
| `accept_rm_m.py` | OpenMC 0.15.3 | The three acceptance checks. |
| `r6_worker.py` | OpenMC 0.15.3 or 0.16.0 | One R6 step: the R1 generation step, or an analog run, or a window-file comparison. |
| `r6_version.py` | any Python 3 | R6 driver: runs the workers on both versions under the memory cap, applies the rule, writes the choice. |
| `test_risk_tests.py` | any Python 3 | Unit tests for the pure parts (labels, geometry, rules). No OpenMC, CadQuery or numpy needed. |

## Environments

- CAD: `~/.cache/avila-night/feasibility-cad/cadvenv` (Python 3.14, cadquery 2.8, cad_to_dagmc 0.14.2, gmsh, h5py).
- OpenMC 0.15.3: `~/.venvs/w003env`. OpenMC 0.16.0: `~/micromamba/envs/openmc016`. Neither is modified.
- `openmc-plasma-source` 0.9.0 and its dependency NeSST are pure Python. They are installed with
  `pip install --no-deps --target ~/.cache/avila-night/cad-risk-tests/pylib` and put on `PYTHONPATH`, so the
  protected environments stay untouched. Both OpenMC interpreters import them.
- Nuclear data: the FARIS audited library (FENDL-3.2 neutron plus ENDF/B-VII.1 photon overlay), at
  `data/raw/combined-fendl32-endfbvii1/cross_sections.xml` in the main checkout. Override with
  `FARIS_CROSS_SECTIONS`. `rm_m_openmc.library_identity()` checks its SHA-256 against
  `references/openmc-library-audit.json` and that every nuclide RM-M uses is present.

## Memory cap and threads

Every OpenMC or Python job runs under

    systemd-run --user --scope --quiet -p MemoryMax=6G -p MemorySwapMax=0 --

with `OMP_NUM_THREADS=4`, at most two jobs at a time. `r6_version.py` applies the cap itself. For the others:

    CAP="systemd-run --user --scope --quiet -p MemoryMax=6G -p MemorySwapMax=0 --"
    S=~/.cache/avila-night/cad-risk-tests

## 1. Build RM-M

    $CAP ~/.cache/avila-night/feasibility-cad/cadvenv/bin/python build_rm_m.py \
        --out-dir $S/models/rm-m --tolerance-cm 0.1 --angular-tolerance 0.1 --random-ray

Writes `rm_m.step`, `rm_m.h5m` (vacuum as `void`), `rm_m_rr.h5m` (the same
mesh with `void` renamed `filler`, for random ray), `rm_m_model_card.json` and
`rm_m_roles.json` (solid name, material tag, CAD volume, h5m volume id). The
card labels every dimension published (with its source) or authored.

Every poloidal profile is a closed polygon of 240 points (deviation D5 in the model card: OCCT booleans and
volume integration fail on periodic-spline surfaces of revolution). The card records the maximum chord sag and the
maximum departure of the layer thickness from the published value. The CAD volume of each uncut solid is checked
against Pappus' theorem (1e-6), and port-cut solids against the sum of their parts. The solids are exported to STEP,
re-imported and converted with the default `cad-to-dagmc-mesher` backend (`imprint=1`). The TF coil is one homogenised
solid per coil (deviation D4); the tolerance of 0.1 cm gives every faceted solid within 0.02 % of its CAD volume.

The conversion peaks at about 5 GB with one imprint thread (the default);
more imprint threads raise the peak.

## 2. Accept RM-M

    OMP_NUM_THREADS=4 PYTHONPATH=$S/pylib $CAP ~/.venvs/w003env/bin/python accept_rm_m.py \
        --model-dir $S/models/rm-m --run-dir $S/runs/accept --out $S/runs/accept/acceptance.json

1. Faceted volume against CAD volume for every solid, within 0.5 % (GEO-030).
2. 1e6 analog histories with the plasma source: lost particles at most 1e-6 per history (GEO-025).
3. 200 000 source sites sampled with OpenMC's own source machinery, each located in the DAGMC model: all must be
   in the plasma volume (SRC-018). The sites are also checked against the analytic plasma boundary. The result
   with and without the plasma-cell constraint is written.

## 3. R6

    python3 r6_version.py --model-dir $S/models/rm-m --work-dir $S/runs/r6 --out $S/runs/r6/r6.json

On each version: the R1 generation step (continuous-energy MGXS generation, random-ray forward and adjoint
FW-CADIS solve with the adjoint source on the TF fast flux, the weight-window file, and a short windowed run
that loads it), then one analog run. The driver applies the protocol's rule (`rm_m_checks.r6_choice`) and
writes the choice and its reasons. The generation step takes 1 to 1.5 hours on 0.15.3 (random-ray run, then a
windowed run of 2e5 histories), so its default timeout is 3 hours; the analog run takes 35 minutes on 0.15.3. A job that outlives its timeout is stopped by process group and recorded.

## Tests

    python3 -m unittest discover -s integrations/openmc/risk_tests -p 'test_*.py'

## Workarounds recorded by the scripts

OpenMC 0.15.3 on a DAGMC model needs these; `r6_worker.py` lists them per step, and the R6 rule counts any extra
one needed by 0.16.0:

- `source_region_meshes` takes `openmc.Universe(universe_id=dagmc_universe.id)`, not the `DAGMCUniverse`.
- Every volume has a real material for random ray: void is tagged `filler` (H-1, 0.001 g/cm3).
- The random-ray source is a discrete-energy `IndependentSource` constrained to the plasma cell.
- The bounding surface has an explicit `surface_id`.

0.16.0 needed one more for MGXS generation (`openmc.config['cross_sections']`, because the stochastic-slab model does
not inherit `model.materials.cross_sections`); `r6_worker.py` retries once with it and records it on the step.

Model-building notes (CadQuery and OCCT) are in the docstrings of `build_rm_m.py`.
