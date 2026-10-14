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
    tags: Vec<String>,
    tess: Facets,
    booleans: RefCell<Vec<BooleanResult>>,
    convert_failure: Option<String>,
    deviation: Option<Vec<DeviationRow>>,
    sites: Vec<[f64; 3]>,
    calls: Cell<[u32; 5]>,
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
        _: &TessellateRequest,
    ) -> Result<ToolResult<TessellateOutcome>, DesignError> {
        self.bump(0);
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
        if let Some(why) = &self.convert_failure {
            return Ok(ToolResult::Failed {
                why: why.clone(),
                next_step: "Fix it.".into(),
            });
        }
        let (facets, manifest) = pair_of(&self.imprinted, &self.tags);
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
                sha256: "a".repeat(64),
                bytes: 10,
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
            report,
            facets,
            manifest,
            facets_bin: PathBuf::from("model.facets.bin"),
            reused: false,
            helper_sha256: "h".repeat(64),
        }))
    }
    fn deviation(&self, _: &DeviationRequest) -> Result<ToolResult<DeviationOutcome>, DesignError> {
        self.bump(3);
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
        self.bump(4);
        Ok(ToolResult::Done(SourceOutcome {
            points: self.sites.clone(),
            versions: serde_json::json!({"openmc": "0.15.3"}),
            bin_sha256: "s".repeat(64),
            seconds: 0.2,
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
        tags: vec!["mat:shell".into(), "mat:core".into()],
        tess: separate_boxes(&[([-10.0; 3], [10.0; 3]), ([30.0; 3], [32.0; 3])]),
        booleans: RefCell::new(Vec::new()),
        convert_failure: None,
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
                at_m: Some([0.0, 0.0, 2.0]),
            },
        ]),
        sites,
        calls: Cell::new([0; 5]),
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
    assert!(!conv.reused && conv.chord_cm == 0.1 && conv.angular_rad == 0.1);
    let gate = g.volume_gate.as_ref().unwrap();
    assert_eq!(gate.tolerance_source, "default");
    assert!(gate.solids.iter().all(|s| s.verdict == "WITHIN"));
    assert_eq!(conv.volumes.len(), 2);
    assert!(conv.volumes[0].triangles > 0 && conv.volumes[0].vertices > 0);
    assert_eq!(g.binding.len(), 2);
    assert_eq!(g.binding[0].solid_id.as_deref(), Some("shell"));
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
    assert!((gate.chord_m - DEFAULT_CHORD_M).abs() < 1e-12 && gate.limit == 0.001);
    assert_eq!(gate.solids[0].verdict, "EXCEEDED");
    assert_eq!(gate.solids[1].verdict, "WITHIN");
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
    assert_eq!(b.count(3) + b.count(4), 0, "later stages did not run");
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
    assert_eq!(b.count(4), 0);
}

// Verifies: GEO-032
#[test]
fn deviation_above_the_tolerance_blocks_and_can_be_waived() {
    let mut b = geo();
    b.deviation.as_mut().unwrap()[1].max_distance_m = 5.0e-3;
    b.deviation.as_mut().unwrap()[1].points = 40;
    let report = run_with(&design(Vec::new()), &b);
    assert_eq!(report.checks[2].status, Fail);
    let f = report.checks[2]
        .findings
        .iter()
        .find(|f| f.requirement.as_deref() == Some("GEO-032"))
        .unwrap();
    assert_eq!(f.cells, ["core"]);
    assert!(f.why.contains("sampled maximum"));
    assert!(report.geometry.deviation.as_ref().unwrap().solids[1].above_tolerance);
    let waived = run_with(&design(vec![waiver("GEO-032", &["core"])]), &b);
    assert_eq!(waived.checks[2].status, Pass);
    assert_eq!(statuses(&waived).len(), 4);
}

// Verifies: GEO-032
#[test]
fn a_capped_deviation_says_it_measured_a_stride() {
    let mut b = geo();
    b.deviation.as_mut().unwrap()[0].points = 50;
    let report = run_with(&design(Vec::new()), &b);
    let record = report.geometry.deviation.as_ref().unwrap();
    assert!(record.statement.contains("--deviation-points"));
    assert!(record.statement.contains("sampled maximum"));
    assert_eq!(record.solids[0].points_available, 100);
    assert!(!record.solids[0].complete && record.solids[1].complete);
    let plain = run_with(&design(Vec::new()), &geo());
    let statement = &plain.geometry.deviation.as_ref().unwrap().statement;
    assert!(
        statement.starts_with("maximum over all points"),
        "{statement}"
    );
    assert!(!statement.contains("--deviation-points"));
}

// Verifies: GEO-032
#[test]
fn the_finding_above_the_tolerance_says_sampled_only_when_it_was() {
    let mut b = geo();
    b.deviation.as_mut().unwrap()[1].max_distance_m = 5.0e-3;
    let all = run_with(&design(Vec::new()), &b);
    let why = |r: &ImportReport| {
        r.checks[2]
            .findings
            .iter()
            .find(|f| f.requirement.as_deref() == Some("GEO-032"))
            .unwrap()
            .why
            .clone()
    };
    assert!(
        why(&all).contains("maximum over all points"),
        "{}",
        why(&all)
    );
    b.deviation.as_mut().unwrap()[1].points = 10;
    let some = run_with(&design(Vec::new()), &b);
    assert!(why(&some).contains("sampled maximum"), "{}", why(&some));
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

// Verifies: SRC-010
#[test]
fn the_declared_power_is_reported_next_to_the_profiles_power_without_changing_the_verdict() {
    let b = geo();
    let mut d = design(Vec::new());
    let rm_m: Design = serde_json::from_str(include_str!(
        "../../../../../scenarios/rm-m/rm-m.faris-design.json"
    ))
    .unwrap();
    d.plasma = rm_m.plasma;
    d.plasma.chamber_solid_id = Some("core".into());
    let report = run_with(&d, &b);
    // The RM-M profiles imply 190.5 MW against 525 MW declared: shown, not blocking.
    assert_eq!(report.checks[3].status, Pass);
    let closure = report.geometry.source_closure.as_ref().unwrap();
    assert!(!closure.closes);
    assert_eq!(closure.declared_fusion_power_mw, 525.0);
    let notice = report
        .notices
        .iter()
        .find(|n| n.item == "plasma.fusion_power_mw")
        .unwrap();
    assert_eq!(notice.check, 9);
    assert!(
        notice.message.contains("525.00 MW") && notice.message.contains("190.55 MW"),
        "{}",
        notice.message
    );
    assert!(
        notice.message.contains("Correct the profiles"),
        "{}",
        notice.message
    );
    // Made to close, the same block reports a closing notice.
    d.plasma.fusion_power_mw = Some(closure.implied_fusion_power_mw);
    let report = run_with(&d, &b);
    assert!(report.geometry.source_closure.as_ref().unwrap().closes);
    let notice = report
        .notices
        .iter()
        .find(|n| n.item == "plasma.fusion_power_mw")
        .unwrap();
    assert!(
        notice.message.contains("closes with the profiles"),
        "{}",
        notice.message
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
