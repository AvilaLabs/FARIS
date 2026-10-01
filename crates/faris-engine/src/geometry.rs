//! Independent geometry checks and bounded numerical volume estimates.

use serde::Serialize;

#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
pub struct NumericalVolumeEstimate {
    pub volume_m3: f64,
    /// Absolute change between the reported midpoint grid and its half-size
    /// predecessor. This is a convergence diagnostic, not a rigorous bound.
    pub refinement_delta_m3: f64,
    pub grid_per_axis: usize,
    pub independently_validated: bool,
}

/// Integrates a circular torus-shell intersection with an XYZ-aligned box.
/// The torus is symmetric in toroidal angle; this cross-section formulation
/// integrates x analytically and applies a midpoint rule over the box's YZ
/// face. It deliberately does not qualify the result as an exact volume.
pub fn estimate_torus_shell_box_intersection(
    major_radius_m: f64,
    inner_minor_radius_m: f64,
    outer_minor_radius_m: f64,
    minimum_xyz_m: &[f64; 3],
    maximum_xyz_m: &[f64; 3],
) -> NumericalVolumeEstimate {
    let coarse = integrate(
        major_radius_m,
        inner_minor_radius_m,
        outer_minor_radius_m,
        minimum_xyz_m,
        maximum_xyz_m,
        128,
    );
    let fine = integrate(
        major_radius_m,
        inner_minor_radius_m,
        outer_minor_radius_m,
        minimum_xyz_m,
        maximum_xyz_m,
        256,
    );
    NumericalVolumeEstimate {
        volume_m3: fine,
        refinement_delta_m3: (fine - coarse).abs(),
        grid_per_axis: 256,
        independently_validated: false,
    }
}

fn integrate(r: f64, inner: f64, outer: f64, min: &[f64; 3], max: &[f64; 3], n: usize) -> f64 {
    let dy = (max[1] - min[1]) / n as f64;
    let dz = (max[2] - min[2]) / n as f64;
    let mut total = 0.0;
    for iy in 0..n {
        let y = min[1] + (iy as f64 + 0.5) * dy;
        for iz in 0..n {
            let z = min[2] + (iz as f64 + 0.5) * dz;
            let outer_length = torus_cross_section_x_length(r, outer, y, z, min[0], max[0]);
            let inner_length = torus_cross_section_x_length(r, inner, y, z, min[0], max[0]);
            total += (outer_length - inner_length).max(0.0);
        }
    }
    total * dy * dz
}

fn torus_cross_section_x_length(r: f64, minor: f64, y: f64, z: f64, xmin: f64, xmax: f64) -> f64 {
    if y.abs() >= minor {
        return 0.0;
    }
    let half = (minor * minor - y * y).sqrt();
    let s_lo = r - half;
    let s_hi = r + half;
    let x_lo = (s_lo * s_lo - z * z).max(0.0).sqrt().max(xmin);
    let x_hi = (s_hi * s_hi - z * z).max(0.0).sqrt().min(xmax);
    (x_hi - x_lo).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: [f64; 3] = [4.34, -0.15, -0.15];
    const MAX: [f64; 3] = [5.59, 0.15, 0.15];

    #[test]
    fn prism_intersection_estimate_is_bounded_and_partition_conserving() {
        let pieces = [
            (1.08, 1.11),
            (1.11, 1.56),
            (1.56, 2.01),
            (2.01, 2.06),
            (2.06, 2.12),
            (2.12, 2.28),
        ];
        let values: Vec<_> = pieces
            .iter()
            .map(|(a, b)| estimate_torus_shell_box_intersection(3.3, *a, *b, &MIN, &MAX))
            .collect();
        let total: f64 = values.iter().map(|x| x.volume_m3).sum();
        let combined = estimate_torus_shell_box_intersection(3.3, 1.08, 2.28, &MIN, &MAX);
        assert!(values.iter().all(|x| x.volume_m3 > 0.0));
        assert!(values.iter().all(|x| x.refinement_delta_m3 >= 0.0));
        assert!(total <= (MAX[0] - MIN[0]) * (MAX[1] - MIN[1]) * (MAX[2] - MIN[2]));
        assert!((total - combined.volume_m3).abs() < 2e-4);
        assert!(!combined.independently_validated);
    }

    #[test]
    fn midpoint_estimates_match_independent_scipy_adaptive_controls() {
        // Constants were computed independently with scipy.integrate.dblquad
        // QUADPACK at epsabs=1e-10 m³, epsrel=1e-9 in
        // controls/check_port_geometry.py (SciPy 1.18.0).
        let expected_reference = [
            0.0027090450441959323,
            0.04059535921021764,
            0.040554495697482165,
            0.004504679561483164,
            0.005405345978904079,
            0.014412983907360106,
        ];
        let mut inner = 1.08;
        let mut total = 0.0;
        for (index, layer_thickness) in [0.03, 0.45, 0.45, 0.05, 0.06, 0.16].into_iter().enumerate()
        {
            let outer = inner + layer_thickness;
            let result = estimate_torus_shell_box_intersection(3.3, inner, outer, &MIN, &MAX);
            assert!((result.volume_m3 - expected_reference[index]).abs() < 2e-5);
            total += result.volume_m3;
            inner = outer;
        }
        assert!((total - 0.1081819093996431).abs() < 2e-5);
    }

    #[test]
    fn box_away_from_shell_has_zero_intersection() {
        let estimate = estimate_torus_shell_box_intersection(
            3.3,
            1.08,
            1.11,
            &[4.0, 0.5, 0.5],
            &[4.1, 0.6, 0.6],
        );
        assert_eq!(estimate.volume_m3, 0.0);
    }
}
