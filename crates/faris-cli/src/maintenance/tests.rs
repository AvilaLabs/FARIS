use super::*;
use faris_engine::{
    fixtures,
    history::EventKind,
    maintenance::{CurveRequest, DecaySource, InstallationCurve, NotEvaluated},
};
use faris_model::maintenance::{
    CoolingGrid, GoverningQuantity, MAINTENANCE_ASSUMPTIONS_VERSION, MaintenanceClass,
};

/// Amplitude grows with the installation's age, so cooldowns lengthen with plant age.
struct Analytic;

impl DecaySource for Analytic {
    fn curves(
        &mut self,
        design: &str,
        history: &HistoryResult,
        requests: &[CurveRequest],
        grid_s: &[f64],
        _cancel: &Cancellation,
    ) -> Result<Vec<Result<InstallationCurve, NotEvaluated>>, String> {
        let scale = if design == "alt" { 1.5 } else { 1.0 };
        Ok(requests
            .iter()
            .map(|r| {
                let install_s = history
                    .events
                    .iter()
                    .filter(|e| {
                        e.component_id.as_deref() == Some(r.component.as_str())
                            && e.kind == EventKind::ReplacementCompleted
                            && e.time_s <= r.shutdown_s
                    })
                    .map(|e| e.time_s)
                    .fold(0.0, f64::max);
                let age = r.shutdown_s - install_s;
                let amp = if r.component == "magnets" { 40.0 } else { 25.0 } * scale;
                Ok(InstallationCurve {
                    points: grid_s
                        .iter()
                        .map(|&t| {
                            (
                                t,
                                amp * (1.0 + age / 10.0)
                                    * (0.7 * (-t / 8.0).exp() + 0.3 * (-t / 300.0).exp()),
                            )
                        })
                        .collect(),
                    volume_m3: 2.0,
                    install_s,
                })
            })
            .collect())
    }
}

fn sample_assumptions() -> MaintenanceAssumptions {
    let mut classes = BTreeMap::new();
    classes.insert(
        "magnet".to_string(),
        MaintenanceClass {
            component_id: "magnets".into(),
            governing: vec!["blanket".into(), "magnets".into()],
            work_s: 2.0,
            threshold: Threshold::Calibrate {
                design: "ref".into(),
                target_cooldown_s: 10.0,
            },
        },
    );
    MaintenanceAssumptions {
        schema_version: MAINTENANCE_ASSUMPTIONS_VERSION.into(),
        governing_quantity: GoverningQuantity::Heat,
        classes,
        cooling: CoolingGrid {
            min_s: 1.0,
            max_s: 1000.0,
            points: 40,
        },
        max_iterations: 10,
        convergence_s: 1e-6,
    }
}

/// A result from the engine's coupling loop with the analytic source.
fn sample_result() -> MaintenanceResult {
    let mut alt = fixtures::assumptions();
    for l in &mut alt.service_limits {
        if l.component_id == "magnets" {
            l.limit = 2.0e11;
        }
    }
    let designs = vec![
        DesignInput {
            name: "ref".into(),
            history_assumptions: fixtures::assumptions(),
        },
        DesignInput {
            name: "alt".into(),
            history_assumptions: alt,
        },
    ];
    let mut runner = |_: &str, a: &OperatingHistoryAssumptions, c: &Cancellation| {
        run_operating_history_cancellable(a, &fixtures::rates_without_covariance(0.0, 'a'), c)
    };
    let mut result = run_maintenance(
        &sample_assumptions(),
        &designs,
        &mut runner,
        &mut Analytic,
        &Cancellation::default(),
        &mut |_, _, _| {},
    )
    .unwrap();
    result.decay_source = Some(DecaySourceRecord {
        kind: "actinv-continuations".into(),
        actinv_runs: 7,
        cache_hits: 3,
    });
    result
}

#[test]
fn markdown_shows_both_models_per_design_event_and_contrast() {
    let result = sample_result();
    let text = markdown(&result);
    for design in ["ref", "alt"] {
        assert!(
            text.contains(&format!("| {design} | EVALUATED |")),
            "{text}"
        );
        assert!(
            text.contains(&format!("## Replacements: {design}")),
            "{text}"
        );
    }
    assert!(text.contains("Downtime fixed (d)") && text.contains("Downtime computed (d)"));
    assert!(text.contains("Availability computed") && text.contains("Net electricity computed"));
    assert!(text.contains("7 ACTINV runs, 3 cached curves"));
    // Thresholds, one row per class.
    assert!(text.contains("| magnet |") && text.contains("calibrated on ref"));
    // Every replacement is listed with its governing component, share and age.
    let reference = &result.designs["ref"];
    let events = final_events(&reference.computed);
    assert!(!events.is_empty());
    let rows = text
        .split("## Replacements: ref")
        .nth(1)
        .unwrap()
        .split("##")
        .next()
        .unwrap()
        .lines()
        .filter(|l| l.starts_with("| magnets |"))
        .count();
    assert_eq!(rows, events.len());
    let first = &events[0];
    let why = first.why.as_ref().unwrap();
    let top = why
        .governing
        .iter()
        .reduce(|a, g| if g.share > a.share { g } else { a })
        .unwrap();
    assert!(text.contains(&format!(
        "| {} | {:.0} % |",
        top.component,
        top.share * 100.0
    )));
    // The computed duration column is cooldown plus work, in days.
    assert!(text.contains(&days(first.duration_computed_s.unwrap())));
    // Between designs: a difference under each model and the ratio.
    assert!(text.contains("## Between designs"));
    let c = &result.contrasts[0];
    assert!(text.contains(&contrast_row(c)));
    assert!(c.computed_difference_s.is_some());
}

