//! CAM verification oracle: grade a simulated stock against the target part.
//!
//! After a toolpath has been subtracted from a [`Stock`], the remaining
//! material should match the target part. Two things can go wrong, and each
//! gets its own check:
//!
//! * **Gouge** — the toolpath removed material the part needs. Detected as
//!   samples inside the target (deeper than `tolerance`) where the stock
//!   reads air.
//! * **Excess** — the toolpath failed to remove material it was supposed
//!   to. Detected as samples outside the target (farther than `allowance`)
//!   where the stock still reads material.
//!
//! Sampling happens on a grid aligned with the octree leaf centers, where
//! the stored SDF sign is most trustworthy. Violations thinner than one
//! leaf cell can escape detection, so pick the stock resolution below the
//! finest defect you care about.

use serde::{Deserialize, Serialize};

use crate::Stock;

/// Cap on total grid samples; deeper octrees get strided sampling.
const MAX_SAMPLES: usize = 1_000_000;

/// Options for [`verify_stock_against_mesh`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyOptions {
    /// Maximum depth (mm) that material removal may intrude past the target
    /// surface before it counts as a gouge.
    pub tolerance: f64,
    /// Maximum thickness (mm) of leftover material outside the target
    /// (machining allowance) before it counts as excess.
    pub allowance: f64,
    /// Cap on example violation points reported per check.
    pub max_examples: usize,
}

impl Default for VerifyOptions {
    fn default() -> Self {
        Self {
            tolerance: 0.1,
            allowance: 0.1,
            max_examples: 16,
        }
    }
}

/// A sampled violation point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViolationSample {
    /// World position of the sample (mm).
    pub position: [f64; 3],
    /// Distance from the target surface (mm): intrusion depth for gouges,
    /// leftover thickness for excess.
    pub depth: f64,
}

/// Outcome of one check (gouge or excess).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckReport {
    /// True when no violations were found.
    pub pass: bool,
    /// Number of violating grid samples.
    pub violation_count: usize,
    /// Deepest violation found (mm); 0 when clean.
    pub worst_depth: f64,
    /// Up to `max_examples` violating points, deepest kept.
    pub examples: Vec<ViolationSample>,
}

/// Structured result of stock-vs-target verification.
///
/// Plain serializable data so it can be embedded in higher-level reports
/// (e.g. a build receipt claim).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StockVerification {
    /// True when both checks pass.
    pub pass: bool,
    /// Material removed that the target needed.
    pub gouge: CheckReport,
    /// Material left that the toolpath should have removed.
    pub excess: CheckReport,
    /// Gouge tolerance requested (mm).
    pub tolerance: f64,
    /// Machining allowance requested (mm).
    pub allowance: f64,
    /// Extra margin added to both thresholds to absorb simulation
    /// discretization error (mm).
    pub sim_margin: f64,
    /// Number of grid samples evaluated.
    pub grid_samples: usize,
    /// Grid cell size per axis (mm). Violations thinner than a cell can
    /// escape detection.
    pub cell_size: [f64; 3],
}

