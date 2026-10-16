//! The ACTINV activation adapter: transport group fluxes and an irradiation schedule in,
//! nuclide inventory, activity, decay heat, decay-photon source and pathways out.
//!
//! ACTINV (Avila Labs' inventory solver) owns the physics: transmutation, decay, decay heat,
//! photon lines. This module owns the hand-off and nothing else:
//!
//! * one ACTINV problem per zone, written from the zone's mass, composition and group flux,
//!   the irradiation schedule (`maintenance::irradiation`) and a cooling grid; the schedule
//!   steps are passed as written and the cooling grid becomes zero-flux steps;
//! * the run goes through [`crate::jobs`] (wall-time limit, bounded logs, process group,
//!   cancellation);
//! * the result is read back into a typed [`ActivationResult`], with units converted at this
//!   boundary only (ACTINV is per gram, FARIS reports W/m3, kW and Bq of the whole zone);
//! * every number that ACTINV states as a hand-off (total flux, fluence, step times, data
//!   hashes) is checked against what FARIS sent, and a mismatch is an error, not a warning;
//! * everything ACTINV reports as lacking data becomes a [`DataGap`], and a gap that can only
//!   lower the totals turns the zone's [`Completeness`] to `LOWER_BOUND`. Nothing is zeroed.
//!
//! Identical requests give identical results: the result holds no timings or paths, and a
//! request already run is served from the content-addressed cache (key: the ACTINV problem,
//! which names the spectrum, schedule, cooling grid, composition and options, plus the hashes
//! of the library and decay files).
//!
//! Every output carries the research-screening statement. The request records which values are
//! authored (mass, composition, schedule) and which are solver results; the result labels its
//! numbers `calculated`, or `conditional` when a data gap makes them a lower bound.

use crate::jobs::{Cancellation, ExecutionStatus, JobSpec, ResourceLimits, run_job};
use crate::maintenance::actinv::{hash_file, hex_sha256, resolve_program, tail_of};
use crate::maintenance::irradiation::{IrradiationSchedule, IrradiationStep};
use crate::transport::NormalizedEnergySpectrum;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const RESULT_SCHEMA: &str = "faris-activation-result/v0.1";
pub const REQUEST_SCHEMA: &str = "faris-activation-request/v0.1";
pub const PHOTON_SOURCE_SCHEMA: &str = "faris-decay-photon-source/v0.1";
const CACHE_SCHEMA: &str = "faris-activation-zone-cache/v0.1";

/// Groups of the neutron activation library ACTINV ships (`fispact-709`).
pub const GROUPS: usize = 709;
/// First boundary of `fispact-709`, eV. FARIS tallies start at 0; the lowest group stands for
/// the library's first group (the same substitution `scripts/build_activation_inputs.py` makes).
pub const LOWEST_EDGE_EV: f64 = 1.0e-5;
/// Cooling times ACTINV is asked for are within this range (ACT-019).
pub const MIN_COOLING_S: f64 = 1.0;
pub const MAX_COOLING_S: f64 = 1.0e9;
/// Largest number of schedule steps sent to ACTINV: each step is a full result record, so a
/// 40,000-step history would write gigabytes. Condense the schedule first.
pub const MAX_SCHEDULE_STEPS: usize = 2_000;
const MAX_RESULT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_LISTED_ITEMS: usize = 25;
const PATHWAY_TOP: usize = 10;
const YEAR_S: f64 = 365.25 * 86_400.0;
/// Relative disagreement allowed between what was sent to ACTINV and what it reports back.
const HANDOFF_TOLERANCE: f64 = 1.0e-9;

// ------------------------------------------------------------------- types --

/// What kind of statement a number is (ACT-047).
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A solver result from the recorded ACTINV run.
    Calculated,
    /// An authored input (mass, composition, schedule, cooling grid).
    Authored,
    /// Valid only under a stated condition: here, a lower bound because data are missing.
    Conditional,
    /// Not evaluated; the entry says why and what to do next.
    NotEvaluated,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompositionBasis {
    WtPercent,
    AtomFraction,
    AtomsPerG,
}

impl CompositionBasis {
    fn actinv(self) -> &'static str {
        match self {
            Self::WtPercent => "wt_percent",
            Self::AtomFraction => "atom_fraction",
            Self::AtomsPerG => "atoms_per_g",
        }
    }
}

/// ACTINV's solution mode. `auto` picks trace when the target burn-up is negligible; trace holds
/// the starting-material nuclides constant, so a material that itself contains radionuclides
/// should be run `coupled`.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SolverMode {
    #[default]
    Auto,
    Trace,
    Coupled,
}

impl SolverMode {
    fn actinv(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Trace => "trace",
            Self::Coupled => "coupled",
        }
    }
}

/// The mode actually sent for a zone. A zone that starts from named nuclides may start from
/// radionuclides, which ACTINV's trace mode holds constant (fixed in ACTINV 1.5.0, which
/// no longer picks trace for them), so `auto` becomes `coupled` here. Returns the mode and,
/// when it differs from the request's, the reason.
fn effective_mode(zone: &ZoneInput, requested: SolverMode) -> (SolverMode, Option<String>) {
    if requested == SolverMode::Auto && !starting_nuclides(zone).is_empty() {
        (
            SolverMode::Coupled,
            Some(
                "The zone starts from named nuclides, which may be radioactive; ACTINV before 1.5.0 would hold them constant in auto mode. FARIS sent the coupled mode instead."
                    .to_string(),
            ),
        )
    } else {
        (requested, None)
    }
}

/// A quantity FARIS cannot give, with the reason and what to do about it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct NotEvaluated {
    pub quantity: String,
    pub reason: String,
    pub next_step: String,
}

fn not_evaluated(quantity: &str, reason: &str, next_step: &str) -> NotEvaluated {
    NotEvaluated {
        quantity: quantity.into(),
        reason: reason.into(),
        next_step: next_step.into(),
    }
}

/// Group flux of one zone in neutrons per cm2 per second, one value per group, with the group
/// edges it was tallied on. An empty spectrum is a zero flux and says so in the result.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GroupFlux {
    pub energy_edges_ev: Vec<f64>,
    pub flux_per_cm2_s: Vec<f64>,
}

