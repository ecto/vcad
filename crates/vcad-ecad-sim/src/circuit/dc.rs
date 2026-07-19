//! DC operating-point analysis.
//!
//! Solves the static network: capacitors open, inductors short (each carries
//! an MNA branch-current unknown enforcing `v(p) − v(n) = 0`), sources at
//! their DC values, nonlinear devices (the Shockley diode) by Newton–Raphson
//! with `pnjlim` junction limiting.
//!
//! **gmin stepping** is the convergence aid it is, not physics: a shunt
//! conductance `gmin` from every node to ground is ramped 1e-3 → 1e-12 and
//! then removed entirely, each stage warm-starting the next (the standard
//! SPICE continuation method — Nagel, UCB ERL-M520, 1975, §6). The final
//! solve runs at `gmin = 0`, so converged answers are exact for the network
//! as written, not the gmin-augmented one. Fail-closed: if any stage fails
//! to converge, the whole solve is an error — there is no "best effort"
//! operating point.

use super::devices::{inject, stamp_conductance};
use super::linalg::solve_dense;
use super::{Circuit, Device};

/// Why a DC solve failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DcError {
    /// The MNA matrix was singular (floating node, source loop, …).
    Singular,
    /// Newton–Raphson failed to converge within the iteration budget.
    NoConvergence,
    /// A device kind this analysis does not support (e.g. `Motor`, whose DC
    /// state couples to a mechanical equilibrium).
    Unsupported(&'static str),
}

impl std::fmt::Display for DcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DcError::Singular => write!(f, "singular MNA system"),
            DcError::NoConvergence => write!(f, "Newton-Raphson did not converge"),
            DcError::Unsupported(what) => write!(f, "unsupported device in DC analysis: {what}"),
        }
    }
}

impl std::error::Error for DcError {}

/// A converged DC operating point.
#[derive(Debug, Clone, PartialEq)]
pub struct DcSolution {
    /// Voltage at each node (`[0]` is ground, 0 V).
    pub node_voltages: Vec<f64>,
    /// Current (A, `p`→`n`) through each device. Capacitors carry 0.
    pub device_currents: Vec<f64>,
    /// Newton iterations used by the final (`gmin = 0`) stage.
    pub newton_iterations: usize,
    /// Tellegen residual Σ v·i over all devices (W). Zero for an exact
    /// solution; the magnitude measures solver error only.
    pub power_balance_w: f64,
}

/// Convergence: max node-voltage delta below `RELTOL·max|v| + VNTOL`
/// (scale-invariant — a 1 MV network and a 1 mV network stop equally well).
const RELTOL: f64 = 1e-12;
const VNTOL: f64 = 1e-12;
const MAX_NEWTON: usize = 200;

/// Number of MNA branch unknowns at DC: voltage sources and inductor shorts.
fn dc_branches(circuit: &Circuit) -> usize {
    circuit
        .devices
        .iter()
        .filter(|d| matches!(d, Device::VSource { .. } | Device::Inductor { .. }))
        .count()
}

