//! Discrete adjoint of the traced yield objective.
//!
//! Computes exact-to-discretization gradients of the ensemble-mean D-D
//! yield integral J = ⟨∫ σ_DDn(E)·v dt⟩ with respect to
//!
//! - **electrode potentials** (per ring + chamber wall), via reverse-mode
//!   accumulation into the potential grid (PIC-style deposits through the
//!   bilinear-patch weights) followed by one adjoint Poisson solve — the
//!   axisymmetric operator is self-adjoint under the radial weighting
//!   `w_i = r_i` (axis row: `w₀ = Δr/8`), so the adjoint solve reuses the
//!   forward SOR stencil with a right-hand side;
//! - **coil ampere-turns**, via ⟨λ_B, B(I=1)⟩ along the trajectory (the
//!   loop field is linear in its current).
//!
//! To keep the adjoint exactly consistent with its forward pass, this
//! module runs its **own** forward integration: fixed time step (no
//! speed/proximity adaptivity — adaptive dt is a non-differentiated
//! control flow), analytic coil B (no grid cache), full trajectory
//! storage, and a pure time budget (no wire/wall termination, so the
//! objective has no non-differentiable boundary terms). Position
//! Jacobians of the sampled fields are taken by central differences at
//! the particle (the fields themselves stay exact); everything linear in
//! φ and I is handled analytically.
//!
//! Validated against central differences over the full pipeline — see the
//! tests at the bottom.

use crate::device::Device;
use crate::field::{b_ring, RingCoil};
use crate::poisson::{Solution, SolveError, SolveOptions};
use crate::trace::{Species, DEUTERON};
use crate::xsection::dd_n_sigma_m2;

/// Configuration for adjoint gradient runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdjointConfig {
    /// Fixed step as a fraction of the grid spacing at the reference
    /// speed (smaller than the tracer default — the fixed step must stay
    /// accurate near wires without adaptivity).
    pub dt_fraction: f64,
    /// Flight-time budget in ideal chamber crossings (the only stopping
    /// rule — keeps J smooth).
    pub time_budget_crossings: f64,
    /// Ensemble size (deterministic launch grid, same as the tracer).
    pub n_particles: usize,
    /// Launch shell radius as a fraction of the smaller chamber dimension.
    pub launch_shell_fraction: f64,
    /// Launch directions span cos θ ∈ [−this, +this].
    pub launch_cos_max: f64,
    /// Reference potential drop (volts) that sets the fixed time step and
    /// time budget. `None` derives it from the device — fine for a single
    /// gradient, but **freeze it explicitly** when comparing runs across
    /// parameter perturbations (finite differences, line searches), or the
    /// integration window itself becomes a function of the parameters and
    /// contaminates the comparison.
    pub reference_drop_v: Option<f64>,
}

impl Default for AdjointConfig {
    fn default() -> Self {
        Self {
            dt_fraction: 0.1,
            time_budget_crossings: 3.0,
            n_particles: 16,
            launch_shell_fraction: 0.85,
            launch_cos_max: 0.95,
            reference_drop_v: None,
        }
    }
}

/// Yield value and its gradient.
#[derive(Debug, Clone, PartialEq)]
pub struct YieldGradient {
    /// Ensemble-mean yield integral ⟨∫σv dt⟩, m³.
    pub value: f64,
    /// d value / d (ring potential), one entry per `Device::rings`.
    pub d_ring_potentials: Vec<f64>,
    /// d value / d (wall potential).
    pub d_wall_potential: f64,
    /// d value / d (ring ampere-turns), one entry per `Device::rings`.
    pub d_ampere_turns: Vec<f64>,
}

#[derive(Clone, Copy)]
struct Step {
    p: [f64; 3],
    v: [f64; 3],
}

