//! 2D spatial acceleration for mesh triangle lookup.

use std::collections::HashMap;

/// A triangle in the mesh with precomputed data.
#[derive(Debug, Clone)]
pub struct Triangle {
    /// Vertex positions.
    pub v: [[f64; 3]; 3],
    /// Triangle normal.
    pub normal: [f64; 3],
    /// Plane equation: normal · p = d
    pub d: f64,
    /// 2D bounding box [min_x, min_y, max_x, max_y].
    pub bbox_2d: [f64; 4],
}

impl Triangle {
    /// Create a new triangle from vertices.
    pub fn new(v0: [f64; 3], v1: [f64; 3], v2: [f64; 3]) -> Self {
        let e1 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
        let e2 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];

        let normal = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];

        let len = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
        let normal = if len > 1e-10 {
            [normal[0] / len, normal[1] / len, normal[2] / len]
        } else {
            [0.0, 0.0, 1.0]
        };

        let d = normal[0] * v0[0] + normal[1] * v0[1] + normal[2] * v0[2];

        let min_x = v0[0].min(v1[0]).min(v2[0]);
        let min_y = v0[1].min(v1[1]).min(v2[1]);
        let max_x = v0[0].max(v1[0]).max(v2[0]);
        let max_y = v0[1].max(v1[1]).max(v2[1]);

        Self {
            v: [v0, v1, v2],
            normal,
            d,
            bbox_2d: [min_x, min_y, max_x, max_y],
        }
    }

    /// Get the Z coordinate on the triangle plane at (x, y).
    /// Returns None if the normal is nearly horizontal.
    pub fn z_at_xy(&self, x: f64, y: f64) -> Option<f64> {
        if self.normal[2].abs() < 1e-10 {
            return None;
        }
        Some((self.d - self.normal[0] * x - self.normal[1] * y) / self.normal[2])
    }

    /// Check if point (x, y) is inside the triangle in 2D projection.
    pub fn contains_xy(&self, x: f64, y: f64) -> bool {
        // Use barycentric coordinates
        let v0 = &self.v[0];
        let v1 = &self.v[1];
        let v2 = &self.v[2];

        let d00 = (v1[0] - v0[0]) * (v1[0] - v0[0]) + (v1[1] - v0[1]) * (v1[1] - v0[1]);
        let d01 = (v1[0] - v0[0]) * (v2[0] - v0[0]) + (v1[1] - v0[1]) * (v2[1] - v0[1]);
        let d11 = (v2[0] - v0[0]) * (v2[0] - v0[0]) + (v2[1] - v0[1]) * (v2[1] - v0[1]);
        let d20 = (x - v0[0]) * (v1[0] - v0[0]) + (y - v0[1]) * (v1[1] - v0[1]);
        let d21 = (x - v0[0]) * (v2[0] - v0[0]) + (y - v0[1]) * (v2[1] - v0[1]);

        let denom = d00 * d11 - d01 * d01;
        if denom.abs() < 1e-10 {
            return false;
        }

        let v = (d11 * d20 - d01 * d21) / denom;
        let w = (d00 * d21 - d01 * d20) / denom;
        let u = 1.0 - v - w;

        // Allow small negative values for numerical stability at edges
        let eps = -1e-8;
        u >= eps && v >= eps && w >= eps
    }

    /// Get the edges of the triangle as line segments.
    pub fn edges(&self) -> [[[f64; 3]; 2]; 3] {
        [
            [self.v[0], self.v[1]],
            [self.v[1], self.v[2]],
            [self.v[2], self.v[0]],
        ]
    }
}

/// 2D grid-based spatial acceleration for mesh triangles.
pub struct MeshAccel {
    triangles: Vec<Triangle>,
    cell_size: f64,
    bounds: [f64; 4], // [min_x, min_y, max_x, max_y]
    grid_nx: usize,
    grid_ny: usize,
    cells: HashMap<(usize, usize), Vec<usize>>,
}

