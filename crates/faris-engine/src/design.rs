//! Design-file import checks 1 to 5 and the first-draft writer (`design init`).
//!
//! The model crate holds the format and the pure checks. This module runs the
//! two external helpers as bounded jobs (the CAD helper that reads the STEP file,
//! and the OpenMC material audit), runs the checks in the order of the format
//! document, stops before a later stage when an earlier one fails, and returns
//! a report. Checks 6 to 9 (conversion, DAGMC binding, integrity, source sites)
//! belong to stage S1b and are listed as NOT_EVALUATED. Nothing is repaired.

use crate::jobs::{Cancellation, ExecutionStatus, JobSpec, ResourceLimits, run_job};
use faris_model::design::{
    self, CENTROID_TOLERANCE, Cad, CatalogMaterial, Component, DESIGN_VERSION, Design,
    DesignMaterial, Extent, Finding, Fingerprint, LengthUnit, Notice, OperatingScenarioLink,
    Plasma, Profile, ScenarioFacts, Solid, StepInspection, TemperatureProfile, VOLUME_TOLERANCE,
    VoidSolid,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

pub const IMPORT_REPORT_VERSION: &str = "faris-design-import-report/v0.1";
const CAD_HELPER: &str = include_str!("../../../integrations/cad/faris_cad.py");
const MATERIAL_AUDIT: &str = include_str!("../../../integrations/openmc/material_audit.py");
const AUDIT_LIBRARY: &str = include_str!("../../../integrations/openmc/audit_library.py");
const MAX_JSON_BYTES: u64 = 64 * 1024 * 1024;
const CAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const AUDIT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, thiserror::Error)]
pub enum DesignError {
    /// A file could not be read or written.
    #[error("{0}")]
    Io(String),
    /// A helper could not run or gave an unusable answer. The design was not judged.
    #[error("{0}")]
    Tool(String),
    /// A draft could not be written because the STEP file or request is unusable.
    #[error("{0}")]
    Init(String),
}

