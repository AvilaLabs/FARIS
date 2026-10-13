# Why RM-M's magnet response resists weight windows (stage S0f)

Status: diagnostics and pilots recorded 2026-10-11, before any S0f protocol. Nothing here is a rule outcome: every run
below was a single short pilot on a shared, loaded machine, and its figure of merit carries the heavy-tail uncertainty
S0e found. The protocol, when written, is appended below and frozen before its runs. Parent:
[CAD_TRANSPORT_S0E.md](CAD_TRANSPORT_S0E.md).

S0e left three questions: why E2′'s aligned windows stalled, why identical windows gave per-history variances 3.2×
apart, and where (a)'s variance comes from. These diagnostics answer the first fully and the others in part.

## Facts established

### 1. The E2′ stall is an OpenMC defect, not window depth

- **Mechanism (verified).** OpenMC scales each particle's windows by the window it first meets
  (`wgt_born / wgt_ww_born`). When that first check falls in a bin the generator wrote as −1 (rejected flux estimate),
  `wgt_ww_born` is set to 1.0. FW-CADIS windows on RM-M sit near 1e-7, so the particle is split by `max_split` at every
  event, up to `max_history_splits` (1e7). Drafted as [OPENMC-004](../defects/OPENMC-004-first-check-in-invalid-window-splits-without-limit.md).
- **On RM-M.** E2′'s windows had 2 invalid bins in the 0.82–20 MeV group, one in the plasma core
  (R 368–393 cm, φ 355–360°, Z 0–25 cm). Of 200,000 sampled source sites, 3.45e-4 fall in an invalid bin. T2's
  windows have none, which is why T2 never stalled.
- **Reproduced.** A 30,000-history run with E2′'s windows and the cut-off stopped in batch 11 of 30. With the 16
  invalid bins filled from their face neighbours (geometric mean), the same run finished in 8.8 s. A 100 cm iron cube
  reproduces it on 0.15.3 and 0.16.0 (200 histories: 1.2 s valid, no finish in 120 s with the source bin invalid); the
  code is unchanged on develop aa4afc41e.
- **Depth was not the cause.** After the per-particle scaling, what matters is a window's ratio to the birth window.
  In the 0.82–20 MeV group that ratio spans 4.8 decades for T2 and 5.4 for E2′.
