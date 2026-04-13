//! File I/O utilities for vcad documents.
//!
//! Handles format detection, parsing, and part derivation for `.vcad` files.
//! Supports three formats:
//! - v0.1 JSON — direct JSON `Document`
//! - v0.2 VCode — token-efficient text format
//! - v0.3 loon — loon language source code

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{CsgOp, Document, NodeId};

/// Type alias for an optional loon evaluator callback.
type LoonEvaluator<'a> = Option<&'a dyn Fn(&str) -> Result<Document, String>>;

/// Detected file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VcadFormat {
    /// JSON format (v0.1).
    Json,
    /// VCode format (v0.2).
    VCode,
    /// Loon source format (v0.3).
    Loon,
}

/// A parsed vcad file with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcadFile {
    /// Format version string.
    pub version: String,
    /// The parsed document.
    pub document: Document,
    /// Derived part information.
    pub parts: Vec<PartInfo>,
    /// Parts consumed by boolean operations (optional metadata).
    #[serde(default)]
    pub consumed_parts: HashMap<String, PartInfo>,
    /// Next available node ID.
    pub next_node_id: u64,
    /// Next available part number.
    pub next_part_num: u64,
    /// Original loon source (if format was loon).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loon_source: Option<String>,
}

/// Part information derived from walking the document DAG.
///
/// Tagged union matching the TypeScript `PartInfo` type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum PartInfo {
    /// A primitive shape (cube, cylinder, sphere).
    #[serde(rename = "cube")]
    Cube {
        id: String,
        name: String,
        #[serde(rename = "primitiveNodeId")]
        primitive_node_id: NodeId,
        #[serde(rename = "scaleNodeId")]
        scale_node_id: NodeId,
        #[serde(rename = "rotateNodeId")]
        rotate_node_id: NodeId,
        #[serde(rename = "translateNodeId")]
        translate_node_id: NodeId,
    },
    #[serde(rename = "cylinder")]
    Cylinder {
        id: String,
        name: String,
        #[serde(rename = "primitiveNodeId")]
        primitive_node_id: NodeId,
        #[serde(rename = "scaleNodeId")]
        scale_node_id: NodeId,
        #[serde(rename = "rotateNodeId")]
        rotate_node_id: NodeId,
        #[serde(rename = "translateNodeId")]
        translate_node_id: NodeId,
    },
    #[serde(rename = "sphere")]
    Sphere {
        id: String,
        name: String,
        #[serde(rename = "primitiveNodeId")]
        primitive_node_id: NodeId,
        #[serde(rename = "scaleNodeId")]
        scale_node_id: NodeId,
        #[serde(rename = "rotateNodeId")]
        rotate_node_id: NodeId,
        #[serde(rename = "translateNodeId")]
        translate_node_id: NodeId,
    },
    /// A boolean operation.
    #[serde(rename = "boolean")]
    Boolean {
        id: String,
        name: String,
        #[serde(rename = "booleanType")]
        boolean_type: String,
        #[serde(rename = "booleanNodeId")]
        boolean_node_id: NodeId,
        #[serde(rename = "scaleNodeId")]
        scale_node_id: NodeId,
        #[serde(rename = "rotateNodeId")]
        rotate_node_id: NodeId,
        #[serde(rename = "translateNodeId")]
        translate_node_id: NodeId,
        #[serde(rename = "sourcePartIds")]
        source_part_ids: Vec<String>,
    },
    /// An extrusion.
    #[serde(rename = "extrude")]
    Extrude {
        id: String,
        name: String,
        #[serde(rename = "sketchNodeId")]
        sketch_node_id: NodeId,
        #[serde(rename = "extrudeNodeId")]
        extrude_node_id: NodeId,
        #[serde(rename = "scaleNodeId")]
        scale_node_id: NodeId,
        #[serde(rename = "rotateNodeId")]
        rotate_node_id: NodeId,
        #[serde(rename = "translateNodeId")]
        translate_node_id: NodeId,
    },
    /// A revolution.
    #[serde(rename = "revolve")]
    Revolve {
        id: String,
        name: String,
        #[serde(rename = "sketchNodeId")]
        sketch_node_id: NodeId,
        #[serde(rename = "revolveNodeId")]
        revolve_node_id: NodeId,
        #[serde(rename = "scaleNodeId")]
        scale_node_id: NodeId,
        #[serde(rename = "rotateNodeId")]
        rotate_node_id: NodeId,
        #[serde(rename = "translateNodeId")]
        translate_node_id: NodeId,
    },
    /// A sweep.
    #[serde(rename = "sweep")]
    Sweep {
        id: String,
        name: String,
        #[serde(rename = "sketchNodeId")]
        sketch_node_id: NodeId,
        #[serde(rename = "sweepNodeId")]
        sweep_node_id: NodeId,
        #[serde(rename = "scaleNodeId")]
        scale_node_id: NodeId,
        #[serde(rename = "rotateNodeId")]
        rotate_node_id: NodeId,
        #[serde(rename = "translateNodeId")]
        translate_node_id: NodeId,
    },
    /// A loft.
    #[serde(rename = "loft")]
    Loft {
        id: String,
        name: String,
        #[serde(rename = "sketchNodeIds")]
        sketch_node_ids: Vec<NodeId>,
        #[serde(rename = "loftNodeId")]
        loft_node_id: NodeId,
        #[serde(rename = "scaleNodeId")]
        scale_node_id: NodeId,
        #[serde(rename = "rotateNodeId")]
        rotate_node_id: NodeId,
        #[serde(rename = "translateNodeId")]
        translate_node_id: NodeId,
    },
    /// An imported mesh.
    #[serde(rename = "imported-mesh")]
    ImportedMesh {
        id: String,
        name: String,
        #[serde(rename = "meshNodeId")]
        mesh_node_id: NodeId,
        #[serde(rename = "scaleNodeId")]
        scale_node_id: NodeId,
        #[serde(rename = "rotateNodeId")]
        rotate_node_id: NodeId,
        #[serde(rename = "translateNodeId")]
        translate_node_id: NodeId,
    },
    /// A fillet.
    #[serde(rename = "fillet")]
    Fillet {
        id: String,
        name: String,
        #[serde(rename = "sourcePartId")]
        source_part_id: String,
        #[serde(rename = "filletNodeId")]
        fillet_node_id: NodeId,
        #[serde(rename = "scaleNodeId")]
        scale_node_id: NodeId,
        #[serde(rename = "rotateNodeId")]
        rotate_node_id: NodeId,
        #[serde(rename = "translateNodeId")]
        translate_node_id: NodeId,
    },
    /// A chamfer.
    #[serde(rename = "chamfer")]
    Chamfer {
        id: String,
        name: String,
        #[serde(rename = "sourcePartId")]
        source_part_id: String,
        #[serde(rename = "chamferNodeId")]
        chamfer_node_id: NodeId,
        #[serde(rename = "scaleNodeId")]
        scale_node_id: NodeId,
        #[serde(rename = "rotateNodeId")]
        rotate_node_id: NodeId,
        #[serde(rename = "translateNodeId")]
        translate_node_id: NodeId,
    },
    /// A shell operation.
    #[serde(rename = "shell")]
    Shell {
        id: String,
        name: String,
        #[serde(rename = "sourcePartId")]
        source_part_id: String,
        #[serde(rename = "shellNodeId")]
        shell_node_id: NodeId,
        #[serde(rename = "scaleNodeId")]
        scale_node_id: NodeId,
        #[serde(rename = "rotateNodeId")]
        rotate_node_id: NodeId,
        #[serde(rename = "translateNodeId")]
        translate_node_id: NodeId,
    },
    /// A linear pattern.
    #[serde(rename = "linear-pattern")]
    LinearPattern {
        id: String,
        name: String,
        #[serde(rename = "sourcePartId")]
        source_part_id: String,
        #[serde(rename = "patternNodeId")]
        pattern_node_id: NodeId,
        #[serde(rename = "scaleNodeId")]
        scale_node_id: NodeId,
        #[serde(rename = "rotateNodeId")]
        rotate_node_id: NodeId,
        #[serde(rename = "translateNodeId")]
        translate_node_id: NodeId,
    },
    /// A circular pattern.
    #[serde(rename = "circular-pattern")]
    CircularPattern {
        id: String,
        name: String,
        #[serde(rename = "sourcePartId")]
        source_part_id: String,
        #[serde(rename = "patternNodeId")]
        pattern_node_id: NodeId,
        #[serde(rename = "scaleNodeId")]
        scale_node_id: NodeId,
        #[serde(rename = "rotateNodeId")]
        rotate_node_id: NodeId,
        #[serde(rename = "translateNodeId")]
        translate_node_id: NodeId,
    },
    /// A PCB board.
    #[serde(rename = "pcb-board")]
    PcbBoard {
        id: String,
        name: String,
        #[serde(rename = "boardNodeId")]
        board_node_id: NodeId,
        #[serde(rename = "scaleNodeId")]
        scale_node_id: NodeId,
        #[serde(rename = "rotateNodeId")]
        rotate_node_id: NodeId,
        #[serde(rename = "translateNodeId")]
        translate_node_id: NodeId,
    },
    /// An embroidery pattern.
    #[serde(rename = "embroidery-pattern")]
    EmbroideryPattern {
        id: String,
        name: String,
        #[serde(rename = "patternNodeId")]
        pattern_node_id: NodeId,
        #[serde(rename = "scaleNodeId")]
        scale_node_id: NodeId,
        #[serde(rename = "rotateNodeId")]
        rotate_node_id: NodeId,
        #[serde(rename = "translateNodeId")]
        translate_node_id: NodeId,
    },
}

