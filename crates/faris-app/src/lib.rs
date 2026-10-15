#![allow(clippy::disallowed_methods)]
// Plotting, colour and camera arithmetic only; nothing here is recorded as evidence.
// The browser build compiles the desktop-only panels but never starts them.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

mod archive_panel;
mod badge;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod browser;
mod camera;
mod commands;
mod compare_panel;
mod design_check_panel;
mod design_check_result;
mod design_check_run;
mod design_shared;
mod design_tools;
mod design_transport_panel;
mod design_transport_result;
mod design_transport_run;
mod dialog;
mod export_panel;
mod history_panel;
mod interface_check;
#[cfg(not(target_arch = "wasm32"))]
mod interface_prefs;
mod maintenance_panel;
mod maintenance_run;
mod package;
mod recorder;
mod settings;
mod study_file;
mod study_panel;
mod sweep_panel;
mod thumbnail;
mod tool_finder;
mod tour;
mod transport_panel;
/// The Avila Labs account controls: the device sign-in on the desktop, the
/// session cookie in the browser.
#[cfg(not(target_arch = "wasm32"))]
type Suite = avila_account::ui_desktop::DesktopSuite;
#[cfg(target_arch = "wasm32")]
type Suite = avila_account::ui_web::WebSuite;

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod steps;
mod thread;
mod time;
mod uncertainty;
mod viewport;

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(not(target_arch = "wasm32"))]
mod web_ensembles;

/// Native entry point: parses the command line and opens the window.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_native() -> Result<(), Box<dyn std::error::Error>> {
    native::main()
}

/// Navigation help beside the viewport: every drag has a key equivalent (UX-037).
pub(crate) const VIEWPORT_HELP: &str = "Drag to orbit · Shift-drag to pan · scroll to zoom · click to select · Tab to the viewport, then arrows orbit, Shift and arrows pan, + and − zoom, 0 resets, Enter reads out";

#[cfg(test)]
mod audit;

/// Smallest text anywhere in the interface, and the size of body text (A11Y-021).
pub(crate) const MIN_TEXT_PX: f32 = 11.0;
pub(crate) const BODY_TEXT_PX: f32 = 13.0;

/// Raise egui's default text styles (small text is 9 px, body 12.5 px) to the
/// minimums above; larger styles are left alone.
pub(crate) fn apply_text_sizes(style: &mut egui::Style) {
    for (text_style, font) in &mut style.text_styles {
        let floor = match text_style {
            egui::TextStyle::Body | egui::TextStyle::Button => BODY_TEXT_PX,
            _ => MIN_TEXT_PX,
        };
        font.size = font.size.max(floor);
    }
}

use crate::time::Instant;
use camera::{Camera, triangle_hit};
use eframe::egui;
use faris_engine::{
    DemoManifest, VariantGeometry, build_manifest,
    mesh::{CUTAWAY_SWEEP, MeshVertex, torus_shell, torus_shell_with_prism_cut},
};
use faris_model::LoadedScenario;
use std::{
    collections::{BTreeMap, BTreeSet},
    f32::consts::TAU,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Guided workflow: the left panel shows only the current step's content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
enum Step {
    #[default]
    Design,
    Simulate,
    Operate,
    Compare,
    Evidence,
}

impl Step {
    const ALL: [Step; 5] = [
        Step::Design,
        Step::Simulate,
        Step::Operate,
        Step::Compare,
        Step::Evidence,
    ];
    fn number(self) -> usize {
        self as usize + 1
    }
    fn name(self) -> &'static str {
        match self {
            Step::Design => "Design",
            Step::Simulate => "Simulate",
            Step::Operate => "Operate",
            Step::Compare => "Compare",
            Step::Evidence => "Evidence",
        }
    }
}

/// What package mode adds to the running app: messages for the Evidence step
/// and the evidence extraction that must outlive the window.
#[derive(Default)]
struct PackageSession {
    /// Set when the package checks failed and nothing was loaded.
    failure: Option<study_file::EvidenceBadge>,
    development_binary: Option<study_file::EvidenceBadge>,
    evidence: Option<study_file::EvidenceBadge>,
    /// The recorded maintenance result the package carries, if any.
    maintenance_result: Option<package::RecordedMaintenance>,
    /// The package's evidence store with its saved studies, read in place.
    saved_store: Option<package::SavedStore>,
}

/// A package-mode message: its badge, with the why and next step also as text
/// so they do not depend on hovering.
fn package_notice(ui: &mut egui::Ui, notice: &study_file::EvidenceBadge) {
    ui.horizontal_wrapped(|ui| {
        badge::badge(ui, notice.kind, notice.label, &notice.text);
    });
    ui.add(egui::Label::new(egui::RichText::new(&notice.text).small()).wrap());
    ui.add_space(8.0);
}

/// Everything needed to load one study's arrangements and assumptions. Built
/// from command-line flags or from the files a .faris study was unpacked to.
#[derive(Clone, Default)]
struct SessionInputs {
    scenario: Option<PathBuf>,
    physics: Vec<PathBuf>,
    run: Vec<PathBuf>,
    bundle: Vec<PathBuf>,
    control_scenario: Option<PathBuf>,
    control_physics: Vec<PathBuf>,
    control_run: Vec<PathBuf>,
    control_bundle: Vec<PathBuf>,
    assumptions: Option<PathBuf>,
    python: Option<PathBuf>,
    openmc: Option<PathBuf>,
    audit: Option<PathBuf>,
    cross_sections: Option<PathBuf>,
    runs_directory: PathBuf,
    field_view: transport_panel::FieldView,
}

impl SessionInputs {
    /// The bundled geometry demo with nothing loaded.
    fn empty(runs_directory: PathBuf) -> Self {
        Self {
            runs_directory,
            ..Self::default()
        }
    }
}

/// The loaded arrangements and history panel; sendable from a worker thread.
struct Session {
    manifest: DemoManifest,
    control: Option<(DemoManifest, transport_panel::TransportPanel)>,
    transport: transport_panel::TransportPanel,
    history: history_panel::HistoryPanel,
}

fn build_session(inputs: SessionInputs) -> Result<Session, String> {
    let err = |e: Box<dyn std::error::Error>| e.to_string();
    let loaded = if let Some(path) = &inputs.scenario {
        load_scenario(path)?
    } else if let Some(path) = inputs.bundle.first() {
        scenario_from_bundle(path).map_err(err)?
    } else {
        LoadedScenario::from_bytes(include_bytes!(
            "../../../scenarios/arc-inspired/scenario.json"
        ))
        .map_err(|e| e.to_string())?
    };
    let manifest = build_manifest(&loaded).map_err(|e| e.to_string())?;
    let control = if inputs.control_scenario.is_some() || !inputs.control_bundle.is_empty() {
        let scenario = if let Some(path) = &inputs.control_scenario {
            load_scenario(path)?
        } else {
            scenario_from_bundle(&inputs.control_bundle[0]).map_err(err)?
        };
        let control_manifest = build_manifest(&scenario).map_err(|e| e.to_string())?;
        let panel = transport_panel::TransportPanel::new(
            scenario,
            transport_panel::TransportConfiguration {
                python: inputs.python.clone(),
                openmc: inputs.openmc.clone(),
                audit: inputs.audit.clone(),
                cross_sections: inputs.cross_sections.clone(),
                physics: inputs.control_physics,
                runs: inputs.control_run,
                bundles: inputs.control_bundle,
                runs_directory: inputs.runs_directory.clone(),
            },
        )?;
        Some((control_manifest, panel))
    } else {
        None
    };
    let mut transport = transport_panel::TransportPanel::new(
        loaded,
        transport_panel::TransportConfiguration {
            python: inputs.python,
            openmc: inputs.openmc,
            audit: inputs.audit,
            cross_sections: inputs.cross_sections,
            physics: inputs.physics,
            runs: inputs.run,
            bundles: inputs.bundle,
            runs_directory: inputs.runs_directory,
        },
    )?;
    transport.view = inputs.field_view;
    let history = history_panel::HistoryPanel::new(inputs.assumptions)?;
    Ok(Session {
        manifest,
        control,
        transport,
        history,
    })
}

/// Load the sweep's bundles through the ordinary transport record validation.
fn load_sweep(
    paths: &[PathBuf],
    runs_directory: &std::path::Path,
) -> Result<(transport_panel::TransportPanel, LoadedScenario), String> {
    let scenario = scenario_from_bundle(&paths[0]).map_err(|e| e.to_string())?;
    let panel = transport_panel::TransportPanel::new(
        scenario.clone(),
        transport_panel::TransportConfiguration {
            python: None,
            openmc: None,
            audit: None,
            cross_sections: None,
            physics: Vec::new(),
            runs: Vec::new(),
            bundles: paths.to_vec(),
            runs_directory: runs_directory.to_path_buf(),
        },
    )?;
    Ok((panel, scenario))
}

/// The sweep's panel with no records yet and the scenario its first bundle
/// carries; the browser adds the bundles one frame at a time.
#[cfg(target_arch = "wasm32")]
fn start_sweep(
    first: &std::path::Path,
    runs_directory: &std::path::Path,
) -> Result<(transport_panel::TransportPanel, LoadedScenario), String> {
    let scenario = scenario_from_bundle(first).map_err(|e| e.to_string())?;
    let panel = transport_panel::TransportPanel::new(
        scenario.clone(),
        transport_panel::TransportConfiguration {
            python: None,
            openmc: None,
            audit: None,
            cross_sections: None,
            physics: Vec::new(),
            runs: Vec::new(),
            bundles: Vec::new(),
            runs_directory: runs_directory.to_path_buf(),
        },
    )?;
    Ok((panel, scenario))
}

/// A scenario file, read where the loaders read recorded files (the browser
/// build has them in memory).
fn load_scenario(path: &Path) -> Result<LoadedScenario, String> {
    let bytes = faris_engine::vfs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    LoadedScenario::from_bytes(&bytes).map_err(|e| e.to_string())
}

fn scenario_from_bundle(
    path: &std::path::Path,
) -> Result<LoadedScenario, Box<dyn std::error::Error>> {
    Ok(faris_engine::core_evidence::scenario_from_bundle_file(
        path,
    )?)
}

