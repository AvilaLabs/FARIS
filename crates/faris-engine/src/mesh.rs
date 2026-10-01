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

/// Tessellates the same toroidal shell with a finite axis-aligned XYZ prism
/// removed. The prism must start and end outside the shell, so its four
/// longitudinal faces form the tunnel walls and no artificial end caps are
/// added inside material.
pub fn torus_shell_with_prism_cut(
    major: f32,
    inner: f32,
    outer: f32,
    sweep: f32,
    minimum_xyz: [f32; 3],
    maximum_xyz: [f32; 3],
) -> Result<Vec<MeshVertex>, MeshError> {
    if !minimum_xyz
        .into_iter()
        .chain(maximum_xyz)
        .all(f32::is_finite)
        || !(0..3).all(|axis| minimum_xyz[axis] < maximum_xyz[axis])
        || minimum_xyz[0] <= major
    {
        return Err(MeshError);
    }
    let mut output = Vec::new();
    let base = torus_shell(major, inner, outer, sweep)?;
    for triangle in base.as_chunks::<3>().0 {
        refine_triangle(*triangle, minimum_xyz, maximum_xyz, 0, &mut output);
    }
    append_prism_walls(
        major,
        inner,
        outer,
        sweep,
        minimum_xyz,
        maximum_xyz,
        &mut output,
    );
    Ok(output)
}

fn refine_triangle(
    triangle: [MeshVertex; 3],
    min: [f32; 3],
    max: [f32; 3],
    depth: u8,
    out: &mut Vec<MeshVertex>,
) {
    let intersects = (0..3).all(|axis| {
        let lo = triangle
            .iter()
            .map(|v| v.position[axis])
            .fold(f32::INFINITY, f32::min);
        let hi = triangle
            .iter()
            .map(|v| v.position[axis])
            .fold(f32::NEG_INFINITY, f32::max);
        hi >= min[axis] && lo <= max[axis]
    });
    if !intersects {
        out.extend(triangle);
        return;
    }
    if depth == 7 {
        // Conservatively discard terminal triangles whose bounding boxes touch
        // the prism. This bounds display-only overcut to the local tessellation
        // scale and prevents shell triangles leaking into the port.
        return;
    }
    let ab = midpoint(triangle[0], triangle[1]);
    let bc = midpoint(triangle[1], triangle[2]);
    let ca = midpoint(triangle[2], triangle[0]);
    for sub in [
        [triangle[0], ab, ca],
        [ab, triangle[1], bc],
        [ca, bc, triangle[2]],
        [ab, bc, ca],
    ] {
        refine_triangle(sub, min, max, depth + 1, out);
    }
}

fn midpoint(a: MeshVertex, b: MeshVertex) -> MeshVertex {
    let position = std::array::from_fn(|i| (a.position[i] + b.position[i]) * 0.5);
    let mut normal = std::array::from_fn(|i| (a.normal[i] + b.normal[i]) * 0.5);
    let length = normal.iter().map(|x| x * x).sum::<f32>().sqrt();
    if length > 0.0 {
        normal.iter_mut().for_each(|x| *x /= length);
    }
    MeshVertex { position, normal }
}

fn append_prism_walls(
    major: f32,
    inner: f32,
    outer: f32,
    sweep: f32,
    min: [f32; 3],
    max: [f32; 3],
    out: &mut Vec<MeshVertex>,
) {
    const STEP: f32 = PORT_WALL_GRID_SPACING_M;
    for (axis, fixed_sign) in [(1, -1.0_f32), (1, 1.0), (2, -1.0), (2, 1.0)] {
        let fixed = if fixed_sign < 0.0 {
            min[axis]
        } else {
            max[axis]
        };
        let other = if axis == 1 { 2 } else { 1 };
        let nx = ((max[0] - min[0]) / STEP).ceil() as usize;
        let no = ((max[other] - min[other]) / STEP).ceil() as usize;
        for ix in 0..nx {
            for io in 0..no {
                let x0 = min[0] + (max[0] - min[0]) * ix as f32 / nx as f32;
                let x1 = min[0] + (max[0] - min[0]) * (ix + 1) as f32 / nx as f32;
                let o0 = min[other] + (max[other] - min[other]) * io as f32 / no as f32;
                let o1 = min[other] + (max[other] - min[other]) * (io + 1) as f32 / no as f32;
                let x = (x0 + x1) * 0.5;
                let ov = (o0 + o1) * 0.5;
                let y = if axis == 1 { fixed } else { ov };
                let z = if axis == 2 { fixed } else { ov };
                let minor = ((x.hypot(z) - major).powi(2) + y * y).sqrt();
                if !(inner..=outer).contains(&minor) {
                    continue;
                }
                let inward = -fixed_sign;
                let normal = if axis == 1 {
                    [0.0, inward, 0.0]
                } else {
                    [0.0, 0.0, inward]
                };
                let make = |xx: f32, oo: f32| MeshVertex {
                    position: if axis == 1 {
                        [xx, fixed, oo]
                    } else {
                        [xx, oo, fixed]
                    },
                    normal,
                };
                let corners = [make(x0, o0), make(x1, o0), make(x1, o1), make(x0, o1)];
                let reverse = (axis == 1 && fixed_sign < 0.0) || (axis == 2 && fixed_sign > 0.0);
                if reverse {
                    append_visible_quad(
                        out,
                        [corners[0], corners[3], corners[2], corners[1]],
                        sweep,
                    );
                } else {
                    append_visible_quad(out, corners, sweep);
                }
            }
        }
    }
}

