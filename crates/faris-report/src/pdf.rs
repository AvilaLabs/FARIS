//! The two-page US Letter summary. Text is drawn as real text in the app's
//! fonts (embedded, subset); charts are the exported SVGs drawn as vectors.

use crate::{
    ArrangementData,
    charts::{ChartSvg, wrap_lines},
    fonts::{BODY_FONT, FontKind, MONO_FONT, font_database, text_width},
    svg::{INK, MUTED, Rgb, darken, tint},
    uncertainty::{ColumnStatus, UncertaintyReport},
};
use faris_engine::{
    brief::{
        AssumptionRow, BLANKET_PLUS_SHIELD_M, BREEDER_BLANKET_M, Caveat, Contrast,
        REFERENCE_BLANKET_M, StatusKind, StudyComparison,
    },
    history_uncertainty::{SCOPE_DETAIL, SCOPE_LINE},
};
use faris_model::RESEARCH_SCREENING_STATEMENT;
use krilla::{
    Document,
    color::rgb,
    geom::{PathBuilder, Point, Size, Transform},
    image::Image,
    metadata::Metadata,
    num::NormalizedF32,
    page::PageSettings,
    paint::{Fill, Stroke},
    surface::Surface,
    text::{Font, TextDirection},
};
use krilla_svg::{SurfaceExt, SvgSettings};
use std::sync::Arc;

const PAGE_W: f32 = 612.0;
const PAGE_H: f32 = 792.0;
const MARGIN: f32 = 36.0;
const CONTENT_W: f32 = PAGE_W - 2.0 * MARGIN;
const BOTTOM: f32 = 756.0;
const RULE: Rgb = [208, 212, 220];
const STRIPE: Rgb = [246, 247, 250];

/// Everything the two pages show.
pub struct PdfContent<'a> {
    pub title: &'a str,
    pub subtitle: String,
    pub question: String,
    pub takeaways: &'a [String],
    pub arrangements: &'a [&'a ArrangementData],
    pub study: &'a StudyComparison,
    /// The timeline chart drawn at a given height, so page one can fill
    /// exactly the room that is left.
    pub timeline: &'a dyn Fn(f32) -> ChartSvg,
    pub sweep_charts: Option<[&'a ChartSvg; 3]>,
    pub sweep_findings: Vec<(StatusKind, String)>,
    pub sweep_note: String,
    pub assumptions: &'a [AssumptionRow],
    pub caveats: &'a [Caveat],
    pub footer: String,
    pub view_image: Option<&'a [u8]>,
    /// The history ensembles of the arrangements; a third page when present.
    pub uncertainty: Option<&'a UncertaintyReport>,
    /// The tritium and net-electricity band charts, drawn under the table
    /// when they fit.
    pub band_charts: Vec<&'a ChartSvg>,
}

struct Canvas<'a, 's> {
    surface: Option<&'a mut Surface<'s>>,
    body: Font,
    mono: Font,
}

fn fill(c: Rgb, opacity: f32) -> Fill {
    Fill {
        paint: rgb::Color::new(c[0], c[1], c[2]).into(),
        opacity: NormalizedF32::new(opacity.clamp(0.0, 1.0)).unwrap_or(NormalizedF32::ONE),
        rule: Default::default(),
    }
}

fn stroke(c: Rgb, width: f32) -> Stroke {
    Stroke {
        paint: rgb::Color::new(c[0], c[1], c[2]).into(),
        width,
        ..Default::default()
    }
}

impl Canvas<'_, '_> {
    #[allow(clippy::too_many_arguments)]
    fn text(&mut self, x: f32, y: f32, s: &str, size: f32, c: Rgb, kind: FontKind, bold: bool) {
        let Some(surface) = self.surface.as_deref_mut() else {
            return;
        };
        let font = match kind {
            FontKind::Body => self.body.clone(),
            FontKind::Mono => self.mono.clone(),
        };
        surface.set_fill(Some(fill(c, 1.0)));
        if bold {
            surface.set_stroke(Some(stroke(c, size * 0.045)));
        }
        surface.draw_text(
            Point::from_xy(x, y),
            font,
            size,
            s,
            false,
            TextDirection::Auto,
        );
        if bold {
            surface.set_stroke(None);
        }
    }

    fn body(&mut self, x: f32, y: f32, s: &str, size: f32, c: Rgb) {
        self.text(x, y, s, size, c, FontKind::Body, false);
    }

    fn bold(&mut self, x: f32, y: f32, s: &str, size: f32, c: Rgb) {
        self.text(x, y, s, size, c, FontKind::Body, true);
    }

    fn mono(&mut self, x: f32, y: f32, s: &str, size: f32, c: Rgb) {
        self.text(x, y, s, size, c, FontKind::Mono, false);
    }

    fn right(&mut self, right: f32, y: f32, s: &str, size: f32, c: Rgb, kind: FontKind) {
        let w = text_width(kind, s, size);
        self.text(right - w, y, s, size, c, kind, false);
    }

