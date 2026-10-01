mod camera;
mod history_panel;
mod study_panel;
mod transport_panel;
mod viewport;

use camera::{Camera, triangle_hit};
use clap::Parser;
use eframe::egui;
use faris_engine::{
    DemoManifest, build_manifest,
    mesh::{CUTAWAY_SWEEP, MeshVertex, torus_shell, torus_shell_with_prism_cut},
};
use faris_model::LoadedScenario;
use std::{
    collections::{BTreeMap, BTreeSet},
    f32::consts::TAU,
    path::PathBuf,
    sync::Arc,
    time::Instant,
};

#[derive(Parser)]
#[command(about = "FARIS native desktop workspace", version)]
struct Arguments {
    /// Load a scenario; otherwise use the bundled geometry demo.
    #[arg(long)]
    scenario: Option<PathBuf>,
    /// Capture this application's window to PNG and exit (development check).
    #[arg(long)]
    capture: Option<PathBuf>,
    /// Measure native frame throughput and exit. Includes startup separately.
    #[arg(long, value_parser = clap::value_parser!(f64))]
    benchmark_seconds: Option<f64>,
    /// Exercise camera orbit and calculated-history scrubbing during measurement.
    #[arg(long, requires = "benchmark_seconds")]
    benchmark_motion: bool,
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
    /// Portable identified recorded-transport bundle; repeat for both arrangements.
    #[arg(long)]
    bundle: Vec<PathBuf>,
    /// Matched feature-free scenario to compare against the penetration.
    #[arg(long)]
    control_scenario: Option<PathBuf>,
    #[arg(long)]
    control_physics: Vec<PathBuf>,
    #[arg(long)]
    control_run: Vec<PathBuf>,
    #[arg(long)]
    control_bundle: Vec<PathBuf>,
    /// Identified fuel, maintenance, service-limit and energy assumptions.
    #[arg(long)]
    assumptions: Option<PathBuf>,
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
    if args
        .benchmark_seconds
        .is_some_and(|s| !s.is_finite() || !(2.0..=120.0).contains(&s))
    {
        return Err("benchmark duration must be finite and in 2..=120 seconds".into());
    }
    let launch_started = Instant::now();
    let loaded = if let Some(path) = args.scenario {
        LoadedScenario::load(&path)?
    } else if let Some(path) = args.bundle.first() {
        scenario_from_bundle(path)?
    } else {
        LoadedScenario::from_bytes(include_bytes!(
            "../../../scenarios/arc-inspired/scenario.json"
        ))?
    };
    let manifest = build_manifest(&loaded)?;
    let control = if args.control_scenario.is_some() || !args.control_bundle.is_empty() {
        let scenario = if let Some(path) = &args.control_scenario {
            LoadedScenario::load(path)?
        } else {
            scenario_from_bundle(&args.control_bundle[0])?
        };
        let control_manifest = build_manifest(&scenario)?;
        let panel = transport_panel::TransportPanel::new(
            scenario,
            transport_panel::TransportConfiguration {
                python: args.python.clone(),
                openmc: args.openmc.clone(),
                audit: args.audit.clone(),
                cross_sections: args.cross_sections.clone(),
                physics: args.control_physics,
                runs: args.control_run,
                bundles: args.control_bundle,
                runs_directory: args.runs_directory.clone(),
            },
        )
        .map_err(std::io::Error::other)?;
        Some((control_manifest, panel))
    } else {
        None
    };
    let mut transport = transport_panel::TransportPanel::new(
        loaded,
        transport_panel::TransportConfiguration {
            python: args.python,
            openmc: args.openmc,
            audit: args.audit,
            cross_sections: args.cross_sections,
            physics: args.physics,
            runs: args.run,
            bundles: args.bundle,
            runs_directory: args.runs_directory.clone(),
        },
    )
    .map_err(std::io::Error::other)?;
    transport.view = args.field_view;
    let history =
        history_panel::HistoryPanel::new(args.assumptions).map_err(std::io::Error::other)?;
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
            eprintln!("FARIS graphics: {:?}", state.adapter.get_info());
            let mut visuals = egui::Visuals::dark();
            visuals.panel_fill = egui::Color32::from_rgb(37, 39, 44);
            visuals.selection.bg_fill = egui::Color32::from_rgb(67, 78, 125);
            cc.egui_ctx.set_visuals(visuals);
            let mut app = FarisApp::new(
                manifest,
                args.capture,
                args.core,
                args.runs_directory,
                transport,
                control,
                history,
            )?;
            app.started = launch_started;
            app.benchmark = args.benchmark_seconds.map(|duration| Benchmark {
                duration,
                motion: args.benchmark_motion,
                launch_started,
                ready_at: None,
                frame_times: Vec::new(),
            });
            Ok(Box::new(app))
        }),
    )?;
    Ok(())
}

