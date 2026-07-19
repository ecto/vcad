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
//! source values). Devices without a primary scalar use the slot for their
//! leading parameter — diode/BJT → Is, MOSFET → kp — with the second
//! parameter (vt0, βF) in [`DcSensitivity::gradient_aux`]. At AC the diode
//! slot carries the full dH/dIs including the operating-point chain term;
//! transistor AC slots are deferred placeholders (see
//! [`AcSensitivity::deferred`]).

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
    /// Devices without a primary scalar use the slot for their leading
    /// parameter: diode → d/dIs, MOSFET → d/dkp, BJT → d/dIs.
    pub gradient: Vec<f64>,
    /// Second-parameter gradient slot, one entry per device in device order:
    /// MOSFET → d/dvt0, BJT → d/dβF; 0 for every other device kind.
    pub gradient_aux: Vec<f64>,
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
    // nonlinear device currents). Each device touches at most three rows.
    let mut gradient = vec![0.0f64; circuit.devices.len()];
    let mut gradient_aux = vec![0.0f64; circuit.devices.len()];
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
            Device::Mosfet { d, g, s, model } => {
                // F_d += ids, F_s −= ids ⇒ dy/dp = −(λ_d − λ_s)·∂ids/∂p.
                let (_, _, _, dkp, dvt0) = model.eval(v[g] - v[s], v[d] - v[s]);
                gradient_aux[id] = -(lam(d) - lam(s)) * dvt0;
                -(lam(d) - lam(s)) * dkp
            }
            Device::Bjt { c, b, e, model } => {
                // Three branch currents, all ∝ Is: rows (c,e) get iT,
                // (b,e) get i_be, (b,c) get i_bc.
                let ev = model.eval(v[b] - v[e], v[b] - v[c]);
                gradient_aux[id] = (lam(b) - lam(e)) * ev.ibe / model.beta_f;
                -((lam(c) - lam(e)) * ev.it
                    + (lam(b) - lam(e)) * ev.ibe
                    + (lam(b) - lam(c)) * ev.ibc)
                    / model.is
            }
            Device::Motor { .. } => unreachable!("rejected by the DC solve"),
        };
    }

    Ok(DcSensitivity {
        value: op.node_voltages[out_node],
        gradient,
        gradient_aux,
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
    use super::devices::{stamp_conductance, stamp_vccs};
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
        Device::Mosfet { d, g, s, model } => {
            let v = &op.node_voltages;
            let (_, gm, gds, _, _) = model.eval(v[g] - v[s], v[d] - v[s]);
            stamp_conductance(a, m, d, s, gds);
            stamp_vccs(a, m, d, s, g, s, gm);
        }
        Device::Bjt { c, b, e, model } => {
            let v = &op.node_voltages;
            let ev = model.eval(v[b] - v[e], v[b] - v[c]);
            stamp_conductance(a, m, b, e, ev.gpi);
            stamp_conductance(a, m, b, c, ev.gmu);
            stamp_vccs(a, m, c, e, b, e, ev.gmf);
            stamp_vccs(a, m, c, e, b, c, -ev.gmr);
        }
        Device::Motor { .. } => unreachable!(),
    }
}

