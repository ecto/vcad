//! Public API types and entry point for boolean operations.

use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::{tessellate_brep, TriangleMesh};

use crate::bbox;
use crate::cyl_cyl;
use crate::pipeline::{brep_boolean, non_overlapping_boolean};
use crate::ssi::SsiError;
use crate::validate::{validate_boolean_result, ValidityError};

/// Error from a CSG boolean operation.
///
/// Boolean failures used to be panics deep in the pipeline; in the browser
/// a panic poisons the WASM instance for the rest of the session. They now
/// propagate as values through the 4-stage pipeline (AABB filter → SSI →
/// classification → sewing) so `vcad-kernel-wasm` can surface a clean JS
/// error instead of trapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BooleanError {
    /// Surface-surface intersection failed for a candidate face pair.
    Ssi(SsiError),
    /// The mesh-CSG fallback produced a solid with negative or non-finite
    /// volume. Returned instead of a plausible-looking wrong solid.
    InvalidResult(ValidityError),
}

impl std::fmt::Display for BooleanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BooleanError::Ssi(e) => write!(f, "boolean operation failed: {e}"),
            BooleanError::InvalidResult(e) => {
                write!(f, "boolean operation produced an invalid solid: {e}")
            }
        }
    }
}

impl std::error::Error for BooleanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BooleanError::Ssi(e) => Some(e),
            BooleanError::InvalidResult(_) => None,
        }
    }
}

impl From<SsiError> for BooleanError {
    fn from(e: SsiError) -> Self {
        BooleanError::Ssi(e)
    }
}

/// CSG boolean operation type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BooleanOp {
    /// Union: combine both solids.
    Union,
    /// Difference: subtract the tool from the target.
    Difference,
    /// Intersection: keep only the overlapping region.
    Intersection,
}

/// Result of a boolean operation.
///
/// Always a B-rep solid. Cases that previously fell back to a mesh-only
/// result (e.g. the perpendicular equal-radius cylinder Steinmetz path) now
/// reconstruct topology from the tessellated mesh so that downstream
/// features that depend on B-rep (DFM, STEP export, direct ray tracing,
/// fillets, drafting projections) continue to work.
#[derive(Debug, Clone)]
pub enum BooleanResult {
    /// B-rep result.
    BRep(Box<BRepSolid>),
}

impl BooleanResult {
    /// Get the triangle mesh by tessellating the B-rep.
    pub fn to_mesh(&self, segments: u32) -> TriangleMesh {
        match self {
            BooleanResult::BRep(brep) => tessellate_brep(brep.as_ref(), segments),
        }
    }

    /// Get a reference to the BRep solid.
    pub fn as_brep(&self) -> Option<&BRepSolid> {
        match self {
            BooleanResult::BRep(brep) => Some(brep.as_ref()),
        }
    }

    /// Convert to BRepSolid, consuming self.
    pub fn into_brep(self) -> Option<BRepSolid> {
        match self {
            BooleanResult::BRep(brep) => Some(*brep),
        }
    }
}

/// How faithfully a boolean result is carried by analytic surfaces.
///
/// [`BooleanResult`] is *always* a `BRep`, so "is it a B-rep?" no longer
/// separates a real result from a degraded one. Every mesh fallback is
/// re-wrapped as a triangle-soup B-rep by [`crate::mesh::mesh_to_brep`] —
/// topologically valid, but every face is a one-triangle `Plane`. Such a
/// solid still passes `Solid::can_export_step()` and still exports STEP;
/// it just exports thousands of facets instead of the cylinder, sphere or
/// cone the model was authored from. This enum is the signal that
/// distinguishes the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fidelity {
    /// The B-rep pipeline produced the result; faces carry their analytic
    /// surfaces (Plane, Cylinder, Cone, Sphere, Torus, NURBS).
    Analytic,
    /// The result came from the mesh-CSG fallback and was re-wrapped as a
    /// triangle-soup B-rep. Analytic surfaces are lost.
    TriangleSoup,
}

impl Fidelity {
    /// Stable identifier, for reports and serialization.
    pub fn as_str(self) -> &'static str {
        match self {
            Fidelity::Analytic => "analytic",
            Fidelity::TriangleSoup => "triangle-soup",
        }
    }
}

