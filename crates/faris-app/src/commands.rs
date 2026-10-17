//! The command registry (UX-030) and the keyboard command palette (A11Y-001).
//!
//! One table lists every app-level action with an id, label, search words,
//! shortcut, whether it changes the study, and the reason it is unavailable.
//! The palette (Ctrl/Cmd+K) lists the table with type-to-filter, arrows and
//! Enter; the global shortcuts are dispatched from the same table, so a
//! shortcut cannot exist without a palette entry. Controls inside a panel
//! (sliders, run buttons, text fields) are ordinary egui widgets: Tab reaches
//! them and Space or Enter operates them. `PANEL_ACTIONS` lists each panel's
//! primary actions and the command that carries them.

use crate::{FarisApp, Step, browser, study_file::Action};
use eframe::egui::{self, Key, Modifiers};
use faris_model::validation::{Quantity, note};

/// A key chord. `command` is Ctrl on Windows and Linux and Cmd on macOS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub command: bool,
    pub shift: bool,
    pub key: Key,
}

impl Shortcut {
    const fn key(key: Key) -> Self {
        Self {
            command: false,
            shift: false,
            key,
        }
    }
    const fn command(key: Key) -> Self {
        Self {
            command: true,
            shift: false,
            key,
        }
    }
    const fn command_shift(key: Key) -> Self {
        Self {
            command: true,
            shift: true,
            key,
        }
    }
    fn modifiers(self) -> Modifiers {
        let mut modifiers = Modifiers::NONE;
        if self.command {
            modifiers |= Modifiers::COMMAND;
        }
        if self.shift {
            modifiers |= Modifiers::SHIFT;
        }
        modifiers
    }
    /// "Ctrl+Shift+S"; on macOS the guide and the palette read Ctrl as Cmd.
    pub fn text(self) -> String {
        let mut text = String::new();
        if self.command {
            text.push_str("Ctrl+");
        }
        if self.shift {
            text.push_str("Shift+");
        }
        text.push_str(self.key.name());
        text
    }
}

pub struct Command {
    pub id: &'static str,
    pub label: &'static str,
    /// Extra words the palette filter matches.
    pub synonyms: &'static [&'static str],
    pub shortcut: Option<Shortcut>,
    /// Changes the study or a design input (not just what is shown).
    pub mutating: bool,
    /// The reason the command cannot run now, if it cannot.
    pub blocked: Option<fn(&FarisApp) -> Option<String>>,
    pub run: fn(&mut FarisApp, &egui::Context),
}

const fn command(
    id: &'static str,
    label: &'static str,
    synonyms: &'static [&'static str],
    shortcut: Option<Shortcut>,
    run: fn(&mut FarisApp, &egui::Context),
) -> Command {
    Command {
        id,
        label,
        synonyms,
        shortcut,
        mutating: false,
        blocked: None,
        run,
    }
}

impl Command {
    const fn mutating(mut self) -> Self {
        self.mutating = true;
        self
    }
    const fn blocked_by(mut self, reason: fn(&FarisApp) -> Option<String>) -> Self {
        self.blocked = Some(reason);
        self
    }
}

fn file_busy(app: &FarisApp) -> Option<String> {
    app.file
        .is_busy()
        .then(|| "A study file is opening or saving; wait for it to finish.".to_owned())
}

fn cannot_save(app: &FarisApp) -> Option<String> {
    file_busy(app).or_else(|| app.file.inputs.unsaveable.clone())
}

fn export_unavailable(app: &FarisApp) -> Option<String> {
    app.export_blocked().map(str::to_owned)
}

fn no_pair(app: &FarisApp) -> Option<String> {
    app.paired.is_none().then(|| {
        "The matched port or no-port scenario is not loaded. Start FARIS with --control-scenario or --control-bundle to switch."
            .to_owned()
    })
}

fn single_allocation(app: &FarisApp) -> Option<String> {
    (app.manifest.variants.len() < 2)
        .then(|| "This study has only one allocation, so there is nothing to switch to.".to_owned())
}

fn nothing_to_undo(app: &FarisApp) -> Option<String> {
    (!app.edits.history.can_undo()).then(|| {
        "There is nothing to undo yet. Changes you make in this session are listed under Edit, Edit history.".to_owned()
    })
}

fn nothing_to_redo(app: &FarisApp) -> Option<String> {
    (!app.edits.history.can_redo())
        .then(|| "There is nothing to redo. Redo is available after you undo a change.".to_owned())
}

fn go_to(app: &mut FarisApp, step: Step) {
    app.step = step;
    app.message = format!("Step {} {}.", step.number(), step.name());
}

/// Zoom factors the Interface size menu offers.
const SIZES: [f32; 5] = [1.0, 1.25, 1.5, 1.75, 2.0];

/// The next interface size up or down from `current`, staying in range.
pub fn next_size(current: f32, larger: bool) -> f32 {
    if larger {
        SIZES
            .into_iter()
            .find(|s| *s > current + 0.001)
            .unwrap_or(SIZES[SIZES.len() - 1])
    } else {
        SIZES
            .into_iter()
            .rev()
            .find(|s| *s < current - 0.001)
            .unwrap_or(SIZES[0])
    }
}

