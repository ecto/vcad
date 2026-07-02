//! Energy minimization via FIRE (Fast Inertial Relaxation Engine).
//!
//! FIRE is the workhorse for atomic relaxation: it's a damped-MD scheme that
//! is robust, gradient-only, and converges quickly to a local energy minimum.
//! Convergence is on the max force component (eV/Å).

use crate::potential::ForceField;
use crate::system::AtomSystem;
use crate::units::FORCE_TO_ACCEL;
use crate::vec3;

/// Options controlling a FIRE relaxation.
#[derive(Debug, Clone, Copy)]
pub struct MinimizeOptions {
    /// Maximum iterations.
    pub max_iters: usize,
    /// Force convergence tolerance (eV/Å) on the max component.
    pub force_tol: f64,
    /// Initial timestep (fs).
    pub dt: f64,
    /// Maximum timestep (fs).
    pub dt_max: f64,
}

impl Default for MinimizeOptions {
    fn default() -> Self {
        Self {
            max_iters: 2000,
            force_tol: 1e-4,
            dt: 1.0,
            dt_max: 10.0,
        }
    }
}

/// Outcome of a minimization.
#[derive(Debug, Clone)]
pub struct MinimizeResult {
    /// Whether the force tolerance was reached.
    pub converged: bool,
    /// Iterations performed.
    pub iters: usize,
    /// Final potential energy (eV).
    pub energy: f64,
    /// Final max force component (eV/Å).
    pub max_force: f64,
}

fn max_force_component(forces: &[[f64; 3]]) -> f64 {
    let mut m = 0.0;
    for f in forces {
        for &c in f {
            m = f64::max(m, c.abs());
        }
    }
    m
}

/// Relax `sys` in place to a local energy minimum using FIRE. Velocities are
/// used as the internal FIRE state and left near zero on return.
pub fn minimize(
    ff: &dyn ForceField,
    sys: &mut AtomSystem,
    opts: &MinimizeOptions,
) -> MinimizeResult {
    // Standard FIRE parameters.
    const N_MIN: usize = 5;
    const F_INC: f64 = 1.1;
    const F_DEC: f64 = 0.5;
    const ALPHA_START: f64 = 0.1;
    const F_ALPHA: f64 = 0.99;

    let n = sys.len();
    sys.velocities.fill([0.0; 3]);
    let (mut energy, mut forces) = ff.energy_forces(sys);
    let mut dt = opts.dt;
    let mut alpha = ALPHA_START;
    let mut steps_since_neg = 0usize;

    let mut max_f = max_force_component(&forces);
    if max_f <= opts.force_tol {
        return MinimizeResult {
            converged: true,
            iters: 0,
            energy,
            max_force: max_f,
        };
    }

    for iter in 1..=opts.max_iters {
        // MD half-kick + drift + kick with the current forces (velocity-Verlet).
        for i in 0..n {
            let inv_m = FORCE_TO_ACCEL / sys.masses[i];
            let a = vec3::scale(forces[i], inv_m);
            vec3::add_assign(&mut sys.velocities[i], vec3::scale(a, 0.5 * dt));
            let dx = vec3::scale(sys.velocities[i], dt);
            vec3::add_assign(&mut sys.positions[i], dx);
        }
        let (e_new, f_new) = ff.energy_forces(sys);
        energy = e_new;
        forces = f_new;
        for i in 0..n {
            let inv_m = FORCE_TO_ACCEL / sys.masses[i];
            let a = vec3::scale(forces[i], inv_m);
            vec3::add_assign(&mut sys.velocities[i], vec3::scale(a, 0.5 * dt));
        }

        // FIRE: P = F · v
        let mut power = 0.0;
        let mut vnorm2 = 0.0;
        let mut fnorm2 = 0.0;
        for i in 0..n {
            power += vec3::dot(forces[i], sys.velocities[i]);
            vnorm2 += vec3::norm2(sys.velocities[i]);
            fnorm2 += vec3::norm2(forces[i]);
        }
        let vnorm = vnorm2.sqrt();
        let fnorm = fnorm2.sqrt().max(1e-30);
        // v = (1-alpha) v + alpha |v| fhat
        for i in 0..n {
            let fhat = vec3::scale(forces[i], 1.0 / fnorm);
            sys.velocities[i] = vec3::add(
                vec3::scale(sys.velocities[i], 1.0 - alpha),
                vec3::scale(fhat, alpha * vnorm),
            );
        }
        if power > 0.0 {
            steps_since_neg += 1;
            if steps_since_neg > N_MIN {
                dt = (dt * F_INC).min(opts.dt_max);
                alpha *= F_ALPHA;
            }
        } else {
            steps_since_neg = 0;
            dt *= F_DEC;
            alpha = ALPHA_START;
            sys.velocities.fill([0.0; 3]);
        }

        max_f = max_force_component(&forces);
        if max_f <= opts.force_tol {
            return MinimizeResult {
                converged: true,
                iters: iter,
                energy,
                max_force: max_f,
            };
        }
    }

    MinimizeResult {
        converged: false,
        iters: opts.max_iters,
        energy,
        max_force: max_f,
    }
}
