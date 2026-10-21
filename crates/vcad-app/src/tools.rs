//! AI tool calls lowered to [`DocCommand`]s.
//!
//! The chat tools (`create`, `translate`, …) are a convenience vocabulary
//! shaped for language models: IR-style params (`size: {x, y, z}`), part ids
//! called `child` / `left` / `right`, and `parent_part_id` for modifiers.
//! [`lower`] turns one call into the [`DocCommand`] the engine applies, so
//! every host runs the same tool semantics instead of re-implementing them.
//!
//! Lowering reads the document where a tool needs it: rotate pivots about
//! the part's evaluated world bbox centre, and refs are checked by `apply`.
//! `place` resolves its anchors against evaluated bboxes the same way. Tools
//! still handled by hosts (reads, bulk edits) return
//! [`CommandError::Unsupported`].

use serde_json::Value;

use crate::command::{CommandError, DocCommand};
use crate::feature::{BooleanType, FeatureInput};
use crate::DocumentApi;

/// Lower one tool call into a command. `tool` is the top-level tool name
/// (`create`, `translate`, `linear_pattern`, …); for `create`, `args.type`
/// picks the feature kind and `args.params` carries its parameters.
pub fn lower(tool: &str, args: &Value, doc: &DocumentApi) -> Result<DocCommand, CommandError> {
    match tool {
        "create" => {
            let kind = str_arg(args, "type", "create")?;
            let kind = match kind {
                // Models sometimes split the digit off (`sketch_2_d`).
                "sketch_2_d" => "sketch_2d",
                k => k,
            };
            let empty = Value::Object(Default::default());
            let params = args.get("params").unwrap_or(&empty);
            let parent = args.get("parent_part_id").and_then(Value::as_str);
            let name = args.get("name").and_then(Value::as_str).map(str::to_string);
            lower_create(kind, params, parent, name, doc)
        }
        "linear_pattern" | "circular_pattern" | "mirror" | "translate" | "rotate" | "scale"
        | "union" | "difference" | "intersection" | "fillet" | "chamfer" | "shell" | "tube"
        | "polyline_tube" => {
            let name = args.get("name").and_then(Value::as_str).map(str::to_string);
            lower_create(tool, args, None, name, doc)
        }
        "delete" => Ok(DocCommand::DeleteFeature {
            id: str_arg(args, "part_id", tool)?.to_string(),
        }),
        "set_material" => lower_set_material(args, doc),
        "update" => lower_update(args, doc),
        "place" => lower_place(args, doc),
        "place_part" => lower_place_part(args),
        other => crate::doc_edits::lower(other, args, doc)
            .or_else(|| crate::ecad_tools::lower(other, args, doc))
            .unwrap_or_else(|| Err(unsupported(other))),
    }
}

/// `place_part {path, version, params?, name?}`: insert a parts-library
/// instance. The host resolves `version` and default params from its parts
/// manifest; a `position` is applied by the host as a follow-up `translate`
/// on the created id.
fn lower_place_part(args: &Value) -> Result<DocCommand, CommandError> {
    let path = str_arg(args, "path", "place_part")?;
    if path.is_empty() {
        return Err(invalid("place_part: `path` must not be empty".into()));
    }
    let version = str_arg(args, "version", "place_part")?.to_string();
    let params = match args.get("params") {
        None | Some(Value::Null) => Value::Object(Default::default()),
        Some(v @ Value::Object(_)) => v.clone(),
        Some(_) => return Err(invalid("place_part: `params` must be an object".into())),
    };
    let name = args.get("name").and_then(Value::as_str).map(str::to_string);
    Ok(DocCommand::AddFeature {
        feature: FeatureInput::PartInstance {
            path: path.to_string(),
            version,
            params_json: params.to_string(),
        },
        name,
    })
}

/// Named bbox anchors `place` accepts.
const ANCHOR_NAMES: [&str; 9] = [
    "center", "min", "max", "top", "bottom", "front", "back", "left", "right",
];

/// `place {part_id, from?, to}`: translate the part so its `from` anchor
/// (default `center`) lands on `to`. An anchor is a named bbox anchor on
/// this part, `{part, anchor}` on another part, or a world `{x, y, z}`.
/// Only the part's offset changes, so rotation and scale are kept.
fn lower_place(args: &Value, doc: &DocumentApi) -> Result<DocCommand, CommandError> {
    let id = str_arg(args, "part_id", "place")?.to_string();
    if doc.stable_ids().resolve(&id).is_none() {
        return Err(CommandError::UnknownId { id });
    }
    let center = Value::String("center".into());
    let from = args.get("from").filter(|v| !v.is_null()).unwrap_or(&center);
    let to = args.get("to").filter(|v| !v.is_null()).ok_or_else(|| {
        invalid("place requires a `to` anchor ({x,y,z}, named anchor, or {part, anchor})".into())
    })?;
    let from_world = resolve_anchor(from, &id, doc, "from")?;
    let to_world = resolve_anchor(to, &id, doc, "to")?;
    let offset = doc.feature_vec3(&id, "offset").unwrap_or([0.0; 3]);
    Ok(DocCommand::SetTranslation {
        id,
        offset: [
            offset[0] + to_world[0] - from_world[0],
            offset[1] + to_world[1] - from_world[1],
            offset[2] + to_world[2] - from_world[2],
        ],
    })
}

fn resolve_anchor(
    anchor: &Value,
    self_id: &str,
    doc: &DocumentApi,
    which: &str,
) -> Result<[f64; 3], CommandError> {
    match anchor {
        Value::String(name) => anchor_on_part(self_id, name, doc, which),
        Value::Object(o) if o.contains_key("part") => {
            let part = str_arg(anchor, "part", &format!("place `{which}`"))?;
            if doc.stable_ids().resolve(part).is_none() {
                return Err(CommandError::UnknownId {
                    id: part.to_string(),
                });
            }
            let name = str_arg(anchor, "anchor", &format!("place `{which}`"))?;
            anchor_on_part(part, name, doc, which)
        }
        Value::Object(_) => vec3(&serde_json::json!({ which: anchor }), which, "place"),
        _ => Err(invalid(format!(
            "place `{which}` must be a named anchor, {{x, y, z}}, or {{part, anchor}}"
        ))),
    }
}

