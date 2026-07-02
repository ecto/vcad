//! Interatomic potentials (force fields).
//!
//! Every potential implements [`ForceField`], returning total energy (eV) and
//! per-atom forces (eV/Å). Terms are small and composable so each can be
//! validated in isolation against the finite-difference oracle in
//! [`crate::fd`]. A [`Sum`] combines terms into a full force field.
//!
//! Periodic boundaries use the minimum-image convention for orthorhombic
//! cells (diagonal lattice); non-orthorhombic cells are treated as
//! non-periodic in the force evaluation.

use std::collections::HashMap;

use crate::system::AtomSystem;
use crate::units::KE_COULOMB;
use crate::vec3;
use vcad_ir::molecule::Cell;

/// A potential energy function over an [`AtomSystem`].
pub trait ForceField {
    /// Return total potential energy (eV) and per-atom forces (eV/Å).
    fn energy_forces(&self, sys: &AtomSystem) -> (f64, Vec<[f64; 3]>);

    /// Convenience: energy only.
    fn energy(&self, sys: &AtomSystem) -> f64 {
        self.energy_forces(sys).0
    }
}

/// Orthorhombic minimum-image displacement `ri - rj`, wrapped into the cell if
/// the cell is present, periodic per-axis, and diagonal. Returns the raw
/// displacement otherwise.
#[inline]
pub fn min_image(d: [f64; 3], cell: &Option<Cell>) -> [f64; 3] {
    let Some(c) = cell else { return d };
    // Only orthorhombic (diagonal) cells are handled for MIC.
    let lx = c.a[0];
    let ly = c.b[1];
    let lz = c.c[2];
    let off_diag =
        c.a[1].abs() + c.a[2].abs() + c.b[0].abs() + c.b[2].abs() + c.c[0].abs() + c.c[1].abs();
    if off_diag > 1e-9 || lx <= 0.0 || ly <= 0.0 || lz <= 0.0 {
        return d;
    }
    let mut out = d;
    let dims = [lx, ly, lz];
    for (k, &l) in dims.iter().enumerate() {
        if c.periodic[k] {
            out[k] -= l * (out[k] / l).round();
        }
    }
    out
}

/// Lennard-Jones potential with per-element parameters and Lorentz-Berthelot
/// mixing, a spherical cutoff, and optional energy shift for continuity.
#[derive(Debug, Clone)]
pub struct LennardJones {
    /// Per-atomic-number `(epsilon eV, sigma Å)`.
    pub params: HashMap<u32, (f64, f64)>,
    /// Fallback parameters for atoms with no entry.
    pub default: (f64, f64),
    /// Cutoff radius in Å.
    pub cutoff: f64,
    /// Shift the potential so `E(cutoff) == 0`.
    pub shift: bool,
}

impl LennardJones {
    /// A single-species LJ fluid (e.g. argon: eps=0.0103 eV, sigma=3.4 Å).
    pub fn monatomic(epsilon: f64, sigma: f64, cutoff: f64) -> Self {
        Self {
            params: HashMap::new(),
            default: (epsilon, sigma),
            cutoff,
            shift: true,
        }
    }

    #[inline]
    fn pair_params(&self, zi: u32, zj: u32) -> (f64, f64) {
        let (ei, si) = self.params.get(&zi).copied().unwrap_or(self.default);
        let (ej, sj) = self.params.get(&zj).copied().unwrap_or(self.default);
        // Lorentz-Berthelot: eps = sqrt(ei ej), sigma = (si+sj)/2.
        ((ei * ej).sqrt(), 0.5 * (si + sj))
    }
}

impl ForceField for LennardJones {
    fn energy_forces(&self, sys: &AtomSystem) -> (f64, Vec<[f64; 3]>) {
        let n = sys.len();
        let mut forces = vec![[0.0; 3]; n];
        let mut energy = 0.0;
        let rc2 = self.cutoff * self.cutoff;
        for i in 0..n {
            for j in (i + 1)..n {
                let d = min_image(vec3::sub(sys.positions[i], sys.positions[j]), &sys.cell);
                let r2 = vec3::norm2(d);
                if r2 > rc2 || r2 < 1e-12 {
                    continue;
                }
                let (eps, sigma) = self.pair_params(sys.numbers[i], sys.numbers[j]);
                let inv_r2 = 1.0 / r2;
                let s2 = sigma * sigma * inv_r2;
                let s6 = s2 * s2 * s2;
                let s12 = s6 * s6;
                let mut e = 4.0 * eps * (s12 - s6);
                if self.shift {
                    let sc2 = sigma * sigma / rc2;
                    let sc6 = sc2 * sc2 * sc2;
                    e -= 4.0 * eps * (sc6 * sc6 - sc6);
                }
                energy += e;
                // E = 4 eps (s12 - s6), s6 = sigma^6 (r2)^-3.
                // dE/dr2 = 4 eps (-6 s12 + 3 s6) / r2.
                // F_i = -dE/dr_i = -dE/dr2 * d(r2)/dr_i = -dE/dr2 * 2 d.
                let de_dr2 = 4.0 * eps * (-6.0 * s12 + 3.0 * s6) * inv_r2;
                let fmag = -2.0 * de_dr2; // multiply by d gives force on i
                let f = vec3::scale(d, fmag);
                vec3::add_assign(&mut forces[i], f);
                vec3::add_assign(&mut forces[j], vec3::scale(f, -1.0));
            }
        }
        (energy, forces)
    }
}

