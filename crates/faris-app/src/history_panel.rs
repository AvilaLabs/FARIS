//! Native presentation of the shared Rust history engine. No UI-owned physics.

use crate::badge::{self, Kind};
use eframe::egui;
use faris_engine::{
    brief::{Arrangement, decimate, limits_differ as differs, magnet_limit},
    comparison::{
        HistorySensitivityGrid, HistorySensitivityResult, OperatingState, classify_operating_state,
        component_replacement_spans, run_history_sensitivity_cancellable,
    },
    history::{
        EventKind, HistoryEvent, HistoryResult, HistorySnapshot, JULIAN_YEAR_SECONDS,
        TransportDrivingRates, run_operating_history_cancellable,
    },
    jobs::Cancellation,
    reactor::ReactorRun,
};
use faris_model::history::OperatingHistoryAssumptions;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};

/// Quiet period after the last what-if edit before all histories recalculate.
const DEBOUNCE: Duration = Duration::from_millis(350);
/// Upper bound on plotted points per series; scrubbing redraws every frame.
const MAX_PLOT_POINTS: usize = 1500;
const DAY_S: f64 = 86_400.0;

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

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Debug)]
enum Plot {
    #[default]
    MagnetFluence,
    Electricity,
    Tritium,
    FullPower,
}

impl Plot {
    fn value(self, s: &HistorySnapshot) -> Option<f64> {
        match self {
            Plot::MagnetFluence => s.component_fluence_n_m2.get("magnets").copied(),
            Plot::Electricity => s.cumulative_net_electricity_mwh.map(|v| v / 1e6),
            Plot::Tritium => Some(s.available_tritium_kg),
            Plot::FullPower => Some(s.cumulative_full_power_seconds / JULIAN_YEAR_SECONDS),
        }
    }
    fn title(self) -> &'static str {
        match self {
            Plot::MagnetFluence => "Magnet fluence · component-average, n/m²",
            Plot::Electricity => "Cumulative signed net electricity · TWh",
            Plot::Tritium => "Usable tritium inventory · kg",
            Plot::FullPower => "Cumulative full-power time · years",
        }
    }
    fn format(self, v: f64) -> String {
        match self {
            Plot::MagnetFluence => format!("{} n/m²", fmt_sci(v)),
            Plot::Electricity => format!("{v:.2} TWh"),
            Plot::Tritium => format!("{v:.3} kg"),
            Plot::FullPower => format!("{v:.2} y"),
        }
    }
}

struct Preset {
    name: String,
    assumptions: OperatingHistoryAssumptions,
    note: String,
}

/// Decimated curves and replacement spans, valid for one history revision.
#[derive(Default)]
struct CachedSeries {
    points: BTreeMap<Plot, Vec<[f64; 2]>>,
    magnet_spans: Option<Vec<(f64, f64)>>,
}

