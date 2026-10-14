<!-- kind: explanation -->
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

### Units correction (applies to the R6 and acceptance values above)

- **What was wrong:** the R6 and acceptance runs set the source strength to the
  plant rate, 1.8618e20 n/s. OpenMC multiplies fixed-source tallies by the total
  source strength, so the stored R6 analog value (1.78e17) was a rate in
  n·cm/s, not a value per source neutron.
- **Corrected values:** per source neutron, the analog TF fast track length
  (E > 0.1 MeV, summed over the 18 coils) is 9.55e-4 cm on 0.15.3 and 8.09e-4 cm
  on 0.16.0.
- **What does not change:** the R6 z-score is unchanged, because both runs
  carry the same factor.
- **Hand check:** the plasma-cell track length is 680 cm per source neutron at
  strength 1, which is plausible against the plasma's mean chord, and exactly
  1.8618e20 times smaller than at plant strength.
- **Guard for later runs:** R1 to R3 run at strength 1, and the scripts refuse
  to convert between "cm per source neutron" and "n·cm/s at 525 MW" without the
  unit changing (unit-tested).

### R1: weight-window gain on the magnet: FAIL

| Run | R(a) | T (s) | Histories | FOM(a) | Gain | z vs analog |
| --- | --- | --- | --- | --- | --- | --- |
| Analog | 0.196 | 7120 | 3.5M | 3.64e-3 | 1 | — |
| C1 (1 group) | 0.231 | 1761 | 0.15M | 1.07e-2 | 2.93 | −0.04 |
| C2 (CASMO-8) | 0.158 | 1750 | 0.75M | 2.28e-2 | **6.27** | 0.77 |
| C3 (CASMO-25) | 0.218 | 1739 | 0.95M | 1.21e-2 | 3.33 | 0.67 |
| C4 (CASMO-8, a+b) | 0.201 | 1742 | 0.65M | 1.42e-2 | 3.90 | 0.06 |
| C5 (CASMO-8, wide) | 0.318 | 1799 | 0.55M | 5.51e-3 | 1.52 | 0.19 |
| C6 (CASMO-25, a+b) | 0.232 | 1747 | 0.95M | 1.06e-2 | 2.91 | 1.13 |

**Verdict and statistics:**

- **Verdict:** FAIL. The best gain is 6.3× (C2), against the 100× rule.
- **The gain is a lower bound:** the analog reached R = 0.196 in its 7200 s
  budget, above 0.1, so each gain is a labelled lower bound.
- **Unbiasedness:** NOT_EVALUATED for the same reason. All six windowed results
  lie within 1.13σ of the analog.
- **The gains are imprecise:** R itself rests on 15 (C1) to 95 batches.

**Response (b), the inboard midplane peak:**

- Every run's peak voxel has R between 0.7 and 1.0.
- So the peak is reported only as a location, with no pass rule, as the protocol
  says.
- Response (b) is not resolved at these run lengths with any configuration.

**Implementation notes recorded by the run:**

- **Group structures:** R6's generation used a custom 8-group structure. R1 used
  the declared CASMO-8 and CASMO-25 from `openmc.mgxs.GROUP_STRUCTURES`.
- **Adjoint energy range:** a multigroup adjoint response must follow group
  edges. It therefore covers E > 0.821 MeV for CASMO-8 and E > 0.111 MeV for
  CASMO-25. The 1-group case covers everything.
  - The production tally is exactly E > 0.1 MeV.
  - CASMO-25, which matches the response, did no better than CASMO-8.
- **Objective (a)+(b):** OpenMC 0.15.3 builds the adjoint source itself from
  the model's tallies, weighted by the inverse forward estimate, and does not
  expose user weights.
  - C4 and C6 therefore add the (b) mesh tally and leave the weighting to
    OpenMC.
  - The C4 windows differ from C2 only as much as two seeds of the same
    objective do (median ratio 0.994 against 1.013).
  - So the second objective made no detectable difference.
- **C5 settings:** upper bound = 10 × lower bound, `survival_ratio` = 5. Every
  other config used the generated windows (ratio 5, survival 3, `max_split` 10).
