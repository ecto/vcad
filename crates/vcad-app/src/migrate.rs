//! V1 file format migration.
//!
//! Converts legacy `.vcad` JSON documents (v1 format: IR `Document` with parts
//! array) into the new CRDT document format. Features are reconstructed by
//! walking the existing nodes and emitting CreateFeature + SetParam ops.

use std::collections::HashMap;

use vcad_crdt::{CrdtDocument, FeatureId, FractionalIndex, ReplicaId, Value};
use vcad_ir::{CsgOp, Document, JointKind, NodeId, Vec3};

/// Migrate a v1 document (IR DAG + parts metadata) into a CRDT document.
///
/// Each scene entry's root node tree is converted recursively: primitives
/// become leaf features, boolean/modifier operations become features that
/// reference previously-migrated child features by id. Assembly metadata
/// (partDefs / instances / joints) migrates to its own feature kinds after
/// the regular scene roots.
pub fn migrate_v1(doc: &Document) -> CrdtDocument {
    let mut crdt = CrdtDocument::new(ReplicaId(0));
    let mut ctx = MigrationCtx::new();

    for entry in &doc.roots {
        let _ = migrate_node(&mut crdt, &mut ctx, doc, entry.root);
    }

    migrate_assembly(&mut crdt, &mut ctx, doc);

    crdt
}

/// Tracks the position cursor so each new feature appends at the end of
/// the ordered feature list.
struct MigrationCtx {
    last_pos: Option<FractionalIndex>,
}

impl MigrationCtx {
    fn new() -> Self {
        Self { last_pos: None }
    }

    /// Allocate the next ordered position (strictly greater than all prior).
    fn next_position(&mut self) -> FractionalIndex {
        let pos = FractionalIndex::between(self.last_pos.as_ref(), None);
        self.last_pos = Some(pos.clone());
        pos
    }
}

