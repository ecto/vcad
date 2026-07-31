//! URDF file reader: converts URDF XML to vcad Document.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use vcad_ir::{
    CsgOp, Document, InertialProperties, Instance, Joint as VcadJoint, JointKind, MaterialDef,
    Node, NodeId, PartDef, SceneEntry, Vec3,
};

use crate::error::UrdfError;
use crate::types::{Geometry, Joint, Link, Robot};

/// Locate the byte position just past `<robot ...>` and the position of
/// `</robot>` (start of close tag). Used by [`normalize_robot_child_order`].
fn locate_robot_bounds(reader: &mut quick_xml::Reader<&[u8]>) -> Result<(usize, usize), UrdfError> {
    use quick_xml::events::Event;
    let mut robot_open_end: Option<usize> = None;
    let mut depth: i32 = 0;
    loop {
        let pos_before = reader.buffer_position() as usize;
        match reader
            .read_event()
            .map_err(|e| UrdfError::InvalidFormat(format!("xml scan: {e}")))?
        {
            Event::Start(e) => {
                if e.name().as_ref() == b"robot" && robot_open_end.is_none() {
                    robot_open_end = Some(reader.buffer_position() as usize);
                    depth = 1;
                } else if robot_open_end.is_some() {
                    depth += 1;
                }
            }
            Event::End(e) => {
                if let Some(open_end) = robot_open_end {
                    depth -= 1;
                    if depth == 0 && e.name().as_ref() == b"robot" {
                        return Ok((open_end, pos_before));
                    }
                }
            }
            Event::Eof => {
                return Err(UrdfError::InvalidFormat(if robot_open_end.is_some() {
                    "URDF has unclosed <robot>".into()
                } else {
                    "URDF has no <robot> root element".into()
                }));
            }
            _ => {}
        }
    }
}

/// Reorder top-level children of `<robot>` so all `<link>` siblings come
/// before all `<joint>` siblings.
///
/// quick-xml's serde adapter (as of 0.37) treats repeated siblings as
/// `Vec<T>` only when they're contiguous; an interleaved `<link><joint>
/// <link>` produces a "duplicate field" error. Real URDFs in the wild
/// (e.g. Unitree's official g1_23dof.urdf) do interleave heavily, so we
/// run a quick pre-pass that sorts the immediate children of `<robot>`
/// while preserving everything else byte-for-byte.
fn normalize_robot_child_order(xml: &str) -> Result<String, UrdfError> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    // Walk to <robot>. Everything before it (XML prolog, comments, top-
    // level whitespace) is preserved verbatim.
    // Pass 1: find <robot> opening end + </robot> position.
    let (robot_open_end, robot_close_start) = locate_robot_bounds(&mut reader)?;

    // Pass 2: walk inside <robot>, classify each top-level child by tag
    // name, and capture its byte range in the original source.
    let mut links: Vec<&str> = Vec::new();
    let mut joints: Vec<&str> = Vec::new();
    let mut materials: Vec<&str> = Vec::new();
    let mut others: Vec<&str> = Vec::new();

    let mut reader = Reader::from_str(&xml[robot_open_end..robot_close_start]);
    reader.config_mut().trim_text(false);
    let inner_offset = robot_open_end;

    loop {
        let pos_before = reader.buffer_position() as usize + inner_offset;
        match reader
            .read_event()
            .map_err(|e| UrdfError::InvalidFormat(format!("xml scan: {e}")))?
        {
            Event::Start(e) => {
                let tag = e.name().as_ref().to_vec();
                reader
                    .read_to_end(e.name())
                    .map_err(|err| UrdfError::InvalidFormat(format!("read_to_end: {err}")))?;
                let pos_after = reader.buffer_position() as usize + inner_offset;
                let slice = &xml[pos_before..pos_after];
                match tag.as_slice() {
                    b"link" => links.push(slice),
                    b"joint" => joints.push(slice),
                    b"material" => materials.push(slice),
                    _ => others.push(slice),
                }
            }
            Event::Empty(e) => {
                let tag = e.name().as_ref().to_vec();
                let pos_after = reader.buffer_position() as usize + inner_offset;
                let slice = &xml[pos_before..pos_after];
                match tag.as_slice() {
                    b"link" => links.push(slice),
                    b"joint" => joints.push(slice),
                    b"material" => materials.push(slice),
                    _ => others.push(slice),
                }
            }
            Event::Eof => break,
            // Comments / text / whitespace at the top level are dropped by
            // the reorder — they get folded into the gaps between elements
            // in the rebuilt string. URDF semantics don't care about
            // top-level whitespace.
            _ => {}
        }
    }

    // Rebuild: prefix (everything up to and including the <robot ...>
    // opening tag) verbatim, then materials, links, joints, others, then
    // the </robot> closing tag and any trailing content verbatim.
    let prefix = &xml[..robot_open_end];
    let mut out = String::with_capacity(xml.len());
    out.push_str(prefix);
    for s in &materials {
        out.push('\n');
        out.push_str(s);
    }
    for s in &links {
        out.push('\n');
        out.push_str(s);
    }
    for s in &joints {
        out.push('\n');
        out.push_str(s);
    }
    for s in &others {
        out.push('\n');
        out.push_str(s);
    }
    out.push('\n');
    out.push_str(&xml[robot_close_start..]);
    Ok(out)
}

