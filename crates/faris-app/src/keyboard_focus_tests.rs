// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! Rendered keyboard focus regression for the native app shell (A11Y-001).
//!
//! This drives the same `eframe::App::ui` implementation used by the desktop
//! window, with egui input and its rendered AccessKit tree. It does not model
//! focus order in a duplicate widget tree.

use super::{FarisApp, SessionInputs, Step, build_session};
use eframe::egui::{self, Key, Modifiers};
use std::{collections::BTreeMap, path::PathBuf};

struct RenderedApp {
    app: FarisApp,
    ctx: egui::Context,
    frame: eframe::Frame,
    nodes: BTreeMap<egui::accesskit::NodeId, (String, String, String)>,
    time: f64,
}

impl RenderedApp {
    fn new() -> Self {
        let session = build_session(SessionInputs::empty(PathBuf::from("runs")))
            .expect("default app session builds");
        let app = FarisApp::new(
            session.manifest,
            None,
            None,
            PathBuf::from("runs"),
            session.transport,
            session.control,
            session.history,
        )
        .expect("default app scene builds");
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Self {
            app,
            ctx,
            frame: eframe::Frame::_new_kittest(),
            nodes: BTreeMap::new(),
            time: 0.0,
        }
    }

    fn frame(&mut self, events: Vec<egui::Event>) -> Option<String> {
        self.time += 1.0 / 30.0;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 960.0));
        let output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                time: Some(self.time),
                focused: true,
                events,
                ..Default::default()
            },
            |ui| <FarisApp as eframe::App>::ui(&mut self.app, ui, &mut self.frame),
        );
        if let Some(update) = &output.platform_output.accesskit_update {
            for (id, node) in &update.nodes {
                self.nodes.insert(
                    *id,
                    (
                        format!("{:?}", node.role()),
                        node.label().unwrap_or_default().to_owned(),
                        format!("{:?}", node.bounds()),
                    ),
                );
            }
        }
        let focus = self.ctx.memory(|memory| memory.focused()).map(|id| {
            let id = id.accesskit_id();
            self.nodes.get(&id).map_or_else(
                || format!("{id:?} (no AccessKit node in rendered tree)"),
                |(role, label, bounds)| {
                    format!("{id:?} role={role} label={label:?} bounds={bounds}")
                },
            )
        });
        output.drop_without_applying_deltas();
        focus
    }

    fn key(&mut self, key: Key, modifiers: Modifiers) -> Option<String> {
        self.frame(vec![egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        }])
    }

    fn tab_walk(&mut self, count: usize) -> Vec<String> {
        (0..count)
            .filter_map(|_| self.key(Key::Tab, Modifiers::NONE))
            .collect()
    }
}

// Verifies: A11Y-001
#[test]
fn native_app_shell_steps_palette_viewport_and_timeline_are_keyboard_reachable() {
    let mut rendered = RenderedApp::new();
    // Let egui settle the full shell and its first AccessKit tree.
    for _ in 0..3 {
        rendered.frame(Vec::new());
    }

    // Check keyboard step dispatch before the focus walk reaches a text field;
    // app shortcuts intentionally defer to an actively edited text field.
    for (key, step) in [
        (Key::Num1, Step::Design),
        (Key::Num2, Step::Simulate),
        (Key::Num3, Step::Operate),
        (Key::Num4, Step::Compare),
        (Key::Num5, Step::Evidence),
    ] {
        rendered.key(key, Modifiers::NONE);
        assert_eq!(rendered.app.step, step, "step shortcut {key:?}");
    }

    let mut walk = rendered.tab_walk(80);
    for step in Step::ALL {
        // Select each real app content view to exercise its rendered widgets.
        // Step shortcut behavior itself is asserted immediately above.
        rendered.app.step = step;
        rendered.frame(Vec::new());
        walk.extend(rendered.tab_walk(20));
    }

    // Open the actual command palette through its app shortcut. It must take
    // text focus, and Escape must return to the app without leaving the popup.
    rendered.key(Key::K, Modifiers::COMMAND);
    rendered.frame(Vec::new());
    assert!(rendered.app.palette.open, "Ctrl/Cmd+K opens the palette");
    assert!(
        rendered.ctx.text_edit_focused(),
        "palette query takes focus"
    );
    walk.extend(rendered.tab_walk(8));
    rendered.key(Key::Escape, Modifiers::NONE);
    rendered.frame(Vec::new());
    assert!(!rendered.app.palette.open, "Escape closes the palette");

    let focused = walk.join("\n");
    assert!(
        walk.len() >= 40,
        "the rendered app focus walk visited too few widgets: {focused}"
    );
    assert!(
        focused.contains("3D viewport"),
        "the app's rendered viewport is not in the keyboard focus walk: {focused}"
    );
    assert!(
        focused.contains("calendar years") || focused.contains("years"),
        "the operating timeline preview slider is not in the keyboard focus walk: {focused}"
    );
    assert!(
        !focused.contains("no AccessKit node"),
        "a focused app widget is missing from its rendered accessibility tree: {focused}"
    );
    assert!(
        walk.iter().all(|sample| sample.contains("bounds=Some(")),
        "a focused app widget has no rendered bounds: {focused}"
    );
}
