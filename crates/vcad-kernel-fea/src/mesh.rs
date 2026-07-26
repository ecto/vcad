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

use serde::{Deserialize, Serialize};
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
                "no lattice cell center lies inside the surface mesh — the part is thinner \
                 than the lattice pitch everywhere. Raise the resolution if the part is only \
                 slightly under it; if it is sheet or tube, the lattice is the wrong \
                 discretization entirely — use the closed-form prismatic route (`beam_check`)"
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

/// How thick the solid actually is, measured rather than assumed.
///
/// Produced by [`wall_thickness`]: axis-aligned rays are cast through the
/// part on a sampling grid, the solid spans along each ray are collected,
/// and the distribution is reported. For plate and prismatic-tube geometry
/// — where the thin direction is (near enough) an axis — the low
/// percentiles of that distribution *are* the wall thickness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThicknessStats {
    /// Shortest solid span seen, mm. Grazing rays at a curved surface make
    /// this pessimistic; prefer `p05_mm` for decisions.
    pub min_mm: f64,
    /// 5th-percentile solid span, mm — the working estimate of the
    /// thinnest load-bearing section.
    pub p05_mm: f64,
    /// Median solid span, mm.
    pub median_mm: f64,
    /// Axis (`"x"`, `"y"`, `"z"`) the 5th-percentile span was measured
    /// along, i.e. the direction the part is thin in.
    pub thin_axis: String,
    /// Number of solid spans sampled.
    pub samples: usize,
    /// Longest bounding-box edge, mm — the axis the lattice resolution
    /// counts cells along.
    pub longest_bbox_mm: f64,
}

/// Measure the solid-span thickness distribution of a closed mesh.
///
/// `samples_per_axis` rays are cast on a `n x n` grid for each of the
/// three axis directions (so `3·n²` rays); 32 is a good default — the cost
/// is a small fraction of one FE solve.
///
/// Known limits, both in the conservative direction for the diagnosis this
/// feeds: a thin feature covering less than ~5% of the sampled spans (one
/// fin on a chunky block) sits below the 5th percentile and will not trip
/// the gate — `min_mm` is reported so a caller can see it anyway — and a
/// part whose thin direction is not near an axis reads thicker than it is,
/// because an oblique ray crosses a wall along the hypotenuse.
pub fn wall_thickness(
    mesh: &TriangleMesh,
    samples_per_axis: usize,
) -> Result<ThicknessStats, MeshError> {
    let nv = mesh.num_vertices();
    if nv == 0 {
        return Err(MeshError::EmptyMesh);
    }
    let n = samples_per_axis.clamp(4, 256);
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

    // Spans per ray axis, so the thin direction can be named.
    let mut per_axis: [Vec<f64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for (axis, spans) in per_axis.iter_mut().enumerate() {
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        // Offsets are irrational-ish fractions of the cell so rays never
        // land on a vertex or edge, where parity is ambiguous.
        for iu in 0..n {
            let pu = min[u] + (iu as f64 + 0.5137) / n as f64 * size[u];
            for iv in 0..n {
                let pv = min[v] + (iv as f64 + 0.4861) / n as f64 * size[v];
                let mut hits: Vec<f64> = tris
                    .iter()
                    .filter_map(|tri| ray_crossing(tri, pu, pv, axis))
                    .collect();
                if hits.len() < 2 {
                    continue;
                }
                hits.sort_by(|a, b| a.partial_cmp(b).unwrap());
                for pair in hits.chunks_exact(2) {
                    let span = pair[1] - pair[0];
                    if span > 0.0 {
                        spans.push(span);
                    }
                }
            }
        }
    }

    let mut all: Vec<f64> = per_axis.iter().flatten().copied().collect();
    if all.is_empty() {
        return Err(MeshError::NoInteriorCells);
    }
    all.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |sorted: &[f64], q: f64| -> f64 {
        if sorted.is_empty() {
            return f64::INFINITY;
        }
        let i = ((sorted.len() - 1) as f64 * q).round() as usize;
        sorted[i]
    };
    let p05 = pct(&all, 0.05);
    // Name the axis whose own 5th percentile is smallest — the direction
    // the part is thin in.
    let mut thin_axis = 0usize;
    let mut best = f64::INFINITY;
    for (axis, spans) in per_axis.iter_mut().enumerate() {
        if spans.is_empty() {
            continue;
        }
        spans.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let q = pct(spans, 0.05);
        if q < best {
            best = q;
            thin_axis = axis;
        }
    }
    Ok(ThicknessStats {
        min_mm: all[0],
        p05_mm: p05,
        median_mm: pct(&all, 0.5),
        thin_axis: ["x", "y", "z"][thin_axis].to_string(),
        samples: all.len(),
        longest_bbox_mm: longest,
    })
}

/// The lattice can or cannot resolve this part's thinnest section — and if
/// it cannot, what to do instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThinWallDiagnosis {
    /// Measured thickness distribution.
    pub thickness: ThicknessStats,
    /// Lattice pitch at the finest level actually solved, mm.
    pub finest_pitch_mm: f64,
    /// Cells through the thinnest section at that pitch.
    pub cells_through_section: f64,
    /// Coarse-level resolution that would put `cells_wanted` cells through
    /// the thinnest section at the finest level.
    pub required_resolution: usize,
    /// The resolution cap this tier enforces.
    pub resolution_cap: usize,
    /// Whether `required_resolution` fits under the cap.
    pub reachable: bool,
    /// Set when the lattice cannot resolve the section at all — this text
    /// is appended to the convergence reasons, so a caller never has to
    /// redo the cell arithmetic to learn why a study failed.
    pub blocking_advice: Option<String>,
    /// Set when the section is resolved thinly but not fatally — the study
    /// still runs and the convergence gate still judges it.
    pub advisory: Option<String>,
}

