//! WASM bindings for the differentiable PCB design engine.
//!
//! Exposes the Rust kernel solvers — PDN copper sizing (with the
//! implicit-function adjoint through the linear solve) and the differentiable
//! plant/controller co-design — to JavaScript, so the MCP server can route
//! coupled/mesh problems into the real engine instead of the TS approximations.
//! In/out is JSON strings (no `serde-wasm-bindgen` needed).

use serde::Deserialize;
use vcad_ecad_diff::{codesign_motor, PdnEdge, PdnSystem, SolverConfig};
use wasm_bindgen::prelude::*;

#[derive(Deserialize)]
struct EdgeSpec {
    a: usize,
    b: usize,
    length: f64,
}

#[derive(Deserialize)]
struct PdnSpec {
    nodes: usize,
    edges: Vec<EdgeSpec>,
    loads: Vec<(usize, f64)>,
    targets: Vec<(usize, f64)>,
    #[serde(default = "default_sigma")]
    sigma: f64,
    #[serde(default = "default_thickness")]
    thickness: f64,
    #[serde(default = "default_min_width")]
    min_width: f64,
    #[serde(default = "default_max_width")]
    max_width: f64,
    #[serde(default = "default_seed")]
    seed_width: f64,
}

fn default_sigma() -> f64 {
    1.0 / 1.68e-5
}
fn default_thickness() -> f64 {
    0.035
}
fn default_min_width() -> f64 {
    0.1
}
fn default_max_width() -> f64 {
    5.0
}
fn default_seed() -> f64 {
    0.5
}

/// Size a PDN resistor mesh in the Rust engine (analytic IFT adjoint). Takes a
/// JSON spec, returns a JSON result `{widths_mm, drops_v, converged,
/// iterations, residual_norm}` (or `{error}`).
#[wasm_bindgen]
pub fn size_pdn(spec_json: &str) -> String {
    let spec: PdnSpec = match serde_json::from_str(spec_json) {
        Ok(s) => s,
        Err(e) => return format!("{{\"error\":\"parse: {e}\"}}"),
    };
    let ne = spec.edges.len();
    let edges: Vec<PdnEdge> = spec
        .edges
        .iter()
        .map(|e| PdnEdge::new(e.a, e.b, e.length))
        .collect();
    let sys = PdnSystem::new(
        spec.nodes,
        edges,
        spec.loads,
        spec.targets,
        spec.sigma,
        spec.thickness,
    )
    .with_bounds(vec![spec.min_width; ne], vec![spec.max_width; ne]);
    let mut widths = vec![spec.seed_width; ne];
    let res = sys.solve(&mut widths, &SolverConfig::default());
    let drops = sys.drops(&widths);
    serde_json::json!({
        "widths_mm": widths,
        "drops_v": drops,
        "converged": res.converged,
        "iterations": res.iterations,
        "residual_norm": res.residual_norm,
    })
    .to_string()
}

/// Co-design a 1-DOF motor (plant geometry + controller gain) in the Rust
/// engine. Input `{seed_radius_mm, seed_kp}`; returns `{outer_radius_mm, kp}`.
#[wasm_bindgen]
pub fn codesign_motor_json(spec_json: &str) -> String {
    #[derive(Deserialize)]
    struct Seed {
        #[serde(default = "default_r")]
        seed_radius_mm: f64,
        #[serde(default = "default_kp")]
        seed_kp: f64,
    }
    fn default_r() -> f64 {
        20.0
    }
    fn default_kp() -> f64 {
        5.0
    }
    let seed: Seed = match serde_json::from_str(spec_json) {
        Ok(s) => s,
        Err(e) => return format!("{{\"error\":\"parse: {e}\"}}"),
    };
    let (sol, res) = codesign_motor([seed.seed_radius_mm, seed.seed_kp]);
    serde_json::json!({
        "outer_radius_mm": sol[0],
        "kp": sol[1],
        "iterations": res.iterations,
    })
    .to_string()
}
