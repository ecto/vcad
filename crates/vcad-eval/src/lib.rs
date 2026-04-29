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
pub mod resolve;
pub mod validate;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use vcad_ir::Transform3D;
use vcad_kernel::Solid;

// Re-export main entry points
pub use evaluate::{evaluate_document, evaluate_node};
pub use kinematics::solve_forward_kinematics;
pub use resolve::{resolve_document, resolve_document_cloned, ResolvePatchError};
pub use validate::validate_document;

/// Platform-agnostic clock for timing instrumentation.
///
/// Implement this trait to provide millisecond-precision timing.
/// In WASM, use `performance.now()`; in native, use `std::time::Instant`.
pub trait Clock: Send + Sync {
    /// Returns the current time in milliseconds (monotonic).
    fn now_ms(&self) -> f64;
}

/// Timing data for a single evaluated node.
#[derive(Debug, Clone, Serialize)]
pub struct NodeTiming {
    /// Operation name (e.g. "Sweep", "Union").
    pub op: String,
    /// Time spent in the kernel operation (ms).
    pub eval_ms: f64,
    /// Time spent tessellating this node (ms).
    pub mesh_ms: f64,
}

/// Timing breakdown for a full document evaluation.
#[derive(Debug, Clone, Serialize)]
pub struct EvalTiming {
    /// Total evaluation time (ms).
    pub total_ms: f64,
    /// JSON parse time at the WASM boundary (ms). Only set in WASM.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_ms: Option<f64>,
    /// serde_wasm_bindgen serialization time (ms). Only set in WASM.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serialize_ms: Option<f64>,
    /// Total tessellation time across all nodes (ms).
    pub tessellate_ms: f64,
    /// Clash detection time (ms).
    pub clash_ms: f64,
    /// Assembly evaluation time (ms).
    pub assembly_ms: f64,
    /// Per-node timing keyed by NodeId (as string for JS object compatibility).
    pub nodes: HashMap<String, NodeTiming>,
}

/// Options for document evaluation.
#[derive(Default)]
pub struct EvalOptions {
    /// Skip O(n^2) clash detection for faster parametric editing.
    pub skip_clash_detection: bool,
    /// Optional clock for timing instrumentation. When `None`, timing is zero-cost.
    pub clock: Option<Box<dyn Clock>>,
}

/// Errors that can occur during evaluation.
#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    /// A node referenced by ID was not found in the document.
    #[error("missing node: {0}")]
    MissingNode(vcad_ir::NodeId),

    /// A node referenced by ID was not found in the document; includes the
    /// dotted path that led to the dangling reference (e.g.
    /// `nodes[47].Translate.child`).
    #[error("missing node {node_id} (referenced from {path})")]
    MissingNodeAt {
        /// The missing node id.
        node_id: vcad_ir::NodeId,
        /// Dotted path describing where the reference came from.
        path: String,
    },

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

    /// Parameter / binding resolution failed before kernel evaluation.
    #[error("parameter resolution failed: {0}")]
    ResolveBindings(String),
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
    /// Optional per-triangle face-kind tag (one byte per `indices / 3`).
    /// Used by the viewport click-to-inspect debugger to identify
    /// which BRep face (or synthetic fan-fill) contributed each
    /// triangle. Values: 0=Unknown, 1=Plane, 2=Cylinder, 3=Sphere,
    /// 4=Cone, 5=Bilinear, 6=Torus, 7=BSpline, 8=FanFill.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub face_kinds: Option<Vec<u8>>,
}