/// Compute the ensemble yield and its adjoint gradient for `device` on an
/// `nr × nz` grid.
pub fn yield_gradient(
    device: &Device,
    nr: usize,
    nz: usize,
    sopts: &SolveOptions,
    cfg: &AdjointConfig,
) -> Result<YieldGradient, SolveError> {
    let sol = crate::poisson::solve(device, nr, nz, sopts)?;
    let coils: Vec<RingCoil> = device
        .rings
        .iter()
        .map(|r| RingCoil {
            radius_m: r.ring_radius_mm * 1e-3,
            z_m: r.z_mm * 1e-3,
            ampere_turns: r.ampere_turns,
            wire_radius_m: (r.wire_radius_mm * 1e-3).max(1e-6),
        })
        .collect();

    let species = DEUTERON;
    let qm = species.charge_c / species.mass_kg;
    let drop_v = cfg
        .reference_drop_v
        .unwrap_or_else(|| device.max_potential_drop_v())
        .max(1.0);
    let v_ref = (2.0 * species.charge_c * drop_v / species.mass_kg).sqrt();
    let h = sol.dr.min(sol.dz);
    let dth = cfg.dt_fraction * h / v_ref;
    let min_dim = device.chamber_radius_mm.min(device.chamber_half_height_mm) * 1e-3;
    let shell = cfg.launch_shell_fraction * min_dim;
    let t_max = cfg.time_budget_crossings * 2.0 * shell / v_ref;
    let n_steps = (t_max / dth).ceil() as usize;

    let mut value = 0.0_f64;
    let mut lam_phi = vec![0.0_f64; sol.nr * sol.nz];
    let mut d_turns = vec![0.0_f64; device.rings.len()];

    let n = cfg.n_particles.max(2);
    for k in 0..n {
        let c = -cfg.launch_cos_max + 2.0 * cfg.launch_cos_max * k as f64 / (n - 1) as f64;
        let s = (1.0 - c * c).max(0.0).sqrt();
        let start = Step {
            p: [shell * s, 0.0, shell * c],
            v: [0.0, 0.0, 0.0],
        };

        // Forward with storage.
        let mut traj = Vec::with_capacity(n_steps);
        let mut st = start;
        for _ in 0..n_steps {
            traj.push(st);
            st = boris_step(&sol, &coils, qm, dth, st);
            let vv = dot(st.v, st.v);
            value += yield_rate(species, vv) * dth;
        }

        // Reverse.
        let mut lp = [0.0_f64; 3];
        let mut lv = [0.0_f64; 3];
        for st in traj.iter().rev() {
            let after = boris_step(&sol, &coils, qm, dth, *st);
            // Running cost g(v⁺) = σ(E(v⁺))·|v⁺| accrued this step.
            let gv = yield_rate_grad(species, after.v);
            lv[0] += dth * gv[0];
            lv[1] += dth * gv[1];
            lv[2] += dth * gv[2];
            let (nlp, nlv) = reverse_step(
                &sol,
                &coils,
                qm,
                dth,
                *st,
                lp,
                lv,
                &mut lam_phi,
                &mut d_turns,
            );
            lp = nlp;
            lv = nlv;
        }
    }
    let inv_n = 1.0 / n as f64;
    value *= inv_n;
    for l in lam_phi.iter_mut() {
        *l *= inv_n;
    }
    for d in d_turns.iter_mut() {
        *d *= inv_n;
    }

    // Split deposits into free-node sources and direct fixed-node terms,
    // run the adjoint Poisson solve, and assemble potential gradients.
    let membership = electrode_membership(device, &sol);
    let psi = adjoint_poisson(&sol, &lam_phi, sopts);

    let mut d_ring = vec![0.0_f64; device.rings.len()];
    let mut d_wall = 0.0_f64;
    let idx = |i: usize, j: usize| i * sol.nz + j;
    for i in 0..sol.nr {
        for j in 0..sol.nz {
            let id = idx(i, j);
            if !sol.fixed[id] {
                continue;
            }
            // Direct term: yield sampled this fixed node's potential.
            let mut total = lam_phi[id];
            // Indirect term: this fixed node sources adjacent free rows.
            for (ni, nj) in neighbors(i, j, sol.nr, sol.nz) {
                let nid = idx(ni, nj);
                if sol.fixed[nid] {
                    continue;
                }
                total += psi[nid] * stencil_coeff(&sol, ni, nj, i, j);
            }
            match membership[id] {
                Some(ring) => d_ring[ring] += total,
                None => d_wall += total,
            }
        }
    }

    Ok(YieldGradient {
        value,
        d_ring_potentials: d_ring,
        d_wall_potential: d_wall,
        d_ampere_turns: d_turns,
    })
}

