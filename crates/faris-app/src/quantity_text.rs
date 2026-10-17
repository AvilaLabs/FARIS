//! Quantities as the desktop shows and reads them: the unit and number text of a widget come from
//! the user's `DisplayPrefs` (units.system, units.<group>, format.decimal and format.grouping,
//! CFG-037, CFG-040, UX-066), a value with an uncertainty is written by the engine's formatter with
//! its interval convention beside it (UNC-081, UNC-082), and a typed quantity is read through
//! `DisplayPrefs::parse` so a wrong dimension is refused with the accepted units (L10N-021).
//!
//! Display only: every function takes a value in SI base units and returns text, or takes text and
//! returns a value in SI base units. Nothing here is stored with a study or a result (L10N-023).
//! The `*_with` functions take the preferences as an argument so they are tested without the
//! shared state; the short ones read the preferences the user chose.

use crate::settings::{self, Settings};
use faris_engine::tally_health;
use faris_model::display::{Decimal, DisplayPrefs};
use faris_model::interval_display::{IntervalConvention, format_with_uncertainty};
use faris_model::tally_statistics::MeshConvergence;
use faris_model::units;
use std::{path::Path, sync::Mutex};

/// The settings behind the current preferences, read from the user settings file on first use
/// (the browser has no file, so its choices last for the session).
static CHOICES: Mutex<Option<Settings>> = Mutex::new(None);

/// The preferences the settings give. Choosing the SI system names its unit for every quantity,
/// so the SI choice shows W, s and n/m2/s; with no choice at all the panels keep their usual
/// units (see [`quantity_with`]).
fn derive(
    settings: &Settings,
    file: &Path,
    env: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> DisplayPrefs {
    let mut settings = settings.clone();
    if settings.value("units.system").as_deref() == Some("si") {
        for group in units::GROUPS {
            let key = format!("units.{}", group.id);
            if settings.value(&key).is_none() {
                let _ = settings.set_value(&key, group.si);
            }
        }
    }
    let resolved = settings.resolve(file, env);
    DisplayPrefs::from_resolved(&resolved).unwrap_or_default()
}

/// The user settings file. Tests never read it: what the developer chose must not change them.
fn settings_file() -> Option<std::path::PathBuf> {
    #[cfg(all(not(target_arch = "wasm32"), not(test)))]
    return settings::default_path();
    #[cfg(any(target_arch = "wasm32", test))]
    None
}

/// An environment variable (FARIS_UNITS and the like); tests see none.
fn env(name: &str) -> Option<std::ffi::OsString> {
    #[cfg(not(test))]
    return std::env::var_os(name);
    #[cfg(test)]
    {
        let _ = name;
        None
    }
}

fn with_choices<T>(read: impl FnOnce(&mut Settings) -> T) -> T {
    let mut guard = CHOICES.lock().unwrap_or_else(|e| e.into_inner());
    let choices = guard.get_or_insert_with(|| {
        settings_file().map_or_else(Settings::default, |file| settings::load(&file).settings)
    });
    read(choices)
}

/// The display choices in force: the panels' usual units and plain numbers until the user
/// chooses otherwise.
pub fn prefs() -> DisplayPrefs {
    let file = settings_file().unwrap_or_default();
    with_choices(|choices| derive(choices, &file, &env))
}

/// Chooses `key` (`units.system`, `units.<group>`, `format.decimal`, ...) for this session and,
/// when there is a settings file that can be written, for the next start. The shared registry
/// checks the value; a refused value changes nothing and says why. Only the preferences change:
/// no study or result is touched.
pub fn choose(key: &str, text: &str) -> Result<(), String> {
    let file = settings_file();
    with_choices(|choices| choose_in(choices, file.as_deref(), key, text))
}

fn choose_in(
    choices: &mut Settings,
    file: Option<&Path>,
    key: &str,
    text: &str,
) -> Result<(), String> {
    let mut trial = choices.clone();
    trial.set_value(key, text)?;
    let resolved = trial.resolve(file.unwrap_or(Path::new("")), &env);
    DisplayPrefs::from_resolved(&resolved).map_err(|p| p.join(" "))?;
    *choices = trial;
    if let Some(file) = file {
        save_choice(file, key, Some(text));
    }
    Ok(())
}

/// The text chosen for `key` (`units.system`, ...); None when nothing is set, which means each
/// panel keeps its usual unit.
pub fn chosen(key: &str) -> Option<String> {
    with_choices(|choices| choices.value(key))
}

/// Forgets the choice for `key`, so the system default or the built-in default applies again.
pub fn forget(key: &str) {
    let file = settings_file();
    with_choices(|choices| forget_in(choices, file.as_deref(), key));
}

fn forget_in(choices: &mut Settings, file: Option<&Path>, key: &str) {
    choices.unset_value(key);
    if let Some(file) = file {
        save_choice(file, key, None);
    }
}

/// Writes one choice into the settings file, keeping every other section. A failure only logs:
/// the choice still applies for this session.
pub fn save_choice(file: &Path, key: &str, text: Option<&str>) {
    let loaded = settings::load(file);
    if !loaded.writable {
        eprintln!("FARIS did not remember the display choice {key}");
        return;
    }
    let mut stored = loaded.settings;
    match text {
        Some(text) => {
            if let Err(error) = stored.set_value(key, text) {
                eprintln!("FARIS did not remember {key}: {error}");
                return;
            }
        }
        None => stored.unset_value(key),
    }
    if let Err(error) = settings::save(file, &stored) {
        eprintln!(
            "FARIS could not remember {key} in {}: {error}",
            file.display()
        );
    }
}

/// The preferences a settings file gives (SI and plain numbers when it chooses nothing).
#[cfg(test)]
pub fn prefs_from_file(file: &Path) -> DisplayPrefs {
    derive(&settings::load(file).settings, file, &env)
}

/// The number written in the unit shown for `group`, and the unit's symbol, always together
/// (L10N-022). `habit` is the unit the panel has always printed (a symbol of the group), kept
/// while the user has chosen nothing for the group; a chosen system or override replaces it.
pub fn quantity_with(
    p: &DisplayPrefs,
    group: &str,
    habit: &str,
    si: f64,
    decimals: usize,
) -> String {
    p.show_in(p.unit_or(group, habit), si, decimals)
}

pub fn quantity(group: &str, habit: &str, si: f64, decimals: usize) -> String {
    quantity_with(&prefs(), group, habit, si, decimals)
}

/// The symbol of the unit shown for `group`.
pub fn unit_symbol_with(p: &DisplayPrefs, group: &str, habit: &str) -> &'static str {
    p.unit_or(group, habit).symbol
}

