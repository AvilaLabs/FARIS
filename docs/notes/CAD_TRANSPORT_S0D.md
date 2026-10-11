# Targeted weight windows on RM-M (stage S0d)

Status: protocol frozen 2026-10-10, before any S0d run. Results are appended in a section below; this protocol is never
edited. Any change to a rule after the first run is an amendment with a date and reason. Parents:
[CAD_TRANSPORT_DIAGNOSTICS_S0B.md](CAD_TRANSPORT_DIAGNOSTICS_S0B.md) (S0b, G6 gain 8.87×) and
[CAD_TRANSPORT_S0C.md](CAD_TRANSPORT_S0C.md) (S0c, 85× on the P1 slab with the same settings).

S0c ended with RM-M losing an order of magnitude of weight-window gain to something the slab does not contain, and
proposed separating two causes: window depth (G6 windows clipped against unclipped) and 3D paths (a 2D stand-in with a
port). Reading the existing records and OpenMC's source before running anything found a third cause that comes first,
and showed the clipping test would mostly clip windows in a group that response (a) does not score. S0d replaces S0c's
proposed test with the one below. **S0d is capped at 60 minutes of machine time**; an item that does not fit is reported
NOT_EVALUATED with its measured cost so far, and a separate run is proposed with an estimate.

## Facts established before this protocol

Read from existing records and source code; no new runs.

1. **The windows so far were global, not aimed at (a).** In OpenMC 0.15.3 the random-ray FW-CADIS adjoint source is
   1/φ (forward flux) in every source region and every energy group (`FlatSourceDomain::set_adjoint_sources`,
   `src/random_ray/flat_source_domain.cpp` at v0.15.3). No tally restricts it. G6's generation record agrees: 155,483 of
   172,336 source regions carried an adjoint source. So S0, S0b and S0c all generated windows that try to make the
   flux's relative error uniform everywhere and at every energy. The `objective: a` field in the generation records
   names the tally present in the model, not what OpenMC aimed at.
2. **OpenMC 0.16.0 can aim them.** PR #3717 (merged 2026-04-28, in 0.16.0, released 2026-08-05) adds
   `WeightWindowGenerator(targets=...)`, which sets 1/φ only in source regions and groups a target tally covers
   (targeted FW-CADIS), and `settings.random_ray['adjoint_source']`, a user adjoint source with no forward solve
   (CADIS). `openmc016` is the conda-forge 0.16.0 DAGMC build with double-down linked (`libdd.so`), the same ray tracer
   S0b D1 adopted.
3. **G6's windows barely act on (a).** Per history, G6 lowers the variance of (a) 9.6× against the analog
   (R²N = 13,300 against 127,700, from S0b D2 and G6) at 1.08× the cost per history (3,924 against 4,242 histories/s).
   On the slab, the 25 cm windows lower it 194× at 2.3× the cost. The windows are not concentrating histories on the
   paths to the coils.
4. **G6's deep windows are thermal.** The 2.7e-13 minimum is in the group below 0.058 eV. In the two groups above
   0.1 MeV, the ones (a) scores, G6's lower bounds span 3.3e-7 to 0.25. Along the inboard midplane they fall from
   about 0.04 in the plasma to 3e-4 at the shield's back face and 3e-5 inside the coil, a profile like the slab's.
5. **The response is spread over 18 whole coils.** (a) is a material tally over every TF coil, inboard and outboard
   legs. The outboard build differs from the inboard one (100 cm FLiBe against 20 cm; 20 cm TiH2, authored, against
   51 cm), and the port is a 30 cm duct at the outboard midplane between two coils. Global windows spend effort on all
   of this, and on every other region, at every energy.

**Hypothesis H4**, stated before testing, may be false: RM-M's gain is limited because its windows are global. Windows
aimed at (a) raise the gain on RM-M well above G6's 8.87×.

## Common settings

- Model: RM-M as in S0b (h5m `rm_m_rr.h5m` sha256 `5144ec9a…` for generation, `rm_m.h5m` for production), the FARIS
  audited library, the S0b production tallies and source.
- **Cross sections for generation: G6's own `mgxs.h5`** (sha256 `cc4262da…`, made on 0.15.3, CASMO-8 plus a 0.1 MeV
  edge), not regenerated. This keeps the multigroup data identical across configurations and keeps 0.16.0's
  stochastic-slab defect (OPENMC-003) out of the test.
