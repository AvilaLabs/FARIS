//! A small SVG writer for the charts: text, lines, rectangles, polylines.
//! Every chart is built once as a string here; the same string is saved as
//! `.svg`, rasterised to `.png` and drawn into the PDF.

use crate::fonts::{BODY_FAMILY, FontKind, MONO_FAMILY, text_width};
use std::fmt::Write;

pub type Rgb = [u8; 3];

pub fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// Darken a colour for text or strokes on white paper.
pub fn darken(c: Rgb, factor: f32) -> Rgb {
    [
        (f32::from(c[0]) * factor) as u8,
        (f32::from(c[1]) * factor) as u8,
        (f32::from(c[2]) * factor) as u8,
    ]
}

/// Blend a colour toward white (`amount` 0 = unchanged, 1 = white).
pub fn tint(c: Rgb, amount: f32) -> Rgb {
    let mix = |v: u8| (f32::from(v) + (255.0 - f32::from(v)) * amount).round() as u8;
    [mix(c[0]), mix(c[1]), mix(c[2])]
}

pub const INK: Rgb = [34, 38, 46];
pub const MUTED: Rgb = [102, 108, 120];
pub const GRID: Rgb = [226, 229, 235];
pub const AXIS: Rgb = [140, 146, 158];

pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Start,
    Middle,
    End,
}

#[derive(Clone, Copy)]
pub struct TextStyle {
    pub size: f32,
    pub fill: Rgb,
    pub anchor: Anchor,
    pub bold: bool,
    pub mono: bool,
    pub rotate: Option<f32>,
}

impl TextStyle {
    pub fn new(size: f32, fill: Rgb) -> Self {
        Self {
            size,
            fill,
            anchor: Anchor::Start,
            bold: false,
            mono: false,
            rotate: None,
        }
    }
    pub fn anchor(mut self, anchor: Anchor) -> Self {
        self.anchor = anchor;
        self
    }
    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }
    pub fn rotate(mut self, degrees: f32) -> Self {
        self.rotate = Some(degrees);
        self
    }
}

pub struct Svg {
    width: f32,
    height: f32,
    body: String,
}