/// An AC transfer value and its complex gradient per device primary.
#[derive(Debug, Clone, PartialEq)]
pub struct AcSensitivity {
    /// The complex transfer H(jω) = V(out) per unit source amplitude.
    pub h: Cplx,
    /// dH/dp per device primary, in device order. Diode slots carry the full
    /// dH/dIs including the operating-point chain term (the op point shifts
    /// when a parameter changes, which shifts the small-signal conductance).
    /// A slot listed in [`AcSensitivity::deferred`] is a **placeholder
    /// zero**, not a computed value — see [`AcSensitivity::is_deferred`].
    pub gradient: Vec<Cplx>,
    /// Device ids whose gradient slot is a placeholder, not a computed
    /// sensitivity. As of M1 this is exactly the transistors (MOSFET / BJT):
    /// their AC sensitivities need d(gm, gds, …)/d(op point) — second
    /// derivatives of the device models — which are not implemented yet.
    /// Diode slots were deferred at M0 and are now computed. Callers that
    /// must distinguish "sensitivity is genuinely zero" from "sensitivity was
    /// not computed" consult this; the list shrinks without an API break.
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
            // A diode's H-dependence on its own Is at a *fixed* op point runs
            // entirely through g_d; both the explicit ∂g_d/∂Is term and the
            // op-point chain term are added in the pass below.
            Device::Diode { .. } => Cplx::ZERO,
            // Transistor AC sensitivities need d(gm, gds, gπ, gµ)/d(op) —
            // second derivatives of the models. Deferred honestly.
            Device::Mosfet { .. } | Device::Bjt { .. } => {
                deferred.push(id);
                Cplx::ZERO
            }
            Device::Motor { .. } => unreachable!("rejected by build_and_solve"),
        };
    }

    // Operating-point chain term (closes the M0 gap): each diode's
    // small-signal conductance g_d = (Is/vte)·e^{v_d/vte} depends on every
    // parameter through the DC junction voltage v_d(p). The total derivative
    //
    //   dH/dpᵢ = ∂H/∂pᵢ + Σ_diodes (∂H/∂g_d)·[ (g_d/vte)·dv_d/dpᵢ + δ·g_d/Is ]
    //
    // with ∂H/∂g_d from the AC adjoint (the conductance stamp pattern) and
    // dv_d/dpᵢ from the DC adjoint (`dc_sensitivities` at the diode's nodes).
    // δ = 1 only for the diode's own Is slot (the explicit ∂g_d/∂Is = g_d/Is).
    let diodes: Vec<(usize, usize, usize, f64, f64)> = circuit
        .devices
        .iter()
        .enumerate()
        .filter_map(|(id, d)| match *d {
            Device::Diode { p, n, model } => Some((id, p, n, model.n * VT, model.is)),
            _ => None,
        })
        .collect();
    if !diodes.is_empty() {
        let op = sys
            .op
            .as_ref()
            .expect("nonlinear network has an op point")
            .clone();
        // DC node-voltage gradients, one adjoint solve per distinct diode node.
        let ndev = circuit.devices.len();
        let mut node_grad: Vec<Option<Vec<f64>>> = vec![None; nn];
        node_grad[0] = Some(vec![0.0; ndev]); // ground never moves
        for &(_, p, n, _, _) in &diodes {
            for node in [p, n] {
                if node_grad[node].is_none() {
                    node_grad[node] = Some(
                        dc_sensitivities(circuit, node)
                            .map_err(AcError::Dc)?
                            .gradient,
                    );
                }
            }
        }
        for &(jid, p, n, vte, is) in &diodes {
            let vd = op.node_voltages[p] - op.node_voltages[n];
            let gd = (is / vte) * (vd / vte).min(60.0).exp();
            // ∂H/∂g_d over the conductance stamp pattern.
            let dh_dg = -((lam(p) - lam(n)) * (xv(p) - xv(n)));
            let gp = node_grad[p].as_ref().expect("computed above");
            let gn = node_grad[n].as_ref().expect("computed above");
            for i in 0..ndev {
                if deferred.contains(&i) {
                    continue; // keep placeholder slots exactly zero
                }
                let mut dg = (gd / vte) * (gp[i] - gn[i]);
                if i == jid {
                    dg += gd / is;
                }
                // The driven source's slot means d/d(AC amplitude), not
                // d/d(DC value) — the op point does not depend on the AC
                // amplitude, so no chain term lands there.
                if i == source {
                    continue;
                }
                gradient[i] += Cplx::real(dg) * dh_dg;
            }
        }
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
    fn ac_diode_chain_term_matches_fd() {
        // DC-biased diode with a bypass cap, driven by the (DC-offset) source:
        // the small-signal conductance g_d moves when R or Is moves, so the
        // AC sensitivities of |H| carry the operating-point chain term. The
        // diode slot must no longer be deferred — M0's placeholder is closed.
        let (vdc, r, cval) = (5.0, 1_000.0, 1e-7);
        let build = |r: f64, is: f64| {
            let mut c = Circuit::new();
            let vin = c.node();
            let out = c.node();
            let src = c.add(Device::VSource {
                p: vin,
                n: 0,
                v: vdc,
            });
            c.add(Device::Resistor { p: vin, n: out, r });
            c.add(Device::Diode {
                p: out,
                n: 0,
                model: DiodeModel { is, n: 1.0 },
            });
            c.add(Device::Capacitor {
                p: out,
                n: 0,
                c: cval,
            });
            (c, src, out)
        };
        let is0 = DiodeModel::silicon().is;
        let omega = 2.0 * std::f64::consts::PI * 10_000.0;
        let (ckt, src, out) = build(r, is0);
        let sens = ac_sensitivities(&ckt, src, omega, out).unwrap();
        assert!(sens.deferred.is_empty(), "diode slot is computed as of M1");

        let hmag = |r: f64, is: f64| {
            let (ckt, src, out) = build(r, is);
            super::super::ac::ac_response(&ckt, src, omega)
                .unwrap()
                .node_voltages[out]
                .abs()
        };
        // Resistor slot (id 1): pure chain term — H depends on R directly
        // *and* through the op-point shift of g_d.
        let rel = 1e-6;
        let fd_r = (hmag(r * (1.0 + rel), is0) - hmag(r * (1.0 - rel), is0)) / (2.0 * r * rel);
        let ad_r = sens.d_magnitude(1);
        assert!(
            (fd_r - ad_r).abs() / fd_r.abs().max(ad_r.abs()) < 1e-4,
            "R slot: adjoint {ad_r} vs FD {fd_r}"
        );
        // Diode slot (id 2): explicit ∂g/∂Is + chain through v_d(Is).
        let fd_is = (hmag(r, is0 * (1.0 + rel)) - hmag(r, is0 * (1.0 - rel))) / (2.0 * is0 * rel);
        let ad_is = sens.d_magnitude(2);
        assert!(
            (fd_is - ad_is).abs() / fd_is.abs().max(ad_is.abs()) < 1e-4,
            "Is slot: adjoint {ad_is} vs FD {fd_is}"
        );
    }

    #[test]
    fn dc_adjoint_matches_fd_through_the_mosfet() {
        use crate::circuit::MosfetModel;
        // Common-source stage: VDD — Rd — drain, gate driven by a divider.
        let build = |kp: f64, vt0: f64| {
            let mut c = Circuit::new();
            let vdd = c.node();
            let gate = c.node();
            let drain = c.node();
            c.add(Device::VSource {
                p: vdd,
                n: 0,
                v: 12.0,
            });
            c.add(Device::Resistor {
                p: vdd,
                n: gate,
                r: 10_000.0,
            });
            c.add(Device::Resistor {
                p: gate,
                n: 0,
                r: 3_300.0,
            });
            c.add(Device::Resistor {
                p: vdd,
                n: drain,
                r: 1_000.0,
            });
            c.add(Device::Mosfet {
                d: drain,
                g: gate,
                s: 0,
                model: MosfetModel {
                    kp,
                    vt0,
                    lambda: 0.02,
                    polarity: crate::circuit::Polarity::N,
                },
            });
            (c, drain)
        };
        let (kp0, vt00) = (0.05, 2.0);
        let (ckt, out) = build(kp0, vt00);
        let sens = dc_sensitivities(&ckt, out).unwrap();

        // Linear slots against the generic FD helper.
        for id in 0..4 {
            let fd = fd_dc(&ckt, out, id, 1e-6);
            let ad = sens.gradient[id];
            let scale = fd.abs().max(ad.abs()).max(1e-12);
            assert!(
                (fd - ad).abs() / scale < 1e-4,
                "device {id}: adjoint {ad} vs FD {fd}"
            );
        }
        // kp (leading slot) and vt0 (aux slot) by direct rebuild.
        let vout = |kp: f64, vt0: f64| {
            let (c, out) = build(kp, vt0);
            operating_point(&c).unwrap().node_voltages[out]
        };
        let rel = 1e-6;
        let fd_kp =
            (vout(kp0 * (1.0 + rel), vt00) - vout(kp0 * (1.0 - rel), vt00)) / (2.0 * kp0 * rel);
        let fd_vt =
            (vout(kp0, vt00 * (1.0 + rel)) - vout(kp0, vt00 * (1.0 - rel))) / (2.0 * vt00 * rel);
        let (ad_kp, ad_vt) = (sens.gradient[4], sens.gradient_aux[4]);
        assert!(
            (fd_kp - ad_kp).abs() / fd_kp.abs().max(ad_kp.abs()) < 1e-4,
            "kp: adjoint {ad_kp} vs FD {fd_kp}"
        );
        assert!(
            (fd_vt - ad_vt).abs() / fd_vt.abs().max(ad_vt.abs()) < 1e-4,
            "vt0: adjoint {ad_vt} vs FD {fd_vt}"
        );
    }

    #[test]
    fn dc_adjoint_matches_fd_through_the_bjt() {
        use crate::circuit::BjtModel;
        // Classic four-resistor bias: divider on the base, Rc and Re.
        let build = |is: f64, bf: f64| {
            let mut c = Circuit::new();
            let vcc = c.node();
            let base = c.node();
            let coll = c.node();
            let emit = c.node();
            c.add(Device::VSource {
                p: vcc,
                n: 0,
                v: 12.0,
            });
            c.add(Device::Resistor {
                p: vcc,
                n: base,
                r: 47_000.0,
            });
            c.add(Device::Resistor {
                p: base,
                n: 0,
                r: 10_000.0,
            });
            c.add(Device::Resistor {
                p: vcc,
                n: coll,
                r: 2_200.0,
            });
            c.add(Device::Resistor {
                p: emit,
                n: 0,
                r: 1_000.0,
            });
            c.add(Device::Bjt {
                c: coll,
                b: base,
                e: emit,
                model: BjtModel {
                    is,
                    beta_f: bf,
                    beta_r: 1.0,
                    polarity: crate::circuit::Polarity::N,
                },
            });
            (c, coll)
        };
        let (is0, bf0) = (1e-14, 100.0);
        let (ckt, out) = build(is0, bf0);
        let sens = dc_sensitivities(&ckt, out).unwrap();

        for id in 0..5 {
            let fd = fd_dc(&ckt, out, id, 1e-6);
            let ad = sens.gradient[id];
            let scale = fd.abs().max(ad.abs()).max(1e-12);
            assert!(
                (fd - ad).abs() / scale < 1e-4,
                "device {id}: adjoint {ad} vs FD {fd}"
            );
        }
        let vout = |is: f64, bf: f64| {
            let (c, out) = build(is, bf);
            operating_point(&c).unwrap().node_voltages[out]
        };
        let rel = 1e-5;
        let fd_is =
            (vout(is0 * (1.0 + rel), bf0) - vout(is0 * (1.0 - rel), bf0)) / (2.0 * is0 * rel);
        let fd_bf =
            (vout(is0, bf0 * (1.0 + rel)) - vout(is0, bf0 * (1.0 - rel))) / (2.0 * bf0 * rel);
        let (ad_is, ad_bf) = (sens.gradient[5], sens.gradient_aux[5]);
        assert!(
            (fd_is - ad_is).abs() / fd_is.abs().max(ad_is.abs()) < 1e-4,
            "Is: adjoint {ad_is} vs FD {fd_is}"
        );
        assert!(
            (fd_bf - ad_bf).abs() / fd_bf.abs().max(ad_bf.abs()) < 1e-4,
            "betaF: adjoint {ad_bf} vs FD {fd_bf}"
        );
    }

    #[test]
    fn ac_transistor_slots_are_flagged_deferred() {
        use crate::circuit::MosfetModel;
        // Transistor AC sensitivities need model second derivatives —
        // deferred honestly, distinguishable from a genuine zero.
        let mut c = Circuit::new();
        let vdd = c.node();
        let gate = c.node();
        let drain = c.node();
        c.add(Device::VSource {
            p: vdd,
            n: 0,
            v: 12.0,
        });
        let src = c.add(Device::VSource {
            p: gate,
            n: 0,
            v: 3.0,
        });
        let rid = c.add(Device::Resistor {
            p: vdd,
            n: drain,
            r: 1_000.0,
        });
        let mid = c.add(Device::Mosfet {
            d: drain,
            g: gate,
            s: 0,
            model: MosfetModel::nmos(),
        });
        let sens = ac_sensitivities(&c, src, 1e4, drain).unwrap();
        assert_eq!(sens.deferred, vec![mid]);
        assert!(!sens.is_deferred(rid));
    }
}
