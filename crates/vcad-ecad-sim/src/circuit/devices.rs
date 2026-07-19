//! Lumped two-terminal devices and their MNA "stamps".
//!
//! Sign convention: each device has a `p` (positive) and `n` (negative) terminal,
//! and a device current defined as flowing **from `p` to `n` through the device**.
//! Node `0` is ground and never gets a matrix row/column.

/// Thermal voltage at ~300 K (kT/q), in volts.
pub const VT: f64 = 0.025_852;

/// Shockley diode model: `i = Is·(exp(v / (n·Vt)) − 1)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiodeModel {
    /// Saturation current Is (A).
    pub is: f64,
    /// Emission/ideality coefficient n.
    pub n: f64,
}

impl DiodeModel {
    /// A generic small-signal silicon diode (Vf ≈ 0.65 V).
    pub fn silicon() -> Self {
        DiodeModel { is: 1e-14, n: 1.0 }
    }

    /// A red LED (Vf ≈ 1.8 V at ~10 mA). Higher ideality + tiny Is push the knee up.
    pub fn led() -> Self {
        DiodeModel { is: 1e-18, n: 1.8 }
    }

    /// Thermal voltage scaled by the ideality coefficient (n·Vt).
    fn vte(&self) -> f64 {
        self.n * VT
    }

    /// Diode current at junction voltage `v` (clamped for numeric safety).
    pub fn current(&self, v: f64) -> f64 {
        let x = (v / self.vte()).min(60.0);
        self.is * (x.exp() - 1.0)
    }
}

/// Channel polarity of a MOSFET (or the junction polarity of a BJT).
///
/// P-type devices are handled by the standard sign transformation: internal
/// junction/channel voltages are negated on the way in, currents negated on
/// the way out, so one set of N-type equations serves both polarities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Polarity {
    /// N-channel MOSFET / NPN BJT.
    N,
    /// P-channel MOSFET / PNP BJT.
    P,
}

impl Polarity {
    /// +1 for N, −1 for P — the sign of the internal↔external transformation.
    fn sign(self) -> f64 {
        match self {
            Polarity::N => 1.0,
            Polarity::P => -1.0,
        }
    }
}

/// Shichman–Hodges level-1 MOSFET (SPICE2: Nagel, UCB ERL-M520, 1975, §2):
/// cutoff / triode / saturation with channel-length modulation.
///
/// `kp` is the full transconductance factor β = KP·W/L (A/V²) — W/L is folded
/// in rather than carried separately. With overdrive `vov = vgs − vt0`:
///
/// ```text
/// cutoff     (vov ≤ 0):        ids = 0
/// triode     (vds < vov):      ids = kp·(vov·vds − vds²/2)·(1 + λ·vds)
/// saturation (vds ≥ vov):      ids = (kp/2)·vov²·(1 + λ·vds)
/// ```
///
/// The model is symmetric: for `vds < 0` the source and drain swap roles
/// internally (standard SPICE practice). For P-channel, pass `vt0` negative
/// (as in SPICE) and [`Polarity::P`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MosfetModel {
    /// Transconductance factor β = KP·W/L (A/V²).
    pub kp: f64,
    /// Threshold voltage (V). Negative for a P-channel device.
    pub vt0: f64,
    /// Channel-length modulation λ (1/V).
    pub lambda: f64,
    /// N- or P-channel.
    pub polarity: Polarity,
}

impl MosfetModel {
    /// A small-signal N-channel enhancement FET (2N7000-ish: Vt ≈ 2 V).
    pub fn nmos() -> Self {
        MosfetModel {
            kp: 0.1,
            vt0: 2.0,
            lambda: 0.01,
            polarity: Polarity::N,
        }
    }

    /// The P-channel complement of [`MosfetModel::nmos`].
    pub fn pmos() -> Self {
        MosfetModel {
            kp: 0.1,
            vt0: -2.0,
            lambda: 0.01,
            polarity: Polarity::P,
        }
    }