/// Detect the format of a vcad file from its content.
pub fn detect_format(content: &str) -> VcadFormat {
    let trimmed = content.trim();
    if trimmed.starts_with('{') {
        VcadFormat::Json
    } else if trimmed.starts_with('[') || trimmed.starts_with(';') {
        VcadFormat::Loon
    } else {
        VcadFormat::VCode
    }
}

/// Parse a vcad file (JSON or VCode format).
///
/// For loon format, use `parse_vcad_file_with_loon` which accepts a loon
/// evaluator function.
pub fn parse_vcad_file(content: &str) -> Result<VcadFile, String> {
    parse_vcad_file_with_loon(content, None)
}

/// Parse a vcad file with an optional loon evaluator.
///
/// The `eval_loon` callback, if provided, evaluates loon source and returns
/// a JSON-serialized `Document`.
pub fn parse_vcad_file_with_loon(
    content: &str,
    eval_loon: LoonEvaluator<'_>,
) -> Result<VcadFile, String> {
    let trimmed = content.trim();
    match detect_format(trimmed) {
        VcadFormat::Json => parse_json_vcad(trimmed),
        VcadFormat::VCode => parse_vcode_vcad(trimmed),
        VcadFormat::Loon => parse_loon_vcad(trimmed, eval_loon),
    }
}

