// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
use super::super::mesh::fixtures::*;
use super::super::testutil::{design_for, manifest_of};
use super::super::*;
use super::*;
use faris_model::math::{cos, sin};
use std::cell::{Cell, RefCell};

const INSPECT: &str =
    include_str!("../../../../faris-model/tests/fixtures/design/pass.inspect.json");

struct Geo {
    imprinted: Facets,
    conversion_candidates: RefCell<Vec<Facets>>,
    conversion_chords: RefCell<Vec<f64>>,
    stochastic_volume_h5m: RefCell<Vec<PathBuf>>,
    tessellation_chords: RefCell<Vec<f64>>,
    deviation_chords: RefCell<Vec<f64>>,
    tags: Vec<String>,
    tess: Facets,
    booleans: RefCell<Vec<BooleanResult>>,
    convert_failure: Option<String>,
    convert_failure_after: Option<usize>,
    convert_unavailable_after: Option<usize>,
    deviation: Option<Vec<DeviationRow>>,
    stochastic_volume_exceeded: bool,
    stochastic_volume_unavailable: bool,
    sites: Vec<[f64; 3]>,
    calls: Cell<[u32; 6]>,
}

impl Geo {
    fn bump(&self, i: usize) {
        let mut c = self.calls.get();
        c[i] += 1;
        self.calls.set(c);
    }
    fn count(&self, i: usize) -> u32 {
        self.calls.get()[i]
    }
}

fn pair_of(f: &Facets, tags: &[String]) -> (Facets, FacetsManifest) {
    let refs: Vec<&str> = tags.iter().map(String::as_str).collect();
    (f.clone(), manifest_of(f, &refs))
}

impl Backends for Geo {
    fn inspect_step(&self, _: &Path, _: Option<LengthUnit>) -> Result<InspectOutcome, DesignError> {
        unreachable!()
    }
    fn audit_materials(&self, _: &[AuditMaterial]) -> Result<AuditOutcome, DesignError> {
        unreachable!()
    }
    fn tessellate(
        &self,
        request: &TessellateRequest,
    ) -> Result<ToolResult<TessellateOutcome>, DesignError> {
        self.bump(0);
        self.tessellation_chords
            .borrow_mut()
            .push(request.chord_cm / 100.0);
        let tags: Vec<String> = (0..self.tess.n_volumes)
            .map(|i| format!("mat:s{i}"))
            .collect();
        let (facets, manifest) = pair_of(&self.tess, &tags);
        Ok(ToolResult::Done(TessellateOutcome {
            facets,
            manifest,
            problems: Vec::new(),
            seconds: 0.5,
            reused: false,
            reader: ReaderStamp::python(),
        }))
    }
    fn overlap_booleans(
        &self,
        r: &BooleanRequest,
    ) -> Result<ToolResult<BooleanOutcome>, DesignError> {
        self.bump(1);
        let results = r
            .pairs
            .iter()
            .map(|&(a, b)| {
                self.booleans
                    .borrow()
                    .iter()
                    .find(|x| x.a == a && x.b == b)
                    .cloned()
                    .unwrap_or(BooleanResult {
                        a,
                        b,
                        status: "decided".into(),
                        common_volume_m3: Some(0.0),
                        reason: None,
                        seconds: None,
                    })
            })
            .collect();
        Ok(ToolResult::Done(BooleanOutcome {
            results,
            seconds: 0.25,
            reused: false,
        }))
    }
    fn convert(&self, r: &ConvertRequest) -> Result<ToolResult<ConvertOutcome>, DesignError> {
        self.bump(2);
        self.conversion_chords.borrow_mut().push(r.chord_cm / 100.0);
        if self
            .convert_unavailable_after
            .is_some_and(|after| self.count(2) > after as u32)
        {
            return Ok(ToolResult::Unavailable {
                why: "the CAD converter became unavailable during refinement".into(),
                next_step: "Configure the CAD converter and check again.".into(),
            });
        }
        if let Some(why) = self.convert_failure.as_ref().filter(|_| {
            self.convert_failure_after
                .is_none_or(|after| self.count(2) > after as u32)
        }) {
            return Ok(ToolResult::Failed {
                why: why.clone(),
                next_step: "Fix it.".into(),
            });
        }
        std::fs::create_dir_all(r.directory).unwrap();
        let h5m_bytes = b"selected nested converter attempt H5M";
        std::fs::write(r.directory.join("model.h5m"), h5m_bytes).unwrap();
        let facets = if self.conversion_candidates.borrow().is_empty() {
            self.imprinted.clone()
        } else {
            self.conversion_candidates.borrow_mut().remove(0)
        };
        let (facets, manifest) = pair_of(&facets, &self.tags);
        let report = ConversionReport {
            schema: "faris-conversion/v1".into(),
            step: geometry::ConversionStep {
                file_name: "m.step".into(),
                sha256: r.step_sha256.into(),
            },
            tolerances: geometry::ConversionTolerances {
                chord_cm: r.chord_cm,
                angular_rad: r.angular_rad,
                scale_factor_to_cm: r.shape_unit_m * 100.0,
                imprint_threads: r.imprint_threads,
                implicit_complement_tag: "void".into(),
            },
            tags: r.tags.to_vec(),
            versions: serde_json::json!({"cad_to_dagmc": "0.14.2"}),
            timing_seconds: BTreeMap::from([("total".to_string(), 3.0)]),
            peak_rss_mb: 100.0,
            h5m: geometry::ConversionFile {
                file: "model.h5m".into(),
                sha256: super::super::sha256_hex(h5m_bytes),
                bytes: h5m_bytes.len() as u64,
            },
            facets: geometry::ConversionFacets {
                bin: "model.facets.bin".into(),
                json: "model.facets.json".into(),
                bin_sha256: manifest.binary_sha256.clone(),
            },
            counts: serde_json::json!({}),
            log_tail: Vec::new(),
        };
        Ok(ToolResult::Done(ConvertOutcome {
            directory: r.directory.to_path_buf(),
            report,
            facets,
            manifest,
            facets_bin: PathBuf::from("model.facets.bin"),
            reused: false,
            helper_sha256: "h".repeat(64),
        }))
    }
    fn deviation(
        &self,
        request: &DeviationRequest,
    ) -> Result<ToolResult<DeviationOutcome>, DesignError> {
        self.bump(3);
        self.deviation_chords.borrow_mut().push(request.tolerance_m);
        Ok(match &self.deviation {
            Some(rows) => ToolResult::Done(DeviationOutcome {
                rows: rows.clone(),
                seconds: 0.1,
                method: "test".into(),
            }),
            None => ToolResult::Unavailable {
                why: "no CAD interpreter".into(),
                next_step: "Pass --cad-python.".into(),
            },
        })
    }
    fn source_sites(&self, _: &SourceRequest) -> Result<ToolResult<SourceOutcome>, DesignError> {
        self.bump(5);
        Ok(ToolResult::Done(SourceOutcome {
            points: self.sites.clone(),
            versions: serde_json::json!({"openmc": "0.15.3"}),
            bin_sha256: "s".repeat(64),
            seconds: 0.2,
        }))
    }
    fn stochastic_volumes(
        &self,
        request: &StochasticVolumeRequest,
    ) -> Result<ToolResult<StochasticVolumeOutcome>, DesignError> {
        self.bump(4);
        self.stochastic_volume_h5m
            .borrow_mut()
            .push(request.h5m.to_path_buf());
        let h5m_bytes = std::fs::read(request.h5m)
            .expect("stochastic-volume adapter receives the selected attempt H5M");
        assert_eq!(super::super::sha256_hex(&h5m_bytes), request.h5m_sha256);
        let rows = request
            .targets
            .iter()
            .map(|target| {
                let cad_volume_cm3 = target.cad_volume_m3 * 1.0e6;
                let openmc_volume_cm3 = cad_volume_cm3
                    * if self.stochastic_volume_exceeded {
                        1.006
                    } else {
                        1.0
                    };
                let standard_error_cm3 = cad_volume_cm3 * 0.001;
                let allowed_difference_cm3 =
                    binding::stochastic_volume_allowance_cm3(cad_volume_cm3, standard_error_cm3);
                StochasticVolumeRow {
                    volume: target.volume,
                    global_id: target.global_id,
                    solid_id: target.solid_id.clone(),
                    material_tag: target.material_tag.clone(),
                    cad_volume_cm3,
                    faceted_volume_cm3: target.faceted_volume_m3 * 1.0e6,
                    openmc_volume_cm3,
                    standard_error_cm3,
                    samples: request.samples as u64,
                    absolute_difference_cm3: (openmc_volume_cm3 - cad_volume_cm3).abs(),
                    allowed_difference_cm3,
                    relative_difference: (openmc_volume_cm3 - cad_volume_cm3) / cad_volume_cm3,
                    verdict: if self.stochastic_volume_exceeded {
                        "EXCEEDED"
                    } else {
                        "WITHIN"
                    }
                    .into(),
                }
            })
            .collect();
        if self.stochastic_volume_unavailable {
            return Ok(ToolResult::Unavailable {
                why: "OpenMC unavailable in fixture".into(),
                next_step: "Configure OpenMC".into(),
            });
        }
        Ok(ToolResult::Done(StochasticVolumeOutcome {
            run_directory: "fixture-run".into(),
            scope: "geometry-only test fixture".into(),
            criterion: "absolute difference <= max(0.5% of CAD volume, 3 standard errors)".into(),
            source_h5m_sha256: request.h5m_sha256.into(),
            cross_sections_sha256: "c".repeat(64),
            request_sha256: "r".repeat(64),
            volume_result_sha256: "v".repeat(64),
            samples: request.samples as u64,
            seed: request.seed,
            unit: "cm3".into(),
            bounds_cm: StochasticVolumeBoundsCm {
                lower_left: [-1.0; 3],
                upper_right: [1.0; 3],
            },
            dummy_material_names: request
                .targets
                .iter()
                .map(|t| t.material_tag.clone())
                .collect(),
            versions: serde_json::json!({"openmc":"fixture"}),
            seconds: 0.1,
            rows,
            helper_sha256: "h".repeat(64),
        }))
    }
}

