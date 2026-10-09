# Design file format `faris-design/v0.1` (stage S1)

Status: design, 2026-10-09. Plan: [NEXT_RELEASE_PLAN.md](../NEXT_RELEASE_PLAN.md),
scope 1, 2 and 4. Requirements: GEO-010, GEO-011, GEO-012, GEO-019, GEO-022,
GEO-026, GEO-030, GEO-039, SRC-001, SRC-002, SRC-010, SRC-011, SRC-018, MAG-003,
MAG-004, MAT-018.

A STEP file says where the solids are. It says nothing about what they are made
of, what job they do, when they wear out, or where the plasma is. The design
file says those things. A user ships two files, `machine.step` and
`machine.faris-design.json`, and FARIS reads nothing else about the design.

The design file follows the conventions of `faris-scenario/v0.1`:

- snake_case keys;
- SI units, with the unit in the key (`_m`, `_m3`, `_kg_m3`, `_s`, `_mw`, `_ev`);
- `references` entries with `id`, `title`, `url` and `use`;
- `assumptions` as plain sentences;
- every number a user supplies carries a provenance label.

## What problems the format has to solve

1. **Solid identity.** STEP solids are often unnamed, and names are not unique.
   `cad_to_dagmc` assigns material tags by solid order. Imprinting can reorder
   volumes, so a tag can end up on the wrong solid without any error. The tag
   field is 32 bytes and `cad_to_dagmc` prefixes `mat:`, which leaves 28
   characters. A longer name is cut short without warning.
2. **No silent drops.** A STEP solid with no entry in the design file, or an
   entry with no solid, must stop the import (GEO-010).
3. **Units.** STEP declares a length unit. A design file that assumes another
   one would scale every volume by 10⁶ or 10⁹ without warning (GEO-011).
4. **Reserved names.** DAGMC treats `vacuum` and `graveyard` (in any case) as
   special. A user material called `Vacuum` would become empty space.
5. **Void.** Transport needs void regions, but random ray and FW-CADIS need a
   material in every cell. FARIS replaces void with a low-density filler in the
   random-ray model only, as it did in S0, and records the replacement.

## Decisions

Each decision gives its reason.

1. **One DAGMC tag per solid, not per material.** The tag is the solid's `id`.
   FARIS makes one OpenMC material per solid from the solid's `material_id`.
   - Why: if the tag names the solid, a reordered tag can be found by checking
     the solid's fingerprint (decision 2). A tag that names only a material
     cannot be checked that way.
   - Why: activation, R2S and replacement accounting need per-solid materials
     anyway.
   - Cost: more OpenMC materials. The effect on memory and run time is not
     measured yet; S1 measures it on RM-M and reports it.
2. **Solids are bound by fingerprint, not by name or order.**
   - Each entry records the STEP read order (`step_index`), the CAD volume and
     the centroid.
   - At import, FARIS reads the STEP file and checks each `step_index` against
     its fingerprint. This catches a STEP file edited after the design file was
     written.
   - After conversion, FARIS measures each DAGMC volume's faceted volume and
     centroid, and matches it to exactly one entry. The tag must agree with the
     match. This catches imprint reordering.
   - The STEP name, when present, is stored and compared, but it is never the
     binding.
   - Two solids whose fingerprints cannot be told apart within tolerance stop
     the import, and FARIS names both. Mirror-symmetric pairs differ in
     centroid, so only coincident solids collide, and those are an overlap
     (GEO-021) anyway.
3. **Ids are short and safe.**
   - Solid and material ids match `^[a-z0-9][a-z0-9-]{0,27}$`, so they never
     reach the 28-character cut.
   - `vacuum` and `graveyard` are rejected as ids.
   - `void` is a built-in material id, not a user one.
4. **FARIS writes the first draft.** `faris-app --design-init machine.step`
   writes a design file that already holds:
   - every solid, with `step_index`, `step_name`, fingerprint and a suggested
     id;
   - every `material_id` and `role` as `null`;
   - the STEP hash and unit.

   The user fills in the nulls. An import with any null left stops, and lists
   each one. Nobody has to measure fingerprints by hand, and a null is never
   given a default.
5. **Full 360-degree models only in v0.1.**
   - A sector model stops the import with this reason: DAGMC in OpenMC 0.15.3
     has no periodic boundary, and FARIS does not yet replicate sectors.
   - Next step shown to the user: export the full model by patterning the sector
     in the CAD tool.
   - `extent` is still a field, so a later version can add `sector` without
     breaking v0.1 files.
