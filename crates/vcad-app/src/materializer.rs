//! Materializer — maps CRDT feature graph to kernel IR.
//!
//! One declarative function replaces the 85+ imperative mutations in TypeScript.
//! Each feature kind is a match arm that reads CRDT params and emits IR nodes.

use std::collections::HashMap;

use vcad_crdt::{CrdtDocument, FeatureId, FeatureState, Value};
use vcad_ir::{CsgOp, Document, Node, NodeId, SceneEntry, Vec3};

use crate::part_info::PartInfo;

/// Result of materialization: an IR document plus part metadata.
pub struct MaterializeResult {
    /// The IR document ready for evaluation.
    pub document: Document,
    /// Part info for each materialized feature.
    pub parts: Vec<PartInfo>,
}

/// Materialization context — tracks node ID allocation and feature-to-node mapping.
struct Context {
    next_node_id: NodeId,
    /// Maps feature IDs to their root (translate) node ID.
    feature_roots: HashMap<String, NodeId>,
}

impl Context {
    fn new() -> Self {
        Self {
            next_node_id: 1,
            feature_roots: HashMap::new(),
        }
    }

    fn alloc(&mut self) -> NodeId {
        let id = self.next_node_id;
        self.next_node_id += 1;
        id
    }
}

/// Materialize the full CRDT document into an IR document and parts list.
pub fn materialize(crdt: &CrdtDocument) -> MaterializeResult {
    let mut doc = Document::new();
    let mut parts = Vec::new();
    let mut ctx = Context::new();

    for (fid, feature) in crdt.ordered_features() {
        if let Some((part, root_id)) = materialize_feature(&mut doc, &mut ctx, fid, feature, crdt)
        {
            doc.roots.push(SceneEntry {
                root: root_id,
                material: "default".to_string(),
                visible: None,
            });
            ctx.feature_roots.insert(fid_to_string(fid), root_id);
            parts.push(part);
        }
    }

    MaterializeResult { document: doc, parts }
}

