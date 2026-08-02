//! M6 — seeding synthesis: reproduce prior milestones with **zero hand
//! seeding**.
//!
//! Every earlier milestone hand-writes its [`ParamSeeding`]: the author
//! transcribes "this θ grows these radii and retreats these centers" by hand.
//! [`synthesize_seeding`] derives that map from the build function itself, by
//! central-difference probing the *surface fields* (radii, offsets, axes,
//! centers) at θ ± h and reading the observable per-surface seed components off
//! the matched field deltas. These gates re-run M2, M3, and the M5 rounded cube
//! using only synthesized seedings and check they reproduce the closed forms,
//! the FD oracle, and (for the rounded cube) the hand seedings exactly.
//!
//! ## The base-instance contract
//!
//! A synthesized seeding is keyed by geometry-store index into `build(theta)`,
//! and the boolean/fillet stores are order-nondeterministic (a rebuild permutes
//! the store, flips axis signs, slides centers). So a seeding is only meaningful
//! against a base built *identically*. Each gate builds a canonical base once
//! and hands the synthesizer a closure that returns a clone of that canonical
//! base at the base θ (and rebuilds fresh for the perturbations, which are
//! matched geometrically). This mirrors real usage, where `build(theta)` is
//! called once per optimizer iterate and both the plan and the seeding come
//! from that one instance.

use std::f64::consts::PI;

use vcad_kernel::Solid;
use vcad_kernel_diff::{
    evaluate_with_pullback, evaluate_with_sensitivity, fd_volume_derivative, synthesize_all,
    synthesize_seeding, volume_gradient, volume_with_derivative, DiffError, ParamSeeding,
    SurfaceSeed,
};
use vcad_kernel_geom::{CylinderSurface, Plane, SphereSurface};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::{make_cube, make_cylinder, BRepSolid};
use vcad_kernel_tessellate::frozen::{capture_plan, FrozenError};
use vcad_kernel_tessellate::TessellationParams;

const H: f64 = 1e-6;
const FD_GATE: f64 = 1e-6;
/// Synthesized-vs-hand / synthesized-vs-forward agreement. The θ→field map is
/// a central difference, so the synthesis carries O(h²) field error and cannot
/// match a hand seeding to machine precision — 1e-6 relative is the honest bar.
const AGREE: f64 = 1e-6;

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs().max(1.0)
}

// ---------------------------------------------------------------------------
// Gate 1 — M2 boolean through-hole, θ = hole radius.
// ---------------------------------------------------------------------------

const M2_L: f64 = 10.0;
const M2_W: f64 = 8.0;
const M2_T: f64 = 5.0;
const M2_R0: f64 = 2.5;
const M2_SEG: u32 = 32;

/// The M2 model (copied from `tests/m2_boolean_hole.rs`): block minus a
/// through-hole of radius `r`.
fn build_m2(r: f64) -> BRepSolid {
    let block = Solid::cube(M2_L, M2_W, M2_T);
    let tool = Solid::cylinder(r, M2_T + 2.0, M2_SEG).translate(M2_L / 2.0, M2_W / 2.0, -1.0);
    block
        .difference(&tool)
        .as_brep()
        .expect("boolean should stay BRep")
        .clone()
}

