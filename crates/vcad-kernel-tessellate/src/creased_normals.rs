//! Angle-based creased vertex normals.
//!
//! Port of three.js' `BufferGeometryUtils.toCreasedNormals` into the kernel so
//! every renderer (web three.js, native wgpu, CLI exports, ray tracer, …)
//! consumes the same per-vertex normals without recomputing them.
//!
//! The algorithm:
//!
//! 1. For each triangle compute a flat face normal.
//! 2. Group triangles sharing a position (by spatial key with a tolerance).
//! 3. For every (triangle, corner) pair, average the flat normals of every
//!    triangle in that group whose face normal is within `crease_angle` of the
//!    current triangle's face normal. Triangles that fall outside the cone
//!    don't contribute — their corner gets its own normal, producing a crease.
//! 4. Write the averaged, normalized normal back into a per-triangle-vertex
//!    flat attribute (unindexed). Indices are rewritten so that each vertex
//!    only belongs to one smoothing group.
//!
//! After this runs, consumers treat the output as a render-ready mesh with a
//! normal attribute that exactly matches what `three.js`' `toCreasedNormals`
//! would have produced, but computed once deterministically at tessellation
//! time rather than at upload time in the browser.
//!
//! The default crease angle of 30° (`π/6`) matches the long-standing web-app
//! default.

use crate::TriangleMesh;

/// Default crease angle in radians (30°). Below this, adjacent faces are
/// smoothed; above this a hard crease is introduced.
pub const DEFAULT_CREASE_ANGLE_RAD: f64 = std::f64::consts::FRAC_PI_6;

/// Tolerance used to treat two vertex positions as coincident when building
/// the smoothing groups. 1 µm in kernel units (mm).
const POSITION_WELD_EPS: f32 = 1e-3;

/// Rewrite `mesh.vertices` / `mesh.indices` / `mesh.normals` so that vertex
/// normals are averaged across faces meeting at an angle ≤ `crease_angle`
/// (radians) and split across sharper edges.
///
/// The returned mesh is **unindexed** in the three.js sense: every triangle
/// has its own three vertices. `face_kinds` is preserved one-to-one with
/// triangles. Positions are unchanged numerically but may be duplicated.
pub fn apply_creased_normals(mesh: &mut TriangleMesh, crease_angle_rad: f64) {
    let triangle_count = mesh.num_triangles();
    if triangle_count == 0 {
        return;
    }

    let cos_threshold = crease_angle_rad.cos() as f32;

    // --- Step 1: flat face normals and a welded-position key for each corner.
    let mut face_normals: Vec<[f32; 3]> = Vec::with_capacity(triangle_count);
    let mut corner_keys: Vec<u64> = Vec::with_capacity(triangle_count * 3);

    for tri in 0..triangle_count {
        let i0 = mesh.indices[tri * 3] as usize;
        let i1 = mesh.indices[tri * 3 + 1] as usize;
        let i2 = mesh.indices[tri * 3 + 2] as usize;

        let p0 = vertex(mesh, i0);
        let p1 = vertex(mesh, i1);
        let p2 = vertex(mesh, i2);

        face_normals.push(face_normal(p0, p1, p2));

        corner_keys.push(position_key(p0));
        corner_keys.push(position_key(p1));
        corner_keys.push(position_key(p2));
    }

    // --- Step 2: group corners by welded position.
    let mut groups: std::collections::HashMap<u64, Vec<u32>> =
        std::collections::HashMap::new();
    for (corner_idx, &key) in corner_keys.iter().enumerate() {
        groups.entry(key).or_default().push(corner_idx as u32);
    }

    // --- Step 3: compute per-corner averaged normal + emit unindexed geometry.
    let mut new_vertices: Vec<f32> = Vec::with_capacity(triangle_count * 9);
    let mut new_normals: Vec<f32> = Vec::with_capacity(triangle_count * 9);

    for tri in 0..triangle_count {
        let tri_normal = face_normals[tri];
        for corner in 0..3 {
            let corner_idx = tri * 3 + corner;
            let vertex_idx = mesh.indices[corner_idx] as usize;
            let pos = vertex(mesh, vertex_idx);

            let key = corner_keys[corner_idx];
            let group = groups.get(&key).expect("group key populated above");

            // Sum face normals that are within the crease cone.
            let mut sum = [0.0_f32; 3];
            for &other_corner in group {
                let other_tri = (other_corner / 3) as usize;
                let other_normal = face_normals[other_tri];
                if dot(tri_normal, other_normal) >= cos_threshold {
                    sum[0] += other_normal[0];
                    sum[1] += other_normal[1];
                    sum[2] += other_normal[2];
                }
            }
            let averaged = normalize_or(sum, tri_normal);

            new_vertices.extend_from_slice(&pos);
            new_normals.extend_from_slice(&averaged);
        }
    }

    // Indices become trivial: 0..(3 * triangle_count).
    let mut new_indices: Vec<u32> = Vec::with_capacity(triangle_count * 3);
    for i in 0..(triangle_count * 3) as u32 {
        new_indices.push(i);
    }

    mesh.vertices = new_vertices;
    mesh.normals = new_normals;
    mesh.indices = new_indices;
    // face_kinds is indexed per-triangle, not per-vertex, so it stays valid.
}

