//! GEO-021 before imprinting. Imprinting can split overlapping solids into
//! fragments, so an overlap may not survive into the imprinted model in a form
//! the faceted check sees. The CAD helper therefore meshes every solid on its
//! own, and this module finds the *candidate* pairs from those meshes with
//! exact predicates:
//!
//! * pairs whose triangles cross, or coincide (touching faces);
//! * pairs whose triangles come within twice the faceting tolerance (the
//!   distance is `f64`, with a margin that only ever adds candidates);
//! * pairs where one mesh lies inside the other (exact point-in-volume of one
//!   vertex).
//!
//! The CAD helper then runs OCCT's exact boolean intersection on the candidates
//! only. A common volume above 1e-6 of the smaller solid's volume is an overlap;
//! a smaller one is touching. A boolean that fails or returns an invalid shape is
//! undecided: it blocks, and it can be waived.

use super::exact::P3;
use super::intersections::{self, Kind};
use super::mesh::to_metres;
use super::model::Model;
use super::par::{Cancelled, Ctl};
use super::rays::{Caster, Rng};
use faris_model::design::Finding;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const CHECK: u8 = 6;
/// The smallest common volume, as a fraction of the smaller solid, that is an overlap.
pub const OVERLAP_FRACTION: f64 = 1.0e-6;
/// Candidates within this many faceting tolerances.
pub const NEAR_FACTOR: f64 = 2.0;
/// Added to the near threshold, nm, so rounding can only add candidates.
const MARGIN_NM: f64 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Crossing,
    Coincident,
    Contained,
    Near,
}

/// A pair of solids (indices into the STEP file's solids, from 0) to test with OCCT.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub a: usize,
    pub b: usize,
    pub reason: Reason,
    /// A place between the two solids, metres.
    pub at_m: [f64; 3],
}

type V3 = [f64; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn f(p: P3) -> V3 {
    [p[0] as f64, p[1] as f64, p[2] as f64]
}

/// Squared distance from `p` to triangle `t` (Ericson, Real-Time Collision Detection).
fn point_triangle_sq(p: V3, t: [V3; 3]) -> f64 {
    let [a, b, c] = t;
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    let at = |q: V3| {
        let d = sub(p, q);
        dot(d, d)
    };
    if d1 <= 0.0 && d2 <= 0.0 {
        return at(a);
    }
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return at(b);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return at([a[0] + v * ab[0], a[1] + v * ab[1], a[2] + v * ab[2]]);
    }
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return at(c);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return at([a[0] + w * ac[0], a[1] + w * ac[1], a[2] + w * ac[2]]);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        let bc = sub(c, b);
        return at([b[0] + w * bc[0], b[1] + w * bc[1], b[2] + w * bc[2]]);
    }
    let denom = 1.0 / (va + vb + vc);
    let (v, w) = (vb * denom, vc * denom);
    at([
        a[0] + ab[0] * v + ac[0] * w,
        a[1] + ab[1] * v + ac[1] * w,
        a[2] + ab[2] * v + ac[2] * w,
    ])
}

/// Distance from a point to a triangle, nm (`f64`).
pub fn point_triangle_distance(p: P3, t: [P3; 3]) -> f64 {
    point_triangle_sq(f(p), t.map(f)).sqrt()
}