#[test]
fn m2_hole_radius_synthesized() {
    let canonical = build_m2(M2_R0);
    let build = |t: &[f64]| -> BRepSolid {
        if t.len() == 1 && t[0] == M2_R0 {
            canonical.clone()
        } else {
            build_m2(t[0])
        }
    };

    let seeding = synthesize_seeding(&build, &[M2_R0], 0, H).expect("synthesize");

    let params = TessellationParams {
        circle_segments: M2_SEG,
        height_segments: 3,
        ..Default::default()
    };
    let plan = capture_plan(&canonical, &params).expect("capture");
    let seam = evaluate_with_sensitivity(&canonical, &plan, &seeding).expect("seam");
    let (_v, dv) = volume_with_derivative(&seam);

    // Discrete closed form for the frozen N-gon rim: dV/dr = −N·sin(2π/N)·r·t.
    // N is the kernel's canonical (sag-adaptive) rim count, not `M2_SEG`.
    let n = vcad_kernel::vcad_kernel_booleans::split::arc_segments(M2_R0, M2_SEG) as f64;
    let dv_closed = -n * (2.0 * PI / n).sin() * M2_R0 * M2_T;
    let dv_fd = fd_volume_derivative(build_m2, M2_R0, H, &plan).expect("fd");

    assert!(
        rel(dv, dv_closed) <= FD_GATE,
        "synth dV/dr {dv} vs closed form {dv_closed} (rel {:.3e})",
        rel(dv, dv_closed)
    );
    assert!(
        rel(dv, dv_fd) <= FD_GATE,
        "synth dV/dr {dv} vs FD {dv_fd} (rel {:.3e})",
        rel(dv, dv_fd)
    );
}

// ---------------------------------------------------------------------------
// Gate 2 — M3 cylinder height, θ = height (moving cap plane, Boundary rims).
// ---------------------------------------------------------------------------

const M3_R: f64 = 5.0;
const M3_H0: f64 = 8.0;
const M3_SEG: u32 = 24;

fn build_m3(h: f64) -> BRepSolid {
    make_cylinder(M3_R, h, M3_SEG)
}

#[test]
fn m3_cylinder_height_synthesized() {
    let canonical = build_m3(M3_H0);
    let build = |t: &[f64]| -> BRepSolid {
        if t.len() == 1 && t[0] == M3_H0 {
            canonical.clone()
        } else {
            build_m3(t[0])
        }
    };

    let seeding = synthesize_seeding(&build, &[M3_H0], 0, H).expect("synthesize");

    let params = TessellationParams {
        circle_segments: M3_SEG,
        height_segments: 4,
        ..Default::default()
    };
    let plan = capture_plan(&canonical, &params).expect("capture");
    let seam = evaluate_with_sensitivity(&canonical, &plan, &seeding).expect("seam");
    let (_v, dv) = volume_with_derivative(&seam);

    // dV/dh = inscribed N-gon area.
    let n = M3_SEG as f64;
    let area = 0.5 * n * (2.0 * PI / n).sin() * M3_R * M3_R;
    let dv_fd = fd_volume_derivative(build_m3, M3_H0, H, &plan).expect("fd");

    assert!(
        rel(dv, area) <= FD_GATE,
        "synth dV/dh {dv} vs N-gon area {area} (rel {:.3e})",
        rel(dv, area)
    );
    assert!(
        rel(dv, dv_fd) <= FD_GATE,
        "synth dV/dh {dv} vs FD {dv_fd} (rel {:.3e})",
        rel(dv, dv_fd)
    );
}

// ---------------------------------------------------------------------------
// Gate 3 — the rounded cube, θ = [a, r]: the hard case (20 moving surfaces,
// composite seeds, frame nondeterminism). Synthesized seedings for BOTH
// parameters must match the hand seedings and the FD oracle.
// ---------------------------------------------------------------------------

const RC_A0: f64 = 10.0;
const RC_R0: f64 = 1.5;

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

/// Hand seeding for θ = fillet radius (copied from `tests/m5_reverse_mode.rs`).
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