impl MeshAccel {
    /// Create a new mesh acceleration structure from vertices and triangle indices.
    ///
    /// # Arguments
    ///
    /// * `vertices` - Vertex positions as [x, y, z]
    /// * `indices` - Triangle indices (groups of 3)
    /// * `cell_size` - Size of grid cells for spatial hashing
    pub fn new(vertices: &[[f64; 3]], indices: &[u32], cell_size: f64) -> Self {
        let mut triangles = Vec::with_capacity(indices.len() / 3);

        // Compute mesh bounds
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;

        for v in vertices {
            min_x = min_x.min(v[0]);
            min_y = min_y.min(v[1]);
            max_x = max_x.max(v[0]);
            max_y = max_y.max(v[1]);
        }

        // Add small padding
        let padding = cell_size * 0.1;
        min_x -= padding;
        min_y -= padding;
        max_x += padding;
        max_y += padding;

        let bounds = [min_x, min_y, max_x, max_y];

        // Create triangles
        for chunk in indices.chunks(3) {
            if chunk.len() == 3 {
                let v0 = vertices[chunk[0] as usize];
                let v1 = vertices[chunk[1] as usize];
                let v2 = vertices[chunk[2] as usize];
                triangles.push(Triangle::new(v0, v1, v2));
            }
        }

        let grid_nx = ((max_x - min_x) / cell_size).ceil() as usize + 1;
        let grid_ny = ((max_y - min_y) / cell_size).ceil() as usize + 1;

        let mut cells: HashMap<(usize, usize), Vec<usize>> = HashMap::new();

        // Insert triangles into grid cells
        for (tri_idx, tri) in triangles.iter().enumerate() {
            let x0 = ((tri.bbox_2d[0] - min_x) / cell_size).floor() as usize;
            let y0 = ((tri.bbox_2d[1] - min_y) / cell_size).floor() as usize;
            let x1 = ((tri.bbox_2d[2] - min_x) / cell_size).floor() as usize;
            let y1 = ((tri.bbox_2d[3] - min_y) / cell_size).floor() as usize;

            for iy in y0..=y1.min(grid_ny - 1) {
                for ix in x0..=x1.min(grid_nx - 1) {
                    cells.entry((ix, iy)).or_default().push(tri_idx);
                }
            }
        }

        Self {
            triangles,
            cell_size,
            bounds,
            grid_nx,
            grid_ny,
            cells,
        }
    }

    /// Create from flat vertex array (interleaved x, y, z).
    pub fn from_flat_vertices(vertices: &[f64], indices: &[u32], cell_size: f64) -> Self {
        let verts: Vec<[f64; 3]> = vertices.chunks(3).map(|c| [c[0], c[1], c[2]]).collect();
        Self::new(&verts, indices, cell_size)
    }

    /// Get triangles that may intersect a circle at (x, y) with given radius.
    pub fn query_circle(&self, x: f64, y: f64, radius: f64) -> Vec<usize> {
        let mut result = Vec::new();

        let x0 = ((x - radius - self.bounds[0]) / self.cell_size).floor() as isize;
        let y0 = ((y - radius - self.bounds[1]) / self.cell_size).floor() as isize;
        let x1 = ((x + radius - self.bounds[0]) / self.cell_size).floor() as isize;
        let y1 = ((y + radius - self.bounds[1]) / self.cell_size).floor() as isize;

        for iy in y0.max(0)..=y1.min(self.grid_ny as isize - 1) {
            for ix in x0.max(0)..=x1.min(self.grid_nx as isize - 1) {
                if let Some(indices) = self.cells.get(&(ix as usize, iy as usize)) {
                    for &idx in indices {
                        if !result.contains(&idx) {
                            result.push(idx);
                        }
                    }
                }
            }
        }

        result
    }

    /// Get a reference to a triangle by index.
    pub fn triangle(&self, idx: usize) -> &Triangle {
        &self.triangles[idx]
    }

    /// Get all triangles.
    pub fn triangles(&self) -> &[Triangle] {
        &self.triangles
    }

    /// Get the mesh bounds.
    pub fn bounds(&self) -> [f64; 4] {
        self.bounds
    }

    /// Get the Z extent of the mesh.
    pub fn z_bounds(&self) -> (f64, f64) {
        let mut min_z = f64::INFINITY;
        let mut max_z = f64::NEG_INFINITY;

        for tri in &self.triangles {
            for v in &tri.v {
                min_z = min_z.min(v[2]);
                max_z = max_z.max(v[2]);
            }
        }

        (min_z, max_z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_flat_triangle() -> MeshAccel {
        // Single triangle in XY plane at z=0
        let vertices = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [5.0, 10.0, 0.0]];
        let indices = [0, 1, 2];
        MeshAccel::new(&vertices, &indices, 5.0)
    }

    #[test]
    fn test_triangle_z_at_xy() {
        let tri = Triangle::new([0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [5.0, 10.0, 5.0]);

        // Point at origin should have z=0
        if let Some(z) = tri.z_at_xy(0.0, 0.0) {
            assert!(z.abs() < 1e-6);
        }
    }

    #[test]
    fn test_triangle_contains_xy() {
        let tri = Triangle::new([0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [5.0, 10.0, 0.0]);

        // Center should be inside
        assert!(tri.contains_xy(5.0, 3.0));

        // Outside points
        assert!(!tri.contains_xy(-1.0, 0.0));
        assert!(!tri.contains_xy(5.0, 15.0));
    }

    #[test]
    fn test_mesh_accel_query() {
        let accel = make_flat_triangle();

        // Query at center should find the triangle
        let result = accel.query_circle(5.0, 5.0, 1.0);
        assert!(!result.is_empty());

        // Query far away should be empty
        let result = accel.query_circle(100.0, 100.0, 1.0);
        assert!(result.is_empty());
    }
}
