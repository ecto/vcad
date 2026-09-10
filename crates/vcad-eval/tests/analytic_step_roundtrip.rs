//! Acceptance: a document whose operands are all analytic must reach STEP as
//! a true B-rep, not as triangle soup.
//!
//! # What was wrong
//!
//! `vcad export part.loon part.step` wrote a tessellated mesh — thousands of
//! one-triangle `ADVANCED_FACE`s carrying `PLANE` surfaces — for any solid
//! that went through the mesh-boolean fallback, while simple analytic solids
//! got real B-rep. On the rana-60-cnc set that meant a milled can at 214 213
//! faces and a pocketed rotor at 190 468, both of them unusable as CNC
//! deliverables: CAM cannot put a tool on a facet, and the can even exceeded
//! `vcad import`'s own 200 000-face cap, so it did not round-trip through
//! vcad itself.
//!
//! The trigger was never the writer, which has no mesh path at all. It was
//! the evaluator cutting `n` pockets as `n` chained differences, each one
//! re-trimming faces the last had already trimmed, until a splitter produced
//! a result the boolean's crack gate condemned — after which every remaining
//! cut inherited the soup.
//!
//! # What is asserted
//!
//! For each fixture: the evaluated root is [`SolidFidelity::Analytic`], every
//! surface in its geometry store is a `Plane` or a `Cylinder`, the STEP it
//! writes parses back in, and the round-tripped volume agrees with the
//! original to within [`VOLUME_TOLERANCE`].
//!
//! # Failing before the fix
//!
//! `disc_with_twenty_pockets_stays_analytic` is the one that names the bug.
//! On pre-fix main the disc came out `TriangleSoup` at 12 174 planar faces;
//! the other three fixtures passed there already and are here to keep them
//! passing.

use std::time::Instant;

use vcad_eval::{evaluate_document, EvalOptions};
use vcad_kernel::{Solid, SolidFidelity};
use vcad_kernel_geom::SurfaceKind;
use vcad_kernel_step::{read_step_from_buffer, write_step_to_buffer};
use vcad_loon::eval_vcad;

/// Fractional volume difference tolerated across a STEP round trip.
///
/// The issue asks for 0.5 %. Nothing here should come close: every fixture is
/// planes and cylinders, and the reader rebuilds both exactly. The margin is
/// for the tessellation the volume oracle does on curved faces, not for the
/// representation.
const VOLUME_TOLERANCE: f64 = 0.005;

fn evaluate(src: &str) -> Solid {
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
    scene.parts[0].solid.as_ref().expect("root solid").clone()
}

/// Evaluate `src`, then assert everything the issue asks for.
///
/// `max_faces` is an upper bound on `ADVANCED_FACE` count, not a prediction:
/// it is set well above what each shape currently produces and well below the
/// five-figure counts the mesh fallback produced, so it catches a regression
/// back to facets without churning every time the splitters fragment a face
/// differently.
fn assert_analytic_roundtrip(name: &str, src: &str, max_faces: usize) {
    let solid = evaluate(src);

    assert_eq!(
        solid.fidelity(),
        SolidFidelity::Analytic,
        "{name}: expected a true B-rep, got {:?}: {}",
        solid.fidelity(),
        solid
            .why_not_brep()
            .unwrap_or_else(|| "no reason recorded".to_string()),
    );

    let brep = solid.as_brep().expect("analytic solid must carry a BRep");

    let offenders: Vec<SurfaceKind> = brep
        .geometry
        .surfaces
        .iter()
        .map(|s| s.surface_type())
        .filter(|k| !matches!(k, SurfaceKind::Plane | SurfaceKind::Cylinder))
        .collect();
    assert!(
        offenders.is_empty(),
        "{name}: every surface should be PLANE or CYLINDRICAL_SURFACE, found {offenders:?}",
    );

    let face_count = brep.topology.faces.len();
    assert!(
        face_count <= max_faces,
        "{name}: {face_count} faces exceeds the {max_faces} allowed — the writer is \
         emitting facets again",
    );

    let buffer = write_step_to_buffer(brep).expect("STEP write");
    let solids = read_step_from_buffer(&buffer).expect("STEP read");
    assert_eq!(solids.len(), 1, "{name}: expected exactly one solid back");

    let before = solid.volume();
    let after = Solid::from_brep(solids.into_iter().next().unwrap()).volume();
    let drift = (after - before).abs() / before.abs();
    assert!(
        drift <= VOLUME_TOLERANCE,
        "{name}: volume moved {:.4} % across the round trip ({before} -> {after})",
        drift * 100.0,
    );
}