/// Why a boolean dropped from analytic surfaces to triangle soup.
///
/// One variant per fallback site in [`boolean_op_reported`]. These are not
/// errors — each one produced a *usable* solid — but each one is a point
/// where the model stopped being a B-rep in any useful sense, and a caller
/// restructuring a design to avoid the loss needs to know which fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DegradeReason {
    /// Two perpendicular, intersecting, equal-radius cylinders (the
    /// Steinmetz cross-shaft). The cylindrical-face splitter cannot
    /// decompose the figure-8 SSI boundary, so a specialised mesh path
    /// emits the result directly.
    SteinmetzCylinders,
    /// An operand was *already* triangle soup (a prior fallback, chained).
    /// The B-rep pipeline is skipped entirely — it gains nothing from
    /// anonymous triangles and its face-pair stages scale quadratically.
    /// Degradation is therefore contagious: one fallback poisons every
    /// downstream boolean on that branch.
    SoupOperand,
    /// The B-rep result had negative or non-finite volume — provably wrong,
    /// since a bounded solid always has positive volume.
    InvertedVolume,
    /// The arrangement was flagged unrepresentable *and* the B-rep result's
    /// volume disagreed grossly with what the operands imply.
    VolumeDisagreement,
    /// A Difference removed no volume at all from a subtrahend it
    /// demonstrably overlaps — a silently skipped cut.
    DifferenceRemovedNothing,
    /// A spherical face needed splitting by intersecting circles, which the
    /// cap splitter cannot partition. The pipeline reports this from the
    /// inside rather than returning the uncut operand.
    SphereArrangement,
    /// The B-rep result was sound but *cracked* (open edges) on an
    /// arrangement already declared unrepresentable, and the mesh fallback
    /// was watertight and agreed on volume. Analytic surfaces were traded
    /// for watertightness.
    WatertightnessSwap,
}

impl DegradeReason {
    /// Stable identifier, for reports and serialization.
    pub fn as_str(self) -> &'static str {
        match self {
            DegradeReason::SteinmetzCylinders => "steinmetz-cylinders",
            DegradeReason::SoupOperand => "soup-operand",
            DegradeReason::InvertedVolume => "inverted-volume",
            DegradeReason::VolumeDisagreement => "volume-disagreement",
            DegradeReason::DifferenceRemovedNothing => "difference-removed-nothing",
            DegradeReason::SphereArrangement => "sphere-arrangement",
            DegradeReason::WatertightnessSwap => "watertightness-swap",
        }
    }
}

impl std::fmt::Display for DegradeReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            DegradeReason::SteinmetzCylinders => {
                "perpendicular equal-radius cylinders (Steinmetz) take the specialised mesh path"
            }
            DegradeReason::SoupOperand => {
                "an operand was already triangle soup from an earlier fallback"
            }
            DegradeReason::InvertedVolume => "the B-rep result had negative or non-finite volume",
            DegradeReason::VolumeDisagreement => {
                "the B-rep result's volume disagreed grossly with the operands"
            }
            DegradeReason::DifferenceRemovedNothing => {
                "the difference removed no volume from an overlapping subtrahend"
            }
            DegradeReason::SphereArrangement => {
                "a spherical face needed splitting by intersecting circles"
            }
            DegradeReason::WatertightnessSwap => {
                "the B-rep result was cracked and the watertight mesh result was taken instead"
            }
        };
        write!(f, "{msg}")
    }
}

/// What a single boolean operation did to the representation.
///
/// Returned alongside the result by [`boolean_op_reported`]. `boolean_op`
/// discards it, which is why degradation used to be invisible until STEP
/// export several operations later.
#[derive(Debug, Clone)]
pub struct BooleanReport {
    /// The operation performed.
    pub op: BooleanOp,
    /// Whether the result kept its analytic surfaces.
    pub fidelity: Fidelity,
    /// Which fallback fired, when `fidelity` is `TriangleSoup`.
    pub reason: Option<DegradeReason>,
    /// The arrangement was flagged as one the splitters provably cannot
    /// represent. This is a *capability declaration*, not a verdict — a
    /// flagged arrangement often still comes out analytic and correct.
    pub flagged_unrepresentable: bool,
    /// Unpaired directed edges in the result's tessellation. Advisory
    /// only: known-good results score 3 and 64, so no threshold separates
    /// good from bad (see [`crate::mesh_report`]).
    pub open_edges: usize,
    /// Face count of the result B-rep. A four-digit count with
    /// `fidelity == TriangleSoup` is the signature of soup.
    pub faces: usize,
}

impl BooleanReport {
    fn analytic(op: BooleanOp) -> Self {
        Self {
            op,
            fidelity: Fidelity::Analytic,
            reason: None,
            flagged_unrepresentable: false,
            open_edges: 0,
            faces: 0,
        }
    }

    fn degraded(op: BooleanOp, reason: DegradeReason) -> Self {
        Self {
            op,
            fidelity: Fidelity::TriangleSoup,
            reason: Some(reason),
            flagged_unrepresentable: false,
            open_edges: 0,
            faces: 0,
        }
    }

    /// Did this operation lose the analytic surfaces?
    pub fn degraded_p(&self) -> bool {
        self.fidelity == Fidelity::TriangleSoup
    }

    fn with_result(mut self, result: &BooleanResult) -> Self {
        let BooleanResult::BRep(brep) = result;
        self.faces = brep.topology.faces.len();
        self
    }
}

