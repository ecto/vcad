//! M7 — cone and torus coverage for the differentiable seam.
//!
//! Every seam extension point (implicit form, `SurfaceSeed`, lift-bridge,
//! frame transport, reverse-mode cotangent) is exercised through the two
//! curved surface kinds fillets/chamfers of curved parts need.
//!
//! **Cone** (`make_cone` frustum, so the apex is virtual and no degenerate
//! apex vertex appears):
//! - θ = half-angle α, grown about a *fixed apex* — a pure `ConeAngle` seed
//!   on the lateral surface. Interior samples ride the lift-bridge; the two
//!   cap rims are `Boundary` (cone ∩ plane) nodes fed by the cone implicit
//!   row; the seam vertices are topology vertices with the same row. This is
//!   observable precisely because a cone's radius at any fixed height moves
//!   when the half-angle opens (documented in the M7 note).
//! - θ = height h, the top cap plane translating along +ẑ while the cone
//!   stays fixed — a plane `Translate` seed, the cone analogue of the M3
//!   cylinder-height Boundary case, but the rim now rides *inward* along the
//!   sloped ruling.
//!
//! **Torus** (`make_torus`, a genuine single-face torus solid — tori ARE
//! constructible through the public kernel, so the full solid-level gate
//! battery applies, not just unit tests):
//! - θ = minor radius r and θ = major radius R.
//!
//! Gates per parameter: forward seam dV/dθ vs the central-difference oracle
//! and (where cheap) a discrete N-gon closed form; node-wise dx/dθ vs the FD
//! oracle; and reverse mode (`evaluate_with_pullback` + `contract`) vs
//! forward mode.

use std::f64::consts::PI;

use vcad_kernel_diff::{
    compare_velocities, evaluate_with_pullback, evaluate_with_sensitivity, fd_velocities,
    fd_volume_derivative, volume_gradient, volume_with_derivative, ParamSeeding, SurfaceSeed,
};
use vcad_kernel_geom::{Plane, SurfaceKind};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::{make_cone, make_torus, BRepSolid};
use vcad_kernel_tessellate::frozen::{capture_plan, FrozenPlan};
use vcad_kernel_tessellate::TessellationParams;

const SEGMENTS: u32 = 24;
const H_FD: f64 = 1e-6;
const GATE: f64 = 1e-6;
const AGREE: f64 = 1e-11;

fn params() -> TessellationParams {
    TessellationParams {
        circle_segments: SEGMENTS,
        height_segments: 2,
        latitude_segments: SEGMENTS,
        ..Default::default()
    }
}

/// Inscribed N-gon area of radius `r`.
fn ngon_area(n: u32, r: f64) -> f64 {
    0.5 * n as f64 * (2.0 * PI / n as f64).sin() * r * r
}

fn seed_count(seeding: &ParamSeeding, geom: &vcad_kernel_geom::GeometryStore, expect: usize) {
    let mut count = 0;
    for i in 0..geom.surfaces.len() {
        count += seeding.get(i).len().min(1);
    }
    assert_eq!(count, expect, "unexpected number of seeded surfaces");
}

// ============================================================================
// Cone
// ============================================================================

const APEX_Z: f64 = 20.0;
const TAN_A0: f64 = 0.25; // half-angle atan(0.25); base model make_cone(5, 3, 8)
const CONE_H0: f64 = 8.0;

/// θ = half-angle α, apex pinned at z = APEX_Z. Keeping the apex fixed while
/// α opens makes both cap radii `(APEX_Z − z)·tan α` grow: build with the
/// radii that reproduce a fixed apex over z ∈ [0, h].
fn build_cone_alpha(alpha: f64) -> BRepSolid {
    let t = alpha.tan();
    make_cone(APEX_Z * t, (APEX_Z - CONE_H0) * t, CONE_H0, SEGMENTS)
}

fn seeding_cone_alpha(brep: &BRepSolid) -> ParamSeeding {
    let mut s = ParamSeeding::new();
    let n = s.seed_where(
        &brep.geometry,
        |surf| surf.surface_type() == SurfaceKind::Cone,
        SurfaceSeed::ConeAngle { rate: 1.0 },
    );
    assert_eq!(n, 1, "expected exactly the cone lateral surface");
    s
}

/// θ = height h: the top cap plane translates along +ẑ while the cone stays
/// fixed. Build with a top radius that keeps the cone's apex/axis/half-angle
/// invariant, so only the cap plane (and its rim) moves.
fn build_cone_height(h: f64) -> BRepSolid {
    let base = APEX_Z * TAN_A0; // = 5
    let top = (APEX_Z - h) * TAN_A0;
    make_cone(base, top, h, SEGMENTS)
}

