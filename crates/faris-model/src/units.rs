//! The unit registry: every non-SI domain unit FARIS shows or accepts, defined once with an exact
//! conversion factor and its source (requirements CFG-044, CFG-042, CFG-040, L10N-021, L10N-022).
//!
//! Stored values are always in SI base units (kg, m, s, K, ...) and never change with a display
//! choice; this module only converts. A unit is a symbol, a [`Dimension`] (an exponent vector) and
//! a factor to SI written as an exact decimal fraction ([`Exact`]), so `1 eV = 1.602176634e-19 J`
//! is held as the integers it is defined by, not as a rounded float. Units that share an SI
//! dimension but not a meaning are told apart by two extra axes (see [`Dimension`]).
//!
//! Sources: "SI 2019" is the BIPM SI Brochure, 9th edition (2019), whose revised SI fixes the
//! elementary charge at exactly 1.602176634e-19 C; "SP 811" is NIST Special Publication 811
//! (2008 edition), Appendix B, for the units outside the SI (curie, rem, rad, barn, bar, atm,
//! gauss); "FARIS definition" marks a convention this project fixes and states in
//! `docs/guide/units.md` (dpa, appm, the full-power year, burn-up in MWd/kg).

use std::fmt;

/// Exponents of the seven SI base quantities plus two tag axes: length, mass, time, electric
/// current, temperature, amount of substance, luminous intensity, displacements per atom
/// (`dpa`), and radiation weighting (`Sv` against `Gy`). The tag axes make `dpa` refuse to
/// convert to a plain fraction and `Sv` refuse to convert to `Gy`: the numbers would convert, the
/// physics would not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Dimension(pub [i8; 9]);

const AXES: [&str; 9] = ["m", "kg", "s", "A", "K", "mol", "cd", "dpa", "weighted"];

impl Dimension {
    pub const NONE: Self = Self([0; 9]);

    /// Text such as `m^2 kg s^-2`; `1` for a dimensionless quantity.
    pub fn text(&self) -> String {
        let parts: Vec<String> = self
            .0
            .iter()
            .zip(AXES)
            .filter(|(e, _)| **e != 0)
            .map(|(e, name)| {
                if *e == 1 {
                    name.to_string()
                } else {
                    format!("{name}^{e}")
                }
            })
            .collect();
        if parts.is_empty() {
            "1".into()
        } else {
            parts.join(" ")
        }
    }
}

const fn dim(m: i8, kg: i8, s: i8, k: i8, dpa: i8, weighted: i8) -> Dimension {
    Dimension([m, kg, s, 0, k, 0, 0, dpa, weighted])
}

/// An exact decimal fraction `num / den * 10^exp10`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exact {
    pub num: i128,
    pub den: i128,
    pub exp10: i32,
}

const fn x(num: i128, den: i128, exp10: i32) -> Exact {
    Exact { num, den, exp10 }
}

impl Exact {
    const ZERO: Self = x(0, 1, 0);

    /// The nearest f64. The decimal part is parsed (correctly rounded by the standard library) and
    /// a denominator, when there is one, costs one more rounding.
    pub fn to_f64(self) -> f64 {
        let top: f64 = format!("{}e{}", self.num, self.exp10)
            .parse()
            .unwrap_or(f64::NAN);
        if self.den == 1 {
            top
        } else {
            top / self.den as f64
        }
    }

    /// `self / other` as one exact fraction.
    fn over(self, other: Self) -> Self {
        let (mut num, mut den) = (self.num * other.den, self.den * other.num);
        if den < 0 {
            (num, den) = (-num, -den);
        }
        let g = gcd(num.unsigned_abs(), den.unsigned_abs()).max(1) as i128;
        x(num / g, den / g, self.exp10 - other.exp10)
    }
}

fn gcd(a: u128, b: u128) -> u128 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// The length of a day in seconds (SI Brochure 2019, Table 8: 1 d = 86 400 s).
pub const DAY_S: f64 = 86_400.0;
/// The Julian year of 365.25 days, in seconds. FARIS uses it for the year and for the
/// full-power year (see the `FPY` unit).
pub const JULIAN_YEAR_S: f64 = 365.25 * DAY_S;
/// Seconds in an hour.
pub const HOUR_S: f64 = 3_600.0;
/// Becquerel per curie (NIST SP 811 Appendix B: 1 Ci = 3.7e10 Bq exactly).
pub const CURIE_BQ: f64 = 3.7e10;

/// One unit of the registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnitDef {
    /// The ASCII name used in settings, files and the command line.
    pub symbol: &'static str,
    /// Other spellings accepted on entry.
    pub aliases: &'static [&'static str],
    pub name: &'static str,
    /// The quantity group (a key of [`GROUPS`]).
    pub group: &'static str,
    /// `1 unit = factor * SI` (then plus `offset` for an offset unit).
    pub factor: Exact,
    /// Added after scaling: only the degree Celsius has one.
    pub offset: Exact,
    pub source: &'static str,
}

