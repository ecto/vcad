//! M9 — many-parameter optimization: reverse-mode gradients + L-BFGS.
//!
//! Forward mode ([`objective_gradient`]) costs one seam pass per parameter;
//! reverse mode ([`objective_gradient_reverse`]) prices every parameter from
//! one pullback. This gate proves the pair on a genuinely multi-parameter
//! model — a rounded, drilled box with **five** independent parameters:
//!
//! ```text
//! θ = [sx, sy, sz, r_fillet, r_hole]
//! ```
//!
//! built as `fillet_all_edges(cube(sx, sy, sz), r_fillet)` with a centered
//! through-hole of radius `r_hole` drilled along ẑ. The seedings are
//! hand-written (seeding synthesis is a later milestone): the three box
//! dimensions each translate their far face, their adjacent edge blends and
//! corner spheres, and the recentring hole; the fillet radius drives all
//! twenty blends with composite radius-plus-retreat seeds (the M4/M5
//! recipe generalized per-axis); the hole radius drives the one hole-wall
//! cylinder.
//!
//! Gates:
//! 1. Reverse gradient == forward `objective_gradient` to ≤1e-11 relative,
//!    per component, at θ₀ — the two share row construction and differ only
//!    in linear-algebra order, so this pins the analytic mesh gradients of
//!    the centroid- and inertia-based QoIs against the trusted dual-number
//!    mass-property pipeline.
//! 2. Finite-difference spot-check of gradient components to ≤1e-6.
//! 3. Local identifiability: the 5×5 QoI Jacobian is well away from singular.
//! 4. Recovery: from a distinct θ₀, `minimize_lbfgs` recovers θ* to 1e-3,
//!    in fewer objective evaluations than the GD `minimize` baseline.

use std::cell::Cell;
use std::time::Instant;

use vcad_kernel::Solid;
use vcad_kernel_diff::{
    evaluate_with_pullback, evaluate_with_sensitivity, mass_properties_with_derivative, minimize,
    minimize_lbfgs, objective_gradient, objective_gradient_reverse, MeshObjective, OptimizeOptions,
    ParamSeeding, SeamMesh, StopReason, SurfaceSeed,
};
use vcad_kernel_geom::{CylinderSurface, Plane, SphereSurface};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::{make_cube, BRepSolid};
use vcad_kernel_tessellate::frozen::{capture_plan, evaluate_plan};
use vcad_kernel_tessellate::TessellationParams;

const RHO: f64 = 1.0;
/// Geometric tolerance for classifying a surface by its pinned coordinates.
const GEOM_EPS: f64 = 1e-6;
/// Reverse-vs-forward agreement floor (shared rows, different LA order).
const AGREE: f64 = 1e-11;
/// FD spot-check gate.
const FD_GATE: f64 = 1e-6;
/// FD step for the spot-check gate.
const FD_GATE_H: f64 = 1e-6;

// ---------------------------------------------------------------- the model

fn build(theta: &[f64]) -> BRepSolid {
    let (sx, sy, sz, r, rhole) = (theta[0], theta[1], theta[2], theta[3], theta[4]);
    let rounded = vcad_kernel_fillet::fillet_all_edges(&make_cube(sx, sy, sz), r);
    let tool = Solid::cylinder(rhole, sz + 2.0, 24).translate(sx / 2.0, sy / 2.0, -1.0);
    Solid::from_brep(rounded)
        .difference(&tool)
        .as_brep()
        .expect("boolean stays BRep")
        .clone()
}

fn tess() -> TessellationParams {
    TessellationParams {
        circle_segments: 16,
        height_segments: 2,
        ..Default::default()
    }
}

