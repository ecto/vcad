//! WASM bindings for STEP import.
//!
//! This is a lazy-loaded module that provides STEP file import functionality.
//! It returns mesh data rather than Solid objects to avoid cross-module type issues.

use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

/// Initialize the WASM module.
#[wasm_bindgen(start)]
pub fn init() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
    web_sys::console::log_1(&"[WASM] vcad-kernel-wasm-step loaded".into());
}

/// Triangle mesh output for rendering.
#[derive(Serialize, Deserialize)]
pub struct Mesh {
    /// Flat array of vertex positions: [x0, y0, z0, x1, y1, z1, ...]
    pub positions: Vec<f32>,
    /// Flat array of triangle indices: [i0, i1, i2, ...]
    pub indices: Vec<u32>,
}

/// Import a STEP file from raw bytes.
///
/// # Arguments
/// * `data` - The raw STEP file bytes
///
/// # Returns
/// An array of meshes, one for each solid in the STEP file.
#[wasm_bindgen]
pub fn import_step(data: &[u8]) -> Result<JsValue, JsError> {
    let solids = vcad_kernel::Solid::from_step_buffer_all(data)
        .map_err(|e| JsError::new(&e.to_string()))?;

    // Convert each solid to a mesh
    let meshes: Vec<Mesh> = solids
        .iter()
        .map(|solid| {
            let mesh = solid.to_mesh(16); // Lower segments for imported files
            Mesh {
                positions: mesh.vertices,
                indices: mesh.indices,
            }
        })
        .collect();

    serde_wasm_bindgen::to_value(&meshes).map_err(|e| JsError::new(&e.to_string()))
}

/// Check if STEP import is available.
#[wasm_bindgen]
pub fn is_step_available() -> bool {
    true
}
