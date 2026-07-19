//! Lumped-element circuit simulation — a generalized network solver.
//!
//! This is the SPICE-style, time-stepping counterpart to the field solvers
//! elsewhere in the stack. Each device "stamps" its contribution into a
//! Modified Nodal Analysis (MNA) system; reactive elements (C, L) use
//! backward-Euler **companion models** (a conductance in parallel with a history
//! current source) so a transient step is just "re-stamp with updated history,
//! solve". The result is node voltages + branch currents at each tick.
//!
//! The public surface mirrors the physics engine's gym/env: build a [`Circuit`],
//! wrap it in a [`CircuitEnv`], then `reset()` / `step()` / `observe()`.
//!
//! ```
//! use vcad_ecad_sim::circuit::{Circuit, CircuitEnv, Device};
//! // 5 V battery charging a 1 µF cap through a 1 kΩ resistor.
//! let mut c = Circuit::new();
//! let vin = c.node();
//! let mid = c.node();
//! c.add(Device::VSource { p: vin, n: 0, v: 5.0 });
//! c.add(Device::Resistor { p: vin, n: mid, r: 1_000.0 });
//! let cap = c.add(Device::Capacitor { p: mid, n: 0, c: 1e-6 });
//! let mut env = CircuitEnv::new(c, 1e-5);
//! env.reset();
//! for _ in 0..1000 { env.step(); }      // ~10 ms ≈ 10 τ
//! assert!(env.observe().node_voltages[mid] > 4.9); // cap fully charged
//! let _ = cap;
//! ```

mod linalg;
pub use linalg::solve_dense;

mod devices;
pub use devices::{BjtModel, Device, DiodeModel, MosfetModel, MotorParams, Polarity};

pub mod ac;
pub mod adjoint;
pub mod dc;
pub mod netlist;
pub mod receipt;
pub mod tolerance;
pub mod transient_adjoint;

/// Companion-model integration method for reactive elements (C, L).
///
/// - [`Integrator::BackwardEuler`] — first-order, L-stable, the historical
///   default of this module. Error is O(dt).
/// - [`Integrator::Trapezoidal`] — second-order, the SPICE2 default
///   (Nagel, "SPICE2: A Computer Program to Simulate Semiconductor
///   Circuits", UCB ERL-M520, 1975, §4). Error is O(dt²): halving dt
///   quarters the local truncation error, which the validation suite
///   verifies against the analytic RC step response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Integrator {
    /// Backward Euler (first order). Default, kept for existing consumers.
    #[default]
    BackwardEuler,
    /// Trapezoidal rule (second order, SPICE2 default).
    Trapezoidal,
}

/// Configuration for LTE-based adaptive timestep control (opt-in via
/// [`CircuitEnv::set_adaptive`]).
///
/// Per accepted step the local truncation error (LTE) of every capacitor
/// voltage and inductor current is estimated by comparing the trapezoidal
/// corrector against a lower-order explicit predictor built from the stored
/// companion derivative (the divided-difference approach of Nagel, SPICE2,
/// UCB ERL-M520, 1975, §4.4). A step is accepted when every estimate is
/// within `reltol·|x| + abstol`; the next step size follows the standard
/// third-order controller `dt·min(2, 0.9·(tol/LTE)^{1/3})`, and a rejected
/// step is redone at half the size. All step sizes are clamped to
/// `[dt_min, dt_max]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdaptiveConfig {
    /// Relative tolerance on each state variable (dimensionless).
    pub reltol: f64,
    /// Absolute tolerance floor (V for capacitor voltages, A for inductor
    /// currents).
    pub abstol: f64,
    /// Smallest allowed step (s). A step that still fails at `dt_min` is
    /// accepted anyway — there is nothing smaller to try.
    pub dt_min: f64,
    /// Largest allowed step (s).
    pub dt_max: f64,
}

/// A snapshot of every piece of mutable transient state, so a rejected
/// adaptive step can be rolled back with no partial companion history.
#[derive(Debug, Clone)]
struct StateSnapshot {
    time: f64,
    first_step: bool,
    cap_v: Vec<f64>,
    cap_i: Vec<f64>,
    ind_i: Vec<f64>,
    ind_v: Vec<f64>,
    nl_state: Vec<[f64; 2]>,
    mech_omega: Vec<f64>,
    mech_theta: Vec<f64>,
    node_v: Vec<f64>,
    dev_i: Vec<f64>,
}

