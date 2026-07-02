//! WASM bindings for the atomic domain (feature `wasm`).
//!
//! Mirrors the physics crate's `PhysicsSim` pattern: JSON in, JSON out, so the
//! JS engine wrapper (`packages/engine/src/atoms.ts`) and the MCP tools drive
//! the tested Rust core without a bespoke binding per method. These bindings are
//! compiled into the `vcad-kernel-wasm` bundle (which depends on this crate with
//! the `wasm` feature), so there is a single kernel WASM module for the app.

use wasm_bindgen::prelude::*;

use crate::gym::MdEnv;
use crate::integrate::Thermostat;
use crate::minimize::{minimize, MinimizeOptions};
use crate::potential::{Coulomb, ForceField, HarmonicBonds, LennardJones, Sum};
use crate::system::AtomSystem;
use crate::{inspect, io, receipt};
use vcad_ir::molecule::MoleculeSystem;

/// Force-field / integrator configuration passed from JS as JSON.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct MdConfig {
    /// "lj" (default) or "mlip-stub".
    force_field: String,
    /// LJ well depth (eV).
    epsilon: f64,
    /// LJ sigma (Å).
    sigma: f64,
    /// LJ / Coulomb cutoff (Å).
    cutoff: f64,
    /// Include harmonic bonds using the molecule's bond list.
    use_bonds: bool,
    /// Harmonic bond force constant (eV/Å²).
    bond_k: f64,
    /// Harmonic bond equilibrium length (Å).
    bond_r0: f64,
    /// Include direct Coulomb over partial charges.
    use_coulomb: bool,
    /// Timestep (fs).
    dt: f64,
    /// Optional thermostat target temperature (K); <=0 disables.
    thermostat_k: f64,
    /// Thermostat coupling time (fs).
    thermostat_tau: f64,
}

impl Default for MdConfig {
    fn default() -> Self {
        Self {
            force_field: "lj".into(),
            epsilon: 0.0103,
            sigma: 3.4,
            cutoff: 8.0,
            use_bonds: false,
            bond_k: 20.0,
            bond_r0: 1.5,
            use_coulomb: false,
            dt: 1.0,
            thermostat_k: 0.0,
            thermostat_tau: 100.0,
        }
    }
}

fn build_force_field(cfg: &MdConfig) -> Box<dyn ForceField> {
    let mut terms: Vec<Box<dyn ForceField>> = Vec::new();
    match cfg.force_field.as_str() {
        "mlip-stub" => {
            use crate::mlip::{MlipPotential, PairwiseStubBackend};
            terms.push(Box::new(MlipPotential::new(PairwiseStubBackend {
                cutoff: cfg.cutoff,
                ..Default::default()
            })));
        }
        _ => {
            terms.push(Box::new(LennardJones::monatomic(
                cfg.epsilon,
                cfg.sigma,
                cfg.cutoff,
            )));
        }
    }
    if cfg.use_bonds {
        terms.push(Box::new(HarmonicBonds::uniform(cfg.bond_k, cfg.bond_r0)));
    }
    if cfg.use_coulomb {
        terms.push(Box::new(Coulomb { cutoff: cfg.cutoff }));
    }
    Box::new(Sum::new(terms))
}

fn err(e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}

/// Parse XYZ / extended-XYZ text into a `MoleculeSystem` JSON string.
#[wasm_bindgen]
pub fn atoms_parse_xyz(text: &str) -> Result<String, JsValue> {
    let mol = io::parse_xyz(text).map_err(err)?;
    serde_json::to_string(&mol).map_err(err)
}

/// Serialize a `MoleculeSystem` JSON string to XYZ text.
#[wasm_bindgen]
pub fn atoms_write_xyz(molecule_json: &str) -> Result<String, JsValue> {
    let mol: MoleculeSystem = serde_json::from_str(molecule_json).map_err(err)?;
    Ok(io::write_xyz(&mol))
}

