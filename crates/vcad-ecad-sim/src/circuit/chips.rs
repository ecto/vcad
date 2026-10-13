//! Chips: behavioural ICs (and firmware) in the transient loop.
//!
//! The MNA solver knows R, C, L, sources, diodes and transistors. Real boards
//! are mostly ICs whose transistor netlists nobody has: comparators,
//! regulators, hot-swap controllers, watchdogs, microcontrollers. A [`Chip`]
//! is any of those, described by *behaviour*: each step it reads the voltages
//! and currents on its pins and decides what its pins do next.
//!
//! A chip never touches the matrix. Each pin is realised with devices the
//! solver already has, and the chip just sets their values between steps:
//!
//! | [`PinKind`]       | realised as                                   | chip sets            |
//! |-------------------|-----------------------------------------------|----------------------|
//! | `Input`           | nothing (ideal high impedance)                | -                    |
//! | `Output{r_out}`   | VSource → internal node → `r` → pin           | volts, or Hi-Z       |
//! | `Sink`            | ISource drawing from the pin to ground        | amps drawn           |
//! | `Switch{to}`      | Resistor pin ↔ `to` node                      | ohms (on/off/ramp)   |
//!
//! Pin voltages are referenced to ground (node 0); a chip's GND pin is ground.
//!
//! **Coupling is explicit (one step of lag):** a chip sees the solution of the
//! previous step. That is the classic mixed-signal co-simulation contract and is
//! exact in the limit of small `dt`; pick `dt` well under the fastest chip
//! response you care about (1 µs for a comparator into a 220 µF bus is plenty).
//!
//! [`FnChip`] makes a chip out of a closure - the hook for firmware or an agent
//! in the loop: give it pins and a `step` function and it runs against the
//! analog circuit every tick.

use super::{Circuit, CircuitEnv, Device, Observation};

/// What a chip pin is, electrically.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PinKind {
    /// High-impedance input: the chip only reads it.
    Input,
    /// Driven output: an ideal source behind `r_out` ohms, or high impedance.
    Output {
        /// Output resistance when driving (Ω).
        r_out: f64,
    },
    /// Supply or bias pin: the chip draws a current from it (positive = out of
    /// the node into the chip). Use it to make regulators conserve power.
    Sink,
    /// A switch from this pin to node `to` (a FET channel, a load switch).
    Switch {
        /// The other end of the switch.
        to: usize,
    },
}

/// What a pin does this step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Drive {
    /// Inputs: nothing.
    None,
    /// Output pin driven to this voltage (through its `r_out`).
    Volts(f64),
    /// Output pin driven to a voltage through a chosen resistance (V, Ω) -
    /// a current limit, a weak pull, a soft driver.
    Thevenin(f64, f64),
    /// Output pin released (open drain off, tri-stated).
    HiZ,
    /// Sink pin draws this current (A).
    Amps(f64),
    /// Switch pin resistance (Ω). Use [`OPEN`] for off.
    Ohms(f64),
}

/// "Open" resistance used for released outputs and open switches (Ω).
pub const OPEN: f64 = 1e12;

/// A chip's view of its pins for one step.
#[derive(Debug, Clone)]
pub struct ChipIo {
    /// Simulated time at the start of this step (s).
    pub t: f64,
    /// Timestep (s).
    pub dt: f64,
    /// Pin voltages to ground (V), from the previous solution.
    pub v: Vec<f64>,
    /// Pin currents (A), from the previous solution: for an `Output`, current
    /// sourced *out of* the chip into the node; for a `Sink`, the current
    /// drawn; for a `Switch`, current from the pin to `to`; 0 for inputs.
    pub i: Vec<f64>,
    /// What each pin does next. Starts as last step's drive - set only what changes.
    pub drive: Vec<Drive>,
}

/// A behavioural IC.
pub trait Chip {
    /// Pin list. The index is the pin number used in [`ChipIo`].
    fn pins(&self) -> Vec<PinKind>;
    /// Advance one step: read `io.v` / `io.i`, set `io.drive`.
    fn step(&mut self, io: &mut ChipIo);
}