/// A lumped-element circuit: a set of [`Device`]s connecting numbered nodes.
///
/// Node `0` is always ground (the voltage reference). Allocate other nodes with
/// [`Circuit::node`].
#[derive(Debug, Clone, Default)]
pub struct Circuit {
    /// Number of nodes including ground (node 0). Starts at 1.
    pub num_nodes: usize,
    /// Devices in insertion order; the index is the device id.
    pub devices: Vec<Device>,
}

impl Circuit {
    /// A fresh circuit containing only ground (node 0).
    pub fn new() -> Self {
        Circuit {
            num_nodes: 1,
            devices: Vec::new(),
        }
    }

    /// Allocate a new (non-ground) node and return its id.
    pub fn node(&mut self) -> usize {
        let id = self.num_nodes;
        self.num_nodes += 1;
        id
    }

    /// Add a device, returning its id (its index in [`Circuit::devices`]).
    pub fn add(&mut self, device: Device) -> usize {
        self.devices.push(device);
        self.devices.len() - 1
    }

    /// Number of voltage-source-like devices (each needs an MNA branch current).
    fn num_branches(&self) -> usize {
        self.devices.iter().filter(|d| d.needs_branch()).count()
    }
}

/// A snapshot of the circuit state after a step.
#[derive(Debug, Clone, Default)]
pub struct Observation {
    /// Simulated time (s).
    pub time: f64,
    /// Voltage at each node, indexed by node id. `node_voltages[0]` is 0 (ground).
    pub node_voltages: Vec<f64>,
    /// Current through each device (A), indexed by device id. Positive flows
    /// from the device's `p` terminal to its `n` terminal.
    pub device_currents: Vec<f64>,
    /// Rotor angle (rad) per device id; 0 for non-motors. Drives 3D spin.
    pub rotor_angles: Vec<f64>,
    /// Rotor angular velocity (rad/s) per device id; 0 for non-motors.
    pub rotor_speeds: Vec<f64>,
}

/// A steppable circuit simulation. Mirrors the physics engine's env: `reset`,
/// `step`, `observe`.
#[derive(Debug, Clone)]
pub struct CircuitEnv {
    circuit: Circuit,
    dt: f64,
    time: f64,
    integrator: Integrator,
    /// True until the first step completes. The trapezoidal rule needs a
    /// consistent history current, which a t = 0 source discontinuity denies
    /// it — so the first step always runs backward Euler (standard SPICE
    /// startup practice; one O(dt²) local error does not break global
    /// second-order accuracy).
    first_step: bool,
    /// Per-device companion history: previous capacitor voltage (device id → V).
    cap_v: Vec<f64>,
    /// Per-device companion history: previous capacitor current (device id → A).
    /// Used only by the trapezoidal integrator.
    cap_i: Vec<f64>,
    /// Per-device companion history: previous inductor current (device id → A).
    ind_i: Vec<f64>,
    /// Per-device companion history: previous inductor voltage (device id → V).
    /// Used only by the trapezoidal integrator.
    ind_v: Vec<f64>,
    /// Per-device nonlinear state (device id → up to two junction/terminal
    /// voltages), warm-started across steps and limited across Newton
    /// iterations. Diode: `[v_d, –]`; MOSFET: `[vgs, vds]`; BJT: `[vbe, vbc]`.
    nl_state: Vec<[f64; 2]>,
    /// Per-device rotor angular velocity (device id → rad/s) for motors.
    mech_omega: Vec<f64>,
    /// Per-device rotor angle (device id → rad) for motors.
    mech_theta: Vec<f64>,
    /// Latest node voltages (length `num_nodes`, index 0 = ground = 0).
    node_v: Vec<f64>,
    /// Latest device currents (length `devices.len()`).
    dev_i: Vec<f64>,
    /// Newton iteration cap for nonlinear devices.
    max_newton: usize,
    /// Adaptive-timestep control; `None` (the default) keeps the historical
    /// fixed-step behavior bit-for-bit. The adjoint machinery and existing
    /// WASM consumers depend on the frozen fixed-dt discretization (the
    /// accept/reject control flow is not differentiated — see the particle
    /// crate's adjoint docs), so adaptivity is strictly opt-in.
    adaptive: Option<AdaptiveConfig>,
    /// Current adaptive step size (s); meaningful only when `adaptive` is set.
    adaptive_dt: f64,
}

