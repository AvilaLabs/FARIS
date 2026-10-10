//! Rays and chords through the faceted model, decided exactly.
//!
//! Point-in-volume uses ray parity: a ray from the point to a far point outside
//! the box crosses a closed surface an odd number of times when the point is
//! inside. A single ray against the model's one hierarchy gives the parity of
//! every volume (and every boundary component) at once.
//!
//! A ray is only trusted when every triangle it meets is crossed cleanly: it
//! passes through the open interior of the triangle and its endpoints are
//! strictly on opposite sides of the triangle's plane. Passing through an edge
//! or a vertex, or lying in a plane, is detected exactly and the direction is
//! redrawn from the seeded generator. Nothing is ever nudged. If 64 directions
//! in a row are degenerate the point is reported undecided, which blocks.

use super::bvh::Bvh;
use super::exact::{P3, SegTri, point_on_triangle, seg_tri, side};
use super::mesh::Mesh;

pub const MAX_REDRAWS: u32 = 64;
/// Two crossings of one chord closer than this (nm along the chord) are not ordered
/// reliably in `f64`; the chord is redrawn.
const TIE_NM: f64 = 0.01;

/// SplitMix64: small, fast, and identical on every platform.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// A generator for item `index` of a stream seeded `seed`, independent of thread scheduling.
    pub fn for_item(seed: u64, index: u64) -> Self {
        let mut r = Self(seed ^ index.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        r.next_u64();
        r
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in `lo..=hi`.
    pub fn range_i64(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next_u64() % (hi - lo + 1) as u64) as i64
    }

    pub fn unit_vector(&mut self) -> [f64; 3] {
        let z = 2.0 * self.next_f64() - 1.0;
        let phi = std::f64::consts::TAU * self.next_f64();
        let r = (1.0 - z * z).sqrt();
        [r * phi.cos(), r * phi.sin(), z]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RayError {
    /// Every direction tried met an edge, a vertex or a plane.
    Undecided,
}

/// What a point is inside, by exact ray parity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PointClass {
    /// Volumes (1-based) the point is strictly inside, sorted.
    pub volumes: Vec<u32>,
    /// Boundary components (see [`Caster::with_components`]) the point is inside, sorted.
    pub components: Vec<u32>,
    /// Volumes (1-based) whose surface the point lies on; their parity is not decided.
    pub on_surface_of: Vec<u32>,
    /// Directions drawn before one was clean.
    pub redraws: u32,
}

/// One clean crossing of a chord.
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub tri: u32,
    /// Position along the chord, 0 at the start and 1 at the end.
    pub t: f64,
    /// The chord moves from the side the normal points away from to the side it points to.
    pub towards_normal: bool,
}

pub struct Caster<'a> {
    pub mesh: &'a Mesh,
    pub bvh: &'a Bvh,
    /// Slots per surface: volume slots, then an optional component slot.
    slots: Vec<[u32; 3]>,
    n_slots: usize,
    n_volumes: usize,
}

const NO_SLOT: u32 = u32::MAX;

pub struct Scratch {
    parity: Vec<u8>,
    touched: Vec<u32>,
    on_surface: Vec<u32>,
    pub hits: Vec<Hit>,
}

impl<'a> Caster<'a> {
    /// A caster that tracks the parity of every volume.
    pub fn new(mesh: &'a Mesh, bvh: &'a Bvh) -> Self {
        Self::with_components(mesh, bvh, &[], 0)
    }