#[inline]
fn yield_rate(species: Species, v2: f64) -> f64 {
    let e_lab_kev = 0.5 * species.mass_kg * v2 / (crate::constants::ELEMENTARY_CHARGE * 1.0e3);
    let sig = dd_n_sigma_m2(0.5 * e_lab_kev);
    if sig > 0.0 {
        sig * v2.sqrt()
    } else {
        0.0
    }
}

/// ∂(σ(E(v))·|v|)/∂v by central differences on the closed form (cheap and
/// robust across the σ validity floor).
fn yield_rate_grad(species: Species, v: [f64; 3]) -> [f64; 3] {
    let mut g = [0.0; 3];
    let speed = dot(v, v).sqrt().max(1.0);
    let hstep = 1e-6 * speed;
    for i in 0..3 {
        let mut vp = v;
        vp[i] += hstep;
        let mut vm = v;
        vm[i] -= hstep;
        g[i] =
            (yield_rate(species, dot(vp, vp)) - yield_rate(species, dot(vm, vm))) / (2.0 * hstep);
    }
    g
}

fn e_cart(sol: &Solution, p: [f64; 3]) -> [f64; 3] {
    let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
    let (er, ez) = sol.e_at(r, p[2]);
    if r < 1e-12 {
        return [0.0, 0.0, ez];
    }
    [er * p[0] / r, er * p[1] / r, ez]
}

fn b_cart_analytic(coils: &[RingCoil], p: [f64; 3]) -> [f64; 3] {
    if coils.is_empty() {
        return [0.0, 0.0, 0.0];
    }
    let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
    let mut br = 0.0;
    let mut bz = 0.0;
    for c in coils {
        let (cr, cz) = b_ring(c, r, p[2]);
        br += cr;
        bz += cz;
    }
    if r < 1e-12 {
        return [0.0, 0.0, bz];
    }
    [br * p[0] / r, br * p[1] / r, bz]
}

fn boris_step(sol: &Solution, coils: &[RingCoil], qm: f64, dth: f64, st: Step) -> Step {
    let e = e_cart(sol, st.p);
    let b = b_cart_analytic(coils, st.p);
    let half = 0.5 * qm * dth;
    let vm = add(st.v, scale(e, half));
    let t = scale(b, half);
    let t2 = dot(t, t);
    let vprime = add(vm, cross(vm, t));
    let s = 2.0 / (1.0 + t2);
    let vp = add(vm, scale(cross(vprime, t), s));
    let v_new = add(vp, scale(e, half));
    Step {
        p: add(st.p, scale(v_new, dth)),
        v: v_new,
    }
}

/// Reverse one Boris step: pull (λp⁺, λv⁺) at the output back to the
/// input, accumulating field adjoints into `lam_phi` (potential-node
/// deposits) and `d_turns` (per-coil ampere-turn gradients).
#[allow(clippy::too_many_arguments)]
fn reverse_step(
    sol: &Solution,
    coils: &[RingCoil],
    qm: f64,
    dth: f64,
    st: Step,
    lp_out: [f64; 3],
    lv_out: [f64; 3],
    lam_phi: &mut [f64],
    d_turns: &mut [f64],
) -> ([f64; 3], [f64; 3]) {
    // Recompute forward intermediates.
    let e = e_cart(sol, st.p);
    let b = b_cart_analytic(coils, st.p);
    let half = 0.5 * qm * dth;
    let vm = add(st.v, scale(e, half));
    let t = scale(b, half);
    let t2 = dot(t, t);
    let vprime = add(vm, cross(vm, t));
    let s = 2.0 / (1.0 + t2);

    // p⁺ = p + v⁺·dth.
    let mut lam_vnew = lv_out;
    lam_vnew = add(lam_vnew, scale(lp_out, dth));
    let lam_p_partial = lp_out;

    // v⁺ = vp + half·e.
    let lam_vp = lam_vnew;
    let mut lam_e = scale(lam_vnew, half);

    // vp = vm + s·(v'×t).
    let mut lam_vm = lam_vp;
    let mut lam_vprime = scale(cross(t, lam_vp), s);
    let mut lam_t = scale(cross(lam_vp, vprime), s);
    let lam_s = dot(lam_vp, cross(vprime, t));
    // ds = −s²·(t·dt).
    lam_t = add(lam_t, scale(t, -lam_s * s * s));

    // v' = vm + vm×t.
    lam_vm = add(lam_vm, lam_vprime);
    lam_vm = add(lam_vm, cross(t, lam_vprime));
    lam_t = add(lam_t, cross(lam_vprime, vm));
    lam_vprime = [0.0; 3];
    let _ = lam_vprime;

    // vm = v + half·e.
    let lam_v_in = lam_vm;
    lam_e = add(lam_e, scale(lam_vm, half));

    // t = half·b.
    let lam_b = scale(lam_t, half);

    // Field position Jacobians by central differences at p.
    let mut lam_p = lam_p_partial;
    let hp = 1e-7 * (sol.dr.min(sol.dz)).max(1e-9) * 100.0;
    for i in 0..3 {
        let mut pp = st.p;
        pp[i] += hp;
        let mut pm = st.p;
        pm[i] -= hp;
        let de = scale(sub(e_cart(sol, pp), e_cart(sol, pm)), 1.0 / (2.0 * hp));
        lam_p[i] += dot(lam_e, de);
        if !coils.is_empty() {
            let db = scale(
                sub(b_cart_analytic(coils, pp), b_cart_analytic(coils, pm)),
                1.0 / (2.0 * hp),
            );
            lam_p[i] += dot(lam_b, db);
        }
    }

    // Deposit λ_E into potential nodes (exact linear weights of e_at + the
    // Cartesian rotation), matching the forward sampling exactly.
    deposit_e_adjoint(sol, st.p, lam_e, lam_phi);

    // Ampere-turn gradients: B is linear in each coil's current.
    for (k, c) in coils.iter().enumerate() {
        let unit = RingCoil {
            ampere_turns: 1.0,
            ..*c
        };
        let bu = b_cart_analytic(std::slice::from_ref(&unit), st.p);
        d_turns[k] += dot(lam_b, bu);
    }

    (lam_p, lam_v_in)
}

