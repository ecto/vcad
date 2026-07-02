//! Document-parameter gradients: the differentiable seam over parametric
//! `.vcad` documents.
//!
//! The M6 design note rejected this as unreachable — a Rust-side
//! document → BRep evaluator sat behind the excluded `loon` sibling. With the
//! sibling in scope, `vcad-eval` *is* that evaluator, and the seam's
//! document-agnostic machinery ([`synthesize_seeding`] takes any
//! `Fn(&[f64]) -> BRepSolid`) composes with it directly:
//!
//! 1. A **named document parameter** (the `parameters` + `bindings` sidecar
//!    the product's parametric documents already carry) is the θ.
//! 2. The **build closure** clones the document, overwrites the parameter's
//!    value, and runs [`evaluate_document`](crate::evaluate_document) — the
//!    same resolve-and-walk every other consumer uses.
//! 3. **Seeding synthesis** (M6) machine-derives how θ moves every surface;
//!    the frozen-plan seam prices the mass-property derivatives exactly.
//!
//! The result: `d(mass properties)/d(parameter)` for every solid part of a
//! document, with zero hand seeding — a `.vcad` file with a named `"r"`
//! differentiates end to end. Documents authored in loon reach this path by
//! evaluating to a [`Document`] first (`vcad_loon::eval_vcad`) and binding
//! the parameter onto the produced nodes (exercised in the tests).

use vcad_ir::{Document, Expr};
use vcad_kernel::vcad_kernel_primitives::BRepSolid;
use vcad_kernel_diff::{
    evaluate_with_sensitivity, mass_properties_with_derivative, synthesize_seeding, DiffError,
    MassProperties,
};
use vcad_kernel_tessellate::frozen::capture_plan;
use vcad_kernel_tessellate::TessellationParams;

use crate::{evaluate_document, EvalError, EvalOptions};

/// Gradient of one solid part's mass properties with respect to a document
/// parameter.
#[derive(Debug, Clone)]
pub struct DocumentPartGradient {
    /// Index of the part in the evaluated scene's solid-part order (parts
    /// without a BRep solid are skipped and do not consume an index).
    pub part_index: usize,
    /// Mass properties at the current parameter value.
    pub properties: MassProperties<f64>,
    /// `d(properties)/dθ` — every field differentiated with respect to the
    /// named parameter.
    pub derivative: MassProperties<f64>,
}

/// Errors from [`document_parameter_gradient`].
#[derive(Debug, thiserror::Error)]
pub enum DocDiffError {
    /// The named parameter is not declared in `doc.parameters`.
    #[error("document has no parameter named {0:?}")]
    UnknownParameter(String),
    /// Parameter resolution failed (cycle, malformed formula, …).
    #[error("parameter resolution failed: {0}")]
    Resolve(String),
    /// Document evaluation failed.
    #[error("document evaluation failed: {0}")]
    Eval(#[from] EvalError),
    /// The document evaluated to no BRep-solid parts.
    #[error("document has no solid parts to differentiate")]
    NoSolidParts,
    /// A probe evaluation (θ ± h) produced a different number of solid parts
    /// than the base — the parameter crosses a topology/part-count boundary
    /// at this value and the frozen-plan contract cannot hold.
    #[error("solid part count changed under probe: {base} at θ, {probe} at θ ± h")]
    PartCountChanged {
        /// Solid parts at θ.
        base: usize,
        /// Solid parts at the probe value.
        probe: usize,
    },
    /// A seam error (capture, synthesis, evaluation).
    #[error("seam error: {0}")]
    Seam(#[from] DiffError),
}

/// Evaluate the document with `parameter = value` and collect the BRep solid
/// of every part that has one (skipping mesh-only or empty parts).
fn solids_at(
    doc: &Document,
    parameter: &str,
    value: f64,
    options: &EvalOptions,
) -> Result<Vec<BRepSolid>, DocDiffError> {
    let mut d = doc.clone();
    match d.parameters.get_mut(parameter) {
        Some(p) => p.value = Expr::Number(value),
        None => return Err(DocDiffError::UnknownParameter(parameter.to_string())),
    }
    let scene = evaluate_document(&d, options)?;
    Ok(scene
        .parts
        .iter()
        .filter_map(|p| p.solid.as_ref().and_then(|s| s.as_brep()).cloned())
        .collect())
}

/// Differentiate every solid part's mass properties with respect to a named
/// document parameter, `d(V, m, centroid, inertia)/dθ`, via the seam.
///
/// `density` is the physical density fed to the mass-property integrals
/// (geometry is in mm; pass `kg/m³ · 1e-9` for masses in kg, or `1.0` for
/// raw volume moments). `probe_step` is the finite step used **only** by
/// seeding synthesis to *match* surfaces between θ and θ ± h (M6); the
/// returned derivatives are analytic seam evaluations, not finite
/// differences.
///
/// The document is re-evaluated three times (θ, θ ± h for synthesis probes)
/// plus one frozen-plan capture and one seam pass per solid part. Parameter
/// values that change the part count or a part's topology between θ − h and
/// θ + h error rather than returning a wrong gradient.
pub fn document_parameter_gradient(
    doc: &Document,
    parameter: &str,
    density: f64,
    tess: &TessellationParams,
    probe_step: f64,
) -> Result<Vec<DocumentPartGradient>, DocDiffError> {
    let options = EvalOptions {
        skip_clash_detection: true,
        clock: None,
    };
    let env = vcad_ir::resolve_parameters(&doc.parameters)
        .map_err(|e| DocDiffError::Resolve(e.to_string()))?;
    let theta0 = *env
        .get(parameter)
        .ok_or_else(|| DocDiffError::UnknownParameter(parameter.to_string()))?;

    let base = solids_at(doc, parameter, theta0, &options)?;
    if base.is_empty() {
        return Err(DocDiffError::NoSolidParts);
    }

    // Pre-validate the synthesis probes: both must evaluate cleanly and hold
    // the part count, because the synthesis closure below cannot propagate
    // errors (`synthesize_seeding` takes an infallible build function).
    for probe in [theta0 - probe_step, theta0 + probe_step] {
        let parts = solids_at(doc, parameter, probe, &options)?;
        if parts.len() != base.len() {
            return Err(DocDiffError::PartCountChanged {
                base: base.len(),
                probe: parts.len(),
            });
        }
    }

    let mut out = Vec::with_capacity(base.len());
    for (i, brep) in base.iter().enumerate() {
        // Build closure for this part alone. Probe failures were ruled out
        // above, so the expects are unreachable under the validated contract.
        let build = |theta: &[f64]| -> BRepSolid {
            solids_at(doc, parameter, theta[0], &options)
                .expect("probe evaluation validated above")
                .into_iter()
                .nth(i)
                .expect("part count validated above")
        };
        let seeding = synthesize_seeding(&build, &[theta0], 0, probe_step)?;
        let plan = capture_plan(brep, tess).map_err(DiffError::from)?;
        let seam = evaluate_with_sensitivity(brep, &plan, &seeding)?;
        let (properties, derivative) = mass_properties_with_derivative(&seam, density);
        out.push(DocumentPartGradient {
            part_index: i,
            properties,
            derivative,
        });
    }
    Ok(out)
}
