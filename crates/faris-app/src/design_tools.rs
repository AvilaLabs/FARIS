//! The Tools section the Design check and Design flux windows share: the six tool paths set once
//! (kept in the settings file, `settings`), the environment variables that win over them, and
//! "Find tools", which looks in the usual places (`tool_finder`) and offers what it found.
//!
//! Both windows hold the same `SharedTools`, so a path set in one is the path in the other. A
//! value the user typed is never replaced without the user ticking the replacement. The search
//! runs on a worker thread; nothing here owns physics.

use crate::{
    badge::Kind,
    design_shared::{Run, path_row_enabled},
    settings::{self, Settings, Tool},
    tool_finder::{self, Outcome, Proposal, SearchRoots},
};
use eframe::egui;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    rc::Rc,
    sync::mpsc::{self, Receiver, TryRecvError},
};

/// The one set of tool values both windows read and write.
pub type SharedTools = Rc<RefCell<ToolsModel>>;

/// The two windows whose last used folder is remembered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    DesignCheck,
    DesignFlux,
}

/// Where a tool's value comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// The named environment variable, set before FARIS started; it wins for the session.
    Environment(&'static str),
    /// The settings file (or typed this session, which is saved at once).
    Saved,
    Empty,
}

#[derive(Clone, Default)]
pub struct FindProgress {
    pub line: String,
}

/// A finished search as the window shows it, with the user's tick for each proposal.
struct Shown {
    outcome: Outcome,
    /// For each proposal, which offer is ticked.
    choice: Vec<Option<usize>>,
}

pub struct ToolsModel {
    /// `None`: nothing is read or written (tests).
    path: Option<PathBuf>,
    writable: bool,
    settings: Settings,
    env: BTreeMap<Tool, (&'static str, String)>,
    /// Said until the user dismisses it: the settings file was unreadable.
    notice: Option<String>,
    save_error: Option<String>,
    dirty: bool,
    /// Search once on first draw when nothing is stored.
    auto_find: bool,
    find: Run<FindProgress, Outcome>,
    shown: Option<Shown>,
    extra_draft: String,
    pick: Option<(Tool, Receiver<Option<PathBuf>>)>,
}

impl ToolsModel {
    /// The settings of this user, with the process environment.
    pub fn load() -> Self {
        let path = settings::default_path();
        let env = |name: &str| std::env::var_os(name);
        Self::from_parts(path, &env, true)
    }

    /// Reads `path` (when there is one) and the given environment.
    pub fn from_parts(
        path: Option<PathBuf>,
        env: &dyn Fn(&str) -> Option<OsString>,
        auto_find: bool,
    ) -> Self {
        let loaded = path.as_deref().map(settings::load);
        let (settings, notice, writable) = match loaded {
            Some(l) => (l.settings, l.notice, l.writable),
            None => (Settings::default(), None, true),
        };
        let mut from_env = BTreeMap::new();
        for tool in Tool::ALL {
            if let Some(name) = tool.environment()
                && let Some(value) = env(name).filter(|v| !v.is_empty())
            {
                from_env.insert(tool, (name, value.to_string_lossy().into_owned()));
            }
        }
        Self {
            path,
            writable,
            settings,
            env: from_env,
            notice,
            save_error: None,
            dirty: false,
            auto_find,
            find: Run::default(),
            shown: None,
            extra_draft: String::new(),
            pick: None,
        }
    }

    /// No file, no environment, no automatic search: for tests and for panels built alone.
    pub fn in_memory() -> Self {
        Self::from_parts(None, &|_| None, false)
    }

    pub fn shared(self) -> SharedTools {
        Rc::new(RefCell::new(self))
    }

    /// The value to use: the environment's if it names one, else the saved one.
    pub fn value(&self, tool: Tool) -> String {
        match self.env.get(&tool) {
            Some((_, value)) => value.clone(),
            None => self.settings.raw(tool).to_string(),
        }
    }

    pub fn source(&self, tool: Tool) -> Source {
        if let Some((name, _)) = self.env.get(&tool) {
            Source::Environment(name)
        } else if self.settings.tool(tool).is_some() {
            Source::Saved
        } else {
            Source::Empty
        }
    }

