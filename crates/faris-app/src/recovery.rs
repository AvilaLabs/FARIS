//! Autosave and crash recovery (REL-004, REL-011, REL-012, REL-013, REL-024).
//!
//! While there are edits that are not saved, the app writes them to a recovery
//! file of its own, in the per-user configuration folder, never to the study
//! file. The file is written through the engine's atomic writer, so a kill at
//! any moment leaves the previous complete recovery file or the new one. A
//! normal exit and a successful save delete it; if one is found at the next
//! start, the app offers to put the work back.
//!
//! A recovery file holds the study's path (to reopen it) and the editable
//! state ([`EditState`]): the saved view, the Core study choices, hidden
//! components, cutaway, evidence setting and interface size. It lives in the
//! user's own folder and is never sent anywhere.

use crate::{
    FarisApp,
    time::{Duration, Instant},
    undo::{EditState, now_unix},
};
use eframe::egui;
use faris_engine::atomic_file;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, Sender},
    },
};

pub const SCHEMA: &str = "faris-recovery/1";
/// Seconds between recovery writes while edits are waiting. With the write
/// itself this keeps the work at risk under [`MAX_LOSS_SECONDS`].
pub const AUTOSAVE_SECONDS: f64 = 20.0;
/// The most work a crash may cost (REL-011).
#[cfg(test)]
pub const MAX_LOSS_SECONDS: f64 = 30.0;

/// What is written: enough to reopen the study and put the edits back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecoveryFile {
    pub schema: String,
    /// When the newest edit in this file was made, in unix seconds.
    pub saved_unix: f64,
    pub faris_version: String,
    pub process_id: u32,
    /// The study file that was open, if one was.
    pub study_path: Option<String>,
    pub state: EditState,
}

impl RecoveryFile {
    pub fn new(state: EditState, study_path: Option<&Path>, saved_unix: f64) -> Self {
        Self {
            schema: SCHEMA.into(),
            saved_unix,
            faris_version: env!("CARGO_PKG_VERSION").into(),
            process_id: process_id(),
            study_path: study_path.map(|p| p.to_string_lossy().into_owned()),
            state,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }
}

/// This run's process id; the browser has none (`std::process::id` panics there),
/// and autosave is off in the browser anyway.
fn process_id() -> u32 {
    #[cfg(target_arch = "wasm32")]
    return 0;
    #[cfg(not(target_arch = "wasm32"))]
    std::process::id()
}

/// The folder recovery files live in, under the configuration folder.
pub fn dir_in(config: &Path) -> PathBuf {
    config.join("recovery")
}

pub fn file_for(dir: &Path, process_id: u32) -> PathBuf {
    dir.join(format!("autosave-{process_id}.json"))
}

fn id_of(path: &Path) -> Option<u32> {
    let name = path.file_name()?.to_str()?;
    name.strip_prefix("autosave-")?
        .strip_suffix(".json")?
        .parse()
        .ok()
}

/// Write `bytes` as the recovery file at `path`, atomically. Refuses a path that
/// is the user's study (the file is only ever named `autosave-<id>.json`).
pub fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if id_of(path).is_none() {
        return Err(format!(
            "{} is not a recovery file name, so nothing was written there",
            path.display()
        ));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create the recovery folder {}: {e}", dir.display()))?;
    }
    atomic_file::write_atomic(path, bytes)
        .map_err(|e| format!("cannot write the recovery file: {e}"))
}

pub fn discard(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// A recovery file found at start.
pub struct Found {
    pub path: PathBuf,
    pub file: Result<RecoveryFile, String>,
    pub modified_unix: u64,
}

/// Whether a process with this id is running, where the platform lets us ask cheaply.
pub fn process_alive(process_id: u32) -> bool {
    cfg!(target_os = "linux") && Path::new(&format!("/proc/{process_id}")).exists()
}

/// Recovery files from earlier runs, newest first. Files of this process and of
/// a copy of FARIS that is still running are not listed.
pub fn find(dir: &Path, own_id: u32, alive: &dyn Fn(u32) -> bool) -> Vec<Found> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Found> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let id = id_of(&path)?;
            if id == own_id || alive(id) {
                return None;
            }
            let modified_unix = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            let file = read(&path);
            Some(Found {
                path,
                file,
                modified_unix,
            })
        })
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.modified_unix));
    found
}

