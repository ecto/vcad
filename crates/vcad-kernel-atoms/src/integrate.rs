//! Time integration: velocity-Verlet with an optional Berendsen thermostat.
//!
//! Positions are in Å, velocities in Å/fs, forces in eV/Å, masses in amu; the
//! [`crate::units::FORCE_TO_ACCEL`] factor keeps `a = f·factor/m` consistent so
//! energy is conserved in NVE. The step numerics delegate to
//! [`phyz_md::field::verlet`]; this type binds them to [`AtomSystem`] and a
//! [`ForceField`].

use crate::potential::ForceField;
use crate::system::AtomSystem;
use phyz_md::field::verlet::{verlet_step, Berendsen};

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
        let ff = self.ff;
        let thermostat = self.thermostat.map(|t| Berendsen {
            target_k: t.target_k,
            tau_fs: t.tau_fs,
        });
        // Move positions/velocities into locals so phyz-md can integrate them
        // while the force closure re-materializes positions on `sys` for the
        // AtomSystem-based ForceField. (Force fields see the system with the
        // trial positions; velocities are restored after the step.)
        let masses = sys.masses.clone();
        let mut positions = std::mem::take(&mut sys.positions);
        let mut velocities = std::mem::take(&mut sys.velocities);
        verlet_step(
            self.dt,
            thermostat,
            &mut positions,
            &mut velocities,
            &masses,
            &mut self.forces,
            &mut self.potential,
            |pos| {
                sys.positions.clear();
                sys.positions.extend_from_slice(pos);
                ff.energy_forces(sys)
            },
        );
        sys.positions = positions;
        sys.velocities = velocities;
    }

    /// Total energy (potential + kinetic) in eV given the current system.
    pub fn total_energy(&self, sys: &AtomSystem) -> f64 {
        self.potential + sys.kinetic_energy()
    }
}