#[derive(Clone)]
struct Series {
    key: String,
    port: bool,
    breeder: bool,
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
    presets: Vec<Preset>,
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
    /// Time of the last what-if edit while the debounce window is open.
    debounce: Option<Instant>,
    validation: Option<String>,
    hidden: BTreeSet<String>,
    cache: BTreeMap<String, CachedSeries>,
    cache_revision: u64,
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
        if let Some(loaded) = &assumptions {
            let mut push = |name: &str, a: OperatingHistoryAssumptions, extra: Option<&str>| {
                let note = preset_note(&a, extra);
                presets.push(Preset {
                    name: name.into(),
                    assumptions: a,
                    note,
                });
            };
            let bundled = |bytes: &[u8]| -> Result<OperatingHistoryAssumptions, String> {
                let a: OperatingHistoryAssumptions =
                    serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
                a.validate()?;
                Ok(a)
            };
            push(
                "Demountable magnets · REBCO fluence limit",
                bundled(include_bytes!(
                    "../../../scenarios/arc-inspired/demountable-magnet-assumptions.json"
                ))?,
                None,
            );
            push("Loaded assumptions", loaded.clone(), None);
            push(
                "Baseline authored scenario",
                bundled(include_bytes!(
                    "../../../scenarios/arc-inspired/demo-operating-assumptions.json"
                ))?,
                None,
            );
            push(
                "Permanent-trip test (numerical control)",
                bundled(include_bytes!(
                    "../../../scenarios/arc-inspired/demo-event-assumptions.json"
                ))?,
                Some(
                    "Numerical control only: it deliberately trips the magnets permanently to exercise the replacement and shutdown logic. This preset ends with negative net electricity and is not a plant scenario.",
                ),
            );
        }
        // The demountable-magnet story is the default whenever presets exist.
        let assumptions = presets.first().map(|p| p.assumptions.clone());
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
            debounce: None,
            validation: None,
            hidden: BTreeSet::new(),
            cache: BTreeMap::new(),
            cache_revision: 0,
        })
    }

    pub fn result(&self, scenario: &str, variant: &str) -> Option<&HistoryResult> {
        self.results.get(&key(scenario, variant))
    }

    /// Stable read-only state for bounded native interface checks. This exposes
    /// the exact authored inputs and their current result bindings without
    /// adding a second history or sensitivity implementation.
    pub fn interface_status(&self, scenario: &str, variant: &str) -> serde_json::Value {
        use sha2::{Digest, Sha256};

        let identity = key(scenario, variant);
        let assumptions_json = self
            .assumptions
            .as_ref()
            .and_then(|value| serde_json::to_vec(value).ok());
        let assumptions_sha256 = assumptions_json
            .as_ref()
            .map(|bytes| format!("{:x}", Sha256::digest(bytes)));
        let result = self.results.get(&identity);
        let current_transport_sha256 = self
            .rates
            .get(&identity)
            .map(|rates| rates.transport_artifact_sha256.as_str());
        let history_transport_sha256 =
            result.map(|history| history.driving_rates.transport_artifact_sha256.as_str());
        let assumptions_match =
            result.map(|history| self.assumptions.as_ref() == Some(&history.assumptions));
        let history_assumptions_json =
            result.and_then(|history| serde_json::to_vec(&history.assumptions).ok());
        let history_assumptions_sha256 = history_assumptions_json
            .as_ref()
            .map(|bytes| format!("{:x}", Sha256::digest(bytes)));
        let transport_matches = result.map(|history| {
            self.rates
                .get(&identity)
                .is_some_and(|rates| rates == &history.driving_rates)
        });
        let sensitivity = self.sensitivities.get(&identity);
        let sensitivity_pending = self
            .pending_sensitivity
            .as_ref()
            .is_some_and(|pending| pending.identity == identity);
        let sensitivity_assumptions_match =
            sensitivity.map(|(assumptions, _, _)| self.assumptions.as_ref() == Some(assumptions));
        let sensitivity_transport_matches = sensitivity
            .map(|(_, transport, _)| current_transport_sha256 == Some(transport.as_str()));
        let sensitivity_result_sha256 = sensitivity.and_then(|(_, _, value)| {
            let mut bytes = serde_json::to_vec_pretty(value).ok()?;
            bytes.push(b'\n');
            Some(format!("{:x}", Sha256::digest(bytes)))
        });
        let sensitivity_status = if sensitivity_pending {
            "pending"
        } else if sensitivity.is_none() {
            "not_run"
        } else if sensitivity_assumptions_match == Some(true)
            && sensitivity_transport_matches == Some(true)
        {
            "current"
        } else {
            "earlier_inputs"
        };

        serde_json::json!({
            "scenario_sha256": scenario,
            "variant_id": variant,
            "preset_index": self.preset_index,
            "preset_name": self.presets.get(self.preset_index).map(|preset| preset.name.as_str()),
            "current_assumptions": self.assumptions,
            "current_assumptions_sha256": assumptions_sha256,
            "history_assumptions": result.map(|history| &history.assumptions),
            "history_assumptions_sha256": history_assumptions_sha256,
            "history_pending": self.pending.is_some(),
            "history_requested": self.requested,
            "history_status": if result.is_none() { "not_loaded" } else if assumptions_match == Some(true) && transport_matches == Some(true) { "current" } else { "earlier_inputs" },
            "history_assumptions_match": assumptions_match,
            "history_transport_matches": transport_matches,
            "current_transport_artifact_sha256": current_transport_sha256,
            "history_transport_artifact_sha256": history_transport_sha256,
            "sensitivity_pending": sensitivity_pending,
            "sensitivity_status": sensitivity_status,
            "sensitivity_point_count": sensitivity.map_or(0, |(_, _, value)| value.points.len()),
            "sensitivity_assumptions_match": sensitivity_assumptions_match,
            "sensitivity_transport_matches": sensitivity_transport_matches,
            "sensitivity_transport_artifact_sha256": sensitivity.map(|(_, transport, _)| transport.as_str()),
            "sensitivity_result_sha256": sensitivity_result_sha256,
        })
    }
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
    pub fn is_stale(&self) -> bool {
        self.edited
    }
    /// Name of the selected operating-assumption preset, for conditional labels.
    /// The selected preset's own magnet service limit, to tell an edited limit
    /// from the preset's.
    pub fn preset_magnet_limit(&self) -> Option<f64> {
        self.presets
            .get(self.preset_index)
            .and_then(|p| service_limit(&p.assumptions, "magnets"))
    }

    pub fn preset_label(&self) -> &str {
        self.presets
            .get(self.preset_index)
            .map_or("the loaded operating assumptions", |preset| {
                preset.name.as_str()
            })
    }

    /// Select a preset by name and recalculate; false when no such preset exists.
    pub fn select_preset(&mut self, name: &str) -> bool {
        match self.presets.iter().position(|preset| preset.name == name) {
            Some(index) => {
                self.preset_index = index;
                self.apply_preset(index);
                true
            }
            None => false,
        }
    }

    fn apply_preset(&mut self, index: usize) {
        self.assumptions = Some(self.presets[index].assumptions.clone());
        self.edited = true;
        self.requested = true;
        self.debounce = None;
        self.validation = None;
        if let Some(pending) = &self.pending {
            pending.cancellation.cancel();
        }
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
        if history.processing_model.as_deref()
            != Some(faris_engine::history::HISTORY_PROCESSING_MODEL_ID)
        {
            return Err("Saved history uses an earlier processing method; recalculating with continuous delayed release.".into());
        }
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
            // A cancelled job was superseded by an edit or preset change.
            Err(_) if pending.cancellation.is_cancelled() => {}
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
        if let Some(edit) = self.debounce {
            let remaining = DEBOUNCE.saturating_sub(edit.elapsed());
            if !remaining.is_zero() {
                ctx.request_repaint_after(remaining);
                return;
            }
            self.debounce = None;
            self.requested = true;
        }
        if identity != self.bound_transport && self.pending.is_none() {
            self.requested = true;
        }
        if !self.requested || self.pending.is_some() || inputs.is_empty() {
            return;
        }
        let Some(assumptions) = self.assumptions.clone() else {
            return;
        };
        if let Err(error) = assumptions.validate() {
            self.validation = Some(error);
            self.requested = false;
            return;
        }
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
        // Retain the last complete curves for inspection while the worker runs,
        // but never present them as belonging to a new transport driver.
        if !self.results.is_empty() {
            self.edited = true;
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

    /// Start (calendar years) of the first calculated magnet replacement.
    pub fn first_magnet_replacement_year(&self, scenario: &str, variant: &str) -> Option<f64> {
        let history = self.result(scenario, variant)?;
        component_replacement_spans(&history.events, "magnets", history.assumptions.horizon_s)
            .first()
            .map(|(start, _)| start / JULIAN_YEAR_SECONDS)
    }

    /// Returns the area of the "What if…" group when histories can be edited.
    pub fn controls(&mut self, ui: &mut egui::Ui) -> Option<egui::Rect> {
        if !self.presets.is_empty() {
            let old_index = self.preset_index;
            let combo = egui::ComboBox::from_id_salt("history-preset")
                .selected_text(&self.presets[self.preset_index].name)
                .show_ui(ui, |ui| {
                    for (index, preset) in self.presets.iter().enumerate() {
                        ui.selectable_value(&mut self.preset_index, index, &preset.name)
                            .on_hover_text(&preset.note);
                    }
                });
            combo
                .response
                .on_hover_text(&self.presets[self.preset_index].note);
            if self.preset_index != old_index {
                self.apply_preset(self.preset_index);
            }
            if self.presets[self.preset_index]
                .name
                .starts_with("Permanent-trip")
            {
                ui.colored_label(
                    Kind::Partial.color(),
                    "Numerical control: not a plant scenario",
                );
            }
        }
        let top = ui.cursor().top();
        self.what_if(ui);
        let what_if = self.assumptions.is_some().then(|| {
            egui::Rect::from_min_max(
                egui::pos2(ui.max_rect().left(), top),
                egui::pos2(ui.max_rect().right(), ui.cursor().top()),
            )
        });
        ui.horizontal(|ui| {
            if self.pending.is_some() || self.debounce.is_some() {
                ui.spinner();
                ui.small("Recalculating all histories…");
            }
            if let Some(pending) = &self.pending {
                if ui.small_button("Cancel").clicked() {
                    pending.cancellation.cancel();
                }
            } else if ui
                .small_button("Recalculate history")
                .on_hover_text("Edits recalculate automatically; this is a manual fallback.")
                .clicked()
            {
                self.debounce = None;
                self.requested = true;
            }
        });
        if self.edited && self.pending.is_none() && self.debounce.is_none() {
            ui.colored_label(
                egui::Color32::YELLOW,
                "Displayed history belongs to earlier inputs.",
            );
        }
        if let Some(a) = &self.assumptions {
            ui.collapsing("Model provenance", |ui| {
                ui.small(&a.provenance);
                ui.small(&a.energy.provenance);
                for limit in &a.service_limits {
                    ui.small(format!(
                        "{}: {:.3e} {} · {:?}",
                        limit.component_id, limit.limit, limit.unit, limit.class
                    ));
                    ui.small(&limit.provenance);
                }
            });
        }
        if let Some(error) = &self.validation {
            ui.colored_label(egui::Color32::LIGHT_RED, format!("Not run: {error}"));
        }
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
        what_if
    }

    /// Sliders bound to the selected authored assumptions. Any edit cancels the
    /// running job and schedules a recalculation after a short quiet period.
    fn what_if(&mut self, ui: &mut egui::Ui) {
        let literature = self
            .presets
            .first()
            .and_then(|p| service_limit(&p.assumptions, "magnets"));
        let base = self.presets.get(self.preset_index).map(|p| &p.assumptions);
        let Some(a) = self.assumptions.as_mut() else {
            ui.weak("Load an identified operating-assumptions file to calculate history.");
            return;
        };
        let mut changed = false;
        ui.add_space(4.0);
        ui.strong("What if…");
        ui.small("Authored assumptions: drag to recalculate every history.");

        let base_magnet = base.and_then(|b| service_limit(b, "magnets"));
        let base_magnet_swap = base.and_then(|b| replacement_days(b, "magnets"));
        if let Some(limit) = a
            .service_limits
            .iter_mut()
            .find(|l| l.component_id == "magnets")
        {
            let is_edited = base_magnet.is_some_and(|b| differs(limit.limit, b));
            if row_header(
                ui,
                "Magnet service limit",
                &format!("{} n/m²", fmt_sci(limit.limit)),
                is_edited,
            ) {
                limit.limit = base_magnet.unwrap_or(limit.limit);
                changed = true;
            }
            let response = log_slider(ui, &mut limit.limit, 1e21..=1e23);
            changed |= response.changed();
            if let Some(lit) = literature {
                let (_, marker) = ui.allocate_space(egui::vec2(response.rect.width(), 12.0));
                let frac = ((lit.ln() - 1e21_f64.ln()) / (1e23_f64.ln() - 1e21_f64.ln())) as f32;
                let radius = response.rect.height() / 2.5;
                let x =
                    response.rect.left() + radius + frac * (response.rect.width() - 2.0 * radius);
                let color = Kind::Literature.color();
                let painter = ui.painter();
                painter.line_segment(
                    [
                        egui::pos2(x, marker.top()),
                        egui::pos2(x, marker.top() + 5.0),
                    ],
                    egui::Stroke::new(1.5, color),
                );
                painter.text(
                    egui::pos2(x, marker.top() + 5.0),
                    egui::Align2::CENTER_TOP,
                    format!("{} literature", fmt_sci(lit)),
                    egui::FontId::proportional(10.0),
                    color,
                );
                if differs(limit.limit, lit)
                    && ui
                        .small_button(format!("reset to literature value ({})", fmt_sci(lit)))
                        .clicked()
                {
                    limit.limit = lit;
                    changed = true;
                }
            }
            if let Some(duration) = limit.replacement_duration_s.as_mut() {
                let mut days = (*duration / DAY_S).round();
                let is_edited = base_magnet_swap.is_some_and(|b| differs(days, b));
                if row_header(
                    ui,
                    "Magnet replacement duration",
                    &format!("{days:.0} days"),
                    is_edited,
                ) {
                    days = base_magnet_swap.unwrap_or(days);
                    *duration = days * DAY_S;
                    changed = true;
                }
                if full_slider(ui, &mut days, 14.0..=365.0, false, Some(1.0)).changed() {
                    *duration = days * DAY_S;
                    changed = true;
                }
            }
        } else {
            ui.small("This preset declares no magnet service limit.");
        }

        let base_blanket = base.and_then(|b| service_limit(b, "blanket"));
        if let Some(limit) = a
            .service_limits
            .iter_mut()
            .find(|l| l.component_id == "blanket")
        {
            let is_edited = base_blanket.is_some_and(|b| differs(limit.limit, b));
            if row_header(
                ui,
                "Blanket service limit",
                &format!("{} n/m²", fmt_sci(limit.limit)),
                is_edited,
            ) {
                limit.limit = base_blanket.unwrap_or(limit.limit);
                changed = true;
            }
            changed |= log_slider(ui, &mut limit.limit, 1e25..=1e27).changed();
        }

        let base_recovery = base.map(|b| b.recovery_fraction);
        let is_edited = base_recovery.is_some_and(|b| differs(a.recovery_fraction, b));
        if row_header(
            ui,
            "Tritium recovery fraction",
            &format!("{:.3}", a.recovery_fraction),
            is_edited,
        ) {
            a.recovery_fraction = base_recovery.unwrap_or(a.recovery_fraction);
            changed = true;
        }
        changed |=
            full_slider(ui, &mut a.recovery_fraction, 0.0..=1.0, false, Some(0.005)).changed();

        let base_delay = base.map(|b| b.processing_delay_s / DAY_S);
        let mut delay_days = a.processing_delay_s / DAY_S;
        let is_edited = base_delay.is_some_and(|b| differs(delay_days, b));
        if row_header(
            ui,
            "Processing delay",
            &format!("{delay_days:.2} days"),
            is_edited,
        ) {
            delay_days = base_delay.unwrap_or(delay_days);
            a.processing_delay_s = delay_days * DAY_S;
            changed = true;
        }
        if full_slider(ui, &mut delay_days, 0.0..=7.0, false, Some(0.25)).changed() {
            a.processing_delay_s = delay_days * DAY_S;
            changed = true;
        }

        if let Some(efficiency) = a.energy.thermal_to_electric_efficiency.as_mut() {
            let base_eff = base.and_then(|b| b.energy.thermal_to_electric_efficiency);
            let is_edited = base_eff.is_some_and(|b| differs(*efficiency, b));
            if row_header(
                ui,
                "Thermal-to-electric efficiency",
                &format!("{:.3}", *efficiency),
                is_edited,
            ) {
                *efficiency = base_eff.unwrap_or(*efficiency);
                changed = true;
            }
            changed |= full_slider(ui, efficiency, 0.0..=1.0, false, Some(0.01)).changed();
        }

        ui.horizontal(|ui| {
            ui.label("Opening usable fuel / kg");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut a.initial_available_tritium_kg)
                        .range(0.0..=1000.0)
                        .speed(0.1),
                )
                .changed();
        });
        if let Some(b) = base
            && *a != *b
            && ui.button("Revert all to preset").clicked()
        {
            *a = b.clone();
            changed = true;
        }
        if changed {
            self.edited = true;
            if let Some(pending) = &self.pending {
                pending.cancellation.cancel();
            }
            match self.assumptions.as_ref().map(|a| a.validate()) {
                Some(Err(error)) => {
                    self.validation = Some(error);
                    self.debounce = None;
                }
                _ => {
                    self.validation = None;
                    self.debounce = Some(Instant::now());
                }
            }
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
                let grid: HistorySensitivityGrid = serde_json::from_slice(include_bytes!("../../../scenarios/arc-inspired/demo-operating-sensitivity.json"))
                    .expect("bundled sensitivity input is checked with the authored scenario");
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

    /// Which family a result belongs to, from the bound transport name
    /// ("reference · penetration", "breeder-emphasis · control").
    fn series_list(&self) -> Vec<Series> {
        let mut list: Vec<Series> = self
            .results
            .keys()
            .map(|id| {
                let name = self.names.get(id).map_or_else(
                    || id.rsplit("::").next().unwrap_or(id).to_string(),
                    Clone::clone,
                );
                Series {
                    key: id.clone(),
                    port: name.contains("penetration"),
                    breeder: name.starts_with("breeder"),
                }
            })
            .collect();
        list.sort_by_key(|s| (!s.port, s.breeder));
        list
    }

    fn ensure_cache(&mut self) {
        if self.cache_revision != self.revision {
            self.cache.clear();
            self.cache_revision = self.revision;
        }
        let plot = self.plot;
        for (id, history) in &self.results {
            let entry = self.cache.entry(id.clone()).or_default();
            entry.points.entry(plot).or_insert_with(|| {
                let raw: Vec<[f64; 2]> = history
                    .snapshots
                    .iter()
                    .filter_map(|s| plot.value(s).map(|v| [s.time_s / JULIAN_YEAR_SECONDS, v]))
                    .collect();
                decimate(raw, MAX_PLOT_POINTS)
            });
            entry.magnet_spans.get_or_insert_with(|| {
                component_replacement_spans(
                    &history.events,
                    "magnets",
                    history.assumptions.horizon_s,
                )
            });
        }
    }

    pub fn timeline(
        &mut self,
        ui: &mut egui::Ui,
        scenario: &str,
        variant: &str,
        year: &mut f64,
        horizon_years: f64,
    ) -> Option<egui::Rect> {
        ui.horizontal_wrapped(|ui| {
            ui.strong("Calculated operating history");
            ui.separator();
            for (value, label) in [
                (Plot::MagnetFluence, "Magnet fluence"),
                (Plot::Electricity, "Electricity"),
                (Plot::Tritium, "Tritium"),
                (Plot::FullPower, "Full-power years"),
            ] {
                ui.selectable_value(&mut self.plot, value, label);
            }
            if self.pending.is_some() || self.debounce.is_some() {
                ui.spinner();
            }
            if self.edited {
                ui.colored_label(egui::Color32::YELLOW, "Earlier history inputs")
                    .on_hover_text("Displayed history belongs to earlier transport or operating inputs. Recalculation applies the current inputs.");
            }
        });
        let active = key(scenario, variant);
        if self.results.is_empty() {
            ui.weak("Waiting for identified transport and completed history.");
            return None;
        }
        self.ensure_cache();
        let series = self.series_list();
        ui.horizontal_wrapped(|ui| {
            for s in &series {
                let color = arrangement_color(s.port, s.breeder);
                let visible = !self.hidden.contains(&s.key);
                let is_active = s.key == active;
                let glyph = if s.port { "—" } else { "- -" };
                let mut text = egui::RichText::new(format!(
                    "{glyph} {}",
                    arrangement_label(s.port, s.breeder)
                ))
                .color(if visible { color } else { egui::Color32::GRAY });
                if is_active {
                    text = text.strong();
                }
                let response = ui
                    .add(
                        egui::Button::new(text)
                            .fill(if visible {
                                color.gamma_multiply(0.16)
                            } else {
                                egui::Color32::TRANSPARENT
                            })
                            .stroke(egui::Stroke::new(
                                if is_active { 1.6 } else { 1.0 },
                                color.gamma_multiply(if visible { 0.9 } else { 0.35 }),
                            ))
                            .corner_radius(10.0),
                    )
                    .on_hover_text(if is_active {
                        "Click to show or hide. This is the arrangement drawn in 3D; it is drawn thicker."
                    } else {
                        "Click to show or hide this arrangement."
                    });
                if response.clicked() && !self.hidden.remove(&s.key) {
                    self.hidden.insert(s.key.clone());
                }
            }
        });
        self.headline(ui, &active, *year, horizon_years);
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::Slider::new(year, 0.0..=horizon_years).text("calendar years"));
            if let Some(history) = self.results.get(&active) {
                let spans = self
                    .cache
                    .get(&active)
                    .and_then(|c| c.magnet_spans.as_deref())
                    .unwrap_or(&[]);
                egui::ComboBox::from_id_salt("history-event-jump")
                    .selected_text("Jump to a calculated event…")
                    .show_ui(ui, |ui| {
                        for (index, (start, _)) in spans.iter().enumerate() {
                            if ui
                                .selectable_label(
                                    false,
                                    format!(
                                        "Magnet replacement #{} · {:.1} y",
                                        index + 1,
                                        start / JULIAN_YEAR_SECONDS
                                    ),
                                )
                                .clicked()
                            {
                                *year = start / JULIAN_YEAR_SECONDS;
                            }
                        }
                        if !spans.is_empty() {
                            ui.separator();
                        }
                        egui::ScrollArea::vertical().max_height(200.0).show_rows(
                            ui,
                            22.0,
                            history.events.len(),
                            |ui, rows| {
                                for index in rows {
                                    let event = &history.events[index];
                                    if ui.selectable_label(false, event_label(event)).clicked() {
                                        *year = event.time_s / JULIAN_YEAR_SECONDS;
                                    }
                                }
                            },
                        );
                    });
            }
        });
        let plot = self.draw_plot(ui, &active, year, horizon_years, &series);
        ui.small("Click or drag on the plot to scrub; hover for values. Scrubbing selects the computed snapshot at or before the requested time; discrete events are never interpolated across. Conditional on authored assumptions.");
        Some(plot)
    }

    /// Headline numbers for the active arrangement at the scrub time.
    fn headline(&self, ui: &mut egui::Ui, active: &str, year: f64, horizon_years: f64) {
        let Some(history) = self.results.get(active) else {
            ui.weak("Waiting for identified transport and completed history.");
            return;
        };
        let t = year * JULIAN_YEAR_SECONDS;
        let Some(snapshot) = snapshot_at(history, t) else {
            return;
        };
        let spans = self
            .cache
            .get(active)
            .and_then(|c| c.magnet_spans.as_deref())
            .unwrap_or(&[]);
        let state = classify_operating_state(
            &history.events,
            &history.assumptions.planned_outages,
            snapshot.operating,
            snapshot.time_s,
            history.assumptions.horizon_s,
        );
        let (state_text, state_kind, state_note) = match state {
            OperatingState::Operating => (
                "Operating",
                Kind::Calculated,
                "Fusion source on in the calculated snapshot.",
            ),
            OperatingState::MagnetReplacement => (
                "Magnet replacement",
                Kind::Conditional,
                "Magnet envelope swap outage; its duration is an authored assumption.",
            ),
            OperatingState::OtherReplacement => (
                "Component replacement",
                Kind::Conditional,
                "A non-magnet replaceable component is being swapped; the duration is authored.",
            ),
            OperatingState::PlannedOutage => (
                "Planned outage",
                Kind::Authored,
                "Authored illustrative maintenance outage, not an estimated availability.",
            ),
            OperatingState::FuelLimited => (
                "Fuel-limited",
                Kind::Partial,
                "Usable tritium fell to the startup reserve; operation resumes at the restart threshold.",
            ),
            OperatingState::Stopped => (
                "Stopped",
                Kind::Calculated,
                "Source off with no declared outage or replacement at this instant.",
            ),
        };
        let swaps = spans.iter().filter(|(start, _)| *start <= t).count();
        let in_progress = spans.iter().any(|(start, end)| *start <= t && t < *end);
        let next = spans.iter().find(|(start, _)| *start > t);
        let next_text = match (in_progress, next) {
            (true, _) => "underway".to_string(),
            (false, Some((start, _))) => format!("in {:.1} y", (start - t) / JULIAN_YEAR_SECONDS),
            (false, None) => format!("none before {horizon_years:.0} y"),
        };
        ui.horizontal_wrapped(|ui| {
            badge::metric(
                ui,
                "Year",
                &format!("{:.1}", snapshot.time_s / JULIAN_YEAR_SECONDS),
                Kind::Calculated,
                "Calculated snapshot time at or before the requested year.",
            );
            ui.add_space(14.0);
            badge::metric(ui, "State", state_text, state_kind, state_note);
            ui.add_space(14.0);
            badge::metric(
                ui,
                "Usable tritium",
                &format!("{:.2} kg", snapshot.available_tritium_kg),
                Kind::Calculated,
                &format!(
                    "Usable stock; {:.3} kg is still in processing. Conditional on authored recovery and delay.",
                    snapshot.in_process_tritium_kg
                ),
            );
            ui.add_space(14.0);
            match snapshot.cumulative_net_electricity_mwh {
                Some(v) => badge::metric(
                    ui,
                    "Net electricity so far",
                    &format!("{:.2} TWh", v / 1e6),
                    Kind::Conditional,
                    "Signed net of gross output and auxiliary load under authored energy assumptions; not a plant estimate.",
                ),
                None => badge::metric(
                    ui,
                    "Net electricity so far",
                    "unavailable",
                    Kind::NotEvaluated,
                    history.energy_unavailable_reason.as_deref().unwrap_or("No total nuclear-heat response is bound to this transport run."),
                ),
            };
            ui.add_space(14.0);
            badge::metric(
                ui,
                "Magnet swaps so far",
                &swaps.to_string(),
                Kind::Conditional,
                "Counted from calculated replacement events; they follow from the authored service limit applied to the transport-driven fluence.",
            );
            ui.add_space(14.0);
            badge::metric(
                ui,
                "Next magnet swap",
                &next_text,
                Kind::Conditional,
                "From the computed events of this history; none means the limit is not reached within the horizon.",
            );
        });
    }

    fn draw_plot(
        &self,
        ui: &mut egui::Ui,
        active: &str,
        year: &mut f64,
        horizon_years: f64,
        series: &[Series],
    ) -> egui::Rect {
        let plot = self.plot;
        let height = (ui.clip_rect().bottom() - ui.cursor().top() - 52.0).clamp(150.0, 1400.0);
        let (outer, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width().max(200.0), height),
            egui::Sense::click_and_drag(),
        );
        let painter = ui.painter_at(outer);
        painter.rect_filled(outer, 4.0, egui::Color32::from_rgb(24, 26, 31));
        let area = egui::Rect::from_min_max(
            outer.min + egui::vec2(78.0, 26.0),
            outer.max - egui::vec2(18.0, 34.0),
        );
        let text_color = egui::Color32::from_gray(190);
        let grid_color = egui::Color32::from_white_alpha(14);
        let limit = self.results.get(active).and_then(|h| {
            (plot == Plot::MagnetFluence)
                .then(|| service_limit(&h.assumptions, "magnets"))
                .flatten()
                .map(|value| (value, self.limit_label(h, value)))
        });

        // Vertical range over every series so hiding one does not rescale.
        let mut lo = 0.0_f64;
        let mut hi = f64::MIN;
        for cached in self.cache.values() {
            for p in cached.points.get(&plot).into_iter().flatten() {
                lo = lo.min(p[1]);
                hi = hi.max(p[1]);
            }
        }
        if let Some((value, _)) = &limit {
            hi = hi.max(*value * 1.12);
        }
        if !hi.is_finite() || hi <= lo {
            hi = lo + 1.0;
        }
        let (y_min, y_max, step) = nice_axis(lo, hi, 5);
        let map_x = |yr: f64| area.left() + (yr / horizon_years) as f32 * area.width();
        let map_y = |v: f64| area.bottom() - ((v - y_min) / (y_max - y_min)) as f32 * area.height();

        // Axes and grid.
        let font = egui::FontId::proportional(11.0);
        let n_y = ((y_max - y_min) / step).round() as i32;
        for i in 0..=n_y {
            let v = y_min + step * f64::from(i);
            let y = map_y(v);
            painter.line_segment(
                [egui::pos2(area.left(), y), egui::pos2(area.right(), y)],
                egui::Stroke::new(1.0, grid_color),
            );
            painter.text(
                egui::pos2(area.left() - 6.0, y),
                egui::Align2::RIGHT_CENTER,
                fmt_tick(plot, v, step),
                font.clone(),
                text_color,
            );
        }
        for yr in 0..=horizon_years.floor() as i32 {
            let x = map_x(f64::from(yr));
            let major = yr % 5 == 0;
            if major {
                painter.line_segment(
                    [egui::pos2(x, area.top()), egui::pos2(x, area.bottom())],
                    egui::Stroke::new(1.0, grid_color),
                );
                painter.text(
                    egui::pos2(x, area.bottom() + 5.0),
                    egui::Align2::CENTER_TOP,
                    yr.to_string(),
                    font.clone(),
                    text_color,
                );
            }
            painter.line_segment(
                [
                    egui::pos2(x, area.bottom()),
                    egui::pos2(x, area.bottom() + if major { 4.0 } else { 2.0 }),
                ],
                egui::Stroke::new(1.0, text_color.gamma_multiply(0.6)),
            );
        }
        painter.line_segment(
            [area.left_bottom(), area.right_bottom()],
            egui::Stroke::new(1.0, text_color.gamma_multiply(0.7)),
        );
        painter.line_segment(
            [area.left_top(), area.left_bottom()],
            egui::Stroke::new(1.0, text_color.gamma_multiply(0.7)),
        );
        painter.text(
            egui::pos2(area.center().x, outer.bottom() - 4.0),
            egui::Align2::CENTER_BOTTOM,
            "calendar year",
            font.clone(),
            text_color,
        );
        painter.text(
            outer.left_top() + egui::vec2(10.0, 6.0),
            egui::Align2::LEFT_TOP,
            plot.title(),
            egui::FontId::proportional(12.0),
            egui::Color32::from_gray(225),
        );

        // Replacement outages of the active arrangement only.
        if let Some(spans) = self.cache.get(active).and_then(|c| c.magnet_spans.as_ref()) {
            for (start, end) in spans {
                let x0 = map_x(start / JULIAN_YEAR_SECONDS);
                let x1 = map_x(end / JULIAN_YEAR_SECONDS).max(x0 + 3.0);
                painter.rect_filled(
                    egui::Rect::from_x_y_ranges(x0..=x1, area.y_range()),
                    0.0,
                    Kind::Partial.color().gamma_multiply(0.16),
                );
            }
        }

        // Service limit.
        if let Some((value, label)) = &limit {
            let y = map_y(*value);
            let color = Kind::Failed.color();
            painter.extend(egui::Shape::dashed_line(
                &[egui::pos2(area.left(), y), egui::pos2(area.right(), y)],
                egui::Stroke::new(1.5, color),
                8.0,
                5.0,
            ));
            painter.text(
                egui::pos2(area.right() - 6.0, y - 3.0),
                egui::Align2::RIGHT_BOTTOM,
                label,
                font.clone(),
                color,
            );
        }

        // Curves: inactive first, active last.
        let mut ordered: Vec<&Series> = series
            .iter()
            .filter(|s| !self.hidden.contains(&s.key))
            .collect();
        ordered.sort_by_key(|s| s.key == active);
        for s in &ordered {
            let Some(points) = self.cache.get(&s.key).and_then(|c| c.points.get(&plot)) else {
                continue;
            };
            let is_active = s.key == active;
            let color = arrangement_color(s.port, s.breeder).gamma_multiply(if is_active {
                1.0
            } else {
                0.6
            });
            let stroke = egui::Stroke::new(if is_active { 2.8 } else { 1.5 }, color);
            let path: Vec<egui::Pos2> = points
                .iter()
                .map(|p| egui::pos2(map_x(p[0]), map_y(p[1])))
                .collect();
            if path.len() < 2 {
                continue;
            }
            if s.port {
                painter.add(egui::Shape::line(path, stroke));
            } else {
                painter.extend(egui::Shape::dashed_line(&path, stroke, 7.0, 4.0));
            }
        }

        // Magnet replacement starts, stacked per series along the bottom axis.
        for (row, s) in ordered.iter().enumerate() {
            let color = arrangement_color(s.port, s.breeder);
            let base = area.bottom() - 1.0 - row as f32 * 7.0;
            for (start, _) in self
                .cache
                .get(&s.key)
                .and_then(|c| c.magnet_spans.as_deref())
                .unwrap_or(&[])
            {
                let x = map_x(start / JULIAN_YEAR_SECONDS);
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        egui::pos2(x - 4.0, base),
                        egui::pos2(x + 4.0, base),
                        egui::pos2(x, base - 7.0),
                    ],
                    color,
                    egui::Stroke::new(0.8, egui::Color32::BLACK),
                ));
            }
        }

        // Scrub: click or drag sets the year.
        let to_year = |x: f32| {
            (f64::from((x - area.left()) / area.width()) * horizon_years).clamp(0.0, horizon_years)
        };
        if (response.clicked() || response.dragged())
            && let Some(p) = response.interact_pointer_pos()
        {
            *year = to_year(p.x);
        }
        let cursor_x = map_x(*year);
        painter.line_segment(
            [
                egui::pos2(cursor_x, area.top()),
                egui::pos2(cursor_x, area.bottom()),
            ],
            egui::Stroke::new(1.5, egui::Color32::from_gray(235)),
        );
        painter.text(
            egui::pos2(cursor_x, area.top() - 2.0),
            egui::Align2::CENTER_BOTTOM,
            format!("{:.1} y", *year),
            font.clone(),
            egui::Color32::from_gray(235),
        );
        let t_cursor = *year * JULIAN_YEAR_SECONDS;
        for s in &ordered {
            if let Some(v) = self
                .results
                .get(&s.key)
                .and_then(|h| snapshot_at(h, t_cursor))
                .and_then(|snap| plot.value(snap))
            {
                painter.circle_filled(
                    egui::pos2(cursor_x, map_y(v)),
                    if s.key == active { 4.5 } else { 3.0 },
                    arrangement_color(s.port, s.breeder),
                );
            }
        }

        // Hover.
        let hover = response
            .hover_pos()
            .filter(|p| area.expand2(egui::vec2(0.0, 6.0)).contains(*p))
            .map(|p| to_year(p.x));
        if let Some(hover_year) = hover {
            let x = map_x(hover_year);
            painter.line_segment(
                [egui::pos2(x, area.top()), egui::pos2(x, area.bottom())],
                egui::Stroke::new(1.0, egui::Color32::from_white_alpha(60)),
            );
            let t = hover_year * JULIAN_YEAR_SECONDS;
            response.on_hover_ui_at_pointer(|ui| {
                ui.strong(format!("{hover_year:.2} y"));
                for s in &ordered {
                    let color = arrangement_color(s.port, s.breeder);
                    let value = self
                        .results
                        .get(&s.key)
                        .and_then(|h| snapshot_at(h, t))
                        .and_then(|snap| plot.value(snap));
                    ui.horizontal(|ui| {
                        ui.colored_label(color, if s.port { "—" } else { "- -" });
                        ui.label(arrangement_label(s.port, s.breeder));
                        ui.strong(value.map_or("—".to_string(), |v| plot.format(v)));
                    });
                    let spans = self
                        .cache
                        .get(&s.key)
                        .and_then(|c| c.magnet_spans.as_deref())
                        .unwrap_or(&[]);
                    for (index, (start, end)) in spans.iter().enumerate() {
                        let (start_y, end_y) =
                            (start / JULIAN_YEAR_SECONDS, end / JULIAN_YEAR_SECONDS);
                        if (start_y - hover_year).abs() <= 0.3
                            || (start_y..=end_y).contains(&hover_year)
                        {
                            ui.colored_label(
                                color,
                                format!(
                                    "   Magnet replacement #{} · {:.1}–{:.1} y",
                                    index + 1,
                                    start_y,
                                    end_y
                                ),
                            );
                        }
                    }
                }
            });
        }
        outer
    }

    /// Wording of the service-limit line from the limit's declared provenance;
    /// a value moved away from the selected preset is always called authored.
    fn limit_label(&self, history: &HistoryResult, value: f64) -> String {
        let preset = self
            .presets
            .get(self.preset_index)
            .and_then(|p| service_limit(&p.assumptions, "magnets"));
        let literature = magnet_limit(history, preset).is_some_and(|(_, literature)| literature);
        if literature {
            format!("REBCO limit {} n/m² (literature)", fmt_sci(value))
        } else {
            format!("Magnet limit {} n/m² (authored limit)", fmt_sci(value))
        }
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

fn snapshot_at(history: &HistoryResult, time_s: f64) -> Option<&HistorySnapshot> {
    let i = history
        .snapshots
        .partition_point(|s| s.time_s <= time_s)
        .saturating_sub(1);
    history.snapshots.get(i)
}

fn service_limit(assumptions: &OperatingHistoryAssumptions, component: &str) -> Option<f64> {
    assumptions
        .service_limits
        .iter()
        .find(|l| l.component_id == component)
        .map(|l| l.limit)
}

fn replacement_days(assumptions: &OperatingHistoryAssumptions, component: &str) -> Option<f64> {
    assumptions
        .service_limits
        .iter()
        .find(|l| l.component_id == component)
        .and_then(|l| l.replacement_duration_s)
        .map(|s| (s / DAY_S).round())
}

/// Hover text for a preset: its key authored numbers and the start of its
/// provenance statement.
fn preset_note(a: &OperatingHistoryAssumptions, extra: Option<&str>) -> String {
    let mut parts = Vec::new();
    for l in &a.service_limits {
        let days = l
            .replacement_duration_s
            .map(|s| format!(", {:.0}-day swap", s / DAY_S))
            .unwrap_or_default();
        parts.push(format!(
            "{} service limit {} n/m²{days}",
            l.component_id,
            fmt_sci(l.limit)
        ));
    }
    if parts.is_empty() {
        parts.push("no service limits".into());
    }
    parts.push(format!(
        "recovery {:.0}%, processing delay {:.1} d",
        a.recovery_fraction * 100.0,
        a.processing_delay_s / DAY_S
    ));
    let mut note = parts.join(" · ");
    if let Some(extra) = extra {
        note = format!("{extra}\n{note}");
    }
    let provenance: String = a
        .provenance
        .split_inclusive(". ")
        .take(2)
        .collect::<String>();
    format!("{note}\n\nProvenance: {}", provenance.trim())
}

fn event_label(event: &HistoryEvent) -> String {
    let what = match event.kind {
        EventKind::Imported => "Tritium import",
        EventKind::ProcessingReleased => "Processed tritium released",
        EventKind::OperationStarted => "Operation started",
        EventKind::OperationStopped => "Operation stopped",
        EventKind::FuelUnavailable => "Fuel-limited stop",
        EventKind::FuelAvailable => "Fuel available again",
        EventKind::PlannedOutageStarted => "Planned outage starts",
        EventKind::PlannedOutageEnded => "Planned outage ends",
        EventKind::ServiceLimitReached => "Service limit reached",
        EventKind::ReplacementStarted => "Replacement starts",
        EventKind::ReplacementCompleted => "Replacement completed",
    };
    match event.component_id.as_deref() {
        Some(component) => format!(
            "{:.2} y · {what} · {component}",
            event.time_s / JULIAN_YEAR_SECONDS
        ),
        None => format!("{:.2} y · {what}", event.time_s / JULIAN_YEAR_SECONDS),
    }
}

/// Series colour: port is warm and no-port cool; the breeder-heavy allocation
/// is the lighter shade of each family (the engine owns the palette).
pub fn arrangement_color(port: bool, breeder: bool) -> egui::Color32 {
    let [r, g, b] = Arrangement { port, breeder }.rgb();
    egui::Color32::from_rgb(r, g, b)
}

pub fn arrangement_label(port: bool, breeder: bool) -> &'static str {
    Arrangement { port, breeder }.label()
}

