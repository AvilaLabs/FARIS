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
    /// Press and release a key in one input batch (layout-independent).
    Tap {
        key: String,
    },
    /// Drag from the 3D viewport centre by (dx, dy) points over `duration_s`,
    /// then release and move the pointer away. Orbits the camera; needs no pixels.
    Orbit {
        dx: f32,
        dy: f32,
        duration_s: f64,
    },
    /// Set the history year cursor, optionally moving it linearly to `to`.
    SetYear {
        year: f64,
        #[serde(default)]
        to: Option<f64>,
        #[serde(default)]
        duration_s: f64,
    },
    Checkpoint {
        name: String,
        #[serde(default)]
        expected: BTreeMap<String, Value>,
    },
}

/// A multi-frame action in progress.
enum Motion {
    Orbit {
        started: Option<f64>,
        centre: egui::Pos2,
        dx: f32,
        dy: f32,
        duration: f64,
    },
    Year {
        started: f64,
        from: f64,
        to: f64,
        duration: f64,
    },
}

/// Fraction of a motion completed at `elapsed` seconds after `started`.
fn progress(started: f64, elapsed: f64, duration: f64) -> f64 {
    if duration <= 0.0 {
        1.0
    } else {
        ((elapsed - started) / duration).clamp(0.0, 1.0)
    }
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
    motions: Vec<Motion>,
    viewport: Option<egui::Rect>,
    year_request: Option<f64>,
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
                Action::Key { key, .. } | Action::Tap { key }
                    if egui::Key::from_name(key).is_none() =>
                {
                    return Err(format!("unknown interface check key: {key}").into());
                }
                Action::Orbit { dx, dy, duration_s } => {
                    if [*dx, *dy]
                        .iter()
                        .any(|v| !v.is_finite() || v.abs() > 4096.0)
                        || !duration_s.is_finite()
                        || !(0.0..=60.0).contains(duration_s)
                    {
                        return Err("interface check orbit is nonfinite or too long".into());
                    }
                }
                Action::SetYear {
                    year,
                    to,
                    duration_s,
                } => {
                    if !year.is_finite()
                        || to.is_some_and(|t| !t.is_finite())
                        || !duration_s.is_finite()
                        || !(0.0..=60.0).contains(duration_s)
                    {
                        return Err("interface check year is nonfinite or too long".into());
                    }
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
            motions: Vec::new(),
            viewport: None,
            year_request: None,
        })
    }

    pub fn finished(&self) -> bool {
        self.finished
    }

    /// True once the plan clock has started (the app is settled and ready).
    pub fn armed(&self) -> bool {
        self.armed.is_some()
    }

    /// The 3D viewport's screen rectangle in the last frame, for orbit drags.
    pub fn set_viewport(&mut self, rect: Option<egui::Rect>) {
        self.viewport = rect;
    }

    /// The year cursor requested by a `set_year` action since the last call.
    pub fn take_year_request(&mut self) -> Option<f64> {
        self.year_request.take()
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
                Action::Tap { key } => {
                    self.modifiers = egui::Modifiers::default();
                    let key = egui::Key::from_name(key).expect("validated key");
                    for pressed in [true, false] {
                        input.events.push(egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed,
                            repeat: false,
                            modifiers: self.modifiers,
                        });
                    }
                }
                Action::Orbit { dx, dy, duration_s } => match self.viewport {
                    Some(rect) => self.motions.push(Motion::Orbit {
                        started: None,
                        centre: rect.center(),
                        dx: *dx,
                        dy: *dy,
                        duration: *duration_s,
                    }),
                    None => eprintln!("FARIS interface check: orbit skipped, no viewport yet"),
                },
                Action::SetYear {
                    year,
                    to,
                    duration_s,
                } => self.motions.push(Motion::Year {
                    started: armed.elapsed().as_secs_f64(),
                    from: *year,
                    to: to.unwrap_or(*year),
                    duration: *duration_s,
                }),
                Action::Checkpoint { name, expected } => {
                    self.pending.push((name.clone(), expected.clone()));
                    // Observe the completed UI pass before injecting later actions.
                    break;
                }
            }
        }
        self.advance_motions(armed.elapsed().as_secs_f64(), input);
    }

    fn advance_motions(&mut self, now: f64, input: &mut egui::RawInput) {
        let mut year = None;
        self.motions.retain_mut(|motion| match motion {
            Motion::Orbit {
                started,
                centre,
                dx,
                dy,
                duration,
            } => {
                let start = *started.get_or_insert_with(|| {
                    input.events.push(egui::Event::PointerMoved(*centre));
                    input.events.push(egui::Event::PointerButton {
                        pos: *centre,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::default(),
                    });
                    now
                });
                let f = progress(start, now, *duration) as f32;
                let at = *centre + egui::vec2(*dx * f, *dy * f);
                input.events.push(egui::Event::PointerMoved(at));
                if f >= 1.0 {
                    input.events.push(egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::default(),
                    });
                    // No resting hover tooltip over the scene.
                    input.events.push(egui::Event::PointerGone);
                }
                f < 1.0
            }
            Motion::Year {
                started,
                from,
                to,
                duration,
            } => {
                let f = progress(*started, now, *duration);
                year = Some(*from + (*to - *from) * f);
                f < 1.0
            }
        });
        if year.is_some() {
            self.year_request = year;
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
        let timed_out = plan_timed_out(
            self.launched.elapsed().as_secs_f64(),
            self.armed.map(|armed| armed.elapsed().as_secs_f64()),
        );
        if (self.next == self.plan.steps.len() && self.motions.is_empty()) || timed_out {
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

/// Seconds a plan may run once armed (plan step times are at most 120 s).
const ARMED_LIMIT_S: f64 = 150.0;
/// Seconds the app may take to become ready for the plan. Recording waits for
/// the automatic uncertainty ensembles, which take minutes on the release data.
const ARMING_LIMIT_S: f64 = 600.0;

/// Whether the plan has run out of time: before arming, measured from launch;
/// after arming, measured from the arming instant.
fn plan_timed_out(since_launch_s: f64, since_armed_s: Option<f64>) -> bool {
    match since_armed_s {
        Some(armed) => armed >= ARMED_LIMIT_S,
        None => since_launch_s >= ARMING_LIMIT_S,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_time_limit_counts_from_arming_once_armed() {
        assert!(!plan_timed_out(400.0, None));
        assert!(plan_timed_out(600.0, None));
        assert!(!plan_timed_out(500.0, Some(149.0)));
        assert!(plan_timed_out(10.0, Some(150.0)));
    }

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
    fn progress_is_linear_and_clamped() {
        assert_eq!(progress(1.0, 0.0, 4.0), 0.0);
        assert_eq!(progress(1.0, 3.0, 4.0), 0.5);
        assert_eq!(progress(1.0, 9.0, 4.0), 1.0);
        assert_eq!(progress(1.0, 1.0, 0.0), 1.0);
    }

    #[test]
    fn pixel_free_actions_parse_and_bad_ones_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        let good = write_plan(
            directory.path(),
            serde_json::json!([
                {"time_s":0.0,"type":"tap","key":"2"},
                {"time_s":0.5,"type":"orbit","dx":-200.0,"dy":10.0,"duration_s":4.0},
                {"time_s":5.0,"type":"set_year","year":0.0,"to":30.0,"duration_s":5.0},
                {"time_s":10.0,"type":"checkpoint","name":"end"},
            ]),
        );
        assert!(InterfaceCheck::load(&good, &directory.path().join("a.json")).is_ok());
        for bad in [
            serde_json::json!({"time_s":0.0,"type":"tap","key":"Nope"}),
            serde_json::json!({"time_s":0.0,"type":"orbit","dx":1.0,"dy":1.0,"duration_s":-1.0}),
            serde_json::json!({"time_s":0.0,"type":"set_year","year":0.0,"duration_s":999.0}),
        ] {
            let plan = write_plan(
                directory.path(),
                serde_json::json!([bad, {"time_s":1.0,"type":"checkpoint","name":"c"}]),
            );
            assert!(InterfaceCheck::load(&plan, &directory.path().join("b.json")).is_err());
        }
    }

    #[test]
    fn set_year_interpolates_and_orbit_drags_from_viewport_centre() {
        let directory = tempfile::tempdir().unwrap();
        let plan = write_plan(
            directory.path(),
            serde_json::json!([
                {"time_s":0.0,"type":"checkpoint","name":"c"},
            ]),
        );
        let mut check = InterfaceCheck::load(&plan, &directory.path().join("r.json")).unwrap();
        check.motions.push(Motion::Orbit {
            started: None,
            centre: egui::pos2(500.0, 300.0),
            dx: 100.0,
            dy: 0.0,
            duration: 2.0,
        });
        let mut input = egui::RawInput::default();
        check.advance_motions(0.0, &mut input);
        assert!(matches!(
            input.events[1],
            egui::Event::PointerButton { pressed: true, .. }
        ));
        let mut input = egui::RawInput::default();
        check.advance_motions(2.5, &mut input);
        assert!(input.events.iter().any(|e| matches!(
            e, egui::Event::PointerMoved(p) if *p == egui::pos2(600.0, 300.0))));
        assert!(check.motions.is_empty());
        check.motions.push(Motion::Year {
            started: 0.0,
            from: 0.0,
            to: 30.0,
            duration: 10.0,
        });
        check.advance_motions(5.0, &mut egui::RawInput::default());
        assert_eq!(check.take_year_request(), Some(15.0));
        assert_eq!(check.take_year_request(), None);
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