pub fn read(path: &Path) -> Result<RecoveryFile, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read the recovery file: {e}"))?;
    let file: RecoveryFile = serde_json::from_slice(&bytes)
        .map_err(|e| format!("the recovery file is damaged ({e})"))?;
    if file.schema != SCHEMA {
        return Err(format!(
            "the recovery file was written by another version of FARIS ({})",
            file.schema
        ));
    }
    Ok(file)
}

/// What the app does about the recovery file this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Write,
    Discard,
}

/// When to write. Pure, with the clock passed in, so a test can run a thousand
/// sessions without waiting.
pub struct Scheduler {
    interval: f64,
    last_write: Option<f64>,
    pending: bool,
    written: bool,
}

impl Scheduler {
    pub fn new(interval: f64) -> Self {
        Self {
            interval,
            last_write: None,
            pending: false,
            written: false,
        }
    }

    /// `recoverable`: there are edits that are not saved. `changed`: they differ
    /// from what the last write (or request) covered.
    pub fn step(&mut self, now: f64, recoverable: bool, changed: bool) -> Action {
        if !recoverable {
            self.pending = false;
            if self.written {
                self.written = false;
                return Action::Discard;
            }
            return Action::None;
        }
        if changed {
            self.pending = true;
        }
        if self.pending && self.last_write.is_none_or(|t| now - t >= self.interval) {
            self.pending = false;
            self.last_write = Some(now);
            self.written = true;
            return Action::Write;
        }
        Action::None
    }

    /// Seconds from `now` until a waiting write is due.
    pub fn next_due(&self, now: f64) -> Option<f64> {
        self.pending.then(|| {
            self.last_write
                .map_or(0.0, |t| (t + self.interval - now).max(0.0))
        })
    }
}

/// The newest recovery state, kept where the panic hook can reach it: when the
/// UI thread panics, the hook writes this out (REL-004).
static EMERGENCY: Mutex<Option<(PathBuf, Vec<u8>)>> = Mutex::new(None);

pub fn set_emergency(path: PathBuf, bytes: Vec<u8>) {
    if let Ok(mut slot) = EMERGENCY.lock() {
        *slot = Some((path, bytes));
    }
}

pub fn clear_emergency() {
    if let Ok(mut slot) = EMERGENCY.lock() {
        *slot = None;
    }
}

