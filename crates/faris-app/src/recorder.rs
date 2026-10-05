//! Development frame recorder: saves the app's own window as numbered PNGs at a
//! fixed cadence (measured on frame time) for assembling README media. Frames
//! are encoded on a worker thread behind a bounded queue; a full queue drops
//! and counts the frame rather than stalling the UI.

use eframe::egui;
use std::{
    io::BufWriter,
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{SyncSender, TrySendError, sync_channel},
    },
    thread::JoinHandle,
};

pub const MAX_FRAMES: u64 = 600;
const QUEUE_DEPTH: usize = 6;
/// Tag on the screenshot command so it is not mistaken for --capture or export.
pub const SCREENSHOT_TAG: &str = "faris-record-frame";

/// Decides when the next frame is due; no I/O.
#[derive(Debug)]
pub struct Cadence {
    interval: f64,
    next_due: f64,
    taken: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Poll {
    Wait,
    Take,
    /// The frame bound has been reached; recording must stop.
    Full,
}

impl Cadence {
    pub fn new(fps: f64) -> Self {
        Self {
            interval: 1.0 / fps,
            next_due: 0.0,
            taken: 0,
        }
    }

    /// `t` is seconds since recording began. Slow frames never cause a burst
    /// of catch-up captures: a late frame reschedules one interval ahead.
    pub fn poll(&mut self, t: f64) -> Poll {
        if self.taken >= MAX_FRAMES {
            Poll::Full
        } else if t >= self.next_due {
            self.taken += 1;
            let steady = self.next_due + self.interval;
            self.next_due = if steady > t {
                steady
            } else {
                t + self.interval
            };
            Poll::Take
        } else {
            Poll::Wait
        }
    }

    #[cfg(test)]
    pub fn taken(&self) -> u64 {
        self.taken
    }
}

/// Checks the command-line settings; returns an error message when refused.
pub fn validate(
    dir: &Path,
    fps: f64,
    seconds: f64,
    has_plan: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if !dir.is_dir() {
        return Err(format!(
            "--record-frames needs an existing directory: {}",
            dir.display()
        )
        .into());
    }
    if std::fs::read_dir(dir)?.next().is_some() {
        return Err(format!("--record-frames directory is not empty: {}", dir.display()).into());
    }
    if !fps.is_finite() || !(1.0..=30.0).contains(&fps) {
        return Err("--record-fps must be in 1..=30".into());
    }
    if !seconds.is_finite() || !(1.0..=120.0).contains(&seconds) {
        return Err("--record-seconds must be in 1..=120".into());
    }
    if !has_plan && (fps * seconds).ceil() > MAX_FRAMES as f64 {
        return Err(
            format!("--record-fps x --record-seconds would exceed {MAX_FRAMES} frames").into(),
        );
    }
    Ok(())
}

struct Job {
    index: u64,
    image: Arc<egui::ColorImage>,
}

pub struct Recorder {
    dir: PathBuf,
    fps: f64,
    seconds: f64,
    has_plan: bool,
    started: Option<std::time::Instant>,
    cadence: Cadence,
    outstanding: Option<(f64, std::time::Instant)>,
    tx: Option<SyncSender<Job>>,
    worker: Option<JoinHandle<u64>>,
    /// Capture time of each saved frame, in order of file index.
    times: Vec<f64>,
    dropped: u64,
    stopping: bool,
}

impl Recorder {
    pub fn new(dir: PathBuf, fps: f64, seconds: f64, has_plan: bool) -> Self {
        let (tx, rx) = sync_channel::<Job>(QUEUE_DEPTH);
        let worker_dir = dir.clone();
        let worker = std::thread::spawn(move || {
            let mut failed = 0;
            for job in rx {
                if let Err(error) = write_png(&worker_dir, job.index, &job.image) {
                    eprintln!("FARIS recorder: frame {} failed: {error}", job.index);
                    failed += 1;
                }
            }
            failed
        });
        Self {
            dir,
            fps,
            seconds,
            has_plan,
            started: None,
            cadence: Cadence::new(fps),
            outstanding: None,
            tx: Some(tx),
            worker: Some(worker),
            times: Vec::new(),
            dropped: 0,
            stopping: false,
        }
    }