fn imprinted() -> Facets {
    shell_and_core(10.0, 2.0)
}

fn geo() -> Geo {
    let sites = (0..500)
        .map(|i| {
            let t = f64::from(i) / 500.0;
            [1.5 * sin(t * 7.0), 1.5 * cos(t * 11.0), 1.5 * sin(t * 3.0)]
        })
        .collect();
    Geo {
        imprinted: imprinted(),
        conversion_candidates: RefCell::new(Vec::new()),
        conversion_chords: RefCell::new(Vec::new()),
        stochastic_volume_h5m: RefCell::new(Vec::new()),
        tessellation_chords: RefCell::new(Vec::new()),
        deviation_chords: RefCell::new(Vec::new()),
        tags: vec!["mat:shell".into(), "mat:core".into()],
        tess: separate_boxes(&[([-10.0; 3], [10.0; 3]), ([30.0; 3], [32.0; 3])]),
        booleans: RefCell::new(Vec::new()),
        convert_failure: None,
        convert_failure_after: None,
        convert_unavailable_after: None,
        deviation: Some(vec![
            DeviationRow {
                volume: 1,
                step_index: 0,
                points: 100,
                points_available: 100,
                points_closed_form: 100,
                points_occt: 0,
                points_occt_needed: 0,
                faces: 12,
                max_distance_m: 1.0e-5,
                global_upper_bound_m: Some(2.0e-5),
                global_bound_status: "certified".into(),
                global_bound_queries: 24,
                global_bound_max_queries: 100_000,
                global_bound_max_depth: 18,
                at_m: Some([1.0, 0.0, 0.0]),
            },
            DeviationRow {
                volume: 2,
                step_index: 1,
                points: 100,
                points_available: 100,
                points_closed_form: 100,
                points_occt: 0,
                points_occt_needed: 0,
                faces: 6,
                max_distance_m: 2.0e-5,
                global_upper_bound_m: Some(2.0e-5),
                global_bound_status: "certified".into(),
                global_bound_queries: 12,
                global_bound_max_queries: 100_000,
                global_bound_max_depth: 18,
                at_m: Some([0.0, 0.0, 2.0]),
            },
        ]),
        stochastic_volume_exceeded: false,
        stochastic_volume_unavailable: false,
        sites,
        calls: Cell::new([0; 6]),
    }
}

fn design(waivers: Vec<Waiver>) -> Design {
    design_of(&imprinted(), waivers)
}

