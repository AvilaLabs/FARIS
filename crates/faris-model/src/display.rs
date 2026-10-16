//! How numbers, dates and quantities are shown and typed, as the user chose (CFG-037, CFG-040,
//! UX-066, L10N-022, L10N-023).
//!
//! Everything here turns a stored value into text or text into a stored value; nothing here
//! changes a stored value or a file. Stored files stay in SI base units, a decimal point and ISO
//! dates whatever the display settings say, so a file reads the same on every machine and under
//! every locale. The CLI and the desktop both call [`DisplayPrefs`], built from the resolved
//! settings with [`DisplayPrefs::from_resolved`].

use crate::settings::Resolved;
use crate::units::{
    self, Dimension, GROUPS, GroupDef, UnitDef, UnitError, accepted_symbols, convert,
};
use std::collections::BTreeMap;

/// The unit system: where each quantity group starts before a per-quantity override.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitSystem {
    Si,
    Fusion,
}

impl UnitSystem {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "si" => Some(Self::Si),
            "fusion" => Some(Self::Fusion),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Si => "si",
            Self::Fusion => "fusion",
        }
    }

    /// The unit this system shows for `group`.
    pub fn default_unit(self, group: &GroupDef) -> &'static UnitDef {
        let symbol = match self {
            Self::Si => group.si,
            Self::Fusion => group.fusion,
        };
        units::find_in(group.id, symbol).expect("registry test: every default is in its group")
    }
}

/// The decimal separator of displayed numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decimal {
    Point,
    Comma,
}

/// The digit-group separator of displayed numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grouping {
    None,
    /// U+202F, the SI Brochure's thin space.
    ThinSpace,
    Comma,
    Point,
    Apostrophe,
}

impl Grouping {
    fn character(self) -> Option<char> {
        match self {
            Self::None => None,
            Self::ThinSpace => Some('\u{202f}'),
            Self::Comma => Some(','),
            Self::Point => Some('.'),
            Self::Apostrophe => Some('\''),
        }
    }
}

/// How numbers are written on screen and in printed reports. The default prints exactly what
/// Rust's `{:.N}` prints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NumberFormat {
    pub decimal: Decimal,
    pub grouping: Grouping,
}

impl Default for NumberFormat {
    fn default() -> Self {
        Self {
            decimal: Decimal::Point,
            grouping: Grouping::None,
        }
    }
}

impl NumberFormat {
    fn decimal_char(self) -> char {
        match self.decimal {
            Decimal::Point => '.',
            Decimal::Comma => ',',
        }
    }

    /// Refuses a pair where the group separator is the decimal separator.
    pub fn check(self) -> Result<Self, String> {
        if self.grouping.character() == Some(self.decimal_char()) {
            return Err(format!(
                "format.decimal and format.grouping both use '{}', so a number could not be read \
                 back. Next step: choose a different grouping, for example thin-space or none.",
                self.decimal_char()
            ));
        }
        Ok(self)
    }

    /// `{:.decimals}` with this format's separators. Digits are grouped in threes from five
    /// integer digits up (4 digits stay together, as the SI Brochure writes them).
    pub fn fixed(self, value: f64, decimals: usize) -> String {
        self.localize(&format!("{value:.decimals$}"))
    }

    /// The value with `digits` significant digits, written with [`NumberFormat::fixed`] (no
    /// exponent), so a number in a freshly chosen unit keeps its precision whatever its size.
    pub fn significant(self, value: f64, digits: usize) -> String {
        let magnitude = if value == 0.0 || !value.is_finite() {
            0
        } else {
            crate::math::log10(value.abs()).floor() as i64
        };
        let decimals = (digits as i64 - 1 - magnitude).clamp(0, 15) as usize;
        self.fixed(value, decimals)
    }

    /// `{:.decimals e}` with this format's decimal separator: `1.2500e3`.
    pub fn scientific(self, value: f64, decimals: usize) -> String {
        let plain = format!("{value:.decimals$e}");
        match plain.split_once('e') {
            Some((mantissa, exponent)) => {
                format!(
                    "{}e{exponent}",
                    mantissa.replace('.', &self.decimal_char().to_string())
                )
            }
            None => plain,
        }
    }