fn decimal_mark(p: &DisplayPrefs, text: String) -> String {
    match p.number.decimal {
        Decimal::Point => text,
        Decimal::Comma => text.replace('.', ","),
    }
}

/// `1.234 ± 0.012 (1 σ standard error)`: the digits follow the uncertainty and the convention is
/// written beside the interval, never left to a hover (UNC-081, UNC-082). `standard_error` is one
/// Monte Carlo standard error.
pub fn measurement_with(p: &DisplayPrefs, value: f64, standard_error: f64) -> String {
    interval_text(p, value, standard_error, "")
}

pub fn measurement(value: f64, standard_error: f64) -> String {
    measurement_with(&prefs(), value, standard_error)
}

/// The same for a quantity of `group`: value and error converted to the unit shown, the unit's
/// symbol after them and the convention label last.
pub fn measurement_in_with(
    p: &DisplayPrefs,
    group: &str,
    habit: &str,
    si_value: f64,
    si_standard_error: f64,
) -> String {
    let unit = p.unit_or(group, habit);
    // A standard error is a difference, so it is scaled and not offset.
    let scale = unit.from_si(1.0) - unit.from_si(0.0);
    interval_text(
        p,
        unit.from_si(si_value),
        si_standard_error * scale,
        unit.symbol,
    )
}

pub fn measurement_in(group: &str, habit: &str, si_value: f64, si_standard_error: f64) -> String {
    measurement_in_with(&prefs(), group, habit, si_value, si_standard_error)
}

