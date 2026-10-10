//! The faceted model on the integer grid: snapped vertices, triangles grouped
//! into surfaces, and the volumes on the two sides of each surface.
//!
//! Vertices are snapped to 1 nm (at most 0.5 nm per axis); the snap is stated in
//! every integrity report. A model whose bounding box is larger than the exact
//! predicates allow is refused with the reason. Triangles that have no area
//! after snapping are listed in [`Mesh::degenerate`] and kept out of every
//! geometric query, because a triangle with no plane would answer every
//! question with "on the plane". They are reported as findings and block.

use super::exact::{MAX_EXTENT, P3, dominant_axis};
use faris_model::design::Facets;
use faris_model::math::powi;

/// Grid units per metre.
pub const GRID_PER_M: f64 = 1.0e9;

#[derive(Clone, Copy, Debug)]
pub struct SurfaceInfo {
    pub first_triangle: u32,
    pub triangle_count: u32,
    /// Normals point out of this volume (1-based; 0 is the implicit complement).
    pub forward: u32,
    /// ... and into this one.
    pub reverse: u32,
}

#[derive(Debug)]
pub struct Mesh {
    pub pts: Vec<P3>,
    pub tris: Vec<[u32; 3]>,
    /// Surface index of every triangle.
    pub tri_surface: Vec<u32>,
    pub surfaces: Vec<SurfaceInfo>,
    pub n_volumes: u32,
    /// Bounding box of the vertices, nm.
    pub lo: P3,
    pub hi: P3,
    /// Triangles with no area on the grid.
    pub degenerate: Vec<u32>,
    pub unimprinted: bool,
}

pub fn to_grid(x: f64) -> Option<i64> {
    let v = (x * GRID_PER_M).round();
    (v.is_finite() && v.abs() < (1u64 << 62) as f64).then_some(v as i64)
}

pub fn to_metres(p: P3) -> [f64; 3] {
    [
        p[0] as f64 / GRID_PER_M,
        p[1] as f64 / GRID_PER_M,
        p[2] as f64 / GRID_PER_M,
    ]
}

