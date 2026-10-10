# The design file (bring your own CAD)

> **Available from the next FARIS release.** FARIS 0.2.0 does not include this. The format and the checks
> described here are in the source tree and are not yet in a download.

A STEP file says where the solids are. It does not say what they are made of, what job they do, when they wear
out or where the plasma is. The **design file** says those things.

You give FARIS two files:

- `machine.step`, from your CAD tool.
- `machine.faris-design.json`, which FARIS starts for you and you finish.

FARIS reads nothing else about the design. The format is `faris-design/v0.1`.

This version only checks the files. It does not yet convert the model or run transport. See
[What this version does not do](#what-this-version-does-not-do).

## What you need

- The `faris` command, from the package.
- A Python with **cadquery**, to read the STEP file. FARIS does not install it. Point to it with
  `--cad-python PATH`, or set `FARIS_CAD_PYTHON`.
- For the nuclear-data check only: a Python with **openmc** and **h5py**, and the `cross_sections.xml` of the
  library you use. Point to them with `--openmc-python` and `--cross-sections`, or set `FARIS_OPENMC_PYTHON` and
  `FARIS_CROSS_SECTIONS`. Without them that one check says `NOT_EVALUATED`.

Both helpers run as separate processes with a memory limit and a time limit. Ctrl-C stops them.

## Step 1: write the first draft

```bash
faris design init machine.step
```

This writes `machine.faris-design.json` beside the STEP file. It never overwrites a file. Use `--out PATH` to write
somewhere else. (The desktop program does the same with `faris-app --design-init machine.step`.)

The draft already holds:

- every solid in the STEP file, with its read order (`step_index`), its STEP name if it has one, and a
  *fingerprint* (its volume and centroid);
- a suggested `id` for each solid: the STEP name in lower case with unsafe characters turned into hyphens, or
  `solid-007` when there is no name;
- the STEP file's hash and length unit;
- `extent` full, `machine_axis` z, and a plasma block with empty fields.

Every `material_id` and every `role` is `null`. So is every plasma value. FARIS never gives a missing value a
default.

If the STEP file declares no length unit, say what its numbers are in: `--length-unit mm`. The draft records that
the unit was assumed.

## Step 2: fill it in

Open the file in any editor. You do five things.

1. **Describe the materials.** Add them to `materials`. Name a catalog material, or write the recipe.

   ```json
   { "id": "tungsten", "catalog_id": "tungsten-natural" }
   ```

   ```json
   {
     "id": "winding-pack",
     "label": "TF winding pack",
     "density_kg_m3": 8500.0,
     "composition_basis": "weight",
     "components": [
       { "element": "Cu", "fraction": 0.7 },
       { "element": "Fe", "fraction": 0.3 }
     ],
     "temperature_k": null,
     "provenance": { "label": "authored", "note": "Homogenised mix." }
   }
   ```

   Fractions must sum to 1. An element without `isotopes` uses natural abundance. Empty space is the built-in
   material `void`, not a material of yours.

2. **Give every solid a material and a role.** Set `material_id` and `role` on each solid. Roles are
   `plasma_chamber`, `first_wall`, `divertor`, `blanket`, `multiplier`, `shield`, `vacuum_vessel`, `tf_magnet`,
   `pf_magnet`, `cs_magnet`, `structure`, `port_plug` and `vacuum`.

   You may rename a solid's `id`. Ids use 1 to 28 lower-case letters, digits and hyphens. `vacuum`, `graveyard`
   and `void` are reserved, in any letter case.

3. **Group the parts you replace together.** Add a replacement group and set `replacement_group_id` on its
   solids. A solid you never replace keeps `null`. A group has one or more governing limits. The first limit
   reached triggers the replacement.

   ```json
   {
     "id": "tf-coils",
     "label": "All TF coils",
     "limits": [
       {
         "metric": "fast_neutron_fluence",
         "energy_threshold_ev": 1e5,
         "limit": 3e22,
         "averaging": { "kind": "peak_mesh", "voxel_m": [0.05, 0.10, null] },
         "provenance": { "label": "published", "reference_id": "sorbom-2015",
                         "note": "Screening value, not a qualified allowable." }
       }
     ],
     "replacement_duration": { "kind": "computed" }
   }
   ```

   `null` in `voxel_m` means the full width of the solid in that direction. A published value needs a
   `reference_id` that matches an entry in `references`.

4. **Describe the plasma.** Fill the `plasma` block and set `chamber_solid_id` to the id of the plasma chamber.

5. **Link the operating history.** Set `operating_scenario` to the path and sha256 of a
   `faris-operating-history/v0.1` file. That file must not have `service_limits`: a CAD design takes its limits
   from the replacement groups.

Label every number you supply as `published` (with a reference) or `authored`. FARIS keeps the two apart in every
report.

## Step 3: check it

```bash
faris design check machine.faris-design.json --report check.json
```

It prints one block per check with `PASS`, `FAIL` or `NOT_EVALUATED`, and writes the full report as JSON. It stops
at the first stage that fails, so you fix a few things at a time. Every failure names the item, says why it failed
and tells you the next step. Nothing is repaired for you.

| Exit status | Meaning |
| --- | --- |
| 0 | No check failed. |
| 1 | A check failed. |
| 2 | A usage or file error, or a helper could not run. |

The JSON report also records the hash of the design file and of the STEP file, the versions of the helpers and
the interpreter that ran them, and every void solid.

## What each check means

1. **The file parses.** The JSON is valid and matches the format. A misspelt or unknown key stops the import, so a
   typo is never ignored. A `null` in a field that must be filled stops it too, and FARIS lists every null by
   path. A fresh draft always stops here until you finish it.
2. **The STEP file is the one you described.** Its hash matches. The length unit it declares matches
   `length_unit`. A wrong unit would scale every volume by a thousand or more without a warning. FARIS reads the
   unit two independent ways and stops if they disagree or the file declares several units. The model must be a
   full 360 degrees and sit on the z axis. The linked operating-history file must exist, match its hash and have
   no `service_limits`.
3. **Every solid is accounted for.** The STEP file has as many solids as the design file lists, and no solid is
   dropped. Each entry's fingerprint matches its solid: volume within 1 part in a million, centroid within 1
   millionth of the model's bounding-box diagonal. This catches a STEP file that was edited after the design file
   was written. Two solids that cannot be told apart by fingerprint stop the import.
4. **Ids and references.** Ids are unique and valid. Every material, group, reference and the plasma chamber you
   name exists. Every group has at least one solid.
5. **Roles and materials.** There is exactly one `plasma_chamber`, and its material is `void`. `vacuum` solids are
   `void`. Only `plasma_chamber`, `vacuum` and `port_plug` may be `void`, so an empty blanket cannot pass unseen.
   Densities are positive and fractions sum to 1. FARIS then expands every material into its nuclides and checks
   that each is in your nuclear-data library. A missing nuclide stops the run. If OpenMC is not configured, this
   part is `NOT_EVALUATED`, never `PASS`.

6. **The STEP model converts.** The CAD helper meshes every solid, imprints touching surfaces and writes a DAGMC
   model, as a bounded job (memory cap, time limit, cancel). Each solid gets the material tag `mat:<solid id>`.
   The faceting tolerance is 0.1 cm and 0.1 rad unless `faceting_tolerance_m` in the design file says otherwise.
   The report records both, the versions of `cad_to_dagmc` and OpenCASCADE, the wall time, the peak memory, the
   triangle count of every volume and the hash of the converted file. Before imprinting, FARIS also looks for
   solids that overlap in the CAD model itself (OpenCASCADE's boolean common of every candidate pair). A pair
   whose common volume is more than 1e-6 of the smaller solid is an overlap; a pair the kernel cannot decide is
   reported as undecided and stops the import.
7. **Every converted volume is one of your solids.** Volumes are matched to solids by faceted volume (within 0.5 %)
   and centroid (within 1e-3 of the model's bounding-box diagonal). The next-best solid must be at least ten times
   farther away, and the material tag on the volume must be the matched solid's id. Each solid and each volume
   is matched exactly once. FARIS also reports the largest distance between the faceted surface and the CAD
   surface (GEO-032, a sampled maximum); a deviation larger than the faceting tolerance stops the import.
8. **The faceted model is sound.** FARIS reads the converted model itself and checks it with its own code, using
   exact arithmetic for every yes-or-no decision. It stops on an edge used once or three times (a hole or a fold),
   neighbouring triangles that disagree about the inside, a volume with negative or zero volume, a solid thinner
   than ten faceting tolerances (a sliver), a solid that intersects itself, two surfaces that coincide but were
   not merged, overlapping solids, an enclosed region that belongs to no solid (a gap), sampled points that are
   in two solids or in a gap, random chords that cross a surface that does not lead out of the region they are in
   (a particle there would be lost), and a sum of volumes that disagrees with the sampled volume of the whole.
   Every finding gives the solids, a position in metres, why it stops the import and what to change in your CAD
   tool. Nothing is repaired.
9. **The plasma source lies inside the chamber.** FARIS samples 100,000 source sites from the plasma block with
   OpenMC's plasma source and tests each against the `plasma_chamber` volume. All must be inside.

Every void solid is listed in the report, because a void region hides streaming paths.

### Options

| Option | Meaning |
| --- | --- |
| `--work-dir DIR` | Keep the conversion and its products here. A conversion found here is reused only when the STEP hash, the tags and the tolerances all match; `--no-reuse` converts again. Without it, a temporary directory is used and removed. |
| `--threads N` | Worker threads for check 8. Default: the available cores, at most 16. |
| `--seed N`, `--samples N`, `--chords N`, `--source-sites N` | The sampling seed and sizes. The same seed gives the same report. Defaults 1, 1,000,000, 100,000, 100,000. |
| `--deviation-points N` | Points per solid for the faceting deviation of check 7. Default 0 measures every triangle centroid and edge midpoint, which takes hours on a model with many spline surfaces (a 30-solid fusion model with 1.1 million triangles did not finish in two hours). A positive number takes an even stride of the points, and the report says so. |
| `--imprint-threads N` | Threads the converter may use. Default 1. |
| `--cad-python`, `--openmc-python`, `--plasma-source-path DIR` | The interpreters and the folder holding `openmc_plasma_source`. |

Progress goes to the terminal. Ctrl-C cancels cleanly; a cancelled run reports nothing partial.

## Waivers

Some findings are real but acceptable to you, for example a deliberate thin layer. You can accept one by adding an
entry to the design file's optional `waivers` array:

```json
"waivers": [
  {
    "check_id": "SLIVER",
    "solid_ids": ["first-wall"],
    "reason": "A 2 mm sacrificial layer, modelled on purpose.",
    "author": "A. Designer",
    "date": "2026-10-10"
  }
]
```

A waiver names the finding's class, the solids it covers (all of them, as a set) or, for a gap, the point FARIS
reported (`"point_m": [x, y, z]`, to a nanometre), a reason, who accepted it and when. A waiver matches exactly;
nothing matches loosely. A waived finding stays in the report and in every receipt, marked with the waiver, and
no longer stops the import. A waiver that covers nothing is reported.

You can waive `GEO-021` (overlap), `GEO-022` (gap), `GEO-024`, `GEO-025`, `GEO-031`, `GEO-032`, `SLIVER`,
`SELF-INTERSECTION` and `DUPLICATE-SURFACE`. A gap is found by two checks that share a position; one waiver covers
both. You cannot waive an open or non-manifold surface (`GEO-020`), inverted normals, a zero-volume solid or a
degenerate triangle, because transport is not defined for them. You also cannot waive a failed conversion
(`GEO-012`) or source sites outside the chamber (`SRC-018`).

## What this version does not do

A design that passes checks 1 to 9 is checked and convertible, but not yet run: the transport and maintenance
studies that use it come in later releases. The integrity checks need the converted model to fit the 1 nm exact grid (about 270 m across); larger models are
refused with the reason. Check 8 finds defects on the faceted model; a defect smaller than the faceting tolerance can only be
seen through the sampled checks.
