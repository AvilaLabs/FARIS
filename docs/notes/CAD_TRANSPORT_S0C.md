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