/// Deposit the Cartesian E-field adjoint into the four potential nodes of
/// the bilinear patch at `p` (the transpose of `Solution::e_at` composed
/// with the axisymmetric rotation).
fn deposit_e_adjoint(sol: &Solution, p: [f64; 3], lam_e: [f64; 3], lam_phi: &mut [f64]) {
    let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
    let (lam_er, lam_ez) = if r < 1e-12 {
        (0.0, lam_e[2])
    } else {
        (lam_e[0] * p[0] / r + lam_e[1] * p[1] / r, lam_e[2])
    };
    let eps = 1e-9;
    let u = (r / sol.dr).clamp(0.0, (sol.nr - 1) as f64 - eps);
    let w = ((p[2] + sol.z_half) / sol.dz).clamp(0.0, (sol.nz - 1) as f64 - eps);
    let i0 = u.floor() as usize;
    let j0 = w.floor() as usize;
    let fu = u - i0 as f64;
    let fw = w - j0 as f64;
    let idx = |i: usize, j: usize| i * sol.nz + j;
    // Er = −((p10−p00)(1−fw) + (p11−p01)·fw)/dr
    lam_phi[idx(i0, j0)] += lam_er * (1.0 - fw) / sol.dr;
    lam_phi[idx(i0 + 1, j0)] += -lam_er * (1.0 - fw) / sol.dr;
    lam_phi[idx(i0, j0 + 1)] += lam_er * fw / sol.dr;
    lam_phi[idx(i0 + 1, j0 + 1)] += -lam_er * fw / sol.dr;
    // Ez = −((p01−p00)(1−fu) + (p11−p10)·fu)/dz
    lam_phi[idx(i0, j0)] += lam_ez * (1.0 - fu) / sol.dz;
    lam_phi[idx(i0, j0 + 1)] += -lam_ez * (1.0 - fu) / sol.dz;
    lam_phi[idx(i0 + 1, j0)] += lam_ez * fu / sol.dz;
    lam_phi[idx(i0 + 1, j0 + 1)] += -lam_ez * fu / sol.dz;
}

/// Radial weight under which the discrete axisymmetric operator is
/// symmetric: `w_i = r_i` off-axis, `Δr/8` on the axis (from the
/// 4(φ₁−φ₀)/Δr² axis stencil).
#[inline]
fn radial_weight(sol: &Solution, i: usize) -> f64 {
    if i == 0 {
        sol.dr / 8.0
    } else {
        i as f64 * sol.dr
    }
}

