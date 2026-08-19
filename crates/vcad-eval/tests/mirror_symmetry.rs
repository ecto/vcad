//! Regression tests for the loon mirror vocabulary (`mirror`, the `mirror-x/y/z`
//! axis sugar, and the `mirror-pattern*` / `quad-pattern` symmetric patterns).
//!
//! The invariant every case asserts is the one that catches hand-mirroring
//! bugs: build a solid at a known off-axis position, mirror it, and the
//! union's centre of mass must lie exactly on the mirror plane. A sign error
//! anywhere in the mirrored half moves the COM off the plane.

use vcad_eval::{evaluate_document, EvalOptions};
use vcad_loon::eval_vcad;

/// Tessellation resolution for the measurement oracle. Boxes are exact at any
/// resolution; keep it modest so the tests stay fast.
const SEGMENTS: u32 = 32;

/// Mesh volume and centre of mass via the divergence theorem.
fn inspect(src: &str) -> (f64, [f64; 3]) {
    let doc = eval_vcad(src, None).expect("eval_vcad");
    let scene = evaluate_document(
        &doc,
        &EvalOptions {
            skip_clash_detection: true,
            clock: None,
            root_cache: None,
            mesh_segments: 0,
        },
    )
    .expect("evaluate_document");
    let solid = scene.parts[0].solid.as_ref().expect("root solid");
    let mesh = solid.to_mesh(SEGMENTS);
    let mut vol = 0.0;
    let mut moment = [0.0; 3];
    for t in 0..mesh.indices.len() / 3 {
        let mut p = [[0.0f64; 3]; 3];
        for (k, pk) in p.iter_mut().enumerate() {
            let vi = mesh.indices[t * 3 + k] as usize;
            for (c, slot) in pk.iter_mut().enumerate() {
                *slot = mesh.vertices[vi * 3 + c] as f64;
            }
        }
        let [a, b, c] = p;
        let det = a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
            + a[2] * (b[0] * c[1] - b[1] * c[0]);
        let v = det / 6.0;
        vol += v;
        for k in 0..3 {
            moment[k] += v * (a[k] + b[k] + c[k]) / 4.0;
        }
    }
    assert!(vol.abs() > 1e-9, "degenerate solid from:\n{src}");
    (vol, [moment[0] / vol, moment[1] / vol, moment[2] / vol])
}

/// An off-axis body: a 10×10×10 cube whose centre sits at (25, 35, 45).
const BODY: &str = "[translate 20.0 30.0 40.0 [cube 10.0 10.0 10.0]]";
const BODY_VOLUME: f64 = 1000.0;
const BODY_COM: [f64; 3] = [25.0, 35.0, 45.0];

fn assert_close(got: f64, want: f64, what: &str) {
    assert!((got - want).abs() < 1e-6, "{what}: got {got}, want {want}");
}

/// The axis sugar mirrors through the origin, negating exactly one coordinate.
#[test]
fn axis_sugar_negates_one_coordinate() {
    for (form, axis) in [("mirror-x", 0), ("mirror-y", 1), ("mirror-z", 2)] {
        let (vol, com) = inspect(&format!("[{form} {BODY}]"));
        assert_close(vol, BODY_VOLUME, "mirrored volume");
        for k in 0..3 {
            let want = if k == axis { -BODY_COM[k] } else { BODY_COM[k] };
            assert_close(com[k], want, &format!("{form} com[{k}]"));
        }
    }
}

/// `[mirror-x s]` must be exactly `[mirror 0 0 0 1 0 0 s]` — the sugar is a
/// shorthand, not a different operation.
#[test]
fn axis_sugar_matches_general_form() {
    let sugar = inspect(&format!("[mirror-x {BODY}]"));
    let general = inspect(&format!("[mirror 0.0 0.0 0.0 1.0 0.0 0.0 {BODY}]"));
    assert_close(sugar.0, general.0, "volume");
    for k in 0..3 {
        assert_close(sugar.1[k], general.1[k], &format!("com[{k}]"));
    }
}

/// The invariant: a mirror pattern's COM lies on the mirror plane, and it holds
/// twice the volume. This is the assertion that catches every hand-mirroring
/// sign error.
#[test]
fn mirror_pattern_com_lies_on_the_plane() {
    for (form, axis) in [
        ("mirror-pattern-x", 0),
        ("mirror-pattern-y", 1),
        ("mirror-pattern-z", 2),
    ] {
        let (vol, com) = inspect(&format!("[{form} {BODY}]"));
        assert_close(vol, 2.0 * BODY_VOLUME, &format!("{form} volume"));
        assert_close(com[axis], 0.0, &format!("{form} com on plane"));
        for k in 0..3 {
            if k != axis {
                assert_close(com[k], BODY_COM[k], &format!("{form} com[{k}] unchanged"));
            }
        }
    }
}

/// The general `[mirror-pattern nx ny nz s]` agrees with the axis shorthand.
#[test]
fn general_mirror_pattern_matches_axis_shorthand() {
    let general = inspect(&format!("[mirror-pattern 0.0 1.0 0.0 {BODY}]"));
    let sugar = inspect(&format!("[mirror-pattern-y {BODY}]"));
    assert_close(general.0, sugar.0, "volume");
    for k in 0..3 {
        assert_close(general.1[k], sugar.1[k], &format!("com[{k}]"));
    }
}

/// `[quad-pattern s]` is 4-fold: four copies, COM on both the X and Y planes.
/// A quadruped's legs, a 4-post frame, a vehicle chassis.
#[test]
fn quad_pattern_is_four_fold_symmetric() {
    let (vol, com) = inspect(&format!("[quad-pattern {BODY}]"));
    assert_close(vol, 4.0 * BODY_VOLUME, "quad volume");
    assert_close(com[0], 0.0, "quad com.x on plane");
    assert_close(com[1], 0.0, "quad com.y on plane");
    assert_close(com[2], BODY_COM[2], "quad com.z unchanged");
}