fn anchor_on_part(
    part: &str,
    name: &str,
    doc: &DocumentApi,
    which: &str,
) -> Result<[f64; 3], CommandError> {
    if !ANCHOR_NAMES.contains(&name) {
        return Err(invalid(format!(
            "place `{which}`: unknown anchor {name:?} (expected one of {})",
            ANCHOR_NAMES.join(", ")
        )));
    }
    let (lo, hi) = doc.part_world_bbox(part).ok_or_else(|| {
        invalid(format!(
            "place `{which}`: part {part} has no evaluated geometry to anchor on"
        ))
    })?;
    let c = [
        (lo[0] + hi[0]) / 2.0,
        (lo[1] + hi[1]) / 2.0,
        (lo[2] + hi[2]) / 2.0,
    ];
    Ok(match name {
        "min" => lo,
        "max" => hi,
        "top" => [c[0], c[1], hi[2]],
        "bottom" => [c[0], c[1], lo[2]],
        "front" => [c[0], lo[1], c[2]],
        "back" => [c[0], hi[1], c[2]],
        "left" => [lo[0], c[1], c[2]],
        "right" => [hi[0], c[1], c[2]],
        _ => c,
    })
}

/// `set_material` with one of `part_id`, `part_ids[]` or
/// `selector {by: kind|name_prefix|name_contains|name_equals, value}`
/// (case-insensitive). Applied as one batch, so an unknown id refuses the
/// whole call instead of half-painting the model.
fn lower_set_material(args: &Value, doc: &DocumentApi) -> Result<DocCommand, CommandError> {
    let material = str_arg(args, "material", "set_material")?.to_string();
    let ids: Vec<String> = if let Some(ids) = args
        .get("part_ids")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
    {
        ids.iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    } else if let Some(id) = args.get("part_id").and_then(Value::as_str) {
        vec![id.to_string()]
    } else if let Some(sel) = args.get("selector") {
        let by = str_arg(sel, "by", "set_material selector")?;
        let value = sel
            .get("value")
            .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string))
            .ok_or_else(|| invalid("set_material selector requires `value`".into()))?
            .to_lowercase();
        let ids: Vec<String> = parts_json(doc)
            .into_iter()
            .filter(|p| {
                let field = |k: &str| {
                    p.get(k)
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_lowercase()
                };
                match by {
                    "kind" => field("kind") == value,
                    "name_prefix" => field("name").starts_with(&value),
                    "name_contains" => field("name").contains(&value),
                    "name_equals" => field("name") == value,
                    _ => false,
                }
            })
            .filter_map(|p| p.get("id").and_then(Value::as_str).map(str::to_string))
            .collect();
        if ids.is_empty() {
            return Err(invalid(format!(
                "set_material: selector {sel} matched zero parts"
            )));
        }
        ids
    } else {
        return Err(invalid(
            "set_material requires one of: part_id, part_ids[], or selector{by,value}".into(),
        ));
    };
    let mut commands: Vec<DocCommand> = ids
        .into_iter()
        .map(|id| DocCommand::SetMaterial {
            id,
            material: material.clone(),
        })
        .collect();
    Ok(if commands.len() == 1 {
        commands.remove(0)
    } else {
        DocCommand::Batch { commands }
    })
}

/// `update {node_id, params}`: set raw params on the part that owns
/// `node_id` (an IR node id or the part's own id). Numbers, booleans,
/// strings and `{x, y, z}` vectors map to CRDT values.
fn lower_update(args: &Value, doc: &DocumentApi) -> Result<DocCommand, CommandError> {
    let node = args
        .get("node_id")
        .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string))
        .ok_or_else(|| invalid("update requires `node_id`".into()))?;
    let params = args
        .get("params")
        .and_then(Value::as_object)
        .filter(|m| !m.is_empty())
        .ok_or_else(|| invalid("update requires non-empty `params`".into()))?;
    let node_num = node.parse::<u64>().ok();
    let is_node = |v: &Value| node_num.is_some() && v.as_u64() == node_num;
    let owner = parts_json(doc)
        .into_iter()
        .find(|p| {
            p.get("id").and_then(Value::as_str) == Some(node.as_str())
                || p.as_object().is_some_and(|m| {
                    m.iter().any(|(k, v)| {
                        k.ends_with("NodeId")
                            && (is_node(v) || v.as_array().is_some_and(|a| a.iter().any(is_node)))
                    })
                })
        })
        .and_then(|p| p.get("id").and_then(Value::as_str).map(str::to_string))
        .ok_or_else(|| CommandError::UnknownId { id: node.clone() })?;
    // The tool speaks in the create shape (`size: {x,y,z}`); features store
    // flat keys (`size_x`, …). Map onto the keys the feature actually has and
    // refuse anything else — writing an unknown key "succeeds" and changes
    // nothing, which is exactly what this used to do for every cube resize.
    let existing: std::collections::HashSet<String> = doc
        .stable_ids()
        .resolve(&owner)
        .and_then(|fid| doc.crdt().get_feature(fid))
        .map(|f| f.params.keys().cloned().collect())
        .unwrap_or_default();
    let scalar = |v: &Value| -> Option<vcad_crdt::Value> {
        Some(match v {
            Value::Number(n) => vcad_crdt::Value::F64(n.as_f64()?),
            Value::Bool(b) => vcad_crdt::Value::Bool(*b),
            Value::String(s) => vcad_crdt::Value::String(s.clone()),
            _ => return None,
        })
    };
    let mut commands = Vec::new();
    for (key, v) in params {
        let set = |key: String, value| DocCommand::SetParam {
            id: owner.clone(),
            key,
            value,
        };
        if existing.contains(key) {
            let value = scalar(v)
                .or_else(|| {
                    Some(vcad_crdt::Value::Vec3([
                        v.get("x")?.as_f64()?,
                        v.get("y")?.as_f64()?,
                        v.get("z")?.as_f64()?,
                    ]))
                })
                .ok_or_else(|| invalid(format!("update: unsupported value for `{key}`")))?;
            commands.push(set(key.clone(), value));
        } else if let Some(obj) = v.as_object() {
            let mut any = false;
            for (axis, n) in obj {
                let flat = format!("{key}_{axis}");
                if !existing.contains(&flat) {
                    return Err(invalid(format!(
                        "update: `{key}.{axis}` is not a parameter of this part"
                    )));
                }
                let n = scalar(n).ok_or_else(|| {
                    invalid(format!("update: unsupported value for `{key}.{axis}`"))
                })?;
                commands.push(set(flat, n));
                any = true;
            }
            if !any {
                return Err(invalid(format!("update: `{key}` is empty")));
            }
        } else {
            return Err(invalid(format!(
                "update: `{key}` is not a parameter of this part"
            )));
        }
    }
    Ok(DocCommand::Batch { commands })
}