impl CircuitEnv {
    /// Build an env around a circuit with a fixed timestep `dt` (seconds).
    pub fn new(circuit: Circuit, dt: f64) -> Self {
        let nd = circuit.devices.len();
        let nn = circuit.num_nodes;
        CircuitEnv {
            circuit,
            dt,
            time: 0.0,
            integrator: Integrator::default(),
            first_step: true,
            cap_v: vec![0.0; nd],
            cap_i: vec![0.0; nd],
            ind_i: vec![0.0; nd],
            ind_v: vec![0.0; nd],
            nl_state: vec![[0.0; 2]; nd],
            mech_omega: vec![0.0; nd],
            mech_theta: vec![0.0; nd],
            node_v: vec![0.0; nn],
            dev_i: vec![0.0; nd],
            max_newton: 50,
            adaptive: None,
            adaptive_dt: dt,
        }
    }

    /// Reset to the power-on state: t = 0, capacitors discharged, inductors with
    /// no current, all nodes at 0 V.
    pub fn reset(&mut self) {
        self.time = 0.0;
        self.first_step = true;
        self.cap_v.fill(0.0);
        self.cap_i.fill(0.0);
        self.ind_i.fill(0.0);
        self.ind_v.fill(0.0);
        self.nl_state.fill([0.0; 2]);
        self.mech_omega.fill(0.0);
        self.mech_theta.fill(0.0);
        self.node_v.fill(0.0);
        self.dev_i.fill(0.0);
        if let Some(cfg) = self.adaptive {
            self.adaptive_dt = self.dt.clamp(cfg.dt_min, cfg.dt_max);
        }
    }

    /// The configured timestep (s).
    pub fn dt(&self) -> f64 {
        self.dt
    }

    /// Change the timestep (s).
    pub fn set_dt(&mut self, dt: f64) {
        self.dt = dt;
    }

    /// The active companion-model integration method.
    pub fn integrator(&self) -> Integrator {
        self.integrator
    }

    /// Select the companion-model integration method.
    ///
    /// Call before the first [`CircuitEnv::step`] or after a
    /// [`CircuitEnv::reset`]. Switching mid-run is rejected in debug builds:
    /// the trapezoidal companion recurrence assumes its history current was
    /// itself produced by the trapezoidal rule, so a mid-run swap injects a
    /// spurious startup transient. The `first_step` guard machine-checks the
    /// contract the doc used to only describe.
    pub fn set_integrator(&mut self, integrator: Integrator) {
        debug_assert!(
            self.first_step,
            "set_integrator must be called before stepping or after reset(); \
             switching mid-run corrupts the companion history"
        );
        self.integrator = integrator;
    }

    /// Enable LTE-based adaptive timestep control (see [`AdaptiveConfig`]).
    ///
    /// Adaptivity implies the trapezoidal integrator (the LTE estimate and
    /// the 1/3-power controller both assume its third-order local error), so
    /// this also selects [`Integrator::Trapezoidal`]. Call before the first
    /// [`CircuitEnv::step`] or after a [`CircuitEnv::reset`], like
    /// [`CircuitEnv::set_integrator`]. Observation timestamps become
    /// non-uniform; `Observation::time` carries the true simulated time.
    ///
    /// # Panics
    /// Panics if the config is malformed (`dt_min ≤ 0`, `dt_min > dt_max`,
    /// or non-positive tolerances).
    pub fn set_adaptive(&mut self, cfg: AdaptiveConfig) {
        assert!(
            cfg.dt_min > 0.0 && cfg.dt_min <= cfg.dt_max,
            "AdaptiveConfig requires 0 < dt_min <= dt_max"
        );
        assert!(
            cfg.reltol > 0.0 && cfg.abstol > 0.0,
            "AdaptiveConfig requires positive tolerances"
        );
        debug_assert!(
            self.first_step,
            "set_adaptive must be called before stepping or after reset()"
        );
        self.integrator = Integrator::Trapezoidal;
        self.adaptive = Some(cfg);
        self.adaptive_dt = self.dt.clamp(cfg.dt_min, cfg.dt_max);
    }