fn set_size(app: &mut FarisApp, ctx: &egui::Context, size: f32) {
    ctx.set_zoom_factor(size);
    app.message = format!("Interface size {:.0}%.", size * 100.0);
}

fn window_message(name: &str, open: bool) -> String {
    format!("{name} window {}.", if open { "opened" } else { "closed" })
}

fn move_year(app: &mut FarisApp, delta: f64) {
    app.year = (app.year + delta).clamp(0.0, app.manifest.horizon_years);
    app.message = format!("Timeline at year {:.1}.", app.year);
}

fn step_component(app: &mut FarisApp, forward: bool) {
    let components = &app.manifest.variants[app.variant].components;
    let at = components
        .iter()
        .position(|c| c.id == app.selected)
        .unwrap_or(0);
    let next = if forward {
        (at + 1) % components.len()
    } else {
        (at + components.len() - 1) % components.len()
    };
    app.selected = components[next].id.clone();
    app.message = format!("Selected component: {}.", components[next].label);
}

/// Every app-level action. Order is the palette's order for an empty filter.
pub static REGISTRY: &[Command] = &[
    command(
        "palette.open",
        "Open the command palette",
        &["commands", "search", "find"],
        Some(Shortcut::command(Key::K)),
        |app, _| app.palette.open(),
    ),
    command(
        "step.design",
        "Go to step 1: Design",
        &["plant", "geometry", "arrangement"],
        Some(Shortcut::key(Key::Num1)),
        |app, _| go_to(app, Step::Design),
    ),
    command(
        "step.simulate",
        "Go to step 2: Simulate",
        &["transport", "results", "breeding"],
        Some(Shortcut::key(Key::Num2)),
        |app, _| go_to(app, Step::Simulate),
    ),
    command(
        "step.operate",
        "Go to step 3: Operate",
        &["history", "timeline", "what-if", "plant life"],
        Some(Shortcut::key(Key::Num3)),
        |app, _| go_to(app, Step::Operate),
    ),
    command(
        "step.compare",
        "Go to step 4: Compare",
        &["difference", "2 sigma", "sweep"],
        Some(Shortcut::key(Key::Num4)),
        |app, _| go_to(app, Step::Compare),
    ),
    command(
        "step.evidence",
        "Go to step 5: Evidence",
        &["core", "receipts", "verify"],
        Some(Shortcut::key(Key::Num5)),
        |app, _| go_to(app, Step::Evidence),
    ),
    command(
        "edit.undo",
        "Undo the last change",
        &["revert", "back", "history"],
        Some(Shortcut::command(Key::Z)),
        |app, ctx| app.undo(ctx),
    )
    .blocked_by(nothing_to_undo),
    command(
        "edit.redo",
        "Redo the change you undid",
        &["forward", "again", "history"],
        Some(Shortcut::command_shift(Key::Z)),
        |app, ctx| app.redo(ctx),
    )
    .blocked_by(nothing_to_redo),
    command(
        "edit.redo_alt",
        "Redo (alternative key)",
        &["forward", "again", "history"],
        Some(Shortcut::command(Key::Y)),
        |app, ctx| app.redo(ctx),
    )
    .blocked_by(nothing_to_redo),
    command(
        "edit.history",
        "Edit history window (toggle)",
        &["undo list", "changes", "steps"],
        None,
        |app, _| {
            app.edits.window_open = !app.edits.window_open;
            app.message = window_message("Edit history", app.edits.window_open);
        },
    ),
    command(
        "tools.diagnostics",
        "Crash records and diagnostics window (toggle)",
        &["crash", "bundle", "report", "log"],
        None,
        |app, _| {
            app.diagnostics.open = !app.diagnostics.open;
            app.message = window_message("Crash records and diagnostics", app.diagnostics.open);
        },
    ),
    command(
        "file.open",
        "Open a study file…",
        &["load", ".faris"],
        Some(Shortcut::command(Key::O)),
        |app, ctx| app.file_action(ctx, Action::Open),
    )
    .blocked_by(file_busy),
    command(
        "file.save",
        "Save the study",
        &["write"],
        Some(Shortcut::command(Key::S)),
        |app, ctx| app.file_action(ctx, Action::Save),
    )
    .blocked_by(cannot_save),
    command(
        "file.save_as",
        "Save the study as…",
        &["write", "copy"],
        Some(Shortcut::command_shift(Key::S)),
        |app, ctx| app.file_action(ctx, Action::SaveAs),
    )
    .blocked_by(cannot_save),
    command(
        "file.include_evidence",
        "Store the Core evidence inside the saved file (toggle)",
        &["pack", "receipts"],
        None,
        |app, _| {
            app.file.include_evidence = !app.file.include_evidence;
            app.message = format!(
                "Core evidence will be {} the saved file.",
                if app.file.include_evidence {
                    "stored inside"
                } else {
                    "referenced by name and hash from"
                }
            );
        },
    )
    .mutating(),
    command(
        "export.begin",
        "Export the brief, tables and charts…",
        &["pdf", "csv", "report"],
        None,
        |app, ctx| app.begin_export(ctx),
    )
    .blocked_by(export_unavailable),
    command(
        "view.focus_viewport",
        "Focus the 3D viewport (arrows orbit, Shift+arrows pan, + and − zoom, 0 resets, Enter reads out)",
        &["camera", "orbit", "keyboard cursor"],
        None,
        |app, ctx| {
            app.message = if let Some(id) = app.viewport_id {
                ctx.memory_mut(|m| m.request_focus(id));
                "3D viewport focused. Arrows orbit, Shift+arrows pan, + and − zoom, 0 resets the view, Enter reads out what is at the centre. Tab leaves.".into()
            } else {
                "The 3D viewport is not on screen yet; try again after the first frame.".into()
            };
        },
    ),
    command(
        "view.frame_scene",
        "Reset the 3D view (frame scene)",
        &["camera", "home"],
        None,
        |app, _| {
            app.frame_scene();
            app.message = "3D view reset to frame the scene.".into();
        },
    ),
    command(
        "view.cutaway",
        "Cutaway on or off (toggle)",
        &["section", "quadrant"],
        None,
        |app, _| {
            app.cutaway = !app.cutaway;
            app.message = format!("Cutaway {}.", if app.cutaway { "on" } else { "off" });
        },
    ),
    command(
        "component.next",
        "Select the next component",
        &["outliner", "layer"],
        None,
        |app, _| step_component(app, true),
    ),
    command(
        "component.previous",
        "Select the previous component",
        &["outliner", "layer"],
        None,
        |app, _| step_component(app, false),
    ),
    command(
        "component.visibility",
        "Show or hide the selected component (toggle)",
        &["outliner", "hide"],
        None,
        |app, _| {
            if !app.hidden.remove(&app.selected) {
                app.hidden.insert(app.selected.clone());
            }
            let shown = !app.hidden.contains(&app.selected);
            app.message = format!(
                "Component {} is {}.",
                app.selected,
                if shown { "shown" } else { "hidden" }
            );
        },
    ),
    command(
        "allocation.next",
        "Switch to the next blanket and shield allocation",
        &["design parameter", "thickness", "variant", "change"],
        None,
        |app, _| {
            app.variant = (app.variant + 1) % app.manifest.variants.len();
            app.message = format!(
                "Allocation: {}.",
                crate::allocation_text(&app.manifest.variants[app.variant])
            );
        },
    )
    .mutating()
    .blocked_by(single_allocation),
    command(
        "arrangement.swap",
        "Switch between the port and no-port arrangement",
        &["penetration", "control", "compare"],
        None,
        |app, _| app.swap_arrangement(),
    )
    .mutating()
    .blocked_by(no_pair),
    command(
        "timeline.forward",
        "Timeline: one year later",
        &["history", "scrub", "year"],
        None,
        |app, _| move_year(app, 1.0),
    ),
    command(
        "timeline.back",
        "Timeline: one year earlier",
        &["history", "scrub", "year"],
        None,
        |app, _| move_year(app, -1.0),
    ),
    command(
        "timeline.start",
        "Timeline: go to year 0",
        &["history", "scrub", "year", "beginning"],
        None,
        |app, _| move_year(app, f64::NEG_INFINITY),
    ),
    command(
        "timeline.end",
        "Timeline: go to the end of the horizon",
        &["history", "scrub", "year", "last"],
        None,
        |app, _| move_year(app, f64::INFINITY),
    ),
    command(
        "readout.breeding",
        "Read out the tritium breeding ratio and its error",
        &["tbr", "h3", "result", "standard error"],
        None,
        |app, _| app.message = app.breeding_readout(),
    ),
    command(
        "readout.why_unknown",
        "Explain why a value is NOT_EVALUATED and what would settle it",
        &["why", "unknown", "validation", "settle"],
        None,
        |app, _| app.message = not_evaluated_readout(),
    ),
    command(
        "tools.maintenance",
        "Maintenance window (toggle)",
        &["outage", "replacement"],
        None,
        |app, _| {
            app.maintenance.open = !app.maintenance.open;
            app.message = window_message("Maintenance", app.maintenance.open);
        },
    ),
    command(
        "tools.compare_versions",
        "Compare versions window (toggle)",
        &["diff", "difference", "changes", "semantic"],
        None,
        |app, _| {
            app.diff.open = !app.diff.open;
            app.message = window_message("Compare versions", app.diff.open);
        },
    ),
    command(
        "tools.design_check",
        "Design check window (toggle)",
        &["step file", "cad", "import"],
        None,
        |app, _| {
            app.design_check.open = !app.design_check.open;
            app.message = window_message("Design check", app.design_check.open);
        },
    )
    .blocked_by(|_| browser::IS_BROWSER.then(|| browser::desktop_only("The Design check window"))),
    command(
        "tools.design_flux",
        "Design flux window (toggle)",
        &["magnet", "fluence limit"],
        None,
        |app, _| {
            app.design_flux.open = !app.design_flux.open;
            app.message = window_message("Design flux", app.design_flux.open);
        },
    )
    .blocked_by(|_| browser::IS_BROWSER.then(|| browser::desktop_only("The Design flux window"))),
    command(
        "tour.replay",
        "Replay the guided tour",
        &["help", "introduction"],
        None,
        |app, _| {
            app.start_tour(0);
            app.message =
                "Guided tour started. Right arrow or Enter go on, Left goes back, Esc skips."
                    .into();
        },
    ),
    // Only the browser version has this window; the desktop keyboard page does
    // not list it.
    #[cfg(target_arch = "wasm32")]
    command(
        "browser.about",
        "About this browser version",
        &["disclaimer", "limits", "desktop app", "what works here"],
        None,
        |app, _| app.notice.open_about(),
    ),
    command(
        "interface.larger",
        "Interface size: larger",
        &["zoom", "text size", "scale"],
        None,
        |app, ctx| {
            let size = next_size(ctx.zoom_factor(), true);
            set_size(app, ctx, size)
        },
    ),
    command(
        "interface.smaller",
        "Interface size: smaller",
        &["zoom", "text size", "scale"],
        None,
        |app, ctx| {
            let size = next_size(ctx.zoom_factor(), false);
            set_size(app, ctx, size)
        },
    ),
    command(
        "interface.reset",
        "Interface size: 100%",
        &["zoom", "text size", "scale"],
        None,
        |app, ctx| set_size(app, ctx, 1.0),
    ),
];

