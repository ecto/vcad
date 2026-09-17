//! The gate the rana-60 stator has to pass: the mesh a user exports must
//! still be the part, checked against the 2D CSG of the same source.
//!
//! `#[ignore]`d because the part lives in the `rana` repo. Point
//! `VCAD_STATOR_LOON` at `cad/parts-60-cnc/stator.loon` and
//! `VCAD_STATOR_OUTLINE` at a DXF of its plan view (the 2D CSG evaluated
//! exactly — `docs/cam-fixtures/stator-outline.dxf`, or one generated
//! per-stage by the scratch `stage_truth.py`):
//!
//! ```text
//! VCAD_STATOR_LOON=…/stator.loon VCAD_STATOR_OUTLINE=…/stator-outline.dxf \
//!   cargo test -p vcad-eval --test stator_section_gate -- --ignored --nocapture
//! ```
//!
//! Heights are deliberately NOT 14.1 alone: the tab-corner defect pinches to
//! zero at mid-height, so a single mid-height section is the one place that
//! cannot see it.
//!
//! On `claude/cam-roadmap` before the export repair's shape guard this fails
//! at every height — the section does not close, with gaps of 0.0871,
//! 0.3026 and 0.4055 mm — because `repair_watertightness` deleted 478
//! triangles (the lead-notch fillet walls among them) to drive a defect count
//! down, inside a volume guard of 1%. After the guard it closes at every
//! height with a max boundary distance of 0.005 mm.

use vcad_eval::{evaluate_document, EvalOptions};
use vcad_kernel::vcad_kernel_cam::outline::{
    compare_outlines, read_dxf, section_at_z, CompareOptions, SectionOptions,
};
use vcad_loon::eval_vcad;

/// Tessellation-level agreement. Measured noise floor on this part is
/// 0.002–0.005 mm (a plain ring already scores 0.00216 against the same
/// truth), so 0.02 mm is four times the floor and the same number the CAM
/// section oracle holds a job to.
const MAX_BOUNDARY: f64 = 0.02;

#[test]
#[ignore = "needs VCAD_STATOR_LOON + VCAD_STATOR_OUTLINE (the part lives in the rana repo)"]
fn the_exported_stator_is_still_the_part() {
    let loon = std::env::var("VCAD_STATOR_LOON").expect("set VCAD_STATOR_LOON");
    let dxf_path = std::env::var("VCAD_STATOR_OUTLINE").expect("set VCAD_STATOR_OUTLINE");
    let src = std::fs::read_to_string(&loon).expect("read loon");
    let dir = std::path::Path::new(&loon)
        .parent()
        .map(|p| p.to_path_buf());
    let truth =
        read_dxf(&std::fs::read_to_string(&dxf_path).expect("read dxf")).expect("parse dxf");

    let doc = eval_vcad(&src, dir.as_deref()).expect("eval_vcad");
    let scene = evaluate_document(
        &doc,
        &EvalOptions {
            skip_clash_detection: true,
            ..Default::default()
        },
    )
    .expect("evaluate_document");
    let solid = scene.parts[0].solid.as_ref().expect("root solid");

    // The EXPORT mesh — what a user gets from STL/GLB — not the raw
    // tessellation. `Solid::to_mesh` is where `repair_export_mesh` runs.
    let mesh = solid.to_mesh(256);
    let pts: Vec<[f64; 3]> = (0..mesh.vertices.len() / 3)
        .map(|i| {
            [
                mesh.vertices[i * 3] as f64,
                mesh.vertices[i * 3 + 1] as f64,
                mesh.vertices[i * 3 + 2] as f64,
            ]
        })
        .collect();

    // Healed at the shape tolerance, not at tessellation round-off, and the
    // difference is the point. Two defects remain in this part and they are
    // different in kind:
    //
    //   * the export repair used to TEAR it — gaps of 0.09, 0.30 and 0.41 mm
    //     at the lead notch and the post fillets, surface that simply is not
    //     the part. Nothing at this tolerance hides those.
    //   * the B-rep itself leaves a 0.0151 mm crack at two of the three
    //     stadium-tab corners, where the round end's cylinder is tangent to
    //     the tab cube's side plane. That is a boolean seam defect, it is
    //     four times smaller than the agreement this gate demands, and it is
    //     what the cylinder–plane tangency work is for. When that lands, drop
    //     `heal_tolerance` to 1e-3 and this gate gets strictly stronger.
    let opts = SectionOptions {
        weld_tolerance: 1e-4,
        heal_tolerance: 0.02,
        ..SectionOptions::default()
    };

    for z in [11.4, 13.0, 16.8] {
        let outline = section_at_z(&pts, &mesh.indices, z, &opts)
            .unwrap_or_else(|e| panic!("z {z}: the exported part does not section: {e}"));
        let diff = compare_outlines(&truth, &outline, &CompareOptions::default());
        assert!(
            diff.max_boundary_distance <= MAX_BOUNDARY,
            "z {z}: the exported outline is {:.5} mm from the 2D CSG at {:?} \
             (allowed {MAX_BOUNDARY})",
            diff.max_boundary_distance,
            diff.max_boundary_at
        );
        assert_eq!(outline.regions.len(), 1, "z {z}: region count");
        assert_eq!(outline.holes().len(), 4, "z {z}: hole count");
        assert_eq!(
            outline.circular_holes(1e-3).len(),
            3,
            "z {z}: the three tapped pilots must read as circles"
        );
        // Nothing the heal closed may be tear-sized: the boolean's own
        // residual crack is 0.0151 mm, the export repair's tears were 20x
        // that.
        let worst_heal = outline
            .healed
            .iter()
            .map(|h| h.distance)
            .fold(0.0f64, f64::max);
        assert!(
            worst_heal <= MAX_BOUNDARY,
            "z {z}: the section only closed by bridging {worst_heal:.5} mm — \
             that is a tear, not a seam"
        );
    }
}
