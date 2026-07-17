//! Discrete adjoint: exact-to-discretization gradients of EM quantities
//! of interest.
//!
//! The forward problem is `A(ν)·u = b(I, Br)` with `A` symmetric by
//! construction (shared face conductances) — so the adjoint system is
//! the **same operator** with a different right-hand side, and one extra
//! SOR solve per quantity of interest prices every parameter:
//!
//! - For `J(u)` linear in `u` (flux linkage, force, torque), solve
//!   `A·λ = ∂J/∂u`, then
//!   - **sources**: `dJ/dI_k = λᵀ·U_k` (+ any explicit dependence),
//!     `dJ/d(Br scale) = λᵀ·dep_m` likewise;
//!   - **materials**: `λᵀ·A·u = Σ_f G_f·Δu_f·Δλ_f`, so
//!     `dJ/dG_f = −Δu_f·Δλ_f`, rolled up to cells through the face ←
//!     cell incidence weights the builders record
//!     ([`crate::grid::FaceWeights`] — the adjoint differentiates the
//!     *same* assembly the solver used, never a re-derivation), then to
//!     `dJ/dμ_r = Σ_cells (−1/(μ₀·μ_r²))·g_ν`.
//!
//! Every gradient here is validated against frozen-discretization
//! central differences in the tests (same grid on both sides of the
//! probe — the particle crate's lesson; the current-gradients are exact
//! to solver tolerance because J is linear in I, and dΛ_j/dI_k must
//! reproduce the mutual-inductance matrix).
//!
//! **Scope and honesty:** linear materials only — a saturable region's
//! converged secant ν depends on `u` through the Picard fixed point, so
//! the frozen-ν formula here is wrong for it, and asking for a material
//! gradient of a saturable region is a fail-closed error (finite
//! differences remain the honest route until the fixed-point adjoint
//! lands). Geometry gradients (region edges move the deposit masks) are
//! likewise FD-only, as in the particle crate's role split. Dirichlet
//! values are zero in both magnetostatic modules, which is what makes
//! the boundary terms of `dJ/dG_f` vanish.

use crate::axisym::{AxisymMagSolution, AxisymMagnetostatics};
use crate::constants::MU_0;
use crate::grid::{FvSystem, SolveError, SolveOptions, NO_CELL};
use crate::planar::{PlanarMagSolution, PlanarMagnetostatics};

/// Failures of an adjoint gradient computation.
#[derive(Debug, Clone, PartialEq)]
pub enum AdjointError {
    /// The inner adjoint solve failed.
    Solve(SolveError),
    /// A material-μ gradient was requested for a saturable region
    /// (index given): the frozen-ν adjoint is invalid there — use finite
    /// differences through `solve_nonlinear`.
    SaturableMaterial(usize),
    /// The system carried no face ← cell weights (not built by a
    /// magnetostatic builder).
    MissingFaceWeights,
}

impl std::fmt::Display for AdjointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdjointError::Solve(e) => write!(f, "adjoint solve failed: {e}"),
            AdjointError::SaturableMaterial(i) => write!(
                f,
                "material {i} is saturable: its secant ν depends on the field, \
                 the frozen-ν adjoint is invalid — use finite differences"
            ),
            AdjointError::MissingFaceWeights => {
                write!(
                    f,
                    "system has no face-cell weights (not a magnetostatic build)"
                )
            }
        }
    }
}

impl std::error::Error for AdjointError {}

impl From<SolveError> for AdjointError {
    fn from(e: SolveError) -> Self {
        AdjointError::Solve(e)
    }
}

/// Solve `A·λ = dj_du` on the system's operator (homogeneous Dirichlet).
fn adjoint_solve(
    sys: &FvSystem,
    dj_du: &[f64],
    opts: &SolveOptions,
) -> Result<Vec<f64>, SolveError> {
    let mut a = sys.clone();
    a.source.copy_from_slice(dj_du);
    for v in a.u0.iter_mut() {
        *v = 0.0;
    }
    Ok(a.solve(opts)?.u)
}

