//! Browser entry point. The app is the same one the desktop runs: this module
//! hosts it on a canvas, brings the bundled recorded study into memory so the
//! ordinary loaders can read it by path, takes files from the picker or a
//! drop as bytes, and hands exports to the browser as downloads.

#![cfg(target_arch = "wasm32")]

use crate::{
    FarisApp, PackageSession, Session, SessionInputs, build_session, export_panel, study_file,
    sweep_panel, viewport,
};
use eframe::egui;
use faris_report::{ExportOutcome, ReportInput};
use serde::Deserialize;
use std::{
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::JsFuture;

/// The page's context, so a dropped frame can ask for the next one.
static SURFACE_CONTEXT: Mutex<Option<egui::Context>> = Mutex::new(None);
static DROPPED_FRAMES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// What to do when the browser gives no surface to draw on (typically after
/// the canvas size or the display scale changed). The default drops the frame
/// and draws nothing more until some input arrives, which leaves the page
/// looking frozen; this asks for the next frame at once.
fn on_surface_status(
    status: &eframe::wgpu::CurrentSurfaceTexture,
) -> eframe::egui_wgpu::SurfaceErrorAction {
    use eframe::egui_wgpu::SurfaceErrorAction as Action;
    use eframe::wgpu::CurrentSurfaceTexture as Status;
    let action = match status {
        Status::Outdated => Action::Reconfigure,
        Status::Lost => Action::RecreateSurface,
        _ => Action::SkipFrame,
    };
    if !matches!(status, Status::Occluded) {
        DROPPED_FRAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Ok(held) = SURFACE_CONTEXT.lock()
            && let Some(ctx) = held.as_ref()
        {
            ctx.request_repaint();
        }
    }
    action
}

/// The uncertainty ranges that came with the bundled study, kept so a study the
/// user opens whose inputs are the same (same key) shows them too.
static DEMO_ENSEMBLES: Mutex<Vec<faris_study::StoredEnsemble>> = Mutex::new(Vec::new());

/// The bundled study's precomputed ranges; empty when none were loaded.
pub(crate) fn demo_ensembles() -> Vec<faris_study::StoredEnsemble> {
    DEMO_ENSEMBLES
        .lock()
        .map(|held| held.clone())
        .unwrap_or_default()
}

const CANVAS_ID: &str = "faris_canvas";
const LOADING_ID: &str = "faris-loading";
/// Where the bundled study lives on the page's own origin.
const DEMO_BASE: &str = "demo/";
/// The in-memory folder the study files are read from.
const DEMO_ROOT: &str = "/demo";
const DISCLAIMER_KEY: &str = "faris-browser-notice-seen";
const TOUR_KEY: &str = "faris-tour-completed";

/// The list of files that make up the bundled study.
#[derive(Deserialize)]
struct DemoManifest {
    schema: String,
    /// Needed before the first frame: the port and no-port arrangements and
    /// the operating assumptions.
    core: Vec<DemoFile>,
    /// The allocation sweep's bundles, fetched after the first frame.
    sweep: Vec<DemoFile>,
    /// The recorded maintenance result, if the study carries one.
    maintenance: Option<DemoMaintenance>,
    /// The history uncertainty ranges calculated beforehand.
    #[serde(default)]
    ensembles: Option<DemoFile>,
}

#[derive(Deserialize)]
struct DemoFile {
    path: String,
    bytes: u64,
}

#[derive(Deserialize)]
struct DemoMaintenance {
    path: String,
    sha256: String,
    bytes: u64,
}

/// Local storage, wrapped: a blocked or full store reads as empty and writes
/// nothing, so a failure never stops the page.
fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

pub fn storage_get(key: &str) -> Option<String> {
    storage()?.get_item(key).ok().flatten()
}

pub fn storage_set(key: &str, value: &str) {
    if let Some(store) = storage() {
        let _ = store.set_item(key, value);
    }
}

fn set_loading(text: &str) {
    if let Some(element) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(LOADING_ID))
    {
        element.set_text_content(Some(text));
    }
}

