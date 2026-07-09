//! Convert a [`Document`] back to loon source code.
//!
//! Walks the node graph in topological order (dependencies before dependents)
//! and emits `[let nN ...]` bindings for each node, with `[root ...]` entries
//! for scene roots. Materials are emitted first.

use std::collections::HashSet;
use std::fmt::Write;

use crate::{CsgOp, Document, Node, NodeId, PathCurve, SketchSegment2D};

/// Convert a [`Document`] to loon source code, also returning names of unsupported variants.
///
/// Returns `(source, unsupported)` where `unsupported` is a list of variant type names
/// (e.g. `"Text2D"`, `"ImportedMesh"`) that could not be represented in loon. Callers
/// should warn the user when this list is non-empty — data will be lost on round-trip.
pub fn document_to_loon_checked(doc: &Document) -> (String, Vec<String>) {
    let mut lines: Vec<String> = Vec::new();
    let mut emitted = HashSet::new();
    let mut unsupported: Vec<String> = Vec::new();

    // Header
    lines.push("; Generated from vcad document".to_string());
    lines.push(String::new());

    // Emit materials (before geometry so they can be referenced)
    let mut mat_entries: Vec<_> = doc.materials.iter().collect();
    mat_entries.sort_by_key(|(k, _)| (*k).clone());
    if !mat_entries.is_empty() {
        for (key, mat) in &mat_entries {
            if key.as_str() == "default" {
                continue;
            }
            lines.push(format!(
                "[let mat-{} [material {:?} {} {} {} {} {}]]",
                sanitize_name(key),
                mat.name,
                fmt_f64(mat.color[0]),
                fmt_f64(mat.color[1]),
                fmt_f64(mat.color[2]),
                fmt_f64(mat.metallic),
                fmt_f64(mat.roughness),
            ));
        }
        lines.push(String::new());
    }

    // Topological sort: emit dependencies first
    let order = topo_sort(doc);

    for node_id in &order {
        let Some(node) = doc.nodes.get(node_id) else {
            continue;
        };
        emit_node(node, doc, &mut lines, &mut emitted, &mut unsupported);
    }

    // Emit scene roots
    if !doc.roots.is_empty() {
        lines.push(String::new());
        if doc.roots.len() == 1 {
            let entry = &doc.roots[0];
            let name = node_name(entry.root, doc);
            let mat = if entry.material.is_empty() {
                "default"
            } else {
                &entry.material
            };
            lines.push(format!("[root {} {:?}]", name, mat));
        } else {
            lines.push("#[".to_string());
            for entry in &doc.roots {
                let name = node_name(entry.root, doc);
                let mat = if entry.material.is_empty() {
                    "default"
                } else {
                    &entry.material
                };
                lines.push(format!("  [root {} {:?}]", name, mat));
            }
            lines.push("]".to_string());
        }
    }

    // Deduplicate while preserving first-seen order
    let mut seen = HashSet::new();
    unsupported.retain(|s| seen.insert(s.clone()));

    let mut result = lines.join("\n");
    result.push('\n');
    (result, unsupported)
}

/// Convert a [`Document`] to loon source code.
///
/// Silently ignores unsupported variants (they are replaced with comment placeholders).
/// Use [`document_to_loon_checked`] to also receive a list of dropped variant names.
pub fn document_to_loon(doc: &Document) -> String {
    let (source, _) = document_to_loon_checked(doc);
    source
}

/// Sanitize a name for use as a loon identifier.
fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// Get a readable name for a node — uses `node.name` if available, else `nN`.
fn node_name(id: NodeId, doc: &Document) -> String {
    if let Some(node) = doc.nodes.get(&id) {
        if let Some(name) = &node.name {
            return sanitize_name(name);
        }
    }
    format!("n{}", id)
}

