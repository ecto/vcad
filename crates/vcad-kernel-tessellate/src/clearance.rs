//! Mesh-to-mesh clearance measurement.
//!
//! Computes the minimum separation distance between two triangle meshes —
//! or a penetration depth when they intersect — using a triangle BVH per
//! mesh with branch-and-bound closest-pair traversal. This is the
//! tessellation-based backend for named clearance/clash assertions
//! (air gaps, press fits, screw-head clearances): exact BRep distance can
//! replace it later without changing the result contract.
//!
//! Semantics of the signed distance:
//! - `distance > 0` — the meshes are separated by that many mm.
//! - `distance == 0` — the meshes touch (or cross without any vertex of
//!   one landing strictly inside the other).
//! - `distance < 0` — the meshes intersect; the magnitude is the depth of
//!   the deepest vertex of either mesh inside the other (a tessellation
//!   estimate of penetration depth, which also catches full containment
//!   where the surfaces never cross).

use crate::TriangleMesh;

/// A triangle as three f64 corner points.
type Tri = [[f64; 3]; 3];

/// Result of a mesh-to-mesh clearance query.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClearanceResult {
    /// Signed distance in mm: minimum separation when non-negative, the
    /// negated deepest penetration when the meshes intersect.
    pub distance: f64,
    /// True when the meshes intersect (crossing surfaces or containment).
    pub intersecting: bool,
    /// Point on mesh A realizing the reported distance.
    pub point_a: [f64; 3],
    /// Point on mesh B realizing the reported distance.
    pub point_b: [f64; 3],
}

/// Minimum signed distance between two triangle meshes.
///
/// Returns `None` when either mesh has no triangles. Meshes are treated as
/// closed solids for the intersection/containment test (vertex parity ray
/// casts); open meshes still get a correct surface-to-surface distance but
/// an unreliable `intersecting` flag.
pub fn mesh_clearance(a: &TriangleMesh, b: &TriangleMesh) -> Option<ClearanceResult> {
    let bvh_a = TriBvh::build(a)?;
    let bvh_b = TriBvh::build(b)?;

    let mut best = Best {
        dist_sq: f64::INFINITY,
        point_a: [0.0; 3],
        point_b: [0.0; 3],
        crossing: false,
    };
    closest_pair(&bvh_a, 0, &bvh_b, 0, &mut best);

    let mut intersecting = best.crossing;
    let mut distance = best.dist_sq.sqrt();
    let mut point_a = best.point_a;
    let mut point_b = best.point_b;

    // Penetration pass: only possible when the root boxes overlap. Catches
    // both crossing surfaces (depth of the deepest penetrating vertex) and
    // full containment, which the surface-distance pass cannot see.
    let ra = &bvh_a.nodes[0];
    let rb = &bvh_b.nodes[0];
    if aabb_dist_sq(ra.min, ra.max, rb.min, rb.max) == 0.0 {
        let pen_ab = deepest_penetration(a, &bvh_b);
        let pen_ba = deepest_penetration(b, &bvh_a);
        let (depth, vertex, surface, vertex_is_a) = match (pen_ab, pen_ba) {
            (Some(x), Some(y)) => {
                if x.0 >= y.0 {
                    (x.0, x.1, x.2, true)
                } else {
                    (y.0, y.1, y.2, false)
                }
            }
            (Some(x), None) => (x.0, x.1, x.2, true),
            (None, Some(y)) => (y.0, y.1, y.2, false),
            (None, None) => (0.0, [0.0; 3], [0.0; 3], true),
        };
        if pen_ab.is_some() || pen_ba.is_some() {
            intersecting = true;
            if depth > 0.0 {
                distance = -depth;
                if vertex_is_a {
                    point_a = vertex;
                    point_b = surface;
                } else {
                    point_a = surface;
                    point_b = vertex;
                }
            } else {
                distance = 0.0;
            }
        } else if intersecting {
            // Surfaces cross but no vertex lands strictly inside the other
            // mesh (a thin pierce): report zero depth at the crossing point.
            distance = 0.0;
        }
    }

    Some(ClearanceResult {
        distance,
        intersecting,
        point_a,
        point_b,
    })
}

