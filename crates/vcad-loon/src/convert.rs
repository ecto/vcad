use loon_lang::interp::Value;
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
                Value::Str(s) => s.clone(),
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
        // Vec of entries
        Value::Vec(items) => {
            for item in items {
                merge_value_into_doc(&mut ctx, item)?;
            }
        }
        _ => return Err(format!("expected Solid, SceneEntry, or Vec, got {value}")),
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
                Value::Str(s) => s.clone(),
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
        Value::Adt(tag, _) if is_solid_tag(tag) => {
            let root_id = ctx.convert_solid(value)?;
            ctx.doc.roots.push(SceneEntry {
                root: root_id,
                material: "default".into(),
                visible: None,
            });
        }
        _ => return Err(format!("expected SceneEntry, Material, or Solid in Vec, got {value}")),
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
            Value::Str(s) => Ok(s.clone()),
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

            _ => return Err(format!("unknown Solid variant: {tag}")),
        };

        Ok(self.insert_node(op))
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

        items.iter().map(|item| self.convert_sketch_seg(item)).collect()
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
        Value::Str(v.to_string())
    }
    fn adt(tag: &str, fields: Vec<Value>) -> Value {
        Value::Adt(tag.to_string(), fields)
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
            CsgOp::Cylinder {
                radius, height, ..
            } => {
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
            vec![
                adt("Cube", vec![f(10.0), f(10.0), f(10.0)]),
                s("steel"),
            ],
        );
        let entry2 = adt(
            "SceneEntry",
            vec![adt("Sphere", vec![f(5.0)]), s("glass")],
        );
        let vec_val = Value::Vec(vec![entry1, entry2]);
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
                f(0.0), f(0.0), f(0.0), // origin
                f(1.0), f(0.0), f(0.0), // x_dir
                f(0.0), f(1.0), f(0.0), // y_dir
                Value::Vec(vec![line1, line2, line3, line4]),
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
            CsgOp::LinearPattern {
                count, spacing, ..
            } => {
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
                f(0.0), f(0.0), f(0.0), // axis_origin
                f(0.0), f(0.0), f(1.0), // axis_dir
                i(8),
                f(360.0),
            ],
        );
        let doc = value_to_document(&pat).unwrap();
        match &doc.nodes[&1].op {
            CsgOp::CircularPattern {
                count,
                angle_deg,
                ..
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
        let vec_val = Value::Vec(vec![mat, entry]);
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
    fn error_on_unknown_tag() {
        let val = adt("UnknownShape", vec![f(1.0)]);
        assert!(value_to_document(&val).is_err());
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
                f(0.0), f(0.0), f(0.0),
                f(1.0), f(0.0), f(0.0),
                f(0.0), f(1.0), f(0.0),
                Value::Vec(vec![line1, line2, line3, line4]),
            ],
        );
        let revolve = adt(
            "Revolve",
            vec![
                sketch,
                f(0.0), f(0.0), f(0.0), // axis origin
                f(0.0), f(1.0), f(0.0), // axis dir
                f(360.0),               // angle
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
                f(0.0), f(0.0), f(10.0), f(0.0), f(5.0), f(0.0),
                Value::Bool(true),
            ],
        );
        let sketch = adt(
            "Sketch",
            vec![
                f(0.0), f(0.0), f(0.0),
                f(1.0), f(0.0), f(0.0),
                f(0.0), f(1.0), f(0.0),
                Value::Vec(vec![arc]),
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
