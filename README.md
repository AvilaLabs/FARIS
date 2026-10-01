# FARIS

**Fusion Analysis and Reactor Integration Simulator — by Avila Labs.**

FARIS is a Rust project for exploring how a fusion plant's components, fuel
supply, operating history, and maintenance affect one another over its life.
It will combine existing open-source scientific tools with new models where
the chosen research question exposes a gap.

The first target is a narrow native desktop demo: one ARC-inspired compact
D-T tokamak, two blanket/shield arrangements, and a shared 3D scene. The app
uses **egui/eframe for the interface and wgpu for the 3D viewport**, with an
outliner, component inspector, central viewport, and timeline inspired by
Blender's workspace layout.

## Current implementation

- A Rust workspace with separate model, engine, CLI, and desktop crates.
- One versioned scenario with two allocations inside the same radial envelope.
- Strict scenario parsing, geometric validation, and source-file identity.
- Shared full-torus geometry records and display meshes.
- Native GPU rendering, orbit/zoom, cutaway, component picking, visibility,
  arrangement switching, and component dimensions.
- A timeline selector prepared for future operating-history results.
- CLI validation, deterministic geometry exports, and optional tool detection.
- Strict transport requests and raw-tally imports, checked source normalization,
  integrated rates, volume averages, units, and Monte Carlo standard errors.
- A Rust external-job runner with time/log bounds and process-group cancellation.
- An independently specified absorber-sphere control that actually runs OpenMC,
  with identified raw inputs/statepoints and scoped numerical comparisons.
- A scientific baseline, benchmark route, and local tool/data readiness audit.
- Typed material, source, and nuclear-data inputs bound to exact scenario bytes.
- Generated study dependencies, actual external Avila Core compilation, saved
  compiler reports, and a native Compile study button with cancellation.

**The desktop remains a geometry scaffold.** Reactor transport, activation,
ageing, fuel inventory, maintenance, and power production are not implemented. Their result
records say `NOT_EVALUATED`; the app leaves their values empty. Component
colors identify layers and do not represent calculated physical fields.
The bundled starting scenario has unassigned materials. The idealized circular
tori and magnet envelope do not reproduce the published ARC engineering design.

## Run the desktop

Rust 1.98.1 is pinned. The first build needs network access to crates.io unless
dependencies are already cached. Run from this directory:

```bash
cargo run -p faris-app
```

The scenario is embedded, so the desktop also works after its binary is moved.
Load an authored scenario explicitly:

```bash
cargo run -p faris-app -- --scenario scenarios/arc-inspired/scenario.json
```

Drag in the viewport to orbit, scroll to zoom, and click a component to inspect
it. Use the outliner to select or hide components. The cutaway is a display
operation; it does not change the full-torus model or reported volumes.

The desktop needs a graphical session and compatible graphics drivers.
Linux source builds may need `pkg-config`, `libxkbcommon-dev`, and
`libwayland-dev` from the system package manager. Windows and macOS builds
require their usual Rust native linker/toolchain setup.

Select analyses in the left panel and use **Compile study** in the upper right.
Choose an external Core executable in compiler settings or pass `--core`.
Compilation, run readiness, and scientific assessment are separate. See
[generated Core studies](docs/CORE_STUDIES.md) for the CLI and recorded evidence.

## Use the CLI

The default workspace member is the CLI, so headless operations do not build
the graphics stack. Python, OpenMC, and Avila Core are not required:

```bash
cargo run -- validate scenarios/arc-inspired/scenario.json
cargo run -- export --scenario scenarios/arc-inspired/scenario.json --output runs/geometry-001.json
cargo run -- doctor
```

Export refuses an existing destination to preserve previous run records.
`doctor` checks names on `PATH` without executing tools; detection does not
mean a FARIS adapter is available.

The first executable scientific increment is a mathematical transport control:

```bash
cargo run -- control absorber --python /path/to/openmc-env/bin/python \
  --openmc /path/to/openmc-env/bin/openmc --output runs/absorber-001
```

It uses synthetic one-group data, 1,000,000 histories and one thread by default.
The Rust worker caps histories, runtime and captured logs; Ctrl-C cancels its
solver process group. External execution currently requires Unix. A numerical
control PASS applies only to the declared control, with its sampling rule and
limitations in [NUMERICAL_CONTROLS.md](docs/NUMERICAL_CONTROLS.md). It establishes
no reactor prediction or nuclear-data qualification. Native solver memory and
disk usage are not sandboxed by this runner.

Transport imports are independently usable without OpenMC:

```bash
cargo run -- transport validate-request --scenario scenarios/arc-inspired/scenario.json --request /path/to/request.json
cargo run -- transport normalize --scenario scenarios/arc-inspired/scenario.json \
  --request /path/to/request.json --artifact /path/to/raw-tallies.json --output runs/normalized-001.json
```

An import checks definitions and arithmetic; the artifact's solver identities
and raw numbers are adapter assertions. Import success leaves scientific
qualification `NOT_EVALUATED`. See [TRANSPORT.md](docs/TRANSPORT.md) for contracts,
normalization, trust limits and unavailable reactor functionality.

## Development

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python3 -m unittest discover -s controls -p 'test_*.py'
```

Use `--jobs 1` when compiling the graphics stack on a memory-constrained machine.
For a headless core check, run `cargo test -p faris-model -p faris-engine -p faris-cli`.

An opt-in desktop rendering check captures only FARIS's own window and exits:

```bash
cargo run -p faris-app -- --capture runs/desktop-preview.png
```

The screenshot destination's directory must already exist. Screenshots are
development artifacts, not simulation evidence.

## Project map

| Path | Responsibility |
| --- | --- |
| `crates/faris-model/` | Scenario types, units, validation, and identities |
| `crates/faris-engine/` | Shared geometry, display mesh generation, result records, future simulation operations |
| `crates/faris-cli/` | Headless client of the same engine |
| `crates/faris-app/` | Native egui workspace and wgpu renderer |
| `scenarios/arc-inspired/scenario.json` | The single editable demo scenario |
| `docs/DEMO.md` | Demo question, requirements, and completion criteria |
| `docs/ARCHITECTURE.md` | Rust boundaries, units, adapter and evidence design |
| `docs/ROADMAP.md` | Long-term project direction and development phases |
| `docs/DEMO_ROADMAP.md` | Comprehensive demo-only roadmap: dependencies, detailed requirements, and acceptance walkthrough |
| `docs/TOOLING.md` | Open-source tools and planned integration roles |
| `docs/SCIENTIFIC_BASELINE.md` | Benchmark selection, validation scope, uncertainty rules and unresolved scientific inputs |
| `docs/MATERIAL_BASELINE.md` | Primary-source material candidates and inputs that still need resolution |
| `docs/TRANSPORT.md` | Strict raw-tally contracts, dimensions and checked Rust normalization |
| `controls/` | Independent analytic controls and external OpenMC numerical checks |
| `references/` | Identified scientific sources and read-only tool/data audits |
| `integrations/` | Reactor adapter plans; Core integration remains unimplemented |
| `runs/` | Ignored generated records and captures |
| `data/raw/` | Ignored downloaded scientific inputs |

## License and repository

Copyright © 2026 Avila Labs. FARIS source code and documentation are licensed
under the [GNU Affero General Public License, version 3 only](LICENSE)
(`AGPL-3.0-only`). Third-party tools and data retain their respective licenses.

Cargo packages use `publish = false` while the scaffold evolves.
The project repository is [AvilaLabs/FARIS](https://github.com/AvilaLabs/FARIS).