fn design_of(f: &Facets, waivers: Vec<Waiver>) -> Design {
    let manifest = manifest_of(f, &["mat:shell", "mat:core"]);
    let mesh = super::super::mesh::Mesh::from_facets(f).unwrap();
    let stats = binding::volume_stats(&mesh, f, &manifest);
    let mut d = design_for(&["shell", "core"], &stats);
    d.plasma.chamber_solid_id = Some("core".into());
    // The template solid is a first wall; only the shell is one here.
    d.solids[1].role = Some(Role::PlasmaChamber);
    d.waivers = waivers;
    d
}

fn waiver(id: &str, solids: &[&str]) -> Waiver {
    Waiver {
        check_id: id.into(),
        solid_ids: solids.iter().map(|s| (*s).to_string()).collect(),
        point_m: None,
        reason: "accepted after review".into(),
        author: "A. Designer".into(),
        date: "2026-10-10".into(),
    }
}

fn options() -> GeometryOptions {
    GeometryOptions {
        threads: 2,
        samples: 20_000,
        chords: 2_000,
        source_sites: 500,
        ..GeometryOptions::default()
    }
}

fn run_with(d: &Design, backend: &Geo) -> ImportReport {
    let inspection: StepInspection = serde_json::from_str(INSPECT).unwrap();
    let mut report = ImportReport {
        schema_version: IMPORT_REPORT_VERSION.into(),
        scope: SCOPE.into(),
        design: DesignIdentity {
            path: "d".into(),
            sha256: "0".repeat(64),
            id: None,
        },
        step: None,
        cad_tool: None,
        material_audit: None,
        checks: Vec::new(),
        void_solids: Vec::new(),
        notices: Vec::new(),
        geometry: GeometryRecords::default(),
        evaluated_checks_pass: true,
        import_complete: false,
        summary: String::new(),
    };
    let opts = options();
    geometry::run(
        &geometry::Inputs {
            design: d,
            inspection: &inspection,
            step: Path::new("m.step"),
            backends: backend,
            options: &opts,
        },
        &mut report,
    )
    .unwrap();
    report
}

fn statuses(r: &ImportReport) -> Vec<(u8, CheckStatus)> {
    r.checks.iter().map(|c| (c.check, c.status)).collect()
}

use CheckStatus::{Fail, NotEvaluated, Pass};

// Verifies: GEO-012, GEO-019, GEO-020, GEO-029, SRC-018
#[test]
fn a_clean_model_passes_checks_6_to_9_and_records_what_was_measured() {
    let backend = geo();
    let report = run_with(&design(Vec::new()), &backend);
    assert_eq!(
        statuses(&report),
        [(6, Pass), (7, Pass), (8, Pass), (9, Pass)]
    );
    let g = &report.geometry;
    let conv = g.conversion.as_ref().unwrap();
    let selected_h5m = backend.stochastic_volume_h5m.borrow()[0].clone();
    let attempt_dir = selected_h5m
        .parent()
        .expect("H5M has its attempt directory");
    assert_eq!(attempt_dir.file_name().unwrap(), "attempt-01");
    assert_eq!(
        attempt_dir.parent().unwrap().file_name().unwrap(),
        "convert"
    );
    assert_eq!(Path::new(&conv.directory), attempt_dir);
    assert_eq!(
        super::super::sha256_file(&selected_h5m).unwrap(),
        conv.h5m_sha256
    );
    assert!(!conv.reused && conv.chord_cm == 0.1 && conv.angular_rad == 0.1);
    let adaptive = g.adaptive_chord.as_ref().unwrap();
    assert!(adaptive.was_default && !adaptive.exhausted);
    assert_eq!(adaptive.attempt_records.len(), 1);
    assert_eq!(
        adaptive.attempt_records[0].status,
        "VOLUME_AND_BINDING_GATES_PASSED"
    );
    let gate = g.volume_gate.as_ref().unwrap();
    assert_eq!(gate.tolerance_source, "default");
    assert!(gate.solids.iter().all(|s| s.verdict == "WITHIN"));
    assert_eq!(conv.volumes.len(), 2);
    assert!(conv.volumes[0].triangles > 0 && conv.volumes[0].vertices > 0);
    assert_eq!(g.binding.len(), 2);
    assert_eq!(g.binding[0].solid_id.as_deref(), Some("shell"));
    let stochastic = g.stochastic_volume.as_ref().unwrap();
    assert_eq!(stochastic.rows.len(), 2);
    assert_eq!(stochastic.samples, 100_000);
    assert_eq!(stochastic.unit, "cm3");
    assert!(stochastic.scope.contains("geometry-only"));
    assert!(stochastic.rows.iter().all(|row| row.verdict == "WITHIN"));
    for (stochastic, binding) in stochastic.rows.iter().zip(&g.binding) {
        assert_eq!(stochastic.volume, binding.volume);
        assert_eq!(stochastic.global_id, binding.global_id);
        assert_eq!(stochastic.solid_id, binding.solid_id.as_deref().unwrap());
    }
    let integrity = g.integrity.as_ref().unwrap();
    assert_eq!(integrity.volumes.len(), 2);
    assert!(integrity.partition.as_ref().unwrap().sigmas <= 3.0);
    assert_eq!(g.deviation.as_ref().unwrap().solids.len(), 2);
    let sources = g.sources.as_ref().unwrap();
    assert_eq!((sources.inside, sources.outside), (500, 0));
    assert!(g.stages.iter().any(|s| s.stage == "check 7"));
    // the 13 integrity sub-checks are all listed
    assert_eq!(report.checks[2].sub_checks.len(), 13);
    assert!(report.checks[2].sub_checks.iter().all(|s| s.status == Pass));
}

