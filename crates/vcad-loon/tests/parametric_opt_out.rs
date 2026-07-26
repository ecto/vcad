//! `VCAD_LOON_NO_PARAM_RECOVERY` lives in its own test binary: it is process
//! -global state, and setting it inside the main parametric suite would race
//! the other tests running in parallel threads.

use vcad_ir::{CsgOp, Expr};

#[test]
fn recovery_can_be_skipped_for_speed() {
    // SAFETY: this test binary contains exactly one test, so nothing else is
    // reading the environment concurrently.
    unsafe { std::env::set_var("VCAD_LOON_NO_PARAM_RECOVERY", "1") };
    let (doc, warnings) = vcad_loon::eval_vcad_parametric(
        "[defparam a 5.0]\n[root [cube a 1.0 1.0] \"steel\"]",
        None,
        None,
    )
    .unwrap();
    // Declarations still land; only the binding search is skipped.
    assert_eq!(doc.parameters["a"].value, Expr::Number(5.0));
    assert!(doc.bindings.is_empty());
    assert!(warnings.is_empty());
    let size = doc
        .nodes
        .values()
        .find_map(|n| match &n.op {
            CsgOp::Cube { size } => Some(*size),
            _ => None,
        })
        .unwrap();
    assert_eq!(size.x, 5.0, "the geometry is still correct");
}
