#![warn(missing_docs)]

//! Analytic surface and curve types for the vcad kernel.
//!
//! Provides trait-based abstractions for parametric surfaces and curves,
//! with concrete implementations for the common analytic types used in
//! B-rep CAD: planes, cylinders, cones, spheres, lines, and circles.

use std::any::Any;
use std::f64::consts::PI;
use vcad_kernel_math::{Dir3, Point2, Point3, Transform, Vec2, Vec3};

// =============================================================================
// Surface types
// =============================================================================

/// The kind of a surface (for match-based dispatch).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceKind {
    /// Infinite plane.
    Plane,
    /// Cylindrical surface (infinite extent along axis).
    Cylinder,
    /// Conical surface.
    Cone,
    /// Spherical surface.
    Sphere,
    /// Toroidal surface.
    Torus,
    /// B-spline or NURBS surface.
    BSpline,
    /// Bilinear patch (4-corner interpolation).
    Bilinear,
}

/// A parametric surface in 3D space.
pub trait Surface: Send + Sync + std::fmt::Debug {
    /// Evaluate the surface at parameter `(u, v)` to get a 3D point.
    fn evaluate(&self, uv: Point2) -> Point3;

    /// Surface normal at parameter `(u, v)`.
    fn normal(&self, uv: Point2) -> Dir3;

    /// Partial derivative with respect to u at `(u, v)`.
    fn d_du(&self, uv: Point2) -> Vec3;

    /// Partial derivative with respect to v at `(u, v)`.
    fn d_dv(&self, uv: Point2) -> Vec3;

    /// Parameter domain as `((u_min, u_max), (v_min, v_max))`.
    fn domain(&self) -> ((f64, f64), (f64, f64));

    /// The kind of this surface.
    fn surface_type(&self) -> SurfaceKind;

    /// Clone this surface into a boxed trait object.
    fn clone_box(&self) -> Box<dyn Surface>;

    /// Downcast to a concrete type via `Any`.
    fn as_any(&self) -> &dyn Any;

    /// Apply an affine transform to this surface, returning a new surface.
    fn transform(&self, t: &Transform) -> Box<dyn Surface>;

    /// Create an offset surface at the given signed distance.
    ///
    /// Positive distance offsets in the direction of the surface normal.
    /// Returns `None` if the offset is not supported or would degenerate
    /// (e.g., cylinder offset with radius <= 0).
    fn offset(&self, _distance: f64) -> Option<Box<dyn Surface>> {
        None
    }
}

impl Clone for Box<dyn Surface> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

// =============================================================================
// Plane
// =============================================================================

/// An infinite plane defined by an origin point and a coordinate frame.
///
/// Parameterization: `P(u, v) = origin + u * x_dir + v * y_dir`
#[derive(Debug, Clone)]
pub struct Plane {
    /// Origin point on the plane.
    pub origin: Point3,
    /// Unit vector along the u direction.
    pub x_dir: Dir3,
    /// Unit vector along the v direction.
    pub y_dir: Dir3,
    /// Unit normal (x_dir × y_dir).
    pub normal_dir: Dir3,
}

impl Plane {
    /// Create a plane from origin and two orthogonal direction vectors.
    /// The vectors do not need to be normalized.
    pub fn new(origin: Point3, x_dir: Vec3, y_dir: Vec3) -> Self {
        let x = Dir3::new_normalize(x_dir);
        let y = Dir3::new_normalize(y_dir);
        let n = Dir3::new_normalize(x_dir.cross(&y_dir));
        Self {
            origin,
            x_dir: x,
            y_dir: y,
            normal_dir: n,
        }
    }

    /// Create a plane from origin and normal. X/Y directions are chosen arbitrarily.
    pub fn from_normal(origin: Point3, normal: Vec3) -> Self {
        let n = Dir3::new_normalize(normal);
        // Pick an arbitrary perpendicular vector
        let arbitrary = if n.as_ref().x.abs() < 0.9 {
            Vec3::x()
        } else {
            Vec3::y()
        };
        let x = Dir3::new_normalize(arbitrary.cross(n.as_ref()));
        let y = Dir3::new_normalize(n.as_ref().cross(x.as_ref()));
        Self {
            origin,
            x_dir: x,
            y_dir: y,
            normal_dir: n,
        }
    }

    /// XY plane at the origin.
    pub fn xy() -> Self {
        Self::new(Point3::origin(), Vec3::x(), Vec3::y())
    }

    /// XZ plane at the origin.
    pub fn xz() -> Self {
        Self::new(Point3::origin(), Vec3::x(), Vec3::z())
    }

    /// YZ plane at the origin.
    pub fn yz() -> Self {
        Self::new(Point3::origin(), Vec3::y(), Vec3::z())
    }

    /// Project a 3D point onto this plane's (u, v) parameter space.
    pub fn project(&self, p: &Point3) -> Point2 {
        let d = p - self.origin;
        Point2::new(d.dot(self.x_dir.as_ref()), d.dot(self.y_dir.as_ref()))
    }

    /// Signed distance from a point to this plane.
    pub fn signed_distance(&self, p: &Point3) -> f64 {
        (p - self.origin).dot(self.normal_dir.as_ref())
    }
}

impl Surface for Plane {
    fn evaluate(&self, uv: Point2) -> Point3 {
        self.origin + uv.x * self.x_dir.as_ref() + uv.y * self.y_dir.as_ref()
    }

    fn normal(&self, _uv: Point2) -> Dir3 {
        self.normal_dir
    }

    fn d_du(&self, _uv: Point2) -> Vec3 {
        *self.x_dir.as_ref()
    }

    fn d_dv(&self, _uv: Point2) -> Vec3 {
        *self.y_dir.as_ref()
    }

    fn domain(&self) -> ((f64, f64), (f64, f64)) {
        ((-1e10, 1e10), (-1e10, 1e10))
    }

    fn surface_type(&self) -> SurfaceKind {
        SurfaceKind::Plane
    }