/// How one chip pin was realised in the circuit.
#[derive(Debug, Clone, Copy)]
enum Port {
    Input { node: usize },
    Output { node: usize, src: usize, r: usize, r_out: f64 },
    Sink { node: usize, src: usize },
    Switch { node: usize, r: usize },
}

struct Placed {
    chip: Box<dyn Chip>,
    ports: Vec<Port>,
    drive: Vec<Drive>,
}

/// A circuit plus the chips wired into it. Build devices on `.circuit` as usual,
/// then [`ChipCircuit::add_chip`] with the node each pin lands on.
pub struct ChipCircuit {
    /// The analog circuit.
    pub circuit: Circuit,
    placed: Vec<Placed>,
}

impl ChipCircuit {
    /// Wrap a circuit.
    pub fn new(circuit: Circuit) -> Self {
        ChipCircuit { circuit, placed: Vec::new() }
    }

    /// Place a chip; `nodes[k]` is the node pin `k` connects to. Returns the
    /// chip's index (for [`ChipEnv::chip`]).
    ///
    /// # Panics
    /// If `nodes` does not have one entry per pin.
    pub fn add_chip(&mut self, chip: Box<dyn Chip>, nodes: &[usize]) -> usize {
        let kinds = chip.pins();
        assert_eq!(kinds.len(), nodes.len(), "chip has {} pins, got {} nodes", kinds.len(), nodes.len());
        let mut ports = Vec::new();
        let mut drive = Vec::new();
        for (kind, &node) in kinds.iter().zip(nodes) {
            match *kind {
                PinKind::Input => {
                    ports.push(Port::Input { node });
                    drive.push(Drive::None);
                }
                PinKind::Output { r_out } => {
                    let m = self.circuit.node();
                    let src = self.circuit.add(Device::VSource { p: m, n: 0, v: 0.0 });
                    let r = self.circuit.add(Device::Resistor { p: m, n: node, r: OPEN });
                    ports.push(Port::Output { node, src, r, r_out });
                    drive.push(Drive::HiZ);
                }
                PinKind::Sink => {
                    // ISource{p, n, i} pushes i into `p` and draws it from `n`, so
                    // p = ground, n = node draws from the pin (see the sign test).
                    let src = self.circuit.add(Device::ISource { p: 0, n: node, i: 0.0 });
                    ports.push(Port::Sink { node, src });
                    drive.push(Drive::Amps(0.0));
                }
                PinKind::Switch { to } => {
                    let r = self.circuit.add(Device::Resistor { p: node, n: to, r: OPEN });
                    ports.push(Port::Switch { node, r });
                    drive.push(Drive::Ohms(OPEN));
                }
            }
        }
        self.placed.push(Placed { chip, ports, drive });
        self.placed.len() - 1
    }

    /// Build the steppable env.
    pub fn into_env(self, dt: f64) -> ChipEnv {
        let mut env = CircuitEnv::new(self.circuit, dt);
        env.reset();
        let last = env.observe();
        ChipEnv { env, placed: self.placed, last, dt }
    }
}

/// A [`CircuitEnv`] with chips stepping alongside it.
pub struct ChipEnv {
    /// The underlying analog env (devices, `set_value`, `observe`).
    pub env: CircuitEnv,
    placed: Vec<Placed>,
    last: Observation,
    dt: f64,
}

