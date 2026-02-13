//! Document evaluation — DAG traversal producing triangle meshes.

use anyhow::Result;
use vcad_ir::{CsgOp, Document, NodeId};
use vcad_kernel::Solid;

/// A single evaluated mesh with its source node ID.
#[derive(Debug, Clone)]
pub struct EvaluatedMesh {
    /// The node that produced this mesh.
    pub node_id: NodeId,
    /// Interleaved xyz vertex positions.
    pub vertices: Vec<f32>,
    /// Triangle indices.
    pub indices: Vec<u32>,
}

/// The complete evaluated scene.
#[derive(Debug, Clone)]
pub struct EvaluatedScene {
    /// One mesh per visible root.
    pub meshes: Vec<EvaluatedMesh>,
}

impl EvaluatedScene {
    /// Total triangle count across all meshes.
    pub fn triangle_count(&self) -> usize {
        self.meshes.iter().map(|m| m.indices.len() / 3).sum()
    }

    /// Total vertex count across all meshes.
    pub fn vertex_count(&self) -> usize {
        self.meshes.iter().map(|m| m.vertices.len() / 3).sum()
    }
}

/// Evaluate a document into an EvaluatedScene.
pub fn evaluate_document(doc: &Document) -> Result<EvaluatedScene> {
    let mut meshes = Vec::new();

    for entry in &doc.roots {
        if let Some(visible) = entry.visible {
            if !visible {
                continue;
            }
        }
        if let Some(solid) = evaluate_node(doc, entry.root)? {
            let mesh = solid.to_mesh(32);
            meshes.push(EvaluatedMesh {
                node_id: entry.root,
                vertices: mesh.vertices,
                indices: mesh.indices,
            });
        }
    }

    Ok(EvaluatedScene { meshes })
}

/// Recursively evaluate a node to a Solid.
fn evaluate_node(doc: &Document, node_id: NodeId) -> Result<Option<Solid>> {
    let node = doc
        .nodes
        .get(&node_id)
        .ok_or_else(|| anyhow::anyhow!("Node {} not found", node_id))?;

    let solid = match &node.op {
        CsgOp::Empty => Some(Solid::empty()),
        CsgOp::Cube { size } => Some(Solid::cube(size.x, size.y, size.z)),
        CsgOp::Cylinder {
            radius,
            height,
            segments,
        } => Some(Solid::cylinder(*radius, *height, *segments)),
        CsgOp::Sphere { radius, segments } => Some(Solid::sphere(*radius, *segments)),
        CsgOp::Cone {
            radius_bottom,
            radius_top,
            height,
            segments,
        } => Some(Solid::cone(*radius_bottom, *radius_top, *height, *segments)),
        CsgOp::Union { left, right } => {
            let l = evaluate_node(doc, *left)?;
            let r = evaluate_node(doc, *right)?;
            match (l, r) {
                (Some(l), Some(r)) => Some(l.union(&r)),
                (Some(l), None) => Some(l),
                (None, Some(r)) => Some(r),
                (None, None) => None,
            }
        }
        CsgOp::Difference { left, right } => {
            let l = evaluate_node(doc, *left)?;
            let r = evaluate_node(doc, *right)?;
            match (l, r) {
                (Some(l), Some(r)) => Some(l.difference(&r)),
                (Some(l), None) => Some(l),
                _ => None,
            }
        }
        CsgOp::Intersection { left, right } => {
            let l = evaluate_node(doc, *left)?;
            let r = evaluate_node(doc, *right)?;
            match (l, r) {
                (Some(l), Some(r)) => Some(l.intersection(&r)),
                _ => None,
            }
        }
        CsgOp::Translate { child, offset } => {
            let c = evaluate_node(doc, *child)?;
            c.map(|s| s.translate(offset.x, offset.y, offset.z))
        }
        CsgOp::Rotate { child, angles } => {
            let c = evaluate_node(doc, *child)?;
            c.map(|s| s.rotate(angles.x, angles.y, angles.z))
        }
        CsgOp::Scale { child, factor } => {
            let c = evaluate_node(doc, *child)?;
            c.map(|s| s.scale(factor.x, factor.y, factor.z))
        }
        CsgOp::LinearPattern {
            child,
            direction,
            count,
            spacing,
        } => {
            let c = evaluate_node(doc, *child)?;
            c.map(|s| {
                s.linear_pattern(
                    vcad_kernel_math::Vec3::new(direction.x, direction.y, direction.z),
                    *count,
                    *spacing,
                )
            })
        }
        CsgOp::CircularPattern {
            child,
            axis_origin,
            axis_dir,
            count,
            angle_deg,
        } => {
            let c = evaluate_node(doc, *child)?;
            c.map(|s| {
                s.circular_pattern(
                    vcad_kernel_math::Point3::new(
                        axis_origin.x,
                        axis_origin.y,
                        axis_origin.z,
                    ),
                    vcad_kernel_math::Vec3::new(axis_dir.x, axis_dir.y, axis_dir.z),
                    *count,
                    *angle_deg,
                )
            })
        }
        CsgOp::Shell { child, thickness } => {
            let c = evaluate_node(doc, *child)?;
            c.map(|s| s.shell(*thickness))
        }
        CsgOp::Fillet { child, radius } => {
            let c = evaluate_node(doc, *child)?;
            c.map(|s| s.fillet(*radius))
        }
        CsgOp::Chamfer { child, distance } => {
            let c = evaluate_node(doc, *child)?;
            c.map(|s| s.chamfer(*distance))
        }
        CsgOp::StepImport { path } => Solid::from_step(path).ok(),
        // These need further processing to become solids
        CsgOp::Sketch2D { .. }
        | CsgOp::Extrude { .. }
        | CsgOp::Revolve { .. }
        | CsgOp::Text2D { .. } => None,
    };

    Ok(solid)
}