/// Cells through the thinnest section below which a lattice result is not
/// worth reporting at all (one or two cells cannot bend).
const CELLS_BLOCKING: f64 = 4.0;
/// Cells through the thinnest section we actually want (linear tets are
/// stiff in bending).
const CELLS_WANTED: f64 = 6.0;

/// Diagnose whether the lattice pitch can resolve the part's thinnest
/// section, given the finest level a study will reach and this tier's cap.
///
/// `finest_resolution` is the cell count along the longest bbox axis at the
/// finest level (`resolution << (levels - 1)`); `resolution_cap` is the
/// largest coarse-level resolution the caller is allowed to ask for.
pub fn diagnose_thin_wall(
    thickness: ThicknessStats,
    finest_resolution: usize,
    resolution_cap: usize,
) -> ThinWallDiagnosis {
    let finest_pitch_mm = thickness.longest_bbox_mm / finest_resolution.max(1) as f64;
    let cells_through_section = thickness.p05_mm / finest_pitch_mm.max(1e-12);
    let required_pitch = thickness.p05_mm / CELLS_WANTED;
    let required_resolution =
        (thickness.longest_bbox_mm / required_pitch.max(1e-12)).ceil() as usize;
    let reachable = required_resolution <= resolution_cap;
    let blocking_advice = if cells_through_section < CELLS_BLOCKING {
        Some(format!(
            "THIN-WALLED GEOMETRY — the lattice cannot resolve this part. Its thinnest \
             load-bearing section measures {:.2} mm (5th-percentile solid span, along {}), and \
             the finest level solved has a {:.2} mm pitch, so only {:.1} cell(s) sit through \
             that section. {:.0} cells would need a {:.3} mm pitch, i.e. resolution \
             {required_resolution} along the {:.0} mm longest axis — {}. \
             A staircase at one cell per wall is not an approximation of a thin wall, it is a \
             different part, so this study claims nothing. Route forward: if the member is \
             prismatic (plate, round or rectangular tube, I-section), use `beam_check` — \
             closed-form section properties with Bredt thin-wall torsion and beam bending, \
             which for a constant cross-section is not a fallback but the MORE accurate \
             answer, and it carries the same predicted-basis receipt. Otherwise split the \
             assembly and check the thin members individually, or wait on shell/beam elements \
             (the real fix for thin-walled FEA).",
            thickness.p05_mm,
            thickness.thin_axis,
            finest_pitch_mm,
            cells_through_section,
            CELLS_WANTED,
            required_pitch,
            thickness.longest_bbox_mm,
            if reachable {
                format!(
                    "within the cap of {resolution_cap}, so raising `resolution` would work \
                         (at a steep cost: cells scale as the cube)"
                )
            } else {
                format!("far past the cap of {resolution_cap}, so no resolution setting reaches it")
            },
        ))
    } else {
        None
    };
    let advisory = if blocking_advice.is_none() && cells_through_section < CELLS_WANTED {
        Some(format!(
            "only {cells_through_section:.1} cells sit through the thinnest section \
             ({:.2} mm, along {}) at the {finest_pitch_mm:.2} mm finest pitch; linear tets are \
             stiff in bending, so this reads too stiff. Resolution {required_resolution} would \
             give {CELLS_WANTED:.0}. The convergence gate still judges the study.",
            thickness.p05_mm, thickness.thin_axis
        ))
    } else {
        None
    };
    ThinWallDiagnosis {
        thickness,
        finest_pitch_mm,
        cells_through_section,
        required_resolution,
        resolution_cap,
        reachable,
        blocking_advice,
        advisory,
    }
}

