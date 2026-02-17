//! Canonical document evaluator for the vcad IR.
//!
//! This crate provides a single, authoritative implementation of the vcad
//! document evaluator. It walks the IR DAG, calls vcad-kernel operations,
//! and produces triangle meshes ready for rendering.
//!
//! # Usage
//!
//! ```ignore
//! use vcad_eval::{evaluate_document, EvalOptions};
//!
//! let doc = vcad_ir::Document::from_json(json_str)?;
//! let scene = evaluate_document(&doc, &EvalOptions::default())?;
//! for part in &scene.parts {
//!     // part.mesh.positions, part.mesh.indices, etc.
//! }
//! ```

pub mod convert;
pub mod evaluate;
pub mod kinematics;

use serde::{Deserialize, Serialize};
use vcad_ir::Transform3D;
use vcad_kernel::Solid;

// Re-export main entry points
pub use evaluate::{evaluate_document, evaluate_node};
pub use kinematics::solve_forward_kinematics;

/// Options for document evaluation.
#[derive(Debug, Clone, Default)]
pub struct EvalOptions {
    /// Skip O(n^2) clash detection for faster parametric editing.
    pub skip_clash_detection: bool,
}

/// Errors that can occur during evaluation.
#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    /// A node referenced by ID was not found in the document.
    #[error("missing node: {0}")]
    MissingNode(vcad_ir::NodeId),

    /// An Extrude/Revolve/Sweep references a node that is not a Sketch2D.
    #[error("invalid sketch reference — expected Sketch2D node")]
    InvalidSketchRef,

    /// Sketch profile construction failed.
    #[error("sketch error: {0}")]
    Sketch(vcad_kernel_sketch::SketchError),

    /// Sweep operation failed.
    #[error("sweep error: {0}")]
    Sweep(vcad_kernel_sweep::SweepError),

    /// Loft operation failed.
    #[error("loft error: {0}")]
    Loft(vcad_kernel_sweep::LoftError),

    /// Unknown font name.
    #[error("unknown font: {0}")]
    UnknownFont(String),
}

/// Triangle mesh output from evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluatedMesh {
    /// Flat vertex positions (x, y, z, x, y, z, ...).
    pub positions: Vec<f32>,
    /// Triangle indices.
    pub indices: Vec<u32>,
    /// Optional vertex normals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normals: Option<Vec<f32>>,
}

impl EvaluatedMesh {
    /// Create an empty mesh.
    pub fn empty() -> Self {
        Self {
            positions: Vec::new(),
            indices: Vec::new(),
            normals: None,
        }
    }
}

/// A single evaluated part with mesh, material, and optional BRep solid.
#[derive(Debug, Clone)]
pub struct EvaluatedPart {
    /// Triangle mesh for rendering.
    pub mesh: EvaluatedMesh,
    /// Material key.
    pub material: String,
    /// Optional BRep solid (for ray tracing or further operations).
    pub solid: Option<Solid>,
}

/// A part definition in an assembly (reusable geometry).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluatedPartDef {
    /// Part definition ID.
    pub id: String,
    /// Triangle mesh.
    pub mesh: EvaluatedMesh,
}

/// An instance of a part definition with transform and material.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluatedInstance {
    /// Instance ID.
    pub instance_id: String,
    /// Part definition ID.
    pub part_def_id: String,
    /// Optional name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Triangle mesh.
    pub mesh: EvaluatedMesh,
    /// Material key.
    pub material: String,
    /// World transform.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transform: Option<Transform3D>,
}

/// Result of evaluating a full document.
#[derive(Debug, Clone)]
pub struct EvaluatedScene {
    /// Evaluated parts from document roots.
    pub parts: Vec<EvaluatedPart>,
    /// Part definitions for assembly mode.
    pub part_defs: Option<Vec<EvaluatedPartDef>>,
    /// Instances for assembly mode.
    pub instances: Option<Vec<EvaluatedInstance>>,
    /// Clash meshes (intersections between overlapping parts).
    pub clashes: Vec<EvaluatedMesh>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use vcad_ir::*;