6. **The machine axis is +z through the origin.** `openmc-plasma-source` builds
   its source on a cylindrical mesh about z. Any other axis stops the import,
   and the next step is to place the model on z in the CAD tool.
7. **The plasma chamber is a solid.** One solid with role `plasma_chamber`
   holds the plasma. Its material is `void`. Source checks (SRC-018) test
   sampled source sites against this solid, not against analytic shapes.
8. **The operating scenario stays in its own file.** The design file links an
   operating-history file by path and hash and does not copy its contents. A
   design can then run under several scenarios, and the scenario keeps one
   format.
9. **Life limits sit on replacement groups, not on solids.**
   - A group is the unit that is replaced together, for example "inboard
     blanket modules" or "all 18 TF coils".
   - A group holds one or more governing limits. The first limit reached
     triggers replacement and is recorded as the reason (MAT-018).
   - Each limit says how the metric is averaged: over the group's solids, or as
     a peak on a mesh with a stated voxel size (MAG-003, MAG-004).
   - A metric FARIS cannot compute yet is accepted and shown as NOT_EVALUATED,
     with why and a next step. It never silently becomes "never reached".

## Format

### Top level

| Key | Type | Meaning |
| --- | --- | --- |
| `schema_version` | `"faris-design/v0.1"` | Format version. |
| `id`, `title`, `description` | string | As in `faris-scenario/v0.1`. |
| `cad` | object | The STEP file and how to read it. |
| `materials` | array | Materials used by the solids. |
| `solids` | array | One entry per STEP solid. |
| `replacement_groups` | array | Parts replaced together, and their limits. |
| `plasma` | object | Plasma shape, profiles and power. |
| `operating_scenario` | object | Linked operating-history file. |
| `references`, `assumptions` | arrays | As in `faris-scenario/v0.1`. |

Unknown keys stop the import, so a misspelt key cannot be ignored without
notice.

### `cad`

| Key | Type | Meaning |
| --- | --- | --- |
| `step_file` | string | Path relative to the design file. |
| `step_sha256` | string | Hash of the STEP bytes. A mismatch stops the import (GEO-019). |
| `length_unit` | `"m"`, `"cm"` or `"mm"` | Must match the unit the STEP file declares. A STEP file with no declared unit is read in this unit, and the report says so. |
| `extent` | `{"kind": "full"}` | Decision 5. |
| `machine_axis` | `"z"` | Decision 6. |
| `faceting_tolerance_m` | number or `null` | Chord deviation for faceting. `null` uses FARIS's default, which is chosen to meet GEO-030. The value used is always recorded (GEO-012). |
| `implicit_complement_material_id` | `"void"` | The space outside all solids and inside the graveyard. FARIS builds the graveyard, and the user must not supply one. |

### `materials[]`

Either a reference to FARIS's baseline:

```json
{ "id": "tungsten", "catalog_id": "tungsten-natural" }
```

or a full recipe:

| Key | Type | Meaning |
| --- | --- | --- |
| `id` | string | Decision 3. |
| `label` | string | Display name. |
| `density_kg_m3` | number | Must be positive. |
| `composition_basis` | `"atom"` or `"weight"` | Fractions basis. |
| `components` | array | `{"element": "Li", "fraction": …, "isotopes": {"Li6": 0.9, "Li7": 0.1}}` or `{"nuclide": "Fe56", "fraction": …}`. Without `isotopes`, an element uses natural abundance, and the exported nuclide vector is recorded. Fractions must sum to 1 within 1e-9. |
| `temperature_k` | number or `null` | `null` uses the library's nearest stored temperature, as the demo does. |
| `provenance` | object | `{"label": "published" or "authored", "reference_id": …, "note": …}`. `published` needs a `reference_id`. |

The nuclear-data audit (`audit_library.py`) runs on the expanded nuclide
vectors before any transport. A nuclide missing from the library stops the run.

### `solids[]`