#[cfg(test)]
/// The primary actions of each panel and the command that carries each.
/// Controls not listed here are native egui widgets that Tab reaches and
/// Space or Enter operates (see `NATIVE_CONTROLS`).
pub const PANEL_ACTIONS: &[(&str, &str, &str)] = &[
    ("Top bar", "Steps 1 to 5", "step.design"),
    ("Top bar", "Steps 1 to 5", "step.simulate"),
    ("Top bar", "Steps 1 to 5", "step.operate"),
    ("Top bar", "Steps 1 to 5", "step.compare"),
    ("Top bar", "Steps 1 to 5", "step.evidence"),
    ("Top bar", "Edit > Undo", "edit.undo"),
    ("Top bar", "Edit > Redo", "edit.redo"),
    ("Top bar", "Edit > Redo", "edit.redo_alt"),
    ("Top bar", "Edit > Edit history", "edit.history"),
    ("Top bar", "File > Crash records", "tools.diagnostics"),
    ("Top bar", "File > Open", "file.open"),
    ("Top bar", "File > Save", "file.save"),
    ("Top bar", "File > Save as", "file.save_as"),
    (
        "Top bar",
        "File > include Core evidence",
        "file.include_evidence",
    ),
    ("Top bar", "Export", "export.begin"),
    ("Top bar", "Interface size menu", "interface.larger"),
    ("Top bar", "Interface size menu", "interface.smaller"),
    ("Top bar", "Interface size menu", "interface.reset"),
    ("Top bar", "Maintenance", "tools.maintenance"),
    ("Top bar", "Compare versions", "tools.compare_versions"),
    ("Top bar", "Design check", "tools.design_check"),
    ("Top bar", "Design flux", "tools.design_flux"),
    ("Top bar", "Tour", "tour.replay"),
    #[cfg(target_arch = "wasm32")]
    ("Top bar", "Browser version menu", "browser.about"),
    ("Design step", "Port or no-port", "arrangement.swap"),
    ("Design step", "Allocation", "allocation.next"),
    ("Outliner", "Select a component", "component.next"),
    ("Outliner", "Select a component", "component.previous"),
    ("Outliner", "Show component", "component.visibility"),
    ("3D viewport", "Cutaway", "view.cutaway"),
    ("3D viewport", "Frame scene", "view.frame_scene"),
    (
        "3D viewport",
        "Orbit, pan, zoom, read out",
        "view.focus_viewport",
    ),
    ("Timeline", "Scrub the year", "timeline.forward"),
    ("Timeline", "Scrub the year", "timeline.back"),
    ("Timeline", "Scrub the year", "timeline.start"),
    ("Timeline", "Scrub the year", "timeline.end"),
    (
        "Simulate step",
        "Read the breeding ratio",
        "readout.breeding",
    ),
    (
        "Status line",
        "Why a value is NOT_EVALUATED",
        "readout.why_unknown",
    ),
];

