# CAD transport diagnostics (stage S0b)

Status: protocol frozen 2026-10-10, before any S0b run. Results are appended in a
section below and never edit this protocol. Any change to a rule after the first
run is an amendment with a date and reason. Parent:
[CAD_TRANSPORT_RISK_TESTS.md](CAD_TRANSPORT_RISK_TESTS.md) (S0). Plan:
[NEXT_RELEASE_PLAN.md](../NEXT_RELEASE_PLAN.md), risks 1–3.

S0 failed R1, R2 and R3. Those verdicts stand and are not re-scored here. S0b
answers a different question: **what is reachable**, and why S0 fell short, so
that change control for NUC-050, PERF-025, VAL-043 and the S3 dose design rests
on measured numbers. The project standard is best in class on every metric, so a
target is lowered only when S0b shows it is out of reach on the reference laptop,
never because S0's first configuration missed it.

Every configuration run is reported, including those that did worse.

## Facts established before this protocol

These were read from S0 outputs that already existed. No S0b run had started.

1. **The 13× version speed gap is a build difference, not a version
   difference.** The 0.16.0 analog log says "Using the DOUBLE-DOWN interface to
   Embree"; the 0.15.3 log does not. The conda-forge 0.16.0 build links DAGMC
   built with double-down (Embree ray tracing). The 0.15.3 install in
   `~/.venvs/w003env` links DAGMC with MOAB's own ray tracer. Transport time was
   1939 s on 0.15.3 and 92 s on 0.16.0, with the same model XML, library, seed
   and histories.
2. **A 0.15.3 build with double-down exists.** The micromamba env
   `openmc0153dd` holds conda-forge `openmc 0.15.3 dagmc_nompi_py313h6424856_102`
   (commit 27e38e89, the same commit as `w003env`) with `dagmc 3.2.4
   nompi_doubledown`, `double-down 1.1.0`, `embree 4.4.1` and `moab 5.6.0`.
   R6's version choice (0.15.3) therefore stands; only the ray tracer changes.
3. **S0's R1 windows were coarse.** The windows and the random-ray source
   regions used one 25 cm regular mesh (52 × 52 × 31) with flat sources,
   20,000 rays, 500 cm inactive and 2,500 cm active length, 30 active batches,
   `max_split` 10 and OpenMC's default checkpoints. The inboard build from the
   plasma to the TF leg is about 90 cm, so it spans about four window cells.
4. **S0's R2S used direct continuous-energy reaction rates**
   (`get_microxs_and_flux`, `reaction_rate_mode = 'direct'`), so its activation
   carries correct self-shielding. Its quoted R (about 0.001) is photon-step
   statistics only. The neutron-step uncertainty was not propagated.
5. **The D1S/R2S discrepancy has a nuclide signature.** From S0's dominant-nuclide
   lists, D1S's Mn-54 dose is about 0.3 of R2S's at every cooling time, while
   D1S ranks W-187 higher than R2S. A uniform normalisation error would scale
   every nuclide equally. **Hypothesis H1, stated before testing:** OpenMC's D1S
   emits decay photons only for the summed reaction MT that the chain names
   (for example MT 103 for (n,p)), and misses events sampled on partial MTs
   (MT 600–649 for (n,p), and similar for (n,α), (n,d) etc.), which is how
   FENDL-3.2 stores Fe-54 (n,p). H1 is tested in D5a and may be false.

## Machine and execution rules

- Reference laptop (Intel Core i3-N305, 8 E-cores in two 4-core clusters,
  30 GB).
- Every OpenMC or Python job runs under `systemd-run --user --scope -p
  MemoryMax=6G -p MemorySwapMax=0`.
- **FOM runs** (D2, D3): 4 threads, pinned with `taskset -c 0-3`, at most one
  other job on the machine, and that job pinned to cores 4–7 or not CPU-bound.
  The 1-minute load average is recorded at the start and end of each run.
- **Timing runs** (D1 speed, D4): run alone. The marker file
  `~/.cache/avila-night/cad-risk-tests/S0B_EXCLUSIVE` is present for the
  duration, and other work in this repository waits on it.
