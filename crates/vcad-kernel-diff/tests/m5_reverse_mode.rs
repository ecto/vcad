//! M5 — reverse mode: one pullback pass prices every parameter.
//!
//! Forward mode costs one seam evaluation per θ. The adjoint seam
//! (`evaluate_with_pullback`) pulls a mesh functional's gradient `∂J/∂x`
//! back to per-surface seed cotangents once; `dJ/dθ_k` for each parameter
//! is then a dot product (`MeshCotangents::contract`), with no further
//! seam evaluations.
//!
//! Gate: for every model class the seam supports — pure topology vertices,
//! boolean rims with duplicated store surfaces, Boundary trim rings, and
//! the full rounded cube with composite seeds and tangency-completion rows
//! — the contraction of one pullback must reproduce the forward-mode
//! derivative to near machine precision (the two sides share row
//! construction and differ only in linear-algebra order), and the forward
//! side is itself FD-validated.

use vcad_kernel::Solid;
use vcad_kernel_diff::{
    evaluate_with_pullback, evaluate_with_sensitivity, fd_volume_derivative, volume_gradient,
    volume_with_derivative, ParamSeeding, SurfaceSeed,
};
use vcad_kernel_geom::{CylinderSurface, Plane, SphereSurface};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::{make_cube, make_cylinder, BRepSolid};
use vcad_kernel_tessellate::frozen::{capture_plan, FrozenPlan};
use vcad_kernel_tessellate::TessellationParams;

/// Reverse-vs-forward agreement gate: the two modes share row construction
/// and differ only in the order of linear operations.
const AGREE: f64 = 1e-11;
const FD_GATE: f64 = 1e-6;
const H: f64 = 1e-6;

/// One pullback, then per-seeding forward reference: returns
/// `(forward dJ/dθ, reverse dJ/dθ)` pairs for the volume functional.
fn pullback_vs_forward(
    brep: &BRepSolid,
    plan: &FrozenPlan,
    seedings: &[ParamSeeding],
) -> Vec<(f64, f64)> {
    let base = evaluate_with_sensitivity(brep, plan, &ParamSeeding::new()).expect("base seam");
    let w = volume_gradient(&base.positions, &base.triangles);
    let cots = evaluate_with_pullback(brep, plan, &w).expect("pullback");
    seedings
        .iter()
        .map(|s| {
            let fwd = volume_with_derivative(
                &evaluate_with_sensitivity(brep, plan, s).expect("forward seam"),
            )
            .1;
            (fwd, cots.contract(s))
        })
        .collect()
}

fn assert_agree(fwd: f64, rev: f64, what: &str) {
    let scale = fwd.abs().max(1.0);
    assert!(
        (fwd - rev).abs() / scale <= AGREE,
        "{what}: forward {fwd} vs reverse {rev}"
    );
}

