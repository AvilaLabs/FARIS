<!-- kind: explanation -->
# Transport levers on stand-in models (stage S0c)

Status: protocol frozen 2026-10-10, before any S0c run. Results are appended in a section below; this protocol is never
edited. Any change to a rule after the first run is an amendment with a date and reason. Parent:
[CAD_TRANSPORT_DIAGNOSTICS_S0B.md](CAD_TRANSPORT_DIAGNOSTICS_S0B.md) (S0b), whose results name the three questions here.

S0b answered its questions on the full RM-M model, at hours per question. S0c answers each one on the smallest model that
contains the effect, in minutes, and uses RM-M only where the question is about RM-M itself. **The whole of S0c is capped
at 60 minutes of machine time.** An item that does not fit is reported NOT_EVALUATED with its measured cost so far, and a
separate run is proposed with an estimate.

Common settings: OpenMC 0.15.3 with double-down (`openmc0153dd`, adopted in S0b D1), the FARIS audited library, new seeds
disjoint from S0 and S0b and recorded, jobs under `systemd-run --user --scope -p MemoryMax=6G -p MemorySwapMax=0`, 4
threads for comparisons (every configuration within one question runs under the same conditions), scripts with unit tests
in `integrations/openmc/risk_tests/`, scratch in `~/.cache/avila-night/cad-risk-tests/s0c/`.

## Q1: what window resolution needs (NUC-050)

S0b's levers that did not change the windows gave ×3.4 together; the window mesh (25 cm) was never refined (G1 ran out of
memory), and windows reaching 1e-20 stalled G2.

**Stand-in model P1.** RM-M's inboard radial build from `rm_m_spec.py` (same layers, materials, thicknesses, in order from
the plasma to the TF inboard leg and beyond) as a CSG slab: layers normal to x, 100 cm × 100 cm in y and z with reflective
sides, vacuum at both x ends. Source: 14.1 MeV, isotropic, uniform in the plasma-side layer as RM-M's source region.
Response (a′): neutron flux above 0.1 MeV in the TF inboard layer, the same definition as RM-M's (a). TBR is not scored.

**Runs.**

1. **Analog reference:** analog P1 until R(a′) ≤ 0.05 or 600 s.
2. **Resolution sweep:** FW-CADIS windows from random ray on a regular mesh with x-spacing 25, 10, 5 and 2.5 cm (one
   cell in y and z, so source regions follow the layers), S0b G6's settings otherwise (CASMO-8 plus a 0.1 MeV edge,
   `max_split` 1000, surface and collision checkpoints). Production 120 s each.
3. **Window floor:** at the resolution chosen by the rule below, the same windows with every lower bound below
   1e-8 × the largest lower bound raised to that value. Production 120 s.

**Report:** for each configuration, generation time and memory, source regions, window range, R(a′), FOM(a′), gain against
the analog, z against the analog.

**Rules.**

- Unbiased: (a′) within 3σ of the analog; a configuration that fails is excluded.
- **Resolution needed (Δ\*):** the coarsest spacing whose FOM is within 1σ (combined) of the best FOM in the sweep.
- **Floor:** adopted if it raises FOM by more than the combined 1σ and stays unbiased.
- **RM-M confirmation:** if Δ\* is finer than 25 cm, RM-M windows are generated with a source-region mesh of spacing Δ\*
  applied only to the blanket, shield, vessel and TF domains (`source_region_meshes` per domain), 25 cm elsewhere, with the
  floor if adopted. The source-region count is computed before generation; generation and a 600 s production (4 threads,
  cores 0–3) run only if they fit the time box. The gain is formed against S0b's D2 (a standing reference: same model,
  library and build). Otherwise the item is NOT_EVALUATED with the source-region count and a cost estimate.

## Q2: what the 60 s preview spends its time on (PERF-025)

S0b D4 (alone): the first batch arrived 42–44 s after start; (a) reached R 0.25, against 0.20.

