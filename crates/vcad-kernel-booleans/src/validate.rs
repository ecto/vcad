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
    /// separately. Like `open_edges` it is advisory — known-good results
    /// in this crate's catalogue score up to 13 — but it is the only
    /// number that sees this class at all.
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

/// Slack on the union volume bound, as a fraction of `vol(A) + vol(B)`.
///
/// The bound itself is exact; the slack absorbs tessellation schedules. A
/// result's rims are re-sampled sag-adaptively, so its curved faces can read a
/// fraction of a percent larger than the operands' inscribed polygons (0.64%
/// at 32 segments). 2% keeps every such difference out and still catches the
/// failures this guards against, which are gross: the rana-60 stator's tangent
/// fillet blocks read +46% and −46%.
const UNION_VOLUME_SLACK: f64 = 0.02;

/// Sound post-hoc check for a Union: `max(vol A, vol B) ≤ vol(A ∪ B) ≤ vol A +
/// vol B`, always. Unlike the sampled oracles this module tried and removed,
/// the bound is a theorem, not an estimate — thin features cannot fool it —
/// so a result outside it (beyond tessellation slack) is definitely the wrong
/// solid.
///
/// Measured on the rana-60 stator: a ring, a tab and fillet blocks whose cut
/// cylinders are tangent to the ring's face. The sixth union returned a
/// 7314 mm³ "analytic" solid from operands totalling 5006 mm³ (298
/// non-manifold edges), and the whole 50-stage part came out at a third of
/// its true volume while reporting `Analytic` throughout.
pub(crate) fn union_volume_out_of_bounds(
    result: &TriangleMesh,
    mesh_a: &TriangleMesh,
    mesh_b: &TriangleMesh,
) -> bool {
    let (va, vb, vr) = (
        mesh_signed_volume(mesh_a),
        mesh_signed_volume(mesh_b),
        mesh_signed_volume(result),
    );
    // An operand that does not read as a positive solid cannot bound anything.
    if !(va.is_finite() && vb.is_finite() && vr.is_finite()) || va <= 0.0 || vb <= 0.0 {
        return false;
    }
    let slack = UNION_VOLUME_SLACK * (va + vb);
    vr > va + vb + slack || vr < va.max(vb) - slack
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

/// A retained face that is not on the result's boundary: the trim that should
/// have removed it did not happen.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BuriedFace {
    /// A sample point on the face that is buried.
    pub at: Point3,
    /// How deep the sample stayed inside the other operand, mm — the largest
    /// nudge at which parity still said "inside".
    pub depth: f64,
}

/// Depths at which a sample is pushed out along its face's normal.
///
/// A face that merely TOUCHES the other operand is normal and correct — every
/// coplanar contact does it — so parity at a point on the surface says
/// nothing. The sample has to still read "inside" at every step; a grazing
/// face stops at the first. The last step is where a tessellated curved
/// neighbour can still legitimately bulge past a planar face of the other
/// operand, so it is the deepest that proves anything.
const BURIED_STEPS: [f64; 3] = [1e-3, 4e-3, 1.2e-2];

/// Distance a sample must keep from its face's boundary, mm.
///
/// Slivers must not vote. A sample near an edge sits in the tolerance band
/// where the two operands' surfaces are indistinguishable, and would report
/// every legitimate coplanar seam as buried. A face with no point this far
/// inside it is a sliver and is skipped: its area is too small to carry the
/// kind of error this check exists for.
const SAMPLE_INSET: f64 = 5e-3;

/// How far a sample must be from an operand's surface before "inside" means
/// buried rather than touching, mm.
///
/// Sized above the chordal sag the operand meshes carry at the radii this
/// kernel works at (a 256-segment rim at r 28.75 sags 2.2e-3 mm) so a face
/// resting on a tessellated curve is never mistaken for one inside it, and
/// well below the depth at which a missing trim shows up — the near-miss
/// fillet's surviving stretch reads 1.2e-2 mm deep.
const CONTACT_TOL: f64 = 6e-3;