    /// Rewrites the separators of a number written with a point and no grouping.
    fn localize(self, plain: &str) -> String {
        let (sign, digits) = match plain.strip_prefix('-') {
            Some(rest) => ("-", rest),
            None => ("", plain),
        };
        let (whole, fraction) = match digits.split_once('.') {
            Some((w, f)) => (w, Some(f)),
            None => (digits, None),
        };
        let mut out = String::from(sign);
        match self.grouping.character() {
            Some(separator) if whole.len() >= 5 && whole.bytes().all(|b| b.is_ascii_digit()) => {
                for (i, digit) in whole.chars().enumerate() {
                    if i > 0 && (whole.len() - i).is_multiple_of(3) {
                        out.push(separator);
                    }
                    out.push(digit);
                }
            }
            _ => out.push_str(whole),
        }
        if let Some(fraction) = fraction {
            out.push(self.decimal_char());
            out.push_str(fraction);
        }
        out
    }

    /// Reads a number typed in this format. A point is also accepted as the decimal separator
    /// when the grouping is not a point, so a pasted `12.5` works under a comma setting.
    pub fn parse(self, text: &str) -> Result<f64, UnitError> {
        let bad = || UnitError::BadNumber {
            text: text.trim().into(),
        };
        let mut plain = String::new();
        let decimal = self.decimal_char();
        let group = self.grouping.character();
        for c in text.trim().chars() {
            if c == decimal {
                plain.push('.');
            } else if Some(c) == group || c == '\u{a0}' || c == ' ' {
                continue;
            } else if c == '.' && group != Some('.') && decimal != '.' {
                plain.push('.');
            } else {
                plain.push(c);
            }
        }
        let value: f64 = plain.parse().map_err(|_| bad())?;
        if value.is_finite() {
            Ok(value)
        } else {
            Err(bad())
        }
    }
}

/// How dates are written on screen. Stored dates are always ISO 8601 (`2026-10-15`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateFormat {
    Iso,
    DayMonthYear,
    MonthDayYear,
}

impl DateFormat {
    pub fn render(self, year: i64, month: u32, day: u32) -> String {
        match self {
            Self::Iso => format!("{year:04}-{month:02}-{day:02}"),
            Self::DayMonthYear => format!("{day:02}.{month:02}.{year:04}"),
            Self::MonthDayYear => format!("{month:02}/{day:02}/{year:04}"),
        }
    }

    /// The date of a Unix time in UTC.
    pub fn render_unix(self, unix_s: i64) -> String {
        let (y, m, d) = civil_date(unix_s);
        self.render(y, m, d)
    }
}

/// `(year, month, day)` in UTC of a Unix time (the civil-from-days algorithm of H. Hinnant).
pub fn civil_date(unix_s: i64) -> (i64, u32, u32) {
    let z = unix_s.div_euclid(units::DAY_S as i64) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// The user's display choices: unit system, per-quantity overrides, number and date formats.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayPrefs {
    pub system: UnitSystem,
    overrides: BTreeMap<&'static str, &'static UnitDef>,
    pub number: NumberFormat,
    pub date: DateFormat,
}

impl Default for DisplayPrefs {
    fn default() -> Self {
        Self {
            system: UnitSystem::Si,
            overrides: BTreeMap::new(),
            number: NumberFormat::default(),
            date: DateFormat::Iso,
        }
    }
}

