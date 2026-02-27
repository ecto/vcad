//! V1 file format migration.
//!
//! Converts legacy `.vcad` JSON documents (v1 format: IR `Document` with parts
//! array) into the new CRDT document format. Features are reconstructed by
//! walking the existing nodes and emitting CreateFeature + SetParam ops.

use std::collections::HashMap;

use vcad_crdt::{CrdtDocument, FractionalIndex, ReplicaId, Value};
use vcad_ir::{CsgOp, Document, NodeId, Vec3};

/// Migrate a v1 document (IR DAG + parts metadata) into a CRDT document.
///
/// Each scene entry's root node chain is analyzed to determine the feature
/// kind and extract parameters. The result is a fresh CrdtDocument with
/// equivalent features.
pub fn migrate_v1(doc: &Document) -> CrdtDocument {
    let mut crdt = CrdtDocument::new(ReplicaId(0));
    let mut prev_pos: Option<FractionalIndex> = None;

    for entry in &doc.roots {
        let root_id = entry.root;
        let position = FractionalIndex::between(prev_pos.as_ref(), None);

        if let Some((kind, params)) = analyze_node_chain(doc, root_id) {
            let (fid, _) = crdt.create_feature(&kind, position.clone(), params);
            let _ = fid;
        }

        prev_pos = Some(position);
    }

    crdt
}

/// Walk a node chain from root (translate → rotate → scale → primitive/op)
/// and extract the feature kind and parameters.
fn analyze_node_chain(doc: &Document, root_id: NodeId) -> Option<(String, HashMap<String, Value>)> {
    let mut params = HashMap::new();
    let mut current_id = root_id;

    // Walk through transform chain: Translate → Rotate → Scale → core op
    loop {
        let node = doc.nodes.get(&current_id)?;
        match &node.op {
            CsgOp::Translate { child, offset } => {
                if *offset != Vec3::new(0.0, 0.0, 0.0) {
                    params.insert(
                        "offset".to_string(),
                        Value::Vec3([offset.x, offset.y, offset.z]),
                    );
                }
                if let Some(name) = &node.name {
                    params.insert("name".to_string(), Value::String(name.clone()));
                }
                current_id = *child;
            }
            CsgOp::Rotate { child, angles } => {
                if *angles != Vec3::new(0.0, 0.0, 0.0) {
                    params.insert(
                        "rotation".to_string(),
                        Value::Vec3([angles.x, angles.y, angles.z]),
                    );
                }
                current_id = *child;
            }
            CsgOp::Scale { child, factor } => {
                if *factor != Vec3::new(1.0, 1.0, 1.0) {
                    params.insert(
                        "scale".to_string(),
                        Value::Vec3([factor.x, factor.y, factor.z]),
                    );
                }
                current_id = *child;
            }
            // Leaf operations — extract kind + params
            CsgOp::Cube { size } => {
                params.insert("size_x".to_string(), Value::F64(size.x));
                params.insert("size_y".to_string(), Value::F64(size.y));
                params.insert("size_z".to_string(), Value::F64(size.z));
                return Some(("cube".to_string(), params));
            }
            CsgOp::Cylinder {
                radius,
                height,
                segments,
            } => {
                params.insert("radius".to_string(), Value::F64(*radius));
                params.insert("height".to_string(), Value::F64(*height));
                params.insert("segments".to_string(), Value::F64(*segments as f64));
                return Some(("cylinder".to_string(), params));
            }
            CsgOp::Sphere { radius, segments } => {
                params.insert("radius".to_string(), Value::F64(*radius));
                params.insert("segments".to_string(), Value::F64(*segments as f64));
                return Some(("sphere".to_string(), params));
            }
            CsgOp::Cone {
                radius_bottom,
                radius_top,
                height,
                segments,
            } => {
                params.insert("radius_bottom".to_string(), Value::F64(*radius_bottom));
                params.insert("radius_top".to_string(), Value::F64(*radius_top));
                params.insert("height".to_string(), Value::F64(*height));
                params.insert("segments".to_string(), Value::F64(*segments as f64));
                return Some(("cone".to_string(), params));
            }
            CsgOp::Union { .. } => {
                params.insert(
                    "boolean_type".to_string(),
                    Value::String("union".to_string()),
                );
                return Some(("boolean".to_string(), params));
            }
            CsgOp::Difference { .. } => {
                params.insert(
                    "boolean_type".to_string(),
                    Value::String("difference".to_string()),
                );
                return Some(("boolean".to_string(), params));
            }
            CsgOp::Intersection { .. } => {
                params.insert(
                    "boolean_type".to_string(),
                    Value::String("intersection".to_string()),
                );
                return Some(("boolean".to_string(), params));
            }
            CsgOp::Fillet { radius, .. } => {
                params.insert("radius".to_string(), Value::F64(*radius));
                return Some(("fillet".to_string(), params));
            }
            CsgOp::Chamfer { distance, .. } => {
                params.insert("distance".to_string(), Value::F64(*distance));
                return Some(("chamfer".to_string(), params));
            }
            CsgOp::Shell { thickness, .. } => {
                params.insert("thickness".to_string(), Value::F64(*thickness));
                return Some(("shell".to_string(), params));
            }
            CsgOp::Extrude { direction, .. } => {
                let depth =
                    (direction.x * direction.x + direction.y * direction.y + direction.z * direction.z).sqrt();
                if depth > 0.0 {
                    params.insert("depth".to_string(), Value::F64(depth));
                    params.insert(
                        "direction".to_string(),
                        Value::Vec3([
                            direction.x / depth,
                            direction.y / depth,
                            direction.z / depth,
                        ]),
                    );
                }
                return Some(("extrude".to_string(), params));
            }
            // For other ops, return a generic representation
            _ => {
                return None;
            }
        }
    }
}

