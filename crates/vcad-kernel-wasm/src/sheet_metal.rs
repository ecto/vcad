//! WASM bindings for sheet-metal evaluation.
//!
//! The web app never builds a [`SheetMetalModel`] itself. Instead it
//! describes the desired ops as a base-to-tip chain, hands the chain JSON to
//! [`evaluate_sheet_metal_chain`], and receives a bundle containing:
//!
//! - A triangle mesh suitable for the existing scene renderer.
//! - The projected flat pattern (panels + holes + creases) in global flat 2D.
//! - A compact snapshot of every panel and bend (thickness, K-factor and
//!   provenance, allowance, etc.) so the property panel and DFM tools can
//!   render the manufacturing-side view without a second WASM call.
//!
//! Single JSON-over-the-boundary entry point keeps the bindings narrow and
//! lets us extend ops without touching `wasm-bindgen` signatures.
//!
//! The chain JSON shape mirrors the matching IR variants:
//! ```text
//! [
//!   {"type":"BaseFlangeRect","width":100,"depth":50,"thickness":1.0,"material":"Al-soft"},
//!   {"type":"EdgeFlange","panelId":0,"edgeIndex":0,"length":25,"angle":1.5707963,"radius":1.0,"direction":"Up"}
//! ]
//! ```
//!
//! See `docs/design/sheet-metal.md` for the strategic vision; this module is
//! the foundation tier exposed to the web app.

use serde::{Deserialize, Serialize};
use vcad_kernel_sheet::{
    add_edge_flange, base_flange_rect,
    bend_table::{self, BendTable},
    edge_flange::EdgeFlangeParams,
    BendDirection, FlangePosition, FlatPattern, SheetMetalModel,
};
use wasm_bindgen::prelude::*;

/// One op in a sheet-metal evaluation chain. Tag uses the same wire spelling
/// the IR variants ship under, minus the `SheetMetal` prefix.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
enum ChainOp {
    /// Initialise the model from an axis-aligned rectangle in the XY plane.
    BaseFlangeRect {
        width: f64,
        depth: f64,
        thickness: f64,
        /// Material name for K-factor lookup (e.g. `"Al-soft"`).
        material: String,
    },
    /// Add a flange off `edge_index` of `panel_id`.
    EdgeFlange {
        panel_id: usize,
        edge_index: usize,
        length: f64,
        angle: f64,
        radius: f64,
        direction: BendDirection,
        /// Optional manual K-factor override (skips bend-table lookup).
        manual_k: Option<f64>,
    },
}

/// What the binding returns to the web app. `mesh` is empty on error;
/// `error` is non-empty when something went wrong inside the kernel.
#[derive(Debug, Clone, Serialize, Default)]
struct SheetMetalEvalResult {
    mesh: MeshDto,
    flat_pattern: FlatPatternDto,
    model: ModelSummaryDto,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
struct MeshDto {
    positions: Vec<f32>,
    indices: Vec<u32>,
    normals: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Default)]
struct FlatPatternDto {
    thickness: f64,
    panel_outlines_2d: Vec<Vec<[f64; 2]>>,
    panel_holes_2d: Vec<Vec<Vec<[f64; 2]>>>,
    creases: Vec<FlatCreaseDto>,
    area_mm2: f64,
    /// `[min_x, min_y, max_x, max_y]`.
    bbox: [f64; 4],
}

#[derive(Debug, Clone, Serialize)]
struct FlatCreaseDto {
    line: [[f64; 2]; 2],
    angle: f64,
    radius: f64,
    k_factor: f64,
    k_factor_source: Option<String>,
    direction: BendDirection,
    bend_id: usize,
}

/// Per-panel + per-bend summary surfaced to the property panel. The full
/// `SheetMetalModel` lives inside the WASM heap; this is the user-facing
/// projection (no frames, no internal IDs that wouldn't survive across
/// rebuilds).
#[derive(Debug, Clone, Serialize, Default)]
struct ModelSummaryDto {
    thickness: f64,
    panel_count: usize,
    bend_count: usize,
    bends: Vec<BendSummaryDto>,
}

