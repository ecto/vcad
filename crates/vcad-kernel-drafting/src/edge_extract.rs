//! Sharp edge and silhouette edge detection from triangle meshes.
//!
//! Extracts edges that should be shown in technical drawings:
//! - Sharp edges: where adjacent faces have significantly different normals
//! - Silhouette edges: boundary between front-facing and back-facing faces
//! - Boundary edges: mesh boundaries (edges with only one adjacent face)

use std::collections::HashMap;

use vcad_kernel_math::Point3;
use vcad_kernel_tessellate::TriangleMesh;

use crate::types::{EdgeType, MeshEdge, Triangle3D, ViewDirection};

/// Default threshold angle (in radians) for sharp edge detection.
/// Edges where adjacent face normals differ by more than this are considered sharp.
pub const DEFAULT_SHARP_ANGLE: f64 = std::f64::consts::FRAC_PI_6; // 30 degrees

/// Edge key for hash map lookup (canonical vertex pair ordering).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct EdgeKey {
    v0: u32,
    v1: u32,
}

impl EdgeKey {
    fn new(a: u32, b: u32) -> Self {
        if a < b {
            Self { v0: a, v1: b }
        } else {
            Self { v0: b, v1: a }
        }
    }
}

/// Temporary edge data during extraction.
struct EdgeData {
    tri0: u32,
    tri1: Option<u32>,
}

/// Extract all mesh edges with adjacency information.
///
/// Returns edges with their adjacent triangles, which can be used to
/// classify edges as sharp, silhouette, or boundary.
pub fn extract_edges(mesh: &TriangleMesh) -> Vec<MeshEdge> {
    extract_edges_with_threshold(mesh, DEFAULT_SHARP_ANGLE)
}

/// Extract mesh edges with a custom sharp angle threshold.
///
/// # Arguments
///
/// * `mesh` - Triangle mesh to extract edges from
/// * `sharp_threshold` - Angle threshold in radians for sharp edge detection
pub fn extract_edges_with_threshold(mesh: &TriangleMesh, sharp_threshold: f64) -> Vec<MeshEdge> {
    let triangles = build_triangles(mesh);
    let mut edge_map: HashMap<EdgeKey, EdgeData> = HashMap::new();

    // Build edge adjacency
    for (tri_idx, tri_indices) in mesh.indices.chunks(3).enumerate() {
        let tri_idx = tri_idx as u32;
        let edges = [
            EdgeKey::new(tri_indices[0], tri_indices[1]),
            EdgeKey::new(tri_indices[1], tri_indices[2]),
            EdgeKey::new(tri_indices[2], tri_indices[0]),
        ];

        for key in edges {
            edge_map
                .entry(key)
                .and_modify(|e| {
                    if e.tri1.is_none() {
                        e.tri1 = Some(tri_idx);
                    }
                })
                .or_insert(EdgeData {
                    tri0: tri_idx,
                    tri1: None,
                });
        }
    }

    // Classify edges
    let mut result = Vec::with_capacity(edge_map.len());

    for (key, data) in edge_map {
        let edge_type = classify_edge(&triangles, data.tri0, data.tri1, sharp_threshold);
        result.push(MeshEdge::new(
            key.v0, key.v1, data.tri0, data.tri1, edge_type,
        ));
    }

    result
}

/// Classify an edge based on adjacent face normals.
fn classify_edge(
    triangles: &[Triangle3D],
    tri0: u32,
    tri1: Option<u32>,
    sharp_threshold: f64,
) -> EdgeType {
    match tri1 {
        None => EdgeType::Boundary,
        Some(tri1_idx) => {
            let n0 = triangles[tri0 as usize].normal;
            let n1 = triangles[tri1_idx as usize].normal;

            // Compute angle between normals
            let dot = n0.dot(n1).clamp(-1.0, 1.0);
            let angle = dot.acos();

            if angle > sharp_threshold {
                EdgeType::Sharp
            } else {
                // Will be marked as silhouette later if applicable
                EdgeType::Sharp // Default to sharp, will be filtered
            }
        }
    }
}

/// Extract only sharp edges (angle > threshold) from the mesh.
pub fn extract_sharp_edges(mesh: &TriangleMesh, sharp_threshold: f64) -> Vec<MeshEdge> {
    let triangles = build_triangles(mesh);
    let mut edge_map: HashMap<EdgeKey, EdgeData> = HashMap::new();

    // Build edge adjacency
    for (tri_idx, tri_indices) in mesh.indices.chunks(3).enumerate() {
        let tri_idx = tri_idx as u32;
        let edges = [
            EdgeKey::new(tri_indices[0], tri_indices[1]),
            EdgeKey::new(tri_indices[1], tri_indices[2]),
            EdgeKey::new(tri_indices[2], tri_indices[0]),
        ];

        for key in edges {
            edge_map
                .entry(key)
                .and_modify(|e| {
                    if e.tri1.is_none() {
                        e.tri1 = Some(tri_idx);
                    }
                })
                .or_insert(EdgeData {
                    tri0: tri_idx,
                    tri1: None,
                });
        }
    }

    // Filter to sharp and boundary edges only
    let mut result = Vec::new();

    for (key, data) in edge_map {
        let is_sharp = match data.tri1 {
            None => true, // Boundary edges are always included
            Some(tri1_idx) => {
                let n0 = triangles[data.tri0 as usize].normal;
                let n1 = triangles[tri1_idx as usize].normal;
                let dot = n0.dot(n1).clamp(-1.0, 1.0);
                let angle = dot.acos();
                angle > sharp_threshold
            }
        };

        if is_sharp {
            let edge_type = if data.tri1.is_none() {
                EdgeType::Boundary
            } else {
                EdgeType::Sharp
            };
            result.push(MeshEdge::new(
                key.v0, key.v1, data.tri0, data.tri1, edge_type,
            ));
        }
    }

    result
}

