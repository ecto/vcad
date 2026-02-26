//! Drop-cutter algorithm for ball end mills.
//!
//! A ball end mill has a spherical tip. The contact regions are:
//! 1. Sphere-triangle face contact
//! 2. Sphere-edge contact
//! 3. Sphere-vertex contact

use super::mesh_accel::MeshAccel;

/// Compute drop-cutter height for a ball end mill.
///
/// # Arguments
///
/// * `accel` - Mesh acceleration structure
/// * `radius` - Tool radius (sphere radius)
/// * `x` - X coordinate of tool center
/// * `y` - Y coordinate of tool center
///
/// # Returns
///
/// The minimum Z height at which the tool center can be positioned without collision.
/// Note: This is the Z of the sphere center, not the bottom of the tool.
pub fn drop_cutter_ball(accel: &MeshAccel, radius: f64, x: f64, y: f64) -> f64 {
    let candidates = accel.query_circle(x, y, radius);
    let mut max_z = f64::NEG_INFINITY;

    for &tri_idx in &candidates {
        let tri = accel.triangle(tri_idx);

        // Test 1: Face contact
        // The sphere touches the triangle surface
        let z = face_contact_ball(x, y, radius, tri);
        max_z = max_z.max(z);

        // Test 2: Edge contact
        for [v0, v1] in tri.edges() {
            let z = edge_contact_ball(x, y, radius, v0, v1);
            max_z = max_z.max(z);
        }

        // Test 3: Vertex contact
        for v in &tri.v {
            let z = vertex_contact_ball(x, y, radius, *v);
            max_z = max_z.max(z);
        }
    }

    max_z
}

/// Compute sphere center Z for face contact.
fn face_contact_ball(x: f64, y: f64, radius: f64, tri: &super::mesh_accel::Triangle) -> f64 {
    // For a sphere touching a plane, the center is at: z_surface + r / cos(theta)
    // where theta is the angle between the surface normal and vertical
    let nz = tri.normal[2];

    // Skip nearly vertical or downward-facing triangles
    if nz <= 0.01 {
        return f64::NEG_INFINITY;
    }

    // Find where the sphere center projects onto the triangle
    // The contact point is offset from the sphere center by -r * normal
    let contact_x = x - radius * tri.normal[0] / nz;
    let contact_y = y - radius * tri.normal[1] / nz;

    // Check if contact point is inside triangle
    if !tri.contains_xy(contact_x, contact_y) {
        return f64::NEG_INFINITY;
    }

    // Z of contact point on triangle surface
    if let Some(z_surface) = tri.z_at_xy(contact_x, contact_y) {
        // Sphere center is radius above the contact point in the normal direction
        z_surface + radius / nz
    } else {
        f64::NEG_INFINITY
    }
}

/// Compute sphere center Z for edge contact.
fn edge_contact_ball(x: f64, y: f64, radius: f64, v0: [f64; 3], v1: [f64; 3]) -> f64 {
    // Find closest point on the 3D edge to the vertical line at (x, y)
    // This involves projecting onto the edge in XY, then computing 3D distance

    let dx = v1[0] - v0[0];
    let dy = v1[1] - v0[1];
    let dz = v1[2] - v0[2];
    let len_sq = dx * dx + dy * dy + dz * dz;

    if len_sq < 1e-10 {
        return vertex_contact_ball(x, y, radius, v0);
    }

    // For edge contact, we need to find where on the edge the sphere touches
    // The sphere center is at (x, y, z_center), and we need to find the point
    // on the edge that is exactly radius away

    // Project tool center onto edge in XY
    let t_xy = ((x - v0[0]) * dx + (y - v0[1]) * dy) / (dx * dx + dy * dy + 1e-10);
    let t_xy = t_xy.clamp(0.0, 1.0);

    // Point on edge at parameter t_xy
    let px = v0[0] + t_xy * dx;
    let py = v0[1] + t_xy * dy;
    let pz = v0[2] + t_xy * dz;

    // Horizontal distance from tool center to edge point
    let dist_xy = ((x - px) * (x - px) + (y - py) * (y - py)).sqrt();

    if dist_xy >= radius {
        return f64::NEG_INFINITY;
    }

    // For a sphere centered at (x, y, z_center) touching point (px, py, pz):
    // (px - x)^2 + (py - y)^2 + (pz - z_center)^2 = radius^2
    // Solving for z_center:
    // z_center = pz + sqrt(radius^2 - dist_xy^2)
    let dz_offset = (radius * radius - dist_xy * dist_xy).sqrt();
    pz + dz_offset
}

/// Compute sphere center Z for vertex contact.
fn vertex_contact_ball(x: f64, y: f64, radius: f64, v: [f64; 3]) -> f64 {
    let dist_xy = ((x - v[0]) * (x - v[0]) + (y - v[1]) * (y - v[1])).sqrt();

    if dist_xy >= radius {
        return f64::NEG_INFINITY;
    }

    // z_center such that distance from (x, y, z_center) to v equals radius
    let dz_offset = (radius * radius - dist_xy * dist_xy).sqrt();
    v[2] + dz_offset
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_flat_mesh() -> MeshAccel {
        // Flat triangle in XY plane at z=0
        let vertices = [[0.0, 0.0, 0.0], [20.0, 0.0, 0.0], [10.0, 20.0, 0.0]];
        let indices = [0, 1, 2];
        MeshAccel::new(&vertices, &indices, 10.0)
    }

    fn make_sloped_mesh() -> MeshAccel {
        // Triangle sloped 45 degrees
        let vertices = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [5.0, 10.0, 10.0]];
        let indices = [0, 1, 2];
        MeshAccel::new(&vertices, &indices, 5.0)
    }

    #[test]
    fn test_ball_flat_face_contact() {
        let accel = make_flat_mesh();

        // Ball centered at (10, 10) should touch the flat surface
        // The sphere center Z should be exactly the radius above the surface
        let z = drop_cutter_ball(&accel, 3.0, 10.0, 10.0);
        assert!((z - 3.0).abs() < 0.1, "Expected z ≈ 3.0, got {}", z);
    }

    #[test]
    fn test_ball_sloped_face_contact() {
        let accel = make_sloped_mesh();

        // On a sloped surface, the sphere center is higher than r
        let z = drop_cutter_ball(&accel, 2.0, 5.0, 3.0);
        assert!(z > 2.0, "On slope, z should be > radius");
    }

    #[test]
    fn test_ball_vertex_contact() {
        let accel = make_flat_mesh();

        // Ball near vertex at (0,0,0)
        let z = drop_cutter_ball(&accel, 2.0, 1.0, 1.0);
        // Distance from (1,1) to (0,0) is sqrt(2) ≈ 1.414
        // z = 0 + sqrt(4 - 2) = sqrt(2) ≈ 1.414
        // But on a flat surface, face contact dominates: z = radius = 2.0
        assert!(
            z >= 1.0 && z <= 3.5,
            "Expected z between 1.0 and 3.5, got {}",
            z
        );
    }

    #[test]
    fn test_ball_no_contact() {
        let accel = make_flat_mesh();

        // Far from the mesh
        let z = drop_cutter_ball(&accel, 2.0, 100.0, 100.0);
        assert!(z == f64::NEG_INFINITY);
    }
}