#[cfg(test)]
/// Controls that are plain egui widgets: Tab moves between them, Space or
/// Enter presses or toggles them, arrows change sliders and open lists.
pub const NATIVE_CONTROLS: &[(&str, &str)] = &[
    ("Simulate step", "transport run settings and Run"),
    (
        "Operate step",
        "what-if assumptions, sensitivity and ensemble controls",
    ),
    ("Evidence step", "Core stage selection and run buttons"),
    ("Compare step", "allocation sweep controls"),
    (
        "Timeline",
        "plot choice, series toggles, year slider, event jump list",
    ),
    (
        "Windows",
        "Maintenance, Design check and Design flux fields and buttons",
    ),
];

#[cfg(test)]
/// Keys that act only while a plot or the viewport has keyboard focus.
pub const FOCUSED_KEYS: &[(&str, &str, &str)] = &[
    ("3D viewport", "Arrow keys", "Orbit"),
    ("3D viewport", "Shift+Arrow keys", "Pan"),
    ("3D viewport", "+ or =", "Zoom in"),
    ("3D viewport", "-", "Zoom out"),
    ("3D viewport", "0", "Reset the view"),
    (
        "3D viewport",
        "Enter or Space",
        "Read out what is at the centre of the view",
    ),
    (
        "Timeline plot",
        "Left or Right",
        "Move the cursor year by 0.1",
    ),
    (
        "Timeline plot",
        "Shift+Left or Shift+Right",
        "Move the cursor year by 1",
    ),
    (
        "Timeline plot",
        "Page Up or Page Down",
        "Move the cursor year by 5",
    ),
    ("Timeline plot", "Home or End", "Go to the start or end"),
];

