<!-- kind: how-to -->
# Compare two studies or two models

`faris diff A B` compares two `.faris` study files, or two model files (a scenario, a physics case, operating assumptions or a design file). It lists every parameter that changed, says which results the change makes stale, and says which stay usable. Run it before you decide whether a new transport run is needed.

```bash
faris diff before.faris after.faris
faris diff before.faris after.faris --json --output diff.json
faris diff scenario-a.json scenario-b.json
```

The exit code is 0 when the two sides are the same, 6 when they differ, and 2 when a file cannot be read. This follows `diff`: a script can tell "different" from "failed".

## What counts as a change

A change is a different value. These are not changes, in a study or a model:

- the order of keys in a file;
- white space and line endings (a CRLF and an LF are the same text);
- the spelling of a number: `1`, `1.0`, `1e0` and `1.00` are one number;
- a number written as text with a decimal comma or a decimal point (`"1,5"` and `"1.5"`);
- saving the same study again.

A number that differs in its last binary digit is a change. FARIS does not round values before it compares them.

## What the report holds

The text and the JSON have the same parts, in the same order.

| Part | What is in it |
| --- | --- |
| Inputs changed | The causes. The scenario, the physics cases, the operating assumptions, and the inputs recorded with each transport run. Each change has a path, the old value, the new value, the unit (when the key name states one, such as `thickness_m`), a cost class, and where it applies. |
| Evidence changed | The Core evidence of the study and the nuclear data audit of each recorded run. |
| Results changed | The effects. A recorded transport run, or a stored uncertainty ensemble, whose content differs. Each links to the input changes that can explain it, or says "no input change found". |
| Presentation only | Labels, colours, descriptions, references, the view the author left open, who saved the file. These change no number. |
| Components | For a scenario: components added, removed or moved in the radial build, material changes, thickness changes, and the volume of every cell whose volume changed. |
| What this invalidates | Every recorded result, as invalidated or reusable, with what it costs to get it again. |

## Cost classes

Each input change has one of three classes. FARIS decides them in one place, and a path it does not know is treated as needing a new transport run, because a wrong "reusable" would show a stale result.

| Class | Meaning | Examples |
| --- | --- | --- |
| instant | Nothing is recalculated. | A label, a colour, a description, the view |
| seconds | The operating history and what depends on it are recalculated. The transport is reused. | The operating assumptions, the horizon in years |
| transport run | The transport has to be run again. | A thickness, a material, the major radius, the fusion power, the nuclear data, the seed |

A change to one allocation's layers invalidates that allocation's recorded transport and not the others. A change to the geometry shared by all allocations invalidates all of them.

## The JSON

`--json` writes `faris-diff/v1`. The schema name changes when a field name or meaning changes, so a script can check it. The output has no dates, no timings and no host names: the same two files give the same bytes. The fields are `schema`, `a`, `b`, `compared`, `identical`, `summary`, `inputs`, `evidence`, `results`, `presentation`, `components` and `impact`.

By default a changed result is reported by its hash. `--values` also lists the values that changed inside it (up to 200 per file). Result files are megabytes of numbers, so this is slower and the report is long.

## Compare two settings files

`faris config diff a.json b.json` compares two settings files. It lists each key whose effective value differs, with old value, new value, unit, the layer that supplied each side (default or user file) and the effect class of the setting: appearance, behaviour or result-changing. The header counts the result-changing differences. A setting written out at its default value is not a difference. The exit codes are the same: 0 identical, 6 different.

## The geometry table

```bash
faris geometry summary --scenario scenario.json --physics case.physics.json --output cells.csv
```

This writes one row for each cell of the radial build: name, id, variant, material, volume in m³, density, mass in kg, representation and kind label, then a `TOTAL` row. Volumes are calculated from the torus shell formula and are labelled `calculated`. They are the full torus shell: a penetration is not subtracted, and the note column says so. Without a physics case the mass is NOT_EVALUATED, not guessed. A declared void has no mass.

## How long it takes

The comparison reads each study's manifest first and skips any file whose hash is the same on both sides. Only the files that differ are decompressed and parsed. `faris diff A B --time` prints the time on standard error.

Measured with a debug build on a shared, busy laptop (not a reference machine), on the largest recorded study in the repository, a 6.1 MB study file with about 154 MB of recorded text in 61 parts:

| Comparison | Result | Time |
| --- | --- | --- |
| The study against itself | identical | 0.03 s |
| The 54 MB study with evidence against itself | identical | 0.02 s |
| The study against a copy written again from the same inputs | identical | 0.02 s |
| The study against a copy with one run's blanket and shield thickness edited | 4 input changes, 0 result changes | 0.37 s to 0.56 s over 20 runs |
| The study against a copy with one whole run replaced by another | 36 input changes, 1 result change | 0.66 s |
| The same replaced-run pair with `--values` (every changed number listed) | 870 lines of text | 2.9 s |

The 2 second target for a study of this size is met in these runs. A pair of 1 GB studies was not available, so the 10 second target for that size is not measured. A release build is faster than these debug figures.

## Not done yet

The desktop has no diff view yet, so COL-023 is met on the command line only. The comparison does not yet list the numbers inside changed results by default, and it does not mark a change below two standard errors as within noise. A merged study is not covered.

Next: [Command line](cli.md).
