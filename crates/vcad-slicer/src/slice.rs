//! Mesh slicing - intersect triangle mesh with horizontal planes.

use rayon::prelude::*;
use vcad_kernel_math::Point2;
use vcad_kernel_tessellate::TriangleMesh;

use crate::error::{Result, SlicerError};
use crate::path::Polygon;

/// A single layer from slicing.
#[derive(Debug, Clone)]
pub struct SliceLayer {
    /// Z height of this layer (mm).
    pub z: f64,
    /// Layer index (0 = first layer).
    pub index: usize,
    /// Contours at this layer (outer boundaries and holes).
    /// Outer contours are CCW, holes are CW.
    pub contours: Vec<Polygon>,
}

impl SliceLayer {
    /// Create a new empty layer.
    pub fn new(z: f64, index: usize) -> Self {
        Self {
            z,
            index,
            contours: Vec::new(),
        }
    }
}

/// Slice a triangle mesh at multiple Z heights.
///
/// Returns layers sorted by Z height from bottom to top.
pub fn slice_mesh(mesh: &TriangleMesh, layer_heights: &[f64]) -> Result<Vec<SliceLayer>> {
    if mesh.vertices.is_empty() || mesh.indices.is_empty() {
        return Err(SlicerError::EmptyMesh);
    }

    // Extract triangles
    let triangles = extract_triangles(mesh)?;

    // Slice in parallel
    let layers: Vec<SliceLayer> = layer_heights
        .par_iter()
        .enumerate()
        .map(|(idx, &z)| slice_at_z(&triangles, z, idx))
        .collect();

    Ok(layers)
}

/// A triangle with its vertices and bounding Z range.
#[derive(Debug, Clone, Copy)]
struct Triangle {
    v0: [f64; 3],
    v1: [f64; 3],
    v2: [f64; 3],
    z_min: f64,
    z_max: f64,
}

/// Extract triangles from mesh for slicing.
fn extract_triangles(mesh: &TriangleMesh) -> Result<Vec<Triangle>> {
    let num_triangles = mesh.indices.len() / 3;
    let mut triangles = Vec::with_capacity(num_triangles);

    for i in 0..num_triangles {
        let i0 = mesh.indices[i * 3] as usize;
        let i1 = mesh.indices[i * 3 + 1] as usize;
        let i2 = mesh.indices[i * 3 + 2] as usize;

        let v0 = [
            mesh.vertices[i0 * 3] as f64,
            mesh.vertices[i0 * 3 + 1] as f64,
            mesh.vertices[i0 * 3 + 2] as f64,
        ];
        let v1 = [
            mesh.vertices[i1 * 3] as f64,
            mesh.vertices[i1 * 3 + 1] as f64,
            mesh.vertices[i1 * 3 + 2] as f64,
        ];
        let v2 = [
            mesh.vertices[i2 * 3] as f64,
            mesh.vertices[i2 * 3 + 1] as f64,
            mesh.vertices[i2 * 3 + 2] as f64,
        ];

        let z_min = v0[2].min(v1[2]).min(v2[2]);
        let z_max = v0[2].max(v1[2]).max(v2[2]);

        triangles.push(Triangle {
            v0,
            v1,
            v2,
            z_min,
            z_max,
        });
    }

    Ok(triangles)
}

/// Slice mesh at a single Z height.
fn slice_at_z(triangles: &[Triangle], z: f64, index: usize) -> SliceLayer {
    let mut segments: Vec<([f64; 2], [f64; 2])> = Vec::new();

    for tri in triangles {
        // Skip triangles that don't cross this Z plane
        if tri.z_max < z || tri.z_min > z {
            continue;
        }

        // Find intersection segment
        if let Some(seg) = triangle_plane_intersection(tri, z) {
            segments.push(seg);
        }
    }

    // Chain segments into contours
    let contours = chain_segments(segments);

    SliceLayer { z, index, contours }
}