    /// Sets what the user typed or accepted. A value from the environment cannot be changed here.
    pub fn set(&mut self, tool: Tool, text: &str) {
        if self.env.contains_key(&tool) || self.settings.raw(tool) == text {
            return;
        }
        self.settings.set_tool(tool, text);
        self.dirty = true;
    }

    pub fn last_folder(&self, window: Window) -> Option<PathBuf> {
        let folders = &self.settings.last_folders;
        let text = match window {
            Window::DesignCheck => &folders.design_check,
            Window::DesignFlux => &folders.design_flux,
        };
        text.as_deref().map(PathBuf::from).filter(|p| p.is_dir())
    }

    /// Remembers the folder a window last picked in: the path itself if it is a folder, else the
    /// folder it is in.
    pub fn remember_folder(&mut self, window: Window, picked: &Path) {
        let folder = if picked.is_dir() {
            picked
        } else {
            crate::design_shared::beside(picked)
        };
        let text = folder.display().to_string();
        let folders = &mut self.settings.last_folders;
        let slot = match window {
            Window::DesignCheck => &mut folders.design_check,
            Window::DesignFlux => &mut folders.design_flux,
        };
        if slot.as_deref() != Some(text.as_str()) {
            *slot = Some(text);
            self.dirty = true;
        }
    }

    #[cfg(test)]
    pub fn extra_search_folders(&self) -> &[String] {
        &self.settings.extra_search_folders
    }

    pub fn add_search_folder(&mut self, text: &str) {
        let text = text.trim();
        if !text.is_empty() && !self.settings.extra_search_folders.iter().any(|f| f == text) {
            self.settings.extra_search_folders.push(text.to_string());
            self.dirty = true;
        }
    }

    pub fn remove_search_folder(&mut self, index: usize) {
        if index < self.settings.extra_search_folders.len() {
            self.settings.extra_search_folders.remove(index);
            self.dirty = true;
        }
    }

    #[cfg(test)]
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    /// True while no tool has a value from anywhere: a first run.
    fn nothing_known(&self) -> bool {
        self.env.is_empty() && !self.settings.has_any_tool()
    }

    /// True while Find tools is looking (a capture waits for it).
    pub fn is_finding(&self) -> bool {
        self.find.is_running()
    }

    /// Writes the settings if they changed and nobody is typing in a field.
    pub fn flush(&mut self, typing: bool) {
        if !self.dirty || typing {
            return;
        }
        self.dirty = false;
        let Some(path) = self.path.clone() else {
            return;
        };
        if !self.writable {
            return;
        }
        self.save_error = settings::save(&path, &self.settings).err().map(|e| {
            format!(
                "Your choices could not be saved: {e}. They are used until FARIS closes. \
                 Next step: check that the folder is writable."
            )
        });
    }

    fn start_find(&mut self, ctx: &egui::Context) {
        if self.find.is_running() {
            return;
        }
        self.shown = None;
        let roots = SearchRoots::from_environment(&self.settings.extra_search_folders);
        self.find
            .start("faris-find-tools", ctx, move |cancel, sink| {
                tool_finder::find_tools(&roots, &cancel, &|line| {
                    sink.update(|p| p.line = line);
                })
            });
    }

    /// Takes a finished search and picks each proposal's starting tick.
    fn poll_find(&mut self) {
        if let Some(outcome) = self.find.poll() {
            let choice = outcome
                .proposals
                .iter()
                .map(|p| self.default_choice(p))
                .collect();
            self.shown = Some(Shown { outcome, choice });
        }
    }

    /// Fill an empty field with an unambiguous find; leave everything else for the user to tick.
    fn default_choice(&self, proposal: &Proposal) -> Option<usize> {
        let empty = self.value(proposal.tool).trim().is_empty();
        (empty && !proposal.ambiguous && !self.env.contains_key(&proposal.tool)).then_some(0)
    }

