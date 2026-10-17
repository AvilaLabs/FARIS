//! Undo and redo for every change a user makes in the desktop (GEO-062,
//! UX-020 to UX-025, CFG-025).
//!
//! The editable state is one value, [`EditState`]: the saved view (allocation,
//! arrangement, what-if assumptions, preset, year, field view, plot, sweep
//! pick), the Core study choices, the hidden components, the cutaway, whether
//! evidence is packed into saved files and the interface size. Once per frame
//! the app compares that value with the last one in the [`History`]; a
//! difference is one step. Because steps come from comparing states, a new
//! control that changes any of these is undoable without extra code, and a
//! hover, a selection or a camera move creates nothing.
//!
//! One gesture is one step: while the pointer is down or a text field has
//! focus, successive changes of the same kind extend the open step. A slider
//! drag, a typed value and a click each undo in one step.
//!
//! The history is a list of states with a cursor, so undo, redo and a jump to
//! any listed step are the same move. It is bounded by memory, not by a short
//! depth: [`MAX_STEPS`] steps or [`MAX_BYTES`] bytes, whichever comes first;
//! the oldest step is then dropped and the user is told.
//!
//! Undo only changes what this window holds. A saved file, a written export
//! and a started run are outside it: each is recorded as a note in the list,
//! the file menu and buttons say "cannot be undone" before the action, and
//! undoing past a started run cancels it without pretending to rewind it.

use crate::FarisApp;
use eframe::egui;
use faris_engine::study::StudySelection;
use faris_study::ViewState;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};

/// Most steps kept before the oldest is dropped (UX-021).
pub const MAX_STEPS: usize = 10_000;
/// Most history kept in memory, in bytes, before the oldest step is dropped.
pub const MAX_BYTES: usize = 500 * 1024 * 1024;
/// How long after its last change a continuous edit (a typed number, a
/// dragged value) still counts as the same step.
const COALESCE_SECONDS: f64 = 0.6;
/// Settings changes kept in the activity list (CFG-025).
pub const SETTINGS_LOG_LIMIT: usize = 100;

/// The words that go with an action undo cannot take back (UX-024).
pub const CANNOT_UNDO: &str = "cannot be undone";

/// Everything the user can edit that undo covers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EditState {
    pub view: ViewState,
    pub selection: StudySelection,
    pub hidden: BTreeSet<String>,
    pub cutaway: bool,
    pub include_evidence: bool,
    pub interface_scale: f32,
}

fn view_same(a: &ViewState, b: &ViewState) -> bool {
    // The step the user is looking at is navigation, not an edit.
    a.preset == b.preset
        && a.what_if == b.what_if
        && a.year == b.year
        && a.field_view == b.field_view
        && a.history_tab == b.history_tab
        && a.arrangement == b.arrangement
        && a.allocation == b.allocation
        && a.sweep_blanket_m == b.sweep_blanket_m
}

impl EditState {
    /// Equal in every edit; the workflow step is ignored.
    pub fn same_edit(&self, other: &Self) -> bool {
        view_same(&self.view, &other.view)
            && self.selection == other.selection
            && self.hidden == other.hidden
            && self.cutaway == other.cutaway
            && self.include_evidence == other.include_evidence
            && (self.interface_scale - other.interface_scale).abs() < 0.001
    }

    /// What the state costs to keep, roughly: its serialized size plus a fixed overhead.
    fn approx_bytes(&self) -> usize {
        serde_json::to_vec(self).map_or(1024, |bytes| bytes.len()) + 512
    }
}

/// One kind of change, in the order a label names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Allocation,
    Arrangement,
    Preset,
    WhatIf,
    Year,
    Quantity,
    HistoryTab,
    Sweep,
    Selection,
    Hidden,
    Cutaway,
    Evidence,
    InterfaceScale,
}

impl Field {
    /// Edits made by dragging or typing, which keep extending one step for a moment.
    fn continuous(self) -> bool {
        matches!(self, Field::Year | Field::WhatIf | Field::Selection)
    }
}

/// The kinds of change between two states, in label order.
pub fn changed_fields(a: &EditState, b: &EditState) -> Vec<Field> {
    let mut out = Vec::new();
    let (x, y) = (&a.view, &b.view);
    if x.allocation != y.allocation {
        out.push(Field::Allocation);
    }
    if x.arrangement != y.arrangement {
        out.push(Field::Arrangement);
    }
    if x.preset != y.preset {
        out.push(Field::Preset);
    }
    if x.what_if != y.what_if {
        out.push(Field::WhatIf);
    }
    if x.year != y.year {
        out.push(Field::Year);
    }
    if x.field_view != y.field_view {
        out.push(Field::Quantity);
    }
    if x.history_tab != y.history_tab {
        out.push(Field::HistoryTab);
    }
    if x.sweep_blanket_m != y.sweep_blanket_m {
        out.push(Field::Sweep);
    }
    if a.selection != b.selection {
        out.push(Field::Selection);
    }
    if a.hidden != b.hidden {
        out.push(Field::Hidden);
    }
    if a.cutaway != b.cutaway {
        out.push(Field::Cutaway);
    }
    if a.include_evidence != b.include_evidence {
        out.push(Field::Evidence);
    }
    if (a.interface_scale - b.interface_scale).abs() >= 0.001 {
        out.push(Field::InterfaceScale);
    }
    out
}

/// Leaf paths whose values differ between two what-if assumption sets.
fn what_if_paths(a: &ViewState, b: &ViewState) -> Vec<String> {
    fn walk(prefix: &str, a: &serde_json::Value, b: &serde_json::Value, out: &mut Vec<String>) {
        match (a, b) {
            (serde_json::Value::Object(x), serde_json::Value::Object(y)) => {
                let keys: BTreeSet<_> = x.keys().chain(y.keys()).collect();
                for key in keys {
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix} {key}")
                    };
                    walk(
                        &path,
                        x.get(key).unwrap_or(&serde_json::Value::Null),
                        y.get(key).unwrap_or(&serde_json::Value::Null),
                        out,
                    );
                }
            }
            _ if a != b => out.push(prefix.replace('_', " ")),
            _ => {}
        }
    }
    let value = |v: &ViewState| serde_json::to_value(&v.what_if).unwrap_or_default();
    let mut out = Vec::new();
    walk("", &value(a), &value(b), &mut out);
    out
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

fn selection_label(a: &StudySelection, b: &StudySelection) -> String {
    for (name, before, after) in [
        ("breeding", a.breeding, b.breeding),
        ("shielding", a.shielding, b.shielding),
        ("fuel history", a.fuel_history, b.fuel_history),
        ("electricity", a.electricity, b.electricity),
    ] {
        if before != after {
            return format!("Core study {name} turned {}", on_off(after));
        }
    }
    format!("Core study minimum breeding ratio set to {}", b.minimum_tbr)
}

