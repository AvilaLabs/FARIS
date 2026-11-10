// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! Native client for the engine's independent winding-pack heating operation.

use eframe::egui;
use faris_engine::{
    design::{
        DesignHeatingOptions, DesignHeatingReport, HeatingProgress, ToolPaths, design_heating,
        render_design_heating_text,
    },
    jobs::Cancellation,
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, TryRecvError},
    },
    time::Duration,
};

use crate::{design_tools::SharedTools, design_transport_panel::CadHistoryContext, settings::Tool};

struct Running {
    cancellation: Cancellation,
    receiver: Receiver<Result<DesignHeatingReport, String>>,
    progress: Arc<Mutex<Option<HeatingProgress>>>,
}

pub struct DesignHeatingPanel {
    pub open: bool,
    design: String,
    check_report: String,
    output: String,
    voxel_r_m: f64,
    voxel_z_m: f64,
    max_histories: u64,
    max_seconds: u64,
    target_error: f64,
    tools: Option<SharedTools>,
    running: Option<Running>,
    result: Option<Result<DesignHeatingReport, String>>,
    cad_history: Option<CadHistoryContext>,
    combine: Option<Receiver<Result<PathBuf, String>>>,
    history_handoff: Option<CadHistoryContext>,
    cad_history_message: Option<String>,
}

impl Default for DesignHeatingPanel {
    fn default() -> Self {
        Self {
            open: false,
            design: String::new(),
            check_report: String::new(),
            output: String::new(),
            voxel_r_m: 0.05,
            voxel_z_m: 0.05,
            max_histories: 50_000_000,
            max_seconds: 3600,
            target_error: 0.05,
            tools: None,
            running: None,
            result: None,
            cad_history: None,
            combine: None,
            history_handoff: None,
            cad_history_message: None,
        }
    }
}

impl DesignHeatingPanel {
    pub fn prefill_cad_history(&mut self, context: CadHistoryContext) -> Result<(), String> {
        if self.is_running() {
            return Err(
                "Finish or cancel the current heating run before switching CAD design.".into(),
            );
        }
        self.prefill(Some(&context.design), Some(&context.check_report));
        self.output = crate::maintenance_run::unused_path(
            &context
                .design
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("faris-heating-run"),
        )
        .display()
        .to_string();
        self.result = None;
        self.history_handoff = None;
        self.cad_history_message = None;
        self.cad_history = Some(context);
        self.open = true;
        Ok(())
    }

    pub fn take_history_handoff(&mut self) -> Option<CadHistoryContext> {
        self.history_handoff.take()
    }

    pub fn use_tools(&mut self, tools: SharedTools) {
        self.tools = Some(tools);
    }

    pub fn prefill(&mut self, design: Option<&std::path::Path>, report: Option<&std::path::Path>) {
        self.cad_history = None;
        self.history_handoff = None;
        self.cad_history_message = None;
        if let Some(path) = design {
            self.design = path.display().to_string();
            if self.output.is_empty() {
                self.output = path
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .join("faris-heating-run")
                    .display()
                    .to_string();
            }
        }
        if let Some(path) = report {
            self.check_report = path.display().to_string();
        }
    }