/// Recursively migrate a subtree rooted at `node_id` into one or more
/// CRDT features. Returns the `FeatureId` of the topmost feature created
/// for this subtree (the one callers should reference as an input).
fn migrate_node(
    crdt: &mut CrdtDocument,
    ctx: &mut MigrationCtx,
    doc: &Document,
    node_id: NodeId,
) -> Option<FeatureId> {
    let mut params: HashMap<String, Value> = HashMap::new();
    let mut current_id = node_id;

    // Peel off the Translate → Rotate → Scale chain, accumulating
    // offset/rotation/scale/name params on the resulting feature.
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
            _ => break,
        }
    }

    let leaf = doc.nodes.get(&current_id)?;
    match &leaf.op {
        CsgOp::Cube { size } => {
            params.insert("size_x".to_string(), Value::F64(size.x));
            params.insert("size_y".to_string(), Value::F64(size.y));
            params.insert("size_z".to_string(), Value::F64(size.z));
            Some(create(crdt, ctx, "cube", params))
        }
        CsgOp::Cylinder {
            radius,
            height,
            segments,
        } => {
            params.insert("radius".to_string(), Value::F64(*radius));
            params.insert("height".to_string(), Value::F64(*height));
            params.insert("segments".to_string(), Value::F64(*segments as f64));
            Some(create(crdt, ctx, "cylinder", params))
        }
        CsgOp::Sphere { radius, segments } => {
            params.insert("radius".to_string(), Value::F64(*radius));
            params.insert("segments".to_string(), Value::F64(*segments as f64));
            Some(create(crdt, ctx, "sphere", params))
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
            Some(create(crdt, ctx, "cone", params))
        }
        // Boolean ops: migrate both operands first so their feature ids
        // exist before the boolean feature that references them.
        CsgOp::Union { left, right } => {
            migrate_boolean(crdt, ctx, doc, *left, *right, "union", params)
        }
        CsgOp::Difference { left, right } => {
            migrate_boolean(crdt, ctx, doc, *left, *right, "difference", params)
        }
        CsgOp::Intersection { left, right } => {
            migrate_boolean(crdt, ctx, doc, *left, *right, "intersection", params)
        }
        // Unary modifiers: migrate the child first, then reference it.
        CsgOp::Fillet { child, radius } => {
            let input = migrate_node(crdt, ctx, doc, *child)?;
            params.insert("input".to_string(), Value::FeatureRef(fid_to_stable(input)));
            params.insert("radius".to_string(), Value::F64(*radius));
            Some(create(crdt, ctx, "fillet", params))
        }
        CsgOp::Chamfer { child, distance } => {
            let input = migrate_node(crdt, ctx, doc, *child)?;
            params.insert("input".to_string(), Value::FeatureRef(fid_to_stable(input)));
            params.insert("distance".to_string(), Value::F64(*distance));
            Some(create(crdt, ctx, "chamfer", params))
        }
        CsgOp::Shell { child, thickness } => {
            let input = migrate_node(crdt, ctx, doc, *child)?;
            params.insert("input".to_string(), Value::FeatureRef(fid_to_stable(input)));
            params.insert("thickness".to_string(), Value::F64(*thickness));
            Some(create(crdt, ctx, "shell", params))
        }
        CsgOp::LinearPattern {
            child,
            direction,
            count,
            spacing,
        } => {
            let input = migrate_node(crdt, ctx, doc, *child)?;
            params.insert("input".to_string(), Value::FeatureRef(fid_to_stable(input)));
            params.insert(
                "direction".to_string(),
                Value::Vec3([direction.x, direction.y, direction.z]),
            );
            params.insert("count".to_string(), Value::F64(*count as f64));
            params.insert("spacing".to_string(), Value::F64(*spacing));
            Some(create(crdt, ctx, "linear-pattern", params))
        }
        CsgOp::CircularPattern {
            child,
            axis_origin,
            axis_dir,
            count,
            angle_deg,
        } => {
            let input = migrate_node(crdt, ctx, doc, *child)?;
            params.insert("input".to_string(), Value::FeatureRef(fid_to_stable(input)));
            params.insert(
                "axis_origin".to_string(),
                Value::Vec3([axis_origin.x, axis_origin.y, axis_origin.z]),
            );
            params.insert(
                "axis_dir".to_string(),
                Value::Vec3([axis_dir.x, axis_dir.y, axis_dir.z]),
            );
            params.insert("count".to_string(), Value::F64(*count as f64));
            params.insert("angle_deg".to_string(), Value::F64(*angle_deg));
            Some(create(crdt, ctx, "circular-pattern", params))
        }
        CsgOp::Extrude {
            sketch,
            direction,
            twist_angle,
            scale_end,
        } => {
            let depth =
                (direction.x * direction.x + direction.y * direction.y + direction.z * direction.z)
                    .sqrt();
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
            if let Some(sketch_node) = doc.nodes.get(sketch) {
                if let Ok(json) = serde_json::to_string(&sketch_node.op) {
                    params.insert("sketch".to_string(), Value::String(json));
                }
            }
            if let Some(ta) = twist_angle {
                params.insert("twist_angle".to_string(), Value::F64(*ta));
            }
            if let Some(se) = scale_end {
                params.insert("scale_end".to_string(), Value::F64(*se));
            }
            Some(create(crdt, ctx, "extrude", params))
        }
        CsgOp::Revolve {
            sketch,
            axis_origin,
            axis_dir,
            angle_deg,
        } => {
            if let Some(sketch_node) = doc.nodes.get(sketch) {
                if let Ok(json) = serde_json::to_string(&sketch_node.op) {
                    params.insert("sketch".to_string(), Value::String(json));
                }
            }
            params.insert(
                "axis_origin".to_string(),
                Value::Vec3([axis_origin.x, axis_origin.y, axis_origin.z]),
            );
            params.insert(
                "axis_dir".to_string(),
                Value::Vec3([axis_dir.x, axis_dir.y, axis_dir.z]),
            );
            params.insert("angle_deg".to_string(), Value::F64(*angle_deg));
            Some(create(crdt, ctx, "revolve", params))
        }
        CsgOp::Sweep {
            sketch,
            path,
            twist_angle,
            scale_start,
            scale_end,
            ..
        } => {
            if let Some(sketch_node) = doc.nodes.get(sketch) {
                if let Ok(json) = serde_json::to_string(&sketch_node.op) {
                    params.insert("sketch".to_string(), Value::String(json));
                }
            }
            if let Ok(path_json) = serde_json::to_string(path) {
                params.insert("path".to_string(), Value::String(path_json));
            }
            if let Some(ta) = twist_angle {
                params.insert("twist_angle".to_string(), Value::F64(*ta));
            }
            if let Some(ss) = scale_start {
                params.insert("scale_start".to_string(), Value::F64(*ss));
            }
            if let Some(se) = scale_end {
                params.insert("scale_end".to_string(), Value::F64(*se));
            }
            Some(create(crdt, ctx, "sweep", params))
        }
        CsgOp::Loft { sketches, closed } => {
            params.insert(
                "sketch_count".to_string(),
                Value::F64(sketches.len() as f64),
            );
            for (i, sketch_id) in sketches.iter().enumerate() {
                if let Some(sketch_node) = doc.nodes.get(sketch_id) {
                    if let Ok(json) = serde_json::to_string(&sketch_node.op) {
                        params.insert(format!("sketch_{i}"), Value::String(json));
                    }
                }
            }
            if let Some(true) = closed {
                params.insert("closed".to_string(), Value::Bool(true));
            }
            Some(create(crdt, ctx, "loft", params))
        }
        CsgOp::Text2D { text, height, .. } => {
            params.insert("text".to_string(), Value::String(text.clone()));
            params.insert("height".to_string(), Value::F64(*height));
            Some(create(crdt, ctx, "text", params))
        }
        CsgOp::ImportedMesh {
            positions,
            indices,
            normals,
            source,
        } => {
            if let Ok(json) = serde_json::to_string(positions) {
                params.insert("positions_json".to_string(), Value::String(json));
            }
            if let Ok(json) = serde_json::to_string(indices) {
                params.insert("indices_json".to_string(), Value::String(json));
            }
            if let Some(n) = normals {
                if let Ok(json) = serde_json::to_string(n) {
                    params.insert("normals_json".to_string(), Value::String(json));
                }
            }
            if let Some(s) = source {
                params.insert("source".to_string(), Value::String(s.clone()));
            }
            Some(create(crdt, ctx, "imported-mesh", params))
        }
        // Ops we don't represent in the CRDT feature model — drop silently.
        _ => None,
    }
}

