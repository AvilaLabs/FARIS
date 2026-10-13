//! `faris design transport`: the fast-neutron flux in a checked design's TF magnets.
//!
//! The design must have passed `faris design check` with every check evaluated; the
//! check report names the conversion whose h5m is run. The OpenMC worker
//! (`integrations/openmc/design_transport.py`) runs as a bounded job and stops at a
//! target relative error or at its history or time limit. It answers per source
//! neutron; the source rate and the volumes are applied here.

use super::{
    CAD_ADDRESS_SPACE, DESIGN_TRANSPORT_WORKER, DesignError, DesignIdentity, ImportReport,
    Launched, MAX_JSON_BYTES, ToolPaths, io_error, launch_python, sha256_file, sha256_hex,
};
use crate::jobs::{Cancellation, ResourceLimits};
use faris_model::design::{self, Design, Metric, Role};
use faris_model::transport::FAST_NEUTRON_ENERGY_MIN_EV;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fmt::Write as _,
    path::{Path, PathBuf},
    time::Duration,
};

pub const DESIGN_TRANSPORT_VERSION: &str = "faris-design-transport-report/v0.1";
const REQUEST_SCHEMA: &str = "faris-design-transport-request/v1";
const RESULT_SCHEMA: &str = "faris-design-transport/v1";
/// Energy released per D-T reaction, as in the reactor transport request.
const ENERGY_PER_REACTION_EV: f64 = 17.6e6;
const ELEMENTARY_CHARGE_C: f64 = 1.602_176_634e-19;
const MIN_BATCHES: u64 = 10;
/// Time the worker gets beyond its sampling limit to load the geometry and write its result.
const STARTUP_ALLOWANCE: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Debug)]
pub struct DesignTransportOptions {
    /// The JSON report `faris design check --report` wrote for this design.
    pub check_report: PathBuf,
    /// A new directory for the run.
    pub out_dir: PathBuf,
    pub target_relative_error: f64,
    pub particles_per_batch: u64,
    pub max_histories: u64,
    pub max_seconds: u64,
    pub seed: u64,
    pub threads: u32,
    pub cancellation: Cancellation,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct WorkerValue {
    mean: f64,
    standard_error: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct WorkerSolid {
    solid_id: String,
    mean: f64,
    standard_error: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct WorkerResult {
    schema: String,
    openmc: String,
    h5m_sha256: String,
    void_h5m_sha256: String,
    model_xml_sha256: String,
    seed: u64,
    threads: u32,
    particles_per_batch: u64,
    batches: u64,
    histories: u64,
    elapsed_seconds: f64,
    stop_reason: String,
    target_relative_error: f64,
    target_reached: bool,
    estimator: String,
    unit: String,
    energy_min_ev: f64,
    total: WorkerValue,
    solids: Vec<WorkerSolid>,
    why: Option<String>,
    next_step: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SolidFlux {
    pub solid_id: String,
    pub volume_m3: f64,
    pub flux_n_m2_s: f64,
    /// `None`: fewer than two batches, or no score.
    pub relative_error: Option<f64>,
}

/// Something the run did not compute, with the reason and what to do.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct NotEvaluated {
    pub item: String,
    pub why: String,
    pub next_step: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DesignTransportReport {
    pub schema_version: String,
    pub design: DesignIdentity,
    pub check_report_sha256: String,
    pub h5m_sha256: String,
    pub cross_sections_sha256: String,
    pub openmc: String,
    pub run_directory: String,
    pub energy_min_ev: f64,
    /// Source neutrons per second, from the fusion power at 17.6 MeV per reaction.
    pub source_rate_per_s: f64,
    pub fusion_power_mw: f64,
    pub histories: u64,
    pub elapsed_seconds: f64,
    pub seed: u64,
    pub threads: u32,
    pub target_relative_error: f64,
    pub target_reached: bool,
    pub stop_reason: String,
    /// Why the target was not reached and what to do; `None` when it was.
    pub why: Option<String>,
    pub next_step: Option<String>,
    /// Volume-averaged flux above the threshold over all TF magnet solids.
    pub mean_flux_n_m2_s: f64,
    pub mean_relative_error: Option<f64>,
    pub total_volume_m3: f64,
    pub solids: Vec<SolidFlux>,
    pub notices: Vec<String>,
    pub not_evaluated: Vec<NotEvaluated>,
}

/// Flux in n/m²/s from a track length per source neutron (cm), a source rate and a volume.
pub fn flux_n_m2_s(cm_per_source: f64, source_rate_per_s: f64, volume_m3: f64) -> f64 {
    // cm / cm³ = 1/cm² = 1e4 /m²; the volume is 1e6 cm³ per m³.
    cm_per_source * source_rate_per_s / (volume_m3 * 1.0e6) * 1.0e4
}

fn source_rate_per_s(fusion_power_mw: f64) -> f64 {
    fusion_power_mw * 1.0e6 / (ENERGY_PER_REACTION_EV * ELEMENTARY_CHARGE_C)
}

fn tool(message: impl Into<String>) -> DesignError {
    DesignError::Tool(message.into())
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, DesignError> {
    let metadata = std::fs::metadata(path).map_err(|e| io_error(path, e))?;
    if !metadata.is_file() || metadata.len() > MAX_JSON_BYTES {
        return Err(io_error(path, "not a regular file under 64 MiB"));
    }
    std::fs::read(path).map_err(|e| io_error(path, e))
}

/// One entry per solid that is not void: its nuclide vector from the check's audit and its density.
fn material_entries(
    design: &Design,
    report: &ImportReport,
) -> Result<Vec<serde_json::Value>, DesignError> {
    let audited: BTreeMap<&str, &BTreeMap<String, f64>> = report
        .material_audit
        .iter()
        .flat_map(|a| &a.materials)
        .filter_map(|m| Some((m.id.as_str(), m.nuclides.as_ref()?)))
        .collect();
    let mut entries = Vec::new();
    for solid in &design.solids {
        let Some(material_id) = solid.material_id.as_deref() else {
            return Err(tool(format!("solid {} has no material", solid.id)));
        };
        if material_id == design.cad.implicit_complement_material_id || material_id == "void" {
            continue;
        }
        let material = design
            .materials
            .iter()
            .find(|m| m.id == material_id)
            .ok_or_else(|| tool(format!("material {material_id} is not in the design file")))?;
        let nuclides = audited.get(material_id).ok_or_else(|| {
            tool(format!(
                "the check report holds no audited composition for material {material_id}; run \
                 `faris design check` with --openmc-python and --cross-sections and pass its report"
            ))
        })?;
        let density = material
            .density_kg_m3
            .or_else(|| {
                material
                    .catalog_id
                    .as_deref()
                    .and_then(design::catalog_material)
                    .map(|c| c.density_kg_m3)
            })
            .ok_or_else(|| tool(format!("material {material_id} has no density")))?;
        entries.push(serde_json::json!({
            "solid_id": solid.id,
            "nuclide_atom_fractions": nuclides,
            "density_kg_m3": density,
            "temperature_k": material.temperature_k,
        }));
    }
    Ok(entries)
}

/// The check report must describe this design file and a complete, passing import.
fn accept_check_report(report: &ImportReport, design_sha256: &str) -> Result<(), DesignError> {
    if !report.design.sha256.eq_ignore_ascii_case(design_sha256) {
        return Err(tool(
            "the check report was written for a different version of the design file; run \
             `faris design check --report` again and pass the new report",
        ));
    }
    if !report.import_complete {
        return Err(tool(format!(
            "the design has not passed all nine import checks ({}); transport runs only on a \
             design that imports. Resolve the findings of `faris design check` first",
            report.summary
        )));
    }
    Ok(())
}

/// Run the fast-neutron flux response of a checked design to a target error.
pub fn design_transport(
    design_path: &Path,
    tools: &ToolPaths,
    options: &DesignTransportOptions,
) -> Result<DesignTransportReport, DesignError> {
    if !(options.target_relative_error > 0.0 && options.target_relative_error < 1.0) {
        return Err(tool("the target relative error must lie between 0 and 1"));
    }
    if options.particles_per_batch == 0
        || options.max_histories < MIN_BATCHES * options.particles_per_batch
    {
        return Err(tool(format!(
            "the history limit must allow at least {MIN_BATCHES} batches of {} histories",
            options.particles_per_batch
        )));
    }
    let python = tools.openmc_python.as_deref().ok_or_else(|| {
        tool("no OpenMC interpreter is configured; pass --openmc-python (or set FARIS_OPENMC_PYTHON)")
    })?;
    let cross_sections = tools.cross_sections.as_deref().ok_or_else(|| {
        tool("no nuclear data is configured; pass --cross-sections (or set FARIS_CROSS_SECTIONS)")
    })?;
    let plasma_source = tools.plasma_source_path.as_deref().ok_or_else(|| {
        tool(
            "no path to openmc_plasma_source is configured; pass --plasma-source-path (or set \
             FARIS_PLASMA_SOURCE_PATH)",
        )
    })?;

    let design_bytes = read_bounded(design_path)?;
    let design_sha256 = sha256_hex(&design_bytes);
    let design = design::parse_design(&design_bytes)
        .design
        .ok_or_else(|| tool("the design file does not parse; run `faris design check`"))?;
    let report_bytes = read_bounded(&options.check_report)?;
    let report: ImportReport = serde_json::from_slice(&report_bytes).map_err(|e| {
        tool(format!(
            "{} is not a design check report: {e}",
            options.check_report.display()
        ))
    })?;
    accept_check_report(&report, &design_sha256)?;
    let conversion = report
        .geometry
        .conversion
        .as_ref()
        .ok_or_else(|| tool("the check report records no conversion"))?;
    let h5m = Path::new(&conversion.directory).join("model.h5m");
    if !h5m.is_file() || !sha256_file(&h5m)?.eq_ignore_ascii_case(&conversion.h5m_sha256) {
        return Err(tool(format!(
            "{} is missing or is not the geometry the check report describes; run `faris design \
             check --work-dir DIR --report FILE` again and keep DIR",
            h5m.display()
        )));
    }

    let response: Vec<(&str, f64)> = design
        .solids
        .iter()
        .filter(|s| s.role == Some(Role::TfMagnet) && s.material_id.as_deref() != Some("void"))
        .map(|s| (s.id.as_str(), s.fingerprint.cad_volume_m3))
        .collect();
    if response.is_empty() {
        return Err(tool(
            "the design has no solid with the role tf_magnet, so there is no magnet response to \
             compute; give the TF coil solids that role in the design file",
        ));
    }
    let fusion_power_mw = design
        .plasma
        .fusion_power_mw
        .ok_or_else(|| tool("the design's plasma block has no fusion power"))?;
    let void_ids: Vec<&str> = design
        .solids
        .iter()
        .filter(|s| s.material_id.as_deref() == Some("void"))
        .map(|s| s.id.as_str())
        .collect();

    let dir = &options.out_dir;
    std::fs::create_dir(dir).map_err(|e| {
        io_error(
            dir,
            format!("{e} (the run directory must be new; nothing is overwritten)"),
        )
    })?;
    let dir = dir.canonicalize().map_err(|e| io_error(dir, e))?;
    let cross_sections = cross_sections
        .canonicalize()
        .map_err(|e| io_error(cross_sections, e))?;
    let cross_sections_sha256 = sha256_file(&cross_sections)?;
    let request = serde_json::json!({
        "schema": REQUEST_SCHEMA,
        "h5m": h5m,
        "cross_sections": cross_sections,
        "void_solid_ids": void_ids,
        "materials": material_entries(&design, &report)?,
        "plasma": design.plasma,
        "response": {
            "solid_ids": response.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            "energy_min_ev": FAST_NEUTRON_ENERGY_MIN_EV,
        },
        "sampling": {
            "target_relative_error": options.target_relative_error,
            "particles_per_batch": options.particles_per_batch,
            "min_batches": MIN_BATCHES,
            "max_batches": options.max_histories / options.particles_per_batch,
            "max_seconds": options.max_seconds,
            "seed": options.seed,
            "threads": options.threads,
        },
    });
    let script = dir.join("design_transport.py");
    std::fs::write(&script, DESIGN_TRANSPORT_WORKER).map_err(|e| io_error(&script, e))?;
    let request_path = dir.join("request.json");
    std::fs::write(
        &request_path,
        serde_json::to_vec_pretty(&request).unwrap_or_default(),
    )
    .map_err(|e| io_error(&request_path, e))?;
    let solver = dir.join("solver");
    let tmp = dir.join("tmp");
    std::fs::create_dir_all(&tmp).map_err(|e| io_error(&tmp, e))?;
    let arguments: Vec<OsString> = vec![
        script.into_os_string(),
        "--request".into(),
        request_path.into_os_string(),
        "--out-dir".into(),
        solver.clone().into_os_string(),
    ];
    let environment: Vec<(OsString, OsString)> = vec![
        ("TMPDIR".into(), tmp.into_os_string()),
        // OpenMC initialises MPI, which refuses to start without a home directory.
        ("HOME".into(), dir.as_os_str().to_owned()),
        ("PYTHONPATH".into(), plasma_source.as_os_str().to_owned()),
        ("OMP_NUM_THREADS".into(), options.threads.to_string().into()),
    ];
    match launch_python(
        python,
        arguments,
        &dir,
        Duration::from_secs(options.max_seconds) + STARTUP_ALLOWANCE,
        "the transport run",
        &options.cancellation,
        environment,
        ResourceLimits::with_address_space(CAD_ADDRESS_SPACE),
    )? {
        Launched::Succeeded(_) => {}
        Launched::Failed(text) => return Err(tool(text)),
    }
    let result_path = solver.join("result.json");
    let worker: WorkerResult = serde_json::from_slice(&read_bounded(&result_path)?)
        .map_err(|e| tool(format!("{}: {e}", result_path.display())))?;
    assemble(
        worker,
        Inputs {
            design_identity: DesignIdentity {
                path: design_path.display().to_string(),
                sha256: design_sha256,
                id: Some(design.id.clone()),
            },
            check_report_sha256: sha256_hex(&report_bytes),
            h5m_sha256: conversion.h5m_sha256.clone(),
            cross_sections_sha256,
            audit_cross_sections_sha256: report
                .material_audit
                .as_ref()
                .and_then(|a| a.cross_sections_sha256.clone()),
            run_directory: dir.display().to_string(),
            fusion_power_mw,
            response: &response,
            design: &design,
        },
    )
}

struct Inputs<'a> {
    design_identity: DesignIdentity,
    check_report_sha256: String,
    h5m_sha256: String,
    cross_sections_sha256: String,
    audit_cross_sections_sha256: Option<String>,
    run_directory: String,
    fusion_power_mw: f64,
    response: &'a [(&'a str, f64)],
    design: &'a Design,
}

/// Check the worker's answer against the request and convert it to fluxes.
fn assemble(worker: WorkerResult, inputs: Inputs) -> Result<DesignTransportReport, DesignError> {
    if worker.schema != RESULT_SCHEMA || worker.unit != "cm_per_source" {
        return Err(tool(
            "the transport worker's result has an unknown schema or unit",
        ));
    }
    if !worker.h5m_sha256.eq_ignore_ascii_case(&inputs.h5m_sha256) {
        return Err(tool(
            "the transport worker ran a different geometry than the checked one",
        ));
    }
    if worker.histories != worker.batches * worker.particles_per_batch
        || worker.solids.len() != inputs.response.len()
        || worker
            .solids
            .iter()
            .zip(inputs.response)
            .any(|(w, (id, _))| w.solid_id != *id)
    {
        return Err(tool(
            "the transport worker's result does not match its request",
        ));
    }
    let finite = |v: f64| v.is_finite() && v >= 0.0;
    if !finite(worker.total.mean) || worker.solids.iter().any(|s| !finite(s.mean)) {
        return Err(tool(
            "the transport worker returned a value that is not a finite score",
        ));
    }
    let rate = source_rate_per_s(inputs.fusion_power_mw);
    let relative = |mean: f64, se: Option<f64>| se.filter(|_| mean > 0.0).map(|se| se / mean);
    let total_volume_m3: f64 = inputs.response.iter().map(|(_, v)| v).sum();
    let solids: Vec<SolidFlux> = worker
        .solids
        .iter()
        .zip(inputs.response)
        .map(|(w, (id, volume))| SolidFlux {
            solid_id: (*id).to_string(),
            volume_m3: *volume,
            flux_n_m2_s: flux_n_m2_s(w.mean, rate, *volume),
            relative_error: relative(w.mean, w.standard_error),
        })
        .collect();
    let mut notices = vec![
        "Volumes are the CAD volumes of the design file's fingerprints.".to_string(),
        "The run is analog (no weight windows).".to_string(),
    ];
    if let Some(audited) = &inputs.audit_cross_sections_sha256
        && !audited.eq_ignore_ascii_case(&inputs.cross_sections_sha256)
    {
        notices.push(
            "The nuclear data of this run is not the library the design check audited.".into(),
        );
    }
    let mut not_evaluated = Vec::new();
    for group in &inputs.design.replacement_groups {
        for limit in &group.limits {
            if limit.metric == Metric::FastNeutronFluence {
                not_evaluated.push(NotEvaluated {
                    item: format!(
                        "{}: fast-neutron fluence against the limit of {:e} n/m²",
                        group.id, limit.limit
                    ),
                    why: "This run gives a flux averaged over each coil. The limit applies to a \
                          fluence, which also needs the operating history, at the averaging the \
                          design file asks for."
                        .into(),
                    next_step: "Not available from this command yet; compare the highest coil \
                                flux above with the limit by hand only as a screening value."
                        .into(),
                });
            }
        }
    }
    Ok(DesignTransportReport {
        schema_version: DESIGN_TRANSPORT_VERSION.into(),
        design: inputs.design_identity,
        check_report_sha256: inputs.check_report_sha256,
        h5m_sha256: inputs.h5m_sha256,
        cross_sections_sha256: inputs.cross_sections_sha256,
        openmc: worker.openmc,
        run_directory: inputs.run_directory,
        energy_min_ev: worker.energy_min_ev,
        source_rate_per_s: rate,
        fusion_power_mw: inputs.fusion_power_mw,
        histories: worker.histories,
        elapsed_seconds: worker.elapsed_seconds,
        seed: worker.seed,
        threads: worker.threads,
        target_relative_error: worker.target_relative_error,
        target_reached: worker.target_reached,
        stop_reason: worker.stop_reason,
        why: worker.why,
        next_step: worker.next_step,
        mean_flux_n_m2_s: flux_n_m2_s(worker.total.mean, rate, total_volume_m3),
        mean_relative_error: relative(worker.total.mean, worker.total.standard_error),
        total_volume_m3,
        solids,
        notices,
        not_evaluated,
    })
}

fn percent(relative: Option<f64>) -> String {
    relative.map_or_else(
        || "error unknown".into(),
        |r| format!("± {:.1} %", 100.0 * r),
    )
}

/// The report as text for the terminal.
pub fn render_transport_text(report: &DesignTransportReport) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Fast-neutron flux (above {} MeV) in the TF magnets of {}",
        report.energy_min_ev / 1.0e6,
        report.design.id.as_deref().unwrap_or("the design")
    );
    let _ = writeln!(
        out,
        "  Mean over {} coil solids: {:.3e} n/m²/s {}",
        report.solids.len(),
        report.mean_flux_n_m2_s,
        percent(report.mean_relative_error)
    );
    if let Some(highest) = report
        .solids
        .iter()
        .max_by(|a, b| a.flux_n_m2_s.total_cmp(&b.flux_n_m2_s))
    {
        let _ = writeln!(
            out,
            "  Highest coil: {} at {:.3e} n/m²/s {}",
            highest.solid_id,
            highest.flux_n_m2_s,
            percent(highest.relative_error)
        );
    }
    let target = 100.0 * report.target_relative_error;
    if report.target_reached {
        let _ = writeln!(out, "  Target of {target} % on the mean: reached.");
    } else {
        let _ = writeln!(out, "  Target of {target} % on the mean: NOT reached.");
        if let (Some(why), Some(next)) = (&report.why, &report.next_step) {
            let _ = writeln!(out, "    Why: {why}");
            let _ = writeln!(out, "    Next: {next}");
        }
    }
    let _ = writeln!(
        out,
        "  {} histories in {:.0} s on {} thread(s); source {:.3e} n/s from {} MW of fusion power.",
        report.histories,
        report.elapsed_seconds,
        report.threads,
        report.source_rate_per_s,
        report.fusion_power_mw
    );
    for notice in &report.notices {
        let _ = writeln!(out, "  Note: {notice}");
    }
    for item in &report.not_evaluated {
        let _ = writeln!(out, "  Not evaluated: {}", item.item);
        let _ = writeln!(out, "    Why: {}", item.why);
        let _ = writeln!(out, "    Next: {}", item.next_step);
    }
    let _ = writeln!(out, "  Run directory: {}", report.run_directory);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flux_conversion_has_the_right_units() {
        // 1 cm of track per source neutron in 1 m³ at 1e6 n/s: 1e6 cm/s in 1e6 cm³ = 1 /cm²/s.
        assert!((flux_n_m2_s(1.0, 1.0e6, 1.0) - 1.0e4).abs() < 1.0e-9);
    }

    #[test]
    fn source_rate_follows_the_fusion_power() {
        // 525 MW at 17.6 MeV per reaction.
        let rate = source_rate_per_s(525.0);
        assert!((rate / 1.8618e20 - 1.0).abs() < 1.0e-3, "{rate:e}");
    }
}