    fn make_cube_doc(sx: f64, sy: f64, sz: f64) -> Document {
        let mut doc = Document::new();
        doc.nodes.insert(
            1,
            Node {
                id: 1,
                name: Some("cube".to_string()),
                op: CsgOp::Cube {
                    size: Vec3::new(sx, sy, sz),
                },
            },
        );
        doc.roots.push(SceneEntry {
            root: 1,
            material: "default".to_string(),
            visible: None,
        });
        doc.materials.insert(
            "default".to_string(),
            MaterialDef {
                name: "default".to_string(),
                color: [0.8, 0.8, 0.8],
                metallic: 0.0,
                roughness: 0.5,
                density: None,
                friction: None,
            },
        );
        doc
    }

    #[test]
    fn evaluate_cube() {
        let doc = make_cube_doc(10.0, 20.0, 30.0);
        let scene = evaluate_document(&doc, &EvalOptions::default()).unwrap();
        assert_eq!(scene.parts.len(), 1);
        assert!(!scene.parts[0].mesh.positions.is_empty());
        assert!(!scene.parts[0].mesh.indices.is_empty());
    }

    #[test]
    fn evaluate_boolean_difference() {
        let mut doc = make_cube_doc(10.0, 10.0, 10.0);
        doc.nodes.insert(
            2,
            Node {
                id: 2,
                name: None,
                op: CsgOp::Cylinder {
                    radius: 3.0,
                    height: 20.0,
                    segments: 0,
                },
            },
        );
        doc.nodes.insert(
            3,
            Node {
                id: 3,
                name: None,
                op: CsgOp::Difference { left: 1, right: 2 },
            },
        );
        doc.roots[0].root = 3;

        let scene = evaluate_document(&doc, &EvalOptions::default()).unwrap();
        assert_eq!(scene.parts.len(), 1);
        assert!(!scene.parts[0].mesh.positions.is_empty());
    }

    #[test]
    fn evaluate_sketch_extrude() {
        let mut doc = Document::new();
        doc.nodes.insert(
            1,
            Node {
                id: 1,
                name: Some("rect".to_string()),
                op: CsgOp::Sketch2D {
                    origin: Vec3::new(0.0, 0.0, 0.0),
                    x_dir: Vec3::new(1.0, 0.0, 0.0),
                    y_dir: Vec3::new(0.0, 1.0, 0.0),
                    segments: vec![
                        SketchSegment2D::Line {
                            start: Vec2::new(0.0, 0.0),
                            end: Vec2::new(10.0, 0.0),
                        },
                        SketchSegment2D::Line {
                            start: Vec2::new(10.0, 0.0),
                            end: Vec2::new(10.0, 5.0),
                        },
                        SketchSegment2D::Line {
                            start: Vec2::new(10.0, 5.0),
                            end: Vec2::new(0.0, 5.0),
                        },
                        SketchSegment2D::Line {
                            start: Vec2::new(0.0, 5.0),
                            end: Vec2::new(0.0, 0.0),
                        },
                    ],
                },
            },
        );
        doc.nodes.insert(
            2,
            Node {
                id: 2,
                name: Some("extrude".to_string()),
                op: CsgOp::Extrude {
                    sketch: 1,
                    direction: Vec3::new(0.0, 0.0, 20.0),
                    twist_angle: None,
                    scale_end: None,
                },
            },
        );
        doc.roots.push(SceneEntry {
            root: 2,
            material: "default".to_string(),
            visible: None,
        });

        let scene = evaluate_document(&doc, &EvalOptions::default()).unwrap();
        assert_eq!(scene.parts.len(), 1);
        assert!(!scene.parts[0].mesh.positions.is_empty());
    }

