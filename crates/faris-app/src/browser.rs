//! The browser version's own surface: the first-open card, the About text and
//! the one-line reasons shown where a desktop feature is not available.
//!
//! The browser build shows recorded results. Anything that starts a program
//! (transport runs, CAD design checks, weight windows, ACTINV, Avila Core)
//! stays in the desktop app; those controls are disabled with the reason and
//! the link to it.

use eframe::egui;

/// Where the desktop app is downloaded.
pub const DESKTOP_URL: &str = "https://github.com/AvilaLabs/FARIS/releases";

/// Whether this is the browser build.
pub const IS_BROWSER: bool = cfg!(target_arch = "wasm32");

/// The features the browser version leaves to the desktop app.
pub const DESKTOP_ONLY: &[&str] = &[
    "Running new transport (OpenMC) and weight windows",
    "Design check and CAD import from STEP files, and Design flux",
    "Maintenance and ACTINV runs",
    "Compile study and Core stage runs (Avila Core)",
];

/// The one-line reason a feature is unavailable in the browser.
pub fn desktop_only(feature: &str) -> String {
    format!(
        "{feature} needs the desktop app, because it starts programs on your computer and a web page cannot. Get it at {DESKTOP_URL}"
    )
}

/// A disabled-feature note for a panel: the reason in text, with the link.
pub fn desktop_only_note(ui: &mut egui::Ui, feature: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.weak(format!(
            "{feature} needs the desktop app, because it starts programs on your computer."
        ));
        ui.hyperlink_to("Get the desktop app", DESKTOP_URL);
    });
}

/// The paragraphs of the first-open card and the About window.
pub const CARD_PARAGRAPHS: [&str; 3] = [
    "This is the browser version of FARIS. It shows recorded results and everything you can explore from them: the 3D reactor, the transport fields, the 30-year operating history, uncertainty, comparisons and the allocation sweep.",
    "It cannot run new transport, check CAD designs, or run ACTINV or Avila Core. Those need the desktop app.",
    "Your files stay in this browser tab; nothing is uploaded. Signing in with your Avila Labs account is optional.",
];

/// What the card asked for this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    None,
    /// "Start the tour" or "Replay the tour".
    StartTour,
    /// Closed without the tour.
    Closed,
}

/// The card shown on first open and from the About item.
#[derive(Default)]
pub struct Notice {
    pub open: bool,
    /// True while this is the first open: the buttons then lead into the tour.
    first_visit: bool,
}

impl Notice {
    /// Show the card because this is the first open.
    pub fn open_first_visit(&mut self) {
        self.open = true;
        self.first_visit = true;
    }

    /// Show the card again from the About item or the command palette.
    pub fn open_about(&mut self) {
        self.open = true;
        self.first_visit = false;
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome {
        if !self.open {
            return Outcome::None;
        }
        let mut outcome = Outcome::None;
        egui::Area::new(egui::Id::new("browser-notice-veil"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::Pos2::ZERO)
            .show(ctx, |ui| {
                let screen = ctx.content_rect();
                ui.painter()
                    .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(150));
                ui.allocate_rect(screen, egui::Sense::click_and_drag());
            });
        egui::Window::new("About this browser version")
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Tooltip)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.set_max_width(460.0);
                ui.heading("FARIS in your browser");
                ui.add_space(6.0);
                for paragraph in CARD_PARAGRAPHS {
                    ui.add(egui::Label::new(paragraph).wrap());
                    ui.add_space(4.0);
                }
                ui.weak("Not available here:");
                for feature in DESKTOP_ONLY {
                    ui.weak(format!("  {feature}"));
                }
                ui.add_space(4.0);
                ui.hyperlink_to("Get the desktop app", DESKTOP_URL);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let tour = if self.first_visit {
                        "Start the tour"
                    } else {
                        "Replay the tour"
                    };
                    if ui.button(tour).clicked() {
                        outcome = Outcome::StartTour;
                    }
                    let close = if self.first_visit {
                        "Explore without the tour"
                    } else {
                        "Close"
                    };
                    if ui.button(close).clicked() {
                        outcome = Outcome::Closed;
                    }
                });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            outcome = Outcome::Closed;
        }
        if outcome != Outcome::None {
            self.open = false;
            self.first_visit = false;
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_is_plain_and_names_the_desktop_app() {
        let all = CARD_PARAGRAPHS.join(" ").to_lowercase();
        for banned in ["free", "price", "$", "upload your", "revolution"] {
            assert!(!all.contains(banned), "card must not say {banned:?}");
        }
        assert!(all.contains("desktop app"));
        assert!(all.contains("nothing is uploaded"));
        assert!(all.contains("optional"));
    }

    #[test]
    fn every_desktop_only_reason_links_the_desktop_app() {
        for feature in DESKTOP_ONLY {
            let reason = desktop_only(feature);
            assert!(reason.contains(DESKTOP_URL));
            assert!(reason.contains("needs the desktop app"));
        }
    }

    #[test]
    fn card_state_follows_the_open_kind() {
        let mut notice = Notice::default();
        assert!(!notice.open);
        notice.open_first_visit();
        assert!(notice.open && notice.first_visit);
        notice.open_about();
        assert!(notice.open && !notice.first_visit);
    }
}
