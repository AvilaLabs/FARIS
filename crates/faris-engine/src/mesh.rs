//! Bounded display tessellation of the same toroidal shells used in geometry records.
//! A cutaway changes display meshes only, never the full-torus volumes or solver inputs.

use std::f32::consts::{PI, TAU};

#[derive(Clone, Copy, Debug)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
}

#[derive(Debug, thiserror::Error)]
#[error("mesh requires finite radii with 0 < inner < outer < major, and a sweep in (0, 2π]")]
pub struct MeshError;

pub fn torus_shell(
    major: f32,
    inner: f32,
    outer: f32,
    sweep: f32,
) -> Result<Vec<MeshVertex>, MeshError> {
    if ![major, inner, outer, sweep].into_iter().all(f32::is_finite)
        || !(0.0 < inner && inner < outer && outer < major && 0.0 < sweep && sweep <= TAU)
    {
        return Err(MeshError);
    }
    const TOROIDAL: usize = 48;
    const POLOIDAL: usize = 24;
    let mut vertices = Vec::with_capacity(TOROIDAL * POLOIDAL * 12 + POLOIDAL * 12);
    for (radius, sign) in [(outer, 1.0), (inner, -1.0)] {
        for i in 0..TOROIDAL {
            let phi0 = sweep * i as f32 / TOROIDAL as f32;
            let phi1 = sweep * (i + 1) as f32 / TOROIDAL as f32;
            for j in 0..POLOIDAL {
                let theta0 = TAU * j as f32 / POLOIDAL as f32;
                let theta1 = TAU * (j + 1) as f32 / POLOIDAL as f32;
                let make = |phi: f32, theta: f32| MeshVertex {
                    position: position(major, radius, phi, theta),
                    normal: [
                        sign * theta.cos() * phi.cos(),
                        sign * theta.sin(),
                        sign * theta.cos() * phi.sin(),
                    ],
                };
                quad(
                    &mut vertices,
                    [
                        make(phi0, theta0),
                        make(phi1, theta0),
                        make(phi1, theta1),
                        make(phi0, theta1),
                    ],
                );
            }
        }
    }
    if sweep < TAU - 1e-5 {
        for (phi, sign) in [(0.0_f32, -1.0), (sweep, 1.0)] {
            for j in 0..POLOIDAL {
                let theta0 = TAU * j as f32 / POLOIDAL as f32;
                let theta1 = TAU * (j + 1) as f32 / POLOIDAL as f32;
                let make = |radius: f32, theta: f32| MeshVertex {
                    position: position(major, radius, phi, theta),
                    normal: [-sign * phi.sin(), 0.0, sign * phi.cos()],
                };
                quad(
                    &mut vertices,
                    [
                        make(inner, theta0),
                        make(outer, theta0),
                        make(outer, theta1),
                        make(inner, theta1),
                    ],
                );
            }
        }
    }
    Ok(vertices)
}

fn position(major: f32, radius: f32, phi: f32, theta: f32) -> [f32; 3] {
    let radial = major + radius * theta.cos();
    [radial * phi.cos(), radius * theta.sin(), radial * phi.sin()]
}

fn quad(vertices: &mut Vec<MeshVertex>, corners: [MeshVertex; 4]) {
    for index in [0, 1, 2, 0, 2, 3] {
        vertices.push(corners[index]);
    }
}

pub const CUTAWAY_SWEEP: f32 = 1.5 * PI;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_vertices_lie_on_the_declared_shell_or_cut_faces() {
        let vertices = torus_shell(3.3, 1.08, 1.11, CUTAWAY_SWEEP).unwrap();
        for vertex in vertices {
            let [x, y, z] = vertex.position;
            let radius = ((x.hypot(z) - 3.3).powi(2) + y.powi(2)).sqrt();
            assert!((1.07999..=1.11001).contains(&radius));
            let norm = vertex
                .normal
                .iter()
                .map(|value| value * value)
                .sum::<f32>()
                .sqrt();
            assert!((norm - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn rejects_mesh_domain_errors() {
        assert!(torus_shell(1.0, 0.5, 2.0, TAU).is_err());
        assert!(torus_shell(3.3, 1.1, 1.0, TAU).is_err());
        assert!(torus_shell(3.3, 1.0, 1.1, 0.0).is_err());
        assert!(torus_shell(3.3, 1.0, 1.1, f32::NAN).is_err());
    }
}