    /// Evaluate the model at external `(vgs, vds)`.
    ///
    /// Returns `(ids, gm, gds, dids_dkp, dids_dvt0)`: the drain→source channel
    /// current, its derivatives wrt the external `vgs` / `vds` (the Newton /
    /// small-signal conductances), and its derivatives wrt the parameters
    /// `kp` and `vt0`. Polarity and the `vds < 0` source-drain swap are
    /// handled internally, so all five outputs are in external convention.
    pub fn eval(&self, vgs: f64, vds: f64) -> (f64, f64, f64, f64, f64) {
        let s = self.polarity.sign();
        // Internal N-type frame.
        let (vgs_i, vds_i, vt0_i) = (s * vgs, s * vds, s * self.vt0);
        // Source-drain swap for reverse operation: the channel is symmetric,
        // so evaluate with the roles exchanged and negate the current.
        let swapped = vds_i < 0.0;
        let (vgs_e, vds_e) = if swapped {
            (vgs_i - vds_i, -vds_i)
        } else {
            (vgs_i, vds_i)
        };

        let vov = vgs_e - vt0_i;
        // (f, df/dvgs_e, df/dvds_e, df/dkp, df/dvt0_i) in the swapped frame.
        let (f, fg, fd, fkp, fvt) = if vov <= 0.0 {
            (0.0, 0.0, 0.0, 0.0, 0.0)
        } else if vds_e < vov {
            // Triode.
            let core = self.kp * (vov * vds_e - 0.5 * vds_e * vds_e);
            let clm = 1.0 + self.lambda * vds_e;
            let fg = self.kp * vds_e * clm;
            let fd = self.kp * (vov - vds_e) * clm + core * self.lambda;
            (core * clm, fg, fd, core * clm / self.kp, -fg)
        } else {
            // Saturation.
            let core = 0.5 * self.kp * vov * vov;
            let clm = 1.0 + self.lambda * vds_e;
            let fg = self.kp * vov * clm;
            (
                core * clm,
                fg,
                core * self.lambda,
                core * clm / self.kp,
                -fg,
            )
        };

        // Un-swap: ids_i = −f, and vgs_e/vds_e depend on (vgs_i, vds_i) as
        // vgs_e = vgs_i − vds_i, vds_e = −vds_i.
        let (ids_i, gg_i, gd_i) = if swapped {
            (-f, -fg, fg + fd)
        } else {
            (f, fg, fd)
        };
        let (fkp_i, fvt_i) = if swapped { (-fkp, -fvt) } else { (fkp, fvt) };

        // Un-polarize: ids = s·ids_i; conductances pick up s twice (once from
        // the current, once from the internal voltage) so they are unchanged.
        // dids/dvt0 = s·(∂ids_i/∂vt0_i)·(dvt0_i/dvt0 = s) = ∂ids_i/∂vt0_i… ×s·s.
        (s * ids_i, gg_i, gd_i, s * fkp_i, fvt_i)
    }
}

/// Ebers–Moll BJT (transport form): two junction diodes plus the transport
/// current `iT = Is·(e^{vbe/Vt} − e^{vbc/Vt})` flowing collector→emitter.
///
/// ```text
/// i_be = (Is/βF)·(e^{vbe/Vt} − 1)   (base→emitter)
/// i_bc = (Is/βR)·(e^{vbc/Vt} − 1)   (base→collector)
/// iC = iT − i_bc,  iB = i_be + i_bc,  iE = iT + i_be
/// ```
///
/// For PNP the junction voltages and currents are negated internally
/// ([`Polarity::P`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BjtModel {
    /// Transport saturation current Is (A).
    pub is: f64,
    /// Forward current gain βF.
    pub beta_f: f64,
    /// Reverse current gain βR.
    pub beta_r: f64,
    /// NPN or PNP.
    pub polarity: Polarity,
}

impl BjtModel {
    /// A generic small-signal NPN (2N3904-ish).
    pub fn npn() -> Self {
        BjtModel {
            is: 1e-14,
            beta_f: 100.0,
            beta_r: 1.0,
            polarity: Polarity::N,
        }
    }

    /// A generic small-signal PNP.
    pub fn pnp() -> Self {
        BjtModel {
            is: 1e-14,
            beta_f: 100.0,
            beta_r: 1.0,
            polarity: Polarity::P,
        }
    }
}

