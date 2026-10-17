//! The colour themes (A11Y-010, A11Y-011, A11Y-012). Every colour the app draws text, borders,
//! chart lines, axes and status pills with is named here, and `visuals` builds the egui style
//! from the same names, so native and browser builds share one definition. The tests at the
//! bottom compute the WCAG 2.2 contrast ratio of every pairing for each theme.

use std::sync::atomic::{AtomicU8, Ordering};

use eframe::egui::{self, Color32, Stroke};

/// The built-in themes. `ui.theme` (see the settings registry) selects one at start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Default,
    HighContrast,
}

impl Theme {
    #[cfg(test)]
    pub const ALL: [Theme; 2] = [Theme::Default, Theme::HighContrast];

    /// The theme a `ui.theme` setting value names. `system`, `dark`, `light` and anything
    /// unknown give the default theme: there is one ordinary theme, and it is dark.
    pub fn from_setting(value: Option<&str>) -> Theme {
        match value.map(str::trim) {
            Some("high-contrast") => Theme::HighContrast,
            _ => Theme::Default,
        }
    }

    #[cfg(test)]
    pub fn name(self) -> &'static str {
        match self {
            Theme::Default => "default",
            Theme::HighContrast => "high-contrast",
        }
    }

    pub fn palette(self) -> &'static Palette {
        match self {
            Theme::Default => &DEFAULT,
            Theme::HighContrast => &HIGH_CONTRAST,
        }
    }
}

/// Series colours of the replacement-outage chart (fixed, computed cooldown, computed work).
/// Light enough to pass 3:1 against every background of both themes.
pub const SERIES_FIXED: Color32 = Color32::from_rgb(140, 152, 176);
pub const SERIES_COOLDOWN: Color32 = Color32::from_rgb(224, 140, 60);
pub const SERIES_WORK: Color32 = Color32::from_rgb(86, 156, 214);
/// Neutron and photon spectrum lines of the design-transport result.
pub const SPECTRUM_NEUTRON: Color32 = Color32::from_rgb(108, 183, 238);
pub const SPECTRUM_PHOTON: Color32 = Color32::from_rgb(234, 164, 83);
/// Bars of the sweep history chart.
pub const SWEEP_BAR: Color32 = Color32::from_rgb(120, 140, 175);

/// Every named colour of one theme.
pub struct Palette {
    // Backgrounds.
    pub panel: Color32,
    pub window: Color32,
    /// Text-edit and code backgrounds.
    pub extreme: Color32,
    /// The stripe tint, added to the background it lies on.
    pub faint: Color32,
    /// The lane panel in the middle of the window.
    pub central: Color32,
    /// The fill behind charts.
    pub plot: Color32,
    // Text.
    pub text: Color32,
    pub text_weak: Color32,
    pub text_button: Color32,
    pub text_hovered: Color32,
    pub text_active: Color32,
    pub hyperlink: Color32,
    pub warn: Color32,
    pub error: Color32,
    pub caption: Color32,
    pub chart_text: Color32,
    pub chart_title: Color32,
    // Widget fills.
    pub button: Color32,
    pub button_hovered: Color32,
    pub button_active: Color32,
    pub selection_fill: Color32,
    // Strokes: borders, separators, axes, the focus ring.
    pub separator: Color32,
    pub control_border: Color32,
    pub control_border_hovered: Color32,
    pub control_border_active: Color32,
    pub focus_ring: Color32,
    pub chart_axis: Color32,
    pub marker: Color32,
    // Status pills and series legends: how much a colour is lifted toward white for its text,
    // and the opacity of its tinted fill and of its border.
    pub pill_text_lift: f32,
    pub pill_fill_alpha: f32,
    pub pill_border_alpha: f32,
    /// How strongly error bars show their series colour.
    pub error_bar_alpha: f32,
    /// How much of a disabled control's colour is kept (egui fades disabled widgets by it).
    pub disabled_alpha: f32,
    // Labels over the 3D view: the plate behind them and the label colour.
    pub overlay_plate: Color32,
    pub overlay_text: Color32,
    // Contrast a label drawn on a data-coloured fill must reach.
    pub label_min: f32,
    // Series colours (lines, markers, bars) are lifted toward white by this much, and bars
    // are drawn with this opacity.
    pub series_lift: f32,
    pub bar_alpha: f32,
}

