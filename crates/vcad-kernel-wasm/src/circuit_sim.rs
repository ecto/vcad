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
use vcad_ecad_sim::circuit::{
    ac, adjoint, dc, receipt, BjtModel, Circuit, CircuitEnv, Device, DiodeModel, Integrator,
    MosfetModel, MotorParams,
};
use wasm_bindgen::prelude::*;

/// One device in a [`CircuitSpec`]. `p`/`n` are node ids (0 = ground).
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum DeviceSpec {
    Resistor {
        p: usize,
        n: usize,
        value: f64,
    },
    Capacitor {
        p: usize,
        n: usize,
        value: f64,
    },
    Inductor {
        p: usize,
        n: usize,
        value: f64,
    },
    Vsource {
        p: usize,
        n: usize,
        value: f64,
    },
    Isource {
        p: usize,
        n: usize,
        value: f64,
    },
    Diode {
        p: usize,
        n: usize,
    },
    Led {
        p: usize,
        n: usize,
    },
    Motor {
        p: usize,
        n: usize,
    },
    /// N-channel level-1 MOSFET (drain / gate / source node ids).
    Nmos {
        d: usize,
        g: usize,
        s: usize,
    },
    /// P-channel level-1 MOSFET.
    Pmos {
        d: usize,
        g: usize,
        s: usize,
    },
    /// NPN BJT (collector / base / emitter node ids).
    Npn {
        c: usize,
        b: usize,
        e: usize,
    },
    /// PNP BJT.
    Pnp {
        c: usize,
        b: usize,
        e: usize,
    },
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
            | DeviceSpec::Led { p, n }
            | DeviceSpec::Motor { p, n } => p.max(n),
            DeviceSpec::Nmos { d, g, s } | DeviceSpec::Pmos { d, g, s } => d.max(g).max(s),
            DeviceSpec::Npn { c, b, e } | DeviceSpec::Pnp { c, b, e } => c.max(b).max(e),
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
            DeviceSpec::Motor { p, n } => Device::Motor {
                p,
                n,
                params: MotorParams::small_dc(),
            },
            DeviceSpec::Nmos { d, g, s } => Device::Mosfet {
                d,
                g,
                s,
                model: MosfetModel::nmos(),
            },
            DeviceSpec::Pmos { d, g, s } => Device::Mosfet {
                d,
                g,
                s,
                model: MosfetModel::pmos(),
            },
            DeviceSpec::Npn { c, b, e } => Device::Bjt {
                c,
                b,
                e,
                model: BjtModel::npn(),
            },
            DeviceSpec::Pnp { c, b, e } => Device::Bjt {
                c,
                b,
                e,
                model: BjtModel::pnp(),
            },
        }
    }
}

/// JSON description of a circuit to simulate. `dt` is only used by the
/// transient paths; DC/AC analyses ignore it.
#[derive(Debug, Deserialize)]
struct CircuitSpec {
    #[serde(default)]
    dt: f64,
    devices: Vec<DeviceSpec>,
}

impl CircuitSpec {
    fn parse(spec_json: &str) -> Result<CircuitSpec, JsError> {
        serde_json::from_str(spec_json).map_err(|e| JsError::new(&format!("bad circuit spec: {e}")))
    }

    fn build(&self) -> Circuit {
        let max_node = self.devices.iter().map(|d| d.max_node()).max().unwrap_or(0);
        let mut circuit = Circuit::new();
        while circuit.num_nodes <= max_node {
            circuit.node();
        }
        for d in &self.devices {
            circuit.add(d.to_device());
        }
        circuit
    }
}

/// Serializable observation handed back to JS (camelCase fields).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WasmObservation {
    time: f64,
    node_voltages: Vec<f64>,
    device_currents: Vec<f64>,
    rotor_angles: Vec<f64>,
    rotor_speeds: Vec<f64>,
}