/// Perform a CSG boolean operation on two B-rep solids.
///
/// Uses a B-rep classification pipeline:
/// 1. AABB filter to check for overlap
/// 2. Classify each face of A relative to B and vice versa
/// 3. Select faces based on the boolean operation
/// 4. Sew selected faces into a result solid
///
/// For non-overlapping solids, shortcuts are taken (e.g., union is
/// just both solids combined). Falls back to mesh-based approach
/// when the B-rep pipeline can't handle a case.
///
/// This wrapper discards the [`BooleanReport`]. Callers that need to know
/// whether the result kept its analytic surfaces — anything that will
/// export STEP, ray-trace, fillet or draft the result — should call
/// [`boolean_op_reported`] instead.
///
/// # Errors
///
/// Returns [`BooleanError`] when the pipeline cannot produce a result —
/// currently only when surface-surface intersection hits a surface whose
/// reported kind doesn't match its concrete type. Unsupported-but-consistent
/// surface pairs degrade to a sampled intersection instead of erroring.
pub fn boolean_op(
    solid_a: &BRepSolid,
    solid_b: &BRepSolid,
    op: BooleanOp,
    segments: u32,
) -> Result<BooleanResult, BooleanError> {
    boolean_op_reported(solid_a, solid_b, op, segments).map(|(result, _)| result)
}

