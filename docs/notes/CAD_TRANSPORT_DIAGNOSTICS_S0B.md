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
