//! Bounded development checks delivered through eframe's real raw-input hook.
//! This exercises egui controls in the native app, not OS input routing.

use eframe::egui;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs::File, io::Write, path::Path, time::Instant};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    schema_version: String,
    steps: Vec<Step>,
}

#[derive(Deserialize)]
struct Step {
    time_s: f64,
    #[serde(flatten)]
    action: Action,
}

#[derive(Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Move {
        x: f32,
        y: f32,
        #[serde(default)]
        shift: bool,
    },
    Button {
        x: f32,
        y: f32,
        pressed: bool,
        #[serde(default)]
        shift: bool,
    },
    Scroll {
        x: f32,
        y: f32,
    },
    Key {
        key: String,
        pressed: bool,
        #[serde(default)]
        control: bool,
        #[serde(default)]
        shift: bool,
    },
    Text {
        text: String,
    },
    Checkpoint {
        name: String,
        #[serde(default)]
        expected: BTreeMap<String, Value>,
    },
}

pub struct InterfaceCheck {
    plan: Plan,
    output: File,
    launched: Instant,
    armed: Option<Instant>,
    next: usize,
    pending: Vec<(String, BTreeMap<String, Value>)>,
    observations: Vec<Value>,
    finished: bool,
    modifiers: egui::Modifiers,
    plan_sha256: String,
    delivered: Vec<Value>,
}

impl InterfaceCheck {
    pub fn load(plan: &Path, output: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let metadata = std::fs::metadata(plan)?;
        if !metadata.is_file() || metadata.len() > 64 * 1024 {
            return Err("interface check requires a regular plan no larger than 64 KiB".into());
        }
        let plan_bytes = std::fs::read(plan)?;
        let plan_sha256 = format!("{:x}", Sha256::digest(&plan_bytes));
        let plan: Plan = serde_json::from_slice(&plan_bytes)?;
        if plan.schema_version != "faris-interface-check-plan/v0.1"
            || plan.steps.is_empty()
            || plan.steps.len() > 512
        {
            return Err("unsupported interface check plan or step count outside 1..=512".into());
        }
        let mut prior = 0.0;
        let mut checkpoints = 0;
        for step in &plan.steps {
            if !step.time_s.is_finite() || !(prior..=120.0).contains(&step.time_s) {
                return Err("interface check times must increase within 0..=120 seconds".into());
            }
            prior = step.time_s;
            match &step.action {
                Action::Move { x, y, .. }
                | Action::Button { x, y, .. }
                | Action::Scroll { x, y } => {
                    if [x, y].iter().any(|v| !v.is_finite() || v.abs() > 16_384.0) {
                        return Err("interface check coordinates are nonfinite or excessive".into());
                    }
                }
                Action::Key { key, .. } if egui::Key::from_name(key).is_none() => {
                    return Err(format!("unknown interface check key: {key}").into());
                }
                Action::Text { text } if text.len() > 4096 => {
                    return Err("interface check text exceeds 4 KiB".into());
                }
                Action::Checkpoint { name, expected } => {
                    checkpoints += 1;
                    if name.is_empty()
                        || name.len() > 128
                        || expected.len() > 32
                        || expected
                            .keys()
                            .any(|p| !p.starts_with('/') || p.len() > 128)
                    {
                        return Err(
                            "interface checkpoint has an invalid name or expectation".into()
                        );
                    }
                }
                _ => {}
            }
        }
        if !(1..=32).contains(&checkpoints) {
            return Err("interface checks require 1..=32 checkpoints".into());
        }
        Ok(Self {
            plan,
            output: File::options().create_new(true).write(true).open(output)?,
            launched: Instant::now(),
            armed: None,
            next: 0,
            pending: Vec::new(),
            observations: Vec::new(),
            finished: false,
            modifiers: egui::Modifiers::default(),
            plan_sha256,
            delivered: Vec::new(),
        })
    }

    pub fn finished(&self) -> bool {
        self.finished
    }