fn migrate_boolean(
    crdt: &mut CrdtDocument,
    ctx: &mut MigrationCtx,
    doc: &Document,
    left: NodeId,
    right: NodeId,
    boolean_type: &str,
    mut params: HashMap<String, Value>,
) -> Option<FeatureId> {
    let a = migrate_node(crdt, ctx, doc, left)?;
    let b = migrate_node(crdt, ctx, doc, right)?;
    params.insert(
        "boolean_type".to_string(),
        Value::String(boolean_type.to_string()),
    );
    params.insert("input_a".to_string(), Value::FeatureRef(fid_to_stable(a)));
    params.insert("input_b".to_string(), Value::FeatureRef(fid_to_stable(b)));
    Some(create(crdt, ctx, "boolean", params))
}

fn create(
    crdt: &mut CrdtDocument,
    ctx: &mut MigrationCtx,
    kind: &str,
    params: HashMap<String, Value>,
) -> FeatureId {
    let position = ctx.next_position();
    let (fid, _) = crdt.create_feature(kind, position, params);
    fid
}

/// Format a `FeatureId` as the stable `"replica:seq"` string used throughout
/// the higher-level API (see `document_api::StableIdMap`).
fn fid_to_stable(fid: FeatureId) -> String {
    format!("{}:{}", fid.0 .0, fid.1)
}

