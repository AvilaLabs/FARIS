use super::*;
use num_bigint::BigInt;

// ---- an independent exact evaluation -----------------------------------------
//
// Big integers, a different expansion (Leibniz over permutations) and, for the
// segment against triangle, the parametric intersection point as a fraction with
// a common denominator. Nothing here shares code with the predicates.

fn big(x: i64) -> BigInt {
    BigInt::from(x)
}

fn bsub(a: P3, b: P3) -> [BigInt; 3] {
    [
        big(a[0]) - big(b[0]),
        big(a[1]) - big(b[1]),
        big(a[2]) - big(b[2]),
    ]
}

fn bdet(u: &[BigInt; 3], v: &[BigInt; 3], w: &[BigInt; 3]) -> BigInt {
    let rows = [u, v, w];
    let mut total = BigInt::from(0);
    for (perm, sign) in [
        ([0, 1, 2], 1),
        ([1, 2, 0], 1),
        ([2, 0, 1], 1),
        ([0, 2, 1], -1),
        ([2, 1, 0], -1),
        ([1, 0, 2], -1),
    ] {
        let term = &rows[0][perm[0]] * &rows[1][perm[1]] * &rows[2][perm[2]];
        total += term * BigInt::from(sign);
    }
    total
}

fn bsign(x: &BigInt) -> i32 {
    match x.sign() {
        num_bigint::Sign::Minus => -1,
        num_bigint::Sign::NoSign => 0,
        num_bigint::Sign::Plus => 1,
    }
}

fn ref_side(a: P3, b: P3, c: P3, p: P3) -> i32 {
    bsign(&bdet(&bsub(b, a), &bsub(c, a), &bsub(p, a)))
}

fn cross(u: &[BigInt; 3], v: &[BigInt; 3]) -> [BigInt; 3] {
    [
        &u[1] * &v[2] - &u[2] * &v[1],
        &u[2] * &v[0] - &u[0] * &v[2],
        &u[0] * &v[1] - &u[1] * &v[0],
    ]
}

fn dot(u: &[BigInt; 3], v: &[BigInt; 3]) -> BigInt {
    &u[0] * &v[0] + &u[1] * &v[1] + &u[2] * &v[2]
}

/// A fraction `num / den` with `den > 0`.
#[derive(Clone)]
struct Frac {
    num: BigInt,
    den: BigInt,
}

impl Frac {
    fn new(num: BigInt, den: BigInt) -> Self {
        if bsign(&den) < 0 {
            Self {
                num: -num,
                den: -den,
            }
        } else {
            Self { num, den }
        }
    }
    fn cmp(&self, other: &Self) -> i32 {
        bsign(&(&self.num * &other.den - &other.num * &self.den))
    }
}