- **Fix in FARIS:** `s0f_common.fill_invalid` and `fill_file` (pure Python, tested in `test_s0f.py`; identical to a
  numpy implementation within 2e-15 on E2′'s file).

### 2. Source-position biasing would not help

At the 200,000 sampled source sites, the importance at birth (1/lower bound, source group) has relative variance
E[I²]/E[I]² − 1 = 0.05 for T2's windows and 0.40 for E2′'s. A source biased in position by this importance could remove
at most that much relative variance per history, against the 3,700–21,000 measured below.

### 3. The variance sits on one strip of the two port coils

- **Per coil** (T2 windows with the cut-off, 1.5 M histories): every coil reaches a relative error of 0.08–0.13. The
  two coils beside the port carry 57 % of (a) and 92 % of its variance (summing coil variances, without covariance).
- **Within the port coil** (a 5 cm map over tf_coil_00's region, 2.9 M histories): 84 % of the coil's (a) and 99 % of
  its variance lie in the strip φ < 4°. That is the coil's side face, looking into the void between the coils
  (R 581–645 cm, y ≈ 20–25 cm).
  - 81 % of the variance lies in that strip at |Z| < 20 cm, level with the duct.
  - The largest-variance bins are all at y = 22 cm, R 588–643 cm. One of them holds 14 % of the variance and 0.4 % of
    the mean: a single large event.
- **Not straight-through streaming.**
  - Neutrons flying from the plasma through the duct without a collision and entering a port coil's side face give
    2.8e-6 cm per source neutron. That is a lower bound, with everything but plasma, SOL and duct opaque;
    `uncollided.py` uses 8 M importance-sampled rays.
  - The port coils' (a) is about 6e-4 cm per source neutron, so straight-through flight is about 0.5 % of it.
- **What the collision-count tally can and cannot say.**
  - In an analog run, 1.3 % of the port coils' (a) comes from neutrons with no collision.
  - The count includes collisions inside the coil, so it does not separate paths outside it.
  - Under windows the count is not usable at all: OpenMC restarts it at 0 for every split copy (`from_source`).

### 4. No change of scalar windows moved the variance floor

Pilots, each a single run with the 0.1 MeV cut-off, checkpoints at collisions and surfaces, and `max_split` 1000.
"Variance per history" is R² × N for (a); the gain is FOM / 0.0332 (S0b D2), as before.

| Pilot | Windows | Histories/s | Variance per history | Gain vs D2 | Load start–end |
| --- | --- | --- | --- | --- | --- |
| coll-t2 | T2 (25 cm Cartesian, 2 groups above 0.1 MeV) | 4,004 | 4,650 | 25.9 | 1.9–7.0 |
| p1-e2p | E2′ aligned cylindrical, filled | 4,052 | 7,654 | 15.9 | 3.3–6.3 |
| p1-fr | 25 cm Cartesian, 10 groups above 0.1 MeV, filled | 9,068 | 7,571 | 36.1 | 3.4–5.7 |
| p1-frp | cylindrical refined at the port (1°, 10 cm), 10 groups, filled | 5,059 | 7,528 | 20.2 | 3.8–6.1 |
| map-fr | as p1-fr, with the port map tally | 6,869 | 21,569 | 9.6 | 3.8–6.0 |
| p1-b10 | p1-frp with windows ÷10 in the port region | 3,845 | 5,436 | 21.3 | 3.9–5.0 |
| p1-b100 | p1-frp with windows ÷100 in the port region | 506 | 3,678 | 4.1 | 2.3–2.5 |
| analog-coll | none (analog with the cut-off) | 6,286 | 91,236 | 2.1 | 2.7–7.6 |

- **Machine conditions.**
  - coll-t2 and p1-e2p ran unpinned. `AllowedCPUs` is not enforced in user scopes on this machine; later runs used
    `taskset -c 0-3`.
  - Pilots with the collision tally (18 coils × 400 collision bins) or the map tally pay its cost in histories/s.
  - Seeds 77000001–77000051 (77000051 was reused for p1-b10, and p1-b100 got 770000510); outside every protocol's
    range.
- **Energy groups.** With 10 groups above 0.1 MeV (edges 0.1, 0.5, 0.821, 1.353, 2.231, 3.679, 6.0655, 10, 13, 15,
  20 MeV), the windows tell a 15–20 MeV neutron from a 1 MeV one. Their largest bound falls from 2.7e-4 (0.1–0.5 MeV)
  to 2.5e-9 (15–20 MeV). Histories ran about twice as fast (9,068/s), but the variance per history did not fall.
  - Generation took 288 s, against T2's 256 s.
  - The multigroup library is stochastic slab on 0.15.3 with S0b G6's settings and seed 77000031
    (`mgxs.h5` 944f1b6a…).
- **Port refinement and importance boosts.** Refining the windows at the port (1° in φ, 10 cm in Z and in the outboard
  FLiBe; 190,608 bins, 364 s) lowered the relative error of the coils away from the port: 0.07 for the two read (tf_coil_01 and tf_coil_07),
  against 0.08–0.13 with T2. It left the port coils' errors where they were (0.14 and 0.12). Lowering the windows around the duct 10× or 100× bought more splitting and no fall in
  variance.

### 5. What the floor is (inference, not yet tested directly)

- **The mechanism.**
  - OpenMC applies a window after a collision has chosen the new direction, so every split copy leaves in the same
    direction.
  - A copy that is going to fly down the duct and out to the coil's side face takes its siblings with it. They arrive
    together, and the event is as rare as the single direction choice that made it.
  - Splitting at surface crossings in void does the same.
  - The direction choices that feed the strip happen at collisions in material that faces the duct mouth across void,
    which includes the plasma-facing wall anywhere in line of sight of it.
  - A window that is the same for every direction cannot make those choices more frequent without flooding the whole
    wall with copies. Fact 4's boosts are consistent with this: copies made near the duct mostly share their parent's
    direction.
- **Size of the floor.** If one class of event carries a share s of (a) at an independent rate λ per history, it adds
  about s²/λ to the variance per history. With s ≈ 0.5 and a floor near 4,000–8,000, λ ≈ 3–6e-5: roughly one
  independent duct event per 20,000–30,000 histories, whatever the windows.
- **Tools that act on direction.** DXTRAN, point or next-event estimators, and angle-dependent (CADIS-Ω) windows do.
  None is in OpenMC 0.15.3, 0.16.0 or develop aa4afc41e (searched 2026-10-11).

### 6. Splitting before the collision does not move the floor (2026-10-12; weakens fact 5)

- **What was tried.** A patched OpenMC 0.15.3 that, when a neutron arrives at a collision above its window, splits it
  before the reaction is sampled, so each copy samples its own outgoing energy and direction. It is switched on with
  `OPENMC_SPLIT_BEFORE_COLLISION=1`; with the switch off it is stock 0.15.3. The patch is
  `~/.cache/avila-night/openmc-presplit/split-before-collision.patch` (sha256 2658db06…). It refuses to run with
  collision, analog or pulse-height tallies.
- **Unbiased on a duct problem.** The problem is an iron block with a straight void duct and an iron detector beside
  the duct mouth (`presplit/duct.py`, a36d6b06…). Detector flux, mean ± standard error over replicate seeds:
  - stock windows: 0.01198 ± 0.00007 (8 seeds × 200,000 histories);
  - split before collision: 0.01197 ± 0.00014 (8 seeds × 200,000);
  - analog: 0.01159 ± 0.00029 (4 seeds × 2 M).
- **No gain on RM-M.** Same settings as fact 4 (cut-off, both checkpoints, `max_split` 1000, cores 0–3, 300 s):

  | Pilot | Windows | Histories/s | Variance per history | Gain vs D2 | Load start–end |
  | --- | --- | --- | --- | --- | --- |
  | ps-fr | as p1-fr, split before collision | 8,815 | 7,437 | 35.7 | 1.6–4.1 |
  | ps-b10 | as p1-b10, split before collision | 6,052 | 9,096 | 20.0 | 1.9–3.3 |

  Against p1-fr (7,571) and p1-b10 (5,436), there is no gain beyond single-run scatter. Seeds were 77000061 and 77000062.
- **Why, on reflection.** Copies made by a split separate at their next collision, because each samples its own flight
  distance. Sharing a direction only correlates the one flight that follows the split. Fact 5's mechanism therefore
  cannot be the whole floor: windows that split a particle before it reaches the duct's walls already give its copies
  independent scattering directions there. What makes the side-face strip's events rare is not settled.

### 7. The spikes are straight flights through void onto the coil face; aiming the source at the duct removes most (2026-10-12)

Diagnostics only, single scans on a shared machine (other sessions were building Rust). Not a rule outcome.

- **Method.** The p1-fr model was run in batches of 1,000 histories (3,000 batches, seed 77000041, so the same histories
  as p1-fr), recording each batch's (a) and a side-face box score. The box is x 575–655, |y| < 50, |z| < 30, in coil
  material, above 0.1 MeV. Batches with spikes were replayed with OpenMC's particle tracks. Scripts are in
  `replay/`.
- **The spikes.** Five of 3,000 batches hold 45 % of (a)'s variance; the top one holds 15 %. Each spike is one history.
  - Batches 324, 542 and 1399: a source neutron flies uncollided from the plasma straight through the void duct onto a
    port coil's side face, leaving the duct at its edge (|y| or |z| ≈ 15 cm). In 1399 it clips the TiH2 lip at the
    exit (R 574–578) once.
  - Batch 2318: a neutron scatters off the inboard first wall (R 214). It flies at weight 4.4, having been rouletted up
    in a low-importance region, then crosses the chamber and the duct straight onto the coil face.
  - Batch 264: the first coil entry follows a scatter in FLiBe beside the duct exit; not traced further.
  - The windows split each arrival up to 1,000 times at the duct exit and again at the coil (3,946 to 21,319 tracks
    per history). Every copy is on the same line, so the event's size is set by one direction choice made at birth or
    on a far wall, where windows do not split.
- **Correction to fact 3.** Fact 3 said the strip is "not straight-through streaming", because uncollided flight is
  about 0.5 % of the mean. That was wrong. A rare class small in the mean can carry most of the variance, and this one
  does.
- **Fact 5 restored, with a correction.** The mechanism is a direction choice across void, as fact 5 said. The choice
  is made at birth or at far-wall collisions, where scalar windows roulette rather than split. That is why splitting
  before the collision (fact 6) had nothing to act on.
- **Aperture biasing (patched 0.15.3).** `OPENMC_APERTURE_BIAS="beta c u v"` sends a fraction β of isotropic source
  directions at a uniform point of the rectangle c + s·u + t·v.
  - Every site's weight is the isotropic density over the mixture density (1 − β)/4π + β·p_aperture(Ω).
  - The windows keep the unbiased birth weight, so biased particles are not split back up.
  - Tallies are normalized by the unbiased weight.
  - `OPENMC_WW_SKIP_VOID=1` skips split and roulette at birth and in void, so aimed particles are not rouletted before
    they reach material.
  - The patch with fact 6's is `~/.cache/avila-night/openmc-presplit/faris-openmc-0.15.3.patch` (c8cce2da…).
- **Unbiased on the duct problem.** Detector flux, mean ± standard error over seeds:
  - analog: 0.01178 ± 0.00009 (8 × 2 M);
  - analog with aperture biasing (β 0.2, aimed at the duct inlet): 0.01169 ± 0.00014 (8 × 2 M);
  - windows with skip-void and aperture: 0.01175 ± 0.00007 (8 × 200,000);
  - stock windows (fact 6): 0.01198 ± 0.00007.
- **RM-M scans.** All scans are 3 M histories with p1-fr's windows. Variance per history is the batch variance
  × 1,000 / mean²:

  | Scan | Change | Mean of (a) | Variance per history | Without top 20 batches | Histories/s |
  | --- | --- | --- | --- | --- | --- |
  | scan | none (stock) | 1.128e-3 | 7,978 | 2,761 | 8,324 |
  | sv-scan | skip-void | 1.135e-3 | 9,060 | 2,782 | 7,374 |
  | ap-scan | skip-void, β 0.1 at the duct exit (x 580, \|y\|, \|z\| < 15) | 1.127e-3 | 6,235 | 2,526 | 7,611 |
  | ap2-scan | skip-void, β 0.1 at x 578, \|y\|, \|z\| < 22 (exit and lip) | 1.041e-3 | 2,388 | 1,121 | 11,028 |

  - Skip-void alone does nothing: sv-fr's single 300 s pilot (4,753, 61× against D2) was scatter.
  - The narrow aperture removed spike 324 but not 542 or 1399, whose paths leave at the duct's edge.
  - The wide aperture removed all three birth-streaming spikes. Inboard-wall spike 2318 remains as the top batch.
  - ap2-scan's mean agrees with S0b's analog D2 (1.071e-3, R 0.065) within 0.4σ.
  - ap2-scan's FOM is 1/(R²T) = 1/(0.0282² × 272 s) ≈ 4.6, about 139× against D2. The stock scan's is about 31×
    under the same load. One scan per configuration; heavy tails make each variance uncertain by a factor of order 2
    (S0e saw 3.2× between identical runs).
- **Open.**
  - The far-wall scatter class (2318) needs the same aiming at collisions, which needs the scattering angle density.
  - The aperture was placed by hand from the traced spikes. For FARIS it must be found from the CAD: shield
    penetrations with a line of sight from the plasma to the response.
  - The tracks of the two largest histories were written without their `offsets` attribute (19,234 and 21,319 tracks;
    probably HDF5's 64 KB compact-attribute limit), so `openmc.Tracks` cannot read them. Not yet verified as a
    defect.

## Protocol (frozen 2026-10-12, before any production run; approved by Connor for about 70 min)

**Question.** Does fact 7's aperture-biased source reach NUC-050's 100× on RM-M, against S0b's D2, once it is measured
over replicate seeds? Does it beat the current windows?

**Configurations.** Both run the same base model: p1-fr's `model.xml`, copied to
`~/.cache/avila-night/cad-risk-tests/s0f/base-model.xml` (sha256 c0c107f1f79c06d6…). It has `fr_filled.h5`'s windows
inline, the 0.1 MeV cut-off, `max_split` 1000, checkpoints at collisions and surfaces, and no photons.

- **F0 (current windows):** stock OpenMC 0.15.3 from `openmc0153dd` (`libopenmc.so` ba09ae9d19c4c789…), with no FARIS
  variables set.
- **F1 (aperture-biased source):** the patched 0.15.3 (`libopenmc.so` efa4ae74da9be455…, patch c8cce2da…), run through
  the shadow package. It sets `OPENMC_WW_SKIP_VOID=1` and `OPENMC_APERTURE_BIAS="0.1 578 0 0 0 22 0 0 0 22"`: β 0.1,
  a rectangle at x = 578 cm with |y|, |z| < 22 cm.
- Each configuration checks its library and variables at start and refuses to run otherwise (`s0f_scan.py`).

**Runs.**
- Seeds 77100001–77100004 serve both configurations, run in the order F0, F1 for each seed. A shared seed gives the
  same source sites, and 90 % of F1's histories keep their direction.
- Each run is 3,000 batches of 1,000 histories, read batch by batch through `openmc.lib`. Threads 4, cores 0–3
  (`taskset`), memory cap 6 GB.
- T is the time from process start (before the library loads) to the last batch.
- Before each run, the driver waits up to 10 min for a 1-minute load below 3.0. Waiting is not job time. A run whose
  end load is above 6.0 is reported as contended.
- The time box is 75 min of job time. A run that would cross it is NOT_RUN.
- Driver: `s0f_scripts/run.sh`. Output: `~/.cache/avila-night/cad-risk-tests/s0f/`.

**Analysis** (`s0f_analyse.py`; rules in `s0f_common.py`, tested in `test_s0f.py`).
- **Pooled per configuration.** All 12,000 batches give the mean of (a), R, the variance per history and the VOV. T is
  the sum of the four runs. FOM = 1/(R²T), and the gain G is FOM / 0.03318 (D2).
- **Lower bound L.** The smaller of:
  - the one-sided 95 % Student-t bound on the four per-run gains;
  - the 5th percentile of a 2,000-resample bootstrap over batches (numpy seed 20261012).
- **Bias.** z of the pooled mean against D2 (1.0707e-3 ± 6.93e-5); unbiased when |z| ≤ 3.
- **VOV** is reported per run and pooled; above 0.1, the tail is reported as not resolved. No rule depends on it.
- **H7 (F1 beats F0).** Confirmed when all four paired per-seed FOM ratios F1/F0 exceed 1 and the pooled ratio is at
  least 1.2; otherwise rejected.
- **NUC-050 band for F1:**
  - biased: aperture biasing is not adopted; investigate;
  - L ≥ 100: NUC-050 met on RM-M with a hand-placed aperture;
  - G ≥ 100 > L: met at the point estimate only; more replicates before any claim;
  - 10 ≤ G < 100: target kept; the next lever is the far-wall scatter class;
  - G < 10: the scan's gain does not carry over.
- **Not decided here:** automatic aperture placement, which any FARIS claim needs, and whether FARIS ships the patched
  OpenMC.

## Results (2026-10-12; protocol 203938f; `references/cad-transport-s0f.json`)

All 8 runs completed (job time 2,320 s of 4,500). None was contended (end loads 3.2–4.9). Other sessions were building
during the job.

| Run | Gain vs D2 | Mean of (a) | Variance per history | VOV | T (s) |
| --- | --- | --- | --- | --- | --- |
| F0 seed 1 | 40.4 | 1.026e-3 | 7,910 | 0.19 | 283 |
| F0 seed 2 | 25.3 | 1.283e-3 | 12,629 | 0.11 | 283 |
| F0 seed 3 | 4.8 | 1.103e-3 | 67,720 | 0.90 | 276 |
| F0 seed 4 | 40.9 | 1.011e-3 | 8,156 | 0.12 | 271 |
| F1 seed 1 | 42.8 | 1.093e-3 | 6,629 | 0.34 | 318 |
| F1 seed 2 | 74.1 | 1.075e-3 | 4,325 | 0.27 | 282 |
| F1 seed 3 | 5.5 | 1.395e-3 | 53,529 | 0.48 | 309 |
| F1 seed 4 | 196.0 | 0.975e-3 | 1,653 | 0.02 | 279 |

- **F0, pooled.** G = 13.3, lower bound L = 6.7 (bootstrap; the seed bound is 7.9). Mean 1.106e-3, z = +0.41 against D2,
  so unbiased. Pooled VOV 0.43: the tail is not resolved.
- **F1, pooled.** G = 13.2, L = −17.5 (seed bound; the bootstrap gives 7.1). Mean 1.135e-3, z = +0.75, so unbiased.
  Pooled VOV 0.37: the tail is not resolved.
- **H7: rejected.** The paired per-seed FOM ratios F1/F0 are 1.06, 2.93, 1.13 and 4.79, all above 1. The pooled ratio
  is 0.995, below 1.2.
- **NUC-050: 10–100.** The target is kept, and the next lever is the far-wall scatter class.
- **What decided it.** In seed 3, one batch (1714) holds 95 % of F0's variance and scores 440× the mean. F1 shares the
  history, so it holds 65 % of F1's.
- **Fact 7's 139× scan does not carry over.** Its 3 M histories happened not to contain an event of this size.

### 8. The seed-3 event: a roulette survivor from the inboard wall (diagnostic, after the results)

The batch was replayed with tracks on the stock build (`replay/s3-b1714`). History 180 produced 1.6 M track states.

1. A 13.9 MeV source neutron hits the inboard tungsten at R 214 cm and scatters to 0.41 MeV.
2. Its window there is far above its weight (inboard wall, low importance), so roulette keeps it with probability
   1/1000. The survival weight is capped at weight × `max_split`, so it survives at weight 1,000.
3. It scatters back into the chamber, crosses the void at 0.40 MeV and enters the duct.
4. It is split at the duct entrance and again in the FLiBe beside the exit.
5. Copies fly onto tf_coil_00's side face (R 618, z −15), with 777,617 states in the box.

This is batch 2318's class (fact 7), at 1,000 times the weight. Two levers act on it:

- **Gentler roulette.** A separate, lower cap on the roulette survival factor, keeping splitting at 1,000. A wall
  neutron that escapes toward the duct early then carries a weight of about 10, not 1,000. The variance from these
  paths scales roughly with the weight of escaping particles. Cheap to test.
- **Aiming at collisions.** Bias directions at collisions on chamber-facing walls toward the duct, with the weight from
  the scattering density. OpenMC 0.16.0 can evaluate that density (#3550, "angular PDF evaluation for angle-energy
  distributions"); 0.15.3 cannot. A larger piece of work.

### 9. A gentler roulette moves the spike rather than removing it (diagnostic)

`OPENMC_WW_ROULETTE_FACTOR=10` caps a roulette survivor at 10× its weight before roulette; splitting stays at 1,000.
The patched build was rebuilt with it, and with the variable unset it behaves as before. Seed 77100003, 3,000 × 1,000
histories:

| Scan | Variance per history | Top batch's share | VOV | Gain vs D2 |
| --- | --- | --- | --- | --- |
| F0 (production) | 67,720 | 95 % (1714) | 0.90 | 4.8 |
| F1 (production) | 53,529 | 65 % (1714) | 0.48 | 5.5 |
| F0 + roulette ×10 | 28,755 | 90 % (1792) | 0.81 | 10.6 |
| F1 + roulette ×10 | 26,971 | 79 % (1792) | 0.65 | 11.0 |

Batch 1714's event is gone, but another history (batch 1792) now holds most of the variance. The cap's T includes an
estimated 15 s for library loading, which `batches.py` does not time.

Wall neutrons still reach high weight, only in steps. What fixes the class is making neutrons that scatter off
chamber-facing walls reach the duct more often, at low weight: aiming at collisions with the scattering density
(fact 8). The roulette factor is not a fix on its own.

### 10. Aiming elastic collisions at the aperture removes the seed-3 class (diagnostic, after the results)

Fact 8's second lever, built on 0.15.3 instead of migrating to 0.16.0. The patch backports the angular density
evaluation that elastic aiming needs (`AngleDistribution::evaluate`, `Tabular::evaluate`). It is
`integrations/openmc/patches/faris-openmc-0.15.3-aim.patch` (sha256 1c3d82df…), and the build's `libopenmc.so` is
c109f962…. With none of its variables set, it runs as stock 0.15.3.

- **The method.** `OPENMC_COLLISION_AIM="beta materials"` (`*` = all materials) aims a fraction β of elastic
  scatters at a uniform point of the aperture.
  - It applies only to targets heavier than the neutron at rest (awr > 1), so hydrogen is not aimed.
  - The weight is multiplied by p_phys / ((1 − β)·p_phys + β·p_aperture). p_phys is the lab-frame density of the
    direction, from the centre-of-mass angular density by the exact two-body Jacobian; the outgoing energy follows
    from the aimed angle.
  - Inelastic reactions are not aimed. That keeps the game unbiased, because the reaction is chosen before, and
    independently of, the direction.
- **A second window rule.** In F3, aiming worked but a surface-crossing roulette killed the aimed particles; a survivor
  then carried the spike (batch 767). So while aiming is on, a particle headed at the aperture skips window games until
  its direction changes.
- **Unbiased on the duct problem** (iron wall at z −35 to −30 cm, 8 seeds each, mean ± standard error):
  - detector flux: analog 0.015074 ± 0.000192, analog with aiming 0.015247 ± 0.000291;
  - duct band (mesh in the duct, z 50–100 cm, 12.5–13.8 MeV): analog 8.558e-4 ± 2.41e-5, analog with aiming
    8.703e-4 ± 1.64e-5;
  - with windows: stock 9.159e-4 ± 4.56e-5; windows with aiming in every material 8.882e-4 ± 3.81e-5. That is 0.7σ
    from analog. Its mean variance per history is 2.1× lower and its mean FOM 3.2× higher.
- **RM-M, seed 77100003** (protocol 1's worst seed), 3,000 × 1,000 histories, single scans:

  | Scan | Change from F1 | Mean of (a) | Variance per history | Top batch's share | Gain vs D2 |
  | --- | --- | --- | --- | --- | --- |
  | F1 (production) | — | 1.395e-3 | 53,529 | 65 % (1714) | 5.5 |
  | F2 | aim β 0.2 in material 7 only | 1.163e-3 | 12,211 | 70 % (2447) | 23.5 |
  | F3 | aim β 0.2 in all materials | 1.141e-3 | 9,135 | 85 % (767) | 25.1 |
  | F4 | F3 + aperture-headed particles skip windows | 1.083e-3 | 2,325 | 25 % (1459) | 99.7 |

  - F4's VOV is 0.090, the only run so far below 0.1. T was 390 s against F1's 309 s.
  - F2's spike (2447) was a FLiBe scatter behind the armour, outside material 7. F3's was the roulette described above.
- **Caveat.** This is one seed, already looked at, and heavy tails moved protocol 1's per-seed gains by 40×. The
  99.7 is not a result. Protocol 2 measures it on new seeds.

## Protocol 2 (frozen 2026-10-12, before any protocol 2 run; within the approved time box, about 50 min)

**Question.** Does F4 (fact 10) reach NUC-050's 100× on RM-M against S0b's D2, measured over new replicate seeds that
no diagnostic has seen? Does it beat the current windows?

**Configurations.** Same base model as protocol 1 (sha256 c0c107f1f79c06d6…), copied to
`~/.cache/avila-night/cad-risk-tests/s0f2/base-model.xml`.
- **F0:** as in protocol 1 (stock 0.15.3, `libopenmc.so` ba09ae9d…, no FARIS variables).
- **F4:** the patched 0.15.3 (`libopenmc.so` c109f962…, patch 1c3d82df…) through the shadow package, with
  `OPENMC_WW_SKIP_VOID=1`, `OPENMC_APERTURE_BIAS="0.1 578 0 0 0 22 0 0 0 22"` and `OPENMC_COLLISION_AIM="0.2 *"`.
  No other patch variable may be set (`s0f_common.CONFIG_ENV`, checked by `s0f_scan.py`).

**Runs.** As protocol 1, with seeds 77200001–77200004, run F0 then F4 for each seed. 3,000 batches of 1,000 histories,
4 threads on cores 0–3, 6 GB cap, the same load wait and contention flag, and a 75 min job-time box. Driver
`s0f_scripts/run2.sh`; output `~/.cache/avila-night/cad-risk-tests/s0f2/`.

**Analysis.** `s0f_analyse.py --pair F0 F4 --seed-set 2`. Pooled gain G, lower bound L, bias z against D2 and VOV, all
as in protocol 1.
- **H8 (F4 beats F0):** H7's rule. All four paired per-seed FOM ratios F4/F0 exceed 1, and the pooled ratio is at
  least 1.2.
- **NUC-050 band for F4** (`nuc050_outcome2`):
  - biased (|z| > 3): collision aiming is not adopted; find the bias first;
  - L ≥ 100: NUC-050 met on RM-M with a hand-placed aperture;
  - G ≥ 100 > L: met at the point estimate only; more replicates before any claim;
  - 10 ≤ G < 100: target kept; trace F4's top batches for the class that remains;
  - G < 10: the diagnostic gain does not carry over.
- **Not decided here:** automatic aperture placement from the CAD, and whether FARIS ships the patched OpenMC.

## Protocol 2 results (2026-10-12; protocol 4dd53a6; `references/cad-transport-s0f2.json`)

All 8 runs completed (job time 3,059 s of 4,500). One run was contended: F4 seed 1, with an end load of 6.81. Other
sessions were building, and loads ran higher than in protocol 1 (end loads 3.3–6.8).

| Run | Gain vs D2 | Mean of (a) | Variance per history | VOV | T (s) | Top batch's share |
| --- | --- | --- | --- | --- | --- | --- |
| F0 seed 1 | 55.6 | 0.985e-3 | 5,952 | 0.08 | 273 | 22 % |
| F0 seed 2 | 40.3 | 1.064e-3 | 6,208 | 0.05 | 361 | 14 % |
| F0 seed 3 | 26.1 | 1.086e-3 | 11,342 | 0.11 | 306 | 22 % |
| F0 seed 4 | 41.5 | 1.046e-3 | 8,049 | 0.09 | 271 | 17 % |
| F4 seed 1 (contended) | 39.1 | 1.091e-3 | 4,813 | 0.37 | 481 | 60 % (802) |
| F4 seed 2 | 128.8 | 1.052e-3 | 1,674 | 0.02 | 419 | 11 % |
| F4 seed 3 | 106.7 | 1.107e-3 | 1,961 | 0.07 | 432 | 24 % |
| F4 seed 4 | 133.6 | 1.005e-3 | 1,369 | 0.01 | 494 | 7 % |

- **F0, pooled.** G = 37.3, L = 26.7 (seed bound; the bootstrap gives 31.5). Mean 1.045e-3, z = −0.34 against D2, so
  unbiased. Pooled VOV 0.026.
- **F4, pooled.** G = 78.8, L = 50.8 (seed bound; the bootstrap gives 51.2). Mean 1.064e-3, z = −0.10, so unbiased.
  Pooled VOV 0.097, below 0.1: the tail is resolved.
- **H8: rejected.** The paired per-seed FOM ratios F4/F0 are 0.70, 3.20, 4.09 and 3.22. The pooled ratio is 2.11, above
  1.2, but seed 1 is below 1.
- **NUC-050: 10–100.** The target is kept; trace F4's top batches for the class that remains.
- **Where F4's gain goes.**
  - Pooled variance per history is 3.2× lower than F0's (2,512 against 8,005).
  - F4 is 1.5× slower per history (1,827 s against 1,211 s over the four runs), which costs a third of that. The
    slowdown is partly load (F4's runs ended at loads of 5.3–6.8, F0's at 3.3–5.9). The rest is aiming work and
    the window games skipped on aimed particles.
  - In seed 1, one batch (802) holds 60 % of F4's variance. Without it that run would sit with the other three.
- **F0 differs between protocols.** The same configuration pooled to 13.3 on protocol 1's seeds and 37.3 here. Seed 3
  of protocol 1 carried a weight-1,000 event, and nothing that large occurred in these 12 M histories. Gains on RM-M
  are only as good as the seed count behind them.

### 11. The remaining spikes enter the duct inlet off the exit aperture; a second, one-sided rectangle (diagnostic, after protocol 2)

- **Traces** (patched build, tracks; scripts `replay/raw_top.py`). Read with h5py because the large histories lack
  `offsets` (fact 7).
  - F4 seed 1, batch 802: history 398 holds all of the batch's box score (538,739 track states).
    1. Its source neutron hits the outboard tungsten (φ 37°).
    2. Wall collisions roulette it up in steps to a weight of 789 (1.1 → 317 → 3,972 → … → 789).
    3. It leaves the inboard tungsten (R 218) at 0.40 MeV and enters the duct inlet (x 446, y −4.6).
    4. Its direction passes the exit plane at y ≈ 49 cm, outside the exit rectangle, so it is neither aimed nor
       weighted down. It hits the duct's side wall (FLiBe, x 496, y 15), is split 1,000 times, and the copies reach
       tf_coil_00's side face.
  - F4 seed 3, batch 942: history 167 holds 99 %. The same path at weight 25: from the inboard wall (R 213), across
    the inlet (y −15) to the far duct wall (x 529, y 15).
- **The patch now takes several rectangles.** `OPENMC_APERTURE_BIAS` accepts further rectangles after `;`. The mixture
  picks one with equal probability, and its density is the mean of the rectangles' densities. With one rectangle the
  random-number stream is unchanged: F4 seed 1 reproduces to 1.7e-18 per batch, thread-order rounding.
  - **F5** adds the duct inlet: x 446 cm, with the duct's 15 cm half-widths.
- **One-sided rectangles.** `OPENMC_APERTURE_ONE_SIDED=1` aims at a rectangle only from points on the side opposite its
  normal u × v, which points toward the response. Where no rectangle is visible, β is 0.
  - Without it, collisions past the aperture (in the coil, insulation and lip) aim back through it. That is wasted
    work: a paired 300-batch timing on RM-M gave F0 35 and 26 s, F1 31 and 30 s, F5 50 and 44 s, and F5 aiming only
    in materials 1, 2, 3 and 7 33 and 38 s.
  - **F6** is F5 with one-sided rectangles. With the variable unset the build reproduces F5 (2.6e-18 per batch).
- **Unbiased on the duct toy** (duct2; 8 seeds each, mean ± standard error; analog references from facts 7 and 10):

  | Tally | Configuration (all with windows, skip-void, aiming β 0.2 in every material) | Mean | z | Mean FOM |
  | --- | --- | --- | --- | --- |
  | detector | no aiming (stock windows) | 1.5140e-2 ± 6.2e-5 | +0.32 | 220 |
  | detector | exit rectangle | 1.5101e-2 ± 1.10e-4 | +0.12 | 157 |
  | detector | exit + inlet | 1.5327e-2 ± 5.6e-5 | +1.26 | 140 |
  | detector | exit + inlet, one-sided | 1.5111e-2 ± 7.1e-5 | +0.18 | 150 |
  | duct band | exit + inlet | 9.018e-4 ± 3.41e-5 | +1.10 | 7.8 |
  | duct band | exit + inlet, one-sided | 8.130e-4 ± 2.14e-5 | −1.33 | 15.0 |

  - The analog references are detector 1.5074e-2 ± 1.92e-4 and duct band 8.558e-4 ± 2.41e-5. Single-rectangle
    aiming gave a duct-band FOM of 6.6 (fact 10).
  - Aiming spends effort on the aimed response: the detector, which is not behind an aperture, loses about a third of
    its FOM.
- **Hazard: aiming without windows.** Analog transport with aiming in every material gave a detector flux of
  9.79e-3 ± 1.58e-3, z −3.3, with per-seed relative errors up to 0.47. Every un-aimed collision multiplies the weight
  by up to 1/(1 − β), so after many collisions in iron the weight distribution is heavy-tailed and its mean is
  under-sampled. Fact 10's analog check aimed in one material only. No configuration here runs without windows. The
  patch should refuse collision aiming without windows (open).
- **RM-M, seed 77200001** (protocol 2's spike seed, already looked at), 3,000 × 1,000 histories:

  | Scan | Gain vs D2 | Variance per history | Top batch's share | VOV | T (s) | End load |
  | --- | --- | --- | --- | --- | --- | --- |
  | F0 (protocol 2) | 55.6 | 5,952 | 22 % | 0.08 | 273 | 3.3 |
  | F4 (protocol 2) | 39.1 | 4,813 | 60 % (802) | 0.37 | 481 | 6.8 |
  | F5 | 78.2 | 2,088 | 27 % (22) | 0.09 | 554 | 5.3 |
  | F6 | 100.0 | 1,889 | 24 % (22) | 0.09 | 479 | 5.7 |

  Batch 802's event is gone in F5 and F6. One seed, already looked at: not a result.
- **The patch** in `integrations/openmc/patches/faris-openmc-0.15.3-aim.patch` is now sha256 3f1fcd7e…, and its
  `libopenmc.so` is 5caefac2…. Protocol 2's version (1c3d82df…) is at commit 4dd53a6.

## Protocol 3 (frozen 2026-10-12, before any protocol 3 run; about 55 min, within my one-hour latitude)

**Question.** Does F6 (fact 11) reach NUC-050's 100× on RM-M against S0b's D2, on new seeds? Does it beat the current
windows?

**Configurations.** Same base model (sha256 c0c107f1f79c06d6…), copied to `~/.cache/avila-night/cad-risk-tests/s0f3/`.
- **F0:** as before.
- **F6:** the patched 0.15.3 (`libopenmc.so` 5caefac2…, patch 3f1fcd7e…) through the shadow package. It sets
  `OPENMC_WW_SKIP_VOID=1`, `OPENMC_APERTURE_BIAS="0.1 578 0 0 0 22 0 0 0 22; 446 0 0 0 15 0 0 0 15"`,
  `OPENMC_APERTURE_ONE_SIDED=1` and `OPENMC_COLLISION_AIM="0.2 *"`, and no other patch variable
  (`s0f_common.CONFIG_ENV`).

**Runs.** As protocol 2, with seeds 77300001–77300004 and F0 then F6 for each seed. Driver `s0f_scripts/run3.sh`;
output `~/.cache/avila-night/cad-risk-tests/s0f3/`.

**Analysis.** `s0f_analyse.py --pair F0 F6 --seed-set 3`, with protocol 2's statistics.
- **H9 (F6 beats F0):** H7's rule.
- **NUC-050 band for F6:** protocol 2's bands (`nuc050_outcome3`).
- **Not decided here:** finding apertures automatically from the CAD, refusing aiming without windows, and whether
  FARIS ships the patched OpenMC.

## Protocol 3 results (2026-10-12; protocol a2264c9; `references/cad-transport-s0f3.json`)

All 8 runs completed (job time 2,732 s of 4,500). None was contended (end loads 4.1–5.3).

| Run | Gain vs D2 | Mean of (a) | Variance per history | VOV | T (s) | Top batch's share |
| --- | --- | --- | --- | --- | --- | --- |
| F0 seed 1 | 26.1 | 1.065e-3 | 10,044 | 0.09 | 345 | 16 % |
| F0 seed 2 | 0.8 | 1.707e-3 | 375,959 | 0.96 | 315 | 98 % (1394) |
| F0 seed 3 | 32.6 | 1.073e-3 | 10,185 | 0.23 | 272 | 45 % |
| F0 seed 4 | 23.8 | 0.998e-3 | 14,086 | 0.29 | 270 | 50 % |
| F6 seed 1 | 90.0 | 1.149e-3 | 2,648 | 0.16 | 379 | 37 % (423) |
| F6 seed 2 | 99.1 | 1.074e-3 | 2,451 | 0.19 | 372 | 41 % (1093) |
| F6 seed 3 | 172.5 | 1.040e-3 | 1,431 | 0.02 | 366 | 8 % |
| F6 seed 4 | 208.5 | 1.031e-3 | 1,104 | 0.02 | 393 | 13 % |

- **F0, pooled.** G = 1.6, L = 0.8 (bootstrap; the seed bound is 4.5). Mean 1.211e-3, z = +0.83. Pooled VOV 0.90.
  - Seed 2's batch 1394 holds 98 % of the variance and scores about 600 times the mean. Its variance per history
    (375,959) leaves the run below the analog D2 (gain 0.8).
- **F6, pooled.** G = 121.9, L = 74.9 (seed bound; the bootstrap gives 90.9). Mean 1.073e-3, z = +0.04, so unbiased.
  Pooled VOV 0.043: the tail is resolved.
- **H9: confirmed.** The paired per-seed FOM ratios F6/F0 are 3.4, 130, 5.3 and 8.8, and the pooled ratio is 78.
- **NUC-050: ≥ 100 at the point estimate only.** More replicates are needed before any claim.
- **Cost.** F6 takes 1.26× F0's time per history (1,511 s against 1,202 s), down from F4's 1.5×. Its variance per
  history is 1,964, 98× below F0's pooled value on these seeds and 4.1× below F0's protocol 2 value.
- **Across protocols** (F0 on 12 seeds): F0 pooled 13.3, 37.3 and 1.6. Each time, one or two rare events set its
  gain. F6's per-seed gains run from 90 to 209, and its worst batch holds 41 %.

### 12. After protocol 3: a windows guard, and line-of-sight windows (diagnostic)

- **Guard.** The patch now refuses `OPENMC_COLLISION_AIM` without weight windows ("OPENMC_COLLISION_AIM needs weight
  windows"; fact 11's hazard). `libopenmc.so` 2d25e1cd…. Configured runs behave as before.
- **F6's two largest remaining spikes** (protocol 3 seeds 1 and 2, replayed with tracks):
  - Seed 1, batch 423 (37 %). Wall roulette lifts a source neutron to weight 131 on the inboard tungsten (R 214).
    Aiming works: the neutron goes through the exit rectangle at weight 1.23. But the coil face's window is about
    1/1,000 of that, so it is split 1,000 times on arrival. The windows rate the wall about 10⁶ times less important
    than the coil face. The line of sight through the duct alone makes it about 10³.
  - Seed 2, batch 1093 (41 %). An aperture-biased source neutron (weight 0.021) enters the duct inlet, collides in the
    duct-wall FLiBe at 13.8 MeV, and is rouletted up to 0.18, then 1.3. The survivor's copies reach the coil face.
- **Line-of-sight windows (F7; `risk_tests/s0f_los_windows.py`).** Each voxel's lower bound is capped at
  w_ap(g) · 4π / (Ω · T), never raised.
  - Ω is the exact solid angle of each one-sided rectangle from the voxel centre.
  - T is the uncollided transmission (σ = 0.15/cm), averaged over rays to a 3 × 3 grid on the rectangle and
    ray-marched through the model with `openmc.lib.find_material` in 5 cm steps.
  - w_ap is the lowest lower bound just beyond the rectangle.
  - Upper bounds keep their ratio to the lower.
  - A first version marched only to the rectangle's centre. From oblique wall points that ray misses the inlet and
    crosses the port wall, so it lowered almost nothing at the spike origins.
  - Result: 37,704 of 838,240 positive bounds lowered, median factor 7.1 (99th percentile 78). Generation took 23 min,
    single-threaded. Output model sha256 cf6b14a5af59ab4e….
  - At the spike origins: batch 423's voxel ×27, batch 802's ×4.6. Batch 942's and the duct-wall FLiBe are
    unchanged. The inlet's own window is high, so its cap does not bite: the adjoint under-rates the duct too.
- **F7 on protocol 3 seeds** (diagnostics; the seeds have been looked at):

  | Seed | F6 gain (protocol 3) | F7 gain | F6 → F7 variance per history | F7 top batch | F7 T (s), end load |
  | --- | --- | --- | --- | --- | --- |
  | 77300001 | 90.0 | 126.4 | 2,648 → 1,592 | 22 % (2865) | 449, 5.1 |
  | 77300002 | 99.1 | 83.9 | 2,451 → 2,417 | 39 % (1093) | 446, 6.3 |

  - Batch 423's class is gone. Batch 1093's is untouched.
  - F7 ran about 15 % slower, partly from the higher load.
  - Not adopted. The duct-wall class needs the response itself (the coil face) as a line-of-sight target, not only
    the apertures.

### 13. Windows capped by reciprocity from the response outward (F8; diagnostic)

Basis: `CAD_TRANSPORT_STREAMING_LITERATURE.md` and its amendment. The aiming needs windows that deliver particles to
the targets at an appropriate weight; fact 12 measured them about 1,000 times too heavy on the inboard wall.

- **Method** (`risk_tests/s0f_los_windows.py`, rewritten). For each target rectangle, an analog OpenMC run with a
  cosine-law surface source on the target, emitting backward, tallies the uncollided flux φ on the window mesh
  (collision filter 0, the 10 groups above the 0.1 MeV cut-off). By reciprocity, a neutron of that group emitted
  isotropically in the voxel crosses the target uncollided with probability P = φ·A/4.
  - Each lower bound is capped at w_t(g)/P, where w_t is the lowest lower bound just beyond the target. It is never
    raised; bins with a relative error above 0.5 are left alone; upper bounds keep their ratio.
  - Targets run from the response outward, each reading w_t from the bounds as already capped: the two port-coil
    faces (planes y = ±20 cm, x 581–645, |z| < 30), the duct exit, the duct inlet.
  - **Check.** In void voxels facing the inlet, P is 55e-4 and 22.5e-4 against exact Ω/4π of 54.8e-4 and 22.2e-4.
  - **Cost.** 4 runs of 2 M histories: 4.4 min in all on 4 threads. Fact 12's ray march took 23 min.
  - **Result.** 23,092 of 838,240 positive bounds lowered, median factor 6.8, 99th percentile 155. At the traced
    spike origins: batch 423's voxel ×114, batch 802's ×7.6, batch 1093's duct-wall voxel ×3.7; batch 942's
    unchanged. Model sha256 ad135bd4….
- **F8 = F6's variables on these windows.** Protocol 3 seeds, already looked at, 3,000 × 1,000 histories:

  | Seed | F6 gain (protocol 3) | F7 gain | F8 gain | F8 variance per history | F8 top batch | F8 VOV | F8 T (s), end load |
  | --- | --- | --- | --- | --- | --- | --- | --- |
  | 77300001 | 90.0 | 126.4 | 222.3 | 1,668 | 21 % | 0.057 | 244, 3.7 |
  | 77300002 | 99.1 | 83.9 | 250.1 | 1,510 | 17 % | 0.039 | 239, 3.7 |

  - F8 ran at about 12,400 histories/s, against F6's 7,900–8,100 in protocol 3 (end loads 4.3–4.5 there).
  - Two seeds, already looked at: not a result.
- The targets are still placed by hand.

## Protocol 4 (frozen 2026-10-12, before any protocol 4 run; about 40 min, within my one-hour latitude)

**Question.** Does F8 reach NUC-050's 100× on RM-M against S0b's D2, on new seeds? Does it beat the current windows?

**Configurations.**
- **F0:** as before, on `base-model.xml` (sha256 c0c107f1f79c06d6…).
- **F8:** the patched 0.15.3 (`libopenmc.so` 2d25e1cd…, patch 20237c38…) with F6's variables (protocol 3), on
  `base-model-f8.xml` (sha256 ad135bd4…): the same model with fact 13's capped windows.
- Both files are in `~/.cache/avila-night/cad-risk-tests/s0f4/`.

**Runs.** As protocol 3, with seeds 77400001–77400004 and F0 then F8 for each seed. Driver `s0f_scripts/run4.sh`.
T excludes window generation for both configurations; F8's extra 4.4 min is reported beside the result.

**Analysis.** `s0f_analyse.py --pair F0 F8 --seed-set 4`, with protocol 2's statistics.
- **H10 (F8 beats F0):** H7's rule.
- **NUC-050 band for F8:** protocol 2's bands (`nuc050_outcome4`).
- **Not decided here:** NUC-050's basis (literature note, section 1), finding targets from the CAD, and whether
  FARIS ships the patched OpenMC.

## Protocol 4 results (2026-10-12; protocol 75add3d; `references/cad-transport-s0f4.json`)

All 8 runs completed (job time about 1,060 s of 4,500). None was contended (end loads 2.7–3.5).

| Run | Gain vs D2 | Mean of (a) | Variance per history | VOV | T (s) | Top batch's share |
| --- | --- | --- | --- | --- | --- | --- |
| F0 seed 1 | 59.9 | 1.153e-3 | 8,531 | 0.06 | 177 | 14 % |
| F0 seed 2 | 101.4 | 1.021e-3 | 8,266 | 0.20 | 108 | 41 % |
| F0 seed 3 | 63.4 | 1.122e-3 | 13,626 | 0.21 | 105 | 41 % |
| F0 seed 4 | 122.0 | 1.081e-3 | 7,143 | 0.06 | 104 | 17 % |
| F8 seed 1 | 434.4 | 1.090e-3 | 1,483 | 0.05 | 140 | 22 % |
| F8 seed 2 | 284.1 | 1.066e-3 | 2,195 | 0.08 | 145 | 19 % |
| F8 seed 3 | 235.4 | 1.097e-3 | 2,740 | 0.15 | 140 | 34 % (44) |
| F8 seed 4 | 573.2 | 1.057e-3 | 1,143 | 0.01 | 138 | 4 % |

- **F0, pooled.** G = 77.3, L = 51.2 (seed bound; the bootstrap gives 60.5). Mean 1.094e-3, z = +0.31. VOV 0.042.
- **F8, pooled.** G = 337.6, L = 201.6 (seed bound; the bootstrap gives 262.8). Mean 1.078e-3, z = +0.10, so
  unbiased. Pooled VOV 0.029.
- **H10: confirmed.** Paired per-seed FOM ratios F8/F0: 7.2, 2.8, 3.7 and 4.7; pooled 4.4.
- **NUC-050: ≥ 100 at the 95 % lower bound**, by the frozen rule: met on RM-M with hand-placed targets.
- **The machine was faster in this protocol, and that inflates every gain against D2.**
  - F0 took 104–177 s per run here, against 270–361 s in protocols 2 and 3. The other sessions' builds had ended.
  - D2's FOM was timed once, in S0b. A gain against it mixes the method with the machine's state on the day. F0's
    pooled gain was 13.3, 37.3, 1.6 and 77.3 in the four protocols; the last value owes about a factor of 2.4 to
    speed.
  - The quantities that do not depend on the machine:
    - Variance per history: F8 1,901, F0 9,491 here (5.0× lower). F6 had 1,964 in protocol 3, the same within
      noise.
    - Time per history relative to F0 in the same protocol: F8 1.14×, F6 1.26×, F4 1.5×.
  - So F8's step over F6 is in time per history, not in variance. The 222× and 250× of fact 13 against F6's 90× and
    99× were mostly the faster machine.
  - At protocol 3's machine speed, F8's pooled gain would be about 337.6 / 2.4 ≈ 140. That is an estimate, not a
    measurement.
- **Cost not in T.** F8's windows need 4.4 min of reciprocity runs on top of the adjoint that F0 also needs.

### 14. FARIS finds the targets itself (F9; diagnostic, used seeds)

F8's four targets were placed by hand from the drawing. `s0f_find_targets.py` now finds them from the model alone.

- **Method.**
  - One analog run of the model's own source, 10 M histories, tallying uncollided flux above 10 MeV on a 10 cm
    mesh, for all materials and for the response material. About 1.7 min.
  - Response faces: the lit voxels of the response material, clustered; the thin axis and the lit side give each
    face's plane and normal.
  - For each face, a reciprocity run from the face (2 M histories). The channel is where forward flux times reverse
    reach probability is within 2 % of its slice maximum.
  - Walking from the face toward the source, slices below 1 % of the largest slice sum are dropped as noise. The
    duct is the longest run whose channel area stays within 1.4× the smallest. Its ends are the exit and the inlet.
  - Faces that share a duct give the same exit and inlet twice; these are merged.
- **What it found on RM-M, against the hand-placed targets.**

| Target | Found | Hand-placed |
| --- | --- | --- |
| Coil face, −y | centre (615, −20, −5), 35 × 25 half-edges | (613, −20, 0), 32 × 30 |
| Coil face, +y | centre (625, 20, 0), 25 × 30 | (613, 20, 0), 32 × 30 |
| Duct exit | x = 590, centre (0, 0), ±20 × ±20 | x = 577.5, ±22 × ±22 |
| Duct inlet | x = 435, centre (5, 0), ±25 × ±20 | x = 445.5, ±15 × ±15 |

- **Result on two used seeds (protocol 3's seeds 1 and 2; not a protocol).**

| Run | Mean of (a) | Variance per history | VOV | T (s) | FOM |
| --- | --- | --- | --- | --- | --- |
| F9 seed 1 | 1.097e-3 | 1,663 | 0.06 | 153 | 11.8 |
| F9 seed 2 | 1.075e-3 | 1,455 | 0.04 | 158 | 13.0 |
| F8 seed 1, rerun in the same hour | 1.100e-3 | 1,668 | 0.06 | 168 | 10.7 |

  - F8 had 1,668 and 1,510 on these seeds in fact 13. So the automatic targets give the same variance per history
    as the hand-placed ones, and no visible bias.
  - Gains against D2 are not quoted: they depend on the machine's speed on the day (protocol 4 results).
- **Cost.** About 2 min to find the targets and 2.8 min for the capped windows, on top of the adjoint.
- **Limits.**
  - Rectangles are axis-aligned on a 10 cm mesh. A duct at an angle would get a loose bounding rectangle.
  - Only 17 response voxels were lit at 10 M histories. A dimmer face would need more histories.
  - One duct, one geometry. Whether it carries to a second geometry is the next test.
- **Records.** `~/.cache/avila-night/s0f-diag/auto/`: `targets.json`, `model-f9.xml` (sha256 62464ca5), runs
  `F9-77300001`, `F9-77300002`, `F8-77300001`.

### 15. A second geometry: the targets are found and the method carries over (diagnostic)

The test set before building fact 14: FARIS must find the targets on a geometry it was not developed on, and the
method must help there. If not, stop and write up.

- **The model** (`s0f_second_model.py`). Made to differ from RM-M:
  - Constructive solid geometry, not CAD. A 300 cm void room inside TiH2.
  - Source: 14.1 MeV, isotropic, uniform in a 200 cm box. Not a plasma.
  - One duct, 16 cm × 24 cm, along +y through a 200 cm wall, 20 cm off the source's centre line in x and z.
  - Response: flux above 0.1 MeV in a slab that faces the duct, 50 cm beyond the wall. In RM-M the response lies
    beside the duct.
- **Targets found, unaided, in 28 s.** One code change was made first, from reading the code and not from a run:
  the rectangle's edges are ordered so that u × v points toward the response, which the one-sided rule needs for a
  duct along y.

| Target | Found | True |
| --- | --- | --- |
| Response face | y = 400, centre (20, −20), ±20 × ±20 | y = 400; the duct's projection is centred (20, −20) |
| Exit | y = 385, centre (20, −20), ±20 × ±20 | duct exit at y = 350, 16 × 24 |
| Inlet | y = 125, centre (20, −20), ±10 × ±20 | duct inlet at y = 150, 16 × 24 |

  - The exit sits in the void between the wall and the slab, and the inlet 25 cm inside the room. Both are on the
    duct's axis. The duct run is cut by channel area, so it extends into void where the beam is still narrow.
- **Windows.** This model has no adjoint pipeline, so its windows are not FARIS's.
  - OpenMC's MAGIC generator gave global windows spanning 10⁻¹⁹ to 0.5. A run with them alone did 410 of 3,000
    batches in about 9 min, against 15 s for the whole analog run. It was stopped. No windows-only baseline exists
    for this model.
  - Used instead: a flat window (0.2 to 1.0) everywhere, set to three times the source's uncollided reach
    probability (8.1e-6) in the mesh layer holding the slab, then capped along the line of sight by
    `s0f_los_windows.py` from the found targets. 7,488 of 79,200 bounds lowered.
- **Result, 3 M histories per run, seeds 78100001 onward.**

| Configuration | Runs | Mean | Variance per history | T per run (s) | Pooled FOM |
| --- | --- | --- | --- | --- | --- |
| Analog | 12 | 2.687e-4 ± 0.169e-4 | 141,350 | 15.4 | 1.37 |
| Automatic (B9) | 4 | 2.636e-4 ± 0.058e-4 | 5,574 | 19.9 | 26.1 |

  - Pooled FOM ratio 19. Variance per history 25× lower; time per history 1.3× higher.
  - z = −0.28 against the analog mean, so no visible bias.
  - Per-seed FOM of B9: 47, 18, 21 and 42. Two of the four runs have VOV above 0.1 (0.13 and 0.17), so the tail is
    not fully resolved in single runs.
- **What this does and does not show.**
  - The target finder carried over without tuning, and aiming with line-of-sight windows gave a large gain on a
    second geometry.
  - It is not a protocol: seeds were not frozen ahead and there is one model.
  - It does not test FARIS's adjoint windows on this model, a bent duct, or a duct at an angle to the mesh.
  - A bent duct has no uncollided path from source to response. The finder would return no face. That limit is
    known, not measured.
- **Records.** `~/.cache/avila-night/s0f-diag/second/`: `analog.xml` (sha256 503d7735), `windows-flat-capped.xml`
  (0f507097), `auto/targets.json`, runs `analog-781000NN` and `autoflat-781000NN`.

## Protocol 5 (frozen 2026-10-12, before any protocol 5 run; about 35 min, within my one-hour latitude)

**Question.** On new seeds, do targets found automatically (F9) do as well as the hand-placed ones (F8)? Does F9
reach NUC-050's 100× on RM-M?

**Configurations.** All files in `~/.cache/avila-night/cad-risk-tests/s0f5/`.
- **F0:** as before, on `base-model.xml` (sha256 c0c107f1f79c06d6…).
- **F8:** as protocol 4, on `base-model-f8.xml` (ad135bd4…).
- **F9:** the patched 0.15.3 (`libopenmc.so` 2d25e1cd…) with F6's variables, except the rectangles are fact 14's
  (`s0f_common.AUTO_APERTURE`), on `base-model-f9.xml` (62464ca5…): windows capped from the found targets.

**Runs.** As protocol 4, with seeds 77500001–77500004 and F0, F8, then F9 for each seed. Driver
`s0f_scripts/run5.sh`. Time box 90 min of job time.

**Analysis.** `s0f_analyse.py --pair F0 F9 --seed-set 5` and `--pair F8 F9 --seed-set 6`, protocol 2's statistics.
- **H11 (F9 beats F0):** H7's rule.
- **H12 (automatic as good as hand-placed):** confirmed if the pooled FOM ratio F9/F8 is at least 0.8.
- **NUC-050 band for F9:** protocol 2's bands (`nuc050_outcome5`). D2 was timed once, so the gain against it moves
  with the machine's speed (protocol 4 results). F0's gain on the same seeds, variance per history and time per
  history are reported beside it.
- **Not decided here:** NUC-050's basis, and whether FARIS ships the patched OpenMC.

## Records

`~/.cache/avila-night/s0f-diag/`:

- Each pilot has its folder with `record.json` and its last statepoint.
- Window files (sha256 prefix):
  - `e2p_filled.h5` 4cd6c771
  - `fr_filled.h5` 27d817f7
  - `frp_filled.h5` 764effb5
  - `frp_b10.h5` 337c50dd
  - `frp_b100.h5` 10e53e55
- Scripts: `lookup.py` (birth windows), `ww_outliers.py`, `uncollided.py`, `readmap.py`, `boost.py`, and the
  `repro/` reproducer.
