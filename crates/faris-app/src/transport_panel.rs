use crate::badge::{self, Kind};
use eframe::egui;
use faris_engine::{
    jobs::Cancellation,
    reactor::{
        FieldMesh, MeshPreset, ReactorJob, ReactorRun, SamplingPlan, load_physics_case,
        load_reactor_run, run_reactor,
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

pub use faris_engine::brief::TransportSummary;

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
    mesh_preset: MeshPreset,
    pending: Option<Pending>,
    last_attempt: Option<(faris_engine::jobs::ExecutionStatus, PathBuf)>,
    error: Option<String>,
    revision: u64,
    pub view: FieldView,
    pub slice: usize,
}

/// Thousands-separated integer for compact badge labels.
pub fn grouped(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
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
            let input_path = path
                .parent()
                .ok_or("Run record needs its input directory.")?
                .join("input.json");
            let input: serde_json::Value = serde_json::from_slice(
                &faris_engine::reactor::read_json_bytes(&input_path).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let case: PhysicsCase =
                serde_json::from_value(input["physics"].clone()).map_err(|e| e.to_string())?;
            case.validate_against(&scenario)
                .map_err(|e| e.to_string())?;
            if let Some(configured) = cases.get(&case.variant_id)
                && serde_json::to_value(configured).map_err(|e| e.to_string())?
                    != serde_json::to_value(&case).map_err(|e| e.to_string())?
            {
                return Err("Configured physics differs from the loaded transport. Open the historical record with its bound physics, or run the new case separately.".into());
            }
            cases.entry(case.variant_id.clone()).or_insert(case);
            if records.contains_key(&record.variant_id) {
                return Err("Duplicate transport record for an arrangement.".into());
            }
            locations.insert(record.variant_id.clone(), path);
            records.insert(record.variant_id.clone(), record);
        }
        for path in config.bundles {
            let loaded = faris_engine::core_evidence::load_recorded_bundle(&path, &scenario)?;
            let (record, case, directory) = (loaded.record, loaded.case, loaded.directory);
            if records.contains_key(&record.variant_id) {
                return Err("Duplicate transport record for an arrangement.".into());
            }
            if let Some(configured) = cases.get(&case.variant_id)
                && serde_json::to_value(configured).map_err(|e| e.to_string())?
                    != serde_json::to_value(&case).map_err(|e| e.to_string())?
            {
                return Err(
                    "Configured physics differs from the recorded bundle's bound physics.".into(),
                );
            }
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
            mesh_preset: MeshPreset::Coarse,
            pending: None,
            last_attempt: None,
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
    pub fn interface_status(&self, variant: &str) -> serde_json::Value {
        serde_json::json!({
            "pending":self.pending.is_some(), "has_error":self.error.is_some(),
            "error":self.error,
            "readiness":self.readiness(variant),
            "completed_record_available":self.record(variant).is_some_and(|r|r.normalized.is_some()),
            "last_attempt_status":self.last_attempt.as_ref().map(|(s,_)|s),
            "last_attempt_directory":self.last_attempt.as_ref().map(|(_,p)|p),
            "slice":self.slice,
        })
    }
    pub fn mesh_response(&self, variant: &str, bin: usize) -> Option<&NormalizedTally> {
        let record = self.record(variant)?;
        record.normalized.as_ref()?.results.iter().find(|r| {
            r.domain
                == faris_model::transport::ResponseDomain::Mesh {
                    mesh_id: record.mesh.id.clone(),
                    bin: bin as u64,
                }
        })
    }

    pub fn has_results(&self) -> bool {
        self.records
            .values()
            .any(|record| record.normalized.is_some())
    }

    /// Recorded, already-normalized transport quantities for one arrangement
    /// (the engine's summary; nothing is computed here).
    pub fn summary(&self, variant: &str) -> TransportSummary {
        self.record(variant)
            .map(faris_engine::brief::transport_summary)
            .unwrap_or_default()
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
                if let Some(execution) = &record.execution {
                    self.last_attempt = Some((execution.execution_status, pending.output.clone()));
                }
                if record.normalized.is_none() {
                    self.error = Some(record.import_error.clone().unwrap_or_else(|| {
                        format!(
                            "Transport did not produce accepted results; see {}",
                            pending.output.join("run.json").display()
                        )
                    }));
                } else {
                    self.locations
                        .insert(record.variant_id.clone(), pending.output.join("run.json"));
                    self.records.insert(record.variant_id.clone(), record);
                    self.revision += 1;
                }
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
        let mesh = match faris_engine::build_manifest(&scenario)
            .map_err(|e| e.to_string())
            .and_then(|m| FieldMesh::for_preset(&m, self.mesh_preset).map_err(|e| e.to_string()))
        {
            Ok(mesh) => mesh,
            Err(error) => {
                self.error = Some(error.to_string());
                return;
            }
        };
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
                    mesh: Some(mesh),
                    scenario: &scenario,
                    physics: &physics,
                    audit: &audit,
                    cross_sections: &cross_sections,
                    python: &python,
                    openmc: &openmc,
                    output: &worker_output,
                    sampling,
                    timeout: Duration::from_secs(3600),
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
        ui.strong("Run and review transport");
        ui.horizontal_wrapped(|ui| {
            if let Some(case) = self.case(variant) {
                ui.small(format!("Case: {}", case.id));
            } else {
                ui.small("No physics input selected for this arrangement.");
            }
            badge::badge(
                ui,
                Kind::Conditional,
                "cold-data surrogate · NOT_EVALUATED",
                "Cold-data surrogate. Scientific qualification: NOT_EVALUATED.",
            );
        });
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
            egui::ComboBox::from_id_salt("fresh-field-mesh")
                .selected_text(match self.mesh_preset {MeshPreset::Coarse => "Whole model · coarse", MeshPreset::OutboardLocalCoarse => "Port region · coarse", MeshPreset::OutboardLocal => "Port region · fine", MeshPreset::OutboardPortWindow => "Direct port-window average"})
                .show_ui(ui, |ui| {
                    for (preset, label) in [(MeshPreset::Coarse,"Whole model · 12 × 8 × 12"),(MeshPreset::OutboardLocalCoarse,"Port region · 12 × 6 × 12"),(MeshPreset::OutboardLocal,"Port region · 24 × 12 × 24"),(MeshPreset::OutboardPortWindow,"Direct port window · one bin")] {ui.selectable_value(&mut self.mesh_preset, preset, label);}
                });
            ui.small("One thread; 1 hour limit; 4 GiB process memory; 512 MiB artifacts. Mesh/request/JSON sizes are checked before spawning. Sampling precision is measured after execution.");
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
        if let Some((status, directory)) = &self.last_attempt {
            ui.small(format!("Latest attempt: {status:?}"));
            ui.collapsing("Attempt diagnostics", |ui| {
                ui.monospace(directory.display().to_string());
                ui.small("Input, logs and execution state are retained here. Completed results remain available after cancellation or failure.");
            });
        }
        if let Some(record) = self.record(variant) {
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
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
                    u64::from(record.sampling.batches)
                        * u64::from(record.sampling.particles_per_batch)
                ));
            });
            if let Some(precision) = &record.sampling_precision_summary {
                let targets = format!(
                    "Targets: {:.0}% integrated · {:.0}% local relative SE",
                    precision.integrated_goal * 100.0,
                    precision.local_goal * 100.0,
                );
                let review = "Sampling review only. Local goals may remain unresolved even when whole-model rates are precise.";
                let plan = format!(
                    "{}\n{}\n{}",
                    precision.plan_id, precision.purpose, precision.estimator
                );
                ui.horizontal_wrapped(|ui| {
                    if precision.all_goals_met {
                        badge::badge(
                            ui,
                            Kind::Checked,
                            "exploratory sampling goals met",
                            &format!("Exploratory sampling goals met.\n{targets}\n{review}\n{plan}"),
                        );
                    } else {
                        badge::badge(
                            ui,
                            Kind::Partial,
                            &format!(
                                "local precision {} / {} bins",
                                grouped(precision.checks_met),
                                grouped(precision.check_count)
                            ),
                            &format!(
                                "{} / {} sampling checks unresolved.\n{targets}\n{review}\n{plan}",
                                precision.checks_unmet, precision.check_count,
                            ),
                        );
                    }
                    if let (Some(tbr), Some(heating)) = (
                        precision.whole_model_tbr_relative_standard_error,
                        precision.whole_model_heating_relative_standard_error,
                    ) && tbr.max(heating) <= precision.integrated_goal
                    {
                        badge::badge(
                            ui,
                            Kind::Checked,
                            "whole-model goals met",
                            &format!(
                                "Whole-model relative standard errors: tritium breeding {:.2}%, heating {:.2}%, within the {:.0}% integrated target. Sampling precision only; model and data uncertainty are separate.",
                                tbr * 100.0,
                                heating * 100.0,
                                precision.integrated_goal * 100.0
                            ),
                        );
                    }
                });
            } else {
                ui.horizontal_wrapped(|ui| {
                    badge::badge(
                        ui,
                        Kind::NotEvaluated,
                        "sampling receipt unavailable",
                        "Sampling-goal receipt unavailable for this historical record.",
                    );
                });
            }
            if let Some(tally) = self.response(variant, "total-tritium-production") {
                let rate = record
                    .normalized
                    .as_ref()
                    .expect("normalized result")
                    .source_neutron_rate_per_s;
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!(
                        "Total H3/source: {:.4} ± {:.4}",
                        tally.integrated_mean / rate,
                        tally.integrated_standard_error / rate
                    ));
                    badge::badge(
                        ui,
                        Kind::Calculated,
                        "± 1 SE",
                        "± one Monte Carlo standard error; total-model production per primary neutron.",
                    );
                });
                if let Some(breeder) = self.response(variant, "blanket-tritium") {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!(
                            "Breeder H3/source: {:.4} ± {:.4}",
                            breeder.integrated_mean / rate,
                            breeder.integrated_standard_error / rate
                        ));
                        badge::badge(
                            ui,
                            Kind::Calculated,
                            "gross births",
                            "Gross births. Recovery and fuel availability are separate.",
                        );
                    });
                }
            }
            if let Some(location) = self.locations.get(variant) {
                ui.collapsing("Transport record", |ui| {
                    ui.monospace(location.display().to_string());
                    ui.small(&record.notice);
                });
            }
        }
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
        if matches!(
            self.view,
            FieldView::ComponentFlux | FieldView::FluxSlice | FieldView::NuclearHeating
        ) {
            ui.small("Stationary reference-source rate")
                .on_hover_text("These transport fields use the recorded source strength. They stay fixed during history outages; accumulated fluence follows the timeline. Instantaneous fields and decay heat are not shown here.");
        }
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
        let note = if self.view == FieldView::ComponentFluence {
            "Gray = zero sampled-mean fluence · no-track scores give no upper bound · magenta = unavailable. Conditional point history; uncertainty is not propagated."
        } else {
            "Gray = nonpositive sampled score · desaturated = >30% relative SE · magenta = unavailable. Limits saturate the color scale."
        };
        ui.horizontal_wrapped(|ui| {
            ui.small(format!("{label} · log₁₀, fixed across arrangements"));
            ui.small("(details)").on_hover_text(note);
        });
        let decades = (upper - lower) as usize;
        let bar_width = 360.0_f32.min(ui.available_width().max(120.0));
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(bar_width + 34.0, 30.0), egui::Sense::hover());
        response.on_hover_text(format!(
            "Values at or below 10^{lower:.0} and at or above 10^{upper:.0} saturate at the ends of the scale."
        ));
        let bar = egui::Rect::from_min_size(
            rect.min + egui::vec2(17.0, 0.0),
            egui::vec2(bar_width, 10.0),
        );
        let to_color32 = |color: [f32; 3]| {
            egui::Color32::from_rgb(
                (color[0] * 255.0).round() as u8,
                (color[1] * 255.0).round() as u8,
                (color[2] * 255.0).round() as u8,
            )
        };
        let painter = ui.painter();
        let mut gradient = egui::Mesh::default();
        const SEGMENTS: usize = 64;
        for i in 0..=SEGMENTS {
            let t = i as f32 / SEGMENTS as f32;
            let color = to_color32(colormap(t));
            let x = bar.left() + bar.width() * t;
            gradient.colored_vertex(egui::pos2(x, bar.top()), color);
            gradient.colored_vertex(egui::pos2(x, bar.bottom()), color);
            if i > 0 {
                let base = (i as u32 - 1) * 2;
                gradient.add_triangle(base, base + 1, base + 2);
                gradient.add_triangle(base + 1, base + 3, base + 2);
            }
        }
        painter.add(egui::Shape::mesh(gradient));
        painter.rect_stroke(
            bar,
            1.0,
            egui::Stroke::new(1.0, egui::Color32::from_gray(90)),
            egui::StrokeKind::Outside,
        );
        let label_step = if bar_width / decades.max(1) as f32 >= 34.0 {
            1
        } else {
            2
        };
        for decade in 0..=decades {
            let x = bar.left() + bar.width() * decade as f32 / decades.max(1) as f32;
            painter.line_segment(
                [
                    egui::pos2(x, bar.bottom()),
                    egui::pos2(x, bar.bottom() + 3.0),
                ],
                egui::Stroke::new(1.0, egui::Color32::from_gray(150)),
            );
            if decade % label_step == 0 {
                painter.text(
                    egui::pos2(x, bar.bottom() + 4.0),
                    egui::Align2::CENTER_TOP,
                    format!("10^{:.0}", lower + decade as f64),
                    egui::FontId::monospace(9.5),
                    egui::Color32::from_gray(185),
                );
            }
        }
        ui.horizontal_wrapped(|ui| {
            for (color, text, hover) in [
                (
                    to_color32(NONPOSITIVE_COLOR),
                    "no sampled score",
                    "Nonpositive or zero sampled score; no upper bound is inferred.",
                ),
                (
                    to_color32(scalar_color(10.0_f64.powf((lower + upper) * 0.5), 1.0e30, lower, upper)),
                    ">30% rel. SE",
                    "Relative Monte Carlo standard error above 30%: colour is blended toward neutral gray.",
                ),
                (
                    to_color32(UNAVAILABLE_COLOR),
                    "unavailable",
                    "No result is recorded for this component or bin.",
                ),
            ] {
                let (swatch, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().rect_filled(swatch, 2.0, color);
                ui.small(text).on_hover_text(hover);
                ui.add_space(6.0);
            }
        });
    }

    pub fn spectra(&self, ui: &mut egui::Ui, variant: &str, component: &str) {
        let Some(record) = self.record(variant) else {
            return;
        };
        let Some(spectra) = &record.normalized_spectra else {
            return;
        };
        ui.collapsing("Reference-source energy-group spectra",|ui|{
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

/// Sampled zero or nonpositive scores: neutral dark gray, never a data colour.
const NONPOSITIVE_COLOR: [f32; 3] = [0.18; 3];
/// Missing result (component or bin without a record).
const UNAVAILABLE_COLOR: [f32; 3] = [0.75, 0.10, 0.65];

/// Standard perceptually uniform "inferno" control points (sRGB, t = 0, 0.1, ... 1).
const INFERNO: [[f32; 3]; 11] = [
    [0.001462, 0.000466, 0.013866],
    [0.087411, 0.044556, 0.224813],
    [0.258234, 0.038571, 0.406485],
    [0.416331, 0.090203, 0.432943],
    [0.578304, 0.148039, 0.404411],
    [0.735683, 0.215906, 0.330245],
    [0.865006, 0.316822, 0.226055],
    [0.954506, 0.468744, 0.099874],
    [0.987622, 0.645320, 0.039886],
    [0.964394, 0.843848, 0.273391],
    [0.988362, 0.998364, 0.644924],
];

/// Inferno interpolated linearly between its control points. The darkest 4 %
/// is trimmed so the low end stays distinct from black backgrounds and from
/// the gray used for nonpositive scores.
fn colormap(t: f32) -> [f32; 3] {
    let position = (0.04 + 0.96 * t.clamp(0.0, 1.0)) * (INFERNO.len() - 1) as f32;
    let index = (position.floor() as usize).min(INFERNO.len() - 2);
    let fraction = position - index as f32;
    std::array::from_fn(|channel| {
        INFERNO[index][channel] * (1.0 - fraction) + INFERNO[index + 1][channel] * fraction
    })
}

pub fn scalar_color(
    mean: f64,
    standard_error: f64,
    lower_log10: f64,
    upper_log10: f64,
) -> [f32; 3] {
    if mean <= 0.0 {
        return NONPOSITIVE_COLOR;
    }
    let t = ((mean.log10() - lower_log10) / (upper_log10 - lower_log10)).clamp(0.0, 1.0) as f32;
    let mut color = colormap(t);
    if standard_error / mean > 0.3 {
        // Clearly muted: 55 % toward neutral gray.
        for channel in &mut color {
            *channel = *channel * 0.45 + 0.40 * 0.55;
        }
    }
    color
}

#[cfg(test)]
mod colormap_tests {
    use super::*;

    fn luminance(c: [f32; 3]) -> f32 {
        0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
    }

    #[test]
    fn scale_is_monotonic_in_luminance_and_saturates_at_the_limits() {
        let mut previous = -1.0;
        for step in 0..=20 {
            let color = scalar_color(10.0_f64.powf(10.0 + step as f64 * 0.5), 0.0, 10.0, 20.0);
            assert!(luminance(color) > previous);
            previous = luminance(color);
        }
        assert_eq!(
            scalar_color(1.0, 0.0, 10.0, 20.0),
            scalar_color(1e10, 0.0, 10.0, 20.0)
        );
        assert_eq!(
            scalar_color(1e30, 0.0, 10.0, 20.0),
            scalar_color(1e20, 0.0, 10.0, 20.0)
        );
    }

    #[test]
    fn zero_scores_are_gray_and_unavailable_is_not_a_data_color() {
        assert_eq!(scalar_color(0.0, 0.0, 10.0, 20.0), [0.18; 3]);
        assert_eq!(scalar_color(-1.0, 0.0, 10.0, 20.0), [0.18; 3]);
        for step in 0..=100 {
            let color = scalar_color(10.0_f64.powf(step as f64 * 0.1 + 10.0), 0.0, 10.0, 20.0);
            let distance: f32 = (0..3)
                .map(|i| (color[i] - UNAVAILABLE_COLOR[i]).abs())
                .sum();
            assert!(distance > 0.25);
        }
    }

    #[test]
    fn imprecise_bins_are_pulled_toward_neutral_gray() {
        let sharp = scalar_color(1e15, 1e13, 10.0, 20.0);
        let noisy = scalar_color(1e15, 5e14, 10.0, 20.0);
        let spread = |c: [f32; 3]| {
            c.iter().cloned().fold(0.0, f32::max) - c.iter().cloned().fold(1.0, f32::min)
        };
        assert!(spread(noisy) < 0.5 * spread(sharp));
    }
}
