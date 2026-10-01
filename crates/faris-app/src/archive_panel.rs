//! Reopening saved studies revalidates their identities on a worker thread.

use eframe::egui;
use faris_engine::{
    case_archive::{SavedCaseInspection, inspect_saved_case},
    history::HistoryResult,
};
use serde::Deserialize;
use std::{
    path::{Component, Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Locations {
    case_directory: PathBuf,
    execution_report: PathBuf,
    execution_workspace: PathBuf,
}

struct Pending {
    handle: std::thread::JoinHandle<()>,
    receiver: Receiver<Vec<Result<SavedCaseInspection, String>>>,
}

#[derive(Default)]
pub struct ArchivePanel {
    queued_descriptors: Vec<PathBuf>,
    pending: Option<Pending>,
    saved: Vec<SavedCaseInspection>,
    selected: usize,
    incoming: Vec<(String, String, HistoryResult)>,
    errors: Vec<String>,
    case_directory: String,
    execution_report: String,
    execution_workspace: String,
}

impl ArchivePanel {
    pub fn queue_descriptors(&mut self, paths: Vec<PathBuf>) -> Result<(), String> {
        if paths.len() > 16 {
            return Err("At most 16 saved study descriptors can be opened together".into());
        }
        self.queued_descriptors = paths;
        Ok(())
    }

    pub fn is_loading(&self) -> bool {
        self.pending.is_some() || !self.queued_descriptors.is_empty()
    }

    pub fn take_histories(&mut self) -> Vec<(String, String, HistoryResult)> {
        std::mem::take(&mut self.incoming)
    }

    pub fn core_tooltip(&self, scenario_sha256: &str, variant_id: &str) -> Option<String> {
        let matches = |saved: &&SavedCaseInspection| {
            scenario_digest(saved) == scenario_sha256 && saved.variant_id == variant_id
        };
        let saved = self
            .saved
            .get(self.selected)
            .filter(matches)
            .or_else(|| self.saved.iter().find(matches))?;
        Some(format!(
            "Verified saved study: {}\n{}\n{}\nCompiler SHA-256: {}\n{} stage receipts verified.\n{}",
            saved.case_id,
            saved.compiler_id,
            saved.semantic_profile,
            saved.compiler_executable_sha256,
            saved.verified_receipt_count,
            saved.scope_notice,
        ))
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        if self.pending.is_none() && !self.queued_descriptors.is_empty() {
            let descriptors = std::mem::take(&mut self.queued_descriptors);
            self.start(ctx.clone(), move || {
                descriptors
                    .iter()
                    .map(|path| load_descriptor(path).and_then(inspect))
                    .collect()
            });
        }
        let Some(pending) = &self.pending else {
            return;
        };
        let results = match pending.receiver.try_recv() {
            Ok(value) => value,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                vec![Err("Saved-study worker ended without results".into())]
            }
        };
        let pending = self.pending.take().expect("worker exists");
        let _ = pending.handle.join();
        for result in results {
            match result {
                Ok(mut saved) => {
                    if let Some(history) = saved.history_result.take() {
                        self.incoming.push((
                            scenario_digest(&saved).to_owned(),
                            saved.variant_id.clone(),
                            history,
                        ));
                    }
                    self.saved.push(saved);
                    self.selected = self.saved.len() - 1;
                }
                Err(error) => self.errors.push(error),
            }
        }
        ctx.request_repaint();
    }

    fn start(
        &mut self,
        ctx: egui::Context,
        work: impl FnOnce() -> Vec<Result<SavedCaseInspection, String>> + Send + 'static,
    ) {
        let (sender, receiver) = mpsc::channel();
        match std::thread::Builder::new()
            .name("faris-reopen-saved-study".into())
            .spawn(move || {
                let _ = sender.send(work());
                ctx.request_repaint();
            }) {
            Ok(handle) => self.pending = Some(Pending { handle, receiver }),
            Err(error) => self.errors.push(error.to_string()),
        }
    }

    pub fn controls(&mut self, ui: &mut egui::Ui, current_scenario: &str, current_variant: &str) {
        ui.collapsing("Reopen saved study", |ui| {
            for (label, value) in [
                ("Case directory", &mut self.case_directory),
                ("Saved execution report", &mut self.execution_report),
                ("Execution workspace", &mut self.execution_workspace),
            ] {
                ui.label(label);
                ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY));
            }
            if ui
                .add_enabled(
                    !self.is_loading(),
                    egui::Button::new("Open and verify saved case"),
                )
                .clicked()
            {
                self.errors.clear();
                let locations = Locations {
                    case_directory: self.case_directory.clone().into(),
                    execution_report: self.execution_report.clone().into(),
                    execution_workspace: self.execution_workspace.clone().into(),
                };
                self.start(ui.ctx().clone(), move || vec![inspect(locations)]);
            }
            if self.is_loading() {
                ui.small("Checking saved inputs, outputs, contract and receipts…");
            }
            for error in &self.errors {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }
            if !self.saved.is_empty() {
                egui::ComboBox::from_id_salt("saved-study-selection")
                    .selected_text(&self.saved[self.selected].case_id)
                    .show_ui(ui, |ui| {
                        for (index, saved) in self.saved.iter().enumerate() {
                            ui.selectable_value(&mut self.selected, index, &saved.case_id);
                        }
                    });
                let saved = &self.saved[self.selected];
                if scenario_digest(saved) != current_scenario || saved.variant_id != current_variant
                {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "Saved study belongs to another scenario or arrangement.",
                    );
                }
                ui.label("Saved Core workflow: executed and verified");
                ui.small(format!("{} · {}", saved.scenario_id, saved.variant_id));
                ui.small(format!(
                    "{} verified stage receipts · {} history snapshots",
                    saved.verified_receipt_count,
                    saved.history_snapshot_count.unwrap_or(0)
                ));
                for verdict in &saved.requirement_verdicts {
                    ui.label(format!(
                        "{} · {}",
                        verdict.requirement_id,
                        verdict.status.to_uppercase()
                    ));
                }
                ui.collapsing("Saved identities and scope", |ui| {
                    ui.small(format!("Compiler: {}", saved.compiler_id));
                    ui.small(format!("Semantic profile: {}", saved.semantic_profile));
                    ui.small(format!(
                        "Compiler SHA-256: {}",
                        saved.compiler_executable_sha256
                    ));
                    ui.small(format!("Scenario SHA-256: {}", saved.scenario_sha256));
                    ui.small(format!("Contract: {}", saved.compiled_snapshot_sha256));
                    ui.small(format!("Package: {}", saved.package_sha256));
                    ui.small(&saved.scope_notice);
                    for limitation in &saved.limitations {
                        ui.small(limitation);
                    }
                });
            }
        });
    }
}

fn scenario_digest(saved: &SavedCaseInspection) -> &str {
    saved
        .scenario_sha256
        .strip_prefix("sha256:")
        .unwrap_or(&saved.scenario_sha256)
}

fn inspect(locations: Locations) -> Result<SavedCaseInspection, String> {
    inspect_saved_case(
        &locations.case_directory,
        &locations.execution_report,
        &locations.execution_workspace,
    )
    .map_err(|e| e.to_string())
}

fn load_descriptor(path: &Path) -> Result<Locations, String> {
    let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > 16 * 1024 {
        return Err(
            "Saved-study descriptor must be a regular JSON file no larger than 16 KiB".into(),
        );
    }
    let mut locations: Locations =
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let root = path.parent().unwrap_or(Path::new("."));
    for location in [
        &mut locations.case_directory,
        &mut locations.execution_report,
        &mut locations.execution_workspace,
    ] {
        if location.as_os_str().is_empty()
            || location
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(
                "Saved-study descriptors require relative paths without parent traversal".into(),
            );
        }
        *location = root.join(&*location);
    }
    Ok(locations)
}

impl Drop for ArchivePanel {
    fn drop(&mut self) {
        if let Some(pending) = self.pending.take() {
            let _ = pending.handle.join();
        }
    }
}
