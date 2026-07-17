//! Time-harmonic (phasor) magnetics: eddy currents, AC impedance,
//! skin-depth physics.
//!
//! At angular frequency ω, conducting regions add the induced current
//! `J_eddy = −jωσA` to the magnetostatic equation, giving the complex
//! system
//!
//! ```text
//!   planar:  ∇·(ν ∇Â)      − jωσ·Â   = −Ĵ_src
//!   axisym:  ∇·(ν/r ∇ψ̂)   − jωσ/r·ψ̂ = −Ĵ_src
//! ```
//!
//! discretized as the M0 real system plus a per-node imaginary diagonal
//! `D_n` (the σ-weighted control-volume measure — the same exact-overlap
//! machinery as the sources). The complex SOR sweep reuses the real
//! stencil; complex arithmetic is carried as explicit (re, im) pairs.
//!
//! Outputs: complex flux linkage per coil, series impedance
//! `Z = jωΛ/I` (AC inductance `Im(Z)/ω` and eddy-referred resistance
//! `Re(Z)`), and the volume eddy loss `P = ½ω²·Σ D·|u|²` — which must
//! agree with the circuit form `½·Re(Z)·I²`, and the tests check it.
//!
//! **Scope and honesty:** linear materials only (a phasor solve cannot
//! carry a B–H curve — saturation with AC is harmonic balance, out of
//! scope); source coils are filamentary drives with **no conductor
//! resistance and no skin effect inside the winding itself** (add the DC
//! resistance externally); single frequency per solve; no motion, no
//! hysteresis loss.

use crate::axisym::{Annulus, AxisymMagnetostatics};
use crate::grid::{FvSystem, SolveError, SolveOptions};
use crate::planar::{PlanarMagnetostatics, Rect};

/// A conducting region for eddy currents, axisymmetric geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisymSigma {
    /// Region it occupies.
    pub region: Annulus,
    /// Conductivity, S/m.
    pub sigma_s_m: f64,
}

/// A conducting region for eddy currents, planar geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanarSigma {
    /// Region it occupies.
    pub region: Rect,
    /// Conductivity, S/m.
    pub sigma_s_m: f64,
}

/// A converged phasor solution.
#[derive(Debug, Clone, PartialEq)]
pub struct AcSolution {
    /// The real part of the assembled system (conductances, sources).
    pub system: FvSystem,
    /// Solution real part per node.
    pub u_re: Vec<f64>,
    /// Solution imaginary part per node.
    pub u_im: Vec<f64>,
    /// The σ-weighted diagonal per node (multiplies ω on the imaginary
    /// axis).
    pub diag: Vec<f64>,
    /// Angular frequency, rad/s.
    pub omega: f64,
    /// SOR sweeps used.
    pub sweeps: usize,
    /// Final relative residual.
    pub residual: f64,
    /// Per-coil unit source vectors (linkage functionals).
    pub unit_sources: Vec<Vec<f64>>,
    /// Coil drive currents (the phase reference — real).
    pub currents: Vec<f64>,
}

impl AcSolution {
    /// Complex flux linkage of coil `k`, weber-turns: `(Re Λ, Im Λ)`.
    pub fn flux_linkage(&self, k: usize) -> (f64, f64) {
        let u = &self.unit_sources[k];
        let re = u.iter().zip(&self.u_re).map(|(a, b)| a * b).sum();
        let im = u.iter().zip(&self.u_im).map(|(a, b)| a * b).sum();
        (re, im)
    }

    /// Series impedance of coil `k`: `Z = jωΛ/I` → `(R_eddy, X)` ohms.
    /// The eddy-referred resistance only — the winding's own DC
    /// resistance is not modeled here.
    pub fn impedance(&self, k: usize) -> (f64, f64) {
        let (lr, li) = self.flux_linkage(k);
        let i = self.currents[k];
        (-self.omega * li / i, self.omega * lr / i)
    }

    /// AC inductance of coil `k`, henries: `Im(Z)/ω = Re(Λ)/I`.
    pub fn inductance(&self, k: usize) -> f64 {
        self.flux_linkage(k).0 / self.currents[k]
    }

    /// Time-averaged eddy-current loss, watts (axisymmetric) or W/m
    /// (planar): `½ω²·Σ D·|u|²`.
    pub fn eddy_loss(&self) -> f64 {
        0.5 * self.omega
            * self.omega
            * self
                .diag
                .iter()
                .zip(self.u_re.iter().zip(&self.u_im))
                .map(|(d, (re, im))| d * (re * re + im * im))
                .sum::<f64>()
    }