/// Reference segment-triangle classification by exact fractions.
fn ref_seg_tri(p: P3, q: P3, t: [P3; 3]) -> SegTri {
    let n = cross(&bsub(t[1], t[0]), &bsub(t[2], t[0]));
    let dir = bsub(q, p);
    let numerator = dot(&n, &bsub(t[0], p)); // n . (a - p)
    let denominator = dot(&n, &dir); // n . (q - p)
    let inside = |x: &[Frac; 3]| -> (bool, bool) {
        // (in closed triangle, in open triangle), for a point given as fractions
        // sharing one denominator.
        let mut signs = Vec::new();
        for (u, v) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let edge = bsub(v, u);
            let rel = [
                &x[0].num - &x[0].den * big(u[0]),
                &x[1].num - &x[1].den * big(u[1]),
                &x[2].num - &x[2].den * big(u[2]),
            ];
            signs.push(bsign(&dot(&n, &cross(&edge, &rel))));
        }
        (signs.iter().all(|&s| s >= 0), signs.iter().all(|&s| s > 0))
    };
    if bsign(&denominator) == 0 {
        if bsign(&numerator) != 0 {
            return SegTri::Miss; // parallel, off the plane
        }
        // In the plane: clip the segment against the three half-planes (Cyrus-Beck).
        let mut lo = Frac::new(big(0), big(1));
        let mut hi = Frac::new(big(1), big(1));
        for (u, v) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let edge = bsub(v, u);
            let h = |x: P3| dot(&n, &cross(&edge, &bsub(x, u)));
            let (hp, hq) = (h(p), h(q));
            // h(t) = hp + t (hq - hp) >= 0
            let slope = &hq - &hp;
            match bsign(&slope) {
                0 => {
                    if bsign(&hp) < 0 {
                        return SegTri::Miss;
                    }
                }
                s => {
                    let root = Frac::new(-hp.clone(), slope.clone());
                    if s > 0 {
                        if root.cmp(&lo) > 0 {
                            lo = root;
                        }
                    } else if root.cmp(&hi) < 0 {
                        hi = root;
                    }
                }
            }
        }
        return if lo.cmp(&hi) <= 0 {
            SegTri::Touch
        } else {
            SegTri::Miss
        };
    }
    // t = numerator / denominator must lie in [0, 1].
    let t_param = Frac::new(numerator.clone(), denominator.clone());
    if t_param.cmp(&Frac::new(big(0), big(1))) < 0 || t_param.cmp(&Frac::new(big(1), big(1))) > 0 {
        return SegTri::Miss;
    }
    // x = p + t (q - p) = (p * den + num * dir) / den
    let d = if bsign(&denominator) < 0 {
        -denominator.clone()
    } else {
        denominator.clone()
    };
    let nm = if bsign(&denominator) < 0 {
        -numerator.clone()
    } else {
        numerator.clone()
    };
    let x = [
        Frac {
            num: big(p[0]) * &d + &nm * &dir[0],
            den: d.clone(),
        },
        Frac {
            num: big(p[1]) * &d + &nm * &dir[1],
            den: d.clone(),
        },
        Frac {
            num: big(p[2]) * &d + &nm * &dir[2],
            den: d.clone(),
        },
    ];
    let (closed, open) = inside(&x);
    if !closed {
        SegTri::Miss
    } else if open && bsign(&nm) > 0 && nm < d {
        SegTri::Cross
    } else {
        SegTri::Touch
    }
}

// ---- random inputs ---------------------------------------------------------------

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 11
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % (hi - lo + 1) as u64) as i64
    }
}

fn point(rng: &mut Lcg, span: i64) -> P3 {
    [
        rng.range(-span, span),
        rng.range(-span, span),
        rng.range(-span, span),
    ]
}

/// A point exactly on the plane of `a b c`: an integer combination of two edges, when it fits.
fn on_plane(rng: &mut Lcg, a: P3, b: P3, c: P3, span: i64) -> P3 {
    let (s, t) = (rng.range(-span, span), rng.range(-span, span));
    let u = sub(b, a);
    let v = sub(c, a);
    [
        a[0] + s * u[0] + t * v[0],
        a[1] + s * u[1] + t * v[1],
        a[2] + s * u[2] + t * v[2],
    ]
}

// Verifies: GEO-020
#[test]
fn side_matches_the_big_integer_determinant_on_random_and_degenerate_points() {
    let mut rng = Lcg(1);
    // tiny coordinates make coplanar and collinear cases common
    for _ in 0..20_000 {
        let span = [2, 3, 6, 40][rng.range(0, 3) as usize];
        let (a, b, c, p) = (
            point(&mut rng, span),
            point(&mut rng, span),
            point(&mut rng, span),
            point(&mut rng, span),
        );
        assert_eq!(
            side(a, b, c, p),
            ref_side(a, b, c, p),
            "{a:?} {b:?} {c:?} {p:?}"
        );
    }
    // points built exactly on a plane
    for _ in 0..5_000 {
        let (a, b, c) = (
            point(&mut rng, 50),
            point(&mut rng, 50),
            point(&mut rng, 50),
        );
        let p = on_plane(&mut rng, a, b, c, 3);
        assert_eq!(side(a, b, c, p), 0);
        assert_eq!(ref_side(a, b, c, p), 0);
    }
}

