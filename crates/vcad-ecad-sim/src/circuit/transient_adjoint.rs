//! Transient adjoint — d(time-domain objective)/d(every component value) by
//! reverse sweep over the companion-model recurrence.
//!
//! This extends the transposed-network method (Director & Rohrer, "The
//! generalized adjoint network and network sensitivities", IEEE Trans.
//! Circuit Theory CT-16, 1969) from a single solve to the whole transient:
//! the discrete adjoint of exactly the time-stepping scheme `CircuitEnv`
//! runs, so the gradient is exact to machine precision **for the trajectory
//! as discretized** — the standard discrete-adjoint (backpropagation-through-
//! the-solver) framing, the same contract the particle crate's adjoint keeps.
//!
//! # The recurrence and its adjoint
//!
//! One forward step `k` (fixed `dt`, no adaptivity) is
//!
//! ```text
//! (1)  A·x_k = b_k(h_{k−1}, p)          — the MNA solve
//! (2)  h_k   = U(x_k, h_{k−1}, p)       — companion-history update
//! ```
//!
//! where `h` collects the per-device histories: capacitor voltage `vc` and
//! current `ic`, inductor current `il` and voltage `vl`. Concretely
//! (Nagel, SPICE2, UCB ERL-M520, 1975, §4), with `Δv = v_p − v_n` from `x_k`:
//!
//! ```text
//!            stamp (into A)   RHS injection b_k        history update U
//! C, BE:     g = C/dt         +g·vc_{k−1}              ic_k = g(Δv − vc_{k−1}); vc_k = Δv
//! C, trap:   g = 2C/dt        +g·vc_{k−1} + ic_{k−1}   ic_k = g(Δv − vc_{k−1}) − ic_{k−1}; vc_k = Δv
//! L, BE:     g = dt/L         −il_{k−1}                il_k = g·Δv + il_{k−1}; vl_k = Δv
//! L, trap:   g = dt/2L        −il_{k−1} − g·vl_{k−1}   il_k = g(Δv + vl_{k−1}) + il_{k−1}; vl_k = Δv
//! ```
//!
//! For a scalar objective `J = Σ_k j_k(x_k)`, introduce Lagrange multipliers
//! `λ_k` for (1) and history adjoints `ĥ_k = ∂J/∂h_k` for (2). Differentiating
//! the Lagrangian and collecting terms gives the **reverse sweep**, one step
//! per forward step, newest first:
//!
//! ```text
//! (R1)  Aᵀ·λ_k = ∂j_k/∂x_k + (∂U/∂x_k)ᵀ·ĥ_k
//! (R2)  ĥ_{k−1} = (∂b_k/∂h_{k−1})ᵀ·λ_k + (∂U/∂h_{k−1})ᵀ·ĥ_k
//! (R3)  dJ/dp  += λ_kᵀ·(∂b_k/∂p − (∂A/∂p)·x_k) + ĥ_kᵀ·(∂U/∂p)
//! ```
//!
//! (R1) is Director & Rohrer's transposed network, driven not just by the
//! output functional but by the history adjoints flowing back from step
//! `k+1` — the companion-history coupling. (R2) is the adjoint of the
//! history recurrence: for the trapezoidal capacitor, e.g., the forward RHS
//! term `g·vc_{k−1} + ic_{k−1}` contributes `g·Λ` to `v̂c_{k−1}` and `Λ` to
//! `îc_{k−1}` (with `Λ = λ_p − λ_n`), and the update `ic_k = g(Δv − vc_{k−1})
//! − ic_{k−1}` contributes `−g·îc_k` and `−îc_k`. (R3) includes the
//! ∂/∂p of the companion **conductances** (`dg/dC = 2/dt` for trap, `1/dt`
//! for BE; `dg/dL = −g/L`) and of the **history currents** — both matter,
//! and finite differences catch you if you drop either.
//!
//! The forward pass here is the adjoint's own (linear devices only, one
//! LU per step, first step always backward Euler exactly like
//! `CircuitEnv::step`), storing the full trajectory `x_1..x_N`. Storage is
//! O(N·m) — fine at lumped-circuit scale; checkpointing is a scale problem
//! this module does not have.
//!
//! Validated against central finite differences over the full transient with
//! the discretization frozen across probes (same `dt`, same step count, same
//! integrator), per element kind, both integrators — the house pattern from
//! `vcad-kernel-particle::adjoint`.
//!
//! Nonlinear devices (diode) and motors are rejected with a typed error at
//! this milestone: the diode's reverse sweep needs the per-step converged
//! Newton Jacobian stored alongside the trajectory, which is deliberately a
//! follow-up (`docs/spice-m0.md` M1 ladder).

