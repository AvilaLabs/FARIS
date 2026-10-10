//! Edge topology of the faceted model: watertightness and orientation per
//! volume (GEO-020), and the connected components of the surfaces that face the
//! implicit complement, with their enclosed volume (GEO-022).
//!
//! A volume is watertight when every edge of its triangles is used exactly
//! twice, once in each direction, after applying each surface's sense: a
//! triangle of a surface for which the volume is the forward volume is read as
//! stored, and for which it is the reverse volume, backwards. An edge used once
//! is open, an edge used more than twice is non-manifold, and an edge used twice
//! in the same direction means two triangles disagree about which side is inside.

use super::mesh::Mesh;
use super::par::{Cancelled, Ctl};

/// What is wrong with one volume's edges.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EdgeProblems {
    pub volume: u32,
    pub open: u64,
    pub non_manifold: u64,
    pub misoriented: u64,
    /// The first edge of each kind, as vertex ids.
    pub first_open: Option<(u32, u32)>,
    pub first_non_manifold: Option<(u32, u32)>,
    pub first_misoriented: Option<(u32, u32)>,
    /// Triangles that repeat a vertex.
    pub repeated_vertex_triangles: u64,
}

impl EdgeProblems {
    #[cfg(test)]
    pub fn clean(&self) -> bool {
        self.open == 0
            && self.non_manifold == 0
            && self.misoriented == 0
            && self.repeated_vertex_triangles == 0
    }
}

fn edge_key(from: u32, to: u32) -> u64 {
    let (lo, hi, direction) = if from < to {
        (from, to, 0u64)
    } else {
        (to, from, 1u64)
    };
    (u64::from(lo) << 33) | (u64::from(hi) << 1) | direction
}

fn key_edge(key: u64) -> (u32, u32) {
    (
        ((key >> 33) & 0xFFFF_FFFF) as u32,
        ((key >> 1) & 0xFFFF_FFFF) as u32,
    )
}

/// Check every volume's edges, one volume per work item.
pub fn edge_problems(mesh: &Mesh, ctl: &Ctl) -> Result<Vec<EdgeProblems>, Cancelled> {
    let tris = mesh.volume_triangles();
    let chunks = ctl.run("edges", mesh.n_volumes as usize, 1, |range| {
        let index = range.start;
        let volume = index as u32 + 1;
        let mut problems = EdgeProblems {
            volume,
            ..EdgeProblems::default()
        };
        let mut keys: Vec<u64> = Vec::with_capacity(tris[index].len() * 3);
        for &t in &tris[index] {
            let [a, b, c] = mesh.tris[t as usize];
            if a == b || b == c || c == a {
                problems.repeated_vertex_triangles += 1;
                continue;
            }
            let surface = mesh.surface_of(t);
            let reversed = surface.reverse == volume && surface.forward != volume;
            let edges = if reversed {
                [(b, a), (c, b), (a, c)]
            } else {
                [(a, b), (b, c), (c, a)]
            };
            for (from, to) in edges {
                keys.push(edge_key(from, to));
            }
        }
        keys.sort_unstable();
        let mut i = 0;
        while i < keys.len() {
            let edge = keys[i] >> 1;
            let mut j = i;
            let (mut forward, mut backward) = (0u64, 0u64);
            while j < keys.len() && keys[j] >> 1 == edge {
                if keys[j] & 1 == 0 {
                    forward += 1;
                } else {
                    backward += 1;
                }
                j += 1;
            }
            let uses = forward + backward;
            let at = key_edge(keys[i]);
            if uses == 1 {
                problems.open += 1;
                problems.first_open.get_or_insert(at);
            } else if uses > 2 {
                problems.non_manifold += 1;
                problems.first_non_manifold.get_or_insert(at);
            } else if forward != 1 {
                problems.misoriented += 1;
                problems.first_misoriented.get_or_insert(at);
            }
            i = j;
        }
        problems
    })?;
    Ok(chunks)
}

/// A connected set of triangles facing the implicit complement.
#[derive(Clone, Debug)]
pub struct Component {
    pub triangles: Vec<u32>,
    pub surfaces: Vec<u32>,
    /// Six times the enclosed signed volume with the normals pointing into the
    /// implicit complement, in grid units cubed. Positive: the complement is
    /// outside (the boundary of a cluster of solids). Negative: the complement is
    /// inside (the wall of an enclosed cavity).
    pub six_volume: i128,
    pub lo: [i64; 3],
    pub hi: [i64; 3],
    /// The solids (volumes) the component bounds.
    pub volumes: Vec<u32>,
    /// A vertex of the component, used to place it among the others.
    pub vertex: [i64; 3],
}

