//! Web-oriented export: a GLB whose glTF nodes are *addressable by name*,
//! plus a small JSON scene graph for consumers that re-render primitives
//! themselves.
//!
//! The static renderers flatten a document into anonymous line art. A
//! Three.js consumer wants the opposite: one named node per loon root, each
//! with its own PBR material, so `scene.getObjectByName("rail")` works.
//!
//! Two conversions happen on the way out:
//!
//!   - **Z-up → Y-up.** The kernel (and loon) are Z-up; glTF is Y-up. The
//!     −90° rotation about X is *baked into positions and normals*
//!     (`y' = z`, `z' = −y`) rather than emitted as a node transform, so a
//!     consumer that ignores node TRS still sees an upright model. The scene
//!     carries `extras` recording that the swap happened.
//!   - **Assembly instances are world-placed.** Forward kinematics runs
//!     first and the instance pose is baked into the vertices, so every
//!     emitted node has an identity transform. Correctness over glTF
//!     elegance: no consumer has to replicate vcad's FK.

use serde_json::{json, Value};
use std::collections::HashMap;
use vcad_kernel_export::{build_glb, GlbMeshSpec, GlbSpec};

/// What an export produced, for the CLI's stderr summary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebExportSummary {
    /// Named glTF nodes written (GLB) / roots described (JSON).
    pub nodes: usize,
    /// Roots whose whole subtree encoded as primitives (JSON only).
    pub roots_primitive: usize,
    /// Roots carrying at least one `{"type":"mesh"}` fallback (JSON only).
    pub roots_mesh_fallback: usize,
}

/// glTF node names are the consumer's addressing scheme, so they must be
/// unique. Unnamed roots become `part_<n>`; a collision gets `_2`, `_3`, …
fn unique_name(taken: &mut HashMap<String, usize>, raw: Option<&str>, index: usize) -> String {
    let base = match raw.map(str::trim) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => format!("part_{index}"),
    };
    let seen = taken.entry(base.clone()).or_insert(0);
    *seen += 1;
    if *seen == 1 {
        base
    } else {
        format!("{base}_{}", *seen)
    }
}

/// Z-up (kernel) → Y-up (glTF) for one vector, in place over a flat
/// `[x, y, z, …]` buffer: `y' = z`, `z' = −y`. Applies to normals as well as
/// positions — the map is a rotation, so it preserves unit length.
fn z_up_to_y_up(flat: &mut [f32]) {
    for v in flat.as_chunks_mut::<3>().0 {
        let (y, z) = (v[1], v[2]);
        v[1] = z;
        v[2] = -y;
    }
}

/// Build a GLB where each visible root (and each assembly instance) is a
/// named node with its own resolved PBR material.
///
/// `segments` is the curved-face facet count; pass the same value to
/// [`crate::with_root_cache`] or a cache hit serves a differently faceted
/// mesh.
pub fn export_web_glb(
    raw_vcad: &str,
    segments: u32,
) -> Result<(Vec<u8>, WebExportSummary), String> {
    let scene = super::evaluate_vcad(raw_vcad)?;

    let mut f32_data: Vec<f32> = Vec::new();
    let mut u32_data: Vec<u32> = Vec::new();
    let mut meshes: Vec<GlbMeshSpec> = Vec::new();
    let mut taken: HashMap<String, usize> = HashMap::new();

    for (i, s) in scene.iter().enumerate() {
        let mesh =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.solid.to_mesh(segments)));
        let Ok(mesh) = mesh else { continue };
        if mesh.indices.is_empty() || mesh.vertices.is_empty() {
            continue;
        }

        let mut positions = mesh.vertices.clone();
        z_up_to_y_up(&mut positions);
        let pos_span = [f32_data.len(), positions.len()];
        f32_data.extend_from_slice(&positions);

        let normals = if mesh.normals.len() == mesh.vertices.len() {
            let mut n = mesh.normals.clone();
            z_up_to_y_up(&mut n);
            let span = [f32_data.len(), n.len()];
            f32_data.extend_from_slice(&n);
            Some(span)
        } else {
            None
        };

        let idx_span = [u32_data.len(), mesh.indices.len()];
        u32_data.extend_from_slice(&mesh.indices);

        let m = s
            .material
            .clone()
            .or_else(|| crate::materials::builtin("default"))
            .unwrap_or_default();
        // Approximate glass: a transmissive material reads as alpha-blended
        // rather than as KHR_materials_transmission (which the shared writer
        // does not emit).
        let alpha = m.transmission.filter(|t| *t > 0.0).map(|t| 1.0 - t);

        meshes.push(GlbMeshSpec {
            name: unique_name(&mut taken, s.name.as_deref(), i),
            positions: pos_span,
            indices: idx_span,
            normals,
            color: m.color,
            metallic: m.metallic,
            roughness: m.roughness,
            emissive: None,
            emissive_strength: None,
            clearcoat: None,
            clearcoat_roughness: None,
            alpha,
            transform: None,
            mesh_key: None,
        });
    }

    let summary = WebExportSummary {
        nodes: meshes.len(),
        ..Default::default()
    };
    let spec = GlbSpec {
        name: "vcad".into(),
        meshes,
        animation: None,
        scene_extras: Some(json!({
            "io.vcad/source_up_axis": "+Z",
            "io.vcad/converted_to_y_up": true,
        })),
    };
    let bytes = build_glb(&spec, &f32_data, &u32_data).map_err(|e| e.to_string())?;
    Ok((bytes, summary))
}