/// Detect whether bytes represent a v1 (JSON) format or v2 (CRDT) format.
pub fn detect_format(bytes: &[u8]) -> FileFormat {
    // V1 format starts with '{' (JSON object).
    // V2 format starts with '{"replica_id"' (CRDT SavedDocument).
    if bytes.is_empty() {
        return FileFormat::Unknown;
    }

    // Try parsing as CRDT first (it's also JSON but has specific fields).
    if let Ok(val) = serde_json::from_slice::<serde_json::Value>(bytes) {
        if val.get("replica_id").is_some() && val.get("ops").is_some() {
            return FileFormat::V2Crdt;
        }
        if val.get("version").is_some() && val.get("nodes").is_some() {
            return FileFormat::V1Json;
        }
    }

    FileFormat::Unknown
}

/// File format version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    /// Legacy JSON IR format.
    V1Json,
    /// New CRDT op-log format.
    V2Crdt,
    /// Unknown format.
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::{Document, Node, SceneEntry};

    #[test]
    fn test_migrate_empty_document() {
        let doc = Document::new();
        let crdt = migrate_v1(&doc);
        assert_eq!(crdt.ordered_features().len(), 0);
    }

    #[test]
    fn test_migrate_single_cube() {
        let mut doc = Document::new();
        let cube_id = 1;
        doc.nodes.insert(
            cube_id,
            Node {
                id: cube_id,
                name: Some("Cube 1".to_string()),
                op: CsgOp::Cube {
                    size: Vec3::new(10.0, 20.0, 30.0),
                },
            },
        );
        // Add transform chain: scale → rotate → translate
        let scale_id = 2;
        doc.nodes.insert(
            scale_id,
            Node {
                id: scale_id,
                name: None,
                op: CsgOp::Scale {
                    child: cube_id,
                    factor: Vec3::new(1.0, 1.0, 1.0),
                },
            },
        );
        let rotate_id = 3;
        doc.nodes.insert(
            rotate_id,
            Node {
                id: rotate_id,
                name: None,
                op: CsgOp::Rotate {
                    child: scale_id,
                    angles: Vec3::new(0.0, 0.0, 0.0),
                },
            },
        );
        let translate_id = 4;
        doc.nodes.insert(
            translate_id,
            Node {
                id: translate_id,
                name: Some("Cube 1".to_string()),
                op: CsgOp::Translate {
                    child: rotate_id,
                    offset: Vec3::new(5.0, 0.0, 0.0),
                },
            },
        );
        doc.roots.push(SceneEntry {
            root: translate_id,
            material: "default".to_string(),
            visible: None,
        });

        let crdt = migrate_v1(&doc);
        let features = crdt.ordered_features();
        assert_eq!(features.len(), 1);

        let f = features[0].1;
        assert_eq!(f.kind, "cube");
        assert_eq!(f.params.get("size_x").unwrap().0, Value::F64(10.0));
        assert_eq!(f.params.get("size_y").unwrap().0, Value::F64(20.0));
        assert_eq!(f.params.get("size_z").unwrap().0, Value::F64(30.0));
        assert_eq!(
            f.params.get("offset").unwrap().0,
            Value::Vec3([5.0, 0.0, 0.0])
        );
    }

    #[test]
    fn test_migrate_bare_cube() {
        // A cube directly as a root (no transform chain)
        let mut doc = Document::new();
        let cube_id = 1;
        doc.nodes.insert(
            cube_id,
            Node {
                id: cube_id,
                name: Some("Box".to_string()),
                op: CsgOp::Cube {
                    size: Vec3::new(5.0, 5.0, 5.0),
                },
            },
        );
        doc.roots.push(SceneEntry {
            root: cube_id,
            material: "default".to_string(),
            visible: None,
        });

        let crdt = migrate_v1(&doc);
        assert_eq!(crdt.ordered_features().len(), 1);
        assert_eq!(crdt.ordered_features()[0].1.kind, "cube");
    }

    #[test]
    fn test_detect_format() {
        let v1 = br#"{"version":"0.1","nodes":{},"materials":{},"part_materials":{},"roots":[]}"#;
        assert_eq!(detect_format(v1), FileFormat::V1Json);

        let v2 = br#"{"replica_id":1,"ops":[],"features":[]}"#;
        assert_eq!(detect_format(v2), FileFormat::V2Crdt);

        assert_eq!(detect_format(b""), FileFormat::Unknown);
        assert_eq!(detect_format(b"garbage"), FileFormat::Unknown);
    }
}
