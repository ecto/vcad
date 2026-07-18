//! Adjoint sensitivities — d(output)/d(every component value) from **one**
//! extra linear solve.
//!
//! This is the classic transposed-network method (Director & Rohrer, "The
//! generalized adjoint network and network sensitivities", IEEE Trans.
//! Circuit Theory CT-16, 1969): for the MNA system `A(p)·x = b(p)` and a
//! scalar output `y = eᵀx`, solve the transposed system `Aᵀλ = e` once, and
//! every parameter's sensitivity is the cheap inner product
//!
//! ```text
//! dy/dpᵢ = λᵀ·(∂b/∂pᵢ − (∂A/∂pᵢ)·x)
//! ```
//!
//! `∂A/∂pᵢ` is one element's stamp pattern, so each inner product touches at
//! most four matrix entries — the whole gradient costs one transposed solve
//! regardless of how many components the circuit has. (Finite differences
//! would cost one full solve **per component**.)
//!
//! Two flavors here, both exact to machine precision for the network as
//! discretized (and both validated against central finite differences over
//! every element kind in the tests at the bottom):
//!
//! - [`dc_sensitivities`] — d(node voltage)/dp at the DC operating point.
//!   For nonlinear circuits the residual form `F(x, p) = 0` and the implicit
//!   function theorem give `dy/dp = −λᵀ·∂F/∂p` with `Jᵀλ = e` evaluated at
//!   the converged solution (J = the final Newton Jacobian, gmin = 0).
//! - [`ac_sensitivities`] — d H(jω)/dp of the complex transfer function.
//!   `Aᵀ` is the plain transpose, **not** the conjugate transpose: H is an
//!   analytic function of the real parameters, so its derivative is complex
//!   and the un-conjugated adjoint carries it exactly. `d|H|/dp` follows as
//!   `Re(conj(H)·dH/dp)/|H|`.
//!
//! Parameter convention: one slot per device, in device order, differentiating
//! with respect to that device's **primary scalar** (R in Ω, C in F, L in H,
//! source values). The diode has no primary scalar; its slot carries
//! d(output)/d(Is) at DC and 0 at AC (AC sensitivity through the operating-
//! point shift is an M1 item, noted in `docs/spice-m0.md`).

use super::ac::{build_and_solve, solve_dense_c, AcError, Cplx};
use super::dc::{operating_point, DcError, DcSolution};
use super::devices::VT;
use super::linalg::solve_dense;
use super::{Circuit, Device};

/// A DC output value and its gradient with respect to every device primary.
#[derive(Debug, Clone, PartialEq)]
pub struct DcSensitivity {
    /// The output: voltage at the requested node (V).
    pub value: f64,
    /// d value / d (device primary), one entry per device in device order.
    /// The diode slot is d value / d Is.
    pub gradient: Vec<f64>,
    /// The operating point the gradient was evaluated at.
    pub op: DcSolution,
}