impl GroupFlux {
    /// From a normalized FARIS spectrum (n per m2 per s per group): the only unit conversion is
    /// the factor 1e-4 from m2 to cm2, applied to every group.
    pub fn from_normalized(spectrum: &NormalizedEnergySpectrum) -> Result<Self, String> {
        if spectrum.particle != "neutron" {
            return Err(format!(
                "{}: the {} spectrum cannot drive neutron activation; use the neutron spectrum",
                spectrum.component_id, spectrum.particle
            ));
        }
        Ok(Self {
            energy_edges_ev: spectrum.energy_edges_ev.clone(),
            flux_per_cm2_s: spectrum
                .mean_per_square_metre_second
                .iter()
                .map(|v| v * 1.0e-4)
                .collect(),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.flux_per_cm2_s.is_empty() && self.energy_edges_ev.is_empty()
    }

    /// Sum over groups, n cm-2 s-1.
    pub fn total(&self) -> f64 {
        self.flux_per_cm2_s.iter().sum()
    }

    fn validate(&self, zone: &str) -> Result<(), String> {
        if self.is_empty() {
            return Ok(());
        }
        if self.flux_per_cm2_s.len() != GROUPS || self.energy_edges_ev.len() != GROUPS + 1 {
            return Err(format!(
                "zone {zone}: the spectrum has {} values and {} edges; the activation library \
                 needs {GROUPS} values and {} edges (the fispact-709 structure). Re-tally the \
                 zone with the activation spectra option, or pass an empty spectrum for zero flux",
                self.flux_per_cm2_s.len(),
                self.energy_edges_ev.len(),
                GROUPS + 1
            ));
        }
        if self
            .flux_per_cm2_s
            .iter()
            .any(|v| !v.is_finite() || *v < 0.0)
        {
            return Err(format!(
                "zone {zone}: a group flux is negative or not finite; fix the transport result"
            ));
        }
        if self
            .energy_edges_ev
            .iter()
            .any(|v| !v.is_finite() || *v < 0.0)
            || self.energy_edges_ev.windows(2).any(|w| w[1] <= w[0])
        {
            return Err(format!(
                "zone {zone}: the energy edges must be finite and strictly increasing"
            ));
        }
        Ok(())
    }

    /// Edges as ACTINV is given them: a leading 0 stands for the library's first edge.
    fn actinv_edges(&self) -> Vec<f64> {
        let mut edges = self.energy_edges_ev.clone();
        if edges.first() == Some(&0.0) {
            edges[0] = LOWEST_EDGE_EV;
        }
        edges
    }
}

/// What a zone is made of, to be joined to a transport spectrum by component id.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ZoneSpec {
    pub id: String,
    /// Component whose neutron spectrum drives the zone; the zone id when absent.
    #[serde(default)]
    pub component_id: Option<String>,
    pub mass_g: f64,
    pub volume_m3: f64,
    pub basis: CompositionBasis,
    pub composition: BTreeMap<String, f64>,
}

/// Joins zones to the neutron spectra of a transport result. A zone with no neutron spectrum
/// is an error: a missing spectrum is not a zero flux.
pub fn zones_from_spectra(
    specs: &[ZoneSpec],
    spectra: &[NormalizedEnergySpectrum],
) -> Result<Vec<ZoneInput>, String> {
    specs
        .iter()
        .map(|spec| {
            let component = spec.component_id.as_deref().unwrap_or(&spec.id);
            let found = spectra
                .iter()
                .find(|s| s.component_id == component && s.particle == "neutron" && s.energy_edges_ev.len() == GROUPS + 1)
                .ok_or_else(|| {
                    format!(
                        "zone {}: the transport result has no {GROUPS}-group neutron spectrum for component {component}; \
                         rerun the transport with the activation spectra option for that component",
                        spec.id
                    )
                })?;
            Ok(ZoneInput {
                id: spec.id.clone(),
                mass_g: spec.mass_g,
                volume_m3: spec.volume_m3,
                basis: spec.basis,
                composition: spec.composition.clone(),
                spectrum: GroupFlux::from_normalized(found)?,
            })
        })
        .collect()
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ZoneInput {
    pub id: String,
    pub mass_g: f64,
    pub volume_m3: f64,
    pub basis: CompositionBasis,
    pub composition: BTreeMap<String, f64>,
    pub spectrum: GroupFlux,
}

/// An `actinv-photon-response-1` file for the contact-dose proxy.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PhotonResponse {
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PhotonOptions {
    /// Photon group boundaries, eV, strictly increasing from 0. None: ACTINV's 24-group
    /// structure (the line list is the exact source; groups are for transport).
    #[serde(default)]
    pub group_boundaries_ev: Option<Vec<f64>>,
    #[serde(default)]
    pub response: Option<PhotonResponse>,
}

/// `groups` photon groups: 0 to 1 keV, then log-spaced to 20 MeV.
pub fn log_photon_groups(groups: usize) -> Result<Vec<f64>, String> {
    if !(2..=2000).contains(&groups) {
        return Err("photon groups must be between 2 and 2000".into());
    }
    let mut edges = vec![0.0];
    let n = groups - 1;
    for i in 0..=n {
        edges.push(1.0e3 * faris_model::math::powf(2.0e7_f64 / 1.0e3, i as f64 / n as f64));
    }
    Ok(edges)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ActivationRequest {
    pub zones: Vec<ZoneInput>,
    /// The irradiation schedule: duration and flux scale per step, as `maintenance::irradiation`
    /// writes it.
    pub steps: Vec<IrradiationStep>,
    /// The `input_sha256` of the irradiation schedule the steps came from, when they came from one.
    #[serde(default)]
    pub schedule_input_sha256: Option<String>,
    /// Cooling times since shutdown, seconds, ascending, 1 s to 1e9 s. Results are given at
    /// these times only.
    pub cooling_s: Vec<f64>,
    #[serde(default)]
    pub photon: PhotonOptions,
    #[serde(default)]
    pub mode: SolverMode,
}

impl ActivationRequest {
    /// A request whose irradiation steps are an irradiation schedule built from the operating
    /// history; its input hash is carried into the receipt. A schedule whose stored hash no
    /// longer matches its content is refused.
    pub fn from_schedule(
        zones: Vec<ZoneInput>,
        schedule: &IrradiationSchedule,
        cooling_s: Vec<f64>,
        photon: PhotonOptions,
    ) -> Result<Self, String> {
        if !schedule.verify() {
            return Err("the irradiation schedule's input hash does not match its content; rebuild it from the history".into());
        }
        Ok(Self {
            zones,
            steps: schedule.steps.clone(),
            schedule_input_sha256: Some(schedule.input_sha256.clone()),
            cooling_s,
            photon,
            mode: SolverMode::Auto,
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.zones.is_empty() {
            return Err("the request has no zones; give at least one component".into());
        }
        let mut seen = BTreeSet::new();
        for z in &self.zones {
            if z.id.trim().is_empty() || !seen.insert(z.id.clone()) {
                return Err(format!("zone id {:?} is empty or repeated", z.id));
            }
            if !(z.mass_g.is_finite() && z.mass_g > 0.0) {
                return Err(format!("zone {}: mass_g must be positive", z.id));
            }
            if !(z.volume_m3.is_finite() && z.volume_m3 > 0.0) {
                return Err(format!("zone {}: volume_m3 must be positive", z.id));
            }
            if z.composition.is_empty()
                || z.composition.values().any(|v| !v.is_finite() || *v < 0.0)
                || z.composition.values().all(|v| *v == 0.0)
            {
                return Err(format!(
                    "zone {}: the composition must name at least one nuclide with a positive, finite amount",
                    z.id
                ));
            }
            z.spectrum.validate(&z.id)?;
            if self.mode == SolverMode::Trace && !starting_nuclides(z).is_empty() {
                return Err(format!(
                    "zone {}: the trace mode holds the starting nuclides constant, so a radionuclide in the starting material would not decay; \
                     use the coupled or auto mode (auto sends coupled for a start from named nuclides)",
                    z.id
                ));
            }
        }
        if self.steps.is_empty() {
            return Err("the irradiation schedule has no steps".into());
        }
        if self.steps.len() > MAX_SCHEDULE_STEPS {
            return Err(format!(
                "the irradiation schedule has {} steps, more than the {MAX_SCHEDULE_STEPS} one \
                 ACTINV run can return; condense it with the irradiation schedule's lumping rule",
                self.steps.len()
            ));
        }
        for s in &self.steps {
            if !(s.duration_s.is_finite() && s.duration_s > 0.0)
                || !(s.flux_scale.is_finite() && s.flux_scale >= 0.0)
            {
                return Err(
                    "a schedule step needs a positive duration and a nonnegative flux scale".into(),
                );
            }
        }
        if self.cooling_s.is_empty() {
            return Err(
                "the cooling grid is empty; give at least one time between 1 s and 1e9 s".into(),
            );
        }
        for w in self.cooling_s.windows(2) {
            if w[1] <= w[0] {
                return Err("cooling times must be strictly ascending".into());
            }
        }
        if self
            .cooling_s
            .iter()
            .any(|t| !(MIN_COOLING_S..=MAX_COOLING_S).contains(t))
        {
            return Err(format!(
                "cooling times must lie between {MIN_COOLING_S} s and {MAX_COOLING_S:e} s"
            ));
        }
        if let Some(edges) = &self.photon.group_boundaries_ev
            && (edges.len() < 2
                || edges
                    .windows(2)
                    .any(|w| w[1].partial_cmp(&w[0]) != Some(std::cmp::Ordering::Greater))
                || edges[0] < 0.0)
        {
            return Err(
                "photon group boundaries must be nonnegative and strictly increasing".into(),
            );
        }
        Ok(())
    }

    fn irradiation_s(&self) -> f64 {
        self.steps.iter().map(|s| s.duration_s).sum()
    }
}

/// Parse `1s,1h,1d,1w,30d,1y` (s, min, h, d, w, y) into seconds.
pub fn parse_cooling_list(text: &str) -> Result<Vec<f64>, String> {
    text.split(',')
        .map(|item| {
            let item = item.trim();
            let split = item
                .find(|c: char| c.is_ascii_alphabetic() && c != 'e' && c != 'E')
                .ok_or_else(|| {
                    format!("cooling time {item:?} needs a unit (s, min, h, d, w, y)")
                })?;
            let (number, unit) = item.split_at(split);
            let scale = match unit {
                "s" => 1.0,
                "min" => 60.0,
                "h" => 3600.0,
                "d" => 86_400.0,
                "w" => 7.0 * 86_400.0,
                "y" => YEAR_S,
                _ => {
                    return Err(format!(
                        "cooling time {item:?}: unit must be s, min, h, d, w or y"
                    ));
                }
            };
            number
                .parse::<f64>()
                .map(|v| v * scale)
                .map_err(|_| format!("cooling time {item:?}: {number:?} is not a number"))
        })
        .collect()
}

/// One decade per point, 1 s to 1e9 s.
pub fn decade_cooling_grid() -> Vec<f64> {
    (0..=9).map(|k| faris_model::math::powi(10.0, k)).collect()
}

// ------------------------------------------------------------------ config --

/// The data files an ACTINV problem names. Their SHA-256 is computed here, pinned in the
/// problem, and compared with the hashes ACTINV records after the run.
#[derive(Clone, Debug, PartialEq)]
pub struct ActivationData {
    pub library: PathBuf,
    pub library_id: String,
    pub decay_primary: PathBuf,
    pub decay_primary_id: String,
    pub decay_fallback: Option<(PathBuf, String)>,
}

impl ActivationData {
    /// The files of ACTINV's catalogue v1.1.0 under an ACTINV data root.
    pub fn catalog(data_dir: &Path) -> Self {
        let v = data_dir.join("v1.1.0");
        Self {
            library: v.join("activation/tendl-2025-neutron-709g.npz"),
            library_id: "tendl-2025-neutron-709g".into(),
            decay_primary: v.join("decay/endf-b-viii-0_decay.dat"),
            decay_primary_id: "endfb-viii-0-decay".into(),
            decay_fallback: Some((v.join("decay/jeff-3-3_decay.dat"), "jeff-3-3-decay".into())),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ActivationConfig {
    pub actinv: PathBuf,
    pub data: ActivationData,
    /// Scratch folder for problems and results (results are deleted after they are read).
    pub work_dir: PathBuf,
    /// Content-addressed zone results; None runs ACTINV every time.
    pub cache_dir: Option<PathBuf>,
    pub timeout: Duration,
    pub resource_limits: ResourceLimits,
}

impl ActivationConfig {
    pub fn new(actinv: PathBuf, data: ActivationData, work_dir: PathBuf) -> Self {
        Self {
            actinv,
            data,
            work_dir,
            cache_dir: None,
            timeout: Duration::from_secs(1800),
            resource_limits: ResourceLimits::default(),
        }
    }
}

// ------------------------------------------------------------------ result --

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DecayHeat {
    pub total_w_per_m3: f64,
    pub alpha_w_per_m3: f64,
    pub beta_w_per_m3: f64,
    pub gamma_w_per_m3: f64,
    /// The whole zone, kW.
    pub total_kw: f64,
    /// |total - (alpha + beta + gamma)| / total as ACTINV reports them (0 when total is 0).
    pub parts_closure_relative: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct NuclideState {
    pub nuclide: String,
    /// Atoms in the whole zone. None for a starting-material nuclide that ACTINV holds as a
    /// constant reservoir (trace mode): it reports its activity but not its atoms.
    pub atoms: Option<f64>,
    /// Activity of the whole zone, Bq; 0 for a stable nuclide.
    pub activity_bq: f64,
    /// Decay-photon power of this nuclide in the whole zone, W (ACTINV evaluated line
    /// energies). Not the nuclide's total decay heat; see the zone's not-evaluated list.
    pub photon_power_w: Option<f64>,
    /// ACTINV's semi-infinite-slab contact gamma dose-rate proxy of this nuclide, Gy/h, as
    /// ACTINV reports it (intensive; not scaled by mass). None without a photon response.
    pub contact_dose_proxy_gy_h: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PhotonGroup {
    pub low_ev: f64,
    pub high_ev: f64,
    pub centroid_ev: f64,
    /// Photons per second from the whole zone.
    pub photons_per_s: f64,
    pub power_w: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PhotonSource {
    pub groups: Vec<PhotonGroup>,
    /// Photons per second from the whole zone, all lines.
    pub total_photons_per_s: f64,
    pub grouped_photons_per_s: f64,
    /// total minus grouped; positive when the group structure leaves photons out.
    pub photons_outside_groups_per_s: f64,
    pub power_w: f64,
    /// Fraction of the gamma power ACTINV could represent as lines.
    pub represented_gamma_power_fraction: f64,
    /// Contact gamma dose-rate proxy of the whole inventory, Gy/h; None without a response.
    pub contact_dose_proxy_gy_h: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Pathway {
    pub from: String,
    pub first_product: String,
    pub atoms_per_g: f64,
    pub fraction: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CoolingPoint {
    pub cooling_s: f64,
    pub kind: Kind,
    pub decay_heat: DecayHeat,
    pub activity_bq: f64,
    pub activity_bq_per_m3: f64,
    pub leakage_atoms: f64,
    pub nuclides: Vec<NuclideState>,
    /// Nuclides left out by [`limit_nuclides`]; their activity is still in the totals.
    pub nuclides_omitted: usize,
    pub photon_source: PhotonSource,
    pub top_by_activity: Vec<String>,
    pub top_by_photon_power: Vec<String>,
    /// Production paths ACTINV attributes to the nuclides above (atoms at this time); empty
    /// when ACTINV gave none (not trace mode, or feed or removal in the schedule).
    pub pathways: BTreeMap<String, Vec<Pathway>>,
}

/// A nuclide or pathway ACTINV could not complete for lack of data. Never read as zero.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DataGap {
    pub category: String,
    pub count: usize,
    /// Up to 25 sorted identifiers (`Z=100 A=261 isomer 0` for numeric ACTINV keys).
    pub items: Vec<String>,
    /// Atoms per g ACTINV puts on the missing items, when it gives an amount.
    pub amount_atoms_per_g: Option<f64>,
    pub effect: String,
    pub next_step: String,
    /// True when the gap can only lower decay heat, activity or photon totals.
    pub lower_bound: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Completeness {
    Complete,
    LowerBound,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ZoneInputRecord {
    pub mass_g: f64,
    pub volume_m3: f64,
    pub kind: Kind,
    pub basis: CompositionBasis,
    pub composition_sha256: String,
    /// SHA-256 of the group edges and fluxes as sent to ACTINV (cm-2 s-1).
    pub spectrum_sha256: String,
    pub spectrum_total_flux_cm2_s: f64,
    /// ACTINV's own total of the flux it received.
    pub spectrum_total_flux_actinv_cm2_s: f64,
    pub fluence_n_cm2: f64,
    /// The solver mode sent to ACTINV.
    pub solver_mode_sent: SolverMode,
    /// Why it differs from the requested mode, when it does.
    pub mode_substitution: Option<String>,
    pub messages: Vec<String>,
}

/// What identifies the ACTINV run behind a zone.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RunRecord {
    /// SHA-256 of the ACTINV problem file as written.
    pub problem_sha256: String,
    pub solver: String,
    pub mode: String,
    pub total_states: u64,
    pub pruned_states: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ZoneResult {
    pub id: String,
    pub input: ZoneInputRecord,
    pub run: RunRecord,
    pub completeness: Completeness,
    pub points: Vec<CoolingPoint>,
    pub gaps: Vec<DataGap>,
    pub not_evaluated: Vec<NotEvaluated>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DataVersion {
    pub id: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DataVersions {
    pub solver: String,
    pub library: DataVersion,
    pub decay_primary: DataVersion,
    pub decay_fallback: Option<DataVersion>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct TotalPoint {
    pub cooling_s: f64,
    pub kind: Kind,
    pub completeness: Completeness,
    /// Sum over zones in zone order.
    pub decay_heat_kw: f64,
    pub activity_bq: f64,
    pub photons_per_s: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ScheduleRecord {
    pub kind: Kind,
    pub steps: usize,
    pub irradiation_s: f64,
    pub steps_sha256: String,
    pub source_input_sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ActivationResult {
    pub schema: String,
    pub research_screening: String,
    pub label: String,
    pub data: DataVersions,
    pub schedule: ScheduleRecord,
    pub cooling_s: Vec<f64>,
    pub zones: Vec<ZoneResult>,
    pub totals: Vec<TotalPoint>,
    pub not_evaluated: Vec<NotEvaluated>,
    /// SHA-256 over the schema, data hashes, schedule, cooling grid and every zone's problem hash.
    pub input_sha256: String,
}

/// What a run did; kept out of the result so the result stays reproducible.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunStats {
    pub actinv_runs: u64,
    pub cache_hits: u64,
}

// ------------------------------------------------------------------ hashing --

fn sha256_json(value: &impl Serialize) -> Result<String, String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    Ok(hex_sha256(&bytes))
}

fn relative_difference(a: f64, b: f64) -> f64 {
    if a == b {
        0.0
    } else {
        (a - b).abs() / a.abs().max(b.abs())
    }
}

// ------------------------------------------------------------------ problem --

struct Pins {
    library_sha: String,
    primary_sha: String,
    fallback_sha: Option<String>,
}

/// Case-insensitive nuclide name with the isomer written `m` (ACTINV prints `Tc99m1`).
fn canonical_nuclide(name: &str) -> String {
    let lower = name.to_lowercase();
    lower
        .strip_suffix("m1")
        .map_or(lower.clone(), |b| format!("{b}m"))
}

/// Explicit nuclides the zone starts with; natural elements expand to stable isotopes that
/// carry no activity.
fn starting_nuclides(zone: &ZoneInput) -> BTreeSet<String> {
    zone.composition
        .keys()
        .filter(|k| k.chars().any(|c| c.is_ascii_digit()))
        .map(|k| canonical_nuclide(k))
        .collect()
}

fn pins(data: &ActivationData) -> Result<Pins, String> {
    let hash = |p: &Path, what: &str| {
        hash_file(p).map_err(|e| {
            format!("{what}: {e}; run `actinv data fetch` for the catalogue, or pass --data-dir")
        })
    };
    Ok(Pins {
        library_sha: hash(&data.library, "activation library")?,
        primary_sha: hash(&data.decay_primary, "decay data")?,
        fallback_sha: match &data.decay_fallback {
            Some((p, _)) => Some(hash(p, "fallback decay data")?),
            None => None,
        },
    })
}

fn dt_string(seconds: f64) -> String {
    format!("{seconds} s")
}

/// The ACTINV problem of one zone. Schedule: the irradiation steps, then one zero-flux step to
/// each cooling time. The spectrum is the zone's group flux, scaled by each step's flux scale.
pub fn build_problem(
    zone: &ZoneInput,
    request: &ActivationRequest,
    data: &ActivationData,
    library_sha256: &str,
) -> Value {
    let mut schedule: Vec<Value> = request
        .steps
        .iter()
        .map(|s| json!({"dt": dt_string(s.duration_s), "flux": s.flux_scale}))
        .collect();
    let mut previous = 0.0;
    for &t in &request.cooling_s {
        schedule.push(json!({"dt": dt_string(t - previous), "flux": 0.0}));
        previous = t;
    }
    let spectrum = if zone.spectrum.is_empty() {
        json!({"structure": "fispact-709", "flux_per_group": vec![0.0; GROUPS], "descending": false})
    } else {
        json!({
            "structure": "custom",
            "boundaries_eV": zone.spectrum.actinv_edges(),
            "flux_per_group": zone.spectrum.flux_per_cm2_s,
            "descending": false,
        })
    };
    let mut photon = json!({
        "group_structure": "fispact-24",
        "build_up_factor": 2.0,
        "gamma_constant_cutoff_eV": 20000.0,
    });
    if let Some(edges) = &request.photon.group_boundaries_ev {
        photon["group_structure"] = json!("custom");
        photon["group_boundaries_eV"] = json!(edges);
    }
    if let Some(r) = &request.photon.response {
        photon["response"] = json!({"path": r.path, "sha256": r.sha256});
    }
    let mut decay = json!({"primary": data.decay_primary});
    if let Some((p, _)) = &data.decay_fallback {
        decay["fallback"] = json!(p);
    }
    json!({
        "spec": "actinv-spec-1",
        "title": format!("FARIS activation zone {}", zone.id),
        "projectile": "neutron",
        "library": {"path": data.library, "sha256": library_sha256},
        "decay": decay,
        "material": {
            "mass_g": zone.mass_g,
            "basis": zone.basis.actinv(),
            "composition": zone.composition,
        },
        "spectrum": spectrum,
        "schedule": schedule,
        "options": {
            "mode": effective_mode(zone, request.mode).0.actinv(),
            "prune": "rate",
            "bmin_atoms_per_g": 1e-8,
            "temperature_K": 293.6,
            "cram_order": 16,
            "outputs": ["inventory", "activity", "heat", "photons", "pathways"],
        },
        "photon": photon,
    })
}

// -------------------------------------------------------------- result reader --

fn number(v: &Value, what: &str) -> Result<f64, String> {
    v.as_f64()
        .filter(|x| x.is_finite())
        .ok_or_else(|| format!("ACTINV result: {what} is missing or not a finite number"))
}

fn field<'a>(v: &'a Value, key: &str, what: &str) -> Result<&'a Value, String> {
    v.get(key)
        .ok_or_else(|| format!("ACTINV result: {what} has no {key}"))
}

fn id_label(key: &str) -> String {
    let parsed = key.split_once('_').and_then(|(za, l)| {
        let za: u32 = za.parse().ok()?;
        let l: u32 = l.parse().ok()?;
        Some((za / 1000, za % 1000, l))
    });
    match parsed {
        Some((z, a, l)) => format!("Z={z} A={a} isomer {l}"),
        None => key.to_string(),
    }
}

fn listed(value: Option<&Value>) -> (usize, Vec<String>, Option<f64>) {
    match value {
        Some(Value::Object(m)) => {
            let mut items: Vec<String> = m.keys().map(|k| id_label(k)).collect();
            items.sort();
            let amount: f64 = m.values().filter_map(Value::as_f64).sum();
            let has_amount = m.values().any(Value::is_number);
            let n = items.len();
            items.truncate(MAX_LISTED_ITEMS);
            (n, items, has_amount.then_some(amount))
        }
        Some(Value::Array(a)) => {
            let mut items: Vec<String> = a
                .iter()
                .map(|x| match x {
                    Value::String(s) => id_label(s),
                    Value::Object(o) => o
                        .get("limitation")
                        .and_then(Value::as_str)
                        .map_or_else(|| x.to_string(), str::to_string),
                    other => other.to_string(),
                })
                .collect();
            items.sort();
            let n = items.len();
            items.truncate(MAX_LISTED_ITEMS);
            (n, items, None)
        }
        Some(Value::Number(n)) => {
            let c = n.as_u64().unwrap_or(0) as usize;
            (c, Vec::new(), None)
        }
        _ => (0, Vec::new(), None),
    }
}

/// Every place ACTINV's ledger says a nuclide, target, product or branch lacked data.
fn data_gaps(result: &Value, final_leakage_atoms_per_g: f64) -> Vec<DataGap> {
    let ledger = result.get("ledger").unwrap_or(&Value::Null);
    let table: [(&str, &str, &str, &str, bool); 9] = [
        (
            "products_no_evaluated_decay_data",
            "products with no decay data",
            "Their decay heat, activity and photon emission are not in the totals, so the totals are lower bounds.",
            "Check the nuclides against the decay library, or add decay data for them (ACTINV data override) and rerun.",
            true,
        ),
        (
            "composition_isotopes_absent_from_decay_library",
            "composition isotopes absent from the decay library",
            "These isotopes of the material carry no decay data; their daughters are missing from the totals.",
            "Use a decay library that lists them, or check the composition.",
            true,
        ),
        (
            "composition_elements_unknown",
            "unknown element symbols in the composition",
            "These entries contribute no atoms to the material.",
            "Correct the element symbols in the zone composition.",
            true,
        ),
        (
            "fission_no_yields_to_leakage",
            "fissioning parents without fission-yield data",
            "Fission products of these parents are routed out of the chain, so totals are lower bounds.",
            "Supply fission-yield files for these parents (ACTINV fission_yields) and rerun.",
            true,
        ),
        (
            "targets_absent_from_decay_library",
            "targets absent from the decay library",
            "Reactions on these nuclides are in the library but the nuclides have no decay record; listed so they are not read as zero.",
            "Decide whether they matter for the zone's composition; supply decay data if so.",
            false,
        ),
        (
            "isomer_state_absent_from_decay_library_used_ground",
            "isomer states without decay data (ground-state data used)",
            "The ground-state decay data stand in for the isomer; its own half-life and emissions are not used.",
            "Supply isomer decay data if the isomer matters for the cooling times asked for.",
            false,
        ),
        (
            "decay_daughters_missing",
            "decay daughters missing from the chain",
            "Decay branches ending on a nuclide ACTINV does not track.",
            "Inspect the ACTINV ledger for the named daughters.",
            false,
        ),
        (
            "spontaneous_fission_branches_to_leakage",
            "spontaneous-fission branches routed out of the chain",
            "Spontaneous fission is not followed; the branches are counted as leakage.",
            "Only matters for actinide-bearing materials; use an actinide-capable setup.",
            false,
        ),
        (
            "library_target_limitations",
            "library target limitations",
            "ACTINV records evaluation limits for these targets (for example reconstructed resonances).",
            "Read the named limitation in the ACTINV ledger before relying on these reactions.",
            false,
        ),
    ];
    let mut gaps = Vec::new();
    for (key, category, effect, next_step, lower_bound) in table {
        let (count, items, amount) = listed(ledger.get(key));
        if count > 0 {
            gaps.push(DataGap {
                category: format!("{category} ({key})"),
                count,
                items,
                amount_atoms_per_g: amount,
                effect: effect.into(),
                next_step: next_step.into(),
                lower_bound,
            });
        }
    }
    if final_leakage_atoms_per_g > 0.0 {
        gaps.push(DataGap {
            category: "atoms routed outside the represented chain (leakage_atoms_per_g)".into(),
            count: 1,
            items: Vec::new(),
            amount_atoms_per_g: Some(final_leakage_atoms_per_g),
            effect: "These atoms are not decayed or counted; totals are lower bounds.".into(),
            next_step: "Read the ACTINV ledger for the routes; add yields or decay data.".into(),
            lower_bound: true,
        });
    }
    gaps
}

fn zone_not_evaluated(has_dose: bool, has_pathways: bool) -> Vec<NotEvaluated> {
    let mut out = vec![
        not_evaluated(
            "decay heat per nuclide (alpha and beta parts)",
            "ACTINV reports decay heat by radiation type for the whole inventory, and decay-photon power per nuclide, but not total or alpha and beta heat per nuclide.",
            "Needs the ACTINV feature that returns heat per nuclide and radiation type (see the ACTINV feature list in docs/guide/activation.md).",
        ),
        not_evaluated(
            "pathway contribution to decay heat or dose",
            "ACTINV ranks production paths by atoms of the product, not by its decay heat or dose.",
            "Needs ACTINV pathway attribution weighted by heat and dose (see docs/guide/activation.md).",
        ),
    ];
    if !has_dose {
        out.push(not_evaluated(
            "contact dose rate (total and per nuclide)",
            "no photon response table was given, so ACTINV does not compute the contact dose proxy.",
            "Pass --photon-response with an actinv-photon-response-1 file and its SHA-256 (build one with ACTINV's scripts/build_photon_response.py).",
        ));
    }
    if !has_pathways {
        out.push(not_evaluated(
            "production pathways",
            "ACTINV gave no pathway list for this run (only trace mode with no feed or removal attributes pathways).",
            "Use a flux low enough for trace mode, or read the inventory without pathways.",
        ));
    }
    out
}

fn reduce_zone(
    zone: &ZoneInput,
    request: &ActivationRequest,
    problem_sha256: &str,
    result: &Value,
    decay_pins: &Pins,
) -> Result<ZoneResult, String> {
    let steps = field(result, "steps", "the result")?
        .as_array()
        .ok_or("ACTINV result: steps is not a list")?;
    let n_irr = request.steps.len();
    let expected = n_irr + request.cooling_s.len();
    if steps.len() != expected {
        return Err(format!(
            "ACTINV returned {} steps for a schedule of {expected}; the run is not usable",
            steps.len()
        ));
    }

    // Hand-off checks: what ACTINV says it received equals what was sent.
    let ledger = result.get("ledger").unwrap_or(&Value::Null);
    let sent_total = if zone.spectrum.is_empty() {
        0.0
    } else {
        zone.spectrum.total()
    };
    let got_total = number(
        &ledger["assembly"]["flux_total"],
        "ledger.assembly.flux_total",
    )?;
    if relative_difference(sent_total, got_total) > HANDOFF_TOLERANCE {
        return Err(format!(
            "zone {}: ACTINV received a total flux of {got_total:e} n/cm2/s but {sent_total:e} was sent; \
             the spectrum was changed in the hand-off, do not use this run",
            zone.id
        ));
    }
    let weighted_s: f64 = request
        .steps
        .iter()
        .map(|s| s.duration_s * s.flux_scale)
        .sum();
    let sent_fluence = sent_total * weighted_s;
    let irr_end = &steps[n_irr - 1];
    let got_fluence = number(&irr_end["fluence_n_cm2"], "fluence_n_cm2")?;
    if relative_difference(sent_fluence, got_fluence) > HANDOFF_TOLERANCE {
        return Err(format!(
            "zone {}: ACTINV's fluence {got_fluence:e} n/cm2 differs from the schedule's {sent_fluence:e}; \
             the irradiation history was changed in the hand-off",
            zone.id
        ));
    }
    let certificate = result.get("certificate").unwrap_or(&Value::Null);
    let recorded = |name: &str| {
        certificate["inputs"][name]["sha256"]
            .as_str()
            .map(str::to_string)
    };
    if recorded("decay_primary").as_deref() != Some(decay_pins.primary_sha.as_str()) {
        return Err(
            "ACTINV used a decay file whose SHA-256 differs from the one FARIS pinned".into(),
        );
    }
    if let Some(want) = &decay_pins.fallback_sha
        && recorded("decay_fallback").as_deref() != Some(want.as_str())
    {
        return Err(
            "ACTINV used a fallback decay file whose SHA-256 differs from the one FARIS pinned"
                .into(),
        );
    }

    let irr_end_s = request.irradiation_s();
    let volume_m3 = zone.volume_m3;
    let mass = zone.mass_g;
    let pathway_lists = result.get("pathways").and_then(Value::as_array);
    let has_pathways = pathway_lists.is_some_and(|p| p.len() == steps.len());
    let mut has_dose = false;
    let mut held_constant: BTreeSet<String> = BTreeSet::new();

    let mut points = Vec::with_capacity(request.cooling_s.len());
    for (j, &cooling_s) in request.cooling_s.iter().enumerate() {
        let index = n_irr + j;
        let step = &steps[index];
        let t_s = number(&step["t_s"], "t_s")?;
        if relative_difference(t_s, irr_end_s + cooling_s) > HANDOFF_TOLERANCE {
            return Err(format!(
                "zone {}: ACTINV's step {} ends at {t_s} s, expected {} s; the cooling grid was changed",
                zone.id,
                index + 1,
                irr_end_s + cooling_s
            ));
        }
        let heat = &step["heat_W_per_g"];
        let scale = mass / volume_m3;
        let total = number(&heat["total"], "heat_W_per_g.total")?;
        let (a, b, g) = (
            number(&heat["alpha"], "heat_W_per_g.alpha")?,
            number(&heat["beta"], "heat_W_per_g.beta")?,
            number(&heat["gamma"], "heat_W_per_g.gamma")?,
        );
        let decay_heat = DecayHeat {
            total_w_per_m3: total * scale,
            alpha_w_per_m3: a * scale,
            beta_w_per_m3: b * scale,
            gamma_w_per_m3: g * scale,
            total_kw: total * mass / 1000.0,
            parts_closure_relative: if total > 0.0 {
                (total - (a + b + g)).abs() / total
            } else {
                0.0
            },
        };

        let photon = &step["photon_source"];
        let by_nuclide: BTreeMap<&str, &Value> = photon["by_nuclide"]
            .as_array()
            .map(|l| {
                l.iter()
                    .filter_map(|n| Some((n["nuclide"].as_str()?, n)))
                    .collect()
            })
            .unwrap_or_default();
        let activity = step["activity_Bq_per_g"].as_object().ok_or(
            "ACTINV result: activity_Bq_per_g is missing; the run did not return activity",
        )?;
        let inventory = step["inventory"]
            .as_array()
            .ok_or("ACTINV result: inventory is missing")?;
        let mut nuclides = Vec::with_capacity(inventory.len());
        let mut seen = BTreeSet::new();
        let starting = starting_nuclides(zone);
        for n in inventory {
            let name = n["nuclide"]
                .as_str()
                .ok_or("ACTINV result: nuclide without a name")?;
            seen.insert(name);
            let atoms = Some(number(&n["atoms_per_g"], "atoms_per_g")? * mass);
            let act = activity
                .get(name)
                .map_or(Ok(0.0), |v| number(v, "activity"))?
                * mass;
            let ph = by_nuclide.get(name);
            let dose = ph.and_then(|p| p["contact_gamma_air_dose_proxy_Gy_h"].as_f64());
            has_dose |= dose.is_some();
            nuclides.push(NuclideState {
                nuclide: name.to_string(),
                atoms,
                activity_bq: act,
                photon_power_w: ph
                    .and_then(|p| p["source_power_W_g"].as_f64())
                    .map(|w| w * mass),
                contact_dose_proxy_gy_h: dose,
            });
        }
        // An activity with no inventory row is a nuclide silently dropped, unless it is a
        // starting-material nuclide the zone named itself (a constant reservoir in trace mode).
        for (name, value) in activity {
            if seen.contains(name.as_str()) {
                continue;
            }
            if !starting.contains(&canonical_nuclide(name)) {
                return Err(format!(
                    "ACTINV result: nuclide {name} has an activity but no inventory row"
                ));
            }
            held_constant.insert(name.clone());
            let ph = by_nuclide.get(name.as_str());
            nuclides.push(NuclideState {
                nuclide: name.clone(),
                atoms: None,
                activity_bq: number(value, "activity")? * mass,
                photon_power_w: ph
                    .and_then(|p| p["source_power_W_g"].as_f64())
                    .map(|w| w * mass),
                contact_dose_proxy_gy_h: ph
                    .and_then(|p| p["contact_gamma_air_dose_proxy_Gy_h"].as_f64()),
            });
        }
        nuclides.sort_by(|x, y| {
            y.activity_bq
                .total_cmp(&x.activity_bq)
                .then(y.atoms.unwrap_or(0.0).total_cmp(&x.atoms.unwrap_or(0.0)))
                .then(x.nuclide.cmp(&y.nuclide))
        });
        let activity_bq: f64 = nuclides.iter().map(|n| n.activity_bq).sum();

        let groups: Vec<PhotonGroup> = photon["groups"]
            .as_array()
            .ok_or("ACTINV result: photon groups are missing")?
            .iter()
            .map(|gp| {
                Ok(PhotonGroup {
                    low_ev: number(&gp["low_eV"], "low_eV")?,
                    high_ev: number(&gp["high_eV"], "high_eV")?,
                    centroid_ev: number(&gp["centroid_eV"], "centroid_eV")?,
                    photons_per_s: number(&gp["photons_s"], "photons_s")?,
                    power_w: number(&gp["power_W"], "power_W")?,
                })
            })
            .collect::<Result<_, String>>()?;
        let total_photons = number(&photon["total_photons_s"], "total_photons_s")?;
        let grouped = number(&photon["grouped_photons_s"], "grouped_photons_s")?;
        let photon_source = PhotonSource {
            groups,
            total_photons_per_s: total_photons,
            grouped_photons_per_s: grouped,
            photons_outside_groups_per_s: total_photons - grouped,
            power_w: number(&photon["source_power_W"], "source_power_W")?,
            represented_gamma_power_fraction: number(
                &photon["represented_gamma_power_fraction"],
                "represented_gamma_power_fraction",
            )?,
            contact_dose_proxy_gy_h: photon["contact_gamma_air_dose_proxy_Gy_h"].as_f64(),
        };

        let top = |key: fn(&NuclideState) -> f64| -> Vec<String> {
            let mut order: Vec<&NuclideState> = nuclides.iter().filter(|n| key(n) > 0.0).collect();
            order.sort_by(|x, y| key(y).total_cmp(&key(x)).then(x.nuclide.cmp(&y.nuclide)));
            order
                .iter()
                .take(PATHWAY_TOP)
                .map(|n| n.nuclide.clone())
                .collect()
        };
        let top_by_activity = top(|n| n.activity_bq);
        let top_by_photon_power = top(|n| n.photon_power_w.unwrap_or(0.0));
        let mut pathways = BTreeMap::new();
        if has_pathways && let Some(map) = pathway_lists.and_then(|l| l[index].as_object()) {
            for name in top_by_activity.iter().chain(&top_by_photon_power) {
                if let Some(list) = map.get(name) {
                    let list: Vec<Pathway> = serde_json::from_value(list.clone())
                        .map_err(|e| format!("ACTINV result: pathways of {name}: {e}"))?;
                    pathways.insert(name.clone(), list);
                }
            }
        }

        let leakage = step["leakage_atoms_per_g"].as_f64().unwrap_or(0.0) * mass;
        points.push(CoolingPoint {
            cooling_s,
            kind: Kind::Calculated,
            decay_heat,
            activity_bq,
            activity_bq_per_m3: activity_bq / volume_m3,
            leakage_atoms: leakage,
            nuclides,
            nuclides_omitted: 0,
            photon_source,
            top_by_activity,
            top_by_photon_power,
            pathways,
        });
    }

    let final_leakage = steps
        .last()
        .and_then(|s| s["leakage_atoms_per_g"].as_f64())
        .unwrap_or(0.0);
    let mut gaps = data_gaps(result, final_leakage);
    if result["mode"].as_str() == Some("trace") && !held_constant.is_empty() {
        gaps.push(DataGap {
            category: "starting-material radionuclides held constant (trace mode)".into(),
            count: held_constant.len(),
            items: held_constant.iter().take(MAX_LISTED_ITEMS).cloned().collect(),
            amount_atoms_per_g: None,
            effect: "ACTINV's trace mode does not decay the starting material, so these nuclides keep their starting activity and the totals are overstated.".into(),
            next_step: "Rerun with the coupled solver mode (--mode coupled).".into(),
            lower_bound: false,
        });
    }
    let completeness = if gaps.iter().any(|g| g.lower_bound) {
        Completeness::LowerBound
    } else {
        Completeness::Complete
    };
    if completeness == Completeness::LowerBound {
        for p in &mut points {
            p.kind = Kind::Conditional;
        }
    }
    let mut messages = Vec::new();
    if zone.spectrum.is_empty() {
        messages.push(
            "The spectrum is empty, so the flux is zero: nothing is activated and the inventory is only what decays from the starting material.".to_string(),
        );
    } else if sent_total == 0.0 {
        messages.push(
            "Every group flux is zero: nothing is activated and the inventory is only what decays from the starting material.".to_string(),
        );
    }
    if zone.spectrum.energy_edges_ev.first() == Some(&0.0) {
        messages.push(format!(
            "The first energy edge 0 eV was sent to ACTINV as {LOWEST_EDGE_EV:e} eV, the library's first boundary."
        ));
    }
    let (mode_sent, mode_substitution) = effective_mode(zone, request.mode);
    let composition_sha = sha256_json(&(zone.basis, &zone.composition))?;
    let spectrum_sha = sha256_json(&(zone.spectrum.actinv_edges(), &zone.spectrum.flux_per_cm2_s))?;
    Ok(ZoneResult {
        id: zone.id.clone(),
        input: ZoneInputRecord {
            mass_g: mass,
            volume_m3,
            kind: Kind::Authored,
            basis: zone.basis,
            composition_sha256: composition_sha,
            spectrum_sha256: spectrum_sha,
            spectrum_total_flux_cm2_s: sent_total,
            spectrum_total_flux_actinv_cm2_s: got_total,
            fluence_n_cm2: got_fluence,
            solver_mode_sent: mode_sent,
            mode_substitution,
            messages,
        },
        run: RunRecord {
            problem_sha256: problem_sha256.to_string(),
            solver: certificate["solver"]
                .as_str()
                .unwrap_or("unknown")
                .to_string(),
            mode: result["mode"].as_str().unwrap_or("unknown").to_string(),
            total_states: result["total_states"].as_u64().unwrap_or(0),
            pruned_states: result["pruned_states"].as_u64().unwrap_or(0),
        },
        completeness,
        points,
        gaps,
        not_evaluated: zone_not_evaluated(has_dose, has_pathways),
    })
}

/// Reads an ACTINV result document (already parsed) for one zone. Public so a recorded result
/// can be checked without running ACTINV.
pub fn zone_from_actinv_result(
    zone: &ZoneInput,
    request: &ActivationRequest,
    data: &ActivationData,
    problem_sha256: &str,
    result: &Value,
) -> Result<ZoneResult, String> {
    let pins = pins(data)?;
    reduce_zone(zone, request, problem_sha256, result, &pins)
}

// ------------------------------------------------------------------ running --

fn environment(config: &ActivationConfig) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    let mut env: Vec<_> = ["PATH", "HOME", "LANG", "LC_ALL", "TMPDIR"]
        .into_iter()
        .filter_map(|k| std::env::var_os(k).map(|v| (k.into(), v)))
        .collect();
    if let Some(root) = config.data.library.ancestors().nth(3) {
        env.push(("ACTINV_DATA_DIR".into(), root.as_os_str().to_owned()));
    }
    env
}

fn run_actinv(
    config: &ActivationConfig,
    actinv: &Path,
    problem: &Path,
    result: &Path,
    scratch: &Path,
    cancel: &Cancellation,
) -> Result<(), String> {
    let spec = JobSpec {
        program: actinv.to_owned(),
        arguments: vec!["run".into(), problem.into(), result.into()],
        working_directory: scratch.to_owned(),
        environment: environment(config),
        timeout: config.timeout,
        capture_limit_bytes: 1 << 20,
        artifact_roots: Vec::new(),
        resource_limits: config.resource_limits,
    };
    let outcome = run_job(&spec, cancel).map_err(|e| format!("actinv run: {e}"))?;
    match outcome.execution_status {
        ExecutionStatus::Succeeded => Ok(()),
        ExecutionStatus::Cancelled => Err("cancelled".into()),
        status => Err(format!(
            "actinv run failed ({status:?}, exit {:?}): {}",
            outcome.exit_code,
            tail_of(&format!("{}\n{}", outcome.stdout, outcome.stderr))
        )),
    }
}

fn cache_path(config: &ActivationConfig, key: &str) -> Option<PathBuf> {
    config
        .cache_dir
        .as_ref()
        .map(|d| d.join(format!("{key}.zone.json")))
}

#[derive(Deserialize, Serialize)]
struct CacheFile {
    schema: String,
    key: String,
    zone: ZoneResult,
}

fn read_cache(path: &Path, key: &str) -> Option<ZoneResult> {
    let bytes = std::fs::read(path).ok()?;
    let file: CacheFile = serde_json::from_slice(&bytes).ok()?;
    (file.schema == CACHE_SCHEMA && file.key == key).then_some(file.zone)
}

/// Runs the request: one ACTINV problem per zone, in zone order, cache first.
pub fn run_activation(
    request: &ActivationRequest,
    config: &ActivationConfig,
    cancel: &Cancellation,
) -> Result<(ActivationResult, RunStats), String> {
    request.validate()?;
    let actinv = resolve_program(&config.actinv)?;
    let pins = pins(&config.data)?;
    let mut stats = RunStats::default();
    let mut zones = Vec::with_capacity(request.zones.len());
    let mut solver = None;
    for zone in &request.zones {
        let problem = build_problem(zone, request, &config.data, &pins.library_sha);
        let problem_bytes = serde_json::to_vec_pretty(&problem).map_err(|e| e.to_string())?;
        let problem_sha = hex_sha256(&problem_bytes);
        let key = hex_sha256(
            format!(
                "{CACHE_SCHEMA}\0{problem_sha}\0{}\0{}",
                pins.primary_sha,
                pins.fallback_sha.clone().unwrap_or_default()
            )
            .as_bytes(),
        );
        let cached = cache_path(config, &key).and_then(|p| read_cache(&p, &key));
        let zone_result = match cached {
            Some(z) => {
                stats.cache_hits += 1;
                z
            }
            None => {
                if cancel.is_cancelled() {
                    return Err("cancelled".into());
                }
                let scratch = config.work_dir.join(&key[..16]);
                let _ = std::fs::remove_dir_all(&scratch);
                std::fs::create_dir_all(&scratch)
                    .map_err(|e| format!("{}: {e}", scratch.display()))?;
                let problem_path = scratch.join("problem.json");
                let result_path = scratch.join("result.json");
                std::fs::write(&problem_path, &problem_bytes)
                    .map_err(|e| format!("{}: {e}", problem_path.display()))?;
                let ran = run_actinv(config, &actinv, &problem_path, &result_path, &scratch, cancel)
                    .and_then(|()| {
                        let size = std::fs::metadata(&result_path)
                            .map_err(|e| format!("{}: {e}", result_path.display()))?
                            .len();
                        if size > MAX_RESULT_BYTES {
                            return Err(format!(
                                "the ACTINV result is {size} bytes, over the {MAX_RESULT_BYTES} limit; shorten the schedule or the cooling grid"
                            ));
                        }
                        let bytes = std::fs::read(&result_path)
                            .map_err(|e| format!("{}: {e}", result_path.display()))?;
                        serde_json::from_slice::<Value>(&bytes)
                            .map_err(|e| format!("ACTINV result is not JSON: {e}"))
                    });
                let _ = std::fs::remove_dir_all(&scratch);
                stats.actinv_runs += 1;
                let value = ran.map_err(|e| format!("zone {}: {e}", zone.id))?;
                let z = reduce_zone(zone, request, &problem_sha, &value, &pins)?;
                if let Some(path) = cache_path(config, &key) {
                    let file = CacheFile {
                        schema: CACHE_SCHEMA.into(),
                        key: key.clone(),
                        zone: z.clone(),
                    };
                    let mut bytes = serde_json::to_vec(&file).map_err(|e| e.to_string())?;
                    bytes.push(b'\n');
                    // A cache that cannot be written costs time, not correctness.
                    let _ = std::fs::create_dir_all(crate::atomic_file::parent_directory(&path));
                    let _ = crate::atomic_file::write_atomic(&path, &bytes);
                }
                z
            }
        };
        match &solver {
            None => solver = Some(zone_result.run.solver.clone()),
            Some(s) if *s != zone_result.run.solver => {
                return Err(format!(
                    "zones were solved by different ACTINV versions ({s} and {})",
                    zone_result.run.solver
                ));
            }
            _ => {}
        }
        zones.push(zone_result);
    }
    Ok((
        assemble(
            request,
            &config.data,
            &pins,
            solver.unwrap_or_default(),
            zones,
        )?,
        stats,
    ))
}

fn assemble(
    request: &ActivationRequest,
    data: &ActivationData,
    pins: &Pins,
    solver: String,
    zones: Vec<ZoneResult>,
) -> Result<ActivationResult, String> {
    let versions = DataVersions {
        solver,
        library: DataVersion {
            id: data.library_id.clone(),
            sha256: pins.library_sha.clone(),
        },
        decay_primary: DataVersion {
            id: data.decay_primary_id.clone(),
            sha256: pins.primary_sha.clone(),
        },
        decay_fallback: match (&data.decay_fallback, &pins.fallback_sha) {
            (Some((_, id)), Some(sha)) => Some(DataVersion {
                id: id.clone(),
                sha256: sha.clone(),
            }),
            _ => None,
        },
    };
    let schedule = ScheduleRecord {
        kind: Kind::Authored,
        steps: request.steps.len(),
        irradiation_s: request.irradiation_s(),
        steps_sha256: sha256_json(&request.steps)?,
        source_input_sha256: request.schedule_input_sha256.clone(),
    };
    let totals = request
        .cooling_s
        .iter()
        .enumerate()
        .map(|(j, &cooling_s)| {
            let lower = zones
                .iter()
                .any(|z| z.completeness == Completeness::LowerBound);
            TotalPoint {
                cooling_s,
                kind: if lower {
                    Kind::Conditional
                } else {
                    Kind::Calculated
                },
                completeness: if lower {
                    Completeness::LowerBound
                } else {
                    Completeness::Complete
                },
                decay_heat_kw: zones.iter().map(|z| z.points[j].decay_heat.total_kw).sum(),
                activity_bq: zones.iter().map(|z| z.points[j].activity_bq).sum(),
                photons_per_s: zones
                    .iter()
                    .map(|z| z.points[j].photon_source.total_photons_per_s)
                    .sum(),
            }
        })
        .collect();
    let input_sha256 = sha256_json(&json!({
        "schema": RESULT_SCHEMA,
        "data": versions,
        "schedule": schedule,
        "cooling_s": request.cooling_s,
        "problems": zones.iter().map(|z| (&z.id, &z.run.problem_sha256)).collect::<Vec<_>>(),
    }))?;
    Ok(ActivationResult {
        schema: RESULT_SCHEMA.into(),
        research_screening: faris_model::RESEARCH_SCREENING_STATEMENT.into(),
        label: "research screening: activation inventory, decay heat and decay-photon source from ACTINV; not a waste, licensing or dose-compliance statement".into(),
        data: versions,
        schedule,
        cooling_s: request.cooling_s.clone(),
        zones,
        totals,
        not_evaluated: vec![not_evaluated(
            "decay-only recompute from a stored inventory",
            "ACTINV cannot start from a stored inventory, so a changed cooling grid reruns the whole problem (seconds); only an identical request is served from the cache.",
            "Needs the ACTINV feature that continues from a saved inventory (see docs/guide/activation.md).",
        )],
        input_sha256,
    })
}

/// Keeps the `keep` most active nuclides of every cooling point (ordering: activity, then
/// atoms); totals are untouched and the number left out is recorded.
pub fn limit_nuclides(result: &mut ActivationResult, keep: usize) {
    for z in &mut result.zones {
        for p in &mut z.points {
            if p.nuclides.len() > keep {
                p.nuclides_omitted = p.nuclides.len() - keep;
                p.nuclides.truncate(keep);
            }
        }
    }
}

// --------------------------------------------------------- photon source file --

/// The decay-photon source of one zone at one cooling time, in the form a shutdown-dose
/// transport run reads (ACT-029). Rates are per cm3 of the zone.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PhotonSourceFile {
    pub schema: String,
    pub research_screening: String,
    pub zone_id: String,
    pub cooling_s: f64,
    pub unit: String,
    pub normalization: String,
    pub volume_cm3: f64,
    pub energy_edges_ev: Vec<f64>,
    pub photons_per_s_per_cm3: Vec<f64>,
    /// Photons per second from the whole zone, all lines (ACTINV).
    pub total_photons_per_s: f64,
    pub photons_outside_groups_per_s: f64,
    /// Identity of the activation result the source came from.
    pub activation_input_sha256: String,
    pub problem_sha256: String,
}

pub fn photon_source_file(
    result: &ActivationResult,
    zone_id: &str,
    cooling_s: f64,
) -> Result<PhotonSourceFile, String> {
    let zone = result
        .zones
        .iter()
        .find(|z| z.id == zone_id)
        .ok_or_else(|| format!("no zone {zone_id} in the activation result"))?;
    let point = zone
        .points
        .iter()
        .find(|p| p.cooling_s == cooling_s)
        .ok_or_else(|| {
            format!("zone {zone_id} has no cooling time {cooling_s} s; the result has only the requested grid")
        })?;
    let groups = &point.photon_source.groups;
    if groups.is_empty() {
        return Err("the result has no photon groups".into());
    }
    if groups.windows(2).any(|w| w[0].high_ev != w[1].low_ev) {
        return Err("photon groups are not contiguous".into());
    }
    let volume_cm3 = zone.input.volume_m3 * 1.0e6;
    let mut edges: Vec<f64> = groups.iter().map(|g| g.low_ev).collect();
    edges.push(groups[groups.len() - 1].high_ev);
    Ok(PhotonSourceFile {
        schema: PHOTON_SOURCE_SCHEMA.into(),
        research_screening: faris_model::RESEARCH_SCREENING_STATEMENT.into(),
        zone_id: zone_id.into(),
        cooling_s,
        unit: "photons/s/cm3".into(),
        normalization: "group photon emission rate of the whole zone divided by the zone volume; multiply by volume_cm3 to recover the zone rate".into(),
        volume_cm3,
        energy_edges_ev: edges,
        photons_per_s_per_cm3: groups.iter().map(|g| g.photons_per_s / volume_cm3).collect(),
        total_photons_per_s: point.photon_source.total_photons_per_s,
        photons_outside_groups_per_s: point.photon_source.photons_outside_groups_per_s,
        activation_input_sha256: result.input_sha256.clone(),
        problem_sha256: zone.run.problem_sha256.clone(),
    })
}

/// Writes the source file (refusing to replace one) and returns its SHA-256.
pub fn write_photon_source_file(path: &Path, file: &PhotonSourceFile) -> Result<String, String> {
    let mut bytes = serde_json::to_vec_pretty(file).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    crate::atomic_file::write_new(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(hex_sha256(&bytes))
}

#[cfg(test)]
mod tests;
