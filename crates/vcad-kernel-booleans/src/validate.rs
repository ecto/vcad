//! Post-boolean validity checks.
//!
//! Every failure in the 2026-08-11 hemispherical-socket handoff was
//! *silent*: the pipeline returned a closed, plausible-looking mesh of the
//! wrong solid. Structural checks cannot catch that — a no-op difference
//! returns a perfectly valid mesh of the wrong solid.
//!
//! Two sampled oracles were built and measured here before being removed:
//! one comparing per-probe set membership, one integrating the same probes
//! into a predicted volume. Both false-positive on legitimate geometry,
//! because CAD parts routinely carry features thinner than any affordable
//! sample spacing — a 3.2 mm³ thin plate reads as zero predicted volume on
//! a 12³ grid, and a *correct* result was measured disagreeing at 4 of 216
//! probes while a result 32% short of the truth disagreed at 7. Sampling
//! cannot separate those populations.
//!
//! So the guard against wrong solids lives in `boolean_op` instead, as an
//! up-front capability declaration: arrangements the splitters provably
//! cannot represent are routed to the mesh fallback before the B-rep
//! pipeline ever runs. What remains here is the one check that is sound
//! post hoc — signed volume — plus an advisory structural report.

use vcad_kernel_math::Point3;
use vcad_kernel_tessellate::TriangleMesh;

use crate::api::BooleanOp;
use crate::mesh::point_in_mesh;

/// Signed volume of a triangle mesh via the divergence theorem.
pub fn mesh_signed_volume(mesh: &TriangleMesh) -> f64 {
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

/// Why a boolean result was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidityError {
    /// Signed volume is negative (inverted orientation) or not finite.
    ///
    /// This is the only *sound* post-hoc check available. Sampled
    /// set-semantics oracles were tried and removed: thin features live
    /// below any affordable grid resolution, so they false-positive on
    /// legitimate geometry (a 3.2 mm³ thin plate reads as 0 predicted
    /// volume on a 12³ grid), and routing correct results into the coarse
    /// fallback is itself a regression. Unrepresentable arrangements are
    /// instead declared up front — see `boolean_op`.
    BadVolume,
}

impl std::fmt::Display for ValidityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidityError::BadVolume => {
                write!(f, "result solid has negative or non-finite volume")
            }
        }
    }
}

/// Structural report on a result mesh. Advisory only — see
/// [`validate_boolean_result`] for why closedness cannot gate a boolean.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshReport {
    /// Signed volume via the divergence theorem.
    pub signed_volume: f64,
    /// Directed edges without an opposite-direction partner. Zero for a
    /// closed, consistently oriented surface.
    pub open_edges: usize,
    /// Triangle count.
    pub triangles: usize,
}

/// Measure a result mesh: signed volume, unpaired directed edges, triangles.
///
/// Exposed for diagnostics and tests. Callers must not treat `open_edges > 0`
/// as "wrong solid": measured on known-good results, a thin-blade
/// intersection scores 3 and a sheet-metal fold union 64, while a
/// known-bad perpendicular-cylinder cut scores 236 — the populations
/// overlap, so no threshold separates them.
pub fn mesh_report(mesh: &TriangleMesh) -> MeshReport {
    let quantum = 1e-5;
    let vkey = |vi: usize| -> [i64; 3] {
        [
            (mesh.vertices[vi * 3] as f64 / quantum).round() as i64,
            (mesh.vertices[vi * 3 + 1] as f64 / quantum).round() as i64,
            (mesh.vertices[vi * 3 + 2] as f64 / quantum).round() as i64,
        ]
    };
    let mut net: std::collections::HashMap<([i64; 3], [i64; 3]), i64> =
        std::collections::HashMap::new();
    for t in 0..mesh.indices.len() / 3 {
        for k in 0..3 {
            let x = vkey(mesh.indices[t * 3 + k] as usize);
            let y = vkey(mesh.indices[t * 3 + (k + 1) % 3] as usize);
            if x == y {
                continue;
            }
            if x < y {
                *net.entry((x, y)).or_default() += 1;
            } else {
                *net.entry((y, x)).or_default() -= 1;
            }
        }
    }
    MeshReport {
        signed_volume: mesh_signed_volume(mesh),
        open_edges: net.values().map(|n| n.unsigned_abs() as usize).sum(),
        triangles: mesh.indices.len() / 3,
    }
}

