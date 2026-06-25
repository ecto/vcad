//! Parameter and binding resolution pre-pass.
//!
//! [`resolve_document`] evaluates every [`Parameter`](vcad_ir::Parameter) in
//! dependency order and applies every [`Bindings`](vcad_ir::Bindings) entry
//! to the matching concrete field on the target `CsgOp`. After this pass,
//! the document is kernel-ready — downstream code continues to consume
//! `f64` / `Vec3` without ever seeing an expression.
//!
//! The binding `field_path` grammar is dotted:
//! - scalar field: `"radius"`, `"height"`, `"thickness"`, `"angle_deg"`, ...
//! - vector component: `"size.x"`, `"offset.z"`, `"direction.y"`, ...
//! - optional scalar: `"twist_angle"`, `"scale_end"` (sets `Some(..)` if
//!   previously `None`)
//!
//! Unknown / unsupported paths produce [`ResolvePatchError`] rather than a
//! panic so partially-known documents can still report diagnostics.

use std::collections::HashMap;

use vcad_ir::{BindingKey, CsgOp, Document, Vec2, Vec3};

/// Error applying a binding to a concrete field.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvePatchError {
    /// The referenced node was not found.
    MissingNode(BindingKey),
    /// The dotted field path does not exist on this op variant.
    UnknownField {
        /// The binding key.
        key: BindingKey,
        /// The variant name (e.g. "Cube", "Cylinder").
        op_name: &'static str,
    },
    /// Underlying parameter / eval failure bubbled up.
    Resolve(vcad_ir::ResolveError),
}

