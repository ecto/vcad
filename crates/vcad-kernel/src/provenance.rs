//! Representation-loss provenance: *when* a solid stopped being a real
//! B-rep, and *which operation* did it.
//!
//! # Why this exists
//!
//! `Solid::to_step` fails with `StepExportError::NotBRep` — "B-rep data
//! lost after boolean operations". That message describes only the
//! narrowest of three degradations, and it arrives at export time, which
//! may be dozens of operations after the loss.
//!
//! There are actually three tiers, and the middle one was completely
//! invisible:
//!
//! | Tier | `can_export_step()` | What STEP export writes |
//! |---|---|---|
//! | [`SolidFidelity::Analytic`] | `true` | Real Plane/Cylinder/Cone/Sphere/Torus faces |
//! | [`SolidFidelity::TriangleSoup`] | **`true`** | Thousands of one-triangle planar facets |
//! | [`SolidFidelity::MeshOnly`] | `false` | Nothing — `NotBRep` |
//!
//! The middle row is the one that bites. `vcad_kernel_booleans::BooleanResult`
//! has a single variant, `BRep`: every mesh-CSG fallback is re-wrapped as a
//! triangle-soup B-rep by `mesh_to_brep`. Such a result is topologically
//! valid, so `can_export_step()` returns `true`, STEP export succeeds, and
//! the fabricator receives a faceted approximation of what the model says
//! is a Ø12 H7 bore. Nothing in the pipeline said a word.
//!
//! Worse, the loss is *contagious*: `boolean_op` routes any triangle-soup
//! operand straight to the mesh fallback, so one degradation converts every
//! downstream boolean on that branch.
//!
//! This module records each loss at the point it happens. See
//! [`Solid::fidelity`](crate::Solid::fidelity),
//! [`Solid::why_not_brep`](crate::Solid::why_not_brep) and
//! [`Solid::degradations`](crate::Solid::degradations).

use vcad_kernel_booleans::{BooleanError, DegradeReason};

/// How faithfully a [`Solid`](crate::Solid) still represents its geometry.
///
/// Ordered worst-last, so `max` over a set of operands gives the worst
/// fidelity present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SolidFidelity {
    /// No geometry.
    Empty,
    /// A true B-rep: faces carry their analytic surfaces. STEP, ray
    /// tracing, fillets, drafting projections and DFM all work properly.
    Analytic,
    /// A B-rep in name only — hundreds or thousands of one-triangle planar
    /// faces produced by the mesh-CSG fallback. `can_export_step()` still
    /// returns `true`; the STEP is facet soup, fillets have no edges worth
    /// blending, and every subsequent boolean is forced down the mesh path.
    TriangleSoup,
    /// No B-rep at all. STEP export fails with `NotBRep`.
    MeshOnly,
}

impl SolidFidelity {
    /// Stable identifier, for reports and serialization.
    pub fn as_str(self) -> &'static str {
        match self {
            SolidFidelity::Empty => "empty",
            SolidFidelity::Analytic => "analytic",
            SolidFidelity::TriangleSoup => "triangle-soup",
            SolidFidelity::MeshOnly => "mesh-only",
        }
    }

    /// Does this fidelity still support the B-rep-dependent downstream
    /// features (meaningful STEP, ray tracing, fillets, drafting)?
    pub fn is_brep(self) -> bool {
        matches!(self, SolidFidelity::Analytic | SolidFidelity::Empty)
    }
}