// ---------------------------------------------------------------------------
// Triangle BVH
// ---------------------------------------------------------------------------

const LEAF_SIZE: usize = 4;

struct BvhNode {
    min: [f64; 3],
    max: [f64; 3],
    /// Child node indices for internal nodes.
    left: u32,
    right: u32,
    /// Triangle range for leaves; `count == 0` marks an internal node.
    start: u32,
    count: u32,
}

impl BvhNode {
    fn is_leaf(&self) -> bool {
        self.count > 0
    }
}

pub(crate) struct TriBvh {
    tris: Vec<Tri>,
    nodes: Vec<BvhNode>,
}

impl TriBvh {
    /// Build a BVH over the mesh triangles. Returns `None` for empty meshes.
    pub(crate) fn build(mesh: &TriangleMesh) -> Option<TriBvh> {
        let num_tris = mesh.indices.len() / 3;
        if num_tris == 0 {
            return None;
        }
        let mut tris = Vec::with_capacity(num_tris);
        for t in 0..num_tris {
            let mut tri = [[0.0f64; 3]; 3];
            for (k, corner) in tri.iter_mut().enumerate() {
                let vi = mesh.indices[3 * t + k] as usize;
                for (ax, coord) in corner.iter_mut().enumerate() {
                    *coord = mesh.vertices[3 * vi + ax] as f64;
                }
            }
            tris.push(tri);
        }

        let mut order: Vec<u32> = (0..num_tris as u32).collect();
        let mut nodes = Vec::new();
        // Reserve slot 0 for the root so recursion can reference it by index.
        nodes.push(BvhNode {
            min: [0.0; 3],
            max: [0.0; 3],
            left: 0,
            right: 0,
            start: 0,
            count: 0,
        });
        build_node(&tris, &mut order, 0, num_tris, &mut nodes, 0);

        // Reorder triangles so each leaf references a contiguous range.
        let sorted: Vec<Tri> = order.iter().map(|&i| tris[i as usize]).collect();
        Some(TriBvh {
            tris: sorted,
            nodes,
        })
    }
}

