//! Energy minimization via FIRE (Fast Inertial Relaxation Engine).
//!
//! FIRE is the workhorse for atomic relaxation: it's a damped-MD scheme that
//! is robust, gradient-only, and converges quickly to a local energy minimum.
//! Convergence is on the max force component (eV/Å). The FIRE numerics
//! delegate to [`phyz_md::field::fire`]; this module binds them to
//! [`AtomSystem`] and a [`ForceField`].

use crate::potential::ForceField;
use crate::system::AtomSystem;
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

/// Relax `sys` in place to a local energy minimum using FIRE. Velocities are
/// used as the internal FIRE state and left near zero on return.
///
/// Implemented as an unbounded [`MinimizeRun`] drive, so the one-shot and
/// chunked paths are identical by construction; the parity test in this
/// module pins the stepwise loop against `phyz_md::field::fire` itself.
pub fn minimize(
    ff: &dyn ForceField,
    sys: &mut AtomSystem,
    opts: &MinimizeOptions,
) -> MinimizeResult {
    let mut run = MinimizeRun::new(ff, sys, opts);
    run.advance(ff, sys, usize::MAX);
    run.finish(sys)
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

/// Stepwise FIRE relaxation: [`MinimizeRun::new`] performs the initial
/// force evaluation, each [`MinimizeRun::advance`] runs up to a budget of
/// FIRE iterations, and [`MinimizeRun::finish`] restores the relaxed
/// positions onto the system and returns the [`MinimizeResult`]. All FIRE
/// state (timestep, mixing, velocity memory) lives in the run, so a
/// chunked drive is bit-identical to a single [`minimize`] call — the
/// iteration body mirrors [`phyz_md::field::fire::fire`] operation for
/// operation, and the `stepwise_fire_matches_phyz_fire` test pins it.
pub struct MinimizeRun {
    opts: MinimizeOptions,
    positions: Vec<[f64; 3]>,
    velocities: Vec<[f64; 3]>,
    masses: Vec<f64>,
    forces: Vec<[f64; 3]>,
    energy: f64,
    max_force: f64,
    dt: f64,
    alpha: f64,
    steps_since_neg: usize,
    iters: usize,
    converged: bool,
}

// Standard FIRE parameters (identical to phyz-md's).
const N_MIN: usize = 5;
const F_INC: f64 = 1.1;
const F_DEC: f64 = 0.5;
const ALPHA_START: f64 = 0.1;
const F_ALPHA: f64 = 0.99;

impl MinimizeRun {
    /// Take the system's positions/velocities and perform the initial
    /// force evaluation. `sys.positions` is left empty until
    /// [`Self::finish`] restores it (the force field re-materializes
    /// positions on `sys` per evaluation, exactly as [`minimize`] does).
    pub fn new(ff: &dyn ForceField, sys: &mut AtomSystem, opts: &MinimizeOptions) -> Self {
        let masses = sys.masses.clone();
        let positions = std::mem::take(&mut sys.positions);
        let mut velocities = std::mem::take(&mut sys.velocities);
        velocities.fill([0.0; 3]);
        let (energy, forces) = Self::eval(ff, sys, &positions);
        let max_force = max_force_component(&forces);
        let converged = max_force <= opts.force_tol;
        MinimizeRun {
            opts: *opts,
            positions,
            velocities,
            masses,
            forces,
            energy,
            max_force,
            dt: opts.dt,
            alpha: ALPHA_START,
            steps_since_neg: 0,
            iters: 0,
            converged,
        }
    }

    fn eval(ff: &dyn ForceField, sys: &mut AtomSystem, pos: &[[f64; 3]]) -> (f64, Vec<[f64; 3]>) {
        sys.positions.clear();
        sys.positions.extend_from_slice(pos);
        ff.energy_forces(sys)
    }

    /// Whether the run is finished (converged or out of iterations).
    pub fn done(&self) -> bool {
        self.converged || self.iters >= self.opts.max_iters
    }

    /// FIRE iterations performed so far.
    pub fn iters(&self) -> usize {
        self.iters
    }

    /// The iteration budget.
    pub fn max_iters(&self) -> usize {
        self.opts.max_iters
    }

    /// Current potential energy (eV).
    pub fn energy(&self) -> f64 {
        self.energy
    }

    /// Current max force component (eV/Å).
    pub fn max_force(&self) -> f64 {
        self.max_force
    }

    /// Run up to `budget` FIRE iterations (min 1). Returns `true` when
    /// the run is finished.
    pub fn advance(&mut self, ff: &dyn ForceField, sys: &mut AtomSystem, budget: usize) -> bool {
        let n = self.positions.len();
        let mut left = budget.max(1);
        while !self.done() && left > 0 {
            self.iters += 1;
            left -= 1;
            // MD half-kick + drift + kick with the current forces
            // (velocity-Verlet).
            for i in 0..n {
                let inv_m = phyz_md::field::units::FORCE_TO_ACCEL / self.masses[i];
                let a = vec3::scale(self.forces[i], inv_m);
                vec3::add_assign(&mut self.velocities[i], vec3::scale(a, 0.5 * self.dt));
                let dx = vec3::scale(self.velocities[i], self.dt);
                vec3::add_assign(&mut self.positions[i], dx);
            }
            let (e_new, f_new) = Self::eval(ff, sys, &self.positions);
            self.energy = e_new;
            self.forces = f_new;
            for i in 0..n {
                let inv_m = phyz_md::field::units::FORCE_TO_ACCEL / self.masses[i];
                let a = vec3::scale(self.forces[i], inv_m);
                vec3::add_assign(&mut self.velocities[i], vec3::scale(a, 0.5 * self.dt));
            }

            // FIRE: P = F · v
            let mut power = 0.0;
            let mut vnorm2 = 0.0;
            let mut fnorm2 = 0.0;
            for i in 0..n {
                power += vec3::dot(self.forces[i], self.velocities[i]);
                vnorm2 += vec3::norm2(self.velocities[i]);
                fnorm2 += vec3::norm2(self.forces[i]);
            }
            let vnorm = vnorm2.sqrt();
            let fnorm = fnorm2.sqrt().max(1e-30);
            // v = (1-alpha) v + alpha |v| fhat
            for i in 0..n {
                let fhat = vec3::scale(self.forces[i], 1.0 / fnorm);
                self.velocities[i] = vec3::add(
                    vec3::scale(self.velocities[i], 1.0 - self.alpha),
                    vec3::scale(fhat, self.alpha * vnorm),
                );
            }
            if power > 0.0 {
                self.steps_since_neg += 1;
                if self.steps_since_neg > N_MIN {
                    self.dt = (self.dt * F_INC).min(self.opts.dt_max);
                    self.alpha *= F_ALPHA;
                }
            } else {
                self.steps_since_neg = 0;
                self.dt *= F_DEC;
                self.alpha = ALPHA_START;
                self.velocities.fill([0.0; 3]);
            }

            self.max_force = max_force_component(&self.forces);
            if self.max_force <= self.opts.force_tol {
                self.converged = true;
            }
        }
        self.done()
    }

    /// Restore positions/velocities onto `sys` and return the result.
    pub fn finish(self, sys: &mut AtomSystem) -> MinimizeResult {
        sys.positions = self.positions;
        sys.velocities = self.velocities;
        MinimizeResult {
            converged: self.converged,
            iters: if self.converged {
                self.iters
            } else {
                self.opts.max_iters
            },
            energy: self.energy,
            max_force: self.max_force,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::potential::LennardJones;

    fn dimer() -> AtomSystem {
        AtomSystem {
            elements: vec!["Ar".into(), "Ar".into()],
            numbers: vec![18, 18],
            masses: vec![39.948, 39.948],
            charges: vec![0.0, 0.0],
            positions: vec![[0.0; 3], [3.9, 0.0, 0.0]],
            velocities: vec![[0.0; 3]; 2],
            bonds: Vec::new(),
            cell: None,
        }
    }

    fn lj() -> LennardJones {
        LennardJones::monatomic(0.0103, 3.4, 12.0)
    }

    /// The stepwise loop must be bit-identical to phyz-md's `fire` — phyz
    /// is the oracle for the mirrored iteration body.
    #[test]
    fn stepwise_fire_matches_phyz_fire() {
        use phyz_md::field::fire::{fire, FireOptions};
        let opts = MinimizeOptions::default();
        let ff = lj();

        // Oracle: drive phyz fire exactly as the old one-shot did.
        let mut oracle_sys = dimer();
        let masses = oracle_sys.masses.clone();
        let mut positions = std::mem::take(&mut oracle_sys.positions);
        let mut velocities = std::mem::take(&mut oracle_sys.velocities);
        let fire_opts = FireOptions {
            max_iters: opts.max_iters,
            force_tol: opts.force_tol,
            dt: opts.dt,
            dt_max: opts.dt_max,
        };
        let oracle = fire(
            &mut positions,
            &mut velocities,
            &masses,
            &fire_opts,
            |pos| {
                oracle_sys.positions.clear();
                oracle_sys.positions.extend_from_slice(pos);
                ff.energy_forces(&oracle_sys)
            },
        );

        // Chunked drive with a tiny odd budget.
        let mut sys = dimer();
        let mut run = MinimizeRun::new(&ff, &mut sys, &opts);
        let mut calls = 0;
        while !run.advance(&ff, &mut sys, 3) {
            calls += 1;
        }
        assert!(calls > 2, "budget too generous to exercise chunking");
        let res = run.finish(&mut sys);

        assert_eq!(res.converged, oracle.converged);
        assert_eq!(res.iters, oracle.iters);
        assert_eq!(res.energy.to_bits(), oracle.energy.to_bits());
        assert_eq!(res.max_force.to_bits(), oracle.max_force.to_bits());
        for (a, b) in sys.positions.iter().zip(&positions) {
            for c in 0..3 {
                assert_eq!(a[c].to_bits(), b[c].to_bits());
            }
        }
    }

    /// One-shot `minimize` rides the stepper; sanity-check it converges.
    #[test]
    fn one_shot_minimize_converges_on_the_dimer() {
        let mut sys = dimer();
        let res = minimize(&lj(), &mut sys, &MinimizeOptions::default());
        assert!(res.converged);
        let r = (sys.positions[1][0] - sys.positions[0][0]).abs();
        let r_min = 2.0f64.powf(1.0 / 6.0) * 3.4;
        assert!((r - r_min).abs() < 1e-2, "r = {r}");
    }
}