/// Verify a simulated stock against a target triangle mesh.
///
/// `vertices` and `indices` use the tessellation layout: flat `xyz` f32
/// triples and triangle index triples (`TriangleMesh::vertices` /
/// `TriangleMesh::indices`). The mesh must be closed, since containment is
/// decided by ray casting.
pub fn verify_stock_against_mesh(
    stock: &Stock,
    vertices: &[f32],
    indices: &[u32],
    opts: &VerifyOptions,
) -> StockVerification {
    let tris = collect_triangles(vertices, indices);
    let b = stock.bounds();
    let n = 1usize << stock.max_depth();
    let cell = [
        (b[3] - b[0]) / n as f64,
        (b[4] - b[1]) / n as f64,
        (b[5] - b[2]) / n as f64,
    ];

    // Stride the grid when the octree is deep enough that sampling every
    // leaf would blow the budget.
    let mut stride = 1usize;
    while (n.div_ceil(stride)).pow(3) > MAX_SAMPLES {
        stride += 1;
    }

    // Samples sit exactly on leaf centers, where the stored SDF sign is
    // near-exact. The margin only needs to absorb the small model errors:
    // stamp scallops, arc linearization, and target tessellation sag.
    let max_cell = cell[0].max(cell[1]).max(cell[2]);
    let sim_margin = 0.25 * max_cell + 0.01;
    let gouge_threshold = opts.tolerance + sim_margin;
    let excess_threshold = opts.allowance + sim_margin;

    let mut gouge = CheckAccum::new(opts.max_examples);
    let mut excess = CheckAccum::new(opts.max_examples);
    let mut grid_samples = 0usize;

    for i in (0..n).step_by(stride) {
        let x = b[0] + (i as f64 + 0.5) * cell[0];
        for j in (0..n).step_by(stride) {
            let y = b[1] + (j as f64 + 0.5) * cell[1];
            for k in (0..n).step_by(stride) {
                let z = b[2] + (k as f64 + 0.5) * cell[2];
                grid_samples += 1;

                let p = [x, y, z];
                let stock_sdf = stock.sdf_at(x, y, z);
                let inside_target = point_in_triangles(p, &tris);

                if inside_target && stock_sdf > 0.0 {
                    // Target needs material here but the stock lost it.
                    let d = distance_to_triangles(p, &tris);
                    if d > gouge_threshold {
                        gouge.add(p, d);
                    }
                } else if !inside_target && stock_sdf < 0.0 {
                    // Stock kept material the target doesn't want.
                    let d = distance_to_triangles(p, &tris);
                    if d > excess_threshold {
                        excess.add(p, d);
                    }
                }
            }
        }
    }

    let gouge = gouge.finish();
    let excess = excess.finish();
    StockVerification {
        pass: gouge.pass && excess.pass,
        gouge,
        excess,
        tolerance: opts.tolerance,
        allowance: opts.allowance,
        sim_margin,
        grid_samples,
        cell_size: cell,
    }
}

/// Accumulator for one check, keeping the deepest examples.
struct CheckAccum {
    count: usize,
    worst: f64,
    examples: Vec<ViolationSample>,
    cap: usize,
}

impl CheckAccum {
    fn new(cap: usize) -> Self {
        Self {
            count: 0,
            worst: 0.0,
            examples: Vec::new(),
            cap,
        }
    }

    fn add(&mut self, position: [f64; 3], depth: f64) {
        self.count += 1;
        self.worst = self.worst.max(depth);
        if self.examples.len() < self.cap {
            self.examples.push(ViolationSample { position, depth });
        } else if let Some((idx, shallowest)) = self
            .examples
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| a.depth.total_cmp(&b.depth))
            .map(|(idx, s)| (idx, s.depth))
        {
            if depth > shallowest {
                self.examples[idx] = ViolationSample { position, depth };
            }
        }
    }

    fn finish(self) -> CheckReport {
        CheckReport {
            pass: self.count == 0,
            violation_count: self.count,
            worst_depth: self.worst,
            examples: self.examples,
        }
    }
}

type Triangle = [[f64; 3]; 3];

fn collect_triangles(vertices: &[f32], indices: &[u32]) -> Vec<Triangle> {
    indices
        .chunks_exact(3)
        .map(|tri| {
            let v = |idx: u32| {
                let i = idx as usize * 3;
                [
                    vertices[i] as f64,
                    vertices[i + 1] as f64,
                    vertices[i + 2] as f64,
                ]
            };
            [v(tri[0]), v(tri[1]), v(tri[2])]
        })
        .collect()
}

/// Point containment by ray crossing count. The ray direction is slightly
/// tilted so it doesn't hit shared edges or vertices exactly.
fn point_in_triangles(p: [f64; 3], tris: &[Triangle]) -> bool {
    let dir = [1.0, 1e-7, 1.3e-7];
    let mut crossings = 0u32;
    for tri in tris {
        if ray_hits_triangle(p, dir, tri) {
            crossings += 1;
        }
    }
    crossings % 2 == 1
}

/// Möller–Trumbore ray-triangle intersection (t > 0 only).
fn ray_hits_triangle(orig: [f64; 3], dir: [f64; 3], tri: &Triangle) -> bool {
    let e1 = sub(tri[1], tri[0]);
    let e2 = sub(tri[2], tri[0]);
    let h = cross(dir, e2);
    let a = dot(e1, h);
    if a.abs() < 1e-12 {
        return false;
    }
    let f = 1.0 / a;
    let s = sub(orig, tri[0]);
    let u = f * dot(s, h);
    if !(0.0..=1.0).contains(&u) {
        return false;
    }
    let q = cross(s, e1);
    let v = f * dot(dir, q);
    if v < 0.0 || u + v > 1.0 {
        return false;
    }
    f * dot(e2, q) > 1e-9
}

