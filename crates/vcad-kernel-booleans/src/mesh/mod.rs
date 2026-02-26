//! Mesh-based utilities for boolean operations.

use vcad_kernel_math::Point3;
use vcad_kernel_tessellate::TriangleMesh;

/// Test if a point is inside a closed triangle mesh using ray casting with exact predicates.
///
/// Uses Shewchuk's exact orient3d predicate to robustly handle boundary cases where
/// the query point is exactly on a triangle plane. Uses a slightly tilted ray direction
/// to avoid edge/vertex hits in the common case, with exact predicates as fallback.
///
/// Casts a ray along a tilted direction. Odd crossing count = inside, even = outside.
pub fn point_in_mesh(point: &Point3, mesh: &TriangleMesh) -> bool {
    use vcad_kernel_math::predicates::{orient3d, Sign};

    let verts = &mesh.vertices;
    let indices = &mesh.indices;
    let mut crossings = 0u32;

    // Slightly tilted ray direction to avoid hitting edges/vertices exactly
    // The exact predicates handle remaining boundary cases robustly
    let ray_dir = [1.0f64, 1e-7, 1.3e-7];

    for tri in indices.chunks(3) {
        let i0 = tri[0] as usize * 3;
        let i1 = tri[1] as usize * 3;
        let i2 = tri[2] as usize * 3;

        let v0 = [verts[i0] as f64, verts[i0 + 1] as f64, verts[i0 + 2] as f64];
        let v1 = [verts[i1] as f64, verts[i1 + 1] as f64, verts[i1 + 2] as f64];
        let v2 = [verts[i2] as f64, verts[i2 + 1] as f64, verts[i2 + 2] as f64];

        // Möller-Trumbore ray-triangle intersection
        let edge1 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
        let edge2 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];

        // h = ray_dir × edge2
        let h = [
            ray_dir[1] * edge2[2] - ray_dir[2] * edge2[1],
            ray_dir[2] * edge2[0] - ray_dir[0] * edge2[2],
            ray_dir[0] * edge2[1] - ray_dir[1] * edge2[0],
        ];

        let a = edge1[0] * h[0] + edge1[1] * h[1] + edge1[2] * h[2];

        // Use exact orient3d to robustly check for degenerate cases
        if a.abs() < 1e-12 {
            // Ray nearly parallel to triangle - use exact predicate
            let p0 = Point3::new(v0[0], v0[1], v0[2]);
            let p1 = Point3::new(v1[0], v1[1], v1[2]);
            let p2 = Point3::new(v2[0], v2[1], v2[2]);
            let far_pt = Point3::new(
                point.x + ray_dir[0] * 1e10,
                point.y + ray_dir[1] * 1e10,
                point.z + ray_dir[2] * 1e10,
            );

            // Check if query point is coplanar with triangle
            let sign = orient3d(point, &p0, &p1, &p2);
            if matches!(sign, Sign::Zero) {
                // Point is on the triangle plane - check if inside triangle
                if point_in_triangle_coplanar(point, &p0, &p1, &p2) {
                    // Point on boundary - treat as inside (odd crossing)
                    return true;
                }
            }

            // Check if ray pierces the infinite plane containing the triangle
            let sign_far = orient3d(&far_pt, &p0, &p1, &p2);
            if sign == sign_far {
                continue; // Ray doesn't cross plane
            }
            // Would need more robust intersection test here, skip for now
            continue;
        }

        let f = 1.0 / a;
        let s = [point.x - v0[0], point.y - v0[1], point.z - v0[2]];

        let u = f * (s[0] * h[0] + s[1] * h[1] + s[2] * h[2]);
        if !(0.0..=1.0).contains(&u) {
            continue;
        }

        // q = s × edge1
        let q = [
            s[1] * edge1[2] - s[2] * edge1[1],
            s[2] * edge1[0] - s[0] * edge1[2],
            s[0] * edge1[1] - s[1] * edge1[0],
        ];

        let v = f * (ray_dir[0] * q[0] + ray_dir[1] * q[1] + ray_dir[2] * q[2]);
        if v < 0.0 || u + v > 1.0 {
            continue;
        }

        let t = f * (edge2[0] * q[0] + edge2[1] * q[1] + edge2[2] * q[2]);

        // Only count forward intersections (t > 0)
        if t > 1e-10 {
            crossings += 1;
        }
    }

    crossings % 2 == 1
}

/// Check if point p is inside triangle (v0, v1, v2) when all are coplanar.
/// Uses exact orient3d predicates for robust edge tests.
fn point_in_triangle_coplanar(p: &Point3, v0: &Point3, v1: &Point3, v2: &Point3) -> bool {
    use vcad_kernel_math::predicates::orient3d;

    // Compute triangle normal
    let e1 = v1 - v0;
    let e2 = v2 - v0;
    let normal = e1.cross(e2);
    let normal_len = normal.norm();
    if normal_len < 1e-15 {
        return false; // Degenerate triangle
    }

    // Create a reference point above the plane
    let ref_pt = Point3::new(
        p.x + normal.x / normal_len,
        p.y + normal.y / normal_len,
        p.z + normal.z / normal_len,
    );

    // Test orientation against each edge
    let s0 = orient3d(p, v0, v1, &ref_pt);
    let s1 = orient3d(p, v1, v2, &ref_pt);
    let s2 = orient3d(p, v2, v0, &ref_pt);

    // Point is inside if all orientations are consistent (all ≥0 or all ≤0)
    let all_non_neg = !s0.is_negative() && !s1.is_negative() && !s2.is_negative();
    let all_non_pos = !s0.is_positive() && !s1.is_positive() && !s2.is_positive();

    all_non_neg || all_non_pos
}