/// Recursively build the node at `nodes[slot]` over `order[start..start+count]`.
fn build_node(
    tris: &[Tri],
    order: &mut [u32],
    start: usize,
    count: usize,
    nodes: &mut Vec<BvhNode>,
    slot: usize,
) {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for &i in &order[start..start + count] {
        for p in &tris[i as usize] {
            for ax in 0..3 {
                min[ax] = min[ax].min(p[ax]);
                max[ax] = max[ax].max(p[ax]);
            }
        }
    }

    if count <= LEAF_SIZE {
        nodes[slot] = BvhNode {
            min,
            max,
            left: 0,
            right: 0,
            start: start as u32,
            count: count as u32,
        };
        return;
    }

    // Median split on the axis with the largest centroid spread.
    let centroid = |i: u32| -> [f64; 3] {
        let t = &tris[i as usize];
        [
            (t[0][0] + t[1][0] + t[2][0]) / 3.0,
            (t[0][1] + t[1][1] + t[2][1]) / 3.0,
            (t[0][2] + t[1][2] + t[2][2]) / 3.0,
        ]
    };
    let mut cmin = [f64::INFINITY; 3];
    let mut cmax = [f64::NEG_INFINITY; 3];
    for &i in &order[start..start + count] {
        let c = centroid(i);
        for ax in 0..3 {
            cmin[ax] = cmin[ax].min(c[ax]);
            cmax[ax] = cmax[ax].max(c[ax]);
        }
    }
    let mut axis = 0;
    let mut spread = cmax[0] - cmin[0];
    for ax in 1..3 {
        if cmax[ax] - cmin[ax] > spread {
            spread = cmax[ax] - cmin[ax];
            axis = ax;
        }
    }

    let mid = count / 2;
    order[start..start + count].select_nth_unstable_by(mid, |&i, &j| {
        centroid(i)[axis]
            .partial_cmp(&centroid(j)[axis])
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let left = nodes.len();
    nodes.push(BvhNode {
        min: [0.0; 3],
        max: [0.0; 3],
        left: 0,
        right: 0,
        start: 0,
        count: 0,
    });
    let right = nodes.len();
    nodes.push(BvhNode {
        min: [0.0; 3],
        max: [0.0; 3],
        left: 0,
        right: 0,
        start: 0,
        count: 0,
    });
    build_node(tris, order, start, mid, nodes, left);
    build_node(tris, order, start + mid, count - mid, nodes, right);

    nodes[slot] = BvhNode {
        min,
        max,
        left: left as u32,
        right: right as u32,
        start: 0,
        count: 0,
    };
}

// ---------------------------------------------------------------------------
// Closest-pair traversal
// ---------------------------------------------------------------------------

struct Best {
    dist_sq: f64,
    point_a: [f64; 3],
    point_b: [f64; 3],
    crossing: bool,
}

fn aabb_dist_sq(min_a: [f64; 3], max_a: [f64; 3], min_b: [f64; 3], max_b: [f64; 3]) -> f64 {
    let mut d2 = 0.0;
    for ax in 0..3 {
        let gap = (min_a[ax] - max_b[ax]).max(min_b[ax] - max_a[ax]).max(0.0);
        d2 += gap * gap;
    }
    d2
}

fn closest_pair(a: &TriBvh, ia: u32, b: &TriBvh, ib: u32, best: &mut Best) {
    let na = &a.nodes[ia as usize];
    let nb = &b.nodes[ib as usize];
    if best.crossing || aabb_dist_sq(na.min, na.max, nb.min, nb.max) >= best.dist_sq {
        return;
    }

    if na.is_leaf() && nb.is_leaf() {
        for ta in &a.tris[na.start as usize..(na.start + na.count) as usize] {
            for tb in &b.tris[nb.start as usize..(nb.start + nb.count) as usize] {
                if let Some(hit) = tri_tri_crossing(ta, tb) {
                    best.dist_sq = 0.0;
                    best.point_a = hit;
                    best.point_b = hit;
                    best.crossing = true;
                    return;
                }
                let (d2, pa, pb) = tri_tri_closest(ta, tb);
                if d2 < best.dist_sq {
                    best.dist_sq = d2;
                    best.point_a = pa;
                    best.point_b = pb;
                }
            }
        }
        return;
    }

    // Descend the larger (or only non-leaf) node, nearer child first.
    let split_a = !na.is_leaf() && (nb.is_leaf() || node_extent(na) >= node_extent(nb));
    if split_a {
        let (l, r) = (na.left, na.right);
        let dl = child_dist_sq(a, l, nb);
        let dr = child_dist_sq(a, r, nb);
        if dl <= dr {
            closest_pair(a, l, b, ib, best);
            closest_pair(a, r, b, ib, best);
        } else {
            closest_pair(a, r, b, ib, best);
            closest_pair(a, l, b, ib, best);
        }
    } else {
        let (l, r) = (nb.left, nb.right);
        let dl = child_dist_sq(b, l, na);
        let dr = child_dist_sq(b, r, na);
        if dl <= dr {
            closest_pair(a, ia, b, l, best);
            closest_pair(a, ia, b, r, best);
        } else {
            closest_pair(a, ia, b, r, best);
            closest_pair(a, ia, b, l, best);
        }
    }
}

fn node_extent(n: &BvhNode) -> f64 {
    (n.max[0] - n.min[0]) + (n.max[1] - n.min[1]) + (n.max[2] - n.min[2])
}

fn child_dist_sq(tree: &TriBvh, child: u32, other: &BvhNode) -> f64 {
    let c = &tree.nodes[child as usize];
    aabb_dist_sq(c.min, c.max, other.min, other.max)
}

// ---------------------------------------------------------------------------
// Triangle-triangle distance
// ---------------------------------------------------------------------------

/// Closest points between two triangles that do not cross, as
/// `(dist_sq, point_on_t1, point_on_t2)`: the minimum over the 9 edge-edge
/// pairs and the 6 vertex-to-face projections.
fn tri_tri_closest(t1: &Tri, t2: &Tri) -> (f64, [f64; 3], [f64; 3]) {
    let mut best = (f64::INFINITY, [0.0; 3], [0.0; 3]);

    for i in 0..3 {
        let (p1, q1) = (t1[i], t1[(i + 1) % 3]);
        for j in 0..3 {
            let (p2, q2) = (t2[j], t2[(j + 1) % 3]);
            let (d2, ca, cb) = closest_pt_segment_segment(p1, q1, p2, q2);
            if d2 < best.0 {
                best = (d2, ca, cb);
            }
        }
    }
    for &v in t1 {
        let c = closest_point_on_triangle(v, t2[0], t2[1], t2[2]);
        let d2 = dist_sq(v, c);
        if d2 < best.0 {
            best = (d2, v, c);
        }
    }
    for &v in t2 {
        let c = closest_point_on_triangle(v, t1[0], t1[1], t1[2]);
        let d2 = dist_sq(v, c);
        if d2 < best.0 {
            best = (d2, c, v);
        }
    }
    best
}

/// If the triangles cross (an edge of one passes through the interior of the
/// other), return a point on the crossing. Coplanar overlap is not detected —
/// for closed solids in general position it is always accompanied by an
/// edge-interior crossing or caught by the vertex-containment pass.
fn tri_tri_crossing(t1: &Tri, t2: &Tri) -> Option<[f64; 3]> {
    for i in 0..3 {
        if let Some(p) = segment_triangle_hit(t1[i], t1[(i + 1) % 3], t2) {
            return Some(p);
        }
        if let Some(p) = segment_triangle_hit(t2[i], t2[(i + 1) % 3], t1) {
            return Some(p);
        }
    }
    None
}

/// Möller–Trumbore segment-triangle intersection, `t` clamped to the segment.
fn segment_triangle_hit(p: [f64; 3], q: [f64; 3], tri: &Tri) -> Option<[f64; 3]> {
    let dir = sub(q, p);
    let e1 = sub(tri[1], tri[0]);
    let e2 = sub(tri[2], tri[0]);
    let h = cross(dir, e2);
    let det = dot(e1, h);
    if det.abs() < 1e-14 {
        return None;
    }
    let f = 1.0 / det;
    let s = sub(p, tri[0]);
    let u = f * dot(s, h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let qv = cross(s, e1);
    let v = f * dot(dir, qv);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = f * dot(e2, qv);
    // Strict interior of the segment: endpoints touching the triangle count
    // as distance-zero contact, not a crossing.
    if t <= 1e-10 || t >= 1.0 - 1e-10 {
        return None;
    }
    Some(add(p, scale(dir, t)))
}

/// Closest points between segments `p1q1` and `p2q2` (Ericson, Real-Time
/// Collision Detection §5.1.9). Returns `(dist_sq, point_on_1, point_on_2)`.
fn closest_pt_segment_segment(
    p1: [f64; 3],
    q1: [f64; 3],
    p2: [f64; 3],
    q2: [f64; 3],
) -> (f64, [f64; 3], [f64; 3]) {
    let d1 = sub(q1, p1);
    let d2 = sub(q2, p2);
    let r = sub(p1, p2);
    let a = dot(d1, d1);
    let e = dot(d2, d2);
    let f = dot(d2, r);

    let (s, t);
    if a <= 1e-18 && e <= 1e-18 {
        (s, t) = (0.0, 0.0);
    } else if a <= 1e-18 {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = dot(d1, r);
        if e <= 1e-18 {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = dot(d1, d2);
            let denom = a * e - b * b;
            let s0 = if denom > 1e-18 {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let t0 = (b * s0 + f) / e;
            if t0 < 0.0 {
                t = 0.0;
                s = (-c / a).clamp(0.0, 1.0);
            } else if t0 > 1.0 {
                t = 1.0;
                s = ((b - c) / a).clamp(0.0, 1.0);
            } else {
                s = s0;
                t = t0;
            }
        }
    }

    let c1 = add(p1, scale(d1, s));
    let c2 = add(p2, scale(d2, t));
    (dist_sq(c1, c2), c1, c2)
}

/// Closest point on triangle `abc` to `p` (Ericson, Real-Time Collision
/// Detection §5.1.5).
fn closest_point_on_triangle(p: [f64; 3], a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> [f64; 3] {
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }

    let bp = sub(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }

    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return add(a, scale(ab, v));
    }

    let cp = sub(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }

    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return add(a, scale(ac, w));
    }

    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return add(b, scale(sub(c, b), w));
    }

    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    add(add(a, scale(ab, v)), scale(ac, w))
}

// ---------------------------------------------------------------------------
// Containment / penetration depth
// ---------------------------------------------------------------------------

/// Deepest vertex of `mesh` strictly inside the solid bounded by `other`,
/// as `(depth, vertex, closest_point_on_other_surface)`.
fn deepest_penetration(mesh: &TriangleMesh, other: &TriBvh) -> Option<(f64, [f64; 3], [f64; 3])> {
    let root = &other.nodes[0];
    let num_verts = mesh.vertices.len() / 3;
    let mut visited = vec![false; num_verts];
    let mut best: Option<(f64, [f64; 3], [f64; 3])> = None;
    for &vi in &mesh.indices {
        let vi = vi as usize;
        if visited[vi] {
            continue;
        }
        visited[vi] = true;
        let p = [
            mesh.vertices[3 * vi] as f64,
            mesh.vertices[3 * vi + 1] as f64,
            mesh.vertices[3 * vi + 2] as f64,
        ];
        // Only vertices inside the other mesh's AABB can be inside it.
        if p[0] < root.min[0]
            || p[0] > root.max[0]
            || p[1] < root.min[1]
            || p[1] > root.max[1]
            || p[2] < root.min[2]
            || p[2] > root.max[2]
        {
            continue;
        }
        if !point_in_mesh(p, other) {
            continue;
        }
        let (depth, surface) = point_mesh_closest(p, other);
        if best.is_none_or(|(d, _, _)| depth > d) {
            best = Some((depth, p, surface));
        }
    }
    best
}

/// Fixed skew ray directions for parity voting — mutually spread and away
/// from the coordinate axes so tessellated geometry rarely aligns with more
/// than one of them.
const PARITY_DIRS: [[f64; 3]; 6] = [
    [
        0.485_071_250_072_666,
        0.727_606_875_108_999,
        0.485_071_250_072_665,
    ],
    [-0.573_29, 0.616_71, 0.539_53],
    [0.259_11, -0.836_43, 0.483_27],
    [0.627_81, 0.330_92, -0.704_49],
    [-0.446_02, -0.529_11, 0.721_84],
    [0.716_93, -0.485_22, -0.500_31],
];

/// Is `p` inside the closed mesh?
///
/// Parity ray casting hardened for real-world meshes (boolean outputs carry
/// welded seams, duplicated faces, and near-degenerate slivers): each ray
/// dedupes crossings at equal `t` (duplicate faces), discards directions
/// that graze a triangle edge (where adjacent triangles double-count or
/// both miss), and the verdict is a majority vote across skew directions
/// so one unlucky ray through a seam cannot flip the classification.
pub(crate) fn point_in_mesh(p: [f64; 3], bvh: &TriBvh) -> bool {
    let mut inside_votes = 0usize;
    let mut outside_votes = 0usize;
    let mut grazing_fallback: Option<bool> = None;
    for dir in PARITY_DIRS {
        let (inside, suspect) = parity_ray(p, dir, bvh);
        if suspect {
            grazing_fallback.get_or_insert(inside);
            continue;
        }
        if inside {
            inside_votes += 1;
        } else {
            outside_votes += 1;
        }
        if inside_votes >= 2 {
            return true;
        }
        if outside_votes >= 2 {
            return false;
        }
    }
    match inside_votes.cmp(&outside_votes) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        // Every direction grazed (pathological): trust the first parity.
        std::cmp::Ordering::Equal => grazing_fallback.unwrap_or(false),
    }
}