/// Header line of a what-if row. Returns true when "revert to preset" is clicked.
fn row_header(ui: &mut egui::Ui, label: &str, value: &str, edited: bool) -> bool {
    let mut revert = false;
    ui.horizontal_wrapped(|ui| {
        ui.label(label);
        ui.label(egui::RichText::new(value).strong());
        if edited {
            badge::badge(
                ui,
                Kind::Authored,
                "edited",
                "You moved this authored value away from the selected preset. It stays an authored scenario assumption and is not a qualified input.",
            );
            if ui.small_button("revert to preset").clicked() {
                revert = true;
            }
        }
    });
    revert
}

fn full_slider(
    ui: &mut egui::Ui,
    value: &mut f64,
    range: std::ops::RangeInclusive<f64>,
    logarithmic: bool,
    step: Option<f64>,
) -> egui::Response {
    ui.scope(|ui| {
        ui.spacing_mut().slider_width = (ui.available_width() - 8.0).max(80.0);
        let mut slider = egui::Slider::new(value, range)
            .show_value(false)
            .clamping(egui::SliderClamping::Edits);
        if logarithmic {
            slider = slider.logarithmic(true);
        }
        if let Some(step) = step {
            slider = slider.step_by(step);
        }
        ui.add(slider)
    })
    .inner
}

