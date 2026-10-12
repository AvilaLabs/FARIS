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
