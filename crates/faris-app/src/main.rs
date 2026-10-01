mod camera;
mod study_panel;
mod transport_panel;
mod viewport;

use camera::{Camera, triangle_hit};
use clap::Parser;
use eframe::egui;
use faris_engine::{
    DemoManifest, build_manifest,
    mesh::{CUTAWAY_SWEEP, MeshVertex, torus_shell},
};
use faris_model::LoadedScenario;
use std::{collections::BTreeSet, f32::consts::TAU, path::PathBuf, sync::Arc, time::Instant};

#[derive(Parser)]
#[command(about = "FARIS native desktop workspace", version)]
struct Arguments {
    /// Load a scenario; otherwise use the bundled geometry demo.
    #[arg(long)]
    scenario: Option<PathBuf>,
    /// Capture this application's window to PNG and exit (development check).
    #[arg(long)]
    capture: Option<PathBuf>,
    /// External Avila Core executable used by Compile study.
    #[arg(long)]
    core: Option<PathBuf>,
    /// Directory for generated study records.
    #[arg(long, default_value = "runs")]
    runs_directory: PathBuf,
    /// Physics input for an arrangement; repeat to provide both arrangements.
    #[arg(long)]
    physics: Vec<PathBuf>,
    /// Load a verified local run.json; repeat to compare arrangements.
    #[arg(long)]
    run: Vec<PathBuf>,
    #[arg(long)]
    python: Option<PathBuf>,
    #[arg(long)]
    openmc: Option<PathBuf>,
    #[arg(long)]
    audit: Option<PathBuf>,
    #[arg(long)]
    cross_sections: Option<PathBuf>,
    /// Initial coloring for identified results loaded with --run.
    #[arg(long,value_enum,default_value_t=transport_panel::FieldView::Materials)]
    field_view: transport_panel::FieldView,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Arguments::parse();
    let loaded = if let Some(path) = args.scenario {
        LoadedScenario::load(&path)?
    } else {
        LoadedScenario::from_bytes(include_bytes!(
            "../../../scenarios/arc-inspired/scenario.json"
        ))?
    };
    let manifest = build_manifest(&loaded)?;
    let mut transport = transport_panel::TransportPanel::new(
        loaded,
        transport_panel::TransportConfiguration {
            python: args.python,
            openmc: args.openmc,
            audit: args.audit,
            cross_sections: args.cross_sections,
            physics: args.physics,
            runs: args.run,
            runs_directory: args.runs_directory.clone(),
        },
    )
    .map_err(std::io::Error::other)?;
    transport.view = args.field_view;
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        depth_buffer: 32,
        multisampling: 0,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([980.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "FARIS · Avila Labs",
        options,
        Box::new(move |cc| {
            let state = cc
                .wgpu_render_state
                .as_ref()
                .ok_or("wgpu is required for the 3D viewport")?;
            viewport::initialize(state);
            let mut visuals = egui::Visuals::dark();
            visuals.panel_fill = egui::Color32::from_rgb(37, 39, 44);
            visuals.selection.bg_fill = egui::Color32::from_rgb(67, 78, 125);
            cc.egui_ctx.set_visuals(visuals);
            Ok(Box::new(FarisApp::new(
                manifest,
                args.capture,
                args.core,
                args.runs_directory,
                transport,
            )?))
        }),
    )?;
    Ok(())
}

struct ComponentMesh {
    id: String,
    vertices: Vec<MeshVertex>,
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
    frames: usize,
    started: Instant,
    study: study_panel::StudyPanel,
    transport: transport_panel::TransportPanel,
}

impl FarisApp {
    fn new(
        manifest: DemoManifest,
        capture: Option<PathBuf>,
        core: Option<PathBuf>,
        runs_directory: PathBuf,
        transport: transport_panel::TransportPanel,
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
            frames: 0,
            started: Instant::now(),
            study: study_panel::StudyPanel::new(core, runs_directory),
            transport,
        };
        if app.transport.has_results() {
            app.message="Checked transport records loaded. Cold-data surrogate; scientific qualification NOT_EVALUATED.".into();
        }
        if app.transport.view == transport_panel::FieldView::FluxSlice {
            app.camera.distance = 20.0;
        }
        app.rebuild()?;
        Ok(app)
    }

