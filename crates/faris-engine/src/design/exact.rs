//! Exact geometric predicates on the 1 nm integer grid.
//!
//! Every yes/no decision about the faceted model (which side of a plane, does
//! this segment cross that triangle, do these triangles touch) is made here in
//! integer arithmetic, so the answer cannot depend on rounding. Vertices are
//! snapped to a grid of 1 nm (the snap moves a vertex by at most 0.5 nm per
//! axis); the predicates then take `i64` points and evaluate in `i128`.
//!
//! # The bit bound
//!
//! The model's bounding box may be at most [`MAX_EXTENT`] = 2^38 nm (about
//! 275 m) on every axis. Every point a predicate sees (model vertices, query
//! points, and the far ends of rays and chords) lies within 4 extents of the
//! box, so any difference of two coordinates satisfies |d| < D = 2^41.
//!
//! * A 2x2 minor of differences is at most 2 D^2 = 2^83.
//! * A 3x3 determinant of differences is a sum of three products of a
//!   coordinate difference and a minor, at most 3 * D * 2 D^2 = 6 D^3
//!   < 2^126, inside `i128` (2^127).
//! * A triangle normal has components at most 2 D^2 = 2^83; choosing the
//!   dominant axis compares two of them and projects to 2D, where orientation
//!   is a 2x2 determinant, at most 2 D^2.
//!
//! No predicate multiplies a determinant by anything, so these are the only
//! magnitudes. A cheap `f64` filter (Shewchuk's error bound, valid because the
//! differences are exact integers below 2^53) decides most signs first, and
//! the `i128` evaluation decides the rest; the tests check both against a
//! big-integer evaluation.

/// A point on the 1 nm grid.
pub type P3 = [i64; 3];

/// Largest bounding-box side the exact predicates accept, in grid units (nm).
pub const MAX_EXTENT: i64 = 1 << 38;

/// Largest coordinate difference any predicate may see, 2^41.
#[cfg(test)]
pub const MAX_DIFFERENCE: i64 = 1 << 41;

