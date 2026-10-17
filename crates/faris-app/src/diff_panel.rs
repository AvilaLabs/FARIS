//! The Compare versions window (GEO-067, COL-023): the semantic diff of two study files or two
//! model files, the same report `faris diff` writes.
//!
//! No comparison logic is here. The report is `faris_engine::semantic_diff` (model files) or
//! `faris_study::diff_study_files` (study files); this file reads the two paths on a worker thread,
//! draws what changed (components first, then the same text `faris diff` prints) and says why
//! when it cannot compare. The browser has no files to read, so the window says so there.

use crate::badge::{self, Kind};
use eframe::egui;
use faris_engine::semantic_diff::{DiffReport, diff_models, render_text};
use faris_study::{StudyDiffOptions, diff_study_files};
use serde_json::Value;
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};

/// Largest model file read, the same bound `faris diff` uses.
const MAX_MODEL_BYTES: u64 = faris_model::transport::MAX_ARTIFACT_BYTES as u64;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    A,
    B,
}

struct Shown {
    outcome: Result<DiffReport, String>,
}

/// The words of one component row.
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentRow {
    pub name: String,
    /// `added`, `removed`, `moved`, `material`, `thickness`, `volume`, comma separated.
    pub what: String,
    pub material: String,
    pub volume: String,
}

fn is_study(path: &Path) -> bool {
    if path.extension().is_some_and(|e| e == "faris") {
        return true;
    }
    let mut magic = [0u8; 2];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .is_ok()
        && &magic == b"PK"
}

fn read_model(path: &Path) -> Result<Value, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(MAX_MODEL_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_MODEL_BYTES {
        return Err(format!("{} exceeds the 16 MiB limit", path.display()));
    }
    serde_json::from_slice(&bytes).map_err(|e| {
        format!(
            "{} is neither a .faris study nor a JSON model file ({e}). Next step: choose two .faris files, or two scenario, physics, assumptions or design files",
            path.display()
        )
    })
}

/// Compare two paths: two studies, or two model documents. A study against a model file is
/// refused with the way out, as `faris diff` does.
pub fn compare_paths(a: &Path, b: &Path) -> Result<DiffReport, String> {
    match (is_study(a), is_study(b)) {
        (true, true) => diff_study_files(
            a,
            b,
            StudyDiffOptions {
                include_result_values: false,
            },
        )
        .map_err(|e| format!("{e}")),
        (false, false) => Ok(diff_models(
            &a.display().to_string(),
            &read_model(a)?,
            &b.display().to_string(),
            &read_model(b)?,
        )),
        _ => Err("One path is a .faris study and the other is a model file; a study can only be compared with a study. Next step: choose two of the same kind.".into()),
    }
}

/// The headline over the lists.
pub fn summary_line(report: &DiffReport) -> String {
    if report.identical {
        return "No differences: the two sides are the same.".into();
    }
    let s = &report.summary;
    let cost = s.highest_cost.map_or("none", |c| c.words());
    format!(
        "{} inputs, {} evidence, {} results and {} presentation items differ; getting the results again costs at most: {cost}.",
        s.inputs_changed, s.evidence_changed, s.results_changed, s.presentation_changed
    )
}

/// One row per component that was added, removed, moved or changed in material, thickness or
/// volume.
pub fn component_rows(report: &DiffReport) -> Vec<ComponentRow> {
    report
        .components
        .iter()
        .map(|c| ComponentRow {
            name: format!("{} / {}", c.variant, c.component),
            what: c.changes.join(", "),
            material: match (&c.material_old, &c.material_new) {
                (Some(a), Some(b)) if a != b => format!("{a} -> {b}"),
                (None, Some(b)) => b.clone(),
                _ => String::new(),
            },
            volume: match (c.volume_old_m3, c.volume_new_m3, c.volume_delta_m3) {
                (Some(a), Some(b), Some(d)) => format!("{a:.6} -> {b:.6} m³ ({d:+.6})"),
                (None, Some(b), _) => format!("{b:.6} m³"),
                (Some(a), None, _) => format!("was {a:.6} m³"),
                _ => String::new(),
            },
        })
        .collect()
}

#[derive(Default)]
pub struct DiffPanel {
    pub open: bool,
    a: String,
    b: String,
    pick: Option<(Side, Receiver<Option<PathBuf>>)>,
    job: Option<Receiver<Result<DiffReport, String>>>,
    shown: Option<Shown>,
}

