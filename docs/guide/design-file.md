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
   surface (GEO-032); a deviation larger than the faceting tolerance stops the import. Planes, cylinders, cones,
   spheres and tori are measured at every sample point in closed form; other surfaces (splines and the like) at an
   even stride of at most 2000 points per solid. The report says, for each solid, which it was: "maximum over all
   points" or "sampled maximum".
8. **The faceted model is sound.** FARIS reads the converted model itself and checks it with its own code, using
   exact arithmetic for every yes-or-no decision. It stops on an edge used once or three times (a hole or a fold),
   neighbouring triangles that disagree about the inside, a volume with negative or zero volume, a solid thinner
   than the faceting can represent, or whose faceted thickness is off by more than a tenth (a sliver, see below), a solid that intersects itself, two surfaces that coincide but were
   not merged, overlapping solids, an enclosed region that belongs to no solid (a gap), sampled points that are
   in two solids or in a gap, random chords that cross a surface that does not lead out of the region they are in
   (a particle there would be lost), and a sum of volumes that disagrees with the sampled volume of the whole.
   Every finding gives the solids, a position in metres, why it stops the import and what to change in your CAD
   tool. Nothing is repaired.
9. **The plasma source lies inside the chamber.** FARIS samples 100,000 source sites from the plasma block with
   OpenMC's plasma source and tests each against the `plasma_chamber` volume. All must be inside. The report
   also gives the smallest distance from any site to a `first_wall` solid and the site that has it
   (`Clearance to the first wall`). No minimum is set: only a site inside the first wall stops the import, and a
   design with no `first_wall` solid says the clearance was not measured.

   Once checks 1 to 5 pass, whatever stops checks 6 to 9, the report also adds a line `Source closure`: the D-T fusion power that the
   plasma block's ion density and temperature profiles imply, next to `fusion_power_mw`. Transport takes its
   source strength from `fusion_power_mw` alone and the profiles only set the shape, so if the two differ by more
   than 1e-3 relative the line reads `EXCEEDED` and a notice gives both numbers. It does not stop the import and
   FARIS rescales neither value: correct the profiles, or set `fusion_power_mw` to the profiles' value if the
   profiles are the plasma you mean. The profile power counts D-T reactions only, with the profile shapes and
   the Bosch-Hale reactivity that `openmc_plasma_source` uses.

Every void solid is listed in the report, because a void region hides streaming paths.

### Options

| Option | Meaning |
| --- | --- |
| `--work-dir DIR` | Keep the conversion and its products here. A conversion, a pre-imprint tessellation and the overlap boolean results found here are reused only when the STEP hash, the tags, the tolerances and the helper version all match; `--no-reuse` redoes all of them. Without it, a temporary directory is used and removed. |
| `--threads N` | Worker threads for check 8. Default: the available cores, at most 16. |
| `--seed N`, `--samples N`, `--chords N`, `--source-sites N` | The sampling seed and sizes. The same seed gives the same report. Defaults 1, 1,000,000, 100,000, 100,000. |
| `--deviation-points N` | Points per solid that OpenCASCADE projects onto surfaces that are not planes, cylinders, cones, spheres or tori, for the faceting deviation of check 7. Default 2000, on an even stride; 0 projects them all, which can take hours on a model with many spline surfaces. Points on the five closed-form surface types are always all measured. The report says for each solid whether the result is a maximum over all points or a sampled maximum. |
| `--imprint-threads N` | Threads the converter may use. Default 4. |
| `--cad-python`, `--openmc-python`, `--plasma-source-path DIR` | The interpreters and the folder holding `openmc_plasma_source`. |
| `--cad-reader auto\|native\|python`, `--occt-worker PATH` | Which reader runs the STEP inspect and the per-solid tessellation. `native` is the OpenCASCADE worker `faris-occt-worker` (found from `--occt-worker`, then `FARIS_OCCT_WORKER`, then next to `faris`); it needs no Python for those two stages. `python` is the cadquery helper. `auto`, the default, uses the worker when there is one and it answers, and the helper otherwise. `FARIS_CAD_READER` sets the default. The conversion, the overlap booleans and the faceting deviation still use the Python helper. The report says which reader ran each stage; a result recorded in `--work-dir` by one reader is not reused for the other. |

Progress goes to the terminal. Ctrl-C cancels cleanly; a cancelled run reports nothing partial.

## Sliver findings

A sliver finding comes in two kinds. Both report the solid's mean thickness 2V/A (twice its faceted volume over its
surface area), both block, and both can be waived with the check id `SLIVER`; one waiver for a solid covers both.

- **"sliver (thinner than the faceting)".** 2V/A is below the faceting tolerance (`cad.faceting_tolerance_m`). The
  facets cannot represent a body that thin, so its volume and its boundaries are unreliable. Either set the
  tolerance below the solid's thickness and convert again, thicken the solid or merge it into its neighbour in the
  CAD tool, or accept it with a waiver if it does not matter to your result.
- **"sliver (thickness misrepresented)".** The faceting deviation of check 7 for this solid is more than 10 % of 2V/A.
  The solid is thick enough to be faceted, but the facets sit far enough from the CAD surface that the faceted layer's
  thickness may be off by more than a tenth. Set the faceting tolerance smaller and convert again; a thin curved layer
  needs a tighter tolerance than a thick one. The finding says "maximum over all points" or "sampled maximum" for the
  deviation. This rule needs check 7's deviation, so when that measurement is unavailable the report says the rule
  could not run.

