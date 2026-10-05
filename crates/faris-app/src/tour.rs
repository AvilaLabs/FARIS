//! Guided tour: a dimmed overlay with a spotlight on one part of the window and
//! a short explanation card. The tour only presents the app; each stop names
//! the step, field view and year it talks about, and `main.rs` applies them.

use crate::{Step, transport_panel::FieldView};
use eframe::egui;
use std::{
    collections::HashMap,
    ffi::OsString,
    path::{Path, PathBuf},
};

/// Whether the tour plays on launch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum TourMode {
    /// Play the first time (marker file absent), except in scripted runs.
    #[default]
    Auto,
    Always,
    Never,
}

/// Decide whether the tour starts on launch. Scripted runs (capture,
/// benchmark, interface check) never auto-play; `always` overrides that.
pub fn should_play(mode: TourMode, scripted: bool, marker_exists: bool) -> bool {
    match mode {
        TourMode::Always => true,
        TourMode::Never => false,
        TourMode::Auto => !scripted && !marker_exists,
    }
}

/// `$XDG_CONFIG_HOME/faris/tour-completed`, else `~/.config/faris/tour-completed`.
pub fn marker_path_from(xdg: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let base = match xdg.filter(|x| !x.is_empty()) {
        Some(xdg) => PathBuf::from(xdg),
        None => PathBuf::from(home.filter(|h| !h.is_empty())?).join(".config"),
    };
    Some(base.join("faris").join("tour-completed"))
}

pub fn marker_path() -> Option<PathBuf> {
    marker_path_from(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

/// Record that the tour was finished or skipped. Failures only log.
pub fn write_marker(path: &Path) {
    let result = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(path, "completed\n"));
    if let Err(error) = result {
        eprintln!(
            "FARIS tour marker not written ({}): {error}",
            path.display()
        );
    }
}

/// Which year a stop shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum YearSpec {
    Fixed(f64),
    /// Start of the first magnet replacement for the active arrangement, or
    /// the given year when no history exists.
    FirstMagnetReplacement {
        fallback: f64,
    },
}

pub struct Stop {
    pub title: &'static str,
    pub body: &'static str,
    pub anchor: Option<&'static str>,
    pub step: Option<Step>,
    pub view: Option<FieldView>,
    pub year: Option<YearSpec>,
}