/// The plain-English label of the change from `a` to `b`.
pub fn describe(a: &EditState, b: &EditState) -> String {
    let fields = changed_fields(a, b);
    let Some(first) = fields.first() else {
        return "No change".into();
    };
    let head = match first {
        Field::Allocation => format!("Allocation changed to {}", b.view.allocation),
        Field::Arrangement => format!(
            "Arrangement changed to {}",
            if b.view.arrangement == "control" {
                "the no-port control"
            } else {
                "the outboard port"
            }
        ),
        Field::Preset => format!(
            "Operating preset changed to {}",
            b.view.preset.as_deref().unwrap_or("none")
        ),
        Field::WhatIf => {
            let paths = what_if_paths(&a.view, &b.view);
            match paths.as_slice() {
                [] => "What-if assumptions changed".into(),
                [one] => format!("What-if changed: {one}"),
                [one, rest @ ..] => format!("What-if changed: {one} and {} more", rest.len()),
            }
        }
        Field::Year => format!("Timeline moved to year {:.1}", b.view.year),
        Field::Quantity => format!("Field view changed to {}", b.view.field_view),
        Field::HistoryTab => format!("Plot changed to {}", b.view.history_tab),
        Field::Sweep => match b.view.sweep_blanket_m {
            Some(m) => format!("Sweep allocation chosen: {m:.3} m blanket"),
            None => "Sweep allocation choice cleared".into(),
        },
        Field::Selection => selection_label(&a.selection, &b.selection),
        Field::Hidden => {
            let shown: Vec<_> = a.hidden.difference(&b.hidden).collect();
            let hidden: Vec<_> = b.hidden.difference(&a.hidden).collect();
            match (hidden.first(), shown.first()) {
                (Some(id), _) => format!("Component {id} hidden"),
                (_, Some(id)) => format!("Component {id} shown"),
                _ => "Component visibility changed".into(),
            }
        }
        Field::Cutaway => format!("Cutaway turned {}", on_off(b.cutaway)),
        Field::Evidence => format!(
            "Core evidence in saved files turned {}",
            on_off(b.include_evidence)
        ),
        Field::InterfaceScale => {
            format!("Interface size set to {:.0} %", b.interface_scale * 100.0)
        }
    };
    match fields.len() - 1 {
        0 => head,
        1 => format!("{head} (and 1 other change)"),
        n => format!("{head} (and {n} other changes)"),
    }
}

/// A settings-like change inside a step: the key, old and new text, and the layer it is kept in.
fn setting_changes(
    a: &EditState,
    b: &EditState,
) -> Vec<(&'static str, String, String, &'static str)> {
    let mut out = Vec::new();
    if (a.interface_scale - b.interface_scale).abs() >= 0.001 {
        out.push((
            "ui.scale",
            format!("{}", a.interface_scale),
            format!("{}", b.interface_scale),
            "user file",
        ));
    }
    if a.include_evidence != b.include_evidence {
        out.push((
            "study.include_evidence",
            on_off(a.include_evidence).to_owned(),
            on_off(b.include_evidence).to_owned(),
            "study file",
        ));
    }
    out
}

/// One line of the settings activity list (CFG-025).
#[derive(Clone, Debug, PartialEq)]
pub struct SettingLogEntry {
    pub at_unix: f64,
    pub key: &'static str,
    pub old: String,
    pub new: String,
    pub layer: &'static str,
    /// True when the line records an undo, redo or jump.
    pub by_undo: bool,
}

/// Something that happened outside the history and that undo does not reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteKind {
    Saved,
    Exported,
    RunStarted,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    /// How many steps were applied when it happened.
    pub position: usize,
    pub kind: NoteKind,
    pub text: String,
    pub at_unix: f64,
}

pub struct StepEntry {
    pub label: String,
    pub at_unix: f64,
    state: EditState,
    bytes: usize,
}

struct Open {
    field: Field,
    last: f64,
    gesture: bool,
}

/// What [`History::observe`] did with the frame's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observed {
    Nothing,
    Pushed,
    Extended,
    /// A change that returned to where the step began removed the step.
    Cancelled,
}

/// A move of the cursor, to apply to the app.
pub struct Move {
    pub from: usize,
    pub to: usize,
    pub state: EditState,
    /// The workflow step to show: where the change being undone or redone was made.
    pub show_step: String,
    /// Labels of the steps crossed, newest first for an undo and oldest first for a redo.
    pub labels: Vec<String>,
    /// Outside actions an undo passed over.
    pub crossed: Vec<Note>,
}

pub struct History {
    base: EditState,
    base_bytes: usize,
    steps: VecDeque<StepEntry>,
    cursor: usize,
    notes: Vec<Note>,
    settings_log: VecDeque<SettingLogEntry>,
    dropped: usize,
    dropped_unreported: usize,
    total_bytes: usize,
    max_steps: usize,
    max_bytes: usize,
    open: Option<Open>,
}

impl History {
    pub fn new(base: EditState) -> Self {
        Self::with_limits(base, MAX_STEPS, MAX_BYTES)
    }

    pub fn with_limits(base: EditState, max_steps: usize, max_bytes: usize) -> Self {
        let base_bytes = base.approx_bytes();
        Self {
            base,
            base_bytes,
            steps: VecDeque::new(),
            cursor: 0,
            notes: Vec::new(),
            settings_log: VecDeque::new(),
            dropped: 0,
            dropped_unreported: 0,
            total_bytes: base_bytes,
            max_steps,
            max_bytes,
            open: None,
        }
    }

    /// The state at the cursor.
    pub fn head(&self) -> &EditState {
        self.state_at(self.cursor)
    }

    pub fn state_at(&self, position: usize) -> &EditState {
        match position {
            0 => &self.base,
            n => &self.steps[n - 1].state,
        }
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.cursor < self.steps.len()
    }

    pub fn entries(&self) -> impl Iterator<Item = &StepEntry> {
        self.steps.iter()
    }

    pub fn notes(&self) -> &[Note] {
        &self.notes
    }

    pub fn settings_log(&self) -> impl Iterator<Item = &SettingLogEntry> {
        self.settings_log.iter()
    }

