//! The native launcher: command-line flags, package discovery and the window.

use super::*;
use clap::Parser;

#[derive(Parser)]
#[command(about = "FARIS native desktop workspace", version)]
pub(crate) struct Arguments {
    /// Open a .faris study file: it supplies every input and the saved view.
    #[arg(
        value_name = "STUDY.faris",
        conflicts_with_all = [
            "scenario", "physics", "run", "bundle", "sweep_bundle", "control_scenario",
            "control_physics", "control_run", "control_bundle", "assumptions", "saved_study",
            "step", "field_view", "initial_year", "package",
        ]
    )]
    pub(crate) study: Option<PathBuf>,
    /// Open a release package folder: the study, its recorded Core evidence and
    /// Avila Core. With no study file and no input flags, a package beside this
    /// program's bin folder is opened automatically.
    #[arg(
        long,
        value_name = "DIR",
        conflicts_with_all = [
            "scenario", "physics", "run", "bundle", "sweep_bundle", "control_scenario",
            "control_physics", "control_run", "control_bundle", "assumptions", "saved_study",
            "saved_study_ready_marker",
        ]
    )]
    pub(crate) package: Option<PathBuf>,
    /// Load a scenario; otherwise use the bundled geometry demo.
    #[arg(long)]
    pub(crate) scenario: Option<PathBuf>,
    /// Capture this application's window to PNG and exit (development check).
    #[arg(long)]
    pub(crate) capture: Option<PathBuf>,
    /// Deliver a bounded development input plan through eframe's raw-input hook.
    #[arg(long, requires = "interface_check_output")]
    pub(crate) interface_check: Option<PathBuf>,
    /// Exclusive output report for the development interface check.
    #[arg(long, requires = "interface_check")]
    pub(crate) interface_check_output: Option<PathBuf>,
    /// Development: save the window as numbered PNGs into this existing, empty
    /// folder at a fixed cadence until the interface-check plan ends (README media).
    #[arg(long, hide = true, conflicts_with_all = ["capture", "benchmark_seconds"])]
    pub(crate) record_frames: Option<PathBuf>,
    /// Frames per second of frame time for --record-frames.
    #[arg(long, hide = true, default_value_t = 15.0, requires = "record_frames")]
    pub(crate) record_fps: f64,
    /// Recording length when no interface-check plan is given.
    #[arg(long, hide = true, default_value_t = 20.0, requires = "record_frames")]
    pub(crate) record_seconds: f64,
    /// Measure native frame throughput and exit. Includes startup separately.
    #[arg(long, value_parser = clap::value_parser!(f64))]
    pub(crate) benchmark_seconds: Option<f64>,
    /// Exercise camera orbit and calculated-history scrubbing during measurement.
    #[arg(long, requires = "benchmark_seconds")]
    pub(crate) benchmark_motion: bool,
    /// Development check: export the study into this existing folder once the
    /// histories are ready, then continue with --capture or exit.
    #[arg(long, hide = true)]
    pub(crate) export_on_load: Option<PathBuf>,
    /// Development check: calculate the history ensembles on a synthetic
    /// covariance so the evaluated uncertainty views can be seen. Exists only
    /// in builds with the `uncertainty-fixture` feature.
    #[cfg(feature = "uncertainty-fixture")]
    #[arg(long, hide = true)]
    pub(crate) uncertainty_fixture: bool,
    /// Initial computed-history position in calendar years.
    #[arg(long, default_value_t = 0.0)]
    pub(crate) initial_year: f64,
    /// Initial native window dimensions in logical points.
    #[arg(long, default_value_t = 1440)]
    pub(crate) window_width: u32,
    #[arg(long, default_value_t = 900)]
    pub(crate) window_height: u32,
    /// Initial interface magnification, relative to native desktop display scaling.
    #[arg(long, default_value_t = 1.0)]
    pub(crate) interface_scale: f32,
    /// External Avila Core executable used by Compile study.
    #[arg(long)]
    pub(crate) core: Option<PathBuf>,
    /// Development check: open the package without a window, wait for its Core
    /// evidence, reopen the saved studies, load the study inputs, write a JSON
    /// report to this new file and exit (non-zero if anything failed).
    #[arg(long, hide = true)]
    pub(crate) check_package: Option<PathBuf>,
    /// Headless: read this STEP file, write a draft design file (every solid listed,
    /// every material and role null), print a one-line summary and exit.
    #[arg(
        long,
        value_name = "STEP",
        conflicts_with_all = [
            "study", "package", "scenario", "capture", "interface_check", "benchmark_seconds",
            "check_package", "export_on_load", "record_frames",
        ]
    )]
    pub(crate) design_init: Option<PathBuf>,
    /// With --design-init: where to write the draft [default: beside the STEP file].
    #[arg(long, requires = "design_init")]
    pub(crate) out: Option<PathBuf>,
    /// With --design-init: the unit of a STEP file that declares none.
    #[arg(long, requires = "design_init", value_parser = ["m", "cm", "mm"])]
    pub(crate) length_unit: Option<String>,
    /// With --design-init: Python with cadquery, used to read the STEP file
    /// [env: FARIS_CAD_PYTHON].
    #[arg(long, requires = "design_init", value_name = "PYTHON")]
    pub(crate) cad_python: Option<PathBuf>,
    /// Directory for generated study records [default: runs; a package's own
    /// per-user folder in package mode].
    #[arg(long)]
    pub(crate) runs_directory: Option<PathBuf>,
    /// Physics input for an arrangement; repeat to provide both arrangements.
    #[arg(long)]
    pub(crate) physics: Vec<PathBuf>,
    /// Load a verified local run.json; repeat to compare arrangements.
    #[arg(long)]
    pub(crate) run: Vec<PathBuf>,
    /// Portable identified recorded-transport bundle; repeat for both arrangements.
    #[arg(long)]
    pub(crate) bundle: Vec<PathBuf>,
    /// Recorded-transport bundle for the allocation sweep; repeat once per allocation.
    #[arg(long)]
    pub(crate) sweep_bundle: Vec<PathBuf>,
    /// Open a faris-maintenance-result/v0.1 file in the Maintenance window.
    #[arg(long, value_name = "RESULT.json")]
    pub(crate) maintenance: Option<PathBuf>,
    /// Open the Maintenance window on its Run section, optionally prefilled by
    /// --maintenance-designs, --maintenance-assumptions, --actinv and --actinv-data.
    #[arg(long)]
    pub(crate) maintenance_run: bool,
    #[arg(long, value_name = "DESIGNS.json")]
    pub(crate) maintenance_designs: Option<PathBuf>,
    #[arg(long, value_name = "ASSUMPTIONS.json")]
    pub(crate) maintenance_assumptions: Option<PathBuf>,
    /// The actinv executable, for the Maintenance window's Run section.
    #[arg(long)]
    pub(crate) actinv: Option<PathBuf>,
    /// ACTINV data root, for the Maintenance window's Run section.
    #[arg(long)]
    pub(crate) actinv_data: Option<PathBuf>,
    /// Open the Design check window (write a draft design file from a STEP file, and check a
    /// design file).
    #[arg(long)]
    pub(crate) design_check: bool,
    /// The STEP file for the Design check window.
    #[arg(long, value_name = "STEP")]
    pub(crate) check_step: Option<PathBuf>,
    /// The design file for the Design check window.
    #[arg(long, value_name = "DESIGN.json")]
    pub(crate) check_design: Option<PathBuf>,
    /// Open the Design flux window (the neutron flux of a checked design in its magnets).
    #[arg(long)]
    pub(crate) design_flux: bool,
    /// The design file for the Design flux window.
    #[arg(long, value_name = "DESIGN.json")]
    pub(crate) flux_design: Option<PathBuf>,
    /// The `design check --report` file for the Design flux window.
    #[arg(long, value_name = "CHECK.json")]
    pub(crate) flux_check_report: Option<PathBuf>,
    /// Matched feature-free scenario to compare against the penetration.
    #[arg(long)]
    pub(crate) control_scenario: Option<PathBuf>,
    #[arg(long)]
    pub(crate) control_physics: Vec<PathBuf>,
    #[arg(long)]
    pub(crate) control_run: Vec<PathBuf>,
    #[arg(long)]
    pub(crate) control_bundle: Vec<PathBuf>,
    /// Identified fuel, maintenance, service-limit and energy assumptions.
    #[arg(long)]
    pub(crate) assumptions: Option<PathBuf>,
    /// Reopen an identified saved study via a JSON descriptor with relative
    /// case_directory, execution_report and execution_workspace paths. Repeatable.
    #[arg(long)]
    pub(crate) saved_study: Vec<PathBuf>,
    /// Wait in the saved-evidence worker for a legacy study file's packed
    /// evidence to finish unpacking. Transport exploration stays available;
    /// this marker verifies no Core claims.
    #[arg(long, requires = "saved_study")]
    pub(crate) saved_study_ready_marker: Option<PathBuf>,
    #[arg(long)]
    pub(crate) python: Option<PathBuf>,
    #[arg(long)]
    pub(crate) openmc: Option<PathBuf>,
    #[arg(long)]
    pub(crate) audit: Option<PathBuf>,
    #[arg(long)]
    pub(crate) cross_sections: Option<PathBuf>,
    /// Initial coloring for identified results loaded with --run.
    #[arg(long,value_enum,default_value_t=transport_panel::FieldView::Materials)]
    pub(crate) field_view: transport_panel::FieldView,
    /// Workflow step shown on launch.
    #[arg(long, value_enum, default_value_t = Step::Design)]
    pub(crate) step: Step,
    /// Guided tour on launch: auto plays it once, until finished or skipped.
    #[arg(long, value_enum, default_value_t = tour::TourMode::Auto)]
    pub(crate) tour: tour::TourMode,
    /// Development: calculate the history uncertainty ensembles of the study and
    /// write them to this file for the browser version, without a window.
    #[arg(long, hide = true, value_name = "FILE")]
    pub(crate) write_web_ensembles: Option<PathBuf>,
    /// Start the tour at this stop (1-based); for captures.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=tour::STOPS.len() as i64))]
    pub(crate) tour_stop: Option<u32>,
}

