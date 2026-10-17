use crate::time::{Duration, SystemTime, UNIX_EPOCH};
use crate::{
    badge::{self, Kind},
    design_tools::SharedTools,
    settings::Tool,
};
use eframe::egui;
use faris_engine::{
    jobs::Cancellation,
    reactor::{
        FieldMesh, MeshPreset, ReactorJob, ReactorRun, SamplingPlan, WindowOptions,
        load_physics_case, load_reactor_run, run_reactor,
    },
    transport::NormalizedTally,
};
use faris_model::{LoadedScenario, physics::PhysicsCase};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
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

/// How hard a run samples: a quick look, or a run that stops on its error.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tier {
    #[default]
    Preview,
    Production,
}

/// Preview: a fixed count small enough to finish in about a minute. Measured on
/// the cold-coupled control, coarse mesh, 2 threads, on the reference laptop:
/// 30 × 3,000 = 90,000 histories took 46 s from start to finished record (about
/// 13 s of that is the worker's data check, 33 s the transport). At this size the
/// total-tritium error is about 0.4 % and the magnet fast flux about 50 to 65 %,
/// so the result view states both errors beside the numbers.
const PREVIEW_BATCHES: u32 = 30;
const PREVIEW_PARTICLES: u32 = 3_000;
/// Production stops once the magnet fast flux is known to this relative error.
const PRODUCTION_TARGET: f64 = 0.05;
/// Production budget: the engine's whole history cap (`MAX_RUN_HISTORIES`,
/// 50 million) in 250,000-history batches, so the run can stop at any batch
/// from the 30th on.
const PRODUCTION_BATCHES: u32 = 200;
const PRODUCTION_PARTICLES: u32 = 250_000;
/// With weight windows the magnet flux reaches 5 % in well under a million histories, and a run never stops
/// before 30 batches, so the batches must be small: 1,000 of 20,000 is a 20 million history budget. Measured on
/// the cold-coupled control, coarse mesh, 3 threads: 5 % after 30 to 40 such batches.
const WINDOWED_PRODUCTION_BATCHES: u32 = 1_000;
const WINDOWED_PRODUCTION_PARTICLES: u32 = 20_000;

/// Threads for a run: all but one of the machine's, at most four, at least one.
fn run_threads() -> u32 {
    let available = crate::thread::available_parallelism().map_or(1, |n| n.get());
    available.saturating_sub(1).clamp(1, 4) as u32
}

fn sampling_for(tier: Tier, threads: u32, windows: bool) -> SamplingPlan {
    match tier {
        Tier::Preview => SamplingPlan {
            batches: PREVIEW_BATCHES,
            particles_per_batch: PREVIEW_PARTICLES,
            threads,
            ..SamplingPlan::default()
        },
        Tier::Production => SamplingPlan {
            batches: if windows {
                WINDOWED_PRODUCTION_BATCHES
            } else {
                PRODUCTION_BATCHES
            },
            particles_per_batch: if windows {
                WINDOWED_PRODUCTION_PARTICLES
            } else {
                PRODUCTION_PARTICLES
            },
            threads,
            target_relative_error: Some(PRODUCTION_TARGET),
            ..SamplingPlan::default()
        },
    }
}

/// Which tier made a record, in words; earlier records only have a fixed count.
fn tier_label(plan: &SamplingPlan) -> &'static str {
    if plan.target_relative_error.is_some() {
        "Production tier"
    } else if plan.batches == PREVIEW_BATCHES && plan.particles_per_batch == PREVIEW_PARTICLES {
        "Preview tier"
    } else {
        "Fixed history count"
    }
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
    handle: Option<crate::thread::JoinHandle<()>>,
    cancellation: Cancellation,
    receiver: Receiver<Result<ReactorRun, String>>,
    output: PathBuf,
}

