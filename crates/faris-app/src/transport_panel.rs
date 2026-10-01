use eframe::egui;
use faris_engine::{
    jobs::Cancellation,
    reactor::{
        ReactorJob, ReactorRun, SamplingPlan, load_physics_case, load_reactor_run, run_reactor,
    },
    transport::NormalizedTally,
};
use faris_model::{LoadedScenario, physics::PhysicsCase};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum FieldView {
    #[default]
    Materials,
    ComponentFlux,
    FluxSlice,
    NuclearHeating,
    ComponentFluence,
}

pub struct TransportConfiguration {
    pub python: Option<PathBuf>,
    pub openmc: Option<PathBuf>,
    pub audit: Option<PathBuf>,
    pub cross_sections: Option<PathBuf>,
    pub physics: Vec<PathBuf>,
    pub runs: Vec<PathBuf>,
    pub bundles: Vec<PathBuf>,
    pub runs_directory: PathBuf,
}

struct Pending {
    handle: Option<std::thread::JoinHandle<()>>,
    cancellation: Cancellation,
    receiver: Receiver<Result<ReactorRun, String>>,
    output: PathBuf,
}

pub struct TransportPanel {
    scenario: LoadedScenario,
    cases: BTreeMap<String, PhysicsCase>,
    records: BTreeMap<String, ReactorRun>,
    locations: BTreeMap<String, PathBuf>,
    _replay_directories: Vec<tempfile::TempDir>,
    python: String,
    openmc: String,
    audit: String,
    cross_sections: String,
    runs_directory: PathBuf,
    sampling: SamplingPlan,
    pending: Option<Pending>,
    error: Option<String>,
    revision: u64,
    pub view: FieldView,
    pub slice: usize,
}

fn path_text(path: Option<PathBuf>) -> String {
    path.map_or_else(String::new, |p| p.to_string_lossy().into_owned())
}

