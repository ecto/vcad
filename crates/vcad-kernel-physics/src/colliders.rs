//! Collision shape generation from vcad geometry.

use phyz::math::Vec3;
use phyz::Geometry;
use vcad_kernel_tessellate::TriangleMesh;

use crate::error::PhysicsError;

/// Strategy for generating collision shapes.
#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)]
pub enum ColliderStrategy {
    /// Use convex hull (fast, approximate) — mapped to Mesh.
    #[default]
    ConvexHull,
    /// Use triangle mesh (accurate, slower).
    TriMesh,
    /// Use axis-aligned bounding box (fastest, rough).
    Aabb,
}

/// Generate a collision geometry from a triangle mesh.
///
/// # Arguments
///
/// * `mesh` - The triangle mesh to convert
/// * `strategy` - The collision shape strategy to use
/// * `name` - Name for error messages
///
/// # Returns
///
/// A `phyz::Geometry` ready for use with the physics engine.
pub fn mesh_to_collider(
    mesh: &TriangleMesh,
    strategy: ColliderStrategy,
    name: &str,
) -> Result<Geometry, PhysicsError> {
    if mesh.vertices.is_empty() || mesh.indices.is_empty() {
        return Err(PhysicsError::CollisionShape {
            name: name.to_string(),
            reason: "Empty mesh".to_string(),
        });
    }

    match strategy {
        ColliderStrategy::ConvexHull | ColliderStrategy::TriMesh => create_mesh(mesh, name),
        ColliderStrategy::Aabb => create_aabb(mesh, name),
    }
}

fn create_mesh(mesh: &TriangleMesh, name: &str) -> Result<Geometry, PhysicsError> {
    // Extract vertices, converting from mm to meters
    let vertices: Vec<Vec3> = mesh
        .vertices
        .chunks(3)
        .map(|v| {
            Vec3::new(
                v[0] as f64 / 1000.0,
                v[1] as f64 / 1000.0,
                v[2] as f64 / 1000.0,
            )
        })
        .collect();

    if vertices.len() < 4 {
        return Err(PhysicsError::CollisionShape {
            name: name.to_string(),
            reason: "Need at least 4 vertices for mesh collider".to_string(),
        });
    }

    // Extract triangle faces
    let faces: Vec<[usize; 3]> = mesh
        .indices
        .chunks(3)
        .map(|i| [i[0] as usize, i[1] as usize, i[2] as usize])
        .collect();

    if faces.is_empty() {
        return Err(PhysicsError::CollisionShape {
            name: name.to_string(),
            reason: "No triangles in mesh".to_string(),
        });
    }

    Ok(Geometry::Mesh { vertices, faces })
}

fn create_aabb(mesh: &TriangleMesh, _name: &str) -> Result<Geometry, PhysicsError> {
    // Compute bounding box
    let mut min = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut max = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);

    for v in mesh.vertices.chunks(3) {
        // Convert from mm to meters
        let x = v[0] as f64 / 1000.0;
        let y = v[1] as f64 / 1000.0;
        let z = v[2] as f64 / 1000.0;

        min.x = min.x.min(x);
        min.y = min.y.min(y);
        min.z = min.z.min(z);
        max.x = max.x.max(x);
        max.y = max.y.max(y);
        max.z = max.z.max(z);
    }

    let half_extents = (max - min) / 2.0;

    Ok(Geometry::Box { half_extents })
}

/// Compute the center of mass from a triangle mesh.
///
/// Returns the center of mass in meters.
#[allow(dead_code)]
pub fn compute_center_of_mass(mesh: &TriangleMesh) -> Vec3 {
    if mesh.vertices.is_empty() {
        return Vec3::zeros();
    }

    let mut sum = Vec3::zeros();
    let count = mesh.vertices.len() / 3;

    for v in mesh.vertices.chunks(3) {
        // Convert from mm to meters
        sum.x += v[0] as f64 / 1000.0;
        sum.y += v[1] as f64 / 1000.0;
        sum.z += v[2] as f64 / 1000.0;
    }

    sum / count as f64
}

/// Estimate mass from mesh volume assuming uniform density.
///
/// # Arguments
///
/// * `mesh` - The triangle mesh
/// * `density` - Density in kg/m³ (default: 1000 for plastic-like material)
///
/// # Returns
///
/// Estimated mass in kg.
pub fn estimate_mass(mesh: &TriangleMesh, density: f64) -> f64 {
    // Use signed volume method
    let mut volume = 0.0f64;

    for tri in mesh.indices.chunks(3) {
        let i0 = tri[0] as usize * 3;
        let i1 = tri[1] as usize * 3;
        let i2 = tri[2] as usize * 3;

        // Convert from mm to meters
        let v0 = Vec3::new(
            mesh.vertices[i0] as f64 / 1000.0,
            mesh.vertices[i0 + 1] as f64 / 1000.0,
            mesh.vertices[i0 + 2] as f64 / 1000.0,
        );
        let v1 = Vec3::new(
            mesh.vertices[i1] as f64 / 1000.0,
            mesh.vertices[i1 + 1] as f64 / 1000.0,
            mesh.vertices[i1 + 2] as f64 / 1000.0,
        );
        let v2 = Vec3::new(
            mesh.vertices[i2] as f64 / 1000.0,
            mesh.vertices[i2 + 1] as f64 / 1000.0,
            mesh.vertices[i2 + 2] as f64 / 1000.0,
        );

        // Signed volume of tetrahedron with origin
        volume += v0.dot(v1.cross(v2)) / 6.0;
    }

    (volume.abs() * density).max(0.001) // Minimum mass of 1 gram
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simple_cube_mesh() -> TriangleMesh {
        // 10mm cube centered at origin
        let s = 5.0; // half-size in mm
        TriangleMesh {
            vertices: vec![
                // Front face
                -s, -s, s, s, -s, s, s, s, s, -s, s, s, // Back face
                -s, -s, -s, -s, s, -s, s, s, -s, s, -s, -s,
            ],
            indices: vec![
                // Front
                0, 1, 2, 0, 2, 3, // Back
                4, 5, 6, 4, 6, 7, // Top
                3, 2, 6, 3, 6, 5, // Bottom
                0, 7, 1, 0, 4, 7, // Right
                1, 7, 6, 1, 6, 2, // Left
                0, 3, 5, 0, 5, 4,
            ],
            normals: vec![],
            face_kinds: Vec::new(),
        }
    }

    #[test]
    fn test_mesh_collider() {
        let mesh = simple_cube_mesh();
        let geom = mesh_to_collider(&mesh, ColliderStrategy::ConvexHull, "test").unwrap();
        assert!(matches!(geom, Geometry::Mesh { .. }));
    }

    #[test]
    fn test_trimesh() {
        let mesh = simple_cube_mesh();
        let geom = mesh_to_collider(&mesh, ColliderStrategy::TriMesh, "test").unwrap();
        assert!(matches!(geom, Geometry::Mesh { .. }));
    }

    #[test]
    fn test_aabb() {
        let mesh = simple_cube_mesh();
        let geom = mesh_to_collider(&mesh, ColliderStrategy::Aabb, "test").unwrap();
        assert!(matches!(geom, Geometry::Box { .. }));
    }

    #[test]
    fn test_center_of_mass() {
        let mesh = simple_cube_mesh();
        let com = compute_center_of_mass(&mesh);
        // Should be near origin
        assert!(com.norm() < 0.001);
    }
}