/// Keep the chosen interface size for the next start, or forget it (UX-060).
fn remember_interface_size(scale: Option<f32>) {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(path) = interface_prefs::path() {
        match scale {
            Some(scale) => interface_prefs::save(&path, scale.clamp(0.75, 2.0)),
            None => interface_prefs::reset(&path),
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = scale;
}

fn interface_size_menu(ui: &mut egui::Ui) {
    let current = ui.ctx().zoom_factor();
    ui.menu_button(format!("Interface size: {:.0}%", current * 100.0), |ui| {
        ui.label("Scale text, controls, panels, and plots");
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
            if ui
                .selectable_label(
                    (current - scale).abs() < 0.001,
                    format!("{:.0}%", scale * 100.0),
                )
                .clicked()
            {
                ui.ctx().set_zoom_factor(scale);
                remember_interface_size(Some(scale));
                ui.close();
            }
        }
        ui.separator();
        egui::gui_zoom::zoom_menu_buttons(ui);
        if (ui.ctx().zoom_factor() - current).abs() > 0.001 {
            remember_interface_size(Some(ui.ctx().zoom_factor()));
        }
        ui.separator();
        if ui
            .button("Reset to default size")
            .on_hover_text("Return to 100 % and forget the size saved for the next start.")
            .clicked()
        {
            ui.ctx().set_zoom_factor(1.0);
            remember_interface_size(None);
            ui.close();
        }
        ui.label("100% follows your desktop's display scaling.");
    })
    .response
    .on_hover_text(
        "Resize the whole interface. Ctrl/Cmd + or − adjusts size; Ctrl/Cmd 0 resets it.",
    );
}

struct ComponentMesh {
    id: String,
    vertices: Arc<[MeshVertex]>,
    color: [f32; 3],
}

struct Benchmark {
    duration: f64,
    motion: bool,
    launch_started: Instant,
    ready_at: Option<Instant>,
    frame_times: Vec<Instant>,
    completed: bool,
}

struct FarisApp {
    manifest: DemoManifest,
    variant: usize,
    selected: String,
    hidden: BTreeSet<String>,
    cutaway: bool,
    camera: Camera,
    year: f64,
    meshes: Vec<ComponentMesh>,
    vertices: Arc<[viewport::Vertex]>,
    revision: u64,
    message: String,
    capture: Option<PathBuf>,
    capture_requested: bool,
    frames: usize,
    started: Instant,
    study: study_panel::StudyPanel,
    transport: transport_panel::TransportPanel,
    paired: Option<(DemoManifest, transport_panel::TransportPanel)>,
    /// The tool paths shared with the design windows (the Window Python for Production runs).
    tools: Option<design_tools::SharedTools>,
    history: history_panel::HistoryPanel,
    sweep: Option<sweep_panel::SweepPanel>,
    maintenance: maintenance_panel::MaintenancePanel,
    design_flux: design_transport_panel::DesignTransportPanel,
    design_check: design_check_panel::DesignCheckPanel,
    step: Step,
    benchmark: Option<Benchmark>,
    geometry_cache: BTreeMap<String, Arc<[MeshVertex]>>,
    interface_check: Option<interface_check::InterfaceCheck>,
    recorder: Option<recorder::Recorder>,
    tour: tour::Tour,
    tour_marker: Option<PathBuf>,
    /// Optional Avila Labs account controls; built once the egui context exists.
    suite: Option<Suite>,
    /// Whether the first-visit sign-in prompt may appear (not in scripted runs).
    sign_in_prompt: bool,
    file: study_file::FileState,
    package: Option<PackageSession>,
    export: export_panel::ExportPanel,
    /// Screen rectangle of the 3D viewport in the last frame, for the export picture.
    viewport_rect: Option<egui::Rect>,
    /// The 3D viewport's widget id, so a command can give it keyboard focus.
    viewport_id: Option<egui::Id>,
    /// Keyboard command palette (Ctrl/Cmd+K) over the command registry.
    palette: commands::Palette,
    /// The browser version's first-open card and About window.
    notice: browser::Notice,
}

impl FarisApp {
    fn new(
        manifest: DemoManifest,
        capture: Option<PathBuf>,
        core: Option<PathBuf>,
        runs_directory: PathBuf,
        transport: transport_panel::TransportPanel,
        paired: Option<(DemoManifest, transport_panel::TransportPanel)>,
        history: history_panel::HistoryPanel,
    ) -> Result<Self, faris_engine::mesh::MeshError> {
        let mut app = Self {
            selected: manifest.variants[0].components
                [1.min(manifest.variants[0].components.len() - 1)]
            .id
            .clone(),
            manifest,
            variant: 0,
            hidden: BTreeSet::new(),
            cutaway: true,
            camera: Camera::default(),
            year: 0.0,
            meshes: Vec::new(),
            vertices: Arc::from([]),
            revision: 0,
            message: "Geometry preview ready. Transport and lifetime calculations are pending."
                .into(),
            capture,
            capture_requested: false,
            frames: 0,
            started: Instant::now(),
            study: study_panel::StudyPanel::new(core, runs_directory),
            transport,
            paired,
            tools: None,
            history,
            sweep: None,
            maintenance: maintenance_panel::MaintenancePanel::default(),
            design_flux: design_transport_panel::DesignTransportPanel::default(),
            design_check: design_check_panel::DesignCheckPanel::default(),
            step: Step::Design,
            benchmark: None,
            geometry_cache: BTreeMap::new(),
            interface_check: None,
            recorder: None,
            tour: tour::Tour::default(),
            tour_marker: None,
            suite: None,
            sign_in_prompt: false,
            file: study_file::FileState::default(),
            package: None,
            export: export_panel::ExportPanel::new(None),
            viewport_rect: None,
            viewport_id: None,
            palette: commands::Palette::default(),
            notice: browser::Notice::default(),
        };
        if app.transport.has_results() {
            app.message = "Checked transport records loaded.".into();
        }
        app.reset_for_session()?;
        Ok(app)
    }

    /// Initial per-study state once the arrangements and history panel are in
    /// place: first allocation, default selection, camera, and the Core
    /// analyses the loaded inputs can support.
    fn reset_for_session(&mut self) -> Result<(), faris_engine::mesh::MeshError> {
        let show_history = self.history.assumptions.is_some();
        self.variant = 0;
        self.hidden.clear();
        self.selected = self.manifest.variants[0].components
            [1.min(self.manifest.variants[0].components.len() - 1)]
        .id
        .clone();
        self.study.selection.fuel_history = show_history;
        self.study.selection.electricity = show_history
            && self
                .transport
                .response(&self.manifest.variants[0].id, "heating-total-whole-model")
                .is_some();
        self.camera = Camera::default();
        if self.transport.view == transport_panel::FieldView::FluxSlice
            && let Some(record) = self
                .transport
                .record(&self.manifest.variants[self.variant].id)
        {
            self.camera.frame_bounds(
                record.mesh.lower_left_m.map(|x| x as f32),
                record.mesh.upper_right_m.map(|x| x as f32),
            );
        }
        self.rebuild()
    }

    fn rebuild(&mut self) -> Result<(), faris_engine::mesh::MeshError> {
        self.meshes.clear();
        let mut slice_vertices: Option<Arc<[viewport::Vertex]>> = None;
        if self.transport.view == transport_panel::FieldView::FluxSlice
            && let Some(record) = self
                .transport
                .record(&self.manifest.variants[self.variant].id)
            && let Some(result) = &record.normalized
        {
            let mesh = &record.mesh;
            let widths: [f32; 3] = std::array::from_fn(|axis| {
                ((mesh.upper_right_m[axis] - mesh.lower_left_m[axis])
                    / mesh.dimensions[axis] as f64) as f32
            });
            let mut vertices = Vec::new();
            for response in &result.results {
                let faris_model::transport::ResponseDomain::Mesh { bin, .. } = &response.domain
                else {
                    continue;
                };
                if ((*bin as usize / mesh.dimensions[0]) % mesh.dimensions[1])
                    != self.transport.slice
                {
                    continue;
                }
                let Some(center) = mesh.bin_center_m(*bin as usize) else {
                    continue;
                };
                let x = center[0] as f32;
                let y = center[1] as f32;
                let z = center[2] as f32;
                let dx = widths[0] * 0.48;
                let dz = widths[2] * 0.48;
                let points = [
                    [x - dx, y, z - dz],
                    [x + dx, y, z - dz],
                    [x + dx, y, z + dz],
                    [x - dx, y, z + dz],
                ];
                let color = transport_panel::flux_color(response.mean, response.standard_error);
                for index in [0, 2, 1, 0, 3, 2] {
                    vertices.push(viewport::Vertex {
                        position: points[index],
                        normal: [0.0, 1.0, 0.0],
                        color,
                    });
                }
            }
            slice_vertices = Some(vertices.into());
        }
        let sweep = if self.cutaway { CUTAWAY_SWEEP } else { TAU };
        for component in &self.manifest.variants[self.variant].components {
            if component.material_id == "void" || self.hidden.contains(&component.id) {
                continue;
            }
            let cache_key = format!(
                "{}::{}::{}::{}",
                self.manifest.source_sha256,
                self.manifest.variants[self.variant].id,
                self.cutaway,
                component.id
            );
            let mesh = if let Some(mesh) = self.geometry_cache.get(&cache_key) {
                mesh.clone()
            } else {
                let generated = if let Some(faris_model::Penetration::OutboardRectangularPrism {
                    bounds_m,
                    affected_component_ids,
                    ..
                }) = &self.manifest.penetration
                    && affected_component_ids.contains(&component.id)
                {
                    torus_shell_with_prism_cut(
                        self.manifest.major_radius_m as f32,
                        component.inner_minor_radius_m as f32,
                        component.outer_minor_radius_m as f32,
                        sweep,
                        bounds_m.minimum_xyz_m.map(|x| x as f32),
                        bounds_m.maximum_xyz_m.map(|x| x as f32),
                    )?
                } else {
                    torus_shell(
                        self.manifest.major_radius_m as f32,
                        component.inner_minor_radius_m as f32,
                        component.outer_minor_radius_m as f32,
                        sweep,
                    )?
                };
                // Cache finite scenario tessellations; selection/color edits need no geometry work.
                let cache_bytes: usize = self
                    .geometry_cache
                    .values()
                    .map(|m| m.len() * std::mem::size_of::<MeshVertex>())
                    .sum();
                if cache_bytes + generated.len() * std::mem::size_of::<MeshVertex>()
                    > 128 * 1024 * 1024
                {
                    self.geometry_cache.clear();
                }
                let mesh: Arc<[MeshVertex]> = generated.into();
                self.geometry_cache.insert(cache_key, mesh.clone());
                mesh
            };
            let mut color = [0.0; 3];
            for (channel, value) in color.iter_mut().enumerate() {
                *value = u8::from_str_radix(&component.color[1 + channel * 2..3 + channel * 2], 16)
                    .expect("validated hexadecimal color") as f32
                    / 255.0;
                if component.id == self.selected {
                    *value = (*value * 1.1 + 0.12).min(1.0);
                }
            }
            if self.transport.view == transport_panel::FieldView::ComponentFlux {
                color = self
                    .transport
                    .response(
                        &self.manifest.variants[self.variant].id,
                        &format!("{}-flux", component.id),
                    )
                    .map_or([0.75, 0.10, 0.65], |response| {
                        transport_panel::flux_color(response.mean, response.standard_error)
                    });
            }
            if self.transport.view == transport_panel::FieldView::NuclearHeating {
                color = self
                    .transport
                    .response(
                        &self.manifest.variants[self.variant].id,
                        &format!("heating-total-{}", component.id),
                    )
                    .map_or([0.75, 0.10, 0.65], |r| {
                        transport_panel::scalar_color(r.mean, r.standard_error, 0.0, 8.0)
                    });
            }
            if self.transport.view == transport_panel::FieldView::ComponentFluence {
                color = self
                    .history
                    .snapshot(
                        &self.manifest.source_sha256,
                        &self.manifest.variants[self.variant].id,
                        self.year * faris_engine::history::JULIAN_YEAR_SECONDS,
                    )
                    .and_then(|s| s.component_fluence_n_m2.get(&component.id))
                    .map_or([0.75, 0.10, 0.65], |v| {
                        transport_panel::scalar_color(*v, 0.0, 18.0, 28.0)
                    });
            }
            self.meshes.push(ComponentMesh {
                id: component.id.clone(),
                vertices: mesh,
                color,
            });
        }
        // Component fields change a few colors, while the scenario-derived
        // geometry stays in its cached MeshVertex buffers. Spatial slices keep
        // their separately colored per-bin vertices; the component meshes then
        // serve as faint context geometry around the slice.
        self.vertices = slice_vertices.unwrap_or_else(|| Arc::from([]));
        self.revision += 1;
        Ok(())
    }

    /// Back to the default view (or the slice, in the slice view).
    fn frame_scene(&mut self) {
        self.camera = Camera::default();
        if self.transport.view == transport_panel::FieldView::FluxSlice
            && let Some(record) = self
                .transport
                .record(&self.manifest.variants[self.variant].id)
        {
            self.camera.frame_bounds(
                record.mesh.lower_left_m.map(|x| x as f32),
                record.mesh.upper_right_m.map(|x| x as f32),
            );
        }
    }

    /// Keys while the 3D viewport has focus: arrows orbit (Shift pans), plus
    /// and minus zoom, 0 resets, Enter or Space reads out the view centre.
    fn viewport_keys(&mut self, ctx: &egui::Context, rect: egui::Rect) {
        use egui::{Key, Modifiers};
        let mut reset = false;
        let mut read_out = false;
        let camera = &mut self.camera;
        ctx.input_mut(|input| {
            // Shift first: `consume_key` ignores a surplus Shift.
            for (modifiers, pan) in [(Modifiers::SHIFT, true), (Modifiers::NONE, false)] {
                for key in [
                    Key::ArrowLeft,
                    Key::ArrowRight,
                    Key::ArrowUp,
                    Key::ArrowDown,
                ] {
                    if input.consume_key(modifiers, key) {
                        camera.nudge(key, pan);
                    }
                }
            }
            for key in [Key::Plus, Key::Equals, Key::Minus] {
                if input.consume_key(Modifiers::NONE, key) {
                    camera.nudge(key, false);
                }
            }
            reset = input.consume_key(Modifiers::NONE, Key::Num0);
            read_out = input.consume_key(Modifiers::NONE, Key::Enter)
                || input.consume_key(Modifiers::NONE, Key::Space);
        });
        if reset {
            self.frame_scene();
            self.message = "3D view reset to frame the scene.".into();
        }
        if read_out {
            self.read_out_viewport(rect);
        }
    }

    /// Say in the status line what the centre of the view shows.
    fn read_out_viewport(&mut self, rect: egui::Rect) {
        let slice = self.transport.view == transport_panel::FieldView::FluxSlice
            && self
                .transport
                .record(&self.manifest.variants[self.variant].id)
                .is_some();
        if !self.pick(rect, rect.center()) {
            self.message = format!(
                "3D viewport: nothing at the centre of the view. Selected component stays {}. Camera distance {:.1} m.",
                self.selected, self.camera.distance
            );
        } else if !slice {
            let label = self.manifest.variants[self.variant]
                .components
                .iter()
                .find(|c| c.id == self.selected)
                .map_or(self.selected.clone(), |c| c.label.clone());
            self.message = format!(
                "3D viewport: the centre of the view shows {label} ({}), now selected. Camera distance {:.1} m.",
                self.selected, self.camera.distance
            );
        }
    }

    fn pick(&mut self, rect: egui::Rect, point: egui::Pos2) -> bool {
        let (origin, direction) = self.camera.ray(rect, point);
        if self.transport.view == transport_panel::FieldView::FluxSlice
            && let Some(record) = self
                .transport
                .record(&self.manifest.variants[self.variant].id)
        {
            let mesh = &record.mesh;
            let mut hit = false;
            let y = mesh
                .bin_center_m(mesh.dimensions[0] * self.transport.slice)
                .expect("valid slice")[1] as f32;
            let distance = (y - origin[1]) / direction[1];
            if distance.is_finite() && distance >= 0.0 {
                let point = [
                    origin[0] + distance * direction[0],
                    y,
                    origin[2] + distance * direction[2],
                ];
                if [0, 2].iter().all(|axis| {
                    point[*axis] as f64 >= mesh.lower_left_m[*axis]
                        && (point[*axis] as f64) < mesh.upper_right_m[*axis]
                }) {
                    let index = |axis: usize| {
                        (((point[axis] as f64 - mesh.lower_left_m[axis])
                            / (mesh.upper_right_m[axis] - mesh.lower_left_m[axis]))
                            * mesh.dimensions[axis] as f64)
                            .floor() as usize
                    };
                    let bin = index(0)
                        + mesh.dimensions[0]
                            * (self.transport.slice + mesh.dimensions[1] * index(2));
                    if let Some(response) = self.transport.mesh_response(&record.variant_id, bin) {
                        hit = true;
                        self.message = format!(
                            "{} · {} · Y slice {} · bin {bin}: {:.3e} ± {:.2e} neutrons/m²/s (Monte Carlo SE); full-bin average. Qualification NOT_EVALUATED.",
                            self.manifest.scenario_id,
                            record.variant_id,
                            self.transport.slice,
                            response.mean,
                            response.standard_error
                        );
                    }
                }
            }
            return hit;
        }
        let mut nearest = f32::INFINITY;
        let mut selected = None;
        for mesh in &self.meshes {
            for triangle in mesh.vertices.as_chunks::<3>().0 {
                if let Some(distance) = triangle_hit(
                    origin,
                    direction,
                    [
                        triangle[0].position,
                        triangle[1].position,
                        triangle[2].position,
                    ],
                ) && distance < nearest
                {
                    nearest = distance;
                    selected = Some(mesh.id.clone());
                }
            }
        }
        let hit = selected.is_some();
        if let Some(selected) = selected {
            self.selected = selected;
        }
        hit
    }

    /// Double-click framing: select the component under the pointer and move
    /// the camera goal to its bounds (the displayed camera eases there).
    fn frame_component(&mut self, rect: egui::Rect, point: egui::Pos2) {
        if self.transport.view == transport_panel::FieldView::FluxSlice {
            return;
        }
        let components: Vec<viewport::ComponentDraw> = self
            .meshes
            .iter()
            .map(|mesh| viewport::ComponentDraw {
                vertices: mesh.vertices.clone(),
                color: mesh.color,
            })
            .collect();
        if let Some(index) = viewport::pick_component(&self.camera, rect, point, &components) {
            let mesh = &self.meshes[index];
            self.selected = mesh.id.clone();
            let mut minimum = [f32::INFINITY; 3];
            let mut maximum = [f32::NEG_INFINITY; 3];
            for vertex in mesh.vertices.iter() {
                for axis in 0..3 {
                    minimum[axis] = minimum[axis].min(vertex.position[axis]);
                    maximum[axis] = maximum[axis].max(vertex.position[axis]);
                }
            }
            if minimum.iter().chain(&maximum).all(|x| x.is_finite()) {
                self.camera.frame_bounds(minimum, maximum);
            }
        }
    }

    /// Everything the frame recorder waits for before its first frame,
    /// including the automatic uncertainty ensembles and the allocation sweep.
    fn recording_settled(&self) -> bool {
        self.frames >= 8
            && !self.file.is_busy()
            && !self.history.is_pending()
            && !self.history.uncertainty_pending()
            && !self.sweep.as_ref().is_some_and(|s| s.is_pending())
            && !self.maintenance.is_pending()
            && !self.study.archive.is_loading()
            && self.export.development_settled()
    }

    /// Development frame recording (`--record-frames`): starts once the app is
    /// settled (the plan's clock, if any) and closes it when recording ends.
    fn record_frame(&mut self, ctx: &egui::Context) {
        let ready =
            self.recording_settled() && self.interface_check.as_ref().is_none_or(|c| c.armed());
        let plan_done = self.interface_check.as_ref().is_some_and(|c| c.finished());
        if let Some(recorder) = &mut self.recorder
            && recorder.frame(ctx, ready, plan_done)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn capture_frame(&mut self, ctx: &egui::Context) {
        let Some(path) = &self.capture else {
            return;
        };
        if self.frames >= 8
            && !self.capture_requested
            && !self.file.is_busy()
            && !self.history.is_pending()
            && !self.history.uncertainty_pending()
            && !self.sweep.as_ref().is_some_and(|s| s.is_pending())
            && !self.study.archive.is_loading()
            && !self.design_check.tools_busy()
            && self.tour.settled()
            && self.interface_check.as_ref().is_none_or(|c| c.finished())
            && self.export.development_settled()
            && self.started.elapsed().as_secs_f64() >= 1.0
        {
            self.capture_requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        let screenshot = ctx.input(|input| {
            input.events.iter().find_map(|event| {
                // Screenshots tagged by the export belong to the export panel.
                if let egui::Event::Screenshot {
                    image, user_data, ..
                } = event
                    && user_data.data.is_none()
                {
                    Some(image.clone())
                } else {
                    None
                }
            })
        });
        if let Some(screenshot) = screenshot {
            let bytes: Vec<u8> = screenshot
                .pixels
                .iter()
                .flat_map(|pixel| pixel.to_array())
                .collect();
            let result = image::save_buffer(
                path,
                &bytes,
                screenshot.size[0] as u32,
                screenshot.size[1] as u32,
                image::ColorType::Rgba8,
            );
            match result {
                Ok(()) => eprintln!("Captured FARIS to {}", path.display()),
                Err(error) => eprintln!("FARIS capture failed: {error}"),
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if self.started.elapsed().as_secs()
            > if self.interface_check.is_some() {
                170
            } else if self.sweep.is_some() || self.file.is_busy() {
                // Seven 16 MiB bundles validate slowly in unoptimized builds.
                400
            } else {
                100
            }
        {
            eprintln!("FARIS capture timed out");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            ctx.request_repaint();
        }
    }
}

fn hex_color(hex: &str) -> egui::Color32 {
    let channel = |i: usize| {
        hex.get(1 + i * 2..3 + i * 2)
            .and_then(|h| u8::from_str_radix(h, 16).ok())
            .unwrap_or(128)
    };
    egui::Color32::from_rgb(channel(0), channel(1), channel(2))
}

/// One horizontal bar of stacked layers, widths proportional to thickness.
/// Layers whose thickness differs from `other` are outlined in gold.
fn draw_bar(
    ui: &mut egui::Ui,
    lane: &str,
    variant: &VariantGeometry,
    other: Option<&VariantGeometry>,
    height: f32,
    total_m: f64,
    selected: &mut String,
) {
    let gold = egui::Color32::from_rgb(232, 178, 92);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let mut x = rect.left();
    for component in &variant.components {
        let w = (component.thickness_m / total_m) as f32 * width;
        let segment = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(w, height));
        x += w;
        let response = ui
            .interact(
                segment,
                ui.id().with(("radial-build", lane, &component.id)),
                egui::Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let fill = hex_color(&component.color);
        ui.painter().rect_filled(segment, 0.0, fill);
        let counterpart = other
            .and_then(|o| o.components.iter().find(|c| c.id == component.id))
            .filter(|c| (c.thickness_m - component.thickness_m).abs() > 1e-9);
        if counterpart.is_some() {
            ui.painter().rect_stroke(
                segment.shrink(1.0),
                0.0,
                egui::Stroke::new(2.0, gold),
                egui::StrokeKind::Inside,
            );
        }
        if *selected == component.id {
            ui.painter().rect_stroke(
                segment,
                0.0,
                egui::Stroke::new(2.0, egui::Color32::WHITE),
                egui::StrokeKind::Inside,
            );
        }
        if w >= 34.0 && height >= 22.0 {
            let luminance =
                0.299 * fill.r() as f32 + 0.587 * fill.g() as f32 + 0.114 * fill.b() as f32;
            ui.painter().text(
                segment.center(),
                egui::Align2::CENTER_CENTER,
                format!("{:.2}", component.thickness_m),
                egui::FontId::proportional(11.0),
                if luminance > 140.0 {
                    egui::Color32::BLACK
                } else {
                    egui::Color32::WHITE
                },
            );
        }
        if response.clicked() {
            *selected = component.id.clone();
        }
        response.on_hover_ui(|ui| {
            ui.strong(&component.label);
            ui.label(format!(
                "{:.3} m thick · {}",
                component.thickness_m, component.material_id
            ));
            ui.weak(format!("{} · click to select", variant.label));
            if let Some(counterpart) = counterpart {
                ui.label(format!(
                    "Differs from the other allocation: {:.3} m there.",
                    counterpart.thickness_m
                ));
            }
        });
    }
}

/// Radial-build diagram: active allocation on top, the other beneath.
fn radial_build(
    ui: &mut egui::Ui,
    active: &VariantGeometry,
    other: Option<&VariantGeometry>,
    selected: &mut String,
) {
    let total = |v: &VariantGeometry| v.components.iter().map(|c| c.thickness_m).sum::<f64>();
    let total_m = total(active).max(other.map_or(0.0, total)).max(1e-9);
    ui.strong("Radial build");
    ui.weak("First wall to magnets · widths proportional to thickness");
    ui.add_space(4.0);
    ui.small(format!("{} (shown)", active.label));
    draw_bar(ui, "active", active, other, 28.0, total_m, selected);
    if let Some(other) = other {
        ui.add_space(4.0);
        draw_bar(ui, "other", other, Some(active), 14.0, total_m, selected);
        ui.small(format!("{} (comparison)", other.label));
        ui.weak("Outlined layers differ between the two allocations.");
    }
    if let Some(component) = active.components.iter().find(|c| c.id == *selected) {
        ui.add_space(4.0);
        ui.small(format!(
            "Selected: {} · {:.3} m",
            component.label, component.thickness_m
        ));
    }
}

/// "Reference · blanket 0.45 m · shield 0.45 m" for an allocation choice.
fn allocation_text(variant: &VariantGeometry) -> String {
    let mut text = variant.label.clone();
    for id in ["blanket", "shield"] {
        if let Some(component) = variant.components.iter().find(|c| c.id == id) {
            text.push_str(&format!(" · {id} {:.2} m", component.thickness_m));
        }
    }
    text
}

impl FarisApp {
    fn step_bar(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.x = 2.0;
        let mut bounds = egui::Rect::NOTHING;
        for step in Step::ALL {
            let response = ui
                .selectable_label(
                    self.step == step,
                    format!("{} {}", step.number(), step.name()),
                )
                .on_hover_text(format!("Press {} to switch here", step.number()));
            bounds = bounds.union(response.rect);
            if response.clicked() {
                self.step = step;
            }
        }
        self.tour.anchor("steps-bar", bounds);
    }

    fn start_tour(&mut self, stop: usize) {
        self.tour.start(
            tour::Saved {
                step: self.step,
                view: self.transport.view,
                year: self.year,
            },
            stop,
        );
    }

    /// Apply a tour transition to the app state. A stop that shows a history
    /// waits for the histories to finish so it never falls back needlessly.
    fn apply_tour(&mut self) {
        let Some(effect) = self.tour.pending() else {
            return;
        };
        let variant = self.manifest.variants[self.variant].id.clone();
        let (view, year, step) = match effect {
            tour::Effect::Enter(index) => {
                let stop = &tour::STOPS[index];
                if stop.year.is_some()
                    && (self.history.is_pending()
                        || (self.history.assumptions.is_some() && self.frames < 8))
                {
                    return;
                }
                let has_history = self
                    .history
                    .result(&self.manifest.source_sha256, &variant)
                    .is_some();
                let has_transport = self
                    .transport
                    .record(&variant)
                    .is_some_and(|r| r.normalized.is_some());
                let view = stop.view.map(|view| match view {
                    transport_panel::FieldView::FluxSlice if !has_transport => {
                        transport_panel::FieldView::Materials
                    }
                    transport_panel::FieldView::ComponentFluence if !has_history => {
                        transport_panel::FieldView::Materials
                    }
                    view => view,
                });
                let year = stop.year.map(|spec| match spec {
                    tour::YearSpec::Fixed(year) => year,
                    tour::YearSpec::FirstMagnetReplacement { fallback } => self
                        .history
                        .first_magnet_replacement_year(&self.manifest.source_sha256, &variant)
                        .unwrap_or(fallback),
                });
                (view, year, stop.step)
            }
            tour::Effect::Skipped(saved) => (Some(saved.view), Some(saved.year), Some(saved.step)),
            tour::Effect::Finished => (
                Some(transport_panel::FieldView::Materials),
                Some(0.0),
                Some(Step::Design),
            ),
        };
        if let Some(step) = step {
            self.step = step;
        }
        if let Some(view) = view {
            if self.transport.view == transport_panel::FieldView::FluxSlice
                && view != transport_panel::FieldView::FluxSlice
            {
                self.camera = Camera::default();
            }
            self.transport.view = view;
        }
        if let Some(year) = year {
            self.year = year.clamp(0.0, self.manifest.horizon_years);
        }
        if matches!(effect, tour::Effect::Skipped(_) | tour::Effect::Finished)
            && let Some(path) = &self.tour_marker
        {
            tour::write_marker(path);
        }
        self.tour.clear_pending();
    }

    /// Record that the tour was offered and finished or skipped.
    fn write_tour_marker(&self) {
        if let Some(path) = &self.tour_marker {
            tour::write_marker(path);
        }
    }

    /// The header menu of the browser version: About, and what it leaves to the
    /// desktop app.
    fn browser_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("Browser version", |ui| {
            if ui.button("About this browser version").clicked() {
                self.notice.open_about();
                ui.close();
            }
            ui.separator();
            ui.weak("Not in the browser version:");
            for feature in browser::DESKTOP_ONLY {
                ui.weak(format!("  {feature}"));
            }
            ui.hyperlink_to("Get the desktop app", browser::DESKTOP_URL);
        });
    }

    fn swap_arrangement(&mut self) {
        if let Some((manifest, panel)) = &mut self.paired {
            std::mem::swap(&mut self.manifest, manifest);
            std::mem::swap(&mut self.transport, panel);
            self.transport.view = panel.view;
            self.message = "Switched physical scenario. Recorded results retain their distinct scenario identities.".into();
            self.rebuild()
                .unwrap_or_else(|error| self.message = error.to_string());
        }
    }

    fn design_step(&mut self, ui: &mut egui::Ui) {
        ui.label(&self.manifest.title);
        ui.weak("ARC-inspired · idealized geometry");
        ui.add_space(12.0);
        let arrangement_top = ui.cursor().top();
        ui.strong("Arrangement");
        ui.add_space(4.0);
        let has_port = self.manifest.penetration.is_some();
        ui.horizontal_wrapped(|ui| {
            ui.label("Port");
            if self.paired.is_some() {
                let mut swap = false;
                swap |= ui
                    .selectable_label(has_port, "With outboard port")
                    .clicked()
                    && !has_port;
                swap |= ui
                    .selectable_label(!has_port, "No port (matched control)")
                    .clicked()
                    && has_port;
                if swap {
                    self.swap_arrangement();
                }
            } else {
                let _ = ui.selectable_label(
                    true,
                    if has_port {
                        "With outboard port"
                    } else {
                        "No port (feature-free control)"
                    },
                );
                ui.weak("matched arrangement not loaded");
            }
        });
        ui.add_space(4.0);
        ui.label("Allocation");
        for (index, variant) in self.manifest.variants.iter().enumerate() {
            ui.selectable_value(&mut self.variant, index, allocation_text(variant));
        }
        ui.add_space(12.0);
        let active = &self.manifest.variants[self.variant];
        let other = self
            .manifest
            .variants
            .iter()
            .enumerate()
            .find(|(index, _)| *index != self.variant)
            .map(|(_, v)| v);
        radial_build(ui, active, other, &mut self.selected);
        let arrangement = egui::Rect::from_min_max(
            egui::pos2(ui.max_rect().left(), arrangement_top),
            egui::pos2(ui.max_rect().right(), ui.cursor().top()),
        );
        self.tour.anchor_in(ui, "design-arrangement", arrangement);
        ui.add_space(12.0);
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.strong("Plant inputs");
            badge::badge(
                ui,
                badge::Kind::Authored,
                badge::Kind::Authored.default_label(),
                "Scenario inputs authored for this ARC-inspired study; tunable, not measured.",
            );
        });
        egui::ScrollArea::horizontal()
            .id_salt("plant-inputs-scroll")
            .show(ui, |ui| {
                egui::Grid::new("plant-inputs").show(ui, |ui| {
                    for (label, value) in [
                        (
                            "Major radius",
                            format!("{:.2} m", self.manifest.major_radius_m),
                        ),
                        (
                            "Radial build",
                            format!("{:.2} m", self.manifest.radial_build_m),
                        ),
                        (
                            "Fusion power",
                            format!("{:.0} MW", self.manifest.fusion_power_mw),
                        ),
                    ] {
                        ui.label(label);
                        ui.label(value);
                        ui.end_row();
                    }
                });
            });
        ui.add_space(12.0);
        ui.collapsing("Sources and assumptions", |ui| {
            if !self.manifest.references.is_empty() {
                badge::badge(
                    ui,
                    badge::Kind::Literature,
                    badge::Kind::Literature.default_label(),
                    "Cited sources for scenario inputs. Linking a source does not qualify the value.",
                );
            }
            for reference in &self.manifest.references {
                ui.hyperlink_to(&reference.title, &reference.url);
            }
            badge::badge(
                ui,
                badge::Kind::Authored,
                "authored assumptions",
                "Assumptions authored for this scenario; not measurements or literature values.",
            );
            for assumption in &self.manifest.assumptions {
                ui.small(assumption);
                ui.add_space(5.0);
            }
        });
    }

    fn transport_card(&self, ui: &mut egui::Ui) {
        let variant = &self.manifest.variants[self.variant];
        let note = "Monte Carlo ± one standard error; cold-data surrogate; scientific qualification NOT_EVALUATED.";
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.strong("Transport results");
            ui.weak(allocation_text(variant));
            let Some(record) = self
                .transport
                .record(&variant.id)
                .filter(|r| r.normalized.is_some())
            else {
                ui.add_space(4.0);
                ui.weak("No transport results for this arrangement. Run transport below.");
                return;
            };
            let rate = record
                .normalized
                .as_ref()
                .expect("normalized record")
                .source_neutron_rate_per_s;
            ui.add_space(8.0);
            if let Some(tally) = self
                .transport
                .response(&variant.id, "total-tritium-production")
                .or_else(|| self.transport.response(&variant.id, "blanket-tritium"))
            {
                badge::metric(
                    ui,
                    "Tritium breeding · H3 per source neutron",
                    &format!(
                        "{:.4} ± {:.4}",
                        tally.integrated_mean / rate,
                        tally.integrated_standard_error / rate
                    ),
                    badge::Kind::Calculated,
                    &format!("Gross births per primary neutron; recovery and fuel availability are separate. {note}"),
                );
                ui.add_space(6.0);
            }
            if let Some(flux) = self.transport.response(&variant.id, "magnets-flux") {
                let (value, explanation) = if flux.mean == 0.0 && flux.standard_error == 0.0 {
                    (
                        "no sampled tracks".to_owned(),
                        format!("No sampled tracks; this does not establish zero flux or an upper bound. {note}"),
                    )
                } else {
                    (
                        format!("{:.2e} ± {:.1e}", flux.mean, flux.standard_error),
                        format!(
                            "Component volume average in neutrons/m²/s; relative sampling SE {:.1}%. Volume and model/data uncertainty are separate. {note}",
                            100.0 * flux.standard_error / flux.mean
                        ),
                    )
                };
                badge::metric(
                    ui,
                    "Magnet-region mean flux · neutrons/m²/s",
                    &value,
                    badge::Kind::Calculated,
                    &explanation,
                );
                ui.add_space(6.0);
            }
            if let Some(heat) = self
                .transport
                .response(&variant.id, "heating-total-whole-model")
            {
                badge::metric(
                    ui,
                    "Total nuclear heating · MW",
                    &format!(
                        "{:.2} ± {:.2}",
                        heat.integrated_mean / 1e6,
                        heat.integrated_standard_error / 1e6
                    ),
                    badge::Kind::Calculated,
                    &format!("Coupled neutron/photon deposition. Includes material reaction energy and can exceed D–T source power; heat recovery is an authored assumption. {note}"),
                );
                ui.add_space(6.0);
            }
            badge::metric(
                ui,
                "Histories",
                &transport_panel::grouped(
                    record.histories_sampled() as usize,
                ),
                badge::Kind::Calculated,
                &format!("Particle histories sampled in the recorded run (seed {}). {note}", record.sampling.seed),
            );
        });
    }

    fn simulate_step(&mut self, ui: &mut egui::Ui) {
        let card_top = ui.cursor().top();
        self.transport_card(ui);
        let card = egui::Rect::from_min_max(
            egui::pos2(ui.max_rect().left(), card_top),
            egui::pos2(ui.max_rect().right(), ui.min_rect().bottom()),
        );
        self.tour.anchor_in(ui, "transport-card", card);
        ui.add_space(12.0);
        self.transport.controls(
            ui,
            &self.manifest.variants[self.variant].id,
            self.tools.as_ref(),
        );
    }

    fn operate_step(&mut self, ui: &mut egui::Ui) {
        ui.label("Scrub the timeline below; edits recalculate the histories of every arrangement and, when present, of the sweep.");
        ui.add_space(8.0);
        if let Some(what_if) = self.history.controls(ui) {
            self.tour.anchor_in(ui, "what-if", what_if);
        }
        ui.add_space(8.0);
        self.history.sensitivity_controls(
            ui,
            &self.manifest.source_sha256,
            &self.manifest.variants[self.variant].id,
        );
        self.history.uncertainty_controls(
            ui,
            &self.manifest.source_sha256,
            &self.manifest.variants[self.variant].id,
        );
    }

    fn compare_step(&mut self, ui: &mut egui::Ui) {
        ui.label("Compares two allocations, each with and without the outboard port, under the same envelope, materials and source.");
        ui.add_space(8.0);
        ui.weak("The comparison is in the panel below; drag its top edge to resize.");
        if self.paired.is_none() {
            ui.add_space(8.0);
            badge::badge(
                ui,
                badge::Kind::Partial,
                "single port setting loaded",
                "Only this arrangement's port setting is loaded. Provide the matched scenario (--control-scenario or --control-bundle) to compare port and no-port.",
            );
        }
    }

    fn evidence_step(&mut self, ui: &mut egui::Ui) {
        if let Some(package) = &self.package {
            for notice in [&package.development_binary, &package.evidence]
                .into_iter()
                .flatten()
            {
                package_notice(ui, notice);
            }
        }
        if let Some(note) = study_file::evidence_badge(&self.file.evidence) {
            ui.horizontal_wrapped(|ui| {
                badge::badge(ui, note.kind, note.label, &note.text);
            });
            ui.add_space(8.0);
        }
        // Store trees saved by reference: the badge, and the why and next step
        // also as text, so they do not depend on hovering.
        if let Some(note) = self
            .file
            .evidence_store
            .as_ref()
            .and_then(|s| study_file::store_evidence_badge(s, &self.file.evidence_trees))
        {
            package_notice(ui, &note);
        }
        // Saved Core receipts bind the assumptions they were executed with
        // (the loaded file). Offer that set explicitly instead of silently
        // showing receipts beside histories they do not cover.
        if self.study.archive.has_saved() && self.history.preset_label() != "Loaded assumptions" {
            ui.horizontal_wrapped(|ui| {
                badge::badge(
                    ui,
                    badge::Kind::Partial,
                    "receipts cover the loaded assumptions",
                    "The saved Core receipts were executed with the loaded operating assumptions. The current preset differs, so its histories are not covered by those receipts. Switch to see the covered histories, or run the bound study stages to cover the current preset.",
                );
                if ui.button("Use the covered assumptions").clicked() {
                    self.history.select_preset("Loaded assumptions");
                }
            });
            ui.add_space(8.0);
        }
        let variant = &self.manifest.variants[self.variant].id;
        self.study.controls(
            ui,
            variant,
            &self.manifest.source_sha256,
            self.transport.readiness(variant),
        );
        ui.add_space(8.0);
        self.study.evidence_controls(
            ui,
            &self.manifest,
            variant,
            self.transport.location(variant),
            self.history.assumptions.as_ref(),
        );
        if let Some(status) = self.study.status_rect.take() {
            self.tour.anchor_in(ui, "evidence-status", status);
        }
    }

    /// Bottom-panel content on the Compare step.
    fn compare_view(&mut self, ui: &mut egui::Ui) {
        // The displayed scenario swaps with its matched control; the
        // compare view always receives the port case first.
        let paired = self.paired.as_ref().map(|(_, panel)| panel);
        let (port, control) = match paired {
            Some(other) if self.manifest.penetration.is_none() => (other, Some(&self.transport)),
            _ => (&self.transport, paired),
        };
        let preset = self.history.preset_label().to_owned();
        let sweep = &mut self.sweep;
        compare_panel::compare_view(ui, &self.history, port, control, &mut |ui| {
            if let Some(sweep) = sweep.as_mut() {
                sweep.view(ui, &preset);
            }
        });
    }
}