use super::devices::{inject, stamp_conductance};
use super::linalg::solve_dense;
use super::{Circuit, Device, Integrator};

/// Why a transient adjoint run could not proceed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransientAdjointError {
    /// The MNA matrix was singular (floating node, source loop, …).
    Singular,
    /// A device kind this analysis does not support yet. Carries the device
    /// id and its kind name. Diodes need per-step Jacobian storage (M1);
    /// motors couple to mechanical state.
    Unsupported {
        /// Index of the offending device in [`Circuit::devices`].
        id: usize,
        /// Human-readable device kind.
        kind: &'static str,
    },
    /// `targets` / `weights` length does not match the requested step count.
    LengthMismatch,
    /// The output node is ground or out of range.
    BadOutputNode,
}

impl std::fmt::Display for TransientAdjointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransientAdjointError::Singular => write!(f, "singular MNA system"),
            TransientAdjointError::Unsupported { id, kind } => {
                write!(
                    f,
                    "unsupported device in transient adjoint: {kind} (id {id})"
                )
            }
            TransientAdjointError::LengthMismatch => {
                write!(f, "targets/weights length must equal the step count")
            }
            TransientAdjointError::BadOutputNode => {
                write!(f, "output must be a non-ground node in range")
            }
        }
    }
}

impl std::error::Error for TransientAdjointError {}

/// A time-domain objective value and its gradient per device primary.
#[derive(Debug, Clone, PartialEq)]
pub struct TransientSensitivity {
    /// The objective J = Σ_k w_k·(v_out(t_k) − target_k)².
    pub value: f64,
    /// dJ/d(device primary), one entry per device in device order
    /// (R in Ω, C in F, L in H, source values).
    pub gradient: Vec<f64>,
    /// The simulated output-node waveform, v_out(t_k) for k = 1..N.
    pub v_out: Vec<f64>,
}

/// Per-device companion history (the `h` of the module docs).
#[derive(Clone, Default)]
struct History {
    cap_v: Vec<f64>,
    cap_i: Vec<f64>,
    ind_i: Vec<f64>,
    ind_v: Vec<f64>,
}