/// [`boolean_op`], plus a [`BooleanReport`] describing what the operation
/// did to the representation.
///
/// A successful return is *not* a promise that the result is still a real
/// B-rep: check [`BooleanReport::fidelity`]. See [`DegradeReason`] for the
/// seven ways a result can come back as triangle soup.
///
/// # Errors
///
/// Same as [`boolean_op`].
pub fn boolean_op_reported(
    solid_a: &BRepSolid,
    solid_b: &BRepSolid,
    op: BooleanOp,
    segments: u32,
) -> Result<(BooleanResult, BooleanReport), BooleanError> {
    // Analytic surfaces of both operands, kept so the mesh fallback's
    // result can be re-projected onto them (defect D: BSP splitting and
    // seam healing leave quadric-face vertices up to ~0.6 mm off-surface,
    // which is bigger than a printed part's entire fit budget).
    let quadrics = QuadricCtx::collect(solid_a, solid_b);

    // Check if solids overlap at all
    let aabb_a = bbox::solid_aabb(solid_a);
    let aabb_b = bbox::solid_aabb(solid_b);

    if !aabb_a.overlaps(&aabb_b) {
        // No overlap — shortcut
        let result = non_overlapping_boolean(solid_a, solid_b, op, segments);
        let report = BooleanReport::analytic(op).with_result(&result);
        return Ok((result, report));
    }

    // Specialized fast path: two simple cylinders with perpendicular,
    // intersecting, equal-radius axes (the cross-shaft / Steinmetz
    // geometry). The general BRep pipeline can't render this case
    // correctly because cylinder × cylinder analytic SSI returns a
    // figure-8 boundary that the cylindrical-face splitter can't
    // decompose into faces the existing tessellator handles. Emit a
    // tessellated mesh whose discretization respects the Steinmetz
    // boundary directly.
    if let Some(result) = cyl_cyl::cylinder_cylinder_mesh_boolean(solid_a, solid_b, op) {
        let report =
            BooleanReport::degraded(op, DegradeReason::SteinmetzCylinders).with_result(&result);
        return Ok((result, report));
    }

    // Triangle-soup operands (a prior mesh-fallback result, chained): the
    // BRep pipeline gains nothing from thousands of anonymous planar
    // triangle faces and its face-pair stages blow up quadratically — a
    // two-cut chain through a spherical pocket used to produce 246k
    // triangles in 21 s. Go straight to the mesh boolean.
    if crate::mesh::is_triangle_soup(solid_a) || crate::mesh::is_triangle_soup(solid_b) {
        let mesh_a = tessellate_brep(solid_a, segments);
        let mesh_b = tessellate_brep(solid_b, segments);
        let result = mesh_fallback(&mesh_a, &mesh_b, op, &quadrics)?;
        let report = BooleanReport::degraded(op, DegradeReason::SoupOperand).with_result(&result);
        return Ok((result, report));
    }

    // Flag arrangements the splitters provably cannot represent. This is a
    // capability declaration, not a verdict: the B-rep pipeline often
    // copes anyway (a crossing may be resolved through other, analytic
    // face pairs), so a flag alone must not condemn the result — measured,
    // a flagged sphere × cylinder case came out within 2.3% of truth, and
    // swapping it for the coarse fallback would have been the regression.
    let flagged =
        crate::unrepresentable::arrangement_is_unrepresentable(solid_a, solid_b, segments);

    // `sphere_unrepresentable` reports the one unsound path the pipeline
    // can only detect from the inside: a spherical face cut by intersecting
    // circles, which the cap splitter cannot partition.
    let mut sphere_unrepresentable = false;
    let result = brep_boolean(solid_a, solid_b, op, segments, &mut sphere_unrepresentable)?;
    let result_mesh = result.to_mesh(segments);

    // Sound unconditionally: a bounded solid always has positive volume.
    let inverted = validate_boolean_result(&result_mesh).is_err();

    // Tessellate each operand at most once. Every path below that needs the
    // operand meshes — the volume checks, the fallback, the watertightness
    // swap — shares these.
    //
    // Difference always pays for them: its no-op guard below is the only
    // thing standing between a silently-skipped cut and a plausible mesh of
    // the wrong solid, and that guard needs both operand meshes. The cost is
    // two tessellations against a pipeline that has already run SSI,
    // splitting, classification and sewing — and the guard's expensive half
    // (the probe grid) still only runs when the cheap volume test trips.
    let operands = (flagged || sphere_unrepresentable || inverted || op == BooleanOp::Difference)
        .then(|| {
            (
                tessellate_brep(solid_a, segments),
                tessellate_brep(solid_b, segments),
            )
        });

    // The sphere gate is a capability flag like the others, not a verdict.
    // It fires whenever a spherical face *would* need splitting by
    // intersecting circles, and the pipeline then returns the operands
    // unchanged — which is correct whenever the cut removes nothing anyway,
    // and a silent no-op when it does not. The volume check tells the two
    // apart, so a flagged-but-correct result keeps its analytic surfaces.
    // Which condition condemned the B-rep result, for the report. `None`
    // means nothing did.
    let mut broken_reason: Option<DegradeReason> = if inverted {
        Some(DegradeReason::InvertedVolume)
    } else {
        match &operands {
            Some((mesh_a, mesh_b)) if flagged || sphere_unrepresentable => {
                crate::validate::volume_disagrees_grossly(&result_mesh, mesh_a, mesh_b, op)
                    .then_some(if sphere_unrepresentable {
                        DegradeReason::SphereArrangement
                    } else {
                        DegradeReason::VolumeDisagreement
                    })
            }
            _ => None,
        }
    };
    let mut broken = broken_reason.is_some();

    // Fail closed on a Difference that removed nothing at all from a
    // subtrahend it demonstrably overlaps. This is sound where a general
    // volume oracle is not (see `validate::difference_removed_nothing`), and
    // it is the guard that would have caught both 2026-08-11 field reports
    // at the moment they were produced rather than in a print.
    if !broken && op == BooleanOp::Difference {
        if let Some((mesh_a, mesh_b)) = &operands {
            if crate::validate::difference_removed_nothing(&result_mesh, mesh_a, mesh_b) {
                eprintln!(
                    "vcad boolean: Difference removed no volume from a subtrahend that \
                     overlaps the minuend — rejecting the B-rep result and re-cutting \
                     with the mesh boolean"
                );
                broken = true;
                broken_reason = Some(DegradeReason::DifferenceRemovedNothing);
            }
        }
    }

    // Opt-in structural report. Deliberately not on by default: measured on
    // known-good results a thin-blade intersection scores 3 unpaired edges
    // and a sheet-metal fold union 64, so an unconditional warning would cry
    // wolf on correct geometry (see `mesh_report`). It stays available for
    // exactly the kind of investigation that produced these fixes.
    if std::env::var_os("VCAD_BOOLEAN_WARN").is_some() {
        let report = crate::mesh_report(&result_mesh);
        if report.open_edges > 0 {
            eprintln!(
                "vcad boolean: {op:?} result is not closed — {} unpaired directed edges \
                 across {} triangles (volume {:.3})",
                report.open_edges, report.triangles, report.signed_volume
            );
        }
    }

    // Cheap structural stat carried on every report so a caller can see a
    // cracked-but-analytic result without re-tessellating. One hashed pass
    // over a mesh the pipeline already built.
    let result_open_edges = crate::mesh_report(&result_mesh).open_edges;
    // Analytic outcome, shared by the four "keep the B-rep" returns below.
    let keep = |result: BooleanResult| {
        let report = BooleanReport {
            flagged_unrepresentable: flagged || sphere_unrepresentable,
            open_edges: result_open_edges,
            ..BooleanReport::analytic(op)
        }
        .with_result(&result);
        (result, report)
    };

    if broken {
        let Some((mesh_a, mesh_b)) = &operands else {
            // Nothing to re-cut with — the condemned B-rep is all there is.
            // Report it as flagged so the caller can see the result was not
            // trusted even though it kept its surfaces.
            return Ok(keep(result));
        };
        let alt = mesh_fallback(mesh_a, mesh_b, op, &quadrics)?;
        let reason = broken_reason.unwrap_or(DegradeReason::VolumeDisagreement);
        let mut report = BooleanReport::degraded(op, reason).with_result(&alt);
        report.flagged_unrepresentable = flagged || sphere_unrepresentable;
        return Ok((alt, report));
    }

    // The result is trustworthy, but it may still be *cracked*: the splitters
    // routinely leave hairline seams where they conform curved faces. That is
    // not a correctness failure — which is exactly why it must not gate a
    // boolean (see `validate`) — but a watertight result is strictly more
    // useful downstream (STL for printing, STEP, ray tracing), and the mesh
    // fallback heals its own seams.
    //
    // The swap is offered only on arrangements already declared
    // unrepresentable. Tying it to the capability flag rather than to a
    // crack-size threshold is what keeps analytic surfaces where they are
    // worth having: a thin blade cut from a cylinder leaves 3 hairline
    // seam edges, and its faces are still true cylindrical bands the
    // tessellator needs — trading those for triangle soup to close 3 edges
    // is a bad deal. On a flagged arrangement the splitters were already
    // working outside what they can represent, so the analytic surfaces are
    // not reliable in the first place.
    //
    // Even then the fallback is taken only if it is watertight *and* agrees
    // on volume. Agreement is what makes this safe: it establishes the two
    // represent the same solid, so the swap trades analytic surfaces for
    // watertightness and nothing else.
    //
    // Eligibility is asked of the CAPABILITY FLAGS, never of whether the
    // operand meshes happen to be in hand — Difference now tessellates them
    // unconditionally for its no-op guard above, and reading `operands` as
    // the gate silently offered this swap to every ordinary difference. It
    // took `torr_boolean_catalogue::b1` immediately: a blade cut from a
    // cylinder has a few hairline seams, the mesh fallback is watertight and
    // agrees on volume, so the analytic r45 wall was traded for coarse
    // triangle soup and the volume fell 529 mm³ short of analytic truth.
    if !(flagged || sphere_unrepresentable || inverted) {
        return Ok(keep(result));
    }
    let Some((mesh_a, mesh_b)) = &operands else {
        return Ok(keep(result));
    };
    if result_open_edges == 0 {
        return Ok(keep(result));
    }
    let Ok(alt) = mesh_fallback(mesh_a, mesh_b, op, &quadrics) else {
        return Ok(keep(result));
    };
    let alt_mesh = alt.to_mesh(segments);
    let alt_report = crate::mesh_report(&alt_mesh);
    let brep_vol = crate::validate::mesh_signed_volume(&result_mesh).abs();
    let alt_vol = alt_report.signed_volume.abs();
    let agree = (alt_vol - brep_vol).abs() <= 0.10 * brep_vol.max(alt_vol);
    if alt_report.open_edges == 0 && alt_report.triangles > 0 && agree {
        let mut report =
            BooleanReport::degraded(op, DegradeReason::WatertightnessSwap).with_result(&alt);
        report.flagged_unrepresentable = true;
        return Ok((alt, report));
    }
    Ok(keep(result))
}