impl FarisApp {
    /// The one place the saved study file enters the export. The export names
    /// the `.faris` file and its SHA-256 when this returns a stamp; None means
    /// an unsaved study. The study-file layer wires this to the open file.
    fn export_study_file_stamp(&self) -> Option<faris_report::StudyFileStamp> {
        self.study_file_stamp()
    }

    /// The name the export folder and PDF title carry: the study file's name
    /// once saved or opened.
    fn export_study_name(&self) -> String {
        faris_report::study_name_from_path(self.file.path.as_deref())
    }

    /// Why the export cannot start now, if it cannot.
    fn export_blocked(&self) -> Option<&'static str> {
        if self.history.assumptions.is_none() {
            Some("This study has no operating assumptions, so there are no histories to export.")
        } else if self.history.is_pending() || self.history.is_stale() {
            Some("The operating histories are still calculating; export when the timeline settles.")
        } else if self.history.uncertainty_pending() {
            Some("The uncertainty ranges are still being calculated; export when they finish.")
        } else if self.sweep.as_ref().is_some_and(|s| s.is_pending()) {
            Some("The allocation sweep is still loading or calculating.")
        } else if self.study.archive.is_loading() {
            Some("Saved evidence is still loading.")
        } else {
            None
        }
    }

    /// Everything the export needs, copied from the engine results on screen.
    /// The selection and layout of the data is `faris_report::assemble_report_input`,
    /// shared with `faris study-file export`.
    fn export_input(&self) -> Result<faris_report::ReportInput, String> {
        fn arrangement<'a>(
            manifest: &'a DemoManifest,
            panel: &'a transport_panel::TransportPanel,
        ) -> faris_report::LoadedArrangement<'a> {
            faris_report::LoadedArrangement {
                manifest,
                records: panel.records(),
            }
        }
        let paired = self.paired.as_ref().map(|(m, p)| arrangement(m, p));
        Ok(faris_report::assemble_report_input(
            arrangement(&self.manifest, &self.transport),
            paired,
            |scenario, variant| self.history.result(scenario, variant),
            |scenario, variant| self.history.export_ensemble(scenario, variant),
            faris_report::ReportContext {
                study_name: self.export_study_name(),
                sweep: self.sweep.as_ref().and_then(|s| s.export_data()),
                preset_label: self.history.preset_label().to_owned(),
                preset_magnet_limit: self.history.preset_magnet_limit(),
                fusion_power_mw: self.manifest.fusion_power_mw,
                study_file: self.export_study_file_stamp(),
                view_image_note: None,
                generated_unix_s: faris_report::now_unix_s(),
            },
        ))
    }

    fn begin_export(&mut self, ctx: &egui::Context) {
        match self.export_input() {
            Ok(input) => self.export.begin(ctx, input, self.viewport_rect),
            Err(error) => self.message = format!("Export not started: {error}"),
        }
    }

    /// `--export-on-load`: a development check that exports once everything is
    /// calculated, without the folder dialog.
    fn run_development_export(&mut self, ctx: &egui::Context) {
        if !self.export.development_requested()
            || self.frames < 8
            || self.export_blocked().is_some()
            || self.started.elapsed().as_secs_f64() < 1.0
        {
            return;
        }
        let Some(parent) = self.export.take_development_request() else {
            return;
        };
        match self.export_input() {
            Ok(input) => self
                .export
                .begin_into(ctx, input, self.viewport_rect, parent),
            Err(error) => {
                eprintln!("FARIS export not started: {error}");
                self.export.fail(error);
            }
        }
    }
}

