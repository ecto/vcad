//! Isosurface extraction from the converged density field.
//!
//! Uses naive surface nets over cell-centered density samples: one vertex
//! per grid cell straddling the iso level, quads across sign-changing
//! sample edges. The result is watertight, and a few Taubin smoothing
//! passes give the characteristic organic topology-optimized look without
//! shrinking the part.

use std::collections::HashMap;

use crate::domain::Domain;
use vcad_kernel_tessellate::TriangleMesh;

/// Density value treated as the material boundary.
const ISO: f64 = 0.5;

/// Extract a triangle mesh from a per-element density field.
///
/// `densities` is indexed like `domain.active` (one value per element,
/// zero outside the structure). `smooth_pairs` is the number of Taubin
/// λ/μ smoothing pairs applied to the raw surface.
pub fn extract_mesh(domain: &Domain, densities: &[f64], smooth_pairs: usize) -> TriangleMesh {
    // Density sample at a voxel center, 0 outside the grid. Sample point
    // (i, j, k) sits at origin + (i + 0.5) · h.
    let sample = |i: i32, j: i32, k: i32| -> f64 {
        if i < 0
            || j < 0
            || k < 0
            || i >= domain.nx as i32
            || j >= domain.ny as i32
            || k >= domain.nz as i32
        {
            return 0.0;
        }
        densities[domain.eidx(i as usize, j as usize, k as usize)]
    };
    let solid = |i: i32, j: i32, k: i32| sample(i, j, k) >= ISO;
    let sample_pos = |i: i32, j: i32, k: i32| -> [f64; 3] {
        [
            domain.origin[0] + (i as f64 + 0.5) * domain.h,
            domain.origin[1] + (j as f64 + 0.5) * domain.h,
            domain.origin[2] + (k as f64 + 0.5) * domain.h,
        ]
    };

    // Pass 1: place one vertex in every cell whose corner samples straddle
    // the iso level. Cell (i, j, k) spans samples (i..i+1, j..j+1, k..k+1);
    // the padding layer of zero samples closes the surface at the domain
    // boundary.
    let mut cell_vertex: HashMap<(i32, i32, i32), u32> = HashMap::new();
    let mut positions: Vec<[f64; 3]> = Vec::new();

    const CORNERS: [[i32; 3]; 8] = [
        [0, 0, 0],
        [1, 0, 0],
        [1, 1, 0],
        [0, 1, 0],
        [0, 0, 1],
        [1, 0, 1],
        [1, 1, 1],
        [0, 1, 1],
    ];
    // Cell edges as corner-index pairs.
    const EDGES: [[usize; 2]; 12] = [
        [0, 1],
        [1, 2],
        [2, 3],
        [3, 0],
        [4, 5],
        [5, 6],
        [6, 7],
        [7, 4],
        [0, 4],
        [1, 5],
        [2, 6],
        [3, 7],
    ];

    for ci in -1..domain.nx as i32 {
        for cj in -1..domain.ny as i32 {
            for ck in -1..domain.nz as i32 {
                let vals: [f64; 8] = std::array::from_fn(|c| {
                    sample(ci + CORNERS[c][0], cj + CORNERS[c][1], ck + CORNERS[c][2])
                });
                let inside = vals.map(|v| v >= ISO);
                if inside.iter().all(|&s| s) || inside.iter().all(|&s| !s) {
                    continue;
                }
                // Vertex = centroid of the edge crossings.
                let mut acc = [0.0f64; 3];
                let mut count = 0.0;
                for e in &EDGES {
                    let (a, b) = (e[0], e[1]);
                    if inside[a] == inside[b] {
                        continue;
                    }
                    let t = (ISO - vals[a]) / (vals[b] - vals[a]);
                    let pa = sample_pos(ci + CORNERS[a][0], cj + CORNERS[a][1], ck + CORNERS[a][2]);
                    let pb = sample_pos(ci + CORNERS[b][0], cj + CORNERS[b][1], ck + CORNERS[b][2]);
                    for x in 0..3 {
                        acc[x] += pa[x] + t * (pb[x] - pa[x]);
                    }
                    count += 1.0;
                }
                let idx = positions.len() as u32;
                positions.push([acc[0] / count, acc[1] / count, acc[2] / count]);
                cell_vertex.insert((ci, cj, ck), idx);
            }
        }
    }

    // Pass 2: for every sample-grid edge with a sign change, connect the
    // four surrounding cell vertices into a quad, wound so normals point
    // from solid to void.
    let mut indices: Vec<u32> = Vec::new();
    // Ring around the edge, counterclockwise when viewed from +axis
    // (offsets applied to the two non-axis directions, cyclic order).
    const RING: [[i32; 2]; 4] = [[-1, -1], [0, -1], [0, 0], [-1, 0]];

    for axis in 0..3usize {
        let u = (axis + 1) % 3;
        let v = (axis + 2) % 3;
        // Sample index bounds per axis, including the padding layer.
        let n = [domain.nx as i32, domain.ny as i32, domain.nz as i32];
        let lo = [-1, -1, -1];
        let hi = [n[0], n[1], n[2]]; // inclusive
        let mut s = [0i32; 3];
        for a0 in lo[0]..=hi[0] {
            for a1 in lo[1]..=hi[1] {
                for a2 in lo[2]..=hi[2] {
                    s[0] = a0;
                    s[1] = a1;
                    s[2] = a2;
                    if s[axis] + 1 > hi[axis] {
                        continue;
                    }
                    let mut t = s;
                    t[axis] += 1;
                    let s_solid = solid(s[0], s[1], s[2]);
                    let t_solid = solid(t[0], t[1], t[2]);
                    if s_solid == t_solid {
                        continue;
                    }
                    // Quad corners: the four cells sharing this edge.
                    let mut quad = [0u32; 4];
                    let mut ok = true;
                    for (qi, ring) in RING.iter().enumerate() {
                        let mut c = s;
                        c[u] += ring[0];
                        c[v] += ring[1];
                        match cell_vertex.get(&(c[0], c[1], c[2])) {
                            Some(&idx) => quad[qi] = idx,
                            None => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if !ok {
                        // Should not happen: cells around a sign-changing
                        // edge always straddle the iso level.
                        continue;
                    }
                    let [q0, q1, q2, q3] = if s_solid {
                        quad // normal along +axis (solid below, void above)
                    } else {
                        [quad[3], quad[2], quad[1], quad[0]]
                    };
                    indices.extend_from_slice(&[q0, q1, q2, q0, q2, q3]);
                }
            }
        }
    }

    if smooth_pairs > 0 {
        taubin_smooth(&mut positions, &indices, smooth_pairs);
    }

    build_mesh(&positions, &indices)
}

/// Taubin λ/μ smoothing: alternating shrink/inflate Laplacian steps that
/// smooth without net volume loss.
fn taubin_smooth(positions: &mut [[f64; 3]], indices: &[u32], pairs: usize) {
    const LAMBDA: f64 = 0.5;
    const MU: f64 = -0.53;

    // Vertex adjacency from triangle edges.
    let nv = positions.len();
    let mut neighbors: Vec<Vec<u32>> = vec![Vec::new(); nv];
    for tri in indices.chunks_exact(3) {
        for k in 0..3 {
            let a = tri[k] as usize;
            let b = tri[(k + 1) % 3];
            neighbors[a].push(b);
        }
    }
    for adj in &mut neighbors {
        adj.sort_unstable();
        adj.dedup();
    }

    let mut scratch = positions.to_vec();
    for _ in 0..pairs {
        for &factor in &[LAMBDA, MU] {
            for (i, adj) in neighbors.iter().enumerate() {
                if adj.is_empty() {
                    scratch[i] = positions[i];
                    continue;
                }
                let mut avg = [0.0f64; 3];
                for &j in adj {
                    for x in 0..3 {
                        avg[x] += positions[j as usize][x];
                    }
                }
                let inv = 1.0 / adj.len() as f64;
                for x in 0..3 {
                    scratch[i][x] = positions[i][x] + factor * (avg[x] * inv - positions[i][x]);
                }
            }
            positions.copy_from_slice(&scratch);
        }
    }
}

/// Assemble a [`TriangleMesh`] with area-weighted vertex normals.
fn build_mesh(positions: &[[f64; 3]], indices: &[u32]) -> TriangleMesh {
    let mut normals = vec![[0.0f64; 3]; positions.len()];
    for tri in indices.chunks_exact(3) {
        let a = positions[tri[0] as usize];
        let b = positions[tri[1] as usize];
        let c = positions[tri[2] as usize];
        let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        // Cross product magnitude = 2 · area, so this is area weighting.
        let n = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        for &vi in tri {
            for x in 0..3 {
                normals[vi as usize][x] += n[x];
            }
        }
    }

    let mut mesh = TriangleMesh::new();
    mesh.vertices.reserve(positions.len() * 3);
    mesh.normals.reserve(positions.len() * 3);
    for (p, n) in positions.iter().zip(&normals) {
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        let inv = if len > 1e-30 { 1.0 / len } else { 0.0 };
        for &coord in p {
            mesh.vertices.push(coord as f32);
        }
        for &comp in n {
            mesh.normals.push((comp * inv) as f32);
        }
    }
    mesh.indices = indices.to_vec();
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Every undirected edge of a watertight mesh is shared by exactly two
    /// triangles, once in each direction.
    fn assert_watertight(mesh: &TriangleMesh) {
        let mut edges: HashMap<(u32, u32), i32> = HashMap::new();
        for tri in mesh.indices.chunks_exact(3) {
            for k in 0..3 {
                let a = tri[k];
                let b = tri[(k + 1) % 3];
                *edges.entry((a.min(b), a.max(b))).or_insert(0) += if a < b { 1 } else { -1 };
            }
        }
        for (edge, balance) in &edges {
            assert_eq!(
                *balance, 0,
                "edge {edge:?} is unbalanced — mesh not watertight"
            );
        }
    }

    #[test]
    fn solid_box_extracts_to_box() {
        let domain = Domain::from_bbox([0.0; 3], [8.0, 4.0, 2.0], 8);
        let densities = vec![1.0; domain.num_elements()];
        let mesh = extract_mesh(&domain, &densities, 0);
        assert!(mesh.num_triangles() > 0);
        assert_watertight(&mesh);

        // Surface should sit on the domain boundary (crossing halfway
        // between the outermost cell center and the zero padding sample).
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for v in mesh.vertices.chunks_exact(3) {
            for x in 0..3 {
                min[x] = min[x].min(v[x]);
                max[x] = max[x].max(v[x]);
            }
        }
        assert!((min[0] - 0.0).abs() < 1e-4 && (max[0] - 8.0).abs() < 1e-4);
        assert!((min[1] - 0.0).abs() < 1e-4 && (max[1] - 4.0).abs() < 1e-4);
        assert!((min[2] - 0.0).abs() < 1e-4 && (max[2] - 2.0).abs() < 1e-4);
    }

    #[test]
    fn empty_field_extracts_nothing() {
        let domain = Domain::from_bbox([0.0; 3], [4.0; 3], 4);
        let densities = vec![0.0; domain.num_elements()];
        let mesh = extract_mesh(&domain, &densities, 2);
        assert_eq!(mesh.num_triangles(), 0);
    }

    #[test]
    fn smoothing_preserves_watertightness_and_scale() {
        let domain = Domain::from_bbox([0.0; 3], [6.0; 3], 6);
        let densities = vec![1.0; domain.num_elements()];
        let mesh = extract_mesh(&domain, &densities, 5);
        assert_watertight(&mesh);
        let mut max = [f32::NEG_INFINITY; 3];
        for v in mesh.vertices.chunks_exact(3) {
            for x in 0..3 {
                max[x] = max[x].max(v[x]);
            }
        }
        // Taubin smoothing must not collapse the part.
        for x in 0..3 {
            assert!(max[x] > 5.0, "axis {x} shrank to {}", max[x]);
        }
    }

    #[test]
    fn normals_point_outward_for_box() {
        let domain = Domain::from_bbox([0.0; 3], [4.0; 3], 4);
        let densities = vec![1.0; domain.num_elements()];
        let mesh = extract_mesh(&domain, &densities, 0);
        let center = [2.0f32; 3];
        let mut outward = 0usize;
        let mut inward = 0usize;
        for i in 0..mesh.num_vertices() {
            let p = &mesh.vertices[3 * i..3 * i + 3];
            let n = &mesh.normals[3 * i..3 * i + 3];
            let d: f32 = (0..3).map(|x| (p[x] - center[x]) * n[x]).sum();
            if d > 0.0 {
                outward += 1;
            } else if d < 0.0 {
                inward += 1;
            }
        }
        assert!(
            outward > inward * 10,
            "normals look inverted: {outward} outward vs {inward} inward"
        );
    }
}
