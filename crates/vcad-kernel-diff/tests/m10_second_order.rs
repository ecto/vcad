//! M10 — second-order derivatives.
//!
//! Two families, each stating exactly what it computes:
//!
//! 1. **Gauss–Newton curvature** for least-squares mass-property objectives:
//!    `H_GN = 2 JᵀJ` from the QoI Jacobian the seam already prices, validated
//!    on the M9 five-parameter model near its optimum against a finite
//!    difference of the *exact* gradient (= the full Hessian; near the optimum
//!    the two differ only by the residual-curvature term, which vanishes at a
//!    perfect fit). Gated loosely — the fillet model's re-capture
//!    correspondence noise dominates the FD Hessian (documented).
//!
//! 2. **Exact `d²V/dθ²`** for the volume QoI via second-order node kinematics:
//!    the boolean hole against its quadratic closed form
//!    `d²V/dr² = −N·sin(2π/N)·t` and a central FD of the analytic `dV/dr`;
//!    the cylinder height (`d²V/dh² = 0`, `V` linear); and the rounded cube
//!    `d²V/dr²` against a central FD of the analytic `dV/dr` (plane, cylinder,
//!    and sphere lift nodes, boundary rings, and tangency-completed corners
//!    all exercised).

use vcad_kernel::Solid;
use vcad_kernel_diff::{
    evaluate_with_second_derivative, evaluate_with_sensitivity, gauss_newton_gradient,
    gauss_newton_hessian, mass_properties, mass_properties_with_derivative, volume_with_derivative,
    volume_with_second_derivative, ParamSeeding, SecondOrderSeeding, SurfaceSeed,
};
use vcad_kernel_geom::{CylinderSurface, Plane, SphereSurface};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::{make_cube, make_cylinder, BRepSolid};
use vcad_kernel_tessellate::frozen::capture_plan;
use vcad_kernel_tessellate::TessellationParams;

// ============================================================ volume gates

/// Second-order volume of a fresh capture at `theta`, given a first-order
/// velocity seeding derived from the built B-rep. Returns `(V, dV/dθ, d²V/dθ²)`.
fn second_order_volume(
    build: &dyn Fn(f64) -> BRepSolid,
    velocity_seeding: &dyn Fn(&BRepSolid) -> ParamSeeding,
    theta: f64,
    params: &TessellationParams,
) -> (f64, f64, f64) {
    let brep = build(theta);
    let plan = capture_plan(&brep, params).expect("capture");
    let seeding = SecondOrderSeeding::linear(velocity_seeding(&brep));
    let seam = evaluate_with_second_derivative(&brep, &plan, &seeding).expect("second-order seam");
    volume_with_second_derivative(&seam)
}

/// The analytic `dV/dθ` of a fresh capture at `theta` (first-order seam).
fn analytic_dv(
    build: &dyn Fn(f64) -> BRepSolid,
    velocity_seeding: &dyn Fn(&BRepSolid) -> ParamSeeding,
    theta: f64,
    params: &TessellationParams,
) -> f64 {
    let brep = build(theta);
    let plan = capture_plan(&brep, params).expect("capture");
    let seam = evaluate_with_sensitivity(&brep, &plan, &velocity_seeding(&brep))
        .expect("first-order seam");
    volume_with_derivative(&seam).1
}

/// Central FD of the analytic `dV/dθ` at `theta` (a fresh capture per probe):
/// the honest second-derivative oracle for the re-captured family.
fn fd_of_analytic_dv(
    build: &dyn Fn(f64) -> BRepSolid,
    velocity_seeding: &dyn Fn(&BRepSolid) -> ParamSeeding,
    theta: f64,
    h: f64,
    params: &TessellationParams,
) -> f64 {
    let plus = analytic_dv(build, velocity_seeding, theta + h, params);
    let minus = analytic_dv(build, velocity_seeding, theta - h, params);
    (plus - minus) / (2.0 * h)
}

const HOLE_L: f64 = 10.0;
const HOLE_W: f64 = 8.0;
const HOLE_T: f64 = 5.0;
const HOLE_SEGMENTS: u32 = 32;