/// Solve the DC operating point of `circuit`.
pub fn operating_point(circuit: &Circuit) -> Result<DcSolution, DcError> {
    if circuit
        .devices
        .iter()
        .any(|d| matches!(d, Device::Motor { .. }))
    {
        return Err(DcError::Unsupported("Motor"));
    }

    let nn = circuit.num_nodes;
    let nb = dc_branches(circuit);
    let m = (nn - 1) + nb;
    if m == 0 {
        return Ok(DcSolution {
            node_voltages: vec![0.0; nn],
            device_currents: vec![0.0; circuit.devices.len()],
            newton_iterations: 0,
            power_balance_w: 0.0,
        });
    }

    // gmin ladder: heavy shunt first, then relax, then remove entirely.
    let gmins = [1e-3, 1e-6, 1e-9, 1e-12, 0.0];
    let mut node_v = vec![0.0f64; nn];
    let mut nl_state = vec![[0.0f64; 2]; circuit.devices.len()];
    let mut branch_i = vec![0.0f64; nb];
    let mut final_iters = 0usize;

    for &gmin in &gmins {
        let mut converged = false;
        for iter in 0..MAX_NEWTON {
            let mut a = vec![0.0f64; m * m];
            let mut rhs = vec![0.0f64; m];
            let mut branch = 0usize;

            for (dev, nl) in circuit.devices.iter().zip(nl_state.iter_mut()) {
                stamp_dc(dev, &mut a, &mut rhs, m, nn, &mut branch, &node_v, nl);
            }
            if gmin > 0.0 {
                for node in 1..nn {
                    a[(node - 1) * m + (node - 1)] += gmin;
                }
            }

            let solution = solve_dense(&mut a, &mut rhs, m).ok_or(DcError::Singular)?;

            let mut next = vec![0.0; nn];
            next[1..nn].copy_from_slice(&solution[..(nn - 1)]);
            branch_i.copy_from_slice(&solution[(nn - 1)..]);

            let mut delta = 0.0f64;
            let mut vmax = 0.0f64;
            for node in 1..nn {
                delta = delta.max((next[node] - node_v[node]).abs());
                vmax = vmax.max(next[node].abs());
            }
            node_v = next;

            if delta < RELTOL * vmax + VNTOL {
                converged = true;
                final_iters = iter + 1;
                break;
            }
        }
        if !converged {
            return Err(DcError::NoConvergence);
        }
    }

    // Device currents from the converged voltages + branch unknowns.
    let mut device_currents = vec![0.0f64; circuit.devices.len()];
    let mut b = 0usize;
    for (id, dev) in circuit.devices.iter().enumerate() {
        match *dev {
            Device::Resistor { p, n, r } => device_currents[id] = (node_v[p] - node_v[n]) / r,
            Device::Capacitor { .. } => device_currents[id] = 0.0,
            Device::Inductor { .. } | Device::VSource { .. } => {
                device_currents[id] = branch_i[b];
                b += 1;
            }
            Device::ISource { i, .. } => device_currents[id] = i,
            Device::Diode { p, n, model } => {
                device_currents[id] = model.current(node_v[p] - node_v[n]);
            }
            Device::Mosfet { .. } | Device::Bjt { .. } => {
                device_currents[id] = dev.current(&node_v);
            }
            Device::Motor { .. } => unreachable!("rejected above"),
        }
    }

    let power_balance_w = circuit
        .devices
        .iter()
        .enumerate()
        .map(|(id, d)| d.power(&node_v, device_currents[id]))
        .sum();

    Ok(DcSolution {
        node_voltages: node_v,
        device_currents,
        newton_iterations: final_iters,
        power_balance_w,
    })
}

