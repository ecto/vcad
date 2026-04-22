//! URDF file reader: converts URDF XML to vcad Document.

use std::collections::HashMap;
use std::path::Path;

use vcad_ir::{
    CsgOp, Document, Instance, Joint as VcadJoint, JointKind, MaterialDef, Node, NodeId, PartDef,
    SceneEntry, Vec3,
};

use crate::error::UrdfError;
use crate::types::{Geometry, Joint, Link, Robot};

/// Maximum URDF size we will attempt to parse. URDFs are tiny descriptors
/// (tens of KB); a multi-MB "URDF" is almost always an attack payload.
const MAX_URDF_BYTES: usize = 8 * 1024 * 1024;
/// Caps on structural count to bound post-parse work.
const MAX_LINKS: usize = 10_000;
const MAX_JOINTS: usize = 10_000;

/// Read a URDF file from a path.
///
/// # Arguments
///
/// * `path` - Path to the URDF file
///
/// # Returns
///
/// A vcad Document representing the robot.
pub fn read_urdf(path: impl AsRef<Path>) -> Result<Document, UrdfError> {
    let content = std::fs::read_to_string(path)?;
    read_urdf_from_str(&content)
}

/// Read a URDF from a string.
///
/// # Arguments
///
/// * `xml` - URDF XML content as string
///
/// # Returns
///
/// A vcad Document representing the robot.
pub fn read_urdf_from_str(xml: &str) -> Result<Document, UrdfError> {
    if xml.len() > MAX_URDF_BYTES {
        return Err(UrdfError::InvalidFormat(format!(
            "URDF exceeds {} byte limit",
            MAX_URDF_BYTES
        )));
    }
    // Reject DOCTYPE outright. quick-xml 0.37 does not expand entity
    // references, but a DOCTYPE is otherwise never needed in URDF and
    // rejecting it provides defense-in-depth against XXE / billion-laughs
    // if a future quick-xml release (or a different parser) ever starts
    // expanding them.
    if contains_doctype(xml) {
        return Err(UrdfError::InvalidFormat(
            "URDF contains a DOCTYPE declaration (rejected)".into(),
        ));
    }
    let robot: Robot = quick_xml::de::from_str(xml)?;
    if robot.links.len() > MAX_LINKS {
        return Err(UrdfError::InvalidFormat(format!(
            "URDF has {} links (cap {})",
            robot.links.len(),
            MAX_LINKS
        )));
    }
    if robot.joints.len() > MAX_JOINTS {
        return Err(UrdfError::InvalidFormat(format!(
            "URDF has {} joints (cap {})",
            robot.joints.len(),
            MAX_JOINTS
        )));
    }
    let reader = UrdfReader::new(&robot);
    reader.into_document()
}

/// Case-insensitive scan for `<!DOCTYPE` that ignores leading whitespace.
fn contains_doctype(xml: &str) -> bool {
    // Walk through comments/whitespace and check for a DOCTYPE at the prolog.
    // A stricter parse isn't worth the cost — false positives here just make
    // us reject a document that would have been rejected anyway.
    let bytes = xml.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'<' => {
                if bytes[i..].starts_with(b"<!--") {
                    if let Some(end) = xml[i..].find("-->") {
                        i += end + 3;
                        continue;
                    }
                    return false;
                }
                if bytes[i..].starts_with(b"<?") {
                    if let Some(end) = xml[i..].find("?>") {
                        i += end + 2;
                        continue;
                    }
                    return false;
                }
                if bytes[i..].len() >= 9
                    && bytes[i..i + 9].eq_ignore_ascii_case(b"<!DOCTYPE")
                {
                    return true;
                }
                return false;
            }
            _ => return false,
        }
    }
    false
}

/// Context for reading URDF and building vcad Document.
struct UrdfReader<'a> {
    robot: &'a Robot,
    /// Maps link name to vcad part def ID.
    link_to_part: HashMap<String, String>,
    /// Maps link name to instance ID.
    link_to_instance: HashMap<String, String>,
    /// Next node ID.
    next_node_id: NodeId,
}