pub struct TransportPanel {
    scenario: LoadedScenario,
    cases: BTreeMap<String, PhysicsCase>,
    records: BTreeMap<String, ReactorRun>,
    locations: BTreeMap<String, PathBuf>,
    _replay_directories: Vec<faris_engine::vfs::ScratchDir>,
    python: String,
    openmc: String,
    audit: String,
    cross_sections: String,
    runs_directory: PathBuf,
    /// The Python with OpenMC 0.16 for Production windows, as last read from the tool paths the design windows
    /// share (a plain string, because panels are built on a thread and handed over).
    window_python: String,
    tier: Tier,
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
        let replay_directories = Vec::new();
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
        let mut panel = Self {
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
            window_python: String::new(),
            tier: Tier::default(),
            mesh_preset: MeshPreset::Coarse,
            pending: None,
            last_attempt: None,
            error: None,
            revision: 0,
            view: FieldView::default(),
            slice: 4,
        };
        for path in config.bundles {
            panel.add_bundle(path)?;
        }
        Ok(panel)
    }

    /// Check a recorded transport bundle against the scenario and add its
    /// record. The browser adds them one per frame.
    pub fn add_bundle(&mut self, path: PathBuf) -> Result<(), String> {
        let loaded = faris_engine::core_evidence::load_recorded_bundle(&path, &self.scenario)?;
        let (record, case, directory) = (loaded.record, loaded.case, loaded.directory);
        if self.records.contains_key(&record.variant_id) {
            return Err("Duplicate transport record for an arrangement.".into());
        }
        if let Some(configured) = self.cases.get(&case.variant_id)
            && serde_json::to_value(configured).map_err(|e| e.to_string())?
                != serde_json::to_value(&case).map_err(|e| e.to_string())?
        {
            return Err(
                "Configured physics differs from the recorded bundle's bound physics.".into(),
            );
        }
        self.cases.entry(case.variant_id.clone()).or_insert(case);
        self.locations
            .insert(record.variant_id.clone(), directory.path().join("run.json"));
        self.records.insert(record.variant_id.clone(), record);
        self._replay_directories.push(directory);
        Ok(())
    }

    /// The Python with OpenMC 0.16 that makes weight windows, when one is set.
    fn window_python(&self) -> Option<PathBuf> {
        let value = self.window_python.trim();
        (!value.is_empty()).then(|| PathBuf::from(value))
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

    /// A transport run is under way.
    pub fn is_running(&self) -> bool {
        self.pending.is_some()
    }

    /// Ask the run under way to stop. What it already wrote stays as it is.
    pub fn cancel_run(&self) {
        if let Some(pending) = &self.pending {
            pending.cancellation.cancel();
        }
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
        // Only a Production run uses windows, so a Preview never waits for them.
        let window_python = self
            .window_python()
            .filter(|_| self.tier == Tier::Production);
        let sampling = sampling_for(self.tier, run_threads(), window_python.is_some());
        let window_cache = self
            .runs_directory
            .join(faris_engine::reactor::WINDOW_CACHE_NAME);
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
        match crate::thread::Builder::new()
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
                    timeout: Duration::from_secs(faris_engine::reactor::MAX_RUN_TIMEOUT_SECONDS),
                    activation_spectra: None,
                    adapter: include_bytes!("../../../integrations/openmc/reactor_transport.py"),
                    windows: window_python.as_deref().map(|python| WindowOptions {
                        python,
                        cache_dir: Some(&window_cache),
                    }),
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

    pub fn controls(&mut self, ui: &mut egui::Ui, variant: &str, tools: Option<&SharedTools>) {
        if let Some(tools) = tools {
            self.window_python = tools.borrow().value(Tool::WindowPython);
        }
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
                &crate::not_evaluated::NOT_QUALIFIED.line(),
            );
        });
        if crate::browser::IS_BROWSER {
            crate::browser::desktop_only_note(ui, "Running new transport (OpenMC)");
        } else {
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
                if let Some(tools) = tools {
                    ui.label("Window Python (OpenMC 0.16, for Production)");
                    let mut text = self.window_python.clone();
                    if ui
                        .add(egui::TextEdit::singleline(&mut text).desired_width(f32::INFINITY))
                        .changed()
                    {
                        let mut tools = tools.borrow_mut();
                        tools.set(Tool::WindowPython, &text);
                        tools.flush(ui.ctx().egui_wants_keyboard_input());
                        self.window_python = text;
                    }
                }
                egui::ComboBox::from_id_salt("fresh-field-mesh")
                    .selected_text(match self.mesh_preset {MeshPreset::Coarse => "Whole model · coarse", MeshPreset::OutboardLocalCoarse => "Port region · coarse", MeshPreset::OutboardLocal => "Port region · fine", MeshPreset::OutboardPortWindow => "Direct port-window average"})
                    .show_ui(ui, |ui| {
                        for (preset, label) in [(MeshPreset::Coarse,"Whole model · 12 × 8 × 12"),(MeshPreset::OutboardLocalCoarse,"Port region · 12 × 6 × 12"),(MeshPreset::OutboardLocal,"Port region · 24 × 12 × 24"),(MeshPreset::OutboardPortWindow,"Direct port window · one bin")] {ui.selectable_value(&mut self.mesh_preset, preset, label);}
                    });
                ui.small(format!("{} thread(s); 4 hour limit; 4 GiB process memory; 512 MiB artifacts. Mesh/request/JSON sizes are checked before spawning. Sampling precision is measured after execution.", run_threads()));
            });
            ui.label("Run quality");
            ui.radio_value(&mut self.tier, Tier::Preview, "Preview · about a minute");
            ui.radio_value(
                &mut self.tier,
                Tier::Production,
                "Production · stop at 5 % on the magnet flux",
            );
            ui.small(match self.tier {
                Tier::Preview => "A fixed small run. The errors are stated beside the results; the magnet flux is only roughly known at this size.",
                Tier::Production if self.window_python().is_some() => "Makes weight windows aimed at the magnet flux first (kept for the next run on the same model), runs an ordinary pass for the whole-model results (tritium, heating), then a windowed pass until the magnet flux is known to 5 % or the 20 million history budget is used. Each result is taken from the pass that is valid for it. You can cancel at any time.",
                Tier::Production => "Runs until the magnet fast flux is known to 5 %, or until the 50 million history budget is used. Without a Window Python (OpenMC 0.16, under Transport configuration) it is analog and can take hours; you can cancel at any time.",
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
                .on_hover_text(crate::undo::cannot_undo_hint(
                    "Starts a transport run on this computer.",
                ))
                .clicked()
            {
                self.start(ui.ctx().clone(), variant);
            }
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
                            faris_engine::jobs::ExecutionStatus::MemoryLimit =>
                                "memory limit reached",
                        })
                ));
                ui.small(format!(
                    "{} histories",
                    grouped(record.histories_total() as usize)
                ));
            });
            ui.horizontal_wrapped(|ui| {
                ui.strong(tier_label(&record.sampling));
                match &record.sampling_outcome {
                    Some(outcome) if outcome.target_relative_error.is_some() => {
                        if let Some(passes) = record.passes_summary() {
                            ui.label(passes);
                            ui.label(format!("Analog pass: {}", outcome.summary()));
                            if let Some(windowed) = record
                                .windowed_run
                                .as_deref()
                                .and_then(|run| run.sampling_outcome.as_ref())
                            {
                                ui.label(format!("Windowed pass: {}", windowed.summary()));
                            }
                        } else {
                            ui.label(outcome.summary());
                        }
                        if let Some(variance) = &record.variance_reduction {
                            ui.label(variance.summary());
                        }
                    }
                    _ => {
                        let relative = |id: &str| {
                            self.response(variant, id).filter(|t| t.integrated_mean > 0.0).map(
                                |t| 100.0 * t.integrated_standard_error / t.integrated_mean,
                            )
                        };
                        if let (Some(magnet), Some(tritium)) = (
                            relative(faris_engine::reactor::STOP_RULE_RESPONSE_ID),
                            relative("total-tritium-production"),
                        ) {
                            ui.label(format!("Magnet fast flux {magnet:.1} % and total tritium {tritium:.2} % relative error. Choose Production to sample until the magnet flux is known to 5 %."));
                        }
                    }
                }
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
                if let Some(normalized) = record.normalized.as_ref() {
                    crate::tally_sum::zone_sum_ui(ui, normalized);
                }
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

    /// The legend of the current field view. `values` are the plotted values (`None`: no result
    /// recorded), from which the clamp and zero counts are made (VIS-025, VIS-026).
    pub fn field_legend(&self, ui: &mut egui::Ui, values: &[Option<f64>]) {
        let Some(scale) = field_scale(self.view) else {
            return;
        };
        let (lower, upper) = (scale.lower, scale.upper);
        let counts = scale_counts(values, &scale);
        let text = legend_text(&scale, &counts);
        let note = if self.view == FieldView::ComponentFluence {
            "Gray = zero sampled-mean fluence · no-track scores give no upper bound · magenta = unavailable. Conditional point history; uncertainty is not propagated."
        } else {
            "Gray = nonpositive sampled score · desaturated = >30% relative SE · magenta = unavailable. Limits saturate the color scale."
        };
        ui.horizontal_wrapped(|ui| {
            ui.small(&text.header);
            crate::badge::note(ui, "(details)", note);
        });
        let decades = (upper - lower) as usize;
        let bar_width = 360.0_f32.min(ui.available_width().max(120.0));
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(bar_width + 34.0, 30.0), egui::Sense::click());
        crate::badge::explain(response, |ui| {
            ui.label(format!(
                "Values at or below 10^{lower:.0} and at or above 10^{upper:.0} saturate at the ends of the scale."
            ));
        });
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
                    egui::FontId::monospace(11.0),
                    egui::Color32::from_gray(185),
                );
            }
        }
        // Clamp markers at the ends of the bar: a triangle where values outside the limits are
        // drawn at the end colour, so saturation is never silent.
        let marker_color = egui::Color32::from_gray(230);
        if counts.below > 0 {
            let (x, y) = (bar.left() - 8.0, bar.center().y);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(x - 4.0, y),
                    egui::pos2(x + 3.0, y - 5.0),
                    egui::pos2(x + 3.0, y + 5.0),
                ],
                marker_color,
                egui::Stroke::NONE,
            ));
        }
        if counts.above > 0 {
            let (x, y) = (bar.right() + 8.0, bar.center().y);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(x + 4.0, y),
                    egui::pos2(x - 3.0, y - 5.0),
                    egui::pos2(x - 3.0, y + 5.0),
                ],
                marker_color,
                egui::Stroke::NONE,
            ));
        }
        ui.horizontal_wrapped(|ui| {
            for (color, swatch_text, hover) in [
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
                crate::badge::note(ui, swatch_text, hover);
                ui.add_space(6.0);
            }
        });
        for line in &text.notes {
            ui.small(line);
        }
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