/// Materialize a single feature into IR nodes.
///
/// Returns the PartInfo and the root (outermost translate) node ID, or None
/// if the feature kind is unknown.
fn materialize_feature(
    doc: &mut Document,
    ctx: &mut Context,
    fid: FeatureId,
    feature: &FeatureState,
    crdt: &CrdtDocument,
) -> Option<(PartInfo, NodeId)> {
    let id_str = fid_to_string(fid);
    let name = get_str(feature, "name").unwrap_or_else(|| feature.kind.clone());

    match feature.kind.as_str() {
        "cube" => {
            let sx = get_f64(feature, "size_x").unwrap_or(10.0);
            let sy = get_f64(feature, "size_y").unwrap_or(10.0);
            let sz = get_f64(feature, "size_z").unwrap_or(10.0);

            let prim_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(doc, prim_id, &name, CsgOp::Cube { size: Vec3::new(sx, sy, sz) });
            insert_transform_chain(doc, ctx, feature, prim_id, scale_id, rotate_id, translate_id);

            Some((
                PartInfo::Cube {
                    id: id_str,
                    name,
                    primitive_node_id: prim_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        "cylinder" => {
            let radius = get_f64(feature, "radius").unwrap_or(5.0);
            let height = get_f64(feature, "height").unwrap_or(10.0);
            let segments = get_f64(feature, "segments").map(|v| v as u32).unwrap_or(32);

            let prim_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(doc, prim_id, &name, CsgOp::Cylinder { radius, height, segments });
            insert_transform_chain(doc, ctx, feature, prim_id, scale_id, rotate_id, translate_id);

            Some((
                PartInfo::Cylinder {
                    id: id_str,
                    name,
                    primitive_node_id: prim_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        "sphere" => {
            let radius = get_f64(feature, "radius").unwrap_or(5.0);
            let segments = get_f64(feature, "segments").map(|v| v as u32).unwrap_or(32);

            let prim_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(doc, prim_id, &name, CsgOp::Sphere { radius, segments });
            insert_transform_chain(doc, ctx, feature, prim_id, scale_id, rotate_id, translate_id);

            Some((
                PartInfo::Sphere {
                    id: id_str,
                    name,
                    primitive_node_id: prim_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        "cone" => {
            let radius_bottom = get_f64(feature, "radius_bottom").unwrap_or(5.0);
            let radius_top = get_f64(feature, "radius_top").unwrap_or(0.0);
            let height = get_f64(feature, "height").unwrap_or(10.0);
            let segments = get_f64(feature, "segments").map(|v| v as u32).unwrap_or(32);

            let prim_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(
                doc,
                prim_id,
                &name,
                CsgOp::Cone { radius_bottom, radius_top, height, segments },
            );
            insert_transform_chain(doc, ctx, feature, prim_id, scale_id, rotate_id, translate_id);

            Some((
                PartInfo::Cone {
                    id: id_str,
                    name,
                    primitive_node_id: prim_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        "boolean" => {
            let bool_type = get_str(feature, "boolean_type").unwrap_or_else(|| "union".to_string());
            let input_a = get_str(feature, "input_a").unwrap_or_default();
            let input_b = get_str(feature, "input_b").unwrap_or_default();

            let left = ctx.feature_roots.get(&input_a).copied().unwrap_or(0);
            let right = ctx.feature_roots.get(&input_b).copied().unwrap_or(0);

            let bool_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            let op = match bool_type.as_str() {
                "difference" => CsgOp::Difference { left, right },
                "intersection" => CsgOp::Intersection { left, right },
                _ => CsgOp::Union { left, right },
            };
            insert_node(doc, bool_id, &name, op);
            insert_transform_chain(doc, ctx, feature, bool_id, scale_id, rotate_id, translate_id);

            Some((
                PartInfo::Boolean {
                    id: id_str,
                    name,
                    boolean_type: bool_type,
                    boolean_node_id: bool_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                    source_part_ids: [input_a, input_b],
                },
                translate_id,
            ))
        }
        "fillet" => {
            let input = get_str(feature, "input").unwrap_or_default();
            let radius = get_f64(feature, "radius").unwrap_or(1.0);

            let child = ctx.feature_roots.get(&input).copied().unwrap_or(0);

            let fillet_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(doc, fillet_id, &name, CsgOp::Fillet { child, radius });
            insert_transform_chain(doc, ctx, feature, fillet_id, scale_id, rotate_id, translate_id);

            Some((
                PartInfo::Fillet {
                    id: id_str,
                    name,
                    source_part_id: input,
                    fillet_node_id: fillet_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        "chamfer" => {
            let input = get_str(feature, "input").unwrap_or_default();
            let distance = get_f64(feature, "distance").unwrap_or(1.0);

            let child = ctx.feature_roots.get(&input).copied().unwrap_or(0);

            let chamfer_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(doc, chamfer_id, &name, CsgOp::Chamfer { child, distance });
            insert_transform_chain(
                doc, ctx, feature, chamfer_id, scale_id, rotate_id, translate_id,
            );

            Some((
                PartInfo::Chamfer {
                    id: id_str,
                    name,
                    source_part_id: input,
                    chamfer_node_id: chamfer_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        "shell" => {
            let input = get_str(feature, "input").unwrap_or_default();
            let thickness = get_f64(feature, "thickness").unwrap_or(1.0);

            let child = ctx.feature_roots.get(&input).copied().unwrap_or(0);

            let shell_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(doc, shell_id, &name, CsgOp::Shell { child, thickness });
            insert_transform_chain(doc, ctx, feature, shell_id, scale_id, rotate_id, translate_id);

            Some((
                PartInfo::Shell {
                    id: id_str,
                    name,
                    source_part_id: input,
                    shell_node_id: shell_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        "extrude" => {
            let sketch_data = get_str(feature, "sketch");
            let depth = get_f64(feature, "depth").unwrap_or(10.0);
            let direction = get_vec3(feature, "direction").unwrap_or([0.0, 0.0, 1.0]);

            let sketch_id = ctx.alloc();
            let extrude_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            // Default sketch on XY plane
            let sketch_op = if let Some(data) = sketch_data {
                serde_json::from_str::<CsgOp>(&data).unwrap_or(CsgOp::Sketch2D {
                    origin: Vec3::new(0.0, 0.0, 0.0),
                    x_dir: Vec3::new(1.0, 0.0, 0.0),
                    y_dir: Vec3::new(0.0, 1.0, 0.0),
                    segments: Vec::new(),
                })
            } else {
                CsgOp::Sketch2D {
                    origin: Vec3::new(0.0, 0.0, 0.0),
                    x_dir: Vec3::new(1.0, 0.0, 0.0),
                    y_dir: Vec3::new(0.0, 1.0, 0.0),
                    segments: Vec::new(),
                }
            };
            insert_node(doc, sketch_id, &format!("{name} Sketch"), sketch_op);
            insert_node(
                doc,
                extrude_id,
                &name,
                CsgOp::Extrude {
                    sketch: sketch_id,
                    direction: Vec3::new(
                        direction[0] * depth,
                        direction[1] * depth,
                        direction[2] * depth,
                    ),
                    twist_angle: None,
                    scale_end: None,
                },
            );
            insert_transform_chain(
                doc, ctx, feature, extrude_id, scale_id, rotate_id, translate_id,
            );

            Some((
                PartInfo::Extrude {
                    id: id_str,
                    name,
                    sketch_node_id: sketch_id,
                    extrude_node_id: extrude_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        // Unknown feature kinds are silently skipped.
        _ => {
            let _ = crdt; // suppress unused warning
            None
        }
    }
}

// -- Helpers --

fn fid_to_string(fid: FeatureId) -> String {
    format!("{}:{}", fid.0 .0, fid.1)
}

fn get_f64(feature: &FeatureState, key: &str) -> Option<f64> {
    match &feature.params.get(key)?.0 {
        Value::F64(v) => Some(*v),
        _ => None,
    }
}

fn get_str(feature: &FeatureState, key: &str) -> Option<String> {
    match &feature.params.get(key)?.0 {
        Value::String(v) => Some(v.clone()),
        Value::FeatureRef(v) => Some(v.clone()),
        _ => None,
    }
}

fn get_vec3(feature: &FeatureState, key: &str) -> Option<[f64; 3]> {
    match &feature.params.get(key)?.0 {
        Value::Vec3(v) => Some(*v),
        _ => None,
    }
}

fn insert_node(doc: &mut Document, id: NodeId, name: &str, op: CsgOp) {
    doc.nodes.insert(
        id,
        Node {
            id,
            name: Some(name.to_string()),
            op,
        },
    );
}

/// Insert the standard transform chain: child → Scale → Rotate → Translate.
fn insert_transform_chain(
    doc: &mut Document,
    ctx: &mut Context,
    feature: &FeatureState,
    child_id: NodeId,
    scale_id: NodeId,
    rotate_id: NodeId,
    translate_id: NodeId,
) {
    let _ = ctx; // ctx available for future use

    let scale = get_vec3(feature, "scale").unwrap_or([1.0, 1.0, 1.0]);
    let rotation = get_vec3(feature, "rotation").unwrap_or([0.0, 0.0, 0.0]);
    let offset = get_vec3(feature, "offset").unwrap_or([0.0, 0.0, 0.0]);

    doc.nodes.insert(
        scale_id,
        Node {
            id: scale_id,
            name: None,
            op: CsgOp::Scale {
                child: child_id,
                factor: Vec3::new(scale[0], scale[1], scale[2]),
            },
        },
    );
    doc.nodes.insert(
        rotate_id,
        Node {
            id: rotate_id,
            name: None,
            op: CsgOp::Rotate {
                child: scale_id,
                angles: Vec3::new(rotation[0], rotation[1], rotation[2]),
            },
        },
    );
    doc.nodes.insert(
        translate_id,
        Node {
            id: translate_id,
            name: None,
            op: CsgOp::Translate {
                child: rotate_id,
                offset: Vec3::new(offset[0], offset[1], offset[2]),
            },
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_crdt::{FractionalIndex, ReplicaId};

    #[test]
    fn test_materialize_cube() {
        let mut crdt = CrdtDocument::new(ReplicaId(1));
        crdt.create_feature(
            "cube",
            FractionalIndex::between(None, None),
            HashMap::from([
                ("size_x".to_string(), Value::F64(10.0)),
                ("size_y".to_string(), Value::F64(20.0)),
                ("size_z".to_string(), Value::F64(30.0)),
            ]),
        );

        let result = materialize(&crdt);
        assert_eq!(result.parts.len(), 1);
        assert_eq!(result.document.roots.len(), 1);
        assert_eq!(result.document.nodes.len(), 4); // prim + scale + rotate + translate

        // Check the primitive node has correct dimensions
        let prim_node_id = match &result.parts[0] {
            PartInfo::Cube { primitive_node_id, .. } => *primitive_node_id,
            _ => panic!("expected cube part"),
        };
        let prim = result.document.nodes.get(&prim_node_id).unwrap();
        match &prim.op {
            CsgOp::Cube { size } => {
                assert_eq!(size.x, 10.0);
                assert_eq!(size.y, 20.0);
                assert_eq!(size.z, 30.0);
            }
            _ => panic!("expected Cube op"),
        }
    }

    #[test]
    fn test_materialize_multiple_features() {
        let mut crdt = CrdtDocument::new(ReplicaId(1));
        crdt.create_feature(
            "cube",
            FractionalIndex::between(None, None),
            HashMap::from([("size_x".to_string(), Value::F64(10.0))]),
        );
        crdt.create_feature(
            "cylinder",
            FractionalIndex::between(None, None),
            HashMap::from([
                ("radius".to_string(), Value::F64(5.0)),
                ("height".to_string(), Value::F64(20.0)),
            ]),
        );

        let result = materialize(&crdt);
        assert_eq!(result.parts.len(), 2);
        assert_eq!(result.document.roots.len(), 2);
    }

    #[test]
    fn test_materialize_with_transforms() {
        let mut crdt = CrdtDocument::new(ReplicaId(1));
        crdt.create_feature(
            "cube",
            FractionalIndex::between(None, None),
            HashMap::from([
                ("size_x".to_string(), Value::F64(10.0)),
                ("size_y".to_string(), Value::F64(10.0)),
                ("size_z".to_string(), Value::F64(10.0)),
                ("offset".to_string(), Value::Vec3([5.0, 10.0, 15.0])),
                ("rotation".to_string(), Value::Vec3([0.0, 0.0, 45.0])),
            ]),
        );

        let result = materialize(&crdt);
        let translate_id = result.parts[0].root_node_id();
        let translate = result.document.nodes.get(&translate_id).unwrap();
        match &translate.op {
            CsgOp::Translate { offset, .. } => {
                assert_eq!(offset.x, 5.0);
                assert_eq!(offset.y, 10.0);
                assert_eq!(offset.z, 15.0);
            }
            _ => panic!("expected Translate"),
        }
    }

    #[test]
    fn test_materialize_boolean() {
        let mut crdt = CrdtDocument::new(ReplicaId(1));
        let pos1 = FractionalIndex::between(None, None);
        let (fid1, _) = crdt.create_feature("cube", pos1.clone(), HashMap::new());

        let pos2 = FractionalIndex::between(Some(&pos1), None);
        let (fid2, _) = crdt.create_feature("cylinder", pos2.clone(), HashMap::new());

        let id1_str = fid_to_string(fid1);
        let id2_str = fid_to_string(fid2);

        let pos3 = FractionalIndex::between(Some(&pos2), None);
        crdt.create_feature(
            "boolean",
            pos3,
            HashMap::from([
                ("boolean_type".to_string(), Value::String("difference".to_string())),
                ("input_a".to_string(), Value::FeatureRef(id1_str)),
                ("input_b".to_string(), Value::FeatureRef(id2_str)),
            ]),
        );

        let result = materialize(&crdt);
        assert_eq!(result.parts.len(), 3);
        match &result.parts[2] {
            PartInfo::Boolean { boolean_type, .. } => {
                assert_eq!(boolean_type, "difference");
            }
            _ => panic!("expected boolean part"),
        }
    }

    #[test]
    fn test_materialize_fillet() {
        let mut crdt = CrdtDocument::new(ReplicaId(1));
        let pos1 = FractionalIndex::between(None, None);
        let (fid, _) = crdt.create_feature("cube", pos1.clone(), HashMap::new());
        let id_str = fid_to_string(fid);

        let pos2 = FractionalIndex::between(Some(&pos1), None);
        crdt.create_feature(
            "fillet",
            pos2,
            HashMap::from([
                ("input".to_string(), Value::FeatureRef(id_str.clone())),
                ("radius".to_string(), Value::F64(2.0)),
            ]),
        );

        let result = materialize(&crdt);
        assert_eq!(result.parts.len(), 2);
        match &result.parts[1] {
            PartInfo::Fillet {
                source_part_id,
                fillet_node_id,
                ..
            } => {
                assert_eq!(source_part_id, &id_str);
                let node = result.document.nodes.get(fillet_node_id).unwrap();
                match &node.op {
                    CsgOp::Fillet { radius, .. } => assert_eq!(*radius, 2.0),
                    _ => panic!("expected Fillet op"),
                }
            }
            _ => panic!("expected fillet part"),
        }
    }

    #[test]
    fn test_unknown_feature_skipped() {
        let mut crdt = CrdtDocument::new(ReplicaId(1));
        crdt.create_feature(
            "unknown_thing",
            FractionalIndex::between(None, None),
            HashMap::new(),
        );

        let result = materialize(&crdt);
        assert_eq!(result.parts.len(), 0);
        assert_eq!(result.document.roots.len(), 0);
    }
}