- Seeds are new, disjoint from S0's and from each other, and recorded.
- Model: RM-M as accepted in S0 (`rm_m.h5m` sha256 24fda318…, random-ray model
  `rm_m_rr.h5m`). Library: the FARIS audited library, as in S0.
- Scripts in `integrations/openmc/risk_tests/` with unit tests; scratch in
  `~/.cache/avila-night/cad-risk-tests/s0b/`.
- Time box: S0b's machine time is capped at 16 hours of wall time in total. If
  the cap is reached, unfinished items are reported as NOT_EVALUATED with why
  and the next step.

## D1: ray tracer

**Run:** the S0 R6 analog model (1e6 histories, 4 threads, a new seed) on
`openmc0153dd`, alone. Then the same on `w003env` with the same seed, also
alone, so both rates come from the same conditions.

**Report:** particles per second (active), initialization time, response (a),
TBR, lost particles, peak memory.

**Rule (adopt double-down for all FARIS CAD transport):** all of

- speed-up ≥ 3× in active particles per second;
- lost particles ≤ 1e-6 per history (GEO-025), over the 1e6 histories;
- response (a) and TBR each agree with the `w003env` run within 3σ (combined).

Otherwise FARIS stays on `w003env` and the reason is recorded. D2–D5 use
whichever build D1 selects.

## D2: analog reference

**Run:** analog RM-M, 4 threads, until response (a) reaches R ≤ 0.05 or 7200 s,
whichever first, with a statepoint at least every 300 s.

**Report:** R(a), wall time T, histories, FOM(a) = 1/(R²T), TBR and its R.

This is the reference for every gain in D3. If R(a) > 0.1 at the budget, every
gain is a labelled lower bound and unbiasedness is NOT_EVALUATED, as in S0.

## D3: weight-window gain, what limits it

Each configuration generates windows (random-ray FW-CADIS, adjoint on response
(a), as in S0) and then runs production for **1200 s** at 4 threads. The
production tally and response definitions are S0's.

| Config | Change from G0 | Regenerate windows |
| --- | --- | --- |
| G0 | S0's C2 settings, on the D1 build | yes |
| G1 | window and source-region mesh 10 cm; rays × 15.6 (keeps track length per cell) | yes |
| G2 | linear source shape (`source_shape = 'linear'`), 25 cm | yes |
| G3 | the windows' `max_split` 1000; `settings.max_history_splits` left at its default and recorded (G0 windows) | no |
| G4 | weight-window checkpoints at surface crossings and collisions (G0 windows) | no |
| G5 | best of G1/G2 mesh and source shape, plus whichever of G3 and G4 improved FOM | yes, if G1 or G2 changed |
| G6 | G5 with the group structure CASMO-8 plus an edge at 0.1 MeV (9 groups), so the adjoint source matches response (a)'s 0.1 MeV cut | yes |

"Improved" in G5 means FOM higher than G0's by more than the combined 1σ of the
two FOMs. FOM uncertainty is estimated from R's batch statistics (relative
uncertainty of R² is taken as √(2/(n−1)) for n batches).

**Report for each:** generation time and peak memory, random-ray source regions
and the fraction with a positive adjoint flux, window range, production R(a),
FOM, gain against D2, z against D2, histories, and histories per second (the
cost of splitting shows here). Split counts are reported where OpenMC 0.15.3
exposes them, and otherwise marked NOT_EVALUATED.

**Decision rules:**

- **Unbiasedness:** every configuration's (a) must agree with D2 within 3σ. A
  configuration that fails is reported and excluded from "best".
- **Best gain G\*** is the largest gain among unbiased configurations.
- **G\* ≥ 100×:** NUC-050 keeps 100×. A new R1 run (R1′) with G\*'s settings,
  fixed in advance, goes into the S2 protocol. S0's R1 verdict stays FAIL in
  the record.