impl ChipEnv {
    /// Step every chip on the last solution, apply their drives, then step the circuit.
    pub fn step(&mut self) -> Observation {
        let t = self.last.time;
        for p in &mut self.placed {
            let (mut v, mut i) = (Vec::with_capacity(p.ports.len()), Vec::with_capacity(p.ports.len()));
            for port in &p.ports {
                let (node, cur) = match *port {
                    Port::Input { node } => (node, 0.0),
                    Port::Output { node, r, .. } => (node, self.last.device_currents[r]),
                    Port::Sink { node, src } => (node, self.last.device_currents[src]),
                    Port::Switch { node, r } => (node, self.last.device_currents[r]),
                };
                v.push(self.last.node_voltages[node]);
                i.push(cur);
            }
            let mut io = ChipIo { t, dt: self.dt, v, i, drive: p.drive.clone() };
            p.chip.step(&mut io);
            for (k, port) in p.ports.iter().enumerate() {
                match (*port, io.drive[k]) {
                    (Port::Output { src, r, r_out, .. }, Drive::Volts(x)) => {
                        self.env.set_value(src, x);
                        self.env.set_value(r, r_out);
                    }
                    (Port::Output { src, r, .. }, Drive::Thevenin(x, ro)) => {
                        self.env.set_value(src, x);
                        self.env.set_value(r, ro.max(1e-6));
                    }
                    (Port::Output { r, .. }, Drive::HiZ) => self.env.set_value(r, OPEN),
                    (Port::Sink { src, .. }, Drive::Amps(a)) => self.env.set_value(src, a),
                    (Port::Switch { r, .. }, Drive::Ohms(o)) => self.env.set_value(r, o.max(1e-6)),
                    _ => {}
                }
            }
            p.drive = io.drive;
        }
        self.last = self.env.step();
        self.last.clone()
    }

    /// Latest solution.
    pub fn observe(&self) -> &Observation {
        &self.last
    }

    /// Current drive state of a chip's pins (for logging what a chip decided).
    pub fn drives(&self, chip: usize) -> &[Drive] {
        &self.placed[chip].drive
    }
}

// ============================================================================
// Library models
// ============================================================================

/// Push-pull comparator. Pins: `[IN+, IN-, VDD, OUT]`.
///
/// Output goes to VDD when `IN+ > IN-` (plus internal hysteresis), 0 V
/// otherwise, after `delay` seconds. VDD draws `iq`.
#[derive(Debug, Clone)]
pub struct Comparator {
    /// Internal input hysteresis (V), total.
    pub hysteresis: f64,
    /// Propagation delay (s).
    pub delay: f64,
    /// Supply current (A).
    pub iq: f64,
    /// Output resistance (Ω).
    pub r_out: f64,
    high: bool,
    pending: Option<(bool, f64)>,
}

impl Comparator {
    /// A comparator with no internal hysteresis and the given delay.
    pub fn new(delay: f64) -> Self {
        Comparator { hysteresis: 0.0, delay, iq: 0.3e-6, r_out: 50.0, high: false, pending: None }
    }
    /// TI TLV7031-like: 3 µs, 335 nA, push-pull.
    pub fn tlv7031() -> Self {
        Comparator { hysteresis: 0.0, delay: 3e-6, iq: 335e-9, r_out: 50.0, high: false, pending: None }
    }
}

impl Chip for Comparator {
    fn pins(&self) -> Vec<PinKind> {
        vec![PinKind::Input, PinKind::Input, PinKind::Sink, PinKind::Output { r_out: self.r_out }]
    }
    fn step(&mut self, io: &mut ChipIo) {
        let d = io.v[0] - io.v[1];
        let h = self.hysteresis / 2.0;
        let want = if self.high { d > -h } else { d > h };
        let target = self.pending.map(|p| p.0).unwrap_or(self.high);
        if want != target {
            self.pending = Some((want, io.t + self.delay));
        }
        if let Some((w, at)) = self.pending {
            if io.t >= at {
                self.high = w;
                self.pending = None;
            }
        }
        io.drive[2] = Drive::Amps(self.iq);
        io.drive[3] = Drive::Volts(if self.high { io.v[2] } else { 0.0 });
    }
}

/// N-input AND gate (74LVC1G08/11-like). Pins: `[A, B, .., VCC, Y]`.
#[derive(Debug, Clone)]
pub struct AndGate {
    /// Number of inputs.
    pub inputs: usize,
}

impl Chip for AndGate {
    fn pins(&self) -> Vec<PinKind> {
        let mut p = vec![PinKind::Input; self.inputs];
        p.push(PinKind::Sink);
        p.push(PinKind::Output { r_out: 25.0 });
        p
    }
    fn step(&mut self, io: &mut ChipIo) {
        let vcc = io.v[self.inputs];
        let all = io.v[..self.inputs].iter().all(|&x| x > vcc / 2.0);
        io.drive[self.inputs] = Drive::Amps(1e-6);
        io.drive[self.inputs + 1] = Drive::Volts(if all { vcc } else { 0.0 });
    }
}

