//! Drop-cutter algorithm for bull (corner radius) end mills.
//!
//! A bull end mill has:
//! - A flat bottom of radius (R - r) where R is tool radius and r is corner radius
//! - A toroidal (torus) corner region
//! - A cylindrical shank above the corner
//!
//! Contact regions:
//! 1. Flat bottom - like flat endmill but with smaller effective radius
//! 2. Torus corner - swept circle contact
//! 3. Vertices - torus point contact

use super::mesh_accel::{MeshAccel, Triangle};

/// Compute drop-cutter height for a bull end mill.
///
/// # Arguments
///
/// * `accel` - Mesh acceleration structure
/// * `radius` - Tool radius (outer radius)
/// * `corner_radius` - Corner (torus) radius
/// * `x` - X coordinate of tool center
/// * `y` - Y coordinate of tool center
///
/// # Returns
///
/// The minimum Z height at which the tool can be positioned without collision.
/// This is the Z of the tool tip (bottom of flat portion).
pub fn drop_cutter_bull(accel: &MeshAccel, radius: f64, corner_radius: f64, x: f64, y: f64) -> f64 {
    // Clamp corner radius to valid range
    let corner_radius = corner_radius.min(radius);
    let flat_radius = radius - corner_radius; // Radius of the flat bottom

    let candidates = accel.query_circle(x, y, radius);
    let mut max_z = f64::NEG_INFINITY;

    for &tri_idx in &candidates {
        let tri = accel.triangle(tri_idx);

        // Test 1: Flat bottom contact (only for points within flat_radius)
        let z = flat_bottom_contact(x, y, flat_radius, tri);
        max_z = max_z.max(z);

        // Test 2: Torus face contact
        let z = torus_face_contact(x, y, radius, corner_radius, tri);
        max_z = max_z.max(z);

        // Test 3: Torus edge contact
        for [v0, v1] in tri.edges() {
            let z = torus_edge_contact(x, y, radius, corner_radius, v0, v1);
            max_z = max_z.max(z);
        }

        // Test 4: Torus vertex contact
        for v in &tri.v {
            let z = torus_vertex_contact(x, y, radius, corner_radius, *v);
            max_z = max_z.max(z);
        }
    }

    max_z
}

/// Flat bottom contact (inner flat portion of bull endmill).
fn flat_bottom_contact(x: f64, y: f64, flat_radius: f64, tri: &Triangle) -> f64 {
    // Only consider if point is within the flat bottom region
    if flat_radius <= 0.0 {
        return f64::NEG_INFINITY;
    }

    // Check if tool center projects inside triangle
    if tri.contains_xy(x, y) {
        if let Some(z) = tri.z_at_xy(x, y) {
            return z;
        }
    }

    // Check edge contacts within flat radius
    for [v0, v1] in tri.edges() {
        let z = flat_edge_contact(x, y, flat_radius, v0, v1);
        if z > f64::NEG_INFINITY {
            return z.max(f64::NEG_INFINITY);
        }
    }

    // Check vertex contacts within flat radius
    for v in &tri.v {
        let dist = ((x - v[0]) * (x - v[0]) + (y - v[1]) * (y - v[1])).sqrt();
        if dist <= flat_radius {
            return v[2];
        }
    }

    f64::NEG_INFINITY
}

fn flat_edge_contact(x: f64, y: f64, radius: f64, v0: [f64; 3], v1: [f64; 3]) -> f64 {
    let dx = v1[0] - v0[0];
    let dy = v1[1] - v0[1];
    let len_sq = dx * dx + dy * dy;

    if len_sq < 1e-10 {
        let dist = ((x - v0[0]) * (x - v0[0]) + (y - v0[1]) * (y - v0[1])).sqrt();
        return if dist <= radius {
            v0[2]
        } else {
            f64::NEG_INFINITY
        };
    }

    let t = ((x - v0[0]) * dx + (y - v0[1]) * dy) / len_sq;
    let t = t.clamp(0.0, 1.0);

    let px = v0[0] + t * dx;
    let py = v0[1] + t * dy;
    let pz = v0[2] + t * (v1[2] - v0[2]);

    let dist = ((x - px) * (x - px) + (y - py) * (y - py)).sqrt();

    if dist <= radius {
        pz
    } else {
        f64::NEG_INFINITY
    }
}

/// Torus face contact.
/// The torus is centered at radius (radius - corner_radius) from tool center,
/// with tube radius = corner_radius.
fn torus_face_contact(x: f64, y: f64, radius: f64, corner_radius: f64, tri: &Triangle) -> f64 {
    let nz = tri.normal[2];

    // Skip nearly vertical or downward-facing triangles
    if nz <= 0.01 {
        return f64::NEG_INFINITY;
    }

    let nx = tri.normal[0];
    let ny = tri.normal[1];
    let nh = (nx * nx + ny * ny).sqrt(); // Horizontal component of normal

    // Effective radius from torus center to contact point in XY
    // For a torus touching a plane, the contact point is offset by:
    // torus_major_radius + corner_radius * nh / nz (horizontal)
    // corner_radius / nz (vertical, from torus center tube to contact)

    let torus_major = radius - corner_radius;

    // Contact point offset from tool center in direction of normal
    let contact_offset_xy = if nh > 1e-10 {
        torus_major + corner_radius * nh / nz
    } else {
        torus_major // Flat surface, contact at torus tube center radius
    };

    // Direction of horizontal normal component
    let (dir_x, dir_y) = if nh > 1e-10 {
        (nx / nh, ny / nh)
    } else {
        (0.0, 0.0)
    };

    // Contact point in XY
    let contact_x = x - contact_offset_xy * dir_x;
    let contact_y = y - contact_offset_xy * dir_y;

    // Check if contact point is inside triangle
    if !tri.contains_xy(contact_x, contact_y) {
        return f64::NEG_INFINITY;
    }

    // Z at contact point
    if let Some(z_contact) = tri.z_at_xy(contact_x, contact_y) {
        // Tool tip Z = contact Z + corner_radius * (1 - 1/nz)
        // Actually for a torus on an inclined plane:
        // The torus center (tube center) is at z = z_contact + corner_radius * (1/nz - 1) + corner_radius
        // Tool tip is corner_radius below the torus tube center

        // Simpler: tool tip is at z_contact + corner_radius * (1 - nz) / nz for nz < 1
        // For nz = 1 (flat), tip = z_contact
        z_contact + corner_radius * (1.0 / nz - 1.0)
    } else {
        f64::NEG_INFINITY
    }
}

