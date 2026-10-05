//! Independent geometry checks and bounded numerical volume estimates.

use faris_model::transport::ToroidalRegion;
use serde::Serialize;
use std::f64::consts::PI;

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
    integrate_clipped(r, inner, outer, min, max, n, &|_| (min[0], max[0]))
}

/// Like `integrate`, with the x range of each (y, z) line narrowed by `clip(z)`.
fn integrate_clipped(
    r: f64,
    inner: f64,
    outer: f64,
    min: &[f64; 3],
    max: &[f64; 3],
    n: usize,
    clip: &dyn Fn(f64) -> (f64, f64),
) -> f64 {
    let dy = (max[1] - min[1]) / n as f64;
    let dz = (max[2] - min[2]) / n as f64;
    let mut total = 0.0;
    for iy in 0..n {
        let y = min[1] + (iy as f64 + 0.5) * dy;
        for iz in 0..n {
            let z = min[2] + (iz as f64 + 0.5) * dz;
            let (xmin, xmax) = clip(z);
            if xmax <= xmin {
                continue;
            }
            let outer_length = torus_cross_section_x_length(r, outer, y, z, xmin, xmax);
            let inner_length = torus_cross_section_x_length(r, inner, y, z, xmin, xmax);
            total += (outer_length - inner_length).max(0.0);
        }
    }
    total * dy * dz
}

/// Exact volume of a region of the full torus shell between minor radii
/// `a < b` about major radius `R0`.
///
/// A point of the shell has cylindrical radius `R = R0 + r cos(theta)` for
/// minor radius `r` and cross-section angle `theta`, and volume element
/// `dV = R r dr dtheta dphi = (R0 + r cos(theta)) r dr dtheta dphi`. The inboard half
/// (`R < R0`, `cos(theta) < 0`) and outboard half (`cos(theta) > 0`) therefore
/// integrate over a half annulus each:
///
/// ```text
/// int (R0 + r cos(theta)) r dr dtheta
///   = R0 * pi * (b^2 - a^2) / 2  +/-  (b^3 - a^3)/3 * int cos(theta) dtheta
///   = R0 * pi * (b^2 - a^2) / 2  +/-  2 (b^3 - a^3) / 3
/// ```
///
/// with `+` outboard and `-` inboard. Multiplying by the toroidal extent
/// `dphi` gives the region volume: `2 pi` for a half, `2 w` for the port
/// sector of half width `w`, and `2 pi - 2 w` for the outboard half without
/// it. The two halves sum to the full torus `2 pi^2 R0 (b^2 - a^2)`.
pub fn torus_shell_region_volume_m3(
    major_radius_m: f64,
    inner_minor_radius_m: f64,
    outer_minor_radius_m: f64,
    region: &ToroidalRegion,
) -> f64 {
    let (a, b) = (inner_minor_radius_m, outer_minor_radius_m);
    let half_annulus = major_radius_m * PI * (b * b - a * a) / 2.0;
    let skew = 2.0 * (b.powi(3) - a.powi(3)) / 3.0;
    let inboard = half_annulus - skew;
    let outboard = half_annulus + skew;
    match region {
        ToroidalRegion::InboardHalf => 2.0 * PI * inboard,
        ToroidalRegion::OutboardHalf {
            excluding_sector_half_width_rad: None,
        } => 2.0 * PI * outboard,
        ToroidalRegion::OutboardHalf {
            excluding_sector_half_width_rad: Some(w),
        } => (2.0 * PI - 2.0 * w) * outboard,
        ToroidalRegion::PortSector { half_width_rad } => 2.0 * half_width_rad * outboard,
    }
}

