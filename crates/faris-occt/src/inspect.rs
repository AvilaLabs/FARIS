//! `step-inspect` in Rust: the report `integrations/cad/faris_cad.py step-inspect` writes
//! (schema `faris-step-inspect/v1`), with the same fields and rules, plus each solid's face
//! count, assembly path and colour.

use crate::native::StepModel;
use crate::units::{
    INTERNAL_FACTOR, UNIT_FACTORS, describe_factor, is_close, occt_name_factor, text_length_units,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, io::Read, path::Path, time::Instant};

pub const SCHEMA: &str = "faris-step-inspect/v1";
pub const HELPER_VERSION: &str = "faris-occt-1";
/// Adaptive-integration tolerance for volume and centroid, as in the Python helper.
pub const VOLUME_EPS: f64 = 1.0e-7;
/// The unit OpenCASCADE converts a file with a declared length unit into.
pub const INTERNAL_UNIT: &str = "MM";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Versions {
    pub python: String,
    pub interpreter: String,
    pub cadquery: Option<String>,
    pub ocp: Option<String>,
    pub occt: Option<String>,
    pub helper: String,
    pub occt_full: String,
    pub occt_source_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StepFile {
    pub file_name: String,
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Unit {
    pub declared: Option<String>,
    pub factor_to_m: Option<f64>,
    pub used: Option<String>,
    pub used_is_assumed: bool,
    pub text_units: Vec<String>,
    pub ocp_units: Vec<String>,
    pub problems: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Solid {
    pub step_index: usize,
    pub step_name: Option<String>,
    pub assembly_path: Option<String>,
    pub colour_srgb: Option<[f64; 3]>,
    pub faces: u32,
    pub gprop_relative_error_estimate: f64,
    pub occt_valid: bool,
    pub cad_volume_m3: Option<f64>,
    pub centroid_m: Option<[f64; 3]>,
    pub bbox_min_m: Option<[f64; 3]>,
    pub bbox_max_m: Option<[f64; 3]>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub schema: String,
    pub versions: Versions,
    pub step: StepFile,
    pub unit: Option<Unit>,
    pub solids: Vec<Solid>,
    pub model_bbox_min_m: Option<[f64; 3]>,
    pub model_bbox_max_m: Option<[f64; 3]>,
    pub model_bbox_diagonal_m: Option<f64>,
    pub volume_method: String,
    pub problems: Vec<String>,
    pub timing_seconds: BTreeMap<String, f64>,
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut digest = Sha256::new();
    let mut block = vec![0u8; 1 << 20];
    loop {
        let n = file
            .read(&mut block)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        digest.update(&block[..n]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

pub fn versions() -> Versions {
    let full = crate::native::occt_version();
    Versions {
        python: "none".into(),
        interpreter: std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        cadquery: None,
        ocp: None,
        occt: Some(full.split('.').take(2).collect::<Vec<_>>().join(".")),
        helper: HELPER_VERSION.into(),
        occt_full: full,
        occt_source_sha256: env!("FARIS_OCCT_SHA256").into(),
    }
}

/// Names OpenCASCADE gives a solid when the file carries none.
fn name_is_synthetic(name: &str) -> bool {
    Regex::new(r"^Open CASCADE STEP translator [0-9.]+ [0-9.]+$")
        .expect("pattern")
        .is_match(name)
}

/// The usable name of a raw XCAF name, as the Python helper's `solid_name` returns it.
pub fn clean_name(raw: Option<&str>) -> Option<String> {
    let name = raw?;
    if name.trim().is_empty() || name_is_synthetic(name) {
        return None;
    }
    Some(name.to_string())
}

fn factor_list(factors: &[f64]) -> Vec<String> {
    factors.iter().map(|f| describe_factor(*f)).collect()
}

/// Reads `path` and reports it. `assume_unit` is one of `m`, `cm`, `mm`, `inch`.
pub fn inspect_step(path: &Path, assume_unit: Option<&str>) -> Result<Report, String> {
    let sha256 = sha256_file(path)?;
    let size_bytes = fs::metadata(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .len();
    let mut report = Report {
        schema: SCHEMA.into(),
        versions: versions(),
        step: StepFile {
            file_name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            sha256,
            size_bytes,
        },
        unit: None,
        solids: Vec::new(),
        model_bbox_min_m: None,
        model_bbox_max_m: None,
        model_bbox_diagonal_m: None,
        volume_method: format!(
            "BRepGProp.VolumeProperties adaptive, eps={}",
            crate::units::format_g(VOLUME_EPS)
        ),
        problems: Vec::new(),
        timing_seconds: BTreeMap::new(),
    };
    let assume_factor = match assume_unit {
        None => None,
        Some(label) => Some(
            UNIT_FACTORS
                .iter()
                .find(|(l, _)| *l == label)
                .map(|(_, f)| *f)
                .ok_or_else(|| format!("unknown unit {label:?}; use one of m, cm, mm, inch"))?,
        ),
    };

    let started = Instant::now();
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (text_factors, text_problems) = text_length_units(&bytes);
    drop(bytes);
    report
        .timing_seconds
        .insert("unit_text_parse".into(), started.elapsed().as_secs_f64());

    let started = Instant::now();
    let model = match StepModel::read(&path.display().to_string(), INTERNAL_UNIT) {
        Ok(model) => model,
        Err(error) => {
            report.problems.push(format!("STEP read failed: {error}"));
            report.unit = Some(Unit {
                declared: None,
                factor_to_m: None,
                used: None,
                used_is_assumed: false,
                text_units: factor_list(&text_factors),
                ocp_units: Vec::new(),
                problems: text_problems,
            });
            return Ok(report);
        }
    };
    report
        .timing_seconds
        .insert("read_and_transfer".into(), started.elapsed().as_secs_f64());

    // The units OpenCASCADE found.
    let mut ocp_factors: Vec<f64> = Vec::new();
    let mut ocp_problems: Vec<String> = Vec::new();
    for name in model.file_length_units() {
        match occt_name_factor(&name) {
            None => ocp_problems.push(format!(
                "OpenCASCADE reports the unit '{name}', which FARIS does not recognise"
            )),
            Some(f) => {
                if !ocp_factors.iter().any(|seen| is_close(f, *seen)) {
                    ocp_factors.push(f);
                }
            }
        }
    }
    ocp_factors.sort_by(|a, b| a.total_cmp(b));
    ocp_problems.sort();
    ocp_problems.dedup();

    let mut unit_problems: Vec<String> = text_problems;
    unit_problems.extend(ocp_problems);
    let mut declared = None;
    let mut factor = None;
    if text_factors.len() > 1 || ocp_factors.len() > 1 {
        unit_problems.push(format!(
            "the file declares several different length units: text {}, OpenCASCADE {}",
            py_list(&factor_list(&text_factors)),
            py_list(&factor_list(&ocp_factors))
        ));
    } else if text_factors.len() != ocp_factors.len()
        || text_factors
            .iter()
            .zip(&ocp_factors)
            .any(|(a, b)| !is_close(*a, *b))
    {
        let show = |list: &[f64]| {
            if list.is_empty() {
                "no unit".to_string()
            } else {
                py_list(&factor_list(list))
            }
        };
        unit_problems.push(format!(
            "the two unit readers disagree: text parse {}, OpenCASCADE {}",
            show(&text_factors),
            show(&ocp_factors)
        ));
    } else if let Some(first) = text_factors.first() {
        factor = Some(*first);
        declared = Some(describe_factor(*first));
    }
    let mut used = None;
    let mut used_factor = None; // metres per shape unit
    if unit_problems.is_empty() {
        if let Some(label) = &declared {
            used = Some(label.clone());
            used_factor = Some(INTERNAL_FACTOR);
        } else if let (Some(label), Some(f)) = (assume_unit, assume_factor) {
            used = Some(label.to_string());
            used_factor = Some(f);
        }
    }
    report.unit = Some(Unit {
        declared: declared.clone(),
        factor_to_m: factor,
        used: used.clone(),
        used_is_assumed: declared.is_none() && used.is_some(),
        text_units: factor_list(&text_factors),
        ocp_units: factor_list(&ocp_factors),
        problems: unit_problems,
    });

    let started = Instant::now();
    let mut lows: Vec<[f64; 3]> = Vec::new();
    let mut highs: Vec<[f64; 3]> = Vec::new();
    for index in 0..model.solid_count() {
        let info = model
            .solid_info(index, VOLUME_EPS)
            .map_err(|e| format!("solid {index}: {e}"))?;
        lows.push(info.bbox_min);
        highs.push(info.bbox_max);
        let scale = |v: [f64; 3], f: f64| [v[0] * f, v[1] * f, v[2] * f];
        report.solids.push(Solid {
            step_index: index,
            step_name: clean_name(info.has_name.then_some(info.name.as_str())),
            assembly_path: (!info.assembly_path.is_empty()).then(|| info.assembly_path.clone()),
            colour_srgb: info.has_colour.then_some(info.colour),
            faces: info.faces,
            gprop_relative_error_estimate: info.volume_error,
            occt_valid: info.valid,
            cad_volume_m3: used_factor.map(|f| info.volume * f * f * f),
            centroid_m: used_factor.map(|f| scale(info.centroid, f)),
            bbox_min_m: used_factor.map(|f| scale(info.bbox_min, f)),
            bbox_max_m: used_factor.map(|f| scale(info.bbox_max, f)),
        });
    }
    report
        .timing_seconds
        .insert("measure_solids".into(), started.elapsed().as_secs_f64());
    if let (Some(f), false) = (used_factor, lows.is_empty()) {
        let low: [f64; 3] =
            std::array::from_fn(|i| lows.iter().map(|v| v[i]).fold(f64::INFINITY, f64::min));
        let high: [f64; 3] =
            std::array::from_fn(|i| highs.iter().map(|v| v[i]).fold(f64::NEG_INFINITY, f64::max));
        report.model_bbox_min_m = Some([low[0] * f, low[1] * f, low[2] * f]);
        report.model_bbox_max_m = Some([high[0] * f, high[1] * f, high[2] * f]);
        let diagonal = (0..3)
            .map(|i| (high[i] - low[i]) * (high[i] - low[i]))
            .sum::<f64>()
            .sqrt();
        report.model_bbox_diagonal_m = Some(diagonal * f);
    }
    Ok(report)
}

/// Python's list repr of strings, as the helper's messages show them.
fn py_list(items: &[String]) -> String {
    format!(
        "[{}]",
        items
            .iter()
            .map(|s| format!("'{s}'"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}