impl EvaluatedMesh {
    /// Create an empty mesh.
    pub fn empty() -> Self {
        Self {
            positions: Vec::new(),
            indices: Vec::new(),
            normals: None,
            face_kinds: None,
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

/// A single feature that failed to evaluate.
///
/// Evaluation is per-root: one bad feature yields an empty mesh + a
/// `RootFailure` entry rather than aborting the whole scene.
#[derive(Debug, Clone, Serialize)]
pub struct RootFailure {
    /// Where the failure happened: `"root[<idx>]"` for a scene root,
    /// `"partDef[<id>]"` for an assembly part definition.
    pub scope: String,
    /// The root node id we tried to evaluate.
    pub node_id: vcad_ir::NodeId,
    /// Human-readable error message.
    pub error: String,
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
    /// Per-root evaluation failures. Empty on a fully successful eval.
    pub failures: Vec<RootFailure>,
    /// Timing breakdown (populated when a `Clock` is provided in `EvalOptions`).
    pub timing: Option<EvalTiming>,
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
        assert!(scene.failures.is_empty());
    }

    #[test]
    fn one_broken_root_does_not_blank_the_scene() {
        // Regression: before per-root resilience, one dangling NodeId
        // reference aborted evaluation of the entire document and every
        // feature rendered as nothing. Now the broken feature reports
        // a failure and its siblings still produce meshes.
        let mut doc = make_cube_doc(10.0, 10.0, 10.0);
        // Fillet referencing node 0 (which doesn't exist) — mirrors the
        // real bug where the materializer emitted NodeId(0) as a sentinel.
        doc.nodes.insert(
            99,
            Node {
                id: 99,
                name: None,
                op: CsgOp::Fillet {
                    child: 0,
                    radius: 1.0,
                },
            },
        );
        doc.roots.push(SceneEntry {
            root: 99,
            material: "default".to_string(),
            visible: None,
        });

        let scene = evaluate_document(&doc, &EvalOptions::default()).unwrap();
        assert_eq!(scene.parts.len(), 2, "both roots kept their slot");
        assert!(
            !scene.parts[0].mesh.positions.is_empty(),
            "good cube still renders"
        );
        assert!(
            scene.parts[1].mesh.positions.is_empty(),
            "broken fillet rendered as empty mesh"
        );
        assert_eq!(scene.failures.len(), 1);
        assert_eq!(scene.failures[0].scope, "root[1]");
        assert_eq!(scene.failures[0].node_id, 99);
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
            tags: Vec::new(),
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
    fn parametric_bike_skeleton_resolves_and_evaluates() {
        // A miniature bike-like document: two wheels and a frame tube, all
        // driven off named parameters. Changing `wheelbase` at resolve time
        // shifts the rear wheel; the frame tube length derives from it.
        use vcad_ir::{BindingKey, Expr, Parameter};

        let mut doc = Document::new();
        doc.parameters
            .insert("wheelbase".into(), Parameter::literal(1000.0));
        doc.parameters
            .insert("wheel_radius".into(), Parameter::literal(350.0));
        doc.parameters.insert(
            "tube_thickness".into(),
            Parameter::derived("wheel_radius * 0.08"),
        );

        // Front wheel cylinder (static at origin)
        doc.nodes.insert(
            1,
            Node {
                id: 1,
                name: Some("front_wheel".into()),
                op: CsgOp::Cylinder {
                    radius: 0.0,
                    height: 40.0,
                    segments: 32,
                },
            },
        );
        // Rear wheel cylinder (translated by wheelbase)
        doc.nodes.insert(
            2,
            Node {
                id: 2,
                name: Some("rear_wheel".into()),
                op: CsgOp::Cylinder {
                    radius: 0.0,
                    height: 40.0,
                    segments: 32,
                },
            },
        );
        doc.nodes.insert(
            3,
            Node {
                id: 3,
                name: Some("rear_wheel_placed".into()),
                op: CsgOp::Translate {
                    child: 2,
                    offset: Vec3::new(0.0, 0.0, 0.0),
                },
            },
        );
        // Frame tube (cube sized to wheelbase)
        doc.nodes.insert(
            4,
            Node {
                id: 4,
                name: Some("top_tube".into()),
                op: CsgOp::Cube {
                    size: Vec3::new(0.0, 0.0, 0.0),
                },
            },
        );

        doc.bindings
            .bind(BindingKey::new(1, "radius"), Expr::formula("wheel_radius"));
        doc.bindings
            .bind(BindingKey::new(2, "radius"), Expr::formula("wheel_radius"));
        doc.bindings
            .bind(BindingKey::new(3, "offset.x"), Expr::formula("wheelbase"));
        doc.bindings.bind(
            BindingKey::new(4, "size.x"),
            Expr::formula("wheelbase * 0.7"),
        );
        doc.bindings.bind(
            BindingKey::new(4, "size.y"),
            Expr::formula("tube_thickness"),
        );
        doc.bindings.bind(
            BindingKey::new(4, "size.z"),
            Expr::formula("tube_thickness"),
        );

        for (root, mat) in [(1, "default"), (3, "default"), (4, "default")] {
            doc.roots.push(SceneEntry {
                root,
                material: mat.into(),
                visible: None,
            });
        }
        doc.materials.insert(
            "default".into(),
            MaterialDef {
                name: "default".into(),
                color: [0.8, 0.8, 0.8],
                metallic: 0.0,
                roughness: 0.5,
                density: None,
                friction: None,
            },
        );

        // Resolve and peek at concrete numbers.
        let (resolved, env) = crate::resolve_document_cloned(&doc).unwrap();
        assert_eq!(env["wheelbase"], 1000.0);
        assert_eq!(env["wheel_radius"], 350.0);
        assert_eq!(env["tube_thickness"], 28.0);
        match &resolved.nodes[&4].op {
            CsgOp::Cube { size } => {
                assert_eq!(size.x, 700.0);
                assert_eq!(size.y, 28.0);
                assert_eq!(size.z, 28.0);
            }
            _ => panic!(),
        }
        match &resolved.nodes[&3].op {
            CsgOp::Translate { offset, .. } => assert_eq!(offset.x, 1000.0),
            _ => panic!(),
        }

        // Evaluating via evaluate_document picks up the pre-pass and produces
        // non-empty geometry for all three parts.
        let scene = evaluate_document(&doc, &EvalOptions::default()).unwrap();
        assert_eq!(scene.parts.len(), 3);
        for p in &scene.parts {
            assert!(!p.mesh.positions.is_empty(), "every part tessellates");
        }
        assert!(scene.failures.is_empty());

        // Changing a parameter reshapes the document.
        let mut doc2 = doc.clone();
        doc2.parameters
            .insert("wheelbase".into(), Parameter::literal(1400.0));
        let (resolved2, _) = crate::resolve_document_cloned(&doc2).unwrap();
        match &resolved2.nodes[&4].op {
            CsgOp::Cube { size } => assert!((size.x - 980.0).abs() < 1e-9),
            _ => panic!(),
        }
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
