use super::*;
use faris_engine::{
    history::{
        ELEMENTARY_CHARGE_J_PER_EV, ScalarRate, TransportDrivingRates, run_operating_history,
    },
    sweep::{Estimate, summarize_history},
};
use faris_model::history::{
    ComponentClass, EnergyAssumptions, OperatingHistoryAssumptions, PowerPeriod, ServiceLimit,
    TimeInterval,
};
use std::{collections::BTreeMap, process::Command};

const YEAR_S: f64 = JULIAN_YEAR_SECONDS;
const FIXTURE_UNIX_S: i64 = 1_790_000_000;

fn assumptions(horizon_years: f64, limit: f64) -> OperatingHistoryAssumptions {
    let horizon = horizon_years * YEAR_S;
    OperatingHistoryAssumptions {
        schema_version: faris_model::history::OPERATING_HISTORY_VERSION.into(),
        horizon_s: horizon,
        maximum_step_s: 3_600.0,
        snapshot_interval_s: 86_400.0,
        initial_available_tritium_kg: 5.0,
        initial_in_process_tritium_kg: 0.0,
        initial_in_process_release_s: 0.0,
        startup_reserve_kg: 0.5,
        restart_inventory_kg: 0.6,
        recovery_fraction: 0.95,
        processing_delay_s: 86_400.0,
        operation: vec![PowerPeriod {
            start_s: 0.0,
            end_s: horizon,
            power_fraction: 1.0,
        }],
        planned_outages: (0..horizon_years as usize)
            .map(|y| TimeInterval {
                start_s: y as f64 * YEAR_S,
                end_s: y as f64 * YEAR_S + 30.0 * 86_400.0,
                reason: "authored illustrative annual maintenance outage".into(),
            })
            .collect(),
        imports: vec![],
        service_limits: vec![
            ServiceLimit {
                component_id: "magnets".into(),
                class: ComponentClass::Replaceable,
                response_id: "magnets-flux".into(),
                metric: "energy_integrated_component_average_neutron_flux".into(),
                unit: "neutrons/m²".into(),
                limit,
                replacement_duration_s: Some(120.0 * 86_400.0),
                provenance: "Literature-anchored REBCO tape fast-neutron fluence screening value of 3e18 n/cm2 cited by Sorbom et al. 2015 (ARC conceptual design). Demountable-coil replacement and its 120-day duration are authored assumptions.".into(),
            },
        ],
        energy: EnergyAssumptions {
            alpha_deposition_fraction: Some(1.0),
            thermal_to_electric_efficiency: Some(0.4),
            transport_heat_recovery_fraction: Some(0.9),
            auxiliary_power_mw_while_operating: Some(60.0),
            auxiliary_power_mw_while_off: Some(5.0),
            provenance: "All values are authored scenario controls for a transparent arithmetic demonstration, not plant design estimates.".into(),
        },
        provenance: "Authored illustrative control scenario. No value is a measured, optimized, or qualified ARC parameter.".into(),
    }
}

fn rates(tbr: f64, flux: f64) -> TransportDrivingRates {
    let q_ev = 17.6e6;
    let reactions = 525.0e6 / (q_ev * ELEMENTARY_CHARGE_J_PER_EV);
    let mut component = BTreeMap::new();
    component.insert(
        "magnets".to_string(),
        ScalarRate {
            mean: flux,
            standard_error: Some(flux * 0.15),
            unit: "neutrons/m²/s".into(),
            response_id: "magnets-flux".into(),
        },
    );
    TransportDrivingRates {
        reference_fusion_power_mw: 525.0,
        fusion_reaction_rate_per_s: reactions,
        neutron_source_rate_per_s: reactions,
        total_reaction_energy_ev: q_ev,
        primary_neutron_energy_ev: 14.1e6,
        breeder_h3_per_source_neutron: ScalarRate {
            mean: tbr,
            standard_error: Some(0.0014),
            unit: "particles/source_neutron".into(),
            response_id: "blanket-tritium".into(),
        },
        component_average_flux_n_m2_s: component,
        region_flux_n_m2_s: BTreeMap::new(),
        transport_deposited_heat_w: Some(ScalarRate {
            mean: 4.9e8,
            standard_error: Some(2.0e5),
            unit: "W".into(),
            response_id: "heating-total-whole-model".into(),
        }),
        scenario_sha256: "a".repeat(64),
        transport_artifact_sha256: "b".repeat(64),
        solver_digest: format!("sha256:{}", "c".repeat(64)),
        nuclear_data_digest: format!("sha256:{}", "d".repeat(64)),
        covariance: None,
    }
}

fn arrangement(arrangement: Arrangement, tbr: f64, flux: f64) -> ArrangementInput {
    let history = run_operating_history(&assumptions(12.0, 1.5e22), &rates(tbr, flux)).unwrap();
    ArrangementInput {
        arrangement,
        transport: Some(TransportSummary {
            breeder_h3_per_source: Some((tbr * 0.9, 0.0012)),
            total_h3_per_source: Some((tbr, 0.0014)),
            magnet_flux: Some((flux, flux * 0.15)),
            nuclear_heat_w: Some((4.9e8, 2.0e5)),
        }),
        sampling: Some(Sampling {
            seed: 17,
            histories: 1_000_000,
        }),
        history: Some(history),
        ensemble: EnsembleInput::None,
    }
}