impl Svg {
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            body: String::new(),
        }
    }

    pub fn rect(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fill: Option<(Rgb, f32)>,
        stroke: Option<(Rgb, f32)>,
    ) {
        let _ = write!(
            self.body,
            "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\""
        );
        match fill {
            Some((c, o)) => {
                let _ = write!(self.body, " fill=\"{}\" fill-opacity=\"{o:.3}\"", hex(c));
            }
            None => self.body.push_str(" fill=\"none\""),
        }
        if let Some((c, w)) = stroke {
            let _ = write!(self.body, " stroke=\"{}\" stroke-width=\"{w:.2}\"", hex(c));
        }
        self.body.push_str("/>\n");
    }

    #[allow(clippy::too_many_arguments)]
    pub fn line(
        &mut self,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        color: Rgb,
        width: f32,
        dash: Option<&str>,
    ) {
        let _ = write!(
            self.body,
            "<line x1=\"{x1:.2}\" y1=\"{y1:.2}\" x2=\"{x2:.2}\" y2=\"{y2:.2}\" stroke=\"{}\" stroke-width=\"{width:.2}\"",
            hex(color)
        );
        if let Some(d) = dash {
            let _ = write!(self.body, " stroke-dasharray=\"{d}\"");
        }
        self.body.push_str("/>\n");
    }

    pub fn polyline(&mut self, points: &[(f32, f32)], color: Rgb, width: f32, dash: Option<&str>) {
        if points.len() < 2 {
            return;
        }
        let mut d = String::with_capacity(points.len() * 14);
        for (i, (x, y)) in points.iter().enumerate() {
            let _ = write!(d, "{}{x:.2} {y:.2} ", if i == 0 { "M" } else { "L" });
        }
        let _ = write!(
            self.body,
            "<path d=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{width:.2}\" stroke-linejoin=\"round\"",
            d.trim_end(),
            hex(color)
        );
        if let Some(dash) = dash {
            let _ = write!(self.body, " stroke-dasharray=\"{dash}\"");
        }
        self.body.push_str("/>\n");
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, fill: Rgb, stroke: Rgb, width: f32) {
        let _ = writeln!(
            self.body,
            "<circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{r:.2}\" fill=\"{}\" stroke=\"{}\" stroke-width=\"{width:.2}\"/>",
            hex(fill),
            hex(stroke)
        );
    }

    /// Text from plain content (escaped here).
    pub fn text(&mut self, x: f32, y: f32, content: &str, style: TextStyle) {
        self.text_markup(x, y, &escape(content), style);
    }

    /// Text from content that already contains markup (`<tspan>` runs).
    pub fn text_markup(&mut self, x: f32, y: f32, markup: &str, style: TextStyle) {
        // The embedded families first; generic fallbacks for other viewers.
        let family = if style.mono {
            format!("{MONO_FAMILY}, monospace")
        } else {
            format!("{BODY_FAMILY}, sans-serif")
        };
        let anchor = match style.anchor {
            Anchor::Start => "start",
            Anchor::Middle => "middle",
            Anchor::End => "end",
        };
        let _ = write!(
            self.body,
            "<text x=\"{x:.2}\" y=\"{y:.2}\" font-family=\"{family}\" font-size=\"{:.2}\" fill=\"{}\" text-anchor=\"{anchor}\"",
            style.size,
            hex(style.fill)
        );
        if style.bold {
            // Ubuntu Light is the app's only proportional face; a thin stroke
            // gives headings weight without a second font.
            let _ = write!(
                self.body,
                " stroke=\"{}\" stroke-width=\"{:.2}\" stroke-linejoin=\"round\"",
                hex(style.fill),
                style.size * 0.045
            );
        }
        if let Some(r) = style.rotate {
            let _ = write!(self.body, " transform=\"rotate({r:.1} {x:.2} {y:.2})\"");
        }
        let _ = writeln!(self.body, ">{markup}</text>");
    }

    /// Raw markup for a nested drawing (a complete `<svg>` element).
    pub fn embed(&mut self, markup: &str) {
        self.body.push_str(markup);
        if !markup.ends_with('\n') {
            self.body.push('\n');
        }
    }

    pub fn finish(self) -> String {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">\n<rect width=\"{w}\" height=\"{h}\" fill=\"#ffffff\"/>\n{body}</svg>\n",
            w = self.width,
            h = self.height,
            body = self.body
        )
    }
}

/// Scientific notation as SVG markup with a raised exponent: `3×10²²`.
pub fn sci_markup(v: f64, size: f32) -> String {
    if v == 0.0 {
        return "0".into();
    }
    if !v.is_finite() {
        return "—".into();
    }
    let sign = if v < 0.0 { "−" } else { "" };
    let (mantissa, exponent) = sci_parts(v.abs());
    if exponent == 0 {
        return format!("{sign}{mantissa}");
    }
    format!(
        "{sign}{mantissa}×10<tspan baseline-shift=\"super\" font-size=\"{:.2}\">{exponent}</tspan>",
        size * 0.7
    )
}

/// Approximate width of [`sci_markup`] output.
pub fn sci_width(v: f64, size: f32) -> f32 {
    if v == 0.0 || !v.is_finite() {
        return text_width(FontKind::Body, "0", size);
    }
    let (mantissa, exponent) = sci_parts(v.abs());
    if exponent == 0 {
        return text_width(FontKind::Body, &mantissa, size);
    }
    text_width(FontKind::Body, &format!("{mantissa}×10"), size)
        + text_width(FontKind::Body, &exponent.to_string(), size * 0.7)
}

fn sci_parts(a: f64) -> (String, i32) {
    let mut exponent = a.log10().floor() as i32;
    let mut mantissa = (a / 10f64.powi(exponent) * 10.0).round() / 10.0;
    if mantissa >= 10.0 {
        mantissa /= 10.0;
        exponent += 1;
    }
    let text = if (mantissa - mantissa.round()).abs() < 1e-9 {
        format!("{}", mantissa.round() as i64)
    } else {
        format!("{mantissa:.1}")
    };
    (text, exponent)
}

/// Plain-text scientific notation: `3×10²²`.
#[cfg(test)]
pub fn sci_text(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    if !v.is_finite() {
        return "—".into();
    }
    let sign = if v < 0.0 { "−" } else { "" };
    let (mantissa, exponent) = sci_parts(v.abs());
    if exponent == 0 {
        return format!("{sign}{mantissa}");
    }
    let sup: String = exponent
        .to_string()
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
        .collect();
    format!("{sign}{mantissa}×10{sup}")
}