fn build_hole(r: f64) -> BRepSolid {
    let block = Solid::cube(HOLE_L, HOLE_W, HOLE_T);
    let tool =
        Solid::cylinder(r, HOLE_T + 2.0, HOLE_SEGMENTS).translate(HOLE_L / 2.0, HOLE_W / 2.0, -1.0);
    block
        .difference(&tool)
        .as_brep()
        .expect("boolean stays BRep")
        .clone()
}

/// Seed every cylinder's radius at rate 1 (the hole model has exactly one).
fn hole_seeding(brep: &BRepSolid) -> ParamSeeding {
    let mut s = ParamSeeding::new();
    let n = s.seed_where(
        &brep.geometry,
        |surf| surf.as_any().downcast_ref::<CylinderSurface>().is_some(),
        SurfaceSeed::CylinderRadius { rate: 1.0 },
    );
    assert_eq!(n, 1, "hole model should carry exactly one cylinder");
    s
}

#[test]
fn m10_hole_second_derivative_matches_closed_form_and_fd() {
    let params = TessellationParams {
        circle_segments: HOLE_SEGMENTS,
        height_segments: 3,
        ..Default::default()
    };
    let r0 = 2.5;
    let (v, dv, ddv) = second_order_volume(&build_hole, &hole_seeding, r0, &params);

    // V(r) = LWT − ½ N sin(2π/N) r² T (inscribed N-gon rim) ⇒
    // dV/dr = −N sin(2π/N) r T, d²V/dr² = −N sin(2π/N) T (constant).
    let n = HOLE_SEGMENTS as f64;
    let sector = 2.0 * std::f64::consts::PI / n;
    let v_exact = HOLE_L * HOLE_W * HOLE_T - 0.5 * n * sector.sin() * r0 * r0 * HOLE_T;
    let dv_exact = -n * sector.sin() * r0 * HOLE_T;
    let ddv_exact = -n * sector.sin() * HOLE_T;

    assert!((v - v_exact).abs() / v_exact < 1e-12, "V {v} vs {v_exact}");
    assert!(
        (dv - dv_exact).abs() / dv_exact.abs() < 1e-9,
        "dV {dv} vs {dv_exact}"
    );
    let rel_closed = (ddv - ddv_exact).abs() / ddv_exact.abs();
    assert!(
        rel_closed <= 1e-9,
        "d²V/dr² {ddv} vs closed form {ddv_exact} (rel {rel_closed:.3e})"
    );

    // Central FD of the analytic dV/dr at h = 1e-6.
    let fd = fd_of_analytic_dv(&build_hole, &hole_seeding, r0, 1e-6, &params);
    let rel_fd = (ddv - fd).abs() / ddv_exact.abs();
    eprintln!(
        "hole d²V/dr²: analytic {ddv:.6} closed {ddv_exact:.6} FD {fd:.6} (rel_fd {rel_fd:.3e})"
    );
    assert!(
        rel_fd <= 1e-6,
        "d²V/dr² {ddv} vs FD {fd} (rel {rel_fd:.3e})"
    );
}

