// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! Viewer for dose-governed maintenance results. The view reports the authored task location,
//! selected estimate method and D1S/R2S comparison state alongside the computed duration.

use eframe::egui;
use faris_engine::jobs::Cancellation;
use faris_engine::maintenance::{
    DoseMaintenanceResult, MAX_DOSE_RESULT_BYTES,
    files::{DoseRunConfig, default_event_dose_worker, run_dose_from_files},
    parse_dose_result, prepare_dose_cad,
    spatial_dose::ComparisonStatus,
};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver, TryRecvError},
    },
};

const MAX_RESULT_BYTES: u64 = MAX_DOSE_RESULT_BYTES as u64;

enum LoadState {
    Empty,
    Failed(String),
    Ready(DoseMaintenanceResult),
}

pub struct MaintenanceDosePanel {
    pub open: bool,
    state: LoadState,
    dialog: Option<(DoseField, Receiver<Option<PathBuf>>)>,
    loading: Option<Receiver<Result<DoseMaintenanceResult, String>>>,
    file_name: String,
    selected_design: Option<String>,
    inputs: DoseInputs,
    run: Option<DoseRunState>,
    prepare: Option<DosePrepareState>,
    prepare_config: String,
    prepare_message: Option<String>,
}

#[derive(Default)]
struct DoseInputs {
    designs: String,
    assumptions: String,
    curves: String,
    output: String,
}

struct DoseRunState {
    cancel: Cancellation,
    receiver: Receiver<Result<(PathBuf, DoseMaintenanceResult), String>>,
    progress: Arc<std::sync::Mutex<Option<String>>>,
}

struct DosePrepareState {
    cancel: Cancellation,
    receiver: Receiver<Result<PathBuf, String>>,
}

#[derive(Clone, Copy)]
enum DoseField {
    Designs,
    Assumptions,
    Curves,
    Output,
    PrepareConfig,
    Result,
}

impl Default for MaintenanceDosePanel {
    fn default() -> Self {
        Self {
            open: false,
            state: LoadState::Empty,
            dialog: None,
            loading: None,
            file_name: String::new(),
            selected_design: None,
            inputs: DoseInputs::default(),
            run: None,
            prepare: None,
            prepare_config: String::new(),
            prepare_message: None,
        }
    }
}

pub fn parse_result(bytes: &[u8]) -> Result<DoseMaintenanceResult, String> {
    parse_dose_result(bytes).map_err(|e| format!("not a readable dose maintenance result: {e}"))
}