#[cfg(test)]
pub fn find(id: &str) -> Option<&'static Command> {
    REGISTRY.iter().find(|c| c.id == id)
}

/// Commands matching every word of `query` (label, id or search words), best
/// first: label starts with the query, then label contains it, then the rest.
pub fn matching(query: &str) -> Vec<&'static Command> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut found: Vec<(u8, usize, &'static Command)> = REGISTRY
        .iter()
        .enumerate()
        .filter_map(|(index, command)| {
            let label = command.label.to_lowercase();
            let haystack = format!("{label} {} {}", command.id, command.synonyms.join(" "));
            if !words.iter().all(|w| haystack.contains(w.as_str())) {
                return None;
            }
            let rank = if label.starts_with(&words.join(" ")) {
                0
            } else if words.iter().all(|w| label.contains(w.as_str())) {
                1
            } else {
                2
            };
            Some((rank, index, command))
        })
        .collect();
    found.sort_by_key(|(rank, index, _)| (*rank, *index));
    found.into_iter().map(|(_, _, command)| command).collect()
}

/// The breeding ratio's validation state in words, with why and what next.
pub fn not_evaluated_readout() -> String {
    let note = note(Quantity::Tbr);
    format!(
        "Tritium breeding ratio is {}. Why: {} Next step: {}",
        note.state.label(),
        note.why,
        note.next_step
    )
}

/// Keep the arrow keys for the widget that has focus instead of moving focus.
pub fn lock_arrows(ctx: &egui::Context, id: egui::Id) {
    ctx.memory_mut(|m| {
        m.set_focus_lock_filter(
            id,
            egui::EventFilter {
                horizontal_arrows: true,
                vertical_arrows: true,
                ..Default::default()
            },
        )
    });
}

/// The cursor year after this frame's keys on a focused plot, if one moved it.
pub fn plot_year_after(ctx: &egui::Context, year: f64, horizon: f64) -> Option<f64> {
    ctx.input_mut(|input| {
        let mut next = year;
        for (modifiers, key, delta) in [
            (Modifiers::SHIFT, Key::ArrowLeft, -1.0),
            (Modifiers::SHIFT, Key::ArrowRight, 1.0),
            (Modifiers::NONE, Key::ArrowLeft, -0.1),
            (Modifiers::NONE, Key::ArrowRight, 0.1),
            (Modifiers::NONE, Key::PageUp, 5.0),
            (Modifiers::NONE, Key::PageDown, -5.0),
        ] {
            if input.consume_key(modifiers, key) {
                next += delta;
                break;
            }
        }
        if input.consume_key(Modifiers::NONE, Key::Home) {
            next = 0.0;
        }
        if input.consume_key(Modifiers::NONE, Key::End) {
            next = horizon;
        }
        let next = next.clamp(0.0, horizon);
        ((next - year).abs() > 1e-12).then_some(next)
    })
}

/// The palette window: a text filter over the registry.
#[derive(Default)]
pub struct Palette {
    pub open: bool,
    query: String,
    selected: usize,
    fresh: bool,
}

impl Palette {
    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
        self.fresh = true;
    }

    /// Draw the palette. Returns the command to run when Enter or a click chose one.
    pub fn show(&mut self, ctx: &egui::Context, app: &FarisApp) -> Option<&'static Command> {
        if !self.open {
            return None;
        }
        let (up, down, enter, escape) = ctx.input_mut(|input| {
            (
                input.consume_key(Modifiers::NONE, Key::ArrowUp),
                input.consume_key(Modifiers::NONE, Key::ArrowDown),
                input.consume_key(Modifiers::NONE, Key::Enter),
                input.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if escape {
            self.open = false;
            return None;
        }
        let mut chosen = None;
        egui::Window::new("Command palette")
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 56.0))
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .default_width(560.0)
            .show(ctx, |ui| {
                ui.strong("Command palette");
                ui.weak("Type to filter · Up and Down to choose · Enter to run · Esc to close");
                let before = self.query.clone();
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("Type a command")
                        .desired_width(f32::INFINITY),
                );
                if self.fresh {
                    edit.request_focus();
                    self.fresh = false;
                }
                if self.query != before {
                    self.selected = 0;
                }
                let matches = matching(&self.query);
                if matches.is_empty() {
                    ui.add_space(4.0);
                    ui.label("No command matches. Clear the text to list every command.");
                    return;
                }
                if down {
                    self.selected = (self.selected + 1) % matches.len();
                }
                if up {
                    self.selected = (self.selected + matches.len() - 1) % matches.len();
                }
                self.selected = self.selected.min(matches.len() - 1);
                if enter {
                    chosen = Some(matches[self.selected]);
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(340.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (index, command) in matches.iter().enumerate() {
                            let reason = command.blocked.and_then(|blocked| blocked(app));
                            let mut button = egui::Button::new(command.label)
                                .selected(index == self.selected)
                                .min_size(egui::vec2(ui.available_width(), 0.0));
                            if let Some(shortcut) = command.shortcut {
                                button = button.shortcut_text(shortcut.text());
                            }
                            let response = ui.add(button);
                            if (up || down) && index == self.selected {
                                response.scroll_to_me(None);
                            }
                            if let Some(reason) = &reason {
                                ui.weak(format!("Unavailable now: {reason}"));
                            }
                            if response.clicked() {
                                chosen = Some(*command);
                            }
                        }
                    });
            });
        if chosen.is_some() {
            self.open = false;
        }
        chosen
    }
}

