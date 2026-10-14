<!-- kind: explanation -->
# Streaming variance reduction: what the literature says (2026-10-12)

A literature pass made after S0f protocol 3 (`CAD_TRANSPORT_S0F.md`), to replace spike-by-spike fixes with a designed
method. Each item says how it was checked: **read** (the source text was opened and the quoted words found),
**abstract** (abstract only), or **unverified** (search snippet or secondary source).

## 1. NUC-050's 275× is a deep-penetration figure

- **Read (abstract, OSTI 1020795).** Ibrahim et al., "ITER neutronics modeling using hybrid Monte Carlo/deterministic
  and CAD-based Monte Carlo methods", Nuclear Technology 175 (2011). The 275× is for "the total nuclear heating in
  the inboard leg of the toroidal field coils". The same abstract gives 9× over traditional variance reduction and
  21,000× for dose outside the biological shield.
- RM-M's response (a) is dominated by the two port coils behind an open duct (S0e: 64 % of (a), about 97 % of its
  variance). That is a streaming response, not the inboard leg through the bulk shield.
- **Not found:** any published figure-of-merit gain for a port, duct or other streaming-path response in a fusion
  model, with FW-CADIS or any other method.
- **Open for Connor.** NUC-050's 100× borrows its basis from a different kind of problem. The literature gives no
  streaming figure to replace it. The requirement is unchanged until he decides.
- **Decided by Connor, 2026-10-13.** The requirement is split by kind of problem.
  - NUC-050 is the deep-penetration requirement: at least 275× on heating in RM-M's inboard toroidal-field leg, the
    response its source measured. This tightens the target from 100×; it is not yet measured.
  - NUC-055 carries the streaming target: at least 100× for the magnet response behind RM-M's open port, with
    targets FARIS finds itself. Its basis is stated as a FARIS choice.
  - The patched OpenMC is an optional accelerator. FARIS keeps working on stock OpenMC.
- **Corrected the same day, approved by Connor.** NUC-050 is 100× on fast-neutron flux in the inboard leg, which
  is what magnet lifetime rests on. The 275× on heating moves to a new requirement, NUC-078, at a later phase.
  Reasons: the addendum below (121 days of analog computing; 275× only after a gap was closed) and the pilot.

## 2. The failure seen in S0f is documented

- **Read.** Martz and Turner, LA-UR-17-22351 (MCNP6 long-history diagnostics): "a high weight particle entering a
  streaming path and arriving in a region of much lower expected weight". An improved window "may alleviate this
  effect. However, it is often impractical to obtain an ideal WW in a large and complex" model. The report says this
  of stochastic and deterministic windows alike.
- **Read (by the search agent).** Davis and Turner (CCFE), M&C 2011, MAGIC on the ITER reference model: long
  histories "hypothesised to be due to high weight source particles streaming along various gaps in the model, then
  interacting in a part of the model that has significantly lower weight bounds". Their global mean FOM was 74×
  analog for MAGIC mesh windows and 48× for FW-CADIS windows (a global metric, not a streaming response).