fn load(path: PathBuf) -> Result<DoseMaintenanceResult, String> {
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > MAX_RESULT_BYTES {
        return Err("dose maintenance result must be a regular file no larger than 16 MiB".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(MAX_RESULT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_RESULT_BYTES {
        return Err("dose maintenance result exceeds the 16 MiB input limit".into());
    }
    parse_result(&bytes)
}

impl MaintenanceDosePanel {
    pub fn is_pending(&self) -> bool {
        self.run.is_some()
            || self.prepare.is_some()
            || self.loading.is_some()
            || self.dialog.is_some()
    }

    pub fn is_running(&self) -> bool {
        self.run.is_some() || self.prepare.is_some()
    }

    pub fn window(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        if let Some((field, receiver)) = &self.dialog {
            match receiver.try_recv() {
                Ok(path) => {
                    let field = *field;
                    self.dialog = None;
                    if let Some(path) = path {
                        match field {
                            DoseField::Result => {
                                let name = path.file_name().map_or_else(
                                    || path.display().to_string(),
                                    |n| n.to_string_lossy().into_owned(),
                                );
                                let (tx, rx) = mpsc::channel();
                                std::thread::spawn(move || {
                                    let _ = tx.send(load(path));
                                });
                                self.file_name = name;
                                self.loading = Some(rx);
                            }
                            other => {
                                let value = path.display().to_string();
                                match other {
                                    DoseField::Designs => self.inputs.designs = value,
                                    DoseField::Assumptions => self.inputs.assumptions = value,
                                    DoseField::Curves => self.inputs.curves = value,
                                    DoseField::Output => self.inputs.output = value,
                                    DoseField::PrepareConfig => self.prepare_config = value,
                                    DoseField::Result => unreachable!(),
                                }
                            }
                        }
                    }
                }
                Err(TryRecvError::Disconnected) => self.dialog = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(receiver) = &self.loading {
            match receiver.try_recv() {
                Ok(Ok(result)) => {
                    self.selected_design = result.designs.keys().next().cloned();
                    self.state = LoadState::Ready(result);
                    self.loading = None;
                }
                Ok(Err(error)) => {
                    self.state = LoadState::Failed(error);
                    self.loading = None;
                }
                Err(TryRecvError::Disconnected) => {
                    self.state = LoadState::Failed("dose result reader stopped".into());
                    self.loading = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(run) = &self.run {
            match run.receiver.try_recv() {
                Ok(Ok((output, result))) => {
                    self.file_name = output.display().to_string();
                    self.selected_design = result.designs.keys().next().cloned();
                    self.state = LoadState::Ready(result);
                    self.run = None;
                }
                Ok(Err(error)) => {
                    self.state = LoadState::Failed(error);
                    self.run = None;
                }
                Err(TryRecvError::Disconnected) => {
                    self.state = LoadState::Failed("dose maintenance worker stopped".into());
                    self.run = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(prepare) = &self.prepare {
            match prepare.receiver.try_recv() {
                Ok(Ok(path)) => {
                    self.prepare_message =
                        Some(format!("Prepared physics manifest: {}", path.display()));
                    self.prepare = None;
                }
                Ok(Err(error)) => {
                    self.state = LoadState::Failed(error);
                    self.prepare = None;
                }
                Err(TryRecvError::Disconnected) => {
                    self.state = LoadState::Failed("CAD dose preparation worker stopped".into());
                    self.prepare = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        let mut open = self.open;
        egui::Window::new("Dose-governed maintenance result").open(&mut open).default_width(780.0).show(ctx, |ui| {
            if !crate::browser::IS_BROWSER {
                ui.collapsing("Run dose-governed maintenance", |ui| {
                    ui.label("Prepare dose physics from a passing checked CAD Design and ImportReport. The preparation config authors the solid-to-history-component map and spatial inputs; FARIS clones materials per solid and builds the full-geometry model.");
                    path_row(ui, "CAD dose preparation config", &mut self.prepare_config, DoseField::PrepareConfig, &mut self.dialog);
                    ui.horizontal(|ui| {
                        if self.prepare.is_none() && ui.button("Prepare CAD dose physics").clicked() {
                            match start_dose_prepare(&self.prepare_config) {
                                Ok(state) => { self.prepare = Some(state); self.prepare_message = None; }
                                Err(error) => self.state = LoadState::Failed(error),
                            }
                        }
                        if let Some(prepare) = &self.prepare {
                            if ui.button("Cancel preparation").clicked() { prepare.cancel.cancel(); }
                            ui.spinner();
                        }
                    });
                    if let Some(message) = &self.prepare_message { ui.label(message); }
                    ui.label("Provide event-specific D1S/R2S spatial curves. FARIS computes the history coupling and duration; this workflow does not run a general D1S/R2S transport solver.");
                    path_row(ui, "Designs", &mut self.inputs.designs, DoseField::Designs, &mut self.dialog);
                    path_row(ui, "Assumptions", &mut self.inputs.assumptions, DoseField::Assumptions, &mut self.dialog);
                    path_row(ui, "Optional spatial dose curve cache", &mut self.inputs.curves, DoseField::Curves, &mut self.dialog);
                    path_row(ui, "New result file", &mut self.inputs.output, DoseField::Output, &mut self.dialog);
                    ui.horizontal(|ui| {
                        if self.run.is_none() && ui.button("Run bounded dose coupling").clicked() {
                            match start_dose_run(&self.inputs) {
                                Ok(run) => self.run = Some(run),
                                Err(error) => self.state = LoadState::Failed(error),
                            }
                        }
                        if let Some(run) = &self.run { if ui.button("Cancel").clicked() { run.cancel.cancel(); } ui.spinner(); ui.label(run.progress.lock().ok().and_then(|p| p.clone()).unwrap_or_else(|| "Starting dose coupling".into())); }
                    });
                });
                ui.separator();
            }
            ui.horizontal(|ui| {
                if ui.button("Open result…").clicked() && self.dialog.is_none() {
                    let (tx, rx) = mpsc::channel(); self.dialog = Some((DoseField::Result, rx));
                    std::thread::spawn(move || { let path = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file(); let _ = tx.send(path); });
                }
                if !self.file_name.is_empty() { ui.label(&self.file_name); }
                if self.loading.is_some() { ui.spinner(); ui.label("Reading result"); }
            });
            match &self.state {
                LoadState::Empty => { ui.label("Open a faris-maintenance-result/v0.2 file to inspect authored spatial-dose scenarios."); },
                LoadState::Failed(error) => { ui.colored_label(egui::Color32::from_rgb(180, 45, 45), error); },
                LoadState::Ready(result) => show_result(ui, result, &mut self.selected_design),
            }
        });
        self.open = open;
    }
}

fn path_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    field: DoseField,
    pending: &mut Option<(DoseField, Receiver<Option<PathBuf>>)>,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::TextEdit::singleline(value).desired_width(420.0));
        if ui.button("Browse…").clicked() && pending.is_none() {
            let (tx, rx) = mpsc::channel();
            *pending = Some((field, rx));
            std::thread::spawn(move || {
                let dialog = rfd::FileDialog::new().add_filter("JSON", &["json"]);
                let path = if matches!(field, DoseField::Output) {
                    dialog.save_file()
                } else {
                    dialog.pick_file()
                };
                let _ = tx.send(path);
            });
        }
    });
}

fn start_dose_run(inputs: &DoseInputs) -> Result<DoseRunState, String> {
    let designs = required_path("designs", &inputs.designs)?;
    let assumptions = required_path("assumptions", &inputs.assumptions)?;
    let curves = optional_path(&inputs.curves);
    let output = required_path("new result file", &inputs.output)?;
    if output.exists() {
        return Err(format!("refusing to overwrite {}", output.display()));
    }
    let worker_script = default_event_dose_worker().ok_or(
        "OpenMC event-dose worker script was not found in the application package or source tree",
    )?;
    let config = DoseRunConfig {
        designs,
        assumptions,
        spatial_dose_curves: curves,
        python: PathBuf::from("python3"),
        worker_script,
        work_dir: output.with_extension("dose-work"),
    };
    let cancel = Cancellation::default();
    let worker_cancel = cancel.clone();
    let progress = Arc::new(std::sync::Mutex::new(None));
    let worker_progress = progress.clone();
    let (tx, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = run_dose_from_files(
            &config,
            &worker_cancel,
            Arc::new(move |p| {
                if let Ok(mut current) = worker_progress.lock() {
                    *current = Some(p.line());
                }
            }),
        )
        .and_then(|result| {
            let mut bytes = serde_json::to_vec_pretty(&result).map_err(|e| e.to_string())?;
            if bytes.len() >= MAX_DOSE_RESULT_BYTES {
                return Err("dose maintenance result exceeds the 16 MiB output limit".into());
            }
            bytes.push(b'\n');
            faris_engine::atomic_file::write_new(&output, &bytes)
                .map_err(|e| format!("cannot write {}: {e}", output.display()))?;
            Ok((output, result))
        });
        let _ = tx.send(result);
    });
    Ok(DoseRunState {
        cancel,
        receiver,
        progress,
    })
}

fn start_dose_prepare(config: &str) -> Result<DosePrepareState, String> {
    let config = required_path("dose CAD preparation config", config)?;
    let worker = default_event_dose_worker().ok_or(
        "OpenMC event-dose worker script was not found in the application package or source tree",
    )?;
    let cancel = Cancellation::default();
    let worker_cancel = cancel.clone();
    let python = PathBuf::from("python3");
    let (tx, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = prepare_dose_cad(&config, &python, &worker, &worker_cancel);
        let _ = tx.send(result);
    });
    Ok(DosePrepareState { cancel, receiver })
}

fn required_path(label: &str, value: &str) -> Result<PathBuf, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(format!("choose {label}"))
    } else {
        Ok(Path::new(trimmed).to_path_buf())
    }
}

fn optional_path(value: &str) -> Option<PathBuf> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| Path::new(trimmed).to_path_buf())
}

fn show_result(ui: &mut egui::Ui, result: &DoseMaintenanceResult, selected: &mut Option<String>) {
    ui.label("Criterion: ICRP-116 AP photon effective dose rate at the authored task location (Sv/h). Missing curves are computed with the bounded OpenMC event worker when a design supplies a dose-physics adapter; heterogeneous histories use R2S with D1S marked NOT_EVALUATED.");
    let names: Vec<_> = result.designs.keys().cloned().collect();
    if selected
        .as_ref()
        .is_none_or(|s| !result.designs.contains_key(s))
    {
        *selected = names.first().cloned();
    }
    if names.len() > 1 {
        egui::ComboBox::from_label("Design")
            .selected_text(selected.as_deref().unwrap_or("Choose design"))
            .show_ui(ui, |ui| {
                for name in names {
                    ui.selectable_value(selected, Some(name.clone()), name);
                }
            });
    }
    let Some(name) = selected.as_ref() else {
        ui.label("No designs in this result.");
        return;
    };
    let Some(design) = result.designs.get(name) else {
        return;
    };
    ui.heading(name);
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "Fixed downtime: {:.3} days",
            design.fixed.total_replacement_downtime_s / 86_400.0
        ));
        ui.label(format!("Computed status: {:?}", design.computed.status));
        if let Some(summary) = &design.computed.summary {
            ui.label(format!(
                "Computed: {:.3} days",
                summary.total_replacement_downtime_s / 86_400.0
            ));
        }
        ui.label(format!(
            "Curve comparison: {:?}",
            design.computed.validation
        ));
    });
    if let Some(ne) = &design.computed.not_evaluated {
        ui.colored_label(
            egui::Color32::from_rgb(165, 90, 20),
            format!("NOT_EVALUATED: {} Next step: {}", ne.reason, ne.next_step),
        );
    }
    egui::ScrollArea::both().max_height(460.0).show(ui, |ui| {
        egui::Grid::new("dose-maintenance-events")
            .striped(true)
            .show(ui, |ui| {
                for heading in [
                    "Class / replacement",
                    "Task location",
                    "Limit source",
                    "Selected method",
                    "Dose limit (Sv/h)",
                    "Cooldown / work",
                    "Duration used",
                    "D1S / R2S central ratio",
                    "Comparison",
                    "Crossing uncertainty",
                ] {
                    ui.strong(heading);
                }
                ui.end_row();
                for iteration in &design.computed.iterations {
                    for event in &iteration.events {
                        ui.label(format!(
                            "{} / {} #{}",
                            event.class, event.component, event.replacement_k
                        ));
                        ui.label(format!(
                            "{} · {}…",
                            event.location_id,
                            &event.location_sha256[..8.min(event.location_sha256.len())]
                        ));
                        ui.label(&event.dose_limit_source);
                        let (method, ratio, comparison) = event
                            .curve
                            .as_ref()
                            .and_then(|curve| {
                                curve.points.first().map(|point| {
                                    (
                                        format!("{:?}", point.selected_method),
                                        point
                                            .d1s_over_r2s
                                            .map_or("undefined".into(), |v| format!("{v:.4}")),
                                        format!("{:?}", curve.comparison),
                                    )
                                })
                            })
                            .unwrap_or_else(|| ("—".into(), "—".into(), "NOT_EVALUATED".into()));
                        ui.label(method);
                        ui.label(format!("{:.3e}", event.dose_limit_sv_h));
                        ui.label(format!(
                            "{} + {:.3} d",
                            event
                                .cooldown_s
                                .map_or("—".into(), |s| format!("{:.3}", s / 86_400.0)),
                            event.work_s / 86_400.0
                        ));
                        ui.label(format!("{:.3} d", event.duration_used_s / 86_400.0));
                        ui.label(ratio);
                        let color = if comparison == "INCONCLUSIVE_COVARIANCE" {
                            egui::Color32::from_rgb(165, 90, 20)
                        } else if comparison == "OUTSIDE_BAND" || event.not_evaluated.is_some() {
                            egui::Color32::from_rgb(180, 45, 45)
                        } else {
                            ui.visuals().text_color()
                        };
                        ui.colored_label(color, comparison);
                        ui.label(
                            event
                                .curve
                                .as_ref()
                                .and_then(|c| c.points.first())
                                .map_or_else(
                                    || "NOT_EVALUATED".into(),
                                    |p| p.crossing_uncertainty.clone(),
                                ),
                        );
                        ui.end_row();
                    }
                }
            });
    });
    for iteration in &design.computed.iterations {
        for event in &iteration.events {
            if let Some(curve) = &event.curve {
                ui.collapsing(
                    format!(
                        "{} #{} spatial dose points",
                        event.component, event.replacement_k
                    ),
                    |ui| {
                        ui.label(&curve.validation_scope);
                        ui.label(format!(
                            "Normalization: {}",
                            curve
                                .points
                                .first()
                                .map_or("not recorded", |p| p.source_normalization.as_str())
                        ));
                        ui.label(format!(
                            "History SHA-256: {} · irradiation schedule SHA-256: {}",
                            curve.history_sha256, curve.irradiation_schedule_sha256
                        ));
                        ui.label(format!(
                            "Design input SHA-256 values: {}",
                            curve
                                .design_input_sha256
                                .iter()
                                .map(|(k, v)| format!("{k}={v}"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                        egui::Grid::new(format!(
                            "dose-points-{}-{}-{}",
                            name, event.component, event.replacement_k
                        ))
                        .striped(true)
                        .show(ui, |ui| {
                            for heading in [
                                "Cooling time (days)",
                                "D1S (Sv/h)",
                                "R2S (Sv/h)",
                                "D1S/R2S",
                                "D1S SE",
                                "R2S SE",
                                "Ratio SE",
                                "Comparison",
                                "Crossing uncertainty",
                            ] {
                                ui.strong(heading);
                            }
                            ui.end_row();
                            for point in &curve.points {
                                ui.label(format!("{:.4}", point.cooling_s / 86_400.0));
                                ui.label(
                                    point
                                        .d1s_sv_h
                                        .map_or("NOT_EVALUATED".into(), |v| format!("{v:.6e}")),
                                );
                                ui.label(format!("{:.6e}", point.r2s_sv_h));
                                ui.label(
                                    point
                                        .d1s_over_r2s
                                        .map_or("undefined".into(), |r| format!("{r:.5}")),
                                );
                                ui.label(
                                    point
                                        .d1s_standard_error_sv_h
                                        .map_or("not available".into(), |se| format!("{se:.3e}")),
                                );
                                ui.label(
                                    point
                                        .r2s_standard_error_sv_h
                                        .map_or("not available".into(), |se| format!("{se:.3e}")),
                                );
                                ui.label(
                                    point
                                        .ratio_standard_error
                                        .map_or("not available".into(), |se| format!("{se:.3e}")),
                                );
                                let color = if point.comparison
                                    == ComparisonStatus::InconclusiveCovariance
                                {
                                    egui::Color32::from_rgb(165, 90, 20)
                                } else if point.comparison == ComparisonStatus::OutsideBand {
                                    egui::Color32::from_rgb(180, 45, 45)
                                } else {
                                    ui.visuals().text_color()
                                };
                                ui.colored_label(color, format!("{:?}", point.comparison));
                                ui.label(&point.crossing_uncertainty);
                                ui.end_row();
                                if let Some(reason) = &point.d1s_not_evaluated {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(165, 90, 20),
                                        format!("D1S NOT_EVALUATED: {reason}"),
                                    );
                                    ui.end_row();
                                }
                            }
                        });
                    },
                );
            }
        }
    }
    if design.computed.validation == ComparisonStatus::InconclusiveCovariance {
        ui.colored_label(egui::Color32::from_rgb(165, 90, 20), "D1S/R2S central values are in band, but comparison covariance is unknown. Physical validation remains inconclusive; pointwise standard errors are not cooling-time uncertainty.");
    }
}
