//! Energy minimization via FIRE (Fast Inertial Relaxation Engine).
//!
//! FIRE is the workhorse for atomic relaxation: it's a damped-MD scheme that
//! is robust, gradient-only, and converges quickly to a local energy minimum.
//! Convergence is on the max force component (eV/Å). The FIRE numerics
//! delegate to [`phyz_md::field::fire()`]; this module binds them to
//! [`AtomSystem`] and a [`ForceField`].

use crate::potential::ForceField;
use crate::system::AtomSystem;
use phyz_md::field::fire::{fire, FireOptions};

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

/// Relax `sys` in place to a local energy minimum using FIRE. Velocities are
/// used as the internal FIRE state and left near zero on return.
pub fn minimize(
    ff: &dyn ForceField,
    sys: &mut AtomSystem,
    opts: &MinimizeOptions,
) -> MinimizeResult {
    let fire_opts = FireOptions {
        max_iters: opts.max_iters,
        force_tol: opts.force_tol,
        dt: opts.dt,
        dt_max: opts.dt_max,
    };
    // Move positions/velocities into locals so phyz-md's FIRE can drive them
    // while the force closure re-materializes positions on `sys` for the
    // AtomSystem-based ForceField.
    let masses = sys.masses.clone();
    let mut positions = std::mem::take(&mut sys.positions);
    let mut velocities = std::mem::take(&mut sys.velocities);
    let res = fire(
        &mut positions,
        &mut velocities,
        &masses,
        &fire_opts,
        |pos| {
            sys.positions.clear();
            sys.positions.extend_from_slice(pos);
            ff.energy_forces(sys)
        },
    );
    sys.positions = positions;
    sys.velocities = velocities;
    MinimizeResult {
        converged: res.converged,
        iters: res.iters,
        energy: res.energy,
        max_force: res.max_force,
    }
}