    fn clone_box(&self) -> Box<dyn Surface> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn transform(&self, t: &Transform) -> Box<dyn Surface> {
        let new_origin = t.apply_point(&self.origin);
        let new_x = t.apply_vec(self.x_dir.as_ref());
        let new_y = t.apply_vec(self.y_dir.as_ref());
        Box::new(Plane::new(new_origin, new_x, new_y))
    }

    fn offset(&self, distance: f64) -> Option<Box<dyn Surface>> {
        // Offset a plane by shifting origin along its normal.
        // Planes never degenerate, so this always succeeds.
        let new_origin = self.origin + distance * self.normal_dir.as_ref();
        Some(Box::new(Plane {
            origin: new_origin,
            x_dir: self.x_dir,
            y_dir: self.y_dir,
            normal_dir: self.normal_dir,
        }))
    }
}

// =============================================================================
// Cylinder
// =============================================================================

/// A cylindrical surface defined by an axis line and radius.
///
/// Parameterization: `P(u, v) = center + radius * (cos(u) * x_dir + sin(u) * y_dir) + v * axis`
///
/// Where `u ∈ [0, 2π)` is the angular parameter and `v` is the height along the axis.
#[derive(Debug, Clone)]
pub struct CylinderSurface {
    /// Center point at the base of the cylinder axis.
    pub center: Point3,
    /// Unit direction along the cylinder axis.
    pub axis: Dir3,
    /// Reference direction for u=0 (perpendicular to axis).
    pub ref_dir: Dir3,
    /// Radius of the cylinder.
    pub radius: f64,
}

impl CylinderSurface {
    /// Create a cylinder with axis along Z, centered at origin.
    pub fn new(radius: f64) -> Self {
        Self {
            center: Point3::origin(),
            axis: Dir3::new_normalize(Vec3::z()),
            ref_dir: Dir3::new_normalize(Vec3::x()),
            radius,
        }
    }

    /// Create a cylinder with a custom center and axis.
    pub fn with_axis(center: Point3, axis: Vec3, radius: f64) -> Self {
        let a = Dir3::new_normalize(axis);
        let arbitrary = if a.as_ref().x.abs() < 0.9 {
            Vec3::x()
        } else {
            Vec3::y()
        };
        let ref_dir = Dir3::new_normalize(arbitrary - arbitrary.dot(a.as_ref()) * a.as_ref());
        Self {
            center,
            axis: a,
            ref_dir,
            radius,
        }
    }

    /// Compute the y direction (perpendicular to axis and ref_dir).
    pub fn y_dir(&self) -> Vec3 {
        self.axis.as_ref().cross(self.ref_dir.as_ref())
    }
}

impl Surface for CylinderSurface {
    fn evaluate(&self, uv: Point2) -> Point3 {
        let (sin_u, cos_u) = uv.x.sin_cos();
        self.center
            + self.radius * (cos_u * self.ref_dir.as_ref() + sin_u * self.y_dir())
            + uv.y * self.axis.as_ref()
    }

    fn normal(&self, uv: Point2) -> Dir3 {
        let (sin_u, cos_u) = uv.x.sin_cos();
        Dir3::new_normalize(cos_u * self.ref_dir.as_ref() + sin_u * self.y_dir())
    }

    fn d_du(&self, uv: Point2) -> Vec3 {
        let (sin_u, cos_u) = uv.x.sin_cos();
        self.radius * (-sin_u * self.ref_dir.as_ref() + cos_u * self.y_dir())
    }

    fn d_dv(&self, _uv: Point2) -> Vec3 {
        *self.axis.as_ref()
    }

    fn domain(&self) -> ((f64, f64), (f64, f64)) {
        ((0.0, 2.0 * PI), (-1e10, 1e10))
    }

    fn surface_type(&self) -> SurfaceKind {
        SurfaceKind::Cylinder
    }

    fn clone_box(&self) -> Box<dyn Surface> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn transform(&self, t: &Transform) -> Box<dyn Surface> {
        let new_center = t.apply_point(&self.center);
        let new_axis = t.apply_vec(self.axis.as_ref());
        let new_ref = t.apply_vec(self.ref_dir.as_ref());
        // Scale factor affects radius — use the length of the transformed ref_dir
        let scale = new_ref.norm();
        Box::new(CylinderSurface {
            center: new_center,
            axis: Dir3::new_normalize(new_axis),
            ref_dir: Dir3::new_normalize(new_ref),
            radius: self.radius * scale,
        })
    }

    fn offset(&self, distance: f64) -> Option<Box<dyn Surface>> {
        // Offset a cylinder by changing the radius.
        // Positive distance = offset in normal direction (outward) = increase radius.
        let new_radius = self.radius + distance;
        if new_radius <= 0.0 {
            return None; // Degenerate: collapsed to a line or inverted
        }
        Some(Box::new(CylinderSurface {
            center: self.center,
            axis: self.axis,
            ref_dir: self.ref_dir,
            radius: new_radius,
        }))
    }
}

// =============================================================================
// Cone
// =============================================================================

/// A conical surface defined by an apex, axis, and half-angle.
///
/// Parameterization: `P(u, v) = apex + v * (cos(half_angle) * axis + sin(half_angle) * (cos(u) * x + sin(u) * y))`
///
/// Where `u ∈ [0, 2π)` is the angular parameter and `v ≥ 0` is the distance from apex along the cone.
#[derive(Debug, Clone)]
pub struct ConeSurface {
    /// Apex (tip) of the cone.
    pub apex: Point3,
    /// Unit direction along the cone axis (from apex toward base).
    pub axis: Dir3,
    /// Reference direction for u=0 (perpendicular to axis).
    pub ref_dir: Dir3,
    /// Half-angle of the cone in radians.
    pub half_angle: f64,
}

impl ConeSurface {
    /// Create a cone with apex at origin, axis along Z, with the given half-angle.
    pub fn new(half_angle: f64) -> Self {
        Self {
            apex: Point3::origin(),
            axis: Dir3::new_normalize(Vec3::z()),
            ref_dir: Dir3::new_normalize(Vec3::x()),
            half_angle,
        }
    }

