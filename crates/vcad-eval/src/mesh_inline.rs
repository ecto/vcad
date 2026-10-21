//! Inline `MeshImport` nodes from STL bytes the host supplies.
//!
//! A URDF (or a loon `[import-mesh]`) references meshes by path. Native hosts
//! read them at evaluation time; hosts that can't hand the evaluator a
//! filesystem (the browser, the MCP server, which jails paths itself) read the
//! bytes and rewrite each node to an `ImportedMesh` here, with the same metres
//! → millimetres conversion and URDF `<mesh scale>` the native loader applies.

use std::collections::HashMap;

use vcad_ir::{CsgOp, Document, Vec3};

/// Parse STL bytes (binary or ASCII) into `(positions_mm, indices)`, scaling
/// metres to millimetres times the optional URDF per-axis `scale`. Faces that
/// reference vertices past the vertex table are dropped.
pub fn stl_to_mesh(bytes: &[u8], scale: Option<&Vec3>) -> Result<(Vec<f64>, Vec<u32>), String> {
    let mut cursor = std::io::Cursor::new(bytes);
    let stl = stl_io::read_stl(&mut cursor).map_err(|e| format!("STL parse error: {e}"))?;
    let s = [
        scale.map_or(1.0, |s| s.x) * 1000.0,
        scale.map_or(1.0, |s| s.y) * 1000.0,
        scale.map_or(1.0, |s| s.z) * 1000.0,
    ];
    let positions = stl
        .vertices
        .iter()
        .flat_map(|v| [v[0] as f64 * s[0], v[1] as f64 * s[1], v[2] as f64 * s[2]])
        .collect();
    let n = stl.vertices.len();
    let indices = stl
        .faces
        .iter()
        .filter(|f| f.vertices.iter().all(|&i| i < n))
        .flat_map(|f| f.vertices.map(|i| i as u32))
        .collect();
    Ok((positions, indices))
}

/// Outcome of [`inline_mesh_imports`].
#[derive(Debug, Default, serde::Serialize)]
pub struct InlineReport {
    /// Nodes rewritten to `ImportedMesh`.
    pub inlined: usize,
    /// `(node key, reason)` for each mesh that could not be inlined.
    pub errors: Vec<(String, String)>,
}

/// Rewrite each `MeshImport` node named in `meshes` (node key → STL bytes)
/// to an `ImportedMesh`. Keys that are not `MeshImport` nodes, or whose bytes
/// don't parse, are reported in `errors` and left unchanged.
pub fn inline_mesh_imports(doc: &mut Document, meshes: &HashMap<String, Vec<u8>>) -> InlineReport {
    let mut report = InlineReport::default();
    let mut keys: Vec<&String> = meshes.keys().collect();
    keys.sort();
    for key in keys {
        let node = key.parse().ok().and_then(|id| doc.nodes.get_mut(&id));
        let Some(node) = node else {
            report.errors.push((key.clone(), "no such node".into()));
            continue;
        };
        let CsgOp::MeshImport { path, scale } = &node.op else {
            report
                .errors
                .push((key.clone(), "not a MeshImport node".into()));
            continue;
        };
        match stl_to_mesh(&meshes[key], scale.as_ref()) {
            Ok((positions, indices)) => {
                node.op = CsgOp::ImportedMesh {
                    positions,
                    indices,
                    normals: None,
                    source: Some(path.clone()),
                };
                report.inlined += 1;
            }
            Err(e) => report.errors.push((key.clone(), e)),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::Node;

    const ASCII: &str = "solid m\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid m\n";

    fn binary_one_tri() -> Vec<u8> {
        let mut b = vec![0u8; 80];
        b.extend(1u32.to_le_bytes());
        for f in [
            0.0f32, 0.0, 1.0, 0.0, 0.0, 0.0, 0.002, 0.0, 0.0, 0.0, 0.003, 0.0,
        ] {
            b.extend(f.to_le_bytes());
        }
        b.extend([0u8, 0]);
        b
    }

    fn doc_with(op: CsgOp) -> Document {
        let mut doc = Document::default();
        doc.nodes.insert(
            4,
            Node {
                id: 4,
                name: Some("link".into()),
                op,
            },
        );
        doc
    }

    #[test]
    fn ascii_and_binary_scale_metres_to_mm() {
        let (p, i) = stl_to_mesh(ASCII.as_bytes(), None).unwrap();
        assert_eq!(i, [0, 1, 2]);
        assert!(p.contains(&1000.0));
        let (p, i) = stl_to_mesh(&binary_one_tri(), Some(&Vec3::new(2.0, 1.0, 1.0))).unwrap();
        assert_eq!(i.len(), 3);
        let max_x = p.iter().step_by(3).cloned().fold(f64::MIN, f64::max);
        let max_y = p
            .iter()
            .skip(1)
            .step_by(3)
            .cloned()
            .fold(f64::MIN, f64::max);
        assert!((max_x - 4.0).abs() < 1e-6, "{p:?}"); // 0.002 m × 2 × 1000
        assert!((max_y - 3.0).abs() < 1e-6, "{p:?}");
        assert!(stl_to_mesh(b"not an stl", None).is_err());
    }

    #[test]
    fn inlines_mesh_import_nodes_and_reports_failures() {
        let mut doc = doc_with(CsgOp::MeshImport {
            path: "package://bot/m.stl".into(),
            scale: None,
        });
        doc.nodes.insert(
            5,
            Node {
                id: 5,
                name: None,
                op: CsgOp::Empty,
            },
        );
        let meshes: HashMap<String, Vec<u8>> = [
            ("4".to_string(), ASCII.as_bytes().to_vec()),
            ("5".to_string(), ASCII.as_bytes().to_vec()),
            ("9".to_string(), Vec::new()),
        ]
        .into();
        let r = inline_mesh_imports(&mut doc, &meshes);
        assert_eq!(r.inlined, 1);
        assert_eq!(r.errors.len(), 2);
        match &doc.nodes[&4].op {
            CsgOp::ImportedMesh {
                indices, source, ..
            } => {
                assert_eq!(indices.len(), 3);
                assert_eq!(source.as_deref(), Some("package://bot/m.stl"));
            }
            other => panic!("expected ImportedMesh, got {other:?}"),
        }
        assert_eq!(doc.nodes[&4].name.as_deref(), Some("link"));
        // A bad STL leaves the node a MeshImport.
        let mut doc = doc_with(CsgOp::MeshImport {
            path: "m.stl".into(),
            scale: None,
        });
        let r = inline_mesh_imports(&mut doc, &[("4".to_string(), b"junk".to_vec())].into());
        assert_eq!(r.inlined, 0);
        assert!(matches!(doc.nodes[&4].op, CsgOp::MeshImport { .. }));
    }
}
