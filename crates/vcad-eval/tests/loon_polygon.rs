//! `[polygon outer holes]` — a router-cut plate as ONE sketch + ONE extrude.
//!
//! The point of the form is that a plate with bolt holes is a single clean
//! profile: no per-segment `[line ...]` soup, no `[difference]` of extruded
//! cutters (and its coplanar-cap boolean warnings). These tests pin that the
//! loon form lands as one `Sketch2D` carrying `holes`, and that the kernel
//! extrudes it to the right solid.

use std::f64::consts::PI;
use vcad_eval::{evaluate_document, EvalOptions};
use vcad_ir::CsgOp;
use vcad_loon::eval_vcad;

fn solid_volume(src: &str) -> f64 {
    let doc = eval_vcad(src, None).expect("loon eval");
    let scene = evaluate_document(&doc, &EvalOptions::default()).expect("kernel eval");
    assert!(scene.failures.is_empty(), "failures: {:?}", scene.failures);
    scene.parts[0]
        .solid
        .as_ref()
        .expect("a sketch extrude evaluates to a BRep solid")
        .volume()
        .abs()
}

/// 120 x 60 x 3.2 aluminium plate, a notched corner, four M3 clearance holes
/// (true circles) and one square cable pass-through.
const PLATE: &str = r#"
[extrude 0.0 0.0 3.2
  [sketch 0.0 0.0 0.0  1.0 0.0 0.0  0.0 1.0 0.0
    #[[polygon #[0.0 0.0  120.0 0.0  120.0 50.0  110.0 60.0  0.0 60.0]
               #[#[6.0 6.0 1.6] #[114.0 6.0 1.6] #[6.0 54.0 1.6] #[100.0 50.0 1.6]
                 #[50.0 20.0  70.0 20.0  70.0 40.0  50.0 40.0]]]]]]
"#;

#[test]
fn plate_with_holes_is_one_sketch_and_one_extrude() {
    let doc = eval_vcad(PLATE, None).unwrap();
    assert_eq!(doc.nodes.len(), 2, "sketch + extrude, no boolean cutters");
    let sketch = doc
        .nodes
        .values()
        .find_map(|n| match &n.op {
            CsgOp::Sketch2D {
                segments, holes, ..
            } => Some((segments.len(), holes.clone())),
            _ => None,
        })
        .expect("a Sketch2D node");
    assert_eq!(sketch.0, 5, "outer pentagon = 5 lines");
    let holes = sketch.1.expect("holes carried on the sketch");
    assert_eq!(holes.len(), 5);
    assert_eq!(holes[0].len(), 2, "a #[cx cy r] hole is two arcs");
    assert_eq!(holes[4].len(), 4, "the square cut-out is four lines");
}

#[test]
fn plate_with_holes_extrudes_to_the_right_volume() {
    let outer = 120.0 * 60.0 - 0.5 * 10.0 * 10.0;
    let cut = 4.0 * PI * 1.6 * 1.6 + 20.0 * 20.0;
    let expected = (outer - cut) * 3.2;
    let v = solid_volume(PLATE);
    assert!(
        (v - expected).abs() < 2e-3 * expected,
        "plate volume {v} != {expected}"
    );
}

#[test]
fn hole_free_polygon_extrudes_and_closing_point_is_optional() {
    let open = solid_volume(
        "[extrude 0.0 0.0 2.0 [sketch 0.0 0.0 0.0 1.0 0.0 0.0 0.0 1.0 0.0 \
         #[[polygon #[0.0 0.0 10.0 0.0 10.0 5.0 0.0 5.0]]]]]",
    );
    let closed = solid_volume(
        "[extrude 0.0 0.0 2.0 [sketch 0.0 0.0 0.0 1.0 0.0 0.0 0.0 1.0 0.0 \
         #[[polygon #[0.0 0.0 10.0 0.0 10.0 5.0 0.0 5.0 0.0 0.0]]]]]",
    );
    assert!((open - 100.0).abs() < 1e-6, "{open}");
    assert!((closed - 100.0).abs() < 1e-6, "{closed}");
}

#[test]
fn polygon_sections_loft() {
    let src = "[loft #[[sketch 0.0 0.0 0.0 1.0 0.0 0.0 0.0 1.0 0.0 \
                  #[[polygon #[0.0 0.0 10.0 0.0 10.0 10.0 0.0 10.0]]]] \
                [sketch 0.0 0.0 10.0 1.0 0.0 0.0 0.0 1.0 0.0 \
                  #[[polygon #[0.0 0.0 10.0 0.0 10.0 10.0 0.0 10.0]]]]]]";
    let v = solid_volume(src);
    assert!((v - 1000.0).abs() < 1.0, "lofted prism volume {v}");
}

#[test]
fn polygon_mixed_with_other_segments_is_refused() {
    let err = eval_vcad(
        "[extrude 0.0 0.0 1.0 [sketch 0.0 0.0 0.0 1.0 0.0 0.0 0.0 1.0 0.0 \
         #[[polygon #[0.0 0.0 1.0 0.0 1.0 1.0]] [line 0.0 0.0 1.0 1.0]]]]",
        None,
    )
    .unwrap_err();
    assert!(err.contains("must hold nothing else"), "{err}");
}

#[test]
fn degenerate_polygon_is_refused() {
    let err = eval_vcad(
        "[extrude 0.0 0.0 1.0 [sketch 0.0 0.0 0.0 1.0 0.0 0.0 0.0 1.0 0.0 \
         #[[polygon #[0.0 0.0 1.0 0.0]]]]]",
        None,
    )
    .unwrap_err();
    assert!(err.contains("at least three"), "{err}");
}