    fn apply_ticked(&mut self) {
        let Some(shown) = &self.shown else { return };
        let picks: Vec<(Tool, String)> = shown
            .outcome
            .proposals
            .iter()
            .zip(&shown.choice)
            .filter_map(|(p, c)| c.map(|i| (p.tool, p.offers[i].value.clone())))
            .collect();
        for (tool, value) in picks {
            self.set(tool, &value);
        }
        self.shown = None;
    }

    // ---------------------------------------------------------------- drawing --

    /// The Tools section. `needs_attention` opens it when a tool the window needs is missing.
    pub fn ui(&mut self, ui: &mut egui::Ui, needs_attention: bool) {
        let ctx = ui.ctx().clone();
        self.poll_find();
        self.poll_pick();
        if self.auto_find {
            self.auto_find = false;
            if self.nothing_known() {
                self.start_find(&ctx);
            }
        }
        let mut browse = None;
        egui::CollapsingHeader::new(egui::RichText::new("Tools").heading())
            .default_open(needs_attention || self.nothing_known())
            .show(ui, |ui| {
                ui.weak(
                    "The programs FARIS uses for CAD and for OpenMC. Set once: both design windows \
                     share these paths and they are kept for the next session. Find tools looks in \
                     the usual places and shows what it found before changing anything.",
                );
                self.notices(ui);
                self.grid(ui, &mut browse);
                ui.add_space(4.0);
                self.find_controls(ui, &ctx);
                self.found(ui);
                self.search_folders(ui);
            });
        if let Some(tool) = browse {
            self.browse(&ctx, tool);
        }
        self.flush(ctx.egui_wants_keyboard_input());
    }