    /// Steps dropped from the front so far because of the limits.
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    /// Memory the history holds, roughly.
    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.total_bytes
    }

    pub fn limits(&self) -> (usize, usize) {
        (self.max_steps, self.max_bytes)
    }

    /// The label of the step an undo would take back.
    pub fn undo_label(&self) -> Option<&str> {
        self.cursor
            .checked_sub(1)
            .map(|i| self.steps[i].label.as_str())
    }

    /// The label of the step a redo would apply again.
    pub fn redo_label(&self) -> Option<&str> {
        self.steps.get(self.cursor).map(|s| s.label.as_str())
    }

    /// Say once that the limits dropped steps since the last call.
    pub fn take_cap_notice(&mut self) -> Option<String> {
        let n = std::mem::take(&mut self.dropped_unreported);
        (n > 0).then(|| {
            format!(
                "The undo history reached its limit ({} steps or {} MB), so the oldest {n} step{} can no longer be undone. Save your work to keep its current state.",
                self.max_steps,
                self.max_bytes / (1024 * 1024),
                if n == 1 { "" } else { "s" }
            )
        })
    }

    /// Start over from `state`: another study was opened, so earlier steps no longer apply.
    pub fn rebase(&mut self, state: EditState) {
        self.base_bytes = state.approx_bytes();
        self.total_bytes = self.base_bytes;
        self.base = state;
        self.steps.clear();
        self.cursor = 0;
        self.notes.clear();
        self.open = None;
    }

    /// Take `state` as the current one without making a step (the guided tour, scripted runs).
    pub fn adopt(&mut self, state: EditState) {
        self.open = None;
        match self.cursor {
            0 => {
                self.total_bytes -= self.base_bytes;
                self.base_bytes = state.approx_bytes();
                self.total_bytes += self.base_bytes;
                self.base = state;
            }
            n => {
                let step = &mut self.steps[n - 1];
                self.total_bytes -= step.bytes;
                step.bytes = state.approx_bytes();
                self.total_bytes += step.bytes;
                step.state = state;
            }
        }
    }

    /// Record an outside action at the current position.
    pub fn note(&mut self, kind: NoteKind, text: impl Into<String>, at_unix: f64) {
        self.notes.push(Note {
            position: self.cursor,
            kind,
            text: text.into(),
            at_unix,
        });
        self.open = None;
    }

    fn log_settings(
        &mut self,
        a: &EditState,
        b: &EditState,
        at_unix: f64,
        extend: bool,
        by_undo: bool,
    ) {
        for (key, old, new, layer) in setting_changes(a, b) {
            if extend
                && let Some(last) = self.settings_log.back_mut()
                && last.key == key
                && !last.by_undo
            {
                last.new = new;
                last.at_unix = at_unix;
                continue;
            }
            self.settings_log.push_back(SettingLogEntry {
                at_unix,
                key,
                old,
                new,
                layer,
                by_undo,
            });
            while self.settings_log.len() > SETTINGS_LOG_LIMIT {
                self.settings_log.pop_front();
            }
        }
    }

    /// Compare the frame's state with the head. `gesture` is true while a drag
    /// or a text edit is under way. A difference makes a step, extends the open
    /// one, or (when a drag ends where it began) removes it.
    pub fn observe(&mut self, current: &EditState, gesture: bool, now: f64) -> Observed {
        if current.same_edit(self.head()) {
            if !gesture {
                self.open = None;
            }
            return Observed::Nothing;
        }
        let fields = changed_fields(self.head(), current);
        let primary = fields[0];
        let extend = self.cursor == self.steps.len()
            && self.cursor > 0
            && self.open.as_ref().is_some_and(|open| {
                open.field == primary
                    && fields.len() == 1
                    && ((gesture && open.gesture)
                        || (primary.continuous() && now - open.last <= COALESCE_SECONDS))
            });
        if extend {
            let before = self.state_at(self.cursor - 1).clone();
            if current.same_edit(&before) {
                // The drag came back to where it began: nothing happened.
                let step = self.steps.pop_back().expect("an open step exists");
                self.total_bytes -= step.bytes;
                self.cursor -= 1;
                self.open = None;
                return Observed::Cancelled;
            }
            let head_before_extend = self.head().clone();
            let label = describe(&before, current);
            let step = self.steps.back_mut().expect("an open step exists");
            self.total_bytes -= step.bytes;
            step.bytes = current.approx_bytes();
            self.total_bytes += step.bytes;
            step.state = current.clone();
            step.label = label;
            step.at_unix = now;
            if let Some(open) = &mut self.open {
                open.last = now;
                open.gesture = gesture;
            }
            self.log_settings(&head_before_extend, current, now, true, false);
            return Observed::Extended;
        }
        let before = self.head().clone();
        // A new change after an undo discards the steps that were undone.
        while self.steps.len() > self.cursor {
            let gone = self.steps.pop_back().expect("longer than the cursor");
            self.total_bytes -= gone.bytes;
        }
        for note in &mut self.notes {
            note.position = note.position.min(self.cursor);
        }
        let bytes = current.approx_bytes();
        self.steps.push_back(StepEntry {
            label: describe(&before, current),
            at_unix: now,
            state: current.clone(),
            bytes,
        });
        self.total_bytes += bytes;
        self.cursor += 1;
        self.open = Some(Open {
            field: primary,
            last: now,
            gesture,
        });
        self.log_settings(&before, current, now, false, false);
        self.enforce_limits();
        Observed::Pushed
    }

    fn enforce_limits(&mut self) {
        while self.steps.len() > 1
            && (self.steps.len() > self.max_steps || self.total_bytes > self.max_bytes)
        {
            let gone = self.steps.pop_front().expect("more than one step");
            self.total_bytes -= self.base_bytes;
            self.base_bytes = gone.state.approx_bytes();
            self.total_bytes += self.base_bytes;
            self.total_bytes -= gone.bytes;
            self.base = gone.state;
            self.cursor = self.cursor.saturating_sub(1);
            for note in &mut self.notes {
                note.position = note.position.saturating_sub(1);
            }
            self.dropped += 1;
            self.dropped_unreported += 1;
            if self.cursor == 0 {
                self.open = None;
            }
        }
    }

    /// Move the cursor to `position` (0 is the state the session began with).
    pub fn jump_to(&mut self, position: usize, now: f64) -> Option<Move> {
        if position > self.steps.len() || position == self.cursor {
            return None;
        }
        let from = self.cursor;
        let before = self.head().clone();
        let state = self.state_at(position).clone();
        let (labels, show_step) = if position < from {
            (
                (position..from)
                    .rev()
                    .map(|i| self.steps[i].label.clone())
                    .collect(),
                self.steps[from - 1].state.view.step.clone(),
            )
        } else {
            (
                (from..position)
                    .map(|i| self.steps[i].label.clone())
                    .collect(),
                self.steps[position - 1].state.view.step.clone(),
            )
        };
        let crossed = if position < from {
            self.notes
                .iter()
                .filter(|n| n.position > position && n.position <= from)
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        self.cursor = position;
        self.open = None;
        self.log_settings(&before, &state, now, false, true);
        Some(Move {
            from,
            to: position,
            state,
            show_step,
            labels,
            crossed,
        })
    }

    pub fn undo(&mut self, now: f64) -> Option<Move> {
        self.cursor
            .checked_sub(1)
            .and_then(|to| self.jump_to(to, now))
    }

    pub fn redo(&mut self, now: f64) -> Option<Move> {
        self.jump_to(self.cursor + 1, now)
    }
}

/// Clock time of a unix second count, "12:03:45 UTC".
pub fn clock(unix: f64) -> String {
    let s = (unix.max(0.0) as u64) % faris_model::units::DAY_S as u64;
    format!("{:02}:{:02}:{:02} UTC", s / 3600, (s / 60) % 60, s % 60)
}

pub fn now_unix() -> f64 {
    crate::time::SystemTime::now()
        .duration_since(crate::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// The history plus what the app tracks around it.
pub struct Edits {
    pub history: History,
    /// The next frame takes its state as the new beginning (a study was just opened).
    pub rebase: bool,
    pub window_open: bool,
    was_running: bool,
    /// An interface size just set: egui applies it at the start of the next pass, so
    /// until then the state reads this instead of the context.
    pending_scale: Option<f32>,
}

impl EditState {
    /// A state to hold the place of the real one until the app exists.
    pub fn placeholder() -> Self {
        Self {
            view: ViewState::default(),
            selection: StudySelection::default(),
            hidden: BTreeSet::new(),
            cutaway: true,
            include_evidence: false,
            interface_scale: 1.0,
        }
    }
}

impl Edits {
    pub fn new(base: EditState) -> Self {
        Self {
            history: History::new(base),
            rebase: true,
            window_open: false,
            was_running: false,
            pending_scale: None,
        }
    }
}

/// The rows of the history list: which was clicked and where each is.
#[derive(Default)]
pub struct Rows {
    pub clicked: Option<usize>,
    pub rects: Vec<(usize, egui::Rect)>,
}

/// The step list, newest first, with the outside actions between steps. A row
/// is the state after its step; the first row is how the session began.
pub fn history_rows(ui: &mut egui::Ui, history: &History, now: f64) -> Rows {
    let mut rows = Rows::default();
    for position in (0..=history.len()).rev() {
        for note in history
            .notes()
            .iter()
            .filter(|n| n.position == position)
            .rev()
        {
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.weak(format!("{}  {}", clock(note.at_unix), note.text));
            });
        }
        let (label, at) = match position {
            0 => (
                "Start of this session (the state when it was opened)".to_owned(),
                None,
            ),
            n => {
                let step = history.entries().nth(n - 1).expect("position is in range");
                (step.label.clone(), Some(step.at_unix))
            }
        };
        let current = position == history.cursor();
        let undone = position > history.cursor();
        let mut text = egui::RichText::new(match at {
            Some(at) => format!("{}  {label}", clock(at)),
            None => label,
        });
        if undone {
            text = text.weak();
        }
        if current {
            text = text.strong();
        }
        let response = ui.selectable_label(current, text).on_hover_text(match at {
            Some(at) => format!(
                "{:.0} s ago. Click to go to the state after this step.",
                (now - at).max(0.0)
            ),
            None => "Click to go back to how the session began.".to_owned(),
        });
        rows.rects.push((position, response.rect));
        if response.clicked() && !current {
            rows.clicked = Some(position);
        }
        if current {
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.weak("You are here");
            });
        }
    }
    rows
}