/// Round tick values covering `lo..=hi` with about `target` divisions.
pub fn nice_ticks(lo: f64, hi: f64, target: usize) -> (Vec<f64>, f64) {
    let raw = (hi - lo) / target.max(1) as f64;
    if !(raw.is_finite() && raw > 0.0) {
        return (vec![lo], 1.0);
    }
    let magnitude = 10f64.powf(raw.log10().floor());
    let norm = raw / magnitude;
    let step = magnitude
        * if norm < 1.5 {
            1.0
        } else if norm < 3.0 {
            2.0
        } else if norm < 7.0 {
            5.0
        } else {
            10.0
        };
    let mut ticks = Vec::new();
    let mut v = (lo / step).ceil() * step;
    while v <= hi + step * 1e-9 {
        ticks.push(if v.abs() < step * 1e-9 { 0.0 } else { v });
        v += step;
    }
    (ticks, step)
}

/// 1-2-5 ticks inside a log range; denser when the range spans under a decade.
pub fn log_ticks(lo: f64, hi: f64) -> Vec<f64> {
    let collect = |mantissas: &[f64]| {
        let mut ticks = Vec::new();
        for k in lo.log10().floor() as i32..=hi.log10().ceil() as i32 {
            for m in mantissas {
                let v = m * 10f64.powi(k);
                if v >= lo * (1.0 - 1e-9) && v <= hi * (1.0 + 1e-9) {
                    ticks.push(v);
                }
            }
        }
        ticks
    };
    let ticks = collect(&[1.0, 2.0, 5.0]);
    if ticks.len() >= 3 {
        ticks
    } else {
        collect(&[1.0, 1.5, 2.0, 3.0, 4.0, 5.0, 7.0])
    }
}

pub fn decimals(step: f64) -> usize {
    (-step.log10().floor()).clamp(0.0, 6.0) as usize
}

/// Linear or log mapping of a data range onto a pixel range.
#[derive(Clone, Copy, Debug)]
pub struct Scale {
    pub lo: f64,
    pub hi: f64,
    pub log: bool,
    pub from_px: f32,
    pub to_px: f32,
}

impl Scale {
    pub fn px(&self, v: f64) -> f32 {
        let f = if self.log {
            (v.max(self.lo).log10() - self.lo.log10()) / (self.hi.log10() - self.lo.log10())
        } else {
            (v - self.lo) / (self.hi - self.lo)
        };
        self.from_px + (self.to_px - self.from_px) * f.clamp(0.0, 1.0) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scientific_markup_raises_the_exponent() {
        assert_eq!(sci_text(3e22), "3×10²²");
        assert_eq!(sci_text(1.6e14), "1.6×10¹⁴");
        assert!(sci_markup(3e22, 8.0).contains("baseline-shift"));
        assert_eq!(sci_markup(0.0, 8.0), "0");
    }

    #[test]
    fn ticks_are_round_and_cover_the_range() {
        let (ticks, step) = nice_ticks(0.0, 33.0, 5);
        assert_eq!(step, 5.0);
        assert_eq!(ticks.first(), Some(&0.0));
        assert!(*ticks.last().unwrap() <= 33.0);
        let log = log_ticks(1e13, 4e14);
        assert!(log.contains(&1e14) && log.contains(&2e14));
    }

    #[test]
    fn scale_maps_linear_and_log_and_clamps() {
        let lin = Scale {
            lo: 0.0,
            hi: 10.0,
            log: false,
            from_px: 0.0,
            to_px: 100.0,
        };
        assert_eq!(lin.px(5.0), 50.0);
        assert_eq!(lin.px(20.0), 100.0);
        let log = Scale {
            lo: 1.0,
            hi: 100.0,
            log: true,
            from_px: 100.0,
            to_px: 0.0,
        };
        assert!((log.px(10.0) - 50.0).abs() < 1e-4);
    }

    #[test]
    fn text_is_escaped() {
        let mut svg = Svg::new(10.0, 10.0);
        svg.text(0.0, 0.0, "a < b & c", TextStyle::new(8.0, INK));
        assert!(svg.finish().contains("a &lt; b &amp; c"));
    }
}
