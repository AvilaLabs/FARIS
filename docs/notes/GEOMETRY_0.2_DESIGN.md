# Transport geometry for 0.2: design and build slices

Recorded 2026-10-05. Decision and dimension choices: docs/DEMO_ROADMAP.md, "First step: a more realistic
transport geometry". Published ARC dimensions and their sources: [ARC_GEOMETRY_SOURCES_0.2.md](ARC_GEOMETRY_SOURCES_0.2.md).

## What 0.1 does

- Scenario `faris-scenario/v0.1`: `geometry {major_radius_m, plasma_minor_radius_m, plasma_to_first_wall_gap_m,
  radial_build_m}` and per variant a list of layers with one `thickness_m` each.
- `faris-engine` `build_manifest` stacks the layers as concentric circular tori about one major radius. Full
  volumes are closed form, `2π² R0 (b² − a²)`. The port intersection is a numerical estimate
  (`geometry::estimate_torus_shell_box_intersection`), and region volumes come from `torus_shell_region_volume_m3`.
- The OpenMC adapter (`integrations/openmc/reactor_transport.py`, `compose`) builds one `YTorus` per boundary
  with `b = c`. The torus axis is y, which is vertical. There is one axis-aligned port prism at toroidal angle 0,
  and region sub-cells are split by a y-cylinder at R0 and two planes. The adapter checks port volumes by
  independent point sampling, and it repeats the region closed forms in Python.
- The viewer meshes circular shells (`faris-engine/src/mesh.rs`, `torus_shell`, `torus_shell_with_prism_cut`).

## Geometry model for 0.2

**Boundaries.** Each layer boundary is an elliptical torus about the vertical axis with its own centre radius
`A`, horizontal semi-axis `C` and vertical semi-axis `B`. In OpenMC this is `YTorus(a=A, b=B, c=C)`, where b
runs along the axis, so it stays an analytic CSG surface with no mesh or CAD. The scenario
describes boundaries by their extents, not by A, B and C:

- the plasma: `major_radius_m`, `minor_radius_m`, `elongation`. Triangularity is recorded in the scenario, but
  a torus cannot represent it, and the scenario says so;
- each layer: `thickness_m` as a number (the same on every side, as in 0.1) or as
  `{inboard_m, outboard_m, vertical_m}`.

Boundary k has inboard radius `R_in`, outboard radius `R_out` and half-height `H`, each accumulated from the
plasma boundary plus the gap. Then `A = (R_in + R_out)/2`, `C = (R_out − R_in)/2`, `B = H`.

**v0.1 compatibility.** A v0.1 scenario reads as plasma elongation 1 with uniform thicknesses. Every boundary
then has `A = R0` and `B = C`, which is exactly the 0.1 geometry. Acceptance: for each shipped 0.1 scenario,
`compose` writes byte-identical `geometry.xml` and the manifest volumes are unchanged. This keeps the 0.1
recorded evidence reproducible.

**Nesting must be checked, not assumed.** When centre radii differ, monotone extents do not imply nesting. For
example, an outer ellipse with A = 0, C = 10, B = 1 does not contain an inner ellipse with A = 5, C = 4.9,
B = 0.99, because the inner top point (5, 0.99) lies outside. Validation therefore samples each inner boundary
(at least 3,600 points, then local refinement near the minimum) against the next boundary. It reports the
minimum local thickness of every layer, and it rejects a scenario whose minimum is below 1 mm or whose innermost
point of any boundary is at or below R = 0. The minimum local thickness is shown in the radial-build panel, so
a thin shoulder is visible.

**Exact volumes, in Rust only.** By Pappus, the full volume of an elliptical torus is `2π A · π B C`. The part
inboard of the cylinder `R = R0` has a closed form: substitute `u = (R − A)/C` with `u0 = (R0 − A)/C`, so the
region is the unit disk below `u0`, with area `π − acos(u0) + u0 √(1 − u0²)` and first moment
`−(2/3)(1 − u0²)^{3/2}`. The volume is then `2π C B [A · area + C · moment]`, and a shell volume is the
difference of two boundaries. Sectors bounded by planes through the axis multiply by the angular fraction.
These formulas replace the Python copy in the adapter. The adapter receives volumes in the manifest and checks
them by its own point sampling, which is independent of the Rust code, as the port check is now. Volumes that
have no closed form (coils, divertor, ports) use the existing pattern: a deterministic Rust estimate with a
refinement delta, then an independent sampling check in the adapter.

## New components

**TF coils.** This is an optional `coils` block on the magnet layer: `count` (ARC: 18), `leg_width_m` (authored,
because ARC does not publish it), and `first_coil_angle_deg`. The default is half a coil pitch (10° for 18), so
a port at 0° sits between coils, as in ARC.

- Coil k is the magnet shell ∩ the slab `|distance to the plane at φ_k| ≤ leg_width/2` ∩ the wedge
  `|φ − φ_k| ≤ π/count`. The wedge makes the inboard legs wedged where adjacent slabs would overlap, while the
  outboard legs keep the slab width. All surfaces are planes.
- The space between coils is a separate cell with an explicit fill. The default is void, which FARIS already
  null-fills.
- The magnet component volume becomes the coil volume.

Response domains:

- the magnets component, covering all coils;
- `inboard_half` and `outboard_half`, as now;
- `port_flanking_coils`, the two coils on either side of each port.