impl From<vcad_ecad_sim::circuit::Observation> for WasmObservation {
    fn from(o: vcad_ecad_sim::circuit::Observation) -> Self {
        WasmObservation {
            time: o.time,
            node_voltages: o.node_voltages,
            device_currents: o.device_currents,
            rotor_angles: o.rotor_angles,
            rotor_speeds: o.rotor_speeds,
        }
    }
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
        let spec = CircuitSpec::parse(spec_json)?;
        let circuit = spec.build();
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
        let out = WasmObservation::from(obs);
        serde_wasm_bindgen::to_value(&out).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Current state without advancing time.
    #[wasm_bindgen(js_name = observe)]
    pub fn observe(&self) -> Result<JsValue, JsError> {
        let out = WasmObservation::from(self.env.observe());
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

// ---------------------------------------------------------------------------
// Stateless analyses: DC operating point, AC sweep, adjoint sensitivities,
// batched transient, and adjoint-driven tuning. Each takes the same
// `{ devices: [...] }` spec JSON as `CircuitSim` and returns camelCase JSON.
// ---------------------------------------------------------------------------

fn json_out<T: Serialize>(out: &T) -> Result<JsValue, JsError> {
    let ser = serde_wasm_bindgen::Serializer::json_compatible();
    out.serialize(&ser)
        .map_err(|e| JsError::new(&e.to_string()))
}

fn check_out_node(circuit: &Circuit, out_node: usize) -> Result<(), JsError> {
    if out_node == 0 || out_node >= circuit.num_nodes {
        return Err(JsError::new(&format!(
            "outNode must be a non-ground node in 1..{} (got {out_node})",
            circuit.num_nodes
        )));
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WasmDcSolution {
    node_voltages: Vec<f64>,
    device_currents: Vec<f64>,
    /// Tellegen residual Σ v·i (W) — the honesty signal: nonzero only
    /// through solver error.
    power_balance_w: f64,
    newton_iterations: usize,
    claim_set: receipt::ClaimSet,
    receipt_claims: Vec<vcad_receipt::ReceiptClaim>,
}

/// DC operating point of a `{ devices: [...] }` circuit spec: node voltages,
/// device currents, the Tellegen power-balance residual, and predicted
/// `vcad.spice-claims/1` claims (Provisional rollup, never Pass).
#[wasm_bindgen(js_name = circuitDcOperatingPoint)]
pub fn circuit_dc_operating_point(spec_json: &str) -> Result<JsValue, JsError> {
    let circuit = CircuitSpec::parse(spec_json)?.build();
    let sol = dc::operating_point(&circuit).map_err(|e| JsError::new(&e.to_string()))?;
    let claim_set = receipt::dc_claims(&sol, circuit.devices.len());
    let receipt_claims = receipt::design_claims(&claim_set);
    json_out(&WasmDcSolution {
        node_voltages: sol.node_voltages.clone(),
        device_currents: sol.device_currents.clone(),
        power_balance_w: sol.power_balance_w,
        newton_iterations: sol.newton_iterations,
        claim_set,
        receipt_claims,
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WasmAcPoint {
    omega: f64,
    /// Real parts of the complex node voltages, indexed by node (0 = ground).
    node_voltages_re: Vec<f64>,
    /// Imaginary parts, same indexing.
    node_voltages_im: Vec<f64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WasmAcResponse {
    source: usize,
    points: Vec<WasmAcPoint>,
}

/// Small-signal AC response driven by device `source_id` (a V or I source)
/// with unit amplitude, solved at each angular frequency in `omegas` (rad/s).
/// Returns per-omega complex node voltages as re/im arrays.
#[wasm_bindgen(js_name = circuitAcResponse)]
pub fn circuit_ac_response(
    spec_json: &str,
    source_id: usize,
    omegas: Vec<f64>,
) -> Result<JsValue, JsError> {
    let circuit = CircuitSpec::parse(spec_json)?.build();
    let mut points = Vec::with_capacity(omegas.len());
    for &omega in &omegas {
        let sol = ac::ac_response(&circuit, source_id, omega)
            .map_err(|e| JsError::new(&format!("AC solve at omega={omega}: {e}")))?;
        points.push(WasmAcPoint {
            omega,
            node_voltages_re: sol.node_voltages.iter().map(|z| z.re).collect(),
            node_voltages_im: sol.node_voltages.iter().map(|z| z.im).collect(),
        });
    }
    json_out(&WasmAcResponse {
        source: source_id,
        points,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AcSensSpec {
    source_id: usize,
    omega: f64,
}

/// Analysis selector for [`circuit_sensitivities`]: `{"dc": true}` or
/// `{"ac": {"sourceId": .., "omega": ..}}`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SensitivitySpec {
    #[serde(default)]
    dc: Option<bool>,
    #[serde(default)]
    ac: Option<AcSensSpec>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WasmSensitivities {
    /// `"dc"` or `"ac"`.
    analysis: String,
    /// DC: the out-node voltage (V). AC: |H(jω)|.
    value: f64,
    /// DC: d(voltage)/d(primary) per device. AC: d|H|/d(primary) per device.
    gradient: Vec<f64>,
    /// AC only: complex dH/dp per device, as re/im arrays.
    #[serde(skip_serializing_if = "Option::is_none")]
    gradient_re: Option<Vec<f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gradient_im: Option<Vec<f64>>,
    /// AC only: complex H(jω).
    #[serde(skip_serializing_if = "Option::is_none")]
    h_re: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    h_im: Option<f64>,
    /// Device ids whose gradient slot is a **placeholder**, not a computed
    /// value (at M0: diodes at AC). Empty for DC.
    deferred: Vec<usize>,
}

/// Adjoint sensitivities of the voltage at `out_node` to every device primary
/// — one extra transposed solve for the whole gradient. `analysis_json`
/// selects `{"dc": true}` or `{"ac": {"sourceId", "omega"}}`.
#[wasm_bindgen(js_name = circuitSensitivities)]
pub fn circuit_sensitivities(
    spec_json: &str,
    out_node: usize,
    analysis_json: &str,
) -> Result<JsValue, JsError> {
    let circuit = CircuitSpec::parse(spec_json)?.build();
    check_out_node(&circuit, out_node)?;
    let sel: SensitivitySpec = serde_json::from_str(analysis_json)
        .map_err(|e| JsError::new(&format!("bad analysis selector: {e}")))?;
    match (sel.dc, sel.ac) {
        (Some(true), None) => {
            let sens = adjoint::dc_sensitivities(&circuit, out_node)
                .map_err(|e| JsError::new(&e.to_string()))?;
            json_out(&WasmSensitivities {
                analysis: "dc".into(),
                value: sens.value,
                gradient: sens.gradient,
                gradient_re: None,
                gradient_im: None,
                h_re: None,
                h_im: None,
                deferred: Vec::new(),
            })
        }
        (None | Some(false), Some(acs)) => {
            let sens = adjoint::ac_sensitivities(&circuit, acs.source_id, acs.omega, out_node)
                .map_err(|e| JsError::new(&e.to_string()))?;
            let d_mag: Vec<f64> = (0..circuit.devices.len())
                .map(|i| sens.d_magnitude(i))
                .collect();
            json_out(&WasmSensitivities {
                analysis: "ac".into(),
                value: sens.h.abs(),
                gradient: d_mag,
                gradient_re: Some(sens.gradient.iter().map(|z| z.re).collect()),
                gradient_im: Some(sens.gradient.iter().map(|z| z.im).collect()),
                h_re: Some(sens.h.re),
                h_im: Some(sens.h.im),
                deferred: sens.deferred,
            })
        }
        _ => Err(JsError::new(
            "analysis selector must be exactly one of {\"dc\": true} or {\"ac\": {\"sourceId\", \"omega\"}}",
        )),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WasmTransient {
    dt: f64,
    /// Sampled times (s), every `sample_every` steps (last step always included).
    times: Vec<f64>,
    /// Sampled node-voltage vectors, one per time.
    node_voltages: Vec<Vec<f64>>,
    /// Device currents at the final step.
    final_device_currents: Vec<f64>,
    /// Tellegen residual of the final solved step (W).
    power_balance_w: f64,
}

/// Batched transient run (trapezoidal integrator): step `steps` times from
/// the power-on state, sampling every `sample_every` steps. Sample count is
/// capped at 5000 — raise `sample_every` for long runs.
#[wasm_bindgen(js_name = circuitTransient)]
pub fn circuit_transient(
    spec_json: &str,
    steps: usize,
    sample_every: usize,
) -> Result<JsValue, JsError> {
    let spec = CircuitSpec::parse(spec_json)?;
    if spec.dt <= 0.0 {
        return Err(JsError::new("transient requires dt > 0 in the spec"));
    }
    if steps == 0 || steps > 2_000_000 {
        return Err(JsError::new("steps must be in 1..=2000000"));
    }
    let every = sample_every.max(1);
    if steps / every > 5000 {
        return Err(JsError::new(&format!(
            "{} samples requested (cap 5000) — raise sampleEvery",
            steps / every
        )));
    }
    let circuit = spec.build();
    let mut env = CircuitEnv::new(circuit, spec.dt);
    env.reset();
    env.set_integrator(Integrator::Trapezoidal);
    let mut times = Vec::new();
    let mut node_voltages = Vec::new();
    let mut obs = env.observe();
    for k in 1..=steps {
        obs = env.step();
        if k % every == 0 || k == steps {
            times.push(obs.time);
            node_voltages.push(obs.node_voltages.clone());
        }
    }
    json_out(&WasmTransient {
        dt: spec.dt,
        times,
        node_voltages,
        final_device_currents: obs.device_currents,
        power_balance_w: env.power_balance(),
    })
}

// ---------------------------------------------------------------------------
// Adjoint-driven tuning — the filter_autotune loop, generalized.
// ---------------------------------------------------------------------------

/// A Butterworth-style AC magnitude target: cutoff + Q of a 2nd-order
/// response at `out_node`, driven by `source_id`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilterTarget {
    cutoff_hz: f64,
    q_factor: f64,
    source_id: usize,
    out_node: usize,
}

/// A DC target: hold `node` at `dc_voltage` volts.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DcTarget {
    node: usize,
    dc_voltage: f64,
}

/// One tunable device: its id plus optional positive bounds on the primary.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FreeDevice {
    device: usize,
    #[serde(default)]
    min: Option<f64>,
    #[serde(default)]
    max: Option<f64>,
}

/// Tune request: exactly one of `filter` / `dc`, plus the devices allowed to
/// move. Tuning runs in log-parameter space, so every free device's primary
/// (and its bounds) must be strictly positive.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TuneSpec {
    #[serde(default)]
    filter: Option<FilterTarget>,
    #[serde(default)]
    dc: Option<DcTarget>,
    free_devices: Vec<FreeDevice>,
    #[serde(default)]
    max_iters: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WasmTunedValue {
    device: usize,
    before: f64,
    after: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WasmBodePoint {
    frequency_hz: f64,
    magnitude_before: f64,
    magnitude_after: f64,
    magnitude_target: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WasmTuneResult {
    tuned_values: Vec<WasmTunedValue>,
    iterations: usize,
    objective_before: f64,
    objective_after: f64,
    /// Filter tune: Bode points before/after/target at the probe frequencies.
    #[serde(skip_serializing_if = "Option::is_none")]
    response: Option<Vec<WasmBodePoint>>,
    /// Filter tune: −3 dB cutoff measured from the tuned response (bisection).
    #[serde(skip_serializing_if = "Option::is_none")]
    achieved_cutoff_hz: Option<f64>,
    /// Filter tune: Q measured as |H| at the −90° phase crossing / |H_dc|.
    #[serde(skip_serializing_if = "Option::is_none")]
    achieved_q_factor: Option<f64>,
    /// DC tune: the achieved node voltage.
    #[serde(skip_serializing_if = "Option::is_none")]
    achieved_dc_voltage: Option<f64>,
    claim_set: receipt::ClaimSet,
    receipt_claims: Vec<vcad_receipt::ReceiptClaim>,
}

/// Validate the free-device list and return the initial (positive) values.
fn free_values(circuit: &Circuit, free: &[FreeDevice]) -> Result<Vec<f64>, JsError> {
    if free.is_empty() {
        return Err(JsError::new("freeDevices must not be empty"));
    }
    free.iter()
        .map(|f| {
            let dev = circuit.devices.get(f.device).ok_or_else(|| {
                JsError::new(&format!("freeDevices: no device with id {}", f.device))
            })?;
            let v = dev.primary();
            if v <= 0.0 {
                return Err(JsError::new(&format!(
                    "free device {} has non-positive primary {v} — log-space tuning needs > 0",
                    f.device
                )));
            }
            if let (Some(lo), Some(hi)) = (f.min, f.max) {
                if lo > hi {
                    return Err(JsError::new(&format!(
                        "free device {}: min {lo} > max {hi}",
                        f.device
                    )));
                }
            }
            if f.min.is_some_and(|lo| lo <= 0.0) {
                return Err(JsError::new(&format!(
                    "free device {}: min must be > 0 for log-space tuning",
                    f.device
                )));
            }
            Ok(v)
        })
        .collect()
}

fn clamp_free(values: &mut [f64], free: &[FreeDevice]) {
    for (v, f) in values.iter_mut().zip(free) {
        if let Some(lo) = f.min {
            *v = v.max(lo);
        }
        if let Some(hi) = f.max {
            *v = v.min(hi);
        }
    }
}

fn with_values(circuit: &Circuit, free: &[FreeDevice], values: &[f64]) -> Circuit {
    let mut c = circuit.clone();
    for (f, &v) in free.iter().zip(values) {
        c.devices[f.device].set_primary(v);
    }
    c
}

/// Analytic 2nd-order low-pass magnitude (the tuning target).
fn target_mag(f: f64, f0: f64, q: f64) -> f64 {
    let x = f / f0;
    1.0 / ((1.0 - x * x).powi(2) + (x / q).powi(2)).sqrt()
}

/// Generic gradient descent in log-parameter space with backtracking line
/// search and a scale-invariant stop — the loop from
/// `vcad-ecad-sim/examples/filter_autotune.rs`, over N free parameters.
/// `eval` returns (J, dJ/d ln p per free parameter).
fn descend(
    mut values: Vec<f64>,
    free: &[FreeDevice],
    max_iters: usize,
    mut eval: impl FnMut(&[f64]) -> Result<(f64, Vec<f64>), JsError>,
) -> Result<(Vec<f64>, f64, f64, usize), JsError> {
    let (mut j, mut grad) = eval(&values)?;
    let j0 = j;
    let norm = |g: &[f64]| g.iter().map(|x| x * x).sum::<f64>().sqrt();
    let mut step = 0.25f64;
    let mut iters = 0usize;
    for _ in 0..max_iters {
        iters += 1;
        let mut accepted = false;
        for _ in 0..40 {
            let scale = 1.0 + norm(&grad);
            let mut trial: Vec<f64> = values
                .iter()
                .zip(&grad)
                .map(|(v, g)| v * (-step * g / scale).exp())
                .collect();
            clamp_free(&mut trial, free);
            let (jt, gt) = eval(&trial)?;
            if jt < j {
                let improvement = (j - jt) / j.max(1e-300);
                values = trial;
                j = jt;
                grad = gt;
                step *= 1.5;
                accepted = improvement > 1e-12;
                break;
            }
            step *= 0.5;
        }
        if !accepted || j < 1e-16 {
            break;
        }
    }
    Ok((values, j0, j, iters))
}

/// |H| at frequency `f` (Hz) for the circuit as configured.
fn mag_at(circuit: &Circuit, source: usize, out: usize, f: f64) -> Result<f64, JsError> {
    let omega = 2.0 * std::f64::consts::PI * f;
    Ok(ac::ac_response(circuit, source, omega)
        .map_err(|e| JsError::new(&format!("AC solve at {f} Hz: {e}")))?
        .node_voltages[out]
        .abs())
}

/// Bisect a scalar monotone-crossing function of log-frequency.
fn bisect_freq(
    mut lo: f64,
    mut hi: f64,
    mut f: impl FnMut(f64) -> Result<f64, JsError>,
) -> Result<Option<f64>, JsError> {
    let (flo, fhi) = (f(lo)?, f(hi)?);
    if flo.signum() == fhi.signum() {
        return Ok(None);
    }
    let rising = fhi > flo;
    for _ in 0..80 {
        let mid = (lo.ln() + hi.ln()).mul_add(0.5, 0.0).exp();
        let fm = f(mid)?;
        if (fm > 0.0) == rising {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Ok(Some((lo * hi).sqrt()))
}

/// Tune the free devices toward the target by adjoint gradient descent.
/// Fails closed if any free device's sensitivity slot is deferred
/// (a placeholder, not a computed gradient — at M0, diodes at AC).
#[wasm_bindgen(js_name = circuitTune)]
pub fn circuit_tune(spec_json: &str, tune_json: &str) -> Result<JsValue, JsError> {
    let circuit = CircuitSpec::parse(spec_json)?.build();
    let tune: TuneSpec = serde_json::from_str(tune_json)
        .map_err(|e| JsError::new(&format!("bad tune spec: {e}")))?;
    let start = free_values(&circuit, &tune.free_devices)?;
    let max_iters = tune.max_iters.unwrap_or(500).min(5000);

    match (tune.filter, tune.dc) {
        (Some(t), None) => tune_filter(&circuit, &tune.free_devices, start, max_iters, &t),
        (None, Some(t)) => tune_dc(&circuit, &tune.free_devices, start, max_iters, &t),
        _ => Err(JsError::new(
            "tune spec must carry exactly one of `filter` or `dc`",
        )),
    }
}

fn tune_filter(
    circuit: &Circuit,
    free: &[FreeDevice],
    start: Vec<f64>,
    max_iters: usize,
    t: &FilterTarget,
) -> Result<JsValue, JsError> {
    check_out_node(circuit, t.out_node)?;
    if t.cutoff_hz <= 0.0
        || t.q_factor <= 0.0
        || !t.cutoff_hz.is_finite()
        || !t.q_factor.is_finite()
    {
        return Err(JsError::new("cutoffHz and qFactor must be > 0"));
    }
    // 25 log-spaced probes over two decades centered on the target cutoff.
    let probes: Vec<f64> = (0..25)
        .map(|i| (t.cutoff_hz / 10.0) * 10f64.powf(2.0 * i as f64 / 24.0))
        .collect();
    let targets: Vec<f64> = probes
        .iter()
        .map(|&f| target_mag(f, t.cutoff_hz, t.q_factor))
        .collect();

    let eval = |values: &[f64]| -> Result<(f64, Vec<f64>), JsError> {
        let ckt = with_values(circuit, free, values);
        let mut j = 0.0;
        let mut grad = vec![0.0f64; free.len()];
        for (k, &f) in probes.iter().enumerate() {
            let omega = 2.0 * std::f64::consts::PI * f;
            let sens = adjoint::ac_sensitivities(&ckt, t.source_id, omega, t.out_node)
                .map_err(|e| JsError::new(&format!("adjoint solve at {f} Hz: {e}")))?;
            for fd in free {
                if sens.is_deferred(fd.device) {
                    return Err(JsError::new(&format!(
                        "free device {} has a deferred (placeholder) AC sensitivity — cannot tune it",
                        fd.device
                    )));
                }
            }
            let err = sens.h.abs() - targets[k];
            j += err * err;
            for (gi, fd) in free.iter().enumerate() {
                // Chain to log-space: d/d ln p = p · d/dp.
                grad[gi] += 2.0 * err * sens.d_magnitude(fd.device) * values[gi];
            }
        }
        Ok((j, grad))
    };

    let (tuned, j0, j, iters) = descend(start.clone(), free, max_iters, eval)?;
    let before_ckt = with_values(circuit, free, &start);
    let after_ckt = with_values(circuit, free, &tuned);

    let response: Vec<WasmBodePoint> = probes
        .iter()
        .zip(&targets)
        .map(|(&f, &tm)| {
            Ok(WasmBodePoint {
                frequency_hz: f,
                magnitude_before: mag_at(&before_ckt, t.source_id, t.out_node, f)?,
                magnitude_after: mag_at(&after_ckt, t.source_id, t.out_node, f)?,
                magnitude_target: tm,
            })
        })
        .collect::<Result<_, JsError>>()?;

    // Measure the achieved response rather than trusting closed forms:
    // cutoff = the −3 dB crossing of |H|/|H_low|; Q = |H|/|H_low| at the
    // −90° phase crossing (exact for a 2nd-order section).
    let f_lo = t.cutoff_hz / 1000.0;
    let f_hi = t.cutoff_hz * 1000.0;
    let h_low = mag_at(&after_ckt, t.source_id, t.out_node, f_lo)?;
    let achieved_cutoff = bisect_freq(f_lo, f_hi, |f| {
        Ok(mag_at(&after_ckt, t.source_id, t.out_node, f)? / h_low
            - std::f64::consts::FRAC_1_SQRT_2)
    })?;
    let phase_at = |f: f64| -> Result<f64, JsError> {
        let omega = 2.0 * std::f64::consts::PI * f;
        Ok(ac::ac_response(&after_ckt, t.source_id, omega)
            .map_err(|e| JsError::new(&e.to_string()))?
            .node_voltages[t.out_node]
            .arg())
    };
    let f_90 = bisect_freq(f_lo, f_hi, |f| {
        Ok(phase_at(f)? + std::f64::consts::FRAC_PI_2)
    })?;
    let achieved_q = match f_90 {
        Some(f0) => Some(mag_at(&after_ckt, t.source_id, t.out_node, f0)? / h_low),
        None => None,
    };

    let claim_set = receipt::filter_claims(
        achieved_cutoff.unwrap_or(f64::NAN),
        achieved_q.unwrap_or(f64::NAN),
        circuit.num_nodes,
        circuit.devices.len(),
    );
    let receipt_claims = receipt::design_claims(&claim_set);
    json_out(&WasmTuneResult {
        tuned_values: free
            .iter()
            .zip(start.iter().zip(&tuned))
            .map(|(f, (&b, &a))| WasmTunedValue {
                device: f.device,
                before: b,
                after: a,
            })
            .collect(),
        iterations: iters,
        objective_before: j0,
        objective_after: j,
        response: Some(response),
        achieved_cutoff_hz: achieved_cutoff,
        achieved_q_factor: achieved_q,
        achieved_dc_voltage: None,
        claim_set,
        receipt_claims,
    })
}

fn tune_dc(
    circuit: &Circuit,
    free: &[FreeDevice],
    start: Vec<f64>,
    max_iters: usize,
    t: &DcTarget,
) -> Result<JsValue, JsError> {
    check_out_node(circuit, t.node)?;
    let eval = |values: &[f64]| -> Result<(f64, Vec<f64>), JsError> {
        let ckt = with_values(circuit, free, values);
        let sens =
            adjoint::dc_sensitivities(&ckt, t.node).map_err(|e| JsError::new(&e.to_string()))?;
        let err = sens.value - t.dc_voltage;
        let grad = free
            .iter()
            .enumerate()
            .map(|(gi, fd)| 2.0 * err * sens.gradient[fd.device] * values[gi])
            .collect();
        Ok((err * err, grad))
    };

    let (tuned, j0, j, iters) = descend(start.clone(), free, max_iters, eval)?;
    let after_ckt = with_values(circuit, free, &tuned);
    let sol = dc::operating_point(&after_ckt).map_err(|e| JsError::new(&e.to_string()))?;
    let achieved = sol.node_voltages[t.node];
    let claim_set = receipt::dc_claims(&sol, circuit.devices.len());
    let receipt_claims = receipt::design_claims(&claim_set);
    json_out(&WasmTuneResult {
        tuned_values: free
            .iter()
            .zip(start.iter().zip(&tuned))
            .map(|(f, (&b, &a))| WasmTunedValue {
                device: f.device,
                before: b,
                after: a,
            })
            .collect(),
        iterations: iters,
        objective_before: j0,
        objective_after: j,
        response: None,
        achieved_cutoff_hz: None,
        achieved_q_factor: None,
        achieved_dc_voltage: Some(achieved),
        claim_set,
        receipt_claims,
    })
}

// ---------------------------------------------------------------------------
// Schematic → circuit mapping (the #583 netlist seam, over WASM).
// ---------------------------------------------------------------------------

/// One supply rail to inject as an independent voltage source (net → node,
/// referenced to ground).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SupplySpec {
    net: String,
    volts: f64,
}

/// Options for [`circuit_from_schematic_wasm`], all optional.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MapOptions {
    /// Reference designators stubbed as open circuits (connectors, power
    /// symbols, unpopulated ICs).
    #[serde(default)]
    stub_as_open: Vec<String>,
    /// Extra net names to collapse onto ground (node 0) beyond the
    /// GND/VSS/0-style names the converter recognizes on its own — the app
    /// resolves ground by power *symbol*, not net name, and passes the result
    /// here.
    #[serde(default)]
    ground_nets: Vec<String>,
    /// Supply rails: each net listed here (if any mapped device touches it)
    /// gets an injected `vsource` to ground at the given voltage.
    #[serde(default)]
    supplies: Vec<SupplySpec>,
}

/// One serialized device of the mapped circuit, in the same `{kind,p,n,value}`
/// shape every `circuit*` analysis entry point parses.
#[derive(Debug, Serialize)]
struct SpecDeviceOut {
    kind: &'static str,
    p: usize,
    n: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<f64>,
}

/// One component that blocked the conversion (fail-closed: if any component
/// can't be mapped, nothing simulates and every blocker is listed).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BlockerOut {
    reference: String,
    message: String,
}

/// Result of [`circuit_from_schematic_wasm`]: either the mapped circuit spec
/// plus the name↔id bookkeeping, or the full blocker list. Failure is data,
/// not an exception, so the app can pin each blocker on its component.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MappedCircuitOut {
    ok: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    blockers: Vec<BlockerOut>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    devices: Vec<SpecDeviceOut>,
    num_nodes: usize,
    node_of_net: std::collections::BTreeMap<String, usize>,
    device_of_ref: std::collections::BTreeMap<String, usize>,
    ground_nets: Vec<String>,
    stubbed: Vec<String>,
    /// Injected supply rails: net name → device id of the added vsource.
    supply_source_of_net: std::collections::BTreeMap<String, usize>,
    /// Supplies requested but touching no mapped device (nothing injected).
    unconnected_supplies: Vec<String>,
}

/// Map a schematic sheet to a simulatable circuit spec via the fail-closed
/// netlist seam (`vcad-ecad-sim::circuit::netlist`).
///
/// * `sch_json` — JSON-serialized `SchematicSheet` (same shape as
///   `ecadGenerateNetlist` takes).
/// * `options_json` — JSON [`MapOptions`] (`{}` for defaults).
///
/// Returns `{ok: true, devices, nodeOfNet, deviceOfRef, ...}` on success, or
/// `{ok: false, blockers: [{reference, message}]}` when any component can't
/// be mapped — nothing is silently skipped.
#[wasm_bindgen(js_name = circuitFromSchematic)]
pub fn circuit_from_schematic_wasm(sch_json: &str, options_json: &str) -> Result<JsValue, JsError> {
    use vcad_ecad_sim::circuit::netlist::{
        circuit_from_netlist, ConvertError, ConvertOptions, SimComponent,
    };

    let sheet: vcad_ir::ecad::SchematicSheet = serde_json::from_str(sch_json)
        .map_err(|e| JsError::new(&format!("bad schematic JSON: {e}")))?;
    let options: MapOptions = serde_json::from_str(if options_json.trim().is_empty() {
        "{}"
    } else {
        options_json
    })
    .map_err(|e| JsError::new(&format!("bad options JSON: {e}")))?;

    let mut netlist = vcad_ecad_schematic::generate_netlist(&sheet);

    // Collapse caller-declared ground nets onto a canonical ground name so the
    // converter maps them to node 0. Remember the aliases so the returned
    // node map still answers by the original net name.
    let ground_aliases: std::collections::BTreeSet<String> =
        options.ground_nets.iter().cloned().collect();
    for net in &mut netlist.nets {
        if ground_aliases.contains(&net.name) {
            net.name = "GND".to_string();
        }
    }

    let components: Vec<SimComponent> = sheet
        .components
        .iter()
        .map(|c| SimComponent {
            reference: c.reference.clone(),
            value: c.value.clone(),
        })
        .collect();

    let convert_options = ConvertOptions {
        stub_as_open: options.stub_as_open.clone(),
    };
    let mut mapped = match circuit_from_netlist(&components, &netlist, &convert_options) {
        Ok(m) => m,
        Err(ConvertError::Unmappable { blockers }) => {
            return json_out(&MappedCircuitOut {
                ok: false,
                blockers: blockers
                    .into_iter()
                    .map(|b| BlockerOut {
                        message: b.reason.to_string(),
                        reference: b.reference,
                    })
                    .collect(),
                devices: Vec::new(),
                num_nodes: 0,
                node_of_net: Default::default(),
                device_of_ref: Default::default(),
                ground_nets: Vec::new(),
                stubbed: Vec::new(),
                supply_source_of_net: Default::default(),
                unconnected_supplies: Vec::new(),
            });
        }
        Err(e @ ConvertError::UnknownStub(_)) => return Err(JsError::new(&e.to_string())),
    };

    // Inject one vsource-to-ground per supply rail that a mapped device
    // actually touches (an untouched supply must not become a dangling node).
    let mut supply_source_of_net = std::collections::BTreeMap::new();
    let mut unconnected_supplies = Vec::new();
    for supply in &options.supplies {
        match mapped.node_of_net.get(&supply.net).copied() {
            Some(node) if node != 0 => {
                let id = mapped.circuit.add(Device::VSource {
                    p: node,
                    n: 0,
                    v: supply.volts,
                });
                supply_source_of_net.insert(supply.net.clone(), id);
            }
            _ => unconnected_supplies.push(supply.net.clone()),
        }
    }

    // Serialize devices in the `{kind,p,n,value}` spec vocabulary. The
    // converter only emits R/C/L/V/I/diode; LED-valued diodes keep the LED
    // model by reporting kind "led" (matched by the component value string,
    // the same rule the converter used to pick the model).
    let led_refs: std::collections::BTreeSet<usize> = mapped
        .device_of_ref
        .iter()
        .filter_map(|(reference, &id)| {
            let comp = components.iter().find(|c| &c.reference == reference)?;
            comp.value
                .to_ascii_uppercase()
                .contains("LED")
                .then_some(id)
        })
        .collect();
    let devices: Vec<SpecDeviceOut> = mapped
        .circuit
        .devices
        .iter()
        .enumerate()
        .map(|(id, d)| match *d {
            Device::Resistor { p, n, r } => SpecDeviceOut {
                kind: "resistor",
                p,
                n,
                value: Some(r),
            },
            Device::Capacitor { p, n, c } => SpecDeviceOut {
                kind: "capacitor",
                p,
                n,
                value: Some(c),
            },
            Device::Inductor { p, n, l } => SpecDeviceOut {
                kind: "inductor",
                p,
                n,
                value: Some(l),
            },
            Device::VSource { p, n, v } => SpecDeviceOut {
                kind: "vsource",
                p,
                n,
                value: Some(v),
            },
            Device::ISource { p, n, i } => SpecDeviceOut {
                kind: "isource",
                p,
                n,
                value: Some(i),
            },
            Device::Diode { p, n, .. } => SpecDeviceOut {
                kind: if led_refs.contains(&id) {
                    "led"
                } else {
                    "diode"
                },
                p,
                n,
                value: None,
            },
            ref other => unreachable!("netlist seam emitted unexpected device {other:?}"),
        })
        .collect();

    // Answer node lookups by the original net names too (ground aliases → 0).
    let mut node_of_net = mapped.node_of_net.clone();
    for alias in &ground_aliases {
        node_of_net.insert(alias.clone(), 0);
    }
    let mut ground_nets = mapped.ground_nets.clone();
    for alias in &ground_aliases {
        if !ground_nets.contains(alias) {
            ground_nets.push(alias.clone());
        }
    }
    json_out(&MappedCircuitOut {
        ok: true,
        blockers: Vec::new(),
        devices,
        num_nodes: mapped.circuit.num_nodes,
        node_of_net,
        device_of_ref: mapped.device_of_ref,
        ground_nets,
        stubbed: mapped.stubbed,
        supply_source_of_net,
        unconnected_supplies,
    })
}
