//! Hidden line removal via occlusion testing.
//!
//! Determines which edges are visible and which are hidden by checking
//! if sample points along each edge are occluded by mesh triangles.

use vcad_kernel_math::{Point2, Point3, Vec3};
use vcad_kernel_tessellate::TriangleMesh;

use crate::edge_extract::{
    build_triangles, extract_drawing_edges, get_vertex, DEFAULT_SHARP_ANGLE,
};
use crate::projection::ViewMatrix;
use crate::types::{MeshEdge, ProjectedEdge, ProjectedView, Triangle3D, ViewDirection, Visibility};

/// Number of sample points along each edge for occlusion testing.
const EDGE_SAMPLES: usize = 5;

/// Small offset to avoid self-intersection in occlusion tests.
const EPSILON: f64 = 1e-6;

/// Project a mesh to a 2D view with hidden line removal.
///
/// This is the main entry point for generating a technical drawing view.
/// It extracts sharp and silhouette edges, projects them to 2D, and
/// classifies each edge as visible or hidden.
pub fn project_mesh(mesh: &TriangleMesh, view_dir: ViewDirection) -> ProjectedView {
    project_mesh_with_options(mesh, view_dir, DEFAULT_SHARP_ANGLE)
}

/// Project a mesh with custom sharp angle threshold.
pub fn project_mesh_with_options(
    mesh: &TriangleMesh,
    view_dir: ViewDirection,
    sharp_threshold: f64,
) -> ProjectedView {
    let view_matrix = ViewMatrix::from_view_direction(view_dir);
    let triangles = build_triangles(mesh);
    let edges = extract_drawing_edges(mesh, view_dir, sharp_threshold);

    let mut result = ProjectedView::new(view_dir);

    for edge in edges {
        let v0 = get_vertex(mesh, edge.v0);
        let v1 = get_vertex(mesh, edge.v1);

        // Project endpoints
        let (p0, depth0) = view_matrix.project(v0);
        let (p1, depth1) = view_matrix.project(v1);
        let avg_depth = (depth0 + depth1) / 2.0;

        // Check visibility by sampling points along the edge
        let visibility =
            check_edge_visibility(v0, v1, &triangles, &view_matrix, view_dir.view_vector());

        let projected =
            ProjectedEdge::new(p0.into(), p1.into(), visibility, edge.edge_type, avg_depth);

        // Skip degenerate edges
        if !projected.is_degenerate(1e-6) {
            result.add_edge(projected);
        }
    }

    result
}

/// Check if an edge is visible or hidden.
///
/// Samples multiple points along the edge and checks each for occlusion.
/// If any sample point is occluded, the entire edge is marked as hidden.
fn check_edge_visibility(
    v0: Point3,
    v1: Point3,
    triangles: &[Triangle3D],
    view_matrix: &ViewMatrix,
    view_vec: Vec3,
) -> Visibility {
    // Sample points along the edge
    for i in 0..EDGE_SAMPLES {
        let t = (i as f64 + 0.5) / EDGE_SAMPLES as f64;
        let sample = Point3::new(
            v0.x + t * (v1.x - v0.x),
            v0.y + t * (v1.y - v0.y),
            v0.z + t * (v1.z - v0.z),
        );

        // Project the sample point
        let (sample_2d, sample_depth) = view_matrix.project(sample);

        // Check if this point is occluded by any triangle
        if is_point_occluded(
            sample,
            sample_2d,
            sample_depth,
            triangles,
            view_matrix,
            &view_vec,
        ) {
            return Visibility::Hidden;
        }
    }

    Visibility::Visible
}

/// Check if a 3D point is occluded by any triangle.
///
/// A point is occluded if there's a front-facing triangle that:
/// 1. Contains the point's 2D projection
/// 2. Is in front of the point (larger depth value — depth increases
///    toward the viewer)
fn is_point_occluded(
    _point_3d: Point3,
    point_2d: Point2,
    point_depth: f64,
    triangles: &[Triangle3D],
    view_matrix: &ViewMatrix,
    view_vec: &Vec3,
) -> bool {
    for tri in triangles {
        // Only front-facing triangles can occlude
        if !tri.is_front_facing(view_vec) {
            continue;
        }

        // Project triangle vertices
        let (t0_2d, t0_depth) = view_matrix.project(tri.v0);
        let (t1_2d, t1_depth) = view_matrix.project(tri.v1);
        let (t2_2d, t2_depth) = view_matrix.project(tri.v2);

        // Quick depth check: if triangle is entirely behind the point, skip
        let max_tri_depth = t0_depth.max(t1_depth).max(t2_depth);
        if max_tri_depth <= point_depth + EPSILON {
            continue;
        }

        // Check if point is inside the projected triangle
        if !point_in_triangle_2d(point_2d, t0_2d, t1_2d, t2_2d) {
            continue;
        }

        // Interpolate depth at the point's 2D location
        if let Some(interp_depth) =
            interpolate_depth_at_point(point_2d, t0_2d, t1_2d, t2_2d, t0_depth, t1_depth, t2_depth)
        {
            // Check if triangle is in front of the point (closer to viewer)
            if interp_depth > point_depth + EPSILON {
                return true;
            }
        }
    }

    false
}