fn interval_text(p: &DisplayPrefs, value: f64, standard_error: f64, symbol: &str) -> String {
    let pair = format_with_uncertainty(value, standard_error);
    let unit = if symbol.is_empty() {
        String::new()
    } else {
        format!(" {symbol}")
    };
    match (&pair.uncertainty, pair.exponent) {
        // Written in plain form: both numbers take the decimal mark and the digit grouping.
        (Some(error), None) => format!(
            "{} ± {}{unit} ({})",
            localize(p, &pair.value),
            localize(p, error),
            IntervalConvention::OneSigma.label()
        ),
        (Some(_), Some(_)) => decimal_mark(
            p,
            format!(
                "{}{unit} ({})",
                pair.text,
                IntervalConvention::OneSigma.label()
            ),
        ),
        (None, _) => decimal_mark(p, format!("{}{unit}", pair.text)),
    }
}

/// A plain decimal number rewritten with the format's decimal mark and grouping, keeping its digits.
fn localize(p: &DisplayPrefs, number: &str) -> String {
    let decimals = number.split_once('.').map_or(0, |(_, f)| f.len());
    number
        .parse::<f64>()
        .map_or_else(|_| number.to_owned(), |x| p.number.fixed(x, decimals))
}

/// A relative standard error as `± 4.3 % (1 σ standard error)`.
pub fn relative_error_with(p: &DisplayPrefs, fraction: f64) -> String {
    format!(
        "± {} % ({})",
        p.number.fixed(100.0 * fraction, 1),
        IntervalConvention::OneSigma.label()
    )
}

pub fn relative_error(fraction: f64) -> String {
    relative_error_with(&prefs(), fraction)
}

/// Scientific notation for a large or small quantity, in the unit the user chose or else in
/// `habit`, always with the registry's symbol.
pub fn scientific_with(
    p: &DisplayPrefs,
    group: &str,
    habit: &str,
    si: f64,
    decimals: usize,
) -> String {
    let unit = p.unit_or(group, habit);
    format!(
        "{} {}",
        p.number.scientific(unit.from_si(si), decimals),
        unit.symbol
    )
}

pub fn scientific(group: &str, habit: &str, si: f64, decimals: usize) -> String {
    scientific_with(&prefs(), group, habit, si, decimals)
}

/// An ISO date (`2026-10-15`) as the date format chosen writes it; text that is not an ISO date
/// is returned as it is.
pub fn date_with(p: &DisplayPrefs, iso: &str) -> String {
    let mut parts = iso.trim().splitn(3, '-');
    match (
        parts.next().and_then(|y| y.parse::<i64>().ok()),
        parts.next().and_then(|m| m.parse::<u32>().ok()),
        parts.next().and_then(|d| d.parse::<u32>().ok()),
    ) {
        (Some(y), Some(m), Some(d)) if (1..=12).contains(&m) && (1..=31).contains(&d) => {
            p.date.render(y, m, d)
        }
        _ => iso.to_owned(),
    }
}

pub fn date(iso: &str) -> String {
    date_with(&prefs(), iso)
}

/// Reads text typed for a quantity of `group` and returns SI base units. A bare number is taken
/// in the unit the field shows; a number with a unit may use any unit of the same dimension. Any
/// other is refused with the cause, both dimensions and the accepted units (L10N-021).
pub fn parse_with(p: &DisplayPrefs, group: &str, habit: &str, text: &str) -> Result<f64, String> {
    let text = text.trim();
    match p.number.parse(text) {
        Ok(value) => Ok(p.unit_or(group, habit).to_si(value)),
        Err(_) => p.parse(group, text).map_err(|e| e.to_string()),
    }
}

/// The convergence of a mesh tally as one sentence for the panel that shows the tally (NUC-049).
pub fn convergence_text(c: &MeshConvergence) -> String {
    let line = tally_health::convergence_line_for(c, "voxels");
    match c.worst_relative_error {
        Some(worst) => format!(
            "Mesh convergence: {line}; worst relative error {:.1} %.",
            100.0 * worst
        ),
        None => format!("Mesh convergence: {line}; no voxel has a score."),
    }
}

/// The convergence sentence for the mesh tally in `results`, or None when it has no voxel result
/// (the panel then says nothing about a mesh it does not have).
pub fn mesh_note(
    results: &[faris_engine::transport::NormalizedTally],
    mesh_id: &str,
    voxels: usize,
) -> Option<String> {
    tally_health::mesh_tally_convergence(results, mesh_id, voxels).map(|c| convergence_text(&c))
}

#[cfg(test)]
mod tests;