/// Parse JSON format (v0.1).
fn parse_json_vcad(json: &str) -> Result<VcadFile, String> {
    // Try parsing as a full VcadFile first (legacy format with parts, etc.)
    #[derive(Deserialize)]
    #[allow(dead_code)]
    struct LegacyVcadFile {
        document: Document,
        parts: Vec<serde_json::Value>,
        #[serde(default, rename = "consumedParts")]
        consumed_parts: HashMap<String, serde_json::Value>,
        #[serde(rename = "nextNodeId")]
        next_node_id: u64,
        #[serde(rename = "nextPartNum")]
        next_part_num: u64,
        #[serde(rename = "loonSource")]
        loon_source: Option<String>,
    }

    if let Ok(legacy) = serde_json::from_str::<LegacyVcadFile>(json) {
        // Re-derive parts from the document for consistency
        let parts = derive_parts(&legacy.document);
        let (next_node_id, next_part_num) = compute_next_ids(&legacy.document, &parts);
        return Ok(VcadFile {
            version: "0.1".to_string(),
            document: legacy.document,
            parts,
            consumed_parts: HashMap::new(),
            next_node_id,
            next_part_num,
            loon_source: legacy.loon_source,
        });
    }

    // Try parsing as just a Document
    let document: Document =
        serde_json::from_str(json).map_err(|e| format!("Invalid JSON vcad file: {}", e))?;
    let parts = derive_parts(&document);
    let (next_node_id, next_part_num) = compute_next_ids(&document, &parts);

    Ok(VcadFile {
        version: "0.1".to_string(),
        document,
        parts,
        consumed_parts: HashMap::new(),
        next_node_id,
        next_part_num,
        loon_source: None,
    })
}

