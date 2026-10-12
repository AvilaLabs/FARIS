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