/// Mesh-CSG fallback: combine the operand tessellations with the BSP
/// boolean and wrap the result as a triangle-soup B-rep. The fallback is
/// itself held to the validity oracle — if even the mesh boolean cannot
/// produce a sound result, fail closed with [`BooleanError::InvalidResult`]
/// rather than returning a wrong solid.
fn mesh_fallback(
    mesh_a: &TriangleMesh,
    mesh_b: &TriangleMesh,
    op: BooleanOp,
    quadrics: &QuadricCtx,
) -> Result<BooleanResult, BooleanError> {
    let mut out = crate::mesh::csg::mesh_csg(mesh_a, mesh_b, op);
    quadrics.project_mesh(&mut out);
    validate_boolean_result(&out).map_err(BooleanError::InvalidResult)?;
    let mut brep = crate::mesh::mesh_to_brep(&out);
    // Carry the operands' quadric carriers forward in the result's geometry
    // store (no face references them; they are dormant). A later boolean
    // whose operand is this triangle soup can then still recognize sphere
    // and cylinder vertices and re-project them — without this, the first
    // fallback in a chain is the LAST time the surfaces are known, and
    // every subsequent cut re-shatters the quadric regions unrepaired.
    quadrics.stash_into(&mut brep.geometry);
    Ok(BooleanResult::BRep(Box::new(brep)))
}

/// Analytic quadric carriers of the two operands, used to push the mesh
/// fallback's vertices back onto the surfaces they came from.
///
/// The mesh boolean works on tessellations: operand triangles get split by
/// the other operand's carrier planes and the seams re-welded, so vertices
/// that belong to a sphere end up on chords, midpoints, and snapped seam
/// positions — measured up to ~0.6 mm off a R25 sphere, versus a 0.03 mm
/// chord sag. Planes and cylinders come through exact; spheres do not.
///
/// The repair is conservative by construction:
///  - a vertex is only moved if some quadric is within `BAND` of it, the
///    move is small, and exactly ONE quadric claims it (seam vertices
///    between two quadrics are left alone);
///  - a vertex lying on one operand plane is projected ALONG that plane
///    (onto the plane∩sphere circle), so planar faces stay planar;
///  - a vertex on two or more planes (a box edge) is never moved.
struct QuadricCtx {
    spheres: Vec<(Point3, f64)>,
    cylinders: Vec<(Point3, Vec3, f64)>,
    planes: Vec<(Point3, Vec3)>,
}