#[test]
fn m10_cylinder_height_second_derivative_is_zero() {
    let params = TessellationParams {
        circle_segments: 24,
        height_segments: 4,
        ..Default::default()
    };
    let build = |h: f64| make_cylinder(5.0, h, 24);
    // The top cap plane translates at ẑ; V = area·h is linear ⇒ d²V/dh² = 0.
    let seeding = |brep: &BRepSolid| {
        let mut s = ParamSeeding::new();
        let n = s.seed_where(
            &brep.geometry,
            |surf| {
                surf.as_any()
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
        assert_eq!(n, 1, "expected exactly the top cap plane");
        s
    };
    let (_, dv, ddv) = second_order_volume(&build, &seeding, 8.0, &params);
    let area = 0.5 * 24.0 * (2.0 * std::f64::consts::PI / 24.0).sin() * 25.0;
    eprintln!("cylinder height: dV/dh {dv:.6} (area {area:.6}), d²V/dh² {ddv:.3e}");
    assert!((dv - area).abs() / area < 1e-9, "dV/dh {dv} vs area {area}");
    assert!(ddv.abs() < 1e-6, "d²V/dh² should vanish, got {ddv:.3e}");
}

const RC_A: f64 = 10.0;

fn build_rc(r: f64) -> BRepSolid {
    vcad_kernel_fillet::fillet_all_edges(&make_cube(RC_A, RC_A, RC_A), r)
}

/// The M4/M5 fillet-radius seeding: every blend radius grows at rate 1 while
/// its center retreats from the edge.
fn rc_seeding(brep: &BRepSolid) -> ParamSeeding {
    // Read the radius back off any blend cylinder (they all carry it).
    let r = brep
        .geometry
        .surfaces
        .iter()
        .find_map(|s| {
            s.as_any()
                .downcast_ref::<CylinderSurface>()
                .map(|c| c.radius)
        })
        .expect("rounded cube has blend cylinders");
    let a = RC_A;
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

#[test]
fn m10_rounded_cube_second_derivative_matches_fd() {
    let params = TessellationParams {
        circle_segments: 16,
        height_segments: 2,
        ..Default::default()
    };
    let r0 = 1.5;
    let (_, dv, ddv) = second_order_volume(&build_rc, &rc_seeding, r0, &params);
    // Every node position is linear in r here (blend centers retreat and radii
    // grow at constant rate), so the second-order vertex/lift machinery
    // correctly computes *zero* node acceleration and d²V/dr² is the
    // position-curvature term alone — asserted below.
    let max_accel = {
        let brep = build_rc(r0);
        let plan = capture_plan(&brep, &params).unwrap();
        let seam = evaluate_with_second_derivative(
            &brep,
            &plan,
            &SecondOrderSeeding::linear(rc_seeding(&brep)),
        )
        .unwrap();
        seam.accelerations
            .iter()
            .map(|a| a.norm())
            .fold(0.0_f64, f64::max)
    };
    assert!(
        max_accel < 1e-9,
        "expected zero node acceleration, max {max_accel:.3e}"
    );

    // FD of the analytic dV/dr. A filleted solid re-captured at r ± h carries
    // O(1e-4)-scale mesh-correspondence jitter (the M9 fillet-frame caveat),
    // so a 1e-6 step sits in the noise; a 1e-3 step clears it and the central
    // difference of the (smooth in r) analytic derivative converges to the
    // second derivative. Gated at 5e-3, well inside the 1e-3-scale residual.
    let fd = fd_of_analytic_dv(&build_rc, &rc_seeding, r0, 1e-3, &params);
    let rel = (ddv - fd).abs() / fd.abs().max(1.0);
    eprintln!(
        "rounded cube: dV/dr {dv:.6}, d²V/dr² analytic {ddv:.6} vs FD {fd:.6} (rel {rel:.3e})"
    );
    assert!(
        rel <= 5e-3,
        "rounded-cube d²V/dr² {ddv} vs FD {fd} (rel {rel:.3e})"
    );
}

// ================================================= Gauss–Newton curvature

// The M9 five-parameter model: fillet_all_edges(cube(sx,sy,sz), r) drilled by
// a centered ẑ hole of radius r_hole. Replicated here (test-local) so the GN
// gate stands on the same geometry M9's recovery does.

const THETA_STAR: [f64; 5] = [10.0, 12.0, 8.0, 1.8, 2.2];
const GEOM_EPS: f64 = 1e-6;
const RHO: f64 = 1.0;

fn m9_build(theta: &[f64]) -> BRepSolid {
    let (sx, sy, sz, r, rhole) = (theta[0], theta[1], theta[2], theta[3], theta[4]);
    let rounded = vcad_kernel_fillet::fillet_all_edges(&make_cube(sx, sy, sz), r);
    let tool = Solid::cylinder(rhole, sz + 2.0, 24).translate(sx / 2.0, sy / 2.0, -1.0);
    Solid::from_brep(rounded)
        .difference(&tool)
        .as_brep()
        .expect("boolean stays BRep")
        .clone()
}

fn m9_tess() -> TessellationParams {
    TessellationParams {
        circle_segments: 16,
        height_segments: 2,
        ..Default::default()
    }
}

fn m9_dims(brep: &BRepSolid) -> [f64; 3] {
    let mut mx = [f64::MIN; 3];
    for v in brep.topology.vertices.values() {
        mx[0] = mx[0].max(v.point.x);
        mx[1] = mx[1].max(v.point.y);
        mx[2] = mx[2].max(v.point.z);
    }
    mx
}

fn axis(k: usize) -> Vec3 {
    [
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
    ][k]
}
fn coord(p: Point3, k: usize) -> f64 {
    [p.x, p.y, p.z][k]
}
fn is_hole(c: &CylinderSurface, d: [f64; 3]) -> bool {
    c.axis.as_ref().cross(Vec3::z()).norm() < 1e-9
        && (c.center.x - d[0] / 2.0).abs() < GEOM_EPS
        && (c.center.y - d[1] / 2.0).abs() < GEOM_EPS
}
fn fillet_radius(brep: &BRepSolid, d: [f64; 3]) -> f64 {
    brep.geometry
        .surfaces
        .iter()
        .find_map(|s| {
            s.as_any()
                .downcast_ref::<CylinderSurface>()
                .filter(|c| !is_hole(c, d))
                .map(|c| c.radius)
        })
        .expect("rounded box has blend cylinders")
}

fn m9_seeding_for(brep: &BRepSolid, k: usize) -> ParamSeeding {
    let d = m9_dims(brep);
    let r = fillet_radius(brep, d);
    let mut seeding = ParamSeeding::new();
    match k {
        0..=2 => {
            let e = axis(k);
            for (i, s) in brep.geometry.surfaces.iter().enumerate() {
                if let Some(p) = s.as_any().downcast_ref::<Plane>() {
                    if (*p.normal_dir.as_ref() - e).norm() < 1e-9 {
                        seeding.seed(i, SurfaceSeed::Translate { velocity: e });
                    }
                } else if let Some(c) = s.as_any().downcast_ref::<CylinderSurface>() {
                    if is_hole(c, d) {
                        if k < 2 {
                            seeding.seed(i, SurfaceSeed::Translate { velocity: e * 0.5 });
                        }
                    } else if (coord(c.center, k) - (d[k] - r)).abs() < GEOM_EPS {
                        seeding.seed(i, SurfaceSeed::Translate { velocity: e });
                    }
                } else if let Some(sp) = s.as_any().downcast_ref::<SphereSurface>() {
                    if (coord(sp.center, k) - (d[k] - r)).abs() < GEOM_EPS {
                        seeding.seed(i, SurfaceSeed::Translate { velocity: e });
                    }
                }
            }
        }
        3 => {
            let comp = |c: f64, dim: f64| {
                if (c - r).abs() < GEOM_EPS {
                    1.0
                } else if (c - (dim - r)).abs() < GEOM_EPS {
                    -1.0
                } else {
                    0.0
                }
            };
            let retreat =
                |ctr: Point3| Vec3::new(comp(ctr.x, d[0]), comp(ctr.y, d[1]), comp(ctr.z, d[2]));
            for (i, s) in brep.geometry.surfaces.iter().enumerate() {
                if let Some(c) = s.as_any().downcast_ref::<CylinderSurface>() {
                    if !is_hole(c, d) {
                        seeding.seed(i, SurfaceSeed::CylinderRadius { rate: 1.0 });
                        seeding.seed(
                            i,
                            SurfaceSeed::Translate {
                                velocity: retreat(c.center),
                            },
                        );
                    }
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
        }
        _ => {
            for (i, s) in brep.geometry.surfaces.iter().enumerate() {
                if let Some(c) = s.as_any().downcast_ref::<CylinderSurface>() {
                    if is_hole(c, d) {
                        seeding.seed(i, SurfaceSeed::CylinderRadius { rate: 1.0 });
                    }
                }
            }
        }
    }
    seeding
}

/// QoI vector `[V, cx, cy, cz, I_zz]` and its Jacobian `dQ/dθ` (5×5) at
/// `theta`, both from a fresh forward-mode seam capture.
fn m9_qoi_and_jacobian(theta: &[f64]) -> ([f64; 5], [[f64; 5]; 5]) {
    let brep = m9_build(theta);
    let plan = capture_plan(&brep, &m9_tess()).expect("capture");
    let mut q = [0.0; 5];
    let mut jac = [[0.0; 5]; 5]; // rows = QoIs, cols = params
                                 // k is a parameter index (into the seeding) as well as a column index.
    #[allow(clippy::needless_range_loop)]
    for k in 0..5 {
        let seam =
            evaluate_with_sensitivity(&brep, &plan, &m9_seeding_for(&brep, k)).expect("seam");
        let (p, dp) = mass_properties_with_derivative(&seam, RHO);
        if k == 0 {
            q = [
                p.volume,
                p.centroid.x,
                p.centroid.y,
                p.centroid.z,
                p.inertia_origin[2][2],
            ];
        }
        jac[0][k] = dp.volume;
        jac[1][k] = dp.centroid.x;
        jac[2][k] = dp.centroid.y;
        jac[3][k] = dp.centroid.z;
        jac[4][k] = dp.inertia_origin[2][2];
    }
    (q, jac)
}

fn m9_targets() -> [f64; 5] {
    let brep = m9_build(&THETA_STAR);
    let plan = capture_plan(&brep, &m9_tess()).expect("capture");
    let seam = evaluate_with_sensitivity(&brep, &plan, &ParamSeeding::new()).expect("seam");
    let mp = mass_properties(&seam.positions, &seam.triangles, RHO);
    [
        mp.volume,
        mp.centroid.x,
        mp.centroid.y,
        mp.centroid.z,
        mp.inertia_origin[2][2],
    ]
}

/// Relative residuals `r_q = (Q_q − t_q)/t_q` and their Jacobian rows
/// `∂r_q/∂θ = (1/t_q) ∂Q_q/∂θ` — the least-squares residual Jacobian the seam
/// hands Gauss–Newton.
fn residual_jacobian(theta: &[f64], targets: &[f64; 5]) -> (Vec<f64>, Vec<Vec<f64>>) {
    let (q, jac) = m9_qoi_and_jacobian(theta);
    let r: Vec<f64> = (0..5).map(|i| (q[i] - targets[i]) / targets[i]).collect();
    let rows: Vec<Vec<f64>> = (0..5)
        .map(|i| (0..5).map(|k| jac[i][k] / targets[i]).collect())
        .collect();
    (r, rows)
}

/// The exact least-squares gradient `g = 2 Jᵀ r` at `theta`.
fn exact_gradient(theta: &[f64], targets: &[f64; 5]) -> Vec<f64> {
    let (r, rows) = residual_jacobian(theta, targets);
    gauss_newton_gradient(&rows, &r)
}

/// Eigenvalues of a symmetric 5×5 matrix by Jacobi rotation (annihilate the
/// largest off-diagonal each sweep), ascending. Each rotation applies
/// `A ← JᵀAJ` through fresh copies so the pivot block is never double-updated.
fn sym_eigenvalues(mut a: [[f64; 5]; 5]) -> [f64; 5] {
    for _ in 0..200 {
        let (mut p, mut q, mut off) = (0, 1, 0.0);
        // i, j are the pivot indices being searched for (stored as p, q).
        #[allow(clippy::needless_range_loop)]
        for i in 0..5 {
            for j in i + 1..5 {
                if a[i][j].abs() > off {
                    off = a[i][j].abs();
                    p = i;
                    q = j;
                }
            }
        }
        if off < 1e-13 {
            break;
        }
        // Jacobi rotation angle for A ← JᵀAJ with J[p][q] = +sin θ:
        // zeroing a'_pq needs θ = ½ atan2(−2·a_pq, a_pp − a_qq).
        let phi = 0.5 * (-2.0 * a[p][q]).atan2(a[p][p] - a[q][q]);
        let (s, c) = phi.sin_cos();
        // B = A·J (rotate columns p, q).
        let mut b = a;
        for k in 0..5 {
            b[k][p] = c * a[k][p] - s * a[k][q];
            b[k][q] = s * a[k][p] + c * a[k][q];
        }
        // A' = Jᵀ·B (rotate rows p, q).
        let mut d = b;
        for k in 0..5 {
            d[p][k] = c * b[p][k] - s * b[q][k];
            d[q][k] = s * b[p][k] + c * b[q][k];
        }
        a = d;
    }
    let mut ev = [a[0][0], a[1][1], a[2][2], a[3][3], a[4][4]];
    ev.sort_by(|x, y| x.partial_cmp(y).unwrap());
    ev
}

#[test]
fn m10_gauss_newton_hessian_matches_full_hessian_near_optimum() {
    let targets = m9_targets();

    // Gauss–Newton Hessian at θ* (residuals ≈ 0 by construction): H_GN = 2 JᵀJ.
    let (_r, rows) = residual_jacobian(&THETA_STAR, &targets);
    let h_gn_vec = gauss_newton_hessian(&rows);
    let mut h_gn = [[0.0; 5]; 5];
    for i in 0..5 {
        for j in 0..5 {
            h_gn[i][j] = h_gn_vec[i][j];
        }
    }

    // Full Hessian ≈ central FD of the exact gradient g(θ) = 2 Jᵀ r. Near θ*
    // the residual term 2 Σ r_q ∇²r_q vanishes, so H_full ≈ H_GN. The fillet
    // model's re-capture correspondence noise (the M9 fillet-frame caveat)
    // dominates the FD estimate: a filleted, drilled solid captured freshly at
    // each θ ± h carries O(1e-4) mesh jitter, so the FD Hessian is only good
    // to a few percent — hence a *loose, documented* gate (aggregate spectrum
    // agreement, not per-eigenvalue). The step is 1e-3 so the jitter divided
    // by 2h clears the truncation.
    let h = 1e-3;
    let mut h_full = [[0.0; 5]; 5];
    for l in 0..5 {
        let mut plus = THETA_STAR;
        let mut minus = THETA_STAR;
        plus[l] += h;
        minus[l] -= h;
        let gp = exact_gradient(&plus, &targets);
        let gm = exact_gradient(&minus, &targets);
        for k in 0..5 {
            h_full[k][l] = (gp[k] - gm[k]) / (2.0 * h);
        }
    }
    // Symmetrize the FD Hessian before comparing.
    let mut h_sym = [[0.0; 5]; 5];
    for i in 0..5 {
        for j in 0..5 {
            h_sym[i][j] = 0.5 * (h_full[i][j] + h_full[j][i]);
        }
    }

    let ev_gn = sym_eigenvalues(h_gn);
    let ev_full = sym_eigenvalues(h_sym);
    eprintln!("GN eigenvalues   : {ev_gn:?}");
    eprintln!("full eigenvalues : {ev_full:?}");

    // (a) H_GN is positive semidefinite by construction (a Gram matrix).
    assert!(ev_gn[0] >= -1e-6, "H_GN not PSD: min eig {}", ev_gn[0]);

    // (b) Total curvature (trace = Σ eigenvalues) — the robust aggregate — is
    // reproduced tightly.
    let tr_gn: f64 = ev_gn.iter().sum();
    let tr_full: f64 = ev_full.iter().sum();
    let rel_trace = (tr_gn - tr_full).abs() / tr_gn.abs();
    eprintln!("trace: GN {tr_gn:.5e} vs full {tr_full:.5e} (rel {rel_trace:.3e})");
    assert!(
        rel_trace < 0.02,
        "trace GN {tr_gn:.5e} vs full {tr_full:.5e} (rel {rel_trace:.3e})"
    );

    // (c) Whole-matrix agreement: relative Frobenius distance inside the
    // documented FD-noise band.
    let mut num = 0.0;
    let mut den = 0.0;
    for i in 0..5 {
        for j in 0..5 {
            num += (h_gn[i][j] - h_sym[i][j]).powi(2);
            den += h_gn[i][j].powi(2);
        }
    }
    let rel_frob = (num / den).sqrt();
    eprintln!("relative Frobenius ‖H_GN − H_full‖/‖H_GN‖ = {rel_frob:.3e}");
    assert!(
        rel_frob < 0.15,
        "relative Frobenius {rel_frob:.3e} exceeds noise band"
    );
}
