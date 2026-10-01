use eframe::egui;
use faris_engine::{
    DemoManifest,
    jobs::Cancellation,
    study::{
        CoreCompilation, StudySelection, compile_study, generate_study, local_core_executable,
    },
};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{SystemTime, UNIX_EPOCH},
};

struct PendingCompilation {
    cancellation: Cancellation,
    receiver: Receiver<Result<CoreCompilation, String>>,
    variant_id: String,
    selection: StudySelection,
    output: PathBuf,
}

/// Presentation only. All contract generation and compiler execution live in
/// faris-engine, shared with the CLI. No scientific status is inferred here.
pub struct StudyPanel {
    pub selection: StudySelection,
    core_path: String,
    runs_directory: PathBuf,
    pending: Option<PendingCompilation>,
    completed: Option<CoreCompilation>,
    compiled_variant: Option<String>,
    compiled_selection: Option<StudySelection>,
    output: Option<PathBuf>,
    error: Option<String>,
    attempted: bool,
}

impl StudyPanel {
    pub fn new(core: Option<PathBuf>, runs_directory: PathBuf) -> Self {
        Self {
            selection: StudySelection::default(),
            core_path: core
                .or_else(local_core_executable)
                .map_or_else(String::new, |p| p.to_string_lossy().into_owned()),
            runs_directory,
            pending: None,
            completed: None,
            compiled_variant: None,
            compiled_selection: None,
            output: None,
            error: None,
            attempted: false,
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
                Err("Compiler worker ended without a result.".into())
            }
        };
        let pending = self.pending.take().expect("pending worker exists");
        self.output = Some(pending.output);
        self.compiled_variant = Some(pending.variant_id);
        self.compiled_selection = Some(pending.selection);
        match result {
            Ok(compilation) => self.completed = Some(compilation),
            Err(error) => self.error = Some(error),
        }
        ctx.request_repaint();
    }

    pub fn header(&mut self, ui: &mut egui::Ui, manifest: &DemoManifest, variant_id: &str) {
        if let Some(pending) = &self.pending {
            if ui.button("Cancel compile").clicked() {
                pending.cancellation.cancel();
            }
            ui.spinner();
        } else if ui.button("Compile study").clicked() {
            self.start(ui.ctx().clone(), manifest, variant_id);
        }
        if self.attempted {
            ui.small("✦ Powered by Avila Core");
        }
    }

    fn start(&mut self, ctx: egui::Context, manifest: &DemoManifest, variant_id: &str) {
        self.error = None;
        self.completed = None;
        self.output = None;
        let study = match generate_study(manifest, variant_id, &self.selection) {
            Ok(study) => study,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        if self.core_path.trim().is_empty() {
            self.error = Some("Choose an Avila Core executable below the study selections.".into());
            return;
        }
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let output = self.runs_directory.join(format!("study-{stamp}"));
        let executable = PathBuf::from(&self.core_path);
        let cancellation = Cancellation::default();
        let worker_cancellation = cancellation.clone();
        let worker_output = output.clone();
        let (sender, receiver) = mpsc::channel();
        match std::thread::Builder::new()
            .name("faris-core-compile".into())
            .spawn(move || {
                let result =
                    compile_study(&study, &executable, &worker_output, &worker_cancellation)
                        .map_err(|error| error.to_string());
                let _ = sender.send(result);
                ctx.request_repaint();
            }) {
            Ok(_) => {
                self.attempted = true;
                self.pending = Some(PendingCompilation {
                    cancellation,
                    receiver,
                    variant_id: variant_id.into(),
                    selection: self.selection.clone(),
                    output,
                });
            }
            Err(error) => self.error = Some(format!("Could not start compiler worker: {error}")),
        }
    }

    pub fn controls(&mut self, ui: &mut egui::Ui, variant_id: &str) {
        ui.strong("Study analyses");
        ui.checkbox(&mut self.selection.breeding, "Tritium breeding");
        ui.checkbox(&mut self.selection.shielding, "Magnet-region shielding");
        ui.checkbox(
            &mut self.selection.fuel_history,
            "Fuel and operating history",
        );
        ui.checkbox(&mut self.selection.electricity, "Lifetime electricity");
        if self.selection.electricity {
            ui.small("Includes operating history and transport.");
        } else if self.selection.fuel_history {
            ui.small("Includes transport.");
        }
        if self.selection.breeding {
            ui.horizontal(|ui| {
                ui.label("Research TBR ≥");
                ui.add(
                    egui::TextEdit::singleline(&mut self.selection.minimum_tbr).desired_width(65.0),
                );
            });
        }
        ui.collapsing("Compiler settings", |ui| {
            ui.label("Avila Core executable");
            ui.add(egui::TextEdit::singleline(&mut self.core_path).desired_width(f32::INFINITY));
            ui.small("Or set FARIS_CORE_EXECUTABLE before launch.");
        });
        ui.add_space(8.0);
        if self.pending.is_some() {
            ui.label("Compilation: running");
        } else if let Some(compilation) = &self.completed {
            let stale = self.compiled_variant.as_deref() != Some(variant_id)
                || self.compiled_selection.as_ref() != Some(&self.selection);
            let status = compilation.report["status"]
                .as_str()
                .unwrap_or("not_available");
            ui.label(format!(
                "Compilation: {}{}",
                status,
                if stale { " · selections changed" } else { "" }
            ));
            ui.collapsing("Compiler findings", |ui| {
                if let Some(findings) = compilation.report["findings"].as_array() {
                    if findings.is_empty() {
                        ui.small("No compiler findings.");
                    }
                    for finding in findings {
                        ui.small(format!(
                            "{}: {}",
                            finding["code"].as_str().unwrap_or("finding"),
                            finding["message"]
                                .as_str()
                                .unwrap_or("See compilation.json")
                        ));
                    }
                }
                if let Some(reason) = compilation.report["reason"].as_str() {
                    ui.small(reason);
                }
                ui.small(format!(
                    "Execution: {:?}",
                    compilation.execution.execution_status
                ));
                if !compilation.execution.stderr.is_empty() {
                    ui.small(&compilation.execution.stderr);
                }
            });
        } else {
            ui.label("Compilation: not attempted");
        }
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
        if let Some(output) = &self.output {
            ui.collapsing("Saved study", |ui| {
                ui.monospace(output.display().to_string());
            });
        }
        ui.label("Run readiness: inputs not bound");
        ui.small("Materials, nuclear data and transport execution must be bound separately.");
        if self.selection.fuel_history || self.selection.electricity {
            ui.small("History and electricity stages are declared; their adapters are pending.");
        }
        ui.label("Scientific assessment: NOT_EVALUATED");
    }
}

impl Drop for StudyPanel {
    fn drop(&mut self) {
        if let Some(pending) = &self.pending {
            pending.cancellation.cancel();
        }
    }
}
