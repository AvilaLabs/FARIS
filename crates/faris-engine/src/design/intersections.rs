//! Triangles that meet where they should not: self-intersection, overlap and
//! duplicate surfaces (GEO-021), found from the faceted surfaces themselves.
//!
//! Every triangle is tested exactly against every triangle whose box overlaps
//! it. Two triangles may legitimately meet only in the vertices or edge they
//! share (by mesh vertex identity); any other contact is a finding. What kind
//! depends on which volumes the two triangles bound:
//!
//! * the same surface, or a volume in common: the volume's boundary meets itself
//!   (self-intersection, which includes a fold-over of coplanar triangles);
//! * no volume in common and the triangles coplanar: two surfaces coincide but
//!   were not merged (duplicate surface);
//! * no volume in common otherwise: the volumes on either side overlap.

use super::exact::{Relation, relate};
use super::mesh::Mesh;
use super::model::Model;
use super::par::{Cancelled, Ctl};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    SelfIntersection,
    /// Edges of one volume pass through the other.
    Overlap,
    /// Faces of different volumes touch without sharing the vertices a conforming mesh would.
    Contact,
    DuplicateSurface,
}

/// `a` and `b` are volumes (1-based, `a <= b`; `b` is 0 for a single volume) for
/// self-intersection and overlap, and surface indices for a duplicate surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub kind: Kind,
    pub a: u32,
    pub b: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct Aggregate {
    /// Triangle pairs that meet.
    pub pairs: u64,
    /// The lowest-numbered triangle of any pair, and the one it meets.
    pub first: (u32, u32),
    pub proper: bool,
    pub coplanar: bool,
}

fn merge(into: &mut BTreeMap<Key, Aggregate>, key: Key, value: Aggregate) {
    into.entry(key)
        .and_modify(|e| {
            e.pairs += value.pairs;
            e.proper |= value.proper;
            e.coplanar |= value.coplanar;
            if value.first < e.first {
                e.first = value.first;
            }
        })
        .or_insert(value);
}

fn shared_vertices(a: [u32; 3], b: [u32; 3]) -> ([(usize, usize); 3], usize) {
    let mut out = [(0, 0); 3];
    let mut n = 0;
    for (i, &x) in a.iter().enumerate() {
        for (j, &y) in b.iter().enumerate() {
            if x == y {
                out[n] = (i, j);
                n += 1;
            }
        }
    }
    (out, n)
}

fn volumes(mesh: &Mesh, t: u32) -> [u32; 2] {
    let s = mesh.surface_of(t);
    [s.forward, s.reverse]
}

/// Classify one meeting of triangles `t` and `u` and add it to `out`.
fn record(
    mesh: &Mesh,
    t: u32,
    u: u32,
    coplanar: bool,
    proper: bool,
    out: &mut BTreeMap<Key, Aggregate>,
) {
    let (st, su) = (mesh.tri_surface[t as usize], mesh.tri_surface[u as usize]);
    let (vt, vu) = (volumes(mesh, t), volumes(mesh, u));
    let first = (t.min(u), t.max(u));
    let value = Aggregate {
        pairs: 1,
        first,
        proper,
        coplanar,
    };
    let between = if proper { Kind::Overlap } else { Kind::Contact };
    let common: Vec<u32> = vt
        .iter()
        .copied()
        .filter(|&v| v != 0 && vu.contains(&v))
        .collect();
    if st == su || !common.is_empty() {
        let own: Vec<u32> = if st == su {
            vt.iter().copied().filter(|&v| v != 0).collect()
        } else {
            common.clone()
        };
        for v in own {
            merge(
                out,
                Key {
                    kind: Kind::SelfIntersection,
                    a: v,
                    b: 0,
                },
                value,
            );
        }
        // Surfaces (A, B) and (A, C) crossing also make B and C overlap.
        if st != su {
            for &x in vt.iter().filter(|&&v| v != 0 && !common.contains(&v)) {
                for &y in vu.iter().filter(|&&v| v != 0 && !common.contains(&v)) {
                    if x != y && !coplanar {
                        merge(
                            out,
                            Key {
                                kind: between,
                                a: x.min(y),
                                b: x.max(y),
                            },
                            value,
                        );
                    }
                }
            }
        }
        return;
    }
    if coplanar {
        merge(
            out,
            Key {
                kind: Kind::DuplicateSurface,
                a: st.min(su),
                b: st.max(su),
            },
            value,
        );
        return;
    }
    for &x in vt.iter().filter(|&&v| v != 0) {
        for &y in vu.iter().filter(|&&v| v != 0) {
            if x != y {
                merge(
                    out,
                    Key {
                        kind: between,
                        a: x.min(y),
                        b: x.max(y),
                    },
                    value,
                );
            }
        }
    }
}