struct UnionFind(Vec<u32>);

impl UnionFind {
    fn new(n: usize) -> Self {
        Self((0..n as u32).collect())
    }
    fn find(&mut self, mut x: u32) -> u32 {
        while self.0[x as usize] != x {
            self.0[x as usize] = self.0[self.0[x as usize] as usize];
            x = self.0[x as usize];
        }
        x
    }
    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra.max(rb) as usize] = ra.min(rb);
        }
    }
}

/// The connected components of the surfaces with exactly one volume, in order of
/// their lowest surface index; `surface_component[s]` gives each surface's component (or `u32::MAX`).
/// Returns `Err` if a component's enclosed volume overflows `i128`.
pub fn boundary_components(mesh: &Mesh) -> Result<(Vec<Component>, Vec<u32>), String> {
    let boundary: Vec<u32> = (0..mesh.surfaces.len() as u32)
        .filter(|&s| {
            let surface = &mesh.surfaces[s as usize];
            (surface.forward == 0) != (surface.reverse == 0)
        })
        .collect();
    let mut surface_component = vec![u32::MAX; mesh.surfaces.len()];
    if boundary.is_empty() {
        return Ok((Vec::new(), surface_component));
    }
    // Union-find over the boundary surfaces: a surface is atomic, and two surfaces
    // that share an edge join.
    let local: std::collections::HashMap<u32, u32> = boundary
        .iter()
        .enumerate()
        .map(|(i, &s)| (s, i as u32))
        .collect();
    let mut uf = UnionFind::new(boundary.len());
    let mut edges: Vec<(u64, u32)> = Vec::new();
    for &s in &boundary {
        let surface = &mesh.surfaces[s as usize];
        for t in surface.first_triangle..surface.first_triangle + surface.triangle_count {
            let [a, b, c] = mesh.tris[t as usize];
            for (u, w) in [(a, b), (b, c), (c, a)] {
                let (lo, hi) = if u < w { (u, w) } else { (w, u) };
                edges.push(((u64::from(lo) << 32) | u64::from(hi), local[&s]));
            }
        }
    }
    edges.sort_unstable();
    for pair in edges.windows(2) {
        if pair[0].0 == pair[1].0 {
            uf.union(pair[0].1, pair[1].1);
        }
    }
    let mut ids: std::collections::BTreeMap<u32, u32> = std::collections::BTreeMap::new();
    for (i, &s) in boundary.iter().enumerate() {
        let root = uf.find(i as u32);
        let next = ids.len() as u32;
        let id = *ids.entry(root).or_insert(next);
        surface_component[s as usize] = id;
    }
    let mut components: Vec<Component> = (0..ids.len())
        .map(|_| Component {
            triangles: Vec::new(),
            surfaces: Vec::new(),
            six_volume: 0,
            lo: [i64::MAX; 3],
            hi: [i64::MIN; 3],
            volumes: Vec::new(),
            vertex: [0; 3],
        })
        .collect();
    for &s in &boundary {
        let c = &mut components[surface_component[s as usize] as usize];
        let surface = &mesh.surfaces[s as usize];
        c.surfaces.push(s);
        let volume = surface.forward.max(surface.reverse);
        if !c.volumes.contains(&volume) {
            c.volumes.push(volume);
        }
        for t in surface.first_triangle..surface.first_triangle + surface.triangle_count {
            c.triangles.push(t);
            // Orient the triangle out of its volume, which is into the implicit complement.
            let [a, b, cc] = mesh.oriented_for(t, volume);
            c.six_volume = c
                .six_volume
                .checked_add(super::mesh::six_volume_term(mesh.lo, a, b, cc))
                .ok_or("a boundary component's enclosed volume overflows the exact 128-bit sum")?;
            for p in [a, b, cc] {
                for (k, &coordinate) in p.iter().enumerate() {
                    c.lo[k] = c.lo[k].min(coordinate);
                    c.hi[k] = c.hi[k].max(coordinate);
                }
            }
        }
    }
    for c in &mut components {
        c.volumes.sort_unstable();
        c.vertex = mesh.tri_pts(c.triangles[0])[0];
    }
    Ok((components, surface_component))
}