    /// Complex field value at a point (bilinear on both parts).
    pub fn value_at(&self, x_m: f64, y_m: f64) -> (f64, f64) {
        let g = &self.system.grid;
        (
            g.value_at(&self.u_re, x_m, y_m),
            g.value_at(&self.u_im, x_m, y_m),
        )
    }
}

/// Complex SOR on the real system `sys` plus the imaginary diagonal
/// `ω·diag`: per free node, `u = (Σ G·u_nbr + S) / (Σ G + jω·D)`.
fn solve_complex(
    sys: &FvSystem,
    diag: &[f64],
    omega: f64,
    opts: &SolveOptions,
) -> Result<(Vec<f64>, Vec<f64>, usize, f64), SolveError> {
    let g = &sys.grid;
    if g.nx < 3 || g.ny < 3 {
        return Err(SolveError::GridTooSmall);
    }
    // Complex systems default to Gauss–Seidel (ω = 1), NOT the Chebyshev
    // over-relaxation: at the optimal real-SOR ω the iteration matrix is
    // defective, and even a small imaginary diagonal (measured: 1.4% of
    // Σ G) perturbs its coalesced eigenvalues like √ε — past 1, and the
    // solve diverges. ω = 1 is guaranteed by the weak diagonal dominance
    // |Σ G + jωD| ≥ Σ G. Override via `opts.omega` at your own risk.
    let relax = if opts.omega > 0.0 { opts.omega } else { 1.0 };
    let mut re = sys.u0.clone();
    let mut im = vec![0.0; re.len()];
    let mut scale = re.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
    let mut rel = f64::MAX;
    let mut sweeps = 0;
    while sweeps < opts.max_sweeps {
        let mut residual = 0.0_f64;
        for i in 0..g.nx {
            for j in 0..g.ny {
                let id = g.idx(i, j);
                if sys.fixed[id] {
                    continue;
                }
                let mut num_re = sys.source[id];
                let mut num_im = 0.0;
                let mut den = 0.0;
                let mut acc = |gf: f64, nbr: usize| {
                    num_re += gf * re[nbr];
                    num_im += gf * im[nbr];
                    den += gf;
                };
                if i > 0 {
                    acc(sys.gx[g.fx(i - 1, j)], g.idx(i - 1, j));
                } else if g.periodic_x {
                    acc(sys.gx[g.fx(g.nx - 1, j)], g.idx(g.nx - 1, j));
                }
                if i + 1 < g.nx {
                    acc(sys.gx[g.fx(i, j)], g.idx(i + 1, j));
                } else if g.periodic_x {
                    acc(sys.gx[g.fx(i, j)], g.idx(0, j));
                }
                if j > 0 {
                    acc(sys.gy[g.fy(i, j - 1)], g.idx(i, j - 1));
                }
                if j + 1 < g.ny {
                    acc(sys.gy[g.fy(i, j)], g.idx(i, j + 1));
                }
                if den == 0.0 {
                    continue;
                }
                // (num_re + j·num_im) / (den + jω·D)
                let d_im = omega * diag[id];
                let m2 = den * den + d_im * d_im;
                let upd_re = (num_re * den + num_im * d_im) / m2;
                let upd_im = (num_im * den - num_re * d_im) / m2;
                let d_re = upd_re - re[id];
                let d_im2 = upd_im - im[id];
                re[id] += relax * d_re;
                im[id] += relax * d_im2;
                let ad = (d_re * d_re + d_im2 * d_im2).sqrt();
                // NaN fails closed: a poisoned update must read as
                // "maximally unconverged", never as converged (NaN
                // comparisons are all false, which once let a diverged
                // solve report success with infinite fields).
                if !ad.is_finite() {
                    residual = f64::MAX;
                } else if ad > residual {
                    residual = ad;
                }
                let au = (re[id] * re[id] + im[id] * im[id]).sqrt();
                if au > scale {
                    scale = au;
                }
            }
        }
        sweeps += 1;
        rel = if scale > 0.0 {
            residual / scale
        } else if residual == 0.0 {
            0.0
        } else {
            f64::MAX
        };
        if rel < opts.tol {
            break;
        }
    }
    if rel >= opts.tol {
        return Err(SolveError::NotConverged {
            residual: rel,
            sweeps,
        });
    }
    Ok((re, im, sweeps, rel))
}

#[inline]
fn overlap(a_lo: f64, a_hi: f64, b_lo: f64, b_hi: f64) -> f64 {
    (a_hi.min(b_hi) - a_lo.max(b_lo)).max(0.0)
}

