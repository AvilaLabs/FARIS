//! Compact provenance and status badges.
//!
//! A badge keeps a scientific qualifier attached to a value without letting the
//! qualifier dominate the screen: the short label stays visible, and the full
//! explanation (why, and what would settle it) is always available on hover.
//! Never use a badge to hide a FAIL or to imply a PASS that was not evaluated.

use eframe::egui;
use faris_engine::brief::StatusKind;

/// What kind of statement a value or result is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Calculated by the FARIS engine or a recorded solver run.
    Calculated,
    /// A numerical control or check that passed within its declared scope.
    Checked,
    /// An authored scenario assumption (tunable, not measured).
    Authored,
    /// A value taken from cited literature.
    Literature,
    /// Valid only under stated conditions (e.g. a cold-data surrogate).
    Conditional,
    /// Precision or coverage goal not yet met; result usable with care.
    Partial,
    /// Not evaluated; no claim either way.
    NotEvaluated,
    /// A failed check or an error.
    Failed,
}

impl Kind {
    /// The engine's status kind, which owns the colours and labels shared with
    /// the exported summary.
    fn status(self) -> StatusKind {
        match self {
            Kind::Calculated => StatusKind::Calculated,
            Kind::Checked => StatusKind::Checked,
            Kind::Authored => StatusKind::Authored,
            Kind::Literature => StatusKind::Literature,
            Kind::Conditional => StatusKind::Conditional,
            Kind::Partial => StatusKind::Partial,
            Kind::NotEvaluated => StatusKind::NotEvaluated,
            Kind::Failed => StatusKind::Failed,
        }
    }

    pub fn color(self) -> egui::Color32 {
        let [r, g, b] = self.status().rgb();
        egui::Color32::from_rgb(r, g, b)
    }

    pub fn default_label(self) -> &'static str {
        self.status().label()
    }
}

/// Draw a small pill with `label`; `explanation` is shown on hover.
/// The explanation should say why the status applies and, where possible,
/// what would change it.
pub fn badge(ui: &mut egui::Ui, kind: Kind, label: &str, explanation: &str) -> egui::Response {
    let color = kind.color();
    let text = egui::RichText::new(label).small().color(color);
    let frame = egui::Frame::new()
        .fill(color.gamma_multiply(0.14))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.55)))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(6, 1));
    let response = frame
        .show(ui, |ui| ui.add(egui::Label::new(text).selectable(false)))
        .response
        .interact(egui::Sense::hover());
    if explanation.is_empty() {
        response
    } else {
        response.on_hover_ui(|ui| {
            ui.set_max_width(360.0);
            ui.label(egui::RichText::new(label).strong().color(color));
            ui.label(explanation);
        })
    }
}

/// Badge using the kind's default label.
pub fn kind_badge(ui: &mut egui::Ui, kind: Kind, explanation: &str) -> egui::Response {
    badge(ui, kind, kind.default_label(), explanation)
}

/// A headline value with a caption, unit and status badge, for result cards.
pub fn metric(
    ui: &mut egui::Ui,
    caption: &str,
    value: &str,
    kind: Kind,
    explanation: &str,
) -> egui::Response {
    ui.vertical(|ui| {
        ui.weak(caption);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(value).size(18.0).strong());
            kind_badge(ui, kind, explanation);
        });
    })
    .response
}
