//! The Monte Carlo ensemble of each arrangement's operating history, as the
//! export shows it: ranges beside nominal values, the per-sample and summary
//! tables, the bands on the charts and the manifest records.
//!
//! Every range is transport Monte Carlo sampling uncertainty only. The text
//! comes from `faris_engine::history_uncertainty`, the same functions the
//! desktop views use, so the export and the screen cannot disagree.

use crate::{
    ArrangementData,
    charts::{Band, BandSeries},
};
use faris_engine::{
    brief::Arrangement,
    history::JULIAN_YEAR_SECONDS,
    history_ensemble::{
        EnsembleComparison, EnsembleStatus, HistoryEnsemble, compare_ensembles, nominal_sample,
    },
    history_uncertainty::{
        BAND_FLUENCE_MAGNETS, BAND_LIMIT_FLUENCE_MAGNETS, BAND_NET_ELECTRICITY, BAND_TRITIUM,
        CONTRASTS, HistoryContrast, UncertaintyRow, history_rows, not_evaluated_text,
    },
};
use serde::Serialize;
use std::sync::Arc;

/// What the desktop hands over for one arrangement's ensemble.
#[derive(Clone, Debug, Default)]
pub enum EnsembleInput {
    /// No ensemble was offered (for example, no history).
    #[default]
    None,
    /// The ensemble could not be calculated; the text says why.
    Failed(String),
    Ready(Arc<HistoryEnsemble>),
}

/// How one arrangement's column of the table stands.
#[derive(Clone, Debug)]
pub enum ColumnStatus {
    Evaluated {
        samples: u32,
        rejections: u64,
        seed: u64,
    },
    NotEvaluated {
        why: String,
        next_step: String,
    },
    Failed(String),
    /// A history exists but no ensemble was supplied.
    NotCalculated,
    /// Nothing recorded or calculated for this arrangement.
    NoHistory,
}

#[derive(Clone, Debug)]
pub struct Column {
    pub arrangement: Arrangement,
    pub status: ColumnStatus,
    /// Every output with its nominal value and, when evaluated, its range.
    pub rows: Vec<UncertaintyRow>,
}

#[derive(Clone, Debug)]
pub struct UncertaintyReport {
    pub columns: Vec<Column>,
}

impl UncertaintyReport {
    /// Output names and labels in first-seen order across the columns.
    pub fn row_labels(&self) -> Vec<(String, String)> {
        let mut labels: Vec<(String, String)> = Vec::new();
        for column in &self.columns {
            for row in &column.rows {
                if !labels.iter().any(|(name, _)| *name == row.name) {
                    labels.push((row.name.clone(), row.label.clone()));
                }
            }
        }
        labels
    }