/// Options for resolving URDF `<mesh>` references.
///
/// Controls how `package://NAME/path` URIs and bare relative paths are
/// turned into absolute filesystem paths the physics layer can `open()`.
#[derive(Debug, Clone, Default)]
pub struct UrdfReadOptions {
    /// Directories to search for `package://NAME/...` resolution. Each
    /// root is checked for a `NAME/` subdirectory containing the rest of
    /// the URI; the first match wins.
    pub package_roots: Vec<PathBuf>,
    /// Directory the URDF lives in — used as the base for resolving
    /// relative mesh paths. Set automatically by [`read_urdf`].
    pub urdf_dir: Option<PathBuf>,
}

impl UrdfReadOptions {
    /// Resolve a URDF `<mesh filename="...">` value to an absolute path on
    /// disk, or `None` if no candidate exists. Logs (via `eprintln!`) when
    /// a `package://` URI cannot be located so the caller knows a mesh
    /// fell back to a placeholder.
    pub fn resolve_mesh(&self, filename: &str) -> Option<PathBuf> {
        // package://NAME/rest/of/path
        if let Some(rest) = filename.strip_prefix("package://") {
            let mut split = rest.splitn(2, '/');
            let pkg = split.next()?;
            let sub = split.next().unwrap_or("");
            for root in &self.package_roots {
                let candidate = root.join(pkg).join(sub);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
            eprintln!(
                "urdf: package URI {filename:?} not found under {:?}",
                self.package_roots
            );
            return None;
        }
        // file:// or absolute or relative
        let stripped = filename.strip_prefix("file://").unwrap_or(filename);
        let p = Path::new(stripped);
        if p.is_absolute() {
            return p.is_file().then(|| p.to_path_buf());
        }
        if let Some(dir) = &self.urdf_dir {
            let candidate = dir.join(p);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }
}

/// Maximum URDF size we will attempt to parse. URDFs are tiny descriptors
/// (tens of KB); a multi-MB "URDF" is almost always an attack payload.
const MAX_URDF_BYTES: usize = 8 * 1024 * 1024;
/// Caps on structural count to bound post-parse work.
const MAX_LINKS: usize = 10_000;
const MAX_JOINTS: usize = 10_000;

/// Read a URDF file from a path with default mesh-resolution options
/// (mesh paths are resolved relative to the URDF's parent directory; no
/// `package://` roots are configured).
pub fn read_urdf(path: impl AsRef<Path>) -> Result<Document, UrdfError> {
    let path = path.as_ref();
    let opts = UrdfReadOptions {
        urdf_dir: path.parent().map(|p| p.to_path_buf()),
        ..UrdfReadOptions::default()
    };
    read_urdf_with_options(path, &opts)
}

/// Read a URDF file from a path, providing explicit mesh-resolution
/// options (e.g. `package_roots` for `package://NAME/...` URIs).
pub fn read_urdf_with_options(
    path: impl AsRef<Path>,
    opts: &UrdfReadOptions,
) -> Result<Document, UrdfError> {
    let path = path.as_ref();
    let content = std::fs::read_to_string(path)?;
    let mut effective = opts.clone();
    if effective.urdf_dir.is_none() {
        effective.urdf_dir = path.parent().map(|p| p.to_path_buf());
    }
    read_urdf_from_str_with_options(&content, &effective)
}

/// Read a URDF from a string with default options. Use
/// [`read_urdf_from_str_with_options`] when meshes need to be resolved
/// against package roots.
pub fn read_urdf_from_str(xml: &str) -> Result<Document, UrdfError> {
    read_urdf_from_str_with_options(xml, &UrdfReadOptions::default())
}

/// Read a URDF from a string with explicit mesh-resolution options.
pub fn read_urdf_from_str_with_options(
    xml: &str,
    opts: &UrdfReadOptions,
) -> Result<Document, UrdfError> {
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
    // Real-world URDFs interleave <link> and <joint> siblings; quick-xml's
    // serde adapter only deserialises Vec<T> when those siblings are
    // contiguous, so reorder them up front. The normalisation is a no-op
    // when the file is already in canonical (links-then-joints) order.
    let normalized = normalize_robot_child_order(xml)?;
    let robot: Robot = quick_xml::de::from_str(&normalized)?;
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
    let reader = UrdfReader::new(&robot, opts);
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
                if bytes.len() - i >= 9 && bytes[i..i + 9].eq_ignore_ascii_case(b"<!DOCTYPE") {
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
    /// Resolves `<mesh filename>` references to absolute paths.
    opts: &'a UrdfReadOptions,
}

impl<'a> UrdfReader<'a> {
    fn new(robot: &'a Robot, opts: &'a UrdfReadOptions) -> Self {
        Self {
            robot,
            link_to_part: HashMap::new(),
            link_to_instance: HashMap::new(),
            next_node_id: 1,
            opts,
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
                tags: Vec::new(),
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
                ..Default::default()
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
                ..Default::default()
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

        // Get geometry from the first visual, falling back to the first
        // collision. URDFs that ship multiple visuals per link describe
        // multi-mesh parts; the importer picks one geometry to evaluate
        // (full multi-mesh support is a future extension — the rest of
        // the parts come along for free once the IR can carry a list of
        // child geometries here).
        let (geom, origin) = if let Some(visual) = link.visuals.first() {
            (&visual.geometry, visual.origin.as_ref())
        } else if let Some(collision) = link.collisions.first() {
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
                    inertial: None,
                },
                nodes,
            ));
        };

        // Create geometry node
        let geom_node_id = self.alloc_node_id();
        let (geom_op, center_offset) = self.geometry_to_csg(geom)?;
        nodes.push((
            geom_node_id,
            Node {
                id: geom_node_id,
                name: Some(format!("{}_geom", link.name)),
                op: geom_op,
            },
        ));

        // URDF box/cylinder/cone primitives are centered on the link frame
        // origin. vcad's Cube extends from the origin into the +X/+Y/+Z
        // octant, and Cylinder/Cone sit on z=0 → z=height. Wrap each
        // primitive in a Translate so the geometry lines up with the URDF
        // convention before any visual <origin> rotation/translation is
        // applied.
        let centered_node_id = if center_offset.x.abs() > 1e-9
            || center_offset.y.abs() > 1e-9
            || center_offset.z.abs() > 1e-9
        {
            let id = self.alloc_node_id();
            nodes.push((
                id,
                Node {
                    id,
                    name: Some(format!("{}_center", link.name)),
                    op: CsgOp::Translate {
                        child: geom_node_id,
                        offset: center_offset,
                    },
                },
            ));
            id
        } else {
            geom_node_id
        };

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
                            child: centered_node_id,
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
                            child: centered_node_id,
                            offset: Vec3::new(xyz_mm[0], xyz_mm[1], xyz_mm[2]),
                        },
                    },
                ));
                translate_id
            } else {
                centered_node_id
            }
        } else {
            centered_node_id
        };

        let inertial = link.inertial.as_ref().map(|i| {
            // URDF stores COM in metres relative to the link frame; the rest
            // of the IR uses millimetres.
            let com_xyz = i
                .origin
                .as_ref()
                .map(|o| o.xyz_vec())
                .unwrap_or([0.0, 0.0, 0.0]);
            let inertia = &i.inertia;
            InertialProperties {
                mass_kg: i.mass.value,
                com_mm: Vec3::new(
                    com_xyz[0] * 1000.0,
                    com_xyz[1] * 1000.0,
                    com_xyz[2] * 1000.0,
                ),
                // [ixx, iyy, izz, ixy, ixz, iyz]
                inertia_kg_m2: [
                    inertia.ixx,
                    inertia.iyy,
                    inertia.izz,
                    inertia.ixy,
                    inertia.ixz,
                    inertia.iyz,
                ],
            }
        });

        Ok((
            PartDef {
                id: format!("part_{}", link.name),
                name: Some(link.name.clone()),
                root: root_id,
                default_material: Some("default".to_string()),
                inertial,
            },
            nodes,
        ))
    }

    /// Convert a URDF `<geometry>` to a vcad `CsgOp` and return the
    /// translation needed to center it on the link frame origin.
    ///
    /// URDF primitives (box, cylinder, sphere) are defined centered at the
    /// link frame. vcad's kernel primitives are anchored differently:
    /// `Cube` puts its corner at the origin, `Cylinder`/`Cone` sit on the
    /// XY plane. Returning the centering offset lets the caller wrap the
    /// geometry node in a Translate so the URDF semantics survive.
    fn geometry_to_csg(&self, geom: &Geometry) -> Result<(CsgOp, Vec3), UrdfError> {
        if let Some(box_geom) = &geom.box_geom {
            let size = box_geom.size_vec();
            // URDF uses meters, vcad uses mm
            let sx = size[0] * 1000.0;
            let sy = size[1] * 1000.0;
            let sz = size[2] * 1000.0;
            Ok((
                CsgOp::Cube {
                    size: Vec3::new(sx, sy, sz),
                },
                Vec3::new(-sx / 2.0, -sy / 2.0, -sz / 2.0),
            ))
        } else if let Some(cyl) = &geom.cylinder {
            // URDF cylinder is along Z axis, centered; vcad cylinder sits
            // on the XY plane so shift it down by half its height.
            let h = cyl.length * 1000.0;
            Ok((
                CsgOp::Cylinder {
                    radius: cyl.radius * 1000.0,
                    height: h,
                    segments: 32,
                },
                Vec3::new(0.0, 0.0, -h / 2.0),
            ))
        } else if let Some(sphere) = &geom.sphere {
            Ok((
                CsgOp::Sphere {
                    radius: sphere.radius * 1000.0,
                    segments: 32,
                },
                Vec3::new(0.0, 0.0, 0.0),
            ))
        } else if let Some(mesh) = &geom.mesh {
            let scale = mesh
                .scale
                .as_ref()
                .map(|s| {
                    let parts: Vec<f64> = s
                        .split_whitespace()
                        .filter_map(|p| p.parse().ok())
                        .collect();
                    if parts.len() >= 3 {
                        Some(Vec3::new(parts[0], parts[1], parts[2]))
                    } else {
                        None
                    }
                })
                .unwrap_or(None);
            // Try to resolve against filesystem (CLI path with urdf_dir
            // set). If that succeeds, emit an absolute path so downstream
            // physics / CSG eval can `open()` the file. Otherwise emit
            // MeshImport with the original URDF filename verbatim — the
            // browser entry point loads meshes JS-side and post-processes
            // these nodes into `ImportedMesh`, and physics paths without a
            // filesystem just skip them.
            let path = self
                .opts
                .resolve_mesh(&mesh.filename)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| mesh.filename.clone());
            Ok((CsgOp::MeshImport { path, scale }, Vec3::new(0.0, 0.0, 0.0)))
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
                    // URDF effort is already N·m; velocity is rad/s → deg/s
                    effort_limit: joint.limit.as_ref().and_then(|l| l.effort),
                    velocity_limit: joint
                        .limit
                        .as_ref()
                        .and_then(|l| l.velocity)
                        .map(f64::to_degrees),
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
                    effort_limit: joint.limit.as_ref().and_then(|l| l.effort),
                    velocity_limit: joint
                        .limit
                        .as_ref()
                        .and_then(|l| l.velocity)
                        .map(f64::to_degrees),
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
                    // URDF effort is already N; velocity is m/s → mm/s
                    effort_limit: joint.limit.as_ref().and_then(|l| l.effort),
                    velocity_limit: joint
                        .limit
                        .as_ref()
                        .and_then(|l| l.velocity)
                        .map(|v| v * 1000.0),
                }
            }
            "floating" => {
                // Full 6-DOF: the child (e.g. a humanoid's floating base)
                // can translate and rotate freely. Previously mapped to
                // Ball, which silently pinned the base in space.
                JointKind::Free
            }
            "planar" => {
                // Approximation: URDF planar is 2 translation + 1 rotation
                // DOF in the plane normal to `axis`. vcad has no planar
                // joint kind yet, so approximate with Ball (3 rotational
                // DOF about the anchor) to keep the link mobile without
                // letting it leave the anchor entirely.
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
            JointKind::Revolute {
                axis,
                limits,
                effort_limit,
                velocity_limit,
            } => {
                assert!((axis.z - 1.0).abs() < 0.01);
                assert!(limits.is_some());
                let (lower, upper) = limits.unwrap();
                // -1.57 rad ≈ -90 deg
                assert!((lower - (-90.0)).abs() < 1.0);
                assert!((upper - 90.0).abs() < 1.0);
                // effort passes through in N·m; velocity 1 rad/s → deg/s
                assert_eq!(*effort_limit, Some(10.0));
                assert!((velocity_limit.unwrap() - 1.0_f64.to_degrees()).abs() < 1e-9);
            }
            _ => panic!("Expected Revolute joint"),
        }
    }

    #[test]
    fn test_parse_actuator_limits_k1_knee() {
        // Booster K1 knee: 40 N·m effort, 12.5 rad/s velocity.
        let urdf = r#"<?xml version="1.0"?>
<robot name="k1_knee">
    <link name="thigh"/>
    <link name="shank"/>
    <joint name="knee" type="revolute">
        <parent link="thigh"/>
        <child link="shank"/>
        <axis xyz="0 1 0"/>
        <limit lower="-0.1" upper="2.27" effort="40" velocity="12.5"/>
    </joint>
</robot>"#;

        let doc = read_urdf_from_str(urdf).unwrap();
        let joints = doc.joints.unwrap();
        match &joints[0].kind {
            JointKind::Revolute {
                effort_limit,
                velocity_limit,
                ..
            } => {
                assert_eq!(*effort_limit, Some(40.0));
                assert!((velocity_limit.unwrap() - 12.5_f64.to_degrees()).abs() < 1e-9);
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
            JointKind::Revolute { axis, limits, .. } => {
                assert!((axis.y - 1.0).abs() < 0.01);
                assert!(limits.is_none()); // Continuous has no limits
            }
            _ => panic!("Expected Revolute joint for continuous"),
        }
    }

    #[test]
    fn test_parse_floating_joint_maps_to_free() {
        let urdf = r#"<?xml version="1.0"?>
<robot name="humanoid">
    <link name="base"/>
    <link name="pelvis"/>
    <joint name="world_joint" type="floating">
        <parent link="base"/>
        <child link="pelvis"/>
    </joint>
</robot>"#;

        let doc = read_urdf_from_str(urdf).unwrap();
        let joints = doc.joints.unwrap();
        assert!(
            matches!(joints[0].kind, JointKind::Free),
            "floating must map to Free (6-DOF), got {:?}",
            joints[0].kind
        );
    }

    #[test]
    fn test_parse_planar_joint_maps_to_ball() {
        let urdf = r#"<?xml version="1.0"?>
<robot name="p">
    <link name="base"/>
    <link name="slider"/>
    <joint name="pj" type="planar">
        <parent link="base"/>
        <child link="slider"/>
        <axis xyz="0 0 1"/>
    </joint>
</robot>"#;

        let doc = read_urdf_from_str(urdf).unwrap();
        let joints = doc.joints.unwrap();
        // Documented approximation: vcad has no planar joint kind yet.
        assert!(matches!(joints[0].kind, JointKind::Ball));
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
            JointKind::Slider { axis, limits, .. } => {
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
