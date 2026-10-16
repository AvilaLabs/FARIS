//! How a value and its uncertainty are written: digits set by the uncertainty (UNC-081), the interval
//! convention stated beside every interval (UNC-082), and a notice naming what a partial interval
//! leaves out (UNC-086). Clients call these functions; none of them decides a number.

use crate::math;
use serde::Serialize;

/// What an interval means. The label is shown beside the interval, never left to a hover.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "convention", rename_all = "snake_case")]
pub enum IntervalConvention {
    /// The value plus or minus one standard error.
    OneSigma,
    /// The value plus or minus two standard errors.
    TwoSigma,
    /// A central confidence interval at this level (0.95 for a 95 % interval).
    Confidence { level: f64 },
    /// A range between two percentiles of a sampled distribution (5 and 95 for P5 to P95).
    Percentile { low: f64, high: f64 },
}

impl IntervalConvention {
    /// Words for the convention, as shown beside the interval.
    pub fn label(&self) -> String {
        match self {
            IntervalConvention::OneSigma => "1 σ standard error".into(),
            IntervalConvention::TwoSigma => "2 σ (two standard errors)".into(),
            IntervalConvention::Confidence { level } => {
                format!("{} % confidence interval", trim_number(100.0 * level))
            }
            IntervalConvention::Percentile { low, high } => {
                format!("P{} to P{} range", trim_number(*low), trim_number(*high))
            }
        }
    }

    /// Multiplier from one standard error to the half-width, for the conventions that have one.
    pub fn sigma_multiple(&self) -> Option<f64> {
        match self {
            IntervalConvention::OneSigma => Some(1.0),
            IntervalConvention::TwoSigma => Some(2.0),
            _ => None,
        }
    }
}

