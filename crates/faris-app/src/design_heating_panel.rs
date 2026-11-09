// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! Native client for the engine's independent winding-pack heating operation.

use eframe::egui;
use faris_engine::{
    design::{
        DesignHeatingOptions, DesignHeatingReport, ToolPaths, design_heating,
        render_design_heating_text,
    },
    jobs::Cancellation,
};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::Duration,
};

use crate::{design_tools::SharedTools, settings::Tool};

struct Running {
    cancellation: Cancellation,
    receiver: Receiver<Result<DesignHeatingReport, String>>,
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
    tools: Option<SharedTools>,
    running: Option<Running>,
    result: Option<Result<DesignHeatingReport, String>>,
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
            tools: None,
            running: None,
            result: None,
        }
    }
}

impl DesignHeatingPanel {
    pub fn use_tools(&mut self, tools: SharedTools) {
        self.tools = Some(tools);
    }

    pub fn prefill(&mut self, design: Option<&std::path::Path>, report: Option<&std::path::Path>) {
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

    fn start(&mut self) -> Result<(), String> {
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
        let (sender, receiver) = mpsc::channel();
        let options = DesignHeatingOptions {
            check_report,
            out_dir: output,
            peak_voxel_m: [self.voxel_r_m, self.voxel_z_m],
            particles_per_batch: 20_000,
            max_histories: self.max_histories,
            max_seconds: self.max_seconds,
            seed: 1,
            threads: 4,
            cancellation: worker_cancellation,
        };
        crate::thread::Builder::new()
            .name("faris-design-heating".into())
            .spawn(move || {
                let result = design_heating(&design, &tools, &options).map_err(|e| e.to_string());
                let _ = sender.send(result);
            })
            .map_err(|e| format!("Could not start the heating worker: {e}"))?;
        self.running = Some(Running {
            cancellation,
            receiver,
        });
        self.result = None;
        Ok(())
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(running) = &self.running else { return };
        match running.receiver.try_recv() {
            Ok(result) => {
                self.running = None;
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
        self.running.is_some()
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
                });
            if let Some(tools) = &self.tools {
                ui.collapsing("OpenMC tools", |ui| {
                    tools.borrow_mut().ui(ui, false);
                });
            }
        });
        ui.horizontal(|ui| {
            if !self.is_running() {
                if ui.button("Run coupled heating").clicked()
                    && let Err(message) = self.start()
                {
                    self.result = Some(Err(message));
                }
            } else {
                ui.spinner();
                ui.label("OpenMC coupled heating is running");
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