    /// Disable adaptive stepping, returning to the fixed-dt path.
    pub fn clear_adaptive(&mut self) {
        debug_assert!(
            self.first_step,
            "clear_adaptive must be called before stepping or after reset()"
        );
        self.adaptive = None;
    }

    /// Tellegen power-balance residual (W) of the latest solved state:
    /// Σ over devices of (v_p − v_n)·i, with i the device current the KCL
    /// solve actually used. Tellegen's theorem says this is exactly zero for
    /// any circuit satisfying KCL/KVL, so the residual measures nothing but
    /// solver error — the energy conscience of this module. Meaningful only
    /// after a [`CircuitEnv::step`].
    pub fn power_balance(&self) -> f64 {
        self.circuit
            .devices
            .iter()
            .enumerate()
            .map(|(id, d)| d.power(&self.node_v, self.dev_i[id]))
            .sum()
    }

    /// Mutate a device's primary scalar (resistance, source value, …). Lets a
    /// caller drive the circuit live — a switch, a PWM source, a scrubbed value.
    pub fn set_value(&mut self, device_id: usize, value: f64) {
        if let Some(d) = self.circuit.devices.get_mut(device_id) {
            d.set_primary(value);
        }
    }

    /// Read a device's primary scalar.
    pub fn value(&self, device_id: usize) -> Option<f64> {
        self.circuit.devices.get(device_id).map(|d| d.primary())
    }

    /// Advance the simulation by one timestep and return the new observation.
    ///
    /// On the default fixed-step path this advances by exactly `dt`. With
    /// adaptive control enabled ([`CircuitEnv::set_adaptive`]) it advances by
    /// one *accepted* step — internally the step may be halved and redone
    /// (LTE too large, or Newton non-convergence) before it is committed.
    pub fn step(&mut self) -> Observation {
        if self.adaptive.is_some() {
            return self.step_adaptive(None);
        }
        // First step always runs backward Euler (see `first_step` docs).
        let integ = if self.first_step {
            Integrator::BackwardEuler
        } else {
            self.integrator
        };
        self.attempt_step(self.dt, integ);
        self.observe()
    }

    /// Step repeatedly until simulated time reaches `t_end`, returning every
    /// observation along the way. With adaptive control the final step is
    /// shortened to land exactly on `t_end`; on the fixed-step path the last
    /// observation may overshoot by up to one `dt` (the fixed grid is never
    /// distorted).
    pub fn step_to(&mut self, t_end: f64) -> Vec<Observation> {
        let mut out = Vec::new();
        while self.time < t_end * (1.0 - 1e-12) {
            let obs = if self.adaptive.is_some() {
                self.step_adaptive(Some(t_end - self.time))
            } else {
                self.step()
            };
            out.push(obs);
        }
        out
    }