/// The three Ebers–Moll branch currents and their junction conductances,
/// all in **external** convention (polarity already applied).
///
/// Branches: `it` flows c→e, `ibe` b→e, `ibc` b→c. Each conductance is the
/// derivative of its branch current wrt the **external** vbe / vbc.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BjtEval {
    pub it: f64,
    pub ibe: f64,
    pub ibc: f64,
    /// ∂it/∂vbe (the forward transconductance gmf).
    pub gmf: f64,
    /// −∂it/∂vbc (the reverse transconductance gmr, positive).
    pub gmr: f64,
    /// ∂ibe/∂vbe (gπ).
    pub gpi: f64,
    /// ∂ibc/∂vbc (gµ).
    pub gmu: f64,
}

impl BjtModel {
    /// Evaluate at external `(vbe, vbc)`.
    pub(crate) fn eval(&self, vbe: f64, vbc: f64) -> BjtEval {
        let s = self.polarity.sign();
        let ebe = (s * vbe / VT).min(60.0).exp();
        let ebc = (s * vbc / VT).min(60.0).exp();
        // Internal currents; external = s·internal. Conductances pick up s
        // twice (current and controlling voltage) → unchanged.
        BjtEval {
            it: s * self.is * (ebe - ebc),
            ibe: s * (self.is / self.beta_f) * (ebe - 1.0),
            ibc: s * (self.is / self.beta_r) * (ebc - 1.0),
            gmf: (self.is / VT) * ebe,
            gmr: (self.is / VT) * ebc,
            gpi: (self.is / (self.beta_f * VT)) * ebe,
            gmu: (self.is / (self.beta_r * VT)) * ebc,
        }
    }

    /// Collector current at external `(vbe, vbc)`.
    pub fn ic(&self, vbe: f64, vbc: f64) -> f64 {
        let e = self.eval(vbe, vbc);
        e.it - e.ibc
    }
}

/// A brushed DC motor / gyrator: couples an electrical winding to a mechanical
/// rotor. The winding (R + L) carries the armature current; the back-EMF is
/// `Ke·ω` and the torque is `Kt·i`. SI units throughout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotorParams {
    /// Winding resistance (Ω).
    pub r: f64,
    /// Winding inductance (H).
    pub l: f64,
    /// Back-EMF constant Ke (V·s/rad).
    pub ke: f64,
    /// Torque constant Kt (N·m/A).
    pub kt: f64,
    /// Rotor moment of inertia J (kg·m²).
    pub j: f64,
    /// Viscous friction b (N·m·s/rad).
    pub b: f64,
    /// External load torque opposing rotation (N·m).
    pub load: f64,
}

impl MotorParams {
    /// A small hobby DC motor (~5 V, no-load ≈ 4700 RPM, stall ≈ 2.5 A at 5 V).
    pub fn small_dc() -> Self {
        MotorParams {
            r: 2.0,
            l: 0.5e-3,
            ke: 0.01,
            kt: 0.01,
            j: 1e-5,
            b: 1e-6,
            load: 0.0,
        }
    }
}

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
    /// Nonlinear diode / LED (Shockley), solved by Newton-Raphson.
    Diode {
        p: usize,
        n: usize,
        model: DiodeModel,
    },
    /// Brushed DC motor (electromechanical gyrator). Needs a branch current and
    /// carries rotor state (updated post-solve).
    Motor {
        p: usize,
        n: usize,
        params: MotorParams,
    },
    /// Level-1 MOSFET (Shichman–Hodges). `d`/`g`/`s` are the drain, gate, and
    /// source node ids; the gate draws no current. The device-current slot
    /// reports the drain current.
    Mosfet {
        d: usize,
        g: usize,
        s: usize,
        model: MosfetModel,
    },
    /// Ebers–Moll BJT. `c`/`b`/`e` are the collector, base, and emitter node
    /// ids. The device-current slot reports the collector current.
    Bjt {
        c: usize,
        b: usize,
        e: usize,
        model: BjtModel,
    },
}

