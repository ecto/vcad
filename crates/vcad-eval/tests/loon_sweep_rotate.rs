//! End-to-end: a `sweep-rotate` clearance cut evaluates through loon → IR →
//! kernel and removes the swept volume from the body.

use vcad_eval::{evaluate_document, EvalOptions};
use vcad_loon::eval_vcad;

fn volume(src: &str) -> f64 {
    let doc = eval_vcad(src, None).expect("eval_vcad");
    let scene = evaluate_document(
        &doc,
        &EvalOptions {
            skip_clash_detection: true,
            clock: None,
            root_cache: None,
            mesh_segments: 0,
            on_root: None,
        },
    )
    .expect("evaluate_document");
    scene.parts[0].solid.as_ref().expect("root solid").volume()
}

#[test]
fn quarter_swing_relief_removes_the_swept_sector() {
    // A 40 mm bar swinging 0..90° about Z through the block's corner sweeps
    // a quarter disc of radius ~40 (far corner at √(40²+5²) ≈ 40.3) through
    // the full 20 mm block height.
    let body = 100.0 * 100.0 * 20.0;
    let v = volume(
        "[pipe [cube 100 100 20] \
           [difference [sweep-rotate 0 0 0  0 0 1  0 90 \
                         [translate 0 -5 -5 [cube 40 10 30]]]]]",
    );
    let removed = body - v;
    let lo = std::f64::consts::PI * 40.0 * 40.0 / 4.0 * 20.0 * 0.97;
    let hi = std::f64::consts::PI * 40.4 * 40.4 / 4.0 * 20.0 * 1.03;
    assert!(
        removed > lo && removed < hi,
        "removed {removed} mm³, want within [{lo}, {hi}]"
    );
}