impl FarisApp {
    /// The editable state as it is now.
    pub(crate) fn edit_state(&self, ctx: &egui::Context) -> EditState {
        let scale = self
            .edits
            .pending_scale
            .unwrap_or_else(|| ctx.zoom_factor());
        self.edit_state_scaled(scale)
    }

    pub(crate) fn edit_state_scaled(&self, interface_scale: f32) -> EditState {
        EditState {
            view: self.view_state(),
            selection: self.study.selection.clone(),
            hidden: self.hidden.clone(),
            cutaway: self.cutaway,
            include_evidence: self.file.include_evidence,
            interface_scale,
        }
    }

    /// Put the app into `state`, showing the workflow step `show_step`.
    pub(crate) fn apply_edit_state(
        &mut self,
        ctx: &egui::Context,
        state: &EditState,
        show_step: &str,
    ) {
        self.hidden = state.hidden.clone();
        self.cutaway = state.cutaway;
        self.study.selection = state.selection.clone();
        self.file.include_evidence = state.include_evidence;
        if (self.edit_state(ctx).interface_scale - state.interface_scale).abs() > 0.001 {
            ctx.set_zoom_factor(state.interface_scale);
            self.edits.pending_scale = Some(state.interface_scale);
            #[cfg(not(test))]
            crate::remember_interface_size(Some(state.interface_scale));
        }
        let mut view = state.view.clone();
        view.step = show_step.to_owned();
        self.apply_view(&view);
    }

    pub(crate) fn scripted_run(&self) -> bool {
        self.capture.is_some()
            || self.recorder.is_some()
            || self.interface_check.is_some()
            || self.benchmark.is_some()
    }

    /// Once per frame, after the controls have drawn: turn this frame's change
    /// into a step, extend the open one, or leave the history alone.
    pub(crate) fn track_edits(&mut self, ctx: &egui::Context) {
        if self
            .edits
            .pending_scale
            .is_some_and(|pending| (ctx.zoom_factor() - pending).abs() < 0.001)
        {
            self.edits.pending_scale = None;
        }
        let state = self.edit_state(ctx);
        if self.edits.rebase {
            self.edits.rebase = false;
            self.edits.history.rebase(state);
            return;
        }
        if self.tour.active || self.scripted_run() {
            // The tour and scripted runs move the view themselves; they are not edits.
            self.edits.history.adopt(state);
            return;
        }
        let now = now_unix();
        let running = self.transport.is_running();
        if running && !self.edits.was_running {
            self.edits.history.note(
                NoteKind::RunStarted,
                format!(
                    "A transport run started (undo {CANNOT_UNDO}: it stops the run but never rewinds it)"
                ),
                now,
            );
        }
        self.edits.was_running = running;
        let gesture = ctx.input(|i| i.pointer.any_down() || i.pointer.any_released())
            || ctx.text_edit_focused();
        self.edits.history.observe(&state, gesture, now);
        if let Some(notice) = self.edits.history.take_cap_notice() {
            self.message = notice;
        }
    }

    /// Tell the history an outside action happened (a save, an export).
    pub(crate) fn note_outside_action(&mut self, kind: NoteKind, text: &str) {
        self.edits.history.note(kind, text, now_unix());
    }

    pub(crate) fn undo(&mut self, ctx: &egui::Context) {
        let moved = self.edits.history.undo(now_unix());
        self.finish_move(ctx, moved, "Undid");
    }

    pub(crate) fn redo(&mut self, ctx: &egui::Context) {
        let moved = self.edits.history.redo(now_unix());
        self.finish_move(ctx, moved, "Redid");
    }

    pub(crate) fn jump_history(&mut self, ctx: &egui::Context, position: usize) {
        let moved = self.edits.history.jump_to(position, now_unix());
        let verb = match &moved {
            Some(m) if m.to < m.from => "Went back",
            _ => "Went forward",
        };
        self.finish_move(ctx, moved, verb);
    }

    fn finish_move(&mut self, ctx: &egui::Context, moved: Option<Move>, verb: &str) {
        let Some(moved) = moved else {
            self.message = "There is nothing to undo or redo in that direction.".into();
            return;
        };
        self.apply_edit_state(ctx, &moved.state, &moved.show_step);
        // The state is the stored one; take whatever the app now shows as the head
        // so a rounding difference can never become a phantom step.
        let shown = self.edit_state(ctx);
        self.edits.history.adopt(shown);
        let mut message = match moved.labels.as_slice() {
            [one] => format!("{verb}: {one}."),
            [first, .., _] if moved.to < moved.from => {
                format!(
                    "{verb} {} steps, back to before: {}.",
                    moved.labels.len(),
                    first
                )
            }
            [first, ..] => format!("{verb} {} steps, to: {}.", moved.labels.len(), first),
            [] => format!("{verb}."),
        };
        for note in &moved.crossed {
            message.push_str(&format!(" Note: {}.", note.text));
            if note.kind == NoteKind::RunStarted && self.transport.is_running() {
                self.transport.cancel_run();
                message.push_str(" The run was cancelled; it is not rewound.");
            }
        }
        self.message = message;
        ctx.request_repaint();
    }