That last domain replaces `port_sector` for magnets. With discrete coils a port no longer cuts any magnet, so
the 0.1 port-sector magnet response would be empty. The flanking coils are the ones a port exposes, and they
are what the history's magnet service limit must read. The response id and the service-limit mapping change
together, and the scenario records which domain the limit reads.

**Divertor.** This is an optional `divertor` block: `material_id`, `vertical_start_m` (|y| beyond which the
divertor replaces the listed layers), and `replaces_component_ids` (for example first wall and blanket).

- It models a double null, top and bottom, as in ARC (K18), using horizontal planes intersected with the listed
  shells.
- It is a component in its own right, so it gets its own heating, damage and fluence responses and its own
  service limit (class replaceable). The history engine already handles any replaceable component by id.
- Material and thickness are authored surrogates and are labelled as such.

**Ports.** `penetrations` becomes a list. A port is `{id, toroidal_angle_deg, vertical_centre_m, width_m,
height_m, fill_material_id, affected_component_ids}`, and it runs radially from the plasma edge to outside
the outer boundary.

- A port away from 0° is a prism rotated about the axis, bounded by general `Plane`s.
- A v0.1 `outboard_rectangular_prism` reads as one port at 0°, with the radial range taken from its bounds.
- The existing check that a port is centred on angle 0 becomes a per-port angle. Region membership uses the
  angle relative to the port.
- Ports must not overlap each other, must not cut a coil (checked), and must not reach the plasma.

## Viewer

- **Elliptical shells.** Generalize `mesh::torus_shell` to elliptical boundaries with centre offsets. The
  cutaway still changes display meshes only.
- **Coils and divertor** render as their own meshes. Coils use the magnet colour per coil, with the between-coil
  void not drawn.
- **Ports at their angles.** The 12-edge port outline is rotated per port.
- **Cross-section panel.** A 2D poloidal slice at a chosen toroidal angle, drawn with the egui painter. It shows
  each boundary ellipse, the divertor planes, the coil leg if the slice passes through one, and hover readouts of
  component and local thickness.
- **Radial-build panel.** Inboard, outboard and vertical bars per variant, with the minimum local thickness
  flagged when it falls below 5 % of the side thickness.

## Touchpoints

- `faris-model`: the `Geometry`, `Layer` and `Penetration` schema, v0.2 parsing, and v0.1 upgrade on read.
- `faris-engine`:
  - `lib.rs` (`build_manifest`, `ComponentGeometry` gains boundary extents and coil/divertor geometry);
  - `geometry` (exact volumes, containment check, numerical estimates);
  - `reactor.rs` (`check_geometric_volumes`);
  - `sweep.rs`, which needs to know which side the sweep varies. For a per-side layer the scenario names the side
    or sides. The ARC sweep varies the outboard blanket, because the inboard build is space-limited;
  - `mesh.rs`.
- `faris-app`: `main.rs` (the inspector's inner/outer radius readouts become per side), `viewport.rs`,
  `sweep_panel.rs`, and the new cross-section and radial-build panels.
- `integrations/openmc/reactor_transport.py`:
  - `compose`, the source box, `audit_geometry_ownership` (probes along inboard, outboard and vertical rays
    through each layer's local midpoint), and the region universes;
  - `torus_region_volume_m3` and `region_contains` are removed in favour of the manifest volumes.
- `integrations/openmc/fwcadis_spike.py`: the peak-mesh boxes come from the manifest and are no longer
  hard-coded x ranges.
- Controls: `controls/check_port_geometry.py` and `controls/test_region_volumes.py`.
- Scripts: `generate_allocation_sweep`, `package_recorded_demo`, `recorded_bundle_contract`.

## Build slices

Each slice lands green on its own, in this order. Slices 3 to 5 touch `compose` and the manifest, so they run
one after another, not in parallel worktrees.

1. **Schema and kernel.**
   - Work: v0.2 schema, the v0.1 upgrade, boundary extents, the containment check with minimum local thickness,
     exact volumes and half-volumes, and the manifest fields.
   - Tests: the Pappus and half-volume closed forms against brute-force quadrature; the nesting counter-example
     above is rejected; v0.1 manifests are numerically unchanged.
2. **Adapter: elliptical boundaries.**
   - Work: YTorus per boundary from the manifest, the plasma ellipse source, volumes taken from the manifest and
     checked by sampling, and ownership probes on three rays.
   - Acceptance: byte-identical `geometry.xml` for every 0.1 scenario, plus a small run on an elliptical
     scenario whose sampled volumes agree with the manifest within 3 standard errors.
3. **TF coils**, with the flanking-coil domain and the service-limit mapping.
4. **Divertor.**
5. **Multiple ports.**
6. **Viewer**: elliptical meshes after slice 1, then coils, divertor and ports as each lands, then the
   cross-section and radial-build panels.
7. **ARC-fitted scenario and benchmark.**
   - The scenario uses the published inboard build; authored values are labelled.
   - The benchmark is a TF fast-fluence run compared with Sorbom's at least 9 FPY to 3e18 n/cm² above 0.1 MeV.
     This step needs transport CPU, so it runs after the 0.1.0 release and alongside the FW-CADIS trial.

## Not in 0.2

- Triangularity and true D-shaped coils. They need CAD geometry (Paramak through DAGMC). DAGMC is enabled in
  the local OpenMC build, so this is a later step that can reuse the same scenario dimensions.
- Coil case and winding-pack split, port plugs, and the vessel's double wall. ARC does not publish these.