    pub fn inject(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        if self.finished {
            return;
        }
        ctx.request_repaint();
        let Some(armed) = self.armed else {
            return;
        };
        // These checks deliberately bypass OS routing; real egui widgets still
        // consume the same raw event types supplied by eframe/winit.
        input.focused = true;
        while let Some(step) = self.plan.steps.get(self.next) {
            if step.time_s > armed.elapsed().as_secs_f64() {
                break;
            }
            self.delivered.push(serde_json::json!({
                "step_index":self.next,"target_time_s":step.time_s,
                "delivered_time_s":armed.elapsed().as_secs_f64(),
            }));
            self.next += 1;
            match &step.action {
                Action::Move { x, y, shift } => {
                    self.modifiers.shift = *shift;
                    input
                        .events
                        .push(egui::Event::ModifiersChanged(self.modifiers));
                    input
                        .events
                        .push(egui::Event::PointerMoved(egui::pos2(*x, *y)));
                }
                Action::Button {
                    x,
                    y,
                    pressed,
                    shift,
                } => {
                    self.modifiers.shift = *shift;
                    input
                        .events
                        .push(egui::Event::ModifiersChanged(self.modifiers));
                    input
                        .events
                        .push(egui::Event::PointerMoved(egui::pos2(*x, *y)));
                    input.events.push(egui::Event::PointerButton {
                        pos: egui::pos2(*x, *y),
                        button: egui::PointerButton::Primary,
                        pressed: *pressed,
                        modifiers: self.modifiers,
                    });
                }
                Action::Scroll { x, y } => input.events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(*x, *y),
                    phase: egui::TouchPhase::Move,
                    modifiers: self.modifiers,
                }),
                Action::Key {
                    key,
                    pressed,
                    control,
                    shift,
                } => {
                    self.modifiers = egui::Modifiers {
                        ctrl: *control,
                        command: *control,
                        shift: *shift,
                        ..Default::default()
                    };
                    input
                        .events
                        .push(egui::Event::ModifiersChanged(self.modifiers));
                    input.events.push(egui::Event::Key {
                        key: egui::Key::from_name(key).expect("validated key"),
                        physical_key: None,
                        pressed: *pressed,
                        repeat: false,
                        modifiers: self.modifiers,
                    });
                }
                Action::Text { text } => input.events.push(egui::Event::Text(text.clone())),
                Action::Checkpoint { name, expected } => {
                    self.pending.push((name.clone(), expected.clone()));
                    // Observe the completed UI pass before injecting later actions.
                    break;
                }
            }
        }
    }

    pub fn observe(&mut self, state: Value, ready: bool) -> Result<(), std::io::Error> {
        if self.finished {
            return Ok(());
        }
        if ready && self.armed.is_none() {
            self.armed = Some(Instant::now());
        }
        for (name, expected) in self.pending.drain(..) {
            let failures: Vec<_> = expected
                .iter()
                .filter(|(pointer, wanted)| state.pointer(pointer) != Some(*wanted))
                .map(|(pointer, wanted)| {
                    serde_json::json!({
                        "pointer":pointer, "expected":wanted, "observed":state.pointer(pointer),
                    })
                })
                .collect();
            self.observations.push(serde_json::json!({
                "name":name, "passed":failures.is_empty(), "failures":failures, "state":state,
                "observed_time_s":self.armed.map(|a|a.elapsed().as_secs_f64()),
            }));
        }
        let timed_out = self.launched.elapsed().as_secs() >= 150;
        if self.next == self.plan.steps.len() || timed_out {
            let passed = !timed_out
                && !self.observations.is_empty()
                && self.observations.iter().all(|o| o["passed"] == true);
            let report = serde_json::json!({
                "schema_version":"faris-interface-check-report/v0.1", "passed":passed,
                "timed_out":timed_out, "steps_delivered":self.next, "observations":self.observations,
                "input_plan_sha256":self.plan_sha256, "application_version":env!("CARGO_PKG_VERSION"),
                "input_delivery_times":self.delivered,
                "scope":"Synthetic pointer/keyboard events through eframe raw_input_hook in the native egui/wgpu app. Actual controls and external workers execute. OS pointer/keyboard routing is not verified by this check.",
            });
            self.output
                .write_all(&serde_json::to_vec_pretty(&report)?)?;
            self.output.write_all(b"\n")?;
            self.output.sync_all()?;
            self.finished = true;
            eprintln!(
                "FARIS interface check: {}",
                if passed { "PASS" } else { "FAIL" }
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_plan(directory: &Path, steps: Value) -> std::path::PathBuf {
        let path = directory.join("plan.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "schema_version":"faris-interface-check-plan/v0.1", "steps":steps,
            }))
            .unwrap(),
        )
        .unwrap();
        path
    }

    #[test]
    fn invalid_key_is_refused_before_output_creation() {
        let directory = tempfile::tempdir().unwrap();
        let plan = write_plan(
            directory.path(),
            serde_json::json!([
                {"time_s":0.0,"type":"key","key":"UnknownKey","pressed":true},
                {"time_s":0.0,"type":"checkpoint","name":"result"},
            ]),
        );
        let output = directory.path().join("report.json");
        assert!(InterfaceCheck::load(&plan, &output).is_err());
        assert!(!output.exists());
    }

    #[test]
    fn prior_report_is_never_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let plan = write_plan(
            directory.path(),
            serde_json::json!([
                {"time_s":0.0,"type":"checkpoint","name":"result"},
            ]),
        );
        let output = directory.path().join("report.json");
        std::fs::write(&output, b"original receipt").unwrap();
        assert!(InterfaceCheck::load(&plan, &output).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), b"original receipt");
    }

    #[test]
    fn delivered_checkpoint_failure_cannot_be_reported_as_success() {
        let directory = tempfile::tempdir().unwrap();
        let plan = write_plan(
            directory.path(),
            serde_json::json!([
                {"time_s":0.0,"type":"checkpoint","name":"result","expected":{"/loaded":true}},
            ]),
        );
        let output = directory.path().join("report.json");
        let mut check = InterfaceCheck::load(&plan, &output).unwrap();
        check.observe(Value::Null, true).unwrap();
        check.inject(&egui::Context::default(), &mut egui::RawInput::default());
        check
            .observe(serde_json::json!({"loaded":false}), true)
            .unwrap();
        assert!(check.finished());
        let report: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(report["passed"], false);
        assert_eq!(report["observations"][0]["failures"][0]["observed"], false);
    }
}