impl DisplayPrefs {
    /// Builds the choices from resolved settings (`units.*` and `format.*`). Every problem is
    /// returned, not only the first.
    pub fn from_resolved(all: &[Resolved]) -> Result<Self, Vec<String>> {
        let get = |key: &str| {
            all.iter()
                .find(|r| r.key == key)
                .and_then(|r| r.value.as_deref())
        };
        let mut problems = Vec::new();
        let mut prefs = Self::default();
        if let Some(text) = get("units.system") {
            match UnitSystem::parse(text) {
                Some(system) => prefs.system = system,
                None => problems.push(format!("units.system: '{text}' is not si or fusion.")),
            }
        }
        for group in GROUPS {
            let key = format!("units.{}", group.id);
            match get(&key) {
                None | Some("system") => {}
                Some(text) => match units::find_in(group.id, text) {
                    Some(unit) => {
                        prefs.overrides.insert(group.id, unit);
                    }
                    None => problems.push(format!(
                        "{key}: '{text}' is not a unit of {}. Next step: use one of {}.",
                        group.label,
                        units::units_of(group.id)
                            .map(|u| u.symbol)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                },
            }
        }
        if let Some(text) = get("format.decimal") {
            match text {
                "point" => prefs.number.decimal = Decimal::Point,
                "comma" => prefs.number.decimal = Decimal::Comma,
                other => problems.push(format!("format.decimal: '{other}' is not point or comma.")),
            }
        }
        if let Some(text) = get("format.grouping") {
            match text {
                "none" => prefs.number.grouping = Grouping::None,
                "thin-space" => prefs.number.grouping = Grouping::ThinSpace,
                "comma" => prefs.number.grouping = Grouping::Comma,
                "point" => prefs.number.grouping = Grouping::Point,
                "apostrophe" => prefs.number.grouping = Grouping::Apostrophe,
                other => problems.push(format!(
                    "format.grouping: '{other}' is not a known grouping."
                )),
            }
        }
        if let Err(m) = prefs.number.check() {
            problems.push(m);
        }
        if let Some(text) = get("format.date") {
            match text {
                "iso" => prefs.date = DateFormat::Iso,
                "dmy" => prefs.date = DateFormat::DayMonthYear,
                "mdy" => prefs.date = DateFormat::MonthDayYear,
                other => problems.push(format!("format.date: '{other}' is not iso, dmy or mdy.")),
            }
        }
        if problems.is_empty() {
            Ok(prefs)
        } else {
            Err(problems)
        }
    }

    /// The unit shown for a group: the per-quantity override, else the system's.
    pub fn unit_for(&self, group_id: &str) -> &'static UnitDef {
        if let Some(unit) = self.overrides.get(group_id) {
            return unit;
        }
        let group = units::group(group_id).expect("a group of the registry");
        self.system.default_unit(group)
    }

    /// True when the user chose something for this group: the fusion system or an override.
    pub fn is_chosen(&self, group_id: &str) -> bool {
        self.system != UnitSystem::Si || self.overrides.contains_key(group_id)
    }

    /// The chosen unit, or `habit` (a symbol of the group) when the user chose nothing. For a
    /// printer whose output has always used a friendly unit, such as days for an outage, so the
    /// default does not change what it prints (the SI default still holds for stored data).
    pub fn unit_or(&self, group_id: &str, habit: &str) -> &'static UnitDef {
        if self.is_chosen(group_id) {
            self.unit_for(group_id)
        } else {
            units::find_in(group_id, habit).unwrap_or_else(|| self.unit_for(group_id))
        }
    }

    /// The value in the unit shown for the group, from SI base units.
    pub fn value_in(&self, group_id: &str, si: f64) -> f64 {
        self.unit_for(group_id).from_si(si)
    }

    /// `12.50 keV`: the number in the chosen unit and its symbol, always together (L10N-022).
    pub fn show(&self, group_id: &str, si: f64, decimals: usize) -> String {
        self.show_in(self.unit_for(group_id), si, decimals)
    }

    pub fn show_in(&self, unit: &UnitDef, si: f64, decimals: usize) -> String {
        format!(
            "{} {}",
            self.number.fixed(unit.from_si(si), decimals),
            unit.symbol
        )
    }

    /// Reads text a user typed for a quantity of `group_id` and returns SI base units. A bare
    /// number is taken in the chosen unit; a number with a unit may use any unit of the same
    /// dimension, and any other is refused with the accepted ones (L10N-021).
    pub fn parse(&self, group_id: &str, text: &str) -> Result<f64, UnitError> {
        let group = units::group(group_id).expect("a group of the registry");
        let shown = self.unit_for(group_id);
        let text = text.trim();
        // The number is the longest prefix of number characters; the rest is the unit.
        let split = text
            .char_indices()
            .find(|(i, c)| {
                !(c.is_ascii_digit()
                    || matches!(
                        c,
                        '.' | ',' | '+' | '-' | '\u{202f}' | '\'' | ' ' | '\u{a0}'
                    )
                    || (matches!(c, 'e' | 'E')
                        && text[*i + 1..]
                            .starts_with(|n: char| n.is_ascii_digit() || n == '-' || n == '+')))
            })
            .map_or(text.len(), |(i, _)| i);
        let (number, unit_text) = text.split_at(split);
        let value = self.number.parse(number)?;
        let unit_text = unit_text.trim();
        if unit_text.is_empty() {
            return Ok(shown.to_si(value));
        }
        let candidates = units::find_all(unit_text);
        match candidates.iter().find(|u| u.dimension() == group.dimension) {
            Some(unit) => Ok(unit.to_si(value)),
            None => match candidates.first() {
                Some(other) => Err(convert(value, other, shown).expect_err("dimensions differ")),
                None => Err(UnitError::Unknown {
                    text: unit_text.into(),
                    accepted: accepted_symbols(group.dimension),
                }),
            },
        }
    }
}

