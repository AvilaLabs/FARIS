//! The four study charts as SVG strings in a light print theme. Series colours
//! are the app's arrangement colours; the geometry (not the data) is decided
//! here, every number comes from the engine.

use crate::{
    fonts::{FontKind, text_width},
    svg::{
        AXIS, Anchor, GRID, INK, MUTED, Rgb, Scale, Svg, TextStyle, darken, decimals, log_ticks,
        nice_ticks, sci_markup, sci_width, tint,
    },
};
use faris_engine::{
    brief::{Arrangement, BREEDER_BLANKET_M, REFERENCE_BLANKET_M, StatusKind},
    sweep::{HistorySummary, TransportPoint},
};
use faris_model::RESEARCH_SCREENING_STATEMENT;

/// One finished chart: file stem, natural size in points and the SVG text.
#[derive(Clone, Debug)]
pub struct ChartSvg {
    pub name: &'static str,
    pub width: f32,
    pub height: f32,
    pub svg: String,
}

impl ChartSvg {
    /// The chart as saved to `charts/`: the drawing above a footer strip
    /// carrying the research-screening statement. The PDF draws `svg` itself
    /// and has the statement in its page footer.
    pub fn export_svg(&self) -> String {
        const SIZE: f32 = 5.8;
        let lines = wrap_lines(RESEARCH_SCREENING_STATEMENT, self.width - 8.0, SIZE);
        let mut svg = Svg::new(self.width, self.height + 3.0 + 7.5 * lines.len() as f32);
        svg.embed(&self.svg);
        for (i, line) in lines.iter().enumerate() {
            svg.text(
                4.0,
                self.height + 6.0 + 7.5 * i as f32,
                line,
                TextStyle::new(SIZE, MUTED),
            );
        }
        svg.finish()
    }
}

/// A P5-P95 band on a common time grid. A point without a value breaks it.
#[derive(Clone, Debug)]
pub struct Band {
    pub years: Vec<f64>,
    pub low: Vec<Option<f64>>,
    pub high: Vec<Option<f64>>,
}

impl Band {
    /// The largest and smallest value the band reaches, if it has any.
    fn extent(&self) -> Option<(f64, f64)> {
        let values = self.low.iter().chain(&self.high).flatten();
        values.fold(None, |acc, v| {
            Some(acc.map_or((*v, *v), |(lo, hi): (f64, f64)| (lo.min(*v), hi.max(*v))))
        })
    }

    /// One closed shape per run of consecutive points that have values.
    fn shapes(&self, xs: &Scale, ys: &Scale) -> Vec<Vec<(f32, f32)>> {
        let mut shapes = Vec::new();
        let mut run: Vec<usize> = Vec::new();
        let n = self.years.len().min(self.low.len()).min(self.high.len());
        let mut close = |run: &mut Vec<usize>| {
            if run.len() >= 2 {
                let mut shape: Vec<(f32, f32)> = run
                    .iter()
                    .filter_map(|k| Some((xs.px(self.years[*k]), ys.px(self.low[*k]?))))
                    .collect();
                shape.extend(
                    run.iter()
                        .rev()
                        .filter_map(|k| Some((xs.px(self.years[*k]), ys.px(self.high[*k]?)))),
                );
                shapes.push(shape);
            }
            run.clear();
        };
        for k in 0..n {
            if self.low[k].is_some() && self.high[k].is_some() {
                run.push(k);
            } else {
                close(&mut run);
            }
        }
        close(&mut run);
        shapes
    }
}

/// Magnet-fluence curve of one arrangement and its swap outages in years, with
/// the P5-P95 band of its ensemble when one was calculated.
#[derive(Clone, Debug)]
pub struct TimelineSeries {
    pub arrangement: Arrangement,
    pub points: Vec<[f64; 2]>,
    pub swap_spans_years: Vec<(f64, f64)>,
    pub band: Option<Band>,
}

/// A nominal curve with its band, for the tritium and electricity charts.
#[derive(Clone, Debug)]
pub struct BandSeries {
    pub arrangement: Arrangement,
    pub nominal: Vec<[f64; 2]>,
    pub band: Band,
}

/// The service-limit line of the timeline.
#[derive(Clone, Copy, Debug)]
pub struct LimitLine {
    pub value: f64,
    pub literature: bool,
}

pub const TIMELINE_SIZE: (f32, f32) = (540.0, 250.0);
pub const SWEEP_SIZE: (f32, f32) = (178.0, 176.0);
/// Height of the tritium and net-electricity band charts.
pub const BAND_CHART_HEIGHT: f32 = 150.0;

