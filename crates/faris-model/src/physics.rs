//! Explicit material, source, and data inputs joined to one exact scenario.
//!
//! Validation checks record integrity and completeness. It is not a physical
//! qualification or a claim that an assumption reproduces an actual reactor.

use crate::{LoadedScenario, Reference};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const PHYSICS_CASE_VERSION: &str = "faris-physics-case/v0.1";
pub const MAX_MATERIALS: usize = 128;
pub const MAX_COMPONENT_ASSIGNMENTS: usize = 4096;
pub const MAX_NUCLIDES_PER_MATERIAL: usize = 256;
pub const MAX_DATA_FILES: usize = 1024;
pub const MAX_DATA_NUCLIDES_PER_FILE: usize = 16_384;
pub const MAX_DATA_TEMPERATURES_PER_FILE: usize = 256;
pub const MAX_REFERENCES: usize = 128;
pub const MAX_ASSUMPTIONS: usize = 256;
/// Metadata temperatures may use a rounded library label; this tolerance does
/// not imply interpolation or establish physical-temperature validity.
pub const NUCLEAR_DATA_TEMPERATURE_LABEL_TOLERANCE_K: f64 = 0.1;
const FRACTION_SUM_TOLERANCE: f64 = 1.0e-8;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PhysicsCase {
    pub schema_version: String,
    pub id: String,
    pub scenario_id: String,
    /// SHA-256 of the exact scenario file bytes, not a reserialized object.
    pub scenario_sha256: String,
    pub variant_id: String,
    pub materials: Vec<MaterialDefinition>,
    pub component_assignments: Vec<ComponentAssignment>,
    pub source: PhysicsSource,
    pub nuclear_data: NuclearDataSelection,
    pub scientific_scope: ScientificScope,
    pub assumptions: Vec<String>,
    pub references: Vec<Reference>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaterialDefinition {
    pub id: String,
    pub recipe: MaterialRecipe,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MaterialRecipe {
    NuclideMixture {
        nuclides: Vec<NuclideAtomFraction>,
        density_kg_m3: f64,
        /// `None` means the physical material temperature is not specified.
        /// It is only treated as complete for an explicit cold-data numerical reference.
        material_temperature_k: Option<f64>,
        nuclear_data_temperature_k: f64,
        provenance: InputProvenance,
    },
    /// Explicit geometric void. This is not a material with an inferred density.
    Void { provenance: InputProvenance },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NuclideAtomFraction {
    /// OpenMC nuclide name, e.g. `Li6`, `Fe56`, or `Am242_m1`.
    pub nuclide: String,
    pub atom_fraction: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "origin", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputProvenance {
    AuthoredAssumption {
        description: String,
    },
    LiteratureInput {
        reference_id: String,
        locator: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ComponentAssignment {
    pub component_id: String,
    pub material_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PhysicsSource {
    /// Total D-T reaction energy used for fusion-power normalization, in eV.
    pub energy_per_reaction_ev: f64,
    /// Primary neutron energy for the monoenergetic source, in eV.
    pub neutron_energy_ev: f64,
    /// This first source idealization requires exactly one neutron per reaction.
    pub neutrons_per_reaction: f64,
    pub energy_distribution: EnergyDistribution,
    pub spatial_distribution: SpatialDistribution,
    pub angular_distribution: AngularDistribution,
    pub provenance: InputProvenance,
}

impl PhysicsSource {
    /// Map the declared source to the transport normalization contract.
    pub fn to_transport_source(&self) -> crate::transport::DtSource {
        crate::transport::DtSource {
            energy_per_reaction_ev: self.energy_per_reaction_ev,
            neutron_energy_ev: self.neutron_energy_ev,
            neutrons_per_reaction: self.neutrons_per_reaction,
            distribution_id: "idealized-uniform-circular-plasma-torus-isotropic-monoenergetic/v1"
                .into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScientificScope {
    NumericalReference {
        reference_kind: NumericalReferenceKind,
        description: String,
    },
    ConditionalDesignPrediction {
        description: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NumericalReferenceKind {
    ColdDataNumericalReference,
    MathematicalControl,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EnergyDistribution {
    Monoenergetic,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SpatialDistribution {
    /// Uniform sampling by volume in the scenario's circular plasma torus.
    UniformCircularPlasmaTorus,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AngularDistribution {
    Isotropic,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum NuclearDataSelection {
    Unselected,
    Inventory {
        id: String,
        name: String,
        version: String,
        files: Vec<NuclearDataFile>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NuclearDataFile {
    pub file_id: String,
    /// Safe path relative to the selected library root.
    pub relative_path: String,
    /// SHA-256 of the exact installed file contents.
    pub sha256: String,
    pub size_bytes: u64,
    pub temperatures_k: Vec<f64>,
    pub nuclides: Vec<String>,
    pub capabilities: Vec<NuclearDataCapability>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum NuclearDataCapability {
    ContinuousEnergyNeutronTransport,
    Heating,
    PhotonTransport,
    AtomicRelaxation,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessCode {
    MaterialTemperatureUnspecified,
    DataNotSelected,
    DataInventoryEmpty,
    MissingNuclideData,
    MissingDataTemperature,
    MissingNeutronTransportCapability,
    MissingHeatingCapability,
    MissingPhotonTransportCapability,
    MissingAtomicRelaxationCapability,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReadinessDiagnostic {
    pub code: ReadinessCode,
    pub subject_id: Option<String>,
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PhysicsCaseError {
    #[error("invalid physics case: {0}")]
    Invalid(String),
}

impl PhysicsCase {
    /// Check typed inputs and bind them to the selected variant's geometry.
    /// Success does not qualify the assumptions or the model physically.
    pub fn validate_against(&self, loaded: &LoadedScenario) -> Result<(), PhysicsCaseError> {
        let fail = |message: String| PhysicsCaseError::Invalid(message);
        loaded
            .validate_identity()
            .map_err(|error| fail(error.to_string()))?;
        let scenario = &loaded.scenario;
        scenario.validate().map_err(|e| fail(e.to_string()))?;
        if self.schema_version != PHYSICS_CASE_VERSION {
            return Err(fail(format!(
                "supported schema_version is {PHYSICS_CASE_VERSION}"
            )));
        }
        for (value, name) in [
            (&self.id, "id"),
            (&self.scenario_id, "scenario_id"),
            (&self.scenario_sha256, "scenario_sha256"),
            (&self.variant_id, "variant_id"),
        ] {
            text(value, name)?;
        }
        match &self.scientific_scope {
            ScientificScope::NumericalReference { description, .. }
            | ScientificScope::ConditionalDesignPrediction { description } => {
                text(description, "scientific_scope.description")?;
                bounded_text(description, 4096, "scientific_scope.description")?;
            }
        }
        if self.scenario_id != scenario.id || self.scenario_sha256 != loaded.source_sha256 {
            return Err(fail(
                "physics case does not identify these exact scenario bytes".into(),
            ));
        }
        let variant = scenario
            .variants
            .iter()
            .find(|v| v.id == self.variant_id)
            .ok_or_else(|| fail("variant_id is absent from scenario".into()))?;
        if self.materials.is_empty() || self.materials.len() > MAX_MATERIALS {
            return Err(fail(format!(
                "materials must contain 1 to {MAX_MATERIALS} definitions"
            )));
        }
        if variant.layers.is_empty() || variant.layers.len() > MAX_COMPONENT_ASSIGNMENTS {
            return Err(fail(
                "selected variant component count is outside the supported bound".into(),
            ));
        }
        if self.component_assignments.len() != variant.layers.len() {
            return Err(fail(
                "component_assignments must assign every selected-variant component exactly once"
                    .into(),
            ));
        }
        let mut materials = BTreeMap::new();
        for material in &self.materials {
            text(&material.id, "material.id")?;
            if materials
                .insert(material.id.as_str(), &material.recipe)
                .is_some()
            {
                return Err(fail(format!("duplicate material id: {}", material.id)));
            }
            if material.id == "void" && !matches!(&material.recipe, MaterialRecipe::Void { .. }) {
                return Err(fail(
                    "material id `void` must use the explicit void recipe".into(),
                ));
            }
            match &material.recipe {
                MaterialRecipe::NuclideMixture {
                    nuclides,
                    density_kg_m3,
                    material_temperature_k,
                    nuclear_data_temperature_k,
                    provenance,
                } => {
                    positive_bounded(*density_kg_m3, 1.0e9, "density_kg_m3")?;
                    if let Some(temperature) = material_temperature_k {
                        positive_bounded(*temperature, 1.0e5, "material_temperature_k")?;
                    }
                    positive_bounded(
                        *nuclear_data_temperature_k,
                        1.0e5,
                        "nuclear_data_temperature_k",
                    )?;
                    if nuclides.is_empty() || nuclides.len() > MAX_NUCLIDES_PER_MATERIAL {
                        return Err(fail(format!(
                            "material {} must contain 1 to {MAX_NUCLIDES_PER_MATERIAL} nuclides",
                            material.id
                        )));
                    }
                    let mut names = BTreeSet::new();
                    let mut sum = 0.0;
                    for component in nuclides {
                        if !valid_nuclide_id(&component.nuclide) {
                            return Err(fail(format!(
                                "invalid OpenMC nuclide id: {}",
                                component.nuclide
                            )));
                        }
                        if !names.insert(&component.nuclide) {
                            return Err(fail(format!(
                                "duplicate nuclide {} in material {}",
                                component.nuclide, material.id
                            )));
                        }
                        positive_bounded(component.atom_fraction, 1.0, "atom_fraction")?;
                        sum += component.atom_fraction;
                    }
                    if !sum.is_finite() || (sum - 1.0).abs() > FRACTION_SUM_TOLERANCE {
                        return Err(fail(format!(
                            "atom fractions for {} must sum to 1 within {FRACTION_SUM_TOLERANCE}",
                            material.id
                        )));
                    }
                    validate_provenance(provenance, &self.references)?;
                }
                MaterialRecipe::Void { provenance } => {
                    validate_provenance(provenance, &self.references)?
                }
            }
        }
        let mut assigned_components = BTreeSet::new();
        let mut used_materials = BTreeSet::new();
        for assignment in &self.component_assignments {
            text(
                &assignment.component_id,
                "component_assignment.component_id",
            )?;
            text(&assignment.material_id, "component_assignment.material_id")?;
            if !assigned_components.insert(assignment.component_id.as_str()) {
                return Err(fail(format!(
                    "duplicate component assignment: {}",
                    assignment.component_id
                )));
            }
            let layer = variant
                .layers
                .iter()
                .find(|layer| layer.id == assignment.component_id)
                .ok_or_else(|| {
                    fail(format!(
                        "assignment names unknown component {}",
                        assignment.component_id
                    ))
                })?;
            if layer.material_id != assignment.material_id {
                return Err(fail(format!(
                    "component {} assignment material_id must match scenario material_id {}",
                    layer.id, layer.material_id
                )));
            }
            if !materials.contains_key(assignment.material_id.as_str()) {
                return Err(fail(format!(
                    "assignment for {} references undefined material {}",
                    assignment.component_id, assignment.material_id
                )));
            }
            used_materials.insert(assignment.material_id.as_str());
        }
        if assigned_components.len() != variant.layers.len() {
            return Err(fail(
                "one or more selected-variant components have no material assignment".into(),
            ));
        }
        if used_materials.len() != materials.len() {
            return Err(fail(
                "every material definition must be used by an assignment".into(),
            ));
        }
        validate_source(&self.source, &self.references)?;
        validate_references(&self.references)?;
        if self.assumptions.is_empty() || self.assumptions.len() > MAX_ASSUMPTIONS {
            return Err(fail(format!(
                "assumptions must contain 1 to {MAX_ASSUMPTIONS} entries"
            )));
        }
        for assumption in &self.assumptions {
            text(assumption, "assumption")?;
            bounded_text(assumption, 4096, "assumption")?;
        }
        validate_data_selection(&self.nuclear_data)?;
        Ok(())
    }

    /// Report declared data gaps separately from input validity and physics qualification.
    pub fn readiness_diagnostics(
        &self,
        loaded: &LoadedScenario,
    ) -> Result<Vec<ReadinessDiagnostic>, PhysicsCaseError> {
        self.validate_against(loaded)?;
        let mut diagnostics = Vec::new();
        let is_cold_data_reference = matches!(
            self.scientific_scope,
            ScientificScope::NumericalReference {
                reference_kind: NumericalReferenceKind::ColdDataNumericalReference,
                ..
            }
        );
        let materials: BTreeMap<_, _> = self
            .materials
            .iter()
            .map(|m| (m.id.as_str(), &m.recipe))
            .collect();
        for assignment in &self.component_assignments {
            if let MaterialRecipe::NuclideMixture {
                material_temperature_k: None,
                ..
            } = materials[assignment.material_id.as_str()]
                && !is_cold_data_reference
            {
                diagnostics.push(diagnostic(
                    ReadinessCode::MaterialTemperatureUnspecified,
                    Some(assignment.material_id.clone()),
                    "Physical material temperature is unspecified.",
                ));
            }
        }
        let files = match &self.nuclear_data {
            NuclearDataSelection::Unselected => {
                diagnostics.push(diagnostic(
                    ReadinessCode::DataNotSelected,
                    None,
                    "No evaluated nuclear-data files are selected.",
                ));
                &[][..]
            }
            NuclearDataSelection::Inventory { files, .. } if files.is_empty() => {
                diagnostics.push(diagnostic(
                    ReadinessCode::DataInventoryEmpty,
                    None,
                    "The selected nuclear-data inventory contains no files.",
                ));
                files.as_slice()
            }
            NuclearDataSelection::Inventory { files, .. } => files.as_slice(),
        };
        for assignment in &self.component_assignments {
            let MaterialRecipe::NuclideMixture {
                nuclides,
                nuclear_data_temperature_k,
                ..
            } = materials[assignment.material_id.as_str()]
            else {
                continue;
            };
            let neutron_files: Vec<_> = files
                .iter()
                .filter(|f| {
                    f.capabilities
                        .contains(&NuclearDataCapability::ContinuousEnergyNeutronTransport)
                })
                .collect();
            if neutron_files.is_empty() {
                diagnostics.push(diagnostic(
                    ReadinessCode::MissingNeutronTransportCapability,
                    Some(assignment.material_id.clone()),
                    "No inventory file declares continuous-energy neutron transport capability.",
                ));
            }
            for (capability, code, label) in [
                (
                    NuclearDataCapability::Heating,
                    ReadinessCode::MissingHeatingCapability,
                    "MT=301 heating",
                ),
                (
                    NuclearDataCapability::PhotonTransport,
                    ReadinessCode::MissingPhotonTransportCapability,
                    "photon transport",
                ),
                (
                    NuclearDataCapability::AtomicRelaxation,
                    ReadinessCode::MissingAtomicRelaxationCapability,
                    "populated atomic-relaxation cascades",
                ),
            ] {
                let missing: Vec<_> = nuclides
                    .iter()
                    .filter(|n| {
                        !files.iter().any(|f| {
                            f.capabilities.contains(&capability) && f.nuclides.contains(&n.nuclide)
                        })
                    })
                    .map(|n| n.nuclide.as_str())
                    .collect();
                if !missing.is_empty() {
                    diagnostics.push(diagnostic(
                        code,
                        Some(assignment.material_id.clone()),
                        format!(
                            "No selected file declares {label} data for: {}.",
                            missing.join(", ")
                        ),
                    ));
                }
            }
            let missing_nuclides: Vec<_> = nuclides
                .iter()
                .filter(|n| {
                    !neutron_files
                        .iter()
                        .any(|f| f.nuclides.contains(&n.nuclide))
                })
                .map(|n| n.nuclide.clone())
                .collect();
            if !missing_nuclides.is_empty() {
                diagnostics.push(diagnostic(
                    ReadinessCode::MissingNuclideData,
                    Some(assignment.material_id.clone()),
                    format!(
                        "No inventory file declares data for: {}.",
                        missing_nuclides.join(", ")
                    ),
                ));
            }
            let missing_temperature_nuclides: Vec<_> = nuclides
                .iter()
                .filter(|n| {
                    !neutron_files.iter().any(|f| {
                        f.nuclides.contains(&n.nuclide)
                            && f.temperatures_k.iter().any(|t| {
                                (*t - *nuclear_data_temperature_k).abs()
                                    <= NUCLEAR_DATA_TEMPERATURE_LABEL_TOLERANCE_K
                            })
                    })
                })
                .map(|n| n.nuclide.clone())
                .collect();
            if !missing_temperature_nuclides.is_empty() {
                diagnostics.push(diagnostic(
                    ReadinessCode::MissingDataTemperature,
                    Some(assignment.material_id.clone()),
                    format!(
                        "No neutron-data file declares a temperature within {} K of the requested {} K for: {}.",
                        NUCLEAR_DATA_TEMPERATURE_LABEL_TOLERANCE_K,
                        nuclear_data_temperature_k,
                        missing_temperature_nuclides.join(", ")
                    ),
                ));
            }
        }
        diagnostics.sort_by(|a, b| {
            (a.code, &a.subject_id, &a.message).cmp(&(b.code, &b.subject_id, &b.message))
        });
        diagnostics.dedup();
        Ok(diagnostics)
    }
}

fn validate_data_selection(selection: &NuclearDataSelection) -> Result<(), PhysicsCaseError> {
    let NuclearDataSelection::Inventory {
        id,
        name,
        version,
        files,
    } = selection
    else {
        return Ok(());
    };
    text(id, "nuclear_data.id")?;
    text(name, "nuclear_data.name")?;
    text(version, "nuclear_data.version")?;
    if files.len() > MAX_DATA_FILES {
        return Err(invalid(format!(
            "nuclear-data file count exceeds {MAX_DATA_FILES}"
        )));
    }
    let mut ids = BTreeSet::new();
    for file in files {
        text(&file.file_id, "nuclear_data.file_id")?;
        validate_relative_path(&file.relative_path)?;
        if !ids.insert(&file.file_id) {
            return Err(invalid(format!(
                "duplicate nuclear-data file_id: {}",
                file.file_id
            )));
        }
        validate_sha256(&file.sha256, "nuclear_data.file.sha256")?;
        if file.size_bytes == 0 {
            return Err(invalid(format!(
                "nuclear-data file {} size_bytes must be positive",
                file.file_id
            )));
        }
        if file
            .temperatures_k
            .iter()
            .any(|t| !t.is_finite() || *t <= 0.0 || *t > 1.0e5)
        {
            return Err(invalid(format!(
                "nuclear-data file {} has invalid temperature",
                file.file_id
            )));
        }
        if file.temperatures_k.len() > MAX_DATA_TEMPERATURES_PER_FILE {
            return Err(invalid(format!(
                "nuclear-data file {} has too many temperature entries",
                file.file_id
            )));
        }
        let mut temperatures = BTreeSet::new();
        for t in &file.temperatures_k {
            if !temperatures.insert(t.to_bits()) {
                return Err(invalid(format!(
                    "nuclear-data file {} repeats a temperature",
                    file.file_id
                )));
            }
        }
        if file.nuclides.len() > MAX_DATA_NUCLIDES_PER_FILE {
            return Err(invalid(format!(
                "nuclear-data file {} has too many nuclide entries",
                file.file_id
            )));
        }
        let mut nuclides = BTreeSet::new();
        for nuclide in &file.nuclides {
            if !valid_nuclide_id(nuclide) {
                return Err(invalid(format!(
                    "invalid data inventory nuclide id: {nuclide}"
                )));
            }
            if !nuclides.insert(nuclide) {
                return Err(invalid(format!(
                    "nuclear-data file {} repeats nuclide {nuclide}",
                    file.file_id
                )));
            }
        }
        let mut capabilities = BTreeSet::new();
        for capability in &file.capabilities {
            if !capabilities.insert(*capability) {
                return Err(invalid(format!(
                    "nuclear-data file {} repeats a capability",
                    file.file_id
                )));
            }
        }
    }
    Ok(())
}

fn validate_source(
    source: &PhysicsSource,
    references: &[Reference],
) -> Result<(), PhysicsCaseError> {
    positive_bounded(
        source.energy_per_reaction_ev,
        1.0e9,
        "source.energy_per_reaction_ev",
    )?;
    positive_bounded(source.neutron_energy_ev, 1.0e9, "source.neutron_energy_ev")?;
    if source.neutron_energy_ev > source.energy_per_reaction_ev {
        return Err(invalid(
            "source neutron energy exceeds total reaction energy",
        ));
    }
    if !source.neutrons_per_reaction.is_finite() || source.neutrons_per_reaction != 1.0 {
        return Err(invalid(
            "D-T source neutron yield must be exactly one per reaction",
        ));
    }
    validate_provenance(&source.provenance, references)
}

fn validate_relative_path(value: &str) -> Result<(), PhysicsCaseError> {
    text(value, "nuclear_data.file.relative_path")?;
    if value.contains('\\')
        || value.contains(':')
        || value.starts_with('/')
        || std::path::Path::new(value)
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(invalid(
            "nuclear-data file path must be a normalized library-relative path",
        ));
    }
    Ok(())
}

fn validate_references(references: &[Reference]) -> Result<(), PhysicsCaseError> {
    if references.len() > MAX_REFERENCES {
        return Err(invalid(format!("references exceed {MAX_REFERENCES}")));
    }
    let mut ids = BTreeSet::new();
    for r in references {
        for (value, name) in [
            (&r.id, "reference.id"),
            (&r.title, "reference.title"),
            (&r.url, "reference.url"),
            (&r.r#use, "reference.use"),
        ] {
            text(value, name)?;
        }
        if !ids.insert(&r.id) {
            return Err(invalid(format!("duplicate reference id: {}", r.id)));
        }
        if !(r.url.starts_with("https://") || r.url.starts_with("http://")) {
            return Err(invalid(format!(
                "reference {} URL must use http or https",
                r.id
            )));
        }
    }
    Ok(())
}

fn validate_provenance(
    provenance: &InputProvenance,
    references: &[Reference],
) -> Result<(), PhysicsCaseError> {
    match provenance {
        InputProvenance::AuthoredAssumption { description } => {
            text(description, "provenance.description")?;
            bounded_text(description, 4096, "provenance.description")?;
        }
        InputProvenance::LiteratureInput {
            reference_id,
            locator,
        } => {
            text(reference_id, "provenance.reference_id")?;
            text(locator, "provenance.locator")?;
            bounded_text(locator, 512, "provenance.locator")?;
            if !references.iter().any(|r| r.id == *reference_id) {
                return Err(invalid(format!(
                    "provenance references unknown reference_id {reference_id}"
                )));
            }
        }
    }
    Ok(())
}

fn valid_nuclide_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    if bytes.len() < 2 {
        return false;
    }
    let mut index = 0;
    if !bytes[index].is_ascii_uppercase() {
        return false;
    }
    index += 1;
    if index < bytes.len() && bytes[index].is_ascii_lowercase() {
        index += 1;
    }
    let mass_start = index;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    if index == mass_start
        || index - mass_start > 3
        || id[mass_start..index]
            .parse::<u16>()
            .map_or(true, |m| m == 0 || m > 350)
    {
        return false;
    }
    if index == bytes.len() {
        return true;
    }
    if !id[index..].starts_with("_m") {
        return false;
    }
    index += 2;
    let state_start = index;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    index == bytes.len()
        && index > state_start
        && id[state_start..index]
            .parse::<u8>()
            .is_ok_and(|n| n > 0 && n <= 9)
}

fn validate_sha256(value: &str, name: &str) -> Result<(), PhysicsCaseError> {
    let hex = value
        .strip_prefix("sha256:")
        .ok_or_else(|| invalid(format!("{name} must use sha256:<64 lowercase hex>")))?;
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid(format!(
            "{name} must use sha256:<64 lowercase hex>"
        )));
    }
    Ok(())
}

fn positive_bounded(value: f64, max: f64, name: &str) -> Result<(), PhysicsCaseError> {
    if value.is_finite() && value > 0.0 && value <= max {
        Ok(())
    } else {
        Err(invalid(format!("{name} must be finite and in (0, {max}]")))
    }
}
fn text(value: &str, name: &str) -> Result<(), PhysicsCaseError> {
    if value.trim().is_empty() {
        Err(invalid(format!("{name} must be nonempty")))
    } else {
        bounded_text(value, 4096, name)
    }
}
fn bounded_text(value: &str, max: usize, name: &str) -> Result<(), PhysicsCaseError> {
    if value.len() > max {
        Err(invalid(format!("{name} exceeds {max} bytes")))
    } else {
        Ok(())
    }
}
fn invalid(message: impl Into<String>) -> PhysicsCaseError {
    PhysicsCaseError::Invalid(message.into())
}
fn diagnostic(
    code: ReadinessCode,
    subject_id: Option<String>,
    message: impl Into<String>,
) -> ReadinessDiagnostic {
    ReadinessDiagnostic {
        code,
        subject_id,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCENARIO_BYTES: &[u8] = include_bytes!("../../../scenarios/arc-inspired/scenario.json");
    fn case() -> (PhysicsCase, LoadedScenario) {
        let loaded = LoadedScenario::from_bytes(SCENARIO_BYTES).unwrap();
        let scenario = &loaded.scenario;
        let variant = &scenario.variants[0];
        let materials = vec![
            MaterialDefinition {
                id: "first-wall-unassigned".into(),
                recipe: mixture("Fe56", 7800.0, Some(600.0), 600.0),
            },
            MaterialDefinition {
                id: "breeder-unassigned".into(),
                recipe: mixture("Li6", 2000.0, Some(600.0), 600.0),
            },
            MaterialDefinition {
                id: "shield-unassigned".into(),
                recipe: mixture("Fe56", 7800.0, Some(600.0), 600.0),
            },
            MaterialDefinition {
                id: "vessel-unassigned".into(),
                recipe: mixture("Fe56", 7800.0, Some(600.0), 600.0),
            },
            MaterialDefinition {
                id: "void".into(),
                recipe: MaterialRecipe::Void {
                    provenance: authored(),
                },
            },
            MaterialDefinition {
                id: "magnet-unassigned".into(),
                recipe: mixture("Nb93", 8500.0, Some(20.0), 293.6),
            },
        ];
        let component_assignments = variant
            .layers
            .iter()
            .map(|l| ComponentAssignment {
                component_id: l.id.clone(),
                material_id: l.material_id.clone(),
            })
            .collect();
        (
            PhysicsCase {
                schema_version: PHYSICS_CASE_VERSION.into(),
                id: "test-idealized-case".into(),
                scenario_id: scenario.id.clone(),
                scenario_sha256: loaded.source_sha256.clone(),
                variant_id: variant.id.clone(),
                materials,
                component_assignments,
                source: PhysicsSource {
                    energy_per_reaction_ev: 17.6e6,
                    neutron_energy_ev: 14.1e6,
                    neutrons_per_reaction: 1.0,
                    energy_distribution: EnergyDistribution::Monoenergetic,
                    spatial_distribution: SpatialDistribution::UniformCircularPlasmaTorus,
                    angular_distribution: AngularDistribution::Isotropic,
                    provenance: authored(),
                },
                nuclear_data: NuclearDataSelection::Unselected,
                scientific_scope: ScientificScope::NumericalReference {
                    reference_kind: NumericalReferenceKind::ColdDataNumericalReference,
                    description:
                        "Idealized cold-data circular plasma torus; not an actual ARC plasma."
                            .into(),
                },
                assumptions: vec![
                    "TEST FIXTURE ONLY: explicit synthetic compositions and temperatures.".into(),
                ],
                references: vec![],
            },
            loaded,
        )
    }
    fn authored() -> InputProvenance {
        InputProvenance::AuthoredAssumption {
            description: "TEST FIXTURE ONLY; no physical conclusion.".into(),
        }
    }
    fn mixture(
        nuclide: &str,
        density: f64,
        material_temperature: Option<f64>,
        data_temperature: f64,
    ) -> MaterialRecipe {
        MaterialRecipe::NuclideMixture {
            nuclides: vec![NuclideAtomFraction {
                nuclide: nuclide.into(),
                atom_fraction: 1.0,
            }],
            density_kg_m3: density,
            material_temperature_k: material_temperature,
            nuclear_data_temperature_k: data_temperature,
            provenance: authored(),
        }
    }
    fn data_file(nuclides: Vec<&str>, temperatures_k: Vec<f64>) -> NuclearDataFile {
        NuclearDataFile {
            file_id: "test-neutron-data".into(),
            relative_path: "neutron/test.h5".into(),
            sha256: format!("sha256:{}", "a".repeat(64)),
            size_bytes: 64,
            temperatures_k,
            nuclides: nuclides.into_iter().map(str::to_owned).collect(),
            capabilities: vec![
                NuclearDataCapability::ContinuousEnergyNeutronTransport,
                NuclearDataCapability::Heating,
                NuclearDataCapability::PhotonTransport,
                NuclearDataCapability::AtomicRelaxation,
            ],
        }
    }
    fn inventory(files: Vec<NuclearDataFile>) -> NuclearDataSelection {
        NuclearDataSelection::Inventory {
            id: "test-library".into(),
            name: "TEST ONLY".into(),
            version: "fixture".into(),
            files,
        }
    }

    // Verifies: GEO-040
    #[test]
    fn exact_scenario_assignment_and_explicit_void_are_required() {
        let (mut c, loaded) = case();
        c.validate_against(&loaded).unwrap();
        c.scenario_sha256.push('0');
        assert!(c.validate_against(&loaded).is_err());
        let (mut c, loaded) = case();
        c.component_assignments.pop();
        assert!(c.validate_against(&loaded).is_err());
        let (mut c, loaded) = case();
        c.materials[4].recipe = mixture("He4", 1.0, Some(293.0), 293.0);
        assert!(c.validate_against(&loaded).is_err());
        let (mut c, loaded) = case();
        c.materials[0].id = "unused-material".into();
        assert!(c.validate_against(&loaded).is_err());
    }

    // Verifies: GEO-043
    #[test]
    fn validates_composition_temperature_and_nuclide_syntax() {
        let (mut c, loaded) = case();
        if let MaterialRecipe::NuclideMixture { nuclides, .. } = &mut c.materials[0].recipe {
            nuclides[0].atom_fraction = 0.9;
        }
        assert!(c.validate_against(&loaded).is_err());
        let (mut c, loaded) = case();
        if let MaterialRecipe::NuclideMixture { nuclides, .. } = &mut c.materials[0].recipe {
            nuclides[0].nuclide = "FE56".into();
        }
        assert!(c.validate_against(&loaded).is_err());
        let (mut c, loaded) = case();
        if let MaterialRecipe::NuclideMixture {
            nuclear_data_temperature_k,
            ..
        } = &mut c.materials[0].recipe
        {
            *nuclear_data_temperature_k = f64::NAN;
        }
        assert!(c.validate_against(&loaded).is_err());
    }

    #[test]
    fn readiness_reports_missing_transport_facts_without_qualifying_physics() {
        let (mut c, loaded) = case();
        let diagnostics = c.readiness_diagnostics(&loaded).unwrap();
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == ReadinessCode::DataNotSelected)
        );
        c.nuclear_data = inventory(vec![data_file(vec!["Fe56"], vec![600.0, 293.6])]);
        let diagnostics = c.readiness_diagnostics(&loaded).unwrap();
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == ReadinessCode::MissingNuclideData
                    && d.subject_id.as_deref() == Some("breeder-unassigned"))
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == ReadinessCode::MissingDataTemperature
                    && d.subject_id.as_deref() == Some("magnet-unassigned"))
        );
        let mut neutron_only = data_file(vec!["Fe56", "Li6", "Nb93"], vec![600.0, 293.6]);
        neutron_only.capabilities = vec![NuclearDataCapability::ContinuousEnergyNeutronTransport];
        c.nuclear_data = inventory(vec![neutron_only]);
        let diagnostics = c.readiness_diagnostics(&loaded).unwrap();
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == ReadinessCode::MissingHeatingCapability)
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == ReadinessCode::MissingPhotonTransportCapability)
        );
    }

    #[test]
    fn missing_material_temperature_is_only_waived_for_cold_data_reference() {
        let (mut c, loaded) = case();
        if let MaterialRecipe::NuclideMixture {
            material_temperature_k,
            ..
        } = &mut c.materials[0].recipe
        {
            *material_temperature_k = None;
        }
        let diagnostics = c.readiness_diagnostics(&loaded).unwrap();
        assert!(
            !diagnostics
                .iter()
                .any(|d| d.code == ReadinessCode::MaterialTemperatureUnspecified)
        );
        c.scientific_scope = ScientificScope::ConditionalDesignPrediction {
            description: "conditional test case".into(),
        };
        let diagnostics = c.readiness_diagnostics(&loaded).unwrap();
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == ReadinessCode::MaterialTemperatureUnspecified)
        );
    }

    #[test]
    fn complete_inventory_clears_declared_transport_readiness_gaps() {
        let (mut c, loaded) = case();
        if let MaterialRecipe::NuclideMixture { nuclides, .. } = &mut c.materials[0].recipe {
            *nuclides = vec![
                NuclideAtomFraction {
                    nuclide: "Fe56".into(),
                    atom_fraction: 0.5,
                },
                NuclideAtomFraction {
                    nuclide: "C12".into(),
                    atom_fraction: 0.5,
                },
            ];
        }
        let mut carbon = data_file(vec!["C12"], vec![600.05]);
        carbon.file_id = "test-carbon-data".into();
        carbon.relative_path = "neutron/carbon.h5".into();
        c.nuclear_data = inventory(vec![
            data_file(vec!["Fe56", "Li6", "Nb93"], vec![600.0, 293.595]),
            carbon,
        ]);
        let diagnostics = c.readiness_diagnostics(&loaded).unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    // Verifies: GEO-041
    #[test]
    fn temperature_rounding_is_tolerated_but_distant_data_is_not() {
        let (mut c, loaded) = case();
        // 0.0253 eV corresponds to about 293.595 K; do not substitute its
        // human-readable rounded `294 K` label for the effective temperature.
        c.nuclear_data = inventory(vec![data_file(
            vec!["Fe56", "Li6", "Nb93"],
            vec![600.0, 293.595],
        )]);
        let diagnostics = c.readiness_diagnostics(&loaded).unwrap();
        assert!(!diagnostics.iter().any(|d| {
            d.code == ReadinessCode::MissingDataTemperature
                && d.subject_id.as_deref() == Some("magnet-unassigned")
        }));

        if let NuclearDataSelection::Inventory { files, .. } = &mut c.nuclear_data {
            files[0].temperatures_k[1] = 294.0;
        }
        let diagnostics = c.readiness_diagnostics(&loaded).unwrap();
        assert!(diagnostics.iter().any(|d| {
            d.code == ReadinessCode::MissingDataTemperature
                && d.subject_id.as_deref() == Some("magnet-unassigned")
        }));
    }

    #[test]
    fn source_helper_preserves_energy_and_has_stable_distribution_identity() {
        let (case, _) = case();
        let transport_source = case.source.to_transport_source();
        assert_eq!(
            transport_source.energy_per_reaction_ev,
            case.source.energy_per_reaction_ev
        );
        assert_eq!(
            transport_source.neutron_energy_ev,
            case.source.neutron_energy_ev
        );
        assert_eq!(transport_source.neutrons_per_reaction, 1.0);
        assert_eq!(
            transport_source.distribution_id,
            "idealized-uniform-circular-plasma-torus-isotropic-monoenergetic/v1"
        );
    }

    #[test]
    fn rejects_bad_data_identities_and_orphan_provenance() {
        let (mut c, loaded) = case();
        c.nuclear_data = inventory(vec![data_file(vec!["Fe56"], vec![600.0])]);
        if let NuclearDataSelection::Inventory { files, .. } = &mut c.nuclear_data {
            files[0].sha256 = "sha256:fake".into();
        }
        assert!(c.validate_against(&loaded).is_err());
        let (mut c, loaded) = case();
        c.nuclear_data = inventory(vec![data_file(vec!["Fe56"], vec![600.0])]);
        if let NuclearDataSelection::Inventory { files, .. } = &mut c.nuclear_data {
            files[0].relative_path = "../../outside.h5".into();
        }
        assert!(c.validate_against(&loaded).is_err());
        let (mut c, loaded) = case();
        if let MaterialRecipe::NuclideMixture { provenance, .. } = &mut c.materials[0].recipe {
            *provenance = InputProvenance::LiteratureInput {
                reference_id: "missing".into(),
                locator: "Table 1".into(),
            };
        }
        assert!(c.validate_against(&loaded).is_err());
    }
}