impl Arguments {
    /// Whether any flag or file supplies the study, which turns off finding a
    /// package automatically.
    pub(crate) fn supplies_study(&self) -> bool {
        self.study.is_some()
            || self.scenario.is_some()
            || self.control_scenario.is_some()
            || self.assumptions.is_some()
            || self.saved_study_ready_marker.is_some()
            || [
                &self.physics,
                &self.run,
                &self.bundle,
                &self.sweep_bundle,
                &self.control_physics,
                &self.control_run,
                &self.control_bundle,
                &self.saved_study,
            ]
            .iter()
            .any(|paths| !paths.is_empty())
    }
}

/// The study inputs that the command-line flags (or package mode) supply.
pub(crate) fn session_inputs_from(args: &Arguments, runs_directory: &Path) -> SessionInputs {
    SessionInputs {
        scenario: args.scenario.clone(),
        physics: args.physics.clone(),
        run: args.run.clone(),
        bundle: args.bundle.clone(),
        control_scenario: args.control_scenario.clone(),
        control_physics: args.control_physics.clone(),
        control_run: args.control_run.clone(),
        control_bundle: args.control_bundle.clone(),
        assumptions: args.assumptions.clone(),
        python: args.python.clone(),
        openmc: args.openmc.clone(),
        audit: args.audit.clone(),
        cross_sections: args.cross_sections.clone(),
        runs_directory: runs_directory.to_path_buf(),
        field_view: args.field_view,
    }
}