fn log_slider(
    ui: &mut egui::Ui,
    value: &mut f64,
    range: std::ops::RangeInclusive<f64>,
) -> egui::Response {
    full_slider(ui, value, range, true, None)
}

/// Round axis limits outward to a step of 1, 2 or 5 times a power of ten.
/// Returns (minimum, maximum, step).
fn nice_axis(lo: f64, hi: f64, target_ticks: usize) -> (f64, f64, f64) {
    let span = (hi - lo).max(f64::MIN_POSITIVE);
    let raw = span / target_ticks.max(1) as f64;
    let magnitude = 10f64.powf(raw.log10().floor());
    let normalized = raw / magnitude;
    let step = magnitude
        * if normalized <= 1.0 {
            1.0
        } else if normalized <= 2.0 {
            2.0
        } else if normalized <= 5.0 {
            5.0
        } else {
            10.0
        };
    ((lo / step).floor() * step, (hi / step).ceil() * step, step)
}

fn superscript(n: i32) -> String {
    n.to_string()
        .chars()
        .map(|c| match c {
            '-' => '⁻',
            '0' => '⁰',
            '1' => '¹',
            '2' => '²',
            '3' => '³',
            '4' => '⁴',
            '5' => '⁵',
            '6' => '⁶',
            '7' => '⁷',
            '8' => '⁸',
            '9' => '⁹',
            other => other,
        })
        .collect()
}

