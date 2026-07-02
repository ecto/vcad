//! Gym-style molecular-dynamics environment: `reset` / `step` / `observe`,
//! mirroring the physics crate's `RobotEnv` so the MCP surface and any RL/
//! optimization loops share the same verbs across domains.

use crate::integrate::{Integrator, Thermostat};
use crate::potential::ForceField;
use crate::system::AtomSystem;
use vcad_ir::molecule::MoleculeSystem;

/// Observation returned by an [`MdEnv`] each step.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MdObservation {
    /// Steps taken since reset.
    pub step: usize,
    /// Potential energy (eV).
    pub potential_energy: f64,
    /// Kinetic energy (eV).
    pub kinetic_energy: f64,
    /// Total energy (eV).
    pub total_energy: f64,
    /// Instantaneous temperature (K).
    pub temperature: f64,
    /// Max force component (eV/Å).
    pub max_force: f64,
}

/// A molecular-dynamics environment owning its system, force field, and
/// integrator settings.
pub struct MdEnv<F: ForceField> {
    ff: F,
    sys: AtomSystem,
    initial: MoleculeSystem,
    dt: f64,
    thermostat: Option<Thermostat>,
    seed: Option<(f64, u64)>,
    step_count: usize,
}

impl<F: ForceField> MdEnv<F> {
    /// Build an environment from an IR molecule store and a force field.
    pub fn new(mol: &MoleculeSystem, ff: F, dt: f64) -> Result<Self, String> {
        let sys = AtomSystem::from_ir(mol)?;
        Ok(Self {
            ff,
            sys,
            initial: mol.clone(),
            dt,
            thermostat: None,
            seed: None,
            step_count: 0,
        })
    }

    /// Enable NVT dynamics with the given thermostat.
    pub fn with_thermostat(mut self, t: Thermostat) -> Self {
        self.thermostat = Some(t);
        self
    }

    /// Seed Maxwell-Boltzmann velocities at `target_k` (deterministic from
    /// `seed`), now and on every reset, so the run starts with thermal motion.
    pub fn seeded(mut self, target_k: f64, seed: u64) -> Self {
        self.seed = Some((target_k, seed));
        self.sys.seed_velocities(target_k, seed);
        self
    }

    /// Reset to the initial structure, re-seeding velocities if configured.
    pub fn reset(&mut self) -> Result<MdObservation, String> {
        self.sys = AtomSystem::from_ir(&self.initial)?;
        if let Some((t, seed)) = self.seed {
            self.sys.seed_velocities(t, seed);
        }
        self.step_count = 0;
        Ok(self.observe())
    }

    /// Run `n` velocity-Verlet steps and return the final observation.
    pub fn run(&mut self, n: usize) -> MdObservation {
        let mut integ = Integrator::new(&self.ff, &self.sys, self.dt);
        if let Some(t) = self.thermostat {
            integ = integ.with_thermostat(t);
        }
        for _ in 0..n {
            integ.step(&mut self.sys);
            self.step_count += 1;
        }
        self.observe()
    }

    /// Current observation without stepping.
    pub fn observe(&self) -> MdObservation {
        let (pot, forces) = self.ff.energy_forces(&self.sys);
        let ke = self.sys.kinetic_energy();
        let mut max_force = 0.0_f64;
        for f in &forces {
            for &c in f {
                max_force = max_force.max(c.abs());
            }
        }
        MdObservation {
            step: self.step_count,
            potential_energy: pot,
            kinetic_energy: ke,
            total_energy: pot + ke,
            temperature: self.sys.temperature(),
            max_force,
        }
    }

    /// Borrow the current system (e.g. to read positions or export).
    pub fn system(&self) -> &AtomSystem {
        &self.sys
    }

    /// Snapshot the current state back to an IR molecule store.
    pub fn to_ir(&self) -> MoleculeSystem {
        self.sys.to_ir()
    }
}