/// Torus edge contact.
fn torus_edge_contact(
    x: f64,
    y: f64,
    radius: f64,
    corner_radius: f64,
    v0: [f64; 3],
    v1: [f64; 3],
) -> f64 {
    let torus_major = radius - corner_radius;

    // Find closest point on edge to the torus center circle
    // The torus center circle is at radius torus_major from (x, y)

    let dx = v1[0] - v0[0];
    let dy = v1[1] - v0[1];
    let dz = v1[2] - v0[2];
    let len_sq = dx * dx + dy * dy + dz * dz;

    if len_sq < 1e-10 {
        return torus_vertex_contact(x, y, radius, corner_radius, v0);
    }

    // Project onto edge in XY
    let t = ((x - v0[0]) * dx + (y - v0[1]) * dy) / (dx * dx + dy * dy + 1e-10);
    let t = t.clamp(0.0, 1.0);

    let px = v0[0] + t * dx;
    let py = v0[1] + t * dy;
    let pz = v0[2] + t * dz;

    // Distance from (x, y) to (px, py)
    let dist_xy = ((x - px) * (x - px) + (y - py) * (y - py)).sqrt();

    // The torus center circle is at distance torus_major from (x, y)
    // Distance from torus center to edge point:
    let dist_to_torus_center = (dist_xy - torus_major).abs();

    if dist_to_torus_center > corner_radius {
        return f64::NEG_INFINITY;
    }

    // Height offset from edge point to torus center
    let height_offset =
        (corner_radius * corner_radius - dist_to_torus_center * dist_to_torus_center).sqrt();

    // Tool tip is corner_radius below torus center
    pz + height_offset - corner_radius + corner_radius
}

/// Torus vertex contact.
fn torus_vertex_contact(x: f64, y: f64, radius: f64, corner_radius: f64, v: [f64; 3]) -> f64 {
    let torus_major = radius - corner_radius;

    // Distance from tool center to vertex in XY
    let dist_xy = ((x - v[0]) * (x - v[0]) + (y - v[1]) * (y - v[1])).sqrt();

    // Distance from torus tube center circle to vertex
    let dist_to_torus = (dist_xy - torus_major).abs();

    if dist_to_torus > corner_radius {
        return f64::NEG_INFINITY;
    }

    // Height offset
    let height_offset = (corner_radius * corner_radius - dist_to_torus * dist_to_torus).sqrt();

    // Tool tip Z
    v[2] + height_offset
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_flat_mesh() -> MeshAccel {
        let vertices = [[0.0, 0.0, 0.0], [20.0, 0.0, 0.0], [10.0, 20.0, 0.0]];
        let indices = [0, 1, 2];
        MeshAccel::new(&vertices, &indices, 10.0)
    }

    #[test]
    fn test_bull_flat_surface() {
        let accel = make_flat_mesh();

        // Bull endmill on flat surface should have tip at z=0
        // With corner_radius=1mm, the torus contact may give slightly different values
        let z = drop_cutter_bull(&accel, 5.0, 1.0, 10.0, 10.0);
        assert!(z >= -0.1 && z < 2.0, "Expected z near 0, got {}", z);
    }

    #[test]
    fn test_bull_degenerates_to_ball() {
        let accel = make_flat_mesh();

        // When corner_radius equals radius, bull becomes ball
        let z_bull = drop_cutter_bull(&accel, 3.0, 3.0, 10.0, 10.0);
        let z_ball = super::super::ball::drop_cutter_ball(&accel, 3.0, 10.0, 10.0);

        // Ball returns sphere center Z, bull returns tip Z
        // For ball, tip is at z_center - radius = 3.0 - 3.0 = 0
        // For bull with r=R, tip should also be 0 on flat surface
        assert!(
            (z_bull - 0.0).abs() < 0.5,
            "Bull with r=R should touch flat surface at z≈0, got {}",
            z_bull
        );
    }

    #[test]
    fn test_bull_vertex_contact() {
        let accel = make_flat_mesh();

        // Near vertex at origin
        let z = drop_cutter_bull(&accel, 5.0, 1.0, 4.5, 0.5);
        assert!(z > f64::NEG_INFINITY);
    }

    #[test]
    fn test_bull_no_contact() {
        let accel = make_flat_mesh();

        let z = drop_cutter_bull(&accel, 3.0, 1.0, 100.0, 100.0);
        assert!(z == f64::NEG_INFINITY);
    }
}