/// An N-channel FET with its gate driver folded in: a switch D→S that is on
/// when `V(G) - V(S) > vth`. Pins: `[G, D, S]` where D is a `Switch` to S.
/// Build with [`IdealFet::pins_for`] knowing the S node.
#[derive(Debug, Clone)]
pub struct IdealFet {
    /// Gate threshold (V).
    pub vth: f64,
    /// On resistance (Ω).
    pub r_on: f64,
    s_node: usize,
}

impl IdealFet {
    /// A FET whose source is node `s`.
    pub fn new(vth: f64, r_on: f64, s: usize) -> Self {
        IdealFet { vth, r_on, s_node: s }
    }
}

impl Chip for IdealFet {
    fn pins(&self) -> Vec<PinKind> {
        vec![PinKind::Input, PinKind::Switch { to: self.s_node }, PinKind::Input]
    }
    fn step(&mut self, io: &mut ChipIo) {
        let on = io.v[0] - io.v[2] > self.vth;
        io.drive[1] = Drive::Ohms(if on { self.r_on } else { OPEN });
    }
}

/// Linear regulator. Pins: `[VIN, VOUT]`. Regulates to `vout`, drops out at
/// `vin - dropout`, limits at `ilim`. VIN draws the output current plus `iq`,
/// so power is conserved (the difference is dissipated in the pass element).
#[derive(Debug, Clone)]
pub struct Ldo {
    /// Set point (V).
    pub vout: f64,
    /// Dropout (V).
    pub dropout: f64,
    /// Quiescent current (A).
    pub iq: f64,
    /// Current limit (A).
    pub ilim: f64,
    limiting: bool,
}

impl Ldo {
    /// Diodes AP2112K-3.3-like.
    pub fn ap2112k_3v3() -> Self {
        Ldo { vout: 3.3, dropout: 0.25, iq: 55e-6, ilim: 0.8, limiting: false }
    }
}

impl Chip for Ldo {
    fn pins(&self) -> Vec<PinKind> {
        vec![PinKind::Sink, PinKind::Output { r_out: 0.02 }]
    }
    fn step(&mut self, io: &mut ChipIo) {
        let iout = io.i[1].max(0.0);
        let v = self.vout.min(io.v[0] - self.dropout).max(0.0);
        // Current limit as a resistance sized to the limit, entered on
        // over-current and left once the output is back near regulation - a
        // subtract-the-excess fold-back chatters against the one-step lag.
        if iout > self.ilim {
            self.limiting = true;
        } else if self.limiting && iout > 1e-9 {
            // leave the limit once the load (V/I seen now) would draw under it at full set point
            let r_load = io.v[1].max(0.0) / iout;
            if v / r_load < 0.95 * self.ilim {
                self.limiting = false;
            }
        }
        io.drive[0] = Drive::Amps(iout + self.iq);
        io.drive[1] = if self.limiting && v > 0.0 { Drive::Thevenin(v, v / self.ilim) } else { Drive::Volts(v) };
    }
}

/// Averaged (state-space-averaged) synchronous buck. Pins: `[VIN, VOUT, FB, EN]`.
///
/// No switching ripple: the output is a controlled source whose set point
/// integrates until `V(FB)` equals `vref` - so the converter regulates through
/// the board's real feedback divider, and a wrong divider shows up as a wrong
/// output. VIN draws `Pout / (eff * Vin) + iq` (power balance). Starts when
/// `V(EN) > en_th` and `VIN > uvlo`, with a soft-start slew.
#[derive(Debug, Clone)]
pub struct BuckAvg {
    /// Feedback reference (V).
    pub vref: f64,
    /// Efficiency (0..1).
    pub eff: f64,
    /// Input under-voltage lockout (V).
    pub uvlo: f64,
    /// EN threshold (V).
    pub en_th: f64,
    /// Output slew limit during start-up (V/s).
    pub slew: f64,
    /// Output current limit (A).
    pub ilim: f64,
    /// Quiescent input current (A).
    pub iq: f64,
    /// Loop integrator gain (V/s per V of FB error).
    pub gain: f64,
    v: f64,
}

