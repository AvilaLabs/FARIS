//! Assembly of a `ReportInput` from loaded transport records and calculated
//! histories. The desktop and `faris study-file export` both call
//! `assemble_report_input`, so the two exports cannot differ in what they pick.

use crate::{ArrangementInput, ReportInput, Sampling, StudyFileStamp, SweepInput};
use faris_engine::{
    DemoManifest, GeometryVolumeStatus,
    brief::{Arrangement, VARIANTS, transport_summary},
    history::HistoryResult,
    reactor::ReactorRun,
};
use std::collections::BTreeMap;

/// One loaded arrangement: its scenario manifest and recorded transport,
/// keyed by variant id.
#[derive(Clone, Copy)]
pub struct LoadedArrangement<'a> {
    pub manifest: &'a DemoManifest,
    pub records: &'a BTreeMap<String, ReactorRun>,
}

/// Everything besides the arrangements that the export carries.
#[derive(Clone, Debug)]
pub struct ReportContext {
    pub study_name: String,
    pub sweep: Option<SweepInput>,
    /// Name of the operating-assumption preset the histories use.
    pub preset_label: String,
    pub preset_magnet_limit: Option<f64>,
    pub fusion_power_mw: f64,
    pub study_file: Option<StudyFileStamp>,
    pub view_image_note: Option<String>,
    pub generated_unix_s: i64,
}

/// Build the export input. `current` and `paired` are the arrangements as the
/// desktop holds them; the ported case is always reported first, as in the
/// compare view. `history` finds a calculated history by scenario SHA-256 and
/// variant id. The 3D view is never captured here; callers that have a
/// capture set `view_image` afterwards.
pub fn assemble_report_input<'a>(
    current: LoadedArrangement<'a>,
    paired: Option<LoadedArrangement<'a>>,
    history: impl Fn(&str, &str) -> Option<&'a HistoryResult>,
    context: ReportContext,
) -> ReportInput {
    let (port, control) = match paired {
        Some(other) if current.manifest.penetration.is_none() => (other, Some(current)),
        _ => (current, paired),
    };
    let mut arrangements = Vec::new();
    for (side, is_port) in [(Some(port), true), (control, false)] {
        let Some(side) = side else { continue };
        for (index, variant) in VARIANTS.iter().enumerate() {
            let record = side.records.get(*variant);
            arrangements.push(ArrangementInput {
                arrangement: Arrangement {
                    port: is_port,
                    breeder: index == 1,
                },
                transport: record
                    .filter(|r| r.normalized.is_some())
                    .map(transport_summary),
                sampling: record.map(|r| Sampling {
                    seed: r.sampling.seed,
                    histories: u64::from(r.sampling.batches)
                        * u64::from(r.sampling.particles_per_batch),
                }),
                history: record
                    .and_then(|r| history(&r.scenario_sha256, variant))
                    .cloned(),
            });
        }
    }
    let port_volume_unvalidated = std::iter::once(current.manifest)
        .chain(paired.map(|p| p.manifest))
        .any(|m| {
            m.penetration.is_some()
                && m.geometry_volume_status
                    == GeometryVolumeStatus::PenetrationEstimateNotIndependentlyValidated
        });
    ReportInput {
        study_name: context.study_name,
        arrangements,
        sweep: context.sweep,
        preset_label: context.preset_label,
        preset_magnet_limit: context.preset_magnet_limit,
        fusion_power_mw: context.fusion_power_mw,
        port_volume_unvalidated,
        study_file: context.study_file,
        view_image: None,
        view_image_note: context.view_image_note,
        generated_unix_s: context.generated_unix_s,
    }
}

/// The name the export folder and PDF title carry: the study file's name
/// without its extension.
pub fn study_name_from_path(path: Option<&std::path::Path>) -> String {
    path.and_then(std::path::Path::file_stem)
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Outboard port and allocation study".into())
}

/// Why the command-line export carries no 3D view.
pub const NO_VIEW_ON_COMMAND_LINE: &str = "The 3D view is not captured on the command line, so the PDF and charts folder have no 3D image. To include it, open the study file in the FARIS desktop app and export from there.";