fn io_error(path: &Path, error: impl std::fmt::Display) -> DesignError {
    DesignError::Io(format!("{}: {error}", path.display()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sha256_file(path: &Path) -> Result<String, DesignError> {
    let mut file = std::fs::File::open(path).map_err(|e| io_error(path, e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buffer).map_err(|e| io_error(path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

// ---------------------------------------------------------------------------
// Tools.

/// Where the external helpers live. Nothing is searched for or installed.
#[derive(Clone, Debug, Default)]
pub struct ToolPaths {
    /// Python with cadquery (the CAD environment); runs `faris_cad.py`.
    pub cad_python: Option<PathBuf>,
    /// Python with OpenMC and h5py; runs `material_audit.py`.
    pub openmc_python: Option<PathBuf>,
    /// `cross_sections.xml` of the audited library.
    pub cross_sections: Option<PathBuf>,
}

impl ToolPaths {
    /// Explicit values win; then `FARIS_CAD_PYTHON`, `FARIS_OPENMC_PYTHON`, `FARIS_CROSS_SECTIONS`.
    pub fn from_flags_and_environment(
        cad_python: Option<PathBuf>,
        openmc_python: Option<PathBuf>,
        cross_sections: Option<PathBuf>,
    ) -> Self {
        let from_env = |name: &str| std::env::var_os(name).map(PathBuf::from);
        Self {
            cad_python: cad_python.or_else(|| from_env("FARIS_CAD_PYTHON")),
            openmc_python: openmc_python.or_else(|| from_env("FARIS_OPENMC_PYTHON")),
            cross_sections: cross_sections.or_else(|| from_env("FARIS_CROSS_SECTIONS")),
        }
    }
}

/// What the CAD helper returned, with the identity of what ran.
#[derive(Clone, Debug)]
pub struct InspectOutcome {
    pub inspection: StepInspection,
    pub helper_sha256: String,
    pub elapsed_seconds: f64,
}

/// A recipe or ready nuclide vector sent to the material audit.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuditMaterial {
    Nuclides {
        id: String,
        atom_fractions: BTreeMap<String, f64>,
    },
    Recipe {
        id: String,
        basis: String,
        components: Vec<Component>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct AuditedMaterial {
    pub id: String,
    pub nuclides: Option<BTreeMap<String, f64>>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct AuditedNuclide {
    pub present: bool,
    pub readable: bool,
    #[serde(default)]
    pub temperatures_k: Vec<f64>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct MaterialAuditReport {
    pub schema: String,
    pub openmc: String,
    #[serde(default)]
    pub cross_sections_sha256: Option<String>,
    pub materials: Vec<AuditedMaterial>,
    pub nuclides: BTreeMap<String, AuditedNuclide>,
    #[serde(default)]
    pub photon_elements: BTreeMap<String, serde_json::Value>,
}

pub enum AuditOutcome {
    Evaluated {
        report: MaterialAuditReport,
        elapsed_seconds: f64,
    },
    NotEvaluated {
        why: String,
        next_step: String,
    },
}

/// The two external steps, behind a trait so tests can supply recorded answers.
pub trait Backends {
    /// Read the STEP file. `assume` is the design's `cad.length_unit`, used only
    /// when the file declares none.
    fn inspect_step(
        &self,
        step: &Path,
        assume: Option<LengthUnit>,
    ) -> Result<InspectOutcome, DesignError>;
    fn audit_materials(&self, materials: &[AuditMaterial]) -> Result<AuditOutcome, DesignError>;
}

pub struct ExternalBackends<'a> {
    pub tools: &'a ToolPaths,
    pub cancellation: &'a Cancellation,
}

fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().rev().take(12).collect();
    lines.into_iter().rev().collect::<Vec<_>>().join("\n")
}

fn read_json_bounded(path: &Path) -> Result<Vec<u8>, DesignError> {
    let size = std::fs::metadata(path)
        .map_err(|e| io_error(path, e))?
        .len();
    if size > MAX_JSON_BYTES {
        return Err(DesignError::Tool(format!(
            "{} is {size} bytes, over the {MAX_JSON_BYTES}-byte bound",
            path.display()
        )));
    }
    std::fs::read(path).map_err(|e| io_error(path, e))
}

fn run_python(
    python: &Path,
    arguments: Vec<OsString>,
    scratch: &Path,
    timeout: Duration,
    what: &str,
    cancellation: &Cancellation,
) -> Result<f64, DesignError> {
    // Not canonicalised: a virtual environment's python is a symlink, and the
    // interpreter finds its packages from the path it was started by.
    let python = std::path::absolute(python)
        .ok()
        .filter(|p| p.is_file())
        .ok_or_else(|| {
            DesignError::Tool(format!(
                "{what}: the interpreter {} cannot be used (not a file); give a Python that has \
                 the required packages",
                python.display()
            ))
        })?;
    let spec = JobSpec {
        program: python,
        arguments,
        working_directory: scratch.to_path_buf(),
        environment: vec![("PYTHONDONTWRITEBYTECODE".into(), "1".into())],
        timeout,
        capture_limit_bytes: 1 << 20,
        artifact_roots: Vec::new(),
        resource_limits: ResourceLimits::default(),
    };
    let result =
        run_job(&spec, cancellation).map_err(|e| DesignError::Tool(format!("{what}: {e}")))?;
    match result.execution_status {
        ExecutionStatus::Succeeded => Ok(result.elapsed_seconds),
        ExecutionStatus::Cancelled => Err(DesignError::Tool(format!("{what}: cancelled"))),
        ExecutionStatus::TimedOut => Err(DesignError::Tool(format!(
            "{what} did not finish within {} s and was stopped",
            timeout.as_secs()
        ))),
        other => Err(DesignError::Tool(format!(
            "{what} failed ({other:?}, exit {:?}); the job runner limits memory to {} MiB: {}",
            result.exit_code,
            result.resource_limits.address_space_bytes >> 20,
            tail(&format!("{}\n{}", result.stdout, result.stderr))
        ))),
    }
}

impl Backends for ExternalBackends<'_> {
    fn inspect_step(
        &self,
        step: &Path,
        assume: Option<LengthUnit>,
    ) -> Result<InspectOutcome, DesignError> {
        let python = self.tools.cad_python.as_deref().ok_or_else(|| {
            DesignError::Tool(
                "no CAD interpreter: pass --cad-python PATH or set FARIS_CAD_PYTHON to a Python \
                 with cadquery installed. FARIS does not install it"
                    .into(),
            )
        })?;
        let step = step.canonicalize().map_err(|e| io_error(step, e))?;
        let scratch = tempfile::tempdir().map_err(|e| DesignError::Io(e.to_string()))?;
        let script = scratch.path().join("faris_cad.py");
        std::fs::write(&script, CAD_HELPER).map_err(|e| io_error(&script, e))?;
        let out = scratch.path().join("inspect.json");
        let mut arguments: Vec<OsString> = vec![
            script.into_os_string(),
            "step-inspect".into(),
            step.into_os_string(),
            "--out".into(),
            out.clone().into_os_string(),
        ];
        if let Some(unit) = assume {
            arguments.push("--assume-unit".into());
            arguments.push(unit.label().into());
        }
        let elapsed_seconds = run_python(
            python,
            arguments,
            scratch.path(),
            CAD_TIMEOUT,
            "the CAD helper",
            self.cancellation,
        )?;
        let inspection =
            StepInspection::from_bytes(&read_json_bounded(&out)?).map_err(DesignError::Tool)?;
        Ok(InspectOutcome {
            inspection,
            helper_sha256: sha256_hex(CAD_HELPER.as_bytes()),
            elapsed_seconds,
        })
    }

    fn audit_materials(&self, materials: &[AuditMaterial]) -> Result<AuditOutcome, DesignError> {
        let (Some(python), Some(cross_sections)) =
            (&self.tools.openmc_python, &self.tools.cross_sections)
        else {
            let missing = match (&self.tools.openmc_python, &self.tools.cross_sections) {
                (None, None) => "no OpenMC interpreter and no cross_sections.xml are configured",
                (None, _) => "no OpenMC interpreter is configured",
                _ => "no cross_sections.xml is configured",
            };
            return Ok(AuditOutcome::NotEvaluated {
                why: format!(
                    "{missing}, so the material nuclides cannot be checked against the library"
                ),
                next_step:
                    "Pass --openmc-python and --cross-sections (or set FARIS_OPENMC_PYTHON and \
                            FARIS_CROSS_SECTIONS) and check again."
                        .into(),
            });
        };
        let cross_sections = cross_sections.canonicalize().map_err(|e| {
            DesignError::Tool(format!(
                "cross_sections.xml {}: {e}",
                cross_sections.display()
            ))
        })?;
        let scratch = tempfile::tempdir().map_err(|e| DesignError::Io(e.to_string()))?;
        for (name, text) in [
            ("material_audit.py", MATERIAL_AUDIT),
            ("audit_library.py", AUDIT_LIBRARY),
        ] {
            let path = scratch.path().join(name);
            std::fs::write(&path, text).map_err(|e| io_error(&path, e))?;
        }
        let request = scratch.path().join("request.json");
        let body = serde_json::to_vec(&serde_json::json!({ "materials": materials }))
            .map_err(|e| DesignError::Tool(e.to_string()))?;
        std::fs::write(&request, body).map_err(|e| io_error(&request, e))?;
        let out = scratch.path().join("audit.json");
        let arguments: Vec<OsString> = vec![
            scratch.path().join("material_audit.py").into_os_string(),
            "--request".into(),
            request.into_os_string(),
            "--cross-sections".into(),
            cross_sections.into_os_string(),
            "--out".into(),
            out.clone().into_os_string(),
        ];
        let elapsed = match run_python(
            python,
            arguments,
            scratch.path(),
            AUDIT_TIMEOUT,
            "the material audit",
            self.cancellation,
        ) {
            Ok(seconds) => seconds,
            Err(DesignError::Tool(text)) if !text.ends_with("cancelled") => {
                return Ok(AuditOutcome::NotEvaluated {
                    why: format!("the audit could not run: {text}"),
                    next_step: "Check that --openmc-python has openmc and h5py, and that \
                                cross_sections.xml is the audited library, then check again."
                        .into(),
                });
            }
            Err(error) => return Err(error),
        };
        let report: MaterialAuditReport = serde_json::from_slice(&read_json_bounded(&out)?)
            .map_err(|e| {
                DesignError::Tool(format!(
                    "the material audit's report could not be read: {e}"
                ))
            })?;
        Ok(AuditOutcome::Evaluated {
            report,
            elapsed_seconds: elapsed,
        })
    }
}

// ---------------------------------------------------------------------------
// The report.

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CheckStatus {
    Pass,
    Fail,
    NotEvaluated,
}

impl CheckStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::NotEvaluated => "NOT_EVALUATED",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SubCheck {
    pub name: String,
    pub status: CheckStatus,
    pub why: Option<String>,
    pub next_step: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CheckOutcome {
    pub check: u8,
    pub name: String,
    pub status: CheckStatus,
    pub findings: Vec<Finding>,
    /// Why the check was not evaluated, or the one-line meaning of a PASS.
    pub why: Option<String>,
    pub next_step: Option<String>,
    pub sub_checks: Vec<SubCheck>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DesignIdentity {
    pub path: String,
    pub sha256: String,
    pub id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct StepIdentity {
    pub path: String,
    pub sha256: String,
    pub solids: Option<usize>,
    /// The unit the design file gives and whether the STEP file declared it.
    pub length_unit: String,
    pub unit_declared_by_file: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CadToolRecord {
    pub interpreter: String,
    pub python: String,
    pub cadquery: Option<String>,
    pub ocp: Option<String>,
    pub occt: Option<String>,
    pub helper_version: String,
    pub helper_sha256: String,
    pub volume_method: String,
    pub elapsed_seconds: f64,
    /// The helper's own phase times (read and transfer, measuring the solids).
    pub timing_seconds: BTreeMap<String, f64>,
    pub tolerances: Tolerances,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Tolerances {
    pub volume_relative: f64,
    pub centroid_fraction_of_model_diagonal: f64,
    pub model_bbox_diagonal_m: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct AuditRecord {
    pub status: CheckStatus,
    pub openmc: Option<String>,
    pub cross_sections_sha256: Option<String>,
    pub elapsed_seconds: Option<f64>,
    pub materials: Vec<AuditedMaterial>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ImportReport {
    pub schema_version: String,
    pub scope: String,
    pub design: DesignIdentity,
    pub step: Option<StepIdentity>,
    pub cad_tool: Option<CadToolRecord>,
    pub material_audit: Option<AuditRecord>,
    pub checks: Vec<CheckOutcome>,
    /// Every void solid (GEO-039, NUC-056).
    pub void_solids: Vec<VoidSolid>,
    pub notices: Vec<Notice>,
    /// No check among those evaluated failed.
    pub evaluated_checks_pass: bool,
    /// Always false in stage S1a: conversion and the later checks do not exist yet.
    pub import_complete: bool,
    pub summary: String,
}

const SCOPE: &str = "Stage S1a: checks 1 to 5 of 9. Checks 6 to 9 (conversion, DAGMC binding, \
                     integrity, source sites) are stage S1b and are not evaluated.";

fn check_name(check: u8) -> &'static str {
    match check {
        1 => "The file parses, matches the format, has no unknown keys and no nulls",
        2 => "STEP hash, length unit, full extent, machine axis, operating scenario",
        3 => "STEP solid count and per-solid fingerprints",
        4 => "Ids are unique and valid; every reference resolves",
        5 => "Role and material rules; nuclear-data audit",
        6 => "Conversion to DAGMC as a bounded job",
        7 => "Each DAGMC volume matches one solid by fingerprint; faceted volume",
        8 => "Integrity: watertightness, overlaps, gaps, lost particles",
        9 => "Source sites all lie in the plasma chamber",
        _ => "",
    }
}

fn outcome(check: u8, findings: Vec<Finding>) -> CheckOutcome {
    CheckOutcome {
        check,
        name: check_name(check).into(),
        status: if findings.is_empty() {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        findings,
        why: None,
        next_step: None,
        sub_checks: Vec::new(),
    }
}

fn not_evaluated(check: u8, why: String, next_step: String) -> CheckOutcome {
    CheckOutcome {
        check,
        name: check_name(check).into(),
        status: CheckStatus::NotEvaluated,
        findings: Vec::new(),
        why: Some(why),
        next_step: Some(next_step),
        sub_checks: Vec::new(),
    }
}

fn stopped(check: u8, failed: u8) -> CheckOutcome {
    not_evaluated(
        check,
        format!("check {failed} failed, and the checks stop at the first failing stage"),
        format!("Fix the findings of check {failed}, then check again."),
    )
}

fn later_stage(check: u8) -> CheckOutcome {
    not_evaluated(
        check,
        "implemented in stage S1b".into(),
        "Nothing to do yet: this check runs once stage S1b is released. Until then FARIS checks \
         the design file and the STEP fingerprints but does not convert or run it."
            .into(),
    )
}

fn resolve(base: &Path, relative: &str) -> PathBuf {
    base.parent().unwrap_or(Path::new(".")).join(relative)
}

fn read_scenario(path: &Path) -> Result<ScenarioFacts, String> {
    let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > 16 * 1024 * 1024 {
        return Err("not a regular file under 16 MiB".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    Ok(design::scenario_facts(&bytes))
}

/// Expand each material to what the audit needs. Catalog materials are expanded
/// here from the baseline; recipes are expanded by OpenMC in the audit job.
fn audit_requests(design: &Design) -> Vec<AuditMaterial> {
    design
        .materials
        .iter()
        .filter_map(|material| {
            if let Some(catalog_id) = &material.catalog_id {
                let entry: &CatalogMaterial = design::catalog_material(catalog_id)?;
                Some(AuditMaterial::Nuclides {
                    id: material.id.clone(),
                    atom_fractions: entry.nuclide_atom_fractions.clone(),
                })
            } else {
                Some(AuditMaterial::Recipe {
                    id: material.id.clone(),
                    basis: match material.composition_basis? {
                        design::CompositionBasis::Atom => "atom".into(),
                        design::CompositionBasis::Weight => "weight".into(),
                    },
                    components: material.components.clone()?,
                })
            }
        })
        .collect()
}

fn audit_findings(design: &Design, report: &MaterialAuditReport) -> Vec<Finding> {
    const CHECK: u8 = 5;
    let mut findings = Vec::new();
    for audited in &report.materials {
        let index = design.materials.iter().position(|m| m.id == audited.id);
        let item = index.map_or_else(
            || audited.id.clone(),
            |i| format!("materials[{i}] ({})", audited.id),
        );
        if let Some(error) = &audited.error {
            findings.push(Finding::new(
                CHECK,
                format!("{item}.components"),
                format!("OpenMC could not expand the composition: {error}"),
                "Correct the element or nuclide names and fractions.",
            ));
            continue;
        }
        for nuclide in audited.nuclides.iter().flatten().map(|(n, _)| n) {
            match report.nuclides.get(nuclide) {
                Some(info) if info.present && info.readable && info.error.is_none() => {}
                Some(info) => findings.push(Finding::new(
                    CHECK,
                    format!("{item}: {nuclide}"),
                    format!(
                        "{nuclide} is {} in the cross-section library{}; a missing nuclide stops the run",
                        if info.present { "present but not usable" } else { "missing" },
                        info.error.as_ref().map(|e| format!(" ({e})")).unwrap_or_default()
                    ),
                    "Use a material built from nuclides in the audited library, or add the \
                     data to the library and audit it.",
                )),
                None => findings.push(Finding::new(
                    CHECK,
                    format!("{item}: {nuclide}"),
                    format!("the audit returned nothing for {nuclide}"),
                    "Check again; if it repeats, the material audit is faulty.",
                )),
            }
        }
    }
    findings
}

#[derive(Default)]
struct Stage {
    findings: Vec<Finding>,
    notices: Vec<Notice>,
}

/// Run checks 1 to 5 on a design file.
pub fn check_design(
    design_path: &Path,
    backends: &dyn Backends,
) -> Result<ImportReport, DesignError> {
    let bytes = std::fs::read(design_path).map_err(|e| io_error(design_path, e))?;
    let mut report = ImportReport {
        schema_version: IMPORT_REPORT_VERSION.into(),
        scope: SCOPE.into(),
        design: DesignIdentity {
            path: design_path.display().to_string(),
            sha256: sha256_hex(&bytes),
            id: None,
        },
        step: None,
        cad_tool: None,
        material_audit: None,
        checks: Vec::new(),
        void_solids: Vec::new(),
        notices: Vec::new(),
        evaluated_checks_pass: true,
        import_complete: false,
        summary: String::new(),
    };
    let result = run_stages(design_path, &bytes, backends, &mut report);
    // Whatever happened, the nine checks are all listed.
    let failed_at = report
        .checks
        .iter()
        .find(|c| c.status == CheckStatus::Fail)
        .map(|c| c.check);
    while report.checks.len() < 9 {
        let next = report.checks.len() as u8 + 1;
        report.checks.push(match (next, failed_at) {
            (6..=9, _) => later_stage(next),
            (_, Some(failed)) => stopped(next, failed),
            _ => later_stage(next),
        });
    }
    result?;
    report.evaluated_checks_pass = failed_at.is_none();
    let evaluated = report
        .checks
        .iter()
        .filter(|c| c.status != CheckStatus::NotEvaluated)
        .count();
    report.summary = match failed_at {
        Some(failed) => format!(
            "FAIL: check {failed} found {} problem(s); later checks were not run.",
            report.checks[usize::from(failed) - 1].findings.len()
        ),
        None => {
            let unevaluated: Vec<String> = report
                .checks
                .iter()
                .take(5)
                .filter(|c| c.status == CheckStatus::NotEvaluated)
                .map(|c| c.check.to_string())
                .collect();
            if unevaluated.is_empty() {
                format!(
                    "No check failed: {evaluated} of 9 evaluated, all PASS. The import is not \
                     complete; checks 6 to 9 are stage S1b."
                )
            } else {
                format!(
                    "No check failed, but check {} could not be fully evaluated. The import is \
                     not complete; checks 6 to 9 are stage S1b.",
                    unevaluated.join(", ")
                )
            }
        }
    };
    Ok(report)
}

fn run_stages(
    design_path: &Path,
    bytes: &[u8],
    backends: &dyn Backends,
    report: &mut ImportReport,
) -> Result<(), DesignError> {
    // Check 1.
    let parsed = design::parse_design(bytes);
    report.checks.push(outcome(1, parsed.findings));
    let Some(design) = parsed.design else {
        return Ok(());
    };
    report.design.id = Some(design.id.clone());
    if report.checks[0].status == CheckStatus::Fail {
        return Ok(());
    }

    // Check 2.
    let step_path = resolve(design_path, &design.cad.step_file);
    let mut stage = Stage::default();
    let step_hash = match sha256_file(&step_path) {
        Ok(hash) => Some(hash),
        Err(error) => {
            stage.findings.push(Finding::new(
                2,
                "cad.step_file",
                format!("the STEP file cannot be read: {error}"),
                "Put the STEP file at that path (relative to the design file) or correct step_file.",
            ));
            None
        }
    };
    let scenario = design
        .operating_scenario
        .path
        .as_deref()
        .map(|path| read_scenario(&resolve(design_path, path)));
    let scenario_ref = match &scenario {
        Some(Ok(facts)) => Ok(facts),
        Some(Err(error)) => Err(error.as_str()),
        None => Err("no path"),
    };
    if let Some(hash) = &step_hash {
        stage
            .findings
            .extend(design::check_2_static(&design, hash, scenario_ref));
    } else {
        // Extent, axis and scenario do not need the STEP file.
        let probe = "0".repeat(64);
        stage.findings.extend(
            design::check_2_static(&design, &probe, scenario_ref)
                .into_iter()
                .filter(|f| f.item != "cad.step_sha256"),
        );
    }
    let mut inspection: Option<StepInspection> = None;
    let hash_matches = step_hash
        .as_deref()
        .is_some_and(|h| h.eq_ignore_ascii_case(&design.cad.step_sha256));
    if let (Some(hash), true) = (&step_hash, hash_matches) {
        let ran = backends.inspect_step(&step_path, Some(design.cad.length_unit))?;
        let (findings, notices) = design::check_2_unit(&design, hash, &ran.inspection);
        stage.findings.extend(findings);
        stage.notices.extend(notices);
        let i = &ran.inspection;
        report.cad_tool = Some(CadToolRecord {
            interpreter: i.versions.interpreter.clone(),
            python: i.versions.python.clone(),
            cadquery: i.versions.cadquery.clone(),
            ocp: i.versions.ocp.clone(),
            occt: i.versions.occt.clone(),
            helper_version: i.versions.helper.clone(),
            helper_sha256: ran.helper_sha256.clone(),
            volume_method: i.volume_method.clone(),
            elapsed_seconds: ran.elapsed_seconds,
            timing_seconds: i.timing_seconds.clone(),
            tolerances: Tolerances {
                volume_relative: VOLUME_TOLERANCE,
                centroid_fraction_of_model_diagonal: CENTROID_TOLERANCE,
                model_bbox_diagonal_m: i.model_bbox_diagonal_m,
            },
        });
        report.step = Some(StepIdentity {
            path: step_path.display().to_string(),
            sha256: hash.clone(),
            solids: Some(i.solids.len()),
            length_unit: design.cad.length_unit.label().into(),
            unit_declared_by_file: Some(i.unit.declared.is_some()),
        });
        inspection = Some(ran.inspection);
    } else {
        report.step = Some(StepIdentity {
            path: step_path.display().to_string(),
            sha256: step_hash.clone().unwrap_or_default(),
            solids: None,
            length_unit: design.cad.length_unit.label().into(),
            unit_declared_by_file: None,
        });
    }
    report.notices.extend(stage.notices);
    let failed = !stage.findings.is_empty();
    report.checks.push(outcome(2, stage.findings));
    let Some(inspection) = inspection.filter(|_| !failed) else {
        return Ok(());
    };

    // Check 3.
    let (findings, notices) = design::check_3(&design, &inspection);
    report.notices.extend(notices);
    report.checks.push(outcome(3, findings));
    if report.checks[2].status == CheckStatus::Fail {
        return Ok(());
    }

    // Check 4.
    report.checks.push(outcome(4, design::check_4(&design)));
    if report.checks[3].status == CheckStatus::Fail {
        return Ok(());
    }

    // Check 5.
    let (mut findings, notices) = design::check_5(&design);
    report.notices.extend(notices);
    report.void_solids = design::void_solids(&design);
    let rules_ok = findings.is_empty();
    let mut sub_checks = vec![SubCheck {
        name: "role and material rules".into(),
        status: if rules_ok {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        why: None,
        next_step: None,
    }];
    let mut audit_status = CheckStatus::NotEvaluated;
    if rules_ok && !design.materials.is_empty() {
        match backends.audit_materials(&audit_requests(&design))? {
            AuditOutcome::Evaluated {
                report: audit,
                elapsed_seconds,
            } => {
                let audit_findings = audit_findings(&design, &audit);
                audit_status = if audit_findings.is_empty() {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Fail
                };
                sub_checks.push(SubCheck {
                    name: "nuclear-data audit".into(),
                    status: audit_status,
                    why: None,
                    next_step: None,
                });
                report.material_audit = Some(AuditRecord {
                    status: audit_status,
                    openmc: Some(audit.openmc.clone()),
                    cross_sections_sha256: audit.cross_sections_sha256.clone(),
                    elapsed_seconds: Some(elapsed_seconds),
                    materials: audit.materials.clone(),
                });
                findings.extend(audit_findings);
            }
            AuditOutcome::NotEvaluated { why, next_step } => {
                sub_checks.push(SubCheck {
                    name: "nuclear-data audit".into(),
                    status: CheckStatus::NotEvaluated,
                    why: Some(why.clone()),
                    next_step: Some(next_step.clone()),
                });
                report.material_audit = Some(AuditRecord {
                    status: CheckStatus::NotEvaluated,
                    openmc: None,
                    cross_sections_sha256: None,
                    elapsed_seconds: None,
                    materials: Vec::new(),
                });
            }
        }
    } else if rules_ok {
        audit_status = CheckStatus::Pass;
        sub_checks.push(SubCheck {
            name: "nuclear-data audit".into(),
            status: CheckStatus::Pass,
            why: Some("the design has no materials to audit".into()),
            next_step: None,
        });
    } else {
        sub_checks.push(SubCheck {
            name: "nuclear-data audit".into(),
            status: CheckStatus::NotEvaluated,
            why: Some("the role and material rules failed first".into()),
            next_step: Some("Fix those findings, then check again.".into()),
        });
    }
    let mut five = outcome(5, findings);
    if five.status == CheckStatus::Pass && audit_status == CheckStatus::NotEvaluated {
        five.status = CheckStatus::NotEvaluated;
        five.why = sub_checks
            .iter()
            .find(|s| s.status == CheckStatus::NotEvaluated)
            .and_then(|s| s.why.clone());
        five.next_step = sub_checks
            .iter()
            .find(|s| s.status == CheckStatus::NotEvaluated)
            .and_then(|s| s.next_step.clone());
    }
    five.sub_checks = sub_checks;
    report.checks.push(five);
    Ok(())
}

// ---------------------------------------------------------------------------
// Text for people.

/// The report as readable text, grouped by check.
pub fn render_text(report: &ImportReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("FARIS design check: {}\n", report.design.path));
    out.push_str(&format!(
        "  design id {}   sha256 {}\n",
        report.design.id.as_deref().unwrap_or("(unreadable)"),
        &report.design.sha256[..16.min(report.design.sha256.len())]
    ));
    if let Some(step) = &report.step {
        out.push_str(&format!(
            "  STEP {}   sha256 {}   {} solids   length unit {}{}\n",
            step.path,
            &step.sha256[..16.min(step.sha256.len())],
            step.solids.map_or("?".into(), |n| n.to_string()),
            step.length_unit,
            match step.unit_declared_by_file {
                Some(false) => " (the file declares none; the design file's unit was used)",
                _ => "",
            }
        ));
    }
    if let Some(tool) = &report.cad_tool {
        out.push_str(&format!(
            "  CAD helper: cadquery {} / OCP {} / OpenCASCADE {} / Python {}   {:.1} s   ({})\n",
            tool.cadquery.as_deref().unwrap_or("?"),
            tool.ocp.as_deref().unwrap_or("?"),
            tool.occt.as_deref().unwrap_or("?"),
            tool.python,
            tool.elapsed_seconds,
            tool.interpreter
        ));
    }
    out.push('\n');
    for check in &report.checks {
        out.push_str(&format!(
            "Check {}  {:<13}  {}\n",
            check.check,
            check.status.label(),
            check.name
        ));
        if let Some(why) = &check.why {
            out.push_str(&format!("    why: {why}\n"));
        }
        if let Some(next) = &check.next_step {
            out.push_str(&format!("    next step: {next}\n"));
        }
        for sub in &check.sub_checks {
            out.push_str(&format!("    - {}: {}\n", sub.name, sub.status.label()));
            if let Some(why) = &sub.why {
                out.push_str(&format!("        why: {why}\n"));
            }
            if let Some(next) = &sub.next_step {
                out.push_str(&format!("        next step: {next}\n"));
            }
        }
        for finding in &check.findings {
            out.push_str(&format!(
                "    * {}\n        why: {}\n        next step: {}\n",
                finding.item, finding.why, finding.next_step
            ));
        }
    }
    if !report.void_solids.is_empty() {
        out.push_str("\nVoid solids (GEO-039):\n");
        for v in &report.void_solids {
            out.push_str(&format!(
                "  {}  (STEP solid {}, role {})\n",
                v.id,
                v.step_index,
                v.role.label()
            ));
        }
    }
    if !report.notices.is_empty() {
        out.push_str("\nNotices:\n");
        for n in &report.notices {
            out.push_str(&format!("  check {}  {}: {}\n", n.check, n.item, n.message));
        }
    }
    out.push_str(&format!("\n{}\n{}\n", report.summary, report.scope));
    out
}

// ---------------------------------------------------------------------------
// design init.

#[derive(Debug)]
pub struct InitSummary {
    pub path: PathBuf,
    pub solids: usize,
    pub length_unit: LengthUnit,
    pub unit_declared_by_file: bool,
    pub step_sha256: String,
    pub nulls_to_fill: usize,
}

impl InitSummary {
    pub fn line(&self) -> String {
        format!(
            "Wrote a DRAFT design file with {} solids to {} (STEP unit {}{}); {} nulls to fill in, then run `faris design check`.",
            self.solids,
            self.path.display(),
            self.length_unit.label(),
            if self.unit_declared_by_file {
                ", declared by the file"
            } else {
                ", NOT declared by the file; taken from --length-unit"
            },
            self.nulls_to_fill
        )
    }
}

fn design_id_from(stem: &str) -> String {
    let mut id = String::new();
    let mut previous_dash = true;
    for ch in stem.to_lowercase().chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            id.push(ch);
            previous_dash = false;
        } else if !previous_dash {
            id.push('-');
            previous_dash = true;
        }
    }
    let id = id.trim_end_matches('-').to_string();
    if id.is_empty() { "design".into() } else { id }
}

/// Path from `from_dir` to the file `to`, with `..` where needed. The file's
/// own name is kept as given (a symlink keeps its name); directories must exist.
fn relative_path(from_dir: &Path, to: &Path) -> Option<String> {
    let from = from_dir.canonicalize().ok()?;
    let name = to.file_name()?;
    let to_dir = to
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let to_dir = to_dir.canonicalize().ok()?;
    let from_parts: Vec<_> = from.components().collect();
    let to_parts: Vec<_> = to_dir.components().collect();
    let common = from_parts
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();
    if common == 0 {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    for _ in common..from_parts.len() {
        parts.push("..".into());
    }
    for component in &to_parts[common..] {
        parts.push(component.as_os_str().to_string_lossy().into_owned());
    }
    parts.push(name.to_string_lossy().into_owned());
    Some(parts.join("/"))
}

/// The first draft for a STEP file. Pure given the inspection.
pub fn build_draft(
    step_file: &str,
    title_stem: &str,
    inspection: &StepInspection,
    unit: LengthUnit,
) -> Result<Design, DesignError> {
    let mut taken: Vec<String> = Vec::new();
    let mut solids = Vec::new();
    for solid in &inspection.solids {
        let (Some(volume), Some(centroid)) = (solid.cad_volume_m3, solid.centroid_m) else {
            return Err(DesignError::Init(format!(
                "STEP solid {} has no volume; the length unit is unknown",
                solid.step_index
            )));
        };
        let id = design::suggest_id(solid.step_name.as_deref(), solid.step_index, &taken);
        taken.push(id.clone());
        solids.push(Solid {
            id,
            step_index: solid.step_index,
            step_name: solid.step_name.clone(),
            fingerprint: Fingerprint {
                cad_volume_m3: volume,
                centroid_m: centroid,
            },
            material_id: None,
            role: None,
            replacement_group_id: None,
            tally: true,
        });
    }
    let null_profile = || Profile {
        centre: None,
        pedestal: None,
        separatrix: None,
        peaking_factor: None,
    };
    Ok(Design {
        schema_version: DESIGN_VERSION.into(),
        id: design_id_from(title_stem),
        title: title_stem.to_string(),
        description: format!(
            "Draft written by `faris design init` from {step_file}. Fill in every null, then run \
             `faris design check`."
        ),
        cad: Cad {
            step_file: step_file.to_string(),
            step_sha256: inspection.step.sha256.clone(),
            length_unit: unit,
            extent: Extent {
                kind: "full".into(),
            },
            machine_axis: "z".into(),
            faceting_tolerance_m: None,
            implicit_complement_material_id: design::VOID.into(),
        },
        materials: Vec::<DesignMaterial>::new(),
        solids,
        replacement_groups: Vec::new(),
        plasma: Plasma {
            chamber_solid_id: None,
            major_radius_m: None,
            minor_radius_m: None,
            elongation: None,
            triangularity: None,
            shafranov_shift_m: None,
            pedestal_radius_m: None,
            mode: None,
            ion_density_m3: null_profile(),
            ion_temperature_ev: TemperatureProfile {
                centre: None,
                pedestal: None,
                separatrix: None,
                peaking_factor: None,
                beta: None,
            },
            fuel: None,
            fusion_power_mw: None,
            emissivity_table: None,
            provenance: None,
            field_provenance: BTreeMap::new(),
        },
        operating_scenario: OperatingScenarioLink {
            path: None,
            sha256: None,
        },
        references: Vec::new(),
        assumptions: Vec::new(),
    })
}

/// Read the STEP file and write a draft design file. Never overwrites a file.
pub fn design_init(
    step: &Path,
    out: Option<&Path>,
    length_unit: Option<LengthUnit>,
    backends: &dyn Backends,
) -> Result<InitSummary, DesignError> {
    std::fs::metadata(step).map_err(|e| io_error(step, e))?;
    let step_abs = std::path::absolute(step).map_err(|e| io_error(step, e))?;
    let stem = step_abs
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "design".into());
    let out_path = out
        .map(Path::to_path_buf)
        .unwrap_or_else(|| step_abs.with_file_name(format!("{stem}.faris-design.json")));
    if out_path.exists() {
        return Err(DesignError::Init(format!(
            "{} already exists; FARIS never overwrites a design file. Choose another --out or \
             move the old file",
            out_path.display()
        )));
    }
    let ran = backends.inspect_step(&step_abs, length_unit)?;
    let inspection = ran.inspection;
    if !inspection.problems.is_empty() || !inspection.unit.problems.is_empty() {
        let all: Vec<String> = inspection
            .problems
            .iter()
            .chain(&inspection.unit.problems)
            .cloned()
            .collect();
        return Err(DesignError::Init(format!(
            "the STEP file cannot be used: {}. Export it again from the CAD tool with one length \
             unit (mm, cm or m).",
            all.join("; ")
        )));
    }
    let (unit, declared) = match (inspection.unit.declared.as_deref(), length_unit) {
        (Some(label), asked) => {
            let unit = match label {
                "m" => LengthUnit::M,
                "cm" => LengthUnit::Cm,
                "mm" => LengthUnit::Mm,
                other => {
                    return Err(DesignError::Init(format!(
                        "the STEP file declares {other}, which v0.1 does not accept (it accepts m, cm and mm). Export it again in mm, cm or m."
                    )));
                }
            };
            if let Some(asked) = asked
                && asked != unit
            {
                return Err(DesignError::Init(format!(
                    "the STEP file declares {label} but --length-unit {} was given; remove the option or re-export the file",
                    asked.label()
                )));
            }
            (unit, true)
        }
        (None, Some(asked)) => (asked, false),
        (None, None) => {
            return Err(DesignError::Init(
                "the STEP file declares no length unit. Say what its numbers are in with \
                 --length-unit m, cm or mm"
                    .into(),
            ));
        }
    };
    if inspection.solids.is_empty() {
        return Err(DesignError::Init("the STEP file holds no solids".into()));
    }
    let out_dir = out_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let step_ref =
        relative_path(out_dir, &step_abs).unwrap_or_else(|| step_abs.display().to_string());
    let draft = build_draft(&step_ref, &stem, &inspection, unit)?;
    let value = serde_json::to_value(&draft).map_err(|e| DesignError::Init(e.to_string()))?;
    let nulls = design::nullable_paths(&value).len();
    // Written from the typed draft, so keys keep the format document's order.
    let mut body =
        serde_json::to_vec_pretty(&draft).map_err(|e| DesignError::Init(e.to_string()))?;
    body.push(b'\n');
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&out_path)
            .map_err(|e| io_error(&out_path, e))?;
        file.write_all(&body).map_err(|e| io_error(&out_path, e))?;
    }
    Ok(InitSummary {
        path: out_path,
        solids: draft.solids.len(),
        length_unit: unit,
        unit_declared_by_file: declared,
        step_sha256: inspection.step.sha256,
        nulls_to_fill: nulls,
    })
}

/// `design init` with the external CAD helper.
pub fn design_init_with_tools(
    step: &Path,
    out: Option<&Path>,
    length_unit: Option<LengthUnit>,
    tools: &ToolPaths,
    cancellation: &Cancellation,
) -> Result<InitSummary, DesignError> {
    design_init(
        step,
        out,
        length_unit,
        &ExternalBackends {
            tools,
            cancellation,
        },
    )
}

/// `design check` with the external CAD helper and material audit.
pub fn check_design_with_tools(
    design_path: &Path,
    tools: &ToolPaths,
    cancellation: &Cancellation,
) -> Result<ImportReport, DesignError> {
    check_design(
        design_path,
        &ExternalBackends {
            tools,
            cancellation,
        },
    )
}

#[cfg(test)]
mod tests;