impl BuckAvg {
    /// TI LM5164-like: 100 V, 1 A, 1.2 V reference.
    pub fn lm5164() -> Self {
        BuckAvg { vref: 1.2, eff: 0.88, uvlo: 6.0, en_th: 1.5, slew: 2_000.0, ilim: 1.5, iq: 0.5e-3, gain: 5e4, v: 0.0 }
    }
    /// TI TPS54302-like: 28 V, 3 A, 0.596 V reference.
    pub fn tps54302() -> Self {
        BuckAvg { vref: 0.596, eff: 0.92, uvlo: 4.5, en_th: 1.2, slew: 5_000.0, ilim: 4.0, iq: 0.1e-3, gain: 5e4, v: 0.0 }
    }
}

impl Chip for BuckAvg {
    fn pins(&self) -> Vec<PinKind> {
        vec![PinKind::Sink, PinKind::Output { r_out: 0.01 }, PinKind::Input, PinKind::Input]
    }
    fn step(&mut self, io: &mut ChipIo) {
        let (vin, fb, en) = (io.v[0], io.v[2], io.v[3]);
        let iout = io.i[1].max(0.0);
        if en > self.en_th && vin > self.uvlo {
            let dv = (self.gain * (self.vref - fb) * io.dt).clamp(-self.slew * io.dt, self.slew * io.dt);
            self.v = (self.v + dv).clamp(0.0, vin * 0.98);
            if iout > self.ilim {
                self.v = (self.v - (iout - self.ilim) * 0.5).max(0.0);
            }
        } else {
            self.v = 0.0;
        }
        let pout = io.v[1].max(0.0) * iout;
        io.drive[0] = Drive::Amps(if vin > 1.0 { pout / (self.eff * vin) + self.iq } else { 0.0 });
        io.drive[1] = if self.v > 0.0 { Drive::Volts(self.v) } else { Drive::HiZ };
    }
}

/// High-side hot-swap / protection controller with its external FET.
/// Pins: `[VIN, OUT, EN, UVLO, OVP]`; VIN is a `Switch` to OUT.
///
/// On when `EN > en_th`, `UVLO > uvlo_th` and `OVP < ovp_th`. While on, the
/// switch resistance is raised so the charging current stays under `i_inrush`
/// (the dv/dt-limited start of an LM5060-class part), settling at `r_on`.
#[derive(Debug, Clone)]
pub struct HotSwap {
    /// EN threshold (V).
    pub en_th: f64,
    /// UVLO pin threshold (V).
    pub uvlo_th: f64,
    /// OVP pin threshold (V).
    pub ovp_th: f64,
    /// Inrush current limit (A).
    pub i_inrush: f64,
    /// Fully-on resistance (Ω).
    pub r_on: f64,
    out_node: usize,
}

impl HotSwap {
    /// A controller whose OUT is node `out`.
    pub fn new(out: usize, en_th: f64, uvlo_th: f64, ovp_th: f64, i_inrush: f64, r_on: f64) -> Self {
        HotSwap { en_th, uvlo_th, ovp_th, i_inrush, r_on, out_node: out }
    }
}

impl Chip for HotSwap {
    fn pins(&self) -> Vec<PinKind> {
        vec![PinKind::Switch { to: self.out_node }, PinKind::Input, PinKind::Input, PinKind::Input, PinKind::Input]
    }
    fn step(&mut self, io: &mut ChipIo) {
        let on = io.v[2] > self.en_th && io.v[3] > self.uvlo_th && io.v[4] < self.ovp_th;
        let r = if on {
            let dv = (io.v[0] - io.v[1]).max(0.0);
            (dv / self.i_inrush).max(self.r_on)
        } else {
            OPEN
        };
        io.drive[0] = Drive::Ohms(r);
    }
}

