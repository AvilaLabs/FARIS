//! Study export from the desktop: capture the 3D view, ask for a parent folder,
//! write the export folder on a worker thread, then say where it went.
//!
//! The panel only schedules and reports. The folder contents come from
//! `faris-report`, which formats numbers computed by `faris-engine`.

use eframe::egui;
use faris_report::{ExportOutcome, ReportInput};
use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};

/// Tag that tells this panel's screenshot apart from the development capture.
const SCREENSHOT_TAG: &str = "faris-export-view";
/// How long to wait for the window capture before exporting without it.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(3);
/// A viewport smaller than this (in pixels) is not worth a picture.
const MIN_VIEW_PIXELS: usize = 96;

enum Destination {
    /// Ask with the native folder dialog.
    Ask,
    /// Development check: write straight into this parent folder.
    Fixed(PathBuf),
}

/// What the worker reports: the finished export, or None when the folder
/// dialog was dismissed.
type WorkerResult = Result<Option<ExportOutcome>, String>;

enum Stage {
    Idle,
    Capturing {
        input: Box<ReportInput>,
        viewport: Option<egui::Rect>,
        destination: Destination,
        since: Instant,
    },
    Working {
        receiver: Receiver<WorkerResult>,
        started: Instant,
    },
}

/// The last finished attempt, shown in the status bar.
enum Finished {
    Done {
        folder: PathBuf,
        files: usize,
        seconds: f64,
        view_image: bool,
    },
    Failed(String),
}

pub struct ExportPanel {
    stage: Stage,
    finished: Option<Finished>,
    /// `--export-on-load`: export into this parent folder once the histories
    /// are ready, then let the scripted capture or exit proceed.
    development_dir: Option<PathBuf>,
    development_requested: bool,
}

impl ExportPanel {
    pub fn new(development_dir: Option<PathBuf>) -> Self {
        Self {
            stage: Stage::Idle,
            finished: None,
            development_dir,
            development_requested: false,
        }
    }

    pub fn busy(&self) -> bool {
        !matches!(self.stage, Stage::Idle)
    }

    /// The development export has not been asked for or has completed.
    pub fn development_settled(&self) -> bool {
        self.development_dir.is_none()
            || (self.development_requested && !self.busy() && self.finished.is_some())
    }

    /// The `--export-on-load` parent folder, taken once when it is time to start.
    pub fn take_development_request(&mut self) -> Option<PathBuf> {
        if self.development_requested {
            return None;
        }
        let dir = self.development_dir.clone()?;
        self.development_requested = true;
        Some(dir)
    }

    pub fn development_requested(&self) -> bool {
        self.development_dir.is_some()
    }

    /// Record a failure that happened before the worker started.
    pub fn fail(&mut self, error: String) {
        self.finished = Some(Finished::Failed(error));
    }

    /// The top-bar button. Returns true when it was clicked.
    pub fn button(&self, ui: &mut egui::Ui, blocked: Option<&str>) -> bool {
        let label = if self.busy() {
            "Exporting…"
        } else {
            "Export…"
        };
        let response = ui.add_enabled(!self.busy() && blocked.is_none(), egui::Button::new(label));
        response
            .on_hover_text(
                "Write a two-page PDF summary, CSV data and charts for this study into a new folder.",
            )
            .on_disabled_hover_text(blocked.unwrap_or("An export is running."))
            .clicked()
    }

    /// Start an export. `input` carries everything but the 3D view image,
    /// which is captured from the window first.
    pub fn begin(&mut self, ctx: &egui::Context, input: ReportInput, viewport: Option<egui::Rect>) {
        self.start(ctx, input, viewport, Destination::Ask);
    }

    pub fn begin_into(
        &mut self,
        ctx: &egui::Context,
        input: ReportInput,
        viewport: Option<egui::Rect>,
        parent: PathBuf,
    ) {
        self.start(ctx, input, viewport, Destination::Fixed(parent));
    }

