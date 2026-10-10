//! A bounding-volume hierarchy over triangles, so each integrity check costs
//! about N log N instead of N squared. Boxes are integer (grid units), so box
//! overlap is exact; the segment test uses `f64` slabs inflated by one grid unit
//! and is only a filter: every triangle it passes is then decided exactly.

use super::exact::P3;
use super::mesh::Mesh;

const LEAF_SIZE: usize = 4;

#[derive(Clone, Copy, Debug)]
struct Node {
    lo: P3,
    hi: P3,
    /// Leaf: first index into `order`. Internal: index of the left child (the right child follows).
    first: u32,
    /// Leaf: number of triangles. Internal: 0.
    count: u32,
}

#[derive(Debug)]
pub struct Bvh {
    nodes: Vec<Node>,
    order: Vec<u32>,
    /// Box of every triangle of the mesh, by triangle id.
    boxes: Vec<(P3, P3)>,
}

fn tri_box(mesh: &Mesh, t: u32) -> (P3, P3) {
    let [a, b, c] = mesh.tri_pts(t);
    let mut lo = a;
    let mut hi = a;
    for p in [b, c] {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

impl Bvh {
    /// Build over the given triangles.
    pub fn build(mesh: &Mesh, triangles: Vec<u32>) -> Self {
        let mut order = triangles;
        let boxes: Vec<(P3, P3)> = (0..mesh.tris.len() as u32)
            .map(|t| tri_box(mesh, t))
            .collect();
        let mut nodes = Vec::with_capacity(order.len() / 2 + 1);
        if order.is_empty() {
            return Self {
                nodes,
                order,
                boxes,
            };
        }
        nodes.push(Node {
            lo: [0; 3],
            hi: [0; 3],
            first: 0,
            count: 0,
        });
        // (node index, range start, range end)
        let mut stack = vec![(0usize, 0usize, order.len())];
        while let Some((index, start, end)) = stack.pop() {
            let mut lo = [i64::MAX; 3];
            let mut hi = [i64::MIN; 3];
            for &t in &order[start..end] {
                let (l, h) = boxes[t as usize];
                for k in 0..3 {
                    lo[k] = lo[k].min(l[k]);
                    hi[k] = hi[k].max(h[k]);
                }
            }
            if end - start <= LEAF_SIZE {
                nodes[index] = Node {
                    lo,
                    hi,
                    first: start as u32,
                    count: (end - start) as u32,
                };
                continue;
            }
            // Split at the median of triangle box centres on the longest axis of the box.
            let axis = (0..3).max_by_key(|&k| hi[k] - lo[k]).expect("three axes");
            let mid = start + (end - start) / 2;
            order[start..end].select_nth_unstable_by_key(mid - start, |&t| {
                let (l, h) = boxes[t as usize];
                l[axis] + h[axis]
            });
            let left = nodes.len();
            nodes.push(Node {
                lo: [0; 3],
                hi: [0; 3],
                first: 0,
                count: 0,
            });
            nodes.push(Node {
                lo: [0; 3],
                hi: [0; 3],
                first: 0,
                count: 0,
            });
            nodes[index] = Node {
                lo,
                hi,
                first: left as u32,
                count: 0,
            };
            stack.push((left, start, mid));
            stack.push((left + 1, mid, end));
        }
        Self {
            nodes,
            order,
            boxes,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// Visit every triangle whose box overlaps `[lo, hi]` (inclusive, exact).
    pub fn query_box(&self, lo: P3, hi: P3, visit: &mut impl FnMut(u32)) {
        if self.nodes.is_empty() {
            return;
        }
        let mut stack = [0u32; 64];
        let mut top = 1;
        while top > 0 {
            top -= 1;
            let node = &self.nodes[stack[top] as usize];
            if (0..3).any(|k| node.hi[k] < lo[k] || node.lo[k] > hi[k]) {
                continue;
            }
            if node.count > 0 {
                for &t in &self.order[node.first as usize..(node.first + node.count) as usize] {
                    let (l, h) = self.boxes[t as usize];
                    if (0..3).all(|k| h[k] >= lo[k] && l[k] <= hi[k]) {
                        visit(t);
                    }
                }
            } else {
                stack[top] = node.first;
                stack[top + 1] = node.first + 1;
                top += 2;
            }
        }
    }

    /// Visit every triangle whose box the segment `p`-`q` may touch. A filter only.
    pub fn query_segment(&self, p: P3, q: P3, visit: &mut impl FnMut(u32)) {
        if self.nodes.is_empty() {
            return;
        }
        let pf = [p[0] as f64, p[1] as f64, p[2] as f64];
        let d = [
            (q[0] - p[0]) as f64,
            (q[1] - p[1]) as f64,
            (q[2] - p[2]) as f64,
        ];
        let mut stack = [0u32; 64];
        let mut top = 1;
        while top > 0 {
            top -= 1;
            let node = &self.nodes[stack[top] as usize];
            let mut t0 = 0.0f64;
            let mut t1 = 1.0f64;
            let mut hit = true;
            for k in 0..3 {
                let lo = node.lo[k] as f64 - 2.0;
                let hi = node.hi[k] as f64 + 2.0;
                if d[k] == 0.0 {
                    if pf[k] < lo || pf[k] > hi {
                        hit = false;
                        break;
                    }
                } else {
                    let (mut a, mut b) = ((lo - pf[k]) / d[k], (hi - pf[k]) / d[k]);
                    if a > b {
                        std::mem::swap(&mut a, &mut b);
                    }
                    t0 = t0.max(a);
                    t1 = t1.min(b);
                    if t0 > t1 + 1e-9 {
                        hit = false;
                        break;
                    }
                }
            }
            if !hit {
                continue;
            }
            if node.count > 0 {
                for &t in &self.order[node.first as usize..(node.first + node.count) as usize] {
                    visit(t);
                }
            } else {
                stack[top] = node.first;
                stack[top + 1] = node.first + 1;
                top += 2;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::mesh::fixtures::*;
    use super::*;

    fn mesh() -> Mesh {
        // a row of boxes, 1 m apart
        let boxes: Vec<_> = (0..40)
            .map(|i| ([i as f64 * 2.0, 0.0, 0.0], [i as f64 * 2.0 + 1.0, 1.0, 1.0]))
            .collect();
        Mesh::from_facets(&separate_boxes(&boxes)).unwrap()
    }

    #[test]
    fn box_queries_find_exactly_the_overlapping_triangles() {
        let m = mesh();
        let bvh = Bvh::build(&m, (0..m.tris.len() as u32).collect());
        for (lo, hi) in [
            ([0, 0, 0], [3_000_000_000, 1, 1]),
            ([-5, -5, -5], [-1, -1, -1]),
            (
                [39_000_000_000, 0, 0],
                [90_000_000_000, 1_000_000_000, 1_000_000_000],
            ),
            (
                [5_000_000_000, 400_000_000, 400_000_000],
                [5_000_000_001, 500_000_000, 500_000_000],
            ),
        ] {
            let mut got = Vec::new();
            bvh.query_box(lo, hi, &mut |t| got.push(t));
            got.sort();
            let want: Vec<u32> = (0..m.tris.len() as u32)
                .filter(|&t| {
                    let (l, h) = tri_box(&m, t);
                    (0..3).all(|k| h[k] >= lo[k] && l[k] <= hi[k])
                })
                .collect();
            assert_eq!(got, want, "{lo:?} {hi:?}");
        }
    }

    #[test]
    fn segment_queries_never_drop_a_triangle_the_segment_touches() {
        let m = mesh();
        let bvh = Bvh::build(&m, (0..m.tris.len() as u32).collect());
        // a segment along the row, through every box
        let p = [-1_000_000_000, 500_000_000, 500_000_000];
        let q = [90_000_000_000, 500_000_000, 500_000_000];
        let mut got = Vec::new();
        bvh.query_segment(p, q, &mut |t| got.push(t));
        // each box is crossed at its two x faces; every x-face triangle must be passed
        for t in 0..m.tris.len() as u32 {
            let tri = m.tri_pts(t);
            if super::super::exact::seg_tri(p, q, tri) != super::super::exact::SegTri::Miss {
                assert!(
                    got.contains(&t),
                    "triangle {t} met by the segment but not visited"
                );
            }
        }
        // and far fewer than everything is visited for a short segment
        let mut near = 0;
        bvh.query_segment(
            [0, 0, 0],
            [500_000_000, 500_000_000, 500_000_000],
            &mut |_| near += 1,
        );
        assert!(near < 40, "{near}");
    }

    #[test]
    fn an_empty_hierarchy_visits_nothing() {
        let m = mesh();
        let bvh = Bvh::build(&m, Vec::new());
        assert!(bvh.is_empty());
        bvh.query_box([0; 3], [1; 3], &mut |_| panic!());
        bvh.query_segment([0; 3], [1; 3], &mut |_| panic!());
    }
}
