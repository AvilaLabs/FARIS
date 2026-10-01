use crate::badge::{self, Kind};
use eframe::egui;
use faris_engine::{
    DemoManifest,
    core_evidence::{CoreEvidenceRun, prepare_case_with_assumptions, run_case, write_new},
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

/// Badge kind for a requirement verdict; absent or unknown states stay NOT_EVALUATED.
pub fn verdict_kind(status: &str) -> Kind {
    match status.to_ascii_lowercase().as_str() {
        "pass" => Kind::Checked,
        "fail" => Kind::Failed,
        "inconclusive" => Kind::Partial,
        _ => Kind::NotEvaluated,
    }
}

/// One labelled status line: caption on the left, badge on the right.
fn status_row(ui: &mut egui::Ui, caption: &str, kind: Kind, label: &str, explanation: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(caption);
        badge::badge(ui, kind, label, explanation);
    });
}

struct PendingCompilation {
    handle: Option<std::thread::JoinHandle<()>>,
    cancellation: Cancellation,
    receiver: Receiver<Result<CoreCompilation, String>>,
    variant_id: String,
    selection: StudySelection,
    output: PathBuf,
    scenario_sha256: String,
}

struct PendingEvidence {
    handle: Option<std::thread::JoinHandle<()>>,
    cancellation: Cancellation,
    receiver: Receiver<Result<CoreEvidenceRun, String>>,
    output: PathBuf,
    identity: EvidenceIdentity,
}

#[derive(Clone, PartialEq)]
struct EvidenceIdentity {
    scenario_sha256: String,
    variant: String,
    selection: StudySelection,
    run_path: PathBuf,
    assumptions: Option<faris_model::history::OperatingHistoryAssumptions>,
    core_path: String,
    faris_path: String,
}

/// Presentation only. All contract generation and compiler execution live in
/// faris-engine, shared with the CLI. No scientific status is inferred here.
pub struct StudyPanel {
    pub archive: crate::archive_panel::ArchivePanel,
    pub selection: StudySelection,
    /// Where the Status block was last drawn (guided tour spotlight).
    pub status_rect: Option<egui::Rect>,
    core_path: String,
    runs_directory: PathBuf,
    pending: Option<PendingCompilation>,
    completed: Option<CoreCompilation>,
    compiled_variant: Option<String>,
    compiled_selection: Option<StudySelection>,
    output: Option<PathBuf>,
    error: Option<String>,
    attempted: bool,
    compiled_scenario: Option<String>,
    faris_path: String,
    pending_evidence: Option<PendingEvidence>,
    evidence: Option<CoreEvidenceRun>,
    evidence_output: Option<PathBuf>,
    evidence_identity: Option<EvidenceIdentity>,
    reduced_motion: bool,
}

impl StudyPanel {
    pub fn new(core: Option<PathBuf>, runs_directory: PathBuf) -> Self {
        Self {
            archive: crate::archive_panel::ArchivePanel::default(),
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
            compiled_scenario: None,
            faris_path: std::env::var("FARIS_CLI_EXECUTABLE")
                .ok()
                .or_else(|| {
                    std::env::current_exe()
                        .ok()
                        .map(|p| p.with_file_name("faris").to_string_lossy().into_owned())
                })
                .unwrap_or_default(),
            pending_evidence: None,
            evidence: None,
            evidence_output: None,
            evidence_identity: None,
            reduced_motion: false,
            status_rect: None,
        }
    }