// Verifies: GEO-020
#[test]
fn side_is_exact_at_the_largest_coordinate_differences() {
    // Differences just under 2^41, where the f64 filter cannot decide and the
    // i128 evaluation must: nearly flat tetrahedra.
    let big_span = MAX_DIFFERENCE / 2 - 1;
    let mut rng = Lcg(7);
    for _ in 0..20_000 {
        let a = point(&mut rng, big_span);
        let b = point(&mut rng, big_span);
        let c = point(&mut rng, big_span);
        // d is on the plane of abc, then moved by the smallest grid step
        let mut p = on_plane(&mut rng, a, b, c, 1);
        for axis in 0..3 {
            if p[axis].abs() > big_span {
                p[axis] = a[axis];
            }
        }
        let q = {
            let mut q = p;
            q[(rng.next() % 3) as usize] += rng.range(-1, 1);
            q
        };
        assert_eq!(side(a, b, c, p), ref_side(a, b, c, p));
        assert_eq!(
            side(a, b, c, q),
            ref_side(a, b, c, q),
            "{a:?} {b:?} {c:?} {q:?}"
        );
        // The determinant itself, not just its sign, is exact in i128.
        let det = det3(sub(b, a), sub(c, a), sub(q, a));
        let reference = bdet(&bsub(b, a), &bsub(c, a), &bsub(q, a));
        assert_eq!(det.to_string(), reference.to_string());
    }
}

#[test]
fn the_extreme_determinant_fits_in_i128() {
    // All entries at the bound: 6 D^3 < 2^126.
    let d = MAX_DIFFERENCE - 1;
    let extreme = det3([d, -d, d], [-d, d, d], [d, d, -d]);
    assert_eq!(
        extreme.to_string(),
        bdet(
            &[big(d), big(-d), big(d)],
            &[big(-d), big(d), big(d)],
            &[big(d), big(d), big(-d)]
        )
        .to_string()
    );
    assert!(extreme.unsigned_abs() < 1u128 << 126);
}

// Verifies: GEO-021
#[test]
fn seg_tri_matches_the_exact_fraction_reference() {
    let mut rng = Lcg(3);
    let mut seen = [0usize; 3];
    for round in 0..60_000 {
        let span = [2, 3, 5, 12][round % 4];
        let t = [
            point(&mut rng, span),
            point(&mut rng, span),
            point(&mut rng, span),
        ];
        if dominant_axis(t[0], t[1], t[2]).is_none() {
            continue;
        }
        let p = point(&mut rng, span);
        let q = match round % 3 {
            0 => point(&mut rng, span),
            // a segment ending on the plane of the triangle
            1 => on_plane(&mut rng, t[0], t[1], t[2], 2),
            // a segment in the plane
            _ => on_plane(&mut rng, t[0], t[1], t[2], 3),
        };
        let p = if round % 5 == 0 {
            on_plane(&mut rng, t[0], t[1], t[2], 2)
        } else {
            p
        };
        let got = seg_tri(p, q, t);
        let want = ref_seg_tri(p, q, t);
        assert_eq!(got, want, "p {p:?} q {q:?} t {t:?}");
        seen[match got {
            SegTri::Miss => 0,
            SegTri::Cross => 1,
            SegTri::Touch => 2,
        }] += 1;
    }
    assert!(
        seen.iter().all(|&n| n > 500),
        "every outcome is exercised: {seen:?}"
    );
}

#[test]
fn seg_tri_basic_cases() {
    let t = [[0, 0, 0], [10, 0, 0], [0, 10, 0]];
    assert_eq!(seg_tri([2, 2, -5], [2, 2, 5], t), SegTri::Cross);
    assert_eq!(seg_tri([20, 20, -5], [20, 20, 5], t), SegTri::Miss);
    assert_eq!(
        seg_tri([2, 2, 0], [2, 2, 5], t),
        SegTri::Touch,
        "starts on the triangle"
    );
    assert_eq!(
        seg_tri([5, 0, -5], [5, 0, 5], t),
        SegTri::Touch,
        "through an edge"
    );
    assert_eq!(
        seg_tri([0, 0, -5], [0, 0, 5], t),
        SegTri::Touch,
        "through a vertex"
    );
    assert_eq!(
        seg_tri([-5, 2, 0], [20, 2, 0], t),
        SegTri::Touch,
        "in the plane, crossing"
    );
    assert_eq!(
        seg_tri([-5, 20, 0], [20, 20, 0], t),
        SegTri::Miss,
        "in the plane, beyond"
    );
    assert_eq!(
        seg_tri([2, 2, 1], [2, 2, 5], t),
        SegTri::Miss,
        "stops short of the plane"
    );
}