/// Hand seeding for θ = cube edge length `a` (copied from
/// `tests/m5_reverse_mode.rs`).
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
fn rounded_cube_both_parameters_synthesized() {
    let canonical = build_rc(&[RC_A0, RC_R0]);
    let build = |t: &[f64]| -> BRepSolid {
        if t.len() == 2 && t[0] == RC_A0 && t[1] == RC_R0 {
            canonical.clone()
        } else {
            build_rc(t)
        }
    };

    let params = TessellationParams {
        circle_segments: 16,
        height_segments: 2,
        ..Default::default()
    };
    let plan = capture_plan(&canonical, &params).expect("capture");

    // Synthesize both parameters from one base build.
    let syn = synthesize_all(&build, &[RC_A0, RC_R0], H).expect("synthesize_all");
    let syn_a = &syn[0];
    let syn_r = &syn[1];

    // Forward mode with the synthesized seedings.
    let dv_da_syn = volume_with_derivative(
        &evaluate_with_sensitivity(&canonical, &plan, syn_a).expect("syn a"),
    )
    .1;
    let dv_dr_syn = volume_with_derivative(
        &evaluate_with_sensitivity(&canonical, &plan, syn_r).expect("syn r"),
    )
    .1;

    // Forward mode with the hand seedings (the M5 reference).
    let hand_a = seeding_a(&canonical, RC_A0, RC_R0);
    let hand_r = seeding_r(&canonical, RC_A0, RC_R0);
    let dv_da_hand =
        volume_with_derivative(&evaluate_with_sensitivity(&canonical, &plan, &hand_a).expect("a"))
            .1;
    let dv_dr_hand =
        volume_with_derivative(&evaluate_with_sensitivity(&canonical, &plan, &hand_r).expect("r"))
            .1;

    // Gate 3a: synthesized vs hand seedings.
    assert!(
        rel(dv_da_syn, dv_da_hand) <= AGREE,
        "dV/da synth {dv_da_syn} vs hand {dv_da_hand} (rel {:.3e})",
        rel(dv_da_syn, dv_da_hand)
    );
    assert!(
        rel(dv_dr_syn, dv_dr_hand) <= AGREE,
        "dV/dr synth {dv_dr_syn} vs hand {dv_dr_hand} (rel {:.3e})",
        rel(dv_dr_syn, dv_dr_hand)
    );

    // Gate 3b: synthesized vs FD oracle.
    let fd_a = fd_volume_derivative(|a| build_rc(&[a, RC_R0]), RC_A0, H, &plan).expect("fd a");
    let fd_r = fd_volume_derivative(|r| build_rc(&[RC_A0, r]), RC_R0, H, &plan).expect("fd r");
    assert!(
        rel(dv_da_syn, fd_a) <= FD_GATE,
        "dV/da synth {dv_da_syn} vs FD {fd_a} (rel {:.3e})",
        rel(dv_da_syn, fd_a)
    );
    assert!(
        rel(dv_dr_syn, fd_r) <= FD_GATE,
        "dV/dr synth {dv_dr_syn} vs FD {fd_r} (rel {:.3e})",
        rel(dv_dr_syn, fd_r)
    );

    // Gate 5: synthesized seedings compose with reverse mode — one pullback,
    // contracted against the synthesized seedings, reproduces forward mode.
    let seam0 = evaluate_with_sensitivity(&canonical, &plan, &ParamSeeding::new()).expect("seam0");
    let w = volume_gradient(&seam0.positions, &seam0.triangles);
    let cots = evaluate_with_pullback(&canonical, &plan, &w).expect("pullback");
    let dv_da_rev = cots.contract(syn_a);
    let dv_dr_rev = cots.contract(syn_r);
    assert!(
        rel(dv_da_rev, dv_da_syn) <= AGREE,
        "dV/da reverse {dv_da_rev} vs forward {dv_da_syn}"
    );
    assert!(
        rel(dv_dr_rev, dv_dr_syn) <= AGREE,
        "dV/dr reverse {dv_dr_rev} vs forward {dv_dr_syn}"
    );
}

// ---------------------------------------------------------------------------
// Gate 4 — a parameter whose ±h probe crosses a topology change must error.
// ---------------------------------------------------------------------------

