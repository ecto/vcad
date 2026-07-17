//! The M2 validation ladder: adjoint impedance gradients against finite
//! differences (frozen segmentation), and the gradient actually designing
//! an antenna (Newton onto resonance).

use vcad_kernel_antenna::adjoint::{perturbed_mesh, z_in_gradient, ParamVelocity};
use vcad_kernel_antenna::{find_resonance, solve_driven, Mesh, SolveOptions, WireGrid};

const OPTS: SolveOptions = SolveOptions {
    quad_outer: 6,
    quad_inner: 6,
};

fn dipole_mesh(len_mm: f64, nseg: usize) -> Mesh {
    let mut g = WireGrid::new();
    g.add_wire(
        [0.0, 0.0, -len_mm / 2.0],
        [0.0, 0.0, len_mm / 2.0],
        1.0,
        nseg,
    )
    .unwrap();
    Mesh::build(&g).unwrap()
}

/// An axial stretch: every node moves in proportion to its z, so unit
/// parameter = unit relative elongation × (ℓ/2) per arm — i.e. p measures
/// total length in meters when v = z/ℓ · 2 ... we use v = z (pure strain):
/// dZ/dp is then Ω per unit strain.
fn stretch(mesh: &Mesh) -> ParamVelocity {
    ParamVelocity::from_fn(mesh, |p| [0.0, 0.0, p[2]])
}

/// The adjoint-contracted gradient must match a central finite difference
/// of the full solve — computed on meshes with FROZEN segmentation (same
/// topology, nodes moved smoothly). Re-meshing between probes would make
/// the discretization a hidden parameter; `perturbed_mesh` exists so that
/// cannot happen.
#[test]
fn adjoint_gradient_matches_frozen_segmentation_fd() {
    let f = 140.0e6; // off resonance: both R and X gradients well-scaled
    let mesh = dipole_mesh(1000.0, 24);
    let feed = mesh.nearest_basis([0.0, 0.0, 0.0]).unwrap();
    let vel = stretch(&mesh);

    let res = z_in_gradient(&mesh, feed, f, std::slice::from_ref(&vel), &OPTS).unwrap();
    let adj = res.dz_dp[0];

    // Central FD through the entire solve, same frozen meshes.
    let h = 1e-5; // strain
    let zp = {
        let m = perturbed_mesh(&mesh, &vel, h);
        solve_driven(&m, feed, f, &OPTS).unwrap().z_in
    };
    let zm = {
        let m = perturbed_mesh(&mesh, &vel, -h);
        solve_driven(&m, feed, f, &OPTS).unwrap().z_in
    };
    let fd = (zp - zm).scale(0.5 / h);

    let rel = (adj - fd).abs() / fd.abs();
    assert!(
        rel < 1e-4,
        "adjoint {adj:?} vs frozen-FD {fd:?} Ω/strain (rel {rel:.2e})"
    );

    // Physics: stretching a below-resonance dipole drives it inductive
    // (dX/dp > 0) and raises the radiation resistance (dR/dp > 0).
    assert!(adj.im > 0.0 && adj.re > 0.0, "signs: {adj:?}");
}

/// Same identity through a junction + ground plane: the top-hat monopole.
/// The gradient machinery must survive image sources and KCL bases.
#[test]
fn adjoint_gradient_matches_fd_with_ground_and_junction() {
    let f = 80.0e6;
    let mut g = WireGrid::new();
    g.set_ground_plane(true);
    g.add_wire([0.0, 0.0, 0.0], [0.0, 0.0, 500.0], 1.0, 10)
        .unwrap();
    g.add_wire([0.0, 0.0, 500.0], [250.0, 0.0, 500.0], 1.0, 5)
        .unwrap();
    g.add_wire([0.0, 0.0, 500.0], [-250.0, 0.0, 500.0], 1.0, 5)
        .unwrap();
    let mesh = Mesh::build(&g).unwrap();
    let feed = mesh.nearest_basis([0.0, 0.0, 0.0]).unwrap();
    // Stretch the hat arms only (x-strain, keeps grounded node fixed).
    let vel = ParamVelocity::from_fn(&mesh, |p| [p[0], 0.0, 0.0]);

    let res = z_in_gradient(&mesh, feed, f, std::slice::from_ref(&vel), &OPTS).unwrap();
    let adj = res.dz_dp[0];
    let h = 1e-5;
    let zp = solve_driven(&perturbed_mesh(&mesh, &vel, h), feed, f, &OPTS)
        .unwrap()
        .z_in;
    let zm = solve_driven(&perturbed_mesh(&mesh, &vel, -h), feed, f, &OPTS)
        .unwrap()
        .z_in;
    let fd = (zp - zm).scale(0.5 / h);
    let rel = (adj - fd).abs() / fd.abs();
    assert!(
        rel < 1e-4,
        "hatted-monopole adjoint {adj:?} vs FD {fd:?} (rel {rel:.2e})"
    );
    // Longer hat → more capacitive loading → the antenna is electrically
    // longer, so below resonance X climbs toward zero: dX/dp > 0. With
    // Foster's dX/df > 0 near a series resonance, that is precisely
    // df_res/dp = −(∂X/∂p)/(∂X/∂f) < 0 — hat growth pulls the resonant
    // frequency down, which is why short verticals wear hats.
    assert!(
        adj.im > 0.0,
        "hat growth must load the mast down in f: {adj:?}"
    );
}

/// The gradient designs the antenna: Newton on the arm strain drives a
/// 10%-detuned dipole onto resonance (Im Z = 0) in a few steps, landing
/// on the same length the bisection search finds — but each Newton step
/// costs one solve + two fills instead of a frequency sweep.
#[test]
fn newton_with_adjoint_gradient_tunes_dipole_to_resonance() {
    let f_target = 143.6127e6; // design frequency
    let mut len_mm = 900.0; // start 10% short → capacitive
    let nseg = 24;
    let mut steps = 0;
    let mut x_now = f64::INFINITY;
    for _ in 0..8 {
        let mesh = dipole_mesh(len_mm, nseg);
        let feed = mesh.nearest_basis([0.0, 0.0, 0.0]).unwrap();
        let vel = stretch(&mesh);
        let res = z_in_gradient(&mesh, feed, f_target, &[vel], &OPTS).unwrap();
        x_now = res.solution.z_in.im;
        steps += 1;
        if x_now.abs() < 1e-3 {
            break;
        }
        // Newton in strain: X(p + dp) ≈ X + dX/dp · dp = 0.
        let dp = -x_now / res.dz_dp[0].im;
        len_mm *= 1.0 + dp;
    }
    assert!(
        x_now.abs() < 1e-3,
        "Newton did not reach resonance: X = {x_now:.4e} after {steps} steps"
    );
    assert!(
        steps <= 5,
        "expected quadratic convergence, took {steps} steps"
    );

    // Independent check: bisection on frequency for the tuned length must
    // put the resonance at (essentially) the design frequency.
    let mesh = dipole_mesh(len_mm, nseg);
    let feed = mesh.nearest_basis([0.0, 0.0, 0.0]).unwrap();
    let f_res = find_resonance(&mesh, feed, 0.9 * f_target, 1.1 * f_target, &OPTS).unwrap();
    let rel = (f_res - f_target).abs() / f_target;
    assert!(
        rel < 1e-4,
        "tuned dipole resonates at {:.4} MHz vs target {:.4} MHz",
        f_res / 1e6,
        f_target / 1e6
    );
}