impl DiffPanel {
    /// The floating window. `current` is the study file the app has open, offered as side A.
    pub fn window(&mut self, ctx: &egui::Context, current: Option<&Path>) {
        self.poll();
        if !self.open {
            return;
        }
        let mut open = self.open;
        egui::Window::new("Compare versions")
            .open(&mut open)
            .default_size([820.0, 640.0])
            .show(ctx, |ui| self.view(ui, current));
        self.open = open;
    }

    fn poll(&mut self) {
        if let Some((side, receiver)) = &self.pick {
            let side = *side;
            match receiver.try_recv() {
                Ok(picked) => {
                    self.pick = None;
                    if let Some(path) = picked {
                        *self.text_mut(side) = path.display().to_string();
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.pick = None,
            }
        }
        if let Some(receiver) = &self.job {
            match receiver.try_recv() {
                Ok(outcome) => {
                    self.job = None;
                    self.shown = Some(Shown { outcome });
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.job = None;
                    self.shown = Some(Shown {
                        outcome: Err(
                            "The comparison stopped unexpectedly. Next step: press Compare again."
                                .into(),
                        ),
                    });
                }
            }
        }
    }

    fn text_mut(&mut self, side: Side) -> &mut String {
        match side {
            Side::A => &mut self.a,
            Side::B => &mut self.b,
        }
    }

    fn pick_path(&mut self, ctx: &egui::Context, side: Side) {
        if self.pick.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let context = ctx.clone();
        let spawned = crate::thread::Builder::new()
            .name("faris-diff-dialog".into())
            .spawn(move || {
                let picked = crate::dialog::FileDialog::new()
                    .set_title(match side {
                        Side::A => "Choose version A",
                        Side::B => "Choose version B",
                    })
                    .add_filter("Studies and models", &["faris", "json"])
                    .pick_file();
                let _ = sender.send(picked);
                context.request_repaint();
            });
        if spawned.is_ok() {
            self.pick = Some((side, receiver));
        }
    }

    fn start(&mut self, ctx: &egui::Context) {
        let (a, b) = (PathBuf::from(self.a.trim()), PathBuf::from(self.b.trim()));
        let (sender, receiver) = mpsc::channel();
        let context = ctx.clone();
        let spawned = crate::thread::Builder::new()
            .name("faris-diff".into())
            .spawn(move || {
                let _ = sender.send(compare_paths(&a, &b));
                context.request_repaint();
            });
        match spawned {
            Ok(_) => {
                self.shown = None;
                self.job = Some(receiver);
            }
            Err(error) => {
                self.shown = Some(Shown {
                    outcome: Err(format!("The comparison could not start: {error}")),
                });
            }
        }
    }

    fn view(&mut self, ui: &mut egui::Ui, current: Option<&Path>) {
        ui.weak(
            "What differs between two versions of a study or two model files (scenario, physics \
             case, operating assumptions, design): changed parameters with old and new value, \
             components added, removed or moved, material and volume changes, and which results \
             each change invalidates. The same report `faris diff` writes.",
        );
        if crate::browser::IS_BROWSER {
            crate::browser::desktop_only_note(ui, "Comparing two files");
            return;
        }
        let ctx = ui.ctx().clone();
        for side in [Side::A, Side::B] {
            ui.horizontal(|ui| {
                ui.label(match side {
                    Side::A => "Version A",
                    Side::B => "Version B",
                });
                ui.add(
                    egui::TextEdit::singleline(self.text_mut(side))
                        .desired_width(420.0)
                        .hint_text("path of a .faris study or a JSON model file"),
                );
                if ui.button("Choose\u{2026}").clicked() {
                    self.pick_path(&ctx, side);
                }
                if side == Side::A
                    && let Some(path) = current
                    && ui
                        .button("Use the open study")
                        .on_hover_text(path.display().to_string())
                        .clicked()
                {
                    self.a = path.display().to_string();
                }
            });
        }
        let running = self.job.is_some();
        let ready = !self.a.trim().is_empty() && !self.b.trim().is_empty();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(ready && !running, egui::Button::new("Compare"))
                .on_disabled_hover_text("Choose both versions first.")
                .clicked()
            {
                self.start(&ctx);
            }
            if running {
                ui.spinner();
                ui.weak("Comparing\u{2026}");
            }
        });
        let Some(shown) = &self.shown else {
            return;
        };
        ui.separator();
        match &shown.outcome {
            Err(error) => {
                badge::kind_badge(ui, Kind::NotEvaluated, error);
                ui.label(error);
            }
            Ok(report) => report_ui(ui, report),
        }
    }
}

