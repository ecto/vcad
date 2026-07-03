//! Homogenization — the atoms → continuum bridge.
//!
//! Given a periodic atomic structure and a force field, produce the bulk
//! material properties a millimetre-scale part consumes: mass density and
//! (for cubic crystals) elastic constants, reduced to isotropic engineering
//! moduli by the Voigt–Reuss–Hill average. The output [`MaterialCard`] is the
//! contract between the atomic domain and part-scale physics: its density
//! feeds the mass-property channel of `vcad-kernel-physics::diff`, closing
//! the cross-scale chain `d(rollout objective)/d(lattice parameter)`
//! (see `docs/atomic-design-simulation.md`).
//!
//! # Method
//!
//! Elastic constants come from strain-energy second differences. A strain
//! `ε` maps positions and lattice vectors affinely (`r' = (I + ε) r`,
//! `H' = (I + ε) H`); internal coordinates optionally re-relax under the
//! strained cell (FIRE). Three cubic strain modes give the three cubic
//! constants:
//!
//! ```text
//! uniaxial     ε_xx = δ            u(δ) = ½ C11 δ²
//! hydrostatic  ε = δ·I             u(δ) = 3/2 (C11 + 2 C12) δ²
//! shear        ε_xy = ε_yx = δ     u(δ) = 2 C44 δ²
//! ```
//!
//! with `u = (U(+δ) + U(−δ) − 2U(0)) / (2 V₀)` the central second
//! difference of the energy density. Shear strains make the cell
//! non-orthorhombic — the general-cell minimum image in
//! [`crate::potential::min_image`] is what makes that mode legal.
//!
//! The constants are physically meaningful stiffnesses when the reference
//! structure is at mechanical equilibrium (zero pressure); use
//! [`equilibrium_scale`] to find the equilibrium lattice scaling first.
//!
//! # Gradients
//!
//! [`fd_gradient`] central-differences any scalar property of a
//! parametrized structure (density, a modulus, …) with respect to design
//! parameters θ. As everywhere in this crate, finite differences are the
//! oracle and the seam where `tang-ad` reverse mode drops in later; the
//! chain into part-scale rollouts is
//! `vcad-kernel-physics::diff::rollout_gradient_via_density`.

use serde::{Deserialize, Serialize};

use crate::minimize::{minimize, MinimizeOptions};
use crate::potential::ForceField;
use crate::system::AtomSystem;
use crate::vec3;
use vcad_ir::molecule::{Cell, MoleculeSystem};

/// Mass density conversion: `amu/Å³ → kg/m³`.
///
/// `1 amu = 1.66053906660e-27 kg`, `1 Å³ = 1e-30 m³`.
pub const AMU_PER_A3_TO_KG_M3: f64 = 1_660.539_066_60;

/// Energy density conversion: `eV/Å³ → GPa`.
///
/// `1 eV = 1.602176634e-19 J`, so `1 eV/Å³ = 1.602176634e11 Pa`.
pub const EV_PER_A3_TO_GPA: f64 = 160.217_663_4;

/// The homogenized bulk-property record — what the atomic domain hands to
/// the part scale. Densities are SI; moduli are GPa; the isotropic moduli
/// are the Voigt–Reuss–Hill reduction of the cubic constants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaterialCard {
    /// Mass density (kg/m³).
    pub density_kg_m3: f64,
    /// Cubic elastic constant C11 (GPa).
    pub c11_gpa: f64,
    /// Cubic elastic constant C12 (GPa).
    pub c12_gpa: f64,
    /// Cubic elastic constant C44 (GPa).
    pub c44_gpa: f64,
    /// Bulk modulus `K = (C11 + 2 C12)/3` (GPa).
    pub bulk_gpa: f64,
    /// Isotropic (VRH) shear modulus (GPa).
    pub shear_gpa: f64,
    /// Isotropic Young's modulus `E = 9KG/(3K + G)` (GPa).
    pub youngs_gpa: f64,
    /// Isotropic Poisson ratio `ν = (3K − 2G)/(2(3K + G))`.
    pub poisson: f64,
    /// Potential energy per atom at the reference state (eV; negative for a
    /// bound crystal).
    pub energy_ev_atom: f64,
    /// Number of atoms in the supercell.
    pub atoms: usize,
    /// Reference cell volume (Å³).
    pub volume_a3: f64,
}

