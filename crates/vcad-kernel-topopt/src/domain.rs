//! Design domain: a uniform voxel grid with an active-element mask.

use vcad_kernel_tessellate::TriangleMesh;

/// A uniform cubic voxel grid over an axis-aligned box, with a boolean
/// mask marking which voxels belong to the design domain.
///
/// Elements (voxels) are indexed `(iz * ny + iy) * nx + ix`; grid nodes
/// are indexed `(iz * (ny+1) + iy) * (nx+1) + ix`.
#[derive(Debug, Clone)]
pub struct Domain {
    /// Element counts along each axis.
    pub nx: usize,
    /// Element count along Y.
    pub ny: usize,
    /// Element count along Z.
    pub nz: usize,
    /// World position of grid node `(0, 0, 0)` in mm.
    pub origin: [f64; 3],
    /// Voxel edge length in mm (voxels are cubic).
    pub h: f64,
    /// Active mask, one entry per element.
    pub active: Vec<bool>,
}

impl Domain {
    /// Number of elements in the grid (active or not).
    pub fn num_elements(&self) -> usize {
        self.nx * self.ny * self.nz
    }

    /// Number of active (design) elements.
    pub fn num_active(&self) -> usize {
        self.active.iter().filter(|a| **a).count()
    }

    /// Number of grid nodes.
    pub fn num_nodes(&self) -> usize {
        (self.nx + 1) * (self.ny + 1) * (self.nz + 1)
    }

    /// Flat element index.
    #[inline]
    pub fn eidx(&self, ix: usize, iy: usize, iz: usize) -> usize {
        (iz * self.ny + iy) * self.nx + ix
    }

    /// Flat node index.
    #[inline]
    pub fn nidx(&self, ix: usize, iy: usize, iz: usize) -> usize {
        (iz * (self.ny + 1) + iy) * (self.nx + 1) + ix
    }

    /// World position of a grid node.
    #[inline]
    pub fn node_pos(&self, ix: usize, iy: usize, iz: usize) -> [f64; 3] {
        [
            self.origin[0] + ix as f64 * self.h,
            self.origin[1] + iy as f64 * self.h,
            self.origin[2] + iz as f64 * self.h,
        ]
    }

    /// Build a fully-active domain covering `[min, max]`, with `resolution`
    /// voxels along the longest axis.
    pub fn from_bbox(min: [f64; 3], max: [f64; 3], resolution: usize) -> Self {
        let size = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
        let longest = size[0].max(size[1]).max(size[2]).max(1e-9);
        let resolution = resolution.clamp(2, 256);
        let h = longest / resolution as f64;
        let nx = ((size[0] / h).round() as usize).max(1);
        let ny = ((size[1] / h).round() as usize).max(1);
        let nz = ((size[2] / h).round() as usize).max(1);
        Domain {
            nx,
            ny,
            nz,
            origin: min,
            h,
            active: vec![true; nx * ny * nz],
        }
    }