/// Test every triangle against its neighbours by box. Returns the findings grouped by key.
pub fn intersection_pass(model: &Model, ctl: &Ctl) -> Result<BTreeMap<Key, Aggregate>, Cancelled> {
    let mesh = &model.mesh;
    let n = mesh.tris.len();
    let mut degenerate = vec![false; n];
    for &t in &mesh.degenerate {
        degenerate[t as usize] = true;
    }
    let parts = ctl.run("triangle intersections", n, 2048, |range| {
        let mut local: BTreeMap<Key, Aggregate> = BTreeMap::new();
        for t in range {
            let t = t as u32;
            if degenerate[t as usize] {
                continue;
            }
            let ids = mesh.tris[t as usize];
            let tri = mesh.tri_pts(t);
            let (lo, hi) = model.bvh.triangle_box(t);
            model.bvh.query_box(lo, hi, &mut |u| {
                if u <= t {
                    return;
                }
                let other_ids = mesh.tris[u as usize];
                let (shared, k) = shared_vertices(ids, other_ids);
                if let Relation::Intersect { coplanar, proper } =
                    relate(tri, mesh.tri_pts(u), &shared[..k])
                {
                    record(mesh, t, u, coplanar, proper, &mut local);
                }
            });
        }
        local
    })?;
    let mut all = BTreeMap::new();
    for part in parts {
        for (key, value) in part {
            merge(&mut all, key, value);
        }
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::super::mesh::fixtures::*;
    use super::*;
    use crate::jobs::Cancellation;
    use faris_model::design::Facets;

    fn run(f: &Facets) -> BTreeMap<Key, Aggregate> {
        let model = Model::new(f).unwrap();
        let cancel = Cancellation::default();
        let progress = |_: &str, _: usize, _: usize| {};
        intersection_pass(&model, &Ctl::new(&cancel, &progress, 2)).unwrap()
    }

    // Verifies: GEO-021
    #[test]
    fn separate_and_touching_clean_meshes_have_no_findings() {
        assert!(
            run(&separate_boxes(&[
                ([0.0; 3], [1.0; 3]),
                ([5.0; 3], [6.0; 3])
            ]))
            .is_empty()
        );
        assert!(run(&shell_and_core(10.0, 2.0)).is_empty());
    }

    // Verifies: GEO-021
    #[test]
    fn overlapping_boxes_are_an_overlap_of_the_two_volumes() {
        let found = run(&separate_boxes(&[
            ([0.0; 3], [2.0; 3]),
            ([0.7, 0.5, 0.3], [2.9, 2.6, 2.2]),
        ]));
        let key = Key {
            kind: Kind::Overlap,
            a: 1,
            b: 2,
        };
        assert!(found.get(&key).is_some_and(|a| a.proper), "{found:?}");
        assert_eq!(found.len(), 1);
    }

    // Verifies: GEO-021
    #[test]
    fn coincident_unmerged_faces_are_a_duplicate_surface() {
        // two boxes sharing the face x = 1, each with its own surface
        let found = run(&separate_boxes(&[
            ([0.0; 3], [1.0; 3]),
            ([1.0, 0.0, 0.0], [2.0, 1.0, 1.0]),
        ]));
        assert!(
            found.keys().any(|k| k.kind == Kind::DuplicateSurface),
            "{found:?}"
        );
        assert!(
            found.keys().all(|k| k.kind != Kind::Overlap),
            "touching is not overlap: {found:?}"
        );
        assert!(found.keys().any(|k| k.kind == Kind::Contact), "{found:?}");
    }

    // Verifies: GEO-021
    #[test]
    fn a_vertex_pushed_through_the_opposite_face_is_a_self_intersection() {
        let mut f = separate_boxes(&[([0.0; 3], [2.0; 3])]);
        // vertex 6 (2,2,2) moved to the far side of the cube's x = 0 face
        f.vertices[6] = [-1.0, 0.5, 0.5];
        let found = run(&f);
        assert!(
            found.contains_key(&Key {
                kind: Kind::SelfIntersection,
                a: 1,
                b: 0
            }),
            "{found:?}"
        );
    }

    #[test]
    fn a_body_inside_another_has_no_surface_contact_so_the_pass_is_silent() {
        // containment is decided by point-in-volume, not by this pass
        let found = run(&separate_boxes(&[
            ([0.0; 3], [10.0; 3]),
            ([4.0; 3], [5.0; 3]),
        ]));
        assert!(found.is_empty());
    }
}