// Verifies: GEO-012, GEO-030
#[test]
fn the_default_tolerance_is_gated_at_a_tenth_of_a_percent_and_recorded() {
    let backend = geo();
    let mut d = design(Vec::new());
    assert!(d.cad.faceting_tolerance_m.is_none());
    // the CAD volume of the shell is 0.2 % above its facets
    d.solids[0].fingerprint.cad_volume_m3 /= 1.002;
    let report = run_with(&d, &backend);
    assert_eq!(statuses(&report), [(6, Pass), (7, Fail)]);
    let finding = report.checks[1]
        .findings
        .iter()
        .find(|f| f.requirement.as_deref() == Some("GEO-030"))
        .unwrap();
    assert!(finding.why.contains("0.1 %"), "{finding:?}");
    let gate = report.geometry.volume_gate.as_ref().unwrap();
    assert_eq!(gate.tolerance_source, "default");
    assert!((gate.chord_m - DEFAULT_CHORD_M / 4096.0).abs() < 1e-15 && gate.limit == 0.001);
    assert_eq!(gate.solids[0].verdict, "EXCEEDED");
    assert_eq!(gate.solids[1].verdict, "WITHIN");
    assert!(report.geometry.adaptive_chord.as_ref().unwrap().exhausted);
    assert_eq!(
        report
            .geometry
            .adaptive_chord
            .as_ref()
            .unwrap()
            .attempt_records
            .len(),
        DEFAULT_CHORD_ATTEMPTS
    );
    let expected_chords: Vec<f64> = (0..DEFAULT_CHORD_ATTEMPTS)
        .map(|attempt| DEFAULT_CHORD_M / (1_u64 << attempt) as f64)
        .collect();
    let observed_chords = backend.conversion_chords.borrow();
    assert_eq!(observed_chords.len(), expected_chords.len());
    for (index, (observed, expected)) in observed_chords.iter().zip(&expected_chords).enumerate() {
        assert!(
            (observed - expected).abs() <= expected.abs() * 1.0e-12,
            "attempt {} used chord {observed:.6e}, expected {expected:.6e}",
            index + 1
        );
    }
    drop(observed_chords);
    assert_eq!(
        report
            .geometry
            .adaptive_chord
            .as_ref()
            .unwrap()
            .candidate_status,
        "RETAINED_VOLUME_GATE_FAILURE"
    );
    // the same model at a tolerance the user set is judged at 0.5 %, and the report says so
    d.cad.faceting_tolerance_m = Some(1.0e-3);
    let report = run_with(&d, &backend);
    assert_eq!(
        statuses(&report),
        [(6, Pass), (7, Pass), (8, Pass), (9, Pass)]
    );
    let gate = report.geometry.volume_gate.as_ref().unwrap();
    assert_eq!(
        (gate.tolerance_source.as_str(), gate.limit),
        ("design file", 0.005)
    );
    assert_eq!(gate.solids[0].verdict, "WITHIN");
    let adaptive = report.geometry.adaptive_chord.as_ref().unwrap();
    assert!(!adaptive.was_default && !adaptive.exhausted);
    assert_eq!(adaptive.attempt_records.len(), 1);
}

#[test]
fn the_default_refinement_schedule_is_bounded_and_explicit_tolerance_is_single_shot() {
    for attempt in 0..DEFAULT_CHORD_ATTEMPTS {
        assert_eq!(
            adaptive_chord_for_attempt(DEFAULT_CHORD_M, true, attempt),
            Some(DEFAULT_CHORD_M / (1_u64 << attempt) as f64)
        );
    }
    assert_eq!(
        adaptive_chord_for_attempt(DEFAULT_CHORD_M, true, DEFAULT_CHORD_ATTEMPTS),
        None
    );
    assert_eq!(adaptive_chord_for_attempt(0.0025, false, 0), Some(0.0025));
    assert_eq!(adaptive_chord_for_attempt(0.0025, false, 1), None);
    assert_eq!(adaptive_chord_for_attempt(0.0, true, 0), None);
}

#[test]
fn default_chord_refines_and_rechecks_the_selected_mesh_and_deviation_at_that_chord() {
    let backend = geo();
    backend
        .conversion_candidates
        .borrow_mut()
        .extend([shell_and_core(10.01, 2.0), imprinted()]);

    let report = run_with(&design(Vec::new()), &backend);

    assert_eq!(
        statuses(&report),
        [(6, Pass), (7, Pass), (8, Pass), (9, Pass)]
    );
    let selected = DEFAULT_CHORD_M * 0.5;
    let adaptive = report.geometry.adaptive_chord.as_ref().unwrap();
    assert!(adaptive.was_default && !adaptive.exhausted);
    assert_eq!(adaptive.attempt_records.len(), 2);
    assert_eq!(adaptive.attempt_records[0].status, "REFINEMENT_REQUIRED");
    assert_eq!(
        adaptive.attempt_records[1].status,
        "VOLUME_AND_BINDING_GATES_PASSED"
    );
    assert_ne!(
        adaptive.attempt_records[0].facets_sha256,
        adaptive.attempt_records[1].facets_sha256
    );
    assert_eq!(adaptive.selected_chord_m, Some(selected));
    assert_eq!(adaptive.candidate_status, "VOLUME_AND_BINDING_GATES_PASSED");
    assert_eq!(
        backend.conversion_chords.borrow().as_slice(),
        &[DEFAULT_CHORD_M, selected]
    );
    assert_eq!(
        backend.tessellation_chords.borrow().as_slice(),
        &[DEFAULT_CHORD_M, selected]
    );
    assert_eq!(
        backend.deviation_chords.borrow().last().copied(),
        Some(selected)
    );
    assert_eq!(
        report.geometry.pre_imprint.as_ref().unwrap().chord_m,
        selected
    );
    let conversion = report.geometry.conversion.as_ref().unwrap();
    assert_eq!(conversion.chord_cm, selected * 100.0);
    let selected_h5m = backend.stochastic_volume_h5m.borrow()[0].clone();
    let attempt_dir = selected_h5m
        .parent()
        .expect("H5M has its attempt directory");
    assert_eq!(attempt_dir.file_name().unwrap(), "attempt-02");
    assert_eq!(
        attempt_dir.parent().unwrap().file_name().unwrap(),
        "convert"
    );
    assert_eq!(Path::new(&conversion.directory), attempt_dir);
    assert_eq!(
        super::super::sha256_file(&selected_h5m).unwrap(),
        conversion.h5m_sha256
    );
}

