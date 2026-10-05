use faris_engine::history_uncertainty::EnsembleKey;
use faris_model::history::OperatingHistoryAssumptions;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const FORMAT: &str = "faris-study/1";
pub const MIMETYPE: &str = "application/vnd.avila-labs.faris-study";

/// The only blob encoding v1 writes: the exact bytes of the recorded file.
/// `array-f64-le` and `array-f32-le` are reserved for a later minor version; a
/// reader that meets any other encoding refuses the file.
pub const ENCODING_VERBATIM: &str = "verbatim";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobRecord {
    pub sha256: String,
    pub bytes: u64,
    pub media_type: String,
    /// Text, so an unknown encoding is reported by name rather than as a parse error.
    pub encoding: String,
}

/// Media type of a stored history ensemble blob (JSON of `HistoryEnsemble`).
pub const ENSEMBLE_MEDIA_TYPE: &str = "application/vnd.avila-labs.faris-history-ensemble+json";

/// One calculated history ensemble kept as a derived blob. The key is the full
/// identity of the calculation; a reader reuses the blob only when the key it
/// computes from the loaded inputs is equal to this one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnsembleRecord {
    /// Blob holding the ensemble as JSON.
    pub blob: String,
    /// Scenario and allocation the ensemble was calculated for (for people;
    /// the key alone decides reuse).
    pub scenario_sha256: String,
    pub variant: String,
    pub key: EnsembleKey,
}

/// One recorded-transport bundle: its schema version, notice, and a map from
/// member file name to the blob holding that file's exact bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleRecord {
    pub name: String,
    pub schema_version: String,
    pub notice: String,
    pub files: BTreeMap<String, String>,
    /// Whether the recorded JSON file ended with a newline, so unpacking can
    /// write the file back out as it was.
    #[serde(default)]
    pub trailing_newline: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArrangementRecord {
    #[serde(default)]
    pub scenario: Option<String>,
    #[serde(default)]
    pub physics: Vec<String>,
    #[serde(default)]
    pub bundles: Vec<BundleRecord>,
}

/// The two physical arrangements: with the outboard port and the matched control.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arrangements {
    #[serde(default)]
    pub port: Option<ArrangementRecord>,
    #[serde(default)]
    pub control: Option<ArrangementRecord>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArchiveKind {
    Case,
    Workspace,
}

/// One Core evidence archive: which arrangement and allocation it belongs to,
/// and the identity of its bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceArchive {
    pub arrangement: String,
    pub allocation: String,
    pub kind: ArchiveKind,
    pub file_name: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EvidenceMode {
    Packed,
    Referenced,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceLayer {
    pub mode: EvidenceMode,
    pub archives: Vec<EvidenceArchive>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layers {
    #[serde(default)]
    pub evidence: Option<EvidenceLayer>,
}

/// The view the author left open. Calculated histories are not stored; they
/// are recalculated when the study opens.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewState {
    pub step: String,
    pub preset: Option<String>,
    /// The what-if assumption values as edited, when assumptions are loaded.
    pub what_if: Option<OperatingHistoryAssumptions>,
    pub year: f64,
    pub field_view: String,
    pub history_tab: String,
    /// `port` or `control`.
    pub arrangement: String,
    /// Allocation (scenario variant) id shown.
    pub allocation: String,
    /// Allocation selected in the sweep, as blanket thickness in metres.
    pub sweep_blanket_m: Option<f64>,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            step: "design".into(),
            preset: None,
            what_if: None,
            year: 0.0,
            field_view: "materials".into(),
            history_tab: "magnet-fluence".into(),
            arrangement: "port".into(),
            allocation: String::new(),
            sweep_blanket_m: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    #[serde(default)]
    pub created_by: String,
    #[serde(default)]
    pub arrangements: Arrangements,
    #[serde(default)]
    pub sweep: Vec<BundleRecord>,
    /// Blob holding the operating-assumption file.
    #[serde(default)]
    pub assumptions: Option<String>,
    #[serde(default)]
    pub view: ViewState,
    #[serde(default)]
    pub layers: Layers,
    /// Derived history ensembles. Absent in files written before they existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ensembles: Vec<EnsembleRecord>,
    pub blobs: Vec<BlobRecord>,
}