/// Intersect a triangle with a horizontal plane at Z.
/// Returns the 2D line segment (projected to XY) if intersection exists.
fn triangle_plane_intersection(tri: &Triangle, z: f64) -> Option<([f64; 2], [f64; 2])> {
    let eps = 1e-10;

    // Classify vertices relative to plane
    let d0 = tri.v0[2] - z;
    let d1 = tri.v1[2] - z;
    let d2 = tri.v2[2] - z;

    // Count vertices on each side
    let above0 = d0 > eps;
    let above1 = d1 > eps;
    let above2 = d2 > eps;
    let below0 = d0 < -eps;
    let below1 = d1 < -eps;
    let below2 = d2 < -eps;

    // All on same side - no intersection
    if (above0 && above1 && above2) || (below0 && below1 && below2) {
        return None;
    }

    // Collect intersection points
    let mut points: Vec<[f64; 2]> = Vec::with_capacity(2);

    // Check each edge
    let edges = [
        (tri.v0, tri.v1, d0, d1),
        (tri.v1, tri.v2, d1, d2),
        (tri.v2, tri.v0, d2, d0),
    ];

    for (va, vb, da, db) in edges {
        // Edge crosses plane if signs differ (and neither is exactly on plane)
        if (da > eps && db < -eps) || (da < -eps && db > eps) {
            // Interpolate to find intersection point
            let t = da / (da - db);
            let x = va[0] + t * (vb[0] - va[0]);
            let y = va[1] + t * (vb[1] - va[1]);
            points.push([x, y]);
        } else if da.abs() <= eps && db.abs() > eps {
            // Vertex a is on the plane
            points.push([va[0], va[1]]);
        } else if db.abs() <= eps && da.abs() > eps {
            // Vertex b is on the plane
            points.push([vb[0], vb[1]]);
        }
    }

    // Remove duplicates
    points.dedup_by(|a, b| {
        let dx = a[0] - b[0];
        let dy = a[1] - b[1];
        (dx * dx + dy * dy) < eps * eps
    });

    if points.len() >= 2 {
        Some((points[0], points[1]))
    } else {
        None
    }
}

/// Chain line segments into closed polygons.
fn chain_segments(segments: Vec<([f64; 2], [f64; 2])>) -> Vec<Polygon> {
    if segments.is_empty() {
        return Vec::new();
    }

    let eps = 1e-6;
    let mut remaining = segments;
    let mut contours: Vec<Polygon> = Vec::new();

    while !remaining.is_empty() {
        let (start, end) = remaining.remove(0);
        let mut chain = vec![Point2::new(start[0], start[1]), Point2::new(end[0], end[1])];

        let mut changed = true;
        while changed {
            changed = false;

            // Copy start and end to avoid borrow issues
            let chain_start = *chain.first().unwrap();
            let chain_end = *chain.last().unwrap();

            // Try to extend the chain
            let mut i = 0;
            while i < remaining.len() {
                let (seg_a, seg_b) = remaining[i];
                let pa = Point2::new(seg_a[0], seg_a[1]);
                let pb = Point2::new(seg_b[0], seg_b[1]);

                // Check if segment connects to chain end
                if (pb - chain_end).norm() < eps {
                    chain.push(pa);
                    remaining.remove(i);
                    changed = true;
                } else if (pa - chain_end).norm() < eps {
                    chain.push(pb);
                    remaining.remove(i);
                    changed = true;
                }
                // Check if segment connects to chain start
                else if (pb - chain_start).norm() < eps {
                    chain.insert(0, pa);
                    remaining.remove(i);
                    changed = true;
                } else if (pa - chain_start).norm() < eps {
                    chain.insert(0, pb);
                    remaining.remove(i);
                    changed = true;
                } else {
                    i += 1;
                }
            }
        }

        // Check if chain is closed
        if chain.len() >= 3 {
            let dist = (chain.first().unwrap() - chain.last().unwrap()).norm();
            if dist < eps {
                chain.pop(); // Remove duplicate closing point
            }
            if chain.len() >= 3 {
                contours.push(Polygon::new(chain));
            }
        }
    }

    // Sort contours: outer (CCW, positive area) first, then holes (CW, negative area)
    contours.sort_by(|a, b| {
        b.signed_area()
            .abs()
            .partial_cmp(&a.signed_area().abs())
            .unwrap()
    });

    contours
}

