// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
//! Unit conversion for coupled design heating. The OpenMC score remains raw eV per source
//! neutron until this boundary; sampled volume uncertainty is recorded separately from MC SE.

use super::DesignError;

pub const ELEMENTARY_CHARGE_C: f64 = 1.602_176_634e-19;

/// A heating score normalized once to operating power density.
#[derive(Clone, Debug, PartialEq)]
pub struct HeatingEstimate {
    pub energy_ev_per_source: f64,
    pub sampling_se_ev_per_source: Option<f64>,
    pub source_rate_per_s: f64,
    pub volume_m3: f64,
    pub power_density_kw_m3: f64,
    pub sampling_se_kw_m3: Option<f64>,
    /// Relative uncertainty of the independent geometry volume estimator, kept separate from
    /// the Monte Carlo tally error. It is `None` when the volume was authored rather than sampled.
    pub volume_sampling_relative_error: Option<f64>,
}

/// Convert eV/source to kW/m³. The source rate and volume are SI; callers must not normalize again.
pub fn power_density_kw_m3(
    energy_ev_per_source: f64,
    source_rate_per_s: f64,
    volume_m3: f64,
) -> Result<f64, DesignError> {
    if !energy_ev_per_source.is_finite() || energy_ev_per_source < 0.0 {
        return Err(DesignError::Tool(
            "heating score must be finite and non-negative".into(),
        ));
    }
    if !source_rate_per_s.is_finite() || source_rate_per_s <= 0.0 {
        return Err(DesignError::Tool(
            "heating source rate must be finite and positive".into(),
        ));
    }
    if !volume_m3.is_finite() || volume_m3 <= 0.0 {
        return Err(DesignError::Tool(
            "heating averaging volume must be finite and positive".into(),
        ));
    }
    Ok(energy_ev_per_source * source_rate_per_s * ELEMENTARY_CHARGE_C / volume_m3 / 1000.0)
}

/// Normalize a score and its MC sampling SE; volume uncertainty is attached as its own field.
pub fn normalize_heating_ev_per_source(
    energy_ev_per_source: f64,
    sampling_se_ev_per_source: Option<f64>,
    source_rate_per_s: f64,
    volume_m3: f64,
    volume_sampling_relative_error: Option<f64>,
) -> Result<HeatingEstimate, DesignError> {
    if sampling_se_ev_per_source.is_some_and(|se| !se.is_finite() || se < 0.0) {
        return Err(DesignError::Tool(
            "heating sampling error must be finite and non-negative".into(),
        ));
    }
    if volume_sampling_relative_error.is_some_and(|se| !se.is_finite() || se < 0.0) {
        return Err(DesignError::Tool(
            "volume sampling error must be finite and non-negative".into(),
        ));
    }
    let power_density_kw_m3 =
        power_density_kw_m3(energy_ev_per_source, source_rate_per_s, volume_m3)?;
    let sampling_se_kw_m3 = sampling_se_ev_per_source
        .map(|se| power_density_kw_m3(se, source_rate_per_s, volume_m3))
        .transpose()?;
    Ok(HeatingEstimate {
        energy_ev_per_source,
        sampling_se_ev_per_source,
        source_rate_per_s,
        volume_m3,
        power_density_kw_m3,
        sampling_se_kw_m3,
        volume_sampling_relative_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_volumes_scale_density_and_sampling_error_independently() {
        let a =
            normalize_heating_ev_per_source(1000.0, Some(100.0), 1.0e18, 0.01, Some(0.02)).unwrap();
        let b =
            normalize_heating_ev_per_source(1000.0, Some(100.0), 1.0e18, 0.02, Some(0.04)).unwrap();
        assert!((a.power_density_kw_m3 - 16.021_766_34).abs() < 1e-12);
        assert!((b.power_density_kw_m3 - 8.010_883_17).abs() < 1e-12);
        assert!((a.sampling_se_kw_m3.unwrap() - 1.602_176_634).abs() < 1e-12);
        assert!((b.sampling_se_kw_m3.unwrap() - 0.801_088_317).abs() < 1e-12);
        assert_eq!(a.volume_sampling_relative_error, Some(0.02));
        assert_eq!(b.volume_sampling_relative_error, Some(0.04));
    }

    #[test]
    fn invalid_dimensions_and_errors_are_rejected() {
        assert!(power_density_kw_m3(1.0, 1.0, 0.0).is_err());
        assert!(normalize_heating_ev_per_source(1.0, Some(f64::NAN), 1.0, 1.0, None).is_err());
    }
}
