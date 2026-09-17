//! Regression for the rana-60 stator: fillet blocks (a cube minus a cylinder
//! tangent to the body's face) unioned onto a ring one at a time.
//!
//! The kernel's analytic union mishandles the second block of a pair — it
//! meets a face the first block already trimmed along the same tangent line.
//! Sometimes that union degrades to triangle soup (slow, correct); sometimes
//! it returns a grossly wrong solid that still reports `Analytic`: here the
//! sixth operand took a 5001 mm³ body to 7314 mm³ with 298 non-manifold
//! edges, and the real 50-stage stator came out at a third of its true volume.
//!
//! Fidelity is therefore NOT what this asserts. A union must land inside its
//! volume bound (`vcad_kernel_booleans::validate::union_volume_out_of_bounds`
//! re-cuts it with the mesh boolean when it does not), and the part's volume
//! must match the closed form.

use vcad_eval::{evaluate_document, EvalOptions};
use vcad_loon::eval_vcad;

/// One stator OD tab with its two fillet blocks, in the tab's own frame.
const TAB: &str = "[translate 29.95 0.0 0.0 [translate -2.45 -3.1 11.1 [cube 4.9 6.2 6.0]]]";
const BLOCK_A: &str = "[pipe [translate 28.1596 2.8 11.1 [cube 1.35 1.2038 6.0]] \
    [difference [translate 29.5096 4.15 11.09 [cylinder-n 1.05 6.02 32]]]]";
const BLOCK_B: &str = "[pipe [translate 28.1596 -4.0038 11.1 [cube 1.35 1.2038 6.0]] \
    [difference [translate 29.5096 -4.15 11.09 [cylinder-n 1.05 6.02 32]]]]";

/// A ring carrying the tab group at four stations, every piece unioned one at
/// a time the way the rana generator emits it: 13 operands.
fn stator_like() -> String {
    let mut pipe = String::from(
        "[pipe [translate 0.0 0.0 11.1 [cylinder-n 28.75 6.0 96]] \
        [difference [translate 0.0 0.0 11.09 [cylinder-n 24.0 6.02 96]]]",
    );
    for deg in [0.0, 90.0, 180.0, 270.0] {
        for piece in [TAB, BLOCK_A, BLOCK_B] {
            pipe.push_str(&format!(" [union [rotate 0.0 0.0 {deg:.1} {piece}]]"));
        }
    }
    pipe.push(']');
    format!("[root {pipe} \"aluminum\"]")
}

#[test]
fn tangent_fillet_blocks_union_to_the_right_volume() {
    let doc = eval_vcad(&stator_like(), None).expect("eval_vcad");
    let scene = evaluate_document(
        &doc,
        &EvalOptions {
            skip_clash_detection: true,
            ..Default::default()
        },
    )
    .expect("evaluate_document");
    let solid = scene.parts[0].solid.as_ref().expect("root solid");
    // Ring + per station: the tab's material outside the ring (137.9 mm³:
    // 6 · ∫ 32.4 − √(28.75² − y²) dy over y ±3.1) and two ~1.1 mm³ fillet blocks.
    let ring = std::f64::consts::PI * (28.75f64.powi(2) - 24.0f64.powi(2)) * 6.0;
    let expected = ring + 4.0 * (137.9 + 2.0 * 1.06);
    let volume = solid.volume();
    assert!(
        (volume - expected).abs() < 0.01 * expected,
        "volume {volume:.1} mm³, expected {expected:.1} ± 1% — a union in the chain returned the wrong solid"
    );
}
