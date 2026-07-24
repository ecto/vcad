//! Mesh mass-property math: divergence-theorem volume, surface area,
//! bounding box, and volume-weighted center of mass for a triangle soup.
//!
//! This is the single source of truth for tessellation-bound inspection
//! metrics — the WASM binding `computeMeshProperties` (consumed by the app
//! and the MCP server's `inspect_cad` / `measure` / fabricate cost models)
//! wraps [`compute_mesh_properties`] directly.

/// An axis-aligned bounding box (`[x, y, z]` min / max corners).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshBBox {
    /// Minimum corner.
    pub min: [f64; 3],
    /// Maximum corner.
    pub max: [f64; 3],
}

/// Aggregate mass properties of a triangle mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshProperties {
    /// Enclosed volume (mm³ for mm positions), absolute value.
    pub volume: f64,
    /// Total surface area (mm²).
    pub area: f64,
    /// Axis-aligned bounding box over all referenced vertices.
    pub bbox: MeshBBox,
    /// Center of mass. Volume-weighted (divergence theorem) for a closed,
    /// consistently wound mesh; falls back to the area-weighted surface
    /// centroid when the volume integral is unreliable (open meshes, or a
    /// centroid that lands outside the bbox — geometrically impossible for
    /// a real solid).
    pub center_of_mass: [f64; 3],
    /// Number of triangles integrated.
    pub triangles: usize,
}

/// Compute volume, surface area, bounding box, and center of mass for a
/// triangle mesh given flat `[x, y, z, ...]` positions and `[i0, i1, i2, ...]`
/// indices. Degenerate index triples (out of range or truncated) are skipped.
pub fn compute_mesh_properties(positions: &[f32], indices: &[u32]) -> MeshProperties {
    let mut volume = 0.0_f64;
    let mut area = 0.0_f64;
    // Volume-weighted centroid accumulator.
    let mut c = [0.0_f64; 3];
    // Area-weighted surface centroid accumulator (fallback).
    let mut ac = [0.0_f64; 3];
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    let mut triangles = 0_usize;

    let vertex = |i: u32| -> Option<[f64; 3]> {
        let i = i as usize * 3;
        if i + 2 >= positions.len() {
            return None;
        }
        Some([
            positions[i] as f64,
            positions[i + 1] as f64,
            positions[i + 2] as f64,
        ])
    };

    for tri in indices.chunks_exact(3) {
        let (Some(p1), Some(p2), Some(p3)) = (vertex(tri[0]), vertex(tri[1]), vertex(tri[2]))
        else {
            continue;
        };
        triangles += 1;

        // Signed volume of the tetrahedron (origin, p1, p2, p3).
        let v = (p1[0] * (p2[1] * p3[2] - p3[1] * p2[2]) - p2[0] * (p1[1] * p3[2] - p3[1] * p1[2])
            + p3[0] * (p1[1] * p2[2] - p2[1] * p1[2]))
            / 6.0;
        volume += v;

        let e1 = [p2[0] - p1[0], p2[1] - p1[1], p2[2] - p1[2]];
        let e2 = [p3[0] - p1[0], p3[1] - p1[1], p3[2] - p1[2]];
        let cross = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let a = (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt() / 2.0;
        area += a;

        for k in 0..3 {
            // Tet centroid is (0 + p1 + p2 + p3) / 4; area centroid is /3.
            c[k] += v * (p1[k] + p2[k] + p3[k]) / 4.0;
            ac[k] += a * (p1[k] + p2[k] + p3[k]) / 3.0;
            for p in [&p1, &p2, &p3] {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
    }

    let abs_volume = volume.abs();
    let mut com: Option<[f64; 3]> = None;
    if abs_volume > 1e-10 {
        let candidate = [c[0] / volume, c[1] / volume, c[2] / volume];
        let eps = 1e-9
            * (max[0] - min[0])
                .max(max[1] - min[1])
                .max(max[2] - min[2])
                .max(1.0);
        let in_bbox = (0..3).all(|k| candidate[k] >= min[k] - eps && candidate[k] <= max[k] + eps);
        if in_bbox {
            com = Some(candidate);
        }
    }
    let center_of_mass = com.unwrap_or(if area > 0.0 {
        [ac[0] / area, ac[1] / area, ac[2] / area]
    } else {
        [0.0; 3]
    });

    MeshProperties {
        volume: abs_volume,
        area,
        bbox: MeshBBox { min, max },
        center_of_mass,
        triangles,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Axis-aligned box mesh from `min` to `max`, outward-wound.
    fn box_mesh(min: [f32; 3], max: [f32; 3]) -> (Vec<f32>, Vec<u32>) {
        let [x0, y0, z0] = min;
        let [x1, y1, z1] = max;
        let positions = vec![
            x0, y0, z0, x1, y0, z0, x1, y1, z0, x0, y1, z0, // bottom ring
            x0, y0, z1, x1, y0, z1, x1, y1, z1, x0, y1, z1, // top ring
        ];
        #[rustfmt::skip]
        let indices = vec![
            0, 2, 1, 0, 3, 2, // bottom (−z)
            4, 5, 6, 4, 6, 7, // top (+z)
            0, 1, 5, 0, 5, 4, // −y
            2, 3, 7, 2, 7, 6, // +y
            1, 2, 6, 1, 6, 5, // +x
            3, 0, 4, 3, 4, 7, // −x
        ];
        (positions, indices)
    }

    #[test]
    fn unit_cube_pins_known_values() {
        let (pos, idx) = box_mesh([0.0; 3], [1.0; 3]);
        let p = compute_mesh_properties(&pos, &idx);
        assert!((p.volume - 1.0).abs() < 1e-9);
        assert!((p.area - 6.0).abs() < 1e-9);
        assert_eq!(p.triangles, 12);
        assert_eq!(p.bbox.min, [0.0; 3]);
        assert_eq!(p.bbox.max, [1.0; 3]);
        for k in 0..3 {
            assert!((p.center_of_mass[k] - 0.5).abs() < 1e-9, "com axis {k}");
        }
    }

    #[test]
    fn offset_box_center_of_mass_follows_offset() {
        let (pos, idx) = box_mesh([10.0, -4.0, 2.0], [14.0, 0.0, 12.0]);
        let p = compute_mesh_properties(&pos, &idx);
        assert!((p.volume - 4.0 * 4.0 * 10.0).abs() < 1e-6);
        let expected = [12.0, -2.0, 7.0];
        for k in 0..3 {
            assert!(
                (p.center_of_mass[k] - expected[k]).abs() < 1e-6,
                "com axis {k}: {} vs {}",
                p.center_of_mass[k],
                expected[k]
            );
        }
    }

    #[test]
    fn open_mesh_falls_back_to_surface_centroid() {
        // Single triangle in the z=3 plane: no enclosed volume, centroid
        // must still land inside the bbox (area-weighted fallback).
        let pos = vec![0.0, 0.0, 3.0, 2.0, 0.0, 3.0, 0.0, 2.0, 3.0];
        let idx = vec![0, 1, 2];
        let p = compute_mesh_properties(&pos, &idx);
        assert!((p.area - 2.0).abs() < 1e-9);
        assert!((p.center_of_mass[2] - 3.0).abs() < 1e-9);
    }

    #[test]
    fn degenerate_indices_are_skipped() {
        let pos = vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let idx = vec![0, 1, 2, 0, 1, 99]; // second triple out of range
        let p = compute_mesh_properties(&pos, &idx);
        assert_eq!(p.triangles, 1);
    }
}
