//! **M2 — first true boolean seam.** A through-hole (cylinder subtracted from a
//! block), QoI = volume, parameter `θ = r` (hole radius).
//!
//! Acceptance:
//! - total `dV/dr` matches the closed form to the gate;
//! - rim-node implicit-diff (Pillar 3) matches the finite-difference oracle.
//!
//! The hole is discretized into `N` sectors, so the mesh's exact `dV/dr` is the
//! *polygonal* `−t·N·r·sin(2π/N)`. Analytic-vs-FD (both on the same frozen mesh)
//! is discretization-independent and lands at ~1e-9. The continuous closed form
//! `−2π r t` is matched to `O((2π/N)²)`; `N = 4096` puts that under the gate.

use tang::Scalar;
use vcad_kernel_tessellate::frozen::{
    audit, implicit_sensitivity, models::BlockWithHole, DefiningSystem, ParametricModel,
};

const GATE: f64 = 1e-6;
const H: f64 = 1e-6;

const HALF: f64 = 10.0;
const THICK: f64 = 4.0;
const RADIUS: f64 = 5.0;
const SEGMENTS: usize = 4096;

#[test]
fn m2_total_dvol_dr_matches_closed_form() {
    let model = BlockWithHole::new(HALF, THICK, RADIUS, SEGMENTS);
    let report = audit(&model, H).expect("valid radius step keeps topology");

    // Framework check: analytic (dual) dV/dr vs central-difference oracle,
    // both on the identical frozen mesh — discretization cancels.
    eprintln!(
        "M2 analytic dV/dr = {}, FD dV/dr = {}, rel err = {:e}",
        report.analytic_dvol, report.fd_dvol, report.vol_rel_err
    );
    assert!(
        report.vol_rel_err <= GATE,
        "analytic-vs-FD dV/dr rel err {} > gate",
        report.vol_rel_err
    );

    // Node-level sensitivities (interior cylinder samples + rim) vs FD.
    assert!(
        report.max_node_rel_err <= GATE,
        "node dx/dr max rel err {} > gate",
        report.max_node_rel_err
    );

    // The mesh's exact polygonal closed form.
    let discrete = model.analytic_dvol_discrete();
    assert!((report.analytic_dvol - discrete).abs() / discrete.abs() <= 1e-9);

    // The continuous closed form dV/dr = −2π r t, matched to O((2π/N)²).
    let continuous = model.analytic_dvol_continuous();
    let cont_rel = (report.analytic_dvol - continuous).abs() / continuous.abs();
    eprintln!("M2 continuous closed form = {continuous}, discretization rel err = {cont_rel:e}");
    assert!(
        cont_rel <= GATE,
        "continuous closed-form rel err {cont_rel} > gate (raise SEGMENTS)"
    );
}

/// The defining system for a top-rim node of the through-hole: it lies on the
/// top plane `z = t`, on the hole cylinder of radius `r`, and is pinned to the
/// angular direction `φ`. Its sensitivity `dx/dr` is the Pillar-3 object
/// `−F_x⁻¹ F_θ` — differentiating the *equations that define* the boolean rim,
/// not the kernel code that computed it.
struct TopRim {
    t: f64,
    phi: f64,
}

impl DefiningSystem for TopRim {
    fn eval<S: Scalar>(&self, x: [S; 3], r: S) -> [S; 3] {
        let t = S::from_f64(self.t);
        let sphi = S::from_f64(self.phi.sin());
        let cphi = S::from_f64(self.phi.cos());
        [
            x[2] - t,                          // on plane z = t
            x[0] * x[0] + x[1] * x[1] - r * r, // on cylinder radius r
            x[0] * sphi - x[1] * cphi,         // pinned to angular direction φ
        ]
    }
}

#[test]
fn m2_rim_implicit_diff_matches_fd() {
    let r = RADIUS;
    let t = THICK;
    let mut max_err_fd = 0.0_f64;
    let mut max_err_analytic = 0.0_f64;

    for k in 0..16 {
        let phi = std::f64::consts::TAU * (k as f64 + 0.19) / 16.0;
        let sys = TopRim { t, phi };
        // Primal rim point.
        let x = [r * phi.cos(), r * phi.sin(), t];
        let dxdr = implicit_sensitivity(&sys, x, r).expect("F_x nonsingular");

        // Analytic truth: dx/dr = (cosφ, sinφ, 0).
        let truth = [phi.cos(), phi.sin(), 0.0];
        // FD: re-solve the pinned rim point at r±h.
        let solve = |rr: f64| [rr * phi.cos(), rr * phi.sin(), t];
        let xp = solve(r + H);
        let xm = solve(r - H);
        for c in 0..3 {
            let fd = (xp[c] - xm[c]) / (2.0 * H);
            max_err_fd = max_err_fd.max((dxdr[c] - fd).abs());
            max_err_analytic = max_err_analytic.max((dxdr[c] - truth[c]).abs());
        }
    }
    eprintln!("M2 rim implicit-diff: max |analytic-FD| = {max_err_fd:e}, max |analytic-truth| = {max_err_analytic:e}");
    assert!(
        max_err_fd <= GATE,
        "rim implicit-vs-FD err {max_err_fd} > gate"
    );
    assert!(max_err_analytic <= 1e-9);
}

#[test]
fn m2_rim_lift_bridge_agrees_with_implicit_diff() {
    // The rim nodes are stored on the cylinder surface; the lift-bridge gives
    // their dx/dr directly. It must agree with the Pillar-3 implicit result.
    let model = BlockWithHole::new(HALF, THICK, RADIUS, 64);
    let tess = model.tessellation();
    let store = model.build(RADIUS);
    let dual = tess.positions_dual(&store).unwrap();

    // Inner-top ring occupies node indices [3N, 4N).
    let n = 64usize;
    for i in 0..n {
        let node = &tess.nodes[3 * n + i];
        let phi = node.u; // cylinder u == angular coordinate
        let lift = [
            dual[3 * n + i].x.dual,
            dual[3 * n + i].y.dual,
            dual[3 * n + i].z.dual,
        ];

        let sys = TopRim { t: THICK, phi };
        let x = [RADIUS * phi.cos(), RADIUS * phi.sin(), THICK];
        let imp = implicit_sensitivity(&sys, x, RADIUS).unwrap();
        for c in 0..3 {
            assert!(
                (lift[c] - imp[c]).abs() <= 1e-9,
                "rim node {i} coord {c}: lift {} vs implicit {}",
                lift[c],
                imp[c]
            );
        }
    }
}
