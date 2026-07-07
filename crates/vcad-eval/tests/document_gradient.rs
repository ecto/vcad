//! Document-parameter gradients — the M6 stretch goal, unlocked.
//!
//! A `.vcad` document with a named parameter differentiates end to end:
//! parameter → bindings → document evaluation → BRep → synthesized seeding →
//! seam → `d(mass properties)/dθ`. Two gates:
//!
//! 1. **Parametric IR document**: a cylinder whose radius is bound to
//!    parameter `"r"`. `dV/dr` against the N-gon prism closed form
//!    `2·k·r·h` (the capture tessellation's own discrete truth, machine
//!    precision) and the inertia derivative against a rebuild central FD.
//! 2. **Loon-authored document**: the same part authored as loon source,
//!    evaluated through `vcad_loon::eval_vcad`, parameterized after the
//!    fact — proving documents from the loon path reach the same seam.

use vcad_eval::diff::{document_parameter_gradient, document_parameter_qoi_gradient, DocDiffError};
use vcad_eval::{evaluate_document, EvalOptions};
use vcad_ir::{Bindings, CsgOp, Document, Expr, MaterialDef, Node, Parameter, SceneEntry};
use vcad_kernel_diff::mass_properties;
use vcad_kernel_tessellate::frozen::capture_plan;
use vcad_kernel_tessellate::TessellationParams;

const R0: f64 = 10.0;
const HEIGHT: f64 = 8.0;
const SEGS: u32 = 64;

fn tess() -> TessellationParams {
    TessellationParams {
        circle_segments: SEGS,
        height_segments: 2,
        ..Default::default()
    }
}

/// N-gon prism factor: V = k·r²·h.
fn kfac() -> f64 {
    let n = SEGS as f64;
    0.5 * n * (2.0 * std::f64::consts::PI / n).sin()
}

fn parametric_cylinder_doc(r: f64) -> Document {
    let mut doc = Document::new();
    doc.nodes.insert(
        1,
        Node {
            id: 1,
            name: Some("disc".to_string()),
            op: CsgOp::Cylinder {
                radius: 1.0, // overwritten by the binding at resolve time
                height: HEIGHT,
                segments: SEGS,
            },
        },
    );
    doc.roots.push(SceneEntry {
        root: 1,
        material: "default".to_string(),
        visible: None,
    });
    doc.materials.insert(
        "default".to_string(),
        MaterialDef {
            name: "default".to_string(),
            color: [0.8, 0.8, 0.8],
            metallic: 0.0,
            roughness: 0.5,
            density: None,
            friction: None,
            ..Default::default()
        },
    );
    doc.parameters
        .insert("r".to_string(), Parameter::literal(r));
    doc.bindings.bind(
        vcad_ir::BindingKey {
            node_id: 1,
            field_path: "radius".to_string(),
        },
        Expr::formula("r"),
    );
    doc
}

/// Mass properties of the document's single part at its current parameters,
/// via a fresh evaluation + capture (the rebuild oracle).
fn oracle_props(doc: &Document, density: f64) -> vcad_kernel_diff::MassProperties<f64> {
    let scene = evaluate_document(doc, &EvalOptions::default()).expect("evaluate");
    let brep = scene.parts[0]
        .solid
        .as_ref()
        .and_then(|s| s.as_brep())
        .expect("solid part");
    let plan = capture_plan(brep, &tess()).expect("capture");
    let seam = vcad_kernel_diff::evaluate_with_sensitivity(
        brep,
        &plan,
        &vcad_kernel_diff::ParamSeeding::new(),
    )
    .expect("seam");
    mass_properties(&seam.positions, &seam.triangles, density)
}

#[test]
fn parametric_document_cylinder_gradient() {
    const DENSITY: f64 = 2.0;
    let doc = parametric_cylinder_doc(R0);
    let grads = document_parameter_gradient(&doc, "r", DENSITY, &tess(), 1e-4).expect("gradient");
    assert_eq!(grads.len(), 1, "one solid part");
    let g = &grads[0];

    // Closed forms for the N-gon prism: V = k r² h.
    let k = kfac();
    let v_exact = k * R0 * R0 * HEIGHT;
    let dv_exact = 2.0 * k * R0 * HEIGHT;
    assert!(
        (g.properties.volume - v_exact).abs() / v_exact < 1e-12,
        "V {} vs {v_exact}",
        g.properties.volume
    );
    let rel = (g.derivative.volume - dv_exact).abs() / dv_exact;
    assert!(
        rel <= 1e-9,
        "dV/dr {} vs closed form {dv_exact} (rel {rel:.3e})",
        g.derivative.volume
    );
    // Mass scales by density; centroid is r-invariant.
    assert!((g.derivative.mass - DENSITY * dv_exact).abs() / (DENSITY * dv_exact) <= 1e-9);
    assert!(
        g.derivative.centroid.x.abs() < 1e-9
            && g.derivative.centroid.y.abs() < 1e-9
            && g.derivative.centroid.z.abs() < 1e-9,
        "centroid of a centered disc must not move with r: {:?}",
        g.derivative.centroid
    );

    // Inertia derivative vs a document-rebuild central FD (h = 1e-5).
    let h = 1e-5;
    let plus = oracle_props(&parametric_cylinder_doc(R0 + h), DENSITY);
    let minus = oracle_props(&parametric_cylinder_doc(R0 - h), DENSITY);
    let fd_izz = (plus.inertia_origin[2][2] - minus.inertia_origin[2][2]) / (2.0 * h);
    let rel_izz = (g.derivative.inertia_origin[2][2] - fd_izz).abs() / fd_izz.abs();
    assert!(
        rel_izz <= 1e-6,
        "dIzz/dr {} vs rebuild FD {fd_izz} (rel {rel_izz:.3e})",
        g.derivative.inertia_origin[2][2]
    );
}