fn scenario_from_bundle(
    path: &std::path::Path,
) -> Result<LoadedScenario, Box<dyn std::error::Error>> {
    let bundle: faris_engine::core_evidence::RecordedTransportBundle = serde_json::from_slice(
        &faris_engine::reactor::read_json_bytes(path)
            .map_err(|e| std::io::Error::other(e.to_string()))?,
    )?;
    bundle
        .validate()
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(LoadedScenario::from_bytes(
        bundle.files["scenario.json"].as_bytes(),
    )?)
}

struct ComponentMesh {
    id: String,
    vertices: Arc<[MeshVertex]>,
}

struct Benchmark {
    duration: f64,
    motion: bool,
    launch_started: Instant,
    ready_at: Option<Instant>,
    frame_times: Vec<Instant>,
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
    history: history_panel::HistoryPanel,
    show_history: bool,
    benchmark: Option<Benchmark>,
    geometry_cache: BTreeMap<String, Arc<[MeshVertex]>>,
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
        let show_history = history.assumptions.is_some();
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
            history,
            show_history,
            benchmark: None,
            geometry_cache: BTreeMap::new(),
        };
        if app.transport.has_results() {
            app.message="Checked transport records loaded. Cold-data surrogate; scientific qualification NOT_EVALUATED.".into();
        }
        app.study.selection.fuel_history = show_history;
        app.study.selection.electricity = show_history
            && app
                .transport
                .response(&app.manifest.variants[0].id, "heating-total-whole-model")
                .is_some();
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
            if self.transport.view == transport_panel::FieldView::ComponentFlux
                && let Some(response) = self.transport.response(
                    &self.manifest.variants[self.variant].id,
                    &format!("{}-flux", component.id),
                )
            {
                color = transport_panel::flux_color(response.mean, response.standard_error);
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
        if self.frames >= 8
            && !self.capture_requested
            && !self.history.is_pending()
            && self.started.elapsed().as_secs_f64() >= 1.0
        {
            self.capture_requested = true;
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
                if elapsed >= b.duration {
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
                        serde_json::json!({"schema_version":"faris-native-frame-benchmark/v0.1", "measurement":"egui frame throughput; not GPU presentation FPS", "first_measured_frame_seconds": ready_at.duration_since(b.launch_started).as_secs_f64(), "elapsed_seconds":elapsed,"interval_count":intervals.len(),"mean_frames_per_second":intervals.len() as f64/elapsed,"p95_frame_interval_ms":p95*1000.0,"orbit_and_scrub":b.motion,"viewport_points":[1440,900]})
                    );
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        self.study.poll(&ctx);
        self.history.poll(&ctx);
        let field_before = self.transport.render_key();
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
        self.history.update_inputs(&ctx, &history_inputs);
        if field_before.0 != self.transport.render_key().0 {
            self.message="Transport worker finished. Inspect execution status and recorded numerical results; scientific qualification NOT_EVALUATED.".into();
        }
        let before = (
            self.variant,
            self.cutaway,
            self.selected.clone(),
            self.hidden.clone(),
            self.year.to_bits(),
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
            .default_size(if self.show_history { 220.0 } else { 110.0 })
            .size_range(85.0..=360.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.show_history, true, "Operating history");
                    ui.selectable_value(&mut self.show_history, false, "Transport comparison");
                });
                if self.show_history {
                    self.history.timeline(
                        ui,
                        &self.manifest.source_sha256,
                        &self.manifest.variants[self.variant].id,
                        &mut self.year,
                        self.manifest.horizon_years,
                    );
                    return;
                }
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
                    if self.paired.is_some() {
                        let is_control=self.manifest.penetration.is_none();
                        ui.label(if is_control {"Feature-free control"} else {"Finite outboard penetration"});
                        if ui.button(if is_control {"Show penetration"} else {"Show matched control"}).clicked()
                            && let Some((manifest,panel))=&mut self.paired {
                            std::mem::swap(&mut self.manifest,manifest);
                            std::mem::swap(&mut self.transport,panel);
                            self.transport.view=panel.view;
                            self.message="Switched physical scenario. Recorded results retain their distinct scenario identities.".into();
                            self.rebuild().unwrap_or_else(|error|self.message=error.to_string());
                        }
                    }
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
                        &self.manifest.source_sha256,
                        self.transport
                            .readiness(&self.manifest.variants[self.variant].id),
                    );
                    self.transport
                        .controls(ui, &self.manifest.variants[self.variant].id);
                    self.history.controls(ui);
                    self.history.sensitivity_controls(ui,&self.manifest.source_sha256,&self.manifest.variants[self.variant].id);
                    self.study.evidence_controls(ui,&self.manifest,&self.manifest.variants[self.variant].id,self.transport.location(&self.manifest.variants[self.variant].id),self.history.assumptions.as_ref());
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
                egui::ScrollArea::vertical().show(ui, |ui| {
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
                            ("Unperforated reference volume", format!("{:.2} m³", component.full_torus_volume_m3)),
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
                        ui.small(format!("Scored physical volume: {:.6} m³ · volume SE {:.2e} m³", response.volume_m3, response.volume_standard_error_m3));
                    } else {ui.label("Mean neutron flux  —");}
                    if let Some(heating) = self.transport.response(&variant.id, &format!("heating-total-{}", component.id)) {
                        ui.label(format!("Nuclear heating: {:.3} MW", heating.integrated_mean / 1e6));
                        ui.small(format!("Sampling SE: {:.3} MW · coupled neutron/photon", heating.integrated_standard_error / 1e6));
                    } else { ui.label("Nuclear heating  —"); }
                    ui.label("Service limit  —");
                    self.history.inspector(ui,&self.manifest.source_sha256,&variant.id,&component.id,self.year*faris_engine::history::JULIAN_YEAR_SECONDS);
                    self.transport.spectra(ui,&variant.id,&component.id);
                    ui.add_space(10.0);
                    ui.weak(match self.transport.view {transport_panel::FieldView::Materials=>"Display colors identify components.",transport_panel::FieldView::NuclearHeating=>"Directly tallied total nuclear heating; component averages, not temperatures.",transport_panel::FieldView::ComponentFluence=>"Calculated component-average neutron fluence follows the timeline, including local resets on replacement.",_=>"Calculated flux uses a fixed scale across arrangements; inspect sampling precision before comparing."});
                }
                });
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
                    let has_history=self.history.result(&self.manifest.source_sha256,&self.manifest.variants[self.variant].id).is_some();
                    self.transport.viewport_controls(ui,&self.manifest.variants[self.variant].id,has_history);
                    if self.transport.view!=transport_panel::FieldView::Materials {ui.weak(match self.transport.view {transport_panel::FieldView::FluxSlice=>"Sampling uncertainty only · click a spatial bin to inspect",transport_panel::FieldView::ComponentFluence=>"Conditional history · component mean · uncertainty not propagated",_=>"Sampling uncertainty only · component volume averages"});}
                });
                self.transport.field_legend(ui);
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
                        if self.transport.view==transport_panel::FieldView::ComponentFluence{self.year.to_bits()}else{before.4},
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
                    self.transport.view != transport_panel::FieldView::Materials,
                ));
                ui.painter().text(
                    rect.left_bottom() + egui::vec2(16.0, -16.0),
                    egui::Align2::LEFT_BOTTOM,
                    match self.transport.view {
                        transport_panel::FieldView::Materials => {
                            "X red · Y green · Z blue · metre grid · material identities"
                        }
                        transport_panel::FieldView::ComponentFlux => {
                            "Reference-power component mean flux · neutrons/m²/s · stationary cold model"
                        }
                        transport_panel::FieldView::FluxSlice => {
                            "Reference-power bin mean flux · neutrons/m²/s · full voxel volumes"
                        }
                        transport_panel::FieldView::NuclearHeating => "Reference-power nuclear heat deposition · W/m³ · coupled transport",
                        transport_panel::FieldView::ComponentFluence => "Snapshot accumulated neutron fluence · neutrons/m² · component mean",
                    },
                    egui::FontId::monospace(12.0),
                    egui::Color32::from_gray(180),
                );
            });
        self.capture_frame(&ctx);
    }
}