    #[allow(clippy::too_many_arguments)]
    fn rect(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        fill_c: Option<(Rgb, f32)>,
        line: Option<(Rgb, f32)>,
    ) {
        let Some(surface) = self.surface.as_deref_mut() else {
            return;
        };
        let mut b = PathBuilder::new();
        let r = radius.min(w / 2.0).min(h / 2.0);
        if r <= 0.0 {
            b.move_to(x, y);
            b.line_to(x + w, y);
            b.line_to(x + w, y + h);
            b.line_to(x, y + h);
        } else {
            let k = 0.5523 * r;
            b.move_to(x + r, y);
            b.line_to(x + w - r, y);
            b.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
            b.line_to(x + w, y + h - r);
            b.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
            b.line_to(x + r, y + h);
            b.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
            b.line_to(x, y + r);
            b.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
        }
        b.close();
        let Some(path) = b.finish() else {
            return;
        };
        surface.set_fill(fill_c.map(|(c, o)| fill(c, o)));
        surface.set_stroke(line.map(|(c, w)| stroke(c, w)));
        surface.draw_path(&path);
        surface.set_fill(None);
        surface.set_stroke(None);
    }

    fn hline(&mut self, x1: f32, x2: f32, y: f32, c: Rgb, width: f32) {
        let Some(surface) = self.surface.as_deref_mut() else {
            return;
        };
        let mut b = PathBuilder::new();
        b.move_to(x1, y);
        b.line_to(x2, y);
        if let Some(path) = b.finish() {
            surface.set_fill(None);
            surface.set_stroke(Some(stroke(c, width)));
            surface.draw_path(&path);
            surface.set_stroke(None);
        }
    }

    /// A status label in the app's badge colours; text is darkened for paper.
    /// Returns its width.
    fn pill(&mut self, x: f32, y_top: f32, label: &str, kind: StatusKind, size: f32) -> f32 {
        let color = kind.rgb();
        let w = text_width(FontKind::Body, label, size) + 7.0;
        let h = size + 3.2;
        self.rect(
            x,
            y_top,
            w,
            h,
            h / 2.0,
            Some((tint(color, 0.8), 1.0)),
            Some((color, 0.7)),
        );
        self.text(
            x + 3.5,
            y_top + size + 0.4,
            label,
            size,
            darken(color, 0.55),
            FontKind::Body,
            false,
        );
        w
    }

    fn svg(&mut self, chart: &ChartSvg, x: f32, y: f32) {
        let Some(surface) = self.surface.as_deref_mut() else {
            return;
        };
        let options = usvg::Options {
            fontdb: font_database(),
            ..Default::default()
        };
        let Ok(tree) = usvg::Tree::from_str(&chart.svg, &options) else {
            return;
        };
        let Some(size) = Size::from_wh(chart.width, chart.height) else {
            return;
        };
        surface.push_transform(&Transform::from_translate(x, y));
        let _ = surface.draw_svg(&tree, size, SvgSettings::default());
        surface.pop();
    }

    /// Draw a PNG scaled into `max_w` x `max_h`, right edge at `x_right`.
    /// Returns the drawn size, or None when it cannot be decoded.
    fn image(
        &mut self,
        png: &[u8],
        x_right: f32,
        y: f32,
        max_w: f32,
        max_h: f32,
    ) -> Option<(f32, f32)> {
        let image = Image::from_png(png.to_vec().into(), true).ok()?;
        let (iw, ih) = image.size();
        let scale = (max_w / iw as f32).min(max_h / ih as f32);
        let (w, h) = (iw as f32 * scale, ih as f32 * scale);
        if let Some(surface) = self.surface.as_deref_mut() {
            surface.push_transform(&Transform::from_translate(x_right - w, y));
            surface.draw_image(image, Size::from_wh(w, h)?);
            surface.pop();
        }
        Some((w, h))
    }

    /// Wrapped paragraph; returns the y after its last line.
    #[allow(clippy::too_many_arguments)]
    fn paragraph(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        s: &str,
        size: f32,
        leading: f32,
        c: Rgb,
    ) -> f32 {
        let mut y = y;
        for line in wrap_lines(s, width, size) {
            self.body(x, y + size, &line, size, c);
            y += leading;
        }
        y
    }
}

/// Text cut to fit `width`, ending in an ellipsis when shortened.
pub fn fit_text(s: &str, width: f32, size: f32) -> String {
    fit_text_in(FontKind::Body, s, width, size)
}

fn fit_text_in(kind: FontKind, s: &str, width: f32, size: f32) -> String {
    if text_width(kind, s, size) <= width {
        return s.to_string();
    }
    let mut out = String::new();
    for c in s.chars() {
        let candidate = format!("{out}{c}…");
        if text_width(kind, &candidate, size) > width {
            break;
        }
        out.push(c);
    }
    format!("{}…", out.trim_end())
}

/// First sentence of a provenance text, for tables with room for one line.
pub fn first_sentence(s: &str) -> String {
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    for (k, (i, c)) in chars.iter().enumerate() {
        if *c != '.' {
            continue;
        }
        let next = chars.get(k + 1).map(|x| x.1);
        let after = chars.get(k + 2).map(|x| x.1);
        let word_start = s[..*i].rfind(' ').map_or(0, |p| p + 1);
        let word = &s[word_start..*i];
        let abbreviation = matches!(word, "al" | "e.g" | "i.e" | "vs" | "approx");
        if next == Some(' ') && after.is_some_and(char::is_uppercase) && !abbreviation {
            return s[..=*i].to_string();
        }
    }
    s.to_string()
}