fn trim_number(x: f64) -> String {
    let s = format!("{x:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A table or panel may show intervals of one convention, or must label each column; this says which.
/// `Ok` carries the single convention label to put in the header; `Err` names the conventions so each
/// column can be labelled.
pub fn single_convention(conventions: &[IntervalConvention]) -> Result<String, String> {
    let Some(first) = conventions.first() else {
        return Err("no intervals to label".into());
    };
    if conventions.iter().all(|c| c == first) {
        Ok(first.label())
    } else {
        let mut labels: Vec<String> = conventions.iter().map(|c| c.label()).collect();
        labels.dedup();
        Err(format!(
            "intervals of different conventions ({}) must not share a column; label each column with its convention",
            labels.join(", ")
        ))
    }
}

/// A value written with the digits its uncertainty supports.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FormattedMeasurement {
    pub value: String,
    /// None when no usable uncertainty was given.
    pub uncertainty: Option<String>,
    /// Common power of ten, when the pair is written in scientific form.
    pub exponent: Option<i32>,
    /// The whole pair, e.g. `1.234 ± 0.012` or `(1.234 ± 0.012)e-5`.
    pub text: String,
}

/// Decimal exponent of a positive number as `{:e}` writes it.
fn exponent_of(x: f64) -> i32 {
    let s = format!("{x:e}");
    s.rsplit('e')
        .next()
        .and_then(|e| e.parse().ok())
        .unwrap_or(0)
}

/// Decimal place of the last kept digit of the uncertainty (0 is the units digit, -2 the hundredths):
/// two significant digits when the leading three digits are below 355, one otherwise (the rule of the
/// Particle Data Group, within the GUM limit of two).
fn last_digit_place(uncertainty: f64) -> i32 {
    let three = format!("{uncertainty:.2e}");
    let leading: u32 = three
        .split('e')
        .next()
        .map(|m| m.replace('.', ""))
        .and_then(|m| m.parse().ok())
        .unwrap_or(100);
    // The three-digit rounding may carry (9.996 gives 10.0): use the one-digit form's exponent then.
    if leading < 355 {
        let two = format!("{uncertainty:.1e}");
        exponent_of_text(&two) - 1
    } else {
        let one = format!("{uncertainty:.0e}");
        exponent_of_text(&one)
    }
}

fn exponent_of_text(s: &str) -> i32 {
    s.rsplit('e')
        .next()
        .and_then(|e| e.parse().ok())
        .unwrap_or(0)
}

/// Write `value ± uncertainty` so the last shown digit is the first or second digit of the uncertainty.
/// With no usable uncertainty (zero, negative or not finite) the value is shown to three significant
/// digits and the text says no uncertainty was available.
pub fn format_with_uncertainty(value: f64, uncertainty: f64) -> FormattedMeasurement {
    if !value.is_finite() {
        return FormattedMeasurement {
            value: "n/a".into(),
            uncertainty: None,
            exponent: None,
            text: "n/a (the value is not a finite number)".into(),
        };
    }
    let usable = uncertainty.is_finite() && uncertainty > 0.0;
    let larger = value.abs().max(if usable { uncertainty } else { 0.0 });
    let e = if larger > 0.0 { exponent_of(larger) } else { 0 };
    let place = if usable {
        last_digit_place(uncertainty)
    } else {
        e - 2
    };
    let scientific = !(-3..=5).contains(&e);
    // Rounding the value may carry into a new power of ten; the exponent of the pair stays that of the
    // larger of the two before rounding, which keeps the columns of one pair aligned.
    let (scale, decimals) = if scientific {
        (math::powi(10.0, e), (e - place).max(0) as usize)
    } else {
        (1.0, (-place).max(0) as usize)
    };
    let render = |x: f64| -> String {
        if place > 0 && !scientific {
            // Last digit is above the units: round to that power of ten.
            let unit = math::powi(10.0, place);
            format!("{:.0}", (x / unit).round() * unit)
        } else {
            format!("{:.*}", decimals, x / scale)
        }
    };
    let v = render(value);
    let (u, text) = if usable {
        let u = render(uncertainty);
        let pair = format!("{v} ± {u}");
        let text = if scientific {
            format!("({pair})e{e}")
        } else {
            pair
        };
        (Some(u), text)
    } else {
        let note = if scientific {
            format!("{v}e{e}")
        } else {
            v.clone()
        };
        (
            None,
            format!("{note} (no uncertainty available, shown to three significant digits)"),
        )
    };
    FormattedMeasurement {
        value: v,
        uncertainty: u,
        exponent: scientific.then_some(e),
        text,
    }
}

/// `; excludes: a, b` when parts are missing from the uncertainty, otherwise empty.
pub fn exclusion_notice(excluded: &[String]) -> String {
    if excluded.is_empty() {
        String::new()
    } else {
        format!("excludes: {}", excluded.join(", "))
    }
}

/// An interval as shown: the value with its half-width, the convention beside it, and the exclusion
/// notice when parts of the uncertainty are not evaluated. Returns None for conventions that have no
/// half-width from a standard error (use [`display_bounds`] for those).
pub fn display_with_standard_error(
    value: f64,
    standard_error: f64,
    convention: IntervalConvention,
    excluded: &[String],
) -> Option<String> {
    let multiple = convention.sigma_multiple()?;
    let pair = format_with_uncertainty(value, standard_error * multiple);
    Some(join_parts(&pair.text, &convention.label(), excluded))
}

fn join_parts(body: &str, label: &str, excluded: &[String]) -> String {
    let mut text = format!("{body} ({label})");
    let notice = exclusion_notice(excluded);
    if !notice.is_empty() {
        text.push_str("; ");
        text.push_str(&notice);
    }
    text
}

/// A value with explicit interval bounds. The digits follow the narrower side of the interval and the
/// bounds are rounded outward, so the displayed interval is never narrower than the computed one.
pub fn display_bounds(
    value: f64,
    lower: f64,
    upper: f64,
    convention: IntervalConvention,
    excluded: &[String],
) -> String {
    let narrow = (value - lower).min(upper - value);
    if !(lower.is_finite() && upper.is_finite() && value.is_finite()) || narrow <= 0.0 {
        return join_parts(
            &format!(
                "{value:.3e} (bounds not usable: lower, value and upper must be finite with the value inside)"
            ),
            &convention.label(),
            excluded,
        );
    }
    let place = last_digit_place(narrow);
    let unit = math::powi(10.0, place);
    let down = (lower / unit).floor() * unit;
    let up = (upper / unit).ceil() * unit;
    let decimals = (-place).max(0) as usize;
    let body = format!(
        "{:.*} (from {:.*} to {:.*})",
        decimals,
        (value / unit).round() * unit,
        decimals,
        down,
        decimals,
        up
    );
    join_parts(&body, &convention.label(), excluded)
}

#[cfg(test)]
mod tests;