impl<'a> UrdfReader<'a> {
    fn new(robot: &'a Robot) -> Self {
        Self {
            robot,
            link_to_part: HashMap::new(),
            link_to_instance: HashMap::new(),
            next_node_id: 1,
        }
    }

    fn alloc_node_id(&mut self) -> NodeId {
        let id = self.next_node_id;
        self.next_node_id += 1;
        id
    }

    fn into_document(mut self) -> Result<Document, UrdfError> {
        let mut doc = Document::new();

        // Build material map from top-level materials
        let material_map = self.build_materials(&mut doc);

        // Build part definitions from links
        let mut part_defs = HashMap::new();
        for link in &self.robot.links {
            let (part_def, nodes) = self.link_to_part_def(link, &material_map)?;
            let part_id = part_def.id.clone();
            self.link_to_part.insert(link.name.clone(), part_id.clone());
            part_defs.insert(part_id, part_def);
            for (id, node) in nodes {
                doc.nodes.insert(id, node);
            }
        }
        doc.part_defs = Some(part_defs);

        // Find root link (link that is not a child of any joint)
        let root_link = self.find_root_link()?;

        // Build instances for all links
        let mut instances = Vec::new();
        for link in &self.robot.links {
            let part_id = self.link_to_part.get(&link.name).unwrap().clone();
            let instance_id = format!("{}_inst", link.name);
            self.link_to_instance
                .insert(link.name.clone(), instance_id.clone());

            instances.push(Instance {
                id: instance_id,
                part_def_id: part_id,
                name: Some(link.name.clone()),
                transform: None, // Transforms come from joints
                material: None,
            });
        }
        doc.instances = Some(instances);

        // Set ground instance
        doc.ground_instance_id = self.link_to_instance.get(&root_link).cloned();

        // Build joints
        let mut vcad_joints = Vec::new();
        for joint in &self.robot.joints {
            let vcad_joint = self.joint_to_vcad(joint)?;
            vcad_joints.push(vcad_joint);
        }
        doc.joints = Some(vcad_joints);

        // Add scene entries for each link's root geometry
        for link in &self.robot.links {
            if let Some(part_defs) = &doc.part_defs {
                if let Some(part_def) = part_defs.get(&self.link_to_part[&link.name]) {
                    doc.roots.push(SceneEntry {
                        root: part_def.root,
                        material: "default".to_string(),
                        visible: None,
                    });
                }
            }
        }

        Ok(doc)
    }

    fn build_materials(&self, doc: &mut Document) -> HashMap<String, String> {
        let mut map = HashMap::new();

        // Add default material
        doc.materials.insert(
            "default".to_string(),
            MaterialDef {
                name: "default".to_string(),
                color: [0.7, 0.7, 0.7],
                metallic: 0.0,
                roughness: 0.5,
                density: None,
                friction: None,
            },
        );

        // Add materials from URDF
        for mat in &self.robot.materials {
            let color = mat
                .color
                .as_ref()
                .map(|c| {
                    let rgba = c.rgba_vec();
                    [rgba[0], rgba[1], rgba[2]]
                })
                .unwrap_or([0.5, 0.5, 0.5]);

            let mat_def = MaterialDef {
                name: mat.name.clone(),
                color,
                metallic: 0.0,
                roughness: 0.5,
                density: None,
                friction: None,
            };

            doc.materials.insert(mat.name.clone(), mat_def);
            map.insert(mat.name.clone(), mat.name.clone());
        }

        map
    }

