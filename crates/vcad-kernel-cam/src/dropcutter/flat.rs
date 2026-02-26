//! Drop-cutter algorithm for flat end mills.
//!
//! A flat end mill has three contact regions:
//! 1. Bottom face - direct contact with triangle surfaces
//! 2. Corner (bottom edge) - contact with triangle edges
//! 3. Corner - contact with triangle vertices

use super::mesh_accel::MeshAccel;

/// Compute drop-cutter height for a flat end mill.
///
/// # Arguments
///
/// * `accel` - Mesh acceleration structure
/// * `radius` - Tool radius
/// * `x` - X coordinate of tool center
/// * `y` - Y coordinate of tool center
///
/// # Returns
///
/// The minimum Z height at which the tool can be positioned without collision.
pub fn drop_cutter_flat(accel: &MeshAccel, radius: f64, x: f64, y: f64) -> f64 {
    let candidates = accel.query_circle(x, y, radius);
    let mut max_z = f64::NEG_INFINITY;

    for &tri_idx in &candidates {
        let tri = accel.triangle(tri_idx);

        // Test 1: Bottom face contact
        // If (x, y) projects inside the triangle, the tool bottom touches the surface
        if tri.contains_xy(x, y) {
            if let Some(z) = tri.z_at_xy(x, y) {
                max_z = max_z.max(z);
            }
        }

        // Test 2: Edge contact
        // The tool corner can contact triangle edges
        for [v0, v1] in tri.edges() {
            let z = edge_contact_flat(x, y, radius, v0, v1);
            max_z = max_z.max(z);
        }

        // Test 3: Vertex contact
        // The tool corner can contact triangle vertices
        for v in &tri.v {
            let z = vertex_contact_flat(x, y, radius, *v);
            max_z = max_z.max(z);
        }
    }

    max_z
}

/// Compute Z height for edge contact with flat end mill.
fn edge_contact_flat(x: f64, y: f64, radius: f64, v0: [f64; 3], v1: [f64; 3]) -> f64 {
    // Project edge onto XY plane and find closest point to tool center
    let dx = v1[0] - v0[0];
    let dy = v1[1] - v0[1];
    let len_sq = dx * dx + dy * dy;

    if len_sq < 1e-10 {
        // Degenerate edge (zero length), treat as vertex
        return vertex_contact_flat(x, y, radius, v0);
    }

    // Parameter along edge (0 = v0, 1 = v1)
    let t = ((x - v0[0]) * dx + (y - v0[1]) * dy) / len_sq;
    let t = t.clamp(0.0, 1.0);

    // Closest point on edge
    let px = v0[0] + t * dx;
    let py = v0[1] + t * dy;
    let pz = v0[2] + t * (v1[2] - v0[2]);

    // Distance from tool center to closest point
    let dist = ((x - px) * (x - px) + (y - py) * (y - py)).sqrt();

    // If the edge is within tool radius, the tool corner touches it
    if dist <= radius {
        pz
    } else {
        f64::NEG_INFINITY
    }
}

/// Compute Z height for vertex contact with flat end mill.
fn vertex_contact_flat(x: f64, y: f64, radius: f64, v: [f64; 3]) -> f64 {
    let dist = ((x - v[0]) * (x - v[0]) + (y - v[1]) * (y - v[1])).sqrt();

    if dist <= radius {
        v[2]
    } else {
        f64::NEG_INFINITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_mesh() -> MeshAccel {
        // Sloped triangle from z=0 to z=10
        let vertices = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [5.0, 10.0, 10.0]];
        let indices = [0, 1, 2];
        MeshAccel::new(&vertices, &indices, 5.0)
    }

    #[test]
    fn test_flat_face_contact() {
        let accel = make_test_mesh();

        // At center of triangle base
        let z = drop_cutter_flat(&accel, 1.0, 5.0, 2.0);
        assert!(z > f64::NEG_INFINITY);
        assert!(z < 5.0); // Should be below halfway up
    }

    #[test]
    fn test_flat_vertex_contact() {
        let accel = make_test_mesh();

        // Just within radius of the apex vertex
        let z = drop_cutter_flat(&accel, 2.0, 5.0, 8.5);
        assert!(z > f64::NEG_INFINITY);
    }

    #[test]
    fn test_flat_no_contact() {
        let accel = make_test_mesh();

        // Far from the triangle
        let z = drop_cutter_flat(&accel, 1.0, 100.0, 100.0);
        assert!(z == f64::NEG_INFINITY);
    }
}