/// Options for a homogenization run.
#[derive(Debug, Clone, Copy)]
pub struct HomogenizeOptions {
    /// Strain amplitude δ for the second differences.
    pub strain: f64,
    /// Re-relax internal coordinates (FIRE) at the reference state and under
    /// each strained cell. A no-op for crystals whose sites are fixed by
    /// symmetry (every force is already zero), essential for crystals with
    /// internal degrees of freedom.
    pub relax_internal: bool,
    /// FIRE options for the internal relaxations.
    pub minimize: MinimizeOptions,
}

impl Default for HomogenizeOptions {
    fn default() -> Self {
        Self {
            strain: 2e-3,
            relax_internal: true,
            minimize: MinimizeOptions {
                force_tol: 1e-6,
                ..MinimizeOptions::default()
            },
        }
    }
}

/// Cell volume `|a · (b × c)|` in Å³.
pub fn cell_volume(c: &Cell) -> f64 {
    vec3::dot(c.a, vec3::cross(c.b, c.c)).abs()
}

/// Mass density in kg/m³. Requires at least one atom and a periodic cell on
/// all three axes with positive volume — density of a cluster (or of
/// nothing) is not defined.
pub fn density(sys: &AtomSystem) -> Result<f64, String> {
    if sys.is_empty() {
        return Err("density requires at least one atom".to_string());
    }
    let Some(cell) = &sys.cell else {
        return Err("density requires a periodic cell".to_string());
    };
    if !(cell.periodic[0] && cell.periodic[1] && cell.periodic[2]) {
        return Err("density requires periodicity on all three axes".to_string());
    }
    let vol = cell_volume(cell);
    if vol <= 0.0 {
        return Err("density requires a cell with positive volume".to_string());
    }
    let mass_amu: f64 = sys.masses.iter().sum();
    Ok(mass_amu / vol * AMU_PER_A3_TO_KG_M3)
}

/// Apply the small-strain map `r' = (I + ε) r`, `H' = (I + ε) H` to a copy
/// of the system. `eps` is the full (symmetric) strain tensor.
fn apply_strain(sys: &AtomSystem, eps: &[[f64; 3]; 3]) -> AtomSystem {
    let f = [
        [1.0 + eps[0][0], eps[0][1], eps[0][2]],
        [eps[1][0], 1.0 + eps[1][1], eps[1][2]],
        [eps[2][0], eps[2][1], 1.0 + eps[2][2]],
    ];
    let map = |v: [f64; 3]| -> [f64; 3] {
        [
            f[0][0] * v[0] + f[0][1] * v[1] + f[0][2] * v[2],
            f[1][0] * v[0] + f[1][1] * v[1] + f[1][2] * v[2],
            f[2][0] * v[0] + f[2][1] * v[1] + f[2][2] * v[2],
        ]
    };
    let mut out = sys.clone();
    for p in &mut out.positions {
        *p = map(*p);
    }
    if let Some(c) = &mut out.cell {
        c.a = map(c.a);
        c.b = map(c.b);
        c.c = map(c.c);
    }
    out
}

/// Potential energy of the system under strain `eps`, with optional internal
/// relaxation.
fn strained_energy(
    ff: &dyn ForceField,
    sys: &AtomSystem,
    eps: &[[f64; 3]; 3],
    opts: &HomogenizeOptions,
) -> f64 {
    let mut strained = apply_strain(sys, eps);
    if opts.relax_internal {
        minimize(ff, &mut strained, &opts.minimize).energy
    } else {
        ff.energy(&strained)
    }
}

