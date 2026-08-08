//! Validation of the coupled flow ⇄ thermal adjoint.
//!
//! This is vcad's reproduction of SU2's conjugate-heat-transfer
//! validation, and it follows their protocol rather than the usual
//! "compare to one finite difference":
//!
//! 1. A finite-difference **step sweep** establishes the reference, and
//!    is refused if it never plateaus. A reference that has not
//!    demonstrated convergence is not allowed to judge a gradient.
//! 2. The coupled adjoint is compared against it.
//! 3. Each coupling term is then **ablated** — deleted — and the error
//!    has to blow up. A passing FD comparison does not prove a term is
//!    wired in; only removing it does.
//!
//! The headline case is the inlet velocity, which reaches the hotspot
//! temperature *only* through the coupling. The uncoupled adjoint reports
//! exactly zero for it.

use vcad_kernel_adjoint::{fd_sweep, ClaimVerdict};
use vcad_kernel_flow::conjugate_adjoint::{
    ablate, conjugate_gradient, coupled_objective, with_parameter, ConjugateAdjointOptions,
    ConjugateParameter, Coupling, ParameterSpec,
};
use vcad_kernel_flow::model::{Cell, FlowModel, Fluid, ThermalTransport};
use vcad_kernel_thermal::model::{MaterialRegion, PowerSource, Shape, ThermalModel};

/// A small heated block cooled by duct flow — the same physics as the
/// primal conjugate test, sized so a finite-difference sweep over the
/// whole coupled loop is affordable.
fn setup() -> (FlowModel, ThermalModel) {
    let (nx, ny, nz) = (20usize, 5usize, 8usize);
    let size = [20.0, 5.0, 8.0];
    let mut fm = FlowModel::new([0.0; 3], size, [nx, ny, nz]);
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let x = fm.index(i, j, k);
                fm.cells[x] = if k < 3 {
                    Cell::Solid
                } else if i == 0 {
                    Cell::Inlet
                } else if i == nx - 1 {
                    Cell::Outlet
                } else {
                    Cell::Fluid
                };
            }
        }
    }
    fm.fluid = Fluid::AIR_20C;
    fm.inlet_velocity_m_s = [0.09, 0.0, 0.0];
    fm.thermal = Some(ThermalTransport::AIR_20C);

    let mut tm = ThermalModel::new([0.0; 3], size, [nx, ny, nz]);
    tm.materials.push(MaterialRegion::isotropic(
        Shape::Box {
            min_mm: [0.0, 0.0, 0.0],
            size_mm: [20.0, 5.0, 3.0],
        },
        180.0,
    ));
    tm.sources.push(PowerSource {
        name: "chip".into(),
        shape: Shape::Box {
            min_mm: [8.0, 1.0, 0.0],
            size_mm: [4.0, 3.0, 2.0],
        },
        power_w: 0.05,
    });
    tm.reference_c = Some(20.0);
    (fm, tm)
}

fn opts() -> ConjugateAdjointOptions {
    ConjugateAdjointOptions::tightened()
}

const POWER: ConjugateParameter = ConjugateParameter::SourcePower(0);
const VELOCITY: ConjugateParameter = ConjugateParameter::InletVelocity(0);

/// The interface Jacobian must be genuinely nonzero — the two solvers do
/// see each other — and subcritical, or the primal loop would not
/// converge either.
#[test]
fn the_interface_jacobian_is_live_and_subcritical() {
    let (fm, tm) = setup();
    let g = conjugate_gradient(&fm, &tm, &[ParameterSpec::new(POWER)], &opts()).unwrap();

    let a = g.interface_jacobian;
    let any_nonzero = a.iter().flatten().any(|v| v.abs() > 1e-9);
    assert!(any_nonzero, "interface Jacobian is all zeros: {a:?}");
    assert!(
        g.coupling_strength < 1.0,
        "coupling strength {} >= 1 — the primal loop should not have converged",
        g.coupling_strength
    );
    // Both halves of the film channel carry a gradient.
    assert!(
        g.d_objective_d_film[0].abs() > 0.0 && g.d_objective_d_film[1].abs() > 0.0,
        "dJ/ds = {:?} — a conjugate coupling needs both the film and the bulk temperature",
        g.d_objective_d_film
    );
    // More film cools the part; a hotter fluid heats it.
    assert!(g.d_objective_d_film[0] < 0.0, "dJ/dh should be negative");
    assert!(
        g.d_objective_d_film[1] > 0.0,
        "dJ/dT_bulk should be positive"
    );
    // Four Ψ evaluations for the 2x2 plus two for the one parameter.
    assert_eq!(g.psi_evaluations, 6);
}