impl std::fmt::Display for ResolvePatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingNode(k) => write!(f, "binding '{}' references missing node", k),
            Self::UnknownField { key, op_name } => {
                write!(
                    f,
                    "binding '{}' — field path not valid on op {}",
                    key, op_name
                )
            }
            Self::Resolve(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for ResolvePatchError {}

impl From<vcad_ir::ResolveError> for ResolvePatchError {
    fn from(e: vcad_ir::ResolveError) -> Self {
        Self::Resolve(e)
    }
}

/// Evaluate `doc.parameters` into an env, then apply `doc.bindings` onto
/// concrete fields in `doc.nodes`. Mutates `doc` in place; returns the
/// resolved environment (for downstream inspection / UI display).
///
/// When `doc.parameters` and `doc.bindings` are both empty, this function
/// is a cheap no-op — useful for loading old `.vcad` files.
pub fn resolve_document(doc: &mut Document) -> Result<HashMap<String, f64>, ResolvePatchError> {
    if doc.parameters.is_empty() && doc.bindings.is_empty() {
        return Ok(HashMap::new());
    }

    let env = vcad_ir::resolve_parameters(&doc.parameters)?;

    // Collect bindings sorted by node id then field path for deterministic
    // iteration (aids tests and diagnostics).
    let mut keys: Vec<(BindingKey, vcad_ir::Expr)> = doc
        .bindings
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    keys.sort_by(|a, b| {
        a.0.node_id
            .cmp(&b.0.node_id)
            .then_with(|| a.0.field_path.cmp(&b.0.field_path))
    });

    for (key, expr) in keys {
        let value = vcad_ir::resolve_binding(&key, &expr, &env)?;
        // A "pcb."-prefixed field path targets the document's PCB (a non-node
        // domain), so the same Bindings map couples connector_x to a footprint
        // position exactly as it couples it to a node field — one DAG.
        if let Some(pcb_path) = key.field_path.strip_prefix("pcb.") {
            let pcb = doc
                .pcb
                .as_mut()
                .ok_or_else(|| ResolvePatchError::MissingNode(key.clone()))?;
            apply_pcb_patch(pcb, pcb_path, value, &key)?;
            continue;
        }
        let node = doc
            .nodes
            .get_mut(&key.node_id)
            .ok_or_else(|| ResolvePatchError::MissingNode(key.clone()))?;
        apply_to_op(&mut node.op, &key, value)?;
    }

    Ok(env)
}

/// Apply a `"pcb."`-prefixed binding onto the document's PCB. The remaining path
/// is `"<ref>.position.<x|y>"` — drives a footprint's placement from a parameter.
fn apply_pcb_patch(
    pcb: &mut vcad_ir::ecad::Pcb,
    path: &str,
    value: f64,
    key: &BindingKey,
) -> Result<(), ResolvePatchError> {
    let bad = || ResolvePatchError::UnknownField {
        key: key.clone(),
        op_name: "Pcb",
    };
    let parts: Vec<&str> = path.split('.').collect();
    match parts.as_slice() {
        [reference, "position", axis] => {
            let fp = pcb
                .footprints
                .iter_mut()
                .find(|f| f.reference == *reference)
                .ok_or_else(bad)?;
            match *axis {
                "x" => fp.position.x = value,
                "y" => fp.position.y = value,
                _ => return Err(bad()),
            }
        }
        _ => return Err(bad()),
    }
    Ok(())
}

/// Shallow "resolve to JSON" helper: returns a cloned Document with
/// parameters resolved and bindings applied. Primary used by WASM /
/// TS callers that need an immutable view.
pub fn resolve_document_cloned(
    doc: &Document,
) -> Result<(Document, HashMap<String, f64>), ResolvePatchError> {
    let mut cloned = doc.clone();
    let env = resolve_document(&mut cloned)?;
    Ok((cloned, env))
}

fn apply_to_op(op: &mut CsgOp, key: &BindingKey, value: f64) -> Result<(), ResolvePatchError> {
    let path = key.field_path.as_str();

    // Helpers
    let bad = |op_name: &'static str| {
        Err(ResolvePatchError::UnknownField {
            key: key.clone(),
            op_name,
        })
    };

    match op {
        CsgOp::Cube { size } => match path {
            "size.x" => size.x = value,
            "size.y" => size.y = value,
            "size.z" => size.z = value,
            _ => return bad("Cube"),
        },
        CsgOp::Cylinder {
            radius,
            height,
            segments,
        } => match path {
            "radius" => *radius = value,
            "height" => *height = value,
            "segments" => *segments = value.round().max(0.0) as u32,
            _ => return bad("Cylinder"),
        },
        CsgOp::Sphere { radius, segments } => match path {
            "radius" => *radius = value,
            "segments" => *segments = value.round().max(0.0) as u32,
            _ => return bad("Sphere"),
        },
        CsgOp::Cone {
            radius_bottom,
            radius_top,
            height,
            segments,
        } => match path {
            "radius_bottom" => *radius_bottom = value,
            "radius_top" => *radius_top = value,
            "height" => *height = value,
            "segments" => *segments = value.round().max(0.0) as u32,
            _ => return bad("Cone"),
        },
        CsgOp::Torus {
            major_radius,
            minor_radius,
            segments,
        } => match path {
            "major_radius" => *major_radius = value,
            "minor_radius" => *minor_radius = value,
            "segments" => *segments = value.round().max(0.0) as u32,
            _ => return bad("Torus"),
        },
        CsgOp::Translate { offset, .. } => {
            let sub =
                path.strip_prefix("offset.")
                    .ok_or_else(|| ResolvePatchError::UnknownField {
                        key: key.clone(),
                        op_name: "Translate",
                    })?;
            apply_vec3(offset, sub, value).ok_or_else(|| ResolvePatchError::UnknownField {
                key: key.clone(),
                op_name: "Translate",
            })?;
        }
        CsgOp::Rotate { angles, .. } => {
            let sub =
                path.strip_prefix("angles.")
                    .ok_or_else(|| ResolvePatchError::UnknownField {
                        key: key.clone(),
                        op_name: "Rotate",
                    })?;
            apply_vec3(angles, sub, value).ok_or_else(|| ResolvePatchError::UnknownField {
                key: key.clone(),
                op_name: "Rotate",
            })?;
        }
        CsgOp::Scale { factor, .. } => {
            let sub =
                path.strip_prefix("factor.")
                    .ok_or_else(|| ResolvePatchError::UnknownField {
                        key: key.clone(),
                        op_name: "Scale",
                    })?;
            apply_vec3(factor, sub, value).ok_or_else(|| ResolvePatchError::UnknownField {
                key: key.clone(),
                op_name: "Scale",
            })?;
        }
        CsgOp::Extrude {
            direction,
            twist_angle,
            scale_end,
            ..
        } => {
            if let Some(sub) = path.strip_prefix("direction.") {
                if apply_vec3(direction, sub, value).is_none() {
                    return bad("Extrude");
                }
            } else if path == "twist_angle" {
                *twist_angle = Some(value);
            } else if path == "scale_end" {
                *scale_end = Some(value);
            } else {
                return bad("Extrude");
            }
        }
        CsgOp::Revolve {
            axis_origin,
            axis_dir,
            angle_deg,
            ..
        } => {
            let prefix_origin = path.strip_prefix("axis_origin.");
            let prefix_dir = path.strip_prefix("axis_dir.");
            if let Some(sub) = prefix_origin {
                if apply_vec3(axis_origin, sub, value).is_none() {
                    return bad("Revolve");
                }
            } else if let Some(sub) = prefix_dir {
                if apply_vec3(axis_dir, sub, value).is_none() {
                    return bad("Revolve");
                }
            } else if path == "angle_deg" {
                *angle_deg = value;
            } else {
                return bad("Revolve");
            }
        }
        CsgOp::LinearPattern {
            direction,
            count,
            spacing,
            ..
        } => {
            if let Some(sub) = path.strip_prefix("direction.") {
                if apply_vec3(direction, sub, value).is_none() {
                    return bad("LinearPattern");
                }
            } else if path == "spacing" {
                *spacing = value;
            } else if path == "count" {
                *count = value.round().max(0.0) as u32;
            } else {
                return bad("LinearPattern");
            }
        }
        CsgOp::CircularPattern {
            axis_origin,
            axis_dir,
            count,
            angle_deg,
            ..
        } => {
            let prefix_origin = path.strip_prefix("axis_origin.");
            let prefix_dir = path.strip_prefix("axis_dir.");
            if let Some(sub) = prefix_origin {
                if apply_vec3(axis_origin, sub, value).is_none() {
                    return bad("CircularPattern");
                }
            } else if let Some(sub) = prefix_dir {
                if apply_vec3(axis_dir, sub, value).is_none() {
                    return bad("CircularPattern");
                }
            } else if path == "angle_deg" {
                *angle_deg = value;
            } else if path == "count" {
                *count = value.round().max(0.0) as u32;
            } else {
                return bad("CircularPattern");
            }
        }
        CsgOp::Shell { thickness, .. } => {
            if path == "thickness" {
                *thickness = value;
            } else {
                return bad("Shell");
            }
        }
        CsgOp::Fillet { radius, .. } => {
            if path == "radius" {
                *radius = value;
            } else {
                return bad("Fillet");
            }
        }
        CsgOp::Chamfer { distance, .. } => {
            if path == "distance" {
                *distance = value;
            } else {
                return bad("Chamfer");
            }
        }
        CsgOp::Sketch2D {
            origin,
            x_dir,
            y_dir,
            ..
        } => {
            let o = path.strip_prefix("origin.");
            let xd = path.strip_prefix("x_dir.");
            let yd = path.strip_prefix("y_dir.");
            let any = if let Some(s) = o {
                apply_vec3(origin, s, value).is_some()
            } else if let Some(s) = xd {
                apply_vec3(x_dir, s, value).is_some()
            } else if let Some(s) = yd {
                apply_vec3(y_dir, s, value).is_some()
            } else {
                false
            };
            if !any {
                return bad("Sketch2D");
            }
        }
        CsgOp::Text2D {
            origin,
            x_dir,
            y_dir,
            height,
            letter_spacing,
            line_spacing,
            ..
        } => {
            let o = path.strip_prefix("origin.");
            let xd = path.strip_prefix("x_dir.");
            let yd = path.strip_prefix("y_dir.");
            if let Some(s) = o {
                if apply_vec3(origin, s, value).is_none() {
                    return bad("Text2D");
                }
            } else if let Some(s) = xd {
                if apply_vec3(x_dir, s, value).is_none() {
                    return bad("Text2D");
                }
            } else if let Some(s) = yd {
                if apply_vec3(y_dir, s, value).is_none() {
                    return bad("Text2D");
                }
            } else if path == "height" {
                *height = value;
            } else if path == "letter_spacing" {
                *letter_spacing = Some(value);
            } else if path == "line_spacing" {
                *line_spacing = Some(value);
            } else {
                return bad("Text2D");
            }
        }
        CsgOp::Sweep {
            twist_angle,
            scale_start,
            scale_end,
            orientation,
            ..
        } => match path {
            "twist_angle" => *twist_angle = Some(value),
            "scale_start" => *scale_start = Some(value),
            "scale_end" => *scale_end = Some(value),
            "orientation" => *orientation = Some(value),
            _ => return bad("Sweep"),
        },
        CsgOp::SheetMetalBaseFlangeRect {
            width,
            depth,
            thickness,
            ..
        } => match path {
            "width" => *width = value,
            "depth" => *depth = value,
            "thickness" => *thickness = value,
            _ => return bad("SheetMetalBaseFlangeRect"),
        },
        CsgOp::SheetMetalEdgeFlange {
            length,
            angle,
            radius,
            ..
        } => match path {
            "length" => *length = value,
            "angle" => *angle = value,
            "radius" => *radius = Some(value),
            _ => return bad("SheetMetalEdgeFlange"),
        },
        _ => return bad("<unsupported op>"),
    }
    Ok(())
}