/// `--check-package`: everything package mode does before the window opens,
/// then the saved-study reopening and input loading the window would start,
/// reported as JSON. Used by CI on platforms where nobody watches the window.
fn check_package(
    args: &Arguments,
    session: Option<PackageSession>,
    runs_directory: &Path,
    report: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let mut problems = Vec::new();
    let mut saved = Vec::new();
    let mut loaded = serde_json::Value::Null;
    let badge = |b: &Option<study_file::EvidenceBadge>| {
        b.as_ref()
            .map_or(serde_json::Value::Null, |b| b.text.clone().into())
    };
    let Some(session) = session else {
        return Err(
            "no package found: pass --package DIR or run from a package's bin folder".into(),
        );
    };
    if let Some(failure) = &session.failure {
        problems.push(failure.text.clone());
    } else {
        if let Some(store) = &session.saved_store {
            saved = archive_panel::check_saved_studies_in_store(store);
            problems.extend(saved.iter().filter_map(|r| r.as_ref().err().cloned()));
        }
        match build_session(session_inputs_from(args, runs_directory)) {
            Ok(session) => {
                loaded = serde_json::json!({
                    "arrangements": session.manifest.variants.len(),
                    "control": session.control.is_some(),
                });
            }
            Err(error) => problems.push(error),
        }
    }
    let value = serde_json::json!({
        "schema_version": "faris-package-check/v0.1",
        "faris_version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "status": if problems.is_empty() { "PASS" } else { "FAIL" },
        "failure": badge(&session.failure),
        "evidence": badge(&session.evidence),
        "development_binary": session.development_binary.is_some(),
        "saved_studies": saved.iter().map(|r| match r {
            Ok(case) => serde_json::json!({"case_id": case, "reopened": true}),
            Err(error) => serde_json::json!({"reopened": false, "error": error}),
        }).collect::<Vec<_>>(),
        "loaded": loaded,
        "problems": problems,
        "elapsed_s": started.elapsed().as_secs_f64(),
    });
    faris_engine::atomic_file::write_new(
        report,
        (serde_json::to_string_pretty(&value)? + "\n").as_bytes(),
    )?;
    drop(session);
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!("package check failed; see {}", report.display()).into())
    }
}