/// Window watchdog (TPS3430-like). Pins: `[WDI, WDO]`, WDO open-drain.
/// WDO is released while WDI edges arrive inside `[t_min, t_max]` of each
/// other; it pulls low for `t_reset` after a missing or early kick.
#[derive(Debug, Clone)]
pub struct Watchdog {
    /// Earliest acceptable kick interval (s).
    pub t_min: f64,
    /// Latest acceptable kick interval (s).
    pub t_max: f64,
    /// Fault (reset) pulse width (s).
    pub t_reset: f64,
    last_kick: f64,
    last_in: bool,
    fault_until: f64,
}

impl Watchdog {
    /// A window watchdog.
    pub fn new(t_min: f64, t_max: f64, t_reset: f64) -> Self {
        Watchdog { t_min, t_max, t_reset, last_kick: 0.0, last_in: false, fault_until: 0.0 }
    }
}

impl Chip for Watchdog {
    fn pins(&self) -> Vec<PinKind> {
        vec![PinKind::Input, PinKind::Output { r_out: 20.0 }]
    }
    fn step(&mut self, io: &mut ChipIo) {
        let hi = io.v[0] > 1.5;
        if hi && !self.last_in {
            let dt = io.t - self.last_kick;
            if dt < self.t_min && self.last_kick > 0.0 {
                self.fault_until = io.t + self.t_reset;
            }
            self.last_kick = io.t;
        }
        self.last_in = hi;
        if io.t - self.last_kick > self.t_max {
            self.fault_until = io.t + self.t_reset;
            self.last_kick = io.t;
        }
        io.drive[1] = if io.t < self.fault_until { Drive::Volts(0.0) } else { Drive::HiZ };
    }
}

/// A chip made of a closure: firmware, a controller, an agent - anything that
/// can read pins and set drives each tick.
pub struct FnChip<F: FnMut(&mut ChipIo)> {
    pins: Vec<PinKind>,
    f: F,
}

impl<F: FnMut(&mut ChipIo)> FnChip<F> {
    /// Pins and a step function.
    pub fn new(pins: Vec<PinKind>, f: F) -> Self {
        FnChip { pins, f }
    }
}