// ---- relate -------------------------------------------------------------------------

fn dedup_shared(t1: [P3; 3], t2: [P3; 3]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for i in 0..3 {
        for j in 0..3 {
            if t1[i] == t2[j] {
                out.push((i, j));
            }
        }
    }
    out
}

/// Reference: do the closed triangles meet anywhere outside the simplex of the shared
/// vertices? Each edge of either triangle is intersected with the other triangle by
/// exact fractions; the meeting is allowed only when it is empty, the shared edge
/// itself, or a single shared endpoint.
fn ref_relate(t1: [P3; 3], t2: [P3; 3], shared: &[(usize, usize)]) -> bool {
    let shared_pts: Vec<P3> = shared.iter().map(|&(i, _)| t1[i]).collect();
    let n_shared = shared_pts.len();
    if n_shared == 3 {
        return true;
    }
    let mut bad = false;
    for (a, b, other) in edges(t1)
        .iter()
        .map(|&(a, b)| (a, b, t2))
        .chain(edges(t2).iter().map(|&(a, b)| (a, b, t1)))
    {
        bad |= edge_meets_outside_shared(a, b, other, &shared_pts);
    }
    bad
}

/// Exact reference: does segment ab meet triangle `t` at a point that is not one of `shared`
/// and (for a shared edge) not on the segment between two shared points?
fn edge_meets_outside_shared(a: P3, b: P3, t: [P3; 3], shared: &[P3]) -> bool {
    if shared.contains(&a) && shared.contains(&b) {
        return false; // the shared edge lies in both triangles by construction
    }
    // Intersection of ab with t as a parameter interval [lo, hi] of fractions.
    let n = cross(&bsub(t[1], t[0]), &bsub(t[2], t[0]));
    let dir = bsub(b, a);
    let numerator = dot(&n, &bsub(t[0], a));
    let denominator = dot(&n, &dir);
    let zero = Frac::new(big(0), big(1));
    let one = Frac::new(big(1), big(1));
    let (lo, hi);
    if bsign(&denominator) == 0 {
        if bsign(&numerator) != 0 {
            return false;
        }
        let mut l = zero.clone();
        let mut h = one.clone();
        for (u, v) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let edge = bsub(v, u);
            let f = |x: P3| dot(&n, &cross(&edge, &bsub(x, u)));
            let (fa, fb) = (f(a), f(b));
            let slope = &fb - &fa;
            match bsign(&slope) {
                0 => {
                    if bsign(&fa) < 0 {
                        return false;
                    }
                }
                s => {
                    let root = Frac::new(-fa.clone(), slope.clone());
                    if s > 0 {
                        if root.cmp(&l) > 0 {
                            l = root;
                        }
                    } else if root.cmp(&h) < 0 {
                        h = root;
                    }
                }
            }
        }
        if l.cmp(&h) > 0 {
            return false;
        }
        lo = l;
        hi = h;
    } else {
        let tp = Frac::new(numerator, denominator);
        if tp.cmp(&zero) < 0 || tp.cmp(&one) > 0 {
            return false;
        }
        // is the point inside the closed triangle?
        let x: Vec<Frac> = (0..3)
            .map(|i| Frac {
                num: big(a[i]) * &tp.den + &tp.num * &dir[i],
                den: tp.den.clone(),
            })
            .collect();
        for (u, v) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let edge = bsub(v, u);
            let rel = [
                &x[0].num - &x[0].den * big(u[0]),
                &x[1].num - &x[1].den * big(u[1]),
                &x[2].num - &x[2].den * big(u[2]),
            ];
            if bsign(&dot(&n, &cross(&edge, &rel))) < 0 {
                return false;
            }
        }
        lo = tp.clone();
        hi = tp;
    }
    // The meeting is the parameter interval [lo, hi]. It is allowed only if it is a
    // single point equal to a shared endpoint of the segment (parameter 0 or 1 whose
    // endpoint is shared).
    let single = lo.cmp(&hi) == 0;
    let at_start = single && lo.cmp(&zero) == 0 && shared.contains(&a);
    let at_end = single && lo.cmp(&one) == 0 && shared.contains(&b);
    !(at_start || at_end)
}