/// Emit only tunnel-wall triangles fully contained in the displayed toroidal
/// wedge. Dropping a boundary triangle is conservative by at most one wall
/// grid cell and avoids floating tunnel patches in the cutaway quadrant.
fn append_visible_quad(out: &mut Vec<MeshVertex>, corners: [MeshVertex; 4], sweep: f32) {
    for indices in [[0, 1, 2], [0, 2, 3]] {
        let triangle = [
            corners[indices[0]],
            corners[indices[1]],
            corners[indices[2]],
        ];
        if triangle
            .iter()
            .all(|vertex| within_toroidal_sweep(vertex.position, sweep))
        {
            out.extend(triangle);
        }
    }
}

fn within_toroidal_sweep(position: [f32; 3], sweep: f32) -> bool {
    let mut phi = position[2].atan2(position[0]);
    if phi < 0.0 {
        phi += TAU;
    }
    phi <= sweep + 1.0e-6
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
/// Maximum planar grid spacing for port tunnel-wall patches. Boundary
/// classification is within one cell diagonal of the analytic shell crossing.
pub const PORT_WALL_GRID_SPACING_M: f32 = 0.0025;

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

    #[test]
    fn finite_prism_cut_removes_shell_surface_and_adds_tunnel_walls() {
        let min = [4.34, -0.15, -0.15];
        let max = [5.59, 0.15, 0.15];
        let vertices = torus_shell_with_prism_cut(3.3, 1.08, 1.11, TAU, min, max).unwrap();
        let mut wall_triangles = 0;
        for triangle in vertices.as_chunks::<3>().0 {
            let wall_axis = [1, 2].into_iter().find(|axis| {
                [min[*axis], max[*axis]].into_iter().any(|plane| {
                    triangle.iter().all(|v| {
                        (v.position[*axis] - plane).abs() < 1e-6
                            && (min[0]..=max[0]).contains(&v.position[0])
                            && (min[1]..=max[1]).contains(&v.position[1])
                            && (min[2]..=max[2]).contains(&v.position[2])
                    })
                })
            });
            let tunnel_wall = wall_axis.is_some();
            if tunnel_wall {
                wall_triangles += 1;
                let axis = wall_axis.unwrap();
                let plane = triangle[0].position[axis];
                let expected_normal = if (plane - min[axis]).abs() < 1e-6 {
                    1.0
                } else {
                    -1.0
                };
                assert!((triangle[0].normal[axis] - expected_normal).abs() < 1e-6);
                let a = triangle[0].position;
                let b = triangle[1].position;
                let c = triangle[2].position;
                let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let cross = [
                    ab[1] * ac[2] - ab[2] * ac[1],
                    ab[2] * ac[0] - ab[0] * ac[2],
                    ab[0] * ac[1] - ab[1] * ac[0],
                ];
                let winding_dot_normal: f32 = cross
                    .iter()
                    .zip(triangle[0].normal)
                    .map(|(a, b)| a * b)
                    .sum();
                assert!(winding_dot_normal > 0.0);
                for vertex in triangle {
                    let [x, y, z] = vertex.position;
                    let radius = ((x.hypot(z) - 3.3).powi(2) + y.powi(2)).sqrt();
                    assert!(radius >= 1.08 - PORT_WALL_GRID_SPACING_M * 0.75);
                    assert!(radius <= 1.11 + PORT_WALL_GRID_SPACING_M * 0.75);
                }
                continue;
            }
            for vertex in triangle {
                assert!(!(0..3).all(|axis| {
                    vertex.position[axis] > min[axis] && vertex.position[axis] < max[axis]
                }));
            }
        }
        assert!(wall_triangles > 0);
    }

    #[test]
    fn cutaway_port_walls_do_not_extend_into_the_hidden_toroidal_quadrant() {
        let min = [4.34, -0.15, -0.15];
        let max = [5.59, 0.15, 0.15];
        let vertices =
            torus_shell_with_prism_cut(3.3, 1.08, 1.11, CUTAWAY_SWEEP, min, max).unwrap();
        let triangles = vertices.as_chunks::<3>().0;
        assert!(!triangles.is_empty());
        for triangle in triangles {
            for vertex in triangle {
                assert!(within_toroidal_sweep(vertex.position, CUTAWAY_SWEEP));
            }
        }

        let is_tunnel_wall = |triangle: &[MeshVertex; 3]| {
            [1, 2].into_iter().any(|axis| {
                [min[axis], max[axis]].into_iter().any(|plane| {
                    triangle.iter().all(|vertex| {
                        (vertex.position[axis] - plane).abs() < 1e-6
                            && (min[0]..=max[0]).contains(&vertex.position[0])
                            && (min[1]..=max[1]).contains(&vertex.position[1])
                            && (min[2]..=max[2]).contains(&vertex.position[2])
                    })
                })
            })
        };
        let visible_wall_count = triangles
            .iter()
            .filter(|triangle| is_tunnel_wall(triangle))
            .count();
        assert!(
            visible_wall_count > 0,
            "the included-side tunnel walls remain visible"
        );
        let hidden_negative_z_wall_count = triangles
            .iter()
            .filter(|triangle| {
                is_tunnel_wall(triangle)
                    && triangle
                        .iter()
                        .all(|vertex| (vertex.position[2] - min[2]).abs() < 1e-6)
            })
            .count();
        assert_eq!(hidden_negative_z_wall_count, 0);
    }
}