fn apply_vec3(v: &mut Vec3, component: &str, value: f64) -> Option<()> {
    match component {
        "x" => v.x = value,
        "y" => v.y = value,
        "z" => v.z = value,
        _ => return None,
    }
    Some(())
}

#[allow(dead_code)]
fn apply_vec2(v: &mut Vec2, component: &str, value: f64) -> Option<()> {
    match component {
        "x" => v.x = value,
        "y" => v.y = value,
        _ => return None,
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::{BindingKey, CsgOp, Document, Expr, MaterialDef, Node, Parameter, SceneEntry};

    fn doc_with_cube(size: Vec3) -> Document {
        let mut doc = Document::new();
        doc.nodes.insert(
            1,
            Node {
                id: 1,
                name: Some("cube".into()),
                op: CsgOp::Cube { size },
            },
        );
        doc.roots.push(SceneEntry {
            root: 1,
            material: "default".into(),
            visible: None,
        });
        doc.materials.insert(
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
        doc
    }

    #[test]
    fn no_params_no_bindings_is_noop() {
        let mut doc = doc_with_cube(Vec3::new(10.0, 20.0, 30.0));
        let env = resolve_document(&mut doc).unwrap();
        assert!(env.is_empty());
        match &doc.nodes[&1].op {
            CsgOp::Cube { size } => {
                assert_eq!(*size, Vec3::new(10.0, 20.0, 30.0));
            }
            _ => panic!("wrong op"),
        }
    }

    #[test]
    fn binding_patches_cube_dimension() {
        let mut doc = doc_with_cube(Vec3::new(0.0, 0.0, 0.0));
        doc.parameters.insert("w".into(), Parameter::literal(50.0));
        doc.bindings
            .bind(BindingKey::new(1, "size.x"), Expr::formula("w * 2"));
        doc.bindings
            .bind(BindingKey::new(1, "size.z"), Expr::num(17.5));
        let env = resolve_document(&mut doc).unwrap();
        assert_eq!(env["w"], 50.0);
        match &doc.nodes[&1].op {
            CsgOp::Cube { size } => {
                assert_eq!(size.x, 100.0);
                assert_eq!(size.y, 0.0);
                assert_eq!(size.z, 17.5);
            }
            _ => panic!("wrong op"),
        }
    }

    #[test]
    fn binding_missing_node_reports() {
        let mut doc = doc_with_cube(Vec3::new(1.0, 1.0, 1.0));
        doc.bindings
            .bind(BindingKey::new(99, "size.x"), Expr::num(5.0));
        let err = resolve_document(&mut doc).unwrap_err();
        assert!(matches!(err, ResolvePatchError::MissingNode(_)));
    }

    #[test]
    fn binding_unknown_field_reports() {
        let mut doc = doc_with_cube(Vec3::new(1.0, 1.0, 1.0));
        doc.bindings
            .bind(BindingKey::new(1, "nonsense"), Expr::num(5.0));
        let err = resolve_document(&mut doc).unwrap_err();
        assert!(matches!(err, ResolvePatchError::UnknownField { .. }));
    }

    #[test]
    fn cloned_resolve_does_not_mutate_source() {
        let mut doc = doc_with_cube(Vec3::new(1.0, 1.0, 1.0));
        doc.parameters.insert("s".into(), Parameter::literal(42.0));
        doc.bindings
            .bind(BindingKey::new(1, "size.x"), Expr::formula("s"));
        let original = doc.clone();
        let (patched, _env) = resolve_document_cloned(&doc).unwrap();
        assert_eq!(doc, original);
        match &patched.nodes[&1].op {
            CsgOp::Cube { size } => assert_eq!(size.x, 42.0),
            _ => panic!(),
        }
    }

    #[test]
    fn bindings_on_various_ops() {
        let mut doc = Document::new();
        doc.nodes.insert(
            1,
            Node {
                id: 1,
                name: None,
                op: CsgOp::Cylinder {
                    radius: 0.0,
                    height: 0.0,
                    segments: 0,
                },
            },
        );
        doc.nodes.insert(
            2,
            Node {
                id: 2,
                name: None,
                op: CsgOp::Translate {
                    child: 1,
                    offset: Vec3::new(0.0, 0.0, 0.0),
                },
            },
        );
        doc.parameters.insert("r".into(), Parameter::literal(3.0));
        doc.parameters.insert("h".into(), Parameter::literal(40.0));
        doc.bindings
            .bind(BindingKey::new(1, "radius"), Expr::formula("r"));
        doc.bindings
            .bind(BindingKey::new(1, "height"), Expr::formula("h"));
        doc.bindings
            .bind(BindingKey::new(2, "offset.z"), Expr::formula("h / 2"));

        resolve_document(&mut doc).unwrap();

        match &doc.nodes[&1].op {
            CsgOp::Cylinder { radius, height, .. } => {
                assert_eq!(*radius, 3.0);
                assert_eq!(*height, 40.0);
            }
            _ => panic!(),
        }
        match &doc.nodes[&2].op {
            CsgOp::Translate { offset, .. } => assert_eq!(offset.z, 20.0),
            _ => panic!(),
        }
    }
}
