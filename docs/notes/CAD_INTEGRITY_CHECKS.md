# CAD integrity checks (stage S1b)

Status: design, 2026-10-10. Plan: [NEXT_RELEASE_PLAN.md](../NEXT_RELEASE_PLAN.md),
scope 1. Design file: [DESIGN_FILE_FORMAT.md](DESIGN_FILE_FORMAT.md), import
checks 6–8. Requirements: GEO-012, GEO-013, GEO-019, GEO-020, GEO-021, GEO-022,
GEO-023, GEO-024, GEO-025, GEO-026, GEO-027, GEO-028, GEO-029, GEO-030, GEO-031,
GEO-032, GEO-039.

S0 showed why FARIS needs its own checks. The mesher's overlap scan stopped at
its time budget with 283 of RM-M's solid pairs unchecked, and OCCT gave inverted
and duplicated regions from spline surfaces without raising an error. A check
that can run out of time, or that trusts the tool it is checking, is not a gate.

## Principles

1. **Independent of the converter.** FARIS reads the faceted model itself and
   checks it with its own code. DAGMC's `check_watertight` and `overlap_check`
   run as cross-checks in tests, never as the gate.
2. **Exact where it decides.** Every yes/no geometric decision (does this
   triangle cross that one, is this point inside, which side of a plane) uses
   exact arithmetic, so the answer cannot depend on rounding.
3. **Complete, not sampled, where completeness is possible.** Overlaps and gaps
   are found from the surfaces themselves, which finds a defect of any size.
   Point sampling is a second, independent check and the source of volume
   estimates where an exact one is not available.
4. **Nothing repaired** (GEO-027). Every finding names the cells, gives a 3D
   location (GEO-028), says why it blocks, and gives the next step in the user's
   CAD tool.

## Pipeline

Import checks 1–5 (S1a) run first. The CAD-level overlap check (below, under
"Overlaps before imprinting") runs before conversion. Then:

### Check 6: conversion

- `cad_to_dagmc` runs as a bounded job (memory cap, wall time, cancel) in the
  CAD environment, one material tag per solid (`mat:<solid id>`), imprint on.
- Default tolerances: chord 0.1 cm, angular 0.1 rad. These met GEO-030 on RM-M
  in S0 (worst −0.0148 %). A design file's `faceting_tolerance_m` overrides the
  chord value.
- Recorded (GEO-012, GEO-013, GEO-019): tolerances, `cad_to_dagmc`, OCCT and
  mesher versions, wall time, peak memory, triangle and vertex counts per
  volume, and the h5m sha256.

### Export: `faris-facets/v1`

A Python helper reads the h5m with `h5py` (MOAB native layout: node
coordinates, triangle connectivity, geometric sets with `GEOM_DIMENSION`,
`GLOBAL_ID`, `GEOM_SENSE_2`, `CATEGORY` and `NAME` tags, set children) and
writes one binary file plus a JSON manifest:

- vertices as f64 in metres, deduplicated by MOAB handle (not by position);
- triangles as vertex indices, grouped by surface;
- for each surface, its forward and reverse volume (0 = implicit complement);
- for each volume, its global id and material tag;
- counts, units, the h5m sha256 and the helper's version.

The format is little-endian with a fixed header, documented in the module.
Tests compare its counts with MOAB's `mbsize` on the same file.

### Check 7: binding

Each DAGMC volume is matched to exactly one design-file solid by faceted volume
and centroid:

- the best match must be within 0.5 % in volume and 1e-3 of the model's
  bounding-box diagonal in centroid;
- the second-best match must be at least 10 times farther in the same
  normalised distance, or the import stops as ambiguous and names both;
- the volume's tag must equal the matched solid's id, or the import stops with
  "tag on the wrong solid" (the imprint-reordering trap);
- every solid and every volume is matched exactly once.

### Check 8: integrity (FARIS's own, in Rust)

Exact predicates: vertex coordinates are snapped to a 1 nm integer grid (i64),
and orientation and intersection tests are evaluated exactly in i128. A model
6.5 m across needs 34 bits per coordinate difference and about 105 bits for an
orientation determinant, inside i128. The snap moves a vertex by at most 0.5 nm,
far below any faceting tolerance; it is stated in the report. Models larger than
the grid allows are refused with the reason.

A bounding-volume hierarchy over triangles makes each check near-linear. Work
runs on worker threads with progress and cancel (PERF-022, PERF-024).

| ID | Check | Method | Blocks when |
| --- | --- | --- | --- |
| GEO-020 | Watertight | For each volume, count each edge's uses across its surfaces. | Any edge used once (open) or more than twice (non-manifold). |
| GEO-020 | Orientation | Each edge of a closed volume must be used once in each direction, after applying surface senses. | Any edge used twice in one direction. |
| — | Inverted normals | Signed volume of each volume from its oriented triangles (divergence theorem). | Signed volume ≤ 0. |
| — | Zero-volume cell | Signed volume ≤ 1e-12 of the bounding-box volume. | Always. |
| — | Sliver | Mean thickness 2V/A below 10 × the chord tolerance. | Blocks unless waived; reported with its thickness. |
| — | Self-intersection | Non-adjacent triangles of one volume that intersect. | Any. |
| GEO-021 | Overlap (faceted) | Triangles of two different volumes that properly cross (not sharing the surface, edge or vertex where they meet), plus containment (a vertex of one volume strictly inside the other, for volumes whose surfaces never meet). | Any unwaived pair. |
| — | Duplicate surface | Coplanar, overlapping triangles that belong to different surface entities. | Any; reported as "faces coincide but are not merged; imprint them". |
| GEO-022 | Gap | Topological (below). | Any unwaived cavity. |
| GEO-024 | Point sampling | 10⁶ uniform points in the bounding box plus one verified interior point per volume. Each point is classified by exact ray parity against every volume whose box contains it. | A point in two volumes (overlap) or in an enclosed cavity. Also reports each volume's sampled volume with its standard error. |
| GEO-025 | Ray consistency | 10⁵ random chords through the box. Along each chord, every crossing must leave the volume the ray is in and enter the volume on the surface's other side. | Any inconsistent crossing; reported as a would-be lost particle with its location. |
| GEO-030 | Volume | Faceted volume (divergence theorem) against the CAD volume. | Any volume off by more than 0.5 %. |
| GEO-031 | Partition | Sum of faceted volumes plus the cavity volumes against the sampled volume of the assembly. | Disagreement beyond 3 standard errors. |
| GEO-039 | Void list | Every `void` solid and every implicit-complement region is listed. | Never blocks; always reported. |