| Key | Type | Meaning |
| --- | --- | --- |
| `id` | string | Decision 3. This is the DAGMC tag. |
| `step_index` | integer | Read order from `--design-init`. |
| `step_name` | string or `null` | Name from the STEP file, compared but never used for binding. |
| `fingerprint` | object | `{"cad_volume_m3": …, "centroid_m": [x, y, z]}` from `--design-init`. |
| `material_id` | string | A `materials[]` id or `void`. |
| `role` | enum | `plasma_chamber`, `first_wall`, `divertor`, `blanket`, `multiplier`, `shield`, `vacuum_vessel`, `tf_magnet`, `pf_magnet`, `cs_magnet`, `structure`, `port_plug`, `vacuum`. |
| `replacement_group_id` | string or `null` | `null` means the solid is permanent. |
| `tally` | boolean | Score per-solid flux and heating. Defaults to `true`. Turning it off for small structural pieces saves memory. |

Role rules, checked at import:

- exactly one `plasma_chamber`, with material `void`;
- `vacuum` solids have material `void`;
- `void` is allowed only in `plasma_chamber`, `vacuum` and `port_plug` solids;
  other roles must have a material, so an empty blanket cannot pass unseen;
- every void solid is listed in the report (GEO-039, NUC-056).

### `replacement_groups[]`

| Key | Type | Meaning |
| --- | --- | --- |
| `id`, `label` | string | Group identity. |
| `limits` | array | One or more governing limits (decision 9). |
| `replacement_duration` | object | `{"kind": "fixed", "duration_s": …, "provenance": {…}}`, or `{"kind": "computed"}` once the S3 dose rule exists. Until then, `computed` is NOT_EVALUATED, with why and next step. |

A limit:

| Key | Type | Meaning |
| --- | --- | --- |
| `metric` | enum | `fast_neutron_fluence`, `neutron_fluence`, `displacements_per_atom`, `helium_appm`, `nuclear_heating_density`. |
| `energy_threshold_ev` | number | Required for `fast_neutron_fluence` (for example `1e5` for REBCO). |
| `limit` | number | In the metric's SI unit: n/m², dpa, appm, W/m³. |
| `averaging` | object | `{"kind": "group_average"}` or `{"kind": "peak_mesh", "voxel_m": [dr, dz, dphi_or_null]}`. `null` means the full solid width in that direction, as in the risk-test protocol. |
| `provenance` | object | As for materials. A literature screening value says so in its note, as the 0.2 magnet limit did. |

Every solid that names a group must exist, and every group must have at least
one solid.

### `plasma`

Field names match `openmc_plasma_source.tokamak_source` (0.9.0), converted to
SI. FARIS converts lengths to centimetres when it calls the package.

| Key | Meaning |
| --- | --- |
| `chamber_solid_id` | The `plasma_chamber` solid. |
| `major_radius_m`, `minor_radius_m`, `elongation`, `triangularity`, `shafranov_shift_m`, `pedestal_radius_m` | Shape (SRC-011). |
| `mode` | `"L"`, `"H"` or `"A"`. |
| `ion_density_m3` | `{centre, pedestal, separatrix, peaking_factor}`. |
| `ion_temperature_ev` | `{centre, pedestal, separatrix, peaking_factor, beta}`. |
| `fuel` | `{"D": 0.5, "T": 0.5}`. |
| `fusion_power_mw` | Total D-T fusion power. Source strength S = P/(Q·e) with Q = 17.6 MeV (SRC-001). The neutron power fraction 14.1/17.6 is shown wherever neutron power is used (SRC-002). |
| `emissivity_table` | Optional `{path, sha256}`. It replaces the profiles (SRC-004). |
| `provenance` | One object for the block, plus per-field overrides in `field_provenance`. |

The profile integral sets only the shape of the source. Its absolute strength
comes from `fusion_power_mw` alone. FARIS reports the profile's own implied
fusion power next to the declared value (SRC-010 closure), and a difference
above 1e-3 relative is shown, not hidden.

### `operating_scenario`

`{"path": "…operating-history.json", "sha256": "…"}`. The file is a
`faris-operating-history/v0.1` document. Its `service_limits` are not used for
CAD designs, which take their limits from `replacement_groups`. A design run
with a scenario that has `service_limits` stops, so the two sets of limits can
never disagree without notice.

## Import checks, in order

Each failure names the item, why it failed, and the next step. Nothing is
repaired (GEO-027).

1. The JSON parses, matches the schema, and has no unknown keys and no nulls in
   required fields.
2. The STEP hash matches, the unit agrees, and the extent is full.
3. The STEP solid count equals the number of `solids[]`. Each `step_index`
   fingerprint matches within 1e-6 relative volume, and the centroid matches
   within 1e-6 of the model's bounding-box diagonal.