impl UnitDef {
    pub fn dimension(&self) -> Dimension {
        group(self.group).map_or(Dimension::NONE, |g| g.dimension)
    }

    pub fn is_offset(&self) -> bool {
        self.offset.num != 0
    }

    /// The value in SI base units.
    pub fn to_si(&self, value: f64) -> f64 {
        value * self.factor.to_f64() + self.offset.to_f64()
    }

    /// The value in this unit, from SI base units.
    pub fn from_si(&self, si: f64) -> f64 {
        (si - self.offset.to_f64()) / self.factor.to_f64()
    }
}

/// A set of units for one kind of quantity, with each system's default.
#[derive(Clone, Copy, Debug)]
pub struct GroupDef {
    pub id: &'static str,
    pub label: &'static str,
    pub dimension: Dimension,
    /// Default in the SI system.
    pub si: &'static str,
    /// Default in the fusion practical system.
    pub fusion: &'static str,
}

const fn g(
    id: &'static str,
    label: &'static str,
    dimension: Dimension,
    si: &'static str,
    fusion: &'static str,
) -> GroupDef {
    GroupDef {
        id,
        label,
        dimension,
        si,
        fusion,
    }
}

/// Every quantity group, in the order the handbook lists them. Each has a `units.<id>` setting.
pub static GROUPS: &[GroupDef] = &[
    g("length", "Length", dim(1, 0, 0, 0, 0, 0), "m", "cm"),
    g(
        "area",
        "Area and cross-section",
        dim(2, 0, 0, 0, 0, 0),
        "m2",
        "cm2",
    ),
    g("mass", "Mass", dim(0, 1, 0, 0, 0, 0), "kg", "kg"),
    g(
        "density",
        "Mass density",
        dim(-3, 1, 0, 0, 0, 0),
        "kg/m3",
        "g/cm3",
    ),
    g("time", "Duration", dim(0, 0, 1, 0, 0, 0), "s", "d"),
    g(
        "service_time",
        "Time in service",
        dim(0, 0, 1, 0, 0, 0),
        "s",
        "yr",
    ),
    g(
        "temperature",
        "Temperature",
        dim(0, 0, 0, 1, 0, 0),
        "K",
        "K",
    ),
    g("power", "Power", dim(2, 1, -3, 0, 0, 0), "W", "MW"),
    g(
        "energy_particle",
        "Particle energy",
        dim(2, 1, -2, 0, 0, 0),
        "J",
        "keV",
    ),
    g(
        "energy_plant",
        "Plant energy",
        dim(2, 1, -2, 0, 0, 0),
        "J",
        "GWd",
    ),
    g(
        "power_density",
        "Power density",
        dim(-1, 1, -3, 0, 0, 0),
        "W/m3",
        "W/cm3",
    ),
    g(
        "heat_flux",
        "Surface power",
        dim(0, 1, -3, 0, 0, 0),
        "W/m2",
        "MW/m2",
    ),
    g(
        "particle_flux",
        "Particle flux",
        dim(-2, 0, -1, 0, 0, 0),
        "n/m2/s",
        "n/cm2/s",
    ),
    g(
        "fluence",
        "Fluence",
        dim(-2, 0, 0, 0, 0, 0),
        "n/m2",
        "n/cm2",
    ),
    g(
        "damage",
        "Displacement damage",
        dim(0, 0, 0, 0, 1, 0),
        "dpa",
        "dpa",
    ),
    g(
        "damage_rate",
        "Damage rate",
        dim(0, 0, -1, 0, 1, 0),
        "dpa/s",
        "dpa/FPY",
    ),
    g(
        "dose",
        "Dose equivalent",
        dim(2, 0, -2, 0, 0, 1),
        "Sv",
        "Sv",
    ),
    g(
        "dose_rate",
        "Dose-equivalent rate",
        dim(2, 0, -3, 0, 0, 1),
        "Sv/s",
        "Sv/h",
    ),
    g(
        "absorbed_dose",
        "Absorbed dose",
        dim(2, 0, -2, 0, 0, 0),
        "Gy",
        "Gy",
    ),
    g(
        "absorbed_dose_rate",
        "Absorbed-dose rate",
        dim(2, 0, -3, 0, 0, 0),
        "Gy/s",
        "Gy/h",
    ),
    g("activity", "Activity", dim(0, 0, -1, 0, 0, 0), "Bq", "Bq"),
    g("fraction", "Fraction", Dimension::NONE, "1", "appm"),
    g(
        "burnup",
        "Burn-up",
        dim(2, 0, -2, 0, 0, 0),
        "J/kg",
        "MWd/kg",
    ),
    g(
        "magnetic_field",
        "Magnetic flux density",
        dim(0, 1, -2, 0, 0, 0),
        "T",
        "T",
    ),
    g("pressure", "Pressure", dim(-1, 1, -2, 0, 0, 0), "Pa", "Pa"),
];