    /// Create a cone from bottom radius, top radius, and height.
    /// The apex is calculated from the geometry.
    /// Returns `None` if the cone is actually a cylinder (radii equal).
    pub fn from_frustum(
        center: Point3,
        radius_bottom: f64,
        radius_top: f64,
        height: f64,
    ) -> Option<Self> {
        let dr = radius_bottom - radius_top;
        if dr.abs() < 1e-12 {
            return None; // cylinder, not a cone
        }
        let half_angle = (dr / height).abs().atan();
        let apex_z = if radius_bottom > radius_top {
            height * radius_bottom / dr
        } else {
            -height * radius_top / (radius_top - radius_bottom)
        };
        let apex = Point3::new(center.x, center.y, center.z + apex_z);
        let axis = if radius_bottom > radius_top {
            Dir3::new_normalize(-Vec3::z())
        } else {
            Dir3::new_normalize(Vec3::z())
        };
        Some(Self {
            apex,
            axis,
            ref_dir: Dir3::new_normalize(Vec3::x()),
            half_angle,
        })
    }

    fn y_dir(&self) -> Vec3 {
        self.axis.as_ref().cross(self.ref_dir.as_ref())
    }
}

impl Surface for ConeSurface {
    fn evaluate(&self, uv: Point2) -> Point3 {
        let (sin_u, cos_u) = uv.x.sin_cos();
        let ca = self.half_angle.cos();
        let sa = self.half_angle.sin();
        self.apex
            + uv.y
                * (ca * self.axis.as_ref()
                    + sa * (cos_u * self.ref_dir.as_ref() + sin_u * self.y_dir()))
    }

    fn normal(&self, uv: Point2) -> Dir3 {
        let (sin_u, cos_u) = uv.x.sin_cos();
        let ca = self.half_angle.cos();
        let sa = self.half_angle.sin();
        // Normal is perpendicular to the cone surface
        let radial = cos_u * self.ref_dir.as_ref() + sin_u * self.y_dir();
        Dir3::new_normalize(ca * radial - sa * self.axis.as_ref())
    }

    fn d_du(&self, uv: Point2) -> Vec3 {
        let (sin_u, cos_u) = uv.x.sin_cos();
        let sa = self.half_angle.sin();
        uv.y * sa * (-sin_u * self.ref_dir.as_ref() + cos_u * self.y_dir())
    }

    fn d_dv(&self, uv: Point2) -> Vec3 {
        let (sin_u, cos_u) = uv.x.sin_cos();
        let ca = self.half_angle.cos();
        let sa = self.half_angle.sin();
        ca * self.axis.as_ref() + sa * (cos_u * self.ref_dir.as_ref() + sin_u * self.y_dir())
    }

    fn domain(&self) -> ((f64, f64), (f64, f64)) {
        ((0.0, 2.0 * PI), (0.0, 1e10))
    }

    fn surface_type(&self) -> SurfaceKind {
        SurfaceKind::Cone
    }

    fn clone_box(&self) -> Box<dyn Surface> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn transform(&self, t: &Transform) -> Box<dyn Surface> {
        let new_apex = t.apply_point(&self.apex);
        let new_axis = t.apply_vec(self.axis.as_ref());
        let new_ref = t.apply_vec(self.ref_dir.as_ref());
        // Half-angle is preserved under uniform transforms.
        // For non-uniform scaling, this is an approximation.
        Box::new(ConeSurface {
            apex: new_apex,
            axis: Dir3::new_normalize(new_axis),
            ref_dir: Dir3::new_normalize(new_ref),
            half_angle: self.half_angle,
        })
    }

    fn offset(&self, distance: f64) -> Option<Box<dyn Surface>> {
        // Offsetting a cone shifts the apex along the axis.
        // The offset distance along the axis is d / sin(half_angle).
        let sin_a = self.half_angle.sin();
        if sin_a.abs() < 1e-15 {
            return None; // Degenerate cone (zero half-angle = line)
        }
        let apex_shift = distance / sin_a;
        let new_apex = self.apex - apex_shift * self.axis.as_ref();
        Some(Box::new(ConeSurface {
            apex: new_apex,
            axis: self.axis,
            ref_dir: self.ref_dir,
            half_angle: self.half_angle,
        }))
    }
}

// =============================================================================
// Sphere
// =============================================================================

/// A spherical surface defined by center and radius.
///
/// Parameterization: `P(u, v) = center + radius * (cos(v) * (cos(u) * x + sin(u) * y) + sin(v) * z)`
///
/// Where `u ∈ [0, 2π)` is longitude and `v ∈ [-π/2, π/2]` is latitude.
#[derive(Debug, Clone)]
pub struct SphereSurface {
    /// Center of the sphere.
    pub center: Point3,
    /// Radius of the sphere.
    pub radius: f64,
    /// Reference direction for u=0 (perpendicular to axis).
    pub ref_dir: Dir3,
    /// Axis direction (north pole).
    pub axis: Dir3,
}

impl SphereSurface {
    /// Create a sphere centered at origin with the given radius.
    pub fn new(radius: f64) -> Self {
        Self {
            center: Point3::origin(),
            radius,
            ref_dir: Dir3::new_normalize(Vec3::x()),
            axis: Dir3::new_normalize(Vec3::z()),
        }
    }

    /// Create a sphere with a custom center.
    pub fn with_center(center: Point3, radius: f64) -> Self {
        Self {
            center,
            radius,
            ref_dir: Dir3::new_normalize(Vec3::x()),
            axis: Dir3::new_normalize(Vec3::z()),
        }
    }

    /// Compute the y direction (perpendicular to axis and ref_dir).
    pub fn y_dir(&self) -> Vec3 {
        self.axis.as_ref().cross(self.ref_dir.as_ref())
    }
}

impl Surface for SphereSurface {
    fn evaluate(&self, uv: Point2) -> Point3 {
        let (sin_u, cos_u) = uv.x.sin_cos();
        let (sin_v, cos_v) = uv.y.sin_cos();
        self.center
            + self.radius
                * (cos_v * (cos_u * self.ref_dir.as_ref() + sin_u * self.y_dir())
                    + sin_v * self.axis.as_ref())
    }

