//! Pre-flight reference validation for documents.
//!
//! Walks every `NodeId` reference reachable from a `Document` and reports the
//! first dangling reference together with a human-readable path (e.g.
//! `nodes[47].Translate.child`, `roots[2].root`, `partDefs["arm"].root`).

use vcad_ir::{CsgOp, Document, NodeId};

use crate::EvalError;

/// Verify every `NodeId` reachable from `doc` resolves to a node in `doc.nodes`.
///
/// Returns the first dangling reference with its path, or `Ok(())` if all
/// references are valid. O(total refs) — one pass with pure HashMap lookups.
pub fn validate_document(doc: &Document) -> Result<(), EvalError> {
    for (i, entry) in doc.roots.iter().enumerate() {
        check_ref(doc, entry.root, || format!("roots[{i}].root"))?;
    }

    if let Some(part_defs) = &doc.part_defs {
        for (id, part_def) in part_defs {
            check_ref(doc, part_def.root, || format!("partDefs[{id:?}].root"))?;
        }
    }

    for (node_id, node) in &doc.nodes {
        validate_op(doc, *node_id, &node.op)?;
    }

    Ok(())
}

fn validate_op(doc: &Document, node_id: NodeId, op: &CsgOp) -> Result<(), EvalError> {
    let op_name = csg_op_name(op);
    let at = |field: &str| format!("nodes[{node_id}].{op_name}.{field}");

    match op {
        CsgOp::Union { left, right }
        | CsgOp::Difference { left, right }
        | CsgOp::Intersection { left, right } => {
            check_ref(doc, *left, || at("left"))?;
            check_ref(doc, *right, || at("right"))?;
        }
        CsgOp::Translate { child, .. }
        | CsgOp::Rotate { child, .. }
        | CsgOp::Scale { child, .. }
        | CsgOp::LinearPattern { child, .. }
        | CsgOp::CircularPattern { child, .. }
        | CsgOp::Shell { child, .. }
        | CsgOp::Fillet { child, .. }
        | CsgOp::Chamfer { child, .. } => {
            check_ref(doc, *child, || at("child"))?;
        }
        CsgOp::Extrude { sketch, .. }
        | CsgOp::Revolve { sketch, .. }
        | CsgOp::Sweep { sketch, .. } => {
            check_ref(doc, *sketch, || at("sketch"))?;
        }
        CsgOp::Loft { sketches, .. } => {
            for (i, s) in sketches.iter().enumerate() {
                check_ref(doc, *s, || format!("nodes[{node_id}].Loft.sketches[{i}]"))?;
            }
        }
        // Leaf ops without NodeId references.
        CsgOp::Cube { .. }
        | CsgOp::Cylinder { .. }
        | CsgOp::Sphere { .. }
        | CsgOp::Cone { .. }
        | CsgOp::Empty
        | CsgOp::Sketch2D { .. }
        | CsgOp::Text2D { .. }
        | CsgOp::ImportedMesh { .. }
        | CsgOp::StepImport { .. }
        | CsgOp::MeshImport { .. }
        | CsgOp::PcbBoard { .. }
        | CsgOp::EmbroideryPattern { .. }
        | CsgOp::PartInstance { .. } => {}
    }

    Ok(())
}

fn check_ref<F: FnOnce() -> String>(
    doc: &Document,
    node_id: NodeId,
    path: F,
) -> Result<(), EvalError> {
    if doc.nodes.contains_key(&node_id) {
        Ok(())
    } else {
        Err(EvalError::MissingNodeAt {
            node_id,
            path: path(),
        })
    }
}