/// dJ/d(every device primary) of the weighted least-squares tracking
/// objective `J = Σ_k w_k·(v_out(t_k) − target_k)²` over an `N`-step
/// transient from the power-on state (`t_k = k·dt`, k = 1..N — the same
/// trajectory `CircuitEnv` produces with the same `dt` and integrator).
///
/// Linear devices only (R, L, C, V, I) at this milestone; see
/// [`TransientAdjointError::Unsupported`].
pub fn transient_sensitivities(
    circuit: &Circuit,
    dt: f64,
    integrator: Integrator,
    out_node: usize,
    targets: &[f64],
    weights: &[f64],
) -> Result<TransientSensitivity, TransientAdjointError> {
    let nn = circuit.num_nodes;
    if out_node == 0 || out_node >= nn {
        return Err(TransientAdjointError::BadOutputNode);
    }
    let n_steps = targets.len();
    if weights.len() != n_steps {
        return Err(TransientAdjointError::LengthMismatch);
    }
    for (id, dev) in circuit.devices.iter().enumerate() {
        match dev {
            Device::Diode { .. } => {
                return Err(TransientAdjointError::Unsupported { id, kind: "diode" })
            }
            Device::Motor { .. } => {
                return Err(TransientAdjointError::Unsupported { id, kind: "motor" })
            }
            Device::Mosfet { .. } => {
                return Err(TransientAdjointError::Unsupported { id, kind: "mosfet" })
            }
            Device::Bjt { .. } => {
                return Err(TransientAdjointError::Unsupported { id, kind: "bjt" })
            }
            _ => {}
        }
    }

    let nb = circuit.num_branches();
    let m = (nn - 1) + nb;
    let nd = circuit.devices.len();

    // Branch index per voltage source (insertion order, matching the stamps).
    let mut branch_of = vec![None; nd];
    let mut br = 0usize;
    for (id, dev) in circuit.devices.iter().enumerate() {
        if dev.needs_branch() {
            branch_of[id] = Some(br);
            br += 1;
        }
    }

    // ---- Forward pass: own stepping, frozen dt, first step always BE ----
    // (mirrors CircuitEnv::step for the linear device set).
    let mut hist = History {
        cap_v: vec![0.0; nd],
        cap_i: vec![0.0; nd],
        ind_i: vec![0.0; nd],
        ind_v: vec![0.0; nd],
    };
    // Stored trajectory: full MNA solution per step, plus the history that
    // ENTERED each step (needed by the parameter terms in the reverse sweep).
    let mut xs: Vec<Vec<f64>> = Vec::with_capacity(n_steps);
    let mut hists_in: Vec<History> = Vec::with_capacity(n_steps);

    for k in 0..n_steps {
        let integ = if k == 0 {
            Integrator::BackwardEuler
        } else {
            integrator
        };
        let mut a = vec![0.0f64; m * m];
        let mut rhs = vec![0.0f64; m];
        stamp_system(circuit, integ, dt, &hist, &mut a, &mut rhs, m, nn);
        let x = solve_dense(&mut a, &mut rhs, m).ok_or(TransientAdjointError::Singular)?;

        hists_in.push(hist.clone());
        update_history(circuit, integ, dt, &x, &mut hist);
        xs.push(x);
    }

    let node_v = |x: &[f64], node: usize| if node == 0 { 0.0 } else { x[node - 1] };
    let v_out: Vec<f64> = xs.iter().map(|x| node_v(x, out_node)).collect();
    let value: f64 = v_out
        .iter()
        .zip(targets)
        .zip(weights)
        .map(|((v, t), w)| w * (v - t) * (v - t))
        .sum();

    // ---- Reverse sweep ----
    let mut gradient = vec![0.0f64; nd];
    // Adjoints of the history state h_k, initialized at ĥ_N = 0 (nothing
    // after the last step reads it).
    let mut hv = vec![0.0f64; nd]; // v̂c
    let mut hi = vec![0.0f64; nd]; // îc
    let mut li = vec![0.0f64; nd]; // îl
    let mut lv = vec![0.0f64; nd]; // v̂l

    for k in (0..n_steps).rev() {
        let integ = if k == 0 {
            Integrator::BackwardEuler
        } else {
            integrator
        };
        let x = &xs[k];
        let h_in = &hists_in[k];

        // r_k = ∂j_k/∂x_k + (∂U/∂x_k)ᵀ·ĥ_k  — the RHS of (R1).
        let mut r = vec![0.0f64; m];
        r[out_node - 1] = 2.0 * weights[k] * (v_out[k] - targets[k]);
        for (id, dev) in circuit.devices.iter().enumerate() {
            let pat = |r: &mut [f64], p: usize, n: usize, s: f64| {
                if p != 0 {
                    r[p - 1] += s;
                }
                if n != 0 {
                    r[n - 1] -= s;
                }
            };
            match *dev {
                Device::Capacitor { p, n, c } => {
                    // vc_k = Δv; ic_k = g·Δv + (terms without x).
                    let g = comp_g_cap(integ, c, dt);
                    pat(&mut r, p, n, hv[id] + g * hi[id]);
                }
                Device::Inductor { p, n, l } => {
                    // vl_k = Δv; il_k = g·Δv + (terms without x).
                    let g = comp_g_ind(integ, l, dt);
                    pat(&mut r, p, n, lv[id] + g * li[id]);
                }
                _ => {}
            }
        }

        // Solve Aᵀ·λ_k = r_k. A is re-stamped (cheap at this scale) and
        // transposed; the RHS injections don't matter for the matrix.
        let mut a = vec![0.0f64; m * m];
        let mut dummy = vec![0.0f64; m];
        stamp_system(circuit, integ, dt, h_in, &mut a, &mut dummy, m, nn);
        let mut at = vec![0.0f64; m * m];
        for i in 0..m {
            for j in 0..m {
                at[j * m + i] = a[i * m + j];
            }
        }
        let lambda = solve_dense(&mut at, &mut r, m).ok_or(TransientAdjointError::Singular)?;
        let lam = |node: usize| if node == 0 { 0.0 } else { lambda[node - 1] };

        // (R3) parameter accumulation + (R2) history-adjoint propagation.
        for (id, dev) in circuit.devices.iter().enumerate() {
            match *dev {
                Device::Resistor { p, n, r } => {
                    // ∂A/∂R is the conductance pattern scaled dg/dR = −1/R²;
                    // dJ/dR += −λᵀ(∂A/∂R)x = (ΛΔv)/R².
                    gradient[id] += (lam(p) - lam(n)) * (node_v(x, p) - node_v(x, n)) / (r * r);
                }
                Device::VSource { .. } => {
                    // b_br = V every step ⇒ dJ/dV += λ_br.
                    let b = branch_of[id].expect("vsource has a branch");
                    gradient[id] += lambda[(nn - 1) + b];
                }
                Device::ISource { p, n, .. } => {
                    gradient[id] += lam(p) - lam(n);
                }
                Device::Capacitor { p, n, c } => {
                    let g = comp_g_cap(integ, c, dt);
                    let dg = g / c; // dg/dC = 1/dt (BE) or 2/dt (trap)
                    let lam_d = lam(p) - lam(n);
                    let dv = node_v(x, p) - node_v(x, n);
                    let vc_prev = h_in.cap_v[id];
                    // A-term: −λᵀ(∂A/∂C)x over the conductance pattern.
                    gradient[id] -= dg * lam_d * dv;
                    // b-term: RHS injection is g·vc_prev (+ ic_prev, no C dep).
                    gradient[id] += dg * vc_prev * lam_d;
                    // U-term: ic_k = g(Δv − vc_prev) ∓ ic_prev ⇒ ∂/∂C = dg(Δv − vc_prev).
                    gradient[id] += hi[id] * dg * (dv - vc_prev);

                    // (R2): push ĥ_k back to ĥ_{k−1}. Order matters: consume
                    // hv[id]/hi[id] (adjoints of h_k), then overwrite.
                    let (nv, ni) = match integ {
                        Integrator::BackwardEuler => {
                            // b: +g·vc_prev ⇒ v̂c += g·Λ. U: ic_k = g(Δv − vc_prev)
                            // ⇒ v̂c += −g·îc. ic_prev unread ⇒ îc_{k−1} = 0.
                            (g * lam_d - g * hi[id], 0.0)
                        }
                        Integrator::Trapezoidal => {
                            // b: +g·vc_prev + ic_prev; U: ic_k = g(Δv − vc_prev) − ic_prev.
                            (g * lam_d - g * hi[id], lam_d - hi[id])
                        }
                    };
                    hv[id] = nv;
                    hi[id] = ni;
                }
                Device::Inductor { p, n, l } => {
                    let g = comp_g_ind(integ, l, dt);
                    let dg = -g / l; // dg/dL
                    let lam_d = lam(p) - lam(n);
                    let dv = node_v(x, p) - node_v(x, n);
                    let vl_prev = h_in.ind_v[id];
                    // A-term over the conductance pattern.
                    gradient[id] -= dg * lam_d * dv;
                    match integ {
                        Integrator::BackwardEuler => {
                            // b: −il_prev (no L dep). U: il_k = g·Δv + il_prev
                            // ⇒ ∂/∂L = dg·Δv.
                            gradient[id] += li[id] * dg * dv;
                            let n_li = -lam_d + li[id];
                            lv[id] = 0.0;
                            li[id] = n_li;
                        }
                        Integrator::Trapezoidal => {
                            // b: −il_prev − g·vl_prev ⇒ ∂/∂L = −dg·vl_prev.
                            gradient[id] += lam_d * (-dg * vl_prev);
                            // U: il_k = g(Δv + vl_prev) + il_prev ⇒ ∂/∂L = dg(Δv + vl_prev).
                            gradient[id] += li[id] * dg * (dv + vl_prev);
                            let n_lv = -g * lam_d + g * li[id];
                            let n_li = -lam_d + li[id];
                            lv[id] = n_lv;
                            li[id] = n_li;
                        }
                    }
                }
                Device::Diode { .. }
                | Device::Motor { .. }
                | Device::Mosfet { .. }
                | Device::Bjt { .. } => unreachable!("rejected above"),
            }
        }
    }

    Ok(TransientSensitivity {
        value,
        gradient,
        v_out,
    })
}

