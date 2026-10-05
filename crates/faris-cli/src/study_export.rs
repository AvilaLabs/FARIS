//! `faris study-file export`: the study export folder from a `.faris` file,
//! without the desktop. The file is read with the fail-closed reader, the
//! recorded transport is validated as the desktop validates it, and the
//! operating histories are recalculated from the recorded assumptions and
//! rates. Which data enters the report is `faris_report::assemble_report_input`,
//! the same function the desktop's export calls.

use crate::study_file::convert;
use faris_engine::{
    DemoManifest, build_manifest,
    core_evidence::{load_recorded_bundle, scenario_from_bundle_file},
    history::{HistoryResult, TransportDrivingRates, run_operating_history_cancellable},
    jobs::Cancellation,
    presets::{operating_presets, resolve_view, service_limit},
    reactor::{ReactorRun, read_json_bytes},
    sweep::{collect_points, summarize_sweep},
};
use faris_model::{LoadedScenario, history::OperatingHistoryAssumptions};
use faris_report::{
    LoadedArrangement, NO_VIEW_ON_COMMAND_LINE, ReportContext, ReportInput, StudyFileStamp,
    SweepInput, assemble_report_input, study_name_from_path,
};
use faris_study::{ArrangementFiles, StudyReader, ViewState};
use std::{collections::BTreeMap, path::Path};

type Failure = Box<dyn std::error::Error>;

/// One arrangement as the desktop holds it after opening a study.
struct Loaded {
    manifest: DemoManifest,
    records: BTreeMap<String, ReactorRun>,
    // Keeps the replay files of the loaded bundles alive.
    _directories: Vec<tempfile::TempDir>,
}

fn load_records(
    scenario: &LoadedScenario,
    bundles: &[std::path::PathBuf],
) -> Result<(BTreeMap<String, ReactorRun>, Vec<tempfile::TempDir>), Failure> {
    let mut records = BTreeMap::new();
    let mut directories = Vec::new();
    for path in bundles {
        let loaded = load_recorded_bundle(path, scenario)?;
        if records
            .insert(loaded.record.variant_id.clone(), loaded.record)
            .is_some()
        {
            return Err("Duplicate transport record for an arrangement.".into());
        }
        directories.push(loaded.directory);
    }
    Ok((records, directories))
}

fn load_arrangement(files: &ArrangementFiles) -> Result<Loaded, Failure> {
    let scenario = match (&files.scenario, files.bundles.first()) {
        (Some(path), _) => LoadedScenario::load(path)?,
        (None, Some(bundle)) => scenario_from_bundle_file(bundle)?,
        (None, None) => return Err("an arrangement in the study has no scenario".into()),
    };
    let manifest = build_manifest(&scenario)?;
    let (records, directories) = load_records(&scenario, &files.bundles)?;
    Ok(Loaded {
        manifest,
        records,
        _directories: directories,
    })
}

/// Calculate one history per accepted record, as the desktop does for every
/// arrangement it holds.
fn histories(
    loaded: &[&Loaded],
    assumptions: &OperatingHistoryAssumptions,
) -> Result<BTreeMap<(String, String), HistoryResult>, Failure> {
    let cancellation = Cancellation::default();
    let mut results = BTreeMap::new();
    for arrangement in loaded {
        for run in arrangement.records.values() {
            let Some(normalized) = &run.normalized else {
                continue;
            };
            let raw = run
                .raw_artifact_sha256
                .as_deref()
                .ok_or("A transport record has no raw identity.")?;
            let rates = TransportDrivingRates::from_normalized(
                normalized,
                arrangement.manifest.fusion_power_mw,
                raw,
            )?;
            let history = run_operating_history_cancellable(assumptions, &rates, &cancellation)?;
            results.insert(
                (run.scenario_sha256.clone(), run.variant_id.clone()),
                history,
            );
        }
    }
    Ok(results)
}

/// The allocation sweep's validated records.
struct LoadedSweep {
    scenario: LoadedScenario,
    fusion_power_mw: f64,
    records: BTreeMap<String, ReactorRun>,
    _directories: Vec<tempfile::TempDir>,
}

/// A study file read, verified and validated; nothing calculated yet.
struct StudyData {
    file: std::path::PathBuf,
    view: ViewState,
    current: Loaded,
    paired: Option<Loaded>,
    sweep: Option<LoadedSweep>,
    assumptions: OperatingHistoryAssumptions,
}

