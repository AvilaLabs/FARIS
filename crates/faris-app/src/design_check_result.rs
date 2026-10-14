//! The words and numbers of a finished design check, and of a freshly written draft, built
//! from the engine's results.
//!
//! No window and no physics: the report is the engine's, and this turns it into rows of text for
//! the panel to draw, so the wording can be tested. Status words are the engine's own; findings
//! keep the engine's item, why and next step; every number carries its unit. Nothing is built
//! before a result exists, and a record the engine did not measure says why it is absent.

use crate::design_check_run::InitDone;
use faris_engine::design::{CheckStatus, ImportReport};
use std::path::{Path, PathBuf};

/// How the import as a whole stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// All nine checks were evaluated and none failed.
    Complete,
    /// A check failed.
    Failed,
    /// No check failed, but one could not be fully evaluated.
    Incomplete,
}

impl Verdict {
    /// The engine's status word for the import.
    pub fn word(self) -> &'static str {
        match self {
            Self::Complete => "PASS",
            Self::Failed => "FAIL",
            Self::Incomplete => "NOT_EVALUATED",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FindingRow {
    pub item: String,
    pub why: String,
    pub next_step: String,
    /// The requirement or check id a waiver would name.
    pub requirement: Option<String>,
    pub solids: Option<String>,
    pub location: Option<String>,
    /// Who waived it, when, and why; a waived finding no longer stops the import.
    pub waived: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SubRow {
    pub name: String,
    pub status: CheckStatus,
    pub why: Option<String>,
    pub next_step: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CheckRow {
    pub number: u8,
    pub name: String,
    pub status: CheckStatus,
    /// The engine's word: `PASS`, `FAIL` or `NOT_EVALUATED`.
    pub status_word: &'static str,
    pub why: Option<String>,
    pub next_step: Option<String>,
    pub sub_checks: Vec<SubRow>,
    pub findings: Vec<FindingRow>,
}

/// One measured record: what it is, its value with its unit, and what it was judged against.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub caption: String,
    pub value: String,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CheckView {
    pub verdict: Verdict,
    /// The engine's summary line, as it words it.
    pub summary: String,
    /// What to do next, given the verdict.
    pub next_step: String,
    pub design_line: String,
    pub step_line: Option<String>,
    pub checks: Vec<CheckRow>,
    pub records: Vec<Record>,
    pub notices: Vec<String>,
    pub scope: String,
}

fn hash_head(hash: &str) -> &str {
    &hash[..16.min(hash.len())]
}

fn finding_row(finding: &faris_model::design::Finding) -> FindingRow {
    FindingRow {
        item: finding.item.clone(),
        why: finding.why.clone(),
        next_step: finding.next_step.clone(),
        requirement: finding.requirement.clone(),
        solids: (!finding.cells.is_empty()).then(|| finding.cells.join(", ")),
        location: finding
            .location_m
            .map(|p| format!("({:.6}, {:.6}, {:.6}) m", p[0], p[1], p[2])),
        waived: finding
            .waived
            .as_ref()
            .map(|w| format!("WAIVED by {} on {}: {}", w.author, w.date, w.reason)),
    }
}

fn check_rows(report: &ImportReport) -> Vec<CheckRow> {
    report
        .checks
        .iter()
        .map(|c| CheckRow {
            number: c.check,
            name: c.name.clone(),
            status: c.status,
            status_word: c.status.label(),
            why: c.why.clone(),
            next_step: c.next_step.clone(),
            sub_checks: c
                .sub_checks
                .iter()
                .map(|s| SubRow {
                    name: s.name.clone(),
                    status: s.status,
                    why: s.why.clone(),
                    next_step: s.next_step.clone(),
                })
                .collect(),
            findings: c.findings.iter().map(finding_row).collect(),
        })
        .collect()
}

/// Why a record is absent: the check that measures it did not get to.
fn absent(report: &ImportReport, check: u8) -> (String, String) {
    let status = report
        .checks
        .iter()
        .find(|c| c.check == check)
        .map_or("not run", |c| c.status.label());
    (
        "not measured".into(),
        format!(
            "check {check} is {status}, so this was not measured. Next step: see check {check} above."
        ),
    )
}

fn record(
    caption: &str,
    measured: Option<(String, String)>,
    otherwise: (String, String),
) -> Record {
    let (value, note) = measured.unwrap_or(otherwise);
    Record {
        caption: caption.into(),
        value,
        note,
    }
}

fn records(report: &ImportReport) -> Vec<Record> {
    let g = &report.geometry;
    vec![
    record(
        "Solids in the STEP file",
        report.step.as_ref().and_then(|s| s.solids).map(|n| {
            (
                format!("{n} solids"),
                format!("length unit {}", report.step.as_ref().map_or("?", |s| &s.length_unit)),
            )
        }),
        absent(report, 3),
    ),
    record(
        "Conversion tolerance",
        g.conversion.as_ref().map(|c| {
            (
                format!("chord {} cm, angular {} rad", c.chord_cm, c.angular_rad),
                format!(
                    "the faceting tolerance the model was converted at{}",
                    if c.reused { "; REUSED from an earlier run" } else { "" }
                ),
            )
        }),
        absent(report, 6),
    ),
    record(
        "Triangles in the converted model",
        g.conversion.as_ref().map(|c| {
            let triangles: u64 = c.volumes.iter().map(|v| v.triangles).sum();
            (
                format!("{triangles} triangles"),
                format!("in {} volumes", c.volumes.len()),
            )
        }),
        absent(report, 6),
    ),
    record(
        "Largest volume difference, faceted against CAD",
        g.volume_gate.as_ref().map(|v| {
            let worst = v
                .solids
                .iter()
                .max_by(|a, b| a.relative_difference.total_cmp(&b.relative_difference));
            (
                format!("{:.4} %", 100.0 * v.largest_relative_difference),
                format!(
                    "the gate is {:.3} % per solid ({}){}",
                    100.0 * v.limit,
                    v.tolerance_source,
                    worst.map_or(String::new(), |w| format!("; the largest is in {}", w.solid_id))
                ),
            )
        }),
        absent(report, 7),
    ),
    record(
        "Faceting deviation, largest over all solids",
        g.deviation.as_ref().map(|d| {
            let worst = d
                .solids
                .iter()
                .max_by(|a, b| a.max_distance_m.total_cmp(&b.max_distance_m));
            (
                worst.map_or("no solids".into(), |w| format!("{:.3e} m", w.max_distance_m)),
                format!(
                    "against the tolerance {:.3e} m{}. {}",
                    d.chord_tolerance_m,
                    worst.map_or(String::new(), |w| format!(", in {}", w.solid_id)),
                    d.statement
                ),
            )
        }),
        absent(report, 7),
    ),
    record(
        "Source sites inside the plasma chamber",
        g.sources.as_ref().map(|s| {
            (
                format!("{} of {} sites", s.inside, s.sites),
                format!(
                    "inside {}; {} outside, {} undecided",
                    s.plasma_chamber_solid, s.outside, s.undecided
                ),
            )
        }),
        absent(report, 9),
    ),
    record(
        "Clearance from the source sites to the first wall",
        g.sources.as_ref().map(|s| {
            match (s.first_wall_clearance_m, s.first_wall_clearance_at_m) {
                (Some(d), Some(at)) => (
                    format!("{d:.4} m"),
                    format!(
                        "the nearest site to {} is at ({:.3}, {:.3}, {:.3}) m; {} site(s) inside the wall",
                        s.first_wall_solids.join(", "),
                        at[0],
                        at[1],
                        at[2],
                        s.in_first_wall
                    ),
                ),
                _ => (
                    "not measured".into(),
                    "no solid has the role first_wall. Next step: give the first-wall solid that role in the design file.".into(),
                ),
            }
        }),
        absent(report, 9),
    ),
    record(
        "Source closure, fusion power",
        g.source_closure.as_ref().map(|c| {
            (
                if c.closes { "WITHIN" } else { "EXCEEDED" }.to_string(),
                c.summary(),
            )
        }),
        (
            "not computed".into(),
            "the plasma block could not be read, so the power its profiles imply was not computed. Next step: fix the findings of checks 1 to 5.".into(),
        ),
    ),
    ]
}

pub fn check_view(report: &ImportReport) -> CheckView {
    let verdict = if report.import_complete {
        Verdict::Complete
    } else if !report.evaluated_checks_pass {
        Verdict::Failed
    } else {
        Verdict::Incomplete
    };
    let next_step = match verdict {
        Verdict::Complete => {
            "Next step: run the flux for this design; it reads the conversion from the work folder."
        }
        Verdict::Failed => {
            "Next step: fix the findings below in your CAD tool or the design file, then check again. The checks stop at the first failing stage, so more may appear."
        }
        Verdict::Incomplete => {
            "Next step: a check below is NOT_EVALUATED; its why says which tool is missing. Set it and check again. The flux cannot run on an import that is not complete."
        }
    };
    CheckView {
        verdict,
        summary: report.summary.clone(),
        next_step: next_step.into(),
        design_line: format!(
            "Design {} ({}), sha256 {}",
            report.design.id.as_deref().unwrap_or("unreadable"),
            report.design.path,
            hash_head(&report.design.sha256)
        ),
        step_line: report.step.as_ref().map(|s| {
            format!(
                "STEP {}, sha256 {}{}",
                s.path,
                hash_head(&s.sha256),
                match s.unit_declared_by_file {
                    Some(false) => "; the file declares no length unit, the design file's was used",
                    _ => "",
                }
            )
        }),
        checks: check_rows(report),
        records: records(report),
        notices: report
            .notices
            .iter()
            .map(|n| format!("Check {}, {}: {}", n.check, n.item, n.message))
            .collect(),
        scope: report.scope.clone(),
    }
}

/// Whether the flux can be run for this check, and from which files. `Err` is the reason it
/// cannot, as the window says it.
pub fn flux_offer(
    verdict: Verdict,
    design: &Path,
    saved: &Result<PathBuf, String>,
) -> Result<(PathBuf, PathBuf), String> {
    match (verdict, saved) {
        (Verdict::Complete, Ok(report)) => Ok((design.to_path_buf(), report.clone())),
        (Verdict::Complete, Err(error)) => Err(format!(
            "The report file was not saved ({error}), and the flux run reads it. Next step: check again with a report path in a folder you can write to."
        )),
        (Verdict::Failed, _) => Err(
            "A check failed. Next step: fix the findings above and check again; the flux runs only on a design whose nine checks all pass.".into(),
        ),
        (Verdict::Incomplete, _) => Err(
            "A check was NOT_EVALUATED. Next step: set the tools it names and check again; the flux runs only on a design whose nine checks all pass.".into(),
        ),
    }
}

// ----------------------------------------------------------------- the draft --

/// Fields the draft leaves null, said in words.
#[derive(Clone, Debug, PartialEq)]
pub struct Unfilled {
    pub what: String,
    pub count: usize,
    pub paths: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InitView {
    pub summary: String,
    pub path: String,
    pub groups: Vec<Unfilled>,
    /// Why the unfilled fields could not be listed, when the file could not be read back.
    pub unreadable: Option<String>,
    /// What else the design file needs that is not a null in the draft.
    pub also: Vec<String>,
    pub edit_note: String,
}

/// The handbook page that describes the file.
pub const HANDBOOK_PAGE: &str = "The design file (bring your own CAD)";

const ALSO: [&str; 3] = [
    "Add the materials the solids use: a catalog material, or a recipe.",
    "Add a replacement group, with its limits, for each part you replace, and set replacement_group_id on its solids.",
    "Label every number you supply as published (with a reference) or authored.",
];

/// `solids[12].role` as `solids[].role`.
fn normalised(path: &str) -> String {
    let mut out = String::new();
    let mut in_index = false;
    for ch in path.chars() {
        match ch {
            '[' => {
                in_index = true;
                out.push('[');
            }
            ']' => {
                in_index = false;
                out.push(']');
            }
            _ if in_index => {}
            _ => out.push(ch),
        }
    }
    out
}

fn describe(key: &str) -> String {
    match key {
        "solids[].material_id" => "the material of each solid".into(),
        "solids[].role" => {
            "the role of each solid (plasma_chamber, first_wall, blanket, ...)".into()
        }
        k if k.starts_with("plasma.") => {
            "the plasma block: its shape, density and temperature profiles, fuel and fusion power"
                .into()
        }
        k if k.starts_with("operating_scenario.") => {
            "the link to the operating-history file: its path and sha256".into()
        }
        k => k.to_string(),
    }
}

/// The nulls of a draft, grouped the way a person fills them.
pub fn unfilled_groups(paths: &[String]) -> Vec<Unfilled> {
    let mut groups: Vec<Unfilled> = Vec::new();
    for path in paths {
        let what = describe(&normalised(path));
        match groups.iter_mut().find(|g| g.what == what) {
            Some(group) => {
                group.count += 1;
                group.paths.push(path.clone());
            }
            None => groups.push(Unfilled {
                what,
                count: 1,
                paths: vec![path.clone()],
            }),
        }
    }
    groups
}

pub fn init_view(done: &InitDone) -> InitView {
    let s = &done.summary;
    let (groups, unreadable) = match &done.null_paths {
        Ok(paths) => (unfilled_groups(paths), None),
        Err(why) => (Vec::new(), Some(why.clone())),
    };
    InitView {
        summary: format!(
            "Wrote a DRAFT design file with {} solids (STEP unit {}{}).",
            s.solids,
            s.length_unit.label(),
            if s.unit_declared_by_file {
                ", declared by the file"
            } else {
                ", NOT declared by the file; the unit you chose was used"
            }
        ),
        path: s.path.display().to_string(),
        groups,
        unreadable,
        also: ALSO.iter().map(|t| (*t).to_string()).collect(),
        edit_note: format!(
            "The design file is edited outside the app for now: open it in any text editor. The handbook page \"{HANDBOOK_PAGE}\" (docs/guide/design-file.md, step 2) shows every field. When you have filled it in, check it below."
        ),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use faris_engine::design::{
        CheckOutcome, DesignIdentity, GeometryRecords, InitSummary, StepIdentity,
    };
    use faris_model::design::{Finding, LengthUnit, Notice};

    const NAMES: [&str; 9] = [
        "The file parses",
        "STEP hash and unit",
        "Every solid is accounted for",
        "Ids and references",
        "Roles and materials",
        "Conversion",
        "Binding",
        "Integrity",
        "Source sites",
    ];

    fn outcome(check: u8, status: CheckStatus) -> CheckOutcome {
        CheckOutcome {
            check,
            name: NAMES[usize::from(check) - 1].into(),
            status,
            findings: Vec::new(),
            why: None,
            next_step: None,
            sub_checks: Vec::new(),
        }
    }

    /// Geometry records as the engine writes them; the numbers are arbitrary and say nothing
    /// about any design.
    fn geometry() -> GeometryRecords {
        serde_json::from_value(serde_json::json!({
            "conversion": {
                "reused": false, "directory": "/w/conv", "step_sha256": "ab",
                "shape_unit_m": 0.001, "chord_cm": 0.1, "angular_rad": 0.1,
                "scale_factor_to_cm": 0.1, "imprint_threads": 4,
                "implicit_complement_tag": "mat:void", "versions": {},
                "timing_seconds": {"total": 120.0}, "peak_rss_mb": 900.0,
                "h5m_sha256": "cd", "h5m_bytes": 1000, "facets_sha256": "ef",
                "counts": {}, "helper_sha256": "01",
                "volumes": [
                    {"volume": 1, "global_id": 1, "material_tag": "mat:coil-a", "triangles": 1200, "vertices": 600},
                    {"volume": 2, "global_id": 2, "material_tag": "mat:coil-b", "triangles": 800, "vertices": 400}
                ]
            },
            "binding": [],
            "volume_gate": {
                "chord_m": 0.001, "tolerance_source": "default", "limit": 0.001,
                "largest_relative_difference": 0.0004,
                "solids": [
                    {"solid_id": "coil-a", "relative_difference": 0.0004, "verdict": "WITHIN"},
                    {"solid_id": "coil-b", "relative_difference": 0.0001, "verdict": "WITHIN"}
                ]
            },
            "deviation": {
                "method": "closed form", "chord_tolerance_m": 0.001, "seconds": 3.0,
                "statement": "Maximum over all sample points.",
                "solids": [
                    {"solid_id": "coil-a", "max_distance_m": 0.0002, "at_m": null, "points": 100, "above_tolerance": false},
                    {"solid_id": "coil-b", "max_distance_m": 0.0003, "at_m": null, "points": 100, "above_tolerance": false}
                ]
            },
            "sources": {
                "sites": 100000, "seed": 1, "plasma_chamber_solid": "plasma",
                "inside": 100000, "outside": 0, "undecided": 0, "outside_in": {},
                "sites_sha256": "aa", "versions": {}, "sampling_seconds": 2.0, "test_seconds": 1.0,
                "first_wall_solids": ["first-wall"], "first_wall_clearance_m": 0.052,
                "first_wall_clearance_at_m": [6.0, 0.0, 0.5], "in_first_wall": 0
            },
            "source_closure": {
                "declared_fusion_power_mw": 500.0, "implied_fusion_power_mw": 500.2,
                "implied_reaction_rate_per_s": 1.77e20, "ratio": 1.0004,
                "relative_difference": 0.0004, "tolerance": 0.001, "closes": true
            },
            "waivers": [],
            "stages": []
        }))
        .unwrap()
    }

    fn base(summary: &str) -> ImportReport {
        ImportReport {
            schema_version: "faris-design-import-report/v0.2".into(),
            scope: "Stage S1b: checks 1 to 9.".into(),
            design: DesignIdentity {
                path: "/d/machine.faris-design.json".into(),
                sha256: "0123456789abcdef0123".into(),
                id: Some("machine".into()),
            },
            step: Some(StepIdentity {
                path: "machine.step".into(),
                sha256: "fedcba9876543210ff".into(),
                solids: Some(24),
                length_unit: "mm".into(),
                unit_declared_by_file: Some(true),
            }),
            cad_tool: None,
            material_audit: None,
            checks: Vec::new(),
            void_solids: Vec::new(),
            notices: Vec::new(),
            geometry: GeometryRecords::default(),
            evaluated_checks_pass: true,
            import_complete: false,
            summary: summary.into(),
        }
    }

    /// A hand-built report of a design that passed all nine checks.
    pub fn passing_report() -> ImportReport {
        let mut report =
            base("No check failed: all 9 checks evaluated, all PASS. The design imports.");
        report.checks = (1..=9).map(|n| outcome(n, CheckStatus::Pass)).collect();
        report.geometry = geometry();
        report.notices = vec![Notice::new(
            9,
            "plasma.fusion_power_mw",
            "the declared power closes with the profiles",
        )];
        report.import_complete = true;
        report
    }

    /// A hand-built report of a design that failed check 5; the later checks did not run.
    pub fn failing_report() -> ImportReport {
        let mut report =
            base("FAIL: check 5 found 1 blocking problem(s); later checks were not run.");
        for n in 1..=4 {
            report.checks.push(outcome(n, CheckStatus::Pass));
        }
        let mut five = outcome(5, CheckStatus::Fail);
        five.findings = vec![
            Finding::new(
                5,
                "solids[3] (blanket-1)",
                "its role is blanket but its material is void",
                "Give it a material that is not void.",
            )
            .cells(vec!["blanket-1".into()]),
        ];
        report.checks.push(five);
        for n in 6..=9 {
            let mut c = outcome(n, CheckStatus::NotEvaluated);
            c.why = Some("check 5 failed, and the checks stop at the first failing stage".into());
            c.next_step = Some("Fix the findings of check 5, then check again.".into());
            report.checks.push(c);
        }
        report.evaluated_checks_pass = false;
        report
    }

    /// No check failed, but the nuclear-data audit and the source test had no tool.
    fn incomplete_report() -> ImportReport {
        let mut report = passing_report();
        report.checks[4].status = CheckStatus::NotEvaluated;
        report.checks[4].why = Some("no OpenMC interpreter is configured".into());
        report.checks[4].next_step = Some("Set the OpenMC Python and check again.".into());
        report.import_complete = false;
        report.geometry.sources = None;
        report.summary = "No check failed, but check 5 could not be fully evaluated, so the import is not complete.".into();
        report
    }

    #[test]
    fn a_passing_check_shows_nine_pass_words_and_a_next_step() {
        let view = check_view(&passing_report());
        assert_eq!(view.verdict, Verdict::Complete);
        assert_eq!(view.verdict.word(), "PASS");
        assert_eq!(
            view.summary,
            "No check failed: all 9 checks evaluated, all PASS. The design imports."
        );
        assert_eq!(view.checks.len(), 9);
        assert!(view.checks.iter().all(|c| c.status_word == "PASS"));
        assert!(view.next_step.contains("run the flux"));
        assert_eq!(
            view.design_line,
            "Design machine (/d/machine.faris-design.json), sha256 0123456789abcdef"
        );
        assert_eq!(
            view.notices,
            ["Check 9, plasma.fusion_power_mw: the declared power closes with the profiles"]
        );
    }

    #[test]
    fn the_measured_records_carry_their_units() {
        let view = check_view(&passing_report());
        let get = |caption: &str| {
            view.records
                .iter()
                .find(|r| r.caption.starts_with(caption))
                .unwrap_or_else(|| panic!("no record {caption}"))
        };
        assert_eq!(get("Solids in the STEP file").value, "24 solids");
        assert_eq!(
            get("Conversion tolerance").value,
            "chord 0.1 cm, angular 0.1 rad"
        );
        assert_eq!(get("Triangles").value, "2000 triangles");
        assert_eq!(get("Largest volume difference").value, "0.0400 %");
        assert!(
            get("Largest volume difference")
                .note
                .contains("gate is 0.100 % per solid")
        );
        assert!(get("Largest volume difference").note.contains("in coil-a"));
        assert_eq!(get("Faceting deviation").value, "3.000e-4 m");
        assert!(
            get("Faceting deviation")
                .note
                .contains("tolerance 1.000e-3 m")
        );
        assert_eq!(get("Source sites").value, "100000 of 100000 sites");
        assert_eq!(get("Clearance").value, "0.0520 m");
        assert!(get("Clearance").note.contains("0 site(s) inside the wall"));
        let closure = get("Source closure");
        assert_eq!(closure.value, "WITHIN");
        assert!(closure.note.contains("500.20 MW") && closure.note.contains("500.00 MW"));
    }

    #[test]
    fn a_failing_check_shows_the_engines_words_for_the_finding() {
        let view = check_view(&failing_report());
        assert_eq!(view.verdict, Verdict::Failed);
        assert_eq!(view.verdict.word(), "FAIL");
        let words: Vec<_> = view.checks.iter().map(|c| c.status_word).collect();
        assert_eq!(
            words,
            [
                "PASS",
                "PASS",
                "PASS",
                "PASS",
                "FAIL",
                "NOT_EVALUATED",
                "NOT_EVALUATED",
                "NOT_EVALUATED",
                "NOT_EVALUATED"
            ]
        );
        let finding = &view.checks[4].findings[0];
        assert_eq!(finding.item, "solids[3] (blanket-1)");
        assert_eq!(finding.why, "its role is blanket but its material is void");
        assert_eq!(finding.next_step, "Give it a material that is not void.");
        assert_eq!(finding.solids.as_deref(), Some("blanket-1"));
        let later = &view.checks[5];
        assert!(
            later
                .why
                .as_deref()
                .unwrap()
                .contains("stop at the first failing stage")
        );
        assert_eq!(
            later.next_step.as_deref(),
            Some("Fix the findings of check 5, then check again.")
        );
        assert!(view.next_step.contains("fix the findings"));
    }

    #[test]
    fn a_record_the_engine_did_not_measure_says_why_it_is_absent() {
        let view = check_view(&failing_report());
        let conversion = view
            .records
            .iter()
            .find(|r| r.caption == "Conversion tolerance")
            .unwrap();
        assert_eq!(conversion.value, "not measured");
        assert_eq!(
            conversion.note,
            "check 6 is NOT_EVALUATED, so this was not measured. Next step: see check 6 above."
        );
        let closure = view.records.last().unwrap();
        assert_eq!(closure.value, "not computed");
        assert!(closure.note.contains("Next step"));
        // No number appears for what was not measured.
        assert!(
            view.records
                .iter()
                .skip(1)
                .all(|r| r.value == "not measured" || r.value == "not computed")
        );
    }

    #[test]
    fn a_check_with_a_missing_tool_is_incomplete_not_passing() {
        let view = check_view(&incomplete_report());
        assert_eq!(view.verdict, Verdict::Incomplete);
        assert_eq!(view.verdict.word(), "NOT_EVALUATED");
        assert_eq!(view.checks[4].status_word, "NOT_EVALUATED");
        assert!(view.next_step.contains("flux cannot run"));
        let sites = view
            .records
            .iter()
            .find(|r| r.caption.starts_with("Source sites"))
            .unwrap();
        assert_eq!(sites.value, "not measured");
    }

    #[test]
    fn a_waived_finding_stays_visible_and_says_who_waived_it() {
        let mut report = passing_report();
        let mut finding = Finding::new(8, "coil-a: sliver", "thin", "thicken it");
        finding.waived = Some(faris_model::design::WaiverRecord {
            index: 0,
            check_id: "SLIVER".into(),
            author: "A. Designer".into(),
            date: "2026-10-10".into(),
            reason: "on purpose".into(),
        });
        report.checks[7].findings.push(finding);
        let view = check_view(&report);
        let row = &view.checks[7].findings[0];
        assert_eq!(
            row.waived.as_deref(),
            Some("WAIVED by A. Designer on 2026-10-10: on purpose")
        );
        assert_eq!(view.checks[7].status_word, "PASS");
    }

    #[test]
    fn a_first_wall_clearance_that_was_not_measured_says_what_to_do() {
        let mut report = passing_report();
        report
            .geometry
            .sources
            .as_mut()
            .unwrap()
            .first_wall_clearance_m = None;
        let view = check_view(&report);
        let clearance = view
            .records
            .iter()
            .find(|r| r.caption.starts_with("Clearance"))
            .unwrap();
        assert_eq!(clearance.value, "not measured");
        assert!(clearance.note.contains("role first_wall"));
    }

    #[test]
    fn the_flux_is_offered_only_for_a_complete_import_with_a_saved_report() {
        let design = Path::new("/d/machine.faris-design.json");
        let saved = Ok(PathBuf::from("/d/machine-check-report.json"));
        assert_eq!(
            flux_offer(Verdict::Complete, design, &saved),
            Ok((
                design.to_path_buf(),
                PathBuf::from("/d/machine-check-report.json")
            ))
        );
        let unsaved = Err("disk full".to_string());
        let reason = flux_offer(Verdict::Complete, design, &unsaved).unwrap_err();
        assert!(reason.contains("not saved (disk full)") && reason.contains("Next step"));
        for verdict in [Verdict::Failed, Verdict::Incomplete] {
            let reason = flux_offer(verdict, design, &saved).unwrap_err();
            assert!(reason.contains("Next step"), "{reason}");
        }
    }

    // ----------------------------------------------------------------- init --

    fn init_done(paths: Result<Vec<String>, String>, declared: bool) -> InitDone {
        InitDone {
            summary: InitSummary {
                path: PathBuf::from("/d/machine.faris-design.json"),
                solids: 24,
                length_unit: LengthUnit::Mm,
                unit_declared_by_file: declared,
                step_sha256: "0".into(),
                nulls_to_fill: 0,
            },
            null_paths: paths,
        }
    }

    fn sample_nulls() -> Vec<String> {
        let mut paths = Vec::new();
        for i in 0..24 {
            paths.push(format!("solids[{i}].material_id"));
            paths.push(format!("solids[{i}].role"));
        }
        for field in [
            "plasma.chamber_solid_id",
            "plasma.major_radius_m",
            "plasma.ion_density_m3.centre",
        ] {
            paths.push(field.into());
        }
        paths.push("operating_scenario.path".into());
        paths.push("operating_scenario.sha256".into());
        paths
    }

    #[test]
    fn the_unfilled_fields_are_grouped_in_plain_words_with_counts() {
        let groups = unfilled_groups(&sample_nulls());
        let line = |g: &Unfilled| format!("{} x{}", g.what, g.count);
        assert_eq!(
            groups.iter().map(line).collect::<Vec<_>>(),
            [
                "the material of each solid x24",
                "the role of each solid (plasma_chamber, first_wall, blanket, ...) x24",
                "the plasma block: its shape, density and temperature profiles, fuel and fusion power x3",
                "the link to the operating-history file: its path and sha256 x2",
            ]
        );
        assert_eq!(groups[0].paths[1], "solids[1].material_id");
        assert_eq!(
            unfilled_groups(&["mystery[2].x".into()])[0].what,
            "mystery[].x"
        );
    }

    #[test]
    fn the_draft_view_names_the_file_says_it_is_edited_outside_and_names_the_handbook() {
        let view = init_view(&init_done(Ok(sample_nulls()), true));
        assert_eq!(
            view.summary,
            "Wrote a DRAFT design file with 24 solids (STEP unit mm, declared by the file)."
        );
        assert_eq!(view.path, "/d/machine.faris-design.json");
        assert_eq!(view.groups.len(), 4);
        assert!(view.edit_note.contains("edited outside the app"));
        assert!(
            view.edit_note
                .contains("The design file (bring your own CAD)")
        );
        assert_eq!(view.also.len(), 3);
        assert!(view.unreadable.is_none());
        let assumed = init_view(&init_done(Ok(Vec::new()), false));
        assert!(
            assumed
                .summary
                .contains("NOT declared by the file; the unit you chose was used")
        );
    }

    #[test]
    fn a_draft_that_cannot_be_read_back_says_so_without_inventing_a_list() {
        let view = init_view(&init_done(Err("cannot read x".into()), true));
        assert!(view.groups.is_empty());
        assert_eq!(view.unreadable.as_deref(), Some("cannot read x"));
    }
}