/// Sensitivity of the DC voltage at `out_node` to every device primary.
pub fn dc_sensitivities(circuit: &Circuit, out_node: usize) -> Result<DcSensitivity, DcError> {
    let op = operating_point(circuit)?;
    let nn = circuit.num_nodes;
    assert!(
        out_node > 0 && out_node < nn,
        "output must be a non-ground node"
    );

    // Rebuild the final Newton Jacobian at the converged solution (gmin = 0),
    // transpose it, and solve Jᵀλ = e_out.
    let nb = circuit
        .devices
        .iter()
        .filter(|d| matches!(d, Device::VSource { .. } | Device::Inductor { .. }))
        .count();
    let m = (nn - 1) + nb;

    let mut a = vec![0.0f64; m * m];
    let mut branch = 0usize;
    let mut branch_of = vec![None; circuit.devices.len()];
    for (id, dev) in circuit.devices.iter().enumerate() {
        stamp_jacobian_dc(dev, &mut a, m, nn, &mut branch, &op, id, &mut branch_of);
    }

    // Transpose in place into a fresh buffer.
    let mut at = vec![0.0f64; m * m];
    for i in 0..m {
        for j in 0..m {
            at[j * m + i] = a[i * m + j];
        }
    }
    let mut e = vec![0.0f64; m];
    e[out_node - 1] = 1.0;
    let lambda = solve_dense(&mut at, &mut e, m).ok_or(DcError::Singular)?;

    // λ over nodes only (branch adjoints indexed separately).
    let lam = |node: usize| if node == 0 { 0.0 } else { lambda[node - 1] };
    let v = &op.node_voltages;

    // dy/dp = −λᵀ·∂F/∂p, with F the KCL/branch residual A(p)x − b(p) (+ the
    // diode's nonlinear current). Each device touches at most two rows.
    let mut gradient = vec![0.0f64; circuit.devices.len()];
    for (id, dev) in circuit.devices.iter().enumerate() {
        gradient[id] = match *dev {
            Device::Resistor { p, n, r } => {
                // F_p += (v_p − v_n)/R ⇒ ∂F/∂R = −(v_p − v_n)/R²  (row p; −row n)
                (lam(p) - lam(n)) * (v[p] - v[n]) / (r * r)
            }
            // Capacitor absent at DC; inductor short has no L dependence.
            Device::Capacitor { .. } | Device::Inductor { .. } => 0.0,
            Device::VSource { .. } => {
                // F_br = v(p) − v(n) − V ⇒ ∂F/∂V = −1 on the branch row.
                let br = branch_of[id].expect("vsource has a branch");
                lambda[(nn - 1) + br]
            }
            Device::ISource { p, n, .. } => {
                // b_p += I ⇒ ∂F/∂I = −1 at row p, +1 at row n.
                lam(p) - lam(n)
            }
            Device::Diode { p, n, model } => {
                // F_p += Is·(exp(v_d/vte) − 1) ⇒ ∂F/∂Is = exp(v_d/vte) − 1.
                let vte = model.n * VT;
                let ev = ((v[p] - v[n]) / vte).min(60.0).exp();
                -(lam(p) - lam(n)) * (ev - 1.0)
            }
            Device::Motor { .. } => unreachable!("rejected by the DC solve"),
        };
    }

    Ok(DcSensitivity {
        value: op.node_voltages[out_node],
        gradient,
        op,
    })
}

/// Stamp the DC Newton Jacobian at the converged operating point.
#[allow(clippy::too_many_arguments)]
fn stamp_jacobian_dc(
    dev: &Device,
    a: &mut [f64],
    m: usize,
    nn: usize,
    branch: &mut usize,
    op: &DcSolution,
    _id: usize,
    branch_of: &mut [Option<usize>],
) {
    use super::devices::stamp_conductance;
    match *dev {
        Device::Resistor { p, n, r } => stamp_conductance(a, m, p, n, 1.0 / r),
        Device::Capacitor { .. } => {}
        Device::Inductor { p, n, .. } | Device::VSource { p, n, .. } => {
            let br = (nn - 1) + *branch;
            branch_of[_id] = Some(*branch);
            *branch += 1;
            if p != 0 {
                a[(p - 1) * m + br] += 1.0;
                a[br * m + (p - 1)] += 1.0;
            }
            if n != 0 {
                a[(n - 1) * m + br] -= 1.0;
                a[br * m + (n - 1)] -= 1.0;
            }
        }
        Device::ISource { .. } => {}
        Device::Diode { p, n, model } => {
            let vte = model.n * VT;
            let vd = op.node_voltages[p] - op.node_voltages[n];
            let geq = (model.is / vte) * (vd / vte).min(60.0).exp();
            stamp_conductance(a, m, p, n, geq);
        }
        Device::Motor { .. } => unreachable!(),
    }
}

