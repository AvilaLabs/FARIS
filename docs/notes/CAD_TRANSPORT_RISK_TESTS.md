# CAD transport risk tests (stage S0)

Status: protocol frozen 2026-10-09, before any run. Results are appended in a
section below and never edit this protocol. Any change to a rule after the
first run is an amendment with a date and reason. Plan:
[NEXT_RELEASE_PLAN.md](../NEXT_RELEASE_PLAN.md), risks 1, 2, 3 and 6.

Each test answers one question with a pass rule fixed here. A failed rule is a
result, not a reason to retune until it passes. Every configuration tried is
reported, including those that did worse.

## Reference model RM-M (ARC-like CAD)

Built with Paramak (MIT) or CadQuery, exported as STEP, and converted with
`cad_to_dagmc` 0.14.2. The model is full 360 degrees, because DAGMC refuses
periodic boundaries and a full model avoids sector source fractions. Every
dimension carries a label: published (with its source in
[ARC_GEOMETRY_SOURCES_0.2.md](ARC_GEOMETRY_SOURCES_0.2.md)) or authored.

- **Plasma (published, S15 and K18):** R0 = 3.3 m, a = 1.13 m, κ = 1.84,
  δ = 0.375, D-T at 525 MW, so S = 1.86e20 n/s (SRC-001).
- **Inboard midplane build (published, S15 Fig. 2):**
  - scrape-off layer 3 cm;
  - vacuum vessel 8 cm: 1 cm W, 1 cm Inconel 718, 2 cm FLiBe, 1 cm Be, 3 cm
    Inconel 718;
  - FLiBe 20 cm;
  - tank wall 3 cm;
  - thermal shield 3 cm;
  - TiH2 shield 51 cm;
  - TF inboard leg R = 70–134 cm.
- **Outboard (FLiBe published as "about 1 m", others authored):**
  - FLiBe 100 cm;
  - TiH2 shield 20 cm (authored, since the published value is a gap);
  - layer offsets follow the plasma shape.
- **TF coils:** 18 D-shaped coils (published). The 64 cm radial inboard leg is
  published; the toroidal width and the case/winding-pack split are authored.
  The winding-pack composition is published (45.9 % Cu, 46.1 % steel, 8.0 %
  REBCO).
- **One equatorial port** through vessel, tank and shield. Dimensions are
  authored, matching FARIS 0.2's port.
- **Materials:** FARIS's material baseline
  ([MATERIAL_BASELINE.md](../MATERIAL_BASELINE.md)). TiH2 and REBCO come from
  their published compositions, or are authored with a label.
- **Nuclear data:** the FARIS audited library (FENDL-3.2 neutron + ENDF/B-VII.1
  photon overlay). If that library cannot be used for a step, the step records
  which library it used and why.

**Model acceptance (before any risk test):**

- per-solid faceted volume within 0.5 % of the CAD volume (GEO-030);
- lost particles ≤ 1e-6 per history over 1e6 histories (GEO-025);
- 100 % of sampled source sites inside the plasma volume (SRC-018).

A model that fails acceptance is fixed and re-accepted before the risk tests
start. The fix is recorded.

## R6: OpenMC version

Run the R1 generation step and one analog run with both OpenMC 0.15.3
(`~/.venvs/w003env`) and 0.16.0 (`~/micromamba/envs/openmc016`).

**Rule.** Choose 0.16.0 if both hold:

1. every R1–R3 step runs with no workaround beyond those 0.15.3 needs;
2. the analog TF fast flux agrees between versions within 3σ.

Otherwise choose 0.15.3. The choice and its reason are recorded, and R1–R3 then
use only the chosen version.

## R1: weight-window gain on the magnet (NUC-050, NUC-051, NUC-053)

**Responses:**

- (a) **regional**: TF fast flux (E > 0.1 MeV) integrated over all TF coil
  volumes;
- (b) **peak**: maximum over a mesh on the inboard TF legs at the midplane
  (voxels 5 cm radial × 10 cm vertical, full coil toroidal width), reported with
  its voxel.

**Figure of merit:** FOM = 1 / (R² · T), where T is the production wall time at
4 threads.

**Method:**

- FW-CADIS weight windows come from a random-ray forward and adjoint solve on
  the DAGMC model, with the adjoint source on response (a).
- Generation seeds are disjoint from production seeds (NUC-053).
- Up to six configurations: energy groups (1, 8, 25), window bounds and
  survival ratio, and the objective (a alone, or a + b). All six are reported.

