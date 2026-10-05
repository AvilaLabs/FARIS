//! The Monte Carlo uncertainty of the operating history, as the history panel
//! shows it: ranges beside the nominal outputs, bands on the timeline, the
//! progress of the background job, and what it hands to the study file.
//!
//! Every range is transport Monte Carlo sampling uncertainty only; each
//! display says so in one line, with the detail on hover.

use super::*;
use faris_engine::history_ensemble::EnsembleComparison;
use std::sync::Arc;

impl HistoryPanel {
    /// True when the histories on screen belong to the current inputs and
    /// nothing is being recalculated, so an ensemble may be started and shown.
    pub(super) fn settled(&self) -> bool {
        !self.results.is_empty()
            && self.pending.is_none()
            && self.debounce.is_none()
            && !self.requested
            && !self.edited
            && self.validation.is_none()
            && self.error.is_none()
    }

    /// Tell the panel which arrangement is drawn in 3D; its ensemble is
    /// calculated first.
    pub fn set_selected(&mut self, scenario: &str, variant: &str) {
        let id = key(scenario, variant);
        if self.selected != id {
            self.selected = id;
        }
    }

    /// Once per frame: start, collect or cancel the background ensembles.
    pub fn drive_uncertainty(&mut self, ctx: &egui::Context) {
        if !self.settled() {
            self.uncertainty.invalidate();
            return;
        }
        let mut inputs: Vec<(String, &HistoryResult)> = self
            .results
            .iter()
            .map(|(id, history)| (id.clone(), history))
            .collect();
        // Stable: the selected arrangement first, the rest in key order.
        inputs.sort_by_key(|(id, _)| *id != self.selected);
        if self.uncertainty.sync(ctx, self.revision, &inputs) {
            ctx.request_repaint();
        }
    }

    /// The state of one arrangement's ensemble; None while the history is
    /// being recalculated (a range for earlier inputs is never shown).
    pub fn uncertainty_status(&self, scenario: &str, variant: &str) -> Option<Status> {
        self.settled()
            .then(|| self.uncertainty.status(&key(scenario, variant)))
    }

    /// The finished ensemble of an arrangement when it was evaluated.
    fn display_ensemble(&self, id: &str) -> Option<Arc<HistoryEnsemble>> {
        if !self.settled() {
            return None;
        }
        self.uncertainty
            .ensemble(id)
            .filter(|e| e.status == EnsembleStatus::Evaluated)
    }

    /// Paired comparison of two arrangements' ensembles, second minus first.
    pub fn ensemble_comparison(
        &self,
        a: (&str, &str),
        b: (&str, &str),
    ) -> Option<Result<Arc<EnsembleComparison>, String>> {
        if !self.settled() {
            return None;
        }
        self.uncertainty.comparison(&key(a.0, a.1), &key(b.0, b.1))
    }

    /// Finished ensembles for the study file, with the key each was calculated
    /// under. Empty while the histories are unsettled and in fixture sessions.
    pub fn ensemble_drafts(&self) -> Vec<faris_study::EnsembleDraft> {
        if !self.settled() {
            return Vec::new();
        }
        self.uncertainty
            .persistable()
            .into_iter()
            .filter_map(|(id, key, ensemble)| {
                let (scenario, variant) = id.rsplit_once("::")?;
                Some(faris_study::EnsembleDraft {
                    scenario_sha256: scenario.to_owned(),
                    variant: variant.to_owned(),
                    key,
                    ensemble,
                })
            })
            .collect()
    }

    /// Offer the ensembles stored in an opened study file. They are used only
    /// where their key equals the key of the loaded inputs.
    pub fn restore_ensembles(&mut self, stored: Vec<faris_study::StoredEnsemble>) {
        self.uncertainty
            .seed(stored.into_iter().map(|s| (s.key, s.ensemble)).collect());
    }

    /// Read-only state for the development interface check.
    pub(super) fn interface_uncertainty(&self, id: &str) -> serde_json::Value {
        let status = if !self.settled() {
            "unsettled".to_owned()
        } else {
            match self.uncertainty.status(id) {
                Status::NotPlanned => "not_planned".into(),
                Status::Waiting => "waiting".into(),
                Status::Queued => "queued".into(),
                Status::Running { done, total } => format!("running {done}/{total}"),
                Status::Ready(e) => match &e.status {
                    EnsembleStatus::Evaluated => "evaluated".into(),
                    EnsembleStatus::NotEvaluated { .. } => "not_evaluated".into(),
                },
                Status::Failed(_) => "failed".into(),
            }
        };
        serde_json::json!({
            "status": status,
            "samples_setting": self.uncertainty.samples(),
            "runs_started": self.uncertainty.runs_started(),
            "fixture": self.uncertainty.is_fixture(),
        })
    }

    /// Development checks only: give every record a synthetic covariance so
    /// the evaluated views can be seen. Never stored, never in a normal build.
    #[cfg(feature = "uncertainty-fixture")]
    pub fn enable_uncertainty_fixture(&mut self) {
        self.uncertainty = Uncertainty::default().with_rates_transform(fixture_rates);
    }