**Gaps, topologically.** In an imprinted model, two solids that touch share one
surface entity, sensed by both. A surface sensed by only one volume faces the
implicit complement. Those surfaces form closed, connected components (once
GEO-020 passes). Orient each component so its normals point into the implicit
complement and compute its enclosed signed volume:

- a positive component has the implicit complement outside it: it is an outer
  boundary of a cluster of solids;
- a negative component has the implicit complement inside it: it is the outer
  wall of an enclosed cavity, an undefined region inside the model (GEO-022).

The number of cavities is the number of negative components, so a cavity of any
size is found. Its volume is the negative component's enclosed volume minus the
positive components nested directly inside it (nesting decided by exact
point-in-surface tests). Its location is its bounding box and one interior
point. The next step for the user is to model the space as a `vacuum` solid or
close the gap in CAD. A gap thinner than the imprint tolerance is merged by the
converter and is not a gap in the transport model, which is the model FARIS
checks.

**Overlaps before imprinting.** Imprinting can split overlapping solids into
fragments, so an overlap may not survive into the faceted model in a form the
faceted check sees. GEO-021 therefore also runs on the CAD, before conversion:

1. The CAD helper tessellates every solid on its own (no imprint) and exports
   the meshes in `faris-facets/v1`.
2. FARIS's exact checks find **candidate pairs**: pairs whose meshes cross or
   come within twice the chord tolerance of each other, plus pairs where one
   mesh lies inside the other (exact point-in-volume on one vertex). Touching
   solids are candidates too, which is intended: for nested layers that leaves
   neighbouring layers, not all pairs.
3. The CAD helper runs an exact OCCT boolean intersection on each candidate
   pair only, in worker processes. An intersection volume above 1e-6 of the
   smaller solid's volume is an overlap; GEO-021 asks for detection down to
   1e-4. Smaller results are reported as touching.

This replaces the mesher's all-pairs scan, which stopped at its time budget with
283 of RM-M's pairs unchecked. A pair that overlaps only after faceting (no CAD
overlap) is reported as a faceting overlap, with its volume estimated from the
point sampling and a note to tighten the tolerance.

### GEO-032: faceting deviation

A Python helper projects each triangle's centroid and edge midpoints onto the
owning CAD solid's faces (OCCT point projection, worker processes) and reports
the maximum distance per solid. Vertices lie on the CAD surface by
construction, so the sample points are where deviation is largest. The report
calls the result "sampled maximum". A solid above the declared chord tolerance
blocks.

### Waivers (GEO-026)

The design file gains an optional `waivers` array. Each waiver gives `check_id`,
the item (solid ids, or a cavity's interior point), `reason`, `author` and
`date`. A waived finding still appears in the report, labelled waived, and every
run receipt carries the waivers. Watertightness, orientation, inverted normals
and zero-volume cells cannot be waived, because transport is not defined
without them. This amends the design-file format (an addition; v0.1 files
without waivers stay valid).

### Check 9: source sites (SRC-018)

10⁵ sites sampled from the `plasma` block through `openmc-plasma-source`, tested
by FARIS's exact point-in-volume against the `plasma_chamber` volume. Every
site must be inside. This replaces S0's OpenMC-based test with FARIS's own.

## Seeded-defect corpus (GEO-023)

Two families, both generated, never hand-edited:

- **CAD-level** (CadQuery to STEP): overlap, gap, sliver, zero-volume cell,
  duplicate surface (unimprinted coincident faces), wrong units (STEP declared
  in mm, design file in m), missing material (null or unknown `material_id`).
- **Mesh-level** (mutations of a clean `faris-facets` file): open edge (delete
  a triangle), non-manifold edge (duplicate a triangle onto a third volume),
  inverted normals (flip one surface's triangles), self-intersection (displace
  one vertex through the opposite face).

Base models: nested tori (analytic volumes), boxes, a port through a shell, and
RM-M. At least 20 seeded models across all 11 classes, and 20 clean models for
the false-alarm rate. Detection must be 100 % and false alarms at most 1 %. The
small models run in CI. RM-M runs locally, and its result is recorded.

## Performance targets

GEO-029: RM-M's full check (checks 7 and 8, GEO-032 and check 9) in ≤ 5 min on
the reference laptop, with progress and cancel. Conversion (check 6) is timed
separately against GEO-013. S0 measured RM-M's conversion at 23 min, against
GEO-013's 10 min, so conversion speed is a known gap. It is reported, not hidden.

## Out of scope for S1b

Viewport highlighting of defects (GEO-028's UI half) comes with the desktop
design workflow. The report carries coordinates and cell ids now. The 50-file
real STEP corpus (GEO-010) is S1c.