fn seeding_cone_height(brep: &BRepSolid, h: f64) -> ParamSeeding {
    let mut s = ParamSeeding::new();
    let n = s.seed_where(
        &brep.geometry,
        |surf| {
            surf.as_any()
                .downcast_ref::<Plane>()
                .map(|p| {
                    p.normal_dir.as_ref().cross(Vec3::z()).norm() < 1e-12
                        && p.signed_distance(&Point3::new(0.0, 0.0, h)).abs() < 1e-9
                })
                .unwrap_or(false)
        },
        SurfaceSeed::Translate {
            velocity: Vec3::z(),
        },
    );
    assert_eq!(n, 1, "expected exactly the top cap plane");
    s
}

#[test]
fn cone_half_angle_derivative() {
    let alpha0 = TAN_A0.atan();
    let base = build_cone_alpha(alpha0);
    let plan = capture_plan(&base, &params()).expect("capture cone");
    let seeding = seeding_cone_alpha(&base);
    seed_count(&seeding, &base.geometry, 1);

    let seam = evaluate_with_sensitivity(&base, &plan, &seeding).expect("seam");
    let (v, dv) = volume_with_derivative(&seam);

    // Discrete closed form: frustum of N-gon cross sections
    // V = (h·k/3)(Rb² + Rt² + Rb·Rt), k = ½N·sin(2π/N), with Rb = 20 tan α,
    // Rt = 12 tan α. dV/dα = (h·k/3)(dRb²+dRt²+d(RbRt))/dα.
    let k = ngon_area(SEGMENTS, 1.0);
    let rb = APEX_Z * TAN_A0;
    let rt = (APEX_Z - CONE_H0) * TAN_A0;
    let vol_closed = CONE_H0 * k / 3.0 * (rb * rb + rt * rt + rb * rt);
    assert!(
        (v - vol_closed).abs() / vol_closed < 1e-9,
        "volume {v} vs closed {vol_closed}"
    );
    // d/dα of (Rb²+Rt²+RbRt) with Rb=20t, Rt=12t, t=tanα, dt/dα=sec²α:
    let sec2 = 1.0 / (alpha0.cos() * alpha0.cos());
    let dcoef = (2.0 * APEX_Z * APEX_Z
        + 2.0 * (APEX_Z - CONE_H0) * (APEX_Z - CONE_H0)
        + 2.0 * APEX_Z * (APEX_Z - CONE_H0))
        * TAN_A0
        * sec2;
    let dv_closed = CONE_H0 * k / 3.0 * dcoef;
    let rel_closed = (dv - dv_closed).abs() / dv_closed.abs();
    assert!(
        rel_closed <= GATE,
        "cone dV/dα = {dv} vs closed {dv_closed} (rel {rel_closed:.3e})"
    );

    let dv_fd = fd_volume_derivative(build_cone_alpha, alpha0, H_FD, &plan).expect("fd vol");
    let rel_fd = (dv - dv_fd).abs() / dv.abs();
    assert!(
        rel_fd <= GATE,
        "cone dV/dα = {dv} vs FD {dv_fd} (rel {rel_fd:.3e})"
    );

    let fd = fd_velocities(build_cone_alpha, alpha0, H_FD, &plan).expect("fd vel");
    let cmp = compare_velocities(&seam.velocities, &fd);
    assert!(
        cmp.max_rel_err <= GATE,
        "cone dx/dα max rel err {:.3e} at node {}",
        cmp.max_rel_err,
        cmp.worst_node
    );

    // Reverse mode reproduces forward from a single pullback.
    let base_seam = evaluate_with_sensitivity(&base, &plan, &ParamSeeding::new()).expect("base");
    let w = volume_gradient(&base_seam.positions, &base_seam.triangles);
    let cots = evaluate_with_pullback(&base, &plan, &w).expect("pullback");
    let rev = cots.contract(&seeding);
    assert!(
        (rev - dv).abs() / dv.abs() <= AGREE,
        "cone reverse {rev} vs forward {dv}"
    );
}

#[test]
fn cone_height_derivative() {
    let base = build_cone_height(CONE_H0);
    let plan = capture_plan(&base, &params()).expect("capture cone");
    let seeding = seeding_cone_height(&base, CONE_H0);

    let seam = evaluate_with_sensitivity(&base, &plan, &seeding).expect("seam");
    let (_, dv) = volume_with_derivative(&seam);

    // dV/dh = top cross-sectional area = N-gon area of the top radius.
    let rt = (APEX_Z - CONE_H0) * TAN_A0; // = 3
    let dv_closed = ngon_area(SEGMENTS, rt);
    let rel_closed = (dv - dv_closed).abs() / dv_closed;
    assert!(
        rel_closed <= GATE,
        "cone dV/dh = {dv} vs closed {dv_closed} (rel {rel_closed:.3e})"
    );

    let dv_fd = fd_volume_derivative(build_cone_height, CONE_H0, H_FD, &plan).expect("fd vol");
    let rel_fd = (dv - dv_fd).abs() / dv.abs();
    assert!(rel_fd <= GATE, "cone dV/dh = {dv} vs FD {dv_fd}");

    let fd = fd_velocities(build_cone_height, CONE_H0, H_FD, &plan).expect("fd vel");
    let cmp = compare_velocities(&seam.velocities, &fd);
    assert!(
        cmp.max_rel_err <= GATE,
        "cone dx/dh max rel err {:.3e} at node {}",
        cmp.max_rel_err,
        cmp.worst_node
    );

    let base_seam = evaluate_with_sensitivity(&base, &plan, &ParamSeeding::new()).expect("base");
    let w = volume_gradient(&base_seam.positions, &base_seam.triangles);
    let cots = evaluate_with_pullback(&base, &plan, &w).expect("pullback");
    let rev = cots.contract(&seeding);
    assert!(
        (rev - dv).abs() / dv.abs() <= AGREE,
        "cone height reverse {rev} vs forward {dv}"
    );
}