impl<F: FnMut(&mut ChipIo)> Chip for FnChip<F> {
    fn pins(&self) -> Vec<PinKind> {
        self.pins.clone()
    }
    fn step(&mut self, io: &mut ChipIo) {
        (self.f)(io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The arm controller's brake comparator: R16/R17 divider, R20 hysteresis
    /// from the output, R18/R19 reference. Thresholds must match the closed form.
    #[test]
    fn comparator_hysteresis_network_matches_closed_form() {
        let (r16, r17, r20) = (316e3, 10e3, 1e6);
        let mut c = Circuit::new();
        let (vbus, div, refn, v33, cmp) = (c.node(), c.node(), c.node(), c.node(), c.node());
        let src = c.add(Device::VSource { p: vbus, n: 0, v: 50.0 });
        c.add(Device::Resistor { p: vbus, n: div, r: r16 });
        c.add(Device::Resistor { p: div, n: 0, r: r17 });
        c.add(Device::Resistor { p: cmp, n: div, r: r20 });
        c.add(Device::VSource { p: v33, n: 0, v: 3.3 });
        c.add(Device::Resistor { p: v33, n: refn, r: 10e3 });
        c.add(Device::Resistor { p: refn, n: 0, r: 10e3 });
        let mut cc = ChipCircuit::new(c);
        let k = cc.add_chip(Box::new(Comparator::new(0.0)), &[div, refn, v33, cmp]);
        let mut env = cc.into_env(1e-6);
        let (mut on_at, mut off_at) = (None, None);
        for s in 0..20_000 {
            // ramp 50 -> 58 -> 46 V
            let t = s as f64 / 20_000.0;
            let v = if t < 0.5 { 50.0 + 16.0 * t } else { 58.0 - 24.0 * (t - 0.5) };
            env.env.set_value(src, v);
            let high = |d: Drive| matches!(d, Drive::Volts(x) if x > 1.0);
            let before = high(env.drives(k)[3]);
            env.step();
            let after = high(env.drives(k)[3]);
            if after && !before {
                on_at = on_at.or(Some(v));
            }
            if before && !after {
                off_at = off_at.or(Some(v));
            }
        }
        let par = |a: f64, b: f64| a * b / (a + b);
        let a = par(r17, r20) / (r16 + par(r17, r20));
        let b = par(r16, r17) / (r20 + par(r16, r17));
        let (on, off) = (1.65 / a, (1.65 - 3.3 * b) / a);
        assert!((on_at.unwrap() - on).abs() < 0.05, "on {:?} vs {on}", on_at);
        assert!((off_at.unwrap() - off).abs() < 0.05, "off {:?} vs {off}", off_at);
    }

    #[test]
    fn sink_pin_draws_current_out_of_the_node() {
        // 5 V through 10 ohm into a pin that sinks 100 mA: the pin sits at 4.0 V, not 6.0 V.
        let mut c = Circuit::new();
        let (s, pin) = (c.node(), c.node());
        c.add(Device::VSource { p: s, n: 0, v: 5.0 });
        c.add(Device::Resistor { p: s, n: pin, r: 10.0 });
        let mut cc = ChipCircuit::new(c);
        cc.add_chip(Box::new(FnChip::new(vec![PinKind::Sink], |io: &mut ChipIo| io.drive[0] = Drive::Amps(0.1))), &[pin]);
        let mut env = cc.into_env(1e-6);
        env.step();
        env.step();
        assert!((env.observe().node_voltages[pin] - 4.0).abs() < 1e-6, "{}", env.observe().node_voltages[pin]);
    }

    #[test]
    fn ldo_drops_out_and_conserves_current() {
        let mut c = Circuit::new();
        let (vin, vout) = (c.node(), c.node());
        let src = c.add(Device::VSource { p: vin, n: 0, v: 5.0 });
        c.add(Device::Resistor { p: vout, n: 0, r: 33.0 }); // 100 mA
        c.add(Device::Capacitor { p: vout, n: 0, c: 10e-6 });
        let mut cc = ChipCircuit::new(c);
        cc.add_chip(Box::new(Ldo::ap2112k_3v3()), &[vin, vout]);
        let mut env = cc.into_env(1e-6);
        for _ in 0..5_000 {
            env.step();
        }
        let o = env.observe().clone();
        assert!((o.node_voltages[vout] - 3.3).abs() < 0.01, "{}", o.node_voltages[vout]);
        // supply current = load + iq
        let i_in = o.device_currents[src].abs();
        assert!((i_in - (0.1 + 55e-6)).abs() < 1e-3, "{i_in}");
        env.env.set_value(src, 3.4);
        for _ in 0..5_000 {
            env.step();
        }
        assert!((env.observe().node_voltages[vout] - 3.15).abs() < 0.02);
    }

    #[test]
    fn buck_regulates_through_the_board_divider_and_balances_power() {
        // arm controller 12 V rail: LM5164, R29 100k / R30 11k
        let mut c = Circuit::new();
        let (vin, vout, fb, en) = (c.node(), c.node(), c.node(), c.node());
        let src = c.add(Device::VSource { p: vin, n: 0, v: 48.0 });
        c.add(Device::VSource { p: en, n: 0, v: 3.0 });
        c.add(Device::Resistor { p: vout, n: fb, r: 100e3 });
        c.add(Device::Resistor { p: fb, n: 0, r: 11e3 });
        c.add(Device::Capacitor { p: vout, n: 0, c: 22e-6 });
        let load = c.add(Device::Resistor { p: vout, n: 0, r: 24.0 }); // 0.5 A
        let mut cc = ChipCircuit::new(c);
        cc.add_chip(Box::new(BuckAvg::lm5164()), &[vin, vout, fb, en]);
        let mut env = cc.into_env(1e-6);
        for _ in 0..30_000 {
            env.step();
        }
        let o = env.observe().clone();
        let expect = 1.2 * (111.0 / 11.0);
        assert!((o.node_voltages[vout] - expect).abs() < 0.05, "{} vs {expect}", o.node_voltages[vout]);
        let pout = o.node_voltages[vout] * o.device_currents[load];
        let pin = 48.0 * o.device_currents[src].abs();
        assert!((pin - pout / 0.88).abs() / pin < 0.03, "pin {pin} pout {pout}");
    }

    #[test]
    fn hot_swap_limits_inrush_then_settles_on() {
        let mut c = Circuit::new();
        let (vin, out, en, uv, ov) = (c.node(), c.node(), c.node(), c.node(), c.node());
        c.add(Device::VSource { p: vin, n: 0, v: 48.0 });
        c.add(Device::VSource { p: en, n: 0, v: 3.3 });
        c.add(Device::VSource { p: uv, n: 0, v: 2.0 });
        c.add(Device::VSource { p: ov, n: 0, v: 1.0 });
        let cap = c.add(Device::Capacitor { p: out, n: 0, c: 220e-6 });
        let mut cc = ChipCircuit::new(c);
        cc.add_chip(Box::new(HotSwap::new(out, 2.0, 1.6, 2.0, 2.0, 1.5e-3)), &[vin, out, en, uv, ov]);
        let mut env = cc.into_env(1e-6);
        let mut peak = 0.0f64;
        for _ in 0..20_000 {
            let o = env.step();
            peak = peak.max(o.device_currents[cap]);
        }
        assert!(peak < 2.2, "inrush {peak}");
        assert!((env.observe().node_voltages[out] - 48.0).abs() < 0.05);
    }

    #[test]
    fn firmware_in_the_loop_bang_bang() {
        // An "MCU" reads a capacitor through an ADC pin and drives a GPIO that
        // charges it through 1k: a bang-bang controller holding 2.0..2.2 V.
        let mut c = Circuit::new();
        let (gpio, cap) = (c.node(), c.node());
        c.add(Device::Resistor { p: gpio, n: cap, r: 1e3 });
        c.add(Device::Capacitor { p: cap, n: 0, c: 10e-6 });
        c.add(Device::Resistor { p: cap, n: 0, r: 10e3 });
        let mut cc = ChipCircuit::new(c);
        let mut high = false;
        cc.add_chip(
            Box::new(FnChip::new(vec![PinKind::Input, PinKind::Output { r_out: 25.0 }], move |io: &mut ChipIo| {
                let v = io.v[0];
                if v < 2.0 { high = true }
                if v > 2.2 { high = false }
                io.drive[1] = Drive::Volts(if high { 3.3 } else { 0.0 });
            })),
            &[cap, gpio],
        );
        let mut env = cc.into_env(1e-5);
        let (mut lo, mut hi) = (f64::MAX, 0.0f64);
        for k in 0..20_000 {
            let o = env.step();
            if k > 5_000 {
                lo = lo.min(o.node_voltages[cap]);
                hi = hi.max(o.node_voltages[cap]);
            }
        }
        assert!(lo > 1.95 && hi < 2.25, "{lo}..{hi}");
    }

    #[test]
    fn watchdog_faults_when_kicks_stop() {
        let mut c = Circuit::new();
        let (wdi, wdo, v33) = (c.node(), c.node(), c.node());
        c.add(Device::VSource { p: v33, n: 0, v: 3.3 });
        c.add(Device::Resistor { p: v33, n: wdo, r: 10e3 });
        let kick = c.add(Device::VSource { p: wdi, n: 0, v: 0.0 });
        let mut cc = ChipCircuit::new(c);
        cc.add_chip(Box::new(Watchdog::new(0.0, 0.010, 0.005)), &[wdi, wdo]);
        let mut env = cc.into_env(1e-4);
        let mut low_seen_while_kicking = false;
        for k in 0..1_000 {
            let t = k as f64 * 1e-4;
            let kicking = t < 0.05;
            env.env.set_value(kick, if kicking && (k / 20) % 2 == 0 { 3.3 } else { 0.0 });
            let o = env.step();
            if kicking && t > 0.005 && o.node_voltages[wdo] < 1.0 {
                low_seen_while_kicking = true;
            }
        }
        assert!(!low_seen_while_kicking);
        // kicks stopped at 50 ms: by 80 ms the watchdog has pulled WDO low at least once
        let mut env2_low = false;
        for _ in 0..300 {
            if env.step().node_voltages[wdo] < 1.0 {
                env2_low = true;
            }
        }
        assert!(env2_low);
    }
}