    /// Voxelize a closed triangle mesh into a design domain.
    ///
    /// Uses per-column ray parity along +Z: a voxel is active when its
    /// center lies inside the mesh. `resolution` is the voxel count along
    /// the longest bounding-box axis.
    pub fn from_mesh(mesh: &TriangleMesh, resolution: usize) -> Self {
        let nv = mesh.num_vertices();
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for i in 0..nv {
            for a in 0..3 {
                let v = mesh.vertices[3 * i + a] as f64;
                min[a] = min[a].min(v);
                max[a] = max[a].max(v);
            }
        }
        if nv == 0 {
            return Domain {
                nx: 0,
                ny: 0,
                nz: 0,
                origin: [0.0; 3],
                h: 1.0,
                active: Vec::new(),
            };
        }

        let mut domain = Self::from_bbox(min, max, resolution);
        domain.active = vec![false; domain.num_elements()];

        // Gather triangles as f64 for the parity test.
        let tris: Vec<[[f64; 3]; 3]> = (0..mesh.num_triangles())
            .map(|t| {
                let mut tri = [[0.0; 3]; 3];
                for (k, corner) in tri.iter_mut().enumerate() {
                    let vi = mesh.indices[3 * t + k] as usize;
                    for (a, c) in corner.iter_mut().enumerate() {
                        *c = mesh.vertices[3 * vi + a] as f64;
                    }
                }
                tri
            })
            .collect();

        let h = domain.h;
        // Small deterministic offset keeps ray origins away from vertices
        // and edges, where the parity test is ambiguous.
        let jitter = h * 0.01371;
        for iy in 0..domain.ny {
            for ix in 0..domain.nx {
                let cx = domain.origin[0] + (ix as f64 + 0.5) * h + jitter;
                let cy = domain.origin[1] + (iy as f64 + 0.5) * h + jitter * 0.618;

                // Collect z-crossings of the vertical line (cx, cy).
                let mut crossings: Vec<f64> = Vec::new();
                for tri in &tris {
                    if let Some(z) = ray_z_crossing(tri, cx, cy) {
                        crossings.push(z);
                    }
                }
                crossings.sort_by(|a, b| a.partial_cmp(b).unwrap());

                // Fill voxels between successive crossing pairs.
                let mut k = 0;
                while k + 1 < crossings.len() {
                    let z0 = crossings[k];
                    let z1 = crossings[k + 1];
                    k += 2;
                    let i0 = ((z0 - domain.origin[2]) / h - 0.5).ceil().max(0.0) as usize;
                    let i1f = ((z1 - domain.origin[2]) / h - 0.5).floor();
                    if i1f < 0.0 {
                        continue;
                    }
                    let i1 = (i1f as usize).min(domain.nz.saturating_sub(1));
                    for iz in i0..=i1 {
                        if iz < domain.nz {
                            let e = domain.eidx(ix, iy, iz);
                            domain.active[e] = true;
                        }
                    }
                }
            }
        }
        domain
    }
}

/// Z of the intersection between a vertical line at `(x, y)` and a triangle,
/// or `None` when the line misses the triangle.
fn ray_z_crossing(tri: &[[f64; 3]; 3], x: f64, y: f64) -> Option<f64> {
    let [a, b, c] = tri;
    // 2D barycentric test in the XY plane.
    let d = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
    if d.abs() < 1e-30 {
        return None; // Degenerate or vertical triangle.
    }
    let w0 = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (y - c[1])) / d;
    let w1 = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (y - c[1])) / d;
    let w2 = 1.0 - w0 - w1;
    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
        return None;
    }
    Some(w0 * a[2] + w1 * b[2] + w2 * c[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_cube_mesh(size: f32) -> TriangleMesh {
        // 12 triangles, outward normals not required for parity.
        let s = size;
        let verts: Vec<[f32; 3]> = vec![
            [0.0, 0.0, 0.0],
            [s, 0.0, 0.0],
            [s, s, 0.0],
            [0.0, s, 0.0],
            [0.0, 0.0, s],
            [s, 0.0, s],
            [s, s, s],
            [0.0, s, s],
        ];
        let quads = [
            [0, 3, 2, 1], // bottom
            [4, 5, 6, 7], // top
            [0, 1, 5, 4], // front
            [2, 3, 7, 6], // back
            [1, 2, 6, 5], // right
            [3, 0, 4, 7], // left
        ];
        let mut mesh = TriangleMesh::new();
        for v in &verts {
            mesh.vertices.extend_from_slice(v);
        }
        for q in &quads {
            mesh.indices
                .extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
        }
        mesh
    }

    #[test]
    fn bbox_domain_dims() {
        let d = Domain::from_bbox([0.0, 0.0, 0.0], [40.0, 20.0, 10.0], 40);
        assert_eq!((d.nx, d.ny, d.nz), (40, 20, 10));
        assert!((d.h - 1.0).abs() < 1e-12);
        assert_eq!(d.num_active(), d.num_elements());
    }

    #[test]
    fn voxelize_cube_fills_interior() {
        let mesh = unit_cube_mesh(10.0);
        let d = Domain::from_mesh(&mesh, 10);
        // Nearly all voxels of the cube should be active.
        let frac = d.num_active() as f64 / d.num_elements() as f64;
        assert!(frac > 0.9, "active fraction {frac} too low");
    }

    #[test]
    fn voxelize_respects_bounds() {
        let mesh = unit_cube_mesh(8.0);
        let d = Domain::from_mesh(&mesh, 8);
        assert_eq!((d.nx, d.ny, d.nz), (8, 8, 8));
        assert!((d.h - 1.0).abs() < 1e-9);
    }
}