**Runs on RM-M**, G6 windows with S0b G5/G6 settings, 8 threads, nothing else running:

1. **Cold start breakdown**, once: in one Python process with `openmc.lib`, time `init` (cross sections, geometry,
   windows; the log's own lines split these where it can), `simulation_init`, and the first batch, separately.
2. **Warm preview**, twice with the same seed: after `init` and `simulation_init`, run `next_batch` until 60 s of wall time
   have passed since the preview began (not counting `init`), then read TBR and (a) with their R.

**Rule.** If both warm previews give TBR R ≤ 2 % and (a) R ≤ 20 %, the PERF-025 proposal keeps its numbers with the preview
defined in a loaded session, and the cold start is reported separately as its own measured number. Otherwise the measured
errors and the breakdown go to change control.

## Q3: does a voxel-uniform R2S source read high (VAL-043, hypothesis H2)

S0b D5a found D1S correct per nuclide when the decay-photon source follows the production profile, and found a uniform
source over a 5 cm sphere overestimated leakage 1.25–4×. Mesh R2S spreads each voxel's source uniformly, so on 10 cm
voxels over a steep production gradient it should read high. H2 is stated here before testing and may be false.

**Stand-in model P3.** A 40 cm slab of the RM-M iron surrogate (pure Fe, as in S0b D5a), 100 cm × 100 cm with reflective
sides, a 14.1 MeV planar source on one face, and a 10 cm void detector layer on the far face. Irradiation 1 FPY at unit
source rate, cooling 1 d, the p32 chain.

**Runs.**

1. D1S, photon current into the detector layer, with OpenMC's time-correction factors.
2. Mesh R2S (`R2SManager`, direct reaction rates) with voxels of 20, 10 and 2 cm across the slab thickness (full width in
   y and z), the same photon current.
3. Cell-based R2S with the slab divided into 2 cm cells.

Neutron and photon steps sized so each result has R ≤ 1 %; total for Q3 ≤ 20 minutes.

**Rules.**

- **H2 confirmed** if mesh R2S / D1S at 10 cm exceeds 1 by more than 3σ and 5 %, falls toward 1 as voxels shrink, and the
  2 cm mesh and the cell-based R2S are each within 5 % of D1S.
- **H2 rejected** if the 10 cm ratio is within 5 % of 1 or the trend is not monotone. The R3 discrepancy then stays
  unexplained, and the next candidates (RM-M's mixed-material voxels, the time correction for multi-step chains) are named
  in the results.
- Either way the result goes into the VAL-043 proposal and the S3 dose design.

## Outputs

- A results section appended here.
- `references/cad-transport-s0c.json` with every run's record, hashes and rule outcomes.
- Updates to the NUC-050, PERF-025 and VAL-043 change-control proposals.

## Results

Appended after runs.

### Results, 2026-10-10

Records: `~/.cache/avila-night/cad-risk-tests/s0c/` (`results-s0c.json`, `ledger.json`); hashes in
`references/cad-transport-s0c.json`. Job time 56.6 min (58.8 min wall from the first job) of the 60 min box, including 2.4 min of failed attempts and smoke
tests. Rule outcomes are mechanical (`risk_tests/s0c_common.py`, 43 tests); judgments are marked.

**Deviations and choices the protocol left open.**

- Q3's source is mono-directional along +x, sampled uniformly over the x = 0 face; the protocol named the face, not the
  direction.
- Q2 did not run alone: the S1b-fix and library-v2 builders' jobs were running (six processes near full CPU), load 13–14.
  A rerun alone is appended when the machine is free.
- Q3's D1S run stopped at its 420 s timeout without reaching R ≤ 1 %. Its R ≈ 4.4 % comes from the spread of its last 69
  batches, an estimate whose own relative error is about 9 %. The R2S neutron-step R is a first-order estimate that
  treats voxels as independent (`s0c_p3.py`).

#### Q1: window resolution on the P1 slab

The analog ran to its 600 s budget: R(a′) = 0.142 (1.2e7 histories), FOM 0.0829. Gains below are against an analog with R
above 0.1 and carry that caveat.

| Spacing | Source regions | Generation (s) | Window lower bounds | R(a′) | FOM ± σ | Gain vs analog | z |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 25 cm | 19 | 154 | 9.2e-7 … 0.5 | 0.034 | 7.07 ± 0.44 | 85 ± 6 | +2.26 |
| 10 cm | 31 | 201 | 2.1e-11 … 0.5 | 0.093 | 0.99 ± 0.32 | 12 ± 4 | +2.09 |
| 5 cm | 49 | 327 | 1.8e-14 … 0.5 | NOT_EVALUATED: no statepoint written in the 120 s production | | | |
| 2.5 cm | 89 | NOT_EVALUATED: did not fit the box (estimate about 11 min) | | | | | |

**Rule outcomes.** Δ\* = 25 cm, among the spacings evaluated. The floor raises no cell at 25 cm, so it is not adopted. The
RM-M confirmation is not triggered.

Supplementary, outside the protocol: the 5 cm windows with the floor applied (61 of 360 cells raised, range still 1e8)
finished 2 batches (4000 histories) in 120 s, R 0.33.

Judgment (coordinator):

- **Hypothesis wrong.** Finer windows got worse, not better. Each refinement made the windows reach deeper, from 1e-6 at
  25 cm to 1e-14 at 5 cm. Particles then split far beyond what the physical attenuation calls for, and the production
  stalls. The window mesh resolution is not the lever S0b's results pointed to.
- **The physics allows the target.** With S0b G6's settings and 25 cm windows, the same layer stack gives 85× on the slab,
  against 8.9× on RM-M. So 100× is within reach for this build, and RM-M loses an order of magnitude to something the 1D
  slab does not contain: 3D paths (ports, gaps, the toroidal geometry), or windows whose depth runs away as here (RM-M's G6
  windows reach 2.7e-13).
- **NUC-050 stays at 100×.** The release-risk flag stays until the RM-M loss is located. The next test needs to separate
  those two causes on RM-M: the G6 windows clipped to the slab's depth against unclipped, and a 2D stand-in with a port
  against the slab.

#### Q2: preview start-up and the warm preview (RM-M, G6 windows, 8 threads)

Cold start in one `openmc.lib` process: `init` 18.4 s, `simulation_init` 0.02 s, first batch 0.31 s, 18.7 s to the first
batch. S0b's D4 measured 42–44 s to the first batch through its run script, which builds and writes the model before
OpenMC starts.

| Warm preview | Batches (1000 histories) | TBR R | (a) R |
| --- | --- | --- | --- |
| 1 | 224 | 0.46 % | 19.8 % |
| 2 | 134 | 0.55 % | 32.1 % |

**Rule outcome: to change control** (preview 2 misses (a) ≤ 20 %). Both previews ran under load 13–14, and preview 2 got 40 %
fewer batches. Judgment: the rule cannot be judged on a contended run. The rerun alone decides it, and is appended below.

#### Q3: mesh R2S against D1S on the P3 iron slab (tests H2)

Photon current through the far face at 1 d after 1 FPY, unit source rate.

| Method | Result (photons/s) | R | Ratio to D1S |
| --- | --- | --- | --- |
| D1S | 1.782e-5 | 4.4 % | 1 |
| Mesh R2S, 20 cm voxels | 6.325e-5 | 1.2 % | 3.55 ± 0.16 |
| Mesh R2S, 10 cm voxels | 2.648e-5 | 3.1 % | 1.49 ± 0.08 |
| Mesh R2S, 2 cm voxels | 1.739e-5 | 1.9 % | 0.98 ± 0.05 |
| Cell R2S, 2 cm cells | 1.745e-5 | 1.3 % | 0.98 ± 0.05 |

**Rule outcome: H2 confirmed.**

- At 10 cm the ratio exceeds 1 by 0.49, more than 3σ and more than 5 %.
- The ratio falls monotonically with voxel size.
- The 2 cm mesh and cell-based R2S both agree with D1S within 5 %.
- Mn-54 dominates (1.47e-5 of 1.78e-5).

Judgment (coordinator):

- **R3 is explained.** S0's R3 compared D1S with mesh R2S on 10 cm voxels and found D1S/R2S = 0.64, which is R2S/D1S = 1.56.
  Here, 10 cm voxels give 1.49 on a plain iron slab. S0's discrepancy was the R2S reference reading high from voxel-uniform
  sources, not a D1S error. With S0b D5a (D1S right nuclide by nuclide), FARIS's fast shutdown-dose method stands.
- **Caveat on precision:** the D1S precision here is 4.4 %, not the 1 % the protocol asked for.
- **Consequences for VAL-043 and the S3 dose design:**
  - the rigorous reference must be cell-based R2S or mesh R2S with voxels fine enough that refining them changes the
    result by less than its error;
  - a 10 cm mesh R2S is not a reference for this kind of shield;
  - the comparison carries the R2S neutron-step uncertainty (S0b D5c).

#### Q2 rerun alone (2026-10-10, 22:01)

The same job (same script, seed 91060001, 8 threads, a fresh copy of the prepared run directory) with nothing else running:
load 1.7 at the start. Record `q2-rerun/q2.json` (sha256 `da082adfc3e4a9a692afd0cca39df8b7a966504db5990093d2209c3f1c7cea9e`), process snapshot `q2_rerun.ps` (sha256 `54b6b9e3d0badb10d66020b1ddb44d757700cddf23a5979c396467c987c6ba4a`).
Deviation: the rerun falls outside the 60 min box; the ledger total is 3537 s of job time plus this rerun's 139 s.

Cold start: `init` 13.7 s, 14.0 s to the first batch.

| Warm preview | Batches (1000 histories) | TBR R | (a) R |
| --- | --- | --- | --- |
| 1 | 279 | 0.41 % | 21.3 % |
| 2 | 277 | 0.44 % | 14.9 % |

**Rule outcome: to change control** (preview 1 misses (a) ≤ 20 % by 1.3 points). This run replaces the contended one for
judgment.

Judgment (coordinator):

- **TBR is met with a factor of five to spare.** The cold start is 14 s, a third of S0b's 42–44 s through the run script.
- **(a) is on the edge.** With the same seed the two previews differ by 6 points, because the error estimate of a deep,
  sparse response is itself noisy. Meeting 20 % reliably needs about 1.2–1.5× more figure of merit on (a), much less than
  the weight-window gain NUC-050 still has to find (8.9× today against 100×). PERF-025's preview is therefore decided by
  NUC-050, not by start-up.
- **Proposal for PERF-025:** keep the numbers; define the preview as running in a loaded session (the in-process engine of
  the install plan keeps one warm); report the cold start separately as its own measured number; confirm (a) once the
  NUC-050 follow-up lands.

### Summary for change control

| Item | Rule outcome | Proposal |
| --- | --- | --- |
| Q1 (NUC-050) | Δ\* = 25 cm; finer windows go deeper and stall | Keep 100×; release-risk flag stays; next test separates 3D paths from window depth on RM-M |
| Q2 (PERF-025) | to change control: (a) 21.3 % and 14.9 % at 60 s, TBR 0.4 % | Keep the numbers; preview defined in a loaded session; cold start (14 s) reported separately; (a) follows NUC-050 |
| Q3 (VAL-043, S3) | H2 confirmed | Keep the 0.85–1.15 band; the rigorous reference is cell-based R2S or mesh R2S converged in voxel size; 10 cm mesh R2S is not a reference |