#[test]
fn later_conversion_failure_marks_the_previous_candidate_as_retained_not_accepted() {
    let mut backend = geo();
    backend
        .conversion_candidates
        .borrow_mut()
        .push(shell_and_core(10.01, 2.0));
    backend.convert_failure = Some("the refined conversion failed".into());
    backend.convert_failure_after = Some(1);

    let report = run_with(&design(Vec::new()), &backend);

    assert_eq!(statuses(&report), [(6, Fail)]);
    let adaptive = report.geometry.adaptive_chord.as_ref().unwrap();
    assert_eq!(adaptive.selected_chord_m, Some(DEFAULT_CHORD_M));
    assert_eq!(
        adaptive.candidate_status,
        "RETAINED_AFTER_CONVERSION_FAILURE"
    );
    assert_eq!(adaptive.attempt_records.len(), 2);
    assert_eq!(adaptive.attempt_records[0].status, "REFINEMENT_REQUIRED");
    assert_eq!(adaptive.attempt_records[1].status, "CONVERSION_FAILED");
    assert_eq!(report.geometry.conversion.as_ref().unwrap().chord_cm, 0.1);
}

#[test]
fn later_conversion_unavailability_marks_the_previous_candidate_as_unusable_for_readiness() {
    let mut backend = geo();
    backend
        .conversion_candidates
        .borrow_mut()
        .push(shell_and_core(10.01, 2.0));
    backend.convert_unavailable_after = Some(1);

    let report = run_with(&design(Vec::new()), &backend);

    assert_eq!(statuses(&report), [(6, NotEvaluated)]);
    let adaptive = report.geometry.adaptive_chord.as_ref().unwrap();
    assert_eq!(adaptive.selected_chord_m, Some(DEFAULT_CHORD_M));
    assert_eq!(
        adaptive.candidate_status,
        "RETAINED_AFTER_CONVERSION_UNAVAILABLE"
    );
    assert_eq!(adaptive.attempt_records.len(), 2);
    assert_eq!(adaptive.attempt_records[0].status, "REFINEMENT_REQUIRED");
    assert_eq!(adaptive.attempt_records[1].status, "CONVERSION_UNAVAILABLE");
    assert_eq!(report.geometry.conversion.as_ref().unwrap().chord_cm, 0.1);
    assert!(report.geometry.volume_gate.is_none());
}

#[test]
fn adaptive_chord_stops_immediately_when_conversion_drops_a_solid() {
    let backend = geo();
    backend
        .conversion_candidates
        .borrow_mut()
        .push(separate_boxes(&[([0.0; 3], [2.0; 3])]));

    let report = run_with(&design(Vec::new()), &backend);

    let adaptive = report.geometry.adaptive_chord.as_ref().unwrap();
    assert!(adaptive.was_default && !adaptive.exhausted);
    assert_eq!(adaptive.attempt_records.len(), 1);
    assert_eq!(adaptive.attempt_records[0].status, "BINDING_FAILED");
    assert_eq!(adaptive.attempt_records[0].volume_count, 1);
    assert_eq!(adaptive.attempt_records[0].solid_count, 2);
    assert!(
        adaptive.attempt_records[0]
            .binding_findings
            .iter()
            .any(|finding| finding.contains("every solid must be one volume"))
    );
}

// Verifies: GEO-030
#[test]
fn openmc_stochastic_volume_exceedance_is_reported_per_cell_and_cannot_be_waived() {
    let mut backend = geo();
    backend.stochastic_volume_exceeded = true;
    let report = run_with(&design(Vec::new()), &backend);
    assert_eq!(report.checks[1].status, Fail);
    let finding = report.checks[1]
        .findings
        .iter()
        .find(|finding| finding.requirement.as_deref() == Some("GEO-030"))
        .unwrap();
    assert_eq!(finding.cells, ["shell"]);
    assert!(finding.why.contains("max(0.5% of CAD, 3 SE)"));
    assert!(!finding.waivable);
    assert_eq!(
        report.geometry.stochastic_volume.as_ref().unwrap().rows[0].verdict,
        "EXCEEDED"
    );
    let waived = run_with(&design(vec![waiver("GEO-030", &["shell"])]), &backend);
    assert_eq!(waived.checks[1].status, Fail);
}

#[test]
fn missing_openmc_volume_measurement_is_not_evaluated_or_accepted() {
    let mut backend = geo();
    backend.stochastic_volume_unavailable = true;
    let report = run_with(&design(Vec::new()), &backend);
    assert_eq!(report.checks[1].status, NotEvaluated);
    assert!(report.geometry.stochastic_volume.is_none());
    assert_eq!(report.checks.len(), 2);
    assert_eq!(
        backend.count(5),
        0,
        "check 8 cannot follow an unresolved GEO-030 gate"
    );
}

// Verifies: GEO-021
#[test]
fn a_cad_overlap_stops_before_the_conversion() {
    let backend = geo();
    let mut b = backend;
    b.tess = separate_boxes(&[([0.0; 3], [2.0; 3]), ([1.0; 3], [3.0; 3])]);
    b.booleans.borrow_mut().push(BooleanResult {
        a: 0,
        b: 1,
        status: "decided".into(),
        common_volume_m3: Some(1.0),
        reason: None,
        seconds: None,
    });
    let report = run_with(&design(Vec::new()), &b);
    assert_eq!(statuses(&report), [(6, Fail)]);
    assert_eq!(b.count(2), 0, "no conversion of overlapping solids");
    let f = &report.checks[0].findings[0];
    assert_eq!(f.requirement.as_deref(), Some("GEO-021"));
    assert_eq!(f.cells, ["core", "shell"]);
    assert!(f.waivable && f.location_m.is_some() && f.blocks());
    assert_eq!(
        report.geometry.pre_imprint.as_ref().unwrap().pairs[0].verdict,
        "overlap"
    );
}

// Verifies: GEO-021, GEO-026
#[test]
fn a_waived_overlap_is_reported_and_no_longer_blocks() {
    let mut b = geo();
    b.tess = separate_boxes(&[([0.0; 3], [2.0; 3]), ([1.0; 3], [3.0; 3])]);
    b.booleans.borrow_mut().push(BooleanResult {
        a: 0,
        b: 1,
        status: "undecided".into(),
        common_volume_m3: None,
        reason: Some("OpenCASCADE error".into()),
        seconds: None,
    });
    let d = design(vec![
        waiver("GEO-021", &["shell", "core"]),
        waiver("GEO-022", &["shell"]),
    ]);
    let report = run_with(&d, &b);
    assert_eq!(b.count(2), 1, "the waived finding lets the conversion run");
    assert_eq!(report.checks[0].status, Pass);
    let f = &report.checks[0].findings[0];
    assert!(f.waived.is_some() && !f.blocks());
    assert!(f.why.contains("OpenCASCADE error"));
    // the waiver that matched nothing is named
    assert_eq!(report.geometry.waivers.len(), 2);
    assert_eq!(report.geometry.waivers[0].covered_findings, 1);
    assert!(report.notices.iter().any(|n| n.item == "waivers[1]"));
}