fn parts_json(doc: &DocumentApi) -> Vec<Value> {
    doc.get_state()
        .parts
        .iter()
        .filter_map(|p| serde_json::to_value(p).ok())
        .collect()
}

fn lower_create(
    kind: &str,
    p: &Value,
    parent: Option<&str>,
    name: Option<String>,
    doc: &DocumentApi,
) -> Result<DocCommand, CommandError> {
    if let Some(cmd) = lower_geometry(kind, p, name.clone(), doc)? {
        return Ok(cmd);
    }
    let add = |feature| Ok(DocCommand::AddFeature { feature, name });
    match kind {
        "cube" => {
            let size = p.get("size");
            add(FeatureInput::Cube {
                size_x: size.map_or(Ok(20.0), |s| num(s, "x", "cube size"))?,
                size_y: size.map_or(Ok(20.0), |s| num(s, "y", "cube size"))?,
                size_z: size.map_or(Ok(20.0), |s| num(s, "z", "cube size"))?,
            })
        }
        "cylinder" => add(FeatureInput::Cylinder {
            radius: num_or(p, "radius", 10.0)?,
            height: num_or(p, "height", 20.0)?,
            segments: Some(segments(p)?),
        }),
        "sphere" => add(FeatureInput::Sphere {
            radius: num_or(p, "radius", 10.0)?,
            segments: Some(segments(p)?),
        }),
        "cone" => add(FeatureInput::Cone {
            radius_bottom: num(p, "radius_bottom", "cone")?,
            radius_top: num_or(p, "radius_top", 0.0)?,
            height: num(p, "height", "cone")?,
            segments: Some(segments(p)?),
        }),
        "union" | "difference" | "intersection" => add(FeatureInput::Boolean {
            boolean_type: BooleanType::from_str_lossy(kind),
            input_a: str_arg(p, "left", kind)?.to_string(),
            input_b: str_arg(p, "right", kind)?.to_string(),
        }),
        "fillet" => add(FeatureInput::Fillet {
            input: target(p, parent, kind)?,
            radius: positive(p, "radius", kind)?,
        }),
        "chamfer" => add(FeatureInput::Chamfer {
            input: target(p, parent, kind)?,
            distance: positive(p, "distance", kind)?,
        }),
        "shell" => add(FeatureInput::Shell {
            input: target(p, parent, kind)?,
            thickness: positive(p, "thickness", kind)?,
        }),
        "linear_pattern" => add(FeatureInput::LinearPattern {
            input: str_arg(p, "child", kind)?.to_string(),
            direction: vec3(p, "direction", kind)?,
            count: count(p, kind)?,
            spacing: num(p, "spacing", kind)?,
        }),
        "circular_pattern" => add(FeatureInput::CircularPattern {
            input: str_arg(p, "child", kind)?.to_string(),
            axis_origin: vec3(p, "axis_origin", kind)?,
            axis_dir: vec3(p, "axis_dir", kind)?,
            count: count(p, kind)?,
            angle_deg: num(p, "angle_deg", kind)?,
        }),
        "mirror" => {
            let plane = str_arg(p, "plane", kind)?;
            if !matches!(plane, "XY" | "XZ" | "YZ") {
                return Err(invalid(format!(
                    "mirror requires plane: 'XY' | 'XZ' | 'YZ', got {plane:?}"
                )));
            }
            add(FeatureInput::Mirror {
                input: str_arg(p, "child", kind)?.to_string(),
                plane: plane.to_string(),
            })
        }
        "translate" => Ok(DocCommand::SetTranslation {
            id: str_arg(p, "child", kind)?.to_string(),
            offset: vec3(p, "offset", kind)?,
        }),
        "scale" => Ok(DocCommand::SetScale {
            id: str_arg(p, "child", kind)?.to_string(),
            factor: vec3(p, "factor", kind)?,
        }),
        other => Err(unsupported(other)),
    }
}

