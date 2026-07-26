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
use vcad_kernel_geom::{Surface, SurfaceKind};
use vcad_kernel_math::Point2;

/// Result of a ray-surface intersection (before trim testing).
#[derive(Debug, Clone, Copy)]
pub struct SurfaceHit {
    /// Parameter along the ray.
    pub t: f64,
    /// Surface parameter coordinates (u, v).
    pub uv: Point2,
}

/// Intersect a ray with a surface, returning all intersections sorted by t.
///
/// This dispatches to the appropriate intersector based on surface type.
pub fn intersect_surface(ray: &Ray, surface: &dyn Surface) -> Vec<SurfaceHit> {
    match surface.surface_type() {
        SurfaceKind::Plane => {
            if let Some(plane) = surface.as_any().downcast_ref::<vcad_kernel_geom::Plane>() {
                intersect_plane(ray, plane).into_iter().collect()
            } else {
                Vec::new()
            }
        }
        SurfaceKind::Cylinder => {
            if let Some(cyl) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::CylinderSurface>()
            {
                intersect_cylinder(ray, cyl)
            } else {
                Vec::new()
            }
        }
        SurfaceKind::Sphere => {
            if let Some(sph) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::SphereSurface>()
            {
                intersect_sphere(ray, sph)
            } else {
                Vec::new()
            }
        }
        SurfaceKind::Cone => {
            if let Some(cone) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::ConeSurface>()
            {
                intersect_cone(ray, cone)
            } else {
                Vec::new()
            }
        }
        SurfaceKind::Torus => {
            if let Some(torus) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::TorusSurface>()
            {
                intersect_torus(ray, torus)
            } else {
                Vec::new()
            }
        }
        SurfaceKind::Bilinear => {
            if let Some(bil) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::BilinearSurface>()
            {
                intersect_bilinear(ray, bil)
            } else {
                Vec::new()
            }
        }
        SurfaceKind::BSpline => {
            // B-spline surfaces use Newton iteration
            intersect_bspline(ray, surface)
        }
    }
}