- **10× ≤ G\* < 100×:** change control for NUC-050 receives G\*, the
  configuration, and the measured limiting factor (from the G0–G6
  differences, unpopulated source regions, or the window range). The target is lowered only
  if no remaining lever is identified; otherwise a follow-up protocol tests that
  lever.
- **G\* < 10×:** same, and FW-CADIS on DAGMC is flagged as a release risk to
  Connor.

## D4: laptop preview

**Run:** twice with the same seed, alone, 8 threads, on the D1 build, with G\*'s
windows. Stop 60 s after process start, as in S0's R2.

**Report:** TBR R and response (a) R at the last statepoint before 60 s,
startup time split into cross-section loading, geometry loading and window
loading, histories, peak memory.

**Rule (change control for PERF-025):** if both runs give TBR R ≤ 2 % and (a)
R ≤ 20 %, PERF-025 keeps its numbers. Otherwise the measured errors go to change
control with the plan's fallbacks.

## D5: shutdown dose

### D5a: reaction-pathway audit (tests H1)

Small fixed-source CSG problems with no CAD: a 5 cm radius sphere of each of
tungsten, Inconel 718, SS316L and TiH2 (FARIS baseline compositions) in vacuum,
with an isotropic point source at the centre. Two sources per material: 14.1 MeV,
and the S0 TF fast-flux spectrum tabulated from the R6 analog run. Continuous
energy, `openmc0153dd`, the audited library, the p32 chain. Scenario: 1 FPY at
unit source rate, then 1 d cooling.

For each material and source, and for every radionuclide that gives ≥ 1 % of
either method's photon leakage:

1. **D1S leakage:** photon current out of the sphere surface with a
   `ParentNuclideFilter`, times OpenMC's D1S time-correction factor.
2. **Reference leakage:** the nuclide's production rate from direct reaction-rate
   tallies on every reaction in the chain that produces it (OpenMC reaction
   scores by the chain's reaction names), turned into activity with the
   single-step Bateman solution, then a photon run with that nuclide's chain
   decay-photon spectrum as a uniform source in the sphere, scoring the same
   current.
3. **Partial-MT fraction:** the share of each producing reaction that the
   library stores on partial MTs (MT 600–649, 650–699, 700–749, 800–849),
   scored by those MTs.

**Rule:** a nuclide whose D1S/reference ratio differs from 1 by more than 3σ
and more than 2 % is a defect. For each defect, the cause is located in the
OpenMC source, checked on 0.16.0, and reported upstream (see D6). H1 is
confirmed if 1 − ratio equals the partial-MT fraction within 3σ.

### D5b: D1S cost

On RM-M with the D1 build, 4 threads, 300 s each, report particles per second
for:

1. neutron-only analog;
2. photon transport on, no decay photons;
3. D1S decay photons on, no tallies;
4. S0's full D1S tally (mesh × parent-nuclide filter × mesh-born filter).

No rule; the result says which part of D1S's 12 histories per second (S0) is
geometry, physics or tallying.

### D5c: R2S neutron-step uncertainty

Repeat S0's R2S neutron step and activation with two new seeds (same histories,
same mesh and groups) on the D1 build, then repeat the photon step at 1 d. Report
the spread of the total dose at 1 d over the three seeds (S0's and the two new
ones) against the photon-step R.

**Rule:** if the seed-to-seed relative standard deviation exceeds 3 × the
photon-step R, S0's R2S uncertainty is understated, and VAL-043 must propagate
the neutron-step uncertainty in S3.

### What D5 does not do

It does not re-run R3. A converged D1S against R2S comparison is set in a new
protocol after D5a, because if H1 holds, D1S on this library needs a fix or a
workaround first.

## D6: upstream defect reproductions

The project reports upstream defects it finds. Each candidate gets a minimal
reproducer (no FARIS code, smallest model that shows it), run on 0.15.3 and
0.16.0, and a check of open and closed OpenMC issues. Candidates from S0:

1. The stochastic-slab MGXS model does not inherit `model.materials.cross_sections`
   (0.16.0).
2. The 0.16.0 MGXS library gives a low-density material a negative total cross
   section in one group, which aborts random ray.
