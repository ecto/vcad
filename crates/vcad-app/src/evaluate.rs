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
        CsgOp::Torus {
            major_radius,
            minor_radius,
            segments,
        } => Some(Solid::torus(*major_radius, *minor_radius, *segments)),
        CsgOp::Wedge { size } => Some(Solid::wedge(size.x, size.y, size.z)),
        CsgOp::Prism {
            sides,
            radius,
            height,
        } => Some(Solid::prism(*sides, *radius, *height)),
        CsgOp::Mirror {
            child,
            plane_origin,
            plane_normal,
        } => {
            let c = evaluate_node(doc, *child)?;
            c.map(|s| {
                s.mirror(
                    [plane_origin.x, plane_origin.y, plane_origin.z],
                    [plane_normal.x, plane_normal.y, plane_normal.z],
                )
            })
        }
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
                    vcad_kernel_math::Point3::new(axis_origin.x, axis_origin.y, axis_origin.z),
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
        CsgOp::EdgeBlend {
            child,
            edges,
            profile,
        } => {
            let c = evaluate_node(doc, *child)?;
            let (query, keys) = kernel_blend_args(edges, profile);
            c.map(|s| s.edge_blend(&query, &keys))
        }
        CsgOp::StepImport { path } => Solid::from_step(path).ok(),
        // STL meshes feed the physics path directly; the editor doesn't yet
        // build a BRep from a triangle soup.
        CsgOp::MeshImport { .. } => None,
        // These need further processing to become solids
        CsgOp::Sketch2D { .. }
        | CsgOp::Extrude { .. }
        | CsgOp::Revolve { .. }
        | CsgOp::Text2D { .. }
        | CsgOp::Sweep { .. }
        | CsgOp::Loft { .. }
        | CsgOp::ImportedMesh { .. }
        | CsgOp::PcbBoard { .. }
        | CsgOp::EmbroideryPattern { .. }
        | CsgOp::PartInstance { .. }
        | CsgOp::SheetMetalBaseFlangeRect { .. }
        | CsgOp::SheetMetalBaseFlangePolygon { .. }
        | CsgOp::SheetMetalEdgeFlange { .. }
        | CsgOp::SheetMetalHem { .. }
        | CsgOp::SheetMetalJog { .. }
        | CsgOp::SheetMetalBendRelief { .. } => None,
    };

    Ok(solid)
}

/// Convert IR edge-blend arguments to their kernel equivalents.
fn kernel_blend_args(
    edges: &vcad_ir::EdgeQuery,
    profile: &vcad_ir::BlendProfile,
) -> (
    vcad_kernel::vcad_kernel_fillet::EdgeQuery,
    Vec<vcad_kernel::vcad_kernel_fillet::BlendKey>,
) {
    use vcad_kernel::vcad_kernel_fillet as kf;
    let q = match edges {
        vcad_ir::EdgeQuery::All => kf::EdgeQuery::All,
        vcad_ir::EdgeQuery::Near { point } => kf::EdgeQuery::Near {
            point: vcad_kernel_math::Point3::new(point.x, point.y, point.z),
        },
        vcad_ir::EdgeQuery::Direction { axis, tol_deg } => kf::EdgeQuery::Direction {
            axis: vcad_kernel_math::Vec3::new(axis.x, axis.y, axis.z),
            tol_deg: *tol_deg,
        },
    };
    let keys = match profile {
        vcad_ir::BlendProfile::Constant { size, shape } => vec![kf::BlendKey {
            t: 0.0,
            section: kf::BlendSection {
                size: *size,
                shape: *shape,
            },
        }],
        vcad_ir::BlendProfile::Keyed { keys } => keys
            .iter()
            .map(|k| kf::BlendKey {
                t: k.t,
                section: kf::BlendSection {
                    size: k.size,
                    shape: k.shape,
                },
            })
            .collect(),
    };
    (q, keys)
}