    fn normal(&self, uv: Point2) -> Dir3 {
        let (sin_u, cos_u) = uv.x.sin_cos();
        let (sin_v, cos_v) = uv.y.sin_cos();
        Dir3::new_normalize(
            cos_v * (cos_u * self.ref_dir.as_ref() + sin_u * self.y_dir())
                + sin_v * self.axis.as_ref(),
        )
    }

    fn d_du(&self, uv: Point2) -> Vec3 {
        let (sin_u, cos_u) = uv.x.sin_cos();
        let cos_v = uv.y.cos();
        self.radius * cos_v * (-sin_u * self.ref_dir.as_ref() + cos_u * self.y_dir())
    }

    fn d_dv(&self, uv: Point2) -> Vec3 {
        let (sin_u, cos_u) = uv.x.sin_cos();
        let (sin_v, cos_v) = uv.y.sin_cos();
        self.radius
            * (-sin_v * (cos_u * self.ref_dir.as_ref() + sin_u * self.y_dir())
                + cos_v * self.axis.as_ref())
    }

    fn domain(&self) -> ((f64, f64), (f64, f64)) {
        ((0.0, 2.0 * PI), (-PI / 2.0, PI / 2.0))
    }

    fn surface_type(&self) -> SurfaceKind {
        SurfaceKind::Sphere
    }

    fn clone_box(&self) -> Box<dyn Surface> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn transform(&self, t: &Transform) -> Box<dyn Surface> {
        let new_center = t.apply_point(&self.center);
        let new_ref = t.apply_vec(self.ref_dir.as_ref());
        let new_axis = t.apply_vec(self.axis.as_ref());
        // Scale factor affects radius — use the length of the transformed ref_dir
        let scale = new_ref.norm();
        Box::new(SphereSurface {
            center: new_center,
            radius: self.radius * scale,
            ref_dir: Dir3::new_normalize(new_ref),
            axis: Dir3::new_normalize(new_axis),
        })
    }

    fn offset(&self, distance: f64) -> Option<Box<dyn Surface>> {
        // Offset a sphere by changing the radius.
        let new_radius = self.radius + distance;
        if new_radius <= 0.0 {
            return None; // Degenerate: collapsed to a point or inverted
        }
        Some(Box::new(SphereSurface {
            center: self.center,
            radius: new_radius,
            ref_dir: self.ref_dir,
            axis: self.axis,
        }))
    }
}

// =============================================================================
// Torus
// =============================================================================

/// A toroidal surface defined by center, axis, and two radii.
///
/// Parameterization:
/// ```text
/// P(u, v) = center + (R + r·cos(v))·(cos(u)·ref_dir + sin(u)·y_dir) + r·sin(v)·axis
/// ```
///
/// Where:
/// - `R` = major radius (center to tube center)
/// - `r` = minor radius (tube radius)
/// - `u ∈ [0, 2π)` is the toroidal angle (around the main axis)
/// - `v ∈ [0, 2π)` is the poloidal angle (around the tube)
#[derive(Debug, Clone)]
pub struct TorusSurface {
    /// Center of the torus.
    pub center: Point3,
    /// Unit direction of the torus axis (perpendicular to the plane of the ring).
    pub axis: Dir3,
    /// Reference direction for u=0 (perpendicular to axis).
    pub ref_dir: Dir3,
    /// Major radius: distance from center to tube center.
    pub major_radius: f64,
    /// Minor radius: radius of the tube.
    pub minor_radius: f64,
}

impl TorusSurface {
    /// Create a torus centered at origin with axis along Z.
    pub fn new(major_radius: f64, minor_radius: f64) -> Self {
        Self {
            center: Point3::origin(),
            axis: Dir3::new_normalize(Vec3::z()),
            ref_dir: Dir3::new_normalize(Vec3::x()),
            major_radius,
            minor_radius,
        }
    }

    /// Create a torus with a custom center and axis.
    pub fn with_axis(center: Point3, axis: Vec3, major_radius: f64, minor_radius: f64) -> Self {
        let a = Dir3::new_normalize(axis);
        let arbitrary = if a.as_ref().x.abs() < 0.9 {
            Vec3::x()
        } else {
            Vec3::y()
        };
        let ref_dir = Dir3::new_normalize(arbitrary - arbitrary.dot(a.as_ref()) * a.as_ref());
        Self {
            center,
            axis: a,
            ref_dir,
            major_radius,
            minor_radius,
        }
    }

    fn y_dir(&self) -> Vec3 {
        self.axis.as_ref().cross(self.ref_dir.as_ref())
    }
}

impl Surface for TorusSurface {
    fn evaluate(&self, uv: Point2) -> Point3 {
        let (sin_u, cos_u) = uv.x.sin_cos();
        let (sin_v, cos_v) = uv.y.sin_cos();

        // Point on the tube centerline (at angle u around the main axis)
        let tube_center_dir = cos_u * self.ref_dir.as_ref() + sin_u * self.y_dir();

        self.center
            + (self.major_radius + self.minor_radius * cos_v) * tube_center_dir
            + self.minor_radius * sin_v * self.axis.as_ref()
    }

    fn normal(&self, uv: Point2) -> Dir3 {
        let (sin_u, cos_u) = uv.x.sin_cos();
        let (sin_v, cos_v) = uv.y.sin_cos();

        let tube_center_dir = cos_u * self.ref_dir.as_ref() + sin_u * self.y_dir();

        // Normal points outward from the tube center
        let n = cos_v * tube_center_dir + sin_v * self.axis.as_ref();
        Dir3::new_normalize(n)
    }

    fn d_du(&self, uv: Point2) -> Vec3 {
        let (sin_u, cos_u) = uv.x.sin_cos();
        let cos_v = uv.y.cos();

        // d/du of tube_center_dir = -sin_u * ref_dir + cos_u * y_dir
        let d_tube_center_dir = -sin_u * self.ref_dir.as_ref() + cos_u * self.y_dir();

        (self.major_radius + self.minor_radius * cos_v) * d_tube_center_dir
    }

