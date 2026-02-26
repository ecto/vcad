//! URDF file writer: converts vcad Document to URDF XML.

use std::io::Write;
use std::path::Path;

use vcad_ir::{CsgOp, Document, JointKind};

use crate::error::UrdfError;
use crate::types::{
    Axis, BoxGeom, ChildLink, Color, CylinderGeom, Geometry, Joint, Limit, Link, Material,
    MaterialRef, MeshGeom, Origin, ParentLink, Robot, SphereGeom, Visual,
};

/// Write a vcad Document to a URDF file.
///
/// # Arguments
///
/// * `doc` - The vcad Document to export
/// * `path` - Output file path
///
/// # Returns
///
/// `Ok(())` on success, or an error.
pub fn write_urdf(doc: &Document, path: impl AsRef<Path>) -> Result<(), UrdfError> {
    let xml = write_urdf_to_string(doc)?;
    let mut file = std::fs::File::create(path)?;
    file.write_all(xml.as_bytes())?;
    Ok(())
}

/// Convert a vcad Document to URDF XML string.
///
/// # Arguments
///
/// * `doc` - The vcad Document to export
///
/// # Returns
///
/// The URDF XML as a string.
pub fn write_urdf_to_string(doc: &Document) -> Result<String, UrdfError> {
    let writer = UrdfWriter::new(doc);
    writer.into_urdf_string()
}

/// Context for writing vcad Document to URDF.
struct UrdfWriter<'a> {
    doc: &'a Document,
}

impl<'a> UrdfWriter<'a> {
    fn new(doc: &'a Document) -> Self {
        Self { doc }
    }

    fn into_urdf_string(self) -> Result<String, UrdfError> {
        let robot = self.to_robot()?;

        // Serialize to XML
        let xml = quick_xml::se::to_string(&robot)?;

        // Add XML declaration
        Ok(format!("<?xml version=\"1.0\"?>\n{}", xml))
    }

    fn to_robot(&self) -> Result<Robot, UrdfError> {
        let mut links = Vec::new();
        let mut joints = Vec::new();
        let mut materials = Vec::new();

        // Export materials
        for (name, mat_def) in &self.doc.materials {
            materials.push(Material {
                name: name.clone(),
                color: Some(Color {
                    rgba: format!(
                        "{} {} {} 1.0",
                        mat_def.color[0], mat_def.color[1], mat_def.color[2]
                    ),
                }),
                texture: None,
            });
        }

        // Export part definitions as links
        if let Some(part_defs) = &self.doc.part_defs {
            for part_def in part_defs.values() {
                let link = self.part_def_to_link(part_def)?;
                links.push(link);
            }
        } else {
            // No assembly mode - export scene entries as links
            for (i, entry) in self.doc.roots.iter().enumerate() {
                let link = self.scene_entry_to_link(entry, i)?;
                links.push(link);
            }
        }

        // Export joints
        if let Some(vcad_joints) = &self.doc.joints {
            for joint in vcad_joints {
                let urdf_joint = self.vcad_joint_to_urdf(joint)?;
                joints.push(urdf_joint);
            }
        }

        // Determine robot name
        let name = if let Some(instances) = &self.doc.instances {
            if let Some(first) = instances.first() {
                first.name.clone().unwrap_or_else(|| "robot".to_string())
            } else {
                "robot".to_string()
            }
        } else {
            "robot".to_string()
        };

        Ok(Robot {
            name,
            links,
            joints,
            materials,
        })
    }

    fn part_def_to_link(&self, part_def: &vcad_ir::PartDef) -> Result<Link, UrdfError> {
        let name = part_def.name.clone().unwrap_or_else(|| part_def.id.clone());

        // Get geometry from root node
        let (geometry, origin) = self.node_to_geometry(part_def.root)?;

        let material_ref = part_def.default_material.as_ref().map(|m| MaterialRef {
            name: Some(m.clone()),
            color: None,
            texture: None,
        });

        Ok(Link {
            name,
            visual: Some(Visual {
                name: None,
                origin,
                geometry,
                material: material_ref,
            }),
            collision: None,
            inertial: None,
        })
    }

    fn scene_entry_to_link(
        &self,
        entry: &vcad_ir::SceneEntry,
        index: usize,
    ) -> Result<Link, UrdfError> {
        let node = self
            .doc
            .nodes
            .get(&entry.root)
            .ok_or_else(|| UrdfError::Conversion(format!("Node {} not found", entry.root)))?;

        let name = node
            .name
            .clone()
            .unwrap_or_else(|| format!("link_{}", index));

        let (geometry, origin) = self.node_to_geometry(entry.root)?;

        let material_ref = Some(MaterialRef {
            name: Some(entry.material.clone()),
            color: None,
            texture: None,
        });

        Ok(Link {
            name,
            visual: Some(Visual {
                name: None,
                origin,
                geometry,
                material: material_ref,
            }),
            collision: None,
            inertial: None,
        })
    }