fn show_boot_error(message: &str) {
    if let Some(element) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(LOADING_ID))
    {
        element.set_text_content(Some(&format!("FARIS could not start: {message}")));
    }
}

fn js_text(value: JsValue) -> String {
    value
        .as_string()
        .or_else(|| {
            js_sys::Reflect::get(&value, &JsValue::from_str("message"))
                .ok()
                .and_then(|m| m.as_string())
        })
        .unwrap_or_else(|| format!("{value:?}"))
}

/// Start fetching `url`; the future gives the body bytes.
fn fetch(url: String) -> impl std::future::Future<Output = Result<Vec<u8>, String>> {
    let promise = web_sys::window().map(|w| w.fetch_with_str(&url));
    async move {
        let promise = promise.ok_or("no window")?;
        let response: web_sys::Response = JsFuture::from(promise)
            .await
            .map_err(|e| format!("{url}: {}", js_text(e)))?
            .dyn_into()
            .map_err(|_| format!("{url}: not a response"))?;
        if !response.ok() {
            return Err(format!("{url}: HTTP {}", response.status()));
        }
        let buffer = JsFuture::from(response.array_buffer().map_err(js_text)?)
            .await
            .map_err(|e| format!("{url}: {}", js_text(e)))?;
        Ok(js_sys::Uint8Array::new(&buffer).to_vec())
    }
}

/// Fetch every file in parallel, then put each into the in-memory folder.
async fn load_files(files: &[DemoFile], label: &str) -> Result<(), String> {
    let total: u64 = files.iter().map(|f| f.bytes).sum();
    let started: Vec<_> = files
        .iter()
        .map(|f| (f, fetch(format!("{DEMO_BASE}{}", f.path))))
        .collect();
    let mut done = 0u64;
    for (file, pending) in started {
        let bytes = pending.await?;
        if bytes.len() as u64 != file.bytes {
            return Err(format!(
                "{} arrived as {} bytes; the study lists {}",
                file.path,
                bytes.len(),
                file.bytes
            ));
        }
        done += file.bytes;
        faris_engine::vfs::install(&PathBuf::from(DEMO_ROOT).join(&file.path), bytes);
        set_loading(&format!(
            "{label}: {:.0} of {:.0} MB",
            done as f64 / 1e6,
            total as f64 / 1e6
        ));
    }
    Ok(())
}

fn demo_path(relative: &str) -> PathBuf {
    PathBuf::from(DEMO_ROOT).join(relative)
}

/// The sweep bundles, once fetched after the first frame.
static SWEEP_READY: Mutex<Option<Result<Vec<PathBuf>, String>>> = Mutex::new(None);

/// Whether the page was opened with `?verify=1`, which publishes the app's
/// state for the smoke test.
fn verify_requested() -> bool {
    web_sys::window()
        .and_then(|w| w.location().search().ok())
        .is_some_and(|search| search.contains("verify=1"))
}

impl FarisApp {
    /// Publish a small status object as `window.__farisStatus` (smoke test).
    fn publish_status(&self) {
        let Some(window) = web_sys::window() else {
            return;
        };
        let status = serde_json::json!({
            "frames": self.frames,
            "noticeOpen": self.notice.open,
            "tourActive": self.tour.active,
            "step": self.step.name(),
            "hasResults": self.transport.has_results(),
            "historyAssumptions": self.history.assumptions.is_some(),
            "historyPending": self.history.is_pending(),
            "pairedLoaded": self.paired.is_some(),
            "sweepStarted": self.sweep.is_some(),
            "sweepPending": self.sweep.as_ref().is_some_and(|s| s.is_pending()),
            "sweepReady": self.sweep.as_ref().is_some_and(|s| !s.is_pending()),
            "droppedFrames": DROPPED_FRAMES.load(std::sync::atomic::Ordering::Relaxed),
            "allocations": self.manifest.variants.len(),
            "message": self.message,
            "slowJobs": crate::thread::slow_jobs(),
        });
        let _ = js_sys::Reflect::set(
            &window,
            &JsValue::from_str("__farisStatus"),
            &js_sys::JSON::parse(&status.to_string()).unwrap_or(JsValue::NULL),
        );
    }

