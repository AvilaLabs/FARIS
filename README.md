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

## Current scaffold

- A Rust workspace with separate model, engine, CLI, and desktop crates.
- One versioned scenario with two allocations inside the same radial envelope.
- Strict scenario parsing, geometric validation, and source-file identity.
- Shared full-torus geometry records and display meshes.
- Native GPU rendering, orbit/zoom, cutaway, component picking, visibility,
  arrangement switching, and component dimensions.
- A timeline selector prepared for future operating-history results.
- CLI validation, deterministic geometry exports, and optional tool detection.

**This is a geometry scaffold.** Neutron transport, activation, ageing, fuel
inventory, maintenance, and power production are not implemented. Their result
records say `NOT_EVALUATED`; the app leaves their values empty. Component
colors identify layers and do not represent calculated physical fields.
Material definitions are deliberately unassigned. The idealized circular
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

## Development

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
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
| `integrations/` | Adapter plans; no active solver or Core integration yet |
| `runs/` | Ignored generated records and captures |
| `data/raw/` | Ignored downloaded scientific inputs |

## License and repository

Copyright © 2026 Avila Labs. FARIS source code and documentation are licensed
under the [GNU Affero General Public License, version 3 only](LICENSE)
(`AGPL-3.0-only`). Third-party tools and data retain their respective licenses.

Cargo packages use `publish = false` while the scaffold evolves.
The project repository is [AvilaLabs/FARIS](https://github.com/AvilaLabs/FARIS).
