//! Time integration: velocity-Verlet with an optional Berendsen thermostat.
//!
//! Positions are in Å, velocities in Å/fs, forces in eV/Å, masses in amu; the
//! [`crate::units::FORCE_TO_ACCEL`] factor keeps `a = f·factor/m` consistent so
//! energy is conserved in NVE.

use crate::potential::ForceField;
use crate::system::AtomSystem;
use crate::units::{FORCE_TO_ACCEL, KB_EV_PER_K};
use crate::vec3;

/// Thermostat configuration for constant-temperature (NVT) runs.
#[derive(Debug, Clone, Copy)]
pub struct Thermostat {
    /// Target temperature in K.
    pub target_k: f64,
    /// Berendsen coupling time constant in fs (larger = weaker coupling).
    pub tau_fs: f64,
}

/// Integrator holding the current force cache (so each step does one force
/// evaluation, as velocity-Verlet requires).
pub struct Integrator<'a> {
    ff: &'a dyn ForceField,
    /// Timestep in fs.
    pub dt: f64,
    /// Optional Berendsen thermostat (None = NVE).
    pub thermostat: Option<Thermostat>,
    forces: Vec<[f64; 3]>,
    /// Last computed potential energy (eV).
    pub potential: f64,
}

impl<'a> Integrator<'a> {
    /// Create an integrator and evaluate the initial forces.
    pub fn new(ff: &'a dyn ForceField, sys: &AtomSystem, dt: f64) -> Self {
        let (potential, forces) = ff.energy_forces(sys);
        Self {
            ff,
            dt,
            thermostat: None,
            forces,
            potential,
        }
    }

    /// Enable a Berendsen thermostat.
    pub fn with_thermostat(mut self, t: Thermostat) -> Self {
        self.thermostat = Some(t);
        self
    }

    /// Advance the system by one velocity-Verlet step.
    pub fn step(&mut self, sys: &mut AtomSystem) {
        let n = sys.len();
        let dt = self.dt;
        // v(t+dt/2) = v(t) + 0.5 dt a(t); x(t+dt) = x(t) + dt v(t+dt/2)
        for i in 0..n {
            let inv_m = FORCE_TO_ACCEL / self.masses(sys, i);
            let a = vec3::scale(self.forces[i], inv_m);
            vec3::add_assign(&mut sys.velocities[i], vec3::scale(a, 0.5 * dt));
            let dx = vec3::scale(sys.velocities[i], dt);
            vec3::add_assign(&mut sys.positions[i], dx);
        }
        // Recompute forces at new positions.
        let (pot, forces) = self.ff.energy_forces(sys);
        self.forces = forces;
        self.potential = pot;
        // v(t+dt) = v(t+dt/2) + 0.5 dt a(t+dt)
        for i in 0..n {
            let inv_m = FORCE_TO_ACCEL / self.masses(sys, i);
            let a = vec3::scale(self.forces[i], inv_m);
            vec3::add_assign(&mut sys.velocities[i], vec3::scale(a, 0.5 * dt));
        }
        // Berendsen velocity rescale toward target temperature.
        if let Some(t) = self.thermostat {
            self.apply_berendsen(sys, t);
        }
    }

    #[inline]
    fn masses(&self, sys: &AtomSystem, i: usize) -> f64 {
        sys.masses[i]
    }

    fn apply_berendsen(&self, sys: &mut AtomSystem, t: Thermostat) {
        let n = sys.len();
        if n == 0 {
            return;
        }
        let dof = 3.0 * n as f64;
        let ke = sys.kinetic_energy();
        let cur_t = 2.0 * ke / (dof * KB_EV_PER_K);
        if cur_t <= 1e-12 {
            return;
        }
        // lambda = sqrt(1 + dt/tau (T0/T - 1))
        let ratio = t.target_k / cur_t;
        let lambda2 = 1.0 + (self.dt / t.tau_fs) * (ratio - 1.0);
        let lambda = lambda2.max(0.0).sqrt();
        for v in &mut sys.velocities {
            *v = vec3::scale(*v, lambda);
        }
    }

    /// Total energy (potential + kinetic) in eV given the current system.
    pub fn total_energy(&self, sys: &AtomSystem) -> f64 {
        self.potential + sys.kinetic_energy()
    }
}