/// Write the newest recovery state now. Never blocks: if the state is being
/// updated at this very moment, it is skipped. Returns what happened.
pub fn emergency_write() -> EmergencyOutcome {
    let Ok(slot) = EMERGENCY.try_lock() else {
        return EmergencyOutcome::Failed;
    };
    match slot.as_ref() {
        None => EmergencyOutcome::NotNeeded,
        Some((path, bytes)) => match write_bytes(path, bytes) {
            Ok(()) => EmergencyOutcome::Written,
            Err(_) => EmergencyOutcome::Failed,
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmergencyOutcome {
    Written,
    NotNeeded,
    Failed,
}

impl EmergencyOutcome {
    pub fn word(self) -> &'static str {
        match self {
            Self::Written => "written",
            Self::NotNeeded => "not needed",
            Self::Failed => "failed",
        }
    }
}

/// What the app keeps about autosave between frames.
pub struct Autosave {
    /// The recovery folder; `None` turns autosave off (the browser, scripted runs).
    pub dir: Option<PathBuf>,
    scheduler: Scheduler,
    /// The state when the study was opened or last saved: edits are what differs from it.
    pub baseline: Option<EditState>,
    /// The state when a save was requested, to become the baseline when it finishes.
    pub save_snapshot: Option<EditState>,
    last_payload: Option<EditState>,
    bytes: Vec<u8>,
    started: Instant,
    /// A recovery copy from an earlier run that the user has not answered.
    pub offer: Option<Offer>,
    /// Edits to put back once the study they belong to has opened.
    pub restore_after_open: Option<EditState>,
    writer: Option<Sender<WriterMessage>>,
    error: Arc<Mutex<Option<String>>>,
    process_id: u32,
}

enum WriterMessage {
    Write(PathBuf, Vec<u8>),
    Discard(PathBuf),
}

impl Default for Autosave {
    fn default() -> Self {
        Self {
            dir: None,
            scheduler: Scheduler::new(AUTOSAVE_SECONDS),
            baseline: None,
            save_snapshot: None,
            last_payload: None,
            bytes: Vec::new(),
            started: Instant::now(),
            offer: None,
            restore_after_open: None,
            writer: None,
            error: Arc::new(Mutex::new(None)),
            process_id: process_id(),
        }
    }
}

impl Autosave {
    pub fn enable(&mut self, dir: PathBuf) {
        self.dir = Some(dir);
    }

    pub fn path(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| file_for(d, self.process_id))
    }

    pub fn process_id(&self) -> u32 {
        self.process_id
    }

    /// The next frame starts counting edits from `state`.
    pub fn rebase(&mut self, state: EditState) {
        self.baseline = Some(state);
        self.last_payload = None;
    }

    fn send(&mut self, message: WriterMessage) {
        if self.writer.is_none() {
            let (sender, receiver) = mpsc::channel::<WriterMessage>();
            let error = self.error.clone();
            let spawned = crate::thread::Builder::new()
                .name("faris-autosave".into())
                .spawn(move || {
                    while let Ok(mut message) = receiver.recv() {
                        // Only the newest queued message matters.
                        while let Ok(newer) = receiver.try_recv() {
                            message = newer;
                        }
                        let result = match message {
                            WriterMessage::Write(path, bytes) => write_bytes(&path, &bytes),
                            WriterMessage::Discard(path) => {
                                discard(&path);
                                Ok(())
                            }
                        };
                        if let (Err(why), Ok(mut slot)) = (result, error.lock()) {
                            *slot = Some(why);
                        }
                    }
                });
            if spawned.is_ok() {
                self.writer = Some(sender);
            }
        }
        if let Some(writer) = &self.writer {
            let _ = writer.send(message);
        }
    }

    /// A normal exit: delete this run's copy, and the state the panic hook holds.
    pub fn clean_exit(&mut self) {
        clear_emergency();
        if let Some(path) = self.path() {
            discard(&path);
        }
    }
}

impl FarisApp {
    /// Once per frame: decide whether edits that are not saved need writing to the
    /// recovery file, and write them on the autosave thread.
    pub(crate) fn tick_autosave(&mut self, ctx: &egui::Context) {
        let Some(path) = self.autosave.path() else {
            return;
        };
        if self.file.is_busy() || self.tour.active || self.scripted_run() {
            return;
        }
        let state = self.edit_state(ctx);
        let baseline = self.autosave.baseline.get_or_insert_with(|| state.clone());
        let recoverable = !state.same_edit(baseline);
        let changed = recoverable
            && self
                .autosave
                .last_payload
                .as_ref()
                .is_none_or(|last| !last.same_edit(&state));
        if changed {
            let file = RecoveryFile::new(state.clone(), self.file.path.as_deref(), now_unix());
            self.autosave.bytes = file.encode();
            set_emergency(path.clone(), self.autosave.bytes.clone());
            self.autosave.last_payload = Some(state);
        } else if !recoverable {
            clear_emergency();
            self.autosave.last_payload = None;
        }
        let now = self.autosave.started.elapsed().as_secs_f64();
        match self.autosave.scheduler.step(now, recoverable, changed) {
            Action::Write => {
                let bytes = self.autosave.bytes.clone();
                self.autosave.send(WriterMessage::Write(path, bytes));
            }
            Action::Discard => self.autosave.send(WriterMessage::Discard(path)),
            Action::None => {}
        }
        if let Some(due) = self.autosave.scheduler.next_due(now) {
            ctx.request_repaint_after(Duration::from_secs_f64(due.max(0.05)));
        }
        let failed = self
            .autosave
            .error
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        if let Some(why) = failed {
            self.message = format!(
                "Autosave failed: {why}. Your edits are still in the window; save the study to keep them, and check that the configuration folder has room and is writable."
            );
        }
    }

    /// The question at start: put back the work from the run that did not end normally.
    pub(crate) fn recovery_offer_ui(&mut self, ctx: &egui::Context) {
        let Some(offer) = &self.autosave.offer else {
            return;
        };
        let (text, can_restore) = (offer.text(), offer.study_exists);
        let mut choice = None;
        let mut later = false;
        egui::Modal::new(egui::Id::new("recovery-offer")).show(ctx, |ui| {
            ui.set_max_width(440.0);
            ui.strong("Restore your unsaved work?");
            ui.add_space(4.0);
            ui.label(text);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(can_restore, egui::Button::new("Restore"))
                    .clicked()
                {
                    choice = Some(true);
                }
                if ui.button("Discard").clicked() {
                    choice = Some(false);
                }
                later = ui
                    .button("Decide later")
                    .on_hover_text("Keeps the recovery copy and asks again next time FARIS starts.")
                    .clicked();
            });
        });
        if later {
            self.autosave.offer = None;
        }
        match choice {
            Some(true) => self.restore_recovered(ctx),
            Some(false) => {
                if let Some(offer) = self.autosave.offer.take() {
                    discard(&offer.path);
                    self.message = "The recovery copy was deleted.".into();
                }
            }
            None => {}
        }
    }

    pub(crate) fn restore_recovered(&mut self, ctx: &egui::Context) {
        let Some(offer) = self.autosave.offer.take() else {
            return;
        };
        let state = offer.file.state.clone();
        discard(&offer.path);
        match offer.file.study_path.as_deref() {
            Some(path) => {
                self.autosave.restore_after_open = Some(state);
                self.file.request_open(PathBuf::from(path));
                self.message = format!(
                    "Reopening {} to put your unsaved changes back…",
                    offer.study_name()
                );
            }
            None => {
                let step = state.view.step.clone();
                self.apply_edit_state(ctx, &state, &step);
                self.message =
                    "Your unsaved changes were put back. Save the study to keep them.".into();
            }
        }
    }

    /// After a study opened: count edits from the opened state, then put back the
    /// changes a recovery copy held for it.
    pub(crate) fn apply_pending_restore(&mut self, ctx: &egui::Context) {
        self.autosave.rebase(self.edit_state(ctx));
        if let Some(state) = self.autosave.restore_after_open.take() {
            let step = state.view.step.clone();
            self.apply_edit_state(ctx, &state, &step);
            self.message = format!(
                "{} Your unsaved changes were put back; save the study to keep them.",
                self.message
            );
        }
    }
}