/// Add `v` at MNA position (`row`, `col`) in node-id space (ground skipped).
pub(crate) fn add_a(a: &mut [f64], m: usize, row: usize, col: usize, v: f64) {
    if row != 0 && col != 0 {
        a[(row - 1) * m + (col - 1)] += v;
    }
}

/// Stamp a VCCS: current `g·(v(cp) − v(cn))` flowing from `op` to `on`
/// through the source (i.e. drawn out of node `op`, into node `on`).
pub(crate) fn stamp_vccs(
    a: &mut [f64],
    m: usize,
    op: usize,
    on: usize,
    cp: usize,
    cn: usize,
    g: f64,
) {
    add_a(a, m, op, cp, g);
    add_a(a, m, op, cn, -g);
    add_a(a, m, on, cp, -g);
    add_a(a, m, on, cn, g);
}

/// Conductance stamp for a 2-terminal element between `p` and `n`.
pub(crate) fn stamp_conductance(a: &mut [f64], m: usize, p: usize, n: usize, g: f64) {
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
pub(crate) fn inject(rhs: &mut [f64], p: usize, n: usize, i: f64) {
    if p != 0 {
        rhs[p - 1] += i;
    }
    if n != 0 {
        rhs[n - 1] -= i;
    }
}

/// SPICE-style pn-junction limiting: damp a Newton step in junction voltage so
/// the exponential can't explode. `vnew` is this iteration's raw junction
/// voltage, `vold` the previous iteration's (limited) value.
pub(crate) fn pnjlim(vnew: f64, vold: f64, vte: f64, vcrit: f64) -> f64 {
    if vnew > vcrit && (vnew - vold).abs() > 2.0 * vte {
        if vold > 0.0 {
            let arg = 1.0 + (vnew - vold) / vte;
            if arg > 0.0 {
                vold + vte * arg.ln()
            } else {
                vcrit
            }
        } else if vnew > 0.0 {
            vte * (vnew / vte).ln()
        } else {
            vnew
        }
    } else {
        vnew
    }
}

/// Clamp a Newton step in a FET terminal voltage to ±`max_step` — the
/// square-law analogue of `pnjlim` (the polynomial can't explode, but an
/// undamped step can oscillate across the triode/saturation boundary).
pub(crate) fn fetlim(vnew: f64, vold: f64, max_step: f64) -> f64 {
    vold + (vnew - vold).clamp(-max_step, max_step)
}

/// Stamp a MOSFET's Newton companion (linearized ids as gds + gm·VCCS + a
/// Norton current) into `a`/`rhs`. `nl` carries the limited `[vgs, vds]`
/// across iterations; `guess` is the current node-voltage estimate.
/// Shared by the transient and DC solvers (and, with a scratch rhs, the
/// adjoint Jacobian rebuild).
#[allow(clippy::too_many_arguments)]
pub(crate) fn stamp_mosfet(
    a: &mut [f64],
    rhs: &mut [f64],
    m: usize,
    d: usize,
    g: usize,
    s: usize,
    model: &MosfetModel,
    nl: &mut [f64; 2],
    guess: &[f64],
) {
    let vgs = fetlim(guess[g] - guess[s], nl[0], 2.0);
    let vds = fetlim(guess[d] - guess[s], nl[1], 2.0);
    *nl = [vgs, vds];
    let (ids, gm, gds, _, _) = model.eval(vgs, vds);
    stamp_conductance(a, m, d, s, gds);
    stamp_vccs(a, m, d, s, g, s, gm);
    let ieq = ids - gm * vgs - gds * vds;
    inject(rhs, d, s, -ieq);
}

/// Stamp a BJT's Newton companion: the two junction diodes (b–e, b–c) plus
/// the transport current c→e controlled by both junctions. `nl` carries the
/// limited `[vbe, vbc]`. Both junctions get `pnjlim` limiting.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stamp_bjt(
    a: &mut [f64],
    rhs: &mut [f64],
    m: usize,
    c: usize,
    b: usize,
    e: usize,
    model: &BjtModel,
    nl: &mut [f64; 2],
    guess: &[f64],
) {
    let sgn = model.polarity.sign();
    let vcrit = VT * (VT / (std::f64::consts::SQRT_2 * model.is)).ln();
    // Limit in the internal (N-type) frame where the exponentials grow.
    let vbe = sgn * pnjlim(sgn * (guess[b] - guess[e]), sgn * nl[0], VT, vcrit);
    let vbc = sgn * pnjlim(sgn * (guess[b] - guess[c]), sgn * nl[1], VT, vcrit);
    *nl = [vbe, vbc];
    let ev = model.eval(vbe, vbc);

    // b–e junction diode.
    stamp_conductance(a, m, b, e, ev.gpi);
    inject(rhs, b, e, -(ev.ibe - ev.gpi * vbe));
    // b–c junction diode.
    stamp_conductance(a, m, b, c, ev.gmu);
    inject(rhs, b, c, -(ev.ibc - ev.gmu * vbc));
    // Transport current c→e: iT = gmf·vbe − gmr·vbc + const.
    stamp_vccs(a, m, c, e, b, e, ev.gmf);
    stamp_vccs(a, m, c, e, b, c, -ev.gmr);
    inject(rhs, c, e, -(ev.it - ev.gmf * vbe + ev.gmr * vbc));
}

