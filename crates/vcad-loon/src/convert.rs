use std::collections::HashMap;

use loon_lang::interp::Value;
use vcad_ir::ecad;
use vcad_ir::*;

use crate::fastener;

/// Walk a loon `Value::Adt` tree and produce a vcad-ir `Document`.
pub fn value_to_document(value: &Value) -> Result<Document, String> {
    value_to_document_in(value, None)
}

/// As [`value_to_document`], resolving relative import paths (`import-step`,
/// `import-mesh`) against `base_dir` — normally the directory of the `.loon`
/// file being evaluated, so a model and its vendor STEPs travel together.
pub fn value_to_document_in(
    value: &Value,
    base_dir: Option<&std::path::Path>,
) -> Result<Document, String> {
    let mut ctx = ConvertCtx::new(base_dir);

    match value {
        // Single solid
        Value::Adt(tag, _) if is_solid_tag(tag) => {
            let root_id = ctx.convert_solid(value)?;
            ctx.doc.roots.push(SceneEntry {
                root: root_id,
                material: "default".into(),
                visible: None,
            });
        }
        // SceneEntry
        Value::Adt(tag, fields) if tag == "SceneEntry" && fields.len() == 2 => {
            let root_id = ctx.convert_solid(&fields[0])?;
            let mat_name = match &fields[1] {
                Value::Str(s) => s.to_string(),
                _ => "default".into(),
            };
            ctx.doc.roots.push(SceneEntry {
                root: root_id,
                material: mat_name,
                visible: None,
            });
        }
        // Named SceneEntry — `[root <symbol> "mat"]`
        Value::Adt(tag, fields) if tag == "NamedSceneEntry" && fields.len() == 3 => {
            push_named_entry(&mut ctx, fields)?;
        }
        // Material definition (standalone)
        Value::Adt(tag, fields) if tag == "Material" && fields.len() == 6 => {
            let name = ctx.str_val(&fields[0])?;
            let r = ctx.f64_val(&fields[1])?;
            let g = ctx.f64_val(&fields[2])?;
            let b = ctx.f64_val(&fields[3])?;
            let metallic = ctx.f64_val(&fields[4])?;
            let roughness = ctx.f64_val(&fields[5])?;
            ctx.doc.materials.insert(
                name.clone(),
                MaterialDef {
                    name,
                    color: [r, g, b],
                    metallic,
                    roughness,
                    density: None,
                    friction: None,
                    ..Default::default()
                },
            );
        }
        // Assembly
        Value::Adt(tag, fields) if tag == "Assembly" && fields.len() == 4 => {
            convert_assembly(&mut ctx, fields)?;
        }
        // Vec of entries
        Value::Vec(items) => {
            for item in items {
                merge_value_into_doc(&mut ctx, item)?;
            }
        }
        _ => {
            return Err(format!(
                "expected Solid, SceneEntry, Assembly, or Vec, got {value}"
            ))
        }
    }

    // Add default material if any root references it and it's missing
    if !ctx.doc.roots.is_empty() && !ctx.doc.materials.contains_key("default") {
        ctx.doc.materials.insert(
            "default".into(),
            MaterialDef {
                name: "default".into(),
                color: [0.8, 0.8, 0.8],
                metallic: 0.0,
                roughness: 0.5,
                density: None,
                friction: None,
                ..Default::default()
            },
        );
    }

    Ok(ctx.doc)
}

/// Push a `NamedSceneEntry` — solid, material name, root-node name — stamping
/// the name onto the root IR node so it survives into exports.
fn push_named_entry(ctx: &mut ConvertCtx, fields: &[Value]) -> Result<(), String> {
    let root_id = ctx.convert_solid(&fields[0])?;
    let mat_name = match &fields[1] {
        Value::Str(s) => s.to_string(),
        _ => "default".into(),
    };
    if let Value::Str(name) = &fields[2] {
        if !name.is_empty() {
            if let Some(node) = ctx.doc.nodes.get_mut(&root_id) {
                node.name = Some(name.to_string());
            }
        }
    }
    ctx.doc.roots.push(SceneEntry {
        root: root_id,
        material: mat_name,
        visible: None,
    });
    Ok(())
}