/// Does the result keep a face that is not on its boundary?
///
/// The failure this catches is a MISSING TRIM: a stretch of one operand's
/// face that should have been cut away survives inside the other, so the
/// result encloses that material twice and its volume comes out high — while
/// every face is analytic, the shell looks fine, and the volume bound
/// (`max(A,B) <= vol <= A+B`) is far too loose to notice. Measured: a fillet
/// block drawn 0.01 mm off tangency gives a union of 4734.78 mm3 against a
/// closed form of 4726.91, +0.167%, reported `Analytic`.
///
/// Unpaired-edge counts cannot separate that from a healthy result — the
/// rana-60 stator carries 642 of them as doubled slivers of negligible area
/// and is correct to 0.06%, while this case has 6 and is 7.9 mm3 wrong. What
/// separates them is set semantics: after a union, no point just outside a
/// retained face may lie inside either operand; after a difference, the
/// material just outside a retained face is the removed material, which lies
/// inside BOTH.
///
/// Containment is judged against the OPERAND meshes, which are valid solids,
/// by the same three-ray parity vote the mesh boolean trusts — parity is
/// robust on t-junction soup where BSP leaf classification is not.
pub(crate) fn buried_retained_face(
    result: &vcad_kernel_primitives::BRepSolid,
    mesh_a: &TriangleMesh,
    mesh_b: &TriangleMesh,
    op: BooleanOp,
) -> Option<BuriedFace> {
    if matches!(op, BooleanOp::Intersection) {
        // An intersection's faces are bounded by BOTH operands, so "just
        // outside a retained face" is inside neither by construction and the
        // test degenerates. Left to the other gates.
        return None;
    }
    if mesh_a.indices.is_empty() || mesh_b.indices.is_empty() {
        return None;
    }
    let member_a = crate::mesh::csg::Membership::new(mesh_a);
    let member_b = crate::mesh::csg::Membership::new(mesh_b);
    let (index_a, index_b) = (member_a.index(), member_b.index());
    // Parity alone cannot tell a face that LIES ON the other operand from one
    // strictly inside it, and every sound boolean produces the first: a
    // coplanar contact, a tangent fillet, two rings stacked face to face. All
    // of them read "inside" on one side and "inside" on the other, because
    // both sides are material. Distance separates them — a contact face is
    // ON the other operand's surface, a buried one is `CONTACT_TOL` away from
    // it — and this is the single guard that took the check from seven false
    // positives on known-good results to none.
    let (Some(dist_a), Some(dist_b)) = (
        vcad_kernel_tessellate::MeshDistance::new(mesh_a),
        vcad_kernel_tessellate::MeshDistance::new(mesh_b),
    ) else {
        return None;
    };

    for (face_id, face) in &result.topology.faces {
        let loop_pts: Vec<Point3> = result
            .topology
            .loop_half_edges(face.outer_loop)
            .map(|he| result.topology.vertices[result.topology.half_edges[he].origin].point)
            .collect();
        if loop_pts.len() < 3 {
            continue;
        }
        let Some(normal) = newell_normal(&loop_pts) else {
            continue;
        };
        // Orientation-free on purpose. Whether a loop's winding encodes the
        // outward side is exactly the sort of assumption that makes a
        // validity oracle report every second face, so the test does not use
        // it: a face ON the boundary has result-material on one side and void
        // on the other, and a BURIED face has it on both. That is the whole
        // invariant, and it needs no normal direction, only a normal line.
        let inside = |p: &Point3| -> bool {
            let (a, b) = (
                crate::mesh::csg::contains(&index_a, p),
                crate::mesh::csg::contains(&index_b, p),
            );
            match op {
                BooleanOp::Union => a || b,
                BooleanOp::Difference => a && !b,
                BooleanOp::Intersection => a && b,
            }
        };

        for sample in interior_samples(result, face_id, &loop_pts) {
            // The sample sits on its own operand. To be BURIED it has to be
            // well inside the other one, not resting against it.
            let at = [sample.x, sample.y, sample.z];
            let clear_of_contact = dist_a.distance(at).max(dist_b.distance(at)) > CONTACT_TOL;
            if !clear_of_contact {
                continue;
            }
            let mut depth = 0.0;
            let mut buried = true;
            for &step in &BURIED_STEPS {
                buried &= inside(&(sample + normal * step)) && inside(&(sample - normal * step));
                if !buried {
                    break;
                }
                depth = step;
            }
            if buried && std::env::var_os("VCAD_BURIED_DEBUG").is_some() {
                let probe = |d: f64| {
                    let q = sample + normal * d;
                    (
                        crate::mesh::csg::contains(&index_a, &q),
                        crate::mesh::csg::contains(&index_b, &q),
                    )
                };
                eprintln!(
                    "buried {face_id:?}: sample ({:.5},{:.5},{:.5}) n ({:.3},{:.3},{:.3}) \
                     dA {:.5} dB {:.5} | +1e-3 {:?} -1e-3 {:?} | +4e-3 {:?} -4e-3 {:?} \
                     | +1.2e-2 {:?} -1.2e-2 {:?}",
                    sample.x,
                    sample.y,
                    sample.z,
                    normal.x,
                    normal.y,
                    normal.z,
                    dist_a.distance(at),
                    dist_b.distance(at),
                    probe(1e-3),
                    probe(-1e-3),
                    probe(4e-3),
                    probe(-4e-3),
                    probe(1.2e-2),
                    probe(-1.2e-2),
                );
            }
            if buried {
                return Some(BuriedFace { at: sample, depth });
            }
        }
    }
    None
}