/// Phasor solve of an axisymmetric device with conducting regions at
/// angular frequency `omega_rad_s`. Linear materials only (saturating
/// materials are frozen at their initial slope — stated in the module
/// docs).
pub fn solve_axisym_ac(
    device: &AxisymMagnetostatics,
    sigmas: &[AxisymSigma],
    omega_rad_s: f64,
    nr: usize,
    nz: usize,
    opts: &SolveOptions,
) -> Result<AcSolution, SolveError> {
    let (sys, unit_sources) = device.build_system(nr, nz, &device.initial_nu_cells(nr, nz))?;
    let g = sys.grid.clone();
    let two_pi = 2.0 * std::f64::consts::PI;
    // D_n = σ·2π·(∫ dr/r over CV∩region)·(z overlap): the eddy term of
    // the ψ formulation is −jω(σ/r)ψ, weighted by the same 2π measure as
    // the sources. The axis column is Dirichlet, so its divergent
    // integral is never used.
    let mut diag = vec![0.0; g.nx * g.ny];
    for s in sigmas {
        let (r_lo_mm, r_hi_mm) = (s.region.r_inner_mm, s.region.r_outer_mm);
        let (z_lo, z_hi) = (s.region.z_min_mm * 1e-3, s.region.z_max_mm * 1e-3);
        for i in 1..g.nx {
            let cv_rl = (g.x(i) - 0.5 * g.dx).max(1e-12);
            let cv_rh = g.x(i) + 0.5 * g.dx;
            let rl = cv_rl.max(r_lo_mm * 1e-3);
            let rh = cv_rh.min(r_hi_mm * 1e-3);
            if rh <= rl {
                continue;
            }
            let log_int = (rh / rl).ln();
            for j in 0..g.ny {
                let cv_zl = g.y(j) - 0.5 * g.dy;
                let cv_zh = g.y(j) + 0.5 * g.dy;
                let wz = overlap(cv_zl, cv_zh, z_lo, z_hi);
                if wz > 0.0 {
                    diag[g.idx(i, j)] += s.sigma_s_m * two_pi * log_int * wz;
                }
            }
        }
    }
    let (u_re, u_im, sweeps, residual) = solve_complex(&sys, &diag, omega_rad_s, opts)?;
    Ok(AcSolution {
        currents: device.coils.iter().map(|c| c.current_a).collect(),
        system: sys,
        u_re,
        u_im,
        diag,
        omega: omega_rad_s,
        sweeps,
        residual,
        unit_sources,
    })
}