pub const STOPS: [Stop; 12] = [
    Stop {
        title: "A fusion plant, through time",
        body: "FARIS explores one design question for a compact ARC-inspired tokamak: inside a fixed radial build, what happens when you trade neutron shield for breeding blanket? Every number comes from real Monte Carlo transport and a calculated 30-year operating history. This tour takes about a minute.",
        anchor: None,
        step: None,
        view: None,
        year: None,
    },
    Stop {
        title: "Five steps",
        body: "Design the arrangement, Simulate neutron transport, Operate the plant for 30 years, Compare the options, and check the Evidence. Keys 1\u{2013}5 jump between steps.",
        anchor: Some("steps-bar"),
        step: Some(Step::Design),
        view: Some(FieldView::Materials),
        year: None,
    },
    Stop {
        title: "The reactor",
        body: "Concentric shells from first wall to magnets, cut away to show the layers. The amber box is a 30 cm outboard service port, the one real 3D feature here, and the reason the magnets behind it see more neutrons. Drag to orbit, scroll to zoom, click a layer to inspect it.",
        anchor: Some("viewport"),
        step: Some(Step::Design),
        view: Some(FieldView::Materials),
        year: None,
    },
    Stop {
        title: "Two choices",
        body: "Turn the port on or off, and pick the reference split (0.45 m blanket, 0.45 m shield) or a breeder-heavy split (0.55 m, 0.35 m). The bars show the radial build; outlined layers are the ones that change.",
        anchor: Some("design-arrangement"),
        step: Some(Step::Design),
        view: None,
        year: None,
    },
    Stop {
        title: "Real transport",
        body: "One million OpenMC neutron histories per case. Tritium breeding, magnet-region flux and nuclear heating each carry their Monte Carlo error.",
        anchor: Some("transport-card"),
        step: Some(Step::Simulate),
        view: Some(FieldView::Materials),
        year: None,
    },
    Stop {
        title: "Where the neutrons go",
        body: "The coloured plane is the calculated neutron flux around the port, inside a ghost of the reactor shells. Colours use a fixed logarithmic scale, so arrangements are directly comparable.",
        anchor: Some("viewport"),
        step: Some(Step::Simulate),
        view: Some(FieldView::FluxSlice),
        year: None,
    },
    Stop {
        title: "Thirty years in one plot",
        body: "Magnet exposure climbs toward the REBCO fluence limit (dashed line). When it gets there, the magnets are swapped and the plant is down for 120 days. Drag across the plot to scrub time; the 3D model recolours as exposure builds up.",
        anchor: Some("timeline-plot"),
        step: Some(Step::Operate),
        view: Some(FieldView::ComponentFluence),
        year: Some(YearSpec::FirstMagnetReplacement { fallback: 7.0 }),
    },
    Stop {
        title: "Ask what if",
        body: "Each slider reruns all four 30-year histories in about a second. Try a stricter magnet limit or a faster swap and watch the replacements move.",
        anchor: Some("what-if"),
        step: Some(Step::Operate),
        view: None,
        year: None,
    },
    Stop {
        title: "The answer",
        body: "All four cases side by side, with every difference flagged as beyond or within 2σ sampling noise, and a one-line takeaway written from the current numbers. Scroll down for the seven-point allocation sweep.",
        anchor: Some("compare-view"),
        step: Some(Step::Compare),
        view: None,
        year: None,
    },
    Stop {
        title: "Honest labels",
        body: "Badges say what kind of number you are looking at: calculated, literature, authored, conditional or not evaluated. Hover any badge to see why, and what would settle it.",
        anchor: Some("status-badge"),
        step: None,
        view: None,
        year: None,
    },
    Stop {
        title: "Evidence you can check",
        body: "Avila Core compiles the study and records receipts that bind each result to its exact inputs. Compilation, run readiness and the scientific verdict stay separate: a successful run is not a claim that the design works.",
        anchor: Some("evidence-status"),
        step: Some(Step::Evidence),
        view: None,
        year: None,
    },
    Stop {
        title: "That's the tour",
        body: "File saves the whole study as one .faris file that reopens exactly as you left it. Export writes a two-page PDF brief, the data as CSV and the charts, stamped with that file's hash. Replay this tour any time from Tour in the top bar.",
        anchor: Some("tour-button"),
        step: Some(Step::Design),
        view: Some(FieldView::Materials),
        year: Some(YearSpec::Fixed(0.0)),
    },
];

/// The user's own state, restored when the tour is skipped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Saved {
    pub step: Step,
    pub view: FieldView,
    pub year: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    /// Apply this stop's app state.
    Enter(usize),
    /// Skipped: put the user's state back.
    Skipped(Saved),
    /// Finished: Design step, year 0, materials view.
    Finished,
}

#[derive(Default)]
pub struct Tour {
    pub active: bool,
    pub stop: usize,
    saved: Option<Saved>,
    pending: Option<Effect>,
    current: HashMap<&'static str, egui::Rect>,
    previous: HashMap<&'static str, egui::Rect>,
    frames_on_stop: u32,
    card_height: f32,
}

const PADDING: f32 = 6.0;
const CARD_WIDTH: f32 = 380.0;
const GAP: f32 = 14.0;
const MARGIN: f32 = 12.0;

impl Tour {
    /// Begin at `stop` (0-based, clamped), remembering the state to restore.
    pub fn start(&mut self, saved: Saved, stop: usize) {
        self.active = true;
        self.stop = stop.min(STOPS.len() - 1);
        self.saved = Some(saved);
        self.pending = Some(Effect::Enter(self.stop));
        self.frames_on_stop = 0;
    }

    pub fn next(&mut self) {
        if !self.active {
            return;
        }
        if self.stop + 1 >= STOPS.len() {
            self.exit(Effect::Finished);
        } else {
            self.stop += 1;
            self.pending = Some(Effect::Enter(self.stop));
            self.frames_on_stop = 0;
        }
    }

    pub fn back(&mut self) {
        if self.active && self.stop > 0 {
            self.stop -= 1;
            self.pending = Some(Effect::Enter(self.stop));
            self.frames_on_stop = 0;
        }
    }