    /// True while any arrangement's ensemble is still to be calculated, so an
    /// export waits for the ranges rather than leaving them out.
    pub fn uncertainty_pending(&self) -> bool {
        self.settled()
            && self.results.keys().any(|id| {
                matches!(
                    self.uncertainty.status(id),
                    Status::NotPlanned | Status::Waiting | Status::Queued | Status::Running { .. }
                )
            })
    }

    /// The ensemble of an arrangement as the export takes it.
    pub fn export_ensemble(&self, scenario: &str, variant: &str) -> faris_report::EnsembleInput {
        match self.uncertainty_status(scenario, variant) {
            Some(Status::Ready(ensemble)) => faris_report::EnsembleInput::Ready(ensemble),
            Some(Status::Failed(why)) => faris_report::EnsembleInput::Failed(why),
            _ => faris_report::EnsembleInput::None,
        }
    }

    /// The label of an arrangement from the transport name it is bound to.
    fn arrangement_name(&self, id: &str) -> &'static str {
        let name = self.names.get(id).map_or("", String::as_str);
        arrangement_label(name.contains("penetration"), name.starts_with("breeder"))
    }

    /// The "Uncertainty in the history" section of the Operate step: the
    /// sample setting, the progress, and every history output beside its
    /// range or distribution.
    pub fn uncertainty_controls(&mut self, ui: &mut egui::Ui, scenario: &str, variant: &str) {
        let active = key(scenario, variant);
        ui.add_space(8.0);
        ui.separator();
        ui.strong("Uncertainty in the history");
        scope_line(ui);
        if self.results.is_empty() {
            ui.weak("Uncertainty: calculated after the history.");
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label("Samples");
            let mut samples = self.uncertainty.samples();
            for choice in SAMPLE_CHOICES {
                ui.radio_value(&mut samples, choice, choice.to_string());
            }
            self.uncertainty.set_samples(samples);
        });
        ui.small("1000 gives narrower sampling error and takes about 5× longer.");

        let Some(status) = self.uncertainty_status(scenario, variant) else {
            ui.weak("Uncertainty: starts when the history has settled.");
            return;
        };
        match status {
            Status::NotPlanned | Status::Waiting => {
                ui.horizontal_wrapped(|ui| {
                    ui.spinner();
                    ui.weak("Uncertainty: starting…");
                });
            }
            Status::Queued => {
                let running =
                    self.results
                        .keys()
                        .find_map(|id| match self.uncertainty.status(id) {
                            Status::Running { done, total } => Some((id.clone(), done, total)),
                            _ => None,
                        });
                ui.horizontal_wrapped(|ui| {
                    ui.spinner();
                    match running {
                        Some((id, done, total)) => ui.weak(format!(
                            "{} · {}",
                            progress_text(done, total),
                            self.arrangement_name(&id)
                        )),
                        None => ui.weak("Uncertainty: waiting for another arrangement."),
                    };
                });
            }
            Status::Running { done, total } => {
                ui.horizontal_wrapped(|ui| {
                    ui.spinner();
                    ui.label(progress_text(done, total));
                });
                ui.add(
                    egui::ProgressBar::new(done as f32 / total.max(1) as f32).desired_height(6.0),
                );
            }
            Status::Failed(why) => {
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    format!("Uncertainty could not be calculated: {why}"),
                );
            }
            Status::Ready(ensemble) => {
                let Some(history) = self.results.get(&active) else {
                    return;
                };
                if let Some((why, next_step)) = not_evaluated_text(&ensemble) {
                    ui.colored_label(
                        Kind::Partial.color(),
                        format!("No uncertainty range: {why}"),
                    );
                    ui.label(format!("Next step: {next_step}"));
                }
                ui.small(format!(
                    "{} samples · {}",
                    ensemble.samples.len(),
                    self.arrangement_name(&active)
                ));
                rows_ui(ui, &history_rows(history, Some(&ensemble)));
            }
        }
    }

    /// A short status for the timeline header: progress while an ensemble
    /// runs, otherwise what the bands on the plot are.
    pub(super) fn uncertainty_header(&self, ui: &mut egui::Ui, scenario: &str, variant: &str) {
        let Some(status) = self.uncertainty_status(scenario, variant) else {
            return;
        };
        let running = self
            .results
            .keys()
            .find_map(|id| match self.uncertainty.status(id) {
                Status::Running { done, total } => Some((done, total)),
                _ => None,
            });
        if let Some((done, total)) = running {
            ui.spinner();
            ui.small(progress_text(done, total));
            return;
        }
        match status {
            Status::Ready(ensemble) => match not_evaluated_text(&ensemble) {
                None => {
                    badge::badge(
                        ui,
                        Kind::Conditional,
                        "bands: P5–P95, transport sampling only",
                        SCOPE_DETAIL,
                    );
                }
                Some((why, next_step)) => {
                    badge::badge(
                        ui,
                        Kind::NotEvaluated,
                        "no uncertainty range",
                        &format!("{why}.\nNext step: {next_step}."),
                    );
                }
            },
            Status::Failed(why) => {
                badge::badge(ui, Kind::Failed, "uncertainty failed", &why);
            }
            _ => {}
        }
    }

    /// Hover lines for the band of `plot` at `year`, for one arrangement.
    pub(super) fn band_hover(
        &self,
        ui: &mut egui::Ui,
        id: &str,
        plot: Plot,
        year: f64,
        color: egui::Color32,
    ) {
        let Some(ensemble) = self.display_ensemble(id) else {
            return;
        };
        let Some((grid, band, scale)) = plot_band(plot, &ensemble) else {
            return;
        };
        let t = year * JULIAN_YEAR_SECONDS;
        let last = grid.len().saturating_sub(1).max(1);
        let index = ((t / grid[last]) * last as f64)
            .round()
            .clamp(0.0, last as f64) as usize;
        let (Some(lo), Some(hi)) = (
            band.p5.get(index).copied().flatten(),
            band.p95.get(index).copied().flatten(),
        ) else {
            ui.colored_label(color, "   no uncertainty band here (no sample has a value)");
            return;
        };
        ui.colored_label(
            color,
            format!(
                "   P5–P95 {} to {}",
                plot.format(lo * scale),
                plot.format(hi * scale)
            ),
        );
        let samples = ensemble.samples.len() as u32;
        if let Some(note) = band_coverage_note(band.n[index], samples) {
            ui.colored_label(color, format!("   {note}"));
        }
    }

    /// The plotted values the bands of `plot` reach, for the vertical range.
    pub(super) fn band_extent(&self, plot: Plot) -> Option<(f64, f64)> {
        let mut extent: Option<(f64, f64)> = None;
        for id in self.results.keys() {
            let Some(ensemble) = self.display_ensemble(id) else {
                continue;
            };
            let Some((_, band, scale)) = plot_band(plot, &ensemble) else {
                continue;
            };
            for v in band.p5.iter().chain(&band.p95).flatten() {
                let v = v * scale;
                extent = Some(extent.map_or((v, v), |(lo, hi)| (lo.min(v), hi.max(v))));
            }
        }
        extent
    }

    /// Draw the shaded P5–P95 band of one arrangement. A point with no value
    /// breaks the band.
    pub(super) fn draw_band(
        &self,
        painter: &egui::Painter,
        id: &str,
        plot: Plot,
        color: egui::Color32,
        map_x: &dyn Fn(f64) -> f32,
        map_y: &dyn Fn(f64) -> f32,
    ) {
        let Some(ensemble) = self.display_ensemble(id) else {
            return;
        };
        let Some((grid, band, scale)) = plot_band(plot, &ensemble) else {
            return;
        };
        let mut mesh = egui::Mesh::default();
        let mut previous: Option<u32> = None;
        for (k, t) in grid.iter().enumerate() {
            let (Some(lo), Some(hi)) = (
                band.p5.get(k).copied().flatten(),
                band.p95.get(k).copied().flatten(),
            ) else {
                previous = None;
                continue;
            };
            let x = map_x(t / JULIAN_YEAR_SECONDS);
            let i = mesh.vertices.len() as u32;
            mesh.colored_vertex(egui::pos2(x, map_y(lo * scale)), color);
            mesh.colored_vertex(egui::pos2(x, map_y(hi * scale)), color);
            if let Some(p) = previous {
                mesh.add_triangle(p, p + 1, i);
                mesh.add_triangle(p + 1, i + 1, i);
            }
            previous = Some(i);
        }
        painter.add(egui::Shape::mesh(mesh));
    }
}