#[test]
fn qoi_gradient_matches_central_finite_differences() {
    // The MCP/WASM QoI bundle: analytic volume/mass/centroid derivatives must
    // match a central finite difference of the bundle's own reported values,
    // and the (finite-difference) bbox extents must be internally consistent.
    const DENSITY: f64 = 3.0;
    let h = 1e-3;
    let qoi = |r: f64| {
        document_parameter_qoi_gradient(&parametric_cylinder_doc(r), "r", DENSITY, &tess(), 1e-4)
            .expect("qoi gradient")
    };

    let base = qoi(R0);
    assert_eq!(base.len(), 1, "one solid part");
    let g = &base[0];

    let plus = qoi(R0 + h);
    let minus = qoi(R0 - h);

    // Analytic dVolume/dr vs central FD of the bundle's own volume.
    let fd_dv = (plus[0].volume - minus[0].volume) / (2.0 * h);
    let rel = (g.d_volume - fd_dv).abs() / fd_dv.abs();
    assert!(
        rel <= 1e-4,
        "dVolume/dr {} vs central FD {fd_dv} (rel {rel:.3e})",
        g.d_volume
    );
    // Mass tracks density · volume.
    assert!((g.mass - DENSITY * g.volume).abs() / (DENSITY * g.volume) <= 1e-12);
    let fd_dm = (plus[0].mass - minus[0].mass) / (2.0 * h);
    assert!((g.d_mass - fd_dm).abs() / fd_dm.abs() <= 1e-4);

    // A centered disc's centroid is r-invariant.
    assert!(
        g.d_centroid.iter().all(|c| c.abs() < 1e-6),
        "centroid must not move with r: {:?}",
        g.d_centroid
    );

    // A 64-gon disc starting at angle 0 spans [-r, r] on both x and y, so the
    // bbox x/y extents are 2r (grow at 2·dr) and z is the fixed height.
    let _ = (&plus, &minus);
    assert!((g.bbox_extents[0] - 2.0 * R0).abs() < 1e-2, "x extent ≈ 2r");
    assert!(
        (g.bbox_extents[2] - HEIGHT).abs() < 1e-2,
        "z extent ≈ height"
    );
    assert!(
        (g.d_bbox_extents[0] - 2.0).abs() < 1e-2,
        "d(x extent)/dr ≈ 2: {}",
        g.d_bbox_extents[0]
    );
    assert!(
        (g.d_bbox_extents[1] - 2.0).abs() < 1e-2,
        "d(y extent)/dr ≈ 2: {}",
        g.d_bbox_extents[1]
    );
    assert!(
        g.d_bbox_extents[2].abs() < 1e-2,
        "z extent (height) is r-invariant: {}",
        g.d_bbox_extents[2]
    );
}

#[test]
fn unknown_parameter_is_an_error() {
    let doc = parametric_cylinder_doc(R0);
    let err = document_parameter_gradient(&doc, "nope", 1.0, &tess(), 1e-4);
    assert!(matches!(err, Err(DocDiffError::UnknownParameter(_))));
}

#[test]
fn loon_authored_document_gradient() {
    // Author the part in loon, evaluate to a Document through the sibling
    // interpreter, then parameterize the produced node — the full loon path
    // the M6 note deferred.
    let source = format!("[root [cylinder {R0} {HEIGHT}] \"steel\"]");
    let mut doc = vcad_loon::eval_vcad(&source, None).expect("loon eval");

    // Find the cylinder node the loon program produced and bind its radius.
    let cyl_id = *doc
        .nodes
        .iter()
        .find(|(_, n)| matches!(n.op, CsgOp::Cylinder { .. }))
        .expect("loon program produced a cylinder")
        .0;
    doc.parameters
        .insert("r".to_string(), Parameter::literal(R0));
    if doc.bindings.is_empty() {
        doc.bindings = Bindings::new();
    }
    doc.bindings.bind(
        vcad_ir::BindingKey {
            node_id: cyl_id,
            field_path: "radius".to_string(),
        },
        Expr::formula("r"),
    );

    let grads = document_parameter_gradient(&doc, "r", 1.0, &tess(), 1e-4).expect("gradient");
    assert_eq!(grads.len(), 1);
    let dv = grads[0].derivative.volume;
    let dv_exact = 2.0 * kfac() * R0 * HEIGHT;
    let rel = (dv - dv_exact).abs() / dv_exact;
    assert!(
        rel <= 1e-9,
        "loon-authored dV/dr {dv} vs closed form {dv_exact} (rel {rel:.3e})"
    );
}