    fn paths(&self) -> ToolPaths {
        let value = |tool| {
            self.tools
                .as_ref()
                .map(|tools| tools.borrow().value(tool))
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from)
        };
        ToolPaths::from_flags_and_environment(
            None,
            value(Tool::OpenmcPython),
            value(Tool::CrossSections),
        )
        .with_plasma_source_path(value(Tool::PlasmaSource))
    }

    fn start(&mut self, ctx: &egui::Context) -> Result<(), String> {
        let design = PathBuf::from(self.design.trim());
        let check_report = PathBuf::from(self.check_report.trim());
        let output = PathBuf::from(self.output.trim());
        if !design.is_file() || !check_report.is_file() {
            return Err("Choose an existing design file and design check report.".into());
        }
        if output.as_os_str().is_empty() || output.exists() {
            return Err("Choose a new output folder. Existing folders are not overwritten.".into());
        }
        let tools = self.paths();
        let cancellation = Cancellation::default();
        let worker_cancellation = cancellation.clone();
        let progress = Arc::new(Mutex::new(None));
        let worker_progress = progress.clone();
        let repaint = ctx.clone();
        let (sender, receiver) = mpsc::channel();
        let options = DesignHeatingOptions {
            check_report,
            out_dir: output,
            peak_voxel_m: [self.voxel_r_m, self.voxel_z_m],
            particles_per_batch: 20_000,
            max_histories: self.max_histories,
            max_seconds: self.max_seconds,
            target_relative_error: self.target_error,
            progress: Some(Arc::new(move |update| {
                if let Ok(mut current) = worker_progress.lock() {
                    *current = Some(update);
                }
                repaint.request_repaint();
            })),
            seed: 1,
            threads: 4,
            cancellation: worker_cancellation,
        };
        let history_rates = self
            .cad_history
            .as_ref()
            .map(|context| context.history_rates.clone());
        crate::thread::Builder::new()
            .name("faris-design-heating".into())
            .spawn(move || {
                let result = if let Some(history_rates) = history_rates {
                    faris_engine::design::design_heating_with_history_rates(
                        &design,
                        &tools,
                        &options,
                        &history_rates,
                    )
                } else {
                    design_heating(&design, &tools, &options)
                }
                .map_err(|e| e.to_string());
                let _ = sender.send(result);
            })
            .map_err(|e| format!("Could not start the heating worker: {e}"))?;
        self.running = Some(Running {
            cancellation,
            receiver,
            progress,
        });
        self.result = None;
        Ok(())
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = &self.combine {
            match receiver.try_recv() {
                Ok(Ok(path)) => {
                    self.combine = None;
                    if let Some(context) = &self.cad_history {
                        let mut context = context.clone();
                        context.history_rates = path;
                        self.history_handoff = Some(context);
                    }
                    ctx.request_repaint();
                }
                Ok(Err(error)) => {
                    self.combine = None;
                    self.result = Some(Err(error));
                }
                Err(TryRecvError::Disconnected) => {
                    self.combine = None;
                    self.result = Some(Err("CAD history-energy binding worker stopped.".into()));
                }
                Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
            }
        }
        let Some(running) = &self.running else { return };
        match running.receiver.try_recv() {
            Ok(result) => {
                self.running = None;
                let energy_response_ready = result.as_ref().ok().is_some_and(|report| {
                    report
                        .history_energy_response
                        .as_ref()
                        .is_some_and(|response| {
                            response.status == "CALCULATED"
                                && response.target_reached
                                && response.closure_residual.within_3se
                        })
                });
                if energy_response_ready
                    && let (Ok(report), Some(context)) = (&result, self.cad_history.as_ref())
                {
                    let base = context.history_rates.clone();
                    let design_path = context.design.clone();
                    let report_path = PathBuf::from(&report.run_directory).join("report.json");
                    let output =
                        PathBuf::from(&report.run_directory).join("cad-history-energy-rates.json");
                    let (tx, rx) = mpsc::channel();
                    let _ = crate::thread::Builder::new()
                        .name("faris-cad-history-energy-binding".into())
                        .spawn(move || {
                            let result = faris_engine::design::combine_cad_history_energy_rates(
                                &base,
                                &report_path,
                                &design_path,
                                &output,
                            )
                            .map(|_| output);
                            let _ = tx.send(result);
                        });
                    self.combine = Some(rx);
                } else if self.cad_history.is_some() && result.is_ok() {
                    self.cad_history_message = Some("CAD life study paused: the whole-model heating response did not reach CALCULATED with its target reached and energy closure within 3 SE. Next step: complete the coupled-heating energy response before dose-governed history can run.".into());
                }
                self.result = Some(result);
                ctx.request_repaint();
            }
            Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
            Err(TryRecvError::Disconnected) => {
                self.running = None;
                self.result = Some(Err("The heating worker stopped without a result.".into()));
            }
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.is_some() || self.combine.is_some()
    }

    pub fn window(&mut self, ctx: &egui::Context) {
        self.poll(ctx);
        if !self.open {
            return;
        }
        let mut open = self.open;
        egui::Window::new("Design heating")
            .open(&mut open)
            .default_size([760.0, 650.0])
            .vscroll(true)
            .show(ctx, |ui| self.ui(ui));
        self.open = open;
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        ui.weak("Separate all-energy neutron, photon, electron and positron heating in solids explicitly classified as winding pack. The checked CAD conversion supplies geometry identity; sampled ring voxels estimate the peak.");
        if let Some(context) = &self.cad_history {
            ui.label(format!(
                "Whole-life rates for {} · {:.3} MW · CAD SHA-256 {}: {}",
                context.design_id,
                context.fusion_power_mw,
                context.design_sha256,
                context.history_rates.display()
            ));
            ui.weak("This run also scores the complete active CAD model for the operating-energy ledger. The winding-pack peak remains a separate response. The resulting energy response is bound to the same checked CAD source and transport history.");
        }
        ui.add_enabled_ui(!self.is_running(), |ui| {
            egui::Grid::new("design-heating-inputs")
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("Design file");
                    ui.text_edit_singleline(&mut self.design);
                    ui.end_row();
                    ui.label("Design check report");
                    ui.text_edit_singleline(&mut self.check_report);
                    ui.end_row();
                    ui.label("New output folder");
                    ui.text_edit_singleline(&mut self.output);
                    ui.end_row();
                    ui.label("Peak voxel (radial × vertical, m)");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.voxel_r_m)
                                .range(0.01..=1.0)
                                .speed(0.01),
                        );
                        ui.add(
                            egui::DragValue::new(&mut self.voxel_z_m)
                                .range(0.01..=1.0)
                                .speed(0.01),
                        );
                    });
                    ui.end_row();
                    ui.label("History limit");
                    ui.add(
                        egui::DragValue::new(&mut self.max_histories)
                            .range(600_000..=2_000_000_000),
                    );
                    ui.end_row();
                    ui.label("Time limit (s)");
                    ui.add(egui::DragValue::new(&mut self.max_seconds).range(1..=86_400));
                    ui.end_row();
                    ui.label("Mean relative error target");
                    ui.add(egui::DragValue::new(&mut self.target_error).range(0.001..=0.5).speed(0.005));
                    ui.end_row();
                });
            ui.label(format!("Resolved per-solid peak target: {:.1}% (2× mean target); minimum 30 independent batches and VOV ≤ 0.1.", 200.0 * self.target_error));
            if let Some(tools) = &self.tools {
                ui.collapsing("OpenMC tools", |ui| {
                    tools.borrow_mut().ui(ui, false);
                });
            }
        });
        ui.horizontal(|ui| {
            if !self.is_running() {
                if ui.button("Run coupled heating").clicked()
                    && let Err(message) = self.start(ui.ctx())
                {
                    self.result = Some(Err(message));
                }
            } else {
                ui.spinner();
                ui.label(if self.combine.is_some() {
                    "Binding whole-model heating to the CAD history rates"
                } else {
                    "OpenMC coupled heating is running"
                });
                if let Some(update) = self.running.as_ref().and_then(|run| run.progress.lock().ok()?.clone()) {
                    let mean = update.mean_relative_error.map_or_else(|| "n/a".into(), |v| format!("{:.1}%", 100.0 * v));
                    let peak = update.peak_relative_error.map_or_else(|| "n/a".into(), |v| format!("{:.1}%", 100.0 * v));
                    ui.label(format!("{} batches / {} histories · mean error {mean} · peak error {peak} · VOV {}",
                        update.batches, update.histories,
                        update.variance_of_variance.map_or_else(|| "n/a".into(), |v| format!("{v:.3}"))));
                }
                if ui.button("Cancel").clicked()
                    && let Some(job) = &self.running
                {
                    job.cancellation.cancel();
                }
            }
        });
        if let Some(result) = &self.result {
            match result {
                Ok(report) => {
                    ui.separator();
                    ui.monospace(render_design_heating_text(report));
                    ui.label(heating_validation_summary(&report.validation));
                    if let Some(message) = &self.cad_history_message {
                        ui.colored_label(ui.visuals().warn_fg_color, message);
                    }
                }
                Err(message) => {
                    ui.colored_label(ui.visuals().error_fg_color, message);
                }
            }
        }
    }
}