/// Find and open the package this run is for, if any, and put its inputs into
/// `args` as the flags that would have supplied them.
fn open_package(args: &mut Arguments) -> Option<PackageSession> {
    let exe = std::env::current_exe();
    let (root, explicit) = match &args.package {
        Some(dir) => (dir.clone(), true),
        None if !args.supplies_study() => (exe.as_deref().ok().and_then(package::discover)?, false),
        None => return None,
    };
    let mut session = PackageSession::default();
    let opened = match &exe {
        Ok(exe) => package::open(&root, exe, explicit, args.runs_directory.as_deref()),
        Err(error) => Err(format!(
            "Why: the running program could not be located ({error}).\n\nNext step: start \
             FARIS from the bin folder of the package."
        )),
    };
    let opened = match opened {
        Ok(opened) => opened,
        Err(text) => {
            session.failure = Some(package::failure_message(&text));
            return Some(session);
        }
    };
    session.saved_store = opened.saved_store;
    session.evidence = package::evidence_message(&opened.evidence);
    if opened.development_binary {
        session.development_binary = Some(package::development_binary_message());
    }
    session.maintenance_result = opened.maintenance_result;
    args.core.get_or_insert(opened.core);
    args.bundle = opened.bundles;
    args.control_scenario = Some(opened.control_scenario);
    args.control_bundle = opened.control_bundles;
    args.assumptions = Some(opened.assumptions);
    args.sweep_bundle = opened.sweep_bundles;
    args.runs_directory = Some(opened.runs_directory);
    Some(session)
}

/// `--design-init`: the same engine function as `faris design init`, no window.
fn design_init(args: &Arguments, step: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let unit = match args.length_unit.as_deref() {
        Some("m") => Some(faris_model::design::LengthUnit::M),
        Some("cm") => Some(faris_model::design::LengthUnit::Cm),
        Some("mm") => Some(faris_model::design::LengthUnit::Mm),
        _ => None,
    };
    let tools = faris_engine::design::ToolPaths::from_flags_and_environment(
        args.cad_python.clone(),
        None,
        None,
    );
    let summary = faris_engine::design::design_init_with_tools(
        step,
        args.out.as_deref(),
        unit,
        &tools,
        &faris_engine::jobs::Cancellation::default(),
    )?;
    println!("{}", summary.line());
    Ok(())
}

