//! Compact provenance and status badges.
//!
//! A badge keeps a scientific qualifier attached to a value without letting the
//! qualifier dominate the screen: the short label stays visible, and the full
//! explanation (why, and what would settle it) is always available on hover.
//! Never use a badge to hide a FAIL or to imply a PASS that was not evaluated.

use eframe::egui;

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
    pub fn color(self) -> egui::Color32 {
        match self {
            Kind::Calculated => egui::Color32::from_rgb(96, 165, 250),
            Kind::Checked => egui::Color32::from_rgb(74, 196, 140),
            Kind::Authored => egui::Color32::from_rgb(196, 160, 250),
            Kind::Literature => egui::Color32::from_rgb(110, 200, 210),
            Kind::Conditional => egui::Color32::from_rgb(170, 176, 190),
            Kind::Partial => egui::Color32::from_rgb(232, 178, 92),
            Kind::NotEvaluated => egui::Color32::from_rgb(140, 146, 160),
            Kind::Failed => egui::Color32::from_rgb(240, 110, 110),
        }
    }

    pub fn default_label(self) -> &'static str {
        match self {
            Kind::Calculated => "calculated",
            Kind::Checked => "checked",
            Kind::Authored => "authored",
            Kind::Literature => "literature",
            Kind::Conditional => "conditional",
            Kind::Partial => "partial",
            Kind::NotEvaluated => "not evaluated",
            Kind::Failed => "failed",
        }
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