/// The coupled gradient of the hotspot temperature with respect to the
/// source power, against a finite-difference sweep over the entire
/// conjugate loop.
#[test]
fn coupled_power_gradient_matches_a_converged_finite_difference() {
    let (fm, tm) = setup();
    let o = opts();
    let base = ConjugateParameter::SourcePower(0);
    let theta0 = 0.05;

    let sweep = fd_sweep(
        |p| {
            let (f, t) = with_parameter(&fm, &tm, base, p);
            coupled_objective(&f, &t, &o).expect("coupled objective")
        },
        theta0,
        &[5e-3, 2e-3, 1e-3, 5e-4],
        5e-3,
    )
    .expect("the finite differences must plateau before they may judge anything");
    println!("{}", sweep.render());

    let g = conjugate_gradient(&fm, &tm, &[ParameterSpec::new(base)], &o).unwrap();
    let adj = g.get("source_power[0]").unwrap();
    let rel = sweep.rel_error(adj);
    println!(
        "adjoint {adj:.9e}  fd {:.9e}  rel {:.3e}",
        sweep.derivative(),
        rel
    );
    assert!(
        rel < 0.05,
        "coupled dJ/dP: adjoint {adj:.9e}, fd {:.9e} (rel {rel:.3e})",
        sweep.derivative()
    );
    assert!(adj > 0.0, "more power must mean a hotter part");
}

/// The demonstration parameter: inlet velocity reaches the objective only
/// through the coupling.
#[test]
fn coupled_velocity_gradient_matches_a_converged_finite_difference() {
    let (fm, tm) = setup();
    let o = opts();
    let theta0 = 0.09;

    let sweep = fd_sweep(
        |v| {
            let (f, t) = with_parameter(&fm, &tm, VELOCITY, v);
            coupled_objective(&f, &t, &o).expect("coupled objective")
        },
        theta0,
        &[9e-3, 4.5e-3, 2e-3, 1e-3],
        5e-3,
    )
    .expect("the finite differences must plateau before they may judge anything");
    println!("{}", sweep.render());

    let g = conjugate_gradient(&fm, &tm, &[ParameterSpec::new(VELOCITY)], &o).unwrap();
    let adj = g.get("inlet_velocity.x").unwrap();
    let rel = sweep.rel_error(adj);
    println!(
        "adjoint {adj:.9e}  fd {:.9e}  rel {:.3e}",
        sweep.derivative(),
        rel
    );
    assert!(
        rel < 0.10,
        "coupled dJ/dv: adjoint {adj:.9e}, fd {:.9e} (rel {rel:.3e})",
        sweep.derivative()
    );
    assert!(
        adj < 0.0,
        "blowing harder must cool the part, got {adj:.6e}"
    );
}

/// The headline ablation. Delete the coupling and the inlet-velocity
/// gradient does not degrade — it vanishes. The uncoupled adjoint's
/// answer is "how fast you blow air over the heatsink does not affect how
/// hot it gets", stated with total confidence.
#[test]
fn dropping_the_coupling_zeroes_the_velocity_gradient() {
    let (fm, tm) = setup();
    let o = opts();

    let full = conjugate_gradient(&fm, &tm, &[ParameterSpec::new(VELOCITY)], &o).unwrap();
    let reference = full.get("inlet_velocity.x").unwrap();

    let report = ablate(
        &fm,
        &tm,
        ParameterSpec::new(VELOCITY),
        reference,
        &o,
        Coupling::None,
    )
    .unwrap();
    println!("{}", report.summary());

    assert_eq!(
        report.ablated, 0.0,
        "the uncoupled adjoint must report exactly zero here"
    );
    assert!(
        report.load_bearing(1e3),
        "the coupling carries the whole gradient: {}",
        report.summary()
    );
    assert!((report.ablated_rel_err - 1.0).abs() < 1e-9);
}