impl FarisApp {
    /// Run a command, or say why it cannot run.
    pub fn run_command(&mut self, ctx: &egui::Context, command: &Command) {
        if let Some(reason) = command.blocked.and_then(|blocked| blocked(self)) {
            self.message = format!("{} is unavailable: {reason}", command.label);
            return;
        }
        (command.run)(self, ctx);
    }

    /// Global shortcuts from the registry, then the palette. Unmodified keys
    /// wait while a text field has focus, while the palette is open and during
    /// the tour (which owns the arrows, Enter and Escape).
    pub fn keyboard(&mut self, ctx: &egui::Context) {
        let text_focused = ctx.text_edit_focused();
        let plain_ok = !self.tour.active && !self.palette.open && !text_focused;
        let pressed = ctx.input_mut(|input| {
            // Shift chords first: `consume_key` ignores a surplus Shift.
            for with_shift in [true, false] {
                for command in REGISTRY {
                    let Some(shortcut) = command.shortcut else {
                        continue;
                    };
                    if shortcut.shift != with_shift || (!shortcut.command && !plain_ok) {
                        continue;
                    }
                    // A text field being edited keeps its own undo for Ctrl+Z and Ctrl+Y.
                    if shortcut.command && matches!(shortcut.key, Key::Z | Key::Y) && text_focused {
                        continue;
                    }
                    if !shortcut.command && input.modifiers.any() {
                        continue;
                    }
                    if input.consume_key(shortcut.modifiers(), shortcut.key) {
                        return Some(command);
                    }
                }
            }
            None
        });
        if let Some(command) = pressed {
            if self.tour.active && command.id == "palette.open" {
                self.message = "Finish or skip the tour first (Esc), then open the palette.".into();
            } else {
                self.run_command(ctx, command);
            }
        }
        let palette = std::mem::take(&mut self.palette);
        let mut palette = palette;
        let chosen = palette.show(ctx, self);
        self.palette = palette;
        if let Some(command) = chosen {
            self.run_command(ctx, command);
        }
    }

