//! Document-level design constraint solver.
//!
//! Lowers [`vcad_ir::constraints::DesignConstraint`]s — spanning PCB layout
//! (footprints, board outline), sketches, and mechanical part anchors — into
//! per-plane `Sketch2D` systems solved by the Levenberg-Marquardt solver in
//! `vcad-kernel-constraints`, then writes the solved geometry back into the
//! document.
//!
//! Solve model:
//! - Constraints are partitioned into **planar groups** by the node their
//!   free anchors reference (one group per PCB board node, one per sketch
//!   node). Groups solve independently.
//! - **Mechanical geometry is authoritative**: `PartEdge` anchors resolve
//!   through an injected [`AnchorResolver`] to world coordinates, project
//!   into the group plane, and enter the solve as *fixed* reference
//!   geometry. Constraints pull boards and sketches toward parts, never the
//!   reverse.
//! - **Driven dimensions** are excluded from the solve entirely (zero
//!   residuals) and measured from the solved geometry afterward.
//! - Anchor resolution is **fail-closed**: a bad footprint ref, an
//!   out-of-range outline index, or an ambiguous/lost part edge skips that
//!   constraint with an error entry; it never silently rebinds.

#![warn(missing_docs)]

mod lower;
mod measure;
mod report;

pub use measure::constraint_residual;
pub use report::{ConstraintResidual, DesignSolveReport, DrivenValue, GroupReport};

use vcad_ir::Document;

/// Resolves cross-domain part anchors to world-space geometry.
///
/// Implemented by callers that can evaluate part geometry (the kernel, the
/// WASM bindings). The solver crate itself stays kernel-free.
pub trait AnchorResolver {
    /// World-space endpoints (mm) of the part edge named by two adjacent
    /// face names, fail-closed: ambiguous or lost names are errors.
    fn resolve_part_edge(
        &self,
        node: vcad_ir::NodeId,
        face_a: &str,
        face_b: &str,
    ) -> Result<([f64; 3], [f64; 3]), String>;
}

/// A resolver for documents with no part anchors: every part-edge lookup
/// fails with a clear message.
pub struct NoPartAnchors;

impl AnchorResolver for NoPartAnchors {
    fn resolve_part_edge(
        &self,
        node: vcad_ir::NodeId,
        face_a: &str,
        face_b: &str,
    ) -> Result<([f64; 3], [f64; 3]), String> {
        Err(format!(
            "part-edge anchor {face_a}/{face_b} on node {node}: no part geometry resolver available"
        ))
    }
}

/// Options for a solve pass.
#[derive(Debug, Clone, Default)]
pub struct SolveOptions {
    /// Temporarily pin these footprints (by board node + reference) at
    /// their current positions for this solve only — used by interactive
    /// drag so the dragged part anchors the solve. Never persisted.
    pub extra_fixed: Vec<(vcad_ir::NodeId, String)>,
}

/// Solve the document's constraints and write solved geometry (footprint
/// positions/rotations, outline vertices, sketch segment points) back into
/// `doc`. Driven dimension values are back-annotated as literal numbers.
pub fn solve_design_constraints(
    doc: &mut Document,
    resolver: &dyn AnchorResolver,
    options: &SolveOptions,
) -> DesignSolveReport {
    lower::run(doc, resolver, options, true)
}

/// Validate and measure the document's constraints without mutating
/// geometry: resolves anchors, reports DOF, and measures every dimensional
/// constraint's current value.
pub fn check_design_constraints(
    doc: &Document,
    resolver: &dyn AnchorResolver,
) -> DesignSolveReport {
    let mut clone = doc.clone();
    lower::run(&mut clone, resolver, &SolveOptions::default(), false)
}

#[cfg(test)]
mod tests;