    /// Drives recording once per UI frame. `ready` starts the clock; `plan_done`
    /// ends a plan-driven recording. Returns true when recording has finished
    /// and the app should close.
    pub fn frame(&mut self, ctx: &egui::Context, ready: bool, plan_done: bool) -> bool {
        if self.tx.is_none() {
            return true;
        }
        if self.started.is_none() {
            if ready {
                self.started = Some(std::time::Instant::now());
            } else {
                return false;
            }
        }
        let t = self.started.expect("started").elapsed().as_secs_f64();
        let shot = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Screenshot {
                    image, user_data, ..
                } if user_data
                    .data
                    .as_ref()
                    .and_then(|d| d.downcast_ref::<&str>())
                    .is_some_and(|tag| *tag == SCREENSHOT_TAG) =>
                {
                    Some(image.clone())
                }
                _ => None,
            })
        });
        if let Some(image) = shot
            && let Some((requested_t, _)) = self.outstanding.take()
        {
            let index = self.times.len() as u64;
            let sent = self
                .tx
                .as_ref()
                .is_some_and(|tx| match tx.try_send(Job { index, image }) {
                    Ok(()) => true,
                    Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
                });
            if sent {
                self.times.push(requested_t);
            } else {
                self.dropped += 1;
            }
        }
        if self
            .outstanding
            .is_some_and(|(_, asked)| asked.elapsed().as_secs() >= 5)
        {
            eprintln!("FARIS recorder: a screenshot never arrived; stopping");
            self.stopping = true;
        }
        let over_time = !self.has_plan && t >= self.seconds;
        if (plan_done || over_time) && self.outstanding.is_none() {
            self.stopping = true;
        }
        if self.stopping && self.outstanding.is_none() {
            self.finish();
            return true;
        }
        if !self.stopping && !plan_done && !over_time && self.outstanding.is_none() {
            match self.cadence.poll(t) {
                Poll::Take => {
                    self.outstanding = Some((t, std::time::Instant::now()));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
                        SCREENSHOT_TAG,
                    )));
                }
                Poll::Full => {
                    eprintln!("FARIS recorder: {MAX_FRAMES}-frame bound reached; stopping");
                    self.stopping = true;
                }
                Poll::Wait => {}
            }
        }
        ctx.request_repaint();
        false
    }

    fn finish(&mut self) {
        drop(self.tx.take());
        let failed = self.worker.take().map_or(0, |w| w.join().unwrap_or(0));
        let saved = self.times.len() as u64 - failed.min(self.times.len() as u64);
        let index: Vec<_> = self
            .times
            .iter()
            .enumerate()
            .map(|(i, t)| serde_json::json!({"file":frame_name(i as u64),"t":t}))
            .collect();
        let manifest = serde_json::json!({
            "schema_version":"faris-recorded-frames/v0.1",
            "target_fps":self.fps, "frames":index,
        });
        if let Err(error) = std::fs::write(
            self.dir.join("frames.json"),
            serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
        ) {
            eprintln!("FARIS recorder: frames.json failed: {error}");
        }
        eprintln!(
            "FARIS recorder: {saved} frames saved, {} dropped (queue full), {} failed to encode, in {}",
            self.dropped,
            failed,
            self.dir.display()
        );
    }
}

pub fn frame_name(index: u64) -> String {
    format!("frame-{index:05}.png")
}

fn write_png(
    dir: &Path,
    index: u64,
    image: &egui::ColorImage,
) -> Result<(), Box<dyn std::error::Error>> {
    use image::{
        ImageEncoder,
        codecs::png::{CompressionType, FilterType, PngEncoder},
    };
    let bytes: &[u8] = bytemuck::cast_slice(&image.pixels);
    let file = std::fs::File::create(dir.join(frame_name(index)))?;
    PngEncoder::new_with_quality(BufWriter::new(file), CompressionType::Fast, FilterType::Sub)
        .write_image(
            bytes,
            image.size[0] as u32,
            image.size[1] as u32,
            image::ExtendedColorType::Rgba8,
        )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cadence_follows_frame_time_not_frame_count() {
        let mut c = Cadence::new(10.0);
        assert_eq!(c.poll(0.0), Poll::Take);
        assert_eq!(c.poll(0.05), Poll::Wait);
        assert_eq!(c.poll(0.1), Poll::Take);
        // A long stall yields one frame, not a burst of catch-up frames.
        assert_eq!(c.poll(1.0), Poll::Take);
        assert_eq!(c.poll(1.05), Poll::Wait);
        assert_eq!(c.taken(), 3);
    }

    #[test]
    fn cadence_stops_at_the_frame_bound() {
        let mut c = Cadence::new(30.0);
        for i in 0..MAX_FRAMES {
            assert_eq!(c.poll(i as f64), Poll::Take);
        }
        assert_eq!(c.poll(1e6), Poll::Full);
        assert_eq!(c.taken(), MAX_FRAMES);
    }

    #[test]
    fn missing_nonempty_and_oversized_requests_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert!(validate(dir.path(), 15.0, 20.0, false).is_ok());
        assert!(validate(&dir.path().join("absent"), 15.0, 20.0, false).is_err());
        assert!(validate(dir.path(), 30.0, 30.0, false).is_err());
        // With a plan, the 600-frame bound is enforced while recording.
        assert!(validate(dir.path(), 30.0, 30.0, true).is_ok());
        assert!(validate(dir.path(), 0.5, 20.0, false).is_err());
        std::fs::write(dir.path().join("x"), b"1").unwrap();
        assert!(validate(dir.path(), 15.0, 20.0, false).is_err());
    }

    #[test]
    fn frames_are_written_in_numbered_png_files() {
        let dir = tempfile::tempdir().unwrap();
        let image = egui::ColorImage::filled([4, 3], egui::Color32::RED);
        write_png(dir.path(), 7, &image).unwrap();
        let read = image::open(dir.path().join("frame-00007.png")).unwrap();
        assert_eq!((read.width(), read.height()), (4, 3));
    }
}