pub(crate) fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = Arguments::parse();
    if let Some(step) = args.design_init.clone() {
        return design_init(&args, &step);
    }
    let package_session = open_package(&mut args);
    let runs_directory = args
        .runs_directory
        .clone()
        .unwrap_or_else(|| PathBuf::from("runs"));
    if let Some(report) = args.check_package.clone() {
        return check_package(&args, package_session, &runs_directory, &report);
    }
    if let Some(out) = args.write_web_ensembles.clone() {
        if let Some(failure) = package_session.as_ref().and_then(|s| s.failure.as_ref()) {
            return Err(format!("the package did not open: {}", failure.text).into());
        }
        return crate::web_ensembles::write(&args, &runs_directory, &out);
    }
    if args
        .benchmark_seconds
        .is_some_and(|s| !s.is_finite() || !(2.0..=120.0).contains(&s))
    {
        return Err("benchmark duration must be finite and in 2..=120 seconds".into());
    }
    if let Some(dir) = &args.record_frames {
        recorder::validate(
            dir,
            args.record_fps,
            args.record_seconds,
            args.interface_check.is_some(),
        )?;
    }
    let launch_started = Instant::now();
    let scripted = args.capture.is_some()
        || args.record_frames.is_some()
        || args.benchmark_seconds.is_some()
        || args.interface_check.is_some()
        || args.export_on_load.is_some();
    let tour_marker = tour::marker_path();
    let play_tour = tour::should_play(
        args.tour,
        scripted || args.study.is_some(),
        tour_marker.as_deref().is_some_and(|p| p.exists()),
    );
    // No first-visit sign-in prompt in scripted runs (capture, benchmark,
    // interface check, export), as the tour's auto mode; nor while it plays.
    let sign_in_prompt = !scripted;
    let interface_check = args
        .interface_check
        .as_deref()
        .map(|plan| {
            interface_check::InterfaceCheck::load(
                plan,
                args.interface_check_output
                    .as_deref()
                    .expect("required output"),
            )
        })
        .transpose()?;
    if !(980..=3840).contains(&args.window_width)
        || !(640..=2160).contains(&args.window_height)
        || !args.interface_scale.is_finite()
        || !(0.75..=2.0).contains(&args.interface_scale)
    {
        return Err(
            "window size must be 980..3840 × 640..2160 points; interface scale must be 0.75..2"
                .into(),
        );
    }
    let mut inputs = study_file::StudyInputs::from_arguments(&args);
    if let Some(store) = package_session
        .as_ref()
        .and_then(|s| s.saved_store.as_ref())
    {
        inputs = inputs.with_saved_store(store);
    }
    let session_inputs = if args.study.is_some() {
        SessionInputs::empty(runs_directory.clone())
    } else {
        session_inputs_from(&args, &runs_directory)
    };
    let Session {
        manifest,
        control,
        transport,
        history,
    } = build_session(session_inputs).map_err(std::io::Error::other)?;
    #[cfg(feature = "uncertainty-fixture")]
    let history = {
        let mut history = history;
        if args.uncertainty_fixture {
            history.enable_uncertainty_fixture();
        }
        history
    };
    if !args.initial_year.is_finite()
        || !(0.0..=manifest.horizon_years).contains(&args.initial_year)
    {
        return Err("initial year must be finite and within the scenario horizon".into());
    }
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        depth_buffer: 32,
        multisampling: 0,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([args.window_width as f32, args.window_height as f32])
            .with_min_inner_size([980.0, 640.0])
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        study_file::window_title(None, false).as_str(),
        options,
        Box::new(move |cc| {
            let state = cc
                .wgpu_render_state
                .as_ref()
                .ok_or_else(|| graphics_failure("no wgpu render state was created"))?;
            viewport::initialize(state);
            eprintln!("FARIS graphics: {:?}", state.adapter.get_info());
            let mut visuals = egui::Visuals::dark();
            visuals.panel_fill = egui::Color32::from_rgb(37, 39, 44);
            visuals.selection.bg_fill = egui::Color32::from_rgb(67, 78, 125);
            cc.egui_ctx.set_visuals(visuals);
            cc.egui_ctx.all_styles_mut(crate::apply_text_sizes);
            // An explicit --interface-scale wins; otherwise the size saved last time.
            // Scripted runs (capture, recording, checks) always use the option.
            let scripted = args.capture.is_some()
                || args.record_frames.is_some()
                || interface_check.is_some()
                || args.benchmark_seconds.is_some();
            let scale = if scripted || args.interface_scale != crate::interface_prefs::DEFAULT_SCALE
            {
                args.interface_scale
            } else {
                crate::interface_prefs::path()
                    .and_then(|path| {
                        crate::interface_prefs::load(
                            &path,
                            crate::interface_prefs::old_path().as_deref(),
                            &|name| std::env::var_os(name),
                        )
                    })
                    .unwrap_or(args.interface_scale)
            };
            cc.egui_ctx.set_zoom_factor(scale);
            let sweep = (!args.sweep_bundle.is_empty()).then(|| {
                let (paths, runs) = (args.sweep_bundle.clone(), runs_directory.clone());
                sweep_panel::SweepPanel::load_in_background(&cc.egui_ctx, move || {
                    load_sweep(&paths, &runs)
                })
            });
            let mut app = FarisApp::new(
                manifest,
                args.capture,
                args.core,
                runs_directory.clone(),
                transport,
                control,
                history,
            )?;
            app.sweep = sweep;
            app.maintenance.prefill_run(
                args.maintenance_designs.as_deref(),
                args.maintenance_assumptions.as_deref(),
                args.actinv.as_deref(),
                args.actinv_data.as_deref(),
            );
            if args.maintenance_run {
                app.maintenance.open = true;
            }
            // One set of tool paths for both design windows, kept between sessions.
            let tools = design_tools::ToolsModel::load().shared();
            app.design_flux.use_tools(tools.clone());
            app.design_check.use_tools(tools.clone());
            app.tools = Some(tools);
            app.design_flux.prefill(
                args.flux_design.as_deref(),
                args.flux_check_report.as_deref(),
            );
            app.design_flux.open = args.design_flux;
            app.design_check
                .prefill(args.check_step.as_deref(), args.check_design.as_deref());
            app.design_check.open = args.design_check;
            if let Some(path) = args.maintenance.clone() {
                app.maintenance.load_in_background(&cc.egui_ctx, path);
            }
            app.file = study_file::FileState::new(inputs, runs_directory.clone());
            if let Some(session) = package_session {
                if let Some(failure) = &session.failure {
                    app.message = failure.text.clone();
                }
                if let Some(recorded) = session.maintenance_result.clone() {
                    app.maintenance.set_recorded(recorded);
                }
                if let Some(store) = session.saved_store.clone() {
                    app.file.package_store = Some(store.path.clone());
                    app.study.archive.queue_store(store);
                }
                app.package = Some(session);
            }
            if let Some(path) = args.study.clone() {
                app.file.request_open(path);
            }
            app.export = export_panel::ExportPanel::new(args.export_on_load);
            app.started = launch_started;
            app.year = args.initial_year;
            app.step = args.step;
            app.interface_check = interface_check;
            app.recorder = args.record_frames.map(|dir| {
                recorder::Recorder::new(
                    dir,
                    args.record_fps,
                    args.record_seconds,
                    app.interface_check.is_some(),
                )
            });
            app.tour_marker = tour_marker;
            app.suite = Some(avila_account::ui_desktop::DesktopSuite::new(
                &cc.egui_ctx,
                "faris",
            ));
            app.sign_in_prompt = sign_in_prompt;
            if play_tour {
                app.start_tour(args.tour_stop.map_or(0, |n| n as usize - 1));
            }
            app.study
                .archive
                .queue_descriptors(args.saved_study, args.saved_study_ready_marker)
                .map_err(std::io::Error::other)?;
            app.benchmark = args.benchmark_seconds.map(|duration| Benchmark {
                duration,
                motion: args.benchmark_motion,
                launch_started,
                ready_at: None,
                frame_times: Vec::new(),
                completed: false,
            });
            Ok(Box::new(app))
        }),
    )
    .map_err(|error| graphics_failure(&error.to_string()))?;
    Ok(())
}

/// What the user sees when the graphics adapter or window cannot start: what
/// failed, why it matters, and what to try next (PLAT-013).
fn graphics_failure(detail: &str) -> String {
    use crate::error_catalogue::GRAPHICS_START as entry;
    format!(
        "{} ({detail}) {} Why: {} Next step: {}",
        entry.what, entry.code, entry.why, entry.fix
    )
}

/// The 256 px tile icon for the window and taskbar.
fn app_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/faris-icon-256.png"))
        .expect("the bundled application icon is a valid PNG")
}

#[cfg(test)]
mod tests {
    use super::graphics_failure;

    // Verifies: PLAT-013
    #[test]
    fn a_graphics_failure_names_the_cause_and_a_next_step() {
        let text = graphics_failure("no suitable adapter");
        assert!(text.contains("no suitable adapter"));
        assert!(text.contains("Next step"));
        assert!(text.contains("driver"));
    }
}