impl Mesh {
    pub fn from_facets(facets: &Facets) -> Result<Self, String> {
        if facets.vertices.len() >= 1 << 31 {
            return Err(format!(
                "the model has {} vertices; FARIS's edge tables index vertices with 31 bits",
                facets.vertices.len()
            ));
        }
        let mut pts = Vec::with_capacity(facets.vertices.len());
        for (i, v) in facets.vertices.iter().enumerate() {
            let snapped = [to_grid(v[0]), to_grid(v[1]), to_grid(v[2])];
            let [Some(x), Some(y), Some(z)] = snapped else {
                return Err(format!(
                    "vertex {i} is at {v:?} m, outside the range the 1 nm integer grid can hold"
                ));
            };
            pts.push([x, y, z]);
        }
        let mut lo = [i64::MAX; 3];
        let mut hi = [i64::MIN; 3];
        for p in &pts {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        for k in 0..3 {
            if hi[k] - lo[k] > MAX_EXTENT {
                return Err(format!(
                    "the model is {:.1} m across on axis {}; the exact predicates handle at most \
                     {:.1} m (their 128-bit arithmetic is proven only for coordinate differences \
                     below 2^41 nm). FARIS refuses the model rather than check it approximately",
                    (hi[k] - lo[k]) as f64 / GRID_PER_M,
                    ["x", "y", "z"][k],
                    MAX_EXTENT as f64 / GRID_PER_M
                ));
            }
        }
        let mut tri_surface = Vec::with_capacity(facets.triangles.len());
        let mut surfaces = Vec::with_capacity(facets.surfaces.len());
        for (i, s) in facets.surfaces.iter().enumerate() {
            surfaces.push(SurfaceInfo {
                first_triangle: s.first_triangle as u32,
                triangle_count: s.triangle_count as u32,
                forward: s.forward_volume,
                reverse: s.reverse_volume,
            });
            tri_surface.extend(std::iter::repeat_n(i as u32, s.triangle_count as usize));
        }
        let degenerate = facets
            .triangles
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                let [a, b, c] = t.map(|i| pts[i as usize]);
                dominant_axis(a, b, c).is_none()
            })
            .map(|(i, _)| i as u32)
            .collect();
        Ok(Self {
            pts,
            tris: facets.triangles.clone(),
            tri_surface,
            surfaces,
            n_volumes: facets.n_volumes,
            lo,
            hi,
            degenerate,
            unimprinted: facets.unimprinted(),
        })
    }

    #[inline]
    pub fn tri_pts(&self, t: u32) -> [P3; 3] {
        let [a, b, c] = self.tris[t as usize];
        [
            self.pts[a as usize],
            self.pts[b as usize],
            self.pts[c as usize],
        ]
    }

    #[inline]
    pub fn surface_of(&self, t: u32) -> &SurfaceInfo {
        &self.surfaces[self.tri_surface[t as usize] as usize]
    }

    /// The nonzero volumes on the two sides of a triangle's surface.
    pub fn volumes_of(&self, t: u32) -> impl Iterator<Item = u32> {
        let s = self.surface_of(t);
        [s.forward, s.reverse].into_iter().filter(|&v| v != 0)
    }

    pub fn extent(&self) -> [i64; 3] {
        [
            self.hi[0] - self.lo[0],
            self.hi[1] - self.lo[1],
            self.hi[2] - self.lo[2],
        ]
    }

    /// Length of the box diagonal, nm.
    pub fn diagonal(&self) -> f64 {
        let e = self.extent();
        (powi(e[0] as f64, 2) + powi(e[1] as f64, 2) + powi(e[2] as f64, 2)).sqrt()
    }

    pub fn bbox_volume_m3(&self) -> f64 {
        let e = self.extent();
        (e[0] as f64 / GRID_PER_M) * (e[1] as f64 / GRID_PER_M) * (e[2] as f64 / GRID_PER_M)
    }

    pub fn contains(&self, p: P3) -> bool {
        (0..3).all(|k| p[k] >= self.lo[k] && p[k] <= self.hi[k])
    }

    /// Triangle ids of each volume (index = volume - 1).
    pub fn volume_triangles(&self) -> Vec<Vec<u32>> {
        let mut out = vec![Vec::new(); self.n_volumes as usize];
        for s in &self.surfaces {
            for v in [s.forward, s.reverse] {
                if v != 0 {
                    out[v as usize - 1]
                        .extend(s.first_triangle..s.first_triangle + s.triangle_count);
                }
            }
        }
        out
    }

    /// The vertices of a triangle in the order its surface gives volume `v`'s
    /// outward orientation: as stored for the forward volume, reversed for the reverse volume.
    pub fn oriented_for(&self, t: u32, v: u32) -> [P3; 3] {
        let [a, b, c] = self.tri_pts(t);
        if self.surface_of(t).reverse == v && self.surface_of(t).forward != v {
            [a, c, b]
        } else {
            [a, b, c]
        }
    }

    /// Exact signed volume of volume `v`, in grid units cubed times 6 (the sum of
    /// determinants), measured from the box's lower corner, or `None` if the sum overflows `i128`.
    pub fn six_volume_exact(&self, v: u32, tris: &[u32]) -> Option<i128> {
        let mut total: i128 = 0;
        for &t in tris {
            let [a, b, c] = self.oriented_for(t, v);
            total = total.checked_add(six_volume_term(self.lo, a, b, c))?;
        }
        Some(total)
    }
}

/// `det(a - o, b - o, c - o)`: six times the signed volume of the tetrahedron `o a b c`.
#[inline]
pub fn six_volume_term(o: P3, a: P3, b: P3, c: P3) -> i128 {
    super::exact::det3(
        [a[0] - o[0], a[1] - o[1], a[2] - o[2]],
        [b[0] - o[0], b[1] - o[1], b[2] - o[2]],
        [c[0] - o[0], c[1] - o[1], c[2] - o[2]],
    )
}

/// Cubic nanometres per cubic metre.
pub const GRID3_PER_M3: f64 = 1.0e27;

#[cfg(test)]
pub(crate) mod fixtures {
    //! Small closed meshes built in code, for the tests of every integrity check.
    use faris_model::design::{Facets, Surface};

    /// An axis-aligned box as 12 triangles with outward normals, in metres.
    pub fn box_triangles(lo: [f64; 3], hi: [f64; 3], base: u32) -> (Vec<[f64; 3]>, Vec<[u32; 3]>) {
        let v = vec![
            [lo[0], lo[1], lo[2]],
            [hi[0], lo[1], lo[2]],
            [hi[0], hi[1], lo[2]],
            [lo[0], hi[1], lo[2]],
            [lo[0], lo[1], hi[2]],
            [hi[0], lo[1], hi[2]],
            [hi[0], hi[1], hi[2]],
            [lo[0], hi[1], hi[2]],
        ];
        let t = [
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [2, 3, 7],
            [2, 7, 6],
            [3, 0, 4],
            [3, 4, 7],
        ];
        (
            v,
            t.iter()
                .map(|t| [t[0] + base, t[1] + base, t[2] + base])
                .collect(),
        )
    }