#[test]
fn markdown_names_the_reason_and_next_step_of_every_gap() {
    let mut result = sample_result();
    let design = result.designs.get_mut("alt").unwrap();
    design.computed.status = Status::NotEvaluated;
    design.computed.summary = None;
    design.computed.not_evaluated = Some(faris_engine::maintenance::NotEvaluated {
        reason: "decay curve never falls below q* within 365 days".into(),
        next_step: "raise q* | or lengthen the grid".into(),
    });
    result.contrasts[0].computed_difference_s = None;
    result.contrasts[0].ratio_computed_over_fixed = None;
    result.contrasts[0].not_evaluated = Some(faris_engine::maintenance::NotEvaluated {
        reason: "a design has no computed downtime".into(),
        next_step: "see that design's computed NOT_EVALUATED reason".into(),
    });
    let text = markdown(&result);
    assert!(text.contains("| alt | NOT_EVALUATED |"), "{text}");
    assert!(text.contains(
        "- alt: NOT_EVALUATED. decay curve never falls below q* within 365 days Next step: raise q* | or lengthen the grid"
    ));
    assert!(!text.contains("## Replacements: alt"));
    assert!(text.contains("## Replacements: ref"));
    assert!(text.contains("a design has no computed downtime Next step: see that design's"));
    assert!(text.contains("| n/a | n/a |"));
}

#[test]
fn result_survives_a_json_round_trip_through_the_report_reader() {
    let result = sample_result();
    let text = serde_json::to_string_pretty(&result).unwrap();
    let back: MaintenanceResult = serde_json::from_str(&text).unwrap();
    assert_eq!(back, result);
    assert_eq!(markdown(&back), markdown(&result));
    // Without a decay-source record the field is absent from the file.
    let mut plain = result;
    plain.decay_source = None;
    assert!(
        !serde_json::to_string(&plain)
            .unwrap()
            .contains("decay_source")
    );
}

// ----------------------------------------------------------- designs file --

fn designs_json(edit: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let mut v = serde_json::json!({
        "schema_version": DESIGNS_VERSION,
        "designs": {
            "ref": {"scenario": "ref/s.json", "physics": "ref/p.json", "history_run": "ref/run.json",
                    "spectrum_run": "ref/spec.json", "history_assumptions": "h.json"}
        }
    });
    edit(&mut v);
    serde_json::to_vec(&v).unwrap()
}

fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("ref")).unwrap();
    for f in [
        "ref/s.json",
        "ref/p.json",
        "ref/run.json",
        "ref/spec.json",
        "h.json",
    ] {
        std::fs::write(dir.path().join(f), "{}").unwrap();
    }
    dir
}

#[test]
fn designs_file_paths_are_relative_to_the_file() {
    let dir = tree();
    let got = parse_designs(&designs_json(|_| {}), dir.path()).unwrap();
    let d = &got["ref"];
    assert_eq!(d.scenario, dir.path().join("ref/s.json"));
    assert_eq!(d.history_assumptions, dir.path().join("h.json"));
    assert_eq!(d.files().len(), 5);
    // An absolute path is kept.
    let absolute = dir.path().join("h.json");
    let bytes = designs_json(|v| {
        v["designs"]["ref"]["history_assumptions"] = absolute.to_str().unwrap().into();
    });
    let got = parse_designs(&bytes, Path::new("/nonexistent")).unwrap_err();
    assert!(got.to_string().contains("is not a file"));
}

#[test]
fn bad_designs_files_are_refused_with_a_reason() {
    let dir = tree();
    type Edit = Box<dyn Fn(&mut serde_json::Value)>;
    let cases: Vec<(&str, Edit, &str)> = vec![
        (
            "schema",
            Box::new(|v| v["schema_version"] = "x".into()),
            "schema_version must be faris-maintenance-designs/v0.1",
        ),
        (
            "empty",
            Box::new(|v| v["designs"] = serde_json::json!({})),
            "names no design",
        ),
        (
            "blank name",
            Box::new(|v| {
                let d = v["designs"]["ref"].clone();
                v["designs"] = serde_json::json!({" ": d});
            }),
            "nonempty",
        ),
        (
            "unknown field",
            Box::new(|v| v["designs"]["ref"]["extra"] = 1.into()),
            "unknown field",
        ),
        (
            "missing field",
            Box::new(|v| {
                v["designs"]["ref"]
                    .as_object_mut()
                    .unwrap()
                    .remove("physics");
            }),
            "missing field",
        ),
        (
            "missing file",
            Box::new(|v| v["designs"]["ref"]["spectrum_run"] = "nope.json".into()),
            "design ref: spectrum_run",
        ),
    ];
    for (label, edit, expected) in cases {
        let err = parse_designs(&designs_json(edit), dir.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains(expected), "{label}: {err}");
    }
    assert!(parse_designs(b"not json", dir.path()).is_err());
}