    /// Per-frame work only the browser has: hand over the sweep when its files
    /// have arrived, and open files dropped on the page.
    pub(crate) fn poll_web(&mut self, ctx: &egui::Context) {
        if verify_requested() {
            self.publish_status();
        }
        let requests: Vec<_> = OPEN_REQUESTS
            .lock()
            .map(|mut queue| std::mem::take(&mut *queue))
            .unwrap_or_default();
        for request in requests {
            match request {
                Ok(path) => self.begin_open(ctx, path),
                Err(error) => self.message = error,
            }
        }
        let ready = SWEEP_READY.lock().ok().and_then(|mut slot| slot.take());
        match ready {
            Some(Ok(paths)) => {
                let runs = PathBuf::from("/faris-runs");
                self.sweep = Some(sweep_panel::SweepPanel::load_stepwise(paths, runs));
                ctx.request_repaint();
            }
            Some(Err(error)) => {
                self.message = format!("The allocation sweep could not be loaded: {error}");
            }
            None => {}
        }
    }
}

/// Study files the picker or a drop has read, waiting to be opened.
static OPEN_REQUESTS: Mutex<Vec<Result<PathBuf, String>>> = Mutex::new(Vec::new());

/// Where an uploaded file is kept in memory. Its bytes never leave the tab.
fn upload_path(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    PathBuf::from(format!(
        "/upload/{}/{safe}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn queue_open(request: Result<PathBuf, String>, ctx: &egui::Context) {
    if let Ok(mut queue) = OPEN_REQUESTS.lock() {
        queue.push(request);
    }
    ctx.request_repaint();
}

/// Ask the browser for a study file and queue it for opening.
pub fn pick_study(ctx: egui::Context) {
    wasm_bindgen_futures::spawn_local(async move {
        let Some(handle) = rfd::AsyncFileDialog::new()
            .add_filter("FARIS study", &["faris"])
            .pick_file()
            .await
        else {
            return;
        };
        let name = handle.file_name();
        let bytes = handle.read().await;
        let path = upload_path(&name);
        faris_engine::vfs::install(&path, bytes);
        queue_open(Ok(path), &ctx);
    });
}

/// Read a file dropped on the page and queue it for opening.
pub fn read_dropped(file: egui::DroppedFileHandle, ctx: egui::Context) {
    wasm_bindgen_futures::spawn_local(async move {
        let name = file.path().to_string_lossy().into_owned();
        match file.bytes_async().await {
            Ok(bytes) => {
                let path = upload_path(&name);
                faris_engine::vfs::install(&path, bytes);
                queue_open(Ok(path), &ctx);
            }
            Err(error) => queue_open(Err(format!("{name} could not be read: {error}")), &ctx),
        }
    });
}

/// Trigger a browser download for in-memory bytes.
pub fn download_bytes(name: &str, bytes: &[u8]) -> Result<(), String> {
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or("no document")?;
    let parts = js_sys::Array::new();
    parts.push(&js_sys::Uint8Array::from(bytes));
    let blob = web_sys::Blob::new_with_u8_array_sequence(&parts).map_err(js_text)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(js_text)?;
    let anchor = document.create_element("a").map_err(js_text)?;
    anchor.set_attribute("href", &url).map_err(js_text)?;
    anchor.set_attribute("download", name).map_err(js_text)?;
    if let Ok(element) = anchor.dyn_into::<web_sys::HtmlElement>() {
        element.click();
    }
    let _ = web_sys::Url::revoke_object_url(&url);
    Ok(())
}

/// Build the whole export in memory, pack it into one stored zip archive and
/// download it.
pub fn download_export(input: &ReportInput) -> Result<ExportOutcome, String> {
    let export = faris_report::export_files(input).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, bytes) in &export.files {
        archive
            .start_file(format!("{}/{path}", export.folder_name), options)
            .map_err(|e| e.to_string())?;
        archive.write_all(bytes).map_err(|e| e.to_string())?;
    }
    let bytes = archive.finish().map_err(|e| e.to_string())?.into_inner();
    let name = format!("{}.zip", export.folder_name);
    download_bytes(&name, &bytes)?;
    Ok(ExportOutcome {
        folder: PathBuf::from(name),
        files: export.records,
        view_image_included: export.view_image_included,
    })
}

#[wasm_bindgen(start)]
pub async fn start_web() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or_else(|| JsValue::from_str("no window/document"))?;
    let canvas = document
        .get_element_by_id(CANVAS_ID)
        .and_then(|el| el.dyn_into::<web_sys::HtmlCanvasElement>().ok())
        .ok_or_else(|| JsValue::from_str("no faris_canvas element"))?;
    match boot(canvas).await {
        Ok(()) => Ok(()),
        Err(message) => {
            show_boot_error(&message);
            Err(JsValue::from_str(&message))
        }
    }
}