/// Companion conductance of a capacitor under `integ`.
fn comp_g_cap(integ: Integrator, c: f64, dt: f64) -> f64 {
    match integ {
        Integrator::BackwardEuler => c / dt,
        Integrator::Trapezoidal => 2.0 * c / dt,
    }
}

/// Companion conductance of an inductor under `integ`.
fn comp_g_ind(integ: Integrator, l: f64, dt: f64) -> f64 {
    match integ {
        Integrator::BackwardEuler => dt / l,
        Integrator::Trapezoidal => dt / (2.0 * l),
    }
}

/// Stamp the full linear MNA system for one step (matches `CircuitEnv::step`
/// for the linear device set, per the table in the module docs).
#[allow(clippy::too_many_arguments)]
fn stamp_system(
    circuit: &Circuit,
    integ: Integrator,
    dt: f64,
    hist: &History,
    a: &mut [f64],
    rhs: &mut [f64],
    m: usize,
    nn: usize,
) {
    let mut branch = 0usize;
    for (id, dev) in circuit.devices.iter().enumerate() {
        match *dev {
            Device::Resistor { p, n, r } => stamp_conductance(a, m, p, n, 1.0 / r),
            Device::Capacitor { p, n, c } => {
                let g = comp_g_cap(integ, c, dt);
                stamp_conductance(a, m, p, n, g);
                let i_eq = match integ {
                    Integrator::BackwardEuler => g * hist.cap_v[id],
                    Integrator::Trapezoidal => g * hist.cap_v[id] + hist.cap_i[id],
                };
                inject(rhs, p, n, i_eq);
            }
            Device::Inductor { p, n, l } => {
                let g = comp_g_ind(integ, l, dt);
                stamp_conductance(a, m, p, n, g);
                let i_eq = match integ {
                    Integrator::BackwardEuler => -hist.ind_i[id],
                    Integrator::Trapezoidal => -(hist.ind_i[id] + g * hist.ind_v[id]),
                };
                inject(rhs, p, n, i_eq);
            }
            Device::VSource { p, n, v } => {
                let br = (nn - 1) + branch;
                branch += 1;
                if p != 0 {
                    a[(p - 1) * m + br] += 1.0;
                    a[br * m + (p - 1)] += 1.0;
                }
                if n != 0 {
                    a[(n - 1) * m + br] -= 1.0;
                    a[br * m + (n - 1)] -= 1.0;
                }
                rhs[br] += v;
            }
            Device::ISource { p, n, i } => inject(rhs, p, n, i),
            Device::Diode { .. }
            | Device::Motor { .. }
            | Device::Mosfet { .. }
            | Device::Bjt { .. } => {
                unreachable!("rejected before the forward pass")
            }
        }
    }
}