fn distance_to_triangles(p: [f64; 3], tris: &[Triangle]) -> f64 {
    tris.iter()
        .map(|tri| {
            let c = closest_point_on_triangle(p, tri[0], tri[1], tri[2]);
            norm(sub(p, c))
        })
        .fold(f64::INFINITY, f64::min)
}

/// Closest point on a triangle to `p` (Ericson, Real-Time Collision
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

fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a closed 12-triangle box mesh in the tessellation layout.
    fn box_mesh(min: [f64; 3], max: [f64; 3]) -> (Vec<f32>, Vec<u32>) {
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
        let vertices: Vec<f32> = corners
            .iter()
            .flat_map(|c| c.iter().map(|&v| v as f32))
            .collect();
        let quads: [[u32; 4]; 6] = [
            [0, 3, 2, 1], // bottom
            [4, 5, 6, 7], // top
            [0, 1, 5, 4], // -y
            [2, 3, 7, 6], // +y
            [1, 2, 6, 5], // +x
            [0, 4, 7, 3], // -x
        ];
        let indices: Vec<u32> = quads
            .iter()
            .flat_map(|q| [q[0], q[1], q[2], q[0], q[2], q[3]])
            .collect();
        (vertices, indices)
    }

    #[test]
    fn test_untouched_stock_matching_target_passes() {
        let stock = Stock::from_box([0.0, 0.0, 0.0, 10.0, 10.0, 10.0], 1.0);
        let (verts, idx) = box_mesh([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);

        let result = verify_stock_against_mesh(&stock, &verts, &idx, &VerifyOptions::default());
        assert!(result.pass, "identical stock and target: {result:?}");
        assert_eq!(result.gouge.violation_count, 0);
        assert_eq!(result.excess.violation_count, 0);
        assert!(result.grid_samples > 0);
    }

    #[test]
    fn test_subtracted_sphere_is_a_gouge() {
        let mut stock = Stock::from_box([0.0, 0.0, 0.0, 10.0, 10.0, 10.0], 1.0);
        stock.subtract_sphere([5.0, 5.0, 5.0], 3.0);
        let (verts, idx) = box_mesh([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);

        let result = verify_stock_against_mesh(&stock, &verts, &idx, &VerifyOptions::default());
        assert!(!result.gouge.pass, "hollowed core must be a gouge");
        assert!(
            result.gouge.worst_depth > 2.0,
            "deepest gouge sample should approach the box center, got {}",
            result.gouge.worst_depth
        );
        assert!(!result.gouge.examples.is_empty());
        assert!(
            result.excess.pass,
            "nothing was left over: {:?}",
            result.excess
        );
        assert!(!result.pass);
    }

    #[test]
    fn test_unmachined_layer_is_excess() {
        // Target stops at z=8 but the stock was never machined: the top
        // 2mm is leftover material.
        let stock = Stock::from_box([0.0, 0.0, 0.0, 10.0, 10.0, 10.0], 1.0);
        let (verts, idx) = box_mesh([0.0, 0.0, 0.0], [10.0, 10.0, 8.0]);

        let result = verify_stock_against_mesh(&stock, &verts, &idx, &VerifyOptions::default());
        assert!(result.gouge.pass);
        assert!(!result.excess.pass, "unmachined top layer must be excess");
        assert!(
            result.excess.worst_depth > 1.0,
            "leftover layer is ~2mm thick, got {}",
            result.excess.worst_depth
        );
        assert!(!result.pass);
    }

    #[test]
    fn test_verification_serde_round_trip() {
        let mut stock = Stock::from_box([0.0, 0.0, 0.0, 10.0, 10.0, 10.0], 1.0);
        stock.subtract_sphere([5.0, 5.0, 5.0], 3.0);
        let (verts, idx) = box_mesh([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);

        let result = verify_stock_against_mesh(&stock, &verts, &idx, &VerifyOptions::default());
        let json = serde_json::to_string(&result).unwrap();
        let parsed: StockVerification = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.pass, result.pass);
        assert_eq!(parsed.gouge.violation_count, result.gouge.violation_count);
        assert_eq!(parsed.grid_samples, result.grid_samples);
    }
}