/// Squared distance between segments `p1 q1` and `p2 q2`.
fn segment_segment_sq(p1: V3, q1: V3, p2: V3, q2: V3) -> f64 {
    let (d1, d2, r) = (sub(q1, p1), sub(q2, p2), sub(p1, p2));
    let (a, e, ff) = (dot(d1, d1), dot(d2, d2), dot(d2, r));
    let clamp = |x: f64| x.clamp(0.0, 1.0);
    let (s, t);
    if a <= 1e-30 && e <= 1e-30 {
        (s, t) = (0.0, 0.0);
    } else if a <= 1e-30 {
        (s, t) = (0.0, clamp(ff / e));
    } else {
        let c = dot(d1, r);
        if e <= 1e-30 {
            (s, t) = (clamp(-c / a), 0.0);
        } else {
            let b = dot(d1, d2);
            let denom = a * e - b * b;
            let mut s0 = if denom > 1e-30 {
                clamp((b * ff - c * e) / denom)
            } else {
                0.0
            };
            let mut t0 = (b * s0 + ff) / e;
            if t0 < 0.0 {
                t0 = 0.0;
                s0 = clamp(-c / a);
            } else if t0 > 1.0 {
                t0 = 1.0;
                s0 = clamp((b - c) / a);
            }
            (s, t) = (s0, t0);
        }
    }
    let c1 = [p1[0] + d1[0] * s, p1[1] + d1[1] * s, p1[2] + d1[2] * s];
    let c2 = [p2[0] + d2[0] * t, p2[1] + d2[1] * t, p2[2] + d2[2] * t];
    let d = sub(c1, c2);
    dot(d, d)
}

/// Distance between two triangles that do not cross, nm (`f64`).
pub fn triangle_distance(t1: [P3; 3], t2: [P3; 3]) -> f64 {
    let (a, b) = (t1.map(f), t2.map(f));
    let mut best = f64::INFINITY;
    for p in a {
        best = best.min(point_triangle_sq(p, b));
    }
    for p in b {
        best = best.min(point_triangle_sq(p, a));
    }
    for i in 0..3 {
        for j in 0..3 {
            best = best.min(segment_segment_sq(
                a[i],
                a[(i + 1) % 3],
                b[j],
                b[(j + 1) % 3],
            ));
        }
    }
    best.sqrt()
}

fn place(model: &Model, a: u32, b: u32) -> [f64; 3] {
    let mesh = &model.mesh;
    let per = mesh.volume_triangles();
    let boxed = |v: u32| {
        let mut lo = [i64::MAX; 3];
        let mut hi = [i64::MIN; 3];
        for &t in &per[v as usize - 1] {
            let (l, h) = model.bvh.triangle_box(t);
            for k in 0..3 {
                lo[k] = lo[k].min(l[k]);
                hi[k] = hi[k].max(h[k]);
            }
        }
        (lo, hi)
    };
    let (la, ha) = boxed(a);
    let (lb, hb) = boxed(b);
    let mut mid = [0i64; 3];
    for k in 0..3 {
        let (lo, hi) = (la[k].max(lb[k]), ha[k].min(hb[k]));
        mid[k] = if lo <= hi {
            (lo + hi) / 2
        } else {
            ((la[k] + ha[k]) / 4) + ((lb[k] + hb[k]) / 4)
        };
    }
    to_metres(mid)
}