    fn start(
        &mut self,
        ctx: &egui::Context,
        input: ReportInput,
        viewport: Option<egui::Rect>,
        destination: Destination,
    ) {
        if self.busy() {
            return;
        }
        self.finished = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
            SCREENSHOT_TAG,
        )));
        ctx.request_repaint();
        self.stage = Stage::Capturing {
            input: Box::new(input),
            viewport,
            destination,
            since: Instant::now(),
        };
    }

    /// Advance the capture and worker stages; call once per frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        match std::mem::replace(&mut self.stage, Stage::Idle) {
            Stage::Idle => {}
            Stage::Capturing {
                mut input,
                viewport,
                destination,
                since,
            } => {
                let screenshot = ctx.input(|i| {
                    i.events.iter().find_map(|event| match event {
                        egui::Event::Screenshot {
                            image, user_data, ..
                        } if is_ours(user_data) => Some(image.clone()),
                        _ => None,
                    })
                });
                if let Some(image) = screenshot {
                    match crop_png(&image, viewport, ctx.pixels_per_point()) {
                        Ok(png) => input.view_image = Some(png),
                        Err(note) => input.view_image_note = Some(note),
                    }
                } else if since.elapsed() < CAPTURE_TIMEOUT {
                    self.stage = Stage::Capturing {
                        input,
                        viewport,
                        destination,
                        since,
                    };
                    ctx.request_repaint();
                    return;
                } else {
                    input.view_image_note = Some(
                        "The window capture did not arrive; this graphics backend may not support screenshots."
                            .into(),
                    );
                }
                self.stage = spawn_worker(ctx, *input, destination);
            }
            Stage::Working { receiver, started } => match receiver.try_recv() {
                Ok(result) => {
                    self.finished = match result {
                        Ok(Some(outcome)) => Some(Finished::Done {
                            files: outcome.files.len() + 1,
                            view_image: outcome.view_image_included,
                            folder: outcome.folder,
                            seconds: started.elapsed().as_secs_f64(),
                        }),
                        Ok(None) => None,
                        Err(error) => Some(Finished::Failed(error)),
                    };
                    if let Some(Finished::Done {
                        folder, seconds, ..
                    }) = &self.finished
                        && self.development_requested
                    {
                        eprintln!(
                            "FARIS export: wrote {} in {seconds:.2} s (including capture and file writing)",
                            folder.display()
                        );
                    }
                    ctx.request_repaint();
                }
                Err(TryRecvError::Empty) => {
                    self.stage = Stage::Working { receiver, started };
                }
                Err(TryRecvError::Disconnected) => {
                    self.finished = Some(Finished::Failed(
                        "The export worker ended without a result.".into(),
                    ));
                }
            },
        }
    }

    /// Status line for the bottom bar: where the last export went, and a
    /// button to open it.
    pub fn status_ui(&mut self, ui: &mut egui::Ui) {
        let mut dismiss = false;
        match &self.finished {
            Some(Finished::Done {
                folder,
                files,
                seconds,
                view_image,
            }) => {
                if ui.small_button("Dismiss").clicked() {
                    dismiss = true;
                }
                if ui.small_button("Open folder").clicked() {
                    open_folder(folder);
                }
                let text = format!("Exported to {}", folder.display());
                ui.add(egui::Label::new(egui::RichText::new(text).small()).truncate())
                    .on_hover_text(format!(
                        "{files} files in {seconds:.1} s{}",
                        if *view_image {
                            ""
                        } else {
                            " · no 3D view image (see export-manifest.json)"
                        }
                    ));
            }
            Some(Finished::Failed(error)) => {
                if ui.small_button("Dismiss").clicked() {
                    dismiss = true;
                }
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!("Export failed: {error}"))
                            .small()
                            .color(egui::Color32::from_rgb(240, 110, 110)),
                    )
                    .truncate(),
                )
                .on_hover_text(error);
            }
            None => {}
        }
        if dismiss {
            self.finished = None;
        }
    }
}

fn is_ours(user_data: &egui::UserData) -> bool {
    user_data
        .data
        .as_ref()
        .and_then(|d| d.downcast_ref::<&str>())
        .is_some_and(|tag| *tag == SCREENSHOT_TAG)
}

fn spawn_worker(ctx: &egui::Context, input: ReportInput, destination: Destination) -> Stage {
    let (sender, receiver) = mpsc::channel();
    let context = ctx.clone();
    let started = Instant::now();
    let spawned = std::thread::Builder::new()
        .name("faris-export".into())
        .spawn(move || {
            let mut input = input;
            let parent = match destination {
                Destination::Fixed(path) => Some(path),
                Destination::Ask => rfd::FileDialog::new()
                    .set_title("Choose where to create the export folder")
                    .pick_folder(),
            };
            let result = match parent {
                None => Ok(None),
                Some(parent) => {
                    input.generated_unix_s = faris_report::now_unix_s();
                    faris_report::export_study(&input, &parent)
                        .map(Some)
                        .map_err(|e| e.to_string())
                }
            };
            let _ = sender.send(result);
            context.request_repaint();
        });
    match spawned {
        Ok(_) => Stage::Working { receiver, started },
        Err(error) => {
            // Report through the normal channel on the next poll.
            let (sender, receiver) = mpsc::channel();
            let _ = sender.send(Err(format!("could not start the export worker: {error}")));
            Stage::Working { receiver, started }
        }
    }
}

