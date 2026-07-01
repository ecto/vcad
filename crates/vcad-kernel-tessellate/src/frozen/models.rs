//! Parametric models used by the differentiable-seam milestones.
//!
//! Each model implements [`ParametricModel`](super::ParametricModel): a
//! `build(θ)` that produces the concrete [`GeometryStore`] and a
//! θ-independent [`FrozenTessellation`]. They are deliberately small and
//! analytic — the point is to validate the *framework*, not the geometry.

use super::{FrozenTessellation, ParametricModel, SampleAddr};
use std::f64::consts::PI;
use vcad_kernel_geom::{CylinderSurface, GeometryStore, Plane, SurfaceSeed};
use vcad_kernel_math::{Point3, Vec3};

/// **M0/M1 model.** A rectangular slab `[0,sx] × [0,sy]` extruded to height
/// `θ` along `+z`. The parameter `θ` is the extrude distance; the top face
/// slides along `+z`, the bottom face is frozen.
///
/// `V(θ) = sx·sy·θ`, so `dV/dθ = sx·sy` in closed form.
#[derive(Debug, Clone)]
pub struct ExtrudedBox {
    sx: f64,
    sy: f64,
    height: f64,
}

impl ExtrudedBox {
    /// A slab of base `[sx, sy]` at nominal `height`.
    pub fn new(base: [f64; 2], height: f64) -> Self {
        Self {
            sx: base[0],
            sy: base[1],
            height,
        }
    }

    /// Closed-form `dV/dθ = sx·sy`.
    pub fn analytic_dvol(&self) -> f64 {
        self.sx * self.sy
    }
}

impl ParametricModel for ExtrudedBox {
    fn build(&self, theta: f64) -> GeometryStore {
        let mut store = GeometryStore::new();
        // surface 0: bottom plane z=0, outward normal -z.
        store.add_surface(Box::new(Plane::new(
            Point3::new(0.0, 0.0, 0.0),
            Vec3::x(),
            Vec3::y(),
        )));
        // surface 1: top plane z=theta, outward normal +z (slides with theta).
        store.add_surface(Box::new(Plane::new(
            Point3::new(0.0, 0.0, theta),
            Vec3::x(),
            Vec3::y(),
        )));
        store
    }

    fn tessellation(&self) -> FrozenTessellation {
        let (sx, sy) = (self.sx, self.sy);
        // 8 corners: bottom (surface 0) then top (surface 1). Plane uv == xy.
        let nodes = vec![
            SampleAddr {
                surface_index: 0,
                u: 0.0,
                v: 0.0,
            }, // 0
            SampleAddr {
                surface_index: 0,
                u: sx,
                v: 0.0,
            }, // 1
            SampleAddr {
                surface_index: 0,
                u: sx,
                v: sy,
            }, // 2
            SampleAddr {
                surface_index: 0,
                u: 0.0,
                v: sy,
            }, // 3
            SampleAddr {
                surface_index: 1,
                u: 0.0,
                v: 0.0,
            }, // 4
            SampleAddr {
                surface_index: 1,
                u: sx,
                v: 0.0,
            }, // 5
            SampleAddr {
                surface_index: 1,
                u: sx,
                v: sy,
            }, // 6
            SampleAddr {
                surface_index: 1,
                u: 0.0,
                v: sy,
            }, // 7
        ];
        // Outward-wound triangulation of the box surface.
        let tris = vec![
            // bottom (normal -z)
            [0, 2, 1],
            [0, 3, 2],
            // top (normal +z)
            [4, 5, 6],
            [4, 6, 7],
            // front y=0 (normal -y)
            [0, 1, 5],
            [0, 5, 4],
            // right x=sx (normal +x)
            [1, 2, 6],
            [1, 6, 5],
            // back y=sy (normal +y)
            [2, 3, 7],
            [2, 7, 6],
            // left x=0 (normal -x)
            [3, 0, 4],
            [3, 4, 7],
        ];
        // Seeds: bottom frozen, top translates along +z at unit rate.
        let seeds = vec![
            SurfaceSeed::Frozen,
            SurfaceSeed::PlaneTranslate { rate: Vec3::z() },
        ];
        FrozenTessellation { nodes, tris, seeds }
    }

    fn theta0(&self) -> f64 {
        self.height
    }
}

/// **M2 model.** A square block `[−a,a]²` of height `t` with a coaxial
/// through-hole of radius `θ = r` (a cylinder subtracted along `z`).
///
/// `V(r) = A_outer·t − π r² t`, so the continuous `dV/dr = −2π r t`. The hole
/// wall (a cylinder) moves with `r` (Pillar 2 via lift); the top/bottom rim is
/// the moving trim boundary `{on plane} ∩ {on cylinder(r)}` — sampled here on
/// the cylinder surface at the face height, so its sensitivity is exact.
///
/// The circular hole is discretized into `segments` sectors; the mesh's
/// `dV/dr` therefore equals `−t·N·r·sin(2π/N)`, which approaches the continuous
/// `−2π r t` as `O((2π/N)²)`.
#[derive(Debug, Clone)]
pub struct BlockWithHole {
    half: f64,
    thickness: f64,
    radius: f64,
    segments: usize,
}