// ============================================================================
// Torus
// ============================================================================

const TORUS_R: f64 = 8.0;
const TORUS_MINOR: f64 = 3.0;

fn build_torus_minor(r: f64) -> BRepSolid {
    make_torus(TORUS_R, r, SEGMENTS)
}

fn build_torus_major(major: f64) -> BRepSolid {
    make_torus(major, TORUS_MINOR, SEGMENTS)
}

fn torus_seeding(brep: &BRepSolid, seed: SurfaceSeed) -> ParamSeeding {
    let mut s = ParamSeeding::new();
    let n = s.seed_where(
        &brep.geometry,
        |surf| surf.surface_type() == SurfaceKind::Torus,
        seed,
    );
    assert_eq!(n, 1, "expected exactly the torus surface");
    s
}

fn torus_gate(
    build: fn(f64) -> BRepSolid,
    theta0: f64,
    seed: SurfaceSeed,
    plan: &FrozenPlan,
    base: &BRepSolid,
    label: &str,
) {
    let seeding = torus_seeding(base, seed);
    let seam = evaluate_with_sensitivity(base, plan, &seeding).expect("seam");
    let (_, dv) = volume_with_derivative(&seam);

    let dv_fd = fd_volume_derivative(build, theta0, H_FD, plan).expect("fd vol");
    let rel_fd = (dv - dv_fd).abs() / dv.abs().max(1.0);
    assert!(
        rel_fd <= GATE,
        "{label}: dV/dθ = {dv} vs FD {dv_fd} (rel {rel_fd:.3e})"
    );

    let fd = fd_velocities(build, theta0, H_FD, plan).expect("fd vel");
    let cmp = compare_velocities(&seam.velocities, &fd);
    assert!(
        cmp.max_rel_err <= GATE,
        "{label}: dx/dθ max rel err {:.3e} at node {}",
        cmp.max_rel_err,
        cmp.worst_node
    );

    let base_seam = evaluate_with_sensitivity(base, plan, &ParamSeeding::new()).expect("base");
    let w = volume_gradient(&base_seam.positions, &base_seam.triangles);
    let cots = evaluate_with_pullback(base, plan, &w).expect("pullback");
    let rev = cots.contract(&seeding);
    assert!(
        (rev - dv).abs() / dv.abs().max(1.0) <= AGREE,
        "{label}: reverse {rev} vs forward {dv}"
    );
}

#[test]
fn torus_minor_radius_derivative() {
    let base = build_torus_minor(TORUS_MINOR);
    let plan = capture_plan(&base, &params()).expect("capture torus");

    // Continuum check: V = 2π²R r², so dV/dr = 4π²R r; the seam
    // differentiates the discrete mesh, so this is a sanity band not a gate.
    let seam = evaluate_with_sensitivity(
        &base,
        &plan,
        &torus_seeding(&base, SurfaceSeed::TorusMinorRadius { rate: 1.0 }),
    )
    .expect("seam");
    let (v, dv) = volume_with_derivative(&seam);
    // Two polygonizations (major and minor circles, each an N-gon) put the
    // discrete mesh a few percent under the smooth torus; this is a sanity
    // band, not the correctness gate (that is the FD comparison below).
    let v_cont = 2.0 * PI * PI * TORUS_R * TORUS_MINOR * TORUS_MINOR;
    let dv_cont = 4.0 * PI * PI * TORUS_R * TORUS_MINOR;
    assert!(
        (v - v_cont).abs() / v_cont < 0.05,
        "torus volume {v} vs continuum {v_cont}"
    );
    assert!(
        (dv - dv_cont).abs() / dv_cont < 0.05,
        "torus dV/dr {dv} vs continuum {dv_cont}"
    );

    torus_gate(
        build_torus_minor,
        TORUS_MINOR,
        SurfaceSeed::TorusMinorRadius { rate: 1.0 },
        &plan,
        &base,
        "torus minor r",
    );
}

#[test]
fn torus_major_radius_derivative() {
    let base = build_torus_major(TORUS_R);
    let plan = capture_plan(&base, &params()).expect("capture torus");
    torus_gate(
        build_torus_major,
        TORUS_R,
        SurfaceSeed::TorusMajorRadius { rate: 1.0 },
        &plan,
        &base,
        "torus major R",
    );
}