/// Newell normal of a polygon, `None` when degenerate.
fn newell_normal(pts: &[Point3]) -> Option<vcad_kernel_math::Vec3> {
    let mut n = vcad_kernel_math::Vec3::zeros();
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        n += (a - Point3::origin()).cross(b - Point3::origin());
    }
    (n.norm() > 1e-12).then(|| n.normalize())
}

/// A few points strictly inside a face AND on its surface, each at least
/// [`SAMPLE_INSET`] from the boundary. Empty for a sliver.
///
/// Built in the surface's own parameter space, not in 3D. The obvious
/// construction — centroids of a fan over the loop vertices — puts the sample
/// on a CHORD, which for a cylinder of any size is deep inside the solid: a
/// r 46 wall sampled that way reported its point 0.42 mm off its own surface,
/// read "material on both sides", and every such wall in the corpus came back
/// buried. Averaging the vertices' UVs and evaluating the surface there lands
/// on the surface by construction, for planes and quadrics alike.
fn interior_samples(
    brep: &vcad_kernel_primitives::BRepSolid,
    face_id: vcad_kernel_topo::FaceId,
    pts: &[Point3],
) -> Vec<Point3> {
    let face = &brep.topology.faces[face_id];
    let surface = brep.geometry.surfaces[face.surface_index].as_ref();
    let mut uvs: Vec<vcad_kernel_math::Point2> = pts
        .iter()
        .map(|p| crate::trim::project_point_to_uv(surface, p))
        .collect();
    if uvs.len() < 3 {
        return Vec::new();
    }
    // A periodic surface's loop can straddle the u seam, where averaging is
    // meaningless. Unwrap onto one branch; a face that still spans more than
    // half the period after that wraps the whole way round and has no
    // meaningful parameter centroid, so it is skipped.
    let period = std::f64::consts::TAU;
    let u0 = uvs[0].x;
    for uv in &mut uvs {
        while uv.x - u0 > period * 0.5 {
            uv.x -= period;
        }
        while u0 - uv.x > period * 0.5 {
            uv.x += period;
        }
    }
    let (umin, umax) = uvs.iter().fold((f64::MAX, f64::MIN), |(lo, hi), uv| {
        (lo.min(uv.x), hi.max(uv.x))
    });
    if umax - umin > period * 0.5 {
        return Vec::new();
    }

    let n = uvs.len();
    let centre = vcad_kernel_math::Point2::new(
        uvs.iter().map(|uv| uv.x).sum::<f64>() / n as f64,
        uvs.iter().map(|uv| uv.y).sum::<f64>() / n as f64,
    );
    let mut out = Vec::new();
    let stride = n.div_ceil(6).max(1);
    for i in (0..n).step_by(stride) {
        // Between the centre and a vertex, in parameter space: interior for
        // any face whose parameter domain is star-shaped about its centroid,
        // and checked against `point_in_face` for the ones that are not.
        for pull in [0.5f64, 0.25, 0.75] {
            let uv = vcad_kernel_math::Point2::new(
                centre.x + (uvs[i].x - centre.x) * pull,
                centre.y + (uvs[i].y - centre.y) * pull,
            );
            let p = surface.evaluate(uv);
            if !crate::trim::point_in_face(brep, face_id, &p) {
                continue;
            }
            let clear = (0..pts.len()).all(|k| {
                let (u, v) = (pts[k], pts[(k + 1) % pts.len()]);
                let uvv = v - u;
                let len2 = uvv.norm_squared();
                let t = if len2 < 1e-18 {
                    0.0
                } else {
                    ((p - u).dot(uvv) / len2).clamp(0.0, 1.0)
                };
                (p - (u + uvv * t)).norm() > SAMPLE_INSET
            });
            if clear {
                out.push(p);
                break;
            }
        }
    }
    out
}