/// Process a single item from a Vec (can be SceneEntry, Material, or bare Solid).
fn merge_value_into_doc(ctx: &mut ConvertCtx, value: &Value) -> Result<(), String> {
    match value {
        // Top-level effects (print → Unit) and stray scalars (docstrings,
        // numbers) contribute nothing to the scene — same as when they were
        // non-final expressions before the multi-value rewrite.
        Value::Unit
        | Value::Str(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::Keyword(_) => {}
        // Nested vectors of entries (e.g. a top-level #[...] scene vector
        // alongside other roots) merge recursively.
        Value::Vec(items) => {
            for item in items {
                merge_value_into_doc(ctx, item)?;
            }
        }
        Value::Adt(tag, fields) if tag == "SceneEntry" && fields.len() == 2 => {
            let root_id = ctx.convert_solid(&fields[0])?;
            let mat_name = match &fields[1] {
                Value::Str(s) => s.to_string(),
                _ => "default".into(),
            };
            ctx.doc.roots.push(SceneEntry {
                root: root_id,
                material: mat_name,
                visible: None,
            });
        }
        Value::Adt(tag, fields) if tag == "NamedSceneEntry" && fields.len() == 3 => {
            push_named_entry(ctx, fields)?;
        }
        Value::Adt(tag, fields) if tag == "Material" && fields.len() == 6 => {
            let name = ctx.str_val(&fields[0])?;
            let r = ctx.f64_val(&fields[1])?;
            let g = ctx.f64_val(&fields[2])?;
            let b = ctx.f64_val(&fields[3])?;
            let metallic = ctx.f64_val(&fields[4])?;
            let roughness = ctx.f64_val(&fields[5])?;
            ctx.doc.materials.insert(
                name.clone(),
                MaterialDef {
                    name,
                    color: [r, g, b],
                    metallic,
                    roughness,
                    density: None,
                    friction: None,
                    ..Default::default()
                },
            );
        }
        Value::Adt(tag, fields) if tag == "Assembly" && fields.len() == 4 => {
            convert_assembly(ctx, fields)?;
        }
        Value::Adt(tag, _) if is_solid_tag(tag) => {
            let root_id = ctx.convert_solid(value)?;
            ctx.doc.roots.push(SceneEntry {
                root: root_id,
                material: "default".into(),
                visible: None,
            });
        }
        Value::Adt(tag, _) if is_ecad_tag(tag) => {
            convert_ecad_value(ctx, value)?;
        }
        _ => {
            return Err(format!(
                "expected SceneEntry, Material, Assembly, ECAD, or Solid in Vec, got {value}"
            ))
        }
    }
    Ok(())
}

/// Convert an Assembly ADT into Document assembly fields.
///
/// Assembly fields: [parts_vec, instances_vec, joints_vec, ground_str]
fn convert_assembly(ctx: &mut ConvertCtx, fields: &[Value]) -> Result<(), String> {
    // 1. Parts → PartDef + geometry nodes
    let parts = match &fields[0] {
        Value::Vec(v) => v,
        _ => {
            return Err(format!(
                "Assembly: expected Vec of PartEntry, got {}",
                fields[0]
            ))
        }
    };

    let mut part_defs = HashMap::new();
    for part_val in parts {
        let (tag, pf) = match part_val {
            Value::Adt(t, f) => (t.as_str(), f.as_slice()),
            _ => return Err(format!("expected PartEntry ADT, got {part_val}")),
        };
        if tag != "PartEntry" || pf.len() != 3 {
            return Err(format!(
                "expected PartEntry with 3 fields, got {tag}/{}",
                pf.len()
            ));
        }
        let name = ctx.str_val(&pf[0])?;
        let root_id = ctx.convert_solid(&pf[1])?;
        let material = ctx.str_val(&pf[2])?;
        part_defs.insert(
            name.clone(),
            PartDef {
                id: name.clone(),
                name: Some(name),
                root: root_id,
                default_material: Some(material),
                inertial: None,
                colliders: None,
            },
        );
    }
    ctx.doc.part_defs = Some(part_defs);

    // 2. Instances → Instance list
    let instances = match &fields[1] {
        Value::Vec(v) => v,
        _ => {
            return Err(format!(
                "Assembly: expected Vec of InstanceEntry, got {}",
                fields[1]
            ))
        }
    };

    let mut inst_list = Vec::new();
    for inst_val in instances {
        let (tag, inf) = match inst_val {
            Value::Adt(t, f) => (t.as_str(), f.as_slice()),
            _ => return Err(format!("expected InstanceEntry ADT, got {inst_val}")),
        };
        if tag != "InstanceEntry" || inf.len() != 5 {
            return Err(format!(
                "expected InstanceEntry with 5 fields, got {tag}/{}",
                inf.len()
            ));
        }
        let id = ctx.str_val(&inf[0])?;
        let part_def_id = ctx.str_val(&inf[1])?;
        let tx = ctx.f64_val(&inf[2])?;
        let ty = ctx.f64_val(&inf[3])?;
        let tz = ctx.f64_val(&inf[4])?;

        let transform = if tx != 0.0 || ty != 0.0 || tz != 0.0 {
            Some(Transform3D {
                translation: Vec3::new(tx, ty, tz),
                ..Transform3D::default()
            })
        } else {
            None
        };

        inst_list.push(Instance {
            id: id.clone(),
            part_def_id,
            name: Some(id),
            tags: Vec::new(),
            transform,
            material: None,
        });
    }
    ctx.doc.instances = Some(inst_list);

    // 3. Joints → Joint list
    let joints = match &fields[2] {
        Value::Vec(v) => v,
        _ => {
            return Err(format!(
                "Assembly: expected Vec of JointDef, got {}",
                fields[2]
            ))
        }
    };

    let mut joint_list = Vec::new();
    for (idx, jval) in joints.iter().enumerate() {
        let (tag, jf) = match jval {
            Value::Adt(t, f) => (t.as_str(), f.as_slice()),
            _ => return Err(format!("expected JointDef ADT, got {jval}")),
        };
        let joint = match tag {
            // [RevoluteJoint name ax ay az lo hi parent px py pz child cx cy cz]
            "RevoluteJoint" => {
                assert_fields(tag, jf, 14)?;
                let name = ctx.str_val(&jf[0])?;
                let axis = ctx.vec3(jf, 1)?;
                let lo = ctx.f64_val(&jf[4])?;
                let hi = ctx.f64_val(&jf[5])?;
                let parent_id = ctx.str_val(&jf[6])?;
                let parent_anchor = ctx.vec3(jf, 7)?;
                let child_id = ctx.str_val(&jf[10])?;
                let child_anchor = ctx.vec3(jf, 11)?;
                Joint {
                    id: format!("joint_{idx}"),
                    name: Some(name),
                    parent_instance_id: Some(parent_id),
                    child_instance_id: child_id,
                    parent_anchor,
                    child_anchor,
                    kind: JointKind::Revolute {
                        axis,
                        limits: Some((lo, hi)),
                        effort_limit: None,
                        velocity_limit: None,
                    },
                    state: 0.0,
                }
            }
            // [PrismaticJoint name ax ay az lo hi parent px py pz child cx cy cz]
            "PrismaticJoint" => {
                assert_fields(tag, jf, 14)?;
                let name = ctx.str_val(&jf[0])?;
                let axis = ctx.vec3(jf, 1)?;
                let lo = ctx.f64_val(&jf[4])?;
                let hi = ctx.f64_val(&jf[5])?;
                let parent_id = ctx.str_val(&jf[6])?;
                let parent_anchor = ctx.vec3(jf, 7)?;
                let child_id = ctx.str_val(&jf[10])?;
                let child_anchor = ctx.vec3(jf, 11)?;
                Joint {
                    id: format!("joint_{idx}"),
                    name: Some(name),
                    parent_instance_id: Some(parent_id),
                    child_instance_id: child_id,
                    parent_anchor,
                    child_anchor,
                    kind: JointKind::Slider {
                        axis,
                        limits: Some((lo, hi)),
                        effort_limit: None,
                        velocity_limit: None,
                    },
                    state: 0.0,
                }
            }
            // [FixedJoint name parent px py pz child cx cy cz]
            "FixedJoint" => {
                assert_fields(tag, jf, 9)?;
                let name = ctx.str_val(&jf[0])?;
                let parent_id = ctx.str_val(&jf[1])?;
                let parent_anchor = ctx.vec3(jf, 2)?;
                let child_id = ctx.str_val(&jf[5])?;
                let child_anchor = ctx.vec3(jf, 6)?;
                Joint {
                    id: format!("joint_{idx}"),
                    name: Some(name),
                    parent_instance_id: Some(parent_id),
                    child_instance_id: child_id,
                    parent_anchor,
                    child_anchor,
                    kind: JointKind::Fixed,
                    state: 0.0,
                }
            }
            // [BallJoint name parent px py pz child cx cy cz]
            "BallJoint" => {
                assert_fields(tag, jf, 9)?;
                let name = ctx.str_val(&jf[0])?;
                let parent_id = ctx.str_val(&jf[1])?;
                let parent_anchor = ctx.vec3(jf, 2)?;
                let child_id = ctx.str_val(&jf[5])?;
                let child_anchor = ctx.vec3(jf, 6)?;
                Joint {
                    id: format!("joint_{idx}"),
                    name: Some(name),
                    parent_instance_id: Some(parent_id),
                    child_instance_id: child_id,
                    parent_anchor,
                    child_anchor,
                    kind: JointKind::Ball,
                    state: 0.0,
                }
            }
            _ => return Err(format!("unknown JointDef variant: {tag}")),
        };
        joint_list.push(joint);
    }
    ctx.doc.joints = Some(joint_list);

    // 4. Ground instance
    let ground_id = ctx.str_val(&fields[3])?;
    ctx.doc.ground_instance_id = Some(ground_id);

    // Add default material if missing
    if !ctx.doc.materials.contains_key("default") {
        ctx.doc.materials.insert(
            "default".into(),
            MaterialDef {
                name: "default".into(),
                color: [0.8, 0.8, 0.8],
                metallic: 0.0,
                roughness: 0.5,
                density: None,
                friction: None,
                ..Default::default()
            },
        );
    }

    Ok(())
}

fn is_ecad_tag(tag: &str) -> bool {
    matches!(
        tag,
        "EcadComponent"
            | "EcadWire"
            | "EcadLabel"
            | "EcadTrace"
            | "EcadVia"
            | "EcadFootprint"
            | "EcadNet"
            | "EcadRules"
    )
}

/// Convert an ECAD ADT value and merge it into the document.
fn convert_ecad_value(ctx: &mut ConvertCtx, value: &Value) -> Result<(), String> {
    let (tag, fields) = match value {
        Value::Adt(t, f) => (t.as_str(), f.as_slice()),
        _ => return Err(format!("expected ECAD ADT, got {value}")),
    };

    // Ensure schematic exists
    let ensure_schematic = |ctx: &mut ConvertCtx| {
        if ctx.doc.schematic.is_none() {
            ctx.doc.schematic = Some(ecad::SchematicSheet {
                nets: None,
                title: None,
                components: vec![],
                wires: vec![],
                junctions: vec![],
                labels: vec![],
            });
        }
    };

    match tag {
        // [EcadComponent ref value footprint-id x y rotation]
        "EcadComponent" => {
            assert_fields(tag, fields, 6)?;
            let reference = ctx.str_val(&fields[0])?;
            let value = ctx.str_val(&fields[1])?;
            let footprint_id = ctx.str_val(&fields[2])?;
            let x = ctx.f64_val(&fields[3])?;
            let y = ctx.f64_val(&fields[4])?;
            let rotation = ctx.f64_val(&fields[5])?;
            ensure_schematic(ctx);
            let sheet = ctx.doc.schematic.as_mut().unwrap();
            sheet.components.push(ecad::SchematicComponent {
                reference,
                value,
                footprint_id,
                position: Vec2::new(x, y),
                rotation,
                mirror: false,
                pins: vec![],
                pads_override: None,
                properties: HashMap::new(),
            });
        }
        // [EcadWire x1 y1 x2 y2]
        "EcadWire" => {
            assert_fields(tag, fields, 4)?;
            let x1 = ctx.f64_val(&fields[0])?;
            let y1 = ctx.f64_val(&fields[1])?;
            let x2 = ctx.f64_val(&fields[2])?;
            let y2 = ctx.f64_val(&fields[3])?;
            ensure_schematic(ctx);
            let sheet = ctx.doc.schematic.as_mut().unwrap();
            sheet.wires.push(ecad::SchematicWire {
                start: Vec2::new(x1, y1),
                end: Vec2::new(x2, y2),
            });
        }
        // [EcadLabel name x y scope]
        "EcadLabel" => {
            assert_fields(tag, fields, 4)?;
            let name = ctx.str_val(&fields[0])?;
            let x = ctx.f64_val(&fields[1])?;
            let y = ctx.f64_val(&fields[2])?;
            let scope_str = ctx.str_val(&fields[3])?;
            let scope = match scope_str.as_str() {
                "global" | "Global" => ecad::LabelScope::Global,
                "hierarchical" | "Hierarchical" => ecad::LabelScope::Hierarchical,
                _ => ecad::LabelScope::Local,
            };
            ensure_schematic(ctx);
            let sheet = ctx.doc.schematic.as_mut().unwrap();
            sheet.labels.push(ecad::SchematicLabel {
                name,
                position: Vec2::new(x, y),
                rotation: 0.0,
                scope,
            });
        }
        // [EcadNet id name] — stored on PCB
        "EcadNet" => {
            assert_fields(tag, fields, 2)?;
            // Nets are typically used when building PCB data; store for later use
        }
        // Other ECAD types are PCB-level and handled during PCB construction
        _ => {}
    }

    Ok(())
}

/// Apply an Euler XYZ rotation (degrees, X then Y then Z) to a point.
fn rotate_xyz(angles: Vec3, p: Vec3) -> Vec3 {
    let (a, b, c) = (
        angles.x.to_radians(),
        angles.y.to_radians(),
        angles.z.to_radians(),
    );
    let (y, z) = (p.y * a.cos() - p.z * a.sin(), p.y * a.sin() + p.z * a.cos());
    let (x, z) = (p.x * b.cos() + z * b.sin(), -p.x * b.sin() + z * b.cos());
    let (x, y) = (x * c.cos() - y * c.sin(), x * c.sin() + y * c.cos());
    Vec3::new(x, y, z)
}

/// The two in-plane axes of a rotated frame — the plane a bolt circle lies
/// in, taken from the same rotation that orients its fasteners.
fn plane_basis(angles: Vec3) -> (Vec3, Vec3) {
    (
        rotate_xyz(angles, Vec3::new(1.0, 0.0, 0.0)),
        rotate_xyz(angles, Vec3::new(0.0, 1.0, 0.0)),
    )
}

fn is_solid_tag(tag: &str) -> bool {
    matches!(
        tag,
        "Cube"
            | "Cylinder"
            | "Sphere"
            | "Cone"
            | "Torus"
            | "Wedge"
            | "Prism"
            | "Gear"
            | "Empty"
            | "Union"
            | "Difference"
            | "Intersection"
            | "Translate"
            | "Rotate"
            | "Scale"
            | "Mirror"
            | "Extrude"
            | "Revolve"
            | "Shell"
            | "Fillet"
            | "Chamfer"
            | "LinearPattern"
            | "CircularPattern"
            | "SweepLine"
            | "SweepHelix"
            | "Loft"
            | "LoftClosed"
            | "Fastener"
            | "BoltCircle"
            | "ClearanceHole"
            | "TappedHole"
            | "CylinderN"
            | "SphereN"
            | "ConeN"
            | "TorusN"
            | "MeshImport"
            | "StepImport"
            | "SheetBaseFlangeRect"
            | "SheetBaseFlangePolygon"
            | "SheetEdgeFlange"
            | "SheetJog"
            | "SheetHem"
            | "SheetBendRelief"
    )
}

/// Smallest segment count the `-n` primitive forms accept.
///
/// Below this a count is far likelier to mean "give me this many flats"
/// than "approximate this circle coarsely", and the `-n` forms cannot
/// deliver flats — see `segments_val`.
const MIN_SEGMENT_HINT: u32 = 8;

/// What a converted sheet-metal node is, from the point of view of the ops
/// that reference it. Only the rectangular base flange has named edges, and
/// only on its own panel — everywhere else an edge is an index, because the
/// outline is not known until the chain is evaluated.
#[derive(Debug, Clone, Copy)]
struct SheetChain {
    /// The chain's root is a `SheetBaseFlangeRect`, so panel 0's edges are
    /// south/east/north/west = 0/1/2/3.
    rect_root: bool,
}

/// Map a named edge of a rectangular base flange to its outline index.
///
/// `base_flange_rect` emits the outline `(0,0) → (w,0) → (w,d) → (0,d)`, so
/// edge 0 runs along -Y and the rest follow CCW.
fn rect_edge_index(name: &str) -> Option<usize> {
    match name.to_ascii_lowercase().as_str() {
        "south" | "front" | "-y" => Some(0),
        "east" | "right" | "+x" => Some(1),
        "north" | "back" | "+y" => Some(2),
        "west" | "left" | "-x" => Some(3),
        _ => None,
    }
}

struct ConvertCtx {
    doc: Document,
    next_id: NodeId,
    /// Directory to resolve relative import paths against — the directory of
    /// the `.loon` file being evaluated, when there is one.
    base_dir: Option<std::path::PathBuf>,
    /// Sheet-metal nodes emitted so far. Doubles as the check that a sheet op
    /// was actually handed a sheet chain and not a solid.
    sheet_chains: HashMap<NodeId, SheetChain>,
}

impl ConvertCtx {
    fn new(base_dir: Option<&std::path::Path>) -> Self {
        Self {
            doc: Document::default(),
            next_id: 0,
            base_dir: base_dir.map(|p| p.to_path_buf()),
            sheet_chains: HashMap::new(),
        }
    }

    fn alloc_id(&mut self) -> NodeId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn insert_node(&mut self, op: CsgOp) -> NodeId {
        let id = self.alloc_id();
        self.doc.nodes.insert(id, Node { id, name: None, op });
        id
    }

    fn f64_val(&self, v: &Value) -> Result<f64, String> {
        match v {
            Value::Float(f) => Ok(*f),
            Value::Int(i) => Ok(*i as f64),
            _ => Err(format!("expected number, got {v}")),
        }
    }

    fn u32_val(&self, v: &Value) -> Result<u32, String> {
        match v {
            Value::Int(i) => Ok(*i as u32),
            Value::Float(f) => Ok(*f as u32),
            _ => Err(format!("expected integer, got {v}")),
        }
    }

    fn str_val(&self, v: &Value) -> Result<String, String> {
        match v {
            Value::Str(s) => Ok(s.to_string()),
            _ => Err(format!("expected string, got {v}")),
        }
    }

    fn bool_val(&self, v: &Value) -> Result<bool, String> {
        match v {
            Value::Bool(b) => Ok(*b),
            _ => Err(format!("expected bool, got {v}")),
        }
    }

    fn vec3(&self, fields: &[Value], offset: usize) -> Result<Vec3, String> {
        Ok(Vec3::new(
            self.f64_val(&fields[offset])?,
            self.f64_val(&fields[offset + 1])?,
            self.f64_val(&fields[offset + 2])?,
        ))
    }

    fn str_vec(&self, v: &Value) -> Result<Vec<String>, String> {
        match v {
            Value::Vec(items) => items.iter().map(|i| self.str_val(i)).collect(),
            _ => Err(format!("expected a list of strings, got {v}")),
        }
    }

    /// A pinned segment count. `0` is the kernel's "auto" and is reachable
    /// through the plain `[cylinder ...]` forms, so asking for it here is a
    /// mistake; so is anything under 3, which cannot close a face loop.
    fn segments_val(&self, tag: &str, v: &Value) -> Result<u32, String> {
        let n = self.u32_val(v)?;
        if n < MIN_SEGMENT_HINT {
            // A count this low reads as a request for a FACETED PRISM —
            // `[cylinder-n 7.5 24.0 6]` looks like a hex boss in review.
            // It is not one: the segment count is a fidelity hint for the
            // boolean/seam machinery, the surface stays an analytic
            // cylinder, and the tessellator draws a circle. That lie
            // shipped a round bore where a hex drive was specified, so
            // refuse it rather than let the source say one thing and the
            // solid be another.
            return Err(format!(
                "{tag}: segments must be at least {MIN_SEGMENT_HINT}, got {n}. \
                 The count is a fidelity hint for booleans and seams — the \
                 surface stays a true circle, so a low count does NOT make a \
                 faceted prism. For real facets use [prism sides radius height] \
                 (or [hex-prism across-flats height]); to let the kernel \
                 choose the fidelity, drop the -n form."
            ));
        }
        Ok(n)
    }

    /// Resolve an import path against the source file's directory, so a model
    /// can name its vendor geometry relative to itself.
    fn import_path(&self, tag: &str, v: &Value) -> Result<String, String> {
        let raw = self.str_val(v)?;
        if raw.trim().is_empty() {
            return Err(format!("{tag}: path must not be empty"));
        }
        let path = std::path::Path::new(&raw);
        if path.is_absolute() {
            return Ok(raw);
        }
        match &self.base_dir {
            Some(dir) => Ok(dir.join(path).to_string_lossy().into_owned()),
            None => Ok(raw),
        }
    }

    /// Scale up the hardware lines added since `mark` — a fastener inside a
    /// pattern is needed once per instance, so the BOM count follows the
    /// geometry instead of being re-tallied by hand.
    fn multiply_hardware(&mut self, mark: usize, count: u32) {
        for line in self.doc.hardware.iter_mut().skip(mark) {
            line.qty = line.qty.saturating_mul(count.max(1));
        }
    }

    /// Record a hardware requirement, merging into an identical existing line.
    fn add_hardware(&mut self, line: HardwareLine) {
        if let Some(existing) = self
            .doc
            .hardware
            .iter_mut()
            .find(|l| l.catalog_id == line.catalog_id && l.spec == line.spec)
        {
            existing.qty += line.qty;
            return;
        }
        self.doc.hardware.push(line);
    }

    /// Turn one local-frame piece into a node, positioned along local `+Z`.
    fn emit_piece(&mut self, piece: &fastener::Piece) -> NodeId {
        match *piece {
            fastener::Piece::Cylinder { radius, height, z0 } => {
                let c = self.insert_node(CsgOp::Cylinder {
                    radius,
                    height,
                    segments: 0,
                });
                self.insert_node(CsgOp::Translate {
                    child: c,
                    offset: Vec3::new(0.0, 0.0, z0),
                })
            }
            fastener::Piece::Prism {
                sides,
                radius,
                height,
                z0,
            } => {
                let p = self.insert_node(CsgOp::Prism {
                    sides,
                    radius,
                    height,
                });
                self.insert_node(CsgOp::Translate {
                    child: p,
                    offset: Vec3::new(0.0, 0.0, z0),
                })
            }
            fastener::Piece::Cone {
                radius_bottom,
                radius_top,
                height,
                z0,
            } => {
                let c = self.insert_node(CsgOp::Cone {
                    radius_bottom,
                    radius_top,
                    height,
                    segments: 0,
                });
                self.insert_node(CsgOp::Translate {
                    child: c,
                    offset: Vec3::new(0.0, 0.0, z0),
                })
            }
            fastener::Piece::Dome {
                base_radius,
                height,
                z0,
            } => {
                // Spherical cap: sphere of radius R clipped to the slab the
                // cap occupies, which runs from z0 - height up to z0.
                let r = (base_radius * base_radius + height * height) / (2.0 * height);
                let sphere = self.insert_node(CsgOp::Sphere {
                    radius: r,
                    segments: 0,
                });
                let centred = self.insert_node(CsgOp::Translate {
                    child: sphere,
                    offset: Vec3::new(0.0, 0.0, z0 - height + r),
                });
                let clip = self.insert_node(CsgOp::Cylinder {
                    radius: base_radius,
                    height,
                    segments: 0,
                });
                let clip = self.insert_node(CsgOp::Translate {
                    child: clip,
                    offset: Vec3::new(0.0, 0.0, z0 - height),
                });
                self.insert_node(CsgOp::Intersection {
                    left: centred,
                    right: clip,
                })
            }
        }
    }

    /// Build one fastener: union the pieces in the local frame, cut the hex
    /// socket, then rotate the whole thing onto its axis and move it into
    /// place. Head and shaft travel together, so mirroring cannot separate
    /// them or flip one without the other.
    fn emit_fastener(&mut self, plan: &fastener::FastenerPlan) -> Result<NodeId, String> {
        let mut root: Option<NodeId> = None;
        for piece in &plan.additive {
            let id = self.emit_piece(piece);
            root = Some(match root {
                None => id,
                Some(left) => self.insert_node(CsgOp::Union { left, right: id }),
            });
        }
        let mut root = root.ok_or_else(|| "fastener produced no geometry".to_string())?;
        for piece in &plan.subtractive {
            let tool = self.emit_piece(piece);
            root = self.insert_node(CsgOp::Difference {
                left: root,
                right: tool,
            });
        }

        let angles = fastener::axis_to_euler_xyz(plan.axis);
        if angles.x != 0.0 || angles.y != 0.0 || angles.z != 0.0 {
            root = self.insert_node(CsgOp::Rotate {
                child: root,
                angles,
            });
        }
        if plan.origin.x != 0.0 || plan.origin.y != 0.0 || plan.origin.z != 0.0 {
            root = self.insert_node(CsgOp::Translate {
                child: root,
                offset: plan.origin,
            });
        }

        for line in &plan.hardware {
            self.add_hardware(line.clone());
        }
        Ok(root)
    }

    /// `[BoltCircle spec style bcd count cx cy cz ax ay az grip stack]`
    fn convert_bolt_circle(&mut self, fields: &[Value]) -> Result<NodeId, String> {
        assert_fields("BoltCircle", fields, 12)?;
        let spec = self.str_val(&fields[0])?;
        let style = self.str_val(&fields[1])?;
        let bcd = self.f64_val(&fields[2])?;
        let count = self.u32_val(&fields[3])?;
        let center = self.vec3(fields, 4)?;
        let axis = self.vec3(fields, 7)?;
        let grip = self.f64_val(&fields[10])?;
        let stack = self.str_vec(&fields[11])?;

        if count == 0 {
            return Err("bolt-circle needs at least one fastener".into());
        }
        if grip <= 0.0 {
            return Err(format!(
                "bolt-circle grip must be positive (how far the fastener is driven \
                 along its axis), got {grip}"
            ));
        }
        let n = (axis.x * axis.x + axis.y * axis.y + axis.z * axis.z).sqrt();
        if n < 1e-9 {
            return Err("bolt-circle axis must be non-zero".into());
        }
        let axis = Vec3::new(axis.x / n, axis.y / n, axis.z / n);

        // In-plane basis, taken from the same rotation that orients each
        // fastener — so the ring and the bolts agree by construction.
        let e = fastener::axis_to_euler_xyz(axis);
        let (u, v) = plane_basis(e);

        let radius = bcd / 2.0;
        let mut root: Option<NodeId> = None;
        for i in 0..count {
            let theta = std::f64::consts::TAU * (i as f64) / (count as f64);
            let (c, s) = (theta.cos(), theta.sin());
            let from = Vec3::new(
                center.x + radius * (c * u.x + s * v.x),
                center.y + radius * (c * u.y + s * v.y),
                center.z + radius * (c * u.z + s * v.z),
            );
            let to = Vec3::new(
                from.x + grip * axis.x,
                from.y + grip * axis.y,
                from.z + grip * axis.z,
            );
            let plan = fastener::plan(&spec, &style, from, to, &stack)?;
            let id = self.emit_fastener(&plan)?;
            root = Some(match root {
                None => id,
                Some(left) => self.insert_node(CsgOp::Union { left, right: id }),
            });
        }
        Ok(root.expect("count > 0"))
    }

    /// `[ClearanceHole|TappedHole size depth ox oy oz dx dy dz]` — a tool
    /// solid to subtract, sized from the thread designation so the hole and
    /// the fastener that goes in it cannot disagree.
    fn convert_hole(&mut self, tag: &str, fields: &[Value]) -> Result<NodeId, String> {
        assert_fields(tag, fields, 8)?;
        let size = self.str_val(&fields[0])?.to_ascii_uppercase();
        let depth = self.f64_val(&fields[1])?;
        let origin = self.vec3(fields, 2)?;
        let dir = self.vec3(fields, 5)?;
        if depth <= 0.0 {
            return Err(format!("{tag}: depth must be positive, got {depth}"));
        }
        let n = (dir.x * dir.x + dir.y * dir.y + dir.z * dir.z).sqrt();
        if n < 1e-9 {
            return Err(format!("{tag}: direction must be non-zero"));
        }
        let dia = if tag == "TappedHole" {
            fastener::tap_drill_dia(&size)
        } else {
            fastener::clearance_hole_dia(&size)
        };

        // Overshoot both ends slightly so the cut leaves no membrane.
        let eps = 0.01;
        let cyl = self.insert_node(CsgOp::Cylinder {
            radius: dia / 2.0,
            height: depth + 2.0 * eps,
            segments: 0,
        });
        let lifted = self.insert_node(CsgOp::Translate {
            child: cyl,
            offset: Vec3::new(0.0, 0.0, -eps),
        });
        let angles = fastener::axis_to_euler_xyz(Vec3::new(dir.x / n, dir.y / n, dir.z / n));
        let rotated = if angles.x != 0.0 || angles.y != 0.0 || angles.z != 0.0 {
            self.insert_node(CsgOp::Rotate {
                child: lifted,
                angles,
            })
        } else {
            lifted
        };
        Ok(self.insert_node(CsgOp::Translate {
            child: rotated,
            offset: origin,
        }))
    }

    // -------------------------------------------------------------------
    // Sheet metal
    //
    // These build a bend graph, not a CSG tree: the engine detects a
    // sheet-metal root and routes the whole chain to the sheet kernel,
    // which returns a body *and* an exact flat pattern. That is the whole
    // point of authoring them natively — the alternative is unioned plates
    // whose bends have to be inferred back out of the solid afterwards.
    // -------------------------------------------------------------------

    fn convert_sheet_metal(&mut self, tag: &str, fields: &[Value]) -> Result<NodeId, String> {
        let (op, chain) = match tag {
            "SheetBaseFlangeRect" => {
                assert_fields(tag, fields, 6)?;
                let width = self.f64_val(&fields[0])?;
                let depth = self.f64_val(&fields[1])?;
                let thickness = self.f64_val(&fields[2])?;
                self.check_positive(tag, "width", width)?;
                self.check_positive(tag, "depth", depth)?;
                self.check_positive(tag, "thickness", thickness)?;
                (
                    CsgOp::SheetMetalBaseFlangeRect {
                        width,
                        depth,
                        thickness,
                        material: self.str_val(&fields[3])?,
                        shop_profile: self.opt_str(&fields[4])?,
                        engravings: self.engravings(&fields[5])?,
                    },
                    SheetChain { rect_root: true },
                )
            }
            "SheetBaseFlangePolygon" => {
                assert_fields(tag, fields, 6)?;
                let outline = self.point_loop(tag, "outline", &fields[0])?;
                let holes = match &fields[1] {
                    Value::Vec(items) => items
                        .iter()
                        .map(|h| self.point_loop(tag, "hole", h))
                        .collect::<Result<Vec<_>, _>>()?,
                    other => return Err(format!("{tag}: expected a list of holes, got {other}")),
                };
                let thickness = self.f64_val(&fields[2])?;
                self.check_positive(tag, "thickness", thickness)?;
                (
                    CsgOp::SheetMetalBaseFlangePolygon {
                        outline,
                        holes,
                        thickness,
                        material: self.str_val(&fields[3])?,
                        shop_profile: self.opt_str(&fields[4])?,
                        engravings: self.engravings(&fields[5])?,
                    },
                    SheetChain { rect_root: false },
                )
            }
            "SheetEdgeFlange" => {
                assert_fields(tag, fields, 8)?;
                let (parent, chain) = self.sheet_parent(tag, &fields[0])?;
                let panel_id = self.usize_val(&fields[1])?;
                let length = self.f64_val(&fields[3])?;
                let angle_deg = self.f64_val(&fields[4])?;
                self.check_positive(tag, "length", length)?;
                if angle_deg <= 0.0 || angle_deg > 180.0 {
                    return Err(format!(
                        "{tag}: angle must be in (0, 180] degrees, got {angle_deg}"
                    ));
                }
                (
                    CsgOp::SheetMetalEdgeFlange {
                        parent,
                        panel_id,
                        edge_index: self.edge_index(tag, &fields[2], panel_id, chain)?,
                        length,
                        angle: angle_deg.to_radians(),
                        radius: self.opt_positive(tag, "radius", &fields[5])?,
                        direction: self.direction(tag, &fields[6])?,
                        manual_k: self.opt_positive(tag, "k", &fields[7])?,
                    },
                    chain,
                )
            }
            "SheetJog" => {
                assert_fields(tag, fields, 7)?;
                let (parent, chain) = self.sheet_parent(tag, &fields[0])?;
                let panel_id = self.usize_val(&fields[1])?;
                let offset = self.f64_val(&fields[3])?;
                let length = self.f64_val(&fields[4])?;
                self.check_positive(tag, "offset", offset)?;
                self.check_positive(tag, "length", length)?;
                (
                    CsgOp::SheetMetalJog {
                        parent,
                        panel_id,
                        edge_index: self.edge_index(tag, &fields[2], panel_id, chain)?,
                        offset,
                        length,
                        radius: self.opt_positive(tag, "radius", &fields[5])?,
                        direction: self.direction(tag, &fields[6])?,
                    },
                    chain,
                )
            }
            "SheetHem" => {
                assert_fields(tag, fields, 7)?;
                let (parent, chain) = self.sheet_parent(tag, &fields[0])?;
                let panel_id = self.usize_val(&fields[1])?;
                let kind = self.hem_kind(tag, &fields[3])?;
                let length = self.f64_val(&fields[4])?;
                let gap = self.f64_val(&fields[5])?;
                self.check_positive(tag, "length", length)?;
                if kind == SheetMetalHemKind::Open && gap <= 0.0 {
                    return Err(format!(
                        "{tag}: an open hem needs a positive gap, got {gap}"
                    ));
                }
                (
                    CsgOp::SheetMetalHem {
                        parent,
                        panel_id,
                        edge_index: self.edge_index(tag, &fields[2], panel_id, chain)?,
                        kind,
                        length,
                        gap,
                        direction: self.direction(tag, &fields[6])?,
                    },
                    chain,
                )
            }
            "SheetBendRelief" => {
                assert_fields(tag, fields, 3)?;
                let (parent, chain) = self.sheet_parent(tag, &fields[0])?;
                (
                    CsgOp::SheetMetalBendRelief {
                        parent,
                        width: self.opt_positive(tag, "width", &fields[1])?,
                        depth: self.opt_positive(tag, "depth", &fields[2])?,
                    },
                    chain,
                )
            }
            _ => return Err(format!("unknown sheet-metal variant: {tag}")),
        };

        let id = self.insert_node(op);
        self.sheet_chains.insert(id, chain);
        Ok(id)
    }

    /// Convert a sheet op's parent, refusing anything that isn't a sheet
    /// chain. A solid here would otherwise reach the engine as a sheet
    /// parent and fail deep in the kernel with no mention of the loon form
    /// that produced it.
    fn sheet_parent(&mut self, tag: &str, value: &Value) -> Result<(NodeId, SheetChain), String> {
        let parent = self.convert_solid(value)?;
        match self.sheet_chains.get(&parent).copied() {
            Some(chain) => Ok((parent, chain)),
            None => Err(format!(
                "{tag}: expected a sheet-metal chain, got a solid — a sheet chain \
                 must start at [sheet-base-flange-rect ...] or [sheet-base-flange ...]"
            )),
        }
    }

    /// An outline edge: an index, or a compass name on a rectangular base
    /// flange's own panel (the only place the outline is known here).
    fn edge_index(
        &self,
        tag: &str,
        value: &Value,
        panel_id: usize,
        chain: SheetChain,
    ) -> Result<usize, String> {
        match value {
            Value::Str(name) => {
                let Some(index) = rect_edge_index(name) else {
                    return Err(format!(
                        "{tag}: unknown edge name {name:?} — expected \
                         \"south\" \"east\" \"north\" \"west\", or an edge index"
                    ));
                };
                if !chain.rect_root || panel_id != 0 {
                    return Err(format!(
                        "{tag}: edge name {name:?} is only defined on panel 0 of a \
                         rectangular base flange — use a numeric edge index here"
                    ));
                }
                Ok(index)
            }
            _ => self.usize_val(value),
        }
    }

    fn direction(&self, tag: &str, value: &Value) -> Result<SheetMetalDirection, String> {
        match self.str_val(value)?.to_ascii_lowercase().as_str() {
            "up" => Ok(SheetMetalDirection::Up),
            "down" => Ok(SheetMetalDirection::Down),
            other => Err(format!(
                "{tag}: direction must be \"up\" or \"down\", got {other:?}"
            )),
        }
    }

    fn hem_kind(&self, tag: &str, value: &Value) -> Result<SheetMetalHemKind, String> {
        match self.str_val(value)?.to_ascii_lowercase().as_str() {
            "closed" => Ok(SheetMetalHemKind::Closed),
            "open" => Ok(SheetMetalHemKind::Open),
            other => Err(format!(
                "{tag}: hem kind must be \"closed\" or \"open\", got {other:?}"
            )),
        }
    }

    fn usize_val(&self, v: &Value) -> Result<usize, String> {
        match v {
            Value::Int(i) if *i >= 0 => Ok(*i as usize),
            Value::Float(f) if *f >= 0.0 && f.fract() == 0.0 => Ok(*f as usize),
            _ => Err(format!("expected a non-negative integer, got {v}")),
        }
    }

    /// An optional string field: empty means "not set".
    fn opt_str(&self, v: &Value) -> Result<Option<String>, String> {
        let s = self.str_val(v)?;
        Ok((!s.trim().is_empty()).then_some(s))
    }

    /// An optional dimension: `0.0` means "use the default" (material
    /// thickness, the shop profile's fixed radius, the bend table). Negative
    /// is always a mistake.
    fn opt_positive(&self, tag: &str, what: &str, v: &Value) -> Result<Option<f64>, String> {
        let x = self.f64_val(v)?;
        if x < 0.0 {
            return Err(format!("{tag}: {what} must not be negative, got {x}"));
        }
        Ok((x > 0.0).then_some(x))
    }

    fn check_positive(&self, tag: &str, what: &str, x: f64) -> Result<(), String> {
        if x <= 0.0 || x.is_nan() {
            return Err(format!("{tag}: {what} must be positive, got {x}"));
        }
        Ok(())
    }

    /// A closed loop given as a flat `#[x0 y0 x1 y1 ...]`.
    fn point_loop(&self, tag: &str, what: &str, v: &Value) -> Result<Vec<Vec2>, String> {
        let items = match v {
            Value::Vec(items) => items,
            other => return Err(format!("{tag}: expected a {what} point list, got {other}")),
        };
        if items.len() % 2 != 0 {
            return Err(format!(
                "{tag}: {what} needs an even number of coordinates (x y x y ...), got {}",
                items.len()
            ));
        }
        if items.len() < 6 {
            return Err(format!(
                "{tag}: {what} needs at least 3 points, got {}",
                items.len() / 2
            ));
        }
        (0..items.len() / 2)
            .map(|i| {
                Ok(Vec2::new(
                    self.f64_val(&items[2 * i])?,
                    self.f64_val(&items[2 * i + 1])?,
                ))
            })
            .collect()
    }

    fn engravings(&self, v: &Value) -> Result<Option<Vec<SheetMetalEngraving>>, String> {
        let items = match v {
            Value::Vec(items) => items,
            other => return Err(format!("expected a list of engravings, got {other}")),
        };
        if items.is_empty() {
            return Ok(None);
        }
        let marks = items
            .iter()
            .map(|item| self.engraving(item))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(marks))
    }

    fn engraving(&self, v: &Value) -> Result<SheetMetalEngraving, String> {
        let (tag, fields) = match v {
            Value::Adt(tag, fields) => (tag.as_str(), fields.as_slice()),
            other => return Err(format!("expected an engraving, got {other}")),
        };
        match tag {
            "EngravePolyline" => {
                assert_fields(tag, fields, 1)?;
                let items = match &fields[0] {
                    Value::Vec(items) => items,
                    other => return Err(format!("{tag}: expected a point list, got {other}")),
                };
                if items.len() % 2 != 0 || items.len() < 4 {
                    return Err(format!(
                        "{tag}: needs at least 2 points as x y x y ..., got {} coordinates",
                        items.len()
                    ));
                }
                Ok(SheetMetalEngraving::Polyline {
                    points: (0..items.len() / 2)
                        .map(|i| {
                            Ok(Vec2::new(
                                self.f64_val(&items[2 * i])?,
                                self.f64_val(&items[2 * i + 1])?,
                            ))
                        })
                        .collect::<Result<Vec<_>, String>>()?,
                })
            }
            "EngraveText" => {
                assert_fields(tag, fields, 5)?;
                let height = self.f64_val(&fields[3])?;
                self.check_positive(tag, "height", height)?;
                Ok(SheetMetalEngraving::Text {
                    text: self.str_val(&fields[0])?,
                    x: self.f64_val(&fields[1])?,
                    y: self.f64_val(&fields[2])?,
                    height,
                    angle: self.f64_val(&fields[4])?.to_radians(),
                })
            }
            _ => Err(format!("unknown engraving variant: {tag}")),
        }
    }

    fn convert_solid(&mut self, value: &Value) -> Result<NodeId, String> {
        let (tag, fields) = match value {
            Value::Adt(tag, fields) => (tag.as_str(), fields.as_slice()),
            _ => return Err(format!("expected Solid ADT, got {value}")),
        };

        // Catalog-backed forms build a whole subtree (and emit BOM lines)
        // rather than a single op.
        match tag {
            "Fastener" => {
                assert_fields(tag, fields, 9)?;
                let spec = self.str_val(&fields[0])?;
                let style = self.str_val(&fields[1])?;
                let from = self.vec3(fields, 2)?;
                let to = self.vec3(fields, 5)?;
                let stack = self.str_vec(&fields[8])?;
                let plan = fastener::plan(&spec, &style, from, to, &stack)?;
                return self.emit_fastener(&plan);
            }
            "BoltCircle" => return self.convert_bolt_circle(fields),
            "ClearanceHole" | "TappedHole" => return self.convert_hole(tag, fields),
            "SheetBaseFlangeRect"
            | "SheetBaseFlangePolygon"
            | "SheetEdgeFlange"
            | "SheetJog"
            | "SheetHem"
            | "SheetBendRelief" => return self.convert_sheet_metal(tag, fields),
            _ => {}
        }

        let op = match tag {
            // Primitives
            "Cube" => {
                assert_fields(tag, fields, 3)?;
                CsgOp::Cube {
                    size: self.vec3(fields, 0)?,
                }
            }
            "Cylinder" => {
                assert_fields(tag, fields, 2)?;
                CsgOp::Cylinder {
                    radius: self.f64_val(&fields[0])?,
                    height: self.f64_val(&fields[1])?,
                    segments: 0,
                }
            }
            "Sphere" => {
                assert_fields(tag, fields, 1)?;
                CsgOp::Sphere {
                    radius: self.f64_val(&fields[0])?,
                    segments: 0,
                }
            }
            "Cone" => {
                assert_fields(tag, fields, 3)?;
                CsgOp::Cone {
                    radius_bottom: self.f64_val(&fields[0])?,
                    radius_top: self.f64_val(&fields[1])?,
                    height: self.f64_val(&fields[2])?,
                    segments: 0,
                }
            }
            "Torus" => {
                assert_fields(tag, fields, 2)?;
                CsgOp::Torus {
                    major_radius: self.f64_val(&fields[0])?,
                    minor_radius: self.f64_val(&fields[1])?,
                    segments: 0,
                }
            }
            // Involute spur gear. Unlike the other primitives this is not its
            // own `CsgOp` — it lowers to a `Sketch2D` of the exact involute
            // profile plus an `Extrude`. That is not a shortcut: the profile
            // *is* the gear, and extruding a closed 2D loop builds the walls
            // directly with no boolean pass, where a per-tooth union of boxes
            // (the recipe this replaces) pays N booleans and is where the
            // cracks came from. See `crate::gear` for the profile math and the
            // two documented approximations.
            //
            // [Gear module teeth face-width backlash internal]
            "Gear" => {
                assert_fields(tag, fields, 5)?;
                let teeth = self.f64_val(&fields[1])?;
                if teeth.fract() != 0.0 || teeth < 0.0 || teeth > u32::MAX as f64 {
                    return Err(format!(
                        "gear tooth count must be a whole number, got {teeth}"
                    ));
                }
                let spec = crate::gear::GearSpec {
                    module: self.f64_val(&fields[0])?,
                    teeth: teeth as u32,
                    face_width: self.f64_val(&fields[2])?,
                    backlash: self.f64_val(&fields[3])?,
                    internal: self.bool_val(&fields[4])?,
                };
                let segments = spec.sketch_segments()?;
                // Base on z = 0, extruding up the face width. This matches
                // what `cylinder` and `prism` actually do (their doc comments
                // say "centered at origin", but the kernel builds them from
                // z = 0), so a gear stacks onto a shaft the way an author
                // expects rather than sinking half its face into it.
                let sketch = self.insert_node(CsgOp::Sketch2D {
                    origin: Vec3::new(0.0, 0.0, 0.0),
                    x_dir: Vec3::new(1.0, 0.0, 0.0),
                    y_dir: Vec3::new(0.0, 1.0, 0.0),
                    segments,
                    holes: None,
                });
                CsgOp::Extrude {
                    sketch,
                    direction: Vec3::new(0.0, 0.0, spec.face_width),
                    twist_angle: None,
                    scale_end: None,
                }
            }
            // Same primitives with the segment count pinned by the author.
            "CylinderN" => {
                assert_fields(tag, fields, 3)?;
                CsgOp::Cylinder {
                    radius: self.f64_val(&fields[0])?,
                    height: self.f64_val(&fields[1])?,
                    segments: self.segments_val(tag, &fields[2])?,
                }
            }
            "SphereN" => {
                assert_fields(tag, fields, 2)?;
                CsgOp::Sphere {
                    radius: self.f64_val(&fields[0])?,
                    segments: self.segments_val(tag, &fields[1])?,
                }
            }
            "ConeN" => {
                assert_fields(tag, fields, 4)?;
                CsgOp::Cone {
                    radius_bottom: self.f64_val(&fields[0])?,
                    radius_top: self.f64_val(&fields[1])?,
                    height: self.f64_val(&fields[2])?,
                    segments: self.segments_val(tag, &fields[3])?,
                }
            }
            "TorusN" => {
                assert_fields(tag, fields, 3)?;
                CsgOp::Torus {
                    major_radius: self.f64_val(&fields[0])?,
                    minor_radius: self.f64_val(&fields[1])?,
                    segments: self.segments_val(tag, &fields[2])?,
                }
            }

            // Imported vendor geometry.
            "MeshImport" => {
                assert_fields(tag, fields, 4)?;
                let scale = self.vec3(fields, 1)?;
                CsgOp::MeshImport {
                    path: self.import_path(tag, &fields[0])?,
                    scale: (scale.x != 1.0 || scale.y != 1.0 || scale.z != 1.0).then_some(scale),
                }
            }
            "StepImport" => {
                assert_fields(tag, fields, 2)?;
                let index = self.u32_val(&fields[1])?;
                CsgOp::StepImport {
                    path: self.import_path(tag, &fields[0])?,
                    solid_index: (index != 0).then_some(index),
                }
            }

            "Wedge" => {
                assert_fields(tag, fields, 3)?;
                CsgOp::Wedge {
                    size: self.vec3(fields, 0)?,
                }
            }
            "Prism" => {
                assert_fields(tag, fields, 3)?;
                CsgOp::Prism {
                    sides: self.f64_val(&fields[0])?.round().max(3.0) as u32,
                    radius: self.f64_val(&fields[1])?,
                    height: self.f64_val(&fields[2])?,
                }
            }
            "Empty" => CsgOp::Empty,

            // Booleans
            "Union" => {
                assert_fields(tag, fields, 2)?;
                let left = self.convert_solid(&fields[0])?;
                let right = self.convert_solid(&fields[1])?;
                CsgOp::Union { left, right }
            }
            "Difference" => {
                assert_fields(tag, fields, 2)?;
                let left = self.convert_solid(&fields[0])?;
                let right = self.convert_solid(&fields[1])?;
                CsgOp::Difference { left, right }
            }
            "Intersection" => {
                assert_fields(tag, fields, 2)?;
                let left = self.convert_solid(&fields[0])?;
                let right = self.convert_solid(&fields[1])?;
                CsgOp::Intersection { left, right }
            }

            // Transforms
            "Translate" => {
                assert_fields(tag, fields, 4)?;
                let child = self.convert_solid(&fields[0])?;
                CsgOp::Translate {
                    child,
                    offset: self.vec3(fields, 1)?,
                }
            }
            "Rotate" => {
                assert_fields(tag, fields, 4)?;
                let child = self.convert_solid(&fields[0])?;
                CsgOp::Rotate {
                    child,
                    angles: self.vec3(fields, 1)?,
                }
            }
            "Scale" => {
                assert_fields(tag, fields, 4)?;
                let child = self.convert_solid(&fields[0])?;
                CsgOp::Scale {
                    child,
                    factor: self.vec3(fields, 1)?,
                }
            }
            "Mirror" => {
                // [Mirror solid ox oy oz nx ny nz]
                assert_fields(tag, fields, 7)?;
                let child = self.convert_solid(&fields[0])?;
                CsgOp::Mirror {
                    child,
                    plane_origin: self.vec3(fields, 1)?,
                    plane_normal: self.vec3(fields, 4)?,
                }
            }

            // Features
            "Extrude" => {
                assert_fields(tag, fields, 4)?;
                let sketch = self.convert_sketch(&fields[0])?;
                CsgOp::Extrude {
                    sketch,
                    direction: self.vec3(fields, 1)?,
                    twist_angle: None,
                    scale_end: None,
                }
            }
            "Revolve" => {
                // [Revolve sketch aox aoy aoz adx ady adz angle]
                assert_fields(tag, fields, 8)?;
                let sketch = self.convert_sketch(&fields[0])?;
                CsgOp::Revolve {
                    sketch,
                    axis_origin: self.vec3(fields, 1)?,
                    axis_dir: self.vec3(fields, 4)?,
                    angle_deg: self.f64_val(&fields[7])?,
                }
            }
            "Shell" => {
                assert_fields(tag, fields, 2)?;
                let child = self.convert_solid(&fields[0])?;
                CsgOp::Shell {
                    child,
                    thickness: self.f64_val(&fields[1])?,
                }
            }
            "Fillet" => {
                assert_fields(tag, fields, 2)?;
                let child = self.convert_solid(&fields[0])?;
                CsgOp::Fillet {
                    child,
                    radius: self.f64_val(&fields[1])?,
                }
            }
            "Chamfer" => {
                assert_fields(tag, fields, 2)?;
                let child = self.convert_solid(&fields[0])?;
                CsgOp::Chamfer {
                    child,
                    distance: self.f64_val(&fields[1])?,
                }
            }

            // Patterns
            "LinearPattern" => {
                // [LinearPattern solid dx dy dz count spacing]
                assert_fields(tag, fields, 6)?;
                let mark = self.doc.hardware.len();
                let child = self.convert_solid(&fields[0])?;
                let count = self.u32_val(&fields[4])?;
                self.multiply_hardware(mark, count);
                CsgOp::LinearPattern {
                    child,
                    direction: self.vec3(fields, 1)?,
                    count: self.u32_val(&fields[4])?,
                    spacing: self.f64_val(&fields[5])?,
                }
            }
            "CircularPattern" => {
                // [CircularPattern solid ox oy oz ax ay az count angle]
                assert_fields(tag, fields, 9)?;
                let mark = self.doc.hardware.len();
                let child = self.convert_solid(&fields[0])?;
                let count = self.u32_val(&fields[7])?;
                self.multiply_hardware(mark, count);
                CsgOp::CircularPattern {
                    child,
                    axis_origin: self.vec3(fields, 1)?,
                    axis_dir: self.vec3(fields, 4)?,
                    count: self.u32_val(&fields[7])?,
                    angle_deg: self.f64_val(&fields[8])?,
                }
            }

            // Sweep along a line path
            // [SweepLine sketch sx sy sz ex ey ez]
            "SweepLine" => {
                assert_fields(tag, fields, 7)?;
                let sketch = self.convert_sketch(&fields[0])?;
                CsgOp::Sweep {
                    sketch,
                    path: PathCurve::Line {
                        start: self.vec3(fields, 1)?,
                        end: self.vec3(fields, 4)?,
                    },
                    twist_angle: None,
                    scale_start: None,
                    scale_end: None,
                    orientation: None,
                    path_segments: None,
                    arc_segments: None,
                }
            }
            // Sweep along a helix path
            // [SweepHelix sketch radius pitch height turns]
            "SweepHelix" => {
                assert_fields(tag, fields, 5)?;
                let sketch = self.convert_sketch(&fields[0])?;
                CsgOp::Sweep {
                    sketch,
                    path: PathCurve::Helix {
                        radius: self.f64_val(&fields[1])?,
                        pitch: self.f64_val(&fields[2])?,
                        height: self.f64_val(&fields[3])?,
                        turns: self.f64_val(&fields[4])?,
                    },
                    twist_angle: None,
                    scale_start: None,
                    scale_end: None,
                    orientation: None,
                    path_segments: None,
                    arc_segments: None,
                }
            }
            // Loft between sketches (open)
            "Loft" => {
                assert_fields(tag, fields, 1)?;
                let sketches = self.convert_sketch_list(&fields[0])?;
                CsgOp::Loft {
                    sketches,
                    closed: None,
                }
            }
            // Loft between sketches (closed — last connects to first)
            "LoftClosed" => {
                assert_fields(tag, fields, 1)?;
                let sketches = self.convert_sketch_list(&fields[0])?;
                CsgOp::Loft {
                    sketches,
                    closed: Some(true),
                }
            }

            _ => return Err(format!("unknown Solid variant: {tag}")),
        };

        Ok(self.insert_node(op))
    }

    fn convert_sketch_list(&mut self, value: &Value) -> Result<Vec<NodeId>, String> {
        let items = match value {
            Value::Vec(v) => v,
            _ => return Err(format!("expected Vec of Sketch, got {value}")),
        };
        items.iter().map(|item| self.convert_sketch(item)).collect()
    }

    fn convert_sketch(&mut self, value: &Value) -> Result<NodeId, String> {
        let (tag, fields) = match value {
            Value::Adt(tag, fields) => (tag.as_str(), fields.as_slice()),
            _ => return Err(format!("expected Sketch ADT, got {value}")),
        };

        match tag {
            "Sketch" => {
                // [Sketch ox oy oz xx xy xz yx yy yz segments]
                assert_fields(tag, fields, 10)?;
                let origin = self.vec3(fields, 0)?;
                let x_dir = self.vec3(fields, 3)?;
                let y_dir = self.vec3(fields, 6)?;
                let segments = self.convert_sketch_segments(&fields[9])?;

                Ok(self.insert_node(CsgOp::Sketch2D {
                    origin,
                    x_dir,
                    y_dir,
                    segments,
                    holes: None,
                }))
            }
            _ => Err(format!("expected Sketch, got {tag}")),
        }
    }

    fn convert_sketch_segments(&self, value: &Value) -> Result<Vec<SketchSegment2D>, String> {
        let items = match value {
            Value::Vec(v) => v,
            _ => return Err(format!("expected Vec of SketchSeg, got {value}")),
        };

        items
            .iter()
            .map(|item| self.convert_sketch_seg(item))
            .collect()
    }

    fn convert_sketch_seg(&self, value: &Value) -> Result<SketchSegment2D, String> {
        let (tag, fields) = match value {
            Value::Adt(tag, fields) => (tag.as_str(), fields.as_slice()),
            _ => return Err(format!("expected SketchSeg ADT, got {value}")),
        };

        match tag {
            "SLine" => {
                // [SLine x1 y1 x2 y2]
                assert_fields(tag, fields, 4)?;
                Ok(SketchSegment2D::Line {
                    start: Vec2::new(self.f64_val(&fields[0])?, self.f64_val(&fields[1])?),
                    end: Vec2::new(self.f64_val(&fields[2])?, self.f64_val(&fields[3])?),
                })
            }
            "SArc" => {
                // [SArc x1 y1 x2 y2 cx cy ccw]
                assert_fields(tag, fields, 7)?;
                Ok(SketchSegment2D::Arc {
                    start: Vec2::new(self.f64_val(&fields[0])?, self.f64_val(&fields[1])?),
                    end: Vec2::new(self.f64_val(&fields[2])?, self.f64_val(&fields[3])?),
                    center: Vec2::new(self.f64_val(&fields[4])?, self.f64_val(&fields[5])?),
                    ccw: self.bool_val(&fields[6])?,
                })
            }
            _ => Err(format!("unknown SketchSeg variant: {tag}")),
        }
    }
}