    /// The Edit menu: undo, redo and the history window.
    pub(crate) fn edit_menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.menu_button("Edit", |ui| {
            let undo_text = self
                .edits
                .history
                .undo_label()
                .map_or_else(|| "Undo".to_owned(), |l| format!("Undo: {l}"));
            if ui
                .add_enabled(
                    self.edits.history.can_undo(),
                    egui::Button::new(undo_text).shortcut_text("Ctrl+Z"),
                )
                .clicked()
            {
                self.undo(&ctx);
                ui.close();
            }
            let redo_text = self
                .edits
                .history
                .redo_label()
                .map_or_else(|| "Redo".to_owned(), |l| format!("Redo: {l}"));
            if ui
                .add_enabled(
                    self.edits.history.can_redo(),
                    egui::Button::new(redo_text).shortcut_text("Ctrl+Shift+Z"),
                )
                .clicked()
            {
                self.redo(&ctx);
                ui.close();
            }
            ui.separator();
            if ui.button("Edit history…").clicked() {
                self.edits.window_open = true;
                ui.close();
            }
            ui.weak(format!(
                "Saving, exporting and starting a run {CANNOT_UNDO}."
            ));
        })
        .response
        .on_hover_text("Undo, redo and the list of changes made in this session");
    }

    /// The list of every undoable step; a click goes to that state (UX-025).
    pub(crate) fn edit_history_window(&mut self, ctx: &egui::Context) -> Rows {
        let mut rows = Rows::default();
        if !self.edits.window_open {
            return rows;
        }
        let mut open = true;
        let (can_undo, can_redo) = (self.edits.history.can_undo(), self.edits.history.can_redo());
        let (mut undo, mut redo) = (false, false);
        egui::Window::new("Edit history")
            .open(&mut open)
            .default_width(380.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    undo = ui.add_enabled(can_undo, egui::Button::new("Undo")).clicked();
                    redo = ui.add_enabled(can_redo, egui::Button::new("Redo")).clicked();
                });
                let (steps, bytes) = self.edits.history.limits();
                ui.weak(format!(
                    "Every change in this session is listed, newest first. Click one to go to the state after it. The list keeps up to {steps} steps or {} MB; past that the oldest step is dropped and you are told. {} step{} dropped so far.",
                    bytes / (1024 * 1024),
                    self.edits.history.dropped(),
                    if self.edits.history.dropped() == 1 { "" } else { "s" },
                ));
                ui.weak(format!(
                    "Saving, exporting and starting a run {CANNOT_UNDO}; they appear in the list as notes."
                ));
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .auto_shrink([false, true])
                    .id_salt("edit-history-list")
                    .show(ui, |ui| {
                        rows = history_rows(ui, &self.edits.history, now_unix());
                    });
                ui.separator();
                ui.collapsing("Settings changes (last 100)", |ui| {
                    if self.edits.history.settings_log().next().is_none() {
                        ui.weak("No setting has been changed in this session.");
                    }
                    egui::Grid::new("settings-activity").striped(true).show(ui, |ui| {
                        for entry in self.edits.history.settings_log().collect::<Vec<_>>().into_iter().rev() {
                            ui.label(clock(entry.at_unix));
                            ui.label(entry.key);
                            ui.label(format!("{} → {}", entry.old, entry.new));
                            ui.label(if entry.by_undo {
                                format!("{} (by undo or redo)", entry.layer)
                            } else {
                                entry.layer.to_owned()
                            });
                            ui.end_row();
                        }
                    });
                });
            });
        self.edits.window_open = open;
        if undo {
            self.undo(ctx);
        }
        if redo {
            self.redo(ctx);
        }
        if let Some(position) = rows.clicked {
            self.jump_history(ctx, position);
        }
        rows
    }
}

