//! Finite-difference force oracle.
//!
//! Central-difference `-dE/dx` for every coordinate, compared against the
//! analytic forces a [`ForceField`] returns. This is the correctness gate for
//! every potential term — an analytic force that disagrees with `-∇E` is a bug,
//! full stop. Modeled on the `vcad-kernel-diff` finite-difference oracle.

use crate::potential::ForceField;
use crate::system::AtomSystem;

/// Result of comparing analytic forces to finite differences.
#[derive(Debug, Clone)]
pub struct FdReport {
    /// Maximum absolute component error (eV/Å).
    pub max_abs_error: f64,
    /// Index and axis of the worst component.
    pub worst: (usize, usize),
}

/// Compute numerical forces via central differences with step `h` (Å).
pub fn numerical_forces(ff: &dyn ForceField, sys: &AtomSystem, h: f64) -> Vec<[f64; 3]> {
    let n = sys.len();
    let mut forces = vec![[0.0; 3]; n];
    let mut probe = sys.clone();
    for i in 0..n {
        for axis in 0..3 {
            let orig = sys.positions[i][axis];
            probe.positions[i][axis] = orig + h;
            let e_plus = ff.energy(&probe);
            probe.positions[i][axis] = orig - h;
            let e_minus = ff.energy(&probe);
            probe.positions[i][axis] = orig;
            // F = -dE/dx
            forces[i][axis] = -(e_plus - e_minus) / (2.0 * h);
        }
    }
    forces
}

/// Compare a force field's analytic forces to central differences.
pub fn check_forces(ff: &dyn ForceField, sys: &AtomSystem, h: f64) -> FdReport {
    let (_, analytic) = ff.energy_forces(sys);
    let numeric = numerical_forces(ff, sys, h);
    let mut max_abs_error = 0.0;
    let mut worst = (0, 0);
    for i in 0..sys.len() {
        for axis in 0..3 {
            let err = (analytic[i][axis] - numeric[i][axis]).abs();
            if err > max_abs_error {
                max_abs_error = err;
                worst = (i, axis);
            }
        }
    }
    FdReport {
        max_abs_error,
        worst,
    }
}
