//! Interatomic potentials (force fields).
//!
//! Every potential implements [`ForceField`], returning total energy (eV) and
//! per-atom forces (eV/Å). Terms are small and composable so each can be
//! validated in isolation against the finite-difference oracle in
//! [`crate::fd`]. A [`Sum`] combines terms into a full force field.
//!
//! The numerics live in `phyz-md`'s structure-of-arrays engine
//! ([`phyz_md::field`]): the potential types here are re-exports of the phyz
//! implementations, and this module binds them to [`AtomSystem`] (the IR-backed
//! working state) via the [`ForceField`] trait. Periodic boundaries use the
//! minimum-image convention — see [`min_image`] for the exactness contract.

use crate::system::AtomSystem;
use crate::vec3;
use phyz_md::field::Lattice;
use vcad_ir::molecule::Cell;

pub use phyz_md::field::potentials::{Coulomb, HarmonicAngles, HarmonicBonds, LennardJones};

/// A potential energy function over an [`AtomSystem`].
pub trait ForceField {
    /// Return total potential energy (eV) and per-atom forces (eV/Å).
    fn energy_forces(&self, sys: &AtomSystem) -> (f64, Vec<[f64; 3]>);

    /// Convenience: energy only.
    fn energy(&self, sys: &AtomSystem) -> f64 {
        self.energy_forces(sys).0
    }
}

/// Convert an IR [`Cell`] into the phyz-md lattice representation.
#[inline]
pub(crate) fn lattice(cell: &Option<Cell>) -> Option<Lattice> {
    cell.as_ref().map(|c| Lattice {
        a: c.a,
        b: c.b,
        c: c.c,
        periodic: c.periodic,
    })
}

/// Minimum-image displacement `ri - rj`, wrapped into the cell if one is
/// present. Diagonal (orthorhombic) cells take a fast per-axis path (exact
/// minimum image); general cells wrap in fractional coordinates (`s = H⁻¹d`,
/// round the periodic components, map back). For a non-orthorhombic cell,
/// fractional rounding recovers the exact minimum image whenever that image
/// is shorter than half the cell's minimum slab width — so with an
/// interaction cutoff below that bound (the usual MD condition, and true of
/// every strained cell in an elastic-constant sweep) all images that matter
/// are exact; displacements near the Wigner–Seitz boundary may wrap to a
/// near-minimal image instead, the standard trade-off. Returns the raw
/// displacement for degenerate (non-invertible) cells.
#[inline]
pub fn min_image(d: [f64; 3], cell: &Option<Cell>) -> [f64; 3] {
    phyz_md::field::min_image(d, lattice(cell).as_ref())
}

impl ForceField for LennardJones {
    fn energy_forces(&self, sys: &AtomSystem) -> (f64, Vec<[f64; 3]>) {
        self.compute(&sys.numbers, &sys.positions, lattice(&sys.cell).as_ref())
    }
}

impl ForceField for HarmonicBonds {
    fn energy_forces(&self, sys: &AtomSystem) -> (f64, Vec<[f64; 3]>) {
        let bonds: Vec<(usize, usize)> = sys.bonds.iter().map(|b| (b.i, b.j)).collect();
        self.compute(&bonds, &sys.positions, lattice(&sys.cell).as_ref())
    }
}

impl ForceField for HarmonicAngles {
    fn energy_forces(&self, sys: &AtomSystem) -> (f64, Vec<[f64; 3]>) {
        self.compute(&sys.positions, lattice(&sys.cell).as_ref())
    }
}

impl ForceField for Coulomb {
    fn energy_forces(&self, sys: &AtomSystem) -> (f64, Vec<[f64; 3]>) {
        self.compute(&sys.charges, &sys.positions, lattice(&sys.cell).as_ref())
    }
}

/// A boxed force field is itself a force field, so `Box<dyn ForceField>` can be
/// used wherever `F: ForceField` is expected (e.g. `MdEnv<Box<dyn ForceField>>`
/// when the term set is chosen at runtime).
impl ForceField for Box<dyn ForceField> {
    fn energy_forces(&self, sys: &AtomSystem) -> (f64, Vec<[f64; 3]>) {
        (**self).energy_forces(sys)
    }
}

/// Sum of several force-field terms.
pub struct Sum {
    /// The component terms.
    pub terms: Vec<Box<dyn ForceField>>,
}

impl Sum {
    /// Build from a list of boxed terms.
    pub fn new(terms: Vec<Box<dyn ForceField>>) -> Self {
        Self { terms }
    }
}

impl ForceField for Sum {
    fn energy_forces(&self, sys: &AtomSystem) -> (f64, Vec<[f64; 3]>) {
        let n = sys.len();
        let mut total_e = 0.0;
        let mut total_f = vec![[0.0; 3]; n];
        for t in &self.terms {
            let (e, f) = t.energy_forces(sys);
            total_e += e;
            for i in 0..n {
                vec3::add_assign(&mut total_f[i], f[i]);
            }
        }
        (total_e, total_f)
    }
}