pub fn group(id: &str) -> Option<&'static GroupDef> {
    GROUPS.iter().find(|g| g.id == id)
}

const SI19: &str = "SI Brochure 9th ed. (2019)";
const SI19_E: &str = "SI 2019: e = 1.602176634e-19 C exactly, so 1 eV = 1.602176634e-19 J";
const SP811: &str = "NIST SP 811 (2008), Appendix B";
const FARIS: &str = "FARIS definition (docs/guide/units.md)";
const DAYS: &str = "SI 2019 Table 8: 1 d = 86 400 s";
const JULIAN: &str = "FARIS definition: Julian year of 365.25 d = 31 557 600 s";

const fn u(
    symbol: &'static str,
    name: &'static str,
    group: &'static str,
    factor: Exact,
    source: &'static str,
) -> UnitDef {
    UnitDef {
        symbol,
        aliases: &[],
        name,
        group,
        factor,
        offset: Exact::ZERO,
        source,
    }
}

const fn alias(mut unit: UnitDef, aliases: &'static [&'static str]) -> UnitDef {
    unit.aliases = aliases;
    unit
}

/// Every unit. A symbol appears once; a symbol is case sensitive (`mSv` is not `MSv`).
pub static UNITS: &[UnitDef] = &[
    u("m", "metre", "length", x(1, 1, 0), SI19),
    u("cm", "centimetre", "length", x(1, 1, -2), SI19),
    u("mm", "millimetre", "length", x(1, 1, -3), SI19),
    u("km", "kilometre", "length", x(1, 1, 3), SI19),
    alias(u("m2", "square metre", "area", x(1, 1, 0), SI19), &["m^2"]),
    alias(
        u("cm2", "square centimetre", "area", x(1, 1, -4), SI19),
        &["cm^2"],
    ),
    alias(
        u(
            "barn",
            "barn",
            "area",
            x(1, 1, -28),
            "SP 811 Appendix B: 1 b = 1e-28 m2 exactly",
        ),
        &["b"],
    ),
    u("kg", "kilogram", "mass", x(1, 1, 0), SI19),
    u("g", "gram", "mass", x(1, 1, -3), SI19),
    u(
        "t",
        "tonne",
        "mass",
        x(1, 1, 3),
        "SI 2019 Table 8: 1 t = 1000 kg",
    ),
    alias(
        u(
            "kg/m3",
            "kilogram per cubic metre",
            "density",
            x(1, 1, 0),
            SI19,
        ),
        &["kg/m^3"],
    ),
    alias(
        u(
            "g/cm3",
            "gram per cubic centimetre",
            "density",
            x(1, 1, 3),
            SI19,
        ),
        &["g/cm^3"],
    ),
    alias(u("s", "second", "time", x(1, 1, 0), SI19), &["sec"]),
    u(
        "min",
        "minute",
        "time",
        x(60, 1, 0),
        "SI 2019 Table 8: 1 min = 60 s",
    ),
    alias(
        u(
            "h",
            "hour",
            "time",
            x(3600, 1, 0),
            "SI 2019 Table 8: 1 h = 3600 s",
        ),
        &["hr"],
    ),
    alias(u("d", "day", "time", x(86_400, 1, 0), DAYS), &["day"]),
    alias(u("s", "second", "service_time", x(1, 1, 0), SI19), &["sec"]),
    alias(
        u("d", "day", "service_time", x(86_400, 1, 0), DAYS),
        &["day"],
    ),
    alias(
        u(
            "yr",
            "year (Julian)",
            "service_time",
            x(31_557_600, 1, 0),
            JULIAN,
        ),
        &["a", "y", "year"],
    ),
    alias(
        u(
            "FPY",
            "full-power year",
            "service_time",
            x(31_557_600, 1, 0),
            "FARIS definition: 1 FPY is 31 557 600 s of full-power operation; the capacity factor is applied by the operating history, not here",
        ),
        &["fpy"],
    ),
    u("K", "kelvin", "temperature", x(1, 1, 0), SI19),
    alias(
        UnitDef {
            offset: x(27_315, 1, -2),
            ..u(
                "degC",
                "degree Celsius",
                "temperature",
                x(1, 1, 0),
                "SI 2019: 0 degC = 273.15 K",
            )
        },
        &["\u{b0}C"],
    ),
    u("W", "watt", "power", x(1, 1, 0), SI19),
    u("kW", "kilowatt", "power", x(1, 1, 3), SI19),
    u("MW", "megawatt", "power", x(1, 1, 6), SI19),
    u("GW", "gigawatt", "power", x(1, 1, 9), SI19),
    u("J", "joule", "energy_particle", x(1, 1, 0), SI19),
    u(
        "eV",
        "electronvolt",
        "energy_particle",
        x(1_602_176_634, 1, -28),
        SI19_E,
    ),
    u(
        "keV",
        "kiloelectronvolt",
        "energy_particle",
        x(1_602_176_634, 1, -25),
        SI19_E,
    ),
    u(
        "MeV",
        "megaelectronvolt",
        "energy_particle",
        x(1_602_176_634, 1, -22),
        SI19_E,
    ),
    u("J", "joule", "energy_plant", x(1, 1, 0), SI19),
    u(
        "kWh",
        "kilowatt-hour",
        "energy_plant",
        x(36, 1, 5),
        "1 kWh = 1000 W * 3600 s = 3.6e6 J (SI 2019)",
    ),
    u(
        "MWh",
        "megawatt-hour",
        "energy_plant",
        x(36, 1, 8),
        "1 MWh = 3.6e9 J (SI 2019)",
    ),
    u(
        "GWh",
        "gigawatt-hour",
        "energy_plant",
        x(36, 1, 11),
        "1 GWh = 3.6e12 J (SI 2019)",
    ),
    u(
        "MWd",
        "megawatt-day",
        "energy_plant",
        x(864, 1, 8),
        "1 MWd = 1e6 W * 86 400 s = 8.64e10 J (SI 2019)",
    ),
    u(
        "GWd",
        "gigawatt-day",
        "energy_plant",
        x(864, 1, 11),
        "1 GWd = 1e9 W * 86 400 s = 8.64e13 J (SI 2019)",
    ),
    alias(
        u(
            "W/m3",
            "watt per cubic metre",
            "power_density",
            x(1, 1, 0),
            SI19,
        ),
        &["W/m^3"],
    ),
    alias(
        u(
            "W/cm3",
            "watt per cubic centimetre",
            "power_density",
            x(1, 1, 6),
            SI19,
        ),
        &["W/cm^3"],
    ),
    alias(
        u(
            "MW/m3",
            "megawatt per cubic metre",
            "power_density",
            x(1, 1, 6),
            SI19,
        ),
        &["MW/m^3"],
    ),
    alias(
        u(
            "W/m2",
            "watt per square metre",
            "heat_flux",
            x(1, 1, 0),
            SI19,
        ),
        &["W/m^2"],
    ),
    alias(
        u(
            "kW/m2",
            "kilowatt per square metre",
            "heat_flux",
            x(1, 1, 3),
            SI19,
        ),
        &["kW/m^2"],
    ),
    alias(
        u(
            "MW/m2",
            "megawatt per square metre",
            "heat_flux",
            x(1, 1, 6),
            SI19,
        ),
        &["MW/m^2"],
    ),
    alias(
        u(
            "W/cm2",
            "watt per square centimetre",
            "heat_flux",
            x(1, 1, 4),
            SI19,
        ),
        &["W/cm^2"],
    ),
    alias(
        u(
            "n/m2/s",
            "neutrons per square metre per second",
            "particle_flux",
            x(1, 1, 0),
            "count per area per time; the neutron is a count, not a dimension",
        ),
        &["n/m^2/s"],
    ),
    alias(
        u(
            "n/cm2/s",
            "neutrons per square centimetre per second",
            "particle_flux",
            x(1, 1, 4),
            "1 cm-2 = 1e4 m-2 exactly",
        ),
        &["n/cm^2/s", "n cm-2 s-1"],
    ),
    alias(
        u(
            "n/m2",
            "neutrons per square metre",
            "fluence",
            x(1, 1, 0),
            "count per area",
        ),
        &["n/m^2"],
    ),
    alias(
        u(
            "n/cm2",
            "neutrons per square centimetre",
            "fluence",
            x(1, 1, 4),
            "1 cm-2 = 1e4 m-2 exactly",
        ),
        &["n/cm^2"],
    ),
    u(
        "dpa",
        "displacements per atom",
        "damage",
        x(1, 1, 0),
        "FARIS definition: dpa counts displacements per lattice atom; its own axis, so it never converts to a plain fraction",
    ),
    u("dpa/s", "dpa per second", "damage_rate", x(1, 1, 0), FARIS),
    alias(
        u(
            "dpa/yr",
            "dpa per Julian year",
            "damage_rate",
            x(1, 31_557_600, 0),
            JULIAN,
        ),
        &["dpa/y", "dpa/a"],
    ),
    u(
        "dpa/FPY",
        "dpa per full-power year",
        "damage_rate",
        x(1, 31_557_600, 0),
        "FARIS definition: per 31 557 600 s of full-power operation",
    ),
    u("Sv", "sievert", "dose", x(1, 1, 0), SI19),
    u("mSv", "millisievert", "dose", x(1, 1, -3), SI19),
    u(
        "rem",
        "rem",
        "dose",
        x(1, 1, -2),
        "SP 811 Appendix B: 1 rem = 0.01 Sv exactly",
    ),
    u("Sv/s", "sievert per second", "dose_rate", x(1, 1, 0), SI19),
    u(
        "Sv/h",
        "sievert per hour",
        "dose_rate",
        x(1, 3600, 0),
        "SI 2019 Table 8: 1 h = 3600 s",
    ),
    u(
        "mSv/h",
        "millisievert per hour",
        "dose_rate",
        x(1, 3600, -3),
        "SI 2019 Table 8: 1 h = 3600 s",
    ),
    alias(
        u(
            "uSv/h",
            "microsievert per hour",
            "dose_rate",
            x(1, 3600, -6),
            "SI 2019 Table 8: 1 h = 3600 s",
        ),
        &["\u{b5}Sv/h"],
    ),
    u("rem/h", "rem per hour", "dose_rate", x(1, 3600, -2), SP811),
    u("Gy", "gray", "absorbed_dose", x(1, 1, 0), SI19),
    u("mGy", "milligray", "absorbed_dose", x(1, 1, -3), SI19),
    u(
        "rad",
        "rad",
        "absorbed_dose",
        x(1, 1, -2),
        "SP 811 Appendix B: 1 rad = 0.01 Gy exactly",
    ),
    u(
        "Gy/s",
        "gray per second",
        "absorbed_dose_rate",
        x(1, 1, 0),
        SI19,
    ),
    u(
        "Gy/h",
        "gray per hour",
        "absorbed_dose_rate",
        x(1, 3600, 0),
        "SI 2019 Table 8: 1 h = 3600 s",
    ),
    u(
        "mGy/h",
        "milligray per hour",
        "absorbed_dose_rate",
        x(1, 3600, -3),
        "SI 2019 Table 8: 1 h = 3600 s",
    ),
    u("Bq", "becquerel", "activity", x(1, 1, 0), SI19),
    u("kBq", "kilobecquerel", "activity", x(1, 1, 3), SI19),
    u("MBq", "megabecquerel", "activity", x(1, 1, 6), SI19),
    u("GBq", "gigabecquerel", "activity", x(1, 1, 9), SI19),
    u("TBq", "terabecquerel", "activity", x(1, 1, 12), SI19),
    u(
        "mCi",
        "millicurie",
        "activity",
        x(37, 1, 6),
        "SP 811 Appendix B: 1 Ci = 3.7e10 Bq exactly",
    ),
    u(
        "Ci",
        "curie",
        "activity",
        x(37, 1, 9),
        "SP 811 Appendix B: 1 Ci = 3.7e10 Bq exactly",
    ),
    u("1", "fraction", "fraction", x(1, 1, 0), "a pure number"),
    u("%", "percent", "fraction", x(1, 1, -2), "1 % = 0.01"),
    u(
        "appm",
        "atomic parts per million",
        "fraction",
        x(1, 1, -6),
        "FARIS definition: atoms of the species per 1e6 atoms of the material",
    ),
    u("J/kg", "joule per kilogram", "burnup", x(1, 1, 0), SI19),
    u(
        "MWd/kg",
        "megawatt-day per kilogram",
        "burnup",
        x(864, 1, 8),
        "1 MWd/kg = 8.64e10 J/kg (SI 2019); thermal energy per mass of heavy metal",
    ),
    alias(
        u(
            "GWd/t",
            "gigawatt-day per tonne",
            "burnup",
            x(864, 1, 8),
            "1 GWd/t = 1e9 W * 86 400 s / 1000 kg = 8.64e10 J/kg (SI 2019); equal to 1 MWd/kg",
        ),
        &["GWd/tHM"],
    ),
    u("T", "tesla", "magnetic_field", x(1, 1, 0), SI19),
    u("mT", "millitesla", "magnetic_field", x(1, 1, -3), SI19),
    u(
        "G",
        "gauss",
        "magnetic_field",
        x(1, 1, -4),
        "SP 811 Appendix B: 1 G = 1e-4 T exactly",
    ),
    u("Pa", "pascal", "pressure", x(1, 1, 0), SI19),
    u("kPa", "kilopascal", "pressure", x(1, 1, 3), SI19),
    u("MPa", "megapascal", "pressure", x(1, 1, 6), SI19),
    u(
        "bar",
        "bar",
        "pressure",
        x(1, 1, 5),
        "SP 811 Appendix B: 1 bar = 1e5 Pa exactly",
    ),
    u(
        "atm",
        "standard atmosphere",
        "pressure",
        x(101_325, 1, 0),
        "SP 811 Appendix B: 1 atm = 101 325 Pa exactly",
    ),
];