    pub fn skip(&mut self) {
        if self.active
            && let Some(saved) = self.saved
        {
            self.exit(Effect::Skipped(saved));
        }
    }

    fn exit(&mut self, effect: Effect) {
        self.active = false;
        self.saved = None;
        self.pending = Some(effect);
    }

    pub fn pending(&self) -> Option<Effect> {
        self.pending
    }

    pub fn clear_pending(&mut self) {
        self.pending = None;
        self.frames_on_stop = 0;
    }

    /// True once the current stop's state has been applied and drawn for a
    /// few frames (always true while the tour is off).
    pub fn settled(&self) -> bool {
        !self.active || (self.pending.is_none() && self.frames_on_stop >= 6)
    }

    /// Record where a named part of the window was drawn this frame.
    pub fn anchor(&mut self, name: &'static str, rect: egui::Rect) {
        if rect.is_positive() && rect.min.is_finite() && rect.max.is_finite() {
            self.current.insert(name, rect);
        }
    }

    /// Like `anchor`, limited to what `ui` can currently show (scroll areas).
    pub fn anchor_in(&mut self, ui: &egui::Ui, name: &'static str, rect: egui::Rect) {
        self.anchor(name, rect.intersect(ui.clip_rect()));
    }

    fn spotlight(&self, stop: usize) -> Option<egui::Rect> {
        STOPS[stop]
            .anchor
            .and_then(|name| self.previous.get(name).copied())
    }

    /// Draw the overlay after all panels and rotate the frame's anchors.
    pub fn show(&mut self, ctx: &egui::Context) {
        if self.active && self.pending.is_none() {
            self.overlay(ctx);
        } else if self.active {
            ctx.request_repaint();
        }
        self.previous = std::mem::take(&mut self.current);
    }

    fn overlay(&mut self, ctx: &egui::Context) {
        let screen = ctx.content_rect();
        let stop = &STOPS[self.stop];
        let spot = self
            .spotlight(self.stop)
            .map(|r| r.expand(PADDING).intersect(screen))
            .filter(|r| r.is_positive());
        let accent = ctx.global_style().visuals.selection.stroke.color;

        egui::Area::new(egui::Id::new("tour-scrim"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .constrain(false)
            .fade_in(false)
            .show(ctx, |ui| {
                ui.allocate_rect(screen, egui::Sense::click_and_drag());
                let scrim = egui::Color32::from_black_alpha(140);
                let painter = ui.painter();
                let Some(hole) = spot else {
                    painter.rect_filled(screen, 0.0, scrim);
                    return;
                };
                for rect in [
                    egui::Rect::from_min_max(screen.min, egui::pos2(screen.max.x, hole.min.y)),
                    egui::Rect::from_min_max(egui::pos2(screen.min.x, hole.max.y), screen.max),
                    egui::Rect::from_min_max(
                        egui::pos2(screen.min.x, hole.min.y),
                        egui::pos2(hole.min.x, hole.max.y),
                    ),
                    egui::Rect::from_min_max(
                        egui::pos2(hole.max.x, hole.min.y),
                        egui::pos2(screen.max.x, hole.max.y),
                    ),
                ] {
                    painter.rect_filled(rect, 0.0, scrim);
                }
                painter.rect_stroke(
                    hole,
                    PADDING,
                    egui::Stroke::new(2.0, accent),
                    egui::StrokeKind::Outside,
                );
            });

        let card_size = egui::vec2(CARD_WIDTH, self.card_height.max(120.0));
        let position = place_card(spot, card_size, screen);
        let (mut next, mut back, mut skip) = (false, false, false);
        let card = egui::Area::new(egui::Id::new("tour-card"))
            .order(egui::Order::Foreground)
            .fixed_pos(position)
            .constrain(false)
            .fade_in(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(egui::Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_width(CARD_WIDTH - 28.0);
                        ui.weak(format!("{} / {}", self.stop + 1, STOPS.len()));
                        ui.add_space(2.0);
                        ui.label(egui::RichText::new(stop.title).strong().size(19.0));
                        ui.add_space(6.0);
                        ui.label(stop.body);
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            back = ui
                                .add_enabled(self.stop > 0, egui::Button::new("Back"))
                                .clicked();
                            let last = self.stop + 1 == STOPS.len();
                            next = ui
                                .add(
                                    egui::Button::new(if last { "Finish" } else { "Next" })
                                        .fill(accent.gamma_multiply(0.45)),
                                )
                                .clicked();
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| skip = ui.button("Skip tour").clicked(),
                            );
                        });
                    });
            });
        self.card_height = card.response.rect.height();
        ctx.move_to_top(card.response.layer_id);

        ctx.input_mut(|input| {
            let none = egui::Modifiers::NONE;
            next |= input.consume_key(none, egui::Key::ArrowRight)
                | input.consume_key(none, egui::Key::Enter);
            back |= input.consume_key(none, egui::Key::ArrowLeft);
            skip |= input.consume_key(none, egui::Key::Escape);
        });
        self.frames_on_stop = self.frames_on_stop.saturating_add(1);
        if skip {
            self.skip();
        } else if next {
            self.next();
        } else if back {
            self.back();
        }
        ctx.request_repaint();
    }
}