/// Sound post-boolean check: reject only what is unambiguously invalid.
///
/// A bounded solid — outer shells minus enclosed voids — always has
/// positive signed volume, so a negative or non-finite total is a
/// definite defect. Nothing else is checked here, deliberately: both a
/// probe-agreement oracle and a sampled-volume oracle were implemented and
/// measured against this crate's own suites and the torture corpus, and
/// both false-positived on legitimate thin and sliver geometry. Guarding
/// against wrong solids is done by declaring unrepresentable arrangements
/// before the fact rather than sampling after it.
pub(crate) fn validate_boolean_result(result: &TriangleMesh) -> Result<(), ValidityError> {
    if result.indices.is_empty() {
        return Ok(());
    }
    let v = mesh_signed_volume(result);
    if !v.is_finite() || v < -1e-6 {
        return Err(ValidityError::BadVolume);
    }
    Ok(())
}

/// Probes per axis when estimating a boolean's expected volume.
const PROBES_PER_AXIS: usize = 12;

/// Relative volume discrepancy that counts as a wrong solid.
///
/// Chosen from measured separation, not tuned: on flagged arrangements a
/// correct-but-marginal B-rep result lands at 2.3% of the sampled
/// prediction, while genuine wrong solids land at 19% (a cross-drill whose
/// tool surface was merged into the bar instead of removed), 52% (a pocket
/// that never appeared) and 100% (an emptied result).
const VOLUME_RTOL: f64 = 0.10;

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

/// Does `result` disagree grossly with the volume the operation's set
/// semantics predict from the operands?
///
/// **Only sound on arrangements already flagged as unrepresentable.** The
/// prediction comes from a stratified point grid, which cannot resolve
/// features thinner than its cell size — a 3.2 mm³ sheet-metal plate reads
/// as zero predicted volume on a 12³ grid — so applying this to every
/// boolean false-positives on legitimate thin geometry and routes correct
/// results into the coarse fallback. Restricted to flagged arrangements it
/// is decisive, because those are curved-surface crossings whose failures
/// are wholesale (an entire pocket missing) rather than sliver-scale.
pub(crate) fn volume_disagrees_grossly(
    result: &TriangleMesh,
    mesh_a: &TriangleMesh,
    mesh_b: &TriangleMesh,
    op: BooleanOp,
) -> bool {
    let (Some((min_a, max_a)), Some((min_b, max_b))) = (mesh_aabb(mesh_a), mesh_aabb(mesh_b))
    else {
        return false;
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
    let span = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
    let box_vol = span[0] * span[1] * span[2];
    if !box_vol.is_finite() || box_vol <= 0.0 {
        return false;
    }

    let expect = |in_a: bool, in_b: bool| match op {
        BooleanOp::Union => in_a || in_b,
        BooleanOp::Difference => in_a && !in_b,
        BooleanOp::Intersection => in_a && in_b,
    };

    let n = PROBES_PER_AXIS;
    let mut inside = 0usize;
    for i in 0..n {
        for j in 0..n {
            for k in 0..n {
                let frac = |t: usize| (t as f64 + 0.5) / n as f64;
                let p = Point3::new(
                    min[0] + span[0] * frac(i),
                    min[1] + span[1] * frac(j),
                    min[2] + span[2] * frac(k),
                );
                if expect(point_in_mesh(&p, mesh_a), point_in_mesh(&p, mesh_b)) {
                    inside += 1;
                }
            }
        }
    }
    let predicted = box_vol * inside as f64 / (n * n * n) as f64;
    let got = mesh_signed_volume(result).abs();
    let cell = box_vol / (n * n * n) as f64;
    // Absolute slack of a few cells: the estimator cannot resolve a
    // prediction finer than its own cell, so tiny results must not trip on
    // quantisation alone.
    // Measure against the prediction, not against whichever value is
    // larger: a result that is wrong by *gaining* the tool's volume would
    // otherwise widen its own tolerance and slip through.
    let slack = VOLUME_RTOL * predicted + 3.0 * cell;
    (got - predicted).abs() > slack
}
