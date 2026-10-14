//! The fusion power the design file's plasma profiles imply (SRC-010 closure).
//!
//! `plasma.fusion_power_mw` sets the source strength for transport; the ion density and
//! temperature profiles set only its shape. The two are separate inputs, so they can disagree.
//! This module integrates the D-T reaction-rate density of the profiles over the plasma volume,
//! with the profile, flux-surface and reactivity definitions `openmc_plasma_source`
//! (`tokamak_source`, after Fausser et al. 2012) uses, and compares the power it implies with the
//! declared one. It is pure Rust: it needs neither OpenMC nor the package.
//!
//! The package also counts D-D and T-T neutrons when the fuel has both D and T. They carry no
//! D-T fusion power, so they are left out here: this is the D-T power, which is what the
//! declared field means.

use faris_model::design::{Plasma, PlasmaMode};
use faris_model::math::{cos, exp, powf, powi, sin};
use serde::{Deserialize, Serialize};

/// Energy released per D-T reaction, eV (SRC-001; the same constant `transport.rs` divides by).
const ENERGY_PER_REACTION_EV: f64 = 17.6e6;
const ELEMENTARY_CHARGE_C: f64 = 1.602_176_634e-19;
/// The relative difference above which the declared power and the profiles are said to disagree.
pub const CLOSURE_TOLERANCE: f64 = 1.0e-3;
/// Midpoint-rule cells in the minor radius and in the poloidal angle. At 1000 x 1000 the RM-M
/// integral is within 1e-6 relative of the 2000 x 2000 one.
const RADIAL_CELLS: usize = 1000;
const ANGULAR_CELLS: usize = 1000;

/// What the profiles imply, next to what the design file declares.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SourceClosure {
    pub declared_fusion_power_mw: f64,
    /// D-T fusion power of the profile integral, MW.
    pub implied_fusion_power_mw: f64,
    /// D-T reactions per second of the profile integral.
    pub implied_reaction_rate_per_s: f64,
    /// `implied / declared`.
    pub ratio: f64,
    /// `|implied - declared| / declared`.
    pub relative_difference: f64,
    pub tolerance: f64,
    /// `relative_difference <= tolerance`.
    pub closes: bool,
}

impl SourceClosure {
    /// One line with both numbers, for the report.
    pub fn summary(&self) -> String {
        format!(
            "the profiles imply {:.2} MW of D-T fusion power; the design file declares {:.2} MW \
             (ratio {:.4}, difference {:.3} % against a limit of {:.1} %)",
            self.implied_fusion_power_mw,
            self.declared_fusion_power_mw,
            self.ratio,
            100.0 * self.relative_difference,
            100.0 * self.tolerance
        )
    }

    /// What the disagreement means, when the two do not close: both powers, the ratio, and what
    /// transport takes from each.
    pub fn disagreement(&self) -> String {
        format!(
            "{}. Transport takes the source strength from the declared power alone and uses the \
             profiles only for its shape, so every flux, heating and fluence number follows the \
             declared {:.2} MW while the plasma the profiles describe would give {:.2} MW. FARIS \
             does not rescale either one.",
            self.summary(),
            self.declared_fusion_power_mw,
            self.implied_fusion_power_mw
        )
    }

    /// The next step when the two do not close.
    pub fn next_step() -> &'static str {
        "Correct the profiles (ion density, ion temperature, peaking) so they give the declared \
         power, or correct plasma.fusion_power_mw if the profiles are the intended plasma. If the \
         profile is only an approximate shape for a known fusion power, accept the difference with \
         a waiver for SRC-010 that names the plasma chamber solid in solid_ids and gives the reason."
    }
}

/// `tokamak_ion_density` of the package, m^-3, at minor radius `a` (same units as `minor`).
fn ion_density(p: &Plasma, a: f64, minor: f64, pedestal: f64) -> f64 {
    let d = &p.ion_density_m3;
    let (centre, peaking) = (d.centre.unwrap_or(0.0), d.peaking_factor.unwrap_or(0.0));
    match p.mode {
        Some(PlasmaMode::L) => centre * powf(1.0 - powi(a / minor, 2), peaking),
        _ => {
            let (ped, sep) = (d.pedestal.unwrap_or(0.0), d.separatrix.unwrap_or(0.0));
            if a < pedestal {
                (centre - ped) * powf(1.0 - powi(a / pedestal, 2), peaking) + ped
            } else {
                (ped - sep) * (minor - a) / (minor - pedestal) + sep
            }
        }
    }
}

