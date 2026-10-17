//! The Units menu and the entry field for a quantity (UX-066, CFG-040, L10N-021, L10N-023).
//!
//! The menu chooses the unit system, a unit per quantity group and the number format. It changes
//! only what the desktop displays and how typed numbers are read; the choice is kept in the same
//! settings file as `faris config set units.system`. The field shows a quantity in the unit
//! chosen and refuses a value of another dimension with the cause and the accepted units.

use crate::quantity_text as text;
use eframe::egui;
use faris_model::display::{Decimal, DisplayPrefs};
use faris_model::units::{self, GROUPS};
use std::ops::RangeInclusive;

const ERROR_ID: &str = "units-menu-error";

/// The Units menu of the top bar.
pub fn menu(ui: &mut egui::Ui) {
    let current = text::prefs();
    ui.menu_button(format!("Units: {}", current.system.name()), |ui| {
        ui.label("Display units and number format");
        let mut problem: Option<String> = None;
        for (value, label) in [
            ("si", "SI (m, s, W, J)"),
            ("fusion", "Fusion practical (MW, keV, cm, GWd, dpa)"),
        ] {
            if ui
                .selectable_label(current.system.name() == value, label)
                .clicked()
            {
                problem = text::choose("units.system", value).err();
            }
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Decimal mark");
            for (value, label) in [("point", "1.5"), ("comma", "1,5")] {
                let on = (value == "comma") == (current.number.decimal == Decimal::Comma);
                if ui.selectable_label(on, label).clicked() {
                    problem = text::choose("format.decimal", value).err();
                }
            }
        });
        ui.collapsing("Unit for each quantity", |ui| {
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(ui, |ui| {
                    for group in GROUPS {
                        problem = group_row(ui, group.id, &current).or(problem.take());
                    }
                });
        });
        ui.small(
            "Display only: studies and results stay in SI base units, and nothing stored changes.",
        );
        if let Some(message) = problem {
            ui.data_mut(|d| d.insert_temp(egui::Id::new(ERROR_ID), message));
        }
        if let Some(message) = ui.data(|d| d.get_temp::<String>(egui::Id::new(ERROR_ID))) {
            ui.colored_label(egui::Color32::from_rgb(200, 60, 60), message);
        }
    })
    .response
    .on_hover_text(
        "Choose the units quantities are shown and typed in. Stored files do not change.",
    );
}

fn group_row(ui: &mut egui::Ui, group_id: &'static str, current: &DisplayPrefs) -> Option<String> {
    let group = units::group(group_id)?;
    let mut problem = None;
    ui.horizontal(|ui| {
        ui.label(group.label);
        let chosen = current.is_chosen(group_id);
        egui::ComboBox::from_id_salt(("units-group", group_id))
            .selected_text(current.unit_for(group_id).symbol)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(false, "follow the unit system")
                    .clicked()
                {
                    text::forget(&format!("units.{group_id}"));
                }
                for unit in units::units_of(group_id) {
                    let on = chosen && current.unit_for(group_id).symbol == unit.symbol;
                    if ui
                        .selectable_label(on, format!("{} ({})", unit.symbol, unit.name))
                        .clicked()
                    {
                        problem = text::choose(&format!("units.{group_id}"), unit.symbol).err();
                    }
                }
            });
    });
    problem
}

/// What a typed quantity means: the value in SI base units, or the message that refuses it.
/// `range` is in SI base units.
pub fn entry_value(
    p: &DisplayPrefs,
    group: &str,
    habit: &str,
    typed: &str,
    range: &RangeInclusive<f64>,
) -> Result<f64, String> {
    let si = text::parse_with(p, group, habit, typed)?;
    if range.contains(&si) {
        return Ok(si);
    }
    Err(format!(
        "{} is outside the allowed range {} to {}. Next step: type a value inside it.",
        typed.trim(),
        text::quantity_with(p, group, habit, *range.start(), 4),
        text::quantity_with(p, group, habit, *range.end(), 4),
    ))
}

/// The value as a field shows it before it is edited: whole numbers without decimals.
fn shown(p: &DisplayPrefs, group: &str, habit: &str, si: f64) -> String {
    let value = p.unit_or(group, habit).from_si(si);
    if (value - value.round()).abs() < 1e-9 * value.abs().max(1.0) {
        p.number.fixed(value, 0)
    } else {
        p.number.significant(value, 4)
    }
}

#[derive(Clone)]
struct Draft {
    text: String,
    error: Option<String>,
}

/// Takes the typed draft: a value that is accepted becomes the new `si` and clears the message;
/// one that is refused leaves `si` as it was and keeps the message for the field to show.
fn apply_entry(
    draft: &mut Draft,
    p: &DisplayPrefs,
    group: &str,
    habit: &str,
    si: &mut f64,
    range: &RangeInclusive<f64>,
) -> bool {
    match entry_value(p, group, habit, &draft.text, range) {
        Ok(value) => {
            *si = value;
            draft.error = None;
            true
        }
        Err(message) => {
            draft.error = Some(message);
            false
        }
    }
}

/// A text field for a quantity of `group`, held in SI base units in `si`. The field shows the
/// number in the unit chosen with the unit beside it; typing a number takes it in that unit,
/// typing a number and a unit takes any unit of the same dimension, and a unit of another
/// dimension is refused with the message under the field, leaving the value as it was.
/// Returns true when `si` changed.
pub fn quantity_field(
    ui: &mut egui::Ui,
    salt: &str,
    group: &str,
    habit: &str,
    si: &mut f64,
    range: RangeInclusive<f64>,
) -> bool {
    let p = text::prefs();
    let id = ui.make_persistent_id(salt);
    let mut draft = ui
        .data(|d| d.get_temp::<Draft>(id))
        .unwrap_or_else(|| Draft {
            text: shown(&p, group, habit, *si),
            error: None,
        });
    let mut changed = false;
    let response = ui.add(egui::TextEdit::singleline(&mut draft.text).desired_width(90.0));
    if response.changed() {
        changed = apply_entry(&mut draft, &p, group, habit, si, &range);
    } else if !response.has_focus() {
        // Not being edited: follow the value and the chosen unit; an unaccepted entry is dropped.
        draft.text = shown(&p, group, habit, *si);
        draft.error = None;
    }
    ui.label(text::unit_symbol_with(&p, group, habit));
    if let Some(message) = &draft.error {
        ui.colored_label(egui::Color32::from_rgb(200, 60, 60), message);
    }
    ui.data_mut(|d| d.insert_temp(id, draft));
    changed
}

#[cfg(test)]
mod tests;