#[inline]
fn sub(a: P3, b: P3) -> [i64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// The determinant of the rows `u`, `v`, `w`, exactly (see the module's bit bound).
#[inline]
pub fn det3(u: [i64; 3], v: [i64; 3], w: [i64; 3]) -> i128 {
    let m0 = i128::from(v[1]) * i128::from(w[2]) - i128::from(v[2]) * i128::from(w[1]);
    let m1 = i128::from(v[2]) * i128::from(w[0]) - i128::from(v[0]) * i128::from(w[2]);
    let m2 = i128::from(v[0]) * i128::from(w[1]) - i128::from(v[1]) * i128::from(w[0]);
    i128::from(u[0]) * m0 + i128::from(u[1]) * m1 + i128::from(u[2]) * m2
}

#[inline]
fn signum(x: i128) -> i32 {
    match x.cmp(&0) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// Sign of `det3(u, v, w)`, filtered in `f64` first.
#[inline]
pub fn det3_sign(u: [i64; 3], v: [i64; 3], w: [i64; 3]) -> i32 {
    let (u0, u1, u2) = (u[0] as f64, u[1] as f64, u[2] as f64);
    let (v0, v1, v2) = (v[0] as f64, v[1] as f64, v[2] as f64);
    let (w0, w1, w2) = (w[0] as f64, w[1] as f64, w[2] as f64);
    let (a, b) = (v1 * w2, v2 * w1);
    let (c, d) = (v2 * w0, v0 * w2);
    let (e, f) = (v0 * w1, v1 * w0);
    let det = u0 * (a - b) + u1 * (c - d) + u2 * (e - f);
    let permanent = u0.abs() * (a.abs() + b.abs())
        + u1.abs() * (c.abs() + d.abs())
        + u2.abs() * (e.abs() + f.abs());
    // Shewchuk's o3derrboundA: (7 + 56 eps) eps with eps = 2^-53.
    let bound = 7.771_561_172_376_103e-16 * permanent;
    if det > bound {
        1
    } else if -det > bound {
        -1
    } else {
        signum(det3(u, v, w))
    }
}

/// Which side of the plane through `a`, `b`, `c` the point `p` is on: +1 on the
/// side the right-hand normal `(b - a) x (c - a)` points to, -1 on the other, 0 on the plane.
#[inline]
pub fn side(a: P3, b: P3, c: P3, p: P3) -> i32 {
    det3_sign(sub(b, a), sub(c, a), sub(p, a))
}

/// The triangle normal `(b - a) x (c - a)`, exactly (components below 2^83).
#[inline]
pub fn normal(a: P3, b: P3, c: P3) -> [i128; 3] {
    let u = sub(b, a);
    let v = sub(c, a);
    [
        i128::from(u[1]) * i128::from(v[2]) - i128::from(u[2]) * i128::from(v[1]),
        i128::from(u[2]) * i128::from(v[0]) - i128::from(u[0]) * i128::from(v[2]),
        i128::from(u[0]) * i128::from(v[1]) - i128::from(u[1]) * i128::from(v[0]),
    ]
}

/// The axis along which the normal is largest; `None` for a triangle with no area.
pub fn dominant_axis(a: P3, b: P3, c: P3) -> Option<usize> {
    let n = normal(a, b, c);
    let (k, best) = (0..3)
        .map(|i| (i, n[i].abs()))
        .max_by_key(|&(_, v)| v)
        .expect("three axes");
    (best != 0).then_some(k)
}

/// Orientation of `a`, `b`, `c` after dropping axis `k`: the sign of the k-th
/// component of `(b - a) x (c - a)`.
#[inline]
pub fn orient2(k: usize, a: P3, b: P3, c: P3) -> i32 {
    let (i, j) = ((k + 1) % 3, (k + 2) % 3);
    let (u, v) = (b[i] - a[i], b[j] - a[j]);
    let (x, y) = (c[i] - a[i], c[j] - a[j]);
    signum(i128::from(u) * i128::from(y) - i128::from(v) * i128::from(x))
}

/// Is `x` inside the closed triangle, given that it lies in the triangle's plane?
fn in_triangle_2d(k: usize, x: P3, t: [P3; 3]) -> bool {
    let o = [
        orient2(k, t[0], t[1], x),
        orient2(k, t[1], t[2], x),
        orient2(k, t[2], t[0], x),
    ];
    !(o.contains(&1) && o.contains(&-1))
}

/// Is `p` on the closed triangle `t` (which must have area)?
pub fn point_on_triangle(p: P3, t: [P3; 3]) -> bool {
    side(t[0], t[1], t[2], p) == 0
        && dominant_axis(t[0], t[1], t[2]).is_some_and(|k| in_triangle_2d(k, p, t))
}

fn on_segment_2d(k: usize, p: P3, q: P3, x: P3) -> bool {
    let (i, j) = ((k + 1) % 3, (k + 2) % 3);
    x[i] >= p[i].min(q[i])
        && x[i] <= p[i].max(q[i])
        && x[j] >= p[j].min(q[j])
        && x[j] <= p[j].max(q[j])
}

/// Closed segment-segment intersection of two segments in one plane (axis `k` dropped).
fn segments_meet_2d(k: usize, p: P3, q: P3, u: P3, v: P3) -> bool {
    let d1 = orient2(k, p, q, u);
    let d2 = orient2(k, p, q, v);
    let d3 = orient2(k, u, v, p);
    let d4 = orient2(k, u, v, q);
    if d1 * d2 < 0 && d3 * d4 < 0 {
        return true;
    }
    (d1 == 0 && on_segment_2d(k, p, q, u))
        || (d2 == 0 && on_segment_2d(k, p, q, v))
        || (d3 == 0 && on_segment_2d(k, u, v, p))
        || (d4 == 0 && on_segment_2d(k, u, v, q))
}

/// How a segment meets a closed triangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegTri {
    /// They share no point.
    Miss,
    /// The segment's endpoints are strictly on opposite sides of the triangle's plane
    /// and it passes through the open interior of the triangle: one clean crossing.
    Cross,
    /// They share a point that is not a clean crossing: an endpoint on the triangle,
    /// a pass through an edge or vertex, or a segment lying in the triangle's plane.
    Touch,
}

fn coplanar_seg_tri(p: P3, q: P3, t: [P3; 3]) -> SegTri {
    let Some(k) = dominant_axis(t[0], t[1], t[2]) else {
        // A triangle with no area has no plane; the callers keep these out, and
        // if one gets here the answer that blocks more is the safe one.
        return SegTri::Touch;
    };
    if in_triangle_2d(k, p, t) || in_triangle_2d(k, q, t) {
        return SegTri::Touch;
    }
    for (u, v) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
        if segments_meet_2d(k, p, q, u, v) {
            return SegTri::Touch;
        }
    }
    SegTri::Miss
}

/// Classify the closed segment `p`-`q` against the closed triangle `t`.
pub fn seg_tri(p: P3, q: P3, t: [P3; 3]) -> SegTri {
    let sp = side(t[0], t[1], t[2], p);
    let sq = side(t[0], t[1], t[2], q);
    if sp * sq > 0 {
        return SegTri::Miss;
    }
    if sp == 0 && sq == 0 {
        return coplanar_seg_tri(p, q, t);
    }
    // The line p-q crosses the plane at one point. That point is in the closed
    // triangle exactly when the three edge orientations are not strictly mixed.
    let e = [
        side(p, q, t[0], t[1]),
        side(p, q, t[1], t[2]),
        side(p, q, t[2], t[0]),
    ];
    if e.contains(&1) && e.contains(&-1) {
        return SegTri::Miss;
    }
    if sp != 0 && sq != 0 && !e.contains(&0) {
        SegTri::Cross
    } else {
        SegTri::Touch
    }
}

