//! AI tool calls lowered to [`DocCommand`]s.
//!
//! The chat tools (`create`, `translate`, …) are a convenience vocabulary
//! shaped for language models: IR-style params (`size: {x, y, z}`), part ids
//! called `child` / `left` / `right`, and `parent_part_id` for modifiers.
//! [`lower`] turns one call into the [`DocCommand`] the engine applies, so
//! every host runs the same tool semantics instead of re-implementing them.
//!
//! Tools that need evaluated geometry (rotate about a bbox pivot, tubes,
//! sketch-based features) are not lowered here yet and return
//! [`CommandError::Unsupported`]; hosts keep their own path for those.

use serde_json::Value;

use crate::command::{CommandError, DocCommand};
use crate::feature::{BooleanType, FeatureInput};

/// Lower one tool call into a command. `tool` is the top-level tool name
/// (`create`, `translate`, `linear_pattern`, …); for `create`, `args.type`
/// picks the feature kind and `args.params` carries its parameters.
pub fn lower(tool: &str, args: &Value) -> Result<DocCommand, CommandError> {
    match tool {
        "create" => {
            let kind = str_arg(args, "type", "create")?;
            let empty = Value::Object(Default::default());
            let params = args.get("params").unwrap_or(&empty);
            let parent = args.get("parent_part_id").and_then(Value::as_str);
            let name = args.get("name").and_then(Value::as_str).map(str::to_string);
            lower_create(kind, params, parent, name)
        }
        "linear_pattern" | "circular_pattern" | "mirror" | "translate" | "scale" | "union"
        | "difference" | "intersection" | "fillet" | "chamfer" | "shell" => {
            let name = args.get("name").and_then(Value::as_str).map(str::to_string);
            lower_create(tool, args, None, name)
        }
        other => Err(unsupported(other)),
    }
}

fn lower_create(
    kind: &str,
    p: &Value,
    parent: Option<&str>,
    name: Option<String>,
) -> Result<DocCommand, CommandError> {
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

    #[test]
    fn create_cube_maps_ir_size() {
        let cmd = lower(
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
        let cmd = lower("create", &json!({"type": "cylinder"})).unwrap();
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
        let cmd = lower(
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
                matches!(lower(tool, &args), Err(CommandError::Invalid { .. })),
                "{tool} should be invalid"
            );
        }
    }

    #[test]
    fn geometry_dependent_tools_are_unsupported() {
        assert!(matches!(
            lower("rotate", &json!({})),
            Err(CommandError::Unsupported { .. })
        ));
        assert!(matches!(
            lower("create", &json!({"type": "tube"})),
            Err(CommandError::Unsupported { .. })
        ));
    }

    #[test]
    fn boolean_with_unknown_operand_fails_closed() {
        let mut api = DocumentApi::new(ReplicaId(1));
        let a = api
            .apply(lower("create", &json!({"type": "cube"})).unwrap())
            .unwrap()
            .created_feature_id
            .unwrap();
        let err = api
            .apply(lower("difference", &json!({"left": a, "right": "nope"})).unwrap())
            .unwrap_err();
        assert_eq!(err, CommandError::UnknownId { id: "nope".into() });
        assert_eq!(api.get_state().parts.len(), 1);
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
        let v: Value = serde_json::from_str(&api.apply_tool_envelope("rotate", "{}")).unwrap();
        assert_eq!(v["error"]["kind"], "unsupported");
    }
}