/// An AC transfer value and its complex gradient per device primary.
#[derive(Debug, Clone, PartialEq)]
pub struct AcSensitivity {
    /// The complex transfer H(jω) = V(out) per unit source amplitude.
    pub h: Cplx,
    /// dH/dp per device primary, in device order. A slot listed in
    /// [`AcSensitivity::deferred`] is a **placeholder zero**, not a computed
    /// value — see [`AcSensitivity::is_deferred`].
    pub gradient: Vec<Cplx>,
    /// Device ids whose gradient slot is a placeholder, not a computed
    /// sensitivity. At M0 this is exactly the diodes (their AC sensitivity
    /// runs through the operating-point shift, deferred to M1). Callers that
    /// must distinguish "sensitivity is genuinely zero" from "sensitivity was
    /// not computed" consult this; M1 shrinks the list without an API break.
    pub deferred: Vec<usize>,
}

impl AcSensitivity {
    /// Whether device `id`'s gradient slot is a deferred placeholder rather
    /// than a computed value.
    pub fn is_deferred(&self, id: usize) -> bool {
        self.deferred.contains(&id)
    }

    /// d|H|/dp for parameter slot `i`: `Re(conj(H)·dH/dpᵢ)/|H|`. Returns 0 for
    /// a deferred slot (the value is not available at M0).
    pub fn d_magnitude(&self, i: usize) -> f64 {
        let habs = self.h.abs();
        if habs == 0.0 {
            return 0.0;
        }
        let g = self.h.conj() * self.gradient[i];
        g.re / habs
    }
}