    /// One accepted adaptive step: attempt at the current step size, estimate
    /// the LTE of every reactive state, and shrink-and-redo until the step
    /// passes (or `dt_min` says nothing smaller is possible). `cap` bounds the
    /// step so [`CircuitEnv::step_to`] can land exactly on its end time.
    fn step_adaptive(&mut self, cap: Option<f64>) -> Observation {
        let cfg = self.adaptive.expect("adaptive config present");
        let mut dt = self.adaptive_dt.clamp(cfg.dt_min, cfg.dt_max);
        if let Some(c) = cap {
            dt = dt.min(c.max(cfg.dt_min));
        }

        loop {
            let integ = if self.first_step {
                Integrator::BackwardEuler
            } else {
                self.integrator
            };
            let snap = self.snapshot();

            // Explicit predictor per reactive state from the stored companion
            // derivative (divided differences — Nagel §4.4): the corrector-
            // minus-predictor gap is the LTE estimate.
            let preds: Vec<(usize, f64, f64)> = self
                .circuit
                .devices
                .iter()
                .enumerate()
                .filter_map(|(id, d)| match *d {
                    Device::Capacitor { c, .. } => Some((id, self.cap_v[id], self.cap_i[id] / c)),
                    Device::Inductor { l, .. } => Some((id, self.ind_i[id], self.ind_v[id] / l)),
                    _ => None,
                })
                .collect();

            let converged = self.attempt_step(dt, integ);
            let at_floor = dt <= cfg.dt_min * (1.0 + 1e-12);

            if !converged && !at_floor {
                // The classic transient-convergence rescue: Newton failed, so
                // roll everything back and retry at half the step.
                self.restore(snap);
                dt = (dt * 0.5).max(cfg.dt_min);
                continue;
            }

            // Worst LTE-to-tolerance ratio over all reactive states.
            let mut ratio = 0.0f64;
            for &(id, x_prev, dxdt) in &preds {
                let x_new = match self.circuit.devices[id] {
                    Device::Capacitor { .. } => self.cap_v[id],
                    _ => self.ind_i[id],
                };
                let est = (x_new - (x_prev + dt * dxdt)).abs();
                let tol = cfg.reltol * x_new.abs().max(x_prev.abs()) + cfg.abstol;
                ratio = ratio.max(est / tol);
            }

            if ratio > 1.0 && !at_floor {
                self.restore(snap);
                dt = (dt * 0.5).max(cfg.dt_min);
                continue;
            }

            // Accepted. Third-order controller for the trapezoidal rule:
            // dt' = dt·min(2, 0.9·(tol/LTE)^{1/3}), clamped to [dt_min, dt_max].
            let grow = if ratio > 0.0 {
                (0.9 * ratio.powf(-1.0 / 3.0)).min(2.0)
            } else {
                2.0
            };
            self.adaptive_dt = (dt * grow).clamp(cfg.dt_min, cfg.dt_max);
            return self.observe();
        }
    }

    fn snapshot(&self) -> StateSnapshot {
        StateSnapshot {
            time: self.time,
            first_step: self.first_step,
            cap_v: self.cap_v.clone(),
            cap_i: self.cap_i.clone(),
            ind_i: self.ind_i.clone(),
            ind_v: self.ind_v.clone(),
            nl_state: self.nl_state.clone(),
            mech_omega: self.mech_omega.clone(),
            mech_theta: self.mech_theta.clone(),
            node_v: self.node_v.clone(),
            dev_i: self.dev_i.clone(),
        }
    }

    fn restore(&mut self, s: StateSnapshot) {
        self.time = s.time;
        self.first_step = s.first_step;
        self.cap_v = s.cap_v;
        self.cap_i = s.cap_i;
        self.ind_i = s.ind_i;
        self.ind_v = s.ind_v;
        self.nl_state = s.nl_state;
        self.mech_omega = s.mech_omega;
        self.mech_theta = s.mech_theta;
        self.node_v = s.node_v;
        self.dev_i = s.dev_i;
    }