/// One parity cast: `(odd_crossings, saw_a_grazing_hit)`.
fn parity_ray(p: [f64; 3], dir: [f64; 3], bvh: &TriBvh) -> (bool, bool) {
    let mut hits: Vec<f64> = Vec::new();
    let mut suspect = false;
    let mut stack = vec![0u32];
    while let Some(idx) = stack.pop() {
        let n = &bvh.nodes[idx as usize];
        if !ray_hits_aabb(p, dir, n.min, n.max) {
            continue;
        }
        if n.is_leaf() {
            for tri in &bvh.tris[n.start as usize..(n.start + n.count) as usize] {
                match ray_triangle_crossing(p, dir, tri) {
                    RayHit::Cross(t, _) => hits.push(t),
                    RayHit::Graze => suspect = true,
                    RayHit::Miss => {}
                }
            }
        } else {
            stack.push(n.left);
            stack.push(n.right);
        }
    }
    hits.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    hits.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    (hits.len() % 2 == 1, suspect)
}

/// Is `p` inside the mesh by **winding count** — the union-tolerant verdict.
///
/// Each crossing ahead of the point contributes `+1` when the ray enters
/// through a triangle's outward face and `-1` when it leaves; `p` is inside
/// when the sum is non-zero. Unlike parity this survives *overlapping
/// construction bodies* welded into one mesh — two shells that share a slab
/// give a point in the overlap a count of 2, still inside, where parity would
/// see two crossings and call it void — and it survives coincident duplicate
/// faces, whose contributions cancel instead of flipping the answer.
///
/// The sign convention is load-bearing and is pinned by tests: an inverted
/// sum reports *nothing* as inside, and a probe suite of void assertions then
/// passes vacuously. That is not hypothetical — it is the bug rana's first
/// shell check shipped with (`tools/support-check.py`, finding #11).
///
/// Like the parity cast, the verdict is a majority vote over skew directions
/// with edge-grazing rays discarded.
pub(crate) fn point_in_mesh_winding(p: [f64; 3], bvh: &TriBvh) -> bool {
    let mut inside_votes = 0usize;
    let mut outside_votes = 0usize;
    let mut grazing_fallback: Option<bool> = None;
    for dir in PARITY_DIRS {
        let (inside, suspect) = winding_ray(p, dir, bvh);
        if suspect {
            grazing_fallback.get_or_insert(inside);
            continue;
        }
        if inside {
            inside_votes += 1;
        } else {
            outside_votes += 1;
        }
        if inside_votes >= 2 {
            return true;
        }
        if outside_votes >= 2 {
            return false;
        }
    }
    match inside_votes.cmp(&outside_votes) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => grazing_fallback.unwrap_or(false),
    }
}