/// SU2's actual case: a parameter both solvers can see, where dropping
/// the coupling gives a plausible-looking but materially wrong number.
#[test]
fn dropping_the_coupling_materially_degrades_the_power_gradient() {
    let (fm, tm) = setup();
    let o = opts();

    let full = conjugate_gradient(&fm, &tm, &[ParameterSpec::new(POWER)], &o).unwrap();
    let reference = full.get("source_power[0]").unwrap();

    for mode in [Coupling::None, Coupling::NoFeedback] {
        let report = ablate(&fm, &tm, ParameterSpec::new(POWER), reference, &o, mode).unwrap();
        println!("{}", report.summary());
        assert!(
            report.ablated_rel_err > 0.02,
            "ablating {mode:?} should move the power gradient by more than 2%: {}",
            report.summary()
        );
    }
}

/// An ablated gradient does not merely carry a bigger error bar — it
/// comes back with an `Unverifiable` verdict and refuses to steer an
/// optimizer. That is the fail-closed half of the ledger.
#[test]
fn an_ablated_gradient_refuses_to_steer() {
    let (fm, tm) = setup();
    let mut o = opts();

    o.coupling = Coupling::Full;
    let full = conjugate_gradient(&fm, &tm, &[ParameterSpec::new(POWER)], &o).unwrap();
    assert!(full.table.all_usable(), "{}", full.table.render());
    assert_eq!(full.table.rows[0].verdict, ClaimVerdict::Pass);
    // The 2x2 block is finite-differenced, so it may never claim Verified.
    assert_eq!(
        full.table.rows[0].basis,
        vcad_kernel_adjoint::ClaimBasis::Predicted
    );

    for mode in [Coupling::None, Coupling::NoFeedback] {
        o.coupling = mode;
        let g = conjugate_gradient(&fm, &tm, &[ParameterSpec::new(POWER)], &o).unwrap();
        assert!(
            !g.table.all_usable(),
            "{mode:?} must not be usable:\n{}",
            g.table.render()
        );
        assert_eq!(g.table.rows[0].verdict, ClaimVerdict::Unverifiable);
        assert!(!g.ledger.completeness().may_optimize());
    }
}

/// The ledger is not decoration: it renders, it rolls up, and its
/// roll-up is what the sensitivity rows inherit.
#[test]
fn the_ledger_describes_what_was_actually_computed() {
    let (fm, tm) = setup();
    let g = conjugate_gradient(&fm, &tm, &[ParameterSpec::new(POWER)], &opts()).unwrap();
    let rendered = g.ledger.render();
    println!("{rendered}");
    assert!(rendered.contains("flow"));
    assert!(rendered.contains("thermal"));
    // Both cross blocks implemented, so no INCOMPLETE line.
    assert!(!rendered.contains("INCOMPLETE"));
    // But finite-differenced, so not all-exact.
    assert!(rendered.contains("finite-differenced"));
}

/// Several parameters at once: the direct terms all come out of one
/// thermal adjoint, so the Ψ budget grows by two per parameter and not
/// by the grid size.
#[test]
fn multiple_parameters_share_one_adjoint_solve() {
    let (fm, tm) = setup();
    let specs = [
        ParameterSpec::bounded(POWER, 0.0, 0.2),
        ParameterSpec::bounded(
            ConjugateParameter::Conductivity { region: 0, axis: 0 },
            10.0,
            400.0,
        ),
        ParameterSpec::bounded(VELOCITY, 0.02, 0.3),
    ];
    let g = conjugate_gradient(&fm, &tm, &specs, &opts()).unwrap();
    println!("{}", g.table.render());
    assert_eq!(g.table.len(), 3);
    // 4 for the interface block + 2 per parameter.
    assert_eq!(g.psi_evaluations, 4 + 2 * 3);

    // Physics signs.
    assert!(g.get("source_power[0]").unwrap() > 0.0);
    assert!(g.get("conductivity[0].x").unwrap() < 0.0);
    assert!(g.get("inlet_velocity.x").unwrap() < 0.0);

    // Every row carries a trust radius that contains its own value, so
    // the ranking is well defined.
    for r in &g.table.rows {
        assert!(r.trust.is_some(), "{} has no trust radius", r.parameter);
        assert!(r.in_trust(), "{} sits outside its own radius", r.parameter);
        assert!(r.influence().is_some());
    }
    let ranked = g.table.ranked_for("hotspot_c");
    assert_eq!(ranked.len(), 3);
    // Ranking is by influence, descending.
    for w in ranked.windows(2) {
        assert!(w[0].influence().unwrap() >= w[1].influence().unwrap());
    }
}