    /// Several separate boxes, each its own volume in an unimprinted-style layout
    /// (every surface faces the complement).
    pub fn separate_boxes(boxes: &[([f64; 3], [f64; 3])]) -> Facets {
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        let mut surfaces = Vec::new();
        for (i, (lo, hi)) in boxes.iter().enumerate() {
            let (v, t) = box_triangles(*lo, *hi, vertices.len() as u32);
            surfaces.push(Surface {
                first_triangle: triangles.len() as u64,
                triangle_count: t.len() as u64,
                forward_volume: i as u32 + 1,
                reverse_volume: 0,
            });
            vertices.extend(v);
            triangles.extend(t);
        }
        Facets {
            flags: 0,
            n_volumes: boxes.len() as u32,
            vertices,
            triangles,
            surfaces,
        }
    }

    /// A shell: an outer box with a box-shaped hole, as volume 1 (outer boundary
    /// forward, inner boundary reverse), and a core box filling the hole as volume 2.
    /// The inner surface is shared by the shell and the core, as after imprinting.
    pub fn shell_and_core(outer: f64, inner: f64) -> Facets {
        let (mut vertices, outer_t) = box_triangles([-outer; 3], [outer; 3], 0);
        let (v2, inner_t) = box_triangles([-inner; 3], [inner; 3], 8);
        vertices.extend(v2);
        let mut triangles = outer_t.clone();
        triangles.extend(inner_t);
        Facets {
            flags: 0,
            n_volumes: 2,
            vertices,
            triangles,
            surfaces: vec![
                Surface {
                    first_triangle: 0,
                    triangle_count: 12,
                    forward_volume: 1,
                    reverse_volume: 0,
                },
                // normals out of the core (volume 2), into the shell (volume 1)
                Surface {
                    first_triangle: 12,
                    triangle_count: 12,
                    forward_volume: 2,
                    reverse_volume: 1,
                },
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    #[test]
    fn snaps_to_nanometres_and_measures_the_box() {
        let f = separate_boxes(&[([0.0; 3], [1.0, 2.0, 3.0000000004])]);
        let m = Mesh::from_facets(&f).unwrap();
        assert_eq!(m.hi, [1_000_000_000, 2_000_000_000, 3_000_000_000]);
        assert_eq!(m.extent()[2], 3_000_000_000);
        assert!(m.degenerate.is_empty());
        assert!((m.bbox_volume_m3() - 6.0).abs() < 1e-9);
    }

    // Verifies: GEO-030
    #[test]
    fn exact_signed_volume_of_boxes_and_shells() {
        let m = Mesh::from_facets(&separate_boxes(&[([0.0; 3], [1.0, 2.0, 3.0])])).unwrap();
        let tris = &m.volume_triangles()[0];
        let six = m.six_volume_exact(1, tris).unwrap();
        assert_eq!(six, 6 * 6_000_000_000_000_000_000_000_000_000);
        // shell (10 m cube with a 2 m cube removed): 1000 - 8 = 992 m3 is not what the
        // shell has: outer 20^3 = 8000, inner 4^3 = 64 m3.
        let m = Mesh::from_facets(&shell_and_core(10.0, 2.0)).unwrap();
        let v = m.volume_triangles();
        let shell = m.six_volume_exact(1, &v[0]).unwrap() as f64 / 6.0 / GRID3_PER_M3;
        let core = m.six_volume_exact(2, &v[1]).unwrap() as f64 / 6.0 / GRID3_PER_M3;
        assert!((shell - (8000.0 - 64.0)).abs() < 1e-6, "{shell}");
        assert!((core - 64.0).abs() < 1e-9, "{core}");
    }

    #[test]
    fn a_model_that_is_too_large_is_refused_with_the_reason() {
        let f = separate_boxes(&[([0.0; 3], [300.0, 1.0, 1.0])]);
        let err = Mesh::from_facets(&f).unwrap_err();
        assert!(err.contains("at most") && err.contains("refuses"), "{err}");
        let f = separate_boxes(&[([0.0; 3], [1.0e11, 1.0, 1.0])]);
        assert!(Mesh::from_facets(&f).unwrap_err().contains("grid can hold"));
    }

    #[test]
    fn a_collapsed_triangle_is_listed_and_not_trusted() {
        let mut f = separate_boxes(&[([0.0; 3], [1.0; 3])]);
        // move vertex 1 onto vertex 0 within half a nanometre
        f.vertices[1] = [4e-10, 0.0, 0.0];
        let m = Mesh::from_facets(&f).unwrap();
        assert!(!m.degenerate.is_empty());
    }
}
