//! Native presentation of the shared Rust history engine. No UI-owned physics.

use eframe::egui;
use faris_engine::{
    comparison::{
        HistorySensitivityGrid, HistorySensitivityResult, run_history_sensitivity_cancellable,
    },
    history::{
        HistoryResult, HistorySnapshot, JULIAN_YEAR_SECONDS, TransportDrivingRates,
        run_operating_history_cancellable,
    },
    jobs::Cancellation,
    reactor::ReactorRun,
};
use faris_model::history::OperatingHistoryAssumptions;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
};

type Histories = BTreeMap<String, HistoryResult>;
struct Pending {
    handle: Option<std::thread::JoinHandle<()>>,
    cancellation: Cancellation,
    receiver: Receiver<Result<Histories, String>>,
    assumptions: OperatingHistoryAssumptions,
}

struct PendingSensitivity {
    handle: Option<std::thread::JoinHandle<()>>,
    cancellation: Cancellation,
    receiver: Receiver<Result<HistorySensitivityResult, String>>,
    identity: String,
    assumptions: OperatingHistoryAssumptions,
    transport_sha256: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Plot {
    #[default]
    Inventory,
    Electricity,
    MagnetExposure,
    FullPower,
}

pub struct HistoryPanel {
    pub assumptions: Option<OperatingHistoryAssumptions>,
    results: Histories,
    pending: Option<Pending>,
    error: Option<String>,
    requested: bool,
    edited: bool,
    plot: Plot,
    names: BTreeMap<String, String>,
    bound_transport: BTreeMap<String, String>,
    rates: BTreeMap<String, TransportDrivingRates>,
    presets: Vec<(String, OperatingHistoryAssumptions)>,
    preset_index: usize,
    revision: u64,
    pending_sensitivity: Option<PendingSensitivity>,
    sensitivities: BTreeMap<
        String,
        (
            OperatingHistoryAssumptions,
            String,
            HistorySensitivityResult,
        ),
    >,
}

pub fn key(scenario: &str, variant: &str) -> String {
    format!("{scenario}::{variant}")
}

impl HistoryPanel {
    pub fn new(path: Option<PathBuf>) -> Result<Self, String> {
        let assumptions = path
            .map(|p| -> Result<OperatingHistoryAssumptions, String> {
                let value: OperatingHistoryAssumptions = serde_json::from_slice(
                    &faris_engine::reactor::read_json_bytes(&p).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
                value.validate()?;
                Ok(value)
            })
            .transpose()?;
        let mut presets = Vec::new();
        if let Some(a) = &assumptions {
            presets.push(("Loaded assumptions".into(), a.clone()));
            for (name, bytes) in [
                (
                    "Baseline authored scenario",
                    include_bytes!(
                        "../../../scenarios/arc-inspired/demo-operating-assumptions.json"
                    )
                    .as_slice(),
                ),
                (
                    "Replacement-event demonstration",
                    include_bytes!("../../../scenarios/arc-inspired/demo-event-assumptions.json")
                        .as_slice(),
                ),
            ] {
                let a: OperatingHistoryAssumptions =
                    serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
                a.validate()?;
                presets.push((name.into(), a));
            }
        }
        Ok(Self {
            requested: assumptions.is_some(),
            assumptions,
            results: BTreeMap::new(),
            pending: None,
            error: None,
            edited: false,
            plot: Plot::default(),
            names: BTreeMap::new(),
            bound_transport: BTreeMap::new(),
            rates: BTreeMap::new(),
            presets,
            preset_index: 0,
            revision: 0,
            pending_sensitivity: None,
            sensitivities: BTreeMap::new(),
        })
    }

    pub fn result(&self, scenario: &str, variant: &str) -> Option<&HistoryResult> {
        self.results.get(&key(scenario, variant))
    }
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Accept a revalidated saved history only for these exact loaded transport
    /// inputs and current assumptions. Reopening never silently changes a study.
    pub fn cache_saved_history(
        &mut self,
        history: HistoryResult,
        run: &ReactorRun,
        reference_power_mw: f64,
    ) -> Result<(), String> {
        let normalized = run
            .normalized
            .as_ref()
            .ok_or("Loaded transport is not accepted")?;
        let raw = run
            .raw_artifact_sha256
            .as_deref()
            .ok_or("Loaded transport has no artifact identity")?;
        let rates = TransportDrivingRates::from_normalized(normalized, reference_power_mw, raw)?;
        if history.driving_rates != rates || self.assumptions.as_ref() != Some(&history.assumptions)
        {
            return Err(
                "Saved history belongs to other transport inputs or operating assumptions".into(),
            );
        }
        self.results
            .insert(key(&run.scenario_sha256, &run.variant_id), history);
        self.revision += 1;
        Ok(())
    }
    pub fn snapshot(&self, scenario: &str, variant: &str, time_s: f64) -> Option<&HistorySnapshot> {
        let history = self.result(scenario, variant)?;
        let i = history
            .snapshots
            .partition_point(|s| s.time_s <= time_s)
            .saturating_sub(1);
        history.snapshots.get(i)
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        if let Some(pending) = &self.pending_sensitivity {
            let message = match pending.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(
                    "Sensitivity worker ended without a completed grid.".into(),
                )),
            };
            if let Some(message) = message {
                let mut pending = self.pending_sensitivity.take().expect("worker exists");
                if let Some(handle) = pending.handle.take() {
                    let _ = handle.join();
                }
                match message {
                    Ok(result) => {
                        self.sensitivities.insert(
                            pending.identity,
                            (pending.assumptions, pending.transport_sha256, result),
                        );
                    }
                    Err(error) => self.error = Some(error),
                }
                ctx.request_repaint();
            }
        }
        let Some(pending) = &self.pending else {
            return;
        };
        let result = match pending.receiver.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                Err("History worker ended without a completed result.".into())
            }
        };
        let mut pending = self.pending.take().expect("worker exists");
        if let Some(handle) = pending.handle.take() {
            let _ = handle.join();
        }
        match result {
            Ok(results) => {
                self.results = results;
                self.revision += 1;
                self.edited = self.assumptions.as_ref() != Some(&pending.assumptions);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        ctx.request_repaint();
    }

    /// Only clones the compact driving rates when calculation is requested.
    pub fn update_inputs(&mut self, ctx: &egui::Context, inputs: &[(&ReactorRun, f64, String)]) {
        let identity: BTreeMap<_, _> = inputs
            .iter()
            .filter_map(|(run, _, _)| {
                run.raw_artifact_sha256
                    .as_ref()
                    .map(|hash| (key(&run.scenario_sha256, &run.variant_id), hash.clone()))
            })
            .collect();
        if identity != self.bound_transport && self.pending.is_none() {
            self.requested = true;
        }
        if !self.requested || self.pending.is_some() || inputs.is_empty() {
            return;
        }
        let Some(assumptions) = self.assumptions.clone() else {
            return;
        };
        let prepared: Result<Vec<_>, String> = inputs
            .iter()
            .map(|(run, power, name)| {
                let normalized = run
                    .normalized
                    .as_ref()
                    .ok_or("A transport record has no accepted result.")?;
                let raw = run
                    .raw_artifact_sha256
                    .as_deref()
                    .ok_or("A transport record has no raw identity.")?;
                let rates = TransportDrivingRates::from_normalized(normalized, *power, raw)?;
                Ok((
                    key(&run.scenario_sha256, &run.variant_id),
                    name.clone(),
                    rates,
                ))
            })
            .collect();
        let prepared = match prepared {
            Ok(p) => p,
            Err(e) => {
                self.error = Some(e);
                self.requested = false;
                return;
            }
        };
        self.names = prepared
            .iter()
            .map(|(id, name, _)| (id.clone(), name.clone()))
            .collect();
        self.rates = prepared
            .iter()
            .map(|(id, _, rates)| (id.clone(), rates.clone()))
            .collect();
        self.bound_transport = identity;
        self.requested = false;
        // A reopened case supplies the exact history produced by the declared
        // Rust stage. Check every current input before reusing the saved result.
        if prepared.iter().all(|(id, _, rates)| {
            self.results
                .get(id)
                .is_some_and(|r| r.assumptions == assumptions && r.driving_rates == *rates)
        }) {
            self.edited = false;
            self.error = None;
            return;
        }
        let cancellation = Cancellation::default();
        let worker_cancellation = cancellation.clone();
        let (sender, receiver) = mpsc::channel();
        let context = ctx.clone();
        let spawn = std::thread::Builder::new()
            .name("faris-operating-history".into())
            .spawn(move || {
                let result = (|| {
                    let mut results = BTreeMap::new();
                    for (id, _, rates) in prepared {
                        let history = run_operating_history_cancellable(
                            &assumptions,
                            &rates,
                            &worker_cancellation,
                        )?;
                        results.insert(id, history);
                    }
                    Ok(results)
                })();
                let _ = sender.send(result);
                context.request_repaint();
            });
        match spawn {
            Ok(handle) => {
                self.pending = Some(Pending {
                    handle: Some(handle),
                    cancellation,
                    receiver,
                    assumptions: self.assumptions.clone().expect("validated assumptions"),
                })
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    pub fn controls(&mut self, ui: &mut egui::Ui) {
        if !self.presets.is_empty() {
            let old_index = self.preset_index;
            egui::ComboBox::from_id_salt("history-preset")
                .selected_text(&self.presets[self.preset_index].0)
                .show_ui(ui, |ui| {
                    for (index, (name, _)) in self.presets.iter().enumerate() {
                        ui.selectable_value(&mut self.preset_index, index, name);
                    }
                });
            if self.preset_index != old_index {
                self.assumptions = Some(self.presets[self.preset_index].1.clone());
                self.edited = true;
                self.requested = true;
                if let Some(pending) = &self.pending {
                    pending.cancellation.cancel();
                }
            }
        }
        ui.collapsing("Operating assumptions",|ui|{
            let Some(a)=&mut self.assumptions else{ui.weak("Load an identified operating-assumptions file to calculate history.");return;};
            let mut changed=false;
            changed|=ui.add(egui::Slider::new(&mut a.recovery_fraction,0.0..=1.0).text("Recovery fraction")).changed();
            ui.horizontal(|ui|{ui.label("Processing delay / s");changed|=ui.add(egui::DragValue::new(&mut a.processing_delay_s).range(0.0..=604800.0).speed(3600.0)).changed();});
            ui.horizontal(|ui|{ui.label("Opening usable fuel / kg");changed|=ui.add(egui::DragValue::new(&mut a.initial_available_tritium_kg).range(0.0..=1000.0).speed(0.1)).changed();});
            if let Some(value)=&mut a.energy.thermal_to_electric_efficiency{changed|=ui.add(egui::Slider::new(value,0.0..=1.0).text("Thermal efficiency")).changed();}
            if changed{self.edited=true;}
            ui.small("Authored scenario assumptions. Editing these reuses eligible transport and recalculates the Rust ledger.");
            if let Some(pending)=&self.pending{ui.spinner();if ui.button("Cancel history").clicked(){pending.cancellation.cancel();}}
            else if ui.button("Recalculate history").clicked(){self.requested=true;}
            if self.edited{ui.colored_label(egui::Color32::YELLOW,"Displayed history belongs to previous assumptions.");}
            ui.collapsing("Model provenance",|ui|{ui.small(&a.provenance);ui.small(&a.energy.provenance);for limit in &a.service_limits{ui.small(format!("{}: {:.3e} {} · {:?}",limit.component_id,limit.limit,limit.unit,limit.class));ui.small(&limit.provenance);}});
        });
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
    }

    pub fn sensitivity_controls(&mut self, ui: &mut egui::Ui, scenario: &str, variant: &str) {
        let identity = key(scenario, variant);
        ui.collapsing("Conditional sensitivity", |ui| {
            ui.small("27 full-history reruns: recovery 0.90 / 0.95 / 0.99; processing delay and service limits ×0.5 / ×1 / ×2. These authored probes are not uncertainty bounds.");
            if let Some(pending) = &self.pending_sensitivity {
                ui.spinner();
                if ui.button("Cancel sensitivity").clicked() { pending.cancellation.cancel(); }
            } else if ui.add_enabled(self.assumptions.is_some() && self.rates.contains_key(&identity) && !self.edited && self.pending.is_none(), egui::Button::new("Run sensitivity for current arrangement")).clicked() {
                let assumptions = self.assumptions.clone().expect("enabled assumptions");
                let rates = self.rates[&identity].clone();
                let transport_sha256 = rates.transport_artifact_sha256.clone();
                let worker_assumptions = assumptions.clone();
                let cancellation = Cancellation::default();
                let worker_cancel = cancellation.clone();
                let (sender, receiver) = mpsc::channel();
                let context = ui.ctx().clone();
                let grid = HistorySensitivityGrid { recovery_fraction_levels: vec![0.90,0.95,0.99],delay_multipliers:vec![0.5,1.0,2.0],service_limit_multipliers:vec![0.5,1.0,2.0], rationale:"Authored factor-of-two service/delay and 90–99% recovery probes expose consequence sensitivity; no materials qualification or probability distribution is asserted.".into() };
                match std::thread::Builder::new().name("faris-history-sensitivity".into()).spawn(move || {
                    let result = run_history_sensitivity_cancellable(&worker_assumptions,&rates,&grid,&worker_cancel);
                    let _ = sender.send(result); context.request_repaint();
                }) {
                    Ok(handle) => self.pending_sensitivity = Some(PendingSensitivity{handle:Some(handle),cancellation,receiver,identity:identity.clone(),assumptions,transport_sha256}),
                    Err(error) => self.error = Some(error.to_string()),
                }
            }
            if let Some((assumptions, transport_sha256, result)) = self.sensitivities.get(&identity) {
                if self.assumptions.as_ref() != Some(assumptions) { ui.colored_label(egui::Color32::YELLOW,"Sensitivity belongs to earlier assumptions."); }
                if self.rates.get(&identity).is_none_or(|r| &r.transport_artifact_sha256 != transport_sha256) {ui.colored_label(egui::Color32::YELLOW,"Sensitivity belongs to earlier transport results.");}
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    egui::Grid::new("sensitivity-points").striped(true).show(ui, |ui| {
                        for label in ["Recovery","Delay ×","Limit ×","FP years","Net TWh"] { ui.strong(label); } ui.end_row();
                        for point in &result.points {
                            ui.label(format!("{:.2}",point.recovery_fraction)); ui.label(format!("{:.1}",point.delay_multiplier)); ui.label(format!("{:.1}",point.service_limit_multiplier));
                            ui.label(format!("{:.4}",point.final_snapshot.cumulative_full_power_seconds/JULIAN_YEAR_SECONDS));
                            ui.label(point.final_snapshot.cumulative_net_electricity_mwh.map_or_else(||"—".into(),|v|format!("{:.4}",v/1e6)));ui.end_row();
                        }
                    });
                });
            }
        });
    }

    pub fn timeline(
        &mut self,
        ui: &mut egui::Ui,
        scenario: &str,
        variant: &str,
        year: &mut f64,
        horizon_years: f64,
    ) {
        ui.horizontal(|ui| {
            ui.strong("Calculated operating history");
            ui.separator();
            for (value, label) in [
                (Plot::Inventory, "Fuel"),
                (Plot::Electricity, "Electricity"),
                (Plot::MagnetExposure, "Exposure"),
                (Plot::FullPower, "Operation"),
            ] {
                ui.selectable_value(&mut self.plot, value, label);
            }
            if self.pending.is_some() {
                ui.spinner();
            }
        });
        ui.add(egui::Slider::new(year, 0.0..=horizon_years).text("calendar years"));
        if let Some(snapshot) = self.snapshot(scenario, variant, *year * JULIAN_YEAR_SECONDS) {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "{} · calculated snapshot {:.5} y",
                    if snapshot.operating {
                        "SOURCE ON"
                    } else {
                        "SOURCE OFF"
                    },
                    snapshot.time_s / JULIAN_YEAR_SECONDS
                ));
                ui.separator();
                ui.label(format!(
                    "Usable {:.4} kg · processing {:.4} kg",
                    snapshot.available_tritium_kg, snapshot.in_process_tritium_kg
                ));
                ui.separator();
                ui.label(format!(
                    "Full-power {:.3} y",
                    snapshot.cumulative_full_power_seconds / JULIAN_YEAR_SECONDS
                ));
                ui.separator();
                ui.label(snapshot.cumulative_net_electricity_mwh.map_or_else(
                    || "Net electricity unavailable".into(),
                    |v| format!("Net {:.3} TWh", v / 1e6),
                ));
            });
            if let Some(history) = self.result(scenario, variant) {
                let now = *year * JULIAN_YEAR_SECONDS;
                if let Some(outage) = history
                    .assumptions
                    .planned_outages
                    .iter()
                    .find(|o| o.start_s <= now && now < o.end_s)
                {
                    ui.small(format!("Planned outage: {}", outage.reason));
                }
                egui::ComboBox::from_id_salt("history-event-jump")
                    .selected_text("Jump to a calculated event…")
                    .show_ui(ui, |ui| {
                        egui::ScrollArea::vertical().max_height(200.0).show_rows(
                            ui,
                            22.0,
                            history.events.len(),
                            |ui, rows| {
                                for index in rows {
                                    let event = &history.events[index];
                                    if ui
                                        .selectable_label(
                                            false,
                                            format!(
                                                "{:.5} y · {:?} {}",
                                                event.time_s / JULIAN_YEAR_SECONDS,
                                                event.kind,
                                                event.component_id.as_deref().unwrap_or("")
                                            ),
                                        )
                                        .clicked()
                                    {
                                        *year = event.time_s / JULIAN_YEAR_SECONDS;
                                    }
                                }
                            },
                        );
                    });
            }
            ui.small("Scrubbing selects a computed snapshot at or before the requested time. Discrete events are never interpolated across.");
            ui.horizontal_wrapped(|ui| {
                ui.small(format!(
                    "Source power {:.1}% of reference",
                    snapshot.power_fraction * 100.0
                ));
                if let Some(value) = snapshot.instantaneous_transport_recovered_heat_mw {
                    ui.small(format!(
                        "Assumed recovered transport-tally heat {value:.1} MW"
                    ));
                }
                if let Some(value) = snapshot.instantaneous_alpha_recovered_heat_mw {
                    ui.small(format!("Alpha heat {value:.1} MW"));
                }
                if let Some(value) = snapshot.instantaneous_gross_electricity_mw {
                    ui.small(format!("Gross {value:.1} MWe"));
                }
                if let Some(value) = snapshot.instantaneous_auxiliary_electricity_mw {
                    ui.small(format!("Aux {value:.1} MWe"));
                }
                if let Some(value) = snapshot.instantaneous_net_electricity_mw {
                    ui.small(format!("Net {value:.1} MWe"));
                }
            });
        } else {
            ui.weak("Waiting for identified transport and completed history.");
        }
        self.plot(ui, horizon_years);
    }

    fn plot(&self, ui: &mut egui::Ui, horizon_years: f64) {
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 80.0), egui::Sense::hover());
        let colors = [
            egui::Color32::from_rgb(108, 183, 238),
            egui::Color32::from_rgb(234, 164, 83),
            egui::Color32::from_rgb(101, 191, 155),
            egui::Color32::from_rgb(188, 143, 228),
        ];
        let values = |s: &HistorySnapshot| -> Option<f64> {
            match self.plot {
                Plot::Inventory => Some(s.available_tritium_kg),
                Plot::Electricity => s.cumulative_net_electricity_mwh.map(|v| v / 1e6),
                Plot::MagnetExposure => s.component_fluence_n_m2.get("magnets").copied(),
                Plot::FullPower => Some(s.cumulative_full_power_seconds / JULIAN_YEAR_SECONDS),
            }
        };
        let max = self
            .results
            .values()
            .flat_map(|r| r.snapshots.iter())
            .filter_map(values)
            .fold(0.0_f64, f64::max)
            .max(1e-12);
        let min = self
            .results
            .values()
            .flat_map(|r| r.snapshots.iter())
            .filter_map(values)
            .fold(0.0_f64, f64::min);
        ui.painter()
            .rect_filled(rect, 2.0, egui::Color32::from_rgb(26, 28, 32));
        for (index, (id, history)) in self.results.iter().enumerate() {
            let stride = (history.snapshots.len() / 1200).max(1);
            let points: Vec<_> = history
                .snapshots
                .iter()
                .step_by(stride)
                .filter_map(|s| {
                    values(s).map(|v| {
                        egui::pos2(
                            rect.left()
                                + (s.time_s / JULIAN_YEAR_SECONDS / horizon_years) as f32
                                    * rect.width(),
                            rect.bottom() - ((v - min) / (max - min)) as f32 * rect.height(),
                        )
                    })
                })
                .collect();
            if points.len() > 1 {
                ui.painter().add(egui::Shape::line(
                    points,
                    egui::Stroke::new(1.5, colors[index % colors.len()]),
                ));
            }
            let label = self.names.get(id).map_or(id.as_str(), String::as_str);
            ui.painter().text(
                rect.left_top() + egui::vec2(8.0, index as f32 * 14.0),
                egui::Align2::LEFT_TOP,
                label,
                egui::FontId::proportional(11.0),
                colors[index % colors.len()],
            );
        }
        ui.small(match self.plot {
            Plot::Inventory => format!("Usable tritium / kg · vertical range {min:.3}–{max:.3}"),
            Plot::Electricity => {
                format!("Cumulative signed net electricity / TWh · {min:.3}–{max:.3}")
            }
            Plot::MagnetExposure => format!(
                "Component-average integrated neutron fluence / neutrons/m² · {min:.2e}–{max:.2e}"
            ),
            Plot::FullPower => format!("Cumulative full-power time / years · {min:.3}–{max:.3}"),
        });
    }

    pub fn inspector(
        &self,
        ui: &mut egui::Ui,
        scenario: &str,
        variant: &str,
        component: &str,
        time_s: f64,
    ) {
        if let Some(snapshot) = self.snapshot(scenario, variant, time_s) {
            if let Some(value) = snapshot.component_fluence_n_m2.get(component) {
                ui.label(format!("Accumulated mean fluence: {value:.3e} neutrons/m²"));
            }
            if let Some(count) = snapshot.component_replacements.get(component) {
                ui.label(format!("Replacements completed: {count}"));
            }
            ui.small(format!(
                "Mass residual: {:.2e} kg",
                snapshot.mass_balance_residual_kg
            ));
        }
        if let Some(history) = self.result(scenario, variant) {
            if let Some(limit) = history
                .assumptions
                .service_limits
                .iter()
                .find(|l| l.component_id == component)
            {
                ui.label(format!(
                    "Authored {:?} trigger: {:.3e} {}",
                    limit.class, limit.limit, limit.unit
                ));
                ui.small(&limit.provenance);
            } else {
                ui.weak("No service trigger declared for this component.");
            }
            ui.collapsing("Operating events", |ui| {
                for event in history
                    .events
                    .iter()
                    .filter(|e| e.component_id.as_deref() == Some(component))
                    .take(24)
                {
                    ui.small(format!(
                        "{:.4} y · {:?}",
                        event.time_s / JULIAN_YEAR_SECONDS,
                        event.kind
                    ));
                    ui.small(&event.note);
                }
            });
            ui.small("Conditional scenario history; sampling and model uncertainty are not propagated as a qualified lifetime bound.");
        }
    }
}

impl Drop for HistoryPanel {
    fn drop(&mut self) {
        if let Some(p) = &mut self.pending {
            p.cancellation.cancel();
            if let Some(handle) = p.handle.take() {
                let _ = handle.join();
            }
        }
        if let Some(p) = &mut self.pending_sensitivity {
            p.cancellation.cancel();
            if let Some(handle) = p.handle.take() {
                let _ = handle.join();
            }
        }
    }
}