#[derive(Debug, Clone, Serialize)]
struct BendSummaryDto {
    parent: usize,
    child: usize,
    angle_rad: f64,
    radius: f64,
    direction: BendDirection,
    k_factor: f64,
    k_factor_source: Option<String>,
    /// `θ · (R + K · t)`.
    allowance_mm: f64,
}

/// Evaluate a chain of sheet-metal ops and return `(mesh, flat-pattern,
/// model-summary)` as a JSON string. Caller is responsible for parsing.
///
/// On error, returns a JSON object with a non-null `error` field; the other
/// fields are zeroed. Never panics — every fallible kernel call is mapped
/// to an error string.
#[wasm_bindgen(js_name = evaluateSheetMetalChain)]
pub fn evaluate_sheet_metal_chain(chain_json: &str) -> String {
    let result = match build_result(chain_json) {
        Ok(r) => r,
        Err(msg) => SheetMetalEvalResult {
            error: Some(msg),
            ..Default::default()
        },
    };
    serde_json::to_string(&result).unwrap_or_else(|_| r#"{"error":"serialize failed"}"#.to_string())
}

fn build_result(chain_json: &str) -> Result<SheetMetalEvalResult, String> {
    let chain: Vec<ChainOp> =
        serde_json::from_str(chain_json).map_err(|e| format!("chain JSON: {e}"))?;
    if chain.is_empty() {
        return Err("empty op chain".to_string());
    }
    let table = BendTable::builtin();
    let mut model = build_model(&chain, &table)?;
    // Recompute flat frames before projecting — the kernel keeps them in
    // sync after every `add_edge_flange`, but `unfold` is the source of
    // truth and is a cheap call on a tree of this size.
    vcad_kernel_sheet::unfold(&mut model).map_err(|e| format!("unfold: {e:?}"))?;

    let mesh = tessellate_model(&model);
    let flat = FlatPattern::from_model(&model);
    let flat_dto = flat_pattern_to_dto(flat);
    let summary = summarise_model(&model);
    Ok(SheetMetalEvalResult {
        mesh,
        flat_pattern: flat_dto,
        model: summary,
        error: None,
    })
}

fn build_model(chain: &[ChainOp], table: &BendTable) -> Result<SheetMetalModel, String> {
    let mut iter = chain.iter();
    let base = iter
        .next()
        .ok_or_else(|| "chain has no base flange".to_string())?;
    let mut model = match base {
        ChainOp::BaseFlangeRect {
            width,
            depth,
            thickness,
            ..
        } => {
            base_flange_rect(*width, *depth, *thickness).map_err(|e| format!("base flange: {e}"))?
        }
        ChainOp::EdgeFlange { .. } => {
            return Err("first chain op must be a base flange".to_string());
        }
    };
    for (i, op) in iter.enumerate() {
        match op {
            ChainOp::BaseFlangeRect { .. } => {
                return Err(format!(
                    "chain op #{} is a base flange (only one allowed)",
                    i + 1
                ));
            }
            ChainOp::EdgeFlange {
                panel_id,
                edge_index,
                length,
                angle,
                radius,
                direction,
                manual_k,
            } => {
                let material = match base {
                    ChainOp::BaseFlangeRect { material, .. } => material.clone(),
                    _ => String::new(),
                };
                let params = EdgeFlangeParams {
                    panel: *panel_id,
                    edge_index: *edge_index,
                    length: *length,
                    angle: *angle,
                    radius: *radius,
                    direction: *direction,
                    position: FlangePosition::MaterialInside,
                    material,
                    manual_k: *manual_k,
                };
                add_edge_flange(&mut model, table, params)
                    .map_err(|e| format!("edge flange #{}: {e}", i + 1))?;
            }
        }
    }
    Ok(model)
}

/// Foundation-tier tessellation: each panel becomes a thickness-`t` slab
/// (top + bottom + side quads). Bend regions are not yet rendered as
/// cylinders — adjacent panels meet at the hinge. The bend cylinders land
/// alongside the springback / lofted-flange tier; this is sufficient for
/// the property panel + flat pattern to be visibly meaningful.
fn tessellate_model(model: &SheetMetalModel) -> MeshDto {
    let mut positions: Vec<f32> = Vec::new();
    let mut normals: Vec<f32> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let half_t = model.thickness * 0.5;
    for panel in &model.panels {
        let frame = panel.frame_bent;
        let n = frame.normal();
        let outline = &panel.outline;
        if outline.len() < 3 {
            continue;
        }
        // Top vertices (outside face, +n side).
        let base_top = (positions.len() / 3) as u32;
        for p in outline {
            let w = frame.to_world(*p);
            positions.push((w.x + n.x * half_t) as f32);
            positions.push((w.y + n.y * half_t) as f32);
            positions.push((w.z + n.z * half_t) as f32);
            normals.push(n.x as f32);
            normals.push(n.y as f32);
            normals.push(n.z as f32);
        }
        // Bottom vertices.
        let base_bot = (positions.len() / 3) as u32;
        for p in outline {
            let w = frame.to_world(*p);
            positions.push((w.x - n.x * half_t) as f32);
            positions.push((w.y - n.y * half_t) as f32);
            positions.push((w.z - n.z * half_t) as f32);
            normals.push(-n.x as f32);
            normals.push(-n.y as f32);
            normals.push(-n.z as f32);
        }
        // Triangulate top with fan from vertex 0.
        for i in 1..(outline.len() - 1) {
            indices.push(base_top);
            indices.push(base_top + i as u32);
            indices.push(base_top + (i + 1) as u32);
        }
        // Triangulate bottom (reverse winding).
        for i in 1..(outline.len() - 1) {
            indices.push(base_bot);
            indices.push(base_bot + (i + 1) as u32);
            indices.push(base_bot + i as u32);
        }
        // Side walls: one quad per outline edge with its own normals.
        for i in 0..outline.len() {
            let a = outline[i];
            let b = outline[(i + 1) % outline.len()];
            let a_world = frame.to_world(a);
            let b_world = frame.to_world(b);
            let edge_x = b_world.x - a_world.x;
            let edge_y = b_world.y - a_world.y;
            let edge_z = b_world.z - a_world.z;
            // Side normal = edge × n, normalised.
            let mut snx = edge_y * n.z - edge_z * n.y;
            let mut sny = edge_z * n.x - edge_x * n.z;
            let mut snz = edge_x * n.y - edge_y * n.x;
            let m = (snx * snx + sny * sny + snz * snz).sqrt();
            if m > 1e-12 {
                snx /= m;
                sny /= m;
                snz /= m;
            } else {
                snx = 0.0;
                sny = 0.0;
                snz = 1.0;
            }
            let base = (positions.len() / 3) as u32;
            // a_top, b_top, b_bot, a_bot.
            let push = |positions: &mut Vec<f32>, normals: &mut Vec<f32>, x, y, z| {
                positions.push(x);
                positions.push(y);
                positions.push(z);
                normals.push(snx as f32);
                normals.push(sny as f32);
                normals.push(snz as f32);
            };
            push(
                &mut positions,
                &mut normals,
                (a_world.x + n.x * half_t) as f32,
                (a_world.y + n.y * half_t) as f32,
                (a_world.z + n.z * half_t) as f32,
            );
            push(
                &mut positions,
                &mut normals,
                (b_world.x + n.x * half_t) as f32,
                (b_world.y + n.y * half_t) as f32,
                (b_world.z + n.z * half_t) as f32,
            );
            push(
                &mut positions,
                &mut normals,
                (b_world.x - n.x * half_t) as f32,
                (b_world.y - n.y * half_t) as f32,
                (b_world.z - n.z * half_t) as f32,
            );
            push(
                &mut positions,
                &mut normals,
                (a_world.x - n.x * half_t) as f32,
                (a_world.y - n.y * half_t) as f32,
                (a_world.z - n.z * half_t) as f32,
            );
            indices.push(base);
            indices.push(base + 1);
            indices.push(base + 2);
            indices.push(base);
            indices.push(base + 2);
            indices.push(base + 3);
        }
    }
    MeshDto {
        positions,
        indices,
        normals,
    }
}

fn flat_pattern_to_dto(flat: FlatPattern) -> FlatPatternDto {
    let bbox = flat.bbox();
    let panel_outlines_2d = flat
        .panel_outlines_2d
        .iter()
        .map(|o| o.iter().map(|p| [p.x, p.y]).collect::<Vec<[f64; 2]>>())
        .collect();
    let panel_holes_2d = flat
        .panel_holes_2d
        .iter()
        .map(|panel_holes| {
            panel_holes
                .iter()
                .map(|hole| hole.iter().map(|p| [p.x, p.y]).collect())
                .collect()
        })
        .collect();
    let creases = flat
        .creases
        .iter()
        .map(|c| FlatCreaseDto {
            line: [[c.line.0.x, c.line.0.y], [c.line.1.x, c.line.1.y]],
            angle: c.angle,
            radius: c.radius,
            k_factor: c.k_factor,
            k_factor_source: c.k_factor_source.clone(),
            direction: c.direction,
            bend_id: c.bend_id,
        })
        .collect();
    FlatPatternDto {
        thickness: flat.thickness,
        panel_outlines_2d,
        panel_holes_2d,
        creases,
        area_mm2: flat.area_mm2,
        bbox: [bbox.0 .0, bbox.0 .1, bbox.1 .0, bbox.1 .1],
    }
}

fn summarise_model(model: &SheetMetalModel) -> ModelSummaryDto {
    let bends = model
        .bends
        .iter()
        .map(|b| BendSummaryDto {
            parent: b.parent,
            child: b.child,
            angle_rad: b.angle,
            radius: b.radius,
            direction: b.direction,
            k_factor: b.k_factor,
            k_factor_source: b.k_factor_source.clone(),
            allowance_mm: bend_table::bend_allowance(
                b.angle,
                b.radius,
                b.k_factor,
                model.thickness,
            ),
        })
        .collect();
    ModelSummaryDto {
        thickness: model.thickness,
        panel_count: model.panels.len(),
        bend_count: model.bends.len(),
        bends,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_u_channel() {
        let chain = r#"[
            {"type":"BaseFlangeRect","width":100,"depth":50,"thickness":1.0,"material":"Al-soft"},
            {"type":"EdgeFlange","panelId":0,"edgeIndex":0,"length":25,"angle":1.5707963267948966,"radius":1.0,"direction":"Up"},
            {"type":"EdgeFlange","panelId":0,"edgeIndex":2,"length":25,"angle":1.5707963267948966,"radius":1.0,"direction":"Up"}
        ]"#;
        let out = evaluate_sheet_metal_chain(chain);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["error"].is_null(), "got error: {parsed}");
        assert_eq!(parsed["model"]["panel_count"], 3);
        assert_eq!(parsed["model"]["bend_count"], 2);
        assert!(!parsed["mesh"]["positions"].as_array().unwrap().is_empty());
        assert_eq!(
            parsed["flat_pattern"]["creases"].as_array().unwrap().len(),
            2
        );
    }

    #[test]
    fn error_on_empty_chain() {
        let parsed: serde_json::Value =
            serde_json::from_str(&evaluate_sheet_metal_chain("[]")).unwrap();
        assert!(parsed["error"].as_str().unwrap().contains("empty"));
    }

    #[test]
    fn error_when_first_is_edge_flange() {
        let chain = r#"[{"type":"EdgeFlange","panelId":0,"edgeIndex":0,"length":25,"angle":1.57,"radius":1,"direction":"Up"}]"#;
        let parsed: serde_json::Value =
            serde_json::from_str(&evaluate_sheet_metal_chain(chain)).unwrap();
        assert!(parsed["error"].as_str().unwrap().contains("base flange"));
    }
}
