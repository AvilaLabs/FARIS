//! The design file `faris-design/v0.1`: what a STEP file cannot say.
//!
//! A STEP file says where the solids are. The design file says what they are
//! made of, what job they do, when they wear out and where the plasma is. Types
//! here follow `docs/notes/DESIGN_FILE_FORMAT.md` key for key and refuse unknown
//! keys. A field the user must fill is `Option` only so that the draft written
//! by `design init` can hold `null`; the import stops at check 1 and lists each
//! one. Nothing is repaired or defaulted (GEO-027). Validation here is pure: the
//! STEP file is represented by the helper's inspection result.

use crate::Reference;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

mod catalog;
mod checks;
mod parse;

pub use catalog::{CatalogMaterial, catalog, catalog_material};
pub use checks::{
    ScenarioFacts, VoidSolid, check_2_static, check_2_unit, check_3, check_4, check_5,
    scenario_facts, void_solids,
};
pub use parse::{ParseOutcome, nullable_paths, parse_design};

pub const DESIGN_VERSION: &str = "faris-design/v0.1";
pub const INSPECT_SCHEMA: &str = "faris-step-inspect/v1";
/// DAGMC material tags are 32 bytes and `cad_to_dagmc` prefixes `mat:`.
pub const MAX_ID_LEN: usize = 28;
/// Ids the DAGMC writer or FARIS treats as special, in any letter case.
pub const RESERVED_IDS: [&str; 3] = ["vacuum", "graveyard", "void"];
pub const VOID: &str = "void";
/// Fingerprint volume tolerance, relative.
pub const VOLUME_TOLERANCE: f64 = 1e-6;
/// Fingerprint centroid tolerance, as a fraction of the model bounding-box diagonal.
pub const CENTROID_TOLERANCE: f64 = 1e-6;
pub const FRACTION_SUM_TOLERANCE: f64 = 1e-9;

/// A failed rule: the item, why it failed and the user's next step.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    pub check: u8,
    pub item: String,
    pub why: String,
    pub next_step: String,
}

impl Finding {
    pub fn new(
        check: u8,
        item: impl Into<String>,
        why: impl Into<String>,
        next_step: impl Into<String>,
    ) -> Self {
        Self {
            check,
            item: item.into(),
            why: why.into(),
            next_step: next_step.into(),
        }
    }
}

/// Information that does not stop the import.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Notice {
    pub check: u8,
    pub item: String,
    pub message: String,
}

impl Notice {
    pub fn new(check: u8, item: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            check,
            item: item.into(),
            message: message.into(),
        }
    }
}

/// A key that must be present but may be `null`.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Design {
    pub schema_version: String,
    pub id: String,
    pub title: String,
    pub description: String,
    pub cad: Cad,
    pub materials: Vec<DesignMaterial>,
    pub solids: Vec<Solid>,
    pub replacement_groups: Vec<ReplacementGroup>,
    pub plasma: Plasma,
    pub operating_scenario: OperatingScenarioLink,
    pub references: Vec<Reference>,
    pub assumptions: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LengthUnit {
    M,
    Cm,
    Mm,
}

impl LengthUnit {
    pub fn label(self) -> &'static str {
        match self {
            Self::M => "m",
            Self::Cm => "cm",
            Self::Mm => "mm",
        }
    }
    /// Exact metres per unit.
    pub fn metres(self) -> f64 {
        match self {
            Self::M => 1.0,
            Self::Cm => 1e-2,
            Self::Mm => 1e-3,
        }
    }
}