// Verifies: GEO-012
#[test]
fn a_failed_conversion_is_a_finding_and_stops() {
    let mut b = geo();
    b.convert_failure = Some("ValueError: Overlapping solids".into());
    let report = run_with(&design(Vec::new()), &b);
    assert_eq!(statuses(&report), [(6, Fail)]);
    let f = &report.checks[0].findings[0];
    assert_eq!(f.requirement.as_deref(), Some("GEO-012"));
    assert!(f.why.contains("Overlapping solids"));
    assert!(!f.waivable);
}

// Verifies: GEO-019
#[test]
fn a_tag_on_the_wrong_solid_stops_at_check_7() {
    let mut b = geo();
    b.tags = vec!["mat:core".into(), "mat:shell".into()];
    let report = run_with(&design(Vec::new()), &b);
    assert_eq!(statuses(&report), [(6, Pass), (7, Fail)]);
    assert!(
        report.checks[1].findings[0]
            .why
            .contains("tag on the wrong solid")
    );
    assert_eq!(
        b.count(3) + b.count(4) + b.count(5),
        0,
        "later stages did not run"
    );
}

// Verifies: GEO-020
#[test]
fn an_integrity_failure_stops_before_the_source_sites() {
    let mut b = geo();
    // an open edge: the core loses a triangle
    let mut f = imprinted();
    f.triangles.remove(14);
    f.surfaces[1].triangle_count -= 1;
    // the fingerprints are those of the damaged model, so binding passes and integrity fails
    let d = design_of(&f, Vec::new());
    b.imprinted = f;
    let report = run_with(&d, &b);
    assert_eq!(statuses(&report), [(6, Pass), (7, Pass), (8, Fail)]);
    assert!(
        report.checks[2]
            .findings
            .iter()
            .any(|f| f.requirement.as_deref() == Some("GEO-020") && !f.waivable)
    );
    assert_eq!(b.count(5), 0);
}

// Verifies: GEO-032
#[test]
fn unresolved_global_deviation_bound_does_not_accept_a_sampled_run_or_allow_waiver() {
    let mut b = geo();
    b.deviation.as_mut().unwrap()[1].global_bound_status = "unresolved".into();
    b.deviation.as_mut().unwrap()[1].global_upper_bound_m = Some(5.0e-3);
    let report = run_with(&design(Vec::new()), &b);
    assert_eq!(report.checks[2].status, NotEvaluated);
    assert!(
        report.checks[2]
            .why
            .as_deref()
            .unwrap()
            .contains("global triangle bound was unresolved")
    );
    let waived = run_with(&design(vec![waiver("GEO-032", &["core"])]), &b);
    assert_eq!(waived.checks[2].status, NotEvaluated);
    assert_eq!(statuses(&waived).len(), 4);
}

// Verifies: GEO-032
#[test]
fn a_capped_deviation_says_it_measured_a_stride() {
    let mut b = geo();
    b.deviation.as_mut().unwrap()[0].points = 50;
    let report = run_with(&design(Vec::new()), &b);
    let record = report.geometry.deviation.as_ref().unwrap();
    assert!(record.statement.contains("certified upper bound"));
    assert_eq!(record.solids[0].points_available, 100);
    assert!(!record.solids[0].complete && record.solids[1].complete);
    let row_json = serde_json::to_value(&record.solids[0]).unwrap();
    assert!(row_json.get("sampled_max_distance_m").is_some());
    assert!(row_json.get("max_distance_m").is_none());
    let plain = run_with(&design(Vec::new()), &geo());
    let statement = &plain.geometry.deviation.as_ref().unwrap().statement;
    assert!(
        statement.starts_with("certified upper bound"),
        "{statement}"
    );
    assert_eq!(record.solids[0].global_bound_status, "certified");
}

// Verifies: GEO-032
#[test]
fn a_large_sample_distance_cannot_substitute_for_a_global_bound() {
    let mut b = geo();
    b.deviation.as_mut().unwrap()[1].max_distance_m = 5.0e-3;
    b.deviation.as_mut().unwrap()[1].global_bound_status = "unresolved".into();
    let all = run_with(&design(Vec::new()), &b);
    assert_eq!(all.checks[2].status, NotEvaluated);
    assert!(
        !all.checks[2]
            .findings
            .iter()
            .any(|f| f.requirement.as_deref() == Some("GEO-032"))
    );
}

// Verifies: GEO-032
#[test]
fn a_deviation_over_a_tenth_of_the_thickness_is_a_waivable_misrepresented_sliver() {
    // core is a cube of side 4 m: 2V/A = 1.333 m, so 0.2 m is 15 %
    let mut b = geo();
    b.deviation.as_mut().unwrap()[1].max_distance_m = 0.2;
    let slivers = |r: &ImportReport| {
        r.checks[2]
            .findings
            .iter()
            .filter(|f| f.requirement.as_deref() == Some("SLIVER"))
            .cloned()
            .collect::<Vec<_>>()
    };
    let report = run_with(&design(Vec::new()), &b);
    let found = slivers(&report);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].item.contains("thickness misrepresented"));
    assert!(found[0].why.contains("2V/A of 1.333"), "{}", found[0].why);
    assert!(found[0].why.contains("maximum over all points"));
    assert!(found[0].blocks() && found[0].waivable);
    assert_eq!(report.checks[2].status, Fail);
    let waived = run_with(
        &design(vec![
            waiver("SLIVER", &["core"]),
            waiver("GEO-032", &["core"]),
        ]),
        &b,
    );
    assert_eq!(waived.checks[2].status, Pass);
    assert!(slivers(&waived).iter().all(|f| f.waived.is_some()));
    // a partial deviation says so in the sliver finding too
    b.deviation.as_mut().unwrap()[1].points = 10;
    let sampled = run_with(&design(Vec::new()), &b);
    assert!(slivers(&sampled)[0].why.contains("sampled maximum"));
    // a deviation of exactly the rule's size or less passes
    b.deviation.as_mut().unwrap()[1].max_distance_m = 0.13;
    b.deviation.as_mut().unwrap()[1].points = 100;
    let quiet = run_with(&design(vec![waiver("GEO-032", &["core"])]), &b);
    assert!(slivers(&quiet).is_empty());
}

