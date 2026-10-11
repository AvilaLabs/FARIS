# Install, import and repair: plan to be the easiest tool in its class

Status: scope approved by Connor 2026-10-10. Data hosting decided 2026-10-10: free, with acknowledgement (section 2). Parent: [NEXT_RELEASE_PLAN.md](../NEXT_RELEASE_PLAN.md),
section 1 ("Bring your own design") and section 6 (product quality).

The standard is best in class on every metric, ease of use included. This plan sets the bar from a survey of the tools
FARIS competes with, then states what FARIS builds to beat each stage, what is verified, and what is not.

## Where FARIS stands today (checked in the repository)

- **Setup.** A user needs four things besides FARIS: their own Python with cadquery, their own OpenMC 0.15.3 environment
  with DAGMC, the nuclear data built by hand (9.4 GB for library v2), and the command line. No step is automated.
- **Import.** Command line only. `faris design init` writes a design file with every solid's material and role left
  null, to be filled in by hand as JSON, without seeing which solid is which.
- **Defects.** Detected and explained; no repair until phase F7 (GEO-027).
- **Windows.** The app runs; CAD transport does not, because none of OpenMC, DAGMC, MOAB or double-down publishes a
  Windows build.

## The bar (survey of 2026-10-10)

| Stage | Best today | Gap nobody fills |
| --- | --- | --- |
| Install | pip or conda for the open tools; Docker for the open chain; Cubit is a paid licence | one download with the transport engine inside, on all three platforms |
| Import | Attila4MC (commercial): large assemblies with little cleanup | no open tool has a graphical import |
| Materials and roles | Attila4MC assigns from CAD part names; Cubit uses `mat:` group names; open tools use ordered lists in code | rule-based bulk assignment with visual review, saved to an open file |
| Defects | DAGMC checkers (users report silent loss of all particles and false overlaps); UKAEA's SpaceClaim tools | checks proven against a seeded-defect corpus |
| Repair | manual: Cubit imprint/merge, SpaceClaim; Attila4MC meshes instead | guided, auditable, revertible repair |
| Re-import after a CAD change | full reconversion everywhere | diff-aware re-import keeping assignments (the IFMIF-DONES paper, Frontiers 2026, asks for this) |
| Existing models | openmc_mcnp_adapter: no formal V&V, skips sources and tallies | MCNP import with V&V and an unsupported-feature report |

Measured pain: about one month of manual CAD work per reactor concept (UKAEA, checked). Sources are listed at the end;
claims not read at the source are left out of this table.

## What FARIS builds

### 1. One download, no Python, all three platforms

- **Transport engine bundled.** libopenmc, DAGMC, double-down with Embree, MOAB and HDF5 ship as shared libraries beside
  the FARIS programs, about 100 MB on Linux (Embree is half). Licences checked from conda-meta: MIT, BSD, Apache-2.0 and
  LGPL-3.0 (MOAB, kept as a replaceable shared library); all compatible with AGPL-3.0.
- **Driven from Rust.** FARIS writes OpenMC's XML itself and drives the run through OpenMC's C API (175 functions in
  0.15.3, including weight-window import, export and bounds; checked). A warm in-process session also serves the
  60-second preview (PERF-025).
