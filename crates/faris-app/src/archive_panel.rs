//! Reopening saved studies revalidates their identities on a worker thread.

use crate::badge::{self, Kind};
use eframe::egui;
use faris_engine::{
    case_archive::{SavedCaseInspection, inspect_saved_case},
    history::HistoryResult,
    jobs::Cancellation,
};
use serde::Deserialize;
use std::{
    io::Read,
    path::{Component, Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
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
    cancellation: Cancellation,
}

/// How long the saved-evidence worker waits for a package's Core evidence to
/// finish unpacking. The package expands 823 MB; a debug build takes about
/// 20 s on the reference laptop, so this leaves room for slow disks.
const MATERIALIZATION_LIMIT: Duration = Duration::from_secs(600);

#[derive(Default)]
pub struct ArchivePanel {
    queued_descriptors: Vec<PathBuf>,
    ready_marker: Option<PathBuf>,
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
    pub fn queue_descriptors(
        &mut self,
        paths: Vec<PathBuf>,
        ready_marker: Option<PathBuf>,
    ) -> Result<(), String> {
        if paths.len() > 16 {
            return Err("At most 16 saved study descriptors can be opened together".into());
        }
        if paths.is_empty() && ready_marker.is_some() {
            return Err("A delivery marker requires at least one saved study".into());
        }
        self.queued_descriptors = paths;
        self.ready_marker = ready_marker;
        Ok(())
    }

    pub fn has_saved(&self) -> bool {
        !self.saved.is_empty()
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
            let marker = self.ready_marker.take();
            self.start(ctx.clone(), move |cancellation| {
                if let Some(path) = marker
                    && let Err(error) =
                        wait_for_materialization(&path, &cancellation, MATERIALIZATION_LIMIT)
                {
                    return vec![Err(error)];
                }
                descriptors
                    .iter()
                    .map(|path| {
                        if cancellation.is_cancelled() {
                            return Err("Saved-study reopening cancelled".into());
                        }
                        // Delivery readiness alone never establishes evidence.
                        load_descriptor(path).and_then(inspect)
                    })
                    .collect()
            });
        }
        let Some(pending) = &self.pending else {
            return;
        };
        let results = match pending.receiver.try_recv() {
            Ok(value) => value,
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(100));
                return;
            }
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
        work: impl FnOnce(Cancellation) -> Vec<Result<SavedCaseInspection, String>> + Send + 'static,
    ) {
        let (sender, receiver) = mpsc::channel();
        let cancellation = Cancellation::default();
        let worker_cancellation = cancellation.clone();
        match std::thread::Builder::new()
            .name("faris-reopen-saved-study".into())
            .spawn(move || {
                let _ = sender.send(work(worker_cancellation));
                ctx.request_repaint();
            }) {
            Ok(handle) => {
                self.pending = Some(Pending {
                    handle,
                    receiver,
                    cancellation,
                });
            }
            Err(error) => self.errors.push(error.to_string()),
        }
    }

    pub fn controls(&mut self, ui: &mut egui::Ui, current_scenario: &str, current_variant: &str) {
        if self.is_loading() {
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.small("Preparing/checking saved study evidence…");
            });
        }
        if !self.errors.is_empty() {
            ui.horizontal_wrapped(|ui| {
                badge::badge(
                    ui,
                    Kind::Failed,
                    "saved study unavailable",
                    "A saved study could not be opened or verified. Expand Reopen saved study for diagnostics.",
                );
            });
        }
        // Keyed by the saved count so the section opens once background reopening finishes.
        egui::CollapsingHeader::new("Reopen saved study").id_salt(("reopen-saved-study", self.saved.len())).default_open(!self.saved.is_empty()).show(ui, |ui| {
            if self.saved.is_empty() && !self.is_loading() {
                ui.small("No saved study is open. Saved studies open from a descriptor file given on the command line; to point at the files yourself, use the section below.");
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
                    badge::badge(
                        ui,
                        Kind::Partial,
                        "other scenario or arrangement",
                        "Saved study belongs to another scenario or arrangement than the one currently shown. Its receipts are valid only for their own identities.",
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    ui.label("Saved Core workflow");
                    badge::badge(
                        ui,
                        Kind::Checked,
                        "executed and verified",
                        "Inputs, outputs, contract and stage receipts of the saved case were re-verified on opening. This is not a scientific verdict.",
                    );
                });
                ui.small(format!("{} · {}", saved.scenario_id, saved.variant_id));
                ui.small(format!(
                    "{} verified stage receipts · {} history snapshots",
                    saved.verified_receipt_count,
                    saved.history_snapshot_count.unwrap_or(0)
                ));
                for verdict in &saved.requirement_verdicts {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(&verdict.requirement_id);
                        let status = verdict.status.to_uppercase();
                        badge::badge(
                            ui,
                            crate::study_panel::verdict_kind(&status),
                            &status,
                            "Verdict recorded in the verified saved evidence, within the scope and limitations listed under Saved identities and scope.",
                        );
                    });
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
            ui.collapsing("Advanced: enter paths by hand", |ui| {
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
                    self.start(ui.ctx().clone(), move |_| vec![inspect(locations)]);
                }
                if self.is_loading() {
                    ui.small("Checking saved inputs, outputs, contract and receipts…");
                }
            });
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Materialization {
    schema_version: String,
    status: String,
    #[serde(default)]
    error: Option<String>,
}