/// The recorded rates with a synthetic covariance consistent with their
/// standard errors (correlation 0.4 between every pair).
#[cfg(feature = "uncertainty-fixture")]
fn fixture_rates(rates: &TransportDrivingRates) -> TransportDrivingRates {
    let mut fixed = rates.clone();
    fixed.covariance = Some(faris_engine::fixtures::synthetic_covariance(rates, 0.4));
    fixed
}

/// "Transport Monte Carlo sampling uncertainty only…", with the detail on hover.
fn scope_line(ui: &mut egui::Ui) {
    ui.small(SCOPE_LINE).on_hover_ui(|ui| {
        ui.set_max_width(360.0);
        ui.label(SCOPE_DETAIL);
    });
}

/// Each output with its nominal value and its range or distribution.
fn rows_ui(ui: &mut egui::Ui, rows: &[UncertaintyRow]) {
    for row in rows {
        ui.add_space(3.0);
        ui.horizontal_wrapped(|ui| {
            ui.strong(&row.label);
            ui.monospace(format!("nominal {}", row.nominal));
        });
        match &row.result {
            Some(result) => {
                ui.label(&result.text).on_hover_ui(|ui| {
                    ui.set_max_width(380.0);
                    ui.label(&result.detail);
                    ui.add_space(4.0);
                    ui.small(SCOPE_LINE);
                });
            }
            None => {
                ui.weak("no uncertainty range");
            }
        }
    }
}
