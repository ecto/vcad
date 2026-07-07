//! Regression: STEP writer must accept BReps produced by boolean operations.
//!
//! Before this test, the writer rejected every Difference result with
//! `Invalid topology: half-edge has no parent edge`, because the sewing
//! step left some half-edges without a parent edge link. Mecheval's
//! step_roundtrip check failed on every cube-minus-cylinder shape it
//! evaluated.

use vcad_kernel_booleans::{boolean_op, BooleanOp};
use vcad_kernel_primitives::{make_cube, make_cylinder};
use vcad_kernel_step::{read_step_from_buffer, write_step_to_buffer};

#[test]
fn write_step_after_difference() {
    let cube = make_cube(20.0, 20.0, 20.0);
    let cyl = make_cylinder(5.0, 30.0, 32);
    let result =
        boolean_op(&cube, &cyl, BooleanOp::Difference, 32).expect("boolean should succeed");
    let brep = result.as_brep().expect("Difference should yield BRep");

    let buffer = write_step_to_buffer(brep).expect("STEP write should succeed");
    assert!(!buffer.is_empty());
}

#[test]
fn write_step_after_difference_round_trips() {
    // The writer used to reject every Difference output with
    // `InvalidTopology`; ensure not just that write succeeds but that the
    // result parses back into at least one solid.
    let cube = make_cube(20.0, 20.0, 20.0);
    let cyl = make_cylinder(5.0, 30.0, 32);
    let result =
        boolean_op(&cube, &cyl, BooleanOp::Difference, 32).expect("boolean should succeed");
    let brep = result.as_brep().expect("Difference should yield BRep");

    let buffer = write_step_to_buffer(brep).expect("STEP write should succeed");
    let solids = read_step_from_buffer(&buffer).expect("STEP read should succeed");
    assert!(
        !solids.is_empty(),
        "expected at least one solid round-tripped"
    );
}

#[test]
fn write_step_after_union_overlap() {
    let a = make_cube(20.0, 20.0, 20.0);
    let mut b = make_cube(20.0, 20.0, 20.0);
    for (_, v) in &mut b.topology.vertices {
        v.point.x += 10.0;
    }
    let result = boolean_op(&a, &b, BooleanOp::Union, 32).expect("boolean should succeed");
    let brep = result.as_brep().expect("Union should yield BRep");

    let buffer = write_step_to_buffer(brep).expect("STEP write should succeed");
    assert!(!buffer.is_empty());
}
