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
