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
pub mod receipt;

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
    pub fn step(&mut self) -> Observation {
        let nn = self.circuit.num_nodes;
        let nb = self.circuit.num_branches();
        let m = (nn - 1) + nb;
        // First step always runs backward Euler (see `first_step` docs).
        let integ = if self.first_step {
            Integrator::BackwardEuler
        } else {
            self.integrator
        };

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
                            let g = 2.0 * c / self.dt;
                            devices::stamp_conductance(&mut a, m, p, n, g);
                            devices::inject(&mut rhs, p, n, g * self.cap_v[id] + self.cap_i[id]);
                            continue;
                        }
                        Device::Inductor { p, n, l } => {
                            let g = self.dt / (2.0 * l);
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
                    self.dt,
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
                None => break, // singular — keep previous guess
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
                        Integrator::BackwardEuler => (c / self.dt) * (v_new - self.cap_v[id]),
                        Integrator::Trapezoidal => {
                            (2.0 * c / self.dt) * (v_new - self.cap_v[id]) - self.cap_i[id]
                        }
                    };
                    self.dev_i[id] = i_new;
                    self.cap_v[id] = v_new;
                    self.cap_i[id] = i_new;
                }
                Device::Inductor { p, n, l } => {
                    let v = self.node_v[p] - self.node_v[n];
                    let i_new = match integ {
                        Integrator::BackwardEuler => (self.dt / l) * v + self.ind_i[id],
                        Integrator::Trapezoidal => {
                            (self.dt / (2.0 * l)) * (v + self.ind_v[id]) + self.ind_i[id]
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
                    let omega_new = omega_prev + (self.dt / params.j) * torque;
                    self.mech_theta[id] += self.dt * omega_new;
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

        self.time += self.dt;
        self.first_step = false;
        self.observe()
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
