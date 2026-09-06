//! Ray-surface intersection algorithms.
//!
//! Each surface type has a dedicated intersector that computes exact
//! intersection points and surface parameters.

mod bilinear;
mod bspline;
mod cone;
mod cylinder;
mod plane;
mod sphere;
mod torus;
mod triangle;

pub use bilinear::intersect_bilinear;
pub use bspline::intersect_bspline;
pub use cone::intersect_cone;
pub use cylinder::intersect_cylinder;
pub use plane::intersect_plane;
pub use sphere::intersect_sphere;
pub use torus::intersect_torus;
pub use triangle::{intersect_triangle, TriangleHit};

use crate::Ray;
use smallvec::SmallVec;
use vcad_kernel_geom::{Surface, SurfaceKind};
use vcad_kernel_math::{Point2, Vec3};

/// Hits from one ray-surface test, sorted by `t`.
///
/// Inline capacity 4 covers every analytic surface exactly — a torus is the
/// worst case at four roots — so the hot path never touches the allocator.
/// A B-spline can in principle exceed it and spills to the heap, which is
/// the right trade: that case is already dominated by Newton iteration.
pub type SurfaceHits = SmallVec<[SurfaceHit; 4]>;

/// Result of a ray-surface intersection (before trim testing).
#[derive(Debug, Clone, Copy)]
pub struct SurfaceHit {
    /// Parameter along the ray.
    pub t: f64,
    /// Surface parameter coordinates (u, v).
    pub uv: Point2,
}

/// The surface tangent `dP/du` at `uv`, when the parameterisation carries a
/// physically meaningful direction.
///
/// Analytic surfaces are parameterised the way the geometry actually runs, so
/// `dP/du` is the surface's grain rather than an arbitrary basis:
///
/// - **Plane** — the plane's own x-axis.
/// - **Cylinder / cone / sphere / torus** — the circumferential (around-the-
///   axis) direction. On a turned or bored feature that is the direction the
///   tool travelled, which is what makes anisotropic highlights read as
///   machined rather than rendered.
///
/// Returns `None` for surfaces whose parameterisation is an artefact of
/// fitting rather than of the geometry (B-spline, bilinear), and at
/// parametric degeneracies where `dP/du` vanishes — sphere poles, the cone
/// apex — so that shading falls back to an arbitrary tangent frame instead of
/// snapping to a garbage direction.
pub fn surface_tangent(surface: &dyn Surface, uv: Point2) -> Option<Vec3> {
    match surface.surface_type() {
        SurfaceKind::Plane
        | SurfaceKind::Cylinder
        | SurfaceKind::Cone
        | SurfaceKind::Sphere
        | SurfaceKind::Torus => {
            let d = surface.d_du(uv);
            // Degenerate at poles/apex: length collapses to zero there.
            if d.norm() > 1e-9 {
                Some(d)
            } else {
                None
            }
        }
        SurfaceKind::Bilinear | SurfaceKind::BSpline => None,
    }
}

/// Intersect a ray with a surface, returning all intersections sorted by t.
///
/// This dispatches to the appropriate intersector based on surface type.
pub fn intersect_surface(ray: &Ray, surface: &dyn Surface) -> SurfaceHits {
    match surface.surface_type() {
        SurfaceKind::Plane => {
            if let Some(plane) = surface.as_any().downcast_ref::<vcad_kernel_geom::Plane>() {
                intersect_plane(ray, plane).into_iter().collect()
            } else {
                SurfaceHits::new()
            }
        }
        SurfaceKind::Cylinder => {
            if let Some(cyl) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::CylinderSurface>()
            {
                intersect_cylinder(ray, cyl)
            } else {
                SurfaceHits::new()
            }
        }
        SurfaceKind::Sphere => {
            if let Some(sph) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::SphereSurface>()
            {
                intersect_sphere(ray, sph)
            } else {
                SurfaceHits::new()
            }
        }
        SurfaceKind::Cone => {
            if let Some(cone) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::ConeSurface>()
            {
                intersect_cone(ray, cone)
            } else {
                SurfaceHits::new()
            }
        }
        SurfaceKind::Torus => {
            if let Some(torus) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::TorusSurface>()
            {
                intersect_torus(ray, torus)
            } else {
                SurfaceHits::new()
            }
        }
        SurfaceKind::Bilinear => {
            if let Some(bil) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::BilinearSurface>()
            {
                intersect_bilinear(ray, bil)
            } else {
                SurfaceHits::new()
            }
        }
        SurfaceKind::BSpline => {
            // B-spline surfaces use Newton iteration
            intersect_bspline(ray, surface)
        }
    }
}