impl TransportPanel {
    pub fn new(scenario: LoadedScenario, config: TransportConfiguration) -> Result<Self, String> {
        let mut cases = BTreeMap::new();
        for path in config.physics {
            let case = load_physics_case(&path, &scenario).map_err(|e| e.to_string())?;
            if cases.insert(case.variant_id.clone(), case).is_some() {
                return Err("Only one physics input per arrangement can be selected.".into());
            }
        }
        let mut records = BTreeMap::new();
        let mut locations = BTreeMap::new();
        let mut replay_directories = Vec::new();
        for path in config.runs {
            let record = load_reactor_run(&path, &scenario).map_err(|e| e.to_string())?;
            locations.insert(record.variant_id.clone(), path);
            records.insert(record.variant_id.clone(), record);
        }
        for path in config.bundles {
            let bundle: faris_engine::core_evidence::RecordedTransportBundle =
                serde_json::from_slice(
                    &faris_engine::reactor::read_json_bytes(&path).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
            let directory = bundle.materialize().map_err(|e| e.to_string())?;
            let record = load_reactor_run(&directory.path().join("run.json"), &scenario)
                .map_err(|e| e.to_string())?;
            if records.contains_key(&record.variant_id) {
                return Err("Duplicate transport record for an arrangement.".into());
            }
            let input: serde_json::Value =
                serde_json::from_str(&bundle.files["input.json"]).map_err(|e| e.to_string())?;
            let case: PhysicsCase =
                serde_json::from_value(input["physics"].clone()).map_err(|e| e.to_string())?;
            case.validate_against(&scenario)
                .map_err(|e| e.to_string())?;
            cases.entry(case.variant_id.clone()).or_insert(case);
            locations.insert(record.variant_id.clone(), directory.path().join("run.json"));
            records.insert(record.variant_id.clone(), record);
            replay_directories.push(directory);
        }
        Ok(Self {
            scenario,
            cases,
            records,
            locations,
            _replay_directories: replay_directories,
            python: path_text(config.python),
            openmc: path_text(config.openmc),
            audit: path_text(config.audit),
            cross_sections: path_text(config.cross_sections),
            runs_directory: config.runs_directory,
            sampling: SamplingPlan::default(),
            pending: None,
            error: None,
            revision: 0,
            view: FieldView::default(),
            slice: 4,
        })
    }

    pub fn case(&self, variant: &str) -> Option<&PhysicsCase> {
        self.cases.get(variant)
    }
    pub fn record(&self, variant: &str) -> Option<&ReactorRun> {
        self.records.get(variant)
    }
    pub fn records(&self) -> &BTreeMap<String, ReactorRun> {
        &self.records
    }
    pub fn location(&self, variant: &str) -> Option<&std::path::Path> {
        self.locations.get(variant).map(PathBuf::as_path)
    }
    pub fn response(&self, variant: &str, id: &str) -> Option<&NormalizedTally> {
        self.record(variant)?
            .normalized
            .as_ref()?
            .results
            .iter()
            .find(|r| r.response_id == id)
    }
    pub fn render_key(&self) -> (u64, FieldView, usize) {
        (self.revision, self.view, self.slice)
    }

    pub fn has_results(&self) -> bool {
        self.records
            .values()
            .any(|record| record.normalized.is_some())
    }

    pub fn comparison(&self, ui: &mut egui::Ui) {
        ui.strong("Transport comparison · cold-data surrogate");
        egui::Grid::new("transport-comparison")
            .striped(true)
            .show(ui, |ui| {
                for heading in [
                    "Arrangement",
                    "Breeder H3 / source ± SE",
                    "Magnet mean flux / m² / s",
                    "Histories",
                ] {
                    ui.strong(heading);
                }
                ui.end_row();
                for variant in &self.scenario.scenario.variants {
                    ui.label(&variant.label);
                    let record = self.record(&variant.id);
                    if let Some(breeder) = self.response(&variant.id, "blanket-tritium") {
                        let rate = record
                            .and_then(|r| r.normalized.as_ref())
                            .expect("normalized record")
                            .source_neutron_rate_per_s;
                        ui.monospace(format!(
                            "{:.4} ± {:.4}",
                            breeder.integrated_mean / rate,
                            breeder.integrated_standard_error / rate
                        ));
                    } else {
                        ui.weak("Not calculated");
                    }
                    if let Some(flux) = self.response(&variant.id, "magnets-flux") {
                        ui.monospace(format!("{:.3e} ± {:.2e}", flux.mean, flux.standard_error));
                    } else {
                        ui.weak("Not calculated");
                    }
                    ui.label(record.map_or_else(
                        || "—".into(),
                        |r| {
                            (u64::from(r.sampling.batches)
                                * u64::from(r.sampling.particles_per_batch))
                            .to_string()
                        },
                    ));
                    ui.end_row();
                }
            });
        ui.small("Uncertainties are Monte Carlo standard errors. No engineering ranking or qualified bound is inferred.");
    }
    pub fn readiness(&self, variant: &str) -> &'static str {
        if self.cases.contains_key(variant)
            && [
                &self.python,
                &self.openmc,
                &self.audit,
                &self.cross_sections,
            ]
            .iter()
            .all(|p| !p.trim().is_empty())
        {
            "Transport configured; preflight on run"
        } else {
            "Transport inputs not configured"
        }
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.pending else {
            return;
        };
        let result = match pending.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                Err("Transport worker ended without a record.".into())
            }
        };
        let mut pending = self.pending.take().expect("pending worker exists");
        if let Some(handle) = pending.handle.take() {
            let _ = handle.join();
        }
        match result {
            Ok(record) => {
                if record.normalized.is_none() {
                    self.error = Some(record.import_error.clone().unwrap_or_else(|| {
                        format!(
                            "Transport did not produce accepted results; see {}",
                            pending.output.join("run.json").display()
                        )
                    }));
                }
                self.locations
                    .insert(record.variant_id.clone(), pending.output.join("run.json"));
                self.records.insert(record.variant_id.clone(), record);
                self.revision += 1;
            }
            Err(error) => self.error = Some(error),
        }
        ctx.request_repaint();
    }

    fn start(&mut self, ctx: egui::Context, variant: &str) {
        let Some(physics) = self.cases.get(variant).cloned() else {
            self.error =
                Some("Select a physics input for this arrangement using --physics.".into());
            return;
        };
        let scenario = self.scenario.clone();
        let python = PathBuf::from(&self.python);
        let openmc = PathBuf::from(&self.openmc);
        let audit = PathBuf::from(&self.audit);
        let cross_sections = PathBuf::from(&self.cross_sections);
        let sampling = self.sampling.clone();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let output = self
            .runs_directory
            .join(format!("transport-{variant}-{stamp}"));
        let worker_output = output.clone();
        let cancellation = Cancellation::default();
        let worker_cancellation = cancellation.clone();
        let (sender, receiver) = mpsc::channel();
        self.error = None;
        match std::thread::Builder::new()
            .name("faris-openmc-transport".into())
            .spawn(move || {
                let job = ReactorJob {
                    scenario: &scenario,
                    physics: &physics,
                    audit: &audit,
                    cross_sections: &cross_sections,
                    python: &python,
                    openmc: &openmc,
                    output: &worker_output,
                    sampling,
                    timeout: Duration::from_secs(600),
                    adapter: include_bytes!("../../../integrations/openmc/reactor_transport.py"),
                };
                let result =
                    run_reactor(&job, &worker_cancellation).map_err(|error| error.to_string());
                let _ = sender.send(result);
                ctx.request_repaint();
            }) {
            Ok(handle) => {
                self.pending = Some(Pending {
                    handle: Some(handle),
                    cancellation,
                    receiver,
                    output,
                })
            }
            Err(error) => self.error = Some(format!("Could not start transport worker: {error}")),
        }
    }

    pub fn controls(&mut self, ui: &mut egui::Ui, variant: &str) {
        ui.separator();
        ui.strong("Neutron transport");
        if let Some(case) = self.case(variant) {
            ui.small(format!("Case: {}", case.id));
        } else {
            ui.small("No physics input selected for this arrangement.");
        }
        ui.collapsing("Transport configuration", |ui| {
            for (label, path) in [
                ("OpenMC Python", &mut self.python),
                ("OpenMC executable", &mut self.openmc),
                ("Data audit JSON", &mut self.audit),
                ("cross_sections.xml", &mut self.cross_sections),
            ] {
                ui.label(label);
                ui.add(egui::TextEdit::singleline(path).desired_width(f32::INFINITY));
            }
            ui.label("Histories (100 batches)");
            ui.radio_value(
                &mut self.sampling.particles_per_batch,
                1_000,
                "100,000 · pilot",
            );
            ui.radio_value(&mut self.sampling.particles_per_batch, 10_000, "1,000,000");
            ui.small("One thread; 600 s limit. Sampling precision is measured after execution.");
        });
        if let Some(pending) = &self.pending {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Transport running");
                if ui.button("Cancel").clicked() {
                    pending.cancellation.cancel();
                }
            });
        } else if ui
            .add_enabled(
                self.readiness(variant).starts_with("Transport configured"),
                egui::Button::new("Run transport"),
            )
            .clicked()
        {
            self.start(ui.ctx().clone(), variant);
        }
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
        if let Some(record) = self.record(variant) {
            ui.label(format!(
                "Execution: {}",
                record
                    .execution
                    .as_ref()
                    .map_or("unavailable", |e| match e.execution_status {
                        faris_engine::jobs::ExecutionStatus::Succeeded => "completed",
                        faris_engine::jobs::ExecutionStatus::Failed => "failed",
                        faris_engine::jobs::ExecutionStatus::Cancelled => "cancelled",
                        faris_engine::jobs::ExecutionStatus::TimedOut => "timed out",
                        faris_engine::jobs::ExecutionStatus::OutputLimit => "log limit reached",
                        faris_engine::jobs::ExecutionStatus::ArtifactLimit =>
                            "artifact limit reached",
                        faris_engine::jobs::ExecutionStatus::FileSizeLimit =>
                            "single-file limit reached",
                    })
            ));
            ui.small(format!(
                "{} histories",
                u64::from(record.sampling.batches) * u64::from(record.sampling.particles_per_batch)
            ));
            if let Some(tally) = self.response(variant, "total-tritium-production") {
                let rate = record
                    .normalized
                    .as_ref()
                    .expect("normalized result")
                    .source_neutron_rate_per_s;
                ui.label(format!(
                    "Total H3/source: {:.4} ± {:.4}",
                    tally.integrated_mean / rate,
                    tally.integrated_standard_error / rate
                ));
                ui.small(
                    "± one Monte Carlo standard error; total-model production per primary neutron.",
                );
                if let Some(breeder) = self.response(variant, "blanket-tritium") {
                    ui.label(format!(
                        "Breeder H3/source: {:.4} ± {:.4}",
                        breeder.integrated_mean / rate,
                        breeder.integrated_standard_error / rate
                    ));
                    ui.small("Gross births. Recovery and fuel availability are separate.");
                }
            }
            if let Some(location) = self.locations.get(variant) {
                ui.collapsing("Transport record", |ui| {
                    ui.monospace(location.display().to_string());
                    ui.small(&record.notice);
                });
            }
        }
        ui.small("Cold-data surrogate. Scientific qualification: NOT_EVALUATED.");
    }

    pub fn viewport_controls(&mut self, ui: &mut egui::Ui, variant: &str, has_history: bool) {
        if self.record(variant).is_none_or(|r| r.normalized.is_none()) {
            self.view = FieldView::Materials;
            return;
        }
        egui::ComboBox::from_id_salt("field-view")
            .selected_text(match self.view {
                FieldView::Materials => "Materials",
                FieldView::ComponentFlux => "Mean component flux",
                FieldView::FluxSlice => "Spatial neutron flux",
                FieldView::NuclearHeating => "Nuclear heat deposition",
                FieldView::ComponentFluence => "Accumulated component fluence",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.view, FieldView::Materials, "Materials");
                ui.selectable_value(
                    &mut self.view,
                    FieldView::ComponentFlux,
                    "Mean component flux",
                );
                ui.selectable_value(&mut self.view, FieldView::FluxSlice, "Spatial neutron flux");
                let heat = self
                    .response(variant, "heating-total-whole-model")
                    .is_some();
                ui.add_enabled_ui(heat, |ui| {
                    ui.selectable_value(
                        &mut self.view,
                        FieldView::NuclearHeating,
                        "Nuclear heat deposition",
                    );
                });
                ui.add_enabled_ui(has_history, |ui| {
                    ui.selectable_value(
                        &mut self.view,
                        FieldView::ComponentFluence,
                        "Accumulated component fluence",
                    );
                });
            });
        if self.view == FieldView::FluxSlice {
            let last_slice = self
                .record(variant)
                .map_or(0, |r| r.mesh.dimensions[1].saturating_sub(1));
            ui.add(egui::Slider::new(&mut self.slice, 0..=last_slice).text("Y slice"));
        }
    }

    pub fn field_legend(&self, ui: &mut egui::Ui) {
        let (lower, upper, label) = match self.view {
            FieldView::Materials => return,
            FieldView::ComponentFlux | FieldView::FluxSlice => (10.0, 20.0, "neutrons/m²/s"),
            FieldView::NuclearHeating => (0.0, 8.0, "W/m³"),
            FieldView::ComponentFluence => (18.0, 28.0, "neutrons/m²"),
        };
        ui.horizontal(|ui| {
            ui.small(format!("≤10^{lower:.0}"));
            let (rect, _) = ui.allocate_exact_size(egui::vec2(150.0, 8.0), egui::Sense::hover());
            for i in 0..75 {
                let color = scalar_color(
                    10.0_f64.powf(lower + (upper - lower) * i as f64 / 74.0),
                    0.0,
                    lower,
                    upper,
                );
                ui.painter().rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(rect.left() + rect.width() * i as f32 / 75.0, rect.top()),
                        egui::pos2(
                            rect.left() + rect.width() * (i + 1) as f32 / 75.0,
                            rect.bottom(),
                        ),
                    ),
                    0.0,
                    egui::Color32::from_rgb(
                        (color[0] * 255.0) as u8,
                        (color[1] * 255.0) as u8,
                        (color[2] * 255.0) as u8,
                    ),
                );
            }
            ui.small(format!(
                "≥10^{upper:.0} {label} · log₁₀, fixed across arrangements"
            ));
        });
        ui.small(if self.view==FieldView::ComponentFluence {"Gray = zero accumulated exposure · magenta = unavailable. Conditional point history; uncertainty is not propagated."}else{"Gray = nonpositive sampled score · desaturated = >30% relative SE · magenta = unavailable. Limits saturate the color scale."});
    }

    pub fn spectra(&self, ui: &mut egui::Ui, variant: &str, component: &str) {
        let Some(record) = self.record(variant) else {
            return;
        };
        let Some(spectra) = &record.normalized_spectra else {
            return;
        };
        ui.collapsing("Energy-group spectra",|ui|{
            for spectrum in spectra.iter().filter(|s|s.component_id==component) {
                ui.strong(format!("{} group flux",spectrum.particle));
                let (rect,_)=ui.allocate_exact_size(egui::vec2(ui.available_width(),100.0),egui::Sense::hover());
                ui.painter().rect_filled(rect,2.0,egui::Color32::from_rgb(26,28,32));
                let maximum=spectrum.mean_per_square_metre_second.iter().copied().fold(0.0,f64::max);
                if maximum<=0.0 {ui.weak("No sampled nonzero group scores; no upper bound is inferred.");continue;}
                let ymax=maximum.log10().ceil(); let ymin=ymax-8.0;
                let xminimum=spectrum.energy_edges_ev.iter().copied().find(|e|*e>0.0).unwrap_or(1e-5).log10();
                let xmax=spectrum.energy_edges_ev.last().expect("validated energy edges").log10();
                let map_x=|e:f64|rect.left()+((e.max(10.0_f64.powf(xminimum)).log10()-xminimum)/(xmax-xminimum)) as f32*rect.width();
                let map_y=|v:f64|rect.bottom()-((v.max(10.0_f64.powf(ymin)).log10()-ymin)/(ymax-ymin)).clamp(0.0,1.0) as f32*rect.height();
                let color=if spectrum.particle=="neutron"{egui::Color32::from_rgb(108,183,238)}else{egui::Color32::from_rgb(234,164,83)};
                for (i,value) in spectrum.mean_per_square_metre_second.iter().enumerate() {
                    let left=map_x(spectrum.energy_edges_ev[i]);let right=map_x(spectrum.energy_edges_ev[i+1]);
                    ui.painter().line_segment([egui::pos2(left,map_y(*value)),egui::pos2(right,map_y(*value))],egui::Stroke::new(1.5,color));
                    let error=spectrum.standard_error_per_square_metre_second[i];let x=(left+right)*0.5;
                    ui.painter().line_segment([egui::pos2(x,map_y(value-error)),egui::pos2(x,map_y(value+error))],egui::Stroke::new(1.0,color.gamma_multiply(0.65)));
                }
                ui.small(format!("Energy: 10^{xminimum:.1}–10^{xmax:.1} eV (log). Flux: 10^{ymin:.0}–10^{ymax:.0} {}/m²/s per group (log).",spectrum.particle));
                ui.small("Whiskers: ±1 Monte Carlo SE, clipped at display floor. First group includes zero energy; these are group integrals, not differential flux or heat spectra.");
            }
            ui.small(format!("Sidecar: {}",record.transport_spectra_sha256.as_deref().unwrap_or("unavailable")));
        });
    }
}
impl Drop for TransportPanel {
    fn drop(&mut self) {
        if let Some(pending) = &mut self.pending {
            pending.cancellation.cancel();
            if let Some(handle) = pending.handle.take() {
                let _ = handle.join();
            }
        }
    }
}

/// Fixed physical scale across arrangements. Low-precision bins are desaturated;
/// sampled zero remains gray, with no inferred upper bound or zero-flux claim.
pub fn flux_color(mean: f64, standard_error: f64) -> [f32; 3] {
    scalar_color(mean, standard_error, 10.0, 20.0)
}

pub fn scalar_color(
    mean: f64,
    standard_error: f64,
    lower_log10: f64,
    upper_log10: f64,
) -> [f32; 3] {
    if mean <= 0.0 {
        return [0.18; 3];
    }
    let t = ((mean.log10() - lower_log10) / (upper_log10 - lower_log10)).clamp(0.0, 1.0) as f32;
    let mut color = [
        (2.0 * t - 0.6).clamp(0.0, 1.0),
        (1.0 - (2.0 * t - 1.0).abs()).clamp(0.0, 1.0),
        (1.4 - 2.0 * t).clamp(0.0, 1.0),
    ];
    if standard_error / mean > 0.3 {
        for channel in &mut color {
            *channel = *channel * 0.3 + 0.35;
        }
    }
    color
}