/// `extent`: only `{"kind": "full"}` is accepted by v0.1. Other kinds parse so
/// the import can stop with a reason instead of a schema error.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Extent {
    pub kind: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Cad {
    pub step_file: String,
    pub step_sha256: String,
    pub length_unit: LengthUnit,
    pub extent: Extent,
    /// `"z"` in v0.1; any other value stops the import with a reason.
    pub machine_axis: String,
    #[serde(deserialize_with = "present")]
    pub faceting_tolerance_m: Option<f64>,
    pub implicit_complement_material_id: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceLabel {
    Published,
    Authored,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub label: ProvenanceLabel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_id: Option<String>,
    pub note: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompositionBasis {
    Atom,
    Weight,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Component {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nuclide: Option<String>,
    pub fraction: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isotopes: Option<BTreeMap<String, f64>>,
}

/// Either `{id, catalog_id}` or a full recipe; the two shapes are told apart in
/// check 1 so that an unknown key still produces a precise error.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DesignMaterial {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub density_kg_m3: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composition_basis: Option<CompositionBasis>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub components: Option<Vec<Component>>,
    /// `null` or absent: the library's nearest stored temperature, as the demo does.
    #[serde(default)]
    pub temperature_k: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    PlasmaChamber,
    FirstWall,
    Divertor,
    Blanket,
    Multiplier,
    Shield,
    VacuumVessel,
    TfMagnet,
    PfMagnet,
    CsMagnet,
    Structure,
    PortPlug,
    Vacuum,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Self::PlasmaChamber => "plasma_chamber",
            Self::FirstWall => "first_wall",
            Self::Divertor => "divertor",
            Self::Blanket => "blanket",
            Self::Multiplier => "multiplier",
            Self::Shield => "shield",
            Self::VacuumVessel => "vacuum_vessel",
            Self::TfMagnet => "tf_magnet",
            Self::PfMagnet => "pf_magnet",
            Self::CsMagnet => "cs_magnet",
            Self::Structure => "structure",
            Self::PortPlug => "port_plug",
            Self::Vacuum => "vacuum",
        }
    }
    /// Roles whose material may be `void`.
    pub fn allows_void(self) -> bool {
        matches!(self, Self::PlasmaChamber | Self::Vacuum | Self::PortPlug)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Fingerprint {
    pub cad_volume_m3: f64,
    pub centroid_m: [f64; 3],
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Solid {
    pub id: String,
    pub step_index: usize,
    #[serde(deserialize_with = "present")]
    pub step_name: Option<String>,
    pub fingerprint: Fingerprint,
    #[serde(deserialize_with = "present")]
    pub material_id: Option<String>,
    #[serde(deserialize_with = "present")]
    pub role: Option<Role>,
    #[serde(deserialize_with = "present")]
    pub replacement_group_id: Option<String>,
    #[serde(default = "default_true")]
    pub tally: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    FastNeutronFluence,
    NeutronFluence,
    DisplacementsPerAtom,
    HeliumAppm,
    NuclearHeatingDensity,
}

impl Metric {
    pub fn label(self) -> &'static str {
        match self {
            Self::FastNeutronFluence => "fast_neutron_fluence",
            Self::NeutronFluence => "neutron_fluence",
            Self::DisplacementsPerAtom => "displacements_per_atom",
            Self::HeliumAppm => "helium_appm",
            Self::NuclearHeatingDensity => "nuclear_heating_density",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Averaging {
    GroupAverage,
    /// `[dr, dz, dphi]`; `null` means the full solid width in that direction.
    PeakMesh {
        voxel_m: Vec<Option<f64>>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Limit {
    pub metric: Metric,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub energy_threshold_ev: Option<f64>,
    pub limit: f64,
    pub averaging: Averaging,
    pub provenance: Provenance,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReplacementDuration {
    Fixed {
        duration_s: f64,
        provenance: Provenance,
    },
    /// Until the S3 dose rule exists this is NOT_EVALUATED.
    Computed,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReplacementGroup {
    pub id: String,
    pub label: String,
    pub limits: Vec<Limit>,
    pub replacement_duration: ReplacementDuration,
}

/// A radial profile such as ion density; each value is filled by the user.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    #[serde(deserialize_with = "present")]
    pub centre: Option<f64>,
    #[serde(deserialize_with = "present")]
    pub pedestal: Option<f64>,
    #[serde(deserialize_with = "present")]
    pub separatrix: Option<f64>,
    #[serde(deserialize_with = "present")]
    pub peaking_factor: Option<f64>,
}

/// The ion temperature profile also carries `beta`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TemperatureProfile {
    #[serde(deserialize_with = "present")]
    pub centre: Option<f64>,
    #[serde(deserialize_with = "present")]
    pub pedestal: Option<f64>,
    #[serde(deserialize_with = "present")]
    pub separatrix: Option<f64>,
    #[serde(deserialize_with = "present")]
    pub peaking_factor: Option<f64>,
    #[serde(deserialize_with = "present")]
    pub beta: Option<f64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum PlasmaMode {
    L,
    H,
    A,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EmissivityTable {
    pub path: String,
    pub sha256: String,
}

/// Field names follow `openmc_plasma_source.tokamak_source` (0.9.0), in SI.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Plasma {
    #[serde(deserialize_with = "present")]
    pub chamber_solid_id: Option<String>,
    #[serde(deserialize_with = "present")]
    pub major_radius_m: Option<f64>,
    #[serde(deserialize_with = "present")]
    pub minor_radius_m: Option<f64>,
    #[serde(deserialize_with = "present")]
    pub elongation: Option<f64>,
    #[serde(deserialize_with = "present")]
    pub triangularity: Option<f64>,
    #[serde(deserialize_with = "present")]
    pub shafranov_shift_m: Option<f64>,
    #[serde(deserialize_with = "present")]
    pub pedestal_radius_m: Option<f64>,
    #[serde(deserialize_with = "present")]
    pub mode: Option<PlasmaMode>,
    pub ion_density_m3: Profile,
    pub ion_temperature_ev: TemperatureProfile,
    #[serde(deserialize_with = "present")]
    pub fuel: Option<BTreeMap<String, f64>>,
    #[serde(deserialize_with = "present")]
    pub fusion_power_mw: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emissivity_table: Option<EmissivityTable>,
    #[serde(deserialize_with = "present")]
    pub provenance: Option<Provenance>,
    pub field_provenance: BTreeMap<String, Provenance>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OperatingScenarioLink {
    #[serde(deserialize_with = "present")]
    pub path: Option<String>,
    #[serde(deserialize_with = "present")]
    pub sha256: Option<String>,
}

// ---------------------------------------------------------------------------
// The helper's inspection result.

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct InspectVersions {
    pub python: String,
    pub interpreter: String,
    pub cadquery: Option<String>,
    pub ocp: Option<String>,
    pub occt: Option<String>,
    pub helper: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct InspectStep {
    pub file_name: String,
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct InspectUnit {
    /// `mm`, `cm`, `m`, `inch`, or `<factor> m`; `None` when the file declares none.
    pub declared: Option<String>,
    pub factor_to_m: Option<f64>,
    /// The unit the SI numbers were computed with (declared or assumed).
    pub used: Option<String>,
    pub used_is_assumed: bool,
    pub text_units: Vec<String>,
    pub ocp_units: Vec<String>,
    pub problems: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct InspectSolid {
    pub step_index: usize,
    pub step_name: Option<String>,
    pub cad_volume_m3: Option<f64>,
    pub centroid_m: Option<[f64; 3]>,
    pub bbox_min_m: Option<[f64; 3]>,
    pub bbox_max_m: Option<[f64; 3]>,
    pub occt_valid: bool,
    pub gprop_relative_error_estimate: f64,
}

/// What `faris_cad.py step-inspect` reports about a STEP file.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct StepInspection {
    pub schema: String,
    pub versions: InspectVersions,
    pub step: InspectStep,
    pub unit: InspectUnit,
    pub solids: Vec<InspectSolid>,
    pub model_bbox_diagonal_m: Option<f64>,
    pub volume_method: String,
    pub problems: Vec<String>,
}

impl StepInspection {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let inspection: Self = serde_json::from_slice(bytes)
            .map_err(|error| format!("the CAD helper's report could not be read: {error}"))?;
        if inspection.schema != INSPECT_SCHEMA {
            return Err(format!(
                "the CAD helper's report has schema {:?}; this FARIS reads {INSPECT_SCHEMA}",
                inspection.schema
            ));
        }
        Ok(inspection)
    }
}

/// `true` for an id matching `^[a-z0-9][a-z0-9-]{0,27}$`.
pub fn valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_ID_LEN
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

pub fn is_reserved(id: &str) -> bool {
    RESERVED_IDS.iter().any(|r| id.eq_ignore_ascii_case(r))
}

/// Suggested id for a solid: the STEP name lower-cased and made safe, else
/// `solid-NNN`; unique among `taken` within 28 characters. Never a reserved id.
pub fn suggest_id(step_name: Option<&str>, step_index: usize, taken: &[String]) -> String {
    let mut base = String::new();
    if let Some(name) = step_name {
        let mut previous_dash = true; // trims a leading dash
        for ch in name.to_lowercase().chars() {
            if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
                base.push(ch);
                previous_dash = false;
            } else if !previous_dash {
                base.push('-');
                previous_dash = true;
            }
        }
        while base.ends_with('-') {
            base.pop();
        }
        base.truncate(MAX_ID_LEN);
        while base.ends_with('-') {
            base.pop();
        }
    }
    if base.is_empty() || !valid_id(&base) || is_reserved(&base) {
        base = format!("solid-{step_index:03}");
    }
    let mut candidate = base.clone();
    let mut n = 2;
    while taken.iter().any(|t| *t == candidate) || is_reserved(&candidate) {
        let suffix = format!("-{n}");
        let keep = MAX_ID_LEN - suffix.len();
        let mut stem: String = base.chars().take(keep).collect();
        while stem.ends_with('-') {
            stem.pop();
        }
        candidate = format!("{stem}{suffix}");
        n += 1;
    }
    candidate
}

impl Design {
    pub fn solid(&self, id: &str) -> Option<&Solid> {
        self.solids.iter().find(|s| s.id == id)
    }
}

#[cfg(test)]
mod tests;
