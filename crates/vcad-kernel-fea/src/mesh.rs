//! Lattice tetrahedral fill of a closed triangle mesh.
//!
//! Interior detection is per-column ray parity along +Z (the same
//! machinery `vcad-kernel-topopt` voxelizes with): a lattice cell belongs
//! to the solid when its center lies inside the surface mesh. Each
//! interior cell is split into six linear tetrahedra with the Kuhn
//! (Freudenthal) decomposition about the cell's main diagonal, which is
//! face-conforming across identically split neighbor cells — the mesh is
//! watertight by construction wherever cells touch.
//!
//! The boundary is therefore a staircase at the lattice pitch `h`. That
//! geometric error is first-order in `h` and is exactly what the M1
//! convergence gate measures; do not read a single-resolution solve as
//! converged.

use vcad_kernel_tessellate::TriangleMesh;

/// A linear tetrahedral mesh on a uniform lattice.
#[derive(Debug, Clone)]
pub struct TetMesh {
    /// Node positions, mm.
    pub nodes: Vec<[f64; 3]>,
    /// Tetrahedra as node indices, positively oriented (signed volume > 0).
    pub tets: Vec<[u32; 4]>,
    /// Lattice pitch, mm.
    pub h: f64,
    /// Lattice cell counts per axis.
    pub grid: [usize; 3],
}

impl TetMesh {
    /// Total mesh volume, mm³.
    pub fn volume(&self) -> f64 {
        self.tets
            .iter()
            .map(|t| tet_volume(&self.nodes, t))
            .sum::<f64>()
    }
}

/// Signed volume of one tet, mm³.
pub(crate) fn tet_volume(nodes: &[[f64; 3]], t: &[u32; 4]) -> f64 {
    let p = |i: usize| nodes[t[i] as usize];
    let a = p(0);
    let d1 = sub(p(1), a);
    let d2 = sub(p(2), a);
    let d3 = sub(p(3), a);
    det3(d1, d2, d3) / 6.0
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn det3(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> f64 {
    a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
        + a[2] * (b[0] * c[1] - b[1] * c[0])
}

/// Meshing failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeshError {
    /// The surface mesh is empty.
    EmptyMesh,
    /// No lattice cell center fell inside the surface — the part is
    /// thinner than the lattice pitch everywhere; raise the resolution.
    NoInteriorCells,
}

impl std::fmt::Display for MeshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MeshError::EmptyMesh => write!(f, "surface mesh has no vertices"),
            MeshError::NoInteriorCells => write!(
                f,
                "no lattice cell center lies inside the surface mesh — the part is \
                 thinner than the lattice pitch; raise the resolution"
            ),
        }
    }
}

impl std::error::Error for MeshError {}

/// The six Kuhn tets of a unit cell, as local corner indices with corner
/// `ix + 2*iy + 4*iz`. Every tet shares the main diagonal 0–7; the split
/// is identical in every cell, so shared faces match across neighbors.
const KUHN: [[usize; 4]; 6] = [
    [0, 1, 3, 7],
    [0, 3, 2, 7],
    [0, 2, 6, 7],
    [0, 6, 4, 7],
    [0, 4, 5, 7],
    [0, 5, 1, 7],
];