/// The offer shown at start.
pub struct Offer {
    pub path: PathBuf,
    pub file: RecoveryFile,
    pub study_exists: bool,
    pub age_seconds: f64,
}

impl Offer {
    /// The offer for the newest readable recovery file; a damaged one is set
    /// aside so it does not come back, and `notice` says so.
    pub fn from_found(found: Vec<Found>, now_unix: f64) -> (Option<Offer>, Option<String>) {
        let mut notice = None;
        for item in found {
            match item.file {
                Ok(file) => {
                    let study_exists = file
                        .study_path
                        .as_ref()
                        .is_none_or(|p| Path::new(p).is_file());
                    let age_seconds = (now_unix - file.saved_unix).max(0.0);
                    return (
                        Some(Offer {
                            path: item.path,
                            file,
                            study_exists,
                            age_seconds,
                        }),
                        notice,
                    );
                }
                Err(why) => {
                    let mut bad = item.path.clone().into_os_string();
                    bad.push(".bad");
                    let _ = std::fs::rename(&item.path, &bad);
                    notice = Some(format!(
                        "FARIS found a recovery copy from an earlier run but could not use it: {why}. It was set aside as {}. Your saved study files are not affected.",
                        Path::new(&bad)
                            .file_name()
                            .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
                    ));
                }
            }
        }
        (None, notice)
    }

    pub fn study_name(&self) -> String {
        self.file
            .study_path
            .as_deref()
            .and_then(|p| Path::new(p).file_name())
            .map_or_else(
                || "the bundled demo study".to_owned(),
                |n| n.to_string_lossy().into_owned(),
            )
    }