fn sweep() -> SweepInput {
    let points: Vec<TransportPoint> = (0..7)
        .map(|i| {
            let b = 0.35 + 0.05 * f64::from(i);
            TransportPoint {
                variant_id: format!("blanket-{b:.2}"),
                label: format!("{b:.2} m blanket"),
                blanket_m: b,
                shield_m: 0.9 - b,
                breeding: Estimate {
                    mean: 1.20 + 0.04 * f64::from(i),
                    standard_error: 0.0014,
                },
                magnet_flux: Estimate {
                    mean: 2.0e14 * (1.0 - 0.05 * f64::from(i)),
                    standard_error: 3.0e13,
                },
                seed: 100 + u64::from(i as u32),
                histories: 1_000_000,
            }
        })
        .collect();
    let summaries = points
        .iter()
        .map(|p| {
            let h = run_operating_history(
                &assumptions(12.0, 1.5e22),
                &rates(p.breeding.mean, p.magnet_flux.mean),
            )
            .unwrap();
            Some(summarize_history(&h, "magnets"))
        })
        .collect();
    SweepInput { points, summaries }
}

fn input() -> ReportInput {
    ReportInput {
        study_name: "Demountable magnets".into(),
        arrangements: vec![
            arrangement(Arrangement::ORDER[0], 1.18, 3.0e14),
            arrangement(Arrangement::ORDER[1], 1.25, 1.6e14),
            arrangement(Arrangement::ORDER[2], 1.21, 3.4e14),
            arrangement(Arrangement::ORDER[3], 1.28, 1.7e14),
        ],
        sweep: Some(sweep()),
        preset_label: "Demountable magnets · REBCO fluence limit".into(),
        preset_magnet_limit: Some(1.5e22),
        fusion_power_mw: 525.0,
        port_volume_unvalidated: true,
        study_file: None,
        view_image: None,
        view_image_note: Some("capture unavailable in this test".into()),
        generated_unix_s: FIXTURE_UNIX_S,
    }
}

fn read(folder: &Path, relative: &str) -> Vec<u8> {
    fs::read(folder.join(relative)).unwrap_or_else(|e| panic!("{relative}: {e}"))
}

fn text(folder: &Path, relative: &str) -> String {
    String::from_utf8(read(folder, relative)).unwrap()
}

/// A CSV file without its leading `# ` comment lines.
fn table(folder: &Path, relative: &str) -> String {
    let all = text(folder, relative);
    let mut lines = all
        .lines()
        .skip_while(|l| l.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    lines.push('\n');
    lines
}

fn tool_available(name: &str) -> bool {
    Command::new(name).arg("-v").output().is_ok()
}

/// One export shared by the tests that only read it; each gets its own copy.
fn exported() -> (tempfile::TempDir, ExportOutcome) {
    static ONCE: std::sync::OnceLock<ExportOutcome> = std::sync::OnceLock::new();
    let outcome = ONCE.get_or_init(|| {
        let dir = std::env::temp_dir().join("faris-report-shared-export");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        export_study(&input(), &dir).unwrap()
    });
    let own = tempfile::tempdir().unwrap();
    let target = own.path().join(outcome.folder.file_name().unwrap());
    copy_dir(&outcome.folder, &target);
    (
        own,
        ExportOutcome {
            folder: target,
            files: outcome.files.clone(),
            view_image_included: outcome.view_image_included,
        },
    )
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn export_writes_the_documented_folder_layout() {
    let (_guard, outcome) = exported();
    assert_eq!(
        outcome.folder.file_name().unwrap().to_str().unwrap(),
        "Demountable magnets-export"
    );
    for relative in [
        "summary.pdf",
        "export-manifest.json",
        "data/histories.csv",
        "data/comparison.csv",
        "data/sweep.csv",
        "data/assumptions.csv",
        "charts/magnet-fluence-timeline.svg",
        "charts/magnet-fluence-timeline.png",
        "charts/sweep-breeding.svg",
        "charts/sweep-breeding.png",
        "charts/sweep-magnet-flux.svg",
        "charts/sweep-magnet-flux.png",
        "charts/sweep-swaps-electricity.svg",
        "charts/sweep-swaps-electricity.png",
    ] {
        assert!(
            outcome.folder.join(relative).is_file(),
            "{relative} missing"
        );
    }
    // No staging directory is left behind.
    let parent = outcome.folder.parent().unwrap();
    let leftovers: Vec<_> = fs::read_dir(parent)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(".faris-export-")
        })
        .collect();
    assert!(leftovers.is_empty());
}