    fn d_dv(&self, uv: Point2) -> Vec3 {
        let (sin_u, cos_u) = uv.x.sin_cos();
        let (sin_v, cos_v) = uv.y.sin_cos();

        let tube_center_dir = cos_u * self.ref_dir.as_ref() + sin_u * self.y_dir();

        // d/dv: -r·sin(v)·tube_center_dir + r·cos(v)·axis
        -self.minor_radius * sin_v * tube_center_dir + self.minor_radius * cos_v * self.axis.as_ref()
    }

    fn domain(&self) -> ((f64, f64), (f64, f64)) {
        ((0.0, 2.0 * PI), (0.0, 2.0 * PI))
    }

    fn surface_type(&self) -> SurfaceKind {
        SurfaceKind::Torus
    }

    fn clone_box(&self) -> Box<dyn Surface> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn transform(&self, t: &Transform) -> Box<dyn Surface> {
        let new_center = t.apply_point(&self.center);
        let new_axis = t.apply_vec(self.axis.as_ref());
        let new_ref = t.apply_vec(self.ref_dir.as_ref());
        // Scale factor affects radii — use the length of the transformed ref_dir
        let scale = new_ref.norm();
        Box::new(TorusSurface {
            center: new_center,
            axis: Dir3::new_normalize(new_axis),
            ref_dir: Dir3::new_normalize(new_ref),
            major_radius: self.major_radius * scale,
            minor_radius: self.minor_radius * scale,
        })
    }

    fn offset(&self, distance: f64) -> Option<Box<dyn Surface>> {
        // Offset a torus by changing the minor radius.
        let new_minor = self.minor_radius + distance;
        if new_minor <= 0.0 {
            return None; // Degenerate: tube collapsed
        }
        Some(Box::new(TorusSurface {
            center: self.center,
            axis: self.axis,
            ref_dir: self.ref_dir,
            major_radius: self.major_radius,
            minor_radius: new_minor,
        }))
    }
}

// =============================================================================
// BilinearSurface
// =============================================================================

/// A bilinear patch defined by four corner points with optional corner normals.
///
/// Parameterization:
/// ```text
/// P(u, v) = (1-u)(1-v)*p00 + u*(1-v)*p10 + (1-u)*v*p01 + u*v*p11
/// ```
///
/// When corner_normals are provided, the `normal()` method bilinearly interpolates
/// them instead of computing the geometric normal. This enables smooth shading
/// for swept surfaces where the intended normal differs from the flat quad normal.
#[derive(Debug, Clone)]
pub struct BilinearSurface {
    /// Corner at (u=0, v=0).
    pub p00: Point3,
    /// Corner at (u=1, v=0).
    pub p10: Point3,
    /// Corner at (u=0, v=1).
    pub p01: Point3,
    /// Corner at (u=1, v=1).
    pub p11: Point3,
    /// Optional corner normals for smooth shading [n00, n10, n01, n11].
    pub corner_normals: Option<[Dir3; 4]>,
}

impl BilinearSurface {
    /// Create a bilinear surface from four corner points.
    pub fn new(p00: Point3, p10: Point3, p01: Point3, p11: Point3) -> Self {
        Self {
            p00,
            p10,
            p01,
            p11,
            corner_normals: None,
        }
    }

    /// Create a bilinear surface with explicit corner normals for smooth shading.
    #[allow(clippy::too_many_arguments)]
    pub fn with_normals(
        p00: Point3,
        p10: Point3,
        p01: Point3,
        p11: Point3,
        n00: Dir3,
        n10: Dir3,
        n01: Dir3,
        n11: Dir3,
    ) -> Self {
        Self {
            p00,
            p10,
            p01,
            p11,
            corner_normals: Some([n00, n10, n01, n11]),
        }
    }

    /// Check if this bilinear surface is planar (all 4 points coplanar).
    pub fn is_planar(&self) -> bool {
        let e1 = self.p10 - self.p00;
        let e2 = self.p01 - self.p00;
        let n = e1.cross(&e2);
        if n.norm() < 1e-12 {
            return true;
        }
        let d = self.p11 - self.p00;
        (d.dot(&n).abs() / n.norm()) < 1e-10
    }
}

impl Surface for BilinearSurface {
    fn evaluate(&self, uv: Point2) -> Point3 {
        let u = uv.x;
        let v = uv.y;
        let u1 = 1.0 - u;
        let v1 = 1.0 - v;
        Point3::new(
            u1 * v1 * self.p00.x + u * v1 * self.p10.x + u1 * v * self.p01.x + u * v * self.p11.x,
            u1 * v1 * self.p00.y + u * v1 * self.p10.y + u1 * v * self.p01.y + u * v * self.p11.y,
            u1 * v1 * self.p00.z + u * v1 * self.p10.z + u1 * v * self.p01.z + u * v * self.p11.z,
        )
    }

    fn normal(&self, uv: Point2) -> Dir3 {
        // If corner normals provided, bilinearly interpolate them
        if let Some([n00, n10, n01, n11]) = &self.corner_normals {
            let u = uv.x;
            let v = uv.y;
            let u1 = 1.0 - u;
            let v1 = 1.0 - v;
            let nx = u1 * v1 * n00.x + u * v1 * n10.x + u1 * v * n01.x + u * v * n11.x;
            let ny = u1 * v1 * n00.y + u * v1 * n10.y + u1 * v * n01.y + u * v * n11.y;
            let nz = u1 * v1 * n00.z + u * v1 * n10.z + u1 * v * n01.z + u * v * n11.z;
            return Dir3::new_normalize(Vec3::new(nx, ny, nz));
        }
        // Otherwise compute from cross product
        let du = self.d_du(uv);
        let dv = self.d_dv(uv);
        let n = du.cross(&dv);
        if n.norm() < 1e-12 {
            Dir3::new_normalize(Vec3::z())
        } else {
            Dir3::new_normalize(n)
        }
    }

    fn d_du(&self, uv: Point2) -> Vec3 {
        let v = uv.y;
        let v1 = 1.0 - v;
        Vec3::new(
            -v1 * self.p00.x + v1 * self.p10.x - v * self.p01.x + v * self.p11.x,
            -v1 * self.p00.y + v1 * self.p10.y - v * self.p01.y + v * self.p11.y,
            -v1 * self.p00.z + v1 * self.p10.z - v * self.p01.z + v * self.p11.z,
        )
    }