/// `tokamak_ion_temperature` of the package, eV.
fn ion_temperature_ev(p: &Plasma, a: f64, minor: f64, pedestal: f64) -> f64 {
    let t = &p.ion_temperature_ev;
    let (centre, peaking) = (t.centre.unwrap_or(0.0), t.peaking_factor.unwrap_or(0.0));
    match p.mode {
        Some(PlasmaMode::L) => centre * powf(1.0 - powi(a / minor, 2), peaking),
        _ => {
            let (ped, sep, beta) = (
                t.pedestal.unwrap_or(0.0),
                t.separatrix.unwrap_or(0.0),
                t.beta.unwrap_or(0.0),
            );
            if a < pedestal {
                ped + (centre - ped) * powf(1.0 - powf(a / pedestal, beta), peaking)
            } else {
                sep + (ped - sep) * (minor - a) / (minor - pedestal)
            }
        }
    }
}

/// D-T reactivity, m^3/s, at an ion temperature in eV: the Bosch-Hale fit as `NeSST.reac_DT`
/// (Atzeni and Meyer-ter-Vehn) evaluates it, including its truncated exponents 0.333333333 and
/// 0.833333333.
pub fn reactivity_dt(temperature_ev: f64) -> f64 {
    let kev = temperature_ev / 1.0e3;
    if kev.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
        return 0.0;
    }
    let numerator = ((-0.10675e-3 * kev + 4.6064e-3) * kev + 15.136e-3) * kev;
    let denominator = ((0.01366e-3 * kev + 13.5e-3) * kev + 75.189e-3) * kev + 1.0;
    let xi = 6.6610 * powf(kev, -0.333333333);
    let eta = 1.0 - numerator / denominator;
    643.41e-22 * powf(eta, -0.833333333) * xi * xi * exp(-3.0 * powf(eta, 0.333333333) * xi)
}

fn positive(name: &str, value: Option<f64>) -> Result<f64, String> {
    match value {
        Some(v) if v.is_finite() && v > 0.0 => Ok(v),
        other => Err(format!("plasma.{name} is {other:?}, not a positive number")),
    }
}