/// The units of one group, in registry order.
pub fn units_of(group_id: &str) -> impl Iterator<Item = &'static UnitDef> + use<'_> {
    UNITS.iter().filter(move |u| u.group == group_id)
}

/// The text `text` as a unit symbol: superscripts and `^` are dropped (`cm²` is `cm2`) and the
/// micro sign is `u`. Case is kept.
fn tidy(text: &str) -> String {
    text.trim()
        .chars()
        .filter(|c| *c != '^')
        .map(|c| match c {
            '\u{b2}' => '2',
            '\u{b3}' => '3',
            '\u{b5}' | '\u{3bc}' => 'u',
            other => other,
        })
        .collect()
}

/// A unit of `group_id` by symbol or alias.
pub fn find_in(group_id: &str, text: &str) -> Option<&'static UnitDef> {
    let want = tidy(text);
    units_of(group_id).find(|u| u.symbol == want || u.aliases.iter().any(|a| tidy(a) == want))
}

/// Every unit with this symbol or alias, across groups (a symbol such as `d` is in two).
pub fn find_all(text: &str) -> Vec<&'static UnitDef> {
    let want = tidy(text);
    UNITS
        .iter()
        .filter(|u| u.symbol == want || u.aliases.iter().any(|a| tidy(a) == want))
        .collect()
}