    fn d_dv(&self, uv: Point2) -> Vec3 {
        let u = uv.x;
        let u1 = 1.0 - u;
        Vec3::new(
            -u1 * self.p00.x - u * self.p10.x + u1 * self.p01.x + u * self.p11.x,
            -u1 * self.p00.y - u * self.p10.y + u1 * self.p01.y + u * self.p11.y,
            -u1 * self.p00.z - u * self.p10.z + u1 * self.p01.z + u * self.p11.z,
        )
    }

    fn domain(&self) -> ((f64, f64), (f64, f64)) {
        ((0.0, 1.0), (0.0, 1.0))
    }

    fn surface_type(&self) -> SurfaceKind {
        SurfaceKind::Bilinear
    }

    fn clone_box(&self) -> Box<dyn Surface> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn transform(&self, t: &Transform) -> Box<dyn Surface> {
        Box::new(BilinearSurface {
            p00: t.apply_point(&self.p00),
            p10: t.apply_point(&self.p10),
            p01: t.apply_point(&self.p01),
            p11: t.apply_point(&self.p11),
            corner_normals: self
                .corner_normals
                .map(|normals| normals.map(|n| Dir3::new_normalize(t.apply_vec(&n.into_inner())))),
        })
    }
}

// =============================================================================
// Curve types
// =============================================================================

/// The kind of a curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveKind {
    /// Straight line.
    Line,
    /// Circle.
    Circle,
}

/// A parametric curve in 3D space.
pub trait Curve3d: Send + Sync + std::fmt::Debug {
    /// Evaluate the curve at parameter `t` to get a 3D point.
    fn evaluate(&self, t: f64) -> Point3;

    /// Tangent vector at parameter `t`.
    fn tangent(&self, t: f64) -> Vec3;

    /// Parameter domain `(t_min, t_max)`.
    fn domain(&self) -> (f64, f64);

    /// The kind of this curve.
    fn curve_type(&self) -> CurveKind;

    /// Clone into a boxed trait object.
    fn clone_box(&self) -> Box<dyn Curve3d>;

    /// Suggested number of segments for smooth tessellation.
    ///
    /// Override this for curves with high curvature (like helices).
    /// Default returns 32.
    fn suggested_segments(&self) -> usize {
        32
    }
}

impl Clone for Box<dyn Curve3d> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

/// A 2D parametric curve (for trim curves in surface parameter space).
pub trait Curve2d: Send + Sync + std::fmt::Debug {
    /// Evaluate the curve at parameter `t` to get a 2D point.
    fn evaluate(&self, t: f64) -> Point2;

    /// Tangent vector at parameter `t`.
    fn tangent(&self, t: f64) -> Vec2;

    /// Parameter domain `(t_min, t_max)`.
    fn domain(&self) -> (f64, f64);

    /// Clone into a boxed trait object.
    fn clone_box(&self) -> Box<dyn Curve2d>;
}

impl Clone for Box<dyn Curve2d> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

// =============================================================================
// Line3d
// =============================================================================

/// A 3D line segment/ray defined by origin and direction.
///
/// Parameterization: `P(t) = origin + t * direction`
#[derive(Debug, Clone)]
pub struct Line3d {
    /// Starting point.
    pub origin: Point3,
    /// Direction (not necessarily unit length — magnitude determines speed).
    pub direction: Vec3,
}

impl Line3d {
    /// Create a line from two endpoints, parameterized so `t=0` gives `start` and `t=1` gives `end`.
    pub fn from_points(start: Point3, end: Point3) -> Self {
        Self {
            origin: start,
            direction: end - start,
        }
    }
}

impl Curve3d for Line3d {
    fn evaluate(&self, t: f64) -> Point3 {
        self.origin + t * self.direction
    }

    fn tangent(&self, _t: f64) -> Vec3 {
        self.direction
    }

    fn domain(&self) -> (f64, f64) {
        (0.0, 1.0)
    }

    fn curve_type(&self) -> CurveKind {
        CurveKind::Line
    }

    fn clone_box(&self) -> Box<dyn Curve3d> {
        Box::new(self.clone())
    }
}

// =============================================================================
// Circle3d
// =============================================================================

/// A circle in 3D space defined by center, normal, and radius.
///
/// Parameterization: `P(t) = center + radius * (cos(t) * x_dir + sin(t) * y_dir)`
///
/// Where `t ∈ [0, 2π)`.
#[derive(Debug, Clone)]
pub struct Circle3d {
    /// Center of the circle.
    pub center: Point3,
    /// Radius.
    pub radius: f64,
    /// Reference direction for t=0.
    pub x_dir: Dir3,
    /// Second in-plane direction (perpendicular to x_dir and normal).
    pub y_dir: Dir3,
    /// Normal to the circle plane.
    pub normal: Dir3,
}

impl Circle3d {
    /// Create a circle in the XY plane centered at the given point.
    pub fn new(center: Point3, radius: f64) -> Self {
        Self {
            center,
            radius,
            x_dir: Dir3::new_normalize(Vec3::x()),
            y_dir: Dir3::new_normalize(Vec3::y()),
            normal: Dir3::new_normalize(Vec3::z()),
        }
    }

    /// Create a circle with a custom normal direction.
    pub fn with_normal(center: Point3, radius: f64, normal: Vec3) -> Self {
        let n = Dir3::new_normalize(normal);
        let arbitrary = if n.as_ref().x.abs() < 0.9 {
            Vec3::x()
        } else {
            Vec3::y()
        };
        let x = Dir3::new_normalize(arbitrary.cross(n.as_ref()));
        let y = Dir3::new_normalize(n.as_ref().cross(x.as_ref()));
        Self {
            center,
            radius,
            x_dir: x,
            y_dir: y,
            normal: n,
        }
    }
}

impl Curve3d for Circle3d {
    fn evaluate(&self, t: f64) -> Point3 {
        let (sin_t, cos_t) = t.sin_cos();
        self.center + self.radius * (cos_t * self.x_dir.as_ref() + sin_t * self.y_dir.as_ref())
    }