/// Parse VCode format (v0.2).
fn parse_vcode_vcad(compact: &str) -> Result<VcadFile, String> {
    let document =
        crate::vcode::from_vcode(compact).map_err(|e| format!("VCode parse error: {}", e))?;
    let parts = derive_parts(&document);
    let (next_node_id, next_part_num) = compute_next_ids(&document, &parts);

    Ok(VcadFile {
        version: "0.2".to_string(),
        document,
        parts,
        consumed_parts: HashMap::new(),
        next_node_id,
        next_part_num,
        loon_source: None,
    })
}

/// Parse loon format (v0.3).
fn parse_loon_vcad(source: &str, eval_loon: LoonEvaluator<'_>) -> Result<VcadFile, String> {
    let eval = eval_loon.ok_or_else(|| {
        "Loon format detected but no evaluator provided. Engine may not be ready.".to_string()
    })?;
    let document = eval(source)?;
    let parts = derive_parts(&document);
    let (next_node_id, next_part_num) = compute_next_ids(&document, &parts);

    Ok(VcadFile {
        version: "0.3".to_string(),
        document,
        parts,
        consumed_parts: HashMap::new(),
        next_node_id,
        next_part_num,
        loon_source: Some(source.to_string()),
    })
}

// ============================================================================
// Part derivation
// ============================================================================

/// Derive [`PartInfo`] from a [`Document`] by analyzing the node graph.
///
/// For each scene root, walks backward through the transform chain
/// (Translate → Rotate → Scale → core operation) to identify the part type.
pub fn derive_parts(document: &Document) -> Vec<PartInfo> {
    let mut parts = Vec::new();
    let mut part_num: u64 = 1;

    for root in &document.roots {
        if let Some(part) = derive_part_from_root(document, root.root, part_num) {
            parts.push(part);
            part_num += 1;
        }
    }

    parts
}

struct TransformChain {
    translate_node_id: NodeId,
    rotate_node_id: NodeId,
    scale_node_id: NodeId,
    core_node_id: NodeId,
    core_op: CsgOp,
}

/// Walk backward from a root node through the transform chain.
///
/// Expected pattern: root(Translate) → Rotate → Scale → core operation.
/// If transforms are missing, we use the root node ID as default.
fn walk_transform_chain(document: &Document, root_node_id: NodeId) -> Option<TransformChain> {
    let root_node = document.nodes.get(&root_node_id)?;

    let translate_node_id = root_node_id;
    let mut rotate_node_id = root_node_id;
    let mut scale_node_id = root_node_id;

    let mut current_node = root_node;
    let mut core_node_id = root_node_id;
    let mut core_op = root_node.op.clone();

    let mut saw_rotate = false;
    let mut saw_scale = false;

    loop {
        let child_id = match &current_node.op {
            CsgOp::Translate { child, .. } => Some(*child),
            CsgOp::Rotate { child, .. } => {
                if !saw_rotate {
                    rotate_node_id = current_node.id;
                    saw_rotate = true;
                }
                Some(*child)
            }
            CsgOp::Scale { child, .. } => {
                if !saw_scale {
                    scale_node_id = current_node.id;
                    saw_scale = true;
                }
                Some(*child)
            }
            _ => break,
        };

        let Some(child_id) = child_id else { break };
        let Some(child_node) = document.nodes.get(&child_id) else {
            // Dangling transform; treat this as the core
            core_node_id = current_node.id;
            core_op = current_node.op.clone();
            return Some(TransformChain {
                translate_node_id,
                rotate_node_id,
                scale_node_id,
                core_node_id,
                core_op,
            });
        };

        current_node = child_node;
        core_node_id = current_node.id;
        core_op = current_node.op.clone();
    }

    Some(TransformChain {
        translate_node_id,
        rotate_node_id,
        scale_node_id,
        core_node_id,
        core_op,
    })
}