A thin layer that is faceted accurately (a 2 mm tungsten armour layer faceted to 0.1 mm) is not a sliver.

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

## Check a design from the desktop

Available from the next release. Open **Design check** in the top bar. The window does the two steps above without a
terminal: it runs the same functions as `faris design init` and `faris design check` (see [Command line](cli.md)).
You can also open it at launch with `--design-check`, with `--check-step FILE` and `--check-design FILE` to fill in the
files.

**1. Start from a STEP file.** Choose the STEP file and, if you want it elsewhere, the design file to write (empty:
beside the STEP file, as the command line does). If the STEP file declares no length unit, choose one. Press **Read the
STEP file**. The window then shows the path of the file it wrote, how many fields are still empty, grouped in plain words
(the material and role of each solid, the plasma block, the operating-history link), and what else the file needs
(materials, replacement groups, labels). The window has no editor for the design file yet: open the file in any text
editor and fill it in as described under Step 2 above. The design file you just wrote is filled into the second section.

**2. Check a design.** Choose the design file. The **work folder** and the **report** have defaults beside the design
file (`NAME-check-work` and `NAME-check-report.json`, numbered if it exists). Unlike the command line, which uses a
temporary folder and removes it, the window keeps the work folder: the flux step reads the conversion from it, and a second
check reuses what matches (tick **Do everything again** to redo it all). A report is never overwritten.

The list of tools says, for each tool, whether it is set, what it is for and what the check loses without it. The CAD
Python is required. Without the OpenMC Python, the cross sections or the plasma source folder the check still runs, but
check 5's nuclear-data audit or check 9 says `NOT_EVALUATED`, the import is not complete, and the flux cannot be run. Anything
that stops the check is marked beside its field, once, and the button area only says how many things need fixing.

While it runs, the window shows the time since you pressed **Check** and the engine's stage line (`stage: done of total`),
with a bar for that stage only. It gives no estimate for the whole check, because its stages differ in length and the
conversion, the STEP reading and the material audit report no count while they work. **Cancel** stops the check and the programs
it started and writes no report; the work folder is left as it is.

When the check ends, the window shows the summary line, then the nine checks, each with its status word (`PASS`, `FAIL` or
`NOT_EVALUATED`) and, for each finding, the item, why and next step as the engine words them (a long list scrolls inside
the window). It also shows the notices and what was measured, each with its unit: the number of solids, the conversion
tolerance and triangle count, the largest volume difference against its gate, the faceting deviation against its tolerance,
the source sites inside the chamber, the clearance to the first wall and the source closure with both power numbers. A
record the engine could not measure says which check did not get there. Nothing is shown before a check has finished.

When all nine checks pass, **Run the flux for this design** opens the Design flux window with this design file and this
report already filled in.

## Run the flux from the desktop

Available from the next release. Open **Design flux** in the top bar. The window runs the same computation as
`faris design transport` (see [Command line](cli.md)); the command line stays the way to script it.

1. Choose the design file and the JSON report `faris design check --report` wrote for it. The check's work folder must
   still hold the conversion. You can also pass them at launch with `--design-flux --flux-design FILE
   --flux-check-report FILE`.
2. Under **Tools the run needs**, set the OpenMC Python, the cross sections and the plasma source folder. FARIS fills
   them from `FARIS_OPENMC_PYTHON`, `FARIS_CROSS_SECTIONS` and `FARIS_PLASMA_SOURCE_PATH` when those are set. Each row says
   whether it is set, what it is for and what to do if it is missing. The window Python (OpenMC 0.16 or later) and the
   OpenMC package folder are optional; the row says what the run loses without them (weight windows, aiming).
3. Set the target error (5 % by default), the history limit and the time limit, then press **Run**. Anything that stops
   the run is listed under the form with what to do next.

While it runs, the window shows the current step in plain words. Once sampling starts it shows the elapsed time, the error
on the mean against the target and, when the limit asks for a peak, the error on the highest piece. From the second batch it
shows a progress bar and a sentence marked **Estimate**: the error falls as 1 over the square root of the histories, so the
histories still needed follow from the error reached and the target. If the history or time limit will come first, the
sentence says so. The errors shown are provisional while the run goes. **Cancel** stops OpenMC and writes no result; the run
folder is left as it is. You can close the window and reopen it while a run goes on.

When the run ends, the window shows the mean flux with its error, the highest solid, the highest piece with its place and
volume when the limit asks for a peak, each fluence limit with its status (WITHIN, EXCEEDED or INCONCLUSIVE), the fluence over
the history and the years to the limit with their error, and the why and next step for each. It also shows the speed-up mode
and why it ran, every note, and everything that was not evaluated with why. A number with no error says why it has none.
Nothing is shown before a run has finished. The report is also saved as `design-transport-report.json` in the run folder.

## What this version does not do

A design that passes checks 1 to 9 is checked and convertible, but not yet run: the transport and maintenance
studies that use it come in later releases. The integrity checks need the converted model to fit the 1 nm exact grid (about 270 m across); larger models are
refused with the reason. Check 8 finds defects on the faceted model; a defect smaller than the faceting tolerance can only be
seen through the sampled checks.