    fn tangent(&self, t: f64) -> Vec3 {
        let (sin_t, cos_t) = t.sin_cos();
        self.radius * (-sin_t * self.x_dir.as_ref() + cos_t * self.y_dir.as_ref())
    }

    fn domain(&self) -> (f64, f64) {
        (0.0, 2.0 * PI)
    }

    fn curve_type(&self) -> CurveKind {
        CurveKind::Circle
    }

    fn clone_box(&self) -> Box<dyn Curve3d> {
        Box::new(self.clone())
    }
}

// =============================================================================
// 2D curves (for trim curves in parameter space)
// =============================================================================

/// A 2D line segment in parameter space.
#[derive(Debug, Clone)]
pub struct Line2d {
    /// Starting point.
    pub origin: Point2,
    /// Direction.
    pub direction: Vec2,
}

impl Line2d {
    /// Create from two endpoints.
    pub fn from_points(start: Point2, end: Point2) -> Self {
        Self {
            origin: start,
            direction: end - start,
        }
    }
}

impl Curve2d for Line2d {
    fn evaluate(&self, t: f64) -> Point2 {
        self.origin + t * self.direction
    }

    fn tangent(&self, _t: f64) -> Vec2 {
        self.direction
    }

    fn domain(&self) -> (f64, f64) {
        (0.0, 1.0)
    }

    fn clone_box(&self) -> Box<dyn Curve2d> {
        Box::new(self.clone())
    }
}

/// A 2D circle/arc in parameter space.
#[derive(Debug, Clone)]
pub struct Circle2d {
    /// Center of the circle.
    pub center: Point2,
    /// Radius.
    pub radius: f64,
}

impl Circle2d {
    /// Create a circle at the given center with the given radius.
    pub fn new(center: Point2, radius: f64) -> Self {
        Self { center, radius }
    }
}

impl Curve2d for Circle2d {
    fn evaluate(&self, t: f64) -> Point2 {
        let (sin_t, cos_t) = t.sin_cos();
        self.center + self.radius * Vec2::new(cos_t, sin_t)
    }

    fn tangent(&self, t: f64) -> Vec2 {
        let (sin_t, cos_t) = t.sin_cos();
        self.radius * Vec2::new(-sin_t, cos_t)
    }

    fn domain(&self) -> (f64, f64) {
        (0.0, 2.0 * PI)
    }

    fn clone_box(&self) -> Box<dyn Curve2d> {
        Box::new(self.clone())
    }
}

// =============================================================================
// Geometry store
// =============================================================================

/// Storage for all geometric entities (surfaces and curves) associated with a B-rep.
#[derive(Debug, Clone, Default)]
pub struct GeometryStore {
    /// Surfaces indexed by position (Face.surface_index refers to these).
    pub surfaces: Vec<Box<dyn Surface>>,
    /// 3D curves indexed by position.
    pub curves_3d: Vec<Box<dyn Curve3d>>,
    /// 2D trim curves indexed by position.
    pub curves_2d: Vec<Box<dyn Curve2d>>,
}

impl GeometryStore {
    /// Create an empty geometry store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a surface and return its index.
    pub fn add_surface(&mut self, surface: Box<dyn Surface>) -> usize {
        let idx = self.surfaces.len();
        self.surfaces.push(surface);
        idx
    }

    /// Add a 3D curve and return its index.
    pub fn add_curve_3d(&mut self, curve: Box<dyn Curve3d>) -> usize {
        let idx = self.curves_3d.len();
        self.curves_3d.push(curve);
        idx
    }