    /// The full solve-and-commit body of one transient step at step size `dt`
    /// with integrator `integ`. Returns whether the Newton loop converged
    /// (`true` for linear circuits after their exact solve). The fixed-step
    /// path ignores the flag — its behavior is unchanged from M0.
    fn attempt_step(&mut self, dt: f64, integ: Integrator) -> bool {
        let nn = self.circuit.num_nodes;
        let nb = self.circuit.num_branches();
        let m = (nn - 1) + nb;
        let mut converged = false;

        // Newton-Raphson outer loop: nonlinear devices linearise around the
        // current node-voltage guess each iteration. Linear circuits converge in
        // a single pass.
        let mut guess = self.node_v.clone();
        for _ in 0..self.max_newton.max(1) {
            let mut a = vec![0.0f64; m * m];
            let mut rhs = vec![0.0f64; m];
            let mut branch = 0usize;

            // Device is Copy, so reading it out releases the borrow on `circuit`
            // and lets us update `nl_state[id]` in the same pass.
            for id in 0..self.circuit.devices.len() {
                let dev = self.circuit.devices[id];
                // Trapezoidal companions for C and L (Nagel, SPICE2, §4):
                //   C: i_n = (2C/dt)(v_n − v_{n−1}) − i_{n−1}
                //   L: i_n = (dt/2L)(v_n + v_{n−1}) + i_{n−1}
                // Everything else shares the backward-Euler stamps.
                if integ == Integrator::Trapezoidal {
                    match dev {
                        Device::Capacitor { p, n, c } => {
                            let g = 2.0 * c / dt;
                            devices::stamp_conductance(&mut a, m, p, n, g);
                            devices::inject(&mut rhs, p, n, g * self.cap_v[id] + self.cap_i[id]);
                            continue;
                        }
                        Device::Inductor { p, n, l } => {
                            let g = dt / (2.0 * l);
                            devices::stamp_conductance(&mut a, m, p, n, g);
                            devices::inject(&mut rhs, p, n, -(self.ind_i[id] + g * self.ind_v[id]));
                            continue;
                        }
                        _ => {}
                    }
                }
                if let Some(vd) = dev.stamp(
                    &mut a,
                    &mut rhs,
                    m,
                    nn,
                    &mut branch,
                    dt,
                    self.cap_v[id],
                    self.ind_i[id],
                    self.nl_state[id],
                    self.mech_omega[id],
                    &guess,
                ) {
                    self.nl_state[id] = vd;
                }
            }

            let solution = match solve_dense(&mut a, &mut rhs, m) {
                Some(x) => x,
                None => break, // singular — keep previous guess (not converged)
            };

            let mut next = vec![0.0; nn];
            next[1..nn].copy_from_slice(&solution[..(nn - 1)]);

            // Convergence: largest node-voltage change between Newton iterations.
            let mut delta = 0.0f64;
            for node in 1..nn {
                delta = delta.max((next[node] - guess[node]).abs());
            }
            guess = next;

            // Stash branch currents for voltage-source-like devices.
            let mut b = 0usize;
            for (id, dev) in self.circuit.devices.iter().enumerate() {
                if dev.needs_branch() {
                    self.dev_i[id] = solution[(nn - 1) + b];
                    b += 1;
                }
            }

            if delta < 1e-9 {
                converged = true;
                break;
            }
        }

        self.node_v = guess;

        // Update companion history + record device currents.
        for (id, dev) in self.circuit.devices.iter().enumerate() {
            match *dev {
                Device::Capacitor { p, n, c } => {
                    let v_new = self.node_v[p] - self.node_v[n];
                    let i_new = match integ {
                        // companion current = gc·(v_new − v_prev) = C·dv/dt
                        Integrator::BackwardEuler => (c / dt) * (v_new - self.cap_v[id]),
                        Integrator::Trapezoidal => {
                            (2.0 * c / dt) * (v_new - self.cap_v[id]) - self.cap_i[id]
                        }
                    };
                    self.dev_i[id] = i_new;
                    self.cap_v[id] = v_new;
                    self.cap_i[id] = i_new;
                }
                Device::Inductor { p, n, l } => {
                    let v = self.node_v[p] - self.node_v[n];
                    let i_new = match integ {
                        Integrator::BackwardEuler => (dt / l) * v + self.ind_i[id],
                        Integrator::Trapezoidal => {
                            (dt / (2.0 * l)) * (v + self.ind_v[id]) + self.ind_i[id]
                        }
                    };
                    self.dev_i[id] = i_new;
                    self.ind_i[id] = i_new;
                    self.ind_v[id] = v;
                }
                Device::Motor { params, .. } => {
                    // Armature current solved as the branch current (already in
                    // dev_i via the branch stash). Advance the rotor (semi-implicit
                    // Euler) and the inductor-history current.
                    let i_m = self.dev_i[id];
                    let omega_prev = self.mech_omega[id];
                    let torque = params.kt * i_m - params.b * omega_prev - params.load;
                    let omega_new = omega_prev + (dt / params.j) * torque;
                    self.mech_theta[id] += dt * omega_new;
                    self.mech_omega[id] = omega_new;
                    self.ind_i[id] = i_m;
                }
                _ => {
                    if !dev.needs_branch() {
                        self.dev_i[id] = dev.current(&self.node_v);
                    }
                }
            }
        }

        self.time += dt;
        self.first_step = false;
        converged
    }

    /// The current state without advancing time.
    pub fn observe(&self) -> Observation {
        Observation {
            time: self.time,
            node_voltages: self.node_v.clone(),
            device_currents: self.dev_i.clone(),
            rotor_angles: self.mech_theta.clone(),
            rotor_speeds: self.mech_omega.clone(),
        }
    }
}

#[cfg(test)]
mod tests;