/// `cylinder - cylinder`: the simplest thing that has to stay analytic.
#[test]
fn tube_stays_analytic() {
    assert_analytic_roundtrip(
        "tube",
        "[pipe [cylinder-n 20.0 40.0 64] \
           [difference [translate 0.0 0.0 -1.0 [cylinder-n 14.0 42.0 64]]]]",
        16,
    );
}

/// `cylinder u cylinder`, coaxial: a turned step shaft.
#[test]
fn coaxial_union_stays_analytic() {
    assert_analytic_roundtrip(
        "coaxial union",
        "[pipe [cylinder-n 20.0 10.0 64] \
           [union [translate 0.0 0.0 10.0 [cylinder-n 12.0 25.0 64]]]]",
        16,
    );
}

/// A tube with three radial slots milled through the wall — cylinders and
/// boxes in one chain.
#[test]
fn slotted_tube_stays_analytic() {
    assert_analytic_roundtrip(
        "slotted tube",
        "[pipe [cylinder-n 20.0 40.0 64] \
           [difference [translate 0.0 0.0 -1.0 [cylinder-n 14.0 42.0 64]]] \
           [difference [rotate 0.0 0.0 0.0 \
             [translate 12.0 -3.0 10.0 [cube 12.0 6.0 20.0]]]] \
           [difference [rotate 0.0 0.0 120.0 \
             [translate 12.0 -3.0 10.0 [cube 12.0 6.0 20.0]]]] \
           [difference [rotate 0.0 0.0 240.0 \
             [translate 12.0 -3.0 10.0 [cube 12.0 6.0 20.0]]]]]",
        256,
    );
}

/// Twenty rectangular pockets milled into one face of a disc.
///
/// This is the fixture the fix exists for, and the only one that failed
/// before it. Twenty chained differences all re-trim the same top plane; the
/// fourth used to produce a result the crack gate condemned, and the sixteen
/// after it inherited triangle soup. Recorded on pre-fix main: `TriangleSoup`,
/// 12 174 `ADVANCED_FACE`, every one of them a `PLANE`.
#[test]
fn disc_with_twenty_pockets_stays_analytic() {
    assert_analytic_roundtrip("disc with 20 pockets", &pocketed_disc(), 2_000);
}

/// Twenty rectangular pockets on a bolt circle, cut from one disc in one
/// left-leaning difference chain.
fn pocketed_disc() -> String {
    let mut src = String::from("[pipe [cylinder-n 40.0 10.0 96]");
    for i in 0..20 {
        let deg = 360.0 * f64::from(i) / 20.0;
        src.push_str(&format!(
            " [difference [rotate 0.0 0.0 {deg:.4} \
               [translate 25.0 0.0 0.0 \
                 [translate -4.0 -2.5 6.0 [cube 8.0 5.0 5.0]]]]]",
        ));
    }
    src.push(']');
    src
}

/// Wall-clock ceiling for the pocketed disc, evaluation through STEP write.
///
/// Deliberately enormous next to what this actually costs — about a second
/// in release, a handful in a debug test binary — because it is not a
/// performance assertion, it is a hang detector. The regression it exists
/// for is a batched difference whose cost is unbounded in the tool set: on
/// the rana-60-cnc rotor an unbounded batch ran past 30 minutes where the
/// chained path took 339 s. A bound that tracked the real number would flake
/// on a loaded CI runner; a bound of three minutes cannot be reached by any
/// amount of ordinary slowness and is reached instantly by an unbounded
/// batch.
const TIME_BUDGET_SECS: u64 = 180;

/// The pocketed disc must stay analytic *and* stay fast.
///
/// `cut_chain` batches `A - B - C - ...` into `A - (B u C u ...)`, which is
/// what makes this fixture analytic at all — but a fused tool costs more the
/// more faces it carries, so the batching is capped by tool count, by fused
/// face count, and by a wall-clock budget, and falls back to the plain chain
/// whenever a cap is hit. This test pins the two halves of that policy
/// together: removing the caps would keep it passing on fidelity and blow
/// the time bound on bigger parts; removing the batching would keep it fast
/// and fail on fidelity.
#[test]
fn disc_with_twenty_pockets_exports_in_bounded_time() {
    let start = Instant::now();

    let solid = evaluate(&pocketed_disc());
    assert_eq!(
        solid.fidelity(),
        SolidFidelity::Analytic,
        "pocketed disc lost its B-rep: {}",
        solid
            .why_not_brep()
            .unwrap_or_else(|| "no reason recorded".to_string()),
    );
    write_step_to_buffer(solid.as_brep().expect("analytic solid carries a BRep"))
        .expect("STEP write");

    let elapsed = start.elapsed();
    assert!(
        elapsed.as_secs() < TIME_BUDGET_SECS,
        "pocketed disc took {elapsed:?}, over the {TIME_BUDGET_SECS} s budget — \
         the difference batching is no longer cost-bounded",
    );
}