/// Fill the interior of a closed triangle mesh with linear tets.
///
/// `resolution` is the lattice cell count along the longest bounding-box
/// axis, clamped to `[2, 256]`.
pub fn tet_fill(mesh: &TriangleMesh, resolution: usize) -> Result<TetMesh, MeshError> {
    let nv = mesh.num_vertices();
    if nv == 0 {
        return Err(MeshError::EmptyMesh);
    }
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for i in 0..nv {
        for a in 0..3 {
            let v = mesh.vertices[3 * i + a] as f64;
            min[a] = min[a].min(v);
            max[a] = max[a].max(v);
        }
    }
    let size = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
    let longest = size[0].max(size[1]).max(size[2]).max(1e-9);
    let resolution = resolution.clamp(2, 256);
    let h = longest / resolution as f64;
    let nx = ((size[0] / h).round() as usize).max(1);
    let ny = ((size[1] / h).round() as usize).max(1);
    let nz = ((size[2] / h).round() as usize).max(1);

    // Gather triangles as f64 for the parity test.
    let tris: Vec<[[f64; 3]; 3]> = (0..mesh.num_triangles())
        .map(|t| {
            let mut tri = [[0.0; 3]; 3];
            for (k, corner) in tri.iter_mut().enumerate() {
                let vi = mesh.indices[3 * t + k] as usize;
                for (a, c) in corner.iter_mut().enumerate() {
                    *c = mesh.vertices[3 * vi + a] as f64;
                }
            }
            tri
        })
        .collect();

    // Per-column parity fill of the active-cell mask. The small
    // deterministic offset keeps ray origins away from vertices and
    // edges, where parity is ambiguous.
    let mut active = vec![false; nx * ny * nz];
    let eidx = |ix: usize, iy: usize, iz: usize| (iz * ny + iy) * nx + ix;
    let jitter = h * 0.01371;
    for iy in 0..ny {
        for ix in 0..nx {
            let cx = min[0] + (ix as f64 + 0.5) * h + jitter;
            let cy = min[1] + (iy as f64 + 0.5) * h + jitter * 0.618;
            let mut crossings: Vec<f64> = tris
                .iter()
                .filter_map(|tri| ray_z_crossing(tri, cx, cy))
                .collect();
            crossings.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let mut k = 0;
            while k + 1 < crossings.len() {
                let z0 = crossings[k];
                let z1 = crossings[k + 1];
                k += 2;
                let i0 = ((z0 - min[2]) / h - 0.5).ceil().max(0.0) as usize;
                let i1f = ((z1 - min[2]) / h - 0.5).floor();
                if i1f < 0.0 {
                    continue;
                }
                let i1 = (i1f as usize).min(nz.saturating_sub(1));
                for iz in i0..=i1 {
                    if iz < nz {
                        active[eidx(ix, iy, iz)] = true;
                    }
                }
            }
        }
    }
    if !active.iter().any(|a| *a) {
        return Err(MeshError::NoInteriorCells);
    }

    // Compact lattice nodes used by active cells and emit tets.
    let nxp = nx + 1;
    let nyp = ny + 1;
    let nidx = |ix: usize, iy: usize, iz: usize| (iz * nyp + iy) * nxp + ix;
    let mut node_map = vec![u32::MAX; nxp * nyp * (nz + 1)];
    let mut nodes: Vec<[f64; 3]> = Vec::new();
    let mut tets: Vec<[u32; 4]> = Vec::new();
    for iz in 0..nz {
        for iy in 0..ny {
            for ix in 0..nx {
                if !active[eidx(ix, iy, iz)] {
                    continue;
                }
                let mut corner = [0u32; 8];
                for (c, slot) in corner.iter_mut().enumerate() {
                    let (dx, dy, dz) = (c & 1, (c >> 1) & 1, (c >> 2) & 1);
                    let gi = nidx(ix + dx, iy + dy, iz + dz);
                    if node_map[gi] == u32::MAX {
                        node_map[gi] = nodes.len() as u32;
                        nodes.push([
                            min[0] + (ix + dx) as f64 * h,
                            min[1] + (iy + dy) as f64 * h,
                            min[2] + (iz + dz) as f64 * h,
                        ]);
                    }
                    *slot = node_map[gi];
                }
                for k in &KUHN {
                    let mut t = [corner[k[0]], corner[k[1]], corner[k[2]], corner[k[3]]];
                    if tet_volume(&nodes, &t) < 0.0 {
                        t.swap(1, 2);
                    }
                    tets.push(t);
                }
            }
        }
    }

    Ok(TetMesh {
        nodes,
        tets,
        h,
        grid: [nx, ny, nz],
    })
}