    #[test]
    fn evaluate_sketch_revolve() {
        let mut doc = Document::new();
        // L-shaped profile for revolve
        doc.nodes.insert(
            1,
            Node {
                id: 1,
                name: None,
                op: CsgOp::Sketch2D {
                    origin: Vec3::new(0.0, 0.0, 0.0),
                    x_dir: Vec3::new(1.0, 0.0, 0.0),
                    y_dir: Vec3::new(0.0, 1.0, 0.0),
                    segments: vec![
                        SketchSegment2D::Line {
                            start: Vec2::new(5.0, 0.0),
                            end: Vec2::new(10.0, 0.0),
                        },
                        SketchSegment2D::Line {
                            start: Vec2::new(10.0, 0.0),
                            end: Vec2::new(10.0, 2.0),
                        },
                        SketchSegment2D::Line {
                            start: Vec2::new(10.0, 2.0),
                            end: Vec2::new(5.0, 2.0),
                        },
                        SketchSegment2D::Line {
                            start: Vec2::new(5.0, 2.0),
                            end: Vec2::new(5.0, 0.0),
                        },
                    ],
                },
            },
        );
        doc.nodes.insert(
            2,
            Node {
                id: 2,
                name: None,
                op: CsgOp::Revolve {
                    sketch: 1,
                    axis_origin: Vec3::new(0.0, 0.0, 0.0),
                    axis_dir: Vec3::new(0.0, 1.0, 0.0),
                    angle_deg: 360.0,
                },
            },
        );
        doc.roots.push(SceneEntry {
            root: 2,
            material: "default".to_string(),
            visible: None,
        });

        let scene = evaluate_document(&doc, &EvalOptions::default()).unwrap();
        assert_eq!(scene.parts.len(), 1);
        assert!(!scene.parts[0].mesh.positions.is_empty());
    }

    #[test]
    fn evaluate_hidden_parts_skipped() {
        let mut doc = make_cube_doc(10.0, 10.0, 10.0);
        doc.roots[0].visible = Some(false);

        let scene = evaluate_document(&doc, &EvalOptions::default()).unwrap();
        assert_eq!(scene.parts.len(), 0);
    }

    #[test]
    fn evaluate_assembly() {
        let mut doc = Document::new();
        doc.nodes.insert(
            1,
            Node {
                id: 1,
                name: None,
                op: CsgOp::Cube {
                    size: Vec3::new(10.0, 10.0, 10.0),
                },
            },
        );

        let mut part_defs = HashMap::new();
        part_defs.insert(
            "base".to_string(),
            PartDef {
                id: "base".to_string(),
                name: Some("Base".to_string()),
                root: 1,
                default_material: Some("steel".to_string()),
            },
        );
        doc.part_defs = Some(part_defs);
        doc.instances = Some(vec![Instance {
            id: "inst1".to_string(),
            part_def_id: "base".to_string(),
            name: None,
            transform: None,
            material: None,
        }]);
        doc.ground_instance_id = Some("inst1".to_string());

        let scene = evaluate_document(&doc, &EvalOptions::default()).unwrap();
        assert!(scene.part_defs.is_some());
        assert_eq!(scene.part_defs.as_ref().unwrap().len(), 1);
        assert!(scene.instances.is_some());
        assert_eq!(scene.instances.as_ref().unwrap().len(), 1);
        assert_eq!(scene.instances.as_ref().unwrap()[0].material, "steel");
    }

    #[test]
    fn evaluate_imported_mesh() {
        let mut doc = Document::new();
        doc.nodes.insert(
            1,
            Node {
                id: 1,
                name: None,
                op: CsgOp::ImportedMesh {
                    positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
                    indices: vec![0, 1, 2],
                    normals: Some(vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]),
                    source: Some("test.stl".to_string()),
                },
            },
        );
        doc.roots.push(SceneEntry {
            root: 1,
            material: "default".to_string(),
            visible: None,
        });

        let scene = evaluate_document(&doc, &EvalOptions::default()).unwrap();
        assert_eq!(scene.parts.len(), 1);
        assert_eq!(scene.parts[0].mesh.positions.len(), 9);
        assert_eq!(scene.parts[0].mesh.indices.len(), 3);
    }
}
