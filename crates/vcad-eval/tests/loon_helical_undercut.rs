//! Loon CSG of a helical through-wall slot must evaluate to a closed
//! manifold shell (issue #840). The full six-slot rana-scale example lives
//! at `hardware/helical-slot-tube/shell.loon`; this test uses one channel
//! so it stays cheap enough for the eval suite.

use vcad_eval::{evaluate_document, EvalOptions};
use vcad_loon::eval_vcad;

const ONE_SLOT: &str = r#"
[let tube [difference [translate 0 0 -1 [cylinder 16.0 22.0]] [cylinder 18.0 20.0]]]
[let slot-sk [sketch 0 0 0 1 0 0 0 1 0 #[
  [line -4 -2 6 -2]
  [line 6 -2 6 2]
  [line 6 2 -4 2]
  [line -4 2 -4 -2]]]]
[let cam [translate 0 0 7 [sweep-helix 17.0 40.0 6.0 0.15 slot-sk]]]
[root [difference cam tube] "petg"]
"#;

#[test]
fn loon_helical_slot_in_tube_is_manifold() {
    let doc = eval_vcad(ONE_SLOT, None).expect("eval_vcad");
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
    let mesh = solid.to_mesh(24);
    assert_eq!(
        mesh.welded_defective_edge_count(),
        0,
        "helical-slot tube is not a closed manifold"
    );
    let vol = solid.volume();
    let tube_vol = std::f64::consts::PI * (18.0_f64.powi(2) - 16.0_f64.powi(2)) * 20.0;
    assert!(
        vol > 10.0 && vol < tube_vol - 5.0,
        "cut volume {vol} should be a slotted tube (bare tube ~{tube_vol})"
    );
}
