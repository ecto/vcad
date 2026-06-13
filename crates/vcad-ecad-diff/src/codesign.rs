//! Differentiable simulation + plant/controller co-design (1-DOF motor).
//!
//! The "play it forward / differentiate backward" idea, made real end to end: a
//! closed-loop motor spin-up is rolled out step by step over `tang::Scalar`, so
//! **evaluating** it (`f64`) simulates the trajectory and **tracing** it
//! (`tang_expr::ExprId`) yields the gradient of a trajectory cost with respect
//! to BOTH a plant parameter (stator geometry → torque constant) and a
//! controller gain. Those gradients feed the same Levenberg-Marquardt engine, so
//! the motor's *body* and its *reflexes* are optimized together.
//!
//! Scope (stated honestly): a deliberately simplified 1-DOF model —
//! `J·dω/dt = Kt·i − b·ω`, explicit-Euler integration, proportional speed
//! control. The full articulated dynamics (Featherstone ABA, contact, multi-DOF,
//! adjoint backprop-through-time at scale) live in the phyz repo and are NOT
//! modeled here. This is the geometry→performance→control bridge they build on,
//! proven differentiable through the whole rollout.

use crate::design::{DesignSystem, ResidualFn};
use tang::Scalar;
use tang_expr::ExprId;
use vcad_ecad_sim::magnetics::motor_torque_constant;
use vcad_kernel_constraints::{SolveResult, SolverConfig};

// Fixed 1-DOF plant + sim config (normalized; chosen for a stable explicit-Euler
// rollout across the design box).
const INERTIA: f64 = 0.02;
const DAMPING: f64 = 0.05;
const DT: f64 = 0.01;
/// Number of integration steps in a spin-up rollout.
pub const STEPS: usize = 25;
/// Target angular velocity (rad/s) the controller tracks.
pub const TARGET: f64 = 30.0;
// Fixed magnetics: pole pairs, series turns, winding factor, airgap flux (T), bore radius (mm).
const POLE_PAIRS: f64 = 6.0;
const TURNS: f64 = 60.0;
const KW: f64 = 0.866;
const B_GAP: f64 = 0.4;
const R_IN: f64 = 5.0;

/// One explicit-Euler step of a 1-DOF motor: `J·dω/dt = Kt·i − b·ω`.
pub fn motor_step<S: Scalar>(omega: S, kt: S, i_cmd: S, j: S, b: S, dt: S) -> S {
    omega + dt * (kt * i_cmd - b * omega) / j
}

/// Closed-loop spin-up: roll the proportional-controlled 1-DOF motor forward
/// `steps` steps and return the final angular velocity. Generic over `Scalar`,
/// so the same code simulates (`f64`) and differentiates (`ExprId`).
/// `outer_r_mm` (plant geometry → Kt) and `kp` (controller gain) are the design
/// variables.
pub fn spin_up_omega<S: Scalar>(outer_r_mm: S, kp: S, steps: usize) -> S {
    // Plant: torque constant from stator geometry.
    let kt = motor_torque_constant(
        S::from_f64(POLE_PAIRS),
        S::from_f64(TURNS),
        S::from_f64(KW),
        S::from_f64(B_GAP),
        S::from_f64(R_IN),
        outer_r_mm,
    );
    let (j, b, dt, target) = (
        S::from_f64(INERTIA),
        S::from_f64(DAMPING),
        S::from_f64(DT),
        S::from_f64(TARGET),
    );
    let mut omega = S::from_f64(0.0);
    for _ in 0..steps {
        let i_cmd = kp * (target - omega); // proportional speed control
        omega = motor_step(omega, kt, i_cmd, j, b, dt);
    }
    omega
}

/// Co-optimize `[outer_radius_mm, kp]` so the spin-up tracks the target speed
/// across its back half while penalizing aggressive control. Gradients flow
/// through the entire traced rollout to both the plant and the controller.
/// Returns the solved design and the LM result.
pub fn codesign_motor(seed: [f64; 2]) -> ([f64; 2], SolveResult) {
    let effort_w = 0.01;
    let residuals: Vec<ResidualFn> = vec![
        // Track the target across the settling half of the trajectory.
        Box::new(|v| spin_up_omega(v[0], v[1], STEPS / 2) - ExprId::from_f64(TARGET)),
        Box::new(|v| spin_up_omega(v[0], v[1], (STEPS * 3) / 4) - ExprId::from_f64(TARGET)),
        Box::new(|v| spin_up_omega(v[0], v[1], STEPS) - ExprId::from_f64(TARGET)),
        // Penalize control effort (the initial current spike ∝ kp·target).
        Box::new(move |v| v[1] * ExprId::from_f64(TARGET * effort_w)),
    ];
    let sys = DesignSystem::build(&residuals, 2).with_bounds(vec![15.0, 1.0], vec![45.0, 50.0]);
    let mut params = seed.to_vec();
    let res = sys.solve(&mut params, &SolverConfig::default());
    ([params[0], params[1]], res)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tang_expr::trace;

    #[test]
    fn rollout_gradient_matches_finite_difference() {
        // d(final ω)/d(outer_radius), symbolic through the 25-step closed-loop
        // rollout vs central finite difference. var(0)=radius, var(1)=kp.
        let (r0, kp0) = (25.0_f64, 8.0_f64);
        let (mut g, expr) = trace(|| spin_up_omega(ExprId::var(0), ExprId::var(1), STEPS));

        let direct = spin_up_omega(r0, kp0, STEPS);
        assert!((g.eval(expr, &[r0, kp0]) - direct).abs() < 1e-9 * (1.0 + direct.abs()));

        let dexpr = g.diff(expr, 0);
        let grad = g.eval(dexpr, &[r0, kp0]);
        let eps = 1e-4;
        let fd = (spin_up_omega(r0 + eps, kp0, STEPS) - spin_up_omega(r0 - eps, kp0, STEPS))
            / (2.0 * eps);
        assert!(
            (grad - fd).abs() < 1e-5 * (1.0 + fd.abs()),
            "dω/dr {grad} vs fd {fd}"
        );
        assert!(grad > 0.0, "a bigger stator (more torque) spins up faster");
    }

    #[test]
    fn co_design_tracks_target_better_than_the_seed() {
        let seed = [20.0_f64, 5.0_f64];
        let omega_seed = spin_up_omega(seed[0], seed[1], STEPS);
        let (sol, _res) = codesign_motor(seed);
        let omega_sol = spin_up_omega(sol[0], sol[1], STEPS);

        // Jointly tuning geometry + gain tracks the target far better than the seed.
        assert!(
            (omega_sol - TARGET).abs() < (omega_seed - TARGET).abs(),
            "co-design should improve tracking: seed ω={omega_seed}, solved ω={omega_sol}"
        );
        assert!(
            (omega_sol - TARGET).abs() < 0.25 * TARGET,
            "tracks within 25%: ω={omega_sol}"
        );
        // Solution respects the design box, and the plant param actually moved.
        assert!((15.0..=45.0).contains(&sol[0]) && (1.0..=50.0).contains(&sol[1]));
        assert!(
            sol[0] > seed[0],
            "co-design grew the stator for more torque: {}",
            sol[0]
        );
    }
}