/// A failed conversion or entry, with the cause and what is accepted (L10N-021).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnitError {
    /// The units measure different things.
    Mismatch {
        from: String,
        to: String,
        from_dimension: String,
        to_dimension: String,
        accepted: Vec<&'static str>,
    },
    /// The unit text is in no group the quantity can take.
    Unknown {
        text: String,
        accepted: Vec<&'static str>,
    },
    /// The number could not be read.
    BadNumber { text: String },
}

impl fmt::Display for UnitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mismatch {
                from,
                to,
                from_dimension,
                to_dimension,
                accepted,
            } => write!(
                f,
                "{from} ({from_dimension}) cannot be used where {to} ({to_dimension}) is \
                 expected: they measure different things. Next step: enter the value in one of \
                 {}.",
                accepted.join(", ")
            ),
            Self::Unknown { text, accepted } => write!(
                f,
                "'{text}' is not a unit FARIS knows for this quantity. Next step: use one of {}.",
                accepted.join(", ")
            ),
            Self::BadNumber { text } => write!(
                f,
                "'{text}' is not a number. Next step: type digits with one decimal separator, \
                 for example 12.5, then the unit."
            ),
        }
    }
}

impl std::error::Error for UnitError {}

/// The symbols of every unit with this dimension, without repeats.
pub fn accepted_symbols(dimension: Dimension) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for unit in UNITS.iter().filter(|u| u.dimension() == dimension) {
        if !out.contains(&unit.symbol) {
            out.push(unit.symbol);
        }
    }
    out
}