async fn boot(canvas: web_sys::HtmlCanvasElement) -> Result<(), String> {
    set_loading("Loading the recorded study…");
    let manifest_bytes = fetch(format!("{DEMO_BASE}manifest.json")).await?;
    let manifest: DemoManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| format!("the study list is unreadable: {e}"))?;
    if manifest.schema != "faris-web-demo/v1" {
        return Err(format!("unknown study list version {:?}", manifest.schema));
    }
    mark("boot: manifest read");
    load_files(&manifest.core, "Loading the recorded study").await?;
    mark("boot: files fetched");
    set_loading("Checking the recorded transport…");
    // Let the page paint the message before the (synchronous) checks run.
    yield_to_page().await;

    let runs = PathBuf::from("/faris-runs");
    let mut inputs = SessionInputs::empty(runs.clone());
    // The bundles are checked one at a time below, with the page painting between.
    inputs.scenario = Some(demo_path("port/scenario.json"));
    inputs.control_scenario = Some(demo_path("control/scenario.json"));
    inputs.assumptions = Some(demo_path("operating-assumptions.json"));
    let Session {
        manifest: reactor,
        mut control,
        mut transport,
        mut history,
    } = build_session(inputs)?;
    for variant in ["reference", "breeder-emphasis"] {
        transport.add_bundle(demo_path(&format!(
            "port/bundles/{variant}.transport-bundle.json"
        )))?;
        yield_to_page().await;
        if let Some((_, panel)) = control.as_mut() {
            panel.add_bundle(demo_path(&format!(
                "control/bundles/{variant}.transport-bundle.json"
            )))?;
            yield_to_page().await;
        }
    }
    mark("boot: session built");
    if let Some(file) = &manifest.ensembles {
        load_files(std::slice::from_ref(file), "Loading the uncertainty ranges").await?;
        let path = demo_path(&file.path);
        let bytes = faris_engine::vfs::read(&path).map_err(|e| e.to_string())?;
        faris_engine::vfs::remove(&path);
        let ranges: crate::browser::PrecomputedEnsembles = serde_json::from_slice(&bytes)
            .map_err(|e| format!("the uncertainty ranges are unreadable: {e}"))?;
        if ranges.schema != crate::browser::ENSEMBLES_SCHEMA {
            return Err(format!(
                "unknown uncertainty file version {:?}",
                ranges.schema
            ));
        }
        let stored: Vec<_> = ranges
            .ensembles
            .into_iter()
            .map(|e| faris_study::StoredEnsemble {
                scenario_sha256: e.scenario_sha256,
                variant: e.variant,
                key: e.key,
                ensemble: e.ensemble,
            })
            .collect();
        if let Ok(mut held) = DEMO_ENSEMBLES.lock() {
            *held = stored.clone();
        }
        history.restore_ensembles(stored);
    }
    // The checked records are in memory now; free the raw bundle files.
    for file in &manifest.core {
        if file.path.ends_with(".transport-bundle.json") {
            faris_engine::vfs::remove(&demo_path(&file.path));
        }
    }
    let maintenance = manifest.maintenance.as_ref().map(|m| {
        (
            DemoFile {
                path: m.path.clone(),
                bytes: m.bytes,
            },
            m.sha256.clone(),
        )
    });
    if let Some((file, _)) = &maintenance {
        load_files(std::slice::from_ref(file), "Loading the maintenance result").await?;
    }
    let recorded = maintenance.map(|(file, sha256)| crate::package::RecordedMaintenance {
        path: demo_path(&file.path),
        sha256,
    });

    let tour_seen = storage_get(TOUR_KEY).is_some();
    let notice_seen = storage_get(DISCLAIMER_KEY).is_some();
    let sweep_files = manifest.sweep;

    let slot: Arc<Mutex<Option<(egui::Context, f64)>>> = Arc::new(Mutex::new(None));
    let mut options = eframe::WebOptions {
        depth_buffer: 32,
        ..Default::default()
    };
    options.wgpu_options.on_surface_status = Arc::new(on_surface_status);
    let creator_slot = slot.clone();
    eframe::WebRunner::new()
        .start(
            canvas,
            options,
            Box::new(move |cc| {
                let state = cc.wgpu_render_state.as_ref().ok_or(
                    "the 3D viewport needs WebGPU or WebGL2, and this browser offers neither",
                )?;
                viewport::initialize(state);
                if let Ok(mut held) = SURFACE_CONTEXT.lock() {
                    *held = Some(cc.egui_ctx.clone());
                }
                let mut visuals = egui::Visuals::dark();
                visuals.panel_fill = egui::Color32::from_rgb(37, 39, 44);
                visuals.selection.bg_fill = egui::Color32::from_rgb(67, 78, 125);
                cc.egui_ctx.set_visuals(visuals);
                mark("boot: creating the app");
                let mut app = FarisApp::new(
                    reactor,
                    None,
                    None,
                    runs.clone(),
                    transport,
                    control,
                    history,
                )?;
                if let Some(recorded) = recorded {
                    app.maintenance.set_recorded(recorded);
                }
                app.file = study_file::FileState::new(
                    study_file::StudyInputs {
                        unsaveable: Some(crate::browser::desktop_only(
                            "Saving a .faris study file",
                        )),
                        ..Default::default()
                    },
                    runs.clone(),
                );
                app.export = export_panel::ExportPanel::new(None);
                app.tour_marker = Some(PathBuf::from(TOUR_KEY));
                app.package = Some(PackageSession::default());
                app.suite = Some(crate::Suite::new(&cc.egui_ctx, "faris"));
                app.sign_in_prompt = true;
                if !notice_seen {
                    app.notice.open_first_visit();
                    storage_set(DISCLAIMER_KEY, "1");
                } else if !tour_seen {
                    app.start_tour(0);
                }
                if let Ok(mut slot) = creator_slot.lock() {
                    *slot = Some((cc.egui_ctx.clone(), 0.0));
                }
                Ok(Box::new(app))
            }),
        )
        .await
        .map_err(js_text)?;
    if let Some(element) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(LOADING_ID))
    {
        element.remove();
    }
    // The sweep is large; it arrives after the first frame.
    let context = slot
        .lock()
        .ok()
        .and_then(|s| s.as_ref().map(|(c, _)| c.clone()));
    wasm_bindgen_futures::spawn_local(async move {
        let result = load_files(&sweep_files, "Loading the allocation sweep")
            .await
            .map(|()| sweep_files.iter().map(|f| demo_path(&f.path)).collect());
        if let Ok(mut slot) = SWEEP_READY.lock() {
            *slot = Some(result);
        }
        if let Some(context) = context {
            context.request_repaint();
        }
    });
    Ok(())
}

/// A named point on the page's performance timeline, for the smoke test.
fn mark(name: &str) {
    if let Some(performance) = web_sys::window().and_then(|w| w.performance()) {
        let _ = performance.mark(name);
    }
}

/// Resolve on the next browser task, so queued painting happens first.
async fn yield_to_page() {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0);
        }
    });
    let _ = JsFuture::from(promise).await;
}