fn fmt_sig(v: f64) -> String {
    format!("{v:.2e}")
}

/// One value cell of the comparison table: text and its status label.
struct Cell {
    text: String,
    kind: StatusKind,
    label: String,
    muted: bool,
}

fn comparison_cells(d: &ArrangementData) -> [Cell; 5] {
    let s = &d.summary;
    let not_recorded = || Cell {
        text: "not recorded".into(),
        kind: StatusKind::NotEvaluated,
        label: "not evaluated".into(),
        muted: true,
    };
    if !s.recorded {
        return [
            not_recorded(),
            not_recorded(),
            not_recorded(),
            not_recorded(),
            not_recorded(),
        ];
    }
    let none = |text: &str| Cell {
        text: text.into(),
        kind: StatusKind::NotEvaluated,
        label: "not evaluated".into(),
        muted: true,
    };
    let swaps = match s.swaps {
        Some(n) => Cell {
            text: n.to_string(),
            kind: StatusKind::Conditional,
            label: "conditional".into(),
            muted: false,
        },
        None => none("no limit declared"),
    };
    let first = match (s.first_swap_y, s.first_swap_relative_sampling) {
        (Some(y), Some(rel)) => Cell {
            text: format!("{y:.1} ± {:.1} y", y * rel),
            kind: StatusKind::Partial,
            label: "partial · sampling, first order".into(),
            muted: false,
        },
        (Some(y), None) => Cell {
            text: format!("{y:.1} y"),
            kind: StatusKind::Conditional,
            label: "conditional".into(),
            muted: false,
        },
        (None, _) if s.swaps == Some(0) => Cell {
            text: "none in horizon".into(),
            kind: StatusKind::Conditional,
            label: "conditional".into(),
            muted: false,
        },
        (None, _) => none("—"),
    };
    let net = match s.net_twh {
        Some(v) => Cell {
            text: format!("{v:.2}"),
            kind: StatusKind::Conditional,
            label: "conditional".into(),
            muted: false,
        },
        None => none("—"),
    };
    let breeding = match s.breeding {
        Some((m, se)) => Cell {
            text: format!("{m:.4} ± {se:.4}"),
            kind: StatusKind::Calculated,
            label: "calculated".into(),
            muted: false,
        },
        None => none("—"),
    };
    let flux = match s.magnet_flux {
        Some((m, se)) => Cell {
            text: format!("{} ± {}", fmt_sig(m), fmt_sig(se)),
            kind: StatusKind::Calculated,
            label: "calculated".into(),
            muted: false,
        },
        None => none("—"),
    };
    [swaps, first, net, breeding, flux]
}

/// Signed value with a unit; a value that rounds to zero carries no sign.
fn signed(v: f64, decimals: usize, unit: &str) -> String {
    let text = format!("{v:+.decimals$}");
    let text = if text[1..].chars().all(|c| c == '0' || c == '.') {
        text[1..].to_string()
    } else {
        text
    };
    if unit.is_empty() {
        text
    } else {
        format!("{text} {unit}")
    }
}