/// Apply creasing with [`DEFAULT_CREASE_ANGLE_RAD`].
pub fn apply_default_creased_normals(mesh: &mut TriangleMesh) {
    apply_creased_normals(mesh, DEFAULT_CREASE_ANGLE_RAD);
}

// --- helpers ---------------------------------------------------------------

fn vertex(mesh: &TriangleMesh, idx: usize) -> [f32; 3] {
    [
        mesh.vertices[idx * 3],
        mesh.vertices[idx * 3 + 1],
        mesh.vertices[idx * 3 + 2],
    ]
}

fn face_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ];
    normalize_or(n, [0.0, 0.0, 1.0])
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize_or(v: [f32; 3], fallback: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-12 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        fallback
    }
}

/// Quantize a 3D position to a u64 bucket so that vertices within
/// [`POSITION_WELD_EPS`] land in the same bucket when walking smoothing groups.
///
/// The position is rounded to the nearest multiple of `POSITION_WELD_EPS` and
/// the three i20 buckets are packed into a single u64.
fn position_key(p: [f32; 3]) -> u64 {
    let scale = 1.0 / POSITION_WELD_EPS;
    let qx = (p[0] * scale).round() as i64;
    let qy = (p[1] * scale).round() as i64;
    let qz = (p[2] * scale).round() as i64;
    // Pack three i21-ish coordinates into 63 bits of a u64. Max absolute value
    // of roughly 2^20 ≈ 1 million *scale-units* = 1 km at 1 µm precision, which
    // is way beyond any plausible CAD bounds.
    let mix = |v: i64| (v.wrapping_add(1 << 20) as u64) & 0x1F_FFFF;
    (mix(qx) << 42) | (mix(qy) << 21) | mix(qz)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two triangles forming an L-shape sharing one edge at 90°.
    /// With a 30° crease angle, the shared edge should become a hard crease
    /// — the two corners on the shared edge end up with distinct normals.
    #[test]
    fn creases_90_degree_edge() {
        let mut mesh = TriangleMesh {
            // Triangle A: floor (z=0), normal = +Z
            // Triangle B: wall (y=0), normal = -Y
            vertices: vec![
                // A (on z=0 plane)
                0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 10.0, 10.0, 0.0,
                // B (on y=0 plane)
                0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 10.0, 0.0, 10.0,
            ],
            indices: vec![0, 1, 2, 3, 4, 5],
            normals: Vec::new(),
            face_kinds: Vec::new(),
        };

        apply_creased_normals(&mut mesh, DEFAULT_CREASE_ANGLE_RAD);

        // After creasing, triangle A's corners should carry +Z,
        // triangle B's corners should carry -Y. Both triangles now have 3
        // unique vertices each (no welding across the crease).
        assert_eq!(mesh.normals.len(), 6 * 3);
        for tri in 0..2 {
            for corner in 0..3 {
                let n_idx = (tri * 3 + corner) * 3;
                let n = [mesh.normals[n_idx], mesh.normals[n_idx + 1], mesh.normals[n_idx + 2]];
                let expected = if tri == 0 { [0.0, 0.0, 1.0] } else { [0.0, -1.0, 0.0] };
                let diff = (n[0] - expected[0]).abs()
                    + (n[1] - expected[1]).abs()
                    + (n[2] - expected[2]).abs();
                assert!(diff < 1e-5, "tri {tri} corner {corner}: got {n:?} want {expected:?}");
            }
        }
    }

    /// Two triangles forming a flat square sharing one edge, coplanar.
    /// With any crease angle the shared corners should average to the common
    /// face normal (both are +Z).
    #[test]
    fn smooths_coplanar_edge() {
        let mut mesh = TriangleMesh {
            vertices: vec![
                0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 10.0, 10.0, 0.0,
                0.0, 0.0, 0.0, 10.0, 10.0, 0.0, 0.0, 10.0, 0.0,
            ],
            indices: vec![0, 1, 2, 3, 4, 5],
            normals: Vec::new(),
            face_kinds: Vec::new(),
        };

        apply_creased_normals(&mut mesh, DEFAULT_CREASE_ANGLE_RAD);

        for corner in 0..6 {
            let n_idx = corner * 3;
            let n = [mesh.normals[n_idx], mesh.normals[n_idx + 1], mesh.normals[n_idx + 2]];
            let expected = [0.0, 0.0, 1.0];
            let diff = (n[0] - expected[0]).abs()
                + (n[1] - expected[1]).abs()
                + (n[2] - expected[2]).abs();
            assert!(diff < 1e-5, "corner {corner}: got {n:?} want {expected:?}");
        }
    }

    /// Three triangles forming a fan around one shared corner, all within 30°.
    /// The averaged normal at the shared corner should be between the three
    /// face normals.
    #[test]
    fn averages_smooth_group() {
        // Three triangles on a gently curved surface, all normals within
        // ~10° of vertical.
        let tilt = 0.15_f32; // small tilt in x; roughly 8.5°
        let mut mesh = TriangleMesh {
            vertices: vec![
                0.0, 0.0, 0.0, 1.0, 0.0, tilt, 0.5, 1.0, 0.0,
                0.0, 0.0, 0.0, 0.5, 1.0, 0.0, -1.0, 0.0, -tilt,
                0.0, 0.0, 0.0, -1.0, 0.0, -tilt, 1.0, 0.0, tilt,
            ],
            indices: vec![0, 1, 2, 3, 4, 5, 6, 7, 8],
            normals: Vec::new(),
            face_kinds: Vec::new(),
        };

        apply_creased_normals(&mut mesh, DEFAULT_CREASE_ANGLE_RAD);

        // The shared corner (position 0,0,0) is the first vertex of every
        // triangle. After unindexing there are three copies; they should all
        // carry the same averaged normal.
        let n0 = [mesh.normals[0], mesh.normals[1], mesh.normals[2]];
        let n3 = [mesh.normals[9], mesh.normals[10], mesh.normals[11]];
        let n6 = [mesh.normals[18], mesh.normals[19], mesh.normals[20]];
        for (i, n) in [n0, n3, n6].iter().enumerate() {
            let diff = (n[0] - n0[0]).abs() + (n[1] - n0[1]).abs() + (n[2] - n0[2]).abs();
            assert!(diff < 1e-5, "copy {i} normal differs from others: {n:?} vs {n0:?}");
        }
    }
}