/// Tools that need sketches, paths or evaluated geometry. `Ok(None)` means
/// "not one of these".
fn lower_geometry(
    kind: &str,
    p: &Value,
    name: Option<String>,
    doc: &DocumentApi,
) -> Result<Option<DocCommand>, CommandError> {
    let add = |feature| {
        Ok(Some(DocCommand::AddFeature {
            feature,
            name: name.clone(),
        }))
    };
    match kind {
        "rotate" => {
            let id = str_arg(p, "child", kind)?.to_string();
            let angles = vec3(p, "angles", kind)?;
            let pivot = match p.get("pivot") {
                Some(Value::String(s)) if s == "origin" => None,
                Some(v @ Value::Object(_)) => {
                    Some(vec3(&serde_json::json!({ "pivot": v }), "pivot", kind)?)
                }
                _ => doc.part_world_bbox(&id).map(|(lo, hi)| {
                    [
                        (lo[0] + hi[0]) / 2.0,
                        (lo[1] + hi[1]) / 2.0,
                        (lo[2] + hi[2]) / 2.0,
                    ]
                }),
            };
            let Some(pivot) = pivot else {
                return Ok(Some(DocCommand::SetRotation { id, angles }));
            };
            // Keep `pivot` fixed in world space: the part's transform is
            // T(offset)·R(angles), so solve for the offset that maps the
            // pivot's local position back onto itself under the new angles.
            let t_old = doc.feature_vec3(&id, "offset").unwrap_or([0.0; 3]);
            let r_old = doc.feature_vec3(&id, "rotation").unwrap_or([0.0; 3]);
            let local = inverse_rotate(sub(pivot, t_old), r_old);
            let after = rotate(local, angles);
            Ok(Some(DocCommand::Batch {
                commands: vec![
                    DocCommand::SetRotation {
                        id: id.clone(),
                        angles,
                    },
                    DocCommand::SetTranslation {
                        id,
                        offset: sub(pivot, after),
                    },
                ],
            }))
        }
        "tube" => add(tube(p)?),
        "polyline_tube" => {
            let points = p
                .get("points")
                .and_then(Value::as_array)
                .filter(|a| a.len() >= 2)
                .ok_or_else(|| {
                    invalid("polyline_tube requires points[] with at least 2 entries".into())
                })?;
            let base = name.clone().unwrap_or_else(|| "Tube".into());
            let mut commands = Vec::with_capacity(points.len() - 1);
            for (i, w) in points.windows(2).enumerate() {
                let seg = serde_json::json!({
                    "start": w[0], "end": w[1],
                    "radius": p.get("radius").cloned().unwrap_or(Value::Null),
                });
                let feature =
                    tube(&seg).map_err(|e| invalid(format!("polyline_tube segment {i}: {e}")))?;
                commands.push(DocCommand::AddFeature {
                    feature,
                    name: Some(format!("{base} {}", i + 1)),
                });
            }
            Ok(Some(DocCommand::Batch { commands }))
        }
        "extrude" => {
            let direction = vec3(p, "direction", kind)?;
            let depth = norm(direction);
            if depth < 1e-9 {
                return Err(invalid("extrude direction must be non-zero".into()));
            }
            add(FeatureInput::Extrude {
                sketch: sketch_json(p.get("sketch"), kind)?,
                depth,
                direction: scale(direction, 1.0 / depth),
                twist_angle: opt_num(p, "twist_angle")?,
                scale_end: opt_num(p, "scale_end")?,
            })
        }
        "revolve" => add(FeatureInput::Revolve {
            sketch: sketch_json(p.get("sketch"), kind)?,
            axis_origin: vec3(p, "axis_origin", kind)?,
            axis_dir: vec3(p, "axis_dir", kind)?,
            angle_deg: num(p, "angle_deg", kind)?,
        }),
        "sweep" => {
            let sketch = sketch_json(p.get("sketch"), kind)?;
            let path = p
                .get("path")
                .ok_or_else(|| invalid("sweep requires `path`".into()))?;
            let sweep = |path: String| FeatureInput::Sweep {
                sketch: sketch.clone(),
                path: Some(path),
                twist_angle: opt_num(p, "twist_angle").ok().flatten(),
                scale_start: opt_num(p, "scale_start").ok().flatten(),
                scale_end: opt_num(p, "scale_end").ok().flatten(),
            };
            // Polyline is a tool convenience the kernel doesn't have: one
            // Line sweep per segment, kept as separate parts.
            if path.get("type").and_then(Value::as_str) == Some("Polyline") {
                let points = path
                    .get("points")
                    .and_then(Value::as_array)
                    .filter(|a| a.len() >= 2)
                    .ok_or_else(|| {
                        invalid("sweep Polyline requires points[] with ≥2 entries".into())
                    })?;
                let commands = points
                    .windows(2)
                    .map(|w| {
                        let line =
                            serde_json::json!({ "type": "Line", "start": w[0], "end": w[1] });
                        Ok(DocCommand::AddFeature {
                            feature: sweep(path_json(&line)?),
                            name: name.clone(),
                        })
                    })
                    .collect::<Result<_, CommandError>>()?;
                return Ok(Some(DocCommand::Batch { commands }));
            }
            add(sweep(path_json(path)?))
        }
        "loft" => {
            let sketches = p
                .get("sketches")
                .and_then(Value::as_array)
                .filter(|a| a.len() >= 2)
                .ok_or_else(|| invalid("loft requires at least 2 sketch profiles".into()))?;
            add(FeatureInput::Loft {
                profiles: sketches
                    .iter()
                    .map(|s| sketch_json(Some(s), kind))
                    .collect::<Result<_, _>>()?,
                closed: p.get("closed").and_then(Value::as_bool),
            })
        }
        _ => Ok(None),
    }
}