/// The dimension of a group, for a caller that checks an entry before it parses it.
pub fn dimension_of(group_id: &str) -> Option<Dimension> {
    units::group(group_id).map(|g| g.dimension)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Layers, UserSettings};
    use std::{ffi::OsString, path::Path};

    fn prefs_with(flags: &[(&str, &str)]) -> Result<DisplayPrefs, Vec<String>> {
        let none = |_: &str| None::<OsString>;
        let flags: Vec<(String, String)> = flags
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let file = UserSettings::default();
        let all = Layers {
            file: &file,
            file_path: Path::new("/u/settings.json"),
            env: &none,
            flags: &flags,
        }
        .resolve_all()?;
        DisplayPrefs::from_resolved(&all)
    }

    // Verifies: UX-066, CFG-040
    #[test]
    fn si_is_the_default_and_the_fusion_set_is_one_choice() {
        let si = prefs_with(&[]).unwrap();
        assert_eq!(si.system, UnitSystem::Si);
        assert_eq!(si.unit_for("power").symbol, "W");
        assert_eq!(si.unit_for("length").symbol, "m");
        assert_eq!(si.unit_for("energy_particle").symbol, "J");
        let fusion = prefs_with(&[("units.system", "fusion")]).unwrap();
        for (group, symbol) in [
            ("power", "MW"),
            ("energy_particle", "keV"),
            ("length", "cm"),
            ("energy_plant", "GWd"),
            ("damage", "dpa"),
        ] {
            assert_eq!(fusion.unit_for(group).symbol, symbol, "{group}");
        }
        // 1.5 GW is 1500 MW in the fusion set and 1.5e9 W in SI, from the same stored number.
        assert_eq!(si.show("power", 1.5e9, 0), "1500000000 W");
        assert_eq!(fusion.show("power", 1.5e9, 0), "1500 MW");
    }

    // Verifies: CFG-040, UX-066
    #[test]
    fn a_per_quantity_override_beats_the_system() {
        let p =
            prefs_with(&[("units.system", "fusion"), ("units.energy_particle", "MeV")]).unwrap();
        assert_eq!(p.unit_for("energy_particle").symbol, "MeV");
        assert_eq!(p.unit_for("power").symbol, "MW");
        assert_eq!(
            p.show("energy_particle", 14.1e6 * 1.602176634e-19, 1),
            "14.1 MeV"
        );
        // An alias is accepted and stored under the symbol.
        let q = prefs_with(&[("units.particle_flux", "n/cm\u{b2}/s")]).unwrap();
        assert_eq!(q.unit_for("particle_flux").symbol, "n/cm2/s");
        // A unit of another quantity is refused with the allowed ones.
        let errors = prefs_with(&[("units.length", "kg")]).unwrap_err();
        assert!(
            errors[0].contains("units.length") && errors[0].contains("cm"),
            "{errors:?}"
        );
    }

    #[test]
    fn a_habit_unit_is_used_only_while_nothing_is_chosen() {
        let si = prefs_with(&[]).unwrap();
        assert_eq!(si.unit_or("time", "d").symbol, "d");
        let chosen = prefs_with(&[("units.time", "h")]).unwrap();
        assert_eq!(chosen.unit_or("time", "d").symbol, "h");
        let fusion = prefs_with(&[("units.system", "fusion")]).unwrap();
        assert_eq!(fusion.unit_or("energy_plant", "MWh").symbol, "GWd");
    }

    // Verifies: L10N-021, L10N-022
    #[test]
    fn typed_quantities_take_any_unit_of_the_right_dimension_and_refuse_others() {
        let p = prefs_with(&[("units.system", "fusion")]).unwrap();
        // A bare number is in the shown unit (cm).
        assert_eq!(p.parse("length", "30").unwrap(), 0.30);
        assert_eq!(p.parse("length", "30 cm").unwrap(), 0.30);
        assert_eq!(p.parse("length", "0.5 m").unwrap(), 0.5);
        assert!((p.parse("energy_particle", "14.1 MeV").unwrap() - 2.2589e-12).abs() < 1e-15);
        assert!(
            (p.parse("energy_particle", "2e3 eV").unwrap() / 3.204353268e-16 - 1.0).abs() < 1e-12
        );
        assert!((p.parse("particle_flux", "1e14 n/cm\u{b2}/s").unwrap() - 1e18).abs() < 1.0);
        // Wrong dimension: named, with what is accepted.
        let wrong = p.parse("length", "5 MeV").unwrap_err().to_string();
        assert!(
            wrong.contains("MeV") && wrong.contains("different things"),
            "{wrong}"
        );
        assert!(wrong.contains("cm") && wrong.contains("km"), "{wrong}");
        // Sv is not Gy.
        assert!(p.parse("absorbed_dose", "1 Sv").is_err());
        // Unknown unit and bad number.
        let unknown = p.parse("length", "5 furlong").unwrap_err().to_string();
        assert!(
            unknown.contains("furlong") && unknown.contains("Next step"),
            "{unknown}"
        );
        assert!(matches!(
            p.parse("length", "abc cm"),
            Err(UnitError::BadNumber { .. })
        ));
        assert!(p.parse("length", "").is_err());
        assert!(p.parse("length", "nan").is_err());
    }

    // Verifies: L10N-021
    #[test]
    fn fuzzed_wrong_dimension_entries_are_all_refused() {
        let p = DisplayPrefs::default();
        let mut state = 0x1234_5678_9ABC_DEF0_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut refused = 0;
        for _ in 0..2000 {
            let group = &GROUPS[(next() % GROUPS.len() as u64) as usize];
            let other = &units::UNITS[(next() % units::UNITS.len() as u64) as usize];
            // The bare unit "1" cannot be told from a digit of the number.
            if other.symbol == "1" {
                continue;
            }
            let text = format!("{} {}", (next() % 1000) as f64 / 7.0, other.symbol);
            let result = p.parse(group.id, &text);
            if other.dimension() == group.dimension {
                assert!(result.is_ok(), "{} / {text}: {result:?}", group.id);
            } else {
                let message = result.unwrap_err().to_string();
                assert!(message.contains("different things") && message.contains("Next step"));
                refused += 1;
            }
        }
        assert!(refused > 1000, "{refused}");
    }

    // Verifies: CFG-037
    #[test]
    fn numbers_follow_the_format_and_read_back() {
        let de = NumberFormat {
            decimal: Decimal::Comma,
            grouping: Grouping::Point,
        };
        let fr = NumberFormat {
            decimal: Decimal::Comma,
            grouping: Grouping::ThinSpace,
        };
        let us = NumberFormat {
            decimal: Decimal::Point,
            grouping: Grouping::Comma,
        };
        let plain = NumberFormat::default();
        assert_eq!(plain.fixed(1234567.891, 2), "1234567.89");
        assert_eq!(de.fixed(1234567.891, 2), "1.234.567,89");
        assert_eq!(fr.fixed(1234567.891, 2), "1\u{202f}234\u{202f}567,89");
        assert_eq!(us.fixed(-1234567.891, 1), "-1,234,567.9");
        assert_eq!(de.fixed(1234.5, 1), "1234,5");
        assert_eq!(de.fixed(12.0, 0), "12");
        assert_eq!(de.scientific(1250.0, 3), "1,250e3");
        assert_eq!(plain.scientific(1250.0, 3), "1.250e3");
        for format in [plain, de, fr, us] {
            for value in [0.0, 12.5, -3.25, 1234567.891, 99999.5, 0.000125] {
                let text = format.fixed(value, 6);
                let back = format.parse(&text).unwrap();
                assert!((back - value).abs() < 1e-6, "{text}: {back} vs {value}");
            }
        }
        // A pasted point still reads under a comma setting without point grouping.
        assert_eq!(fr.parse("12.5").unwrap(), 12.5);
        // A format that cannot be read back is refused.
        let clash = NumberFormat {
            decimal: Decimal::Comma,
            grouping: Grouping::Comma,
        };
        assert!(clash.check().unwrap_err().contains("Next step"));
        assert!(prefs_with(&[("format.decimal", "comma"), ("format.grouping", "comma")]).is_err());
    }

    #[test]
    fn significant_digits_keep_precision_whatever_the_size() {
        let n = NumberFormat::default();
        assert_eq!(n.significant(1234.5678, 4), "1235");
        assert_eq!(n.significant(12.345678, 4), "12.35");
        assert_eq!(n.significant(0.00123456, 4), "0.001235");
        assert_eq!(n.significant(0.0, 4), "0.000");
        assert_eq!(n.significant(f64::NAN, 4), "NaN");
    }

    // Verifies: CFG-037
    #[test]
    fn dates_follow_the_format() {
        // 2026-10-15 00:00:00 UTC.
        let t = 1_792_022_400;
        assert_eq!(civil_date(t), (2026, 10, 15));
        assert_eq!(DateFormat::Iso.render_unix(t), "2026-10-15");
        assert_eq!(DateFormat::DayMonthYear.render_unix(t), "15.10.2026");
        assert_eq!(DateFormat::MonthDayYear.render_unix(t), "10/15/2026");
        assert_eq!(civil_date(0), (1970, 1, 1));
        assert_eq!(civil_date(951_782_400), (2000, 2, 29));
        let p = prefs_with(&[("format.date", "dmy")]).unwrap();
        assert_eq!(p.date, DateFormat::DayMonthYear);
    }
}