fn emit_node(
    node: &Node,
    doc: &Document,
    lines: &mut Vec<String>,
    emitted: &mut HashSet<NodeId>,
    unsupported: &mut Vec<String>,
) {
    if emitted.contains(&node.id) {
        return;
    }
    emitted.insert(node.id);

    // Ensure dependencies are emitted first
    for dep in get_child_ids(&node.op) {
        if let Some(dep_node) = doc.nodes.get(&dep) {
            emit_node(dep_node, doc, lines, emitted, unsupported);
        }
    }

    let name = node_name(node.id, doc);
    match op_to_loon(&node.op, doc) {
        OpResult::Ok(expr) => lines.push(format!("[let {} {}]", name, expr)),
        OpResult::Unsupported(variant, comment) => {
            unsupported.push(variant);
            lines.push(format!("[let {} {}]", name, comment));
        }
    }
}

/// Result of converting a single op to loon.
enum OpResult {
    /// Successfully converted to loon expression.
    Ok(String),
    /// Variant is not yet supported; emits a comment placeholder.
    /// The first field is the variant name for the warning list.
    Unsupported(String, String),
}

/// Format a number for loon output. Always includes decimal for floats.
fn fmt_f64(n: f64) -> String {
    if n == n.trunc() && n.is_finite() {
        format!("{:.1}", n)
    } else {
        // Avoid excessive precision, strip trailing zeros
        let s = format!("{:.10}", n);
        let s = s.trim_end_matches('0');
        let s = if s.ends_with('.') {
            format!("{}0", s)
        } else {
            s.to_string()
        };
        s
    }
}

fn node_ref(id: NodeId, doc: &Document) -> String {
    node_name(id, doc)
}