/// Converts `value` between two units. Different dimensions are refused with the accepted units.
/// Without an offset the result is `value` times one exact factor, so a round trip is within a
/// few units in the last place (CFG-042).
pub fn convert(value: f64, from: &UnitDef, to: &UnitDef) -> Result<f64, UnitError> {
    let (a, b) = (from.dimension(), to.dimension());
    if a != b {
        return Err(UnitError::Mismatch {
            from: from.symbol.into(),
            to: to.symbol.into(),
            from_dimension: a.text(),
            to_dimension: b.text(),
            accepted: accepted_symbols(b),
        });
    }
    if from.is_offset() || to.is_offset() {
        return Ok(to.from_si(from.to_si(value)));
    }
    Ok(value * from.factor.over(to.factor).to_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed pseudo-random sequence in [0, 1): xorshift64*.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    fn unit(group: &str, symbol: &str) -> &'static UnitDef {
        find_in(group, symbol).unwrap()
    }

    #[test]
    fn registry_is_well_formed() {
        for group in GROUPS {
            assert!(units_of(group.id).count() >= 1, "{}", group.id);
            for default in [group.si, group.fusion] {
                assert!(
                    find_in(group.id, default).is_some(),
                    "{}: {default}",
                    group.id
                );
            }
            let mut symbols = std::collections::BTreeSet::new();
            for u in units_of(group.id) {
                assert!(
                    symbols.insert(u.symbol),
                    "{} repeats {}",
                    group.id,
                    u.symbol
                );
            }
        }
        for u in UNITS {
            assert!(group(u.group).is_some(), "{}", u.symbol);
            assert!(u.symbol.is_ascii() && !u.symbol.contains(char::is_whitespace));
            assert!(!u.source.is_empty() && !u.name.is_empty(), "{}", u.symbol);
            assert!(u.factor.num > 0 && u.factor.den > 0, "{}", u.symbol);
            assert_eq!(u.is_offset(), u.symbol == "degC");
        }
        // The same symbol in two groups is the same unit.
        for a in UNITS {
            for b in UNITS.iter().filter(|b| b.symbol == a.symbol) {
                assert_eq!(a.factor, b.factor, "{}", a.symbol);
                assert_eq!(a.dimension(), b.dimension(), "{}", a.symbol);
            }
        }
    }

    // Verifies: CFG-044
    #[test]
    fn domain_units_have_the_exact_defined_factors() {
        let f = |g: &str, s: &str| unit(g, s).factor.to_f64();
        // 1 eV is exactly e * 1 V; the decimal literal is the correctly rounded f64.
        assert_eq!(f("energy_particle", "eV"), 1.602176634e-19);
        assert_eq!(f("energy_particle", "keV"), 1.602176634e-16);
        assert_eq!(f("energy_particle", "MeV"), 1.602176634e-13);
        assert_eq!(f("area", "barn"), 1e-28);
        assert_eq!(f("service_time", "FPY"), 31_557_600.0);
        assert_eq!(f("service_time", "yr"), JULIAN_YEAR_S);
        assert_eq!(f("energy_plant", "GWd"), 8.64e13);
        assert_eq!(f("energy_plant", "MWd"), 8.64e10);
        assert_eq!(f("burnup", "MWd/kg"), 8.64e10);
        assert_eq!(f("burnup", "GWd/t"), f("burnup", "MWd/kg"));
        assert_eq!(f("activity", "Ci"), 3.7e10);
        assert_eq!(f("dose", "rem"), 0.01);
        assert_eq!(f("pressure", "atm"), 101_325.0);
        assert_eq!(f("dose_rate", "Sv/h"), 1.0 / 3600.0);
        assert_eq!(f("power_density", "W/cm3"), 1e6);
        assert_eq!(f("damage_rate", "dpa/FPY"), 1.0 / 31_557_600.0);
        assert_eq!(DAY_S, f("time", "d"));
        // The named constants are the registry's.
        assert_eq!(HOUR_S, f("time", "h"));
        // The units the requirement names are each defined with a source.
        for (g, s) in [
            ("area", "barn"),
            ("damage", "dpa"),
            ("burnup", "MWd/kg"),
            ("energy_plant", "GWd"),
            ("service_time", "FPY"),
        ] {
            assert!(unit(g, s).source.len() > 10, "{s}");
        }
    }

    // Verifies: L10N-022
    #[test]
    fn the_common_nuclear_and_engineering_units_are_offered() {
        let find = |g: &str, s: &str| assert!(find_in(g, s).is_some(), "{g}: {s}");
        for s in ["eV", "keV", "MeV", "J"] {
            find("energy_particle", s);
        }
        find("length", "cm");
        find("length", "m");
        find("area", "barn");
        find("particle_flux", "n/cm2/s");
        find("particle_flux", "n/cm\u{b2}/s");
        find("heat_flux", "MW/m2");
        find("heat_flux", "MW/m\u{b2}");
        find("fraction", "appm");
        find("damage", "dpa");
        find("dose_rate", "Sv/h");
        find("dose", "Sv");
        find("absorbed_dose", "Gy");
        find("activity", "Bq");
        find("activity", "Ci");
        find("power_density", "W/cm3");
        find("service_time", "yr");
        find("service_time", "FPY");
        find("time", "s");
    }

    // Verifies: CFG-042
    #[test]
    fn every_conversion_round_trips_within_1e_12() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut pairs = 0;
        for a in UNITS {
            for b in UNITS.iter().filter(|b| b.dimension() == a.dimension()) {
                pairs += 1;
                for _ in 0..40 {
                    // 1e-6 to 1e9 in either sign, log uniform.
                    let magnitude = crate::math::powf(10.0, -6.0 + 15.0 * rng.next());
                    let value = if rng.next() < 0.5 {
                        -magnitude
                    } else {
                        magnitude
                    };
                    let there = convert(value, a, b).unwrap();
                    let back = convert(there, b, a).unwrap();
                    // An offset unit is judged against the size of the values it adds, since
                    // 0 degC and 273.15 K are the same point and a relative error at 0 means nothing.
                    let scale = if a.is_offset() || b.is_offset() {
                        value.abs().max(274.0)
                    } else {
                        value.abs()
                    };
                    assert!(
                        (back - value).abs() <= 1e-12 * scale,
                        "{} -> {} -> {}: {value} came back as {back}",
                        a.symbol,
                        b.symbol,
                        a.symbol
                    );
                }
            }
        }
        assert!(pairs > 300, "{pairs}");
    }

    // Verifies: CFG-042
    #[test]
    fn conversions_agree_with_independent_arithmetic() {
        let ev = unit("energy_particle", "eV");
        let kev = unit("energy_particle", "keV");
        let j = unit("energy_particle", "J");
        let mev = unit("energy_particle", "MeV");
        assert_eq!(convert(14.1, mev, kev).unwrap(), 14.1 * 1000.0);
        assert!((convert(1.0, ev, j).unwrap() - 1.602176634e-19).abs() < 1e-34);
        // 14.1 MeV in joules, worked by hand: 14.1e6 * 1.602176634e-19.
        assert!((convert(14.1, mev, j).unwrap() / 2.2590e-12 - 1.0).abs() < 1e-4);
        // 1 GWd = 24 GWh.
        let gwd = unit("energy_plant", "GWd");
        let gwh = unit("energy_plant", "GWh");
        assert!((convert(1.0, gwd, gwh).unwrap() - 24.0).abs() < 1e-12);
        // 1 Ci = 37 GBq.
        let ci = unit("activity", "Ci");
        let gbq = unit("activity", "GBq");
        assert!((convert(1.0, ci, gbq).unwrap() - 37.0).abs() < 1e-12);
        // 10 mSv/h = 1 rem/h.
        let msvh = unit("dose_rate", "mSv/h");
        let remh = unit("dose_rate", "rem/h");
        assert!((convert(10.0, msvh, remh).unwrap() - 1.0).abs() < 1e-12);
        // 1 MW/m2 = 100 W/cm2.
        let mwm2 = unit("heat_flux", "MW/m2");
        let wcm2 = unit("heat_flux", "W/cm2");
        assert!((convert(1.0, mwm2, wcm2).unwrap() - 100.0).abs() < 1e-10);
        // 1 dpa/FPY is 1 / 31 557 600 dpa per second.
        let per_fpy = unit("damage_rate", "dpa/FPY");
        let per_s = unit("damage_rate", "dpa/s");
        assert!((convert(1.0, per_fpy, per_s).unwrap() * 31_557_600.0 - 1.0).abs() < 1e-12);
    }

    // Verifies: CFG-042
    #[test]
    fn offset_units_and_percent_against_fraction_are_exact_cases() {
        let c = unit("temperature", "degC");
        let k = unit("temperature", "K");
        assert!((convert(0.0, c, k).unwrap() - 273.15).abs() < 1e-12);
        assert!((convert(-196.0, c, k).unwrap() - 77.15).abs() < 1e-12);
        assert!((convert(77.0, k, c).unwrap() + 196.15).abs() < 1e-12);
        // A temperature offset is not a scale: 20 degC is not twice 10 degC in kelvin.
        let twenty = convert(20.0, c, k).unwrap();
        let ten = convert(10.0, c, k).unwrap();
        assert!((twenty / ten - 2.0).abs() > 0.01);
        let pct = unit("fraction", "%");
        let one = unit("fraction", "1");
        let appm = unit("fraction", "appm");
        assert_eq!(convert(5.0, pct, one).unwrap(), 0.05);
        assert_eq!(convert(0.05, one, pct).unwrap(), 5.0);
        assert!((convert(1.0, pct, appm).unwrap() - 10_000.0).abs() < 1e-9);
    }

    // Verifies: L10N-021
    #[test]
    fn mixed_dimensions_are_refused_with_cause_and_accepted_units() {
        let mev = unit("energy_particle", "MeV");
        let cm = unit("length", "cm");
        let error = convert(1.0, cm, mev).unwrap_err();
        let UnitError::Mismatch { accepted, .. } = &error else {
            panic!("{error:?}");
        };
        assert!(
            accepted.contains(&"keV") && accepted.contains(&"GWd"),
            "{accepted:?}"
        );
        let message = error.to_string();
        assert!(
            message.contains("cm") && message.contains("MeV"),
            "{message}"
        );
        assert!(
            message.contains("different things") && message.contains("Next step"),
            "{message}"
        );
        // Sievert against gray, and dpa against a fraction, look like numbers and are not.
        let sv = unit("dose", "Sv");
        let gy = unit("absorbed_dose", "Gy");
        assert!(convert(1.0, sv, gy).is_err());
        let dpa = unit("damage", "dpa");
        let one = unit("fraction", "1");
        assert!(convert(1.0, dpa, one).is_err());
        // Power is not energy.
        assert!(convert(1.0, unit("power", "MW"), unit("energy_plant", "MWd")).is_err());
        // Bq and 1/s are the same dimension, as the SI says.
        assert!(convert(1.0, unit("activity", "Bq"), unit("dose_rate", "Sv/s")).is_err());
    }

    // Verifies: CFG-044, L10N-022
    #[test]
    fn handbook_lists_every_unit_with_its_group_and_source() {
        let page = include_str!("../../../docs/guide/units.md");
        assert!(page.starts_with("<!-- kind: reference -->"));
        let (_, table) = page.split_once("## Every unit").expect("unit table");
        let rows: Vec<&str> = table.lines().filter(|l| l.starts_with("| ")).collect();
        // Header and rule rows, then one row per unit.
        assert_eq!(
            rows.len(),
            UNITS.len() + 2,
            "rows on the page vs the registry"
        );
        for unit in UNITS {
            let want = format!(
                "(`units.{}`) | `{}` | {} |",
                unit.group, unit.symbol, unit.name
            );
            assert!(
                rows.iter().any(|r| r.contains(&want)),
                "missing row: {want}"
            );
        }
    }

    #[test]
    fn dimension_text_is_readable() {
        assert_eq!(group("power").unwrap().dimension.text(), "m^2 kg s^-3");
        assert_eq!(Dimension::NONE.text(), "1");
        assert_eq!(group("length").unwrap().dimension.text(), "m");
    }
}
