//! Post-boolean validity oracle.
//!
//! Every failure in the 2026-08-11 hemispherical-socket handoff was
//! *silent*: the pipeline returned a closed, plausible-looking mesh of the
//! wrong solid. Structural checks (closed, oriented, positive volume)
//! cannot catch a no-op difference — the input IS a valid mesh — so the
//! oracle here is semantic: probe points are classified against the two
//! operand meshes, the boolean's set semantics predict whether each probe
//! must lie inside the result, and the result mesh is checked against that
//! prediction. Probes vote at seven nearby points and only confident,
//! unanimous probes count, so tessellation sag near surfaces cannot raise
//! false alarms.

use vcad_kernel_math::Point3;
use vcad_kernel_tessellate::TriangleMesh;

use crate::api::BooleanOp;
use crate::mesh::point_in_mesh;

/// Signed volume of a triangle mesh via the divergence theorem.
pub(crate) fn mesh_signed_volume(mesh: &TriangleMesh) -> f64 {
    let verts = &mesh.vertices;
    let mut vol = 0.0_f64;
    for tri in mesh.indices.chunks(3) {
        let i0 = tri[0] as usize * 3;
        let i1 = tri[1] as usize * 3;
        let i2 = tri[2] as usize * 3;
        let v0 = [verts[i0] as f64, verts[i0 + 1] as f64, verts[i0 + 2] as f64];
        let v1 = [verts[i1] as f64, verts[i1 + 1] as f64, verts[i1 + 2] as f64];
        let v2 = [verts[i2] as f64, verts[i2 + 1] as f64, verts[i2 + 2] as f64];
        vol += v0[0] * (v1[1] * v2[2] - v2[1] * v1[2]) - v1[0] * (v0[1] * v2[2] - v2[1] * v0[2])
            + v2[0] * (v0[1] * v1[2] - v1[1] * v0[2]);
    }
    vol / 6.0
}

fn mesh_aabb(mesh: &TriangleMesh) -> Option<([f64; 3], [f64; 3])> {
    if mesh.vertices.is_empty() {
        return None;
    }
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for v in mesh.vertices.chunks(3) {
        for k in 0..3 {
            min[k] = min[k].min(v[k] as f64);
            max[k] = max[k].max(v[k] as f64);
        }
    }
    Some((min, max))
}

/// Classify `p` against `mesh` with a 7-point unanimity vote: the point and
/// ±delta along each axis must agree, otherwise the probe abstains (near a
/// surface, where tessellation sag could flip parity).
fn confident_in(mesh: &TriangleMesh, p: &Point3, delta: f64) -> Option<bool> {
    if mesh.indices.is_empty() {
        return Some(false);
    }
    let first = point_in_mesh(p, mesh);
    let offsets = [
        [delta, 0.0, 0.0],
        [-delta, 0.0, 0.0],
        [0.0, delta, 0.0],
        [0.0, -delta, 0.0],
        [0.0, 0.0, delta],
        [0.0, 0.0, -delta],
    ];
    for o in offsets {
        let q = Point3::new(p.x + o[0], p.y + o[1], p.z + o[2]);
        if point_in_mesh(&q, mesh) != first {
            return None;
        }
    }
    Some(first)
}

/// Why a boolean result was rejected by the oracle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidityError {
    /// Result volume is negative: inverted orientation.
    NegativeVolume,
    /// A confident probe point disagreed with the boolean's set semantics
    /// (e.g. a point that must have been removed by a difference is still
    /// inside the result).
    SemanticMismatch {
        /// Probes whose predicted and actual containment disagreed.
        mismatches: usize,
        /// Probes where both prediction and result were confident.
        checked: usize,
    },
}

impl std::fmt::Display for ValidityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidityError::NegativeVolume => write!(f, "result volume is negative"),
            ValidityError::SemanticMismatch {
                mismatches,
                checked,
            } => write!(
                f,
                "result disagrees with boolean set semantics at {mismatches} of {checked} probe points"
            ),
        }
    }
}