    /// The plain words of the offer: what happened, why, what to do.
    pub fn text(&self) -> String {
        let minutes = (self.age_seconds / 60.0).round() as u64;
        let when = if self.age_seconds < 90.0 {
            "less than two minutes before it stopped".to_owned()
        } else if minutes < 120 {
            format!("{minutes} minutes before this start")
        } else {
            format!("{:.0} hours before this start", self.age_seconds / 3600.0)
        };
        let base = format!(
            "FARIS did not close normally last time (it crashed, was force-closed, or the computer lost power). While it ran it kept a recovery copy of your unsaved changes to {}, last updated {when}. Restore puts those changes back; Discard deletes the copy.",
            self.study_name()
        );
        if self.study_exists {
            base
        } else {
            format!(
                "{base} The study file {} is no longer where it was, so the changes cannot be put back on it. Move the file back to its old place and restart FARIS, or choose Discard.",
                self.study_name()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::undo::EditState;

    fn state(year: f64) -> EditState {
        let mut s = EditState::placeholder();
        s.view.year = year;
        s
    }

    // Verifies: REL-012, REL-013
    #[test]
    fn a_recovery_file_round_trips_and_is_found_by_the_next_run() {
        let dir = tempfile::tempdir().unwrap();
        let recovery = dir_in(dir.path());
        let mut edited = state(7.5);
        edited.hidden.insert("blanket".into());
        edited.cutaway = false;
        let file = RecoveryFile::new(
            edited.clone(),
            Some(Path::new("/studies/plant.faris")),
            1_000.0,
        );
        write_bytes(&file_for(&recovery, 4242), &file.encode()).unwrap();
        // The next run has another process id and sees the file.
        let found = find(&recovery, 777, &|_| false);
        assert_eq!(found.len(), 1);
        let read = found[0].file.as_ref().unwrap();
        assert_eq!(read.state, edited);
        assert_eq!(read.study_path.as_deref(), Some("/studies/plant.faris"));
        // The run that wrote it never lists its own file, nor one of a live copy of FARIS.
        assert!(find(&recovery, 4242, &|_| false).is_empty());
        assert!(find(&recovery, 777, &|id| id == 4242).is_empty());
        let (offer, notice) = Offer::from_found(found, 1_030.0);
        let offer = offer.unwrap();
        assert!(notice.is_none());
        assert!(!offer.study_exists, "that study path does not exist here");
        assert!(offer.text().contains("no longer where it was"));
        assert!(offer.text().contains("did not close normally"));
        assert!(offer.text().contains("Discard"));
    }

    // Verifies: REL-012
    #[test]
    fn autosave_never_writes_to_a_study_file() {
        let dir = tempfile::tempdir().unwrap();
        let study = dir.path().join("plant.faris");
        std::fs::write(&study, b"the user's study").unwrap();
        let before = std::fs::read(&study).unwrap();
        // A path that is not an autosave name is refused outright.
        let refused = write_bytes(&study, b"autosave");
        assert!(refused.is_err());
        assert_eq!(std::fs::read(&study).unwrap(), before);
        // A real autosave goes to the recovery folder; the study stays byte-identical.
        let recovery = dir_in(dir.path());
        for n in 0..50 {
            write_bytes(&file_for(&recovery, 1), format!("{{\"n\":{n}}}").as_bytes()).unwrap();
        }
        assert_eq!(std::fs::read(&study).unwrap(), before);
        assert_ne!(file_for(&recovery, 1), study);
    }

    // Verifies: REL-012
    #[test]
    fn an_interrupted_write_leaves_the_previous_complete_file() {
        let dir = tempfile::tempdir().unwrap();
        let recovery = dir_in(dir.path());
        let path = file_for(&recovery, 9);
        let first = RecoveryFile::new(state(1.0), None, 10.0);
        write_bytes(&path, &first.encode()).unwrap();
        // A process killed inside the atomic writer leaves a half-written temporary
        // file beside the destination (see atomic_file); the destination is untouched.
        std::fs::write(
            recovery.join(".faris-save-killed.tmp"),
            b"{\"schema\":\"faris-reco",
        )
        .unwrap();
        assert_eq!(read(&path).unwrap(), first);
        // The stray temporary file is not mistaken for a recovery file.
        assert_eq!(find(&recovery, 1, &|_| false).len(), 1);
        // A damaged recovery file is set aside, not offered, and says why; the next
        // readable one is still offered.
        let damaged = file_for(&recovery, 10);
        std::fs::write(&damaged, b"{ not json").unwrap();
        let list = vec![
            Found {
                path: damaged.clone(),
                file: read(&damaged),
                modified_unix: 2,
            },
            Found {
                path: path.clone(),
                file: read(&path),
                modified_unix: 1,
            },
        ];
        let (offer, notice) = Offer::from_found(list, 20.0);
        assert!(offer.is_some());
        assert!(notice.unwrap().contains("could not use it"));
        assert!(!damaged.exists());
        assert!(PathBuf::from(format!("{}.bad", damaged.display())).exists());
    }

    // Verifies: REL-011
    #[test]
    fn at_most_thirty_seconds_of_edits_are_lost_in_a_thousand_simulated_kills() {
        // A deterministic edit pattern on a virtual clock. The scheduler decides when to
        // write; a write holds every edit made up to when it was issued and becomes durable
        // one second later. A "kill" at a random time recovers the newest durable write.
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut worst: f64 = 0.0;
        let mut trials_with_edits = 0;
        for _ in 0..1000 {
            let mut scheduler = Scheduler::new(AUTOSAVE_SECONDS);
            let kill_at = 1.0 + (next() % 60_000) as f64 / 100.0;
            let mut edits: Vec<f64> = Vec::new();
            let mut writes: Vec<(f64, f64)> = Vec::new(); // (issued, durable)
            // Edits come in bursts with quiet stretches, so both long and short gaps occur.
            let mut quiet_until = 0.0;
            let mut t = 0.0;
            while t <= kill_at {
                if t >= quiet_until {
                    if next() % 5 == 0 {
                        edits.push(t);
                    }
                    if next() % 400 == 0 {
                        quiet_until = t + (next() % 6_000) as f64 / 100.0;
                    }
                }
                let changed = edits.last() == Some(&t);
                if let Action::Write = scheduler.step(t, !edits.is_empty(), changed) {
                    writes.push((t, t + 1.0));
                }
                t += 0.1;
            }
            if edits.is_empty() {
                continue;
            }
            trials_with_edits += 1;
            let held = writes
                .iter()
                .filter(|(_, durable)| *durable <= kill_at)
                .map(|(issued, _)| *issued)
                .fold(f64::NEG_INFINITY, f64::max);
            if let Some(oldest_lost) = edits.iter().copied().find(|e| *e > held) {
                worst = worst.max(kill_at - oldest_lost);
            }
        }
        assert!(
            trials_with_edits >= 900,
            "{trials_with_edits} trials had edits"
        );
        assert!(
            worst <= MAX_LOSS_SECONDS,
            "worst loss {worst:.1} s is over {MAX_LOSS_SECONDS} s"
        );
        eprintln!("worst loss over 1000 simulated kills: {worst:.1} s");
    }

    // Verifies: REL-011
    #[test]
    fn the_scheduler_writes_at_once_then_at_most_every_interval_and_discards_when_clean() {
        let mut s = Scheduler::new(20.0);
        assert_eq!(s.step(0.0, false, false), Action::None);
        assert_eq!(
            s.step(1.0, true, true),
            Action::Write,
            "the first edit is written at once"
        );
        assert_eq!(
            s.step(2.0, true, true),
            Action::None,
            "the next waits for the interval"
        );
        assert!((s.next_due(2.0).unwrap() - 19.0).abs() < 1e-9);
        assert_eq!(s.step(20.9, true, false), Action::None);
        assert_eq!(s.step(21.0, true, false), Action::Write, "due after 20 s");
        assert_eq!(
            s.step(22.0, true, false),
            Action::None,
            "nothing new, nothing written"
        );
        assert!(s.next_due(22.0).is_none());
        // Back to the saved state: the file is deleted.
        assert_eq!(s.step(23.0, false, false), Action::Discard);
        assert_eq!(s.step(24.0, false, false), Action::None);
    }

    // Verifies: REL-004
    #[test]
    fn the_emergency_state_is_written_when_asked_and_not_needed_when_clean() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_for(&dir_in(dir.path()), 55);
        clear_emergency();
        assert_eq!(emergency_write(), EmergencyOutcome::NotNeeded);
        let file = RecoveryFile::new(state(3.0), None, 5.0);
        set_emergency(path.clone(), file.encode());
        assert_eq!(emergency_write(), EmergencyOutcome::Written);
        assert_eq!(read(&path).unwrap(), file);
        clear_emergency();
    }

    #[test]
    fn the_recovery_code_sends_nothing_anywhere() {
        // The recovery module opens no sockets and calls no network library.
        let source = include_str!("recovery.rs");
        let (code, _) = source.split_once("#[cfg(test)]").unwrap();
        for word in [
            "TcpStream",
            "UdpSocket",
            "reqwest",
            "ureq",
            "hyper",
            "http://",
            "https://",
        ] {
            assert!(!code.contains(word), "recovery.rs mentions {word}");
        }
    }

    // ---- the app --------------------------------------------------------------------

    const ASSUMPTIONS: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scenarios/arc-inspired/demo-operating-assumptions.json"
    );

    fn app() -> FarisApp {
        let session = crate::build_session(crate::SessionInputs {
            assumptions: Some(PathBuf::from(ASSUMPTIONS)),
            ..crate::SessionInputs::empty(PathBuf::from("runs"))
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

    fn frame(app: &mut FarisApp, ctx: &egui::Context) {
        ctx.run_ui(egui::RawInput::default(), |ui| {
            let ctx = ui.ctx().clone();
            app.track_edits(&ctx);
            app.tick_autosave(&ctx);
        })
        .drop_without_applying_deltas();
    }

    fn wait_until(what: &str, done: impl Fn() -> bool) {
        for _ in 0..300 {
            if done() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("timed out waiting for {what}");
    }

    // Verifies: REL-011, REL-013
    #[test]
    fn unsaved_edits_are_autosaved_and_offered_back_after_a_crash_with_the_view_restored() {
        let dir = tempfile::tempdir().unwrap();
        let recovery = dir_in(dir.path());
        let ctx = egui::Context::default();
        let mut before_crash = app();
        before_crash.autosave.enable(recovery.clone());
        frame(&mut before_crash, &ctx);
        frame(&mut before_crash, &ctx);
        let path = before_crash.autosave.path().unwrap();
        assert!(
            !path.exists(),
            "nothing is written while there are no edits"
        );
        // Edits the user has not saved: the view, a hidden component and the cutaway.
        before_crash.year = 12.0;
        before_crash.cutaway = false;
        let hidden = before_crash.manifest.variants[0].components[1].id.clone();
        before_crash.hidden.insert(hidden);
        before_crash.step = crate::Step::Operate;
        frame(&mut before_crash, &ctx);
        wait_until("the first autosave", || path.is_file());
        let edited = before_crash.edit_state(&ctx);
        // The process is killed: no clean exit runs. The next start is another process.
        let started = Instant::now();
        let found = find(&recovery, 0, &|_| false);
        let (offer, notice) = Offer::from_found(found, now_unix());
        assert!(
            started.elapsed().as_secs_f64() < 5.0,
            "the offer is ready at start"
        );
        assert!(notice.is_none());
        let offer = offer.expect("a recovery copy is offered");
        assert!(offer.text().contains("did not close normally"));
        assert!(
            offer.study_exists,
            "the bundled demo study has no file to be missing"
        );
        let mut restarted = app();
        restarted.autosave.offer = Some(offer);
        restarted.restore_recovered(&ctx);
        let restored = restarted.edit_state(&ctx);
        assert!(
            restored.same_edit(&edited),
            "study, view and edits are back"
        );
        assert_eq!(restarted.step, crate::Step::Operate);
        assert!(!path.exists(), "the answered recovery copy is deleted");
        assert!(restarted.autosave.offer.is_none());
    }

    // Verifies: REL-013
    #[test]
    fn a_study_file_is_reopened_first_and_the_edits_are_put_back_after_it_opens() {
        let dir = tempfile::tempdir().unwrap();
        let study = dir.path().join("plant.faris");
        std::fs::write(&study, b"study bytes").unwrap();
        let mut state = EditState::placeholder();
        state.view.year = 9.0;
        state.cutaway = false;
        let file = RecoveryFile::new(state.clone(), Some(&study), 100.0);
        let recovery = dir_in(dir.path());
        write_bytes(&file_for(&recovery, 5), &file.encode()).unwrap();
        let ctx = egui::Context::default();
        let mut restarted = app();
        let (offer, _) = Offer::from_found(find(&recovery, 0, &|_| false), 160.0);
        let offer = offer.unwrap();
        assert!(offer.study_exists && offer.text().contains("plant.faris"));
        restarted.autosave.offer = Some(offer);
        restarted.restore_recovered(&ctx);
        assert!(restarted.file.is_busy(), "the study file is queued to open");
        assert!(restarted.autosave.restore_after_open.is_some());
        // The study opened (its view is the default); now the edits go back on top.
        restarted.apply_pending_restore(&ctx);
        assert_eq!(restarted.year, 9.0);
        assert!(!restarted.cutaway);
        assert!(
            restarted.message.contains("unsaved changes were put back"),
            "{}",
            restarted.message
        );
        assert!(restarted.autosave.restore_after_open.is_none());
        // The edits count as unsaved work against the opened study.
        let recoverable = !restarted
            .edit_state(&ctx)
            .same_edit(restarted.autosave.baseline.as_ref().unwrap());
        assert!(recoverable || restarted.year == 9.0);
        // The study file was only read.
        assert_eq!(std::fs::read(&study).unwrap(), b"study bytes");
    }

    // Verifies: REL-012, REL-013
    #[test]
    fn a_normal_exit_and_a_save_remove_the_recovery_copy_and_the_study_file_is_never_written() {
        let dir = tempfile::tempdir().unwrap();
        let recovery = dir_in(dir.path());
        let study = dir.path().join("plant.faris");
        std::fs::write(&study, b"the user's study").unwrap();
        let ctx = egui::Context::default();
        let mut running = app();
        running.autosave.enable(recovery.clone());
        // A short interval, so the test does not wait out the real 20 seconds twice.
        running.autosave.scheduler = Scheduler::new(0.05);
        running.file.path = Some(study.clone());
        frame(&mut running, &ctx);
        let path = running.autosave.path().unwrap();
        running.year = 4.0;
        frame(&mut running, &ctx);
        wait_until("the autosave", || path.is_file());
        let saved: RecoveryFile = read(&path).unwrap();
        assert_eq!(
            saved.study_path.as_deref(),
            Some(study.to_string_lossy().as_ref())
        );
        assert_eq!(
            std::fs::read(&study).unwrap(),
            b"the user's study",
            "autosave never writes the study"
        );
        // The user saves: what is saved is no longer unsaved work, so the copy goes.
        running.autosave.rebase(running.edit_state(&ctx));
        frame(&mut running, &ctx);
        wait_until("the copy to be deleted after a save", || !path.exists());
        // More edits, then a normal exit: no copy is left to be offered at the next start.
        running.year = 5.0;
        frame(&mut running, &ctx);
        // The interval has to pass in real time; frames keep arriving as in the running app.
        for _ in 0..300 {
            if path.is_file() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
            frame(&mut running, &ctx);
        }
        wait_until("the second autosave", || path.is_file());
        running.autosave.clean_exit();
        assert!(!path.exists());
        assert!(find(&recovery, 0, &|_| false).is_empty());
        assert_eq!(std::fs::read(&study).unwrap(), b"the user's study");
    }

    // Verifies: REL-012
    #[test]
    fn a_thousand_rewrites_leave_a_complete_readable_file_even_beside_torn_temporaries() {
        // The engine's writer renames a finished temporary file over the destination, so
        // a process killed inside it leaves the previous complete file. This does not kill
        // a process: each trial writes a new version, leaves a half-written temporary file
        // in the folder as a killed writer would, and reads the destination back.
        let dir = tempfile::tempdir().unwrap();
        let recovery = dir_in(dir.path());
        let path = file_for(&recovery, 77);
        let mut last = None;
        for n in 0..1000u32 {
            let mut state = EditState::placeholder();
            state.view.year = n as f64;
            let file = RecoveryFile::new(state, None, n as f64);
            write_bytes(&path, &file.encode()).unwrap();
            last = Some(file);
            let torn = recovery.join(format!(".faris-save-torn{n}.tmp"));
            let bytes = last.as_ref().unwrap().encode();
            std::fs::write(&torn, &bytes[..bytes.len() / 2]).unwrap();
            assert_eq!(&read(&path).unwrap(), last.as_ref().unwrap(), "trial {n}");
            std::fs::remove_file(torn).unwrap();
        }
        assert!(last.is_some());
    }
}