/// Swatch width, how to draw the swatch at (x, baseline y), and the label.
type LegendEntry = (f32, Box<dyn Fn(&mut Svg, f32, f32)>, String);

const TICK: f32 = 6.8;
const LABEL: f32 = 7.2;

fn title_block(svg: &mut Svg, x: f32, title: &str, subtitles: &[&str]) -> f32 {
    svg.text(x, 11.5, title, TextStyle::new(9.2, INK).bold());
    let mut y = 11.5;
    for line in subtitles {
        y += 9.6;
        svg.text(x, y, line, TextStyle::new(6.4, MUTED));
    }
    y
}

fn placeholder(name: &'static str, size: (f32, f32), title: &str, message: &str) -> ChartSvg {
    let mut svg = Svg::new(size.0, size.1);
    title_block(&mut svg, 4.0, title, &[]);
    svg.rect(
        4.0,
        24.0,
        size.0 - 8.0,
        size.1 - 30.0,
        None,
        Some((GRID, 0.8)),
    );
    let mut y = size.1 / 2.0;
    for line in wrap_lines(message, size.0 - 36.0, 7.2) {
        svg.text(
            size.0 / 2.0,
            y,
            &line,
            TextStyle::new(7.2, MUTED).anchor(Anchor::Middle),
        );
        y += 9.5;
    }
    ChartSvg {
        name,
        width: size.0,
        height: size.1,
        svg: svg.finish(),
    }
}