/// Cubic elastic constants in GPa, via strain-energy second differences on
/// the (assumed cubic) periodic system. The system should be at mechanical
/// equilibrium for the constants to be true stiffnesses.
pub fn elastic_constants(
    ff: &dyn ForceField,
    sys: &AtomSystem,
    opts: &HomogenizeOptions,
) -> Result<(f64, f64, f64), String> {
    let Some(cell) = &sys.cell else {
        return Err("elastic constants require a periodic cell".to_string());
    };
    let v0 = cell_volume(cell);
    if v0 <= 0.0 {
        return Err("elastic constants require a cell with positive volume".to_string());
    }
    let d = opts.strain;
    if d <= 0.0 {
        return Err("strain amplitude must be positive".to_string());
    }

    // Reference energy, internally relaxed under the same policy as the
    // strained probes so the second differences are consistent.
    let mut base = sys.clone();
    let u0 = if opts.relax_internal {
        minimize(ff, &mut base, &opts.minimize).energy
    } else {
        ff.energy(&base)
    };

    let zero = [[0.0; 3]; 3];
    let mode = |exx: f64, eyy: f64, ezz: f64, exy: f64| -> [[f64; 3]; 3] {
        let mut e = zero;
        e[0][0] = exx;
        e[1][1] = eyy;
        e[2][2] = ezz;
        e[0][1] = exy;
        e[1][0] = exy;
        e
    };
    // Central second difference of the energy density, d²u/dδ² in eV/Å³.
    let curvature = |plus: [[f64; 3]; 3], minus: [[f64; 3]; 3]| -> f64 {
        let up = strained_energy(ff, &base, &plus, opts);
        let um = strained_energy(ff, &base, &minus, opts);
        (up + um - 2.0 * u0) / (v0 * d * d)
    };

    // uniaxial: d²u/dδ² = C11
    let c11 = curvature(mode(d, 0.0, 0.0, 0.0), mode(-d, 0.0, 0.0, 0.0));
    // hydrostatic: d²u/dδ² = 3 (C11 + 2 C12)
    let hydro = curvature(mode(d, d, d, 0.0), mode(-d, -d, -d, 0.0));
    let c12 = (hydro / 3.0 - c11) / 2.0;
    // shear ε_xy = δ: d²u/dδ² = 4 C44
    let c44 = curvature(mode(0.0, 0.0, 0.0, d), mode(0.0, 0.0, 0.0, -d)) / 4.0;

    Ok((
        c11 * EV_PER_A3_TO_GPA,
        c12 * EV_PER_A3_TO_GPA,
        c44 * EV_PER_A3_TO_GPA,
    ))
}

/// Voigt–Reuss–Hill isotropic reduction of cubic constants:
/// `(K, G_vrh, E, ν)` in GPa (ν dimensionless).
pub fn vrh_moduli(c11: f64, c12: f64, c44: f64) -> (f64, f64, f64, f64) {
    let k = (c11 + 2.0 * c12) / 3.0;
    let g_voigt = (c11 - c12 + 3.0 * c44) / 5.0;
    let reuss_den = 4.0 * c44 + 3.0 * (c11 - c12);
    let g_reuss = if reuss_den.abs() > 1e-12 {
        5.0 * (c11 - c12) * c44 / reuss_den
    } else {
        0.0
    };
    let g = 0.5 * (g_voigt + g_reuss);
    let e = if (3.0 * k + g).abs() > 1e-12 {
        9.0 * k * g / (3.0 * k + g)
    } else {
        0.0
    };
    let nu = if (3.0 * k + g).abs() > 1e-12 {
        (3.0 * k - 2.0 * g) / (2.0 * (3.0 * k + g))
    } else {
        0.0
    };
    (k, g, e, nu)
}

/// Homogenize a periodic structure under a force field into a
/// [`MaterialCard`].
pub fn homogenize(
    ff: &dyn ForceField,
    mol: &MoleculeSystem,
    opts: &HomogenizeOptions,
) -> Result<MaterialCard, String> {
    let sys = AtomSystem::from_ir(mol)?;
    let rho = density(&sys)?;
    let cell = sys.cell.as_ref().expect("density verified the cell");
    let v0 = cell_volume(cell);
    let (c11, c12, c44) = elastic_constants(ff, &sys, opts)?;
    let (k, g, e, nu) = vrh_moduli(c11, c12, c44);

    // Reference energy per atom, relaxed consistently with the sweeps.
    let mut base = sys.clone();
    let u0 = if opts.relax_internal {
        minimize(ff, &mut base, &opts.minimize).energy
    } else {
        ff.energy(&base)
    };

    Ok(MaterialCard {
        density_kg_m3: rho,
        c11_gpa: c11,
        c12_gpa: c12,
        c44_gpa: c44,
        bulk_gpa: k,
        shear_gpa: g,
        youngs_gpa: e,
        poisson: nu,
        energy_ev_atom: if sys.is_empty() {
            0.0
        } else {
            u0 / sys.len() as f64
        },
        atoms: sys.len(),
        volume_a3: v0,
    })
}