#[test]
fn history_csv_has_long_format_headers_and_every_snapshot() {
    let (_guard, outcome) = exported();
    let csv = table(&outcome.folder, "data/histories.csv");
    let mut lines = csv.lines();
    assert_eq!(
        lines.next().unwrap(),
        "arrangement,calendar_year,state,magnet_fluence_n_m2,magnet_limit_fluence_n_m2,blanket_fluence_n_m2,usable_tritium_kg,net_electricity_twh,magnet_swaps"
    );
    let source = input();
    let expected: usize = source
        .arrangements
        .iter()
        .map(|a| a.history.as_ref().unwrap().snapshots.len())
        .sum();
    let body: Vec<&str> = lines.collect();
    assert_eq!(body.len(), expected);
    // Full precision: the last port-reference row round-trips the engine value.
    let history = source.arrangements[0].history.as_ref().unwrap();
    let last = history.snapshots.last().unwrap();
    let port_rows: Vec<&&str> = body
        .iter()
        .filter(|l| l.starts_with("port-reference,"))
        .collect();
    assert_eq!(port_rows.len(), history.snapshots.len());
    let cells: Vec<&str> = port_rows.last().unwrap().split(',').collect();
    assert_eq!(cells[1].parse::<f64>().unwrap(), last.time_s / YEAR_S);
    assert_eq!(
        cells[3].parse::<f64>().unwrap(),
        last.component_fluence_n_m2["magnets"]
    );
    // The magnet's exposure toward its limit; with an energy-integrated limit
    // it is the same fluence.
    assert_eq!(
        cells[4].parse::<f64>().unwrap(),
        last.component_fluence_n_m2["magnets"]
    );
    assert_eq!(cells[6].parse::<f64>().unwrap(), last.available_tritium_kg);
    assert_eq!(
        cells[7].parse::<f64>().unwrap(),
        last.cumulative_net_electricity_mwh.unwrap() / 1.0e6
    );
    assert!(
        body.iter()
            .any(|l| l.split(',').nth(2) == Some("magnet_replacement"))
    );
    assert!(
        body.iter()
            .any(|l| l.split(',').nth(2) == Some("operating"))
    );
}

#[test]
fn comparison_sweep_and_assumption_tables_carry_units_and_kinds() {
    let (_guard, outcome) = exported();
    let comparison = table(&outcome.folder, "data/comparison.csv");
    let header = comparison.lines().next().unwrap();
    for column in [
        "magnet_flux_n_m2_s",
        "first_swap_year",
        "net_electricity_twh",
        "final_usable_tritium_kg",
        "breeding_ratio_kind",
        "magnet_swaps_kind",
        "first_swap_kind",
        "net_electricity_kind",
    ] {
        assert!(header.split(',').any(|c| c == column), "{column}");
    }
    assert_eq!(comparison.lines().count(), 5);
    let port_ref = comparison
        .lines()
        .find(|l| l.starts_with("port-reference,"))
        .unwrap();
    let cells: Vec<&str> = port_ref.split(',').collect();
    assert_eq!(cells[6].parse::<f64>().unwrap(), 1.18);
    assert!(cells.contains(&"conditional"));
    assert!(cells.contains(&"calculated"));

    let sweep = table(&outcome.folder, "data/sweep.csv");
    assert_eq!(sweep.lines().count(), 8);
    assert!(
        sweep
            .lines()
            .next()
            .unwrap()
            .contains("net_electricity_twh")
    );

    let assumptions = table(&outcome.folder, "data/assumptions.csv");
    assert_eq!(
        assumptions.lines().next().unwrap(),
        "assumption,value,value_number,unit,kind,provenance"
    );
    let limit = assumptions
        .lines()
        .find(|l| l.starts_with("magnets service limit"))
        .unwrap();
    assert!(limit.contains(",literature,"), "{limit}");
    assert!(assumptions.contains(",authored,"));

    let differences = table(&outcome.folder, "data/differences.csv");
    assert_eq!(differences.lines().count(), 5);
}

#[test]
fn manifest_hashes_match_the_files() {
    let (_guard, outcome) = exported();
    let manifest: serde_json::Value =
        serde_json::from_slice(&read(&outcome.folder, "export-manifest.json")).unwrap();
    assert_eq!(manifest["schema"], "faris-export/1");
    assert_eq!(manifest["faris_version"], FARIS_VERSION);
    assert_eq!(manifest["generated_utc"], "2026-09-21T14:13:20Z");
    assert!(manifest["study_file"].is_null());
    assert_eq!(manifest["view_image"]["included"], false);
    assert_eq!(
        manifest["view_image"]["note"],
        "capture unavailable in this test"
    );
    let files = manifest["files"].as_array().unwrap();
    assert!(files.len() >= 14);
    for entry in files {
        let path = entry["path"].as_str().unwrap();
        let bytes = read(&outcome.folder, path);
        assert_eq!(
            entry["bytes"].as_u64().unwrap(),
            bytes.len() as u64,
            "{path}"
        );
        assert_eq!(
            entry["sha256"].as_str().unwrap(),
            sha256_hex(&bytes),
            "{path}"
        );
    }
    assert!(files.iter().all(|f| f["path"] != "export-manifest.json"));
}