/// Stamp one device into the DC MNA system. `nl_state` carries the device's
/// limited nonlinear voltages across Newton iterations (diode: `[v_d, –]`,
/// MOSFET: `[vgs, vds]`, BJT: `[vbe, vbc]`).
#[allow(clippy::too_many_arguments)]
fn stamp_dc(
    dev: &Device,
    a: &mut [f64],
    rhs: &mut [f64],
    m: usize,
    nn: usize,
    branch: &mut usize,
    guess: &[f64],
    nl_state: &mut [f64; 2],
) {
    let mut stamp_branch = |a: &mut [f64], rhs: &mut [f64], p: usize, n: usize, v: f64| {
        let br = (nn - 1) + *branch;
        *branch += 1;
        if p != 0 {
            a[(p - 1) * m + br] += 1.0;
            a[br * m + (p - 1)] += 1.0;
        }
        if n != 0 {
            a[(n - 1) * m + br] -= 1.0;
            a[br * m + (n - 1)] -= 1.0;
        }
        rhs[br] += v;
    };

    match *dev {
        Device::Resistor { p, n, r } => stamp_conductance(a, m, p, n, 1.0 / r),
        Device::Capacitor { .. } => {} // open at DC
        Device::Inductor { p, n, .. } => stamp_branch(a, rhs, p, n, 0.0), // short at DC
        Device::VSource { p, n, v } => stamp_branch(a, rhs, p, n, v),
        Device::ISource { p, n, i } => inject(rhs, p, n, i),
        Device::Diode { p, n, model } => {
            // Same linearization + pnjlim limiting as the transient path.
            let vte = model.n * super::devices::VT;
            let vcrit = vte * (vte / (std::f64::consts::SQRT_2 * model.is)).ln();
            let vd_raw = guess[p] - guess[n];
            let vd = super::devices::pnjlim(vd_raw, nl_state[0], vte, vcrit);
            nl_state[0] = vd;
            let ev = (vd / vte).min(60.0).exp();
            let id = model.is * (ev - 1.0);
            let geq = (model.is / vte) * ev;
            let ieq = id - geq * vd;
            stamp_conductance(a, m, p, n, geq);
            inject(rhs, p, n, -ieq);
        }
        Device::Mosfet { d, g, s, model } => {
            super::devices::stamp_mosfet(a, rhs, m, d, g, s, &model, nl_state, guess);
        }
        Device::Bjt { c, b, e, model } => {
            super::devices::stamp_bjt(a, rhs, m, c, b, e, &model, nl_state, guess);
        }
        Device::Motor { .. } => unreachable!("rejected before stamping"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voltage_divider_exact() {
        let mut c = Circuit::new();
        let vin = c.node();
        let out = c.node();
        c.add(Device::VSource {
            p: vin,
            n: 0,
            v: 12.0,
        });
        c.add(Device::Resistor {
            p: vin,
            n: out,
            r: 3_000.0,
        });
        c.add(Device::Resistor {
            p: out,
            n: 0,
            r: 1_000.0,
        });
        let sol = operating_point(&c).unwrap();
        assert!((sol.node_voltages[out] - 3.0).abs() < 1e-12);
        assert!(sol.power_balance_w.abs() < 1e-12);
    }

    #[test]
    fn inductor_is_a_dc_short() {
        // V — R — L — ground: all the drop lands on R, full current flows.
        let mut c = Circuit::new();
        let vin = c.node();
        let mid = c.node();
        c.add(Device::VSource {
            p: vin,
            n: 0,
            v: 5.0,
        });
        c.add(Device::Resistor {
            p: vin,
            n: mid,
            r: 100.0,
        });
        let l = c.add(Device::Inductor {
            p: mid,
            n: 0,
            l: 1e-3,
        });
        let sol = operating_point(&c).unwrap();
        assert!(sol.node_voltages[mid].abs() < 1e-12);
        assert!((sol.device_currents[l] - 0.05).abs() < 1e-12);
    }

    #[test]
    fn capacitor_is_a_dc_open() {
        // V — R — C: no current flows, the cap node sits at the source voltage.
        let mut c = Circuit::new();
        let vin = c.node();
        let top = c.node();
        c.add(Device::VSource {
            p: vin,
            n: 0,
            v: 9.0,
        });
        let r = c.add(Device::Resistor {
            p: vin,
            n: top,
            r: 1_000.0,
        });
        c.add(Device::Capacitor {
            p: top,
            n: 0,
            c: 1e-6,
        });
        let sol = operating_point(&c).unwrap();
        assert!((sol.node_voltages[top] - 9.0).abs() < 1e-9);
        assert!(sol.device_currents[r].abs() < 1e-12);
    }

    #[test]
    fn cmos_inverter_transfers_and_converges_through_the_gmin_ladder() {
        // NMOS pull-down + PMOS pull-up, 5 V rail. The strongly nonlinear
        // switching region is exactly what the gmin ladder exists for.
        use crate::circuit::MosfetModel;
        let solve = |vin_v: f64| {
            let mut c = Circuit::new();
            let vdd = c.node();
            let vin = c.node();
            let out = c.node();
            c.add(Device::VSource {
                p: vdd,
                n: 0,
                v: 5.0,
            });
            c.add(Device::VSource {
                p: vin,
                n: 0,
                v: vin_v,
            });
            c.add(Device::Mosfet {
                d: out,
                g: vin,
                s: 0,
                model: MosfetModel::nmos(),
            });
            c.add(Device::Mosfet {
                d: out,
                g: vin,
                s: vdd,
                model: MosfetModel::pmos(),
            });
            // Weak load so the off-state output node is never floating.
            c.add(Device::Resistor {
                p: out,
                n: 0,
                r: 1e9,
            });
            operating_point(&c).unwrap().node_voltages[out]
        };
        // Input low → output at the rail; input high → output at ground.
        assert!(solve(0.0) > 4.9, "low in must pull out to VDD");
        assert!(solve(5.0) < 0.1, "high in must pull out to ground");
        // Midpoint: both devices on; with symmetric N/P models the output
        // sits at the switching point near VDD/2.
        let mid = solve(2.5);
        assert!(
            (1.5..3.5).contains(&mid),
            "switching-point output {mid} should be near VDD/2"
        );
        // The transfer curve is monotonically non-increasing.
        let vs: Vec<f64> = (0..=10).map(|k| solve(0.5 * k as f64)).collect();
        for w in vs.windows(2) {
            assert!(w[1] <= w[0] + 1e-9, "inverter transfer must be monotone");
        }
    }

    #[test]
    fn motor_is_rejected() {
        let mut c = Circuit::new();
        let vin = c.node();
        c.add(Device::VSource {
            p: vin,
            n: 0,
            v: 5.0,
        });
        c.add(Device::Motor {
            p: vin,
            n: 0,
            params: crate::circuit::MotorParams::small_dc(),
        });
        assert_eq!(operating_point(&c), Err(DcError::Unsupported("Motor")));
    }
}
