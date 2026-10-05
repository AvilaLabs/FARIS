//! Audited cold-data transport execution. Rust owns identity checks, requested
//! responses and normalization; Python only prepares and runs OpenMC.

use crate::{
    DemoManifest, build_manifest,
    jobs::{Cancellation, ExecutionStatus, JobResult, JobSpec, ResourceLimits, run_job},
    transport::{
        DomainVolume, MAX_TRANSPORT_ARTIFACT_BYTES, NormalizedEnergySpectrum, NormalizedTally,
        NormalizedTransportResult, PhysicalUnit, RawTally, RawTallyUnit, RawTransportSpectra,
        TallyEstimator, ToolIdentity, TransportArtifact, VolumeUnit, normalize_spectra,
        normalize_transport_artifact,
    },
};
use faris_model::{
    LoadedScenario,
    physics::{
        MaterialRecipe, NuclearDataCapability, NuclearDataFile, NuclearDataSelection, PhysicsCase,
        ScientificScope,
    },
    transport::{
        HeatingConvention, HeatingParticleScope, ProducedParticle, ResponseDefinition,
        ResponseDomain, ScoreDefinition, TRANSPORT_ARTIFACT_VERSION,
        TRANSPORT_REQUEST_LEGACY_VERSION, TRANSPORT_REQUEST_VERSION, TransportRequest,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

pub type ReactorError = Box<dyn std::error::Error + Send + Sync>;
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_JSON_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FieldMesh {
    pub id: String,
    pub dimensions: [usize; 3],
    pub lower_left_m: [f64; 3],
    pub upper_right_m: [f64; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshPreset {
    Coarse,
    OutboardLocalCoarse,
    OutboardLocal,
    OutboardPortWindow,
}

impl MeshPreset {
    pub fn from_cli(value: &str) -> Result<Self, ReactorError> {
        match value {
            "coarse" => Ok(Self::Coarse),
            "outboard-local-coarse" => Ok(Self::OutboardLocalCoarse),
            "outboard-local" => Ok(Self::OutboardLocal),
            "outboard-port-window" => Ok(Self::OutboardPortWindow),
            _ => Err("mesh preset must be coarse, outboard-local-coarse, outboard-local, or outboard-port-window".into()),
        }
    }
}

impl FieldMesh {
    pub fn bins(&self) -> usize {
        self.dimensions.iter().product()
    }
    pub fn validate(&self, manifest: &DemoManifest) -> Result<(), ReactorError> {
        let count = self
            .dimensions
            .iter()
            .try_fold(1usize, |n, d| n.checked_mul(*d))
            .ok_or("mesh bin count overflows")?;
        if self.id.trim().is_empty()
            || self.dimensions.contains(&0)
            || count > 32_768
            || (0..3).any(|a| {
                !self.lower_left_m[a].is_finite()
                    || !self.upper_right_m[a].is_finite()
                    || self.lower_left_m[a] >= self.upper_right_m[a]
            })
        {
            return Err("mesh dimensions/bounds are invalid or exceed the 32,768-bin cap".into());
        }
        let extent = manifest.major_radius_m
            + manifest
                .variants
                .iter()
                .flat_map(|v| &v.components)
                .map(|c| c.outer_minor_radius_m)
                .fold(0.0_f64, f64::max);
        let outer = manifest
            .variants
            .iter()
            .flat_map(|v| &v.components)
            .map(|c| c.outer_minor_radius_m)
            .fold(0.0_f64, f64::max);
        if self.lower_left_m[0] < -extent
            || self.upper_right_m[0] > extent
            || self.lower_left_m[2] < -extent
            || self.upper_right_m[2] > extent
            || self.lower_left_m[1] < -outer
            || self.upper_right_m[1] > outer
        {
            return Err("mesh bounds exceed the model's rectangular torus envelope".into());
        }
        Ok(())
    }

    pub fn for_preset(manifest: &DemoManifest, preset: MeshPreset) -> Result<Self, ReactorError> {
        let outer = manifest
            .variants
            .iter()
            .flat_map(|v| &v.components)
            .map(|c| c.outer_minor_radius_m)
            .fold(0.0_f64, f64::max);
        let extent = manifest.major_radius_m + outer;
        let mesh = match preset {
            MeshPreset::Coarse => Self {
                id: "neutron-flux".into(),
                dimensions: [12, 8, 12],
                lower_left_m: [-extent, -outer, -extent],
                upper_right_m: [extent, outer, extent],
            },
            MeshPreset::OutboardLocalCoarse => Self {
                id: "neutron-flux-outboard-local-coarse".into(),
                dimensions: [12, 6, 12],
                lower_left_m: [manifest.major_radius_m + 0.7, -0.45, -0.45],
                upper_right_m: [extent, 0.45, 0.45],
            },
            MeshPreset::OutboardLocal => Self {
                id: "neutron-flux-outboard-local".into(),
                dimensions: [24, 12, 24],
                lower_left_m: [manifest.major_radius_m + 0.7, -0.45, -0.45],
                upper_right_m: [extent, 0.45, 0.45],
            },
            MeshPreset::OutboardPortWindow => Self {
                id: "neutron-flux-outboard-port-window".into(),
                dimensions: [1, 1, 1],
                lower_left_m: [manifest.major_radius_m + 1.04, -0.15, -0.15],
                upper_right_m: [extent, 0.15, 0.15],
            },
        };
        mesh.validate(manifest)?;
        Ok(mesh)
    }
    pub fn bin_volume_m3(&self) -> f64 {
        (0..3)
            .map(|axis| {
                (self.upper_right_m[axis] - self.lower_left_m[axis]) / self.dimensions[axis] as f64
            })
            .product()
    }

    /// OpenMC regular mesh convention: x varies fastest, then y, then z.
    pub fn bin_center_m(&self, bin: usize) -> Option<[f64; 3]> {
        if bin >= self.bins() {
            return None;
        }
        let indices = [
            bin % self.dimensions[0],
            (bin / self.dimensions[0]) % self.dimensions[1],
            bin / (self.dimensions[0] * self.dimensions[1]),
        ];
        Some(std::array::from_fn(|axis| {
            self.lower_left_m[axis]
                + (indices[axis] as f64 + 0.5)
                    * (self.upper_right_m[axis] - self.lower_left_m[axis])
                    / self.dimensions[axis] as f64
        }))
    }
}

fn mesh_payload_preflight(
    request: &TransportRequest,
    manifest: &DemoManifest,
    input_json_bytes: usize,
) -> Result<MeshPreflight, ReactorError> {
    mesh_payload_preflight_with_encoding(request, manifest, input_json_bytes, true, true)
}

fn mesh_payload_preflight_legacy_compact(
    request: &TransportRequest,
    manifest: &DemoManifest,
    input_json_bytes: usize,
) -> Result<MeshPreflight, ReactorError> {
    mesh_payload_preflight_with_encoding(request, manifest, input_json_bytes, false, false)
}

fn mesh_payload_preflight_with_encoding(
    request: &TransportRequest,
    manifest: &DemoManifest,
    input_json_bytes: usize,
    pretty_json: bool,
    spectra_reserves_request_copy: bool,
) -> Result<MeshPreflight, ReactorError> {
    let request_bytes = if pretty_json {
        serde_json::to_vec_pretty(request)?.len()
    } else {
        serde_json::to_vec(request)?.len()
    };
    if request_bytes > MAX_JSON_BYTES as usize {
        return Err("transport request exceeds 16 MiB JSON limit".into());
    }
    let mut volumes = Vec::new();
    let mut volume_domains = BTreeSet::new();
    let mut tallies = Vec::with_capacity(request.responses.len());
    let max = f64::MAX;
    for response in &request.responses {
        let domain_key = match &response.domain {
            ResponseDomain::WholeModel => (0_u8, String::new(), 0_u64),
            ResponseDomain::Component { component_id } => (1_u8, component_id.clone(), 0_u64),
            ResponseDomain::Mesh { mesh_id, bin } => (2_u8, mesh_id.clone(), *bin),
        };
        if volume_domains.insert(domain_key) {
            volumes.push(DomainVolume {
                domain: response.domain.clone(),
                value: max,
                standard_error: max,
                unit: VolumeUnit::CubicCentimetre,
            });
        }
        let (estimator, unit) = match &response.score {
            ScoreDefinition::Flux => (TallyEstimator::Tracklength, RawTallyUnit::CmPerSource),
            ScoreDefinition::ReactionRate { .. } => {
                (TallyEstimator::Tracklength, RawTallyUnit::EventsPerSource)
            }
            ScoreDefinition::ParticleProduction { .. } => (
                TallyEstimator::Tracklength,
                RawTallyUnit::ParticlesPerSource,
            ),
            ScoreDefinition::Heating { .. } => {
                (TallyEstimator::Collision, RawTallyUnit::EvPerSource)
            }
        };
        tallies.push(RawTally {
            response_id: response.id.clone(),
            estimator,
            unit,
            mean: max,
            standard_error: max,
        });
    }
    let artifact = TransportArtifact {
        schema_version: TRANSPORT_ARTIFACT_VERSION.into(),
        request: request.clone(),
        solver: ToolIdentity {
            name: "OpenMC".into(),
            version: "0.15.3".into(),
            digest: format!("sha256:{}", "0".repeat(64)),
        },
        nuclear_data: ToolIdentity {
            name: "bounded preflight".into(),
            version: "local".into(),
            digest: format!("sha256:{}", "0".repeat(64)),
        },
        histories: 10_000_000,
        volumes,
        tallies,
    };
    let raw_bound = if pretty_json {
        serde_json::to_vec_pretty(&artifact)?.len()
    } else {
        serde_json::to_vec(&artifact)?.len()
    };
    if raw_bound > MAX_TRANSPORT_ARTIFACT_BYTES {
        return Err("requested tallies exceed the 16 MiB raw-artifact budget".into());
    }

    let variant = manifest
        .variants
        .iter()
        .find(|v| v.id == request.variant_id)
        .ok_or("mesh preflight variant missing")?;
    let mut normalized_results = Vec::with_capacity(request.responses.len());
    for response in &request.responses {
        let (unit, integrated_unit) = match &response.score {
            ScoreDefinition::Flux => (
                PhysicalUnit::NeutronsPerSquareMetreSecond,
                PhysicalUnit::NeutronMetresPerSecond,
            ),
            ScoreDefinition::ReactionRate { .. } => (
                PhysicalUnit::ReactionsPerCubicMetreSecond,
                PhysicalUnit::ReactionsPerSecond,
            ),
            ScoreDefinition::ParticleProduction { .. } => (
                PhysicalUnit::ParticlesPerCubicMetreSecond,
                PhysicalUnit::ParticlesPerSecond,
            ),
            ScoreDefinition::Heating { .. } => {
                (PhysicalUnit::WattsPerCubicMetre, PhysicalUnit::Watts)
            }
        };
        let domain_volume = match &response.domain {
            ResponseDomain::Mesh { .. } => 1.0,
            ResponseDomain::WholeModel => 1.0,
            ResponseDomain::Component { component_id } => variant
                .components
                .iter()
                .find(|c| &c.id == component_id)
                .map(|c| c.full_torus_volume_m3)
                .unwrap_or(1.0),
        };
        normalized_results.push(NormalizedTally {
            response_id: response.id.clone(),
            domain: response.domain.clone(),
            score: response.score.clone(),
            estimator: match &response.score {
                ScoreDefinition::Heating { .. } => TallyEstimator::Collision,
                _ => TallyEstimator::Tracklength,
            },
            mean: max,
            standard_error: max,
            unit,
            integrated_mean: max,
            integrated_standard_error: max,
            integrated_unit,
            volume_m3: domain_volume,
            volume_standard_error_m3: max,
        });
    }
    let normalized = NormalizedTransportResult {
        response_covariance: None,
        schema_version: "faris-normalized-transport/v0.1".into(),
        scenario_id: request.scenario_id.clone(),
        scenario_sha256: request.scenario_sha256.clone(),
        variant_id: request.variant_id.clone(),
        source: request.source.clone(),
        solver: artifact.solver.clone(),
        nuclear_data: artifact.nuclear_data.clone(),
        histories: artifact.histories,
        source_reaction_rate_per_s: max,
        source_neutron_rate_per_s: max,
        results: normalized_results,
    };
    let serialized_run = if pretty_json {
        serde_json::to_vec_pretty(&normalized)?.len()
    } else {
        serde_json::to_vec(&normalized)?.len()
    };
    let run_bound = serialized_run.saturating_add(64 * 1024);
    if run_bound > MAX_JSON_BYTES as usize {
        return Err("normalized run record exceeds the 16 MiB JSON budget".into());
    }
    let spectrum_bound = (if spectra_reserves_request_copy {
        request_bytes
    } else {
        0
    })
    .saturating_add(
        request
            .responses
            .iter()
            .filter(|r| {
                matches!(&r.score, ScoreDefinition::Flux)
                    && matches!(&r.domain, ResponseDomain::Component { .. })
            })
            .count()
            .saturating_mul(
                if request
                    .responses
                    .iter()
                    .any(|r| matches!(r.score, ScoreDefinition::Heating { .. }))
                {
                    2
                } else {
                    1
                },
            )
            .saturating_mul(2 * (11 * 48 + 1024)),
    );
    let package_bound = input_json_bytes
        .saturating_add(raw_bound)
        .saturating_add(run_bound)
        .saturating_add(spectrum_bound)
        .saturating_add(4 * 1024 * 1024);
    if package_bound > 32 * 1024 * 1024 {
        return Err("mesh request exceeds the 32 MiB evidence-package content budget".into());
    }
    Ok(MeshPreflight {
        serialization_method: if pretty_json {
            "pretty-json-v2".into()
        } else {
            "compact-json-v1".into()
        },
        request_json_bytes: request_bytes,
        raw_artifact_upper_bound_bytes: raw_bound,
        run_json_upper_bound_bytes: run_bound,
        package_content_upper_bound_bytes: package_bound,
    })
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SamplingPlan {
    pub batches: u32,
    pub particles_per_batch: u32,
    pub seed: u64,
    pub threads: u32,
}
impl Default for SamplingPlan {
    fn default() -> Self {
        Self {
            batches: 100,
            particles_per_batch: 10_000,
            seed: 123_456_789,
            threads: 1,
        }
    }
}
impl SamplingPlan {
    pub fn validate(&self) -> Result<(), ReactorError> {
        if self.batches < 30
            || self.batches > 1000
            || self.particles_per_batch == 0
            || u64::from(self.batches) * u64::from(self.particles_per_batch) > 10_000_000
            || self.seed == 0
            || self.seed > i64::MAX as u64
            || self.threads == 0
            || self.threads > 32
        {
            return Err("sampling requires 30..1000 batches, 1..10M total histories, positive signed-64-bit seed and 1..32 threads".into());
        }
        Ok(())
    }
}

#[derive(Serialize)]
pub struct ReactorInput {
    pub schema_version: String,
    pub manifest: DemoManifest,
    pub physics: PhysicsCase,
    pub request: TransportRequest,
    pub sampling: SamplingPlan,
    pub mesh: FieldMesh,
    pub cross_sections: PathBuf,
    pub openmc_executable: PathBuf,
    pub nuclear_data_digest: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReactorRun {
    pub schema_version: String,
    pub scenario_sha256: String,
    pub variant_id: String,
    pub physics_sha256: String,
    pub input_sha256: String,
    pub adapter_sha256: String,
    pub python_sha256: String,
    pub openmc_sha256: String,
    pub cross_sections_sha256: String,
    pub audit_sha256: String,
    pub scientific_scope: ScientificScope,
    pub sampling: SamplingPlan,
    pub mesh: FieldMesh,
    #[serde(default)]
    pub mesh_preflight: Option<MeshPreflight>,
    pub execution: Option<JobResult>,
    pub import_error: Option<String>,
    pub raw_artifact_sha256: Option<String>,
    pub normalized: Option<NormalizedTransportResult>,
    #[serde(default)]
    pub transport_spectra_sha256: Option<String>,
    /// Exact worker receipt bytes; absent only on historical runs before the
    /// geometry ownership audit was recorded.
    #[serde(default)]
    pub worker_result_sha256: Option<String>,
    #[serde(default)]
    pub normalized_spectra: Option<Vec<NormalizedEnergySpectrum>>,
    /// Recomputed from the SHA-bound worker receipt; intentionally omitted
    /// from run.json so established evidence bytes stay immutable.
    #[serde(skip)]
    pub sampling_precision_summary: Option<SamplingPrecisionSummary>,
    pub scientific_qualification: String,
    pub notice: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SamplingPrecisionSummary {
    pub plan_id: String,
    pub purpose: String,
    pub estimator: String,
    pub integrated_goal: f64,
    pub local_goal: f64,
    pub check_count: usize,
    pub checks_met: usize,
    pub checks_unmet: usize,
    pub all_goals_met: bool,
    pub whole_model_tbr_relative_standard_error: Option<f64>,
    pub whole_model_heating_relative_standard_error: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MeshPreflight {
    #[serde(default = "legacy_preflight_serialization_method")]
    pub serialization_method: String,
    pub request_json_bytes: usize,
    pub raw_artifact_upper_bound_bytes: usize,
    pub run_json_upper_bound_bytes: usize,
    pub package_content_upper_bound_bytes: usize,
}

fn legacy_preflight_serialization_method() -> String {
    "compact-json-v1".into()
}

pub struct ReactorJob<'a> {
    pub scenario: &'a LoadedScenario,
    pub physics: &'a PhysicsCase,
    pub audit: &'a Path,
    pub cross_sections: &'a Path,
    pub python: &'a Path,
    pub openmc: &'a Path,
    pub output: &'a Path,
    pub sampling: SamplingPlan,
    pub mesh: Option<FieldMesh>,
    pub timeout: Duration,
    /// Embedded worker supplied by the client. Its exact bytes are preserved.
    pub adapter: &'a [u8],
}

pub fn read_json_bytes(path: &Path) -> Result<Vec<u8>, ReactorError> {
    if !path.metadata()?.is_file() {
        return Err("JSON input must be a regular file".into());
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err("JSON input exceeds 16 MiB".into());
    }
    Ok(bytes)
}

pub fn load_physics_case(
    path: &Path,
    scenario: &LoadedScenario,
) -> Result<PhysicsCase, ReactorError> {
    let physics: PhysicsCase = serde_json::from_slice(&read_json_bytes(path)?)?;
    physics.validate_against(scenario)?;
    Ok(physics)
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn hash_file(path: &Path) -> Result<String, ReactorError> {
    let metadata = path.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return Err("identity input must be a regular file below 512 MiB".into());
    }
    let mut file = File::open(path)?.take(MAX_FILE_BYTES + 1);
    let mut hasher = Sha256::new();
    let mut total = 0;
    let mut buffer = [0_u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_FILE_BYTES {
            return Err("identity input grew beyond 512 MiB".into());
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn canonicalize_required_input(path: &Path, label: &str) -> Result<PathBuf, ReactorError> {
    path.canonicalize()
        .map_err(|error| format!("cannot locate {label} at {}: {error}", path.display()).into())
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), ReactorError> {
    let mut file = File::options().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Bind only the case's required nuclides. The audit assertions are checked
/// against local file hashes; this does not authenticate upstream provenance.
pub fn bind_audited_library(
    physics: &PhysicsCase,
    scenario: &LoadedScenario,
    audit: &Value,
    cross_sections: &Path,
) -> Result<PhysicsCase, ReactorError> {
    physics.validate_against(scenario)?;
    if audit["schema"] != "faris.openmc-library-audit/1.0.0" {
        return Err("unsupported library audit".into());
    }
    let cross_sections = cross_sections.canonicalize()?;
    if audit["cross_sections_xml"]["sha256"].as_str() != Some(hash_file(&cross_sections)?.as_str())
    {
        return Err("cross_sections.xml differs from audited file".into());
    }
    let root = cross_sections
        .parent()
        .ok_or("cross-section file needs a parent")?;
    let needed: BTreeSet<_> = physics
        .materials
        .iter()
        .flat_map(|m| match &m.recipe {
            MaterialRecipe::NuclideMixture { nuclides, .. } => {
                nuclides.iter().map(|n| n.nuclide.clone()).collect()
            }
            MaterialRecipe::Void { .. } => Vec::new(),
        })
        .collect();
    let mut files = Vec::new();
    for nuclide in &needed {
        let entry = &audit["neutron_library"][&nuclide];
        if entry["readable_by_openmc_data_api"] != true
            || entry["nuclide_name_in_hdf5"].as_str() != Some(nuclide.as_str())
        {
            return Err(format!("missing or unreadable audited neutron data for {nuclide}").into());
        }
        if entry["reactions"]["301"]["present"] != true {
            return Err(format!("missing MT=301 heating coefficients for {nuclide}").into());
        }
        let relative_path = entry["relative_path"]
            .as_str()
            .ok_or("audit relative path missing")?;
        let relative = Path::new(relative_path);
        if relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err("data path must be safe and relative".into());
        }
        let path = root.join(relative).canonicalize()?;
        if !path.starts_with(root) {
            return Err("audited file escapes library root".into());
        }
        let size_bytes = entry["size_bytes"]
            .as_u64()
            .ok_or("audit file size missing")?;
        let sha256 = entry["sha256"]
            .as_str()
            .ok_or("audit file hash missing")?
            .to_owned();
        if path.metadata()?.len() != size_bytes || hash_file(&path)? != sha256 {
            return Err(format!("data file differs from audit: {nuclide}").into());
        }
        let temperatures_k = serde_json::from_value(entry["temperatures_k"].clone())?;
        files.push(NuclearDataFile {
            file_id: format!("neutron-{nuclide}"),
            relative_path: relative_path.into(),
            sha256: format!("sha256:{sha256}"),
            size_bytes,
            temperatures_k,
            nuclides: vec![nuclide.clone()],
            capabilities: vec![
                NuclearDataCapability::ContinuousEnergyNeutronTransport,
                NuclearDataCapability::Heating,
            ],
        });
    }
    let mut elements: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for nuclide in &needed {
        let element: String = nuclide
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect();
        if element.is_empty() {
            return Err(format!("cannot determine photon-data element for {nuclide}").into());
        }
        elements.entry(element).or_default().push(nuclide.clone());
    }
    for (element, isotopes) in elements {
        let entry = &audit["photon_atomic_library"][&element];
        if entry["library_entry_present"] != true || entry["readable_by_h5py"] != true {
            return Err(
                format!("missing or unreadable audited photon atomic data for {element}").into(),
            );
        }
        let relative_path = entry["relative_path"]
            .as_str()
            .ok_or("photon audit relative path missing")?;
        let relative = Path::new(relative_path);
        if relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err("photon data path must be safe and relative".into());
        }
        let path = root.join(relative).canonicalize()?;
        if !path.starts_with(root) {
            return Err("photon data path escapes library root".into());
        }
        let size_bytes = entry["size_bytes"]
            .as_u64()
            .ok_or("photon audit file size missing")?;
        let sha256 = entry["sha256"]
            .as_str()
            .ok_or("photon audit file hash missing")?
            .to_owned();
        if path.metadata()?.len() != size_bytes || hash_file(&path)? != sha256 {
            return Err(format!("photon data file differs from audit: {element}").into());
        }
        let mut photon_capabilities = vec![NuclearDataCapability::PhotonTransport];
        if entry["atomic_relaxation_populated"] == true {
            photon_capabilities.push(NuclearDataCapability::AtomicRelaxation);
        }
        files.push(NuclearDataFile {
            file_id: format!("photon-{element}"),
            relative_path: relative_path.into(),
            sha256: format!("sha256:{sha256}"),
            size_bytes,
            temperatures_k: Vec::new(),
            nuclides: isotopes,
            capabilities: photon_capabilities,
        });
    }
    let mut bound = physics.clone();
    bound.nuclear_data = NuclearDataSelection::Inventory {
        id: "local-hybrid-fendl32-endfbvii1".into(),
        name: "Local FENDL-3.2 neutron and ENDF/B-VII.1 photon inventory; provenance per component audit".into(),
        version: "FENDL-3.2 neutron + ENDF/B-VII.1 photon".into(),
        files,
    };
    let diagnostics = bound.readiness_diagnostics(scenario)?;
    if !diagnostics.is_empty() {
        return Err(format!(
            "physics readiness gaps: {}",
            serde_json::to_string(&diagnostics)?
        )
        .into());
    }
    Ok(bound)
}

pub fn request_for_case(
    scenario: &LoadedScenario,
    physics: &PhysicsCase,
) -> Result<(TransportRequest, FieldMesh), ReactorError> {
    let manifest = build_manifest(scenario)?;
    let mesh = FieldMesh::for_preset(&manifest, MeshPreset::Coarse)?;
    request_for_case_with_mesh(scenario, physics, mesh)
}

pub fn request_for_case_with_mesh(
    scenario: &LoadedScenario,
    physics: &PhysicsCase,
    mesh: FieldMesh,
) -> Result<(TransportRequest, FieldMesh), ReactorError> {
    physics.validate_against(scenario)?;
    let manifest = build_manifest(scenario)?;
    mesh.validate(&manifest)?;
    let variant = manifest
        .variants
        .iter()
        .find(|v| v.id == physics.variant_id)
        .ok_or("unknown variant")?;
    if variant.components.is_empty() {
        return Err("empty variant".into());
    }
    let mut responses = vec![ResponseDefinition {
        id: "total-tritium-production".into(),
        domain: ResponseDomain::WholeModel,
        score: ScoreDefinition::ParticleProduction {
            particle: ProducedParticle::Tritium,
            score: "H3-production".into(),
        },
    }];
    for (suffix, scope) in [
        ("total", HeatingParticleScope::Total),
        ("neutron", HeatingParticleScope::Neutron),
        ("photon", HeatingParticleScope::Photon),
        ("electron", HeatingParticleScope::Electron),
        ("positron", HeatingParticleScope::Positron),
    ] {
        responses.push(ResponseDefinition {
            id: format!("heating-{suffix}-whole-model"),
            domain: ResponseDomain::WholeModel,
            score: ScoreDefinition::Heating {
                convention: HeatingConvention::Heating,
                particle_scope: scope,
            },
        });
    }
    for component in &variant.components {
        let domain = ResponseDomain::Component {
            component_id: component.id.clone(),
        };
        responses.push(ResponseDefinition {
            id: format!("{}-flux", component.id),
            domain: domain.clone(),
            score: ScoreDefinition::Flux,
        });
        if component.material_id != "void" {
            responses.push(ResponseDefinition {
                id: format!("{}-tritium", component.id),
                domain: domain.clone(),
                score: ScoreDefinition::ParticleProduction {
                    particle: ProducedParticle::Tritium,
                    score: "H3-production".into(),
                },
            });
            for (suffix, scope) in [
                ("total", HeatingParticleScope::Total),
                ("neutron", HeatingParticleScope::Neutron),
                ("photon", HeatingParticleScope::Photon),
                ("electron", HeatingParticleScope::Electron),
                ("positron", HeatingParticleScope::Positron),
            ] {
                responses.push(ResponseDefinition {
                    id: format!("heating-{suffix}-{}", component.id),
                    domain: domain.clone(),
                    score: ScoreDefinition::Heating {
                        convention: HeatingConvention::Heating,
                        particle_scope: scope,
                    },
                });
            }
        }
    }
    for bin in 0..mesh.bins() {
        responses.push(ResponseDefinition {
            id: if mesh.id == "neutron-flux-outboard-port-window" && bin == 0 {
                "outboard-port-window-flux".into()
            } else {
                format!("mesh-flux-{bin}")
            },
            domain: ResponseDomain::Mesh {
                mesh_id: mesh.id.clone(),
                bin: bin as u64,
            },
            score: ScoreDefinition::Flux,
        });
    }
    let request = TransportRequest {
        schema_version: TRANSPORT_REQUEST_VERSION.into(),
        scenario_id: scenario.scenario.id.clone(),
        scenario_sha256: scenario.source_sha256.clone(),
        variant_id: physics.variant_id.clone(),
        fusion_power_mw: scenario.scenario.operating_plan.fusion_power_mw,
        source: physics.source.to_transport_source(),
        responses,
    };
    request.validate_against(scenario)?;
    Ok((request, mesh))
}

fn legacy_request_for_case(
    scenario: &LoadedScenario,
    physics: &PhysicsCase,
) -> Result<(TransportRequest, FieldMesh), ReactorError> {
    let (mut request, mesh) = request_for_case(scenario, physics)?;
    request.schema_version = TRANSPORT_REQUEST_LEGACY_VERSION.into();
    request
        .responses
        .retain(|response| !matches!(response.score, ScoreDefinition::Heating { .. }));
    request.validate_against(scenario)?;
    Ok((request, mesh))
}

fn check_geometric_volumes(
    result: &NormalizedTransportResult,
    scenario: &LoadedScenario,
    mesh: &FieldMesh,
) -> Result<(), ReactorError> {
    let manifest = build_manifest(scenario)?;
    let variant = manifest
        .variants
        .iter()
        .find(|v| v.id == result.variant_id)
        .ok_or("unknown result variant")?;
    let outer = variant
        .components
        .last()
        .ok_or("empty variant")?
        .outer_minor_radius_m;
    let penetration = scenario.scenario.penetration.as_ref();
    let mut port_component_geometry = BTreeMap::new();
    if let Some(faris_model::Penetration::OutboardRectangularPrism { bounds_m, .. }) = penetration {
        for component in &variant.components {
            let intersection = crate::geometry::estimate_torus_shell_box_intersection(
                scenario.scenario.geometry.major_radius_m,
                component.inner_minor_radius_m,
                component.outer_minor_radius_m,
                &bounds_m.minimum_xyz_m,
                &bounds_m.maximum_xyz_m,
            );
            port_component_geometry.insert(
                component.id.as_str(),
                (
                    component.full_torus_volume_m3 - intersection.volume_m3,
                    intersection.refinement_delta_m3,
                ),
            );
        }
    }
    for response in &result.results {
        let expected = match &response.domain {
            ResponseDomain::WholeModel => {
                2.0 * std::f64::consts::PI.powi(2) * manifest.major_radius_m * outer.powi(2)
            }
            ResponseDomain::Component { component_id } => {
                let component = variant
                    .components
                    .iter()
                    .find(|c| &c.id == component_id)
                    .ok_or("unknown component volume")?;
                port_component_geometry
                    .get(component_id.as_str())
                    .map(|entry| entry.0)
                    .unwrap_or(component.full_torus_volume_m3)
            }
            ResponseDomain::Mesh { mesh_id, bin }
                if mesh_id == &mesh.id && *bin < mesh.bins() as u64 =>
            {
                mesh.bin_volume_m3()
            }
            _ => return Err("unknown field volume".into()),
        };
        let tolerance = if let (Some(penetration), ResponseDomain::Component { component_id }) =
            (penetration, &response.domain)
        {
            let _ = penetration;
            let refinement_delta = port_component_geometry
                .get(component_id.as_str())
                .map(|entry| entry.1)
                .ok_or("missing port geometry volume estimate for affected component")?;
            (3.0 * response.volume_standard_error_m3.hypot(refinement_delta)).max(1.0e-8)
        } else {
            1.0e-10 * expected
        };
        if (response.volume_m3 - expected).abs() > tolerance {
            return Err(format!(
                "adapter volume differs from geometry for {}",
                response.response_id
            )
            .into());
        }
    }
    Ok(())
}

fn embedded_worker_sha256() -> String {
    digest(include_bytes!(
        "../../../integrations/openmc/reactor_transport.py"
    ))
}

/// Bind the worker's geometry-only OpenMC Geometry.find probes to the exact
/// input and expected scenario/material assignments. This is a software and
/// geometry control, not experimental or physical validation.
fn validate_geometry_ownership_receipt(
    worker: &Value,
    input_sha256: &str,
    scenario: &LoadedScenario,
    physics: &PhysicsCase,
) -> Result<(), ReactorError> {
    fn audit_str<'a>(obj: &'a Value, key: &str) -> Result<&'a str, ReactorError> {
        obj.get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("geometry ownership audit is missing {key}").into())
    }
    let audit = worker
        .get("geometry_ownership_audit")
        .ok_or("worker receipt is missing geometry_ownership_audit")?;
    let manifest = build_manifest(scenario)?;
    let variant = manifest
        .variants
        .iter()
        .find(|variant| variant.id == physics.variant_id)
        .ok_or("geometry ownership variant is absent from scenario")?;
    if audit_str(audit, "schema_version")? != "faris-openmc-geometry-ownership-audit/v0.1"
        || audit_str(audit, "status")? != "PASS"
        || audit_str(audit, "clearance_status")? != "PASS"
        || audit_str(audit, "scientific_qualification")? != "NOT_EVALUATED"
        || audit_str(audit, "scenario_sha256")? != scenario.source_sha256
        || audit_str(audit, "variant_id")? != physics.variant_id
        || audit_str(audit, "input_sha256")? != input_sha256
        || audit
            .get("checks_are_geometry_only")
            .and_then(Value::as_bool)
            != Some(true)
        || audit.get("probe_count").and_then(Value::as_u64) != Some(117)
        || audit.get("failed_probe_count").and_then(Value::as_u64) != Some(0)
        || audit.get("plasma_radius_m").and_then(Value::as_f64)
            != Some(manifest.plasma_minor_radius_m)
        || audit
            .get("declared_plasma_to_first_wall_clearance_m")
            .and_then(Value::as_f64)
            .is_none_or(|measured| {
                (measured - scenario.scenario.geometry.plasma_to_first_wall_gap_m).abs()
                    > 4.0
                        * f64::EPSILON
                        * scenario
                            .scenario
                            .geometry
                            .plasma_to_first_wall_gap_m
                            .abs()
                            .max(1.0)
            })
        || audit
            .get("first_wall_inner_radius_m")
            .and_then(Value::as_f64)
            != variant
                .components
                .first()
                .map(|component| component.inner_minor_radius_m)
    {
        return Err("worker geometry ownership audit identity/status/geometry differs".into());
    }

    let mut expected_ids = BTreeMap::<String, (String, Option<String>)>::new();
    let toroidal = ["1.57079633", "3.14159265", "4.71238898"];
    for phi in toroidal {
        expected_ids.insert(
            format!("plasma-interior-phi-{phi}"),
            ("plasma-source-domain".into(), None),
        );
        for label in ["near-plasma", "near-first-wall"] {
            expected_ids.insert(
                format!("clearance-{label}-phi-{phi}"),
                ("plasma-first-wall-clearance".into(), None),
            );
        }
    }
    let assignments: BTreeMap<_, _> = physics
        .component_assignments
        .iter()
        .map(|assignment| {
            (
                assignment.component_id.as_str(),
                assignment.material_id.as_str(),
            )
        })
        .collect();
    for component in &variant.components {
        let material_id = assignments
            .get(component.id.as_str())
            .ok_or("geometry ownership component has no material assignment")?;
        let expected_material = if matches!(
            physics
                .materials
                .iter()
                .find(|material| material.id == **material_id),
            Some(faris_model::physics::MaterialDefinition {
                recipe: MaterialRecipe::Void { .. },
                ..
            })
        ) {
            None
        } else {
            Some((**material_id).to_owned())
        };
        for phi in toroidal {
            for theta in ["0.00000000", "1.57079633", "3.14159265", "4.71238898"] {
                expected_ids.insert(
                    format!("{}-mid-phi-{phi}-theta-{theta}", component.id),
                    (component.id.clone(), expected_material.clone()),
                );
            }
            for edge in ["near-inner", "near-outer"] {
                expected_ids.insert(
                    format!("{}-{edge}-phi-{phi}", component.id),
                    (component.id.clone(), expected_material.clone()),
                );
            }
        }
    }
    let probes = audit
        .get("probes")
        .and_then(Value::as_array)
        .ok_or("geometry ownership audit probes are missing")?;
    if probes.len() != expected_ids.len() {
        return Err("geometry ownership audit probe count differs from required coverage".into());
    }
    let mut observed_ids = BTreeSet::new();
    for probe in probes {
        let probe_id = audit_str(probe, "probe_id")?;
        let (cell, material) = expected_ids
            .get(probe_id)
            .ok_or("geometry ownership audit contains an unexpected probe")?;
        let expected_material_id = material.as_ref().map_or("void", |id| id.as_str());
        let openmc_material_name = material.as_ref().map(|id| id.as_str());
        if !observed_ids.insert(probe_id)
            || audit_str(probe, "status")? != "PASS"
            || audit_str(probe, "expected_cell_name")? != cell
            || audit_str(probe, "observed_cell_name")? != cell
            || audit_str(probe, "expected_material_id")? != expected_material_id
            || probe
                .get("expected_openmc_material_name")
                .and_then(Value::as_str)
                != openmc_material_name
            || probe
                .get("observed_openmc_material_name")
                .and_then(Value::as_str)
                != openmc_material_name
            || probe.get("expected_cell_id").and_then(Value::as_u64)
                != probe.get("observed_cell_id").and_then(Value::as_u64)
            || probe
                .get("expected_openmc_material_id")
                .and_then(Value::as_u64)
                != probe
                    .get("observed_openmc_material_id")
                    .and_then(Value::as_u64)
        {
            return Err(format!("geometry ownership probe {probe_id} failed binding").into());
        }
    }
    if observed_ids.len() != expected_ids.len() {
        return Err("geometry ownership audit is missing required probes".into());
    }
    Ok(())
}

/// Extract only the bounded summary shown to the UI. The full per-bin check
/// list stays in the SHA-bound worker artifact; no new run.json fields are
/// written and these exploratory RSE goals never qualify physical accuracy.
fn sampling_precision_summary(worker: &Value) -> Result<SamplingPrecisionSummary, ReactorError> {
    let precision = worker
        .get("sampling_precision")
        .ok_or("worker receipt is missing sampling_precision")?;
    let text = |key: &str| -> Result<String, ReactorError> {
        let value = precision
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("sampling precision is missing {key}"))?;
        if value.len() > 512 {
            return Err(format!("sampling precision {key} exceeds its size bound").into());
        }
        Ok(value)
    };
    let integrated_goal = precision
        .get("integrated_goal")
        .and_then(Value::as_f64)
        .ok_or("sampling precision integrated goal is missing")?;
    let local_goal = precision
        .get("local_goal")
        .and_then(Value::as_f64)
        .ok_or("sampling precision local goal is missing")?;
    if text("plan_id")? != "faris-exploratory-precision-goals/v0.1"
        || integrated_goal != 0.05
        || local_goal != 0.1
    {
        return Err("sampling precision plan identity or goals differ".into());
    }
    let checks = precision
        .get("checks")
        .and_then(Value::as_array)
        .ok_or("sampling precision checks are missing")?;
    if checks.is_empty() || checks.len() > 32_832 {
        return Err("sampling precision check count is outside bounds".into());
    }
    let mut checks_met = 0;
    let mut tbr_rse = None;
    let mut heating_rse = None;
    for check in checks {
        let target = check
            .get("target_relative_standard_error")
            .and_then(Value::as_f64)
            .ok_or("sampling precision target is missing")?;
        let met = check
            .get("met")
            .and_then(Value::as_bool)
            .ok_or("sampling precision check state is missing")?;
        let observed = check
            .get("observed_relative_standard_error")
            .filter(|value| !value.is_null())
            .and_then(Value::as_f64);
        if target != integrated_goal && target != local_goal {
            return Err("sampling precision check uses an undeclared target".into());
        }
        if observed.is_some_and(|value| !value.is_finite() || value < 0.0)
            || met != observed.is_some_and(|value| value <= target)
        {
            return Err("sampling precision check state disagrees with its RSE".into());
        }
        checks_met += usize::from(met);
        match (
            check.get("response_id").and_then(Value::as_str),
            check.get("quantity").and_then(Value::as_str),
        ) {
            (Some("total-tritium-production"), Some("whole-model tritium production"))
                if target != integrated_goal =>
            {
                return Err("whole-model tritium precision check uses the wrong target".into());
            }
            (Some("total-tritium-production"), Some("whole-model tritium production"))
                if tbr_rse.replace(observed).is_some() =>
            {
                return Err("whole-model tritium precision check is ambiguous".into());
            }
            (Some("total-tritium-production"), Some("whole-model tritium production")) => {
                tbr_rse = Some(observed);
            }
            (Some("heating-total-whole-model"), Some("whole-model deposited heating"))
                if target != integrated_goal =>
            {
                return Err("whole-model heating precision check uses the wrong target".into());
            }
            (Some("heating-total-whole-model"), Some("whole-model deposited heating"))
                if heating_rse.replace(observed).is_some() =>
            {
                return Err("whole-model heating precision check is ambiguous".into());
            }
            (Some("heating-total-whole-model"), Some("whole-model deposited heating")) => {
                heating_rse = Some(observed);
            }
            _ => {}
        }
    }
    let declared_all = precision
        .get("all_goals_met")
        .and_then(Value::as_bool)
        .ok_or("sampling precision aggregate state is missing")?;
    let checks_unmet = checks.len() - checks_met;
    if declared_all != (checks_unmet == 0) || tbr_rse.is_none() || heating_rse.is_none() {
        return Err("sampling precision aggregate or whole-model checks are inconsistent".into());
    }
    Ok(SamplingPrecisionSummary {
        plan_id: text("plan_id")?,
        purpose: text("purpose")?,
        estimator: text("estimator")?,
        integrated_goal,
        local_goal,
        check_count: checks.len(),
        checks_met,
        checks_unmet,
        all_goals_met: declared_all,
        whole_model_tbr_relative_standard_error: tbr_rse.flatten(),
        whole_model_heating_relative_standard_error: heating_rse.flatten(),
    })
}

pub fn run_reactor(
    job: &ReactorJob<'_>,
    cancellation: &Cancellation,
) -> Result<ReactorRun, ReactorError> {
    job.sampling.validate()?;
    if job.timeout.is_zero() || job.timeout > Duration::from_secs(3600) {
        return Err("reactor timeout must be 1..3600 seconds".into());
    }
    let python = canonicalize_required_input(job.python, "Python interpreter")?;
    let openmc = canonicalize_required_input(job.openmc, "OpenMC executable")?;
    let python_sha256 = hash_file(&python)?;
    let openmc_sha256 = hash_file(&openmc)?;
    let cross_sections = canonicalize_required_input(job.cross_sections, "cross_sections.xml")?;
    let cross_sections_sha256 = hash_file(&cross_sections)?;
    let audit_bytes = read_json_bytes(job.audit).map_err(|error| {
        format!(
            "cannot read OpenMC library audit JSON at {}: {error}",
            job.audit.display()
        )
    })?;
    let physics = bind_audited_library(
        job.physics,
        job.scenario,
        &serde_json::from_slice(&audit_bytes)?,
        &cross_sections,
    )?;
    let default_mesh = FieldMesh::for_preset(&build_manifest(job.scenario)?, MeshPreset::Coarse)?;
    let requested_mesh = job.mesh.clone().unwrap_or(default_mesh);
    let (request, mesh) = request_for_case_with_mesh(job.scenario, &physics, requested_mesh)?;
    let nuclear_data_digest = format!(
        "sha256:{}",
        digest(&serde_json::to_vec(&physics.nuclear_data)?)
    );
    let input = ReactorInput {
        schema_version: "faris-openmc-input/v0.1".into(),
        manifest: build_manifest(job.scenario)?,
        physics: physics.clone(),
        request: request.clone(),
        sampling: job.sampling.clone(),
        mesh: mesh.clone(),
        cross_sections,
        openmc_executable: openmc.clone(),
        nuclear_data_digest,
    };
    let input_bytes = serde_json::to_vec_pretty(&input)?;
    if input_bytes.len() > MAX_JSON_BYTES as usize {
        return Err("serialized reactor input exceeds 16 MiB".into());
    }
    let mesh_preflight = mesh_payload_preflight(&request, &input.manifest, input_bytes.len())?;
    let output = std::path::absolute(job.output)?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir(&output)?;
    write_new(&output.join("input.json"), &input_bytes)?;
    write_new(&output.join("scenario.json"), job.scenario.source_bytes())?;
    write_new(&output.join("audit.json"), &audit_bytes)?;
    let worker = output.join("reactor_transport.py");
    write_new(&worker, job.adapter)?;
    let mut environment = vec![
        (
            OsString::from("OMP_NUM_THREADS"),
            job.sampling.threads.to_string().into(),
        ),
        ("OPENBLAS_NUM_THREADS".into(), "1".into()),
        ("PYTHONDONTWRITEBYTECODE".into(), "1".into()),
    ];
    let path = std::env::join_paths([
        python.parent().ok_or("Python has no parent")?.to_path_buf(),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ])?;
    environment.push(("PATH".into(), path));
    if let Some(home) = std::env::var_os("HOME") {
        environment.push(("HOME".into(), home));
    }
    let spec = JobSpec {
        program: python,
        arguments: vec![
            worker.into_os_string(),
            "--input".into(),
            output.join("input.json").into_os_string(),
            "--output-dir".into(),
            output.join("solver").into_os_string(),
        ],
        working_directory: output.clone(),
        environment,
        timeout: job.timeout,
        capture_limit_bytes: 4 * 1024 * 1024,
        artifact_roots: vec![],
        resource_limits: ResourceLimits::default(),
    };
    let mut record=ReactorRun{schema_version:"faris-reactor-run/v0.1".into(),scenario_sha256:job.scenario.source_sha256.clone(),variant_id:physics.variant_id.clone(),physics_sha256:digest(&serde_json::to_vec(&physics)?),input_sha256:digest(&input_bytes),adapter_sha256:digest(job.adapter),python_sha256,openmc_sha256,cross_sections_sha256,audit_sha256:digest(&audit_bytes),scientific_scope:physics.scientific_scope.clone(),sampling:job.sampling.clone(),mesh:mesh.clone(),mesh_preflight:Some(mesh_preflight.clone()),execution:None,import_error:None,raw_artifact_sha256:None,normalized:None,transport_spectra_sha256:None,worker_result_sha256:None,normalized_spectra:None,sampling_precision_summary:None,scientific_qualification:"NOT_EVALUATED".into(),notice:"Coupled cold-data numerical surrogate with unqualified material/data/source applicability. Monte Carlo standard errors describe sampling only. Heating is transport energy deposition under OpenMC's local electron treatment; it is not a component thermal model, actual coil lifetime or operating-reactor prediction.".into()};
    match run_job(&spec, cancellation) {
        Ok(execution) => record.execution = Some(execution),
        Err(error) => {
            record.import_error = Some(format!("external execution unavailable: {error}"))
        }
    }
    if record
        .execution
        .as_ref()
        .is_some_and(|e| e.execution_status == ExecutionStatus::Succeeded)
    {
        let import = (|| -> Result<_, ReactorError> {
            let worker_bytes = read_json_bytes(&output.join("solver/worker-result.json"))?;
            let worker: Value = serde_json::from_slice(&worker_bytes)?;
            validate_geometry_ownership_receipt(
                &worker,
                &record.input_sha256,
                job.scenario,
                &physics,
            )?;
            let precision_summary = sampling_precision_summary(&worker)?;
            let raw_bytes = read_json_bytes(&output.join("solver/transport-artifact.json"))?;
            let artifact = TransportArtifact::from_bytes(&raw_bytes)?;
            if artifact.histories
                != u64::from(job.sampling.batches) * u64::from(job.sampling.particles_per_batch)
                || artifact.solver.digest != format!("sha256:{}", record.openmc_sha256)
                || artifact.nuclear_data.digest != input.nuclear_data_digest
            {
                return Err("solver, data or histories differ from the requested execution".into());
            }
            // Recheck local executable identity after execution; altered files
            // invalidate binding. This is a local receipt, not signed attestation.
            if hash_file(&openmc)? != record.openmc_sha256
                || hash_file(&spec.program)? != record.python_sha256
            {
                return Err("executable changed during execution".into());
            }
            let normalized = normalize_transport_artifact(&request, &artifact, job.scenario)?;
            let spectra_bytes = read_json_bytes(&output.join("solver/transport-spectra.json"))?;
            let spectra: RawTransportSpectra = serde_json::from_slice(&spectra_bytes)?;
            let normalized_spectra = normalize_spectra(
                &spectra,
                &artifact,
                &normalized,
                &request,
                &digest(&input_bytes),
            )?;
            check_geometric_volumes(&normalized, job.scenario, &record.mesh)?;
            Ok((
                digest(&raw_bytes),
                digest(&worker_bytes),
                precision_summary,
                normalized,
                digest(&spectra_bytes),
                normalized_spectra,
            ))
        })();
        match import {
            Ok((
                digest,
                worker_digest,
                precision_summary,
                normalized,
                spectra_digest,
                normalized_spectra,
            )) => {
                record.raw_artifact_sha256 = Some(digest);
                record.worker_result_sha256 = Some(worker_digest);
                record.sampling_precision_summary = Some(precision_summary);
                record.normalized = Some(normalized);
                record.transport_spectra_sha256 = Some(spectra_digest);
                record.normalized_spectra = Some(normalized_spectra);
            }
            Err(error) => record.import_error = Some(error.to_string()),
        }
    }
    write_new(
        &output.join("run.json"),
        &serde_json::to_vec_pretty(&record)?,
    )?;
    Ok(record)
}

/// Load locally recorded outputs and recheck exact request/normalization against
/// the original raw artifact. This authenticates no external scientific source.
pub fn load_reactor_run(
    path: &Path,
    scenario: &LoadedScenario,
) -> Result<ReactorRun, ReactorError> {
    let mut record: ReactorRun = serde_json::from_slice(&read_json_bytes(path)?)?;
    if record.schema_version != "faris-reactor-run/v0.1"
        || record.scenario_sha256 != scenario.source_sha256
        || record.scientific_qualification != "NOT_EVALUATED"
    {
        return Err("run record does not match scenario or supported scientific scope".into());
    }
    record.sampling.validate()?;
    let parent = path.parent().ok_or("run record needs a directory")?;
    let input_bytes = read_json_bytes(&parent.join("input.json"))?;
    if digest(&input_bytes) != record.input_sha256 {
        return Err("recorded input digest differs".into());
    }
    let input: Value = serde_json::from_slice(&input_bytes)?;
    let physics: PhysicsCase = serde_json::from_value(input["physics"].clone())?;
    let input_request: TransportRequest = serde_json::from_value(input["request"].clone())?;
    let (request, mesh) = if input_request.schema_version == TRANSPORT_REQUEST_LEGACY_VERSION {
        legacy_request_for_case(scenario, &physics)?
    } else {
        let mesh: FieldMesh = serde_json::from_value(input["mesh"].clone())?;
        request_for_case_with_mesh(scenario, &physics, mesh)?
    };
    let manifest = build_manifest(scenario)?;
    // Enforce current pretty-JSON budgets even when replaying a receipt created
    // by the earlier compact-size estimator.
    let current_preflight = mesh_payload_preflight(&request, &manifest, input_bytes.len())?;
    let saved_preflight_matches = if let Some(saved) = &record.mesh_preflight {
        let expected = match saved.serialization_method.as_str() {
            "pretty-json-v2" => current_preflight,
            "compact-json-v1" => {
                mesh_payload_preflight_legacy_compact(&request, &manifest, input_bytes.len())?
            }
            _ => return Err("unsupported mesh preflight method".into()),
        };
        saved == &expected
    } else {
        true
    };
    if digest(&serde_json::to_vec(&physics)?) != record.physics_sha256
        || serde_json::from_value::<SamplingPlan>(input["sampling"].clone())? != record.sampling
        || !record
            .execution
            .as_ref()
            .is_some_and(|e| e.execution_status == ExecutionStatus::Succeeded)
        || mesh != record.mesh
        || !saved_preflight_matches
        || physics.variant_id != record.variant_id
        || physics.scientific_scope != record.scientific_scope
        || input_request != request
    {
        return Err("recorded case, mesh or response definitions differ".into());
    }
    let raw_bytes = read_json_bytes(&parent.join("solver/transport-artifact.json"))?;
    if digest(&read_json_bytes(&parent.join("audit.json"))?) != record.audit_sha256
        || hash_file(&parent.join("reactor_transport.py"))? != record.adapter_sha256
    {
        return Err("recorded audit or adapter digest differs".into());
    }
    if record.raw_artifact_sha256.as_deref() != Some(digest(&raw_bytes).as_str()) {
        return Err("recorded raw artifact digest differs".into());
    }
    let artifact = TransportArtifact::from_bytes(&raw_bytes)?;
    if artifact.solver.digest != format!("sha256:{}", record.openmc_sha256)
        || artifact.nuclear_data.digest.as_str()
            != input["nuclear_data_digest"]
                .as_str()
                .ok_or("missing recorded data digest")?
        || artifact.histories
            != u64::from(record.sampling.batches) * u64::from(record.sampling.particles_per_batch)
    {
        return Err("recorded solver/data/sampling identity differs".into());
    }
    if let Some(expected_worker_digest) = record.worker_result_sha256.as_deref() {
        let worker_bytes = read_json_bytes(&parent.join("solver/worker-result.json"))?;
        if digest(&worker_bytes) != expected_worker_digest {
            return Err("recorded worker-result digest differs".into());
        }
        let worker: Value = serde_json::from_slice(&worker_bytes)?;
        validate_geometry_ownership_receipt(&worker, &record.input_sha256, scenario, &physics)?;
        record.sampling_precision_summary = Some(sampling_precision_summary(&worker)?);
    } else if record.adapter_sha256 == embedded_worker_sha256() {
        return Err("current worker run is missing its bound geometry-ownership receipt".into());
    }
    let normalized = normalize_transport_artifact(&request, &artifact, scenario)?;
    let normalized_spectra = if request.schema_version == TRANSPORT_REQUEST_VERSION {
        let spectra_bytes = read_json_bytes(&parent.join("solver/transport-spectra.json"))?;
        if record.transport_spectra_sha256.as_deref() != Some(digest(&spectra_bytes).as_str()) {
            return Err("recorded spectra digest differs".into());
        }
        let spectra: RawTransportSpectra = serde_json::from_slice(&spectra_bytes)?;
        Some(normalize_spectra(
            &spectra,
            &artifact,
            &normalized,
            &request,
            &record.input_sha256,
        )?)
    } else {
        None
    };
    check_geometric_volumes(&normalized, scenario, &mesh)?;
    if record.normalized.as_ref() != Some(&normalized) {
        let saved = serde_json::to_value(&record.normalized)?;
        let checked = serde_json::to_value(&normalized)?;
        let differing_fields: Vec<_> = saved
            .as_object()
            .into_iter()
            .flat_map(|object| object.keys())
            .filter(|key| saved[*key] != checked[*key])
            .cloned()
            .collect();
        let differing_results: Vec<_> = saved["results"]
            .as_array()
            .into_iter()
            .flatten()
            .zip(checked["results"].as_array().into_iter().flatten())
            .filter(|(a, b)| a != b)
            .take(3)
            .map(|(a, b)| {
                format!(
                    "{} saved_unit={} checked_unit={}",
                    a["response_id"].as_str().unwrap_or("?"),
                    a["unit"],
                    b["unit"]
                )
            })
            .collect();
        return Err(format!(
            "recorded normalization differs from checked arithmetic in fields: {}; first response mismatches: {}",
            differing_fields.join(", "),
            differing_results.join("; ")
        )
        .into());
    }
    if record.normalized_spectra != normalized_spectra {
        return Err("recorded spectra normalization differs from checked sidecar".into());
    }
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_solver_executables_fail_early_with_path_context() {
        let scenario = LoadedScenario::from_bytes(include_bytes!(
            "../../../scenarios/arc-inspired/cold-coupled-control.scenario.json"
        ))
        .unwrap();
        let physics = load_physics_case(
            Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../scenarios/arc-inspired/cold-coupled-control.reference.physics.json"
            )),
            &scenario,
        )
        .unwrap();
        let missing_python = std::env::temp_dir().join(format!(
            "faris-no-python-{}-not-installed",
            std::process::id()
        ));
        let missing_openmc = std::env::temp_dir().join(format!(
            "faris-no-openmc-{}-not-installed",
            std::process::id()
        ));
        let output =
            std::env::temp_dir().join(format!("faris-early-path-test-{}", std::process::id()));
        let audit = Path::new("unused-audit.json");
        let cross_sections = Path::new("unused-cross-sections.xml");
        let no_adapter: &[u8] = b"must not execute";

        let python_job = ReactorJob {
            scenario: &scenario,
            physics: &physics,
            audit,
            cross_sections,
            python: &missing_python,
            openmc: &missing_openmc,
            output: &output,
            sampling: SamplingPlan::default(),
            mesh: None,
            timeout: Duration::from_secs(10),
            adapter: no_adapter,
        };
        let error = run_reactor(&python_job, &Cancellation::default())
            .unwrap_err()
            .to_string();
        assert!(error.contains("Python interpreter"));
        assert!(error.contains(&missing_python.display().to_string()));

        let python = std::env::current_exe().unwrap();
        let openmc_job = ReactorJob {
            python: &python,
            ..python_job
        };
        let error = run_reactor(&openmc_job, &Cancellation::default())
            .unwrap_err()
            .to_string();
        assert!(error.contains("OpenMC executable"));
        assert!(error.contains(&missing_openmc.display().to_string()));
        assert!(
            !output.exists(),
            "preflight failures must not create a run directory"
        );
    }

    #[test]
    fn sampling_precision_summary_is_bounded_and_preserves_unresolved_local_checks() {
        let worker = serde_json::json!({
            "sampling_precision": {
                "plan_id": "faris-exploratory-precision-goals/v0.1",
                "purpose": "numerical sampling review only; not a physics or design acceptance test",
                "estimator": "one-standard-error relative to the response mean",
                "integrated_goal": 0.05,
                "local_goal": 0.1,
                "all_goals_met": false,
                "checks": [
                    {"response_id":"total-tritium-production", "quantity":"whole-model tritium production", "target_relative_standard_error":0.05, "observed_relative_standard_error":0.001, "met":true},
                    {"response_id":"heating-total-whole-model", "quantity":"whole-model deposited heating", "target_relative_standard_error":0.05, "observed_relative_standard_error":0.002, "met":true},
                    {"response_id":"mesh-flux-0", "quantity":"magnet or mesh neutron flux", "target_relative_standard_error":0.1, "observed_relative_standard_error":null, "met":false}
                ]
            }
        });
        let summary = sampling_precision_summary(&worker).unwrap();
        assert_eq!(summary.check_count, 3);
        assert_eq!(summary.checks_met, 2);
        assert_eq!(summary.checks_unmet, 1);
        assert!(!summary.all_goals_met);
        assert_eq!(summary.whole_model_tbr_relative_standard_error, Some(0.001));
        assert_eq!(
            summary.whole_model_heating_relative_standard_error,
            Some(0.002)
        );

        let mut inconsistent = worker;
        inconsistent["sampling_precision"]["checks"][0]["met"] = serde_json::json!(false);
        assert!(sampling_precision_summary(&inconsistent).is_err());
    }

    #[test]
    fn mesh_uses_full_bin_volume_and_x_fastest_indexing() {
        let mesh = FieldMesh {
            id: "test".into(),
            dimensions: [12, 8, 12],
            lower_left_m: [-6.0, -2.0, -6.0],
            upper_right_m: [6.0, 2.0, 6.0],
        };
        assert_eq!(mesh.bins(), 1152);
        assert_eq!(mesh.bin_volume_m3(), 0.5);
        assert_eq!(mesh.bin_volume_m3() * mesh.bins() as f64, 576.0);
        assert_eq!(mesh.bin_center_m(0), Some([-5.5, -1.75, -5.5]));
        assert_eq!(mesh.bin_center_m(12), Some([-5.5, -1.25, -5.5]));
        assert_eq!(mesh.bin_center_m(96), Some([-5.5, -1.75, -4.5]));
        assert_eq!(mesh.bin_center_m(1152), None);
    }

    #[test]
    fn json_replay_preserves_float_bits_for_geometry_and_normalization() {
        // These magnitudes include the actual radial boundary and source-rate
        // scales that exposed lossy JSON parsing during record replay.
        let original: [f64; 4] = [
            2.280_000_000_000_000_2,
            293.594_308_480_163_36,
            1.861_813_786_415_852_5e20,
            1.0e-19,
        ];
        let encoded = serde_json::to_vec(&original).unwrap();
        let decoded: [f64; 4] = serde_json::from_slice(&encoded).unwrap();
        for (before, after) in original.into_iter().zip(decoded) {
            assert_eq!(before.to_bits(), after.to_bits());
        }
    }
    #[test]
    fn sampling_plan_rejects_unbounded_or_unusable_work() {
        assert!(SamplingPlan::default().validate().is_ok());
        assert!(
            SamplingPlan {
                batches: 29,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            SamplingPlan {
                particles_per_batch: 100_001,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            SamplingPlan {
                seed: u64::MAX,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn mesh_presets_are_bounded_and_local_pair_shares_exact_bounds() {
        let scenario = LoadedScenario::from_bytes(include_bytes!(
            "../../../scenarios/arc-inspired/cold-coupled-control.scenario.json"
        ))
        .unwrap();
        let manifest = build_manifest(&scenario).unwrap();
        let coarse = FieldMesh::for_preset(&manifest, MeshPreset::Coarse).unwrap();
        let local_coarse =
            FieldMesh::for_preset(&manifest, MeshPreset::OutboardLocalCoarse).unwrap();
        let local_fine = FieldMesh::for_preset(&manifest, MeshPreset::OutboardLocal).unwrap();
        let port_window = FieldMesh::for_preset(&manifest, MeshPreset::OutboardPortWindow).unwrap();
        assert_eq!(coarse.dimensions, [12, 8, 12]);
        assert_eq!(local_coarse.lower_left_m, local_fine.lower_left_m);
        assert_eq!(local_coarse.upper_right_m, local_fine.upper_right_m);
        assert_eq!(local_coarse.bins(), 864);
        assert_eq!(local_fine.bins(), 6912);
        assert_eq!(port_window.bins(), 1);
        assert!((port_window.bin_volume_m3() - 1.24 * 0.3 * 0.3).abs() < 1.0e-12);
        assert!(local_fine.validate(&manifest).is_ok());

        let invalid = FieldMesh {
            dimensions: [usize::MAX, 2, 2],
            ..local_fine.clone()
        };
        assert!(invalid.validate(&manifest).is_err());
        let too_many = FieldMesh {
            dimensions: [64, 64, 64],
            ..local_fine.clone()
        };
        assert!(too_many.validate(&manifest).is_err());
        let inverted = FieldMesh {
            upper_right_m: local_fine.lower_left_m,
            ..local_fine
        };
        assert!(inverted.validate(&manifest).is_err());
    }

    #[test]
    fn local_mesh_json_preflight_fits_explicit_artifact_and_package_budgets() {
        let scenario = LoadedScenario::from_bytes(include_bytes!(
            "../../../scenarios/arc-inspired/cold-coupled-control.scenario.json"
        ))
        .unwrap();
        let physics = load_physics_case(
            Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../scenarios/arc-inspired/cold-coupled-control.reference.physics.json"
            )),
            &scenario,
        )
        .unwrap();
        let manifest = build_manifest(&scenario).unwrap();
        let mesh = FieldMesh::for_preset(&manifest, MeshPreset::OutboardLocal).unwrap();
        let (request, _) = request_for_case_with_mesh(&scenario, &physics, mesh).unwrap();
        let preflight = mesh_payload_preflight(&request, &manifest, 128 * 1024).unwrap();
        let compact =
            mesh_payload_preflight_legacy_compact(&request, &manifest, 128 * 1024).unwrap();
        assert_eq!(preflight.serialization_method, "pretty-json-v2");
        assert_eq!(compact.serialization_method, "compact-json-v1");
        assert!(compact.request_json_bytes < preflight.request_json_bytes);
        assert!(preflight.request_json_bytes < MAX_JSON_BYTES as usize);
        assert!(preflight.raw_artifact_upper_bound_bytes < MAX_TRANSPORT_ARTIFACT_BYTES);
        assert!(preflight.run_json_upper_bound_bytes < MAX_JSON_BYTES as usize);
        assert!(preflight.package_content_upper_bound_bytes < 32 * 1024 * 1024);
        assert!(mesh_payload_preflight(&request, &manifest, 64 * 1024 * 1024).is_err());
        let window = FieldMesh::for_preset(&manifest, MeshPreset::OutboardPortWindow).unwrap();
        let (window_request, _) = request_for_case_with_mesh(&scenario, &physics, window).unwrap();
        assert!(
            window_request
                .responses
                .iter()
                .any(|r| r.id == "outboard-port-window-flux")
        );
        assert!(mesh_payload_preflight(&window_request, &manifest, 128 * 1024).is_ok());
    }

    #[test]
    fn compact_and_pretty_preflight_versions_keep_exact_recorded_sizes() {
        let scenario = LoadedScenario::from_bytes(include_bytes!(
            "../../../scenarios/arc-inspired/cold-coupled-control.scenario.json"
        ))
        .unwrap();
        let physics = load_physics_case(
            Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../scenarios/arc-inspired/cold-coupled-control.breeder-emphasis.physics.json"
            )),
            &scenario,
        )
        .unwrap();
        let manifest = build_manifest(&scenario).unwrap();
        let coarse = FieldMesh::for_preset(&manifest, MeshPreset::Coarse).unwrap();
        let (request, _) = request_for_case_with_mesh(&scenario, &physics, coarse).unwrap();
        let old = mesh_payload_preflight_legacy_compact(&request, &manifest, 296_682).unwrap();
        assert_eq!(
            old,
            MeshPreflight {
                serialization_method: "compact-json-v1".into(),
                request_json_bytes: 130_229,
                raw_artifact_upper_bound_bytes: 500_241,
                run_json_upper_bound_bytes: 620_698,
                package_content_upper_bound_bytes: 5_649_173,
            }
        );
        let current = mesh_payload_preflight(&request, &manifest, 296_682).unwrap();
        assert_eq!(
            current,
            MeshPreflight {
                serialization_method: "pretty-json-v2".into(),
                request_json_bytes: 235_600,
                raw_artifact_upper_bound_bytes: 783_214,
                run_json_upper_bound_bytes: 812_095,
                package_content_upper_bound_bytes: 6_359_143,
            }
        );
    }
}