- **Seeds:**
  - generation 81150101–81150106;
  - MGXS 81150100;
  - production 20261101–20261106;
  - analog 20261100.
- **Cost:** generation took 2415–2788 s per config, at 0.54–0.60 GB with
  8 groups and 1.33 GB with 25 groups.

**Not varied, because outside the declared configurations:**

- the weight-window mesh resolution;
- the random-ray source-region resolution and ray count;
- the filler density;
- `max_split`.

These are candidates for a separate diagnostic protocol. They are not a retune
of this rule.

**Consequence:** NUC-050's provisional 100× target goes to change control with
these measurements, as the plan states (risk 1).

### R3: shutdown dose, D1S against R2S: FAIL

**Setup:**

- Mesh: 24 × 7 × 7 cubes of 10 cm over x 346–586 cm and y, z ±35 cm. This
  covers the port duct (mouth at x = 446 cm, end at x = 580 cm), 100 cm of
  chamber in front of the mouth, and a 20 cm margin.
- Irradiation: 1 FPY at 525 MW in one constant-power step.
- Dose: ICRP-116 AP coefficients with log-log interpolation.
- Chain: both methods use the same chain, p32.
- Source domain:
  - Both methods count only decay photons born inside the scored mesh.
  - D1S's dose tally carries a one-bin `MeshBornFilter`.
  - Mesh R2S activates only material in the mesh, so this is like for like.
  - The run made this choice before any dose was computed. The coordinator
    accepted it on review, after the results were reported.

**Results** (dose rate summed over the mesh, pSv·cm/s per unit source):

| Cooling | D1S | R2S | D1S/R2S | In 0.85–1.15 |
| --- | --- | --- | --- | --- |
| 1 d | 8.32e16 (R 0.14) | 1.306e17 (R 0.0011) | 0.637 ± 0.088 | no |
| 7 d | 1.94e16 (R 0.47) | 7.53e16 (R 0.0012) | 0.258 ± 0.122 | no |
| 30 d | 1.73e16 (R 0.50) | 6.96e16 (R 0.0012) | 0.248 ± 0.125 | no |

**Verdict and statistics:**

- **Verdict:** FAIL as the rule is written.
- **D1S statistics are weak:** the D1S run completed only 70,000 coupled
  histories in its 7000 s budget, about 12 per second.
- **What is inconclusive:** the 7 d and 30 d ratios are not conclusive, because
  their D1S relative errors are 0.47–0.50.
- **What is real:** the 1 d ratio is about 4σ below 1, so that discrepancy is
  real at this precision.
- **Voxel-ratio check:** NOT_EVALUATED. No voxel reached D1S R ≤ 10 %.
- **Next step:** a D1S run long enough, or variance-reduced, to reach about 5 %
  on the total, under a separate protocol.

**Dominant nuclides:**

- The two methods rank on different bases (R2S by decay-photon power, D1S by
  dose by parent nuclide), so the lists are indicative only.
- At 1 d:
  - R2S: Mn-54 42 %, W-187 32 %, W-181 20 %, then Sc-48, Ta-182.
  - D1S: W-187 75 %, Mn-54 21 %, then Mn-56, Fe-59, W-181.
- At 7 d and 30 d, Mn-54 dominates both methods (R2S 65–68 %, D1S 90–96 %).

**Conservation (ACT-023): FAIL at the default setting.**

- The R2S decay-photon source falls short of the inventory's photon emission by
  5.0–5.5e-5 relative. The rule asks for 1e-6.
- The cause is the default `clip_tolerance = 1e-6` in
  `Material.get_decay_photon_energy`.
- With `clip_tolerance = 0` the source matches to 1e-14.
- The inventory emission was computed independently from the chain's photon
  data.
- **Consequence:** FARIS must set `clip_tolerance = 0`, or state the 5e-5
  shortfall.

**Cost:**

- R2S:
  - neutron step 1594 s (8e5 histories, 1026 regions, VITAMIN-J-42);
  - activation 187 s;
  - three photon runs of about 3000 s each;
  - peak 1.49 GB.
