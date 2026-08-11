//! Public API types and entry point for boolean operations.

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
    // Check if solids overlap at all
    let aabb_a = bbox::solid_aabb(solid_a);
    let aabb_b = bbox::solid_aabb(solid_b);

    if !aabb_a.overlaps(&aabb_b) {
        // No overlap — shortcut
        return Ok(non_overlapping_boolean(solid_a, solid_b, op, segments));
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
        return Ok(result);
    }

    // Triangle-soup operands (a prior mesh-fallback result, chained): the
    // BRep pipeline gains nothing from thousands of anonymous planar
    // triangle faces and its face-pair stages blow up quadratically — a
    // two-cut chain through a spherical pocket used to produce 246k
    // triangles in 21 s. Go straight to the mesh boolean.
    if crate::mesh::is_triangle_soup(solid_a) || crate::mesh::is_triangle_soup(solid_b) {
        let mesh_a = tessellate_brep(solid_a, segments);
        let mesh_b = tessellate_brep(solid_b, segments);
        return mesh_fallback(&mesh_a, &mesh_b, op);
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
    let mut broken = inverted
        || match &operands {
            Some((mesh_a, mesh_b)) if flagged || sphere_unrepresentable => {
                crate::validate::volume_disagrees_grossly(&result_mesh, mesh_a, mesh_b, op)
            }
            _ => false,
        };

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

    if broken {
        let Some((mesh_a, mesh_b)) = &operands else {
            return Ok(result);
        };
        return mesh_fallback(mesh_a, mesh_b, op);
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
    let Some((mesh_a, mesh_b)) = &operands else {
        return Ok(result);
    };
    if crate::mesh_report(&result_mesh).open_edges == 0 {
        return Ok(result);
    }
    let Ok(alt) = mesh_fallback(mesh_a, mesh_b, op) else {
        return Ok(result);
    };
    let alt_mesh = alt.to_mesh(segments);
    let alt_report = crate::mesh_report(&alt_mesh);
    let brep_vol = crate::validate::mesh_signed_volume(&result_mesh).abs();
    let alt_vol = alt_report.signed_volume.abs();
    let agree = (alt_vol - brep_vol).abs() <= 0.10 * brep_vol.max(alt_vol);
    if alt_report.open_edges == 0 && alt_report.triangles > 0 && agree {
        return Ok(alt);
    }
    Ok(result)
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
) -> Result<BooleanResult, BooleanError> {
    let out = crate::mesh::csg::mesh_csg(mesh_a, mesh_b, op);
    validate_boolean_result(&out).map_err(BooleanError::InvalidResult)?;
    Ok(BooleanResult::BRep(Box::new(crate::mesh::mesh_to_brep(
        &out,
    ))))
}