/// One winding cast: `(nonzero_winding, saw_a_grazing_hit)`.
fn winding_ray(p: [f64; 3], dir: [f64; 3], bvh: &TriBvh) -> (bool, bool) {
    let mut winding = 0i32;
    let mut suspect = false;
    let mut stack = vec![0u32];
    while let Some(idx) = stack.pop() {
        let n = &bvh.nodes[idx as usize];
        if !ray_hits_aabb(p, dir, n.min, n.max) {
            continue;
        }
        if n.is_leaf() {
            for tri in &bvh.tris[n.start as usize..(n.start + n.count) as usize] {
                match ray_triangle_crossing(p, dir, tri) {
                    RayHit::Cross(_, det) => winding += if det > 0.0 { 1 } else { -1 },
                    RayHit::Graze => suspect = true,
                    RayHit::Miss => {}
                }
            }
        } else {
            stack.push(n.left);
            stack.push(n.right);
        }
    }
    (winding != 0, suspect)
}

enum RayHit {
    Miss,
    /// Ray parameter of the hit, and the Möller–Trumbore determinant, whose
    /// sign says which face of the triangle was struck: positive when the ray
    /// enters through the front (outward-normal) side, negative on exit.
    Cross(f64, f64),
    Graze,
}

/// Möller–Trumbore with grazing detection: `Cross` only for hits decisively
/// interior to the triangle and ahead of the origin; anything within
/// `EDGE_EPS` (barycentric) of an edge or vertex — where adjacent triangles
/// can double-count or both miss — is `Graze`.
fn ray_triangle_crossing(orig: [f64; 3], dir: [f64; 3], tri: &Tri) -> RayHit {
    const EDGE_EPS: f64 = 1e-7;
    let e1 = sub(tri[1], tri[0]);
    let e2 = sub(tri[2], tri[0]);
    let h = cross(dir, e2);
    let det = dot(e1, h);
    if det.abs() < 1e-12 {
        // Parallel: a coplanar graze surfaces as edge hits on the neighbors.
        return RayHit::Miss;
    }
    let f = 1.0 / det;
    let s = sub(orig, tri[0]);
    let u = f * dot(s, h);
    if !(-EDGE_EPS..=1.0 + EDGE_EPS).contains(&u) {
        return RayHit::Miss;
    }
    let q = cross(s, e1);
    let v = f * dot(dir, q);
    if v < -EDGE_EPS || u + v > 1.0 + EDGE_EPS {
        return RayHit::Miss;
    }
    let t = f * dot(e2, q);
    if t <= 1e-9 {
        return RayHit::Miss;
    }
    if u < EDGE_EPS || v < EDGE_EPS || u + v > 1.0 - EDGE_EPS {
        return RayHit::Graze;
    }
    RayHit::Cross(t, det)
}