/// Compute a structural report (formula, Rg, bbox, …) as JSON.
#[wasm_bindgen]
pub fn atoms_inspect(molecule_json: &str) -> Result<String, JsValue> {
    let mol: MoleculeSystem = serde_json::from_str(molecule_json).map_err(err)?;
    serde_json::to_string(&inspect::report(&mol)).map_err(err)
}

/// Minimize a structure and return `{ result, molecule }` JSON, where `molecule`
/// is the relaxed structure.
#[wasm_bindgen]
pub fn atoms_minimize(
    molecule_json: &str,
    config_json: &str,
    max_iters: usize,
    force_tol: f64,
) -> Result<String, JsValue> {
    let mol: MoleculeSystem = serde_json::from_str(molecule_json).map_err(err)?;
    let cfg: MdConfig = parse_config(config_json)?;
    let ff = build_force_field(&cfg);
    let mut sys = AtomSystem::from_ir(&mol).map_err(err)?;
    let opts = MinimizeOptions {
        max_iters,
        force_tol,
        ..Default::default()
    };
    let res = minimize(ff.as_ref(), &mut sys, &opts);
    let out = serde_json::json!({
        "result": {
            "converged": res.converged,
            "iters": res.iters,
            "energy": res.energy,
            "maxForce": res.max_force,
        },
        "molecule": sys.to_ir(),
    });
    serde_json::to_string(&out).map_err(err)
}

fn parse_config(config_json: &str) -> Result<MdConfig, JsValue> {
    if config_json.trim().is_empty() {
        Ok(MdConfig::default())
    } else {
        serde_json::from_str(config_json).map_err(err)
    }
}

/// A stateful molecular-dynamics environment exposed to JS.
#[wasm_bindgen]
pub struct MdSim {
    env: MdEnv<Box<dyn ForceField>>,
}

#[wasm_bindgen]
impl MdSim {
    /// Create an environment from a `MoleculeSystem` JSON and config JSON.
    #[wasm_bindgen(constructor)]
    pub fn new(molecule_json: &str, config_json: &str) -> Result<MdSim, JsValue> {
        let mol: MoleculeSystem = serde_json::from_str(molecule_json).map_err(err)?;
        let cfg = parse_config(config_json)?;
        let ff = build_force_field(&cfg);
        let mut env = MdEnv::new(&mol, ff, cfg.dt).map_err(err)?;
        if cfg.thermostat_k > 0.0 {
            env = env.with_thermostat(Thermostat {
                target_k: cfg.thermostat_k,
                tau_fs: cfg.thermostat_tau,
            });
        }
        Ok(MdSim { env })
    }

    /// Run `steps` MD steps; returns an observation JSON.
    pub fn run(&mut self, steps: usize) -> Result<String, JsValue> {
        serde_json::to_string(&self.env.run(steps)).map_err(err)
    }

    /// Current observation JSON without stepping.
    pub fn observe(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.env.observe()).map_err(err)
    }

    /// Reset to the initial structure; returns an observation JSON.
    pub fn reset(&mut self) -> Result<String, JsValue> {
        serde_json::to_string(&self.env.reset().map_err(err)?).map_err(err)
    }

    /// Current structure as a `MoleculeSystem` JSON string.
    #[wasm_bindgen(js_name = moleculeJson)]
    pub fn molecule_json(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.env.to_ir()).map_err(err)
    }
}

/// Build a reproducibility receipt JSON for a completed run.
#[wasm_bindgen]
pub fn atoms_build_receipt(
    molecule_json: &str,
    force_field: &str,
    run: &str,
    params_json: &str,
    outputs_json: &str,
) -> Result<String, JsValue> {
    let mol: MoleculeSystem = serde_json::from_str(molecule_json).map_err(err)?;
    let params: serde_json::Value =
        serde_json::from_str(params_json).unwrap_or(serde_json::Value::Null);
    let outputs: Vec<(String, f64)> = serde_json::from_str(outputs_json).map_err(err)?;
    let r = receipt::SimReceipt::build(&mol, force_field, run, params, outputs);
    serde_json::to_string(&r).map_err(err)
}