/// A cylindrical pipe between two world points: a circular profile at
/// `start`, perpendicular to the segment, swept along a straight line.
fn tube(p: &Value) -> Result<FeatureInput, CommandError> {
    let start = vec3(p, "start", "tube")?;
    let end = vec3(p, "end", "tube")?;
    let radius = num_or(p, "radius", 5.0)?;
    if radius <= 0.0 {
        return Err(invalid("tube radius must be positive".into()));
    }
    let dir = sub(end, start);
    let len = norm(dir);
    if len < 1e-6 {
        return Err(invalid("tube start and end are coincident".into()));
    }
    let d = scale(dir, 1.0 / len);
    let reference = if d[2].abs() < 0.9 {
        [0.0, 0.0, 1.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let y_dir = normalize(cross(d, reference));
    let x_dir = normalize(cross(y_dir, d));
    let v = |a: [f64; 3]| serde_json::json!({ "x": a[0], "y": a[1], "z": a[2] });
    let arc = |sx: f64, ex: f64| {
        serde_json::json!({
            "type": "Arc", "start": { "x": sx, "y": 0.0 }, "end": { "x": ex, "y": 0.0 },
            "center": { "x": 0.0, "y": 0.0 }, "ccw": false
        })
    };
    let sketch = serde_json::json!({
        "origin": v(start), "x_dir": v(x_dir), "y_dir": v(y_dir),
        "segments": [arc(radius, -radius), arc(-radius, radius)],
    });
    let line = serde_json::json!({ "type": "Line", "start": v(start), "end": v(end) });
    Ok(FeatureInput::Sweep {
        sketch: sketch_json(Some(&sketch), "tube")?,
        path: Some(path_json(&line)?),
        twist_angle: None,
        scale_start: None,
        scale_end: None,
    })
}

/// Validate an inline sketch (`{origin, x_dir, y_dir, segments}`) as a
/// closed `Sketch2D` and serialize it the way the CRDT stores sketches.
fn sketch_json(sketch: Option<&Value>, ctx: &str) -> Result<String, CommandError> {
    let s = sketch.filter(|s| s.is_object()).ok_or_else(|| {
        invalid(format!(
            "{ctx} requires an inline sketch {{origin, x_dir, y_dir, segments}}"
        ))
    })?;
    let segments = s
        .get("segments")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("sketch must have at least one segment".into()))?;
    if segments.len() < 2 {
        return Err(invalid(
            "sketch must have at least 2 segments to form a closed loop. A single arc is not a closed profile — add a line or another arc to close it.".into(),
        ));
    }
    let pt = |v: &Value, k: &str| {
        v.get(k)
            .and_then(|p| Some((p.get("x")?.as_f64()?, p.get("y")?.as_f64()?)))
    };
    for (i, seg) in segments.iter().enumerate() {
        let next = &segments[(i + 1) % segments.len()];
        let (Some(end), Some(_), Some(next_start)) =
            (pt(seg, "end"), pt(seg, "start"), pt(next, "start"))
        else {
            return Err(invalid(format!("segment {i} missing start/end points")));
        };
        if (end.0 - next_start.0).abs() >= 1e-3 || (end.1 - next_start.1).abs() >= 1e-3 {
            return Err(invalid(format!(
                "sketch is not closed: segment {i} ends at ({}, {}) but segment {} starts at ({}, {}). Each segment's end must match the next segment's start.",
                end.0, end.1, (i + 1) % segments.len(), next_start.0, next_start.1
            )));
        }
    }
    let mut obj = s.as_object().cloned().unwrap_or_default();
    obj.insert("type".into(), "Sketch2D".into());
    let value = Value::Object(obj);
    serde_json::from_value::<vcad_ir::CsgOp>(value.clone())
        .map_err(|e| invalid(format!("{ctx} sketch: {e}")))?;
    Ok(value.to_string())
}

fn path_json(path: &Value) -> Result<String, CommandError> {
    serde_json::from_value::<vcad_ir::PathCurve>(path.clone())
        .map_err(|e| invalid(format!("invalid path: {e}")))?;
    Ok(path.to_string())
}

fn opt_num(v: &Value, key: &str) -> Result<Option<f64>, CommandError> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => num(v, key, key).map(Some),
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: [f64; 3], k: f64) -> [f64; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn norm(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

fn normalize(a: [f64; 3]) -> [f64; 3] {
    scale(a, 1.0 / norm(a))
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Euler-XYZ rotation (degrees; X, then Y, then Z) — the kernel's convention.
fn rotate(p: [f64; 3], deg: [f64; 3]) -> [f64; 3] {
    let (sx, cx) = deg[0].to_radians().sin_cos();
    let (sy, cy) = deg[1].to_radians().sin_cos();
    let (sz, cz) = deg[2].to_radians().sin_cos();
    let [x, y, z] = [p[0], p[1] * cx - p[2] * sx, p[1] * sx + p[2] * cx];
    let [x, y, z] = [x * cy + z * sy, y, -x * sy + z * cy];
    [x * cz - y * sz, x * sz + y * cz, z]
}

/// Inverse of [`rotate`]: Z⁻¹, then Y⁻¹, then X⁻¹.
fn inverse_rotate(p: [f64; 3], deg: [f64; 3]) -> [f64; 3] {
    let (sx, cx) = deg[0].to_radians().sin_cos();
    let (sy, cy) = deg[1].to_radians().sin_cos();
    let (sz, cz) = deg[2].to_radians().sin_cos();
    let [x, y, z] = [p[0] * cz + p[1] * sz, -p[0] * sz + p[1] * cz, p[2]];
    let [x, y, z] = [x * cy - z * sy, y, x * sy + z * cy];
    [x, y * cx + z * sx, -y * sx + z * cx]
}

fn unsupported(tool: &str) -> CommandError {
    CommandError::Unsupported {
        tool: tool.to_string(),
    }
}

fn invalid(message: String) -> CommandError {
    CommandError::Invalid { message }
}

fn str_arg<'a>(v: &'a Value, key: &str, ctx: &str) -> Result<&'a str, CommandError> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid(format!("{ctx} requires `{key}`")))
}

fn num(v: &Value, key: &str, ctx: &str) -> Result<f64, CommandError> {
    v.get(key)
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .ok_or_else(|| invalid(format!("{ctx} requires numeric `{key}`")))
}

fn num_or(v: &Value, key: &str, default: f64) -> Result<f64, CommandError> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(_) => num(v, key, key),
    }
}

fn positive(v: &Value, key: &str, ctx: &str) -> Result<f64, CommandError> {
    let n = num(v, key, ctx)?;
    if n > 0.0 {
        Ok(n)
    } else {
        Err(invalid(format!("{ctx} requires `{key}` > 0")))
    }
}

fn segments(v: &Value) -> Result<u32, CommandError> {
    Ok(num_or(v, "segments", 32.0)?.round().max(3.0) as u32)
}

fn count(v: &Value, ctx: &str) -> Result<u32, CommandError> {
    let n = num(v, "count", ctx)?;
    if n >= 1.0 && n.fract() == 0.0 {
        Ok(n as u32)
    } else {
        Err(invalid(format!("{ctx} requires integer `count` ≥ 1")))
    }
}

fn vec3(v: &Value, key: &str, ctx: &str) -> Result<[f64; 3], CommandError> {
    let o = v
        .get(key)
        .ok_or_else(|| invalid(format!("{ctx} requires `{key}` ({{x, y, z}})")))?;
    let ctx = format!("{ctx} {key}");
    Ok([num(o, "x", &ctx)?, num(o, "y", &ctx)?, num(o, "z", &ctx)?])
}