/// The box dimensions read back from the built model (corner at the origin,
/// so the AABB max is `(sx, sy, sz)`).
fn dims(brep: &BRepSolid) -> [f64; 3] {
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

/// The through-hole wall is the ẑ-axis cylinder centered on the box axis;
/// the fillet blends sit on edges, never on the axis.
fn is_hole(c: &CylinderSurface, d: [f64; 3]) -> bool {
    c.axis.as_ref().cross(Vec3::z()).norm() < 1e-9
        && (c.center.x - d[0] / 2.0).abs() < GEOM_EPS
        && (c.center.y - d[1] / 2.0).abs() < GEOM_EPS
}

/// The fillet radius, read off any blend cylinder (they all carry it).
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

/// Parameter `k`'s surface seeding on the freshly built B-rep. `k` in
/// `0..=2` is a box dimension, `3` the fillet radius, `4` the hole radius.
fn seeding_for(brep: &BRepSolid, k: usize) -> ParamSeeding {
    let d = dims(brep);
    let r = fillet_radius(brep, d);
    let mut seeding = ParamSeeding::new();

    match k {
        // A box dimension: the far face translates, its adjacent blends and
        // corner spheres (pinned at dim − r on this axis) ride with it, and
        // the recentring hole follows at half rate.
        0..=2 => {
            let e = axis(k);
            for (i, s) in brep.geometry.surfaces.iter().enumerate() {
                if let Some(p) = s.as_any().downcast_ref::<Plane>() {
                    if (*p.normal_dir.as_ref() - e).norm() < 1e-9 {
                        seeding.seed(i, SurfaceSeed::Translate { velocity: e });
                    }
                } else if let Some(c) = s.as_any().downcast_ref::<CylinderSurface>() {
                    if is_hole(c, d) {
                        // Hole recentres at (sx/2, sy/2); ẑ dimension does not
                        // move it.
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
        // The fillet radius: composite radius-plus-retreat on every blend
        // and corner (the M4/M5 recipe, per-axis). Planes and the hole are
        // radius-independent.
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
        // The hole radius: the one hole-wall cylinder grows.
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

// -------------------------------------------------------- the QoIs and cost

/// The five quantities of interest: volume, centroid x/y/z, and the inertia
/// about the origin's zz component (ρ = 1 ⇒ `P_xx + P_yy`). Their targets
/// pin all five parameters (dims via the centroid, `r`/`r_hole` via volume
/// and inertia).
#[derive(Clone, Copy)]
struct Targets {
    v: f64,
    c: [f64; 3],
    izz: f64,
}

/// Raw polynomial moments of the closed mesh: volume, first moments, and the
/// two second moments the zz-origin inertia needs. Same integrals as
/// `mass_properties`, so the reverse path's value matches the forward
/// (dual) path's exactly.
fn moments(pos: &[Point3], tris: &[[u32; 3]]) -> (f64, [f64; 3], f64, f64) {
    let mut v = 0.0;
    let mut m = [0.0; 3];
    let (mut p00, mut p11) = (0.0, 0.0);
    for t in tris {
        let a = pos[t[0] as usize];
        let b = pos[t[1] as usize];
        let c = pos[t[2] as usize];
        let a = [a.x, a.y, a.z];
        let b = [b.x, b.y, b.z];
        let c = [c.x, c.y, c.z];
        let det = a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
            + a[2] * (b[0] * c[1] - b[1] * c[0]);
        let vt = det / 6.0;
        v += vt;
        let s = [a[0] + b[0] + c[0], a[1] + b[1] + c[1], a[2] + b[2] + c[2]];
        for i in 0..3 {
            m[i] += vt * s[i] / 4.0;
        }
        p00 += vt / 20.0 * (a[0] * a[0] + b[0] * b[0] + c[0] * c[0] + s[0] * s[0]);
        p11 += vt / 20.0 * (a[1] * a[1] + b[1] * b[1] + c[1] * c[1] + s[1] * s[1]);
    }
    (v, m, p00, p11)
}

/// The QoI vector `[V, cx, cy, cz, I_zz]` at θ, under a fixed frozen plan.
fn qoi_vector(theta: &[f64], plan: &vcad_kernel_tessellate::frozen::FrozenPlan) -> [f64; 5] {
    let mesh = evaluate_plan(&build(theta), plan).expect("qoi rebuild");
    let (v, m, p00, p11) = moments(&mesh.positions, &mesh.triangles);
    [v, m[0] / v, m[1] / v, m[2] / v, p00 + p11]
}

/// Analytic per-node gradient of `J = Σ cᵩ·(∂ϕ/∂x)` where the moment
/// coefficients `(cv, cm, cp0, cp1)` price `V`, the three first moments, and
/// `P_xx`, `P_yy`. This is the divergence-theorem `volume_gradient` pattern
/// (contract.rs) extended to first and second moments; all four integrands
/// are polynomials in the node positions, so their gradients are exact.
fn qoi_mesh_gradient(
    pos: &[Point3],
    tris: &[[u32; 3]],
    cv: f64,
    cm: Vec3,
    cp0: f64,
    cp1: f64,
) -> Vec<Vec3> {
    let cp = [cp0, cp1];
    let mut dj = vec![Vec3::new(0.0, 0.0, 0.0); pos.len()];
    for t in tris {
        let (i0, i1, i2) = (t[0] as usize, t[1] as usize, t[2] as usize);
        let a = Vec3::new(pos[i0].x, pos[i0].y, pos[i0].z);
        let b = Vec3::new(pos[i1].x, pos[i1].y, pos[i1].z);
        let c = Vec3::new(pos[i2].x, pos[i2].y, pos[i2].z);
        let v = a.dot(b.cross(c)) / 6.0;
        let gv0 = b.cross(c) / 6.0;
        let gv1 = c.cross(a) / 6.0;
        let gv2 = a.cross(b) / 6.0;
        let s = a + b + c;
        let sc = [s.x, s.y];
        // Σ cp[i] (a_i² + b_i² + c_i² + s_i²), the volume-gradient weight of
        // the second-moment term.
        let ac = [a.x, a.y];
        let bc = [b.x, b.y];
        let cc = [c.x, c.y];
        let mut term_sum = 0.0;
        for i in 0..2 {
            term_sum += cp[i] * (ac[i] * ac[i] + bc[i] * bc[i] + cc[i] * cc[i] + sc[i] * sc[i]);
        }
        // Scalar that multiplies each node's volume gradient: V (direct),
        // plus the first-moment and second-moment shares that ride on ∂v.
        let common = cv + cm.dot(s) / 4.0 + term_sum / 20.0;
        // First-moment term contributes (v/4)·cm at every node.
        let mvec = cm * (v / 4.0);
        // Second-moment term's explicit e_i part, per node (x, y only).
        let p_e = |co: [f64; 2]| {
            Vec3::new(
                v / 10.0 * cp[0] * (co[0] + sc[0]),
                v / 10.0 * cp[1] * (co[1] + sc[1]),
                0.0,
            )
        };
        dj[i0] += gv0 * common + mvec + p_e(ac);
        dj[i1] += gv1 * common + mvec + p_e(bc);
        dj[i2] += gv2 * common + mvec + p_e(cc);
    }
    dj
}

/// The recovery objective as a [`MeshObjective`] (reverse mode): squared
/// relative miss of the five QoIs, with the analytic mesh gradient.
struct QoiMatch {
    t: Targets,
}

impl MeshObjective for QoiMatch {
    fn value_and_mesh_gradient(&self, seam: &SeamMesh) -> (f64, Vec<Vec3>) {
        let (v, m, p00, p11) = moments(&seam.positions, &seam.triangles);
        let cx = m[0] / v;
        let cy = m[1] / v;
        let cz = m[2] / v;
        let izz = p00 + p11;

        let mv = (v - self.t.v) / self.t.v;
        let mcx = (cx - self.t.c[0]) / self.t.c[0];
        let mcy = (cy - self.t.c[1]) / self.t.c[1];
        let mcz = (cz - self.t.c[2]) / self.t.c[2];
        let mi = (izz - self.t.izz) / self.t.izz;
        let j = mv * mv + mcx * mcx + mcy * mcy + mcz * mcz + mi * mi;

        // Outer derivatives dJ/dQoI.
        let dv = 2.0 * mv / self.t.v;
        let dcx = 2.0 * mcx / self.t.c[0];
        let dcy = 2.0 * mcy / self.t.c[1];
        let dcz = 2.0 * mcz / self.t.c[2];
        let di = 2.0 * mi / self.t.izz;

        // Chain rule to raw-moment coefficients: cx = M_x/V, so
        // ∂cx/∂M_x = 1/V and ∂cx/∂V = −cx/V; I_zz = P_xx + P_yy.
        let cv = dv - dcx * cx / v - dcy * cy / v - dcz * cz / v;
        let cm = Vec3::new(dcx / v, dcy / v, dcz / v);
        let dj = qoi_mesh_gradient(&seam.positions, &seam.triangles, cv, cm, di, di);
        (j, dj)
    }
}

/// The same objective in forward form `(J, dJ/dθ_k)` for a seam whose
/// velocities carry parameter `k` — built on the trusted dual-number
/// `mass_properties_with_derivative`, so gate 1 cross-checks the two
/// independent implementations.
fn forward_objective(t: Targets) -> impl Fn(&SeamMesh) -> (f64, f64) {
    move |seam: &SeamMesh| {
        let (p, dp) = mass_properties_with_derivative(seam, RHO);
        let mv = (p.volume - t.v) / t.v;
        let mcx = (p.centroid.x - t.c[0]) / t.c[0];
        let mcy = (p.centroid.y - t.c[1]) / t.c[1];
        let mcz = (p.centroid.z - t.c[2]) / t.c[2];
        let mi = (p.inertia_origin[2][2] - t.izz) / t.izz;
        let j = mv * mv + mcx * mcx + mcy * mcy + mcz * mcz + mi * mi;
        let dj = 2.0 * mv * dp.volume / t.v
            + 2.0 * mcx * dp.centroid.x / t.c[0]
            + 2.0 * mcy * dp.centroid.y / t.c[1]
            + 2.0 * mcz * dp.centroid.z / t.c[2]
            + 2.0 * mi * dp.inertia_origin[2][2] / t.izz;
        (j, dj)
    }
}

fn targets_at(theta: &[f64]) -> Targets {
    let plan = capture_plan(&build(theta), &tess()).expect("capture targets");
    let q = qoi_vector(theta, &plan);
    Targets {
        v: q[0],
        c: [q[1], q[2], q[3]],
        izz: q[4],
    }
}

/// Determinant of a 5×5 matrix by Gaussian elimination with partial
/// pivoting (also returns the smallest pivot magnitude as a conditioning
/// proxy).
fn det5(mut a: [[f64; 5]; 5]) -> (f64, f64) {
    let mut det = 1.0;
    let mut min_pivot = f64::MAX;
    for col in 0..5 {
        let mut piv = col;
        for r in col + 1..5 {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if piv != col {
            a.swap(piv, col);
            det = -det;
        }
        let pivot = a[col];
        let d = pivot[col];
        min_pivot = min_pivot.min(d.abs());
        if d == 0.0 {
            return (0.0, 0.0);
        }
        det *= d;
        for row in a.iter_mut().skip(col + 1) {
            let f = row[col] / d;
            for (cell, &pv) in row.iter_mut().zip(pivot.iter()).skip(col) {
                *cell -= f * pv;
            }
        }
    }
    (det, min_pivot)
}

// --------------------------------------------------------------- the gates

const THETA_STAR: [f64; 5] = [10.0, 12.0, 8.0, 1.8, 2.2];
const THETA0: [f64; 5] = [8.0, 14.0, 6.0, 1.2, 1.6];

fn options() -> OptimizeOptions {
    OptimizeOptions {
        max_iters: 200,
        initial_step: 0.1,
        min_step: 1e-10,
        grad_tol: 1e-9,
        bounds: vec![
            (5.0, 16.0),
            (5.0, 16.0),
            (5.0, 16.0),
            (0.5, 2.4),
            (0.8, 2.4),
        ],
    }
}

#[test]
fn m9_reverse_matches_forward_and_fd() {
    let targets = targets_at(&THETA_STAR);
    let fwd = forward_objective(targets);
    let rev = QoiMatch { t: targets };

    // Gate 1: reverse == forward objective_gradient, per component.
    let (jf, gf) =
        objective_gradient(&build, &seeding_for, &fwd, &THETA0, &tess()).expect("forward");
    let (jr, gr) =
        objective_gradient_reverse(&build, &seeding_for, &rev, &THETA0, &tess()).expect("reverse");
    assert!(
        (jf - jr).abs() <= 1e-11 * jf.abs().max(1.0),
        "objective value forward {jf} vs reverse {jr}"
    );
    for k in 0..5 {
        let rel = (gf[k] - gr[k]).abs() / gf[k].abs().max(1.0);
        eprintln!(
            "k{k}: forward {:+.10} reverse {:+.10} rel {:.2e}",
            gf[k], gr[k], rel
        );
        assert!(
            rel <= AGREE,
            "component {k}: forward {} vs reverse {}",
            gf[k],
            gr[k]
        );
    }

    // Gate 2: FD spot-check under a fixed frozen plan at θ₀.
    let plan = capture_plan(&build(&THETA0), &tess()).expect("capture theta0");
    let j_of = |theta: &[f64]| -> f64 {
        let q = qoi_vector(theta, &plan);
        let mv = (q[0] - targets.v) / targets.v;
        let mcx = (q[1] - targets.c[0]) / targets.c[0];
        let mcy = (q[2] - targets.c[1]) / targets.c[1];
        let mcz = (q[3] - targets.c[2]) / targets.c[2];
        let mi = (q[4] - targets.izz) / targets.izz;
        mv * mv + mcx * mcx + mcy * mcy + mcz * mcz + mi * mi
    };
    // The hole radius and one in-plane dimension differentiate cleanly under
    // the fixed plan; the other dimensions and the fillet radius carry the
    // irreducible frame/correspondence noise of rebuilding a filleted, drilled
    // solid under a plan captured elsewhere (the M4 fillet-frame caveat) — so
    // this is a spot-check, not a per-component gate. Gate 1 is the exact one.
    let mut passed = 0;
    for k in 0..5 {
        let mut plus = THETA0;
        let mut minus = THETA0;
        plus[k] += FD_GATE_H;
        minus[k] -= FD_GATE_H;
        let fd = (j_of(&plus) - j_of(&minus)) / (2.0 * FD_GATE_H);
        let rel = (gr[k] - fd).abs() / fd.abs().max(1e-3);
        eprintln!(
            "FD k{k}: reverse {:+.8} fd {:+.8} rel {:.2e}",
            gr[k], fd, rel
        );
        if rel <= FD_GATE {
            passed += 1;
        }
    }
    assert!(passed >= 2, "fewer than two FD components within {FD_GATE}");

    // Gate 3: local identifiability — the QoI Jacobian near θ* is nonsingular
    // and reasonably conditioned (its determinant and smallest pivot are far
    // from zero), so the five parameters are locally recoverable.
    let plan_star = capture_plan(&build(&THETA_STAR), &tess()).expect("capture star");
    let mut jac = [[0.0; 5]; 5]; // rows = QoIs, cols = params
    for k in 0..5 {
        let mut plus = THETA_STAR;
        let mut minus = THETA_STAR;
        plus[k] += 1e-4;
        minus[k] -= 1e-4;
        let qp = qoi_vector(&plus, &plan_star);
        let qm = qoi_vector(&minus, &plan_star);
        for q in 0..5 {
            jac[q][k] = (qp[q] - qm[q]) / 2e-4;
        }
    }
    let (det, min_pivot) = det5(jac);
    eprintln!("QoI Jacobian det {det:.4e}, min pivot {min_pivot:.4e}");
    assert!(
        det.abs() > 1e-3,
        "QoI Jacobian near-singular (det {det:.3e})"
    );
    assert!(
        min_pivot > 1e-3,
        "QoI Jacobian ill-conditioned (min pivot {min_pivot:.3e})"
    );
}

#[test]
fn m9_lbfgs_recovers_theta_star() {
    let targets = targets_at(&THETA_STAR);
    let fwd = forward_objective(targets);
    let rev = QoiMatch { t: targets };

    // --- L-BFGS (reverse mode) ---
    let lbfgs_evals = Cell::new(0usize);
    let build_lbfgs = |t: &[f64]| {
        lbfgs_evals.set(lbfgs_evals.get() + 1);
        build(t)
    };
    let lbfgs = minimize_lbfgs(
        &build_lbfgs,
        &seeding_for,
        &rev,
        &THETA0,
        &tess(),
        &options(),
    )
    .expect("lbfgs");
    let lbfgs_evals = lbfgs_evals.get();

    let err = |theta: &[f64]| {
        theta
            .iter()
            .zip(&THETA_STAR)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f64, f64::max)
    };
    eprintln!(
        "L-BFGS: stop {:?}, iters {}, evals {}, J {:.3e}, theta {:?}, ||theta-theta*||inf {:.3e}",
        lbfgs.stop,
        lbfgs.history.len() - 1,
        lbfgs_evals,
        lbfgs.objective,
        lbfgs.theta,
        err(&lbfgs.theta)
    );
    assert!(
        lbfgs.stop != StopReason::MaxIters,
        "L-BFGS hit the iteration budget"
    );
    assert!(
        err(&lbfgs.theta) < 1e-3,
        "L-BFGS recovered {:?} vs theta* {:?}",
        lbfgs.theta,
        THETA_STAR
    );

    // --- GD baseline (forward mode) on the identical problem ---
    let gd_evals = Cell::new(0usize);
    let build_gd = |t: &[f64]| {
        gd_evals.set(gd_evals.get() + 1);
        build(t)
    };
    let gd = minimize(&build_gd, &seeding_for, &fwd, &THETA0, &tess(), &options()).expect("gd");
    let gd_evals = gd_evals.get();
    eprintln!(
        "GD:     stop {:?}, iters {}, evals {}, J {:.3e}, ||theta-theta*||inf {:.3e}",
        gd.stop,
        gd.history.len() - 1,
        gd_evals,
        gd.objective,
        err(&gd.theta)
    );

    // Soft comparison (reported, not a hard invariant): reverse-mode L-BFGS
    // reaches the optimum in fewer objective evaluations than GD.
    eprintln!(
        "objective evaluations: L-BFGS {lbfgs_evals} vs GD {gd_evals} (ratio {:.2}x)",
        gd_evals as f64 / lbfgs_evals as f64
    );
    assert!(
        lbfgs_evals < gd_evals,
        "L-BFGS used {lbfgs_evals} evals, GD used {gd_evals}"
    );
}

#[test]
fn m9_reverse_is_cheaper_per_iterate() {
    // Per-iterate cost of the *differentiation* step, isolated from the
    // shared build + capture: forward mode is n=5 seam passes; reverse is one
    // seam pass + one pullback + 5 contractions.
    let targets = targets_at(&THETA_STAR);
    let fwd = forward_objective(targets);
    let rev = QoiMatch { t: targets };
    let reps = 20;

    let brep = build(&THETA_STAR);
    let plan = capture_plan(&brep, &tess()).expect("capture");

    // Also report the shared overhead, so the ratio is honest about what it
    // does and does not include.
    let t0 = Instant::now();
    for _ in 0..reps {
        let b = build(&THETA_STAR);
        let _ = capture_plan(&b, &tess()).unwrap();
    }
    let build_capture_ms = t0.elapsed().as_secs_f64() * 1e3 / reps as f64;

    // Forward: n seam passes.
    let t0 = Instant::now();
    for _ in 0..reps {
        for k in 0..5 {
            let seam = evaluate_with_sensitivity(&brep, &plan, &seeding_for(&brep, k)).unwrap();
            std::hint::black_box(fwd(&seam));
        }
    }
    let fwd_ms = t0.elapsed().as_secs_f64() * 1e3 / reps as f64;

    // Reverse: one positions-only seam pass + one pullback + n contractions.
    let t0 = Instant::now();
    for _ in 0..reps {
        let seam0 = evaluate_with_sensitivity(&brep, &plan, &ParamSeeding::new()).unwrap();
        let (_, w) = rev.value_and_mesh_gradient(&seam0);
        let cots = evaluate_with_pullback(&brep, &plan, &w).unwrap();
        for k in 0..5 {
            std::hint::black_box(cots.contract(&seeding_for(&brep, k)));
        }
    }
    let rev_ms = t0.elapsed().as_secs_f64() * 1e3 / reps as f64;

    eprintln!(
        "shared build+capture {build_capture_ms:.2} ms/iterate; \
         differentiation: forward {fwd_ms:.2} ms (5 seam passes) vs reverse {rev_ms:.2} ms \
         (1 seam + 1 pullback + 5 dots); ratio {:.2}x",
        fwd_ms / rev_ms
    );
    assert!(fwd_ms > 0.0 && rev_ms > 0.0);
}