/// `dJ/dν` per material cell from `dJ/dG_f = −Δu_f·Δλ_f` and the
/// recorded incidence weights.
fn cell_nu_gradients(sys: &FvSystem, u: &[f64], lam: &[f64]) -> Result<Vec<f64>, AdjointError> {
    let w = sys
        .face_weights
        .as_ref()
        .ok_or(AdjointError::MissingFaceWeights)?;
    let g = &sys.grid;
    let n_cells =
        w.x.iter()
            .chain(w.y.iter())
            .flat_map(|p| p.iter())
            .filter(|(c, _)| *c != NO_CELL)
            .map(|(c, _)| c + 1)
            .max()
            .unwrap_or(0);
    let mut grad = vec![0.0; n_cells];
    let x_pairs = if g.periodic_x { g.nx } else { g.nx - 1 };
    for i in 0..x_pairs {
        let i1 = g.right(i);
        for j in 0..g.ny {
            let f = g.fx(i, j);
            let du = u[g.idx(i1, j)] - u[g.idx(i, j)];
            let dl = lam[g.idx(i1, j)] - lam[g.idx(i, j)];
            for (c, wf) in w.x[f] {
                if c != NO_CELL {
                    grad[c] -= wf * du * dl;
                }
            }
        }
    }
    for i in 0..g.nx {
        for j in 0..g.ny - 1 {
            let f = g.fy(i, j);
            let du = u[g.idx(i, j + 1)] - u[g.idx(i, j)];
            let dl = lam[g.idx(i, j + 1)] - lam[g.idx(i, j)];
            for (c, wf) in w.y[f] {
                if c != NO_CELL {
                    grad[c] -= wf * du * dl;
                }
            }
        }
    }
    Ok(grad)
}

/// Roll per-cell ν gradients up to per-material μ_r gradients:
/// `dν/dμ_r = −1/(μ₀·μ_r²)` on the material's cells. Fails closed on
/// saturable regions.
fn material_mu_gradients(
    cell_map: &[Option<usize>],
    mu_r: &[f64],
    saturable: &[bool],
    grad_nu: &[f64],
) -> Result<Vec<f64>, AdjointError> {
    let mut out = vec![0.0; mu_r.len()];
    for (cell, owner) in cell_map.iter().enumerate() {
        let Some(m) = owner else { continue };
        if saturable[*m] {
            return Err(AdjointError::SaturableMaterial(*m));
        }
        if cell < grad_nu.len() {
            out[*m] += grad_nu[cell] * (-1.0 / (MU_0 * mu_r[*m] * mu_r[*m]));
        }
    }
    Ok(out)
}

/// Gradients of one axisymmetric quantity of interest.
#[derive(Debug, Clone, PartialEq)]
pub struct AxisymGradients {
    /// The quantity itself.
    pub value: f64,
    /// dJ/d(coil current), one entry per device coil, per ampere.
    pub d_coil_currents: Vec<f64>,
    /// dJ/d(μ_r), one entry per device material region.
    pub d_material_mu_r: Vec<f64>,
}

/// Gradients of the flux linkage of coil `j` (so `d_coil_currents[k]`
/// is the inductance-matrix row `L_jk`, reproduced through the adjoint).
pub fn linkage_gradients(
    device: &AxisymMagnetostatics,
    solution: &AxisymMagSolution,
    j: usize,
    opts: &SolveOptions,
) -> Result<AxisymGradients, AdjointError> {
    let dj_du = solution.unit_sources[j].clone();
    let lam = adjoint_solve(&solution.system, &dj_du, opts)?;
    finish_axisym(device, solution, &lam, solution.flux_linkage(j), &[])
}