fn derive_part_from_root(
    document: &Document,
    root_node_id: NodeId,
    part_num: u64,
) -> Option<PartInfo> {
    let chain = walk_transform_chain(document, root_node_id)?;

    let translate_node = document.nodes.get(&chain.translate_node_id);
    let name = translate_node
        .and_then(|n| n.name.as_ref())
        .cloned()
        .unwrap_or_else(|| format!("Part {}", part_num));
    let id = format!("part-{}", part_num);

    match &chain.core_op {
        CsgOp::Cube { .. } => Some(PartInfo::Cube {
            id,
            name,
            primitive_node_id: chain.core_node_id,
            scale_node_id: chain.scale_node_id,
            rotate_node_id: chain.rotate_node_id,
            translate_node_id: chain.translate_node_id,
        }),

        CsgOp::Cylinder { .. } => Some(PartInfo::Cylinder {
            id,
            name,
            primitive_node_id: chain.core_node_id,
            scale_node_id: chain.scale_node_id,
            rotate_node_id: chain.rotate_node_id,
            translate_node_id: chain.translate_node_id,
        }),

        CsgOp::Sphere { .. } => Some(PartInfo::Sphere {
            id,
            name,
            primitive_node_id: chain.core_node_id,
            scale_node_id: chain.scale_node_id,
            rotate_node_id: chain.rotate_node_id,
            translate_node_id: chain.translate_node_id,
        }),

        CsgOp::Union { .. } => Some(PartInfo::Boolean {
            id,
            name,
            boolean_type: "union".to_string(),
            boolean_node_id: chain.core_node_id,
            scale_node_id: chain.scale_node_id,
            rotate_node_id: chain.rotate_node_id,
            translate_node_id: chain.translate_node_id,
            source_part_ids: vec!["unknown".to_string(), "unknown".to_string()],
        }),

        CsgOp::Difference { .. } => Some(PartInfo::Boolean {
            id,
            name,
            boolean_type: "difference".to_string(),
            boolean_node_id: chain.core_node_id,
            scale_node_id: chain.scale_node_id,
            rotate_node_id: chain.rotate_node_id,
            translate_node_id: chain.translate_node_id,
            source_part_ids: vec!["unknown".to_string(), "unknown".to_string()],
        }),

        CsgOp::Intersection { .. } => Some(PartInfo::Boolean {
            id,
            name,
            boolean_type: "intersection".to_string(),
            boolean_node_id: chain.core_node_id,
            scale_node_id: chain.scale_node_id,
            rotate_node_id: chain.rotate_node_id,
            translate_node_id: chain.translate_node_id,
            source_part_ids: vec!["unknown".to_string(), "unknown".to_string()],
        }),

        CsgOp::Extrude { sketch, .. } => Some(PartInfo::Extrude {
            id,
            name,
            sketch_node_id: *sketch,
            extrude_node_id: chain.core_node_id,
            scale_node_id: chain.scale_node_id,
            rotate_node_id: chain.rotate_node_id,
            translate_node_id: chain.translate_node_id,
        }),

        CsgOp::Revolve { sketch, .. } => Some(PartInfo::Revolve {
            id,
            name,
            sketch_node_id: *sketch,
            revolve_node_id: chain.core_node_id,
            scale_node_id: chain.scale_node_id,
            rotate_node_id: chain.rotate_node_id,
            translate_node_id: chain.translate_node_id,
        }),

        CsgOp::Sweep { sketch, .. } => Some(PartInfo::Sweep {
            id,
            name,
            sketch_node_id: *sketch,
            sweep_node_id: chain.core_node_id,
            scale_node_id: chain.scale_node_id,
            rotate_node_id: chain.rotate_node_id,
            translate_node_id: chain.translate_node_id,
        }),

        CsgOp::Loft { sketches, .. } => Some(PartInfo::Loft {
            id,
            name,
            sketch_node_ids: sketches.clone(),
            loft_node_id: chain.core_node_id,
            scale_node_id: chain.scale_node_id,
            rotate_node_id: chain.rotate_node_id,
            translate_node_id: chain.translate_node_id,
        }),

        CsgOp::ImportedMesh { .. } => Some(PartInfo::ImportedMesh {
            id,
            name,
            mesh_node_id: chain.core_node_id,
            scale_node_id: chain.scale_node_id,
            rotate_node_id: chain.rotate_node_id,
            translate_node_id: chain.translate_node_id,
        }),

        CsgOp::Fillet { .. } => Some(PartInfo::Fillet {
            id,
            name,
            source_part_id: "unknown".to_string(),
            fillet_node_id: chain.core_node_id,
            scale_node_id: chain.scale_node_id,
            rotate_node_id: chain.rotate_node_id,
            translate_node_id: chain.translate_node_id,
        }),

        CsgOp::Chamfer { .. } => Some(PartInfo::Chamfer {
            id,
            name,
            source_part_id: "unknown".to_string(),
            chamfer_node_id: chain.core_node_id,
            scale_node_id: chain.scale_node_id,
            rotate_node_id: chain.rotate_node_id,
            translate_node_id: chain.translate_node_id,
        }),

        CsgOp::Shell { .. } => Some(PartInfo::Shell {
            id,
            name,
            source_part_id: "unknown".to_string(),
            shell_node_id: chain.core_node_id,
            scale_node_id: chain.scale_node_id,
            rotate_node_id: chain.rotate_node_id,
            translate_node_id: chain.translate_node_id,
        }),

        CsgOp::LinearPattern { .. } => Some(PartInfo::LinearPattern {
            id,
            name,
            source_part_id: "unknown".to_string(),
            pattern_node_id: chain.core_node_id,
            scale_node_id: chain.scale_node_id,
            rotate_node_id: chain.rotate_node_id,
            translate_node_id: chain.translate_node_id,
        }),

        CsgOp::CircularPattern { .. } => Some(PartInfo::CircularPattern {
            id,
            name,
            source_part_id: "unknown".to_string(),
            pattern_node_id: chain.core_node_id,
            scale_node_id: chain.scale_node_id,
            rotate_node_id: chain.rotate_node_id,
            translate_node_id: chain.translate_node_id,
        }),

        CsgOp::PcbBoard { .. } => Some(PartInfo::PcbBoard {
            id,
            name,
            board_node_id: chain.core_node_id,
            scale_node_id: chain.scale_node_id,
            rotate_node_id: chain.rotate_node_id,
            translate_node_id: chain.translate_node_id,
        }),

        CsgOp::EmbroideryPattern { .. } => Some(PartInfo::EmbroideryPattern {
            id,
            name,
            pattern_node_id: chain.core_node_id,
            scale_node_id: chain.scale_node_id,
            rotate_node_id: chain.rotate_node_id,
            translate_node_id: chain.translate_node_id,
        }),

        _ => None,
    }
}