/// Sensitivity of the complex node voltage at `out_node` (per unit `source`
/// amplitude) at angular frequency `omega`, to every device primary.
pub fn ac_sensitivities(
    circuit: &Circuit,
    source: usize,
    omega: f64,
    out_node: usize,
) -> Result<AcSensitivity, AcError> {
    let sys = build_and_solve(circuit, source, omega)?;
    let (m, nn) = (sys.m, sys.nn);
    assert!(
        out_node > 0 && out_node < nn,
        "output must be a non-ground node"
    );

    // Aᵀλ = e_out — plain transpose, no conjugation (see module docs).
    let mut at = vec![Cplx::ZERO; m * m];
    for i in 0..m {
        for j in 0..m {
            at[j * m + i] = sys.a[i * m + j];
        }
    }
    let mut e = vec![Cplx::ZERO; m];
    e[out_node - 1] = Cplx::ONE;
    let lambda = solve_dense_c(&mut at, &mut e, m).ok_or(AcError::Singular)?;

    let lam = |node: usize| {
        if node == 0 {
            Cplx::ZERO
        } else {
            lambda[node - 1]
        }
    };
    let xv = |node: usize| {
        if node == 0 {
            Cplx::ZERO
        } else {
            sys.x[node - 1]
        }
    };

    let h = xv(out_node);
    let mut gradient = vec![Cplx::ZERO; circuit.devices.len()];
    let mut deferred = Vec::new();
    for (id, dev) in circuit.devices.iter().enumerate() {
        gradient[id] = match *dev {
            Device::Resistor { p, n, r } => {
                // ∂A/∂R over the conductance pattern with dg/dR = −1/R²:
                // dH/dR = −dg·(λ_p − λ_n)(x_p − x_n)
                let dg = -1.0 / (r * r);
                -(Cplx::real(dg) * (lam(p) - lam(n)) * (xv(p) - xv(n)))
            }
            Device::Capacitor { p, n, .. } => {
                // dg/dC = jω over the same pattern.
                -(Cplx::imag(omega) * (lam(p) - lam(n)) * (xv(p) - xv(n)))
            }
            Device::Inductor { .. } => {
                // Branch row: v(p) − v(n) − jωL·i = 0 ⇒ ∂A/∂L = −jω on the
                // branch diagonal ⇒ dH/dL = +jω·λ_br·i_br.
                let br = sys.branch_of[id].expect("inductor has a branch");
                let i_br = sys.x[(nn - 1) + br];
                Cplx::imag(omega) * lambda[(nn - 1) + br] * i_br
            }
            Device::VSource { .. } => {
                // ∂b/∂amplitude = 1 on the branch row (only nonzero if this
                // device is the driven source, but the derivative pattern is
                // the same either way: the source term is amplitude·e_br).
                let br = sys.branch_of[id].expect("vsource has a branch");
                if id == source {
                    lambda[(nn - 1) + br]
                } else {
                    // A zeroed AC source: its amplitude is fixed at 0 by the
                    // superposition convention; sensitivity to its DC value
                    // is zero for a linear network.
                    Cplx::ZERO
                }
            }
            Device::ISource { p, n, .. } => {
                if id == source {
                    lam(p) - lam(n)
                } else {
                    Cplx::ZERO
                }
            }
            // Operating-point chain term deferred (M1); see module docs.
            // Flagged in `deferred` so the placeholder zero is distinguishable
            // from a genuine zero sensitivity.
            Device::Diode { .. } => {
                deferred.push(id);
                Cplx::ZERO
            }
            Device::Motor { .. } => unreachable!("rejected by build_and_solve"),
        };
    }

    Ok(AcSensitivity {
        h,
        gradient,
        deferred,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::DiodeModel;

    /// Central finite difference of the DC out-node voltage wrt device `id`'s
    /// primary (or Is for diodes), with frozen everything else.
    fn fd_dc(circuit: &Circuit, out: usize, id: usize, rel: f64) -> f64 {
        let base = match circuit.devices[id] {
            Device::Diode { model, .. } => model.is,
            ref d => d.primary(),
        };
        let h = base.abs() * rel;
        let mut lo = circuit.clone();
        let mut hi = circuit.clone();
        match (&mut lo.devices[id], &mut hi.devices[id]) {
            (Device::Diode { model: ml, .. }, Device::Diode { model: mh, .. }) => {
                ml.is = base - h;
                mh.is = base + h;
            }
            (dl, dh) => {
                dl.set_primary(base - h);
                dh.set_primary(base + h);
            }
        }
        let vlo = operating_point(&lo).unwrap().node_voltages[out];
        let vhi = operating_point(&hi).unwrap().node_voltages[out];
        (vhi - vlo) / (2.0 * h)
    }

    #[test]
    fn dc_adjoint_matches_fd_on_every_linear_element_kind() {
        // V — R1 — out — R2 ∥ I-source, with an L short and a dangling C:
        // every linear element kind in one network.
        let mut c = Circuit::new();
        let vin = c.node();
        let out = c.node();
        let aux = c.node();
        c.add(Device::VSource {
            p: vin,
            n: 0,
            v: 10.0,
        });
        c.add(Device::Resistor {
            p: vin,
            n: out,
            r: 2_000.0,
        });
        c.add(Device::Resistor {
            p: out,
            n: 0,
            r: 3_000.0,
        });
        c.add(Device::ISource {
            p: out,
            n: 0,
            i: 1e-3,
        });
        c.add(Device::Inductor {
            p: out,
            n: aux,
            l: 4.7e-3,
        });
        c.add(Device::Resistor {
            p: aux,
            n: 0,
            r: 1_000.0,
        });
        c.add(Device::Capacitor {
            p: aux,
            n: 0,
            c: 1e-7,
        });

        let sens = dc_sensitivities(&c, out).unwrap();
        for id in 0..c.devices.len() {
            let fd = fd_dc(&c, out, id, 1e-6);
            let ad = sens.gradient[id];
            let scale = fd.abs().max(ad.abs()).max(1e-12);
            assert!(
                (fd - ad).abs() / scale < 1e-5,
                "device {id}: adjoint {ad} vs FD {fd}"
            );
        }
    }

    #[test]
    fn dc_adjoint_matches_fd_through_the_diode() {
        // V — R — diode: the nonlinear case, sensitivities through the
        // converged Newton system (implicit function theorem).
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
        c.add(Device::Diode {
            p: out,
            n: 0,
            model: DiodeModel::silicon(),
        });

        let sens = dc_sensitivities(&c, out).unwrap();
        for id in 0..c.devices.len() {
            let fd = fd_dc(&c, out, id, 1e-5);
            let ad = sens.gradient[id];
            let scale = fd.abs().max(ad.abs()).max(1e-15);
            assert!(
                (fd - ad).abs() / scale < 1e-4,
                "device {id}: adjoint {ad} vs FD {fd}"
            );
        }
    }

    #[test]
    fn ac_adjoint_matches_fd_on_rlc() {
        // Series RLC low-pass: R, L, C, and the source all get nonzero
        // sensitivities of |H| near resonance; compare each to central FD.
        let (r, l, cval) = (100.0, 1e-3, 1e-7);
        let build = |r: f64, l: f64, cv: f64| {
            let mut c = Circuit::new();
            let vin = c.node();
            let mid = c.node();
            let out = c.node();
            let src = c.add(Device::VSource {
                p: vin,
                n: 0,
                v: 0.0,
            });
            c.add(Device::Resistor { p: vin, n: mid, r });
            c.add(Device::Inductor { p: mid, n: out, l });
            c.add(Device::Capacitor {
                p: out,
                n: 0,
                c: cv,
            });
            (c, src, out)
        };
        let (ckt, src, out) = build(r, l, cval);
        // Probe off resonance: at exact ω₀ the L and C sensitivities of |H|
        // are legitimately zero (stationary point) and FD returns pure
        // truncation noise, which is not a comparison.
        let omega = 0.8 / (l * cval).sqrt();
        let sens = ac_sensitivities(&ckt, src, omega, out).unwrap();

        let hmag = |r: f64, l: f64, cv: f64| {
            let (ckt, src, out) = build(r, l, cv);
            super::super::ac::ac_response(&ckt, src, omega)
                .unwrap()
                .node_voltages[out]
                .abs()
        };

        let rel = 1e-6;
        let fd = [
            0.0, // source amplitude handled below
            (hmag(r * (1.0 + rel), l, cval) - hmag(r * (1.0 - rel), l, cval)) / (2.0 * r * rel),
            (hmag(r, l * (1.0 + rel), cval) - hmag(r, l * (1.0 - rel), cval)) / (2.0 * l * rel),
            (hmag(r, l, cval * (1.0 + rel)) - hmag(r, l, cval * (1.0 - rel))) / (2.0 * cval * rel),
        ];
        for (id, fdi) in fd.iter().enumerate().skip(1) {
            let ad = sens.d_magnitude(id);
            let scale = fdi.abs().max(ad.abs()).max(1e-12);
            assert!(
                (fdi - ad).abs() / scale < 1e-5,
                "device {id}: adjoint {ad} vs FD {fdi}"
            );
        }
        // Source amplitude: H scales linearly, so d|H|/dA at A=1 is |H|.
        assert!((sens.d_magnitude(0) - sens.h.abs()).abs() < 1e-9);
        // No diode here, so nothing is deferred.
        assert!(sens.deferred.is_empty());
    }

    #[test]
    fn ac_diode_slot_is_flagged_deferred_not_silently_zero() {
        // A diode's AC sensitivity slot is a placeholder at M0. It must be
        // reported as deferred so a caller can tell it apart from a genuine
        // zero — the linear elements around it are not deferred.
        let mut c = Circuit::new();
        let vin = c.node();
        let out = c.node();
        let src = c.add(Device::VSource {
            p: vin,
            n: 0,
            v: 0.0,
        });
        let rid = c.add(Device::Resistor {
            p: vin,
            n: out,
            r: 1_000.0,
        });
        let did = c.add(Device::Diode {
            p: out,
            n: 0,
            model: DiodeModel::silicon(),
        });
        let sens = ac_sensitivities(&c, src, 2.0 * std::f64::consts::PI * 1_000.0, out).unwrap();
        assert!(sens.is_deferred(did), "diode slot must be flagged deferred");
        assert!(
            !sens.is_deferred(rid),
            "resistor slot is computed, not deferred"
        );
        assert!(
            !sens.is_deferred(src),
            "source slot is computed, not deferred"
        );
        assert_eq!(sens.deferred, vec![did]);
    }
}
