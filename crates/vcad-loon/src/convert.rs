use std::collections::HashMap;

use loon_lang::interp::Value;
use vcad_ir::ecad;
use vcad_ir::*;

/// Walk a loon `Value::Adt` tree and produce a vcad-ir `Document`.
pub fn value_to_document(value: &Value) -> Result<Document, String> {
    let mut ctx = ConvertCtx::new();

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
            },
        );
    }

    Ok(ctx.doc)
}

/// Process a single item from a Vec (can be SceneEntry, Material, or bare Solid).
fn merge_value_into_doc(ctx: &mut ConvertCtx, value: &Value) -> Result<(), String> {
    match value {
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

fn is_solid_tag(tag: &str) -> bool {
    matches!(
        tag,
        "Cube"
            | "Cylinder"
            | "Sphere"
            | "Cone"
            | "Empty"
            | "Union"
            | "Difference"
            | "Intersection"
            | "Translate"
            | "Rotate"
            | "Scale"
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
    )
}

struct ConvertCtx {
    doc: Document,
    next_id: NodeId,
}

impl ConvertCtx {
    fn new() -> Self {
        Self {
            doc: Document::default(),
            next_id: 0,
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

    fn convert_solid(&mut self, value: &Value) -> Result<NodeId, String> {
        let (tag, fields) = match value {
            Value::Adt(tag, fields) => (tag.as_str(), fields.as_slice()),
            _ => return Err(format!("expected Solid ADT, got {value}")),
        };

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
                let child = self.convert_solid(&fields[0])?;
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
                let child = self.convert_solid(&fields[0])?;
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
        Value::Adt(tag.into(), fields.into())
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
            JointKind::Revolute { axis, limits } => {
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
