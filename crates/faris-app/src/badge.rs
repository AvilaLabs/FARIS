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

    /// How firmly a value of this kind is established, for taking the weakest of several inputs
    /// (UNC-016): higher is firmer. A solver result or a passed check is firmest, then a value
    /// cited from the literature, then one valid only under stated conditions, then a value the
    /// author chose; the rest are statuses, not kinds of input, and rank below them.
    pub fn firmness(self) -> u8 {
        match self {
            Kind::Calculated | Kind::Checked => 5,
            Kind::Literature => 4,
            Kind::Conditional => 3,
            Kind::Authored => 2,
            Kind::Partial => 1,
            Kind::NotEvaluated | Kind::Failed => 0,
        }
    }

    /// The least firm of the kinds, or `None` for none.
    pub fn weakest(kinds: impl IntoIterator<Item = Kind>) -> Option<Kind> {
        kinds.into_iter().min_by_key(|k| k.firmness())
    }
}

/// Shows `add_contents` as the explanation of `response` on hover, and also from the keyboard
/// and by tap (A11Y-009): while the widget has keyboard focus the explanation is shown, and
/// Enter, Space or a click pins it open until pressed again or Escape. The response must have
/// been made with [`egui::Sense::click`] so it can take focus.
pub fn explain(
    response: egui::Response,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let pinned_id = response.id.with("explanation-pinned");
    let ctx = response.ctx.clone();
    let mut pinned = ctx.data(|d| d.get_temp::<bool>(pinned_id)).unwrap_or(false);
    if response.clicked() {
        pinned = !pinned;
    }
    if pinned && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        pinned = false;
    }
    ctx.data_mut(|d| d.insert_temp(pinned_id, pinned));
    if response.hovered() {
        response.on_hover_ui(add_contents)
    } else if response.has_focus() || pinned {
        egui::Tooltip::for_widget(&response).show(add_contents);
        response
    } else {
        response
    }
}

/// Draw a small pill with `label`; `explanation` is shown on hover, and from the keyboard (the
/// pill takes focus) or a tap (A11Y-009).
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
    let shown = frame
        .show(ui, |ui| ui.add(egui::Label::new(text).selectable(false)))
        .response;
    // A separate interaction area over the pill, so it can take keyboard focus (A11Y-009).
    let response = ui.interact(shown.rect, shown.id.with("badge"), egui::Sense::click());
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.expand(1.0),
            8.0,
            ui.visuals().selection.stroke,
            egui::StrokeKind::Outside,
        );
    }
    let accessible = if explanation.is_empty() {
        label.to_owned()
    } else {
        format!("{label}. {explanation}")
    };
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), accessible.clone())
    });
    if explanation.is_empty() {
        response
    } else {
        explain(response, |ui| {
            ui.set_max_width(360.0);
            ui.label(egui::RichText::new(label).strong().color(color));
            ui.label(explanation);
        })
    }
}

/// A small weak text with an explanation that opens by hover, keyboard focus or tap (A11Y-009).
pub fn note(ui: &mut egui::Ui, text: &str, explanation: &str) -> egui::Response {
    let response = ui.add(
        egui::Label::new(egui::RichText::new(text).small().weak()).sense(egui::Sense::click()),
    );
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.expand(1.0),
            2.0,
            ui.visuals().selection.stroke,
            egui::StrokeKind::Outside,
        );
    }
    explain(response, |ui| {
        ui.set_max_width(360.0);
        ui.label(explanation);
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs one frame drawing a badge; returns its id and whether its explanation was open the
    /// frame before.
    fn frame(ctx: &egui::Context, events: Vec<egui::Event>) -> (egui::Id, bool) {
        let mut seen = None;
        let input = egui::RawInput {
            events,
            focused: true,
            ..Default::default()
        };
        ctx.run_ui(input, |ui| {
            let response = kind_badge(ui, Kind::Authored, "Chosen by the author, not measured.");
            seen = Some((response.id, response.is_tooltip_open()));
        })
        .drop_without_applying_deltas();
        seen.unwrap()
    }

    fn key(key: egui::Key, pressed: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    // Verifies: A11Y-009
    #[test]
    fn a_badge_explanation_opens_from_the_keyboard_without_hover() {
        let ctx = egui::Context::default();
        let (id, open) = frame(&ctx, vec![]);
        assert!(!open, "closed until asked for");
        ctx.memory_mut(|m| m.request_focus(id));
        frame(&ctx, vec![]);
        frame(&ctx, vec![]);
        let (_, open) = frame(&ctx, vec![]);
        assert!(open, "a focused badge shows its explanation");
        ctx.memory_mut(|m| m.surrender_focus(id));
        frame(&ctx, vec![]);
        let (_, open) = frame(&ctx, vec![]);
        assert!(!open, "losing focus closes it");
    }

    // Verifies: A11Y-009
    // Verifies: A11Y-025
    #[test]
    fn enter_pins_the_explanation_and_escape_closes_it() {
        let ctx = egui::Context::default();
        let (id, _) = frame(&ctx, vec![]);
        ctx.memory_mut(|m| m.request_focus(id));
        frame(&ctx, vec![]);
        frame(&ctx, vec![key(egui::Key::Enter, true)]);
        frame(&ctx, vec![key(egui::Key::Enter, false)]);
        // Focus moves away; the pinned explanation stays.
        ctx.memory_mut(|m| m.surrender_focus(id));
        frame(&ctx, vec![]);
        let (_, open) = frame(&ctx, vec![]);
        assert!(open, "a pinned explanation stays after focus leaves");
        frame(&ctx, vec![key(egui::Key::Escape, true)]);
        frame(&ctx, vec![key(egui::Key::Escape, false)]);
        let (_, open) = frame(&ctx, vec![]);
        assert!(!open, "Escape closes it");
    }

    #[test]
    fn weakest_kind_orders_by_firmness() {
        assert_eq!(
            Kind::weakest([Kind::Calculated, Kind::Authored, Kind::Literature]),
            Some(Kind::Authored)
        );
        assert_eq!(Kind::weakest(Vec::new()), None);
    }
}