    fn node_to_geometry(
        &self,
        node_id: vcad_ir::NodeId,
    ) -> Result<(Geometry, Option<Origin>), UrdfError> {
        let node = self
            .doc
            .nodes
            .get(&node_id)
            .ok_or_else(|| UrdfError::Conversion(format!("Node {} not found", node_id)))?;

        match &node.op {
            CsgOp::Cube { size } => {
                // vcad uses mm, URDF uses meters
                let geometry = Geometry {
                    box_geom: Some(BoxGeom {
                        size: format!(
                            "{} {} {}",
                            size.x / 1000.0,
                            size.y / 1000.0,
                            size.z / 1000.0
                        ),
                    }),
                    cylinder: None,
                    sphere: None,
                    mesh: None,
                };
                Ok((geometry, None))
            }
            CsgOp::Cylinder { radius, height, .. } => {
                let geometry = Geometry {
                    box_geom: None,
                    cylinder: Some(CylinderGeom {
                        radius: radius / 1000.0,
                        length: height / 1000.0,
                    }),
                    sphere: None,
                    mesh: None,
                };
                Ok((geometry, None))
            }
            CsgOp::Sphere { radius, .. } => {
                let geometry = Geometry {
                    box_geom: None,
                    cylinder: None,
                    sphere: Some(SphereGeom {
                        radius: radius / 1000.0,
                    }),
                    mesh: None,
                };
                Ok((geometry, None))
            }
            CsgOp::Cone {
                radius_bottom,
                height,
                ..
            } => {
                // Approximate cone as cylinder (URDF doesn't have native cone)
                let geometry = Geometry {
                    box_geom: None,
                    cylinder: Some(CylinderGeom {
                        radius: radius_bottom / 1000.0,
                        length: height / 1000.0,
                    }),
                    sphere: None,
                    mesh: None,
                };
                Ok((geometry, None))
            }
            CsgOp::Translate { child, offset } => {
                let (geometry, _) = self.node_to_geometry(*child)?;
                let origin = Some(Origin {
                    xyz: Some(format!(
                        "{} {} {}",
                        offset.x / 1000.0,
                        offset.y / 1000.0,
                        offset.z / 1000.0
                    )),
                    rpy: None,
                });
                Ok((geometry, origin))
            }
            CsgOp::Rotate { child, angles } => {
                let (geometry, existing_origin) = self.node_to_geometry(*child)?;
                let rpy = format!(
                    "{} {} {}",
                    angles.x.to_radians(),
                    angles.y.to_radians(),
                    angles.z.to_radians()
                );
                let origin = Some(Origin {
                    xyz: existing_origin.as_ref().and_then(|o| o.xyz.clone()),
                    rpy: Some(rpy),
                });
                Ok((geometry, origin))
            }
            CsgOp::StepImport { path } => {
                // Export as mesh reference
                let geometry = Geometry {
                    box_geom: None,
                    cylinder: None,
                    sphere: None,
                    mesh: Some(MeshGeom {
                        filename: path.clone(),
                        scale: None,
                    }),
                };
                Ok((geometry, None))
            }
            CsgOp::Union { left, .. }
            | CsgOp::Difference { left, .. }
            | CsgOp::Intersection { left, .. } => {
                // For boolean ops, just export left operand (simplification)
                self.node_to_geometry(*left)
            }
            CsgOp::Scale { child, factor } => {
                let (mut geometry, origin) = self.node_to_geometry(*child)?;
                // Apply scale to geometry if mesh
                if let Some(ref mut mesh) = geometry.mesh {
                    mesh.scale = Some(format!("{} {} {}", factor.x, factor.y, factor.z));
                }
                Ok((geometry, origin))
            }
            CsgOp::Empty => {
                // Empty geometry - create tiny placeholder
                let geometry = Geometry {
                    box_geom: Some(BoxGeom {
                        size: "0.001 0.001 0.001".to_string(),
                    }),
                    cylinder: None,
                    sphere: None,
                    mesh: None,
                };
                Ok((geometry, None))
            }
            CsgOp::LinearPattern { child, .. }
            | CsgOp::CircularPattern { child, .. }
            | CsgOp::Shell { child, .. }
            | CsgOp::Fillet { child, .. }
            | CsgOp::Chamfer { child, .. } => {
                // For patterns/shell/fillet/chamfer, export base geometry
                self.node_to_geometry(*child)
            }
            CsgOp::Sketch2D { .. }
            | CsgOp::Text2D { .. }
            | CsgOp::Extrude { .. }
            | CsgOp::Revolve { .. }
            | CsgOp::Sweep { .. }
            | CsgOp::Loft { .. }
            | CsgOp::ImportedMesh { .. }
            | CsgOp::PcbBoard { .. }
            | CsgOp::EmbroideryPattern { .. } => {
                // Sketch-based / imported / PCB / embroidery geometry - cannot export to URDF directly
                Err(UrdfError::Conversion(
                    "Sketch-based geometry cannot be exported to URDF directly".to_string(),
                ))
            }
        }
    }