/// Integrate the D-T reaction-rate density of the plasma block over its volume and compare the
/// power with `fusion_power_mw`. An error says which plasma field cannot be used.
pub fn closure(plasma: &Plasma) -> Result<SourceClosure, String> {
    let major = positive("major_radius_m", plasma.major_radius_m)?;
    let minor = positive("minor_radius_m", plasma.minor_radius_m)?;
    let elongation = positive("elongation", plasma.elongation)?;
    let declared = positive("fusion_power_mw", plasma.fusion_power_mw)?;
    let pedestal = plasma.pedestal_radius_m.unwrap_or(0.0);
    let triangularity = plasma.triangularity.unwrap_or(0.0);
    let shift = plasma.shafranov_shift_m.unwrap_or(0.0);
    if plasma.mode.is_none() {
        return Err("plasma.mode is missing".into());
    }
    if minor >= major {
        return Err("plasma.minor_radius_m is not smaller than plasma.major_radius_m".into());
    }
    if triangularity.is_nan() || triangularity.abs() >= 1.0 {
        return Err("plasma.triangularity is not between -1 and 1".into());
    }
    if shift.is_nan() || shift.abs() >= 0.5 * minor {
        return Err("plasma.shafranov_shift_m is not smaller than half the minor radius".into());
    }
    if plasma.mode != Some(PlasmaMode::L) && pedestal.is_nan()
        || pedestal <= 0.0
        || pedestal >= minor
    {
        return Err("plasma.pedestal_radius_m is not between 0 and the minor radius".into());
    }
    let fuel = plasma.fuel.as_ref();
    let fraction = |isotope: &str| fuel.and_then(|f| f.get(isotope)).copied().unwrap_or(0.0);
    let (deuterium, tritium) = (fraction("D"), fraction("T"));

    // The flux-surface coordinates of the package: R = R0 + a cos(alpha + delta sin alpha)
    // + shift (1 - (a/a0)^2), Z = kappa a sin(alpha); the volume element is 2 pi R |J| da dalpha.
    let da = minor / RADIAL_CELLS as f64;
    let dalpha = std::f64::consts::TAU / ANGULAR_CELLS as f64;
    let mut sum = 0.0;
    for i in 0..RADIAL_CELLS {
        let a = (i as f64 + 0.5) * da;
        let rate = powi(ion_density(plasma, a, minor, pedestal), 2)
            * deuterium
            * tritium
            * reactivity_dt(ion_temperature_ev(plasma, a, minor, pedestal));
        if !rate.is_finite() {
            return Err(format!(
                "the profiles give a non-finite reaction rate at minor radius {a:.4} m"
            ));
        }
        let mut ring = 0.0;
        for j in 0..ANGULAR_CELLS {
            let alpha = (j as f64 + 0.5) * dalpha;
            let theta = alpha + triangularity * sin(alpha);
            let radius = major + a * cos(theta) + shift * (1.0 - powi(a / minor, 2));
            let d_r_da = cos(theta) - 2.0 * shift * a / (minor * minor);
            let d_r_dalpha = -a * sin(theta) * (1.0 + triangularity * cos(alpha));
            let jacobian =
                (d_r_da * elongation * a * cos(alpha) - d_r_dalpha * elongation * sin(alpha)).abs();
            ring += radius * jacobian;
        }
        sum += rate * ring;
    }
    let reactions_per_s = sum * da * dalpha * std::f64::consts::TAU;
    let implied_mw = reactions_per_s * ENERGY_PER_REACTION_EV * ELEMENTARY_CHARGE_C / 1.0e6;
    let relative_difference = (implied_mw - declared).abs() / declared;
    Ok(SourceClosure {
        declared_fusion_power_mw: declared,
        implied_fusion_power_mw: implied_mw,
        implied_reaction_rate_per_s: reactions_per_s,
        ratio: implied_mw / declared,
        relative_difference,
        tolerance: CLOSURE_TOLERANCE,
        closes: relative_difference <= CLOSURE_TOLERANCE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use faris_model::design::Design;

    const RM_M: &str = include_str!("../../../../scenarios/rm-m/rm-m.faris-design.json");

    fn rm_m_plasma() -> Plasma {
        serde_json::from_str::<Design>(RM_M).unwrap().plasma
    }

    /// A circular, uniform L-mode plasma, whose reaction rate is the same everywhere.
    fn uniform(elongation: f64, power_mw: f64) -> Plasma {
        let mut p = rm_m_plasma();
        p.mode = Some(PlasmaMode::L);
        p.elongation = Some(elongation);
        p.triangularity = Some(0.0);
        p.shafranov_shift_m = Some(0.0);
        p.ion_density_m3.centre = Some(1.0e20);
        p.ion_density_m3.peaking_factor = Some(0.0);
        p.ion_temperature_ev.centre = Some(15_000.0);
        p.ion_temperature_ev.peaking_factor = Some(0.0);
        p.fusion_power_mw = Some(power_mw);
        p
    }

    // Verifies: SRC-010
    #[test]
    fn the_dt_reactivity_matches_the_nesst_fit() {
        // NeSST.spectral_model.reac_DT (Bosch-Hale), evaluated with the package's own code.
        for (ev, expected) in [
            (1_000.0, 6.857_069_187e-27),
            (10_000.0, 1.136_173_482e-22),
            (20_000.0, 4.330_220_374e-22),
        ] {
            let got = reactivity_dt(ev);
            assert!(
                (got / expected - 1.0).abs() < 1.0e-8,
                "{ev} eV: {got:e} against {expected:e}"
            );
        }
        assert_eq!(reactivity_dt(0.0), 0.0);
        assert_eq!(reactivity_dt(f64::NAN), 0.0);
    }

    // Verifies: SRC-010
    #[test]
    fn a_uniform_plasma_closes_on_the_closed_form_volume() {
        // V = 2 pi^2 R a^2 kappa, so P = n_D n_T <sigma v> E V with n_D = n_T = n / 2.
        let p = uniform(1.6, 1.0);
        let (r, a, k) = (3.3, 1.13, 1.6);
        let volume = 2.0 * powi(std::f64::consts::PI, 2) * r * a * a * k;
        let density = 1.0e20 / 2.0;
        let expected_mw = density
            * density
            * reactivity_dt(15_000.0)
            * volume
            * ENERGY_PER_REACTION_EV
            * ELEMENTARY_CHARGE_C
            / 1.0e6;
        let c = closure(&p).unwrap();
        assert!(
            (c.implied_fusion_power_mw / expected_mw - 1.0).abs() < 1.0e-5,
            "{} against {expected_mw}",
            c.implied_fusion_power_mw
        );
    }

    // Verifies: SRC-010
    #[test]
    fn a_declared_power_equal_to_the_profiles_closes() {
        let mut p = uniform(1.6, 1.0);
        let implied = closure(&p).unwrap().implied_fusion_power_mw;
        p.fusion_power_mw = Some(implied);
        let c = closure(&p).unwrap();
        assert!(c.closes && c.relative_difference < 1.0e-12, "{c:?}");
        // Just inside and just outside the 1e-3 limit.
        p.fusion_power_mw = Some(implied * 1.0009);
        assert!(closure(&p).unwrap().closes);
        p.fusion_power_mw = Some(implied * 1.0011);
        let c = closure(&p).unwrap();
        assert!(!c.closes);
        assert!(
            c.disagreement().contains("does not rescale"),
            "{}",
            c.disagreement()
        );
        assert!(SourceClosure::next_step().contains("waiver"));
    }

    /// RM-M's plasma block with the S0 model card's own ion densities, before they were scaled
    /// to close at the published fusion power.
    fn model_card_plasma() -> Plasma {
        let mut p = rm_m_plasma();
        p.ion_density_m3.centre = Some(1.5e20);
        p.ion_density_m3.pedestal = Some(1.1e20);
        p.ion_density_m3.separatrix = Some(4.0e19);
        p
    }

    // Verifies: SRC-010
    #[test]
    fn the_model_card_densities_imply_a_power_far_from_the_declared_525_mw() {
        // 190.546 MW, 6.75737e19 D-T reactions per second: the same profile formulas evaluated
        // with openmc_plasma_source's own functions on a 2000 x 2000 grid (a script outside the
        // repository), and checked against tokamak_source by test_source_sampling.py.
        let c = closure(&model_card_plasma()).unwrap();
        assert_eq!(c.declared_fusion_power_mw, 525.0);
        assert!(
            (c.implied_reaction_rate_per_s / 6.757372e19 - 1.0).abs() < 1.0e-5,
            "{c:?}"
        );
        assert!((c.implied_fusion_power_mw - 190.5465).abs() < 0.01, "{c:?}");
        assert!(!c.closes);
        let text = c.disagreement();
        assert!(
            text.contains("190.55 MW") && text.contains("525.00 MW"),
            "{text}"
        );
    }

    // Verifies: SRC-010
    #[test]
    fn the_rm_m_design_file_closes_at_its_published_525_mw() {
        let c = closure(&rm_m_plasma()).unwrap();
        assert_eq!(c.declared_fusion_power_mw, 525.0);
        assert!(c.closes, "{c:?}");
        // The rate test_source_sampling.py compares the package's strength with.
        assert!(
            (c.implied_reaction_rate_per_s / 1.861814e20 - 1.0).abs() < 1.0e-5,
            "{c:?}"
        );
        // The same shape as the model card's profile, scaled in level only.
        let (new, old) = (
            rm_m_plasma().ion_density_m3,
            model_card_plasma().ion_density_m3,
        );
        let factor = new.centre.unwrap() / old.centre.unwrap();
        assert!((factor - 1.65989).abs() < 1.0e-5, "{factor}");
        assert!((new.pedestal.unwrap() / old.pedestal.unwrap() / factor - 1.0).abs() < 1.0e-8);
        assert!((new.separatrix.unwrap() / old.separatrix.unwrap() / factor - 1.0).abs() < 1.0e-8);
    }

    // Verifies: SRC-010
    #[test]
    fn a_plasma_block_that_cannot_be_integrated_says_which_field() {
        let mut p = rm_m_plasma();
        p.pedestal_radius_m = Some(2.0);
        assert!(closure(&p).unwrap_err().contains("pedestal_radius_m"));
        let mut p = rm_m_plasma();
        p.fusion_power_mw = Some(0.0);
        assert!(closure(&p).unwrap_err().contains("fusion_power_mw"));
        let mut p = rm_m_plasma();
        p.fuel = Some([("D".to_string(), 1.0)].into());
        // No tritium: no D-T power, so it cannot close with a positive declared power.
        let c = closure(&p).unwrap();
        assert_eq!(c.implied_fusion_power_mw, 0.0);
        assert!(!c.closes);
    }
}