const fn gray(v: u8) -> Color32 {
    Color32::from_gray(v)
}

pub static DEFAULT: Palette = Palette {
    panel: Color32::from_rgb(37, 39, 44),
    window: gray(27),
    extreme: gray(10),
    faint: Color32::from_rgba_premultiplied(5, 5, 5, 0),
    central: Color32::from_rgb(47, 50, 57),
    plot: Color32::from_rgb(24, 26, 31),
    text: gray(190),
    text_weak: gray(165),
    text_button: gray(200),
    text_hovered: gray(240),
    text_active: Color32::WHITE,
    hyperlink: Color32::from_rgb(90, 170, 255),
    warn: Color32::from_rgb(255, 143, 0),
    error: Color32::from_rgb(245, 120, 120),
    caption: gray(180),
    chart_text: gray(190),
    chart_title: gray(225),
    button: gray(52),
    button_hovered: gray(70),
    button_active: gray(55),
    selection_fill: Color32::from_rgb(67, 78, 125),
    separator: gray(155),
    control_border: gray(155),
    control_border_hovered: gray(170),
    control_border_active: Color32::WHITE,
    focus_ring: Color32::from_rgb(192, 222, 255),
    chart_axis: gray(155),
    marker: gray(230),
    pill_text_lift: 0.3,
    pill_fill_alpha: 0.14,
    pill_border_alpha: 1.0,
    error_bar_alpha: 0.75,
    disabled_alpha: 0.75,
    overlay_plate: Color32::from_rgb(18, 21, 28),
    overlay_text: Color32::from_rgb(255, 178, 46),
    label_min: 4.5,
    series_lift: 0.0,
    bar_alpha: 0.85,
};

pub static HIGH_CONTRAST: Palette = Palette {
    panel: Color32::BLACK,
    window: Color32::BLACK,
    extreme: Color32::BLACK,
    faint: Color32::from_rgba_premultiplied(18, 18, 18, 0),
    central: gray(12),
    plot: Color32::BLACK,
    text: Color32::WHITE,
    text_weak: gray(215),
    text_button: Color32::WHITE,
    text_hovered: Color32::WHITE,
    text_active: Color32::WHITE,
    hyperlink: Color32::from_rgb(130, 195, 255),
    warn: Color32::from_rgb(255, 190, 80),
    error: Color32::from_rgb(255, 140, 140),
    caption: gray(230),
    chart_text: Color32::WHITE,
    chart_title: Color32::WHITE,
    button: gray(28),
    button_hovered: gray(55),
    button_active: gray(75),
    selection_fill: Color32::from_rgb(0, 60, 125),
    separator: gray(200),
    control_border: gray(210),
    control_border_hovered: Color32::WHITE,
    control_border_active: Color32::from_rgb(255, 255, 0),
    focus_ring: Color32::from_rgb(255, 255, 0),
    chart_axis: gray(210),
    marker: Color32::WHITE,
    pill_text_lift: 0.5,
    pill_fill_alpha: 0.0,
    pill_border_alpha: 1.0,
    error_bar_alpha: 1.0,
    disabled_alpha: 0.7,
    overlay_plate: Color32::BLACK,
    overlay_text: Color32::from_rgb(255, 200, 90),
    label_min: 7.0,
    series_lift: 0.15,
    bar_alpha: 1.0,
};

static ACTIVE: AtomicU8 = AtomicU8::new(0);

/// The theme in use. Set once at start by `install`.
pub fn active_theme() -> Theme {
    match ACTIVE.load(Ordering::Relaxed) {
        1 => Theme::HighContrast,
        _ => Theme::Default,
    }
}

/// The palette of the theme in use.
pub fn palette() -> &'static Palette {
    active_theme().palette()
}

/// Make `theme` the one in use and apply its egui style to `ctx`.
pub fn install(ctx: &egui::Context, theme: Theme) {
    ACTIVE.store(
        match theme {
            Theme::Default => 0,
            Theme::HighContrast => 1,
        },
        Ordering::Relaxed,
    );
    ctx.set_visuals(visuals(theme));
}