- D1S: one 7000 s coupled run, peak 0.49 GB.
- R2S needed one extra workaround: its sub-models do not inherit the library
  path, so `openmc.config['cross_sections']` is set.

**Consequence:**

- The plan made D1S the fast method and R2S the check.
- On this model with 0.15.3, D1S was neither fast nor converged.
- VAL-043 and the S3 dose design go to change control with these measurements.
- The 13× analog speed difference between 0.15.3 and 0.16.0 noted under R6 is
  still unexplained. It affects every run time here, but no ratio or gain.

### R2: laptop preview: FAIL

**Setup:**

- RM-M with the C2 windows, OpenMC 0.15.3, 8 threads, run alone.
- 1000 histories per batch, with a statepoint every batch.
- `openmc` was stopped by process group 60.0 s after its process started, so
  cross-section and window loading count.
- Both runs used the same seed.

| | Run 1 | Run 2 |
| --- | --- | --- |
| Histories at the last statepoint | 16,000 | 18,000 |
| Startup (to first batch) | 20.5 s | 20.1 s |
| Last statepoint | 57.3 s | 58.2 s |
| TBR (tritons per source neutron) | 1.234, R 2.02 % | 1.230, R 1.87 % |
| Response (a), cm per source neutron | 6.2e-4, R 56.6 % | 5.6e-4, R 56.0 % |
| Peak memory | 512 MB | 511 MB |

**Verdict:**

- FAIL in both runs.
- Response (a) reaches R ≈ 0.56 against the 0.20 rule.
- TBR sits at the 2 % rule: it fails run 1 and passes run 2.
- About a third of the 60 s is startup.

**One-time costs, excluded from the 60 s:**

- CAD conversion: 1390 s, peak 4.83 GB.
- MGXS: 70 s.
- Random-ray FW-CADIS solve: 2341 s. The whole generation job took 2415 s at a
  peak of 604 MB, measured beside another 4-thread job, so these times are
  pessimistic.

**Context, not a rule:** the TBR of 1.23 comes from this authored model, which
has no divertor, one port and 100 cm of outboard FLiBe. It is not compared
with ARC's published 1.08–1.1.

**Consequence:** PERF-025 goes to change control with these measurements. The
plan's fallbacks (a coarser preview tally, or a stated longer preview target)
are the options. A preview can rank designs by TBR in 60 s on this laptop. It
cannot rank them by magnet fluence.

### S0 summary

| Test | Verdict | Measured |
| --- | --- | --- |
| Acceptance | PASS | volumes within 0.015 %; 0 lost in 1e6; 100 % of source sites in plasma |
| R6 | 0.15.3 | 0.16.0 needed an extra workaround and its random-ray solve failed |
| R1 | FAIL | best FW-CADIS gain 6.3× (lower bound) against 100× |
| R2 | FAIL | TBR R ≈ 2 %; magnet R ≈ 56 % at 60 s against 20 % |
| R3 | FAIL | D1S/R2S 0.64 ± 0.09 at 1 d; 7 d and 30 d inconclusive; conservation needs `clip_tolerance = 0` |

Summary and evidence hashes:
[`references/cad-transport-risk-tests.json`](../../references/cad-transport-risk-tests.json).

## R2′: laptop preview with aimed windows (protocol frozen 2026-10-13, before any R2′ run)

**Why.** R2 failed on response (a): 56 % at 60 s against 20 %. S0f changed how (a) is computed
([CAD_TRANSPORT_S0F.md](CAD_TRANSPORT_S0F.md), protocol 5). R2 is repeated with that method.

**Runs.** Script `s0g_preview.py`; outputs in `~/.cache/avila-night/cad-risk-tests/r2p/`.

- **Aimed:** `base-model-f9.xml` (sha256 62464ca5…) on the patched OpenMC 0.15.3 (`libopenmc.so` 2d25e1cd…) with
  protocol 5's F9 variables. Two runs, seeds 77600001 and 77600002.
- **Stock fall-back, reported and not judged:** `base-model.xml` (c0c107f1…) on stock OpenMC 0.15.3 with no patch
  variables, seed 77600001. This is what a user without the patch gets.