fn report_ui(ui: &mut egui::Ui, report: &DiffReport) {
    let kind = if report.identical {
        Kind::Checked
    } else {
        Kind::Conditional
    };
    ui.horizontal_wrapped(|ui| {
        badge::badge(
            ui,
            kind,
            if report.identical {
                "identical"
            } else {
                "differs"
            },
            &format!("Compared as: {}", report.compared),
        );
        ui.label(summary_line(report));
    });
    let rows = component_rows(report);
    if !rows.is_empty() {
        ui.add_space(4.0);
        ui.strong(format!("Components ({})", rows.len()));
        egui::Grid::new("diff-components")
            .striped(true)
            .num_columns(4)
            .show(ui, |ui| {
                for label in ["Component", "Change", "Material", "Volume"] {
                    ui.strong(label);
                }
                ui.end_row();
                for row in &rows {
                    ui.label(&row.name);
                    ui.label(&row.what);
                    ui.label(&row.material);
                    ui.label(&row.volume);
                    ui.end_row();
                }
            });
    }
    ui.add_space(4.0);
    ui.collapsing("Full report (the text `faris diff` prints)", |ui| {
        let mut text = render_text(report);
        egui::ScrollArea::vertical()
            .max_height(360.0)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY)
                        .interactive(true),
                );
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SCENARIO: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scenarios/arc-inspired/cold-reference.scenario.json"
    );

    fn write(dir: &Path, name: &str, value: &Value) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
        path
    }

    fn scenario() -> Value {
        serde_json::from_slice(&std::fs::read(SCENARIO).unwrap()).unwrap()
    }

    // Verifies: GEO-067, COL-023
    #[test]
    fn the_window_lists_what_the_engine_reports_for_two_model_files() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.json", &scenario());
        let mut edited = scenario();
        edited["variants"][0]["layers"][1]["thickness_m"] = json!(0.6);
        edited["variants"][0]["layers"][1]["material_id"] = json!("swapped");
        let b = write(dir.path(), "b.json", &edited);
        let report = compare_paths(&a, &b).unwrap();
        // The same report the engine (and so `faris diff`) gives for the two documents.
        let direct = diff_models(
            &a.display().to_string(),
            &scenario(),
            &b.display().to_string(),
            &edited,
        );
        assert_eq!(render_text(&report), render_text(&direct));
        assert_eq!(report.summary, direct.summary);
        assert!(!report.identical);
        assert!(
            report
                .inputs
                .iter()
                .any(|c| c.path == "variants[id=reference].layers[id=blanket].thickness_m")
        );
        let rows = component_rows(&report);
        let blanket = rows.iter().find(|r| r.name.ends_with("/ blanket")).unwrap();
        assert!(blanket.what.contains("material"), "{blanket:?}");
        assert!(blanket.what.contains("thickness"), "{blanket:?}");
        assert!(blanket.material.ends_with("-> swapped"), "{blanket:?}");
        assert!(blanket.volume.contains("m³"), "{blanket:?}");
        assert!(summary_line(&report).contains("inputs"));
    }

    #[test]
    fn identical_files_say_so_and_a_mixed_pair_says_what_to_do() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.json", &scenario());
        let b = write(dir.path(), "b.json", &scenario());
        let report = compare_paths(&a, &b).unwrap();
        assert!(report.identical);
        assert!(summary_line(&report).starts_with("No differences"));
        assert!(component_rows(&report).is_empty());
        let study = dir.path().join("x.faris");
        std::fs::write(&study, b"PK").unwrap();
        let error = compare_paths(&a, &study).unwrap_err();
        assert!(error.contains("Next step"), "{error}");
        let missing = compare_paths(&a, &dir.path().join("nope.json")).unwrap_err();
        assert!(missing.contains("nope.json"), "{missing}");
    }

    #[test]
    fn the_window_draws_empty_and_with_a_report() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.json", &scenario());
        let mut edited = scenario();
        edited["variants"][0]["layers"][1]["thickness_m"] = json!(0.6);
        let b = write(dir.path(), "b.json", &edited);
        let ctx = egui::Context::default();
        let mut panel = DiffPanel {
            open: true,
            ..DiffPanel::default()
        };
        let frame = |panel: &mut DiffPanel| {
            ctx.run_ui(egui::RawInput::default(), |ui| panel.view(ui, Some(&a)))
                .drop_without_applying_deltas();
        };
        frame(&mut panel);
        panel.shown = Some(Shown {
            outcome: compare_paths(&a, &b),
        });
        frame(&mut panel);
        panel.shown = Some(Shown {
            outcome: Err("broken".into()),
        });
        frame(&mut panel);
    }
}