/// Candidate pairs from the unimprinted per-solid meshes. Volume `v` is solid `v - 1`.
pub fn candidate_pairs(
    model: &Model,
    chord_m: f64,
    seed: u64,
    ctl: &Ctl,
) -> Result<Vec<Candidate>, Cancelled> {
    let mesh = &model.mesh;
    let mut found: std::collections::BTreeMap<(u32, u32), Reason> = Default::default();
    let mut add = |a: u32, b: u32, reason: Reason| {
        if a == b || a == 0 || b == 0 {
            return;
        }
        let key = (a.min(b), a.max(b));
        found
            .entry(key)
            .and_modify(|r| *r = (*r).min(reason))
            .or_insert(reason);
    };
    let owner = |t: u32| mesh.surface_of(t).forward.max(mesh.surface_of(t).reverse);

    // 1. triangles that cross or coincide
    let crossings = intersections::intersection_pass(model, ctl)?;
    for (key, agg) in &crossings {
        match key.kind {
            Kind::Overlap => add(key.a, key.b, Reason::Crossing),
            Kind::Contact | Kind::DuplicateSurface => {
                let (u, w) = agg.first;
                add(owner(u), owner(w), Reason::Coincident);
            }
            Kind::SelfIntersection => {}
        }
    }

    // 2. triangles within twice the tolerance
    let reach = (NEAR_FACTOR * chord_m * 1.0e9).ceil() as i64 + MARGIN_NM as i64;
    let limit = NEAR_FACTOR * chord_m * 1.0e9 + MARGIN_NM;
    let n = mesh.tris.len();
    let mut degenerate = vec![false; n];
    for &t in &mesh.degenerate {
        degenerate[t as usize] = true;
    }
    let near = ctl.run("candidate pairs", n, 2048, |range| {
        let mut local: BTreeSet<(u32, u32)> = BTreeSet::new();
        for t in range {
            let t = t as u32;
            if degenerate[t as usize] {
                continue;
            }
            let vt = owner(t);
            let tri = mesh.tri_pts(t);
            let (lo, hi) = model.bvh.triangle_box(t);
            let lo = [lo[0] - reach, lo[1] - reach, lo[2] - reach];
            let hi = [hi[0] + reach, hi[1] + reach, hi[2] + reach];
            model.bvh.query_box(lo, hi, &mut |u| {
                if u <= t {
                    return;
                }
                let vu = owner(u);
                if vu == vt {
                    return;
                }
                let key = (vt.min(vu), vt.max(vu));
                if local.contains(&key) {
                    return;
                }
                if triangle_distance(tri, mesh.tri_pts(u)) <= limit {
                    local.insert(key);
                }
            });
        }
        local
    })?;
    for set in near {
        for (a, b) in set {
            add(a, b, Reason::Near);
        }
    }

    // 3. one mesh inside another
    let caster = Caster::new(mesh, &model.bvh);
    let per = mesh.volume_triangles();
    let inside = ctl.run("containment candidates", per.len(), 1, |range| {
        let mut scratch = caster.scratch();
        let mut out = Vec::new();
        for i in range {
            let Some(&t) = per[i].first() else { continue };
            let vertex = mesh.tri_pts(t)[0];
            let seed = Rng::for_item(seed ^ 0xCA, i as u64).next_u64();
            match caster.classify(vertex, seed, &mut scratch) {
                Ok(class) => out.extend(class.volumes.iter().map(|&o| (i as u32 + 1, o, true))),
                // undecided: treat every other solid whose box holds the vertex as a candidate
                Err(_) => out.push((i as u32 + 1, 0, false)),
            }
        }
        out
    })?;
    for (v, o, decided) in inside.into_iter().flatten() {
        if decided {
            add(v, o, Reason::Contained);
        } else {
            for other in 1..=mesh.n_volumes {
                add(v, other, Reason::Contained);
            }
        }
    }
    Ok(found
        .into_iter()
        .map(|((a, b), reason)| Candidate {
            a: a as usize - 1,
            b: b as usize - 1,
            reason,
            at_m: place(model, a, b),
        })
        .collect())
}

/// One boolean intersection, as the CAD helper reports it (`faris-overlap-booleans/v1`).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct BooleanResult {
    pub a: usize,
    pub b: usize,
    pub status: String,
    #[serde(default)]
    pub common_volume_m3: Option<f64>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub seconds: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct PairRecord {
    pub a: String,
    pub b: String,
    pub candidate_because: Reason,
    /// `overlap`, `touching` or `undecided`.
    pub verdict: String,
    pub common_volume_m3: Option<f64>,
    pub smaller_solid_volume_m3: f64,
    pub reason: Option<String>,
}