- Generation parameters: G6's (rays 20,000, 60 batches with 30 inactive, flat source, 25 cm window and source-region
  mesh over the S0b ray box, hybrid volume estimator), new generation seeds.
- **Production: always on `openmc0153dd`**, so S0b's D2 analog stays the reference for every gain (same build, model,
  library). G6's production settings: `max_split` 1000, checkpoints at surface crossings and collisions, 10,000
  histories per batch, 4 threads on cores 0–3, **600 s** (half of S0b's 1200 s; FOM is normalised by time). If 0.15.3
  cannot read a 0.16.0 window file directly, the windows are read with 0.16.0's Python API and written with 0.15.3's,
  and the coordinator checks the bounds are identical before production.
- Seeds from 90110001 upward, disjoint from S0, S0b and S0c, recorded per run.
- Jobs under `systemd-run --user --scope -p MemoryMax=6G -p MemorySwapMax=0`; generation at 4 threads; nothing else
  heavy running during a production (load recorded at start and end); scratch in `~/.cache/avila-night/cad-risk-tests/s0d/`.
- Scripts and their unit tests in `integrations/openmc/risk_tests/` (`s0d_*.py`).

## Runs, in order

- **T0, smoke (≤ 3 min):** 0.16.0 loads the model with G6's `mgxs.h5`, accepts the targeted generator on the
  production tally (material filter on the TF material, energy filter 0.1–20 MeV), and runs one random-ray batch;
  0.15.3 reads a 0.16.0 window file. A failure here ends S0d with the error recorded.
- **T1, targeted FW-CADIS:** 0.16.0, `targets` = the (a) tally. Generation, then production.
- **T3, control** (run only if T1's gain differs from G6's by more than 3σ combined): 0.16.0, no targets, otherwise T1.
  It separates the targeting from other random-ray changes between 0.15.3 and 0.16.0.
- **T2, CADIS:** 0.16.0, no forward solve; `adjoint_source` uniform over the TF material with equal strength in the two
  groups above 0.1 MeV (the flux response (a) scores). Generation, then production.

**Report for each:** generation time and peak memory, source regions carrying an adjoint source, window range in the
two groups above 0.1 MeV and overall, production R(a), histories, histories/s, FOM(a) ± σ, gain against D2, z against D2,
lost particles. FOM σ as in S0b (relative uncertainty of R² taken as √(2/(n−1)) for n batches).

## Rules

- **Unbiased:** (a) within 3σ of D2; a configuration that fails is excluded.
- **H4 confirmed** if T1 or T2 is unbiased and its gain exceeds G6's 8.87 ± 0.62 by more than 3σ combined and by a factor
  of at least 2, and, when T3 ran, T3's gain is within 3σ of G6's (so the rise is the targeting, not the version).
- **H4 rejected** if neither T1 nor T2 exceeds G6's gain by a factor of 2. S0c's two remaining causes (3D paths, window
  depth in the scored groups) are then the next test.
- **Attribution not established** if T3 itself exceeds G6 by more than 3σ: the rise is then at least partly the version,
  and the result says so.
- **NUC-050:** the best unbiased gain G\* among T1 and T2 is compared with 100×. G\* ≥ 100× clears the release-risk flag
  for NUC-050 on RM-M, subject to an S2 rerun at 1200 s. 10× ≤ G\* < 100× keeps the flag and the target; the change-control
  proposal records G\* and the configuration.
- **Generation path:** if H4 is confirmed, FARIS's window generation needs targeting. That is a design decision (0.16.0
  for generation, a targeted adjoint source on 0.15.3, or FARIS's own adjoint source) for the coordinator after the
  results, not a rule outcome here.

## Estimate and footprint

T0 3 min; T1 about 13 min generation (G6 took 742 s) plus 10 min production; T3 the same; T2 about 7 min generation plus
10 min production. About 63 min if T3 runs, 43 min if it does not. Four cores, under 2 GB. The box is 60 min, so T2 may be
NOT_EVALUATED if T3 runs; it is then proposed separately.

## Outputs

- A results section appended here.
- `references/cad-transport-s0d.json` with every run's record, hashes and rule outcomes.
- Updates to the NUC-050 change-control proposal.

## Results

Appended after runs.