/// Modifier target: `parent_part_id` (on `create`) wins over `child`.
fn target(p: &Value, parent: Option<&str>, ctx: &str) -> Result<String, CommandError> {
    match parent {
        Some(id) if !id.is_empty() => Ok(id.to_string()),
        _ => str_arg(p, "child", ctx)
            .map(str::to_string)
            .map_err(|_| invalid(format!("{ctx} requires parent_part_id or child"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DocumentApi;
    use serde_json::json;
    use vcad_crdt::ReplicaId;

    fn lower_empty(tool: &str, args: &Value) -> Result<DocCommand, CommandError> {
        lower(tool, args, &DocumentApi::new(ReplicaId(1)))
    }

    #[test]
    fn create_cube_maps_ir_size() {
        let cmd = lower_empty(
            "create",
            &json!({"type": "cube", "params": {"size": {"x": 1, "y": 2, "z": 3}}, "name": "Block"}),
        )
        .unwrap();
        match cmd {
            DocCommand::AddFeature {
                feature:
                    FeatureInput::Cube {
                        size_x,
                        size_y,
                        size_z,
                    },
                name,
            } => {
                assert_eq!((size_x, size_y, size_z), (1.0, 2.0, 3.0));
                assert_eq!(name.as_deref(), Some("Block"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn primitives_default_like_the_web() {
        let cmd = lower_empty("create", &json!({"type": "cylinder"})).unwrap();
        assert!(matches!(
            cmd,
            DocCommand::AddFeature {
                feature: FeatureInput::Cylinder { radius, height, segments: Some(32) },
                ..
            } if radius == 10.0 && height == 20.0
        ));
    }

    #[test]
    fn fillet_prefers_parent_part_id() {
        let cmd = lower_empty(
            "create",
            &json!({"type": "fillet", "parent_part_id": "1:0", "params": {"radius": 2, "child": "9:9"}}),
        )
        .unwrap();
        assert!(matches!(
            cmd,
            DocCommand::AddFeature { feature: FeatureInput::Fillet { ref input, .. }, .. } if input == "1:0"
        ));
    }

    #[test]
    fn bad_args_are_invalid() {
        for (tool, args) in [
            (
                "linear_pattern",
                json!({"child": "1:0", "direction": {"x": 1, "y": 0, "z": 0}, "count": 0, "spacing": 5}),
            ),
            ("mirror", json!({"child": "1:0", "plane": "AB"})),
            ("fillet", json!({"child": "1:0", "radius": -1})),
            ("translate", json!({"child": "1:0"})),
        ] {
            assert!(
                matches!(lower_empty(tool, &args), Err(CommandError::Invalid { .. })),
                "{tool} should be invalid"
            );
        }
    }

    #[test]
    fn unknown_tools_are_unsupported() {
        assert!(matches!(
            lower_empty("describe_scene", &json!({})),
            Err(CommandError::Unsupported { .. })
        ));
        assert!(matches!(
            lower_empty("create", &json!({"type": "gear"})),
            Err(CommandError::Unsupported { .. })
        ));
    }

    #[test]
    fn boolean_with_unknown_operand_fails_closed() {
        let mut api = DocumentApi::new(ReplicaId(1));
        let a = api
            .apply(lower_empty("create", &json!({"type": "cube"})).unwrap())
            .unwrap()
            .created_feature_id
            .unwrap();
        let err = api
            .apply(lower_empty("difference", &json!({"left": a, "right": "nope"})).unwrap())
            .unwrap_err();
        assert_eq!(err, CommandError::UnknownId { id: "nope".into() });
        assert_eq!(api.get_state().parts.len(), 1);
    }

    fn apply_ok(api: &mut DocumentApi, tool: &str, args: Value) -> Value {
        let v: Value =
            serde_json::from_str(&api.apply_tool_envelope(tool, &args.to_string())).unwrap();
        assert_eq!(v["ok"], true, "{tool} {args}: {v}");
        v
    }

    #[test]
    fn rotate_keeps_bbox_center_fixed() {
        let mut api = DocumentApi::new(ReplicaId(1));
        let id = apply_ok(
            &mut api,
            "create",
            json!({"type": "cube", "params": {"size": {"x": 10, "y": 4, "z": 2}}}),
        )["result"]["createdFeatureId"]
            .as_str()
            .unwrap()
            .to_string();
        apply_ok(
            &mut api,
            "create",
            json!({"type": "translate", "params": {"child": id, "offset": {"x": 30, "y": 0, "z": 0}}}),
        );
        let center = |api: &DocumentApi| {
            let (lo, hi) = api.part_world_bbox(&id).unwrap();
            [
                (lo[0] + hi[0]) / 2.0,
                (lo[1] + hi[1]) / 2.0,
                (lo[2] + hi[2]) / 2.0,
            ]
        };
        let before = center(&api);
        apply_ok(
            &mut api,
            "create",
            json!({"type": "rotate", "params": {"child": id, "angles": {"x": 0, "y": 0, "z": 90}}}),
        );
        let after = center(&api);
        for k in 0..3 {
            assert!(
                (before[k] - after[k]).abs() < 1e-6,
                "axis {k}: {before:?} vs {after:?}"
            );
        }
        let (lo, hi) = api.part_world_bbox(&id).unwrap();
        assert!(((hi[0] - lo[0]) - 4.0).abs() < 1e-6 && ((hi[1] - lo[1]) - 10.0).abs() < 1e-6);
    }

    fn bbox(api: &DocumentApi, id: &str) -> ([f64; 3], [f64; 3]) {
        api.part_world_bbox(id).unwrap()
    }

    fn assert_close(a: [f64; 3], b: [f64; 3]) {
        for k in 0..3 {
            assert!((a[k] - b[k]).abs() < 1e-6, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn place_moves_anchor_onto_world_point() {
        let mut api = DocumentApi::new(ReplicaId(1));
        let id = apply_ok(
            &mut api,
            "create",
            json!({"type": "cube", "params": {"size": {"x": 10, "y": 4, "z": 2}}}),
        )["result"]["createdFeatureId"]
            .as_str()
            .unwrap()
            .to_string();
        // Default `from` is the bbox centre.
        apply_ok(
            &mut api,
            "place",
            json!({"part_id": id, "to": {"x": 100, "y": 0, "z": 0}}),
        );
        let (lo, hi) = bbox(&api, &id);
        assert_close(lo, [95.0, -2.0, -1.0]);
        assert_close(hi, [105.0, 2.0, 1.0]);
        // Placing is absolute: repeating it is a no-op, not a second move.
        apply_ok(
            &mut api,
            "place",
            json!({"part_id": id, "from": "min", "to": {"x": 0, "y": 0, "z": 0}}),
        );
        apply_ok(
            &mut api,
            "place",
            json!({"part_id": id, "from": "min", "to": {"x": 0, "y": 0, "z": 0}}),
        );
        assert_close(bbox(&api, &id).0, [0.0; 3]);
    }

    #[test]
    fn place_stacks_on_another_parts_anchor() {
        let mut api = DocumentApi::new(ReplicaId(1));
        let created = |api: &mut DocumentApi, size: f64| {
            apply_ok(
                api,
                "create",
                json!({"type": "cube", "params": {"size": {"x": size, "y": size, "z": size}}}),
            )["result"]["createdFeatureId"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let base = created(&mut api, 20.0);
        let lid = created(&mut api, 4.0);
        apply_ok(
            &mut api,
            "translate",
            json!({"child": base, "offset": {"x": 5, "y": 5, "z": 3}}),
        );
        apply_ok(
            &mut api,
            "place",
            json!({"part_id": lid, "from": "bottom", "to": {"part": base, "anchor": "top"}}),
        );
        let (blo, bhi) = bbox(&api, &base);
        let (llo, lhi) = bbox(&api, &lid);
        assert!((llo[2] - bhi[2]).abs() < 1e-6, "lid sits on base top");
        assert!(((llo[0] + lhi[0]) / 2.0 - (blo[0] + bhi[0]) / 2.0).abs() < 1e-6);
        assert!(((llo[1] + lhi[1]) / 2.0 - (blo[1] + bhi[1]) / 2.0).abs() < 1e-6);
        // The target part did not move.
        assert_close(blo, [5.0, 5.0, 3.0]);
    }

    #[test]
    fn place_bad_args_fail_closed() {
        let mut api = DocumentApi::new(ReplicaId(1));
        let id = apply_ok(&mut api, "create", json!({"type": "cube"}))["result"]
            ["createdFeatureId"]
            .as_str()
            .unwrap()
            .to_string();
        let before = bbox(&api, &id);
        let err = |args: Value| lower("place", &args, &api).unwrap_err();
        assert!(matches!(
            err(json!({"part_id": "9:9", "to": "top"})),
            CommandError::UnknownId { .. }
        ));
        assert!(matches!(
            err(json!({"part_id": id, "to": {"part": "9:9", "anchor": "top"}})),
            CommandError::UnknownId { .. }
        ));
        for bad in [
            json!({"part_id": id}),
            json!({"part_id": id, "to": "middle"}),
            json!({"part_id": id, "to": {"x": 1, "y": 2}}),
            json!({"part_id": id, "to": 7}),
        ] {
            assert!(
                matches!(err(bad.clone()), CommandError::Invalid { .. }),
                "{bad}"
            );
        }
        assert_eq!(bbox(&api, &id), before);
    }

    #[test]
    fn place_part_lowers_to_named_part_instance() {
        let cmd = lower_empty(
            "place_part",
            &json!({"path": "std:fastener.bolt", "version": "1.0",
                    "params": {"d": 3}, "name": "Bolt"}),
        )
        .unwrap();
        match cmd {
            DocCommand::AddFeature {
                feature:
                    FeatureInput::PartInstance {
                        path,
                        version,
                        params_json,
                    },
                name,
            } => {
                assert_eq!(path, "std:fastener.bolt");
                assert_eq!(version, "1.0");
                assert_eq!(
                    serde_json::from_str::<Value>(&params_json).unwrap(),
                    json!({"d": 3})
                );
                assert_eq!(name.as_deref(), Some("Bolt"));
            }
            other => panic!("expected AddFeature(PartInstance), got {other:?}"),
        }
        // Params default to an empty object.
        match lower_empty("place_part", &json!({"path": "std:x", "version": "1"})).unwrap() {
            DocCommand::AddFeature {
                feature: FeatureInput::PartInstance { params_json, .. },
                name: None,
            } => assert_eq!(params_json, "{}"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn place_part_bad_args_fail_closed() {
        for bad in [
            json!({"version": "1"}),
            json!({"path": "", "version": "1"}),
            json!({"path": "std:x"}),
            json!({"path": "std:x", "version": "1", "params": [1]}),
        ] {
            assert!(
                matches!(
                    lower_empty("place_part", &bad).unwrap_err(),
                    CommandError::Invalid { .. }
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn place_part_applies_a_named_feature() {
        let mut api = DocumentApi::new(ReplicaId(1));
        let out = apply_ok(
            &mut api,
            "place_part",
            json!({"path": "std:x", "version": "1", "name": "Widget"}),
        );
        let id = out["result"]["createdFeatureId"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(api.stable_ids().resolve(&id).is_some());
    }

    #[test]
    fn rotate_about_origin_only_sets_rotation() {
        let cmd = lower_empty(
            "rotate",
            &json!({"child": "1:0", "angles": {"x": 0, "y": 0, "z": 45}, "pivot": "origin"}),
        )
        .unwrap();
        assert!(matches!(cmd, DocCommand::SetRotation { .. }));
    }

    #[test]
    fn tube_spans_start_to_end() {
        let mut api = DocumentApi::new(ReplicaId(1));
        let v = apply_ok(
            &mut api,
            "tube",
            json!({"start": {"x": 0, "y": 0, "z": 0}, "end": {"x": 0, "y": 0, "z": 50}, "radius": 3}),
        );
        let id = v["result"]["createdFeatureId"].as_str().unwrap();
        let (lo, hi) = api.part_world_bbox(id).unwrap();
        assert!((hi[2] - lo[2] - 50.0).abs() < 1e-3, "{lo:?} {hi:?}");
        assert!((hi[0] - lo[0] - 6.0).abs() < 0.2, "{lo:?} {hi:?}");
    }

    #[test]
    fn polyline_tube_names_each_segment() {
        let mut api = DocumentApi::new(ReplicaId(1));
        apply_ok(
            &mut api,
            "polyline_tube",
            json!({"name": "Frame", "points": [
                {"x": 0, "y": 0, "z": 0}, {"x": 40, "y": 0, "z": 0}, {"x": 40, "y": 40, "z": 0}
            ]}),
        );
        let names: Vec<String> = api
            .get_state()
            .parts
            .iter()
            .map(|p| {
                serde_json::to_value(p).unwrap()["name"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(names, ["Frame 1", "Frame 2"]);
    }

    #[test]
    fn extrude_inline_sketch() {
        let mut api = DocumentApi::new(ReplicaId(1));
        let sq = |a: f64| {
            json!([
                {"type": "Line", "start": {"x": 0, "y": 0}, "end": {"x": a, "y": 0}},
                {"type": "Line", "start": {"x": a, "y": 0}, "end": {"x": a, "y": a}},
                {"type": "Line", "start": {"x": a, "y": a}, "end": {"x": 0, "y": a}},
                {"type": "Line", "start": {"x": 0, "y": a}, "end": {"x": 0, "y": 0}}
            ])
        };
        let plane = |segs: Value| json!({"origin": {"x": 0, "y": 0, "z": 0}, "x_dir": {"x": 1, "y": 0, "z": 0}, "y_dir": {"x": 0, "y": 1, "z": 0}, "segments": segs});
        let v = apply_ok(
            &mut api,
            "create",
            json!({"type": "extrude", "params": {"sketch": plane(sq(10.0)), "direction": {"x": 0, "y": 0, "z": 7}}}),
        );
        let id = v["result"]["createdFeatureId"].as_str().unwrap();
        let (lo, hi) = api.part_world_bbox(id).unwrap();
        assert!((hi[2] - lo[2] - 7.0).abs() < 1e-6 && (hi[0] - lo[0] - 10.0).abs() < 1e-6);

        let mut open = sq(10.0);
        open[3]["end"] = json!({"x": 1, "y": 1});
        let err = lower_empty("create", &json!({"type": "extrude", "params": {"sketch": plane(open), "direction": {"x": 0, "y": 0, "z": 1}}})).unwrap_err();
        assert!(
            matches!(err, CommandError::Invalid { ref message } if message.contains("not closed")),
            "{err:?}"
        );
    }

    #[test]
    fn set_material_selector_and_fail_closed_batch() {
        let mut api = DocumentApi::new(ReplicaId(1));
        for name in ["Bolt A", "Bolt B", "Plate"] {
            apply_ok(
                &mut api,
                "create",
                json!({"type": "cylinder", "name": name}),
            );
        }
        apply_ok(
            &mut api,
            "set_material",
            json!({"material": "steel", "selector": {"by": "name_prefix", "value": "bolt"}}),
        );
        let steel = api
            .get_state()
            .document
            .roots
            .iter()
            .filter(|r| r.material == "steel")
            .count();
        assert_eq!(steel, 2);
        let v: Value = serde_json::from_str(&api.apply_tool_envelope(
            "set_material",
            &json!({"material": "brass", "part_ids": ["1:0", "missing"]}).to_string(),
        ))
        .unwrap();
        assert_eq!(v["error"]["kind"], "batch");
        let v: Value = serde_json::from_str(
            &api.apply_tool_envelope(
                "set_material",
                &json!({"material": "brass", "selector": {"by": "kind", "value": "sphere"}})
                    .to_string(),
            ),
        )
        .unwrap();
        assert_eq!(v["error"]["kind"], "invalid");
    }

    #[test]
    fn update_maps_create_shaped_params_and_refuses_unknown_keys() {
        let mut api = DocumentApi::new(ReplicaId(1));
        apply_ok(
            &mut api,
            "create",
            json!({"type": "cube", "params": {"size": {"x": 1, "y": 2, "z": 3}}}),
        );
        let part = serde_json::to_value(&api.get_state().parts[0]).unwrap();
        let node = part["primitiveNodeId"].clone();
        // The create shape (`size: {x,y,z}`) used to be written verbatim as
        // a `size` key the cube never reads: ok, and nothing changed.
        apply_ok(
            &mut api,
            "update",
            json!({"node_id": node, "params": {"size": {"x": 8, "z": 5}}}),
        );
        let doc = api.get_state().document;
        match &doc.nodes[&part["primitiveNodeId"].as_u64().unwrap()].op {
            vcad_ir::CsgOp::Cube { size } => assert_eq!((size.x, size.y, size.z), (8.0, 2.0, 5.0)),
            other => panic!("{other:?}"),
        }
        for bad in [json!({"radius": 1}), json!({"size": {"w": 1}})] {
            let v: Value = serde_json::from_str(&api.apply_tool_envelope(
                "update",
                &json!({"node_id": node, "params": bad}).to_string(),
            ))
            .unwrap();
            assert_eq!(v["error"]["kind"], "invalid", "{bad}");
        }
    }

    #[test]
    fn update_targets_the_part_that_owns_the_node() {
        let mut api = DocumentApi::new(ReplicaId(1));
        apply_ok(
            &mut api,
            "create",
            json!({"type": "sphere", "params": {"radius": 1}}),
        );
        apply_ok(
            &mut api,
            "create",
            json!({"type": "sphere", "params": {"radius": 2}}),
        );
        let second = serde_json::to_value(&api.get_state().parts[1]).unwrap();
        let node = second["primitiveNodeId"].clone();
        apply_ok(
            &mut api,
            "update",
            json!({"node_id": node, "params": {"radius": 9}}),
        );
        let doc = api.get_state().document;
        let radius = |n: &Value| match &doc.nodes[&n.as_u64().unwrap()].op {
            vcad_ir::CsgOp::Sphere { radius, .. } => *radius,
            other => panic!("{other:?}"),
        };
        let first = serde_json::to_value(&api.get_state().parts[0]).unwrap();
        assert_eq!(radius(&first["primitiveNodeId"]), 1.0);
        assert_eq!(radius(&node), 9.0);
        let v: Value = serde_json::from_str(
            &api.apply_tool_envelope("update", r#"{"node_id": 999, "params": {"radius": 1}}"#),
        )
        .unwrap();
        assert_eq!(v["error"]["kind"], "unknown_id");
    }

    #[test]
    fn cone_materializes() {
        let mut api = DocumentApi::new(ReplicaId(1));
        let res = api
            .apply(
                lower_empty(
                    "create",
                    &json!({"type": "cone", "params": {"radius_bottom": 5, "height": 8}}),
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(res.parts.len(), 1);
        assert!(res.warnings.is_empty(), "{:?}", res.warnings);
    }

    #[test]
    fn apply_tool_envelope_runs_end_to_end() {
        let mut api = DocumentApi::new(ReplicaId(1));
        let v: Value = serde_json::from_str(
            &api.apply_tool_envelope("create", r#"{"type":"sphere","params":{"radius":4}}"#),
        )
        .unwrap();
        assert_eq!(v["ok"], true);
        let id = v["result"]["createdFeatureId"]
            .as_str()
            .unwrap()
            .to_string();
        let v: Value = serde_json::from_str(&api.apply_tool_envelope(
            "translate",
            &json!({"child": id, "offset": {"x": 5, "y": 0, "z": 0}}).to_string(),
        ))
        .unwrap();
        assert_eq!(v["ok"], true);
        let v: Value =
            serde_json::from_str(&api.apply_tool_envelope("describe_scene", "{}")).unwrap();
        assert_eq!(v["error"]["kind"], "unsupported");
    }
}