/// Scientific notation such as 3×10²².
pub fn fmt_sci(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    if !v.is_finite() {
        return "—".into();
    }
    let sign = if v < 0.0 { "−" } else { "" };
    let a = v.abs();
    let mut exponent = a.log10().floor() as i32;
    let mut mantissa = (a / 10f64.powi(exponent) * 10.0).round() / 10.0;
    if mantissa >= 10.0 {
        mantissa /= 10.0;
        exponent += 1;
    }
    let mantissa = if (mantissa - mantissa.round()).abs() < 1e-9 {
        format!("{}", mantissa.round() as i64)
    } else {
        format!("{mantissa:.1}")
    };
    if exponent == 0 {
        format!("{sign}{mantissa}")
    } else {
        format!("{sign}{mantissa}×10{}", superscript(exponent))
    }
}

fn fmt_tick(plot: Plot, v: f64, step: f64) -> String {
    if plot == Plot::MagnetFluence {
        return fmt_sci(v);
    }
    let decimals = (-step.log10().floor()).max(0.0) as usize;
    format!("{v:.decimals$}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scientific_notation_uses_superscripts() {
        assert_eq!(fmt_sci(3e22), "3×10²²");
        assert_eq!(fmt_sci(1e25), "1×10²⁵");
        assert_eq!(fmt_sci(2.5e22), "2.5×10²²");
        assert_eq!(fmt_sci(9.96e22), "1×10²³");
        assert_eq!(fmt_sci(0.0), "0");
        assert_eq!(fmt_sci(4e-3), "4×10⁻³");
    }

    #[test]
    fn axis_steps_are_nice_and_cover_the_range() {
        let (lo, hi, step) = nice_axis(0.0, 3.45e22, 5);
        assert_eq!((lo, step), (0.0, 1e22));
        assert!((3.45e22..=4e22 + 1.0).contains(&hi));
        let (lo, hi, step) = nice_axis(-1.0, 36.0, 5);
        assert_eq!(step, 10.0);
        assert!(lo <= -1.0 && hi >= 36.0);
        let ticks = ((hi - lo) / step).round() as i32;
        assert!((3..=8).contains(&ticks));
    }

    #[test]
    fn preset_edits_are_detected_with_tolerance() {
        assert!(!differs(3e22, 3e22 * (1.0 + 1e-12)));
        assert!(differs(3e22, 3.1e22));
    }
}