/// A build whose topology changes at θ = 2.5: above it the block carries a
/// through-hole; at or below it the block is solid. Probing at θ₀ = 2.5 steps
/// across that boundary, so no meaningful derivative exists.
fn build_topo(theta: &[f64]) -> BRepSolid {
    let r = theta[0];
    let block = Solid::cube(10.0, 8.0, 5.0);
    if r > 2.5 {
        let tool = Solid::cylinder(r, 7.0, 32).translate(5.0, 4.0, -1.0);
        block
            .difference(&tool)
            .as_brep()
            .expect("boolean stays BRep")
            .clone()
    } else {
        block.as_brep().expect("cube is BRep").clone()
    }
}

#[test]
fn topology_change_errors_cleanly() {
    match synthesize_seeding(&build_topo, &[2.5], 0, H) {
        Err(DiffError::Frozen(FrozenError::TopologyChanged { .. })) => {}
        other => panic!("expected TopologyChanged, got {other:?}"),
    }
}

#[test]
fn parameter_out_of_range_errors() {
    let build = |t: &[f64]| build_m3(t[0]);
    match synthesize_seeding(&build, &[M3_H0], 3, H) {
        Err(DiffError::ParameterOutOfRange { k: 3, len: 1 }) => {}
        other => panic!("expected ParameterOutOfRange, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Gate 6 — M6 × M7 composition: cone and torus parameters synthesize too.
// ---------------------------------------------------------------------------

/// The M7 cone model (θ = half-angle about a fixed apex at z = 20): both cap
/// radii are functions of α, so the synthesizer must recover a pure
/// `ConeAngle` seed on the wall plus the cap planes staying still.
fn build_cone(theta: &[f64]) -> BRepSolid {
    let t = theta[0].tan();
    vcad_kernel_primitives::make_cone(20.0 * t, 12.0 * t, 8.0, 24)
}

/// The M7 torus model, θ = minor radius.
fn build_torus(theta: &[f64]) -> BRepSolid {
    vcad_kernel_primitives::make_torus(8.0, theta[0], 24)
}

#[test]
fn cone_and_torus_parameters_synthesized() {
    let params = TessellationParams {
        circle_segments: 24,
        height_segments: 2,
        ..Default::default()
    };

    let alpha0 = 0.25_f64.atan();
    let base = build_cone(&[alpha0]);
    let seeding = {
        let canonical = base.clone();
        let probe = move |t: &[f64]| {
            if (t[0] - alpha0).abs() < f64::EPSILON {
                canonical.clone()
            } else {
                build_cone(t)
            }
        };
        synthesize_seeding(&probe, &[alpha0], 0, H).expect("synthesize cone")
    };
    let plan = capture_plan(&base, &params).expect("capture cone");
    let seam = evaluate_with_sensitivity(&base, &plan, &seeding).expect("seam cone");
    let (_, dv) = volume_with_derivative(&seam);
    let fd = fd_volume_derivative(|a| build_cone(&[a]), alpha0, H, &plan).expect("fd cone");
    assert!(
        rel(dv, fd) <= FD_GATE,
        "cone half-angle: synthesized {dv} vs FD {fd}"
    );

    let r0 = 3.0;
    let base = build_torus(&[r0]);
    let seeding = {
        let canonical = base.clone();
        let probe = move |t: &[f64]| {
            if (t[0] - r0).abs() < f64::EPSILON {
                canonical.clone()
            } else {
                build_torus(t)
            }
        };
        synthesize_seeding(&probe, &[r0], 0, H).expect("synthesize torus")
    };
    let plan = capture_plan(&base, &params).expect("capture torus");
    let seam = evaluate_with_sensitivity(&base, &plan, &seeding).expect("seam torus");
    let (_, dv) = volume_with_derivative(&seam);
    let fd = fd_volume_derivative(|r| build_torus(&[r]), r0, H, &plan).expect("fd torus");
    assert!(
        rel(dv, fd) <= FD_GATE,
        "torus minor radius: synthesized {dv} vs FD {fd}"
    );
}