// ─── JSON scene graph ─────────────────────────────────────────────────────

/// The op subset a JSON consumer can rebuild from primitives. Anything else
/// (booleans, fillets, sweeps, sketches, …) is not expressible as a
/// primitive tree and becomes a `mesh` placeholder — the consumer is
/// expected to fall back to the GLB for those.
fn encode_node(id: vcad_ir::NodeId, nodes: &HashMap<vcad_ir::NodeId, vcad_ir::Node>) -> Value {
    use vcad_ir::CsgOp;
    let Some(node) = nodes.get(&id) else {
        return json!({ "type": "mesh", "reason": "dangling node reference" });
    };
    let v3 = |v: &vcad_ir::Vec3| json!([v.x, v.y, v.z]);
    let child = |c: vcad_ir::NodeId| encode_node(c, nodes);
    let name = node.name.clone();
    let mut out = match &node.op {
        CsgOp::Cube { size } => json!({ "type": "cube", "size": v3(size) }),
        CsgOp::Cylinder {
            radius,
            height,
            segments,
        } => {
            json!({ "type": "cylinder", "radius": radius, "height": height, "segments": segments })
        }
        CsgOp::Sphere { radius, segments } => {
            json!({ "type": "sphere", "radius": radius, "segments": segments })
        }
        CsgOp::Cone {
            radius_bottom,
            radius_top,
            height,
            segments,
        } => json!({
            "type": "cone", "radius_bottom": radius_bottom, "radius_top": radius_top,
            "height": height, "segments": segments
        }),
        CsgOp::Torus {
            major_radius,
            minor_radius,
            segments,
        } => json!({
            "type": "torus", "major_radius": major_radius,
            "minor_radius": minor_radius, "segments": segments
        }),
        CsgOp::Translate { child: c, offset } => {
            json!({ "type": "translate", "offset": v3(offset), "child": child(*c) })
        }
        CsgOp::Rotate { child: c, angles } => {
            json!({ "type": "rotate", "angles_deg": v3(angles), "child": child(*c) })
        }
        CsgOp::Scale { child: c, factor } => {
            json!({ "type": "scale", "factor": v3(factor), "child": child(*c) })
        }
        CsgOp::Mirror {
            child: c,
            plane_origin,
            plane_normal,
        } => json!({
            "type": "mirror", "plane_origin": v3(plane_origin),
            "plane_normal": v3(plane_normal), "child": child(*c)
        }),
        CsgOp::Union { left, right } => {
            json!({ "type": "union", "children": [child(*left), child(*right)] })
        }
        CsgOp::LinearPattern {
            child: c,
            direction,
            count,
            spacing,
        } => json!({
            "type": "linear_pattern", "direction": v3(direction),
            "count": count, "spacing": spacing, "child": child(*c)
        }),
        CsgOp::CircularPattern {
            child: c,
            axis_origin,
            axis_dir,
            count,
            angle_deg,
        } => json!({
            "type": "circular_pattern", "axis_origin": v3(axis_origin),
            "axis_dir": v3(axis_dir), "count": count, "angle_deg": angle_deg,
            "child": child(*c)
        }),
        other => json!({
            "type": "mesh",
            "reason": format!("{} not representable as primitives", op_name(other)),
        }),
    };
    if let (Some(name), Some(obj)) = (name, out.as_object_mut()) {
        obj.insert("name".into(), json!(name));
    }
    out
}