#[test]
fn study_file_stamp_is_named_in_the_manifest_and_the_pdf_footer() {
    let dir = tempfile::tempdir().unwrap();
    let mut source = input();
    source.study_file = Some(StudyFileStamp {
        file_name: "demountable.faris".into(),
        sha256: "0123456789abcdef".repeat(4),
    });
    let outcome = export_study(&source, dir.path()).unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&read(&outcome.folder, "export-manifest.json")).unwrap();
    assert_eq!(manifest["study_file"]["file_name"], "demountable.faris");
    assert_eq!(
        manifest["study_file"]["sha256"],
        "0123456789abcdef".repeat(4)
    );
    if tool_available("pdftotext") {
        let out = Command::new("pdftotext")
            .arg(outcome.folder.join("summary.pdf"))
            .arg("-")
            .output()
            .unwrap();
        let flat = String::from_utf8_lossy(&out.stdout).replace('\n', " ");
        assert!(flat.contains("Study file: demountable.faris sha256:0123456789abcdef"));
    }
}

#[test]
fn unsaved_study_says_so_in_the_pdf() {
    let (_guard, outcome) = exported();
    if !tool_available("pdftotext") {
        return;
    }
    let out = Command::new("pdftotext")
        .arg(outcome.folder.join("summary.pdf"))
        .arg("-")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        text.contains("Unsaved study — no study file hash"),
        "{text}"
    );
    assert!(text.contains("Caveats and unknowns"));
    assert!(text.contains("REBCO limit"));
    assert!(text.contains("Allocation sweep"));
    // Every caveat row names why and what would settle it.
    assert!(text.contains("Magnet swaps and first swap"));
}

#[test]
fn pdf_is_two_us_letter_pages() {
    let (_guard, outcome) = exported();
    let pdf = outcome.folder.join("summary.pdf");
    assert!(read(&outcome.folder, "summary.pdf").starts_with(b"%PDF-"));
    if !tool_available("pdfinfo") {
        eprintln!("pdfinfo not installed; page count not checked");
        return;
    }
    let out = Command::new("pdfinfo").arg(&pdf).output().unwrap();
    let info = String::from_utf8_lossy(&out.stdout).to_string();
    let pages = info
        .lines()
        .find_map(|l| l.strip_prefix("Pages:"))
        .map(str::trim);
    assert_eq!(pages, Some("2"), "{info}");
    assert!(info.contains("612 x 792 pts (letter)"), "{info}");
}

#[test]
fn both_pages_fit_their_content() {
    let source = input();
    let prepared = prepare(&source).unwrap();
    let order: Vec<&ArrangementData> = prepared.data.iter().collect();
    let draw = timeline_fn(&prepared);
    let content = pdf_content(&source, &prepared, &order, &draw);
    let extent = pdf::measure(&content).unwrap();
    assert!(extent.page_one_bottom <= 756.0, "{extent:?}");
    assert!(extent.page_two_bottom <= 756.0, "{extent:?}");
    assert!(extent.page_two_scale >= 0.86, "{extent:?}");
}