impl QuadricCtx {
    /// Distance band within which a vertex is considered to belong to a
    /// quadric. Must exceed the worst observed off-surface error (~0.6 mm)
    /// with margin, while staying below feature scale.
    const BAND: f64 = 0.75;
    /// In-plane snap band for plane-constrained projection. Wider than
    /// `BAND` because a plane at height h from the sphere center amplifies
    /// radial error by 1/sin(phi): a 0.62 mm radial error at h = 19.8 on a
    /// R25 sphere shows up as 0.93 mm in the plane.
    const INPLANE_BAND: f64 = 1.3;
    /// Vertices within this of the surface are already correct — leave
    /// them; only genuinely displaced vertices are snapped. This is what
    /// keeps legitimate geometry that merely passes near a quadric safe.
    const GOOD_EPS: f64 = 0.02;

    fn collect(a: &BRepSolid, b: &BRepSolid) -> Self {
        let mut ctx = QuadricCtx {
            spheres: Vec::new(),
            cylinders: Vec::new(),
            planes: Vec::new(),
        };
        let mut plane_keys: std::collections::HashSet<(i64, i64, i64, i64)> =
            std::collections::HashSet::new();
        for solid in [a, b] {
            for surface in &solid.geometry.surfaces {
                let any = surface.as_any();
                if let Some(s) = any.downcast_ref::<vcad_kernel_geom::SphereSurface>() {
                    let r = s.radius.abs();
                    if r > 1e-9
                        && !ctx
                            .spheres
                            .iter()
                            .any(|(c, cr)| (*c - s.center).norm() < 1e-6 && (cr - r).abs() < 1e-6)
                    {
                        ctx.spheres.push((s.center, r));
                    }
                } else if let Some(c) = any.downcast_ref::<vcad_kernel_geom::CylinderSurface>() {
                    let r = c.radius.abs();
                    let axis = c.axis.into_inner();
                    if r > 1e-9
                        && !ctx.cylinders.iter().any(|(cc, ca, cr)| {
                            (cr - r).abs() < 1e-6 && ca.cross(axis).norm() < 1e-6 && {
                                let d = *cc - c.center;
                                (d - axis * d.dot(axis)).norm() < 1e-6
                            }
                        })
                    {
                        ctx.cylinders.push((c.center, axis, r));
                    }
                } else if let Some(p) = any.downcast_ref::<vcad_kernel_geom::Plane>() {
                    // Dedupe coincident carriers: a triangle-soup operand
                    // contributes one Plane per triangle, and thousands of
                    // copies of the same carrier would pin every vertex as
                    // "on two planes". Canonical key: normal with its
                    // largest component made positive, plus signed offset.
                    let mut n = p.normal_dir.into_inner();
                    let amax = n.x.abs().max(n.y.abs()).max(n.z.abs());
                    let flip = if n.x.abs() == amax {
                        n.x < 0.0
                    } else if n.y.abs() == amax {
                        n.y < 0.0
                    } else {
                        n.z < 0.0
                    };
                    if flip {
                        n = -n;
                    }
                    let o = p.origin;
                    let d = (o - Point3::origin()).dot(n);
                    let key = (
                        (n.x * 1e6).round() as i64,
                        (n.y * 1e6).round() as i64,
                        (n.z * 1e6).round() as i64,
                        (d * 1e4).round() as i64,
                    );
                    if plane_keys.insert(key) {
                        ctx.planes.push((o, n));
                    }
                }
            }
        }
        ctx
    }

    /// Append this context's quadrics as dormant surfaces on a geometry
    /// store, so `collect` on a chained boolean rediscovers them.
    fn stash_into(&self, geom: &mut vcad_kernel_geom::GeometryStore) {
        for (c, r) in &self.spheres {
            let mut s = vcad_kernel_geom::SphereSurface::new(*r);
            s.center = *c;
            geom.add_surface(Box::new(s));
        }
        for (c, axis, r) in &self.cylinders {
            let mut cy = vcad_kernel_geom::CylinderSurface::new(*r);
            cy.center = *c;
            cy.axis = vcad_kernel_math::Dir3::new_normalize(*axis);
            geom.add_surface(Box::new(cy));
        }
    }