    fn vcad_joint_to_urdf(&self, joint: &vcad_ir::Joint) -> Result<Joint, UrdfError> {
        // Get parent link name from instance
        let parent_link = if let Some(parent_id) = &joint.parent_instance_id {
            self.instance_to_link_name(parent_id)?
        } else {
            "world".to_string()
        };

        let child_link = self.instance_to_link_name(&joint.child_instance_id)?;

        // Convert joint type
        let (joint_type, axis, limit) = match &joint.kind {
            JointKind::Fixed => ("fixed".to_string(), None, None),
            JointKind::Revolute { axis, limits } => {
                let axis_str = Some(Axis {
                    xyz: format!("{} {} {}", axis.x, axis.y, axis.z),
                });
                let limit = limits.map(|(lower, upper)| Limit {
                    lower: Some(lower.to_radians()),
                    upper: Some(upper.to_radians()),
                    effort: Some(100.0), // Default effort
                    velocity: Some(1.0), // Default velocity
                });
                ("revolute".to_string(), axis_str, limit)
            }
            JointKind::Slider { axis, limits } => {
                let axis_str = Some(Axis {
                    xyz: format!("{} {} {}", axis.x, axis.y, axis.z),
                });
                let limit = limits.map(|(lower, upper)| Limit {
                    lower: Some(lower / 1000.0), // mm to meters
                    upper: Some(upper / 1000.0),
                    effort: Some(100.0),
                    velocity: Some(0.5),
                });
                ("prismatic".to_string(), axis_str, limit)
            }
            JointKind::Cylindrical { axis } => {
                // Approximate as revolute (URDF doesn't have cylindrical)
                let axis_str = Some(Axis {
                    xyz: format!("{} {} {}", axis.x, axis.y, axis.z),
                });
                ("continuous".to_string(), axis_str, None)
            }
            JointKind::Ball => {
                // Approximate as floating
                ("floating".to_string(), None, None)
            }
        };

        // Convert anchor to origin
        let origin = Some(Origin {
            xyz: Some(format!(
                "{} {} {}",
                joint.parent_anchor.x / 1000.0,
                joint.parent_anchor.y / 1000.0,
                joint.parent_anchor.z / 1000.0
            )),
            rpy: None,
        });

        Ok(Joint {
            name: joint.id.clone(),
            joint_type,
            origin,
            parent: ParentLink { link: parent_link },
            child: ChildLink { link: child_link },
            axis,
            limit,
            dynamics: None,
        })
    }

    fn instance_to_link_name(&self, instance_id: &str) -> Result<String, UrdfError> {
        if let Some(instances) = &self.doc.instances {
            for instance in instances {
                if instance.id == instance_id {
                    return Ok(instance
                        .name
                        .clone()
                        .unwrap_or_else(|| instance_id.to_string()));
                }
            }
        }
        // Fall back to instance ID
        Ok(instance_id.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::read_urdf_from_str;

    #[test]
    fn test_roundtrip_simple() {
        let original_urdf = r#"<?xml version="1.0"?>
<robot name="simple">
    <link name="base">
        <visual>
            <geometry>
                <box size="0.1 0.2 0.3"/>
            </geometry>
        </visual>
    </link>
    <link name="arm">
        <visual>
            <geometry>
                <cylinder radius="0.05" length="0.2"/>
            </geometry>
        </visual>
    </link>
    <joint name="base_arm" type="revolute">
        <parent link="base"/>
        <child link="arm"/>
        <origin xyz="0 0 0.15"/>
        <axis xyz="0 0 1"/>
        <limit lower="-1.57" upper="1.57" effort="10" velocity="1"/>
    </joint>
</robot>"#;

        // Parse original
        let doc = read_urdf_from_str(original_urdf).unwrap();

        // Write back to URDF
        let output_urdf = write_urdf_to_string(&doc).unwrap();

        // Parse output
        let doc2 = read_urdf_from_str(&output_urdf).unwrap();

        // Verify structure is preserved
        assert_eq!(
            doc.part_defs.as_ref().map(|p| p.len()),
            doc2.part_defs.as_ref().map(|p| p.len())
        );
        assert_eq!(
            doc.joints.as_ref().map(|j| j.len()),
            doc2.joints.as_ref().map(|j| j.len())
        );
    }

    #[test]
    fn test_write_box_dimensions() {
        let mut doc = Document::new();

        // Add a cube node (100mm x 200mm x 300mm)
        doc.nodes.insert(
            1,
            vcad_ir::Node {
                id: 1,
                name: Some("test_box".to_string()),
                op: CsgOp::Cube {
                    size: vcad_ir::Vec3::new(100.0, 200.0, 300.0),
                },
            },
        );

        doc.roots.push(vcad_ir::SceneEntry {
            root: 1,
            material: "default".to_string(),
            visible: None,
        });

        let urdf = write_urdf_to_string(&doc).unwrap();

        // Check that dimensions are in meters (0.1, 0.2, 0.3)
        assert!(urdf.contains("0.1 0.2 0.3") || urdf.contains("size=\"0.1 0.2 0.3\""));
    }
}