/// Extract silhouette edges for a given view direction.
///
/// A silhouette edge is one where one adjacent face is front-facing
/// and the other is back-facing relative to the view direction.
pub fn extract_silhouette_edges(mesh: &TriangleMesh, view_dir: ViewDirection) -> Vec<MeshEdge> {
    let view_vec = view_dir.view_vector();
    let triangles = build_triangles(mesh);
    let mut edge_map: HashMap<EdgeKey, EdgeData> = HashMap::new();

    // Build edge adjacency
    for (tri_idx, tri_indices) in mesh.indices.chunks(3).enumerate() {
        let tri_idx = tri_idx as u32;
        let edges = [
            EdgeKey::new(tri_indices[0], tri_indices[1]),
            EdgeKey::new(tri_indices[1], tri_indices[2]),
            EdgeKey::new(tri_indices[2], tri_indices[0]),
        ];

        for key in edges {
            edge_map
                .entry(key)
                .and_modify(|e| {
                    if e.tri1.is_none() {
                        e.tri1 = Some(tri_idx);
                    }
                })
                .or_insert(EdgeData {
                    tri0: tri_idx,
                    tri1: None,
                });
        }
    }

    // Find silhouette edges
    let mut result = Vec::new();

    for (key, data) in edge_map {
        let is_silhouette = match data.tri1 {
            None => true, // Boundary edges are silhouettes from any view
            Some(tri1_idx) => {
                let t0 = &triangles[data.tri0 as usize];
                let t1 = &triangles[tri1_idx as usize];
                let front0 = t0.is_front_facing(&view_vec);
                let front1 = t1.is_front_facing(&view_vec);
                front0 != front1 // One front-facing, one back-facing
            }
        };

        if is_silhouette {
            result.push(MeshEdge::new(
                key.v0,
                key.v1,
                data.tri0,
                data.tri1,
                EdgeType::Silhouette,
            ));
        }
    }

    result
}

/// Extract both sharp and silhouette edges for technical drawing.
///
/// This is the primary function for generating edges for a drafting view.
/// It returns all edges that should appear in the drawing:
/// - Sharp edges (geometric features)
/// - Silhouette edges (outline of the shape from this view)
/// - Boundary edges (mesh boundaries)
pub fn extract_drawing_edges(
    mesh: &TriangleMesh,
    view_dir: ViewDirection,
    sharp_threshold: f64,
) -> Vec<MeshEdge> {
    let view_vec = view_dir.view_vector();
    let triangles = build_triangles(mesh);
    let mut edge_map: HashMap<EdgeKey, EdgeData> = HashMap::new();

    // Build edge adjacency
    for (tri_idx, tri_indices) in mesh.indices.chunks(3).enumerate() {
        let tri_idx = tri_idx as u32;
        let edges = [
            EdgeKey::new(tri_indices[0], tri_indices[1]),
            EdgeKey::new(tri_indices[1], tri_indices[2]),
            EdgeKey::new(tri_indices[2], tri_indices[0]),
        ];

        for key in edges {
            edge_map
                .entry(key)
                .and_modify(|e| {
                    if e.tri1.is_none() {
                        e.tri1 = Some(tri_idx);
                    }
                })
                .or_insert(EdgeData {
                    tri0: tri_idx,
                    tri1: None,
                });
        }
    }

    // Extract relevant edges
    let mut result = Vec::new();

    for (key, data) in edge_map {
        let (include, edge_type) = match data.tri1 {
            None => (true, EdgeType::Boundary),
            Some(tri1_idx) => {
                let t0 = &triangles[data.tri0 as usize];
                let t1 = &triangles[tri1_idx as usize];

                // Check if silhouette
                let front0 = t0.is_front_facing(&view_vec);
                let front1 = t1.is_front_facing(&view_vec);
                let is_silhouette = front0 != front1;

                // Check if sharp
                let dot = t0.normal.dot(t1.normal).clamp(-1.0, 1.0);
                let angle = dot.acos();
                let is_sharp = angle > sharp_threshold;

                if is_silhouette {
                    (true, EdgeType::Silhouette)
                } else if is_sharp {
                    (true, EdgeType::Sharp)
                } else {
                    (false, EdgeType::Sharp)
                }
            }
        };

        if include {
            result.push(MeshEdge::new(
                key.v0, key.v1, data.tri0, data.tri1, edge_type,
            ));
        }
    }

    result
}