/// Z of the intersection between a vertical line at `(x, y)` and a
/// triangle, or `None` when the line misses it.
fn ray_z_crossing(tri: &[[f64; 3]; 3], x: f64, y: f64) -> Option<f64> {
    let [a, b, c] = tri;
    let d = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
    if d.abs() < 1e-30 {
        return None;
    }
    let w0 = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (y - c[1])) / d;
    let w1 = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (y - c[1])) / d;
    let w2 = 1.0 - w0 - w1;
    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
        return None;
    }
    Some(w0 * a[2] + w1 * b[2] + w2 * c[2])
}

/// Build a box surface mesh (test/benchmark helper; also handy for
/// closed-form validation cases).
pub fn box_mesh(min: [f64; 3], max: [f64; 3]) -> TriangleMesh {
    let verts: [[f64; 3]; 8] = [
        [min[0], min[1], min[2]],
        [max[0], min[1], min[2]],
        [max[0], max[1], min[2]],
        [min[0], max[1], min[2]],
        [min[0], min[1], max[2]],
        [max[0], min[1], max[2]],
        [max[0], max[1], max[2]],
        [min[0], max[1], max[2]],
    ];
    let quads = [
        [0u32, 3, 2, 1],
        [4, 5, 6, 7],
        [0, 1, 5, 4],
        [2, 3, 7, 6],
        [1, 2, 6, 5],
        [3, 0, 4, 7],
    ];
    let mut mesh = TriangleMesh::new();
    for v in &verts {
        mesh.vertices
            .extend_from_slice(&[v[0] as f32, v[1] as f32, v[2] as f32]);
    }
    for q in &quads {
        mesh.indices
            .extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_fill_recovers_volume_and_conformity() {
        let mesh = box_mesh([0.0; 3], [20.0, 10.0, 10.0]);
        let tm = tet_fill(&mesh, 20).unwrap();
        assert_eq!(tm.grid, [20, 10, 10]);
        // Box faces align with the lattice, so the fill volume is exact.
        let vol = tm.volume();
        assert!((vol - 2000.0).abs() / 2000.0 < 1e-9, "volume {vol} != 2000");
        // 6 tets per cell, all positively oriented.
        assert_eq!(tm.tets.len(), 6 * 20 * 10 * 10);
        for t in &tm.tets {
            assert!(tet_volume(&tm.nodes, t) > 0.0);
        }
        // Watertight interior: every interior face is shared by exactly
        // two tets; boundary faces by one. Count faces by sorted key.
        use std::collections::HashMap;
        let mut faces: HashMap<[u32; 3], u32> = HashMap::new();
        for t in &tm.tets {
            for f in [[0, 1, 2], [0, 1, 3], [0, 2, 3], [1, 2, 3]] {
                let mut key = [t[f[0]], t[f[1]], t[f[2]]];
                key.sort_unstable();
                *faces.entry(key).or_insert(0) += 1;
            }
        }
        assert!(faces.values().all(|&c| c == 1 || c == 2));
        // Boundary face area of the fully-aligned box = surface area.
        let boundary = faces.values().filter(|&&c| c == 1).count();
        // Each lattice square on the boundary contributes 2 or 4 boundary
        // triangles depending on the Kuhn cut; just sanity-check nonzero.
        assert!(boundary > 0);
    }

    #[test]
    fn empty_and_too_thin_fail_closed() {
        assert!(matches!(
            tet_fill(&TriangleMesh::new(), 16),
            Err(MeshError::EmptyMesh)
        ));
        // A plate 0.05 mm thick at pitch 100/16: no cell center inside.
        let thin = box_mesh([0.0; 3], [100.0, 100.0, 0.05]);
        assert!(matches!(
            tet_fill(&thin, 16),
            Err(MeshError::NoInteriorCells)
        ));
    }

    #[test]
    fn refinement_shrinks_pitch() {
        let mesh = box_mesh([0.0; 3], [10.0; 3]);
        let a = tet_fill(&mesh, 8).unwrap();
        let b = tet_fill(&mesh, 16).unwrap();
        assert!((a.h - 2.0 * b.h).abs() < 1e-12);
        assert!(b.tets.len() > a.tets.len());
    }
}
