# Next FARIS release: plan

Status: approved 2026-10-09. The version number is decided at release.

## The question this release answers

**How does a design choice change a fusion plant's life?** That means
component replacements, downtime, tritium and net electricity over thirty years,
with honest uncertainty and checkable evidence, for the user's own design.

FARIS 0.2 answers it for one recorded ARC-inspired study. This release answers
it for any tokamak a user brings as CAD, entirely on their own machine with
open-source tools. Hosted compute is not part of it.

FARIS's distinct capability is the whole-life loop in which outage durations are
computed from activation. A literature and source check (2026-10-06, see
[MAINTENANCE_LITERATURE_2026-10.md](notes/MAINTENANCE_LITERATURE_2026-10.md))
found no published tool that does this. PROCESS uses a duration formula, and
bluemira and FUSE take fixed durations as inputs. FARIS's validation showed that
computed durations change downtime differences between designs by 2–14×
([MAINTENANCE_COUPLING_VALIDATION_RESULT.md](notes/MAINTENANCE_COUPLING_VALIDATION_RESULT.md)).
The release makes that capability usable on real designs and defensible against
expert review.

## Standard

"Best in class" is decided by the existing [requirements](requirements/README.md),
not by description. The release gate is a fixed list of requirement IDs that must
be **Met**, decided by checkers (QA-002, QA-070); see [Release gate](#release-gate).
Every other requirement keeps its honest current status, and every displayed
number carries its validation state (VAL-054). Where this plan needs a
requirement changed, it says so, and the change goes through the requirements'
change control before work depends on it.

## Scope

### 1. Bring your own design: CAD in, checked before transport

- **Input.** A STEP file plus a FARIS sidecar file, because STEP carries no
  physics. The sidecar holds:
  - units, and whether the model is a full 360 degrees or a sector;
  - for each solid: material, role (first wall, blanket, shield, vacuum vessel,
    divertor, TF/PF/CS magnet, structure, port plug, vacuum), replacement group,
    and governing life limit with its source;
  - the plasma chamber volume and the plasma parameters.
- **Conversion.** `cad_to_dagmc` (MIT, active) runs as a separate local process
  under FARIS's bounded-job limits and produces a DAGMC model. The faceting
  tolerance is recorded (GEO-012).
- **Checks that block transport, never repaired silently:**
  - watertightness (GEO-020);
  - overlaps and gaps (GEO-021, GEO-022);
  - lost particles (GEO-025);
  - faceted volume against CAD volume per solid (GEO-030, GEO-032);
  - a seeded-defect corpus that must be fully detected (GEO-023);
  - material and role mapping checked against the solid list. Known trap:
    imprinting can reorder volumes, and names are truncated at 28 characters.
- **Reference fixtures.** The built-in ARC-inspired study stays as the anchor.
  A Paramak-generated (MIT) ARC-like CAD model becomes the medium reference
  model (RM-M). The [ARC V3A open device description](https://doi.org/10.5281/zenodo.19498373)
  (CC-BY-4.0) guides its dimensions.

### 2. Plasma source

- `openmc-plasma-source` (MIT) provides the tokamak source, built from radius,
  elongation, triangularity, shift and profiles (SRC-010, SRC-011).
- A user emissivity table is accepted as the alternative (SRC-004).
- Source checks: positions inside the plasma volume (SRC-018), and strength from
  fusion power (SRC-001, SRC-002).
- Not yet confirmed: whether the source supports a machine axis other than z.

### 3. Transport good enough to trust a magnet lifetime

- **Variance reduction.** FW-CADIS weight windows are generated from a
  random-ray solve on the user's DAGMC model and aimed at the magnet
  (NUC-050, NUC-051, NUC-053, NUC-056).
- **Peak magnet fluence.** Reported from a mesh, with the averaging volume
  stated (MAG-003, MAG-004), next to the regional averages.
- **Run length.** Runs stop on a target error, not a fixed history count
  (NUC-043), with progress and cancel (PERF-022, PERF-024).
- **Two tiers.** A preview tier with a stated error, and a production tier
  (PERF-025).

### 4. Life model: dose-governed maintenance on every replaceable part

- **Every replaceable part has its own life.** Each replacement group from the
  sidecar, including the first wall and divertor, gets its own governing limit
  and records why it was replaced (MAT-018, OPS-001).
- **Outage durations come from physics:**
  - the existing decay-heat rule;
  - a shutdown-dose rule with a remote-handling dose limit;
  - the number of parallel remote-handling systems and ports (OPS-033).
- **Shutdown dose.** The fast method is OpenMC D1S. It is checked against the
  rigorous two-step method (R2S) on the reference model, and the ratio is stated
  (VAL-043).
- **Comparison with fixed durations.** The literature presets (OPS-031,
  OPS-032; EU-DEMO durations from Crofts and Harman) run beside the computed
  durations, so the effect of computing them is always visible.

### 5. Validation: shown by published numbers, not by assertion

VAL-051 applies: a quantity with no benchmark is shown as "not validated", and
an empty validation row blocks release. Targets, all open-access unless marked:

| Target | What it validates | Access |
| --- | --- | --- |
| OKTAVIAN spheres (IAEA open benchmarks; OpenMC inputs exist) | Leakage neutron and photon spectra | CC-BY-4.0 |
| FNS time-of-flight and decay heat; TIARA iron and concrete | Deep-penetration spectra, decay heat | CC-BY-4.0 data; MCNP inputs need converting |
| ITER_1D, HCPB_TBM_1D, WCLL_TBM_1D | Code-to-code reference (labelled as such) | CC-BY-4.0 |
| FNG-ITER streaming (Segantin et al. 2024) | Streaming, activation foils, heating | Geometry MIT; foil data needs SINBAD or CoNDERC |
| FNG-ITER shutdown dose rate (VAL-023) | The activation-to-dose chain this release depends on | Needs SINBAD (licence) or the Zenodo record, if it holds the measured values (unconfirmed) |
| ARC V0 (Sorbom 2015, Kuang 2018) | Magnet lifetime (9 FPY to 3e18 n/cm²), TBR 1.08, heating by layer | Published values. Geometry unpublished, so the comparison is order-of-magnitude and labelled so |
| EU-DEMO maintenance (Crofts and Harman; Federici 2018) | Outage durations against the number of remote-handling systems, availability | Published values |
| PROCESS and bluemira LifeCycle, run locally on matched inputs | Side by side: identical except computed against fixed durations | MIT and LGPL-2.1+, run as separate processes |

A V&V report is generated with the release (VAL-091).

### 6. Product quality at the same bar

Undo-safe edits, atomic saves (REL-010), no code execution from files
(SEC-001), all three desktop platforms (PLAT-001), keyboard operation
(A11Y-001), memory budgets (PERF-041) and uncertainty on every number
(UNC-080). The handbook and every public page naming the version ship with it.

## Feasibility already measured (2026-10-09)

Small runs with OpenMC 0.15.3 (DAGMC build) on this laptop, 2 threads, under
6 GB caps. Scripts and outputs are in `~/.cache/avila-night/feasibility-cad/work`
(local, not committed). Test model: six nested tori, CSG and CAD
(`cadquery` → `cad_to_dagmc`).

| Question | Result |
| --- | --- |
| FW-CADIS on CAD geometry | **Works.** The random-ray forward and adjoint solves run on DAGMC: 2.0 s at 8 groups, after a 55 s multigroup cross-section pass. The windows match the CSG-generated windows (median ratio 1.003). Workarounds needed: a universe-id object in the source-region mesh, a low-density filler instead of void, and a cell-constrained source. |
| Weight windows in a CAD run | **Work and are unbiased** (magnet flux 0.103 ± 0.007 against analog 0.095 ± 0.009). The untuned single-group gain is only **1.7×** in figure of merit. |
| Shutdown dose on CAD | **D1S and mesh-based R2S both run** on DAGMC. D1S on CAD agrees with CSG (2.25e19 ± 0.06e19 against 2.27e19 ± 0.06e19). Dose values themselves are untested: no photon dose tallies were scored. Cell-based R2S does not work with DAGMC; use mesh-based. |
| CAD transport speed | **6–8× slower than CSG** (1,492 against 11,679 particles/s). Memory is unchanged at 158 MB peak. A coarser angular tolerance gives about 1.3×. The CAD conversion peaked at 1.2 GB for the small model. |
| CAD import route | `cad_to_dagmc` 0.14.2 and Paramak 0.10.0 (both MIT, released September–October 2026). Main failure modes: overlapping solids, volume-to-material mapping, units and tolerance, imprint memory, gaps. |
| Peer tools runnable locally | PROCESS (MIT, pure Python) and bluemira LifeCycle (LGPL). FUSE has no replacement model. None computes outage durations from activation. |
| Licences | Compatible as separate processes. Never link or vendor the GPL-2.0 helpers `fast_ctd` and `overlap_checker`. |

## Risks, retired first

Each risk gets a short time-boxed test with a written pass rule before the work
that depends on it starts.

1. **Weight-window gain.** NUC-050 asks for at least 100× in figure of merit; the
   untuned test gave 1.7×.
   - Test: energy-binned windows, a deeper target and tuned parameters on RM-M.
   - If 100× is not reachable on CAD at laptop scale, the requirement's
     provisional number is revisited with measured evidence through change
     control, not quietly missed.
2. **Laptop run time.** CAD is about 8× slower. PERF-025 asks for a preview
   within 60 s on the laptop for RM-M.
   - Test: preview-tier error achievable in 60 s on RM-M with windows.
   - Fallbacks: a coarser preview mesh, or a stated longer preview target.
3. **Shutdown dose values.** These are the first thing this release's maintenance
   rule depends on.
   - Test: score photon dose with flux-to-dose coefficients; check D1S against
     R2S on RM-M (VAL-043 band 0.85–1.15); check against the FNG shutdown-dose
     values once data access is settled.
4. **Real CAD quality.** Real machine STEP files often contain overlaps.
   - GEO-010 asks for a 50-file real STEP corpus, and an open corpus of that size
     is not known to exist.
   - Test: assemble what is openly licensed (Paramak, ARC V3A-derived, open
     benchmark geometries).
   - If 50 cannot be reached, the target is revisited with the count found.
5. **Validation data access.** The FNG experimental tables sit in SINBAD
   (NEA/RSICC licence, terms not yet read) or possibly CoNDERC (not yet
   confirmed). VAL-020 currently says the suite is "drawn from SINBAD"; open
   sources come first instead.
6. **OpenMC version.** 0.15.3 and 0.16.0 are both installed with DAGMC. Pick one
   before transport work, since the workarounds above are version-specific.
7. **Memory for conversion of large models.** Conversion runs under the bounded
   job limits and is refused with an explanation when projected memory exceeds
   the budget (PERF-041).

## Order of work

Each stage ends on a checker-decided exit, and nothing is released between
stages.

| Stage | Content | Exit |
| --- | --- | --- |
| S0 | Retire risks 1–3 and 6 on a Paramak ARC-like RM-M | Written results; requirement changes, if any, approved |
| S1 | CAD import, sidecar, integrity checks, defect corpus | GEO gate IDs Met |
| S2 | Plasma source, FW-CADIS, peak fluence, target-error runs, preview and production tiers | NUC, SRC, MAG and PERF gate IDs Met |
| S3 | Replacement groups, shutdown-dose maintenance, remote handling, D1S against R2S | OPS, MAT and VAL-043 gate IDs Met |
| S4 | Validation suite, ARC and EU-DEMO comparisons, PROCESS and bluemira side by side, V&V report | VAL gate IDs Met; no empty validation row |
| S5 | Product quality, handbook, release package and pages | QA-070 release gate passes |

## Release gate

These IDs must be Met:

- **Geometry:** GEO-012, GEO-020, GEO-021, GEO-022, GEO-023, GEO-025, GEO-030,
  GEO-032.
- **Source:** SRC-001, SRC-002, SRC-010, SRC-011, SRC-018.
- **Transport and magnet:** NUC-013, NUC-043, NUC-050, NUC-051, NUC-053, NUC-056,
  MAG-003, MAG-004.
- **Life model:** MAT-018, OPS-001, OPS-031, OPS-032, OPS-033.
- **Validation:** VAL-023 (or its access outcome recorded), VAL-027, VAL-043,
  VAL-044, VAL-051, VAL-054, VAL-091.
- **Uncertainty and performance:** UNC-080, PERF-022, PERF-024, PERF-025,
  PERF-041.
- **Product:** REL-010, SEC-001, PLAT-001, A11Y-001, QA-070.

GEO-010 (the 50-file corpus) and VAL-020 (at least 10 benchmarks) are
targeted, but their numbers depend on risks 4 and 5. Their final form is set
after S0, under change control.

## Out of scope

- Hosted compute.
- Cost and economics.
- The detailed fuel cycle beyond today's model.
- Optimisers.
- Cluster and GPU transport.
- Non-tokamak sources beyond the emissivity table.
- Automatic CAD repair. FARIS reports defects and blocks; the user fixes them in
  their CAD tool.

## Decisions

1. 2026-10-09: scope and gate approved.
2. 2026-10-09: validation data access. Plan on **not** having the SINBAD FNG
   tables. Access will be sought separately. Until the data is in hand, the FNG
   cases (VAL-021, VAL-022, VAL-023) are shown as "not validated", with the
   reason "experimental tables not available under an open licence" and the next
   step "obtain the SINBAD package". The open benchmarks carry the suite.
   VAL-020 and VAL-023 are then settled under change control at the end of S0,
   with this as the recorded reason.