// Verifies: GEO-032
#[test]
fn without_geo_032_the_misrepresented_rule_cannot_run_and_the_sliver_check_says_so() {
    let mut b = geo();
    b.deviation = None;
    let report = run_with(&design(Vec::new()), &b);
    let sub = report.checks[2]
        .sub_checks
        .iter()
        .find(|s| s.name == "slivers")
        .unwrap();
    assert!(
        sub.why.as_deref().is_some_and(|w| w.contains("GEO-032")),
        "{sub:?}"
    );
}

#[test]
fn a_deviation_helper_that_is_unavailable_is_not_evaluated_not_pass() {
    let mut b = geo();
    b.deviation = None;
    let report = run_with(&design(Vec::new()), &b);
    assert_eq!(report.checks[2].status, NotEvaluated);
    assert!(
        report.checks[2]
            .why
            .as_deref()
            .unwrap()
            .contains("no CAD interpreter")
    );
    let sub = report.checks[2]
        .sub_checks
        .iter()
        .find(|s| s.name.contains("GEO-032"))
        .unwrap();
    assert_eq!(sub.status, NotEvaluated);
    // check 9 still runs
    assert_eq!(report.checks[3].status, Pass);
}

// Verifies: SRC-018
#[test]
fn a_site_outside_the_plasma_chamber_fails_check_9() {
    let mut b = geo();
    b.sites[10] = [5.0, 0.0, 0.0]; // inside the shell, not the core
    b.sites[11] = [100.0, 0.0, 0.0]; // outside everything
    let report = run_with(&design(Vec::new()), &b);
    assert_eq!(report.checks[3].status, Fail);
    let f = &report.checks[3].findings[0];
    assert_eq!(f.requirement.as_deref(), Some("SRC-018"));
    assert!(!f.waivable);
    assert_eq!(f.location_m, Some([5.0, 0.0, 0.0]));
    let s = report.geometry.sources.as_ref().unwrap();
    assert_eq!((s.inside, s.outside), (498, 2));
    assert_eq!(s.outside_in["shell"], 1);
    assert_eq!(s.outside_in["(outside every solid)"], 1);
}

// Verifies: SRC-018
#[test]
fn a_site_on_the_chamber_surface_is_not_inside() {
    let mut b = geo();
    b.sites[0] = [2.0, 0.0, 0.0];
    let report = run_with(&design(Vec::new()), &b);
    assert_eq!(report.checks[3].status, Fail);
}

// Verifies: SRC-018
#[test]
fn the_clearance_to_the_first_wall_is_measured_and_reported() {
    let b = geo();
    let report = run_with(&design(Vec::new()), &b);
    assert_eq!(report.checks[3].status, Pass);
    let s = report.geometry.sources.as_ref().unwrap();
    assert_eq!(s.first_wall_solids, ["shell"]);
    // The shell's nearest surface to the sites is its inner face at 2 m (the core's outer face).
    let reach = b
        .sites
        .iter()
        .flat_map(|p| p.iter().map(|c| c.abs()))
        .fold(0.0, f64::max);
    let clearance = s.first_wall_clearance_m.unwrap();
    assert!(
        (clearance - (2.0 - reach)).abs() < 1.0e-6,
        "{clearance} against {}",
        2.0 - reach
    );
    let at = s.first_wall_clearance_at_m.unwrap();
    assert!(at.iter().any(|c| (c.abs() - reach).abs() < 1.0e-12));
    assert_eq!(s.in_first_wall, 0);
    let note = report.checks[3].sub_checks[0].why.as_deref().unwrap();
    assert!(note.contains("nearest site to the first wall"), "{note}");
}

// Verifies: SRC-018
#[test]
fn a_site_inside_the_first_wall_fails_check_9_with_zero_clearance() {
    let b = geo();
    let mut d = design(Vec::new());
    // The plasma chamber is also the first wall: every site is inside a first-wall solid.
    d.solids[1].role = Some(Role::FirstWall);
    let report = run_with(&d, &b);
    assert_eq!(report.checks[3].status, Fail);
    let f = &report.checks[3].findings[0];
    assert_eq!(f.requirement.as_deref(), Some("SRC-018"));
    assert!(f.why.contains("inside a first-wall solid"), "{}", f.why);
    assert!(!f.waivable);
    assert_eq!(report.geometry.sources.as_ref().unwrap().in_first_wall, 500);
}

// Verifies: SRC-018
#[test]
fn a_design_without_a_first_wall_says_the_clearance_was_not_measured() {
    let b = geo();
    let mut d = design(Vec::new());
    d.solids[0].role = Some(Role::Blanket);
    let report = run_with(&d, &b);
    assert_eq!(report.checks[3].status, Pass);
    let s = report.geometry.sources.as_ref().unwrap();
    assert_eq!(s.first_wall_clearance_m, None);
    let note = report.checks[3].sub_checks[0].why.as_deref().unwrap();
    assert!(note.contains("no solid has the role first_wall"), "{note}");
}

/// The D-T power the fixture's profiles imply, MW (RM-M's plasma block with a 1.0 m pedestal, its densities scaled to close at 525 MW).
fn implied_power_mw(d: &Design) -> f64 {
    source::closure(&d.plasma).unwrap().implied_fusion_power_mw
}

// Verifies: SRC-010
#[test]
fn a_declared_power_that_closes_with_the_profiles_passes_check_9() {
    let b = geo();
    let report = run_with(&design(Vec::new()), &b);
    assert_eq!(report.checks[3].status, Pass);
    assert!(report.checks[3].findings.is_empty());
    let closure = report.geometry.source_closure.as_ref().unwrap();
    assert!(closure.closes);
    assert_eq!(closure.declared_fusion_power_mw, 525.0);
    let notice = report
        .notices
        .iter()
        .find(|n| n.item == "plasma.fusion_power_mw")
        .unwrap();
    assert_eq!(notice.check, 9);
    assert!(
        notice.message.contains("closes with the profiles"),
        "{}",
        notice.message
    );
}