/// Turn the boolean results into pair records and findings. `volumes_m3[i]` and
/// `labels[i]` belong to STEP solid `i`.
pub fn boolean_findings(
    candidates: &[Candidate],
    results: &[BooleanResult],
    volumes_m3: &[f64],
    labels: &[String],
) -> (Vec<PairRecord>, Vec<Finding>) {
    let mut records = Vec::new();
    let mut findings = Vec::new();
    for c in candidates {
        let result = results.iter().find(|r| r.a == c.a && r.b == c.b);
        let (la, lb) = (labels[c.a].clone(), labels[c.b].clone());
        let smaller = volumes_m3[c.a].min(volumes_m3[c.b]);
        let (verdict, common, reason) = match result {
            Some(r) if r.status == "decided" => {
                let common = r.common_volume_m3.unwrap_or(0.0);
                if common > OVERLAP_FRACTION * smaller {
                    ("overlap", Some(common), None)
                } else {
                    ("touching", Some(common), None)
                }
            }
            Some(r) => ("undecided", None, r.reason.clone()),
            None => (
                "undecided",
                None,
                Some("the CAD helper returned no answer for this pair".to_string()),
            ),
        };
        let finding = |item: String, why: String, next: &str| {
            Finding::new(CHECK, item, why, next)
                .requirement("GEO-021")
                .cells(vec![la.clone(), lb.clone()])
                .at(c.at_m)
                .waivable()
        };
        match verdict {
            "overlap" => findings.push(finding(
                format!("{la} and {lb}: overlap in the CAD"),
                format!(
                    "OpenCASCADE's exact intersection of the two solids has a volume of {:.6e} m3, \
                     {:.2e} of the smaller solid ({smaller:.6e} m3); they occupy the same space, \
                     so imprinting would split or reject them",
                    common.unwrap_or(0.0),
                    common.unwrap_or(0.0) / smaller
                ),
                "Remove the overlap in the CAD tool (cut one solid with the other), then export again.",
            )),
            "undecided" => findings.push(finding(
                format!("{la} and {lb}: overlap undecided"),
                format!(
                    "the exact boolean intersection of these neighbouring solids gave no answer: {}",
                    reason.clone().unwrap_or_default()
                ),
                "Simplify or repair the two solids in the CAD tool, or accept the pair with a waiver once you have checked it by eye.",
            )),
            _ => {}
        }
        records.push(PairRecord {
            a: la,
            b: lb,
            candidate_because: c.reason,
            verdict: verdict.to_string(),
            common_volume_m3: common,
            smaller_solid_volume_m3: smaller,
            reason,
        });
    }
    (records, findings)
}

#[cfg(test)]
mod tests {
    use super::super::mesh::fixtures::*;
    use super::*;
    use crate::jobs::Cancellation;
    use faris_model::design::Facets;

    fn pairs(f: &Facets, chord_m: f64) -> Vec<Candidate> {
        let model = Model::new(f).unwrap();
        let cancel = Cancellation::default();
        let progress = |_: &str, _: usize, _: usize| {};
        candidate_pairs(&model, chord_m, 3, &Ctl::new(&cancel, &progress, 2)).unwrap()
    }

    fn keys(c: &[Candidate]) -> Vec<(usize, usize, Reason)> {
        c.iter().map(|c| (c.a, c.b, c.reason)).collect()
    }

    // Verifies: GEO-021
    #[test]
    fn distant_solids_are_not_candidates() {
        let f = separate_boxes(&[([0.0; 3], [1.0; 3]), ([1.5, 0.0, 0.0], [2.5, 1.0, 1.0])]);
        assert!(pairs(&f, 0.1).is_empty());
    }

    // Verifies: GEO-021
    #[test]
    fn solids_within_twice_the_tolerance_are_candidates_and_beyond_are_not() {
        let near = separate_boxes(&[([0.0; 3], [1.0; 3]), ([1.15, 0.0, 0.0], [2.0, 1.0, 1.0])]);
        assert_eq!(keys(&pairs(&near, 0.1)), vec![(0, 1, Reason::Near)]);
        let far = separate_boxes(&[([0.0; 3], [1.0; 3]), ([1.25, 0.0, 0.0], [2.0, 1.0, 1.0])]);
        assert!(pairs(&far, 0.1).is_empty());
    }

