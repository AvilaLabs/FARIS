# CAD to answer: the open-pipeline baseline (UX-079)

Status: protocol frozen 2026-10-11, before any baseline run. Results are appended in a section below; this protocol is
never edited. A rule changed after the first run is an amendment with a date and reason.

[UX-079](../requirements/09-usability.md) asks FARIS to take an engineering STEP model to a checked magnet answer
with no steps outside FARIS, in ≤ 30 min of machine time on RL, and to report that beside a measured baseline: the same
model taken through the open pipeline a skilled user would assemble today. This note defines that baseline. It
measures machine time and lists manual steps; it does not measure a person's time (see "Not measured").

## What the open pipeline is

As of 2026-10-11 the current open route from STEP to a weight-windowed OpenMC answer is:

1. **Conversion:** `cad_to_dagmc` 0.14.3 (released 2026-10-10), whose default meshing backend has been
   `cad-to-dagmc-mesher` since 0.13.1; 0.14.3 pins the same mesher (0.3.3) and cadquery-ocp (7.9.3.1.1) as the
   0.14.2 that FARIS's CAD helper runs today. 0.14.3's changes are documentation and test cleanup.
2. **Model:** OpenMC with DAGMC: materials matched to the tags, the plasma source, the response tally.
3. **Weight windows:** OpenMC's random-ray FW-CADIS. Since 0.16.0 it can aim at a response (targeted FW-CADIS or a
   user adjoint source, CADIS). S0d measured CADIS as the best of these on RM-M.
4. **Production:** a run with the windows to the target error.

FARIS today runs this same chain, partly by hand: its CAD helper calls `cad_to_dagmc` 0.14.2, and the weight-window
work of S0b to S0d used hand-written scripts. So the machine time of the open pipeline and of FARIS today are the same
chain; UX-079 asks FARIS to beat it with the native import pipeline and better windows, and to remove the manual steps.

## Already measured (no new runs)

From existing records on RM-M, a laptop shared with other jobs, OpenMC on 4 pinned cores:

| Stage | Wall time | Record |
| --- | --- | --- |
| Conversion, 4 imprint threads, overlap scan off | 869 s and 961 s (load 10–13) | `DESIGN_FILE_FORMAT.md`, RM-M measured |
| Conversion, 1 imprint thread | about 2,500 s | same (not a recorded run with load) |
| Multigroup library for random ray | 203 s | S0b G6 `generate.json`, step `mgxs` |
| CADIS window generation | 256 s | S0d T2 |
| Production to R(a) = 5 % at T2's figure of merit (0.652 s⁻¹) | 613 s, computed as 1 / (FOM × 0.05²) | S0d T2 |

Sum with the tuned conversion: about 1,941 s (32 min), and only after the windows were known to work. The production
time at a stated error is computed from a measured figure of merit, not timed.

## New runs

Two conversions of RM-M (`~/.cache/avila-night/cad-risk-tests/models/rm-m/rm_m.step`), in a separate environment
(`~/.cache/avila-night/envs/c2d-0143`, `cad_to_dagmc` 0.14.3, Python 3.14), under `MemoryMax=6G`, pinned to cores 0–3:

- **B1, as documented.** `CadToDagmc().add_stp_file(step, scale_factor=0.1, material_tags=<one tag per solid from
  rm_m_roles.json>)`, then `export_dagmc_h5m_file(...)` with every argument at its default except the tolerances, which
  are FARIS's (chord 0.1 cm, angular 0.1 rad), so the mesh quality matches B2. Default imprint (`True`) and default
  overlap scan.
- **B2, as FARIS runs it.** The FARIS CAD helper's `convert` command with its defaults (4 imprint threads, overlap
  scan off, same tolerances).

**Conditions.** Each run starts only when the 1-minute load average is below 2.0 and no cargo build holds the FARIS
build lock; load average is recorded at start and end. B2 runs first. Wall-clock limit 45 min for B1 and 25 min for
B2; a run that hits its limit is reported as "more than" the limit. **The new runs are capped at 70 minutes of machine
time in total.**

**Validity.** A conversion counts only if its h5m loads in OpenMC 0.15.3 with DAGMC and 10,000 analog histories
of the RM-M source lose 0 particles. A conversion that fails is reported with its error and time; it is not retried
with other settings within this protocol.

**Reported, per run:** wall time, peak memory, triangle count, the h5m's sha256, load at start and end, and the
validity result.

## Manual steps

The steps a user takes outside any single tool, listed from what the RM-M work actually needed. Each names the file
that did it in FARIS's risk tests, and the workaround records it produced. This list is the baseline against which
UX-079's in-app decisions are counted; it is a lower bound, because each step was done by someone who already knew the
answer.

1. Build two environments: a CAD one (cadquery, `cad_to_dagmc`) and an OpenMC one with DAGMC; for targeted windows,
   a second OpenMC (0.16.0) beside the 0.15.3 production build.
2. Map solids to material tags and fit the tag to `cad_to_dagmc`'s 28 usable characters (`DESIGN_FILE_FORMAT.md`).
3. Choose conversion tolerances and imprint threads, and turn off the overlap scan, which spends its 60 s budget on
   30 solids (`faris_cad.py convert`).
4. Write materials whose names match the tags (`rm_m_openmc.py`), including a real material for void volumes, which
   random ray requires (G6 workaround: void tagged as filler, H-1 at 0.001 g/cm³).
5. Write the plasma source and the response tally with its energy cut (`rm_m_openmc.py`, `r1_configs.py`).
6. Give the bounding surface an explicit id, and wrap the DAGMC universe for `source_region_meshes`
   (G6 workarounds).
7. Generate the multigroup library on 0.15.3, because 0.16.0's stochastic-slab model ignores the cross-section path
   and gives a negative total cross section in a low-density material (OPENMC-001, OPENMC-003 in `docs/defects`).
8. Convert materials to macroscopic entries by hand, because 0.16.0 renames them (`s0d_generate.py`).
9. Write the random-ray settings and the CADIS adjoint source: mesh, rays, batches, a discrete-energy source
   constrained to the plasma cell (`s0d_generate.py`, `s0d_common.py`).
10. Run production with the windows, a split limit and a stop on target error (`s0b_run.py`).
11. Check the windowed answer against an analog run before trusting it (S0b D2, S0d's z-scores).

## Rules

- The baseline machine time is B2's conversion plus the multigroup, window-generation and production rows above
  (the tuned route a skilled user reaches); B1's is reported beside it as the documented-defaults route.
- UX-079's comparison reports FARIS's measured wall time and step count beside these, as two numbers each; no
  combined score.
- A B1 or B2 that fails validity leaves that route's baseline NOT_EVALUATED, with the error.

## Not measured

- **A person's time.** The UKAEA paper cited in UX-078 puts manual CAD preparation at about a month per concept; no
  published time exists for imported engineering CAD through the open pipeline. A timed session with an outside
  user who prepares models with Cubit or SpaceClaim would measure it. That needs an outside contact, which is Connor's
  decision.
- **Other converters** (GEOUNED, McCad, SuperMC, Attila4MC). They produce CSG for MCNP or need licences; a comparison is
  a separate protocol.

## Estimate and footprint

B2 about 15 min, B1 about 42 min if the 1-thread figure holds, plus 2 validity runs of about 2 min each: about 60 min,
under the 70 min cap. Cores 0–3, at most 6 GB (peak 4.3 GB recorded for conversion). Not run alongside a FARIS build or
a transport run.

## Outputs

`~/.cache/avila-night/cad-to-answer-baseline/` (logs, h5m files, `results.json`); hashes appended here with the
results.

## Results

Appended after runs.