/// What a field view's colour scale is: the quantity, its unit and kind, and the log limits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldScale {
    pub quantity: &'static str,
    pub unit: &'static str,
    /// The kind of statement the plotted values are.
    pub kind: Kind,
    pub lower: f64,
    pub upper: f64,
}

/// The scale of a view; `None` for the materials view, which plots no quantity. The limits are
/// the ones the 3D view colours with (the `flux_color` and `scalar_color` calls in `rebuild`).
pub fn field_scale(view: FieldView) -> Option<FieldScale> {
    let transport = |quantity, unit, lower, upper| FieldScale {
        quantity,
        unit,
        kind: Kind::Calculated,
        lower,
        upper,
    };
    match view {
        FieldView::Materials => None,
        FieldView::ComponentFlux => Some(transport(
            "Mean component flux",
            "neutrons/m²/s",
            10.0,
            20.0,
        )),
        FieldView::FluxSlice => Some(transport(
            "Spatial neutron flux",
            "neutrons/m²/s",
            10.0,
            20.0,
        )),
        FieldView::NuclearHeating => Some(transport("Nuclear heat deposition", "W/m³", 0.0, 8.0)),
        FieldView::ComponentFluence => Some(FieldScale {
            quantity: "Accumulated component fluence",
            unit: "neutrons/m²",
            kind: Kind::Conditional,
            lower: 18.0,
            upper: 28.0,
        }),
    }
}

/// How the plotted values sit against a scale.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScaleCounts {
    /// Values with a result (the unavailable ones are not counted).
    pub total: usize,
    /// At or below zero: drawn gray, because a log scale cannot show them.
    pub nonpositive: usize,
    /// Positive but at or below the lower limit: drawn at the lowest colour.
    pub below: usize,
    /// At or above the upper limit: drawn at the highest colour.
    pub above: usize,
    /// No result recorded: drawn magenta.
    pub unavailable: usize,
}

