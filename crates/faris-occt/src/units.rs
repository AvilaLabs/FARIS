//! The declared length unit read from the STEP text, independently of OpenCASCADE. A port of
//! `text_length_units` in `integrations/cad/faris_cad.py`, so the two readers can be compared.

use regex::bytes::Regex;
use std::collections::HashMap;

/// Metres per unit for the units FARIS accepts, by label, in the helper's order.
pub const UNIT_FACTORS: [(&str, f64); 4] =
    [("mm", 1.0e-3), ("cm", 1.0e-2), ("m", 1.0), ("inch", 0.0254)];
/// Metres per shape unit once OpenCASCADE has converted a declared file to millimetres.
pub const INTERNAL_FACTOR: f64 = 1.0e-3;

const SI_PREFIX: [(&[u8], f64); 7] = [
    (b"$", 1.0),
    (b".MILLI.", 1.0e-3),
    (b".CENTI.", 1.0e-2),
    (b".MICRO.", 1.0e-6),
    (b".KILO.", 1.0e3),
    (b".DECI.", 1.0e-1),
    (b".NANO.", 1.0e-9),
];

/// OpenCASCADE's names for the length units (`STEPControl_Reader::FileUnits`), lower case.
const OCCT_NAMES: [(&str, f64); 10] = [
    ("millimetre", 1.0e-3),
    ("centimetre", 1.0e-2),
    ("metre", 1.0),
    ("kilometre", 1.0e3),
    ("micrometre", 1.0e-6),
    ("inch", 0.0254),
    ("foot", 0.3048),
    ("mile", 1609.344),
    ("mil", 2.54e-5),
    ("microinch", 2.54e-8),
];

/// Python's `math.isclose(a, b, rel_tol=1e-12)`.
pub fn is_close(a: f64, b: f64) -> bool {
    if a == b {
        return true;
    }
    (a - b).abs() <= 1.0e-12 * a.abs().max(b.abs())
}

/// The label of an exact known factor.
pub fn unit_label(factor: f64) -> Option<&'static str> {
    UNIT_FACTORS
        .iter()
        .find(|(_, value)| is_close(factor, *value))
        .map(|(label, _)| *label)
}

/// Python's `f"{x:g}"`.
pub fn format_g(x: f64) -> String {
    if x == 0.0 {
        return "0".into();
    }
    let scientific = format!("{x:.5e}");
    // Rounding can carry into the next power of ten; read the exponent after rounding.
    let rounded_exponent = scientific
        .split('e')
        .nth(1)
        .and_then(|e| e.parse::<i32>().ok())
        .unwrap_or(0);
    if !(-4..6).contains(&rounded_exponent) {
        let (mantissa, _) = scientific.split_once('e').expect("exponent form");
        let mantissa = if mantissa.contains('.') {
            mantissa.trim_end_matches('0').trim_end_matches('.')
        } else {
            mantissa
        };
        let sign = if rounded_exponent < 0 { '-' } else { '+' };
        return format!("{mantissa}e{sign}{:02}", rounded_exponent.abs());
    }
    let decimals = (5 - rounded_exponent).max(0) as usize;
    let fixed = format!("{x:.decimals$}");
    if fixed.contains('.') {
        fixed
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    } else {
        fixed
    }
}

pub fn describe_factor(factor: f64) -> String {
    unit_label(factor)
        .map(str::to_string)
        .unwrap_or_else(|| format!("{} m", format_g(factor)))
}

/// Metres per unit for an OpenCASCADE unit name, if FARIS knows it.
pub fn occt_name_factor(name: &str) -> Option<f64> {
    let lower = name.to_lowercase();
    OCCT_NAMES
        .iter()
        .find(|(n, _)| *n == lower)
        .map(|(_, f)| *f)
}

struct Text<'a> {
    bytes: &'a [u8],
    entity: HashMap<String, Option<Regex>>,
    statements: HashMap<(String, Option<usize>), Option<Vec<u8>>>,
    si: Regex,
    conversion: Regex,
    measure: Regex,
}