/// Compute the next available node ID and part number.
fn compute_next_ids(document: &Document, parts: &[PartInfo]) -> (u64, u64) {
    let max_node_id = document.nodes.keys().copied().max().unwrap_or(0);

    let max_part_num = parts
        .iter()
        .filter_map(|p| {
            let id = match p {
                PartInfo::Cube { id, .. }
                | PartInfo::Cylinder { id, .. }
                | PartInfo::Sphere { id, .. }
                | PartInfo::Boolean { id, .. }
                | PartInfo::Extrude { id, .. }
                | PartInfo::Revolve { id, .. }
                | PartInfo::Sweep { id, .. }
                | PartInfo::Loft { id, .. }
                | PartInfo::ImportedMesh { id, .. }
                | PartInfo::Fillet { id, .. }
                | PartInfo::Chamfer { id, .. }
                | PartInfo::Shell { id, .. }
                | PartInfo::LinearPattern { id, .. }
                | PartInfo::CircularPattern { id, .. }
                | PartInfo::PcbBoard { id, .. }
                | PartInfo::EmbroideryPattern { id, .. } => id,
            };
            id.strip_prefix("part-").and_then(|n| n.parse::<u64>().ok())
        })
        .max()
        .unwrap_or(0);

    (max_node_id + 1, max_part_num + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Node, SceneEntry, Vec3};

    fn make_doc(nodes: Vec<Node>, root_id: NodeId) -> Document {
        let mut doc = Document::new();
        for node in nodes {
            doc.nodes.insert(node.id, node);
        }
        doc.roots.push(SceneEntry {
            root: root_id,
            material: "default".to_string(),
            visible: None,
        });
        doc
    }

    #[test]
    fn detect_json_format() {
        assert_eq!(detect_format("{\"version\": \"0.1\"}"), VcadFormat::Json);
    }

    #[test]
    fn detect_vcode_format() {
        assert_eq!(detect_format("# vcad 0.2\nC 10 20 30"), VcadFormat::VCode);
    }

    #[test]
    fn detect_loon_format() {
        assert_eq!(detect_format("[cube 10.0 20.0 30.0]"), VcadFormat::Loon);
        assert_eq!(
            detect_format("; comment\n[cube 1.0 1.0 1.0]"),
            VcadFormat::Loon
        );
    }

    #[test]
    fn derive_cube_part() {
        let doc = make_doc(
            vec![Node {
                id: 1,
                name: Some("Box".to_string()),
                op: CsgOp::Cube {
                    size: Vec3::new(10.0, 20.0, 30.0),
                },
            }],
            1,
        );
        let parts = derive_parts(&doc);
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            PartInfo::Cube {
                id,
                name,
                primitive_node_id,
                ..
            } => {
                assert_eq!(id, "part-1");
                assert_eq!(name, "Box");
                assert_eq!(*primitive_node_id, 1);
            }
            _ => panic!("expected Cube part"),
        }
    }

    #[test]
    fn derive_transform_chain() {
        let doc = make_doc(
            vec![
                Node {
                    id: 1,
                    name: Some("Root".to_string()),
                    op: CsgOp::Translate {
                        child: 2,
                        offset: Vec3::new(1.0, 2.0, 3.0),
                    },
                },
                Node {
                    id: 2,
                    name: Some("Rot".to_string()),
                    op: CsgOp::Rotate {
                        child: 3,
                        angles: Vec3::new(0.0, 0.0, 0.0),
                    },
                },
                Node {
                    id: 3,
                    name: Some("Scl".to_string()),
                    op: CsgOp::Scale {
                        child: 4,
                        factor: Vec3::new(1.0, 1.0, 1.0),
                    },
                },
                Node {
                    id: 4,
                    name: Some("Cube".to_string()),
                    op: CsgOp::Cube {
                        size: Vec3::new(4.0, 5.0, 6.0),
                    },
                },
            ],
            1,
        );
        let parts = derive_parts(&doc);
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            PartInfo::Cube {
                primitive_node_id,
                translate_node_id,
                rotate_node_id,
                scale_node_id,
                ..
            } => {
                assert_eq!(*primitive_node_id, 4);
                assert_eq!(*translate_node_id, 1);
                assert_eq!(*rotate_node_id, 2);
                assert_eq!(*scale_node_id, 3);
            }
            _ => panic!("expected Cube part"),
        }
    }

    #[test]
    fn derive_rotate_then_cube() {
        let doc = make_doc(
            vec![
                Node {
                    id: 1,
                    name: Some("Root".to_string()),
                    op: CsgOp::Rotate {
                        child: 2,
                        angles: Vec3::new(0.0, 0.0, 0.0),
                    },
                },
                Node {
                    id: 2,
                    name: Some("Cube".to_string()),
                    op: CsgOp::Cube {
                        size: Vec3::new(1.0, 2.0, 3.0),
                    },
                },
            ],
            1,
        );
        let parts = derive_parts(&doc);
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            PartInfo::Cube {
                primitive_node_id,
                translate_node_id,
                rotate_node_id,
                ..
            } => {
                assert_eq!(*primitive_node_id, 2);
                assert_eq!(*translate_node_id, 1);
                assert_eq!(*rotate_node_id, 1);
            }
            _ => panic!("expected Cube part"),
        }
    }

    #[test]
    fn derive_boolean_part() {
        let doc = make_doc(
            vec![
                Node {
                    id: 1,
                    name: None,
                    op: CsgOp::Cube {
                        size: Vec3::new(10.0, 10.0, 10.0),
                    },
                },
                Node {
                    id: 2,
                    name: None,
                    op: CsgOp::Sphere {
                        radius: 5.0,
                        segments: 0,
                    },
                },
                Node {
                    id: 3,
                    name: Some("merged".to_string()),
                    op: CsgOp::Union { left: 1, right: 2 },
                },
            ],
            3,
        );
        let parts = derive_parts(&doc);
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            PartInfo::Boolean {
                boolean_type,
                boolean_node_id,
                ..
            } => {
                assert_eq!(boolean_type, "union");
                assert_eq!(*boolean_node_id, 3);
            }
            _ => panic!("expected Boolean part"),
        }
    }

    #[test]
    fn compute_next_ids_basic() {
        let doc = make_doc(
            vec![Node {
                id: 5,
                name: None,
                op: CsgOp::Cube {
                    size: Vec3::new(1.0, 1.0, 1.0),
                },
            }],
            5,
        );
        let parts = derive_parts(&doc);
        let (next_node, next_part) = compute_next_ids(&doc, &parts);
        assert_eq!(next_node, 6);
        assert_eq!(next_part, 2);
    }
}
