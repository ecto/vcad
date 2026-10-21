//! Build the document a STEP import produces: one root per body, either a
//! lazy B-rep `StepImport` node (the bytes are registered with the kernel
//! under `path`) or a tessellated `ImportedMesh`, all in one material, with
//! the default steel/aluminum/default material table.

use serde_json::json;
use vcad_ir::{CsgOp, Document, MaterialDef, Node, SceneEntry};

/// One imported body.
#[derive(Debug, Clone)]
pub enum StepBody {
    /// Lazy B-rep reference to solid `solid_index` of the file at `path`.
    Brep {
        /// Path the STEP bytes are registered under.
        path: String,
        /// 0-based solid index within the file.
        solid_index: u32,
    },
    /// Tessellated body.
    Mesh {
        /// Flat vertex positions (mm).
        positions: Vec<f64>,
        /// Triangle indices.
        indices: Vec<u32>,
        /// Optional per-vertex normals.
        normals: Option<Vec<f64>>,
        /// Source label for display.
        source: String,
    },
}

/// The material table every STEP import starts with.
pub fn default_materials() -> std::collections::HashMap<String, MaterialDef> {
    let defs = json!({
        "steel": {"name": "Steel", "color": [0.6, 0.6, 0.65], "metallic": 0.9, "roughness": 0.3, "density": 7850.0},
        "aluminum": {"name": "Aluminum", "color": [0.8, 0.8, 0.85], "metallic": 0.9, "roughness": 0.2, "density": 2700.0},
        "default": {"name": "Default", "color": [0.8, 0.8, 0.8], "metallic": 0.0, "roughness": 0.5},
    });
    serde_json::from_value(defs).expect("static material table")
}

/// Build the import document. A single body is named `part_name`; several
/// are `part_name_1`, `part_name_2`, … . Node ids start at 1.
pub fn step_import_document(part_name: &str, material: &str, bodies: Vec<StepBody>) -> Document {
    let mut doc = Document {
        materials: default_materials(),
        ..Document::default()
    };
    let many = bodies.len() > 1;
    for (i, body) in bodies.into_iter().enumerate() {
        let id = i as u64 + 1;
        let name = if many {
            format!("{part_name}_{id}")
        } else {
            part_name.to_string()
        };
        let op = match body {
            StepBody::Brep { path, solid_index } => CsgOp::StepImport {
                path,
                solid_index: (solid_index != 0).then_some(solid_index),
            },
            StepBody::Mesh {
                positions,
                indices,
                normals,
                source,
            } => CsgOp::ImportedMesh {
                positions,
                indices,
                normals,
                source: Some(source),
            },
        };
        doc.nodes.insert(
            id,
            Node {
                id,
                name: Some(name.clone()),
                op,
            },
        );
        doc.roots.push(SceneEntry {
            root: id,
            material: material.to_string(),
            visible: None,
        });
        doc.part_materials.insert(name, material.to_string());
    }
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brep_bodies_name_and_index() {
        let doc = step_import_document(
            "bracket",
            "aluminum",
            vec![
                StepBody::Brep {
                    path: "/x/a.step".into(),
                    solid_index: 0,
                },
                StepBody::Brep {
                    path: "/x/a.step".into(),
                    solid_index: 3,
                },
            ],
        );
        assert_eq!(doc.roots.len(), 2);
        assert_eq!(doc.roots[1].material, "aluminum");
        assert_eq!(doc.nodes[&1].name.as_deref(), Some("bracket_1"));
        assert_eq!(doc.part_materials["bracket_2"], "aluminum");
        match (&doc.nodes[&1].op, &doc.nodes[&2].op) {
            (
                CsgOp::StepImport {
                    solid_index: None, ..
                },
                CsgOp::StepImport {
                    solid_index: Some(3),
                    path,
                },
            ) => assert_eq!(path, "/x/a.step"),
            other => panic!("{other:?}"),
        }
        assert_eq!(doc.materials["steel"].density, Some(7850.0));
        assert_eq!(doc.materials.len(), 3);
    }

    #[test]
    fn single_mesh_body_keeps_the_part_name() {
        let doc = step_import_document(
            "plate",
            "steel",
            vec![StepBody::Mesh {
                positions: vec![0.0; 9],
                indices: vec![0, 1, 2],
                normals: None,
                source: "plate.step".into(),
            }],
        );
        assert_eq!(doc.nodes[&1].name.as_deref(), Some("plate"));
        assert!(matches!(
            &doc.nodes[&1].op,
            CsgOp::ImportedMesh { source: Some(s), .. } if s == "plate.step"
        ));
    }
}