pub(crate) fn wrap_lines(text: &str, width: f32, size: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if !current.is_empty() && text_width(FontKind::Body, &candidate, size) > width {
            lines.push(std::mem::take(&mut current));
            current = word.to_string();
        } else {
            current = candidate;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn limit_markup(limit: &LimitLine) -> String {
    let prefix = if limit.literature {
        "REBCO limit "
    } else {
        "Magnet limit "
    };
    let suffix = if limit.literature {
        " n/m² (literature)"
    } else {
        " n/m² (authored limit)"
    };
    format!("{prefix}{}{suffix}", sci_markup(limit.value, 7.2))
}

/// Magnet fluence of the four arrangements against the service limit, with the
/// swap outages shaded.
pub fn timeline_chart(
    series: &[TimelineSeries],
    limit: Option<LimitLine>,
    horizon_years: f64,
    height: f32,
) -> ChartSvg {
    let (w, h) = (TIMELINE_SIZE.0, height);
    let title = format!("Magnet fluence over {horizon_years:.0} years of operation");
    if series.iter().all(|s| s.points.is_empty()) {
        return placeholder(
            "magnet-fluence-timeline",
            (w, h),
            &title,
            "No calculated operating history is available, so there is no fluence curve to draw.",
        );
    }
    let mut svg = Svg::new(w, h);
    let sub = title_block(
        &mut svg,
        4.0,
        &title,
        &[
            "component-average neutron fluence in the magnet envelope, n/m² · a swap resets it · conditional on authored assumptions",
        ],
    );

    // Legend, wrapped to the chart width.
    let mut entries: Vec<LegendEntry> = Vec::new();
    for s in series {
        let color = s.arrangement.rgb();
        let dashed = !s.arrangement.port;
        entries.push((
            22.0,
            Box::new(move |svg: &mut Svg, x: f32, y: f32| {
                svg.line(
                    x,
                    y - 2.4,
                    x + 17.0,
                    y - 2.4,
                    color,
                    1.8,
                    dashed.then_some("4 2.5"),
                );
            }),
            s.arrangement.label().to_string(),
        ));
    }
    entries.push((
        14.0,
        Box::new(|svg: &mut Svg, x: f32, y: f32| {
            svg.rect(x, y - 7.0, 9.0, 8.0, Some(([150, 156, 168], 0.28)), None);
        }),
        "magnet swap outage".into(),
    ));
    if series.iter().any(|s| s.band.is_some()) {
        entries.push((
            14.0,
            Box::new(|svg: &mut Svg, x: f32, y: f32| {
                svg.rect(x, y - 7.0, 9.0, 8.0, Some(([232, 104, 52], 0.30)), None);
            }),
            "P5-P95 band · transport sampling only".into(),
        ));
    }
    let mut x = 4.0;
    let mut y = sub + 12.0;
    for (swatch_w, draw, label) in &entries {
        let lw = text_width(FontKind::Body, label, LABEL);
        if x + swatch_w + lw > w - 4.0 && x > 4.0 {
            x = 4.0;
            y += 10.5;
        }
        draw(&mut svg, x, y);
        svg.text(x + swatch_w, y, label, TextStyle::new(LABEL, INK));
        x += swatch_w + lw + 12.0;
    }

    let (left, right, top, bottom) = (64.0, 12.0, y + 9.0, 32.0);
    let (pl, pr, pt, pb) = (left, w - right, top, h - bottom);
    let data_max = series
        .iter()
        .flat_map(|s| {
            s.points
                .iter()
                .map(|p| p[1])
                .chain(s.band.as_ref().and_then(Band::extent).map(|e| e.1))
        })
        .fold(0.0, f64::max);
    let top_value = limit.map_or(0.0, |l| l.value * 1.12).max(data_max * 1.05);
    let (ticks, _) = nice_ticks(0.0, top_value, 5);
    let y_hi = ticks.last().copied().unwrap_or(top_value).max(top_value);
    let ys = Scale {
        lo: 0.0,
        hi: y_hi,
        log: false,
        from_px: pb,
        to_px: pt,
    };
    let xs = Scale {
        lo: 0.0,
        hi: horizon_years.max(1.0),
        log: false,
        from_px: pl,
        to_px: pr,
    };
    for t in &ticks {
        let ty = ys.px(*t);
        svg.line(pl, ty, pr, ty, GRID, 0.6, None);
        svg.text_markup(
            pl - 4.0,
            ty + 2.4,
            &sci_markup(*t, TICK),
            TextStyle::new(TICK, MUTED).anchor(Anchor::End),
        );
    }
    let (xticks, _) = nice_ticks(0.0, xs.hi, 6);
    for t in &xticks {
        let tx = xs.px(*t);
        svg.line(tx, pt, tx, pb, GRID, 0.6, None);
        svg.line(tx, pb, tx, pb + 3.0, AXIS, 0.8, None);
        svg.text(
            tx,
            pb + 11.0,
            &format!("{t:.0}"),
            TextStyle::new(TICK, MUTED).anchor(Anchor::Middle),
        );
    }
    // Swap outages first so the curves stay on top.
    for s in series {
        for (a, b) in &s.swap_spans_years {
            let (x0, x1) = (xs.px(*a), xs.px(*b));
            svg.rect(
                x0,
                pt,
                (x1 - x0).max(0.8),
                pb - pt,
                Some((s.arrangement.rgb(), 0.2)),
                None,
            );
        }
    }
    // Bands under the curves.
    for s in series {
        if let Some(band) = &s.band {
            for shape in band.shapes(&xs, &ys) {
                svg.polygon(&shape, s.arrangement.rgb(), 0.22);
            }
        }
    }
    if let Some(l) = limit {
        let ly = ys.px(l.value);
        let color = darken(StatusKind::Literature.rgb(), 0.62);
        svg.line(pl, ly, pr, ly, color, 1.1, Some("5 3"));
        // At the left, where the curves have not yet climbed to the limit.
        svg.text_markup(
            pl + 4.0,
            ly - 3.4,
            &limit_markup(&l),
            TextStyle::new(7.2, color),
        );
    }
    for s in series {
        let pts: Vec<(f32, f32)> = s
            .points
            .iter()
            .map(|p| (xs.px(p[0]), ys.px(p[1])))
            .collect();
        svg.polyline(
            &pts,
            s.arrangement.rgb(),
            if s.arrangement.port { 1.5 } else { 1.3 },
            (!s.arrangement.port).then_some("4 2.5"),
        );
    }
    svg.line(pl, pb, pr, pb, AXIS, 0.9, None);
    svg.line(pl, pt, pl, pb, AXIS, 0.9, None);
    svg.text(
        (pl + pr) / 2.0,
        h - 5.0,
        "Calendar year of operation",
        TextStyle::new(LABEL, MUTED).anchor(Anchor::Middle),
    );
    svg.text(
        10.0,
        (pt + pb) / 2.0,
        "Magnet fluence (n/m²)",
        TextStyle::new(LABEL, MUTED)
            .anchor(Anchor::Middle)
            .rotate(-90.0),
    );
    ChartSvg {
        name: "magnet-fluence-timeline",
        width: w,
        height: h,
        svg: svg.finish(),
    }
}

/// A quantity over the horizon for each arrangement: the nominal curve and its
/// shaded P5-P95 band from the history ensemble.
pub fn band_chart(
    name: &'static str,
    title: &str,
    y_label: &str,
    series: &[BandSeries],
    horizon_years: f64,
    height: f32,
) -> ChartSvg {
    let (w, h) = (TIMELINE_SIZE.0, height);
    if series.iter().all(|s| s.nominal.is_empty()) {
        return placeholder(
            name,
            (w, h),
            title,
            "No calculated operating history is available, so there is no curve to draw.",
        );
    }
    let mut svg = Svg::new(w, h);
    let sub = title_block(
        &mut svg,
        4.0,
        title,
        &[
            "line: nominal history · shaded: P5-P95 of the Monte Carlo ensemble, transport sampling uncertainty only",
        ],
    );
    let mut x = 4.0;
    let mut y = sub + 12.0;
    for s in series {
        let color = s.arrangement.rgb();
        let dashed = !s.arrangement.port;
        let label = s.arrangement.label();
        let lw = text_width(FontKind::Body, label, LABEL);
        if x + 22.0 + lw > w - 4.0 && x > 4.0 {
            x = 4.0;
            y += 10.5;
        }
        svg.line(
            x,
            y - 2.4,
            x + 17.0,
            y - 2.4,
            color,
            1.8,
            dashed.then_some("4 2.5"),
        );
        svg.text(x + 22.0, y, label, TextStyle::new(LABEL, INK));
        x += 22.0 + lw + 12.0;
    }
    let (left, right, top, bottom) = (52.0, 12.0, y + 9.0, 32.0);
    let (pl, pr, pt, pb) = (left, w - right, top, h - bottom);
    let mut lo = 0.0_f64;
    let mut hi = f64::MIN;
    for s in series {
        for p in &s.nominal {
            lo = lo.min(p[1]);
            hi = hi.max(p[1]);
        }
        if let Some((a, b)) = s.band.extent() {
            lo = lo.min(a);
            hi = hi.max(b);
        }
    }
    if !hi.is_finite() || hi <= lo {
        hi = lo + 1.0;
    }
    let (ticks, step) = nice_ticks(lo, hi, 5);
    let y_lo = ticks.first().copied().unwrap_or(lo).min(lo);
    let y_hi = ticks.last().copied().unwrap_or(hi).max(hi);
    let ys = Scale {
        lo: y_lo,
        hi: y_hi,
        log: false,
        from_px: pb,
        to_px: pt,
    };
    let xs = Scale {
        lo: 0.0,
        hi: horizon_years.max(1.0),
        log: false,
        from_px: pl,
        to_px: pr,
    };
    for t in &ticks {
        let ty = ys.px(*t);
        svg.line(pl, ty, pr, ty, GRID, 0.6, None);
        svg.text(
            pl - 4.0,
            ty + 2.4,
            &format!("{:.*}", decimals(step), t),
            TextStyle::new(TICK, MUTED).anchor(Anchor::End),
        );
    }
    let (xticks, _) = nice_ticks(0.0, xs.hi, 6);
    for t in &xticks {
        let tx = xs.px(*t);
        svg.line(tx, pt, tx, pb, GRID, 0.6, None);
        svg.line(tx, pb, tx, pb + 3.0, AXIS, 0.8, None);
        svg.text(
            tx,
            pb + 11.0,
            &format!("{t:.0}"),
            TextStyle::new(TICK, MUTED).anchor(Anchor::Middle),
        );
    }
    for s in series {
        for shape in s.band.shapes(&xs, &ys) {
            svg.polygon(&shape, s.arrangement.rgb(), 0.22);
        }
    }
    for s in series {
        let pts: Vec<(f32, f32)> = s
            .nominal
            .iter()
            .map(|p| (xs.px(p[0]), ys.px(p[1])))
            .collect();
        svg.polyline(
            &pts,
            s.arrangement.rgb(),
            if s.arrangement.port { 1.5 } else { 1.3 },
            (!s.arrangement.port).then_some("4 2.5"),
        );
    }
    svg.line(pl, pb, pr, pb, AXIS, 0.9, None);
    svg.line(pl, pt, pl, pb, AXIS, 0.9, None);
    svg.text(
        (pl + pr) / 2.0,
        h - 5.0,
        "Calendar year of operation",
        TextStyle::new(LABEL, MUTED).anchor(Anchor::Middle),
    );
    svg.text(
        10.0,
        (pt + pb) / 2.0,
        y_label,
        TextStyle::new(LABEL, MUTED)
            .anchor(Anchor::Middle)
            .rotate(-90.0),
    );
    ChartSvg {
        name,
        width: w,
        height: h,
        svg: svg.finish(),
    }
}

fn is_named(blanket_m: f64) -> bool {
    [REFERENCE_BLANKET_M, BREEDER_BLANKET_M]
        .iter()
        .any(|m| (m - blanket_m).abs() < 1e-9)
}

struct SweepFrame {
    svg: Svg,
    plot: (f32, f32, f32, f32),
    xs: Scale,
}

/// Frame, title, x axis and footnote shared by the three sweep charts.
fn sweep_frame(
    title: &str,
    subtitles: &[&str],
    points: &[TransportPoint],
    right: f32,
) -> SweepFrame {
    let (w, h) = SWEEP_SIZE;
    let mut svg = Svg::new(w, h);
    let sub = title_block(&mut svg, 2.0, title, subtitles);
    let (left, top, bottom) = (42.0, sub + 9.0, 40.0);
    let (pl, pr, pt, pb) = (left, w - right, top, h - bottom);
    let first = points.first().map_or(0.0, |p| p.blanket_m);
    let last = points.last().map_or(1.0, |p| p.blanket_m);
    let pad = if points.len() > 1 {
        (last - first) / (points.len() - 1) as f64 * 0.6
    } else {
        0.05
    };
    let xs = Scale {
        lo: first - pad,
        hi: last + pad,
        log: false,
        from_px: pl,
        to_px: pr,
    };
    for p in points {
        let x = xs.px(p.blanket_m);
        svg.line(x, pb, x, pb + 2.8, AXIS, 0.8, None);
        svg.text(
            x,
            pb + 10.0,
            &format!("{:.2}", p.blanket_m),
            TextStyle::new(5.9, MUTED).anchor(Anchor::Middle),
        );
    }
    svg.text(
        (pl + pr) / 2.0,
        h - 15.5,
        "Blanket thickness (m)",
        TextStyle::new(6.8, MUTED).anchor(Anchor::Middle),
    );
    svg.text(
        2.0,
        h - 5.0,
        "filled: reference 0.45 m, breeder-heavy 0.55 m",
        TextStyle::new(5.8, MUTED),
    );
    SweepFrame {
        svg,
        plot: (pl, pt, pr, pb),
        xs,
    }
}

fn marker(svg: &mut Svg, x: f32, y: f32, named: bool, color: Rgb) {
    svg.circle(
        x,
        y,
        if named { 2.9 } else { 2.3 },
        if named { color } else { [255, 255, 255] },
        darken(color, 0.7),
        0.9,
    );
}

fn error_bar(svg: &mut Svg, x: f32, lo: f32, hi: f32, color: Rgb) {
    svg.line(x, lo, x, hi, color, 0.9, None);
    svg.line(x - 2.4, lo, x + 2.4, lo, color, 0.9, None);
    svg.line(x - 2.4, hi, x + 2.4, hi, color, 0.9, None);
}

fn finish_sweep(name: &'static str, f: SweepFrame) -> ChartSvg {
    ChartSvg {
        name,
        width: SWEEP_SIZE.0,
        height: SWEEP_SIZE.1,
        svg: f.svg.finish(),
    }
}

pub fn sweep_breeding_chart(points: &[TransportPoint]) -> ChartSvg {
    let title = "Tritium breeding ratio";
    if points.is_empty() {
        return placeholder(
            "sweep-breeding",
            SWEEP_SIZE,
            title,
            "No allocation-sweep records are loaded.",
        );
    }
    let mut f = sweep_frame(
        title,
        &["total H3 per source neutron", "±2 SE · axis not from zero"],
        points,
        8.0,
    );
    let (pl, pt, pr, pb) = f.plot;
    let lo = points
        .iter()
        .map(|p| p.breeding.interval_2sigma().0)
        .fold(f64::MAX, f64::min);
    let hi = points
        .iter()
        .map(|p| p.breeding.interval_2sigma().1)
        .fold(f64::MIN, f64::max);
    let pad = ((hi - lo) * 0.12).max(1e-9);
    let (ticks, step) = nice_ticks(lo - pad, hi + pad, 4);
    let ys = Scale {
        lo: (lo - pad).min(ticks.first().copied().unwrap_or(lo)),
        hi: (hi + pad).max(ticks.last().copied().unwrap_or(hi)),
        log: false,
        from_px: pb,
        to_px: pt,
    };
    let d = decimals(step);
    for t in &ticks {
        let y = ys.px(*t);
        f.svg.line(pl, y, pr, y, GRID, 0.6, None);
        f.svg.text(
            pl - 3.0,
            y + 2.3,
            &format!("{t:.d$}"),
            TextStyle::new(TICK - 0.4, MUTED).anchor(Anchor::End),
        );
    }
    let color = StatusKind::Calculated.rgb();
    let line: Vec<(f32, f32)> = points
        .iter()
        .map(|p| (f.xs.px(p.blanket_m), ys.px(p.breeding.mean)))
        .collect();
    f.svg.polyline(&line, tint(color, 0.15), 1.1, None);
    for p in points {
        let x = f.xs.px(p.blanket_m);
        let (a, b) = p.breeding.interval_2sigma();
        error_bar(&mut f.svg, x, ys.px(a), ys.px(b), darken(color, 0.7));
        marker(
            &mut f.svg,
            x,
            ys.px(p.breeding.mean),
            is_named(p.blanket_m),
            color,
        );
    }
    f.svg.line(pl, pb, pr, pb, AXIS, 0.9, None);
    f.svg.line(pl, pt, pl, pb, AXIS, 0.9, None);
    finish_sweep("sweep-breeding", f)
}

pub fn sweep_flux_chart(points: &[TransportPoint]) -> ChartSvg {
    let title = "Magnet-region neutron flux";
    if points.is_empty() {
        return placeholder(
            "sweep-magnet-flux",
            SWEEP_SIZE,
            title,
            "No allocation-sweep records are loaded.",
        );
    }
    let mut f = sweep_frame(title, &["n/m²/s, log scale", "±2 SE"], points, 8.0);
    let (pl, pt, pr, pb) = f.plot;
    let positive_lo = points
        .iter()
        .map(|p| p.magnet_flux.mean - 2.0 * p.magnet_flux.standard_error)
        .filter(|v| *v > 0.0)
        .fold(f64::MAX, f64::min);
    let lo = if positive_lo.is_finite() && positive_lo < f64::MAX {
        positive_lo
    } else {
        points
            .iter()
            .map(|p| p.magnet_flux.mean)
            .filter(|v| *v > 0.0)
            .fold(f64::MAX, f64::min)
    };
    let hi = points
        .iter()
        .map(|p| p.magnet_flux.interval_2sigma().1)
        .fold(f64::MIN, f64::max);
    if !(lo.is_finite() && lo > 0.0 && hi.is_finite() && hi > lo) {
        return placeholder(
            "sweep-magnet-flux",
            SWEEP_SIZE,
            title,
            "The recorded magnet flux is not positive, so a log axis cannot be drawn.",
        );
    }
    let (lo, hi) = (lo / 1.25, hi * 1.25);
    let ys = Scale {
        lo,
        hi,
        log: true,
        from_px: pb,
        to_px: pt,
    };
    for t in log_ticks(lo, hi) {
        let y = ys.px(t);
        f.svg.line(pl, y, pr, y, GRID, 0.6, None);
        f.svg.text_markup(
            pl - 3.0,
            y + 2.3,
            &sci_markup(t, TICK - 0.6),
            TextStyle::new(TICK - 0.6, MUTED).anchor(Anchor::End),
        );
    }
    let color = StatusKind::Calculated.rgb();
    let line: Vec<(f32, f32)> = points
        .iter()
        .map(|p| (f.xs.px(p.blanket_m), ys.px(p.magnet_flux.mean)))
        .collect();
    f.svg.polyline(&line, tint(color, 0.15), 1.1, None);
    for p in points {
        let x = f.xs.px(p.blanket_m);
        let (a, b) = p.magnet_flux.interval_2sigma();
        error_bar(
            &mut f.svg,
            x,
            ys.px(a.max(lo)),
            ys.px(b),
            darken(color, 0.7),
        );
        marker(
            &mut f.svg,
            x,
            ys.px(p.magnet_flux.mean),
            is_named(p.blanket_m),
            color,
        );
    }
    f.svg.line(pl, pb, pr, pb, AXIS, 0.9, None);
    f.svg.line(pl, pt, pl, pb, AXIS, 0.9, None);
    finish_sweep("sweep-magnet-flux", f)
}

/// Magnet swaps (bars) and lifetime net electricity (line, right axis) per
/// allocation. `summaries` is aligned with `points`.
pub fn sweep_history_chart(
    points: &[TransportPoint],
    summaries: &[Option<HistorySummary>],
) -> ChartSvg {
    let title = "Magnet swaps and net electricity";
    if points.is_empty() {
        return placeholder(
            "sweep-swaps-electricity",
            SWEEP_SIZE,
            title,
            "No allocation-sweep records are loaded.",
        );
    }
    let ready = summaries.len() == points.len() && summaries.iter().all(Option::is_some);
    if !ready {
        return placeholder(
            "sweep-swaps-electricity",
            SWEEP_SIZE,
            title,
            "Sweep histories were not calculated when this export ran.",
        );
    }
    let rows: Vec<(&TransportPoint, &HistorySummary)> = points
        .iter()
        .zip(summaries)
        .filter_map(|(p, s)| s.as_ref().map(|s| (p, s)))
        .collect();
    let permanent = rows.iter().any(|(_, s)| !s.magnets_replaceable);
    let mut f = sweep_frame(
        title,
        &[
            if permanent {
                "magnets permanent: no swaps modelled"
            } else {
                "bars: swaps in the horizon"
            },
            "line: net TWh (right axis) · conditional",
        ],
        points,
        38.0,
    );
    let (pl, pt, pr, pb) = f.plot;
    let max_swaps = rows
        .iter()
        .map(|(_, s)| s.magnet_replacements)
        .max()
        .unwrap_or(0)
        .max(1);
    let (sticks, _) = nice_ticks(0.0, f64::from(max_swaps) * 1.1, 4);
    let s_hi = sticks
        .last()
        .copied()
        .unwrap_or(f64::from(max_swaps))
        .max(f64::from(max_swaps) * 1.15)
        .max(1.0);
    let ss = Scale {
        lo: 0.0,
        hi: s_hi,
        log: false,
        from_px: pb,
        to_px: pt,
    };
    for t in &sticks {
        if t.fract() != 0.0 {
            continue;
        }
        let y = ss.px(*t);
        f.svg.line(pl, y, pr, y, GRID, 0.6, None);
        f.svg.text(
            pl - 3.0,
            y + 2.3,
            &format!("{t:.0}"),
            TextStyle::new(TICK - 0.4, MUTED).anchor(Anchor::End),
        );
    }
    let bar_w = ((pr - pl) / points.len() as f32 * 0.5).min(14.0);
    let bar_color = StatusKind::Conditional.rgb();
    if !permanent {
        for (p, s) in &rows {
            let x = f.xs.px(p.blanket_m);
            let y = ss.px(f64::from(s.magnet_replacements));
            f.svg.rect(
                x - bar_w / 2.0,
                y,
                bar_w,
                (pb - y).max(0.6),
                Some((bar_color, 0.8)),
                Some((darken(bar_color, 0.6), 0.7)),
            );
        }
    }
    let energy: Vec<(f64, f64)> = rows
        .iter()
        .filter_map(|(p, s)| s.net_electricity_twh.map(|e| (p.blanket_m, e)))
        .collect();
    if let (Some(lo), Some(hi)) = (
        energy.iter().map(|e| e.1).reduce(f64::min),
        energy.iter().map(|e| e.1).reduce(f64::max),
    ) {
        let pad = ((hi - lo) * 0.2).max(0.05);
        let (etick, estep) = nice_ticks(lo - pad, hi + pad, 4);
        let es = Scale {
            lo: (lo - pad).min(etick.first().copied().unwrap_or(lo)),
            hi: (hi + pad).max(etick.last().copied().unwrap_or(hi)),
            log: false,
            from_px: pb,
            to_px: pt,
        };
        let color = darken(StatusKind::Conditional.rgb(), 0.45);
        let d = decimals(estep);
        for t in &etick {
            let y = es.px(*t);
            f.svg.line(pr, y, pr + 2.8, y, AXIS, 0.8, None);
            f.svg.text(
                pr + 5.0,
                y + 2.3,
                &format!("{t:.d$}"),
                TextStyle::new(TICK - 0.4, color),
            );
        }
        let line: Vec<(f32, f32)> = energy
            .iter()
            .map(|(m, e)| (f.xs.px(*m), es.px(*e)))
            .collect();
        f.svg.polyline(&line, color, 1.1, None);
        for (m, e) in &energy {
            marker(&mut f.svg, f.xs.px(*m), es.px(*e), is_named(*m), color);
        }
        f.svg
            .text(pr + 6.0, pt - 3.0, "TWh", TextStyle::new(TICK - 0.4, color));
    }
    f.svg.line(pl, pb, pr, pb, AXIS, 0.9, None);
    f.svg.line(pl, pt, pl, pb, AXIS, 0.9, None);
    finish_sweep("sweep-swaps-electricity", f)
}

/// Width of a sci-notation tick, exposed for layout checks.
#[allow(dead_code)]
pub(crate) fn tick_width(v: f64) -> f32 {
    sci_width(v, TICK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use faris_engine::sweep::Estimate;

    fn point(blanket: f64, tbr: f64, flux: f64) -> TransportPoint {
        TransportPoint {
            variant_id: format!("b{blanket}"),
            label: String::new(),
            blanket_m: blanket,
            shield_m: 0.9 - blanket,
            breeding: Estimate {
                mean: tbr,
                standard_error: 0.0015,
            },
            magnet_flux: Estimate {
                mean: flux,
                standard_error: flux * 0.15,
            },
            seed: 1,
            histories: 1_000_000,
        }
    }

    fn sweep() -> Vec<TransportPoint> {
        (0..7)
            .map(|i| {
                let b = 0.35 + 0.05 * f64::from(i);
                point(
                    b,
                    1.25 + 0.02 * f64::from(i),
                    1.0e14 * (1.0 + 0.1 * f64::from(i)),
                )
            })
            .collect()
    }

    fn well_formed(svg: &str) {
        assert!(svg.starts_with("<svg "));
        assert!(svg.trim_end().ends_with("</svg>"));
        let opened = svg.matches("<text").count();
        assert_eq!(opened, svg.matches("</text>").count());
        assert!(!svg.contains("NaN") && !svg.contains("inf"));
    }

    #[test]
    fn timeline_has_the_limit_line_labelled_as_literature_and_every_series() {
        let series: Vec<TimelineSeries> = Arrangement::ORDER
            .iter()
            .map(|a| TimelineSeries {
                arrangement: *a,
                points: (0..100)
                    .map(|i| [f64::from(i) * 0.3, (f64::from(i) % 25.0) * 1.0e21])
                    .collect(),
                swap_spans_years: vec![(7.0, 7.3), (14.0, 14.3)],
                band: None,
            })
            .collect();
        let chart = timeline_chart(
            &series,
            Some(LimitLine {
                value: 3.0e22,
                literature: true,
            }),
            30.0,
            TIMELINE_SIZE.1,
        );
        well_formed(&chart.svg);
        assert!(chart.svg.contains("(literature)"));
        assert!(chart.svg.contains("REBCO limit"));
        assert!(chart.svg.contains("stroke-dasharray=\"5 3\""));
        for a in Arrangement::ORDER {
            assert!(chart.svg.contains(a.label()), "{}", a.label());
            assert!(chart.svg.contains(&crate::svg::hex(a.rgb())));
        }
        assert_eq!(chart.svg.matches("fill-opacity=\"0.200\"").count(), 8);
    }

    #[test]
    fn authored_limits_are_not_called_literature() {
        let chart = timeline_chart(
            &[TimelineSeries {
                arrangement: Arrangement::ORDER[0],
                points: vec![[0.0, 0.0], [5.0, 1.0e22]],
                swap_spans_years: vec![],
                band: None,
            }],
            Some(LimitLine {
                value: 1.0e25,
                literature: false,
            }),
            30.0,
            TIMELINE_SIZE.1,
        );
        assert!(chart.svg.contains("(authored limit)"));
        assert!(!chart.svg.contains("(literature)"));
    }

    #[test]
    fn empty_data_gives_an_explained_placeholder_not_an_empty_chart() {
        let chart = timeline_chart(&[], None, 30.0, TIMELINE_SIZE.1);
        well_formed(&chart.svg);
        assert!(chart.svg.contains("No calculated operating history"));
    }

    #[test]
    fn sweep_charts_draw_every_point_with_error_bars() {
        let points = sweep();
        let tbr = sweep_breeding_chart(&points);
        let flux = sweep_flux_chart(&points);
        well_formed(&tbr.svg);
        well_formed(&flux.svg);
        assert_eq!(tbr.svg.matches("<circle").count(), 7);
        assert_eq!(flux.svg.matches("<circle").count(), 7);
        // Three lines per error bar plus gridlines and axes.
        assert!(flux.svg.matches("<line").count() > 7 * 3);
    }

    #[test]
    fn history_chart_needs_every_summary() {
        let points = sweep();
        let missing = sweep_history_chart(&points, &[]);
        assert!(missing.svg.contains("not calculated"));
        let summaries: Vec<Option<HistorySummary>> = (0..7)
            .map(|i| {
                Some(HistorySummary {
                    magnets_replaceable: true,
                    magnet_replacements: 4 + (i % 3),
                    first_magnet_replacement_years: Some(6.0),
                    magnet_permanent_limit_years: None,
                    net_electricity_twh: Some(33.0 + 0.1 * f64::from(i)),
                    final_available_tritium_kg: 3.0,
                })
            })
            .collect();
        let chart = sweep_history_chart(&points, &summaries);
        well_formed(&chart.svg);
        assert_eq!(chart.svg.matches("<circle").count(), 7);
        assert!(chart.svg.contains("TWh"));
    }
}