fn csg_op_name(op: &CsgOp) -> &'static str {
    match op {
        CsgOp::Cube { .. } => "Cube",
        CsgOp::Cylinder { .. } => "Cylinder",
        CsgOp::Sphere { .. } => "Sphere",
        CsgOp::Cone { .. } => "Cone",
        CsgOp::Empty => "Empty",
        CsgOp::Union { .. } => "Union",
        CsgOp::Difference { .. } => "Difference",
        CsgOp::Intersection { .. } => "Intersection",
        CsgOp::Translate { .. } => "Translate",
        CsgOp::Rotate { .. } => "Rotate",
        CsgOp::Scale { .. } => "Scale",
        CsgOp::LinearPattern { .. } => "LinearPattern",
        CsgOp::CircularPattern { .. } => "CircularPattern",
        CsgOp::Shell { .. } => "Shell",
        CsgOp::Fillet { .. } => "Fillet",
        CsgOp::Chamfer { .. } => "Chamfer",
        CsgOp::Sketch2D { .. } => "Sketch2D",
        CsgOp::Text2D { .. } => "Text2D",
        CsgOp::Extrude { .. } => "Extrude",
        CsgOp::Revolve { .. } => "Revolve",
        CsgOp::Sweep { .. } => "Sweep",
        CsgOp::Loft { .. } => "Loft",
        CsgOp::ImportedMesh { .. } => "ImportedMesh",
        CsgOp::StepImport { .. } => "StepImport",
        CsgOp::MeshImport { .. } => "MeshImport",
        CsgOp::PcbBoard { .. } => "PcbBoard",
        CsgOp::EmbroideryPattern { .. } => "EmbroideryPattern",
        CsgOp::PartInstance { .. } => "PartInstance",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use vcad_ir::{Node, PartDef, SceneEntry, Vec3};

    fn cube_node(id: NodeId) -> Node {
        Node {
            id,
            name: None,
            op: CsgOp::Cube {
                size: Vec3::new(1.0, 1.0, 1.0),
            },
        }
    }

    #[test]
    fn translate_missing_child_reports_path() {
        let mut doc = Document::new();
        doc.nodes.insert(
            5,
            Node {
                id: 5,
                name: None,
                op: CsgOp::Translate {
                    child: 0,
                    offset: Vec3::new(1.0, 2.0, 3.0),
                },
            },
        );

        let err = validate_document(&doc).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains('0'), "error should mention node id 0: {msg}");
        assert!(
            msg.contains("nodes[5].Translate.child"),
            "error should mention path: {msg}"
        );
    }

    #[test]
    fn root_missing_node_reports_path() {
        let mut doc = Document::new();
        doc.roots.push(SceneEntry {
            root: 999,
            material: "default".to_string(),
            visible: None,
        });

        let err = validate_document(&doc).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("999"), "error should mention 999: {msg}");
        assert!(
            msg.contains("roots[0].root"),
            "error should mention roots[0].root: {msg}"
        );
    }

    #[test]
    fn part_def_missing_root_reports_path() {
        let mut doc = Document::new();
        let mut pd = HashMap::new();
        pd.insert(
            "arm".to_string(),
            PartDef {
                id: "arm".to_string(),
                name: None,
                root: 42,
                default_material: None,
                inertial: None,
            },
        );
        doc.part_defs = Some(pd);

        let err = validate_document(&doc).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("42"), "error should mention 42: {msg}");
        assert!(
            msg.contains("partDefs[\"arm\"].root"),
            "error should mention partDefs[\"arm\"].root: {msg}"
        );
    }

    #[test]
    fn loft_indexes_missing_sketch() {
        let mut doc = Document::new();
        doc.nodes.insert(1, cube_node(1));
        doc.nodes.insert(
            10,
            Node {
                id: 10,
                name: None,
                op: CsgOp::Loft {
                    sketches: vec![1, 7, 1],
                    closed: None,
                },
            },
        );

        let err = validate_document(&doc).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("nodes[10].Loft.sketches[1]"),
            "error should mention Loft.sketches[1]: {msg}"
        );
        assert!(msg.contains('7'), "error should mention node id 7: {msg}");
    }

    #[test]
    fn valid_document_passes() {
        let mut doc = Document::new();
        doc.nodes.insert(1, cube_node(1));
        doc.roots.push(SceneEntry {
            root: 1,
            material: "default".to_string(),
            visible: None,
        });
        assert!(validate_document(&doc).is_ok());
    }
}