    fn project_mesh(&self, mesh: &mut TriangleMesh) {
        let dbg = std::env::var_os("VCAD_PROJ_DEBUG").is_some();
        if self.spheres.is_empty() && self.cylinders.is_empty() {
            return;
        }
        // Per-vertex incident triangle normals. The constraint a vertex
        // lives under is decided LOCALLY: an incident face whose normal a
        // nearby quadric cannot explain is a planar feature the vertex
        // must stay on. Global plane lists cannot do this — a tessellated
        // sphere contributes one facet carrier per triangle and pins
        // everything.
        let nv = mesh.vertices.len() / 3;
        let mut inc: Vec<Vec<Vec3>> = vec![Vec::new(); nv];
        for t in mesh.indices.as_chunks::<3>().0 {
            let g = |k: u32| {
                Point3::new(
                    mesh.vertices[(k as usize) * 3] as f64,
                    mesh.vertices[(k as usize) * 3 + 1] as f64,
                    mesh.vertices[(k as usize) * 3 + 2] as f64,
                )
            };
            let (a, b, c) = (g(t[0]), g(t[1]), g(t[2]));
            let n = (b - a).cross(c - a);
            let l = n.norm();
            if l < 1e-12 {
                continue;
            }
            let n = n / l;
            for &k in t {
                let bucket = &mut inc[k as usize];
                if !bucket.iter().any(|m| m.dot(n).abs() > 0.996) {
                    bucket.push(n);
                }
            }
        }
        let mut moved = 0usize;
        for (vi, chunk) in mesh.vertices.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let v = Point3::new(chunk[0] as f64, chunk[1] as f64, chunk[2] as f64);
            // Split incident normals into quadric-explained and planar.
            let mut planar: Vec<Vec3> = Vec::new();
            for n in &inc[vi] {
                if !self.normal_explained(&v, n) && !planar.iter().any(|m| m.dot(n).abs() > 0.996) {
                    planar.push(*n);
                }
            }
            if planar.len() >= 2 {
                continue; // feature edge or corner: pinned
            }
            let mean_n = {
                let mut m = Vec3::zeros();
                for n in &inc[vi] {
                    // Orient consistently before averaging: flip toward the
                    // first normal so opposite-facing duplicates don't cancel.
                    let r = inc[vi][0];
                    m += if n.dot(r) < 0.0 { -*n } else { *n };
                }
                let l = m.norm();
                if l > 1e-9 {
                    Some(m / l)
                } else {
                    None
                }
            };
            if let Some(p) = self.project_point(&v, planar.first(), mean_n.as_ref()) {
                moved += 1;
                chunk[0] = p.x as f32;
                chunk[1] = p.y as f32;
                chunk[2] = p.z as f32;
            }
        }
        if dbg {
            eprintln!(
                "[proj] spheres={} cyls={} moved={} of {}",
                self.spheres.len(),
                self.cylinders.len(),
                moved,
                nv
            );
        }
    }

    /// Can any nearby quadric's surface normal at `v` explain the incident
    /// facet normal `n`? Facet normals of a tessellated quadric lag the true
    /// normal by up to the facet's angular pitch; 25 degrees of slack covers
    /// coarse tessellations without absorbing genuinely planar features.
    fn normal_explained(&self, v: &Point3, n: &Vec3) -> bool {
        const COS_SLACK: f64 = 0.90; // ~25 degrees
        for (c, r) in &self.spheres {
            let d = *v - *c;
            let dist = d.norm();
            if (dist - r).abs() < Self::BAND && dist > 1e-9 && (d / dist).dot(n).abs() > COS_SLACK {
                return true;
            }
        }
        for (c, axis, r) in &self.cylinders {
            let d = *v - *c;
            let radial = d - *axis * d.dot(*axis);
            let dist = radial.norm();
            if (dist - r).abs() < Self::BAND
                && dist > 1e-9
                && (radial / dist).dot(n).abs() > COS_SLACK
            {
                return true;
            }
        }
        false
    }

    fn project_point(
        &self,
        v: &Point3,
        plane_n: Option<&Vec3>,
        mean_n: Option<&Vec3>,
    ) -> Option<Point3> {
        // A single planar constraint from the caller: the vertex lies on a
        // planar feature through v with this normal, and must stay in it.
        let on_planes: Vec<(Point3, Vec3)> = plane_n.map(|n| (*v, *n)).into_iter().collect();

        // Classify against every quadric: `good` = already on it,
        // `bad` = near it but displaced.
        let mut sph_good: Vec<usize> = Vec::new();
        let mut sph_bad: Vec<usize> = Vec::new();
        for (i, (c, r)) in self.spheres.iter().enumerate() {
            let e = ((*v - *c).norm() - r).abs();
            if e <= Self::GOOD_EPS {
                sph_good.push(i);
            } else if e < Self::BAND {
                sph_bad.push(i);
            }
        }
        let mut cyl_good: Vec<usize> = Vec::new();
        let mut cyl_bad: Vec<usize> = Vec::new();
        for (i, (c, axis, r)) in self.cylinders.iter().enumerate() {
            let d = *v - *c;
            let radial = d - *axis * d.dot(*axis);
            let e = (radial.norm() - r).abs();
            if e <= Self::GOOD_EPS {
                cyl_good.push(i);
            } else if e < Self::BAND {
                cyl_bad.push(i);
            }
        }

        let bads = sph_bad.len() + cyl_bad.len();
        if bads == 0 {
            return None; // on-surface or far from everything
        }

        // Seam cases: displaced from a sphere while on (or also displaced
        // from) a cylinder whose axis passes through the sphere center —
        // the intersection is an exact circle; snap onto it. This keeps
        // the vertex on the cylinder too.
        if on_planes.is_empty() && sph_bad.len() == 1 && (cyl_good.len() + cyl_bad.len()) == 1 {
            let ci = *cyl_good.first().or(cyl_bad.first()).unwrap();
            let (sc, sr) = self.spheres[sph_bad[0]];
            let (cc, axis, cr) = self.cylinders[ci];
            if let Some(p) = Self::seam_circle(v, &sc, sr, &cc, &axis, cr) {
                return Some(p).filter(|p| (*p - *v).norm() < Self::INPLANE_BAND);
            }
            // Osculating pair (equal radii, axis through the sphere center):
            // no seam circle exists — the composite surface hands off from
            // sphere to cylinder AT the tangency plane, and near it the two
            // agree to second order, so the projector used to stand down and
            // leave the whole band unrepaired. The vertex's own facets say
            // which surface it belongs to: sphere normals carry an axial
            // component (t/R at axial offset t), cylinder normals carry none.
            let co = sc - cc;
            let axis_through_center = (co - axis * co.dot(axis)).norm() < 1e-6;
            if axis_through_center && (sr - cr).abs() < 0.5 {
                if let Some(n) = mean_n {
                    let d = *v - sc;
                    let t = d.dot(axis);
                    let expected_axial = (t / sr).abs().min(1.0);
                    let axial = n.dot(axis).abs();
                    // Choose whichever surface predicts the observed facet
                    // orientation better: a sphere at this axial offset would
                    // show |n·axis| ~ t/R; a cylinder shows ~0.
                    let p = if (axial - expected_axial).abs() < axial {
                        let dist = d.norm();
                        if dist < 1e-9 {
                            return None;
                        }
                        sc + d * (sr / dist)
                    } else {
                        let radial = d - axis * t;
                        let rl = radial.norm();
                        if rl < 1e-9 {
                            return None;
                        }
                        *v - radial + radial * (cr / rl)
                    };
                    return Some(p).filter(|p| (*p - *v).norm() < Self::INPLANE_BAND);
                }
            }
            return None;
        }
        if on_planes.is_empty() && cyl_bad.len() == 1 && sph_good.len() == 1 && sph_bad.is_empty() {
            let (sc, sr) = self.spheres[sph_good[0]];
            let (cc, axis, cr) = self.cylinders[cyl_bad[0]];
            if let Some(p) = Self::seam_circle(v, &sc, sr, &cc, &axis, cr) {
                return Some(p).filter(|p| (*p - *v).norm() < Self::INPLANE_BAND);
            }
            return None;
        }
        if !sph_good.is_empty() || !cyl_good.is_empty() {
            return None; // already on some quadric; no clean target
        }
        if bads >= 2 {
            return None; // multi-quadric displacement with no analytic seam
        }

        // Single displaced quadric.
        if let Some(&i) = sph_bad.first() {
            let (c, r) = self.spheres[i];
            let d = *v - c;
            let dist = d.norm();
            if dist < 1e-9 {
                return None;
            }
            let p = if let Some((o, n)) = on_planes.first() {
                // Stay in the plane: project onto the plane∩sphere circle.
                let h = (c - *o).dot(*n);
                let rc2 = r * r - h * h;
                if rc2 <= 1e-12 {
                    return None;
                }
                let rc = rc2.sqrt();
                let cc = c - *n * h;
                let mut u = *v - cc;
                u -= *n * u.dot(*n);
                let ul = u.norm();
                if ul < 1e-9 || (ul - rc).abs() >= Self::INPLANE_BAND {
                    return None;
                }
                cc + u * (rc / ul)
            } else {
                c + d * (r / dist)
            };
            return Some(p).filter(|p| (*p - *v).norm() < Self::INPLANE_BAND);
        }
        if let Some(&i) = cyl_bad.first() {
            let (c, axis, r) = self.cylinders[i];
            let d = *v - c;
            let radial = d - axis * d.dot(axis);
            let dist = radial.norm();
            if dist < 1e-9 || !on_planes.is_empty() {
                return None; // plane-constrained cylinder: skip
            }
            let p = *v - radial + radial * (r / dist);
            return Some(p).filter(|p| (*p - *v).norm() < Self::INPLANE_BAND);
        }
        None
    }

    /// Intersection circle of a sphere and a cylinder whose axis passes
    /// through the sphere center; projection of `v` onto it, or None when
    /// the configuration is not that special case.
    fn seam_circle(
        v: &Point3,
        sc: &Point3,
        sr: f64,
        cc: &Point3,
        axis: &Vec3,
        cr: f64,
    ) -> Option<Point3> {
        let co = *sc - *cc;
        let off = co - *axis * co.dot(*axis);
        if off.norm() > 1e-6 || sr * sr <= cr * cr {
            return None;
        }
        let h = (sr * sr - cr * cr).sqrt();
        let d = *v - *sc;
        let t = d.dot(*axis);
        let radial = d - *axis * t;
        let rl = radial.norm();
        if rl < 1e-9 {
            return None;
        }
        let t_seam = if t >= 0.0 { h } else { -h };
        Some(*sc + *axis * t_seam + radial * (cr / rl))
    }
}