/// The report renderer lists every heating result and its validation tag, but a tag alone does
/// not explain what the benchmark covers or what would improve the evidence. Keep this alongside
/// the result so the state is available without relying on a hover target over a long text block.
fn heating_validation_summary(marks: &faris_model::validation::ValidationMarks) -> String {
    use faris_model::validation::{Quantity, note};

    let mark = marks.mark("power_density_kw_m3", Quantity::Heating);
    let note = note(Quantity::Heating);
    format!(
        "Heating validation {}. Scope: {}. Why: {}. Next step: {}",
        mark.tag(),
        note.scope,
        note.why,
        note.next_step
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use faris_model::validation::{Quantity, ValidationMark, ValidationMarks, ValidationState};

    #[test]
    fn result_validation_summary_preserves_recorded_state_and_catalogue_explanation() {
        let marks = ValidationMarks(std::collections::BTreeMap::from([(
            "power_density_kw_m3".to_owned(),
            ValidationMark {
                quantity: Quantity::Heating,
                state: ValidationState::CodeToCodeOnly,
            },
        )]));
        let text = heating_validation_summary(&marks);
        assert!(text.contains("[code-to-code only: heating]"));
        assert!(text.contains("Scope:"));
        assert!(text.contains("Why:"));
        assert!(text.contains("Next step:"));
        assert!(text.contains(&faris_model::validation::note(Quantity::Heating).next_step));
    }
}