/// Find the isotropic scale factor `s ∈ [lo, hi]` that minimizes the
/// potential energy of `sys` with positions and cell scaled by `s` (golden
/// section; the 1-D "zero pressure" search that precedes an elastic-constant
/// sweep). Returns the optimal `s`.
pub fn equilibrium_scale(ff: &dyn ForceField, sys: &AtomSystem, lo: f64, hi: f64) -> f64 {
    let energy_at = |s: f64| -> f64 {
        let eps = [
            [s - 1.0, 0.0, 0.0],
            [0.0, s - 1.0, 0.0],
            [0.0, 0.0, s - 1.0],
        ];
        ff.energy(&apply_strain(sys, &eps))
    };
    const INV_PHI: f64 = 0.618_033_988_749_894_8;
    let (mut a, mut b) = (lo, hi);
    let mut c = b - INV_PHI * (b - a);
    let mut d = a + INV_PHI * (b - a);
    let (mut fc, mut fd) = (energy_at(c), energy_at(d));
    for _ in 0..80 {
        if fc < fd {
            b = d;
            d = c;
            fd = fc;
            c = b - INV_PHI * (b - a);
            fc = energy_at(c);
        } else {
            a = c;
            c = d;
            fc = fd;
            d = a + INV_PHI * (b - a);
            fd = energy_at(d);
        }
        if (b - a).abs() < 1e-10 {
            break;
        }
    }
    0.5 * (a + b)
}