    /// Sentence for the status line: the breeding ratio and its error, or why
    /// there is none.
    pub fn breeding_readout(&self) -> String {
        let variant = &self.manifest.variants[self.variant];
        let tally = self
            .transport
            .record(&variant.id)
            .filter(|r| r.normalized.is_some())
            .and_then(|record| {
                let rate = record.normalized.as_ref()?.source_neutron_rate_per_s;
                let tally = self
                    .transport
                    .response(&variant.id, "total-tritium-production")
                    .or_else(|| self.transport.response(&variant.id, "blanket-tritium"))?;
                Some((
                    tally.integrated_mean / rate,
                    tally.integrated_standard_error / rate,
                ))
            });
        match tally {
            Some((mean, error)) => format!(
                "Tritium breeding, {}: {mean:.4} ± {error:.4} H3 per source neutron (one Monte Carlo standard error). {}",
                crate::allocation_text(variant),
                crate::not_evaluated::NOT_QUALIFIED.line()
            ),
            None => format!(
                "Tritium breeding ratio: {}",
                crate::not_evaluated::NO_TRANSPORT_RESULT.line()
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SessionInputs, build_session};
    use std::{collections::BTreeSet, path::PathBuf};

    const ASSUMPTIONS: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scenarios/arc-inspired/demo-operating-assumptions.json"
    );
    const KEYBOARD_PAGE: &str = include_str!("../../../docs/guide/keyboard.md");

    fn app() -> FarisApp {
        let session = build_session(SessionInputs {
            assumptions: Some(PathBuf::from(ASSUMPTIONS)),
            ..SessionInputs::empty(PathBuf::from("runs"))
        })
        .unwrap();
        FarisApp::new(
            session.manifest,
            None,
            None,
            PathBuf::from("runs"),
            session.transport,
            session.control,
            session.history,
        )
        .unwrap()
    }

    /// One frame with these events, then the app's keyboard handling.
    struct Keys {
        ctx: egui::Context,
    }

    impl Keys {
        fn new() -> Self {
            Self {
                ctx: egui::Context::default(),
            }
        }
        fn frame(&self, app: &mut FarisApp, events: Vec<egui::Event>) {
            let raw = egui::RawInput {
                events,
                ..Default::default()
            };
            self.ctx
                .run_ui(raw, |ui| app.keyboard(&ui.ctx().clone()))
                .drop_without_applying_deltas();
        }
        fn press(&self, app: &mut FarisApp, modifiers: Modifiers, key: Key) {
            self.frame(
                app,
                vec![egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
            );
        }
        fn type_text(&self, app: &mut FarisApp, text: &str) {
            self.frame(app, vec![egui::Event::Text(text.into())]);
        }
        /// Ctrl+K, type the words, Enter: the palette path to one command.
        fn palette(&self, app: &mut FarisApp, words: &str) {
            self.press(app, Modifiers::COMMAND, Key::K);
            assert!(app.palette.open, "Ctrl+K opens the palette");
            self.frame(app, vec![]);
            self.type_text(app, words);
            self.press(app, Modifiers::NONE, Key::Enter);
            // The closed text field gives up focus on the next frame.
            self.frame(app, vec![]);
        }
    }

    // Verifies: A11Y-001, UX-030, UX-031
    #[test]
    fn every_registry_entry_is_complete_unique_and_in_the_palette() {
        let mut ids = BTreeSet::new();
        let everything = matching("");
        assert_eq!(everything.len(), REGISTRY.len());
        for command in REGISTRY {
            assert!(ids.insert(command.id), "duplicate id {}", command.id);
            assert!(!command.label.is_empty(), "{} has no label", command.id);
            assert!(
                command.id.contains('.') && command.id == command.id.to_lowercase(),
                "{} is not a dotted lower-case id",
                command.id
            );
            assert!(
                everything.iter().any(|c| std::ptr::eq(*c, command)),
                "{} is not listed in the palette",
                command.id
            );
            assert!(
                matching(command.label).iter().any(|c| c.id == command.id),
                "{} cannot be found by its own label",
                command.id
            );
        }
        let mut chords = BTreeSet::new();
        for shortcut in REGISTRY.iter().filter_map(|c| c.shortcut) {
            assert!(chords.insert(shortcut.text()), "{} twice", shortcut.text());
        }
    }

    // Verifies: A11Y-001, UX-030
    #[test]
    fn each_panels_primary_actions_are_registered() {
        for (panel, action, id) in PANEL_ACTIONS {
            assert!(
                find(id).is_some(),
                "{panel}: {action} names no command {id}"
            );
        }
        // Every command is carried by a panel row, so none is orphaned.
        let carried: BTreeSet<_> = PANEL_ACTIONS.iter().map(|(_, _, id)| *id).collect();
        for command in REGISTRY {
            assert!(
                carried.contains(command.id) || command.id == "palette.open",
                "{} belongs to no panel row",
                command.id
            );
        }
        assert!(!NATIVE_CONTROLS.is_empty());
    }

    // Verifies: A11Y-001
    #[test]
    fn every_command_runs_from_the_keyboard_or_says_why_not() {
        let keys = Keys::new();
        for command in REGISTRY {
            if command.id.starts_with("file.") || command.id == "export.begin" {
                // These open native file dialogs; their blocked reasons are checked below.
                continue;
            }
            let mut app = app();
            app.file.inputs.unsaveable = Some("Nothing to save.".into());
            let before = (app.step, app.variant, app.cutaway, app.message.clone());
            keys.palette(&mut app, command.label);
            let blocked = command.blocked.and_then(|b| b(&app));
            let after = (app.step, app.variant, app.cutaway, app.message.clone());
            if let Some(reason) = blocked {
                assert!(
                    app.message.contains(&reason),
                    "{}: reason not shown",
                    command.id
                );
            } else if command.id != "palette.open" {
                assert_ne!(before, after, "{} did nothing from the palette", command.id);
            }
        }
        // The study-file and export commands report their reasons when blocked.
        let mut app = app();
        app.file.inputs.unsaveable = Some("Nothing to save: no input files.".into());
        for id in ["file.save", "file.save_as"] {
            let command = find(id).unwrap();
            app.run_command(&keys.ctx, command);
            assert!(
                app.message.contains("Nothing to save"),
                "{id}: {}",
                app.message
            );
        }
    }

    // Verifies: A11Y-001
    #[test]
    fn step_keys_and_command_shortcuts_dispatch_from_the_registry() {
        let keys = Keys::new();
        let mut app = app();
        for (index, step) in Step::ALL.into_iter().enumerate() {
            let key = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5][index];
            keys.press(&mut app, Modifiers::NONE, key);
            assert_eq!(app.step, step);
        }
        // A digit typed into a text field is text, not a step change.
        app.step = Step::Design;
        keys.press(&mut app, Modifiers::COMMAND, Key::K);
        keys.frame(&mut app, vec![]);
        keys.press(&mut app, Modifiers::NONE, Key::Num3);
        assert_eq!(app.step, Step::Design);
        keys.press(&mut app, Modifiers::NONE, Key::Escape);
        assert!(!app.palette.open);
        // Ctrl+Shift+S is Save as, not Save.
        let save_as = find("file.save_as").unwrap().shortcut.unwrap();
        let save = find("file.save").unwrap().shortcut.unwrap();
        assert!(save_as.shift && !save.shift && save_as.key == save.key);
    }

    #[test]
    fn the_palette_filters_ranks_and_navigates() {
        let keys = Keys::new();
        let mut app = app();
        assert_eq!(matching("zzzz").len(), 0);
        assert_eq!(matching("simulate")[0].id, "step.simulate");
        assert_eq!(matching("breeding")[0].id, "readout.breeding");
        assert!(matching("step compare")[0].id == "step.compare");
        // Arrows move the choice and Enter runs it.
        keys.press(&mut app, Modifiers::COMMAND, Key::K);
        keys.frame(&mut app, vec![]);
        keys.type_text(&mut app, "step");
        keys.press(&mut app, Modifiers::NONE, Key::ArrowDown);
        keys.press(&mut app, Modifiers::NONE, Key::ArrowDown);
        keys.press(&mut app, Modifiers::NONE, Key::Enter);
        // Entries match "step" in label order: design, simulate, operate.
        assert_eq!(app.step, Step::Operate);
        assert!(!app.palette.open);
        assert_eq!(next_size(1.0, true), 1.25);
        assert_eq!(next_size(2.0, true), 2.0);
        assert_eq!(next_size(1.0, false), 1.0);
        assert_eq!(next_size(1.5, false), 1.25);
    }

    // Verifies: A11Y-001, UX-010, UX-036
    #[test]
    fn core_tasks_t1_to_t5_are_reachable_with_key_events_only() {
        let keys = Keys::new();
        let mut app = app();

        // T1: open the sample study (it is open), go to Simulate, read the
        // breeding ratio. With no recorded transport the readout says why.
        keys.press(&mut app, Modifiers::NONE, Key::Num2);
        assert_eq!(app.step, Step::Simulate);
        keys.palette(&mut app, "breeding ratio");
        assert!(app.message.contains("Tritium breeding"), "{}", app.message);
        assert!(app.message.contains("NOT_EVALUATED"));

        // T2: change a design parameter (the allocation) and reach the history.
        let allocation = app.variant;
        keys.palette(&mut app, "next blanket");
        assert_ne!(app.variant, allocation);
        keys.press(&mut app, Modifiers::NONE, Key::Num3);
        assert_eq!(app.step, Step::Operate);
        keys.palette(&mut app, "later");
        assert_eq!(app.year, 1.0);
        keys.palette(&mut app, "end of the horizon");
        assert_eq!(app.year, app.manifest.horizon_years);

        // T3: compare two arrangements. The Compare step opens; switching to
        // the matched scenario says what is missing when it is not loaded.
        keys.press(&mut app, Modifiers::NONE, Key::Num4);
        assert_eq!(app.step, Step::Compare);
        keys.palette(&mut app, "port no-port");
        assert!(app.message.contains("not loaded"), "{}", app.message);

        // T4: save and export. Both are one chord or palette entry away. A
        // study without input files cannot be saved and the app says why; the
        // file and folder dialogs themselves are native and not driven here,
        // so an unblocked export is only checked to be the first palette match.
        app.file.inputs.unsaveable = Some("Nothing to save: no input files.".into());
        keys.press(&mut app, Modifiers::COMMAND, Key::S);
        assert!(app.message.contains("Nothing to save"), "{}", app.message);
        assert_eq!(matching("export")[0].id, "export.begin");
        if export_unavailable(&app).is_some() {
            keys.palette(&mut app, "export");
            assert!(app.message.contains("unavailable"), "{}", app.message);
        }

        // T5: find why a value is NOT_EVALUATED and what would settle it.
        keys.press(&mut app, Modifiers::NONE, Key::Num5);
        assert_eq!(app.step, Step::Evidence);
        keys.palette(&mut app, "why not_evaluated");
        assert!(app.message.contains("Why:"), "{}", app.message);
        assert!(app.message.contains("Next step:"), "{}", app.message);
    }

    // Verifies: A11Y-001, A11Y-008
    #[test]
    fn the_viewport_and_plot_keys_move_the_view_and_the_cursor_year() {
        let keys = Keys::new();
        let mut camera = crate::camera::Camera::default();
        for key in [Key::ArrowLeft, Key::Plus, Key::Minus] {
            assert!(camera.nudge(key, false));
        }
        let mut year = 10.0;
        for (modifiers, key, expected) in [
            (Modifiers::NONE, Key::ArrowRight, 10.1),
            (Modifiers::SHIFT, Key::ArrowLeft, 9.1),
            (Modifiers::NONE, Key::PageUp, 14.1),
            (Modifiers::NONE, Key::Home, 0.0),
            (Modifiers::NONE, Key::End, 30.0),
        ] {
            let mut moved = None;
            let raw = egui::RawInput {
                events: vec![egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            };
            keys.ctx
                .run_ui(raw, |ui| {
                    moved = plot_year_after(&ui.ctx().clone(), year, 30.0);
                })
                .drop_without_applying_deltas();
            let moved = moved.expect("the key moves the year");
            assert!((moved - expected).abs() < 1e-9, "{key:?}: {moved}");
            year = moved;
        }
    }

    // Verifies: A11Y-001, DOC-002, UX-034
    #[test]
    fn the_keyboard_page_lists_every_shortcut_and_command() {
        for command in REGISTRY {
            assert!(
                KEYBOARD_PAGE.contains(command.label),
                "keyboard.md lacks the command {:?}",
                command.label
            );
            if let Some(shortcut) = command.shortcut {
                assert!(
                    KEYBOARD_PAGE.contains(&shortcut.text()),
                    "keyboard.md lacks {}",
                    shortcut.text()
                );
            }
        }
        for (_, keys, what) in FOCUSED_KEYS {
            assert!(KEYBOARD_PAGE.contains(keys), "keyboard.md lacks {keys}");
            assert!(KEYBOARD_PAGE.contains(what), "keyboard.md lacks {what}");
        }
    }
}