/// Counts the values against the scale. A value exactly at a limit counts as clamped, as the
/// end of the scale is where it is drawn.
pub fn scale_counts(values: &[Option<f64>], scale: &FieldScale) -> ScaleCounts {
    let mut counts = ScaleCounts::default();
    for value in values {
        let Some(v) = value else {
            counts.unavailable += 1;
            continue;
        };
        counts.total += 1;
        if *v <= 0.0 || !v.is_finite() {
            counts.nonpositive += 1;
        } else if v.log10() <= scale.lower {
            counts.below += 1;
        } else if v.log10() >= scale.upper {
            counts.above += 1;
        }
    }
    counts
}

/// The words of a legend, made from the scale and the counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegendText {
    /// Quantity, unit, kind, scale type and explicit limits.
    pub header: String,
    /// The zero and clamp statements, one per line.
    pub notes: Vec<String>,
}

pub fn legend_text(scale: &FieldScale, counts: &ScaleCounts) -> LegendText {
    let header = format!(
        "{} · {} · kind: {} · log₁₀ scale from 10^{:.0} (minimum) to 10^{:.0} (maximum), same limits in every arrangement and panel",
        scale.quantity,
        scale.unit,
        scale.kind.default_label(),
        scale.lower,
        scale.upper
    );
    let mut notes = vec![format!(
        "{} of {} cells or components are at or below zero and drawn gray: a log scale cannot show zero, so the scale starts at 10^{:.0} {} and zero is not placed on it.",
        counts.nonpositive, counts.total, scale.lower, scale.unit
    )];
    if counts.below > 0 {
        notes.push(format!(
            "◂ {} at or below 10^{:.0} {} are drawn at the lowest colour (clamped).",
            counts.below, scale.lower, scale.unit
        ));
    }
    if counts.above > 0 {
        notes.push(format!(
            "▸ {} at or above 10^{:.0} {} are drawn at the highest colour (clamped).",
            counts.above, scale.upper, scale.unit
        ));
    }
    if counts.unavailable > 0 {
        notes.push(format!(
            "{} have no recorded result and are drawn magenta.",
            counts.unavailable
        ));
    }
    LegendText { header, notes }
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

/// Matplotlib "viridis", all 256 entries (sRGB, 0 to 1; van der Walt and Smith,
/// CC0). Designed to step evenly in CAM02-UCS and to stay readable with
/// common colour-vision deficiencies (VIS-020).
#[rustfmt::skip]
const VIRIDIS: [[f32; 3]; 256] = [
    [0.267004, 0.004874, 0.329415],
    [0.268510, 0.009605, 0.335427],
    [0.269944, 0.014625, 0.341379],
    [0.271305, 0.019942, 0.347269],
    [0.272594, 0.025563, 0.353093],
    [0.273809, 0.031497, 0.358853],
    [0.274952, 0.037752, 0.364543],
    [0.276022, 0.044167, 0.370164],
    [0.277018, 0.050344, 0.375715],
    [0.277941, 0.056324, 0.381191],
    [0.278791, 0.062145, 0.386592],
    [0.279566, 0.067836, 0.391917],
    [0.280267, 0.073417, 0.397163],
    [0.280894, 0.078907, 0.402329],
    [0.281446, 0.084320, 0.407414],
    [0.281924, 0.089666, 0.412415],
    [0.282327, 0.094955, 0.417331],
    [0.282656, 0.100196, 0.422160],
    [0.282910, 0.105393, 0.426902],
    [0.283091, 0.110553, 0.431554],
    [0.283197, 0.115680, 0.436115],
    [0.283229, 0.120777, 0.440584],
    [0.283187, 0.125848, 0.444960],
    [0.283072, 0.130895, 0.449241],
    [0.282884, 0.135920, 0.453427],
    [0.282623, 0.140926, 0.457517],
    [0.282290, 0.145912, 0.461510],
    [0.281887, 0.150881, 0.465405],
    [0.281412, 0.155834, 0.469201],
    [0.280868, 0.160771, 0.472899],
    [0.280255, 0.165693, 0.476498],
    [0.279574, 0.170599, 0.479997],
    [0.278826, 0.175490, 0.483397],
    [0.278012, 0.180367, 0.486697],
    [0.277134, 0.185228, 0.489898],
    [0.276194, 0.190074, 0.493001],
    [0.275191, 0.194905, 0.496005],
    [0.274128, 0.199721, 0.498911],
    [0.273006, 0.204520, 0.501721],
    [0.271828, 0.209303, 0.504434],
    [0.270595, 0.214069, 0.507052],
    [0.269308, 0.218818, 0.509577],
    [0.267968, 0.223549, 0.512008],
    [0.266580, 0.228262, 0.514349],
    [0.265145, 0.232956, 0.516599],
    [0.263663, 0.237631, 0.518762],
    [0.262138, 0.242286, 0.520837],
    [0.260571, 0.246922, 0.522828],
    [0.258965, 0.251537, 0.524736],
    [0.257322, 0.256130, 0.526563],
    [0.255645, 0.260703, 0.528312],
    [0.253935, 0.265254, 0.529983],
    [0.252194, 0.269783, 0.531579],
    [0.250425, 0.274290, 0.533103],
    [0.248629, 0.278775, 0.534556],
    [0.246811, 0.283237, 0.535941],
    [0.244972, 0.287675, 0.537260],
    [0.243113, 0.292092, 0.538516],
    [0.241237, 0.296485, 0.539709],
    [0.239346, 0.300855, 0.540844],
    [0.237441, 0.305202, 0.541921],
    [0.235526, 0.309527, 0.542944],
    [0.233603, 0.313828, 0.543914],
    [0.231674, 0.318106, 0.544834],
    [0.229739, 0.322361, 0.545706],
    [0.227802, 0.326594, 0.546532],
    [0.225863, 0.330805, 0.547314],
    [0.223925, 0.334994, 0.548053],
    [0.221989, 0.339161, 0.548752],
    [0.220057, 0.343307, 0.549413],
    [0.218130, 0.347432, 0.550038],
    [0.216210, 0.351535, 0.550627],
    [0.214298, 0.355619, 0.551184],
    [0.212395, 0.359683, 0.551710],
    [0.210503, 0.363727, 0.552206],
    [0.208623, 0.367752, 0.552675],
    [0.206756, 0.371758, 0.553117],
    [0.204903, 0.375746, 0.553533],
    [0.203063, 0.379716, 0.553925],
    [0.201239, 0.383670, 0.554294],
    [0.199430, 0.387607, 0.554642],
    [0.197636, 0.391528, 0.554969],
    [0.195860, 0.395433, 0.555276],
    [0.194100, 0.399323, 0.555565],
    [0.192357, 0.403199, 0.555836],
    [0.190631, 0.407061, 0.556089],
    [0.188923, 0.410910, 0.556326],
    [0.187231, 0.414746, 0.556547],
    [0.185556, 0.418570, 0.556753],
    [0.183898, 0.422383, 0.556944],
    [0.182256, 0.426184, 0.557120],
    [0.180629, 0.429975, 0.557282],
    [0.179019, 0.433756, 0.557430],
    [0.177423, 0.437527, 0.557565],
    [0.175841, 0.441290, 0.557685],
    [0.174274, 0.445044, 0.557792],
    [0.172719, 0.448791, 0.557885],
    [0.171176, 0.452530, 0.557965],
    [0.169646, 0.456262, 0.558030],
    [0.168126, 0.459988, 0.558082],
    [0.166617, 0.463708, 0.558119],
    [0.165117, 0.467423, 0.558141],
    [0.163625, 0.471133, 0.558148],
    [0.162142, 0.474838, 0.558140],
    [0.160665, 0.478540, 0.558115],
    [0.159194, 0.482237, 0.558073],
    [0.157729, 0.485932, 0.558013],
    [0.156270, 0.489624, 0.557936],
    [0.154815, 0.493313, 0.557840],
    [0.153364, 0.497000, 0.557724],
    [0.151918, 0.500685, 0.557587],
    [0.150476, 0.504369, 0.557430],
    [0.149039, 0.508051, 0.557250],
    [0.147607, 0.511733, 0.557049],
    [0.146180, 0.515413, 0.556823],
    [0.144759, 0.519093, 0.556572],
    [0.143343, 0.522773, 0.556295],
    [0.141935, 0.526453, 0.555991],
    [0.140536, 0.530132, 0.555659],
    [0.139147, 0.533812, 0.555298],
    [0.137770, 0.537492, 0.554906],
    [0.136408, 0.541173, 0.554483],
    [0.135066, 0.544853, 0.554029],
    [0.133743, 0.548535, 0.553541],
    [0.132444, 0.552216, 0.553018],
    [0.131172, 0.555899, 0.552459],
    [0.129933, 0.559582, 0.551864],
    [0.128729, 0.563265, 0.551229],
    [0.127568, 0.566949, 0.550556],
    [0.126453, 0.570633, 0.549841],
    [0.125394, 0.574318, 0.549086],
    [0.124395, 0.578002, 0.548287],
    [0.123463, 0.581687, 0.547445],
    [0.122606, 0.585371, 0.546557],
    [0.121831, 0.589055, 0.545623],
    [0.121148, 0.592739, 0.544641],
    [0.120565, 0.596422, 0.543611],
    [0.120092, 0.600104, 0.542530],
    [0.119738, 0.603785, 0.541400],
    [0.119512, 0.607464, 0.540218],
    [0.119423, 0.611141, 0.538982],
    [0.119483, 0.614817, 0.537692],
    [0.119699, 0.618490, 0.536347],
    [0.120081, 0.622161, 0.534946],
    [0.120638, 0.625828, 0.533488],
    [0.121380, 0.629492, 0.531973],
    [0.122312, 0.633153, 0.530398],
    [0.123444, 0.636809, 0.528763],
    [0.124780, 0.640461, 0.527068],
    [0.126326, 0.644107, 0.525311],
    [0.128087, 0.647749, 0.523491],
    [0.130067, 0.651384, 0.521608],
    [0.132268, 0.655014, 0.519661],
    [0.134692, 0.658636, 0.517649],
    [0.137339, 0.662252, 0.515571],
    [0.140210, 0.665859, 0.513427],
    [0.143303, 0.669459, 0.511215],
    [0.146616, 0.673050, 0.508936],
    [0.150148, 0.676631, 0.506589],
    [0.153894, 0.680203, 0.504172],
    [0.157851, 0.683765, 0.501686],
    [0.162016, 0.687316, 0.499129],
    [0.166383, 0.690856, 0.496502],
    [0.170948, 0.694384, 0.493803],
    [0.175707, 0.697900, 0.491033],
    [0.180653, 0.701402, 0.488189],
    [0.185783, 0.704891, 0.485273],
    [0.191090, 0.708366, 0.482284],
    [0.196571, 0.711827, 0.479221],
    [0.202219, 0.715272, 0.476084],
    [0.208030, 0.718701, 0.472873],
    [0.214000, 0.722114, 0.469588],
    [0.220124, 0.725509, 0.466226],
    [0.226397, 0.728888, 0.462789],
    [0.232815, 0.732247, 0.459277],
    [0.239374, 0.735588, 0.455688],
    [0.246070, 0.738910, 0.452024],
    [0.252899, 0.742211, 0.448284],
    [0.259857, 0.745492, 0.444467],
    [0.266941, 0.748751, 0.440573],
    [0.274149, 0.751988, 0.436601],
    [0.281477, 0.755203, 0.432552],
    [0.288921, 0.758394, 0.428426],
    [0.296479, 0.761561, 0.424223],
    [0.304148, 0.764704, 0.419943],
    [0.311925, 0.767822, 0.415586],
    [0.319809, 0.770914, 0.411152],
    [0.327796, 0.773980, 0.406640],
    [0.335885, 0.777018, 0.402049],
    [0.344074, 0.780029, 0.397381],
    [0.352360, 0.783011, 0.392636],
    [0.360741, 0.785964, 0.387814],
    [0.369214, 0.788888, 0.382914],
    [0.377779, 0.791781, 0.377939],
    [0.386433, 0.794644, 0.372886],
    [0.395174, 0.797475, 0.367757],
    [0.404001, 0.800275, 0.362552],
    [0.412913, 0.803041, 0.357269],
    [0.421908, 0.805774, 0.351910],
    [0.430983, 0.808473, 0.346476],
    [0.440137, 0.811138, 0.340967],
    [0.449368, 0.813768, 0.335384],
    [0.458674, 0.816363, 0.329727],
    [0.468053, 0.818921, 0.323998],
    [0.477504, 0.821444, 0.318195],
    [0.487026, 0.823929, 0.312321],
    [0.496615, 0.826376, 0.306377],
    [0.506271, 0.828786, 0.300362],
    [0.515992, 0.831158, 0.294279],
    [0.525776, 0.833491, 0.288127],
    [0.535621, 0.835785, 0.281908],
    [0.545524, 0.838039, 0.275626],
    [0.555484, 0.840254, 0.269281],
    [0.565498, 0.842430, 0.262877],
    [0.575563, 0.844566, 0.256415],
    [0.585678, 0.846661, 0.249897],
    [0.595839, 0.848717, 0.243329],
    [0.606045, 0.850733, 0.236712],
    [0.616293, 0.852709, 0.230052],
    [0.626579, 0.854645, 0.223353],
    [0.636902, 0.856542, 0.216620],
    [0.647257, 0.858400, 0.209861],
    [0.657642, 0.860219, 0.203082],
    [0.668054, 0.861999, 0.196293],
    [0.678489, 0.863742, 0.189503],
    [0.688944, 0.865448, 0.182725],
    [0.699415, 0.867117, 0.175971],
    [0.709898, 0.868751, 0.169257],
    [0.720391, 0.870350, 0.162603],
    [0.730889, 0.871916, 0.156029],
    [0.741388, 0.873449, 0.149561],
    [0.751884, 0.874951, 0.143228],
    [0.762373, 0.876424, 0.137064],
    [0.772852, 0.877868, 0.131109],
    [0.783315, 0.879285, 0.125405],
    [0.793760, 0.880678, 0.120005],
    [0.804182, 0.882046, 0.114965],
    [0.814576, 0.883393, 0.110347],
    [0.824940, 0.884720, 0.106217],
    [0.835270, 0.886029, 0.102646],
    [0.845561, 0.887322, 0.099702],
    [0.855810, 0.888601, 0.097452],
    [0.866013, 0.889868, 0.095953],
    [0.876168, 0.891125, 0.095250],
    [0.886271, 0.892374, 0.095374],
    [0.896320, 0.893616, 0.096335],
    [0.906311, 0.894855, 0.098125],
    [0.916242, 0.896091, 0.100717],
    [0.926106, 0.897330, 0.104071],
    [0.935904, 0.898570, 0.108131],
    [0.945636, 0.899815, 0.112838],
    [0.955300, 0.901065, 0.118128],
    [0.964894, 0.902323, 0.123941],
    [0.974417, 0.903590, 0.130215],
    [0.983868, 0.904867, 0.136897],
    [0.993248, 0.906157, 0.143936],
];

/// Viridis interpolated linearly between adjacent table entries. Its darkest
/// entry is a saturated purple, distinct from black backgrounds and from the
/// gray used for nonpositive scores, so nothing is trimmed.
fn colormap(t: f32) -> [f32; 3] {
    let position = t.clamp(0.0, 1.0) * (VIRIDIS.len() - 1) as f32;
    let index = (position.floor() as usize).min(VIRIDIS.len() - 2);
    let fraction = position - index as f32;
    std::array::from_fn(|channel| {
        VIRIDIS[index][channel] * (1.0 - fraction) + VIRIDIS[index + 1][channel] * fraction
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

    // Verifies: VIS-020
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

    /// CAM02-UCS J', a', b' (Luo, Cui and Li 2006) of an sRGB colour with 0 to 1
    /// channels, through CIECAM02 (CIE 159:2004) under the viewing conditions viridis
    /// was designed with: D65 white, adapting luminance 64/(5 pi) cd/m2, background
    /// Y 20, average surround.
    fn cam02_ucs(c: [f32; 3]) -> [f64; 3] {
        let linear = |v: f32| {
            let v = v as f64;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = (linear(c[0]), linear(c[1]), linear(c[2]));
        let xyz = [
            100.0 * (0.4124564 * r + 0.3575761 * g + 0.1804375 * b),
            100.0 * (0.2126729 * r + 0.7151522 * g + 0.0721750 * b),
            100.0 * (0.0193339 * r + 0.1191920 * g + 0.9503041 * b),
        ];
        let white: [f64; 3] = [95.047, 100.0, 108.883];
        let (l_a, y_b) = (64.0 / std::f64::consts::PI / 5.0, 20.0_f64);
        let (f, c_surround, n_c) = (1.0, 0.69, 1.0);
        let cat02 = |v: [f64; 3]| {
            [
                0.7328 * v[0] + 0.4296 * v[1] - 0.1624 * v[2],
                -0.7036 * v[0] + 1.6975 * v[1] + 0.0061 * v[2],
                0.0030 * v[0] + 0.0136 * v[1] + 0.9834 * v[2],
            ]
        };
        // CAT02 inverse followed by Hunt-Pointer-Estevez, as one step.
        let to_hpe = |v: [f64; 3]| {
            let inv = [
                [1.096124, -0.278869, 0.182745],
                [0.454369, 0.473533, 0.072098],
                [-0.009628, -0.005698, 1.015326],
            ];
            let xyz: [f64; 3] =
                std::array::from_fn(|i| inv[i][0] * v[0] + inv[i][1] * v[1] + inv[i][2] * v[2]);
            [
                0.38971 * xyz[0] + 0.68898 * xyz[1] - 0.07868 * xyz[2],
                -0.22981 * xyz[0] + 1.18340 * xyz[1] + 0.04641 * xyz[2],
                xyz[2],
            ]
        };
        let d = (f * (1.0 - (1.0 / 3.6) * ((-l_a - 42.0) / 92.0).exp())).clamp(0.0, 1.0);
        let k = 1.0 / (5.0 * l_a + 1.0);
        let f_l =
            0.2 * k.powi(4) * 5.0 * l_a + 0.1 * (1.0 - k.powi(4)).powi(2) * (5.0 * l_a).cbrt();
        let n = y_b / white[1];
        let z = 1.48 + n.sqrt();
        let n_bb = 0.725 * (1.0 / n).powf(0.2);
        let rgb_w = cat02(white);
        let adapt = |v: [f64; 3]| -> [f64; 3] {
            let rgb = cat02(v);
            let rgb_c = std::array::from_fn(|i| (white[1] * d / rgb_w[i] + 1.0 - d) * rgb[i]);
            to_hpe(rgb_c).map(|v| {
                let p = (f_l * v.abs() / 100.0).powf(0.42);
                v.signum() * 400.0 * p / (27.13 + p) + 0.1
            })
        };
        let achromatic = |a: [f64; 3]| (2.0 * a[0] + a[1] + a[2] / 20.0 - 0.305) * n_bb;
        let a_w = achromatic(adapt(white));
        let rgb_a = adapt(xyz);
        let a = rgb_a[0] - 12.0 * rgb_a[1] / 11.0 + rgb_a[2] / 11.0;
        let b = (rgb_a[0] + rgb_a[1] - 2.0 * rgb_a[2]) / 9.0;
        let h = b.atan2(a).rem_euclid(2.0 * std::f64::consts::PI);
        let e_t = 0.25 * ((h + 2.0).cos() + 3.8);
        let j = 100.0 * (achromatic(rgb_a) / a_w).max(0.0).powf(c_surround * z);
        let t = (50000.0 / 13.0 * n_c * n_bb * e_t * a.hypot(b))
            / (rgb_a[0] + rgb_a[1] + 21.0 / 20.0 * rgb_a[2]);
        let chroma = t.powf(0.9) * (j / 100.0).sqrt() * (1.64 - 0.29f64.powf(n)).powf(0.73);
        let m = chroma * f_l.powf(0.25);
        let j_prime = 1.7 * j / (1.0 + 0.007 * j);
        let m_prime = (1.0 + 0.0228 * m).ln() / 0.0228;
        [j_prime, m_prime * h.cos(), m_prime * h.sin()]
    }

    #[test]
    fn cam02_ucs_matches_the_colour_science_library() {
        // colour-science 0.4.7: XYZ_to_CIECAM02 with the conditions above, then the
        // Luo et al. CAM02-UCS coefficients.
        let reference: [([f32; 3], [f64; 3]); 5] = [
            ([1.0, 1.0, 1.0], [100.0, -1.9139, -1.1496]),
            ([1.0, 0.0, 0.0], [60.0495, 38.6868, 24.3199]),
            ([0.2, 0.6, 0.3], [57.2978, -22.5214, 15.4081]),
            ([0.267004, 0.004874, 0.329415], [18.6503, 18.1557, -16.4061]),
            ([0.993248, 0.906157, 0.143936], [92.3953, -5.4743, 33.395]),
        ];
        for (rgb, expected) in reference {
            let got = cam02_ucs(rgb);
            for i in 0..3 {
                assert!(
                    (got[i] - expected[i]).abs() < 0.02,
                    "{rgb:?}: {got:?} vs {expected:?}"
                );
            }
        }
    }

    // Verifies: VIS-020
    #[test]
    fn adjacent_levels_of_the_default_map_step_evenly_in_cam02_ucs() {
        let levels: Vec<[f64; 3]> = (0..256)
            .map(|i| cam02_ucs(colormap(i as f32 / 255.0)))
            .collect();
        for pair in levels.windows(2) {
            assert!(
                pair[1][0] > pair[0][0],
                "lightness J' must rise at every level"
            );
        }
        let steps: Vec<f64> = levels
            .windows(2)
            .map(|p| {
                ((p[1][0] - p[0][0]).powi(2)
                    + (p[1][1] - p[0][1]).powi(2)
                    + (p[1][2] - p[0][2]).powi(2))
                .sqrt()
            })
            .collect();
        let mean = steps.iter().sum::<f64>() / steps.len() as f64;
        let worst = steps
            .iter()
            .map(|s| (s / mean - 1.0).abs())
            .fold(0.0, f64::max);
        // VIS-020: every step within 25 % of the mean step. Measured 1.9 % for the
        // 256-entry viridis table (colour-science gives the same).
        assert!(
            worst <= 0.25,
            "worst step {:.1} % from the mean {mean:.3}",
            worst * 100.0
        );
    }

    // Verifies: VIS-026
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

    // Verifies: VIS-025
    #[test]
    fn every_field_legend_names_quantity_unit_kind_scale_limits_and_swatches() {
        let views = [
            FieldView::ComponentFlux,
            FieldView::FluxSlice,
            FieldView::NuclearHeating,
            FieldView::ComponentFluence,
        ];
        for view in views {
            let scale = field_scale(view).expect("a plotted view has a scale");
            let text = legend_text(&scale, &ScaleCounts::default());
            assert!(text.header.contains(scale.quantity), "{view:?}");
            assert!(text.header.contains(scale.unit), "{view:?}");
            assert!(text.header.contains("kind: "), "{view:?}");
            assert!(text.header.contains("log"), "{view:?}");
            assert!(
                text.header
                    .contains(&format!("10^{:.0} (minimum)", scale.lower))
            );
            assert!(
                text.header
                    .contains(&format!("10^{:.0} (maximum)", scale.upper))
            );
            assert!(text.header.contains("same limits in every arrangement"));
        }
        assert_eq!(field_scale(FieldView::Materials), None);
        // The limits are the ones the colours use.
        assert_eq!(field_scale(FieldView::ComponentFlux).unwrap().upper, 20.0);
        assert_eq!(field_scale(FieldView::NuclearHeating).unwrap().lower, 0.0);
    }

    // Verifies: VIS-025, VIS-026
    #[test]
    fn clamped_and_zero_values_are_counted_and_named() {
        let scale = field_scale(FieldView::ComponentFlux).unwrap();
        let values = [
            Some(0.0),
            Some(-1.0),
            Some(1.0e5),  // positive, below the lower limit
            Some(1.0e10), // exactly at the lower limit
            Some(1.0e15), // inside
            Some(1.0e20), // exactly at the upper limit
            Some(1.0e25), // above
            None,
        ];
        let counts = scale_counts(&values, &scale);
        assert_eq!(
            counts,
            ScaleCounts {
                total: 7,
                nonpositive: 2,
                below: 2,
                above: 2,
                unavailable: 1
            }
        );
        let text = legend_text(&scale, &counts);
        let all = text.notes.join("\n");
        assert!(
            all.contains("2 of 7 cells or components are at or below zero"),
            "{all}"
        );
        assert!(all.contains("scale starts at 10^10"), "{all}");
        assert!(all.contains("◂ 2 at or below 10^10"), "{all}");
        assert!(all.contains("▸ 2 at or above 10^20"), "{all}");
        assert!(all.contains("1 have no recorded result"), "{all}");
        // Nothing clamped: no clamp line, but the zero statement stays (even for zero zeros).
        let quiet = legend_text(&scale, &scale_counts(&[Some(1.0e15)], &scale));
        assert_eq!(quiet.notes.len(), 1);
        assert!(quiet.notes[0].contains("0 of 1 cells"));
    }

    // Verifies: VIS-032
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

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: PERF-025
    #[test]
    fn both_tiers_are_valid_sampling_plans() {
        for threads in [1, 4] {
            for tier in [Tier::Preview, Tier::Production] {
                for windows in [false, true] {
                    sampling_for(tier, threads, windows).validate().unwrap();
                }
            }
        }
        let preview = sampling_for(Tier::Preview, 2, false);
        assert_eq!(preview.target_relative_error, None);
        assert!(
            u64::from(preview.batches) * u64::from(preview.particles_per_batch) <= 100_000,
            "preview must stay a small fixed run"
        );
    }

    // Verifies: PERF-025, NUC-043
    #[test]
    fn production_stops_on_a_target_within_the_history_cap() {
        let production = sampling_for(Tier::Production, 3, false);
        assert_eq!(production.target_relative_error, Some(0.05));
        assert_eq!(
            u64::from(production.batches) * u64::from(production.particles_per_batch),
            faris_engine::reactor::MAX_RUN_HISTORIES
        );
        assert_eq!(tier_label(&production), "Production tier");
        assert_eq!(
            tier_label(&sampling_for(Tier::Preview, 1, false)),
            "Preview tier"
        );
        // With windows the batches are small, so a run can stop soon after its 30th batch.
        let windowed = sampling_for(Tier::Production, 3, true);
        assert_eq!(windowed.target_relative_error, Some(0.05));
        assert!(
            u64::from(windowed.batches) * u64::from(windowed.particles_per_batch)
                <= faris_engine::reactor::MAX_RUN_HISTORIES
        );
        assert!(30 * u64::from(windowed.particles_per_batch) <= 1_000_000);
        assert_eq!(tier_label(&SamplingPlan::default()), "Fixed history count");
    }

    #[test]
    fn run_threads_are_between_one_and_four() {
        assert!((1..=4).contains(&run_threads()));
    }
}