fn assert_fields(tag: &str, fields: &[Value], expected: usize) -> Result<(), String> {
    if fields.len() != expected {
        return Err(format!(
            "{tag}: expected {expected} fields, got {}",
            fields.len()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use loon_lang::interp::Value;

    fn f(v: f64) -> Value {
        Value::Float(v)
    }
    fn i(v: i64) -> Value {
        Value::Int(v)
    }
    fn s(v: &str) -> Value {
        Value::Str(v.into())
    }
    fn adt(tag: &str, fields: Vec<Value>) -> Value {
        Value::Adt(tag.into(), fields)
    }

    #[test]
    fn cube_to_document() {
        let val = adt("Cube", vec![f(10.0), f(20.0), f(30.0)]);
        let doc = value_to_document(&val).unwrap();
        assert_eq!(doc.roots.len(), 1);
        assert_eq!(doc.nodes.len(), 1);
        match &doc.nodes[&0].op {
            CsgOp::Cube { size } => {
                assert_eq!(size.x, 10.0);
                assert_eq!(size.y, 20.0);
                assert_eq!(size.z, 30.0);
            }
            _ => panic!("expected Cube"),
        }
    }

    #[test]
    fn cylinder_to_document() {
        let val = adt("Cylinder", vec![f(5.0), f(15.0)]);
        let doc = value_to_document(&val).unwrap();
        match &doc.nodes[&0].op {
            CsgOp::Cylinder { radius, height, .. } => {
                assert_eq!(*radius, 5.0);
                assert_eq!(*height, 15.0);
            }
            _ => panic!("expected Cylinder"),
        }
    }

    #[test]
    fn sphere_to_document() {
        let val = adt("Sphere", vec![f(8.0)]);
        let doc = value_to_document(&val).unwrap();
        match &doc.nodes[&0].op {
            CsgOp::Sphere { radius, .. } => assert_eq!(*radius, 8.0),
            _ => panic!("expected Sphere"),
        }
    }

    #[test]
    fn cone_to_document() {
        let val = adt("Cone", vec![f(5.0), f(2.0), f(10.0)]);
        let doc = value_to_document(&val).unwrap();
        match &doc.nodes[&0].op {
            CsgOp::Cone {
                radius_bottom,
                radius_top,
                height,
                ..
            } => {
                assert_eq!(*radius_bottom, 5.0);
                assert_eq!(*radius_top, 2.0);
                assert_eq!(*height, 10.0);
            }
            _ => panic!("expected Cone"),
        }
    }

    #[test]
    fn difference_to_document() {
        let cube = adt("Cube", vec![f(10.0), f(10.0), f(10.0)]);
        let cyl = adt("Cylinder", vec![f(3.0), f(15.0)]);
        let diff = adt("Difference", vec![cube, cyl]);
        let doc = value_to_document(&diff).unwrap();
        assert_eq!(doc.nodes.len(), 3); // cube + cylinder + difference
        match &doc.nodes[&2].op {
            CsgOp::Difference { left, right } => {
                assert_eq!(*left, 0); // cube
                assert_eq!(*right, 1); // cylinder
            }
            _ => panic!("expected Difference"),
        }
    }

    #[test]
    fn translate_to_document() {
        let cube = adt("Cube", vec![f(5.0), f(5.0), f(5.0)]);
        let tr = adt("Translate", vec![cube, f(10.0), f(20.0), f(30.0)]);
        let doc = value_to_document(&tr).unwrap();
        assert_eq!(doc.nodes.len(), 2);
        match &doc.nodes[&1].op {
            CsgOp::Translate { child, offset } => {
                assert_eq!(*child, 0);
                assert_eq!(offset.x, 10.0);
                assert_eq!(offset.y, 20.0);
                assert_eq!(offset.z, 30.0);
            }
            _ => panic!("expected Translate"),
        }
    }

    #[test]
    fn scene_entry_to_document() {
        let cube = adt("Cube", vec![f(10.0), f(10.0), f(10.0)]);
        let entry = adt("SceneEntry", vec![cube, s("aluminum")]);
        let doc = value_to_document(&entry).unwrap();
        assert_eq!(doc.roots.len(), 1);
        assert_eq!(doc.roots[0].material, "aluminum");
    }

    #[test]
    fn vec_of_entries() {
        let entry1 = adt(
            "SceneEntry",
            vec![adt("Cube", vec![f(10.0), f(10.0), f(10.0)]), s("steel")],
        );
        let entry2 = adt("SceneEntry", vec![adt("Sphere", vec![f(5.0)]), s("glass")]);
        let vec_val = Value::Vec(vec![entry1, entry2].into());
        let doc = value_to_document(&vec_val).unwrap();
        assert_eq!(doc.roots.len(), 2);
        assert_eq!(doc.roots[0].material, "steel");
        assert_eq!(doc.roots[1].material, "glass");
        assert_eq!(doc.nodes.len(), 2); // cube + sphere
    }

    #[test]
    fn sketch_extrude_to_document() {
        let line1 = adt("SLine", vec![f(0.0), f(0.0), f(10.0), f(0.0)]);
        let line2 = adt("SLine", vec![f(10.0), f(0.0), f(10.0), f(5.0)]);
        let line3 = adt("SLine", vec![f(10.0), f(5.0), f(0.0), f(5.0)]);
        let line4 = adt("SLine", vec![f(0.0), f(5.0), f(0.0), f(0.0)]);
        let sketch = adt(
            "Sketch",
            vec![
                f(0.0),
                f(0.0),
                f(0.0), // origin
                f(1.0),
                f(0.0),
                f(0.0), // x_dir
                f(0.0),
                f(1.0),
                f(0.0), // y_dir
                Value::Vec(vec![line1, line2, line3, line4].into()),
            ],
        );
        let extrude = adt("Extrude", vec![sketch, f(0.0), f(0.0), f(20.0)]);
        let doc = value_to_document(&extrude).unwrap();
        assert_eq!(doc.nodes.len(), 2); // sketch + extrude
        match &doc.nodes[&0].op {
            CsgOp::Sketch2D { segments, .. } => assert_eq!(segments.len(), 4),
            _ => panic!("expected Sketch2D"),
        }
        match &doc.nodes[&1].op {
            CsgOp::Extrude {
                sketch, direction, ..
            } => {
                assert_eq!(*sketch, 0);
                assert_eq!(direction.z, 20.0);
            }
            _ => panic!("expected Extrude"),
        }
    }

    #[test]
    fn shell_fillet_chamfer() {
        let cube = adt("Cube", vec![f(10.0), f(10.0), f(10.0)]);
        let shelled = adt("Shell", vec![cube, f(1.0)]);
        let doc = value_to_document(&shelled).unwrap();
        match &doc.nodes[&1].op {
            CsgOp::Shell { thickness, .. } => assert_eq!(*thickness, 1.0),
            _ => panic!("expected Shell"),
        }

        let cube2 = adt("Cube", vec![f(10.0), f(10.0), f(10.0)]);
        let filleted = adt("Fillet", vec![cube2, f(2.0)]);
        let doc2 = value_to_document(&filleted).unwrap();
        match &doc2.nodes[&1].op {
            CsgOp::Fillet { radius, .. } => assert_eq!(*radius, 2.0),
            _ => panic!("expected Fillet"),
        }

        let cube3 = adt("Cube", vec![f(10.0), f(10.0), f(10.0)]);
        let chamfered = adt("Chamfer", vec![cube3, f(1.5)]);
        let doc3 = value_to_document(&chamfered).unwrap();
        match &doc3.nodes[&1].op {
            CsgOp::Chamfer { distance, .. } => assert_eq!(*distance, 1.5),
            _ => panic!("expected Chamfer"),
        }
    }

    #[test]
    fn linear_pattern() {
        let cube = adt("Cube", vec![f(5.0), f(5.0), f(5.0)]);
        let pat = adt(
            "LinearPattern",
            vec![cube, f(20.0), f(0.0), f(0.0), i(5), f(25.0)],
        );
        let doc = value_to_document(&pat).unwrap();
        match &doc.nodes[&1].op {
            CsgOp::LinearPattern { count, spacing, .. } => {
                assert_eq!(*count, 5);
                assert_eq!(*spacing, 25.0);
            }
            _ => panic!("expected LinearPattern"),
        }
    }

    #[test]
    fn circular_pattern() {
        let cube = adt("Cube", vec![f(5.0), f(5.0), f(5.0)]);
        let pat = adt(
            "CircularPattern",
            vec![
                cube,
                f(0.0),
                f(0.0),
                f(0.0), // axis_origin
                f(0.0),
                f(0.0),
                f(1.0), // axis_dir
                i(8),
                f(360.0),
            ],
        );
        let doc = value_to_document(&pat).unwrap();
        match &doc.nodes[&1].op {
            CsgOp::CircularPattern {
                count, angle_deg, ..
            } => {
                assert_eq!(*count, 8);
                assert_eq!(*angle_deg, 360.0);
            }
            _ => panic!("expected CircularPattern"),
        }
    }

    #[test]
    fn complex_csg_tree() {
        // Build: translate(difference(cube, cylinder), 10, 0, 0)
        let cube = adt("Cube", vec![f(50.0), f(30.0), f(5.0)]);
        let cyl = adt("Cylinder", vec![f(3.0), f(10.0)]);
        let diff = adt("Difference", vec![cube, cyl]);
        let fillet = adt("Fillet", vec![diff, f(1.0)]);
        let tr = adt("Translate", vec![fillet, f(10.0), f(0.0), f(0.0)]);
        let doc = value_to_document(&tr).unwrap();
        // cube(0) + cylinder(1) + difference(2) + fillet(3) + translate(4)
        assert_eq!(doc.nodes.len(), 5);
    }

    #[test]
    fn int_to_f64_coercion() {
        // Ints should coerce to f64
        let val = adt("Cube", vec![i(10), i(20), i(30)]);
        let doc = value_to_document(&val).unwrap();
        match &doc.nodes[&0].op {
            CsgOp::Cube { size } => {
                assert_eq!(size.x, 10.0);
                assert_eq!(size.y, 20.0);
                assert_eq!(size.z, 30.0);
            }
            _ => panic!("expected Cube"),
        }
    }

    #[test]
    fn material_in_vec() {
        let mat = adt(
            "Material",
            vec![s("steel"), f(0.7), f(0.7), f(0.7), f(1.0), f(0.3)],
        );
        let entry = adt(
            "SceneEntry",
            vec![adt("Cube", vec![f(10.0), f(10.0), f(10.0)]), s("steel")],
        );
        let vec_val = Value::Vec(vec![mat, entry].into());
        let doc = value_to_document(&vec_val).unwrap();
        assert_eq!(doc.materials.len(), 2); // steel + default
        assert!(doc.materials.contains_key("steel"));
        assert_eq!(doc.roots.len(), 1);
    }

    #[test]
    fn default_material_added() {
        let cube = adt("Cube", vec![f(10.0), f(10.0), f(10.0)]);
        let doc = value_to_document(&cube).unwrap();
        assert!(doc.materials.contains_key("default"));
    }

    #[test]
    fn error_on_wrong_field_count() {
        let val = adt("Cube", vec![f(10.0), f(20.0)]);
        assert!(value_to_document(&val).is_err());
    }

    #[test]
    fn sweep_line_to_document() {
        let line1 = adt("SLine", vec![f(0.0), f(0.0), f(10.0), f(0.0)]);
        let line2 = adt("SLine", vec![f(10.0), f(0.0), f(10.0), f(5.0)]);
        let line3 = adt("SLine", vec![f(10.0), f(5.0), f(0.0), f(5.0)]);
        let line4 = adt("SLine", vec![f(0.0), f(5.0), f(0.0), f(0.0)]);
        let sketch = adt(
            "Sketch",
            vec![
                f(0.0),
                f(0.0),
                f(0.0),
                f(1.0),
                f(0.0),
                f(0.0),
                f(0.0),
                f(1.0),
                f(0.0),
                Value::Vec(vec![line1, line2, line3, line4].into()),
            ],
        );
        let sweep = adt(
            "SweepLine",
            vec![sketch, f(0.0), f(0.0), f(0.0), f(0.0), f(0.0), f(50.0)],
        );
        let doc = value_to_document(&sweep).unwrap();
        assert_eq!(doc.nodes.len(), 2); // sketch + sweep
        match &doc.nodes[&1].op {
            CsgOp::Sweep { path, .. } => match path {
                PathCurve::Line { end, .. } => assert_eq!(end.z, 50.0),
                _ => panic!("expected Line path"),
            },
            _ => panic!("expected Sweep"),
        }
    }

    #[test]
    fn sweep_helix_to_document() {
        let line1 = adt("SLine", vec![f(0.0), f(0.0), f(5.0), f(0.0)]);
        let line2 = adt("SLine", vec![f(5.0), f(0.0), f(5.0), f(3.0)]);
        let line3 = adt("SLine", vec![f(5.0), f(3.0), f(0.0), f(3.0)]);
        let line4 = adt("SLine", vec![f(0.0), f(3.0), f(0.0), f(0.0)]);
        let sketch = adt(
            "Sketch",
            vec![
                f(0.0),
                f(0.0),
                f(0.0),
                f(1.0),
                f(0.0),
                f(0.0),
                f(0.0),
                f(1.0),
                f(0.0),
                Value::Vec(vec![line1, line2, line3, line4].into()),
            ],
        );
        let sweep = adt("SweepHelix", vec![sketch, f(10.0), f(5.0), f(20.0), f(4.0)]);
        let doc = value_to_document(&sweep).unwrap();
        match &doc.nodes[&1].op {
            CsgOp::Sweep { path, .. } => match path {
                PathCurve::Helix { radius, turns, .. } => {
                    assert_eq!(*radius, 10.0);
                    assert_eq!(*turns, 4.0);
                }
                _ => panic!("expected Helix path"),
            },
            _ => panic!("expected Sweep"),
        }
    }

    #[test]
    fn loft_to_document() {
        let mk_sketch = |y: f64| {
            let l1 = adt("SLine", vec![f(0.0), f(0.0), f(10.0), f(0.0)]);
            let l2 = adt("SLine", vec![f(10.0), f(0.0), f(10.0), f(5.0)]);
            let l3 = adt("SLine", vec![f(10.0), f(5.0), f(0.0), f(5.0)]);
            let l4 = adt("SLine", vec![f(0.0), f(5.0), f(0.0), f(0.0)]);
            adt(
                "Sketch",
                vec![
                    f(0.0),
                    f(y),
                    f(0.0),
                    f(1.0),
                    f(0.0),
                    f(0.0),
                    f(0.0),
                    f(0.0),
                    f(1.0),
                    Value::Vec(vec![l1, l2, l3, l4].into()),
                ],
            )
        };
        let loft = adt(
            "Loft",
            vec![Value::Vec(vec![mk_sketch(0.0), mk_sketch(20.0)].into())],
        );
        let doc = value_to_document(&loft).unwrap();
        assert_eq!(doc.nodes.len(), 3); // 2 sketches + loft
        match &doc.nodes[&2].op {
            CsgOp::Loft { sketches, closed } => {
                assert_eq!(sketches.len(), 2);
                assert!(closed.is_none());
            }
            _ => panic!("expected Loft"),
        }
    }

    #[test]
    fn loft_closed_to_document() {
        let mk_sketch = |y: f64| {
            let l1 = adt("SLine", vec![f(0.0), f(0.0), f(10.0), f(0.0)]);
            let l2 = adt("SLine", vec![f(10.0), f(0.0), f(0.0), f(0.0)]);
            adt(
                "Sketch",
                vec![
                    f(0.0),
                    f(y),
                    f(0.0),
                    f(1.0),
                    f(0.0),
                    f(0.0),
                    f(0.0),
                    f(0.0),
                    f(1.0),
                    Value::Vec(vec![l1, l2].into()),
                ],
            )
        };
        let loft = adt(
            "LoftClosed",
            vec![Value::Vec(
                vec![mk_sketch(0.0), mk_sketch(10.0), mk_sketch(20.0)].into(),
            )],
        );
        let doc = value_to_document(&loft).unwrap();
        match &doc.nodes[&3].op {
            CsgOp::Loft { sketches, closed } => {
                assert_eq!(sketches.len(), 3);
                assert_eq!(*closed, Some(true));
            }
            _ => panic!("expected Loft"),
        }
    }

    #[test]
    fn error_on_unknown_tag() {
        let val = adt("UnknownShape", vec![f(1.0)]);
        assert!(value_to_document(&val).is_err());
    }

    #[test]
    fn assembly_to_document() {
        // Build: Assembly([parts], [instances], [joints], ground)
        let parts = Value::Vec(
            vec![
                adt(
                    "PartEntry",
                    vec![
                        s("base"),
                        adt("Cylinder", vec![f(40.0), f(30.0)]),
                        s("steel"),
                    ],
                ),
                adt(
                    "PartEntry",
                    vec![
                        s("arm1"),
                        adt("Cube", vec![f(80.0), f(20.0), f(20.0)]),
                        s("aluminum"),
                    ],
                ),
            ]
            .into(),
        );
        let instances = Value::Vec(
            vec![
                adt(
                    "InstanceEntry",
                    vec![s("base-inst"), s("base"), f(0.0), f(0.0), f(0.0)],
                ),
                adt(
                    "InstanceEntry",
                    vec![s("arm1-inst"), s("arm1"), f(0.0), f(0.0), f(30.0)],
                ),
            ]
            .into(),
        );
        let joints = Value::Vec(
            vec![adt(
                "RevoluteJoint",
                vec![
                    s("shoulder"),
                    f(0.0),
                    f(1.0),
                    f(0.0), // axis
                    f(-90.0),
                    f(90.0),        // limits
                    s("base-inst"), // parent
                    f(0.0),
                    f(0.0),
                    f(25.0),        // parent anchor
                    s("arm1-inst"), // child
                    f(0.0),
                    f(0.0),
                    f(0.0), // child anchor
                ],
            )]
            .into(),
        );
        let ground = s("base-inst");
        let assembly = adt("Assembly", vec![parts, instances, joints, ground]);

        let doc = value_to_document(&assembly).unwrap();

        // Verify part_defs
        let pd = doc.part_defs.as_ref().unwrap();
        assert_eq!(pd.len(), 2);
        assert!(pd.contains_key("base"));
        assert!(pd.contains_key("arm1"));
        assert_eq!(pd["base"].default_material, Some("steel".into()));

        // Verify instances
        let insts = doc.instances.as_ref().unwrap();
        assert_eq!(insts.len(), 2);
        assert_eq!(insts[0].id, "base-inst");
        assert_eq!(insts[0].part_def_id, "base");
        assert!(insts[0].transform.is_none()); // all zeros → None
        assert_eq!(insts[1].id, "arm1-inst");
        assert!(insts[1].transform.is_some()); // z=30 → Some

        // Verify joints
        let jts = doc.joints.as_ref().unwrap();
        assert_eq!(jts.len(), 1);
        assert_eq!(jts[0].name, Some("shoulder".into()));
        assert_eq!(jts[0].parent_instance_id, Some("base-inst".into()));
        assert_eq!(jts[0].child_instance_id, "arm1-inst");
        match &jts[0].kind {
            JointKind::Revolute { axis, limits, .. } => {
                assert_eq!(axis.y, 1.0);
                assert_eq!(*limits, Some((-90.0, 90.0)));
            }
            _ => panic!("expected Revolute"),
        }

        // Verify ground
        assert_eq!(doc.ground_instance_id, Some("base-inst".into()));

        // Verify geometry nodes were created
        assert!(doc.nodes.len() >= 2); // cylinder + cube
    }

    #[test]
    fn assembly_with_multiple_joint_types() {
        let parts = Value::Vec(
            vec![
                adt(
                    "PartEntry",
                    vec![
                        s("a"),
                        adt("Cube", vec![f(10.0), f(10.0), f(10.0)]),
                        s("default"),
                    ],
                ),
                adt(
                    "PartEntry",
                    vec![
                        s("b"),
                        adt("Cube", vec![f(10.0), f(10.0), f(10.0)]),
                        s("default"),
                    ],
                ),
                adt(
                    "PartEntry",
                    vec![
                        s("c"),
                        adt("Cube", vec![f(10.0), f(10.0), f(10.0)]),
                        s("default"),
                    ],
                ),
            ]
            .into(),
        );
        let instances = Value::Vec(
            vec![
                adt(
                    "InstanceEntry",
                    vec![s("a-inst"), s("a"), f(0.0), f(0.0), f(0.0)],
                ),
                adt(
                    "InstanceEntry",
                    vec![s("b-inst"), s("b"), f(0.0), f(0.0), f(0.0)],
                ),
                adt(
                    "InstanceEntry",
                    vec![s("c-inst"), s("c"), f(0.0), f(0.0), f(0.0)],
                ),
            ]
            .into(),
        );
        let joints = Value::Vec(
            vec![
                adt(
                    "FixedJoint",
                    vec![
                        s("fix"),
                        s("a-inst"),
                        f(0.0),
                        f(0.0),
                        f(5.0),
                        s("b-inst"),
                        f(0.0),
                        f(0.0),
                        f(0.0),
                    ],
                ),
                adt(
                    "BallJoint",
                    vec![
                        s("ball"),
                        s("b-inst"),
                        f(0.0),
                        f(0.0),
                        f(5.0),
                        s("c-inst"),
                        f(0.0),
                        f(0.0),
                        f(0.0),
                    ],
                ),
            ]
            .into(),
        );
        let assembly = adt("Assembly", vec![parts, instances, joints, s("a-inst")]);
        let doc = value_to_document(&assembly).unwrap();

        let jts = doc.joints.as_ref().unwrap();
        assert_eq!(jts.len(), 2);
        assert!(matches!(jts[0].kind, JointKind::Fixed));
        assert!(matches!(jts[1].kind, JointKind::Ball));
    }

    #[test]
    fn revolve_to_document() {
        let line1 = adt("SLine", vec![f(5.0), f(0.0), f(10.0), f(0.0)]);
        let line2 = adt("SLine", vec![f(10.0), f(0.0), f(10.0), f(20.0)]);
        let line3 = adt("SLine", vec![f(10.0), f(20.0), f(5.0), f(20.0)]);
        let line4 = adt("SLine", vec![f(5.0), f(20.0), f(5.0), f(0.0)]);
        let sketch = adt(
            "Sketch",
            vec![
                f(0.0),
                f(0.0),
                f(0.0),
                f(1.0),
                f(0.0),
                f(0.0),
                f(0.0),
                f(1.0),
                f(0.0),
                Value::Vec(vec![line1, line2, line3, line4].into()),
            ],
        );
        let revolve = adt(
            "Revolve",
            vec![
                sketch,
                f(0.0),
                f(0.0),
                f(0.0), // axis origin
                f(0.0),
                f(1.0),
                f(0.0),   // axis dir
                f(360.0), // angle
            ],
        );
        let doc = value_to_document(&revolve).unwrap();
        assert_eq!(doc.nodes.len(), 2);
        match &doc.nodes[&1].op {
            CsgOp::Revolve { angle_deg, .. } => assert_eq!(*angle_deg, 360.0),
            _ => panic!("expected Revolve"),
        }
    }

    #[test]
    fn arc_sketch_segment() {
        let arc = adt(
            "SArc",
            vec![
                f(0.0),
                f(0.0),
                f(10.0),
                f(0.0),
                f(5.0),
                f(0.0),
                Value::Bool(true),
            ],
        );
        let sketch = adt(
            "Sketch",
            vec![
                f(0.0),
                f(0.0),
                f(0.0),
                f(1.0),
                f(0.0),
                f(0.0),
                f(0.0),
                f(1.0),
                f(0.0),
                Value::Vec(vec![arc].into()),
            ],
        );
        let extrude = adt("Extrude", vec![sketch, f(0.0), f(0.0), f(5.0)]);
        let doc = value_to_document(&extrude).unwrap();
        match &doc.nodes[&0].op {
            CsgOp::Sketch2D { segments, .. } => {
                assert_eq!(segments.len(), 1);
                match &segments[0] {
                    SketchSegment2D::Arc { ccw, .. } => assert!(*ccw),
                    _ => panic!("expected Arc"),
                }
            }
            _ => panic!("expected Sketch2D"),
        }
    }
}