3. `R2SManager` sub-models do not inherit the library path.
4. D5a's defects, if any.

A candidate that reproduces and is not already reported gets a drafted issue in
`docs/defects/` (title, versions, reproducer, expected, observed). Nothing is
posted until Connor has seen the first batch.

Not candidates: `rel_max_lost_particles` refusing 1.0 and the default
`clip_tolerance` in `get_decay_photon_energy` are checked against the
documentation first; they are reported only if the behaviour contradicts it.

## Outputs

- A results section appended here.
- `references/cad-transport-diagnostics-s0b.json` with each run's record,
  hashes and verdicts.
- Change-control proposals for NUC-050, PERF-025 and VAL-043 written from the
  results, for Connor's approval.

## Results

Appended after runs.

### Results, 2026-10-10

Every run is reported. Records: `~/.cache/avila-night/cad-risk-tests/s0b/` (per-item directories, `results-s0b.json`,
`ledger.json`); hashes in `references/cad-transport-diagnostics-s0b.json`. Elapsed machine time 8.3 h of the 16 h cap.
Rule outcomes below are the mechanical results of the rules above (`risk_tests/s0b_common.py`, 114 tests); the
judgments are marked as such.

#### Execution notes and deviations

- **D4 was not run alone the first time.** The first pair overlapped another agent's CAD checks and tests (`d4_all.ps`),
  against the "run alone" condition. The coordinator re-ran the identical command alone (`d4b/`, `d4b_all.ps`, load 1.9 at
  start). Both pairs are reported; the rule is applied to the pair run alone.
- **D2's batch count** was first read from the configured maximum. It was recomputed from the statepoint (3050 batches)
  before any gain was formed and is marked `correction` in `d2/analog.json`.
- **D5a materials.** The FARIS audited library has no Ni, Cr, Mn, Nb or Mo, so Inconel 718 and SS316L could not be built.
  RM-M's iron surrogate (pure Fe) stands in for both. Tungsten, iron and TiH2 were run.
- **D5a supplementary reference.** Besides the protocol's reference (decay photons born uniformly in the sphere), a
  reference with the photons born along the radial production profile was run and labelled supplementary. See D5a.
- **Not evaluated:** G1 (out of memory at the 6 GB cap), G2's production (no batch finished), split counts (0.15.3 exposes
  none), and the D4 split between geometry loading and window loading (not separable in the 0.15.3 log).

#### D1: ray tracer

Seed 90010001, 1e6 histories, 4 threads, alone.

| | `openmc0153dd` (double-down) | `w003env` (MOAB) |
| --- | --- | --- |
| Active particles/s | 5879 | 177 |
| Wall time (s) | 178 | 5670 |
| Initialization (s) | 7.7 | 21.2 |
| Peak memory (MB) | 845 | 463 |
| Lost particles | 0 | 0 |
| (a), TBR | 1.5511e-3, 1.22492 | identical |

Speed-up 33.2×. The results agree to rounding (same seed). **Rule outcome: adopt double-down** for all FARIS CAD
transport. D2–D5 used `openmc0153dd`.

#### D2: analog reference

Seed 90020001, 4 threads on cores 0–3. The 7200 s budget ended the run first: at batch 3050 (30.5 M histories,
T = 7191 s), (a) = 1.0707e-3 with R = 0.0647, FOM(a) = 0.0332 ± 0.0008, TBR = 1.22430 (R 1.9e-4). R ≤ 0.1, so the gains
below are not lower bounds and unbiasedness is evaluated.

#### D3: weight-window gain

1200 s productions, 4 threads on cores 0–3, 10,000 histories per batch, no lost particles.