/// Validate a boolean result mesh against the operands' meshes.
///
/// Returns `Ok(())` when the result has non-negative signed volume and is
/// semantically consistent with `op` at every confident probe point.
///
/// Closedness is deliberately NOT a rejection criterion: unpaired-edge
/// counts cannot separate legitimate results from garbage. A sound
/// thin-blade intersection tessellates with 3 hairline t-junction seams, a
/// sound sheet-metal fold union with 64 — while the perpendicular-cylinder
/// mis-cut scores 236. Every observed wrong-solid failure is caught by the
/// semantic probes (cracks large enough to matter distort parity), so
/// structure stays advisory.
pub(crate) fn validate_boolean_result(
    result: &TriangleMesh,
    mesh_a: &TriangleMesh,
    mesh_b: &TriangleMesh,
    op: BooleanOp,
) -> Result<(), ValidityError> {
    // Structural: orientation.
    let vol = mesh_signed_volume(result);
    if vol < -1e-6 {
        return Err(ValidityError::NegativeVolume);
    }

    // Semantic: probe-grid classification consistency. A cheap single-ray
    // test per (probe, mesh) decides the common all-consistent case; only
    // a probe that *appears* to mismatch pays for the full 7-point
    // unanimity vote on all three meshes, so tessellation sag near a
    // surface cannot raise a false alarm and a sound boolean costs
    // ~3·N³ ray casts.
    let (Some((min_a, max_a)), Some((min_b, max_b))) = (mesh_aabb(mesh_a), mesh_aabb(mesh_b))
    else {
        return Ok(());
    };
    let min = [
        min_a[0].min(min_b[0]),
        min_a[1].min(min_b[1]),
        min_a[2].min(min_b[2]),
    ];
    let max = [
        max_a[0].max(max_b[0]),
        max_a[1].max(max_b[1]),
        max_a[2].max(max_b[2]),
    ];
    let diag =
        ((max[0] - min[0]).powi(2) + (max[1] - min[1]).powi(2) + (max[2] - min[2]).powi(2)).sqrt();
    let delta = (diag * 0.005).clamp(1e-3, 1.0);
    let expect = |in_a: bool, in_b: bool| match op {
        BooleanOp::Union => in_a || in_b,
        BooleanOp::Difference => in_a && !in_b,
        BooleanOp::Intersection => in_a && in_b,
    };

    const N: usize = 6;
    let mut mismatches = 0usize;
    let mut checked = 0usize;
    for i in 0..N {
        for j in 0..N {
            for k in 0..N {
                let frac = |t: usize| (t as f64 + 0.5) / N as f64;
                let p = Point3::new(
                    min[0] + (max[0] - min[0]) * frac(i),
                    min[1] + (max[1] - min[1]) * frac(j),
                    min[2] + (max[2] - min[2]) * frac(k),
                );
                checked += 1;
                // Fast path: single ray per mesh.
                let quick_a = point_in_mesh(&p, mesh_a);
                let quick_b = point_in_mesh(&p, mesh_b);
                let quick_r = !result.indices.is_empty() && point_in_mesh(&p, result);
                if quick_r == expect(quick_a, quick_b) {
                    continue;
                }
                // Apparent mismatch: confirm with unanimous 7-point votes.
                let (Some(in_a), Some(in_b), Some(actual)) = (
                    confident_in(mesh_a, &p, delta),
                    confident_in(mesh_b, &p, delta),
                    confident_in(result, &p, delta),
                ) else {
                    continue; // near a surface — abstain
                };
                if actual != expect(in_a, in_b) {
                    mismatches += 1;
                }
            }
        }
    }
    if mismatches > 0 {
        return Err(ValidityError::SemanticMismatch {
            mismatches,
            checked,
        });
    }
    Ok(())
}