impl Device {
    /// The (`p`, `n`) terminal node ids of this device. For three-terminal
    /// devices this is the current-carrying pair: (drain, source) for a
    /// MOSFET, (collector, emitter) for a BJT.
    pub fn terminals(&self) -> (usize, usize) {
        match *self {
            Device::Resistor { p, n, .. }
            | Device::Capacitor { p, n, .. }
            | Device::Inductor { p, n, .. }
            | Device::VSource { p, n, .. }
            | Device::ISource { p, n, .. }
            | Device::Diode { p, n, .. }
            | Device::Motor { p, n, .. } => (p, n),
            Device::Mosfet { d, s, .. } => (d, s),
            Device::Bjt { c, e, .. } => (c, e),
        }
    }

    /// Whether this device needs its own MNA branch-current unknown.
    pub fn needs_branch(&self) -> bool {
        matches!(self, Device::VSource { .. } | Device::Motor { .. })
    }

    /// Instantaneous power (W) absorbed by this device given the node
    /// voltages and its reported current `i`. For two-terminal devices this
    /// is `(v_p − v_n)·i`; three-terminal devices sum over all terminals
    /// (a BJT's base current carries power that the collector–emitter pair
    /// alone would miss). This is the quantity Tellegen's theorem sums to
    /// zero over the network.
    pub fn power(&self, node_v: &[f64], i: f64) -> f64 {
        match *self {
            // Gate draws no current: all channel power is (v_d − v_s)·ids.
            Device::Mosfet { d, s, .. } => (node_v[d] - node_v[s]) * i,
            Device::Bjt { c, b, e, model } => {
                let ev = model.eval(node_v[b] - node_v[e], node_v[b] - node_v[c]);
                let ic = ev.it - ev.ibc;
                let ib = ev.ibe + ev.ibc;
                (node_v[c] - node_v[e]) * ic + (node_v[b] - node_v[e]) * ib
            }
            _ => {
                let (p, n) = self.terminals();
                (node_v[p] - node_v[n]) * i
            }
        }
    }