/// Phasor solve of a planar device with conducting regions at angular
/// frequency `omega_rad_s`. Results per meter of depth.
pub fn solve_planar_ac(
    device: &PlanarMagnetostatics,
    sigmas: &[PlanarSigma],
    omega_rad_s: f64,
    nx: usize,
    ny: usize,
    opts: &SolveOptions,
) -> Result<AcSolution, SolveError> {
    let (sys, unit_sources, _mag) =
        device.build_system(nx, ny, &device.initial_nu_cells(nx, ny))?;
    let g = sys.grid.clone();
    let mut diag = vec![0.0; g.nx * g.ny];
    for s in sigmas {
        let (x_lo, x_hi) = (s.region.x_min_mm * 1e-3, s.region.x_max_mm * 1e-3);
        let (y_lo, y_hi) = (s.region.y_min_mm * 1e-3, s.region.y_max_mm * 1e-3);
        for i in 0..g.nx {
            let wx = overlap(g.x(i) - 0.5 * g.dx, g.x(i) + 0.5 * g.dx, x_lo, x_hi);
            if wx == 0.0 {
                continue;
            }
            for j in 0..g.ny {
                let wy = overlap(g.y(j) - 0.5 * g.dy, g.y(j) + 0.5 * g.dy, y_lo, y_hi);
                if wy > 0.0 {
                    diag[g.idx(i, j)] += s.sigma_s_m * wx * wy;
                }
            }
        }
    }
    let (u_re, u_im, sweeps, residual) = solve_complex(&sys, &diag, omega_rad_s, opts)?;
    Ok(AcSolution {
        currents: device
            .conductors
            .iter()
            .map(|c| c.total_current_a)
            .collect(),
        system: sys,
        u_re,
        u_im,
        diag,
        omega: omega_rad_s,
        sweeps,
        residual,
        unit_sources,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::MU_0;
    use crate::grid::Bc;
    use crate::planar::Conductor;

    /// Sheet drive above a thick conducting slab, effectively 1D.
    fn slab_setup() -> (PlanarMagnetostatics, PlanarSigma) {
        let mut dev = PlanarMagnetostatics::new(0.0, 10.0, 0.0, 40.0);
        dev.bc_x_low = Bc::Neumann;
        dev.bc_x_high = Bc::Neumann;
        dev.conductors.push(Conductor {
            region: Rect {
                x_min_mm: 0.0,
                x_max_mm: 10.0,
                y_min_mm: 2.0,
                y_max_mm: 3.0,
            },
            total_current_a: 10.0,
        });
        let slab = PlanarSigma {
            region: Rect {
                x_min_mm: 0.0,
                x_max_mm: 10.0,
                y_min_mm: 10.0,
                y_max_mm: 40.0,
            },
            sigma_s_m: 3.5e7,
        };
        (dev, slab)
    }

    #[test]
    fn slab_reproduces_the_analytic_skin_depth() {
        // δ = √(2/(ω·μ₀·σ)) = 3.000 mm at ω = 2/(μ₀·σ·δ²). Inside a
        // thick slab the field decays as exp(−(1+j)·d/δ): magnitude e⁻¹
        // and phase −1 rad per δ of depth. Griffiths 4th ed. §9.4.
        let delta = 3.0e-3;
        let sigma = 3.5e7;
        let omega = 2.0 / (MU_0 * sigma * delta * delta);
        let (dev, slab) = slab_setup();
        let sol = solve_planar_ac(&dev, &[slab], omega, 5, 81, &SolveOptions::default()).unwrap();
        let probe = |depth_mm: f64| sol.value_at(0.005, (10.0 + depth_mm) * 1e-3);
        let (r0, i0) = probe(1.5);
        let mag0 = (r0 * r0 + i0 * i0).sqrt();
        let ph0 = i0.atan2(r0);
        for steps in [1.0_f64, 2.0] {
            let (r, i) = probe(1.5 + steps * 3.0);
            let mag = (r * r + i * i).sqrt();
            let ratio = mag / mag0;
            let expect = (-steps).exp();
            assert!(
                ((ratio - expect) / expect).abs() < 0.03,
                "decay over {steps}δ: {ratio:.4} vs {expect:.4}"
            );
            let mut dph = i.atan2(r) - ph0;
            while dph > std::f64::consts::PI {
                dph -= 2.0 * std::f64::consts::PI;
            }
            while dph < -std::f64::consts::PI {
                dph += 2.0 * std::f64::consts::PI;
            }
            assert!(
                (dph + steps).abs() < 0.05 * steps.max(1.0),
                "phase lag over {steps}δ: {dph:.4} vs {:.4}",
                -steps
            );
        }
    }

    #[test]
    fn eddy_loss_agrees_between_field_and_circuit_forms() {
        let delta = 3.0e-3;
        let sigma = 3.5e7;
        let omega = 2.0 / (MU_0 * sigma * delta * delta);
        let (dev, slab) = slab_setup();
        let sol = solve_planar_ac(&dev, &[slab], omega, 5, 81, &SolveOptions::default()).unwrap();
        let p_field = sol.eddy_loss();
        let (r_eddy, _) = sol.impedance(0);
        let p_circuit = 0.5 * r_eddy * 10.0 * 10.0;
        assert!(p_field > 0.0);
        let rel = ((p_field - p_circuit) / p_field).abs();
        assert!(
            rel < 1e-4,
            "loss mismatch: field {p_field:.6e} vs circuit {p_circuit:.6e} ({rel:.2e})"
        );
    }

    #[test]
    fn low_frequency_limits_are_correct() {
        // ω → 0: L_ac → L_dc; R_eddy scales as ω² (induced currents are
        // linear in ω, loss quadratic).
        let (dev, slab) = slab_setup();
        let l_dc = dev
            .solve(5, 81, &SolveOptions::default())
            .unwrap()
            .unit_sources[0]
            .iter()
            .zip(&dev.solve(5, 81, &SolveOptions::default()).unwrap().a)
            .map(|(u, a)| u * a)
            .sum::<f64>()
            / 10.0;
        let w1 = 5.0;
        let s1 = solve_planar_ac(&dev, &[slab], w1, 5, 81, &SolveOptions::default()).unwrap();
        let s2 = solve_planar_ac(&dev, &[slab], 2.0 * w1, 5, 81, &SolveOptions::default()).unwrap();
        let l1 = s1.inductance(0);
        assert!(
            ((l1 - l_dc) / l_dc).abs() < 5e-3,
            "L_ac(ω→0) = {l1:.5e} vs L_dc {l_dc:.5e}"
        );
        let (r1, _) = s1.impedance(0);
        let (r2, _) = s2.impedance(0);
        let ratio = r2 / r1;
        assert!(
            (ratio - 4.0).abs() < 0.12,
            "R_eddy must scale as ω²: ratio {ratio:.3}"
        );
    }
}