/// Midpoint-rule volume of a region of a torus shell inside an XYZ-aligned
/// box that lies entirely at positive x (as a +x outboard prism does), so the
/// toroidal angle `phi = atan2(z, x)` is within (-pi/2, pi/2). For each
/// (y, z) line the region narrows the x range analytically: the outboard half
/// needs `x >= sqrt(R0^2 - z^2)` (all x once `|z| >= R0`) and the port sector
/// additionally `|z| <= x tan(w)`, that is `x >= |z| / tan(w)`.
pub fn estimate_torus_shell_box_region_intersection(
    major_radius_m: f64,
    inner_minor_radius_m: f64,
    outer_minor_radius_m: f64,
    region: &ToroidalRegion,
    minimum_xyz_m: &[f64; 3],
    maximum_xyz_m: &[f64; 3],
) -> Option<NumericalVolumeEstimate> {
    if minimum_xyz_m[0] <= 0.0 {
        return None;
    }
    let clip = |z: f64| {
        let (xmin, xmax) = (minimum_xyz_m[0], maximum_xyz_m[0]);
        let r0 = major_radius_m;
        let boundary = (r0 * r0 - z * z).max(0.0).sqrt();
        let sector_edge = |w: f64| z.abs() / w.tan();
        match region {
            ToroidalRegion::InboardHalf => (xmin, xmax.min(boundary)),
            ToroidalRegion::OutboardHalf {
                excluding_sector_half_width_rad: None,
            } => (xmin.max(boundary), xmax),
            ToroidalRegion::OutboardHalf {
                excluding_sector_half_width_rad: Some(w),
            } => (xmin.max(boundary), xmax.min(sector_edge(*w))),
            ToroidalRegion::PortSector { half_width_rad } => {
                (xmin.max(boundary).max(sector_edge(*half_width_rad)), xmax)
            }
        }
    };
    let run = |n| {
        integrate_clipped(
            major_radius_m,
            inner_minor_radius_m,
            outer_minor_radius_m,
            minimum_xyz_m,
            maximum_xyz_m,
            n,
            &clip,
        )
    };
    let (coarse, fine) = (run(128), run(256));
    Some(NumericalVolumeEstimate {
        volume_m3: fine,
        refinement_delta_m3: (fine - coarse).abs(),
        grid_per_axis: 256,
        independently_validated: false,
    })
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

    const OUT: ToroidalRegion = ToroidalRegion::OutboardHalf {
        excluding_sector_half_width_rad: None,
    };

    /// Midpoint quadrature of `int (R0 + r cos(theta)) r dr dtheta` over the
    /// half annulus with `cos(theta)` of the given sign, times `dphi`.
    fn quadrature(r0: f64, a: f64, b: f64, outboard: bool, dphi: f64) -> f64 {
        let (nr, nt) = (800, 800);
        let (t0, t1) = if outboard {
            (-PI / 2.0, PI / 2.0)
        } else {
            (PI / 2.0, 3.0 * PI / 2.0)
        };
        let (dr, dt) = ((b - a) / nr as f64, (t1 - t0) / nt as f64);
        let mut sum = 0.0;
        for i in 0..nr {
            let r = a + (i as f64 + 0.5) * dr;
            for j in 0..nt {
                let t = t0 + (j as f64 + 0.5) * dt;
                sum += (r0 + r * t.cos()) * r * dr * dt;
            }
        }
        sum * dphi
    }

    #[test]
    fn region_volumes_sum_to_the_full_torus_and_match_quadrature() {
        let (r0, a, b) = (3.3, 2.12, 2.28);
        let full = 2.0 * PI * PI * r0 * (b * b - a * a);
        let w = 0.1745;
        let inboard = torus_shell_region_volume_m3(r0, a, b, &ToroidalRegion::InboardHalf);
        let outboard = torus_shell_region_volume_m3(r0, a, b, &OUT);
        let sector = torus_shell_region_volume_m3(
            r0,
            a,
            b,
            &ToroidalRegion::PortSector { half_width_rad: w },
        );
        let ex_port = torus_shell_region_volume_m3(
            r0,
            a,
            b,
            &ToroidalRegion::OutboardHalf {
                excluding_sector_half_width_rad: Some(w),
            },
        );
        assert!((inboard + outboard - full).abs() < 1e-12 * full);
        assert!((inboard + sector + ex_port - full).abs() < 1e-12 * full);
        assert!(inboard < outboard);
        assert!((sector / outboard - w / PI).abs() < 1e-14);
        assert!((inboard - quadrature(r0, a, b, false, 2.0 * PI)).abs() < 1e-6 * inboard);
        assert!((outboard - quadrature(r0, a, b, true, 2.0 * PI)).abs() < 1e-6 * outboard);
        assert!((sector - quadrature(r0, a, b, true, 2.0 * w)).abs() < 1e-6 * sector);
    }

    #[test]
    fn region_volumes_close_for_a_thick_low_aspect_shell() {
        // A shell reaching close to the axis makes the inboard/outboard
        // asymmetry large; the exact formulas must still close.
        let (r0, a, b) = (3.3, 1.08, 3.0);
        let full = 2.0 * PI * PI * r0 * (b * b - a * a);
        let inboard = torus_shell_region_volume_m3(r0, a, b, &ToroidalRegion::InboardHalf);
        let outboard = torus_shell_region_volume_m3(r0, a, b, &OUT);
        assert!((inboard + outboard - full).abs() < 1e-12 * full);
        assert!((inboard - quadrature(r0, a, b, false, 2.0 * PI)).abs() < 1e-6 * inboard);
    }

    #[test]
    fn port_box_region_pieces_sum_to_the_unrestricted_intersection() {
        let w = 0.1745;
        let sector = ToroidalRegion::PortSector { half_width_rad: w };
        let ex_port = ToroidalRegion::OutboardHalf {
            excluding_sector_half_width_rad: Some(w),
        };
        for (a, b) in [(1.08, 1.11), (2.12, 2.28)] {
            let all = estimate_torus_shell_box_intersection(3.3, a, b, &MIN, &MAX);
            let est = |region: &ToroidalRegion| {
                estimate_torus_shell_box_region_intersection(3.3, a, b, region, &MIN, &MAX)
                    .unwrap()
                    .volume_m3
            };
            let (inb, out, sec, exp) = (
                est(&ToroidalRegion::InboardHalf),
                est(&OUT),
                est(&sector),
                est(&ex_port),
            );
            // The port prism lies entirely outboard of R0 = 3.3.
            assert_eq!(inb, 0.0);
            assert!((out - all.volume_m3).abs() < 1e-12);
            assert!((sec + exp - out).abs() < 1e-12);
            // At R of about 5.5 m the sector reaches 0.96 m either side of the
            // axis, so the 0.3 m wide port sits wholly inside it.
            assert!((sec - all.volume_m3).abs() < 1e-12);
            assert_eq!(exp, 0.0);
        }
        // A narrow sector cuts the port: both outboard pieces are then nonzero
        // and still sum to the whole.
        let narrow = ToroidalRegion::PortSector {
            half_width_rad: 0.02,
        };
        let narrow_ex = ToroidalRegion::OutboardHalf {
            excluding_sector_half_width_rad: Some(0.02),
        };
        let all = estimate_torus_shell_box_intersection(3.3, 2.12, 2.28, &MIN, &MAX).volume_m3;
        let part = |r: &ToroidalRegion| {
            estimate_torus_shell_box_region_intersection(3.3, 2.12, 2.28, r, &MIN, &MAX)
                .unwrap()
                .volume_m3
        };
        let (s, e) = (part(&narrow), part(&narrow_ex));
        assert!(s > 0.0 && e > 0.0);
        assert!((s + e - all).abs() < 1e-9);
        // A box that straddles x <= 0 has no defined angle bound here.
        assert!(
            estimate_torus_shell_box_region_intersection(
                3.3,
                2.12,
                2.28,
                &OUT,
                &[-1.0, -0.1, -0.1],
                &[1.0, 0.1, 0.1]
            )
            .is_none()
        );
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