    // Verifies: GEO-021
    #[test]
    fn crossing_coincident_and_contained_solids_are_candidates() {
        let crossing = separate_boxes(&[([0.0; 3], [2.0; 3]), ([0.7, 0.5, 0.3], [2.9, 2.6, 2.2])]);
        assert_eq!(
            keys(&pairs(&crossing, 0.01)),
            vec![(0, 1, Reason::Crossing)]
        );
        let touching = separate_boxes(&[([0.0; 3], [1.0; 3]), ([1.0, 0.0, 0.0], [2.0, 1.0, 1.0])]);
        assert_eq!(
            keys(&pairs(&touching, 0.01)),
            vec![(0, 1, Reason::Coincident)]
        );
        let inside = separate_boxes(&[([0.0; 3], [10.0; 3]), ([4.0; 3], [5.0; 3])]);
        assert_eq!(keys(&pairs(&inside, 0.01)), vec![(0, 1, Reason::Contained)]);
    }

    #[test]
    fn nested_layers_leave_only_neighbours() {
        // three nested boxes with gaps wider than the near threshold: containment pairs only
        let f = separate_boxes(&[
            ([-3.0; 3], [3.0; 3]),
            ([-2.0; 3], [2.0; 3]),
            ([-1.0; 3], [1.0; 3]),
        ]);
        let k = keys(&pairs(&f, 0.01));
        assert!(k.contains(&(0, 1, Reason::Contained)) && k.contains(&(1, 2, Reason::Contained)));
    }

    #[test]
    fn distances_match_known_geometry() {
        let a = [[0, 0, 0], [10, 0, 0], [0, 10, 0]];
        let up = |h: i64| [[0, 0, h], [10, 0, h], [0, 10, h]];
        assert!((triangle_distance(a, up(7)) - 7.0).abs() < 1e-9);
        let skew = [[20, 0, 3], [30, 0, 3], [20, 10, 3]];
        assert!((triangle_distance(a, skew) - 109.0f64.sqrt()).abs() < 1e-9);
        // edge to edge, crossing in projection
        let b = [[5, -5, 4], [5, 5, 4], [6, 0, 9]];
        assert!((triangle_distance(a, b) - 4.0).abs() < 1e-9);
    }

    fn result(a: usize, b: usize, common: Option<f64>) -> BooleanResult {
        BooleanResult {
            a,
            b,
            status: if common.is_some() {
                "decided"
            } else {
                "undecided"
            }
            .into(),
            common_volume_m3: common,
            reason: common.is_none().then(|| "OpenCASCADE error".into()),
            seconds: None,
        }
    }

    // Verifies: GEO-021
    #[test]
    fn the_boolean_decides_overlap_touching_or_undecided() {
        let cand = |a, b| Candidate {
            a,
            b,
            reason: Reason::Near,
            at_m: [0.0; 3],
        };
        let candidates = [cand(0, 1), cand(1, 2), cand(0, 2)];
        let results = [
            result(0, 1, Some(1.0e-3)),
            result(1, 2, Some(1.0e-9)),
            result(0, 2, None),
        ];
        let volumes = [1.0, 1.0, 1.0];
        let labels: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        let (records, findings) = boolean_findings(&candidates, &results, &volumes, &labels);
        let verdicts: Vec<&str> = records.iter().map(|r| r.verdict.as_str()).collect();
        assert_eq!(verdicts, ["overlap", "touching", "undecided"]);
        assert_eq!(findings.len(), 2);
        assert!(
            findings
                .iter()
                .all(|f| f.waivable && f.location_m.is_some())
        );
        assert!(findings[1].why.contains("OpenCASCADE error"));
    }

    // Verifies: GEO-021
    #[test]
    fn an_overlap_just_above_the_threshold_is_found() {
        let c = [Candidate {
            a: 0,
            b: 1,
            reason: Reason::Near,
            at_m: [0.0; 3],
        }];
        let labels = vec!["a".to_string(), "b".to_string()];
        let above = boolean_findings(&c, &[result(0, 1, Some(1.1e-6))], &[1.0, 5.0], &labels);
        assert_eq!(above.1.len(), 1);
        let below = boolean_findings(&c, &[result(0, 1, Some(0.9e-6))], &[1.0, 5.0], &labels);
        assert!(below.1.is_empty());
    }
}