    fn link_to_part_def(
        &mut self,
        link: &Link,
        _material_map: &HashMap<String, String>,
    ) -> Result<(PartDef, Vec<(NodeId, Node)>), UrdfError> {
        let mut nodes = Vec::new();

        // Get geometry from visual or collision
        let (geom, origin) = if let Some(visual) = &link.visual {
            (&visual.geometry, visual.origin.as_ref())
        } else if let Some(collision) = &link.collision {
            (&collision.geometry, collision.origin.as_ref())
        } else {
            // Link with no geometry - create empty cube placeholder
            let node_id = self.alloc_node_id();
            nodes.push((
                node_id,
                Node {
                    id: node_id,
                    name: Some(link.name.clone()),
                    op: CsgOp::Cube {
                        size: Vec3::new(0.01, 0.01, 0.01), // 1cm placeholder
                    },
                },
            ));
            return Ok((
                PartDef {
                    id: format!("part_{}", link.name),
                    name: Some(link.name.clone()),
                    root: node_id,
                    default_material: Some("default".to_string()),
                },
                nodes,
            ));
        };

        // Create geometry node
        let geom_node_id = self.alloc_node_id();
        let geom_op = self.geometry_to_csg(geom)?;
        nodes.push((
            geom_node_id,
            Node {
                id: geom_node_id,
                name: Some(format!("{}_geom", link.name)),
                op: geom_op,
            },
        ));

        // Apply origin transform if present
        let root_id = if let Some(origin) = origin {
            let xyz = origin.xyz_vec();
            let rpy = origin.rpy_vec();

            // URDF uses meters, vcad uses mm
            let xyz_mm = [xyz[0] * 1000.0, xyz[1] * 1000.0, xyz[2] * 1000.0];

            // URDF uses radians, vcad uses degrees
            let rpy_deg = [
                rpy[0].to_degrees(),
                rpy[1].to_degrees(),
                rpy[2].to_degrees(),
            ];

            let has_translation = xyz_mm.iter().any(|v| v.abs() > 1e-6);
            let has_rotation = rpy_deg.iter().any(|v| v.abs() > 1e-6);

            if has_rotation {
                let rotate_id = self.alloc_node_id();
                nodes.push((
                    rotate_id,
                    Node {
                        id: rotate_id,
                        name: Some(format!("{}_rotate", link.name)),
                        op: CsgOp::Rotate {
                            child: geom_node_id,
                            angles: Vec3::new(rpy_deg[0], rpy_deg[1], rpy_deg[2]),
                        },
                    },
                ));

                if has_translation {
                    let translate_id = self.alloc_node_id();
                    nodes.push((
                        translate_id,
                        Node {
                            id: translate_id,
                            name: Some(format!("{}_translate", link.name)),
                            op: CsgOp::Translate {
                                child: rotate_id,
                                offset: Vec3::new(xyz_mm[0], xyz_mm[1], xyz_mm[2]),
                            },
                        },
                    ));
                    translate_id
                } else {
                    rotate_id
                }
            } else if has_translation {
                let translate_id = self.alloc_node_id();
                nodes.push((
                    translate_id,
                    Node {
                        id: translate_id,
                        name: Some(format!("{}_translate", link.name)),
                        op: CsgOp::Translate {
                            child: geom_node_id,
                            offset: Vec3::new(xyz_mm[0], xyz_mm[1], xyz_mm[2]),
                        },
                    },
                ));
                translate_id
            } else {
                geom_node_id
            }
        } else {
            geom_node_id
        };

        Ok((
            PartDef {
                id: format!("part_{}", link.name),
                name: Some(link.name.clone()),
                root: root_id,
                default_material: Some("default".to_string()),
            },
            nodes,
        ))
    }

    fn geometry_to_csg(&self, geom: &Geometry) -> Result<CsgOp, UrdfError> {
        if let Some(box_geom) = &geom.box_geom {
            let size = box_geom.size_vec();
            // URDF uses meters, vcad uses mm
            Ok(CsgOp::Cube {
                size: Vec3::new(size[0] * 1000.0, size[1] * 1000.0, size[2] * 1000.0),
            })
        } else if let Some(cyl) = &geom.cylinder {
            // URDF cylinder is along Z axis, centered
            Ok(CsgOp::Cylinder {
                radius: cyl.radius * 1000.0,
                height: cyl.length * 1000.0,
                segments: 32,
            })
        } else if let Some(sphere) = &geom.sphere {
            Ok(CsgOp::Sphere {
                radius: sphere.radius * 1000.0,
                segments: 32,
            })
        } else if let Some(mesh) = &geom.mesh {
            // Mesh reference - store as STEP import for now (could be STL/DAE)
            // This is a simplification; full implementation would support mesh loading
            Ok(CsgOp::StepImport {
                path: mesh.filename.clone(),
            })
        } else {
            Err(UrdfError::InvalidGeometry(
                "No geometry type specified".to_string(),
            ))
        }
    }