/// Coefficient of neighbor `(bi, bj)` in the discrete equation of free
/// node `(i, j)` (the numerator weights of the SOR update).
fn stencil_coeff(sol: &Solution, i: usize, j: usize, bi: usize, bj: usize) -> f64 {
    let dr2 = sol.dr * sol.dr;
    let dz2 = sol.dz * sol.dz;
    if bi == i {
        if bj + 1 == j || bj == j + 1 {
            return 1.0 / dz2;
        }
        return 0.0;
    }
    if bj == j {
        if i == 0 && bi == 1 {
            return 4.0 / dr2;
        }
        let r = i as f64 * sol.dr;
        if bi == i + 1 {
            return (r + 0.5 * sol.dr) / (r * dr2);
        }
        if bi + 1 == i {
            return (r - 0.5 * sol.dr) / (r * dr2);
        }
    }
    0.0
}

fn diagonal_coeff(sol: &Solution, i: usize, _j: usize) -> f64 {
    let dr2 = sol.dr * sol.dr;
    let dz2 = sol.dz * sol.dz;
    if i == 0 {
        4.0 / dr2 + 2.0 / dz2
    } else {
        let r = i as f64 * sol.dr;
        ((r + 0.5 * sol.dr) + (r - 0.5 * sol.dr)) / (r * dr2) + 2.0 / dz2
    }
}

fn neighbors(i: usize, j: usize, nr: usize, nz: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::with_capacity(4);
    if i + 1 < nr {
        out.push((i + 1, j));
    }
    if i > 0 {
        out.push((i - 1, j));
    }
    if j + 1 < nz {
        out.push((i, j + 1));
    }
    if j > 0 {
        out.push((i, j - 1));
    }
    out
}

/// Solve the adjoint Poisson system `Lᵀψ = g` restricted to free nodes
/// (ψ = 0 on Dirichlet nodes), using the radial-weight symmetrization:
/// solve `L χ = g/w` with the forward stencil, then `ψ = w·χ`.
fn adjoint_poisson(sol: &Solution, g: &[f64], sopts: &SolveOptions) -> Vec<f64> {
    let (nr, nz) = (sol.nr, sol.nz);
    let idx = |i: usize, j: usize| i * nz + j;
    let mut chi = vec![0.0_f64; nr * nz];
    let omega = if sopts.omega > 0.0 {
        sopts.omega
    } else {
        let n = nr.max(nz) as f64;
        2.0 / (1.0 + (std::f64::consts::PI / n).sin())
    };

    let mut scale_est = 1e-300_f64;
    for sweeps in 0..sopts.max_sweeps {
        let mut resid = 0.0_f64;
        for i in 0..nr - 1 {
            for j in 1..nz - 1 {
                let id = idx(i, j);
                if sol.fixed[id] {
                    continue;
                }
                let mut num = g[id] / radial_weight(sol, i);
                for (ni, nj) in neighbors(i, j, nr, nz) {
                    let nid = idx(ni, nj);
                    if sol.fixed[nid] {
                        continue;
                    }
                    num += stencil_coeff(sol, i, j, ni, nj) * chi[nid];
                }
                let updated = num / diagonal_coeff(sol, i, j);
                let delta = updated - chi[id];
                chi[id] += omega * delta;
                let ad = delta.abs();
                if ad > resid {
                    resid = ad;
                }
                let ac = chi[id].abs();
                if ac > scale_est {
                    scale_est = ac;
                }
            }
        }
        if sweeps > 20 && resid < sopts.tol * scale_est {
            break;
        }
    }

    let mut psi = vec![0.0_f64; nr * nz];
    for i in 0..nr {
        for j in 0..nz {
            psi[idx(i, j)] = radial_weight(sol, i) * chi[idx(i, j)];
        }
    }
    psi
}

/// Which electrode each fixed node belongs to (`Some(ring index)` or
/// `None` for the chamber wall), replicating the forward mask writer's
/// last-ring-wins order.
fn electrode_membership(device: &Device, sol: &Solution) -> Vec<Option<usize>> {
    let (nr, nz) = (sol.nr, sol.nz);
    let idx = |i: usize, j: usize| i * nz + j;
    let mut member = vec![None; nr * nz];
    for (k, ring) in device.rings.iter().enumerate() {
        let r0 = ring.ring_radius_mm * 1e-3;
        let z0 = ring.z_mm * 1e-3;
        let a = (ring.wire_radius_mm * 1e-3).max(0.75 * sol.dr.max(sol.dz));
        for i in 0..nr {
            for j in 0..nz {
                let r = i as f64 * sol.dr;
                let z = -sol.z_half + j as f64 * sol.dz;
                if (r - r0).powi(2) + (z - z0).powi(2) <= a * a {
                    member[idx(i, j)] = Some(k);
                }
            }
        }
    }
    member
}