    /// Add a 2D trim curve and return its index.
    pub fn add_curve_2d(&mut self, curve: Box<dyn Curve2d>) -> usize {
        let idx = self.curves_2d.len();
        self.curves_2d.push(curve);
        idx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plane_evaluate() {
        let p = Plane::xy();
        let pt = p.evaluate(Point2::new(3.0, 4.0));
        assert!((pt.x - 3.0).abs() < 1e-12);
        assert!((pt.y - 4.0).abs() < 1e-12);
        assert!(pt.z.abs() < 1e-12);
    }

    #[test]
    fn test_plane_normal() {
        let p = Plane::xy();
        let n = p.normal(Point2::origin());
        assert!((n.as_ref().z - 1.0).abs() < 1e-12);
    }

    #[test]
    fn test_plane_project() {
        let p = Plane::xy();
        let pt = Point3::new(5.0, 7.0, 99.0); // z doesn't matter for projection
        let uv = p.project(&pt);
        assert!((uv.x - 5.0).abs() < 1e-12);
        assert!((uv.y - 7.0).abs() < 1e-12);
    }

    #[test]
    fn test_cylinder_evaluate() {
        let c = CylinderSurface::new(5.0);
        // u=0, v=0 should give (5, 0, 0)
        let pt = c.evaluate(Point2::new(0.0, 0.0));
        assert!((pt.x - 5.0).abs() < 1e-12);
        assert!(pt.y.abs() < 1e-12);
        assert!(pt.z.abs() < 1e-12);
        // u=PI/2, v=3 should give (0, 5, 3)
        let pt2 = c.evaluate(Point2::new(PI / 2.0, 3.0));
        assert!(pt2.x.abs() < 1e-12);
        assert!((pt2.y - 5.0).abs() < 1e-12);
        assert!((pt2.z - 3.0).abs() < 1e-12);
    }

    #[test]
    fn test_sphere_evaluate() {
        let s = SphereSurface::new(10.0);
        // u=0, v=0 (equator, at x-axis) -> (10, 0, 0)
        let pt = s.evaluate(Point2::new(0.0, 0.0));
        assert!((pt.x - 10.0).abs() < 1e-12);
        assert!(pt.y.abs() < 1e-12);
        assert!(pt.z.abs() < 1e-12);
        // North pole: v=PI/2
        let north = s.evaluate(Point2::new(0.0, PI / 2.0));
        assert!(north.x.abs() < 1e-10);
        assert!(north.y.abs() < 1e-10);
        assert!((north.z - 10.0).abs() < 1e-10);
    }

    #[test]
    fn test_line3d() {
        let line = Line3d::from_points(Point3::origin(), Point3::new(10.0, 0.0, 0.0));
        let mid = line.evaluate(0.5);
        assert!((mid.x - 5.0).abs() < 1e-12);
    }

    #[test]
    fn test_circle3d() {
        let circle = Circle3d::new(Point3::origin(), 5.0);
        let pt = circle.evaluate(0.0);
        assert!((pt.x - 5.0).abs() < 1e-12);
        let pt90 = circle.evaluate(PI / 2.0);
        assert!(pt90.x.abs() < 1e-12);
        assert!((pt90.y - 5.0).abs() < 1e-12);
    }

    #[test]
    fn test_plane_transform() {
        let p = Plane::xy();
        let t = Transform::translation(0.0, 0.0, 5.0);
        let p2 = p.transform(&t);
        let pt = p2.evaluate(Point2::new(1.0, 2.0));
        assert!((pt.x - 1.0).abs() < 1e-12);
        assert!((pt.y - 2.0).abs() < 1e-12);
        assert!((pt.z - 5.0).abs() < 1e-12);
    }

    #[test]
    fn test_cylinder_transform() {
        let c = CylinderSurface::new(5.0);
        let t = Transform::translation(10.0, 0.0, 0.0);
        let c2 = c.transform(&t);
        // u=0, v=0 on original = (5,0,0), after translate = (15,0,0)
        let pt = c2.evaluate(Point2::new(0.0, 0.0));
        assert!((pt.x - 15.0).abs() < 1e-10);
        assert!(pt.y.abs() < 1e-10);
    }

    #[test]
    fn test_sphere_transform_scale() {
        let s = SphereSurface::new(5.0);
        let t = Transform::scale(2.0, 2.0, 2.0);
        let s2 = s.transform(&t);
        // u=0, v=0 on original = (5,0,0), after 2x scale = (10,0,0)
        let pt = s2.evaluate(Point2::new(0.0, 0.0));
        assert!((pt.x - 10.0).abs() < 1e-10);
    }

    #[test]
    fn test_geometry_store() {
        let mut store = GeometryStore::new();
        let idx = store.add_surface(Box::new(Plane::xy()));
        assert_eq!(idx, 0);
    }

    #[test]
    fn test_torus_evaluate() {
        let torus = TorusSurface::new(10.0, 3.0); // R=10, r=3
        // u=0, v=0: outer equator, at (R+r, 0, 0) = (13, 0, 0)
        let pt = torus.evaluate(Point2::new(0.0, 0.0));
        assert!((pt.x - 13.0).abs() < 1e-10);
        assert!(pt.y.abs() < 1e-10);
        assert!(pt.z.abs() < 1e-10);

        // u=0, v=π: inner equator, at (R-r, 0, 0) = (7, 0, 0)
        let pt_inner = torus.evaluate(Point2::new(0.0, PI));
        assert!((pt_inner.x - 7.0).abs() < 1e-10);
        assert!(pt_inner.y.abs() < 1e-10);
        assert!(pt_inner.z.abs() < 1e-10);

        // u=π/2, v=0: at (0, R+r, 0) = (0, 13, 0)
        let pt_y = torus.evaluate(Point2::new(PI / 2.0, 0.0));
        assert!(pt_y.x.abs() < 1e-10);
        assert!((pt_y.y - 13.0).abs() < 1e-10);
        assert!(pt_y.z.abs() < 1e-10);

        // u=0, v=π/2: top of tube, at (R, 0, r) = (10, 0, 3)
        let pt_top = torus.evaluate(Point2::new(0.0, PI / 2.0));
        assert!((pt_top.x - 10.0).abs() < 1e-10);
        assert!(pt_top.y.abs() < 1e-10);
        assert!((pt_top.z - 3.0).abs() < 1e-10);
    }

    #[test]
    fn test_torus_normal() {
        let torus = TorusSurface::new(10.0, 3.0);
        // At u=0, v=0 (outer equator), normal should point in +x direction
        let n = torus.normal(Point2::new(0.0, 0.0));
        assert!((n.as_ref().x - 1.0).abs() < 1e-10);
        assert!(n.as_ref().y.abs() < 1e-10);
        assert!(n.as_ref().z.abs() < 1e-10);

        // At u=0, v=π/2 (top of tube at x-axis), normal should point in +z
        let n_top = torus.normal(Point2::new(0.0, PI / 2.0));
        assert!(n_top.as_ref().x.abs() < 1e-10);
        assert!(n_top.as_ref().y.abs() < 1e-10);
        assert!((n_top.as_ref().z - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_torus_transform() {
        let torus = TorusSurface::new(10.0, 3.0);
        let t = Transform::translation(100.0, 0.0, 0.0);
        let torus2 = torus.transform(&t);
        let pt = torus2.evaluate(Point2::new(0.0, 0.0));
        assert!((pt.x - 113.0).abs() < 1e-10);
    }

    #[test]
    fn test_torus_partials() {
        let torus = TorusSurface::new(10.0, 3.0);
        // Verify partials by finite difference
        let uv = Point2::new(0.5, 0.3);
        let eps = 1e-7;

        let p0 = torus.evaluate(uv);
        let pu = torus.evaluate(Point2::new(uv.x + eps, uv.y));
        let pv = torus.evaluate(Point2::new(uv.x, uv.y + eps));

        let d_du_fd = (pu - p0) / eps;
        let d_dv_fd = (pv - p0) / eps;

        let d_du = torus.d_du(uv);
        let d_dv = torus.d_dv(uv);

        assert!((d_du.x - d_du_fd.x).abs() < 1e-4);
        assert!((d_du.y - d_du_fd.y).abs() < 1e-4);
        assert!((d_du.z - d_du_fd.z).abs() < 1e-4);
        assert!((d_dv.x - d_dv_fd.x).abs() < 1e-4);
        assert!((d_dv.y - d_dv_fd.y).abs() < 1e-4);
        assert!((d_dv.z - d_dv_fd.z).abs() < 1e-4);
    }
}