#[test]
fn m5_pullback_matches_forward_across_model_classes() {
    // (a) Pure topology vertices: cube with a moving top face.
    let cube = make_cube(4.0, 3.0, 2.0);
    let plan = capture_plan(&cube, &TessellationParams::default()).expect("capture cube");
    let mut top = ParamSeeding::new();
    let n = top.seed_where(
        &cube.geometry,
        |s| {
            s.as_any()
                .downcast_ref::<Plane>()
                .map(|p| {
                    p.normal_dir.as_ref().cross(Vec3::z()).norm() < 1e-12
                        && p.signed_distance(&Point3::new(0.0, 0.0, 2.0)).abs() < 1e-9
                })
                .unwrap_or(false)
        },
        SurfaceSeed::Translate {
            velocity: Vec3::z(),
        },
    );
    assert_eq!(n, 1);
    let pairs = pullback_vs_forward(&cube, &plan, &[top]);
    assert_agree(pairs[0].0, pairs[0].1, "cube dV/dh");
    assert!((pairs[0].1 - 12.0).abs() < 1e-9, "dV/dh = {}", pairs[0].1);

    // (b) Boolean through-hole: rim topology vertices + lift-bridge wall
    // samples, radius seed (the M2 model).
    let hole = {
        let block = Solid::cube(10.0, 8.0, 5.0);
        let tool = Solid::cylinder(2.5, 7.0, 32).translate(5.0, 4.0, -1.0);
        block
            .difference(&tool)
            .as_brep()
            .expect("boolean stays BRep")
            .clone()
    };
    let params = TessellationParams {
        circle_segments: 32,
        height_segments: 3,
        ..Default::default()
    };
    let plan = capture_plan(&hole, &params).expect("capture hole");
    let mut radius = ParamSeeding::new();
    let n = radius.seed_where(
        &hole.geometry,
        |s| {
            s.as_any()
                .downcast_ref::<CylinderSurface>()
                .map(|c| (c.radius - 2.5).abs() < 1e-9)
                .unwrap_or(false)
        },
        SurfaceSeed::CylinderRadius { rate: 1.0 },
    );
    assert!(n >= 1);
    let pairs = pullback_vs_forward(&hole, &plan, &[radius]);
    assert_agree(pairs[0].0, pairs[0].1, "hole dV/dr");

    // (c) Boundary trim rings: cylinder height (the M3 model — cap rims
    // are Newton-tracked Boundary nodes, not topology vertices).
    let cyl = make_cylinder(5.0, 8.0, 24);
    let params = TessellationParams {
        circle_segments: 24,
        height_segments: 4,
        ..Default::default()
    };
    let plan = capture_plan(&cyl, &params).expect("capture cylinder");
    let mut height = ParamSeeding::new();
    let n = height.seed_where(
        &cyl.geometry,
        |s| {
            s.as_any()
                .downcast_ref::<Plane>()
                .map(|p| {
                    p.normal_dir.as_ref().cross(Vec3::z()).norm() < 1e-12
                        && p.signed_distance(&Point3::new(0.0, 0.0, 8.0)).abs() < 1e-9
                })
                .unwrap_or(false)
        },
        SurfaceSeed::Translate {
            velocity: Vec3::z(),
        },
    );
    assert_eq!(n, 1);
    let pairs = pullback_vs_forward(&cyl, &plan, &[height]);
    assert_agree(pairs[0].0, pairs[0].1, "cylinder dV/dh");
}

// ---- the flagship: every fillet-cube parameter from one pullback ----

const A0: f64 = 10.0;
const R0: f64 = 1.5;

fn build_rc(theta: &[f64]) -> BRepSolid {
    vcad_kernel_fillet::fillet_all_edges(&make_cube(theta[0], theta[0], theta[0]), theta[1])
}

fn coord(p: Point3, k: usize) -> f64 {
    match k {
        0 => p.x,
        1 => p.y,
        _ => p.z,
    }
}

fn axes() -> [Vec3; 3] {
    [
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
    ]
}