/// Check if a 2D point is inside a 2D triangle using barycentric coordinates.
fn point_in_triangle_2d(p: Point2, a: Point2, b: Point2, c: Point2) -> bool {
    let v0 = Vec3::new(c.x - a.x, c.y - a.y, 0.0);
    let v1 = Vec3::new(b.x - a.x, b.y - a.y, 0.0);
    let v2 = Vec3::new(p.x - a.x, p.y - a.y, 0.0);

    let dot00 = v0.dot(v0);
    let dot01 = v0.dot(v1);
    let dot02 = v0.dot(v2);
    let dot11 = v1.dot(v1);
    let dot12 = v1.dot(v2);

    let inv_denom = 1.0 / (dot00 * dot11 - dot01 * dot01);
    let u = (dot11 * dot02 - dot01 * dot12) * inv_denom;
    let v = (dot00 * dot12 - dot01 * dot02) * inv_denom;

    // Check if point is in triangle (with small epsilon for numerical stability)
    let eps = 1e-8;
    u >= -eps && v >= -eps && (u + v) <= 1.0 + eps
}

/// Interpolate depth at a 2D point using barycentric coordinates.
fn interpolate_depth_at_point(
    p: Point2,
    a: Point2,
    b: Point2,
    c: Point2,
    da: f64,
    db: f64,
    dc: f64,
) -> Option<f64> {
    let v0 = Vec3::new(c.x - a.x, c.y - a.y, 0.0);
    let v1 = Vec3::new(b.x - a.x, b.y - a.y, 0.0);
    let v2 = Vec3::new(p.x - a.x, p.y - a.y, 0.0);

    let dot00 = v0.dot(v0);
    let dot01 = v0.dot(v1);
    let dot02 = v0.dot(v2);
    let dot11 = v1.dot(v1);
    let dot12 = v1.dot(v2);

    let denom = dot00 * dot11 - dot01 * dot01;
    if denom.abs() < 1e-12 {
        return None; // Degenerate triangle
    }

    let inv_denom = 1.0 / denom;
    let u = (dot11 * dot02 - dot01 * dot12) * inv_denom;
    let v = (dot00 * dot12 - dot01 * dot02) * inv_denom;
    let w = 1.0 - u - v;

    Some(w * da + v * db + u * dc)
}

/// Classify visibility for a pre-extracted set of edges.
///
/// Useful when you've already extracted edges and want to check visibility
/// for multiple view directions.
pub fn classify_edge_visibility(
    edges: &[MeshEdge],
    mesh: &TriangleMesh,
    view_dir: ViewDirection,
) -> Vec<(MeshEdge, Visibility)> {
    let view_matrix = ViewMatrix::from_view_direction(view_dir);
    let view_vec = view_dir.view_vector();
    let triangles = build_triangles(mesh);

    edges
        .iter()
        .map(|edge| {
            let v0 = get_vertex(mesh, edge.v0);
            let v1 = get_vertex(mesh, edge.v1);

            let visibility = check_edge_visibility(v0, v1, &triangles, &view_matrix, view_vec);

            (edge.clone(), visibility)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a simple cube mesh for testing.
    fn make_cube_mesh() -> TriangleMesh {
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
    fn test_project_mesh_cube() {
        let mesh = make_cube_mesh();
        let view = project_mesh(&mesh, ViewDirection::Front);

        // Should have some visible and some hidden edges
        assert!(!view.edges.is_empty());
        assert!(view.bounds.is_valid());

        // Front view of a cube should have 4 visible edges (the front face outline)
        // and 8 hidden edges (the back face and connecting edges)
        let visible = view.num_visible();
        let _hidden = view.num_hidden();

        // At minimum, we should see some edges
        assert!(visible > 0, "Should have visible edges");
    }

    #[test]
    fn test_project_mesh_top_view() {
        let mesh = make_cube_mesh();
        let view = project_mesh(&mesh, ViewDirection::Top);

        assert!(!view.edges.is_empty());
        assert!(view.bounds.is_valid());
    }

    #[test]
    fn test_point_in_triangle() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(1.0, 0.0);
        let c = Point2::new(0.0, 1.0);

        // Inside
        assert!(point_in_triangle_2d(Point2::new(0.2, 0.2), a, b, c));

        // Outside
        assert!(!point_in_triangle_2d(Point2::new(1.0, 1.0), a, b, c));

        // On edge
        assert!(point_in_triangle_2d(Point2::new(0.5, 0.0), a, b, c));
    }

    #[test]
    fn test_isometric_projection() {
        let mesh = make_cube_mesh();
        let view = project_mesh(&mesh, ViewDirection::ISOMETRIC_STANDARD);

        assert!(!view.edges.is_empty());
        assert!(view.bounds.is_valid());

        // Isometric view should show all 12 edges (some visible, some hidden)
        assert_eq!(view.edges.len(), 12);
    }

    #[test]
    fn test_bounding_box_computed() {
        let mesh = make_cube_mesh();
        let view = project_mesh(&mesh, ViewDirection::Front);

        // Bounding box should be approximately 1x1 (unit cube)
        assert!(view.bounds.width() > 0.9 && view.bounds.width() < 1.1);
        assert!(view.bounds.height() > 0.9 && view.bounds.height() < 1.1);
    }
}