/// Gradients of the axial `J×B` force on coil `k` (newtons).
pub fn coil_force_gradients(
    device: &AxisymMagnetostatics,
    solution: &AxisymMagSolution,
    k: usize,
    opts: &SolveOptions,
) -> Result<AxisymGradients, AdjointError> {
    let g = &solution.system.grid;
    let i_k = solution.currents[k];
    // J = I_k·Σ_n U_k[n]·(D_z u)[n]; assemble I_k·D_zᵀ·U_k and the
    // force-per-amp for the explicit ∂J/∂I_k term.
    let u_k = &solution.unit_sources[k];
    let mut dj_du = vec![0.0; u_k.len()];
    let mut f_per_amp = 0.0;
    for i in 0..g.nx {
        for j in 0..g.ny {
            let w = u_k[g.idx(i, j)];
            if w == 0.0 {
                continue;
            }
            if j == 0 {
                dj_du[g.idx(i, 1)] += i_k * w / g.dy;
                dj_du[g.idx(i, 0)] -= i_k * w / g.dy;
                f_per_amp += w * (solution.psi[g.idx(i, 1)] - solution.psi[g.idx(i, 0)]) / g.dy;
            } else if j == g.ny - 1 {
                dj_du[g.idx(i, j)] += i_k * w / g.dy;
                dj_du[g.idx(i, j - 1)] -= i_k * w / g.dy;
                f_per_amp += w * (solution.psi[g.idx(i, j)] - solution.psi[g.idx(i, j - 1)]) / g.dy;
            } else {
                dj_du[g.idx(i, j + 1)] += i_k * w / (2.0 * g.dy);
                dj_du[g.idx(i, j - 1)] -= i_k * w / (2.0 * g.dy);
                f_per_amp += w * (solution.psi[g.idx(i, j + 1)] - solution.psi[g.idx(i, j - 1)])
                    / (2.0 * g.dy);
            }
        }
    }
    let lam = adjoint_solve(&solution.system, &dj_du, opts)?;
    let explicit = vec![(k, f_per_amp)];
    finish_axisym(device, solution, &lam, i_k * f_per_amp, &explicit)
}

fn finish_axisym(
    device: &AxisymMagnetostatics,
    solution: &AxisymMagSolution,
    lam: &[f64],
    value: f64,
    explicit_di: &[(usize, f64)],
) -> Result<AxisymGradients, AdjointError> {
    let mut d_i: Vec<f64> = solution
        .unit_sources
        .iter()
        .map(|u| u.iter().zip(lam).map(|(a, b)| a * b).sum())
        .collect();
    for (k, e) in explicit_di {
        d_i[*k] += e;
    }
    let grad_nu = cell_nu_gradients(&solution.system, &solution.psi, lam)?;
    let g = &solution.system.grid;
    let cell_map = device.material_cell_map(g.nx, g.ny);
    let mu_r: Vec<f64> = device.materials.iter().map(|m| m.mu_r).collect();
    let saturable: Vec<bool> = device.materials.iter().map(|m| m.sat.is_some()).collect();
    let d_mu = material_mu_gradients(&cell_map, &mu_r, &saturable, &grad_nu)?;
    Ok(AxisymGradients {
        value,
        d_coil_currents: d_i,
        d_material_mu_r: d_mu,
    })
}

/// Gradients of one planar quantity of interest.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanarGradients {
    /// The quantity itself (per meter of depth).
    pub value: f64,
    /// dJ/d(conductor total current), per ampere.
    pub d_conductor_currents: Vec<f64>,
    /// dJ/d(remanence scale) per magnet: the derivative w.r.t. a factor
    /// multiplying that magnet's (Br_x, Br_y), evaluated at 1.
    pub d_magnet_strength: Vec<f64>,
    /// dJ/d(μ_r) per material region.
    pub d_material_mu_r: Vec<f64>,
}