/// The launcher owns this small completion signal in a private directory.
/// Its success permits inspection; the case/receipt hashes remain authoritative.
fn wait_for_materialization(
    path: &Path,
    cancellation: &Cancellation,
    timeout: Duration,
) -> Result<(), String> {
    let started = Instant::now();
    loop {
        if cancellation.is_cancelled() {
            return Err("Saved-study materialization wait cancelled".into());
        }
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => {
                if !metadata.is_file() || metadata.len() > 4096 {
                    return Err(
                        "Saved-study delivery marker must be a regular file no larger than 4 KiB"
                            .into(),
                    );
                }
                let mut bytes = Vec::new();
                std::fs::File::open(path)
                    .map_err(|error| format!("Cannot open saved-study delivery marker: {error}"))?
                    .take(4097)
                    .read_to_end(&mut bytes)
                    .map_err(|error| format!("Cannot read saved-study delivery marker: {error}"))?;
                if bytes.len() > 4096 {
                    return Err("Saved-study delivery marker grew beyond 4 KiB".into());
                }
                let marker: Materialization = serde_json::from_slice(&bytes)
                    .map_err(|error| format!("Invalid saved-study delivery marker: {error}"))?;
                if marker.schema_version != "faris-recorded-materialization/v0.1"
                    || marker
                        .error
                        .as_ref()
                        .is_some_and(|error| error.len() > 1024)
                {
                    return Err(
                        "Unsupported saved-study delivery marker or oversized diagnostic".into(),
                    );
                }
                return match (marker.status.as_str(), marker.error) {
                    ("COMPLETE", None) => Ok(()),
                    ("FAILED", Some(error)) if !error.trim().is_empty() => {
                        Err(format!("Saved-study materialization failed: {error}"))
                    }
                    _ => Err("Invalid saved-study delivery state".into()),
                };
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Cannot check saved-study delivery marker: {error}")),
        }
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err("Core evidence did not finish unpacking within 10 minutes, so the saved receipts were not checked. Why: unpacking the evidence archives took longer than this limit, usually because the temporary folder is on a slow or nearly full disk. Next step: free temporary space and reopen FARIS. Everything except the saved receipts remains available.".into());
        }
        std::thread::sleep(remaining.min(Duration::from_millis(20)));
    }
}

impl Drop for ArchivePanel {
    fn drop(&mut self) {
        if let Some(pending) = self.pending.take() {
            pending.cancellation.cancel();
            let _ = pending.handle.join();
        }
    }
}

#[cfg(test)]
mod delivery_tests {
    use super::*;

    #[test]
    fn failed_or_invalid_delivery_never_opens_a_saved_case() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("materialization.json");
        for (value, message) in [
            (
                r#"{"schema_version":"faris-recorded-materialization/v0.1","status":"FAILED","error":"archive hash mismatch"}"#,
                "archive hash mismatch",
            ),
            (
                r#"{"schema_version":"faris-recorded-materialization/v0.1","status":"COMPLETE","error":"partial output"}"#,
                "Invalid saved-study delivery state",
            ),
            (
                r#"{"schema_version":"unknown","status":"COMPLETE"}"#,
                "Unsupported",
            ),
        ] {
            std::fs::write(&marker, value).unwrap();
            let error =
                wait_for_materialization(&marker, &Cancellation::default(), Duration::from_secs(1))
                    .unwrap_err();
            assert!(error.contains(message), "{error}");
        }
        std::fs::write(&marker, vec![b' '; 4097]).unwrap();
        assert!(
            wait_for_materialization(&marker, &Cancellation::default(), Duration::from_secs(1))
                .is_err()
        );
    }

    #[test]
    fn delivery_wait_cancels_without_the_marker_and_times_out() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("pending.json");
        let cancellation = Cancellation::default();
        let worker_cancel = cancellation.clone();
        let worker_marker = marker.clone();
        let worker = std::thread::spawn(move || {
            wait_for_materialization(&worker_marker, &worker_cancel, Duration::from_secs(60))
        });
        std::thread::sleep(Duration::from_millis(10));
        let started = Instant::now();
        cancellation.cancel();
        assert!(worker.join().unwrap().unwrap_err().contains("cancelled"));
        assert!(started.elapsed() < Duration::from_millis(200));
        let started = Instant::now();
        assert!(
            wait_for_materialization(&marker, &Cancellation::default(), Duration::from_millis(10))
                .is_err()
        );
        assert!(started.elapsed() < Duration::from_millis(200));
    }

    #[test]
    fn complete_delivery_still_requires_actual_case_verification() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("complete.json");
        std::fs::write(
            &marker,
            r#"{"schema_version":"faris-recorded-materialization/v0.1","status":"COMPLETE"}"#,
        )
        .unwrap();
        wait_for_materialization(&marker, &Cancellation::default(), Duration::from_secs(1))
            .unwrap();
        let result = inspect(Locations {
            case_directory: temp.path().join("missing-case"),
            execution_report: temp.path().join("missing-report.json"),
            execution_workspace: temp.path().join("missing-workspace"),
        });
        assert!(
            result.is_err(),
            "Delivery readiness must not invent Core evidence"
        );
    }
}