#[allow(clippy::too_many_lines)]
fn op_to_loon(op: &CsgOp, doc: &Document) -> OpResult {
    match op {
        CsgOp::Cube { size } => OpResult::Ok(format!(
            "[cube {} {} {}]",
            fmt_f64(size.x),
            fmt_f64(size.y),
            fmt_f64(size.z)
        )),

        CsgOp::Cylinder { radius, height, .. } => OpResult::Ok(format!(
            "[cylinder {} {}]",
            fmt_f64(*radius),
            fmt_f64(*height)
        )),

        CsgOp::Sphere { radius, .. } => OpResult::Ok(format!("[sphere {}]", fmt_f64(*radius))),

        CsgOp::Cone {
            radius_bottom,
            radius_top,
            height,
            ..
        } => OpResult::Ok(format!(
            "[cone {} {} {}]",
            fmt_f64(*radius_bottom),
            fmt_f64(*radius_top),
            fmt_f64(*height)
        )),

        CsgOp::Torus {
            major_radius,
            minor_radius,
            ..
        } => OpResult::Ok(format!(
            "[torus {} {}]",
            fmt_f64(*major_radius),
            fmt_f64(*minor_radius)
        )),

        CsgOp::Wedge { size } => OpResult::Ok(format!(
            "[wedge {} {} {}]",
            fmt_f64(size.x),
            fmt_f64(size.y),
            fmt_f64(size.z)
        )),

        CsgOp::Prism {
            sides,
            radius,
            height,
        } => OpResult::Ok(format!(
            "[prism {} {} {}]",
            sides,
            fmt_f64(*radius),
            fmt_f64(*height)
        )),

        CsgOp::Empty => OpResult::Ok("Empty".to_string()),

        CsgOp::Union { left, right } => OpResult::Ok(format!(
            "[union {} {}]",
            node_ref(*left, doc),
            node_ref(*right, doc)
        )),

        CsgOp::Difference { left, right } => OpResult::Ok(format!(
            "[difference {} {}]",
            node_ref(*left, doc),
            node_ref(*right, doc)
        )),

        CsgOp::Intersection { left, right } => OpResult::Ok(format!(
            "[intersection {} {}]",
            node_ref(*left, doc),
            node_ref(*right, doc)
        )),

        CsgOp::Translate { child, offset } => OpResult::Ok(format!(
            "[translate {} {} {} {}]",
            fmt_f64(offset.x),
            fmt_f64(offset.y),
            fmt_f64(offset.z),
            node_ref(*child, doc)
        )),

        CsgOp::Rotate { child, angles } => OpResult::Ok(format!(
            "[rotate {} {} {} {}]",
            fmt_f64(angles.x),
            fmt_f64(angles.y),
            fmt_f64(angles.z),
            node_ref(*child, doc)
        )),

        CsgOp::Scale { child, factor } => OpResult::Ok(format!(
            "[scale {} {} {} {}]",
            fmt_f64(factor.x),
            fmt_f64(factor.y),
            fmt_f64(factor.z),
            node_ref(*child, doc)
        )),

        CsgOp::Mirror {
            child,
            plane_origin,
            plane_normal,
        } => OpResult::Ok(format!(
            "[mirror {} {} {} {} {} {} {}]",
            fmt_f64(plane_origin.x),
            fmt_f64(plane_origin.y),
            fmt_f64(plane_origin.z),
            fmt_f64(plane_normal.x),
            fmt_f64(plane_normal.y),
            fmt_f64(plane_normal.z),
            node_ref(*child, doc)
        )),

        CsgOp::Fillet { child, radius } => OpResult::Ok(format!(
            "[fillet {} {}]",
            fmt_f64(*radius),
            node_ref(*child, doc)
        )),

        CsgOp::Chamfer { child, distance } => OpResult::Ok(format!(
            "[chamfer {} {}]",
            fmt_f64(*distance),
            node_ref(*child, doc)
        )),

        CsgOp::EdgeBlendLoft { .. } => OpResult::Unsupported(
            "EdgeBlendLoft".to_string(),
            "variable edge blend has no loon syntax yet".to_string(),
        ),

        CsgOp::Shell { child, thickness } => OpResult::Ok(format!(
            "[shell {} {}]",
            fmt_f64(*thickness),
            node_ref(*child, doc)
        )),

        CsgOp::LinearPattern {
            child,
            direction,
            count,
            spacing,
        } => OpResult::Ok(format!(
            "[linear-pattern {} {} {} {} {} {}]",
            fmt_f64(direction.x),
            fmt_f64(direction.y),
            fmt_f64(direction.z),
            count,
            fmt_f64(*spacing),
            node_ref(*child, doc)
        )),

        CsgOp::CircularPattern {
            child,
            axis_origin,
            axis_dir,
            count,
            angle_deg,
        } => OpResult::Ok(format!(
            "[circular-pattern {} {} {} {} {} {} {} {} {}]",
            fmt_f64(axis_origin.x),
            fmt_f64(axis_origin.y),
            fmt_f64(axis_origin.z),
            fmt_f64(axis_dir.x),
            fmt_f64(axis_dir.y),
            fmt_f64(axis_dir.z),
            count,
            fmt_f64(*angle_deg),
            node_ref(*child, doc)
        )),

        CsgOp::Sketch2D {
            origin,
            x_dir,
            y_dir,
            segments,
            holes,
        } => {
            // The loon CAD dialect has no hole-loop syntax yet; holed
            // sketches emit a comment placeholder like other unsupported ops.
            if holes.as_ref().is_some_and(|h| !h.is_empty()) {
                return OpResult::Unsupported(
                    "Sketch2D (with holes)".to_string(),
                    "[cube 1.0 1.0 1.0] ; TODO: Sketch2D with interior holes not yet supported in loon".to_string(),
                );
            }
            let mut buf = String::new();
            let _ = writeln!(buf, "[sketch");
            let _ = writeln!(
                buf,
                "  {} {} {}",
                fmt_f64(origin.x),
                fmt_f64(origin.y),
                fmt_f64(origin.z)
            );
            let _ = writeln!(
                buf,
                "  {} {} {}",
                fmt_f64(x_dir.x),
                fmt_f64(x_dir.y),
                fmt_f64(x_dir.z)
            );
            let _ = writeln!(
                buf,
                "  {} {} {}",
                fmt_f64(y_dir.x),
                fmt_f64(y_dir.y),
                fmt_f64(y_dir.z)
            );
            let _ = writeln!(buf, "  #[");
            for seg in segments {
                match seg {
                    SketchSegment2D::Line { start, end } => {
                        let _ = writeln!(
                            buf,
                            "    [line {} {} {} {}]",
                            fmt_f64(start.x),
                            fmt_f64(start.y),
                            fmt_f64(end.x),
                            fmt_f64(end.y)
                        );
                    }
                    SketchSegment2D::Arc {
                        start,
                        end,
                        center,
                        ccw,
                    } => {
                        let _ = writeln!(
                            buf,
                            "    [arc {} {} {} {} {} {} {}]",
                            fmt_f64(start.x),
                            fmt_f64(start.y),
                            fmt_f64(end.x),
                            fmt_f64(end.y),
                            fmt_f64(center.x),
                            fmt_f64(center.y),
                            ccw
                        );
                    }
                }
            }
            let _ = write!(buf, "  ]]");
            OpResult::Ok(buf)
        }

        CsgOp::Extrude {
            sketch, direction, ..
        } => OpResult::Ok(format!(
            "[extrude {} {} {} {}]",
            fmt_f64(direction.x),
            fmt_f64(direction.y),
            fmt_f64(direction.z),
            node_ref(*sketch, doc)
        )),

        CsgOp::Revolve {
            sketch,
            axis_origin,
            axis_dir,
            angle_deg,
        } => OpResult::Ok(format!(
            "[revolve {} {} {} {} {} {} {} {}]",
            fmt_f64(axis_origin.x),
            fmt_f64(axis_origin.y),
            fmt_f64(axis_origin.z),
            fmt_f64(axis_dir.x),
            fmt_f64(axis_dir.y),
            fmt_f64(axis_dir.z),
            fmt_f64(*angle_deg),
            node_ref(*sketch, doc)
        )),

        CsgOp::Sweep { sketch, path, .. } => {
            let sk = node_ref(*sketch, doc);
            match path {
                PathCurve::Line { start, end } => OpResult::Ok(format!(
                    "[sweep-line {} {} {} {} {} {} {}]",
                    fmt_f64(start.x),
                    fmt_f64(start.y),
                    fmt_f64(start.z),
                    fmt_f64(end.x),
                    fmt_f64(end.y),
                    fmt_f64(end.z),
                    sk
                )),
                PathCurve::Helix {
                    radius,
                    pitch,
                    height,
                    turns,
                } => OpResult::Ok(format!(
                    "[sweep-helix {} {} {} {} {}]",
                    fmt_f64(*radius),
                    fmt_f64(*pitch),
                    fmt_f64(*height),
                    fmt_f64(*turns),
                    sk
                )),
            }
        }

        CsgOp::Loft { sketches, closed } => {
            let refs: Vec<String> = sketches.iter().map(|id| node_ref(*id, doc)).collect();
            let refs_str = refs.join(" ");
            if closed.unwrap_or(false) {
                OpResult::Ok(format!("[loft-closed #[{}]]", refs_str))
            } else {
                OpResult::Ok(format!("[loft #[{}]]", refs_str))
            }
        }

        CsgOp::Text2D { text, height, .. } => OpResult::Unsupported(
            "Text2D".to_string(),
            format!(
                "; Text2D {:?} (h={}) — not yet supported in loon",
                text,
                fmt_f64(*height)
            ),
        ),

        CsgOp::ImportedMesh {
            positions,
            indices,
            source,
            ..
        } => {
            let verts = positions.len() / 3;
            let tris = indices.len() / 3;
            let src = source
                .as_ref()
                .map(|s| format!(", source: {}", s))
                .unwrap_or_default();
            OpResult::Unsupported(
                "ImportedMesh".to_string(),
                format!(
                    "; ImportedMesh ({} vertices, {} triangles{}) — not representable in loon",
                    verts, tris, src
                ),
            )
        }

        CsgOp::StepImport { path } => OpResult::Unsupported(
            "StepImport".to_string(),
            format!("; StepImport {:?} — not yet supported in loon", path),
        ),

        CsgOp::MeshImport { path, .. } => OpResult::Unsupported(
            "MeshImport".to_string(),
            format!("; MeshImport {:?} — not yet supported in loon", path),
        ),

        CsgOp::PcbBoard { .. } => OpResult::Unsupported(
            "PcbBoard".to_string(),
            "; PcbBoard — not yet supported in loon".to_string(),
        ),

        CsgOp::EmbroideryPattern { .. } => OpResult::Unsupported(
            "EmbroideryPattern".to_string(),
            "; EmbroideryPattern — not yet supported in loon".to_string(),
        ),

        CsgOp::PartInstance {
            path,
            version,
            params,
        } => {
            let mut kv: Vec<(String, String)> = params
                .iter()
                .map(|(k, v)| (k.clone(), fmt_json_value(v)))
                .collect();
            kv.sort_by(|a, b| a.0.cmp(&b.0));
            let params_str = kv
                .iter()
                .map(|(k, v)| format!(":{k} {v}"))
                .collect::<Vec<_>>()
                .join(" ");
            OpResult::Ok(format!(
                "[part-instance {:?} {:?} #{{{}}}]",
                path, version, params_str
            ))
        }

        CsgOp::SheetMetalBaseFlangeRect { .. } => OpResult::Unsupported(
            "SheetMetalBaseFlangeRect".to_string(),
            "; SheetMetalBaseFlangeRect — not yet supported in loon".to_string(),
        ),
        CsgOp::SheetMetalEdgeFlange { .. } => OpResult::Unsupported(
            "SheetMetalEdgeFlange".to_string(),
            "; SheetMetalEdgeFlange — not yet supported in loon".to_string(),
        ),
        CsgOp::SheetMetalHem { .. } => OpResult::Unsupported(
            "SheetMetalHem".to_string(),
            "; SheetMetalHem — not yet supported in loon".to_string(),
        ),
        CsgOp::SheetMetalJog { .. } => OpResult::Unsupported(
            "SheetMetalJog".to_string(),
            "; SheetMetalJog — not yet supported in loon".to_string(),
        ),
        CsgOp::SheetMetalBaseFlangePolygon { .. } => OpResult::Unsupported(
            "SheetMetalBaseFlangePolygon".to_string(),
            "; SheetMetalBaseFlangePolygon — not yet supported in loon".to_string(),
        ),
        CsgOp::SheetMetalBendRelief { .. } => OpResult::Unsupported(
            "SheetMetalBendRelief".to_string(),
            "; SheetMetalBendRelief — not yet supported in loon".to_string(),
        ),
    }
}