fn read_study(file: &Path) -> Result<StudyData, Failure> {
    let mut reader = StudyReader::open(file).map_err(convert)?;
    let workspace = tempfile::Builder::new().prefix("faris-export-").tempdir()?;
    let near = file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let files = reader
        .materialize(workspace.path(), Some(near))
        .map_err(convert)?;
    let view = reader.manifest.view.clone();

    // The desktop's first arrangement is the port when there is one; a second
    // exists only when both do.
    let first = files.port.as_ref().or(files.control.as_ref());
    let second = files.port.as_ref().and(files.control.as_ref());
    let current = first
        .map(load_arrangement)
        .transpose()?
        .ok_or("this study file records no arrangement")?;
    let paired = second.map(load_arrangement).transpose()?;

    let assumptions_path = files
        .assumptions
        .as_ref()
        .ok_or("This study has no operating assumptions, so there are no histories to export.")?;
    let loaded_assumptions: OperatingHistoryAssumptions =
        serde_json::from_slice(&read_json_bytes(assumptions_path).map_err(|e| e.to_string())?)?;
    loaded_assumptions.validate()?;
    let sweep = if files.sweep.is_empty() {
        None
    } else {
        let scenario = scenario_from_bundle_file(&files.sweep[0])?;
        let fusion_power_mw = build_manifest(&scenario)?.fusion_power_mw;
        let (records, directories) = load_records(&scenario, &files.sweep)?;
        Some(LoadedSweep {
            scenario,
            fusion_power_mw,
            records,
            _directories: directories,
        })
    };
    Ok(StudyData {
        file: file.to_path_buf(),
        view,
        current,
        paired,
        sweep,
        assumptions: loaded_assumptions,
    })
}

/// Recalculate the histories and sweep summaries for the recorded view and
/// assemble the report input: what the desktop would export from the same
/// file once everything has calculated, minus the 3D view.
fn report_input(data: &StudyData, generated_unix_s: i64) -> Result<ReportInput, Failure> {
    let presets = operating_presets(&data.assumptions)?;
    let (index, assumptions) = resolve_view(
        &presets,
        data.view.preset.as_deref(),
        data.view.what_if.as_ref(),
    )
    .ok_or("no operating-assumption preset is available")?;
    assumptions.validate()?;

    let mut all = vec![&data.current];
    all.extend(data.paired.as_ref());
    let calculated = histories(&all, &assumptions)?;

    let sweep = data
        .sweep
        .as_ref()
        .map(|sweep| -> Result<SweepInput, Failure> {
            let (points, rates) =
                collect_points(&sweep.records, &sweep.scenario, sweep.fusion_power_mw)?;
            let summaries = summarize_sweep(&rates, &assumptions, &Cancellation::default())?;
            Ok(SweepInput {
                points,
                summaries: summaries.into_iter().map(Some).collect(),
            })
        })
        .transpose()?;

    fn arrangement(loaded: &Loaded) -> LoadedArrangement<'_> {
        LoadedArrangement {
            manifest: &loaded.manifest,
            records: &loaded.records,
        }
    }
    Ok(assemble_report_input(
        arrangement(&data.current),
        data.paired.as_ref().map(arrangement),
        |scenario, variant| calculated.get(&(scenario.to_owned(), variant.to_owned())),
        ReportContext {
            study_name: study_name_from_path(Some(&data.file)),
            sweep,
            preset_label: presets[index].name.clone(),
            preset_magnet_limit: service_limit(&presets[index].assumptions, "magnets"),
            fusion_power_mw: data.current.manifest.fusion_power_mw,
            study_file: StudyFileStamp::from_path(&data.file),
            view_image_note: Some(NO_VIEW_ON_COMMAND_LINE.into()),
            generated_unix_s,
        },
    ))
}