fn widget(
    base: &egui::style::WidgetVisuals,
    fill: Color32,
    border: Color32,
    text: Color32,
    text_width: f32,
) -> egui::style::WidgetVisuals {
    egui::style::WidgetVisuals {
        weak_bg_fill: fill,
        bg_fill: fill,
        bg_stroke: Stroke::new(1.0, border),
        fg_stroke: Stroke::new(text_width, text),
        ..*base
    }
}

/// The egui style of `theme`.
pub fn visuals(theme: Theme) -> egui::Visuals {
    let p = theme.palette();
    let mut v = egui::Visuals::dark();
    v.panel_fill = p.panel;
    v.window_fill = p.window;
    v.window_stroke = Stroke::new(1.0, p.separator);
    v.disabled_alpha = p.disabled_alpha;
    v.extreme_bg_color = p.extreme;
    v.faint_bg_color = p.faint;
    v.hyperlink_color = p.hyperlink;
    v.warn_fg_color = p.warn;
    v.error_fg_color = p.error;
    v.weak_text_color = Some(p.text_weak);
    v.selection.bg_fill = p.selection_fill;
    v.selection.stroke = Stroke::new(1.0, p.focus_ring);
    let w = &mut v.widgets;
    w.noninteractive = widget(&w.noninteractive, p.panel, p.separator, p.text, 1.0);
    w.inactive = widget(&w.inactive, p.button, p.control_border, p.text_button, 1.0);
    w.hovered = widget(
        &w.hovered,
        p.button_hovered,
        p.control_border_hovered,
        p.text_hovered,
        1.5,
    );
    w.active = widget(
        &w.active,
        p.button_active,
        p.control_border_active,
        p.text_active,
        2.0,
    );
    w.open = widget(&w.open, p.button, p.control_border, p.text_button, 1.0);
    v
}