/// Does the segment from the triangle's vertex `v` to `a` run along the triangle
/// `(v, x, y)` beyond `v`? True when `a` is in the triangle's plane and the
/// direction `a - v` lies in the closed cone spanned by `x - v` and `y - v`.
fn runs_along_from_vertex(v: P3, a: P3, x: P3, y: P3) -> bool {
    if side(v, x, y, a) != 0 {
        return false;
    }
    let Some(k) = dominant_axis(v, x, y) else {
        return true;
    };
    let sigma = orient2(k, v, x, y);
    orient2(k, v, x, a) * sigma >= 0 && orient2(k, v, a, y) * sigma >= 0
}

/// How two triangles meet beyond the features they legitimately share.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relation {
    /// They meet in nothing, or only in the shared vertex or shared edge.
    Clean,
    /// They meet somewhere else too.
    Intersect {
        /// All six vertices lie in one plane (coincident or folded surfaces).
        coplanar: bool,
        /// An edge of one passes cleanly through the open interior of the other.
        proper: bool,
    },
}

fn edges(t: [P3; 3]) -> [(P3, P3); 3] {
    [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])]
}

fn plane_separates(t1: [P3; 3], t2: [P3; 3]) -> bool {
    let s = [
        side(t1[0], t1[1], t1[2], t2[0]),
        side(t1[0], t1[1], t1[2], t2[1]),
        side(t1[0], t1[1], t1[2], t2[2]),
    ];
    (s[0] > 0 && s[1] > 0 && s[2] > 0) || (s[0] < 0 && s[1] < 0 && s[2] < 0)
}

fn is_coplanar(t1: [P3; 3], t2: [P3; 3]) -> bool {
    t2.iter().all(|&p| side(t1[0], t1[1], t1[2], p) == 0)
}

fn any_proper(t1: [P3; 3], t2: [P3; 3]) -> bool {
    edges(t1)
        .iter()
        .any(|&(p, q)| seg_tri(p, q, t2) == SegTri::Cross)
        || edges(t2)
            .iter()
            .any(|&(p, q)| seg_tri(p, q, t1) == SegTri::Cross)
}

/// Relate two triangles that share the vertices listed in `shared` (pairs of
/// positions `(i, j)`: vertex `i` of `t1` is vertex `j` of `t2`, by mesh vertex
/// identity). The triangles must have area.
///
/// The two triangles meet legitimately in exactly the simplex of the shared
/// vertices: nothing, a vertex, an edge. Their intersection is convex, so it is
/// confined to that simplex exactly when every edge of either triangle meets the
/// other triangle only inside it.
pub fn relate(t1: [P3; 3], t2: [P3; 3], shared: &[(usize, usize)]) -> Relation {
    let intersect = |t1: [P3; 3], t2: [P3; 3]| Relation::Intersect {
        coplanar: is_coplanar(t1, t2),
        proper: any_proper(t1, t2),
    };
    match shared.len() {
        0 => {
            if plane_separates(t1, t2) || plane_separates(t2, t1) {
                return Relation::Clean;
            }
            let hit = edges(t1)
                .iter()
                .any(|&(p, q)| seg_tri(p, q, t2) != SegTri::Miss)
                || edges(t2)
                    .iter()
                    .any(|&(p, q)| seg_tri(p, q, t1) != SegTri::Miss);
            if hit {
                intersect(t1, t2)
            } else {
                Relation::Clean
            }
        }
        1 => {
            let (i, j) = shared[0];
            let v = t1[i];
            let (a, b) = (t1[(i + 1) % 3], t1[(i + 2) % 3]);
            let (c, d) = (t2[(j + 1) % 3], t2[(j + 2) % 3]);
            let bad = seg_tri(a, b, t2) != SegTri::Miss
                || seg_tri(c, d, t1) != SegTri::Miss
                || runs_along_from_vertex(v, a, c, d)
                || runs_along_from_vertex(v, b, c, d)
                || runs_along_from_vertex(v, c, a, b)
                || runs_along_from_vertex(v, d, a, b);
            if bad {
                intersect(t1, t2)
            } else {
                Relation::Clean
            }
        }
        2 => {
            let (i0, j0) = shared[0];
            let (i1, j1) = shared[1];
            let third1 = t1[3 - i0 - i1];
            let third2 = t2[3 - j0 - j1];
            let (e0, e1) = (t1[i0], t1[i1]);
            if side(e0, e1, third1, third2) != 0 {
                return Relation::Clean;
            }
            // Coplanar: legitimate only when the triangles lie on opposite sides of the shared edge.
            let k = dominant_axis(t1[0], t1[1], t1[2]).expect("triangles have area");
            if orient2(k, e0, e1, third1) * orient2(k, e0, e1, third2) < 0 {
                Relation::Clean
            } else {
                Relation::Intersect {
                    coplanar: true,
                    proper: false,
                }
            }
        }
        _ => Relation::Intersect {
            coplanar: true,
            proper: false,
        },
    }
}

#[cfg(test)]
mod tests;
