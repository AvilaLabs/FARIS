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
}

pub struct TransportConfiguration {
    pub python: Option<PathBuf>,
    pub openmc: Option<PathBuf>,
    pub audit: Option<PathBuf>,
    pub cross_sections: Option<PathBuf>,
    pub physics: Vec<PathBuf>,
    pub runs: Vec<PathBuf>,
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
        for path in config.runs {
            let record = load_reactor_run(&path, &scenario).map_err(|e| e.to_string())?;
            locations.insert(record.variant_id.clone(), path);
            records.insert(record.variant_id.clone(), record);
        }
        Ok(Self {
            scenario,
            cases,
            records,
            locations,
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

    pub fn viewport_controls(&mut self, ui: &mut egui::Ui, variant: &str) {
        if self.record(variant).is_none_or(|r| r.normalized.is_none()) {
            self.view = FieldView::Materials;
            return;
        }
        egui::ComboBox::from_id_salt("field-view")
            .selected_text(match self.view {
                FieldView::Materials => "Materials",
                FieldView::ComponentFlux => "Mean component flux",
                FieldView::FluxSlice => "Spatial neutron flux",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.view, FieldView::Materials, "Materials");
                ui.selectable_value(
                    &mut self.view,
                    FieldView::ComponentFlux,
                    "Mean component flux",
                );
                ui.selectable_value(&mut self.view, FieldView::FluxSlice, "Spatial neutron flux");
            });
        if self.view == FieldView::FluxSlice {
            ui.add(egui::Slider::new(&mut self.slice, 0..=7).text("Y slice"));
        }
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
    if mean <= 0.0 {
        return [0.18; 3];
    }
    let t = ((mean.log10() - 10.0) / 10.0).clamp(0.0, 1.0) as f32;
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