/// θ = fillet radius (the M4 seeding): every blend's radius grows at rate 1
/// while its center retreats from the edge.
fn seeding_r(brep: &BRepSolid, a: f64, r: f64) -> ParamSeeding {
    let retreat = |center: Point3| {
        let component = |c: f64| {
            if (c - r).abs() < 1e-9 {
                1.0
            } else if (c - (a - r)).abs() < 1e-9 {
                -1.0
            } else {
                0.0
            }
        };
        Vec3::new(
            component(center.x),
            component(center.y),
            component(center.z),
        )
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

/// θ = the cube edge length `a`: the three far planes translate outward,
/// and every blend whose center is pinned at `a − r` on some coordinate
/// rides along; radii are unchanged.
fn seeding_a(brep: &BRepSolid, a: f64, r: f64) -> ParamSeeding {
    let ride = |center: Point3, skip_axis: Option<Vec3>| {
        let mut vel = Vec3::new(0.0, 0.0, 0.0);
        for (k, e) in axes().iter().enumerate() {
            if let Some(axis) = skip_axis {
                if axis.dot(*e).abs() > 0.9 {
                    continue;
                }
            }
            let c = coord(center, k);
            if (c - (a - r)).abs() < 1e-9 {
                vel += *e;
            } else {
                assert!(
                    (c - r).abs() < 1e-9,
                    "unexpected blend center coordinate {c}"
                );
            }
        }
        vel
    };
    let mut seeding = ParamSeeding::new();
    let (mut planes, mut cylinders, mut spheres) = (0, 0, 0);
    for (i, s) in brep.geometry.surfaces.iter().enumerate() {
        if let Some(p) = s.as_any().downcast_ref::<Plane>() {
            let n = *p.normal_dir.as_ref();
            for e in &axes() {
                let probe = Point3::new(a * e.x, a * e.y, a * e.z);
                if n.dot(*e).abs() > 1.0 - 1e-9 && p.signed_distance(&probe).abs() < 1e-9 {
                    seeding.seed(i, SurfaceSeed::Translate { velocity: *e });
                    planes += 1;
                }
            }
        } else if let Some(c) = s.as_any().downcast_ref::<CylinderSurface>() {
            let vel = ride(c.center, Some(*c.axis.as_ref()));
            if vel.norm() > 0.0 {
                seeding.seed(i, SurfaceSeed::Translate { velocity: vel });
            }
            cylinders += 1;
        } else if let Some(sp) = s.as_any().downcast_ref::<SphereSurface>() {
            let vel = ride(sp.center, None);
            if vel.norm() > 0.0 {
                seeding.seed(i, SurfaceSeed::Translate { velocity: vel });
            }
            spheres += 1;
        }
    }
    assert_eq!(
        (planes, cylinders, spheres),
        (3, 12, 8),
        "rounded cube census"
    );
    seeding
}

#[test]
fn m5_one_pullback_prices_every_fillet_cube_parameter() {
    let base = build_rc(&[A0, R0]);
    let params = TessellationParams {
        circle_segments: 16,
        height_segments: 2,
        ..Default::default()
    };
    let plan = capture_plan(&base, &params).expect("capture");

    // One reverse pass...
    let seam0 = evaluate_with_sensitivity(&base, &plan, &ParamSeeding::new()).expect("seam");
    let w = volume_gradient(&seam0.positions, &seam0.triangles);
    let cots = evaluate_with_pullback(&base, &plan, &w).expect("pullback");

    // ...contracted against both parameters' seedings.
    let s_a = seeding_a(&base, A0, R0);
    let s_r = seeding_r(&base, A0, R0);
    let dv_da_rev = cots.contract(&s_a);
    let dv_dr_rev = cots.contract(&s_r);

    // Gate 1: agree with forward mode (composite seeds, duplicated-copy
    // handling, and tangency-completion rows all live on both sides).
    let dv_da_fwd =
        volume_with_derivative(&evaluate_with_sensitivity(&base, &plan, &s_a).expect("fwd a")).1;
    let dv_dr_fwd =
        volume_with_derivative(&evaluate_with_sensitivity(&base, &plan, &s_r).expect("fwd r")).1;
    assert_agree(dv_da_fwd, dv_da_rev, "rounded cube dV/da");
    assert_agree(dv_dr_fwd, dv_dr_rev, "rounded cube dV/dr");

    // Gate 2: both parameters against the FD oracle (each direction is a
    // fresh 1-D family through (A0, R0) under the same frozen plan).
    let fd_a = fd_volume_derivative(|a| build_rc(&[a, R0]), A0, H, &plan).expect("fd a");
    let fd_r = fd_volume_derivative(|r| build_rc(&[A0, r]), R0, H, &plan).expect("fd r");
    let rel_a = (dv_da_rev - fd_a).abs() / fd_a.abs();
    let rel_r = (dv_dr_rev - fd_r).abs() / fd_r.abs();
    assert!(rel_a <= FD_GATE, "dV/da reverse {dv_da_rev} vs FD {fd_a}");
    assert!(rel_r <= FD_GATE, "dV/dr reverse {dv_dr_rev} vs FD {fd_r}");

    // Gate 3: continuum sanity — the Minkowski closed form
    // V = s³ + 6s²r + 3πsr² + (4/3)πr³ with s = a − 2r gives
    // dV/da = 3s² + 12sr + 3πr², matched within the polygonization band.
    let s = A0 - 2.0 * R0;
    let dv_da_cont = 3.0 * s * s + 12.0 * s * R0 + 3.0 * std::f64::consts::PI * R0 * R0;
    let gap = (dv_da_rev - dv_da_cont).abs() / dv_da_cont;
    assert!(gap < 0.10, "continuum gap {gap:.3}");
}