4. Ids are unique and valid. Every reference resolves (materials, groups, the
   chamber).
5. Role and material rules pass. The material audit passes.
6. Conversion runs as a bounded job. The tolerance, `cad_to_dagmc` and
   OpenCASCADE versions, and the mesh statistics are recorded (GEO-012,
   GEO-013, GEO-019).
7. Each DAGMC volume matches exactly one solid by fingerprint, and its tag
   agrees. Faceted volume is within 0.5 % of the CAD volume (GEO-030). The
   faceting deviation is reported (GEO-032).
8. The integrity checks from the plan (GEO-020 to GEO-025) run.
9. Source sites sampled from `plasma` all lie in the chamber solid (SRC-018).

Checks 1 to 5 need no CAD kernel beyond reading the STEP file, so they run in
seconds and catch most user errors before the slow steps.

## Example (trimmed)

```json
{
  "schema_version": "faris-design/v0.1",
  "id": "rm-m",
  "title": "ARC-like reference model RM-M",
  "description": "Medium reference model for S0 and S1.",
  "cad": {
    "step_file": "rm-m.step",
    "step_sha256": "…",
    "length_unit": "m",
    "extent": { "kind": "full" },
    "machine_axis": "z",
    "faceting_tolerance_m": null,
    "implicit_complement_material_id": "void"
  },
  "materials": [
    { "id": "tungsten", "catalog_id": "tungsten-natural" },
    {
      "id": "rebco-winding-pack",
      "label": "TF winding pack (Cu, steel, REBCO)",
      "density_kg_m3": "…",
      "composition_basis": "weight",
      "components": [ "…" ],
      "temperature_k": null,
      "provenance": { "label": "published", "reference_id": "sorbom-2015", "note": "45.9 % Cu, 46.1 % steel, 8.0 % REBCO by volume, converted to weight." }
    }
  ],
  "solids": [
    { "id": "plasma", "step_index": 0, "step_name": "plasma", "fingerprint": { "cad_volume_m3": 141.6, "centroid_m": [0, 0, 0] }, "material_id": "void", "role": "plasma_chamber", "replacement_group_id": null, "tally": false },
    { "id": "tf-01", "step_index": 12, "step_name": null, "fingerprint": { "cad_volume_m3": 2.91, "centroid_m": [3.62, 0.0, 0.0] }, "material_id": "rebco-winding-pack", "role": "tf_magnet", "replacement_group_id": "tf-coils", "tally": true }
  ],
  "replacement_groups": [
    {
      "id": "tf-coils",
      "label": "All 18 TF coils",
      "limits": [
        { "metric": "fast_neutron_fluence", "energy_threshold_ev": 1e5, "limit": 3e22, "averaging": { "kind": "peak_mesh", "voxel_m": [0.05, 0.10, null] }, "provenance": { "label": "published", "reference_id": "sorbom-2015", "note": "REBCO screening value, not a qualified allowable." } }
      ],
      "replacement_duration": { "kind": "computed" }
    }
  ],
  "plasma": { "chamber_solid_id": "plasma", "major_radius_m": 3.3, "minor_radius_m": 1.13, "elongation": 1.84, "triangularity": 0.375, "fusion_power_mw": 525, "…": "…" },
  "operating_scenario": { "path": "rm-m.operating-history.json", "sha256": "…" },
  "references": [ { "id": "sorbom-2015", "title": "ARC: a compact, high-field, fusion nuclear science facility…", "url": "https://arxiv.org/abs/1409.3540", "use": "Plasma shape, inboard build, winding-pack composition, REBCO screening fluence." } ],
  "assumptions": [ "Outboard TiH2 thickness is authored." ]
}
```

The numbers in the example only illustrate the format. RM-M's real design file
is written by `--design-init` from its STEP file once the model passes
acceptance.

## Open points

These do not block the format. Each is settled when its stage needs it.

- **Remote-handling inputs** (dose-rate limit, number of systems, ports) belong
  to the maintenance scenario in S3, not to the design.
- **Homogenised solids** (GEO-033): a solid whose material is a smeared mix
  needs its constituents listed. A `homogenised_from` field is added in S1 if
  RM-M's winding pack needs it, and is otherwise deferred.
- **Sector models:** see decision 5.
