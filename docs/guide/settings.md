<!-- kind: reference -->
# Settings

Settings change how FARIS behaves on your machine: where your tools are, how much memory a job may use, how the desktop looks. They never change what a scenario means or what a calculation gives. Anything that would change a result belongs in the study's own inputs, and the settings file refuses it.

## Where a value comes from

A setting takes the first value found, looking from the strongest layer to the weakest:

1. A command-line flag: `--setting KEY=VALUE`, which any `faris` command accepts and which can be repeated. The tool flags of single commands (for example `--cad-python`) still win for that command.
2. An environment variable, where the table below lists one. An empty variable counts as not set.
3. Your settings file.
4. The built-in default.

`faris config get KEY --show-origin` names the layer, and the variable or file, that supplied the value. `faris config list` does it for every setting.

The `units.*` and `format.*` settings choose how quantities, numbers and dates are shown and typed, never what is stored; see [Units and number formats](units.md).

Today the memory settings take effect in the command line. The six tool paths in your file are read by the desktop; the command-line commands that run tools still take them from their own flags and environment variables. The desktop reads `ui.scale` (the interface size, which its size menu saves here; an old `interface.txt` is moved into the file once) and shows where each tool path and the size come from. The desktop also reads `ui.theme` at start: `high-contrast` selects the high-contrast theme (the browser build reads the same choice from its browser storage key `faris-theme`); the other values give the ordinary dark theme.

## Your settings file

It is one JSON file in your per-user configuration folder, the same folder the desktop uses: `~/.config/faris/settings.json` (or under `$XDG_CONFIG_HOME`) on Linux, `~/Library/Application Support/faris/settings.json` on macOS, `%APPDATA%\faris\settings.json` on Windows. Set `FARIS_CONFIG_DIR` to an absolute folder to use that folder instead, on every platform. `faris config path` prints the exact path.

The file carries a `schema` number. FARIS refuses a file from a newer version, and an unknown setting name, with a message that says why and what to do; it never ignores them. Every problem in the file, in an environment variable or in a flag is reported in one message, and nothing starts until they are fixed. `faris config` itself still runs, so you can repair the file with it.

| Command | What it does |
| --- | --- |
| `faris config list [--json]` | Every setting with its value, unit and layer. |
| `faris config get KEY [--show-origin]` | One effective value, and which layer supplied it. |
| `faris config set KEY VALUE` | Checks the value, then writes it to your file. It says when a stronger layer still wins. |
| `faris config unset KEY` | Removes your value, so the environment or default applies again. |
| `faris config reset --yes` | Replaces your file with an empty one, which is the way out of a damaged file. |
| `faris config path` | Where your file is. |

## All settings

The table is checked against the program's list of settings by a test, so it cannot drift.