/// Z (or X/Y, per `axis`) of the intersection between an axis-aligned line
/// through `(u, v)` in the other two coordinates and a triangle.
fn ray_crossing(tri: &[[f64; 3]; 3], u: f64, v: f64, axis: usize) -> Option<f64> {
    let (iu, iv) = ((axis + 1) % 3, (axis + 2) % 3);
    let [a, b, c] = tri;
    let d = (b[iu] - c[iu]) * (a[iv] - c[iv]) - (b[iv] - c[iv]) * (a[iu] - c[iu]);
    if d.abs() < 1e-30 {
        return None;
    }
    let w0 = ((b[iu] - c[iu]) * (v - c[iv]) - (b[iv] - c[iv]) * (u - c[iu])) / d;
    let w1 = ((c[iu] - a[iu]) * (v - c[iv]) - (c[iv] - a[iv]) * (u - c[iu])) / d;
    let w2 = 1.0 - w0 - w1;
    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
        return None;
    }
    Some(w0 * a[axis] + w1 * b[axis] + w2 * c[axis])
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

    /// A real open-ended rectangular tube: the extrusion of a rectangular
    /// annulus along X, closed by the two end annuli. Four outer walls,
    /// four inner walls, eight end quads — watertight, which is all ray
    /// parity needs.
    fn rect_tube_mesh(len: f64, w: f64, h: f64, t: f64) -> TriangleMesh {
        let loop_yz = [
            // outer, then inner, both counter-clockwise in (y, z)
            (0.0, 0.0),
            (w, 0.0),
            (w, h),
            (0.0, h),
            (t, t),
            (w - t, t),
            (w - t, h - t),
            (t, h - t),
        ];
        let mut mesh = TriangleMesh::new();
        for x in [0.0f64, len] {
            for (y, z) in loop_yz {
                mesh.vertices
                    .extend_from_slice(&[x as f32, y as f32, z as f32]);
            }
        }
        // v(end, k): end 0 at x=0, end 1 at x=len; k 0..3 outer, 4..7 inner.
        let v = |end: u32, k: u32| end * 8 + k;
        let mut quad = |a: u32, b: u32, c: u32, d: u32| {
            mesh.indices.extend_from_slice(&[a, b, c, a, c, d]);
        };
        for k in 0..4u32 {
            let kn = (k + 1) % 4;
            // Outer wall, inner wall, and the two end-face quads spanning
            // outer edge k to inner edge k.
            quad(v(0, k), v(0, kn), v(1, kn), v(1, k));
            quad(v(0, k + 4), v(0, kn + 4), v(1, kn + 4), v(1, k + 4));
            quad(v(0, k), v(0, k + 4), v(0, kn + 4), v(0, kn));
            quad(v(1, k), v(1, k + 4), v(1, kn + 4), v(1, kn));
        }
        mesh
    }

    #[test]
    fn thickness_sampler_measures_a_tube_wall() {
        // 312 mm long 40x40 tube, 2 mm wall — the chassis member the
        // lattice cannot resolve.
        let mesh = rect_tube_mesh(312.0, 40.0, 40.0, 2.0);
        let stats = wall_thickness(&mesh, 32).unwrap();
        assert_eq!(stats.longest_bbox_mm, 312.0);
        assert!(
            (stats.p05_mm - 2.0).abs() < 0.2,
            "p05 {} should be the 2 mm wall",
            stats.p05_mm
        );
        assert!(
            stats.thin_axis == "y" || stats.thin_axis == "z",
            "{stats:?}"
        );
    }

    #[test]
    fn thin_wall_diagnosis_carries_the_cell_arithmetic_and_a_route() {
        let mesh = rect_tube_mesh(312.0, 40.0, 40.0, 2.0);
        let stats = wall_thickness(&mesh, 32).unwrap();
        // The exact case from the field report: finest level 160 cells.
        let longest = stats.longest_bbox_mm;
        let d = diagnose_thin_wall(stats, 160, 160);
        assert!(
            (d.finest_pitch_mm - longest / 160.0).abs() < 1e-9,
            "pitch {}",
            d.finest_pitch_mm
        );
        assert!(
            (d.finest_pitch_mm - 1.95).abs() < 0.01,
            "pitch {}",
            d.finest_pitch_mm
        );
        assert!(
            d.cells_through_section < 1.3,
            "cells {}",
            d.cells_through_section
        );
        // ~6 cells through a 2 mm wall needs ~950 along the 312 mm axis.
        assert!(
            (900..1050).contains(&d.required_resolution),
            "required {}",
            d.required_resolution
        );
        assert!(!d.reachable, "950 cells is not under a 160 cap");
        let advice = d.blocking_advice.expect("must diagnose, not just fail");
        assert!(advice.contains("beam_check"), "no route forward: {advice}");
        // The arithmetic the caller would otherwise have to redo by hand.
        assert!(advice.contains("2.00 mm"), "{advice}");
        assert!(advice.contains("1.95 mm pitch"), "{advice}");
        assert!(advice.contains("resolution 936"), "{advice}");
        assert!(advice.contains("cap of 160"), "{advice}");
    }

    #[test]
    fn a_chunky_part_is_not_diagnosed_as_thin_walled() {
        let mesh = box_mesh([0.0; 3], [80.0, 10.0, 10.0]);
        let stats = wall_thickness(&mesh, 32).unwrap();
        assert!((stats.p05_mm - 10.0).abs() < 1e-6, "p05 {}", stats.p05_mm);
        // Finest level 64 -> pitch 1.25 mm -> 8 cells through the 10 mm
        // section: resolved, no advice of any kind.
        let d = diagnose_thin_wall(stats, 64, 160);
        assert!(d.blocking_advice.is_none(), "{:?}", d.blocking_advice);
        assert!(d.advisory.is_none(), "{:?}", d.advisory);
        assert!(d.reachable);
    }

    #[test]
    fn thinly_resolved_but_not_blocked_gets_an_advisory_only() {
        // 5 cells through the section: the study should run (the gate still
        // judges it) but the caller is told the mesh reads too stiff.
        let mesh = box_mesh([0.0; 3], [100.0, 40.0, 10.0]);
        let stats = wall_thickness(&mesh, 32).unwrap();
        assert!((stats.p05_mm - 10.0).abs() < 1e-6);
        let d = diagnose_thin_wall(stats, 50, 160); // pitch 2.0 mm -> 5 cells
        assert!((d.cells_through_section - 5.0).abs() < 1e-9);
        assert!(d.blocking_advice.is_none());
        let advisory = d.advisory.expect("advisory between 4 and 6 cells");
        assert!(advisory.contains("stiff"), "{advisory}");
        assert!(d.reachable, "60 cells reaches 6 through the section");
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