    fn find_root_link(&self) -> Result<String, UrdfError> {
        // Root link is one that is never a child in any joint
        let child_links: std::collections::HashSet<_> =
            self.robot.joints.iter().map(|j| &j.child.link).collect();

        for link in &self.robot.links {
            if !child_links.contains(&link.name) {
                return Ok(link.name.clone());
            }
        }

        // If no root found, use first link
        self.robot
            .links
            .first()
            .map(|l| l.name.clone())
            .ok_or_else(|| UrdfError::MissingElement("No links found".to_string()))
    }

    fn joint_to_vcad(&self, joint: &Joint) -> Result<VcadJoint, UrdfError> {
        let parent_instance_id = self.link_to_instance.get(&joint.parent.link).cloned();
        let child_instance_id = self
            .link_to_instance
            .get(&joint.child.link)
            .cloned()
            .ok_or_else(|| UrdfError::UnknownLink(joint.child.link.clone()))?;

        // Parse origin
        let origin = joint.origin.as_ref();
        let xyz = origin.map(|o| o.xyz_vec()).unwrap_or([0.0, 0.0, 0.0]);

        // URDF uses meters, vcad uses mm
        let parent_anchor = Vec3::new(xyz[0] * 1000.0, xyz[1] * 1000.0, xyz[2] * 1000.0);
        let child_anchor = Vec3::new(0.0, 0.0, 0.0);

        // Convert joint type
        let kind = match joint.joint_type.as_str() {
            "fixed" => JointKind::Fixed,
            "revolute" => {
                let axis = joint
                    .axis
                    .as_ref()
                    .map(|a| a.xyz_vec())
                    .unwrap_or([0.0, 0.0, 1.0]);
                let limits = joint.limit.as_ref().and_then(|l| {
                    match (l.lower, l.upper) {
                        (Some(lower), Some(upper)) => {
                            // URDF uses radians, vcad uses degrees
                            Some((lower.to_degrees(), upper.to_degrees()))
                        }
                        _ => None,
                    }
                });
                JointKind::Revolute {
                    axis: Vec3::new(axis[0], axis[1], axis[2]),
                    limits,
                }
            }
            "continuous" => {
                // Continuous is revolute without limits
                let axis = joint
                    .axis
                    .as_ref()
                    .map(|a| a.xyz_vec())
                    .unwrap_or([0.0, 0.0, 1.0]);
                JointKind::Revolute {
                    axis: Vec3::new(axis[0], axis[1], axis[2]),
                    limits: None,
                }
            }
            "prismatic" => {
                let axis = joint
                    .axis
                    .as_ref()
                    .map(|a| a.xyz_vec())
                    .unwrap_or([1.0, 0.0, 0.0]);
                let limits = joint.limit.as_ref().and_then(|l| {
                    match (l.lower, l.upper) {
                        (Some(lower), Some(upper)) => {
                            // URDF uses meters, vcad uses mm
                            Some((lower * 1000.0, upper * 1000.0))
                        }
                        _ => None,
                    }
                });
                JointKind::Slider {
                    axis: Vec3::new(axis[0], axis[1], axis[2]),
                    limits,
                }
            }
            "floating" | "planar" => {
                // Not directly supported, approximate with ball joint
                JointKind::Ball
            }
            other => return Err(UrdfError::UnsupportedJointType(other.to_string())),
        };

        Ok(VcadJoint {
            id: joint.name.clone(),
            name: Some(joint.name.clone()),
            parent_instance_id,
            child_instance_id,
            parent_anchor,
            child_anchor,
            kind,
            state: 0.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE_URDF: &str = r#"<?xml version="1.0"?>
<robot name="simple_robot">
    <link name="base_link">
        <visual>
            <geometry>
                <box size="0.1 0.1 0.05"/>
            </geometry>
        </visual>
    </link>
    <link name="arm_link">
        <visual>
            <origin xyz="0 0 0.05"/>
            <geometry>
                <cylinder radius="0.02" length="0.1"/>
            </geometry>
        </visual>
    </link>
    <joint name="base_to_arm" type="revolute">
        <parent link="base_link"/>
        <child link="arm_link"/>
        <origin xyz="0 0 0.025"/>
        <axis xyz="0 0 1"/>
        <limit lower="-1.57" upper="1.57" effort="10" velocity="1"/>
    </joint>
</robot>"#;

    #[test]
    fn test_parse_simple_urdf() {
        let doc = read_urdf_from_str(SIMPLE_URDF).unwrap();

        // Check basic structure
        assert!(doc.part_defs.is_some());
        assert!(doc.instances.is_some());
        assert!(doc.joints.is_some());

        let part_defs = doc.part_defs.unwrap();
        assert_eq!(part_defs.len(), 2);

        let instances = doc.instances.unwrap();
        assert_eq!(instances.len(), 2);

        let joints = doc.joints.unwrap();
        assert_eq!(joints.len(), 1);

        // Check joint type
        let joint = &joints[0];
        assert_eq!(joint.id, "base_to_arm");
        match &joint.kind {
            JointKind::Revolute { axis, limits } => {
                assert!((axis.z - 1.0).abs() < 0.01);
                assert!(limits.is_some());
                let (lower, upper) = limits.unwrap();
                // -1.57 rad ≈ -90 deg
                assert!((lower - (-90.0)).abs() < 1.0);
                assert!((upper - 90.0).abs() < 1.0);
            }
            _ => panic!("Expected Revolute joint"),
        }
    }

    #[test]
    fn test_parse_continuous_joint() {
        let urdf = r#"<?xml version="1.0"?>
<robot name="wheel">
    <link name="base"/>
    <link name="wheel"/>
    <joint name="wheel_joint" type="continuous">
        <parent link="base"/>
        <child link="wheel"/>
        <axis xyz="0 1 0"/>
    </joint>
</robot>"#;

        let doc = read_urdf_from_str(urdf).unwrap();
        let joints = doc.joints.unwrap();
        let joint = &joints[0];

        match &joint.kind {
            JointKind::Revolute { axis, limits } => {
                assert!((axis.y - 1.0).abs() < 0.01);
                assert!(limits.is_none()); // Continuous has no limits
            }
            _ => panic!("Expected Revolute joint for continuous"),
        }
    }

    #[test]
    fn test_parse_prismatic_joint() {
        let urdf = r#"<?xml version="1.0"?>
<robot name="linear">
    <link name="base"/>
    <link name="slider"/>
    <joint name="slide_joint" type="prismatic">
        <parent link="base"/>
        <child link="slider"/>
        <axis xyz="1 0 0"/>
        <limit lower="0" upper="0.5" effort="100" velocity="0.5"/>
    </joint>
</robot>"#;

        let doc = read_urdf_from_str(urdf).unwrap();
        let joints = doc.joints.unwrap();
        let joint = &joints[0];

        match &joint.kind {
            JointKind::Slider { axis, limits } => {
                assert!((axis.x - 1.0).abs() < 0.01);
                assert!(limits.is_some());
                let (lower, upper) = limits.unwrap();
                // 0.5m = 500mm
                assert!((lower - 0.0).abs() < 0.1);
                assert!((upper - 500.0).abs() < 0.1);
            }
            _ => panic!("Expected Slider joint"),
        }
    }

    #[test]
    fn test_geometry_conversion() {
        let urdf = r#"<?xml version="1.0"?>
<robot name="shapes">
    <link name="box">
        <visual>
            <geometry><box size="0.1 0.2 0.3"/></geometry>
        </visual>
    </link>
    <link name="cyl">
        <visual>
            <geometry><cylinder radius="0.05" length="0.2"/></geometry>
        </visual>
    </link>
    <link name="sph">
        <visual>
            <geometry><sphere radius="0.1"/></geometry>
        </visual>
    </link>
</robot>"#;

        let doc = read_urdf_from_str(urdf).unwrap();

        // Find the box node
        let box_node = doc
            .nodes
            .values()
            .find(|n| matches!(n.op, CsgOp::Cube { .. }))
            .unwrap();

        if let CsgOp::Cube { size } = &box_node.op {
            // 0.1m = 100mm
            assert!((size.x - 100.0).abs() < 0.1);
            assert!((size.y - 200.0).abs() < 0.1);
            assert!((size.z - 300.0).abs() < 0.1);
        }
    }
}