/// Central-difference gradient of a scalar property of a parametrized
/// structure — `d f / d θ`. The finite-difference oracle for every
/// homogenized-property gradient (density, a modulus), and the seam where
/// `tang-ad` reverse mode drops in later.
pub fn fd_gradient(f: &dyn Fn(&[f64]) -> f64, theta: &[f64], h: f64) -> Vec<f64> {
    let mut g = vec![0.0; theta.len()];
    let mut probe = theta.to_vec();
    for k in 0..theta.len() {
        let orig = theta[k];
        probe[k] = orig + h;
        let fp = f(&probe);
        probe[k] = orig - h;
        let fm = f(&probe);
        probe[k] = orig;
        g[k] = (fp - fm) / (2.0 * h);
    }
    g
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder;
    use crate::potential::{min_image, LennardJones};

    /// Argon LJ parameters (ε/k_B = 119.8 K, σ = 3.405 Å) with a cutoff
    /// between the 3rd and 4th neighbor shells of the FCC lattice, so small
    /// strains never move a shell across the cutoff.
    const AR_EPS: f64 = 0.010_32;
    const AR_SIGMA: f64 = 3.405;
    const AR_CUTOFF: f64 = 2.0 * AR_SIGMA;

    fn argon_lj() -> LennardJones {
        LennardJones::monatomic(AR_EPS, AR_SIGMA, AR_CUTOFF)
    }

    fn argon_fcc(a: f64, n: usize) -> MoleculeSystem {
        builder::fcc("Ar", a, n, n, n)
    }

    #[test]
    fn density_is_physical_and_scales_inversely_with_volume() {
        let sys = AtomSystem::from_ir(&argon_fcc(5.26, 2)).unwrap();
        let rho = density(&sys).unwrap();
        // Solid argon near 0 K is ~1770 kg/m³; the hand value from a = 5.26 Å
        // must land in the physical window.
        assert!(
            (1_700.0..1_900.0).contains(&rho),
            "argon density {rho} kg/m³ out of physical range"
        );
        // ρ ∝ a⁻³ exactly.
        let rho_a = density(&AtomSystem::from_ir(&argon_fcc(5.0, 2)).unwrap()).unwrap();
        let rho_b = density(&AtomSystem::from_ir(&argon_fcc(6.0, 2)).unwrap()).unwrap();
        let ratio = rho_a / rho_b;
        assert!(((6.0f64 / 5.0).powi(3) - ratio).abs() < 1e-9);
    }

    #[test]
    fn density_requires_full_periodicity() {
        let mut mol = argon_fcc(5.26, 2);
        let sys_no_cell = {
            let mut m = mol.clone();
            m.cell = None;
            AtomSystem::from_ir(&m).unwrap()
        };
        assert!(density(&sys_no_cell).is_err());
        mol.cell.as_mut().unwrap().periodic = [true, true, false];
        assert!(density(&AtomSystem::from_ir(&mol).unwrap()).is_err());
    }

    #[test]
    fn general_min_image_picks_the_wrapped_image_in_a_sheared_cell() {
        // Sheared cell: a = (10,0,0), b = (1,10,0), c = (0,0,10). The
        // displacement (9.6, 0.2, 0) wraps by −a to (−0.4, 0.2, 0).
        let cell = Some(Cell {
            a: [10.0, 0.0, 0.0],
            b: [1.0, 10.0, 0.0],
            c: [0.0, 0.0, 10.0],
            periodic: [true, true, true],
        });
        let w = min_image([9.6, 0.2, 0.0], &cell);
        assert!((w[0] + 0.4).abs() < 1e-12, "got {w:?}");
        assert!((w[1] - 0.2).abs() < 1e-12);
        assert!(w[2].abs() < 1e-12);
    }

    #[test]
    fn min_image_wraps_left_handed_diagonal_cells() {
        // A mirrored (negative-determinant) diagonal cell must still be
        // periodic — it routes through the general path, not the raw-
        // displacement fallback.
        let cell = Some(Cell {
            a: [-10.0, 0.0, 0.0],
            b: [0.0, 10.0, 0.0],
            c: [0.0, 0.0, 10.0],
            periodic: [true, true, true],
        });
        let w = min_image([9.6, 0.0, 0.0], &cell);
        assert!((w[0] + 0.4).abs() < 1e-12, "got {w:?}");
    }

    #[test]
    fn energy_is_invariant_under_lattice_translations_in_a_sheared_cell() {
        // Shear the argon crystal (as the C44 sweep does), translate one atom
        // by a full lattice vector, and check the energy is unchanged — the
        // general-cell minimum image is what makes this hold.
        let ff = argon_lj();
        let base = AtomSystem::from_ir(&argon_fcc(5.3, 3)).unwrap();
        let d = 5e-3;
        let eps = [[0.0, d, 0.0], [d, 0.0, 0.0], [0.0, 0.0, 0.0]];
        let sheared = apply_strain(&base, &eps);
        let e0 = ff.energy(&sheared);
        let mut moved = sheared.clone();
        let av = moved.cell.as_ref().unwrap().a;
        moved.positions[7] = crate::vec3::add(moved.positions[7], av);
        let e1 = ff.energy(&moved);
        assert!(
            (e0 - e1).abs() < 1e-9,
            "energy changed under lattice translation: {e0} vs {e1}"
        );
    }

    #[test]
    fn equilibrium_scale_finds_a_zero_pressure_lattice() {
        let ff = argon_lj();
        let sys = AtomSystem::from_ir(&argon_fcc(5.3, 3)).unwrap();
        let s = equilibrium_scale(&ff, &sys, 0.9, 1.1);
        let a_eq = 5.3 * s;
        assert!(
            (5.0..5.6).contains(&a_eq),
            "argon a_eq = {a_eq} Å out of range"
        );
        // dU/ds ≈ 0 at the optimum.
        let du = fd_gradient(
            &|t: &[f64]| {
                let eps = [
                    [t[0] - 1.0, 0.0, 0.0],
                    [0.0, t[0] - 1.0, 0.0],
                    [0.0, 0.0, t[0] - 1.0],
                ];
                ff.energy(&apply_strain(&sys, &eps))
            },
            &[s],
            1e-6,
        )[0];
        // Curvature at the minimum is O(100) eV per unit s; a gradient this
        // small means s is within ~1e-6 of the optimum.
        assert!(du.abs() < 1e-2, "dU/ds = {du} at s = {s}");
    }

    #[test]
    fn lj_fcc_elastic_constants_satisfy_stability_and_cauchy() {
        let ff = argon_lj();
        let base = AtomSystem::from_ir(&argon_fcc(5.3, 3)).unwrap();
        let s = equilibrium_scale(&ff, &base, 0.9, 1.1);
        let a_eq = 5.3 * s;
        let mol = argon_fcc(a_eq, 3);
        let card = homogenize(&ff, &mol, &HomogenizeOptions::default()).unwrap();

        // Mechanical stability of a cubic crystal.
        assert!(card.c11_gpa > 0.0, "C11 = {}", card.c11_gpa);
        assert!(card.c44_gpa > 0.0, "C44 = {}", card.c44_gpa);
        assert!(card.c11_gpa > card.c12_gpa.abs(), "C11 ≤ |C12|");
        assert!(card.bulk_gpa > 0.0);

        // Central-force pair potential, centrosymmetric sites, zero
        // pressure ⇒ the Cauchy relation C12 = C44.
        let rel = (card.c12_gpa - card.c44_gpa).abs() / card.c44_gpa;
        assert!(
            rel < 0.05,
            "Cauchy violation: C12 = {} GPa, C44 = {} GPa ({}%)",
            card.c12_gpa,
            card.c44_gpa,
            rel * 100.0
        );

        // Physical windows for LJ argon near 0 K: K a few GPa, ν well inside
        // (0, 0.5), bound crystal.
        assert!(
            (0.5..10.0).contains(&card.bulk_gpa),
            "K = {} GPa",
            card.bulk_gpa
        );
        assert!(
            card.poisson > 0.0 && card.poisson < 0.5,
            "ν = {}",
            card.poisson
        );
        assert!(card.youngs_gpa > 0.0);
        assert!(card.energy_ev_atom < 0.0, "unbound crystal");
    }

    #[test]
    fn vrh_moduli_match_hand_computed_values() {
        // C11 = 10, C12 = 6, C44 = 5 (GPa), worked by hand:
        //   K   = 22/3
        //   G_V = (10 − 6 + 15)/5 = 3.8
        //   G_R = 5·4·5 / (20 + 12) = 3.125
        //   G   = (3.8 + 3.125)/2 = 3.4625
        //   E   = 9KG/(3K + G) = 228.525/25.4625 = 8.974963…
        //   ν   = (3K − 2G)/(2(3K + G)) = 15.075/50.925 = 0.2960236…
        // Literal expectations keep this independent of the implementation's
        // own formulas.
        let (k, g, e, nu) = vrh_moduli(10.0, 6.0, 5.0);
        assert!((k - 22.0 / 3.0).abs() < 1e-12, "K = {k}");
        assert!((g - 3.4625).abs() < 1e-12, "G = {g}");
        assert!((e - 8.974_963_28).abs() < 1e-6, "E = {e}");
        assert!((nu - 0.296_023_56).abs() < 1e-7, "ν = {nu}");
        // Hill's G lies between the Reuss and Voigt bounds.
        assert!((3.125..=3.8).contains(&g));
    }

    #[test]
    fn fd_gradient_matches_analytic_density_derivative() {
        // ρ(a) ∝ a⁻³ ⇒ dρ/da = −3ρ/a; the FD oracle must reproduce it.
        let f = |theta: &[f64]| -> f64 {
            density(&AtomSystem::from_ir(&argon_fcc(theta[0], 2)).unwrap()).unwrap()
        };
        let a = 5.26;
        let g = fd_gradient(&f, &[a], 1e-5)[0];
        let rho = f(&[a]);
        let analytic = -3.0 * rho / a;
        assert!(
            ((g - analytic) / analytic).abs() < 1e-8,
            "FD dρ/da = {g}, analytic = {analytic}"
        );
    }
}