- Consequence: correcting the windows (S0f fact 12's F7) is the route these reports call impractical.

## 3. The designed remedy: nested DXTRAN

- **Read.** Martz, "Nested DXTRAN", LA-UR-09-05520:
  - "Weight Windows Alone are not Effective"; "Splitting does not solve the angle problem".
  - "Current DXTRAN is Only a Partial Solution"; "It is still possible to get large weights in the tally region."
  - "A second (outer) DXTRAN sphere surrounding the first ensures" that a particle colliding just outside the inner
    sphere has already had its weight reduced. Spheres are nested until the windows alone deliver particles near the
    outermost one with appropriate weight.
- **Read (by the search agent).** Kulesza, Fensin and Martz, LA-UR-18-27914: at each collision a DXTRAN particle is
  created on every sphere, with weight w · p(μ)/p_e(μ) · exp(−λ); demonstrated on a concrete duct (Booth et al. 2009,
  not opened).
- How S0f's aiming differs:
  - It aims a random fraction β of elastic collisions. DXTRAN creates a contribution at every collision, so no
    collision can send a full-weight particle at the target unreduced.
  - It has no attenuation factor: the aimed particle is transported normally and may collide on the way.
  - Its innermost target is the duct exit, not the response.
  - F5's inlet rectangle was a second nesting level, found by tracing (fact 11).
- **Abstract.** Huang et al., Nucl. Sci. Eng. 199 (2025), a DXTRAN-based weight-window generator in the MCS code.
  How it reconciles windows with DXTRAN was not read.
- **Unverified:** the MCNP manual's rules for windows on DXTRAN particles and the DXC and DD cards.

## 4. Other approaches

- **Adaptive multilevel splitting (TRIPOLI-4).** Read by the search agent: Louvin et al., M&C 2017, a seven-leg air
  labyrinth. The importance function "is only used to rank the particles", so a poor map costs efficiency, not
  correctness. No numeric gain is given, and OpenMC has no implementation.
- **CADIS-Ω (Munk, Slaybaugh et al.).** Summarised from the PHYSOR 2016 paper, not read directly: one labyrinth, where
  CADIS-Ω beat CADIS by about 28× and both were below analog. No duct results were found.
- **OpenMC.** No DXTRAN or point-detector feature in a release. Open pull requests seen by the search agent (bodies
  read by it, not by me): #4136 adjoint-driven source biasing, #3114 first-collision source for random ray, #4156
  weight-window lookup by direction of travel where mesh planes coincide with surfaces.
- **Reliability checks.** MCNP's practice is VOV below 0.1 and a history-score tail slope above 3 (secondary source).
  S0f uses the VOV check and replicate seeds; it has no slope test.

## 5. Decision

Build the aiming as nested DXTRAN (S0f fact 13 onward): a contribution at every collision with attenuation, and
the response as the innermost target. Do not pursue window corrections further.

### Amendment (2026-10-12, same day; reason: the decision above mis-read which half of the rule S0f was missing)

- S0f's β mixture already has DXTRAN's defining property: every direction through a target carries a reduced weight,
  aimed or not. A contribution at every collision would remove a 1-in-5 choice, a secondary gain.
- What S0f lacks is the other half of Martz's rule: windows that deliver particles near the outermost target "with
  appropriate weight". Measured (fact 12): inboard-wall windows at 0.5 MeV sit about 10⁶ above the coil face's, and
  the uncollided chance of reaching the coil is about 10⁻³, so an aimed wall neutron arrives about 1,000 times too
  heavy (weights 131 and 789 in the traced spikes).
- So the decision is: make the windows consistent with the aiming, with the response as the innermost target and
  the targets chained outward. Fact 12's F7 was an incomplete version of this (no response target, no chaining, a
  slow ray march). "Do not pursue window corrections further" is withdrawn.
- The every-collision contribution with attenuation stays as a possible later step.

## Addendum, 2026-10-13: what the 275× was, and the photon gap

Read after the inboard-leg pilot ([CAD_TRANSPORT_S0G_INBOARD_PILOT.md](CAD_TRANSPORT_S0G_INBOARD_PILOT.md)).

- **Read (slides, text extracted).** Ibrahim et al., ANS 2010 presentation of the same work
  (`fti.neep.wisc.edu/.../ibrahim_ans0410.pdf`). Its figure-of-merit table for inboard coil heating:

| Run | Time (days) | Max. uncertainty | FOM, first model | FOM, after fixing geometry |
| --- | --- | --- | --- | --- |
| Analog | 121.3 | 5.9 % | 1 | |
| MCNP weight-window generator | 11.0 | 3.6 % | 30 | |
| FW-CADIS | 0.8 | 4.5 % | 42 | 275 |

  - FW-CADIS first gave 42×. The slides say it was "controlled by occasional histories which took several
    minutes": a particle that "acquired high weight" in front of the vessel and "streams directly to TFC where
    weights are very low", through "a gap in VV".
  - That is the failure S0f traced on RM-M. The authors removed it by closing the air gap in the CAD model, and
    275× is the figure after that change. FARIS's aiming treats the gap instead of removing it.
  - The analog reference took 121 days of computing for 5.9 %. The uncertainty is the largest over ten segments
    of the leg. A like-for-like measurement is a cluster-scale job, not a laptop one.
  - Not read: whether the deterministic importance was coupled neutron-photon. The slides do not say.
- **Verified locally.** OpenMC 0.15.3 accepts photon weight windows, and its window generator takes
  `particle_type` photon (`openmc.WeightWindowGenerator`; `src/weight_windows.cpp` accepts neutron and photon).
  FARIS's adjoint comes from the multigroup random-ray solver, which is neutron-only, so it gives no photon
  importance.
- **Search results only, not read.** A TRIPOLI-4 paper on linking the photon importance map to photon production,
  because independently made neutron and photon maps needed hand adjustment; a coupled neutron-photon
  auto-importance method (arXiv 1603.01480). No source was found that quantifies heating gain under neutron-only
  windows.
- **Reading.**
  - The pilot's low heating gain has a standard cause and a standard remedy: photons need their own windows.
  - The only photon windows available to FARIS today are OpenMC's forward generator, which is global, and fact 15
    measured global forward windows as far slower than analog on a duct problem. A response-directed photon map
    needs a coupled adjoint that FARIS does not have.
  - The release's own question is magnet lifetime, which rests on fast-neutron fluence, not heating. On fast
    flux the pilot gives about 320× on the inboard leg and protocol 5 gives 376× behind the port.

