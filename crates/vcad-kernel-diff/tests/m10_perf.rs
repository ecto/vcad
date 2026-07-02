//! M10 — performance: spatial-hash accelerator for the evaluation-time
//! vertex resolution.
//!
//! Profiling (the timing test below) settled where the seam actually spends
//! its time: capture, seam, and pullback are all **linear in node count**
//! (the cross-face dedup was already spatial-hashed in the M0–M2 hardening,
//! and the per-face topology-vertex classification is bounded by a face's
//! boundary loop, not by tessellation density — grid-accelerating it measured
//! *slower*, so it was left alone). The one genuinely `O(nodes · vertices)`
//! scan is the **evaluation-time** (`evaluate_plan`, the FD oracle) resolution
//! of each frozen node to its nearest rebuilt vertex. M10 grids that.
//!
//! Two things here:
//!
//! 1. **Bit-identical equivalence** — [`evaluate_plan`] (grid) vs
//!    [`evaluate_plan_naive`] (linear), on the rounded cube and a
//!    flywheel-class drilled disc, asserted byte-for-byte.
//! 2. **Timing evidence** — a non-criterion harness printing capture / seam /
//!    pullback (to show their linearity) and `evaluate_plan` naive-vs-grid (to
//!    show the win), at `circle_segments ∈ {16, 64, 128}`. No timing
//!    assertions.

use std::time::Instant;

use vcad_kernel::Solid;
use vcad_kernel_diff::{
    evaluate_with_pullback, evaluate_with_sensitivity, volume_gradient, ParamSeeding, SurfaceSeed,
};
use vcad_kernel_geom::{CylinderSurface, SphereSurface};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::{make_cube, BRepSolid};
use vcad_kernel_tessellate::frozen::{
    capture_plan, evaluate_plan, evaluate_plan_naive, FrozenMesh,
};
use vcad_kernel_tessellate::TessellationParams;

// -------------------------------------------------------------- the models

fn rounded_cube() -> BRepSolid {
    vcad_kernel_fillet::fillet_all_edges(&make_cube(10.0, 10.0, 10.0), 1.5)
}

/// A flywheel-class part: a disc with a center bore and four lightening holes
/// (all boolean cuts, so the topology carries many polygonized rim vertices —
/// the worst case for the per-node nearest-vertex resolution).
fn flywheel() -> BRepSolid {
    let disc = Solid::cylinder(20.0, 6.0, 64);
    let bore = Solid::cylinder(5.0, 8.0, 48).translate(0.0, 0.0, -1.0);
    let mut body = disc.difference(&bore);
    for (dx, dy) in [(11.0, 0.0), (-11.0, 0.0), (0.0, 11.0), (0.0, -11.0)] {
        let hole = Solid::cylinder(3.0, 8.0, 32).translate(dx, dy, -1.0);
        body = body.difference(&hole);
    }
    body.as_brep().expect("flywheel stays BRep").clone()
}

/// The M4/M5 fillet-radius seeding on the rounded cube (a representative
/// composite seeding to price the seam / pullback against).
fn rc_seeding(brep: &BRepSolid) -> ParamSeeding {
    let r = brep
        .geometry
        .surfaces
        .iter()
        .find_map(|s| {
            s.as_any()
                .downcast_ref::<CylinderSurface>()
                .map(|c| c.radius)
        })
        .expect("blend cylinders");
    let a = 10.0;
    let retreat = |center: Point3| {
        let comp = |c: f64| {
            if (c - r).abs() < 1e-9 {
                1.0
            } else if (c - (a - r)).abs() < 1e-9 {
                -1.0
            } else {
                0.0
            }
        };
        Vec3::new(comp(center.x), comp(center.y), comp(center.z))
    };
    let mut seeding = ParamSeeding::new();
    for (i, s) in brep.geometry.surfaces.iter().enumerate() {
        if let Some(c) = s.as_any().downcast_ref::<CylinderSurface>() {
            seeding.seed(i, SurfaceSeed::CylinderRadius { rate: 1.0 });
            seeding.seed(
                i,
                SurfaceSeed::Translate {
                    velocity: retreat(c.center),
                },
            );
        } else if let Some(sp) = s.as_any().downcast_ref::<SphereSurface>() {
            seeding.seed(i, SurfaceSeed::SphereRadius { rate: 1.0 });
            seeding.seed(
                i,
                SurfaceSeed::Translate {
                    velocity: retreat(sp.center),
                },
            );
        }
    }
    seeding
}

// ----------------------------------------------------- equivalence (gated)