fn fmt_json_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => format!("{s:?}"),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "nil".to_string(),
        _ => format!("{:?}", v.to_string()),
    }
}

/// Get child node IDs referenced by an operation.
fn get_child_ids(op: &CsgOp) -> Vec<NodeId> {
    match op {
        CsgOp::Translate { child, .. }
        | CsgOp::Rotate { child, .. }
        | CsgOp::Scale { child, .. }
        | CsgOp::Mirror { child, .. }
        | CsgOp::Fillet { child, .. }
        | CsgOp::Chamfer { child, .. }
        | CsgOp::Shell { child, .. }
        | CsgOp::LinearPattern { child, .. }
        | CsgOp::CircularPattern { child, .. } => vec![*child],

        CsgOp::Union { left, right }
        | CsgOp::Difference { left, right }
        | CsgOp::Intersection { left, right } => vec![*left, *right],

        CsgOp::Extrude { sketch, .. }
        | CsgOp::Revolve { sketch, .. }
        | CsgOp::Sweep { sketch, .. } => vec![*sketch],

        CsgOp::Loft { sketches, .. } => sketches.clone(),

        _ => vec![],
    }
}

/// Topological sort of node IDs (dependencies first).
fn topo_sort(doc: &Document) -> Vec<NodeId> {
    let mut visited = HashSet::new();
    let mut order = Vec::new();

    fn visit(id: NodeId, doc: &Document, visited: &mut HashSet<NodeId>, order: &mut Vec<NodeId>) {
        if visited.contains(&id) {
            return;
        }
        visited.insert(id);
        if let Some(node) = doc.nodes.get(&id) {
            for child in get_child_ids(&node.op) {
                visit(child, doc, visited, order);
            }
        }
        order.push(id);
    }

    // Start from roots
    for entry in &doc.roots {
        visit(entry.root, doc, &mut visited, &mut order);
    }
    // Also visit any orphaned nodes
    let mut all_ids: Vec<_> = doc.nodes.keys().copied().collect();
    all_ids.sort_unstable();
    for id in all_ids {
        visit(id, doc, &mut visited, &mut order);
    }

    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MaterialDef, SceneEntry, Vec3};

    fn make_doc_with_cube() -> Document {
        let mut doc = Document::new();
        doc.nodes.insert(
            1,
            Node {
                id: 1,
                name: Some("box".to_string()),
                op: CsgOp::Cube {
                    size: Vec3::new(10.0, 20.0, 30.0),
                },
            },
        );
        doc.roots.push(SceneEntry {
            root: 1,
            material: "default".to_string(),
            visible: None,
        });
        doc
    }

    #[test]
    fn simple_cube() {
        let doc = make_doc_with_cube();
        let loon = document_to_loon(&doc);
        assert!(loon.contains("[let box [cube 10.0 20.0 30.0]]"));
        assert!(loon.contains("[root box \"default\"]"));
    }

    #[test]
    fn boolean_union() {
        let mut doc = Document::new();
        doc.nodes.insert(
            1,
            Node {
                id: 1,
                name: Some("a".into()),
                op: CsgOp::Cube {
                    size: Vec3::new(10.0, 10.0, 10.0),
                },
            },
        );
        doc.nodes.insert(
            2,
            Node {
                id: 2,
                name: Some("b".into()),
                op: CsgOp::Sphere {
                    radius: 5.0,
                    segments: 0,
                },
            },
        );
        doc.nodes.insert(
            3,
            Node {
                id: 3,
                name: Some("merged".into()),
                op: CsgOp::Union { left: 1, right: 2 },
            },
        );
        doc.roots.push(SceneEntry {
            root: 3,
            material: "default".into(),
            visible: None,
        });
        let loon = document_to_loon(&doc);
        assert!(loon.contains("[let merged [union a b]]"));
    }

    #[test]
    fn materials_emitted_first() {
        let mut doc = make_doc_with_cube();
        doc.materials.insert(
            "steel".to_string(),
            MaterialDef {
                name: "steel".to_string(),
                color: [0.8, 0.8, 0.85],
                metallic: 1.0,
                roughness: 0.3,
                density: None,
                friction: None,
                ..Default::default()
            },
        );
        let loon = document_to_loon(&doc);
        let mat_pos = loon.find("[let mat-steel").unwrap();
        let node_pos = loon.find("[let box").unwrap();
        assert!(
            mat_pos < node_pos,
            "materials should be emitted before nodes"
        );
    }

    #[test]
    fn multiple_roots() {
        let mut doc = Document::new();
        doc.nodes.insert(
            1,
            Node {
                id: 1,
                name: Some("a".into()),
                op: CsgOp::Cube {
                    size: Vec3::new(1.0, 1.0, 1.0),
                },
            },
        );
        doc.nodes.insert(
            2,
            Node {
                id: 2,
                name: Some("b".into()),
                op: CsgOp::Sphere {
                    radius: 2.0,
                    segments: 0,
                },
            },
        );
        doc.roots.push(SceneEntry {
            root: 1,
            material: "steel".into(),
            visible: None,
        });
        doc.roots.push(SceneEntry {
            root: 2,
            material: "glass".into(),
            visible: None,
        });
        let loon = document_to_loon(&doc);
        assert!(loon.contains("#["));
        assert!(loon.contains("[root a \"steel\"]"));
        assert!(loon.contains("[root b \"glass\"]"));
    }

    #[test]
    fn transform_chain() {
        let mut doc = Document::new();
        doc.nodes.insert(
            1,
            Node {
                id: 1,
                name: Some("c".into()),
                op: CsgOp::Cube {
                    size: Vec3::new(5.0, 5.0, 5.0),
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
                    offset: Vec3::new(10.0, 0.0, 0.0),
                },
            },
        );
        doc.roots.push(SceneEntry {
            root: 2,
            material: "default".into(),
            visible: None,
        });
        let loon = document_to_loon(&doc);
        assert!(loon.contains("[let c [cube 5.0 5.0 5.0]]"));
        assert!(loon.contains("[let n2 [translate 10.0 0.0 0.0 c]]"));
    }

    #[test]
    fn fmt_precision() {
        assert_eq!(fmt_f64(10.0), "10.0");
        assert_eq!(fmt_f64(3.14259), "3.14259");
        assert_eq!(fmt_f64(0.0), "0.0");
        assert_eq!(fmt_f64(1.5), "1.5");
    }

    #[test]
    fn sanitize_name_special_chars() {
        assert_eq!(sanitize_name("My Part #1"), "my-part--1");
        assert_eq!(sanitize_name("hello_world"), "hello_world");
    }

    #[test]
    fn checked_no_unsupported() {
        let doc = make_doc_with_cube();
        let (loon, unsupported) = document_to_loon_checked(&doc);
        assert!(loon.contains("[let box [cube 10.0 20.0 30.0]]"));
        assert!(
            unsupported.is_empty(),
            "cube should have no unsupported variants"
        );
    }

    #[test]
    fn checked_text2d_is_unsupported() {
        let mut doc = Document::new();
        doc.nodes.insert(
            1,
            Node {
                id: 1,
                name: Some("label".into()),
                op: CsgOp::Text2D {
                    origin: Vec3::new(0.0, 0.0, 0.0),
                    x_dir: Vec3::new(1.0, 0.0, 0.0),
                    y_dir: Vec3::new(0.0, 1.0, 0.0),
                    text: "Hello".to_string(),
                    font: "sans-serif".to_string(),
                    height: 10.0,
                    letter_spacing: None,
                    line_spacing: None,
                    alignment: crate::TextAlignment::default(),
                },
            },
        );
        doc.roots.push(SceneEntry {
            root: 1,
            material: "default".into(),
            visible: None,
        });
        let (loon, unsupported) = document_to_loon_checked(&doc);
        assert!(unsupported.contains(&"Text2D".to_string()));
        // Comment placeholder should still appear in output
        assert!(loon.contains("; Text2D"));
    }

    #[test]
    fn checked_imported_mesh_is_unsupported() {
        let mut doc = Document::new();
        doc.nodes.insert(
            1,
            Node {
                id: 1,
                name: Some("mesh".into()),
                op: CsgOp::ImportedMesh {
                    positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
                    indices: vec![0, 1, 2],
                    normals: None,
                    source: Some("model.stl".to_string()),
                },
            },
        );
        doc.roots.push(SceneEntry {
            root: 1,
            material: "default".into(),
            visible: None,
        });
        let (_, unsupported) = document_to_loon_checked(&doc);
        assert!(unsupported.contains(&"ImportedMesh".to_string()));
    }

    #[test]
    fn checked_deduplicates_same_variant() {
        let mut doc = Document::new();
        doc.nodes.insert(
            1,
            Node {
                id: 1,
                name: Some("m1".into()),
                op: CsgOp::ImportedMesh {
                    positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
                    indices: vec![0, 1, 2],
                    normals: None,
                    source: None,
                },
            },
        );
        doc.nodes.insert(
            2,
            Node {
                id: 2,
                name: Some("m2".into()),
                op: CsgOp::ImportedMesh {
                    positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
                    indices: vec![0, 1, 2],
                    normals: None,
                    source: None,
                },
            },
        );
        doc.roots.push(SceneEntry {
            root: 1,
            material: "default".into(),
            visible: None,
        });
        let (_, unsupported) = document_to_loon_checked(&doc);
        assert_eq!(
            unsupported
                .iter()
                .filter(|s| s.as_str() == "ImportedMesh")
                .count(),
            1,
            "duplicate variants should be deduplicated"
        );
    }
}