impl<'a> Text<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            entity: HashMap::new(),
            statements: HashMap::new(),
            si: Regex::new(r"(?-u)SI_UNIT\s*\(\s*(\$|\.[A-Z]+\.)\s*,\s*\.METRE\.\s*\)").expect("pattern"),
            conversion: Regex::new(r"(?-u)CONVERSION_BASED_UNIT\s*\(\s*'([^']*)'\s*,\s*#([0-9]+)\s*\)").expect("pattern"),
            measure: Regex::new(
                r"(?-u)LENGTH_MEASURE_WITH_UNIT\s*\(\s*(?:LENGTH_MEASURE\s*\(\s*)?([-+0-9.eEdD]+)\s*\)?\s*,\s*#([0-9]+)",
            )
            .expect("pattern"),
        }
    }

    /// Body of entity `#number`: searched in a window around `near` first, since a whole-file
    /// search of a large file is slow.
    fn entity_text(&mut self, number: &str, near: Option<usize>) -> Option<Vec<u8>> {
        let key = (number.to_string(), near);
        if let Some(found) = self.statements.get(&key) {
            return found.clone();
        }
        let pattern = self
            .entity
            .entry(number.to_string())
            .or_insert_with(|| Regex::new(&format!(r"(?-u)(?m)^#{number}\s*=\s*([^;]*);")).ok())
            .clone()?;
        let mut found = None;
        if let Some(near) = near {
            let low = near.saturating_sub(20000);
            let high = (near + 20000).min(self.bytes.len());
            let window = &self.bytes[low..high];
            // A match at the window's first byte only counts at a real line start.
            found = pattern
                .captures_iter(window)
                .find(|c| {
                    let start = c.get(0).map(|m| m.start()).unwrap_or(0);
                    start != 0 || low == 0 || self.bytes[low - 1] == b'\n'
                })
                .map(|c| c[1].to_vec());
        }
        if found.is_none() {
            found = pattern.captures(self.bytes).map(|c| c[1].to_vec());
        }
        self.statements.insert(key, found.clone());
        found
    }

    fn statement_factor(
        &mut self,
        statement: &[u8],
        near: Option<usize>,
        depth: u32,
    ) -> Result<f64, String> {
        if let Some(c) = self.si.captures(statement) {
            let prefix = &c[1];
            return SI_PREFIX
                .iter()
                .find(|(p, _)| *p == prefix)
                .map(|(_, f)| *f)
                .ok_or_else(|| format!("unknown SI prefix {}", String::from_utf8_lossy(prefix)));
        }
        if depth < 3
            && let Some(c) = self.conversion.captures(statement)
        {
            let name = String::from_utf8_lossy(&c[1]).into_owned();
            let number = String::from_utf8_lossy(&c[2]).into_owned();
            let Some(measure) = self.entity_text(&number, near) else {
                return Err(format!(
                    "conversion unit '{name}' references a missing entity"
                ));
            };
            let Some(value) = self.measure.captures(&measure) else {
                return Err(format!(
                    "conversion unit '{name}' has no readable length measure"
                ));
            };
            let number_text = String::from_utf8_lossy(&value[1]).replace(['D', 'd'], "E");
            let base_number = String::from_utf8_lossy(&value[2]).into_owned();
            let Some(base) = self.entity_text(&base_number, near) else {
                return Err(format!(
                    "conversion unit '{name}' references a missing base unit"
                ));
            };
            let base_factor = self.statement_factor(&base, near, depth + 1)?;
            let scale: f64 = number_text
                .parse()
                .map_err(|_| format!("conversion unit '{name}' has no readable length measure"))?;
            return Ok(scale * base_factor);
        }
        Err("length unit is neither SI_UNIT nor CONVERSION_BASED_UNIT".into())
    }
}

/// Distinct length-unit factors (metres) the file's contexts declare, ascending, and problems.
///
/// Only units named by a `GLOBAL_UNIT_ASSIGNED_CONTEXT` count; the base unit a conversion-based
/// unit points at is not a declared unit of the file.
pub fn text_length_units(bytes: &[u8]) -> (Vec<f64>, Vec<String>) {
    let context =
        Regex::new(r"(?-u)GLOBAL_UNIT_ASSIGNED_CONTEXT\s*\(\s*\(([^)]*)\)\s*\)").expect("pattern");
    let reference = Regex::new(r"(?-u)#([0-9]+)").expect("pattern");
    let length_unit = Regex::new(r"(?-u)LENGTH_UNIT\s*\(\s*\)").expect("pattern");
    let mut text = Text::new(bytes);
    let mut factors: Vec<f64> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    let mut cache: HashMap<String, Option<Vec<u8>>> = HashMap::new();
    for found in context.captures_iter(bytes) {
        let start = found.get(0).map(|m| m.start()).unwrap_or(0);
        for number in reference.captures_iter(&found[1]) {
            let number = String::from_utf8_lossy(&number[1]).into_owned();
            let statement = cache
                .entry(number.clone())
                .or_insert_with(|| text.entity_text(&number, Some(start)))
                .clone();
            let Some(statement) = statement else { continue };
            if !length_unit.is_match(&statement) {
                continue;
            }
            match text.statement_factor(&statement, Some(start), 0) {
                Err(problem) => problems.push(problem),
                Ok(factor) => {
                    if !factors.iter().any(|seen| is_close(factor, *seen)) {
                        factors.push(factor);
                    }
                }
            }
        }
    }
    factors.sort_by(|a, b| a.total_cmp(b));
    problems.sort();
    problems.dedup();
    (factors, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g_format_matches_python() {
        assert_eq!(format_g(0.0254), "0.0254");
        assert_eq!(format_g(1.0), "1");
        assert_eq!(format_g(2.54e-5), "2.54e-05");
        assert_eq!(format_g(1609.344), "1609.34");
        assert_eq!(format_g(1.0e6), "1e+06");
        assert_eq!(format_g(1.0e-4), "0.0001");
    }

    #[test]
    fn reads_si_and_conversion_units() {
        let step = b"#1 = (LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.));\n\
            #2 = (GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#1,#3,#4)) REPRESENTATION_CONTEXT('a','b'));\n";
        let (factors, problems) = text_length_units(step);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(factors, vec![1.0e-3]);
        let inch = b"#10 = (CONVERSION_BASED_UNIT('INCH',#11) LENGTH_UNIT() NAMED_UNIT(#12));\n\
            #11 = LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(25.4),#13);\n\
            #13 = (LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.));\n\
            #20 = (GLOBAL_UNIT_ASSIGNED_CONTEXT((#10)));\n";
        let (factors, problems) = text_length_units(inch);
        assert!(problems.is_empty(), "{problems:?}");
        assert!(is_close(factors[0], 0.0254));
        assert_eq!(describe_factor(factors[0]), "inch");
    }
}