/// Gradients of the `J×B` torque on all magnets about `(cx_mm, cy_mm)`
/// (N·m per meter of depth) — the rotor-torque objective of an unrolled
/// or cross-section machine.
pub fn rotor_torque_gradients(
    device: &PlanarMagnetostatics,
    solution: &PlanarMagSolution,
    cx_mm: f64,
    cy_mm: f64,
    opts: &SolveOptions,
) -> Result<PlanarGradients, AdjointError> {
    let g = solution.system.grid.clone();
    let (cx, cy) = (cx_mm * 1e-3, cy_mm * 1e-3);
    // J = Σ_n D[n]·[(x−cx)·(∂_y u)_n − (y−cy)·(∂_x u)_n], D = Σ magnet
    // deposits. Assemble ∂J/∂u by transposing the node-gradient
    // stencils (periodic-aware), and keep per-magnet values for the
    // explicit strength terms.
    let mut total_dep = vec![0.0; g.nx * g.ny];
    for dep in &solution.magnet_sources {
        for (t, d) in total_dep.iter_mut().zip(dep) {
            *t += d;
        }
    }
    let mut dj_du = vec![0.0; g.nx * g.ny];
    let mut scatter = |id_hi: usize, id_lo: usize, coeff: f64| {
        dj_du[id_hi] += coeff;
        dj_du[id_lo] -= coeff;
    };
    for i in 0..g.nx {
        for j in 0..g.ny {
            let d = total_dep[g.idx(i, j)];
            if d == 0.0 {
                continue;
            }
            let ax = -(g.y(j) - cy) * d; // coefficient on (∂_x u)
            let ay = (g.x(i) - cx) * d; // coefficient on (∂_y u)
            if g.periodic_x {
                let ip = (i + 1) % g.nx;
                let im = (i + g.nx - 1) % g.nx;
                scatter(g.idx(ip, j), g.idx(im, j), ax / (2.0 * g.dx));
            } else if i == 0 {
                scatter(g.idx(1, j), g.idx(0, j), ax / g.dx);
            } else if i == g.nx - 1 {
                scatter(g.idx(i, j), g.idx(i - 1, j), ax / g.dx);
            } else {
                scatter(g.idx(i + 1, j), g.idx(i - 1, j), ax / (2.0 * g.dx));
            }
            if j == 0 {
                scatter(g.idx(i, 1), g.idx(i, 0), ay / g.dy);
            } else if j == g.ny - 1 {
                scatter(g.idx(i, j), g.idx(i, j - 1), ay / g.dy);
            } else {
                scatter(g.idx(i, j + 1), g.idx(i, j - 1), ay / (2.0 * g.dy));
            }
        }
    }
    let lam = adjoint_solve(&solution.system, &dj_du, opts)?;

    let value: f64 = (0..device.magnets.len())
        .map(|m| solution.torque_on_magnet(m, cx_mm, cy_mm))
        .sum();

    let d_i: Vec<f64> = solution
        .unit_sources
        .iter()
        .map(|u| u.iter().zip(&lam).map(|(a, b)| a * b).sum())
        .collect();

    // Strength scale s_m multiplies dep_m in BOTH the source vector and
    // the objective's own weights: dJ/ds_m = λᵀ·dep_m + T_m(u).
    let d_s: Vec<f64> = (0..device.magnets.len())
        .map(|m| {
            let implicit: f64 = solution.magnet_sources[m]
                .iter()
                .zip(&lam)
                .map(|(a, b)| a * b)
                .sum();
            implicit + solution.torque_on_magnet(m, cx_mm, cy_mm)
        })
        .collect();

    let grad_nu = cell_nu_gradients(&solution.system, &solution.a, &lam)?;
    let cell_map = device.material_cell_map(g.nx, g.ny);
    let mu_r: Vec<f64> = device.materials.iter().map(|m| m.mu_r).collect();
    let saturable: Vec<bool> = device.materials.iter().map(|m| m.sat.is_some()).collect();
    let d_mu = material_mu_gradients(&cell_map, &mu_r, &saturable, &grad_nu)?;
    Ok(PlanarGradients {
        value,
        d_conductor_currents: d_i,
        d_magnet_strength: d_s,
        d_material_mu_r: d_mu,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axisym::{Annulus, Coil, Material};
    use crate::planar::{Conductor, MagnetBlock, PlanarMaterial, Rect};

    fn tight() -> SolveOptions {
        SolveOptions {
            tol: 1e-11,
            ..SolveOptions::default()
        }
    }

    fn two_coil_iron_device() -> AxisymMagnetostatics {
        let mut dev = AxisymMagnetostatics::new(60.0, -40.0, 40.0);
        dev.coils.push(Coil {
            region: Annulus {
                r_inner_mm: 20.0,
                r_outer_mm: 24.0,
                z_min_mm: -12.0,
                z_max_mm: -8.0,
            },
            turns: 50.0,
            current_a: 2.0,
        });
        dev.coils.push(Coil {
            region: Annulus {
                r_inner_mm: 20.0,
                r_outer_mm: 24.0,
                z_min_mm: 8.0,
                z_max_mm: 12.0,
            },
            turns: 50.0,
            current_a: 1.5,
        });
        dev.materials.push(Material::linear(
            Annulus {
                r_inner_mm: 0.0,
                r_outer_mm: 10.0,
                z_min_mm: -30.0,
                z_max_mm: 30.0,
            },
            200.0,
        ));
        dev
    }

    #[test]
    fn face_weights_reconstruct_the_conductances_exactly() {
        // The incidence weights ARE the assembly: G_f = Σ w·ν must hold
        // to the last bit, or the adjoint differentiates a different
        // system than the solver solved.
        let dev = two_coil_iron_device();
        let nu = dev.initial_nu_cells(31, 41);
        let (sys, _) = dev.build_system(31, 41, &nu).unwrap();
        let w = sys.face_weights.as_ref().unwrap();
        for (f, pair) in w.x.iter().enumerate() {
            let rebuilt: f64 = pair
                .iter()
                .filter(|(c, _)| *c != NO_CELL)
                .map(|(c, wf)| wf * nu[*c])
                .sum();
            assert!(
                (rebuilt - sys.gx[f]).abs() <= 1e-12 * sys.gx[f].abs(),
                "gx[{f}] {} vs rebuilt {rebuilt}",
                sys.gx[f]
            );
        }
        for (f, pair) in w.y.iter().enumerate() {
            let rebuilt: f64 = pair
                .iter()
                .filter(|(c, _)| *c != NO_CELL)
                .map(|(c, wf)| wf * nu[*c])
                .sum();
            assert!(
                (rebuilt - sys.gy[f]).abs() <= 1e-12 * sys.gy[f].abs(),
                "gy[{f}] {} vs rebuilt {rebuilt}",
                sys.gy[f]
            );
        }
    }

    #[test]
    fn linkage_current_gradient_reproduces_the_inductance_matrix() {
        let dev = two_coil_iron_device();
        let sol = dev.solve(31, 41, &tight()).unwrap();
        let g = linkage_gradients(&dev, &sol, 0, &tight()).unwrap();
        let l = crate::axisym::inductance_matrix(&dev, 31, 41, &tight()).unwrap();
        for (k, l0k) in l[0].iter().enumerate() {
            let rel = (g.d_coil_currents[k] - l0k).abs() / l0k.abs();
            assert!(
                rel < 1e-6,
                "dΛ₀/dI_{k} = {:.8e} vs L[0][{k}] = {l0k:.8e} (rel {rel:.2e})",
                g.d_coil_currents[k]
            );
        }
        // And against frozen-grid central differences (J is linear in I,
        // so FD is exact to solver tolerance).
        let h = 1e-3;
        for k in 0..2 {
            let mut dp = dev.clone();
            dp.coils[k].current_a += h;
            let mut dm = dev.clone();
            dm.coils[k].current_a -= h;
            let fp = dp.solve(31, 41, &tight()).unwrap().flux_linkage(0);
            let fm = dm.solve(31, 41, &tight()).unwrap().flux_linkage(0);
            let fd = (fp - fm) / (2.0 * h);
            let rel = (g.d_coil_currents[k] - fd).abs() / fd.abs().max(1e-30);
            assert!(
                rel < 1e-6,
                "dΛ₀/dI_{k}: adjoint {:.8e} vs FD {fd:.8e}",
                g.d_coil_currents[k]
            );
        }
    }

    #[test]
    fn linkage_mu_gradient_matches_finite_differences() {
        let dev = two_coil_iron_device();
        let sol = dev.solve(31, 41, &tight()).unwrap();
        let g = linkage_gradients(&dev, &sol, 0, &tight()).unwrap();
        let h = 0.5; // μ_r step on 200
        let mut dp = dev.clone();
        dp.materials[0].mu_r += h;
        let mut dm = dev.clone();
        dm.materials[0].mu_r -= h;
        let fp = dp.solve(31, 41, &tight()).unwrap().flux_linkage(0);
        let fm = dm.solve(31, 41, &tight()).unwrap().flux_linkage(0);
        let fd = (fp - fm) / (2.0 * h);
        let rel = (g.d_material_mu_r[0] - fd).abs() / fd.abs();
        assert!(
            rel < 1e-3,
            "dΛ₀/dμ_r: adjoint {:.8e} vs FD {fd:.8e} (rel {rel:.2e})",
            g.d_material_mu_r[0]
        );
        // The gradient must be positive: more iron, more linkage.
        assert!(g.d_material_mu_r[0] > 0.0);
    }

    #[test]
    fn coil_force_gradients_match_finite_differences() {
        let dev = two_coil_iron_device();
        let sol = dev.solve(31, 41, &tight()).unwrap();
        let g = coil_force_gradients(&dev, &sol, 1, &tight()).unwrap();
        assert!(
            (g.value - sol.axial_force_on_coil(1)).abs() <= 1e-12 * g.value.abs(),
            "value must equal the forward force"
        );
        let force_of =
            |d: &AxisymMagnetostatics| d.solve(31, 41, &tight()).unwrap().axial_force_on_coil(1);
        // Currents (J is quadratic in I overall — central FD exact).
        let h = 1e-3;
        for k in 0..2 {
            let mut dp = dev.clone();
            dp.coils[k].current_a += h;
            let mut dm = dev.clone();
            dm.coils[k].current_a -= h;
            let fd = (force_of(&dp) - force_of(&dm)) / (2.0 * h);
            let rel = (g.d_coil_currents[k] - fd).abs() / fd.abs().max(1e-30);
            assert!(
                rel < 1e-5,
                "dF/dI_{k}: adjoint {:.8e} vs FD {fd:.8e} (rel {rel:.2e})",
                g.d_coil_currents[k]
            );
        }
        // Iron μ.
        let h = 0.5;
        let mut dp = dev.clone();
        dp.materials[0].mu_r += h;
        let mut dm = dev.clone();
        dm.materials[0].mu_r -= h;
        let fd = (force_of(&dp) - force_of(&dm)) / (2.0 * h);
        let rel = (g.d_material_mu_r[0] - fd).abs() / fd.abs();
        assert!(
            rel < 1e-3,
            "dF/dμ_r: adjoint {:.8e} vs FD {fd:.8e} (rel {rel:.2e})",
            g.d_material_mu_r[0]
        );
    }

    #[test]
    fn saturable_material_gradient_fails_closed() {
        let mut dev = two_coil_iron_device();
        dev.materials[0] = Material::saturable(dev.materials[0].region, 200.0, 0.45);
        let sol = dev.solve(31, 41, &tight()).unwrap();
        let err = linkage_gradients(&dev, &sol, 0, &tight()).unwrap_err();
        assert_eq!(err, AdjointError::SaturableMaterial(0));
    }

    /// Deliberately asymmetric (a first version was mirror-symmetric,
    /// its total torque was ~0, and both gradient routes compared
    /// solver-tolerance dust — a comparison must sit on a quantity of
    /// honest size).
    fn mini_machine() -> PlanarMagnetostatics {
        let mut dev = PlanarMagnetostatics::new(0.0, 40.0, 0.0, 30.0);
        dev.magnets.push(MagnetBlock {
            region: Rect {
                x_min_mm: 13.0,
                x_max_mm: 23.0,
                y_min_mm: 18.0,
                y_max_mm: 22.0,
            },
            br_x_t: 0.0,
            br_y_t: 0.8,
            mu_r: 1.05,
        });
        dev.conductors.push(Conductor {
            region: Rect {
                x_min_mm: 12.0,
                x_max_mm: 18.0,
                y_min_mm: 8.0,
                y_max_mm: 10.0,
            },
            total_current_a: 5.0,
        });
        dev.conductors.push(Conductor {
            region: Rect {
                x_min_mm: 22.0,
                x_max_mm: 28.0,
                y_min_mm: 8.0,
                y_max_mm: 10.0,
            },
            total_current_a: -3.0,
        });
        dev.materials.push(PlanarMaterial::linear(
            Rect {
                x_min_mm: 0.0,
                x_max_mm: 40.0,
                y_min_mm: 0.0,
                y_max_mm: 6.0,
            },
            300.0,
        ));
        dev
    }

    #[test]
    fn rotor_torque_gradients_match_finite_differences() {
        let dev = mini_machine();
        let sol = dev.solve(41, 31, &tight()).unwrap();
        let g = rotor_torque_gradients(&dev, &sol, 18.0, 14.0, &tight()).unwrap();
        let torque_of = |d: &PlanarMagnetostatics| {
            let s = d.solve(41, 31, &tight()).unwrap();
            (0..d.magnets.len())
                .map(|m| s.torque_on_magnet(m, 18.0, 14.0))
                .sum::<f64>()
        };
        assert!(
            (g.value - torque_of(&dev)).abs() <= 1e-9 * g.value.abs().max(1e-12),
            "value mismatch"
        );
        // Conductor currents.
        let h = 1e-3;
        for k in 0..2 {
            let mut dp = dev.clone();
            dp.conductors[k].total_current_a += h;
            let mut dm = dev.clone();
            dm.conductors[k].total_current_a -= h;
            let fd = (torque_of(&dp) - torque_of(&dm)) / (2.0 * h);
            let rel = (g.d_conductor_currents[k] - fd).abs() / fd.abs().max(1e-30);
            assert!(
                rel < 1e-5,
                "dT/dI_{k}: adjoint {:.8e} vs FD {fd:.8e} (rel {rel:.2e})",
                g.d_conductor_currents[k]
            );
        }
        // Magnet strength scale (J is quadratic in the scale — central FD
        // is exact).
        let h = 1e-4;
        let scale = |d: &PlanarMagnetostatics, s: f64| {
            let mut d2 = d.clone();
            d2.magnets[0].br_y_t *= s;
            torque_of(&d2)
        };
        let fd = (scale(&dev, 1.0 + h) - scale(&dev, 1.0 - h)) / (2.0 * h);
        let rel = (g.d_magnet_strength[0] - fd).abs() / fd.abs().max(1e-30);
        assert!(
            rel < 1e-5,
            "dT/ds: adjoint {:.8e} vs FD {fd:.8e} (rel {rel:.2e})",
            g.d_magnet_strength[0]
        );
        // Iron μ.
        let h = 1.0;
        let mut dp = dev.clone();
        dp.materials[0].mu_r += h;
        let mut dm = dev.clone();
        dm.materials[0].mu_r -= h;
        let fd = (torque_of(&dp) - torque_of(&dm)) / (2.0 * h);
        let rel = (g.d_material_mu_r[0] - fd).abs() / fd.abs();
        assert!(
            rel < 2e-3,
            "dT/dμ_r: adjoint {:.8e} vs FD {fd:.8e} (rel {rel:.2e})",
            g.d_material_mu_r[0]
        );
    }
}
