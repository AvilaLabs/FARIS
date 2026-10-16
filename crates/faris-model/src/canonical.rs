//! Canonical forms of JSON values, so that equal inputs compare and hash equally whatever order the
//! keys were written in, whatever the white space, and however a number was spelled (AUTO-034,
//! COL-025). The cache key (`faris_engine::result_cache`) and the semantic diff
//! (`faris_engine::semantic_diff`) both read values through this module, so they agree on what is
//! the same input.
//!
//! What is treated as the same: object key order; white space between tokens; `1`, `1.0`, `1e0`
//! and `1.00` (a number is compared by value); `-0` and `0`; CRLF and LF inside a string; a number
//! written as text with a decimal comma or point (`"1,5"` and `"1.5"`) when both sides are such
//! text. What is not: a different number, string or array order, and a number versus a string.

use serde_json::{Number, Value};

/// Largest whole number every `f64` holds exactly.
const EXACT_INTEGER: f64 = 9_007_199_254_740_992.0;

/// The canonical text of a number: a whole number prints without a point or exponent, any other
/// finite number prints in the shortest form that reads back to the same bits.
pub fn canonical_number(number: &Number) -> String {
    if let Some(i) = number.as_i64() {
        return i.to_string();
    }
    if let Some(u) = number.as_u64() {
        return u.to_string();
    }
    canonical_f64(number.as_f64().unwrap_or(f64::NAN))
}

/// [`canonical_number`] for a plain float.
pub fn canonical_f64(f: f64) -> String {
    if !f.is_finite() {
        // JSON has no spelling for these; keep them apart from every real number.
        return format!("\"{f}\"");
    }
    if f == 0.0 {
        return "0".into();
    }
    if f.fract() == 0.0 && f.abs() < EXACT_INTEGER {
        return format!("{}", f as i64);
    }
    // `{:?}` is the shortest text that reads back to the same bits.
    format!("{f:?}")
}

fn canonical_text(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// The canonical JSON text of `value`: keys sorted, no white space, numbers as above.
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, &mut out);
    out
}

fn write_value(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&canonical_number(n)),
        Value::String(s) => {
            out.push_str(&serde_json::to_string(&canonical_text(s)).unwrap_or_default());
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).unwrap_or_default());
                out.push(':');
                write_value(&map[key], out);
            }
            out.push('}');
        }
    }
}

/// A string that is only a number, read with a point or a comma as the decimal mark.
fn numeric_text(text: &str) -> Option<f64> {
    let trimmed = text.trim();
    let first = trimmed.chars().next()?;
    if !(first.is_ascii_digit() || matches!(first, '-' | '+' | '.' | ',')) {
        return None;
    }
    // One comma and no point is a decimal comma; any other grouping is left alone.
    let normalised = if trimmed.matches(',').count() == 1 && !trimmed.contains('.') {
        trimmed.replace(',', ".")
    } else {
        trimmed.to_string()
    };
    normalised.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// True when two leaves are the same input: same number by value, same text after line endings
/// are made alike, or the same number written as text with either decimal mark.
pub fn leaves_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => canonical_number(x) == canonical_number(y),
        (Value::String(x), Value::String(y)) => {
            canonical_text(x) == canonical_text(y)
                || matches!((numeric_text(x), numeric_text(y)), (Some(i), Some(j)) if i == j)
        }
        _ => a == b,
    }
}

/// True when two whole values are the same input under [`leaves_equal`], ignoring key order.
pub fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| values_equal(v, w)))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(v, w)| values_equal(v, w))
        }
        _ => leaves_equal(a, b),
    }
}

/// A value converted to the base unit of its dimension. The key uses this form, so 30 cm and
/// 0.3 m are the same input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quantity {
    pub value: f64,
    pub unit: &'static str,
}

/// Converts `value` in `unit` to the base unit of its dimension (m, s, eV, kg, K, MW). `None` for
/// a unit this table does not know; the caller must refuse rather than guess.
pub fn to_base_unit(value: f64, unit: &str) -> Option<Quantity> {
    let (v, base) = match unit {
        "m" => (value, "m"),
        "cm" => (value / 100.0, "m"),
        "mm" => (value / 1000.0, "m"),
        "km" => (value * 1000.0, "m"),
        "s" => (value, "s"),
        "min" => (value * 60.0, "s"),
        "h" => (value * crate::units::HOUR_S, "s"),
        "d" => (value * crate::units::DAY_S, "s"),
        "eV" => (value, "eV"),
        "keV" => (value * 1000.0, "eV"),
        "MeV" => (value * 1.0e6, "eV"),
        "kg" => (value, "kg"),
        "g" => (value / 1000.0, "kg"),
        "K" => (value, "K"),
        "MW" => (value, "MW"),
        "kW" => (value / 1000.0, "MW"),
        "GW" => (value * 1000.0, "MW"),
        _ => return None,
    };
    Some(Quantity {
        value: v,
        unit: base,
    })
}

impl Quantity {
    /// The value as JSON, for use as a key component.
    pub fn to_json(self) -> Value {
        serde_json::json!({ "unit": self.unit, "value": self.value })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Verifies: AUTO-034, COL-025
    #[test]
    fn spelling_order_and_white_space_do_not_change_the_canonical_text() {
        let a: Value =
            serde_json::from_str(r#"{"b": 1.0, "a": [1e0, 2.50, -0], "c": "x"}"#).unwrap();
        let b: Value = serde_json::from_str("{\"c\":\"x\",\n\"a\":[1,2.5,0],\"b\":1}").unwrap();
        assert_eq!(canonical_json(&a), canonical_json(&b));
        assert!(values_equal(&a, &b));
        assert_eq!(canonical_json(&a), r#"{"a":[1,2.5,0],"b":1,"c":"x"}"#);
    }

    #[test]
    fn a_different_value_is_a_different_text() {
        assert!(!values_equal(&json!({"a": 1}), &json!({"a": "1"})));
        assert!(!values_equal(&json!([1, 2]), &json!([2, 1])));
        assert!(!leaves_equal(&json!(1.5), &json!(1.500_000_000_000_000_2)));
    }

    #[test]
    fn decimal_comma_and_point_text_are_the_same_number() {
        assert!(leaves_equal(&json!("1,5"), &json!("1.50")));
        assert!(!leaves_equal(&json!("1,5"), &json!("1.6")));
        assert!(!leaves_equal(&json!("abc"), &json!("abd")));
        assert!(leaves_equal(&json!("a\r\nb"), &json!("a\nb")));
    }

    // Verifies: AUTO-034
    #[test]
    fn units_convert_to_one_base_value() {
        let a = to_base_unit(30.0, "cm").unwrap();
        let b = to_base_unit(0.3, "m").unwrap();
        assert_eq!(canonical_json(&a.to_json()), canonical_json(&b.to_json()));
        assert!(to_base_unit(1.0, "furlong").is_none());
    }
}
