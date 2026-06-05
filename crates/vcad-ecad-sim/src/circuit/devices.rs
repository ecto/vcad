//! Lumped two-terminal devices and their MNA "stamps".
//!
//! Sign convention: each device has a `p` (positive) and `n` (negative) terminal,
//! and a device current defined as flowing **from `p` to `n` through the device**.
//! Node `0` is ground and never gets a matrix row/column.

/// A circuit device connecting two nodes. In every variant `p`/`n` are the
/// positive/negative terminal node ids.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(missing_docs)] // p/n are terminals; the remaining scalar is named per its unit.
pub enum Device {
    /// Ideal resistor; `r` in Ω.
    Resistor { p: usize, n: usize, r: f64 },
    /// Ideal capacitor; `c` in F. Backward-Euler companion model.
    Capacitor { p: usize, n: usize, c: f64 },
    /// Ideal inductor; `l` in H. Backward-Euler companion model.
    Inductor { p: usize, n: usize, l: f64 },
    /// Ideal independent voltage source; enforces `V(p) − V(n) = v`.
    VSource { p: usize, n: usize, v: f64 },
    /// Ideal independent current source; pushes `i` A into `p`, out of `n`.
    ISource { p: usize, n: usize, i: f64 },
}

/// Conductance stamp for a 2-terminal element between `p` and `n`.
fn stamp_conductance(a: &mut [f64], m: usize, p: usize, n: usize, g: f64) {
    let mut add = |i: usize, j: usize, v: f64| {
        if i != 0 && j != 0 {
            a[(i - 1) * m + (j - 1)] += v;
        }
    };
    add(p, p, g);
    add(p, n, -g);
    add(n, p, -g);
    add(n, n, g);
}

/// Inject a current `i` into node `p` and out of node `n` (a current source).
fn inject(rhs: &mut [f64], p: usize, n: usize, i: f64) {
    if p != 0 {
        rhs[p - 1] += i;
    }
    if n != 0 {
        rhs[n - 1] -= i;
    }
}

impl Device {
    /// Whether this device needs its own MNA branch-current unknown.
    pub fn needs_branch(&self) -> bool {
        matches!(self, Device::VSource { .. })
    }

    /// Stamp this device's contribution into the MNA matrix `a` and RHS `rhs`.
    ///
    /// - `m` is the system dimension, `nn` the node count (incl. ground).
    /// - `branch` is a running branch-index counter; branch devices consume one.
    /// - `cap_v` / `ind_i` are this device's companion history (prev cap voltage /
    ///   prev inductor current); ignored by non-reactive devices.
    /// - `guess` is the current Newton node-voltage estimate (used by nonlinear
    ///   devices added later).
    #[allow(clippy::too_many_arguments)]
    pub fn stamp(
        &self,
        a: &mut [f64],
        rhs: &mut [f64],
        m: usize,
        nn: usize,
        branch: &mut usize,
        dt: f64,
        cap_v: f64,
        ind_i: f64,
        _guess: &[f64],
    ) {
        match *self {
            Device::Resistor { p, n, r } => {
                stamp_conductance(a, m, p, n, 1.0 / r);
            }
            Device::Capacitor { p, n, c } => {
                let gc = c / dt;
                stamp_conductance(a, m, p, n, gc);
                // companion current source i_eq = gc·v_prev, injected into p
                inject(rhs, p, n, gc * cap_v);
            }
            Device::Inductor { p, n, l } => {
                let geq = dt / l;
                stamp_conductance(a, m, p, n, geq);
                // companion: i_L = geq·v + i_prev; the i_prev term leaves p
                inject(rhs, p, n, -ind_i);
            }
            Device::VSource { p, n, v } => {
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
            }
            Device::ISource { p, n, i } => {
                inject(rhs, p, n, i);
            }
        }
    }

    /// Device current (A, `p`→`n`) computed directly from node voltages. Only
    /// meaningful for memoryless, non-branch devices (R, I); reactive and branch
    /// devices report their current through other channels.
    pub fn current(&self, node_v: &[f64]) -> f64 {
        match *self {
            Device::Resistor { p, n, r } => (node_v[p] - node_v[n]) / r,
            Device::ISource { i, .. } => i,
            _ => 0.0,
        }
    }

    /// This device's primary scalar (resistance, source value, …).
    pub fn primary(&self) -> f64 {
        match *self {
            Device::Resistor { r, .. } => r,
            Device::Capacitor { c, .. } => c,
            Device::Inductor { l, .. } => l,
            Device::VSource { v, .. } => v,
            Device::ISource { i, .. } => i,
        }
    }

    /// Set this device's primary scalar, for live driving (switch, PWM, scrub).
    pub fn set_primary(&mut self, value: f64) {
        match self {
            Device::Resistor { r, .. } => *r = value,
            Device::Capacitor { c, .. } => *c = value,
            Device::Inductor { l, .. } => *l = value,
            Device::VSource { v, .. } => *v = value,
            Device::ISource { i, .. } => *i = value,
        }
    }
}
