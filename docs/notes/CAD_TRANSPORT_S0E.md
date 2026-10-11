# Energy cut-off, a build-aligned mesh and a per-region breakdown on RM-M (stage S0e)

Status: protocol frozen 2026-10-11, before any S0e run. Results are appended in a section below; this protocol is never
edited. Any change to a rule after the first run is an amendment with a date and reason. Parent:
[CAD_TRANSPORT_S0D.md](CAD_TRANSPORT_S0D.md) (S0d: CADIS windows aimed at response (a) give 19.65 ± 1.60× against the
analog; NUC-050 asks for 100×).

S0d's judgment named three levers: a mesh that follows the build, source biasing from the same adjoint, and a breakdown
of (a) by region. Reading OpenMC's source before writing this protocol removed one of them and added another.
**S0e is capped at 75 minutes of machine time** and runs only on a quiet machine (below); an item that does not fit is
reported NOT_EVALUATED with its measured cost so far.

## Facts established before this protocol

Read from existing records and from OpenMC's source at the v0.15.3 and v0.16.0 tags; no new runs. Line references are in
`~/.cache/avila-night/s0e-research/openmc-capabilities.md`.

1. **Source biasing is not available for this source.** 0.15.3 has none at the distribution level. 0.16.0 adds `bias` to
   most distributions (PR #3460), but RM-M's plasma source is a mesh source of per-element sources, and reading the C++ that
   loads a biased `MeshSpatial` distribution suggests it reads the unbiased strengths (not run; to be verified as a
   possible defect before anything relies on it). Random-ray FW-CADIS in 0.16.0 writes windows only; automatic source
   biasing is an open pull request (#4136). Source biasing is therefore not tested in S0e.
2. **Windows on a cylindrical mesh work in both versions, untested upstream.** In 0.16.0, `source_region_meshes` accepts any
   mesh and the random-ray code uses only the generic mesh interface (`bins_crossed`, `get_bin`); `WeightWindowGenerator`
   accepts a `CylindricalMesh`. 0.15.3 applies windows through the generic `get_bin` and reads cylindrical meshes from a
   weights file. If the window mesh and the source-region mesh differ, each source region's flux goes to one window bin
   without warning, so S0e uses the same mesh for both.
3. **A 0.1 MeV neutron energy cut-off is exact for (a).** (a) scores neutron flux above 0.1 MeV in the TF material. RM-M
   holds no fissile material, and its neutron-producing reactions have thresholds in the MeV range (Be-9 (n,2n) 1.85 MeV,
   Li-7 (n,n′t) 2.8 MeV); thermal upscattering reaches eV, not 0.1 MeV. A neutron below 0.1 MeV can therefore never
   contribute to (a), and killing it changes (a)'s expectation by nothing while saving the time spent tracking it. The
   S0d CADIS windows exist only above 0.1 MeV, so those neutrons are tracked in full today. The TBR tally is biased by the
   cut-off and is not reported for cut-off runs. OpenMC 0.15.3 has the setting (`settings.cutoff['energy_neutron']`).
4. **RM-M's 25 cm Cartesian cells do not follow its build.** The build along the midplane at a coil centre (φ = 90°),
   measured by intersecting a line with each solid of `rm_m.step` (`~/.cache/avila-night/s0e-research/midplane.py`) is,
   in cm of R: inboard TF coil 64–128, gap 128–129, TiH2 129–180, thermal shield 180–183, tank 183–186, FLiBe 186–206,
   vessel 206–213, first wall 213–214, scrape-off layer 214–217, plasma 217–443 (the model uses a = 113 cm), then
   outboard scrape-off layer 443–446, first wall 446–447, vessel 447–454, FLiBe 454–554, tank 554–557, thermal shield
   557–560, TiH2 560–580, gap 580–581, TF coil 581–645. At φ = 0° the port duct runs from 446 to 580. The 51 cm inboard
   TiH2 shield, which carries most of the attenuation to the inboard coil legs, spans about two Cartesian cells that also
   cut across neighbouring layers. On the slab, 25 cm cells aligned with these layers gave 85× (S0c).
5. **On the slab, finer global windows went deeper and stalled (S0c).** 10 cm windows reached 1e-11 and gave 12×; 5 cm
   reached 1e-14 and wrote no statepoint. Those were global FW-CADIS windows on 0.15.3, whose deepest bounds were thermal
   (S0d fact 4). Aimed windows exist only above 0.1 MeV, where the depth follows fast attenuation. Whether aimed windows on
   a finer, aligned mesh also stall is not known; S0e keeps the radial cells no finer than 5 cm and the split limit at
   1000, as in S0d.
6. **Where (a) and its variance come from is not recorded.** The S0d productions tallied (a) as one bin (TF material, one
   energy bin) and the TBR, nothing else.

## Hypotheses

- **H5 (cut-off).** With S0d T2's windows, a 0.1 MeV neutron cut-off leaves (a) unbiased and raises its figure of merit
  above the same windows without the cut-off.
- **H6 (aligned mesh).** CADIS windows generated on a cylindrical mesh whose radial edges follow the midplane build raise
  (a)'s figure of merit above T2's 25 cm Cartesian windows.

## Common settings

As S0d T2 except where stated: OpenMC 0.16.0 (`openmc016`) for generation with S0b G6's multigroup library
(`s0b/d3/G6/generate/mgxs.h5`, not regenerated) and CADIS adjoint source over the TF material in the groups above 0.1 MeV;
OpenMC 0.15.3 with double-down (`openmc0153dd`) for production, 600 s budget, 10,000 particles per batch, statepoint
every 5 batches, `max_split` 1000, checkpoints at surface crossings and collisions, 4 threads on cores 0–3 under
`MemoryMax=6G`. Gains are against S0b D2 (FOM 0.0332 ± 0.0008) with the S0d formulas (`s0b_common.gain_with_sigma`,
`z_score`, `unbiased`).

**Breakdown tallies, in every production run** (so every S0e run carries the same tally cost):

- (a) per TF coil: a cell filter over the 18 coil volumes, E > 0.1 MeV, flux.
- (a) as an (R, Z) map: a `CylindricalMesh` filter, R 0–700 cm in 20 cm bins, Z −400 to 400 cm in 20 cm bins, one φ bin,
  with the TF material filter and E > 0.1 MeV, flux.

**The cylindrical mesh for H6** (`CylindricalMesh`, origin on the machine axis, one mesh for source regions and windows):

- R edges (cm), following the measured midplane build of fact 4: 0, 32, 64, 96, 128, 129; TiH2 129–180 in 10 equal
  cells; 183, 186; FLiBe in 2 equal cells to 206; 213, 214, 217; plasma 217–443 in 9 equal cells; 446, 447, 454; FLiBe
  454–554 in 4 equal cells; 557, 560; TiH2 560–580 in 4 equal cells; 581; TF coil 581–645 in 4 equal cells; 670, 700.
  52 radial bins.
- Z: −400 to 400 cm in 25 cm steps (the model spans Z ±381 cm).
- φ: 0 to 360° in 5° steps (72 bins; each coil spans 16° of its 20°). 52 × 32 × 72 = 119,808 mesh bins, against
  172,336 source regions for T2's Cartesian mesh.

The exact edge list is written by `s0e_common.py` and recorded with its sha256 in the generation record.

## Runs, in order

0. **E-smoke.** Two random-ray batches (one inactive) of the H6 generation, then 0.15.3 loads the window file and runs one
   batch. Passes if both complete, the window file's mesh is cylindrical with the edges above, and 0 particles are lost.
   If it fails, E2 and E3 are NOT_EVALUATED with the error, and E0 and E1 still run.
1. **E0 (reference).** T2's window file (`s0d/t2/gen/weight_windows.h5`) unchanged; new production seed. Gives today's
   reference under today's load, a repeat of T2 and the breakdown.
2. **E1 (H5).** As E0 with `cutoff['energy_neutron'] = 1e5` eV.
3. **E2 (H6).** CADIS generation on the cylindrical mesh (new generation seed), then production as E0 with those windows.
4. **E3 (combination, conditional).** E2's windows with E1's cut-off. Runs only if H5 and H6 are both confirmed.

Seeds: generation E-smoke 90130000, E2 90130002; production E0 90140000, E1 90140001, E2 90140002, E3 90140003. They are
disjoint from every earlier stage's seeds (checked by test).

## Quiet machine

Each production starts only when the 1-minute load average is below 2.0 and no cargo build holds the FARIS build lock;
the driver waits (the wait does not count toward the cap). Load is recorded at start and end of each job. If the load
during a production exceeds 6.0 at its end, the run is reported with a "contended" note; the rules still apply.

## Rules

Mechanical, in `s0e_common.py`, tested before the first run.

- **Unbiased:** |z| of (a) against D2 ≤ 3 (as S0d).
- **H5 confirmed** if E1 is unbiased and its gain exceeds E0's by more than 3σ (combined) and by a factor ≥ 1.2.
  **Rejected** if E1 is unbiased and within 3σ of E0 or below it. **Biased** (E1 not unbiased) falsifies fact 3 and stops
  the adoption of the cut-off until explained.
- **H6 confirmed** if E2 is unbiased and its gain exceeds E0's by more than 3σ and by a factor ≥ 1.2. Rejected otherwise;
  NOT_EVALUATED if E2 did not produce a gain.
- **G\*:** the largest gain among unbiased E0–E3. NUC-050 band as S0d: ≥ 100 met; 10–100 flag and target stay, G\* to
  change control; < 10 flag stays and the method is re-examined.
- **E0 against T2:** reported as a repeatability check (z of the gain difference); no rule depends on it.

## Breakdown analysis (descriptive, no rule)

From E0: each coil's share of (a) and its relative error; the (R, Z) map's share of (a) in four regions: inboard leg
(R < 129 cm), outboard leg (R > 580 cm), top and bottom (the rest, split by the sign of Z); and the largest-variance bins,
using σᵢ² / Σσⱼ² as an approximate variance share (it ignores the covariance between bins from one history, and says so).
The coil next to the port (centres at 10° and 350°) is compared with the mean of the other 16.

## Estimate and footprint

E-smoke about 3 min; E0, E1, E3 about 11 min each (600 s plus start-up); E2 about 15–20 min (generation about 4–8 min, as
T2's 256 s plus the cylindrical mesh's extra regions). Total about 50–55 min of jobs, within the 75 min cap, plus waiting
for a quiet machine. Cores 0–3, at most 6 GB per job (S0d productions peaked about 1 GB, generation about 1.3 GB).

## Outputs

`~/.cache/avila-night/cad-risk-tests/s0e/` (`results-s0e.json`, run folders); hashes in `references/cad-transport-s0e.json`
with the results.

## Results

Appended after runs.