/// Slab test for a ray with `t ∈ (0, ∞)`.
fn ray_hits_aabb(orig: [f64; 3], dir: [f64; 3], min: [f64; 3], max: [f64; 3]) -> bool {
    let mut t_min = 0.0f64;
    let mut t_max = f64::INFINITY;
    for ax in 0..3 {
        if dir[ax].abs() < 1e-15 {
            if orig[ax] < min[ax] || orig[ax] > max[ax] {
                return false;
            }
            continue;
        }
        let inv = 1.0 / dir[ax];
        let (t0, t1) = ((min[ax] - orig[ax]) * inv, (max[ax] - orig[ax]) * inv);
        let (t0, t1) = if t0 <= t1 { (t0, t1) } else { (t1, t0) };
        t_min = t_min.max(t0);
        t_max = t_max.min(t1);
        if t_min > t_max {
            return false;
        }
    }
    true
}

/// Closest point on the mesh surface to `p`, as `(distance, point)`.
fn point_mesh_closest(p: [f64; 3], bvh: &TriBvh) -> (f64, [f64; 3]) {
    let mut best_sq = f64::INFINITY;
    let mut best_pt = [0.0; 3];
    fn recurse(p: [f64; 3], bvh: &TriBvh, idx: u32, best_sq: &mut f64, best_pt: &mut [f64; 3]) {
        let n = &bvh.nodes[idx as usize];
        if point_aabb_dist_sq(p, n.min, n.max) >= *best_sq {
            return;
        }
        if n.is_leaf() {
            for tri in &bvh.tris[n.start as usize..(n.start + n.count) as usize] {
                let c = closest_point_on_triangle(p, tri[0], tri[1], tri[2]);
                let d2 = dist_sq(p, c);
                if d2 < *best_sq {
                    *best_sq = d2;
                    *best_pt = c;
                }
            }
        } else {
            let dl = point_aabb_dist_sq(
                p,
                bvh.nodes[n.left as usize].min,
                bvh.nodes[n.left as usize].max,
            );
            let dr = point_aabb_dist_sq(
                p,
                bvh.nodes[n.right as usize].min,
                bvh.nodes[n.right as usize].max,
            );
            if dl <= dr {
                recurse(p, bvh, n.left, best_sq, best_pt);
                recurse(p, bvh, n.right, best_sq, best_pt);
            } else {
                recurse(p, bvh, n.right, best_sq, best_pt);
                recurse(p, bvh, n.left, best_sq, best_pt);
            }
        }
    }
    recurse(p, bvh, 0, &mut best_sq, &mut best_pt);
    (best_sq.sqrt(), best_pt)
}