impl std::fmt::Display for SolidFidelity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What kind of representation loss occurred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LossKind {
    /// The boolean *succeeded* but returned triangle soup from the
    /// mesh-CSG fallback. The geometry is right; the representation is not.
    /// This is the invisible case — `try_union` and friends return `Ok`.
    BooleanFallback(DegradeReason),
    /// The boolean *failed* and a union merged the two tessellations into
    /// a mesh-only solid. Note this is a mesh *concatenation*, not a CSG
    /// union: interior surfaces are retained.
    BooleanFailedMeshMerge(String),
    /// The boolean *failed* and the cut or overlap was silently skipped —
    /// `difference`/`intersection` return the target unchanged. The
    /// geometry is wrong, not merely coarse.
    BooleanFailedNoOp(String),
    /// An operand was already mesh-only, so the two meshes were
    /// concatenated. No longer emitted: mesh operands now run triangle-level
    /// CSG (see [`DegradeReason::MeshOperand`]). Kept so older provenance
    /// ledgers still decode.
    MeshOperandConcat,
    /// The solid was constructed from a triangle mesh and never had a
    /// B-rep (mesh import, topology optimization).
    MeshSource,
}

impl LossKind {
    /// Stable identifier, for reports and serialization.
    pub fn as_str(&self) -> &'static str {
        match self {
            LossKind::BooleanFallback(_) => "boolean-fallback",
            LossKind::BooleanFailedMeshMerge(_) => "boolean-failed-mesh-merge",
            LossKind::BooleanFailedNoOp(_) => "boolean-failed-no-op",
            LossKind::MeshOperandConcat => "mesh-operand-concat",
            LossKind::MeshSource => "mesh-source",
        }
    }

    /// The fidelity a solid drops to when this loss occurs.
    pub fn resulting_fidelity(&self) -> SolidFidelity {
        match self {
            LossKind::BooleanFallback(_) => SolidFidelity::TriangleSoup,
            _ => SolidFidelity::MeshOnly,
        }
    }

    /// Is the *geometry* wrong, as opposed to merely coarsely represented?
    ///
    /// A fallback still computes the right solid. A skipped cut does not.
    pub fn is_wrong_geometry(&self) -> bool {
        matches!(
            self,
            LossKind::BooleanFailedNoOp(_)
                | LossKind::MeshOperandConcat
                | LossKind::BooleanFailedMeshMerge(_)
        )
    }
}

impl std::fmt::Display for LossKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LossKind::BooleanFallback(r) => {
                write!(f, "fell back to the mesh boolean: {r}")
            }
            LossKind::BooleanFailedMeshMerge(e) => write!(
                f,
                "the boolean failed and the operands were merged as meshes ({e})"
            ),
            LossKind::BooleanFailedNoOp(e) => write!(
                f,
                "the boolean failed and the operation was skipped entirely, \
                 leaving the target unchanged ({e})"
            ),
            LossKind::MeshOperandConcat => write!(
                f,
                "an operand was mesh-only, so the meshes were concatenated \
                 instead of the boolean being performed"
            ),
            LossKind::MeshSource => write!(f, "the solid was built from a mesh, not a B-rep"),
        }
    }
}

/// One representation loss, recorded where it happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DegradeEvent {
    /// The operation responsible: `"union"`, `"difference"`,
    /// `"intersection"`, `"shell"`, `"from_mesh"`, …
    pub op: String,
    /// The document node this operation came from, when the caller
    /// attributed it via [`Solid::attribute_to`](crate::Solid::attribute_to).
    /// This is the field a caller restructuring a model actually needs.
    pub node: Option<String>,
    /// What was lost.
    pub kind: LossKind,
}

impl DegradeEvent {
    /// Record a loss with no node attribution yet.
    pub fn new(op: impl Into<String>, kind: LossKind) -> Self {
        Self {
            op: op.into(),
            node: None,
            kind,
        }
    }
}

impl std::fmt::Display for DegradeEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.node {
            Some(n) => write!(f, "`{}` (node {}): {}", self.op, n, self.kind),
            None => write!(f, "`{}`: {}", self.op, self.kind),
        }
    }
}

/// Build a `LossKind` for a failed boolean, distinguishing the union path
/// (mesh merge) from the difference/intersection path (silent no-op).
pub(crate) fn failed_boolean_kind(op_is_union: bool, err: &BooleanError) -> LossKind {
    let msg = err.to_string();
    if op_is_union {
        LossKind::BooleanFailedMeshMerge(msg)
    } else {
        LossKind::BooleanFailedNoOp(msg)
    }
}