fn delta_cells(c: &Contrast) -> [Option<(String, StatusKind, &'static str)>; 5] {
    let sampled = |v: Option<(f64, bool)>, decimals: usize| {
        v.map(|(x, resolved)| {
            (
                signed(x, decimals, "%"),
                if resolved {
                    StatusKind::Checked
                } else {
                    StatusKind::Partial
                },
                if resolved {
                    "resolved >2σ"
                } else {
                    "within noise"
                },
            )
        })
    };
    let cond = |text: String| Some((text, StatusKind::Conditional, "conditional"));
    [
        sampled(c.breeding_pct, 2),
        sampled(c.flux_pct, 1),
        c.swaps.and_then(|v| cond(signed(v as f64, 0, ""))),
        c.first_swap_y.and_then(|v| cond(signed(v, 1, "y"))),
        c.net_twh.and_then(|v| cond(signed(v, 2, "TWh"))),
    ]
}

fn page_footer(cv: &mut Canvas, text: &str, page: usize, pages: usize) {
    cv.hline(MARGIN, PAGE_W - MARGIN, 768.0, RULE, 0.6);
    cv.body(MARGIN, 778.0, &fit_text(text, 440.0, 6.6), 6.6, MUTED);
    cv.right(
        PAGE_W - MARGIN,
        778.0,
        &format!("Page {page} of {pages}"),
        6.6,
        MUTED,
        FontKind::Body,
    );
    cv.body(MARGIN, 786.0, RESEARCH_SCREENING_STATEMENT, 6.6, MUTED);
}

/// Page one. Returns the y reached.
fn page_one(cv: &mut Canvas, c: &PdfContent) -> f32 {
    let mut y = MARGIN;
    let mut text_w = CONTENT_W;
    let mut image_bottom = MARGIN;
    if let Some(png) = c.view_image {
        let box_w = 196.0;
        let box_h = 128.0;
        if let Some((w, h)) = cv.image(png, PAGE_W - MARGIN, MARGIN, box_w, box_h) {
            image_bottom = MARGIN + h;
            text_w = CONTENT_W - box_w - 14.0;
            cv.rect(
                PAGE_W - MARGIN - w,
                MARGIN,
                w,
                h,
                0.0,
                None,
                Some((RULE, 0.6)),
            );
        }
    }
    cv.body(MARGIN, y + 7.0, "FARIS STUDY SUMMARY", 7.0, MUTED);
    y += 12.0;
    // One line at 18 pt when it fits, otherwise smaller, otherwise two lines.
    let (title_size, title_lines) = [18.0_f32, 15.0, 12.5]
        .into_iter()
        .map(|size| (size, wrap_lines(c.title, text_w, size)))
        .find(|(_, lines)| lines.len() <= 1)
        .unwrap_or_else(|| (12.5, wrap_lines(c.title, text_w, 12.5)));
    for line in title_lines.iter().take(2) {
        cv.bold(MARGIN, y + title_size, line, title_size, INK);
        y += title_size * 1.22;
    }
    for line in wrap_lines(&c.subtitle, text_w, 7.6).iter().take(2) {
        cv.body(MARGIN, y + 7.6, line, 7.6, MUTED);
        y += 10.0;
    }
    y += 6.0;
    cv.bold(MARGIN, y + 8.0, "Question", 8.2, INK);
    y += 12.0;
    y = cv.paragraph(MARGIN, y, text_w, &c.question, 9.2, 12.0, INK);
    y += 6.0;
    // Takeaway block.
    if !c.takeaways.is_empty() {
        let text = c.takeaways.join(" ");
        let inner = text_w - 16.0;
        let lines = wrap_lines(&text, inner, 8.6);
        let h = lines.len() as f32 * 11.4 + 22.0;
        cv.rect(
            MARGIN,
            y,
            text_w,
            h,
            4.0,
            Some((STRIPE, 1.0)),
            Some((RULE, 0.6)),
        );
        cv.bold(MARGIN + 8.0, y + 11.0, "What the study says", 7.4, INK);
        let pw = cv.pill(
            MARGIN + 8.0 + text_width(FontKind::Body, "What the study says", 7.4) + 10.0,
            y + 4.2,
            "conditional",
            StatusKind::Conditional,
            6.2,
        );
        let _ = pw;
        let mut ty = y + 15.0;
        for line in &lines {
            cv.body(MARGIN + 8.0, ty + 8.6, line, 8.6, INK);
            ty += 11.4;
        }
        y += h + 8.0;
    } else {
        cv.body(
            MARGIN,
            y + 8.0,
            "No takeaway sentence: the recorded data do not support one.",
            8.4,
            MUTED,
        );
        y += 14.0;
    }
    y = y.max(image_bottom + 10.0);

    // Comparison table.
    cv.bold(MARGIN, y + 8.0, "The arrangements compared", 9.0, INK);
    y += 14.0;
    let label_w = 138.0;
    let n = c.arrangements.len().max(1);
    let col_w = (CONTENT_W - label_w) / n as f32;
    for (i, d) in c.arrangements.iter().enumerate() {
        let x = MARGIN + label_w + col_w * i as f32;
        let color = d.input.arrangement.rgb();
        cv.rect(x + 2.0, y, col_w - 4.0, 3.2, 1.2, Some((color, 1.0)), None);
        cv.bold(x + 4.0, y + 13.0, d.input.arrangement.label(), 7.8, INK);
        let blanket = if d.input.arrangement.breeder {
            BREEDER_BLANKET_M
        } else {
            REFERENCE_BLANKET_M
        };
        cv.body(
            x + 4.0,
            y + 21.5,
            &format!(
                "{:.2} / {:.2} m blanket / shield",
                blanket,
                BLANKET_PLUS_SHIELD_M - blanket
            ),
            6.0,
            MUTED,
        );
    }
    y += 27.0;
    let horizon = c.study.horizon_years;
    let any_total = c.arrangements.iter().any(|d| d.summary.breeding_is_total);
    let row_labels: [(String, &str); 5] = [
        (
            format!("Magnet swaps in {horizon:.0} y"),
            "count, conditional on the authored limit",
        ),
        (
            "First magnet swap".into(),
            "calendar year ± first-order sampling",
        ),
        ("Lifetime net electricity".into(), "TWh, signed"),
        (
            if any_total {
                "Tritium breeding ratio".into()
            } else {
                "Breeder H3 per source".into()
            },
            "H3 per source neutron ± 1 SE",
        ),
        ("Magnet-region flux".into(), "n/m²/s ± 1 SE"),
    ];
    let cells: Vec<[Cell; 5]> = c.arrangements.iter().map(|d| comparison_cells(d)).collect();
    let row_h = 25.0;
    for (r, (name, unit)) in row_labels.iter().enumerate() {
        if r % 2 == 0 {
            cv.rect(
                MARGIN,
                y - 2.0,
                CONTENT_W,
                row_h,
                0.0,
                Some((STRIPE, 1.0)),
                None,
            );
        }
        cv.body(MARGIN + 4.0, y + 8.6, name, 7.8, INK);
        cv.body(MARGIN + 4.0, y + 17.0, unit, 5.9, MUTED);
        for (i, row) in cells.iter().enumerate() {
            let cell = &row[r];
            let x = MARGIN + label_w + col_w * i as f32;
            if cell.muted {
                cv.body(x + 4.0, y + 8.6, &cell.text, 7.6, MUTED);
            } else {
                cv.mono(x + 4.0, y + 8.6, &cell.text, 7.6, INK);
            }
            cv.pill(x + 4.0, y + 11.6, &cell.label, cell.kind, 5.9);
        }
        y += row_h;
    }
    cv.hline(MARGIN, PAGE_W - MARGIN, y - 1.0, RULE, 0.6);
    if c.uncertainty.is_some() {
        cv.body(
            MARGIN + 4.0,
            y + 6.2,
            "Monte Carlo ranges for the history values above, and their distributions, are on page 3 (transport sampling uncertainty only).",
            6.2,
            MUTED,
        );
        y += 9.0;
    }
    y += 8.0;

    // What changes.
    cv.bold(MARGIN, y + 8.0, "What changes", 9.0, INK);
    cv.body(
        MARGIN + 62.0,
        y + 8.0,
        "second arrangement minus the first · transport changes carry a 2σ screening flag",
        6.4,
        MUTED,
    );
    y += 13.0;
    let name_w = 150.0;
    let dw = (CONTENT_W - name_w) / 5.0;
    for (i, h) in [
        "Δ breeding",
        "Δ magnet flux",
        "Δ magnet swaps",
        "Δ first swap",
        "Δ net electricity",
    ]
    .iter()
    .enumerate()
    {
        cv.bold(MARGIN + name_w + dw * i as f32 + 4.0, y + 7.0, h, 6.8, INK);
    }
    y += 10.0;
    cv.hline(MARGIN, PAGE_W - MARGIN, y, RULE, 0.6);
    let rows: [(&str, &Contrast); 4] = [
        (
            "Breeder-heavy − reference · with port",
            &c.study.allocation_with_port,
        ),
        (
            "Breeder-heavy − reference · no port",
            &c.study.allocation_no_port,
        ),
        ("Port − no port · reference", &c.study.port_reference),
        ("Port − no port · breeder-heavy", &c.study.port_breeder),
    ];
    let dh = 21.0;
    for (r, (name, contrast)) in rows.iter().enumerate() {
        if r % 2 == 1 {
            cv.rect(
                MARGIN,
                y + 1.0,
                CONTENT_W,
                dh,
                0.0,
                Some((STRIPE, 1.0)),
                None,
            );
        }
        cv.body(MARGIN + 4.0, y + 11.0, name, 7.0, INK);
        for (i, cell) in delta_cells(contrast).iter().enumerate() {
            let x = MARGIN + name_w + dw * i as f32 + 4.0;
            match cell {
                Some((text, kind, label)) => {
                    cv.mono(x, y + 9.0, text, 7.2, INK);
                    cv.pill(x, y + 11.6, label, *kind, 5.7);
                }
                None => cv.body(x, y + 9.0, "—", 7.2, MUTED),
            }
        }
        y += dh;
    }
    y += 8.0;
    let chart = (c.timeline)((BOTTOM - y).clamp(150.0, 250.0));
    cv.svg(&chart, MARGIN, y);
    y + chart.height
}

/// Page two with every font size multiplied by `k`. Returns the y reached.
fn page_two(cv: &mut Canvas, c: &PdfContent, k: f32) -> f32 {
    let mut y = MARGIN;
    cv.bold(MARGIN, y + 9.0, "Allocation sweep", 10.0, INK);
    cv.body(
        MARGIN + 82.0,
        y + 9.0,
        &fit_text(&c.sweep_note, CONTENT_W - 82.0, 7.0),
        7.0,
        MUTED,
    );
    y += 16.0;
    if let Some(charts) = c.sweep_charts {
        for (i, chart) in charts.iter().enumerate() {
            cv.svg(chart, MARGIN + (chart.width + 3.0) * i as f32, y);
        }
        y += charts[0].height + 6.0;
    } else {
        cv.body(
            MARGIN,
            y + 8.0,
            "No allocation-sweep records are loaded.",
            8.0,
            MUTED,
        );
        y += 16.0;
    }
    if !c.sweep_findings.is_empty() {
        cv.bold(MARGIN, y + 8.0, "Findings", 8.4, INK);
        y += 12.0;
        for (kind, text) in &c.sweep_findings {
            let label = if *kind == StatusKind::Calculated {
                "calculated"
            } else {
                "conditional"
            };
            let size = 7.4 * k;
            cv.pill(MARGIN, y + 0.8, label, *kind, 5.8 * k);
            let lines = wrap_lines(text, CONTENT_W - 52.0, size);
            let mut ly = y;
            for line in &lines {
                cv.body(MARGIN + 50.0, ly + size, line, size, INK);
                ly += size * 1.32;
            }
            y = ly.max(y + 10.0) + 2.0;
        }
        y += 2.0;
    }

    // Assumptions.
    cv.bold(
        MARGIN,
        y + 8.0,
        "Assumptions behind the histories",
        9.0,
        INK,
    );
    cv.body(
        MARGIN + 150.0,
        y + 8.0,
        "authored values are tunable scenario inputs; full provenance text is in data/assumptions.csv",
        6.2 * k,
        MUTED,
    );
    y += 13.0;
    let widths = [128.0, 82.0, 62.0, 56.0, 212.0];
    let mut x = MARGIN;
    for (i, h) in ["Assumption", "Value", "Unit", "Kind", "Provenance"]
        .iter()
        .enumerate()
    {
        cv.bold(x + 3.0, y + 6.4 * k, h, 6.4 * k, INK);
        x += widths[i];
    }
    y += 9.0 * k;
    cv.hline(MARGIN, PAGE_W - MARGIN, y, RULE, 0.6);
    let size = 6.9 * k;
    let psize = 6.4 * k;
    for (r, a) in c.assumptions.iter().enumerate() {
        let mut lines = wrap_lines(&first_sentence(&a.provenance), widths[4] - 6.0, psize);
        if lines.len() > 2 {
            lines.truncate(2);
            let last = lines.pop().unwrap_or_default();
            lines.push(fit_text(&format!("{last} …"), widths[4] - 6.0, psize));
        }
        let rh = (lines.len().max(1) as f32 * psize * 1.2 + 3.4 * k).max(10.0 * k);
        if r % 2 == 1 {
            cv.rect(
                MARGIN,
                y + 0.5,
                CONTENT_W,
                rh,
                0.0,
                Some((STRIPE, 1.0)),
                None,
            );
        }
        let base = y + 1.2 + size;
        cv.body(
            MARGIN + 3.0,
            base,
            &fit_text(&a.name, widths[0] - 6.0, size),
            size,
            INK,
        );
        cv.text(
            MARGIN + widths[0] + 3.0,
            base,
            &fit_text_in(FontKind::Mono, &a.value, widths[1] - 6.0, size - 0.3),
            size - 0.3,
            INK,
            FontKind::Mono,
            false,
        );
        cv.body(
            MARGIN + widths[0] + widths[1] + 3.0,
            base,
            &fit_text(&a.unit, widths[2] - 6.0, size),
            size,
            MUTED,
        );
        cv.pill(
            MARGIN + widths[0] + widths[1] + widths[2] + 3.0,
            y + 1.4,
            a.kind.label(),
            a.kind,
            5.6 * k,
        );
        let mut ly = y + 1.2;
        for line in &lines {
            cv.body(
                MARGIN + widths[0] + widths[1] + widths[2] + widths[3] + 3.0,
                ly + psize,
                line,
                psize,
                MUTED,
            );
            ly += psize * 1.2;
        }
        y += rh;
    }
    cv.hline(MARGIN, PAGE_W - MARGIN, y + 0.5, RULE, 0.6);
    y += 9.0;

    // Caveats and unknowns.
    cv.bold(MARGIN, y + 8.0, "Caveats and unknowns", 9.0, INK);
    cv.body(
        MARGIN + 100.0,
        y + 8.0,
        "each item says why it applies and what would settle it",
        6.2 * k,
        MUTED,
    );
    y += 13.0;
    let cw = [128.0, 226.0, 186.0];
    let size = 6.9 * k;
    let lead = size * 1.2;
    for (r, caveat) in c.caveats.iter().enumerate() {
        let item = wrap_lines(&caveat.item, cw[0] - 8.0, 7.2 * k);
        let why = wrap_lines(&caveat.why, cw[1] - 8.0, size);
        let settle = wrap_lines(&caveat.settle, cw[2] - 6.0, size);
        let left_h = item.len() as f32 * 8.4 * k + 9.0 * k;
        let h = left_h
            .max(why.len() as f32 * lead)
            .max(settle.len() as f32 * lead)
            + 3.0;
        if r % 2 == 0 {
            cv.rect(MARGIN, y, CONTENT_W, h, 0.0, Some((STRIPE, 1.0)), None);
        }
        let mut iy = y + 1.5;
        for line in &item {
            cv.bold(MARGIN + 3.0, iy + 7.2 * k, line, 7.2 * k, INK);
            iy += 8.4 * k;
        }
        cv.pill(
            MARGIN + 3.0,
            iy + 0.4,
            caveat.kind.label(),
            caveat.kind,
            5.4 * k,
        );
        let mut wy = y + 1.5;
        for line in &why {
            cv.body(MARGIN + cw[0] + 4.0, wy + size, line, size, INK);
            wy += lead;
        }
        let mut sy = y + 1.5;
        for line in &settle {
            cv.body(MARGIN + cw[0] + cw[1] + 3.0, sy + size, line, size, INK);
            sy += lead;
        }
        y += h;
    }
    y
}

/// Page three: every history output beside its Monte Carlo range or
/// distribution, one column per arrangement. Returns the y reached.
fn page_three(cv: &mut Canvas, c: &PdfContent, report: &UncertaintyReport) -> f32 {
    let mut y = MARGIN;
    cv.bold(
        MARGIN,
        y + 10.0,
        "Uncertainty in the operating history",
        10.0,
        INK,
    );
    y += 17.0;
    cv.pill(
        MARGIN,
        y - 1.0,
        "transport sampling only",
        StatusKind::Partial,
        6.2,
    );
    cv.body(
        MARGIN + 82.0,
        y + 5.6,
        &fit_text(SCOPE_LINE, CONTENT_W - 82.0, 6.8),
        6.8,
        MUTED,
    );
    y += 12.0;
    y = cv.paragraph(
        MARGIN,
        y,
        CONTENT_W,
        &format!(
            "{SCOPE_DETAIL} P5 and P95 are the 5th and 95th percentiles of the sampled histories; the median sits between them. A count is shown as the share of samples that reach it."
        ),
        6.6,
        8.4,
        MUTED,
    );
    y += 4.0;
    for (arrangements, why, next) in report.not_evaluated_notes() {
        let text = format!(
            "No uncertainty range for {}: {why}. Next step: {next}.",
            arrangements.join(", ")
        );
        let lines = wrap_lines(&text, CONTENT_W - 16.0, 7.0);
        let h = lines.len() as f32 * 9.0 + 7.0;
        cv.rect(
            MARGIN,
            y,
            CONTENT_W,
            h,
            3.0,
            Some((STRIPE, 1.0)),
            Some((RULE, 0.6)),
        );
        let mut ty = y + 4.0;
        for line in &lines {
            cv.body(MARGIN + 8.0, ty + 7.0, line, 7.0, INK);
            ty += 9.0;
        }
        y += h + 5.0;
    }
    y += 2.0;

    let label_w = 112.0;
    let n = report.columns.len().max(1);
    let col_w = (CONTENT_W - label_w) / n as f32;
    for (i, column) in report.columns.iter().enumerate() {
        let x = MARGIN + label_w + col_w * i as f32;
        cv.rect(
            x + 2.0,
            y,
            col_w - 4.0,
            3.0,
            1.2,
            Some((column.arrangement.rgb(), 1.0)),
            None,
        );
        cv.bold(x + 4.0, y + 12.0, column.arrangement.label(), 7.4, INK);
        let status = match &column.status {
            ColumnStatus::Evaluated { samples, .. } => format!("{samples} samples"),
            ColumnStatus::NotEvaluated { .. } => "not evaluated".into(),
            ColumnStatus::Failed(_) => "not calculated".into(),
            ColumnStatus::NotCalculated => "not calculated".into(),
            ColumnStatus::NoHistory => "no history".into(),
        };
        cv.body(x + 4.0, y + 20.0, &status, 6.0, MUTED);
    }
    y += 25.0;
    cv.hline(MARGIN, PAGE_W - MARGIN, y, RULE, 0.6);
    let size = 6.5;
    let lead = 7.8;
    for (r, (name, label)) in report.row_labels().iter().enumerate() {
        // Wrap every cell first so the row is as tall as its tallest.
        let cells: Vec<(String, Vec<String>)> = report
            .columns
            .iter()
            .map(|column| {
                column
                    .rows
                    .iter()
                    .find(|row| row.name == *name)
                    .map_or_else(
                        || ("—".to_string(), Vec::new()),
                        |row| {
                            let lines = match &row.result {
                                Some(result) => wrap_lines(&result.text, col_w - 8.0, size),
                                None => vec!["no uncertainty range".into()],
                            };
                            (format!("nominal {}", row.nominal), lines)
                        },
                    )
            })
            .collect();
        let lines_max = cells.iter().map(|(_, l)| l.len()).max().unwrap_or(0);
        let h = (lines_max as f32 + 1.0) * lead + 5.0;
        if y + h > BOTTOM {
            break;
        }
        if r % 2 == 0 {
            cv.rect(
                MARGIN,
                y + 0.5,
                CONTENT_W,
                h,
                0.0,
                Some((STRIPE, 1.0)),
                None,
            );
        }
        let label_lines = wrap_lines(label, label_w - 8.0, 7.2);
        let mut ly = y + 2.0;
        for line in &label_lines {
            cv.bold(MARGIN + 4.0, ly + 7.2, line, 7.2, INK);
            ly += 8.6;
        }
        for (i, (nominal, lines)) in cells.iter().enumerate() {
            let x = MARGIN + label_w + col_w * i as f32 + 4.0;
            cv.text(
                x,
                y + 2.0 + 6.6,
                &fit_text_in(FontKind::Mono, nominal, col_w - 8.0, 6.6),
                6.6,
                INK,
                FontKind::Mono,
                false,
            );
            let muted = lines.first().is_some_and(|l| l == "no uncertainty range");
            let mut ty = y + 2.0 + lead;
            for line in lines {
                cv.body(x, ty + size, line, size, if muted { MUTED } else { INK });
                ty += lead;
            }
        }
        y += h;
    }
    cv.hline(MARGIN, PAGE_W - MARGIN, y + 0.5, RULE, 0.6);
    y += 8.0;
    // The band charts, when both fit under the table.
    let needed: f32 = c.band_charts.iter().map(|b| b.height + 4.0).sum();
    if !c.band_charts.is_empty() && y + needed <= BOTTOM {
        for chart in &c.band_charts {
            cv.svg(chart, MARGIN, y);
            y += chart.height + 4.0;
        }
    }
    y
}

/// Largest font scale at which page two fits above the footer.
fn page_two_scale(c: &PdfContent, body: &Font, mono: &Font) -> f32 {
    for k in [1.0, 0.95, 0.9, 0.86, 0.82, 0.78] {
        let mut dry = Canvas {
            surface: None,
            body: body.clone(),
            mono: mono.clone(),
        };
        if page_two(&mut dry, c, k) <= BOTTOM {
            return k;
        }
    }
    0.74
}

/// Heights reached by the two pages at their final scale; for layout tests.
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub struct Extent {
    pub page_one_bottom: f32,
    pub page_two_bottom: f32,
    pub page_two_scale: f32,
    pub page_three_bottom: Option<f32>,
}

#[cfg(test)]
pub fn measure(c: &PdfContent) -> Result<Extent, String> {
    let (body, mono) = fonts()?;
    let mut dry = Canvas {
        surface: None,
        body: body.clone(),
        mono: mono.clone(),
    };
    let page_one_bottom = page_one(&mut dry, c);
    let k = page_two_scale(c, &body, &mono);
    let page_two_bottom = page_two(&mut dry, c, k);
    let page_three_bottom = c.uncertainty.map(|u| page_three(&mut dry, c, u));
    Ok(Extent {
        page_one_bottom,
        page_two_bottom,
        page_two_scale: k,
        page_three_bottom,
    })
}

fn fonts() -> Result<(Font, Font), String> {
    let body = Font::new(Arc::new(BODY_FONT.to_vec()).into(), 0).ok_or("body font rejected")?;
    let mono = Font::new(Arc::new(MONO_FONT.to_vec()).into(), 0).ok_or("mono font rejected")?;
    Ok((body, mono))
}

pub fn render_pdf(c: &PdfContent) -> Result<Vec<u8>, String> {
    let (body, mono) = fonts()?;
    let k = page_two_scale(c, &body, &mono);
    let pages = if c.uncertainty.is_some() { 3 } else { 2 };
    let mut document = Document::new();
    document.set_metadata(
        Metadata::new()
            .title(format!("{} · FARIS study summary", c.title))
            .creator("FARIS".into()),
    );
    {
        let mut page = document
            .start_page_with(PageSettings::from_wh(PAGE_W, PAGE_H).ok_or("page size rejected")?);
        let mut surface = page.surface();
        let mut cv = Canvas {
            surface: Some(&mut surface),
            body: body.clone(),
            mono: mono.clone(),
        };
        page_one(&mut cv, c);
        page_footer(&mut cv, &c.footer, 1, pages);
        surface.finish();
        page.finish();
    }
    {
        let mut page = document
            .start_page_with(PageSettings::from_wh(PAGE_W, PAGE_H).ok_or("page size rejected")?);
        let mut surface = page.surface();
        let mut cv = Canvas {
            surface: Some(&mut surface),
            body: body.clone(),
            mono: mono.clone(),
        };
        page_two(&mut cv, c, k);
        page_footer(&mut cv, &c.footer, 2, pages);
        surface.finish();
        page.finish();
    }
    if let Some(report) = c.uncertainty {
        let mut page = document
            .start_page_with(PageSettings::from_wh(PAGE_W, PAGE_H).ok_or("page size rejected")?);
        let mut surface = page.surface();
        let mut cv = Canvas {
            surface: Some(&mut surface),
            body,
            mono,
        };
        page_three(&mut cv, c, report);
        page_footer(&mut cv, &c.footer, 3, pages);
        surface.finish();
        page.finish();
    }
    document
        .finish()
        .map_err(|e| format!("PDF assembly failed: {e:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sentence_skips_abbreviations() {
        assert_eq!(
            first_sentence("Literature value cited by Sorbom et al. 2015 (ARC). Demountable coil."),
            "Literature value cited by Sorbom et al. 2015 (ARC)."
        );
        assert_eq!(first_sentence("No period at all"), "No period at all");
    }

    #[test]
    fn signed_values_drop_the_sign_of_zero() {
        assert_eq!(signed(-0.0004, 2, "TWh"), "0.00 TWh");
        assert_eq!(signed(-0.55, 2, "TWh"), "-0.55 TWh");
        assert_eq!(signed(3.0, 0, ""), "+3");
        assert_eq!(signed(0.0, 0, ""), "0");
        assert_eq!(signed(2.54, 2, "%"), "+2.54 %");
    }

    #[test]
    fn fit_text_shortens_with_an_ellipsis() {
        let long = "a very long provenance statement that cannot possibly fit in the cell";
        let cut = fit_text(long, 60.0, 6.0);
        assert!(cut.ends_with('…'));
        assert!(text_width(FontKind::Body, &cut, 6.0) <= 60.5);
        assert_eq!(fit_text("short", 60.0, 6.0), "short");
    }
}