/// Build Triangle3D structures from mesh data.
pub fn build_triangles(mesh: &TriangleMesh) -> Vec<Triangle3D> {
    let verts = &mesh.vertices;
    mesh.indices
        .chunks(3)
        .map(|tri| {
            let i0 = tri[0] as usize * 3;
            let i1 = tri[1] as usize * 3;
            let i2 = tri[2] as usize * 3;

            Triangle3D::new(
                Point3::new(verts[i0] as f64, verts[i0 + 1] as f64, verts[i0 + 2] as f64),
                Point3::new(verts[i1] as f64, verts[i1 + 1] as f64, verts[i1 + 2] as f64),
                Point3::new(verts[i2] as f64, verts[i2 + 1] as f64, verts[i2 + 2] as f64),
            )
        })
        .collect()
}

/// Get vertex position from mesh.
pub fn get_vertex(mesh: &TriangleMesh, idx: u32) -> Point3 {
    let i = idx as usize * 3;
    Point3::new(
        mesh.vertices[i] as f64,
        mesh.vertices[i + 1] as f64,
        mesh.vertices[i + 2] as f64,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a simple cube mesh for testing.
    fn make_cube_mesh() -> TriangleMesh {
        // 8 vertices of a unit cube
        #[rustfmt::skip]
        let vertices: Vec<f32> = vec![
            0.0, 0.0, 0.0,  // 0
            1.0, 0.0, 0.0,  // 1
            1.0, 1.0, 0.0,  // 2
            0.0, 1.0, 0.0,  // 3
            0.0, 0.0, 1.0,  // 4
            1.0, 0.0, 1.0,  // 5
            1.0, 1.0, 1.0,  // 6
            0.0, 1.0, 1.0,  // 7
        ];

        // 12 triangles (2 per face)
        #[rustfmt::skip]
        let indices: Vec<u32> = vec![
            // Bottom (-Z)
            0, 2, 1, 0, 3, 2,
            // Top (+Z)
            4, 5, 6, 4, 6, 7,
            // Front (-Y)
            0, 1, 5, 0, 5, 4,
            // Back (+Y)
            2, 3, 7, 2, 7, 6,
            // Left (-X)
            0, 4, 7, 0, 7, 3,
            // Right (+X)
            1, 2, 6, 1, 6, 5,
        ];

        TriangleMesh {
            vertices,
            indices,
            normals: Vec::new(),
            face_kinds: Vec::new(),
            face_ids: Vec::new(),
        }
    }

    #[test]
    fn test_extract_edges_cube() {
        let mesh = make_cube_mesh();
        let edges = extract_edges(&mesh);

        // A cube has 12 edges, but with 2 triangles per face (12 total triangles),
        // each edge is shared by 2 triangles. The mesh we create has 18 unique edges
        // because the diagonal edges within each face are also counted.
        // 6 faces × 3 edges per triangle × 2 triangles = 36 edges
        // After deduplication by EdgeKey: 18 unique edges (12 cube edges + 6 diagonals)
        assert!(
            edges.len() >= 12,
            "Should have at least 12 cube edges, got {}",
            edges.len()
        );

        // Most edges should have 2 adjacent triangles
        let manifold_edges = edges.iter().filter(|e| e.tri1.is_some()).count();
        assert!(
            manifold_edges >= 12,
            "Should have at least 12 manifold edges"
        );
    }

    #[test]
    fn test_extract_sharp_edges_cube() {
        let mesh = make_cube_mesh();
        let edges = extract_sharp_edges(&mesh, DEFAULT_SHARP_ANGLE);

        // All 12 cube edges are sharp (90 degree angles)
        assert_eq!(edges.len(), 12, "Cube should have 12 sharp edges");
    }

    #[test]
    fn test_extract_silhouette_edges_cube() {
        let mesh = make_cube_mesh();

        // Front view: should see 4 silhouette edges (the outline)
        let edges = extract_silhouette_edges(&mesh, ViewDirection::Front);
        assert!(
            edges.len() >= 4,
            "Front view should have at least 4 silhouette edges"
        );
    }

    #[test]
    fn test_extract_drawing_edges_cube() {
        let mesh = make_cube_mesh();
        let edges = extract_drawing_edges(&mesh, ViewDirection::Front, DEFAULT_SHARP_ANGLE);

        // Should get all 12 edges (all are sharp in a cube)
        assert_eq!(edges.len(), 12);
    }

    #[test]
    fn test_triangle_normal() {
        let mesh = make_cube_mesh();
        let triangles = build_triangles(&mesh);

        // First triangle is on bottom face, normal should point -Z
        let bottom_tri = &triangles[0];
        assert!(
            bottom_tri.normal.z < -0.9,
            "Bottom face normal should point -Z"
        );
    }
}