/// Crop the window screenshot to the 3D viewport as opaque RGBA bytes and the
/// cropped size. The error text says why there is no picture. The study
/// thumbnail uses the same crop.
pub(crate) fn crop_rgba(
    screenshot: &egui::ColorImage,
    viewport: Option<egui::Rect>,
    pixels_per_point: f32,
) -> Result<(Vec<u8>, [usize; 2]), String> {
    let rect = viewport.ok_or("The 3D viewport was not visible when the export started.")?;
    let [width, height] = screenshot.size;
    let clamp = |v: f32, max: usize| ((v * pixels_per_point).round().max(0.0) as usize).min(max);
    let (x0, x1) = (clamp(rect.min.x, width), clamp(rect.max.x, width));
    let (y0, y1) = (clamp(rect.min.y, height), clamp(rect.max.y, height));
    if x1 <= x0 + MIN_VIEW_PIXELS || y1 <= y0 + MIN_VIEW_PIXELS {
        return Err("The 3D viewport was too small to capture.".into());
    }
    let cropped = screenshot.region_by_pixels([x0, y0], [x1 - x0, y1 - y0]);
    let bytes: Vec<u8> = cropped
        .pixels
        .iter()
        .flat_map(|p| {
            let [r, g, b, _] = p.to_array();
            [r, g, b, 255]
        })
        .collect();
    Ok((bytes, cropped.size))
}

/// Crop the window screenshot to the 3D viewport and encode it as PNG. The
/// error text says why there is no picture.
fn crop_png(
    screenshot: &egui::ColorImage,
    viewport: Option<egui::Rect>,
    pixels_per_point: f32,
) -> Result<Vec<u8>, String> {
    let (bytes, size) = crop_rgba(screenshot, viewport, pixels_per_point)?;
    let mut png = Vec::new();
    image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new(&mut png),
        &bytes,
        size[0] as u32,
        size[1] as u32,
        image::ExtendedColorType::Rgba8,
    )
    .map_err(|e| format!("Could not encode the 3D view image: {e}"))?;
    Ok(png)
}

/// Open a folder in the desktop file manager.
fn open_folder(folder: &Path) {
    let _ = std::process::Command::new("xdg-open")
        .arg(folder)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: usize, height: usize) -> egui::ColorImage {
        egui::ColorImage::filled([width, height], egui::Color32::from_rgb(10, 20, 30))
    }

    #[test]
    fn crop_follows_the_viewport_and_scale() {
        let screenshot = image(400, 300);
        let png = crop_png(
            &screenshot,
            Some(egui::Rect::from_min_size(
                egui::pos2(10.0, 10.0),
                egui::vec2(100.0, 80.0),
            )),
            2.0,
        )
        .unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 200);
        assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 160);
    }

    #[test]
    fn crop_reports_why_there_is_no_picture() {
        let screenshot = image(400, 300);
        let no_rect = crop_png(&screenshot, None, 1.0).unwrap_err();
        assert!(no_rect.contains("not visible"));
        let tiny = crop_png(
            &screenshot,
            Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(20.0, 20.0),
            )),
            1.0,
        )
        .unwrap_err();
        assert!(tiny.contains("too small"));
        // A rectangle past the window edge is clamped rather than panicking.
        let clamped = crop_png(
            &screenshot,
            Some(egui::Rect::from_min_size(
                egui::pos2(300.0, 200.0),
                egui::vec2(500.0, 500.0),
            )),
            1.0,
        )
        .unwrap();
        assert_eq!(u32::from_be_bytes(clamped[16..20].try_into().unwrap()), 100);
        assert_eq!(u32::from_be_bytes(clamped[20..24].try_into().unwrap()), 100);
    }

    #[test]
    fn screenshot_tag_is_recognised_only_on_our_requests() {
        assert!(is_ours(&egui::UserData::new(SCREENSHOT_TAG)));
        assert!(!is_ours(&egui::UserData::default()));
        assert!(!is_ours(&egui::UserData::new("something else")));
    }

    #[test]
    fn an_idle_panel_has_nothing_pending() {
        let panel = ExportPanel::new(None);
        assert!(!panel.busy());
        assert!(panel.development_settled());
        let waiting = ExportPanel::new(Some(PathBuf::from("/nonexistent")));
        assert!(!waiting.development_settled());
    }
}