/// Write the export folder for `file` under `parent` and describe it.
pub fn export(file: &Path, parent: &Path) -> Result<serde_json::Value, Failure> {
    if !parent.is_dir() {
        return Err(format!("{} is not an existing folder", parent.display()).into());
    }
    let input = report_input(&read_study(file)?, faris_report::now_unix_s())?;
    let outcome = faris_report::export_study(&input, parent)?;
    Ok(serde_json::json!({
        "folder": outcome.folder,
        "files": outcome.files.len(),
        "view_image_included": outcome.view_image_included,
        "study_file_sha256": input.study_file.map(|s| s.sha256),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use faris_engine::reactor::FieldMesh;
    use faris_model::physics::ScientificScope;

    fn run(variant: &str, scenario_sha256: &str) -> ReactorRun {
        ReactorRun {
            schema_version: "faris-reactor-run/v0.1".into(),
            scenario_sha256: scenario_sha256.into(),
            variant_id: variant.into(),
            physics_sha256: String::new(),
            input_sha256: String::new(),
            adapter_sha256: String::new(),
            python_sha256: String::new(),
            openmc_sha256: String::new(),
            cross_sections_sha256: String::new(),
            audit_sha256: String::new(),
            scientific_scope: ScientificScope::ConditionalDesignPrediction {
                description: "test".into(),
            },
            sampling: faris_engine::reactor::SamplingPlan::default(),
            mesh: FieldMesh {
                id: "test".into(),
                dimensions: [1, 1, 1],
                lower_left_m: [0.0; 3],
                upper_right_m: [1.0; 3],
            },
            mesh_preflight: None,
            execution: None,
            import_error: None,
            raw_artifact_sha256: None,
            normalized: None,
            transport_spectra_sha256: None,
            worker_result_sha256: None,
            normalized_spectra: None,
            sampling_precision_summary: None,
            scientific_qualification: "NOT_EVALUATED".into(),
            notice: "test".into(),
        }
    }

    fn arrangement() -> Loaded {
        let scenario = LoadedScenario::from_bytes(include_bytes!(
            "../../../scenarios/arc-inspired/scenario.json"
        ))
        .unwrap();
        let records = ["reference", "breeder-emphasis"]
            .into_iter()
            .map(|v| (v.to_owned(), run(v, &scenario.source_sha256)))
            .collect();
        Loaded {
            manifest: build_manifest(&scenario).unwrap(),
            records,
            _directories: Vec::new(),
        }
    }

    fn manifest_without_time(folder: &Path) -> serde_json::Value {
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(folder.join("export-manifest.json")).unwrap())
                .unwrap();
        manifest.as_object_mut().unwrap().remove("generated_utc");
        manifest
    }

    // Verifies: COL-007
    #[test]
    fn cli_and_desktop_assembly_export_identical_csv_and_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("parity.faris");
        std::fs::write(&file, b"stand-in for the study file bytes").unwrap();
        let loaded: OperatingHistoryAssumptions = serde_json::from_slice(include_bytes!(
            "../../../scenarios/arc-inspired/demo-operating-assumptions.json"
        ))
        .unwrap();
        let data = StudyData {
            file: file.clone(),
            view: ViewState::default(),
            current: arrangement(),
            paired: None,
            sweep: None,
            assumptions: loaded,
        };
        let generated = 1_800_000_000;

        // The command line's path: everything after reading the file.
        let from_cli = report_input(&data, generated).unwrap();
        // The desktop's path: the same shared assembly over the panels' records.
        let presets = operating_presets(&data.assumptions).unwrap();
        let from_app = assemble_report_input(
            LoadedArrangement {
                manifest: &data.current.manifest,
                records: &data.current.records,
            },
            None,
            |_, _| None,
            ReportContext {
                study_name: study_name_from_path(Some(&file)),
                sweep: None,
                preset_label: presets[0].name.clone(),
                preset_magnet_limit: service_limit(&presets[0].assumptions, "magnets"),
                fusion_power_mw: data.current.manifest.fusion_power_mw,
                study_file: StudyFileStamp::from_path(&file),
                view_image_note: Some(NO_VIEW_ON_COMMAND_LINE.into()),
                generated_unix_s: generated + 60,
            },
        );
        assert!(from_cli.view_image.is_none());
        assert_eq!(
            from_cli.view_image_note.as_deref(),
            Some(NO_VIEW_ON_COMMAND_LINE)
        );
        assert!(from_cli.study_file.is_some());

        let (a, b) = (dir.path().join("cli"), dir.path().join("app"));
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let (a_out, b_out) = (
            faris_report::export_study(&from_cli, &a).unwrap(),
            faris_report::export_study(&from_app, &b).unwrap(),
        );
        for csv in [
            "histories",
            "comparison",
            "differences",
            "sweep",
            "assumptions",
            "caveats",
        ] {
            let name = format!("data/{csv}.csv");
            let read = |folder: &Path| std::fs::read(folder.join(&name));
            assert_eq!(read(&a_out.folder).ok(), read(&b_out.folder).ok(), "{name}");
        }
        let (mut left, mut right) = (
            manifest_without_time(&a_out.folder),
            manifest_without_time(&b_out.folder),
        );
        // The PDF footer carries the generation date, so only it may differ.
        for manifest in [&mut left, &mut right] {
            for file in manifest["files"].as_array_mut().unwrap() {
                if file["path"] == "summary.pdf" {
                    *file = serde_json::json!({"path": "summary.pdf"});
                }
            }
        }
        assert_eq!(left, right);
    }

    #[test]
    fn a_file_that_is_not_a_study_fails_closed_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("broken.faris");
        std::fs::write(&file, b"not a zip").unwrap();
        let out = dir.path().join("out");
        std::fs::create_dir(&out).unwrap();
        assert!(export(&file, &out).is_err());
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0);
    }
}