/// Assert two evaluated meshes are bit-identical: same triangles, same node
/// positions to the bit (both paths run identical float arithmetic).
fn assert_meshes_identical(grid: &FrozenMesh, naive: &FrozenMesh, what: &str) {
    assert_eq!(grid.triangles, naive.triangles, "{what}: triangles");
    assert_eq!(
        grid.positions.len(),
        naive.positions.len(),
        "{what}: node count"
    );
    for (i, (a, b)) in grid.positions.iter().zip(&naive.positions).enumerate() {
        assert!(
            a.x.to_bits() == b.x.to_bits()
                && a.y.to_bits() == b.y.to_bits()
                && a.z.to_bits() == b.z.to_bits(),
            "{what}: position {i} differs: {a:?} vs {b:?}"
        );
    }
}

#[test]
fn m10_grid_evaluation_is_bit_identical_to_linear() {
    for segs in [16u32, 64, 128] {
        let params = TessellationParams {
            circle_segments: segs,
            height_segments: 2,
            ..Default::default()
        };
        let rc = rounded_cube();
        let plan = capture_plan(&rc, &params).expect("capture");
        let grid = evaluate_plan(&rc, &plan).expect("grid eval");
        let naive = evaluate_plan_naive(&rc, &plan).expect("naive eval");
        assert_meshes_identical(&grid, &naive, &format!("rounded cube @ {segs}"));
    }

    let fw = flywheel();
    let params = TessellationParams {
        circle_segments: 64,
        height_segments: 1,
        ..Default::default()
    };
    let plan = capture_plan(&fw, &params).expect("capture");
    let grid = evaluate_plan(&fw, &plan).expect("grid eval");
    let naive = evaluate_plan_naive(&fw, &plan).expect("naive eval");
    assert_meshes_identical(&grid, &naive, "flywheel");
}

// -------------------------------------------------------- timing (printed)

fn median_ms(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    samples[samples.len() / 2]
}

fn time_ms(reps: usize, mut f: impl FnMut()) -> f64 {
    let mut samples = Vec::with_capacity(reps);
    for _ in 0..reps {
        let t = Instant::now();
        f();
        samples.push(t.elapsed().as_secs_f64() * 1e3);
    }
    median_ms(samples)
}

#[test]
fn m10_capture_seam_pullback_timings() {
    eprintln!("\n=== M10 timings (median ms) ===");
    eprintln!("-- rounded cube: capture / seam / pullback (all ~linear in nodes) --");
    let rc = rounded_cube();
    let seeding = rc_seeding(&rc);
    for segs in [16u32, 64, 128] {
        let params = TessellationParams {
            circle_segments: segs,
            height_segments: 2,
            ..Default::default()
        };
        let reps = if segs >= 128 { 15 } else { 40 };

        let cap_ms = time_ms(reps, || {
            let _ = capture_plan(&rc, &params).unwrap();
        });
        let plan = capture_plan(&rc, &params).unwrap();
        let nodes = plan.nodes.len();
        let seam_ms = time_ms(reps, || {
            let _ = evaluate_with_sensitivity(&rc, &plan, &seeding).unwrap();
        });
        let base = evaluate_with_sensitivity(&rc, &plan, &ParamSeeding::new()).unwrap();
        let w = volume_gradient(&base.positions, &base.triangles);
        let pull_ms = time_ms(reps, || {
            let _ = evaluate_with_pullback(&rc, &plan, &w).unwrap();
        });
        eprintln!(
            "  segs={segs:>3} nodes={nodes:>6}: capture {cap_ms:7.3}  seam {seam_ms:6.3}  pullback {pull_ms:6.3}"
        );
    }

    // The gridded win: evaluation-time vertex resolution, naive vs grid, where
    // the topology carries many rebuilt vertices (the flywheel's boolean rims).
    eprintln!("-- evaluate_plan (FD-oracle resolution): naive O(nodes·verts) vs grid --");
    for (name, brep, hseg) in [
        ("rounded cube", rounded_cube(), 2u32),
        ("flywheel    ", flywheel(), 1u32),
    ] {
        for segs in [16u32, 64, 128] {
            let params = TessellationParams {
                circle_segments: segs,
                height_segments: hseg,
                ..Default::default()
            };
            let plan = capture_plan(&brep, &params).unwrap();
            let verts = plan
                .nodes
                .iter()
                .filter(|n| {
                    matches!(
                        n,
                        vcad_kernel_tessellate::frozen::NodeRecipe::TopoVertex { .. }
                    )
                })
                .count();
            let reps = if segs >= 128 { 20 } else { 60 };
            let naive_ms = time_ms(reps, || {
                let _ = evaluate_plan_naive(&brep, &plan).unwrap();
            });
            let grid_ms = time_ms(reps, || {
                let _ = evaluate_plan(&brep, &plan).unwrap();
            });
            eprintln!(
                "  {name} segs={segs:>3} nodes={:>6} topo-verts={verts:>4}: naive {naive_ms:7.3}  grid {grid_ms:7.3}  (speedup {:.2}x)",
                plan.nodes.len(),
                naive_ms / grid_ms
            );
        }
    }
    eprintln!();
}