/// WCAG 2.2 relative luminance of an opaque colour.
pub fn relative_luminance(c: Color32) -> f64 {
    let linear = |channel: u8| {
        let v = f64::from(channel) / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(c.r()) + 0.7152 * linear(c.g()) + 0.0722 * linear(c.b())
}

/// WCAG 2.2 contrast ratio of two opaque colours.
pub fn contrast_ratio(a: Color32, b: Color32) -> f64 {
    let (a, b) = (relative_luminance(a), relative_luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// The colour of a label drawn on `fill` (a data colour the app does not choose, such as a
/// component colour from a study file), and a backing plate when black or white text alone
/// would fall short of `p.label_min`. The pair always reaches `label_min`: against its plate
/// the text is black on white or white on black, 21 to 1.
pub fn label_colors(p: &Palette, fill: Color32) -> (Color32, Option<Color32>) {
    let (best, other) =
        if contrast_ratio(Color32::BLACK, fill) >= contrast_ratio(Color32::WHITE, fill) {
            (Color32::BLACK, Color32::WHITE)
        } else {
            (Color32::WHITE, Color32::BLACK)
        };
    if f64::from(p.label_min) <= contrast_ratio(best, fill) {
        (best, None)
    } else {
        (best, Some(other))
    }
}

/// Draw `text` centred on `center`, over `fill`, readable at the theme's label contrast.
pub fn label_on_fill(
    painter: &egui::Painter,
    center: egui::Pos2,
    text: &str,
    font: egui::FontId,
    fill: Color32,
) {
    let (color, plate) = label_colors(palette(), fill);
    let galley = painter.layout_no_wrap(text.to_owned(), font, color);
    let rect = egui::Align2::CENTER_CENTER.anchor_size(center, galley.size());
    if let Some(plate) = plate {
        painter.rect_filled(rect.expand2(egui::vec2(2.0, 0.0)), 2.0, plate);
    }
    painter.galley(rect.min, galley, color);
}

#[cfg(test)]
/// Blend `top` (premultiplied, as egui stores it) over the opaque `under`.
pub fn over(top: Color32, under: Color32) -> Color32 {
    let keep = 1.0 - f32::from(top.a()) / 255.0;
    let mix = |t: u8, u: u8| (f32::from(t) + f32::from(u) * keep).round().min(255.0) as u8;
    Color32::from_rgb(
        mix(top.r(), under.r()),
        mix(top.g(), under.g()),
        mix(top.b(), under.b()),
    )
}

/// `color` lifted toward white by `amount`, for text drawn in a status or series colour.
pub fn lifted(color: Color32, amount: f32) -> Color32 {
    let lift = |c: u8| (f32::from(c) + (255.0 - f32::from(c)) * amount).round() as u8;
    Color32::from_rgb(lift(color.r()), lift(color.g()), lift(color.b()))
}

/// Text in a status or series colour.
pub fn pill_text(color: Color32) -> Color32 {
    pill_text_in(palette(), color)
}

pub fn pill_text_in(p: &Palette, color: Color32) -> Color32 {
    lifted(color, p.pill_text_lift)
}

/// The tinted fill behind a status or series pill.
pub fn pill_fill(color: Color32) -> Color32 {
    pill_fill_in(palette(), color)
}

pub fn pill_fill_in(p: &Palette, color: Color32) -> Color32 {
    color.gamma_multiply(p.pill_fill_alpha)
}

/// The border of a status or series pill.
pub fn pill_border(color: Color32) -> Color32 {
    pill_border_in(palette(), color)
}

pub fn pill_border_in(p: &Palette, color: Color32) -> Color32 {
    color.gamma_multiply(p.pill_border_alpha)
}

/// An error bar in a series colour: dimmed in the default theme, full strength in high contrast.
pub fn error_bar(color: Color32) -> Color32 {
    error_bar_in(palette(), color)
}

pub fn error_bar_in(p: &Palette, color: Color32) -> Color32 {
    color.gamma_multiply(p.error_bar_alpha)
}

/// A bar drawn in a series colour.
pub fn bar_fill(color: Color32) -> Color32 {
    bar_fill_in(palette(), color)
}

pub fn bar_fill_in(p: &Palette, color: Color32) -> Color32 {
    color.gamma_multiply(p.bar_alpha)
}

/// A line, marker or bar colour of the theme in use, from the colour the engine names.
pub fn series(color: Color32) -> Color32 {
    series_in(palette(), color)
}

pub fn series_in(p: &Palette, color: Color32) -> Color32 {
    lifted(color, p.series_lift)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::badge::Kind;

    /// The colour the engine names for an arrangement, before the theme lifts it.
    fn arrangement_color(port: bool, breeder: bool) -> Color32 {
        let [r, g, b] = faris_engine::brief::Arrangement { port, breeder }.rgb();
        Color32::from_rgb(r, g, b)
    }

    fn text_minimum(theme: Theme) -> f32 {
        match theme {
            Theme::Default => 4.5,
            Theme::HighContrast => 7.0,
        }
    }

    fn linear(channel: u8) -> f64 {
        let c = f64::from(channel) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    /// WCAG 2.2 relative luminance of an opaque colour.
    fn luminance(c: Color32) -> f64 {
        0.2126 * linear(c.r()) + 0.7152 * linear(c.g()) + 0.0722 * linear(c.b())
    }

    /// WCAG 2.2 contrast ratio; `top` is blended over `under` first when it has alpha.
    fn ratio(top: Color32, under: Color32) -> f64 {
        let a = luminance(over(top, under));
        let b = luminance(under);
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    fn kinds() -> [Kind; 8] {
        [
            Kind::Calculated,
            Kind::Checked,
            Kind::Authored,
            Kind::Literature,
            Kind::Conditional,
            Kind::Partial,
            Kind::NotEvaluated,
            Kind::Failed,
        ]
    }

    const THIN_LINE_COVERAGE: f32 = 0.8;

    /// The colour a one-pixel line of `fg` is drawn as over `bg` at THIN_LINE_COVERAGE.
    fn thin_line(fg: Color32, bg: Color32) -> Color32 {
        let mix = |f: u8, b: u8| {
            (f32::from(b) + (f32::from(f) - f32::from(b)) * THIN_LINE_COVERAGE).round() as u8
        };
        Color32::from_rgb(
            mix(fg.r(), bg.r()),
            mix(fg.g(), bg.g()),
            mix(fg.b(), bg.b()),
        )
    }

    /// Every colour name, value and background it is read against.
    struct Check {
        what: String,
        ratio: f64,
        minimum: f64,
    }

    fn checks(theme: Theme) -> Vec<Check> {
        let p = theme.palette();
        let v = visuals(theme);
        let text_min = f64::from(text_minimum(theme));
        // High contrast raises non-text to the text level of ordinary text.
        let graphic_min = if theme == Theme::HighContrast {
            4.5
        } else {
            3.0
        };
        let mut out = Vec::new();
        let mut text = |what: &str, fg: Color32, bg: Color32, bg_name: &str| {
            out.push(Check {
                what: format!("text {what} on {bg_name}"),
                ratio: ratio(fg, bg),
                minimum: text_min,
            });
        };
        let faint_panel = over(p.faint, p.panel);
        let pages = [
            ("panel", p.panel),
            ("window", p.window),
            ("extreme", p.extreme),
            ("faint stripe", faint_panel),
            ("central", p.central),
        ];
        let plots = [("plot", p.plot), ("panel", p.panel)];
        // Text drawn with the egui style, on every page background.
        for (bg_name, bg) in pages {
            for (name, fg) in [
                ("label", v.widgets.noninteractive.fg_stroke.color),
                ("weak", v.weak_text_color()),
                ("hyperlink", v.hyperlink_color),
                ("warning", v.warn_fg_color),
                ("error", v.error_fg_color),
                ("selection-stroke", v.selection.stroke.color),
                ("caption", p.caption),
                ("light red", Color32::LIGHT_RED),
                ("yellow", Color32::YELLOW),
            ] {
                text(name, fg, bg, bg_name);
            }
            for kind in kinds() {
                text(
                    &format!("status {}", kind.default_label()),
                    pill_text_in(p, kind.color()),
                    bg,
                    bg_name,
                );
            }
        }
        for (bg_name, bg) in plots {
            for (name, fg) in [
                ("chart text", p.chart_text),
                ("chart title", p.chart_title),
                ("hidden legend", p.text_weak),
                ("not recorded", p.text_weak),
            ] {
                text(name, fg, bg, bg_name);
            }
            for (name, fg) in [("neutron", SPECTRUM_NEUTRON), ("photon", SPECTRUM_PHOTON)] {
                text(&format!("{name} label"), pill_text_in(p, fg), bg, bg_name);
            }
        }
        // Labels over the 3D view sit on an opaque plate, so the scene behind never matters.
        for (name, fg) in [
            ("overlay label", p.overlay_text),
            ("viewport caption", p.caption),
        ] {
            text(name, fg, p.overlay_plate, "overlay plate");
        }
        // Button text on each button state, and on the selection fill.
        for (state, widget) in [
            ("idle", v.widgets.inactive),
            ("hovered", v.widgets.hovered),
            ("active", v.widgets.active),
            ("open", v.widgets.open),
        ] {
            text(
                &format!("{state} button"),
                widget.fg_stroke.color,
                widget.weak_bg_fill,
                &format!("{state} button fill"),
            );
        }
        // A disabled control is drawn with its colours faded by disabled_alpha: the label on the
        // page, and the button text on the faded button fill.
        let faded = |c: Color32, under: Color32| over(c.gamma_multiply(v.disabled_alpha), under);
        for (bg_name, bg) in pages {
            text(
                "disabled label",
                faded(v.widgets.noninteractive.fg_stroke.color, bg),
                bg,
                bg_name,
            );
            let fill = faded(v.widgets.inactive.weak_bg_fill, bg);
            text(
                "disabled button",
                faded(v.widgets.inactive.fg_stroke.color, fill),
                fill,
                &format!("disabled button fill on {bg_name}"),
            );
        }
        text(
            "selected text",
            v.selection.stroke.color,
            v.selection.bg_fill,
            "selection fill",
        );
        // Series text on its own tinted fill over the page it lies on.
        for (bg_name, bg) in [("panel", p.panel), ("window", p.window)] {
            for (name, color) in kinds()
                .into_iter()
                .map(|k| (k.default_label().to_owned(), k.color()))
                .chain([
                    (
                        "port".to_owned(),
                        series_in(p, arrangement_color(true, false)),
                    ),
                    (
                        "port breeder".to_owned(),
                        series_in(p, arrangement_color(true, true)),
                    ),
                    (
                        "no port".to_owned(),
                        series_in(p, arrangement_color(false, false)),
                    ),
                    (
                        "no port breeder".to_owned(),
                        series_in(p, arrangement_color(false, true)),
                    ),
                ])
            {
                text(
                    &format!("pill {name}"),
                    pill_text_in(p, color),
                    over(pill_fill_in(p, color), bg),
                    &format!("its pill fill on {bg_name}"),
                );
            }
        }

        // A one-pixel line is anti-aliased across two device pixels at fractional scales, so its
        // drawn colour is only part of the way from the background to the stroke colour. Screenshots
        // at 1.46 scale (scripts/screenshot_contrast.py) measured 0.83 of the way; lines are checked
        // at THIN_LINE_COVERAGE, below that.
        // Filled areas (bars) are drawn at their own colour; every other graphic is a line.
        let mut graphic = |what: &str, fg: Color32, bg: Color32, bg_name: &str| {
            let drawn = if matches!(what, "sweep bar" | "port bar" | "no port bar") {
                fg
            } else {
                thin_line(fg, bg)
            };
            out.push(Check {
                what: format!("non-text {what} against {bg_name}"),
                ratio: ratio(drawn, bg),
                minimum: graphic_min,
            });
        };
        graphic(
            "overlay label border",
            pill_border_in(p, p.overlay_text),
            p.overlay_plate,
            "overlay plate",
        );
        // Control borders, separators, focus ring, axes and markers.
        for (bg_name, bg) in pages {
            for (name, fg) in [
                ("separator", v.widgets.noninteractive.bg_stroke.color),
                ("window stroke", v.window_stroke.color),
                ("idle border", v.widgets.inactive.bg_stroke.color),
                ("hovered border", v.widgets.hovered.bg_stroke.color),
                ("active border", v.widgets.active.bg_stroke.color),
                ("open border", v.widgets.open.bg_stroke.color),
                ("focus ring", v.selection.stroke.color),
            ] {
                graphic(name, fg, bg, bg_name);
            }
        }
        for (bg_name, bg) in plots {
            for (name, fg) in [("axis", p.chart_axis), ("marker", p.marker)] {
                graphic(name, fg, bg, bg_name);
            }
            for (name, fg) in [
                ("outage fixed", SERIES_FIXED),
                ("outage cooldown", SERIES_COOLDOWN),
                ("outage work", SERIES_WORK),
                ("neutron line", SPECTRUM_NEUTRON),
                ("photon line", SPECTRUM_PHOTON),
                ("neutron error bar", error_bar_in(p, SPECTRUM_NEUTRON)),
                ("photon error bar", error_bar_in(p, SPECTRUM_PHOTON)),
                ("port line", series_in(p, arrangement_color(true, false))),
                (
                    "port breeder line",
                    series_in(p, arrangement_color(true, true)),
                ),
                (
                    "no port line",
                    series_in(p, arrangement_color(false, false)),
                ),
                (
                    "no port breeder line",
                    series_in(p, arrangement_color(false, true)),
                ),
                ("sweep bar", bar_fill_in(p, SWEEP_BAR)),
                (
                    "port bar",
                    bar_fill_in(p, series_in(p, arrangement_color(true, false))),
                ),
                (
                    "no port bar",
                    bar_fill_in(p, series_in(p, arrangement_color(false, false))),
                ),
            ] {
                graphic(name, fg, bg, bg_name);
            }
        }
        // Status pills and series buttons: the border against the page, and the filled
        // controls against the page.
        for (bg_name, bg) in [("panel", p.panel), ("window", p.window)] {
            for kind in kinds() {
                graphic(
                    &format!("pill border {}", kind.default_label()),
                    pill_border_in(p, kind.color()),
                    bg,
                    bg_name,
                );
            }
            for (name, color) in [
                ("port", series_in(p, arrangement_color(true, false))),
                ("port breeder", series_in(p, arrangement_color(true, true))),
                ("no port", series_in(p, arrangement_color(false, false))),
                (
                    "no port breeder",
                    series_in(p, arrangement_color(false, true)),
                ),
            ] {
                graphic(
                    &format!("series button border {name}"),
                    pill_border_in(p, color),
                    bg,
                    bg_name,
                );
            }
        }
        out
    }

    // Verifies: A11Y-010, A11Y-011
    #[test]
    fn every_theme_meets_the_contrast_targets() {
        for theme in Theme::ALL {
            let failing: Vec<String> = checks(theme)
                .into_iter()
                .filter(|c| c.ratio < c.minimum)
                .map(|c| format!("{} = {:.2} (needs {})", c.what, c.ratio, c.minimum))
                .collect();
            assert!(
                failing.is_empty(),
                "{} theme fails:\n{}",
                theme.name(),
                failing.join("\n")
            );
        }
    }

    // Verifies: A11Y-010, A11Y-011
    #[test]
    fn the_palette_check_covers_the_named_styles() {
        for theme in Theme::ALL {
            let all = checks(theme);
            assert!(
                all.len() > 200,
                "{} checks only {}",
                theme.name(),
                all.len()
            );
            for needle in [
                "text weak",
                "text label",
                "idle border",
                "focus ring",
                "axis",
            ] {
                assert!(
                    all.iter().any(|c| c.what.contains(needle)),
                    "{} has no {needle} check",
                    theme.name()
                );
            }
        }
    }

    // Verifies: A11Y-010
    #[test]
    fn labels_on_data_coloured_fills_reach_the_theme_contrast() {
        // Component colours come from study files, so every colour is tried: a grid over the
        // whole RGB cube (steps of 15 include both ends) plus every gray.
        let mut worst = f64::MAX;
        for theme in Theme::ALL {
            let p = theme.palette();
            let minimum = f64::from(text_minimum(theme));
            assert_eq!(f64::from(p.label_min), minimum);
            let mut check = |fill: Color32| {
                let (color, plate) = label_colors(p, fill);
                let got = match plate {
                    Some(plate) => ratio(color, plate),
                    None => ratio(color, fill),
                };
                assert!(
                    got >= minimum,
                    "{fill:?} gives {got:.2} in {}",
                    theme.name()
                );
                worst = worst.min(got);
            };
            for r in (0..=255u8).step_by(15) {
                for g in (0..=255u8).step_by(15) {
                    for b in (0..=255u8).step_by(15) {
                        check(Color32::from_rgb(r, g, b));
                    }
                }
            }
            for v in 0..=255u8 {
                check(gray(v));
            }
        }
        assert!(worst >= 4.5);
    }

    // The selection fill is a highlight behind text (checked above as "selected text"); it
    // conveys no information of its own, so WCAG 1.4.11 does not ask 3:1 of the fill. The
    // colour-map gradient and the component bars are graphical objects: each is outlined in
    // `chart_axis`, checked against every background in `checks`, so their boundary is 3:1
    // whatever the fill colours are.
    // Verifies: A11Y-011
    #[test]
    fn data_coloured_objects_are_outlined_at_graphic_contrast() {
        for theme in Theme::ALL {
            let p = theme.palette();
            let minimum = if theme == Theme::HighContrast {
                4.5
            } else {
                3.0
            };
            for bg in [p.panel, p.window, p.central, p.plot] {
                assert!(ratio(p.chart_axis, bg) >= minimum, "{}", theme.name());
            }
        }
    }

    #[test]
    fn the_setting_selects_a_theme() {
        assert_eq!(
            Theme::from_setting(Some("high-contrast")),
            Theme::HighContrast
        );
        assert_eq!(Theme::from_setting(Some("dark")), Theme::Default);
        assert_eq!(Theme::from_setting(Some("system")), Theme::Default);
        assert_eq!(Theme::from_setting(None), Theme::Default);
    }

    #[test]
    fn contrast_ratio_matches_the_wcag_reference() {
        // Black on white is 21:1; the 777 gray on white is the published 4.48:1.
        let r = ratio(Color32::BLACK, Color32::WHITE);
        assert!((r - 21.0).abs() < 1e-9);
        let r = ratio(gray(0x77), Color32::WHITE);
        assert!((r - 4.48).abs() < 0.01, "{r}");
    }

    /// Prints the lowest ratio of each theme: `cargo test -p faris-app lowest -- --nocapture`.
    #[test]
    fn lowest_ratios_are_reported() {
        for theme in Theme::ALL {
            let all = checks(theme);
            let text = all
                .iter()
                .filter(|c| c.what.starts_with("text"))
                .min_by(|a, b| a.ratio.total_cmp(&b.ratio))
                .expect("text checks");
            let graphic = all
                .iter()
                .filter(|c| c.what.starts_with("non-text"))
                .min_by(|a, b| a.ratio.total_cmp(&b.ratio))
                .expect("graphic checks");
            println!(
                "{}: lowest text {:.2} ({}); lowest non-text {:.2} ({})",
                theme.name(),
                text.ratio,
                text.what,
                graphic.ratio,
                graphic.what
            );
        }
    }
}