    /// Stamp this device's contribution into the MNA matrix `a` and RHS `rhs`.
    ///
    /// - `m` is the system dimension, `nn` the node count (incl. ground).
    /// - `branch` is a running branch-index counter; branch devices consume one.
    /// - `cap_v` / `ind_i` are this device's companion history; `nl_prev` is its
    ///   previous-iteration nonlinear state (for Newton limiting): the junction
    ///   voltage in slot 0 for a diode, `[vgs, vds]` for a MOSFET, `[vbe, vbc]`
    ///   for a BJT.
    /// - `guess` is the current Newton node-voltage estimate.
    ///
    /// Returns `Some(state)` with the new limited nonlinear state for
    /// nonlinear devices (so the caller can carry it into the next
    /// iteration), else `None`.
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
        nl_prev: [f64; 2],
        omega: f64,
        guess: &[f64],
    ) -> Option<[f64; 2]> {
        match *self {
            Device::Resistor { p, n, r } => {
                stamp_conductance(a, m, p, n, 1.0 / r);
                None
            }
            Device::Capacitor { p, n, c } => {
                let gc = c / dt;
                stamp_conductance(a, m, p, n, gc);
                // companion current source i_eq = gc·v_prev, injected into p
                inject(rhs, p, n, gc * cap_v);
                None
            }
            Device::Inductor { p, n, l } => {
                let geq = dt / l;
                stamp_conductance(a, m, p, n, geq);
                // companion: i_L = geq·v + i_prev; the i_prev term leaves p
                inject(rhs, p, n, -ind_i);
                None
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
                None
            }
            Device::ISource { p, n, i } => {
                inject(rhs, p, n, i);
                None
            }
            Device::Motor { p, n, params } => {
                // Electrically a branch: v(p) − v(n) = i·Z + E, with winding
                // impedance Z = R + L/dt and EMF E = Ke·ω_prev − (L/dt)·i_prev
                // (back-EMF from the previous rotor speed + inductor history).
                let z = params.r + params.l / dt;
                let e = params.ke * omega - (params.l / dt) * ind_i;
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
                a[br * m + br] -= z;
                rhs[br] += e;
                None
            }
            Device::Diode { p, n, model } => {
                let vte = model.vte();
                let vcrit = vte * (vte / (std::f64::consts::SQRT_2 * model.is)).ln();
                let vd_raw = guess[p] - guess[n];
                let vd = pnjlim(vd_raw, nl_prev[0], vte, vcrit);
                let ev = (vd / vte).min(60.0).exp();
                let id = model.is * (ev - 1.0);
                let geq = (model.is / vte) * ev; // di/dv
                let ieq = id - geq * vd; // companion (Norton) current
                stamp_conductance(a, m, p, n, geq);
                inject(rhs, p, n, -ieq);
                Some([vd, 0.0])
            }
            Device::Mosfet { d, g, s, model } => {
                let mut nl = nl_prev;
                stamp_mosfet(a, rhs, m, d, g, s, &model, &mut nl, guess);
                Some(nl)
            }
            Device::Bjt { c, b, e, model } => {
                let mut nl = nl_prev;
                stamp_bjt(a, rhs, m, c, b, e, &model, &mut nl, guess);
                Some(nl)
            }
        }
    }

    /// Device current (A) computed directly from node voltages. Only
    /// meaningful for memoryless, non-branch devices (R, I, diode, MOSFET,
    /// BJT); reactive and branch devices report their current through other
    /// channels. For a MOSFET this is the drain current; for a BJT the
    /// collector current.
    pub fn current(&self, node_v: &[f64]) -> f64 {
        match *self {
            Device::Resistor { p, n, r } => (node_v[p] - node_v[n]) / r,
            Device::ISource { i, .. } => i,
            Device::Diode { p, n, model } => model.current(node_v[p] - node_v[n]),
            Device::Mosfet { d, g, s, model } => {
                model.eval(node_v[g] - node_v[s], node_v[d] - node_v[s]).0
            }
            Device::Bjt { c, b, e, model } => {
                model.ic(node_v[b] - node_v[e], node_v[b] - node_v[c])
            }
            _ => 0.0,
        }
    }

    /// This device's primary scalar (resistance, source value, …). Diodes and
    /// transistors have no single driven scalar, so they report 0.
    pub fn primary(&self) -> f64 {
        match *self {
            Device::Resistor { r, .. } => r,
            Device::Capacitor { c, .. } => c,
            Device::Inductor { l, .. } => l,
            Device::VSource { v, .. } => v,
            Device::ISource { i, .. } => i,
            Device::Diode { .. } | Device::Mosfet { .. } | Device::Bjt { .. } => 0.0,
            Device::Motor { params, .. } => params.load,
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
            Device::Diode { .. } | Device::Mosfet { .. } | Device::Bjt { .. } => {}
            Device::Motor { params, .. } => params.load = value,
        }
    }
}