| Key | Type | Default | Unit | Environment variable | What it does |
| --- | --- | --- | --- | --- | --- |
| `tools.cad_python` | path | not set | - | `FARIS_CAD_PYTHON` | Python interpreter with cadquery, used to read STEP models when the native reader is not used. |
| `tools.openmc_python` | path | not set | - | `FARIS_OPENMC_PYTHON` | Python interpreter with OpenMC, used for transport runs. |
| `tools.cross_sections` | path | not set | - | `FARIS_CROSS_SECTIONS` | The cross_sections.xml file of the nuclear data library OpenMC uses. |
| `tools.plasma_source` | path | not set | - | `FARIS_PLASMA_SOURCE_PATH` | Folder holding the openmc_plasma_source package, for the plasma source sampler. |
| `tools.window_python` | path | not set | - | - | Python interpreter with OpenMC 0.16 or later, used to make weight windows. |
| `tools.openmc_package` | path | not set | - | - | Folder of an OpenMC built with the aiming patch. |
| `tools.cad_reader` | one of auto, native, python | `auto` | - | `FARIS_CAD_READER` | Which reader opens STEP models: the native worker, the Python helper, or whichever is found. |
| `tools.occt_worker` | path | not set | - | `FARIS_OCCT_WORKER` | The native CAD worker program; when unset, the one beside the running program. |
| `tools.core_executable` | path | not set | - | `FARIS_CORE_EXECUTABLE` | The Avila Core program that compiles and runs studies; when unset, the one on PATH. |
| `memory.cap_mb` | whole number 1 to 4194304 | not set | MiB | `FARIS_MEMORY_CAP_MB` | Address-space cap applied to every external job; when unset, no cap is applied and a job is checked against the memory that is free. |
| `memory.margin_mb` | whole number 1 to 4194304 | `1024` | MiB | `FARIS_MEMORY_MARGIN_MB` | Memory kept free for the rest of the machine when the free-memory check is made before a job starts. |
| `units.system` | one of si, fusion | `si` | - | `FARIS_UNITS` | Unit system for displayed and typed quantities: si, or fusion (MW, keV, cm, GWd, dpa and related practical units). Stored files always stay in SI base units. |
| `units.length` | system, or one of m, cm, mm, km | `system` | - | - | Unit shown for length; system follows units.system. |
| `units.area` | system, or one of m2, cm2, barn | `system` | - | - | Unit shown for area and cross-section; system follows units.system. |
| `units.mass` | system, or one of kg, g, t | `system` | - | - | Unit shown for mass; system follows units.system. |
| `units.density` | system, or one of kg/m3, g/cm3 | `system` | - | - | Unit shown for mass density; system follows units.system. |
| `units.time` | system, or one of s, min, h, d | `system` | - | - | Unit shown for duration; system follows units.system. |
| `units.service_time` | system, or one of s, d, yr, FPY | `system` | - | - | Unit shown for time in service; system follows units.system. |
| `units.temperature` | system, or one of K, degC | `system` | - | - | Unit shown for temperature; system follows units.system. |
| `units.power` | system, or one of W, kW, MW, GW | `system` | - | - | Unit shown for power; system follows units.system. |
| `units.energy_particle` | system, or one of J, eV, keV, MeV | `system` | - | - | Unit shown for particle energy; system follows units.system. |
| `units.energy_plant` | system, or one of J, kWh, MWh, GWh, MWd, GWd | `system` | - | - | Unit shown for plant energy; system follows units.system. |
| `units.power_density` | system, or one of W/m3, W/cm3, MW/m3 | `system` | - | - | Unit shown for power density; system follows units.system. |
| `units.heat_flux` | system, or one of W/m2, kW/m2, MW/m2, W/cm2 | `system` | - | - | Unit shown for surface power; system follows units.system. |
| `units.particle_flux` | system, or one of n/m2/s, n/cm2/s | `system` | - | - | Unit shown for particle flux; system follows units.system. |
| `units.fluence` | system, or one of n/m2, n/cm2 | `system` | - | - | Unit shown for fluence; system follows units.system. |
| `units.damage` | system, or one of dpa | `system` | - | - | Unit shown for displacement damage; system follows units.system. |
| `units.damage_rate` | system, or one of dpa/s, dpa/yr, dpa/FPY | `system` | - | - | Unit shown for damage rate; system follows units.system. |
| `units.dose` | system, or one of Sv, mSv, rem | `system` | - | - | Unit shown for dose equivalent; system follows units.system. |
| `units.dose_rate` | system, or one of Sv/s, Sv/h, mSv/h, uSv/h, rem/h | `system` | - | - | Unit shown for dose-equivalent rate; system follows units.system. |
| `units.absorbed_dose` | system, or one of Gy, mGy, rad | `system` | - | - | Unit shown for absorbed dose; system follows units.system. |
| `units.absorbed_dose_rate` | system, or one of Gy/s, Gy/h, mGy/h | `system` | - | - | Unit shown for absorbed-dose rate; system follows units.system. |
| `units.activity` | system, or one of Bq, kBq, MBq, GBq, TBq, mCi, Ci | `system` | - | - | Unit shown for activity; system follows units.system. |
| `units.fraction` | system, or one of 1, %, appm | `system` | - | - | Unit shown for fraction; system follows units.system. |
| `units.burnup` | system, or one of J/kg, MWd/kg, GWd/t | `system` | - | - | Unit shown for burn-up; system follows units.system. |
| `units.magnetic_field` | system, or one of T, mT, G | `system` | - | - | Unit shown for magnetic flux density; system follows units.system. |
| `units.pressure` | system, or one of Pa, kPa, MPa, bar, atm | `system` | - | - | Unit shown for pressure; system follows units.system. |
| `format.decimal` | one of point, comma | `point` | - | - | Decimal separator of displayed numbers. Stored files always use a point. |
| `format.grouping` | one of none, thin-space, comma, point, apostrophe | `none` | - | - | Digit-group separator of displayed numbers, from five integer digits up. It must differ from the decimal separator. |
| `format.date` | one of iso, dmy, mdy | `iso` | - | - | Date order of displayed dates: iso (2026-10-15), dmy (15.10.2026) or mdy (10/15/2026). Stored dates are always ISO. |
| `ui.scale` | number 0.5 to 4 | `1` | x | - | Interface size as a multiple of the desktop's own scaling. The desktop reads it at start and saves it when you change the size; its size menu offers 0.75 to 2. |
| `ui.theme` | one of system, light, dark, high-contrast | `system` | - | - | Colour theme, read by the desktop at start. high-contrast is white on black with 7 to 1 text contrast; system, dark and light all give the ordinary dark theme for now. |
| `backups.keep` | whole number 1 to 100 | `3` | versions | - | How many earlier versions of a study the desktop keeps beside the file each time you save. |
| `crash.keep_days` | whole number 1 to 365 | `30` | days | - | How long the desktop keeps crash records, diagnostics bundles and recovery files on this computer before deleting them. Nothing is sent anywhere. |