    fn notices(&mut self, ui: &mut egui::Ui) {
        let mut dismiss = false;
        if let Some(notice) = &self.notice {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(Kind::Partial.color(), notice);
                dismiss = ui.button("Got it").clicked();
            });
        }
        if dismiss {
            self.notice = None;
        }
        if let Some(error) = &self.save_error {
            ui.colored_label(Kind::Failed.color(), error);
        }
    }

    fn grid(&mut self, ui: &mut egui::Ui, browse: &mut Option<Tool>) {
        egui::Grid::new("design-tools-grid")
            .num_columns(3)
            .spacing([8.0, 2.0])
            .show(ui, |ui| {
                for tool in Tool::ALL {
                    let source = self.source(tool);
                    let from_env = matches!(source, Source::Environment(_));
                    let mut text = self.value(tool);
                    let note = match &source {
                        Source::Environment(name) => Some(format!(
                            "From the environment variable {name}, set before FARIS started, so it wins \
                             this session. Next step: unset {name} to choose a path here."
                        )),
                        _ => None,
                    };
                    let hint = if tool.is_folder() { "folder" } else { "file" };
                    let label = match tool {
                        Tool::WindowPython | Tool::OpenmcPackage => {
                            format!("{} (optional)", tool.label())
                        }
                        _ => tool.label().to_string(),
                    };
                    if path_row_enabled(
                        ui,
                        &label,
                        hint,
                        &mut text,
                        note.as_deref().map(|n| (Kind::Partial, n)),
                        !from_env,
                    ) {
                        *browse = Some(tool);
                    }
                    self.set(tool, &text);
                }
            });
        let where_kept = self.path.as_ref().map_or_else(
            || "This session only; no settings folder is available.".to_string(),
            |p| format!("Kept in {}", p.display()),
        );
        ui.weak(where_kept);
    }

    fn find_controls(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if let Some(view) = self.find.running_view() {
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.label(&view.progress.line);
                if ui
                    .button("Cancel")
                    .on_hover_text("Stops the search. Nothing is changed.")
                    .clicked()
                {
                    self.find.cancel();
                }
            });
            ctx.request_repaint_after(crate::time::Duration::from_millis(250));
            return;
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .button("Find tools")
                .on_hover_text(
                    "Looks for Python environments (on PATH, conda and mamba environments, \
                     ~/.venvs and your search folders) and asks each one, once, what it has. It \
                     installs and changes nothing, and shows what it found before you accept.",
                )
                .clicked()
            {
                self.start_find(ctx);
            }
            if let Some(error) = self.find.failure() {
                ui.colored_label(
                    Kind::Failed.color(),
                    format!(
                        "The search failed: {error}. Next step: press Find tools to try again."
                    ),
                );
            } else if self.find.was_cancelled() {
                ui.weak("The search was cancelled. Nothing was changed.");
            }
        });
    }

    fn found(&mut self, ui: &mut egui::Ui) {
        // Values the offers are compared with, taken before the results are borrowed.
        let current: BTreeMap<Tool, (String, bool)> = Tool::ALL
            .iter()
            .map(|t| (*t, (self.value(*t), self.env.contains_key(t))))
            .collect();
        let mut apply = false;
        let mut dismiss = false;
        let Some(shown) = &mut self.shown else { return };
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.strong("Found");
            let looked = shown.outcome.looked_at.len();
            if shown.outcome.proposals.is_empty() {
                ui.label(format!(
                    "Nothing usable was found among the {looked} Python(s) looked at."
                ));
            }
            for (proposal, choice) in shown.outcome.proposals.iter().zip(&mut shown.choice) {
                let (now, from_env) = &current[&proposal.tool];
                offer_ui(ui, proposal, choice, now, *from_env);
            }
            for missing in &shown.outcome.not_found {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "{}: not found. {} Next step: {}",
                            missing.tool.label(),
                            missing.why,
                            missing.next_step
                        ))
                        .small()
                        .color(Kind::NotEvaluated.color()),
                    )
                    .wrap(),
                );
            }
            if shown.outcome.skipped > 0 {
                ui.weak(format!(
                    "{} more Python(s) were not asked: the search stops at {} to stay quick.",
                    shown.outcome.skipped,
                    tool_finder::MAX_CANDIDATES
                ));
            }
            egui::CollapsingHeader::new(format!("Pythons looked at ({looked})")).show(ui, |ui| {
                for f in &shown.outcome.looked_at {
                    ui.label(f.candidate.python.display().to_string())
                        .on_hover_text(&f.candidate.origin);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!(
                                "    {} \u{2013} {}",
                                f.candidate.origin,
                                f.summary()
                            ))
                            .small(),
                        )
                        .wrap(),
                    );
                }
            });
            ui.horizontal(|ui| {
                let any = shown.choice.iter().any(Option::is_some);
                apply = ui
                    .add_enabled(any, egui::Button::new("Use the ticked values"))
                    .on_hover_text("Sets the ticked tools and keeps them for the next session.")
                    .clicked();
                dismiss = ui.button("Dismiss").clicked();
            });
        });
        if apply {
            self.apply_ticked();
        } else if dismiss {
            self.shown = None;
        }
    }

    fn search_folders(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Also search these folders").show(ui, |ui| {
            ui.weak(
                "Find tools also looks two levels down in each folder here, for Python \
                     environments, a cross_sections.xml and openmc_plasma_source.",
            );
            let mut remove = None;
            for (i, folder) in self.settings.extra_search_folders.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(folder);
                    if ui.small_button("Remove").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                self.remove_search_folder(i);
            }
            ui.horizontal(|ui| {
                ui.add_sized(
                    [360.0, ui.spacing().interact_size.y],
                    egui::TextEdit::singleline(&mut self.extra_draft)
                        .hint_text("a folder, such as ~/envs or ~/nuclear-data"),
                );
                if ui
                    .add_enabled(
                        !self.extra_draft.trim().is_empty(),
                        egui::Button::new("Add"),
                    )
                    .clicked()
                {
                    let draft = std::mem::take(&mut self.extra_draft);
                    self.add_search_folder(&draft);
                }
            });
        });
    }

    fn browse(&mut self, ctx: &egui::Context, tool: Tool) {
        if self.pick.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let context = ctx.clone();
        let start = crate::design_shared::trimmed(&self.value(tool))
            .map(|p| crate::design_shared::beside(&p).to_path_buf())
            .filter(|p| p.is_dir());
        let spawned = crate::thread::Builder::new()
            .name("faris-tools-dialog".into())
            .spawn(move || {
                let mut dialog =
                    crate::dialog::FileDialog::new().set_title(format!("Choose the {}", tool.label()));
                if let Some(dir) = start {
                    dialog = dialog.set_directory(dir);
                }
                let picked = if tool.is_folder() {
                    dialog.pick_folder()
                } else {
                    dialog.pick_file()
                };
                let _ = sender.send(picked);
                context.request_repaint();
            });
        if spawned.is_ok() {
            self.pick = Some((tool, receiver));
        }
    }

    fn poll_pick(&mut self) {
        let Some((tool, receiver)) = &self.pick else {
            return;
        };
        let tool = *tool;
        match receiver.try_recv() {
            Ok(picked) => {
                self.pick = None;
                if let Some(path) = picked {
                    self.set(tool, &path.display().to_string());
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.pick = None,
        }
    }

    /// Test hook: a finished search to show.
    #[cfg(test)]
    fn show_outcome(&mut self, outcome: Outcome) {
        let choice = outcome
            .proposals
            .iter()
            .map(|p| self.default_choice(p))
            .collect();
        self.shown = Some(Shown { outcome, choice });
    }
}

/// One proposal: what was found and from where, with a tick per offer.
fn offer_ui(
    ui: &mut egui::Ui,
    proposal: &Proposal,
    choice: &mut Option<usize>,
    now: &str,
    from_env: bool,
) {
    let tool = proposal.tool;
    ui.add_space(2.0);
    let now = now.trim();
    if from_env {
        ui.weak(format!(
            "{}: set by the environment variable; not changed.",
            tool.label()
        ));
        *choice = None;
        return;
    }
    if !proposal.ambiguous && proposal.offers[0].value == now {
        ui.weak(format!("{}: already set to what was found.", tool.label()));
        *choice = None;
        return;
    }
    ui.strong(tool.label());
    if proposal.ambiguous {
        ui.weak("Several were found and they are not equal; choose the one you want.");
    }
    if !now.is_empty() {
        ui.colored_label(
            Kind::Partial.color(),
            format!("You already have {now}. Ticking replaces it."),
        );
    }
    for (i, offer) in proposal.offers.iter().enumerate() {
        let mut ticked = *choice == Some(i);
        let text = format!("{}  \u{2013} {}", offer.value, offer.origin);
        let response = ui.checkbox(&mut ticked, text).on_hover_text(&offer.detail);
        if response.changed() {
            *choice = ticked.then_some(i);
        }
        ui.add(
            egui::Label::new(egui::RichText::new(format!("    {}", offer.detail)).small()).wrap(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_finder::{Found, Offer, ProbeReport, Probed};

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| OsString::from(v))
        }
    }

    fn model_at(dir: &Path, env: &[(&str, &str)]) -> ToolsModel {
        ToolsModel::from_parts(
            Some(dir.join("faris").join(settings::FILE_NAME)),
            &env_of(env),
            false,
        )
    }

    #[test]
    fn values_typed_in_one_session_are_there_in_the_next() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = model_at(dir.path(), &[]);
        first.set(Tool::CadPython, "/opt/cad/bin/python");
        first.set(Tool::WindowPython, "/opt/w/bin/python");
        first.remember_folder(Window::DesignCheck, dir.path());
        first.flush(false);
        let second = model_at(dir.path(), &[]);
        assert_eq!(second.value(Tool::CadPython), "/opt/cad/bin/python");
        assert_eq!(second.value(Tool::WindowPython), "/opt/w/bin/python");
        assert_eq!(second.source(Tool::CadPython), Source::Saved);
        assert_eq!(second.source(Tool::CrossSections), Source::Empty);
        assert_eq!(
            second.last_folder(Window::DesignCheck),
            Some(dir.path().to_path_buf())
        );
        assert_eq!(second.last_folder(Window::DesignFlux), None);
    }

    #[test]
    fn nothing_is_written_while_the_user_is_typing() {
        let dir = tempfile::tempdir().unwrap();
        let mut model = model_at(dir.path(), &[]);
        model.set(Tool::CadPython, "/a");
        model.flush(true);
        assert!(!dir.path().join("faris").exists());
        model.flush(false);
        assert!(dir.path().join("faris").join(settings::FILE_NAME).is_file());
    }

    #[test]
    fn the_environment_wins_for_the_session_and_is_never_saved_over() {
        let dir = tempfile::tempdir().unwrap();
        let mut model = model_at(dir.path(), &[]);
        model.set(Tool::CadPython, "/saved/python");
        model.flush(false);
        let mut model = model_at(
            dir.path(),
            &[
                ("FARIS_CAD_PYTHON", "/env/python"),
                ("FARIS_CROSS_SECTIONS", "/env/xs.xml"),
            ],
        );
        assert_eq!(model.value(Tool::CadPython), "/env/python");
        assert_eq!(
            model.source(Tool::CadPython),
            Source::Environment("FARIS_CAD_PYTHON")
        );
        assert_eq!(model.value(Tool::CrossSections), "/env/xs.xml");
        // Editing a value the environment supplies changes nothing.
        model.set(Tool::CadPython, "/typed/python");
        assert_eq!(model.value(Tool::CadPython), "/env/python");
        model.flush(false);
        let later = model_at(dir.path(), &[]);
        assert_eq!(later.value(Tool::CadPython), "/saved/python");
        // A tool with no environment variable is not affected.
        assert_eq!(model.source(Tool::WindowPython), Source::Empty);
    }

    #[test]
    fn an_empty_environment_variable_is_ignored() {
        let model = ToolsModel::from_parts(None, &env_of(&[("FARIS_CAD_PYTHON", "")]), false);
        assert_eq!(model.source(Tool::CadPython), Source::Empty);
    }

    #[test]
    fn a_corrupt_file_gives_one_notice_and_an_empty_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("faris").join(settings::FILE_NAME);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "garbage").unwrap();
        let mut model = ToolsModel::from_parts(Some(path.clone()), &|_| None, false);
        assert!(model.notice().unwrap().contains(".bad"));
        assert_eq!(model.value(Tool::CadPython), "");
        assert!(settings::bad_copy_path(&path).is_file());
        model.set(Tool::CadPython, "/new");
        model.flush(false);
        let again = ToolsModel::from_parts(Some(path), &|_| None, false);
        assert_eq!(again.notice(), None, "the file is good now");
        assert_eq!(again.value(Tool::CadPython), "/new");
    }

    #[test]
    fn a_file_from_a_newer_faris_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(settings::FILE_NAME);
        let original = br#"{"schema": 7, "tools": {}}"#;
        std::fs::write(&path, original).unwrap();
        let mut model = ToolsModel::from_parts(Some(path.clone()), &|_| None, false);
        model.set(Tool::CadPython, "/x");
        model.flush(false);
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn search_folders_are_kept_without_duplicates() {
        let mut model = ToolsModel::in_memory();
        model.add_search_folder("  ~/envs ");
        model.add_search_folder("~/envs");
        model.add_search_folder("   ");
        assert_eq!(model.extra_search_folders(), ["~/envs"]);
        model.remove_search_folder(0);
        assert!(model.extra_search_folders().is_empty());
    }

    #[test]
    fn a_first_run_is_known_and_a_stored_value_is_not() {
        let mut model = ToolsModel::in_memory();
        assert!(model.nothing_known());
        model.set(Tool::CadPython, "/a");
        assert!(!model.nothing_known());
    }

    fn proposal(tool: Tool, values: &[&str], ambiguous: bool) -> Proposal {
        Proposal {
            tool,
            offers: values
                .iter()
                .map(|v| Offer {
                    value: v.to_string(),
                    origin: "environment x".into(),
                    detail: "has things".into(),
                })
                .collect(),
            ambiguous,
        }
    }

    fn outcome(proposals: Vec<Proposal>) -> Outcome {
        Outcome {
            looked_at: vec![Found {
                candidate: tool_finder::Candidate {
                    python: PathBuf::from("/envs/x/bin/python"),
                    origin: "environment x".into(),
                },
                probe: Probed::Answered(Box::new(ProbeReport {
                    cadquery: Some("2.5".into()),
                    ..ProbeReport::default()
                })),
                aiming: None,
            }],
            proposals,
            not_found: vec![tool_finder::NotFound {
                tool: Tool::OpenmcPackage,
                why: "No aiming build.".into(),
                next_step: "nothing to do.".into(),
            }],
            skipped: 0,
        }
    }

    #[test]
    fn an_empty_field_is_filled_by_a_find_but_a_typed_value_is_not_replaced_unasked() {
        let mut model = ToolsModel::in_memory();
        model.set(Tool::OpenmcPython, "/mine/python");
        model.show_outcome(outcome(vec![
            proposal(Tool::CadPython, &["/found/cad"], false),
            proposal(Tool::OpenmcPython, &["/found/omc"], false),
            proposal(Tool::CrossSections, &["/a.xml", "/b.xml"], true),
        ]));
        let ticks = model.shown.as_ref().unwrap().choice.clone();
        assert_eq!(
            ticks,
            vec![Some(0), None, None],
            "only the empty, unambiguous one"
        );
        model.apply_ticked();
        assert_eq!(model.value(Tool::CadPython), "/found/cad");
        assert_eq!(model.value(Tool::OpenmcPython), "/mine/python");
        assert_eq!(model.value(Tool::CrossSections), "");
        assert!(model.shown.is_none());
    }

    #[test]
    fn ticking_a_replacement_replaces_and_the_environment_is_never_offered_a_change() {
        let mut model =
            ToolsModel::from_parts(None, &env_of(&[("FARIS_CAD_PYTHON", "/env/python")]), false);
        model.set(Tool::OpenmcPython, "/mine/python");
        model.show_outcome(outcome(vec![
            proposal(Tool::CadPython, &["/found/cad"], false),
            proposal(Tool::OpenmcPython, &["/found/omc"], false),
        ]));
        assert_eq!(model.shown.as_ref().unwrap().choice, vec![None, None]);
        model.shown.as_mut().unwrap().choice = vec![Some(0), Some(0)];
        model.apply_ticked();
        assert_eq!(model.value(Tool::OpenmcPython), "/found/omc");
        assert_eq!(model.value(Tool::CadPython), "/env/python");
    }

    fn frame(ctx: &egui::Context, model: &mut ToolsModel, attention: bool) {
        ctx.run_ui(egui::RawInput::default(), |ui| model.ui(ui, attention))
            .drop_without_applying_deltas();
    }

    #[test]
    fn the_section_draws_empty_with_values_and_with_results() {
        let ctx = egui::Context::default();
        let mut model = ToolsModel::in_memory();
        frame(&ctx, &mut model, true);
        model.set(Tool::CadPython, "/x/python");
        model.add_search_folder("~/envs");
        model.notice = Some("The settings file could not be read.".into());
        model.save_error = Some("Your choices could not be saved.".into());
        frame(&ctx, &mut model, false);
        model.show_outcome(outcome(vec![
            proposal(Tool::CadPython, &["/found/cad"], false),
            proposal(Tool::CrossSections, &["/a.xml", "/b.xml"], true),
            proposal(Tool::OpenmcPython, &["/mine"], false),
        ]));
        frame(&ctx, &mut model, true);
        model.show_outcome(outcome(Vec::new()));
        frame(&ctx, &mut model, true);
        let mut with_env =
            ToolsModel::from_parts(None, &env_of(&[("FARIS_CAD_PYTHON", "/e")]), false);
        frame(&ctx, &mut with_env, false);
    }

    #[test]
    fn a_search_with_no_interpreters_ends_with_a_result() {
        let dir = tempfile::tempdir().unwrap();
        let roots = SearchRoots {
            home: Some(dir.path().to_path_buf()),
            ..SearchRoots::default()
        };
        let outcome = tool_finder::find_tools(
            &roots,
            &faris_engine::jobs::Cancellation::default(),
            &|_| {},
        )
        .unwrap();
        assert!(outcome.looked_at.is_empty());
        assert!(outcome.proposals.is_empty());
        assert_eq!(outcome.not_found.len(), 6);
    }

    #[test]
    fn a_cancelled_search_is_an_error_and_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = faris_engine::jobs::Cancellation::default();
        cancel.cancel();
        let roots = SearchRoots {
            home: Some(dir.path().to_path_buf()),
            ..SearchRoots::default()
        };
        assert!(tool_finder::find_tools(&roots, &cancel, &|_| {}).is_err());
    }
}
