//! End-to-end measurement of the rana-60 stator, the part the boolean
//! seam work is judged on. It is not checked into this repo (it lives in
//! `rana`, `cad/parts-60-cnc/stator.loon`), so this is `#[ignore]`d and takes
//! the path from the environment:
//!
//! ```text
//! VCAD_STATOR_LOON=…/stator.loon VCAD_CACHE_DIR=$(mktemp -d) \
//!   cargo test -p vcad-eval --test stator_measure -- --ignored --nocapture
//! ```
//!
//! What it prints is the row that belongs in a before/after table: volume
//! (reference 7848–7855 mm³, grid-integrated from the CSG source), fidelity,
//! triangle count, and — the part that is still wrong — unpaired and
//! over-used edges. See `docs/boolean-multilump-union-diagnosis.md`.
//!
//! Measured on `claude/cam-roadmap` (8daf2aa6), debug profile at opt-level 2:
//! 7852.96 mm³ (+0.06 %) by `Solid::volume`, 7869.62 (+0.28 %) by the
//! divergence integral over the 256-segment tessellation — the gap is the
//! frozen per-face sampling, and both clear the ±0.5 % gate. Analytic, not
//! soup, 501 faces, 12 186 triangles, 642 unpaired and 95 over-used edges,
//! ~23 s on a machine with seven other builds running.

use vcad_eval::{evaluate_document, EvalOptions};
use vcad_kernel_booleans::mesh_report;
use vcad_kernel_tessellate::tessellate_brep;
use vcad_loon::eval_vcad;

#[test]
#[ignore = "needs VCAD_STATOR_LOON (the part lives in the rana repo)"]
fn stator_measure() {
    let path =
        std::env::var("VCAD_STATOR_LOON").expect("set VCAD_STATOR_LOON to the stator's .loon path");
    let src = std::fs::read_to_string(&path).expect("read loon");
    let dir = std::path::Path::new(&path)
        .parent()
        .map(|p| p.to_path_buf());

    let doc = eval_vcad(&src, dir.as_deref()).expect("eval_vcad");
    let start = std::time::Instant::now();
    let scene = evaluate_document(
        &doc,
        &EvalOptions {
            skip_clash_detection: true,
            ..Default::default()
        },
    )
    .expect("evaluate_document");
    let solve = start.elapsed();

    let solid = scene.parts[0].solid.as_ref().expect("root solid");
    let brep = solid.as_brep();
    let mesh = match brep {
        Some(b) => tessellate_brep(b, 256),
        None => solid.to_mesh(256),
    };
    let report = mesh_report(&mesh);
    println!(
        "stator: volume={:.2} mm³ (mesh {:.2})  fidelity={:?}  soup={:?}  faces={:?}  \
         triangles={}  open_edges={}  overused_edges={}  solve={:.1}s",
        solid.volume(),
        report.signed_volume,
        solid.fidelity(),
        brep.map(vcad_kernel_booleans::is_triangle_soup),
        brep.map(|b| b.topology.faces.len()),
        report.triangles,
        report.open_edges,
        report.overused_edges,
        solve.as_secs_f64()
    );

    // The volume half of the w1-union exit criterion, which #901 met. The
    // watertightness half is asserted by the reproducers in
    // `vcad-kernel-booleans/tests/tangent_fillet_rim_seam.rs`, which are
    // `#[ignore]`d because they still fail; asserting it here too would just
    // make this harness unusable as a measurement.
    let rel = (report.signed_volume - 7848.0).abs() / 7848.0;
    assert!(
        rel < 0.005,
        "stator volume {:.2} mm³ is {:.2}% off the 7848 mm³ reference",
        report.signed_volume,
        rel * 100.0
    );
}