    /// The Core executable path currently configured, if any.
    pub fn core_path(&self) -> Option<PathBuf> {
        (!self.core_path.is_empty()).then(|| PathBuf::from(&self.core_path))
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        self.archive.poll(ctx);
        if let Some(pending) = &self.pending_evidence {
            let result = match pending.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    Some(Err("Evidence worker ended without a record.".into()))
                }
            };
            if let Some(result) = result {
                let mut pending = self.pending_evidence.take().expect("worker exists");
                if let Some(handle) = pending.handle.take() {
                    let _ = handle.join();
                }
                self.evidence_output = Some(pending.output);
                self.evidence_identity = Some(pending.identity);
                match result {
                    Ok(value) => self.evidence = Some(value),
                    Err(error) => self.error = Some(error),
                }
                ctx.request_repaint();
            }
        }
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
        let mut pending = self.pending.take().expect("pending worker exists");
        if let Some(handle) = pending.handle.take() {
            let _ = handle.join();
        }
        self.output = Some(pending.output);
        self.compiled_variant = Some(pending.variant_id);
        self.compiled_selection = Some(pending.selection);
        self.compiled_scenario = Some(pending.scenario_sha256);
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
            if self.reduced_motion {
                ui.small("Compiling…");
            } else {
                ui.spinner();
            }
        } else if ui.button("Compile study").clicked() {
            self.start(ui.ctx().clone(), manifest, variant_id);
        }
        let saved_tooltip = self
            .archive
            .core_tooltip(&manifest.source_sha256, variant_id);
        if self.attempted || saved_tooltip.is_some() {
            ui.small("Powered by Avila Core")
                .on_hover_text(saved_tooltip.unwrap_or_else(|| {
                    self.completed.as_ref().map_or_else(
                        || "Genuine external Avila Core compilation attempt.".into(),
                        |c| {
                            format!(
                                "{} · compiler {}",
                                c.report["semantic_profile"]
                                    .as_str()
                                    .unwrap_or("Profile unavailable"),
                                c.report.pointer("/compiled/compiler").map_or_else(
                                    || "Identity unavailable".into(),
                                    |v| v.to_string()
                                )
                            )
                        },
                    )
                }));
            // Draw the small spark directly; the default font has no ✦ glyph.
            let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
            let center = rect.center();
            for points in [
                [[0.0, -5.0], [1.7, 0.0], [0.0, 5.0], [-1.7, 0.0]],
                [[-5.0, 0.0], [0.0, -1.7], [5.0, 0.0], [0.0, 1.7]],
            ] {
                ui.painter().add(egui::Shape::convex_polygon(
                    points
                        .into_iter()
                        .map(|[x, y]| center + egui::vec2(x, y))
                        .collect(),
                    egui::Color32::from_rgb(193, 161, 96),
                    egui::Stroke::NONE,
                ));
            }
        }
    }

    pub fn interface_status(&self, scenario_sha256: &str, variant_id: &str) -> serde_json::Value {
        serde_json::json!({
            "compile_attempted":self.attempted, "compile_pending":self.pending.is_some(),
            "compilation_status":self.completed.as_ref().and_then(|c|c.report["status"].as_str()),
            "compilation_stale":self.completed.is_some() && (
                self.compiled_variant.as_deref()!=Some(variant_id)
                || self.compiled_selection.as_ref()!=Some(&self.selection)
                || self.compiled_scenario.as_deref()!=Some(scenario_sha256)),
            "has_error":self.error.is_some(), "evidence_pending":self.pending_evidence.is_some(),
            "error":self.error,
            "evidence_completed":self.evidence.as_ref().is_some_and(|e|e.completed()),
            "saved_core_verified":self.archive.core_tooltip(scenario_sha256,variant_id).is_some(),
        })
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
        if !executable.is_file() {
            self.error = Some("Avila Core executable is unavailable. Choose its installed file in Compiler settings.".into());
            return;
        }
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
            Ok(handle) => {
                self.attempted = true;
                self.pending = Some(PendingCompilation {
                    handle: Some(handle),
                    cancellation,
                    receiver,
                    variant_id: variant_id.into(),
                    selection: self.selection.clone(),
                    output,
                    scenario_sha256: manifest.source_sha256.clone(),
                });
            }
            Err(error) => self.error = Some(format!("Could not start compiler worker: {error}")),
        }
    }

    pub fn controls(
        &mut self,
        ui: &mut egui::Ui,
        variant_id: &str,
        scenario_sha256: &str,
        readiness: &str,
    ) {
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
            ui.checkbox(&mut self.reduced_motion, "Reduce Core progress animation");
            ui.label("Avila Core executable");
            ui.add(egui::TextEdit::singleline(&mut self.core_path).desired_width(f32::INFINITY));
            ui.small("Or set FARIS_CORE_EXECUTABLE before launch.");
            ui.label("FARIS CLI for controlled evidence stages");
            ui.add(egui::TextEdit::singleline(&mut self.faris_path).desired_width(f32::INFINITY));
        });
        ui.add_space(8.0);
        let status_top = ui.cursor().top();
        ui.strong("Status");
        if self.pending.is_some() {
            status_row(
                ui,
                "Compilation",
                Kind::Partial,
                "compiling",
                "The external Avila Core compiler is running on the current draft study. Compilation says only whether the study contract is well formed.",
            );
        } else if let Some(compilation) = &self.completed {
            let stale = self.compiled_variant.as_deref() != Some(variant_id)
                || self.compiled_selection.as_ref() != Some(&self.selection)
                || self.compiled_scenario.as_deref() != Some(scenario_sha256);
            let status = compilation.report["status"]
                .as_str()
                .unwrap_or("not_available");
            let kind = match status {
                "compiled" => Kind::Checked,
                "not_available" => Kind::NotEvaluated,
                s if s.contains("reject") || s.contains("fail") || s.contains("error") => {
                    Kind::Failed
                }
                _ => Kind::Partial,
            };
            ui.horizontal_wrapped(|ui| {
                ui.label("Compilation");
                badge::badge(
                    ui,
                    kind,
                    status,
                    "Result of the external Avila Core compilation of the current draft. Compilation establishes that the study contract is well formed; it is not run readiness and not a scientific verdict.",
                );
                if stale {
                    badge::badge(
                        ui,
                        Kind::Partial,
                        "study changed",
                        "The arrangement, analyses or scenario changed after this compilation. Compile the study again to refresh it.",
                    );
                }
            });
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
            status_row(
                ui,
                "Compilation",
                Kind::NotEvaluated,
                "not compiled",
                "No Avila Core compilation of the current draft yet. Use Compile study in the top bar.",
            );
        }
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
        if let Some(output) = &self.output {
            ui.collapsing("Saved study", |ui| {
                ui.monospace(output.display().to_string());
            });
        }
        status_row(
            ui,
            "Run readiness",
            if readiness.starts_with("Transport configured") {
                Kind::Checked
            } else {
                Kind::Partial
            },
            readiness,
            "Whether a fresh transport run can start for this arrangement: needs a physics input plus OpenMC, data-audit and cross-section paths (Transport configuration, Simulate step). Readiness is not a result and not a verdict.",
        );
        status_row(
            ui,
            "Scientific verdict",
            Kind::NotEvaluated,
            "NOT_EVALUATED",
            "No scientific assessment has been made at this level. Compilation and readiness do not establish one; a verdict needs executed, verified evidence stages with qualified bounds.",
        );
        self.status_rect = Some(egui::Rect::from_min_max(
            egui::pos2(ui.max_rect().left(), status_top),
            egui::pos2(ui.max_rect().right(), ui.cursor().top()),
        ));
        self.archive.controls(ui, scenario_sha256, variant_id);
    }

    pub fn evidence_controls(
        &mut self,
        ui: &mut egui::Ui,
        manifest: &DemoManifest,
        variant: &str,
        run_path: Option<&std::path::Path>,
        assumptions: Option<&faris_model::history::OperatingHistoryAssumptions>,
    ) {
        egui::CollapsingHeader::new("Core execution and evidence").default_open(true).show(ui,|ui|{
            if let Some(pending)=&self.pending_evidence {
                if self.reduced_motion {ui.small("Running…");} else {ui.spinner();} ui.label("Verifying artifacts and executing declared FARIS stages");
                if ui.button("Cancel evidence").clicked(){pending.cancellation.cancel();}
            } else {
                let ready=run_path.is_some()&&!self.core_path.trim().is_empty()&&!self.faris_path.trim().is_empty()
                    && (!(self.selection.fuel_history||self.selection.electricity)||assumptions.is_some());
                if ui.add_enabled(ready,egui::Button::new("Run bound study stages")).clicked()
                    && let Some(path)=run_path {self.start_evidence(ui.ctx().clone(),manifest,variant,path,assumptions);}
                if !ready {ui.small("Needs identified transport, Core/FARIS executables, and selected operating assumptions.");}
            }
            if let Some(evidence)=&self.evidence {
                let current = run_path.map(|run_path|EvidenceIdentity{scenario_sha256:manifest.source_sha256.clone(),variant:variant.into(),selection:self.selection.clone(),run_path:run_path.to_path_buf(),assumptions:assumptions.cloned(),core_path:self.core_path.clone(),faris_path:self.faris_path.clone()});
                if current.as_ref()!=self.evidence_identity.as_ref(){
                    ui.horizontal_wrapped(|ui|{
                        badge::badge(ui,Kind::Partial,"earlier inputs","Saved evidence belongs to earlier inputs or selections. Run the bound study stages again to refresh it.");
                        if self.evidence_identity.as_ref().is_some_and(|e|e.assumptions.as_ref()!=assumptions){
                            badge::badge(ui,Kind::Partial,"receipts cover different assumptions","The operating assumptions currently selected differ from those the saved evidence receipts cover. The receipts remain valid for their own assumptions; re-run the stages to cover the current ones.");
                        }
                    });
                }
                status_row(ui,"Core workflow",if evidence.completed(){Kind::Checked}else{Kind::Failed},if evidence.completed(){"executed and verified"}else{"incomplete or rejected"},"Executed Core stages and their verified receipts. This states that the declared stages ran and verified; it is not a scientific verdict.");
                ui.small(format!("Case: {}",evidence.expected_case_id));
                if let Some(verdicts)=evidence.report.pointer("/campaign/verdicts").and_then(serde_json::Value::as_array){for v in verdicts{
                    let status=v.pointer("/verdict/status").and_then(serde_json::Value::as_str).unwrap_or("not_evaluated").to_uppercase();
                    ui.horizontal_wrapped(|ui|{
                        ui.label(v["requirement_id"].as_str().unwrap_or("requirement"));
                        badge::badge(ui,verdict_kind(&status),&status,v["statement"].as_str().unwrap_or("Verdict scope is stated in the evidence report."));
                    });
                }}
                if let Some(findings)=evidence.report["findings"].as_array(){for f in findings{ui.small(format!("{}: {}",f["code"].as_str().unwrap_or("finding"),f["message"].as_str().unwrap_or("See record")));}}
            }
            if let Some(path)=&self.evidence_output{ui.small(format!("Evidence: {}",path.display()));}
            badge::badge(ui,Kind::Conditional,"qualified bounds unavailable","Transport stage verifies the prior OpenMC run; normalization/history/energy execute through the bound Rust CLI. Qualified physical bounds remain unavailable.");
        });
    }

    fn start_evidence(
        &mut self,
        ctx: egui::Context,
        manifest: &DemoManifest,
        variant: &str,
        run_path: &std::path::Path,
        assumptions: Option<&faris_model::history::OperatingHistoryAssumptions>,
    ) {
        let study = match generate_study(manifest, variant, &self.selection) {
            Ok(s) => s,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let output = self.runs_directory.join(format!("core-evidence-{stamp}"));
        let worker_output = output.clone();
        let core = PathBuf::from(&self.core_path);
        let faris = PathBuf::from(&self.faris_path);
        let run_path = run_path.to_path_buf();
        let assumptions = assumptions.cloned();
        let identity = EvidenceIdentity {
            scenario_sha256: manifest.source_sha256.clone(),
            variant: variant.into(),
            selection: self.selection.clone(),
            run_path: run_path.clone(),
            assumptions: assumptions.clone(),
            core_path: self.core_path.clone(),
            faris_path: self.faris_path.clone(),
        };
        let cancellation = Cancellation::default();
        let worker_cancel = cancellation.clone();
        let (sender, receiver) = mpsc::channel();
        let spawn = std::thread::Builder::new()
            .name("faris-core-evidence".into())
            .spawn(move || {
                let result =
                    (|| -> Result<CoreEvidenceRun, Box<dyn std::error::Error + Send + Sync>> {
                        std::fs::create_dir_all(&worker_output)?;
                        prepare_case_with_assumptions(
                            &study,
                            &run_path,
                            assumptions.as_ref(),
                            &core,
                            &faris,
                            &worker_output.join("case"),
                            &worker_cancel,
                        )?;
                        let record = run_case(
                            &worker_output.join("case"),
                            &core,
                            &faris,
                            &worker_output.join("execution"),
                            &worker_cancel,
                        )?;
                        write_new(
                            &worker_output.join("evidence.json"),
                            &serde_json::to_vec_pretty(&record)?,
                        )?;
                        Ok(record)
                    })()
                    .map_err(|e| e.to_string());
                let _ = sender.send(result);
                ctx.request_repaint();
            });
        match spawn {
            Ok(handle) => {
                self.attempted = true;
                self.pending_evidence = Some(PendingEvidence {
                    handle: Some(handle),
                    cancellation,
                    receiver,
                    output,
                    identity,
                });
                self.error = None;
                self.evidence = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }
}

impl Drop for StudyPanel {
    fn drop(&mut self) {
        if let Some(pending) = &mut self.pending {
            pending.cancellation.cancel();
            if let Some(handle) = pending.handle.take() {
                let _ = handle.join();
            }
        }
        if let Some(pending) = &mut self.pending_evidence {
            pending.cancellation.cancel();
            if let Some(handle) = pending.handle.take() {
                let _ = handle.join();
            }
        }
    }
}