impl eframe::App for FarisApp {
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        if let Some(check) = &mut self.interface_check {
            check.set_viewport(self.viewport_rect);
            check.inject(ctx, input);
            if let Some(year) = check.take_year_request() {
                self.year = year.clamp(0.0, self.manifest.horizon_years);
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        #[cfg(target_arch = "wasm32")]
        {
            self.poll_web(&ctx);
            // Pending work is advanced by frames in the browser; never wait for input.
            if self.history.is_pending()
                || self.sweep.as_ref().is_some_and(|s| s.is_pending())
                || self.file.is_busy()
                || self.maintenance.is_pending()
            {
                ctx.request_repaint();
            } else if self.started.elapsed() < crate::time::Duration::from_secs(20) {
                // A frame the browser dropped while the page was starting
                // (surface, scale or size changes) must not wait for input.
                ctx.request_repaint_after(crate::time::Duration::from_millis(250));
            }
        }
        let year_before_frame = self.year.to_bits();
        if self.frames == 0 {
            eprintln!(
                "FARIS first UI frame: {:.3} s after process setup began",
                self.started.elapsed().as_secs_f64()
            );
        }
        self.frames += 1;
        if let Some(b) = &mut self.benchmark {
            let now = Instant::now();
            // Warm up GPU and let the first history finish before sampling throughput.
            if b.ready_at.is_none()
                && now.duration_since(b.launch_started).as_secs_f64() >= 2.0
                && !self.history.is_pending()
                && !self.file.is_busy()
                && !self.study.archive.is_loading()
            {
                b.ready_at = Some(now);
            }
            if let Some(ready_at) = b.ready_at {
                b.frame_times.push(now);
                let elapsed = now.duration_since(ready_at).as_secs_f64();
                if b.motion {
                    self.camera.yaw = (elapsed * 0.45) as f32;
                    self.year = (elapsed / b.duration) * self.manifest.horizon_years;
                }
                if elapsed >= b.duration && !b.completed {
                    b.completed = true;
                    let mut intervals: Vec<f64> = b
                        .frame_times
                        .windows(2)
                        .map(|w| w[1].duration_since(w[0]).as_secs_f64())
                        .collect();
                    intervals.sort_by(f64::total_cmp);
                    let p95 = intervals
                        .get((intervals.len() * 95 / 100).min(intervals.len().saturating_sub(1)))
                        .copied()
                        .unwrap_or_default();
                    eprintln!(
                        "{}",
                        serde_json::json!({"schema_version":"faris-native-frame-benchmark/v0.1", "measurement":"egui frame throughput; not GPU presentation FPS", "first_measured_frame_seconds": ready_at.duration_since(b.launch_started).as_secs_f64(), "elapsed_seconds":elapsed,"interval_count":intervals.len(),"mean_frames_per_second":intervals.len() as f64/elapsed,"p95_frame_interval_ms":p95*1000.0,"orbit_and_scrub":b.motion,"window_points":ctx.input(|i|{let size=i.content_rect().size();[size.x,size.y]}),"pixels_per_point":ctx.pixels_per_point()})
                    );
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            // Continuous repaint models sustained camera/scrub input. Adding a
            // fixed sleep would measure our throttle plus rendering latency.
            ctx.request_repaint();
        }
        self.poll_file(&ctx);
        self.study.poll(&ctx);
        let history_before = self.history.revision();
        for (scenario, variant, history) in self.study.archive.take_histories() {
            let input = if scenario == self.manifest.source_sha256 {
                self.transport
                    .record(&variant)
                    .map(|r| (r, self.manifest.fusion_power_mw))
            } else {
                self.paired
                    .as_ref()
                    .filter(|(m, _)| m.source_sha256 == scenario)
                    .and_then(|(m, p)| p.record(&variant).map(|r| (r, m.fusion_power_mw)))
            };
            if let Some((run, power)) = input
                && let Err(error) = self.history.cache_saved_history(history, run, power)
            {
                self.message = format!("Saved case reopened; history not reused: {error}");
            }
        }
        self.history.poll(&ctx);
        let field_before = self.transport.render_key();
        self.apply_tour();
        self.transport.poll(&ctx);
        let mut history_inputs: Vec<_> = self
            .transport
            .records()
            .values()
            .filter(|r| r.normalized.is_some())
            .map(|r| {
                (
                    r,
                    self.manifest.fusion_power_mw,
                    format!(
                        "{} · {}",
                        r.variant_id,
                        if self.manifest.penetration.is_some() {
                            "penetration"
                        } else {
                            "control"
                        }
                    ),
                )
            })
            .collect();
        if let Some((manifest, panel)) = &self.paired {
            history_inputs.extend(
                panel
                    .records()
                    .values()
                    .filter(|r| r.normalized.is_some())
                    .map(|r| {
                        (
                            r,
                            manifest.fusion_power_mw,
                            format!(
                                "{} · {}",
                                r.variant_id,
                                if manifest.penetration.is_some() {
                                    "penetration"
                                } else {
                                    "control"
                                }
                            ),
                        )
                    }),
            );
        }
        if !self.study.archive.is_loading() {
            self.history.update_inputs(&ctx, &history_inputs);
        }
        self.history.set_selected(
            &self.manifest.source_sha256,
            &self.manifest.variants[self.variant].id,
        );
        self.history.drive_uncertainty(&ctx);
        if let Some(sweep) = &mut self.sweep {
            sweep.update(&ctx, self.history.assumptions.as_ref());
        }
        self.export.poll(&ctx);
        self.run_development_export(&ctx);
        if field_before.0 != self.transport.render_key().0 {
            self.message="Transport worker finished. Inspect execution status and recorded numerical results; scientific qualification NOT_EVALUATED.".into();
        }
        let before = (
            self.variant,
            self.cutaway,
            self.selected.clone(),
            self.hidden.clone(),
            year_before_frame,
        );
        self.keyboard(&ctx);
        let export_blocked = self.export_blocked();
        let mut export_clicked = false;
        let bar = egui::Panel::top("menu").show(ui, |ui| {
            let compact = ui.available_width() < 900.0;
            ui.horizontal(|ui| {
                ui.strong("FARIS");
                if !compact {
                    ui.weak("Avila Labs");
                }
                ui.separator();
                self.file_menu(ui);
                ui.separator();
                if !compact {
                    self.step_bar(ui);
                    ui.separator();
                }
                interface_size_menu(ui);
                ui.toggle_value(&mut self.maintenance.open, "Maintenance")
                    .on_hover_text("Open a maintenance result: fixed and computed replacement outages per design");
                if browser::IS_BROWSER {
                    self.browser_menu(ui);
                } else {
                    let check_label = if self.design_check.is_running() {
                        "Design check (running)"
                    } else {
                        "Design check"
                    };
                    ui.toggle_value(&mut self.design_check.open, check_label)
                        .on_hover_text("Write a draft design file from your STEP file, and run the nine import checks on a design file");
                    let flux_label = if self.design_flux.is_running() {
                        "Design flux (running)"
                    } else {
                        "Design flux"
                    };
                    ui.toggle_value(&mut self.design_flux.open, flux_label)
                        .on_hover_text("Run the neutron flux of a checked design in its magnets, compared with its fluence limit, and watch it");
                }
                let replay = ui.button("Tour").on_hover_text("Replay the guided tour");
                self.tour.anchor("tour-button", replay.rect);
                if replay.clicked() {
                    self.start_tour(0);
                }
                if !compact {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(suite) = &mut self.suite {
                            suite.controls(ui);
                        }
                        self.study.header(
                            ui,
                            &self.manifest,
                            &self.manifest.variants[self.variant].id,
                            &mut |ui| export_clicked |= self.export.button(ui, export_blocked),
                        );
                    });
                }
            });
            if compact {
                ui.horizontal_wrapped(|ui| self.step_bar(ui));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(suite) = &mut self.suite {
                        suite.controls(ui);
                    }
                    self.study.header(
                        ui,
                        &self.manifest,
                        &self.manifest.variants[self.variant].id,
                        &mut |ui| export_clicked |= self.export.button(ui, export_blocked),
                    );
                });
            }
        });
        let bar_bottom = bar.response.rect.bottom();
        self.maintenance.window(&ctx);
        self.design_check.window(&ctx);
        if let Some((design, report)) = self.design_check.take_flux_request() {
            self.design_flux.prefill(Some(&design), Some(&report));
            self.design_flux.open = true;
        }
        self.design_flux.window(&ctx);
        if self.sign_in_prompt
            && !self.tour.active
            && !self.notice.open
            && let Some(suite) = &mut self.suite
        {
            #[cfg(not(target_arch = "wasm32"))]
            {
                suite.show(&ctx, bar_bottom);
                if let Some(notice) = suite.take_notice() {
                    self.message = notice;
                }
            }
            #[cfg(target_arch = "wasm32")]
            suite.prompt(&ctx, bar_bottom);
        }
        match self.notice.show(&ctx) {
            browser::Outcome::StartTour => self.start_tour(0),
            browser::Outcome::Closed => self.write_tour_marker(),
            browser::Outcome::None => {}
        }
        if export_clicked {
            self.begin_export(&ctx);
        }
        // LEG-040: every result view sits above this line, whichever step is open.
        egui::Panel::bottom("screening").show(ui, |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(faris_model::RESEARCH_SCREENING_STATEMENT)
                        .small()
                        .weak(),
                )
                .truncate(),
            );
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.transport.has_results() {
                    let cold = badge::badge(
                        ui,
                        badge::Kind::Conditional,
                        "cold-data surrogate · NOT_EVALUATED",
                        "Checked transport records loaded. Cold-data surrogate; scientific qualification NOT_EVALUATED.",
                    );
                    self.tour.anchor("status-badge", cold.rect);
                }
                self.export.status_ui(ui);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(&self.message).small()).truncate())
                        .on_hover_text(&self.message);
                });
            });
        });
        let workspace_width = ui.available_width();
        if self.step == Step::Compare {
            let compare_max_height = (ui.available_height() * 0.75).max(120.0);
            let compare = egui::Panel::bottom("compare")
                .resizable(true)
                .default_size((ui.available_height() * 0.55).min(compare_max_height))
                .size_range(120.0..=compare_max_height)
                .show(ui, |ui| self.compare_view(ui));
            self.tour.anchor("compare-view", compare.response.rect);
        } else {
            let timeline_max_height = (ui.available_height() * 0.55).clamp(85.0, 640.0);
            let show_history = self.history.assumptions.is_some();
            egui::Panel::bottom("timeline")
                .resizable(true)
                .default_size(
                    (if show_history { 330.0_f32 } else { 110.0 }).min(timeline_max_height),
                )
                .size_range(85.0..=timeline_max_height)
                .show(ui, |ui| {
                    let content_width = ui.available_width();
                    egui::ScrollArea::both()
                        .id_salt("timeline-content")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(content_width);
                            if show_history {
                                let plot = self.history.timeline(
                                    ui,
                                    &self.manifest.source_sha256,
                                    &self.manifest.variants[self.variant].id,
                                    &mut self.year,
                                    self.manifest.horizon_years,
                                );
                                if let Some(plot) = plot {
                                    self.tour.anchor_in(ui, "timeline-plot", plot);
                                }
                                return;
                            }
                            ui.horizontal(|ui| {
                                ui.strong("Operating history");
                                ui.separator();
                                ui.label(format!("Year {:.1}", self.year));
                                ui.weak("Timeline preview · ageing model pending");
                            });
                            ui.add(
                                egui::Slider::new(
                                    &mut self.year,
                                    0.0..=self.manifest.horizon_years,
                                )
                                .text("years")
                                .show_value(false),
                            );
                            ui.horizontal(|ui| {
                                ui.label("Tritium inventory  —");
                                ui.separator();
                                ui.label("Net electricity  —");
                                ui.separator();
                                ui.label("Replacement events  —");
                            });
                        });
                });
        }
        let left_max_width = (workspace_width * 0.28).clamp(150.0, 380.0);
        egui::Panel::left("scenario")
            .resizable(true)
            .default_size(320.0_f32.min(left_max_width))
            .size_range(140.0..=left_max_width)
            .show(ui, |ui| {
                let content_width = ui.available_width();
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(content_width);
                        ui.heading(format!("{} · {}", self.step.number(), self.step.name()));
                        ui.add_space(8.0);
                        if let Some(failure) =
                            self.package.as_ref().and_then(|p| p.failure.as_ref())
                        {
                            package_notice(ui, failure);
                        }
                        match self.step {
                            Step::Design => self.design_step(ui),
                            Step::Simulate => self.simulate_step(ui),
                            Step::Operate => self.operate_step(ui),
                            Step::Compare => self.compare_step(ui),
                            Step::Evidence => self.evidence_step(ui),
                        }
                    });
            });
        let right_max_width = (workspace_width * 0.32).clamp(180.0, 400.0);
        egui::Panel::right("properties")
            .resizable(true)
            .default_size(285.0_f32.min(right_max_width))
            .size_range(160.0..=right_max_width)
            .show(ui, |ui| {
                let content_width = ui.available_width();
                egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_width(content_width);
                ui.heading("Outliner");
                ui.add_space(8.0);
                let variant = &self.manifest.variants[self.variant];
                for component in &variant.components {
                    ui.horizontal(|ui| {
                        let mut visible = !self.hidden.contains(&component.id);
                        if ui
                            .checkbox(&mut visible, "")
                            .on_hover_text("Show component")
                            .changed()
                        {
                            if visible {
                                self.hidden.remove(&component.id);
                            } else {
                                self.hidden.insert(component.id.clone());
                            }
                        }
                        ui.selectable_value(
                            &mut self.selected,
                            component.id.clone(),
                            &component.label,
                        );
                    });
                }
                ui.add_space(18.0);
                ui.separator();
                ui.heading("Properties");
                ui.add_space(8.0);
                if let Some(component) = variant
                    .components
                    .iter()
                    .find(|component| component.id == self.selected)
                {
                    ui.strong(&component.label);
                    ui.weak(&component.id);
                    ui.add_space(8.0);
                    egui::ScrollArea::horizontal().id_salt("component-properties-scroll").show(ui, |ui| {
                    egui::Grid::new("component-properties").show(ui, |ui| {
                        for (label, value) in [
                            ("Thickness", format!("{:.3} m", component.thickness_m)),
                            ("Inner radius", format!("{:.3} m", component.inner_minor_radius_m)),
                            ("Outer radius", format!("{:.3} m", component.outer_minor_radius_m)),
                            ("Unperforated reference volume", format!("{:.2} m³", component.full_torus_volume_m3)),
                        ] {
                            ui.label(label);
                            ui.label(value);
                            ui.end_row();
                        }
                    });
                    });
                    ui.add_space(12.0);
                    ui.label(format!("Material: {}", component.material_id));
                    if let Some(case)=self.transport.case(&variant.id)
                        && let Some(material)=case.materials.iter().find(|m|m.id==component.material_id) {
                        match &material.recipe {
                            faris_model::physics::MaterialRecipe::NuclideMixture{density_kg_m3,nuclear_data_temperature_k,..}=>{
                                ui.horizontal_wrapped(|ui|{
                                    ui.label(format!("Density: {density_kg_m3:.1} kg/m³"));
                                    badge::badge(ui,badge::Kind::Conditional,"cold reference",&format!("Data selection: {nuclear_data_temperature_k:.1} K · cold reference"));
                                });
                            }
                            faris_model::physics::MaterialRecipe::Void{..}=>{ui.small("Explicit geometric void.");}
                        }
                    } else {ui.weak("Composition and density are pending.");}
                    ui.add_space(12.0);
                    ui.separator();
                    if let Some(response)=self.transport.response(&variant.id,&format!("{}-flux",component.id)) {
                        ui.weak("Reference neutron flux · neutrons/m²/s");
                        ui.horizontal_wrapped(|ui|{
                            ui.label(egui::RichText::new(format!("{:.3e}",response.mean)).size(18.0).strong());
                            badge::badge(ui,badge::Kind::Calculated,&format!("± {:.2e} SE",response.standard_error),&format!("Standard error: {:.2e} neutrons/m²/s\nComponent volume average. Sampling uncertainty only.",response.standard_error));
                            badge::badge(ui,badge::Kind::Calculated,&format!("volume {:.4} m³",response.volume_m3),&format!("Scored physical volume: {:.6} m³ · volume SE {:.2e} m³", response.volume_m3, response.volume_standard_error_m3));
                        });
                        if response.mean == 0.0 && response.standard_error == 0.0 {badge::badge(ui,badge::Kind::Partial,"no sampled tracks","No sampled tracks. This does not establish zero flux or an upper bound.");}
                        else if response.mean > 0.0 && response.standard_error / response.mean > 0.3 {let text=format!("Weak sampling: {:.1}% relative standard error.",100.0*response.standard_error/response.mean);badge::badge(ui,badge::Kind::Partial,&format!("weak sampling · {:.0}% RSE",100.0*response.standard_error/response.mean),&text);}
                    } else {ui.label("Mean neutron flux  —");}
                    ui.add_space(8.0);
                    if let Some(heating) = self.transport.response(&variant.id, &format!("heating-total-{}", component.id)) {
                        ui.weak("Reference nuclear heating · MW");
                        ui.horizontal_wrapped(|ui|{
                            ui.label(egui::RichText::new(format!("{:.3}",heating.integrated_mean / 1e6)).size(18.0).strong());
                            badge::badge(ui,badge::Kind::Calculated,&format!("± {:.3} MW SE",heating.integrated_standard_error / 1e6),&format!("Sampling SE: {:.3} MW · coupled neutron/photon",heating.integrated_standard_error / 1e6));
                            badge::badge(ui,badge::Kind::Conditional,"energy closure not evaluated","Deposition includes material reaction energy and can exceed D–T source power. Full physical energy closure is not evaluated; heat recovery is an authored assumption.");
                        });
                    } else { ui.label("Nuclear heating  —"); }
                    self.history.inspector(ui,&self.manifest.source_sha256,&variant.id,&component.id,self.year*faris_engine::history::JULIAN_YEAR_SECONDS);
                    self.transport.spectra(ui,&variant.id,&component.id);
                    ui.add_space(10.0);
                    ui.weak(match self.transport.view {transport_panel::FieldView::Materials=>"Display colors identify components.",transport_panel::FieldView::NuclearHeating=>"Directly tallied total nuclear heating; component averages, not temperatures.",transport_panel::FieldView::ComponentFluence=>"Calculated component-average neutron fluence follows the timeline, including local resets on replacement.",_=>"Calculated flux uses a fixed scale across arrangements; inspect sampling precision before comparing."});
                }
                });
            });
        let mut viewport_points = [0.0; 2];
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).fill(egui::Color32::from_rgb(47, 50, 57)))
            .show(ui, |ui| {
                let content_width = ui.available_width();
                let controls_max_height = (ui.available_height() * 0.4).max(42.0);
                egui::ScrollArea::both()
                    .id_salt("viewport-controls")
                    .max_height(controls_max_height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.set_width(content_width);
                ui.horizontal_wrapped(|ui| {
                    ui.strong("3D viewport");
                    ui.separator();
                    ui.checkbox(&mut self.cutaway, "Cutaway");
                    if ui.button("Frame scene").clicked() {
                        self.frame_scene();
                    }
                    let help = VIEWPORT_HELP;
                    if ui.available_width() > 450.0 {
                        ui.weak(help);
                    } else {
                        ui.weak("Navigation help").on_hover_text(help);
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    let has_history=self.history.result(&self.manifest.source_sha256,&self.manifest.variants[self.variant].id).is_some();
                    self.transport.viewport_controls(ui,&self.manifest.variants[self.variant].id,has_history);
                    if self.transport.view == transport_panel::FieldView::ComponentFluence && self.history.is_stale() {
                        ui.colored_label(egui::Color32::YELLOW, "Earlier history inputs");
                    }
                    if field_before.1 != self.transport.view && self.transport.view == transport_panel::FieldView::FluxSlice
                        && let Some(record) = self.transport.record(&self.manifest.variants[self.variant].id) {
                        self.camera.frame_bounds(record.mesh.lower_left_m.map(|x| x as f32), record.mesh.upper_right_m.map(|x| x as f32));
                    }
                    if self.transport.view!=transport_panel::FieldView::Materials {ui.weak(match self.transport.view {transport_panel::FieldView::FluxSlice=>"Sampling uncertainty only · click a spatial bin to inspect",transport_panel::FieldView::ComponentFluence=>"Conditional history · component mean · uncertainty not propagated",_=>"Sampling uncertainty only · component volume averages"});}
                });
                self.transport.field_legend(ui);
                    });
                let (rect, response) =
                    ui.allocate_exact_size(ui.available_size().max(egui::vec2(1.0, 1.0)), egui::Sense::click_and_drag());
                viewport_points = [rect.width(), rect.height()];
                self.viewport_rect = Some(rect);
                self.tour.anchor("viewport", rect);
                self.viewport_id = Some(response.id);
                if response.dragged() {
                    let delta = ctx.input(|input| input.pointer.delta());
                    let pan = ctx.input(|input| input.modifiers.shift);
                    self.camera.drag(delta, pan);
                }
                response.widget_info(|| egui::WidgetInfo::labeled(
                    egui::WidgetType::Other,
                    true,
                    "3D viewport. Arrow keys orbit, Shift and arrows pan, plus and minus zoom, 0 resets, Enter reads out the centre.",
                ));
                let viewport_focused = response.has_focus();
                if viewport_focused {
                    commands::lock_arrows(&ctx, response.id);
                    self.viewport_keys(&ctx, rect);
                }
                if response.hovered() {
                    let scroll = ctx.input(|input| input.smooth_scroll_delta.y);
                    self.camera.distance =
                        (self.camera.distance * (-scroll * 0.002).exp()).clamp(0.25, 100.0);
                }
                if response.clicked()
                    && let Some(point) = response.interact_pointer_pos()
                {
                    self.pick(rect, point);
                }
                if response.double_clicked()
                    && let Some(point) = response.interact_pointer_pos()
                {
                    self.frame_component(rect, point);
                }
                if (before
                    != (
                        self.variant,
                        self.cutaway,
                        self.selected.clone(),
                        self.hidden.clone(),
                        if self.transport.view==transport_panel::FieldView::ComponentFluence{self.year.to_bits()}else{before.4},
                    )
                    || field_before != self.transport.render_key()
                    || (history_before != self.history.revision() && self.transport.view == transport_panel::FieldView::ComponentFluence))
                    && let Err(error) = self.rebuild()
                {
                    self.message = error.to_string();
                }
                let flat_color = self.transport.view != transport_panel::FieldView::Materials;
                let slice_view = self.transport.view == transport_panel::FieldView::FluxSlice;
                let components: Arc<[viewport::ComponentDraw]> = self.meshes.iter().map(|mesh| viewport::ComponentDraw {
                    vertices: mesh.vertices.clone(),
                    color: mesh.color,
                }).collect::<Vec<_>>().into();
                let shown_camera = viewport::eased_camera(&ctx, self.camera);
                let hovered = if slice_view { None } else { viewport::hover_component(&ctx, &response, rect, shown_camera, &components) };
                let scene = viewport::Scene {
                    slice: self.vertices.clone(),
                    components,
                    revision: self.revision,
                    flat_color,
                    slice_view,
                    port: self.manifest.penetration.as_ref().map(|faris_model::Penetration::OutboardRectangularPrism { bounds_m, .. }| viewport::PortBox {
                        minimum: bounds_m.minimum_xyz_m.map(|x| x as f32),
                        maximum: bounds_m.maximum_xyz_m.map(|x| x as f32),
                    }),
                    hover: hovered,
                };
                ui.painter().add(viewport::paint(rect, shown_camera, &scene));
                viewport::paint_labels(ui.painter(), rect, shown_camera, &scene);
                if viewport_focused {
                    // Focus ring and the keyboard cursor (the view centre the readout names).
                    let color = ui.visuals().selection.stroke.color;
                    ui.painter().rect_stroke(rect, 0.0, egui::Stroke::new(2.0, color), egui::StrokeKind::Inside);
                    let centre = rect.center();
                    for (a, b) in [(egui::vec2(-8.0, 0.0), egui::vec2(8.0, 0.0)), (egui::vec2(0.0, -8.0), egui::vec2(0.0, 8.0))] {
                        ui.painter().line_segment([centre + a, centre + b], egui::Stroke::new(1.5, color));
                    }
                }
                let hovered_label = hovered.and_then(|index| {
                    let id = &self.meshes.get(index)?.id;
                    self.manifest.variants[self.variant].components.iter().find(|c| &c.id == id).map(|c| c.label.clone())
                });
                let caption = match self.transport.view {
                        transport_panel::FieldView::Materials => {
                            "X red · Y green · Z blue · metre grid · material identities"
                        }
                        transport_panel::FieldView::ComponentFlux => {
                            "Reference-power component mean flux · neutrons/m²/s · stationary cold model"
                        }
                        transport_panel::FieldView::FluxSlice => {
                            "Reference-power bin mean flux · neutrons/m²/s · full voxel volumes, including any material/void mixture"
                        }
                        transport_panel::FieldView::NuclearHeating => "Reference-power nuclear heat deposition · W/m³ · coupled transport",
                        transport_panel::FieldView::ComponentFluence => "Snapshot accumulated neutron fluence · neutrons/m² · component mean",
                    };
                let galley = ui.painter().layout(
                    caption.to_owned(),
                    egui::FontId::monospace(12.0),
                    egui::Color32::from_gray(180),
                    (rect.width() - 32.0).max(1.0),
                );
                if rect.width() >= 200.0 && rect.height() >= 200.0 && galley.size().y + 32.0 <= rect.height() {
                    let caption_position = rect.left_bottom() + egui::vec2(16.0, -16.0 - galley.size().y);
                    ui.painter().galley(caption_position, galley, egui::Color32::from_gray(180));
                }
                response.on_hover_ui(|ui| {
                    if let Some(label) = &hovered_label {
                        ui.strong(label);
                    }
                    ui.weak(caption);
                });
            });
        let recording_settled = self.recorder.is_some() && self.recording_settled();
        if let Some(check) = &mut self.interface_check {
            let variant = &self.manifest.variants[self.variant].id;
            let snapshot = self.history.snapshot(
                &self.manifest.source_sha256,
                variant,
                self.year * faris_engine::history::JULIAN_YEAR_SECONDS,
            );
            let state = serde_json::json!({
                "scenario_id":self.manifest.scenario_id,
                "scenario_sha256":self.manifest.source_sha256,
                "window_points":ctx.input(|i|{let size=i.content_rect().size();[size.x,size.y]}),
                "pixels_per_point":ctx.pixels_per_point(),
                "interface_scale":ctx.zoom_factor(),
                "native_pixels_per_point":ctx.native_pixels_per_point(),
                "viewport_points":viewport_points,
                "variant_id":variant, "field_view":format!("{:?}",self.transport.view),
                "selected_component":self.selected, "hidden_components":self.hidden,
                "camera":{"yaw":self.camera.yaw,"pitch":self.camera.pitch,"distance":self.camera.distance,"target":self.camera.target},
                "step":self.step.name(),
                "tour":{"active":self.tour.active,"stop":self.tour.stop + 1},
                "year":self.year, "history_pending":self.history.is_pending(),
                "history_stale":self.history.is_stale(), "history_snapshot":snapshot,
                "history_controls":self.history.interface_status(&self.manifest.source_sha256, variant),
                "history_loaded":snapshot.is_some(), "source_on":snapshot.map(|s|s.operating),
                "core":self.study.interface_status(&self.manifest.source_sha256, variant),
                "transport":self.transport.interface_status(variant),
                "cutaway":self.cutaway, "scene_revision":self.revision, "message":self.message,
            });
            // While recording, the plan waits for what the recorder waits for, so
            // its actions are not over before the first frame is taken.
            let ready = if self.recorder.is_some() {
                recording_settled
            } else {
                self.frames >= 8
                    && !self.history.is_pending()
                    && !self.study.archive.is_loading()
                    && !self.file.is_busy()
            };
            if let Err(error) = check.observe(state, ready) {
                eprintln!("FARIS interface check report failed: {error}");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            if check.finished() && self.capture.is_none() && self.recorder.is_none() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        self.opening_overlay(&ctx);
        if self.export.development_requested()
            && self.export.development_settled()
            && self.capture.is_none()
            && self.interface_check.is_none()
            && self.benchmark.is_none()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        self.tour.show(&ctx);
        self.capture_frame(&ctx);
        self.record_frame(&ctx);
    }
}
