//! Public API types and entry point for boolean operations.

use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::{tessellate_brep, TriangleMesh};

use crate::bbox;
use crate::cyl_cyl;
use crate::pipeline::{brep_boolean, non_overlapping_boolean};

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
/// In Phase 1, this is a mesh-only result (no B-rep topology).
/// In Phase 2, this will contain a full BRepSolid.
#[derive(Debug, Clone)]
pub enum BooleanResult {
    /// Mesh-only result (Phase 1 fallback).
    Mesh(TriangleMesh),
    /// Full B-rep result (Phase 2, not yet implemented).
    BRep(Box<BRepSolid>),
}

impl BooleanResult {
    /// Get the triangle mesh, tessellating if needed.
    pub fn to_mesh(&self, _segments: u32) -> TriangleMesh {
        match self {
            BooleanResult::Mesh(m) => m.clone(),
            BooleanResult::BRep(brep) => tessellate_brep(brep.as_ref(), _segments),
        }
    }

    /// Get a reference to the BRep solid, if available.
    pub fn as_brep(&self) -> Option<&BRepSolid> {
        match self {
            BooleanResult::BRep(brep) => Some(brep.as_ref()),
            BooleanResult::Mesh(_) => None,
        }
    }

    /// Convert to BRepSolid, consuming self.
    /// Returns None if the result is mesh-only.
    pub fn into_brep(self) -> Option<BRepSolid> {
        match self {
            BooleanResult::BRep(brep) => Some(*brep),
            BooleanResult::Mesh(_) => None,
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
pub fn boolean_op(
    solid_a: &BRepSolid,
    solid_b: &BRepSolid,
    op: BooleanOp,
    segments: u32,
) -> BooleanResult {
    // Check if solids overlap at all
    let aabb_a = bbox::solid_aabb(solid_a);
    let aabb_b = bbox::solid_aabb(solid_b);

    if !aabb_a.overlaps(&aabb_b) {
        // No overlap — shortcut
        return non_overlapping_boolean(solid_a, solid_b, op, segments);
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
        return result;
    }

    // Solids overlap — use classification pipeline
    brep_boolean(solid_a, solid_b, op, segments)
}