// Verifies: SRC-010
#[test]
fn a_declared_power_beyond_the_tolerance_blocks_check_9_and_says_what_to_do() {
    let b = geo();
    let mut d = design(Vec::new());
    let implied = implied_power_mw(&d);
    d.plasma.fusion_power_mw = Some(implied * 1.01);
    let report = run_with(&d, &b);
    assert_eq!(report.checks[3].status, Fail);
    let f = &report.checks[3].findings[0];
    assert_eq!(f.requirement.as_deref(), Some("SRC-010"));
    assert_eq!(f.item, "plasma.fusion_power_mw");
    assert!(f.waivable && f.blocks());
    assert_eq!(f.cells, vec!["core"]);
    assert!(
        f.why.contains(&format!("{:.2} MW", implied * 1.01))
            && f.why.contains(&format!("{implied:.2} MW")),
        "{}",
        f.why
    );
    assert!(f.why.contains("ratio 0.9901"), "{}", f.why);
    assert!(f.why.contains("declared power alone"), "{}", f.why);
    assert!(
        f.next_step.contains("Correct the profiles") && f.next_step.contains("waiver"),
        "{}",
        f.next_step
    );
    assert!(!report.geometry.source_closure.as_ref().unwrap().closes);
    // Inside the 1e-3 tolerance it does not block.
    d.plasma.fusion_power_mw = Some(implied * 1.0005);
    assert_eq!(run_with(&d, &b).checks[3].status, Pass);
}

// Verifies: SRC-010
#[test]
fn a_waiver_with_a_reason_lets_a_closure_mismatch_through_and_stays_in_the_report() {
    let b = geo();
    let mut d = design(vec![waiver("SRC-010", &["core"])]);
    d.plasma.fusion_power_mw = Some(implied_power_mw(&d) * 2.0);
    let report = run_with(&d, &b);
    assert_eq!(report.checks[3].status, Pass);
    let f = &report.checks[3].findings[0];
    assert!(f.waived.is_some() && !f.blocks());
    assert_eq!(f.waived.as_ref().unwrap().reason, "accepted after review");
    assert_eq!(report.geometry.waivers[0].covered_findings, 1);
    // A waiver that names another solid covers nothing.
    let mut d = design(vec![waiver("SRC-010", &["shell"])]);
    d.plasma.fusion_power_mw = Some(implied_power_mw(&d) * 2.0);
    assert_eq!(run_with(&d, &b).checks[3].status, Fail);
}

// Verifies: SRC-010
#[test]
fn a_plasma_block_that_cannot_be_integrated_keeps_its_notice_and_does_not_block() {
    let b = geo();
    let mut d = design(Vec::new());
    d.plasma.pedestal_radius_m = Some(5.0);
    let report = run_with(&d, &b);
    assert_eq!(report.checks[3].status, Pass);
    assert!(report.geometry.source_closure.is_none());
    let notice = report
        .notices
        .iter()
        .find(|n| n.item == "plasma.fusion_power_mw")
        .unwrap();
    assert!(
        notice.message.contains("pedestal_radius_m"),
        "{}",
        notice.message
    );
}

// Verifies: SRC-018
#[test]
fn a_clearance_below_the_users_minimum_fails_check_9_and_is_not_waivable() {
    let b = geo();
    let measured = {
        let report = run_with(&design(Vec::new()), &b);
        report
            .geometry
            .sources
            .as_ref()
            .unwrap()
            .first_wall_clearance_m
            .unwrap()
    };
    let mut d = design(vec![waiver("SRC-018", &["shell"])]);
    d.plasma.min_first_wall_clearance_m = Some(measured * 1.01);
    let report = run_with(&d, &b);
    assert_eq!(report.checks[3].status, Fail);
    let f = &report.checks[3].findings[0];
    assert_eq!(f.requirement.as_deref(), Some("SRC-018"));
    assert!(!f.waivable && f.waived.is_none());
    assert!(f.why.contains("min_first_wall_clearance_m"), "{}", f.why);
    assert!(f.location_m.is_some());
    // At the measured value, or with no minimum, it passes and still reports the distance.
    d.plasma.min_first_wall_clearance_m = Some(measured * 0.99);
    let report = run_with(&d, &b);
    assert_eq!(report.checks[3].status, Pass);
    let note = report.checks[3].sub_checks[0].why.as_deref().unwrap();
    assert!(note.contains("sets a minimum of"), "{note}");
    d.plasma.min_first_wall_clearance_m = None;
    let report = run_with(&d, &b);
    let note = report.checks[3].sub_checks[0].why.as_deref().unwrap();
    assert!(note.contains("sets no minimum"), "{note}");
}

// Verifies: SRC-018
#[test]
fn a_minimum_clearance_without_a_first_wall_is_not_evaluated() {
    let b = geo();
    let mut d = design(Vec::new());
    d.solids[0].role = Some(Role::Blanket);
    d.plasma.min_first_wall_clearance_m = Some(0.1);
    let report = run_with(&d, &b);
    assert_eq!(report.checks[3].status, NotEvaluated);
    assert!(
        report.checks[3]
            .why
            .as_deref()
            .unwrap()
            .contains("no solid has the role first_wall")
    );
}

// Verifies: SRC-018
#[test]
fn the_nearest_wall_search_agrees_with_a_brute_force_distance() {
    let mesh = super::super::mesh::Mesh::from_facets(&imprinted()).unwrap();
    let tris = mesh.volume_triangles()[0].clone();
    let walls = Bvh::build(&mesh, tris.clone());
    let mut rng = Rng::new(7);
    for _ in 0..200 {
        let p = [0; 3].map(|_| super::super::mesh::to_grid(rng.next_f64() * 24.0 - 12.0).unwrap());
        let brute = tris
            .iter()
            .map(|&t| overlap::point_triangle_distance(p, mesh.tri_pts(t)))
            .fold(f64::INFINITY, f64::min);
        let fast = nearest_nm(&mesh, &walls, p, f64::INFINITY, mesh.diagonal() + 1.0).unwrap();
        assert!(
            (brute - fast).abs() < 1.0e-6,
            "{p:?}: {brute} against {fast}"
        );
        // With a bound at or below the answer, nothing nearer is found.
        assert_eq!(nearest_nm(&mesh, &walls, p, brute * 0.5, 1.0e12), None);
    }
}