| Config | R(a) | Histories | Histories/s | FOM(a) | Gain vs D2 | z vs D2 |
| --- | --- | --- | --- | --- | --- | --- |
| G0 (S0's C2) | 0.0977 | 3.50 M | 2887 | 0.0864 ± 0.0065 | 2.61 ± 0.21 | +0.76 |
| G1 (10 cm mesh) | NOT_EVALUATED: the random-ray forward solve was killed at 5.4 GB under the 6 GB cap; scaling G0's 1.0 GB by 15.6× source regions gives about 15 GB. Next step: rerun with a 16 GB cap on an idle machine, or refine the mesh only where the shield is | | | | | |
| G2 (linear source) | NOT_EVALUATED: windows span 2.3e-20 to 0.092 with 14 % of cells unpopulated; no 10,000-history batch finished in 1200 s, nor a 100-history batch in a 240 s diagnostic | | | | | |
| G3 (`max_split` 1000) | 0.0809 | 3.55 M | 2965 | 0.128 ± 0.010 | 3.84 ± 0.31 | −0.17 |
| G4 (surface + collision checkpoints) | 0.0637 | 3.20 M | 2670 | 0.206 ± 0.016 | 6.20 ± 0.52 | −1.08 |
| G5 (G0 windows + G3 + G4) | 0.0573 | 4.65 M | 3875 | 0.254 ± 0.017 | 7.65 ± 0.54 | −0.16 |
| G6 (G5, 9 groups with a 0.1 MeV edge) | 0.0532 | 4.70 M | 3924 | 0.295 ± 0.019 | 8.87 ± 0.62 | −0.85 |

Window generation took 1299 s (G0), 1341 s (G2) and 742 s (G6), at about 1.0–1.3 GB. Every configuration with a result is
unbiased. **Rule outcome: G\* = G6, gain 8.87 ± 0.62, band "G\* < 10".**

Judgment (coordinator):

- **What the levers gave.** Checkpoints ×2.4 (G4/G0), `max_split` 1000 ×1.5 (G3/G0), both ×2.9 (G5), and the 0.1 MeV group
  edge a further ×1.16 (G6/G5). None of these changes the windows themselves.
- **Remaining lever: window resolution, untested.** The most likely limit is still what S0 found: one 25 cm cell
  spans a quarter of the 90 cm inboard build, so each window boundary steps by a large factor. G1 would have tested that
  and was not evaluated, for memory. G2 shows that window range matters too: windows reaching 1e-20 stalled the production.
- **Rule application.** Because a lever remains, the rule says NUC-050 is not lowered. A follow-up protocol tests window
  resolution (a 16 GB cap, a mesh refined only in the shield, or a cylindrical mesh following the radial build) together
  with bounded window range. The "G\* < 10" band also flags FW-CADIS on DAGMC to Connor as a release risk; that flag is
  raised in the change-control proposal.
- **The faster tracer lowered the gain, not the FOM.** S0's 6.3× for C2 came from runs on the MOAB build. On the
  double-down build the analog speeds up more than the windowed runs (4240 against 2887 histories/s), so the same windows
  now measure 2.61×. In absolute terms G6's FOM (0.295) is 13× S0's best C2 FOM (0.0228) and 81× S0's analog.

#### D4: laptop preview

G6 windows with `max_split` 1000 and checkpoints, 8 threads, seed 90060001, stopped 60 s after process start.

| Pair | Run | Batches (1000 histories) | TBR R | (a) R | Time to first batch (s) |
| --- | --- | --- | --- | --- | --- |
| Not alone (excluded) | 1 | 81 | 0.0064 | 0.232 | 33.2 |
| Not alone (excluded) | 2 | 127 | 0.0052 | 0.193 | 25.2 |
| Alone (rule applied) | 1 | 74 | 0.0068 | 0.249 | 41.7 |
| Alone (rule applied) | 2 | 69 | 0.0072 | 0.247 | 43.9 |

Peak memory 0.9 GB. **Rule outcome: PERF-025 does not keep its numbers.** TBR passes by a wide margin; (a) misses
0.20 in both runs alone.

Judgment: start-up takes 42–44 s of the 60 s, so transport gets about 17 s. Removing the start-up, for example with a
library session that stays loaded between previews, would give about 3.5× the histories and R(a) near 0.13. That lever
goes to change control.

#### D5a: reaction-pathway audit

Per protocol rule, with the uniform-source reference: **every scored nuclide meets the defect rule**, with ratios from
0.24 (W-181) to 0.95 (Sc-46, Sc-48). **H1 is confirmed for no nuclide.** Nuclides with no partial-MT production (W-181,
W-187, the Sc isotopes) deviate as much as those with only partial-MT production (Ta-182, Mn-54), so the deviation does
not track the partial-MT fraction. Reading the source settles it: the redundant MT 103 still adds to photon production
(`nuclide.cpp` around lines 399–404).

Judgment (coordinator): the protocol's reference was designed wrongly. With a point source at the centre, activation
peaks at the centre, so decay photons born uniformly over the sphere escape more easily than the real ones. The
supplementary reference, with photons born along the radial production profile, agrees with D1S within 2 % for every
nuclide (from 0.957 ± 0.075 to 1.15 ± 0.10; the tightest are Mn-54 1.00 ± 0.003, Mn-56 0.981 ± 0.001 and Sc-48
0.993 ± 0.001). D1S on 0.16.0 agrees with 0.15.3 (all |z| ≤ 1.42). **No D1S defect was found and H1 is rejected.**

That changes the question for R3. A uniform source over a volume overestimated leakage by 1.25–4× here. Mesh R2S spreads
each voxel's photon source over the voxel, so on 10 cm voxels with steep production gradients it would read high. That
would explain S0's D1S/R2S of 0.64. It is hypothesis H2 for the next shutdown-dose protocol, to be tested with cell-based
or finer-mesh R2S against D1S on RM-M.

#### D5b: D1S cost on RM-M

300 s each, 4 threads, cores 0–3.

| Mode | Particles/s | Histories |
| --- | --- | --- |
| Neutron-only analog | 6098 | 1.66 M |
| Photon transport on, no decay photons | 940 | 241 k |
| D1S decay photons on, no tallies | 280 | 72 k |
| Full D1S tally (52 radionuclides) | 265 | 69 k |

S0 measured 12 histories/s. The ray tracer accounts for about 22× of that. What remains is physics: photon transport costs
×6.5, decay-photon sampling a further ×3.4, and the tallies about 5 %.

#### D5c: R2S neutron-step uncertainty

| Seed | Total dose at 1 d (pSv·cm/s) | Photon-step R |
| --- | --- | --- |
| S0 | 1.3064e17 | 0.00113 |
| 90080001 | 1.3572e17 | 0.00113 |
| 90080011 | 1.3256e17 | 0.00112 |

Seed-to-seed relative standard deviation 1.93 %, against 3 × photon R = 0.34 %. **Rule outcome: S0's R2S uncertainty is
understated** (by about 17×), so VAL-043 must propagate the neutron-step uncertainty in S3. A 2 % spread does not explain
R3's 0.64.

#### D6: upstream defects

Three drafts in `docs/defects/`, each reproduced on both versions, checked against OpenMC's issues, and checked on
develop (aa4afc41eb) by the coordinator:

- **OPENMC-001:** stochastic-slab MGXS ignores `model.materials.cross_sections`. 0.16.0 and develop; not 0.15.3.
- **OPENMC-002:** the R2S neutron step ignores it. Both versions and develop.
- **OPENMC-003:** a negative transport-corrected total for a low-density material aborts random ray. 0.16.0 and develop;
  not 0.15.3. Likely introduced by PR #3658.

None is posted. `rel_max_lost_particles` and `clip_tolerance` behave as documented and are not defects. D5a found no
defect.

#### Summary for change control

| Item | Rule outcome | Goes to |
| --- | --- | --- |
| D1 | adopt double-down (33×) | FARIS default build |
| D3 | G\* 8.87×, band < 10; window-resolution lever untested | NUC-050 proposal + release-risk flag; follow-up protocol |
| D4 | PERF-025 numbers not kept; start-up is 70 % of the budget | PERF-025 proposal |
| D5a | no D1S defect; H1 rejected; H2 (mesh-R2S voxel source) raised | S3 dose design; new shutdown-dose protocol |
| D5c | R2S uncertainty understated | VAL-043 proposal |
