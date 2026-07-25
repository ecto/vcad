//! WASM bindings for `vcad-kernel-enclosure` — enclosure feature extraction
//! and cross-domain PCB ↔ enclosure fit verification.
//!
//! Every entry point returns a **JSON string** rather than a `JsValue`.
//! `serde_wasm_bindgen` renders Rust maps as JS `Map` objects, but the TS
//! consumers (and the `check_enclosure_fit` MCP tool) index the measurement
//! bag as a plain object — and the bag's insertion order is part of the
//! contract. Serializing to text and `JSON.parse`-ing on the TS side preserves
//! both exactly.

use vcad_kernel::vcad_kernel_enclosure as encl;
use wasm_bindgen::prelude::*;

fn to_json<T: serde::Serialize>(value: &T) -> Result<String, JsError> {
    serde_json::to_string(value).map_err(|e| JsError::new(&e.to_string()))
}

fn from_json<T: serde::de::DeserializeOwned>(what: &str, json: &str) -> Result<T, JsError> {
    serde_json::from_str(json).map_err(|e| JsError::new(&format!("invalid {what} JSON: {e}")))
}

/// Extract the cavity, standoffs, and wall openings from an enclosure solid's
/// triangle mesh (flat `[x,y,z,…]` positions + triangle indices).
///
/// Returns `EnclosureFeatures` JSON; `cavity` is `null` when the solid has no
/// open-top pocket (e.g. a solid block).
#[wasm_bindgen]
pub fn enclosure_features(positions: &[f64], indices: &[u32]) -> Result<String, JsError> {
    to_json(&encl::extract_enclosure_features(positions, indices))
}

/// Run the four cross-domain fit checks. Takes `EnclosureFitInput` JSON,
/// returns `EnclosureFitReport` JSON.
#[wasm_bindgen]
pub fn enclosure_fit(input_json: &str) -> Result<String, JsError> {
    let input: encl::EnclosureFitInput = from_json("EnclosureFitInput", input_json)?;
    to_json(&encl::check_enclosure_fit(&input))
}

/// Seed a board from a cavity: inset outline, a hole over every standoff, and
/// the placement that drops it back into the case.
#[wasm_bindgen]
pub fn enclosure_derive_board(
    cavity_json: &str,
    standoffs_json: &str,
    options_json: &str,
) -> Result<String, JsError> {
    let cavity: encl::EnclosureCavity = from_json("EnclosureCavity", cavity_json)?;
    let standoffs: Vec<encl::Standoff> = from_json("Standoff[]", standoffs_json)?;
    let opts: encl::DeriveBoardOptions = from_json("DeriveBoardOptions", options_json)?;
    to_json(&encl::derive_board_from_cavity(&cavity, &standoffs, &opts))
}

/// Mounting holes a board declares (MountingHole footprints + NPTH pads), in
/// board-local coordinates. Takes `Pcb` JSON.
#[wasm_bindgen]
pub fn enclosure_mounting_holes(pcb_json: &str) -> Result<String, JsError> {
    let pcb: encl::PcbLite = from_json("Pcb", pcb_json)?;
    to_json(&encl::mounting_holes_from_pcb(&pcb))
}

/// Edge connectors a board declares, each tagged with the nearest board edge.
#[wasm_bindgen]
pub fn enclosure_connectors(pcb_json: &str, outline_json: &str) -> Result<String, JsError> {
    let pcb: encl::PcbLite = from_json("Pcb", pcb_json)?;
    let outline: encl::BoardOutline = from_json("BoardOutline", outline_json)?;
    to_json(&encl::connectors_from_pcb(&pcb, &outline))
}

/// Per-component Z extents from kernel component meshes (board-local).
#[wasm_bindgen]
pub fn enclosure_component_extents(meshes_json: &str, pcb_json: &str) -> Result<String, JsError> {
    let meshes: Vec<encl::ComponentMeshRef> = from_json("ComponentMesh[]", meshes_json)?;
    let pcb: encl::PcbLite = from_json("Pcb", pcb_json)?;
    to_json(&encl::component_extents_from_meshes(&meshes, &pcb))
}

/// Axis-aligned bounds of a board outline polygon.
#[wasm_bindgen]
pub fn enclosure_outline_aabb(outline_json: &str) -> Result<String, JsError> {
    let outline: encl::BoardOutline = from_json("BoardOutline", outline_json)?;
    to_json(&encl::outline_aabb(&outline))
}

/// Map a board-local point into the enclosure-world frame.
#[wasm_bindgen]
pub fn enclosure_to_world(x: f64, y: f64, z: f64, placement_json: &str) -> Result<String, JsError> {
    let placement: encl::BoardPlacement = from_json("BoardPlacement", placement_json)?;
    to_json(&encl::to_world(x, y, z, &placement))
}
