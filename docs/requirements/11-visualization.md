# Visualization

How FARIS shows geometry, fields, uncertainty, comparisons and time. The test for every
row is whether a reader can take the right number and the right doubt from the picture.
Frame-time and large-data speed targets live in [PERFORMANCE](08-performance.md) and are
referred to by ID (PERF-011, PERF-012, PERF-035); they are not restated here. Text
equivalents and colour-safety rules are in [10-accessibility-and-localisation.md](10-accessibility-and-localisation.md).
Reference hardware and models are in [the index](README.md#reference-hardware-and-models).

Repository check (2026-10-05): the viewport (wgpu) draws component geometry, a spatial
field slice with faint ghost context, selection by picking, and overlay lines. Field
colour is "inferno" through 11 control points with the darkest 4 % trimmed, a fixed log
scale, bins with relative error above 0.3 muted toward grey, sampled zero drawn dark
grey and missing data drawn magenta. Charts are SVG strings in a light print theme,
rasterised with resvg and drawn into the PDF as vectors. There are no movable section planes,
measurement, probes, legends with a clamp marker, animation, side-by-side 3D or video export yet.

## 3D viewport

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| VIS-001 | FARIS shall let the user orbit, pan, zoom and reset the camera. | Orbit, pan, zoom, fit-to-selection, fit-all, and standard views (top, front, side, isometric) each on one key or button; reset returns to the exact saved camera. | UI automation script; camera-state round trip. | Standard in CAD and ParaView [U] | F1 | Partial: orbit, shift-drag pan, scroll zoom exist; fit and standard views not verified |
| VIS-002 | Camera motion shall be predictable. | The point under the pointer stays under the pointer while zooming; orbit has no gimbal flip; inverse of any camera move returns to within 1e-6 of the original pose. | Property test on camera maths. | FARIS choice | F1 | Partial: 6 camera unit tests (e.g. centre ray reaches the orbit target); properties not stated |
| VIS-003 | FARIS shall provide section planes and clipping. | Up to 3 planes with position and angle entered by number or dragged; each plane switchable, with a cap or outline drawn; display clipping never changes solver geometry. | UI test; solver-input hash unchanged after clipping. | ParaView clip and Onshape section view [U]; repository rule: a cutaway cannot change solver geometry [internal] | F2 | Partial: one fixed display cutaway (a toroidal quadrant) can be toggled; no movable planes |
| VIS-004 | FARIS shall show exploded and transparent views. | Per-component and per-group opacity 0–100 % and an exploded view with a distance slider; the selected component always stays opaque or outlined; transparency is order-independent (no flicker on orbit). | Screenshot test over orbit path. | FARIS choice | F2 | Partial: ghost context around the slice only |
| VIS-005 | FARIS shall measure distances, thicknesses and angles. | Point-to-point, thickness through a component and angle tools with units; accuracy ≤ 1e-6 of model units against the model, not the mesh. | Test on analytic geometry. | CAD measurement tools [U]; FARIS choice | F2 | No |
| VIS-006 | FARIS shall provide point and line probes. | Click a point or draw a line to read field value, unit, kind, relative error, material and component, plus a plot-over-line with error band; value matches the stored mesh bin within 1e-12 relative. | Test on stored meshes. | ParaView plot over line [U] | F2 | No |
| VIS-007 | Picking shall show what was clicked. | Clicking any object shows identity, material, volume, mass, field value with error and a link to its receipt ≤ 100 ms (see PERF-016); picks nearest visible surface, never a clipped or hidden one. | UI automation timing; hidden-object test. | Nielsen 0.1 s [V]([NN/g](https://www.nngroup.com/articles/response-times-3-important-limits/)) | F2 | Partial: nearest component picking exists; detail card and timing not checked |
| VIS-008 | The viewport shall hold its frame rate and scale to large models. | As PERF-011 and PERF-012. | As PERF-011 and PERF-012. | See PERF-011, PERF-012 | F1, F7 | Partial: see PERF-011 |
| VIS-009 | FARIS shall show orientation and scale. | Axis gizmo, scale bar with unit and a visible unit-system label in every view and export. | Screenshot lint. | FARIS choice | F1 | No |

## Field overlays and colour maps

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| VIS-010 | FARIS shall overlay flux, heating, activation and dose on the geometry. | Mesh fields for neutron flux, photon flux, nuclear heating, activation (Bq/kg or gamma dose rate) and dose (Sv/h) shown as slice, isosurface, volume and component-surface colouring; each field names quantity, unit, normalisation and kind. | Feature matrix against the field list; label lint. | ParaView feature set [V]([ParaView docs](https://docs.paraview.org/en/latest/UsersGuide/introduction.html)); records in 02-radiation-transport and 03-activation-and-materials | F2 | Partial: neutron flux slice by component on RM-S; others not shown |
| VIS-011 | Slices, clips, isosurfaces, volume rendering, vector glyphs and streamlines shall be available for mesh data. | Each tool works on mesh tallies; each ≤ 1 s on 10⁶ cells on RL (see PERF-035). | Feature matrix; PERF-035 benchmark. | ParaView feature set [V]([ParaView docs](https://docs.paraview.org/en/latest/UsersGuide/introduction.html)) | F2 | Partial: slice only |
| VIS-020 | Default sequential colour maps shall be perceptually uniform and colour-blind safe. | Default sequential map from viridis, cividis or inferno; monotone in CAM02-UCS lightness with step ΔE (CIEDE2000) between adjacent 256-level entries within ±25 % of the mean step; no rainbow, jet or HSV map offered without a "not recommended" tag. | Metric check on the colour-map tables (colour-science library) in CI. | Viridis is perceptually uniform and CVD friendly [V]([BIDS](https://bids.github.io/colormap/)); cividis optimised for CVD [V]([Nunez 2018](https://doi.org/10.1371/journal.pone.0199239)) | F1 | Partial: inferno with 11 control points, lightness test exists (luminance monotone); CIEDE2000 step test absent |
| VIS-021 | The shipped map shall match its reference. | Interpolated map within ΔE (CIEDE2000) ≤ 1.0 of the 256-entry reference table at every level; trimmed ends declared in the legend. | Test against reference table. | Own colour map interpolates 11 points and trims 4 % [internal] | F1 | No: not compared with reference |
| VIS-022 | Diverging maps shall be centred on a stated value. | ≥ 1 diverging map for differences, centred on zero (or a user value); centre marked on the legend; symmetric limits by default. | Visual regression plus legend lint. | Crameri on colour misuse [U]([Nature Comms](https://doi.org/10.1038/s41467-020-19160-7)) | F2 | No |
| VIS-023 | Colour map choice shall be rich enough and safe. | ≥ 12 maps (sequential, diverging, categorical, cyclic), each tagged "colour-blind safe" or not by simulation; greyscale-readable by lightness. | Simulation test; map metadata lint. | Crameri scientific colour maps [U]([Crameri](https://www.fabiocrameri.ch/colourmaps/)) | F4 | No: one map |
| VIS-024 | Colour-vision preview shall be built in. | Any view and any chart can be previewed under deuteranopia, protanopia, tritanopia and greyscale; the preview is for checking only and never saved by accident. | Feature test. | Machado et al. 2009 simulation [U] | F4 | No |
| VIS-025 | Legends shall be honest. | Every field view shows quantity, unit, kind, scale type (linear or log), explicit minimum and maximum, a clamp marker for data outside the range and a swatch for zero, error-masked and missing values; compared panels share limits by default and warn when they do not. | Legend lint; visual regression. | Munzner and Tufte on honest colour bars [U] | F1 | Partial: fixed 10–20 decades log scale, grey for zero, magenta for missing; legend text and clamp marker not verified |
| VIS-026 | Log scales shall handle zero and sub-range values. | Log colour and axis scales with an explicit "N cells at or below zero" note, a stated lower bound, and no clamping without a marker. | Test fields containing zeros and negatives. | Domain standard [U] | F1 | Partial: sampled zero drawn grey; count note absent |
| VIS-027 | Linear and log scales shall be a user choice with sensible defaults. | Scale type, range (auto, fixed, percentile) and map selectable per view; the range used is saved in the study and shown. | UI test. | FARIS choice | F2 | Partial: fixed scale, not selectable |
| VIS-028 | Imported pictures shall not leak bad colour. | Third-party images (such as solver plots) are shown with their origin and a "colours not controlled by FARIS" tag, or redrawn from data. | Review of every imported-image path. | Rainbow images from external tools leak in [R3] | F6 | No |

## Uncertainty

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| VIS-030 | Every plotted Monte Carlo quantity shall show its statistical error or say why not. | 100 % of plotted MC values show 1σ and 2σ intervals or a reason in text; the schema refuses a plotted series without a sigma or a reason. | Plot audit; schema test. | Error bars widely misread [V]([Belia 2005](https://doi.org/10.1037/1082-989X.10.4.389)); house rule [internal] | F1 | Partial: standard errors on key charts; not enforced by schema |
| VIS-031 | Field maps shall have a companion relative-error map. | For every field overlay a relative-error view, same geometry and cell order, toggled with one key. | UI test. | MC mesh tallies carry standard errors [internal] | F2 | No |
| VIS-032 | High-error cells shall be marked, never hidden. | Cells with relative error > 10 % hatched (pattern, not only desaturation); > 30 % additionally desaturated; the hatching threshold is shown and user-settable; 0 cells removed without a visible marker. | Visual regression with synthetic error fields. | Hide-nothing rule [internal]; today's 0.3 threshold [internal] | F2 | Partial: relative error > 0.3 muted toward grey; no hatching |
| VIS-033 | The source of uncertainty shall be named. | Every error depiction labelled "statistical only" or lists the included sources (statistical, nuclear data, geometry, model); statistical-only never reads as total uncertainty. | Label lint; see 07-uncertainty (UNC). | MC standard error excludes data, geometry and model uncertainty [R3] | F1 | Partial: standard errors labelled; scope wording not linted |
| VIS-034 | Alternative encodings shall be available. | Band, gradient interval and sampled-outcome (animated) modes for sweeps and ensembles; a fan chart for plant-life history uncertainty. | Feature tests. | Hypothetical outcome plots [V-2]([Hullman 2015](https://doi.org/10.1371/journal.pone.0142444)); [Kale 2019](https://idl.uw.edu/papers/hops-trends) | F4 | No |
| VIS-035 | Multiple comparisons shall be counted. | Any view showing 2σ flags across many cells or parameters states the number of comparisons and the expected false flags at that level. | Text lint; synthetic test. | About 5 % false flags at 2σ [R3] | F2 | Partial: 2σ flags on four contrasts, count not stated |
| VIS-036 | Uncertainty shall survive export. | 100 % of exported figures and tables carry the same error depiction and scope text as the screen. | Export diff test. | FARIS choice | F2 | Partial: standard errors in comparison.csv |

## Charts

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| VIS-040 | Every chart axis shall have a quantity and a unit. | 100 % of axes labelled "quantity (unit)"; dimensionless quantities say "dimensionless"; every chart title or subtitle names the kind label of what it plots. | Chart lint over all generated charts. | House rule [internal] | F1 | Partial: study charts carry subtitles and units; no lint |
| VIS-041 | Charts shall export as vector and raster at publication quality. | SVG, PDF (vector) and PNG ≥ 300 dpi with embedded or outlined fonts; text ≥ 6 pt at the stated print size; line widths ≥ 0.5 pt; export size set by the user in mm or inches. | Export test; pdffonts and image-size check. | Journal figure rules [U] | F1 | Partial: SVG and PNG exist from one SVG source, PDF draws vector charts; size options and 300 dpi not verified |
| VIS-042 | Chart export shall be self-describing. | Each figure carries a caption with study hash, kinds, date and software version; caption can be turned off but not lost silently. | Export test. | Provenance rule [internal] | F1 | Met: exports are stamped with the .faris sha256 (see docs/STUDY_EXPORT.md) |
| VIS-043 | Charts shall use a restrained, accessible style. | ≤ 7 series colours per chart, each with a marker or dash; light print theme and dark screen theme from one definition; log ticks on decades. | Style lint; screenshot test. | Categorical hues beyond about 7 are indistinguishable [R3] | F1 | Partial: light print theme exists; marker and dash rule not checked |
| VIS-044 | Charts shall be explorable on screen. | Hover shows exact value, unit, error and kind; zoom and pan with reset; legend toggles series; keyboard equivalents for each. | UI test. | FARIS choice | F2 | Partial: charts are drawn; interaction not audited |
| VIS-045 | Chart data shall be readable as text. | As A11Y-044: view as table, CSV copy and generated summary. | UI test. | As A11Y-044 | F2 | Partial: CSV export |
| VIS-046 | Charts shall not mislead by scaling. | Bar charts start at zero; truncated axes carry a break marker; compared charts share axes by default and warn when not. | Chart lint. | Tufte small multiples [U] | F1 | Unmeasured |

## Linked views and comparison

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| VIS-050 | Selection shall link across 3D, tree, table and chart. | Selecting a component or cell in any one highlights it in the other three ≤ 100 ms and scrolls it into view (see PERF-016). | UI automation timing. | ParaView linked views [U]; Nielsen 0.1 s [V]([NN/g](https://www.nngroup.com/articles/response-times-3-important-limits/)) | F2 | Partial: picking exists on the 3D view |
| VIS-051 | Comparison shall show up to six arrangements side by side. | Up to 6 panels with linked cameras and shared scales; shared scale on by default; panel titles name the arrangement and kind. | UI test. | Small multiples [U] | F2 | Partial: four-arrangement table and chart comparison; no linked 3D panels |
| VIS-052 | Difference views shall carry significance. | Difference and ratio fields with a mask for differences below 2σ (σ combined from both inputs), a stated comparison count and a diverging map centred on zero. | Synthetic-field test with known differences. | Compare view with 2σ flags exists [internal]; trap on multiple comparisons [R3] | F2 | Partial: 2σ flags on scalar contrasts; no difference fields |
| VIS-053 | Compared views shall never differ in scale silently. | Different ranges between compared panels trigger a visible banner and are disabled by default. | Visual regression. | Honest colour bars [U] | F2 | No |
| VIS-054 | Component colours shall stay identical across views. | One identity-to-colour map for components used in 3D, tree, table and chart; stable across sessions and exports. | Colour-map consistency test. | Consistency rule [U] | F2 | Partial: component colours in 3D; cross-view consistency unaudited |

## Time-dependent and large-data views

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| VIS-060 | FARIS shall animate quantities over the plant's life. | Time slider and playback over operating years with state markers (outages, swaps, shutdown); fields shown at a chosen time, with a fixed colour range across time by default; keyboard stepping by snapshot. | UI test; range-stability test. | FARIS choice | F4 | Partial: timeline and scrub of operating history on charts; no field animation |
| VIS-061 | Animation shall be controllable and calm. | Pause, step, speed 0.25–4×, loop; honours reduced motion (A11Y-015); never auto-plays; frame rate follows PERF-011. | UI test. | WCAG 2.2.2 [V]([W3C](https://www.w3.org/TR/WCAG22/)) | F4 | No |
| VIS-062 | Large models shall use level of detail without hiding features. | LOD swaps are invisible in screenshot diff at rest (≤ 0.5 % pixels); small but selected components never culled; the view states when it is showing reduced detail. | Screenshot diff at rest; selection test. | See PERF-012; FARIS choice | F7 | No |
| VIS-063 | Field operations on large meshes shall remain interactive. | As PERF-035. | As PERF-035. | See PERF-035 | F2 | No |
| VIS-064 | Rendering shall be deterministic enough to test. | Golden screenshots per view with ≤ 0.1 % differing pixels on a software renderer baseline; GPU differences reported but not failing. | CI screenshot tests. | FARIS choice | F2 | Partial: screenshots captured for docs; no golden diff |

## Export, themes and interoperability

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| VIS-070 | FARIS shall export screenshots at chosen size. | PNG at 1×, 2×, 4× the window, transparent or opaque background, with an optional caption block (study hash, field, range, kind); capture independent of window occlusion. | Export test. | FARIS choice | F2 | Partial: 3D view cropped from the window when capture works; no size choice |
| VIS-071 | FARIS shall export video of an animation. | MP4 (H.264) and image sequence at 1080p and 4K, 24–60 fps, burned-in time and legend; deterministic output for the same study. | Export test; frame-hash test. | FARIS choice | F4 | No |
| VIS-072 | Fields shall export to standard visualisation formats. | VTK/VTU and XDMF-HDF5 openable in ParaView with units and metadata; round-trip value error ≤ 1e-12 relative. | Headless ParaView load and compare. | ParaView and VisIt use these formats [V]([ParaView docs](https://docs.paraview.org/en/latest/UsersGuide/introduction.html)) | F2 | No |
| VIS-073 | FARIS shall offer light, dark and high-contrast themes for UI, charts and 3D. | All three; follow the OS; every colour map and legend legible on both backgrounds (A11Y-010, A11Y-011). | Contrast and screenshot audit per theme. | Blender and VS Code themes [U] | F2 | Partial: dark UI, light print charts |
| VIS-074 | Visual output shall be reproducible from the study. | Re-rendering a stored study view from the .faris file gives the same figure (≤ 0.1 % pixel difference on the software renderer); camera, field, range and map stored in the file. | Replay test. | FARIS choice | F2 | Partial: .faris stores step, preset, year, field view, tab, arrangement and allocation; camera, range and map are not stored |

## Traps

- Viridis as a default does not make a figure honest. Compared panels with different ranges, clipped ranges and uncentred diverging maps fake or hide results.
- A perceptually uniform map still fails if the data is on the wrong scale. State linear or log, and show clamping.
- Hatching or greying noisy cells is only honest if the threshold is visible and nothing disappears. Hiding high-error cells is worse than drawing them.
- Error bars from a Monte Carlo run are statistical only. Without the label, readers take them as total uncertainty. They are also read as bounds, not as standard errors.
- Frame rate on the small demo says nothing about the full plant. Quote it with the model name (RM-S, RM-M, RM-L).
- A pretty colour bar interpolated from a few control points can drift from its reference map. Test it against the full table.
- A screenshot test that passes on the author's GPU can hide per-platform differences. Use the software baseline and report GPU differences separately.
- Charts exported to PDF with raster text look sharp on a laptop and soft in print. Check vector text and embedded fonts.