/// Harmonic bond stretching: `E = 0.5 k (r - r0)²` per bond.
#[derive(Debug, Clone)]
pub struct HarmonicBonds {
    /// Force constant in eV/Å².
    pub k: f64,
    /// Equilibrium length in Å (uniform if `per_bond` is empty).
    pub r0: f64,
    /// Optional per-bond equilibrium lengths, matching `sys.bonds` order.
    pub per_bond: Vec<f64>,
}

impl HarmonicBonds {
    /// Uniform harmonic bonds.
    pub fn uniform(k: f64, r0: f64) -> Self {
        Self {
            k,
            r0,
            per_bond: Vec::new(),
        }
    }
}

impl ForceField for HarmonicBonds {
    fn energy_forces(&self, sys: &AtomSystem) -> (f64, Vec<[f64; 3]>) {
        let mut forces = vec![[0.0; 3]; sys.len()];
        let mut energy = 0.0;
        for (bi, b) in sys.bonds.iter().enumerate() {
            let r0 = self.per_bond.get(bi).copied().unwrap_or(self.r0);
            let d = min_image(vec3::sub(sys.positions[b.i], sys.positions[b.j]), &sys.cell);
            let r = vec3::norm(d);
            if r < 1e-12 {
                continue;
            }
            let dr = r - r0;
            energy += 0.5 * self.k * dr * dr;
            // F_i = -k (r - r0) * rhat
            let fmag = -self.k * dr / r;
            let f = vec3::scale(d, fmag);
            vec3::add_assign(&mut forces[b.i], f);
            vec3::add_assign(&mut forces[b.j], vec3::scale(f, -1.0));
        }
        (energy, forces)
    }
}

/// Harmonic angle bending: `E = 0.5 k (θ - θ0)²` over `(i, j, k)` triples with
/// `j` the apex.
#[derive(Debug, Clone)]
pub struct HarmonicAngles {
    /// `(i, apex, k)` atom-index triples.
    pub triples: Vec<(usize, usize, usize)>,
    /// Force constant in eV/rad².
    pub k: f64,
    /// Equilibrium angle in radians.
    pub theta0: f64,
}

impl ForceField for HarmonicAngles {
    fn energy_forces(&self, sys: &AtomSystem) -> (f64, Vec<[f64; 3]>) {
        let mut forces = vec![[0.0; 3]; sys.len()];
        let mut energy = 0.0;
        for &(ia, ja, ka) in &self.triples {
            let rij = min_image(vec3::sub(sys.positions[ia], sys.positions[ja]), &sys.cell);
            let rkj = min_image(vec3::sub(sys.positions[ka], sys.positions[ja]), &sys.cell);
            let lij = vec3::norm(rij);
            let lkj = vec3::norm(rkj);
            if lij < 1e-9 || lkj < 1e-9 {
                continue;
            }
            let cos_t = (vec3::dot(rij, rkj) / (lij * lkj)).clamp(-1.0, 1.0);
            let theta = cos_t.acos();
            let dtheta = theta - self.theta0;
            energy += 0.5 * self.k * dtheta * dtheta;
            let sin_t = (1.0 - cos_t * cos_t).sqrt().max(1e-9);
            let de_dtheta = self.k * dtheta;
            // F_i = -dV/dr_i = (dV/dθ / sinθ) · d(cosθ)/dr_i, with
            // d(cosθ)/dr_i = rkj/(lij·lkj) − cosθ·rij/lij².
            let coef = de_dtheta / sin_t;
            let fi = vec3::scale(
                vec3::sub(
                    vec3::scale(rkj, 1.0 / (lij * lkj)),
                    vec3::scale(rij, cos_t / (lij * lij)),
                ),
                coef,
            );
            let fk = vec3::scale(
                vec3::sub(
                    vec3::scale(rij, 1.0 / (lij * lkj)),
                    vec3::scale(rkj, cos_t / (lkj * lkj)),
                ),
                coef,
            );
            vec3::add_assign(&mut forces[ia], fi);
            vec3::add_assign(&mut forces[ka], fk);
            vec3::add_assign(&mut forces[ja], vec3::scale(vec3::add(fi, fk), -1.0));
        }
        (energy, forces)
    }
}

/// Direct (cutoff) Coulomb interaction: `E = KE q_i q_j / r`.
///
/// This is the simple real-space sum, not Ewald — adequate for clusters and
/// short-range screening, not for long-range lattice sums.
#[derive(Debug, Clone)]
pub struct Coulomb {
    /// Cutoff radius in Å.
    pub cutoff: f64,
}

impl ForceField for Coulomb {
    fn energy_forces(&self, sys: &AtomSystem) -> (f64, Vec<[f64; 3]>) {
        let n = sys.len();
        let mut forces = vec![[0.0; 3]; n];
        let mut energy = 0.0;
        let rc2 = self.cutoff * self.cutoff;
        for i in 0..n {
            if sys.charges[i] == 0.0 {
                continue;
            }
            for j in (i + 1)..n {
                if sys.charges[j] == 0.0 {
                    continue;
                }
                let d = min_image(vec3::sub(sys.positions[i], sys.positions[j]), &sys.cell);
                let r2 = vec3::norm2(d);
                if r2 > rc2 || r2 < 1e-12 {
                    continue;
                }
                let r = r2.sqrt();
                let qq = KE_COULOMB * sys.charges[i] * sys.charges[j];
                energy += qq / r;
                // F_i = qq / r^2 * rhat = qq / r^3 * d
                let f = vec3::scale(d, qq / (r2 * r));
                vec3::add_assign(&mut forces[i], f);
                vec3::add_assign(&mut forces[j], vec3::scale(f, -1.0));
            }
        }
        (energy, forces)
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