fn point_aabb_dist_sq(p: [f64; 3], min: [f64; 3], max: [f64; 3]) -> f64 {
    let mut d2 = 0.0;
    for ax in 0..3 {
        let gap = (min[ax] - p[ax]).max(p[ax] - max[ax]).max(0.0);
        d2 += gap * gap;
    }
    d2
}

// ---------------------------------------------------------------------------
// Small vector helpers
// ---------------------------------------------------------------------------

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dist_sq(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = sub(a, b);
    dot(d, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Axis-aligned box as a 12-triangle mesh.
    fn box_mesh(min: [f64; 3], max: [f64; 3]) -> TriangleMesh {
        let corners = [
            [min[0], min[1], min[2]],
            [max[0], min[1], min[2]],
            [max[0], max[1], min[2]],
            [min[0], max[1], min[2]],
            [min[0], min[1], max[2]],
            [max[0], min[1], max[2]],
            [max[0], max[1], max[2]],
            [min[0], max[1], max[2]],
        ];
        let quads: [[u32; 4]; 6] = [
            [0, 3, 2, 1], // bottom
            [4, 5, 6, 7], // top
            [0, 1, 5, 4], // front
            [2, 3, 7, 6], // back
            [1, 2, 6, 5], // right
            [0, 4, 7, 3], // left
        ];
        let mut mesh = TriangleMesh::new();
        for c in &corners {
            mesh.vertices.extend(c.iter().map(|&v| v as f32));
        }
        for q in &quads {
            mesh.indices.extend([q[0], q[1], q[2], q[0], q[2], q[3]]);
        }
        mesh
    }

    #[test]
    fn separated_boxes_report_the_gap() {
        let a = box_mesh([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        let b = box_mesh([11.0, 0.0, 0.0], [20.0, 10.0, 10.0]);
        let r = mesh_clearance(&a, &b).unwrap();
        assert!(!r.intersecting);
        assert!((r.distance - 1.0).abs() < 1e-9, "distance = {}", r.distance);
        assert!((r.point_a[0] - 10.0).abs() < 1e-9);
        assert!((r.point_b[0] - 11.0).abs() < 1e-9);
    }

    #[test]
    fn diagonal_gap_uses_corner_distance() {
        let a = box_mesh([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        let b = box_mesh([13.0, 14.0, 0.0], [20.0, 20.0, 10.0]);
        let r = mesh_clearance(&a, &b).unwrap();
        assert!(!r.intersecting);
        let expected = (3.0f64 * 3.0 + 4.0 * 4.0).sqrt();
        assert!(
            (r.distance - expected).abs() < 1e-9,
            "distance = {}",
            r.distance
        );
    }

    #[test]
    fn touching_boxes_report_zero() {
        let a = box_mesh([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        let b = box_mesh([10.0, 0.0, 0.0], [20.0, 10.0, 10.0]);
        let r = mesh_clearance(&a, &b).unwrap();
        assert!(r.distance.abs() < 1e-9, "distance = {}", r.distance);
    }

    #[test]
    fn overlapping_boxes_report_penetration_depth() {
        let a = box_mesh([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        let b = box_mesh([8.0, 2.0, 2.0], [20.0, 8.0, 8.0]);
        let r = mesh_clearance(&a, &b).unwrap();
        assert!(r.intersecting);
        // B's deepest vertices inside A sit at x = 8, i.e. 2 mm from A's
        // x = 10 face.
        assert!((r.distance + 2.0).abs() < 1e-9, "distance = {}", r.distance);
    }

    #[test]
    fn contained_box_is_a_clash_without_surface_crossing() {
        let a = box_mesh([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        let b = box_mesh([4.0, 4.0, 4.0], [6.0, 6.0, 6.0]);
        let r = mesh_clearance(&a, &b).unwrap();
        assert!(r.intersecting);
        assert!((r.distance + 4.0).abs() < 1e-9, "distance = {}", r.distance);
    }

    #[test]
    fn empty_mesh_yields_none() {
        let a = box_mesh([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        assert!(mesh_clearance(&a, &TriangleMesh::new()).is_none());
        assert!(mesh_clearance(&TriangleMesh::new(), &a).is_none());
    }

    #[test]
    fn thin_pierce_without_interior_vertices_is_intersecting() {
        // A long thin sliver skewering a box: none of the sliver's vertices
        // are inside the box, and none of the box's vertices are inside the
        // sliver, but the surfaces cross.
        let a = box_mesh([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        let b = box_mesh([-5.0, 4.9, 4.9], [15.0, 5.1, 5.1]);
        let r = mesh_clearance(&a, &b).unwrap();
        assert!(r.intersecting);
        assert!(r.distance <= 0.0);
    }
}