    fn rebuild(&mut self) -> Result<(), faris_engine::mesh::MeshError> {
        self.meshes.clear();
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
            self.vertices = vertices.into();
            self.revision += 1;
            return Ok(());
        }
        let sweep = if self.cutaway { CUTAWAY_SWEEP } else { TAU };
        let mut vertices = Vec::new();
        for component in &self.manifest.variants[self.variant].components {
            if component.material_id == "void" || self.hidden.contains(&component.id) {
                continue;
            }
            let mesh = torus_shell(
                self.manifest.major_radius_m as f32,
                component.inner_minor_radius_m as f32,
                component.outer_minor_radius_m as f32,
                sweep,
            )?;
            let mut color = [0.0; 3];
            for (channel, value) in color.iter_mut().enumerate() {
                *value = u8::from_str_radix(&component.color[1 + channel * 2..3 + channel * 2], 16)
                    .expect("validated hexadecimal color") as f32
                    / 255.0;
                if component.id == self.selected {
                    *value = (*value * 1.1 + 0.12).min(1.0);
                }
            }
            if self.transport.view == transport_panel::FieldView::ComponentFlux
                && let Some(response) = self.transport.response(
                    &self.manifest.variants[self.variant].id,
                    &format!("{}-flux", component.id),
                )
            {
                color = transport_panel::flux_color(response.mean, response.standard_error);
            }
            vertices.extend(mesh.iter().map(|vertex| viewport::Vertex {
                position: vertex.position,
                normal: vertex.normal,
                color,
            }));
            self.meshes.push(ComponentMesh {
                id: component.id.clone(),
                vertices: mesh,
            });
        }
        self.vertices = vertices.into();
        self.revision += 1;
        Ok(())
    }

    fn pick(&mut self, rect: egui::Rect, point: egui::Pos2) {
        let (origin, direction) = self.camera.ray(rect, point);
        if self.transport.view == transport_panel::FieldView::FluxSlice
            && let Some(record) = self
                .transport
                .record(&self.manifest.variants[self.variant].id)
        {
            let mesh = &record.mesh;
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
                    if let Some(response) = self
                        .transport
                        .response(&record.variant_id, &format!("mesh-flux-{bin}"))
                    {
                        self.message = format!(
                            "Bin {bin}: {:.3e} ± {:.2e} neutrons/m²/s (Monte Carlo SE); full-bin average. Scientific qualification NOT_EVALUATED.",
                            response.mean, response.standard_error
                        );
                    }
                }
            }
            return;
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
        if let Some(selected) = selected {
            self.selected = selected;
        }
    }

    fn capture_frame(&mut self, ctx: &egui::Context) {
        let Some(path) = &self.capture else {
            return;
        };
        self.frames += 1;
        if self.frames == 8 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        let screenshot = ctx.input(|input| {
            input.events.iter().find_map(|event| {
                if let egui::Event::Screenshot { image, .. } = event {
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
        } else if self.started.elapsed().as_secs() > 20 {
            eprintln!("FARIS capture timed out");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            ctx.request_repaint();
        }
    }
}

impl eframe::App for FarisApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.study.poll(&ctx);
        let field_before = self.transport.render_key();
        self.transport.poll(&ctx);
        if field_before.0 != self.transport.render_key().0 {
            self.message="Transport worker finished. Inspect execution status and recorded numerical results; scientific qualification NOT_EVALUATED.".into();
        }
        let before = (
            self.variant,
            self.cutaway,
            self.selected.clone(),
            self.hidden.clone(),
        );
        egui::Panel::top("menu").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong("FARIS");
                ui.weak("Avila Labs");
                ui.separator();
                ui.label("Scene");
                ui.separator();
                ui.label("Research workspace");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.study
                        .header(ui, &self.manifest, &self.manifest.variants[self.variant].id);
                });
            });
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.small(&self.message);
        });
        egui::Panel::bottom("timeline")
            .resizable(true)
            .default_size(110.0)
            .size_range(85.0..=240.0)
            .show(ui, |ui| {
                if self.transport.has_results() {
                    self.transport.comparison(ui);
                    return;
                }
                ui.horizontal(|ui| {
                    ui.strong("Operating history");
                    ui.separator();
                    ui.label(format!("Year {:.1}", self.year));
                    ui.weak("Timeline preview · ageing model pending");
                });
                ui.add(
                    egui::Slider::new(&mut self.year, 0.0..=self.manifest.horizon_years)
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
        egui::Panel::left("scenario")
            .resizable(true)
            .default_size(220.0)
            .size_range(175.0..=320.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading("Scenario");
                    ui.add_space(8.0);
                    ui.label(&self.manifest.title);
                    ui.weak("ARC-inspired · idealized geometry");
                    ui.add_space(12.0);
                    ui.strong("Arrangement");
                    for (index, variant) in self.manifest.variants.iter().enumerate() {
                        ui.selectable_value(&mut self.variant, index, &variant.label);
                    }
                    ui.add_space(16.0);
                    ui.separator();
                    ui.strong("Plant inputs");
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
                    ui.add_space(16.0);
                    ui.separator();
                    self.study.controls(
                        ui,
                        &self.manifest.variants[self.variant].id,
                        self.transport
                            .readiness(&self.manifest.variants[self.variant].id),
                    );
                    self.transport
                        .controls(ui, &self.manifest.variants[self.variant].id);
                    ui.add_space(16.0);
                    ui.collapsing("Sources and assumptions", |ui| {
                        for reference in &self.manifest.references {
                            ui.hyperlink_to(&reference.title, &reference.url);
                        }
                        for assumption in &self.manifest.assumptions {
                            ui.small(assumption);
                            ui.add_space(5.0);
                        }
                    });
                });
            });
        egui::Panel::right("properties")
            .resizable(true)
            .default_size(285.0)
            .size_range(230.0..=400.0)
            .show(ui, |ui| {
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
                    egui::Grid::new("component-properties").show(ui, |ui| {
                        for (label, value) in [
                            ("Thickness", format!("{:.3} m", component.thickness_m)),
                            ("Inner radius", format!("{:.3} m", component.inner_minor_radius_m)),
                            ("Outer radius", format!("{:.3} m", component.outer_minor_radius_m)),
                            ("Full-torus volume", format!("{:.2} m³", component.full_torus_volume_m3)),
                        ] {
                            ui.label(label);
                            ui.label(value);
                            ui.end_row();
                        }
                    });
                    ui.add_space(12.0);
                    ui.label(format!("Material: {}", component.material_id));
                    if let Some(case)=self.transport.case(&variant.id)
                        && let Some(material)=case.materials.iter().find(|m|m.id==component.material_id) {
                        match &material.recipe {
                            faris_model::physics::MaterialRecipe::NuclideMixture{density_kg_m3,nuclear_data_temperature_k,..}=>{
                                ui.label(format!("Density: {density_kg_m3:.1} kg/m³"));
                                ui.small(format!("Data selection: {nuclear_data_temperature_k:.1} K · cold reference"));
                            }
                            faris_model::physics::MaterialRecipe::Void{..}=>{ui.small("Explicit geometric void.");}
                        }
                    } else {ui.weak("Composition and density are pending.");}
                    ui.add_space(12.0);
                    ui.separator();
                    if let Some(response)=self.transport.response(&variant.id,&format!("{}-flux",component.id)) {
                        ui.label(format!("Mean neutron flux: {:.3e} neutrons/m²/s",response.mean));
                        ui.small(format!("Standard error: {:.2e} neutrons/m²/s",response.standard_error));
                        ui.small("Component volume average. Sampling uncertainty only.");
                    } else {ui.label("Mean neutron flux  —");}
                    ui.label("Nuclear heating  —");
                    ui.label("Service limit  —");
                    ui.add_space(10.0);
                    ui.weak(match self.transport.view {transport_panel::FieldView::Materials=>"Display colors identify components.",_=>"Calculated flux: blue to red, fixed log scale 10¹⁰–10²⁰ neutrons/m²/s. Gray/desaturated bins have zero samples or >30% relative standard error."});
                }
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).fill(egui::Color32::from_rgb(47, 50, 57)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong("3D viewport");
                    ui.separator();
                    ui.checkbox(&mut self.cutaway, "Cutaway");
                    if ui.button("Frame scene").clicked() {
                        self.camera = Camera::default();
                        if self.transport.view==transport_panel::FieldView::FluxSlice {self.camera.distance=20.0;}
                    }
                    ui.weak("Drag to orbit · scroll to zoom · click to select");
                });
                ui.horizontal(|ui| {
                    self.transport.viewport_controls(ui,&self.manifest.variants[self.variant].id);
                    if self.transport.view!=transport_panel::FieldView::Materials {ui.weak(if self.transport.view==transport_panel::FieldView::FluxSlice {"Sampling uncertainty only · click a spatial bin to inspect"} else {"Sampling uncertainty only · component volume averages"});}
                });
                let (rect, response) =
                    ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
                if response.dragged() {
                    let delta = ctx.input(|input| input.pointer.delta());
                    self.camera.yaw -= delta.x * 0.008;
                    self.camera.pitch = (self.camera.pitch + delta.y * 0.008).clamp(-1.35, 1.35);
                }
                if response.hovered() {
                    let scroll = ctx.input(|input| input.smooth_scroll_delta.y);
                    self.camera.distance =
                        (self.camera.distance * (-scroll * 0.002).exp()).clamp(6.0, 40.0);
                }
                if response.clicked()
                    && let Some(point) = response.interact_pointer_pos()
                {
                    self.pick(rect, point);
                }
                if (before
                    != (
                        self.variant,
                        self.cutaway,
                        self.selected.clone(),
                        self.hidden.clone(),
                    )
                    || field_before != self.transport.render_key())
                    && let Err(error) = self.rebuild()
                {
                    self.message = error.to_string();
                }
                ui.painter().add(viewport::paint(
                    rect,
                    self.camera,
                    self.vertices.clone(),
                    self.revision,
                ));
                ui.painter().text(
                    rect.left_bottom() + egui::vec2(16.0, -16.0),
                    egui::Align2::LEFT_BOTTOM,
                    match self.transport.view {
                        transport_panel::FieldView::Materials => {
                            "Y ↑    Metres    Material identities"
                        }
                        transport_panel::FieldView::ComponentFlux => {
                            "Component-average neutron flux · neutrons/m²/s · unqualified model"
                        }
                        transport_panel::FieldView::FluxSlice => {
                            "Cartesian bin-average neutron flux · neutrons/m²/s · full voxel volumes"
                        }
                    },
                    egui::FontId::monospace(12.0),
                    egui::Color32::from_gray(180),
                );
            });
        self.capture_frame(&ctx);
    }
}