- As R2: the `openmc` process is stopped by process group 60 s after it starts, so reading the model, cross
  sections and windows counts. The last complete statepoint inside the 60 s gives both responses.
- Departures from R2, fixed here:
  - 4 threads on cores 0 to 3 under a 6 GB cap, not 8 threads alone. The machine is shared; this can only make
    the result worse.
  - Batches of 20,000 histories with a statepoint each, not 1,000. R2's batch size made the statepoints the
    main cost.
  - Each run starts when the 1-minute load is below 3.0 (wait up to 10 min).

**Rule (R2's, unchanged).** PASS if both aimed runs have TBR relative error ≤ 2 % and response (a) ≤ 20 % (1σ) at
60 s. Otherwise FAIL, and the errors go to change control for PERF-025.

**Not covered.** The one-time costs R2 excluded stay excluded and are unchanged, plus about 5 min to find targets
and cap windows. A 60 s single run cannot show an unresolved tail; protocol 5 covers that for 3 M histories.

### R2′ results (2026-10-13; protocol 9b79768): FAIL

| | Aimed, seed 1 | Aimed, seed 2 | Stock fall-back |
| --- | --- | --- | --- |
| Histories at the last statepoint | 1,700,000 | 1,720,000 | 1,980,000 |
| Last statepoint | 59.6 s | 59.4 s | 59.9 s |
| First statepoint after start | 4.6 s | 4.6 s | 4.6 s |
| "TBR" tally | 0.415, R 5.5 % | 0.398, R 5.4 % | 0.452, R 5.0 % |
| Response (a), cm per source neutron | 1.114e-3, R 3.3 % | 1.051e-3, R 3.1 % | 1.282e-3, R 7.4 % |

- **Verdict: FAIL as the rule is written.** The TBR error is 5.4–5.5 % against 2 %.
- **Response (a) passes with a wide margin:** 3.1–3.3 % against 20 %. R2 had 56 %. The stock fall-back gives 7.4 %
  in this one run; protocol 5 showed that its single runs can carry unresolved spikes.
- **The TBR failure is a fault in this protocol, and mine.** The S0f models carry a 0.1 MeV neutron cut-off and
  windows aimed at the magnet. Most tritium is bred below 0.1 MeV, so the tally here is not the TBR: it reads
  0.40–0.45 where R2 measured 1.23. I froze the protocol without checking that the model could answer for TBR.
- **What it shows.** One run cannot serve both responses. A preview needs a TBR run without the cut-off and a
  magnet run with it.

## R2″: laptop preview as two runs side by side (protocol frozen 2026-10-13, before any R2″ run)

**Runs.** `s0g_preview.py`, both processes started together and each stopped 60 s after its own start. Outputs in
`~/.cache/avila-night/cad-risk-tests/r2p/`.

- **TBR run:** `tbr-model.xml` (sha256 c9228b93…): `base-model.xml` with windows off and the cut-off removed. Stock
  OpenMC 0.15.3, 2 threads on cores 0 and 1.
- **Magnet run:** as R2′'s aimed run, 2 threads on cores 2 and 3.
- Two repetitions, seeds 77700001 and 77700002 (the same seed for both runs of a repetition). 6 GB cap each.
  Each repetition starts when the 1-minute load is below 3.0 (wait up to 10 min).
- R2 allowed 8 threads alone. This uses 4 in total, which can only make the result worse.

**Rule (R2's levels).** PASS if, in both repetitions, the TBR run's TBR relative error is ≤ 2 % and the magnet
run's response (a) is ≤ 20 % (1σ) at 60 s. Otherwise FAIL, and the errors go to change control for PERF-025.

**Reported, not judged.** The TBR value against R2's 1.23, as a check that this tally is the TBR.

### R2″ results (2026-10-13; protocol dc9bb84): PASS

| | Repetition 1 | Repetition 2 |
| --- | --- | --- |
| TBR run: histories at the last statepoint | 460,000 | 440,000 |
| TBR run: TBR | 1.227, R 0.16 % | 1.228, R 0.15 % |
| Magnet run: histories at the last statepoint | 780,000 | 760,000 |
| Magnet run: response (a), cm per source neutron | 1.074e-3, R 3.8 % | 1.029e-3, R 3.1 % |
| Last statepoints | 59.2 s and 59.4 s | 58.5 s and 58.7 s |
| 1-minute load at start | 1.65 | 2.64 |

- **Verdict: PASS.** Both repetitions meet both limits: TBR ≤ 2 % and response (a) ≤ 20 % at 60 s.
- The TBR of 1.227–1.228 agrees with R2's 1.23, so this tally is the TBR.
- `s0g_preview.py` prints its own single-run verdict, which reads FAIL for each process because each run answers
  for one response only. The verdict above applies R2″'s rule: TBR from the TBR run, (a) from the magnet run.
- Response (a) agrees with D2's 1.071e-3 ± 0.069e-3 in both repetitions.
- **Limits.**
  - Research scripts on RM-M; the app does not do this yet, and RM-S is not tested here.
  - The magnet run needs the patched OpenMC. On stock OpenMC R2′'s single fall-back run gave 7.4 % at 60 s with
    4 threads, also inside 20 %, but its single runs can hide spikes (protocol 5: VOV 0.21 pooled).
  - One-time costs stay outside the 60 s: CAD conversion, cross sections, the random-ray solve, and about 5 min
    for targets and capped windows.

## R3′: D1S repeated on the faster build (protocol frozen 2026-10-13, before any R3′ run)

**Why.** R3's D1S run did 70,000 histories in 7,000 s, about 12 per second, and its errors were 14–50 %. A timing
check (2026-10-13, `~/.cache/avila-night/s0h-d1s/`, 1,000 histories each on 4 threads) ran R3's own D1S model
file (sha256 12630130…) on the DAGMC build in `openmc0153dd`, also OpenMC 0.15.3:

| Variant | Histories per second |
| --- | --- |
| Neutrons only | 16,900 |
| With prompt photons | 3,400 |
| D1S (decay photons), as R3 | 1,075 |

So the same model runs about 90× faster on this build than R3's run did in `w003env`. Why that build is slow is
not traced. R3's D1S failure may be mostly an unconverged run.

**Run.** `r3_d1s.py` unchanged, with the `openmc0153dd` interpreter and executable, model directory
`~/.cache/avila-night/cad-risk-tests/models/rm-m`, seed 20261302, 4 threads on cores 0 to 3 under a 6 GB cap,
budget 1,500 s. Output `~/.cache/avila-night/cad-risk-tests/r3p/d1s`.

**Comparison.** `r3_compare.py` against R3's R2S results, unchanged (`.../r3/r2s`). R2S is not rerun: its errors
were 0.1 %.

**Rule (R3's, unchanged).** PASS if D1S/R2S of the total dose over the mesh is within 0.85–1.15 at 1 d, 7 d and
30 d. A cooling time whose D1S relative error is above 10 % is INCONCLUSIVE, not a pass or a fail. Otherwise
FAIL, with the dominant nuclides reported.

**Expected, written before the run.** About 1.5 M histories, giving D1S errors near 3 % at 1 d and 11 % at 7 d and
30 d if R3's errors scale. The two builds are assumed to give the same physics; the 1 d value against R3's
8.32e16 ± 14 % is the check.

**Amendment, 2026-10-13, before any R3′ transport.** The first attempt stopped while building the model: the
`openmc0153dd` interpreter has no `openmc_plasma_source`, and that environment is not to be changed. No history
was run. R3′ therefore uses `r3p_d1s_rerun.py`, which takes R3's D1S model file (sha256 12630130…) and its list
of radionuclides as they are, changes only the seed, and applies the same time correction as `r3_d1s.py`.
Everything else in the protocol stands.

### R3′ results (2026-10-13; protocol 9f5b229 with amendment 7fb0d43; `references/cad-transport-r3p.json`): FAIL, narrowly, at 1 d

The run did 1,530,000 histories in its 1,500 s (1,020 per second), against R3's 70,000 in 7,000 s.

| Cooling | D1S (this run) | R2S (from R3) | D1S/R2S | In 0.85–1.15 | R3's ratio |
| --- | --- | --- | --- | --- | --- |
| 1 d | 1.562e17 (R 0.037) | 1.306e17 (R 0.0011) | 1.196 ± 0.044 | no | 0.637 ± 0.088 |
| 7 d | 8.53e16 (R 0.055) | 7.53e16 (R 0.0012) | 1.132 ± 0.062 | yes | 0.258 ± 0.122 |
| 30 d | 7.92e16 (R 0.056) | 6.96e16 (R 0.0012) | 1.138 ± 0.063 | yes | 0.248 ± 0.125 |

- **Verdict: FAIL as the rule is written.** The 1 d ratio is outside the band and its D1S error is below 10 %.
  It is 1.0σ above the band's upper edge, so the data do not separate 1.20 from 1.15.
- **R3's ratios were wrong, not just imprecise.** They were 0.64, 0.26 and 0.25; they are 1.20, 1.13 and 1.14.
  - R3's 1 d dose (8.32e16, quoted R 0.14) is 6σ of its own quoted error below this run's. Its error estimates
    were far too small: 70,000 histories did not sample the photons that carry the dose.
  - The check written before this run, that the 1 d value would agree with R3's, failed for that reason.
- **What remains is a steady offset.** D1S is 13–20 % above R2S at all three cooling times.
  - The two methods now agree on the leading nuclides at 1 d: Mn-54 and W-187 (D1S 52 % and 41 % of dose; R2S
    42 % and 32 % of photon power, with W-181's low-energy photons at 20 %).
  - At 30 d W-187 has decayed and Mn-54 dominates, and the offset is still 14 %. So it is not specific to
    tungsten capture.
  - **Hypothesis, not tested:** the offset sits on the R2S side. Mesh R2S activates with a 42-group flux averaged
    over 10 cm voxels that mix steel, tungsten, FLiBe and void. Mn-54 comes from a threshold reaction on iron,
    which is sensitive to both. D1S uses continuous energy in the real geometry.
- **Voxel check: NOT_EVALUATED.** No useful voxel reached D1S R ≤ 10 %. `r3_compare.py` counted one voxel at 7 d
  with a ratio of 1.8e-290: a voxel with a denormal D1S value passes the error test. That is a fault in the
  comparison's voxel filter and does not touch the totals.
- **Cost.** 25 min on 4 threads for 3.7 % at 1 d and 5.5 % at 7 d and 30 d, with no variance reduction.
- **Not traced.** Why the `w003env` build runs this model about 90× slower.
- **Next.** Test the hypothesis: R2S on the same mesh with a finer group structure and with 5 cm voxels, on the
  faster build. R3's R2S took about 3 h on the slow build; its cost on the faster one is not yet measured.

## R3″: where the D1S/R2S offset comes from (diagnostic; protocol frozen 2026-10-13, before the two runs below)

**Already checked, from stored files and one timing smoke (no rule; recorded here before the runs):**

- **Decay bookkeeping: ruled out.** (Agent check, scripts in the session scratchpad, `agent-decay/`.) D1S's time
  correction factors equal the analytic single-step result to six digits for Mn-54, W-187, W-181 and six others.
  R2S's depletion inventory matches the same formula from its own reaction rates to 0.1 % (Mn-54) and 1.1 %
  (W-187, from target burn-up). The nuclide lists differ by 0.04 % of emission. Together at most about 1 % of the
  offset.
- **R2S neutron statistics: not the cause.** R3's reaction-rate tallies have 1–3 % error in total and 14–30 % per
  region. The smoke run of `r3p_r2s_rerun.py` used a tenth of R3's neutron histories (80,000, new seed) and gave
  1.309e17 at 1 d against R3's 1.306e17. The total does not move with neutron noise.
- **Open lead, opposite in sign.** W-181 carries 26–36 % of R2S's dose-weighted emission but 1.6–2.8 % of D1S's
  dose. Its photons are below 100 keV and should be absorbed in the tungsten, so this may be nothing; it cannot
  make D1S high.
- **Running separately:** a check of R2S's material volumes per voxel (50,000 sample points).

**Runs.** `r3p_r2s_rerun.py` on the `openmc0153dd` build, from R3's stored neutron model, 800,000 neutron
histories as R3, seeds 20261221 and 20261231, photon budget 120 s per cooling time, 2 threads each, side by side,
6 GB cap each. Outputs in `~/.cache/avila-night/cad-risk-tests/r3p/`.

- **Groups:** activation in VITAMIN-J-175 instead of VITAMIN-J-42; 10 cm voxels.
- **Voxels:** 5 cm voxels over the same box (48 × 14 × 14); VITAMIN-J-42.

**Reading rule, on the change in R2S's total dose at 30 d against R3's 6.96e16** (30 d is Mn-54 alone, offset 14 %):

- a rise of 7 % or more: that choice explains at least half the offset;
- a rise of 3–7 %: a partial cause;
- within ±3 %: not a cause;
- a fall of more than 3 %: the offset is larger than measured, and the hypothesis is wrong in sign.

The 1 d and 7 d changes are reported beside it. A run that fails or exceeds its memory cap is NOT_EVALUATED.

### R3″ results (2026-10-13; protocol 55f137e)

**The material-volume check, which was running when the protocol was frozen.** (Agent check, scratchpad
`agent-volumes/`.) `n_samples` is a ray count: 50,000 gives rays about 0.9 cm apart, with exact chords. Against a
0.25 cm point classification of the specified geometry, R2S holds 2.9 % too little steel and 2.3 % too much
tungsten in the box. That bounds the effect at about 3 % at 7 d and 30 d, and it has the other sign at 1 d. The
independent volumes come from the specification, not from the faceted CAD file.

**The two variants** (change in R2S's total dose against R3's):

| Variant | 1 d | 7 d | 30 d | Activation regions | Total time |
| --- | --- | --- | --- | --- | --- |
| VITAMIN-J-175, 10 cm | −2.8 % | −3.4 % | −3.8 % | 1,026 | 15 min |
| VITAMIN-J-42, 5 cm | +3.5 % | −3.2 % | −3.6 % | 6,134 | 40 min |

- **By the frozen rule, both read "wrong in sign":** the 30 d dose fell by more than 3 % in each. Neither finer
  groups nor finer voxels brings R2S toward D1S.
- **The rule's ±3 % band was set without knowing the run-to-run scatter, which is a fault in the protocol.** Both
  variants fell by the same 3–4 % at 7 d and 30 d, on new seeds and a different build from R3. That looks like
  scatter or a build difference in the baseline, not an effect of either choice. No baseline was rerun, so this
  is not settled.
- **Voxel size does move the 1 d dose** (+3.5 %), where tungsten capture matters, but by a quarter of the offset.

**Production check, added after the runs (diagnostic, no rule).** A direct continuous-energy tally of the
production rates in the box (2 M histories, `~/.cache/avila-night/s0h-d1s/prod/`) against what each R2S run fed
to its activation step:

| Run | Fe-54(n,p) → Mn-54 | W-186(n,γ) → W-187 | W-182(n,2n) → W-181 |
| --- | --- | --- | --- |
| Direct tally, atoms/s | 3.26e15 ± 1.8 % | 5.04e15 ± 2.8 % | 2.08e16 ± 1.4 % |
| R3's R2S / direct | 1.065 | 0.902 | 1.015 |
| 175 groups / direct | 1.051 | 0.898 | 1.024 |
| 5 cm / direct | 1.031 | 0.965 | 1.009 |

- **R2S does not under-produce Mn-54.** It is 3–6 % above the direct tally, while its 30 d dose, which is Mn-54,
  is 14 % below D1S. So the offset at 7 d and 30 d is not in R2S's activation. It is in how many decay photons
  D1S emits per reaction, or in how either method carries photons to the dose.
- R2S's tungsten capture is 10 % low on 10 cm voxels and 3.5 % low on 5 cm. That goes the same way as the 1 d
  voxel result and is the only part of the offset traced so far: a few percent at 1 d.
- **A probe that did not work.** A D1S run scoring photon heating over all space for photons born in the box, by
  parent nuclide, was meant to give D1S's emitted energy per reaction. Its values are about 2,800 times below
  the expected 14.6 eV per source neutron for Mn-54, with 14 % error at 600,000 histories. I cannot yet say
  what that tally measures under D1S, so it supports no conclusion.

**State of the question.**

| Candidate | Result |
| --- | --- |
| Unconverged D1S (R3) | Was the cause of R3's 0.25–0.64. Fixed. |
| Decay bookkeeping | About 1 %. |
| R2S neutron statistics | None seen. |
| R2S material volumes | About 3 % at most, at 7 d and 30 d. |
| R2S group structure | None. |
| R2S voxel size | A few percent at 1 d; none at 30 d. |
| R2S activation of Mn-54 | Not low: 3–6 % high. |
| D1S photon emission per reaction; photon transport in either method | Open. |

## R3 cube check (diagnostic; frozen 2026-10-13 before its run; the last round on the offset for now)

**Question.** Is D1S's photon emission per reaction right? R3″ left the offset in D1S's emission or in photon
transport.

**Case.** `r3_cube_check.py`: a 2 cm cube of iron at 1 g/cm³ in vacuum, a 14.1 MeV beam on one face, dose in a
6 cm box around it at 30 d after R3's irradiation. The cube is thin for neutrons and photons. Three values of the
Mn-54 dose: D1S; mesh R2S; and a manual one from a direct Fe-54(n,p) tally, the Bateman factor, the chain's
photons per decay and a photon-only run for dose per photon. 30 M neutron histories; a 3 M smoke run first to see
that the script works, whose numbers are not used.

**Reading rule.**
- D1S and R2S both within 3 % of manual (or 2σ of the combined error, if larger): both methods are right on a
  simple case, and the offset on RM-M is an effect of its geometry. It is then reported as measured.
- D1S more than that above manual: D1S over-emits; the size is compared with RM-M's 14 %.
- R2S more than that below manual: R2S under-emits or under-transports; likewise.

**Amendment, 2026-10-13, after the smoke run.** The smoke showed the case as first written is unusable for R2S:
R3's source rate through a 4 cm² beam transmutes every Fe-54 atom in the cube within hours (per-atom rate
1.6e-5 /s). R2S's depletion follows that burn-up and D1S, which is linear in rate, cannot, so R2S came out 770
times lower. The source rate is changed to 1e12 n/s for all three values. Nothing else changes.

A 30 M run was then started by mistake on the unamended script: my edit had failed and I did not check it. Its
D1S and manual values are valid, since both are linear in rate (D1S/manual 0.9970 ± 0.0025), and its R2S value is
the burnt-out case again. The run below, on the amended script, is the one the reading rule applies to.

### R3 cube check result (2026-10-13; amended protocol a931574; `references/cad-transport-r3-cube.json`)

| Value of the Mn-54 dose at 30 d | Ratio to manual | Statistical error |
| --- | --- | --- |
| D1S | 0.9970 | 0.25 % |
| Mesh R2S | 1.0023 | 0.04 % |

- **Both methods are right on a simple case.** D1S and R2S agree with the hand calculation and with each other
  (D1S/R2S 0.995) well inside 3 %. D1S's photon emission per reaction is correct, and so is R2S's chain from
  reaction rate to photon source.
- **By the frozen rule, the offset on RM-M is an effect of its geometry and is reported as measured:** D1S/R2S
  1.20 ± 0.04 at 1 d, 1.13 ± 0.06 at 7 d and 1.14 ± 0.06 at 30 d.
- **What "geometry" can still mean, none of it tested:**
  - activation gradients across 1–3 cm layers, which 5 cm voxels do not resolve, with R2S starting photons
    evenly through a layer and D1S at the reaction sites facing the duct;
  - secondary photons made inside the box by decay photons born outside it, which D1S's born-in-box filter
    counts and R2S's source does not contain.
- **A limit of D1S found on the way.** At R3's source rate the cube's Fe-54 burns out within hours; R2S follows
  that and D1S cannot. On RM-M the burn-up is small (0.2 % for Fe-54, 1.2 % for W-186 over the year), so it is
  not this offset, but a strongly irradiated thin part would need a check.
- **This closes the diagnosis for now.** Of the two methods, D1S is the one that uses the real geometry and
  continuous energy; nothing found here says its value is the wrong one.