    /// Also track parity of boundary components: `component_of_surface[s]` is the
    /// component of surface `s` (or `u32::MAX` for none).
    pub fn with_components(
        mesh: &'a Mesh,
        bvh: &'a Bvh,
        component_of_surface: &[u32],
        n_components: usize,
    ) -> Self {
        let n_volumes = mesh.n_volumes as usize;
        let slots = mesh
            .surfaces
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let volume_slot = |v: u32| if v == 0 { NO_SLOT } else { v - 1 };
                let component = component_of_surface
                    .get(i)
                    .copied()
                    .filter(|&c| c != u32::MAX)
                    .map_or(NO_SLOT, |c| n_volumes as u32 + c);
                [volume_slot(s.forward), volume_slot(s.reverse), component]
            })
            .collect();
        Self {
            mesh,
            bvh,
            slots,
            n_slots: n_volumes + n_components,
            n_volumes,
        }
    }

    pub fn scratch(&self) -> Scratch {
        Scratch {
            parity: vec![0; self.n_slots],
            touched: Vec::new(),
            on_surface: Vec::new(),
            hits: Vec::new(),
        }
    }

    /// A far end for a ray or half-chord from `p`: beyond the box by a quarter of its reach.
    pub fn far_point(&self, p: P3, direction: [f64; 3]) -> P3 {
        // The farthest the ray must go is the box diagonal from p (p is inside the box);
        // a quarter more and a micron's margin put the end strictly outside.
        let distance = 1.25 * self.mesh.diagonal() + 1024.0;
        [
            p[0] + (direction[0] * distance).round() as i64,
            p[1] + (direction[1] * distance).round() as i64,
            p[2] + (direction[2] * distance).round() as i64,
        ]
    }

    fn toggle(&self, scratch: &mut Scratch, tri: u32) {
        for &slot in &self.slots[self.mesh.tri_surface[tri as usize] as usize] {
            if slot != NO_SLOT {
                let s = slot as usize;
                if scratch.parity[s] == 0 {
                    scratch.touched.push(slot);
                }
                scratch.parity[s] ^= 1;
            }
        }
    }

    fn reset(scratch: &mut Scratch) {
        for &slot in &scratch.touched {
            scratch.parity[slot as usize] = 0;
        }
        scratch.touched.clear();
        scratch.on_surface.clear();
    }

    /// Classify a point by exact ray parity. The same `seed` always gives the same directions.
    pub fn classify(
        &self,
        p: P3,
        seed: u64,
        scratch: &mut Scratch,
    ) -> Result<PointClass, RayError> {
        if !self.mesh.contains(p) {
            return Ok(PointClass::default());
        }
        let mut rng = Rng::new(seed);
        for attempt in 0..MAX_REDRAWS {
            let q = self.far_point(p, rng.unit_vector());
            Self::reset(scratch);
            let mut degenerate = false;
            let mut on_triangle: Vec<u32> = Vec::new();
            self.bvh.query_segment(p, q, &mut |t| {
                if degenerate {
                    return;
                }
                let tri = self.mesh.tri_pts(t);
                match seg_tri(p, q, tri) {
                    SegTri::Miss => {}
                    SegTri::Cross => self.toggle(scratch, t),
                    SegTri::Touch => {
                        if point_on_triangle(p, tri) {
                            on_triangle.push(t);
                        } else {
                            degenerate = true;
                        }
                    }
                }
            });
            if degenerate {
                continue;
            }
            let mut on_surface: Vec<u32> = on_triangle
                .iter()
                .flat_map(|&t| self.mesh.volumes_of(t))
                .collect();
            on_surface.sort_unstable();
            on_surface.dedup();
            let mut class = PointClass {
                redraws: attempt,
                ..PointClass::default()
            };
            for &slot in &scratch.touched {
                if scratch.parity[slot as usize] == 1 {
                    if (slot as usize) < self.n_volumes {
                        class.volumes.push(slot + 1);
                    } else {
                        class.components.push(slot - self.n_volumes as u32);
                    }
                }
            }
            class.volumes.sort_unstable();
            class.volumes.dedup();
            class.components.sort_unstable();
            class.components.dedup();
            // A volume whose own surface the point lies on has an undecided parity.
            class.volumes.retain(|v| !on_surface.contains(v));
            class.on_surface_of = on_surface;
            Self::reset(scratch);
            return Ok(class);
        }
        Err(RayError::Undecided)
    }

    /// Every clean crossing of the chord `s`-`e`, in order along it. Fails if any
    /// triangle is met other than cleanly, or two crossings cannot be ordered.
    pub fn chord(&self, s: P3, e: P3, scratch: &mut Scratch) -> Result<(), RayError> {
        scratch.hits.clear();
        let mut degenerate = false;
        let direction = [e[0] - s[0], e[1] - s[1], e[2] - s[2]];
        let length = ((direction[0] as f64).powi(2)
            + (direction[1] as f64).powi(2)
            + (direction[2] as f64).powi(2))
        .sqrt();
        let hits = &mut scratch.hits;
        self.bvh.query_segment(s, e, &mut |t| {
            if degenerate {
                return;
            }
            let tri = self.mesh.tri_pts(t);
            match seg_tri(s, e, tri) {
                SegTri::Miss => {}
                SegTri::Touch => degenerate = true,
                SegTri::Cross => {
                    let [a, b, c] = tri;
                    // t = n . (a - s) / n . (e - s); each product is below 2^126.
                    let n = super::exact::normal(a, b, c);
                    let dot = |v: [i64; 3]| -> i128 {
                        n[0] * i128::from(v[0]) + n[1] * i128::from(v[1]) + n[2] * i128::from(v[2])
                    };
                    let numerator = dot([a[0] - s[0], a[1] - s[1], a[2] - s[2]]);
                    let denominator = dot(direction);
                    hits.push(Hit {
                        tri: t,
                        t: numerator as f64 / denominator as f64,
                        towards_normal: side(a, b, c, s) < 0,
                    });
                }
            }
        });
        if degenerate {
            return Err(RayError::Undecided);
        }
        hits.sort_by(|x, y| x.t.total_cmp(&y.t));
        for pair in hits.windows(2) {
            if (pair[1].t - pair[0].t) * length < TIE_NM {
                return Err(RayError::Undecided);
            }
        }
        Ok(())
    }

    /// The point at position `t` along the chord, in metres.
    pub fn chord_point_m(s: P3, e: P3, t: f64) -> [f64; 3] {
        super::mesh::to_metres([
            s[0] + ((e[0] - s[0]) as f64 * t).round() as i64,
            s[1] + ((e[1] - s[1]) as f64 * t).round() as i64,
            s[2] + ((e[2] - s[2]) as f64 * t).round() as i64,
        ])
    }

    /// The grid point at position `t` along the chord.
    pub fn chord_point(s: P3, e: P3, t: f64) -> P3 {
        [
            s[0] + ((e[0] - s[0]) as f64 * t).round() as i64,
            s[1] + ((e[1] - s[1]) as f64 * t).round() as i64,
            s[2] + ((e[2] - s[2]) as f64 * t).round() as i64,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::super::mesh::fixtures::*;
    use super::*;
    use faris_model::design::Facets;

    fn setup(f: &Facets) -> (Mesh, Bvh) {
        let m = Mesh::from_facets(f).unwrap();
        let active: Vec<u32> = (0..m.tris.len() as u32).collect();
        let b = Bvh::build(&m, active);
        (m, b)
    }

    const M: i64 = 1_000_000_000;

    // Verifies: GEO-024
    #[test]
    fn parity_classifies_inside_outside_and_shells() {
        let (m, b) = setup(&shell_and_core(10.0, 2.0));
        let c = Caster::new(&m, &b);
        let mut s = c.scratch();
        // in the core (volume 2): inside volume 2 only. The shell's surfaces are the
        // outer box (forward 1) and the shared inner box (reverse 1): a point in the core
        // crosses the shell's inner surface once and its outer surface once going out.
        let core = c.classify([0, 0, 0], 1, &mut s).unwrap();
        assert_eq!(core.volumes, vec![2]);
        // in the shell material
        let shell = c.classify([5 * M, 0, 0], 2, &mut s).unwrap();
        assert_eq!(shell.volumes, vec![1]);
        // outside the model, and outside the box
        assert!(
            c.classify([15 * M, 0, 0], 3, &mut s)
                .unwrap()
                .volumes
                .is_empty()
        );
        // on the shared surface: undecided for both volumes, never "inside"
        let on = c.classify([2 * M, 0, 0], 4, &mut s).unwrap();
        assert_eq!(on.on_surface_of, vec![1, 2]);
        assert!(on.volumes.is_empty());
        // deterministic for a seed
        let a = c.classify([3 * M, M, M], 9, &mut s).unwrap();
        let b2 = c.classify([3 * M, M, M], 9, &mut s).unwrap();
        assert_eq!(a, b2);
    }

    // Verifies: GEO-024
    #[test]
    fn a_ray_that_hits_an_edge_is_redrawn_not_nudged() {
        // Many points whose axis-aligned neighbours would hit edges: whatever the
        // direction drawn, the answer is exact. Check a grid of points against
        // the known geometry (inside the 2 m cube centred at the origin).
        let (m, b) = setup(&separate_boxes(&[([-1.0; 3], [1.0; 3])]));
        let c = Caster::new(&m, &b);
        let mut s = c.scratch();
        let mut total_redraws = 0;
        for i in -3..=3 {
            for j in -3..=3 {
                for k in -3..=3 {
                    let p = [i * M / 2 + 1, j * M / 2 + 3, k * M / 2 + 7];
                    let inside = [i, j, k].iter().all(|&x| (x * M / 2 + 5).abs() < M);
                    let class = c.classify(p, (i * 49 + j * 7 + k) as u64, &mut s);
                    let class = class.unwrap();
                    assert_eq!(class.volumes == vec![1], inside, "{p:?}");
                    total_redraws += class.redraws;
                }
            }
        }
        // some of these rays are degenerate; with random directions it is rare
        assert!(total_redraws < 50, "{total_redraws}");
    }

    // Verifies: GEO-025
    #[test]
    fn a_chord_lists_its_crossings_in_order_with_their_direction() {
        let (m, b) = setup(&shell_and_core(10.0, 2.0));
        let c = Caster::new(&m, &b);
        let mut s = c.scratch();
        let start = [-30 * M, 137, 91];
        let end = [30 * M, 137, 91];
        c.chord(start, end, &mut s).unwrap();
        let hits = &s.hits;
        assert_eq!(hits.len(), 4, "outer, inner, inner, outer");
        assert!(hits.windows(2).all(|w| w[0].t < w[1].t));
        // entering the cube moves against the outward normal; leaving moves along it
        assert!(!hits[0].towards_normal);
        assert!(hits[3].towards_normal);
        let first_point = Caster::chord_point(start, end, hits[0].t);
        assert!((first_point[0] + 10 * M).abs() <= 2);
    }

    #[test]
    fn a_chord_through_an_edge_is_undecided_and_two_equal_surfaces_tie() {
        let (m, b) = setup(&separate_boxes(&[([-1.0; 3], [1.0; 3])]));
        let c = Caster::new(&m, &b);
        let mut s = c.scratch();
        // along the cube's edge direction through a vertex
        assert_eq!(
            c.chord([-5 * M, -M, -M], [5 * M, -M, -M], &mut s),
            Err(RayError::Undecided)
        );
        // two coincident boxes: every chord has tied crossings
        let (m, b) = setup(&separate_boxes(&[
            ([-1.0; 3], [1.0; 3]),
            ([-1.0; 3], [1.0; 3]),
        ]));
        let c = Caster::new(&m, &b);
        let mut s = c.scratch();
        assert_eq!(
            c.chord([-5 * M, 11, 13], [5 * M, 11, 13], &mut s),
            Err(RayError::Undecided)
        );
    }

    #[test]
    fn the_generator_is_reproducible_and_spread() {
        let mut a = Rng::new(5);
        let mut b = Rng::new(5);
        assert_eq!(a.next_u64(), b.next_u64());
        let mut r = Rng::for_item(5, 17);
        let mean = (0..10_000).map(|_| r.next_f64()).sum::<f64>() / 10_000.0;
        assert!((mean - 0.5).abs() < 0.02);
        let v = Rng::new(1).unit_vector();
        assert!((v.iter().map(|x| x * x).sum::<f64>() - 1.0).abs() < 1e-12);
        assert_ne!(
            Rng::for_item(5, 1).next_u64(),
            Rng::for_item(5, 2).next_u64()
        );
    }
}
