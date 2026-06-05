//! WASM binding for the lumped-element circuit simulator.
//!
//! Exposes a stateful [`CircuitSim`] JS class so the app can build a circuit
//! once and step it on every animation frame (rather than re-parsing each tick):
//!
//! ```js
//! const sim = new wasm.CircuitSim(JSON.stringify({ dt: 1e-5, devices: [...] }));
//! const obs = sim.step(20);   // advance 20 timesteps, get { time, nodeVoltages, deviceCurrents }
//! sim.setValue(0, 0);         // open a switch (set a source to 0 V)
//! sim.reset();
//! ```

use serde::{Deserialize, Serialize};
use vcad_ecad_sim::circuit::{Circuit, CircuitEnv, Device, DiodeModel};
use wasm_bindgen::prelude::*;

/// One device in a [`CircuitSpec`]. `p`/`n` are node ids (0 = ground).
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum DeviceSpec {
    Resistor { p: usize, n: usize, value: f64 },
    Capacitor { p: usize, n: usize, value: f64 },
    Inductor { p: usize, n: usize, value: f64 },
    Vsource { p: usize, n: usize, value: f64 },
    Isource { p: usize, n: usize, value: f64 },
    Diode { p: usize, n: usize },
    Led { p: usize, n: usize },
}

impl DeviceSpec {
    fn max_node(&self) -> usize {
        match *self {
            DeviceSpec::Resistor { p, n, .. }
            | DeviceSpec::Capacitor { p, n, .. }
            | DeviceSpec::Inductor { p, n, .. }
            | DeviceSpec::Vsource { p, n, .. }
            | DeviceSpec::Isource { p, n, .. }
            | DeviceSpec::Diode { p, n }
            | DeviceSpec::Led { p, n } => p.max(n),
        }
    }

    fn to_device(&self) -> Device {
        match *self {
            DeviceSpec::Resistor { p, n, value } => Device::Resistor { p, n, r: value },
            DeviceSpec::Capacitor { p, n, value } => Device::Capacitor { p, n, c: value },
            DeviceSpec::Inductor { p, n, value } => Device::Inductor { p, n, l: value },
            DeviceSpec::Vsource { p, n, value } => Device::VSource { p, n, v: value },
            DeviceSpec::Isource { p, n, value } => Device::ISource { p, n, i: value },
            DeviceSpec::Diode { p, n } => Device::Diode {
                p,
                n,
                model: DiodeModel::silicon(),
            },
            DeviceSpec::Led { p, n } => Device::Diode {
                p,
                n,
                model: DiodeModel::led(),
            },
        }
    }
}

/// JSON description of a circuit to simulate.
#[derive(Debug, Deserialize)]
struct CircuitSpec {
    dt: f64,
    devices: Vec<DeviceSpec>,
}

/// Serializable observation handed back to JS (camelCase fields).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WasmObservation {
    time: f64,
    node_voltages: Vec<f64>,
    device_currents: Vec<f64>,
}

/// A live circuit simulation. Build from a [`CircuitSpec`] JSON, then `step`.
#[wasm_bindgen]
pub struct CircuitSim {
    env: CircuitEnv,
}

#[wasm_bindgen]
impl CircuitSim {
    /// Build a simulation from a JSON `{ dt, devices: [...] }` spec.
    #[wasm_bindgen(constructor)]
    pub fn new(spec_json: &str) -> Result<CircuitSim, JsError> {
        let spec: CircuitSpec =
            serde_json::from_str(spec_json).map_err(|e| JsError::new(&e.to_string()))?;

        let max_node = spec.devices.iter().map(|d| d.max_node()).max().unwrap_or(0);
        let mut circuit = Circuit::new();
        while circuit.num_nodes <= max_node {
            circuit.node();
        }
        for d in &spec.devices {
            circuit.add(d.to_device());
        }

        let dt = if spec.dt > 0.0 { spec.dt } else { 1e-5 };
        let mut env = CircuitEnv::new(circuit, dt);
        env.reset();
        Ok(CircuitSim { env })
    }

    /// Advance the simulation by `n` timesteps; returns the final observation.
    #[wasm_bindgen(js_name = step)]
    pub fn step(&mut self, n: usize) -> Result<JsValue, JsError> {
        let mut obs = self.env.observe();
        for _ in 0..n.max(1) {
            obs = self.env.step();
        }
        let out = WasmObservation {
            time: obs.time,
            node_voltages: obs.node_voltages,
            device_currents: obs.device_currents,
        };
        serde_wasm_bindgen::to_value(&out).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Current state without advancing time.
    #[wasm_bindgen(js_name = observe)]
    pub fn observe(&self) -> Result<JsValue, JsError> {
        let obs = self.env.observe();
        let out = WasmObservation {
            time: obs.time,
            node_voltages: obs.node_voltages,
            device_currents: obs.device_currents,
        };
        serde_wasm_bindgen::to_value(&out).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Reset to the power-on state (caps discharged, inductors zero, t = 0).
    #[wasm_bindgen(js_name = reset)]
    pub fn reset(&mut self) {
        self.env.reset();
    }

    /// Mutate a device's primary scalar (drive a switch / PWM / scrubbed value).
    #[wasm_bindgen(js_name = setValue)]
    pub fn set_value(&mut self, device_id: usize, value: f64) {
        self.env.set_value(device_id, value);
    }

    /// The configured timestep (s).
    #[wasm_bindgen(js_name = dt)]
    pub fn dt(&self) -> f64 {
        self.env.dt()
    }
}