#[inline]
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[inline]
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

#[inline]
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::Device;

    fn test_device(cathode_v: f64, amp_turns: f64) -> Device {
        // Fat wire + the short horizon below keep the validation objective
        // smooth: gradients are checked before the cusp lens amplifies
        // trajectory sensitivity into FD-visible roughness.
        Device::shielded_two_ring(100.0, 40.0, 20.0, 6.0, cathode_v, amp_turns)
    }

    fn run(cathode_v: f64, amp_turns: f64) -> YieldGradient {
        let device = test_device(cathode_v, amp_turns);
        yield_gradient(
            &device,
            61,
            121,
            // Tight tolerance: the FD probes must not see SOR stopping
            // noise (the default 1e-6·|V| leaves ~0.02 V jitter in φ).
            &SolveOptions {
                tol: 1e-10,
                ..SolveOptions::default()
            },
            &AdjointConfig {
                n_particles: 6,
                time_budget_crossings: 0.8,
                // Freeze the discretization across FD perturbations.
                reference_drop_v: Some(20_000.0),
                ..AdjointConfig::default()
            },
        )
        .expect("gradient run")
    }

    #[test]
    fn adjoint_matches_finite_differences() {
        let v0 = -20_000.0;
        let i0 = 15_000.0;
        let g = run(v0, i0);
        assert!(g.value > 0.0, "no yield in validation config");

        // FD steps chosen from an h-convergence study: J(V) carries real
        // curvature (FD at h=50 reads 4x low before converging by h=3),
        // which is the chaotic-lens sensitivity the adjoint resolves
        // exactly. dJ/dI is h-stable from h≈75 down.

        // Ampere-turn gradient (bypasses the Poisson path; the builder
        // applies +I to ring 0 and −I to ring 1, so dJ/dI = dJ/dI₀ − dJ/dI₁).
        let hi = 20.0;
        let fd_i = (run(v0, i0 + hi).value - run(v0, i0 - hi).value) / (2.0 * hi);
        let adj_i = g.d_ampere_turns[0] - g.d_ampere_turns[1];
        let rel_i = (adj_i - fd_i).abs() / fd_i.abs().max(g.value * 1e-9);
        assert!(
            rel_i < 0.02,
            "ampere-turn gradient mismatch: adjoint {adj_i:.6e}, fd {fd_i:.6e} (rel {rel_i:.3})"
        );

        // Potential gradient: both rings share the bias, so dJ/dV_bias is
        // the sum over rings. Gauge check: shifting every conductor
        // together must do nothing, so wall + rings ≈ 0.
        let hv = 3.0;
        let fd_v = (run(v0 + hv, i0).value - run(v0 - hv, i0).value) / (2.0 * hv);
        let adj_v: f64 = g.d_ring_potentials.iter().sum();
        let rel_v = (adj_v - fd_v).abs() / fd_v.abs().max(1e-300);
        assert!(
            rel_v < 0.03,
            "potential gradient mismatch: adjoint {adj_v:.6e}, fd {fd_v:.6e} (rel {rel_v:.3})"
        );
        let gauge = (adj_v + g.d_wall_potential).abs() / adj_v.abs().max(1e-300);
        assert!(
            gauge < 0.02,
            "gauge invariance violated: rings {adj_v:.3e} vs wall {:.3e}",
            g.d_wall_potential
        );
    }

    #[test]
    fn deeper_bias_means_more_yield_and_the_gradient_says_so() {
        let g = run(-20_000.0, 0.0);
        // Cathode potentials are negative; making them MORE negative
        // (deeper well) increases yield, so dJ/dV must be negative.
        let dv: f64 = g.d_ring_potentials.iter().sum();
        assert!(
            dv < 0.0,
            "yield should grow with a deeper well: dJ/dV = {dv:.3e}"
        );
    }
}