/// Top-left position for a card of `card` size beside `spot`: right, else
/// left, below, above. A side must fit on screen along its own axis; the
/// other axis is clamped. With no spotlight, or no side fits, the card is
/// centred or placed where it hides the least of the spotlight.
pub fn place_card(spot: Option<egui::Rect>, card: egui::Vec2, screen: egui::Rect) -> egui::Pos2 {
    let clamp = |p: egui::Pos2| {
        let x_max = (screen.max.x - card.x - MARGIN).max(screen.min.x + MARGIN);
        let y_max = (screen.max.y - card.y - MARGIN).max(screen.min.y + MARGIN);
        egui::pos2(
            p.x.clamp(screen.min.x + MARGIN, x_max),
            p.y.clamp(screen.min.y + MARGIN, y_max),
        )
    };
    let Some(spot) = spot else {
        return clamp(screen.center() - card / 2.0);
    };
    let centered = spot.center() - card / 2.0;
    let candidates = [
        (
            spot.max.x + GAP + card.x + MARGIN <= screen.max.x,
            egui::pos2(spot.max.x + GAP, centered.y),
        ),
        (
            spot.min.x - GAP - card.x - MARGIN >= screen.min.x,
            egui::pos2(spot.min.x - GAP - card.x, centered.y),
        ),
        (
            spot.max.y + GAP + card.y + MARGIN <= screen.max.y,
            egui::pos2(centered.x, spot.max.y + GAP),
        ),
        (
            spot.min.y - GAP - card.y - MARGIN >= screen.min.y,
            egui::pos2(centered.x, spot.min.y - GAP - card.y),
        ),
    ];
    if let Some((_, position)) = candidates.iter().find(|(fits, _)| *fits) {
        return clamp(*position);
    }
    let overlap = |p: egui::Pos2| {
        let hidden = egui::Rect::from_min_size(p, card).intersect(spot);
        if hidden.is_positive() {
            hidden.area()
        } else {
            0.0
        }
    };
    candidates
        .iter()
        .map(|(_, p)| clamp(*p))
        .min_by(|a, b| overlap(*a).total_cmp(&overlap(*b)))
        .expect("four candidates")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved() -> Saved {
        Saved {
            step: Step::Compare,
            view: FieldView::NuclearHeating,
            year: 12.5,
        }
    }

    #[test]
    fn auto_is_suppressed_in_scripted_runs() {
        assert!(should_play(TourMode::Auto, false, false));
        assert!(!should_play(TourMode::Auto, true, false));
        assert!(!should_play(TourMode::Auto, false, true));
        assert!(should_play(TourMode::Always, true, true));
        assert!(!should_play(TourMode::Never, false, false));
    }

    // Verifies: CFG-008
    #[test]
    fn marker_path_prefers_xdg_then_home() {
        let some = |s: &str| Some(OsString::from(s));
        assert_eq!(
            marker_path_from(some("/x/cfg"), some("/home/u")),
            Some(PathBuf::from("/x/cfg/faris/tour-completed"))
        );
        assert_eq!(
            marker_path_from(None, some("/home/u")),
            Some(PathBuf::from("/home/u/.config/faris/tour-completed"))
        );
        assert_eq!(
            marker_path_from(some(""), some("/home/u")),
            Some(PathBuf::from("/home/u/.config/faris/tour-completed"))
        );
        assert_eq!(marker_path_from(None, None), None);
    }

    #[test]
    fn marker_is_written_and_failures_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("faris").join("tour-completed");
        write_marker(&path);
        assert!(path.exists());
        // A path under a regular file cannot be created; this must not panic.
        write_marker(&path.join("nested").join("marker"));
    }

    #[test]
    fn card_placement_is_clamped_on_screen() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0));
        let card = egui::vec2(CARD_WIDTH, 240.0);
        let on_screen = |p: egui::Pos2| {
            let r = egui::Rect::from_min_size(p, card);
            screen.contains_rect(r)
        };
        // Centred when there is no spotlight.
        let p = place_card(None, card, screen);
        assert!(on_screen(p));
        // To the right of a small spotlight near the top-left, clamped vertically.
        let spot = egui::Rect::from_min_size(egui::pos2(20.0, 2.0), egui::vec2(100.0, 30.0));
        let p = place_card(Some(spot), card, screen);
        assert!(on_screen(p));
        assert!(p.x >= spot.max.x);
        // Near the right edge: left side is chosen.
        let spot = egui::Rect::from_min_size(egui::pos2(1500.0, 500.0), egui::vec2(90.0, 30.0));
        let p = place_card(Some(spot), card, screen);
        assert!(on_screen(p));
        assert!(p.x + card.x <= spot.min.x);
        // A spotlight spanning the width goes below or above, never overlapping.
        let spot = egui::Rect::from_min_size(egui::pos2(0.0, 300.0), egui::vec2(1600.0, 300.0));
        let p = place_card(Some(spot), card, screen);
        assert!(on_screen(p));
        assert!(!egui::Rect::from_min_size(p, card).intersects(spot));
        // A full-screen spotlight still stays on screen.
        let p = place_card(Some(screen), card, screen);
        assert!(on_screen(p));
        // A screen smaller than the card does not produce NaN or panic.
        let tiny = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 100.0));
        let p = place_card(Some(tiny), card, tiny);
        assert!(p.x.is_finite() && p.y.is_finite());
    }

    // Verifies: UX-053
    #[test]
    fn next_back_skip_and_finish_transitions() {
        let mut tour = Tour::default();
        tour.next();
        assert!(!tour.active && tour.pending().is_none());
        tour.start(saved(), 0);
        assert!(tour.active);
        assert_eq!(tour.pending(), Some(Effect::Enter(0)));
        tour.clear_pending();
        tour.back();
        assert_eq!((tour.stop, tour.pending()), (0, None));
        tour.next();
        assert_eq!((tour.stop, tour.pending()), (1, Some(Effect::Enter(1))));
        tour.back();
        assert_eq!(tour.stop, 0);
        tour.skip();
        assert!(!tour.active);
        assert_eq!(tour.pending(), Some(Effect::Skipped(saved())));
        tour.clear_pending();
        assert!(tour.settled());

        tour.start(saved(), 99);
        assert_eq!(tour.stop, STOPS.len() - 1);
        tour.clear_pending();
        tour.next();
        assert!(!tour.active);
        assert_eq!(tour.pending(), Some(Effect::Finished));
        // No transitions once finished.
        tour.clear_pending();
        tour.skip();
        tour.back();
        assert_eq!(tour.pending(), None);
    }

    #[test]
    fn anchors_rotate_each_frame() {
        let mut tour = Tour::default();
        let rect = egui::Rect::from_min_size(egui::pos2(1.0, 1.0), egui::vec2(10.0, 10.0));
        tour.anchor("viewport", rect);
        tour.anchor("empty", egui::Rect::NOTHING);
        tour.show(&egui::Context::default());
        assert_eq!(tour.previous.get("viewport"), Some(&rect));
        assert!(!tour.previous.contains_key("empty"));
        tour.show(&egui::Context::default());
        assert!(tour.previous.is_empty());
    }

    #[test]
    fn every_anchor_is_unique_to_a_known_name() {
        let known = [
            "steps-bar",
            "viewport",
            "design-arrangement",
            "transport-card",
            "timeline-plot",
            "what-if",
            "compare-view",
            "status-badge",
            "evidence-status",
            "tour-button",
        ];
        for stop in &STOPS {
            if let Some(anchor) = stop.anchor {
                assert!(known.contains(&anchor), "{anchor}");
            }
        }
    }
}