#[cfg(test)]
mod tests {
    use super::super::mesh::fixtures::*;
    use super::*;
    use crate::jobs::Cancellation;
    use faris_model::design::Facets;

    fn ctl_run<T>(f: impl FnOnce(&Ctl) -> T) -> T {
        let cancel = Cancellation::default();
        let progress = |_: &str, _: usize, _: usize| {};
        f(&Ctl::new(&cancel, &progress, 2))
    }

    fn problems(f: &Facets) -> Vec<EdgeProblems> {
        let mesh = Mesh::from_facets(f).unwrap();
        ctl_run(|ctl| edge_problems(&mesh, ctl)).unwrap()
    }

    // Verifies: GEO-020
    #[test]
    fn closed_boxes_and_shells_are_watertight_and_consistently_oriented() {
        assert!(
            problems(&separate_boxes(&[
                ([0.0; 3], [1.0; 3]),
                ([2.0; 3], [3.0; 3])
            ]))
            .iter()
            .all(EdgeProblems::clean)
        );
        assert!(
            problems(&shell_and_core(10.0, 2.0))
                .iter()
                .all(EdgeProblems::clean)
        );
    }

    // Verifies: GEO-020
    #[test]
    fn a_deleted_triangle_leaves_open_edges() {
        let mut f = separate_boxes(&[([0.0; 3], [1.0; 3])]);
        f.triangles.remove(3);
        f.surfaces[0].triangle_count -= 1;
        let p = &problems(&f)[0];
        assert_eq!(p.open, 3, "{p:?}");
        assert_eq!(p.non_manifold, 0);
        assert!(p.first_open.is_some());
    }

    // Verifies: GEO-020
    #[test]
    fn a_duplicated_triangle_makes_a_non_manifold_edge() {
        let mut f = separate_boxes(&[([0.0; 3], [1.0; 3])]);
        let t = f.triangles[2];
        f.triangles.push(t);
        f.surfaces[0].triangle_count += 1;
        let p = &problems(&f)[0];
        assert_eq!(p.non_manifold, 3);
        assert_eq!(p.open, 0);
    }

    // Verifies: GEO-020
    #[test]
    fn a_flipped_triangle_is_a_orientation_failure_not_an_open_edge() {
        let mut f = separate_boxes(&[([0.0; 3], [1.0; 3])]);
        f.triangles[0].swap(1, 2);
        let p = &problems(&f)[0];
        assert_eq!(p.misoriented, 3, "{p:?}");
        assert_eq!(p.open, 0);
    }

    #[test]
    fn a_whole_surface_with_its_sense_swapped_keeps_its_edges_consistent() {
        // Swapping the sense of a whole surface keeps every edge consistent inside each
        // volume; it shows as an inverted signed volume (checked elsewhere), not here.
        let mut f = shell_and_core(10.0, 2.0);
        f.surfaces[1].forward_volume = 1;
        f.surfaces[1].reverse_volume = 2;
        let p = problems(&f);
        assert!(p[0].clean() && p[1].clean(), "{p:?}");
    }

    // Verifies: GEO-022
    #[test]
    fn boundary_components_separate_clusters_and_cavity_walls() {
        // A shell with a hollow (no core): the inner surface faces the complement from the shell.
        let mut f = shell_and_core(10.0, 2.0);
        // make the inner surface belong to the shell only, facing the complement (a hollow)
        f.surfaces[1].forward_volume = 0;
        f.surfaces[1].reverse_volume = 1;
        f.n_volumes = 1;
        let mesh = Mesh::from_facets(&f).unwrap();
        let (components, by_surface) = boundary_components(&mesh).unwrap();
        assert_eq!(components.len(), 2);
        assert_eq!(by_surface, vec![0, 1]);
        let outer = &components[0];
        let hollow = &components[1];
        assert!(
            outer.six_volume > 0,
            "outer boundary encloses positive volume"
        );
        assert!(
            hollow.six_volume < 0,
            "the hollow's wall encloses the complement"
        );
        let hollow_m3 = -hollow.six_volume as f64 / 6.0 / 1e27;
        assert!((hollow_m3 - 64.0).abs() < 1e-9, "{hollow_m3}");
        // a fully imprinted pair has no boundary surface inside
        let mesh = Mesh::from_facets(&shell_and_core(10.0, 2.0)).unwrap();
        let (components, _) = boundary_components(&mesh).unwrap();
        assert_eq!(components.len(), 1);
    }
}