**Analog reference:** the same model with no windows.

- Its FOM counts only if response (a) reaches R ≤ 0.1 within a 2-hour budget at
  4 threads.
- Otherwise the gain is reported as a lower bound, from the analog R reached at
  the budget, and labelled so.

**Rules:**

- **PASS:** the best configuration's FOM gain on (a) is ≥ 100×.
- **Unbiased (NUC-051):** windowed and analog (a) agree within 3σ. This is
  NOT_EVALUATED if the analog R is above 0.1.
- **FAIL:** gain < 100×. The measured gains go to change control for NUC-050's
  provisional target.
- (b) is reported without a pass rule.

## R2: laptop preview (PERF-025)

**Machine:** this laptop, which is the reference laptop (Intel Core i3-N305,
8 cores, 30 GB).

**Run:**

- Production model with the chosen R1 windows, 8 threads, nothing else heavy
  running.
- The run is stopped at 60 s wall time, counting process start and
  cross-section loading.
- One-time per-design costs (CAD conversion, multigroup cross sections,
  random-ray solve) are excluded from the 60 s and reported separately with
  their peak memory.

**Rule:**

- **PASS:** at 60 s, the TBR relative error is ≤ 2 % (1σ) and response (a) is
  ≤ 20 % (1σ). These are FARIS choices: the levels at which a preview still
  ranks designs.
- **FAIL:** the achieved errors at 60 s go to change control for PERF-025.

## R3: shutdown dose (VAL-043)

**Scenario:**

- Irradiation: 1 full-power year at 525 MW, then cooling times of 1 d, 7 d and
  30 d.
- Dose: photon effective dose rate with ICRP-116 AP coefficients
  (`openmc.data.dose_coefficients('photon', 'AP')`).
- Scored region (authored): a mesh in the equatorial port duct and the adjacent
  plasma chamber, where remote-handling equipment would work.
- Activation chain: `~/nuclear-data/p32-work/chain/depletion/chain.xml` (ENDF
  decay + TENDL-2025 reactions). The same chain is used for both methods.

**Methods:**

- **D1S:** `openmc.deplete.d1s`.
- **Mesh-based R2S:** `openmc.deplete.R2SManager`, the rigorous two-step method.

**Rules:**

- **PASS:** at every cooling time, the ratio of D1S to R2S total dose over the
  scored mesh is within 0.85–1.15.
- **Voxel check:** for every voxel where both methods have relative error
  ≤ 10 %, the ratio distribution is reported (median, 5th and 95th percentile).
- **Conservation:** the R2S decay-photon source must integrate to the
  inventory's photon emission to 1e-6 relative (ACT-023).
- **FAIL:** ratio outside the band at any cooling time. Report which nuclides
  dominate the difference.

## Execution rules

- Every OpenMC or Python job runs under `systemd-run --user --scope -p
  MemoryMax=6G -p MemorySwapMax=0`.
- Thread limits: 4 threads per job and at most two jobs at once. The R2 timing
  run uses 8 threads and runs alone.
- Scratch goes in `~/.cache/avila-night/cad-risk-tests/`. Scripts go in
  `integrations/openmc/risk_tests/` with unit tests.
- The result summary is committed as `references/cad-transport-risk-tests.json`
  plus a results section here.
- A wrapped run that outlives its timeout is stopped by its own PID and reported.

## Results

Appended after runs. The protocol above is unchanged (sha256 b18e1e09…).

### Model RM-M (built 2026-10-09)

- **Build:** CadQuery 2.8.0, not Paramak. Paramak 0.10.0 has no graded
  inboard/outboard build, no conformal coil and no port builder.
  - The model has 30 solids: the plasma, 11 layers, the port duct and 18
    TF coils.
  - The protocol's route was used unchanged: STEP export, re-import, then
    `cad_to_dagmc` 0.14.2 with the default `cad-to-dagmc-mesher` backend and
    imprint on.
  - Tolerance is 0.1 cm and angular tolerance 0.1 rad, giving 1,916,482
    triangles.
  - STEP sha256 79b1481b…; h5m sha256 24fda318….