/// Migrate the assembly portion of a v1 document: part definitions, their
/// instances, and the joints connecting them. No-op when the document has
/// no `partDefs`.
fn migrate_assembly(crdt: &mut CrdtDocument, ctx: &mut MigrationCtx, doc: &Document) {
    let Some(part_defs) = &doc.part_defs else {
        return;
    };

    // partDef id (string in v1) -> stable feature id of the `part-def` feature.
    let mut part_def_stable: HashMap<String, String> = HashMap::new();
    for (key, pd) in part_defs {
        let Some(source_fid) = migrate_node(crdt, ctx, doc, pd.root) else {
            continue;
        };
        let mut params = HashMap::new();
        params.insert(
            "source_feature".to_string(),
            Value::FeatureRef(fid_to_stable(source_fid)),
        );
        if let Some(n) = &pd.name {
            params.insert("name".to_string(), Value::String(n.clone()));
        }
        if let Some(m) = &pd.default_material {
            params.insert("default_material".to_string(), Value::String(m.clone()));
        }
        let pd_fid = create(crdt, ctx, "part-def", params);
        part_def_stable.insert(key.clone(), fid_to_stable(pd_fid));
        // Also key by the partDef's own id field, since instances reference
        // `partDefId` which matches `PartDef::id` (typically equal to the
        // HashMap key but not guaranteed).
        if pd.id != *key {
            part_def_stable.insert(pd.id.clone(), fid_to_stable(pd_fid));
        }
    }

    // instance.id -> stable feature id of the `instance` feature.
    let mut instance_stable: HashMap<String, String> = HashMap::new();
    if let Some(instances) = &doc.instances {
        for inst in instances {
            let Some(pd_stable) = part_def_stable.get(&inst.part_def_id) else {
                continue;
            };
            let mut params = HashMap::new();
            params.insert("part_def".to_string(), Value::FeatureRef(pd_stable.clone()));
            if let Some(n) = &inst.name {
                params.insert("name".to_string(), Value::String(n.clone()));
            }
            if let Some(t) = &inst.transform {
                if let Ok(json) = serde_json::to_string(t) {
                    params.insert("transform".to_string(), Value::String(json));
                }
            }
            if let Some(m) = &inst.material {
                params.insert("material".to_string(), Value::String(m.clone()));
            }
            if doc.ground_instance_id.as_deref() == Some(&inst.id) {
                params.insert("is_ground".to_string(), Value::Bool(true));
            }
            let inst_fid = create(crdt, ctx, "instance", params);
            instance_stable.insert(inst.id.clone(), fid_to_stable(inst_fid));
        }
    }

    if let Some(joints) = &doc.joints {
        for joint in joints {
            let Some(child_stable) = instance_stable.get(&joint.child_instance_id) else {
                continue;
            };
            let parent_stable = joint
                .parent_instance_id
                .as_ref()
                .and_then(|id| instance_stable.get(id));

            let (kind_str, axis, limits_json) = match &joint.kind {
                JointKind::Fixed => ("Fixed", None, None),
                JointKind::Revolute { axis, limits } => (
                    "Revolute",
                    Some([axis.x, axis.y, axis.z]),
                    limits.as_ref().and_then(|l| serde_json::to_string(l).ok()),
                ),
                JointKind::Slider { axis, limits } => (
                    "Slider",
                    Some([axis.x, axis.y, axis.z]),
                    limits.as_ref().and_then(|l| serde_json::to_string(l).ok()),
                ),
                JointKind::Cylindrical { axis } => {
                    ("Cylindrical", Some([axis.x, axis.y, axis.z]), None)
                }
                JointKind::Ball => ("Ball", None, None),
            };

            let mut params = HashMap::new();
            params.insert("kind".to_string(), Value::String(kind_str.to_string()));
            params.insert(
                "instance_b".to_string(),
                Value::FeatureRef(child_stable.clone()),
            );
            if let Some(p) = parent_stable {
                params.insert("instance_a".to_string(), Value::FeatureRef(p.clone()));
            }
            params.insert(
                "anchor_a".to_string(),
                Value::Vec3([
                    joint.parent_anchor.x,
                    joint.parent_anchor.y,
                    joint.parent_anchor.z,
                ]),
            );
            params.insert(
                "anchor_b".to_string(),
                Value::Vec3([
                    joint.child_anchor.x,
                    joint.child_anchor.y,
                    joint.child_anchor.z,
                ]),
            );
            if let Some(a) = axis {
                params.insert("axis".to_string(), Value::Vec3(a));
            }
            if let Some(n) = &joint.name {
                params.insert("name".to_string(), Value::String(n.clone()));
            }
            if let Some(j) = limits_json {
                params.insert("limits".to_string(), Value::String(j));
            }
            params.insert("state".to_string(), Value::F64(joint.state));

            create(crdt, ctx, "joint", params);
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
