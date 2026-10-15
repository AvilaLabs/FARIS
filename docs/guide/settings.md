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

Today the memory settings take effect in the command line. The six tool paths in your file are read by the desktop; the command-line commands that run tools still take them from their own flags and environment variables. The desktop reads `ui.scale` (the interface size, which its size menu saves here; an old `interface.txt` is moved into the file once) and shows where each tool path and the size come from. `ui.theme` is stored and reported, and the desktop does not read it yet.

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
| `ui.scale` | number 0.5 to 4 | `1` | x | - | Interface size as a multiple of the desktop's own scaling. The desktop reads it at start and saves it when you change the size; its size menu offers 0.75 to 2. |
| `ui.theme` | one of system, light, dark | `system` | - | - | Colour theme. Reserved for the desktop, which does not read it yet. |