/// Apply the companion-history update `h_k = U(x_k, h_{k−1}, p)` in place.
fn update_history(circuit: &Circuit, integ: Integrator, dt: f64, x: &[f64], hist: &mut History) {
    let node_v = |node: usize| if node == 0 { 0.0 } else { x[node - 1] };
    for (id, dev) in circuit.devices.iter().enumerate() {
        match *dev {
            Device::Capacitor { p, n, c } => {
                let dv = node_v(p) - node_v(n);
                let g = comp_g_cap(integ, c, dt);
                let i_new = match integ {
                    Integrator::BackwardEuler => g * (dv - hist.cap_v[id]),
                    Integrator::Trapezoidal => g * (dv - hist.cap_v[id]) - hist.cap_i[id],
                };
                hist.cap_v[id] = dv;
                hist.cap_i[id] = i_new;
            }
            Device::Inductor { p, n, l } => {
                let dv = node_v(p) - node_v(n);
                let g = comp_g_ind(integ, l, dt);
                let i_new = match integ {
                    Integrator::BackwardEuler => g * dv + hist.ind_i[id],
                    Integrator::Trapezoidal => g * (dv + hist.ind_v[id]) + hist.ind_i[id],
                };
                hist.ind_i[id] = i_new;
                hist.ind_v[id] = dv;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::CircuitEnv;

    /// J over the full transient computed by re-running the forward pass —
    /// the FD oracle, with the discretization frozen (same dt, steps,
    /// integrator) across probes.
    fn objective(
        circuit: &Circuit,
        dt: f64,
        integ: Integrator,
        out: usize,
        targets: &[f64],
        weights: &[f64],
    ) -> f64 {
        transient_sensitivities(circuit, dt, integ, out, targets, weights)
            .unwrap()
            .value
    }

    #[allow(clippy::too_many_arguments)]
    fn fd_gradient(
        circuit: &Circuit,
        dt: f64,
        integ: Integrator,
        out: usize,
        targets: &[f64],
        weights: &[f64],
        id: usize,
        rel: f64,
    ) -> f64 {
        let base = circuit.devices[id].primary();
        let h = base.abs() * rel;
        let mut lo = circuit.clone();
        let mut hi = circuit.clone();
        lo.devices[id].set_primary(base - h);
        hi.devices[id].set_primary(base + h);
        (objective(&hi, dt, integ, out, targets, weights)
            - objective(&lo, dt, integ, out, targets, weights))
            / (2.0 * h)
    }

    /// RC low-pass: V —R— out —C— gnd.
    fn rc() -> (Circuit, usize) {
        let mut c = Circuit::new();
        let vin = c.node();
        let out = c.node();
        c.add(Device::VSource {
            p: vin,
            n: 0,
            v: 5.0,
        });
        c.add(Device::Resistor {
            p: vin,
            n: out,
            r: 1_000.0,
        });
        c.add(Device::Capacitor {
            p: out,
            n: 0,
            c: 1e-6,
        });
        (c, out)
    }

    /// Series RLC with an extra I-source shunt: every element kind.
    fn rlc() -> (Circuit, usize) {
        let mut c = Circuit::new();
        let vin = c.node();
        let mid = c.node();
        let out = c.node();
        c.add(Device::VSource {
            p: vin,
            n: 0,
            v: 5.0,
        });
        c.add(Device::Resistor {
            p: vin,
            n: mid,
            r: 50.0,
        });
        c.add(Device::Inductor {
            p: mid,
            n: out,
            l: 1e-3,
        });
        c.add(Device::Capacitor {
            p: out,
            n: 0,
            c: 1e-7,
        });
        c.add(Device::ISource {
            p: out,
            n: 0,
            i: 1e-3,
        });
        (c, out)
    }

    /// A target that keeps every step's residual nonzero (so no gradient
    /// component is trivially zero): track 60% of the network's own response.
    fn tracking_problem(
        circuit: &Circuit,
        dt: f64,
        integ: Integrator,
        out: usize,
        n: usize,
    ) -> (Vec<f64>, Vec<f64>) {
        let s =
            transient_sensitivities(circuit, dt, integ, out, &vec![0.0; n], &vec![1.0; n]).unwrap();
        let targets: Vec<f64> = s.v_out.iter().map(|v| 0.6 * v).collect();
        (targets, vec![1.0; n])
    }

    fn assert_grad_matches_fd(circuit: &Circuit, dt: f64, integ: Integrator, out: usize, n: usize) {
        let (targets, weights) = tracking_problem(circuit, dt, integ, out, n);
        let sens = transient_sensitivities(circuit, dt, integ, out, &targets, &weights).unwrap();
        for id in 0..circuit.devices.len() {
            let fd = fd_gradient(circuit, dt, integ, out, &targets, &weights, id, 1e-6);
            let ad = sens.gradient[id];
            let scale = fd.abs().max(ad.abs()).max(1e-12);
            assert!(
                (fd - ad).abs() / scale < 1e-5,
                "{integ:?} device {id}: adjoint {ad} vs FD {fd}"
            );
        }
    }

    #[test]
    fn adjoint_matches_fd_rc_backward_euler() {
        let (c, out) = rc();
        assert_grad_matches_fd(&c, 1e-5, Integrator::BackwardEuler, out, 200);
    }

    #[test]
    fn adjoint_matches_fd_rc_trapezoidal() {
        let (c, out) = rc();
        assert_grad_matches_fd(&c, 1e-5, Integrator::Trapezoidal, out, 200);
    }

    #[test]
    fn adjoint_matches_fd_rlc_every_element_kind_backward_euler() {
        let (c, out) = rlc();
        assert_grad_matches_fd(&c, 2e-7, Integrator::BackwardEuler, out, 300);
    }

    #[test]
    fn adjoint_matches_fd_rlc_every_element_kind_trapezoidal() {
        let (c, out) = rlc();
        assert_grad_matches_fd(&c, 2e-7, Integrator::Trapezoidal, out, 300);
    }

    #[test]
    fn forward_pass_matches_circuit_env_trajectory() {
        // The adjoint's private forward pass must reproduce CircuitEnv
        // exactly (same first-step-BE rule, same companion updates) — the
        // gradient is only "of the simulation" if the trajectory IS the
        // simulation's.
        for integ in [Integrator::BackwardEuler, Integrator::Trapezoidal] {
            let (c, out) = rlc();
            let dt = 2e-7;
            let n = 250;
            let sens =
                transient_sensitivities(&c, dt, integ, out, &vec![0.0; n], &vec![1.0; n]).unwrap();
            let mut env = CircuitEnv::new(c, dt);
            env.reset();
            env.set_integrator(integ);
            for k in 0..n {
                let obs = env.step();
                assert!(
                    (obs.node_voltages[out] - sens.v_out[k]).abs() < 1e-12,
                    "{integ:?} step {k}: env {} vs adjoint forward {}",
                    obs.node_voltages[out],
                    sens.v_out[k]
                );
            }
        }
    }

    #[test]
    fn diode_and_motor_are_rejected_with_typed_errors() {
        let mut c = Circuit::new();
        let vin = c.node();
        c.add(Device::VSource {
            p: vin,
            n: 0,
            v: 5.0,
        });
        let did = c.add(Device::Diode {
            p: vin,
            n: 0,
            model: crate::circuit::DiodeModel::silicon(),
        });
        let err = transient_sensitivities(&c, 1e-6, Integrator::BackwardEuler, vin, &[0.0], &[1.0])
            .unwrap_err();
        assert_eq!(
            err,
            TransientAdjointError::Unsupported {
                id: did,
                kind: "diode"
            }
        );
    }

    #[test]
    fn gradient_descent_tunes_rc_time_constant_to_target_step_response() {
        // Demo: recover R so the RC step response matches a target waveform
        // generated at R* = 2.2 kΩ. Log-space descent (R is a positive scale
        // quantity), scale-invariant stopping on the relative gradient.
        let dt = 1e-5;
        let n = 400;
        let build = |r: f64| {
            let mut c = Circuit::new();
            let vin = c.node();
            let out = c.node();
            c.add(Device::VSource {
                p: vin,
                n: 0,
                v: 5.0,
            });
            let rid = c.add(Device::Resistor { p: vin, n: out, r });
            c.add(Device::Capacitor {
                p: out,
                n: 0,
                c: 1e-6,
            });
            (c, rid, out)
        };
        let (ct, _, out) = build(2_200.0);
        let target = transient_sensitivities(
            &ct,
            dt,
            Integrator::Trapezoidal,
            out,
            &vec![0.0; n],
            &vec![1.0; n],
        )
        .unwrap()
        .v_out;
        let weights = vec![1.0; n];

        let mut r = 800.0; // start well off
        let mut iters = 0;
        loop {
            let (c, rid, out) = build(r);
            let s =
                transient_sensitivities(&c, dt, Integrator::Trapezoidal, out, &target, &weights)
                    .unwrap();
            // dJ/d ln R = R·dJ/dR — scale-invariant step and stop.
            let g_log = r * s.gradient[rid];
            if g_log.abs() < 1e-9 * s.value.max(1e-12) / 1e-3 || iters > 500 {
                break;
            }
            // Backtracking step in ln R.
            let mut step = 0.5f64;
            loop {
                let r_try = r * (-step * g_log.signum()).exp();
                let (c2, _, o2) = build(r_try);
                let j2 = transient_sensitivities(
                    &c2,
                    dt,
                    Integrator::Trapezoidal,
                    o2,
                    &target,
                    &weights,
                )
                .unwrap()
                .value;
                if j2 < s.value || step < 1e-6 {
                    r = r_try;
                    break;
                }
                step *= 0.5;
            }
            iters += 1;
            if s.value < 1e-16 {
                break;
            }
        }
        assert!(
            (r - 2_200.0).abs() / 2_200.0 < 1e-3,
            "tuned R = {r} after {iters} iterations, expected 2200"
        );
    }
}