- **The Python-only parts are replaced:**
  - D1S time-correction factors: a short recurrence (`deplete/d1s.py` is 255 lines; checked). Ported to Rust.
  - Multigroup cross sections for random-ray weight windows (`Model.convert_to_multigroup`): the transport is C++; the
    tally post-processing is Python. Ported to Rust for the cross-section types random ray needs, with differential
    tests against OpenMC's Python output on the S0c slab and RM-M.
  - The activation step of R2S (`deplete/r2s.py` is 693 lines and calls OpenMC's depletion solver): replaced by ACTINV,
    our own released activation solver, a Rust crate. R2S then stays inside the suite.
- **Platforms:**
  - Linux x86_64: the libraries already build (our own environment, and an unofficial wheel that bundles the full Embree
    stack).
  - macOS arm64: conda-forge builds DAGMC with double-down, MOAB and double-down for arm64; Embree supports Apple
    silicon. Bundling plus a parity test.
  - Windows x86_64: native. OpenMC builds with MSVC (open pull request #2919 and an unofficial wheel show it). DAGMC,
    MOAB and double-down have no working Windows build (DAGMC's Windows workflow is manual-only; DAGMC issue #972: HDF5
    1.14 is not found under MSVC). This is the long pole: an unsourced estimate of 3 to 6 person-weeks, plus a physics
    parity test against Linux. It starts first, with a CI build of MOAB on Windows as the opening spike.
- **Setup check.** `faris doctor` and the app's first-run screen show the engine, data and versions, and say what is
  missing and what to do.

### 2. Nuclear data fetched for you

- FARIS reads a model's materials and fetches only the nuclides, elements and thermal tables it needs, at the
  temperatures it needs, checks each file against a hash manifest, and records the identity in the run, as today.
- Photon data come from the ENDF/B-VIII.1 library OpenMC publishes (178 MB, already HDF5), which removes the only
  conversion that needs Python. Library v2's ENDF/B-VII.1 photon files stay for existing results.
- Thermal tables are cut to the needed temperatures natively; this changes their hashes, so the manifest records both the
  source and the subset identity.
- **Open point:** OpenMC's libraries are served as single archives (ENDF/B-VIII.1 unpacks to 43 GB, 38 GB of it thermal
  tables), so per-nuclide download needs us to host per-file copies. That depends on IAEA (FENDL-3.2) and NNDC (ENDF/B)
  terms. Any contact with IAEA or NNDC is Connor's.
- **Terms read 2026-10-10:**
  - FENDL (IAEA). The FENDL GitHub repositories (IAEA-NDS/FENDL-Processed, FENDL-ENDF) carry no licence; their
    TERMS_OF_USE.md applies the IAEA website terms. Those terms (https://www.iaea.org/about/terms-of-use, read through the
    Internet Archive copy of 2026-09-26 because the live site blocks automated readers) say two things that pull apart:
    IAEA content "may be adapted, translated, copied, printed and downloaded for private study, research and teaching
    purposes, and for use in commercial and non-commercial products or services, provided that appropriate
    acknowledgement of the IAEA as the source is given and that the IAEA's endorsement of users' views, products or
    services is not stated or implied"; and "The data in the IAEA digital repositories and databases is provided free of
    charge for educational and informational use." Hosting FENDL files free of charge, with acknowledgement, fits both.
    Charging for access to them is not clearly allowed and needs the IAEA's written confirmation first.
  - ENDF/B (NNDC). The NNDC library pages state no licence, terms or conditions. Redistribution is common practice
    (OpenMC republishes ENDF/B in HDF5), but nothing grants it, and nothing addresses charging.
  - OpenMC's data page shows "Content License: CC-BY-4.0"; it reads as the site's content licence and does not clearly
    cover the data files.
  - **FENDL revision (checked 2026-10-10).** OpenMC's FENDL-3.2 HDF5 archive (the source of libraries v1 and v2) is dated
    2022-01-13 by its file entries, which is before FENDL-3.2a (tag 2022-01-31), 3.2b (2022-02-08) and 3.2c (2024-04-02) in
    IAEA-NDS/FENDL-ENDF. Between 3.2b and 3.2c alone, 49 neutron evaluation files changed (GitHub compare of the two
    tags), among them H-3, Li-6, B-10, B-11, C-12, N-15, O-16, F-19, Ti, V-51, Mn-55, Co-59, Ni, Zr-90 and the tungsten
    isotopes (3.2c adopted the ENDF/B-VIII.0 resolved-resonance range for W). The current release is 3.2c, and its ACE
    files are published by the IAEA (IAEA-NDS/FENDL-Processed, tag FENDL-3.2c, files through git-annex). FARIS's hosted
    library is built from FENDL-3.2c, converted by us, and the v1/v2 results stay labelled with the 2022 build they used.
    Whether the git-annex sources download without the IAEA site's browser challenge is still to be tested.
- **Decision (Connor, 2026-10-10):** where charging is a grey area, the data delivery is free. Avila Labs hosts per-file,
  per-temperature copies free of charge, acknowledges the IAEA (FENDL) and NNDC (ENDF/B) as sources in FARIS and on the
  download page, implies no endorsement, and keeps the download-from-source path working.

### 3. Import in the app

- Open STEP, IGES or BREP; the solids appear in 3D with names, colours and assembly paths from the file.
- Assign materials, roles and replacement groups by clicking, by box selection, or by rules on name, layer, colour and
  assembly path. Suggestions from part names, reviewable before they apply. Colour by material or role; unassigned
  solids highlighted. Everything is saved to the design file.
- Integrity checks run on import and show on the model; every failure states why and what to do next.
- Re-import after a CAD change matches solids by name and geometry fingerprint, keeps assignments and flags only added,
  removed and changed solids.
- **Engine:** OpenCASCADE behind our own thin C++ layer (no maintained Rust binding covers the needed calls), run in a
  worker process with time and memory limits so a hang or crash cannot take FARIS down. No pure-Rust CAD kernel reads
  real STEP robustly.
- **DAGMC file written natively.** The `.h5m` layout is plain HDF5 (checked on a FARIS-produced file: `tstt` nodes,
  triangle connectivity, sets with parents and children, and the CATEGORY, GEOM_DIMENSION, GEOM_SENSE_2, GLOBAL_ID and
  NAME tags). A CI gate loads every output through MOAB, DAGMC and OpenMC; linking MOAB's writer is the fallback.
- Built as its own component, used by FARIS, so it can also ship as a standalone tool.

### 4. Guided repair

- For each defect, a ranked list of explicit fixes: sew gaps, resolve overlaps by precedence, remove slivers and small
  faces, defeature selected faces. All built from OpenCASCADE operations.
- Every fix is opt-in, shows the volume before and after and the validity, gives the model a new hash, is recorded in the
  design file, can be reverted, and is re-checked by the exact integrity checks. The STEP file is never changed. This
  moves part of GEO-027 forward from F7.

### 5. Proof on real files

- GEO-010's corpus: every openly licensed fusion STEP model we can find, plus seeded-defect copies; the count found is
  reported.
- Every check is proven against known-bad models, because the survey found DAGMC checkers that passed models losing every
  particle.

### 6. Existing models

- MCNP input decks: geometry, materials, sources and tallies, with a report of every unsupported feature and a V&V suite
  against the original MCNP results where published. OpenMC models read directly. Later in the order than 1 to 5.

## Order

1. Windows spike (MOAB, DAGMC and double-down built natively in CI) and the OpenCASCADE layer, in parallel: the two
   longest items.
2. Engine bundle and Rust driving on Linux and macOS, with the D1S, multigroup and ACTINV replacements and their
   differential tests.
3. Data fetch, after the terms are read.
4. Import in the app, assignment, re-import, then guided repair.
5. Real-file corpus throughout; MCNP import last.

Heavy builds (OpenCASCADE, MOAB) run in CI or under the laptop's memory caps; any job expected to exceed about an hour
is put to Connor first.

## Decisions for Connor

- Approve this scope for the next release.
- Contact with IAEA and NNDC about hosting data, if the published terms do not settle it.
- Whether our Windows patches go upstream to OpenMC, DAGMC and MOAB (outward; later).

## Sources (read 2026-10-10)

- Attila4MC: https://silverfirsoftware.com/Pages/Products/Attila4MC.php
- Coreform Cubit and DAGMC: https://coreform.com/news/neutronics-on-exact-cad-geometry-advances-in-the-coreform-cubit-dagmc-workflow/
- UKAEA SpaceClaim tools: https://github.com/ukaea/SpaceClaim_API_NeutronicsTools
- UKAEA CAD generation time: https://scientific-publications.ukaea.uk/papers/high-speed-generation-of-neutronics-ready-cad-models-for-demo-design-2/
- IFMIF-DONES CAD-to-MC workflow: https://www.frontiersin.org/journals/nuclear-engineering/articles/10.3389/fnuen.2026.1783403/xml
- DAGMC failures reported by users: https://openmc.discourse.group/t/issue-with-lost-particles-from-dagmc-geometry/2663 and
  https://openmc.discourse.group/t/impossible-overlaps-in-dagmc-geometry-with-perfectly-conformal-mesh/3364
- openmc_mcnp_adapter limits: https://github.com/openmc-dev/openmc_mcnp_adapter/blob/main/README.md
- Open tools: https://github.com/fusion-energy/cad_to_dagmc, https://github.com/Thea-Energy/stellarmesh,
  https://geouned-org.github.io/GEOUNED/
- OpenMC Windows: https://github.com/openmc-dev/openmc/pull/2919 (open, checked), https://github.com/openmc-dev/openmc/issues/1243
- conda-forge recipes skipping Windows (checked): openmc, dagmc, moab and double-down feedstocks under https://github.com/conda-forge
- DAGMC Windows: https://github.com/svalinn/DAGMC (issue #972, `windows_build_test.yml`)
- Unofficial wheels: https://github.com/shimwell/wheels
- Local checks: OpenMC 0.15.3 `include/openmc/capi.h`, `openmc/deplete/d1s.py`, `openmc/deplete/r2s.py`; a FARIS `.h5m` read
  with h5py; library sizes under `~/nuclear-data/endfb-viii.1-hdf5/`.