/// Compute the bounding box of a mesh.
/// Returns (min, max) as ([x, y, z], [x, y, z]).
pub fn mesh_bounds(mesh: &TriangleMesh) -> Option<([f64; 3], [f64; 3])> {
    if mesh.vertices.is_empty() {
        return None;
    }

    let mut min = [f64::MAX, f64::MAX, f64::MAX];
    let mut max = [f64::MIN, f64::MIN, f64::MIN];

    for i in 0..(mesh.vertices.len() / 3) {
        let x = mesh.vertices[i * 3] as f64;
        let y = mesh.vertices[i * 3 + 1] as f64;
        let z = mesh.vertices[i * 3 + 2] as f64;

        min[0] = min[0].min(x);
        min[1] = min[1].min(y);
        min[2] = min[2].min(z);
        max[0] = max[0].max(x);
        max[1] = max[1].max(y);
        max[2] = max[2].max(z);
    }

    Some((min, max))
}

/// Generate layer heights for slicing.
pub fn generate_layer_heights(
    z_min: f64,
    z_max: f64,
    first_layer_height: f64,
    layer_height: f64,
) -> Vec<f64> {
    let mut heights = Vec::new();

    if z_max <= z_min {
        return heights;
    }

    // First layer
    let first_z = z_min + first_layer_height / 2.0;
    if first_z <= z_max {
        heights.push(first_z);
    }

    // Subsequent layers
    let mut z = z_min + first_layer_height + layer_height / 2.0;
    while z <= z_max {
        heights.push(z);
        z += layer_height;
    }

    heights
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cube_mesh() -> TriangleMesh {
        // Simple 10x10x10 cube
        let size = 10.0f32;
        let vertices = vec![
            // Bottom face (z=0)
            0.0, 0.0, 0.0, size, 0.0, 0.0, size, size, 0.0, 0.0, size, 0.0,
            // Top face (z=size)
            0.0, 0.0, size, size, 0.0, size, size, size, size, 0.0, size, size,
        ];
        let indices = vec![
            // Bottom
            0, 2, 1, 0, 3, 2, // Top
            4, 5, 6, 4, 6, 7, // Front
            0, 1, 5, 0, 5, 4, // Back
            2, 3, 7, 2, 7, 6, // Left
            0, 4, 7, 0, 7, 3, // Right
            1, 2, 6, 1, 6, 5,
        ];
        TriangleMesh {
            vertices,
            indices,
            normals: Vec::new(),
            face_kinds: Vec::new(),
        }
    }

    #[test]
    fn test_mesh_bounds() {
        let mesh = make_cube_mesh();
        let (min, max) = mesh_bounds(&mesh).unwrap();
        assert!((min[0]).abs() < 1e-6);
        assert!((min[1]).abs() < 1e-6);
        assert!((min[2]).abs() < 1e-6);
        assert!((max[0] - 10.0).abs() < 1e-6);
        assert!((max[1] - 10.0).abs() < 1e-6);
        assert!((max[2] - 10.0).abs() < 1e-6);
    }

    #[test]
    fn test_generate_layer_heights() {
        let heights = generate_layer_heights(0.0, 10.0, 0.3, 0.2);
        assert!(!heights.is_empty());
        assert!(heights[0] > 0.0);
        assert!(*heights.last().unwrap() <= 10.0);
    }

    #[test]
    fn test_slice_cube() {
        let mesh = make_cube_mesh();
        let heights = generate_layer_heights(0.0, 10.0, 0.3, 0.2);
        let layers = slice_mesh(&mesh, &heights).unwrap();
        assert!(!layers.is_empty());
        // Each layer should have exactly one square contour
        for layer in &layers {
            assert!(!layer.contours.is_empty());
        }
    }
}