impl BlockWithHole {
    /// A block of half-width `half` and height `thickness` with a hole of
    /// nominal `radius`, discretized into `segments` sectors.
    pub fn new(half: f64, thickness: f64, radius: f64, segments: usize) -> Self {
        assert!(segments >= 3);
        assert!(radius < half, "hole must fit inside the block");
        Self {
            half,
            thickness,
            radius,
            segments,
        }
    }

    /// Continuous closed-form `dV/dr = −2π r t`.
    pub fn analytic_dvol_continuous(&self) -> f64 {
        -2.0 * PI * self.radius * self.thickness
    }

    /// Discrete closed-form `dV/dr = −t·N·r·sin(2π/N)` for the polygonal hole.
    pub fn analytic_dvol_discrete(&self) -> f64 {
        let n = self.segments as f64;
        -self.thickness * n * self.radius * (2.0 * PI / n).sin()
    }

    fn angle(&self, i: usize) -> f64 {
        2.0 * PI * (i as f64) / (self.segments as f64)
    }

    /// Outer square-boundary radius at angle `phi`.
    fn outer_radius(&self, phi: f64) -> f64 {
        self.half / phi.cos().abs().max(phi.sin().abs())
    }
}

impl ParametricModel for BlockWithHole {
    fn build(&self, theta: f64) -> GeometryStore {
        let t = self.thickness;
        let mut store = GeometryStore::new();
        // 0: bottom plane z=0.
        store.add_surface(Box::new(Plane::new(
            Point3::new(0.0, 0.0, 0.0),
            Vec3::x(),
            Vec3::y(),
        )));
        // 1: top plane z=t.
        store.add_surface(Box::new(Plane::new(
            Point3::new(0.0, 0.0, t),
            Vec3::x(),
            Vec3::y(),
        )));
        // 2: hole cylinder, radius = theta, axis +z at origin.
        store.add_surface(Box::new(CylinderSurface::new(theta)));
        store
    }

    fn tessellation(&self) -> FrozenTessellation {
        let n = self.segments;
        let t = self.thickness;
        // Node blocks (each length n): outer_bottom, outer_top, inner_bottom, inner_top.
        let ob = 0;
        let ot = n;
        let ib = 2 * n;
        let it = 3 * n;

        let mut nodes = Vec::with_capacity(4 * n);
        // outer rings on the planes (frozen); uv on plane == xy.
        for i in 0..n {
            let phi = self.angle(i);
            let r = self.outer_radius(phi);
            let (x, y) = (r * phi.cos(), r * phi.sin());
            nodes.push(SampleAddr {
                surface_index: 0,
                u: x,
                v: y,
            }); // outer_bottom
        }
        for i in 0..n {
            let phi = self.angle(i);
            let r = self.outer_radius(phi);
            let (x, y) = (r * phi.cos(), r * phi.sin());
            nodes.push(SampleAddr {
                surface_index: 1,
                u: x,
                v: y,
            }); // outer_top
        }
        // inner rings on the cylinder (move with r); uv = (phi, z).
        for i in 0..n {
            nodes.push(SampleAddr {
                surface_index: 2,
                u: self.angle(i),
                v: 0.0,
            }); // inner_bottom
        }
        for i in 0..n {
            nodes.push(SampleAddr {
                surface_index: 2,
                u: self.angle(i),
                v: t,
            }); // inner_top
        }

        let mut tris: Vec<[u32; 3]> = Vec::new();
        let idx = |base: usize, i: usize| (base + (i % n)) as u32;
        for i in 0..n {
            let j = (i + 1) % n;
            // Top annulus (normal +z): outer higher-radius, inner lower. CCW from above.
            tris.push([idx(it, i), idx(ot, i), idx(ot, j)]);
            tris.push([idx(it, i), idx(ot, j), idx(it, j)]);
            // Bottom annulus (normal -z): reverse winding.
            tris.push([idx(ib, i), idx(ob, j), idx(ob, i)]);
            tris.push([idx(ib, i), idx(ib, j), idx(ob, j)]);
            // Outer wall (normal outward, +radial): from bottom to top.
            tris.push([idx(ob, i), idx(ob, j), idx(ot, j)]);
            tris.push([idx(ob, i), idx(ot, j), idx(ot, i)]);
            // Inner wall (hole; normal toward axis): reverse of outer.
            tris.push([idx(ib, i), idx(it, j), idx(ib, j)]);
            tris.push([idx(ib, i), idx(it, i), idx(it, j)]);
        }

        let seeds = vec![
            SurfaceSeed::Frozen,         // bottom plane
            SurfaceSeed::Frozen,         // top plane
            SurfaceSeed::CylinderRadius, // hole cylinder
        ];
        FrozenTessellation { nodes, tris, seeds }
    }

    fn theta0(&self) -> f64 {
        self.radius
    }
}