/// The IR `type` tag for an op, for the fallback reason string.
fn op_name(op: &vcad_ir::CsgOp) -> String {
    serde_json::to_value(op)
        .ok()
        .and_then(|v| v.get("type").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| "unknown op".to_string())
}

/// Does this encoded subtree contain a mesh fallback?
fn has_fallback(v: &Value) -> bool {
    if v.get("type").and_then(Value::as_str) == Some("mesh") {
        return true;
    }
    match v.get("child") {
        Some(c) if has_fallback(c) => return true,
        _ => {}
    }
    v.get("children")
        .and_then(Value::as_array)
        .is_some_and(|cs| cs.iter().any(has_fallback))
}

/// Emit the primitive scene graph for a document's visible roots.
///
/// Never fails on an unsupported op — the offending subtree becomes
/// `{"type":"mesh", …}` and the summary counts it.
pub fn export_web_json(raw_vcad: &str) -> Result<(String, WebExportSummary), String> {
    // The primitive graph is read straight off the IR — no evaluation, so an
    // op the kernel would choke on still exports (as a mesh fallback).
    let parsed = vcad_ir::file_io::parse_vcad_file(raw_vcad).map_err(|e| format!("parse: {e}"))?;
    let doc = &parsed.document;
    let mut roots = Vec::new();
    let mut summary = WebExportSummary::default();
    let mut taken: HashMap<String, usize> = HashMap::new();

    for (i, entry) in doc
        .roots
        .iter()
        .filter(|r| r.visible != Some(false))
        .enumerate()
    {
        let raw_name = doc.nodes.get(&entry.root).and_then(|n| n.name.as_deref());
        let name = unique_name(&mut taken, raw_name, i);
        let material = crate::materials::resolve(&doc.materials, &entry.material).map(|m| {
            json!({
                "name": m.name, "color": m.color,
                "metallic": m.metallic, "roughness": m.roughness,
            })
        });
        let node = encode_node(entry.root, &doc.nodes);
        if has_fallback(&node) {
            summary.roots_mesh_fallback += 1;
        } else {
            summary.roots_primitive += 1;
        }
        roots.push(json!({ "name": name, "material": material, "node": node }));
    }
    summary.nodes = roots.len();

    let out = json!({
        "version": "1",
        "up_axis": "+Z",
        "units": "mm",
        "roots": roots,
    });
    serde_json::to_string_pretty(&out)
        .map(|s| (s, summary))
        .map_err(|e| format!("serialize scene graph: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_deduped_and_synthesized() {
        let mut taken = HashMap::new();
        assert_eq!(unique_name(&mut taken, Some("rail"), 0), "rail");
        assert_eq!(unique_name(&mut taken, Some("rail"), 1), "rail_2");
        assert_eq!(unique_name(&mut taken, Some("rail"), 2), "rail_3");
        assert_eq!(unique_name(&mut taken, None, 3), "part_3");
        assert_eq!(unique_name(&mut taken, Some("  "), 4), "part_4");
        assert_eq!(unique_name(&mut taken, Some("part_3"), 5), "part_3_2");
    }

    #[test]
    fn z_up_becomes_y_up() {
        // A point 10mm "up" in kernel Z-up lands 10mm up glTF's +Y.
        let mut v = vec![1.0f32, 2.0, 10.0];
        z_up_to_y_up(&mut v);
        assert_eq!(v, vec![1.0, 10.0, -2.0]);
        // The map is a rotation: it preserves length and handedness.
        let mut basis = vec![1.0f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        z_up_to_y_up(&mut basis);
        assert_eq!(basis, vec![1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 1.0, 0.0]);
    }

    #[test]
    fn boolean_roots_fall_back_to_mesh_without_erroring() {
        let raw = r#"{
            "version": "1",
            "nodes": {
                "1": {"id": 1, "name": "a", "op": {"type": "Cube", "size": {"x": 10, "y": 10, "z": 10}}},
                "2": {"id": 2, "name": "b", "op": {"type": "Sphere", "radius": 6, "segments": 0}},
                "3": {"id": 3, "name": "cut", "op": {"type": "Difference", "left": 1, "right": 2}}
            },
            "roots": [{"root": 3, "material": "aluminum"}],
            "materials": {},
            "part_materials": {}
        }"#;
        let (json_str, summary) = export_web_json(raw).expect("export");
        let v: Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(v["up_axis"], "+Z");
        assert_eq!(v["roots"][0]["name"], "cut");
        assert_eq!(v["roots"][0]["node"]["type"], "mesh");
        assert!(v["roots"][0]["node"]["reason"]
            .as_str()
            .unwrap()
            .contains("not representable"));
        assert_eq!(v["roots"][0]["material"]["name"], "aluminum");
        assert_eq!(summary.roots_mesh_fallback, 1);
        assert_eq!(summary.roots_primitive, 0);
    }

    #[test]
    fn primitive_tree_survives_intact() {
        let raw = r#"{
            "version": "1",
            "nodes": {
                "1": {"id": 1, "name": null, "op": {"type": "Cylinder", "radius": 3, "height": 20, "segments": 32}},
                "2": {"id": 2, "name": "post", "op": {"type": "Translate", "child": 1, "offset": {"x": 1, "y": 2, "z": 3}}}
            },
            "roots": [{"root": 2, "material": "abs-black"}],
            "materials": {},
            "part_materials": {}
        }"#;
        let (json_str, summary) = export_web_json(raw).expect("export");
        let v: Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(summary.roots_primitive, 1);
        assert_eq!(v["roots"][0]["node"]["type"], "translate");
        assert_eq!(v["roots"][0]["node"]["offset"], json!([1.0, 2.0, 3.0]));
        assert_eq!(v["roots"][0]["node"]["child"]["type"], "cylinder");
        assert_eq!(v["roots"][0]["node"]["child"]["radius"], 3.0);
    }

    #[test]
    fn glb_carries_named_nodes_and_up_axis_extras() {
        let raw = r#"{
            "version": "1",
            "nodes": {
                "1": {"id": 1, "name": "tower", "op": {"type": "Cube", "size": {"x": 2, "y": 2, "z": 40}}}
            },
            "roots": [{"root": 1, "material": "aluminum"}],
            "materials": {},
            "part_materials": {}
        }"#;
        let (glb, summary) = export_web_glb(raw, 16).expect("glb");
        assert_eq!(summary.nodes, 1);
        // Chunk 0 of a GLB is the JSON chunk, starting at byte 20.
        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let v: Value = serde_json::from_slice(&glb[20..20 + json_len]).unwrap();
        assert_eq!(v["nodes"][0]["name"], "tower");
        assert_eq!(v["scenes"][0]["extras"]["io.vcad/converted_to_y_up"], true);
        assert!(v["meshes"][0]["primitives"][0]["attributes"]["NORMAL"].is_number());
        // A 40mm-tall (Z) cube spans 40mm in glTF's +Y after conversion.
        let min = v["accessors"][1]["min"].as_array().unwrap();
        let max = v["accessors"][1]["max"].as_array().unwrap();
        let span_y = max[1].as_f64().unwrap() - min[1].as_f64().unwrap();
        assert!(
            (span_y - 40.0).abs() < 1e-3,
            "expected 40mm in Y, got {span_y}"
        );
    }
}
