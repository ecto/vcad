//! Runtime atomic system: the mutable working state the simulator operates on,
//! built from (and writable back to) the IR [`MoleculeSystem`].
//!
//! Unlike the IR store (which is a compact, serializable structure-of-arrays),
//! this expands per-atom mass/charge and element for fast inner loops.

use crate::element;
use crate::vec3;
use vcad_ir::molecule::{Bond, Cell, MoleculeSystem, Species};

/// A bonded pair with its equilibrium reference, resolved once at build time.
#[derive(Debug, Clone, Copy)]
pub struct BondPair {
    /// First atom index.
    pub i: usize,
    /// Second atom index.
    pub j: usize,
    /// Bond order.
    pub order: f64,
}

/// The simulator's working state.
#[derive(Debug, Clone)]
pub struct AtomSystem {
    /// Element symbol per atom.
    pub elements: Vec<String>,
    /// Atomic number per atom.
    pub numbers: Vec<u32>,
    /// Mass in amu per atom.
    pub masses: Vec<f64>,
    /// Partial charge in e per atom.
    pub charges: Vec<f64>,
    /// Position in Å per atom.
    pub positions: Vec<[f64; 3]>,
    /// Velocity in Å/fs per atom.
    pub velocities: Vec<[f64; 3]>,
    /// Bonded pairs.
    pub bonds: Vec<BondPair>,
    /// Optional periodic cell.
    pub cell: Option<Cell>,
}

impl AtomSystem {
    /// Number of atoms.
    pub fn len(&self) -> usize {
        self.positions.len()
    }

    /// Whether there are no atoms.
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// Build a runtime system from an IR molecule store, filling masses/charges
    /// from the species table (falling back to periodic-table data when a
    /// species mass is non-positive).
    pub fn from_ir(mol: &MoleculeSystem) -> Result<Self, String> {
        mol.validate()?;
        let n = mol.len();
        let mut elements = Vec::with_capacity(n);
        let mut numbers = Vec::with_capacity(n);
        let mut masses = Vec::with_capacity(n);
        let mut charges = Vec::with_capacity(n);
        for i in 0..n {
            let sp = &mol.species[mol.species_idx[i] as usize];
            let data = element::lookup(&sp.element);
            elements.push(sp.element.clone());
            numbers.push(if sp.atomic_number > 0 {
                sp.atomic_number
            } else {
                data.number
            });
            masses.push(if sp.mass > 0.0 { sp.mass } else { data.mass });
            charges.push(sp.charge);
        }
        let velocities = if mol.velocities.len() == n {
            mol.velocities.clone()
        } else {
            vec![[0.0; 3]; n]
        };
        let bonds = mol
            .bonds
            .iter()
            .map(|b| BondPair {
                i: b.a as usize,
                j: b.b as usize,
                order: b.order,
            })
            .collect();
        Ok(Self {
            elements,
            numbers,
            masses,
            charges,
            positions: mol.positions.clone(),
            velocities,
            bonds,
            cell: mol.cell,
        })
    }

    /// Write the current positions/velocities back into an IR molecule store,
    /// preserving the species table and bonds.
    pub fn to_ir(&self) -> MoleculeSystem {
        // Deduplicate species by (element, charge, atomic number).
        let mut species: Vec<Species> = Vec::new();
        let mut species_idx: Vec<u32> = Vec::with_capacity(self.len());
        for i in 0..self.len() {
            let key = (&self.elements[i], self.numbers[i], self.charges[i]);
            let found = species
                .iter()
                .position(|s| s.element == *key.0 && s.atomic_number == key.1 && s.charge == key.2);
            let idx = match found {
                Some(k) => k,
                None => {
                    species.push(Species {
                        element: self.elements[i].clone(),
                        atomic_number: self.numbers[i],
                        mass: self.masses[i],
                        charge: self.charges[i],
                        label: None,
                        radius: None,
                        color: None,
                    });
                    species.len() - 1
                }
            };
            species_idx.push(idx as u32);
        }
        MoleculeSystem {
            species,
            positions: self.positions.clone(),
            species_idx,
            velocities: self.velocities.clone(),
            bonds: self
                .bonds
                .iter()
                .map(|b| Bond {
                    a: b.i as u32,
                    b: b.j as u32,
                    order: b.order,
                })
                .collect(),
            cell: self.cell,
            name: None,
        }
    }

    /// Center of mass in Å.
    pub fn center_of_mass(&self) -> [f64; 3] {
        let mut com = [0.0; 3];
        let mut total = 0.0;
        for i in 0..self.len() {
            vec3::add_assign(&mut com, vec3::scale(self.positions[i], self.masses[i]));
            total += self.masses[i];
        }
        if total > 0.0 {
            vec3::scale(com, 1.0 / total)
        } else {
            [0.0; 3]
        }
    }

    /// Total kinetic energy in eV.
    pub fn kinetic_energy(&self) -> f64 {
        let mut ke = 0.0;
        for i in 0..self.len() {
            ke += 0.5 * self.masses[i] * vec3::norm2(self.velocities[i]);
        }
        ke / crate::units::FORCE_TO_ACCEL
    }

    /// Seed velocities to a Maxwell-Boltzmann distribution at `target_k`,
    /// deterministically from `seed` (xorshift PRNG — no `rand` dependency, so
    /// results reproduce across platforms). Removes net center-of-mass drift and
    /// rescales to hit the target temperature exactly.
    pub fn seed_velocities(&mut self, target_k: f64, seed: u64) {
        let n = self.len();
        if n == 0 || target_k <= 0.0 {
            return;
        }
        let mut s = seed ^ 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            // xorshift64* → uniform in [-0.5, 0.5)
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            ((s >> 11) as f64) / ((1u64 << 53) as f64) - 0.5
        };
        for v in &mut self.velocities {
            *v = [next(), next(), next()];
        }
        // Remove center-of-mass momentum.
        let mut p = [0.0; 3];
        let mut mtot = 0.0;
        for i in 0..n {
            vec3::add_assign(&mut p, vec3::scale(self.velocities[i], self.masses[i]));
            mtot += self.masses[i];
        }
        if mtot > 0.0 {
            let vcom = vec3::scale(p, 1.0 / mtot);
            for v in &mut self.velocities {
                *v = vec3::sub(*v, vcom);
            }
        }
        // Rescale to exactly the target temperature.
        let cur = self.temperature();
        if cur > 1e-12 {
            let lambda = (target_k / cur).sqrt();
            for v in &mut self.velocities {
                *v = vec3::scale(*v, lambda);
            }
        }
    }

    /// Instantaneous temperature in K from the kinetic energy, using
    /// `3N` degrees of freedom (no constraints removed).
    pub fn temperature(&self) -> f64 {
        let n = self.len();
        if n == 0 {
            return 0.0;
        }
        let dof = 3.0 * n as f64;
        2.0 * self.kinetic_energy() / (dof * crate::units::KB_EV_PER_K)
    }
}
