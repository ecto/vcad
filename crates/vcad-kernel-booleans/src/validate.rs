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
use crate::mesh::MeshRayIndex;

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
    /// Undirected edges shared by MORE than two triangles.
    ///
    /// `open_edges` is a *net directed* count, so it cancels to zero on a
    /// doubled surface: two coincident patches with opposite winding are
    /// invisible to it. That is exactly the defect a slicer reports as
    /// "non-manifold edges", and auto-repair resolves it by filling —
    /// which closed a rotor's shaft bore on a real print. Count it
    /// separately: unlike an unpaired hairline seam, an over-used edge is
    /// never legitimate geometry.
    pub overused_edges: usize,
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
    let mut uses: std::collections::HashMap<([i64; 3], [i64; 3]), usize> =
        std::collections::HashMap::new();
    for t in 0..mesh.indices.len() / 3 {
        for k in 0..3 {
            let x = vkey(mesh.indices[t * 3 + k] as usize);
            let y = vkey(mesh.indices[t * 3 + (k + 1) % 3] as usize);
            if x == y {
                continue;
            }
            let e = if x < y { (x, y) } else { (y, x) };
            *uses.entry(e).or_default() += 1;
        }
    }

    MeshReport {
        signed_volume: mesh_signed_volume(mesh),
        open_edges: net.values().map(|n| n.unsigned_abs() as usize).sum(),
        triangles: mesh.indices.len() / 3,
        overused_edges: uses.values().filter(|&&n| n > 2).count(),
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

/// Probes used when testing whether two operands share interior volume.
const OVERLAP_PROBES: usize = 1728;

/// Fewest probes that must land strictly inside BOTH operands before a
/// no-op Difference is condemned.
///
/// This is a resolution floor, not a size threshold: at 32 hits the
/// estimator's own error is ~18%, so the 4× margin
/// [`NO_OP_REMOVAL_FRACTION`] demands cannot be manufactured by probe noise.
/// Below it the guard stays silent rather than judge a shared region it
/// cannot resolve.
const MIN_OVERLAP_PROBES: usize = 32;

/// Radical inverse of `i` in `base` — the van der Corput sequence.
///
/// The probes below deliberately avoid a regular lattice, which can
/// resonate with planar geometry: rows of a grid may fall wholly inside or
/// wholly outside a thin oblique slab, biasing its measured volume. A
/// Halton sequence has no lattice to resonate, so a thin feature keeps
/// honest √n error bars — measured on the 0.5 mm blade of
/// `torr_boolean_catalogue::b1`, which fills 7.5% of its own bounding box,
/// 1728 Halton probes estimate 143.7 mm³ against a true 146.3. It is
/// deterministic, so the verdict is reproducible run to run.
fn radical_inverse(mut i: usize, base: usize) -> f64 {
    let inv = 1.0 / base as f64;
    let mut f = inv;
    let mut r = 0.0;
    while i > 0 {
        r += (i % base) as f64 * f;
        i /= base;
        f *= inv;
    }
    r
}

/// Share of the demonstrated overlap a Difference must remove to count as
/// having cut at all. A correct cut removes ~100% of it; the 2026-08-11
/// boss-and-bore failure removed 0.6%.
const NO_OP_REMOVAL_FRACTION: f64 = 0.25;

/// How large the predicted removal must be, relative to the minuend, before
/// the guard is willing to judge at all.
///
/// `removed` is a difference of volumes taken from two meshes that
/// discretise the same curved surfaces on *different* schedules: a boolean
/// result's rims are sag-adaptive, a bare operand's are not. The systematic
/// gap between them is of the order of a chord deficit — 0.65% of the volume
/// for a 32-segment circle — and it can dwarf the quantity being measured.
/// Cutting a 147 mm³ blade out of an 82,000 mm³ cylinder measures as a
/// *negative* 382 mm³ removal for exactly this reason
/// (`torr_boolean_catalogue::b1_difference_dual`).
///
/// So the guard only judges wholesale removals, where the signal stands
/// clear of that noise. At 2% it sits 3× above the coarse-circle deficit,
/// while the failures it exists to catch predict removals of 16% (the
/// boss-and-bore bore) and 8% (the oblique-face bore) of their minuends.
/// The cost is honest and worth stating: a silently-skipped cut smaller
/// than 2% of the part is not caught here.
const MIN_PREDICTED_REMOVAL_FRACTION: f64 = 0.02;

/// Did a Difference leave its minuend essentially untouched while the two
/// operands demonstrably share interior volume?
///
/// This is the one wrong-solid mode the pipeline cannot see from the inside:
/// every stage reports success, the result mesh is closed with positive
/// volume, and the answer is (near enough) the minuend — the cut never
/// happened. Both 2026-08-11 field reports landed here (a bore breaking out
/// through the side face of a union'd boss, and a bore crossing a 45°
/// derived face), and in each the only symptom was the number.
///
/// Note it is *near enough*, not exactly zero: the boss-and-bore failure
/// still shaved 0.1% off the union, because the bore's wall was partly
/// present even though its interior was never removed. A strict
/// zero-removal test would have let that through, so the measure is
/// removal against the overlap the probes actually demonstrate.
///
/// Unlike the sampled oracles this module used to carry (see the header),
/// this one cannot false-positive on thin geometry, because it errs in the
/// only direction sampling is allowed to:
///
/// * a probe reports overlap only by landing strictly inside BOTH operands,
///   which a coarse grid can miss but cannot invent — so a thin cut the grid
///   cannot resolve predicts *less* removal, never more, and stays exempt.
///   That is exactly the population the old oracles tripped over (a 3.2 mm³
///   plate, a sheet-metal wall);
/// * the verdict needs the removal to fall a full 4× short of a prediction
///   built from at least [`MIN_OVERLAP_PROBES`] hits, a gap wider than the
///   grid's own sampling error at that count.
///
/// The grid spans the *overlap* of the two bounding boxes, not the whole
/// scene, so its cells are as small as the shared region itself.
pub(crate) fn difference_removed_nothing(
    result: &TriangleMesh,
    mesh_a: &TriangleMesh,
    mesh_b: &TriangleMesh,
) -> bool {
    let (Some((min_a, max_a)), Some((min_b, max_b))) = (mesh_aabb(mesh_a), mesh_aabb(mesh_b))
    else {
        return false;
    };
    let vol_a = mesh_signed_volume(mesh_a).abs();
    if !vol_a.is_finite() || vol_a <= 0.0 {
        return false;
    }
    let removed = vol_a - mesh_signed_volume(result).abs();
    if !removed.is_finite() {
        return false;
    }

    // Overlap box — `boolean_op` has already established the AABBs meet.
    let mut min = [0.0f64; 3];
    let mut span = [0.0f64; 3];
    for k in 0..3 {
        min[k] = min_a[k].max(min_b[k]);
        let hi = max_a[k].min(max_b[k]);
        span[k] = hi - min[k];
        if !span[k].is_finite() || span[k] <= 0.0 {
            return false;
        }
    }
    let box_vol = span[0] * span[1] * span[2];
    if !box_vol.is_finite() || box_vol <= 0.0 {
        return false;
    }

    // Thousands of probes against the same pair of meshes: index them
    // once instead of re-scanning every triangle per probe. The index
    // answers exactly what `point_in_mesh` does (see `MeshRayIndex`).
    let idx_a = MeshRayIndex::new(mesh_a);
    let idx_b = MeshRayIndex::new(mesh_b);

    let n = OVERLAP_PROBES;
    let mut both = 0usize;
    for i in 0..n {
        // Halton, bases 2/3/5 — see `radical_inverse`.
        let p = Point3::new(
            min[0] + span[0] * radical_inverse(i + 1, 2),
            min[1] + span[1] * radical_inverse(i + 1, 3),
            min[2] + span[2] * radical_inverse(i + 1, 5),
        );
        if idx_a.contains(&p) && idx_b.contains(&p) {
            both += 1;
        }
    }
    if both < MIN_OVERLAP_PROBES {
        // Shared region too small to resolve — stay silent rather than guess.
        return false;
    }
    let predicted = box_vol * both as f64 / n as f64;
    // Under the boolean diagnostics flag, show the numbers behind the
    // verdict. This is what identified `torture::chain-17` step 2 as an
    // EXACT no-op (removed 0.000 against a 1068 mm³ overlap resolved by
    // 1031 of 1728 probes) rather than a marginal cut.
    if std::env::var_os("VCAD_BOOLEAN_WARN").is_some() {
        eprintln!(
            "vcad boolean: no-op probe — minuend {vol_a:.3}, removed {removed:.3}, \
             overlap {both}/{n} probes ≈ {predicted:.3}, floor {:.3}",
            MIN_PREDICTED_REMOVAL_FRACTION * vol_a
        );
    }
    if predicted < MIN_PREDICTED_REMOVAL_FRACTION * vol_a {
        // Below the tessellation noise floor — not judgeable. See
        // `MIN_PREDICTED_REMOVAL_FRACTION`.
        return false;
    }
    removed < NO_OP_REMOVAL_FRACTION * predicted
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

    let idx_a = MeshRayIndex::new(mesh_a);
    let idx_b = MeshRayIndex::new(mesh_b);

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
                if expect(idx_a.contains(&p), idx_b.contains(&p)) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_math::Transform;
    use vcad_kernel_primitives::{make_cube, BRepSolid};
    use vcad_kernel_tessellate::tessellate_brep;

    const SEGMENTS: u32 = 32;

    fn translated_cube(s: f64, dx: f64, dy: f64, dz: f64) -> BRepSolid {
        let mut c = make_cube(s, s, s);
        let t = Transform::translation(dx, dy, dz);
        for (_, v) in &mut c.topology.vertices {
            v.point = t.apply_point(&v.point);
        }
        c.geometry.surfaces = c
            .geometry
            .surfaces
            .drain(..)
            .map(|s| s.transform(&t))
            .collect();
        c
    }

    /// A Difference that returned its minuend untouched, against a
    /// subtrahend sitting wholly inside it, is unambiguously wrong.
    #[test]
    fn no_op_difference_against_a_contained_tool_is_caught() {
        let a = make_cube(20.0, 20.0, 20.0);
        let b = translated_cube(10.0, 5.0, 5.0, 5.0);
        let mesh_a = tessellate_brep(&a, SEGMENTS);
        let mesh_b = tessellate_brep(&b, SEGMENTS);
        // The wrong answer the guard exists to catch: the minuend, verbatim.
        assert!(difference_removed_nothing(&mesh_a, &mesh_a, &mesh_b));
    }

    /// The same operands cut correctly must not trip it.
    #[test]
    fn a_correct_difference_is_not_flagged() {
        let a = make_cube(20.0, 20.0, 20.0);
        let b = translated_cube(10.0, 5.0, 5.0, 5.0);
        let mesh_a = tessellate_brep(&a, SEGMENTS);
        let mesh_b = tessellate_brep(&b, SEGMENTS);
        let result = crate::boolean_op(&a, &b, BooleanOp::Difference, SEGMENTS)
            .expect("difference should succeed")
            .to_mesh(SEGMENTS);
        assert!(!difference_removed_nothing(&result, &mesh_a, &mesh_b));
    }

    /// Operands whose bounding boxes meet but whose solids do not share
    /// interior volume: the Difference correctly removes nothing, and the
    /// guard must stay silent. This is the case a naive "bboxes overlap but
    /// nothing was removed" rule would condemn.
    #[test]
    fn touching_but_disjoint_operands_are_not_flagged() {
        let a = make_cube(20.0, 20.0, 20.0);
        // Shares only the x = 20 plane — zero shared interior.
        let b = translated_cube(20.0, 20.0, 0.0, 0.0);
        let mesh_a = tessellate_brep(&a, SEGMENTS);
        let mesh_b = tessellate_brep(&b, SEGMENTS);
        assert!(!difference_removed_nothing(&mesh_a, &mesh_a, &mesh_b));
    }
}