#[test]
fn refuses_to_write_into_an_existing_folder() {
    let dir = tempfile::tempdir().unwrap();
    let existing = dir.path().join("Demountable magnets-export");
    fs::create_dir(&existing).unwrap();
    fs::write(existing.join("keep.txt"), "mine").unwrap();
    let error = export_study(&input(), dir.path()).unwrap_err();
    assert!(matches!(error, ExportError::FolderExists(_)), "{error}");
    assert_eq!(
        fs::read_to_string(existing.join("keep.txt")).unwrap(),
        "mine"
    );
    assert_eq!(fs::read_dir(&existing).unwrap().count(), 1);
    // Nothing else appeared next to it.
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn a_failed_export_leaves_no_folder_behind() {
    let dir = tempfile::tempdir().unwrap();
    let mut source = input();
    source.study_name = "   ".into();
    assert!(matches!(
        export_study(&source, dir.path()),
        Err(ExportError::Invalid(_))
    ));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn charts_are_written_as_svg_and_png_from_the_same_drawing() {
    let (_guard, outcome) = exported();
    let svg = text(&outcome.folder, "charts/magnet-fluence-timeline.svg");
    assert!(svg.contains("(literature)"));
    assert!(svg.contains("REBCO limit"));
    for arrangement in Arrangement::ORDER {
        assert!(svg.contains(arrangement.label()));
    }
    let png = read(&outcome.folder, "charts/magnet-fluence-timeline.png");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    // 540 x 250 points at 3x, plus the research-screening footer strip.
    let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
    assert_eq!(width, 1620);
    assert!((751..800).contains(&height), "{height}");
    // The SVG parses with the same font database the PDF uses.
    let options = usvg::Options {
        fontdb: fonts::font_database(),
        ..Default::default()
    };
    assert!(usvg::Tree::from_str(&svg, &options).is_ok());
}

#[test]
fn pdf_embeds_fonts_and_keeps_text_as_text() {
    let (_guard, outcome) = exported();
    if !tool_available("pdffonts") {
        return;
    }
    let out = Command::new("pdffonts")
        .arg(outcome.folder.join("summary.pdf"))
        .output()
        .unwrap();
    let listing = String::from_utf8_lossy(&out.stdout).to_string();
    let fonts: Vec<&str> = listing.lines().skip(2).collect();
    assert!(!fonts.is_empty(), "{listing}");
    for line in fonts {
        let embedded = line.split_whitespace().rev().nth(4);
        assert_eq!(embedded, Some("yes"), "{line}");
    }
}

#[test]
fn a_supplied_view_image_is_included_and_a_bad_one_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let mut source = input();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(320, 200).unwrap();
    pixmap.fill(resvg::tiny_skia::Color::from_rgba8(60, 90, 140, 255));
    source.view_image = Some(pixmap.encode_png().unwrap());
    let outcome = export_study(&source, dir.path()).unwrap();
    assert!(outcome.view_image_included);
    assert!(outcome.folder.join("charts/3d-view.png").is_file());
    let manifest: serde_json::Value =
        serde_json::from_slice(&read(&outcome.folder, "export-manifest.json")).unwrap();
    assert_eq!(manifest["view_image"]["included"], true);
    assert!(manifest["view_image"]["note"].is_null());

    let dir = tempfile::tempdir().unwrap();
    let mut bad = input();
    bad.view_image = Some(b"not a png".to_vec());
    let outcome = export_study(&bad, dir.path()).unwrap();
    assert!(!outcome.view_image_included);
    let manifest: serde_json::Value =
        serde_json::from_slice(&read(&outcome.folder, "export-manifest.json")).unwrap();
    assert!(
        manifest["view_image"]["note"]
            .as_str()
            .unwrap()
            .contains("not a readable PNG")
    );
}

#[test]
fn a_partial_study_exports_with_explained_gaps() {
    let dir = tempfile::tempdir().unwrap();
    let mut source = input();
    source.arrangements.truncate(2);
    source.sweep = None;
    let outcome = export_study(&source, dir.path()).unwrap();
    let caveats = table(&outcome.folder, "data/caveats.csv");
    assert!(caveats.contains("Arrangements without a transport record"));
    assert!(caveats.contains("Allocation sweep"));
    for line in caveats.lines().skip(1) {
        assert!(line.matches(',').count() >= 3, "{line}");
    }
    assert!(outcome.folder.join("summary.pdf").is_file());
}

#[test]
fn folder_names_and_dates_are_safe_and_exact() {
    assert_eq!(folder_name("My study"), "My study-export");
    assert_eq!(folder_name("a/b\\c:d"), "a-b-c-d-export");
    assert_eq!(folder_name("  ..  "), "study-export");
    assert_eq!(format_timestamp(0), "1970-01-01T00:00:00Z");
    assert_eq!(format_timestamp(FIXTURE_UNIX_S), "2026-09-21T14:13:20Z");
    assert_eq!(format_date(951_782_400), "2000-02-29");
}

/// Development aid: set `FARIS_REPORT_SAMPLE_DIR` to keep a sample export for
/// visual review (the PDF pages rendered with pdftoppm).
#[test]
fn write_sample_export_when_asked() {
    let Some(dir) = std::env::var_os("FARIS_REPORT_SAMPLE_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let _ = fs::remove_dir_all(dir.join("Demountable magnets-export"));
    fs::create_dir_all(&dir).unwrap();
    let mut source = input();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(900, 620).unwrap();
    pixmap.fill(resvg::tiny_skia::Color::from_rgba8(40, 44, 54, 255));
    source.view_image = Some(pixmap.encode_png().unwrap());
    export_study(&source, &dir).unwrap();
}

// ---------------------------------------------------------------------------
// History ensembles in the export

mod ensembles {
    use super::*;
    use faris_engine::{
        fixtures::synthetic_covariance,
        history_ensemble::{EnsembleSettings, run_history_ensemble},
        jobs::Cancellation,
    };
    use std::sync::Arc;

    const SAMPLES: u32 = 6;

    fn ensemble(tbr: f64, flux: f64, artifact: char, seed: u64, covariance: bool) -> EnsembleInput {
        let mut r = rates(tbr, flux);
        r.transport_artifact_sha256 = artifact.to_string().repeat(64);
        if covariance {
            r.covariance = Some(synthetic_covariance(&r, 0.4));
        }
        EnsembleInput::Ready(Arc::new(
            run_history_ensemble(
                &r,
                &assumptions(12.0, 1.5e22),
                &EnsembleSettings {
                    samples: SAMPLES,
                    seed: Some(seed),
                    threads: Some(2),
                },
                &Cancellation::default(),
                &|_, _| {},
            )
            .unwrap(),
        ))
    }

    /// Two evaluated ensembles, one not evaluated (no covariance) and one
    /// arrangement without any.
    fn with_ensembles() -> ReportInput {
        // The ensembles are the slow part: calculate them once.
        static ONCE: std::sync::OnceLock<ReportInput> = std::sync::OnceLock::new();
        ONCE.get_or_init(|| {
            let mut source = input();
            source.arrangements[0].ensemble =
                ensemble(1.18, 3.0e14, 'b', 18_446_744_073_709_551_000, true);
            source.arrangements[1].ensemble = ensemble(1.25, 1.6e14, 'c', 7, true);
            source.arrangements[2].ensemble = ensemble(1.21, 3.4e14, 'd', 8, false);
            source
        })
        .clone()
    }

    fn exported_with_ensembles() -> (tempfile::TempDir, ExportOutcome) {
        static ONCE: std::sync::OnceLock<ExportOutcome> = std::sync::OnceLock::new();
        let outcome = ONCE.get_or_init(|| {
            let dir = std::env::temp_dir().join("faris-report-shared-export-ensembles");
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            export_study(&with_ensembles(), &dir).unwrap()
        });
        let own = tempfile::tempdir().unwrap();
        let target = own.path().join(outcome.folder.file_name().unwrap());
        copy_dir(&outcome.folder, &target);
        (
            own,
            ExportOutcome {
                folder: target,
                files: outcome.files.clone(),
                view_image_included: outcome.view_image_included,
            },
        )
    }

    #[test]
    fn the_samples_csv_has_one_row_per_sample_per_evaluated_arrangement() {
        let (_dir, outcome) = exported_with_ensembles();
        let csv = table(&outcome.folder, "data/history-ensemble-samples.csv");
        let mut lines = csv.lines();
        assert_eq!(
            lines.next().unwrap(),
            "arrangement,sample,rejected_draws,breeder_h3_per_source_neutron,flux_magnets_n_m2_s,heating_w,outcome,terminal_time_years,full_power_time_years,final_usable_tritium_kg,final_in_process_tritium_kg,gross_electricity_twh,auxiliary_electricity_twh,net_electricity_twh,replacements_magnets,first_replacement_year_magnets,first_trigger_magnets"
        );
        let body: Vec<Vec<&str>> = lines.map(|l| l.split(',').collect()).collect();
        // Two evaluated arrangements; the not-evaluated and the missing ones
        // contribute no samples.
        assert_eq!(body.len(), 2 * SAMPLES as usize);
        assert!(body.iter().all(|r| r.len() == 17));
        let ids: Vec<&str> = body.iter().map(|r| r[0]).collect();
        assert!(ids[..6].iter().all(|i| *i == "port-reference"));
        assert!(ids[6..].iter().all(|i| *i == "no-port-reference"));
        let samples: Vec<&str> = body[..6].iter().map(|r| r[1]).collect();
        assert_eq!(samples, ["1", "2", "3", "4", "5", "6"]);
        // The sampled rates differ between samples, and the outcome is named.
        let fluxes: std::collections::BTreeSet<&str> = body[..6].iter().map(|r| r[4]).collect();
        assert!(fluxes.len() > 1);
        assert!(body.iter().all(|r| matches!(
            r[6],
            "horizon_completed" | "fuel_limited_at_horizon" | "permanent_component_limit"
        )));
        // Full precision: the value is the engine's.
        let first = match &with_ensembles().arrangements[0].ensemble {
            EnsembleInput::Ready(e) => e.samples[0].rates.component_average_flux_n_m2_s["magnets"],
            _ => unreachable!(),
        };
        assert_eq!(body[0][4], format!("{first}"));
    }

    #[test]
    fn the_summary_csv_carries_ranges_distributions_and_the_reasons() {
        let (_dir, outcome) = exported_with_ensembles();
        let csv = table(&outcome.folder, "data/history-ensemble-summary.csv");
        assert_eq!(
            csv.lines().next().unwrap(),
            "arrangement,ensemble_status,output,unit,statistic,category,value,ci95_low,ci95_high,n,note"
        );
        let rows: Vec<Vec<String>> = csv
            .lines()
            .skip(1)
            .map(|l| l.split(',').map(String::from).collect())
            .collect();
        let find = |arrangement: &str, output: &str, statistic: &str| {
            rows.iter()
                .find(|r| r[0] == arrangement && r[2] == output && r[4] == statistic)
        };
        // An evaluated arrangement: the nominal value and the quantiles in years.
        for stat in ["nominal", "mean", "p5", "p50", "p95"] {
            let row = find("port-reference", "full_power_time", stat).unwrap();
            assert_eq!(row[1], "evaluated");
            assert_eq!(row[3], "years");
        }
        let p50 = find("port-reference", "full_power_time", "p50").unwrap();
        assert_eq!(p50[9], SAMPLES.to_string());
        assert!(find("port-reference", "cumulative_net_electricity", "p95").unwrap()[3] == "TWh");
        // Shares of a discrete output carry Wilson intervals.
        let share = rows
            .iter()
            .find(|r| r[0] == "port-reference" && r[2] == "replacements:magnets" && r[4] == "share")
            .unwrap();
        assert!(
            share[6].parse::<f64>().is_ok()
                && share[7].parse::<f64>().is_ok()
                && share[8].parse::<f64>().is_ok()
        );
        let shares: f64 = rows
            .iter()
            .filter(|r| r[0] == "port-reference" && r[2] == "terminal_status" && r[4] == "share")
            .map(|r| r[6].parse::<f64>().unwrap())
            .sum();
        assert!((shares - 1.0).abs() < 1e-12);
        // The not-evaluated arrangement keeps its nominal values and says why.
        let nominal = find("port-breeder-heavy", "full_power_time", "nominal").unwrap();
        assert_eq!(nominal[1], "not_evaluated");
        let raw = csv
            .lines()
            .find(|l| {
                l.starts_with("port-breeder-heavy,not_evaluated,full_power_time,years,nominal")
            })
            .unwrap();
        assert!(raw.contains("\"no uncertainty range: This transport record has standard errors but no covariance"), "{raw}");
        assert!(raw.contains("Next step: Rerun transport"), "{raw}");
        assert!(find("port-breeder-heavy", "full_power_time", "p5").is_none());
        // One with no ensemble at all says so.
        let none = find("no-port-breeder-heavy", "full_power_time", "nominal").unwrap();
        assert_eq!(none[1], "not_calculated");
    }

    #[test]
    fn the_manifest_records_method_seed_samples_rejections_and_status() {
        let (_dir, outcome) = exported_with_ensembles();
        let manifest: serde_json::Value =
            serde_json::from_slice(&read(&outcome.folder, "export-manifest.json")).unwrap();
        let records = manifest["history_ensembles"].as_array().unwrap();
        assert_eq!(records.len(), 4);
        let by = |id: &str| records.iter().find(|r| r["arrangement"] == id).unwrap();
        let a = by("port-reference");
        assert_eq!(a["status"], "evaluated");
        assert_eq!(a["method"], "faris-history-ensemble/v1");
        // The seed is text: it is above 2^53 and must not pass through a float.
        assert_eq!(a["seed"], "18446744073709551000");
        assert_eq!(a["samples_requested"], SAMPLES);
        assert_eq!(a["samples_accepted"], SAMPLES);
        assert_eq!(a["rejections"], 0);
        assert!(
            a["scope"]
                .as_str()
                .unwrap()
                .contains("sampling uncertainty only")
        );
        let n = by("port-breeder-heavy");
        assert_eq!(n["status"], "not_evaluated");
        assert!(n["why"].as_str().unwrap().contains("no covariance"));
        assert!(
            n["next_step"]
                .as_str()
                .unwrap()
                .starts_with("Rerun transport")
        );
        assert_eq!(by("no-port-breeder-heavy")["status"], "not_calculated");
        // Without ensembles the key is an empty list, not absent.
        let (_guard, plain) = exported();
        let manifest: serde_json::Value =
            serde_json::from_slice(&read(&plain.folder, "export-manifest.json")).unwrap();
        let records = manifest["history_ensembles"].as_array().unwrap();
        assert!(records.iter().all(|r| r["status"] == "not_calculated"));
    }

    #[test]
    fn charts_draw_the_band_and_the_new_chart_files_exist() {
        let (_dir, outcome) = exported_with_ensembles();
        for name in [
            "charts/history-tritium-bands.svg",
            "charts/history-tritium-bands.png",
            "charts/history-net-electricity-bands.svg",
            "charts/history-net-electricity-bands.png",
        ] {
            assert!(outcome.folder.join(name).is_file(), "{name}");
        }
        let band_paths = |svg: &str| svg.matches("fill-opacity=\"0.220\"").count();
        // Two evaluated arrangements: two bands on each chart.
        assert_eq!(
            band_paths(&text(&outcome.folder, "charts/history-tritium-bands.svg")),
            2
        );
        assert_eq!(
            band_paths(&text(&outcome.folder, "charts/magnet-fluence-timeline.svg")),
            2
        );
        let timeline = text(&outcome.folder, "charts/magnet-fluence-timeline.svg");
        assert!(timeline.contains("P5-P95 band"));
        // A plain export draws no band.
        let (_guard, plain) = exported();
        assert_eq!(
            band_paths(&text(&plain.folder, "charts/magnet-fluence-timeline.svg")),
            0
        );
        assert!(
            !plain
                .folder
                .join("charts/history-tritium-bands.svg")
                .exists()
        );
    }

    // Verifies: LEG-040
    #[test]
    fn the_ensemble_csvs_and_the_third_pdf_page_carry_the_research_screening_statement() {
        let (_dir, outcome) = exported_with_ensembles();
        let statement = faris_model::RESEARCH_SCREENING_STATEMENT;
        for name in ["samples", "summary"] {
            let csv = text(
                &outcome.folder,
                &format!("data/history-ensemble-{name}.csv"),
            );
            assert_eq!(
                csv.lines().next().unwrap(),
                format!("# {statement}"),
                "{name}"
            );
        }
        let manifest: serde_json::Value =
            serde_json::from_slice(&read(&outcome.folder, "export-manifest.json")).unwrap();
        assert_eq!(manifest["research_screening"], statement);
        assert!(
            tool_available("pdftotext"),
            "pdftotext is needed to read the PDF"
        );
        let out = Command::new("pdftotext")
            .args(["-f", "3", "-l", "3"])
            .arg(outcome.folder.join("summary.pdf"))
            .arg("-")
            .output()
            .unwrap();
        let flat = String::from_utf8_lossy(&out.stdout).replace('\n', " ");
        assert!(flat.contains(statement), "{flat}");
    }

    #[test]
    fn a_plain_export_has_no_ensemble_files() {
        let (_guard, plain) = exported();
        for name in [
            "data/history-ensemble-samples.csv",
            "data/history-ensemble-summary.csv",
        ] {
            assert!(!plain.folder.join(name).exists(), "{name}");
        }
    }

    #[test]
    fn the_pdf_gains_a_third_page_that_fits_and_shows_ranges_beside_nominal_values() {
        let source = with_ensembles();
        let prepared = prepare(&source).unwrap();
        let order: Vec<&ArrangementData> = prepared.data.iter().collect();
        let draw = timeline_fn(&prepared);
        let content = pdf_content(&source, &prepared, &order, &draw);
        let extent = pdf::measure(&content).unwrap();
        assert!(extent.page_one_bottom <= 756.0, "{extent:?}");
        assert!(extent.page_three_bottom.unwrap() <= 756.0, "{extent:?}");

        let (_dir, outcome) = exported_with_ensembles();
        if !tool_available("pdfinfo") || !tool_available("pdftotext") {
            return;
        }
        let pdf = outcome.folder.join("summary.pdf");
        let info = Command::new("pdfinfo").arg(&pdf).output().unwrap();
        let info = String::from_utf8_lossy(&info.stdout).to_string();
        let pages = info
            .lines()
            .find_map(|l| l.strip_prefix("Pages:"))
            .map(str::trim);
        assert_eq!(pages, Some("3"), "{info}");
        let out = Command::new("pdftotext")
            .arg("-layout")
            .arg(&pdf)
            .arg("-")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(text.contains("Page 3 of 3"));
        assert!(text.contains("Uncertainty in the operating history"));
        assert!(
            text.contains("transport Monte Carlo sampling uncertainty only")
                || text.contains("Transport Monte Carlo sampling uncertainty only")
        );
        assert!(text.contains("nominal "));
        assert!(text.contains("P5–P95"));
        assert!(text.contains("Magnet swaps: "));
        // The not-evaluated arrangement: its reason and next step are text.
        let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(flat.contains("no covariance between its results"), "{flat}");
        assert!(flat.contains("Next step: Rerun transport with this FARIS version"));
        assert!(flat.contains("no uncertainty range"));
    }

    /// Development aid: keep the three-page export for visual review.
    #[test]
    fn write_sample_ensemble_export_when_asked() {
        let Some(dir) = std::env::var_os("FARIS_REPORT_SAMPLE_DIR") else {
            return;
        };
        let dir = PathBuf::from(dir).join("with-ensembles");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        export_study(&with_ensembles(), &dir).unwrap();
    }
}

// Verifies: LEG-040
#[test]
fn every_csv_starts_with_the_research_screening_statement() {
    let (_guard, outcome) = exported();
    let comment = format!("# {}", faris_model::RESEARCH_SCREENING_STATEMENT);
    for name in [
        "histories",
        "comparison",
        "differences",
        "sweep",
        "assumptions",
        "caveats",
    ] {
        let csv = text(&outcome.folder, &format!("data/{name}.csv"));
        assert_eq!(csv.lines().next().unwrap(), comment, "{name}.csv");
        // The table follows the comment line, with its header unchanged.
        assert!(!csv.lines().nth(1).unwrap().starts_with('#'), "{name}.csv");
    }
}

// Verifies: LEG-040
#[test]
fn every_chart_svg_and_png_carries_the_research_screening_statement() {
    let (_guard, outcome) = exported();
    let statement = faris_model::RESEARCH_SCREENING_STATEMENT;
    let charts: Vec<_> = outcome
        .files
        .iter()
        .filter(|f| f.path.starts_with("charts/") && f.path.ends_with(".svg"))
        .collect();
    assert_eq!(charts.len(), 4);
    for file in charts {
        let svg = text(&outcome.folder, &file.path);
        // The footer lines are the last text elements, in order.
        let texts: Vec<&str> = svg
            .lines()
            .filter(|l| l.starts_with("<text"))
            .map(|l| l.trim_end_matches("</text>").rsplit('>').next().unwrap())
            .collect();
        let words: Vec<&str> = statement.split_whitespace().collect();
        let tail: Vec<&str> = texts
            .iter()
            .flat_map(|t| t.split_whitespace())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .take(words.len())
            .rev()
            .collect();
        assert_eq!(tail, words, "{}", file.path);
        // The PNG is rendered from this same string, so it has the strip too.
        let png = read(&outcome.folder, &file.path.replace(".svg", ".png"));
        let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
        let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).unwrap();
        let scale = height as f32 / tree.size().height();
        assert!((scale - scale.round()).abs() < 0.05, "{}", file.path);
    }
}

// Verifies: LEG-040
#[test]
fn the_manifest_and_both_pdf_pages_carry_the_research_screening_statement() {
    let (_guard, outcome) = exported();
    let manifest: serde_json::Value =
        serde_json::from_slice(&read(&outcome.folder, "export-manifest.json")).unwrap();
    assert_eq!(
        manifest["research_screening"],
        faris_model::RESEARCH_SCREENING_STATEMENT
    );
    assert!(
        tool_available("pdftotext"),
        "pdftotext is needed to read the PDF"
    );
    for page in ["1", "2"] {
        let out = Command::new("pdftotext")
            .args(["-f", page, "-l", page])
            .arg(outcome.folder.join("summary.pdf"))
            .arg("-")
            .output()
            .unwrap();
        let flat = String::from_utf8_lossy(&out.stdout).replace('\n', " ");
        assert!(
            flat.contains(faris_model::RESEARCH_SCREENING_STATEMENT),
            "page {page}: {flat}"
        );
    }
}