    /// Reasons shared by arrangements, each with the arrangements it applies
    /// to: (labels, why, next step).
    pub fn not_evaluated_notes(&self) -> Vec<(Vec<&'static str>, String, String)> {
        let mut notes: Vec<(Vec<&'static str>, String, String)> = Vec::new();
        for column in &self.columns {
            let (why, next) = match &column.status {
                ColumnStatus::NotEvaluated { why, next_step } => (why.clone(), next_step.clone()),
                ColumnStatus::Failed(why) => (
                    format!("The ensemble could not be calculated: {why}"),
                    "Recalculate the study and export again".into(),
                ),
                ColumnStatus::NotCalculated => (
                    "The ensemble was not calculated before this export".into(),
                    "Wait for the uncertainty to finish in the desktop app, then export again"
                        .into(),
                ),
                _ => continue,
            };
            match notes.iter_mut().find(|n| n.1 == why && n.2 == next) {
                Some(note) => note.0.push(column.arrangement.label()),
                None => notes.push((vec![column.arrangement.label()], why, next)),
            }
        }
        notes
    }
}

/// None when no arrangement offered an ensemble.
pub(crate) fn build(data: &[ArrangementData]) -> Option<UncertaintyReport> {
    if data
        .iter()
        .all(|d| matches!(d.input.ensemble, EnsembleInput::None))
    {
        return None;
    }
    let columns = data
        .iter()
        .map(|d| {
            let nominal = d.input.history.as_ref().and_then(nominal_sample);
            let (status, ensemble) = match &d.input.ensemble {
                EnsembleInput::Ready(e) => match &e.status {
                    EnsembleStatus::Evaluated => (
                        ColumnStatus::Evaluated {
                            samples: e.samples.len() as u32,
                            rejections: e.rejections,
                            seed: e.seed,
                        },
                        Some(e.as_ref()),
                    ),
                    EnsembleStatus::NotEvaluated { why, next_step } => (
                        ColumnStatus::NotEvaluated {
                            why: why.clone(),
                            next_step: next_step.clone(),
                        },
                        Some(e.as_ref()),
                    ),
                },
                EnsembleInput::Failed(why) => (ColumnStatus::Failed(why.clone()), None),
                EnsembleInput::None if nominal.is_some() => (ColumnStatus::NotCalculated, None),
                EnsembleInput::None => (ColumnStatus::NoHistory, None),
            };
            let rows = d
                .input
                .history
                .as_ref()
                .map(|h| history_rows(h, ensemble))
                .unwrap_or_default();
            Column {
                arrangement: d.input.arrangement,
                status,
                rows,
            }
        })
        .collect();
    Some(UncertaintyReport { columns })
}

fn band_of(ensemble: &HistoryEnsemble, name: &str, scale: f64) -> Option<Band> {
    let summary = ensemble.summary.as_ref()?;
    let band = summary.series_bands.iter().find(|b| b.name == name)?;
    let scaled = |values: &[Option<f64>]| values.iter().map(|v| v.map(|x| x * scale)).collect();
    Some(Band {
        years: summary
            .time_grid_s
            .iter()
            .map(|t| t / JULIAN_YEAR_SECONDS)
            .collect(),
        low: scaled(&band.p5),
        high: scaled(&band.p95),
    })
}

fn evaluated(d: &ArrangementData) -> Option<&HistoryEnsemble> {
    match &d.input.ensemble {
        EnsembleInput::Ready(e) if e.status == EnsembleStatus::Evaluated => Some(e.as_ref()),
        _ => None,
    }
}

/// The band of the magnet-fluence timeline for one arrangement.
pub(crate) fn fluence_band(d: &ArrangementData) -> Option<Band> {
    let ensemble = evaluated(d)?;
    // A magnet with region limits plots its exposure toward them.
    band_of(ensemble, BAND_LIMIT_FLUENCE_MAGNETS, 1.0)
        .or_else(|| band_of(ensemble, BAND_FLUENCE_MAGNETS, 1.0))
}

/// The tritium and net-electricity charts: nominal curve and band for every
/// arrangement with an evaluated ensemble. Empty when none has one.
pub(crate) fn band_series(data: &[ArrangementData]) -> (Vec<BandSeries>, Vec<BandSeries>) {
    let mut tritium = Vec::new();
    let mut electricity = Vec::new();
    for d in data {
        let (Some(ensemble), Some(history)) = (evaluated(d), d.input.history.as_ref()) else {
            continue;
        };
        let years = |s: &faris_engine::history::HistorySnapshot| s.time_s / JULIAN_YEAR_SECONDS;
        if let Some(band) = band_of(ensemble, BAND_TRITIUM, 1.0) {
            tritium.push(BandSeries {
                arrangement: d.input.arrangement,
                nominal: history
                    .snapshots
                    .iter()
                    .map(|s| [years(s), s.available_tritium_kg])
                    .collect(),
                band,
            });
        }
        if let Some(band) = band_of(ensemble, BAND_NET_ELECTRICITY, 1.0e-6) {
            electricity.push(BandSeries {
                arrangement: d.input.arrangement,
                nominal: history
                    .snapshots
                    .iter()
                    .filter_map(|s| {
                        s.cumulative_net_electricity_mwh
                            .map(|v| [years(s), v * 1.0e-6])
                    })
                    .collect(),
                band,
            });
        }
    }
    (tritium, electricity)
}

/// One arrangement's line of the manifest's `history_ensembles` list.
#[derive(Serialize)]
pub(crate) struct EnsembleRecord {
    pub arrangement: &'static str,
    pub method: Option<String>,
    /// As text: a 64-bit seed does not survive a round trip through JSON numbers.
    pub seed: Option<String>,
    pub samples_requested: Option<u32>,
    pub samples_accepted: Option<u32>,
    pub rejections: Option<u64>,
    pub status: &'static str,
    pub why: Option<String>,
    pub next_step: Option<String>,
    pub scope: &'static str,
}

pub(crate) fn manifest_records(data: &[ArrangementData]) -> Vec<EnsembleRecord> {
    data.iter()
        .filter(|d| d.input.history.is_some() || !matches!(d.input.ensemble, EnsembleInput::None))
        .map(|d| {
            let mut record = EnsembleRecord {
                arrangement: d.input.arrangement.id(),
                method: None,
                seed: None,
                samples_requested: None,
                samples_accepted: None,
                rejections: None,
                status: "not_calculated",
                why: None,
                next_step: None,
                scope: faris_engine::history_uncertainty::SCOPE_LINE,
            };
            match &d.input.ensemble {
                EnsembleInput::Ready(e) => {
                    record.method = Some(e.method.clone());
                    record.seed = Some(e.seed.to_string());
                    record.samples_requested = Some(e.samples_requested);
                    record.samples_accepted = Some(e.samples_accepted);
                    record.rejections = Some(e.rejections);
                    match not_evaluated_text(e) {
                        None => record.status = "evaluated",
                        Some((why, next)) => {
                            record.status = "not_evaluated";
                            record.why = Some(why.to_owned());
                            record.next_step = Some(next.to_owned());
                        }
                    }
                }
                EnsembleInput::Failed(why) => {
                    record.status = "failed";
                    record.why = Some(why.clone());
                }
                EnsembleInput::None => {
                    record.why = Some("No ensemble was calculated before the export.".into());
                    record.next_step =
                        Some("Wait for the uncertainty to finish, then export again.".into());
                }
            }
            record
        })
        .collect()
}

/// One of the four contrasts of the comparison view with the paired
/// comparison of its two ensembles, or why there is none.
#[derive(Clone, Debug)]
pub(crate) struct ContrastComparison {
    pub contrast: HistoryContrast,
    pub outcome: Result<EnsembleComparison, NotComparable>,
}

/// Why a contrast has no paired comparison, and what to do about it.
#[derive(Clone, Debug)]
pub(crate) struct NotComparable {
    pub why: String,
    pub next_step: String,
}

/// The paired comparison of each of the engine's `CONTRASTS`, the same four
/// pairs the compare view shows, second arrangement minus first. Uses only the
/// ensembles already in the input; none is calculated here. Empty when no
/// arrangement offered an ensemble (there is then no uncertainty section).
pub(crate) fn contrast_comparisons(data: &[ArrangementData]) -> Vec<ContrastComparison> {
    if data
        .iter()
        .all(|d| matches!(d.input.ensemble, EnsembleInput::None))
    {
        return Vec::new();
    }
    let find = |a: Arrangement| data.iter().find(|d| d.input.arrangement == a);
    CONTRASTS
        .iter()
        .map(|contrast| {
            let outcome = match (find(contrast.a), find(contrast.b)) {
                (Some(a), Some(b)) => compare_inputs(&a.input.ensemble, &b.input.ensemble),
                _ => Err(NotComparable {
                    why: "an arrangement of this pair is not recorded".into(),
                    next_step: "Record both arrangements, then export again".into(),
                }),
            };
            ContrastComparison {
                contrast: *contrast,
                outcome,
            }
        })
        .collect()
}

fn compare_inputs(
    a: &EnsembleInput,
    b: &EnsembleInput,
) -> Result<EnsembleComparison, NotComparable> {
    let missing = |why: String, next: &str| NotComparable {
        why,
        next_step: next.into(),
    };
    let ready = |e: &EnsembleInput| match e {
        EnsembleInput::Ready(e) => match not_evaluated_text(e) {
            None => Ok(e.clone()),
            Some((why, next)) => Err(missing(
                format!("an ensemble was not evaluated: {why}"),
                next,
            )),
        },
        EnsembleInput::Failed(why) => Err(missing(
            format!("an ensemble could not be calculated: {why}"),
            "Recalculate the study and export again",
        )),
        EnsembleInput::None => Err(missing(
            "an ensemble is missing".into(),
            "Wait for the uncertainty to finish in the desktop app, then export again",
        )),
    };
    let (a, b) = (ready(a)?, ready(b)?);
    compare_ensembles(&a, &b).map_err(|why| NotComparable {
        why,
        next_step: "Recalculate the ensembles from independent transport runs, then export again"
            .into(),
    })
}

/// The manifest's line for one contrast.
#[derive(Serialize)]
pub(crate) struct ComparisonRecord {
    pub contrast: &'static str,
    pub status: &'static str,
    pub method: Option<String>,
    pub pairs: Option<u32>,
    pub why: Option<String>,
    pub next_step: Option<String>,
    pub scope: &'static str,
}

pub(crate) fn comparison_records(comparisons: &[ContrastComparison]) -> Vec<ComparisonRecord> {
    comparisons
        .iter()
        .map(|c| {
            let mut record = ComparisonRecord {
                contrast: c.contrast.title,
                status: "compared",
                method: None,
                pairs: None,
                why: None,
                next_step: None,
                scope: faris_engine::history_uncertainty::SCOPE_LINE,
            };
            match &c.outcome {
                Ok(found) => {
                    record.method = Some(found.method.clone());
                    record.pairs = Some(found.pairs);
                }
                Err(n) => {
                    record.status = "not_compared";
                    record.why = Some(n.why.clone());
                    record.next_step = Some(n.next_step.clone());
                }
            }
            record
        })
        .collect()
}
