//! 3D convex hull (quickhull) over a point cloud, emitted as a closed,
//! outward-wound triangle mesh.
//!
//! This is the mesh-backed evaluation of the `Hull` IR op: operand solids are
//! tessellated and their vertices hulled together with any loose points. The
//! result is exact for faceted inputs and as faithful as the tessellation for
//! curved ones.

use std::collections::HashSet;

use vcad_kernel_tessellate::TriangleMesh;

type P = [f64; 3];

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: P, b: P) -> P {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: P) -> f64 {
    dot(a, a).sqrt()
}

struct Face {
    v: [usize; 3],
    n: P,
    d: f64,
    outside: Vec<usize>,
    alive: bool,
}

impl Face {
    fn new(pts: &[P], v: [usize; 3]) -> Self {
        let n = cross(sub(pts[v[1]], pts[v[0]]), sub(pts[v[2]], pts[v[0]]));
        let len = norm(n);
        let n = if len > 0.0 {
            [n[0] / len, n[1] / len, n[2] / len]
        } else {
            n
        };
        Face {
            v,
            n,
            d: dot(n, pts[v[0]]),
            outside: Vec::new(),
            alive: true,
        }
    }
    fn dist(&self, p: P) -> f64 {
        dot(self.n, p) - self.d
    }
}

