use super::*;

fn fm(v: f64, u: f64) -> String {
    format_with_uncertainty(v, u).text
}

#[test]
fn digits_follow_the_uncertainty() {
    assert_eq!(fm(1.23456, 0.0123), "1.235 ± 0.012");
    assert_eq!(fm(1.23456, 0.0456), "1.23 ± 0.05");
    assert_eq!(fm(12.3456, 0.5), "12.3 ± 0.5");
    assert_eq!(fm(1234.5, 300.0), "1230 ± 300");
    assert_eq!(fm(0.5, 0.0099), "0.50 ± 0.01");
    assert_eq!(fm(3.0e-7, 1.2e-8), "(3.00 ± 0.12)e-7");
    assert_eq!(fm(2.5e9, 3.0e7), "(2.500 ± 0.030)e9");
}

#[test]
fn unusable_uncertainty_shows_the_value_to_three_digits_and_says_so() {
    for u in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let m = format_with_uncertainty(1.23456, u);
        assert!(m.uncertainty.is_none());
        assert_eq!(
            m.text,
            "1.23 (no uncertainty available, shown to three significant digits)"
        );
    }
    assert!(format_with_uncertainty(f64::NAN, 1.0).text.contains("n/a"));
}

/// Significant digits of a decimal string as written: leading zeros do not count, trailing zeros before
/// the decimal point do not count either (they are place holders).
fn significant_digits(s: &str) -> usize {
    let digits: String = s.chars().filter(char::is_ascii_digit).collect();
    let trimmed = digits.trim_start_matches('0');
    if s.contains('.') {
        trimmed.len()
    } else {
        trimmed.trim_end_matches('0').len().max(1)
    }
}

// Verifies: UNC-081
#[test]
fn last_shown_digit_is_first_or_second_digit_of_the_uncertainty_on_1000_pairs() {
    let mut state = 0x1234_5678_9abc_def1_u64;
    let mut next = || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    for _ in 0..1000 {
        let exp = (next() % 12) as i32 - 6;
        let u = (1.0 + (next() % 9000) as f64 / 1000.0) * math::powi(10.0, exp);
        let v = ((next() % 2_000_000) as f64 / 1000.0 - 1000.0) * math::powi(10.0, exp + 1);
        let m = format_with_uncertainty(v, u);
        let shown = m.uncertainty.clone().expect("usable uncertainty");
        let digits = significant_digits(&shown);
        assert!((1..=2).contains(&digits), "{v} ± {u} shown as {}", m.text);
        // Value and uncertainty end at the same decimal place.
        let places = |s: &str| s.split('.').nth(1).map_or(0, str::len);
        assert_eq!(places(&m.value), places(&shown), "{}", m.text);
        // The uncertainty agrees with the input to within half a unit of its last digit.
        let scale = m.exponent.map_or(1.0, |e| math::powi(10.0, e));
        let back: f64 = shown.parse::<f64>().unwrap() * scale;
        let half_unit = if shown.contains('.') {
            0.5 * math::powi(10.0, -(places(&shown) as i32)) * scale
        } else {
            0.5 * math::powi(10.0, shown.len() as i32 - significant_digits(&shown) as i32) * scale
        };
        assert!(
            (back - u).abs() <= half_unit * 1.000_001,
            "{} (input {u})",
            m.text
        );
    }
}

#[test]
fn convention_is_named_beside_every_interval() {
    let none: Vec<String> = vec![];
    let text = display_with_standard_error(1.0, 0.05, IntervalConvention::OneSigma, &none).unwrap();
    assert_eq!(text, "1.00 ± 0.05 (1 σ standard error)");
    let text = display_with_standard_error(1.0, 0.05, IntervalConvention::TwoSigma, &none).unwrap();
    assert_eq!(text, "1.00 ± 0.10 (2 σ (two standard errors))");
    assert!(
        display_with_standard_error(
            1.0,
            0.05,
            IntervalConvention::Confidence { level: 0.95 },
            &none
        )
        .is_none()
    );
    assert_eq!(
        IntervalConvention::Confidence { level: 0.95 }.label(),
        "95 % confidence interval"
    );
    assert_eq!(
        IntervalConvention::Percentile {
            low: 5.0,
            high: 95.0
        }
        .label(),
        "P5 to P95 range"
    );
}

// Verifies: UNC-082
#[test]
fn conventions_are_never_mixed_without_a_column_label() {
    let one = IntervalConvention::OneSigma;
    assert_eq!(
        single_convention(&[one, one]).unwrap(),
        "1 σ standard error"
    );
    let err = single_convention(&[one, IntervalConvention::TwoSigma]).unwrap_err();
    assert!(err.contains("label each column"), "{err}");
    assert!(single_convention(&[]).is_err());
    // Every display function puts the label in the text itself.
    for c in [IntervalConvention::OneSigma, IntervalConvention::TwoSigma] {
        let text = display_with_standard_error(2.0, 0.1, c, &[]).unwrap();
        assert!(text.contains(&c.label()), "{text}");
    }
    let text = display_bounds(
        10.0,
        8.0,
        13.0,
        IntervalConvention::Confidence { level: 0.95 },
        &[],
    );
    assert!(text.contains("95 % confidence interval"), "{text}");
}

// Verifies: UNC-086
#[test]
fn a_partial_interval_lists_what_it_excludes() {
    let excluded = vec!["nuclear data".to_string(), "geometry".to_string()];
    let text =
        display_with_standard_error(1.0, 0.05, IntervalConvention::OneSigma, &excluded).unwrap();
    assert!(
        text.ends_with("; excludes: nuclear data, geometry"),
        "{text}"
    );
    let text = display_bounds(
        10.0,
        8.0,
        13.0,
        IntervalConvention::Confidence { level: 0.95 },
        &excluded,
    );
    assert!(text.contains("excludes: nuclear data, geometry"), "{text}");
    assert_eq!(exclusion_notice(&[]), "");
}

#[test]
fn bounds_round_outward_and_never_narrow_the_interval() {
    for (lo, v, hi) in [
        (8.04, 10.0, 13.96),
        (0.9871, 1.0, 1.0129),
        (1234.0, 1500.0, 1799.0),
    ] {
        let text = display_bounds(
            v,
            lo,
            hi,
            IntervalConvention::Confidence { level: 0.95 },
            &[],
        );
        let inner = text.split("(from ").nth(1).unwrap();
        let mut parts = inner.split(" to ");
        let shown_lo: f64 = parts.next().unwrap().parse().unwrap();
        let shown_hi: f64 = parts
            .next()
            .unwrap()
            .split(')')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(shown_lo <= lo && shown_hi >= hi, "{text}");
    }
    assert!(
        display_bounds(1.0, 1.0, 2.0, IntervalConvention::OneSigma, &[]).contains("not usable")
    );
}