// Verifies: GEO-021
#[test]
fn relate_matches_the_exact_reference_for_every_sharing_pattern() {
    let mut rng = Lcg(11);
    let mut counts = [[0usize; 2]; 4];
    for round in 0..48_000 {
        let span = [2, 3, 4, 8][round % 4];
        let t1 = [
            point(&mut rng, span),
            point(&mut rng, span),
            point(&mut rng, span),
        ];
        let mut t2 = [
            point(&mut rng, span),
            point(&mut rng, span),
            point(&mut rng, span),
        ];
        // force 0..3 shared vertices at random positions
        let k = (round / 4) % 4;
        let mut order = [0usize, 1, 2];
        for i in (1..3).rev() {
            order.swap(i, rng.range(0, i as i64) as usize);
        }
        for s in 0..k {
            t2[order[s]] = t1[(order[s] + (round % 3)) % 3];
        }
        if dominant_axis(t1[0], t1[1], t1[2]).is_none()
            || dominant_axis(t2[0], t2[1], t2[2]).is_none()
        {
            continue;
        }
        // the shared set must be by point identity, as the mesh would have it
        let shared = dedup_shared(t1, t2);
        if shared.len() != k {
            continue; // an accidental extra coincidence; skip
        }
        let got = !matches!(relate(t1, t2, &shared), Relation::Clean);
        let want = ref_relate(t1, t2, &shared);
        assert_eq!(got, want, "k {k} t1 {t1:?} t2 {t2:?}");
        counts[k][usize::from(got)] += 1;
    }
    for (k, c) in counts.iter().enumerate().take(3) {
        assert!(
            c[0] > 100 && c[1] > 100,
            "sharing {k}: clean {} intersecting {}",
            c[0],
            c[1]
        );
    }
    assert_eq!(counts[3][0], 0, "identical triangles are never clean");
}

// Verifies: GEO-021
#[test]
fn relate_known_configurations() {
    let a = [[0, 0, 0], [10, 0, 0], [0, 10, 0]];
    // sharing an edge, folded up out of the plane: clean
    let b = [[0, 0, 0], [10, 0, 0], [0, 0, 10]];
    assert_eq!(relate(a, b, &[(0, 0), (1, 1)]), Relation::Clean);
    // sharing an edge, coplanar on the same side: a fold-over
    let c = [[0, 0, 0], [10, 0, 0], [3, 5, 0]];
    assert!(matches!(
        relate(a, c, &[(0, 0), (1, 1)]),
        Relation::Intersect { coplanar: true, .. }
    ));
    // sharing an edge, coplanar on opposite sides: clean (a flat quad)
    let d = [[0, 0, 0], [10, 0, 0], [3, -5, 0]];
    assert_eq!(relate(a, d, &[(0, 0), (1, 1)]), Relation::Clean);
    // sharing a vertex only: clean when apart, intersecting when a far edge pierces
    let e = [[0, 0, 0], [0, 0, 10], [-5, 0, 3]];
    assert_eq!(relate(a, e, &[(0, 0)]), Relation::Clean);
    let piercing = [[0, 0, 0], [3, 3, 4], [3, 3, -4]];
    assert!(matches!(
        relate(a, piercing, &[(0, 0)]),
        Relation::Intersect { proper: true, .. }
    ));
    // no shared vertex: apart, crossing, touching
    let far = [[20, 20, 20], [30, 20, 20], [20, 30, 20]];
    assert_eq!(relate(a, far, &[]), Relation::Clean);
    let crossing = [[2, 2, -3], [2, 2, 3], [4, 4, 0]];
    assert!(matches!(
        relate(a, crossing, &[]),
        Relation::Intersect {
            proper: true,
            coplanar: false
        }
    ));
    let touching = [[2, 2, 0], [6, 6, 5], [7, 1, 5]];
    assert!(matches!(
        relate(a, touching, &[]),
        Relation::Intersect { proper: false, .. }
    ));
    // the same triangle twice
    assert!(matches!(
        relate(a, a, &[(0, 0), (1, 1), (2, 2)]),
        Relation::Intersect { coplanar: true, .. }
    ));
}