/// Convex hull of `points`. Returns `None` when the points do not span a
/// volume (fewer than 4 distinct points, or all coplanar).
pub fn convex_hull(points: &[P]) -> Option<TriangleMesh> {
    // Drop non-finite input and exact duplicates (tessellations share seam
    // vertices many times over).
    let mut seen = HashSet::new();
    let pts: Vec<P> = points
        .iter()
        .copied()
        .filter(|p| p.iter().all(|c| c.is_finite()))
        .filter(|p| seen.insert(p.map(f64::to_bits)))
        .collect();
    if pts.len() < 4 {
        return None;
    }

    let (mut lo, mut hi) = (pts[0], pts[0]);
    for p in &pts {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let scale = norm(sub(hi, lo)).max(f64::MIN_POSITIVE);
    let eps = scale * 1e-9;

    // Initial simplex: the extreme pair on the widest axis, the point
    // farthest from that line, then the point farthest from that plane.
    let axis = (0..3)
        .max_by(|&a, &b| (hi[a] - lo[a]).total_cmp(&(hi[b] - lo[b])))
        .unwrap();
    let i0 = (0..pts.len())
        .min_by(|&a, &b| pts[a][axis].total_cmp(&pts[b][axis]))
        .unwrap();
    let i1 = (0..pts.len())
        .max_by(|&a, &b| pts[a][axis].total_cmp(&pts[b][axis]))
        .unwrap();
    let dir = sub(pts[i1], pts[i0]);
    let line_dist = |i: usize| norm(cross(dir, sub(pts[i], pts[i0])));
    let i2 = (0..pts.len())
        .max_by(|&a, &b| line_dist(a).total_cmp(&line_dist(b)))
        .unwrap();
    if line_dist(i2) / norm(dir).max(f64::MIN_POSITIVE) <= eps {
        return None;
    }
    let base = Face::new(&pts, [i0, i1, i2]);
    let i3 = (0..pts.len())
        .max_by(|&a, &b| base.dist(pts[a]).abs().total_cmp(&base.dist(pts[b]).abs()))
        .unwrap();
    if base.dist(pts[i3]).abs() <= eps {
        return None;
    }

    // Wind the four faces outward: the apex sits on the negative side of
    // each face that excludes it.
    let mut faces: Vec<Face> = Vec::new();
    let tet = [i0, i1, i2, i3];
    for skip in 0..4 {
        let others: Vec<usize> = (0..4).filter(|&k| k != skip).map(|k| tet[k]).collect();
        let mut f = Face::new(&pts, [others[0], others[1], others[2]]);
        if f.dist(pts[tet[skip]]) > 0.0 {
            f = Face::new(&pts, [others[0], others[2], others[1]]);
        }
        faces.push(f);
    }

    let assign = |faces: &mut Vec<Face>, cands: &[usize], targets: &[usize]| {
        for &p in cands {
            let mut best: Option<(usize, f64)> = None;
            for &fi in targets {
                let d = faces[fi].dist(pts[p]);
                if d > eps && best.is_none_or(|(_, bd)| d > bd) {
                    best = Some((fi, d));
                }
            }
            if let Some((fi, _)) = best {
                faces[fi].outside.push(p);
            }
        }
    };
    let all: Vec<usize> = (0..pts.len()).filter(|i| !tet.contains(i)).collect();
    assign(&mut faces, &all, &[0, 1, 2, 3]);

    while let Some(fi) = faces.iter().position(|f| f.alive && !f.outside.is_empty()) {
        let apex = *faces[fi]
            .outside
            .iter()
            .max_by(|&&a, &&b| faces[fi].dist(pts[a]).total_cmp(&faces[fi].dist(pts[b])))
            .unwrap();
        let ap = pts[apex];

        let visible: Vec<usize> = (0..faces.len())
            .filter(|&i| faces[i].alive && faces[i].dist(ap) > eps)
            .collect();
        let edges: HashSet<(usize, usize)> = visible
            .iter()
            .flat_map(|&i| {
                let v = faces[i].v;
                [(v[0], v[1]), (v[1], v[2]), (v[2], v[0])]
            })
            .collect();
        let horizon: Vec<(usize, usize)> = edges
            .iter()
            .copied()
            .filter(|&(a, b)| !edges.contains(&(b, a)))
            .collect();

        let mut orphans = Vec::new();
        for &i in &visible {
            faces[i].alive = false;
            orphans.append(&mut faces[i].outside);
        }
        let first_new = faces.len();
        for (a, b) in horizon {
            faces.push(Face::new(&pts, [a, b, apex]));
        }
        let new_ids: Vec<usize> = (first_new..faces.len()).collect();
        orphans.retain(|&p| p != apex);
        assign(&mut faces, &orphans, &new_ids);
    }

    // Compact to the hull's own vertices.
    let mut remap = vec![u32::MAX; pts.len()];
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for f in faces.iter().filter(|f| f.alive) {
        for &v in &f.v {
            if remap[v] == u32::MAX {
                remap[v] = (vertices.len() / 3) as u32;
                vertices.extend(pts[v].iter().map(|&c| c as f32));
            }
            indices.push(remap[v]);
        }
    }

    // Area-weighted vertex normals.
    let mut normals = vec![0.0f32; vertices.len()];
    for tri in indices.chunks(3) {
        let p = |i: u32| {
            let i = i as usize * 3;
            [
                vertices[i] as f64,
                vertices[i + 1] as f64,
                vertices[i + 2] as f64,
            ]
        };
        let n = cross(sub(p(tri[1]), p(tri[0])), sub(p(tri[2]), p(tri[0])));
        for &i in tri {
            for k in 0..3 {
                normals[i as usize * 3 + k] += n[k] as f32;
            }
        }
    }
    for n in normals.chunks_mut(3) {
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 0.0 {
            n.iter_mut().for_each(|c| *c /= len);
        }
    }

    Some(TriangleMesh {
        vertices,
        indices,
        normals,
        face_kinds: Vec::new(),
        face_ids: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(m: &TriangleMesh) -> f64 {
        m.indices
            .chunks(3)
            .map(|t| {
                let p = |i: u32| {
                    let i = i as usize * 3;
                    [
                        m.vertices[i] as f64,
                        m.vertices[i + 1] as f64,
                        m.vertices[i + 2] as f64,
                    ]
                };
                dot(p(t[0]), cross(p(t[1]), p(t[2]))) / 6.0
            })
            .sum()
    }

    #[test]
    fn cube_corners_plus_interior_points() {
        let mut pts = Vec::new();
        for i in 0..8 {
            pts.push([
                (i & 1) as f64 * 10.0,
                ((i >> 1) & 1) as f64 * 10.0,
                ((i >> 2) & 1) as f64 * 10.0,
            ]);
        }
        pts.push([5.0, 5.0, 5.0]);
        pts.push([1.0, 2.0, 3.0]);
        pts.push([5.0, 5.0, 10.0]); // on a face: may survive as a seed vertex, harmlessly
        let m = convex_hull(&pts).unwrap();
        assert!((volume(&m) - 1000.0).abs() < 1e-3, "vol {}", volume(&m));
        // Interior points never become vertices.
        assert!(m.vertices.len() / 3 <= 9);
    }

    #[test]
    fn hull_is_closed_two_manifold() {
        // Points on a sphere — every one is a hull vertex.
        let mut pts = Vec::new();
        for i in 0..20 {
            for j in 1..10 {
                let (t, p) = (i as f64 * 0.314, j as f64 * 0.314);
                pts.push([p.sin() * t.cos(), p.sin() * t.sin(), p.cos()]);
            }
        }
        pts.push([0.0, 0.0, 1.0]);
        pts.push([0.0, 0.0, -1.0]);
        let m = convex_hull(&pts).unwrap();
        let mut edges = std::collections::HashMap::new();
        for t in m.indices.chunks(3) {
            for k in 0..3 {
                *edges.entry((t[k], t[(k + 1) % 3])).or_insert(0) += 1;
            }
        }
        for (&(a, b), &c) in &edges {
            assert_eq!(c, 1);
            assert_eq!(edges.get(&(b, a)), Some(&1), "unpaired edge");
        }
        assert!(volume(&m) > 3.0 && volume(&m) < 4.19);
    }

    #[test]
    fn degenerate_inputs_return_none() {
        assert!(convex_hull(&[[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]).is_none());
        let flat: Vec<P> = (0..10).map(|i| [i as f64, (i * i) as f64, 0.0]).collect();
        assert!(convex_hull(&flat).is_none());
    }
}