- **Labels:** 23 dimensions published, 17 authored. Deviations, each recorded
  in the model card:
  - **D1:** the published minor radius puts the inboard separatrix at
    R = 217 cm, while S15 Fig. 2 puts it at 223 cm. All published thicknesses
    are kept, so the inboard TF leg sits at R = 64–128 cm, not 70–134 cm.
  - **D2:** the plasma shape is R = R0 + a cos(t + δ sin t), matching
    `openmc-plasma-source`. Its CAD volume is 145.7 m³, against the published
    141 and 137 m³.
  - **D3:** the coils are 18 wedges of 16 degrees cut from a D-shaped ring with
    a 64 cm normal offset. Layer thickness is graded by cos θ between the
    inboard and outboard values, which is exact at both midplanes.
  - **D4:** each coil is one solid, with the case and winding pack homogenised.
  - **D5:** every poloidal profile is a closed polygon of 240 points, not a
    periodic spline. The reason is that OCCT imprinting of spline surfaces of
    revolution produced inverted and duplicated regions with no error: for the
    layers alone, the scrape-off layer got a region of minus the plasma volume.
    OCCT volume integration on those surfaces was also off by up to 1 %, and by
    15× on one port-cut layer. With polygons the imprint is clean and twice as
    fast, and each uncut solid's volume matches Pappus within 1e-6. The
    geometric cost is a chord sag of at most 0.095 cm, and a departure of the
    normal layer thickness from the published value of at most 0.43 cm, almost
    all of it in the graded TiH2 shield.
- **Cost (reference laptop):** the build (STEP, imprint, mesh and both h5m
  files) took 23 min 10 s at a peak of 4.94 GB.

### Model acceptance: PASS

| Check | Rule | Result |
| --- | --- | --- |
| GEO-030 faceted vs CAD volume | ≤ 0.5 % per solid | worst −0.0148 % (`first_wall_tungsten`); all 30 solids pass |
| GEO-025 lost particles | ≤ 1e-6 per history over 1e6 | 0 lost in 1,000,000 histories |
| SRC-018 source sites in the plasma | 100 % | 200,000 of 200,000 inside (also 100 % without the cell constraint, and against the analytic polygon) |

Notes:

- The mesher's own pre-imprint overlap scan stopped at its 60 s budget with
  283 solid pairs unchecked. Its mesh-level overlap check, which covers every
  solid, found no overlap. FARIS's own GEO-021 check (stage S1) has to close
  this, not rely on the mesher.
- `rel_max_lost_particles` was set to 0.5, because OpenMC rejects 1.0, so a
  run with losses still completes and is counted.

### R6: OpenMC version: 0.15.3

- **Rule 1 fails on 0.16.0:**
  - MGXS generation needed an extra workaround: the stochastic-slab model
    does not inherit `model.materials.cross_sections`, so
    `openmc.config['cross_sections']` had to be set.
  - The random-ray FW-CADIS solve then aborted ("No zero or negative total
    macroscopic cross sections"), because the 0.16.0 MGXS file gives the
    low-density filler material a negative total in one group. The 0.15.3
    file has none.
  - The windows file and the windowed run were therefore not reached.
  - On 0.15.3 every step ran.
- **Rule 2 holds:**
  - The analog TF fast flux agrees between the versions within 0.34σ.
  - The check is weak, because the relative errors are 32–37 % after 1e6
    histories.
  - The absolute value's unit label is under review (an R6 note, below).
    A common scale factor does not change the z-score.
- **Choice:** 0.15.3. R1 to R3 use only 0.15.3.
- **Open note, not part of the rule:** the same 1e6-history analog run took
  146 s on 0.16.0 and 1980 s on 0.15.3, with identical model XML. Why is not
  yet known. It matters for PERF-025 and for when FARIS moves to 0.16.
- **Note on the R6 flux value:** the stored analog response was labelled "per
  source particle", but its magnitude (1.8e17) is not a per-particle track
  length. The normalisation is being traced before any absolute value is used.

### R1 configurations (declared 2026-10-09, before any R1 run)

| Config | Energy groups | Objective | Window settings |
| --- | --- | --- | --- |
| C1 | 1 | (a) | OpenMC defaults |
| C2 | CASMO-8 | (a) | defaults |
| C3 | CASMO-25 | (a) | defaults |
| C4 | CASMO-8 | (a) + (b) | defaults |
| C5 | CASMO-8 | (a) | wider: upper/lower ratio 10, survival ratio 5 |
| C6 | CASMO-25 | (a) + (b) | defaults |

- Objective (a) + (b) uses two adjoint sources, each weighted by the inverse
  of its forward estimate.
- Each windowed production run gets 1800 s wall time at 4 threads.
- The analog reference gets 7200 s at 4 threads.
- Generation and production seeds are disjoint and recorded.