/// A short phrase for the file menu and buttons: what is about to happen cannot be undone.
pub fn cannot_undo_hint(effect: &str) -> String {
    format!("{effect} This {CANNOT_UNDO}: Undo changes the view, never a file or a run.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SessionInputs, build_session, commands};
    use eframe::egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput};
    use sha2::{Digest, Sha256};
    use std::path::PathBuf;

    const ASSUMPTIONS: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scenarios/arc-inspired/demo-operating-assumptions.json"
    );

    fn base() -> EditState {
        EditState::placeholder()
    }

    /// A hash of the edits, not the workflow step.
    fn state_hash(state: &EditState) -> String {
        let mut state = state.clone();
        state.view.step = String::new();
        format!("{:x}", Sha256::digest(serde_json::to_vec(&state).unwrap()))
    }

    fn year(state: &EditState, value: f64) -> EditState {
        let mut next = state.clone();
        next.view.year = value;
        next
    }

    fn rng(seed: u64) -> impl FnMut() -> u64 {
        let mut x = seed | 1;
        move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        }
    }

    /// One random user edit of the state.
    fn random_edit(state: &EditState, next: &mut impl FnMut() -> u64) -> EditState {
        let mut s = state.clone();
        match next() % 9 {
            0 => s.cutaway = !s.cutaway,
            1 => s.include_evidence = !s.include_evidence,
            2 => s.view.year = (next() % 300) as f64 / 10.0,
            3 => {
                let id = format!("part-{}", next() % 4);
                if !s.hidden.remove(&id) {
                    s.hidden.insert(id);
                }
            }
            4 => s.view.allocation = format!("alloc-{}", next() % 3),
            5 => {
                s.view.arrangement = if s.view.arrangement == "port" {
                    "control"
                } else {
                    "port"
                }
                .into()
            }
            6 => {
                s.view.field_view =
                    ["materials", "component-flux", "nuclear-heating"][(next() % 3) as usize].into()
            }
            7 => s.selection.minimum_tbr = format!("1.{}", next() % 20),
            _ => s.interface_scale = [1.0, 1.25, 1.5, 1.75, 2.0][(next() % 5) as usize],
        }
        s
    }

    // Verifies: UX-020, GEO-062
    #[test]
    fn a_thousand_random_edit_sequences_undo_to_the_start_hash_and_redo_to_the_end_hash() {
        let mut next = rng(0x9E37_79B9_7F4A_7C15);
        let mut longest = 0;
        for _ in 0..1000 {
            let start = base();
            let mut history = History::new(start.clone());
            let mut current = start.clone();
            let mut clock = 0.0;
            let mut hashes = vec![state_hash(&start)];
            for _ in 0..(1 + next() % 40) {
                let edited = random_edit(&current, &mut next);
                clock += 5.0;
                match history.observe(&edited, false, clock) {
                    Observed::Pushed => hashes.push(state_hash(&edited)),
                    Observed::Nothing => assert!(edited.same_edit(&current)),
                    other => panic!("unexpected {other:?}"),
                }
                current = edited;
            }
            longest = longest.max(history.len());
            assert_eq!(history.len() + 1, hashes.len());
            let end = state_hash(&current);
            // Undo one step at a time: each state is the one recorded before the step.
            for expected in hashes.iter().rev().skip(1) {
                let moved = history.undo(clock).expect("a step to undo");
                assert_eq!(state_hash(&moved.state), *expected);
            }
            assert!(!history.can_undo());
            assert_eq!(state_hash(history.head()), state_hash(&start));
            while history.redo(clock).is_some() {}
            assert_eq!(state_hash(history.head()), end);
        }
        assert!(longest >= 20, "sequences were too short ({longest})");
    }

    // Verifies: GEO-062, UX-021
    #[test]
    fn a_history_of_ten_thousand_steps_is_kept_and_the_next_one_drops_the_oldest_and_says_so() {
        let mut history = History::new(base());
        let mut state = base();
        for n in 0..MAX_STEPS {
            state = year(&state, n as f64 + 1.0);
            assert_eq!(history.observe(&state, false, n as f64), Observed::Pushed);
        }
        assert_eq!(history.len(), MAX_STEPS);
        assert_eq!(history.dropped(), 0);
        assert!(history.take_cap_notice().is_none());
        // Undo all the way and forward again: every state is exact.
        let top = state_hash(history.head());
        history.jump_to(0, 0.0).unwrap();
        assert_eq!(history.head().view.year, 0.0);
        history.jump_to(MAX_STEPS, 0.0).unwrap();
        assert_eq!(state_hash(history.head()), top);
        // One more step drops the oldest, once, with a notice that states the limit.
        state = year(&state, 99_999.0);
        history.observe(&state, false, 20_000.0);
        assert_eq!(history.len(), MAX_STEPS);
        assert_eq!(history.dropped(), 1);
        let notice = history.take_cap_notice().expect("the user is told");
        assert!(
            notice.contains("10000 steps")
                && notice.contains("500 MB")
                && notice.contains("oldest 1 step"),
            "{notice}"
        );
        assert!(history.take_cap_notice().is_none(), "told once");
        history.jump_to(0, 0.0).unwrap();
        assert_eq!(
            history.head().view.year,
            1.0,
            "the oldest remaining state is the dropped step's"
        );
    }

    // Verifies: UX-021
    #[test]
    fn a_byte_cap_drops_the_oldest_steps_first_and_keeps_the_newest() {
        let one = base().approx_bytes();
        // Room for the start state and about four steps.
        let mut history = History::with_limits(base(), MAX_STEPS, one * 5 + 200);
        let mut state = base();
        for n in 0..50 {
            state = year(&state, n as f64 + 1.0);
            history.observe(&state, false, n as f64);
        }
        assert!(
            history.len() >= 3 && history.len() <= 6,
            "{} steps kept",
            history.len()
        );
        assert!(
            history.bytes() <= one * 6 + 200,
            "{} bytes",
            history.bytes()
        );
        assert_eq!(history.dropped() + history.len(), 50);
        assert_eq!(history.head().view.year, 50.0);
        assert!(history.take_cap_notice().unwrap().contains("oldest"));
    }

    // Verifies: UX-022
    #[test]
    fn a_drag_a_typed_value_and_a_click_are_one_step_each_and_hovering_makes_none() {
        let mut history = History::new(base());
        let mut state = base();
        let mut now = 0.0;
        // Hover, selection and idle frames: the state does not change, so no step.
        for _ in 0..30 {
            now += 0.016;
            assert_eq!(history.observe(&state, false, now), Observed::Nothing);
        }
        assert_eq!(history.len(), 0);
        // A slider drag: 40 frames with the pointer down, then a frame without a change.
        for n in 1..=40 {
            now += 0.016;
            state = year(&state, n as f64 * 0.5);
            history.observe(&state, true, now);
        }
        now += 0.016;
        history.observe(&state, false, now);
        assert_eq!(history.len(), 1, "the drag is one step");
        assert_eq!(history.undo_label(), Some("Timeline moved to year 20.0"));
        // A typed value: several keystrokes while the field is focused.
        for n in 1..=5 {
            now += 0.1;
            state.selection.minimum_tbr = format!("1.{n}");
            history.observe(&state, true, now);
        }
        now += 0.1;
        history.observe(&state, false, now);
        assert_eq!(history.len(), 2, "the typed value is one step");
        // A click on a checkbox: one change on the frame of the release.
        now += 3.0;
        state.cutaway = !state.cutaway;
        history.observe(&state, true, now);
        now += 0.016;
        history.observe(&state, false, now);
        assert_eq!(history.len(), 3);
        // A second click soon after is another step, not a continuation.
        now += 0.2;
        state.cutaway = !state.cutaway;
        history.observe(&state, true, now);
        now += 0.016;
        history.observe(&state, false, now);
        assert_eq!(history.len(), 4);
        // A drag that comes back to where it began leaves no step behind.
        let before = history.len();
        now += 3.0;
        let start_year = state.view.year;
        state = year(&state, start_year + 5.0);
        history.observe(&state, true, now);
        now += 0.016;
        state = year(&state, start_year);
        assert_eq!(history.observe(&state, true, now), Observed::Cancelled);
        assert_eq!(history.len(), before);
    }

    #[test]
    fn a_new_edit_after_an_undo_discards_the_redo_steps() {
        let mut history = History::new(base());
        let mut state = base();
        for n in 1..=3 {
            state = year(&state, n as f64);
            history.observe(&state, false, n as f64 * 10.0);
        }
        history.undo(100.0).unwrap();
        history.undo(100.0).unwrap();
        assert!(history.can_redo());
        let edited = year(history.head(), 9.0);
        history.observe(&edited, false, 200.0);
        assert!(!history.can_redo());
        assert_eq!(history.len(), 2);
        assert_eq!(history.head().view.year, 9.0);
    }

    // Verifies: CFG-025
    #[test]
    fn settings_changes_are_listed_with_time_key_old_new_and_layer_and_undo_restores_them() {
        let mut history = History::new(base());
        let mut state = base();
        state.interface_scale = 1.5;
        history.observe(&state, false, 100.0);
        state.include_evidence = true;
        history.observe(&state, false, 200.0);
        let log: Vec<_> = history.settings_log().cloned().collect();
        assert_eq!(log.len(), 2);
        assert_eq!(
            (
                log[0].key,
                log[0].old.as_str(),
                log[0].new.as_str(),
                log[0].layer
            ),
            ("ui.scale", "1", "1.5", "user file")
        );
        assert_eq!(
            (
                log[1].key,
                log[1].old.as_str(),
                log[1].new.as_str(),
                log[1].layer
            ),
            ("study.include_evidence", "off", "on", "study file")
        );
        assert_eq!((log[0].at_unix, log[1].at_unix), (100.0, 200.0));
        // Undo restores the exact earlier values and is itself listed.
        let moved = history.jump_to(0, 300.0).unwrap();
        assert_eq!(moved.state, base());
        assert_eq!(moved.state.interface_scale, 1.0);
        let log: Vec<_> = history.settings_log().cloned().collect();
        assert_eq!(log.len(), 4);
        assert!(log[2].by_undo && log[3].by_undo);
        assert_eq!((log[2].old.as_str(), log[2].new.as_str()), ("1.5", "1"));
        assert_eq!((log[3].old.as_str(), log[3].new.as_str()), ("on", "off"));
        // Only the last 100 are kept.
        let mut state = base();
        for n in 0..150 {
            state.interface_scale = if n % 2 == 0 { 1.25 } else { 1.0 };
            history.observe(&state, false, 1000.0 + n as f64 * 5.0);
        }
        assert_eq!(history.settings_log().count(), SETTINGS_LOG_LIMIT);
    }

    // Verifies: UX-024
    #[test]
    fn undo_passes_notes_about_things_it_cannot_take_back_and_says_so() {
        let mut history = History::new(base());
        let mut state = base();
        state = year(&state, 1.0);
        history.observe(&state, false, 10.0);
        history.note(NoteKind::Saved, "saved", 11.0);
        state = year(&state, 2.0);
        history.observe(&state, false, 20.0);
        history.note(NoteKind::RunStarted, "run started", 21.0);
        // Undoing the step before the run crosses the run's note; the earlier step crosses the save.
        let moved = history.undo(30.0).unwrap();
        assert_eq!(
            moved.crossed.iter().map(|n| n.kind).collect::<Vec<_>>(),
            [NoteKind::RunStarted]
        );
        let moved = history.undo(31.0).unwrap();
        assert_eq!(
            moved.crossed.iter().map(|n| n.kind).collect::<Vec<_>>(),
            [NoteKind::Saved]
        );
        // Redoing crosses nothing, and a jump all the way back reports every note it passed.
        history.jump_to(2, 32.0).unwrap();
        let moved = history.jump_to(0, 33.0).unwrap();
        assert_eq!(moved.crossed.len(), 2);
        // The wording that goes with each action says it cannot be undone.
        assert!(cannot_undo_hint("Writes the file.").contains(CANNOT_UNDO));
        assert!(CANNOT_UNDO.contains("cannot be undone"));
    }

    #[test]
    fn labels_name_the_change_in_plain_words() {
        let a = base();
        let mut b = a.clone();
        b.cutaway = false;
        assert_eq!(describe(&a, &b), "Cutaway turned off");
        b.view.allocation = "breeder-emphasis".into();
        assert_eq!(
            describe(&a, &b),
            "Allocation changed to breeder-emphasis (and 1 other change)"
        );
        let mut c = a.clone();
        c.hidden.insert("blanket".into());
        assert_eq!(describe(&a, &c), "Component blanket hidden");
        assert_eq!(describe(&c, &a), "Component blanket shown");
        let mut d = a.clone();
        d.interface_scale = 1.5;
        assert_eq!(describe(&a, &d), "Interface size set to 150 %");
        let mut e = a.clone();
        e.selection.electricity = true;
        assert_eq!(describe(&a, &e), "Core study electricity turned on");
        assert_eq!(clock(3_725.0), "01:02:05 UTC");
    }

    // ---- the app --------------------------------------------------------------------

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

    struct Frames {
        ctx: egui::Context,
    }

    impl Frames {
        fn new() -> Self {
            Self {
                ctx: egui::Context::default(),
            }
        }

        /// One frame: the keyboard commands, then the end-of-frame tracking, as `ui` does.
        fn run(&self, app: &mut FarisApp, events: Vec<Event>) {
            let raw = RawInput {
                events,
                ..Default::default()
            };
            self.ctx
                .run_ui(raw, |ui| {
                    let ctx = ui.ctx().clone();
                    app.keyboard(&ctx);
                    app.track_edits(&ctx);
                })
                .drop_without_applying_deltas();
        }

        fn key(&self, app: &mut FarisApp, modifiers: Modifiers, key: Key) {
            self.run(
                app,
                vec![Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
            );
        }

        /// The first frame takes the opened state as the start of the history.
        fn start(&self, app: &mut FarisApp) {
            self.run(app, vec![]);
            assert_eq!(app.edits.history.len(), 0);
        }

        fn command(&self, app: &mut FarisApp, id: &str) {
            let command = commands::REGISTRY.iter().find(|c| c.id == id).unwrap();
            app.run_command(&self.ctx, command);
            app.rebuild().unwrap();
            self.run(app, vec![]);
        }
    }

    fn screen(events: Vec<Event>) -> RawInput {
        RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            events,
            ..Default::default()
        }
    }

    /// A hash of the drawn scene: every component's vertices, in order.
    fn scene_hash(app: &FarisApp) -> String {
        let mut hasher = Sha256::new();
        for mesh in &app.meshes {
            hasher.update(mesh.id.as_bytes());
            for v in mesh.vertices.iter() {
                for x in v.position.iter().chain(v.normal.iter()) {
                    hasher.update(x.to_bits().to_le_bytes());
                }
            }
        }
        format!("{:x}", hasher.finalize())
    }

    // Verifies: UX-020, UX-025
    #[test]
    fn ctrl_z_ctrl_shift_z_and_ctrl_y_undo_and_redo_in_every_step_of_the_workflow() {
        let frames = Frames::new();
        let mut app = app();
        frames.start(&mut app);
        for step in crate::Step::ALL {
            app.step = step;
            let before = app.cutaway;
            frames.command(&mut app, "view.cutaway");
            assert_eq!(app.cutaway, !before);
            frames.key(&mut app, Modifiers::COMMAND, Key::Z);
            assert_eq!(app.cutaway, before, "Ctrl+Z in {step:?}");
            assert!(
                app.message.starts_with("Undid: Cutaway turned"),
                "{}",
                app.message
            );
            frames.key(&mut app, Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
            assert_eq!(app.cutaway, !before, "Ctrl+Shift+Z in {step:?}");
            frames.key(&mut app, Modifiers::COMMAND, Key::Z);
            frames.key(&mut app, Modifiers::COMMAND, Key::Y);
            assert_eq!(app.cutaway, !before, "Ctrl+Y in {step:?}");
            // Back to the start for the next step of the workflow.
            frames.key(&mut app, Modifiers::COMMAND, Key::Z);
            assert_eq!(app.cutaway, before);
            app.edits.history.rebase(app.edit_state(&frames.ctx));
        }
        // With nothing left to undo, the key says why instead of doing nothing.
        frames.key(&mut app, Modifiers::COMMAND, Key::Z);
        assert!(app.message.contains("nothing to undo"), "{}", app.message);
    }

    // Verifies: UX-022
    #[test]
    fn hovering_selecting_and_moving_the_camera_make_no_steps_but_a_drag_makes_one() {
        let frames = Frames::new();
        let mut app = app();
        frames.start(&mut app);
        frames.run(&mut app, vec![Event::PointerMoved(Pos2::new(40.0, 40.0))]);
        frames.command(&mut app, "component.next");
        frames.command(&mut app, "step.simulate");
        app.camera.yaw += 0.3;
        frames.run(&mut app, vec![]);
        assert_eq!(
            app.edits.history.len(),
            0,
            "no step from hover, selection, step or camera"
        );
        // Press, drag the timeline year for several frames, release.
        let press = |pressed| Event::PointerButton {
            pos: Pos2::new(40.0, 40.0),
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        frames.run(&mut app, vec![press(true)]);
        for n in 1..=12 {
            app.year = n as f64;
            frames.run(&mut app, vec![]);
        }
        frames.run(&mut app, vec![press(false)]);
        frames.run(&mut app, vec![]);
        assert_eq!(app.edits.history.len(), 1, "the whole drag is one step");
        frames.key(&mut app, Modifiers::COMMAND, Key::Z);
        assert_eq!(app.year, 0.0, "one undo returns the whole drag");
    }

    // Verifies: UX-020, UX-025, CFG-025
    #[test]
    fn undo_restores_allocation_what_if_hidden_evidence_and_size_exactly() {
        let frames = Frames::new();
        let mut app = app();
        frames.start(&mut app);
        let start = app.edit_state(&frames.ctx);
        let start_scene = scene_hash(&app);
        frames.command(&mut app, "allocation.next");
        frames.command(&mut app, "component.visibility");
        frames.command(&mut app, "file.include_evidence");
        frames.command(&mut app, "interface.larger");
        frames.command(&mut app, "timeline.end");
        app.history.assumptions.as_mut().unwrap().recovery_fraction -= 0.05;
        frames.run(&mut app, vec![]);
        assert_eq!(
            app.edits.history.len(),
            6,
            "{:?}",
            app.edits
                .history
                .entries()
                .map(|e| e.label.clone())
                .collect::<Vec<_>>()
        );
        let edited = app.edit_state(&frames.ctx);
        let edited_scene = scene_hash(&app);
        assert_ne!(edited_scene, start_scene);
        // Back to the start in one jump, then forward again.
        app.jump_history(&frames.ctx, 0);
        assert!(app.edit_state(&frames.ctx).same_edit(&start));
        assert_eq!(scene_hash(&app), start_scene);
        assert_eq!(app.edits.history.len(), 6, "the jump itself is not a step");
        app.jump_history(&frames.ctx, 6);
        assert!(app.edit_state(&frames.ctx).same_edit(&edited));
        assert_eq!(scene_hash(&app), edited_scene);
        assert!((frames.ctx.zoom_factor() - 1.25).abs() < 1e-6);
        // The settings activity list names the size and evidence changes.
        let keys: Vec<_> = app.edits.history.settings_log().map(|e| e.key).collect();
        assert!(
            keys.contains(&"ui.scale") && keys.contains(&"study.include_evidence"),
            "{keys:?}"
        );
    }

    // Verifies: GEO-062, UX-020
    #[test]
    fn a_thousand_command_steps_undo_and_redo_to_identical_scene_hashes() {
        let frames = Frames::new();
        let mut app = app();
        frames.start(&mut app);
        let ids = [
            "view.cutaway",
            "component.visibility",
            "allocation.next",
            "interface.larger",
            "component.next",
            "interface.smaller",
            "file.include_evidence",
        ];
        let mut next = rng(42);
        let mut scenes = vec![scene_hash(&app)];
        let mut hashes = vec![state_hash(&app.edit_state(&frames.ctx))];
        let mut states = vec![app.edit_state(&frames.ctx)];
        let mut steps = 0;
        while steps < 1000 {
            let id = ids[(next() % ids.len() as u64) as usize];
            let before = app.edits.history.len();
            frames.command(&mut app, id);
            if app.edits.history.len() > before {
                steps += 1;
                scenes.push(scene_hash(&app));
                hashes.push(state_hash(&app.edit_state(&frames.ctx)));
                states.push(app.edit_state(&frames.ctx));
            }
        }
        assert_eq!(app.edits.history.len(), 1000);
        // Each undo returns the state and scene before the step; each redo returns them again.
        for k in (0..1000).rev() {
            app.undo(&frames.ctx);
            assert_eq!(
                state_hash(&app.edit_state(&frames.ctx)),
                hashes[k],
                "undo to {k}: differs in {:?}",
                changed_fields(&app.edit_state(&frames.ctx), &states[k])
            );
            assert_eq!(scene_hash(&app), scenes[k], "scene after undo to {k}");
        }
        assert!(!app.edits.history.can_undo());
        for (k, scene) in scenes.iter().enumerate().skip(1) {
            app.redo(&frames.ctx);
            assert_eq!(&scene_hash(&app), scene, "scene after redo to {k}");
        }
        assert_eq!(state_hash(&app.edit_state(&frames.ctx)), hashes[1000]);
    }

    // Verifies: UX-020
    #[test]
    fn every_mutating_command_in_the_registry_is_covered_by_undo() {
        // The edit state is compared every frame, so a command is undoable when what it
        // changes is in `EditState`. This lists the commands that change the study and the
        // test runs each one and checks undo takes it back. A mutating command added to
        // the registry without being listed here fails the first assertion.
        const COVERED: &[&str] = &[
            "file.include_evidence",
            "allocation.next",
            "arrangement.swap",
        ];
        let frames = Frames::new();
        for command in commands::REGISTRY.iter().filter(|c| c.mutating) {
            assert!(
                COVERED.contains(&command.id),
                "{} changes the study but is not covered by an undo test",
                command.id
            );
        }
        for id in COVERED {
            let mut app = app();
            frames.start(&mut app);
            let command = commands::REGISTRY.iter().find(|c| c.id == *id).unwrap();
            if command.blocked.is_some_and(|b| b(&app).is_some()) {
                // The matched port or no-port arrangement is not loaded in the bundled demo.
                continue;
            }
            let before = app.edit_state(&frames.ctx);
            frames.command(&mut app, id);
            assert!(
                !app.edit_state(&frames.ctx).same_edit(&before),
                "{id} changed nothing"
            );
            assert_eq!(app.edits.history.len(), 1, "{id} is one step");
            app.undo(&frames.ctx);
            assert!(
                app.edit_state(&frames.ctx).same_edit(&before),
                "{id} was not undone"
            );
        }
    }

    // Verifies: UX-025
    #[test]
    fn the_history_window_lists_every_step_with_a_label_and_a_click_goes_there() {
        let frames = Frames::new();
        let mut app = app();
        frames.start(&mut app);
        frames.command(&mut app, "view.cutaway");
        frames.command(&mut app, "timeline.end");
        frames.command(&mut app, "component.visibility");
        assert_eq!(app.edits.history.len(), 3);
        app.edits.window_open = true;
        let mut rows = Rows::default();
        // Windows settle over a few frames before their widgets take clicks.
        for _ in 0..4 {
            frames
                .ctx
                .run_ui(screen(Vec::new()), |ui| {
                    let ctx = ui.ctx().clone();
                    rows = app.edit_history_window(&ctx);
                })
                .drop_without_applying_deltas();
        }
        assert_eq!(rows.rects.len(), 4, "start of the session and three steps");
        let (_, start_row) = rows.rects.iter().find(|(p, _)| *p == 0).copied().unwrap();
        let click = start_row.center();
        for pressed in [None, Some(true), Some(false)] {
            let mut events = vec![Event::PointerMoved(click)];
            if let Some(pressed) = pressed {
                events.push(Event::PointerButton {
                    pos: click,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                });
            }
            frames
                .ctx
                .run_ui(screen(events), |ui| {
                    let ctx = ui.ctx().clone();
                    app.edit_history_window(&ctx);
                })
                .drop_without_applying_deltas();
        }
        assert_eq!(
            app.edits.history.cursor(),
            0,
            "a click on the first row goes back to the start"
        );
        assert!(app.message.contains("Went back"), "{}", app.message);
        assert_eq!(app.year, 0.0);
        assert!(app.hidden.is_empty());
        // The labels are plain words.
        let labels: Vec<_> = app
            .edits
            .history
            .entries()
            .map(|e| e.label.as_str())
            .collect();
        assert_eq!(labels[1], "Timeline moved to year 30.0");
        assert!(labels.iter().all(|l| !l.is_empty()));
    }

    // Verifies: UX-023
    #[test]
    fn undo_and_redo_apply_within_a_tenth_of_a_second() {
        let frames = Frames::new();
        let mut app = app();
        frames.start(&mut app);
        for id in [
            "allocation.next",
            "view.cutaway",
            "timeline.end",
            "component.visibility",
        ] {
            frames.command(&mut app, id);
        }
        let mut worst: f64 = 0.0;
        for _ in 0..3 {
            for forward in [false, true] {
                let started = std::time::Instant::now();
                for _ in 0..4 {
                    if forward {
                        app.redo(&frames.ctx);
                    } else {
                        app.undo(&frames.ctx);
                    }
                }
                worst = worst.max(started.elapsed().as_secs_f64() / 4.0);
            }
        }
        eprintln!(
            "slowest undo or redo: {:.1} ms (debug build)",
            worst * 1000.0
        );
        assert!(worst <= 0.1, "an undo took {worst:.3} s");
    }

    // Verifies: UX-024
    #[test]
    fn the_buttons_that_write_files_or_start_runs_say_they_cannot_be_undone() {
        // The hover text of Save, Export and Run transport is built with `cannot_undo_hint`.
        for (name, source) in [
            ("study_file.rs", include_str!("study_file.rs")),
            ("export_panel.rs", include_str!("export_panel.rs")),
            ("transport_panel.rs", include_str!("transport_panel.rs")),
        ] {
            assert!(
                source.contains("cannot_undo_hint("),
                "{name} has no 'cannot be undone' hint"
            );
        }
        let hint = cannot_undo_hint("Starts a transport run on this computer.");
        assert!(
            hint.contains("cannot be undone") && hint.contains("never a file or a run"),
            "{hint}"
        );
    }

    // Verifies: UX-023
    #[test]
    fn undoing_a_what_if_edit_hands_the_history_panel_the_earlier_assumptions() {
        // The panel's result cache is keyed by assumptions and tested in history_panel; this
        // checks that undo and redo put exactly those assumptions back.
        let frames = Frames::new();
        let mut app = app();
        frames.start(&mut app);
        let original = app.history.assumptions.clone().unwrap();
        app.history.assumptions.as_mut().unwrap().recovery_fraction -= 0.05;
        frames.run(&mut app, vec![]);
        assert_eq!(app.edits.history.len(), 1);
        assert!(
            app.edits
                .history
                .undo_label()
                .unwrap()
                .starts_with("What-if changed:"),
            "{:?}",
            app.edits.history.undo_label()
        );
        app.undo(&frames.ctx);
        assert_eq!(app.history.assumptions.as_ref(), Some(&original));
        app.redo(&frames.ctx);
        assert_eq!(
            app.history.assumptions.as_ref().unwrap().recovery_fraction,
            original.recovery_fraction - 0.05
        );
    }
}
